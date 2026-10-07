# `br/pkg/summary/summary.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-summary`，包入口是同目录的 `lib.rs`。`lib.rs` 将本模块声明为 `summary`，并把这里的 12 个公开函数重新导出到 crate 根，因此使用方通常通过 `astersql_br_pkg_summary::CollectInt` 等路径调用，而不必写出 `summary::summary::*`。

它是包级便捷 API 层，不实现摘要字段的聚合和格式化算法。真正的可变状态、日志回调以及成功/失败摘要生成都位于 `br/pkg/summary/collector.rs` 的 `LogCollector`、`logCollector` 和全局 `COLLECTOR` 中。本文件与 Go 的 `br/pkg/summary/summary.go` 同位、同职责。

`br/pkg/summary/Cargo.toml` 将该 crate 定义为 library，并以 `br/pkg/summary` 为 Go 对照包；本文件只直接使用标准库和同 crate 的 `collector` 模块。Cargo 中的日志、错误和格式化依赖由 `collector.rs` 消费，而不是在此门面中展开。

## 核心职责

本文件承担三类职责：

1. 为全局 `LogCollector` 提供与 Go 包函数对应的稳定入口，包括任务单位设置、成功/失败采集、普通字段累加、摘要输出和直接日志输出。
2. 通过 `with_collector` 或 `with_collector_result` 串行访问进程级收集器，使调用者不必持有或传递收集器实例。
3. 用独立的原子变量 `LAST_STATUS` 保存最近一次 `SetSuccessStatus` 的参数，使 `Succeed` 不依赖收集器内部状态或锁。

它不负责初始化或替换全局收集器；这两个入口是 `collector.rs` 的 `InitCollector` 和 `SetLogCollector`。它也不解释字段语义、选择摘要模板或执行错误去重，这些行为均由被转发到的 `LogCollector` 实现决定。

## 主要符号

- `LAST_STATUS: AtomicBool`：本文件唯一的模块级状态，初始为 `false`，使用顺序一致性原子序保存最近成功标志。
- `SetUnit(unit: &str)`：转发到 `LogCollector::SetUnit`，保存诸如 `backup`、`restore` 的任务单位。
- `CollectSuccessUnit(name: &str, unit_count: i32, arg: SummaryValue)`：转发成功单元。`SummaryValue::Duration` 和 `SummaryValue::UInt64` 的具体累加规则由 `collector.rs` 实现。
- `CollectFailureUnit(name: &str, reason: SummaryError)`：转发失败原因；“同名失败只保留首次”是下游 `logCollector::CollectFailureUnit` 的规则。
- `CollectDuration(name: &str, t: Duration)`、`CollectInt(name: &str, t: i32)`、`CollectUint(name: &str, t: u64)`：分别转发命名耗时、有符号计数和无符号计数。`CollectUint` 特意映射到 trait 中拼作 `CollectUInt` 的方法。
- `SetSuccessStatus(success: bool)`：先写 `LAST_STATUS`，再把同一值写入全局收集器；这是本文件唯一包含两个状态写入步骤的函数。
- `Succeed() -> bool`：只读取 `LAST_STATUS`，不取得 `COLLECTOR` 的互斥锁。
- `NowDureTime() -> Duration`：通过 `with_collector_result` 返回收集器自 `start_time` 起经过的时间；函数名保留 Go 版本的 `Dure` 拼写。
- `AdjustStartTimeToEarlierTime(t: Duration)`：要求收集器把起点前移 `t`，用于将调用本模块之前的阶段计入总耗时。
- `Summary(name: &str)`：请求收集器输出名为 `name` 的成功或失败摘要并执行其清理逻辑。
- `Log(msg: &str, fields: &[Field])`：把消息和字段切片直接交给已注入的日志回调，不修改聚合字段。

本文件没有 trait、结构体、枚举、条件编译项或私有函数；除 `LAST_STATUS` 外的全部定义都是公开函数。

## 执行流程

典型生命周期由外部入口先调用 `InitCollector`，业务代码再经本文件收集指标，结束时设置状态并输出摘要：

1. `br/cmd/br/cmd.rs` 中可见 `summary::InitCollector(...)` 初始化全局实现；该函数来自同 crate 的 `collector.rs`，不是本文件。
2. 命令入口可用 `SetUnit` 标记 `BackupUnit` 或 `RestoreUnit`；`br/cmd/br/backup.rs` 和 `br/cmd/br/restore.rs` 展示了同名 API 的预期接入位置，但它们当前经命令 crate 的 `stubs::summary` 路径导入，不能仅凭名称认定为本文件的直接调用边。
3. 已确认直接依赖此 crate 的 `br/pkg/utils/misc.rs` 会用 `CollectSuccessUnit` 汇总 KV/字节数，并用 `CollectInt` 记录各 CF 文件数；`br/pkg/metautil/metafile.rs` 也调用 `CollectSuccessUnit`。
4. 每个转发函数在 `with_collector` 持锁闭包内调用同名 trait 方法。`NowDureTime` 使用带返回值的 `with_collector_result`；`Succeed` 是例外，只读原子变量。
5. 业务完成后应调用 `SetSuccessStatus(true)`，再由生命周期外层调用 `Summary(name)` 输出最终摘要。若没有设置成功，collector 的默认 `success_status == false` 会选择失败摘要路径。
6. `Summary` 的下游实现生成字段、调用 logger，并清空 durations、ints、success_costs 和 failure_reasons；这些动作发生在 `collector.rs`，本文件只在锁内发起调用。

需要特别注意调用时序：如果先 `Summary` 再设置成功状态，刚输出的摘要仍按调用 `Summary` 当时的 collector 状态选择模板。`LAST_STATUS` 也不会被 `Summary` 清除，所以 `Succeed` 表示最近一次显式设置的值，而不是“最近一次摘要已成功写出”。

## 数据与状态

门面层自己只拥有 `LAST_STATUS`。它与 `logCollector.success_status` 在 `SetSuccessStatus` 中双写，但用途不同：前者服务于无锁查询 `Succeed`，后者决定 `logCollector::Summary` 选择成功或失败模板。没有事务把两次写入合并为不可分割操作；正常返回后两者相同，但若持有 collector 锁时发生 panic，原子值可能已更新而 collector 尚未更新。

所有聚合数据都由 `collector.rs` 的全局 `COLLECTOR: LazyLock<Mutex<Box<dyn LogCollector>>>` 所有，包括成功/失败计数、失败原因、duration/int/uint map、成功数据、起始时间和 logger。字符串参数在下游按需复制为 map key；`Field` 切片只在 `Log` 调用期间借用。

`SummaryValue` 限定成功值为 `Duration` 或 `UInt64`。`SummaryError` 是可在线程间共享的错误对象。这些类型由 `collector.rs` 定义，经 `lib.rs` 导出；本文件只在签名中使用，不改变其内容。

## 依赖与调用关系

向下依赖全部来自 `super::collector`：

- `with_collector`：承载 10 个无返回值的 collector 调用。
- `with_collector_result`：承载 `NowDureTime` 的返回值。
- `LogCollector`：让闭包可以通过 trait 对象调用收集器方法；本文件没有构造具体实现。
- `Field`、`SummaryError`、`SummaryValue`：分别构成日志、失败和成功值的公共参数类型。

向上暴露由 `br/pkg/summary/lib.rs` 的 `pub use summary::{...}` 完成。RustCodeGraph 对目标文件给出的文件级反向关系包括 `br/pkg/utils/misc.rs`，另列出的两个 RealTiKV 测试文件属于索引的文件使用关系；对 re-export 后的细粒度跨 crate 函数调用，图查询没有完整展开。因此本文对调用者的结论还用 Cargo 依赖和 `rg` 直接引用核验：`br/pkg/utils/Cargo.toml`、`br/pkg/metautil/Cargo.toml` 明确依赖 `astersql-br-pkg-summary`，对应源码分别调用这里导出的采集函数。

仓库中还有多组同名 `summary` 桩，例如 `br/pkg/task/stubs.rs`、`br/pkg/restore/stubs.rs`、`br/pkg/backup/stubs.rs` 和 `br/pkg/restore/snap_client/stubs.rs`。这些符号不能自动视为本文件的调用者；扩展或迁移时必须沿各 crate 的实际 import/Cargo 依赖逐一确认，避免把桩行为误写成全局 collector 行为。

## 错误处理与边界

这些 API 不返回 `Result`。业务错误以 `SummaryError` 数据传给 `CollectFailureUnit`，由 collector 保存并在失败摘要中记录；本文件不捕获、不转换该错误。

`with_collector*` 对 poisoned mutex 使用 `expect("summary collector mutex poisoned")`，因此此前在持锁区间发生 panic 后，后续门面调用也会 panic。logger 回调在 `Summary`/`Log` 持有全局锁时执行；logger 若 panic，同样会毒化互斥锁。

数值和时间的累加边界由下游实现决定。本门面没有检查负的 `unit_count`、`i32` 溢出、极大的 `Duration` 或 `u64`，也不校验名称为空。`AdjustStartTimeToEarlierTime` 最终执行 `Instant -= Duration`，过大的偏移可能触及平台时间表示边界；调用者应只传入真实的前置阶段耗时。

`CollectFailureUnit` 的去重、context-canceled 的特殊处理、字段名空格转连字符以及成功/失败字段集合均不是本文件自身保证，而是当前 `collector.rs` 实现及其测试覆盖的下游契约。

## 并发与资源生命周期

除 `Succeed` 外，所有函数都同步取得同一个进程级 `Mutex<Box<dyn LogCollector>>`，所以聚合修改和摘要输出彼此串行，API 可从多个线程调用但高频采集会竞争同一把锁。没有异步任务、channel 或后台 worker；调用在当前线程完成。

`LAST_STATUS` 使用 `Ordering::SeqCst`，提供跨线程的单一全序读写。它与 mutex 保护的 `success_status` 是两个同步域；调用方不应把一次并发的 `Succeed` 读取当作另一线程已完成 collector 更新的通知机制。

全局 collector 由 `LazyLock` 首次访问时创建并存活到进程结束。`InitCollector`/`SetLogCollector` 会整体替换它。`Summary` 只清理 collector 的部分聚合 map；根据 `collector.rs` 当前实现，计数、`success_data`、`success_status`、`start_time`、`uints` 和本文件的 `LAST_STATUS` 不会全部复位。因此复用同一全局实例开展逻辑上独立的多次任务前，应核对初始化/替换流程，不能假定一次 `Summary` 等价于全量重置。

## 与 Go 版本的对应关系

`br/pkg/summary/summary.go` 与本文件逐函数对应：Go 包级函数调用全局 `collector`，Rust 函数通过 `with_collector*` 调用 trait 对象；Go 的 `atomic.Bool` 对应 Rust 的 `AtomicBool`。`SetSuccessStatus` 都先保存 `lastStatus`，再设置 collector；`Succeed` 都只返回独立状态；`CollectUint` 都转发到命名为 `CollectUInt` 的 collector 方法。

签名适配主要有：Go 的 `any` 被收窄为 Rust `SummaryValue`，Go 的 `error` 变为 `SummaryError`，Go 的可变参数 `...zap.Field` 变为借用切片 `&[Field]`，Go `int` 明确为 Rust `i32`。这些差异增强了 Rust 调用点的静态约束，也意味着移植新调用时必须显式构造枚举或错误对象，不能照搬 Go 的任意值。

Go 的包级 collector 如何保证并发安全由 `collector.go` 自己处理；Rust 把同步集中在 `with_collector*` 的全局 mutex。Rust 另有独立 `br/pkg/summary/parity_test.rs`，覆盖 Go/Rust 公共契约、失败去重、取消错误识别、摘要后部分状态清理和 `Succeed`；`collector_test.rs` 对照 Go `collector_test.go` 的 duration/int 累加。`main_test.rs` 只保留 common setup，未复刻 Go `goleak.VerifyTestMain`，因为 Rust 测试环境没有该 goroutine 泄漏模型。

## 扩展指南

新增一种包级操作时，应先在 `LogCollector` trait 和 `logCollector` 实现中建立真实语义，再在本文件添加最薄的锁内转发，并在 `lib.rs` re-export；不要把聚合算法塞进门面。若新增值种类，还要同步 `SummaryValue`、摘要字段生成和 Go 对照语义。

涉及成功状态时必须同时评估 `LAST_STATUS` 与 collector 内 `success_status`，并明确 `Summary` 后是否重置。若要改变这两个状态的一致性模型，需要增加并发测试，而不是仅修改原子 ordering。

测试应继续放在独立文件中：门面公共契约和 Go 差异优先扩展 `br/pkg/summary/parity_test.rs`，collector 聚合规则扩展 `br/pkg/summary/collector_test.rs`；若 Go 行为也改变，应同步审阅 `br/pkg/summary/summary.go`、`collector.go` 及 `collector_test.go`。不要把 Rust 测试内嵌回 `summary.rs`。

接线新调用者时，先在调用 crate 的 Cargo manifest 增加或确认 `astersql-br-pkg-summary` 依赖，再导入 crate 根 re-export。仓库现有同名桩必须显式判定是待替换适配层还是有意隔离；直接把名字相同视为已经接线会产生行为误判。高频路径还应评估全局 mutex 竞争，日志回调不得递归调用这些门面函数，否则会在不可重入 mutex 上死锁。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter br/pkg/summary` 确认目标、Go 对照及独立测试均在索引中；`node --file br/pkg/summary/summary.rs --offset 1 --limit 260` 读取了目标文件全部 101 行并报告文件使用关系；对 `SetSuccessStatus`、`Summary`、`CollectSuccessUnit`、`CollectFailureUnit`、`NowDureTime`、`Log` 执行了 `query`/`callers`/`callees` 查询。常见函数名存在跨 Go/Rust/桩歧义，因此没有把未消歧的图结果当作直接调用证据。
- Rust 源码：完整读取 `br/pkg/summary/summary.rs`；读取 `br/pkg/summary/lib.rs` 验证模块声明和 re-export；读取 `br/pkg/summary/collector.rs` 的 trait、全局锁、具体实现、摘要清理及测试重置逻辑。
- crate 与调用点：读取 `br/pkg/summary/Cargo.toml`；以 `br/pkg/utils/Cargo.toml`、`br/pkg/utils/misc.rs`、`br/pkg/metautil/Cargo.toml`、`br/pkg/metautil/metafile.rs` 核验直接依赖和采集调用；以 `rg` 检查 BR 范围内同名 API，并识别各子模块 stubs 的边界。
- Go 对照：完整读取 `br/pkg/summary/summary.go`、`collector_test.go`、`main_test.go`；Rust 测试读取 `br/pkg/summary/collector_test.rs`、`main_test.rs`、`parity_test.rs`。目标目录不存在 `doc.go`。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付结构检查要求本文恰好包含任务指定的 11 个二级标题；事实复核以源码、索引、Cargo/Go 文件和独立测试交叉完成。

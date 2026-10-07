# `br/pkg/summary/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-summary` 的 crate 根文件；`br/pkg/summary/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它设为库入口，并用 `package.metadata.porting.go-package = "br/pkg/summary"` 标明 Go 对照包。它本身不实现聚合算法，而是用 `#[path = "collector.rs"]` 和 `#[path = "summary.rs"]` 组装两个实现模块，再把稳定 API 重导出到 crate 根。调用者因此可以写 `use astersql_br_pkg_summary::{Summary, SetSuccessStatus, ...}`，不必依赖内部模块布局。

该文件属于 BR 的进程级摘要/观测边界：CLI 初始化日志时创建收集器，backup/restore 命令设置业务单元，任务和数据路径持续采样，最终由 `Summary` 输出成功或失败摘要。它不是 SQL statement summary，也不是 `br/pkg/*/stubs.rs` 中的局部替身。

## 核心职责

1. 声明公开实现模块 `collector` 与 `summary`。前者持有聚合状态、日志字段和格式化逻辑，后者提供操作全局收集器的包级薄包装。
2. 将 `LogCollector`、`NewLogCollector`、`InitCollector`、字段常量、错误/值类型及包级 `Collect*`、`SetSuccessStatus`、`Succeed`、`Summary` 等统一重导出，形成与 Go `br/pkg/summary` 接近的扁平 API。
3. 在 `cfg(test)` 下挂载 `parity_test.rs`、`collector_test.rs`、`main_test.rs` 三个独立测试模块，保持生产源与测试源分离。
4. 通过 crate 级 `allow` 接受移植代码保留的 Go 命名（如 `SetUnit`、`TotalKV`）及当前未使用项。这是兼容策略，不代表所有 API 都已被生产调用。

## 主要符号

- `pub mod collector`：真实聚合实现入口。其公开核心包括 `LogCollector: Send`、`NewLogCollector(LogFunc) -> Box<dyn LogCollector>`、`InitCollector(bool)`、`SetLogCollector`、`Field`/`FieldValue`、`SummaryValue` 和 `SummaryError`。
- `pub mod summary`：全局便捷函数入口。`SetUnit`、`CollectSuccessUnit`、`CollectFailureUnit`、`CollectDuration`、`CollectInt`、`CollectUint`、`NowDureTime`、`AdjustStartTimeToEarlierTime`、`Summary`、`Log` 均转发给全局 `LogCollector`。
- `BackupUnit` / `RestoreUnit`：CLI 预执行阶段使用的业务单元标签，值分别为 `"backup"` 和 `"restore"`。
- `TotalKV`、`TotalBytes`、`BackupDataSize`、`RestoreDataSize`、`SkippedKVCountByCheckpoint`、`SkippedBytesByCheckpoint`：约定字段名；`collector.rs::Summary` 对部分字段执行 human-size、平均速度或 checkpoint 展示转换。
- `SetSuccessStatus(bool)` / `Succeed() -> bool`：前者同时更新全局 collector 与 `summary.rs::LAST_STATUS`，后者以 `SeqCst` 原子读取最近状态，即使摘要已输出也可查询。
- `Summary(name)`：要求调用方先采集并明确设置成功状态；实现根据失败原因或 `success_status` 选择 `"<name> failed summary"` / `"<name> success summary"`。
- 三个 `#[cfg(test)]` 模块只在测试构建存在，不进入生产 API；源码中没有条件 feature 分支。

## 执行流程

1. BR CLI 初始化日志。`br/cmd/br/cmd.rs::Init` 在配置了日志文件时调用 `summary::InitCollector(true)`；测试复位路径调用 `InitCollector(false)`。该函数构造新的 `logCollector` 并替换进程级全局实例。
2. 命令树设置分类。`br/cmd/br/backup.rs::NewBackupCommand` 的 `PersistentPreRunE` 调用 `SetUnit(BackupUnit)`；`br/cmd/br/restore.rs::NewRestoreCommand` 对应调用 `SetUnit(RestoreUnit)`。
3. 任务执行期间，调用者通过 crate 根重导出的 `Collect*` 累加 range、耗时、整数、字节数或失败原因。例如 `br/pkg/restore/snap_client/import.rs::Import` 对 `TotalKV`/`TotalBytes` 调用 `CollectSuccessUnit`，`br/pkg/task/backup.rs::run_backup_body` 在成功末尾调用 `SetSuccessStatus(true)`。
4. 每个包级函数经 `with_collector` 或 `with_collector_result` 获取全局 `Mutex<Box<dyn LogCollector>>`，再调用 trait 方法。`CollectSuccessUnit` 的 `Duration` 分支累加成功单元数与耗时，`UInt64` 分支只累加数据量；失败单元按名称去重并保存首次错误。
5. `Summary(name)` 先加入 range 总数、成功数、失败数以及普通 duration/int/uint 字段。存在失败原因或未设置成功时走失败模板，并把真正的 `ContextCanceled` 从普通错误明细中分流；否则加入总耗时，对已知字节字段做格式化，再输出成功模板。
6. 输出后清空 duration、int、success-cost 和 failure-reason 映射。调用者可继续采集下一轮，但下文“数据与状态”列出的未清空状态必须由新的 collector 生命周期或调用约定隔离。

## 数据与状态

真正的可变状态位于 `collector.rs::logCollector`：`unit`、成功/失败单元计数、`success_costs`、`success_data`、`failure_reasons`、`durations`、`ints`、`uints`、`success_status`、`start_time` 与注入的 `LogFunc`。crate 根只负责公开这些能力，不另存状态。

进程级 `COLLECTOR` 是 `LazyLock<Mutex<Box<dyn LogCollector>>>`。`InitCollector` 或 `SetLogCollector` 会整体替换实例；`NewLogCollector` 的 `start_time` 取构造时刻。`summary.rs::LAST_STATUS` 是独立的 `AtomicBool`，由 `SetSuccessStatus` 与 collector 状态双写，`Succeed` 只读该原子值。

需特别注意重置范围：当前 `Summary` 只清空 `durations`、`ints`、`success_costs` 和 `failure_reasons`；不会清空 `uints`、`success_data`、成功/失败计数、`success_status`、`unit` 或重置 `start_time`。这是 Rust 当前实现与 Go `collector.go` 的现有行为；若在同一实例上连续汇总，不能假定所有指标从零开始。

## 依赖与调用关系

上游主链证据如下：

- `br/cmd/br/cmd.rs::Init` → `InitCollector(true)`，建立与日志配置匹配的全局收集器。
- `br/cmd/br/backup.rs::NewBackupCommand` / `br/cmd/br/restore.rs::NewRestoreCommand` → `SetUnit`，区分 backup/restore。
- RustCodeGraph 的 `Summary`/`SetSuccessStatus` 调用图显示 `br/pkg/task/restore.rs`、`restore_raw.rs`、`restore_txn.rs`、`stream.rs` 等任务入口使用包级 API；`br/pkg/restore/restorer.rs::WaitUntilFinish`、`br/pkg/restore/snap_client/import.rs::Import` 等数据路径采集成功单元。
- `br/pkg/task/backup.rs::run_backup_body` 在成功收尾调用 `SetSuccessStatus(true)`；`br/pkg/task/restore.rs::RunRestore` 在完成文件恢复后同样设置成功状态。

下游关系是 `lib.rs` 重导出 → `summary.rs` 薄包装 → `collector.rs::{with_collector, with_collector_result}` → `LogCollector` trait → 默认 `logCollector`。`Summary` 进一步使用 `zap` 风格的本地 `Field` 构造器、`units::HumanSize` 与注入的 `LogFunc`。

Cargo 清单声明这是无 feature 的 library crate，并列出 `astersql-br-pkg-logutil`、`astersql-errors`、`bytesize`、`tracing` 依赖及测试用 `astersql-testkit-testsetup`。当前 `lib.rs` 及两份实现的可见代码主要使用标准库内建替代层；不能仅凭 manifest 依赖推断某个外部日志后端已在此文件直接接线。

## 错误处理与边界

- 聚合 API 大多不返回 `Result`。全局 collector 或日志锁中毒时使用 `expect` 触发 panic；调用方不能从函数返回值恢复。
- `InitCollector(true)` 的本地 `InitLogger` 初始化失败会回退默认日志函数，不中断启动。默认全局日志在隔离环境中是 no-op，因此“调用成功”不保证外部可见日志。
- `CollectFailureUnit` 对同名失败只记首次错误并只增加一次失败计数；测试明确验证第二次错误不会覆盖第一次。
- 取消识别按错误 source 链中的 `ContextCanceled` 类型判断，而不是按错误文本判断；显示文本同为 `"context canceled"` 的其他错误仍属于普通失败。
- `SummaryValue` 只允许 `Duration` 与 `UInt64`，比 Go `any` 更封闭；扩展新变体需要同步 trait 实现、包级 API、Go 语义对照和测试。
- `Summary` 的成功条件要求 `success_status == true` 且没有失败原因。`BackupDataSize`/`RestoreDataSize` 内的 “Nothing to ...” 分支还检查 `!success_status`，但外层已在此条件下提前进入失败路径，按当前控制流该内部分支不可达；文档不将其描述成已验证的成功输出。
- 平均速度使用 `total_dure_time.as_secs_f64()` 作除数；极短时长可能产生非有限浮点值，`HumanSize` 会格式化 `NaN`/`±Inf`，而不是返回错误。

## 并发与资源生命周期

`LogCollector` 要求 `Send`，全局 trait object 被单个 `Mutex` 保护；每次包级采集、查询或输出都持有该互斥锁，因而同一进程内对 collector 状态的访问串行化。注入的 `LogFunc` 要求 `Send + Sync`，错误对象为 `Arc<dyn Error + Send + Sync>`，可跨线程传递。

锁覆盖整个 trait 调用，包括 `Summary` 中的字段构建和日志回调。自定义 `LogFunc` 不应同步回调包级 `Collect*`/`Summary`，否则会尝试重入同一非重入 `Mutex` 并死锁；长时间日志处理也会阻塞所有采集者。`Succeed` 不取 collector 锁，使用 `SeqCst` 原子访问。

collector 从 `InitCollector`/首次惰性初始化存活到被替换或进程结束，没有后台任务、通道或显式关闭接口。测试通过构造独立 collector 或 `reset_global_collector_for_test` 隔离全局状态；生产调用方若要开始完全独立的新任务，应确认是否需要重新初始化，而不能把一次 `Summary` 当作完整重置。

## 与 Go 版本的对应关系

`lib.rs` 将 Rust 的两个源文件重新组合成 Go 单一 package 的公开表面：`collector.rs` 对应 `br/pkg/summary/collector.go`，`summary.rs` 对应 `br/pkg/summary/summary.go`。常量值、`LogCollector` 方法集、包级转发函数、首次失败保留、字段名空格替换、成功/失败摘要标题及 `lastStatus` 原子语义均有直接对照。

关键语言适配包括：Go 的 `any` 参数被收窄为 `SummaryValue`；Go 的 `error` 变为可共享的 `SummaryError`；Go collector 内部 `sync.Mutex` 被提升为包围 trait object 的全局 Rust `Mutex`；Go zap 字段和 docker human-size 由本地 `Field`/`FieldValue`、`zap`、`units` 兼容实现承担。Go 使用 `errors.Cause(reason) == context.Canceled`，Rust 沿 source 链按 `ContextCanceled` 具体类型识别。

测试对照也保持独立文件：`collector_test.rs::test_sum_duration_int` 对应 Go `TestSumDurationInt`；`main_test.rs::test_main_setup` 保留 common test setup，但 Rust 没有 Go `goleak.VerifyTestMain` 的等价泄漏检查；`parity_test.rs::go_rust_public_contract_matches` 额外覆盖失败去重、取消分流、局部重置、键格式化和 human-size 行为。

## 扩展指南

- 新增公开指标常量或 collector 能力时，应先修改 `collector.rs` 的 trait、默认实现及必要数据字段，再在 `summary.rs` 增加全局薄包装，最后由本 `lib.rs` 重导出；避免让业务调用者绕过 crate 根依赖私有布局。
- 新增 `SummaryValue` 类型或字段格式化分支时，必须核对 Go `collector.go` 的 type switch/输出键，并在独立的 `collector_test.rs` 或 `parity_test.rs` 添加累加、输出与边界断言。
- 修改成功判定、取消识别或 Summary 重置范围时，要同时验证失败去重、`Succeed`、连续两次 Summary 和并发调用；这类变化会影响所有 BR 命令的最终可观测结果，兼容风险高于普通内部重构。
- 注入新日志后端时保持 `LogFunc: Send + Sync`，避免在回调内重入本 crate，并评估锁内 I/O 对备份/恢复热路径的延迟影响。
- 测试必须继续放在独立文件中。公开契约放 `parity_test.rs`，collector 聚合细节放 `collector_test.rs`，全局测试初始化放 `main_test.rs`；不要把测试内嵌回 `lib.rs`。
- 若增加直接依赖或 feature，同步更新 `br/pkg/summary/Cargo.toml`，并重新确认 crate 根的 `allow` 是否仍有必要；不要以当前未使用的 manifest 依赖作为实现已经接线的证据。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter br/pkg/summary` 列出 10 个 Go/Rust 源与测试文件。
- RustCodeGraph 源码/符号读取：`br/pkg/summary/lib.rs`（模块声明、重导出、三个测试模块），`collector.rs`（常量、trait、全局锁、默认实现、Summary 分支与清理），`summary.rs`（薄包装与 `LAST_STATUS`）。
- RustCodeGraph 调用证据：`explore "br/pkg/summary/lib.rs InitCollector SetUnit Summary CollectSuccessUnit SetSuccessStatus"`；并读取 `br/cmd/br/cmd.rs::Init`、`br/cmd/br/backup.rs::NewBackupCommand`、`br/cmd/br/restore.rs::NewRestoreCommand`、`br/pkg/task/backup.rs::RunBackup/run_backup_body`、`br/pkg/task/restore.rs::RunRestore` 的调用现场。
- crate 边界：`br/pkg/summary/Cargo.toml`。
- Go 对照：`br/pkg/summary/collector.go`、`br/pkg/summary/summary.go`。
- Rust 独立测试：`br/pkg/summary/collector_test.rs`、`br/pkg/summary/parity_test.rs`、`br/pkg/summary/main_test.rs`；Go 对照测试：`br/pkg/summary/collector_test.go`、`br/pkg/summary/main_test.go`。
- 按任务约束未运行 Cargo；交付只执行 Markdown 结构检查和差异自审。

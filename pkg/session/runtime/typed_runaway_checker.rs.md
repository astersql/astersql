# `pkg/session/runtime/typed_runaway_checker.rs`

## 文件定位

[`typed_runaway_checker.rs`](typed_runaway_checker.rs) 是 `astersql-session` crate 的私有 `runtime::typed_runaway_checker` 模块，由 [`runtime.rs`](../runtime.rs) 的 `mod typed_runaway_checker;` 装配。它不实现 runaway 判定算法，而是在会话侧把 `astersql-resourcegroup-runaway` 的规范 `Checker` 适配为 `astersql-kv` 定义的、可跨存储请求链传递的 `RunawayChecker` trait object。

唯一生产入口位于 [`scan_adapter_runtime.rs`](scan_adapter_runtime.rs) 的 `SessionBoundAdapterOwner::RunawayBeforeExecutor`：语句编译后先从 Domain 的 runaway manager 派生并执行规范 checker，再用本文件的 `SessionRunawayChecker` 包装同一个 `Arc<Checker>`，写入会话状态供后续 DistSQL/KV 请求使用。crate 归属和 Go 包映射由 [`pkg/session/Cargo.toml`](../Cargo.toml) 的 `name = "astersql-session"`、`[lib] path = "lib.rs"` 与 `package.metadata.porting.go-package = "pkg/session"` 确认；`nextgen` feature 不改变本文件。

## 核心职责

1. 在 `kv::resourcegroup::CopRequest` 与 `runaway::CopRequest` 之间双向转换优先级、资源组和最大执行时长，使规范 checker 能在 Cop 请求发出前实施 Kill、CoolDown、SwitchGroup 或截止时间约束。
2. 把 KV 层的可选 RU、`u64` processed keys 和字符串错误转换成规范 checker 的 `RUDetails`、`i64` 计数和 `runaway::Error`，再把“新增 runaway 错误”转换回 KV trait 的 `Result<(), String>`。
3. 将规范动作投影到 KV 层只认识的 `None`、`CoolDown`、`Kill` 三态，并透传 processed-keys 累计器重置。
4. 保持同一个 `Arc<runaway::checker::Checker>` 被执行器侧和 KV 侧共享，因此执行前 watch 命中、后续阈值累计和动作查询观察的是同一查询状态。

## 主要符号

- `SessionRunawayChecker(pub(super) Arc<runaway::checker::Checker>)`：文件唯一自定义类型。结构体及字段都只在 `runtime` 父模块可见，外部 crate 只能经 `kv::resourcegroup::SharedRunawayChecker` 使用它。
- `impl kv::resourcegroup::RunawayChecker for SessionRunawayChecker`：文件唯一实现块。目标 trait 要求 `Send + Sync`；包装的规范 checker以原子字段维护并发可变状态。
- `BeforeCopRequest(&self, &mut kv::resourcegroup::CopRequest) -> Result<(), String>`：构造规范请求、调用 `Checker::BeforeCopRequest`，成功后回写三个字段。已有 `priority_low` 通过 `then_some(1)` 输入，回写使用 `|=`，因此不会把原本的低优先级恢复为高优先级。
- `CheckThresholds(&self, Option<&kv::resourcegroup::RUDetails>, u64, Option<&str>) -> Result<(), String>`：转换响应侧消耗与错误，饱和压缩 processed keys，并区分“原错误原样返回”与“checker 生成了新错误”。
- `CheckAction(&self) -> kv::resourcegroup::RunawayAction`：保留 `CoolDown` 和 `Kill`；规范层的 `SwitchGroup`、`DryRun`、`NoneAction` 等均映射为 KV `None`。
- `ResetTotalProcessedKeys(&self)`：直接调用规范 checker 的原子累计器重置。

本文件没有常量、enum、独立函数、宏或条件编译项，也没有公开 crate API。

## 执行流程

语句入口链为：[`scan_adapter_runtime.rs`](scan_adapter_runtime.rs) 的 `RunawayBeforeExecutor` 清理上一语句状态 → 检查全局资源控制开关和 Domain manager → `Manager::DeriveChecker` 按资源组、SQL/plan digest 与开始时间派生 checker → `Checker::BeforeExecutor` 处理 watch 规则及可能的资源组切换 → 将 checker 放入 `Arc` → 本文件包装后写入 `session.state.runaway_checker`。若资源控制关闭、无 manager、资源组无有效 checker，链路提前成功返回且不会安装本适配器。

请求发出前，存储驱动的 [`pkg/store/driver/runaway_adapter.rs`](../../store/driver/runaway_adapter.rs) 将 wire 请求转为 KV 请求并调用本实现的 `BeforeCopRequest`。本实现再转为规范请求；规范 checker 可能设置剩余 deadline、降低优先级、切换资源组或返回 Kill 错误。只有规范调用成功时，本实现才回写 KV 请求，因此错误路径不会提交局部字段修改。

响应或请求错误到达时，同一存储适配器调用 `CheckThresholds`。本实现把 `read_ru + write_ru` 所需数据、processed keys 与原始错误交给规范 checker；后者累计所有 Cop task 的 processed keys，并按耗时、RU、processed keys 的优先级判断是否超限。返回 `None` 表示无错误；返回与输入构造出的 `Storage` 错误相同表示仅保留既有错误，本适配层返回 `Ok(())`，让调用侧继续使用原有错误语境；只有返回不同错误（典型为 Kill 的 `QueryInterrupted`）时才返回 `Err(String)`。

扫描调度结束时，存储链通过 `ResetTotalProcessedKeys` 开始新的累计周期；调度并发度判断通过 `CheckAction` 识别 CoolDown。`SwitchGroup` 已在请求字段改写阶段生效，因而动作枚举投影为 `None` 并不取消已写入的资源组名。

## 数据与状态

本文件自身只有一个 `Arc<runaway::checker::Checker>` 字段，不复制 checker 状态。规范 checker 保存资源组设置快照、deadline、阈值、watch 动作、累计 processed keys 和“已标记”状态；其中累计值是 `AtomicI64`，规则标记是 `AtomicBool`。

请求转换遵循以下不变量：KV `priority_low = true` 对应规范 `override_priority = Some(1)`；其他 override 数值在本边界没有表示；资源组名与毫秒 deadline 完整克隆/回写。RU 的两个 `f64` 字段逐值复制。`processed_keys` 从 `u64` 转 `i64` 前以 `i64::MAX` 截断，避免大值环绕为负数并绕过阈值。

原错误只保留字符串，并包装为 `runaway::Error::Storage`；跨边界后不再保留原 KV/存储错误的具体类型。比较 `Some(&error) == original.as_ref()` 用于判断规范 checker 是否只是原样传回输入错误。该判定依赖 `runaway::Error` 的值相等语义。

## 依赖与调用关系

上游直接调用者是 [`scan_adapter_runtime.rs`](scan_adapter_runtime.rs) 的 `SessionBoundAdapterOwner::RunawayBeforeExecutor`，它构造 `SessionRunawayChecker` 并保存为会话级共享 trait object。独立测试 [`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 也直接构造本类型，验证 deadline、processed-keys 累计和 Kill 动作。

下游依赖分为三层：

- [`pkg/kv/lib.rs`](../../kv/lib.rs) 提供 `CopRequest`、`RUDetails`、三态 `RunawayAction` 和 `RunawayChecker: Send + Sync` 接口。
- [`pkg/resourcegroup/runaway/checker.rs`](../../resourcegroup/runaway/checker.rs) 提供实际的 `BeforeCopRequest`、`CheckThresholds`、`CheckAction` 与 `ResetTotalProcessedKeys` 算法。
- [`pkg/store/driver/runaway_adapter.rs`](../../store/driver/runaway_adapter.rs) 消费 KV trait object，并继续适配到 `astersql-store-copr` 的 wire 请求与批处理错误；它把本实现的任意错误降格为 Cop `QueryInterrupted`。

`pkg/session/Cargo.toml` 显式声明 `astersql-kv` 和 `astersql-resourcegroup-runaway` 两个路径依赖。RustCodeGraph 把目标文件识别为被 `scan_adapter_runtime.rs` 与 `scan_adapter_runtime_test.rs` 使用；对 impl 方法的动态 trait 调用边未完整解析，因此存储消费端以精确源码引用补证。

## 错误处理与边界

`BeforeCopRequest` 将所有规范错误转为字符串，调用者不能在此边界按 `runaway::Error` variant 分支。规范调用失败时不会执行后续字段回写；不过 checker 内部可能已经原子标记并记录 runaway，这与 Go 的“先标记、再按动作返回”顺序一致。

`CheckThresholds` 有意不把原始存储错误重新作为 `Err(String)` 返回：相同错误被视为“checker 没有替换结果”。真正的新错误才表示 runaway 检查要求中断。扩展该函数时不能简单地把所有 `Some(error)` 都映射成 `Err`，否则普通存储错误会被误报为 runaway interruption。

`u64` 到 `i64` 使用饱和边界；超过 `i64::MAX` 的多个不同输入会被规范 checker 视为同一个最大计数，但都足以命中任何可表示的正阈值。RU 使用浮点数原样传递，本文件不处理 NaN、负值或精度问题，阈值语义由规范 checker负责。

动作投影是刻意有损的：KV 层无 `SwitchGroup`/`DryRun` variant。资源组切换通过 `BeforeCopRequest` 的字段回写表达；DryRun 不应改变 Cop 调度。若规范层新增动作，当前通配分支会静默映射为 `None`，这是兼容性审查点。

## 并发与资源生命周期

`SessionRunawayChecker` 持有 `Arc`，其克隆可随同一查询的并发 Cop 请求跨线程共享；KV trait 的 `Send + Sync` 约束保证该用法在类型层成立。适配器不创建线程、任务、channel、锁或事务，也不持有请求借用超过单次方法调用。

并发正确性由规范 checker 提供：processed keys 通过 `AtomicI64::fetch_add` 累计，重置通过原子 `store`，首次规则命中通过 `AtomicBool::compare_exchange` 只记录一次。Go 的 `TestConcurrentResetAndCheckThresholds` 明确验证重置与阈值检查可并发；Rust 本文件只透传这些操作，没有额外串行化。

执行器侧 `Arc<Checker>` 与会话状态中的 `Arc<dyn kv::resourcegroup::RunawayChecker>` 共享同一底层对象。上一语句的引用会在下一次 `RunawayBeforeExecutor` 开头从两个槽位清除；已被存储请求持有的 `Arc` 则持续到请求链释放，避免悬垂引用。

## 与 Go 版本的对应关系

Go 没有同路径的 `pkg/session/runtime/typed_runaway_checker.go`；本文件是 Rust 分 crate 后新增的类型桥。主要语义对照是 [`pkg/resourcegroup/checker.go`](../../resourcegroup/checker.go) 的 `RunawayChecker` 接口、[`pkg/resourcegroup/runaway/checker.go`](../../resourcegroup/runaway/checker.go) 的 `Checker` 实现，以及 [`pkg/store/copr/coprocessor.go`](../../store/copr/coprocessor.go) 的调用位置。

Go 的 `Checker` 直接接收 `*tikvrpc.Request`、`*util.RUDetails` 和 `error`，所以无需本文件的 DTO/错误转换；Rust 为隔离 session、KV、resourcegroup 和 store-copr crates 增加两级适配。两边都在请求前处理 deadline/CoolDown/SwitchGroup/Kill，在响应后累计跨 task 的 processed keys，在 task-sender 周期结束时重置，并允许并发调用动作查询与阈值检查。

关键差异是 Rust KV trait 只暴露三态动作和字符串错误，且 processed keys 使用 `u64`；本文件分别用有损动作映射、字符串包装和饱和转换弥合差异。规范 Rust checker 的核心阈值优先级及原子状态与 Go 实现保持一致，但 Go 的 failpoint 注入和 nil receiver 安全不属于本适配器；Rust 通过非空 `Arc` 从类型上排除 nil checker。

## 扩展指南

- 新增或修改 Cop 请求字段时，应同时更新 `kv::resourcegroup::CopRequest`、本文件的双向转换和 [`pkg/store/driver/runaway_adapter.rs`](../../store/driver/runaway_adapter.rs) 的 wire 转换，并在独立测试覆盖成功回写与错误不回写。
- 扩充 runaway 动作时，必须审查规范 enum、KV enum、store-copr enum 三层映射；明确新动作由字段副作用表达还是需要新的调度枚举，避免继续落入 `_ => None` 后无声失效。
- 调整错误合同前，应先固定“原错误原样返回不代表 checker 新失败”的不变量，并覆盖无设置、普通存储错误、deadline 错误、Kill 替换错误及非 Kill 超限。
- 修改计数类型时，应保持超大 `u64` 不环绕、并发累计与 reset 的语义；性能上不要在每个 Cop 响应的热路径引入分配之外的阻塞 I/O 或锁。
- Rust 回归测试必须继续放在独立文件。最直接位置是 [`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 的 `canonical_runaway_checker_crosses_kv_interface_with_deadline_and_processed_keys`；wire 映射应同步 [`pkg/store/driver/coprocessor_adapter_test.rs`](../../store/driver/coprocessor_adapter_test.rs)，规范算法变化应同步 [`pkg/resourcegroup/runaway/checker_test.rs`](../../resourcegroup/runaway/checker_test.rs)。

## 验证依据

- RustCodeGraph `status`：本地索引包含 7,032 个 Rust 文件、307,296 个节点和 1,848,419 条边；`query SessionRunawayChecker` 唯一定位到 [`typed_runaway_checker.rs`](typed_runaway_checker.rs)，`node --file` 完整读取其 59 行，并报告被 [`scan_adapter_runtime.rs`](scan_adapter_runtime.rs) 与 [`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 使用。
- RustCodeGraph 源码核验：读取了 [`scan_adapter_runtime.rs`](scan_adapter_runtime.rs) 的 `RunawayBeforeExecutor`、[`pkg/kv/lib.rs`](../../kv/lib.rs) 的 KV trait/DTO、[`pkg/resourcegroup/runaway/checker.rs`](../../resourcegroup/runaway/checker.rs) 的规范 checker，以及 [`pkg/store/driver/runaway_adapter.rs`](../../store/driver/runaway_adapter.rs) 的存储侧桥。图对 trait impl 的方法 callers/callees 无完整结果，已用 `rg` 精确核对调用点。
- crate 与模块证据：[`pkg/session/Cargo.toml`](../Cargo.toml)、[`runtime.rs`](../runtime.rs)；`pkg/session` 没有 `doc.go`，因此不存在可补读的 package contract。
- 独立 Rust 测试：[`scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 的 `canonical_adapter_runaway_manager_switches_group_before_executor` 验证 checker 安装和资源组切换，`canonical_runaway_checker_crosses_kv_interface_with_deadline_and_processed_keys` 验证最大执行时长、跨调用累计阈值及 Kill 动作。相关规范 checker 与存储桥测试分别位于 [`pkg/resourcegroup/runaway/checker_test.rs`](../../resourcegroup/runaway/checker_test.rs) 和 [`pkg/store/driver/coprocessor_adapter_test.rs`](../../store/driver/coprocessor_adapter_test.rs)。
- Go 对照：[`pkg/resourcegroup/checker.go`](../../resourcegroup/checker.go)、[`pkg/resourcegroup/runaway/checker.go`](../../resourcegroup/runaway/checker.go)、[`pkg/resourcegroup/runaway/checker_test.go`](../../resourcegroup/runaway/checker_test.go)、[`pkg/store/copr/coprocessor.go`](../../store/copr/coprocessor.go)。Go 测试覆盖阈值动作与 reset/check 并发；Rust 桥测试覆盖本文件直接转换边界。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付检查使用任务指定的结构命令验证文件存在且恰含 11 个固定二级标题，并人工复核了符号、调用链、边界和扩展位置。

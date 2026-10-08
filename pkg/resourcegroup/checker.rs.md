# `pkg/resourcegroup/checker.rs`

## 文件定位

`checker.rs` 是 `astersql-resourcegroup` crate 的资源组边界定义文件。crate 入口 `pkg/resourcegroup/lib.rs` 通过 `pub mod checker` 和 `pub use checker::*` 将本文件的常量及两个 trait 暴露给依赖方；`pkg/resourcegroup/Cargo.toml` 又把该 crate 对应到 Go 包 `pkg/resourcegroup`。本文件只定义协议，不保存资源组配置，也不实现 runaway 判定算法；Go 侧的具体判定实现位于 `pkg/resourcegroup/runaway/checker.go`，Rust 侧更完整的迁移实现位于独立 crate `pkg/resourcegroup/runaway/`。

当前接线并不完全统一。`DEFAULT_RESOURCE_GROUP_NAME` 已被 session 运行时多个路径直接使用，`ConsumptionReporter` 被 `pkg/domain/ruv2_reporter.rs` 的桥接器使用；本文件的 `RunawayChecker` 则主要作为通用类型边界被 `pkg/session/sessmgr/processinfo.rs` 擦除，并由 `pkg/resourcegroup/migration_aster_unit_test.rs` 验证。实际 KV/coprocessor 请求链当前使用 `kv::resourcegroup::RunawayChecker`、`pkg/store/driver/runaway_adapter.rs` 和 `pkg/store/copr/coprocessor.rs` 中的相邻接口，不能把这些同名接口误认为本 trait 的直接实现。

## 核心职责

- `DEFAULT_RESOURCE_GROUP_NAME` 统一表达未指定资源组时的 `"default"` 回退值；例如 `pkg/session/runtime/scan_adapter_runtime.rs`、`relational_scan.rs`、`system_session.rs` 和 `control.rs` 直接引用它。
- `RunawayChecker` 描述一条查询从执行前检查、发送 coprocessor 请求、响应阈值检查、累计量重置，到读取处置动作和 kill 决策的完整能力集合。它把协议相关类型留给实现者通过关联类型选择。
- `ConsumptionReporter` 描述两种资源消耗上报形状：原始消费对象，以及旧版 engine-slot API 的 TiKV/TiDB/TiFlash 三项 RU v2 数值。
- 两个 trait 都要求 `Send + Sync`，从接口层规定实现可跨线程共享；本文件本身不规定锁、原子变量或调度策略。

## 主要符号

- `pub const DEFAULT_RESOURCE_GROUP_NAME: &str = "default"`：Go `DefaultResourceGroupName` 的 Rust 对应常量。
- `pub trait RunawayChecker: Send + Sync`：runaway 查询检查契约。
  - `Action: Copy + Send + Sync + 'static`：调用者可廉价复制并跨线程读取的处置动作。
  - `Error: Error + Send + Sync + 'static`：检查失败的实现自定义错误。
  - `Request`、`RuDetails`：由客户端集成选择的 coprocessor 请求和 RU 明细类型；本文件未额外约束它们。
  - `before_executor() -> Result<String, Error>`：执行前/编译后检查 watch list；字符串语义由实现提供，Go 具体实现可返回切换目标组名。
  - `before_cop_request(&mut Request) -> Result<(), Error>`：发送前检查，并允许原地修改请求。
  - `check_thresholds(&RuDetails, i64, Option<Error>) -> Option<Error>`：结合 RU 明细、已处理 key 数和已有错误产生最终可选错误。
  - `reset_total_processed_keys()`：清零实现内部的累计 processed-key 状态。
  - `check_action() -> Action`：读取当前处置动作；源码注释明确要求并发调用安全。
  - `check_rule_kill_action() -> (String, bool)`：返回触发原因/规则名和是否应 kill。
- `pub trait ConsumptionReporter: Send + Sync`：消费上报契约。
  - `Consumption`：实现选择的原始消费数据类型。
  - `report_consumption(&str, &Consumption)`：KV interceptor 不可用时直接上报。
  - `report_ruv2_consumption(&str, f64, f64, f64)`：分别上报 TiKV、TiDB、TiFlash RU v2。

## 执行流程

本文件没有可执行默认实现；下面是接口所表达、并由 Go 对照和迁移测试支持的生命周期，而不是本文件主动发起的调用：

1. 调用者为未显式指定组的请求选用 `DEFAULT_RESOURCE_GROUP_NAME`。
2. 查询完成编译、进入执行前，调用 `before_executor` 检查 watch list；实现可以返回目标组名，也可以返回错误阻止后续执行。
3. 每个 coprocessor 请求发送前，调用 `before_cop_request`。参数为 `&mut Request`，因此实现可写入优先级、资源组名或最大执行时间等协议字段；具体适配例可参照 `pkg/store/driver/runaway_adapter.rs` 的相邻 KV 接口桥接。
4. 收到 TiKV 结果后，调用 `check_thresholds`，传入本次 RU、processed keys 和已有错误。返回 `Some(error)` 表示保留或产生终止错误，`None` 表示没有错误。
5. 多个任务共享一个检查器时，`check_action` 可读取当前动作；查询阶段结束或需要重新计量时调用 `reset_total_processed_keys`。
6. 需要按资源组规则作 kill 决策时，调用 `check_rule_kill_action` 取得原因字符串和布尔结果。
7. 消费计量是独立旁路：常规拦截器不可用时调用 `report_consumption`；旧 engine-slot 路径调用 `report_ruv2_consumption`。`pkg/domain/ruv2_reporter.rs::ResourceGroupReporterBridge` 会把后者转发到具体 `ConsumptionReporter`。

## 数据与状态

本文件唯一拥有的数据是静态字符串常量；两个 trait 均无字段，因此所有动态状态都属于实现者。接口暗示的状态至少包括当前 runaway 动作、累计 processed-key 数、watch/rule 命中信息，以及上报器的发送或缓冲状态，但这些不是本文件的既成实现。

`pkg/resourcegroup/migration_aster_unit_test.rs::Checker` 用 `AtomicUsize` 演示 processed-key 累计与重置，用不可变 `Action` 演示动作读取；`Reporter` 用两个 `Mutex<Vec<...>>` 记录两类上报。它们是契约测试桩，不是生产存储模型。Go `pkg/resourcegroup/runaway/checker.go::Checker` 才包含 deadline、RU/processed-key 阈值、watch 动作与原子标志等真实状态。

## 依赖与调用关系

- crate 边界：`pkg/resourcegroup/Cargo.toml` 定义 `astersql-resourcegroup`，`lib.rs` 公开重导出本文件。Cargo 清单只有 `serde` 依赖，但本文件自身只使用 `std::error::Error` 路径，不依赖 `serde`。
- 上游常量用户：`pkg/session/runtime/scan_adapter_runtime.rs`、`relational_scan.rs`、`system_query.rs`、`control.rs` 和 `system_session.rs` 使用 `astersql_resourcegroup::DEFAULT_RESOURCE_GROUP_NAME`。
- 上游 trait 用户：`pkg/domain/ruv2_reporter.rs::ResourceGroupReporterBridge<R>` 约束 `R: astersql_resourcegroup::ConsumptionReporter` 并转发 RU v2；`pkg/session/sessmgr/lib.rs` 重导出该 crate，`processinfo.rs::ErasedRunawayChecker` 对 `resourcegroup::RunawayChecker` 做对象擦除边界。
- 测试实现：`pkg/resourcegroup/migration_aster_unit_test.rs::{Checker, Reporter}` 分别实现两个 trait，验证方法形状和共享约束。
- 相邻但非直接调用链：`pkg/store/driver/runaway_adapter.rs::KVRunawayChecker` 实现的是 `pkg/store/copr/coprocessor.rs::RunawayChecker`，其内部包装 `kv::resourcegroup::SharedRunawayChecker`。这些接口承载当前 coprocessor 生产路径，但没有直接实现本文件的泛型关联类型 trait。
- RustCodeGraph 对本文件识别出 12 个符号；对两个 trait 的 callers/callees 查询没有产出方法级调用边。因此上述接线关系以精确 `rg` 引用和相邻源码为补充证据。

## 错误处理与边界

`RunawayChecker::Error` 必须实现标准错误 trait，并可安全跨线程传递和共享。`before_executor` 与 `before_cop_request` 使用 `Result`，允许实现立即中止当前阶段；`check_thresholds` 接受一个拥有所有权的 `Option<Error>` 并返回 `Option<Error>`，允许保留已有错误、替换为阈值错误或清除错误。此所有权语义与 Go 的可空 `error` 相近，但并非完全同型。

接口未提供默认方法，所以实现者必须覆盖全部六个检查方法。它也没有规定空资源组名、负 `process_keys`、NaN/负 RU v2 数值、上报失败或 panic 的统一处理；调用者不能假定接口层会校验这些输入。尤其 `report_consumption` 和 `report_ruv2_consumption` 没有返回值，失败处理只能由实现内部完成。测试桩用 `process_keys.max(0)` 是测试实现的选择，不是 trait 保证。

## 并发与资源生命周期

`Send + Sync` 是两个 trait 的全局并发不变量；`Action` 和 `Error` 还分别携带相应的跨线程约束。源码只对 `check_action` 明确写出可并发调用，但由于 `&self` 方法可由共享实现同时调用，任何可变累计值、watch 命中状态和上报缓冲都必须使用原子量、锁或等价同步机制。

trait 不拥有线程、异步任务、通道、文件或网络连接，也没有 `Drop` 生命周期协议。请求的可变借用只持续到 `before_cop_request` 返回；RU 和消费对象均以共享借用传入。实现若异步保留数据，必须自行复制并管理其生命周期。`'static` 只约束 `Action` 和 `Error`，`Request`、`RuDetails` 与 `Consumption` 没有该约束。

## 与 Go 版本的对应关系

`pkg/resourcegroup/checker.go` 是一一对应的接口来源：常量值相同，两个接口的方法集合也相同。Rust 用 snake_case、关联类型和 `Result`/`Option` 表达 Go 的方法、具体协议类型和可空 `error`：

- Go 固定使用 `rmpb.RunawayAction`、`tikvrpc.Request`、`util.RUDetails`、`rmpb.Consumption`；Rust 通过 `Action`、`Request`、`RuDetails`、`Consumption` 保持集成可替换性。
- Go `CheckThresholds(..., err error) error` 对应 Rust `check_thresholds(..., err: Option<Error>) -> Option<Error>`。
- Go `BeforeExecutor() (string, error)` 对应 Rust `Result<String, Error>`；其余方法保持相同顺序和意图。
- Go 具体 `pkg/resourcegroup/runaway/checker.go::Checker` 实现 nil receiver 防护、watch list、deadline/RU/processed-key 阈值、动作选择和原子计数；这些逻辑不在本文件中。不能因 trait 形状已迁移，就宣称该文件已经复刻具体算法。
- `pkg/resourcegroup/migration_aster_unit_test.rs` 验证接口形状、默认组名、请求可变写入、错误传播、累计重置、动作读取和两类上报，但它没有覆盖 Go 具体 `Checker` 的全部业务分支；具体迁移行为应看 `pkg/resourcegroup/runaway/checker.rs` 及其独立 `checker_test.rs`。

## 扩展指南

- 新增跨语言接口方法时，应同时更新 `checker.rs`、`checker.go`、所有生产实现/桥接器和 `migration_aster_unit_test.rs`；Rust 测试继续放在独立测试文件，不要内嵌进源文件。
- 新增请求或 RU 协议约束时，优先评估是否应继续使用关联类型；给关联类型增加 `Send`、`Sync` 或 `'static` 约束会影响所有实现者，是兼容性变更。
- 修改阈值错误语义时，要同时检查当前实际请求链的 `kv::resourcegroup::RunawayChecker`、`pkg/store/driver/runaway_adapter.rs` 和 `pkg/store/copr/coprocessor.rs`，避免通用接口与生产适配器继续分叉。
- 为 `ConsumptionReporter` 增加可失败上报会改变现有无返回值契约，需要设计重试、丢弃和背压策略，并同步 `pkg/domain/ruv2_reporter.rs::ResourceGroupReporterBridge`。
- 新增资源组回退规则时，应复查所有 `DEFAULT_RESOURCE_GROUP_NAME` 使用点以及 DDL 自有的同名常量，关注大小写、空名和跨模块一致性。
- 性能风险主要在实现层：高频阈值检查与消费上报不应引入全局锁争用；本 trait 的 `&self` 设计允许共享，但不自动保证低开销。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter pkg/resourcegroup` 定位本模块；`node --file pkg/resourcegroup/checker.rs` 读取完整 88 行并报告使用关联；`query`/`node` 核对 `RunawayChecker`、`ConsumptionReporter`、`before_cop_request`、`report_ruv2_consumption` 和默认组常量；对两个 trait 执行 callers/callees 查询未得到方法级边。
- Rust 源与 crate：`pkg/resourcegroup/checker.rs`、`pkg/resourcegroup/lib.rs`、`pkg/resourcegroup/Cargo.toml`。
- Rust 调用与桥接：`pkg/domain/ruv2_reporter.rs`、`pkg/session/sessmgr/lib.rs`、`pkg/session/sessmgr/processinfo.rs`、`pkg/store/driver/runaway_adapter.rs`、`pkg/store/driver/kv_adapter.rs`、`pkg/store/copr/coprocessor.rs`。
- Rust 独立测试：`pkg/resourcegroup/migration_aster_unit_test.rs`；相关生产链测试可参考 `pkg/store/driver/coprocessor_adapter_test.rs`，但后者验证的是 KV/coprocessor 相邻接口，而非直接实现本 trait。
- Go 对照：`pkg/resourcegroup/checker.go`；具体行为与测试位于 `pkg/resourcegroup/runaway/checker.go`、`pkg/resourcegroup/runaway/checker_test.go`，coprocessor 调用顺序见 `pkg/store/copr/coprocessor.go`。
- 结构检查使用任务指定命令，要求目标存在且恰有 11 个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。

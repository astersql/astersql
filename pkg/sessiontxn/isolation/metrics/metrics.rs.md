# `pkg/sessiontxn/isolation/metrics/metrics.rs`

## 文件定位

本文件是 `astersql-sessiontxn-isolation-metrics` crate 中的具体绑定实现，由同目录 `lib.rs` 通过 `#[path = "../../../../pkg/sessiontxn/isolation/metrics/metrics.rs"]` 挂载为公开模块 `isolation_metrics`。它不定义 Prometheus collector 的描述符，而是从 `lib.rs::metrics::RCCheckTSWriteConfilictCounter` 这个带 `type` 标签的父 `CounterVec` 中取出读、写两个预绑定 `Counter` 句柄。

crate 边界由同目录 `Cargo.toml` 确定：库入口是 `lib.rs`，唯一直接外部依赖是 `prometheus = "0.14"`，`package.metadata.porting.go-package` 指向 Go 包 `pkg/sessiontxn/isolation/metrics`。上层 `pkg/sessiontxn/isolation/Cargo.toml` 虽已声明该 crate 依赖，但当前 Rust 隔离级别实现尚未引用本文件的两个计数句柄；生产初始化接线位于 `pkg/util/metricsutil/common.rs::initMetrics`。

## 核心职责

- 保存 RC（Read Committed）读时间戳检查路径的写冲突子计数器：`RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER`。
- 保存 RC 写时间戳检查路径的写冲突子计数器：`RC_WRITE_CHECK_TS_WRITE_CONFLICT_COUNTER`。
- 在 `init_metrics_vars` 中使用 `read_check` 和 `write_check` 两个固定标签值从同一父 `CounterVec` 取得子序列，使两条路径共享同一指标名但保持序列独立。
- 对外提供 `init` 转发入口；Rust 不会像 Go 的包初始化那样自动执行它，当前实际生产接线是 `pkg/util/metricsutil/common.rs::initMetrics` 直接调用 `init_metrics_vars`。

## 主要符号

- `pub static mut RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER: Option<prometheus::Counter>`：读检查冲突序列句柄。初始为 `None`，成功初始化后指向 `type="read_check"` 的子计数器。
- `pub static mut RC_WRITE_CHECK_TS_WRITE_CONFLICT_COUNTER: Option<prometheus::Counter>`：写检查冲突序列句柄。初始为 `None`，成功初始化后指向 `type="write_check"` 的子计数器。
- `pub fn init()`：无参数、无返回值的包级兼容入口，唯一行为调用 `init_metrics_vars()`。代码搜索未发现当前 Rust 调用者。
- `pub fn init_metrics_vars()`：核心初始化函数。它读取父 `CounterVec`，先后以 `metrics::LblRCReadCheckTS` 和 `metrics::LblRCWriteCheckTS` 调用 `with_label_values`，再将结果写入两个全局 `Option`。

文件没有类型、trait、`impl`、泛型、异步函数或条件编译项。

## 执行流程

1. `pkg/util/metricsutil/common.rs::initMetrics` 先调用 `initParentMetricsCollectors`。
2. `initParentMetricsCollectors` 在隔离 metrics crate 本地的 `RCCheckTSWriteConfilictCounter` 为 `None` 时创建 `rc_check_ts_conflict_total{type}`；指标 help 为 `Counter of WriteConflict caused by RCCheckTS.`。
3. 父 collector 存在后，`initMetrics` 按子系统顺序调用 `isolation_metrics::init_metrics_vars()`。
4. `init_metrics_vars` 在 `unsafe` 区域内取父 `CounterVec` 的共享引用；若仍为 `None`，立即以固定消息 panic。
5. 函数将 `read_check` 和 `write_check` 两个标签子序列分别写入读、写全局句柄。`prometheus::Counter` 是可克隆句柄，子句柄的递增会落到父向量中对应的 time series。
6. Go 运行时在 `pkg/sessiontxn/isolation/readcommitted.go` 中使用这两个语义对应的句柄：`handleAfterQueryError` 只在错误为 `ErrWriteConflict` 且 `StmtCtx.RCCheckTS` 为真时增加读计数；`handleAfterPessimisticLockError` 只在写冲突分支且 `checkTSInWriteStmt` 为真时增加写计数。当前 `pkg/sessiontxn/isolation/readcommitted.rs` 没有对 Rust 句柄的对应递增接线。

## 数据与状态

本文件的可变状态仅是两个进程级 `static mut Option<Counter>`。`None` 表示未绑定，`Some(counter)` 表示已持有对应标签序列的句柄。父状态位于 `lib.rs::metrics::RCCheckTSWriteConfilictCounter: Option<CounterVec>`，标签名是 `type`，子序列值由 `LblRCReadCheckTS = "read_check"` 与 `LblRCWriteCheckTS = "write_check"` 确定。

`init_metrics_vars` 可重复执行：每次都重新从当前父 `CounterVec` 生成句柄并覆盖两个 `Option`。若父向量未替换，重新绑定仍指向同一标签序列；若父向量已替换，后续递增会进入新父向量。`migration_aster_unit_test.rs::init_metrics_vars_rebinds_handles_after_the_source_changes` 直接验证了后一性质。

## 依赖与调用关系

上游调用与装配证据：

- `pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 保证父 `CounterVec` 存在；`initMetrics` 随后直接调用 `isolation_metrics::init_metrics_vars`。
- `pkg/sessiontxn/isolation/metrics/migration_aster_unit_test.rs` 有两个直接调用者，分别验证标签隔离和父 collector 替换后的重绑定。
- `lib.rs` 将源文件公开为 `isolation_metrics`；`pkg/util/metricsutil/Cargo.toml` 依赖该 crate，而 workspace 根 `Cargo.toml` 以 `facade_sessiontxn_isolation_metrics` 名称收录它。

下游依赖只有同 crate 的 `metrics` 模块和 `prometheus::CounterVec::with_label_values`。该文件不执行指标注册、不接触事务上下文，也不判断什么是写冲突；这些分别属于 metrics 装配层和 RC 事务状态机。

RustCodeGraph 将目标文件识别为 3 个符号，并报告它被 `migration_aster_unit_test.rs` 使用；由于 `init_metrics_vars` 在仓库中有多个同名符号，通用 `callers/callees` 查询存在歧义，上述生产调用边另以文件限定源码和 `rg` 直接引用核验。

## 错误处理与边界

- 唯一显式失败边界是父 `RCCheckTSWriteConfilictCounter` 为 `None`：`expect` 会 panic，消息为 `metrics::InitMetrics must run before isolation metrics initialization`。函数不返回 `Result`，因此调用者无法恢复该顺序错误。
- `with_label_values` 返回 `Counter` 而非 `Result`；父向量在 `lib.rs` 中固定为单一 `type` 标签，本文件总是传入一个标签值。若未来修改父 collector 标签数量，必须同步修改这里，否则 Prometheus API 可因标签维度不匹配而 panic。
- 本文件不防止调用方在初始化前解包子句柄，也不为计数句柄提供安全访问函数。直接消费者必须遵守“先初始化、后读取”不变量。
- Rust 生产 RC 状态机未递增这两个计数器，因此本文件当前保证的是指标句柄初始化语义，不是 Rust RC 冲突路径的端到端可观测性。

## 并发与资源生命周期

`prometheus::Counter` 句柄的实际计数状态可被克隆并由 Prometheus 库管理；本文件没有手工释放、线程、异步任务、锁或通道。但两个句柄槽位是 `static mut`，读写槽位本身不具备 Rust 同步保障；`init_metrics_vars` 也没有 `Once`或互斥锁。

生产设计假定初始化在进程启动/指标注册阶段串行完成，然后才有并发请求读取句柄。若在有消费者的同时重新赋值全局 `Option`，将违反 `static mut` 的独占访问要求，本文件不承诺这种用法安全。独立测试用 `TEST_LOCK: Mutex<()>` 串行化全局状态替换，这是测试隔离手段，不是生产代码中的同步机制。

## 与 Go 版本的对应关系

`pkg/sessiontxn/isolation/metrics/metrics.go` 是直接对照：

- Go `RcReadCheckTSWriteConfilictCounter prometheus.Counter` 对应 Rust `RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER: Option<prometheus::Counter>`。
- Go `RcWriteCheckTSWriteConfilictCounter prometheus.Counter` 对应 Rust `RC_WRITE_CHECK_TS_WRITE_CONFLICT_COUNTER: Option<prometheus::Counter>`。
- Go `init()` 自动调用 `InitMetricsVars()`；Rust `init()` 只是普通公开函数，实际装配层选择直接调用 `init_metrics_vars()`。
- 两版都从同一语义的 `RCCheckTSWriteConfilictCounter` 用 `LblRCReadCheckTS` / `LblRCWriteCheckTS` 绑定子序列，并保留上游符号中 `Confilict` 的历史拼写。
- Go 父指标定义在 `pkg/metrics/server.go`，指标全名为 `tidb_server_rc_check_ts_conflict_total`，标签键为 `type`；标签值在 `pkg/metrics/session.go` 中定义。Rust 迁移期父 collector 由 `pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 在本地 crate 边界内创建。
- Go 句柄已由 `pkg/sessiontxn/isolation/readcommitted.go` 的两条冲突处理分支消费；Rust `readcommitted.rs` 保留 RCCheckTS 状态逻辑，但代码搜索未发现它消费本文件的句柄。因此指标定义和绑定语义已对齐，Rust 端运行时递增仍是明确的迁移缺口。

## 扩展指南

- 若新增 RC 冲突类别，先在父 `CounterVec` 的标签契约中确定新值，再在本文件增加子句柄及其绑定；同步更新 Go 对照（如属于 Go 兼容契约）和 `migration_aster_unit_test.rs`。增加高基数动态标签会放大 Prometheus 序列数，不应通过本文件的固定句柄模式随意引入。
- 若改动父 collector 的名称、help、标签键或标签数量，必须同步核对 `pkg/util/metricsutil/common.rs::initParentMetricsCollectors`、`pkg/metrics/server.go`、`pkg/metrics/session.go`和本 crate 测试中的 `fresh_source_counter`，否则可出现面板兼容性破坏或标签数不匹配 panic。
- 若要完成 Rust 端可观测接线，修改点应在 `pkg/sessiontxn/isolation/readcommitted.rs` 中与 Go `handleAfterQueryError` / `handleAfterPessimisticLockError` 相同的精确分支，不能在所有重试或所有锁错误上泛化递增。测试应放在独立 `*_test.rs` 文件，并同时验证正向递增与非写冲突/未启用 RCCheckTS 时不递增。
- 若要消除 `static mut` 风险，需同时调整 `lib.rs` 中父 collector、装配层和所有消费者，优先采用 `OnceLock`/`LazyLock` 或安全 getter；不应只在本文件局部包装而留下另一个无同步全局源。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/sessiontxn/isolation/metrics` 列出 `lib.rs`、`metrics.rs`、`metrics.go` 和 `migration_aster_unit_test.rs`；`node --file pkg/sessiontxn/isolation/metrics/metrics.rs --offset 1 --limit 240` 读取了完整 46 行源文件并报告 3 个符号。`query init_metrics_vars --kind function --json` 确认目标函数在第 35 行；同名图查询有歧义，因此调用边又用文件限定搜索复核。
- Rust 源码与装配：`pkg/sessiontxn/isolation/metrics/metrics.rs`、同目录 `lib.rs`、`Cargo.toml`，以及 `pkg/util/metricsutil/common.rs` 的 `initParentMetricsCollectors` / `initMetrics`。
- Go 对照：`pkg/sessiontxn/isolation/metrics/metrics.go`、`pkg/sessiontxn/isolation/readcommitted.go`、`pkg/metrics/server.go`和 `pkg/metrics/session.go`。
- 独立 Rust 测试：`pkg/sessiontxn/isolation/metrics/migration_aster_unit_test.rs::init_metrics_vars_binds_the_two_go_label_values` 验证读/写标签分离及计数；`init_metrics_vars_rebinds_handles_after_the_source_changes` 验证替换父向量后句柄重绑定。同目录没有 Go 专用 metrics 测试，RC 行为测试位于 `pkg/sessiontxn/isolation/readcommitted_test.go`。
- 目标包与上级 `pkg/sessiontxn` 下未找到 `doc.go`，因此无额外包契约可读。本任务按要求未运行 Cargo；验收以源码、调用边、对照实现、独立测试和文档结构检查为证据。

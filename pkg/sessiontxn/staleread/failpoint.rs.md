# `pkg/sessiontxn/staleread/failpoint.rs`

## 文件定位

本文件是 `astersql-sessiontxn-staleread` crate 中的测试断言辅助，源文件为 [`failpoint.rs`](failpoint.rs)。crate 根 [`lib.rs`](lib.rs) 通过 `pub mod failpoint` 声明模块，并用 `pub use failpoint::*` 将唯一公开函数 `assert_stmt_staleness` 重导出到 crate 根。它位于 stale read（按历史 TSO 读取快照）判定与事务 Provider 之间，用于检查“语句被标记为过期读”与“当前 Provider 已是过期读 Provider”两层状态是否一致。

[`Cargo.toml`](Cargo.toml) 将该 crate 命名为 `astersql-sessiontxn-staleread`，并记录其 Go 对照包为 `pkg/sessiontxn/staleread`。当前函数只使用本 crate 的 `SessionRef` 和 `is_stmt_staleness`，不直接依赖 Cargo 中置于 `target.'cfg(any())'` 下的外部会话、KV 或 planner crate。文件名虽然是 `failpoint.rs`，但 Rust 实现本身不注册、启用或调度 failpoint；它只是可由测试钩子调用的断言函数。

## 核心职责

- 比较调用者给出的 `expected` 与 `is_stmt_staleness(session)` 返回的语句级标记。
- 当 `expected == true` 时，进一步检查 `Session::provider_is_staleness`，防止语句上下文声称是 stale read、事务上下文却仍由普通 Provider 承担。
- 以 panic 形式暴露测试不变量破坏，并提供与 Go `AssertStmtStaleness` 第一条断言兼容的诊断文字。

它不计算读 TSO、不创建快照、不安装 `StalenessTxnContextProvider`，也不改变任何会话字段。真实状态分别由 [`processor.rs`](processor.rs) 的 `BaseProcessor::set_evaluated_values` 和 [`provider.rs`](provider.rs) 的 Provider 初始化路径写入。

## 主要符号

- `pub fn assert_stmt_staleness(session: &SessionRef, expected: bool)`：文件中唯一的函数和公开 API。
  - `session` 是 `Arc<Mutex<Session>>` 的别名引用；函数不取得所有权，也不延长会话生命周期。
  - `expected` 表示调用点认为当前语句应否为 stale read。
  - 返回类型为 `()`；成功时静默返回，失败时由 `assert_eq!` 或 `assert!` panic。
- `is_stmt_staleness`：从 [`util.rs`](util.rs) 导入的下游函数，通过会话锁读取 `Session::statement_is_staleness`；锁中毒时返回 `false`。
- `SessionRef`：从 [`util.rs`](util.rs) 重导出的 `Arc<Mutex<Session>>` 类型别名。

文件没有常量、类型、trait、`impl`、条件编译项或私有函数。`assert_stmt_staleness` 也没有 `#[cfg(test)]`；它随当前 crate 模块正常编译，只是用途和 Go 对照均明确为测试辅助。

## 执行流程

1. 调用 `is_stmt_staleness(session)` 读取语句级 `statement_is_staleness`；结果保存为 `actual`。
2. `assert_eq!(actual, expected, ...)` 校验调用者预期。普通读与过期读都会经过这一步，失败信息包含 expected 与 actual。
3. 若 `expected == false`，函数立即结束，不检查 Provider 标记。
4. 若 `expected == true`，函数再次对 `SessionRef` 加锁，读取 `provider_is_staleness`。锁中毒通过 `unwrap_or(false)` 降级为 `false`。
5. `assert!(provider_is_staleness, ...)` 保证过期读语句已安装/激活过期读 Provider；否则 panic。

因此成功路径表达的是单向不变量：预期为 stale read 时，两项标记都必须为真；预期为普通读时，仅要求语句标记为假，并不要求 Provider 标记也为假。

## 数据与状态

本文件不拥有持久状态，只读取 `Session` 的两个布尔字段：

- `statement_is_staleness`：由 [`processor.rs`](processor.rs) 的 `BaseProcessor::set_evaluated_values` 设置为 `ts != 0`，并由 `Session::begin_statement` 在新语句开始时重置为 `false`。
- `provider_is_staleness`：初值为 `false`；[`provider.rs`](provider.rs) 的 `activate_stale_txn` 与 `enter_new_stale_txn_with_replace_provider` 在成功建立或替换 stale-read 上下文后设为 `true`。

两次读取分别加锁，中间没有持有同一个锁守卫，因此该辅助函数得到的不是两个字段的原子快照。当前测试/会话模型依靠调用点在状态安装完成后执行断言；若未来允许其他线程同时改写同一 `SessionRef`，必须重新评估该观察方式。

## 依赖与调用关系

- 模块接线：[`lib.rs`](lib.rs) 公开声明并重导出 `failpoint` 模块。
- 直接下游：`assert_stmt_staleness` 调用 [`util.rs`](util.rs) 的 `is_stmt_staleness`，并直接读取同文件定义的 `SessionRef`/`Session::provider_is_staleness`。
- 状态生产者：[`processor.rs`](processor.rs) 写入语句级标记；[`provider.rs`](provider.rs) 的两条 Provider 安装路径写入 Provider 标记。
- Rust 上游现状：仓库限定检索只找到函数定义和测试说明文字，没有找到调用 `assert_stmt_staleness` 的 Rust 代码。因此它当前是已公开、但未接入 Rust executor 主链的测试辅助，不能宣称 Rust 应用运行时会执行该断言。
- Go 上游对照：`pkg/executor/compiler.go` 的 `assertTxnManagerInCompile`、`assertStmtCtxIsStaleness`，`pkg/executor/adapter.go` 的 `assertTxnManagerInShortPointGetPlan`、`assertTxnManagerInRebuildPlan`，以及 `pkg/executor/builder.go` 的 `assertExecutePrepareStatementStalenessOption` 会调用 Go `staleread.AssertStmtStaleness`。这些调用都位于 failpoint 注入闭包内，不影响未启用 failpoint 的正常执行。

RustCodeGraph 将目标文件识别为 2 个节点，并准确返回 `assert_stmt_staleness` 的函数节点；精确 callers 查询未返回可用结果，故上述调用关系用限定路径的源码检索核验，没有采用索引输出中与本函数无关的模糊 “used by” 结果。

## 错误处理与边界

- 本函数没有 `Result` 返回和可恢复错误路径；任一不变量失败都会 panic，符合测试断言定位，但不适合作为生产输入校验 API。
- 第一条 panic 信息与 Go 的 `stmtctx isStaleness wrong` 语义一致；第二条 Rust 信息只说明应使用 `StalenessTxnContextProvider`，不像 Go 版本那样打印当前 Provider 的动态类型，因为 Rust 会话模型只保存布尔标记。
- `is_stmt_staleness` 在锁中毒时返回 `false`。当 `expected == true` 时第一条断言会失败；当 `expected == false` 时却可能通过并提前返回，因此“普通读断言成功”不能证明锁一定健康。
- Provider 读取也把锁中毒映射为 `false`，最终表现为断言失败而非显式的 lock-poison 错误。
- `expected == false` 不检查 `provider_is_staleness`；这是与 Go 一致的分支结构，不应擅自加强为双向等价，否则可能改变测试钩子的兼容行为。

## 并发与资源生命周期

`SessionRef` 使用 `Arc<Mutex<Session>>`，函数的两次读取各自只在表达式期间持有锁守卫，并在下一步断言前释放；本文件不创建线程、任务、通道、事务、快照或 I/O 资源，也不克隆 `Arc`。

锁中毒不会从函数签名传播，而会被折叠成 `false` 后按断言语义处理。两个字段不是在同一临界区读取，故不存在跨字段原子一致性保证。函数自身只借用 `SessionRef`，调用结束后不会保留会话引用或延长后端、事务与快照资源的生命周期。

## 与 Go 版本的对应关系

Go 同路径 [`failpoint.go`](failpoint.go) 的 `AssertStmtStaleness(sctx, expected)` 是直接语义来源：先比较 `IsStmtStaleness(sctx)`，再仅在 expected 为真时取 `sessiontxn.GetTxnManager(sctx).GetContextProvider()` 并断言其动态类型为 `*StalenessTxnContextProvider`。

Rust 保留了同样的分支次序和单向不变量，但做了适配：

- Go 使用完整 `sessionctx.Context` 与事务管理器接口；Rust 使用本 crate 的轻量 `SessionRef`。
- Go 通过 Provider 动态类型断言验证真实对象；Rust 读取 `provider_is_staleness` 布尔标记。该标记由真实 Rust Provider 安装路径写入，但类型保证弱于 Go 的运行时类型检查。
- Go 的断言目前已接入 executor failpoint；Rust 函数当前没有调用者。Rust 测试通过 `processor_test.rs`/`externalts_test.rs` 直接检查 `is_stmt_staleness`，通过 `provider_test.rs::provider_is_marked_when_it_is_installed` 检查 Provider 标记，尚未把两者组合成该函数的直接回归测试。
- Rust 的 `Mutex` 中毒降级行为是 Go 版本不存在的边界。

## 扩展指南

- 若要把该辅助接入 Rust executor 或新的 failpoint 框架，应在对应独立测试文件中调用公开函数，不要把测试写进 `failpoint.rs`；同时保留“只有 expected 为真才检查 Provider”的 Go 分支语义。
- 若新增 Provider 类型或改变 Provider 表示，优先让 `provider_is_staleness` 由安装/卸载生命周期统一维护；仅修改断言会掩盖状态生产端错误。若要达到 Go 的动态类型强度，需要先扩展会话/事务管理器抽象，而不是在本文件猜测类型。
- 应补充独立 Rust 回归测试，至少覆盖：两标记皆假且 expected 为假时通过；语句标记与 expected 不符时 panic；语句标记为真但 Provider 标记为假时 panic；两标记皆真时通过。测试可放在新的 `failpoint_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] mod failpoint_test;` 接入，保持源文件与测试分离。
- 若改为返回 `Result` 或显式报告锁中毒，会改变现有 panic 型测试 API；需同步 Go 兼容性说明及所有未来调用点。
- 本函数只做两次互斥锁读取，正常测试成本很低；不要在生产热路径无条件调用。若并发模型允许状态在两次读取间变化，应改为一次锁内读取两个字段并增加并发回归测试。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 7,032 个 Rust 文件；`files --filter pkg/sessiontxn/staleread` 显示目标文件含 2 个节点；`node --file pkg/sessiontxn/staleread/failpoint.rs` 核对了完整 42 行源码；`query assert_stmt_staleness --kind function --json` 定位到第 25 行公开函数。精确 callers 查询持续无输出后中止，调用者结论改由源码检索补证。
- crate 边界：[`Cargo.toml`](Cargo.toml) 证明 crate 名称、Go 包映射和 `cfg(any())` 条件依赖；[`lib.rs`](lib.rs) 证明模块声明、公开重导出及独立测试模块布局。
- Rust 状态链：[`util.rs`](util.rs) 的 `Session`、`SessionRef`、`begin_statement`、`is_stmt_staleness`；[`processor.rs`](processor.rs) 的 `BaseProcessor::set_evaluated_values`；[`provider.rs`](provider.rs) 的 `activate_stale_txn`、`enter_new_stale_txn_with_replace_provider`。
- 独立 Rust 测试：[`processor_test.rs`](processor_test.rs) 与 [`externalts_test.rs`](externalts_test.rs) 验证语句级 stale 标记；[`provider_test.rs`](provider_test.rs) 的 `provider_is_marked_when_it_is_installed` 验证 Provider 标记。仓库检索确认没有 Rust 测试直接调用本函数。
- Go 对照与调用点：[`failpoint.go`](failpoint.go)；`pkg/executor/compiler.go`、`adapter.go`、`builder.go` 中的五类 failpoint 调用；`tests/realtikvtest/txntest/stale_read_test.go` 启用 prepared-statement stale-read 断言。该 RealTiKV 测试中的 `assertStmtCtxIsStaleness` 路径字符串拼写为 `exector`，本文不据此声称该特定钩子已成功触发。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证使用任务指定的固定 11 标题结构命令，并人工复核本文没有把未接线的 Rust helper 描述成已运行的应用主链。

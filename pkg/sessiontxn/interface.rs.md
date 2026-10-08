# `pkg/sessiontxn/interface.rs`

## 文件定位

`interface.rs` 是 `astersql-sessiontxn` crate 的公共事务管理契约。crate 根 `pkg/sessiontxn/lib.rs` 以 `mod interface` 装入本文件，并通过 `pub use interface::*` 把这里的类型、trait 和辅助函数重新导出。`pkg/sessiontxn/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/sessiontxn`，本文件的直接语义基线是同路径的 `pkg/sessiontxn/interface.go`。

它位于会话层与具体事务策略之间：调用方通过 `TxnManager` 访问事务级 InfoSchema、作用域、读时间戳、快照和生命周期钩子；管理器再通过 `TxnContextProvider` 隔离乐观、悲观、不同隔离级别及陈旧读的差异。文件本身不实现事务、TSO、MVCC 或锁算法，只定义边界并提供四个很薄的编排入口：`GetTxnManager`、`AdviseOptimizeWithPlanAndThenWarmUp`、`NewTxn`、`NewTxnInStmt`。

当前 Rust 接线必须与设计意图区分开看：仓库搜索能确认 `TxnManager`、`TxnAdvisable` 和 `TxnManagerContext` 的实现位于 `pkg/sessiontxn/txn_manager_test.rs` 测试夹具；没有找到连接真实会话的生产实现。`pkg/sessiontxn/isolation/lib.rs` 虽有同名 `TxnContextProvider` 及具体 provider 分发，但它是 isolation 子 crate 自己的、签名不同的 trait，并未实现本文件的 `TxnContextProvider`。因此本文件当前是已测试的公共契约和迁移边界，而不是已经贯通生产会话主链的证据。

## 核心职责

1. 用 `Error`、`RequestContext`、`InfoSchemaRef`、`Snapshot`、`Transaction` 和 `Statement` 把下游 crate 的复杂类型收束为稳定的 sessiontxn API。
2. 用 `EnterNewTxnType` 与 `EnterNewTxnRequest` 完整表达进入事务的场景、可替换 provider、事务模式、因果一致性选项和陈旧读时间戳。
3. 用 `StmtErrorHandlePoint`、`StmtErrorAction`、`StmtErrorAdvice` 及三个构造函数描述错误发生位置与下一步动作，而不在接口层决定具体重试策略。
4. 用 `TxnAdvisable`、`TxnContextProvider` 和 `TxnManager` 分层定义优化建议、具体事务上下文能力和会话级管理能力。
5. 用 `TxnManagerContext` 代替 Go 版本可注入的包级函数变量，使管理器的所有权显式落到会话对象上。
6. 保留 Go 辅助流程的先后顺序：先按计划优化再预热；`NewTxn` 固定创建默认乐观事务；`NewTxnInStmt` 在进入事务成功后重新触发当前语句的 `OnStmtStart`。

## 主要符号

- 类型别名：`Error = sessionctx::GoError`；`RequestContext = sessionctx::ExecutionContext`；`InfoSchemaRef = Arc<dyn infoschema::InfoSchema>`；`Snapshot = Box<dyn kv::Snapshot>`；`Transaction = Box<dyn kv::Transaction>`；`Statement = Rc<dyn ast::ast::StmtNode>`。这些别名是接口 ABI 的组成部分，尤其决定了共享所有权、动态分发和错误传播形状。
- `EnterNewTxnType`：四种进入方式。`EnterNewTxnDefault` 是默认值；`EnterNewTxnWithBeginStmt` 对应显式 `BEGIN`/`START TRANSACTION`；`EnterNewTxnBeforeStmt` 对应语句前按需进入；`EnterNewTxnWithReplaceProvider` 表示替换当前 provider，Go 注释指出目前用于 stale read。
- `EnterNewTxnRequest`：字段 `Type`、`Provider`、`TxnMode`、`CausalConsistencyOnly`、`StaleReadTS`。其 `Default` 实现产生“默认类型、无 provider、空模式、非仅因果一致、非陈旧读”的请求；空 `TxnMode` 的 Go 语义是由系统变量决定。
- `StmtErrorHandlePoint`：区分查询失败后的处理点 `StmtErrAfterQuery` 与悲观锁失败后的处理点 `StmtErrAfterPessimisticLock`。
- `StmtErrorAction` 与 `StmtErrorAdvice`：动作分别为直接报错、已准备重试和无明确意见；建议值是 `(动作, Option<Error>)`。`ErrorAction` 保留指定错误，`RetryReady` 和 `NoIdea` 不附带错误。
- `TxnAdvisable`：`AdviseWarmup` 与 `AdviseOptimizeWithPlan` 两个可失败的可变操作。计划类型使用 `&dyn Any`，接口层不绑定 planner 的具体计划类型。
- `TxnContextProvider`：提供 InfoSchema/作用域查询、读 TS 与 ForUpdate TS、对应快照、初始化及语句生命周期钩子、错误建议、惰性事务激活、本地临时表通知和提交前选项设置。
- `TxnManager`：覆盖 provider 的全部事务能力，并增加 `GetContextProvider`、`EnterNewTxn`、`OnTxnEnd`、`OnStmtEnd` 和 `GetCurrentStmt`，即会话级选择/持有 provider 与记录当前语句所需的额外边界。
- `TxnManagerContext`：只有 `txn_manager(&mut self)`，要求上下文返回一个与自身生命周期绑定的可变 `dyn TxnManager`。
- `GetTxnManager`：对 `TxnManagerContext::txn_manager` 的 Go 风格命名适配。
- `AdviseOptimizeWithPlanAndThenWarmUp`：先调用 `AdviseOptimizeWithPlan`，成功后才调用 `AdviseWarmup`。
- `NewTxn`：构造 `Type = EnterNewTxnDefault`、`TxnMode = ast::Optimistic` 的请求并调用 `EnterNewTxn`。
- `NewTxnInStmt`：先调用 `NewTxn`；成功后从管理器读取 `GetCurrentStmt`，再把该值传给 `OnStmtStart`。

## 执行流程

典型调用链按接口契约可分为以下阶段：

1. 会话对象实现 `TxnManagerContext` 并长期持有某个 `TxnManager`；调用方用 `GetTxnManager` 借用它。
2. 进入事务时，上层直接构造 `EnterNewTxnRequest` 调用 `TxnManager::EnterNewTxn`，或使用 `NewTxn` 固定选择默认、乐观模式。provider 的选择、替换和 `OnInitialize` 调用属于管理器实现职责，不在本文件展开。
3. 若事务是在已经开始的语句内部重建，`NewTxnInStmt` 在 `NewTxn` 成功后读取管理器保存的当前 AST，并调用 `OnStmtStart`。即使 `GetCurrentStmt` 返回 `None`，仍必须调用该钩子；`pkg/sessiontxn/interface_test.rs` 和 `pkg/sessiontxn/txn_manager_test.rs` 都验证了此顺序。
4. 执行期按语句种类取得 `GetStmtReadTS` 或 `GetStmtForUpdateTS`，也可直接取得绑定相应 TS 的快照；需要实际 KV 事务时调用 `ActivateTxn`。
5. 普通语句依次经过 `OnStmtStart`，成功时走 `OnStmtCommit`，失败时走 `OnStmtRollback`；内部重试走 `OnStmtRetry`。悲观 DML/`SELECT FOR UPDATE` 另以 `OnPessimisticStmtStart` 和 `OnPessimisticStmtEnd` 包围加锁阶段。
6. 指定处理点发生错误时，调用 `OnStmtErrorForNextAction` 获取建议；调用方需根据 `StmtActionError`、`StmtActionRetryReady`、`StmtActionNoIdea` 决定返回或继续协调重试。
7. 提交前调用 `SetOptionsBeforeCommit`，把真实 KV 事务和 commit TS 校验回调交给 provider；语句与事务收尾分别由 `OnStmtEnd` 和 `OnTxnEnd` 完成。
8. 如果调用优化辅助函数，顺序固定为计划优化后预热。优化阶段返回错误时 `?` 立即短路，预热不会发生；测试 `advise_optimize_with_plan_and_then_warm_up_short_circuits_when_optimize_fails` 固化了该不变量。

## 数据与状态

本文件没有全局可变状态，也没有具体 manager/provider 字段；状态通过请求值和 trait 对象的可变借用跨边界传递。

- `EnterNewTxnRequest::Provider` 是 `Option<Box<dyn TxnContextProvider>>`，表达请求可以转移一个预构造 provider 的唯一所有权；请求以 `&mut` 传给 `EnterNewTxn`，允许实现消费或调整其内容。
- `TxnMode` 使用字符串而非 Rust enum，以保留 Go `ast.Optimistic`、`ast.Pessimistic` 或空字符串的兼容取值。新增调用者不能假设空串等于乐观模式。
- `StaleReadTS == 0` 表示非陈旧读，非零值表示历史读时间戳；`CausalConsistencyOnly` 默认为 `false`。
- `InfoSchemaRef` 用 `Arc` 共享元数据快照；`Snapshot` 与 `Transaction` 用 `Box<dyn ...>` 表达堆上动态对象的所有权；`Statement` 用单线程引用计数 `Rc` 共享 AST。
- `GetCurrentStmt` 返回克隆后的 `Option<Statement>`，使 `NewTxnInStmt` 能在可变借用管理器期间把当前语句再次传入 `OnStmtStart`。
- `StmtErrorAdvice` 中动作与可选错误必须保持一致：三个辅助函数保证 `StmtActionError` 携带错误，而另外两个动作不携带错误。trait 本身没有类型级约束阻止自定义实现返回不一致组合，调用者应按契约处理。

## 依赖与调用关系

直接生产依赖由 `pkg/sessiontxn/Cargo.toml` 与文件导入共同确认：

- `astersql-sessionctx` 提供统一错误和执行上下文；
- `astersql-infoschema` 提供事务可见元数据接口；
- `astersql-kv` 提供快照与事务 trait；
- `astersql-parser-ast` 提供语句节点以及 `Optimistic` 模式常量。

crate 根 `pkg/sessiontxn/lib.rs` 是直接上游装配点。仓库内 Rust 直接使用证据主要集中在本 crate 的 `failpoint.rs`、`txn_context_test.rs`、`txn_manager_test.rs`、`txn_rc_tso_optimize_test.rs` 和 `interface_test.rs`：例如 failpoint 辅助通过 `GetTxnManager` 读取 InfoSchema/读 TS，测试通过 mock manager/provider 验证转发与生命周期顺序。`rg` 还命中若干以 `.rs` 保存但内容仍为 Go 语法的迁移测试；这些文本不能作为 Rust 已完成生产接线的证据。

RustCodeGraph 的文件节点报告 `interface.rs` 被 16 个文件引用，并确认 `NewTxnInStmt`、`AdviseOptimizeWithPlanAndThenWarmUp`、`GetTxnManager` 等符号；但对这些泛型自由函数执行精确 `callers/callees` 没有返回边。因此本文只采用源码可直接复核的调用链，不据此声称完整上游覆盖。具体隔离策略可参考 `pkg/sessiontxn/isolation/lib.rs`，但该子 crate 当前定义自己的 `TxnContextProvider`，不能当作本 trait 的实现。

## 错误处理与边界

- 所有可失败入口统一返回 `Result<_, Error>`，其中 `Error` 是 `sessionctx::GoError`。本文件不包装或记录错误；`NewTxn` 原样传播 `EnterNewTxn` 错误，`NewTxnInStmt` 先传播进入事务错误，再传播 `OnStmtStart` 错误。
- `AdviseOptimizeWithPlanAndThenWarmUp` 使用短路传播：优化失败时不预热；优化成功而预热失败时返回预热错误。
- `OnStmtErrorForNextAction` 不返回 `Result`，而返回动作与可选错误的二元组。`NoIdea` 的 Go 契约是把是否重试留给其他组件；若其他组件也无判断，应返回原始错误，而不能把 `NoIdea` 当成成功。
- `SetOptionsBeforeCommit` 的校验器仅以 `Fn(u64) -> bool` 提供同步判定，接口没有说明失败时的错误种类；实现负责把校验失败映射为 `Error`。
- trait 没有默认方法。新增生产实现必须覆盖全部钩子，不能以“接口存在”推断所有隔离级别已有等价行为。
- `GetTxnManager` 要求调用者实现 `TxnManagerContext`；它不提供缺失管理器的可恢复分支。真实会话必须保证管理器在会话生命周期内始终可取。
- 当前 Rust 接口中的 `Provider` 为公开字段，而 Go 字段注释称 provider；兼容修改需同时检查替换 provider 与 stale-read 路径，不能只验证默认乐观事务。

## 并发与资源生命周期

本接口以 `&mut self` 驱动绝大多数会改变 provider/manager 状态的操作，Rust 借用规则保证一次借用范围内不会并发修改同一实例。但 `TxnAdvisable`、`TxnContextProvider`、`TxnManager` 和 `TxnManagerContext` 均未声明 `Send`/`Sync` 超 trait，`Statement` 又采用 `Rc`，所以本接口本身不承诺跨线程共享；跨线程执行若需要，应在更高层转换所有权或提供额外同步边界，不能直接假设 trait object 可发送。

`InfoSchemaRef` 的 `Arc` 允许元数据对象共享所有权；`Snapshot`、`Transaction` 和可替换 provider 的 `Box` 则由获得者独占，并在离开所有权作用域时释放。`GetTxnManager` 返回的引用绑定到会话可变借用，不会比会话存活更久。`GetContextProvider` 同样返回管理器内部 provider 的可变借用，使用期间会阻止再次可变借用 manager。

生命周期顺序属于语义契约：provider 应先经 `OnInitialize`，再接收语句钩子；`NewTxnInStmt` 只在新事务成功后触发 `OnStmtStart`；提交选项只能在执行完成、真正提交之前设置；`OnStmtEnd` 与 `OnTxnEnd` 分别标记语句和事务资源收尾。测试夹具还验证 `OnStmtRollback` 的 provider 错误必须传到调用方，以及语句/事务结束会清除 mock manager 的当前语句；后一行为由具体 manager 实现负责，并非本 trait 的自动实现。

## 与 Go 版本的对应关系

`pkg/sessiontxn/interface.go` 与本文件的主体一一对应：进入事务枚举/请求、错误处理枚举与辅助函数、三层接口、优化+预热、`NewTxn`、`NewTxnInStmt` 均保留。关键流程也一致：`NewTxn` 使用默认进入类型和 `ast.Optimistic`；`NewTxnInStmt` 在成功进入后无条件把 `GetCurrentStmt` 的结果交给 `OnStmtStart`；优化失败会阻止 warmup。

Rust 为所有权和动态分发做了以下适配：

- Go 的 `error`、`context.Context`、`infoschema.InfoSchema`、`kv.Snapshot`、`kv.Transaction`、`ast.StmtNode` 分别映射为本文件的六个类型别名。
- Go 的可空接口值映射为 `Option<Box<dyn TxnContextProvider>>` 或 `Option<Statement>`；错误建议的第二返回值映射为 `Option<Error>`。
- Go 的 `GetTxnManager` 是包级可注入函数变量，Rust 改为 `TxnManagerContext` + 普通泛型函数，避免全局函数指针，同时要求会话显式拥有 manager。
- Go 接口方法隐含引用语义；Rust 对会改变缓存、TS 或生命周期状态的方法显式使用 `&mut self`。
- Go 的 `SetOptionsBeforeCommit` 接收事务接口值；Rust 接收 `&mut dyn kv::Transaction`，避免转移调用方的事务所有权。

仍存在必须明确的迁移差异：Go 生产包已经由真实 session 注入和调用 `GetTxnManager`；当前 Rust 搜索未发现本文件 traits 的生产实现。isolation 子 crate 的 provider trait 也尚未与本文件统一，其 `OnInitialize`、InfoSchema、计划检查、激活事务和提交选项等签名不同。后续迁移需做显式适配或统一，而不能仅凭同名认定语义已经贯通。

## 扩展指南

- 新增进入事务场景时，应同时修改 `EnterNewTxnType`、管理器/provider 的分派实现和 Go 对照语义；为默认值、显式 BEGIN、语句前惰性模式、替换 provider 分别保留兼容行为。同步扩展独立测试文件 `pkg/sessiontxn/txn_manager_test.rs`，不要把测试嵌入 `interface.rs`。
- 新增请求参数时，必须更新 `EnterNewTxnRequest::default`，并判断空值是否与 Go 零值一致；同步更新 `sessiontxn_aster_unit_test.rs` 的默认值断言以及所有结构体字面量。
- 新增生命周期钩子时，先确定它属于 provider 还是 manager 独有能力，再同步两层 trait、所有实现/适配器和调用顺序测试。涉及真实隔离策略时还需处理 `pkg/sessiontxn/isolation/lib.rs` 的独立接口，而不是只改本文件。
- 修改错误建议时，应维持“处理点”和“动作”正交；为错误携带、重试准备、无意见回退各加独立断言，并检查调用方没有把 `NoIdea` 当成吞错。
- 修改 `NewTxnInStmt` 或优化/预热顺序时，优先扩展 `interface_test.rs` 与 `txn_manager_test.rs` 的事件序列测试；这些顺序影响 provider 初始化、TS 预取和语句状态，属于兼容性边界。
- 建立生产接线时，需要在真实 session 类型上实现 `TxnManagerContext`，为真实 manager 实现本文件的 `TxnManager`/`TxnAdvisable`，并为 isolation providers 提供签名适配。需要重点评估 `Rc<StmtNode>` 的线程边界、`Box<dyn Transaction>` 的所有权、动态分发开销，以及 Go 接口零值/空字符串语义。
- 如果改变任何公共类型别名或 trait 方法签名，先用 RustCodeGraph/`rg` 重查所有使用点；这是 crate 根公开再导出的 API，影响不局限于当前文件。

## 验证依据

- 目标源码：`pkg/sessiontxn/interface.rs`，核对全部别名、枚举、结构体、trait、辅助函数及其实际控制流。
- crate 边界：`pkg/sessiontxn/Cargo.toml` 与 `pkg/sessiontxn/lib.rs`，确认包映射、直接依赖、模块装配和公开再导出；该目录没有 `doc.go`。
- Go 基线：`pkg/sessiontxn/interface.go`，核对零值、四种进入方式、错误建议、trait 方法语义、`NewTxn`/`NewTxnInStmt` 和包级 `GetTxnManager`。
- Rust 独立测试：`pkg/sessiontxn/interface_test.rs`、`pkg/sessiontxn/txn_manager_test.rs`、`pkg/sessiontxn/sessiontxn_aster_unit_test.rs`、`pkg/sessiontxn/txn_context_test.rs`，确认默认值、调用顺序、优化失败短路、错误传播、错误动作转发和生命周期事件。
- 邻接实现：`pkg/sessiontxn/isolation/lib.rs` 与 `pkg/session/txnmanager.rs`。它们证明仓库存在事务策略和另一套管理器迁移实现，同时也证明其签名尚未实现本文件定义的 traits，故未把它们描述为本接口的生产实现。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/sessiontxn` 覆盖目标及 Go/Rust 对照；`node --file pkg/sessiontxn/interface.rs` 显示本文件 263 行并被 16 个文件使用；精确 `query` 找到 Rust/Go 的 `NewTxnInStmt`、`AdviseOptimizeWithPlanAndThenWarmUp`、`GetTxnManager`、`ErrorAction`、`RetryReady`、`NoIdea`。对三个泛型入口的精确 `callers/callees` 未返回结果，因此调用关系另以源码搜索复核并在本文标明限制。
- 结构验收使用任务指定命令，要求目标文件存在且恰好包含本页的十一个固定二级标题；另外执行 `git diff --check` 检查 Markdown 空白错误。按任务约束未运行 Cargo 或代码测试。

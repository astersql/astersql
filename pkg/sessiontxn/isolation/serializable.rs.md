# `pkg/sessiontxn/isolation/serializable.rs`

## 文件定位

本文件属于 `astersql-sessiontxn-isolation` crate；其 crate 根为 [`lib.rs`](lib.rs)，并由 `lib.rs` 的 `mod serializable` / `pub use serializable::*` 纳入模块并公开符号。crate 的 Go 对照包由 [`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package = "pkg/sessiontxn/isolation"` 明确为 `pkg/sessiontxn/isolation`。

它是悲观事务在 `Serializable` 隔离级别下的专用事务上下文 Provider。上游选择链有两层直接证据：`pkg/session/txnmanager.rs::newProviderWithRequest` 在事务模式为 `Pessimistic`、新事务隔离级别为 `Serializable` 时调用 `NewPessimisticSerializableProvider`；随后本 crate 的 `NewRegisteredTxnContextProvider` 将 `ProviderKind::PessimisticSerializable` 映射为 `NewPessimisticSerializableTxnContextProvider`。统一枚举 `RegisteredTxnContextProvider::PessimisticSerializable` 再把初始化、时间戳、快照及错误处理调用分派到本文件的实现。

## 核心职责

1. `NewPessimisticSerializableTxnContextProvider` 用 `IsolationLevel::Serializable` 和 `pessimistic = true` 构造共享的 `BaseTxnContextProvider`，同时原样传入 `causal_consistency_only`。
2. `OnInitialize`、`GetStmtReadTS` 和 `GetStmtForUpdateTS` 将事务准备、激活及时间戳取得委托给基类。基类保证常规情况下读 TS 与 FOR UPDATE TS 都锚定事务 `start_ts`；会话设置 `snapshot_ts` 时，两者改为该快照 TS。
3. 两个快照入口确保返回快照带有 `IsolationLevel::Serializable`；FOR UPDATE 快照直接复用读快照路径，因此二者的 TS 与隔离属性一致。
4. `OnStmtErrorForNextAction` 固定 Serializable 的锁错误策略：在 `AfterPessimisticLock` 切入点返回携带原错误的 `StmtErrorAction::Error`，不建议重试；其他切入点返回 `NoIdea`。

该文件不自行实现 Oracle、KV 事务、InfoSchema、提交选项或锁生命周期；这些能力都由 `BaseTxnContextProvider` 和注入的 `IsolationRuntime` 提供。

## 主要符号

- `pub struct PessimisticSerializableTxnContextProvider { pub base: BaseTxnContextProvider }`：唯一生产类型。公开 `base` 让统一注册器和测试复用共享状态机；专用类型自身没有额外字段。
- `pub fn NewPessimisticSerializableTxnContextProvider(runtime: Box<dyn IsolationRuntime>, causal_consistency_only: bool) -> PessimisticSerializableTxnContextProvider`：构造入口。它向 `BaseTxnContextProvider::new` 传入 `Serializable`、`true` 和调用者的因果一致性标志。
- `OnInitialize(context, enter) -> Result<(), TxnError>`：转发到 `base.OnInitialize`。进入类型决定立即激活还是仅准备状态。
- `GetStmtReadTS() -> Result<u64, TxnError>`：转发到基类；可能惰性激活事务。
- `GetStmtForUpdateTS() -> Result<u64, TxnError>`：转发到基类，而基类再复用读 TS，所以 Serializable 下两者相等。
- `GetSnapshotWithStmtReadTS() -> Result<Snapshot, TxnError>`：先取得读 TS，再调用 `base.GetSnapshotByTS(timestamp, IsolationLevel::Serializable)`。
- `GetSnapshotWithStmtForUpdateTS() -> Result<Snapshot, TxnError>`：直接调用 `GetSnapshotWithStmtReadTS`，保持两个快照入口完全一致。
- `OnStmtErrorForNextAction(point, error) -> StmtErrorAction`：专用错误决策，不访问或改变 Provider 状态。

本文件没有模块级常量、trait、条件编译项或私有辅助函数；所有符号均为公开生产 API 或公开字段。

## 执行流程

典型流程如下：

1. 会话进入新事务时，`pkg/session/txnmanager.rs::newProviderWithRequest` 根据悲观模式和 Serializable 隔离级别选择相应 Provider；隔离 crate 的注册路径则通过 `ProviderKind::PessimisticSerializable` 构造本类型。
2. `RegisteredTxnContextProvider::OnInitialize` 分派到本文件的 `OnInitialize`，后者调用 `BaseTxnContextProvider::OnInitialize`。基类写入运行上下文和事务属性（`is_pessimistic = true`、`isolation = Serializable`），准备时间戳 Future，并按 `EnterNewTxnType` 决定是否立即 `ActivateTxn`。
3. 语句需要时间戳时，注册器分别调用 `GetStmtReadTS` 或 `GetStmtForUpdateTS`。基类的 `GetStmtReadTS` 先确保事务激活，再返回非零 `snapshot_ts` 或事务 `start_ts`；`GetStmtForUpdateTS` 复用同一路径。
4. 语句需要一致性读视图时，`GetSnapshotWithStmtReadTS` 用上述 TS 调用 `GetSnapshotByTS`。运行时创建快照后，基类将其 `isolation` 设为 `Serializable`。FOR UPDATE 快照入口直接复用此流程。
5. 悲观加锁后出现错误时，统一注册器先保存最新 `RuntimeContext` 到 `provider.base.context`，再调用本文件的 `OnStmtErrorForNextAction`。该函数返回 `Error(error)`，上层据此直接传播错误；非锁后切入点没有专用建议。

`EnterNewTxnType::BeforeStmt` 是惰性路径：初始化阶段不激活；第一次读取 TS 时才由 `GetStmtReadTS -> ActivateTxn` 消费时间戳并激活事务。`serializable_test.rs::serializable_initialize_tracks_enter_type_and_causal_consistency` 覆盖了这一流程。

## 数据与状态

专用 Provider 只持有一个 `BaseTxnContextProvider`。与本文件行为直接相关的基类状态包括：

- `isolation = IsolationLevel::Serializable` 和 `pessimistic = true`：构造后固定，并在初始化及激活时写入会话事务状态。
- `causal_consistency_only`：从构造参数保留到激活选项；为 `false` 时仍须结合会话条件决定 `guarantee_linearizability`，不能仅凭 Serializable 推断一定启用全局线性一致性。
- `is_txn_prepared`、`prepared_future`、`txn_active`、`enter_new_txn_type`：控制时间戳准备和惰性激活。具体转换发生在 `BaseTxnContextProvider::{OnInitialize,PrepareTxn,ActivateTxn}`。
- `context`：初始化、语句开始和统一错误分发时更新，供运行时操作使用；本文件的错误决策本身不读取它。
- 运行时会话中的 `snapshot_ts` 与 `txn.start_ts`：决定读/FOR UPDATE TS。`snapshot_ts != 0` 优先；否则使用激活事务的 `start_ts`。

`Snapshot` 是值对象，包含 `timestamp`、`isolation`、`from_active_transaction` 和 `rc_check_ts`。本文件只明确设置前两项的来源；`from_active_transaction` 由基类根据活动事务 TS 是否匹配计算，底层快照则由 `IsolationRuntime::snapshot` 创建。

## 依赖与调用关系

上游直接关系：

- `pkg/session/txnmanager.rs::newProviderWithRequest`：悲观 Serializable 的会话级选择点。
- `isolation/lib.rs::NewRegisteredTxnContextProvider`：把 `ProviderKind::PessimisticSerializable` 构造成该 Provider。
- `isolation/lib.rs::TxnContextProvider for RegisteredTxnContextProvider`：分发 `OnInitialize`、两个 TS 方法、两个快照方法和 `OnStmtErrorForNextAction`；其他通用能力通过 `Base()` / `BaseRef()` 访问基类。

下游直接关系：

- `BaseTxnContextProvider::{new,OnInitialize,GetStmtReadTS,GetStmtForUpdateTS,GetSnapshotByTS}`：本文件所有有状态工作和快照构造的实际实现。
- `IsolationRuntime`：通过基类间接提供 Oracle Future、事务激活、会话状态和快照创建。本文件拥有 `Box<dyn IsolationRuntime>` 的间接所有权，但不依赖具体实现。
- `TxnError`、`StmtErrorHandlePoint`、`StmtErrorAction`、`Snapshot`、`IsolationLevel`：定义返回值和分支契约。

`Cargo.toml` 未为本文件设置 feature 或条件依赖；crate 使用 workspace 版本、edition 和 publish 设置。文件本身只从 crate 根导入符号，具体跨 crate 依赖被基类和 crate 根封装。RustCodeGraph 将目标文件记录为由 `pkg/sessiontxn/isolation/lib.rs` 使用；对目标精确符号执行 `callers` / `callees` 未得到静态边，因此这里的调用关系以索引返回的模块源码和实际分派分支为依据，不声称图中存在未返回的边。

## 错误处理与边界

- 构造函数不返回错误；无效或失败的运行时操作在后续基类调用中通过 `TxnError` 暴露。
- `OnInitialize` 完整传播基类错误，包括旧事务提交、Oracle Future 准备或事务激活失败。
- `GetStmtReadTS` / `GetStmtForUpdateTS` 完整传播时间戳准备、等待和事务激活错误。基类还拒绝非预设 `start_ts` 早于会话 `last_commit_ts` 的情况。
- `GetSnapshotWithStmtReadTS` 用 `?` 先传播 TS 错误，再传播 `IsolationRuntime::snapshot` 经 `GetSnapshotByTS` 返回的错误；不会返回部分构造的快照。
- 锁后错误不按 `TxnErrorKind` 区分：写冲突、可重试死锁、不可重试死锁和一般事务错误均原样放入 `StmtErrorAction::Error`。这正是 Serializable 禁止该路径自动重试的边界。
- 对 `AfterQuery` 等其他处理点返回 `NoIdea`，表示本 Provider 不作决定，而不是表示成功、忽略错误或可重试。
- 初始化前直接触发依赖 InfoSchema 的基类路径可能命中基类的初始化前置条件；安全调用顺序是先 `OnInitialize`，再进入语句/TS/快照操作。

## 并发与资源生命周期

本类型使用 `&mut self` 执行初始化、取 TS 和建快照，状态转换按单个 Provider 的顺序调用设计；文件中没有锁、原子量、线程、异步任务或通道，也没有声明跨线程共享保证。不得仅凭底层数据库并发能力把该实例当作可并发调用对象。

`Box<dyn IsolationRuntime>` 由 `BaseTxnContextProvider` 独占，随 Provider 一同释放。准备阶段持有的 `Box<dyn TimestampFuture>` 在激活时等待并消费其结果；事务真正的存储资源生命周期由 `IsolationRuntime::activate_transaction` 及更上层事务管理器负责。本文件不显式提交、回滚或释放 KV 事务。

`RegisteredTxnContextProvider` 在事务存续期间持有该实例；会话级 `TxnManager::OnTxnEnd` 清除其 Provider。快照是按值返回，所有权交给调用者。本文件的错误决策不重试、不生成后台工作，也不延长错误或快照以外资源的生命周期。

## 与 Go 版本的对应关系

Rust 文件直接对应 [`serializable.go`](serializable.go)：两者都有同名 `PessimisticSerializableTxnContextProvider`、构造函数和 `OnStmtErrorForNextAction`，核心语义一致：悲观事务、Serializable 隔离、读 TS 与 FOR UPDATE TS 都取事务起始 TS、锁后错误直接返回。

实现形态存在可追溯差异：

- Go 构造函数通过 `onInitializeTxnCtx` 回调设置 `TxnCtx.IsPessimistic` / `TxnCtx.Isolation`，并通过 `onTxnActiveFunc` 设置 `kv.Pessimistic`；Rust 将这些参数传给 `BaseTxnContextProvider::new`，由基类初始化状态并组装 `TxnActivationOptions`。
- Go 把 `getStmtForUpdateTSFunc` 和 `getStmtReadTSFunc` 都绑定到 `getTxnStartTS`；Rust 的专用方法转发到基类，而基类在 `snapshot_ts` 非零时优先返回快照 TS。这一快照覆盖语义由两边测试共同确认。
- Go 的错误钩子返回 `(StmtErrorAction, error)`；Rust 用 `StmtErrorAction::Error(TxnError)` 把动作与原错误合并为一个枚举值。
- Rust 显式实现两个快照入口并强制快照的隔离枚举为 `Serializable`；Go 通过嵌入的 `baseTxnContextProvider` 获得相应行为。

[`serializable_test.rs`](serializable_test.rs) 对照 [`serializable_test.go`](serializable_test.go)，已覆盖 Provider 层可独立验证的四组语义：TS 锚定、锁错误不重试、不同进入类型与因果一致性标志、`tidb_snapshot` 覆盖。Rust 测试文件也明确记录，Go 测试中的真实 INSERT/TiKV scope 注入属于 executor/session 集成场景，并未在该 Provider 单元测试中伪造为已覆盖。

## 扩展指南

- 若改变 Serializable 的 TS 规则，优先修改 `GetStmtReadTS` / `GetStmtForUpdateTS` 或基类中被复用的实现，并同步检查快照方法；必须保持读 TS、FOR UPDATE TS、`snapshot_ts` 覆盖和惰性激活之间的明确不变量。
- 若添加新的错误处理切入点或重试策略，修改 `OnStmtErrorForNextAction`，并在独立测试文件 [`serializable_test.rs`](serializable_test.rs) 中分别覆盖每个 `StmtErrorHandlePoint` 和相关 `TxnErrorKind`。不要把测试嵌入生产源文件。
- 若新增专用状态，先判断是否属于所有隔离级别共享状态；共享状态应进入 `BaseTxnContextProvider`，只对 Serializable 有效的状态才进入本结构，并同步更新 `RegisteredTxnContextProvider::{Base,BaseRef}` 或相关分发分支。
- 若新增 `TxnContextProvider` trait 方法，必须同时更新 `isolation/lib.rs` 的枚举分发，以及所有其他 Provider；只在本文件新增同名方法不会自动进入统一调用链。
- 若改变构造参数、Provider 种类或会话选择条件，同时检查 `NewRegisteredTxnContextProvider`、`pkg/session/txnmanager.rs` 的工厂选择和 Go 对照。兼容风险集中在事务模式/隔离级别选择及错误重试行为；性能风险集中在是否额外请求 Oracle TS、是否重复激活或创建快照。
- 修改后至少同步 Provider 单元测试；涉及真实 SQL、作用域、KV 客户端或 DML 激活语义时，还应增加对应 session/executor 集成测试，不能用 MockRuntime 测试替代。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/sessiontxn/isolation`：确认目标源、crate 根、Go 对照及 Rust/Go 独立测试均在索引中。
- RustCodeGraph `node --file pkg/sessiontxn/isolation/serializable.rs`：核对本文件全部 94 行、9 个符号及唯一模块使用方 `isolation/lib.rs`。
- RustCodeGraph `query`：核对 `PessimisticSerializableTxnContextProvider`、`NewPessimisticSerializableTxnContextProvider`、`GetSnapshotWithStmtReadTS`、`OnStmtErrorForNextAction` 的 Rust/Go 候选与位置。
- RustCodeGraph `callers` / `callees`：对目标关键符号执行精确查询但未返回边；随后用索引的 `isolation/lib.rs` 和 `base.rs` 源码节点验证实际构造、分发和下游调用。
- 已读生产证据：[`serializable.rs`](serializable.rs)、[`lib.rs`](lib.rs)、[`base.rs`](base.rs)、`pkg/session/txnmanager.rs`、[`Cargo.toml`](Cargo.toml) 和 [`serializable.go`](serializable.go)。
- 已读测试证据：[`serializable_test.rs`](serializable_test.rs) 与 [`serializable_test.go`](serializable_test.go)；前者由 `lib.rs` 的 `#[cfg(test)] mod serializable_test` 注册为独立测试模块。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务文件规定的命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核唯一生产物与源码事实一致。

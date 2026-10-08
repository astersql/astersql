# `pkg/sessiontxn/isolation/base.rs`

源文件：[base.rs](./base.rs)；所属 crate：`astersql-sessiontxn-isolation`（[Cargo.toml](./Cargo.toml)）。

## 文件定位

`base.rs` 是会话事务隔离 crate 的公共状态机与运行时边界。它不选择具体隔离级别，而是定义四种 Provider 共用的事务初始化、时间戳准备/激活、快照、提交选项和悲观公平锁骨架。模块入口 [lib.rs](./lib.rs) 将本文件导出，并由 `RegisteredTxnContextProvider`/`TxnContextProvider` 把上层调用分发给 `optimistic.rs`、`readcommitted.rs`、`repeatable_read.rs`、`serializable.rs` 中的具体 Provider。

本文件同时是一个适配层：`IsolationRuntime` 把状态机与真实会话、Oracle、KV 事务、InfoSchema 和公平锁实现隔开；`PlanInspection`、`StatementInspection` 则只暴露隔离策略所需的计划形状和语句只读属性。因此这里保存的是可执行的隔离状态机和选项语义，不是完整的 session/store 实现。当前 Rust 测试使用 [main_test.rs](./main_test.rs) 的内存 `MockRuntime` 驱动这条边界；Go 版本则直接连接 `sessionctx.Context` 和 `kv.Transaction`。

## 核心职责

1. 定义隔离层通用词汇：进入事务方式、隔离级别、副本读、断言、磁盘满策略、预写遇锁策略、错误分类以及计划形状（`EnterNewTxnType`、`IsolationLevel`、`ReplicaReadMode`、`TxnErrorKind`、`PlanKind` 等）。
2. 用 `SessionState`/`TxnContextState` 表达状态机所观察和更新的会话事务状态；默认值包括全局事务作用域、自动提交、50 秒锁等待、开启 MDL 和线性一致性等。
3. 用 `BaseTxnContextProvider` 完成“初始化事务上下文 → 准备时间戳 Future → 等待 TS 并激活 → 提供语句读/写 TS 与快照 → 提交前下发选项”的主流程。
4. 用 `BasePessimisticTxnContextProvider` 补充悲观事务的公平锁起止、重试/取消以及语句锁缓存状态转换。
5. 把所有外部副作用收敛到 `IsolationRuntime`，使具体 Provider 只覆盖隔离级别差异，同时让测试可以观察调用顺序和生成的选项。

## 主要符号

- `MAX_TIMESTAMP`：`u64::MAX`，供点查等常量 start TS 优化使用；本文件通过 `ForcePrepareConstStartTS` 接受该类预设值，具体计划判定在子 Provider 中。
- `RuntimeContext`：随操作传递请求 ID 与取消标记；`OnInitialize`、`OnStmtStart`、重试和公平锁回调保存或转发它。
- `TxnContextState`：事务级可变状态，包含 `start_ts`、`for_update_ts`、作用域、InfoSchema 版本、隔离级别、重试/历史/过期读及语句锁缓存标记。
- `SessionState`：状态机所需的会话投影，涵盖 snapshot、自动提交、重试、RC check-ts、公平锁、pipelined DML、临时表、CDC、拦截器、提交协议和线性一致性配置；它不是完整的 Rust session 对象。
- `PlanInspection` / `StatementInspection`：计划和 AST 的最小只读适配 trait；本文件只声明接口，具体隔离 Provider 消费 `PlanKind` 或 `is_read_only`。
- `TimestampFuture` / `ConstantFuture`：抽象异步 Oracle TS；常量实现用于 snapshot TS、已有事务 TS 或强制常量 TS。
- `TxnInfoSchema` / `TxnInfoSchemaRef`：只暴露 schema 版本和是否已会话扩展。引用类型为单线程 `Rc<dyn TxnInfoSchema>`。
- `TxnActivationOptions`：激活 KV 事务时一次性传给运行时的配置快照，包含隔离、作用域、副本读、拦截器、提交协议、请求来源、session ID 和线性一致性等。
- `CommitOptions`：提交前下发的 schema 校验、物理表/临时表、CDC、拦截器、commit-ts checker 和预写遇锁策略。
- `Snapshot`：时间戳、隔离级别、是否复用活跃事务快照以及 RC check-ts 标记的抽象结果。
- `IsolationRuntime`：本文件唯一的强集成边界，负责会话访问、InfoSchema、Oracle、事务激活、快照、提交选项、临时表与公平锁副作用。
- `BaseTxnContextProvider`：核心基类；`prepared_future` 私有保存尚未消费的 TS Future，其余字段记录构造配置与准备/激活状态。
- `BasePessimisticTxnContextProvider`：包装 `BaseTxnContextProvider`，集中实现悲观公平锁和语句锁缓存生命周期。

## 执行流程

1. 构造：`BaseTxnContextProvider::new` 固定隔离级别、悲观/乐观属性和 `causal_consistency_only`，其余状态均为未初始化、未准备、未激活。
2. 初始化：`OnInitialize` 保存上下文，并按 `EnterNewTxnType` 分支。
   - `Default`：先调用 `commit_before_enter_new_txn`，再强制走 Oracle 准备 TS，随后立即激活。
   - `WithBeginStmt`：仅当 `CanReuseTxnWhenExplicitBegin` 为假时提交旧事务并重新准备；设置 `session.in_txn = true`，随后立即激活。
   - `BeforeStmt`：只初始化状态，不立即激活，以保留惰性取 TS。
   初始化还缓存最新 InfoSchema、重建 `TxnContextState`，并优先接管已有事务 start TS 或运行时预先准备的 Future。
3. 准备 TS：`PrepareTxn` 幂等地选择 `snapshot_ts` 常量 Future，否则调用 `PrepareTxnWithOracleTS`；后者把事务作用域和低精度 TSO 开关交给 `IsolationRuntime::oracle_future`。`ReplaceTxnTsFuture` 在事务已经激活后是无操作。
4. 激活：`ActivateTxn` 幂等返回已激活的 `start_ts`；否则确保已准备、等待 Future，并在非预设 TS 情况下拒绝早于 `last_commit_ts` 的时间戳。`BeforeStmt` 且非自动提交/非 snapshot 时会进入显式事务状态。随后函数从 `SessionState` 生成完整 `TxnActivationOptions`，调用 `activate_transaction`，并把 start/for-update TS、悲观标志和隔离级别写回 `TxnContextState`。
5. 语句 TS：`GetTxnStartTS` 直接确保激活；`GetStmtReadTS` 在激活后优先返回 `snapshot_ts`，否则返回事务 start TS；基类 `GetStmtForUpdateTS` 与读 TS 相同，RC/RR 等子类可覆盖刷新规则。
6. 语句生命周期：`OnStmtStart` 更新上下文；`OnStmtRetry` 同时清空当前语句锁缓存；提交/回滚基类钩子不修改状态；`OnStmtErrorForNextAction` 对悲观锁后错误返回 `Error`，其他阶段返回 `NoIdea` 交上层/子类决定。
7. 快照：两个便捷方法分别取得语句读 TS 或 for-update TS，再交给 `GetSnapshotByTS`。只有目标 TS 同时等于活跃事务的 start TS 和 for-update TS 时，`from_active_transaction` 才为真；返回前强制改写为调用者要求的隔离级别。
8. 提交：`SetOptionsBeforeCommit` 先取得事务 InfoSchema 版本。pipelined DML 遇 MDL 关闭、临时表、CDC 来源或 commit-ts checker 时返回不支持错误；否则过滤临时表 ID，构造 `CommitOptions` 并通过运行时安装。可重试的自动提交乐观事务选择 `NoResolve`，其余选择 `TryResolve`。
9. 悲观语句：`OnPessimisticStmtStart` 只在事务活跃、会话启用公平锁、有连接 ID 且非内部 SQL 时启动公平锁；`OnPessimisticStmtEnd` 成功时完成公平锁并标记锁缓存已刷，失败时取消公平锁并清空当前缓存。重试/取消辅助方法只在仍处于公平锁模式时调用运行时。

## 数据与状态

状态存在三层。`SessionState` 是会话配置和跨事务输入；`TxnContextState` 是当前事务及语句锁缓存状态；`BaseTxnContextProvider` 自身保存状态机阶段（`is_txn_prepared`、`txn_active`）、进入方式、InfoSchema、上下文和 TS Future。激活的关键不变量是：`txn_active == true` 后，`TxnContextState.start_ts` 与 `for_update_ts` 都已初始化为激活 TS，且隔离/悲观属性与 Provider 构造值一致。

`is_txn_prepared` 仅说明 Future 已安装，不等于 KV 事务已激活；`txn_active` 才表示 `activate_transaction` 已成功。`const_start_ts` 和 `snapshot_ts` 都属于预设 TS，绕过 `last_commit_ts` 的因果顺序检查；普通 Oracle TS 必须不小于会话上次提交 TS。`GetTxnInfoSchema` 优先返回会话 snapshot schema；普通 schema 只在尚未扩展时调用 `ensure_session_extended_info_schema`，然后更新缓存和事务 schema 版本。

所有集合都以拥有值传入选项：激活/提交时克隆字符串和 ID 向量，避免选项继续借用可变会话状态。`TxnInfoSchemaRef` 使用 `Rc`，说明当前边界设计为单线程引用计数，并未承诺跨线程共享。

## 依赖与调用关系

上游入口是 [lib.rs](./lib.rs) 的 `TxnContextProvider for RegisteredTxnContextProvider`：公共 InfoSchema、作用域、提交选项直接转发给 `Base()`，初始化、TS、快照和激活则按 Provider 变体分发。具体调用边包括 `optimistic.rs::OnInitialize/ActivateTxn`、`readcommitted.rs::OnInitialize/ActivateTxn`、`repeatable_read.rs::OnInitialize` 和 `serializable.rs::OnInitialize` 调用本文件基类；RC/RR 通过 `BasePessimisticTxnContextProvider` 复用公平锁逻辑。RustCodeGraph 对 `lib.rs` 的文件节点还显示其生产上游包括 `pkg/session/txnmanager.rs`，即会话事务管理器通过注册 Provider 进入此状态机。

下游全部经 `IsolationRuntime`：Oracle 路径为 `oracle_future`/`TimestampFuture::wait`，激活路径为 `activate_transaction`，读取路径为 `snapshot`，提交路径为 `set_commit_options`，schema 路径为 `latest_info_schema`/扩展/临时表挂接，悲观路径为四个 fair-lock 回调。这样 `base.rs` 不直接依赖具体 KV 客户端类型。

[Cargo.toml](./Cargo.toml) 把本 crate 声明为 `astersql-sessiontxn-isolation`，`lib.rs` 为库入口；其生产依赖列出 config/domain/infoschema/kv/parser/planner/sessionctx/sessiontxn/store-driver/table 等相邻 crate，开发依赖补充 executor/session/testkit。需要注意，`base.rs` 本身只直接使用标准库；这些跨 crate 依赖主要服务模块入口、具体 Provider 或未来真实运行时接线，不能据 Cargo 列表推断本文件已直接调用对应实现。

## 错误处理与边界

所有可失败的运行时操作统一返回 `TxnError`，并用 `TxnErrorKind` 区分运行时错误、无效事务、写冲突、死锁、锁等待、锁错误和不支持的提交选项。`TxnError` 的 `Display` 只输出消息，错误种类需由结构字段读取。Oracle Future、旧事务提交、激活、快照、提交选项及公平锁错误均用 `?` 原样向上返回。

显式本地校验有三类：激活后再强制常量 TS 返回 `InvalidTransaction`；普通 Oracle start TS 早于 `last_commit_ts` 返回 `Runtime`；pipelined DML 的四种不兼容提交配置返回 `UnsupportedCommitOption`。`prepared_future.expect(...)` 和 `info_schema.expect(...)` 是内部状态不变量断言：调用方必须先完成准备或初始化；违反时会 panic，而不是返回业务错误。

边界限制也应明确：本文件没有处理具体死锁/写冲突的重试算法，基类只对悲观锁后的错误给出默认 `Error`；RC/RR/Optimistic 子类负责覆盖差异。取消标记仅存放在 `RuntimeContext` 并传给运行时，本文件不主动轮询它。`IsolationRuntime` 当前是抽象集成面，因此真实 RPC、资源组、提交钩子和后台生命周期的安装由实现者保证。

## 并发与资源生命周期

事务生命周期是“未准备 → 已准备 Future → 已激活”；准备与激活均幂等，已激活后替换 Future 被忽略。Future 在 `ActivateTxn` 中同步 `wait`，等待失败不会设置 `txn_active`。激活成功后才写回事务状态并置位，避免把运行时激活失败误标为活跃。

公平锁生命周期是“满足条件后 start → 成功语句 done / 失败语句 cancel”；如果子类决定重试，可在锁模式尚存时调用 `RetryFairLockingIfNeeded`。语句成功设置 `flushed_stmt_lock_cache`，失败或普通重试清除 `current_stmt_lock_cache`，防止上一次尝试的锁缓存泄漏到下一次。

本文件自身不创建线程、任务、锁或通道。`Rc<dyn TxnInfoSchema>` 与 `Box<dyn IsolationRuntime>` 没有 `Send + Sync` 约束，Provider 设计上由单个会话执行流独占。Go 对照中的后台 goroutine wait-group 生命周期钩子被压缩为 `TxnActivationOptions::install_background_lifecycle_hooks` 布尔意图，实际资源计数仍属于运行时实现责任。

## 与 Go 版本的对应关系

直接对照文件为 [base.go](./base.go)。Rust 的 `BaseTxnContextProvider`、`BasePessimisticTxnContextProvider` 分别对应 Go 的 `baseTxnContextProvider`、`basePessimisticTxnContextProvider`；`OnInitialize`、`GetTxnInfoSchema`、TS 准备/激活、snapshot、提交选项和公平锁方法保持相同的主干顺序与关键分支。Rust 测试 [base_test.rs](./base_test.rs) 还明确验证了 Go 基类钩子副作用：开始语句只更新上下文，而提交/回滚基类钩子不覆盖它。

Rust 不是逐类型直译：Go 直接持有 `sessionctx.Context`、`kv.Transaction` 和 Oracle Future，并通过 `SetOption` 安装行为；Rust 把这些对象压缩为 `SessionState`、`IsolationRuntime` 以及 `TxnActivationOptions`/`CommitOptions`。Go 通过函数字段让子类提供 read/for-update TS，Rust 则由具体 Provider 方法分发覆盖。Go 的 `SetOptionsOnTxnActive` 直接安装 commit hook、RPC 拦截器、资源组 tagger 和后台 goroutine hooks；Rust 当前只把配置/安装意图传给 runtime。

存在需要扩展时特别核对的语义差异：Go `OnInitialize` 会拒绝未配置 TS 函数和未知进入类型，Rust 的枚举使未知类型不可表示，也没有函数指针空值；Go `forcePrepareConstStartTS` 只设置标志，Rust 同时安装 `ConstantFuture`；Go 创建本地临时表时会立即给活跃事务更新 snapshot interceptor，Rust 只通过 `attach_local_temporary_tables` 更新 InfoSchema；Go pipelined DML 在兼容时提前返回且不触碰事务选项，Rust仍生成带 `pipelined_noop` 的 `CommitOptions` 并调用 runtime。修改这些区域时不能只依赖名称相同，应同时复核 Go 当前行为和 runtime 实现。

## 扩展指南

- 增加新的会话/激活配置：先扩展 `SessionState` 和 `TxnActivationOptions`，在 `ActivateTxn` 中建立唯一映射，再同步所有 `IsolationRuntime` 实现与 [main_test.rs](./main_test.rs) 的 `MockRuntime` 捕获断言；对照 `base.go::SetOptionsOnTxnActive` 判断兼容性和默认值。
- 增加提交选项或限制：修改 `CommitOptions`/`SetOptionsBeforeCommit`，明确 pipelined DML 是否允许、是否需要过滤临时表，并在 [isolation_aster_unit_test.rs](./isolation_aster_unit_test.rs) 添加选项和拒绝路径用例；对照 Go `SetOptionsBeforeCommit`。
- 改变 TS 或 snapshot 规则：优先修改 `PrepareTxn*`、`ActivateTxn`、`GetStmt*TS`、`GetSnapshotByTS` 中最窄的符号，并同步具体隔离 Provider 的独立 `*_test.rs`；必须保持普通 TS 与 `last_commit_ts`、snapshot/常量预设 TS、BeforeStmt 惰性激活等不变量。
- 增加隔离级别差异：不要把策略硬编码进基类；在相应具体 Provider 覆盖 TS、错误或计划建议逻辑，并保持 [lib.rs](./lib.rs) 的注册分发完整。
- 改变公平锁：修改 `BasePessimisticTxnContextProvider` 与 runtime 回调，分别覆盖成功、失败、重试、取消，以及连接 ID 为零/内部 SQL/未激活时不启动的边界。
- 测试必须继续放在独立文件：本文件的直接基类测试在 [base_test.rs](./base_test.rs)，共享夹具在 [main_test.rs](./main_test.rs)，综合选项/InfoSchema/公平锁测试在 [isolation_aster_unit_test.rs](./isolation_aster_unit_test.rs)，各隔离级别行为在对应 `optimistic_test.rs`、`readcommitted_test.rs`、`repeatable_read_test.rs`、`serializable_test.rs`；不要把测试内嵌回 `base.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query BaseTxnContextProvider --kind struct` 定位到 `base.rs:405`；`node BaseTxnContextProvider` 核对字段；`node --file pkg/sessiontxn/isolation/base.rs --offset 535 --limit 365` 核对准备、激活、TS、snapshot、提交和公平锁实现；`node --file pkg/sessiontxn/isolation/lib.rs --offset 130 --limit 210` 核对注册 Provider 的上游分发。`callees ActivateTxn` 对目标 Rust 定义识别出 `PrepareTxn`、`TimestampFuture::wait`、`IsolationRuntime::session/session_mut/activate_transaction` 和 `TxnActivationOptions` 构造等直接下游；同名查询包含其他语言/模块结果，本文只采用路径限定到本文件的边。
- 读取的生产路径：[base.rs](./base.rs)、[lib.rs](./lib.rs)、[Cargo.toml](./Cargo.toml)、`optimistic.rs`、`readcommitted.rs`、`repeatable_read.rs`、`serializable.rs` 的直接调用位置，以及 Go 对照 [base.go](./base.go)。
- 读取的测试路径：[base_test.rs](./base_test.rs)、[main_test.rs](./main_test.rs)、[isolation_aster_unit_test.rs](./isolation_aster_unit_test.rs)，并检索四个隔离级别的 Rust/Go 独立测试。直接证据包括：基类语句钩子副作用、单调 Oracle 时钟与惰性激活、激活/提交选项保真、InfoSchema 仅扩展一次、RC snapshot 标记以及 RR 公平锁重试序列。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章结构检查，并人工确认本文区分了当前 Rust 抽象边界、Go 真实接线和未由本文件实现的子类策略。

# `pkg/sessiontxn/isolation/readcommitted.rs`

## 文件定位

本文件位于 `astersql-sessiontxn-isolation` crate，crate 入口是同目录 `lib.rs`（`Cargo.toml` 的 `[lib] path = "lib.rs"`）。`lib.rs` 将本模块声明为私有模块 `mod readcommitted`，再通过 `pub use readcommitted::*` 导出其公开符号；因此外部使用的是 crate 公共 API，而不是直接引用模块路径。

它实现悲观事务在 Read Committed（RC，读已提交）隔离级别下的事务上下文 Provider。`lib.rs::NewRegisteredTxnContextProvider` 在 `ProviderKind::PessimisticReadCommitted` 分支构造 `NewPessimisticRCTxnContextProvider`，`RegisteredTxnContextProvider` 的 `TxnContextProvider` 实现随后把初始化、语句开始/重试、时间戳、快照、错误决策与计划优化调用分派到这里。再上层的会话事务管理器通过 Provider 工厂选择悲观 RC Provider；本文件不解析 SQL，也不直接执行查询或锁请求。

## 核心职责

1. 为每条 RC 语句选择读时间戳：首条语句可用事务 `start_ts`，RC-check 只读语句可复用最近一次 Oracle 时间戳，其余情况异步向时间戳 Oracle 取新值（`PrepareStmtTS`、`GetStmtTS`）。
2. 管理 RC-check ts：`NeedSetRCCheckTSFlag` 决定只读语句能否开启检查，快照构造方法把检查标志写入 `Snapshot::rc_check_ts`；写路径还可在安全计划上复用已知时间戳，避免向 PD 取 TSO（`PlanSkipGetTSOFromPD`、`AdviseOptimizeWithPlan`）。
3. 在语句级冲突后给出下一步动作：查询后的 RC-check 写冲突可从头重试；悲观加锁后的可重试死锁或未超时写冲突可重试，其余错误返回上层（`OnStmtErrorForNextAction`）。
4. 保证 RC 的读 ts 与 `for_update_ts` 同步，并按选定时间戳更新事务快照（`GetStmtTS`、`GetStmtForUpdateTS`）。

## 主要符号

- `StmtTimestampSource`：私有枚举，明确时间戳三种来源：`StartTimestamp`、`Constant(u64)`、`Oracle(Box<dyn TimestampFuture>)`。它把“尚未决定来源”和“已决定但 Future 尚未等待”分开表示。
- `StmtState`：单语句状态。公开字段 `stmt_ts` 缓存已解析值，`stmt_use_start_ts` 记录首语句策略；私有 `stmt_ts_source` 防止一次语句重复选源。`PrepareStmt` 以新状态整体覆盖旧状态。
- `PessimisticRCTxnContextProvider`：核心状态机。`base` 提供通用悲观事务能力；`stmt` 保存语句状态；`latest_oracle_ts`/`latest_oracle_ts_valid` 维护可复用时间戳；`check_ts_in_write_stmt` 控制写语句快照的 RC-check 模式。
- `NewPessimisticRCTxnContextProvider(runtime, causal_consistency_only)`：公开构造器，将隔离级别固定为 `IsolationLevel::ReadCommitted`、开启悲观模式，并初始化缓存标志。
- `OnInitialize`、`ActivateTxn`：在事务激活后用 `start_ts` 初始化最近时间戳缓存；`ActivateTxn` 仅在从非激活变为激活时更新缓存，避免覆盖重试造成的失效状态。
- `OnStmtStart`、`OnStmtRetry`：分别建立新语句状态和重试状态。重试会令 `latest_oracle_ts_valid = false`，强制后续重新取时钟值。
- `PrepareStmtTS`、`GetStmtTS`：私有选源与公开求值核心；后者还更新 snapshot ts、`txn.for_update_ts` 和语句缓存。
- `GetStmtReadTS`、`GetStmtForUpdateTS`：公开时间戳接口。`snapshot_ts != 0` 时读接口优先使用会话快照；RC 下 for-update 接口与读接口相同。
- `OnStmtErrorForNextAction`、`HandleAfterQueryError`、`HandleAfterPessimisticLockError`：错误分类与重试决策。
- `AdviseWarmup`、`AdviseOptimizeWithPlan`：前者预备事务/时间戳来源，后者在安全条件下将写语句改为常量 ts 来源。
- `GetSnapshotWithStmtReadTS`、`GetSnapshotWithStmtForUpdateTS`：以 RC 隔离构造快照，并分别传播读语句或写语句的 RC-check 标志。
- `NeedSetRCCheckTSFlag`：公开纯谓词；要求有效连接、开启 RC 读检查、显式事务中、非重试且语句只读。
- `PlanSkipGetTSOFromPD`：公开递归计划谓词；识别可安全复用 ts 的点查、锁定物理子树、Update/Delete、纯 VALUES Insert 与 Execute 包装。
- `constant_future`：未被生产路径调用的私有辅助函数，受 `dead_code` 放行；实际生产选源直接构造 `StmtTimestampSource::Constant`。

## 执行流程

典型流程由 `lib.rs::RegisteredTxnContextProvider` 分派：

1. `NewRegisteredTxnContextProvider(PessimisticReadCommitted, ...)` 调用本文件构造器；`OnInitialize` 委托基类建立事务上下文。若事务已经激活，则把事务 `start_ts` 记为有效的 `latest_oracle_ts`。
2. 每条语句进入 `OnStmtStart`。它先执行基类钩子，清除旧 `stmt_rc_check_ts`，再由 `NeedSetRCCheckTSFlag` 对符合条件的只读事务语句重新置位；同时清除写语句检查模式，并调用 `StmtState::PrepareStmt(!is_txn_prepared)`。因此未 Prepare 的首条语句允许使用事务起始时间戳。
3. 上层可先调用 `AdviseWarmup`，让基类 Prepare 事务，并在非 `tidb_snapshot` 模式下提前选择 ts 来源；也可在已有执行计划后调用 `AdviseOptimizeWithPlan`。只有非快照、非 stale read、非首语句、缓存有效且非重试时，安全计划才会使用缓存常量 ts 并开启写语句 RC-check。
4. `GetStmtReadTS` 在 `snapshot_ts` 非零时只激活事务并返回该固定快照；否则进入 `GetStmtTS`。`GetStmtTS` 先复用本语句已解析的 `stmt_ts`，否则激活事务、调用 `PrepareStmtTS` 选源，再求值：start-ts 取激活结果，常量源取缓存值，Oracle 源调用 `wait()` 并刷新最近缓存。
5. 求得 ts 后，运行时的 transaction snapshot ts 与会话 `txn.for_update_ts` 同步更新，最后写入 `stmt.stmt_ts`。同一语句后续读取以及 `GetStmtForUpdateTS` 都返回同一值。
6. 快照接口调用相应时间戳接口，再由基类 `GetSnapshotByTS(..., ReadCommitted)` 创建 `Snapshot`。读快照使用会话 `stmt_rc_check_ts`，写快照使用 `check_ts_in_write_stmt`。
7. 若执行失败，`OnStmtErrorForNextAction` 按切入点分派。返回 `RetryReady` 后，上层调用 `OnStmtRetry`，旧缓存失效且语句不再使用 start-ts；下一次取 ts 必须走新 Oracle 值，保持重试后的 RC 可见性。

## 数据与状态

状态分为事务级与语句级。事务级的 `latest_oracle_ts` 只在 `latest_oracle_ts_valid` 为真时可复用；初始化或首次激活令其有效，`OnStmtRetry` 令其失效，成功等待 Oracle Future 后再次有效。这里的重要不变量是：`OnStmtRetry` 失效的缓存不能被随后对“已激活事务”的调用重新复活，因此 `ActivateTxn` 只在 `was_active == false` 时写缓存。

语句级的 `StmtState` 在每次 `OnStmtStart`/`OnStmtRetry` 被整体重置。`stmt_ts == 0` 表示尚未求值；一旦非零，同一语句所有读/for-update 请求都复用它。`stmt_ts_source: None` 表示尚未决定来源，与 Future 尚未 `wait()` 不同。源码以 0 作为哨兵，因此运行时提供的合法事务时间戳必须非零；该约束来自本模块的缓存判定。

`stmt_rc_check_ts` 存在于共享 `SessionState`，表示当前读语句模式；`check_ts_in_write_stmt` 存在于 Provider，表示计划优化选出的写语句模式。两者故意分离，并分别进入读快照和写快照。`txn.for_update_ts` 则在任何正常的 `GetStmtTS` 成功后与读 ts 对齐。

## 依赖与调用关系

上游直接证据来自 `pkg/sessiontxn/isolation/lib.rs`：

- `NewRegisteredTxnContextProvider` 构造 RC Provider。
- `TxnContextProvider for RegisteredTxnContextProvider` 对 RC 分支转发 `OnInitialize`、`OnStmtStart`、`OnStmtRetry`、错误处理、读/写 ts、两类快照、激活、预热和计划优化。
- 未在本文件定制的提交、回滚、临时表与提交选项逻辑由 `BaseTxnContextProvider`/`BasePessimisticTxnContextProvider` 承担。

主要下游依赖均由 crate 内 `base.rs` 通过 `crate::{...}` 暴露：`IsolationRuntime` 提供 session、Oracle Future 与 snapshot 写入；`BaseTxnContextProvider` 负责事务激活、Prepare、快照与通用错误路径；`BasePessimisticTxnContextProvider` 负责公平锁取消/重试；`StatementInspection`、`PlanInspection` 将 SQL AST/物理计划压缩为本模块需要的只读性、计划种类和子节点接口。

`Cargo.toml` 将本 crate 映射到 Go 包 `pkg/sessiontxn/isolation`，并声明 sessiontxn、sessionctx、kv、planner-core、store-driver-txn、infoschema 等本地依赖；本文件本身通过抽象运行时和检查 trait 隔离了这些具体 crate。RustCodeGraph 的文件关系显示 `readcommitted.rs` 被 `lib.rs` 和 `isolation_aster_unit_test.rs` 使用；独立模块测试由 `lib.rs` 的 `#[cfg(test)] mod readcommitted_test` 接入。

## 错误处理与边界

所有运行时/时间戳错误使用 `TxnError` 返回：基类初始化、激活、Prepare、Oracle Future 等错误以 `?` 原样传播；只有错误决策接口把错误编码为 `StmtErrorAction`。

- `AfterQuery`：仅 `TxnErrorKind::WriteConflict` 且本语句 `stmt_rc_check_ts` 为真时返回 `RetryReady`；其他错误返回 `NoIdea`，留给上层决定。
- `AfterPessimisticLock`：可重试死锁先取消公平锁；写冲突先比较已等待时间和超时阈值。达到阈值时转换为 `LockWaitTimeout`。可重试分支还必须成功执行 `RetryFairLockingIfNeeded`，否则返回该准备错误；非重试错误原样包装为 `Error`。
- 其他错误切入点交给基类。不过当前 Rust `StmtErrorHandlePoint` 的生产枚举只有查询后和悲观加锁后两类，因此源码中的 `_` 是面向未来扩展的回退。
- `PlanSkipGetTSOFromPD` 对未知计划、空物理子树、BatchPointGet、Projection、Index/TableReader 和 `Other` 保守返回 false；Update/Delete 缺少子计划、Execute 缺少内层计划也返回 false。
- snapshot 模式、stale read、首语句、无有效缓存和重试中都禁止计划复用优化，以免使用不符合语义的旧 ts。

## 并发与资源生命周期

Provider 的方法通过 `&mut self` 串行修改状态，本文件不创建线程、任务、锁或通道；它假设一个会话按语句生命周期独占访问 Provider。`TimestampFuture` 被装箱并存入当前语句状态，只在首次求值时 `wait()`；求值成功后 `stmt_ts` 阻止重复等待，新语句整体替换状态时旧 Future 随之释放。

公平锁资源生命周期由 `BasePessimisticTxnContextProvider` 管理。本文件只在可重试死锁前调用 `CancelFairLockingIfNeeded`，在普通可重试锁错误前调用 `RetryFairLockingIfNeeded`，并把失败转为终止动作。快照是按值返回的 `Snapshot`；本模块只设置 ts、隔离级别与 `rc_check_ts`，不持有其底层存储资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessiontxn/isolation/readcommitted.go`。Rust 保留了 Go 的 `stmtState`、Provider 字段、构造/激活钩子、语句开始与重试、三路时间戳来源、RC-check 判定、锁错误重试、预热、计划跳过 TSO 以及两类快照语义。

表示层存在差异：Go 用 `oracle.Future`/`funcFuture` 统一表示 start-ts、常量和 Oracle；Rust 用 `StmtTimestampSource` 显式区分三种来源。Go 的 session、AST 和具体物理计划类型被 Rust 的 `IsolationRuntime`、`StatementInspection`、`PlanInspection` 抽象替代。Go 通过回调 `onTxnActiveFunc` 写缓存，Rust 由 `OnInitialize`/`ActivateTxn` 显式同步。Go 的错误返回是 `(action, error)`，Rust 将携带错误的情况表示为 `StmtErrorAction::Error(TxnError)`。

当前 Rust 不是 Go 文件所有观测副作用的逐项复刻：Go 路径还记录 TSO 等待时长、递增 metrics/failpoint 计数并输出冲突日志；这些副作用在本 Rust 文件中没有对应调用。Go 的计划辅助直接匹配具体 planner 类型，Rust `PlanKind` 还显式支持 `Execute` 解包，同时对若干不可优化种类保守拒绝。行为边界由 `readcommitted_test.rs` 覆盖 Provider 状态机；Go `readcommitted_test.go` 中依赖真实 SQL 执行、failpoint、双会话并发和 testfork scope 注入的集成情形没有在该 Rust 单元文件中复刻，测试文件顶部已明确列为其 harness 范围外。

## 扩展指南

- 新增时间戳来源时，修改 `StmtTimestampSource`、`PrepareStmtTS` 和 `GetStmtTS` 的求值分支，并在独立的 `readcommitted_test.rs` 增加来源选择、失败传播和同语句只求值一次的测试；不要把测试嵌入生产文件。
- 调整 RC-check 资格时，优先修改 `NeedSetRCCheckTSFlag`，同时验证连接 ID、显式事务、retrying、只读性四类负例，以及 `GetSnapshotWithStmtReadTS` 的标志传播。
- 扩展免 TSO 的计划形态时，修改 `PlanSkipGetTSOFromPD`，明确递归时 `in_lock_or_write_stmt` 如何传播，并为允许与拒绝的相邻计划各加测试；误放宽会让写语句读取旧快照，属于正确性风险。
- 修改重试策略时同时审查 `HandleAfterQueryError`、`HandleAfterPessimisticLockError`、`OnStmtRetry` 与基类公平锁生命周期。尤其不能在重试后重新启用旧 `latest_oracle_ts`。
- 增加 Provider 公共能力时，还需在 `lib.rs::TxnContextProvider` 和 `RegisteredTxnContextProvider` 的 RC 分派中接线，并检查上层 Provider 工厂/事务管理器是否需要暴露。
- 与 Go 对齐时，应同步审查 `readcommitted.go` 和 `readcommitted_test.go`；如果补齐日志、metrics、等待时长或 failpoint 等当前差异，应通过相应 runtime 抽象接入，避免把具体 Go 依赖硬编码进本文件。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/sessiontxn/isolation` 显示目标 Rust/Go 源、测试与模块入口均在图中。
- RustCodeGraph 源与关系：`node --file pkg/sessiontxn/isolation/readcommitted.rs` 读取完整 388 行并报告直接使用方 `pkg/sessiontxn/isolation/lib.rs`、`pkg/sessiontxn/isolation/isolation_aster_unit_test.rs`；符号查询确认构造器、Provider 与 Go 同名实现；`lib.rs` 的构造和各生命周期分派构成上游调用证据。
- 已核对生产与装配源码：`pkg/sessiontxn/isolation/readcommitted.rs`、`pkg/sessiontxn/isolation/lib.rs`。
- 已核对 crate 边界：`pkg/sessiontxn/isolation/Cargo.toml` 的 crate 名、`[lib]`、Go package 映射、dependencies/dev-dependencies。
- 已核对 Go 对照：`pkg/sessiontxn/isolation/readcommitted.go`，包括时间戳选源、错误重试、计划优化、快照标志及 Go 独有观测副作用。
- 已核对独立测试：`pkg/sessiontxn/isolation/readcommitted_test.rs` 覆盖 RC-check 复用/失效、锁错误、for-update 新 ts、初始化、`tidb_snapshot` 与锁等待超时；`pkg/sessiontxn/isolation/readcommitted_test.go` 提供原始 Provider 和 SQL 集成语义；`pkg/sessiontxn/isolation/isolation_aster_unit_test.rs` 提供注册表/计划谓词等综合证据。
- 本任务为纯文档分析，未运行 Cargo 或代码测试；交付验证使用任务指定的 11 章节结构命令，并人工复核本文能回答文件存在原因、运行路径、状态不变量、失败边界及安全扩展点。

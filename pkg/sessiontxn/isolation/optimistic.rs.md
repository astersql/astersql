# `pkg/sessiontxn/isolation/optimistic.rs`

## 文件定位

本文件实现 `astersql-sessiontxn-isolation` crate 的乐观事务差异层。crate 入口 `pkg/sessiontxn/isolation/lib.rs` 通过 `mod optimistic` 纳入本模块并公开重导出其符号；`NewRegisteredTxnContextProvider` 在 `ProviderKind::Optimistic` 分支构造 `OptimisticTxnContextProvider`，而 `RegisteredTxnContextProvider` 的 `TxnContextProvider` 实现再把初始化、时间戳、快照、激活和计划优化请求分派到这里。

它不是完整事务引擎。Oracle 时间戳准备、KV 事务激活、InfoSchema、语句生命周期和提交选项等通用工作位于 `BaseTxnContextProvider`（`pkg/sessiontxn/isolation/base.rs`）；本文件只定义乐观模式特有的可重试判定，以及自动提交点查使用 `MAX_TIMESTAMP` 的快速路径。crate 边界和 Go 来源由 `pkg/sessiontxn/isolation/Cargo.toml` 的 `[lib] path = "lib.rs"` 与 `[package.metadata.porting] go-package = "pkg/sessiontxn/isolation"` 明确记录。

## 核心职责

1. `NewOptimisticTxnContextProvider` 用 `IsolationLevel::Optimistic`、`pessimistic = false` 和调用方给出的 `causal_consistency_only` 初始化共享基类，并关闭 MaxTS 优化。
2. `OnInitialize` 和 `ActivateTxn` 在基类完成状态建立或事务激活后调用 `RefreshRetryable`，把当前会话能否自动重试写入 `SessionState::txn.could_retry`。
3. `GetStmtReadTS`、`GetStmtForUpdateTS` 维持乐观事务的单一读时间戳语义；正常路径使用基类读 TS，点查快速路径直接返回 `MAX_TIMESTAMP`。
4. `GetSnapshotWithStmtReadTS`、`GetSnapshotWithStmtForUpdateTS` 以相同时间戳创建 `IsolationLevel::Optimistic` 快照。
5. `AdviseOptimizeWithPlan` 只在尚未激活、没有快照/过期读、处于自动提交且非显式事务时识别点查计划，用常量 MaxTS future 替换尚未消费的 Oracle future。

## 主要符号

- `pub struct OptimisticTxnContextProvider`：持有公开字段 `base: BaseTxnContextProvider` 和 `optimize_with_max_ts: bool`。前者承载运行时、事务状态和时间戳 future；后者决定读 TS 是否绕过激活直接返回最大值。
- `pub fn NewOptimisticTxnContextProvider(runtime, causal_consistency_only)`：模块的构造入口，返回具体类型而非 trait object；统一注册表随后把它装入 `RegisteredTxnContextProvider::Optimistic`。
- `OnInitialize(context, enter) -> Result<(), TxnError>`：重置优化开关，调用 `base.OnInitialize`，成功后刷新重试能力。基类可能按 `EnterNewTxnType` 提交旧事务、准备 Oracle TS、写入事务上下文，并对 `Default`/`WithBeginStmt` 立即激活。
- `RefreshRetryable()`：调用 `IsOptimisticTxnRetryable`，再更新 `base.runtime.session_mut().txn.could_retry`。
- `ActivateTxn() -> Result<u64, TxnError>`：委托 `base.ActivateTxn`，成功后刷新重试能力并返回 start TS。
- `GetStmtReadTS()` 与 `GetStmtForUpdateTS()`：MaxTS 模式直接返回 `MAX_TIMESTAMP`；否则后者最终与前者相同。正常读路径会由基类激活事务，并优先尊重非零 `snapshot_ts`。
- `GetSnapshotWithStmtReadTS()` 与 `GetSnapshotWithStmtForUpdateTS()`：取得上述 TS 后调用 `base.GetSnapshotByTS(timestamp, IsolationLevel::Optimistic)`；for-update 快照完全复用读快照入口。
- `AdviseOptimizeWithPlan(plan)`：MaxTS 优化的状态转换入口。
- 私有 `IsAutoCommitPointGet(plan)`：识别裸点查，以及一层 `Projection`、`Execute`、`Execute -> Projection` 包装。
- 私有 `IsPointGetCandidate(plan)`：接受唯一索引点读、主键点读，或 `no_second_read && !cache_table` 的 `PointGet`；其他 `PlanKind` 返回 `false`。
- `pub fn IsOptimisticTxnRetryable(session, enter)`：纯判定函数，集中表达乐观事务自动重试的禁用项和允许项。

本文件没有 trait、模块级可变静态量或条件编译项；条件编译测试模块声明位于 `pkg/sessiontxn/isolation/lib.rs`，测试实现独立存放在 `optimistic_test.rs`。

## 执行流程

典型链路如下：

1. 上层事务管理器选择乐观模式，`NewRegisteredTxnContextProvider` 调用 `NewOptimisticTxnContextProvider`。
2. `RegisteredTxnContextProvider::OnInitialize` 分派至本文件的 `OnInitialize`。该方法先清除上一个生命周期可能留下的 `optimize_with_max_ts`，再让基类建立事务上下文，最后计算 `could_retry`。
3. 对语句前惰性进入（`BeforeStmt`），基类初始化时不一定激活 KV 事务。规划完成后，上层可通过统一 trait 调用 `AdviseOptimizeWithPlan`。
4. 建议入口依次排除：已经优化、启用 `snapshot_ts`、BEGIN stale read、事务已经激活、关闭 autocommit、已经处于显式事务。全部通过后才检查计划树。
5. 若计划是点查候选，`ForcePrepareConstStartTS(MAX_TIMESTAMP)` 用 `ConstantFuture` 替换已准备但未消费的 future；调用成功后才把 `optimize_with_max_ts` 设为 `true`。因此预热过 Oracle future 仍可安全切换，而激活后的事务不能改写 start TS。
6. 执行器请求读 TS 或 for-update TS 时，优化路径直接得到 `u64::MAX`，不触发 `base.ActivateTxn`；普通路径经基类准备/等待 TS、激活事务，并在同一事务内稳定返回 start TS。请求快照时，再由基类运行时边界创建对应 TS 的乐观快照。

可重试判定独立于 MaxTS：`Default` 进入、流水线事务、`retry_limit == 0` 或设置了 `snapshot_ts` 时必为 `false`；否则，自动提交事务、受限/内部 SQL、或未禁用事务自动重试三者之一成立即为 `true`。初始化和真正激活后都会重算，以吸收激活时才确定的流水线等会话状态。

## 数据与状态

- `optimize_with_max_ts` 是本类型唯一新增状态。它在构造和每次 `OnInitialize` 时为 `false`，仅在常量 TS 准备成功后变为 `true`，本文件没有在同一事务内关闭它的路径。
- `base.txn_active` 是 MaxTS 建议的硬边界。基类 `ForcePrepareConstStartTS` 也会对已激活事务返回 `InvalidTransaction`，因此调用前检查与下游校验形成双重保护。
- `base.const_start_ts` 和 `base.prepared_future` 保存 MaxTS 选择。即便此前 `AdviseWarmup` 已准备 Oracle future，常量 future 仍会替换它。
- `SessionState::txn.could_retry` 是本文件写入的共享会话状态；`RefreshRetryable` 不缓存判定输入，每次从运行时读取 `pipelined`、`retry_limit`、`snapshot_ts`、`in_txn`、`restricted_sql` 和 `disable_txn_auto_retry`。
- `SessionState::txn.start_ts`、`for_update_ts`、InfoSchema 和 KV 激活状态由基类管理。普通乐观事务中 read TS 与 for-update TS 相等；设置 `snapshot_ts` 时基类读路径返回快照 TS，且 `GetTxnInfoSchema` 优先返回 `snapshot_info_schema`。
- `MAX_TIMESTAMP` 定义于 `base.rs`，值为 `u64::MAX`。在该优化中它表达“读取最新已提交版本”的点查快照，而不是 Oracle 分配的常规事务时间戳。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 与 `lib.rs` 共同确认：

- `NewRegisteredTxnContextProvider` 是生产构造入口，调用 `NewOptimisticTxnContextProvider`。
- `RegisteredTxnContextProvider` 的 trait 实现分别调用本类型的 `OnInitialize`、`GetStmtReadTS`、`GetStmtForUpdateTS`、两个快照方法、`ActivateTxn` 和 `AdviseOptimizeWithPlan`。
- 更上层的事务管理器通过 `TxnContextProvider` 接口调用这些统一入口；RustCodeGraph 将 `EnterNewTxn`/`OnInitialize`、事务管理器读 TS 与激活路径识别为上游链路。

下游依赖全部经同 crate 的公开抽象进入：`BaseTxnContextProvider` 提供生命周期、时间戳和快照操作；`IsolationRuntime` 隔离真实会话、Oracle 与 KV 存储；`PlanInspection`/`PlanKind` 提供最小化的计划树视图；`SessionState` 与 `TxnContextState` 保存重试和事务状态；`TxnError` 负责错误传播。虽然 `Cargo.toml` 声明了 planner、sessionctx、kv、sessiontxn 等多个工作区依赖，本文件本身不直接导入这些外部 crate，而是依赖 `lib.rs`/`base.rs` 暴露的 crate 内抽象。

测试调用者包括 `pkg/sessiontxn/isolation/optimistic_test.rs`、`main_test.rs` 和 `isolation_aster_unit_test.rs`。其中前者直接构造真实 provider，后两者提供共享运行时夹具或注册表层覆盖。

## 错误处理与边界

所有可能失败的基类操作都使用 `?` 原样传播 `TxnError`：初始化、激活、普通读 TS、快照构造和常量 TS 准备均不在本文件吞错或改写错误。`AdviseOptimizeWithPlan` 只有在 `ForcePrepareConstStartTS` 成功后才置位优化标志，失败时不会留下“标志已开但 future 未替换”的半状态。

计划不符合候选形状不是错误，而是返回 `Ok(())` 并保持普通事务路径。包装识别有意只查看第一个 child；缺少 child、超过已编码的包装层级或其他 `PlanKind` 都安全退化。`PointGet` 候选要求无二次读且不是缓存表；该匹配当前没有读取 `lock` 字段，因此新增锁语义时必须重新核对 MaxTS 是否仍安全。

快照读和 stale read 明确禁用 MaxTS 建议，已激活事务、显式事务和非自动提交会话也不会切换 TS。普通基类路径仍可能因 Oracle、KV 激活、快照创建、取消上下文，或 start TS 早于 `last_commit_ts` 而失败。

错误处置并非本文件覆盖点。统一注册表对乐观分支把上下文写入 `provider.base.context` 后调用 `base.OnStmtErrorForNextAction`：`AfterPessimisticLock` 返回原错误，其余返回 `NoIdea`。`optimistic_test.rs` 验证这些路径不会推进乐观事务 TS。

## 并发与资源生命周期

provider 的可变 API 以 `&mut self` 暴露，状态转换在单一可变所有权下串行发生；本文件没有锁、线程、异步任务或通道，也没有自行声明 `Send`/`Sync`。真实并发边界隐藏在 `Box<dyn IsolationRuntime>` 后，Oracle future 和 KV 事务资源由基类及运行时负责。

生命周期顺序是不变量：构造 -> 初始化 -> 可选预热/计划建议 -> 首次普通读时激活或 MaxTS 直接读 -> 多语句复用。`ForcePrepareConstStartTS` 只能发生在激活前；基类激活后设置 `txn_active` 并稳定保存 start TS，后续调用幂等返回同一值。MaxTS 快速路径刻意不激活事务，因此读取时间戳本身不会消费 Oracle future 或创建活跃 KV 事务；这一点对自动提交点查的延迟和资源占用至关重要。

`RefreshRetryable` 必须在基类初始化/激活成功之后执行，因为它读取并写回运行时会话状态。若基类返回错误，重试标志不会按失败中的中间状态刷新。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessiontxn/isolation/optimistic.go`，回归语义由 `optimistic_test.go` 与 Rust 的 `optimistic_test.rs` 共同说明。

- Rust 的构造函数加 `OnInitialize` 对应 Go 的 `ResetForNewTxn` 加基类初始化；Rust 使用拥有运行时的 `BaseTxnContextProvider`，而 Go provider 保存 `sessionctx.Context` 并安装函数指针。
- Rust 的 `RefreshRetryable`/`IsOptimisticTxnRetryable` 对应 Go 的 `onTxnActive`/`isOptimisticTxnRetryable`。禁用与允许条件一致，但 Go 还提供 `injectOptimisticTxnRetryable` failpoint，Rust 当前没有对应注入点。
- 两版都让普通 read TS 与 for-update TS 落到事务 start TS，并在 MaxTS 优化时直接返回最大值。Rust 将两个快照方法显式放在此类型中；Go 主要复用嵌入基类的快照行为。
- Go 的 `AdviseOptimizeWithPlan` 接受 `any`、尝试转换真实 planner `base.Plan`，展开 `Execute` 后调用 `IsPointGetWithPKOrUniqueKeyByAutoCommit`；Rust 改为 `PlanInspection`/`PlanKind` 的显式形状匹配，并在本文件检查 `autocommit`/`in_txn`。这保留了已测试的点查主路径，但对所有真实 planner 包装形状的完全等价性没有由现有单元测试穷尽证明。
- Go 在启用 MaxTS 时记录连接/SQL 日志，并在语句优先级未设置时提升为 `kv.PriorityHigh`；Rust 当前只准备常量 TS 和置位标志，没有这两个副作用。文档将其视为当前迁移差异，而不是宣称已支持。
- Rust 测试对应 Go 的三个核心用例：事务内 TS 稳定及自动提交点查 MaxTS、错误后的 TS 不推进、`tidb_snapshot` 优先于实时事务。Go 测试还通过真实 parser/planner/testkit 覆盖整合路径；Rust 测试使用 `MockRuntime` 与 `TestPlan`，因此真实 planner 适配仍需更高层验证。

## 扩展指南

- 新增可优化的计划形状时，优先修改 `IsAutoCommitPointGet`/`IsPointGetCandidate`，并在独立的 `optimistic_test.rs` 增加裸计划与包装计划的正反例；不要把测试内嵌回生产文件。尤其应覆盖锁点查、缓存表、需要二次读、空 child、多层 Projection/Execute、唯一索引和主键 reader。
- 修改重试规则时，只在 `IsOptimisticTxnRetryable` 集中调整，并验证初始化后与激活后两次刷新。必须同步核对 Go 的 `isOptimisticTxnRetryable`，覆盖 Default、流水线、零重试、snapshot、自动提交、restricted SQL 和禁用自动重试的组合。
- 修改 MaxTS 状态机时保持“先成功替换 future，再置位标志”和“激活后不可替换”两个不变量；同时验证预热后的 future 替换、普通事务不激活回归，以及快照/stale read 绕过。
- 若补齐 Go 的日志、优先级或 failpoint 行为，应通过运行时抽象新增最小能力，避免在此文件直接耦合具体 session/planner 实现；并评估日志开销、优先级兼容性和测试可控性。
- 若接入真实 planner 类型，应保留 `PlanInspection` 边界或提供明确适配器，并用集成测试证明 Go helper 与 Rust 形状判断一致。错误地放宽候选会让非安全查询使用“最新版本”语义，属于正确性风险；错误地收紧只会损失点查性能。
- 任意生产改动都应保留文件顶部 PingCAP Apache License 与 AsterSQL 处理标记，并先运行 `cargo fmt --all`；本说明任务本身不修改 Rust，也按计划不运行 Cargo。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录与 `optimistic.rs` 均已索引。
- RustCodeGraph `files --filter pkg/sessiontxn/isolation`：确认模块的 Rust/Go 源、独立测试和 `lib.rs` 边界。
- RustCodeGraph `explore "pkg/sessiontxn/isolation/optimistic.rs ..."`：确认构造入口、`RefreshRetryable`、读 TS、快照和点查判定的内部调用，以及 `NewOptimisticTxnContextProvider` 的注册表与测试调用者。
- RustCodeGraph `node --file`：完整读取 `optimistic.rs` 1-171 行、`lib.rs` 的注册与 trait 分派、`base.rs` 的状态类型、初始化、TS future、激活、读 TS、错误策略和快照构造。
- RustCodeGraph `query`：核对同名 Go/Rust provider、`OnInitialize`、`ActivateTxn`、`GetStmtReadTS`、`ForcePrepareConstStartTS` 与快照函数的定义位置；图中部分 Rust inherent method 未被精确名称查询单列，因此以完整文件节点和 `lib.rs` 分派源码为准。
- 直接读取 `pkg/sessiontxn/isolation/Cargo.toml`、`optimistic.go`、`optimistic_test.go` 和 `optimistic_test.rs`，分别核对 crate/移植边界、Go 语义、Go 集成测试和 Rust 独立回归测试。

人工事实复核结论：该文件存在是为了在共享事务基类之上实现乐观事务的重试资格与 MaxTS 点查优化；运行主线、状态不变量、失败传播、安全退化条件和扩展测试位置均可由上述符号与测试反查。本文不声称已执行 Cargo 测试；任务明确是纯文档分析，验证范围为源码/调用图事实与文档结构。

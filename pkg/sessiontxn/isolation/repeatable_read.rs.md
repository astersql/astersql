# `pkg/sessiontxn/isolation/repeatable_read.rs`

## 文件定位

本文件属于 `astersql-sessiontxn-isolation` crate；crate 根为 `pkg/sessiontxn/isolation/lib.rs`，其中以私有模块 `repeatable_read` 装入本文件，再通过 `pub use repeatable_read::*` 导出公开符号。`pkg/sessiontxn/isolation/Cargo.toml` 的 `package.metadata.porting.go-package` 指向同路径 Go 包，说明这是 `pkg/sessiontxn/isolation/repeatable_read.go` 的 Rust 移植面。

它实现悲观事务在 Repeatable Read（RR）隔离级别下的事务上下文策略：普通读沿用事务 start ts，加锁读维护可前进的 `for_update_ts`，并在特定执行计划上省去一次向时间戳服务取最新值的往返。它不直接执行 SQL、加锁或提交事务；这些能力由 `IsolationRuntime` 和两层基类提供，本文件负责决定何时调用它们以及如何维护 RR 特有状态。

应用侧通过 `NewRegisteredTxnContextProvider(ProviderKind::PessimisticRepeatableRead, ...)` 创建本 Provider，随后由 `RegisteredTxnContextProvider` 的 `TxnContextProvider` 实现分发初始化、语句生命周期、时间戳、快照、错误处理和计划建议。RustCodeGraph 显示目标文件由 `pkg/sessiontxn/isolation/lib.rs` 装配，并被 `repeatable_read_test.rs`、`isolation_aster_unit_test.rs` 引用。

## 核心职责

- `PessimisticRRTxnContextProvider` 组合 `BasePessimisticTxnContextProvider`，把 RR、悲观事务和因果一致性选项交给公共初始化/激活逻辑。
- `GetStmtReadTS` 委托基类，维持 RR 普通读对事务 start ts 的固定视图；`GetStmtForUpdateTS`/`GetForUpdateTS` 为加锁读选择快照变量、语句缓存、会话已有值或新 Oracle 时间戳。
- `OnStmtStart` 和 `OnStmtRetry` 管理语句级缓存：新语句清空缓存，内部重试则在确有更新时继承 `latest_for_update_ts`。
- `OnStmtErrorForNextAction` 仅处理悲观锁之后的错误，区分不可重试死锁、可重试死锁、写冲突超时和其它错误，并协调时间戳刷新与 fair-locking 状态。
- `AdviseOptimizeWithPlan` 与递归谓词 `NotNeedGetLatestTSFromPD` 根据计划形状决定加锁读能否复用会话中的 `for_update_ts`；该建议不得改变查询结果。
- 两个 `GetSnapshotWithStmt*TS` 方法把选定时间戳交给基类构造 RR 快照。

## 主要符号

- `pub struct PessimisticRRTxnContextProvider`：RR 悲观事务状态持有者。`base` 提供公共事务与悲观锁生命周期；`for_update_ts` 是当前语句缓存，`0` 表示未解析；`latest_for_update_ts` 记录错误处理期间最新刷新的值；`optimize_for_not_fetching_latest_ts` 是当前语句的计划优化建议。
- `NewPessimisticRRTxnContextProvider(runtime, causal_consistency_only)`：以 `IsolationLevel::RepeatableRead`、`pessimistic = true` 构造两层基类，并把三个 RR 状态字段初始化为零/假。返回具体类型，注册层再把它包装为 `RegisteredTxnContextProvider::PessimisticRepeatableRead`。
- `OnInitialize`、`GetStmtReadTS`：无 RR 特有分支，分别转发到 `BaseTxnContextProvider::OnInitialize` 和 `GetStmtReadTS`。
- `GetStmtForUpdateTS`：若 `SessionState.snapshot_ts != 0`，先激活事务并直接返回该历史快照 ts；否则进入 `GetForUpdateTS`。
- `GetForUpdateTS`：按“当前语句缓存 → 激活事务 → 计划允许复用会话 `txn.for_update_ts` → `oracle_future(...).wait()` 取新值”的次序解析时间戳。新值同时写回会话事务上下文、底层事务 snapshot ts 和当前语句缓存。
- `UpdateForUpdateTS`：要求事务已激活，通过 `latest_timestamp` 取得新值，写回会话事务上下文和底层 snapshot ts，并更新 `latest_for_update_ts`；供锁错误处理使用。
- `OnStmtStart`：先调用基类钩子，再清零 `for_update_ts` 并关闭优化标志。`OnStmtRetry` 先调用基类钩子；仅当 `latest_for_update_ts > for_update_ts` 时继承该新值，否则把当前缓存清零；随后关闭优化标志。
- `OnStmtErrorForNextAction`/私有 `HandleAfterPessimisticLockError`：把最新 `RuntimeContext` 写入基类，仅在 `AfterPessimisticLock` 切入点执行 RR 错误策略。
- `AdviseOptimizeWithPlan`：历史快照或 `BEGIN` 语句 stale read 生效时不设置优化；否则把递归谓词结果写入当前语句标志。
- `GetSnapshotWithStmtReadTS`、`GetSnapshotWithStmtForUpdateTS`：分别解析普通读/加锁读 ts，再调用 `GetSnapshotByTS(timestamp, RepeatableRead)`。
- `NotNeedGetLatestTSFromPD(plan, in_lock_or_write_stmt)`：公开的纯递归计划谓词。第二参数表示祖先是否已经位于 Update/Delete/PhysicalLock 的加锁或写语境。

本文件没有模块级常量、trait、条件编译项或异步函数；唯一私有行为函数是 `HandleAfterPessimisticLockError`。

## 执行流程

1. 注册层收到 `ProviderKind::PessimisticRepeatableRead` 后调用构造函数；`OnInitialize` 由基类建立 RR 悲观事务上下文。是否立即激活取决于 `EnterNewTxnType` 和会话状态，详见基类，而非本文件重新实现。
2. 每条语句开始时，注册层分发到 `OnStmtStart`。基类先更新上下文/语句状态，本文件再清除上一语句的 `for_update_ts` 缓存和计划优化建议。
3. 规划完成后可调用 `AdviseOptimizeWithPlan`。若不是 snapshot/stale-read 场景，递归检查计划：
   - `PointGet`/`BatchPointGet` 在普通语境可优化；位于写或锁祖先下时必须自身 `lock = true`。
   - `Physical` 节点必须有子节点，且所有子树都可优化；其 `lock` 会传播为子树的锁语境。
   - `Update`/`Delete` 只检查第一个子计划并强制写语境；无子节点即失败。
   - 无 `SELECT` 的 `Insert` 可优化；`Execute` 透明检查第一个子计划。
   - `Projection`、reader 节点和未知 `Other` 单独出现时均返回假。
4. 普通读调用 `GetStmtReadTS`，由基类返回 snapshot ts（若配置）或 RR start ts。加锁读调用 `GetStmtForUpdateTS`：snapshot ts 优先；否则 `GetForUpdateTS` 先用语句缓存。缓存为空时激活事务，若计划优化为真便复用会话事务已有的 `for_update_ts`，否则等待 Oracle future，随后同步三个存储位置。
5. 需要快照时，两个快照方法以相应 ts 调用基类 `GetSnapshotByTS`，隔离级别显式标为 `RepeatableRead`。
6. 悲观锁之后发生错误时：不可重试死锁直接返回原错误；可重试死锁先取消 fair locking；写冲突若累计等待已达到超时阈值则转换为 `LockWaitTimeout`。可重试死锁和未超时写冲突都刷新 ts、恢复 fair locking，并返回 `RetryReady`。其它错误尽力刷新 ts，但无论刷新是否成功都返回原错误。
7. 框架触发内部语句重试时，`OnStmtRetry` 把错误处理得到的更大 `latest_for_update_ts` 带入当前缓存；因此下一次加锁读无需再次取 ts。普通 `OnStmtStart` 不继承它，下一条语句按正常路径重新决策。

## 数据与状态

`for_update_ts` 是语句级惰性缓存：零是内部哨兵，不是有效的已解析时间戳。它可能来自历史快照、会话事务已有值或 Oracle；历史快照分支直接返回而不写该字段。`latest_for_update_ts` 是跨一次错误处理与随后的内部重试传递的桥梁，正常新语句不会直接复用它。`optimize_for_not_fetching_latest_ts` 只是一条语句的建议，`OnStmtStart`/`OnStmtRetry` 都会重置，防止计划结论泄漏到另一条语句。

共享状态位于 `IsolationRuntime::session[_mut]()` 返回的 `SessionState`：`txn.start_ts` 支撑 RR 普通读，`txn.for_update_ts` 是跨语句最近加锁读 ts，`txn.txn_scope` 选择时间戳作用域，`snapshot_ts` 覆盖实时读，`use_low_resolution_tso` 影响 Oracle future，锁等待已耗时和超时阈值决定写冲突能否重试。

时间戳同步不变量是：新取或刷新 `for_update_ts` 后，同时更新会话 `txn.for_update_ts` 与运行时事务 snapshot ts；获取路径还写当前语句 `for_update_ts`，刷新路径先写 `latest_for_update_ts`，待 `OnStmtRetry` 再转入当前缓存。测试 `rr_error_handle_refreshes_for_update_ts_and_retries_only_for_lock_conflicts` 和 `repeatable_read_refreshes_ts_and_preserves_it_for_retry` 均验证了这条传递链。

## 依赖与调用关系

上游静态装配链为 `pkg/sessiontxn/isolation/lib.rs`：`NewRegisteredTxnContextProvider` 构造本类型；`TxnContextProvider for RegisteredTxnContextProvider` 对 RR 变体显式转发 `OnInitialize`、`OnStmtStart`、`OnStmtRetry`、`OnStmtErrorForNextAction`、两类 ts、两类 snapshot 与计划建议。再上游的会话事务管理器面向 `TxnContextProvider` 接口工作；动态分发使 RustCodeGraph 未把所有具体方法列为直接 callers，不能据“无 callers”推断未接线。

下游依赖均从 crate 根再导入：`BaseTxnContextProvider` 负责初始化、激活、普通读 ts、快照和 snapshot/stale-read 判定；`BasePessimisticTxnContextProvider` 负责 fair-locking 取消/重试；`IsolationRuntime` 隔离会话状态、Oracle、底层事务选项与快照实现；`PlanInspection`/`PlanKind` 把真实 planner 计划压缩成该策略需要的只读视图；`TxnError*` 与 `StmtError*` 表示边界错误和后续动作。

RustCodeGraph 明确给出的本文件内部调用边包括 `OnStmtErrorForNextAction -> HandleAfterPessimisticLockError`、`AdviseOptimizeWithPlan -> NotNeedGetLatestTSFromPD`；注册层源码明确给出对本 Provider 方法的分发。图未识别 `GetForUpdateTS`/`UpdateForUpdateTS` 内部的 trait-object 和链式 runtime 调用，相关关系由目标源码符号直接验证。

`Cargo.toml` 没有 feature 条件；本文件自身只使用 crate 内抽象，真实 planner、KV、sessionctx、store driver 等跨 crate 依赖由 `astersql-sessiontxn-isolation` 的依赖表和 `lib.rs`/`base.rs` 适配层承接。

## 错误处理与边界

所有时间戳、激活和快照操作以 `Result<_, TxnError>` 向上传播。`GetForUpdateTS` 不吞掉激活、创建 future、等待 future或写底层 snapshot ts 的失败；这保证只有全部同步成功才把新时间戳作为结果返回。`UpdateForUpdateTS` 在 `txn_active == false` 时返回 `InvalidTransaction`，避免在无有效事务时更新锁读视图。

错误策略的重要边界如下：非 `AfterPessimisticLock` 切入点一律 `NoIdea`；不可重试死锁不刷新 ts；可重试死锁若取消 fair locking 失败，返回取消错误；写冲突等待时间达到（含等于）超时阈值时返回新的 `LockWaitTimeout`；其它错误仅 best-effort 刷新 ts，仍保留并返回原错误。对于可重试冲突，刷新失败时返回原锁错误，fair-locking 重试失败时返回该重试错误。

计划谓词采取保守失败：空 Physical、无选择子计划的 Update/Delete/Execute、Projection、reader 和未知计划均不优化。尤其写/锁祖先下未标锁的点查必须返回假，这是 Go 注释关联 Issue 35524 的结果一致性保护；扩展计划种类时默认应保持“证据不足则取最新 TSO”。

历史 snapshot/stale-read 会阻止计划优化。`GetStmtForUpdateTS` 在 snapshot 模式直接返回 snapshot ts；这与独立测试 `rr_prefers_tidb_snapshot_vars_over_the_live_txn` 的读和加锁读一致性断言相符。

## 并发与资源生命周期

本类型持有可变 Provider 与 runtime，不包含 `Arc`、锁、通道、后台任务或显式线程安全实现；调用方式是会话事务管理器在单个会话生命周期内以 `&mut self` 串行推进状态。这里的“并发”主要指与其它事务发生锁冲突，以及分布式时间戳/底层事务状态的协调，而不是本对象内部并行。

Oracle future 的生命周期局限于一次 `GetForUpdateTS`：构造后立即 `wait()`，成功值写回状态，错误直接传播。事务激活早于取/复用加锁 ts，确保底层事务可接收 snapshot ts。fair-locking 生命周期由悲观基类管理：可重试单语句死锁先 cancel，刷新 ts 后 retry；写冲突直接刷新后 retry。测试运行时记录的事件顺序 `activate -> fair-start -> snapshot-ts -> fair-retry` 验证了资源状态转换顺序。

Provider 状态跨语句存在，但语句缓存和优化位由生命周期钩子复位；`latest_for_update_ts` 只在值确实比当前缓存更新时用于内部重试。修改这些复位规则会带来陈旧快照、额外 TSO 请求或错误复用计划结论的风险。

## 与 Go 版本的对应关系

Rust 的结构与 `pkg/sessiontxn/isolation/repeatable_read.go` 主体一一对应：结构体三个状态字段、构造函数、`getForUpdateTs`、`updateForUpdateTS`、两个语句钩子、锁后错误处理、计划优化谓词和 snapshot 方法语义相同。Rust 用 `IsolationRuntime`、`SessionState`、`PlanInspection`/`PlanKind` 和结构化 `TxnErrorKind` 替代 Go 的 `sessionctx.Context`、真实 `base.Plan` 类型断言及 `error` cause 判断。

可见差异包括：Go 的构造函数通过函数指针把普通读/加锁读 ts 接入基类，Rust 注册枚举直接分发具体方法；Go `AdviseOptimizeWithPlan(any)` 先判断类型并解包 `*Execute`，Rust 接受已经适配成 `&dyn PlanInspection` 的计划并把 `Execute` 表示为带首个 child 的 `PlanKind`；Go 记录取 TSO 耗时、failpoint 计数和日志，Rust 当前抽象未在本文件暴露这些观测副作用。Rust 的其它错误分支以 `let _ = UpdateForUpdateTS()` 明确忽略刷新错误，保留 Go “记录失败但返回原业务错误”的控制流结果，但没有本地日志。

计划谓词保留 Go 的关键规则：写/锁祖先传播、点查自身 lock 校验、Physical 的全子树合取、Update/Delete 的选择子树、无 Select 的 Insert。Rust 额外显式建模 `Execute`，而 Go 在调用谓词前解包 Execute；最终效果相当。Go 中 `PhysicalPlan` 的具体 PhysicalLock 类型判断在 Rust 中由 `PlanKind::Physical { lock }` 适配结果承载。

Rust 独立测试 `pkg/sessiontxn/isolation/repeatable_read_test.rs` 对照 Go 的 `repeatable_read_test.go` 中 `TestPessimisticRRErrorHandle`、`TestRepeatableReadProviderTS`、`TestRepeatableReadProviderInitialize`、`TestTidbSnapshotVarInPessimisticRepeatableRead` 和 `TestOptimizeWithPlanInPessimisticRR`。测试文件头明确把真实 DML、failpoint、slow-log、analyze 等 executor 集成场景留在 Provider 单元测试范围外，因此这些未由本 crate 测试覆盖，不能据此宣称 Rust 已覆盖完整 Go 集成行为。

## 扩展指南

- 新增或修改 RR 时间戳来源时，优先改 `GetForUpdateTS`/`UpdateForUpdateTS`，同时保持会话 `txn.for_update_ts`、底层 snapshot ts、`for_update_ts`/`latest_for_update_ts` 的同步不变量；在独立的 `repeatable_read_test.rs` 增加成功与每个失败点测试，不要把测试写入生产源文件。
- 新增锁错误类别或重试策略时，修改 `HandleAfterPessimisticLockError`，明确它是否刷新 ts、是否改变 fair-locking、返回原错误还是新错误，并覆盖 inactive transaction、超时边界、cancel/retry 失败。若 Go 对照行为变化，应同步核对同名 Go 方法，避免 Rust 简化掉日志之外的控制流。
- 新增 planner 节点时，在 `PlanKind` 适配层和 `NotNeedGetLatestTSFromPD` 同步接线。默认返回假最安全；只有能证明复用旧 ts 不改变结果时才返回真。至少测试节点无子树、嵌套锁/写祖先、所有子树合取及 Execute 包装。
- 修改 snapshot/stale-read 行为时，同时复核 `AdviseOptimizeWithPlan` 的提前返回和 `GetStmtForUpdateTS` 的 snapshot 优先级，并同步 `rr_prefers_tidb_snapshot_vars_over_the_live_txn`。
- 若增加新的公开生命周期方法，必须在 `TxnContextProvider`、`RegisteredTxnContextProvider` RR 分支以及上游 session transaction manager 三处检查接线；RustCodeGraph 对 trait-object 动态调用可能不展示具体 caller，应以注册层源码和测试共同确认。
- 性能敏感点是每语句 TSO 往返；正确性敏感点是 RR start-ts 固定、写冲突后使用真正的最新 TSO、计划优化不改变结果和 fair-locking 状态可恢复。任何优化都应先保证这些约束，再比较 Oracle 请求次数。

## 验证依据

- 目标源码：`pkg/sessiontxn/isolation/repeatable_read.rs`，RustCodeGraph `node --file` 显示 269 行、16 个符号，并列出 `lib.rs` 与两个 Rust 测试引用。
- crate 与注册入口：`pkg/sessiontxn/isolation/Cargo.toml`、`pkg/sessiontxn/isolation/lib.rs`。前者确认 crate 根、Go 包映射及依赖边界；后者确认 RR 变体构造及所有 `TxnContextProvider` 分发点。
- Go 对照：`pkg/sessiontxn/isolation/repeatable_read.go`，核对了构造、时间戳缓存/刷新、语句钩子、计划递归和悲观锁错误分支。RustCodeGraph 查询还定位 `TestPessimisticRRErrorHandle`、`TestRepeatableReadProviderTS`、`TestOptimizeWithPlanInPessimisticRR` 于 `repeatable_read_test.go`。
- Rust 独立测试：`pkg/sessiontxn/isolation/repeatable_read_test.rs` 覆盖锁错误分类、等待超时、start-ts 固定、初始化、snapshot 覆盖和计划优化；`pkg/sessiontxn/isolation/isolation_aster_unit_test.rs` 的 `repeatable_read_refreshes_ts_and_preserves_it_for_retry` 覆盖注册环境下的写冲突、snapshot-ts 与 fair-locking 事件顺序，`plan_inspection_keeps_rr_and_rc_rules_distinct` 覆盖 RR/RC 计划规则差异。
- RustCodeGraph 查询：运行了 `status`、目标 `files`/`node`、主要符号 `query`，以及构造函数/计划谓词的 `callers` 和 `GetForUpdateTS`、`UpdateForUpdateTS`、`OnStmtErrorForNextAction`、`AdviseOptimizeWithPlan` 的 `callees`。索引状态为 11,467 文件、307,296 节点；动态 runtime 调用未形成完整边，因此相关结论以目标源码和注册层源码补证。
- 本任务是纯文档分析，未运行 Cargo。交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工检查没有把 executor 集成测试范围外行为写成已验证事实。

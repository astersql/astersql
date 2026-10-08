# `pkg/sessionctx/stmtctx/stmtctx.rs`

## 文件定位

`stmtctx.rs` 是 `astersql-sessionctx-stmtctx` crate 的核心实现文件，定义单条 SQL 语句从解析/规划、下推执行到结果汇总期间共享的 `StatementContext`。crate 入口 `pkg/sessionctx/stmtctx/lib.rs` 通过 `mod stmtctx; pub use stmtctx::*;` 对外导出本文件 API；`pkg/sessionctx/stmtctx/Cargo.toml` 的 `package.metadata.porting.go-package` 明确指向 Go 包 `pkg/sessionctx/stmtctx`。

它不是 SQL 执行入口，而是跨层状态载体：会话变量在 `pkg/sessionctx/variable/session.rs` 中持有 `StatementContext` 并用 `NewStmtCtx` 初始化；规划运行时在 `pkg/session/runtime/planning.rs` 读取其类型上下文；`pkg/util/stmtsummary/v2/record.rs` 也构造上下文以处理 statement summary 数据。RustCodeGraph 对本文件的文件级引用还列出了 `pkg/sessionctx/variable/sysvar_builtins.rs` 等消费者。

## 核心职责

1. **统一语句语义。** `typeCtx`、`errCtx`、`TypeWarnBridge` 和 `ErrWarnBridge` 把类型转换标志、错误组级别和 SQL warning 汇合到同一上下文；`PushDownFlags`/`InitFromPBFlagAndTz` 则把这些语义编码到或还原自下推请求 flags。
2. **保存语句执行结果。** `affectedRows` 与 `stmtCtxMu` 维护 affected/found/record/deleted/updated/copied/touched 行数和结果消息，`WarnHandler`、`ExtraWarnHandler` 分别维护客户端可见告警与额外诊断。
3. **承载规划与观测状态。** 文件保存 SQL/计划 digest、编码/二进制/扁平计划、逻辑计划构建快照、计划缓存判定、运行统计、使用过的统计信息、MPP 标识、资源组标签和慢日志输出数据。
4. **管理语句生命周期。** `Reset` 用于进入下一条语句，`ResetForRetry` 用于同一语句的新一次执行尝试；两者清理范围不同。语句缓存、stale-read TSO、DistSQL/Ranger/BuildPB 上下文采用各自的惰性缓存规则。
5. **提供并发安全的共享状态。** 原子量、`Mutex`、`RwLock`、`OnceLock` 和 `Arc` 允许规划器、执行器及统计/观测路径共享局部状态，同时保留 Go 版本的锁域和原子语义。

## 主要符号

### 顶层类型与全局设施

- `CacheValue = Arc<dyn Any + Send + Sync>`：迁移期的跨 crate 类型擦除容器；`cache_value` 装箱，`cache_downcast_ref` 只读向下转型。
- `ResourceGroupTagger`、`KEYSPACE_NAME`、`SetResourceGroupTaggerKeyspaceName`：组合 keyspace、SQL digest 与计划 digest。全局 keyspace 由 `RwLock<Vec<u8>>` 保护。
- `AllocateTaskID`：用进程级 `AtomicU64` 分配从 1 开始的语句执行任务 ID，使用 Relaxed 顺序，仅保证唯一递增值而不承担跨字段同步。
- `ReferenceCount`：用 CAS 实现增加、减少、从零冻结及解冻；`ReferenceCountIsFrozen = -1` 是禁止新增引用的哨兵。
- `ReservedRowIDAlloc`：管理 `(base, max]` 半开区间；`Consume` 先递增再返回，耗尽时返回 `(0, false)`。
- `StatementHints`：原子保存 `ForceNthPlan`（默认 `-1`）和 `WriteSlowLog`。
- `LogicalPlanBuildState`：可保存/恢复的规划期快照，覆盖告警、表/统计/锁表集合、UnionScan、动态分区裁剪、视图深度、UPDATE 列引用和 `PlanCacheTracker` 状态。
- `UsedStatsInfoForTable`、`UsedStatsInfo`、`StatsLoadResult`：记录使用过的表统计、列/索引加载状态及加载错误，并输出 EXPLAIN/慢日志格式。
- `StmtLabelContext`、`WithStmtLabel`、`GetStmtLabel`：允许显式覆盖 AST `StatementKind` 的默认标签。
- `StmtCacheKey` 及 `StmtNowTsCacheKey`、`StmtSafeTSCacheKey`、`StmtExternalTSCacheKey`：语句级缓存的三种稳定键。

### `StatementContext` 的状态分组

- 类型与错误：`typeCtx`、`errCtx`、`errWarnBridge`、`WarnHandler`、`ExtraWarnHandler`。
- 语句类别与控制位：`InInsertStmt`、`InUpdateStmt`、`InDeleteStmt`、`InSelectStmt`、`InLoadDataStmt`、EXPLAIN/DDL/restricted SQL/stale-read 等标志。
- 计数与标识：`ctxID`、`TaskID`、`affectedRows`、`mu`、insert ID、重试计数和锁等待起点。
- 计划与 digest：`digestMemo`、`planDigest`、`plan`、`flatPlan`、`encodedPlan`、`binaryPlan`、`logicalPlanBuild` 和 `PlanCacheTracker`。
- 资源与统计：`MemTracker`、`DiskTracker`、`RuntimeStatsColl`、`StatsLoad`、`usedStatsInfo`、`plannerUsedStatsLoadStatus`。
- 惰性缓存：`stmtCache`、`StaleTSOProvider`、`distSQLCtxCache`、`rangerCtxCache`、`buildPBCtxCache`。

### 关键方法

- 构造与复位：`StatementContext::build`、`NewStmtCtx`、`NewStmtCtxWithTimeZone`、`Reset`、`ResetForRetry`。
- 错误策略：`SetTypeFlags`、`SetErrLevels`、`HandleTruncate`、`HandleError`、`HandleErrorWithAlias`、`PushDownFlags`、`InitFromPBFlagAndTz`。
- 规划快照：`SaveLogicalPlanBuildState`、`RestoreLogicalPlanBuildState` 及 `SetLogicalPlan*`/`LogicalPlan*` 访问器。
- 缓存与摘要：`GetOrStoreStmtCache`、`GetOrEvaluateStmtCache`、`SQLDigest`、`SetPlanDigest`、`GetResourceGroupTagger`。
- 执行汇总：`AddAffectedRows`、各类行计数访问器、warning 访问器、`GetExecDetails`、`GetResultRowsCount`。
- stale read 与统计：`SetStaleTSOProviderIfNotExist`、`GetStaleTSO`、`GetUsedStatsInfo`、`RecordUsedStatsLoadStatus`。

## 执行流程

### 创建和语义初始化

1. `NewStmtCtx` 以 UTC 调用 `NewStmtCtxWithTimeZone`，后者把全新 warning handler、锁域和缓存交给 `StatementContext::build`。
2. `build` 创建 `TypeContext`，将 `TypeWarnBridge` 指向主 warning handler；随后由 `newErrCtx` 根据类型 flags 决定截断错误是 Ignore/Warn/Error，并用 `ErrWarnBridge` 把 errctx 告警转为语句 warning。
3. 构造器生成新的 `ctxID`、初始化计划缓存 tracker 和所有原子/锁保护字段。`TaskID` 初始为 0，只有重试等路径显式分配执行 ID。

### 规划、下推与执行

1. 会话/规划器设置语句类别、时区、错误级别、表引用、统计、计划 cache 判定以及 EXPLAIN 状态。
2. 尝试候选逻辑计划时，`SaveLogicalPlanBuildState` 保存 baseline；候选失败或需要回退时，`RestoreLogicalPlanBuildState` 同时恢复规划字段、两组告警和计划缓存 tracker，避免候选路径污染后续计划。
3. 生成 TiKV/TiFlash 请求时，`PushDownFlagsWithTypeFlagsAndErrLevels` 先编码截断、溢出、零日期和除零策略，`PushDownFlags` 再叠加语句类型、Load Data、restricted SQL 与短路表达式标志。反向入口 `InitFromPBFlagAndTz` 恢复可表达的语句类型、错误策略和时区。
4. 执行期间，各执行路径通过原子或 `stmtCtxMu` 累加计数，记录 warning、执行统计、表/索引、锁等待和 tracker 使用；SQL/计划 digest 与若干上下文在首次请求时计算或构造。
5. 结束后，行计数、消息、warning、统计格式和 plan/digest 数据供协议响应、EXPLAIN、慢日志、资源组和 statement summary 消费。

### 重试与下一语句

- `ResetForRetry` 只重置当前执行尝试产生的 affected/行计数、消息、预留 RowID、表/索引列表、warning 和 DistSQL 缓存，并分配新 `TaskID`；SQL/计划等语句身份信息仍可延续。
- `Reset` 代表完整语句边界。它先对 `mu`、`stmtCache`、`StaleTSOProvider` 三个共享锁域执行 `try_lock`，任一忙碌就返回 `false`，不做部分重置。成功后重新 `build` 上下文，保留 Go 语义指定复用的 CTE storage、table stats、lock table IDs、UnionScan 映射、related table IDs 和 index collector；普通表列表、动态裁剪、视图深度、列引用及大多数语句态被清空。

## 数据与状态

- **上下文身份。** `ctxID` 每次完整构造/Reset 都更新；`TaskID` 标识执行尝试，`ResetForRetry` 更新它。二者用途不可互换。
- **双层错误策略。** `TypeContext::Flags` 决定类型转换行为，`errctx::LevelMap` 决定错误组处理；截断组级别始终由类型 flags 重算，因此 `SetErrLevels` 不能越过该约束。
- **两类 warning。** `WarnHandler` 参与客户端 warning count；`ExtraWarnHandler` 保存额外诊断。`WarningCount` 在 `InShowWarning` 时故意返回 0，防止 `SHOW WARNINGS` 自身制造递归计数。
- **多类缓存。** `stmtCache` 支持按键存取和可失败求值；成功值才写入。SQL digest 首次对 `OriginalSQL` 规范化后记忆，之后修改 `OriginalSQL` 不会自动失效，必须显式调用 `ResetSQLDigest`。stale TSO 同样只缓存成功结果，失败允许下一次重试。
- **统计格式。** `UsedStatsInfoForTable::FormatForExplain` 对非 pseudo 状态最多展开三项，其余按状态聚合；`WriteToSlowLog` 按 ID 排序完整输出。`Version == 0` 表示 pseudo 统计。
- **迁移期类型。** 多个尚未由统一 Rust trait 表达的 Go `any`/跨包对象使用 `CacheValue`；`StatsLoadState::ResultCh` 当前是 `Vec<CacheValue>`，源码注释明确它是 Go channel 语义的占位，不应在文档中描述成真实异步通道。

## 依赖与调用关系

### crate 边界

`Cargo.toml` 声明本 crate 依赖：

- `astersql-util-context`：warning handler、计划缓存 tracker、上下文 ID；
- `astersql-errctx` 与 `astersql-types`：错误级别和类型求值上下文；
- `astersql-util-execdetails`：同步执行明细与运行统计；
- `astersql-util-memory`：内存/磁盘 tracker；
- `astersql-util-intset`：规划期列引用集合；
- `astersql-meta-model`：`TableInfo`/`TableItemID` 与下推 flags；
- `astersql-parser`：SQL digest 与语句种类；
- `chrono-tz`：时区。

### 已验证调用边

- RustCodeGraph 文件图显示本文件被 `pkg/sessionctx/variable/session.rs`、`pkg/session/runtime/planning.rs`、`pkg/util/stmtsummary/v2/record.rs`、`pkg/sessionctx/variable/sysvar_builtins.rs` 等引用；其中 `SessionVars` 在 `session.rs` 持有并初始化 `StmtCtx`，statement summary 在 `record.rs` 调用 `NewStmtCtx`。
- RustCodeGraph 的 callee 图确认：`NewStmtCtx -> NewStmtCtxWithTimeZone -> NewStaticWarnHandler`；`ResetForRetry -> resetMuForRetry/AllocateTaskID/TruncateWarnings`；`PushDownFlags -> TypeFlags/ErrLevels/PushDownFlagsWithTypeFlagsAndErrLevels`；`SaveLogicalPlanBuildState -> GetWarnings/GetExtraWarnings/PlanCacheTracker`；`SQLDigest` 下游调用 parser 的规范化 digest 实现。
- `GetResultRowsCount` 只有在 `RuntimeStatsColl`、当前 `plan` 以及通过 `SetPlanIDFunc` 注册的全局回调三者都存在时，才把计划对象解析成 plan ID 并查询实际行数，否则返回 0。

## 错误处理与边界

- `GetOrEvaluateStmtCache` 与 `GetStaleTSO` 使用 `Result` 传播求值错误；失败不会写入缓存，调用者可以安全重试。
- `UsedStatsInfoForTable::WriteToSlowLog` 原样传播 `io::Result`；格式函数本身不吞写入失败。
- `StatsLoadResult::ErrorMsg` 仅在 `Error` 存在时输出包含 table ID、item ID 和 is-index 的上下文，否则返回空字符串。
- 大多数锁使用 `expect`，锁中毒会 panic；逻辑计划和统计待加载列表的部分锁使用 `PoisonError::into_inner` 继续访问。扩展时应保持相邻方法的一致策略，不要无意改变故障模型。
- `Reset` 忙锁时返回 `false` 是并发边界，不是异常；调用者必须处理“尚未重置”，不可假定调用后状态一定清空。
- `SetPlanDigest` 收到 `None` 时不覆盖旧值；`GetResultRowsCount` 缺失任一依赖返回 0；未安装 stale TSO evaluator 时 `GetStaleTSO` 返回 `Ok(0)`。这些均由当前实现定义，不能当作未初始化错误。
- `ReferenceCount::Decrease` 不自行阻止从零继续减小；正确性依赖调用者只为已成功增加的引用配对减少。

## 并发与资源生命周期

- `StatementContext` 将热路径布尔/计数状态放在原子量中；warning、行计数、digest、逻辑计划构建态、表/索引列表和统计状态分别由独立锁保护，以避免单一大锁。
- `OnceLock` 用于 Ranger、BuildPB 和 used-stats 等只初始化一次的共享值；`distSQLCtxCache` 使用 `Mutex<Option<_>>`，因为重试或新语句边界需要显式清空。
- `SetStaleTSOProviderIfNotExist` 和 `GetStaleTSO` 在同一互斥域内维护 evaluator 与成功值。求值闭包在持锁时执行，因此闭包不得回调同一 provider，否则可能自死锁，也不宜执行无界阻塞操作。
- `Reset` 在替换整个 `self` 前必须同时取得三个 Go 对齐锁域；其“全有或全无”的 `try_lock` 流程避免正在被其他线程使用的状态被替换。
- `DetachMemDiskTracker` 在语句资源释放时将内存和磁盘 tracker 从父树卸载；新资源路径若调用 `InitMemTracker`/`InitDiskTracker`，也应确保完成或重置路径执行 detach。
- `ReferenceCount`、`StatementHints`、备选计划信号、MPP ID、锁等待起点和全局 TaskID 都用原子操作。`GetLockWaitStartTime` 通过 CAS 保证所有调用者看到同一个首次等待时间。

## 与 Go 版本的对应关系

主要对应文件是 `pkg/sessionctx/stmtctx/stmtctx.go`，测试基线是 `pkg/sessionctx/stmtctx/stmtctx_test.go`。

- Rust 保留了 Go 的 `StatementContext`、`LogicalPlanBuildState`、`ReferenceCount`、`ReservedRowIDAlloc`、`UsedStatsInfo*`、`StatsLoadResult` 和绝大多数 Go 风格方法名，便于逐项迁移核对。
- 构造、完整 Reset、规划快照、类型/错误上下文、语句缓存、digest、行计数/warning、重试复位、下推 flags、stale TSO、tracker 和统计输出均可在 Go 同名方法中找到对应逻辑。
- Rust 用 `Arc<dyn Any + Send + Sync>` 代替 Go `any`，用 `Mutex`/`RwLock`/原子量表达 Go mutex/atomic；`chrono_tz::Tz` 对应 `*time.Location`。
- Go `GetResourceGroupTagger` 返回 `kv.ResourceGroupTagBuilder`，Rust 当前返回本地 `ResourceGroupTagger` 数据结构；Go 统计加载结果使用 channel，Rust `StatsLoadState::ResultCh` 当前为向量占位。这些是可见的迁移边界，不应声称完全等型。
- Go 测试包含更广的包集成和 failpoint 场景；Rust 独立测试集中在本 crate 可执行的真实 API。`stmtctx_test.rs` 覆盖 flags、快照、构造/时区、Reset、RowID 和统计输出；`stmtctx_1_aster_unit_test.rs` 进一步覆盖并发 TaskID/CAS、重试、缓存、stale TSO、digest 和完整 Reset；`planner_state_aster_unit_test.rs` 覆盖 hint 与规划共享状态。

## 扩展指南

1. **先判断状态生命周期。** 新字段属于执行尝试、整条语句、可跨 Reset 复用，还是进程全局；据此同步修改 `build`、`Reset`、`ResetForRetry` 和（如适用）`LogicalPlanBuildState`。最常见风险是重试泄漏旧值，或 Reset 错误清除 Go 版本要求复用的 map/cache。
2. **保持 Go 增量对齐。** 在 `stmtctx.go` 找到同名字段/方法和测试意图，只移植当前行为；若 Rust 必须以不同类型表达，应在边界处明确转换，而不是静默简化语义。
3. **选择正确同步原语。** 一次写入的缓存优先沿用 `OnceLock`；需要 Reset 的缓存必须可清空；复合状态需在单锁下形成快照；单值热路径可用原子量，但要说明所需 ordering。
4. **更新独立测试文件。** 不把测试嵌入 `stmtctx.rs`。通用 API 对照放入 `stmtctx_test.rs`，Go 边界/并发/生命周期对照放入 `stmtctx_1_aster_unit_test.rs`，规划器专属状态放入 `planner_state_aster_unit_test.rs`。
5. **扩展错误或下推 flags 时成对修改。** 同步更新 `newErrCtx`、`PushDownFlagsWithTypeFlagsAndErrLevels`、`PushDownFlags`、`InitFromPBFlagAndTz` 及 round-trip 测试，确认发送端与接收端对同一位的含义一致。
6. **关注兼容与性能。** 新 warning 会影响客户端协议与慢日志；新锁可能进入每行热路径；新增 `CacheValue` 必须满足 `Send + Sync` 且下转型双方类型一致；tracker 或 Arc 缓存必须有明确释放/失效时机。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/sessionctx/stmtctx` 确认目标源、crate 入口及三份 Rust 独立测试均在图中。
- 源码证据：`pkg/sessionctx/stmtctx/stmtctx.rs` 的 252 个索引符号；重点检查了 `StatementContext`、`build`、`Reset`、`Save/RestoreLogicalPlanBuildState`、缓存/digest、计数/warning、`ResetForRetry`、flags、stale TSO、统计格式和标签相关实现。
- 图查询证据：执行了 `query` 与 `callers`/`callees` 查询，覆盖 `NewStmtCtx`、`ResetForRetry`、`PushDownFlags`、`SQLDigest`、`SaveLogicalPlanBuildState`、`GetStaleTSO`；常见符号名造成的跨 Go/Rust同名噪声已通过文件路径和文件级引用结果消歧。
- crate 证据：`pkg/sessionctx/stmtctx/Cargo.toml`、`pkg/sessionctx/stmtctx/lib.rs`。
- Go 对照：`pkg/sessionctx/stmtctx/stmtctx.go`、`pkg/sessionctx/stmtctx/stmtctx_test.go`。
- Rust 测试证据：`pkg/sessionctx/stmtctx/stmtctx_test.rs`、`pkg/sessionctx/stmtctx/stmtctx_1_aster_unit_test.rs`、`pkg/sessionctx/stmtctx/planner_state_aster_unit_test.rs`。本任务为纯文档分析，按计划未运行 Cargo；测试文件用于确认边界与不变量，不作为本次运行通过的声明。

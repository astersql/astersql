# `pkg/executor/builder.rs`

## 文件定位

这里没有具体表写入实现，也没有新增显式的 `updateForUpdateTS` 调用；事务准备由具体适配负责。

## 核心职责

- 将计划树递归转换为执行器树。`executorBuilder::build` 覆盖管理语句、DML、DDL、连接、聚合、数据读取、Analyze、CTE、窗口、Shuffle 等 `Plan` 变体；简单叶节点统一走 `build_leaf`，有子计划的节点先递归构建子执行器。
- 提供一条可直接运行的 typed 物理计划路径。`BuildTypedTableScan`、`BuildTypedPointGet`、`BuildTypedPhysicalPlan`、`BuildTypedPhysicalPlanWithBindings` 和 `BuildTypedPhysicalSelectLockPlan` 组装 table/index scan、point get、selection、projection、limit、hash join、hash aggregate、index reader/index lookup 等已有 typed 算子。
- 保存一次语句构建期间的上下文和模式状态。`executorBuilder` 固定持有 statement/session context、构建时 `InfoSchema`、stale-read 与 replica scope、写语句/锁语句标志、遥测以及首个构建错误。
- 把环境相关操作隔离在 `ExecutorBuilderDependencies`。快照获取、事务时间戳、表解析、分区裁剪、DDL 系统会话、具体算子创建、CTE 存储、缓存表、索引连接 inner reader 等均通过该 trait 注入。
- 承担公共的构建辅助逻辑，包括索引列与 handle 列映射、下推列裁剪、快照选项、Analyze 采样率、Index Join 范围、动态分区、资源失败清理和读表合法性校验。

本次冻结增量增加 `MViewCompleteDeltaApplyPlanData`、`MViewDeltaMergePlanData`（均继承 `UnaryPlanData` 并提供 `target_table_id`）和对应 `Plan` 分支。`buildMViewCompleteDeltaApply` / `buildMViewDeltaMerge` 先调用 `build_required_child`，再分别委托 `build_mview_complete_delta_apply_executor` / `build_mview_delta_merge_executor`，最后通过 `finish` 保存工厂错误。这里没有具体表写入实现，也没有新增显式的 `updateForUpdateTS` 调用；事务准备由具体适配负责。模块导出和工厂端口不证明 `mview_complete_delta_apply::apply` 已接入 SQL 主链。

`UpdatePlanData::is_multi_table` 使工厂能够读取多表更新标志；冻结的 `buildUpdate` 仍把整个 plan 交给依赖，没有自行使用此标志或创建 `UpdateRowTracker`。

## 主要符号

- `Executor` / `ExecutorBox`：`Executor` 是 `ExecExecutor` 的空扩展 trait，因 blanket impl，任何真实 adapter executor 都能装箱。`builder_contract_test.rs` 验证装箱后仍具有 `Open`、typed `Chunk`、`Next` 和 `Close` 生命周期，而非标记桩。
- `BuildError`：构建期错误载体。`executorBuilder` 对外沿用 Go 风格的 `Option<ExecutorBox>`，把错误保存到 `err`；typed 入口和独立 helper 则直接返回 `Result<_, BuildError>`。
- `TypedScanBinding` 与 `BuildTypedPhysicalPlanWithBindings`：为一棵树中的每个扫描绑定精确 `table_id`、`Retriever` 和已编码 `KeyRange`。`typed_scan_binding` 按遍历顺序消费绑定，多表 Hash Join 因而不会错误复用单一数据源。
- `Plan<'a>`：统一分发枚举。各变体借用对应的 `*PlanData` trait；`PhysicalWrapper` 透明解包，`Deferred` 交给依赖延迟构建，`Mock` 直接返回给测试。
- `ExecutorKind`、`DeferredPlanKind`、`DataReaderPlanKind`、`DataReaderBuildKind`：把分发选择以稳定枚举传给依赖层，避免本文件依赖每个具体执行器类型。
- `ExecutorBuilderDependencies`：文件最重要的端口 trait。它定义构建具体 executor、事务/快照、DDL、DML 外键、Analyze、reader、Index Join、CTE、缓存表及分区等能力。
- `executorBuilder` / `newExecutorBuilder`：主状态机。构造时读取 staleness、transaction scope 和 read-replica scope；`build` 按 `Plan` 分派；`finish` 只保留第一个返回路径上的构建错误。
- `DataReaderBuilder`：Index Join inner worker 使用的可克隆构建上下文。它固定 `snapshot_ts`，共享 `statement_context_lock`、依赖对象和 `OnceLock` 分区裁剪结果，避免并发 worker 重新从可能已销毁的 session 取时间戳。
- `CTEStorages`：共享结果表、迭代表、producer 和 `OnceLock<Result<...>>` 初始化结果；`clear_after_build_error` 为递归计划构建失败提供清理语义。
- 关键纯函数包括 `buildIdxColsConcatHandleCols`、`buildHandleColsForExec`、`getAssignFlag`、`retrieveColumnIdxsUsedByChild`、`buildIndexScanOutputOffsets`、`buildRangesForIndexJoin`、`partitionPruning` 和 `InitSnapshotWithSessCtx`。这些函数把易错的 schema/range/选项推导从具体算子创建中分离出来。

## 执行流程

1. 常规路径由调用方以 `newExecutorBuilder` 创建 builder。构造函数从依赖对象读取 stale-read、事务 scope 与副本读取 scope，并把错误、锁、DML、UnionScan 和并发锁状态初始化为空。
2. `executorBuilder::build` 接收 `Option<Plan>`：空计划返回空；`PhysicalWrapper` 递归解包；其余变体分派到对应 `buildXxx`。叶节点调用 `dependencies.build_executor(kind, plan, [])`，复合节点先构建必要子节点，再把拥有所有权的 children 交给依赖层。
3. 递归构建失败时，子函数写入 `self.err` 并返回 `None`；父节点在继续前检查错误或用 `build_required_child` 将“无 executor 且无既有错误”转换为带算子名的 `BuildError`。成功结果由 `finish` 返回，失败结果被保存。
4. DML/锁语句在递归前设置状态并更新时间戳。`buildInsert` 初始化列和外键检查/级联；`buildUpdate` 计算 assignment flags 并校验更新列表；`buildDelete` 构建选择子树与外键动作；`buildSelectLock` 临时设置 `inSelectLockStmt`，只有满足悲观锁条件才包装锁执行器并设置 `hasLock`。
5. 读路径先取得稳定 snapshot TS，再创建“无范围”的 draft reader，执行访问/下推/动态分区校验，最后由依赖层 finalize。`buildTableReader` 对 MPP 单独走 `buildMPPGather`；`buildIndexReader`、`buildIndexLookUpReader`、`buildIndexMergeReader` 采用相同的 draft/finalize 两阶段形状。
6. Index Join 先构建 outer executor，再创建固定 snapshot 的 `DataReaderBuilder`。inner worker 按 `DataReaderPlanKind` 构建 TableReader、IndexReader、IndexLookupReader、UnionScan、Projection 或 HashJoin；范围可按分区分组、编码、排序，并受 `Tracker` 计量。
7. CTE 通过 `loadOrStoreCTEStorages` 双检共享 storage；`OnceLock` 保证 seed/recursive producer 只初始化一次。递归 child 或 producer 创建失败时清空 storage，防止后续读到半初始化状态。
8. typed 路径从 `build_typed_physical_plan` 对 concrete physical type 做 downcast。扫描使用 binding 创建 lazy KV executor，一元节点递归包 selection/projection/limit，Hash Join 构建左右子树，HashAgg 构建聚合器；不在明确支持集合中的节点立即返回错误，不以空 executor 或简化语义代替。

Analyze 的增量也在本文件：`AnalyzeColumnsTaskData` 新增 `configured_ndv_rate`、`configured_sample_count`、`display_name`，`AnalyzeSamplingConfig` 新增 `ndv_rate: Option<f64>`。`buildAnalyzeSamplingPushdown` 从 stats-meta/base count 取非负行数；PD 近似计数超过其五倍时采用近似值。`chooseNDVRateForTest` 在 threshold 为 0、配置 rate ≥ 1 或行数 ≤ threshold 时返回 `None`（全量 NDV）；否则使用显式正 rate，或 `max(0.05, threshold / rows)`，再提升到显式 sample count 对应的比例或 histogram sample rate。提升时调用 `append_analyze_note`；`chooseNDVRate` 复用同一纯选择器，`statsCountFarBehindPD` 统一五倍判定。配置选择与存储实际采样分别属于 builder 与下游任务的职责。

## 数据与状态

`executorBuilder` 的 `ctx`、`sctx` 和 `is` 是本次构建的稳定输入；与 Go 注释相同，`InfoSchema` 在执行期间不得发生语义替换。`isStaleness`、`txnScope`、`readReplicaScope` 在构造阶段快照化。`inUpdateStmt`、`inDeleteStmt`、`inInsertStmt`、`inSelectLockStmt` 和 `encounterUnionScan` 影响后代 reader 的缓存锁、临时表、锁读及 MPP 校验；这些字段属于单次 builder 的递归状态，不是全局状态。

`err: Option<BuildError>` 是 Go `b.err` 的 Rust 对应物。调用者必须把“返回 `None`”与 `error()` 一起解释；`MockExecutorBuilder::Build` 仅暴露 executor 结果，适合测试，生产接线仍需负责检查 builder 错误。

`TelemetryInfo` 记录账户锁、分区、CTE、多 schema change、exchange/flashback 等构建期事件。`setTelemetryInfo` 和 `buildSimple` 是主要写入点。`withStmtCtxLock` 只在 `stmtCtxLock` 存在时串行化 statement context/telemetry 相关工作，并能从 poisoned mutex 恢复 guard；普通串行 builder 不付出加锁成本。

`DataReaderBuilder` 的 `snapshot_ts` 是不可变的；`partition_pruning_result` 和 statement-context mutex 通过 `Arc` 在克隆间共享。`TypedScanBinding` 也持有 `Arc<dyn Retriever + Send + Sync>`，typed executor 获得 retriever 的共享所有权，不借用调用栈临时对象。

## 依赖与调用关系

上游分为两类。crate 装配由 `pkg/executor/lib.rs` 导出 `builder`；Rust 单元测试直接调用 typed 入口或 helper。完整 Go 主链的结构证据来自 `pkg/executor/builder.go`：Go `newExecutorBuilder`/`build` 与 Rust 同名结构和分发相对应，实际会话接线仍由仓库中实现 `ExecutorBuilderDependencies` 的适配层承担。RustCodeGraph 将 `builder.rs` 标识为已索引文件，并识别 `executorBuilder`、`ExecutorBuilderDependencies` 以及 Go/Rust 同名符号。

下游可分为四组：

- adapter 契约：`crate::adapter::ExecExecutor`、typed `Chunk`、执行上下文、锁键与 detach 生命周期。
- canonical typed runtime：`typed_kv_scan`、`typed_point_get`、`typed_selection`、`typed_projection`、`typed_limit`、`typed_index_reader`、`typed_index_lookup`、`typed_hash_join`、`typed_hash_agg`。
- 规划/元数据/存储接口：`astersql_planner_core_base::PhysicalPlan`、`astersql_planner_core_operator_physicalop::*`、`astersql_meta_model`、`astersql_kv::Retriever`、table/index codec 与 expression。
- 注入端口：`ExecutorBuilderDependencies` 将 executor、DDL、DML、snapshot、reader、partition、CTE 等具体实现隔离出去；`pkg/executor/Cargo.toml` 因此声明 executor 子 crate，以及 planner、expression、kv、meta、distsql、ddl、domain、session/util 等工作区依赖。唯一 feature `nextgen` 转发给 `astersql-dxf-importinto/nextgen`。

典型调用边为 `MockExecutorBuilder::Build -> executorBuilder::build -> buildXxx -> ExecutorBuilderDependencies::*`；typed 边为测试/adapter调用者 `-> BuildTypedPhysicalPlan[WithBindings] -> build_typed_physical_plan -> typed_*::new`。Index Join 边为 `buildIndexLookUpJoin -> newDataReaderBuilder -> DataReaderBuilder::BuildExecutorForIndexJoin -> build_data_reader_executor`。

## 错误处理与边界

- typed 入口严格拒绝不完整计划：缺少 `TableInfo`/`IndexInfo`、非法 partition index、不完整非唯一 point-get key、错误 child 数、缺失 scan binding、越界 index-column offset、非一元 pushdown 子树或不支持的算子都返回 `BuildError`。
- typed SelectLock 只接受 adapter 的普通 `FOR UPDATE` 路径；locking 模式拒绝 PointGet 和 HashJoin。Hash Join 还拒绝 null-aware anti join key，并要求恰好两个 child；TableDual 只允许 0 或 1 行。
- 常规 builder 不用 panic 表示业务构建失败，而把依赖错误写入 `err`。必需 child 为空、schema 列越界、merge-join inner filter 非空、Index Join 找不到 lookup child、非法 `_tidb_rowid` 更新等均有显式错误。
- 临时表和 cache table 禁止 stale read；设置 `tidb_snapshot` 时禁止读本地临时表。`TABLESAMPLE` 对本地临时表报错，对全局临时表使用 empty sampler。
- 动态分区结果会排序；partition index 的负值/越界会报错；`fullRangePartition([-1])` 才表示全分区。索引下推只允许列形式的 `ORDER BY`，输出 offset 必须存在且可转换为 `u32`。
- `buildPlanReplayer` 在 dump 模式逐条解析 SQL，并在错误中保留 SQL 与原始错误上下文。`buildShowDDL` 无论查询 DDL info 成功与否都会在查询后释放 system session。
- 当前 Rust 文件并非 Go `builder.go` 全部具体实现的一比一内联复制：大量算子由 trait 注入，且 typed 直接路径只覆盖代码中明确列出的 canonical 节点。因此扩展时不能把存在 `Plan` 变体等同于已经存在完整生产接线，必须同时验证依赖实现和测试。

## 并发与资源生命周期

执行器所有权通过 `Box<dyn Executor>` 沿树向父节点转移；成功构建后由 adapter 按 `Open -> Next/NextWithContext -> Close` 驱动。typed scan 是惰性的：构建与 `Open` 不启动 KV iterator，首次 `Next` 才读取；相关测试验证分页后不会多开 iterator，`Detach`/`Close` 维持游标和 snapshot 生命周期。

Index Join inner builder 可跨 worker 克隆，但共享固定 snapshot TS、statement-context mutex、分区裁剪 `OnceLock` 和依赖对象。`buildIndexJoinHashJoinChildrenWithCleanup` 保证第二个 child 构建失败时关闭已成功构建的第一个 child；`builder_index_join_cleanup_test.rs` 验证关闭恰好发生一次。

CTE storage 由 `Arc` 共享，三个可选资源各受 `Mutex` 保护，初始化结果由 `OnceLock` 发布。`loadOrStoreCTEStorages` 在锁外先读、锁内再读后创建，避免并发重复存储；递归构建失败会清空 storage。`builder_index_join_cleanup_test.rs` 验证 seed/recursive 失败后的清空状态。

`withStmtCtxLock` 的锁只保护 builder 并发构建触碰的 statement context/telemetry；它不是 executor 运行锁。快照由 `getSnapshot`/`getSnapshotTS` 在构建阶段取得，`InitSnapshotWithSessCtx` 依次设置 replica scope、task id、超时、resource group、request source，并仅在 closest-read 且非 global scope 时增加 store label。

## 与 Go 版本的对应关系

MV signed/full-join 派生源使用 canonical 输出 schema 映射 typed table/join/reader/selection 列；外连接类型由 typed join 保留可空性。TableScan 的 FilterCondition/AccessCondition 在本地求值前按输出 schema 解析下标，不能直接使用存储侧未解析谓词。partial aggregate 输出附带分组值，供 final aggregate 按真实位置读取。MIN/MAX 有界回查选出的 StreamAgg 复用已有 typed 聚合状态内核，保留完整 AggFuncDesc/模式/分组键；该本地路径按组保留状态。旧的手工计划未填写输出 schema 时保留孩子的 typed contract；实际非空 canonical schema 始终按列身份映射。

MV ReaderSnapshot 持有 canonical 非 Send KV snapshot，Snapshot 不再强制 Send；执行器本身仍遵守既有线程内生命周期，未使用 unsafe 扩大线程边界。

Rust 的 `Plan` enum 代替 Go type switch；`*PlanData` traits 代替具体 planner struct 的直接字段访问；`ExecutorBuilderDependencies` 代替 Go 中对 session、domain、DDL、KV 和具体 executor 构造函数的直接调用。

Rust 保留 Go 的关键顺序约束，例如先递归构建 child 再构建父 executor、Index Join 固定 read TS、Show DDL 查询后释放系统会话、DML 在构建前更新 for-update TS、reader 先建 draft 再 finalize、CTE producer 单次初始化以及失败清理。

差异必须显式看待：Rust 通过依赖 trait 把许多 Go 具体结构的字段装配移出本文件；`BuildTypedPhysicalPlan*` 是 Rust 当前可运行 typed 路径，在 Go `builder.go` 中没有同形 API；Go 文件仍包含 Rust `Plan` 未列出的分支或更多细节（例如具体 executor 字段、额外计划种类与优化配置）。

## 扩展指南

新增计划种类时，应同时完成最小闭环：在 `Plan` 增加带正确 plan-data trait 的变体，在 `executorBuilder::build` 添加分支，选择或新增 `ExecutorKind`，把策略校验放在对应 `buildXxx`，把环境/具体算子构造能力放入 `ExecutorBuilderDependencies`，并在独立 `*_test.rs` 中覆盖成功路径、非法 child/schema 和依赖失败。不要把单元测试写回 `builder.rs`。

扩展 typed canonical 支持时，在 `build_typed_physical_plan` 添加精确 downcast 与 child 数校验，并复用现有 `typed_*` executor；若是 scan 类型，还要定义 binding 消费、table/partition ID 和锁键语义。测试应仿照 `typed_limit_test.rs`、`typed_index_reader_test.rs`、`typed_index_lookup_test.rs`、`typed_hash_join_test.rs`、`typed_hash_agg_test.rs`，验证惰性读取、分页、锁键、取消、关闭和不支持分支，而不以“能构建”作为唯一证据。

修改 reader/Index Join 时必须保持 fixed snapshot TS、`Tracker` 传递、动态分区排序/缓存和并发共享状态；修改 CTE 时必须保持双检存储、单次初始化与失败清理；修改 DML/SelectLock 时必须审计递归期间的状态标志恢复、for-update TS 和外键阶段顺序。新增外部能力优先扩展依赖 trait，不要在 builder 内重新引入全局服务定位。

兼容性风险主要是 Go 分支遗漏、错误文本/顺序差异和锁/临时表语义；性能风险主要是重复取 snapshot、重复 partition pruning、提前打开 KV iterator、丢失列裁剪或错误复制大范围数据。任何变更都应与 `pkg/executor/builder.go` 的对应函数逐段核对，并同步最接近的独立 Rust 测试及必要的 Go 测试意图。

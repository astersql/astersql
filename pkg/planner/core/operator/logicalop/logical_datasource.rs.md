# `pkg/planner/core/operator/logicalop/logical_datasource.rs`

## 文件定位

本文件实现逻辑计划中的表叶节点 `DataSource`：它位于 SQL 已解析、表与列元数据已经绑定之后，物理扫描计划尚未确定之前，集中保存表元数据、过滤条件、候选访问路径和统计信息。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 以 `mod logical_datasource` 装配本模块并通过 `pub use logical_datasource::*` 对外导出；`pkg/planner/core/operator/logicalop/Cargo.toml` 将该 crate 定义为 `astersql-planner-core-operator-logicalop`，并声明它直接依赖表达式、cardinality、ranger、statistics、planner property/util、KV、元数据和规则工具等本地 crate。

`DataSource` 不是最终执行器，也不直接读取 KV。它把一个未物理化的表访问表示成逻辑叶子，经过谓词下推、列裁剪、Range/行数推导后，再由 `Convert2Gathers` 变成 `LogicalTableScan` 或 `LogicalIndexScan`，外包一层 `TiKVSingleGather`，供 cascades 转换和物理计划路由继续处理。直接接线证据包括 `pkg/planner/cascades/old/transformation_rules.rs:1351` 和 `pkg/planner/core/operator/physicalop/base_physical_plan.rs:8955` 对 `DataSource::Convert2Gathers` 的调用。

## 核心职责

1. **保存表访问的完整规划状态。** `DataSource` 同时持有 `TableInfo`/`Columns`/`TblCols`、输出 `Schema`、句柄列、分区信息、`AllConds`/`PushedDownConds`、`TableStats` 以及两套访问路径集合。`AllPossibleAccessPaths` 表示构建数据源时的全集，`PossibleAccessPaths` 表示当前逻辑分支在既有谓词下仍可选的路径。
2. **把谓词变成存储访问约束。** `PredicatePushDown` 先调用运行时注册的谓词简化，再区分可编码到存储层的条件与必须保留在上层的条件；随后 `deriveAccessPathsFromPredicates` 为表路径和索引路径拆分 `AccessConds`、`IndexFilters`、`TableFilters`，构造 Range，并估算访问后的行数上下界。
3. **维护逻辑性质。** `PruneColumns` 保留父节点和过滤条件真正需要的列，必要时保留一个物理句柄列；`BuildKeyInfo` 填充唯一键；`PreparePossibleProperties` 枚举句柄/索引可提供的排序；`ExtractCorrelatedCols` 和 `ExtractFD` 为关联子查询及函数依赖规则提供信息。
4. **生成扫描候选。** `Convert2Gathers` 按 `PossibleAccessPaths` 构造表扫描或索引扫描候选，并把 Range、过滤条件、存储类型、索引元数据和单扫/双读判定复制到扫描与 Gather 节点。
5. **守住访问路径边界。** `HandleColsToAppend` 描述非唯一二级索引物理键末尾追加句柄的规则；`IsSingleScan`/`IsIndexCoveringCondition` 判断覆盖索引；`CheckPartialIndexes` 移除不能由已下推条件满足的部分索引，并为计划缓存记录不可缓存原因。

## 主要符号

- `pub type DataSourceRef = Rc<RefCell<DataSource>>`：同一数据源由 Gather 与 Scan 共享的单线程引用句柄。`Rc` 表示非线程安全共享，`RefCell` 将独占/共享借用检查延迟到运行时。
- `DataSource`：核心状态对象。重要字段分为元数据（`TableInfo`、`Columns`、`DBName`）、条件（`AllConds`、`PushedDownConds`）、统计与路径（`TableStats`、`AllPossibleAccessPaths`、`PossibleAccessPaths`）、句柄与全列映射（`HandleCols`、`TblCols`、`CommonHandleCols`）、存储偏好和扩展信息（`PreferStoreType`、`IndexMergeHints`、`FtsPushDown`）。
- `PossiblePropertiesInfo`：返回可用排序列序列和 TiFlash 可用性。`HandleCoverState` 描述列是否由句柄覆盖；当前 `CoveredByPrefix` 变体没有在本文件的判断路径中产生。
- `FTSQueryInfo`/`FTSPushDown`：保存全文索引、查询文本、分词器、TopK 和是否需要评分；本文件只承载状态，不负责提取或执行 FTS。
- `InstallPredicateSimplificationPassthrough`：向 `rule_util` 注册 DataSource/Join 使用的谓词简化回调。它执行常量传播、CNF 拆分和折叠、恒真删除、恒假/冲突等值归零以及 `A AND (A OR B)` 吸收；`pkg/session/runtime/session.rs:1521` 与 `pkg/planner/core/rule/rule_init.rs:41` 是生产接线入口。
- `BuildPseudoHistColl` 与 `pseudo_filter_selectivity`：前者为伪统计补齐公开列直方图元数据，使伪表也能按列进入 cardinality 流程；后者实现本文件内的伪范围因子辅助逻辑，但当前没有被其他符号调用，应视为尚未接线的内部辅助函数。
- `scale_correlated_count_after_access`/`apply_correlated_selectivity`：关联等值条件在执行期才能物化为 Range，这两个函数在点估计按 NDV 缩放时同步缩放 `MinCountAfterAccess`/`MaxCountAfterAccess`，避免风险区间仍保持表级大小。
- `PredicatePushDown`、`deriveAccessPathsFromPredicates`、`DeriveStats`：从条件简化到 Range/选择率/统计落盘的主链。RustCodeGraph 的精确 callers 查询显示，`deriveAccessPathsFromPredicates` 由前两者共同调用。
- `buildTableGather`、`buildIndexGather`、`Convert2Gathers`：把逻辑叶子物化成扫描候选的出口。表路径生成 `LogicalTableScan`；索引路径依据 `IsSingleScan` 设置 `IsDoubleRead`，并携带索引过滤与回表过滤。
- `impl LogicalPlan for DataSource`：把上述方法接入统一逻辑计划接口；`PredicatePushDownRoot` 额外在恒假条件时返回零行 `LogicalTableDual`。

## 执行流程

典型流程如下：

1. 计划构建器填充 `DataSource` 的表/列元数据、句柄、Schema、初始统计和访问路径，然后以 `Init` 安装名为 `DataSource` 的 `BaseLogicalPlan`。存储提示在路径产生后通过 `ForceTiFlashPath` 或 `ForceTiKVPath` 过滤候选；其直接调用点在 `pkg/planner/core/logical_plan_builder_runtime.rs:4918-4921`。
2. 规则层通过 `LogicalPlan::PredicatePushDownRoot` 进入 `PredicatePushDown`。注册回调先简化条件；`AllConds` 保存简化后的全量条件，只有普通列均属于本 Schema 且表达式可编码（尤其排除 `SubqueryRefID != 0` 的延迟子查询常量）时才进入 `PushedDownConds`，其余条件返回父层保留。
3. `deriveAccessPathsFromPredicates` 规范化特定 timestamp-to-datetime cast 后遍历 `PossibleAccessPaths`。表路径以句柄列调用 `DetachCondsForColumn`/`BuildTableRange`；`pk = correlated-column` 被特殊识别为执行期参数化点查。索引路径解析声明索引列，按 `HandleColsToAppend` 补全物理句柄后缀，调用 `DetachCondAndBuildRangeForIndex`，并依据覆盖能力拆成索引过滤与回表过滤。
4. 若普通 detacher 没生成首列 Range，代码会以 `BuildColumnRange` 回退并去重残余过滤；`IS NULL` 对前缀索引有专门处理。关联列访问条件通过 `SplitCorColAccessCondFromFilters` 移入 `AccessConds`，随后按列 NDV 或伪统计比例更新点估计和风险上下界。
5. 每条路径使用真实直方图的 `GetRowCountByColumnRanges`/`GetRowCountByIndexRanges`，或使用等值约 `1/1000`、单边范围约 `1/3` 的伪估计。顶层 DNF 若每个分支都有可用首列索引，还会形成 `PartialAlternativeIndexPaths` 的 index-merge 候选。完成后复制到 `AllPossibleAccessPaths`。
6. `PredicatePushDown` 调用 `CheckPartialIndexes`，然后返回上层保留条件；若全量条件恒假，trait 层构造零行 `LogicalTableDual`。列裁剪阶段再由 `PruneColumns` 保留输出列和条件列，并记录 `ColsRequiringFullLen`/`PrunedOutputColumns` 供覆盖索引与 MPP 投影边界使用。
7. `DeriveStats` 再次确保路径与当前条件一致，优先使用真实 `HistColl`，否则使用 `BuildPseudoHistColl`；它调用 `cardinality::Selectivity` 缩放表行数/NDV，并对追加句柄的 Range 做保守修正，最后以 `SetStats` 缓存结果。
8. 物理枚举调用 `Convert2Gathers`：表路径生成 TableScan + Gather，索引路径生成 IndexScan + Gather。后者将非覆盖索引标成双读，并分别携带 `IndexFilters` 与 `TableFilters`，随后交给物理规则和成本选择。

## 数据与状态

`AllConds` 是 Range、统计、部分索引判断的事实源；`PushedDownConds` 是可交给数据源/存储侧的子集。两者不能随意合并：上层仍需执行的表达式必须由 `PredicatePushDown` 返回，而关联条件虽含外层操作数，仍可借助本地列形成执行期 Range。

`AllPossibleAccessPaths` 与 `PossibleAccessPaths` 也有不同生命周期。强制存储类型、谓词派生和部分索引检查会过滤或扩展当前路径；`PreparePossibleProperties` 使用完整路径集合枚举排序，`Convert2Gathers` 使用当前路径集合生成候选。访问路径中的 `CountAfterAccess`、`MinCountAfterAccess`、`MaxCountAfterAccess`、`CountAfterIndex` 必须同步维护，测试 `correlated_column_selectivity_*` 专门防止只修改点估计。

`TableStats.HistColl` 以 `Arc<dyn Any + ...>` 形态共享。追加 common handle 到索引列映射时，代码先 `Copy` 直方图再发布新的 `Arc`，不原地修改其他计划可能共享的统计快照。`getGroupNDVs` 只读取索引声明列数量对应的映射前缀，避免物理追加句柄污染组合 NDV。

句柄相关状态包括整数 PK/额外 `_tidb_rowid`、common handle 列及长度。`HandleColsToAppend` 仅对满足物理键布局的非唯一、非主索引追加句柄；全局、MV、列式索引、列解析失败、句柄重复以及 v0 新排序规则字符串句柄都会禁止追加。列裁剪若会留下空 Schema，`preferKeyColumnFromTable` 优先保留已有句柄，保证 `SELECT 1 FROM t` 仍能从存储返回行。

## 依赖与调用关系

上游调用关系：

- 统一规则遍历通过 `base_logical_plan.rs` 的 `PredicatePushDownPlan`/`LogicalPlan::PredicatePushDownRoot` 进入本节点；RustCodeGraph 将目标 `PredicatePushDown` 与该 trait 入口关联。
- `pkg/planner/cascades/old/transformation_rules.rs` 和 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 调用 `Convert2Gathers`，把数据源变成扫描/Gather 候选。
- `pkg/planner/core/optimizer_runtime.rs:5897` 的 `refresh_join_order_stats` 是 `DeriveStats` 的直接生产调用者之一；相邻测试与物理路由也通过 trait 调用它。
- `pkg/planner/core/logical_plan_builder_runtime.rs` 调用 `ForceTiFlashPath`/`ForceTiKVPath`；session/rule 初始化调用 `InstallPredicateSimplificationPassthrough`。

主要下游依赖：

- `ranger` 负责条件拆分、表/索引/列 Range 构造与 Range 合并。
- `cardinality` 与 `statistics` 根据 Range、直方图和伪统计计算选择率及行数；`CardinalityContextAdapter` 把 `base::PlanContext` 适配到它所需的 session/expr/ranger 接口。
- `rule_util` 提供谓词简化注册、索引键判定和部分规则桥接；`expression` 提供表达式分类、列/关联列抽取、常量传播与折叠。
- `planner_util::AccessPath` 是本文件与路径生成、物理枚举之间的核心交换对象；`property::StatsInfo`/`GroupNDV` 是优化器成本与性质推导的输出。
- `LogicalTableScan`、`LogicalIndexScan`、`TiKVSingleGather` 是本模块的直接下游计划节点，并通过 `DataSourceRef` 共享源节点。

RustCodeGraph 的文件节点还显示 `memtable_infoschema_extractor.rs`、`physicalop/base_physical_plan.rs`、`physicalop/index_join_probe.rs`、`optimizer_runtime.rs`、`session/runtime/session.rs` 使用本文件。对同名 Go/Rust 方法的未限定查询会混入 Go 节点，因此可靠调用证据应使用 `--file pkg/planner/core/operator/logicalop/logical_datasource.rs` 限定。

## 错误处理与边界

本文件对 ranger/cardinality 错误统一 `map_err(|error| PlannerError(error.to_string()))` 向上传播；缺少必要的索引列类型会返回 `PlannerError("index column type is required")`，`DeriveStats` 或恒假替换缺少计划上下文也返回明确错误。相比之下，`deriveAccessPathsFromPredicates` 在尚未 `Init`、没有 `SCtx` 时直接 `Ok(())`，允许默认对象用于有限的测试/装配；而 `NewExtraHandleSchemaCol` 和 `NewExtraCommitTSSchemaCol` 明确 `expect` 已初始化上下文，调用方必须满足此前置条件。

空/异常边界包括：零 `count_before_split` 时将风险上下界直接设为当前点估计；无真实直方图时走伪统计；无句柄的表路径只能保留全部条件为表过滤；无法解析完整声明索引列时禁止句柄追加；空访问条件的部分索引不可用；部分索引表达式解析失败或无法证明约束时也会被移除。

部分索引的计划缓存判定刻意保守：`partial_index_always_meets_constraints` 只证明单个 `IS NOT NULL` 约束，且要求已下推过滤对同列 null-reject。更复杂的蕴含不能据此扩展，否则可能让带参数的计划错误复用。

当前 Rust 与 Go 的能力边界也属于错误风险：Rust `UsedHypoTiFlashReplicas` 只检查 `TableInfo.TiFlashReplica.Available`，没有 Go 侧 `SessionVars.InExplainStmt/HypoTiFlashReplicas` 上下文；`BuildKeyInfo` 没有 Go 侧 for-update/RC 下查询最新索引状态的分支；`ExtractFD` 没有在本文件复刻 Go 对主键、唯一索引、条件和生成列的完整 FD 构建。调用方不应假定这些部分已完全对齐。

## 并发与资源生命周期

本模块没有启动线程、异步任务、通道、事务或显式 I/O。`DataSourceRef = Rc<RefCell<_>>` 明确限制在单线程规划阶段：共享所有权由 `Rc` 计数，修改必须取得 `borrow_mut`；重叠的可变/不可变借用会在运行时 panic，因此 `buildTableGather`/`buildIndexGather` 都把借用限制在短作用域内，再移动或克隆源引用。

表达式、列、Range 和访问路径多通过克隆形成逻辑分支快照，避免一个候选的派生破坏另一个候选。统计直方图可能跨计划以 `Arc` 共享，但追加句柄映射时采用 copy-on-write 风格：复制 `HistColl`、修改副本、再替换 `TableStats.HistColl`。这不是并行写同步机制；它只保证规划分支之间不共享可变统计对象。

`InstallPredicateSimplificationPassthrough` 修改的是规则层的全局回调注册槽，生命周期应早于任何规划请求。生产代码在 session/rule 初始化时安装，测试入口也显式安装；新增依赖该简化行为的测试必须确保初始化已发生，避免测试顺序隐式决定结果。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_datasource.go`，Cargo metadata 也声明同一 Go package。类型字段和主要方法名称总体对应：`DataSource`、`ExplainInfo`、`PredicatePushDown`、`PruneColumns`、`BuildKeyInfo`、`DeriveStats`、`PreparePossibleProperties`、`ExtractCorrelatedCols`、`ExtractFD`、Gather 构造、句柄追加、覆盖索引和部分索引检查均能找到同名来源。

已保持的关键语义包括：空投影仍保留句柄列；索引等值前缀之后的后缀可提供排序；common handle 只在符合 TiKV 物理键布局时追加；前缀索引可以独立判断 `IS NULL`；部分索引必须被下推条件满足，计划缓存只对安全的 `IS NOT NULL` 情形视为恒成立；伪统计和关联列选择率会影响访问路径行数。

需要明确记录的差异：

- Go 的 `PredicatePushDown` 还调用 shard-index 前缀补全及通用 `PushDownExprs`；Rust 以本地 Schema/可编码性判断替代，并在本文件直接派生 Range。
- Go 的 `DeriveStats` 委托 `utilfuncp.DeriveStats4DataSource`；Rust 在本文件内实现直方图/伪统计选择率和路径风险修正。
- Go 当前 `Convert2Gathers` 只为覆盖索引生成 index gather，并保留 index lookup TODO；Rust 会生成索引候选并用 `IsDoubleRead` 表示需要回表。
- Go `BuildKeyInfo`/`ExtractFD` 含读取最新索引状态和完整 FD 图逻辑；Rust 仅处理公开索引键，并将 FD 提取委托给基类，不能宣称完全对齐。
- Go 的 `HasTiFlash` 包括 session 中的假设副本；Rust `HasTiFlash` 只看真实 `TableInfo`，同文件的 `UsedHypoTiFlashReplicas` 也没有 session 参数。
- Rust `FTSPushDown`、`IndexMergeHints` 等字段在本文件主要是状态载体；它们的产生/消费需要在 builder 或其他规则中继续追踪，不能从字段存在推断本文件已完成端到端功能。

## 扩展指南

修改谓词/Range 行为时，优先接入 `PredicatePushDown` 与 `deriveAccessPathsFromPredicates`，并同时检查 `AllConds`、`PushedDownConds` 和返回父层条件三者的归属。新增表达式类型若能下推，必须同时验证存储编码能力、ranger 是否能识别、未吸收条件是否仍落入正确的 index/table filter，以及真实/伪统计是否给出一致方向的估计。

修改索引物理布局时，以 `HandleColsToAppend` 为单一判定入口，同时检查 Range 建造、`Idx2ColUniqueIDs` 映射、`IsSingleScan` 和 index pruning；不要只在某一路径追加句柄。新增部分索引约束证明应扩展 `partial_index_always_meets_constraints`，并为带参数计划缓存补充正反例，默认应保守返回 false。

修改列裁剪或覆盖索引时，同步检查 `PruneColumns`、`ColsRequiringFullLen`、`PrunedOutputColumns`、`IsIndexCoveringCondition` 和 `preferKeyColumnFromTable`。必须保留“零输出列仍读取一个物理列”和“过滤专用列不一定暴露给父节点”两个不变量。

修改统计时，必须一起维护点估计及 min/max 风险边界，并避免原地修改共享 `HistColl`。修改物理候选构造时，检查 Range、过滤条件、`StoreType`、`NoncacheableReason`、单扫/双读标志在 Scan 与 Gather 间是否完整传递。

测试必须放在独立文件，不内嵌到生产 `.rs`。首选扩展 `logical_datasource_test.rs`（Range、句柄布局、统计风险）和 `logical_datasource_aster_unit_test.rs`（键、裁剪、属性）；涉及物理转换时还应同步 `logical_table_scan_test.rs`、`logical_index_scan_test.rs`、`logical_tikv_single_gather_test.rs` 或 physicalop 路由测试。Go 对照回归可参考 `pkg/planner/core/logical_plans_test.go` 的谓词下推/列裁剪，以及 `pkg/planner/core/casetest/index/index_test.go` 的部分索引计划缓存/裁剪用例。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件节点完整覆盖 2,205 行，并报告 5 个直接使用文件。
- RustCodeGraph `query/node`：核对了 `DataSource` 文件全貌、`deriveAccessPathsFromPredicates`（572 行）、`PredicatePushDown`（1246 行）、`DeriveStats`（1470 行）、`Convert2Gathers`（1672 行）、`PredicatePushDownRoot`（2091 行）与 `BuildPseudoHistColl`（91 行）。
- RustCodeGraph 精确调用查询：`callers deriveAccessPathsFromPredicates --file ...` 返回 `PredicatePushDown` 和 `DeriveStats`；`callers BuildPseudoHistColl --file ...` 返回 `DeriveStats`；`callees Convert2Gathers --file ...` 确认 Gather 构造与覆盖判断关系。未限定文件的同名查询会混入 Go 节点，本文未把这些噪声作为 Rust 调用事实。
- 已读源码/配置：`logical_datasource.rs`、`logicalop/lib.rs`、`logicalop/Cargo.toml`、`logical_datasource.go`，以及直接生产调用点 `transformation_rules.rs`、`base_physical_plan.rs`、`logical_plan_builder_runtime.rs`、`session/runtime/session.rs`、`rule/rule_init.rs` 的搜索结果。目标 Rust package 目录没有 `doc.go`；最近的 `pkg/planner/core/base/doc.go` 属于另一 package，未用它替代本模块事实。
- 已读独立 Rust 测试：`logical_datasource_test.rs` 与 `logical_datasource_aster_unit_test.rs`。覆盖证据包括额外句柄、空列裁剪、Range 回退残余条件、common-handle NDV/映射/布局守卫、追加句柄估计、关联选择率风险区间、主键/唯一键和索引后缀排序。相关 Go 测试位置通过仓库搜索核对为 `pkg/planner/core/logical_plans_test.go` 与 `pkg/planner/core/casetest/index/index_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证：目标文件存在且恰有 11 个固定二级章节。

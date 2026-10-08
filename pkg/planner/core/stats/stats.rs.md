# `pkg/planner/core/stats/stats.rs`

## 文件定位

本文件是独立 crate `astersql-planner-core-stats` 的业务实现；crate 入口 `pkg/planner/core/stats/lib.rs` 公开 `stats` 模块，根工作区在 `Cargo.toml` 中把该目录列为 member，并通过 `facade_planner_core_stats` 暴露它。它负责把“按物理表 ID 取得统计快照”这一规划期需求压缩成一组可独立测试的 Rust 数据模型和函数。

在完整 TiDB/AsterSQL 规划链中的对应位置可由 Go 代码确认：`pkg/planner/core/stats.go:initStats` 调用 Go 的 `stats.GetStatsTable` 初始化逻辑数据源的行数、直方图和版本，并记录 UsedStats；各类物理读计划的 `LoadTableStats` 方法则在执行前记录语句实际用到的统计信息。当前 Rust 侧尚未接入这些生产调用者：RustCodeGraph 与 `rg` 均只发现 `pkg/planner/core/stats/stats_aster_unit_test.rs` 直接调用本文件的三个公开函数。因此，本文件当前是已实现且有行为回归测试的独立移植边界，而不是 Rust 规划主链上已经完成接线的统计服务。

`pkg/planner/core/stats/Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/planner/core/stats`；其历史依赖只声明在 `target.'cfg(windows)'.dependencies` 下，而本文件实际只使用标准库 `BTreeMap`。这意味着本文描述的是当前可编译源码的局部模型，不应把 Cargo 中列出的 Go 侧领域依赖误写成当前 Rust 实现的运行时依赖。

## 核心职责

文件围绕三项职责组织：

1. `table_info_for_used_stats` 把物理 ID 解析成 UsedStats 可展示的名称，并返回对应 `TableInfo`。名称区分普通表（`orders`）、具体分区（`orders p0`）、分区表全局统计（`orders global`）和元数据缺失（`tableID 99`）。
2. `get_stats_table` 根据统计源是否存在、分区裁剪模式、优化目标、行数、初始化状态和过期开关，选择真实统计或伪统计，并保持确定性优化所需的“仅使用 ANALYZE 行数”语义。
3. `load_table_stats` 以物理 ID 为键，把一次规划/执行语句使用的统计摘要写入 `BTreeMap<i64, UsedStatsInfo>`；重复 ID 不重新加载，伪统计统一记录 `PSEUDO_VERSION`。

该文件不实现直方图、NDV、统计缓存刷新、异步加载或指标计数。`StatisticsTable` 只保留实现上述分支所需的元数据字段；真实统计获取与物理 ID 到表/分区的解析通过 `StatsSource` 注入。

## 主要符号

- `PSEUDO_VERSION: u64 = 0`：伪统计写入 UsedStats 时使用的固定版本。`StatisticsTable::pseudo` 和 `load_table_stats` 都遵循这一约定。
- `PartitionInfo { id, name }`：最小分区元数据；仅承担物理 ID 匹配和显示名拼接。
- `TableInfo { id, name, partitions }`：最小表元数据。`partitions` 非空且没有具体分区命中时，`table_info_for_used_stats` 使用 `global` 后缀。
- `StatisticsTable`：统计快照值对象，包含 `realtime_count`、`modify_count`、`analyze_count`、`version`、`initialized`、`outdated`、`pseudo` 和 `allow_pseudo_loading`。所有权按值传递，使调用者可调整副本而不修改数据源缓存。
- `StatisticsTable::pseudo(allow_loading)`：构造默认行数为 `10_000` 的伪表；版本为 0、未初始化、`pseudo = true`，并保存是否允许触发后续加载。
- `StatsSource` trait：外部边界，要求实现 `table_by_physical_id` 和 `physical_stats`。前者支持 UsedStats 名称解析，后者按选定的物理 ID 返回统计快照；两者都允许以 `Option` 表示缺失。
- `OptimizationObjective::{Default, Determinate}`：决定是否保留实时修改，或把行数收敛到非负的 ANALYZE 行数。
- `table_info_for_used_stats(&dyn StatsSource, i64)`：公开的名称/元数据解析入口。
- `get_stats_table(Option<&dyn StatsSource>, &TableInfo, i64, bool, OptimizationObjective, bool)`：公开的统计选择入口。两个布尔参数分别表示动态分区裁剪和“过期统计转伪统计”开关。
- `UsedStatsInfo`：语句级摘要，保存名称、表元数据副本、实时行数、修改量和版本，不保存完整统计快照。
- `load_table_stats(&mut BTreeMap<...>, ...)`：公开的幂等记录入口，内部调用 `get_stats_table`。

文件中没有宏、条件编译项、异步函数或内部私有辅助函数；除 `StatisticsTable::pseudo` 外，列出的类型和函数均为公开 API。

## 执行流程

`table_info_for_used_stats` 的流程如下：

1. 调用 `StatsSource::table_by_physical_id(id)`。
2. 若没有元数据，返回稳定的回退名 `tableID {id}` 和 `None`。
3. 若命中具体 `PartitionInfo`，返回“表名 + 分区名”。
4. 若未命中具体分区但表的 `partitions` 非空，返回“表名 + global”。
5. 否则返回普通表名；成功路径均返回 `Some(TableInfo)`。

`get_stats_table` 的分支顺序是行为契约的一部分：

1. `source` 为 `None` 时立即返回不可触发加载的伪表。
2. 当请求的是表 ID，或 `dynamic_partition_pruning` 为真时，用 `table.id` 取全局统计；否则用传入的 `physical_id` 取分区统计。
3. `StatsSource::physical_stats` 返回 `None` 时，同样返回不可触发加载的伪表。
4. `objective == Determinate` 时把 `analyze_count` 截断到至少 0。只有实时行数或修改量需要变化时才覆盖副本：`realtime_count = analyze`、`modify_count = 0`。
5. 若原统计不是伪表、原实时行数大于 0、但确定性模式得到的 ANALYZE 行数为 0，则设置局部 `allow_loading = true`。随后的零行数分支据此生成可触发加载的伪表，避免把“列/索引统计尚未加载”误当成永久无统计。
6. 调整后 `realtime_count == 0` 时返回新伪表；否则，未初始化或在开关开启时已过期的快照只把 `pseudo` 标志设为真，保留其行数和版本字段。
7. 其余情况返回选中的真实快照。

`load_table_stats` 先检查 `record.contains_key(physical_id)`；已存在即返回。未存在时调用 `get_stats_table`，再在 `table.partitions` 中按 ID 查找分区名，构造 `UsedStatsInfo` 并插入。版本字段在 `stats.pseudo` 为真时强制改为 `PSEUDO_VERSION`，否则沿用统计快照版本。

## 数据与状态

本文件没有全局可变状态。`PSEUDO_VERSION` 是唯一模块级常量；所有表、分区和统计数据都由参数传入或按值返回。

`StatisticsTable` 明确区分三类计数：`realtime_count` 是包含实时增量的估计，`modify_count` 是 ANALYZE 后的修改量，`analyze_count` 是 ANALYZE 所得行数。`Determinate` 模式只在返回值副本上用 `analyze_count.max(0)` 覆盖前两者；`pkg/planner/core/stats/stats_aster_unit_test.rs:determinate_objective_uses_analyze_count_without_mutating_source` 证明数据源中原快照保持不变。

`pseudo` 与 `allow_pseudo_loading` 是不同维度：前者表示优化器应把该快照视为伪统计，后者仅表示这种伪统计能否触发统计加载。未初始化/过期分支只设置 `pseudo`，不会重建为默认 `10_000` 行；无数据源、数据源无记录和零行数分支则调用 `StatisticsTable::pseudo`。

`load_table_stats` 的 `record` 是调用者持有的语句级状态。以 `physical_id` 为唯一键可保证同一物理对象在一次记录生命周期内只加载一次；`UsedStatsInfo.table` 保存 `TableInfo` 副本，因此记录不借用调用者的表对象。

## 依赖与调用关系

直接 Rust 依赖只有 `std::collections::BTreeMap`。外部能力全部收束在 `StatsSource`：

- `table_info_for_used_stats -> StatsSource::table_by_physical_id`；
- `get_stats_table -> StatsSource::physical_stats`；
- `load_table_stats -> get_stats_table`，并实例化 `UsedStatsInfo`、引用 `PSEUDO_VERSION`。

RustCodeGraph 对 `get_stats_table` 的 callees 给出 `physical_stats`，对 `load_table_stats` 给出 `get_stats_table`、`UsedStatsInfo` 和 `PSEUDO_VERSION`；未找到测试外的 Rust callers。`pkg/planner/core/stats/lib.rs` 只公开 `pub mod stats`，并在 `cfg(test)` 下装入独立测试模块。

Go 生产调用关系用于说明计划中的真实位置，而不代表 Rust 已接线：

- `pkg/planner/core/stats.go:initStats -> stats.GetStatsTable`，随后生成 `property.StatsInfo` 并记录 UsedStats；
- `PhysicalIndexReader::LoadTableStats`、`PhysicalTableReader::LoadTableStats`、`PointGetPlan::LoadTableStats`、`BatchPointGetPlan::LoadTableStats`、`PhysicalIndexLookUpReader::LoadTableStats`、`PhysicalIndexMergeReader::LoadTableStats` 均调用 Go 的 `stats.LoadTableStats`；
- `pkg/planner/cardinality/trace.go` 调用 Go 的 `GetTblInfoForUsedStatsByPhysicalID` 生成追踪名称。

Cargo 边界由 `pkg/planner/core/stats/Cargo.toml` 定义。根 `Cargo.toml` 把它注册为 workspace member 和 facade 依赖，`pkg/lib.rs` 再导出 facade。该 crate 自身没有 feature；条件依赖表列出 domain、infoschema、model、sessionctx、statistics 等 Go 对照所需组件，但当前 `stats.rs` 没有引用这些 crate。

## 错误处理与边界

API 不返回 `Result`，缺失通过 `Option` 和伪统计降级表达：元数据缺失产生 `tableID {id}`；统计源或指定统计缺失产生不可加载伪表。实现不会 panic，也没有索引访问；分区名称通过迭代器查找，找不到时退回表名。

需要保持的边界包括：

- `analyze_count < 0` 在确定性模式中按 0 处理；默认模式不改写该字段。
- 动态分区裁剪强制读取表 ID 的全局统计；关闭时才允许按分区物理 ID 读取。
- 仅在“原来是真实非零统计，但 ANALYZE 行数为零”的确定性转换中允许伪表触发加载。
- `pseudo_for_outdated == false` 时，单独的 `outdated` 不会令快照变伪；`initialized == false` 不受该开关控制。
- 未初始化或过期所产生的伪标记保留原版本；只有写入 `UsedStatsInfo` 时版本才归一为 0。
- 重复 `physical_id` 使 `load_table_stats` 完全短路，后续数据源变化不会覆盖已有记录。

与 Go 版本相比，Rust 缺少 `intest.Assert(statsTbl.ColAndIdxExistenceMap != nil)`、伪统计指标递增以及真实 `statistics.Table` 的列/索引存在性映射。这些是明确的迁移差异，不能从当前 Rust API 推断为已支持。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、文件或网络资源。函数均为同步调用，资源生命周期由借用关系和按值返回控制。

`StatsSource` 只要求 `&self`，但没有 `Send`、`Sync` 或线程安全约束；实现者可自行使用内部可变性。测试实现以 `RefCell<Vec<i64>>` 记录请求 ID，进一步说明当前 trait 不能被视为天然线程安全。若未来把它放入并行规划或跨线程缓存，必须在上层增加适当的 `Send + Sync` 边界或调整 trait，而不能依赖本文件现有签名。

`get_stats_table` 从数据源取得拥有所有权的 `StatisticsTable`，因此确定性模式对其字段的修改不会回写共享缓存。`load_table_stats` 对 `&mut BTreeMap` 的独占借用保证单次调用期间的插入安全，但它不是多线程共享容器；语句结束后的记录释放由调用者负责。

## 与 Go 版本的对应关系

Rust 三个公开函数分别对应 `pkg/planner/core/stats/stats.go` 的 `GetTblInfoForUsedStatsByPhysicalID`、`GetStatsTable` 和 `LoadTableStats`，整体分支顺序与核心语义保持一致：物理 ID 名称规则、动态分区裁剪选择全局统计、确定性目标忽略实时修改、零行数/未初始化/过期时使用伪统计、UsedStats 去重及伪版本归一化。

主要建模差异如下：

- Go 从 `PlanContext`/`sessionctx.Context` 取得 InfoSchema、Domain、StatsHandle、会话变量和 StmtCtx；Rust 把这些输入显式拆成 `StatsSource`、布尔值、`OptimizationObjective` 与外部 `BTreeMap`。
- Go 操作完整的 `model.TableInfo`、`statistics.Table` 和 `stmtctx.UsedStatsInfoForTable`；Rust 使用局部最小结构，未携带直方图、列/索引状态或其他统计载荷。
- Go 在确定性模式需要 `CopyAs(statistics.MetaOnly)` 避免修改缓存；Rust 的 `physical_stats` 已返回拥有所有权的值，因此直接修改仍保持同一不变量。
- Go 的 `statistics.PseudoTable(tblInfo, ..., true)` 接收表元数据并执行完整伪表构造；Rust `StatisticsTable::pseudo` 使用固定的 `10_000` 行简化模型，没有表级派生逻辑。
- Go 会递增 `PseudoEstimationNotAvailable` / `PseudoEstimationOutdate` 指标，并断言列索引存在性映射非空；Rust 当前没有对应状态和副作用。
- Go 生产调用已经覆盖逻辑数据源和多类物理读计划；Rust 当前只在 `stats_aster_unit_test.rs` 中被调用，因此属于局部行为移植而非端到端替代。

这些差异意味着后续对齐应优先复用真实 Rust domain/statistics/session context，而不是继续扩充本地影子模型；在接线完成前，文档和调用者都应明确其独立移植状态。

## 扩展指南

修改或扩展时应按职责选择入口：

- 改变伪统计触发条件、确定性模式或分区路由：修改 `get_stats_table`，并同步 `stats_table_routes_global_and_dynamic_partition_requests_like_go`、`determinate_objective_uses_analyze_count_without_mutating_source`、`zero_analyze_count_returns_loadable_pseudo_table_for_real_stats`、`missing_handle_and_zero_rows_return_non_loadable_pseudo_table`、`initialization_and_outdated_switches_match_go_pseudo_rules`。
- 改变表/分区显示名：修改 `table_info_for_used_stats`，同步 `used_stats_name_matches_go_table_partition_global_and_missing_cases`，同时核对 Go 的 `GetTblInfoForUsedStatsByPhysicalID` 和 `pkg/planner/cardinality/trace.go`。
- 扩充语句级记录字段或去重策略：修改 `UsedStatsInfo` / `load_table_stats`，同步两个 `load_table_stats_*` 测试，并核对 Go 的 `stmtctx.UsedStatsInfoForTable` 赋值。
- 接入真实统计系统：实现或替换 `StatsSource` 边界，并在生产规划调用点接线；同时考虑 Go 版本中的指标、存在性断言、完整伪表构造和 UsedStats 生命周期，不能只让现有单元测试通过。

兼容风险集中在布尔参数顺序、物理 ID 路由和伪版本语义；性能风险主要是未来真实 `TableInfo`/统计对象的克隆成本，以及每次名称解析或记录时线性扫描 `partitions`。若分区数量增大或函数进入热点，可在上游提供 ID 索引，但必须保持缺失回退与名称规则。新增 Rust 测试应继续放在独立的 `pkg/planner/core/stats/stats_aster_unit_test.rs`，不要内嵌到生产文件。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/planner/core/stats/stats.rs`（RustCodeGraph `node --file ... --offset 1 --limit 400` 完整读取 349 行）。
- 模块入口与测试装配：`pkg/planner/core/stats/lib.rs`。
- crate 边界：`pkg/planner/core/stats/Cargo.toml`；workspace 注册和 facade：根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/planner/core/stats/stats.go`；生产调用位置包括 `pkg/planner/core/stats.go`、`pkg/planner/core/operator/physicalop/physical_index_reader.go`、`physical_table_reader.go`、`physical_batch_point_get.go`、`physical_indexlookup_reader.go`、`physical_indexmerge_reader.go`，以及 `pkg/planner/cardinality/trace.go`。
- 独立 Rust 回归测试：`pkg/planner/core/stats/stats_aster_unit_test.rs`，覆盖名称四种情况、全局/分区路由、确定性副本语义、可加载伪表、缺失与零行数、初始化/过期开关、UsedStats 去重和伪版本。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/planner/core/stats` 列出实现、入口、Go 对照和独立测试；精确 `query/callers/callees` 确认 `get_stats_table -> StatsSource::physical_stats`、`load_table_stats -> get_stats_table`，且没有测试外 Rust caller。
- 文本交叉检查：`rg` 只在目标实现与 `stats_aster_unit_test.rs` 中找到 Rust 三个入口的使用，并定位上述 Go 生产调用者。

本任务按约束未运行 Cargo 或代码测试。结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级章节；除此之外还人工复核了固定章节顺序、源码链接、当前未接线事实和独立测试路径。

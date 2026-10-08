# `pkg/planner/core/operator/physicalop/physical_insert.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`（`pkg/planner/core/operator/physicalop/Cargo.toml`），由 `lib.rs` 的 `mod physical_insert` 纳入并以 `pub use physical_insert::*` 从 crate 根导出。它定义 INSERT/REPLACE 的语句级物理计划 `Insert`，以及该计划直接依赖的目标表适配、生成列状态和外键检查/级联节点。它位于“AST 已完成目标表解析、表达式改写，执行器尚未真正写行”的边界：`pkg/planner/core/planbuilder_runtime.rs` 负责构造并填充计划，本文件负责保存和校验可执行规划状态，不实现 INSERT 行写入算法。

该文件不是普通的、具有 `children()` 的关系算子。`Insert` 通过 `base::Plan` 暴露空输出 Schema 和计划元数据；`SelectPlan`、`FKChecks`、`FKCascades` 则由 `pkg/planner/core/flat_plan.rs` 特别识别和遍历。文件已在源码首部带有 `// Copyright 2026 AsterSQL.`，表明其 Rust 移植逻辑已经过处理。

## 核心职责

1. `NewInsertTargetTable` 把 `model::TableInfo` 解析成 `Arc<dyn table::Table>`。它先调用 `table::BuildTableFromMeta` 获取宿主安装的真实表；构造器不可用时退回只支持规划元数据读取的 `MetadataTableAdapter`。若 SQL 有 `PARTITION` 子句，它把分区名解析为物理 ID，并用 `SelectedPartitionTable` 限制允许的分区集合。
2. `Insert` 保存目标表、目标 Schema/列名、VALUES 行、INSERT ... SELECT 子计划、ON DUPLICATE 赋值、生成列表达式、语义标志及外键子计划，供后续计划遍历、缓存克隆和执行器构建消费。
3. `Insert::ResolveIndices` 在 builder 已经填好 Schema 后，将 ON DUPLICATE 和生成列表达式中的逻辑列重新绑定为具体列下标；没有相关表达式时不强制要求两个可选 Schema 已初始化。
4. `InsertGeneratedColumns::{CloneForPlanCache, MemoryUsage}` 与 `Insert::clone_for_plan_cache` 提供深克隆和内存估算。外键检查或级联存在时，计划明确不可缓存。
5. `FKCheck`、`FKCascade` 和 `FKCascadeType` 保存外键检查/级联的物理计划描述，并通过 `impl_foreign_key_plan!` 成为可被扁平计划遍历器观察的 `base::Plan` 节点。

## 主要符号

- `EMPTY_INSERT_STATS: LazyLock<property::StatsInfo>`：当 `SimpleSchemaProducer.Plan.StatsInfo()` 未设置时，为 `Insert::stats_info` 提供进程级只读默认引用。
- `MetadataTableAdapter { meta, columns }`：从 `TableInfo` 克隆元数据并构造 `table::Column` 列表。列查询和表元数据查询可用；索引/约束为空；`AddRecord`、`UpdateRecord`、`RemoveRecord` 一律返回 `table::ErrUnsupportedOp`，防止规划期后备对象被误当成可写存储表。
- `SelectedPartitionTable { source, allowed }`：私有分区限制包装器。一般 `table::Table` 操作转发给 `source`；`table::PartitionedTable` 方法只暴露 `allowed` 中的物理分区，并在按行路由越界时返回 `ErrRowDoesNotMatchGivenPartitionSet`。
- `NewInsertTargetTable(meta, partition_names)`：公开目标表工厂。无分区名时直接返回目标；有分区名时验证表的分区元数据、每个名字及可执行分区接口，再返回限制包装器。
- `InsertGeneratedColumns { Exprs, OnDuplicates }`：分别保存普通插入生成列表达式和 ON DUPLICATE 路径的生成列赋值；`Clone` 委托 `CloneForPlanCache`，不会共享表达式树。
- `FKCheck`：保存本表或被引用外键、目标表/索引、参与列、唯一性/主键属性、检查方向和延迟失败错误。`FKCheck::New` 仅建立 `TypeForeignKeyCheck` 基础计划并把业务字段初始化为空。
- `FKCascadeType::{OnDelete, OnUpdate}` 与 `FKCascade`：描述父行删除/更新触发的子表级联；`CascadePlans` 在当前结构中可保存执行期产生的后续计划。
- `impl_foreign_key_plan!`：为两类外键节点转发 `base::Plan` 元数据 API。其 `clone_for_plan_cache` 固定返回 `(None, false)`，单个外键节点本身不可缓存。
- `Insert`：文件主类型。公开字段是 builder 与消费者之间的结构化契约；`New`、`ResolveIndices`、`MemoryUsage` 是主要固有方法，`HasForeignKeyPlans` 是 crate 内缓存门禁。
- `impl base::Plan for Insert`：提供向下转型、空输出 Schema、ID/类型/上下文/输出名等转发，并实现计划缓存克隆。

## 执行流程

以 `pkg/planner/core/planbuilder_runtime.rs` 的 INSERT 构建路径为入口，主流程如下：

1. builder 从 INSERT 的 `TableSource` 解析数据库与表，拒绝派生查询源、未选择数据库、视图和序列，并由 `TableInfo2SchemaAndNames` 建立目标 Schema/列名。
2. builder 调用 `NewInsertTargetTable(table_info, statement.PartitionNames)`。工厂先执行 `BuildTableFromMeta`；没有运行时表构造器时使用 `MetadataTableAdapter`。有 `PARTITION` 子句时，按 `CIStr.L` 大小写不敏感名查找 `PartitionInfo.Definitions`，收集物理 ID，再确认目标实现 `PartitionedTable`。
3. VALUES 路径逐行改写表达式并校验列数/生成列限制，然后调用 `Insert::New`，填入 `Table`、`TableSchema`、`TableColNames`、`Columns`、`Lists` 和标志，最后调用 `ResolveIndices`。INSERT ... SELECT 路径同样建立 `Insert`，另行填入 `SelectPlan`、ON DUPLICATE 与生成列状态，并在收尾再次调用 `ResolveIndices`。
4. `ResolveIndices` 先解析 `SimpleSchemaProducer`。只有存在 ON DUPLICATE 或生成列表达式时才读取 `TableSchema`；只有 ON DUPLICATE 相关表达式存在时才读取 `Schema4OnDuplicate`。赋值左侧列对目标表 Schema 解析，右侧表达式对 duplicate Schema 解析；普通生成列表达式对目标表 Schema 解析。任一步错误立即向上传播。
5. 后续遍历中，`flat_plan.rs` 在普通物理孩子之后依序加入 `SelectPlan`、`FKChecks`、`FKCascades`；遇到 `FKCascade` 时还继续展开 `CascadePlans`。这解释了为什么外键节点实现 `Plan` 而不是作为 `PhysicalPlan` 普通孩子保存。
6. 进入计划缓存时，`Insert::clone_for_plan_cache` 首先拒绝非空外键向量；随后克隆 SELECT 子计划，深克隆 VALUES 表达式、赋值和生成列表达式，复制标志并共享 `Arc<Table>`。独立的 `cache_snapshot.rs` 更严格：除外键外，带会话绑定 `Table` 的 Insert 也拒绝捕获，恢复后 `Table` 为 `None`，等待运行时重新绑定。

## 数据与状态

`Insert::New` 将 `SimpleSchemaProducer` 初始化为 `TypeInsert`，并显式设置空输出 Schema；因此 INSERT 是语句计划而不是返回关系行的算子。目标表与两个输入 Schema 初始均为 `None`，由 DML builder 延迟填充。`RowLen` 记录目标行宽；`IsReplace`、`IgnoreErr`、`NeedFillDefaultValue`、`AllAssignmentsAreConstant` 控制执行语义或优化判定，但本文件只保存这些状态。

表达式所有权是深层的：`Lists`、`OnDuplicate` 与 `GenCols` 持有 boxed 表达式/赋值；缓存克隆通过 `CloneExpr` 或 `Assignment::Clone` 建立独立树。`Table` 及外键表/索引使用 `Arc` 共享动态 trait 对象，表元数据适配器自身克隆 `TableInfo` 和列包装。`SelectedPartitionTable.allowed` 是 `HashSet<i64>`，重复分区名自然合并；`GetAllPartitionIDs` 保留底层枚举顺序，只过滤未获准 ID。

`MemoryUsage` 是规划期估算而非完整堆快照：它统计 schema、VALUES/赋值/生成列表达式、SELECT 子计划以及外键基础计划的内存，但没有显式遍历列名、AST `Columns`、表 trait 对象内部数据、`FKCheck`/`FKCascade` 的所有业务字段。使用者不应把该数值解释为进程实际占用的精确值。

## 依赖与调用关系

上游直接证据：

- `pkg/planner/core/planbuilder_runtime.rs` 调用 `NewInsertTargetTable`，在 VALUES 与 SELECT 两条路径构造 `physicalop::Insert::New`，填充字段并调用 `ResolveIndices`。
- `pkg/planner/core/expression_rewriter.rs` 持有 `&physicalop::Insert`，说明 ON DUPLICATE/INSERT 表达式改写会读取此计划上下文。
- `pkg/planner/core/flat_plan.rs` 向下转型 `Insert` 并遍历 SELECT、外键检查和级联；`pkg/executor/statement_ru_result.rs` 也通过向下转型识别 INSERT 语句计划。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs` 调用 `HasForeignKeyPlans`，捕获/恢复 Insert 的可缓存状态。

下游依赖由 `Cargo.toml` 明确提供：`base`/本 crate 的 `SimpleSchemaProducer` 提供计划接口，`expression` 提供 Schema、Column、Assignment、表达式克隆/索引解析，`model` 和 `parser_ast` 提供表/分区/外键及 SQL 名字元数据，`table`/`kv` 提供目标表、分区、事务与 handle 抽象，`property`/`plancodec` 提供统计与计划类型，`types` 提供行 datum。该 manifest 没有本文件专属 feature gate，crate 设置 `autotests = false`，测试由 `lib.rs` 中的 `#[cfg(test)] mod physical_insert_test` 和 `physical_insert_aster_unit_test` 独立接入。

RustCodeGraph 将目标文件索引为 89 个符号节点，并报告被 `planbuilder_runtime.rs`、`flat_plan.rs`、`expression_rewriter.rs` 等文件使用。由于 `ResolveIndices`、`MemoryUsage` 等名称在仓库中高度重载，通用 callers/callees 查询未产出可可靠消歧的边；上述跨文件关系因此由路径限定搜索和直接源码片段复核，而不是把同名图结果当成目标方法的调用者。

## 错误处理与边界

- `NewInsertTargetTable` 传播 `BuildTableFromMeta` 的 `table::TableResult`（可转换为 `expression::Error`）。在非分区表使用 `PARTITION`、分区名未知、或元数据声称分区但运行时表没有 `PartitionedTable` 接口时，它分别返回明确的规划错误。
- `SelectedPartitionTable::GetPartitionByRow` 传播底层路由错误；路由所得物理 ID 不在允许集合时返回 `ErrRowDoesNotMatchGivenPartitionSet`。`GetPartition` 对未允许/不存在的 ID 返回 `None`。包装器内部对 `source.GetPartitionedTable()` 使用 `expect`，其不变量由 `NewInsertTargetTable` 创建包装器前的检查保证；绕过工厂无法构造该私有类型。
- `MetadataTableAdapter` 只适合缺少宿主表构造器的规划嵌入环境。其写操作显式失败，索引和约束也为空；不能据此声称独立 planner 后备对象可执行真实 INSERT。
- `ResolveIndices` 缺少所需 Schema 时返回 `expression::Error`，但没有相关表达式的空 Insert 可成功解析，这一延迟要求由 `physical_insert_test.rs` 固化。当前 Rust 代码总是解析 ON DUPLICATE 的右值；Go 版本允许 `Assignment.Expr == nil` 表示已有 `LazyErr`。若 Rust 的 `Assignment` 将来支持同样的空右值，需同步调整这里，不能直接假设现状已覆盖 Go 的延迟错误分支。
- SELECT 子计划克隆失败或存在外键计划时，缓存克隆以 `(None, false)` 表示不可缓存而非抛错。`stats_info` 未设置时返回共享空统计；它不会伪造行数估计。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道或锁。`EMPTY_INSERT_STATS` 由 `LazyLock` 线程安全地初始化一次，之后只以共享引用读取。动态表、索引与元数据对象通过 `Arc` 跨计划组件共享；缓存克隆只增加表对象引用计数，同时深克隆可变表达式树，避免缓存实例之间共享表达式变更。

`AddRecord`/`UpdateRecord`/`RemoveRecord` 接收的 `&mut dyn kv::Transaction` 仅在调用期间借用：`SelectedPartitionTable` 原样转发，`MetadataTableAdapter` 不保存也不操作事务。因此事务提交、回滚、锁和 allocator 生命周期均由执行器及真实 `table::Table` 实现负责。`FKCascade.CascadePlans` 是拥有型 `Vec<Box<dyn Plan>>`；其注释和 Go 对照表明这些孩子在执行期间填充，而扁平遍历器负责在存在时观察它们。

## 与 Go 版本的对应关系

主要 Go 基准分散在 `physical_common_plans.go`、`foreign_key.go`、`plan_clone_generated.go` 及 `planbuilder.go`：

- Rust `InsertGeneratedColumns` 与 Go 同名类型字段一致，克隆均深拷贝表达式和赋值；Rust `MemoryUsage` 累加内容对象，但未复刻 Go 对 slice 头/capacity 的固定开销。
- Rust `Insert` 基本逐字段对应 Go `Insert`，`New` 对应 Go 的 `Init`。Rust 用 `Option` 表示 Go 的 nil interface/pointer，用 `Vec` 表示 slice，用 `isize` 表示 Go `int`。Rust 构造器额外保证输出 Schema 为空。
- 两端 `ResolveIndices` 都先处理 schema producer，再分别以目标表 Schema 和 duplicate Schema 解析左/右表达式。Go 对 ON DUPLICATE 的 nil 右值有 `LazyErr` 特判；Rust 的当前类型和实现没有该分支，这是需要保留关注的语义差异。
- Go `CloneForPlanCache` 要求 `FKChecks`、`FKCascades` 为 nil；Rust 用“两个向量均为空”表达同一可缓存状态，并把成功克隆后的外键向量固定为空。Rust 的 cache snapshot 另有“运行时 Table 必须为空”限制，不应与 `base::Plan::clone_for_plan_cache` 混为一种机制。
- `FKCheck`、`FKCascade`、`FKCascadeType` 的核心字段和 OnDelete=1/OnUpdate=2 判别值对应 Go `foreign_key.go`。但 Go 还实现 `AccessObject`、`OperatorInfo`、详细 `ExplainInfo` 及更专门的内存统计；Rust 当前只通过基础计划转发这些 Plan 方法，因此 explain 信息和内存口径没有完整移植。
- `SelectedPartitionTable` 对应 Go builder 使用的 `tables.NewPartitionTableWithGivenSets`。Rust 将这个限制器内聚到目标表工厂；`MetadataTableAdapter` 则是 Rust 为独立嵌入 planner 增加的后备层，不是 Go `Insert` 结构的一部分。

## 扩展指南

- 新增 INSERT 状态字段时，应同时更新 `Insert::New`、`MemoryUsage`、`clone_for_plan_cache`，以及 `cache_snapshot.rs` 的 `CachedInsert::{capture, restore}`；再核对 Go `physical_common_plans.go` 和计划克隆生成规则，避免缓存恢复遗漏或错误共享。
- 修改表达式绑定时，入口是 `Insert::ResolveIndices`。必须分别明确左侧目标列、普通生成列和 ON DUPLICATE 右值使用哪个 Schema，并在独立的 `physical_insert_test.rs` 中覆盖“无表达式无需 Schema”、缺少各 Schema、解析失败传播和延迟错误语义；不要把测试写回生产 `.rs`。
- 扩展分区限制时，应通过 `NewInsertTargetTable` 保持 `SelectedPartitionTable` 的构造不变量，并补充独立测试覆盖非分区表、未知名字、重复名字、按 ID/按行越界以及底层路由错误。还需与 Go `NewPartitionTableWithGivenSets` 的错误类别和名字规范保持一致。
- 接入真实写执行前，必须区分 `MetadataTableAdapter` 与真实表。若需要改变后备行为，应优先完善 `table::BuildTableFromMeta` 的运行时安装边界，而不是让规划用适配器静默写入；这关系到事务、索引、分区和 allocator 正确性。
- 扩展外键 explain、访问对象或内存统计时，应修改 `FKCheck`/`FKCascade` 或 `impl_foreign_key_plan!`，并同步 `flat_plan.rs` 和缓存拒绝测试。不能仅让它们实现普通物理孩子，因为 Go/Rust 当前都把这些节点作为 DML 特殊孩子处理。
- 性能风险集中在大批 VALUES 的表达式深克隆/内存遍历、分区名线性查找以及每行分区路由；兼容性风险集中在 Go 的 nil/空 slice、`LazyErr`、错误码/错误文本和 explain 输出差异。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点，目标文件已索引且含 89 个符号；执行了目标目录 `files`、目标文件两段 `node --file`、`NewInsertTargetTable`/`ResolveIndices`/`MemoryUsage`/`HasForeignKeyPlans` 精确查询，以及 callers/callees 尝试。图查询对重载方法未给出可消歧调用边，故未把模糊结果作为事实。
- 已读 Rust 源与接线：`physical_insert.rs`、`physicalop/lib.rs`、`physicalop/Cargo.toml`、`planbuilder_runtime.rs`、`flat_plan.rs`、`cache_snapshot.rs`；路径限定搜索还核对了 `expression_rewriter.rs`、`statement_ru_result.rs` 和 `table/table.rs` 的直接引用/定义位置。
- 已读独立 Rust 测试：`physical_insert_test.rs` 验证无表达式时不需要 builder Schema；`physical_insert_aster_unit_test.rs` 验证 Schema/Table/SelectPlan 的延迟初始化、生成列深克隆和 Insert 对精确外键节点类型的所有权。`cache_snapshot_test.rs` 的引用位置也通过搜索确认，但本任务未逐段复述其全部测试。
- 已读 Go 对照：`physical_common_plans.go` 的生成列、Insert、内存与索引解析，`foreign_key.go` 的外键节点/类型/说明和内存，`plan_clone_generated.go` 的 Insert 缓存克隆，以及 `planbuilder.go` 使用 `NewPartitionTableWithGivenSets` 的目标表接线。
- 人工复核结论：本文区分了规划状态与真正写入、真实表与元数据后备表、普通物理孩子与特殊 DML 子节点，也明确列出了未完整移植的 Go 细节；没有把未见到的执行器接线或测试结果写成“已支持”。本任务是纯文档分析，按计划未运行 Cargo。

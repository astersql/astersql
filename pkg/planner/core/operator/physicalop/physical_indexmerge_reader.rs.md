# `pkg/planner/core/operator/physicalop/physical_indexmerge_reader.rs`

源码：[physical_indexmerge_reader.rs](./physical_indexmerge_reader.rs)；Go 对照：[physical_indexmerge_reader.go](./physical_indexmerge_reader.go)；独立测试：[physical_indexmerge_reader_test.rs](./physical_indexmerge_reader_test.rs)。

## 文件定位

本文件定义 Rust 规划器中的 `PhysicalIndexMergeReader`，即用多条 partial 访问路径读取同一张表、再按 union 或 intersection 语义合并 handle 的物理读算子。它属于 `astersql-planner-core-operator-physicalop` crate；`Cargo.toml` 将该 crate 的入口设为同目录 `lib.rs`，后者声明 `mod physical_indexmerge_reader`、公开再导出本文件内容，并通过 `impl ConcretePhysicalOperator for PhysicalIndexMergeReader` 与 `impl_concrete_physical_plan!` 把类型接入 `base::PhysicalPlan` 的统一接口。

在物理计划树中，`PartialPlansRaw` 是索引合并的构建侧路径，`TablePlan`（若存在）是根据合并后的 handle 回表取行的探测侧路径。`pkg/planner/core/flat_plan.rs` 据此为 partial 子节点标记 `BuildSide`，并将最后的表侧子节点标记为 `ProbeSide`。本文件保存、初始化和遍历这些子计划，但不实现执行器中的并行扫描或 handle 去重/求交算法。

## 核心职责

- `PhysicalIndexMergeReader` 保存合并方式、MVIndex 标志、下推 Limit、排序项、partial 子计划、可选表计划、分区信息和保序标志。
- `New` 建立 `TypeIndexMerge` 类型的空算子；`Init` 绑定上下文和查询块偏移，并从表计划或首条 partial 路径推导统计信息与输出 Schema。
- `Clone` 为新的 `PlanContext` 深克隆 Schema、排序项、所有子计划和分区信息。
- `ExtractCorrelatedCols`、`ResolveIndices`、`ToPB`、`MemoryUsage` 分别承担关联列汇总、列下标解析、PB 转换委托和内存核算。
- `ExplainInfo`、`ExplainNormalizedInfo` 和 `AccessObject` 提供可解释信息；两个代价入口当前仅转发给 `BasePhysicalPlan`。

该实现不是 Go 版本的完整等价移植。尤其是平均行宽、代价、分区访问对象、统计预加载和 PB 构造仍为简化或占位行为；调用者不能据 Go 文件推断这些能力在 Rust 中已经完整存在。

## 主要符号

### `PhysicalIndexMergeReader`

唯一公开结构体，内嵌 `PhysicalSchemaProducer`，并公开以下状态：

- `IsIntersectionType`：`true` 表示 intersection，否则为 union。
- `AccessMVIndex`：记录是否访问多值索引；本文件只保存和克隆该标志。
- `PushedLimit: Option<PushedDownLimit>`：用于 EXPLAIN 展示的内嵌 offset/count。
- `ByItems`：保序或排序描述；本文件不根据 `KeepOrder` 自动从扫描节点填充它。
- `PartialPlansRaw`：所有 partial 物理子计划的拥有型 trait object。
- `TablePlan`：可选回表子计划；缺省时按 index-only 形态初始化输出。
- `PlanPartInfo`：可选分区剪枝数据。
- `KeepOrder`：保序要求；本文件仅保存/克隆，不实施排序。

### 构造和复制

- `New(ctx) -> Self`：创建 `BasePhysicalPlan::New(ctx, TypeIndexMerge, 0)`，其余字段取空值或 `false`。
- `Init(self, ctx, offset) -> Self`：重设上下文、类型和 query block offset。若存在 `TablePlan`，复制其统计和 Schema；否则在至少一条 partial 路径存在时，将各路径 `stats_count()` 求和，以首路径统计调用 `ScaleByExpectCnt`，保留首路径 `StatsVersion`，再从首路径 flatten 后的根扫描取得 Schema。根必须是 `PhysicalIndexScan` 或 `PhysicalTableScan`，否则 panic。
- `Clone(&self, new_ctx) -> Result<Self, expression::Error>`：克隆基类、Schema、`ByItems`、partial/table 子计划及分区信息；子计划克隆错误直接传播。

### 查询、计划接口和资源核算

- `GetAvgTableRowSize`：表计划存在时返回 `schema().Len() * 8`，否则为零。
- `GetPartialReaderNetDataSize`：返回 `stats_count * schema 列数 * 8`。
- `AccessObject`：只返回 `index merge type:intersection|union` 字符串。
- `ExplainInfo`：输出合并类型，若有 `PushedLimit` 再附加 `limit embedded(offset:..., count:...)`。
- `ExplainNormalizedInfo`：固定返回空字符串。
- `ExtractCorrelatedCols`：遍历表计划 flatten 结果；对每条 partial 同时读取根自身和 flatten 结果的关联列。
- `ResolveIndices`：依次解析自身 Schema producer、每条 partial 计划和可选表计划，首个错误即返回。
- `LoadTableStats`：当前为空函数。
- `GetPlanCostVer1` / `GetPlanCostVer2`：委托 `BasePhysicalPlan`。
- `ToPB`：优先选择 `TablePlan`，否则选择首条 partial，直接调用该子计划的 `to_pb`；没有子计划时返回 `index merge has no child plan`。
- `MemoryUsage`：累加 Schema producer、所有 raw partial 根及其 flatten 结果、表计划根及其 flatten 结果、分区信息的内存用量。

## 执行流程

1. 规划阶段用 `New` 建立类型为 `TypeIndexMerge` 的空节点，再由上层填充 partial 路径、可选表路径及合并标志。
2. `Init` 写入实际上下文和 query block offset。存在表计划时，表计划决定统计与输出 Schema；否则各 partial 行数之和决定期望行数，首条 partial 的统计版本与扫描 Schema 成为基准。
3. index-only 分支调用 `FlattenListPushDownPlan(first_partial)` 并检查第一项：索引扫描优先采用 `DataSourceSchema`，缺失时退回扫描自身 Schema；表扫描直接采用其 Schema。任何其他根类型触发 panic。
4. `lib.rs` 的 `ConcretePhysicalOperator` 实现将 `PartialPlansRaw` 后接 `TablePlan` 暴露为 children，并把 explain、resolve、memory、correlated、cost 和 PB 方法转发到本文件。
5. `pkg/planner/core/flat_plan.rs` 展平计划树时，将 partial 子计划标为 BuildSide，将存在的最后一个表计划标为 ProbeSide，用于后续扁平计划解释与代价上下文传播。
6. 计划缓存路径通过 `cache_snapshot.rs` 的 `CachedIndexMergeReader::capture/restore` 保存并恢复本结构体的全部现有字段；恢复排序表达式和分区表达式时仍可能返回错误。

本文件到此只形成可供后续阶段消费的物理计划描述。真正的多路请求并发、handle 合并和回表执行不在本文件中，`ToPB` 目前也只是委托一个子节点，而非自行构造完整 IndexMerge executor。

## 数据与状态

结构体拥有 `Box<dyn PhysicalPlan>` 子计划，因此其克隆必须递归创建新的 trait object，而不能浅拷贝。`PhysicalSchemaProducer` 持有本节点的上下文、统计和输出 Schema；`Init` 的优先级不变量是“表计划优先，index-only 时首条 partial 提供类型和 Schema 基准”。

无表计划时，`Init` 允许 `PartialPlansRaw` 为空并保持初始统计/Schema；但只要进入首 partial 分支，flatten 结果必须非空，并且根节点必须是表扫描或索引扫描。多条 partial 的行数会相加，但统计分布、版本和 Schema 均以第一条路径为基准。

`MemoryUsage` 有意同时统计 raw 根和 flatten 后节点，与测试所固化的当前行为一致：仅放入一个可 flatten 为自身的表扫描时，扫描内存会被计算两次。该值是递归估算，不表示分配器的精确驻留字节数。

## 依赖与调用关系

直接依赖包括：

- `base::{ContextRef, Plan, PhysicalPlan}`：上下文、通用计划元数据和动态物理计划接口。
- `PhysicalSchemaProducer`、`BasePhysicalPlan`：Schema/统计持有者及通用物理计划实现。
- `PhysicalIndexScan`、`PhysicalTableScan`、`FlattenListPushDownPlan`：初始化 Schema、遍历下推链和资源核算。
- `expression::{CorrelatedColumn, Error}`：关联列结果和统一错误。
- `costusage::{CostVer2, PlanCostOption}`、`property::TaskType`：代价接口类型。
- `kv::StoreType`、`tipb::Executor`：PB 序列化边界。
- `planner_util::ByItems`、`PhysPlanPartInfo`、`PushedDownLimit`：排序、分区与下推限制状态。

RustCodeGraph 将本文件标为 21 个符号，并显示它被 `pkg/planner/core/flat_plan.rs`、`pkg/planner/core/operator/physicalop/cache_snapshot.rs`、`physical_utils.rs`、`pkg/executor/statement_ru_plan_walk.rs` 等 7 个文件使用。具体接线还由 `physicalop/lib.rs` 完成：模块被公开再导出，宏生成通用 `PhysicalPlan` 实现，children 顺序为全部 partial 后跟可选 table。直接测试调用 `New`、`Init`、`ExplainInfo` 和 `MemoryUsage`；缓存测试另行覆盖快照往返。

## 错误处理与边界

- `Clone` 会传播 `CloneWithNewCtx` 或任一子计划 `clone_physical` 的 `expression::Error`；已经构造的局部值随返回自动释放。
- `ResolveIndices` 严格按 producer、partial、table 的顺序执行，遇到首个错误立即停止，因此失败时可能已有较早节点完成原地解析，不具备事务式回滚。
- `ToPB` 在没有 table 且没有 partial 时返回显式错误；存在子计划时原样传播其 `to_pb` 错误。
- `Init` 不把非法 partial 根转换成可恢复错误：空 flatten 结果由 `expect` panic，非表/索引扫描根由显式 `panic!` 终止。调用方必须先满足 partial 路径形状不变量。
- `Init` 在完全无子计划时不会报错；这与 `ToPB` 在序列化阶段拒绝无子计划节点形成不同阶段的边界。
- `GetAvgTableRowSize` 和网络数据量以每列固定 8 字节近似，不考虑真实类型宽度、变长列或统计直方图，不能用于要求 Go 代价精度的判断。

## 并发与资源生命周期

本类型不含锁、原子、通道、后台任务或异步 future，也不直接执行扫描。它通过拥有型 `Box<dyn PhysicalPlan>` 管理子计划生命周期：父节点释放时子计划随之释放；`Clone` 创建独立子树，但共享的 `ContextRef` 仍按引用计数语义持有计划上下文。

并行 DistSQL 扫描属于下游执行和代价模型语义，本文件没有调度并发。`KeepOrder`、`ByItems` 及 intersection/union 标志只是计划状态；调用本文件方法不会建立排序缓冲区或 handle 集合。`MemoryUsage` 是同步递归遍历，若未来子计划图允许共享同一实例，需要重新审视重复计数；当前 children 以独占 `Box` 表达树形所有权。

## 与 Go 版本的对应关系

对应文件为 `pkg/planner/core/operator/physicalop/physical_indexmerge_reader.go`。字段和 explain 文本保留了主要命名与语义，但当前差异必须视为迁移状态而非等价实现：

- Go 同时缓存 `PartialPlans`、`TablePlans` 和 `HandleCols`；Rust 只保存 raw partial 与单个 table，并在需要时临时 flatten。
- Go `Init` 总会展平所有路径、从表扫描提取 handle、在 `KeepOrder` 时复制首扫描的 `ByItems`；Rust 未保存 flattened 列表或 handle，也未自动填充 `ByItems`。
- Go 的行宽估计调用 cardinality/statistics，区分索引扫描；Rust 固定按“列数 × 8”近似。
- Go v1/v2 代价分别接入 `utilfuncp` 的 IndexMerge 专用函数，其中 v2 汇总 table/index 侧子代价和网络代价并除以 DistSQL 并发；Rust 本文件把两个入口委托给基类。`pkg/planner/core/plan_cost_ver1.rs` 虽存在扁平 `PlanNode` 的 IndexMerge 代价函数，但本方法没有直接调用它。
- Go `LoadTableStats` 会从表扫描预加载统计；Rust 是空钩子。
- Go `AccessObject` 只在动态分区裁剪开启且有表路径时构造分区访问对象；Rust 仅返回合并类型字符串。
- Go `ResolveIndices` 还解析虚拟列及保序 handle；Rust 只递归解析 producer 和子计划。
- Go 本文件没有 `ToPB` 方法；其 flattened 路径由其他接线用于构造执行器。Rust 当前 `ToPB` 只序列化 table 或首 partial 子节点。
- Rust 新增 `Clone` 与空的 normalized explain，并通过 `cache_snapshot.rs` 提供独立的计划缓存快照机制。

因此，安全比较应逐个方法核对，不能仅凭同名结构体认定行为一致。

## 扩展指南

- 补齐初始化语义时，优先修改 `Init`，明确是否需要在结构体内持久化 flattened 路径、handle 列和首扫描排序项；同步扩展独立的 `physical_indexmerge_reader_test.rs`，覆盖 table/index-only、空 partial、非法根、`KeepOrder` 及多 partial 统计。
- 提升行宽或代价精度时，修改 `GetAvgTableRowSize`、`GetPartialReaderNetDataSize` 和两个 cost 方法，并对照 Go 的 cardinality、table/index 网络代价与 DistSQL 并发规则。需要避免把 `plan_cost_ver1.rs` 的扁平模型函数误当成本结构体已经接线的实现。
- 实现完整 PB 序列化时，从 `ToPB` 接入所有 partial、合并类型、表侧计划、Limit、排序和 MVIndex 信息，并增加“多 partial + 可选 table”的断言；不能继续只验证首子节点可序列化。
- 补齐分区和统计能力时，分别实现 `AccessObject` 与 `LoadTableStats`，同时验证动态/静态裁剪、别名、无表计划和统计加载失败边界。
- 改动 children 布局时，必须同步 `physicalop/lib.rs`、`flat_plan.rs`、`cache_snapshot.rs` 及其独立测试，因为它们依赖“partial 在前、table 在后”的顺序。
- 所有 Rust 测试应继续放在独立 `*_test.rs` 文件，不应嵌入生产源文件；行为移植需尽量保持 Go 分支和错误语义，不用桩或简化逻辑替代。

兼容性风险主要是 EXPLAIN 文本、children 顺序、Schema 来源和计划缓存快照格式；正确性风险集中在 index-only Schema、关联列重复收集、列下标解析与完整 PB 构造；性能风险集中在真实行宽、DistSQL 并发代价和 `MemoryUsage` 的重复计数。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter ...physical_indexmerge_reader.rs` 确认目标文件含 21 个符号；`node --file ... --offset 1 --limit 400` 返回完整 297 行源码并列出 7 个使用文件。对 `PhysicalIndexMergeReader`、`ExtractCorrelatedCols`、`ResolveIndices`、`ToPB`、`MemoryUsage` 执行了 `query/callers/callees`；名称歧义查询未返回可用的逐方法跨文件边，因此上游关系只采用文件级使用信息并由下列源码直接核验。
- 生产源码：`physical_indexmerge_reader.rs` 的结构体及全部方法；`physicalop/lib.rs` 的模块声明、公开再导出、`ConcretePhysicalOperator` 实现和宏接线；`pkg/planner/core/flat_plan.rs` 的 BuildSide/ProbeSide children 标记；`physicalop/cache_snapshot.rs` 的 capture/restore。
- crate 边界：`physicalop/Cargo.toml` 的 `lib.rs` 入口、路径依赖、`tipb` 依赖和 `package.metadata.porting.go-package`。
- Go 对照：`physical_indexmerge_reader.go` 的结构、初始化、统计行宽、代价、内存、分区访问、统计加载、explain 与列解析；`pkg/planner/core/plan_cost_ver2.go` 的 IndexMerge 专用 v2 代价；`pkg/planner/core/plan_cost_ver1.rs` 的 Rust 扁平计划代价实现。
- 独立测试：`physical_indexmerge_reader_test.rs` 验证 union/intersection 与 Limit 文本、normalized 空串、index-only Schema/统计版本初始化，以及 raw/flatten 双重内存计数；`cache_snapshot_test.rs` 覆盖 `PhysicalIndexMergeReader` 快照恢复；`readers_scans_aster_unit_test.rs` 验证类型满足物理计划接口。
- 本任务仅创建说明文档，按总计划不运行 Cargo；最终以固定 11 个二级标题的结构命令和人工事实复核验收。

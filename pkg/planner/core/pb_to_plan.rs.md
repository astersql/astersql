# `pkg/planner/core/pb_to_plan.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate。`pkg/planner/core/lib.rs` 以私有模块 `mod pb_to_plan` 装配它，再通过 `pub use pb_to_plan::*` 导出其公开符号；测试则由相邻且独立的 `pb_to_plan_test.rs` 在 `#[cfg(test)]` 下装配。它承担“序列化执行器描述到物理计划树”的边界转换：输入是本文件定义的轻量 `PBExecutor` 序列、表元数据和 key range，输出是 crate 内通用的 `PlanNode`/`PlanKind` 表示。

这里的 `PB*` 类型是 Rust 侧的简化传输模型，不是直接使用 `tipb::Executor` 的完整生产协议对象。文件目前只依赖 `crate::{PlanKind, PlanNode, PlannerContext}` 和标准库 `BTreeMap`；尽管 `Cargo.toml` 声明了 `tipb`、expression、infoschema、physicalop 等依赖，本文件没有直接使用它们。因此应把它理解为已接线、可测试但尚未完整等价于 Go 生产实现的局部移植，而不能据文件名推断其已覆盖完整 DAG protobuf 规划链。

## 核心职责

1. `NewPBPlanBuilder` 将表列表按 ID 建索引，并保存规划上下文与扫描范围。
2. `PBPlanBuilder::Build` 按“叶到根”的执行器顺序逐个转换并串成单子树物理计划，随后执行一次谓词下推改写。
3. `pbToPhysicalPlan` 将九类 `PBExecutor` 变体映射为 `PlanKind`，补充展示信息，并把前一轮构建的子树挂到当前节点。
4. `pbToTableScan` 校验表和列，限制入口只能扫描集群表，记录字段类型，并创建带数据库、列、方向及 range 数量描述的扫描节点。
5. `predicatePushDown` 只让 Selection 谓词到达具有已知 extractor 的四类集群内存表；Projection、TopN、Limit、Aggregation 等非 Selection 节点是上层谓词的边界。
6. `validateBroadcastQuery` 以规范化 token 白名单约束远端管理语句，防止任意 SQL 被包装成广播计划。

## 主要符号

- `PBColumnInfo { id, name, field_type }`：可克隆的列描述。`field_type` 暂用字符串保存；`pbToTableScan` 把所选列类型复制到 builder 的 `field_types`。
- `PBTableInfo { id, database, name, cluster_table, columns }`：转换所需的最小表元数据。`cluster_table` 是当前扫描准入条件。
- `PBExecutor`：输入算子枚举，包括 `TableScan`、`Selection`、`Projection`、`TopN`、`Limit`、`Aggregation`、`Kill`、`BroadcastQuery` 和显式错误分支 `Unsupported`。
- `PBPlanBuilder { sctx, field_types, tables, ranges }`：有状态构建器。`tables` 使用 `BTreeMap<i64, PBTableInfo>` 做 ID 查找；其他字段分别保存会话上下文、最近一次扫描的列类型和扫描范围。
- `NewPBPlanBuilder(...) -> PBPlanBuilder`：公开构造函数，消费表列表并建立 ID 索引。重复表 ID 会按 `collect` 的 map 语义由后出现者覆盖，代码没有单独报错。
- `Build(&mut self, &[PBExecutor]) -> Result<PlanNode, String>`：公开主入口；空列表报错，任一转换错误立即向上传播。
- `pbToPhysicalPlan`、`pbToTableScan`、`convertColumnInfo`、`validateBroadcastQuery`：私有转换与校验函数。
- `buildTableScanSchema`：公开辅助函数，按表定义顺序生成列名，且为请求中重复出现的同一列保留重复项。
- `predicatePushDown`：公开递归改写入口，返回“未被消费的谓词”和改写后的计划。
- `SessionContext`：只读暴露构建器持有的 `PlannerContext`，不转移所有权。

## 执行流程

`Build` 从 `source = None` 开始遍历执行器。每一项都交给 `pbToPhysicalPlan(executor, source)`：新节点成为根，旧 `source` 若存在则成为其唯一 child。因此输入必须按照 TableScan 等叶节点在前、Selection/Projection/聚合等上层节点在后的顺序排列。遍历结束后，空输入通过 `ok_or_else` 返回 `executor list is empty`；非空计划以空谓词调用 `predicatePushDown`，最终忽略顶层未消费谓词集合并返回改写树。

单算子转换的关键分支如下：

- `TableScan` 调用 `pbToTableScan`，先按 `table_id` 查表，再拒绝非集群表，随后按请求 ID 顺序解析列并更新 `field_types`。节点 `operator_info` 记录数据库、已解析列名、`desc` 和 range 数量；range 字节内容不在此处解释。
- `Selection` 将条件字符串原样放入 `PlanKind::Selection`。
- `Projection` 的表达式不进入 `PlanKind`，而是以逗号分隔写入 `operator_info`。
- `TopN` 和 `Limit` 分别保留 offset/limit；`Limit` 建树后复制第一个子节点的 `operator_info`，但不会像 Go 版本那样克隆 schema 或设置 slow-query row-limit hint。
- `Aggregation` 根据 `stream` 选择 `StreamAgg` 或普通 `Aggregation`，并把函数及 group-by 的文本摘要写入 `operator_info`。
- `Kill` 和通过白名单的 `BroadcastQuery` 被编码成 `PlanKind::Generic` 文本，而不是 Go 版本的可执行语句/包装计划对象。
- `Unsupported` 立即返回 `this exec type ... doesn't support yet`。

谓词下推遇到 Selection 时先保存调用者传入的 `parent_predicates`，再合并本节点条件并递归唯一 child。若 child 消费完合并谓词，Selection 被移除；否则用返回谓词重写该 Selection。遇到 TableScan 时，仅表名（忽略大小写）属于 `CLUSTER_SLOW_QUERY`、`CLUSTER_STATEMENTS_SUMMARY`、`CLUSTER_STATEMENTS_SUMMARY_HISTORY`、`CLUSTER_TIDB_INDEX_USAGE` 才视为有 extractor，并把谓词文本附加为 `pushed:[...]` 后返回空集合。其他扫描返回原谓词。遇到其他算子时只以空谓词递归其内部 child，明确阻止当前上层谓词穿越该算子。

## 数据与状态

构建器拥有传入的 `PlannerContext`、表元数据和 ranges。`NewPBPlanBuilder` 将 `Vec<PBTableInfo>` 消费为 `BTreeMap`，使扫描转换不依赖外部 infoschema 生命周期。`Build` 需要 `&mut self`，实际可变状态是 `pbToTableScan` 覆盖 `field_types`；如果一个列表含多个 TableScan，最终只保留最后一次成功转换的字段类型。本文件当前没有读取 `field_types` 的后续表达式解码逻辑，这与 Go 版本用 `tps` 解码 Selection、Projection、TopN 和聚合表达式不同。

`PlanNode` 树通过拥有型 `Vec<PlanNode>` 保存 children。构建路径每轮移动旧树到新根，谓词改写则用 `pop` 取出第一个 child、递归后再放回；代码只处理第一个 child，反映本输入协议当前是单链算子而非 join 等多子树结构。`buildTableScanSchema` 的双层匹配以表定义顺序为主，因而请求 `[b,a,a]` 会产生 `[a,a,b]`；`pb_to_plan_test.rs` 固化了这一不变量。

## 依赖与调用关系

crate 边界证据来自 `pkg/planner/core/Cargo.toml`：包名是 `astersql-planner-core`，库入口是 `lib.rs`，`autotests = false`，相邻测试必须由模块入口显式声明。`lib.rs` 第 160、162、225 行分别声明实现模块、条件测试模块和公开再导出。

RustCodeGraph 将 `NewPBPlanBuilder` 的直接 Rust 使用定位到 `pb_to_plan_test.rs` 的四个场景；将 `Build` 的本文件调用边定位为 `Build -> pbToPhysicalPlan` 与 `Build -> predicatePushDown`，并将 `pbToPhysicalPlan -> pbToTableScan/validateBroadcastQuery`、`pbToTableScan -> convertColumnInfo` 标为内部下游边。索引也报告 `pkg/planner/core/integration_test.rs` 中存在多个同名 `Build` 调用，但同名解析可能连接到其他 builder，不能据此认定它们调用本类型。

实际数据依赖保持很窄：`PlanNode::New`/`PlanKind` 承载输出树，`PlannerContext` 只被保存并由 `SessionContext` 返回，`BTreeMap` 提供表查找。当前没有 I/O、网络、infoschema 查询或 protobuf 解码；ranges 只贡献 `len()` 到展示文本。

## 错误处理与边界

所有可预期失败使用 `Result<_, String>`，不保留结构化错误类型或错误链：

- 空执行器列表：`executor list is empty`。
- 表 ID 缺失：`table which ID = ... does not exist`。
- 非集群表：`table ... is not a cluster table`。
- 列 ID 缺失：`column id ... does not exist in table ...`。
- 未支持算子：`this exec type ... doesn't support yet`。
- 广播语句不在白名单：`unexpected statement ... in broadcast query`。

广播校验先 trim 首尾空白、去除末尾所有分号、按空白拆词并转小写；仅精确允许 `admin reload bindings`、`flush stats_delta`、`refresh stats` 三种 token 序列。它不是 SQL parser：注释、引号、非标准空白之外的词法结构不会按 SQL 语义解析。测试只直接验证 `select 1` 被拒绝，允许分支及更多词法边界尚无相邻测试证据。

`predicatePushDown` 对缺 child 的 Selection 不报错，会继续走后续分支并原样返回节点；对多 child 节点只操作最后一个 `pop` 出来的 child。输入构造者必须维持单链形状。TopN/Limit 的 `offset + limit` 溢出策略、表达式类型校验、列索引绑定等 Go 行为在当前 Rust 模型中没有对应实现。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。`PBPlanBuilder` 按普通 Rust 所有权规则被当前调用者独占；`Build(&mut self)` 防止同一实例在安全 Rust 中被并发修改。构建出的 `PlanNode` 完全拥有其 children 和克隆后的字符串，因此不借用输入执行器或表元数据。

唯一跨调用保留的可变状态是 `field_types`。连续复用同一 builder 时，成功的 TableScan 会替换它；扫描在表/列校验之前失败则不会更新。`SessionContext` 返回的引用生命周期受 builder 约束。ranges 和 tables 随 builder 释放，不存在显式清理步骤。

## 与 Go 版本的对应关系

同路径 `pkg/planner/core/pb_to_plan.go` 是直接语义参照。两版共同保留了：叶到根遍历、当前节点挂接旧子计划、支持 TableScan/Selection/Projection/TopN/Limit/Hash 或 Stream Agg/Kill/BroadcastQuery、只允许集群表、列 ID 查找、Limit 的子节点信息补偿意图，以及 Selection 到内存表 extractor 的谓词下推边界。

关键差异必须视为当前迁移限制：

- Go 输入是真实 `tipb.Executor`，通过 infoschema 查表并生成 `base.PhysicalPlan`/`physicalop`；Rust 输入和输出均为本 crate 的轻量模型。
- Go 用 `types.FieldType` 和 `expression.PBToExpr(s)` 解码表达式，分配 schema column unique ID，并构建真实聚合描述；Rust只保存字符串。
- Go 为 slow query 从 ranges 构建时间范围，为四类表安装真实 extractor；Rust只按表名模拟“可消费谓词”，且 ranges 内容未解析。
- Go Limit 克隆 child schema、重设列索引，并给 slow-query extractor 设置包含溢出保护的 row-limit hint；Rust仅复制 `operator_info`。
- Go BroadcastQuery 用 parser 按 AST 类型验证并创建远端 `SQLBindPlan`/`Simple` 包装；Rust使用三条文本 token 白名单和 `Generic` 节点。
- Go 谓词下推会给表达式列补 UniqueID，并调用 extractor 返回剩余表达式；Rust对允许表名把全部字符串谓词视为已消费，仅修改展示文本。
- Go `Build` 对空列表可能得到 nil 计划；Rust显式拒绝空列表。这是可观察的语义差异。

因此，新增行为应优先比对 Go 对应函数，而不能把当前 Rust 表示的简洁性当作最终接口设计。

## 扩展指南

- 增加执行器类型时：先扩展 `PBExecutor`，再在 `pbToPhysicalPlan` 完整映射；同步在独立的 `pkg/planner/core/pb_to_plan_test.rs` 添加成功、错误及 child 挂接测试，不要把测试内嵌到源文件。
- 补齐真实 protobuf 语义时：重点替换字符串表达式、`field_types` 和轻量表元数据，逐项对齐 Go 的 `PBToExpr(s)`、schema 构建、column unique ID 与错误传播；避免仅让类型编译而丢掉 Go 校验。
- 扩展可下推表时：不能只把表名加入 `matches!`。必须先具备与 Go extractor 等价的真实谓词提取和“剩余谓词”返回语义，并添加“可消费/部分消费/不可消费”测试，否则当前全量消费会静默改变结果。
- 修改算子穿透规则时：以 Projection/TopN/Limit/Agg 的表达式及行序语义为边界审查，保持 `selection_is_not_pushed_through_projection` 回归；若引入多 child 计划，需要重写目前只 `pop` 一个 child 的算法。
- 修改扫描列次序时：确认是否仍需 Go `buildTableScanSchema` 的“表顺序优先、重复请求保留”语义，并同步现有重复列测试。
- 修改广播白名单时：优先迁移解析器/AST 校验而非继续扩展字符串特例；至少覆盖允许语句、大小写/分号、相似但非法语句。
- 性能风险集中在 `convertColumnInfo` 和 `buildTableScanSchema` 的嵌套线性查找；表列或请求列很大时可考虑预建 ID 索引，但必须保留表序与重复项语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/planner/core/pb_to_plan.rs` 确认目标文件含 25 个符号。
- RustCodeGraph `node --file pkg/planner/core/pb_to_plan.rs --offset 1 --limit 500`：核对本文件全部 382 行、公开/私有符号、分支、状态修改和递归逻辑。
- RustCodeGraph `explore '"PBPlanBuilder" "NewPBPlanBuilder" "pb_to_plan.rs"'`：核对主要内部边和相邻测试使用；精确 `callers/callees` 对 Rust impl 方法存在同名污染/缺边，因此本文没有把不明确的同名结果作为生产调用结论。
- `pkg/planner/core/lib.rs`：核对模块装配、测试分离和公开再导出。
- `pkg/planner/core/Cargo.toml`：核对 crate 名、库入口、`autotests = false`、feature 与依赖边界。
- `pkg/planner/core/pb_to_plan.go`：逐函数核对 Go 的 builder、真实 protobuf/physical plan 转换、schema、extractor、广播 AST 校验和谓词下推语义。
- `pkg/planner/core/pb_to_plan_test.rs`：核对重复列的表序、Projection 下推屏障、无 extractor 时保留 Selection、非法广播查询错误。
- 未运行 Cargo：任务是纯文档分析，任务计划明确排除 Cargo。本文只声明可由上述静态源码和测试内容证明的行为，没有把测试文件存在等同于本会话执行通过。

# [`pkg/planner/core/hint_utils.rs`](./hint_utils.rs)

## 文件定位

本文件属于 `astersql-planner-core` crate。crate 入口在 `pkg/planner/core/lib.rs:148` 以私有模块 `mod hint_utils` 纳入它，并在 `pkg/planner/core/lib.rs:215` 通过 `pub use hint_utils::*` 导出本文件的公开项。其输入不是 SQL AST，而是 `common_plans.rs` 中的简化物理计划模型 `PlanNode`、`PlanKind`、`StoreType`，以及 `flat_plan.rs` 的 `FlatPhysicalPlan`；输出是本文件自有的 `OptimizerHint` 列表。

从用途看，它负责从既有物理计划反推可描述该计划选择的优化器 Hint，属于“计划结果 -> Hint 表示”的逆向转换层。当前 Rust 代码搜索只发现独立测试 `pkg/planner/core/hint_utils_test.rs` 直接调用公开入口，未发现 Rust 生产调用者；因此它是已导出的迁移实现，但不能据此声称已接入 Rust 的完整 `EXPLAIN FORMAT=HINT` 主链。对应 Go 生产接线位于 `pkg/planner/core/common_plans.go:992-996`。

## 核心职责

1. `GenHintsFromFlatPlan` 从扁平计划的主查询算子序列生成 Hint；每个算子沿用扁平记录上的 `StoreType`（`hint_utils.rs:31-37`）。
2. `GenHintsFromPhysicalPlan` 深度优先遍历树形 `PlanNode`，收集每个节点对应的 Hint；当根节点是受支持的 Hash/Merge Join 链且能提取三个以上表时，再生成 `LEADING`（`hint_utils.rs:39-57`）。
3. `genHintsFromSingle` 把有限的 `PlanKind` 映射为连接算法、聚合算法或存储读取 Hint（`hint_utils.rs:66-126`）。
4. `extractTableAsName` 和 `extractOrderedPhysicalJoinGroup` 负责从计划形状中安全提取连接表名与线性连接次序（`hint_utils.rs:135-200`）。
5. 以 `BTreeSet<OptimizerHint>` 去重并按派生顺序稳定输出，避免遍历中产生重复项或非确定性次序。

本实现当前只覆盖 `HASH_JOIN`、`MERGE_JOIN`、三种 index join、`HASH_AGG`、`STREAM_AGG`、`READ_FROM_STORAGE` 和特定条件下的 `LEADING`。其他 `PlanKind` 明确走空分支，不产生 Hint。

## 主要符号

- `pub struct OptimizerHint { name, tables, store }`（`hint_utils.rs:25-29`）：轻量 Hint 值对象。`name` 是 Hint 名称，`tables` 是相关表名序列，`store` 仅供存储读取 Hint 使用。其 `Eq`/`Ord` 派生是 `BTreeSet` 去重及确定性排序的基础。
- `pub fn GenHintsFromFlatPlan(&FlatPhysicalPlan) -> Vec<OptimizerHint>`（`hint_utils.rs:31-37`）：只遍历 `flat.Main`，将每个 `FlatOperator.Origin` 和该扁平记录的 `StoreType` 送入单节点映射器。它不读取 `CTE`、`ScalarSubQ`、根标志或子树下标。
- `pub fn GenHintsFromPhysicalPlan(&PlanNode) -> Vec<OptimizerHint>`（`hint_utils.rs:39-57`）：树形入口；调用 `collect`，然后仅以传入根节点尝试生成一次 `LEADING`。
- `fn collect`（`hint_utils.rs:59-64`）：前序深度优先递归；先处理当前节点，再按 `children` 顺序处理所有子节点。
- `fn genHintsFromSingle`（`hint_utils.rs:66-126`）：核心类型分派。连接 Hint 使用 `join_tables`；聚合 Hint 不带表；`TableScan` 的存储名来自 `format!("{:?}", store).to_lowercase()`。
- `fn join_tables`（`hint_utils.rs:128-133`）：对连接节点的每个直接孩子调用 `extractTableAsName`，无法提取的孩子被 `filter_map` 静默略过。
- `fn extractTableAsName`（`hint_utils.rs:135-143`）：多于一个孩子时拒绝提取；`TableScan` 和 `DataSource` 返回其 `table` 字段，否则沿唯一孩子链向下寻找第一个可识别表节点。
- `fn extractOrderedPhysicalJoinGroup`（`hint_utils.rs:145-200`）：内嵌递归函数 `visit` 验证连接组，并按连接树结构构造表名序列；失败统一转为空向量。

除以上结构体与函数外，本文件没有模块级常量、trait、`impl`、条件编译项或异步入口。命名沿用 Go 导出函数风格，因而公开 Rust 函数没有采用 snake_case。

## 执行流程

扁平计划路径如下：

1. `GenHintsFromFlatPlan` 创建空 `BTreeSet`。
2. 依次读取 `flat.Main` 中的 `FlatOperator`。
3. 对每项调用 `genHintsFromSingle(&operator.Origin, operator.StoreType, ...)`。
4. 单节点映射器根据 `PlanKind` 插入零个或一个 Hint。
5. 最后按 `OptimizerHint` 的全字段排序次序收集为 `Vec`。

树形计划路径如下：

1. `GenHintsFromPhysicalPlan` 创建空集合，并调用 `collect`。
2. `collect` 对整棵树作前序遍历，所以每个可识别连接、聚合、扫描节点都有机会生成 Hint。
3. 遍历完成后，仅当入口根为 `HashJoin` 或 `MergeJoin` 时调用 `extractOrderedPhysicalJoinGroup`。
4. 连接组提取只接受线性 join 链：叶 join 必须恰有两个可提取表的孩子；非叶 join 只能有一个 join 孩子；两个孩子都是 join 的 bushy tree 被拒绝。
5. Hash Join 没有等值条件时被视为笛卡尔边界并拒绝；Merge Join 只接受 `InnerJoin`、`LeftOuterJoin`、`RightOuterJoin`。
6. 成功提取且表数大于 2 时插入 `LEADING(tables...)`；两表连接无需连接次序 Hint。

单节点映射为：`HashJoin -> HASH_JOIN`，`MergeJoin -> MERGE_JOIN`，`IndexJoin -> INL_JOIN`，`IndexMergeJoin -> INL_MERGE_JOIN`，`IndexHashJoin -> INL_HASH_JOIN`，`HashAgg -> HASH_AGG`，`StreamAgg -> STREAM_AGG`，`TableScan -> READ_FROM_STORAGE`。连接和聚合 Hint 的 `store` 始终为 `None`。

## 数据与状态

本文件只操作调用栈上的临时值，没有全局状态。最重要的数据不变量是：

- `OptimizerHint` 的三个字段共同参与相等性与排序；同名但表序或存储不同的 Hint 不会被误判为重复。
- `BTreeSet` 同时承担去重与稳定排序，因此返回顺序不是计划遍历顺序，而是结构体派生的字典序。调用者不应把输出位置解释为节点访问顺序。
- `join_tables` 保留可提取孩子的原始相对顺序，但会丢弃无法提取的孩子；因此算法 Hint 可能带少于两个表，甚至空表列表，本文件不把这种情况当错误。
- `extractTableAsName` 不读取 `DataSource.alias`，而直接返回 `DataSource.table`；当前 Rust 表名表示不能完整表达 Go 版的数据库名、别名和查询块名。
- `READ_FROM_STORAGE.store` 是 `StoreType` 的 Debug 名称转小写：当前 `common_plans.rs:44-49` 的 `Root`、`TiKV`、`TiFlash` 会分别形成 `root`、`tikv`、`tiflash`。
- `LEADING` 只在树形入口、且入口本身是连接根时生成；`GenHintsFromFlatPlan` 没有对应逻辑。

## 依赖与调用关系

直接 Rust 依赖均来自本 crate：`FlatPhysicalPlan` 定义于 `pkg/planner/core/flat_plan.rs:382-395`，其中 `Main` 是 `Vec<FlatOperator>`；`FlatOperator.Origin` 和 `StoreType` 定义见 `flat_plan.rs:504-516`。`PlanNode`、`PlanKind`、`StoreType` 来自 `pkg/planner/core/common_plans.rs:44-161,222-239`。标准库依赖仅为 `std::collections::BTreeSet`。

RustCodeGraph 核对到的本文件内部主调用边为：

- `GenHintsFromPhysicalPlan -> collect`
- `GenHintsFromPhysicalPlan -> extractOrderedPhysicalJoinGroup`
- `collect -> genHintsFromSingle`，并通过自身递归遍历孩子
- `genHintsFromSingle -> join_tables`
- `join_tables -> extractTableAsName`
- `extractOrderedPhysicalJoinGroup -> visit -> extractTableAsName`

crate 的 `Cargo.toml` 声明 `[lib] path = "lib.rs"`、`autotests = false`，且本文件不直接使用任何 Cargo 外部依赖。测试由 `lib.rs:330-331` 用 `#[cfg(test)]` 下的路径模块接入，而不是 Cargo 自动发现。

上游方面，Rust 侧当前仅有 `hint_utils_test.rs` 的直接调用证据；搜索不到生产 Rust 调用边。Go 侧的实际应用入口是 `Explain.getRows` 的 `ExplainFormatHint` 分支：先 `FlattenPhysicalPlan`，再调用 Go `GenHintsFromFlatPlan` 并恢复为 SQL Hint 字符串（`common_plans.go:992-996`）。Go 的 DAG 和 physical-plan 用例还会比较树形入口与扁平入口结果（如 `pkg/planner/core/casetest/dag/dag_test.go:224-239`）。

## 错误处理与边界

本文件接口不返回 `Result` 或错误对象；不支持和信息不足均采用“跳过该 Hint”的降级策略：

- 未匹配的 `PlanKind` 不产生 Hint。
- 多孩子包装节点不能解析成单表，唯一孩子链上也可能最终返回 `None`。
- `join_tables` 对单个失败孩子直接略过，不使整项连接 Hint 失败。
- `extractOrderedPhysicalJoinGroup` 遇到空等值条件 Hash Join、非三种受支持类型的 Merge Join、孩子数不是 2、bushy join、递归组少于两个表或叶表无法提取时返回空列表。
- 即使根是连接，连接组只有两个表也不生成 `LEADING`。

这些边界由独立测试 `hint_utils_test.rs` 直接覆盖：三种 index join 的 Go 兼容名称；两表和笛卡尔 Hash Join 不生成 `LEADING`；bushy join 与 `SemiJoin` Merge Join 不生成 `LEADING`；算法 Hint 不携带存储引擎数据。测试尚未直接覆盖扁平入口、聚合映射、扫描存储字符串、重复 Hint 排序/去重、单侧表名提取失败和成功生成三表 `LEADING`，扩展时应补齐。

需要注意两个健壮性前提：树形递归深度与计划深度一致，极端深树可能消耗较多栈；`extractOrderedPhysicalJoinGroup` 在确认连接节点后会索引 `children[0]`/`children[1]`，但此前已检查 `children.len() == 2`，不存在本地越界路径。

## 并发与资源生命周期

所有函数都是同步纯计算：只借用输入计划、克隆必要的表名字符串，并构造本次调用私有的集合与向量。没有锁、原子变量、线程、异步任务、通道、文件句柄、网络连接或事务生命周期。

返回的 `Vec<OptimizerHint>` 完全拥有其字符串，不依赖输入计划生命周期。递归期间可变集合由唯一的 `&mut BTreeSet` 沿调用栈串行传递，不存在共享并发写入。主要资源成本是遍历和字符串克隆：树形入口总体按访问节点数线性扫描，集合插入带对数开销；连接组提取会再走一次根连接链。`extractTableAsName` 还可能沿每个连接孩子的单孩子包装链向下遍历。

## 与 Go 版本的对应关系

Rust 的公开入口和若干辅助函数与 `pkg/planner/core/hint_utils.go` 同名或同职责，但当前并非等价完整移植。

- Go `GenHintsFromPhysicalPlan` 先调用 `FlattenPhysicalPlan`，然后统一走扁平入口（Go `hint_utils.go:75-79`）；Rust 版本直接递归 `PlanNode`。
- Go 扁平入口识别 SELECT/UPDATE/DELETE 节点类型，筛选有效 select plan，遍历主计划与 CTE 根，并在遍历各 root join 时借助 visited ID 集合生成 `LEADING`（Go `hint_utils.go:27-73`）。Rust 只遍历 `flat.Main`，没有节点类型、CTE、标量子查询、visited ID 或扁平路径 `LEADING`。
- Go Hint 使用 `ast.TableOptimizerHint`，包含查询块、库表、索引等完整 SQL 恢复信息；Rust `OptimizerHint` 只有名称、字符串表列表和可选存储名。
- Go `genHintsFromSingle` 覆盖 Limit/TopN 下推、Table/Index/IndexLookup/IndexMerge Reader、索引顺序、Apply、更多连接侧语义和查询块推断（Go `hint_utils.go:81-321`）；Rust 只实现九类基础映射。
- Go 的表提取识别 reader 与有限包装算子，优先表别名，并保留数据库名（Go `hint_utils.go:513-541`）；Rust 识别 `TableScan`/`DataSource`，不使用 `DataSource.alias`，并可穿过任意单孩子节点。
- 两边连接组规则的核心意图一致：拒绝笛卡尔 Hash Join、只接受可重排 join type、拒绝 bushy tree、把非 join 叶依次追加到线性组（Go `hint_utils.go:582-637`；Rust `hint_utils.rs:145-200`）。但 Go 同时记录 visited join ID，并把具体节点交给查询块/表名生成器；Rust 直接返回字符串表名。

因此新增行为时应以 Go 文件作为语义目标，但不能直接假定 Go 已有的所有能力在 Rust 中可用。当前 Rust 测试名称 `index_join_variants_keep_their_go_hint_names` 也明确把 Hint 名称兼容性作为已有移植约束。

## 扩展指南

扩展单节点 Hint 时，应在 `genHintsFromSingle` 增加精确 `PlanKind` 分支，并决定表信息应来自当前节点还是孩子；若新增字段参与去重，需要同步评估 `OptimizerHint` 的派生排序是否仍能给出合适的稳定顺序。新增存储类型时，不应默认 Debug 小写就是 SQL 可接受名称，最好在 `StoreType` 上建立显式名称映射并测试。

扩展 `LEADING` 时，入口和连接组提取必须一起考虑。若要让扁平入口与 Go 对齐，需要利用 `FlatOperator.ChildrenIdx`/`IsRoot` 重建根连接组或复用树形来源，并处理 CTE、标量子查询和重复访问；不能简单对 `flat.Main` 中每个 join 独立生成，否则会重复或把子 join 误当根。若支持 bushy join 或更多 join type，也要明确 Hint 的嵌套表示；当前 `Vec<String>` 无法表达 Go/SQL 可能需要的嵌套连接结构。

涉及表身份的改动应同步处理 `DataSource.alias`、数据库名和查询块信息，避免同名表、自连接、派生表场景产生不可回放 Hint。性能上应保留单次或少量线性遍历，避免对每个 join 重扫完整子树。

测试必须继续放在独立文件 `pkg/planner/core/hint_utils_test.rs`，并通过 `lib.rs` 的测试模块接线；不要把测试嵌入生产源文件。优先补充：三表左深/右深 `LEADING` 顺序、别名与自连接、包装节点、畸形孩子数、聚合与存储读取、重复去重、扁平计划的 CTE/子查询策略，以及与 Go golden 行为的一致性。若 Rust 生产主链完成接线，还应增加从计划生成到 Hint SQL 恢复的集成验证。

## 验证依据

- RustCodeGraph 索引状态：项目索引覆盖 11,467 个文件，其中 Rust 7,032 个、Go 4,415 个；使用 `node --file` 阅读了 `pkg/planner/core/hint_utils.rs` 全部 200 行。
- RustCodeGraph 符号与调用查询：查询了 `OptimizerHint`、两个公开入口、`collect`、`genHintsFromSingle`、`join_tables`、`extractTableAsName`、`extractOrderedPhysicalJoinGroup`，并对关键入口执行 `callers`/`callees`；据此核对了本文件内部调用链和 Rust 生产调用者缺失的现状。
- Rust 数据模型：通过图节点核对 `pkg/planner/core/common_plans.rs` 的 `StoreType`、`PlanKind`、`PlanNode`，以及 `pkg/planner/core/flat_plan.rs` 的 `FlatPlanTree`、`FlatPhysicalPlan`、`FlatOperator`。
- crate 与模块边界：读取 `pkg/planner/core/Cargo.toml` 和 `pkg/planner/core/lib.rs:148,215,330-331`。
- Rust 测试：读取 `pkg/planner/core/hint_utils_test.rs` 全文，核对 index join 名称、`LEADING` 拒绝条件和算法 Hint 的存储字段约束。
- Go 对照：读取 `pkg/planner/core/hint_utils.go:27-210,300-637`，并读取生产入口 `pkg/planner/core/common_plans.go:970-1014` 和对照用例 `pkg/planner/core/casetest/dag/dag_test.go:215-239`。
- 仓库搜索：核对 `hint_utils` 的模块声明、再导出和 Rust 调用点；未把没有调用证据的 Rust 生产接线描述为现状。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前用任务指定命令验证文档存在且恰有 11 个固定二级章节，并人工复核所有关键结论均指向上述符号或文件证据。

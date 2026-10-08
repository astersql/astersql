# `pkg/planner/core/rule/rule_order_aware_join_reorder.rs`

## 文件定位

本文件属于 `astersql-planner-core-rule` crate；crate 根在 `pkg/planner/core/rule/lib.rs`，其中以公开模块 `rule_order_aware_join_reorder` 导出本文件。`pkg/planner/core/rule/Cargo.toml` 把该 crate 映射到 Go 包 `pkg/planner/core/rule`，而本文件的可执行 Rust 实现只直接使用同 crate 的 `rule_init::{Expr, JoinType, LogicalRule, Plan, PlanKind}`。

文件包含两层内容，阅读时必须区分：第 16—258 行是注释化的 Go 来源与移植说明，不参与编译；第 259 行之后才是当前可执行 Rust。当前 Rust 面向 `rule_init.rs` 定义的精简逻辑计划 IR，而不是 `operator/logicalop` 的完整优化器计划对象。因此，本文件自身的 `OrderAwareJoinReorder` 主要用于规则 crate 的独立变换与单元测试；完整 SQL 优化器中同名阶段由 `pkg/planner/core/optimizer_runtime.rs::order_aware_join_reorder_descendants` 另行实现。

## 核心职责

`OrderAwareJoinReorder` 在计划树上传播上层排序列需求，并仅对具有至少两个输入的 `Inner` Join 调整子节点顺序。排序键可由 `Sort` 建立，经 `Limit`、确定性 `Selection` 继续下传，并由 `Projection` 按输出 schema 位置改写到输入表达式。重排时，能够按索引前缀提供所需顺序的输入优先，其他输入再按 `estimated_rows` 升序排列。

规则的保守边界同样属于核心职责：没有有效排序需求时保持原 Join 次序；外连接、半连接、反半连接和不足两个输入的 Join 不重排；无法跨越的表达式或非确定性筛选会清空排序需求。这些约束分别由 `reorder`、`extract_ordering_columns`、`rewrite_ordering_for_projection` 和 `satisfies_ordering` 实现。

## 主要符号

- `pub struct OrderAwareJoinReorder`：无字段的规则标记类型，是本文件唯一公开类型。
- `impl LogicalRule for OrderAwareJoinReorder`：`name()` 返回稳定标识 `order_aware_join_reorder`；`optimize(plan)` 取得计划所有权，调用 `reorder(&mut plan, &[])`，再返回修改后的计划与 `changed`。当前实现不产生业务错误，始终返回 `Ok`。
- `fn reorder(plan, inherited_order) -> bool`：核心后序递归。先计算当前节点向下传递的 `local_order`，递归所有子树，再按约束重排当前 Join，最终聚合本节点及后代是否发生变化。
- `fn extract_ordering_columns(items) -> Vec<i64>`：从 `Sort.by` 提取连续的 `Expr::Column` ID；通过 `map_while` 在首个非列表达式处停止。
- `fn rewrite_ordering_for_projection(schema, expressions, order) -> Vec<i64>`：以排序列在 Projection 输出 `schema` 中的位置查找对应输入表达式；任一列不存在或对应表达式不是裸列时整体返回空向量。
- `fn satisfies_ordering(plan, order) -> bool`：判断单个子计划能否提供顺序。数据源要求某个索引列向量以完整 `order` 为前缀；Projection 递归使用改写后的列；Selection/Limit 仅在谓词全为确定性时递归首个子节点；其他节点返回 `false`。

## 执行流程

1. `LogicalRule::optimize` 以空排序需求调用 `reorder`。只有遍历遇到 `PlanKind::Sort { by }` 才会从 `by` 建立新的本地排序列集合。
2. `reorder` 根据当前节点决定向下需求：Sort 重置需求；Projection 改写继承的列；含非确定谓词的 Selection 清空需求；Limit 和其他节点原样继承。
3. 函数先对全部子节点递归，使用逻辑或累积 `changed`。这使嵌套 Join 先处理，再处理当前 Join。
4. 非 Join 节点在递归后立即返回；Join 还要检查 `JoinType::Inner`、输入数不少于二以及 `local_order` 非空。任一条件不满足都保持当前输入顺序。
5. 合格 Join 先克隆原 `children`，再稳定地按两级键排序：`satisfies_ordering == true` 的输入在前；同一组内按 `estimated_rows` 从小到大。浮点数无法比较（例如 NaN）时视为相等，保留该组原相对顺序。
6. 排序后以完整 `children` 与克隆值比较；若不同则把当前节点变化并入 `changed`。`optimize` 最终返回整个计划和这一布尔值。

## 数据与状态

本文件没有全局变量、缓存或内部可变字段。运行时状态全部来自 `Plan`：`kind` 决定传播规则，`schema` 和 Projection 的 `expressions` 完成列映射，DataSource 的 `indexes: BTreeMap<i64, Vec<i64>>` 表示索引列顺序，`predicates` 决定 Selection/Limit 是否可透明穿越，`estimated_rows` 提供次级排序键，`children` 是唯一被修改的数据。

排序需求使用 `Vec<i64>`/`&[i64]` 表示列 ID 序列，顺序具有语义；索引必须以整个需求序列为前缀。`changed` 是递归累计值，不表示“发现可保序输入”，只表示某处子节点向量实际改变。为比较前后状态，当前 Join 会克隆其全部 `children`，因此内存开销随该 Join 子树的 `Plan` 克隆成本增长。

## 依赖与调用关系

上游直接证据有两类：`pkg/planner/core/rule/rule_order_aware_join_reorder_test.rs` 和 `rule_aster_unit_test.rs` 导入并直接调用 `OrderAwareJoinReorder.optimize`；`pkg/planner/core/rule/lib.rs` 公开模块，`rule_init.rs::default_rule_names` 把名称列入默认规则名序列。RustCodeGraph 也将上述两个测试文件识别为目标文件使用者。

下游调用链为 `optimize -> reorder`；`reorder` 递归调用自身，并调用 `extract_ordering_columns`、`rewrite_ordering_for_projection` 与 `satisfies_ordering`；`satisfies_ordering` 又可递归自身并复用 Projection 改写函数。表达式确定性由 `rule_init.rs::Expr::deterministic` 判断，其中 `rand`、`uuid`、`now` 及包含它们的复合表达式为非确定性。

完整应用主链需要单独看待：Go 的 `pkg/planner/core/optimizer.go` 把 `rule.OrderAwareJoinReorder` 放在 `optRuleList` 中，位于 FullText Projection 处理之后、统计同步与普通 Join 重排之前。当前 Rust 完整计划主链的 `optimizer_runtime.rs::LOGICAL_RULES` 保持同一阶段位置，但 dispatch 调用的是该文件内的 `order_aware_join_reorder_descendants`，并未构造本文件的精简 IR 规则类型；因此不能据此声称本文件直接处理 SQL 生成的 `logicalop::LogicalPlan`。

## 错误处理与边界

当前 Rust 函数没有可失败的外部操作。`optimize` 的签名服从 `LogicalRule` 的 `Result<(Plan, bool), String>`，但本实现没有 `Err` 分支；无法证明顺序时均降级为“不传播/不重排”，而不是报错。

重要边界如下：空排序列永远不满足顺序；Sort 中遇到首个非 Column 表达式会保留此前已提取的前缀，而 Go `extractOrderingColumns` 对任一降序项或非列项会整体返回 `nil`，且 Rust 精简 `Expr` 没有方向字段；Projection 任一列映射失败会整体清空需求；`satisfies_ordering` 只沿第一个孩子递归；Selection/Limit 的所有谓词都必须确定；只有 DataSource、Projection、Selection、Limit 被认作可保序；只重排 Inner Join。当前实现也不读取 Go 侧的 session variables、join reorder threshold、用户 hint 或中间筛选条件。

## 并发与资源生命周期

规则是同步、单线程、按所有权运行的纯内存树变换，没有锁、原子变量、线程、异步任务、通道、事务或 I/O 资源。`optimize` 获取 `Plan` 所有权；递归期间通过独占 `&mut Plan` 逐层修改，返回时所有临时借用结束，修改后的树重新交给调用者。

临时资源主要是每层的 `local_order`、Projection 改写向量，以及合格 Join 为变化检测创建的 `children` 深克隆。递归深度等于计划树深度；资源在对应栈帧返回时释放。由于不存在共享状态，同一规则实例可被不同调用独立使用，但调用方仍需自行管理其计划对象的并发访问。

## 与 Go 版本的对应关系

名称、顶层接口目的和“从 Sort/TopN 向下传播顺序、跨 Projection 改写、在数据源验证索引前缀”的总体意图来自同路径 Go 文件 `rule_order_aware_join_reorder.go`。两个版本也都在无有效排序需求时保守退出。

但当前文件不是 Go 逻辑的等价移植。Go 版本遍历真实 `base.LogicalPlan`，同时处理 TopN 和 Sort，识别升降序，只在高级 CDC join reorder 配置与阈值满足时调用 `joinorder.FindOrderedLeadingChoice`，沿选定 carrier vertex 传播需求，累计可帮助索引前缀的 Selection filters，并以 `TryAnnotateOrderedLeading` 设置 Join group 的内部 leading preference。它还显式停止于 CTE、尊重 `TiDBOptJoinReorderThroughSel`，并返回错误。

本 Rust 文件则直接对精简 `Plan.children` 排序：没有 TopN 节点类型、CTE、session variable、carrier vertex、filter accumulation、hint 冲突检测或注解动作；非确定 Selection 无条件阻断，而确定 Selection 无配置门控。`pkg/planner/core/optimizer_runtime.rs` 的完整计划实现更接近 Go 的“设置 `InternalPreferJoinOrder`”效果，SQL 级测试 `optimizer_logical_entry_aster_unit_test.rs::order_aware_join_reorder_marks_index_order_carrier` 与 `casetest/rule/rule_cdc_join_reorder_test.rs::order_aware_join_reorder_pushes_selection_and_keeps_order` 覆盖的是该运行时路径，不应误作本文件直接调用证据。

## 扩展指南

- 若扩展本精简 IR 规则，应优先修改 `reorder` 的传播矩阵或 `satisfies_ordering` 的可保序节点集合，并在独立的 `rule_order_aware_join_reorder_test.rs` 增加回归；不要把测试内嵌到生产 `.rs`。
- 增加排序方向、TopN 或 CTE 语义时，应先扩展 `rule_init.rs` 的表达能力，再使提取逻辑遵循 Go 的全有或全无约束；尤其不要让部分可识别 ORDER BY 前缀被误当作完整需求。
- 修改 Projection 映射时必须保持“输出 schema 位置对应输入 expression”的不变量，并覆盖缺列、计算表达式、多列顺序和空需求。
- 修改 Join 比较器时需同时验证无排序需求不动、非 Inner Join 不动、有序 carrier 最左、其余按基数稳定排序，以及 NaN 统计值的确定性。大计划上的深克隆是明确性能风险，可用“记录原子节点标识/顺序”替代，但必须保持准确的 `changed` 语义。
- 若目标是对齐 Go 完整行为，不应仅继续膨胀本文件的精简模型；需先确认 `optimizer_runtime.rs` 的真实计划路径与本 crate 的职责边界，再同步 session variable、hint、Selection filter、carrier 注解和独立 SQL 级测试，避免出现两套实现漂移。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file pkg/planner/core/rule/rule_order_aware_join_reorder.rs` 读取全文件并识别两个直接测试使用者；`query` 定位 `OrderAwareJoinReorder`、`satisfies_ordering`、`rewrite_ordering_for_projection`、`extract_ordering_columns`，自然语言 `explore` 与部分 callers/callees 查询超时，故调用边由已索引源码节点与精确搜索补充核验。
- 生产源码：`pkg/planner/core/rule/rule_order_aware_join_reorder.rs`；共享 IR 与 trait：`pkg/planner/core/rule/rule_init.rs`；模块入口：`pkg/planner/core/rule/lib.rs`；crate 边界：`pkg/planner/core/rule/Cargo.toml`。
- 直接 Rust 测试：`pkg/planner/core/rule/rule_order_aware_join_reorder_test.rs::projection_rewrites_order_column_before_choosing_leading_join_input`；`pkg/planner/core/rule/rule_aster_unit_test.rs::join_reorder_is_noop_without_an_order_requirement`。
- Go 对照：`pkg/planner/core/rule/rule_order_aware_join_reorder.go`；Go 主链：`pkg/planner/core/optimizer.go`；Go SQL 测试：`pkg/planner/core/casetest/rule/rule_cdc_join_reorder_test.go` 中的 `TestOrderAwareJoinReorderPushSelection` 与 `TestOrderAwareJoinReorderAlternativeRound`。
- 完整 Rust 规划器对照：`pkg/planner/core/optimizer_runtime.rs::{LOGICAL_RULES, order_aware_join_reorder_descendants}`、`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs::order_aware_join_reorder_marks_index_order_carrier`、`pkg/planner/core/casetest/rule/rule_cdc_join_reorder_test.rs::order_aware_join_reorder_pushes_selection_and_keeps_order`。
- 本任务是只新增文档的静态分析；按任务要求未运行 Cargo。结构检查应确认文件存在且恰有十一个固定二级标题；人工复核重点是当前精简实现与 Go/完整运行时路径的边界没有被混写。

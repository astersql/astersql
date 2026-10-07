# `pkg/planner/core/joinorder/ordered_leading.rs`

## 文件定位

本文件属于 `astersql-planner-core-joinorder` crate；crate 入口 `pkg/planner/core/joinorder/lib.rs` 以 `pub mod ordered_leading` 公开该模块，`Cargo.toml` 指定库入口为 `lib.rs`。它在自包含的 Rust `PlanNode` 模型上推断“哪张叶表可凭索引顺序承载 ORDER BY”，并可在连接组根 Join 上合成 merge-join 偏好。

当前接线需要特别区分：RustCodeGraph 显示本文件只被 `ordered_leading_test.rs` 和 `join_order_test.rs` 使用；仓库搜索也没有发现 Rust 生产调用者。Rust 的 `rule_order_aware_join_reorder.rs` 仅在注释保存的 Go 对照片段中提及这些 API。因此，本文描述的是已经实现且受单元测试覆盖的 Rust 局部能力，不把它表述为已进入 Rust 优化器主链。当前完整应用主链存在于 Go：`rule_order_aware_join_reorder.go` 调用同目录 `ordered_leading.go`。

源码第 23—221 行是一段注释掉的、贴近 Go 类型体系的迁移参考，不参与编译；实际 Rust 实现从 `OrderedLeadingChoice`（第 227 行）开始。

## 核心职责

1. `find_ordered_leading_choice` 验证排序列，遍历连接树的等值条件、定值条件和叶表索引，选出第一个能覆盖完整排序向量的表扫描。
2. `collect_equalities` / `collect_equality` 把确定性的“列 = 列”谓词扩展为每个 ORDER BY 位置可接受的列等价集合。
3. `collect_fixed_columns` / `collect_fixed_columns_from_expr` 识别确定性的“单列 = 常量”，允许索引在开始匹配 ORDER BY 列之前跳过已被固定的前缀列。
4. `index_matches_ordering` 严格按顺序验证索引：匹配开始后不允许出现间断或额外列。
5. `try_annotate_ordered_leading` 在候选叶表仍属于当前连接组且根节点没有算法 hint 时，把根 Join 标成 `prefer_merge`。

本模块只读取和修改内存中的逻辑计划表示，不访问表数据、不扫描索引，也不执行 SQL。

## 主要符号

- `pub struct OrderedLeadingChoice { leaf_id, ordering_columns, reverse }`：成功结果。`leaf_id` 是承载顺序的叶节点 ID；`ordering_columns` 保存验证后的原始顺序；`reverse` 为未来反向索引扫描预留，当前构造结果恒为 `false`。
- `pub fn find_ordered_leading_choice(&PlanNode, &[i64]) -> Option<OrderedLeadingChoice>`：候选查找公开入口。空排序、非正 ID、重复 ID、单叶计划或无匹配索引均返回 `None`。
- `pub fn try_annotate_ordered_leading(&mut PlanNode, &OrderedLeadingChoice) -> bool`：标注入口。成功时仅修改传入根节点的 `PlanKind::Join.hint.prefer_merge`，并返回 `true`。
- `collect_leaves`：以深度优先、从左到右顺序收集无孩子节点；候选选择因此是“首个匹配叶、首个匹配索引”。
- `collect_equalities` / `collect_equality`：递归读取所有 Join 的 `equal_conditions` 与 `other_conditions`，处理 `Expr::Eq` 和嵌套 `Expr::And`。
- `collect_fixed_columns` / `collect_fixed_columns_from_expr`：递归收集确定性常量等值固定的列；不会处理 `IN`。
- `index_matches_ordering`：消费 `&[i64]` 索引列、按 ORDER BY 位置组织的等价类，以及可跳过的固定前缀列。

文件没有 trait、模块级可变状态、条件编译项或异步函数。

## 执行流程

`find_ordered_leading_choice` 的完整路径如下：

1. 调用 `PlanNode::vertexes`；连接组只有一个或零个顶点时立即拒绝。
2. 逐个检查 `ordering_columns`。列 ID 必须大于零且不能重复；`BTreeSet` 只用于检测重复，`normalized` 保留调用者给出的列顺序。
3. 为每个排序列建立一个单元素等价类，再遍历整棵计划树。遇到确定性的单列等值 `a = b` 时，任何包含 `a` 或 `b` 的等价类同时加入两者；`AND` 被递归展开。
4. 深度优先收集所有叶节点；同时遍历 Join 条件，把确定性的单列对常量等值记入 `fixed_prefix_columns`。
5. 只检查 `PlanKind::Table` 叶节点的每个索引。`index_matches_ordering` 依次消费排序位置；尚未消费第一个排序列时可跳过固定前缀，开始消费后首次不匹配即失败。
6. 首个完整匹配生成 `OrderedLeadingChoice`；没有索引匹配则返回 `None`。

调用方若随后执行 `try_annotate_ordered_leading`，函数先确认 `choice.leaf_id` 仍在 `root.vertexes()` 中，再要求 `root.kind` 本身就是 Join。若四种算法偏好均未设置，它只把根 Join 的 `prefer_merge` 改为 `true`；不会递归修改内部 Join。`ordered_leading_test.rs::annotation_only_marks_the_join_group_anchor` 明确验证了这一点。

## 数据与状态

- 列、叶节点和连接组身份都以整数 ID 表示；本文件不持有 Go 版的 `LogicalPlan`、`Schema` 或表别名对象。
- `Vec<i64>` 保存有语义的 ORDER BY 次序；`BTreeSet<i64>` 用于确定性去重、列集合、等价类和固定前缀集合。
- 等价类以“每个排序位置一个集合”表示，而不是通用并查集。一次 `a = b` 会扩展所有已包含任一端的相关集合，足以支持从目标排序列出发的匹配，但不是独立的全图等价关系 API。
- 叶表索引来自 `PlanKind::Table.indexes: Vec<Vec<i64>>`；这里只按列 ID 与顺序判断，没有索引可见性、状态、方向、前缀长度等元数据。
- 唯一原地状态变更发生在 `try_annotate_ordered_leading`：将根 Join 的 `JoinMethodHint.prefer_merge` 置真。候选搜索本身为只读。

## 依赖与调用关系

编译依赖仅为本 crate 的 `crate::util::{Expr, PlanKind, PlanNode}` 和标准库 `BTreeSet`。RustCodeGraph 给出的主要下游边为：

- `find_ordered_leading_choice → PlanNode::vertexes`
- `find_ordered_leading_choice → collect_equalities`
- `find_ordered_leading_choice → collect_leaves`
- `find_ordered_leading_choice → collect_fixed_columns`
- `find_ordered_leading_choice → index_matches_ordering`
- `find_ordered_leading_choice → OrderedLeadingChoice`（构造）
- `try_annotate_ordered_leading → PlanNode::vertexes`

当前 Rust 上游仅有测试：`ordered_leading_test.rs` 直接调用两个公开入口，`join_order_test.rs::ordered_leading_skips_fixed_index_prefix` 调用候选查找。没有已验证的 Rust 生产调用边。

Go 主链则是 `pkg/planner/core/rule/rule_order_aware_join_reorder.go` 的递归规则调用 `joinorder.FindOrderedLeadingChoice`、`TryAnnotateOrderedLeading` 和 `DsSatisfiesOrdering`；这条关系用于说明设计来源，不能作为 Rust 已接线的证据。

## 错误处理与边界

本文件没有 `Result` 或可传播错误，所有不适用和验证失败都用 `None` / `false` 表示。

- 排序列为空、非正或重复时拒绝；重复列不会被静默去重，尽管字段注释写有“去重后”，真实实现以 `seen.insert` 失败为无效输入。
- 计划必须至少包含两个顶点；叶节点只有 `PlanKind::Table` 才可能成为承载者。
- 只有两侧都为确定性表达式、且各自恰引用一列的 `Eq` 才扩展列等价；常量等值只要求常量侧不引用列且为确定性。
- 固定前缀只从 Join 的 `equal_conditions` / `other_conditions` 收集；当前 Rust 模型没有读取 Go 版 DataSource 本地条件、组上 Selection 或祖先过滤器的对应入口，也不支持 Go 版单值 `IN`。
- `try_annotate_ordered_leading` 要求根节点直接是 Join，不能像 Go 的 `findLeadingHintAnchor` 那样穿透 Selection；它只检查四个算法偏好位，不检查用户 LEADING、内部 LEADING、`HintInfo` 或表别名可桥接性。
- `reverse` 恒为 `false`，所以当前没有降序或索引反向扫描证明。

这些差异决定了本实现不能被视为 Go 逻辑的完整等价替代。

## 并发与资源生命周期

所有遍历均为同步递归，临时 `Vec` / `BTreeSet` 由单次函数调用独占并在返回时释放。模块不创建线程、任务、通道、锁、事务、文件句柄或网络资源。

`find_ordered_leading_choice` 仅借用不可变计划；返回值拥有自己的列向量，不借用计划节点。`try_annotate_ordered_leading` 通过独占 `&mut PlanNode` 修改根 hint，Rust 借用规则阻止并发无同步写入。递归深度与计划树深度一致，文件中没有显式深度限制。

## 与 Go 版本的对应关系

对应文件为 `pkg/planner/core/joinorder/ordered_leading.go`。两版共同保留的意图是：拒绝无效排序要求、选择单一承载叶表、允许确定性等值固定索引前缀、避免覆盖已有算法 hint，并只标注连接组锚点。

关键差异如下：

- Go 的候选选择先按 Schema/UniqueID 判断完整排序向量属于同一顶点，随后由递归调用在 DataSource 层用物理列 ID 和公共可见索引证明顺序；Rust 在一个函数内直接遍历所有表叶和简化索引列表。
- Go 的 `OrderedLeadingChoice` 保存 `CarrierVertex`、可生成 hint 的 `LeadingTable` 和整组 `Vertices`；Rust 只保存 `leaf_id`、排序列 ID 和恒假的 `reverse`。
- Go 合成独立的内部 LEADING hint，并区分用户 hint、内部 hint 和连接算法 hint；Rust 直接设置 `prefer_merge`，信息量和冲突检查都更少。
- Go 可穿透 Selection 寻找锚点，校验索引为 public 且非 invisible，并从 DataSource、组 Selection、祖先过滤器收集 EQ/单值 IN；Rust 不具备这些元数据与接线。
- Rust 额外从 Join 的列等值构建排序列等价集合；Go 当前 `indexMatchesOrdering` 只按物理排序列 ID 匹配，明确不做一般“a = b”推理。

源码顶部的大块注释大致复述 Go 版本 API，属于不可执行迁移参考；判断 Rust 实际能力必须以第 227—424 行为准。

## 扩展指南

- 若接入 Rust 优化器主链，应在对应 order-aware rule 的真实 Rust 执行路径调用公开入口，并新增独立测试证明候选从规则递归到 Join 标注，而不只测试局部函数。
- 若追求 Go 对齐，应优先扩充 `PlanNode` / hint 模型所需的明确能力：表别名与查询块身份、用户和内部 LEADING 的分离、Selection 锚点穿透、索引 public/invisible 状态、DataSource/祖先过滤器来源。不要仅把 `prefer_merge` 当成 LEADING 的等价物。
- 修改匹配规则时重点维护 `index_matches_ordering` 的不变量：固定列只能在第一个排序列之前跳过，排序匹配一旦开始便必须连续；同步扩展 `ordered_leading_test.rs` 或 `join_order_test.rs`，不要把测试内嵌进生产源文件。
- 若增加 `IN`、可变表达式、降序或反向扫描支持，应分别补充正反例，并让 `reverse` 由真实方向证明产生，而不是只改变返回字段。
- 当前“首个叶/首个索引胜出”依赖树和索引枚举顺序。若改成成本选择或稳定排序，需要记录选择准则和性能影响，避免无意改变计划稳定性。

## 验证依据

- Rust 源码：`pkg/planner/core/joinorder/ordered_leading.rs`，实际符号位于第 227—424 行；RustCodeGraph `node --file ... --offset 230 --limit 220` 返回了完整实现段。
- 模块与类型：`pkg/planner/core/joinorder/lib.rs`、`pkg/planner/core/joinorder/util.rs`。
- crate 边界：`pkg/planner/core/joinorder/Cargo.toml`（package `astersql-planner-core-joinorder`，库入口 `lib.rs`）。
- 独立 Rust 测试：`pkg/planner/core/joinorder/ordered_leading_test.rs`；补充固定索引前缀回归：`pkg/planner/core/joinorder/join_order_test.rs::ordered_leading_skips_fixed_index_prefix`。
- Go 对照与生产调用：`pkg/planner/core/joinorder/ordered_leading.go`、`pkg/planner/core/rule/rule_order_aware_join_reorder.go`；Rust 对应 rule 中仅有注释对照：`pkg/planner/core/rule/rule_order_aware_join_reorder.rs`。
- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`query` 唯一定位 `find_ordered_leading_choice`（第 239 行）、`try_annotate_ordered_leading`（第 287 行）和 `index_matches_ordering`（第 399 行）。`callees` 证实前述内部调用边；`explore` 的 blast radius 与仓库搜索共同证实上游仅为 Rust 测试。
- 人工复核结论：文档区分了不可执行的迁移注释、当前 Rust 局部实现和已接线的 Go 主链，并列明安全扩展时必须同步的独立测试与兼容风险。

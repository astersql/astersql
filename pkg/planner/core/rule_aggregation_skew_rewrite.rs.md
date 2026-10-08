# `pkg/planner/core/rule_aggregation_skew_rewrite.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 根在 `pkg/planner/core/lib.rs` 中以 `pub mod rule_aggregation_skew_rewrite` 公开该模块，并在测试配置下加载独立文件 `rule_aggregation_skew_rewrite_test.rs`。它把 Go 文件 `pkg/planner/core/rule_aggregation_skew_rewrite.go` 的倾斜 DISTINCT 聚合规则移植到 `rule_aggregation_elimination.rs` 定义的简化 `LogicalPlan`/`LogicalAggregation` 模型上，主要用于可独立验证的规则语义。

需要区分“公开模块”和“生产优化主链”：仓库当前真实逻辑优化调度由 `pkg/planner/core/optimizer_runtime.rs` 的 `LOGICAL_RULES`、`LOGICAL_RULE_FLAGS` 与 `rewrite_skew_distinct_aggregation_descendants` 完成，使用的是 `logicalop::LogicalPlanRef`。代码搜索仅发现本文件被 `rule_aggregation_skew_rewrite_test.rs` 直接使用；因此不能把本文件的 `SkewDistinctAggRewriter::Optimize` 描述成当前 SQL 优化器直接调用的入口。

## 核心职责

- `rewriteSkewDistinctAgg` 判断一个分组聚合是否满足改写条件，并将其拆成 Bottom/Top 两层聚合，以“原分组键 + DISTINCT 参数”作为 Bottom 分组键，分散单一高倾斜分组的聚合压力。
- DISTINCT 聚合在 Bottom 层变为 `FirstRow`，在 Top 层取消 `distinct` 后恢复原聚合名；普通 `Count` 在 Top 层改为 `Sum`，再用投影表达式表示向原 COUNT 返回类型的转换。
- `isQualifiedAgg` 集中表达规则白名单和输入形状限制，阻止排序聚合、多参数聚合、非 Complete 模式或复杂标量表达式进入改写。
- `Optimize` 在简化计划树中后序递归，并仅对 `Aggregation` 节点调用改写；`Name` 提供与 Go 规则相同的注册名。

该规则是有条件的等价改写，不负责判断会话变量是否启用规则；真实主链的启用由 `optimizer_runtime.rs` 中 `FLAG_SKEW_DISTINCT_AGG` 对应的调度位控制。

## 主要符号

- `pub struct SkewDistinctAggRewriter { next_column_id: usize }`：无外部资源的有状态改写器。`Default` 从列号 0 开始；每生成一个中间聚合输出列，计数器递增。
- `pub fn rewriteSkewDistinctAgg(&mut self, agg: &LogicalAggregation) -> Option<LogicalPlan>`：核心改写入口。`None` 表示不适用，`Some` 返回两层聚合，普通 COUNT 存在时最外层再包一层 `Projection`。
- `pub fn isQualifiedAgg(&self, function: &AggFuncDesc) -> bool`：逐个聚合函数判定。允许 `FirstRow`、`Count`、`Sum`、`Max`、`Min`；`Avg` 仅在自身为 DISTINCT 时允许；显式拒绝位聚合和其他函数。
- `pub fn Optimize(&mut self, plan: LogicalPlan) -> Result<(LogicalPlan, bool)>`：对 `Aggregation`、`Projection`、`Join`、`UnionAll`、`Expand` 递归，对其他节点原样返回。这里的 `Result` 是 `rule_aggregation_elimination.rs` 中 `Result<T> = std::result::Result<T, String>`。
- `pub fn Name(&self) -> &'static str`：返回固定标识 `skew_distinct_agg_rewrite`。
- `fn synthetic_column(next: &mut usize, field_type: &FieldType) -> Expression`：私有辅助函数，生成名为 `agg_<id>`、携带列号和返回类型的中间列表达式。

## 执行流程

1. `Optimize` 先递归处理子节点，再在当前节点为 `LogicalPlan::Aggregation` 时调用 `rewriteSkewDistinctAgg`；这与 Go `Optimize` 的“先子后父”顺序一致。
2. `rewriteSkewDistinctAgg` 首先拒绝无 `GROUP BY` 的聚合，然后收集 `distinct == true` 的函数。只有恰好一个 DISTINCT 函数、该函数恰好一个参数、并且全部聚合均通过 `isQualifiedAgg` 时继续。
3. Bottom 分组项复制原 `group_by_items`，再无条件追加 DISTINCT 参数。因此即使该参数已经是原分组项，也会保留重复项；独立测试 `rewrite_preserves_go_modes_and_duplicate_group_items` 固定了这一行为。
4. 对 DISTINCT 函数，Bottom 克隆体改成非 DISTINCT、Complete 模式的 `FirstRow`；Top 克隆体保持原函数名，但取消 DISTINCT、设为 Complete，并令参数指向新生成的中间列。
5. 对非 DISTINCT 函数，Bottom 保留原函数克隆；Top 改为读取一个合成列。普通 `Count` 的 Top 函数名改为 `Sum` 并记录需要投影；`FirstRow` 若直接读取原分组列，则从待补列集合中移除该列。
6. 对仍未被已有 `FirstRow` 输出覆盖的原分组列，Bottom 追加 `FirstRow(group)`，使 Top 的原分组键仍有可用输出。
7. 构造 Bottom 聚合并将原 child 接入，再构造 Top 聚合。两层都设置 `no_eliminate: true`，防止后续聚合消除破坏该结构。
8. 若包含普通 COUNT，最外层创建投影：COUNT 对应输出使用字符串形式 `cast(col_<id>)`，其余使用 `col_<id>`；否则直接返回 Top。

## 数据与状态

输入及输出均基于 `rule_aggregation_elimination.rs` 的拥有型简化计划枚举。改写读取原聚合的 `agg_funcs`、`group_by_items`、`schema`、`output_columns` 和 `child`，通过克隆构造新节点，不原地修改传入的 `&LogicalAggregation`。

`first_row_columns: HashSet<_>` 保存原分组表达式中可直接识别的列号，用于判断哪些分组列需要在 Bottom 补 `FirstRow`。只有 `Expression.column` 为 `Some` 的分组项进入集合；复杂分组表达式不会由此集合展开为底层列。

`next_column_id` 是改写器实例级可变状态。它只保证同一实例连续生成的合成列号递增，并不知道输入计划现有列 ID 的全局范围；这与 Go 通过 session 分配计划列 ID 不同，也是把本实现接入真实计划模型前必须解决的边界。返回节点沿用原 `schema` 与 `output_columns`；Bottom 并未建立与新增函数逐项对应的新 schema，而是依赖这套简化模型的测试约定。

## 依赖与调用关系

直接依赖仅有三组：`rule_aggregation_elimination` 提供聚合描述、简化逻辑计划和错误别名；`task::Expression`（以及 `synthetic_column` 签名中的 `task::FieldType`）提供表达式和类型；标准库 `HashSet` 跟踪分组列。

RustCodeGraph 将 `SkewDistinctAggRewriter`、`isQualifiedAgg`、`synthetic_column` 定位到本文件，并报告本文件由 `pkg/planner/core/rule_aggregation_skew_rewrite_test.rs` 使用。仓库文本调用检查还显示 `lib.rs` 负责模块公开，但真实优化调度在 `optimizer_runtime.rs`：`LogicalRule::SkewDistinctAgg` 与 `rule::FLAG_SKEW_DISTINCT_AGG` 按 Go `optRuleList` 顺序配对，最终调用另一套 `rewrite_skew_distinct_aggregation_descendants`。Go 版本则由 `optimizer.go` 的 `optRuleList` 直接注册 `&SkewDistinctAggRewriter{}`。

`pkg/planner/core/Cargo.toml` 将此文件编入 `astersql-planner-core` 的 `lib.rs`，声明 `autotests = false`，因此相邻测试通过 `lib.rs` 的 `#[cfg(test)] mod rule_aggregation_skew_rewrite_test` 纳入，而不是 Cargo 自动发现；该规则自身没有 feature 条件。

## 错误处理与边界

不满足适用条件属于正常分支，`rewriteSkewDistinctAgg` 返回 `None`，不产生错误。明确拒绝的情况包括：无分组项、DISTINCT 函数不是恰好一个、DISTINCT 参数不是单参数、非 Complete 模式、有 `order_by`、超过一个参数、参数既不是列也不是零函数计数的简单值，以及白名单外函数。`Avg` 只有 DISTINCT 形式可通过，因为普通 AVG 不能按当前分解方式直接合并。

常量参数以 `column == None && function_count == 0` 的形式被接受；这对应 Go 对常量 DISTINCT 参数的兼容修复。真实 SQL 回归在 `pkg/planner/core/issuetest/panicrisk_tier2_test.rs` 与同名 Go 测试中覆盖 `count(distinct 1)`、`sum(distinct 2)`，防止把常量强制当列而崩溃或改变结果。

本文件没有主动构造 `Err` 的路径；`Optimize` 的 `?` 只传播递归调用的 `Result`。需注意其 `bool` 不是“当前节点发生改写”的可靠标志：当前聚合成功改写仍返回 `child_changed`；Join 忽略左侧标志，UnionAll 每轮覆盖而非累积标志。这与 Go 循环内覆盖 `planChanged` 的现有契约相近，测试 `optimize_reports_only_descendant_change_like_go_rule_contract` 明确要求根聚合被改写时仍为 `false`，扩展时不应擅自“修正”而破坏对齐。

## 并发与资源生命周期

代码不启动线程、异步任务、通道、事务，也不持有锁、文件或网络资源；所有计划节点和表达式随 Rust 所有权自动释放。`Optimize` 消费输入计划并重建拥有型子树，递归深度与计划树深度一致。

`SkewDistinctAggRewriter` 因 `next_column_id` 需要 `&mut self`，单个实例不设计为并发共享。若跨多个计划复用同一实例，列号会继续递增；若每次 `Default` 新建，则重新从 0 开始。真实运行时实现改用表达式上下文的 `AllocPlanColumnID`，其生命周期归属 session/优化上下文，不能以本地计数器直接替换。

## 与 Go 版本的对应关系

核心意图与 Go `rule_aggregation_skew_rewrite.go` 一致：至少一个分组项、恰好一个 DISTINCT 聚合、聚合白名单、Bottom 追加 DISTINCT 键、Bottom/Top 两级聚合、普通 COUNT 以 Bottom COUNT + Top SUM 合并并恢复输出类型、缺失分组列以 `FirstRow` 补出，以及同名规则标识。

本 Rust 文件是简化模型上的语义移植，并非逐类型复刻。Go 使用 `logicalop.LogicalAggregation`、真实 schema、session 列 ID、表达式构造器及可能失败的 `NewAggFuncDesc`/cast；本文件使用字符串化表达式、局部列号和克隆的原 schema。Go 的 DISTINCT 参数收集发生在资格检查之前，但随后只接受单参数；本文件直接要求唯一 DISTINCT 函数的参数数为 1。Go 会复制 `PreferAggType`/`PreferAggToCop` 等物理偏好，本简化类型没有对应字段。

此外，当前真实 Rust 优化实现位于 `optimizer_runtime.rs`，它会去重已存在于 Bottom 分组中的 DISTINCT 参数，而本文件及其独立测试保留 Go 文件的无条件追加行为；真实实现也直接使用生产表达式/schema 类型。维护者修改语义时应同时核对这三处（Go、简化移植、真实 Rust 调度实现），不能仅凭本文件推断线上行为。

## 扩展指南

- 放宽 DISTINCT 数量或多参数 DISTINCT 时，优先修改资格判定与 Bottom 分组/输出映射，再同步 `rewriteSkewDistinctAgg` 中 DISTINCT 分支；需要新增 `rule_aggregation_skew_rewrite_test.rs` 的拒绝与成功用例，并核对 Go 的 TODO 和生产 `optimizer_runtime.rs`。
- 新增可分解聚合函数时，在 `isQualifiedAgg` 白名单之外还必须定义 Bottom/Top 合并方式和返回类型恢复规则；仅放开枚举会造成错误结果。AVG、位聚合及带 ORDER BY 聚合尤其需要独立的部分状态设计。
- 调整 COUNT 投影时，应验证合成列与 `agg.output_columns`/`schema` 的索引关系；当前代码假定输出列数量和聚合函数数量可按 `offset` 一一对应。
- 若要把本类型接入真实优化主链，必须先替换简化 `LogicalPlan`、字符串 cast、本地列 ID 和复用原 schema 的做法，并处理生产上下文中的错误、输出名、偏好属性及规则 flag；更安全的方向是保持 `optimizer_runtime.rs` 为唯一生产实现，让本文件继续承担隔离语义测试或最终消除重复实现。
- Rust 测试继续放在独立的 `rule_aggregation_skew_rewrite_test.rs`，不要内嵌到生产文件；SQL 级常量回归位于 `issuetest/panicrisk_tier2_test.rs`，真实调度结构回归位于 `optimizer_logical_entry_aster_unit_test.rs`。
- 性能风险主要来自增加一层聚合和可能的投影；正确性风险集中在聚合可分解性、NULL/常量语义、类型转换、列 ID 冲突和 schema 映射；兼容性风险则是与 Go 行为或生产 Rust 实现发生漂移。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`node --file pkg/planner/core/rule_aggregation_skew_rewrite.rs` 展示完整 257 行源码并报告测试文件使用关系；`query` 确认 `SkewDistinctAggRewriter`、`isQualifiedAgg`、`synthetic_column` 的路径与签名。精确 `callers`/`callees` 查询没有返回额外调用边，因此又以模块声明和仓库调用点搜索核验接线，未据此臆造生产调用关系。
- 生产源码：`pkg/planner/core/rule_aggregation_skew_rewrite.rs`；简化数据类型来源：`pkg/planner/core/rule_aggregation_elimination.rs`、`pkg/planner/core/task.rs`。
- crate 与模块边界：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。
- Go 对照及注册：`pkg/planner/core/rule_aggregation_skew_rewrite.go`、`pkg/planner/core/optimizer.go`。
- 真实 Rust 优化接线：`pkg/planner/core/optimizer_runtime.rs` 中 `LogicalRule::SkewDistinctAgg`、`LOGICAL_RULES`、`LOGICAL_RULE_FLAGS`、规则分派与 `rewrite_skew_distinct_aggregation_descendants`。
- 独立 Rust 单元测试：`pkg/planner/core/rule_aggregation_skew_rewrite_test.rs`，覆盖资格判定、重复分组项、COUNT 投影和 changed 标志；集成式调度测试：`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs`；常量参数 Rust/Go 回归：`pkg/planner/core/issuetest/panicrisk_tier2_test.rs` 与 `.go`。
- 本任务为纯文档分析，依计划未运行 Cargo；最终结构检查应确认本文恰有规定的十一个二级标题。

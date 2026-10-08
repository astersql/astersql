# [`pkg/planner/core/rule/rule_column_pruning.rs`](./rule_column_pruning.rs)

## 文件定位

该文件位于 `astersql-planner-core-rule` crate，由 `pkg/planner/core/rule/lib.rs` 以公开模块 `rule_column_pruning` 导出。它在 `rule_init.rs` 定义的精简逻辑计划 IR（`Plan`、`PlanKind`、`Expr`）上实现列裁剪规则，用于表达“从根节点所需输出列向子树反向传播列需求”的移植语义。

需要区分这份精简规则与当前完整优化器的实际入口：`pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place` 对 `LogicalRule::PruneColumns` 和 `PruneColumnsAgain` 直接调用 `logicalop::LogicalPlanRef::PruneColumns`，没有构造本文件的 `ColumnPruner`。仓库中的精确引用只在模块声明和 Rust 独立测试中出现。因此，本文件目前是可测试的规则侧精简实现，而不是完整应用逻辑优化主链的直接执行实现。

`pkg/planner/core/rule/Cargo.toml` 指定 crate 入口为 `lib.rs`，普通依赖包含 logicalop 与 rule-util 等本地 crate；本文件自身只直接使用同 crate 的 `rule_init` 和标准库 `BTreeSet`，没有条件编译项或文件级外部资源。

## 核心职责

- `ColumnPruner` 实现 `LogicalRule`，提供稳定规则名 `column_prune` 和统一的 `optimize` 调用形态。
- `optimize` 把根计划当前 schema 的全部列作为初始必需列，随后调用私有递归函数 `prune`。
- `prune` 先裁剪当前节点 schema，再根据谓词和具体算子语义计算子节点所需列，最后对子树递归执行同样过程。
- 对非 `PlanKind::TableDual` 的节点，若裁剪后 schema 为空，则尝试回填一个列 ID，以维持“普通算子至少暴露一列”的局部不变量。
- 规则内部会计算是否发生 schema 长度变化，但为对齐 Go `ColumnPruner.Optimize` 的返回契约，公开 `optimize` 始终返回 `false` 作为 `plan_changed`。

它不会执行表达式重写、谓词下推、键重算、统计更新、物理计划选择或完整 logicalop 类型分派；这些都不属于该精简文件的职责。

## 主要符号

- `pub struct ColumnPruner`：无字段的规则对象，没有实例状态；公开是为了让同 crate 测试或未来接线方构造规则。
- `impl LogicalRule for ColumnPruner`：实现 `rule_init.rs::LogicalRule` 要求的两个方法。
  - `name(&self) -> &'static str` 返回 `"column_prune"`。这是规则自身对齐 Go `Name` 的标识；`rule_init.rs::default_rule_names` 中列出的 `"column_pruner"` 是另一份默认名称清单，两者当前并未由代码绑定。
  - `optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String>` 获取计划所有权，原地裁剪后返还计划；错误类型沿用 trait 的 `String`。
- `fn prune(plan: &mut Plan, required: &BTreeSet<i64>) -> Result<bool, String>`：文件私有的深度优先递归实现。返回值表示本节点或后代是否发生 schema 长度变化，但当前唯一调用方 `optimize` 不向外传播该布尔值。
- 本文件无模块级常量、类型别名、额外 trait、宏或条件编译代码。顶部大段注释保存了较早的 Go 形态说明，不参与编译；可执行定义从 `use crate::rule_init...` 开始。

## 执行流程

1. `ColumnPruner::optimize` 将根节点 `plan.schema` 中的列 ID 收集为有序去重的 `BTreeSet`。根输出因此不会因本规则自行缩减；真正的裁剪发生在下层不再被根输出或算子语义引用的列上。
2. `prune` 克隆裁剪前的 `original_schema`，记录旧长度，然后只保留 `required` 中出现的当前 schema 列。克隆仅用于 Projection 输出位置到表达式位置的映射。
3. 若结果为空且节点不是 `TableDual`，函数优先取 `required` 中最小的列 ID；若没有，再取第一个子节点 schema 遍历得到的首列，并将其回填。若两处都没有列可取，schema 仍会为空，函数不会报错。
4. 一般情况下，`child_required` 初始包含当前节点所有 `predicates` 中的列引用，再并入上层传入的 `required`。
5. `Projection { expressions }` 会清空一般集合，只遍历投影表达式：按 `original_schema` 的同一偏移找到输出列，仅当该输出列被上层需要时，才把对应表达式引用的输入列加入需求。使用裁剪前 schema 是关键不变量，否则先 `retain` 后偏移会错位。
6. `Aggregation` 在一般需求上加入全部 `group_by` 表达式和全部聚合参数引用的列；`Sort` 加入排序表达式列；`Join` 加入等值条件和其它连接条件列。其余 `PlanKind` 不附加专属需求。
7. 对每个子节点，先用 `child.all_columns()` 取得该子节点当前 schema 集合，再与 `child_required` 求交集，避免把不属于这个子树的列传给它；随后递归调用 `prune`。
8. 函数用逻辑或累计当前节点及后代的长度变化并返回。`optimize` 只检查递归结果是否为 `Err`，成功时返回 `(plan, false)`。

## 数据与状态

- 列身份统一使用 `i64`，没有列对象、来源表或类型信息；算法假设列 ID 足以跨节点关联引用。
- `Plan.schema: Vec<i64>` 保留原有相对顺序，因为裁剪通过 `Vec::retain` 完成；`BTreeSet` 只负责需求集合的去重、求交和确定性遍历。
- `original_schema` 是每个递归栈帧的临时快照，保证 Projection 的输出偏移映射不受当前节点裁剪影响。
- `child_required` 是每个节点独立构造的临时集合；所有子节点共享语义需求，但递归前分别与各自 schema 求交，因此 Join 两侧只收到自己能够提供的列。
- `Plan.predicates`、`Plan.kind` 中的表达式通过 `Expr::columns()` 收集列引用。`keys`、`estimated_rows`、`used_stats` 以及算子其它字段均保持不变。
- 唯一持久变化是传入计划树各节点的 `schema`；`ColumnPruner` 自身没有缓存、计数器或全局状态。

## 依赖与调用关系

上游方面，`pkg/planner/core/rule/lib.rs` 导出该模块；已核实的直接构造和调用来自 `rule_column_pruning_test.rs::column_pruner_preserves_go_rule_contract` 与 `rule_aster_unit_test.rs::column_pruning_uses_original_projection_offsets`。`rule_init.rs::default_rule_names` 含列裁剪相关字符串，但只返回名称列表，并不实例化或调用 `ColumnPruner`。完整 Rust 优化主链在 `optimizer_runtime.rs::logical_optimize_in_place` 走 logicalop 的 `PruneColumns`，所以不能据此宣称本文件已接入生产入口。

下游方面，`optimize` 直接调用本文件的 `prune`；`prune` 依赖 `Plan::all_columns`、`Expr::columns`、`PlanKind` 分支以及 `BTreeSet` 的集合操作。它不调用 Cargo.toml 中列出的外部 crate API，也不进行 I/O、日志、异步调度或数据库访问。

RustCodeGraph 将文件识别为 7 个符号，并确认 `optimize -> prune` 调用边；对 trait 同名方法运行 `callers/callees` 时出现跨仓库同名符号噪声，故上游接线结论还以 `lib.rs`、精确 `rg` 引用和 `optimizer_runtime.rs` 的实际分派代码交叉核验。

## 错误处理与边界

- 签名允许返回 `Result<_, String>`，`optimize` 使用 `?` 传播 `prune` 错误；但当前 `prune` 没有任何 `Err` 构造路径，因此对当前实现而言错误通道是为 trait/未来扩展保留的。
- `TableDual` 明确允许零列 schema。其它节点被裁空时只做“尽力回填”：若 `required` 与所有直接子节点 schema 都为空，仍会留下空 schema，且没有 Go 版本的递归断言。
- 回填优先从 `required` 取列，而不检查它是否原先属于当前 schema；调用者若传入不一致的需求集合，可能把一个此前不在节点输出中的列 ID 加入 schema。当前递归对子节点会先求交，但根调用与父节点自身的回填依赖 IR 构造正确性。
- Projection 只为仍需输出的表达式保留输入列，并特意使用 `original_schema` 偏移。`rule_aster_unit_test.rs` 的回归测试验证从输出列 `200` 正确追溯到输入列 `20`，防止裁剪后偏移错位。
- Projection 分支会清空此前由节点谓词建立的 `child_required`。按当前精简 IR 的预期，Projection 的输入需求由投影表达式决定；若未来允许 Projection 自身携带需独立求值的谓词，必须重新审视该行为。
- 未知或新增 `PlanKind` 落入 `_`，只继承一般的上层需求与谓词列，不会自动理解算子专属表达式。
- `changed` 仅比较 schema 长度，不检测内容替换；并且公开结果固定为 `false`，调用方不得用该返回值判断实际 schema 是否变化。

## 并发与资源生命周期

该规则完全同步、单线程运行。它独占传入的 `Plan`，通过可变借用深度优先遍历子树，没有锁、原子变量、通道、任务、线程局部状态或共享引用更新，因此文件内部不存在并发协调协议。

每层递归会分配 `original_schema`、`child_required`、子节点列集合和交集集合；这些临时值在该栈帧返回时释放。递归深度等于计划树深度，极深的人造计划可能增加栈使用。根 `Plan` 的所有权在 `optimize` 调用期间进入规则，成功时随返回值交还；当前实现没有可能在错误分支中保留外部资源。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/rule/rule_column_pruning.go`。两边都提供无状态 `ColumnPruner`，规则名均为 `column_prune`，都以根 schema 列为起点执行裁剪，并都按 Go 契约把 `planChanged` 报告为 `false`。Rust 独立测试 `column_pruner_preserves_go_rule_contract` 明确锁定了规则名、子节点列裁剪结果和该布尔值。

实现层次存在重要差异：Go `Optimize` 克隆根 schema 后调用每个真实逻辑算子的多态 `lp.PruneColumns`，因此具体算子的完整裁剪规则分散在 logicalop 实现中；Rust 本文件则在精简 `PlanKind` 枚举上集中实现 Projection、Aggregation、Sort 和 Join 四类专属规则。Go 还用 `intest.AssertFunc(noUnexpectedZeroColumnSchema)` 递归验证裁剪后 schema，并豁免复用首个子节点 schema 的节点与 `LogicalTableDual`；Rust 没有同等断言，只对 `TableDual` 特判并尝试回填一列，也没有表达“schema 对象身份复用”的能力。

Go 主优化器 `pkg/planner/core/optimizer.go` 在规则列表前段和靠后位置各注册一次 `&rule.ColumnPruner{}`，第二次裁剪位于构建 key 信息之后；当前 Rust 完整入口同样以 `PruneColumns`/`PruneColumnsAgain` 表示两次阶段，但执行的是 logicalop `PruneColumns`，不是本文件的精简 trait 实现。因此，本文件可用于语义移植和单元验证，但不能等同于 Go 完整算子覆盖率。

在检索到的 Go 测试中没有直接点名 `ColumnPruner` 或 `noUnexpectedZeroColumnSchema` 的独立测试；相关 Go 行为主要由各 logicalop 的 `PruneColumns` 测试及更高层优化器测试间接覆盖。本文不据此推断未逐项读取的 Go 测试结果。

## 扩展指南

- 新增算子专属列需求时，优先在 `prune` 的 `match &plan.kind` 中增加明确分支，并同步检查 `rule_init.rs::PlanKind` 是否携带足够的表达式/列元数据；不要仅依赖 `_` 的一般传播。
- 修改 Projection 时必须保留“基于裁剪前 `original_schema` 映射表达式偏移”的约束，并扩展独立文件 `pkg/planner/core/rule/rule_aster_unit_test.rs` 中的偏移回归测试。
- 修改公开规则契约时，同步更新 `pkg/planner/core/rule/rule_column_pruning_test.rs`，尤其是 `name() == "column_prune"` 和外部 `plan_changed == false`；若有意偏离 Go，需同时记录差异理由与上游调用影响。
- 若让错误通道真正生效，应在 `prune` 返回具有节点/算子上下文的消息，并补充错误传播测试；若维持永不失败，可评估是否仍需 `Result`，但这属于共享 `LogicalRule` trait 设计而非单文件局部决定。
- 若要求与 Go 的零列不变量完全对齐，需要在精简 IR 中先明确 schema 共享身份的表示方式，再实现递归校验；不能把当前回填逻辑直接描述为 Go 断言的等价实现。
- 若要接入完整优化器，应先处理精简 `Plan` 与 `logicalop::LogicalPlanRef` 的架构边界，并明确是否替换 `optimizer_runtime.rs` 现有 `PruneColumns` 分派。仅向 `default_rule_names` 增加或修改字符串不会形成实际接线。
- 性能上应关注每节点 schema 克隆和多个 `BTreeSet` 分配；任何改为哈希集合或复用缓冲区的优化都必须保持输出顺序、Projection 偏移语义与确定性测试结果。
- Rust 测试必须继续放在独立 `*_test.rs` 文件，不应内嵌回本源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/planner/core/rule/rule_column_pruning.rs` 报告该文件有 7 个符号；`node --file ... --offset 1 --limit 500` 读取了完整 153 行；符号查询识别 `ColumnPruner`、`name`、`optimize`、`prune`，调用图确认 `optimize` 调用 `prune`。同名 trait 方法的精确 callers/callees 查询产生跨文件噪声，未作为唯一上游证据。
- Rust 源与模块边界：`pkg/planner/core/rule/rule_column_pruning.rs`、`pkg/planner/core/rule/rule_init.rs`、`pkg/planner/core/rule/lib.rs`、`pkg/planner/core/optimizer_runtime.rs`。
- crate 声明：`pkg/planner/core/rule/Cargo.toml`，核实 package 名、`lib.rs` 入口、普通依赖与 Windows 条件依赖；目标文件没有直接使用外部依赖。
- Go 对照与主链：`pkg/planner/core/rule/rule_column_pruning.go`、`pkg/planner/core/optimizer.go`，核实 `Optimize`、`Name`、零列递归检查以及两次规则注册。
- Rust 独立测试：`pkg/planner/core/rule/rule_column_pruning_test.rs::column_pruner_preserves_go_rule_contract`；`pkg/planner/core/rule/rule_aster_unit_test.rs::column_pruning_uses_original_projection_offsets`。
- 精确文本检索：对 `rule_column_pruning|ColumnPruner|column_prune` 的仓库局部引用确认本文件的构造调用仅见测试；对 `default_rule_names`、`LogicalRule` 和优化入口的检索确认名称清单与完整运行时分派是不同接线面。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；最终以任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核当前事实、限制、扩展入口和测试位置均有直接来源。

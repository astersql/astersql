# `pkg/planner/core/rule_result_reorder.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；`pkg/planner/core/Cargo.toml` 以 `lib.rs` 为库入口，而 `pkg/planner/core/lib.rs` 通过 `pub mod rule_result_reorder` 公开本模块，并只在 `cfg(test)` 下装入独立测试 `rule_result_reorder_test.rs`。模块提供 Go `pkg/planner/core/rule_result_reorder.go` 的轻量 Rust 语义镜像：它在简化的 `crate::task::PlanNode` 树上补全或注入排序，以使无显式完整顺序的查询结果可复现。

当前接线必须与生产实现区分：RustCodeGraph 显示 `ResultReorder` 的直接使用者只有 `pkg/planner/core/rule_result_reorder_test.rs`，仓库文本搜索也未发现生产调用。Rust 生产逻辑优化流水线在 `pkg/planner/core/optimizer_runtime.rs` 中以 `LogicalRule::StabilizeResults -> stabilize_results` 实现同一规则，操作真实的 `logicalop::LogicalPlanRef`；因此本文件不是当前生产流水线的实际执行入口。

## 核心职责

- `ResultReorder::Optimize` 从计划根部越过保持输入顺序的节点，判断第一处实质算子是否已有 `Sort`。已有排序时补齐稳定排序键，否则在该位置插入一个 `Sort`。
- 稳定键优先采用普通 `DataSource` 暴露的整型 handle；无法可靠提取 handle 时退化为按当前节点 schema 的全部列下标排序。
- 规则刻意返回 `changed == false`，即使它修改或替换了计划。这与 Go `Optimize` 中始终为 `false` 的 `planChanged` 保持一致，不能把该布尔值解释成结构是否真的变化。
- 规则只承诺确定输出顺序，不做代价判断，也不判断新增排序的执行成本。Go 注释明确说明这是面向少数客户的特殊规则，而不是默认适用于所有查询的通用优化。

## 主要符号

- `pub struct ResultReorder`：无字段、可 `Default` 构造的规则对象，本身不持有会话或计划状态。
- `pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool)`：所有权式入口。先调用 `completeSort`，仅在其返回 `false` 时调用 `injectSort`，最后固定返回 `(计划, false)`。
- `pub fn completeSort(&self, p: &mut PlanNode) -> bool`：原地检查/补全已有排序。它越过 `Projection`、`Selection`、`Limit`；遇到 `Sort` 时追加缺失键并返回 `true`；遇到其他种类返回 `false`。无孩子的保序节点也返回 `true`。
- `pub fn injectSort(&self, p: PlanNode) -> PlanNode`：越过保序外壳，在第一个不保序节点之上构造 `PlanKind::Sort`。新排序复制被包裹节点的 schema，并将原节点作为唯一孩子。
- `pub fn isInputOrderKeeper(&self, p: &PlanNode) -> bool`：把 `Projection`、`Selection`、`Limit` 认定为保序节点。
- `pub fn extractHandleCol(&self, p: &PlanNode) -> Option<Expression>`：对 `Selection`、`Limit` 递归向下；对带 `flags.from_data_source` 且没有 `common_handle` 标签的节点，从首个可解析的 `handle:<usize>` 标签构造列表达式。
- `fn schemaColumns(count: usize) -> Vec<Expression>`：模块私有回退函数，生成列下标为 `0..count`、名称为 `column:<下标>` 的表达式。
- `pub fn Name(&self) -> &'static str`：返回与 Go 规则相同的注册名 `result_reorder`。本文件没有 trait 实现、模块常量或条件编译项。

## 执行流程

1. 调用者把 `PlanNode` 所有权交给 `Optimize`。
2. `completeSort` 从根开始：若当前节点保序，则只沿第一个孩子递归；若没有孩子，`Option::is_none_or` 使该分支视为已经完整。非保序节点只有 `PlanKind::Sort` 能结束搜索并被认定为已有完整排序。
3. 对已有 `Sort`，先尝试从它的第一个孩子提取 handle。成功时目标键只有该 handle；失败或没有孩子时目标键是排序节点 schema 的全部列。
4. 对每个目标键，按 `Expression.column` 与现有 `by_items` 比较，只追加下标尚未出现的键，保留原有键的顺序和内容。
5. 若步骤 2 未找到完整排序，`injectSort` 再次越过保序节点，并递归替换其第一个孩子；到达首个不保序节点后，优先按 handle、否则按全部 schema 列创建 `Sort`。
6. `Optimize` 返回可能已原地补全或已被新根替换的计划，但变化标志固定为 `false`。

独立测试 `optimize_injects_below_order_keepers_and_reports_go_changed_flag`、`complete_sort_extends_existing_sort_like_go`、`leaf_input_order_keeper_is_already_complete` 和 `top_n_is_not_a_complete_sort_in_the_go_rule` 分别固定了上述注入位置、键补全、叶保序节点以及 `TopN` 不算完整排序的行为。

## 数据与状态

规则对象无内部状态；所有变化都发生在传入的 `PlanNode` 及其 `children`、`by_items`、`schema`、`flags`、`labels` 上。`completeSort` 借用并原地追加排序键，`injectSort` 则取得节点所有权并重建局部树；越过保序节点时使用 `std::mem::take` 暂时以 `PlanNode::default()` 替换第一个孩子，再把递归结果放回。

排序键以轻量 `Expression` 表示。去重只比较可选列下标，不比较 `name`、返回类型或其他表达式字段；因此两个 `column == None` 的表达式会被视作相同，而 handle 与 schema 回退键都由本文件构造为 `Some(index)`。handle 的存在通过 `PlanFlags::from_data_source` 和 `labels` 中的字符串协议表达；`common_handle` 键存在时明确禁用该优化捷径。标签是 `HashMap`，若存在多个合法 `handle:` 键，`find_map` 选择哪一个不保证稳定，所以正常调用方应维持“最多一个有效 handle 标签”的隐含不变量。

## 依赖与调用关系

直接下游只有 `crate::task::{Expression, PlanKind, PlanNode}` 和标准库能力；本文件没有直接使用 `Cargo.toml` 声明的外部 crate 或 feature。`PlanNode` 提供计划树、schema、排序项、数据源标志与标签，`PlanKind` 提供算子分类，`Expression.column` 承担列身份。

RustCodeGraph 对 `rule_result_reorder.rs` 的文件节点报告唯一使用文件为 `rule_result_reorder_test.rs`；符号查询也只找到该独立测试对 `ResultReorder` 的导入与调用。模块由 `lib.rs` 公开，但没有证据表明当前生产路径实例化它。真实 Rust 主链位于 `optimizer_runtime.rs`：`LOGICAL_RULES` 把 `StabilizeResults` 放在列裁剪之后，规则位 `FLAG_STABILIZE_RESULTS` 命中时调度 `stabilize_results`，后者调用生产模型版本的 `complete_result_sort`、`inject_result_sort` 和 `extract_result_handle_column`。

Go 主链在 `pkg/planner/core/optimizer.go`：`optRuleList` 实例化 `ResultReorder`，对应位是 `rule.FlagStabilizeResults`；`adjustOptimizationFlags` 仅在 `checkStableResultMode` 为真时启用该位，而后者排除 INSERT、UPDATE、DELETE 和 LOAD DATA 语句。会话系统变量 `tidb_enable_ordered_result_mode` 负责设置 `EnableStableResultMode`。这些是 Go/生产实现的启用条件，不是本轻量文件自身实现的检查。

## 错误处理与边界

本 API 没有 `Result` 错误通道，也不会主动 panic；缺孩子、缺 handle、标签解析失败都通过 `Option` 分支安全退化。缺 handle 时按全部输出列排序，空 schema 则生成零个 `by_items` 的 `Sort`。`extractHandleCol` 穿过 `Selection`/`Limit` 后会检查 handle 下标仍小于当前 schema 长度，模拟投影内联后列可能消失的保护。

边界差异包括：`TopN` 不是 `Sort`，会在其外层再包排序；`Projection` 虽可被遍历为保序节点，却不会被 `extractHandleCol` 穿透；common handle 明确不支持。直接调用 `injectSort` 处理无孩子的保序节点时会原样返回，而 `Optimize` 通常先由 `completeSort` 将这种叶节点判定为完成。已有无孩子 `Sort` 在 Rust 中会按自身 schema 补键；Go 实现直接访问 `sort.Children()[0]`，依赖真实逻辑计划的 Sort 必有孩子不变量。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。`ResultReorder` 是无状态零尺寸类型；每次调用只处理调用者独占的 `PlanNode`，并发安全取决于调用方不共享同一所有权对象，Rust 类型系统在这里排除了并发可变别名。

资源成本来自树递归和分配：搜索/注入只沿每层的第一个孩子，递归深度等于根部连续保序链长度；新排序会克隆 schema、分配排序键和孩子向量。补键对每个候选列线性扫描现有 `by_items`，最坏复杂度约为 `O(schema 列数 × 排序键数)`。极深的人工计划链理论上可能消耗较多调用栈，但文件没有迭代式保护或深度上限。

## 与 Go 版本的对应关系

Rust 的六个方法与 Go `ResultReorder` 基本逐一对应：`Optimize`、`completeSort`、`injectSort`、`isInputOrderKeeper`、`extractHandleCol`、`Name`。两者都从根越过保序算子、只把 `Sort` 当作已有完整排序、优先使用 handle、否则补齐全部输出列，并且即使改变计划也不设置 `planChanged`。

轻量移植并非生产对象的机械同型：Go 使用 `base.LogicalPlan`、`logicalop.LogicalSort`、`expression.Column` 和 `util.ByItems`，Rust 本文件使用统一的值类型 `PlanNode` 与列下标。Go 还把 `LogicalTableDual` 视为保序节点；本文件的 `PlanKind` 没有对应变体。Go 从 `DataSource.TableInfo.IsCommonHandle` 与 `GetPKIsHandleCol()` 读取真实元数据，本文件用 `from_data_source`/`common_handle`/`handle:<index>` 标签模拟。Go 的列等价性调用 `EqualColumn`，本文件仅比较 `column` 下标。

生产 Rust 的 `optimizer_runtime.rs` 更贴近 Go：它操作真实 logicalop 类型，把 `LogicalTableDual` 纳入保序集合，保留 schema、输出名、上下文与 query block，并通过真实 `DataSource` 提取主键 handle。`optimizer_logical_entry_aster_unit_test.rs::stabilize_results_injects_deterministic_handle_order` 验证的是这条生产 Rust 路径；`rule_result_reorder_test.rs` 验证的是本文件的轻量镜像。仓库中没有找到直接点名 Go `ResultReorder` 的独立 Go 测试。

## 扩展指南

- 新增保序算子时，应同时评估 `isInputOrderKeeper`、`completeSort` 的叶节点语义以及 `injectSort` 的孩子替换逻辑；若要求 Go/生产一致，还应同步 `rule_result_reorder.go` 与 `optimizer_runtime.rs::is_input_order_keeper`。
- 改变 handle 识别时，优先修改 `extractHandleCol`，并为普通数据源、common handle、非法/越界标签、穿过 Selection/Limit 后 schema 丢列分别补充 `rule_result_reorder_test.rs` 中的独立测试。不要把测试嵌入本源文件。
- 改变排序键等价性时需注意当前只按列下标去重；表达式排序、方向、NULL 顺序等语义不能直接套用这一判断。对应风险是重复键、漏键或与 Go `EqualColumn` 不一致。
- 若要把本文件接入生产优化器，不能只实例化 `ResultReorder`：必须先解决轻量 `PlanNode` 与 `LogicalPlanRef` 的模型差异、会话启用条件、语句类型排除、错误接口、上下文/schema/输出名保留及规则位接线。当前生产实现已经位于 `optimizer_runtime.rs`，应避免形成两套漂移实现。
- 性能变更应关注宽 schema 上的二次扫描和无 handle 时的全列排序成本；兼容性变更应保持 `Name() == "result_reorder"`、`changed == false` 以及 `TopN` 不被视为完整排序等 Go 契约。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`query ResultReorder`、`query result_reorder` 定位 Rust/Go 定义和全部辅助方法；`node --file pkg/planner/core/rule_result_reorder.rs` 读取 113 行源码并报告唯一使用文件为 `rule_result_reorder_test.rs`；Go 与测试文件也通过 `node --file` 核对。
- 源码与装配：`pkg/planner/core/rule_result_reorder.rs`、`pkg/planner/core/task.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`。目标包根目录没有 `doc.go`，因此没有可额外读取的包契约文件。
- Go 对照与启用链：`pkg/planner/core/rule_result_reorder.go`、`pkg/planner/core/optimizer.go`、`pkg/planner/core/rule/logical_rules.go`、`pkg/sessionctx/variable/sysvar.go`。
- Rust 生产对照：`pkg/planner/core/rule/logical_rules.rs`、`pkg/planner/core/optimizer_runtime.rs`。
- 测试证据：`pkg/planner/core/rule_result_reorder_test.rs` 的五个测试覆盖注入位置、补全已有 Sort、叶保序节点、TopN 边界与显式 handle；`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs::stabilize_results_injects_deterministic_handle_order` 覆盖生产规则位调度后的 handle 排序。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构由任务指定命令检查恰好 11 个固定二级标题，并另行核对所有引用路径存在、限定 diff 只含本说明和任务文件删除、`plan.md` 未修改。

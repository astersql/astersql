# `pkg/planner/memo/expr_iterator.rs`

## 文件定位

本文件属于 `astersql-planner-memo` crate。crate 根 `pkg/planner/memo/lib.rs` 将其声明为私有模块 `expr_iterator`，再通过 `pub use expr_iterator::*` 向外暴露迭代器 API；`pkg/planner/memo/Cargo.toml` 的 `[package.metadata.porting]` 又明确把该 crate 对应到 Go 包 `pkg/planner/memo`。它位于旧 Cascades 优化器的 Memo 匹配路径中：`pkg/planner/cascades/old/optimize.rs::explore_group_expression` 为规则的 `Pattern` 和一个固定根 `GroupExpr` 调用 `NewExprIterFromGroupElem`，再以 `Matched`、`GetExpr`、`Next` 驱动规则匹配和变换。

该文件只负责“把一个 Pattern 绑定到 Memo 中所有可匹配的表达式组合并依次枚举”，不负责构造 Pattern、插入/删除 Group 表达式、判断具体规则是否成立，也不执行规则变换。Pattern 的算子和引擎判断来自 `astersql-planner-cascades-pattern`，Memo 容器及引用类型来自本 crate 的 `GroupRef`、`GroupExprRef`。

## 核心职责

- `ExprIter` 保存一棵与 `Pattern` 树同形的迭代器树；每个非根节点绑定一个 `Group` 及其中当前匹配表达式的下标，根节点固定到调用方给出的表达式。
- `NewExprIterFromGroupElem` 验证根表达式的算子和所属 Group 引擎是否满足 Pattern，并递归建立首个可用的子绑定；任何必要子模式无法匹配时返回 `None`。
- `Next` 以“最右子节点优先”的顺序推进组合。某个子节点推进成功后，它右侧的兄弟全部 `Reset` 到首个匹配，因此多个子 Group 的结果形成稳定的笛卡尔积枚举顺序。
- `Reset` 在当前 Group 中重新定位第一个满足算子、引擎和递归子模式的表达式；`OperandAny` 则把整个 Group 当作一次匹配，不绑定具体 `Element`。
- `Matched` 和 `GetExpr` 向优化器暴露当前状态与根表达式。`pkg/planner/cascades/old/optimize.rs` 依赖这种状态机约定来循环调用规则。

## 主要符号

- `pub struct ExprIter`：公开状态载体。`Group: Option<GroupRef>` 和 `Element: Option<usize>` 定位普通节点；`Pattern: Pattern` 是当前节点模式的克隆；`Children: Vec<ExprIter>` 保存子模式迭代器；私有 `matched` 是最近一次构造、`Next` 或 `Reset` 的结果；私有 `root_expression` 专门保存不绑定 Group 的固定根表达式。
- `ExprIter::Next(&mut self) -> bool`：推进到下一个匹配组合，同时更新 `matched`。它先倒序推进 `Children`，再尝试当前 Group 的后续同算子表达式。
- `ExprIter::Matched(&self) -> bool`：只读取状态，不重新求值。
- `ExprIter::Reset(&mut self) -> bool`：从 `Group::GetFirstElem(Pattern.Operand)` 开始恢复首个有效绑定；匹配结果同步写入 `matched`。
- `ExprIter::GetExpr(&self) -> Option<GroupExprRef>`：根迭代器从 `root_expression` 返回固定表达式；普通节点按 `Group + Element` 读取 `Equivalents`。缺少定位或下标失效时返回 `None`。
- `reset_children(iterators, groups) -> bool`：内部辅助函数，逐项把子迭代器绑定到表达式的子 Group 并调用 `Reset`；首个失败立即返回。
- `pub fn NewExprIterFromGroupElem(group, element, expression_pattern) -> Option<ExprIter>`：唯一公开构造入口。它校验下标及根模式，递归构造子树，并只在根上保存 `root_expression`/`Element`，有意让根 `Group` 保持 `None`。
- `newExprIterFromGroupExpr`：根据一个固定 `GroupExpr` 构造与 Pattern 子树对应的迭代器树；Pattern 明确给出子模式时，子模式数必须等于表达式子 Group 数。
- `newExprIterFromGroup`：在一个 Group 中从指定 Operand 的首项开始寻找首个能递归构造成功的表达式；`OperandAny` 是无 `Element`、无子迭代器的特殊叶节点。

本文件没有模块级常量、trait、异步函数或条件编译项。

## 执行流程

1. `explore_group_expression` 从规则取得 Pattern，并在当前 Group 的 `Equivalents` 中找到固定根表达式的下标，然后调用 `NewExprIterFromGroupElem`。
2. 构造入口取得根表达式及 Group 的 `EngineType`，用 `Pattern::Match` 同时校验 Operand 和引擎集合。不匹配、下标越界或递归子绑定失败都会得到 `None`。
3. `newExprIterFromGroupExpr` 为 Pattern 的每个子节点调用 `newExprIterFromGroup`。后者用 `Group::GetFirstElem` 直接跳到目标 Operand 的聚簇起点，再递归选择首个子树也可匹配的表达式。
4. 构造成功后，根迭代器固定保存传入表达式，子迭代器则各自保存 Group/Element。调用方在 `while binding.Matched()` 中读取根表达式、执行规则匹配与变换。
5. `Next` 从最右子迭代器向左尝试推进。推进某一位成功后，所有更右侧兄弟回到首个匹配，效果类似多位计数器；若所有子节点都耗尽，普通子迭代器继续扫描自身 Group 的下一个同 Operand 表达式，并重新绑定它的全部子节点。
6. 根迭代器的 `Group` 为 `None`，所以子组合耗尽后 `Next` 直接结束，不会越过调用方指定的根表达式。优化器随后处理下一条规则或下一个根表达式。

`pkg/planner/memo/expr_iterator_test.rs` 用两侧各三个匹配表达式验证组合数为 9，用 `Join(Projection, Selection(Limit))` 验证嵌套组合数为 18；引擎测试还验证 TiKV、TiFlash 和二者并集的过滤结果。

## 数据与状态

`GroupRef`/`GroupExprRef` 是共享的 Memo 引用；迭代器克隆引用而不复制 Group 或表达式。`Element` 是 `Equivalents: Vec<_>` 的下标，因此其有效性依赖枚举期间 Vec 的结构不变。`pkg/planner/cascades/old/optimize.rs` 明确把规则产生的新表达式暂存在 `pending`，待当前固定根的全部绑定枚举结束后再插入，避免插入导致下标移动。

`matched` 是显式状态而不是由字段即时推导。成功构造时为 `true`；每次 `Next`/`Reset` 都在所有返回路径上更新它。调用方应以 `Matched()` 控制循环，并在读取后调用 `Next()`，不能只凭 `Element.is_some()` 判断：`OperandAny` 的有效绑定本来就没有 Element。

`Group::GetFirstElem` 使用 `FirstExpr` 索引找到某 Operand 的首个表达式；`Group::Insert` 把同 Operand 表达式插在该聚簇中并重建索引。因此 `Next` 遇到第一个 `Operand::Match` 失败项即可停止，后续项不会重新匹配该 Operand。Pattern 的引擎约束由 `Pattern::Match`/`MatchOperandAny` 对 Group 的 `EngineType` 检查。

## 依赖与调用关系

上游生产调用者是 `pkg/planner/cascades/old/optimize.rs::explore_group_expression`。其调用链为：规则 `get_pattern` → `memo::NewExprIterFromGroupElem` → `Matched/GetExpr` → `rule.matches`、`rule.on_transform` → `Next`。`GetExpr` 还被 `pkg/planner/cascades/old/transformation_rules.rs` 中的 `ExprIterExt` 和各规则适配逻辑读取，用于访问当前绑定的计划节点与子 Group。

本文件的直接下游包括：

- `crate::{GroupExprRef, GroupRef}` 以及 `Group::GetFirstElem`、`Group::Equivalents`、`Group::EngineType`；
- `astersql_planner_cascades_pattern::{Pattern, GetOperand, Operand::Match}`，负责算子类型和引擎集合语义；
- `GroupExpr::ExprNode` 与 `GroupExpr::Children`，分别提供当前 Operand 和递归子 Group；
- `Rc<RefCell<_>>` 风格引用的 `borrow`/`borrow_mut` 行为（由 Memo 引用别名封装）。

`pkg/planner/memo/Cargo.toml` 声明 `astersql-planner-cascades-pattern` 为直接依赖，并通过同 crate 的 Group/GroupExpr 模块连接 planner core、logical operator、property 等 Memo 数据。测试所需的 domain、parser、physicalop、coretestsdk 等列在 `dev-dependencies`，不属于此迭代器运行时的直接职责。

## 错误处理与边界

该 API 不产生 `Result`，所有“不存在或不匹配”都编码为 `Option` 或 `bool`：构造入口对非法下标、根模式不匹配、子数量不一致、找不到首个子匹配返回 `None`；`Next`/`Reset` 在耗尽或失配时返回 `false` 并清除 `matched`；`GetExpr` 在缺失 Group/Element 或下标无效时返回 `None`。

关键边界如下：

- 根迭代器故意没有 `Group`，但通过 `root_expression` 仍可安全实现 `GetExpr`；它不会在同 Group 中扫描其他根表达式。
- Pattern 的 `Children` 为空时，当前表达式即使自身有子 Group 也可作为叶模式匹配；Pattern 非空时则要求两边子数量完全一致。
- `OperandAny` 只要引擎集合包含 Group 引擎就代表整组一次匹配，既不选择具体表达式也不继续枚举。
- `reset_children` 在某个子节点失败后不会回滚此前已重置的兄弟；调用者只依据整体 `false` 丢弃该候选，随后尝试下一个表达式，因此这些中间状态不会被当作有效结果观察。
- `Next` 重置右侧兄弟时不检查 `Reset` 返回值，依赖这些兄弟在当前父表达式绑定下此前已经成功匹配、且 Memo 在枚举期间不被结构性修改的不变量。
- 公开字段允许外部破坏 `Group`、`Element`、`Pattern`、`Children` 的对应关系；生产调用路径只使用构造器生成的实例。扩展代码不应手工拼装或在枚举中改写这些字段。

## 并发与资源生命周期

本文件没有线程、任务、通道、锁、I/O 或显式事务。迭代器是同步、可变的状态机；`Next` 和 `Reset` 要求独占 `&mut self`。Memo 引用使用单线程共享所有权及内部可变性，短暂 `borrow()` 后会克隆需要继续使用的表达式/子 Group 引用，避免跨递归调用长期持有 RefCell 借用。

资源生命周期由引用计数管理：`ExprIter` 持有 Pattern 值及 Group/GroupExpr 强引用，迭代器释放时这些克隆引用随之释放；它不拥有或清理 Memo 本身。枚举期间调用方必须避免改变 `Equivalents` 的位置布局；优化器通过延迟插入满足该约束。若将来引入并行规则探索，需要先替换或隔离当前 `Rc<RefCell<_>>` 和可变下标模型，不能假设 `ExprIter` 可跨线程共享。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/memo/expr_iterator.go`，Rust 基本保留其控制流：子节点从右向左推进、右侧兄弟复位；同 Operand 表达式连续存放，因此遇到首个 Operand 不匹配即停止；无子模式允许非叶表达式直接匹配；递归子绑定全部成功才接受父表达式；引擎过滤由 Pattern 完成。

数据表示存在必要差异：Go 用嵌入的 `*Group`、`*list.Element`、`*pattern.Pattern` 和 `nil`，Rust 用 `Option<GroupRef>`、`Option<usize>`、拥有的 `Pattern` 克隆与 `Option<GroupExprRef>`。Go 根节点的 `Element` 自身可返回表达式；Rust 的 Element 只是必须配合 Group 的 Vec 下标，所以额外增加私有 `root_expression` 来保持“根 Group 为 nil/None，但 `GetExpr` 仍返回固定根式”的语义。

Go 的 `NewExprIterFromGroupElem` 接收链表元素，Rust 接收 `group + element index`；这使 Rust 调用方必须保证索引稳定。Rust 优化器的延迟插入正是对此差异的局部接线。Go `Reset` 使用 `expr.Group.EngineType`，Rust 使用当前迭代器 Group 的 `EngineType`；正常 Memo 不变量下表达式所属 Group就是当前 Group，语义一致。

测试对照为 `pkg/planner/memo/expr_iterator_test.go` 与独立 Rust 测试 `pkg/planner/memo/expr_iterator_test.rs`。二者覆盖首个子绑定、9 个平面组合、18 个嵌套组合、引擎过滤计数以及无 `OperandAny` 叶模式；Rust 测试没有内嵌在生产源文件中。

## 扩展指南

- 新增枚举顺序或回溯策略时，首先修改 `ExprIter::Next` 与必要的 `Reset` 协议，并同步 `pkg/planner/memo/expr_iterator_test.rs` 中对顺序、组合数和嵌套重置的断言；不得只验证“至少找到一个”而丢失 Go 的全部组合语义。
- 新增 Pattern 节点语义或引擎类型时，应优先在 `pkg/planner/cascades/pattern/pattern.rs` 的 `Match`/`MatchOperandAny` 定义语义，再确认 `NewExprIterFromGroupElem`、`newExprIterFromGroup` 和 `Reset` 三处使用一致，并同步 Go 对照测试意图。
- 若修改 Group 存储结构或插入排序，必须同时复核 `Group::GetFirstElem` 和“同 Operand 连续”不变量；否则 `Next`/`Reset` 的提前终止会漏掉后续匹配。若 Vec 下标不再稳定，应把 `Element` 改为稳定句柄，而不是依赖调用方延迟写入。
- 若需要支持在枚举中即时插入/删除，应重新设计快照或游标生命周期，并修改 `pkg/planner/cascades/old/optimize.rs` 的 `pending` 接线；当前实现对此不安全。
- 新增失败原因诊断时，可考虑在新 API 层提供带原因的结果，但需保留现有 `Option`/`bool` 外观或同步所有优化器调用方，避免把正常的“模式不匹配/枚举耗尽”误作异常。
- 性能上应保留 `FirstExpr` 起点和 Operand 聚簇的短路能力；在热路径中深拷贝表达式树或反复从 Group 起点全扫描都会放大规则数乘以组合数的成本。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标文件被索引为 `pkg/planner/memo/expr_iterator.rs`。
- RustCodeGraph `node --file pkg/planner/memo/expr_iterator.rs` 核对了完整源码，以及 `Next`、`Reset`、`GetExpr`、`Matched`、`reset_children`、`newExprIterFromGroupExpr`、`newExprIterFromGroup`、`NewExprIterFromGroupElem` 的定义和调用轨迹。图中确认 `NewExprIterFromGroupElem` 被四个 Rust 测试辅助路径调用，`Next` 调用 `Reset/reset_children`，`Reset` 调用 `reset_children`。
- RustCodeGraph 对 `pkg/planner/memo/group.rs::GetFirstElem`、`Group::Insert` 的查询确认了 Operand 起点索引、聚簇插入与索引重建；对 `pkg/planner/cascades/pattern/pattern.rs::Match`、`MatchOperandAny` 的查询确认了 Operand 与引擎集合语义。
- 已阅读源码/模块/配置：`pkg/planner/memo/expr_iterator.rs`、`pkg/planner/memo/lib.rs`、`pkg/planner/memo/Cargo.toml`；生产调用：`pkg/planner/cascades/old/optimize.rs`、`pkg/planner/cascades/old/transformation_rules.rs`；Go 对照：`pkg/planner/memo/expr_iterator.go`；独立测试：`pkg/planner/memo/expr_iterator_test.rs`、`pkg/planner/memo/expr_iterator_test.go`。
- 人工事实复核：文档分别说明了文件存在原因、固定根与递归子绑定的运行方式、组合推进顺序、Operand/引擎/子数边界、Vec 下标生命周期，以及修改 Pattern、Group 存储和枚举逻辑时的同步点。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务文件规定的 11 章节命令、范围检查和 diff 自审作为验证。

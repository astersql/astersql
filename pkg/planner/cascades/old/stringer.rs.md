# `pkg/planner/cascades/old/stringer.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-old` crate，是旧版 Cascades 优化器的 memo `Group` 图字符串化工具。模块在 `pkg/planner/cascades/old/lib.rs` 中以私有 `mod stringer` 装配，再通过 `pub use stringer::*` 将唯一公开入口 `ToString` 重导出。它不参与计划选择或执行，只把探索阶段形成的逻辑等价类图转换为稳定的逐行文本，供调试与 golden 结果比较使用。

当前 Rust 仓库内，`ToString` 的直接调用只出现在独立测试 `pkg/planner/cascades/old/stringer_test.rs`；未发现生产 Rust 调用点。因此应把它理解为已接线的诊断/测试 API，而不是 SQL 请求主链的必经步骤。其输入通常由 `astersql_planner_memo::Convert2Group` 建立，并可在 `Optimizer::onPhaseExploration` 改写后用于观察 memo 结构。

## 核心职责

- `ToString` 为根 Group 建立显示编号 `Group#0`，并初始化访问集合和结果行。
- `toString` 以前序方式遍历可能共享子图或形成回边的 memo 图；它先为当前 Group 的全部直接子 Group 分配编号，再输出当前 Group，最后递归子 Group。
- `groupToString` 输出 Group 的 Schema、可选主键/唯一键集合以及所有等价表达式。
- `groupExprToString` 输出逻辑算子的 Explain 标识、子 Group 引用和算子附加说明。
- `getChildrenGroupID` 把有序子输入映射为 `input:[Group#a,Group#b]`。

输出是 `Vec<String>` 而不是一段带换行符的文本，调用方可直接逐行断言或写入 testdata。文件不会计算 Schema、键信息或执行探索；它只读取调用方已经构造好的 memo 状态。

## 主要符号

- `pub fn ToString(ctx: &dyn expression::exprctx::EvalContext, group: &GroupRef) -> Vec<String>`：公开入口。以 `group.borrow().ID()` 取得稳定的内部 `u64` ID，但对外显示时重新编号为从 0 开始的紧凑序号。
- `fn toString(..., id_map: &mut HashMap<u64, usize>, visited: &mut HashSet<u64>, lines: &mut Vec<String>)`：递归驱动器。`id_map` 保存内部 Group ID 到显示编号的映射；`visited` 保证共享节点或环只输出一次；`lines` 是全程复用的结果缓冲区。
- `fn groupToString(ctx, group, id_map) -> Vec<String>`：渲染一个 Group。首行为 `Group#n Schema:[...]`；当 `Schema::PKOrUK` 非空时追加 `, UniqueKey:[...]`；后续每个 `GroupExpr` 各占一行并缩进四个空格。
- `fn groupExprToString(expression: &GroupExpr, id_map: &HashMap<u64, usize>) -> String`：渲染算子表达式。通过 `ExprNode::TP`、`ID`、`SCtx` 和 `ExplainInfo` 形成与 Go `ExplainID().String()` 一致的文本。
- `fn getChildrenGroupID(expression, id_map) -> String`：保持 `GroupExpr::Children` 的原顺序生成子输入列表。

文件没有模块级常量、类型、trait、条件编译项或可变静态状态；除 `ToString` 外，其余函数均为模块私有实现。

## 执行流程

1. `ToString` 把根 Group 的内部 ID 映射到显示编号 0，创建空的 `visited` 与 `lines`，调用 `toString`。
2. `toString` 先把当前内部 ID 插入 `visited`；若已存在则立即返回，因此同一 Group 即使由多条边引用也只输出一次。
3. 函数克隆当前 `Equivalents`，按等价表达式顺序及其 `Children` 顺序扫描所有直接子 Group。尚未出现的子 ID 按 `id_map.len()` 分配显示编号。这个“先登记全部直接子节点、再打印父节点”的顺序使父表达式中引用的编号总是已经存在，也与 Go 实现一致。
4. `groupToString` 借用当前 Group：逐列调用 `Column::StringWithCtx(ctx, RedactLogDisable)` 形成 Schema 文本；再以同样方式渲染每组 `PKOrUK`；最后按 `Equivalents` 顺序调用 `groupExprToString`。
5. `groupExprToString` 检查逻辑节点的 `SCtx()`。当上下文存在且 `ignore_explain_id_suffix()` 为真时只写 `TP()`，否则写 `TP_ID`。叶表达式直接追加空格和 `ExplainInfo()`；非叶表达式先追加 `input:[...]`，仅在 ExplainInfo 非空时再追加 `, ...`。
6. 父 Group 输出完成后，`toString` 再按相同的等价表达式/子节点顺序递归。最终形成“父在前、子在后”的前序行序列。

编号顺序由 `Vec<GroupExprRef>` 和 `Vec<GroupRef>` 的遍历顺序决定，而不是由 `HashMap` 的迭代顺序决定，因此散列表随机化不会改变输出。

## 数据与状态

- `GroupRef` 是 `Rc<RefCell<Group>>`（见 `pkg/planner/memo/group.rs`），其内部 `Group::ID()` 是全局分配的稳定 `u64`。输出刻意不暴露该值，而使用本次调用局部的连续编号。
- `Group::Equivalents` 是有序的 `Vec<GroupExprRef>`；`GroupExpr::Children` 是有序的 `Vec<GroupRef>`（见 `pkg/planner/memo/group_expr.rs`）。二者的顺序共同定义输出和首次发现编号。
- `id_map` 的生命周期只覆盖一次 `ToString` 调用。它既是编号表，也是 `groupExprToString`/`getChildrenGroupID` 的完整性前提。
- `visited` 只控制是否再次展开 Group，不阻止多个父表达式在各自行中引用同一显示编号。
- `lines` 是最终返回值；递归函数通过可变引用原地追加，避免 Go 版本那样在递归间传回切片。
- Schema 与 `PKOrUK` 属于 memo 的逻辑属性。`NewGroupWithSchema` 初建 Group 时只复制列；若调用方需要完整 UniqueKey 输出，应先通过 `BuildKeyInfo` 或其他可信路径填充键信息，本文件不会补推导。

本文件只读取 memo。为了缩短 `RefCell` 借用周期，`toString` 在递归前克隆 `Equivalents` 和各表达式的 `Children`；这些克隆复制的是 `Rc`，不是深拷贝整个计划图。

## 依赖与调用关系

直接依赖只有标准库 `HashMap`/`HashSet`、`astersql-expression` 以及 `astersql-planner-memo::{GroupExpr, GroupRef}`。`pkg/planner/cascades/old/Cargo.toml` 将前两项分别声明为本地 workspace crate `../../../expression` 与 `../../memo`；`[lib] path = "lib.rs"` 确认本文件属于旧 Cascades crate，而非独立 crate，也没有控制本文件的 feature 开关。

上游结构链为：逻辑计划经 `memo::Convert2Group` 变成 Group 图，旧 `Optimizer` 可在 `onPhasePreprocessing`/`onPhaseExploration` 中改写它，调用方随后将根 `GroupRef` 和表达式求值上下文传给 `ToString`。RustCodeGraph 将 `stringer.rs` 与 `stringer_test.rs` 建立使用关系；仓库文本检索确认三处 Rust 直接调用均位于该测试文件，没有生产调用。

下游调用包括 `Group::ID`、读取 `Group::Prop.Schema`/`Equivalents`、读取 `GroupExpr::ExprNode`/`Children`、`Column::StringWithCtx`，以及逻辑计划 trait 的 `TP`、`ID`、`SCtx`、`ExplainInfo`。`lib.rs` 的重导出让测试可使用 `crate::ToString`。

## 错误处理与边界

函数不返回 `Result`，也不吞掉格式化错误；它依赖 memo 已满足结构不变量，违反时会 panic：

- `groupToString` 对 `Prop.Schema` 使用 `expect("a memo group must have a logical schema")`，无 Schema 的 Group 不能被字符串化。
- `id_map[&group.ID()]` 与子 Group 映射使用索引语法。根 ID 在入口登记，直接子 ID 在渲染前登记；如果未来绕过这一流程单独调用私有 helper 或改变遍历顺序，缺失映射会 panic。
- `RefCell::borrow()` 假设调用期间没有冲突的可变借用；重入或在持有 `borrow_mut` 时调用可能触发运行时借用 panic。

空等价表达式仍会输出一行 Group Schema；空 Schema 输出 `Schema:[]`。空 UniqueKey 集合不会显示该字段。叶节点无论 ExplainInfo 是否为空都会在 Explain 标识后追加一个空格；非叶节点只在 ExplainInfo 非空时添加逗号和说明，这是与 Go 格式保持一致的细节。多个唯一键先把每个键内的列以逗号连接，再把各键也以逗号连接，输出本身不显式保留键组边界。

`visited` 使环或共享 DAG 不会无限递归，但深度仍由 Group 图路径长度决定；极深图会占用递归栈。文件关闭日志脱敏（`RedactLogDisable`），所以输出可能含列名等原始 Explain 信息；不应未经评估直接写入面向不可信读者的日志。

## 并发与资源生命周期

实现是同步的，不创建线程、异步任务、通道、锁、事务或外部资源。所有编号表、访问集合和结果缓冲区均在一次 `ToString` 调用内创建，返回后除结果 `Vec<String>` 外全部释放。

`GroupRef` 使用 `Rc<RefCell<_>>`，天然面向单线程共享可变图；本函数本身不提供跨线程安全保证。它在遍历时取得短期不可变借用并克隆 `Rc` 引用，随后释放借用再递归，避免让父级 `RefCell` 借用跨越递归边界。若同线程的其他逻辑通过内部可变性在字符串化过程中修改图，输出一致性仍没有快照保证；正常调用应在 memo 改写阶段结束后进行。

时间复杂度约为对可达 Group、等价表达式、子边、Schema 列和唯一键列各扫描一次；额外空间主要是 `id_map`、`visited`、结果文本及递归栈。共享子图虽只展开一次，但每条表达式边仍会在父表达式文本中出现。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/old/stringer.go`。Rust 保留了 Go 的五层函数划分、前序遍历、先登记直接子 Group、四空格缩进、Schema/UniqueKey 格式、叶与非叶 ExplainInfo 分支以及子输入列表格式。

实现层面的等价改写包括：Go 以 `map[*memo.Group]int` 按指针标识 Group，Rust 以稳定 `Group::ID() -> u64` 为键；Go 的 `Equivalents` 是链表，Rust 是保持兼容顺序的 `Vec`；Go 在递归间返回累积切片，Rust使用共享的 `&mut Vec<String>`；Go 调用 `ExprNode.ExplainID().String()`，Rust显式依据 `SCtx().ignore_explain_id_suffix()` 在 `TP` 与 `TP_ID` 之间选择。

Go 独立测试 `pkg/planner/cascades/old/stringer_test.go::TestGroupStringer` 从 SQL 解析、构建逻辑计划、预处理、探索、`BuildKeyInfo`，再与 `testdata/stringer_suite_out.json` 比较，覆盖了真实 SQL 与 UniqueKey 输出。Rust 独立测试因当前窄运行时没有复刻整条 SQL 构建链，改用手工逻辑计划覆盖三个关键事实：未探索 Group 树的编号/Schema/子引用、探索规则把零行 Limit 改写成空 TableDual 后的可观察结果，以及忽略 Explain ID 后缀的会话选项。Rust 测试保留了 Go 的核心观察意图，但并未逐项复刻 Go testdata 的全部 SQL 与 UniqueKey 场景。

## 扩展指南

- 若新增 Group 级字段，优先修改 `groupToString`，并在 `pkg/planner/cascades/old/stringer_test.rs` 增加确定性断言；涉及 Schema/键信息时还应核对 `pkg/planner/memo/group.rs::BuildKeyInfo` 和 Go golden 输出。
- 若改变算子标识或 Explain 文本，修改 `groupExprToString`，同步检查逻辑计划 trait 的 `TP`/`ID`/`SCtx`/`ExplainInfo` 契约，尤其不能绕过 `ignore_explain_id_suffix`。
- 若改变遍历或编号策略，应同时审查 `toString` 的“先登记全部直接子节点”不变量、共享子图/环的 `visited` 行为，以及 Go `toString` 的顺序；不要依赖 `HashMap` 迭代顺序。
- 若改变子输入格式，集中修改 `getChildrenGroupID`，并覆盖零、一、多个子 Group 以及共享子 Group。
- 测试逻辑必须继续放在独立的 `stringer_test.rs`，由 `optimize.rs` 的 `#[path = "stringer_test.rs"]` 挂载；不要把测试内嵌回生产源文件。
- 格式是 golden 接口，空格、逗号、缩进、编号和 Explain 后缀都可能被测试依赖。任何“美化”都属于兼容性变更，应同步 Go 对照、Rust 测试及相关 testdata，并评估大 memo 上新增字符串分配的性能影响。

## 验证依据

- RustCodeGraph `status`：索引可用，覆盖本仓库 Rust/Go 文件；`files --filter pkg/planner/cascades/old/stringer.rs` 确认目标文件已索引且包含 8 个索引符号，`node --file ...` 读取了目标源码全貌。
- RustCodeGraph `query` 确认目标文件中的 `ToString`、`toString`、`groupToString`、`groupExprToString`、`getChildrenGroupID`；`node` 读取了 `stringer.rs`、`stringer_test.rs`、Go 对照与 Go 测试。由于全仓存在大量同名 `ToString`，`callers --file` 仍返回跨文件同名噪声，调用点结论另以限定 `*.rs`/`*.go` 的仓库检索复核。
- 已读源码/配置：`pkg/planner/cascades/old/stringer.rs`、`lib.rs`、`Cargo.toml`，`pkg/planner/memo/group.rs`、`group_expr.rs`，以及逻辑计划契约 `pkg/planner/core/operator/logicalop/base_logical_plan.rs`。目标目录不存在 `doc.go`。
- 已读测试与 Go 证据：`pkg/planner/cascades/old/stringer_test.rs`、`stringer.go`、`stringer_test.go`、`testdata/stringer_suite_out.json`；另外通过限定检索确认 Go transformation rule 测试也把 `ToString` 用作 golden 观察器。
- 任务为纯文档分析，按计划不运行 Cargo。交付前以任务给定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核本文未把未接线的生产调用或未覆盖的 Rust 测试场景写成已支持事实。

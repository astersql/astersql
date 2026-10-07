# `pkg/parser/ast/node_flags.rs`

## 文件定位

本文件位于 `astersql-parser-ast` crate 内，是“真实解析器 AST”的表达式特征标记计算层。它接收任意 `&dyn Node`，沿 `Node::accept`/`Visitor` 遍历完整语句树，把参数、引用、函数、聚合、窗口函数、子查询等性质写入每个 `ExprNode::Flag`。模块在 `pkg/parser/ast/lib.rs` 中以私有 `mod node_flags` 装配，但公开再导出 `HasAggFlag`、`HasWindowFlag` 和 `SetFlag`。

实际入口位于 `pkg/parser/yy_parser.rs` 的 `Parser::ParseSQL`：语法归约完成且 `check_ast_depth_limit` 成功后，对每条结果语句调用 `parser_ast::SetFlag(statement.as_ref())`，然后才向调用方返回 AST。因此，本文件处在“SQL 已解析为 AST”与“绑定、规划等下游读取表达式性质”之间。

不要把本文件与 `pkg/parser/ast/flag.rs` 中针对精简模型 `FlagExpr` 的同名 API 混为一谈。前者操作 `lib.rs` 定义的真实 `ExprNode`，也是 `lib.rs` 对外再导出的实现；后者是公开 `flag` 模块中的独立移植模型。

## 核心职责

- `SetFlag` 为一棵语句或表达式 AST 中的真实 `ExprNode` 自底向上计算标志位。
- `expression_flag` 按 `ExprKind` 决定节点自身标志，并将相关子表达式的 `GetFlag()` 按位或汇总到父节点。
- `HasAggFlag` 和 `HasWindowFlag` 提供聚合函数、窗口函数标志的常量时间查询。
- 对连续 `Parentheses` 做专门处理：先计算最内层表达式，再把结果逐层复制给括号节点，避免括号包装丢失子表达式性质。
- 对 Go 标志设置器没有赋值规则的节点保留已有标志，而不是无条件清零；这使 `Value` 上预先写入的 `FLAG_PRE_EVALUATED` 等状态得以保留。

它只负责描述 AST 的结构性质，不做名称解析、类型推断、常量折叠或 SQL 合法性检查。

## 主要符号

- `pub fn HasAggFlag(expr: &ExprNode) -> bool`：读取 `ExprNode::GetFlag()`，检查 `FLAG_HAS_AGGREGATE_FUNC` 位是否非零；不触发重新计算。
- `pub fn HasWindowFlag(expr: &ExprNode) -> bool`：同样只读检查 `FLAG_HAS_WINDOW_FUNC`。
- `pub fn SetFlag(node: &dyn Node)`：构造函数内局部类型 `Setter`，令根节点执行 `accept`。参数是共享引用，因为标志字段采用内部可变性。
- 局部 `struct Setter`：实现通用 `Visitor`。`enter` 只为 `ExprKind::Parentheses` 返回 `true` 以接管该子树；其他节点返回 `false`，继续默认子节点遍历。`leave` 在子节点完成后对每个 `ExprNode` 调用 `expression_flag` 并写回。
- `fn expression_flag(expr: &ExprNode) -> u64`：私有、穷举匹配 `ExprKind` 的规则表。内部 `flags` 闭包把表达式切片的标志折叠为按位或。
- 直接依赖的标志常量来自 `super::flag::*`，包括 `FLAG_HAS_PARAM_MARKER`、`FLAG_HAS_FUNC`、`FLAG_HAS_REFERENCE`、`FLAG_HAS_AGGREGATE_FUNC`、`FLAG_HAS_SUBQUERY`、`FLAG_HAS_VARIABLE`、`FLAG_HAS_DEFAULT` 和 `FLAG_HAS_WINDOW_FUNC`。

## 执行流程

1. `Parser::ParseSQL` 生成语句后调用公开再导出的 `SetFlag`。
2. `SetFlag` 从根 `Node` 启动 `Node::accept(&mut Setter)`。各节点的 `accept` 先调用 `enter`，在未跳过时通过 `walk::Children` 依源码顺序访问孩子，最后调用 `leave`。
3. 普通节点在 `enter` 阶段不做修改。后序到达 `leave` 时，只有能向下转型为 `ExprNode` 的节点才计算并写入标志；语句节点本身不存表达式标志。
4. `expression_flag` 对叶子或节点固有性质赋位：参数、列引用、默认值、子查询分别产生对应标志；变量还合并可选值表达式；函数、聚合函数、窗口函数和 cast 在固有位之外合并参数或被转换表达式。
5. 组合表达式按结构传播：二元/比较子查询合并两侧，`Between` 合并三项，`InList` 合并主表达式与列表，`Case` 合并可选 value、全部 when/result 以及可选 else，其他一元包装传播其孩子。
6. 普通 `Function` 仅在 schema 为空且小写函数名为 `values` 时被视为引用；否则标记普通函数并合并参数。
7. 遇到连续括号时，`enter` 找到最内层非括号表达式并单独执行其 `accept`；随后把内层结果写到各层括号节点并返回 `true`。当前括号节点仍会进入 `leave`，最终从直接孩子再次取得同一标志。
8. 遍历结束后，调用方获得已在各真实表达式节点上缓存标志的 AST。

## 数据与状态

状态载体是 `pkg/parser/ast/lib.rs` 的 `ExprNode::Flag: std::cell::Cell<u64>`。`GetFlag`/`SetFlag` 分别调用 `Cell::get`/`Cell::set`，所以 `SetFlag(&dyn Node)` 能在不取得整棵 AST 可变借用的情况下更新缓存。克隆 `ExprNode` 会复制当时的 flag 值，之后对原节点重新计算不会反向修改克隆；`node_flags_test.rs` 对此有直接断言。

各位可以同时存在，父节点通过按位或表示整个相关子树的性质。重要不变量是先完成相关孩子的标记再读取其 `GetFlag()`。`Value`、`IntroducedValue`、`MaxValue`、`MatchAgainst`、`TimeUnit`、`GetFormatSelector`、`TrimDirection`、`TableName`、`JSONSumCrc32` 当前返回节点原有值；它们不会由本函数新增固有位。

子查询的 `ExprKind::Subquery` 自身只置 `FLAG_HAS_SUBQUERY`，但其 `Query: NodeRef` 仍由 AST walk 访问，因此共享查询中的表达式也会被标记。`NodeRef` 使用 `Rc<RefCell<...>>`，表达式标志自身使用 `Cell`，这些都是单线程内部可变状态。

## 依赖与调用关系

上游直接调用边为 `pkg/parser/yy_parser.rs::Parser::ParseSQL -> astersql_parser_ast::SetFlag`；此外 `pkg/parser/ast/node_flags_test.rs` 直接覆盖三个公开函数。仓库 Rust 生产代码搜索未发现 `HasAggFlag` 或 `HasWindowFlag` 的直接调用，因此它们当前是已公开、可供后续绑定/规划代码使用的查询接口，不能据此宣称已进入 Rust 规划主链。

本文件向下依赖：

- `Node::accept` 与 `Visitor::{enter, leave}`（`pkg/parser/ast/lib.rs`），提供先序/后序遍历协议；
- `walk::Children`（`pkg/parser/ast/walk.rs`），定义语句、容器、`NodeRef` 和表达式孩子的覆盖关系；
- `ExprNode`、`ExprKind` 及 `ExprNode::{GetFlag, SetFlag}`（`pkg/parser/ast/lib.rs`），提供真实表达式结构和内部可变缓存；
- `pkg/parser/ast/flag.rs` 的 `FLAG_*` 常量，定义本实现写入和检查的位值。

`pkg/parser/ast/Cargo.toml` 表明该 crate 的库入口是 `lib.rs`，直接依赖 parser 的 auth、charset、mysql、types 子 crate，以及 `serde`、`serde_json`、`url`；本文件自身没有新增外部 crate 依赖或 feature 条件。

## 错误处理与边界

这些 API 不返回 `Result`，也不主动生成诊断。无法向下转型为 `ExprNode` 的节点仅作为遍历容器处理。`Node::accept` 的布尔结果可表示访问中止，但 `SetFlag` 不读取根调用的返回值；当前 `Setter::leave` 总返回 `true`，正常情况下不会自行中止。

边界包括：空参数列表折叠为零；所有可选孩子用 `map_or(0, ...)`；多孩子用按位或，因此重复位是幂等的。`Function` 对 `values` 的识别要求空 schema 且 `FnName.L == "values"`，依赖规范化的小写名称。对未显式赋值的 `ExprKind` 保留旧 flag，调用者若在这些节点上预置了无关或过期位，本文件不会清除它们。

遍历共享 `NodeRef` 时可能触发 `RefCell` 的运行时借用规则；正常的只读 walk 与测试中的 `with_node` 是顺序执行的。极深 AST 的风险由上游 `check_ast_depth_limit` 在调用 `SetFlag` 前控制；若绕过解析器直接对深树调用公开函数，递归访问仍受线程栈限制。

## 并发与资源生命周期

没有锁、线程、异步任务、通道、事务或 I/O。`Setter` 是无字段的栈上临时值，只在一次 `SetFlag` 调用期间存在。算法访问每个通常节点一次，时间复杂度为 O(N)，额外空间主要是访问递归深度 O(H)；连续括号的专门循环与内层访问仍受括号深度影响。

`Cell<u64>`、`Rc` 和 `RefCell` 使真实 AST 默认不是跨线程共享的数据结构。本文件不提供并发同步；同一 AST 的标志计算和读取应留在其单线程所有权/借用生命周期内。共享子查询节点的生命周期由 `NodeRef` 的 `Rc` 引用计数管理，本文件既不取得所有权也不延长根调用之外的借用。

## 与 Go 版本的对应关系

直接对照是 `pkg/parser/ast/flag.go`。两边均以访问者后序遍历实现：Go 的 `flagSetter.Leave` 对具体表达式类型进行 type switch，Rust 的 `Setter::leave` 将真实节点转为 `ExprNode` 后对 `ExprKind` match；聚合、窗口、between、binary、case、列引用、比较子查询、default、exists、普通函数、cast、is-null/is-truth、in、like/regexp、row、subquery、unary、values 和 variable 的合并意图相同。

Rust 真实 AST 将多个 Go 具体结构统一到 `ExprKind`，并额外显式处理 `Collate`、`InSubquery`、连续 `Parentheses` 和共享 `NodeRef` 遍历。Go 的 `Enter` 始终返回 false，而当前 Rust 括号分支会跳过默认孩子遍历后自行下钻。

需特别注意两套常量的位编号：Go `ast.go` 的 `FlagHasParamMarker` 从 `1 << 0` 开始；本文件实际导入的 Rust `flag.rs::FLAG_HAS_PARAM_MARKER` 从 `1 << 1` 开始。另一个 Rust 文件 `ast.rs` 存在与 Go 位号一致的 `FlagHas*` 常量，但不是本文件的依赖。文档只记录当前代码事实，跨语言序列化或与 Go 数值直接比较时不能假定数值一致。

Go 的 `flag_test.go` 通过解析 SQL 覆盖大量表达式组合；真实 Rust 实现的直接测试 `node_flags_test.rs` 当前聚焦函数加参数、聚合/窗口查询、克隆隔离、预求值保留，以及共享子查询和语句孩子遍历。`flag_test.rs` 的广泛案例针对独立 `FlagExpr` 模型，不能替代真实 `ExprNode` 的全部回归证据。

## 扩展指南

新增或修改 `ExprKind` 时，首先在 `expression_flag` 增加明确规则：说明节点固有位、需要传播的孩子以及应保留还是覆盖旧值；同时确认 `walk.rs` 已访问该变体所有携带表达式/语句的字段。若遗漏 match 分支，Rust 的穷举匹配通常会在编译期暴露，但错误地选择“保留原值”仍可能造成静默语义偏差。

修改标志常量时必须同步检查 `flag.rs`、本文件、解析器与消费方，尤其评估上述 Rust/Go 位号差异；不要只改 `ast.rs` 中另一套同名语义常量。新增查询接口可仿照 `HasAggFlag`，但应先确认实际消费方需要的是节点自身缓存还是重新计算后的子树性质。

测试应放在独立的 `pkg/parser/ast/node_flags_test.rs`，不要内嵌到生产源文件。建议至少补充目标变体的真实 `ExprNode` 构造和包含该表达式的语句级遍历；若规则来自 Go，亦应对照或扩展 `pkg/parser/ast/flag_test.go` 的同类 SQL。涉及括号、可选孩子、共享 `NodeRef` 或预置 flag 时，应分别验证传播和保留不变量。兼容风险主要是下游对位值的判断，性能风险主要是重复遍历大树或对子树进行额外递归。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；查询时索引可用。
- RustCodeGraph `explore "pkg/parser/ast/node_flags.rs NodeFlag NodeFlags HasNodeFlag"`：读取目标文件全貌及 `HasAggFlag`、`HasWindowFlag`、`SetFlag`、`expression_flag` 的实现。
- RustCodeGraph `query`：确认同名符号同时存在于 Go `flag.go`、精简 Rust `flag.rs` 和真实 AST `node_flags.rs`，从而限定本文对象。
- RustCodeGraph `node --file`：核对 `lib.rs` 的 `Visitor`、`Node`、`ExprKind`、`ExprNode`、`NodeRef`、`GetFlag`/`SetFlag` 与 `accept` 协议；核对 `walk.rs` 的孩子访问；核对 `yy_parser.rs` 中解析成功后的直接调用边。
- 已读路径：`pkg/parser/ast/node_flags.rs`、`pkg/parser/ast/lib.rs`、`pkg/parser/ast/walk.rs`、`pkg/parser/ast/flag.rs`、`pkg/parser/ast/Cargo.toml`、`pkg/parser/yy_parser.rs`、`pkg/parser/ast/node_flags_test.rs`、`pkg/parser/ast/flag_test.rs`、`pkg/parser/ast/flag.go`、`pkg/parser/ast/flag_test.go`、`pkg/parser/ast/ast.go`、`pkg/parser/ast/ast.rs`。
- 仓库 Rust 搜索确认生产代码直接接线为 `Parser::ParseSQL -> parser_ast::SetFlag`，未发现真实 AST 版 `HasAggFlag`/`HasWindowFlag` 的生产调用。
- 本任务只产出说明文档；按任务约束不运行 Cargo。结构验证命令及人工事实复核结果在任务交付时记录。

# `pkg/parser/ast/flag.rs`

## 文件定位

用 `FlagExpr` 覆盖 Go `flagSetter.Leave` 所处理的主要表达式形态，并在每个复合节点保存或派生 `u64` 标志。

## 核心职责

1. 用 `FLAG_CONSTANT`、`FLAG_HAS_PARAM_MARKER` 到 `FLAG_HAS_WINDOW_FUNC` 表达一个表达式子树是否含参数、普通函数、引用、聚合、子查询、变量、`DEFAULT`、预求值结果或窗口函数。
2. 用 `FlagExpr` 覆盖 Go `flagSetter.Leave` 所处理的主要表达式形态，并在每个复合节点保存或派生 `u64` 标志。
3. `set_flag` 先递归处理子节点，再把子节点标志按位或合并到父节点，并给参数、函数、聚合、窗口、引用、变量等节点叠加固有标志。
4. `has_agg_flag`、`has_window_flag` 提供按位检测；`HasAggFlag`、`HasWindowFlag`、`SetFlag` 是面向 Go 命名习惯的别名。

标志只描述语法树中是否出现某类成分，不执行求值、类型推断、名称绑定或优化。源码注释提到这些信息供绑定和优化判断使用，但当前真实解析结果的生产接线位于 `node_flags.rs`。

## 主要符号

- `FLAG_CONSTANT: u64 = 0`：常量表达式没有任何置位；所以它是“没有其他性质”的零值，而不是一个独立 bit。
- `FLAG_HAS_PARAM_MARKER`、`FLAG_HAS_FUNC`、`FLAG_HAS_REFERENCE`、`FLAG_HAS_AGGREGATE_FUNC`、`FLAG_HAS_SUBQUERY`、`FLAG_HAS_VARIABLE`、`FLAG_HAS_DEFAULT`、`FLAG_PRE_EVALUATED`、`FLAG_HAS_WINDOW_FUNC`：分别使用第 1 至第 9 位（`1 << 1` 到 `1 << 9`）；第 0 位未在本文件定义。
- `FlagExpr`：精简表达式树枚举。带命名 `flag` 字段的多数变体缓存传播结果；`Binary(Box<FlagExpr>, Box<FlagExpr>)` 没有自己的字段，而是在读取时合并左右子树。`Leaf { sql, flag }` 保存测试或调用者提供的原始文本和预置标志，`sql` 不参与传播。
- `FlagExpr::get_flag(&self) -> u64`：对绝大多数变体返回字段值；对 `Binary` 动态返回左右子树标志的按位或。
- `set_flag(&mut FlagExpr)`：传播器核心。它递归改写树中有缓存字段的节点，最后为引用、`DEFAULT` 和子查询叶子设置固有位。
- `has_agg_flag` / `has_window_flag`：分别检测 `FLAG_HAS_AGGREGATE_FUNC` 和 `FLAG_HAS_WINDOW_FUNC` 是否非零。
- `HasAggFlag` / `HasWindowFlag` / `SetFlag`：仅转调对应 snake_case API，没有额外行为。

本文件没有 trait、struct、条件编译项或错误类型。所有上述常量、枚举和函数均为公开 API；枚举各字段也公开可构造。

## 执行流程

典型调用流程是先构造一棵 `FlagExpr`，再把根节点可变引用传给 `set_flag`/`SetFlag`，最后用 `get_flag` 或两个查询函数读取结果。

`set_flag` 的第一阶段按节点类型执行后序遍历：

1. `ParamMarker` 直接写入参数位；`Aggregate`、`Window` 和 `FuncCall` 递归所有参数，再将参数标志与自身固有位合并；`FuncCast` 在子标志上增加普通函数位。
2. `Between`、`CompareSubquery`、`Case`、`PatternIn`、`PatternLike`、`PatternRegexp`、`Row` 等容器先递归所有存在的子项，再按位或汇总。可选子节点缺失时贡献零。
3. `Binary` 只递归左右节点，因为其 `get_flag` 始终动态合并两侧；`IsNull`、`IsTruth`、`Parentheses`、`Unary` 直接继承唯一子节点。
4. `Variable` 在可选值的传播结果上叠加变量位；无值变量只留下变量位。
5. 第一阶段结束后，第二个 `match` 为 `ColumnName`、`Position`、`Values` 写入引用位，为 `Default` 写入默认值位，为 `Subquery` 写入子查询位。

`Leaf` 不被重算，其预置 `flag` 被保留。这让测试可以把尚未建模的原子节点性质注入树中。`FLAG_PRE_EVALUATED` 也不会由 `set_flag` 主动产生，只能从这样的预置叶子或直接字段状态向上传播。

## 数据与状态

所有性质都装在一个 `u64` 位集合中，组合运算只有按位或，查询运算是按位与后判断非零。因此同一性质出现多次不会改变结果，子树组合顺序也不影响最终位集合。

`FlagExpr` 完全拥有其树：单子节点使用 `Box`，多子节点使用 `Vec`，可选分支使用 `Option`。传播期间需要根节点的独占可变借用，不依赖全局状态。除 `Binary` 外，多数复合节点将结果缓存到自己的 `flag` 字段；再次调用 `set_flag` 会依据当前子树重算并覆盖该缓存。`Leaf` 的预置值保持不变，引用类和固有性质叶子的旧值则会被规范化覆盖。

`FLAG_CONSTANT == 0` 带来一个重要不变量：把常量位与任何标志按位或不会新增信息；只有最终结果仍为零时，才能解释为未含已建模的非恒定性质。

## 依赖与调用关系

本文件只使用 Rust 标准库的 `String`、`Vec`、`Box` 和 `Option`，不直接调用 `Cargo.toml` 中的外部依赖。`astersql-parser-ast` 的依赖包括相邻 parser crates、`serde`、`serde_json` 和 `url`，但它们不是本文件传播逻辑的直接依赖。

RustCodeGraph 对 `pkg/parser/ast/flag.rs` 识别到 9 个顶层符号，并将 `get_flag`、`set_flag`、`has_agg_flag`、`has_window_flag` 的边主要定位到本文件递归、Go 风格包装函数及独立测试。直接证据包括：

- `set_flag -> set_flag`：对子表达式递归；`set_flag -> get_flag`：聚合子树结果。
- `has_agg_flag -> get_flag`、`has_window_flag -> get_flag`：查询当前根标志。
- `HasAggFlag -> has_agg_flag`、`HasWindowFlag -> has_window_flag`、`SetFlag -> set_flag`：命名包装关系。
- `flag_test.rs` 和 `flag_5_aster_unit_test.rs` 直接构造 `FlagExpr` 并调用本文件 API。
- `node_flags.rs` 通过 `use super::flag::*` 复用标志常量，但为真实 `ExprNode` 实现另一套 `SetFlag`；`yy_parser.rs` 的解析完成路径调用的是该真实 AST 版本。

因为 `functions.rs` 再次以路径声明本文件，`flag_5_aster_unit_test.rs` 从 `crate::functions::flag::*` 访问相同实现，而 `flag_test.rs` 从 `crate::flag` 访问它。扩展公开符号时要同时考虑这两个模块路径。

## 错误处理与边界

本文件 API 不返回 `Result`，也没有显式错误分支。只要调用者能构造合法的 Rust 枚举值，传播就会完成；不存在 SQL 解析错误、名称解析错误或运行时求值错误的处理。

主要边界如下：

- `Leaf` 被信任且不会重算，错误的预置 flag 会原样参与上层合并。
- `Binary` 不缓存根标志，读取行为与多数其他变体不同；修改其子树后无需再次计算根字段，但子树内部的缓存节点仍需调用 `set_flag`。
- 空参数、空列表和缺失可选子节点贡献零；普通函数即使无参数仍含 `FLAG_HAS_FUNC`，聚合/窗口同理保留自身固有位。
- `FLAG_PRE_EVALUATED` 仅被定义，`set_flag` 没有产生它的分支。真实 AST 的 `node_flags.rs` 对未赋新标志的值节点保留原 flag，相关行为由 `node_flags_test.rs` 覆盖；不能据此推断精简模型会自动判定预求值。
- `FlagExpr` 并未覆盖真实 `ExprKind` 的全部形态，例如 `Collate`、introduced value 和若干专用节点只在 `node_flags.rs` 中处理。因此本文件不能替代真实 AST visitor。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件句柄或网络资源。每次传播只在调用者提供的树和当前调用栈中工作，不共享可变全局状态；不同 `FlagExpr` 树可由不同线程独立处理，前提是调用者满足 Rust 的所有权与线程安全约束。

时间复杂度通常为 `O(n)`，其中 `n` 是遍历到的节点数；每个有缓存的节点只聚合其直接子项。空间开销除树本身外主要是递归栈，最坏为 `O(h)`，`h` 是树高。极深的手工构造树可能耗尽调用栈；本文件不实施深度限制。真实 SQL 主链在 `yy_parser.rs` 调用标志传播前会运行 `check_ast_depth_limit`，但该保护不适用于直接调用本文件 `set_flag` 的任意手工树。

## 与 Go 版本的对应关系

实现载体不同。Rust 本文件没有通用 `Node` visitor，而是递归一个独立的 `FlagExpr` 枚举；其测试因而手工构造与 Go SQL 用例结构等价的树。

`pkg/parser/ast/flag_test.go` 会用真实 Go parser 解析 21 类表达式，再调用 `ast.SetFlag`；`pkg/parser/ast/flag_test.rs` 对应覆盖 Between、Case、子查询、IN/LIKE/REGEXP、Row、参数、函数、聚合、变量、DEFAULT、引用和一元表达式等传播，但明确说明尚未通过本 crate 的 SQL parser 驱动这些 `FlagExpr` 用例。

## 扩展指南

新增一种标志时，应选择未占用 bit、在本文件增加常量，并同时检查 `FlagExpr::get_flag`、`set_flag`、`node_flags.rs::expression_flag`、Go `flag.go` 及所有消费该位的下游逻辑。若该性质可由节点自身产生，应在对应节点分支叠加；若只能由子树继承，应保证所有容器分支都能传播。不要把 `FLAG_CONSTANT` 当作普通可置位标志。

新增 `FlagExpr` 变体时，编译器会迫使更新穷尽的 `get_flag` 和两个 `set_flag` 匹配；仍需明确它是缓存自身 flag、动态读取子树，还是保留调用者预置值。若真实 parser 也新增 `ExprKind`，必须独立更新 `node_flags.rs`，因为修改本文件的精简枚举不会自动影响生产解析主链。

测试应放在独立文件而不是本源文件中。精简模型的 Go 对齐用例更新 `pkg/parser/ast/flag_test.rs`，组合分支可更新 `flag_5_aster_unit_test.rs`；真实 AST visitor 或 parser 接线应更新 `node_flags_test.rs` 及 parser 相关测试，并与 `pkg/parser/ast/flag_test.go` 的预期交叉核对。兼容风险主要是 bit 值改变或遗漏传播导致绑定/优化判断错误；性能风险主要来自新增重复遍历或在每个父节点重复扫描深层子树。

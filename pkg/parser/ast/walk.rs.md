# `pkg/parser/ast/walk.rs`

## 文件定位

本文件是 `astersql-parser-ast` crate 的 AST 子节点遍历表与递归适配层。它不是公开模块：[`lib.rs`](lib.rs) 以 `mod walk` 私有装配，但公开的 `Node::accept`、`Node::accept_in_place` 和 `Walk` 会进入这里。因此调用者通常不直接依赖本文件，而是实现 [`lib.rs`](lib.rs) 中的 `Visitor` 或 `InPlaceVisitor`，再从某个 AST 根节点开始深度优先遍历。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名是 `astersql-parser-ast`，库入口是 `lib.rs`；本文件通过 `use super::*` 使用同 crate 的 AST 类型，不直接引入新的外部 crate，也没有 feature 或条件编译项。RustCodeGraph 的文件级关系显示 `walk.rs` 仅由 `pkg/parser/ast/lib.rs` 使用，符合“内部遍历实现、公开协议位于 crate 根”的结构。

本文件存在的核心原因，是把“每个节点应按什么顺序访问哪些字段”从节点类型和公共访问者协议中集中抽出。它覆盖语句节点、表达式枚举、嵌入式值对象、容器和过程 AST 的动态值；不负责 SQL 解析、格式化、语义检查或执行。

## 核心职责

1. 用 `Children` / `MutChildren` 统一描述每种 AST 对象的直接子节点，并让只读访问与原地可变访问保持同一字段顺序。
2. 用私有 `Visit` / `VisitMut` 为具体节点、`dyn Node`、`Option`、`Vec`、`Box`、引用包装器和特殊叶子类型提供递归适配。
3. 用 `children!` 宏声明绝大多数类型的字段顺序；`&&` 链和 `Iterator::all` 使任一子节点返回 `false` 时立即停止整个遍历。
4. 用 `embedded!` 为不是 `Node` trait 对象、但仍属于 AST 结构的值类型补齐 `enter_embedded -> children -> leave_embedded` 生命周期，并在可变路径支持同具体 Rust 类型的替换。
5. 对普通字段列表不能准确表达的节点显式编码分支，包括 `TableSource`、三类 Binding、`FlashBackToTimestampStmt`、`IndexPartSpecification`、`PlanReplayerStmt`、过程节点中的 `dyn Any`、`ResultSetNode`、`ExprKind` 和 `PartitionDefinitionClause`。

这里维护的是遍历结构契约，而不是通用反射。新增 AST 字段不会被自动发现；若它应该参与访问，必须显式加入对应 `children!` 字段表或手写实现。

## 主要符号

- `pub(crate) trait Children` / `MutChildren`：分别暴露 `visit_children(&self, &mut dyn Visitor) -> bool` 和 `visit_children_mut(&mut self, &mut dyn InPlaceVisitor) -> bool`。它们只遍历当前值的直接孩子，不调用当前节点自己的 `enter` / `leave`。
- 私有 `Visit` / `VisitMut`：递归分发接口。`node_visit!` 为大量语句类型和 `ExprNode` 生成到 `accept` / `accept_in_place` 的桥接；`dyn Node` 也直接转发到相同入口。
- 容器适配：`Option<T>` 对 `None` 返回 `true`；`Vec<T>` 按索引顺序使用 `all`；`Box<T>` 解引用后递归。它们共同保证字段声明顺序和集合顺序就是实际访问顺序。
- 引用适配：`WithClauseRef` 通过 `borrow` / `borrow_mut` 访问共享 `WithClause`；`NodeRef` 通过 `with_node` / `with_node_mut` 访问可选动态节点，并在内部没有节点时返回 `true`。
- 值类型叶子：`TableName`、`ColumnName`、`OnDeleteOpt`、`OnUpdateOpt` 使用专门回调，不经过 `Node::accept`；它们没有可递归孩子，仍保证调用相应的 enter/leave。`TimeUnitType` 则走 embedded 回调。
- `children!`：为类型同时生成只读与可变字段遍历。空字段表表示可访问但无孩子的对象；非空字段按宏实参从左到右短路求值。
- `embedded!`：为一组值对象实现 `Visit` / `VisitMut`。可变版本在 enter 后和 leave 后分别调用替换钩子；返回值必须能向下转型回原具体类型，否则 panic。
- `procedure_any_visitors!` 及生成的 `visit_procedure_any` / `visit_procedure_any_mut`：按白名单对过程 AST 中的 `dyn Any` 值做运行时向下转型；识别 `Box<dyn Node>` 或列出的过程/表达式类型，未知类型视为无可访问孩子并返回 `true`。
- `ExprKind`、`ResultSetNode`、`PartitionDefinitionClause` 的显式实现：穷尽匹配枚举变体，叶子返回 `true`，带孩子的变体按语义顺序递归。这里是新增枚举变体时最重要的编译期同步点。

本文件没有模块级常量、结构体或公开函数；公开遍历函数 `Walk` 和访问者 trait 位于 [`lib.rs`](lib.rs)。

## 执行流程

典型可变遍历从 `Walk(node, visitor)` 开始：

1. [`lib.rs`](lib.rs) 的 `Walk` 调用根节点 `Node::accept_in_place`。
2. 节点先调用 `InPlaceVisitor::enter`，保存其 `skip_children` 决定；随后应用可选的 `enter_replacement`，且替换后的节点将作为后续子树遍历对象。
3. 若未跳过孩子，节点调用 `MutChildren::visit_children_mut`。本文件依据该节点的字段表或显式分支，从左到右调用每个孩子的 `VisitMut::visit_mut`。
4. 具体 `Node` 孩子再次进入 `accept_in_place`；`Option`、`Vec`、`Box` 和共享引用只是递归适配；embedded 值执行自己的 enter/children/leave 链。
5. 任一孩子的 leave 返回 `false` 时，`&&` / `all` 立即短路，父节点不再访问余下兄弟，也不执行父节点 leave。全部孩子成功后才执行当前节点 leave 和可选 `leave_replacement`。

只读路径由 `Node::accept`、`Visitor`、`Children::visit_children` 和 `Visit::visit` 构成，顺序与可变路径一致，但不提供节点替换。当前节点 enter 返回 `true` 时跳过其孩子但仍调用当前节点 leave；这是 [`lib.rs`](lib.rs) 的 `accept` 实现负责的控制流。

特殊分支改变的只是“哪些孩子存在”：例如 `TableSource` 优先访问 `QuerySource`，否则访问 `Source`；`IndexPartSpecification` 优先 `Expr`，否则 `Column`；`PlanReplayerStmt.Load=true` 时没有孩子，非 Load 模式先访问 `HistoricalStatsInfo`，再在 `Stmt` 与 `Where/OrderBy/Limit` 两条路径间选择；`FlashBackToTimestampStmt` 有非零 `FlashbackTSO` 时不访问 `FlashbackTS` 表达式。

## 数据与状态

本文件不持有全局状态、缓存或注册表。遍历状态全部由调用方提供的 `&mut Visitor` / `&mut InPlaceVisitor` 保存，AST 状态则通过共享或可变借用传入。字段表本身是编译期宏展开的控制流，不会在运行时构建元数据表。

关键不变量包括：

- 访问顺序必须与对应 AST 的语义/Go 实现一致；例如 `SelectStmt` 的字段顺序是 `With`、提示、字段、`From`、过滤、分组、窗口、排序、限制、锁信息和附加孩子。
- 只读与可变 `Children` 实现必须覆盖相同字段并保持相同顺序；`children!` 自动保证这一点，手写实现需人工保持镜像。
- `false` 表示停止整次遍历，而不是仅跳过当前节点的余下孩子；`true` enter 才表示“跳过当前孩子但仍 leave”。
- 可变替换必须保持当前具体 Rust 类型。节点替换由 [`lib.rs`](lib.rs) 的 `replace_node<T>` 检查，embedded 替换由本文件 `embedded!` 的 `downcast::<$name>()` 检查。
- `procedure_any_visitors!` 是封闭的类型白名单；放入过程容器的新 `Any` 类型若未加入宏列表，会被静默当成叶子。

[`go_merge_27_test.rs`](go_merge_27_test.rs) 还验证了不启用替换钩子时，框架遍历不会改写字段、重分配 `Vec` 或产生额外分配；visitor 自己的回调仍可修改节点或分配资源。

## 依赖与调用关系

上游公开入口位于 [`lib.rs`](lib.rs)：`Walk` 调用 `accept_in_place`；各 `Node` 实现的 `accept` 调用 `walk::Children::visit_children`，`accept_in_place` 调用 `walk::MutChildren::visit_children_mut`。RustCodeGraph 给出的直接文件边是 `lib.rs -> walk.rs`，并识别出 `accept -> visit_children -> visit/accept` 的递归链。生产调用方只依赖公开的 `Walk` / `Node` / visitor trait，因此本文件可见性保持为 crate 内部。

下游依赖都是同 crate 类型和方法：AST 定义来自 [`lib.rs`](lib.rs) 及其聚合模块；`NodeRef::with_node(_mut)`、`Node::accept(_in_place)`、专用叶子回调和 `Any::downcast_*` 构成实际分发边。Cargo 声明中的 `parser-auth`、`parser-charset`、`parser-mysql`、`parser-types`、`serde`、`serde_json`、`url` 是整个 AST crate 的依赖，但 `walk.rs` 没有直接调用这些外部库。

关键测试调用边包括：[`ast_1_aster_unit_test.rs`](ast_1_aster_unit_test.rs) 和 [`go_merge_23_test.rs`](go_merge_23_test.rs) 从 `Walk` 验证根到孩子的顺序、跳过与短路；[`go_merge_13_test.rs`](go_merge_13_test.rs) 直接调用 `Children` / `MutChildren` 验证分区结构；[`go_merge_25_test.rs`](go_merge_25_test.rs) 覆盖多种手写字段分支；[`go_merge_27_test.rs`](go_merge_27_test.rs) 覆盖替换、embedded、停止传播和分配性质。

## 错误处理与边界

遍历接口不用 `Result`；布尔值是唯一的正常控制信号。`true` 表示允许继续或该分支完成，`false` 表示立即中止。停止从孩子向上传播时，未执行的父级 leave 不会获得清理机会，因此 visitor 不应依赖“每次 enter 必然对应 leave”来释放必须释放的外部资源；需要强保证的清理应使用 Rust 自身的作用域/RAII。

可变替换的类型错误是编程错误，不会转成 `false`：`replace_node<T>` 和 `embedded!` 都会在向下转型失败时 panic，并给出“replacement must preserve ... Rust type”的信息。`WithClauseRef` 的 `RefCell::borrow` / `borrow_mut` 也可能在违反动态借用规则时 panic。`NodeRef` 内部为 `None`、普通 `Option` 为 `None`、过程 `Any` 类型不在白名单时不会报错，而是按无孩子成功处理；后两种宽松边界尤其需要扩展者注意。

枚举实现通过穷尽 `match` 防止新增 `ExprKind`、`ResultSetNode` 或 `PartitionDefinitionClause` 变体后漏编译，但结构体新增字段不会触发遍历覆盖错误。空 `children!` 也不表示对象没有业务字段，只表示这些字段当前不被当作 AST 子节点访问。

## 并发与资源生命周期

遍历是同步、递归、单 visitor 的深度优先过程，不创建线程、异步任务、锁、通道、事务或 I/O 资源。`Visitor` / `InPlaceVisitor` 以独占可变借用贯穿调用链，所以一次遍历内不会并发调用同一个 visitor；是否在多个线程共享 AST 或 visitor 由上层类型和同步策略决定。

`WithClauseRef` 和 `NodeRef` 基于 `Rc<RefCell<...>>`，属于单线程共享所有权与运行时借用模型，不能据此声称 AST 可跨线程并发遍历。借用只覆盖对应递归调用，但 visitor 若在回调中重新借用同一 `RefCell` 仍可能触发冲突。

资源生命周期与调用栈一致：容器迭代器、节点借用和 visitor 借用在递归返回时释放；替换值在 enter 或 leave 阶段移动进原位置，旧值随普通 Rust 所有权规则被释放。常规路径不建立额外长期所有权。深度与 AST 嵌套深度线性相关，极端深树存在递归栈风险；宽节点的时间复杂度与实际访问的边数线性相关，短路可提前结束。

## 与 Go 版本的对应关系

Go 没有同名 `walk.go`；对应语义分布在 [`ast.go`](ast.go) 的 `Visitor` / `InPlaceVisitor` / `Walk` 契约、各 AST 文件的 `Accept` 方法，以及生成文件 [`visitor_inplace_generated.go`](visitor_inplace_generated.go) 的 `AcceptInPlace` 方法。Rust 把大量字段遍历集中到本文件，并由 [`lib.rs`](lib.rs) 的通用 `Node` 实现模板调用。

两端共同保持 enter-before-children、按字段/切片顺序深度优先、skip 时仍 leave、leave 返回 false 时全局短路的语义。Go `visitor_inplace_generated.go` 中 `IndexPartSpecification` 的 Expr/Column 二选一、`ReferenceDef` 的 Table/索引列/删除更新动作顺序等，与本文件手写或 `children!` 路径对应；Rust 回归测试直接固定了这些顺序。

表示方式存在差异：Go 传统 `Visitor.Enter/Leave` 返回替换节点，`InPlaceVisitor` 只原地修改；Rust 的只读 `Visitor` 不替换，而 `InPlaceVisitor` 另有可选的 enter/leave replacement 钩子，并要求替换保持完全相同的具体 Rust 类型。Rust 还需为不是 `Node` 的值类型提供 embedded 和专用叶子回调，并为 `Option`、`Vec`、`Box`、`Rc<RefCell<_>>` 写泛型适配；这些是 Rust 数据模型带来的实现层差异，不改变目标访问顺序。

Go 测试 [`visitor_test.go`](visitor_test.go) 及各 AST 的 `*_test.go` 覆盖传统访问者和生成遍历；Rust 对应的独立测试主要是 [`ast_1_aster_unit_test.rs`](ast_1_aster_unit_test.rs)、[`go_merge_13_test.rs`](go_merge_13_test.rs)、[`go_merge_23_test.rs`](go_merge_23_test.rs)、[`go_merge_25_test.rs`](go_merge_25_test.rs)、[`go_merge_27_test.rs`](go_merge_27_test.rs) 以及各模块的 visitor cover 测试。Rust 测试逻辑保持在独立文件，没有内嵌到 `walk.rs`。

## 扩展指南

新增或修改 AST 结构时，应先判断孩子属于哪类并在最窄位置接线：

1. 新增普通 `Node` 类型：将它加入 `node_visit!`，并用 `children!(Type => field1, field2, ...)` 按 Go/语义顺序声明孩子；同时确认 [`lib.rs`](lib.rs) 已为该类型实现 `Node`。
2. 新增 embedded 值类型：提供 `Children` / `MutChildren`，再加入 `embedded!`；若它只是专用叶子，考虑像 `TableName` 一样提供明确回调，而不是伪装成 `Node`。
3. 修改 `ExprKind`、`ResultSetNode`、`PartitionDefinitionClause`：同步只读和可变两个穷尽 match，保持字段顺序镜像。修改 Binding、Plan Replayer、TableSource、Flashback、索引字段或过程容器时，优先更新现有手写分支。
4. 过程 `dyn Any` 容器新增合法值类型时，必须加入 `procedure_any_visitors!`；否则遍历会静默跳过该值。新增共享引用包装器时还要明确空值和借用冲突语义。
5. 在独立测试中至少固定：enter/children/leave 顺序、skip、false 短路、可变写回；有条件分支时覆盖每条分支，有替换时覆盖 enter 与 leave 替换及错误类型约束。优先扩展现有 [`go_merge_25_test.rs`](go_merge_25_test.rs) / [`go_merge_27_test.rs`](go_merge_27_test.rs)，分区结构使用 [`go_merge_13_test.rs`](go_merge_13_test.rs)。

兼容性风险主要是漏孩子、字段次序改变或停止传播错误，这会影响名称收集、改写、标志分析等所有 visitor；性能风险是引入分配、克隆或重复访问。保持宏字段列表简单、原地遍历零框架分配，并与 Go `Accept` / `AcceptInPlace` 逐字段对照，可降低这些风险。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/ast` 定位 `walk.rs` 的 57 个符号；`node --file pkg/parser/ast/walk.rs` 分段读取全 939 行并确认文件只由 `lib.rs` 使用；`explore` 识别 `accept -> visit_children -> visit/accept` 链。一次针对通用 `visit` 名称的精确 callers/callees 查询超时，未把其无结果当作证据，而以文件边、精确源码和调用检索交叉核对。
- Rust 生产源码：完整核对 [`walk.rs`](walk.rs)；读取 [`lib.rs`](lib.rs) 的 `Visitor`、`InPlaceVisitor`、`Walk`、`Node`、`SelectStmt` / `simple_node!` 的 accept 实现、`NodeRef` / `WithClauseRef` 和 `mod walk`；读取 [`Cargo.toml`](Cargo.toml) 的 crate 入口、依赖及 Go 包迁移元数据。
- Go 对照：读取 [`ast.go`](ast.go) 的访问者协议和 `Walk`；读取 [`visitor_inplace_generated.go`](visitor_inplace_generated.go) 的生成遍历，且通过源码检索定位各 AST 文件的 `Accept` 实现；确认 Go 没有同名 `walk.go`。
- Rust 测试：读取 [`ast_1_aster_unit_test.rs`](ast_1_aster_unit_test.rs)、[`go_merge_23_test.rs`](go_merge_23_test.rs)、[`go_merge_25_test.rs`](go_merge_25_test.rs)、[`go_merge_27_test.rs`](go_merge_27_test.rs) 的直接遍历用例，并检索 [`go_merge_13_test.rs`](go_merge_13_test.rs) 及其他 visitor cover 测试。证据覆盖顺序、跳过、短路、字段写回、类型替换、条件分支、特殊叶子和零框架分配。
- 本任务只新增说明文档，按计划未运行 Cargo。交付前运行任务指定的结构命令，验证目标存在且恰好含 11 个固定二级标题；同时人工复核本文说明了文件职责、真实入口、扩展接点和未验证边界，没有把理想设计写成当前事实。

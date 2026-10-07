# `pkg/parser/ast/ast.rs`

## 文件定位

本文件是 `astersql-parser-ast` crate 中面向 Go `pkg/parser/ast/ast.go` 的兼容接口层。crate 入口 `pkg/parser/ast/lib.rs` 通过 `#[path = "ast.rs"] pub mod ast;` 装配它；`pkg/parser/ast/Cargo.toml` 将 crate 名定义为 `astersql-parser-ast`，并把 `lib.rs` 设为库入口。它不负责 SQL 词法或语法分析，也不保存完整 AST 结点实现，而是集中提供表达式标志常量、若干分类 trait、两个轻量语义结构，以及语句种类到观测标签的映射。

需要特别区分同一 crate 根部的具体数据类型：`lib.rs` 还定义了具体的 `ExprNode`、`ResultSetNode` 等结构/枚举，而本文件的 `ast::ExprNode`、`ast::ResultSetNode` 是 trait。调用者应通过完整模块路径判断所指类型，不能仅凭同名符号推断。

## 核心职责

1. 以 `FlagConstant` 至 `FlagHasWindowFunc` 固定 Go AST 表达式标志的位布局，供迁移代码表达“含参数、函数、引用、聚合、子查询、变量、DEFAULT、预求值或窗口函数”等性质。
2. 以 `ExprNode`、`FuncNode`、`StmtNode`、`DDLNode`、`DMLNode`、`ResultSetNode`、`SensitiveStmtNode` 描述 Go 接口层次；共同基类 `Node` 以及访问者协议由 crate 根部重新导出。
3. 以 `OptBinary` 和 `VectorElementType` 携带语法动作需要的简单值。
4. 以 `StatementKind` 将 Go 的具体语句类型和少量字段条件压缩成可穷举的 Rust 枚举，再由 `GetStmtLabel` 生成监控、日志等使用的稳定标签。

本文件当前更接近“移植契约和分类门面”，不是完整运行时 AST 主模型。仓库搜索没有发现这些分类 trait 的具体 `impl`；`GetStmtLabel` 的直接 Rust 使用证据位于独立测试 `pkg/parser/ast/ast_1_aster_unit_test.rs`。实际解析器大量操作的是 crate 根部 `parser_ast::ExprNode`、`parser_ast::ResultSetNode` 等具体类型。

## 主要符号

- `FlagConstant: u64 = 0`：纯常量的空标志。
- `FlagHasParamMarker` 至 `FlagHasWindowFunc`：连续使用第 0 至第 8 位，可按位或组合；具体数值由 `ast_flags_and_statement_labels_match_go` 验证。
- `ExprNode: Node`：增加 `FieldType`、标志读写和 `Format(&mut dyn Write) -> std::io::Result<()>`。`FieldType` 从 `parser_types::types` 导出。
- `OptBinary { IsBinary, Charset }`：保存可选 `BINARY` 修饰和字符集名；派生 `Default` 时分别为 `false` 与空字符串。
- `VectorElementType { Tp }`：保存单字节 MySQL 类型编号。Go 注释限定 FLOAT/DOUBLE，本结构本身不校验取值。
- `FuncNode: ExprNode`：以私有风格标记方法 `functionExpression` 区分函数表达式。
- `StmtNode: Node`：以 `statement` 作分类标记，并通过 `SEMCommand` 返回 SEM 命令名。
- `DDLNode`、`DMLNode`：在 `StmtNode` 上分别增加 `ddlStatement`、`dmlStatement` 标记。
- `ResultSetNode: Node`：以 `resultSet` 标记能产生结果集的结点。
- `SensitiveStmtNode: StmtNode`：要求 `SecureText` 提供隐藏敏感信息的文本。
- `StatementKind`：包含普通单元变体，以及携带判别字段的 `DropTable { is_view }`、`Explain { show, analyze }`、`Insert { is_replace }`。
- `GetStmtLabel(&StatementKind) -> String`：穷举枚举并返回兼容 Go 的标签；函数总会分配一个新的 `String`。
- `pub use crate::{InPlaceVisitor, Node, Visitor}`：把 crate 根部协议带入 `ast` 模块命名空间，但没有在本文件重新实现遍历。

## 执行流程

本文件唯一包含分支执行逻辑的入口是 `GetStmtLabel`：

1. 调用者先把具体语句归类为 `StatementKind`；本文件没有提供从 crate 根 AST 结点自动完成该转换的函数。
2. `match` 对普通变体直接选择静态标签。
3. `DropTable` 根据 `is_view` 先区分 `DropView` 与 `DropTable`。
4. `Explain` 的优先级是 `show` 高于 `analyze`：只要 `show == true` 就返回 `DescTable`；否则按 `analyze` 返回 `ExplainAnalyzeSQL` 或 `ExplainSQL`。
5. `Insert` 根据 `is_replace` 返回 `Replace` 或 `Insert`；`Set` 与 `SetPassword` 合并为 `Set`。
6. `OptimizeTable` 保持历史标签 `Optimize`，`Other` 回退为小写 `other`；最后把静态字符串复制为拥有所有权的 `String`。

trait 方法只声明契约，不在本文件内形成可执行调用链。遍历的实际入口和控制流位于 `pkg/parser/ast/lib.rs` 的 `Node::accept`、`Node::accept_in_place`、`Visitor`、`InPlaceVisitor` 与 `Walk`，并由具体结点实现决定子结点顺序。

## 数据与状态

表达式标志是 `u64` 位集：零表示没有特殊性质，非零位可以组合。调用方必须保留既有位号，因为它们是与 Go 对照及下游判断之间的兼容契约。注意 `pkg/parser/ast/flag.rs` 的 `FLAG_HAS_*` 属于另一个精简传播模型，当前从第 1 位开始；它与本文件从第 0 位开始的 `FlagHas*` 不是同一组常量，不应混用。

`OptBinary`、`VectorElementType` 和 `StatementKind` 都是拥有所有权的普通值，没有内部可变性、缓存、全局注册表或隐藏资源。`StatementKind` 派生 `Copy`，适合按值传递；`OptBinary` 含 `String`，只能克隆而不能隐式复制。分类 trait 的状态由实现者提供，例如 `ExprNode` 要求实现者持有 `FieldType` 和标志值，但本文件不规定字段布局。

## 依赖与调用关系

- crate 内上游：`pkg/parser/ast/lib.rs` 声明 `pub mod ast`；测试模块 `ast_1_aster_unit_test.rs` 通过 `use crate::{ast::*, base::*}` 直接验证本文件常量和标签。
- crate 内下游：`ExprNode` 依赖 crate 根 `Node`，并依赖 `parser_types::types::FieldType` 与标准库 `std::io::Write`；其余分类 trait 只扩展本文件或 crate 根 trait。
- crate 边界：`pkg/parser/ast/Cargo.toml` 声明 `parser-types` 等本地依赖；本文件直接使用的外部 crate 只有 `parser-types`，没有 feature 条件或条件编译项。
- 应用主链：Go 版本的 `ast.GetStmtLabel` 经 `pkg/sessionctx/stmtctx/stmtctx.go` 被 session、executor、planner 与指标路径调用。RustCodeGraph 对 Rust `GetStmtLabel` 的符号查询只显示本定义及其独立测试，仓库文本搜索也未发现生产 Rust 调用，因此不能声称 Rust 标签函数已经接入对应运行时主链。
- 解析主链：Rust 解析动作在 `pkg/parser/parser_actions/**` 中构造 crate 根部具体 AST 类型；例如 `parser_semantic_support.rs` 和 expression/query 动作广泛使用 `parser_ast::ExprNode`。这些引用不是本文件同名 trait 的调用者。

RustCodeGraph 能索引 `ast.rs` 并识别 `ExprNode`、`StmtNode`、`GetStmtLabel` 等符号，但本次 `explore`/文件 `node` 未返回正文，`callers` 查询也未形成可用输出；调用关系因此用精确 `query` 结果和仓库引用搜索交叉核验，并在此明确验证边界。

## 错误处理与边界

`GetStmtLabel` 对全部 `StatementKind` 变体穷举匹配，不返回 `Result`；无法具体分类的语句必须由上游显式构造成 `Other`，得到 `other`。新增枚举变体若没有同步匹配分支会在编译期暴露非穷举错误。

`ExprNode::Format` 是本文件唯一显式可失败接口，错误类型直接沿用 `std::io::Error`，实现者应传播底层 writer 失败。其余 trait 方法不返回错误；例如 `SetType`、`SetFlag` 不能表达校验失败。`VectorElementType::Tp` 没有运行时校验，调用者需保证 Go 注释所述 FLOAT/DOUBLE 约束。`SecureText` 返回字符串但本文件不保证具体脱敏规则，安全性取决于实现者。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或显式析构逻辑。所有结构都按 Rust 所有权正常释放；`GetStmtLabel` 返回的新 `String` 与输入枚举生命周期无关。

访问者只在此处被重新导出。`pkg/parser/ast/lib.rs` 明确把可变访问和并发同步责任交给调用者：`InPlaceVisitor` 能修改 AST，但协议本身不提供同步。若同一 AST 被跨线程共享，调用方必须在更外层建立互斥或其他同步，并满足具体结点类型的 `Send`/`Sync` 条件；本文件没有提供这些保证。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/parser/ast/ast.go`。标志位名称和值、`OptBinary`、`VectorElementType`、接口分类和标签文本整体按其移植。`pkg/parser/ast/ast_1_aster_unit_test.rs` 覆盖全部旧有标签特例和物化视图扩展标签，并验证 `Explain(show=true, analyze=true)` 仍优先得到 `DescTable`。

关键差异如下：

- Go `GetStmtLabel` 接收动态 `StmtNode` 并通过具体类型 switch 分类；Rust 接收预先归类的 `StatementKind`，因此类型到枚举的转换责任已移到调用者，但本文件尚无转换适配器。
- Go `ExplainStmt` 通过内部 `Stmt` 是否为 `ShowStmt` 判断 `DescTable`；Rust 用布尔字段 `show` 表示这一事实。
- Go `ExprNode::SetType` 接收 `*FieldType`，`Format` 没有错误返回；Rust 按值接收 `FieldType`，返回借用，并让 `Format` 返回 `std::io::Result<()>`。
- Go `Node`、`Visitor`、`InPlaceVisitor` 的完整接口位于同一文件；Rust 的实际基础协议在 `lib.rs`，本文件仅重新导出，且访问者回调签名与 Go 的结点替换模型不同。
- Go 的 `ResultSetNode` 注释提到 `ResultFields` 属性；Rust trait 只有标记方法，没有声明该属性。

因此这里的“对应”是语义契约对齐，不是逐签名 ABI 对齐，也不能据此推断所有 Go 实现类型都已有 Rust trait 实现。

## 扩展指南

- 新增语句标签时，应同时增加 `StatementKind` 变体与 `GetStmtLabel` 分支，并在独立文件 `pkg/parser/ast/ast_1_aster_unit_test.rs` 增加普通路径、字段特例和优先级断言；同时核对 `pkg/parser/ast/ast.go` 的真实增量，避免 Rust 独自发明分类。
- 若要把标签接入 Rust 生产主链，应新增从真实 crate 根 AST 语句模型到 `StatementKind` 的局部适配，并为未知类型保留明确回退；不要把测试用枚举当成已接线的解析结果。
- 新增表达式 flag 必须选择未占用位，更新 Go 对照和独立测试；还需判断 `pkg/parser/ast/flag.rs` 的精简传播模型是否需要独立映射，不能直接复制数值。
- 为分类 trait 增加方法会影响所有实现者；当前没有搜索到实现时也应先用 RustCodeGraph/`rg` 复核，因为后续迁移可能已增加实现。测试逻辑继续放在独立 `*_test.rs`，不要内嵌进生产源文件。
- 扩展 `VectorElementType` 时应决定校验发生在解析动作还是构造 API，避免让任意 `u8` 静默进入下游。修改 `SensitiveStmtNode` 时应以不可泄露凭据为兼容底线。
- 性能上，频繁调用 `GetStmtLabel` 会分配 `String`；若生产接线需要消除分配，必须同步评估调用接口和 Go 兼容语义，而不能只局部改返回类型。

## 验证依据

- 目标源码：`pkg/parser/ast/ast.rs`，共 208 行；核对了 9 个标志常量、2 个结构、7 个分类 trait、`StatementKind` 与 `GetStmtLabel`，未发现条件编译项或实现块。
- crate 边界与装配：`pkg/parser/ast/Cargo.toml`、`pkg/parser/ast/lib.rs`；后者在 `#[path = "ast.rs"] pub mod ast` 处公开模块，并定义真实 `Node`/访问者/具体 AST 类型。
- Go 对照：`pkg/parser/ast/ast.go`；应用侧 Go 引用由 `pkg/sessionctx/stmtctx/stmtctx.go`、`pkg/session/session.go`、`pkg/executor/compiler.go` 等搜索结果佐证。
- Rust 独立测试：`pkg/parser/ast/ast_1_aster_unit_test.rs`；`ast_flags_and_statement_labels_match_go` 覆盖标志值和标签，`go_merge_7_materialized_view_statement_labels` 覆盖物化视图标签，其他测试说明遍历基础协议实现在 crate 根部。
- 相邻边界证据：`pkg/parser/ast/flag.rs` 与 `pkg/parser/ast/flag_test.rs`，用于确认另一套传播常量及组合行为，不将其误认成本文件 API。
- RustCodeGraph：`status` 显示目标仓库已索引（11,467 个文件）；`files --filter pkg/parser/ast` 列出 `ast.rs`、Go 对照与测试；`query ExprNode --kind trait`、`query StmtNode --kind trait`、`query GetStmtLabel --kind function` 分别定位本文件符号及 Go 对照。图的 `explore/node/callers/callees` 没有提供可用调用边输出，因此最终调用边以精确仓库搜索补证，没有据空结果推断生产接线。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前以任务规定命令验证文档存在且恰好包含 11 个固定二级标题，并人工复核所有“已接线/已支持”陈述均有上述路径或符号依据。

# `pkg/parser/ast/base.rs`

## 文件定位

`base.rs` 是 `astersql-parser-ast` crate 的 AST 公共基础层。crate 根在 `pkg/parser/ast/Cargo.toml` 中把 `lib.rs` 指定为入口，`lib.rs:4966` 以 `pub mod base` 暴露本文件；`lib.rs:183-204` 定义的对象安全 `Node` trait，再通过 `node_text`/`node_text_mut` 把所有节点的文本操作委托给这里的 `AstNode`。

它位于“解析器生成节点”与“恢复、日志、优化器等消费者”之间：`pkg/parser/parser_actions/misc.rs:1507-1510` 在语句归约后写入原 SQL、编码及 `NO_BACKSLASH_ESCAPES` 状态；`pkg/parser/ast/sql_restore.rs:76,697,707,901`、`pkg/session/runtime/dispatch.rs:675,689-693` 和 `pkg/server/conn.rs:732` 等下游读取规范化文本。本文件不负责词法/语法分析或 AST 遍历，而负责节点共有的源码文本、偏移、表达式类型/标志，以及 Go 风格分类基类。

## 核心职责

1. `AstNode`/`NodeText` 保存节点原始字节、源字符编码、解析时 SQL mode、源位置和惰性 UTF-8 缓存（`base.rs:28-41`）。空节点不分配 `NodeText`，第一次写入才由 `text_mut` 创建（`base.rs:73-76`）。
2. `SetText`、`Text`、`OriginalText` 和位置方法提供与 Go `Node` 文本契约对应的状态访问；变更原文或会影响反斜杠语义的 mode 时清除缓存（`base.rs:78-125`）。
3. `convertBinaryStringLiterals` 将整段 SQL 解码为 UTF-8，同时把引号内无法严格解码或包含控制字符的内容转换成基于原始字节的 `0x...`，使文本可安全用于日志与 SQL restore（`base.rs:227-344`）。
4. `StmtNodeBase`、`DdlNodeBase`、`DmlNodeBase`、`ExprNodeBase`、`FuncNodeBase` 提供与 Go 嵌入基类同名的分类标记和表达式元数据容器，并保留 Go 风格别名以便移植代码接线（`base.rs:346-437`）。

## 主要符号

- `AstNode { text: Option<Box<NodeText>> }`：公开、可克隆的节点文本句柄。`Option<Box<_>>` 让默认节点保持轻量；自定义 `PartialEq` 把 `None` 与完全默认的 `NodeText` 视为相等（`base.rs:28-32,62-71`）。
- `NodeText`：私有状态，包含 `OnceCell<String>`、`Option<EncodingRef>`、`no_backslash_escapes`、原始 `Vec<u8>` 与 `i32` 偏移。其 `PartialEq` 刻意不比较派生缓存，而用编码名比较编码身份（`base.rs:34-61`）。
- `AstNode::{SetOriginTextPosition, OriginTextPosition}`：设置/读取节点在源 SQL 中的起始偏移；未分配状态时读取为 `0`（`base.rs:78-86`）。
- `AstNode::{SetText, SetNoBackslashEscapes, Text, OriginalText}`：写入原始字节和编码、维护缓存，并分别返回规范化的拥有型 `String` 与借用的原始字节切片（`base.rs:88-125`）。
- `decode`：调用 `EncodingRef::Transform`；宽松模式使用 `OpDecodeReplace`，严格模式使用 `OpDecode`，最终还要求转换结果是合法 UTF-8（`base.rs:127-136`）。
- `is_printable`、`is_ident_char`、`needs_space_before_hex_literal`：分别判定 Unicode 控制字符、ASCII 标识符字符，以及替换成十六进制前是否需要插空格以避免 `_binary0x...` 一类词法粘连（`base.rs:138-151`）。
- `advance_orig_to`、`skip_to_eol`、`skip_to_block_end`、`skip_comment`：维持 UTF-8 解码文本与原始字节的双游标同步，并跳过普通 SQL 注释；`/*!` 和 `/*+` 作为可执行内容不跳过（`base.rs:153-225`）。
- `convertBinaryStringLiterals(text, encoding, no_backslash_escapes) -> String`：本文件的核心转换入口（`base.rs:228-344`）。
- `StmtNodeBase`/`DdlNodeBase`/`DmlNodeBase`/`FuncNodeBase`：包含下一级基类，并用空标记方法表达分类（`base.rs:350-378,413-421`）；对应 trait 定义位于 `pkg/parser/ast/ast.rs:68-87`。
- `ExprNodeBase`：持有 `AstNode`、`FieldType` 和私有 `u64 flag`，用 `SetType`/`GetType`、`SetFlag`/`GetFlag` 访问（`base.rs:380-408`）；标志含义见 `pkg/parser/ast/ast.rs:25-44`。
- `node`、`stmtNode`、`ddlNode`、`dmlNode`、`exprNode`、`funcNode` 与 `TexprNode`：兼容 Go 命名和移植调用面的类型别名（`base.rs:346-348,410-437`）。

## 执行流程

节点文本主流程如下：

1. 解析动作调用 `Node::SetText`；`lib.rs:193-195` 转发到 `AstNode::SetText`，后者保存编码和原始字节并清除既有 UTF-8 缓存。解析动作随后按 SQL mode 调用 `SetNoBackslashEscapes`（`pkg/parser/parser_actions/misc.rs:1507-1510`）。
2. 首次调用 `Text` 时，无 `NodeText` 返回空串；编码为 `None` 时直接做有损 UTF-8 转换但不执行二进制字面量规范化；有编码时通过 `OnceCell::get_or_init` 计算并缓存 `convertBinaryStringLiterals` 的结果（`base.rs:106-119`）。
3. 转换函数先以替换策略解码整段输入。若解码结果没有单/双引号立即返回，这是无引号快路径（`base.rs:233-241`）。
4. 否则同时推进解码后的 `index` 与原始字节的 `orig_index`。普通 `--`、`#`、`/*...*/` 注释整体跳过；MySQL 版本注释 `/*!...*/` 和 hint `/*+...*/` 仍按 SQL 扫描（`base.rs:249-263`、`skip_comment`）。
5. 遇到引号后，识别相同引号、成对引号和（mode 允许时）反斜杠转义，在两个表示中定位同一个字面量。未找到完整边界时不转换该段（`base.rs:267-297`）。
6. 对原始内容做严格解码；合法且无控制字符就保留原字面量。否则按 SQL 转义规则展开内容，必要时在前方插空格，再逐字节写成小写十六进制（`base.rs:298-334`）。
7. 只有第一次实际替换时才分配输出缓冲区；无替换直接返回已解码字符串，有替换则追加最后一段尾部文本（`base.rs:324-343`）。

## 数据与状态

- 权威输入是 `NodeText.text: Vec<u8>`；`utf8_text` 是可丢弃、可重建的派生值。`OriginalText` 始终借用权威字节，`Text` 返回缓存内容的克隆，因此调用者不能通过返回值修改内部状态。
- `encoding: None` 表示 Go 的 nil encoding 契约：不运行 `convertBinaryStringLiterals`。对非 UTF-8 原始字节，此分支使用 `String::from_utf8_lossy`，因此 `Text` 可能含替换字符，而 `OriginalText` 仍精确保留字节（`base.rs:111-123`）。
- `SetText` 总是清缓存；`SetNoBackslashEscapes` 仅在值真正变化时清缓存。偏移变化不影响文本缓存（`base.rs:78-103`）。
- 克隆会复制原始状态及当时缓存，之后两份节点独立修改；`base_test.rs:272-295` 验证克隆隔离与 `Box<dyn Node>` 下的文本访问。
- `ExprNodeBase.field_type` 和 `flag` 是私有状态，必须经访问方法更新；`embedded_node` 公开，用于外层节点转发基础 `Node` 行为（`base.rs:382-407`）。

## 依赖与调用关系

直接 crate 依赖只有两类：`crate::ast::FieldType` 提供表达式结果类型，`parser_charset::encoding::{EncodingRef, OpDecode, OpDecodeReplace}` 提供字符集转换；转义展开下调 `crate::util::UnescapeChar`（`base.rs:20-23,316`）。`pkg/parser/ast/Cargo.toml` 证明这些分别来自同 crate AST 模块、路径依赖 `../charset`，而 `FieldType` 最终由路径依赖 `../types` 导出；本文件没有 feature 条件或条件编译项。

RustCodeGraph 索引显示 `base.rs` 被 71 个文件使用，但对常见方法名没有生成可用的精确 callers/callees 边；因此直接调用证据由源码补足：

- 上游：`pkg/parser/parser_actions/misc.rs:1507-1510` 设置语句文本和 SQL mode；`pkg/parser/yy_parser.rs:282` 设置表达式源偏移。
- trait 转发：`pkg/parser/ast/lib.rs:183-204` 的 `Node` 默认方法调用 `AstNode`，而同文件大量具体节点持有 `base::AstNode`（例如 `DoStmt`、`CallStmt`、`ShowStmt` 在 `lib.rs:206-217,294-323`）。
- 下游：`pkg/parser/ast/sql_restore.rs` 在恢复表达式/语句时读取 `Text`；session dispatch 用它生成日志文本；server connection 用它识别规范化后的 SQL 起始内容。
- `convertBinaryStringLiterals` 的直接生产调用者只有 `AstNode::Text`（`base.rs:114-117`），测试通过 `SetText` 间接覆盖它。

## 错误处理与边界

- API 不向外返回转换错误。整段解码采用替换策略；严格解码失败被解释为“该字面量不是安全可打印文本”，随后转为十六进制。`decode` 只在严格模式失败或转换结果非 UTF-8 时返回 `Err(())`（`base.rs:127-136,298-302`）。
- 未配对引号或原始/UTF-8 双游标无法同步时，扫描停止或保留该段，不 panic（`base.rs:262-297`）。最终 `String::from_utf8(...).expect(...)` 的不变量是缓冲区只由已解码 UTF-8 片段和 ASCII 十六进制组成（`base.rs:337-342`）。
- `--` 只有后接空白或输入结束才是注释；空白集合明确含空格、制表、换行、回车、垂直制表与换页。`--1` 不作为注释（`base.rs:195-210`，测试见 `base_test.rs:185-210`）。
- 普通块注释内的引号不参与转换；`/*!` 与 `/*+` 内容参与转换；`/*T!`、`/*M!` 按普通块注释跳过。Rust 与 Go 测试都固定了这些边界（`base_test.rs:185-210`、`base_test.go:105-210`）。
- 成对引号会还原为单个引号字节；反斜杠只有在 `no_backslash_escapes == false` 时调用 `UnescapeChar`。标识符紧邻被替换字面量时插入空格以保持词法有效（`base.rs:304-333`）。
- `is_printable` 排除所有 Rust `char::is_control` 字符；非法 UTF-8、截断序列、NUL 和其他控制字符的预期结果见 `base_test.rs:125-182`。

## 并发与资源生命周期

`AstNode` 没有锁、原子、线程或异步任务；`std::cell::OnceCell` 是单线程内部可变缓存，因此该类型不提供跨线程共享写入的同步语义。与 Go `sync.Once` 不同，Rust 版本依赖常规的 `&mut self` 约束完成 `SetText`/mode 更新，并用 `&self` 在首次 `Text` 时初始化缓存。若未来要求同一节点跨线程共享，需要重新评估 `OnceCell` 与 `Node` trait 的 `Send`/`Sync` 边界，不能假定当前类型可同步共享。

资源全部由所有权管理：原始字节和缓存随 `NodeText` 释放；空节点延迟分配；转换输出仅在发现首个需替换字面量时分配。缓存生命周期跨多次 `Text` 调用，直到 `SetText` 或有效的 mode 变化使其失效。不存在外部句柄、I/O、事务、通道或显式清理过程。

## 与 Go 版本的对应关系

Go 对照为 `pkg/parser/ast/base.go`，两版的核心算法和边界基本逐段对应：`node` ↔ `AstNode`/`NodeText`，`convertBinaryStringLiterals` 及四个扫描辅助函数、五种分类基类、`exprNode` 的类型/flag 均保留。Rust 的 `TexprNode = ExprNodeBase` 对应 Go `type TexprNode = exprNode`（Go `base.go:378-441`；Rust `base.rs:346-437`）。

主要语言映射差异如下：

- Go 将文本保存为 `string`，用 `*sync.Once` 和 `utf8Text` 缓存；Rust保存原始 `Vec<u8>`，用 `Option<Box<NodeText>>` 优化空节点，并用 `OnceCell<String>` 惰性缓存。
- Go `SetType` 接收 `*types.FieldType` 后复制，Rust直接取得 `FieldType` 所有权；两者 `GetType` 都返回内部类型引用。
- Go 嵌入结构体以方法集实现接口；Rust 基类只保存嵌套基类和标记方法，实际对象安全 `Node` trait 由 `lib.rs` 的具体节点实现/转发。因此不能仅凭 `StmtNodeBase` 的存在认定它自动实现 `StmtNode` trait。
- Go nil encoding 原样返回 Go string 字节；Rust `None` 分支返回拥有型 UTF-8 `String`，对合法 UTF-8 等价，但非法 UTF-8 会有损替换。这是类型系统带来的可观察差异，扩展时需保留 `OriginalText` 作为无损来源。
- Go 注释说明 `/*T!` 的真实执行还依赖 AST 包外 feature gate；Rust沿用同一保守策略，当前扫描器只把 `/*!` 与 `/*+` 当作可执行内容。

独立 Rust 测试 `pkg/parser/ast/base_test.rs` 与 Go `pkg/parser/ast/base_test.go` 共同覆盖 UTF-8/GBK、尾字节 `0x5c`、注释、转义、控制字符、标识符前缀和长查询；Rust另有 nil encoding、clone 与 trait-object 契约用例（Rust `base_test.rs:46-53,272-295`）。

## 扩展指南

- 修改节点文本语义时，首选接入 `AstNode::{SetText, Text, SetNoBackslashEscapes}`，并同步检查 `lib.rs:183-204` 的 `Node` 默认转发；任何影响转换结果的新状态都必须参与缓存失效和 `NodeText::PartialEq`。
- 新增字符集或字面量规则时，应在 `convertBinaryStringLiterals` 及双游标辅助函数中保持“UTF-8 负责安全识别边界、原始字节负责十六进制输出”的不变量。特别要补充 GBK/GB18030 ASCII 范围尾字节、成对引号、反斜杠 mode、未终止输入和注释边界用例。
- 新增 AST 分类时，应同时更新 `pkg/parser/ast/ast.rs` 的 trait、这里的基类/标记方法和 `lib.rs` 中具体节点接线；不要把 Rust 单元测试内嵌到本文件，应更新同目录独立测试 `pkg/parser/ast/base_test.rs`，并按移植目标同步核对 `base_test.go`。
- 修改 `ExprNodeBase` 的类型或 flag 时，要核对 `ast.rs:25-53` 的公开 trait/位定义以及所有访问者/表达式构造点，避免绕过 setter 造成元数据不一致。
- 性能敏感改动需保留无引号快路径、首次替换才分配缓冲区和 `Text` 惰性缓存；`base_test.rs:242-269` 提供长查询冒烟，Go `base_test.go:255-347` 还给出更完整的 benchmark 场景，可作为 Rust benchmark 扩展依据。
- 兼容风险集中在日志/restore 文本变化、字符集严格解码、注释识别和 token 粘连；这些行为改变会影响 parser 之外的 session/server 消费者，必须同时检查直接读取 `Text`/`OriginalText` 的调用点。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/parser/ast` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/parser/ast/base.rs --offset 1 --limit 500` 读取完整 437 行并报告该文件被 71 个文件使用；对 `AstNode`、`Node`、`ResultSetNode`、`DDLNode`、`DMLNode`、`convertBinaryStringLiterals`、`SetText` 做了 `query`。精确 callers/callees 查询未返回方法级边，故未把缺失图边当作不存在调用。
- 已读生产源与入口：`pkg/parser/ast/base.rs`、`pkg/parser/ast/lib.rs:1-120,150-329,4966`、`pkg/parser/ast/ast.rs:1-150`、`pkg/parser/parser_actions/misc.rs:1507-1510`、`pkg/parser/yy_parser.rs:282`、`pkg/parser/ast/sql_restore.rs` 的直接文本读取位置。
- 已读配置：`pkg/parser/ast/Cargo.toml`，确认 crate 名、`lib.rs` 入口、路径依赖和无 feature 声明。
- 已读对照与测试：完整 `pkg/parser/ast/base_test.rs`，Go `pkg/parser/ast/base.go` 和 `pkg/parser/ast/base_test.go` 的行为及边界段；另由源码搜索确认 session/server 的代表性下游调用点。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文档存在且固定二级标题恰好为 11 个，并人工复核本文覆盖“为何存在、如何运行、如何安全扩展”。

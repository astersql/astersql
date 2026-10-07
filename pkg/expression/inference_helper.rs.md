# `pkg/expression/inference_helper.rs`

## 文件定位

本文件属于 `astersql-expression` crate。crate 入口 `pkg/expression/lib.rs` 以 `pub mod inference_helper` 装载该模块，并通过 `pub use inference_helper::*` 将其四个公开能力重新导出：`EmbedTextInfo`、`IsEmbedTextFuncCall`、`ContainsEmbedTextFunc` 和 `ExtractEmbedTextInfo`。它位于 SQL 解析器 AST 与 DDL 生成列校验之间：输入是 `astersql_parser_ast` 对应的 `ast::ExprNode`，输出是对 `EMBED_TEXT()` 形态的判断、遍历结果或供 DDL 保存/比较的常量元数据。

文件不执行模型推理，也不计算向量；运行期推理参数求值在相邻的 `pkg/expression/builtin_inference.rs` 中。这里处理的是建表、改表和 DML 元数据路径需要的静态 AST 信息。RustCodeGraph 显示该文件由 `pkg/ddl/create_table.rs`、`pkg/ddl/generated_column.rs`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/dml.rs` 和独立测试 `pkg/expression/inference_helper_test.rs` 使用。

## 核心职责

1. `IsEmbedTextFuncCall` 判断一个节点本身是否为名称已规范化为小写 `embed_text` 的函数调用，但不校验参数个数或类型。
2. `ContainsEmbedTextFunc` 通过 `ast::ExprNodeVisitor` 遍历整棵表达式树，判断任意层级是否包含 `EMBED_TEXT()`；找到后跳过该节点的子树，避免无意义的继续搜索。
3. `ExtractEmbedTextInfo` 只接受直接的 `EMBED_TEXT()` 调用，验证参数数量、模型名常量、可选 JSON 参数常量及 JSON 对象形态，并返回元数据。
4. `EmbedTextInfo` 保留模型名与用户输入的原始 JSON 文本，使元数据比较能够感知 JSON 文本差异，而不是先做语义规范化。

这些职责共同支持 DDL 对 EMBED_TEXT 生成列的准入检查。`pkg/ddl/generated_column.rs::check_embed_text_generated_column` 先用 `ContainsEmbedTextFunc` 判断是否涉及该函数，再用 `IsEmbedTextFuncCall` 禁止把它嵌入其他表达式，最后调用 `ExtractEmbedTextInfo` 验证调用形态。

## 主要符号

- `pub struct EmbedTextInfo { ModelNameWithProvider: String, OptsInJSON: String }`：生成列表达式中的常量元数据。字段保持与 Go 版本相同的命名；`OptsInJSON` 保存原始字符串，未提供第三参数或第三参数为空字符串时均为空。
- `EmbedTextInfo::Equal(left: Option<&Self>, right: Option<&Self>) -> bool`：借助派生的 `PartialEq` 比较两个可空引用。两个 `None` 相等；仅一侧为空不等；两侧都有值时同时比较模型名和原始 options 文本。
- `pub fn IsEmbedTextFuncCall(expr: &ast::ExprNode) -> bool`：仅在 `expr.Kind` 是 `ast::ExprKind::Function` 且 `FnName.L == "embed_text"` 时返回真。
- `pub fn ContainsEmbedTextFunc(expr: Option<&ast::ExprNode>) -> bool`：用局部 `Finder(bool)` visitor 搜索表达式。输入为 `None` 时直接返回假；`Enter` 命中后将标志置真并返回跳过子树，`Leave` 不改变状态。
- `pub fn ExtractEmbedTextInfo(expr: &ast::ExprNode) -> Result<EmbedTextInfo, Error>`：校验并提取元数据。第二个参数是待嵌入的文本表达式，本函数只要求总参数数为 2 或 3，不负责校验第二个参数的业务类型。

本文件没有模块级常量、trait、条件编译项或持久状态。唯一的内部实现类型是 `ContainsEmbedTextFunc` 中的局部 `Finder`。

## 执行流程

典型 DDL 流程如下：

1. 解析器把生成列表达式构造成 `ast::ExprNode`。
2. `check_embed_text_generated_column` 调用 `ContainsEmbedTextFunc(Some(expr))`。若整棵树没有 `embed_text`，该专用校验立即返回。
3. 若存在该函数，DDL 先通过 `CheckEmbedTextAllowed` 检查部署模式，再调用 `IsEmbedTextFuncCall`，要求根节点本身就是 `EMBED_TEXT()`；因此 `vec_dims(embed_text(...))` 会被视为嵌套使用而拒绝。
4. DDL 还要求该生成列为 stored，随后调用 `ExtractEmbedTextInfo`。提取函数依次验证根节点类型和名称、参数数为 2 到 3、第一参数是字符串字面量、第三参数（若存在）是字符串字面量。
5. 非空 options 通过 `serde_json::from_str::<serde_json::Value>` 解析，并额外要求 `Value::is_object()`；数组、`null`、标量和非法 JSON 都失败。成功时原始文本原样写入 `OptsInJSON`。
6. `pkg/ddl/create_table.rs::check_table_info_valid_with_stmt` 还用 `IsEmbedTextFuncCall` 识别 embedding 生成列，随后禁止其他生成列依赖它。session 的 DDL/DML 路径也直接调用该判断函数来识别相关列。

`ContainsEmbedTextFunc` 与 `IsEmbedTextFuncCall` 有意分工：前者回答“树中是否存在”，后者回答“根节点是否就是该调用”；不能用其中一个替代另一个，否则嵌套表达式的错误分支会改变。

## 数据与状态

`EmbedTextInfo` 是纯拥有型值：两个 `String` 分别保存模型提供方标识和 options 原文。它派生 `Clone`、`Debug`、`Eq`、`PartialEq`，没有内部可变性或共享引用。`Equal` 的 `Option<&Self>` 形态对应 Go 指针可为 `nil` 的语义，同时避免为比较而克隆。

AST 搜索状态只存在于栈上的 `Finder(bool)` 中，并在单次 `Accept` 调用期间变化；函数返回后即释放。提取过程中的 `model`、`options` 和局部闭包 `constant` 也都是调用内临时值。options 的 JSON 解析结果仅用于确认顶层为对象，不保存解析树，因此空白、键顺序等文本差异会保留下来，并影响 `EmbedTextInfo` 相等性。

关键不变量是：成功返回的 `EmbedTextInfo` 一定来自根节点为 `embed_text`、参数数为 2 或 3、第一参数为字符串字面量的表达式；若存在第三参数，它一定是空字符串或有效 JSON 对象文本。该函数不保证第二参数的数据类型，也不保证模型名非空或提供方真实存在，这些属于其他层的职责。

## 依赖与调用关系

直接依赖如下：

- `crate::ast`：提供 `ExprNode`、`ExprKind`、`ValueDatum`、`ExprNodeVisitor` 和 `Accept` 遍历协议；crate 根部再导出了 parser AST 能力。
- `crate::Error` 与 `crate::errors::New`：构造并传播与 Go 文本对齐的表达式错误。
- `serde_json`：只在 `ExtractEmbedTextInfo` 中解析非空 options；`pkg/expression/Cargo.toml` 明确声明 `serde_json = "1"`。

RustCodeGraph 的关键调用边包括：

- `pkg/ddl/generated_column.rs::check_embed_text_generated_column` → `ContainsEmbedTextFunc`、`IsEmbedTextFuncCall`、`ExtractEmbedTextInfo`。
- `pkg/ddl/generated_column.rs::check_embedding_function_usage` → `ContainsEmbedTextFunc`，用于决定是否启用 embedding 表达式的额外非法结构检查。
- `pkg/ddl/create_table.rs::check_table_info_valid_with_stmt` → `IsEmbedTextFuncCall`，用于建立 embedding 生成列集合并禁止依赖。
- `pkg/session/runtime/ddl.rs` 与 `pkg/session/runtime/dml.rs` → `IsEmbedTextFuncCall`，用于会话执行阶段识别 EMBED_TEXT 生成列。
- `ContainsEmbedTextFunc::Finder::Enter` → `IsEmbedTextFuncCall`；`ExtractEmbedTextInfo` → `serde_json::from_str`（源码直接证据）。

这些函数经 `pkg/expression/lib.rs` 的 `pub use inference_helper::*` 暴露，调用方通常从 `astersql_expression` crate 根导入，而不是引用子模块路径。

## 错误处理与边界

`IsEmbedTextFuncCall` 和 `ContainsEmbedTextFunc` 是无错误的布尔查询；未知节点、非函数节点或空输入均返回假。名称比较使用解析器维护的小写字段 `FnName.L`，所以这里不再自行做大小写转换。

`ExtractEmbedTextInfo` 按固定顺序返回错误：

- 非函数或非 `embed_text`：`only generated columns using EMBED_TEXT() are allowed`。
- 参数少于 2 个或多于 3 个：`invalid EMBED_TEXT() usage`。
- 第一参数不是字符串字面量：`EMBED_TEXT() only accepts model name using string constant`。
- 第三参数不是字符串字面量：`EMBED_TEXT() only accepts JSON options using string constant`。
- 第三参数非空但不是有效 JSON 对象：`EMBED_TEXT expects options in JSON format`。

空 options 是合法的，并与缺少第三参数统一表示为 `String::new()`。有效 JSON 数组和 `null` 虽能被 JSON 解析器接受，但因顶层不是对象仍被拒绝。该文件不捕获 panic，也不做 I/O；所有预期失败都通过 `Result` 传播。DDL 调用方会在这些消息外包装生成列名和 `[ddl:3106]` 上下文。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、文件或网络资源。所有 API 都只读取传入 AST；visitor 为每次调用单独创建，`EmbedTextInfo` 拥有返回字符串，因此函数之间没有共享可变状态，可以由多个会话并发调用。

资源成本主要来自两处：`ContainsEmbedTextFunc` 的 AST 遍历会在 visitor 协议中克隆访问节点；命中后会跳过当前子树。`ExtractEmbedTextInfo` 会克隆第一和第三字符串，并为非空 options 构造临时 `serde_json::Value`。这些成本随表达式规模或 options 文本长度增长，但生命周期均限制在当前调用内。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/inference_helper.go`，独立测试是 `pkg/expression/inference_helper_test.go`。Rust 与 Go 的职责和错误顺序保持一致：都区分“任意层级包含”和“根节点直接调用”，都要求 2 或 3 个参数，都只提取第一和第三参数，都要求二者是字符串常量，也都把非空 options 限制为 JSON 对象。

具体映射如下：Go 的 `embedTextFnVisitor` 对应 Rust 局部 `Finder`；Go 的 `ast.Walk` 对应 Rust `ExprNode::Accept`；Go 的 `*EmbedTextInfo` 可空比较对应 Rust `EmbedTextInfo::Equal(Option<&Self>, Option<&Self>)`；Go 用 `json.Unmarshal` 到 `map[string]any` 排除数组、标量和 `null`，Rust用 `serde_json::Value::is_object` 表达相同约束。

Rust 的返回类型是拥有型 `EmbedTextInfo` 而非 Go 指针。Rust 测试额外明确证明不同空白的 JSON 原文不相等；这与 Go 注释声明“保留用户原始 JSON、不做规范化”的语义一致。两边都不在本 helper 中校验第二参数或调用模型服务。

## 扩展指南

修改行为时应保持三层职责边界：AST 搜索放在 `ContainsEmbedTextFunc`，根节点快速识别放在 `IsEmbedTextFuncCall`，参数形态和常量元数据校验放在 `ExtractEmbedTextInfo`。若增加新的常量选项，应同步修改 `EmbedTextInfo`、提取逻辑、`Equal` 语义及 Go 对照实现；如果改变嵌套规则，应同时审查 `pkg/ddl/generated_column.rs::check_embed_text_generated_column` 和 `check_embedding_function_usage`，不能只改 helper。

测试应继续放在独立文件 `pkg/expression/inference_helper_test.rs`，不要内嵌到生产源文件。至少同步覆盖：直接与嵌套识别、`None`、参数数量边界、非字符串常量、空 options、非法 JSON、合法但非对象 JSON、原始文本相等性，以及新增字段的 `None`/非 `None` 比较。涉及 DDL 错误包装或生成列依赖时，还应扩展 `pkg/ddl` 对应的独立测试。

兼容性风险主要是错误优先级、错误文本、原始 JSON 比较语义和大小写规范；性能风险主要是对大 AST 的克隆遍历及对大 options 的完整 JSON 解析。不要在这里把 JSON 规范化，否则相同语义但不同原文的元数据会从“不相等”变为“相等”，可能改变 schema 比较行为。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标文件已索引，报告由 5 个文件使用。
- RustCodeGraph 源码/符号查询：`node --file pkg/expression/inference_helper.rs`；`query EmbedTextInfo`、`query IsEmbedTextFuncCall`、`query ContainsEmbedTextFunc`、`query ExtractEmbedTextInfo`；`node check_embed_text_generated_column`、`node check_embedding_function_usage`、`node check_table_info_valid_with_stmt`、`node IsEmbedTextFuncCall`、`node ExtractEmbedTextInfo`。
- RustCodeGraph 调用查询：对 `ExtractEmbedTextInfo`、`ContainsEmbedTextFunc`、`IsEmbedTextFuncCall` 执行了 `callers`/`callees`；精确调用边由后续 `explore` 与 `node` 的 Trail 交叉确认。
- 已读 Rust 路径：`pkg/expression/inference_helper.rs`、`pkg/expression/inference_helper_test.rs`、`pkg/expression/lib.rs`，以及直接上游 `pkg/ddl/generated_column.rs`、`pkg/ddl/create_table.rs`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/dml.rs` 的相关符号/片段。
- 已读配置与 Go 对照：`pkg/expression/Cargo.toml`、`pkg/expression/inference_helper.go`、`pkg/expression/inference_helper_test.go`。
- 独立 Rust 测试验证了直接/嵌套/空输入识别、2/3 参数、原始 JSON 保留、可空比较及全部主要错误分支；Go 测试覆盖同一组核心契约。本任务为纯文档分析，按计划未运行 Cargo 或代码测试。

# `pkg/types/parser_driver/value_expr.rs`

## 文件定位

本文件属于 `astersql-types-parser_driver` crate，是 SQL 解析器 AST 与具体 `types::Datum` 实现之间的适配层。crate 入口 `pkg/types/parser_driver/lib.rs` 将本模块声明为私有模块并用 `pub use value_expr::*` 重新导出其公开符号；`pkg/types/parser_driver/Cargo.toml` 表明它直接依赖 parser AST/format、Datum、字段类型、标量字面量和 Decimal crate。

它移植自同目录的 Go `value_expr.go`。Go 版本借助包级 `init()` 把构造函数注册到 `ast`，而 Rust 没有同类初始化机制，所以本文件的 `init() -> DriverHooks` 只返回显式钩子表。仓库搜索目前只发现该函数和 `DriverHooks` 的定义，没有发现消费该钩子表的 Rust 接线；同时 `pkg/parser/ast/lib.rs::NewValueExpr` 已有独立的原生 `ExprNode::typed_value` 路径。因此，本文件应理解为可复用的 parser-driver 移植实现，而不能据此断言当前所有 Rust SQL 字面量都经过它。

## 核心职责

- `DriverHooks`、`init`、`new_decimal`、`new_hex_literal`、`new_bit_literal` 提供 Go 包初始化注册项的显式 Rust 表达；Decimal 的 `Truncated` 被当作可接受结果，Hex/Bit 的底层错误转为字符串。
- `ValueExpr` 将 `TexprNode`（AST 节点文本和 `FieldType`）、`types::Datum` 与逻辑规划使用的 `projectionOffset` 组合成简单值表达式。
- `ValueExpr::Restore` 和 `Format` 按 Datum kind 输出 SQL 字面量，并保留布尔标志、字符集前缀、字符串/反斜杠转义、二进制字面量和 Go 风格浮点科学计数法等规则。
- `ParamMarkerExpr` 在 `ValueExpr` 上附加预处理参数的源码偏移、顺序、执行期状态，以及 GROUP BY 改写语义保护标志，并以 `?` 还原。
- 两组 visitor 接口分别服务于通用 `parser_ast::Node` 的借用式/原地遍历，以及本文件为 Go 替换节点语义定义的拥有所有权的 `Visitor`。

## 主要符号

- `DriverHooks`：五个函数指针组成的公开注册表；值表达式、参数占位符、Decimal、Hex 和 Bit 构造入口均在 `init` 中绑定。
- `TexprNode`：包含 `parser_ast::base::AstNode` 和 `types::FieldType`；通过 `ValueExpr` 的 `Deref`/`DerefMut` 暴露字段类型。
- `ValueExpr`：核心结构，字段为 `TexprNode`、`Datum`、`projectionOffset`。实现 `parser_ast::Node`，并提供 `SetValue`、`Restore`、`RestoreToString`、`GetDatumString`、`Format`、投影偏移存取和拥有型 `Accept`。
- `ParamMarkerExpr`：嵌入 `ValueExpr`，另含 `Offset`、`Order`、`InExecute`、`UseAsValueInGbyByClause`。实现 `parser_ast::Node`、`parser_ast::expressions::ValueExpr` 和 `parser_ast::expressions::ParamMarkerExpr`。
- `newValueExpr`：若传入值本身就是 `ValueExpr` 则原样返回；否则先用 `types_field::DefaultTypeForValue` 推导字段类型，再以同一个类型调用 `Datum::SetValue`，最后把投影偏移设为 `-1`。
- `newParamMarkerExpr`：只写入源码 `offset`，其余字段取默认值。
- `WrapInSingleQuotes` / `UnwrapFromSingleQuotes`：按固定顺序处理反斜杠和成对单引号；后者也接受未加引号的普通字符串并原样返回。
- `format_float`：为 32/64 位浮点生成带符号且至少两位的科学计数法指数，并显式处理 `NaN`、`+Inf`、`-Inf`。

## 执行流程

构造普通值表达式时，`newValueExpr` 首先尝试把动态值向下转型为既有 `ValueExpr`，成功即保持对象身份返回。新对象路径用输入值、charset 和 collation 推导 `FieldType`，随后让 `Datum` 按该类型接收值；“类型与 Datum 使用同一排序规则”是与 Go `value_expr.go::newValueExpr` 对齐的关键调用顺序。空值以 `()` 表示并作为 `None` 参与类型推导，初始 `projectionOffset` 为 `-1`。

还原 SQL 时，`ValueExpr::Restore` 按 `Datum::Kind()` 分派：NULL 和带 `IsBooleanFlag` 的有符号整数输出关键字；数字、Decimal 输出普通文本；字符串按 restore flags 决定是否写 `_charset`，并先双写反斜杠再交给 `RestoreCtx::WriteString`；bytes 直接交给 `WriteString`；二进制值根据 `UnsignedFlag` 选择十六进制或 bit literal；Duration/Time 加单引号。`RestoreToString` 只负责创建内存缓冲和 context，再委托 `Restore`。

`Format` 是更窄的紧凑输出路径：支持 NULL、整数、浮点、字符串/bytes、Decimal 和 binary literal；字符串统一走 `WrapInSingleQuotes`，不处理 charset restore flags。`ParamMarkerExpr::Restore` 固定写 `?`，而其 `Format` 尚未实现。

遍历时，`parser_ast::Node::{accept,accept_in_place}` 总是按 `enter` 后 `leave` 的顺序执行；叶节点没有子节点，`enter` 的 skip 值只决定是否立即进入 `leave`，不改变事件数。拥有型 `Accept` 允许 `Enter` 返回替换节点，但非跳过路径要求替换值仍可向下转型为原具体类型，然后调用 `Leave`。

## 数据与状态

`ValueExpr` 的主要可变状态是 `Datum`、字段类型和投影偏移。`SetValue` 使用默认 collation 写 Datum；构造函数则显式先推导 `FieldType`，再带类型写 Datum。规划器可通过 `SetProjectionOffset`/`GetProjectionOffset` 更新和读取偏移，`-1` 表示刚构造时尚未绑定投影位置。

`ParamMarkerExpr::Offset` 是解析源码中的位置，`Order` 是参数序号，`InExecute` 表示执行阶段状态。`UseAsValueInGbyByClause` 对应 Go 注释中的兼容标志：当 `GROUP BY` 的别名被优化器改写成 `?` 时，防止把该 marker 错当成 select-list 的序号。这些都是普通拥有值，没有内部共享引用、全局注册状态或惰性缓存。

`TexprNode`、`ValueExpr` 和 `ParamMarkerExpr` 均可克隆；trait object 形式的参数 marker 通过 `clone_box` 保留该能力。`Box<dyn Any>` 用于跨 parser-driver 边界承载不同值和 visitor 替换节点，运行时向下转型是其类型恢复机制。

## 依赖与调用关系

向下依赖方面，`parser_ast` 提供节点、visitor 和表达式 trait；`parser_format`（由 crate 入口重导出为 `format`）提供 restore context/flags；`types` 提供 `Datum`、kind 常量、MySQL flag 和时间类值；`types_field` 推导字段类型；`types_decimal`、`types_scalar` 分别解析 Decimal 与 Hex/Bit；`hex` 负责 bytes 的十六进制编码。

向上关系方面，RustCodeGraph 将本文件标为被 10 个文件使用，显示的代表包括 `dumpling/export/schema_projection.rs`、`dumpling/export/schema_projection_restore.rs`、`pkg/parser/test_driver/test_driver.rs`、`pkg/planner/core/plan_cache_utils_test.rs` 和 `pkg/planner/core/planbuilder_runtime.rs`。crate 入口 `pkg/types/parser_driver/lib.rs` 是公开导出边界，独立测试由该入口通过 `#[path = "value_expr_test.rs"]` 挂载。

需要区分两条构造链：本文件公开 `newValueExpr` 并在 `DriverHooks` 中返回其函数指针；仓库中大量运行时代码（例如 `pkg/session/runtime/load_data.rs`、`query.rs`）调用的是 `pkg/parser/ast/lib.rs::NewValueExpr`，后者直接构造 AST 自身的 typed value。当前静态搜索没有发现 `parser_driver::init()` 或 `DriverHooks` 的消费者，因此两条链不能视为已经接通。

## 错误处理与边界

`new_decimal` 仅豁免 `DecimalError::Truncated`，其他解析错误字符串化返回；Hex/Bit 构造错误同样字符串化，因而跨钩子边界不保留具体错误类型。

`Restore` 对 Go 已支持的 Datum kind 返回 `Ok(())`；Enum、Bit、Set、Interface、Min/Max、Raw、JSON、VectorFloat32 明确返回 `io::Error("Not implemented")`，未知 kind 返回 `io::Error("can't format to string")`。不过，为对齐 Go `RestoreCtx` 无错误返回的契约，各分支会忽略 context 写入错误；所以其 `io::Result` 主要表示“不支持的 kind”，并不可靠地传播底层 writer 失败。

`Format` 的支持面小于 `Restore`：遇到未列出的 kind 会 panic，writer 的 `write_all` 错误被忽略。`ParamMarkerExpr::Format` 无条件 panic。拥有型 visitor 若在非 skip 路径用错误具体类型替换节点，也会 panic。`RestoreToString` 认为本模块写入的 SQL 必为 UTF-8，并对转换失败使用 `expect`；当前输出都由 UTF-8 字符串字节产生。

转义辅助函数是顺序敏感的简单替换，不是完整 SQL lexer。`UnwrapFromSingleQuotes` 只对首尾单引号成对的输入解包，然后先还原双反斜杠、再还原双单引号；调用方不应把它当作任意 SQL 字面量验证器。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。表达式、context、writer 和 visitor 均由调用方拥有并以借用或 `Box` 传入；临时 restore 缓冲在 `RestoreToString` 返回后释放，动态 visitor 节点则沿 `Enter`/`Leave` 的所有权链返回。

原地遍历不会为访问节点而装箱。`pkg/types/parser_driver/value_expr_test.rs::go_merge_10_value_and_param_marker_walk_without_allocations` 使用线程局部分配计数器，预热后对 `ValueExpr` 与 `ParamMarkerExpr` 各循环遍历 100 次，并断言新增分配为零。这是性能性质，不代表结构体自动具备跨线程共享保证；代码本身没有声明额外同步语义。

## 与 Go 版本的对应关系

结构和核心分支直接对应 `pkg/types/parser_driver/value_expr.go`：Rust 的 `ValueExpr`/`ParamMarkerExpr` 字段、`Restore`/`Format`、引号辅助函数、投影偏移、visitor 顺序及构造调用顺序均保留 Go 意图。Rust 用 `Deref` 模拟 Go 嵌入字段，用 `Box<dyn Any>` 模拟 `any` 和可替换 AST node，用显式 trait impl 表达 Go 接口满足关系。

关键差异有三类。第一，Go `init` 直接写入 `ast.NewValueExpr` 等包变量；Rust `init` 只返回未发现消费者的 `DriverHooks`。第二，Go writer/context API 多数不返回错误，Rust API 虽使用 `io::Result`，却为兼容而主动忽略具体写错误。第三，Go `strconv.FormatFloat(..., 'e', -1, bits)` 由 Rust 私有 `format_float` 复刻，包括特殊值和指数格式；该实现应继续通过与 Go 输出表对照来维护。

`pkg/types/parser_driver/value_expr_test.rs` 的 Restore/Format 13 项表与 `value_expr_test.go` 对应，覆盖 NULL、正负整数、无符号数、32/64 位浮点、字符串/bytes 转义、bit literal、Decimal、Duration、Time 和反斜杠；Format 额外覆盖连续单引号及制表/换行组合。Rust 测试还对齐 Go 的 `ast.Walk` enter/leave 顺序和零分配性质。

## 扩展指南

新增 Datum kind 的文本支持时，应分别审查 `ValueExpr::Restore` 与 `Format`：两者目的和支持面不同，不能只补其中一个就宣称完整支持；同时在独立的 `pkg/types/parser_driver/value_expr_test.rs` 增加与 Go `value_expr_test.go` 一致的边界用例。涉及浮点格式时修改 `format_float`，必须核对 32/64 位舍入、零、极值、NaN/Inf 和指数宽度，避免直接采用 Rust 默认显示格式。

改变构造语义时优先修改 `newValueExpr`，并保持“先 `DefaultTypeForValue`、再用同一 Type 写 Datum”的 collation 不变量；参数解析元数据则在 `newParamMarkerExpr`、`SetOrder` 和两个 parser AST trait impl 处接入。若要让 parser-driver 真正承担全局构造入口，需要在明确的集成层消费 `DriverHooks`，并先厘清与 `parser_ast::NewValueExpr` 原生路径的归一策略，不能仅调用当前 `init` 名称便假定注册发生。

新增 visitor 行为时要同步检查借用式 `parser_ast::Node` 实现和拥有型 `Visitor`/`Accept`，保留 enter、skip、类型替换、leave 的顺序，并继续把测试放在独立测试文件，不能内嵌到生产源文件。改变 `ParamMarkerExpr` 字段时还需核对 `parser_ast::expressions::{ValueExpr, ParamMarkerExpr}` trait 契约以及 GROUP BY marker 的兼容语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/types/parser_driver` 确认目标 Rust/Go 源与测试均已索引。
- RustCodeGraph `node --file pkg/types/parser_driver/value_expr.rs`：读取完整 530 行源文件，并得到“被 10 个文件使用”的文件级反向关系；`query` 确认 `newValueExpr`、`ValueExpr`、`newParamMarkerExpr` 的 Rust/Go 同名符号位置。
- RustCodeGraph `node` 已读取 `pkg/types/parser_driver/value_expr.go`、`value_expr_test.rs`、`value_expr_test.go`、`lib.rs`、`pkg/parser/ast/expressions.rs` 和 `pkg/parser/ast/lib.rs` 的相关定义；精确 `callers/callees` 查询在本次会话时限内未返回，因此没有把缺失的函数级调用边写成已验证事实。
- 直接读取未索引配置 `pkg/types/parser_driver/Cargo.toml`，确认 crate 名称、库入口、直接依赖、dev dependency 和 Go package 迁移元数据。
- 使用 `rg` 核对 `DriverHooks`/`parser_driver::init` 的消费情况及 `parser_ast::NewValueExpr` 的运行时调用点；结论限定为当前仓库静态搜索结果。
- 人工对照 Rust/Go 源与独立测试，确认文档覆盖文件存在理由、构造/输出/遍历流程、未实现和 panic 边界、资源生命周期、迁移接线现状及安全扩展位置；本任务为纯文档分析，按计划未运行 Cargo。

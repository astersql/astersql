# `pkg/planner/core/plan_cache_param.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate 的计划缓存参数化组件。`pkg/planner/core/lib.rs` 通过 `mod plan_cache_param` 编译该模块，并以 `pub use plan_cache_param::*` 将六个公开函数重导出到 crate 根：`GetParamSQLFromAST`、`ParameterizeAST`、`TryParameterizeAST`、`RestoreASTWithParams`、`Params2Expressions` 和 `ParseParameterizedSQL`。因此测试和潜在调用者使用 `astersql_planner_core::...`，而不直接引用私有模块路径。

它位于“把字面量 SQL 归一化为可复用计划缓存键”的边界：输入是单条 SQL 文本，输出是规范化 SQL 和按出现顺序提取的 `Datum`。当前实现不解析或修改真正的 parser AST；名称中的 `AST` 是与 Go API 对齐的兼容命名。全仓 Rust 引用搜索只发现本文件内部包装调用及 `pkg/planner/core/casetest/plancache/plan_cache_param_test.rs`，未发现生产规划主链调用，因此应将其描述为已经公开、已有独立测试但尚无可验证生产调用边的移植实现，而不是已接入 plan-cache 执行链的组件。

## 核心职责

- `TryParameterizeAST` 先以本文件的轻量词法器 `lex` 切分 SQL，再由 `validate_single_statement` 拒绝括号不配对或多语句输入，最后按首个关键字分派。
- SELECT 路径保留投影、`GROUP BY`、`ORDER BY` 和 `LIMIT` 中的字面量，只参数化 `WHERE` 等允许区域；这是为了保留输出列名、分组检查语义以及依赖 LIMIT 值的计划选择。
- INSERT 路径规范化表名和列名，并参数化 `VALUES` 中的字面量。
- 通用表达式路径将字符串、数值和 `NULL`/`TRUE`/`FALSE` 依次替换成 `?`，同时产生同序 `Vec<Datum>`；已有 `?` 只原样保留，不会虚构对应参数。
- `RestoreASTWithParams` 进行逆向文本替换，跳过引号包围的 `?`，并将 `Datum` 序列化成 SQL 字面量。
- `Params2Expressions` 与 `ParseParameterizedSQL` 目前是兼容门面：前者仅克隆 `Datum`，后者只做词法与单语句校验后返回原文本，并未分别构造表达式对象或 parser AST。

## 主要符号

- `TokenKind`：私有词法 token 枚举，区分普通词、反引号标识符、字符串、数字、占位符、单字符符号和运算符。
- `Token { kind, start, end }`：记录 token 类型及其在原 SQL 中的字节区间；`Token::raw` 用于生成带原片段的错误信息。词法器按 UTF-8 字符推进普通符号，但标识符、数字和转义扫描主要以 ASCII 字节规则工作。
- `GetParamSQLFromAST(&str) -> (String, Vec<Datum>)`：不失败的公开便利入口，直接委托 `ParameterizeAST`。与 Go 版本“参数化后恢复原 AST”不同，Rust 输入是不可变字符串，本来就不会被修改。
- `ParameterizeAST(&str) -> (String, Vec<Datum>)`：历史 tuple API；调用可失败版本，并在错误时以 `parameterize SQL: ...` panic。
- `TryParameterizeAST(&str) -> Result<(String, Vec<Datum>), String>`：真正的可恢复错误入口，也是参数化总分派点。
- `parameterize_select`、`parameterize_insert`：SELECT/INSERT 的私有结构化重写器。前者用顶层关键字位置划分子句；后者要求 `INTO` 与 `VALUES`，并可校验列名列表。
- `format_expression`：递归表达式格式器和参数收集器。`parameterize` 标志决定当前区域是否提取字面量；对 `date_format`、`str_to_date`、`time_format`、`from_unixtime` 只参数化第一个参数，保留格式参数。
- `find_top_level_word`、`find_top_level_pair`、`matching_paren`、`split_top_level`：以括号深度为依据的局部结构辅助函数，并非完整 SQL 语法分析器。
- `number_to_datum`：含 `.`, `e` 或 `E` 的文本按 `f64`；其余先尝试 `i64`，再尝试 `u64`。
- `RestoreASTWithParams(&str, &[Datum]) -> Result<String, String>` 与 `datum_sql_literal`：逐字符替换引号外占位符，并覆盖 `Null`、有/无符号整数、浮点、字节、字符串、JSON、布尔值的 SQL 表示。
- `Params2Expressions(&[Datum]) -> Vec<Datum>`：当前仅 `to_vec`，返回值仍是 `Datum` 而不是独立表达式类型。
- `ParseParameterizedSQL(&str) -> Result<String, String>`：当前只验证非空单语句，成功时原样返回 SQL 文本。
- `lex`、`scan_quoted`、`validate_single_statement`：词法、引号转义和单语句/括号边界校验的底层实现。

## 执行流程

1. 调用者通常经 `GetParamSQLFromAST` 或 `ParameterizeAST` 进入；需要处理非法 SQL 时应直接调用 `TryParameterizeAST`，避免包装层 panic。
2. `lex` 忽略空白并生成带源码区间的 token。反引号生成 `QuotedIdentifier`，单双引号都生成 `StringLiteral`；重复引号和反斜杠转义由 `scan_quoted` 解码。
3. `validate_single_statement` 跟踪括号深度。只允许一个位于末尾的顶层分号；额外 token、第二个分号、未闭合括号或无匹配右括号都返回错误。
4. `TryParameterizeAST` 拒绝空 token 流。首词为 SELECT 时进入 `parameterize_select`，为 INSERT 时进入 `parameterize_insert`，其他语句整体交给 `format_expression(..., true)`；所以通用分支只能保证表达式式词法重写，不等价于完整 DML/DDL 格式化。
5. SELECT 先用原 SQL 字节切片恢复投影字段，避免改写其中常量与原始字段文本；表区由 `format_table_tokens` 规范化。随后按顶层 `WHERE`、`GROUP BY`、`ORDER BY`、`LIMIT` 顺序消费子句，仅 WHERE 以 `parameterize=true` 格式化，其余三个子句保留字面量。
6. INSERT 定位 `INTO`、`VALUES` 和可选列括号，规范化表/列标识符，再以 `parameterize=true` 格式化全部 VALUES token。参数按深度优先、从左到右顺序压入向量。
7. 还原时 `RestoreASTWithParams` 扫描 SQL 字符；遇到单引号、双引号或反引号会完整复制被引用片段，只有引号外的 `?` 才消费一个参数。扫描结束后还会检查是否存在未消费参数。

## 数据与状态

本文件没有全局变量、缓存、池、锁或静态可变状态。每次参数化都新建 token 向量、输出 `String` 和参数 `Vec<Datum>`；输入 `&str` 与参数切片均不被修改。`Token.start/end` 是原 SQL 的 UTF-8 字节索引，投影恢复依赖这些索引保持字符边界。

参数值使用 `pkg/planner/core/expression_codec_fn.rs` 定义并由 crate 根重导出的 `Datum`：`Null`、`Int`、`UInt`、`Float`、`Bytes`、`String`、`Bool`、`Json`。参数顺序是不变量：每写出一个由字面量生成的 `?`，就按遍历顺序追加一个值；但输入中已经存在的 `?` 不追加值，所以混合“已有占位符 + 新提取字面量”的 SQL 需要调用者自行区分来源。

格式化会规范化部分文本：关键字通常转大写，标识符加反引号，若干空白被压缩；SELECT 投影则刻意保留原切片并仅以逗号拼接顶层字段。输出目标是缓存参数化文本而非原 SQL 的逐字可逆副本。

## 依赖与调用关系

直接 Rust 数据依赖只有 `crate::Datum`，没有引入外部 crate API；`Datum` 的真实定义位于 `pkg/planner/core/expression_codec_fn.rs`。crate 边界由 `pkg/planner/core/Cargo.toml` 定义，包名为 `astersql-planner-core`，本模块不受 `nextgen` feature 条件控制。

模块装配边是 `pkg/planner/core/lib.rs -> mod plan_cache_param -> pub use plan_cache_param::*`。公开入口内部调用链为：

- `GetParamSQLFromAST -> ParameterizeAST -> TryParameterizeAST`；
- `TryParameterizeAST -> lex -> validate_single_statement -> parameterize_select / parameterize_insert / format_expression`；
- 格式器继续调用括号匹配、顶层切分、关键字分类、标识符/字符串引用和 `number_to_datum`；
- `RestoreASTWithParams -> datum_sql_literal -> quote_string`。

RustCodeGraph 对目标文件报告 34 个符号且“used by 0 files”；其精确 `callers` 查询也没有返回跨文件边。`rg` 补充确认公开 API 仅由独立测试 crate `pkg/planner/core/casetest/plancache/plan_cache_param_test.rs` 使用，尚未发现生产调用者。测试 crate 的 `Cargo.toml` 通过 dev-dependency `astersql-planner-core = { path = "../.." }` 访问这些重导出 API。

## 错误处理与边界

可预期输入错误以 `Result<_, String>` 从 `TryParameterizeAST`、`RestoreASTWithParams` 和 `ParseParameterizedSQL` 返回，包括空 SQL、未终止引号/转义、括号不配对、多语句、SELECT 未支持子句、INSERT 缺少 `INTO`/`VALUES`、非法列清单、非法数字及还原参数数量不匹配。`ParameterizeAST` 和 `GetParamSQLFromAST` 会把同类参数化错误升级为 panic；不可信 SQL 应使用 fallible 入口。

解析边界必须明确：该实现不是完整 TiDB parser。它只识别有限 token、顶层括号和少量 SELECT/INSERT 子句；例如 SELECT 消费循环只接受 WHERE、GROUP BY、ORDER BY、LIMIT 和末尾分号，其他顶层子句会报 `unsupported SELECT clause`。注释、字符集 introducer、复杂 MySQL 数字/字符串语法等未在现有代码和测试中得到完整语法保证。

`scan_quoted` 支持成对重复引号及反斜杠转义；`RestoreASTWithParams` 同样跳过三种引号内的问号。还原严格要求一一对应：占位符更多时返回 `not enough parameters`，参数更多时返回 `too many parameters`。`datum_sql_literal` 将字节编码为十六进制字面量，将 JSON 与普通字符串同样单引号转义；它不携带字符集、collation 或类型元数据。

## 并发与资源生命周期

所有状态均局限于函数栈和返回值，调用间不共享 `Vec`、`String`、词法器或游标，因此并发隔离来自所有权模型，而不是锁。`GetParamSQLFromAST` 接收不可变 `&str`，提取的 `String`/`Vec<Datum>` 由调用者拥有；`Params2Expressions` 也克隆数据，避免返回借用。

独立测试 `get_param_sql_from_ast_matches_go_concurrency_contract` 创建 50 个线程，每线程对自己的 INSERT SQL 参数化 100 次，并验证三项参数的值和顺序不交叉污染。该测试对齐 Go 的并发用例，但实现机制不同：Go 用 `sync.Pool` 复用 visitor、restore context 和 marker；Rust 当前每次分配临时容器，没有池化资源、后台任务、通道或显式清理阶段。性能扩展时可评估分配成本，但引入复用不得破坏当前无共享可变状态的不变量。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/plan_cache_param.go`，回归用例是 Go 的 `pkg/planner/core/casetest/plancache/plan_cache_param_test.go` 与 Rust 的同名 `.rs` 测试。

共同语义包括：SELECT 投影、GROUP BY、ORDER BY、LIMIT 不参数化；格式日期/时间函数只改写第一个参数；普通 WHERE 和 INSERT VALUES 字面量按顺序提取；标识符规范化；并发调用不能污染参数；公开函数名称保持对应。Rust 表驱动测试覆盖了当前 Go `TestParameterize` 的 13 个场景，另有并发对齐测试及参数顺序/还原/解析往返测试。

实现和类型并非一一等价：

- Go 接收 `ast.StmtNode`，`ParameterizeAST` 通过 visitor 把 `ValueExpr` 原地换成带 offset 的 `ParamMarkerExpr`，随后 `RestoreASTWithParams` 恢复原 AST；Rust 接收 SQL 文本并生成新文本，从不修改 AST，因为它没有 AST 参数。
- Go `GetParamSQLFromAST` 会复制 Datum 并恢复输入 AST，且返回 error；Rust 输入不可变，直接返回 owned Datum，但不失败包装层通过 panic 表达错误。
- Go 用 parser 的 restore flags 和 session SQL mode/parser config；Rust 使用有限词法器，`ParseParameterizedSQL` 不接收 session context，也不产生 AST。
- Go `Params2Expressions` 推断 `FieldType` 并构造 `expression.Constant`；Rust 当前只返回克隆后的 `Vec<Datum>`，没有表达式类型和返回类型推断。
- Go 通过多个 `sync.Pool` 降低分配；Rust 当前无池，换取简单的调用隔离。

因此，本文件已经对齐所列基础测试意图，但不能据此宣称完整替代 Go AST、类型推断或 session-aware parser 语义。

## 扩展指南

- 增加 SELECT 子句支持时，需同步修改 `clause_start`、`next_select_clause` 和 `parameterize_select` 的消费分支，明确新子句内字面量是否影响输出名、语义检查或计划选择；否则只添加关键字查找会导致边界截断但无法输出子句。
- 增加 SQL 词法能力时优先扩展 `lex`/`scan_quoted`，并为注释、Unicode 标识符、科学计数法、转义和 MySQL 特有字面量分别增加边界测试；不要把轻量词法验证描述为完整语法验证。
- 增加需保留格式参数的函数时修改 `is_format_function`，同时覆盖零参数、单参数、多参数和嵌套调用；当前递归规则只参数化这些函数的第一个实参。
- 增加 `Datum` 变体时必须同步 `datum_sql_literal`，并检查 `number_to_datum`、参数提取和还原往返。若目标是对齐 Go `Params2Expressions`，应接入真实 expression/FieldType 类型，而不是继续扩充 `Vec<Datum>` 门面。
- 若要接入生产 plan-cache 主链，应从真实调用点显式选择 fallible API，并补充集成测试证明生成文本参与缓存键、参数仍供执行期绑定；目前没有生产调用边，不能仅依赖公开重导出判断已接线。
- 测试逻辑应继续放在独立文件 `pkg/planner/core/casetest/plancache/plan_cache_param_test.rs`，不要内嵌进生产源文件。优先扩展现有三个 Rust 测试，并与 Go 同路径测试保持案例、错误语义和并发意图一致。
- 性能优化若引入缓冲池或复用容器，必须保持跨线程隔离、错误路径清理和返回值所有权；兼容性风险主要集中在 SQL 规范化文本变化，因为它可能改变计划缓存键。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/planner/core/plan_cache_param.rs` 已索引，共报告 34 个符号。
- RustCodeGraph 源码/符号查询：读取目标文件 1–738 行；精确查询确认 `TryParameterizeAST`、`GetParamSQLFromAST`、`ParameterizeAST`、`RestoreASTWithParams` 的定义位置；对六个公开 API 的 callers 查询未返回跨文件边，目标文件摘要为 `used by 0 files`。
- RustCodeGraph 关联源码：读取 `pkg/planner/core/lib.rs` 的模块声明与重导出（167、230 行）、`pkg/planner/core/expression_codec_fn.rs` 的 `Datum` 定义（24–35 行）以及 Rust 独立测试 `pkg/planner/core/casetest/plancache/plan_cache_param_test.rs`（1–177 行）。
- Cargo/模块证据：读取 `pkg/planner/core/Cargo.toml`、`pkg/planner/core/casetest/plancache/Cargo.toml` 和测试 crate 的 `lib.rs`；确认 crate 名称、无目标模块专属 feature、测试 dev-dependency 及 `#[cfg(test)]` 独立测试挂载。
- Go 对照证据：读取 `pkg/planner/core/plan_cache_param.go` 和 `pkg/planner/core/casetest/plancache/plan_cache_param_test.go`，核对 visitor/pool/AST 恢复、类型推断、session parser 以及表驱动和并发测试意图。
- 全仓 `rg` 证据：Rust 生产源码中六个公开 API 只在本文件出现；跨文件 Rust 命中位于独立 plan-cache 测试。该结果用于补足调用图未覆盖的模块/测试装配证据。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前以任务指定命令检查文档存在且恰含 11 个固定二级标题，并人工复核所有“已支持”陈述均限定于当前源码和测试证据。

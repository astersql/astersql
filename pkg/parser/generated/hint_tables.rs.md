# `pkg/parser/generated/hint_tables.rs`

## 文件定位

用 `GENERATED_HINT_XLAT` 将 lexer 输出的稀疏 token 编号翻译为解析表的紧凑列下标，并用 `GENERATED_HINT_SYMBOL_NAMES` 提供终结符和非终结符的可读名称。

## 核心职责

- 保存 hint lexer token 的整数编号，包括 `(`、`)`、`,` 等字面量以及 `hintHashJoin`、`hintMemoryQuota`、`hintLeading` 等 optimizer hint token。
- 用 `GENERATED_HINT_XLAT` 将 lexer 输出的稀疏 token 编号翻译为解析表的紧凑列下标，并用 `GENERATED_HINT_SYMBOL_NAMES` 提供终结符和非终结符的可读名称。
- 用 `GENERATED_HINT_REDUCTIONS`、`GENERATED_HINT_LEGACY_RULES` 和 `GENERATED_HINT_ACTION_REQUIRED` 保留每条产生式的左部符号、右部长度、手写语义动作规则号与动作必需性元数据。
- 用 `GENERATED_HINT_PARSE_TABLE` 保存 338 个状态的稀疏 action/goto 行，由运行时完成 shift、reduce、accept 或 error 判定。

该文件不负责词法扫描、语义值构造、诊断累积或 AST 创建；这些分别由 `hintparserimpl.rs`、`hintparser.rs::reduce_hint` 及其 lexer 协议完成。

## 主要符号

- `mod tokens`：私有 token 常量集；`hintparser.rs` 在同一 `include!` 作用域中通过 `pub use tokens::*` 将它们提供给 lexer/parser 实现。数值与 `grammar/hint.astergram` 的 `%token` 声明对齐。
- `GENERATED_HINT_ACCEPT: i32 = i32::MIN`：独立的接受动作哨兵值；`yyhintParse` 在普通正数 shift 之前先检查它。
- `GENERATED_HINT_XLAT`：102 个 `(token_number, symbol_column)` 项，包含 EOF 的 `(0, 0)`；按 token 编号排序，供 `yyhint_tables::xlat` 二分查找。
- `GENERATED_HINT_SYMBOL_NAMES`：139 个符号名，列下标同时被 action/goto 表和 reduction 左部引用。
- `GENERATED_HINT_REDUCTIONS`：231 个 `(lhs_symbol, rhs_component_count)` 项，描述规约后应弹出的栈元素数以及 goto 所用非终结符。
- `GENERATED_HINT_LEGACY_RULES`：231 个以 1 为基的文法规则号，把 parsergen 内部规约顺序映射到 `hintparser.rs::reduce_hint` 的 `match rule`。
- `GENERATED_HINT_ACTION_REQUIRED`：231 个布尔标记，其中 65 个为 `true`。当前 hint 运行时不直接读取它，但 `parser_manifest_hint_matches_rust_reductions` 用它校验生成表未与文法动作标记漂移。
- `GENERATED_HINT_PARSE_TABLE`：338 行稀疏 `(symbol_column, encoded_action)` 数组；每行按列号排序，供 `yyhint_tables::action` 二分查找。

除 `tokens` 中的 `pub const` 外，所有表都是包含作用域内的私有符号；本文件没有类型、trait、函数、`impl` 或条件编译项。

## 执行流程

1. `ParseHint` 通过 `hintparserimpl.rs` 创建 hint lexer/parser，然后进入 `hintparser.rs::yyhintParse`。
2. lexer 返回 token 编号；`yyhint_tables::xlat` 在 `GENERATED_HINT_XLAT` 中二分查找解析表列。未知 token 被映射到符号数量之外，后续查表得到错误。
3. `yyhint_tables::action(state, symbol)` 在当前稀疏行中二分查找：缺失项返回 0，`GENERATED_HINT_ACCEPT` 表示成功，正数编码为“目标状态 + 1”，负数编码为“`-(generated_reduction + 1)`”。
4. shift 时，`yyhintParse` 压入语义值并进入 `action - 1` 状态；reduce 时，它从 `GENERATED_HINT_REDUCTIONS` 取左部和右部长度，从 `GENERATED_HINT_LEGACY_RULES` 取语义规则号，再调用 `reduce_hint`。
5. 规约后按当前基状态和左部符号再查同一张表得到 goto；非正 goto 被视为内部解析器错误。
6. 到达 accept 时返回成功；表项缺失时走 lexer 诊断路径并返回失败。AST 内容由规约语义动作生成，表本身不持有 AST。

## 数据与状态

全部数据是编译期签入的不可变 `const`/`static` 数组。运行时状态（状态栈、语义值栈、lookahead、诊断与解析结果）全部位于 `yyhintParse`、`hintParser` 和 lexer 实例中，不会回写本文件的表。

数据间存在必须同时成立的索引不变式：XLAT 的列号必须与符号名及 parse-table 列一致；三个 reduction 元数据数组必须等长；reduction 左部必须是合法符号列；每行稀疏表必须按列递增，否则二分查找会静默失配。`parser_manifest_aster_unit_test.rs` 和 `parsergen_baseline_aster_unit_test.rs` 分别从文法元数据与解析轨迹两个层面保护这些关系。

## 依赖与调用关系

- 上游生成链：`pkg/parser/grammar/hint.astergram` → `parsergen::Grammar::parse` → `GeneratedParser::build` → `parsergen/generate.rs::render_compatibility_tables` → `generated/hint_tables.rs`。
- 上游运行时调用链：`parser::ParseHint` → `hintparserimpl.rs` 的 parser/lexer 入口 → `hintparser.rs::yyhintParse` → `yyhint_tables::{xlat, action, reduction}` →本文件的静态表。RustCodeGraph 对生成表只建立了文件节点，因为 `include!` 与静态数据访问未被识别为跨文件调用边；上述连接由 `lib.rs` 和 `hintparser.rs` 的直接引用验证。
- 下游语义依赖：规约编码最终选择 `hintparser.rs::reduce_hint` 分支，该函数构造 `parser_ast::TableOptimizerHint`、表名、索引名、`LEADING` 列表等语义值。
- crate 边界：`pkg/parser/Cargo.toml` 定义 `astersql-parser`，运行时 AST 来自路径依赖 `astersql-parser-ast`。`astersql-parsergen` 仅作为 `dev-dependencies` 和显式生成工具，运行时消费签入文件，不从 `OUT_DIR` 动态生成。

## 错误处理与边界

本文件没有可失败函数，也不分配错误。边界行为由表的消费者定义：未知 token、当前状态不接受的 token 或缺失的 action 都归一为 0，使 `yyhintParse` 累积 lexer 格式的语法错误；不合法的 reduction goto 生成 `invalid parser goto`。空 hint、括号不完整、未知 token 等失败轨迹在 `parsergen_hint_tables_match_rust_baseline` 中被明确断言为 `TraceStep::Error`。

`GENERATED_HINT_PARSE_TABLE` 不包含定制错误文案；`hintparser.rs::yyhint_tables::error_message` 当前恒返回 `None`，与 Go `hintparser.go` 中空 `yyhintXErrors` 映射对应。具体的用户诊断由 lexer 和语义动作提供，并由 `hintparser_test.rs` 校验。

## 并发与资源生命周期

表是 `'static` 不可变数据，没有锁、原子量、任务、通道、文件句柄或事务资源；多个 parser 实例可以共享读取。每次解析的可变栈、lexer、错误集合和 AST 均由调用实例拥有，其生命周期不延伸到静态表。因此本文件本身没有并发争用或清理顺序；性能主要取决于表大小、状态转移数量和每次稀疏行的二分查找。

## 与 Go 版本的对应关系

Rust 表不是对 Go 数组的逐字节拷贝，而是从可维护的 `grammar/hint.astergram` 生成的语义等价表：

- token 名称与外部编号保留 Go 协议，但 Rust 使用 EOF `0`，并在 XLAT 中显式转为列 0；Go 生成物使用 `yyhintEOFCode = 57344`。
- Go `yyhintReductions` 的左部/右部长度对应 Rust `GENERATED_HINT_REDUCTIONS`；Go `switch r` 的语义动作规则号由 Rust `GENERATED_HINT_LEGACY_RULES` 连接到 `reduce_hint`。
- Go 的 `yyhintXErrors` 为空，Rust 也没有生成扩展错误表。

## 扩展指南

1. 新增或修改 hint 语法时，先改 `pkg/parser/grammar/hint.astergram`，并同步 lexer token/关键字映射与 `hintparser.rs::reduce_hint` 的语义动作；不要手工编辑本文件。
2. 用 `astersql-parsergen generate` 重新生成签入产物，并用它的 `check` 模式检查漂移。评审时应将 grammar、生成表、语义动作和测试视为一个原子变更。
3. 在独立测试文件中扩展覆盖：语法/表不变式改动对应 `parser_manifest_aster_unit_test.rs` 和 `parsergen_baseline_aster_unit_test.rs`；生成链改动对应 `parser_generated_sources_aster_unit_test.rs` 及 `parsergen/generate_aster_unit_test.rs`；用户可见语义对应 `hintparser_test.rs`。不要把 Rust 测试嵌入生成源文件。
4. 兼容性风险主要是 token 编号漂移、legacy rule 映射错位、优先级/冲突解决变化和 Rust/Go AST 载荷分歧；性能风险主要是状态数或稀疏行显著膨胀。扩展时应同时保留成功、不完整输入、未知 token 和空输入轨迹。

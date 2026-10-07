# `pkg/parser/parsergen/generate.rs`

源码：[`generate.rs`](generate.rs)

## 文件定位

本文件是 `astersql-parsergen` crate 的产物编排层：它从 `pkg/parser/grammar/main.astergram` 与 `hint.astergram` 读取维护中的 Rust 文法，经 `Grammar::parse` 和 `GeneratedParser::build` 得到解析器数据，再渲染、写入或校验 `pkg/parser/generated/` 下的三份已提交 Rust 源码。crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：库入口是 [`lib.rs`](lib.rs)，命令入口是 [`main.rs`](main.rs)，且本 crate 没有外部 Cargo 依赖。

生成器不在数据库启动或普通 parser 编译时运行。`pkg/parser/lib.rs` 直接 `include!` 已提交的 `generated/main_tables.rs`、`generated/hint_tables.rs` 和 `generated/lexer_tokens.rs`；`pkg/parser/generate.rs::GENERATED_RUST_TABLES` 也将这三项声明为兼容生成入口。CLI 的 `generate` 子命令调用 `write_generated_outputs`，`check` 子命令调用 `check_generated_outputs`。

## 核心职责

- `generate_outputs` 统一生成主 SQL 表、Hint 表和共享 lexer token 常量，并用固定顺序返回三项 `GeneratedOutput`。
- `generate_parser_output` 把“读取文法—解析文法—构建 LALR 数据—渲染兼容表”串成一条带路径上下文的错误链。
- `render_compatibility_tables` 将 `GeneratedParser` 的 token 映射、符号名、规约元数据和稀疏 action/goto 表转成运行时消费的静态 Rust 数据；主表还输出稳定 `RuleId`。
- `write_generated_outputs` 创建目标目录并覆盖三份生成物；`check_generated_outputs` 只读比较当前生成结果与磁盘字节，报告缺失或陈旧文件。
- `render_token_module` 与 `rust_identifier` 生成合法 Rust token 常量，其中对 Rust 关键字和 `super` 等特殊名称做兼容转义。

本文件不生成或执行 SQL 语义动作。主解析器的动作分派仍由 `pkg/parser/parser_runtime.rs` 和 `pkg/parser/parser_actions/` 消费 `RULE_IDS_BY_REDUCTION` 等元数据；Hint 的 shift/reduce 循环在 `pkg/parser/hintparser.rs`。

## 主要符号

- `GENERATED_FILE_HEADER: &str`：所有产物的固定版权与 `@generated` 头，是字节级漂移检查的一部分。
- `GENERATED_OUTPUT_NAMES: [&str; 3]`：固定为 `main_tables.rs`、`hint_tables.rs`、`lexer_tokens.rs`；顺序也是 `generate_outputs` 的返回契约。
- `GeneratedOutput { name, contents }`：一份尚未落盘的命名文本产物；`name` 是静态白名单而非外部输入。
- `GenerationError`：面向生成和写入路径的字符串错误包装；只允许本模块通过 `new` 构造，实现 `Display` 与 `Error`。
- `GeneratedOutputIssueKind::{Missing, Stale}` 与 `GeneratedOutputIssue { path, kind }`：只读检查的逐文件漂移分类。
- `CheckGeneratedOutputsError { issues, message }`：漂移时保存多个 `issues`；生成失败或普通 I/O 失败时保存单条 `message`。其显示文本在漂移场景提示运行 `astersql-parsergen generate`。
- `ParserKind::{Main, Hint}`：内部渲染策略开关。主表使用前缀 `MAIN`、整数 `isize`、token 模块名 `token`；Hint 表使用 `HINT`、`i32`、模块名 `tokens`。
- 公共函数 `generate_outputs`、`write_generated_outputs`、`check_generated_outputs` 是 crate 门面对外暴露的三种操作；`generate_parser_output`、`render_compatibility_tables`、`render_token_module`、`rust_identifier` 是内部实现。

## 执行流程

1. `generate_outputs(parser_root)` 先对 `grammar/main.astergram` 调用 `generate_parser_output(..., ParserKind::Main)`，再对 `grammar/hint.astergram` 调用 Hint 分支。任一阶段失败都会立即返回，因而不会产生部分的内存结果。
2. `generate_parser_output` 用 `fs::read_to_string` 读取 UTF-8 文法，交给 `Grammar::parse` 校验与建模，再由 `GeneratedParser::build` 构造自动机、解析表和渲染元数据。错误分别标注 `read Rust grammar`、`parse Rust grammar` 或 `generate parser data` 及具体路径。
3. `render_compatibility_tables` 写入固定头和 token 模块，随后依次输出接受哨兵、按 token 号排序的 XLAT、符号名、规约元数据、Go 兼容规则序号、语义动作需求标记和稀疏解析表。接受值固定为 `i32::MIN`；表行只输出存在的 `(column, value)` 项。
4. 主表额外按规约顺序输出 `RuleId` 及其 `as_str`，供 Rust 运行时用稳定文法标识选择语义动作。`GENERATED_*_LEGACY_RULES` 则通过在 `grammar.productions` 中定位同一 `rule_id` 并加一，保留 Go/goyacc 的一基规则号语义。
5. `lexer_tokens.rs` 不重复解析文法，而复用主表构建得到的 `GeneratedParser.tokens`，以 `i32` 输出 `token` 模块。每个文件末尾格式固定，从而相同输入得到字节稳定结果。
6. 写入模式先 `create_dir_all(parser_root/generated)`，再逐项 `fs::write`。校验模式重新生成内存文本后逐项 `fs::read`：相同字节通过，不同字节记为 `Stale`，`NotFound` 记为 `Missing`，最后一次性返回所有漂移项。

## 数据与状态

`GeneratedParser` 的公开字段定义在 [`render.rs`](render.rs)：`tokens`、`xlat`、`symbol_names`、`reductions`、`parse_table`、`rule_ids_by_reduction` 和 `action_required_by_reduction`。本文件只将这些确定性数据序列化，不修改其中的自动机或表结构。

主要对应关系如下：token 名称/编号生成常量；`xlat` 将 lexer token 号映射到表列；`symbol_names` 提供诊断名；`reductions` 保存规约左部列与应弹出的语法符号数；`rule_ids_by_reduction` 同时派生稳定 `RuleId` 和 Go 兼容规则序号；`action_required_by_reduction` 标识是否需要 Rust 语义动作；`parse_table.rows` 保存稀疏编码的 shift、reduce、goto、accept 或 error 值。

文件本身没有全局可变状态。固定头、输出名和所有渲染顺序构成可复现性契约。磁盘是唯一外部状态：生成模式会创建目录并覆盖文件，校验模式只读取且不修复。`generate_aster_unit_test.rs::generation_is_deterministic_and_names_all_outputs` 验证同输入的结果和值顺序完全相同。

## 依赖与调用关系

上游调用关系：

- [`main.rs`](main.rs) 根据唯一 CLI 参数调用 `write_generated_outputs` 或 `check_generated_outputs`，错误打印到 stderr 并返回失败退出码。
- [`generate_aster_unit_test.rs`](generate_aster_unit_test.rs) 直接覆盖三项公共 API；`pkg/parser/parsergen_baseline_aster_unit_test.rs` 和 parser 侧契约测试验证生成数据的运行轨迹及消费边界。
- [`lib.rs`](lib.rs) 公开 `generate` 模块并重导出其符号；`pkg/parser/Cargo.toml` 仅将该 crate 声明为开发依赖，因此普通 parser 运行不动态依赖生成器。

下游依赖关系：

- `Grammar::parse`（[`grammar.rs`](grammar.rs)）负责文法解析与合法性校验。
- `GeneratedParser::build`（[`render.rs`](render.rs)）负责自动机、冲突处理、编码表及规约元数据构造；`generate.rs` 不重复这些算法。
- `pkg/parser/parser_runtime.rs` 消费主表的 XLAT、符号名、规约、legacy 规则、`RuleId` 与稀疏表；`pkg/parser/hintparser.rs::yyhint_tables` 消费对应 Hint 数据。
- `fs::{read_to_string, create_dir_all, write, read}` 是唯一系统资源接口。

RustCodeGraph 对本文件给出的关键边为 `generate_outputs → generate_parser_output → render_compatibility_tables`，以及 `generate_outputs → render_token_module`；`rust_identifier` 由 `render_token_module` 调用。由于 `fs` 等通用名称在全仓索引中高度重名，调用者结论同时以 `main.rs` 和精确引用搜索核验。

## 错误处理与边界

- 文法文件不存在、不是 UTF-8、语法非法或解析表构造失败，都被转成包含文法路径和阶段名称的 `GenerationError`。
- 创建 `generated/` 或写任一目标失败会立即终止；错误包含具体目标路径。函数没有回滚机制，所以较早文件可能已经被新内容覆盖，调用者不能把失败等同于“磁盘完全未改变”。
- 校验会聚合所有 `NotFound` 与字节不一致项，但权限错误等其他读取失败会立即返回 I/O 消息，且此时不再返回已收集的漂移列表。
- `check_generated_outputs` 严格按字节比较，包括头注释、空白和末尾换行；它不会解析或规范化已有文件，也不会创建、删除或重写文件。测试 `check_reports_missing_or_stale_output` 明确验证这一只读边界。
- `render_compatibility_tables` 中查找 legacy 规则时使用 `expect("generated rule originates from grammar")`。这是内部不变量：`GeneratedParser::build` 的规约必须来自同一个 `Grammar`；若未来打破此配对会 panic，而不是返回 `GenerationError`。
- `rust_identifier` 将 `super` 改为历史兼容名 `superToken`，将 `self`/`Self`/`crate` 加 `TOKEN_` 前缀，其他 Rust 保留字用 raw identifier；普通名称原样输出。该函数假设文法层已保证其余 token 名可成为 Rust 标识符。

## 并发与资源生命周期

所有生成数据都是函数局部拥有的 `String`、`Vec`、`Grammar` 和 `GeneratedParser`，没有锁、线程、异步任务、通道或缓存；同一输入的并行纯生成调用互不共享状态。

文件句柄由 `std::fs` 的一次性调用内部管理，返回时关闭。`write_generated_outputs` 逐文件直接覆盖，没有临时文件、原子 rename、目录锁或跨文件事务；多个进程对同一 `parser_root` 执行 `generate` 时可能交错写入，因此维护流程应串行运行生成命令。`check` 本身不写入，但若与 `generate` 并发，可能观察到三份文件处于不同代次。测试临时目录的唯一性由测试文件中的原子计数器保证，不属于生产代码状态。

## 与 Go 版本的对应关系

Go 的直接对照不是同名生成器函数，而是已有 goyacc 生成物：`pkg/parser/parser.go` 中的 `yyXLAT`、`yySymNames`、`yyReductions`、`yyParseTab`，以及 `pkg/parser/hintparser.go` 中的 `yyhintXLAT`、`yyhintSymNames`、`yyhintReductions`、`yyhintParseTab`。本文件输出的 `GENERATED_MAIN_*` 与 `GENERATED_HINT_*` 静态项保留这些表在 Rust runtime 中所需的映射、符号、规约和 shift/reduce/goto 语义。

Rust 版本的结构性差异是：使用稀疏 `(列, i32)` 行、以 `i32::MIN` 明示接受动作，并额外输出 `RULE_IDS_BY_REDUCTION` 与 `GENERATED_*_ACTION_REQUIRED`，使语义动作可按稳定规则标识在独立 Rust 模块中实现；同时保留 `GENERATED_*_LEGACY_RULES` 将生成规约索引映射回 Go 的一基规则号。主表 token 类型选择 `isize` 以适配主 lexer 的既有常量形状，Hint 表及独立 lexer token 模块使用 `i32`。

`pkg/parser/parsergen_baseline_aster_unit_test.rs` 用独立基线解释器比较主表和 Hint 表的接受、错误、shift 与 reduce 轨迹；`pkg/parser/parser_manifest_aster_unit_test.rs` 检查规约、legacy 规则和动作标记的对齐。这些测试说明这里追求的是运行语义兼容，而不是逐字复制 Go 源文件布局。

## 扩展指南

- 增加或重命名产物时，应同步修改 `GENERATED_OUTPUT_NAMES`、`generate_outputs` 的返回项、`pkg/parser/lib.rs` 的 `include!`、`pkg/parser/generate.rs::GENERATED_RUST_TABLES`、CLI/维护文档及 `generate_aster_unit_test.rs`；否则可能出现生成但未消费或消费但未校验的文件。
- 修改兼容表布局时，优先调整 `render_compatibility_tables`，并同步核对 `pkg/parser/parser_runtime.rs`、`pkg/parser/hintparser.rs::yyhint_tables`、`parsergen_baseline_aster_unit_test.rs`、`parser_manifest_aster_unit_test.rs` 和具体语义动作测试。编码含义本身应在 `table.rs`/`render.rs` 修改，不应在本文件另建一套算法。
- 新 token 若碰到 Rust 关键字，应更新 `rust_identifier` 并在独立测试文件增加覆盖；不要把测试嵌入 `generate.rs`。现有最接近的测试位置是 `generate_aster_unit_test.rs`，标识符渲染也可参考 `render_test.rs::token_names_with_grammar_punctuation_render_as_distinct_rust_identifiers`。
- 若要提高写入原子性，应围绕 `write_generated_outputs` 设计同目录临时文件、刷新与 rename，并考虑三文件跨文件一致性；这会改变失败后的磁盘状态语义，需要专门的故障注入/文件系统回归测试。
- 文法变化后的维护顺序是更新 `.astergram` 与语义动作，运行 `astersql-parsergen generate` 更新已提交文件，再运行 `astersql-parsergen check` 和相关 parser 测试。不要直接编辑带 `@generated` 头的产物。

## 验证依据

- 源码与边界：`pkg/parser/parsergen/generate.rs`、`Cargo.toml`、`lib.rs`、`main.rs`、`render.rs::GeneratedParser`、`table.rs::ENCODED_ACCEPT`。
- RustCodeGraph：`status` 显示仓库索引可用；`explore` 确认 `generate_outputs → generate_parser_output → render_compatibility_tables` 与 `generate_outputs → render_token_module`；`query/node` 定位 `GeneratedParser`、`generate_parser_output`、`rust_identifier`，并确认 `rust_identifier` 的调用者为 `render_token_module`。
- 消费侧：`pkg/parser/lib.rs` 的三个静态 `include!`、`pkg/parser/parser_runtime.rs` 的主表读取、`pkg/parser/hintparser.rs::yyhint_tables` 的 Hint 表读取、`pkg/parser/generate.rs::GENERATED_RUST_TABLES`。
- Go 对照：`pkg/parser/parser.go` 与 `pkg/parser/hintparser.go` 的 XLAT、符号名、规约和解析表定义；`pkg/parser/parser.y` 与 `hintparser.y` 是对应 Go 文法来源。
- 独立测试：`pkg/parser/parsergen/generate_aster_unit_test.rs`（确定性、名称、缺失/陈旧、只读检查、已提交产物同步），`pkg/parser/parsergen_baseline_aster_unit_test.rs`（主/Hint 表轨迹基线），`pkg/parser/parser_generated_sources_aster_unit_test.rs`（静态消费契约），`pkg/parser/parser_manifest_aster_unit_test.rs`（规约元数据对齐）。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付仅以固定章节结构检查与人工事实复核验证。

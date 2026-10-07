# `pkg/parser/parsergen/render.rs`

## 文件定位

`render.rs` 位于 `astersql-parsergen` crate 的“语法模型 → 自动机/分析表 → Rust 数据”流水线末端。crate 入口 `pkg/parser/parsergen/lib.rs` 公开 `render` 模块并重导出其 API；`pkg/parser/parsergen/Cargo.toml` 同时把该 crate 配置为库和 `astersql-parsergen` 命令行程序，且没有第三方依赖。文件本身不解析 `.astergram`、不执行 SQL 语义动作，也不读写磁盘：它接收已经由 `Grammar::parse` 构造并验证的 `Grammar`，调用 `Automaton::build`、`ParseTable::build` 和 `ParseTable::encode`，形成可供生成器和验证器消费的确定性数据。

仓库提交产物的真实生成入口在 `pkg/parser/parsergen/generate.rs::generate_parser_output`：它读取主/Hint 语法后调用 `GeneratedParser::build`，再由 `render_compatibility_tables` 输出 `pkg/parser/generated/main_tables.rs`、`hint_tables.rs` 等运行时兼容文件。`render.rs::render_rust`/`GeneratedParser::render_rust` 则提供一份自包含、通用的 Rust 常量源码表示，主要被渲染稳定性与标识符测试直接使用；不能把它误写成当前三个提交生成文件的唯一格式化入口。

## 核心职责

1. `GeneratedParser::build` 把 `Grammar`、未编码 `ParseTable` 和 `EncodedTable` 汇聚为同一份解析器快照，并保持 token、列、归约规则及语义动作标记之间的索引对应关系。
2. `GeneratedParser::trace` 用未编码表重放 shift/reduce/goto/accept 流程，提供与状态编号无关的 `TraceStep` 序列，用于把新生成数据和仓库现有解析表进行行为基线比较。
3. `render_rust` 与内部 `render_generated` 将快照确定性地串接成自包含 Rust 源码，包括 token 常量、`XLAT`、符号名、稳定规则标识、动作需求、归约元数据和稀疏解析表。
4. `sorted_tokens`、`rule_variants`、`rust_token_identifier`、`unique_token_identifiers` 等辅助函数负责输出稳定排序、合法 Rust 标识符和碰撞消解；`component_count` 保证语义动作不被误算为解析栈元素。

这些职责只覆盖解析表数据和无语义值的轨迹。AST 构造、`parser_actions::apply`、错误恢复和词法器交互属于 `pkg/parser/parser_runtime.rs::yyParse`，不在本文件中。

## 主要符号

- `GeneratedReduction { symbol, components }`：编码后的单条归约元数据。`symbol` 是产生式左部非终结符的表列号，`components` 是需要弹栈的真实语法符号数。
- `TraceStep::{Shift, Reduce(RuleId), Accept, Error}`：基线比较用的抽象轨迹；故意不暴露目标状态号，使比较不依赖自动机状态编号。
- `GeneratedParser`：公开字段 `tokens`、`xlat`、`symbol_names`、`reductions`、`parse_table`、`rule_ids_by_reduction`、`action_required_by_reduction` 是生成输出所需的数据；私有 `table`、`productions`、`terminals_by_number` 仅支持轨迹重放。
- `GeneratedProduction { lhs, components }`：按 `RuleId` 保存追踪器归约所需的最小产生式信息，不承载语义动作代码。
- `GeneratedParser::build(&Grammar) -> Result<GeneratedParser, RenderError>`：核心构造入口；表构造或编码冲突经 `RenderError` 返回。
- `GeneratedParser::trace(&[u32]) -> Vec<TraceStep>`：为外部 token 编号流自动追加 EOF 并执行表驱动追踪。
- `GeneratedParser::render_rust(&self) -> String`：复用已构造数据，避免重建自动机。
- `RenderError`：只保存可展示消息，实现 `Display`、`Error` 和 `From<TableError>`。
- `render_rust(&Grammar) -> Result<String, RenderError>`：便捷公开 API，组合 `build` 与实例渲染。
- `render_generated`：实际拼接通用 Rust 源码的内部函数。
- `sorted_tokens`、`component_count`、`column_name`、`rule_variants`、`rust_token_identifier`、`unique_token_identifiers`、`is_rust_keyword`：确定性、表元数据和合法标识符辅助函数。文件没有 trait、模块级常量或条件编译项。

## 执行流程

构建路径从 `GeneratedParser::build` 开始：先由 `Automaton::build(grammar)` 构造自动机，再由 `ParseTable::build(grammar, &automaton)` 生成未编码表，并调用 `table.encode()` 得到稀疏整数编码。随后按稳定 `RuleId` 建立最小产生式映射，按 token 编号和名称排序 token，建立“外部编号 → Terminal”映射，并从编码表中查询每个 token 的列号形成 `xlat`。符号名严格遍历 `parse_table.columns`，因此名字数组与表列共享索引。

归约元数据按 `parse_table.reduction_rules` 的顺序生成。每个规则必须能回查到语法产生式；其左部必须有非终结符列。相同循环分别填充 `reductions` 和 `action_required_by_reduction`，而 `rule_ids_by_reduction` 直接克隆编码表规则序列，从而让三个数组的相同下标指向同一条归约。`component_count` 过滤 `ProductionItem::Action`，因为动作不占 LR 状态栈位置。

追踪路径在 `GeneratedParser::trace` 中执行。输入编号经 `terminals_by_number` 翻译并在尾部追加 `Terminal::End`，状态栈从 0 开始。`Shift` 压入目标状态并消耗一个 token；`Reduce` 按产生式组件数截断状态栈，再以基底状态和左部非终结符查 `goto`，压入目标状态并记录规则；`Accept` 立即结束；未知 token、错误单元、action 位置出现 `Goto`、非法弹栈或非法 goto 都记录 `Error` 并结束。循环上限为 1,000,000 步，超限也以 `Error` 收束。

通用渲染路径为 `render_rust(&Grammar)` → `GeneratedParser::build` → `GeneratedParser::render_rust` → `render_generated`。输出依次写入版权/生成标记、EOF 与 token 常量、`XLAT`、`SYMBOL_NAMES`、生成的 `RuleId` 枚举及 `as_str`、规则与动作数组、`REDUCTIONS` 和逐行稀疏 `PARSE_TABLE`。所有顺序来自排序集合、编码表顺序或 `BTreeMap`/`BTreeSet`，同一输入应逐字节稳定。

## 数据与状态

`GeneratedParser` 同时保留“可序列化的公开编码数据”和“追踪所需的私有语义映射”。公开向量之间存在关键不变量：`reductions[i]`、`rule_ids_by_reduction[i]` 与 `action_required_by_reduction[i]` 必须描述同一归约；`xlat` 的列号和 `symbol_names` 的下标必须属于同一个 `EncodedTable.columns` 空间；每个 `GeneratedReduction.symbol` 必须指向产生式左部的非终结符列。这些不变量由 `build` 的统一来源和 `expect` 检查维持，并由 `render_aster_unit_test.rs`、`parsergen_baseline_aster_unit_test.rs::assert_metadata_matches` 验证。

确定性依赖三点：token 使用 `(number, name)` 排序；规则枚举名使用 `BTreeSet` 检测重复并按遇到顺序追加 `_2`、`_3`；语法产生式和编码表规则的顺序直接保留。token 标识符把非法 ASCII 标识符字符编码为 `_uHEX_`，空名回退为 `TOKEN_EMPTY`，普通 Rust 关键字使用原始标识符 `r#name`，路径关键字 `self`/`Self`/`super`/`crate` 则加 `TOKEN_` 前缀；最终再用确定性数字后缀解决规范化碰撞。

本文件不维护全局可变状态、不缓存跨调用结果。`build` 返回拥有所有数据的快照；`trace` 只创建局部输入迭代器、状态栈与轨迹；渲染只追加到局部 `String`。内存规模主要随 token 数、产生式数、表列数和稀疏表条目数线性增长，自动机和表构造成本由下游 `automaton.rs`、`table.rs` 决定。

## 依赖与调用关系

上游 crate 边界由 `pkg/parser/parsergen/Cargo.toml` 与 `lib.rs` 确认：`render` 只依赖同 crate 重导出的 `Automaton`、`Grammar`、`ParseTable`、`EncodedTable`、`Production`、`ProductionItem`、`RuleId`、`TableCell`、`TableColumn`、`TableError`、`Terminal`，以及标准库集合、错误和格式化设施；Cargo 没有声明外部依赖。

生产生成调用链是 `main.rs` 的 `generate`/`check` 命令 → `write_generated_outputs`/`check_generated_outputs` → `generate_outputs` → `generate_parser_output` → `GeneratedParser::build`。`generate.rs::render_compatibility_tables` 和 `render_token_module` 直接读取 `GeneratedParser` 的公开字段，生成仓库运行时格式；`parser_runtime.rs` 再通过 `generated_main_symbol`、`generated_main_action` 和 `yyParse` 消费生成的 XLAT、符号名、归约、规则映射与稀疏表。

测试侧直接调用关系包括 `render_test.rs` 与 `render_aster_unit_test.rs` 调用公开 `render_rust`，以及 `parsergen_baseline_aster_unit_test.rs` 调用 `GeneratedParser::build`、`trace` 和实例 `render_rust`。RustCodeGraph 对 `render_rust`/`build` 等常见同名符号存在歧义，精确 node 和 explore 仍确认了上述文件内链；跨文件调用关系因此以这些调用点源码作为最终依据。

## 错误处理与边界

可恢复的构建错误只有 `TableError`，经 `From<TableError>` 转换为只含消息的 `RenderError`，由 `render_rust` 原样传播。`generate_parser_output` 会再附加具体语法文件路径，形成面向命令行的 `GenerationError`。`Automaton::build` 当前不返回 `Result`，所以本文件没有对应分支。

`build` 中的三个 `expect` 表示内部一致性断言，而非用户输入错误接口：语法 token 必须具有编码表列，编码归约必须来自语法，产生式左部必须具有非终结符列。若这些条件失效，说明 `Grammar`、`ParseTable` 或 `EncodedTable` 之间出现实现缺陷；新增列过滤或归约重排时不能仅把 panic 改成默认值，否则会生成索引错位的静态表。

`trace` 是验证驱动器，不实现 `yyParse` 的完整错误恢复和语义动作。未知 token 映射为 `None` 后立即产生 `Error`；归约弹栈数大于等于当前栈长、缺失 goto、动作查询返回 `Error` 或在 action 位置异常返回 `Goto` 都终止；100 万步上限防止损坏表造成无限循环。调用方若需要错误位置、恢复、警告或 AST，必须使用 `parser_runtime.rs::yyParse`，不能依赖 `trace`。

渲染阶段本身返回 `String` 而不返回错误，前提是 `GeneratedParser` 已成功构造。它生成源码文本但不负责调用 rustfmt、编译或写文件；语法中的名称会通过调试字符串格式输出，token 常量名则由专门的标识符规则处理。

## 并发与资源生命周期

本文件没有线程、锁、通道、异步任务、文件句柄或事务。`GeneratedParser` 派生 `Clone`，所有集合和表均为拥有型数据；只读的 `render_rust(&self)` 与 `trace(&self, ...)` 不修改快照，因此并发共享能否成立取决于其字段类型自动实现的 `Sync`，文件没有手写并发保证或内部同步。

资源生命周期局限于函数调用：`build` 中的自动机和未编码表在成功后由 `GeneratedParser.table` 保留，编码表由 `parse_table` 保留；临时 `by_rule` 在归约数组生成后释放。`trace` 的状态栈和输入迭代器随返回释放；渲染 `String` 的所有权交给调用者。生产写盘发生在 `generate.rs::write_generated_outputs`，其目录创建、文件写入和 I/O 错误生命周期与本文件分离。

## 与 Go 版本的对应关系

仓库不存在 `pkg/parser/parsergen/render.go` 的一一对应文件。最接近的 Go 对照是 `pkg/parser/goyacc/main.go`：其生成阶段同样按确定顺序写出 token 常量、外部 token 到内部符号的 `XLAT`、符号名、归约 `(lhs, components)` 和解析表；生成的实际 Go 产物位于 `pkg/parser/parser.go`、`pkg/parser/hintparser.go`。Rust 的 `pkg/parser/parsergen/generate.rs::render_compatibility_tables` 进一步把 `GeneratedParser` 转成与 Go 运行时语义相近、但编码布局为 Rust 稀疏行的表。

两侧共同约束是：外部 token 必须先翻译成表列；正动作表示 shift/goto，负动作表示 reduce，零表示错误；归约元数据给出左部符号和需要弹出的右部组件数。`parser_runtime.rs::yyParse` 才是 Go `goyacc/main.go` 生成的 `yyParse` 模板的运行时迁移，对应语义值栈、动作分派和三阶段错误恢复。

Rust 特有部分包括稳定字符串 `RuleId`、`ACTION_REQUIRED_BY_REDUCTION`、合法 Rust token 标识符转换、`GeneratedParser::trace` 以及自包含 `render_rust` 格式。Go 生成器直接嵌入语义动作和 Go 标识符/格式规则，Rust 则刻意把动作留给 `parser_actions::apply`。因此验证应比较接受/拒绝、shift/reduce/goto 和归约映射语义，而不应要求 Go/Rust 生成源码逐字节或表的物理布局相同；`parsergen_baseline_aster_unit_test.rs` 也明确以主/Hint 语法契约和现有 Rust 表作基线，而非复制 Go 内部表布局。

## 扩展指南

若新增通用输出字段，优先在 `GeneratedParser` 增加明确数据及其构建逻辑，再同步 `render_generated`；若该字段供实际运行时使用，还必须同步 `generate.rs::render_compatibility_tables`、生成文件消费者和提交生成物，不能只改通用 `render_rust`。所有按归约编号索引的数据都应在同一次遍历中生成，并在独立测试中断言长度与规则对应关系。

若修改 token 命名规则，应集中调整 `rust_token_identifier`、`unique_token_identifiers` 或 `is_rust_keyword`，同时扩展 `pkg/parser/parsergen/render_test.rs`，覆盖标点、空名、路径关键字、普通/保留关键字以及规范化后碰撞。需要注意 `generate.rs` 另有 `rust_identifier` 供生产兼容输出使用；两者若应保持一致，必须显式同步并分别验证，当前实现并非同一个函数。

若修改归约长度、规则枚举或表编码，应同步检查 `component_count`、`rule_variants`、`GeneratedParser::build` 和 `trace`，并扩展 `render_aster_unit_test.rs`、`parsergen_baseline_aster_unit_test.rs` 以及 `table_aster_unit_test.rs`。关键兼容风险是归约数组下标错位、语义动作被计入弹栈数、token 列号与符号名错位；性能风险主要是新增无界复制、把稀疏表改为稠密输出或破坏 `BTree*`/排序带来的稳定性。

新增测试必须继续放在独立文件中，通过 `lib.rs` 的 `#[cfg(test)] mod ...` 接入，不应把测试嵌进 `render.rs`。对只影响文档的修改不需要生成或 Cargo；对未来行为修改则应先用最小语法构造回归用例，并同时验证失败/接受轨迹和生成字节稳定性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/parser/parsergen/render.rs`（37 个符号）；`node --file` 阅读了该文件 1–468 行；`query render_rust` 区分出实例方法（第 193 行）和公开自由函数（第 221 行）；`explore` 确认文件内 `render_rust`、`render_generated`、`build`、`trace` 及辅助函数关系。由于常见名称存在跨仓库同名候选，`callers`/`callees` 未给出可辨识输出，跨文件边改由精确调用点源码核验。
- crate 与入口：`pkg/parser/parsergen/Cargo.toml`、`lib.rs`、`main.rs`。
- 生产生成链：`pkg/parser/parsergen/generate.rs::generate_outputs`、`generate_parser_output`、`render_compatibility_tables`、`render_token_module`；实际消费：`pkg/parser/parser_runtime.rs::generated_main_symbol`、`generated_main_action`、`yyParse`。
- 独立 Rust 测试：`pkg/parser/parsergen/render_test.rs` 验证带标点 token 生成不同合法标识符；`render_aster_unit_test.rs` 验证逐字节可复现、关键输出段和归约元数据长度；`pkg/parser/parsergen_baseline_aster_unit_test.rs` 验证主/Hint 语法的元数据、稳定渲染、接受/拒绝及未知 token 轨迹。
- Go 对照：`pkg/parser/goyacc/main.go` 的常量、XLAT、符号名、归约和解析器模板；其生成产物 `pkg/parser/parser.go`、`pkg/parser/hintparser.go`。未发现同路径 `render.go`，所以文档明确采用最近语义对照而非宣称逐文件复刻。
- 本任务是纯文档分析，按任务约束未运行 Cargo。结构检查要求目标文件存在且固定二级标题恰好为 11 个；最终交付前另行执行并记录退出码。

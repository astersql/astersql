# `pkg/parser/parsergen/table.rs`

## 文件定位

`table.rs` 位于 `astersql-parsergen` crate 的“LALR 自动机之后、Rust 解析器数据渲染之前”。`pkg/parser/parsergen/lib.rs` 以 `pub mod table` 声明该模块并重导出其公开 API；直接生产调用者 `GeneratedParser::build`（`pkg/parser/parsergen/render.rs`）先执行 `Automaton::build`，再调用 `ParseTable::build` 和 `ParseTable::encode`。因此本文件不读取文法文件、不构造 LR 项目闭包，也不执行语义动作；它把已经构造好的 `Grammar` 与 `Automaton` 转换成经过冲突消解的 action/goto 表，并进一步生成确定性的稀疏整数表示。

crate 边界由 `pkg/parser/parsergen/Cargo.toml` 确定：库入口是同目录 `lib.rs`，同时有 `main.rs` 二进制入口，当前没有外部依赖。本文件只使用标准库集合/错误接口和 crate 内 `grammar.rs`、`automaton.rs` 导出的模型。

## 核心职责

1. `ParseTable::build` 从自动机的状态转移和完成项目收集 shift、goto、reduce、accept 候选，并在候选收集完毕后统一消解冲突。
2. `PrecedenceIndex` 根据文法的优先级声明、结合性、产生式 `%prec` 覆盖或最右终结符，处理标准的一移进/一归约冲突；不能证明可消解的候选组合返回 `TableError`。
3. `ParseTable::action` 与 `ParseTable::goto` 提供类型化查询，缺失项或越界状态统一表现为 `TableCell::Error`。
4. `ParseTable::encode` 固定列顺序与归约编号，把类型化单元编码成只保存非零项的 `EncodedTable`；`SparseRow` 提供二分查询和稠密展开。
5. 通过 `BTreeMap`、`BTreeSet`、显式排序和去重，使相同文法/自动机的表布局、冲突诊断候选顺序和渲染输入保持确定。

## 主要符号

- `ENCODED_ACCEPT: i32 = i32::MIN`：整数表中与 shift/goto 正数、reduce 负数和 error 零值互不混淆的接受哨兵。
- `TableCell::{Shift, Reduce, Accept, Error, Goto}`：编码前的强类型表单元。`Shift/Goto` 保存目标状态，`Reduce` 保存稳定的 `RuleId`。
- `TableColumn::{Terminal, Nonterminal}`：合并 action/goto 表后的列键；派生全序，用于列号索引和确定性比较。
- `TableError`：同时承载冲突错误和整数编码错误。冲突含 `state`、`lookahead`、稳定排序的 `candidates` 与完整消息；编码错误没有状态/向前看信息。
- `ParseTable`：保存自动机状态副本、逐状态 action/goto 映射、稳定列布局和稳定归约规则序列。公开入口为 `build`、`states`、`action`、`goto`、`encode`。
- `Candidate` 与 `resolve_candidates`：内部候选模型及冲突消解核心。单候选直接转换；只有“一次 shift + 一次 reduce”能进入优先级规则，其余多候选冲突报错。
- `PrecedenceInfo`、`PrecedenceIndex`、`ShiftReduceResolution`：把 token/literal 映射到优先级与结合性，并返回 shift、reduce、显式 error 三种决议。
- `stable_columns`：产生 EOF、token、非终结符的完整稳定列布局。
- `encode_cell`：实现整数协议并检查状态/归约编号是否能装入 `i32`。
- `SparseEntry`、`SparseRow`、`EncodedTable`：编码结果的数据结构；`SparseRow::get` 用二分查找，`decode` 展开完整行，`EncodedTable::column/get` 提供符号级查询。

## 执行流程

`ParseTable::build` 的主流程如下：

1. 将 `grammar.productions` 建为 `RuleId -> Production` 索引，并由 `PrecedenceIndex::new` 建立 token、literal 和优先级反向索引。
2. 为每个自动机状态准备候选 action 行和 goto 行。遍历 `state.transitions` 时，终结符转移形成 `Candidate::Shift`，非终结符转移直接形成 `TableCell::Goto`。
3. 遍历状态项目。点位为 1 的增广开始项目在 `Terminal::End` 上加入 `Accept`；普通产生式项目只有当点位等于 `production_symbol_count` 时，才在每个 lookahead 上加入 `Reduce`。该计数刻意跳过 `ProductionItem::Action`，因为内嵌语义动作不消耗文法符号。
4. 完成全部收集后逐状态、逐 lookahead 调用 `resolve_candidates`。单候选直接落表；标准 shift/reduce 冲突交给 `PrecedenceIndex::resolve_shift_reduce`；其他或缺少足够优先级信息的冲突通过 `TableError::conflict` 失败。
5. `stable_columns` 把 EOF 放在首列，随后按 `(token.number, token.name)` 排 token，最后按名称排序所有产生式左部非终结符。归约规则按 `RuleId` 排序、去重后写入 `ParseTable`。

`ParseTable::encode` 随后为列和归约规则建立从零开始的编号。逐状态编码 action 与 goto，省略编码值为零的 error 单元，将条目按列号排序并形成 `SparseRow`。整数协议是：`0` 为错误；`target + 1` 的正数为 shift/goto；`-(reduction_index + 1)` 为 reduce；`i32::MIN` 为 accept。最终的 `EncodedTable` 同时保留列布局和负数所引用的 `reduction_rules`。

运行期的直接验证消费者是 `GeneratedParser::trace`（`render.rs`）：它用 `ParseTable::action` 驱动 shift/reduce/accept/error，用 `ParseTable::goto` 完成归约后的状态跳转。生成输出则读取 `EncodedTable` 的列、稀疏行和归约序列。

## 数据与状态

本文件构造的是只读值对象，没有全局可变状态。`ParseTable` 的 `states` 与自动机状态数一一对应，`actions[state]`、`gotos[state]` 和编码后的 `rows[state]` 共享相同状态编号；源码以 `state.id` 直接索引这些向量，因此前置不变量是 `Automaton::build` 产生连续、合法且可用于索引的状态 ID。

`BTreeSet<Candidate>` 同时去重候选并固定诊断次序；action/goto 行和索引使用 `BTreeMap`，列及归约规则再显式排序。这些选择是确定性输出的一部分，而不只是实现细节。`SparseRow.entries` 必须严格按列递增，`SparseRow::get` 的二分查找依赖此不变量；缺失列以零解释。完整 `columns` 仍保留从未出现非零动作的 token，因此新增/未使用 token 不会令后续列在单次构建中因稀疏化而消失。

归约负数不直接编码 `RuleId`，而是引用 `EncodedTable.reduction_rules` 的稳定索引。调用者若持久化或渲染整数表，必须同时保持这份规则序列以及 `columns`，否则无法恢复单元语义。

## 依赖与调用关系

上游数据来自 crate 内类型：`Grammar`/`Production`/`ProductionItem`/`PrecedenceSymbol`/`Associativity` 定义文法与优先级，`Automaton`/`State`/`ItemRule` 提供 LALR 状态、转移、项目及 lookahead，`Terminal`/`Symbol`/`RuleId` 作为键和值。标准库的 `BTreeMap`/`BTreeSet` 提供有序集合，`std::fmt` 与 `std::error::Error` 实现错误接口。

直接生产调用边可由 `pkg/parser/parsergen/render.rs` 核对：`GeneratedParser::build -> Automaton::build -> ParseTable::build -> ParseTable::encode`。同文件的 `GeneratedParser::trace` 再调用 `ParseTable::action/goto` 复现表驱动解析；渲染函数读取 `EncodedTable`，生成自包含 Rust 常量。`pkg/parser/parsergen/lib.rs` 将本文件的公开符号重导出给 crate 使用者。

RustCodeGraph 已索引 `table.rs`（报告 35 个符号），并能展示文件源码与被使用信息；本次对限定名称 `ParseTable::build`、`ParseTable::encode` 执行 callers/callees 未返回可用边，因此上述精确调用关系以已读的 `render.rs` 和 `lib.rs` 源码为直接证据，而不把图工具缺失结果当作不存在调用者。

## 错误处理与边界

- 未消解冲突：`resolve_candidates` 仅允许标准的一 shift/一 reduce 组合尝试优先级消解。reduce/reduce、accept 与其他候选混合、多个 reduce，或缺少所需优先级信息时，返回含状态、lookahead 和候选描述的 `TableError`，不会静默任选动作。
- 非结合运算符：同优先级且 `Associativity::NonAssoc` 时成功生成 `TableCell::Error`，表示该 token 序列在运行期是语法错误；这与“无法决定表项”的构表失败不同。
- `PrecedenceOnly`：同级时不指定结合方向，`resolve_shift_reduce` 返回 `None`，最终作为未消解冲突报告。
- 缺失查询：`action`、`goto` 和 `EncodedTable::get` 对越界状态、不存在的符号/列或稀疏缺项返回错误值，而非 panic。
- 内部一致性：由同一 `Grammar`/`Automaton` 推导出的规则和列使用 `expect` 或索引；例如自动机规则必须存在于文法、优先级 literal 必须已经过文法校验、编码的 reduce 必须存在于规则索引。这些 panic 表示上游模型破坏内部不变量，不是面向无效文法的普通错误通道。
- 数值边界：状态目标或归约序号执行 `checked_add(1)` 和 `i32::try_from`；溢出返回无状态上下文的编码 `TableError`。正数加一也保证状态 0 不与 error 的 0 冲突。
- 稠密展开：`SparseRow::decode(width)` 在条目列号不小于调用方宽度时断言失败；调用者应传 `EncodedTable.columns.len()`。

## 并发与资源生命周期

构表和编码都是同步、单线程、纯内存流程，不创建线程、异步任务、锁、通道、文件句柄或事务。`ParseTable::build` 克隆自动机状态以及作为键/值保存的终结符和规则 ID，返回值不借用 `Grammar` 或 `Automaton`；`EncodedTable` 又拥有列、行和规则列表。因此输入离开作用域后结果仍可独立用于追踪或渲染。

当前公开查询只需要 `&self`，构造完成后没有内部可变性；能否跨线程共享最终取决于所含 crate 类型是否实现 `Send`/`Sync`，本文件没有显式并发承诺。主要资源风险是状态数乘符号数带来的候选/行容器内存和排序成本；稀疏编码只压缩最终的零单元，不改变构表阶段按状态分配候选、action、goto 向量的事实。

## 与 Go 版本的对应关系

同目录没有 `table.go`，所以不存在可证明的逐函数 Go 复刻。最接近的 Go 对照是 `pkg/parser/goyacc/main.go`：它从 goyacc 处理结果的 `p.Table` 生成 token 翻译、符号名、归约信息和稀疏解析表，并把 shift 记为正参数、reduce 记为负参数、accept 作特殊处理；`pkg/parser/parser.go` 是该 Go 流程生成的实际解析器产物。Rust 的 `GeneratedParser::build/render_rust` 与本文件合起来承担相邻职责。

两侧共同目标是把语法分析状态表转成紧凑、可由表驱动运行时消费的数据，但编码不可直接互换：Go 生成器使用 `TabOfs` 偏移和无符号位宽选择来让零单元表示空值；Rust 明确定义 `0`、正数、负数和 `i32::MIN`，并单独保留 `columns`/`reduction_rules`。Go 的冲突统计和消解主要由其 `y.ProcessFile` 所属 yacc 实现产生，而 Rust 在 `ParseTable::build` 内显式收集候选并以 `PrecedenceIndex` 消解，未解冲突直接返回错误。

语义对照应关注这些行为而非要求字节级一致：终结符/非终结符表项含义、shift/reduce/accept/error 的解析结果、优先级和结合性决议、归约弹栈长度与 goto，以及相同输入的确定性生成。当前独立 Rust 测试验证的是 Rust 契约；本任务没有运行 Go 或 Rust 测试，不能据此宣称两套生成器已对完整 TiDB 文法实现结果等价。

## 扩展指南

- 新增 action 类型或改变整数协议时，应同时修改 `TableCell`、`Candidate`（若参与构表）、`resolve_candidates`、`encode_cell`、`GeneratedParser::trace` 和 `render.rs` 的生成格式，并在 `table_aster_unit_test.rs` 增加类型化查询、稀疏编码及哨兵兼容回归。
- 调整冲突策略时，优先在 `PrecedenceIndex::resolve_shift_reduce` 或 `resolve_candidates` 做局部修改；必须分别覆盖左结合、右结合、非结合、`PrecedenceOnly`、不同级别、`%prec`、缺少声明、shift/reduce 和 reduce/reduce，避免把未证明安全的组合静默接受。
- 改变列/归约排序会影响生成产物稳定性及所有索引消费者。需同步检查 `stable_columns`、`ParseTable::encode`、`render.rs` 中 token XLAT、symbol names、reductions 和规则映射，并保留重复构建结果完全一致的测试。
- 若引入更激进压缩，必须维持缺失项等价于 error、按状态查询、列到符号的可逆关系及归约编号映射；若仍使用 `SparseRow::get`，条目升序是不可破坏的不变量。
- 若要支持超出 `i32` 的状态或归约规模，应设计新的生成格式和兼容迁移，不能只去掉 `encode_cell` 的检查。
- Rust 单元测试继续放在独立的 `pkg/parser/parsergen/table_aster_unit_test.rs`，不要嵌入 `table.rs`；涉及渲染或完整 trace 的行为还应同步 `render_aster_unit_test.rs`/`render_test.rs`。如需对齐 Go 行为，再以 `pkg/parser/goyacc/main.go` 与生成的 `pkg/parser/parser.go` 为对照，但不要假定编码格式相同。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含 7,032 个 Rust 文件；`files --filter pkg/parser/parsergen/table.rs` 命中唯一目标并报告 35 个符号；`node --file ... --offset 1 --limit 500` 及后续 `--offset 480 --limit 160` 覆盖了目标文件 574 行。对 `ParseTable::build`/`ParseTable::encode` 的 callers/callees 查询没有返回可用边，调用边改由源码核实。
- 目标源码：`pkg/parser/parsergen/table.rs`，核对了所有公开/内部类型、函数、impl、编码协议、冲突分支和断言；文件没有条件编译项。
- crate 与入口：`pkg/parser/parsergen/Cargo.toml`、`pkg/parser/parsergen/lib.rs`。
- 直接调用者/消费者：`pkg/parser/parsergen/render.rs`，尤其是 `GeneratedParser::build`、`GeneratedParser::trace` 和渲染所需的 `EncodedTable` 数据。
- 独立 Rust 测试：`pkg/parser/parsergen/table_aster_unit_test.rs`，覆盖优先级/结合性/`%prec`、未解 shift/reduce 与 reduce/reduce 诊断、完整稳定列、稀疏行查询/展开、goto、accept 哨兵和重复构建确定性。
- Go 对照：`pkg/parser/goyacc/main.go` 的符号/归约/解析表生成区段，以及生成产物 `pkg/parser/parser.go`；同目录不存在一一对应的 `table.go`。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付前仅运行任务指定的 11 章节结构验证，并人工复核本文所有运行时结论均指向以上源码、调用者或测试证据。

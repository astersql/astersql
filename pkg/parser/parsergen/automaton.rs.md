# `pkg/parser/parsergen/automaton.rs`

## 文件定位

本文件属于 `astersql-parsergen` crate 的自动机构造阶段，位于“文法解析/校验”和“action/goto 表生成”之间。crate 入口 `pkg/parser/parsergen/lib.rs` 公开 `automaton` 模块并重导出其 API；`pkg/parser/parsergen/Cargo.toml` 将该目录同时声明为库和 `astersql-parsergen` 命令行程序，且没有外部 Rust 依赖。

完整生成链路是：`generate::generate_parser_output` 读取 `.astergram` 并调用 `Grammar::parse`，`render::GeneratedParser::build` 调用本文件的 `Automaton::build`，随后把结果交给 `table::ParseTable::build` 生成 action/goto 表并编码。因而本文件只负责从已校验的 `Grammar` 构造确定性的 LALR(1) 项目图，不负责解析文法文本、冲突消解、整数表编码、文件 I/O 或运行时语义动作。

## 核心职责

1. `NormalizedGrammar::new` 把 `Grammar` 转为自动机需要的统一符号序列：token 名称和字面量都成为 `Terminal::Token`，非终结符成为 `Symbol::Nonterminal`，`ProductionItem::Action` 被剔除，并在第 0 条插入 `$accept → start` 增广产生式。
2. `FirstSets::compute` 对用户产生式反复扫描直至不动点，计算每个非终结符的 nullable 属性与 FIRST 终结符集合。
3. `Lr0Automaton::build` 从增广开始项目出发，以 `lr0_closure` 和 `lr0_goto` 构造 LR(0) 核心图，并复用相同 `CoreSet`，从结构上完成具有相同 LR(0) 核心的状态合并。
4. `Lr0Automaton::propagate_lookaheads` 以初始项目的 `$end` 为种子，在状态闭包和 GOTO 边上反复传播向前看集合，直至所有集合稳定，再物化公开的 `State`/`Item`。
5. 全过程使用 `BTreeMap`/`BTreeSet` 固定符号、项目、转换和状态发现顺序，使相同文法得到可重复的状态编号和生成结果。

## 主要符号

- `Terminal::{End, Token(String)}`：规范化终结符。`End` 是内部输入结束标记，`Terminal::name` 分别返回 `$end` 或 token 名称；字面量不会保留为独立终结符，而是在 `NormalizedGrammar::new` 中映射到声明该字面量的 token 名称。
- `Symbol::{Terminal, Nonterminal}`：LR 状态转换边上的统一文法符号，同时作为有序映射的键。
- `FirstSets`：保存私有的 `nullable`、各非终结符的 FIRST `terminals`，以及供未知名称查询返回的稳定空集 `empty`。公开查询 `is_nullable` 与 `terminals` 不修改状态；未知名称的 `terminals` 返回空集而不是报错。
- `ItemRule::{AugmentedStart, Production(RuleId)}`：区分内部增广规则和来自 `Grammar` 的稳定规则标识。
- `Item`：公开 LALR 项目，包含规则、圆点位置 `dot` 和该 LR(0) 核心合并后的 `lookaheads`。
- `State`：公开状态，`id` 等于其在 `Automaton::states` 中的下标，`items` 是有序项目，`transitions` 将终结符或非终结符映射到目标状态编号。
- `Automaton`：公开结果，汇总 `first` 与 `states`；公开入口只有 `Automaton::build(&Grammar) -> Automaton`。
- `AutomatonProduction`、`NormalizedGrammar`：内部规范化表示。`productions[0]` 固定为增广产生式；`productions_by_lhs` 只索引用户产生式，因此 FIRST 计算跳过第 0 条。
- `ItemCore { production, dot }`、`CoreSet`、`LookaheadSets`：内部 LR(0) 身份与向前看集合。项目核心不含 lookahead，所以同核心的多个 LR(1) 项目会合并其集合。
- `Lr0State`、`Lr0Automaton`：仅保存核心集合和转换图的中间结构。
- `lr0_closure`：当圆点后是非终结符时，把该非终结符所有产生式的点前项目加入集合，反复执行到闭包稳定。
- `lr0_goto`：筛出圆点后匹配指定符号的项目，将圆点前移一位，再求闭包。
- `symbol_after_dot`：通过产生式索引和圆点位置返回下一符号；完成项目返回 `None`。
- `extend`：将一组向前看符号并入目标集合，并用集合长度变化报告不动点算法是否继续。

## 执行流程

`Automaton::build` 的执行顺序固定为规范化、FIRST/nullable、LR(0) 图、lookahead 传播四步。

规范化时先建立 token 名集合和“字面量 → token 名”映射。随后插入 `$accept → grammar.start`，并逐条转换用户产生式：已声明 token 的 `ProductionItem::Symbol` 是终结符，其他符号是非终结符，`Literal` 通过映射变成 token，动作项不消耗输入所以不进入右部。最后按左部建立产生式索引。

`FirstSets::compute` 对每条用户产生式从右部起点向后扫描：遇到终结符便加入它并停止；遇到非终结符便合并其当前 FIRST，若该非终结符尚不可空则停止；若整个右部都可空（空产生式也满足），则把左部标为 nullable。任一集合增长都会触发下一轮，因此跨多层产生式的事实最终会传播到稳定状态。

`Lr0Automaton::build` 以 `(production=0, dot=0)` 的闭包作为状态 0。对当前状态圆点后的所有不同符号按有序集合遍历，计算 GOTO 核心；`state_ids: BTreeMap<CoreSet, usize>` 已见过相同核心时复用状态，否则按发现顺序追加新状态。循环在没有未处理的新状态时结束。

`propagate_lookaheads` 为每个状态的每个核心建立空集合，并只向状态 0 的增广开始项目播种 `Terminal::End`。每轮使用当前状态 lookahead 的快照执行两种传播：若圆点后为非终结符 `B`，用 `FIRST(β lookahead)` 传播到同状态中所有 `B → ·γ` 项目；同时沿当前符号对应的 GOTO 边，把 lookahead 原样传播到圆点右移后的同一产生式项目。两类传播都不再扩大集合时，将内部核心转换为公开 `Item`，保留 LR(0) 转换并按向量下标赋予 `State::id`。

## 数据与状态

所有算法状态均为 `Automaton::build` 调用内的拥有型数据，没有全局变量或缓存。主要单调状态包括 nullable 集、FIRST 集、已发现 LR(0) 核心集合和每个核心的 lookahead 集；它们只会新增元素，不会删除，因此有限文法上必然达到不动点。

关键不变量如下：

- `NormalizedGrammar::productions[0]` 始终是唯一的增广开始规则；公开用户规则通过 `RuleId` 关联回 `Grammar::productions`。
- `productions_by_lhs` 中出现的索引都指向用户产生式；其左部必须是非终结符。
- 每个 `Lr0State::cores` 已经过闭包；每条 `transitions[symbol]` 的目标包含所有匹配项目圆点右移后的闭包。
- `items[state_id]` 与 `self.states[state_id].cores` 键集合一致，所以传播阶段对核心的 `get_mut(...).expect(...)` 应始终成功。
- 构造完成时 `state.id == states` 下标；公开项目和转换顺序由树形集合/映射决定，不依赖随机哈希种子。
- `ProductionItem::Action` 不计入自动机右部长度；下游 `table::production_symbol_count` 同样排除动作项，完成项目判定必须保持这一口径一致。

内存开销主要来自每个状态的 LR(0) 闭包、`CoreSet` 到编号的去重映射，以及按“状态 × 项目”保存的终结符 lookahead 集。传播循环每轮克隆单个状态的 `LookaheadSets` 快照，以避免遍历时同时修改；这简化了单调传播，但大型文法扩展时应关注迭代轮数和集合复制成本。

## 依赖与调用关系

上游直接调用者是 `pkg/parser/parsergen/render.rs` 的 `GeneratedParser::build`：它先执行 `Automaton::build(grammar)`，再调用 `ParseTable::build(grammar, &automaton)`。测试还会直接构造自动机或将其交给构表器。再上游，`pkg/parser/parsergen/generate.rs::generate_parser_output` 从 `main.astergram`/`hint.astergram` 创建 `Grammar` 和 `GeneratedParser`；`pkg/parser/parsergen/main.rs` 的 `generate`/`check` 命令触发生成或防漂移检查。

下游 `pkg/parser/parsergen/table.rs::ParseTable::build` 消费本文件的状态：终结符转换产生 shift 候选，非终结符转换产生 goto；完成的普通项目按各自 lookahead 产生 reduce 候选，完成的增广开始项目在 `$end` 上产生 accept。冲突检测与优先级消解属于 `table.rs`，不是本文件职责。

本文件只使用标准库 `BTreeMap`/`BTreeSet`，并依赖 crate 内 `Grammar`、`ProductionItem`、`RuleId`。其正确性还依赖 `grammar.rs` 的前置校验：起始符必须存在，引用符号必须已声明或有产生式，token 名/编号/字面量必须唯一。RustCodeGraph 确认内部调用边 `Automaton::build → NormalizedGrammar::new / FirstSets::compute / Lr0Automaton::build / propagate_lookaheads`，以及 `propagate_lookaheads → sequence_with_lookahead`；工作区文本引用确认生产调用边来自 `render.rs`。

## 错误处理与边界

`Automaton::build` 不返回 `Result`，因为公开契约要求输入已经由 `Grammar::parse` 校验。本文件对违反内部不变量的情况使用 `expect` 或索引失败立即 panic，而不是生成可能错误的解析表：例如未知字面量、缺少产生式左部 FIRST 条目、LR(0) 闭包缺少应有项目、GOTO 缺少右移项目或状态缺少当前符号的转换。

可观察的非错误边界包括：空产生式会使左部 nullable；未知非终结符传给公开 `FirstSets::terminals` 时返回共享空集；已完成项目的 `symbol_after_dot` 返回 `None` 并停止继续传播；动作项被忽略，因此仅含动作的产生式在自动机中等价于空右部。`Terminal::End` 只由内部增广规则和输入结束语义引入，不来自用户 token。

本文件不负责报告 shift/reduce 或 reduce/reduce 冲突。相同 LR(0) 核心合并可能暴露 LALR 冲突，这些冲突由 `ParseTable::build` 结合优先级处理，无法消解时返回带状态与 lookahead 的 `TableError`。

## 并发与资源生命周期

该实现是同步、单线程、无锁的纯内存构造过程，不创建线程、异步任务、通道、事务或外部资源。输入 `&Grammar` 只读借用；构造过程中需要保留的名称、规则标识和终结符均克隆到内部拥有型集合，返回的 `Automaton` 不借用输入，因此生命周期不与 `Grammar` 绑定。

方法内部没有共享可变状态，多个线程可各自对不同或共享只读 `Grammar` 调用 `Automaton::build`；本文件本身不提供跨调用缓存或同步保证。确定性来自有序容器和固定遍历顺序，而非串行化全局状态。

## 与 Go 版本的对应关系

仓库没有与 `pkg/parser/parsergen/automaton.rs` 同路径或同符号布局的 Go 自动机实现，因此不能声称 `FirstSets`、`Lr0Automaton` 等是逐函数翻译。现有 Go 入口 `pkg/parser/goyacc/main.go` 调用外部 `y.ProcessFile(..., &y.Options{...})` 获得包含 `Table`、规则和冲突计数的处理结果，然后渲染 Go 解析表；该文件的 `-c`/`-la` 选项也表明底层工具能够报告状态闭包和 lookahead，但核心算法位于外部 `github.com/cznic/y` 实现，不在本仓库中。

两条路径的可比目标是：从 yacc 风格文法得到状态表、lookahead 驱动的归约、shift/reduce/goto/accept 行为以及稳定的生成产物。Rust 路径将关键阶段拆成本仓库可审阅的 `grammar.rs → automaton.rs → table.rs → render.rs`，并由 `RuleId` 和有序集合强化确定性；Go 路径由外部处理器构表，再在 `goyacc/main.go` 中编码和生成运行时解析器。当前证据支持“职责和产物角色对应”，不支持状态编号、内部数据结构或冲突算法逐项相同；这类兼容性应通过相同文法的生成表或解析行为基线验证，而不能仅凭符号名称推断。

## 扩展指南

- 新增终结符表示或改变字面量规范化时，修改 `Terminal`、`Symbol` 和 `NormalizedGrammar::new`，并同步检查 `table.rs::stable_columns`、`render.rs::column_name` 及 token 翻译逻辑；保持字面量和 token 名映射唯一的前置校验。
- 改变 nullable/FIRST 语义时，聚焦 `FirstSets::compute` 与 `sequence_with_lookahead`。必须覆盖多层可空链、可空后缀、终结符截断、全序列可空继承 lookahead 等分支。
- 改变状态构造或合并策略时，聚焦 `ItemCore`、`lr0_closure`、`lr0_goto`、`Lr0Automaton::build`。不要把 lookahead 加入核心身份，否则会从当前 LALR 核心合并策略偏向规范 LR(1)，显著改变状态数、冲突和生成表。
- 优化 lookahead 传播时，必须保持闭包内 `FIRST(βa)` 与跨 GOTO 边传播两类边，且维持单调不动点。可调整工作队列或减少快照克隆，但需证明结果集合及稳定顺序不变。
- 公开结构字段被 `table.rs` 和测试直接消费；改变 `dot` 口径、增广规则位置或状态编号策略会影响完成项目、accept 状态、表编码和已提交生成物，应作为跨文件兼容变更处理。
- 测试必须继续放在独立文件 `pkg/parser/parsergen/automaton_aster_unit_test.rs`，不要内嵌到生产源文件。算法变化还应同步运行 `table_aster_unit_test.rs` 和生成/渲染相关独立测试；涉及最终产物时核对 `pkg/parser/generated/*.rs` 防漂移。
- 性能改动需关注大型文法下的状态数、闭包大小、传播轮数和集合克隆；确定性是生成器的外部合同，不能用无固定遍历顺序的容器替换而不增加排序边界及回归证据。

## 验证依据

- 源文件全貌：`pkg/parser/parsergen/automaton.rs`，重点符号为 `Automaton::build`、`NormalizedGrammar::new`、`FirstSets::compute`、`FirstSets::sequence_with_lookahead`、`Lr0Automaton::{build, propagate_lookaheads}`、`lr0_closure`、`lr0_goto`、`extend`、`symbol_after_dot`。
- crate 与入口：`pkg/parser/parsergen/Cargo.toml`、`pkg/parser/parsergen/lib.rs`、`pkg/parser/parsergen/main.rs`。
- 上下游实现：`pkg/parser/parsergen/grammar.rs`、`pkg/parser/parsergen/table.rs`、`pkg/parser/parsergen/render.rs`、`pkg/parser/parsergen/generate.rs`。
- RustCodeGraph：索引包含 `pkg/parser/parsergen/automaton.rs` 的 26 个符号；精确查询确认 `propagate_lookaheads` 调用 `sequence_with_lookahead`，并通过文件节点核对了全部 456 行源码。宽泛符号（如 `build`、`state`）存在大量跨仓库同名噪声，生产调用者因此另用限定目录文本引用核验为 `render.rs::GeneratedParser::build`。
- 独立 Rust 测试：`pkg/parser/parsergen/automaton_aster_unit_test.rs` 验证跨层 nullable/FIRST 和动作忽略、可空后缀的闭包 lookahead、相同 LR(0) 核心合并、固定状态/转移快照，以及重复 32 次构造的确定性；`table_aster_unit_test.rs` 进一步通过 `Automaton::build` 验证构表、冲突和编码消费者契约。
- Go 对照：`pkg/parser/goyacc/main.go` 中 `y.ProcessFile` 调用、`-c`/`-la` 选项、`p.Table`/`p.Rules` 的编码流程。仓库未发现本地 Go `automaton.go` 或 FIRST/closure 核心实现，因此实现级逐符号对应明确标记为不存在。
- 按任务约束未运行 Cargo；本任务只新增说明文档，采用任务文件指定的 11 章节结构检查作为交付验证。

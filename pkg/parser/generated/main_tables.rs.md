# `pkg/parser/generated/main_tables.rs`

## 文件定位

只描述语法自动机，不执行词法扫描、不分配 AST、不实现语义动作，也不直接报告用户错误；这些工作分别位于 lexer、`parser_runtime.rs::yyParse` 和 `parser_actions::apply`。

## 核心职责

本文件把 `main.astergram` 的 LALR(1) 自动机编码为只读 Rust 数据，供 `pkg/parser/parser_runtime.rs::yyParse` 驱动。职责分为五层：

1. `token` 模块保存词法器向解析器提交的外部 token 编号。
2. `GENERATED_MAIN_XLAT` 把外部 token 编号翻译为解析表列号，`GENERATED_MAIN_SYMBOL_NAMES` 为终结符和非终结符提供同一列空间中的可读名称。
3. `GENERATED_MAIN_PARSE_TABLE` 保存每个状态的稀疏动作/goto 行。
4. `GENERATED_MAIN_REDUCTIONS` 描述规约的左部符号列和右部栈元素数；`RULE_IDS_BY_REDUCTION`、`GENERATED_MAIN_LEGACY_RULES` 与 `GENERATED_MAIN_ACTION_REQUIRED` 把同一规约关联到稳定动作标识、兼容的 Go 规则序号和语义动作完整性标记。
5. `GENERATED_MAIN_ACCEPT` 提供不会与普通移进或规约编码冲突的接受哨兵。

它只描述语法自动机，不执行词法扫描、不分配 AST、不实现语义动作，也不直接报告用户错误；这些工作分别位于 lexer、`parser_runtime.rs::yyParse` 和 `parser_actions::apply`。

## 主要符号

- `mod token`：crate 内私有模块，其中的 token 常量是 `pub`，以便同一父模块中的 lexer/解析器代码通过 `token::<name>` 使用。当前从字符字面量到关键字、运算符及优先级伪 token 均编码为 `isize`；例如 `identifier` 与 Go `parser.go` 都是 `57346`。Rust 关键字用原始标识符（如 `r#as`、`r#in`）或生成器规定的兼容名字表示。
- `GENERATED_MAIN_ACCEPT: i32 = i32::MIN`：接受动作哨兵。`yyParse` 在解释正负动作之前优先判断它。
- `GENERATED_MAIN_XLAT: &[(isize, usize)]`：946 个按外部 token 编号排序的 `(token, symbol_column)` 对，含 `(0, 0)` 的 EOF 映射。`generated_main_symbol` 用二分查找访问，因此排序是运行时不变量。
- `GENERATED_MAIN_SYMBOL_NAMES: &[&str]`：1705 个符号名，索引就是解析表列号；既用于诊断名称，也用于从规约左部列反查文法符号。`parser_runtime.rs` 的错误恢复还在其中查找名为 `error` 的合成符号。
- `RuleId(&'static str)` 与 `RuleId::as_str`：crate 内可见、可复制且可哈希的稳定规约标识。标识由规约签名和摘要组成，避免语法重排后仅靠数组位置定位语义动作。
- `RULE_IDS_BY_REDUCTION`：3234 个 `RuleId`，下标与生成规约编号一致；`yyParse` 用它调用 `parser_actions::apply` 和 `parser_actions::has_semantic_action`。
- `GENERATED_MAIN_REDUCTIONS: &[(usize, usize)]`：3234 个 `(lhs_symbol, rhs_component_count)`。语义动作本身不占栈位，生成逻辑 `component_count` 只统计真正语法符号。
- `GENERATED_MAIN_LEGACY_RULES: &[usize]`：3234 个一基的原文法 production 序号。`yyParse` 把该值传给 `yyLexerEx::Reduced`，维持 Go 解析器回调契约。
- `GENERATED_MAIN_ACTION_REQUIRED: &[bool]`：3234 个语义动作要求标记。运行时据此并结合 `parser_actions::has_semantic_action` 判断默认值传播是否完整；`parser_3_marks_every_generated_semantic_action_as_implemented` 验证所有要求动作的规则已有实现。
- `GENERATED_MAIN_PARSE_TABLE: &[&[(usize, i32)]]`：5683 个状态行。每行按列号排序且只保存非零单元；值为 `i32::MIN` 时接受，大于零时是“下一状态加一”，小于零时是“负的规约编号加一”，缺项等价于零/错误。

文件中没有函数级解析流程、trait、条件编译项或可变全局状态；除 `RuleId::as_str` 外均为生成的常量/静态只读数据。

## 执行流程

冻结增量新增两项 token（`result=58268`、`returnKwd=58269`）和五条规约：`DEFAULT NDVRATE`、推荐索引 `RETURN RESULT`、bare `WITH CHECK OPTION`，以及 `RETURN`/`RESULT` 的非保留关键字规则。生成表中的稳定动作 ID 分别为 `analyzeoption_default_ndvrate--04eb469da4c755b0`、`recommendindexstmt_recommend_index_return_result--9605d05bc6989d1c`、`viewcheckoption_with_check_option--2f0b08df560596bb`，由 admin/DDL 手写分片构造 AST；表项大量重排不能解释为新增执行器能力。

1. `Scanner` 等 `yyLexer` 实现向 `parser_runtime.rs::yylex1` 返回外部 token；非正值先折叠为 Rust 生成表约定的 EOF `0`。
2. `generated_main_symbol` 在 `GENERATED_MAIN_XLAT` 中二分查找，把 token 转为列号。未知 token 使用超出 `GENERATED_MAIN_SYMBOL_NAMES` 的列号，后续查表得不到动作。
3. `generated_main_action` 先按当前状态选择 `GENERATED_MAIN_PARSE_TABLE` 的行，再在稀疏行中二分查列；状态或列不存在时返回 `0`。
4. `yyParse` 解释单元值：`GENERATED_MAIN_ACCEPT` 返回成功；正值减一后得到移进状态；负值按 `-action - 1` 得到生成规约下标；零进入三阶段错误恢复。
5. 规约时，运行时从 `GENERATED_MAIN_REDUCTIONS` 取得左部列和弹栈数量，从 `RULE_IDS_BY_REDUCTION` 取得语义动作键，从 `GENERATED_MAIN_LEGACY_RULES` 取得兼容回调序号。`parser_actions::apply` 构造或更新语义值后，运行时用规约后的基状态与左部列再次查同一张表，正值减一即 goto 状态。
6. `yyLexerEx::Reduced` 如返回 `true`，解析提前以 `-1` 结束；否则新语义值压栈并继续。查不到合法规约元数据、栈不够或 goto 非正时，运行时追加内部解析错误并返回 `1`。

因此动作和 goto 共用一张稀疏表；“当前列是终结符还是非终结符”由调用阶段决定，而非由两套存储结构决定。

## 数据与状态

所有数据具有 `'static` 生命周期且只读，没有初始化顺序、运行时写入或持久化副作用。表之间存在严格的平行数组不变量：

- `RULE_IDS_BY_REDUCTION`、`GENERATED_MAIN_REDUCTIONS`、`GENERATED_MAIN_LEGACY_RULES`、`GENERATED_MAIN_ACTION_REQUIRED` 长度均为 3234，同一下标必须描述同一条生成规约。
- `GENERATED_MAIN_REDUCTIONS[i].0` 必须是 `GENERATED_MAIN_SYMBOL_NAMES` 的有效索引；其名称应等于 `main.astergram` 中 legacy production 的左部。
- `GENERATED_MAIN_REDUCTIONS[i].1` 是右部真实语法符号数，不包含嵌入的语义动作。
- `GENERATED_MAIN_XLAT` 和每个 `GENERATED_MAIN_PARSE_TABLE` 行都必须按查找键升序排列，否则二分查找会静默得到错误结果。
- 表值 `0` 保留给错误/缺项，普通移进及 goto 用“状态加一”避免占用零，规约用 `-(index + 1)`，接受使用 `i32::MIN`。

运行时解析栈、lookahead、错误恢复计数和 AST 语义值均不存放在本文件；它们由 `yyParse` 的局部变量及 `Parser.cache` 管理。生成表的内存成本随进程共享，不随连接或解析请求复制。

## 依赖与调用关系

生成链为 `grammar/main.astergram` → `parsergen::Grammar::parse` → `GeneratedParser::build` → `parsergen/generate.rs::render_compatibility_tables` → `generated/main_tables.rs`。`generate_outputs` 同时生成主表、Hint 表和 lexer token 表；`check_generated_outputs` 逐字节比较磁盘产物并报告 missing/stale。

编译链为 `pkg/parser/Cargo.toml` 的 `astersql-parser` 库 → `pkg/parser/lib.rs` → `include!("generated/main_tables.rs")`。文件自身不导入外部 crate；类型只使用 Rust 基础整数、切片和静态字符串。生成工具通过 `pkg/parser/Cargo.toml` 的 dev-dependency `astersql-parsergen = { path = "parsergen" }` 参与测试/维护，而不是运行时依赖。

消费链集中在 `pkg/parser/parser_runtime.rs`：`generated_main_symbol` 读取 XLAT，`generated_main_action` 读取稀疏表，`yySymName` 读取符号名，`yyParse` 读取接受哨兵及三组规约元数据。`pkg/parser/parser_actions/mview.rs` 等动作注册代码还读取 `RULE_IDS_BY_REDUCTION` 和 `GENERATED_MAIN_ACTION_REQUIRED` 检查覆盖情况。这些静态数据的消费关系以冻结版本的直接引用核验。

## 错误处理与边界

本文件没有可失败函数，边界行为由编码和运行时解释共同形成：未知 token、状态越界、稀疏行缺列都得到动作 `0`，进入语法错误恢复；EOF 固定映射为外部编号 `0`。`parser_3_generated_sparse_rows_preserve_explicit_columns` 验证稀疏行空洞不会误命中相邻列，`parser_3_symbol_names_match_generated_translation_table` 验证已知/未知 token 名称行为，`parser_3_generated_machine_rejects_unknown_token` 验证未知 token 最终失败。

损坏或彼此错位的生成数组不会在编译期全部暴露。`yyParse` 对规约下标、legacy 映射、RuleId、栈深和 goto 做运行时防御检查，分别报告 `invalid parser reduction`、`invalid parser rule mapping`、`invalid parser RuleId mapping`、`invalid parser stack` 或 `invalid parser goto`。正常维护不应手工修这些数组，而应修改文法/生成器并整体再生成。

边界测试还覆盖空输入可接受、`SELECT`/核心 DML/DDL 族可接受、不完整或未知输入报错、纯动作产生式不会丢失，以及 parsergen 与已提交基线的逐步 shift/reduce/accept/error 轨迹一致。

## 并发与资源生命周期

生成表是不可变 `static` 切片，可被所有解析器实例并发读取，不需要锁、原子变量、通道或任务。`RuleId` 只包装静态字符串，也不拥有堆资源。

每次解析的可变资源属于 `parser_runtime.rs::yyParse`：状态和语义值复用 `Parser.cache`，结束时清空并放回 `Parser`；lookahead 与错误恢复标记只活在一次调用中。`parser_3_reuses_stack_storage_for_long_insert` 验证长 INSERT 的第二次解析复用栈容量。该资源生命周期与本文件的数据共享相互独立：修改表规模可能影响只读二进制体积与查找成本，但不会引入每连接复制或销毁流程。

## 与 Go 版本的对应关系

两边的外部 token 契约保持一致，例如 `identifier = 57346`；Rust `GENERATED_MAIN_LEGACY_RULES` 将 parsergen 的规约顺序映射回一基的 Go/原文法规则序号，以便 `yyLexerEx::Reduced` 获得兼容参数。

布局并非逐字翻译。Go 表用 `yyTabOfs`、密集 `[]uint16` 行及零值偏移解释动作；Rust 生成器输出排序的稀疏 `(column, i32)` 行，并显式用正/负“加一编码”和 `i32::MIN` 接受哨兵。Go 使用 `yyEOFCode = 57344`，Rust 纯 parsergen 运行时将非正 token 归一到 EOF `0`，并由 XLAT 的 `(0, 0)` 表项处理。语义结果仍以相同的 shift/reduce、goto、三阶段错误恢复和 `Reduced` 回调语义对齐，但不能用 Go 数组偏移直接索引 Rust 表。

`parsergen_baseline_aster_unit_test.rs::parsergen_main_tables_match_rust_baseline` 用独立小型 LR 驱动器比较生成器与已提交 Rust 表，并以正常 SELECT、分号、空输入、不完整 SELECT 和未知 token 同时覆盖接受与拒绝路径。

## 扩展指南

新增或修改 SQL 语法时，应先改 `pkg/parser/grammar/main.astergram`，并在独立的 `pkg/parser/parser_actions/*.rs` 文件中为需要语义动作的稳定 `RuleId` 接线；不要直接编辑本文件。随后按仓库维护入口运行 `astersql-parsergen generate` 生成全部受影响产物，再用 `astersql-parsergen check` 检查漂移。若 token 集变化，还要同步检查 lexer token 生成物和 lexer 关键字映射。

安全扩展必须重点检查：

- Rust lexer 与生成表的 token 编号是否一致且 XLAT 仍严格排序（不能假定 Go/Rust 所有编号相同）；token 重编号会破坏 lexer/表协议。
- 四个规约平行数组是否等长并逐项对应；stable `RuleId` 是否唯一，legacy rule 是否仍指向正确 production。
- 新语义动作是否被 `GENERATED_MAIN_ACTION_REQUIRED` 标记且能由 `parser_actions::has_semantic_action` 找到。
- 新状态行是否保持列排序及稀疏缺项为错误的约定；不要把零编码成合法状态或规约。
- 文法冲突、二进制体积和热路径二分查找次数是否可接受；大范围新增产生式会同时增加规约元数据与状态表。

测试应放在独立文件而非本生成文件内。至少同步 `parsergen_baseline_aster_unit_test.rs` 的轨迹用例、`parser_manifest_aster_unit_test.rs` 的清单契约，以及最接近语法族的 `parser_3_aster_unit_test.rs` 或 `parser_actions/*_aster_unit_test.rs`；若改变 Go 兼容行为，还应对照 `parser.y`/`parser.go` 现有测试意图。生成文件漂移和静态 include 由 `parser_generated_sources_aster_unit_test.rs`、`parser_no_legacy_dependency_aster_unit_test.rs` 负责。

## 2026-10-25 PG scalar 01

按main.astergram新增BOOL/BOOLEAN目标规则后由astersql-parsergen generate重新生成表，配套手写AST动作。

## 2026-10-26 PG scalar 03

为限定bool↔text转换的TEXT目标增加一条PgCastType产生式后，由astersql-parsergen generate整体重生成；动作稳定ID为pgcasttype_text--298413db6f8b6803。未新增token。配套parser_test::pg_scalar_03_text_cast_target覆盖后缀/CAST及MySQL隔离，parser_manifest检查元数据映射，parsergen check检查生成一致性。

## PostgreSQL signed16（任务5）

按用户授权为PgCastType增加SMALLINT与INT2两条产生式，使用astersql-parsergen generate生成，动作由expression.rs按稳定RuleId承接；未增加token。

## PostgreSQL 有限 numeric 入口

由 main.astergram 重新生成，增加裸 NUMERIC 目标和仅用于后缀的 DECIMAL 目标；CAST AS DECIMAL 继续复用原 FloatOpt 规则，避免重复归约。

## PostgreSQL numeric 声明与舍入

为numeric声明增加PgNumericOpt：裸类型、precision、precision加正/负scale；生成表由astersql-parsergen generate生成，未新增token。

批次15：生成表增加FixedPointType负scale声明规则，供PG正式CREATE TABLE使用，MySQL在动作阶段拒绝。


## PostgreSQL float4 文本输入（任务16）

由 astersql-parsergen generate 从主文法重新生成，新增 PgCastType FLOAT4 规则及对应稳定 RuleId；关联语义动作构造 TypeFloat/PostgreSQLFloat4，供正式参数表达式使用。

## PostgreSQL 浮点宽度与文本（任务18）

由 `cargo run --locked --release -p astersql-parsergen -- generate` 从 main.astergram 重新生成。新增 float8、double precision 目标及仅用于后缀的 PgPostfixFloat（FLOAT FloatOpt / REAL），CAST仍使用原有FLOAT/REAL动作，避免重复归约；没有新增token。目标宽度、精度拒绝和推导参数描述由独立pg_scalar_18正式入口回归覆盖。

## 2026-10-26 文本族身份边界（任务21）

由 main.astergram 的三个最小文本类型产生式重生成：无长度 VARCHAR 的 CAST 与后缀 CHAR；语义动作由 parser_actions/expression.rs 注册，MySQL 方言仍拒绝新增 PG CAST 目标。

## JSON显式转换与类型交接

从main.astergram增加PgPostfixJson后生成语法表；复用JSON CAST动作，允许PostgreSQL ::json。


## int4 数组共享求值与协议

生成输入加入 PgCastType : PgCastType [ ] 规则，由 expression::PgArrayType 消费其稳定 RuleId；重复后缀不建立不同数组类型身份。表通过 astersql-parsergen generate 再生，未手工修改自动机。


## float4数组声明

由main.astergram的PgCastType : PgPostfixFloat [ ]生成，使既有REAL/FLOAT(p)标量声明也能进入同一数组元素身份分派；没有手改状态表。float4本族接受，其他尚未开放族仍由现有语义门禁拒绝。使用astersql-parsergen generate/check核对生成产物。

## bpchar数组长度语法接线（任务34）

main.astergram新增PgCastType : PgPostfixText [ ]，让现有CHAR/CHARACTER(n)标量声明进入同一数组动作。main_tables由astersql-parsergen generate再生，使用check核对；不手工改自动机。


## date数组逐族接入（任务35）

main.astergram新增PgCastType : PgPostfixDate [ ]，通过parsergen generate再生；不手工修改自动机。独立测试使用pg05_array_date前缀，真实18.6差分见docs/postgresql-pg05-array-date-evidence.md。

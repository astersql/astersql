# `pkg/parser/parser_contract_cases.rs`

## 文件定位

`parser_contract_cases.rs` 属于 `astersql-parser` crate，但不是解析器的生产入口。`pkg/parser/lib.rs` 仅在 `#[cfg(test)]` 下通过 `#[path = "parser_contract_cases.rs"] mod parser_contract_cases;` 装配它，因此这些符号只进入测试构建。文件集中保存 Rust 解析器测试从 Go `pkg/parser/parser_test.go` 迁移而来的表驱动 SQL 合约数据，直接消费者是同目录的 `parser_test.rs` 和独立回归测试 `parser_contract_cases_test.rs`。

crate 边界由 `pkg/parser/Cargo.toml` 确定：包名为 `astersql-parser`，库入口为 `lib.rs`。本文件自身不导入外部 crate；它把测试数据交给 `parser_test.rs`，后者再调用 Rust 解析器及 `parser-ast` 检查结果。

## 核心职责

- `ParserContractCase` 统一表达一条 SQL 的输入文本、预期解析成败和成功时用于二次解析或专项恢复校验的文本。
- `parser_contract_cases` 以 Go 测试函数名作为稳定键，返回该主题的合约用例。主题覆盖普通 DML/DDL、表达式、权限、CTE、分区、窗口函数、Traffic 等语法面。
- `parser_contract_strings` 保存不适合 `成功/失败/恢复文本` 三元组的字符串列表：保留字、非保留字、TABLESAMPLE 补充输入以及有符号 `int64` 越界输入。
- `accepts` 与 `rejects` 降低静态表的重复书写：前者令 `restored == sql`，后者令 `ok == false` 且恢复文本为空。

这里的“合约”是测试输入契约，而不是运行时 API。除 Traffic 专项外，通用运行器只比较原始 SQL 与 `restored` 再次解析后的顶层 AST 数量和节点动态类型，并不调用 AST 的规范化 Restore；因此 `restored` 字段不能一概理解为规范化 SQL 输出。

## 主要符号

- `pub struct ParserContractCase { sql: &'static str, ok: bool, restored: &'static str }`：可复制的只读用例记录。三个字段公开，是因为兄弟测试模块直接读取它们；`Clone + Copy + Debug` 方便按值使用和断言诊断。
- `const fn accepts(sql: &'static str) -> ParserContractCase`：构造成功用例，复用输入作为二次解析目标。它是文件私有的常量函数，只服务静态切片字面量。
- `const fn rejects(sql: &'static str) -> ParserContractCase`：构造失败用例，恢复目标固定为空串；消费者在 `ok == false` 时必须跳过恢复阶段。
- `pub fn parser_contract_cases(name: &str) -> Vec<ParserContractCase>`：按测试名选择静态切片，再以 `to_vec()` 返回拥有所有权的副本。当前匹配表包含多个测试主题；同一用例可被普通合约运行器或 Traffic 专项运行器消费。
- `pub fn parser_contract_strings(name: &str, variable: &str) -> &'static [&'static str]`：按 `(Go 测试名, Go 局部变量名)` 选择静态字符串切片，不发生堆分配。

公开性仅限 crate 的测试模块可见效果：父模块 `parser_contract_cases` 本身在 `lib.rs` 中不是 `pub mod`，并且受 `cfg(test)` 限制，不构成 `astersql-parser` 对下游 crate 的公开 API。

## 执行流程

通用表驱动路径位于 `parser_test.rs::run_contract_table_with_options`：

1. 测试函数传入与 Go 测试一致的名称；宏 `contract_table_tests!` 为一批主题生成 Rust `#[test]` 函数，其他复杂主题在各自测试函数中直接调用运行器。
2. `parser_contract_cases(name)` 的 `match` 选择静态切片，并复制为 `Vec`。
3. 运行器为每个用例新建 `parser::Parser`，应用窗口函数与 MariaDB 模式开关，再执行 `Parse(case.sql, "", "")`。
4. 首先比较解析结果是否与 `case.ok` 一致；失败用例到此结束。
5. 成功用例取出原 AST，再解析 `case.restored`，比较两次结果的语句数，并逐项比较顶层 AST 节点的动态类型。

专用途径补充了通用检查：`parser_test.rs::test_traffic_stmt` 对 25 个 Traffic 用例使用 `ParseOneStmt`，向下转型为 `parser_ast::TrafficStmt`，检查 Capture/Replay 的目录，并用 `restore_traffic` 比较精确恢复文本；`test_simple`、`test_table_sample` 和 `test_signed_int64_out_of_range` 则消费 `parser_contract_strings`，分别检查关键字可解析、TABLESAMPLE 补充语法可解析以及错误消息包含 `out of range`。

`parser_contract_cases_test.rs::go_contract_tables_are_not_reduced_to_smoke_cases` 不解析 SQL，而是守护三个关键数据集的形状：RecommendIndex 为 12 项、Traffic 为 25 项，并精确比对 4 条越界整数输入。

## 数据与状态

所有 SQL 和恢复文本均为 `&'static str`，数据编译进测试二进制且运行期间不可变。文件没有全局可变状态、缓存、环境变量或配置读取。

`parser_contract_cases` 每次调用都会从静态切片克隆元素并分配一个 `Vec`；由于 `ParserContractCase` 只含两个静态字符串引用和一个布尔值，复制不复制字符串内容。`parser_contract_strings` 直接借用静态切片，不分配。当前测试规模较小，这一所有权选择主要换取调用端简单的按值迭代。

布尔值和恢复文本存在配套不变量：`rejects` 产生的空恢复文本只在失败分支出现，通用运行器在访问恢复文本前以 `if !case.ok { continue; }` 截断；Traffic 测试也使用相同保护。手写 `ParserContractCase` 主要用于输入与期望恢复文本不同的 Traffic 成功用例。

## 依赖与调用关系

上游装配与调用边如下：

- `pkg/parser/lib.rs` → `parser_contract_cases`：仅测试配置下声明模块，同时声明独立的 `parser_contract_cases_test` 和 `parser_test`。
- `pkg/parser/parser_test.rs::run_contract_table_with_options` → `parser_contract_cases`：大多数主题的公共执行入口。
- `pkg/parser/parser_test.rs::{test_traffic_stmt,test_simple,test_table_sample,test_signed_int64_out_of_range}` → 两个数据查询函数：执行专项 AST、字符串恢复或错误语义检查。
- `pkg/parser/parser_contract_cases_test.rs::go_contract_tables_are_not_reduced_to_smoke_cases` → 两个数据查询函数：检查关键迁移表没有被进一步缩减。

下游依赖只有标准库数据结构和切片操作；本文件不直接调用解析器。真正的语法处理位于 `parser::New`、`Parser::Parse`、`Parser::ParseOneStmt` 以及 `parser-ast` 节点实现中，它们由 `parser_test.rs` 调用。因此修改本文件会改变测试覆盖和预期，不会改变发布库的解析行为。

RustCodeGraph 的名称查询识别出 `accepts`、`rejects`、`parser_contract_cases`、`parser_contract_strings` 和独立回归测试；文件过滤与调用方查询未给出可用调用边，所以调用关系又以 `rg` 和模块/测试源码直接核验。没有证据表明生产模块调用这些函数。

## 错误处理与边界

两个查询函数都采用封闭映射：未知测试名会在 `parser_contract_cases` 中触发 `panic!("missing Rust parser contract cases for {other}")`；未知 `(name, variable)` 会在 `parser_contract_strings` 中触发包含两个键的 panic。这样可以让拼写错误或新增测试忘记登记数据时立即失败，但它们不是返回 `Result` 的可恢复接口。

失败用例只声明“应解析失败”，通用运行器不约束具体错误种类或消息；唯一例外是 `TestSignedInt64OutOfRange` 字符串表，其消费者明确要求消息包含 `out of range`。成功用例的通用验证只保证可解析以及恢复目标解析出相同数量、相同顶层节点类型，不比较完整 AST 字段，也不验证标准 Restore 输出。Traffic 专项提供更强的字段和恢复文本验证。

`&'static str` 约束意味着调用者不能动态注入临时字符串；这是静态测试表的有意边界。返回 `Vec` 意味着调用者可以重排或删改自己的用例副本，但不能改变源静态表。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、文件句柄、网络连接或事务。静态字符串和切片生命周期覆盖整个测试进程，`Vec<ParserContractCase>` 在单次查询中分配并由调用测试作用域释放。

数据本身只读，因此多个测试线程可以安全并发查询。解析器实例的生命周期由消费者控制：通用运行器每个用例创建一个新实例，Traffic 和字符串专项测试则各自按需要创建实例；本文件不共享解析器状态。MariaDB 与窗口函数选项也由运行器设置，不存放在用例记录内。

## 与 Go 版本的对应关系

`pkg/parser/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/parser"` 明确 crate 的 Go 来源。映射键沿用 `pkg/parser/parser_test.go` 的函数名及局部变量名，使迁移审阅可以按名称对照。

当前对应程度并不统一：

- `TestRecommendIndex` 的 12 条输入与 Go 表数量及输入一致；Rust 通用运行器对这些条目主要验证解析与顶层 AST 类型等价，而 Go `RunTest` 还比较规范恢复结果。
- `TestTrafficStmt` 的 25 个正反例及恢复字符串与 Go 表直接对应；Rust 的专项测试也检查 `TrafficStmt` 类型、目录和恢复文本，是本文件中最完整的对齐组之一。
- `TestSignedInt64OutOfRange.cases` 保留 Go 的 4 条输入，Rust 消费者与 Go 一样要求越界错误语义。
- `TestSimple.reservedKws` 和 `unreservedKws` 目前分别只有 3 个代表词，而 Go 列表包含大量关键字；`TestTableSample.cases` 也只是 Go 补充表达式列表的子集。
- 许多 `parser_contract_cases` 分支只有一至三条代表性 SQL，而同名 Go 测试通常包含更大的正反例表和更细的 AST/恢复断言。它们应描述为 Rust 自有合约烟雾覆盖，不能据此宣称同名 Go 测试已经完整迁移。

因此维护时应以 Go 测试的实际增量为依据：对齐新增语法时既要补输入，也要判断是否需要在 `parser_test.rs` 增加 AST 字段、恢复结果或错误文本断言，不能只增加一个可解析样例。

## 扩展指南

新增或扩展解析合约时，应按以下接入点选择修改：

1. 普通解析正反例：在 `parser_contract_cases` 中新增/扩展测试名分支，并在 `parser_test.rs` 通过现有宏或明确的测试函数调用它。输入与二次解析目标相同时用 `accepts`，预期拒绝时用 `rejects`；只有期望文本确实不同才手写结构体。
2. 纯字符串列表或需要专项断言的数据：在 `parser_contract_strings` 增加明确的 `(name, variable)` 键，并在独立测试逻辑中定义错误、AST 或恢复语义。不要把需要精确错误检查的场景降级为普通 `ok: false`。
3. MariaDB、窗口函数等模式敏感语法：由调用端选择 `run_contract_table` 的模式参数或 `run_contract_table_with_options`；不要在静态数据函数中引入全局模式状态。
4. 与 Go 提交对齐：先核对 `pkg/parser/parser_test.go` 的同名函数，保持输入、正反例与恢复预期；若 Rust 当前解析能力只支持子集，应明确记录缺口，而不是改写 Go 意图。
5. 测试文件必须保持独立：更新 `pkg/parser/parser_contract_cases_test.rs` 中的规模/内容护栏，必要时更新 `pkg/parser/parser_test.rs` 的行为断言，不要把 `#[test]` 嵌入本数据文件。

兼容风险主要是测试键重命名导致 panic、成功/失败标记错误掩盖或制造回归，以及用 `accepts` 错把规范恢复预期等同于原输入。性能风险很低，但若数据表大幅增长，可评估让 `parser_contract_cases` 返回静态切片以避免每次 `to_vec()` 分配；这会改变调用接口，应同步所有消费者。

## 验证依据

- 源文件：`pkg/parser/parser_contract_cases.rs`；核对 `ParserContractCase`、两个构造函数、两个公开查询函数、全部匹配分支及 panic 边界。
- crate 与装配：`pkg/parser/Cargo.toml`、`pkg/parser/lib.rs`；确认 `astersql-parser` 边界、Go 包元数据以及该模块仅受 `#[cfg(test)]` 装配。
- Rust 消费者：`pkg/parser/parser_test.rs`；核对通用运行器、生成测试宏、模式开关、Traffic 专项恢复、关键字/TABLESAMPLE 字符串消费和越界错误检查。
- 独立 Rust 测试：`pkg/parser/parser_contract_cases_test.rs`；核对 12/25 项数量护栏与 4 条越界输入的精确列表。
- Go 对照：`pkg/parser/parser_test.go::{TestSimple,TestRecommendIndex,TestTableSample,TestSignedInt64OutOfRange,TestTrafficStmt}`，并抽查同文件其他同名表测试；确认完整对齐组和代表性子集之间的差异。
- RustCodeGraph：`status` 显示索引有效；`query parser_contract_cases --kind function` 和 `query parser_contract_strings --kind function` 找到本文件符号及独立回归测试。`files --filter pkg/parser/parser_contract_cases` 未返回文件、调用方命令未提供可用边，因此调用边以模块声明和 `rg` 结果补证，没有把缺失图边当作不存在调用的唯一依据。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终结构检查应确认目标文件存在且固定二级标题恰好为 11 个。

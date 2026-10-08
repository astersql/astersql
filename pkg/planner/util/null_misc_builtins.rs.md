# `pkg/planner/util/null_misc_builtins.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-util`（见 `pkg/planner/util/Cargo.toml`），由 crate 根模块 `pkg/planner/util/lib.rs` 以私有模块 `mod null_misc_builtins;` 装配。它不直接对下游 crate 再导出 API，而是为同 crate 的 `pkg/planner/util/null_misc.rs` 提供 NULL 拒绝（null-reject）证明所需的 builtin 属性元数据。

在规划链路中，公开入口是 `null_misc.rs::IsNullRejected`，本文件位于其证明引擎的下游。该入口目前由 `pkg/planner/util/funcdep_misc.rs::ExtractNotNullFromConds`、`pkg/planner/core/operator/logicalop/logical_projection.rs::ExtractFD` 和 `pkg/planner/core/operator/logicalop/logical_join.rs::PredicatePushDown` 调用，因此这里的登记结果会间接影响函数依赖推导，以及投影和连接谓词的 NULL 拒绝判断。

## 核心职责

本文件只描述属性，不执行表达式求值，职责有三项：

1. 用 `NULL_REJECT_NULL_PRESERVING_FUNCTIONS` 声明“任一参数必为 NULL 时，函数结果也必为 NULL”的函数名集合。
2. 用 `NULL_REJECT_REJECT_NULL_TESTS` 区分测试类函数在 NULL 输入下是返回确定的 `FALSE`，还是继续返回 `NULL`。
3. 用 `is_null_reject_null_preserving` 和 `null_reject_test_mode` 向证明引擎提供只读查表接口。

该登记表是一份优化器证明白名单。未登记并不表示函数一定不传播 NULL，而是表示当前证明器不会仅凭本表对它作出该结论；这种保守行为避免把不能证明的表达式错误地当成 NULL 拒绝谓词。

## 主要符号

- `pub enum NullRejectTestMode`：测试类 builtin 的 NULL 结果模式。`ReturnsFalse` 表示 NULL 输入得到非 NULL 的 `FALSE`；`KeepsNull` 表示结果仍为 NULL。它派生 `Clone`、`Copy`、`PartialEq`、`Eq`，便于静态表保存并按值返回。
- `pub static NULL_REJECT_NULL_PRESERVING_FUNCTIONS: &[&str]`：174 个不重复函数名组成的不可变切片。多数元素直接引用 `parser_ast::functions` 常量；`date`、`day`、`hour`、`minute`、`month`、`quarter`、`second`、`time`、`year` 暂以字符串字面量表示，因为注释明确记录 Rust AST 表面尚缺对应常量。
- `pub static NULL_REJECT_REJECT_NULL_TESTS: &[(&str, NullRejectTestMode)]`：三项测试语义映射。`IsTruthWithoutNull` 和 `IsFalsity` 对应 `ReturnsFalse`，`IsTruthWithNull` 对应 `KeepsNull`。
- `pub fn is_null_reject_null_preserving(name: &str) -> bool`：对 174 项切片做精确、区分大小写的线性查找。
- `pub fn null_reject_test_mode(name: &str) -> Option<NullRejectTestMode>`：线性查找三项模式表，命中时复制枚举值，未命中返回 `None`。

这些符号虽声明为 `pub`，但所在模块没有从 `lib.rs` 再导出且模块本身为私有；其实际可见用途是 crate 内部的 `null_misc.rs` 与同 crate 独立测试。

## 执行流程

正常证明路径如下：

1. 上层规划器调用 `null_misc.rs::IsNullRejected`；该函数先下推 `NOT`，再进入递归证明 `proveNullRejected`。
2. 遇到标量函数时，`proveNullRejectedScalarFunc` 先处理 `AND`、`OR`、`NOT`、`IN`、`IS NULL`、`WEEK`/`YEARWEEK` 等具有专门规则的函数。
3. 对测试类函数，`proveNullRejectedScalarFunc` 调用 `null_reject_test_mode`。只有子表达式被证明 `must_null` 时，结果才被判为 `non_true`；模式为 `KeepsNull` 时才同时保留 `must_null`，`ReturnsFalse` 则只保留“非真”。
4. 对普通函数，证明器调用 `is_null_reject_null_preserving`。函数在表内且任一参数被证明 `must_null` 时，函数结果被证明同时满足 `non_true` 和 `must_null`。
5. 常量折叠路径 `tryFoldNullifiedScalarFunc` 也调用 `is_null_reject_null_preserving`：参数全部尝试置换/折叠后，只要出现 NULL 且函数在表内，直接构造 `expression::NewNull()`；否则继续交给 `foldNullifiedFunction`。

因此同一份白名单同时约束递归逻辑证明和“内表列置 NULL”后的常量折叠，修改它必须同时考虑两条消费路径。

## 数据与状态

所有数据都是进程生命周期内的只读静态数据：一个 `&[&str]` 和一个 `&[(&str, NullRejectTestMode)]`。查询函数不缓存、不分配集合、不修改全局状态；`is_null_reject_null_preserving` 返回布尔值，`null_reject_test_mode` 返回可复制的小枚举。

函数身份使用 `expression::ScalarFunction.FuncName.L` 的规范化字符串，与 `parser_ast::functions` 中的名字比较。这一约定要求生产者和登记表使用同一规范化命名；查找本身不做小写化、别名展开或输入校验。174 项表采用线性扫描，时间复杂度为 O(n)，三项模式表同理但规模固定很小；当前实现以静态、无初始化开销为优先，没有哈希表状态。

## 依赖与调用关系

直接依赖只有 `parser_ast::functions`，其 Cargo 依赖由 `pkg/planner/util/Cargo.toml` 中的 `parser-ast = { package = "astersql-parser-ast", path = "../../parser/ast" }` 提供。本文件自身不依赖表达式、会话或计划上下文类型。

RustCodeGraph 给出的直接调用边为：

- `null_misc.rs::proveNullRejectedScalarFunc -> null_reject_test_mode`；
- `null_misc.rs::proveNullRejectedScalarFunc -> is_null_reject_null_preserving`；
- `null_misc.rs::tryFoldNullifiedScalarFunc -> is_null_reject_null_preserving`；
- `null_misc_test.rs::test_null_reject_builtin_registry_snapshot` 直接覆盖两张表及两个查询函数。

上游主链可概括为 `PredicatePushDown` / `ExtractFD` / `ExtractNotNullFromConds -> IsNullRejected -> proveNullRejected -> proveNullRejectedScalarFunc`。本文件没有调用下游函数；其查询实现只使用切片迭代器的 `any`、`find` 和 `map`。

## 错误处理与边界

本文件没有 `Result`、异常或日志路径。未知函数名的边界行为是保守失败：普通属性查询返回 `false`，测试模式查询返回 `None`，调用方随后不给出基于登记表的 NULL 拒绝证明。

需要特别区分以下边界：

- 表内函数必须满足“任一参数 NULL 即结果 NULL”；存在忽略某些 NULL 参数、按参数位置决定语义或具有特殊控制流的函数不能仅因常见输入行为而加入。例如 `null_misc.rs` 对 `WEEK`/`YEARWEEK` 单独处理日期参数，因为 mode 参数为 NULL 时按 0 处理。
- `IS NULL`、`AND`、`OR`、`NOT`、`IN` 等由证明器专门处理；本表不能替代它们的三值逻辑规则。
- 字符串字面量占位与 AST 常量必须保持完全相同的运行时名称，否则查询会静默未命中。
- 重复条目不会改变查询结果，却会造成清单漂移；`null_misc_test.rs::test_null_reject_builtin_registry_snapshot` 通过排序、去重和长度比较防止重复。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、I/O 或显式资源释放。两张表在程序装载后保持不可变，所有查询只借用 `&str` 并返回值类型，可被多个规划线程并发读取，不存在本文件内部的数据竞争或生命周期协调。

查询不会持有调用方表达式或上下文，也不会延长任何计划对象生命周期。唯一需要关注的资源维度是热路径 CPU 成本：174 项线性比较可能在大量标量表达式证明中重复发生；若未来确有性能证据再改为更快结构，必须避免引入运行时初始化锁或改变确定性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/null_misc_builtins.go`。Rust 基本逐项保留 Go 的两个概念：

- Go 的 `nullRejectTestMode uint8` 与两个常量，对应 Rust 的 `NullRejectTestMode::{ReturnsFalse, KeepsNull}`。
- Go 的 `map[string]struct{}` 白名单，对应 Rust 的 `&[&str]`；Go 以 map 查找，Rust 通过 `is_null_reject_null_preserving` 线性查找。
- Go 的 `map[string]nullRejectTestMode`，对应 Rust 的元组切片及 `null_reject_test_mode`。

语义上的三项模式映射一致，NULL 传递清单也按 Go AST 名称迁移。当前 Rust AST 缺少九个日期时间名称常量，所以 Rust 用同值字符串占位；这是表示形式差异，不是有意删减 Go 行为。Go 证明代码直接索引 map，Rust `null_misc.rs` 使用两个查询函数封装相同决策。

测试覆盖尚不完全对称：Rust 的 `pkg/planner/util/null_misc_test.rs` 固定断言白名单长度为 174、测试表长度为 3、白名单无重复，并抽查 `GT`、`Cast` 和两个 truth 模式；同文件还通过 `test_is_null_rejected_proof_modes` 覆盖比较、IS NULL、NOT、AND/OR、IN 等消费行为。Go 的 `pkg/planner/util/null_misc_test.go::TestIsNullRejectedProofModes` 覆盖更广的 builtin/边界组合，并校验登记名称与 builtin 清单，因此扩展 Rust 清单时应同步检查两端，而不能只修改长度快照。

## 扩展指南

新增或调整 builtin NULL 属性时，应按以下顺序处理：

1. 先从真实 builtin 求值语义确认每个参数位置的 NULL 行为；只有“任一参数 NULL 都必然返回 NULL”的函数才能加入 `NULL_REJECT_NULL_PRESERVING_FUNCTIONS`。参数位置特殊或具有控制流的函数应在 `null_misc.rs::proveNullRejectedScalarFunc` / `tryFoldNullifiedScalarFunc` 中建立专门规则。
2. 优先使用 `parser_ast::functions` 常量。只有 AST 确实没有对应符号时才使用精确字符串，并留下缺口说明；AST 补齐后应替换占位字面量。
3. 若新增的是 IS TRUE / IS FALSE 风格测试，加入 `NULL_REJECT_REJECT_NULL_TESTS` 并明确选择 `ReturnsFalse` 或 `KeepsNull`；该选择会改变 `must_null` 的传播，不能只根据 `non_true` 结果决定。
4. 更新独立测试 `pkg/planner/util/null_misc_test.rs`，调整长度快照并增加命中、未命中和实际 `IsNullRejected` 行为用例。测试逻辑不要内嵌到生产文件。还应对照 `pkg/planner/util/null_misc_test.go` 的同类用例，保持 Go/Rust 证明意图一致。
5. 兼容性风险主要是错误优化：误加函数可能把外连接等价性判断变得不可靠；漏加函数通常只损失优化机会。性能风险来自扩大线性表后的查找成本。任何数据结构替换都应保持未知项的保守返回值和静态只读特性。

## 验证依据

- 目标源码：`pkg/planner/util/null_misc_builtins.rs`，RustCodeGraph 显示 234 行、4 个顶层业务符号类别，并显示它被 `null_misc.rs` 和 `null_misc_test.rs` 使用。
- crate 边界：`pkg/planner/util/Cargo.toml` 的包名、`parser-ast` 路径依赖与 `[package.metadata.porting] go-package = "pkg/planner/util"`；`pkg/planner/util/lib.rs` 的私有模块声明、`null_misc` 公开再导出及独立测试接线。
- 调用证据：RustCodeGraph `node is_null_reject_null_preserving`、`node null_reject_test_mode`、`node proveNullRejectedScalarFunc`、`node tryFoldNullifiedScalarFunc`、`node IsNullRejected`，以及对目标符号的 `query`/`explore`；另以 `rg` 核对三个生产调用点和所有直接表引用。
- Go 对照：`pkg/planner/util/null_misc_builtins.go`、`pkg/planner/util/null_misc.go`、`pkg/planner/util/null_misc_test.go`。
- Rust 测试：`pkg/planner/util/null_misc_test.rs::test_null_reject_builtin_registry_snapshot` 与 `test_is_null_rejected_proof_modes`。前者验证 174/3 项、去重和代表性查询；后者验证登记表进入完整 NULL 拒绝证明后的行为。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并人工复核本文未把未知函数的保守未命中描述成运行时错误，也未把尚缺 Rust AST 常量的占位字符串描述成已完成枚举接线。

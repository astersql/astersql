# [`pkg/planner/core/fulltext_to_like.rs`](fulltext_to_like.rs)

## 文件定位

本文件属于 `astersql-planner-core` crate（`pkg/planner/core/Cargo.toml`），是 planner 到 expression 共享全文检索回退构造器之间的薄适配层。`pkg/planner/core/lib.rs` 以私有模块 `mod fulltext_to_like` 装入文件，再通过 `pub use fulltext_to_like::*` 把其唯一函数提升到 crate 根 API。目标包没有 `doc.go`，因此 crate 清单、模块入口和相邻实现是本文件边界的直接依据。

文件意图是对应 Go `expressionRewriter.convertMatchAgainstToLike`：接收规划上下文中已经准备好的列表达式、搜索文本和 modifier，转交 `expression_dependency::BuildFTSToILikeExpression` 构造 ILIKE 谓词树。它不负责决定何时使用回退，也不自行解析搜索文本。

当前 Rust 接线需要特别区分“公开能力”和“生产调用”。RustCodeGraph 显示本文件只被 `pkg/planner/core/fulltext_to_like_test.rs` 使用，仓库搜索也未找到其它直接调用；实际生产重写链 `expression_rewriter.rs::matchAgainstToLike` 调用的是该文件内另一个同名私有 `convertMatchAgainstToLike`。因此可以确认本文件已被 crate 导出且有独立测试，但不能声称它当前已进入生产重写主链。

## 核心职责

本文件只承担无损委托：`convertMatchAgainstToLike` 把四个参数原样传给 expression crate 的 `BuildFTSToILikeExpression`，并把其 `Result<ExprBox, Error>` 原样返回。

搜索语法校验、modifier 分支、ILIKE pattern 转义、跨列/跨词的 DNF 或 CNF 组合、空搜索串处理、NULL 列值归零以及具体错误生成都由 `pkg/expression/planner_bridge.rs::BuildFTSToILikeExpression` 负责。本文件不重复这些规则，目的是避免 planner 回退与选择率估算使用的共享表达式构造语义分叉。

它也不负责上游安全条件。直接布尔上下文判定、原生 TiFlash FTS 可行性、常量搜索串要求、计划缓存禁用、列类型检查、搜索值求值及 NULL 快速路径都位于 `pkg/planner/core/expression_rewriter.rs`。调用者若绕过这些前置步骤直接使用本公开函数，只会获得共享构造器自身提供的校验。

## 主要符号

- `pub fn convertMatchAgainstToLike(context, columns, search_text, modifier) -> Result<ExprBox, Error>`：文件唯一的模块级符号和公开 API；没有模块级常量、类型、trait、`impl` 或条件编译项。
- `context: &dyn expression_dependency::BuildContext`：expression 函数创建、类型和求值相关操作所需的构建上下文；只在调用期间借用。
- `columns: Vec<expression_dependency::ExprBox>`：被 MATCH 的列表达式集合，按值移交下游；本文件不检查为空、列类型或列来源。
- `search_text: String`：已经由上游取得的搜索文本，按值移交；本文件不处理 NULL，因为参数类型已经是非空 `String`。
- `modifier: u8`：保持 parser modifier 的位布局；低四位表示搜索模式，第 4 位表示 WITH QUERY EXPANSION。其合法性由下游解释。
- `expression_dependency::BuildFTSToILikeExpression`：唯一被调用的下游符号，成功时返回拥有所有权的 boxed expression，失败时返回 expression crate 的错误。

## 执行流程

函数执行没有本地分支或循环：

1. 调用者传入 `BuildContext` 引用、列表达式向量、拥有所有权的搜索字符串和 modifier 字节。
2. 函数以相同顺序调用 `expression_dependency::BuildFTSToILikeExpression(context, columns, search_text, modifier)`；向量和字符串在此被移动。
3. 不对成功表达式或错误做包装、映射、日志记录或恢复，直接把下游 `Result` 返回调用者。

在完整设计中的预期上游流程可由相邻重写器说明：`matchAgainstToExpression` 只在 alternative-plan 的 LIKE fallback 轮次且处于直接布尔谓词上下文时进入 `matchAgainstToLike`；后者验证搜索参数是常量、必要时禁止计划缓存、验证列为字符串、处理 NULL、校验受支持的 token 子集，然后调用转换器。当前生产代码最后一步仍使用 `expression_rewriter.rs` 内的私有同名函数，并非本文件函数。

共享构造器的直接下游流程是：拒绝空列和 WITH QUERY EXPANSION，验证 token；空白搜索返回有符号零；自然语言模式对“每列 × 每词”的 `IFNULL(column ILIKE '%term%', 0)` 作 OR；布尔模式把 `+` 必含词、`-` 排除词和可选词组合为 CNF/DNF；未知 mode 返回不支持错误。这些是本函数委托后的行为，不是本文件自行实现的逻辑。

## 数据与状态

本文件没有全局变量、缓存或持久状态。`context` 是不可变 trait object 引用；`columns` 和 `search_text` 的所有权被移入下游；成功返回的 `ExprBox` 由调用者拥有，错误也按值返回。

关键不变量是参数和值语义不被适配层改写：列顺序、表达式对象、搜索文本内容和 modifier 位必须完整到达共享构造器，且下游错误类型保持不变。独立测试以“无列时报错”“WITH QUERY EXPANSION 报错”“空搜索串成功”三个不同分支证明了这一委托关系。

本函数的 `String` 参数无法表达 SQL NULL。NULL 搜索值必须在上游 `matchAgainstToLike` 中先转换为 `Constant(NULL)`；若未来直接从其它路径调用本函数，调用者仍须自行保留 SQL 三值逻辑。返回表达式的内部树、常量与函数签名状态均由 expression crate 拥有和管理。

## 依赖与调用关系

`pkg/planner/core/Cargo.toml` 将 `expression-dependency` 映射到本地 crate `astersql-expression`（路径 `../../expression`）。本文件全部参数类型、返回类型和唯一函数调用都经该依赖进入；没有使用 `nextgen` feature，也没有其它直接依赖。

模块装配关系是 `pkg/planner/core/lib.rs` 的 `mod fulltext_to_like` 加 `pub use fulltext_to_like::*`。测试装配则在同一 `lib.rs` 的 `#[cfg(test)] #[path = "fulltext_to_like_test.rs"] mod fulltext_to_like_test`，测试通过 `super::fulltext_to_like::convertMatchAgainstToLike` 直接调用目标符号。

设计上的上游链为 `matchAgainstToExpression -> matchAgainstToLike -> convertMatchAgainstToLike`，下游链为 `BuildFTSToILikeExpression -> NewFunction / ComposeDNFCondition / ComposeCNFCondition`。但当前 Rust 生产链的转换器节点定义在 `expression_rewriter.rs`，它直接调用 `expression::BuildFTSToILikeExpression`；目标文件函数目前只在测试中形成直接调用边。两份 Rust 转换器的函数体语义相同，但重复接线是后续维护时需要关注的漂移风险。

共享构造器还被 `BuildFTSToILikeExpressionFromBuiltin` 使用，以便选择率估算将单列 FTS builtin 替换为同族 ILIKE 谓词。这解释了转换逻辑下沉到 expression crate 的原因，但该估算入口并不调用本文件。

## 错误处理与边界

本文件没有本地错误分支，所有错误均由 `BuildFTSToILikeExpression` 原样传播。当前下游明确拒绝：空列集合、WITH QUERY EXPANSION、不受支持的 token、非自然语言/布尔模式的 modifier，以及构造 ILIKE/IFNULL/NOT 或条件组合时产生的 expression 错误。

独立 Rust 测试固定了两条错误边界：空列错误文本包含 `no columns`；查询扩展错误文本包含 `WITH QUERY EXPANSION`。测试还确认空字符串在至少一列时成功，下游将其处理为零表达式。适配层不应把这些错误吞掉、替换为通用错误或改为 SQL 字符串拼接。

更早的 planner 边界不在此函数内：非字符串列、非常量搜索表达式、搜索求值失败、SQL NULL 和计划缓存可变参数由 `expression_rewriter.rs::matchAgainstToLike` 处理。调用本函数本身也不会验证当前语境是否为直接布尔谓词位置；在评分或标量比较位置错误使用 ILIKE 回退会把浮点相关度降为 0/1，造成语义错误。

共享 ILIKE 回退本质上是受限近似：没有 MySQL FTS 相关度、停用词、最短词长和词边界语义，且 substring ILIKE 不能利用全文索引。下游通过限制 token 子集避免部分不可等价输入，但这些已知差异仍决定了本 API 只能用于规划器明确选择的布尔回退路径。

## 并发与资源生命周期

本函数同步执行，不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源，也没有跨调用共享的可变状态。并发调用之间的隔离取决于各自输入以及 `BuildContext` 实现；本文件既不增加同步，也不声明额外的 `Send`/`Sync` 保证。

`context` 只借用到函数及下游调用返回为止。`columns`、`search_text` 被移动，避免本层复制；共享构造器会为 pattern 创建字符串并构造新的表达式节点。其工作量随列数与 token 数的乘积增长：自然语言和布尔分支都可能为每个“列 × 词”生成 ILIKE/IFNULL 节点，因此宽 MATCH 列表或多词查询会扩大计划表达式树。本文件没有限流或资源预算。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/planner/core/fulltext_to_like.go::(*expressionRewriter).convertMatchAgainstToLike`。两者都只有一次委托，均调用 expression 包的 `BuildFTSToILikeExpression`，并保持上下文、列、搜索文本、modifier、成功表达式和错误的语义。

类型表达略有差异：Go 通过 receiver 的 `er.sctx` 取得上下文，列类型为 `[]expression.Expression`，modifier 是 `ast.FulltextSearchModifier`；Rust 将上下文显式作为 `&dyn BuildContext` 参数，列为 `Vec<ExprBox>`，modifier 暂以 `u8` 表示。Rust 的所有权移动替代 Go slice/string 的运行时引用语义，但没有业务转换。

Go 文件的大段注释明确该函数只服务直接布尔谓词回退，并记录相关度、停用词、词长、词边界和性能差异。Rust 源文件以更短注释强调共享构造器负责 token、modifier、NULL 和谓词组合；完整的上游防线仍可在 Rust `expression_rewriter.rs` 找到。

迁移接线目前不完全对齐：Go 的 `expression_rewriter.go::matchAgainstToLike` 通过 receiver 方法调用同路径 Go 文件；Rust 的对应生产函数调用 `expression_rewriter.rs` 内重复的私有薄封装。目标 Rust 文件虽公开再导出，但目前只有独立测试使用。后续若消除重复，应让生产重写器调用本文件公开函数，并保持 Go 流程与错误传播不变。

## 扩展指南

- 若新增搜索模式或 modifier 位，优先修改共享 `pkg/expression/planner_bridge.rs::BuildFTSToILikeExpression` 及其独立测试；本薄封装只需在参数类型确实改变时同步，不能在 planner 层另写一套解析规则。
- 若要完成生产接线，修改点应是 `pkg/planner/core/expression_rewriter.rs::matchAgainstToLike` 的最终调用，并移除或避免继续维护其中的重复私有转换器；同步扩充 `pkg/planner/core/fulltext_to_like_test.rs`，必要时在独立重写器测试中覆盖真实主链。测试不得写回生产 `.rs` 文件。
- 必须保留直接布尔上下文限制。SELECT、ORDER BY、比较、CASE 或其它评分上下文需要浮点相关度，不能无条件改成 ILIKE 布尔树。
- 必须保留 SQL NULL、可变参数计划缓存、列类型和常量搜索串的上游检查；本函数的非空 `String` 签名不能替代这些语义防线。
- 新增错误上下文时应保证下游错误身份和可诊断信息仍可见；不要把所有失败折叠成布尔值或零表达式。
- 性能变更应评估表达式树规模约为列数乘 token 数、ILIKE substring 扫描无法利用全文索引，以及重复转换器导致行为漂移的风险。

正确性风险集中在 modifier 位解释、布尔/评分语境混用、NULL 三值逻辑和 token 近似；兼容性风险集中在公开 crate 再导出及 Go 风格函数名；性能风险来自生成的大型谓词树和非索引 ILIKE 扫描。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，其中 Rust 文件 7,032 个。
- RustCodeGraph `node --file pkg/planner/core/fulltext_to_like.rs --offset 1 --limit 240`：读取完整 30 行，确认唯一函数、签名、委托调用和“used by 1 file: pkg/planner/core/fulltext_to_like_test.rs”。
- RustCodeGraph `query convertMatchAgainstToLike --kind function --json --limit 10`：区分目标函数、`expression_rewriter.rs` 的调用位置/私有函数和 Go 对照函数，并取得目标精确符号 ID。对该精确 ID 的 `callers/callees` 查询在限定时间内未返回结果，因此调用关系另用模块入口和精确仓库搜索交叉核验。
- Rust 源与装配：`pkg/planner/core/fulltext_to_like.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/expression_rewriter.rs`。
- crate 与下游实现：`pkg/planner/core/Cargo.toml`、`pkg/expression/planner_bridge.rs`、`pkg/expression/lib.rs`。
- Go 对照：`pkg/planner/core/fulltext_to_like.go`、`pkg/planner/core/expression_rewriter.go`；Go 独立测试 `pkg/planner/core/fulltext_to_like_test.go` 当前覆盖 modifier 与索引可行性辅助逻辑，没有直接调用薄封装。
- Rust 独立测试：`pkg/planner/core/fulltext_to_like_test.rs` 直接验证错误透传、modifier 位和空搜索成功；其中索引探测测试属于相邻重写逻辑，不是本文件职责。
- 人工复核：本文区分了目标薄封装、共享下游实现、上游前置检查及当前未接入生产链的事实，能够回答文件为何存在、如何执行、边界在哪里以及如何安全接线。
- 未运行 Cargo 或代码测试：任务明确为纯文档分析；验证采用源码/调用证据和固定章节结构检查。

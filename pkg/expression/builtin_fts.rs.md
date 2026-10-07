# `pkg/expression/builtin_fts.rs`

## 文件定位

本文件是 `astersql-expression` crate 中全文检索（FTS）内置函数的轻量校验内核，源文件由 [`pkg/expression/lib.rs`](lib.rs) 以 `builtin_fts_kernel` 私有模块装入，再通过公开模块 `astersql_expression::builtin_fts` 全量再导出。crate 边界由 [`pkg/expression/Cargo.toml`](Cargo.toml) 定义；本文件自身唯一直接外部依赖是 `thiserror = "2"`，用于生成稳定的错误展示文本。

它负责把上层已经归类好的表达式形状转换为两种 FTS 签名状态，并在 TiDB/AsterSQL 侧直接求值时执行保护。它不解析 SQL、不持有真实表达式树，也不执行倒排索引匹配。生产代码中可确认的上游是 [`pkg/session/fts_runtime.rs`](../session/fts_runtime.rs) 的 `FtsSessionRuntime::validate_fts_expression`：该适配层从 AST 提取 `FTS_MATCH_WORD` 的查询词和列形状，再调用 `build_match_word`。当前仓库搜索未发现非测试 Rust 代码调用 `build_mysql_match_against` 或 `set_mysql_match_against_modifier`，因此 MySQL `MATCH ... AGAINST` 这一半应视为已实现并公开、但尚未由 Rust 生产主链接线的表达式内核。

## 核心职责

1. 用 `FtsAgainst` 和 `MatchArgument` 表示上层完成类型/形状判断后的最小输入，而不是重复依赖完整 AST 或表达式对象。
2. 在 `build_match_word` 中保持 Go 版 `FTS_MATCH_WORD` 的校验顺序：先参数个数，再 starter 部署门禁，再查询词常量性，最后列形状。
3. 在 `build_mysql_match_against` 中保持 MySQL 形式的约束：至少一个匹配列，`AGAINST` 只能是字符串常量或 `NULL`，且所有匹配参数都必须是字符串列。
4. 用 `FtsMatchWordSig`、`FtsMysqlMatchAgainstSig` 保存构建成功后的不可变核心状态，并允许 MySQL 签名后置写入搜索修饰符。
5. 阻止无全文索引的本地实数求值：`FTS_MATCH_WORD` 总是报错；MySQL 形式仅在 `AGAINST NULL` 时按 SQL 空值语义返回 `NULL`，其余情况报错。

本文件不是完整 builtin 框架的 Rust 等价物。Go 版还构造 `baseBuiltinFunc`、设置 protobuf 签名、写入 statement context，并接入全局函数注册表；Rust 文件只保留当前迁移所需的校验和守卫状态。

## 主要符号

- `FtsAgainst::{String, Null, NonStringConstant, NonConstant}`：查询词一侧的四种互斥分类。`String` 拥有查询字符串；其余变体只保留校验所需的类别。
- `MatchArgument::{StringColumn, NonStringColumn, NonColumn}`：单个 MATCH 参数的分类。它区分“是不是列”以及“列是不是字符串”，不保存列 ID、类型对象或值。
- `FulltextSearchModifier`：本地搜索模式枚举，默认值是 `NaturalLanguage`，另有 `Boolean`、`QueryExpansion` 和 `NaturalLanguageWithQueryExpansion`。该类型是本文件的简化状态，不是 parser AST 修饰符类型。
- `FtsError`：覆盖参数个数、部署模式、查询词常量性/类型、列形状、索引外求值和签名类型不匹配。派生的 `thiserror::Error` 让上层可通过 `to_string()` 传播与 Go 版对应的消息。
- `FtsMatchWordSig`：保存查询词、列数和 `fts_function_used` 标志。字段私有，通过 `against`、`column_count`、`fts_function_used` 读取；`eval_real` 只返回 `MatchWordOutsideIndex`。
- `FtsMysqlMatchAgainstSig`：保存 `Option<String>` 查询词、列数和修饰符。查询词为 `None` 表示 `AGAINST NULL`；`set_modifier`、`modifier` 管理模式，`eval_real` 实现 NULL 特例与索引外保护。
- `FtsSignature::{MatchWord, Mysql}`：为后置的 MySQL 修饰符设置提供带类型分支的统一容器；`mysql_modifier` 对 MatchWord 返回 `None`。
- `validate_columns`：唯一私有函数。它总是拒绝 `NonColumn`，并按 `require_string` 决定是否拒绝 `NonStringColumn`。
- `build_match_word`、`build_mysql_match_against`：两个公开构建入口；只有完整校验成功才产生签名对象。
- `set_mysql_match_against_modifier`：只接受 `FtsSignature::Mysql`，错误签名返回 `UnexpectedSignature`，避免静默修改错误对象。

## 执行流程

`FTS_MATCH_WORD` 路径如下：

1. `FtsSessionRuntime::validate_fts_expression` 识别函数名，先要求 AST 参数总数为 2。
2. 会话适配层把第一参数归类为 `FtsAgainst`，把第二参数归类为 `MatchArgument`，并传入当前 `deploymode::IsStarter()`。
3. `build_match_word` 要求 `columns.len() == 1`；因此即使部署模式或参数类型同时不合法，参数个数错误仍优先返回。
4. 非 starter 模式返回 `StarterOnly`。
5. 只有 `FtsAgainst::String` 被接受；`Null`、非字符串常量和非常量统一映射到 `NonConstantAgainst`，与 Go 当前错误行为一致。
6. `validate_columns(columns, false)` 拒绝非列，但有意接受非字符串列；Go 路径同样只检查 `*Column`，执行类型由 protobuf 签名约束。
7. 成功后构造 `FtsMatchWordSig`，列数为 1，`fts_function_used` 固定为 `true`。后续若调用 `eval_real`，必然得到 `MatchWordOutsideIndex`，提示该函数只能在全文索引路径中计算。

MySQL `MATCH ... AGAINST` 路径如下：

1. `build_mysql_match_against` 先要求 `columns` 非空，对应 Go 注册的最少两个总参数（一个 AGAINST 参数加至少一个 MATCH 列）。
2. 字符串查询词存为 `Some(String)`，`NULL` 存为 `None`；非字符串常量和非常量分别返回不同错误。
3. `validate_columns(columns, true)` 同时要求每项是列且是字符串列，遇到第一个非法项立即返回。
4. 成功签名的修饰符初始化为 `NaturalLanguage`。调用者可将其包成 `FtsSignature::Mysql`，再通过 `set_mysql_match_against_modifier` 修改模式。
5. `eval_real` 对 `None` 返回 `Ok(None)`；非空查询词返回 `MatchAgainstOutsideIndex`，不尝试做扫描式全文匹配。

## 数据与状态

所有输入、签名和错误类型都实现 `Clone`、`Debug` 与相等比较，便于上层传递和独立测试；修饰符和列分类还实现 `Copy`。文件没有全局可变状态。

`FtsMatchWordSig` 中的 `against` 是拥有所有权的字符串，`columns` 只记录数量而不保留列集合；`fts_function_used` 在成功构建时恒为 `true`，对应 Go 版对 `StmtCtx.FTSFunctionIsUsed` 的副作用，但 Rust 目前只把该事实保存在签名对象内，不会直接写会话状态。

`FtsMysqlMatchAgainstSig.against` 使用 `Option<String>` 精确区分 NULL 与非 NULL 查询词。`modifier` 是构建后唯一可变字段，默认自然语言模式；`set_modifier` 不再验证模式与下推能力，兼容性判断应由规划/下推层负责。两个签名的 `columns` 都是构建时切片长度的快照，之后不存在与原切片共享的生命周期。

## 依赖与调用关系

- 装配与导出：[`pkg/expression/lib.rs`](lib.rs) 用 `#[path = "builtin_fts.rs"] mod builtin_fts_kernel;` 编译本文件，再由 `pub mod builtin_fts` 公开符号；测试模块 `builtin_fts_test` 与聚合测试门面复用同一内核。
- 直接生产调用：[`pkg/session/fts_runtime.rs`](../session/fts_runtime.rs) 导入 `astersql_expression::builtin_fts`，在 `FtsSessionRuntime::validate_fts_expression` 中调用 `build_match_word`。该函数递归扫描 SELECT 的 WHERE、投影和 ORDER BY 表达式，构建器错误转换为 `SessionError`。
- 内部调用边：RustCodeGraph 显示 `build_match_word -> validate_columns`、`build_mysql_match_against -> validate_columns`，二者分别实例化对应签名；`set_mysql_match_against_modifier -> FtsMysqlMatchAgainstSig::set_modifier`。图还把切片 `len` 解析到了不相关的同名方法，不能据此推导业务依赖。
- 测试调用：[`pkg/expression/builtin_fts_test.rs`](builtin_fts_test.rs) 直接覆盖全部公开构建/求值/修饰符路径；[`pkg/expression/builtin_encryption_11_aster_unit_test.rs`](builtin_encryption_11_aster_unit_test.rs) 也通过测试门面做聚合覆盖。
- Go 主链：[`pkg/expression/builtin.go`](builtin.go) 把两个函数名注册到各自 function class；[`pkg/expression/builtin_fts.go`](builtin_fts.go) 实现完整 Go builtin。Go 的规划器/ILIKE 后备还通过 `SetFTSMysqlMatchAgainstModifier` 使用 MySQL 签名；这不是对同名 Rust helper 已有生产调用的证明。
- crate 依赖：本文件只使用 `thiserror`。`deploymode` 虽在 expression crate manifest 中声明，但部署模式布尔值由调用者传入，保持本内核可独立测试。

RustCodeGraph 能定位本文件符号和内部 callee，但其 `callers` 查询没有返回跨 crate 的 `pkg/session` 边；上述跨 crate 调用由精确源码搜索和调用点阅读确认。

## 错误处理与边界

构建函数使用 `Result<_, FtsError>`，不 panic，也不吞掉非法输入。校验采用短路顺序，因此错误优先级是外部可观察语义：参数个数先于部署门禁，部署门禁先于 `FTS_MATCH_WORD` 的查询词/列检查；MySQL 路径中查询词检查先于列检查。扩展时不应随意交换这些检查。

重要边界包括：

- `FTS_MATCH_WORD` 必须恰有一个匹配列，且仅 starter 部署可构建。
- 它把 NULL、非字符串常量和非常量统一视为“非常量字符串”错误，并允许 `NonStringColumn`；这是刻意对齐 Go，而不是遗漏类型检查。
- MySQL 形式允许任意正数个字符串列；空列列表、非列或非字符串列均失败。
- MySQL 形式允许 `AGAINST NULL`，且这是唯一可在本地 `eval_real` 成功的情形，结果是 SQL NULL（`Ok(None)`），不是数值 0。
- 两个 `eval_real` 都不接收行或执行上下文，说明它们只是语义守卫，不是实际索引执行器。
- `set_mysql_match_against_modifier` 对 `MatchWord` 返回错误，防止将 MySQL parser 修饰符错误套到 TiDB 扩展函数。

当前类型无法表达具体列身份、排序规则、字符集、索引元数据或 protobuf 签名；这些必须由上层保证。本文件也不判断某种修饰符是否能下推 TiFlash，不能把“成功设置修饰符”等同于“可执行/可下推”。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、本地任务、通道、事务、文件或网络资源。所有构建操作只消费值或借用只读切片，返回拥有自身字符串和标量状态的签名，因此没有跨调用借用关系。

`FtsMatchWordSig` 的状态在构造后不可变；`FtsMysqlMatchAgainstSig` 仅能通过 `&mut self` 修改修饰符，`set_mysql_match_against_modifier` 同样要求对枚举的独占可变借用。因此并发安全由 Rust 所有权规则保证，不需要内部同步。克隆签名会复制字符串和状态，之后修改某个克隆的修饰符不会影响其他实例。

实际事务、全文索引和计划生命周期不属于本文件。生产调用者 [`pkg/session/fts_runtime.rs`](../session/fts_runtime.rs) 自己持有表元数据、prepared 映射和可选 KV 事务；本内核既不借用也不改变那些资源。

## 与 Go 版本的对应关系

Rust `build_match_word` 对应 Go `ftsMatchWordFunctionClass.getFunction` 的核心校验：Go 的 `verifyArgs` 由 Rust 的列数检查近似表达；`deploymode.IsStarter()` 改为显式布尔参数；第一参必须是字符串 `Constant`；其余参数必须是 `Column`。Go 构造 `baseBuiltinFunc`、设置 `tipb.ScalarFuncSig_FTSMatchWord` 并写 `sessionVars.StmtCtx.FTSFunctionIsUsed = true`，Rust 则只保存查询词、列数和本地布尔标志，尚未复制这些框架副作用。

Rust `build_mysql_match_against` 对应 Go `ftsMysqlMatchAgainstFunctionClass.getFunction`：接受字符串常量或 NULL，拒绝非常量与非字符串常量，并要求所有 MATCH 列为字符串列。Go 还创建返回类型为 real 的 builtin 并设置 `tipb.ScalarFuncSig_FTSMatchExpression`；Rust 没有 base builtin、返回类型或 protobuf code。

Rust 两个 `eval_real` 与 Go 两个 `evalReal` 的保护语义一致：MatchWord 始终拒绝 TiDB 侧执行；MySQL 形式先识别 NULL 查询词并返回 NULL，否则拒绝索引外执行。区别是 Go 从 `b.args[0]` 防御性读取实际常量，Rust 已在构建期把 NULL 压缩为 `Option`，求值时不再读取表达式参数。

Rust 的 `set_mysql_match_against_modifier`/`FtsSignature` 对应 Go `SetFTSMysqlMatchAgainstModifier` 的签名类型检查及 `SetModifier`，但 Rust helper 当前未见生产调用。Go 使用 `ast.FulltextSearchModifier`，Rust 使用本地四变体枚举；若将 Rust 路径接入 parser/planner，必须显式验证两者所有组合的映射，尤其是“自然语言 + 查询扩展”的位组合。

Go 证据还包括 [`pkg/expression/fts_to_like_test.go`](fts_to_like_test.go)：它按规划器流程先构建真实 MySQL FTS scalar function，再设置 modifier，并验证 NULL、错误签名、多列后备及非默认模式下推限制。那些属于完整 Go 表达式/规划器层，不能由本 Rust 内核测试替代。

## 扩展指南

- 若新增查询词或列分类，先修改 `FtsAgainst`/`MatchArgument`，再逐一审查两个 builder 的穷尽匹配和错误优先级；同步更新独立测试 [`pkg/expression/builtin_fts_test.rs`](builtin_fts_test.rs)，不要把测试写回生产文件。
- 若改变参数个数、starter 门禁、NULL 或非字符串列语义，必须先对照 [`pkg/expression/builtin_fts.go`](builtin_fts.go) 的 `getFunction` 顺序以及 [`pkg/expression/builtin.go`](builtin.go) 的注册上下界。除非任务明确改变兼容性，不要“改进”成与 Go 不同的错误类型或检查顺序。
- 若新增搜索修饰符，需同时更新 `FulltextSearchModifier`、默认值、访问/设置路径和测试；接入完整 Rust planner 前还要核对 parser AST 与 TiFlash/tipb 是否能表达该模式，避免本地接受但下推丢失。
- 若把 MySQL builder 接入生产主链，最可能的接入点是 AST/表达式到 builtin 的适配层；应复用本构建器而不是复制校验，并补独立集成测试证明字符串常量、NULL、多列、非字符串列及索引外求值行为。
- 若要支持真实匹配计算，不应在 `eval_real` 中做无索引全表扫描；应让规划器把签名绑定到全文索引执行路径，并保留这里的索引外守卫。需要明确索引生命周期、事务可见性和下推协议，而不是只返回一个伪分数。
- 若让 `fts_function_used` 真正影响 statement context，应在会话/表达式构建层完成状态写入并测试生命周期；不要在这个无上下文内核中引入全局可变状态。
- 兼容性风险主要是错误消息/优先级、NULL 三值逻辑和修饰符映射；性能风险主要来自误把守卫替换成逐行本地匹配，或在 builder 中复制/保存完整列对象。当前构建仅线性扫描列分类并复制查询字符串。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用；`query` 分别定位 `build_match_word`（第 170 行）、`build_mysql_match_against`（第 200 行）和 `set_mysql_match_against_modifier`（第 223 行）；`node --file pkg/expression/builtin_fts.rs` 读取完整 234 行源码。
- RustCodeGraph callee：确认两个 builder 均调用 `validate_columns` 并实例化各自签名；modifier helper 调用 `FtsMysqlMatchAgainstSig::set_modifier`。caller 查询未给出跨 crate 结果，因此跨 crate 关系另以源码搜索核验。
- 已读生产源码/配置：[`pkg/expression/builtin_fts.rs`](builtin_fts.rs)、[`pkg/expression/lib.rs`](lib.rs)、[`pkg/expression/Cargo.toml`](Cargo.toml)、[`pkg/session/fts_runtime.rs`](../session/fts_runtime.rs)、[`pkg/session/lib.rs`](../session/lib.rs)、[`pkg/expression/builtin_fts.go`](builtin_fts.go)、[`pkg/expression/builtin.go`](builtin.go)。`pkg/expression` 下未发现 `doc.go`。
- 已读测试：[`pkg/expression/builtin_fts_test.rs`](builtin_fts_test.rs)、[`pkg/expression/builtin_encryption_11_aster_unit_test.rs`](builtin_encryption_11_aster_unit_test.rs) 的 FTS 段，以及 Go [`pkg/expression/fts_to_like_test.go`](fts_to_like_test.go) 的真实 builtin 构建和 modifier/NULL 边界。源码搜索还确认会话级独立测试位于 `pkg/session/fts_runtime_test.rs`。
- 人工复核结论：该文件存在的目的，是在不依赖完整表达式框架的前提下复刻 FTS 构建门禁和索引外求值保护；当前 Rust 生产链只确认接入 `FTS_MATCH_WORD` 校验，MySQL builder/modifier 仍只见测试使用。安全扩展必须保持 Go 校验顺序与 NULL 语义，并把真实执行留给全文索引路径。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构验证要求本文恰好包含任务指定的 11 个二级标题。

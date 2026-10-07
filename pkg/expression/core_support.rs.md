# `pkg/expression/core_support.rs`

## 文件定位

`core_support.rs` 属于 `astersql-expression` crate，是表达式核心类型周围的集中兼容与支撑层。模块由 [`lib.rs`](lib.rs) 通过 `#[path = "core_support.rs"] mod core_support` 挂载，并用 `pub use core_support::*` 向 crate 使用者整体再导出，因此其中的公开函数既可经 `crate::core_support::...` 调用，也可经 `expression::...` 调用。它不负责定义 `Expression`、`Constant`、`Column` 或 `ScalarFunction` 本体，而是基于这些类型提供函数分类表、ROW/列树辅助、CAST 包装、常量折叠、常量向量填充和动态向下转型。

该文件把 Go 版本分散在 `function_traits.go`、`util.go`、`builtin_cast.go`、`constant_fold.go`、`vectorized.go`、`expression.go` 和 `builtin_vectorized.go` 的若干能力集中到一个 Rust 文件中。crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定；本文件通过 `use crate::*` 使用 crate 根再导出的 AST、MySQL 类型、chunk、错误、日志及表达式类型，而不是自行声明外部依赖。

## 核心职责

1. `logicalOps`、`unFoldableFunctions`、`DisableFoldFunctions`、`TryFoldFunctions` 和 `noopFuncs` 保存构建、优化与常量折叠阶段使用的函数分类。
2. `GetRowLen`、`GetFuncArg`、`CheckArgsNotMultiColumnRow`、`SetExprColumnInOperand`、`ExtractColumnsMapFromExpressions` 等函数维护表达式树的结构语义。
3. `BuildCastFunctionWithCheck` 及五个 `WrapWithCastAs*` 入口推导 CAST 的目标 `FieldType`，再委托 `formal_registry::BuildCastFunction` 创建实际标量函数。
4. `FoldConstant`/`foldConstant` 在构建或优化阶段求值可确定子树，同时保护短路分支、延迟常量、计划缓存和类型元数据。
5. `genVecFromConstExpr` 只求值一次常量表达式，再把结果（或 NULL）复制到与输入 `Chunk` 等行数的列中。
6. `DropPool`、`ColumnAllocator` 和三个全局池对象保留 Go 调用面，但当前不复用内存；`impl dyn Expression` 提供统一的安全向下转型入口。

## 主要符号

- `DropPool::Put<T>` 丢弃传入对象；`expressionSlices`、`selPool`、`zeroPool` 都是该零状态占位类型。`ColumnAllocator::get` 每次返回默认 `chunk::Column`，`put` 丢弃列，`globalColumnAllocator` 是其全局单例。这些符号是 API 兼容面，不是 Go `sync.Pool`/`zeropool` 的等价缓存。
- 五张 `LazyLock<HashMap<&'static str, ()>>` 表在首次访问时线程安全初始化。`logicalOps` 有 18 个比较、逻辑或谓词函数；本文件的 `unFoldableFunctions` 有 18 个不可折叠函数；`DisableFoldFunctions` 只有 `benchmark`；`TryFoldFunctions` 有 `if`、`ifnull`、`case`、`and`、`or`、`coalesce`、`interval`；`noopFuncs` 为空。
- `ExpressionErrorKind::{OperandColumns, NotSupportedYet, IncorrectParameterCount}` 与三个同名错误模板静态值通过 `GenWithStackByArgs` 生成字符串错误。名称保留 Go 风格，但 Rust 实现使用 `errors::New`，没有在这里建立 Go `terror` 类型层级。
- `GetRowLen(&dyn Expression) -> usize` 仅对函数名为 `ast::RowFunc` 的标量函数返回参数数，其余表达式返回 1。`GetFuncArg` 克隆指定参数；输入不是标量函数或索引越界属于调用方违反不变量，会分别触发 `expect` 或索引 panic。
- `MultiColumnArguments` 统一支持 `[ExprBox]`、`Vec<ExprBox>` 和单个 `dyn Expression`；`CheckArgsNotMultiColumnRow` 发现任一 ROW 宽度不为 1 时返回 `ErrOperandColumns(1)`。
- `SetExprColumnInOperand` 克隆列并置 `InOperand = true`；遇到标量函数时克隆函数、递归替换参数并调用 `CleanHashCode`，从而不原地污染共享表达式树。常量等其他节点原样返回。
- `DatumToConstant` 根据 MySQL 类型码和 flag 建立 `Constant`；`GetIntFromConstant` 在空行上先走 `EvalString`，NULL 或无法解析成 `i64` 时返回 `(0, true)`。
- `BuildCastFunctionWithCheck` 创建 CAST 后写入显式字符集标记；`WrapWithCastAsInt/Real/Decimal/String/Time` 在源求值类型已经兼容时短路，否则根据源宽度、精度、符号、非空标志、字符集/排序规则和目标时间类型生成 `FieldType`。
- `option_expr_equals` 按 `None`/`Some` 结构比较可选表达式；`ExtractColumnsMapFromExpressions` 深度遍历 `Column` 与 `ScalarFunction`，经函数指针过滤后按 `UniqueID` 去重；`CanImplicitEvalReal` 当前只识别 `DayName`。
- `MaybeOverOptimized4PlanCache` 仅在 `BuildContext::IsUseCache()` 为真时递归查找带 `ParamMarker` 或 `DeferredExpr` 的常量。
- `FoldConstant` 是公开入口；内部 `foldConstant` 返回“折叠结果 + 是否延迟”的二元组，`foldIf`、`foldIfNull`、`foldCase`、`foldIsNull` 实现避免求值未选分支的专用路径。
- `genVecFromConstExpr` 支持 `ETInt`、`ETReal`、`ETDecimal`、`ETDatetime`/`ETTimestamp`、`ETDuration`、`ETString`、`ETJson`、`ETVectorFloat32`；`GetDisplayName` 把部分内部函数名映射成 Explain 运算符。
- `Expression::as_column/as_correlated_column/as_scalar_function/as_constant` 都基于 `Any::downcast_ref`，失败返回 `None`，不改变对象所有权。

## 执行流程

常量折叠的主流程如下：

1. `FoldConstant` 先保存原表达式的 coercibility、repertoire、charset 和 collation，调用 `foldConstant` 后把这些顶层元数据恢复到结果上。
2. `foldConstant` 遇到标量函数时，先跳过 `unFoldableFunctions` 中的函数和扩展函数；若计划缓存检查未判定为易过度优化，再把 `IF`、`IFNULL`、`CASE`、`ISNULL` 分派到短路处理器。
3. 普通函数扫描直接参数，记录是否全部为常量、是否含 NULL、是否含参数标记/延迟表达式。若并非全常量，通常保留原树；只在 null-reject 检查中用 `NewOne()` 替换非常量参数，探测结果是否必为 NULL 或 false。`NullEQ`、`ConcatWS`、`Field` 被排除，因为哑值不能代表其真实结果。
4. 全常量函数在空行上执行 `Eval`。成功后校正 `NotNullFlag`：带延迟成分时用 `Constant::with_deferred` 保留原函数以便以后重算，否则用 `with_subquery` 传播首个正 `SubqueryRefID`。求值失败只记录日志并返回原表达式，让运行阶段仍有机会向客户端报告真实错误。
5. 单独的 `Constant` 若有 `ParamMarker`，通过 `GetUserVar` 取当前参数；若有 `DeferredExpr`，则立即试算并保留延迟属性。读取或求值失败时保留原节点并标记为延迟。
6. `foldIf` 只折叠条件和最终选中的一个分支；`foldIfNull` 只在首参数可确定时决定是否折叠第二参数；`foldCase` 按 WHEN/THEN 对顺序推进，命中后只折叠对应 body，未命中时处理 ELSE；`foldIsNull` 对 `NotNullFlag` 明确的非常量直接返回零。

CAST 流程先读取源 `FieldType`，兼容时直接返回原 `ExprBox`，否则建立目标类型并经私有 `wrap_with_cast` 统一调用 `formal_registry::BuildCastFunction`。字符串 CAST 还依据显式 coercibility、BIT 类型或上下文默认字符集选择 charset/collation；时间 CAST 按源求值类型决定 FSP，再计算 DATE/DATETIME/TIMESTAMP 的显示宽度。

向量填充流程由 `Column` 和 `Constant` 的各个 `VecEval*` 路径调用：读取 `input.NumRows()`，按照目标 `EvalType` 对表达式求值一次，清空或预留对应列存储，然后循环追加相同值或 NULL。JSON 逐行 `clone`，向量浮点值逐行调用 `Clone`；不支持的类型立即返回错误。

## 数据与状态

表达式树通过 `ExprBox`（盒装 `dyn Expression`）传递。多数树变换采用克隆后返回新节点的方式；`SetExprColumnInOperand` 会清理克隆后标量函数的派生哈希缓存，避免结构已变但缓存仍代表旧参数。`ExtractColumnsMapFromExpressions` 的结果拥有 `Column` 克隆并按 `UniqueID` 覆盖去重，遍历范围仅覆盖本文件识别的 `Column` 和 `ScalarFunction` 节点。

常量折叠同时维护值与元数据：`DeferredExpr`/`ParamMarker` 表示值可能随执行上下文改变，`SubqueryRefID` 关联子查询来源，`NotNullFlag` 描述折叠结果的可空性；公开入口还强制保留原根节点的字符集、排序规则、coercibility 和 repertoire。专用 `CASE` 路径会把结果常量的 decimal 调整为原函数返回类型的 decimal。

全局可变语义主要来自 `LazyLock` 的一次性初始化；映射初始化后仅只读。`DropPool` 和 `ColumnAllocator` 自身无字段、无缓存状态。`genVecFromConstExpr` 会重置或扩充调用方传入的 `result`，其最终长度等于输入 chunk 的行数。

## 依赖与调用关系

上游方面，[`lib.rs`](lib.rs) 将本模块的符号公开到 crate 根。源码引用搜索确认：

- [`scalar_function.rs`](scalar_function.rs)、[`expression.rs`](expression.rs) 和 [`builtin.rs`](builtin.rs) 在函数构建、表达式重写或二进制转换中调用 `FoldConstant`；[`constant_fold.rs`](constant_fold.rs) 也复用本文件的计划缓存保护检查。
- [`builtin.rs`](builtin.rs) 以及 [`aggregation/base_func.rs`](aggregation/base_func.rs) 调用 `WrapWithCastAsInt/Real/Decimal/String/Time`，用于内建函数和聚合参数的类型归一。
- [`column.rs`](column.rs) 与 [`constant.rs`](constant.rs) 的向量求值实现调用 `genVecFromConstExpr`。
- [`expression.rs`](expression.rs) 使用 `logicalOps`、`ExtractColumnsMapFromExpressions` 和 `MaybeOverOptimized4PlanCache`；[`constant_propagation.rs`](constant_propagation.rs) 也调用计划缓存检查。

下游方面，本文件依赖 `BuildContext`/`EvalContext` 提供计划缓存开关、求值上下文和默认字符集，依赖 `formal_registry::BuildCastFunction` 构建实际 CAST，依赖 `Expression` 各类型化 `Eval*` 方法求值，依赖 `chunk::{Chunk, Column, Row}` 承载行列数据，并使用 `types`、`mysql`、`charset` 推导字段元数据。错误通过 `errors::Error` 返回，折叠失败日志通过 `logutil::BgLogger` 发出。

RustCodeGraph 的文件节点报告本文件被 39 个文件使用；精确 `query` 能区分 `core_support.rs::FoldConstant`、Go `constant_fold.go::FoldConstant` 和 `constant_fold.rs::FoldConstant`。本次 `callers`/`callees` 查询超时且未返回边，因此上述具体边由 crate 装配文件和源码引用搜索复核，不以同名符号猜测。

## 错误处理与边界

- 可恢复的求值错误使用 `Result<_, errors::Error>`：`GetIntFromConstant` 传播 `EvalString` 错误；CAST 包装当前实际构造路径不返回错误，但 `BuildCastFunctionWithCheck` 为接口兼容包成 `Result`；`genVecFromConstExpr` 传播类型化求值错误并拒绝未支持的 `EvalType`。
- 常量折叠有意吞下优化期求值失败并保留原表达式，因为优化失败不应改变运行语义；普通折叠与延迟表达式失败记 debug，参数读取失败记 warn。该行为不等于函数永不报错，错误可在后续真实执行中重新出现。
- `GetFuncArg`、`foldIf`、`foldIfNull` 和 `foldIsNull` 直接索引参数，依赖函数构建层已保证参数个数；本文件不做防御性长度检查。错误模板中的 `GenWithStackByArgs` 名称也不代表 Rust 实现真的附加了栈。
- `GetIntFromConstant` 把解析失败与 NULL 都表示为布尔值 `true`，并返回数值零；调用方必须检查该标志，不能把零当成成功结果。
- `ExtractColumnsMapFromExpressions` 不遍历未知的 `Expression` 实现；新增复合节点类型若不表现为 `ScalarFunction`，需要显式扩展访问逻辑。
- `genVecFromConstExpr` 假设传入表达式可按目标类型求值；目标类型与表达式不匹配时由相应 `Eval*` 返回错误。空输入会完成一次求值但不追加元素。

## 并发与资源生命周期

`LazyLock` 保证分类表在并发首次访问时只初始化一次，之后只读访问不需要调用方加锁。除此之外，本文件不创建线程、任务、通道、锁、事务或异步生命周期。

Rust 的 `ExprBox`、`String`、`Datum`、`Column` 等由所有权和析构管理。`DropPool::Put` 与 `ColumnAllocator::put` 立即让所有权在函数结束时释放；`ColumnAllocator::get` 新建默认列。因此它们不会像 Go 的 `zeropool` 或本地列池一样跨调用保留容量，行为正确性独立于池命中，但可能存在额外分配成本。`genVecFromConstExpr` 借用输入和上下文、独占借用结果列，不保存跨调用引用；复制 JSON 和向量值时显式克隆，避免多行共享可变所有权。

## 与 Go 版本的对应关系

- `GetRowLen`、`CheckArgsNotMultiColumnRow`、`GetFuncArg`、`DatumToConstant`、`GetIntFromConstant`、`SetExprColumnInOperand`、`ExtractColumnsMapFromExpressions`、`logicalOps`、`MaybeOverOptimized4PlanCache` 对应 [`util.go`](util.go) 的同名或相邻逻辑。Rust `GetFuncArg` 对非标量输入 panic，而 Go 返回 `nil`；Rust 的列提取结果存值克隆，Go 存指针。
- CAST 包装对应 [`builtin_cast.go`](builtin_cast.go) 的 `BuildCastFunctionWithCheck`、`CanImplicitEvalReal` 和 `WrapWithCastAs*`。当前 Rust 保留 `_in_union` 参数但不使用；实际 CAST 构建委托 Rust `formal_registry`。Go 测试 [`builtin_cast_test.go`](builtin_cast_test.go) 覆盖数值、字符串、时间、unsigned flag、FSP 与字符集等意图，是扩展时的重要对照。
- 折叠算法对应 [`constant_fold.go`](constant_fold.go)：均保留根元数据、跳过不可折叠/扩展函数、对四类函数短路、处理 null-reject 探测、延迟常量和子查询引用。一个已验证差异是 Go `ifNullFoldHandler` 在首参数折叠为 NULL 时会把原函数返回类型的 charset/collation 改成第二参数；本文件 `foldIfNull` 直接返回第二参数折叠结果，没有同样的原函数类型写回动作。
- 函数表对应 [`function_traits.go`](function_traits.go)，但不是当前 Go 文件的完全相同快照：Go 的 `unFoldableFunctions` 还包含 `ast.EmbedText`，共 19 项；本文件和 [`core_support_test.rs`](core_support_test.rs) 固定为 18 项。其他三张受该 Rust 测试检查的表与读取到的 Go 条目一致。
- `genVecFromConstExpr` 对应 [`vectorized.go`](vectorized.go)；占位池对应 [`expression.go`](expression.go) 的表达式/选择/零值池以及 [`builtin_vectorized.go`](builtin_vectorized.go) 的列池，但 Rust 当前只保持调用面，不提供复用语义。
- `GetDisplayName` 对应 [`builtin.go`](builtin.go)。本文件只映射比较与算术运算符；Go 当前还处理更多显示名（例如 `IS TRUE` 等），仓库中另有 `builtin.rs` 的同名实现，因此调用点与再导出解析需要结合 `lib.rs` 实际接线检查，不能假设两个 Rust 实现覆盖集合相同。

## 扩展指南

- 新增不可折叠、禁止子折叠或试探折叠函数时，修改对应 `LazyLock` 表，并在独立的 [`core_support_test.rs`](core_support_test.rs) 更新精确集合断言；同时逐项核对 `function_traits.go`，不要只改数量。
- 扩展常量折叠时优先修改 `foldConstant` 或四个专用 handler，并维持三个不变量：不求值未选短路分支、计划缓存中的可变常量不被固化、折叠失败保留原运行时错误。若行为本应由分拆的 [`constant_fold.rs`](constant_fold.rs) 承担，先确认 crate 当前调用的是哪个同名入口，避免只修复未接线副本。
- 新增 CAST 目标应在相应 `WrapWithCastAs*` 中完整推导 `FieldType`，包括 flen、decimal、unsigned/not-null、binary flag、charset/collation；测试应放在独立 Rust 测试文件，且与 `builtin_cast_test.go` 的边界意图对齐。
- 新增 `EvalType` 时同步扩展 `genVecFromConstExpr`、`Column` 和 `Constant` 的向量入口，选择正确的 `Resize`/`Reserve` 与追加 API，并测试 NULL、空 chunk、多行复制及拥有堆数据的值。
- 新增表达式节点种类时审查 `SetExprColumnInOperand`、`ExtractColumnsMapFromExpressions`、`MaybeOverOptimized4PlanCache` 和四个 `as_*` 方法是否需要识别它；任何结构变更都应清除相关哈希缓存。
- 若把占位池替换成真实复用池，必须定义归还前清理规则、容量上限和并发语义，并用独立测试验证无旧数据泄漏；不能仅为性能把 Go 指针复用机械翻译到 Rust 所有权模型。
- 对外扩展错误模板时，应决定是否只需要文本兼容，还是需要可匹配的错误码/类型；当前 `ExpressionErrorKind` 仅提供字符串构造。

## 验证依据

- 目标源码：[`core_support.rs`](core_support.rs)，完整读取 860 行，核对了所有模块级类型、静态值、trait、函数、impl 和分支。
- crate 装配与依赖：[`lib.rs`](lib.rs) 的模块声明和再导出；[`Cargo.toml`](Cargo.toml) 的 crate 名称、lib 入口及 expression 所需本地依赖。
- Rust 测试：[`core_support_test.rs`](core_support_test.rs) 独立于生产源文件，验证 `logicalOps`、函数 trait 表和空 `noopFuncs`；相关调用与覆盖意图还参考 `builtin_cast_4_aster_unit_test.rs`、`builtin_test.rs`、`constant_fold_38_aster_unit_test.rs` 等独立文件，但本纯文档任务未运行 Cargo。
- Go 对照：[`function_traits.go`](function_traits.go)、[`util.go`](util.go)、[`builtin_cast.go`](builtin_cast.go)、[`constant_fold.go`](constant_fold.go)、[`vectorized.go`](vectorized.go)、[`expression.go`](expression.go)、[`builtin_vectorized.go`](builtin_vectorized.go)、[`builtin.go`](builtin.go)；测试对照包括 `util_test.go`、`builtin_cast_test.go` 和 `builtin_test.go`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；文件节点展示本文件及 39 个使用文件；`query FoldConstant --kind function --json` 区分了本文件、分拆 Rust 文件和 Go 文件中的同名符号。自然语言 `explore` 无输出，精确 `callers`/`callees` 两次超时无边，因此调用关系又以 `rg` 的全仓引用和 `lib.rs` 装配交叉验证。
- 文档完成后按任务要求执行固定标题结构检查；本任务只生成说明文档，不运行 Cargo，也不声称运行时测试通过。

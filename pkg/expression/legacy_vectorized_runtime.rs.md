# `pkg/expression/legacy_vectorized_runtime.rs`

## 文件定位

`legacy_vectorized_runtime.rs` 是 `astersql-expression` crate 内部的遗留向量化兼容层。`pkg/expression/lib.rs` 以私有模块 `legacy_vectorized_runtime` 装配它，因此这里的类型不是 crate 对外公开 API；当前直接使用者集中在 `builtin_like.rs`、`builtin_like_vec.rs` 和 `builtin_math_vec.rs`，相关独立测试也通过同一私有模块访问。

这个文件存在的目的，是给已经独立迁移的 LIKE 与数学内核提供一套自洽、较小的表达式运行时：行号和批次、列缓冲区、求值上下文、错误、表达式 trait、字面量表达式，以及数学签名共享状态。它不是 Go `pkg/expression.Expression`、`pkg/util/chunk.Column` 或主 Rust 表达式运行时的完整替代品。`pkg/expression/Cargo.toml` 将本目录声明为 `astersql-expression`，并直接提供本文件使用的 `mathutil` 与 `types-dependency`。

## 核心职责

1. 用 `Row`、`Chunk` 和 `Column` 表示这组遗留内核需要的最小行式/列式数据接口。`Chunk` 只保存行数；`Column` 只覆盖 Int、Real、Decimal、String 四种缓冲区及 NULL 状态。
2. 用 `LegacyExpression` 统一标量与向量化求值。LIKE 标量路径调用 `EvalString`/`EvalInt`，LIKE 和数学批处理路径调用 `VecEval*`，并通过 `ExprRef = Arc<dyn LegacyExpression>` 共享参数表达式。
3. 用 `LiteralExpression` 提供严格常量或逐行测试数据，并实现单元素广播、NULL 传播和有限的类型检查。它是当前唯一的 `LegacyExpression` 实现，主要承担迁移内核测试输入，而不是一般 SQL 表达式树构造。
4. 用 `EvalContext` 收集可跨克隆共享的警告，用 `EvalError` 统一普通消息、DOUBLE/BIGINT 溢出和 Decimal 错误。
5. 用 `MathBase` 保存数学签名共用的参数、MySQL 兼容随机数发生器和返回 DECIMAL 精度；`builtin_math_vec.rs` 的签名宏及多个专用签名在此基础上实现批量计算。

## 主要符号

- `ConstLevel::{ConstNone, ConstOnlyInContext, ConstStrict}`：按可折叠程度排序。枚举派生 `Ord`，所以 `builtin_like.rs::builtinLikeSig::evalInt` 可以用 `>= ConstOnlyInContext` 判定模式与转义符能否在同一上下文内缓存。
- `Row(pub usize)`：Go `chunk.Row` 的最小替身，仅携带逻辑行下标。
- `Chunk { rows }`、`Chunk::new`、`Chunk::NumRows`：只表达批次行数，不保存输入列、选择向量或容量信息。
- `EvalContext { warnings }`：内部为 `Arc<Mutex<Vec<String>>>`；`append_warning` 追加诊断，`warnings` 返回克隆快照。
- `EvalError` 与 `Result<T>`：`Display` 为两个溢出变体生成 MySQL 1690 风格文本；`From<DecimalError>` 保留 decimal 库错误。
- `FieldType { unsigned }`：仅保留整数有符号性。`LiteralExpression::uints` 把 `u64` 位模式转换为 `i64` 存储，同时将该标记置为 `true`。
- 私有 `ColumnKind`：标记列尚未初始化或当前是 Int、Real、Decimal、String。它保护 `MergeNulls` 只能作用于定长结果列。
- `Column`：分别保存 `nulls`、`ints`、`reals`、`decimals`、`strings`。`ResizeInt64`/`ResizeFloat64`/`ResizeDecimal` 重建定长列，`ReserveString` 清空并预留变长列，`AppendNull`/`AppendString` 追加字符串槽位，`MergeNulls` 合并 NULL。
- `LegacyExpression: Send + Sync`：声明 `EvalString`、`EvalInt`、四种 `VecEval*`、`ConstLevel` 和 `GetType`。`ExprRef` 用 `Arc` 承载动态分发对象。
- 私有 `LiteralValues` 与公开 `LiteralExpression`：保存四种可空值向量、字段类型和常量级别；构造器区分严格常量与非常量列。
- `LiteralExpression::index`：若目标行存在则返回该行；若向量长度恰为 1，则向任意越界行广播第 0 项；否则返回 `None`。
- `MathBase { args, mysql_rng, ret_decimal }`：`new` 使用种子 0，`with_seed` 建立 `MysqlRng`，`with_ret_decimal` 以 builder 方式设置结果小数位数。

## 执行流程

以字面量的向量求值为例：

1. 调用方用 `Chunk::new(n)` 指定结果行数，并传入可复用的 `Column`。
2. `VecEvalInt`、`VecEvalReal` 或 `VecEvalDecimal` 先调用相应 `Resize*`，把结果列切换到正确种类、初始化 `n` 个非 NULL 槽位和默认值。
3. 实现匹配 `LiteralValues` 的实际变体；类型不匹配立即返回 `EvalError::Message`。
4. 每行经 `LiteralExpression::index` 读取。`Some(value)` 写入类型缓冲，SQL NULL 或缺失行则用 `SetNull` 标记；单元素向量会广播到整个 Chunk。
5. `VecEvalString` 先 `ReserveString`，再逐行复用标量 `EvalString` 的规则，通过 `AppendString` 或 `AppendNull` 同步增长字符串缓冲和 NULL 数组。

真实消费链有两条代表性路径：

- LIKE：`builtin_like_vec.rs::builtinLikeSig::vecEvalInt` 依次对三个 `ExprRef` 调用 `VecEvalString`/`VecEvalInt`，创建 Int 结果列，使用 `Column::MergeNulls` 合并值、模式、转义列的 NULL，最后逐行编译通配模式并写入 0/1。标量 `builtin_like.rs::builtinLikeSig::evalInt` 则使用 `Row`、`ConstLevel` 和标量求值方法，并在上下文常量条件满足时缓存模式。
- 数学函数：`builtin_math_vec.rs` 的 `define_signature!` 生成持有 `MathBase` 的签名；例如 `unary_real` 先让 `base.args[0]` 原地填充 Real 结果列，再跳过 NULL、处理定义域/溢出、写回数值或标 NULL。多参数内核用临时 `Column` 求参数并通过 `MergeNulls` 传播 NULL。

## 数据与状态

`Column` 采用“类型标签 + 多组独立 Vec”的简化布局，只有 `kind` 对应的那组值缓冲在当前操作中有效。所有 `Resize*` 都清空旧值和 NULL 状态；`ReserveString` 只预留容量，不创建行，之后每次追加必须同时增长 `nulls` 与 `strings`。公开访问器直接按下标索引，因此调用方必须保证行号小于列长度。

NULL 使用 `true` 表示。`MergeNulls` 对每行执行逻辑或：只要任一输入列为 NULL，结果就为 NULL；它不会修改值缓冲，因此 NULL 行中的默认值没有业务语义。

`LiteralExpression` 的 `const_level` 由构造器固定：`constant_string`/`constant_int` 是 `ConstStrict`，`strings`/`ints`/`uints`/`reals`/`decimals` 是 `ConstNone`。单元素非常量构造器虽然会广播，但仍保持 `ConstNone`；调用方不能仅凭长度推断可缓存性。

`EvalContext` 的克隆共享同一个警告向量；`warnings()` 返回快照，后续追加不会改变已返回的 Vec。`MathBase::clone` 也会克隆 `Arc<MysqlRng>`，因此克隆签名共享 RNG 状态而非重新播种。`ret_decimal` 是普通值字段，clone 后相互独立。

## 依赖与调用关系

模块内下游依赖只有两项：标准库的 `fmt`、`Arc`、`Mutex`；`mathutil::MysqlRng`/`NewWithSeed` 提供 MySQL RAND 序列；`types_dependency::decimal::mydecimal::{MyDecimal, DecimalError}` 提供 DECIMAL 存储和错误。它们分别由 `pkg/expression/Cargo.toml` 中的 `mathutil = astersql-util-mathutil` 与 `types-dependency = astersql-types` 声明。

RustCodeGraph 将目标识别为 455 行、86 个符号的已索引文件；精确查询确认 `LegacyExpression`、`LiteralExpression` 和 `MathBase` 定义在本文件。图的精确 `callers/callees` 对这些动态分发类型未返回边，实际引用用模块级搜索补齐：生产代码仅有 `builtin_like.rs`、`builtin_like_vec.rs`、`builtin_math_vec.rs` 直接导入本模块；独立测试为 `legacy_vectorized_runtime_test.rs`、`builtin_like_test.rs`、`builtin_like_vec_test.rs`、`builtin_math_vec_test.rs` 及合并迁移回归文件 `builtin_like_vec_15_aster_unit_test.rs`。

更上游的数学向量签名被 `builtin_math.rs`、`pb_to_expr_runtime.rs` 和 `planner_bridge.rs` 等模块接线；因此本文件通过 `builtin_math_vec.rs` 间接位于 protobuf/规划器表达式到批量数学求值的路径上。LIKE 运行时目前由 LIKE 内核及其测试使用。由于 `lib.rs` 声明的是私有 `mod` 而非 `pub mod`，其他 crate 不能直接依赖这些兼容类型。

## 错误处理与边界

- `LiteralExpression` 的标量接口只支持 String 和 Int；Real/Decimal 只有向量接口。调用错误的类型接口返回 `EvalError::Message`，不会隐式转换。
- `LiteralExpression::index` 对多元素向量的越界行返回 `None`，向量求值随后把它解释为 SQL NULL；长度为 0 时也得到 NULL。长度为 1 时则广播。这是当前兼容层的明确行为，扩展时不能把越界统一改成 panic 而不更新调用方和测试。
- `Column::IsNull`、`SetNull` 和各值切片依赖 Rust 下标检查，越界会 panic。`legacy_vectorized_runtime_test.rs::column_row_access_rejects_out_of_bounds_like_go_chunk_column` 固化了这一点。
- `Column::MergeNulls` 要求结果列为 Int/Real/Decimal，且所有输入列长度与结果一致；违反条件使用 `assert!`/`assert_eq!` panic。字符串结果列即使没有输入列也不允许调用。相应独立测试覆盖长度不一致和变长结果列。
- `EvalContext` 和 `builtin_like.rs` 的互斥锁使用 `expect`；若持锁期间发生 panic 导致锁中毒，后续访问会继续 panic，而非返回 `EvalError`。
- `EvalError::{DoubleOverflow, BigIntOverflow}` 的展示文本包含 `[types:1690]`；数学内核用它们表示 EXP/COT/POW/ABS 等溢出。Decimal 错误通过 `From` 原样进入 `EvalError::Decimal`。
- 该层没有选择向量、时态/JSON/Duration/Vector 求值、完整字段类型或一般转换规则。把它用于新的完整表达式功能之前，应先判断是否应接入主运行时，而不是继续扩大遗留门面。

## 并发与资源生命周期

`LegacyExpression` 强制实现者满足 `Send + Sync`，`ExprRef` 以 `Arc` 共享表达式对象。`LiteralExpression` 本身不可变，适合跨线程读取。`EvalContext` 用 `Arc<Mutex<_>>` 共享警告，克隆上下文不会隔离警告；锁只覆盖一次追加或一次快照复制。

`MathBase.mysql_rng` 也是共享的 `Arc`。`MysqlRng` 的具体同步与内部可变性由 `astersql-util-mathutil` 保证；本文件不额外加锁。`MathBase` 的 clone 共享同一个随机序列，这与 Go `builtinRandSig::Clone` 共享 `mysqlRng` 的做法一致，也意味着并发或克隆后的调用次序会影响后续随机值。新增随机函数时必须先核对是否需要共享序列、按行重播种或独立状态。

`Column` 与 `Chunk` 没有内部同步；批量求值通过 `&mut Column` 保证单个结果缓冲在一次调用中独占。临时列随 `vecEval*` 栈帧释放，底层 Vec 自动回收；当前实现没有列池、显式 allocator、异步任务、通道或事务生命周期。

## 与 Go 版本的对应关系

Go `pkg/expression/expression.go` 的 `ConstLevel` 三个等级与本文件一一对应，`VecExpr`/`Expression` 也提供 `VecEvalInt`、`VecEvalReal`、`VecEvalString`、`VecEvalDecimal` 和标量接口。但 Go 接口还包含 Time、Duration、JSON、VectorFloat32、通用 Datum、克隆/相等/遍历/索引解析/跨会话安全等能力；`LegacyExpression` 只移植 LIKE 和数学内核当前需要的子集。

Go `pkg/expression/constant.go` 的 `Constant::VecEval*` 通过 `genVecFromConstExpr` 生成整列常量，或委托 deferred expression。本文件的 `LiteralExpression` 用单元素广播模拟常量列，也允许 Vec 直接表达逐行输入，但没有参数标记、deferred expression 或完整类型推断，因此更接近迁移测试适配器。

Go `pkg/util/chunk/column.go::Column::MergeNulls` 要求结果为定长列、所有列等长，并逐位合并 NULL bitmap。本文件保留相同的前置条件和 panic 语义，但使用 `Vec<bool>` 按行合并，而不是 Arrow 风格 bitmap 和统一 data/offsets 布局。独立 Rust 测试专门对齐了越界、长度不一致和变长列拒绝行为。

`MathBase` 对应 Go 数学签名中共享的 `baseBuiltinFunc` 字段，加上 `builtinRandSig.mysqlRng` 及部分 DECIMAL 签名需要的精度信息。Go `builtinRandSig::Clone` 复用同一 RNG；Rust 的 `Arc<MysqlRng>` 保留这种共享。它不是 Go `baseBuiltinFunc` 的完整结构，未包含返回类型、protobuf code、buffer allocator 等元数据。

## 扩展指南

新增只服务于这组遗留内核的求值类型时，应同步修改 `ColumnKind`、`Column` 的存储/重置/访问器、`LegacyExpression` 方法、`LiteralValues`、构造器和 `LiteralExpression` 实现；测试必须放在独立的 `*_test.rs` 文件，不要内嵌进生产文件。优先扩展 `legacy_vectorized_runtime_test.rs` 的容器不变量，并在实际消费者对应的 `builtin_*_test.rs` 增加端到端边界用例。

新增数学签名通常在 `builtin_math_vec.rs` 中复用 `MathBase`，并根据 Go 同名签名确认参数求值顺序、NULL 传播、警告与 1690 溢出。需要随机状态时必须明确 clone 是否共享 RNG；需要 DECIMAL 精度时用 `with_ret_decimal`，并覆盖精度、舍入和错误传播。不能只让代码编译或返回默认值来代替 Go 行为。

扩展 `Column::MergeNulls` 到 String 等变长类型前，应先核对 Go `chunk.Column` 的固定/变长约束；当前消费者会依赖对 String 结果列的拒绝。改变 `LiteralExpression::index` 时要分别测试空向量、多行向量越界、单元素广播和显式 NULL，避免把“缺少行”与严格常量广播混淆。

如果新功能要服务于一般 SQL 表达式、更多求值类型或跨 crate 调用，应优先接入现有主表达式/Chunk 类型，而不是公开或无限扩张这个私有兼容模块。任何新增生产 Rust 实现都应继续保留/添加 `// Copyright 2026 AsterSQL.`，并与对应 Go 逻辑及独立测试同步。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/expression/legacy_vectorized_runtime.rs` 与两段 `node --file` 输出核对了全文件 455 行和 86 个符号；`query` 精确定位了 `LegacyExpression`、`LiteralExpression`、`MathBase`。精确 `callers/callees` 没有返回动态分发边，因此调用关系再由下列源码引用核验。
- 目标与装配：`pkg/expression/legacy_vectorized_runtime.rs`、`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`。
- Rust 直接消费者：`pkg/expression/builtin_like.rs`、`pkg/expression/builtin_like_vec.rs`、`pkg/expression/builtin_math_vec.rs`；上游数学接线引用还包括 `pkg/expression/builtin_math.rs`、`pkg/expression/pb_to_expr_runtime.rs`、`pkg/expression/planner_bridge.rs`。
- Rust 独立测试：`pkg/expression/legacy_vectorized_runtime_test.rs` 验证列越界与 `MergeNulls` 前置条件；`builtin_like_test.rs`/`builtin_like_vec_test.rs` 验证标量缓存、逐行匹配和 NULL；`builtin_math_vec_test.rs` 验证警告、NULL、舍入/截断、溢出、常量填充和 RNG 范围。
- Go 对照：`pkg/expression/expression.go`（`VecExpr`、`ConstLevel`、`Expression`）、`pkg/expression/constant.go`（常量向量求值）、`pkg/util/chunk/column.go`（`Column`、`MergeNulls`）、`pkg/expression/builtin_math.go`（RAND 状态与 clone）。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付结构通过任务文件指定的 11 章节命令检查；内容人工复核重点是模块边界、动态分发调用、NULL/越界语义、锁与 RNG 共享状态，以及 Go 对照的明确差异。

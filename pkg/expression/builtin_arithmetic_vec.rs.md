# `pkg/expression/builtin_arithmetic_vec.rs`

## 文件定位

本文件是 `astersql-expression` crate 内的向量化算术内核。`pkg/expression/lib.rs:171-172` 通过 `#[path = "builtin_arithmetic_vec.rs"] mod builtin_arithmetic_vec_kernel;` 将它作为私有模块挂载；`pkg/expression/Cargo.toml` 则确认 crate 名称、`lib.rs` 入口，以及这里直接使用的 `types-dependency` 和 `thiserror` 依赖。

这里的“向量化”是对两列等长数据逐行批量计算，而不是接受完整表达式树或 `chunk::Chunk`。仓库搜索只发现测试模块通过 `pub use crate::builtin_arithmetic_vec_kernel::*` 使用这些公开符号，未发现非测试 Rust 调用者。因此当前可确认的状态是：算术内核已经实现、被 crate 编译并由独立单测覆盖，但尚无证据表明它已接入 Rust 表达式执行主链。Go 版本的完整主链位置在 `pkg/expression/builtin_arithmetic_vec.go`，它直接求值参数表达式并管理 chunk 缓冲列；这部分不能视为 Rust 已接线能力。

## 核心职责

- 用 `Vector<T>`、`IntVector` 表示带 NULL 位图的 REAL、DECIMAL 和整数列，并在运算前验证左右列长度、合并 NULL。
- 为 20 个 Go 同名算术签名提供 `vectorized() == true` 标记，并为其中的 REAL、DECIMAL、整数加减乘除、整除和取模签名实现逐列求值。
- 保留 MySQL/TiDB 的整数位模式语义：有符号和无符号整数都存入 `i64` 通道，由 `IntVector::unsigned` 决定解释方式；混合符号运算按组合独立检查溢出。
- 通过 `EvalContext` 控制除零、DECIMAL 截断、除法精度增量、结果小数位和 `NO_UNSIGNED_SUBTRACTION` 行为。
- 把底层 DECIMAL/整数错误归一为 `ArithmeticError`，同时保证 NULL 行不会触发本行本应忽略的算术错误。

## 主要符号

- `ArithmeticError`：模块的统一失败类型，区分目标类型溢出、除零、DECIMAL 截断、底层 `DecimalError` 和输入长度不一致。`overflow` 与 `map_integer_overflow` 负责补充运算符上下文。
- `EvalContext`：轻量、可复制的策略集合。默认 `div_precision_increment = 4`，除零和截断不作为硬错误，且不强制无符号减法转有符号。
- `Vector<T>`、`RealVector`、`DecimalVector`：值数组和并行 NULL 位图的通用容器；`from_options` 把 NULL 槽位填为 `T::default()`，`options` 再恢复为 `Option<T>`。
- `IntVector`：以 `Vec<i64>` 保存原始 64 位模式，以 `unsigned` 标志决定 `options_i64` 或 `options_u64` 的解释。`signed`、`unsigned` 是公开构造器，`from_raw` 仅供本模块构造结果。
- `Vectorized` 与 `vectorized_signatures!`：批量声明零大小签名类型，并为每个类型提供默认返回 `true` 的能力标记。宏列出的 20 个类型与 Go 向量化签名命名对应。
- `real_binary`：REAL 加、减、乘的共同循环。加减拒绝所有非有限结果，乘法只拒绝无穷大而保留 NaN，与 Go 的 `IsFinite`/`IsInf` 差异一致。
- `decimal_binary`：DECIMAL 加、减、乘的共同循环；底层算术写入临时 `MyDecimal`，再由 `handle_decimal_status` 应用上下文策略。
- `prepare_int_result`：统一检查长度、合并 NULL，并以右列值作为结果缓冲初值；后续整数循环只覆盖非 NULL 行。
- `minus_overflows`：逐字保留 Go `overflowCheck` 的包装减法和符号组合判定，是 `BuiltinArithmeticMinusIntSig` 的关键兼容逻辑。
- 各 `BuiltinArithmetic*Sig::vec_eval_*`：公开求值入口。其输入已是类型化列，而非表达式节点或 chunk。

## 执行流程

1. 调用方构造 `EvalContext` 和两列输入；REAL/DECIMAL 使用 `Vector::from_options`，整数根据字段无符号属性选择 `IntVector::signed` 或 `IntVector::unsigned`。
2. 每个入口先通过 `ensure_same_len` 拒绝长度不一致，再用 `merged_nulls` 做逐行 OR。任一输入为 NULL 的行直接跳过计算。
3. REAL 加减乘交给 `real_binary`，除法和取模单独检测零除数。允许除零时把结果行设为 NULL；严格模式立即返回 `DivisionByZero`。REAL 除法和乘法产生无穷大时返回 DOUBLE 溢出。
4. DECIMAL 加减乘经 `decimal_binary` 调用 `DecimalAdd`、`DecimalSub` 或 `DecimalMul`。乘法显式关闭截断硬错误，匹配 Go 忽略 `ErrTruncated` 的规则。DECIMAL 除法额外传入 `div_precision_increment`，并在结果实际小数位不足 `result_decimal` 时用 `ModeHalfUp` 舍入；除法和取模都把可容忍的除零转成 NULL。
5. 整数取模按 UU、US、SU、SS 四个签名分别解释位模式；负的有符号被除数保持负余数，`i64::MIN % -1` 特判为 0，避免 Rust 运算溢出并匹配 Go。
6. 整数加法在一个 `match (left.unsigned, right.unsigned)` 中覆盖四种符号组合；减法由 `no_unsigned_subtraction` 决定结果是否强制为有符号，再调用 `minus_overflows`；有符号和无符号乘法分别使用对应宽度的 `checked_mul`。
7. 整数 `DIV` 同样按四种符号组合分派；混合符号与 SS 分支复用 `types_dependency::file_group::overflow` 的 `DivUintWithInt`、`DivIntWithUint`、`DivInt64`。DECIMAL `DIV` 先算定点商，再转 `u64`/`i64`，并保留无符号结果位于 `(-1, 0]` 时返回 0 的 Go 特例。

## 数据与状态

所有列都维护“值数组长度等于 NULL 位图长度”的隐含不变量。公开构造器同时生成两者，运算结果也只由 `merged_nulls` 或等长的零值数组构造；但字段为私有且 `is_null(row)` 直接索引，因此调用者仍必须使用有效行号。

NULL 槽位中的值只是占位数据，不具备 SQL 语义。运算循环必须先检查合并后的 NULL 位；例如溢出值恰好位于 NULL 行时会被跳过。`IntVector` 的 `i64` 值可能代表大于 `i64::MAX` 的 `u64`，不能脱离 `unsigned` 标志解释。运算方法返回新列，不修改输入列；为减少初始化工作，REAL/DECIMAL 通常克隆左列值，整数通常克隆右列值，再覆盖非 NULL 行。

`EvalContext` 不保存会话对象，只复制本次计算所需策略。`result_decimal` 为负时 DECIMAL 除法按 0 处理比较阈值；实际 Round 只在当前小数位小于所需位数时执行，这一点来自 `BuiltinArithmeticDivideDecimalSig::vec_eval_decimal` 的明确分支。

## 依赖与调用关系

上游方面，`pkg/expression/lib.rs` 编译挂载内核；`pkg/expression/builtin_arithmetic_vec_test.rs` 在测试配置下重新导出它，`pkg/expression/builtin_arithmetic_vec_1_aster_unit_test.rs` 直接构造各签名并调用 `vec_eval_real`、`vec_eval_decimal`、`vec_eval_int`。RustCodeGraph 对 `BuiltinArithmeticIntDivideDecimalSig` 的查询只解析到 Go 同名符号，且对 Rust `vec_eval_int` 无结果；仓库级 `rg` 也未找到这些算术签名的非测试 Rust 调用，故生产主链调用关系目前未验证。

下游方面，DECIMAL 运算依赖 `types_dependency::decimal::mydecimal` 的 `MyDecimal`、`DecimalAdd/Sub/Mul/Div/Mod`、`Round` 和数值转换；整数整除依赖 `types_dependency::file_group::overflow` 的三个兼容辅助函数。`thiserror::Error` 为 `ArithmeticError` 生成错误展示。Cargo 中这些分别由 `types-dependency = astersql-types` 和 `thiserror = "2"` 提供。

与 Go 相比，Rust 入口不负责调用两个子表达式、不访问字段类型标志、不持有 `bufAllocator`，也不接受 `chunk::Chunk`/`chunk::Column`；这些职责已被前置为调用方提供类型化输入列和 `EvalContext`。因此新增生产分派时，需要在更高层补齐表达式求值、字段无符号属性提取、警告/错误上下文映射和列缓冲生命周期，而不能只调用本文件就宣称完成主链接线。

## 错误处理与边界

- 长度不同始终在任何逐行计算前返回 `LengthMismatch`，避免 `zip` 截断造成静默丢行。
- 除数为零时由 `handle_division_by_zero` 决策：严格上下文返回错误，宽松上下文把当前结果置 NULL。已是 NULL 的行不会检查除数。
- REAL 加减将 NaN 和无穷都视为溢出；REAL 乘法只把无穷视为溢出，NaN 保留；REAL 除法只拒绝无穷，REAL 取模没有额外非有限检查。这是当前代码和 Go 对照的具体行为，不应被统一“简化”。
- `handle_decimal_status` 在宽松模式忽略 `DecimalError::Truncated`，但 `TruncatedWrongValue` 仍映射为 `TruncatedDecimal`；Overflow 映射为 DECIMAL 溢出，其余错误保留在 `ArithmeticError::Decimal`。DECIMAL 取模当前直接包装非除零错误，没有走该统一策略。
- 整数运算必须按位模式及符号组合处理。特别是 US/SU 加法、`NO_UNSIGNED_SUBTRACTION`、`i64::MIN / -1`、无符号 DECIMAL DIV 的负小数结果，均有专门路径。
- 错误表达式文本统一写成 `(lhs <op> rhs)`，不像 Go 会包含实际表达式字符串；这是诊断信息精度上的已知差异。

## 并发与资源生命周期

本文件没有全局可变状态、锁、通道、异步任务或事务。签名类型为零大小、`EvalContext` 为 `Copy`，所有工作数据由调用栈和返回的 `Vec`/`MyDecimal` 拥有；从实现上看，同一签名值可在多个线程独立调用，但文件没有声明额外的并发协议。

每次运算都会为结果 NULL 位图分配新 `Vec<bool>`，并克隆一侧值数组；DECIMAL 行还会创建临时 `MyDecimal`。这些资源在返回值或错误退出时由 Rust 所有权自动回收。Go 对照版从 `bufAllocator` 借用右侧或左右缓冲，并用 `defer put` 归还；Rust 当前没有相同的池化生命周期，因此接入高吞吐生产路径前应单独评估分配量，不能直接假设与 Go 有相同性能特征。

## 与 Go 版本的对应关系

`pkg/expression/builtin_arithmetic_vec.go` 定义同名的 20 个向量化签名，Rust 的 `vectorized_signatures!` 保留这组分派标识。REAL、DECIMAL、四种整数取模、整数加减乘、整数 `DIV` 与 DECIMAL `DIV` 的分支结构均能在 Go 文件找到直接对应；`pkg/expression/builtin_arithmetic_vec_1_aster_unit_test.rs` 又覆盖 NULL、除零、溢出、四种符号组合、`NO_UNSIGNED_SUBTRACTION`、DECIMAL 舍入和 `(-1, 0] -> 0` 特例。

两版职责边界并不相同。Go `vecEval*` 从 `b.args` 求值到 chunk 列，合并 NULL，读取字段 flag 和 SQL mode，并使用会话 `EvalContext` 的错误上下文；Rust 将这些输入压缩为自有 `Vector`/`IntVector` 和布尔策略，返回自有 `ArithmeticError`。Go 错误中包含原表达式文本，Rust 仅保留通用 lhs/rhs 占位。Go 基准/表驱动测试在 `builtin_arithmetic_vec_test.go` 覆盖集成式向量表达式框架，Rust 独立测试验证内核算法，但不等价于 Go 的 chunk、allocator 或表达式分派集成覆盖。

## 扩展指南

新增算术签名时，先在 `vectorized_signatures!` 增加与 Go 分派表一致的类型，再根据结果域选择公共骨架或新增专门入口。可复用 `real_binary`/`decimal_binary` 的操作必须确认非有限值、截断和溢出策略完全一致；若策略不同，应像 REAL 除法或 DECIMAL 取模一样显式实现，避免把差异隐藏在通用闭包中。

整数扩展必须明确输入四种符号组合、结果符号和位模式，优先使用 checked 运算或 `types_dependency` 的兼容辅助函数。任何变更都应保持“先合并 NULL、再算术”的顺序，并为 `LengthMismatch`、NULL 行中的危险值、零除数和边界值补充用例。测试逻辑应继续放在独立文件 `pkg/expression/builtin_arithmetic_vec_1_aster_unit_test.rs`（由 `builtin_arithmetic_vec_test.rs` 挂载），不要内嵌到生产文件；生产文件尾部现有 `#[cfg(test)]` 仅是对独立测试模块的挂载。

若目标是接入真实 SQL 执行，还需修改本文件之外的表达式分派层：把参数表达式/Chunk 转换为这些列类型，从字段元数据传入无符号标志，从会话上下文生成策略，并把 `ArithmeticError` 映射回 TiDB 错误/警告。接线后应增加覆盖真实表达式入口的独立集成测试，并比较 Go 的 `builtin_arithmetic_vec_test.go`；尤其关注额外分配、错误文本、warning 语义和 DECIMAL 精度。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/expression/builtin_arithmetic_vec.rs` 分段读取全部 993 行，确认 51 个索引符号、20 个签名、各公共辅助函数和文件尾测试挂载。
- 索引与调用证据：`rustcodegraph status` 显示项目索引包含 11,467 个文件；`query BuiltinArithmeticIntDivideDecimalSig` 找到 Go 定义/方法，`query vec_eval_int --kind method` 未找到 Rust 方法。随后用精确仓库搜索补足宏生成类型的索引缺口，并确认除测试模块外没有本文件签名的 Rust 调用者。
- crate 与模块证据：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs:171-172`、`pkg/expression/lib.rs:429-432`；目标包未发现 `doc.go`。
- Go 对照：`pkg/expression/builtin_arithmetic_vec.go` 的全部 `vectorized`/`vecEval*` 符号纲要及 REAL、DECIMAL、整数减法、整数/DECIMAL DIV、整数加法关键实现；`pkg/expression/builtin_arithmetic_vec_test.go` 的 `TestVectorizedBuiltinArithmeticFunc` 与 `TestVectorizedDecimalErrOverflow`。
- Rust 测试证据：`pkg/expression/builtin_arithmetic_vec_test.rs` 和 `pkg/expression/builtin_arithmetic_vec_1_aster_unit_test.rs`，后者包含 8 个独立测试，覆盖上述算法边界和全部签名的 `vectorized()` 声明。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务规定命令验证恰有 11 个固定二级标题，并人工复核未把未接线能力写成已支持。

# `pkg/expression/builtin_arithmetic.rs`

## 文件定位

该文件是 `astersql-expression` crate 中的标量算术语义内核，对应 Go 文件 `pkg/expression/builtin_arithmetic.go`。crate 根 `pkg/expression/lib.rs:168-172` 通过 `#[path = "builtin_arithmetic.rs"] mod builtin_arithmetic_kernel;` 将它作为私有模块编译；`pkg/expression/Cargo.toml` 指定 crate 名为 `astersql-expression`、入口为 `lib.rs`，并直接提供本文件所用的 `bigdecimal`、`num-traits` 和 `thiserror` 依赖。

当前 Rust 文件不是 Go 表达式框架的逐类型 `functionClass`/`builtinFunc` 实现，而是在自有的 `Expression`、`FieldType`、`EvalContext` 边界上实现常量二元算术。精确引用搜索只找到 crate 根的私有模块声明、`pkg/expression/builtin_arithmetic_test.rs` 的测试侧再导出，以及 `pkg/expression/builtin_arithmetic_2_aster_unit_test.rs` 的直接调用；没有证据表明 `ArithmeticExpr` 已接入 Rust 的通用表达式树、行求值、函数注册表或 protobuf 下推路径。因此它目前应理解为“可执行且有对等测试的迁移内核”，不能写成已经替代 Go 生产主链。

文件底部 `builtin_arithmetic.rs:1149-1152` 还在 `cfg(test)` 下直接挂载独立测试文件 `builtin_arithmetic_2_aster_unit_test.rs`；crate 根另通过 `builtin_arithmetic_test.rs` 再挂载同一组 Go 对等用例。测试逻辑位于独立文件，没有内嵌进生产源文件。

## 核心职责

1. 建模算术所需的最小类型和值边界：`EvalType`、`TypeCode`、`FieldType`、`EvalValue` 与常量 `Expression`（`builtin_arithmetic.rs:36-369`）。
2. 按 Go 的数值上下文规则推导时间、二进制字面量、BIT、混合类型和普通类型应走的求值类别，入口是 `numeric_context_result_type`（`builtin_arithmetic.rs:371-399`）。
3. 推导 `+`、`-`、`*`、`/`、`DIV`、`%` 的结果 `flen`、`decimal` 和 unsigned 属性，主要由 `set_flen_decimal_for_real_or_decimal`、`set_type_for_div_decimal`、`set_type_for_div_real`、`set_type_for_mod` 完成（`builtin_arithmetic.rs:401-528`）。
4. 在 `ArithmeticExpr::build` 中按“向量优先，其次 REAL、DECIMAL、INT”的规则选择具体 `Signature`，并在 `ArithmeticExpr::eval` 中执行 NULL 传播及具体求值（`builtin_arithmetic.rs:625-800`）。
5. 保留 MySQL/TiDB 的整数位模式、四种有/无符号组合、溢出错误文本、除零策略、DECIMAL 精度和向量维度约束（`builtin_arithmetic.rs:802-1141`）。

本文件不负责 SQL 解析、函数名注册、从通用行中取值、向量化批处理、PB signature 编解码或下推。批处理实现另在 `pkg/expression/builtin_arithmetic_vec.rs`；Go 生产接线则位于 `builtin_arithmetic.go` 的各 `getFunction`、`builtinFunc` signature 及包级 `funcs` 注册链。

## 主要符号

- 常量 `UNSPECIFIED_LENGTH`、`MAX_INT_WIDTH`、`MAX_REAL_WIDTH`、`MAX_DECIMAL_SCALE`、`MAX_DECIMAL_WIDTH`（`builtin_arithmetic.rs:36-45`）：本地类型元数据的边界值，分别对应未指定长度、整数/REAL 最大显示宽度以及 DECIMAL 最大 scale/precision。
- `EvalType` 与 `TypeCode`（`builtin_arithmetic.rs:47-68`）：前者表示运算分派类别，后者保留 Temporal、BinaryString、Bit、hybrid 等影响数值强制转换的源类型信息。
- `FieldType`（`builtin_arithmetic.rs:70-129`）：保存 `eval_type`、`type_code`、`flen`、`decimal`、`unsigned`、`hybrid`；`int`、`decimal`、`real`、`vector` 是构造器。
- `EvalValue`（`builtin_arithmetic.rs:131-142`）：承载 NULL、`i64`、`u64`、`f64`、`BigDecimal`、`Vec<f32>`、字节串和字符串。
- `Expression`（`builtin_arithmetic.rs:144-369`）：拥有一个常量值、字段类型、用于错误信息的显示文本，以及“常量二进制字面量”标志。公开构造器覆盖 signed/unsigned/real/decimal/vector/null/temporal/binary/bit/hybrid；`raw_i64`、`as_real`、`as_decimal`、`as_vector` 是内部强制转换入口。
- `is_constant_binary_literal`、`numeric_context_result_type`（`builtin_arithmetic.rs:371-399`）：区分常量二进制字面量与普通二进制串，并决定算术分派类型。
- `set_flen_decimal_for_real_or_decimal`（`builtin_arithmetic.rs:408-450`）：加减取两侧最大小数位并为进位增加一位，乘法累加小数位；结果再受 REAL/DECIMAL 上限约束。
- `set_type_for_div_decimal`、`set_type_for_div_real`、`set_type_for_mod`（`builtin_arithmetic.rs:476-528`）：分别推导除法与取模结果元数据。DECIMAL 除法使用 `div_precision_increment`，取模的 unsigned 属性跟随左操作数。
- `ArithmeticOp`（`builtin_arithmetic.rs:530-553`）：六种 SQL 运算符；私有 `symbol` 用于稳定生成错误表达式文本。
- `Signature`（`builtin_arithmetic.rs:555-581`）：23 个具体实现分支，其中整数取模按左右 unsigned 状态拆为四个签名，向量只支持加、减、乘。
- `ArithmeticWarning`、`ArithmeticError`（`builtin_arithmetic.rs:583-603`）：警告目前只有除零；错误覆盖溢出、除零、向量维度不等和无效类型。
- `EvalContext`（`builtin_arithmetic.rs:605-623`）：保存 `NO_UNSIGNED_SUBTRACTION` 等价开关、除法精度增量、除零是否升级为错误，以及累积警告。默认增量为 4，默认除零记警告并返回 NULL。
- `ArithmeticExpr`（`builtin_arithmetic.rs:625-1075`）：保存运算符、两个拥有所有权的操作数、已选签名和结果类型。`build` 是分派入口，`eval` 是执行入口，其余 `eval_*` 方法实现各类型路径。
- `subtraction_overflows`、`test_if_sum_overflows_ull`（`builtin_arithmetic.rs:1077-1126`）：移植 Go `builtinArithmeticMinusIntSig.overflowCheck` 及无符号加法溢出辅助逻辑。
- `decimal_precision`（`builtin_arithmetic.rs:1128-1141`）：从 `BigDecimal` 的整数与指数计算有效位数，用于执行后限制 65 位 DECIMAL 精度。

## 执行流程

典型调用分为构建和求值两段：

1. 调用者先用 `Expression::*` 构造两个带类型的拥有值。例如 `Expression::unsigned` 同时写入 `EvalValue::UInt` 和 unsigned `FieldType`；`Expression::named_signed` 允许错误文本保留列名式显示名。
2. `ArithmeticExpr::build(op, lhs, rhs, context)` 调用两次 `numeric_context_result_type`。Temporal 按 FSP 选择 Int/Decimal；常量 binary literal 和 BIT 走 Int；hybrid 走 Real；其他类型只保留 Int、Decimal、VectorFloat32，剩余归入 Real。
3. `build` 按运算符选择 `Signature` 和 `result_type`：
   - 加减乘先判向量，再判 REAL、DECIMAL，最后落到 INT；
   - 减法结果 unsigned 还受 `context.no_unsigned_subtraction` 抑制；
   - 普通除法只分 REAL 与 DECIMAL；
   - `DIV` 在两侧都是 Int 时走 `IntDivideInt`，否则先转 DECIMAL；
   - `%` 先分 REAL/DECIMAL，INT 再按两侧符号标志拆成四种签名。
4. `ArithmeticExpr::eval` 首先统一传播 NULL；任一操作数是 `EvalValue::Null` 就直接返回 `EvalValue::Null`，不会写入警告。
5. `eval` 按签名分派：整数加减乘分别进入 `eval_plus_int`、`eval_minus_int`、`eval_multiply_int`；同族 REAL/DECIMAL 签名复用 `eval_real`/`eval_decimal`；两类整除、四类整数取模、三类向量运算进入各自方法。
6. 整数路径把 `i64` 作为底层位模式，再结合左右 `unsigned` 判定合法范围。只有检查通过后才使用 wrapping 运算产生位模式，并由 `integer_result` 按结果类型包装为 `Int` 或 `UInt`。
7. REAL 路径先转 `f64`，除法或取模的右值为 `0.0` 时走统一除零策略；加减要求结果 finite，乘除拒绝 infinity，取模不额外判 overflow。
8. DECIMAL 路径先转 `BigDecimal`；除法按结果 scale 使用 `RoundingMode::HalfUp`，随后所有 DECIMAL 运算都用 `decimal_precision` 检查 65 位上限。
9. `DIV` 对纯整数保留四种 signedness 组合及 `i64::MIN / -1` 边界；DECIMAL `DIV` 依结果 unsigned 与否转为 `u64` 或 `i64`，商位于 `(-1, 0)` 的 unsigned 特例返回 0。
10. 向量加减乘先要求两个 `Vec<f32>` 等长，再逐维 `zip` 运算并收集新向量；维度不同返回 `ArithmeticError::VectorDimension`。

RustCodeGraph 的内部调用边与上述流程一致：`build -> numeric_context_result_type / set_flen_decimal_for_real_or_decimal / set_type_for_div_* / set_type_for_mod`，`eval -> eval_plus_int / eval_minus_int / eval_multiply_int / eval_real / eval_decimal / eval_int_divide / eval_decimal_int_divide / eval_int_mod / eval_vector`，`eval_minus_int -> subtraction_overflows -> test_if_sum_overflows_ull`。

## 数据与状态

`ArithmeticExpr` 完全拥有 `lhs`、`rhs` 与结果元数据；构建后签名固定，不在求值时重新进行类型分派。`Expression` 也是拥有值，DECIMAL、向量、字节串和字符串均存储在自身枚举中。`Clone` 派生会深拷贝这些容器，测试 `real_and_decimal_plus_minus_and_clone_keep_go_results` 验证克隆后的表达式仍可独立求值。

唯一的可变运行状态是由调用者传入的 `&mut EvalContext`。其中：

- `no_unsigned_subtraction` 与 `div_precision_increment` 在 `build` 阶段影响结果签名/类型；若构建后换用不同上下文求值，只有 `eval_minus_int` 会再次读取当前上下文的 `no_unsigned_subtraction`。调用者应保持构建与求值上下文设置一致。
- `division_by_zero_as_error` 在求值阶段决定返回错误还是 NULL。
- `warnings` 在非错误除零时追加 `ArithmeticWarning::DivisionByZero`，不会自动清空或去重。

`flen` 与 `decimal` 是类型元数据，不是运行值的存储限制。`UNSPECIFIED_LENGTH` 会沿推导传播；已知值由 `clamp_decimal` 和最大宽度常量限制。DECIMAL 实值由 `BigDecimal` 保存，运算结束后另做有效精度检查。

## 依赖与调用关系

上游与装配关系：

- `pkg/expression/lib.rs:169-170` 私有挂载 `builtin_arithmetic_kernel`，所以文件会随 `astersql-expression` 编译，但未从 crate 根公开再导出。
- `pkg/expression/builtin_arithmetic_test.rs:24` 仅在测试模块中 `pub use crate::builtin_arithmetic_kernel::*`；其 `go_parity` 子模块以及生产文件底部的测试子模块执行 `builtin_arithmetic_2_aster_unit_test.rs`。
- 精确 Rust 引用中，`ArithmeticExpr::build`/`eval`、`numeric_context_result_type` 等只由 `builtin_arithmetic_2_aster_unit_test.rs` 使用。当前没有生产调用方证据；`lib.rs` 的“被其他文件使用”不能等价为这些符号已经接入表达式主链。

下游依赖：

- 标准库 `std::fmt` 为 `EvalValue` 提供 Debug 风格的 `Display`。
- `bigdecimal::{BigDecimal, FromPrimitive, RoundingMode, ToPrimitive}` 负责 DECIMAL 表示、浮点转换、HalfUp 缩放以及整型转换。
- `num_traits::Zero` 为 DECIMAL 除零判断和零值比较提供接口。
- `thiserror::Error` 生成 `ArithmeticError` 的展示实现。
- 文件不依赖 crate 内通用 `Expression` trait、`types::FieldType`、chunk row 或 session context；这些概念由本文件的局部类型模拟。

Go 主链对照证据是 `builtin_arithmetic.go:30-64` 的接口断言、各运算 `getFunction`、逐类型 signature 的 `eval*`，以及该文件被 `builtin.go`、线程安全生成文件和向量实现引用。Rust 当前缺少对应注册和适配层，因此安全的文档结论是“算术内核已实现并被测试，生产接线未验证/未发现”。

## 错误处理与边界

- NULL：`ArithmeticExpr::eval` 在所有签名之前统一短路；这与 Rust 对等测试的整数加法 NULL 用例一致。
- 类型错误：`raw_i64`、`as_real`、`as_decimal`、`as_vector` 对不匹配的 `EvalValue` 返回 `ArithmeticError::InvalidType`。非有限 REAL 无法转 DECIMAL 时也走该错误。
- 整数溢出：加、减、乘分别根据左右 unsigned 组合检查；错误类型文本为 `BIGINT` 或 `BIGINT UNSIGNED`，表达式文本由两个操作数的 `display` 与运算符组成。`named_signed` 支持保留类似列名的文本。
- REAL 溢出：加减拒绝所有非有限结果；乘除仅把 infinity 当溢出。该差异来自 `eval_real` 的显式分支，NaN 在乘除路径不会被同一条件拒绝。
- DECIMAL：运算后有效精度超过 65 返回 `Overflow { ty: "DECIMAL" }`；除法按推导 scale 做 HalfUp 舍入。与 Go `MyDecimal` 的精确状态码、截断错误上下文相比，本地错误模型更小，不能推断所有 Go warning/truncation 行为已经覆盖。
- 除零：REAL/DECIMAL `/`、所有 `DIV` 和 `%` 都汇入 `division_by_zero`。默认追加一次 warning 并返回 NULL；`division_by_zero_as_error=true` 时返回 `ArithmeticError::DivisionByZero` 且不追加 warning。
- 整除：显式保护 `i64::MIN / -1`；混合 signedness 的负商按 Go 兼容规则可能返回 unsigned 0，也可能报 unsigned overflow。DECIMAL 商转换为 `i64/u64` 失败时转 overflow。
- 取模：右操作数为零走除零策略；四个整数签名保留“余数符号跟随左操作数”，并特判 `i64::MIN % -1 == 0` 以避免 Rust 溢出。
- 向量：只支持 `+`、`-`、`*`，且必须等长；`/`、`DIV`、`%` 不会构建向量专用签名。若一侧为向量而另一侧不是，构建仍会选向量签名，求值时由 `as_vector` 返回 InvalidType，而非在构建阶段拒绝。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、I/O 或外部资源句柄。一次运算的生命周期是：拥有值的 `Expression` 被移动进 `ArithmeticExpr`，构建结果可被克隆，`eval` 借用不可变表达式并独占借用可变 `EvalContext`，返回拥有的 `EvalValue` 或错误。

由于 warning 写入要求 `&mut EvalContext`，同一个上下文不能在安全 Rust 中被多个求值并发可变访问；并发调用应使用各自上下文或由上层显式同步。`ArithmeticExpr` 自身求值时不修改字段，但本文件未声明或验证跨 session 共享契约。Go signature 结构在 `builtin_arithmetic.go` 中明确要求新增字段保持线程安全或不可变；Rust 未来接入共享表达式主链时仍应重新审查其 `Send`/`Sync` 边界和上下文隔离，而不能只依据当前派生类型作生产共享承诺。

向量与 DECIMAL 运算会分配新值；`as_decimal` 对已有 DECIMAL 克隆，`eval_vector` 收集新 `Vec<f32>`。没有缓存或池化，资源随 Rust 所有权离开作用域自动释放。

## 与 Go 版本的对应关系

直接对应关系包括：

- Rust `is_constant_binary_literal` / `numeric_context_result_type` 对应 Go 同名 camelCase 函数（Go `builtin_arithmetic.go:66-102`），保留 Temporal FSP、常量二进制字面量、BIT、hybrid 的优先规则。
- Rust `set_flen_decimal_for_real_or_decimal` 对应 Go `setFlenDecimal4RealOrDecimal`（Go `:104-141`）；`set_type_for_div_decimal` / `set_type_for_div_real` 对应 Go divide function class 的两个类型辅助（Go `:143-164`）；`set_type_for_mod` 对应 Go `setType4ModRealOrDecimal`（Go `:983-1000`）。
- Rust `ArithmeticExpr::build` 把 Go 六个 function class 的 `getFunction` 分派压缩为一个枚举式入口；`Signature` 对应 Go 的逐类型 signature 结构，包括四种整数 `%` 组合和三种 VectorFloat32 signature。
- Rust 各 `eval_*` 对应 Go signature 的 `evalInt`、`evalReal`、`evalDecimal`、`evalVectorFloat32`；`subtraction_overflows` 是 Go minus signature 的 `overflowCheck` 的直接翻译。

重要差异与迁移限制：

- Go 操作数是通用 `Expression`，在求值时从 `chunk.Row` 取值；Rust 此文件的 `Expression` 只保存常量，不支持列、标量函数或按行变化的数据。
- Go function class 校验参数个数、调用 `newBaseBuiltinFuncWithTp`、写 PB code 并进入包级函数注册；Rust `ArithmeticExpr::build` 没有注册、arity、PB code 或下推接线。
- Go 使用共享的 `types.FieldType`、`types.MyDecimal`、`EvalContext`、SQLMode、errctx 和 `handleDivisionByZeroError`；Rust 使用局部简化类型、`BigDecimal` 和布尔策略。核心数值分支有对等测试，但完整错误码、warning 等级、截断处理和 session 语义并非一一等价。
- Go 各 signature 可 Clone 并声明跨 session 共享约束；Rust 是整个拥有值表达式派生 `Clone`，尚未与通用表达式生命周期一致。
- Go 向量使用 `types.VectorFloat32::{Add,Sub,Mul}`；Rust 直接遍历 `Vec<f32>`，明确自行检查维度。

Rust 独立测试 `builtin_arithmetic_2_aster_unit_test.rs` 覆盖类型推导、元数据、分派、NULL、四种符号组合、溢出、除零 warning/error、DECIMAL scale、DIV 截断、取模符号和向量维度。Go `builtin_arithmetic_test.go` 还验证真实函数注册/签名、PB code、chunk 行求值、Duration/Set/字符串强制转换以及真实列名错误信息；这些用例揭示了当前 Rust 内核之外仍需接线或扩充的语义表面。

## 扩展指南

- 新增运算符：同步扩展 `ArithmeticOp`、`symbol`、`Signature`、`ArithmeticExpr::build` 和 `eval`；按类型增加 `eval_*`，并在独立的 `pkg/expression/builtin_arithmetic_2_aster_unit_test.rs` 添加分派、NULL、边界、错误文本测试。不要把测试写回生产文件。
- 新增类型或转换规则：先更新 `EvalType`/`TypeCode`/`EvalValue`/`FieldType` 与 `Expression` 构造器，再修改 `numeric_context_result_type` 和相应 `as_*`。应同时覆盖普通值、NULL、非有限值、混合类型和二进制字面量，避免改变现有优先级。
- 调整 flen/decimal：修改对应 `set_*` 辅助，并与 Go `builtin_arithmetic.go` 的同名逻辑及 `builtin_arithmetic_test.go::TestSetFlenDecimal4RealOrDecimal` 对照；至少覆盖未指定长度、scale 上限、REAL 23 位、DECIMAL 65/30 上限和 unsigned 长度换算。
- 修改整数算法：必须保留左右 signedness 的完整矩阵，重点测试 `i64::{MIN,MAX}`、`u64::MAX`、负除数、`MIN / -1`、`MIN % -1` 与 `NO_UNSIGNED_SUBTRACTION`。减法应同步检查 `subtraction_overflows`，不可用单一 checked 运算替代混合符号语义。
- 修改除零：统一经 `division_by_zero`，同时验证 warning 模式和 error 模式；避免某一签名绕过上下文策略。
- 扩展向量：除维度不等外，还应明确混合标量/向量是否在 build 阶段拒绝。若加入除法或取模，需要定义逐维除零、NaN/Infinity 和错误聚合策略，再与 Go `types.VectorFloat32` 行为核验。
- 接入生产主链：不能只公开本模块。需要设计到 crate 通用 `Expression` trait、`BuildContext`/`EvalContext`、row/chunk、函数注册、PB signature、clone/thread-safety 以及向量化路径的适配，并把 Go `builtin_arithmetic_test.go` 中真实注册和行求值用例迁移到独立 Rust 测试。此项属于后续实现工作，不是当前文件已经具备的能力。
- 性能风险：`BigDecimal` 转换/克隆、逐次向量分配和动态错误文本都有成本；优化时必须保留精度、舍入、错误文本和所有 signedness 边界，不能仅以 benchmark 结果简化语义。

## 验证依据

本说明基于以下直接证据：

- 源文件：`pkg/expression/builtin_arithmetic.rs` 全部 1152 行，重点符号为 `numeric_context_result_type`、四个结果类型辅助、`ArithmeticExpr::{build,eval}`、全部 `eval_*`、`subtraction_overflows` 和 `decimal_precision`。
- crate 边界：`pkg/expression/Cargo.toml`；确认 crate 名/入口、`autotests = false`、`bigdecimal`、`num-traits`、`thiserror` 依赖及 `go-package = "pkg/expression"` 迁移元数据。
- 模块入口：`pkg/expression/lib.rs:168-172,428-432`；确认生产内核私有挂载及测试模块挂载。
- Rust 测试入口：`pkg/expression/builtin_arithmetic_test.rs`；确认只做内核再导出与独立测试挂载。
- Rust 对等测试：`pkg/expression/builtin_arithmetic_2_aster_unit_test.rs` 全部 478 行；确认实际覆盖的类型推导、元数据、分派、加减乘除、DIV、MOD、向量与错误边界。
- Go 对照：`pkg/expression/builtin_arithmetic.go` 全部 1386 行和 `pkg/expression/builtin_arithmetic_test.go` 全部 816 行；确认 Go function class/signature、行求值、PB code、类型强制转换、错误和并发约束。
- RustCodeGraph：`status` 显示本仓库索引包含 11467 个文件、307296 个节点和 1848419 条边；`files --filter pkg/expression` 确认目标及对照文件均已索引；`query` 唯一定位 `ArithmeticExpr`、`numeric_context_result_type`、`set_flen_decimal_for_real_or_decimal`、`subtraction_overflows`；文件节点和 callees 结果确认上述内部调用链。对同名方法的 callers 查询无法可靠消歧，因此外部接线结论改用 `lib.rs` 模块声明和精确 `rg` 引用交叉核验。
- 精确引用核验：在 `pkg/expression` 中排除目标文件后搜索 `ArithmeticExpr`、`numeric_context_result_type`、`subtraction_overflows` 与模块名，只发现 `lib.rs`、测试入口和 `builtin_arithmetic_2_aster_unit_test.rs` 的引用；因此文档明确标注生产主链接线未发现，而未将测试可执行性夸大为完整应用接入。

本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令确认目标文件存在且恰有 11 个固定二级章节，并人工复核所有“已支持”结论均能回指以上源码、调用边、Cargo、Go 或测试证据。

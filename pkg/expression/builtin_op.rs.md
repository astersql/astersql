# `pkg/expression/builtin_op.rs`

## 文件定位

本文件属于 Cargo crate `astersql-expression`（见 [`pkg/expression/Cargo.toml`](Cargo.toml)），在 [`pkg/expression/lib.rs`](lib.rs) 中以私有模块 `builtin_op_kernel` 装入。它把 Go [`pkg/expression/builtin_op.go`](builtin_op.go) 中与执行上下文无关的标量运算符语义拆成纯 Rust 数据类型、求值函数、函数类门面和签名选择逻辑。

当前接线必须区分两层：

- `builtin_op_kernel` 会随 library 编译，但没有从 crate 根公开导出；`lib.rs` 只在 `#[cfg(test)]` 下用 `mod builtin_op { pub use crate::builtin_op_kernel::*; }` 为测试提供兼容路径。
- 仓库搜索没有发现测试之外对本文件符号的引用。生产侧的函数注册与动态执行接口由 [`pkg/expression/builtin.rs`](builtin.rs) 作为 `expression_builtin` 模块提供，并由 crate 根公开 `formal_registry`。因此，本文件目前是已验证的移植语义内核和签名模型，不应被描述为已经取代 Go 完整表达式执行器或已经接入 Rust 生产注册链。

## 核心职责

本文件围绕以下四类职责组织：

1. 用 `EvalResult<T> = Result<Option<T>, EvalError>` 表达 Go 的 `(value, isNull, error)` 三元结果，其中 `Ok(None)` 是 SQL `NULL`，`Err` 是求值失败。
2. 实现逻辑与/或/异或、位运算、真假判断、一元 `NOT`、一元负号和 `IS NULL` 的无上下文求值语义，尤其保留 MySQL 三值逻辑、从左到右求值和短路边界。
3. 用 `EvalType`、`ScalarFuncSig`、`Signature` 和 `ExprMetadata` 表达 Go 函数类构造阶段的类型归并、PB 下推码、显示宽度、NULL 策略、警告和常量溢出信息。
4. 提供与 Go 函数类/签名大致一一对应的轻量门面，例如 `LogicAndFunctionClass`、`IsTrueOrFalseFunctionClass`、`BuiltinLogicAndSig`，让移植语义能够被独立测试；这些门面不持有 Go 的 `BuildContext`、`EvalContext`、表达式参数数组或行数据。

该文件不负责 SQL 解析、函数名注册、表达式树构造、行读取、向量化执行或会话警告写入；这些是周边表达式框架的职责。

## 主要符号

- `EvalResult<T>`：所有求值函数的统一返回形态。`Option` 与 `Result` 分开保存 SQL `NULL` 和错误，禁止用数值哨兵混淆两者。
- `EvalError`：包含输入求值失败 `Input`、数值越界 `Overflow`、类型不支持 `Unsupported` 和参数个数错误 `InvalidArity`。`EvalError::input` 主要供独立测试构造上游错误。
- `IntValue::{Signed, Unsigned}`：保存整数来源的有/无符号属性，一元负号必须依赖该属性判断 `BIGINT` 边界。
- `BooleanValue`：真值上下文支持的 `Int`、`Real`、`Decimal`、`VectorFloat32`、`Json` 五类值；私有 `is_zero` 集中定义零值判断。
- `EvalType`：函数类派发使用的简化求值类型，包括数值、时间、字符串、JSON 和向量类型。
- `TruthOp`、`BinaryOp`：分别描述真假测试以及逻辑/位二元操作。
- `ScalarFuncSig`：保存本文件能够选择的 PB 标识。向量真假测试刻意没有对应枚举项，`truth_signature` 会将其 `pb_code` 留为 `None`。
- `Signature`：函数构造结果的元数据，包含参数/返回类型、PB 码、`flen`、`decimal`、无符号标志、`keep_null`、常量溢出标志和警告。
- `ExprMetadata`：一元负号类型推断所需的最小表达式信息；`column` 与 `constant` 构造器通过 `is_column` 区分列宽保留规则。
- `logical_and`、`logical_or`、`logical_xor`：逻辑运算核心；右参数是 `FnOnce` 延迟求值器。
- `bit_and`、`bit_or`、`bit_xor`、`left_shift`、`right_shift`：经私有 `bit_binary` 统一传播错误/NULL，再执行整数操作；`bit_neg` 处理一元按位取反。
- `is_true`、`is_false`、`unary_not`：经 `BooleanValue::is_zero` 判定真假；前两者共享私有 `truth_test`。
- `handle_int_overflow`、`unary_minus_int`、`unary_minus_decimal`、`unary_minus_real`：执行一元负号的边界检查和具体类型求值。
- `is_null<T>`：参数求值成功时始终返回非 NULL 的 `0` 或 `1`，参数错误则原样向上传播。
- 函数类：宏 `binary_function_class!` 生成八个二元函数类；另有 `IsTrueOrFalseFunctionClass`、`BitNegFunctionClass`、`UnaryNotFunctionClass`、`UnaryMinusFunctionClass`、`IsNullFunctionClass`。它们先用 `validate_arity` 检查参数个数，再调用相应签名选择函数。
- 签名门面：`binary_int_signature!`、`truth_signature!`、`unary_not_signature!`、`is_null_signature!` 生成薄包装结构；各 `eval_*` 方法仅做类型映射并委托给核心求值函数。
- `binary_signature`、`bit_neg_signature`、`truth_signature`、`unary_not_signature`、`unary_minus_signature`、`is_null_signature`：复现 Go 构造阶段的类型和 PB 码选择。

## 执行流程

二元逻辑运算的流程如下：

1. 先对 `lhs: EvalResult<i64>` 使用 `?`，左侧错误立即返回。
2. `logical_and` 在左侧为非 NULL 的 `0` 时直接返回 `0`；`logical_or` 在左侧为非 NULL 的非零值时直接返回 `1`；`logical_xor` 在左侧为 NULL 时直接返回 NULL。以上分支都不会调用右侧闭包。
3. 只有结果尚未确定时才调用一次 `rhs: FnOnce()`。右侧错误立即返回。
4. `AND` 让任一确定的 `0` 胜过 NULL，`OR` 让任一确定的非零值胜过 NULL；`XOR` 只在两侧均非 NULL 时比较 `(value != 0)`。

位运算先通过 `bit_binary` 顺序取得两侧非 NULL 整数。任一侧为 NULL 或错误时不执行实际操作；左右移把被移数转成 `u64` 做位移，再转回 `i64`。移位量也按 `u64` 解释，若不小于 64（包括负 `i64` 转换成的大 `u64`）则显式返回 `0`，从而对齐 Go 的 64 位无符号移位结果而避免 Rust 的超宽移位行为。

真假判断先把具体输入映射成 `BooleanValue`。`truth_test` 在 `keep_null=true` 时保留 NULL；否则 NULL 作为假值返回 `0`。非 NULL 输入由 `is_zero` 判定：整数、浮点、十进制使用数值零，向量要求所有分量为零，JSON 只把能以数值 `0` 读取的值视为零。`is_false` 通过 `invert` 反转非 NULL 真值；`unary_not` 始终传播 NULL。

一元负号分为构造与求值：

1. `UnaryMinusFunctionClass::get_function` 校验一元参数后调用 `unary_minus_signature`。
2. 对整型常量，`handle_int_overflow` 把 `i64::MIN` 或大于 `2^63` 的无符号值识别为需要提升到 `Decimal` 的构造期溢出；恰好 `2^63` 仍可在运行期变成 `i64::MIN`。
3. `unary_minus_signature` 选择返回类型和 PB 码，计算 `flen`/`decimal`。有符号整型列和十进制列保持已有宽度，其他输入通常增加一位符号宽度；十进制结果最终限制到 `MYSQL_MAX_DECIMAL_WIDTH`（65）。
4. 运行期 `unary_minus_int` 再检查整数边界并生成 `EvalError::Overflow`；十进制用 `checked_mul(Decimal::NEGATIVE_ONE)`，浮点直接取负。

签名选择阶段还会执行类型归并：真假测试把时间、JSON、字符串归并为 `Real`；一元 `NOT` 把时间类归并为 `Int`、字符串归并为 `Real`，JSON 保留并记录布尔上下文警告文本；`IS NULL` 把 `Timestamp` 归并为 `Datetime`、把 `Json` 归并为 `String`。

## 数据与状态

本文件没有全局可变状态。唯一模块常量 `MYSQL_MAX_DECIMAL_WIDTH = 65` 限制一元负号产生的十进制显示宽度。

`Signature` 是构造期快照而不是可执行表达式树：

- `argument_type` 是归并后希望输入采用的类型，`return_type` 是求值结果族。
- `pb_code` 决定是否有可下推标识；向量真假测试为 `None`，表示本文件没有声明对应 PB 码，而不是该求值语义不可用。
- 逻辑、真假、`NOT`、`IS NULL` 的 `flen` 为 1；位运算默认 `-1` 并将 `unsigned_result` 设为真。
- `keep_null` 只影响真假测试；`constant_arg_overflow` 记录整型常量被提升成十进制的原因。
- `warning` 当前只由 JSON 一元 `NOT` 设置为 `"JSON value used in a boolean context"`；本文件不负责把它写入会话诊断区。

所有函数类和签名门面只保存值类型或不可变配置，并派生 `Clone`。它们没有持有行、表达式、会话、事务或引用计数资源。

## 依赖与调用关系

直接外部依赖均由 [`pkg/expression/Cargo.toml`](Cargo.toml) 声明：

- `rust_decimal::Decimal`：十进制零值判断、取负及溢出检测。
- `serde_json::Value`：JSON 真值输入载体。
- `thiserror::Error`：为 `EvalError` 生成错误展示实现。

文件内部的主要调用链是：

- `*FunctionClass::get_function` → `validate_arity` → 对应的 `*_signature` 构造函数。
- `Builtin*Sig::eval_*` → `map_boolean_value`（需要时）→ `logical_*`、`bit_*`、`truth_test`、`unary_*` 或 `is_null`。
- `logical_and`/`logical_or`/`logical_xor` 与 `bit_binary` → 调用方提供的 `FnOnce` 右参数求值器。
- `unary_minus_signature` → `handle_int_overflow`，并读取 `ExprMetadata` 与 `MYSQL_MAX_DECIMAL_WIDTH`。

RustCodeGraph 对 `unary_minus_signature` 给出的直接调用者包括同文件的 `UnaryMinusFunctionClass::get_function`，以及两个独立测试中的签名断言；`truth_signature` 的直接调用者包括 `IsTrueOrFalseFunctionClass::get_function` 和签名选择测试；`is_null_signature` 的直接调用者是 `IsNullFunctionClass::get_function`。对核心 `logical_and` 的索引入口显示测试导入，仓库级精确搜索也未找到测试外调用。

Go 侧 [`pkg/expression/builtin_op.go`](builtin_op.go) 则被 `builtin.go`、生成的线程安全/非线程安全文件、`distsql_builtin.go` 和 `expression.go` 使用，处于完整 Go 表达式执行链。Rust 侧生产注册链当前经过 [`pkg/expression/builtin.rs`](builtin.rs) 的 `formal_registry` 和其中自己的 `isTrueOrFalseFunctionClass`/`TruthBuiltin` 等实现，而不是调用本文件门面。

## 错误处理与边界

- 任意输入 `Err` 都通过 `?` 或 `Result::map` 向上传播；`is_null` 也不会把求值错误误判为 NULL。
- `validate_arity` 精确比较参数数量，错误同时携带 `expected` 与 `actual`。
- 二元逻辑的短路优先于右侧错误：例如 `0 AND error` 返回 `0`，`1 OR error` 返回 `1`。但结果未确定时，右侧错误必须传播。
- 位二元运算在左侧 NULL 时不会求右侧；右侧 NULL 返回 NULL。超宽或负数移位量经无符号解释后返回 `0`。
- `i64::MIN` 的有符号取负和大于 `2^63` 的无符号取负返回 `EvalError::Overflow { type_name: "BIGINT", ... }`；无符号 `2^63` 是合法特例，结果为 `i64::MIN`。
- 十进制取负用检查运算，失败时产生 `DECIMAL` 溢出；浮点取负不额外检查 NaN、无穷或负零。
- 不受支持的签名类型返回 `EvalError::Unsupported`。已知特例是 `truth_signature` 对 `VectorFloat32` 返回成功但 `pb_code=None`。
- JSON 真值并非通用 JSON 到布尔转换：它模拟 Go 中与数值 JSON `0` 的比较。测试明确验证 JSON `false` 不被视为数值零，因此对其执行 `unary_not` 得到 `0`。
- 当前模型没有 Go `errors.Trace` 的错误栈包装、真实类型转换错误、会话 warning 追加或具体表达式读取错误；这些只能由未来生产接线补充，不能从本文件推断为已支持。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、文件描述符、网络连接或事务生命周期。求值输入按值传递，`Decimal`、JSON 和向量的所有权随 `EvalResult` 进入函数。

二元操作把右参数表示为 `FnOnce`，静态保证它最多求值一次；控制流进一步保证短路时完全不求值。这既是求值顺序约束，也是副作用/错误可见性的边界。

函数类和签名门面派生 `Clone`，保存的字段均为值或不可变配置。该设计对应 Go 签名 `Clone` 复制基础状态的意图，也呼应 Go 文件中“跨 session 共享的新增字段必须线程安全或不可变”的注释。不过，Rust 门面目前并未实现或声明跨会话共享接口；若未来加入内部可变状态，必须重新审查 `Send`/`Sync`、克隆语义和会话隔离，不能仅依赖当前的 `Clone`。

## 与 Go 版本的对应关系

对应基线是 [`pkg/expression/builtin_op.go`](builtin_op.go)：

- `EvalResult<T>` 对应各 Go `eval*` 的 `(value, isNull, error)`；Rust 用 `Option` 显式承载 NULL。
- `logical_*` 对齐 Go 的左到右求值和三值真值表；`FnOnce` 代替 Go 中从 `b.args[n]` 读取行值的动作。
- 位运算与移位对齐 Go 的 `uint64` 转换；签名元数据中的 `unsigned_result` 对应 Go 返回类型的 `mysql.UnsignedFlag`。
- `truth_signature` 对齐 Go 对 `ETTimestamp`/`ETDatetime`/`ETDuration`/`ETJson`/`ETString` 到 `ETReal` 的归并、`keepNull` 分支和 PB 码。向量真假签名在 Go 中同样注释掉 PB 码设置，Rust 因而返回 `None`。
- `unary_not_signature` 对齐 Go 类型提升与 JSON 布尔上下文 warning；Rust 仅把 warning 作为元数据返回，不操作 `EvalContext`。
- `handle_int_overflow`、`unary_minus_signature` 和三个求值函数对齐 Go 常量溢出提升、类型选择、列宽规则和运行期整数边界。Rust 用 `rust_decimal::Decimal`，Go 用 `types.MyDecimal`，二者的具体表示和极限不是同一类型。
- `is_null_signature` 和泛型 `is_null` 合并了 Go 针对每个输入族的多个签名实现，同时保留 PB 码选择和“结果本身非 NULL、错误照常传播”的行为。

重要差异与迁移状态：Go 函数类持有 `baseFunctionClass`，通过 `BuildContext` 构建带参数表达式和返回 `FieldType` 的 `builtinFunc`，求值时读取 `EvalContext` 与 `chunk.Row`；Rust 本文件只保留无上下文算法和元数据，不持有表达式树，也未注册到生产 `formal_registry`。Rust 生产侧 [`pkg/expression/builtin.rs`](builtin.rs) 另有正式函数类与执行对象。因此新增行为时，不能只修改本文件就假定完整 Rust SQL 路径已经改变。

## 扩展指南

新增或修改操作时，按职责选择接入点：

1. 改变纯求值语义：修改对应的 `logical_*`、`bit_*`、`truth_test`、`unary_*` 或 `is_null`；把回归放在独立文件 [`pkg/expression/builtin_op_test.rs`](builtin_op_test.rs) 或 [`pkg/expression/builtin_op_20_aster_unit_test.rs`](builtin_op_20_aster_unit_test.rs)，不要把测试嵌入生产源文件。
2. 新增二元操作：扩展 `BinaryOp`、`ScalarFuncSig`、`binary_signature`，并决定是否可复用 `bit_binary`/`binary_int_signature!`；同时核对返回无符号标志和 `flen`。
3. 新增输入类型或真假规则：扩展 `EvalType`、`BooleanValue::is_zero`、具体签名宏实例和 `truth_signature`/`unary_not_signature`/`is_null_signature` 的穷举分支；明确是否存在 PB 下推码。
4. 调整一元负号推断：同步审查 `ExprMetadata`、`handle_int_overflow`、`UnaryMinusFunctionClass::type_infer` 和 `unary_minus_signature`，尤其关注常量与列、signed/unsigned、`flen` 上限和 `decimal`。
5. 接入真实 Rust 执行链：还需修改 `builtin.rs` 的 `formal_registry`/正式函数类或相应执行对象，并验证上下文类型转换、行读取、warning、PB 下推和错误类型；本文件的轻量门面不能替代这些接线。
6. 与 Go 保持一致：逐段核对 `builtin_op.go` 的函数类、签名 `eval*` 和 PB code。Go 后续若改变短路、类型归并或显示宽度规则，应同步更新 Rust 实现及两个独立测试。

主要风险是 SQL NULL 与错误混淆、破坏右侧求值边界、把算术右移误作逻辑右移、遗漏 unsigned `2^63` 特例、给向量真假测试错误声明 PB 能力，以及只更新语义内核却遗漏生产注册链。性能上应保留短路和按值/闭包的一次求值模型，避免为简单标量操作引入不必要的克隆或分配。

## 验证依据

本说明依据以下直接证据编写：

- 目标源文件 [`pkg/expression/builtin_op.rs`](builtin_op.rs)：完整读取 942 行，核对全部常量、类型、宏、函数类、签名门面和签名选择函数。
- crate 边界 [`pkg/expression/Cargo.toml`](Cargo.toml)：确认 crate 名、`autotests=false`、`lib.rs` 入口，以及 `rust_decimal`、`serde_json`、`thiserror` 直接依赖。
- 模块入口 [`pkg/expression/lib.rs`](lib.rs)：确认 `builtin_op_kernel` 是私有生产模块，`builtin_op` 别名仅在测试配置下再导出；确认生产注册接口来自 `expression_builtin::formal_registry`。
- Go 对照 [`pkg/expression/builtin_op.go`](builtin_op.go)：核对逻辑/位运算求值、真假与 NULL、类型归并、PB code、warning、一元负号溢出/宽度和 Go 的并发共享约束。
- Rust 独立测试 [`pkg/expression/builtin_op_test.rs`](builtin_op_test.rs) 与 [`pkg/expression/builtin_op_20_aster_unit_test.rs`](builtin_op_20_aster_unit_test.rs)：核对三值真值表、短路、NULL/错误传播、64 位移位、所有真值族、整数边界、PB 码、`flen` 上限和克隆门面。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`node`/`query` 确认 `logical_and`、`truth_signature`、`unary_minus_signature`、`is_null_signature` 和 `IsTrueOrFalseFunctionClass` 的定义与直接调用轨迹。`unary_minus_signature` 的图调用者包括本文件 `get_function` 及独立测试，`truth_signature` 的图调用者包括真假函数类及测试，`is_null_signature` 的图调用者为 `IsNullFunctionClass::get_function`。
- 仓库精确搜索：除 `lib.rs` 的模块装入/测试再导出和上述两个测试外，没有找到本文件核心符号的 Rust 使用点；这支持“尚未接入生产注册链”的限定，而不是从设计意图推断接线状态。

这是纯文档分析任务，依计划不运行 Cargo。验收使用固定十一章节的结构检查，并人工复核以上路径、符号和边界陈述。

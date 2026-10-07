# `pkg/expression/builtin_other_vec.rs` 逻辑说明

## 文件定位

`pkg/expression/builtin_other_vec.rs` 是 `astersql-expression` crate 中 `builtin_other` 组的手写向量化内核。crate 根 `pkg/expression/lib.rs` 通过 `#[path = "builtin_other_vec.rs"] mod builtin_other_vec_kernel;` 私有装配它，并紧邻装配 `builtin_other_vec_generated.rs`；生成版 IN 实现直接从本模块引用 `EvalContext`、`EvalError`、`EvalResult`、`VectorExpression` 和 `write_int_options`。因此，本文件既实现 VALUES、ROW、BIT_COUNT、GET_PARAM、SET/GET 用户变量，也为生成版 IN 提供轻量求值协议和公共支撑，但它不是 crate 对外公开模块。

`pkg/expression/lib.rs` 只在 `#[cfg(test)]` 的 `expression_other_vec` 测试门面中通配再导出本模块内容。仓库中直接构造本文件签名的可见调用点集中在 `builtin_other_vec_test.rs`、`builtin_other_vec_generated_test.rs` 和 `builtin_other_vec_generated_21_aster_unit_test.rs`；生产侧的确定依赖是生成版 IN 内核对上述基础类型/辅助函数的引用。不要把本文件的 `EvalContext`/`VectorExpression` 与 crate 其他运行时同名接口（例如 `exprctx::EvalContext`）混为一谈。

## 核心职责

- 定义该轻量向量求值子系统的错误、上下文和表达式边界：`EvalError`、`EvalResult<T>`、`EvalContext`、`VectorExpression`。
- 以 `ColumnExpression` 和 `LiteralExpression` 提供列投影与常量广播，使手写签名和生成版 IN 能以统一接口取得不同物理类型的列。
- 提供结果列重建与 `Option<i64>` 写回辅助，其中 `write_int_options` 被 `builtin_other_vec_generated.rs::evaluate_in` 复用。
- 移植 Go `builtin_other_vec.go` 的非生成逻辑：VALUES 明确不支持向量求值、ROW 的不可达求值入口、BIT_COUNT、GET_PARAM，以及字符串/整数/实数/Decimal 用户变量的 SET/GET。
- 保留 NULL 传播、参数越界、类型转换错误和 BIT_COUNT 溢出回退等边界语义；本文件不负责生成版各类型 IN 的具体比较算法。

## 主要符号

- `EvalError`：错误枚举。`Overflow` 是 BIT_COUNT 的特定回退信号；`ParamIndexExceeds` 表示参数索引非法；`NotImplemented` 用于 VALUES；`Unsupported` 用于错误的类型求值入口；`Message` 包装外部转换错误和结构错误。
- `EvalContext`：拥有 `parameters: Vec<Datum>`、大小写归一化后的 `user_vars: HashMap<String, Datum>`、字符串写入所用 `collation` 和转换所用 `type_context`。公开构造器为 `new`、配置器为 `with_collation`，`user_var` 提供不区分大小写的只读查询。
- `VectorExpression: Send + Sync`：定义 int/string/real/decimal/time/duration/json 七类向量求值、整数逐行求值和 unsigned 元数据。默认实现均显式返回 `Unsupported`，而不是隐式转换。
- `ColumnExpression`：按输入列下标投影，所有向量路径都尊重 `Chunk::Sel()`；`unsigned` 只提供整数有符号性元数据。
- `LiteralValue` / `LiteralExpression`：表示 NULL 或七类字面量并广播到 `input.NumRows()`；类型与调用的求值方法不匹配时返回 `Unsupported`。
- `reset_*_column`、`write_int_options`：以正确物理布局替换结果列，再逐项追加值或 NULL。只有 `reset_int_column`、`write_int_options` 为 `pub(crate)`，供生成文件复用。
- `values_signature!` 生成七个 `BuiltinValues*Sig`；它们的 `vectorized()` 恒为 `false`，对应求值方法恒返回 `NotImplemented`。`BuiltinValuesJSONSig` 是命名兼容别名。
- `BuiltinRowSig`：`vectorized()` 返回 `true`，但 `vec_eval_string` 直接 panic，表达该入口按契约不可被执行。
- `bit_count` / `BuiltinBitCountSig`：以 wrapping 位运算统计 i64 二进制补码的置位数；签名先走子表达式向量求值，仅遇 `EvalError::Overflow` 才逐行调用 `eval_int_row`。
- `BuiltinGetParamStringSig`：向量求出索引，调用 `EvalContext::parameter` 取参数，再以 `Datum::ToString` 写为字符串。
- `binary_signature!` 生成四个 SET 签名，`unary_signature!` 生成四个 GET 签名；具体 impl 分别处理 String、Int、Real、Decimal。

文件没有模块级常量和条件编译项；条件编译发生在 `lib.rs` 对测试模块/测试门面的装配处。

## 执行流程

通用向量流程是：调用签名方法时先让子 `VectorExpression` 写入临时 `chunk::Column`，再按 `input.NumRows()` 遍历逻辑行，传播 NULL、执行该签名语义，最后将结果追加到重置后的目标列。`ColumnExpression` 在读取源列时经 `physical_row` 将逻辑行映射到选择向量中的物理行；字面量表达式则按逻辑行数广播。

BIT_COUNT 的正常路径由 `BuiltinBitCountSig::vec_eval_int` 一次求出参数列，对非 NULL 单元调用 `bit_count`。若子表达式返回 `Overflow`，它改为逐逻辑行调用同一子表达式的 `eval_int_row`，再计数并通过 `write_int_options` 输出；其他错误原样上抛。`builtin_other_vec_generated_21_aster_unit_test.rs::bit_count_matches_go_and_falls_back_row_by_row_on_overflow` 同时覆盖正常路径和此回退路径。

GET_PARAM 先求整数索引列。NULL 索引输出 NULL；负索引或超出 `parameters` 长度由 `EvalContext::parameter` 返回 `ParamIndexExceeds`；成功取得的 `Datum` 调用 `ToString`，转换失败按 Go 对照行为输出 NULL，而非终止整批。

SET 用户变量先分别求名称列和值列；任一为 NULL 时本行输出 NULL且不写状态。否则名称转小写，值包装成对应 `Datum` 后写入 `EvalContext::user_vars`，同时把原值写回结果列。GET 先求名称列并转小写；NULL 名称或不存在的变量输出 NULL，存在值按目标类型直接读取或经 `type_context` 转换。

VALUES 的“流程”刻意终止于 `NotImplemented`，由 `vectorized() == false` 指示调度方不应选择该路径；ROW 虽声明可向量化，但其字符串入口是明确的不可达 panic。这两者是兼容契约，不是待补的普通空实现。

## 数据与状态

`EvalContext` 是本文件唯一长期可变状态容器。计划参数在构造时移入且只读；用户变量由 SET 签名通过 `&mut EvalContext` 更新，GET 和普通表达式仅借用 `&EvalContext`。变量键写入和读取都转小写，形成大小写不敏感不变量。默认 collation 为 `utf8mb4_bin`，只影响 `BuiltinSetStringVarSig` 创建的 collation-aware 字符串 Datum；`with_collation` 可覆盖它。`type_context` 从 `DefaultStmtNoWarningContext` 克隆，用于 Real/Decimal GET 转换。

`chunk::Column` 是批量数据载体。各 `reset_*_column` 会整体替换调用方传入的结果列：整数、实数、Decimal、Time 使用定长布局，String、JSON 使用变长布局。NULL 位与值按追加顺序共同形成结果。`ColumnExpression` 的读取行受 `Chunk::Sel()` 影响，而多数已求值临时列随后按连续的逻辑行下标读取；子表达式必须产出与 `NumRows()` 对应的逻辑结果列。

`LiteralValue::Int(i64, bool)` 的布尔值代表 unsigned 元数据，并由 `is_unsigned` 暴露；值本身仍以 i64 保存。`Json` 在广播时 clone；Decimal、String 等按各自 column API 写入。没有全局静态可变数据、缓存或跨上下文共享的用户变量表。

## 依赖与调用关系

crate 边界由 `pkg/expression/Cargo.toml` 确认：包名为 `astersql-expression`，库入口是 `lib.rs`，没有为本文件单设 feature。直接源码依赖是 `crate::chunk` 和 `crate::types`，它们在 `lib.rs` 分别再导出 `astersql-util-chunk` 与 `astersql-types`；错误派生来自 Cargo 依赖 `thiserror = "2"`，标准库提供 `HashMap`、`Send + Sync` 和格式化接口。

已核实的内部调用边包括：`BuiltinBitCountSig::vec_eval_int -> VectorExpression::vec_eval_int / eval_int_row -> bit_count -> write_int_options`；`BuiltinGetParamStringSig::vec_eval_string -> VectorExpression::vec_eval_int -> EvalContext::parameter -> Datum::ToString`；四个 SET 签名调用相应子表达式求值与 `EvalContext::set_user_var`；四个 GET 签名读取 `user_vars` 并在 Real/Decimal 路径调用 `Datum::ToFloat64`/`ToDecimal`。

反向依赖中，`builtin_other_vec_generated.rs::evaluate_in` 和各 `BuiltinIn*Sig` 使用本文件的上下文、trait、错误及整数结果写入器；`builtin_other_vec_generated_test.rs` 使用 `ColumnExpression`/`EvalContext` 测试生成版 IN；`builtin_other_vec_test.rs` 与 `builtin_other_vec_generated_21_aster_unit_test.rs` 直接覆盖本文件签名。RustCodeGraph 可定位这些符号和文件内调用；对若干 `callers` 查询未返回稳定结果，因此模块反向依赖以源码 import 和 `lib.rs` 装配为直接证据，不据此声称存在未见的运行时调用者。

## 错误处理与边界

- 错误类型单一收敛为 `EvalError`；外部 Datum 转换错误通过 `external_error` 丢失具体类型但保留展示文本。
- 参数索引必须非负且小于参数数目，否则整批 GET_PARAM 返回 `ParamIndexExceeds`。索引表达式为 NULL 只影响对应行。
- GET_PARAM 的 `Datum::ToString` 失败被转换为该行 NULL；用户变量字符串/数值转换失败则通过 `Message` 终止求值。这一差异与同路径 Go 实现相符。
- `ColumnExpression::source` 检查列下标并返回带下标信息的 `Message`；选择向量长度和元素范围由 `Chunk` 契约保证，本文件不额外检查。
- trait 的未实现类型路径全部返回 `Unsupported`。调用方若为 BIT_COUNT 提供只实现向量求值、却会产生 `Overflow` 且没有实现 `eval_int_row` 的表达式，回退会返回 `Unsupported("scalar int")`。
- SET 输入中名称或值为 NULL 时不改变变量状态；GET 的 NULL 名称或缺失变量返回 NULL。
- VALUES 返回 `NotImplemented`，ROW 的错误调用会 panic；安全扩展时不应把这两个边界静默改成默认值。
- `bit_count` 使用 `wrapping_sub`/`wrapping_add`，使 debug/release 都保持 Go int64 溢出语义；`-1` 的结果为 64。

## 并发与资源生命周期

`VectorExpression` 要求实现者为 `Send + Sync`，因此表达式对象可以安全跨线程边界共享；但本文件没有创建线程、任务、锁或通道。是否并行调用仍由上层执行器决定。

SET 方法必须获得独占的 `&mut EvalContext`，同一上下文中的 `HashMap` 更新不能并发发生；GET 使用共享借用。上下文拥有参数和变量 Datum，随上下文释放，不存在外部句柄或显式清理。临时列（名称、值、索引、BIT_COUNT 输入）均是方法栈内所有权对象，返回即释放；这与 Go 版通过 `bufAllocator.get/put` 复用列缓冲不同，Rust 版当前没有缓冲池生命周期。

用户变量有跨行状态：SET 按输入行顺序写入，同名变量以最后一个非 NULL 写入为最终状态。Go 主执行链另在 `chunk_executor.rs::HasGetSetVarFunc` 检测 GetVar/SetVar 以维护逐行顺序；本文件的轻量上下文本身不接入该检测，扩展或接线时必须重新验证跨表达式、跨批次的求值顺序，不能只依赖 `Send + Sync`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/builtin_other_vec.go`。七类 VALUES 的 `vectorized=false` 与未实现错误、ROW 的 `vectorized=true` 与 panic 文本、BIT_COUNT 位运算、仅在 overflow 时逐行回退、GET_PARAM 的 NULL/越界/字符串化、四类用户变量 SET/GET 均逐项对应。`pkg/expression/builtin_other_vec_test.go` 还验证 Go 的向量函数注册、GET_PARAM 参数转换及越界；IN 的 Go 测试范围更广，而 Rust 的生成版 IN 位于相邻文件。

需要注意的实现差异：Go 签名依附完整 `builtin*Sig`/`Expression`/session `EvalContext`，通过 buffer allocator 复用临时列，并把用户变量放在 session vars/user-vars reader 中；Rust 本文件定义独立的 `VectorExpression` 与自有 `EvalContext`，临时列每次局部创建，变量只活在该上下文的 `HashMap` 中。Go 的 SET String 显式复制字符串并读取 session charset/collation；Rust 的 `String` 已拥有数据，collation 来自本地字段。Go 的 GET Real/Decimal 注释指出 get/set variable vectorized eval 曾被禁用，但方法仍存在；Rust 签名当前返回 `vectorized=true`。这些差异意味着“算法语义已移植”不等于“已完全接入 Go 的生产 session/runtime 架构”。

Rust 独立测试 `builtin_other_vec_test.rs` 覆盖 BIT_COUNT、GET_PARAM、VALUES/ROW 基础契约；`builtin_other_vec_generated_21_aster_unit_test.rs` 进一步覆盖 overflow 回退、所有 SET/GET 类型、大小写、NULL、缺失变量与 ROW panic；`builtin_other_vec_generated_test.rs` 验证依赖本内核的生成版 IN。测试与源文件保持独立，符合仓库约束。

## 扩展指南

新增返回类型或新签名时，先判断它属于手写“other”逻辑还是生成版 IN：共享表达式/列能力应扩展 `VectorExpression`、`LiteralValue`、`ColumnExpression` 和对应 `reset_*`；具体 IN 比较通常应修改 `builtin_other_vec_generated.rs` 的生成来源而不是只改生成产物。任何 trait 新方法都应保留默认的显式 `Unsupported`，并为列投影、字面量和相关签名补齐独立测试。

修改 BIT_COUNT 时重点保持：NULL 传播、负数补码、仅捕获 `Overflow`、逐行回退与选择向量语义；同步更新 `builtin_other_vec_test.rs` 和 `builtin_other_vec_generated_21_aster_unit_test.rs`。修改 GET_PARAM 时同步验证负索引、等于参数长度的越界、NULL 索引、`ToString` 失败策略，并与 `builtin_other_vec_test.go::TestGetParamVec` 对照。

修改用户变量时应同步四种类型 SET/GET 测试，并评估大小写归一化、string collation、转换上下文、NULL 不写入、同名多行最后写入、跨表达式求值次序。若要接入真实 session，最可能修改 `EvalContext`、SET/GET 签名构造及上层调度，不应把 session 状态复制成新的全局缓存。兼容风险主要是与 Go session 生命周期/错误类型的差距；正确性风险是 NULL 和顺序；性能风险是每次创建临时列、逐行追加和 overflow 回退。

改变 VALUES 或 ROW 前必须先检查标量调度方：`vectorized=false` 与不可达 panic 是既有契约。对列/字面量扩展还需确保输出恰为 `input.NumRows()`，否则生成版 `evaluate_in` 的行数检查会返回 `Message`。若新增直接依赖，需在 `pkg/expression/Cargo.toml` 统一声明；当前没有相关 feature 开关。

## 验证依据

本说明逐行阅读并交叉核对了：目标源 `pkg/expression/builtin_other_vec.rs`；crate 声明 `pkg/expression/Cargo.toml`；模块装配/测试再导出 `pkg/expression/lib.rs`；直接依赖方 `pkg/expression/builtin_other_vec_generated.rs`；Rust 测试 `pkg/expression/builtin_other_vec_test.rs`、`pkg/expression/builtin_other_vec_generated_test.rs`、`pkg/expression/builtin_other_vec_generated_21_aster_unit_test.rs`；Go 对照 `pkg/expression/builtin_other_vec.go` 与 `pkg/expression/builtin_other_vec_test.go`；顺序相关旁证 `pkg/expression/chunk_executor.rs`。

RustCodeGraph 状态显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边。执行过的查询包括 `status`、目标路径 `files`、针对目标文件的 `explore`，以及 `query BuiltinBitCountSig`、`query bit_count`、`query BuiltinGetParamStringSig`、`query VectorExpression`、`node BuiltinBitCountSig` 和相应 callers/callees 尝试。索引精确定位了目标符号，例如 `BuiltinBitCountSig` 在第 624 行、`bit_count` 在第 613 行、`BuiltinGetParamStringSig` 在第 667 行；路径过滤未返回文件，部分 callers 查询无输出或超时，因此这些缺口用源码 import、模块声明和测试引用直接核验，未据不完整图结果推断额外调用关系。

本任务是纯文档分析，按任务约束不运行 Cargo。交付结构检查要求本文恰含“文件定位”至“验证依据”的 11 个固定二级标题；人工复核重点是：文件存在的原因、正常及异常流程、状态生命周期、Go 差异和安全扩展位置均有真实符号或文件路径支撑。

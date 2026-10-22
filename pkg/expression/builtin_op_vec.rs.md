# `pkg/expression/builtin_op_vec.rs`

## 文件定位

实现整数、浮点、DECIMAL 的逻辑非与一元负号，并为整数负号执行有符号/无符号 BIGINT 溢出检查。

## 核心职责

- 用 `vectorized() -> true` 表达这一组运算均允许向量化；当前 Rust API 是统一标记函数，而 Go 是每个签名各自实现 `vectorized()`。
- 在真实 `chunk_dependency::Column` 上实现 SQL 三值逻辑：`vec_logic_or`、`vec_logic_and`、`vec_logic_xor` 将整数零视为假、非零视为真，并按 SQL 的 `NULL` 真值表输出 `0`、`1` 或 `NULL`。
- 为 OR/AND 模拟 Go 的转换告警回退协议：`vec_logic_with_fallback` 在右参数求值报错或新增 warning 时，撤销两侧向量转换期间的 warning，并调用标量回退闭包。
- 实现按无符号位模式计算的 OR/XOR/AND、逻辑左移/右移和按位取反，同时传播输入 `NULL`。
- 实现整数、浮点、DECIMAL 的逻辑非与一元负号，并为整数负号执行有符号/无符号 BIGINT 溢出检查。
- 实现整数、浮点、DECIMAL 的 `IS TRUE`/`IS FALSE`，以及 Time/Int/Real/Decimal/Duration 共用的 `IS NULL` 位图检查。

## 主要符号

- `pub use chunk_dependency::Column` 和 `pub use ...::MyDecimal`：公开底层列类型与 DECIMAL 类型，主要便于当前测试侧从内核模块取用；实现还通过 `DecimalNeg` 完成 DECIMAL 取负。
- `EvalResult<T>`、`EvalError`：统一返回类型。错误枚举包含双列行数不一致 `ColumnLengthMismatch`、整数负号越界 `BigIntOverflow`，以及由回退闭包等边界传入的通用 `Evaluation(String)`。
- `WarningContext`：仅为 OR/AND 回退所需的最小 warning 容器，提供追加、计数、截断和只读列表访问。它不是 expression crate 现有完整求值上下文的替代品。
- `check_binary_rows`、`int_result`、`write_i64`、`push_i64`、`logical_value`：分别负责二元列长度不变量、定长 Int64 结果分配、按原生字节写值、追加可空整数和三值布尔转换。
- `vec_logic_binary` 与 `sql_or`/`sql_and`/`sql_xor`：逻辑运算公共循环和三张真值表；公开包装是 `vec_logic_or`、`vec_logic_and`、`vec_logic_xor`。
- `vec_logic_with_fallback`：泛型回退骨架；公开包装 `vec_logic_or_with_fallback`、`vec_logic_and_with_fallback` 接受左/右向量求值闭包和标量回退闭包。
- `BitOperation`、`vec_bit_binary`：位运算公共分派；公开入口为 `vec_bit_or`、`vec_bit_xor`、`vec_bit_and`、`vec_left_shift`、`vec_right_shift`，一元入口为 `vec_bit_neg`。
- `vec_unary_not_int`、`vec_unary_not_real`、`vec_unary_not_decimal`：按类型读取列值，零输出 `1`、非零输出 `0`、`NULL` 保持为空。
- `vec_unary_minus_real`、`vec_unary_minus_decimal`、`vec_unary_minus_int`：浮点、DECIMAL、整数负号；只有整数版本返回 `EvalResult`，因为它会检查 BIGINT 边界。
- `vec_is_truth_int`、`vec_is_truth_real`、`vec_is_truth_decimal`：真/假判断公共实现；六个公开包装用 `want_true` 选择 TRUE/FALSE，用 `keep_null` 决定输入 `NULL` 是保留为 `NULL` 还是变成非空 `0`。
- `vec_is_null`：只检查 NULL 位图，始终输出非 NULL 的 Int64；五个类型命名包装 `vec_*_is_null` 均委托它。

## 执行流程

1. 上层先把参数求值成真实 `Column`，再调用本文件的公开内核；本文件自身不接收 `EvalContext`、输入 `Chunk` 或表达式签名对象。
2. 二元逻辑或位运算首先通过 `check_binary_rows` 要求左右列行数完全相等，避免 `zip` 式静默截断。逻辑运算逐行转成 `Option<bool>` 后应用 SQL 真值表；位运算先把 `i64` 位模式转为 `u64`，计算后再转回 `i64`。
3. OR/AND 的回退入口先记录左参数求值前的 warning 数，再求左列；左求值错误直接传播。随后记录右求值前的数量并求右列：右侧成功且未新增 warning 才进入向量内核，否则截断到左求值前的 warning 数并调用标量闭包。该流程对应 Go 的 `beforeArg0Warns`、`beforeArg1Warns`、`TruncateWarnings` 和 `fallbackEvalInt`。
4. 一元 NOT、负号、真值和 NULL 判断按输入行数分配结果并逐行处理。多数定长 Int64 路径用 `int_result` 后原位写入；浮点/DECIMAL 负号使用追加式结果列。
5. `vec_unary_minus_int` 根据 `unsigned` 选择读取 `GetUint64` 或 `GetInt64`。无符号值允许到 `2^63`（其负值为 `i64::MIN`），大于此值报错；有符号 `i64::MIN` 无法取负，也报错。
6. `IS TRUE/FALSE` 对非 NULL 值比较“是否为零”；输入为 NULL 时，`keep_null=true` 保留 NULL，否则结果保持预分配的非空零。`IS NULL` 则把位图状态转换为非空 `0/1`。

## 数据与状态

计算数据完全存放在调用方提供的不可变输入 `Column` 和函数新建的结果 `Column` 中。Int64 结果列通过 `ResizeInt64(rows, false)` 初始化；`write_i64` 按 `row * size_of::<i64>()` 覆盖底层 `data` 的原生字节，并由 `SetNull` 维护 NULL 位图。逻辑公共循环使用 `AppendInt64`/`AppendNull`，浮点与 DECIMAL 负号分别使用 `AppendFloat64`/`AppendMyDecimal`。

唯一显式可变上下文是 `WarningContext { warnings: Vec<String> }`。回退协议依赖 warning 数量的快照和 `Vec::truncate`，只撤销本次左右向量参数求值期间新增的 warning，保留进入调用前已有的 warning。所有其他状态都是函数局部变量；文件没有全局可变状态、缓存或持久化状态。

重要不变量包括：二元输入行数必须相等；输出行序与输入一致；逻辑和位运算不会丢行；除 `IS NULL` 及 `keep_null=false` 的真值判断外，输入 NULL 通常传播到输出；移位数大于等于 64 时输出零；右移按 `u64` 执行，因此负数是逻辑右移而非算术右移。

## 依赖与调用关系

直接依赖由 [`pkg/expression/Cargo.toml`](Cargo.toml) 声明：`chunk-dependency` 映射到 `../util/chunk`，提供 `Column`；`types-dependency` 映射到 `../types`，提供 `MyDecimal` 与 `DecimalNeg`；`thiserror` 为 `EvalError` 派生显示和错误实现。该 manifest 没有为本文件设置条件 feature。

模块装配路径是 `pkg/expression/lib.rs -> builtin_op_vec_kernel`。RustCodeGraph 的文件节点报告测试使用者为 `builtin_op_vec_test.rs`、`builtin_op_vec_19_aster_unit_test.rs`，并额外列出 `builtin_convert_charset_test.rs`；后者源码没有调用本文件符号，因而不作为行为覆盖证据。`rg` 对全部 Rust 文件的符号引用核查只发现 `lib.rs` 的模块声明/测试再导出和上述运算符测试，没有生产调用点。

向下调用主要是 `Column` 的 `Rows`、`IsNull`、`GetInt64`/`GetUint64`/`GetFloat64`/`GetDecimal`、`Resize*`、`Append*`、`SetNull`，以及 DECIMAL 的 `IsZero` 和 `DecimalNeg`。向上真正的 SQL 运算符签名与调度目前只在 Go `builtin_op_vec.go` 中完整存在；Rust 的相邻 `builtin_op.rs` 未调用这些内核。

## 错误处理与边界

- 二元入口在任何逐行读写之前验证长度；不相等返回包含左右行数的 `EvalError::ColumnLengthMismatch`。
- OR/AND 回退中，左求值错误使用 `?` 直接返回；右求值错误不是最终错误，而是触发标量重算，最终结果由 `fallback_eval` 决定。这是为保留 Go 的转换告警/错误求值语义，而不是吞掉所有错误。
- `vec_unary_minus_int` 对有符号 `i64::MIN` 和无符号大于 `2^63` 的值返回 `BigIntOverflow`；无符号恰好 `2^63` 使用 wrapping negation 得到 `i64::MIN`。错误发生时函数立即返回，调用方不会获得部分结果列。
- 移位量来自右列的 `i64` 位模式并转成 `u64`；所有负数因转换后很大而落入“大于等于 64”分支，结果为零，这与 Go 对 `uint64(arg1)` 的移位效果一致。
- 浮点真值使用与 `0.0` 的比较，因而 `-0.0` 视为假；浮点负号对 `-0.0` 取负得到 `+0.0`，已有 Rust 测试按位验证。
- `EvalError::Evaluation` 不是本文件内核主动产生的分支，目前用于闭包边界和测试构造。文件没有 panic 型业务错误；但 `write_i64` 假定结果列已正确分配，若未来绕过 `int_result`，底层切片越界会 panic。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。输入列只借用，结果列由函数拥有并返回，因此不同调用之间没有共享可变数据。`WarningContext` 以 `&mut` 独占借用传入，Rust 借用规则阻止同一上下文在一次调用期间被并发修改。

资源生命周期局限于函数栈帧和结果列分配：临时列及其缓冲区随返回值所有权移动或在错误路径释放。与 Go 版本不同，Rust 内核没有 `bufAllocator.get()/put()` 池化，也没有 `defer` 式归还；参数列已由调用者求值并传入。因此把本文件接入生产签名时，需要在上层设计临时列复用，不能从当前内核推断已有缓冲池或完整求值上下文生命周期。

## 与 Go 版本的对应关系

Go [`builtin_op_vec.go`](builtin_op_vec.go) 的每个 `builtin*Sig.vecEval*` 同时负责参数向量求值、临时列借还、NULL 合并与逐行运算；Rust 把其中“参数已成为 Column 后”的逐行部分提炼为自由函数。

- `builtin{BitOr,BitXor,BitAnd,LeftShift,RightShift,BitNeg}Sig` 对应 `vec_bit_*`、`vec_*_shift` 和 `vec_bit_neg`。Rust 显式规定移位量大于等于 64 为零，结果与 Go 的 `uint64` 移位语义一致。
- 五种 `builtin*IsNullSig` 对应五个类型命名包装，但 Rust 共用只读 NULL 位图的 `vec_is_null`。

Rust 当前缺少这一级生产注册与端到端签名测试，不能仅凭内核测试宣称已完成整条 SQL 表达式接线。

## 扩展指南

新增同类运算时，应先判断能否复用现有公共循环：新的三值逻辑可在 `sql_*` 真值函数与 `vec_logic_binary` 层扩展；新的二元位运算可扩展 `BitOperation` 和 `vec_bit_binary`；新的 TRUE/FALSE 输入类型应增加类型专用读取函数并保持 `keep_null` 规则；只依赖位图的类型化 `IS NULL` 应继续委托 `vec_is_null`。

若要完成生产接线，修改点不应局限于本文件：需要让 Rust 表达式签名在参数求值后调用这些内核，并提供真实 warning 上下文、标量回退和临时列复用。接线前应核对 Go `builtin_op_vec.go` 的 `EvalContext`、`bufAllocator` 和每个签名的输入类型；接线后应在独立测试文件中补签名级/注册级测试，而不是把测试内嵌进生产 `.rs`。

任何行为扩展都应同步 [`builtin_op_vec_19_aster_unit_test.rs`](builtin_op_vec_19_aster_unit_test.rs) 的精确边界测试和 [`builtin_op_vec_test.rs`](builtin_op_vec_test.rs) 的冒烟覆盖，并与 Go [`builtin_op_vec_test.go`](builtin_op_vec_test.go) 的标量/向量一致性用例比较。重点兼容风险是 SQL NULL 真值表、warning 回退时点、unsigned BIGINT 边界和大/负移位量；性能风险是逐行方法调用、每次新建结果列，以及生产接线后若没有复用临时列造成的分配开销。

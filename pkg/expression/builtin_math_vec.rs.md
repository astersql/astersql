# `pkg/expression/builtin_math_vec.rs`

## 文件定位

该文件是 `astersql-expression` crate 中一组基于遗留列式运行时的数学 SQL 内建函数实现。`pkg/expression/lib.rs` 通过 `#[path = "builtin_math_vec.rs"] mod builtin_math_vec_kernel;` 将它编入 crate；测试配置下，`expression_group_15` 再导出其符号供独立测试使用。它依赖 `pkg/expression/legacy_vectorized_runtime.rs` 提供的 `Chunk`、`Column`、`EvalContext`、`ExprRef` 与 `MathBase`，以“一次求值一批行”的方式实现 Go `pkg/expression/builtin_math_vec.go` 的主要行为。

当前接线边界需要特别说明：该模块是私有 kernel，仓库内非测试 Rust 代码没有直接引用这里定义的 `builtin*Sig` 类型；正式表达式体系的标量数学函数和签名选择位于 `pkg/expression/builtin_math.rs`，PB 反序列化入口位于 `pkg/expression/pb_to_expr_runtime.rs`。因此，本文件目前是已编译、可由 crate 内部使用并有独立测试覆盖的遗留向量化实现，而不是已经接入 `builtinFunc`/`Expression` 正式执行主链的公开 API。`pkg/expression/Cargo.toml` 将其归入包名 `astersql-expression`、库入口 `lib.rs`，并提供这里直接用到的 `crc32fast`、`mathutil` 与 `types-dependency`。

## 核心职责

- 用 `define_signature!` 生成 46 个持有 `MathBase` 的签名壳，并统一提供 `new`、`from_base` 和 `vectorized() == true`；`builtinConvSig` 单独实现且故意返回 `false`。
- 把参数表达式经 `VecEvalReal`、`VecEvalInt`、`VecEvalDecimal` 或 `VecEvalString` 批量求值到列，再逐行完成数学运算、NULL 合并和错误/告警处理。
- 覆盖对数、平方根、三角函数、角度换算、指数、幂、绝对值、ROUND、TRUNCATE、CEIL、FLOOR、CRC32、PI、RAND、SIGN 和 CONV。
- 对 REAL、INT/UINT、DECIMAL 分别保留 MySQL/Go 的边界语义，例如非法对数参数为 NULL 并追加告警、`ABS(i64::MIN)` 报 BIGINT 溢出、DECIMAL 采用半入或向零截断模式。
- 通过 `MathBase.mysql_rng` 保存无参 RAND 的跨行随机状态，通过 `MathBase.ret_decimal` 限制带小数位参数的 DECIMAL ROUND/TRUNCATE 输出精度。

## 主要符号

- `define_signature!`：批量定义签名结构；每个结构只保存私有 `base: MathBase`。生成类型是 `pub`，但所在模块本身是 crate 私有。
- `UnaryRealOp`、`unary_real`、`unary_real_impl!`：18 种一元 REAL 运算的共享分发内核。`unary_real` 先复用结果列承接第一个参数，再原地改写非 NULL 单元格。
- `round_float(value, decimals)`：按十进制缩放后调用 `round_ties_even`；缩放溢出时保留原值，最终 NaN 被归一为 `0.0`。
- `truncate_float(value, decimals)`：十进制缩放后向零截断；非有限中间值保留原值，极端负位数使非 NaN 输入变为零。
- 二元/专用签名实现：`builtinAtan2ArgsSig::vecEvalReal`、`builtinPowSig::vecEvalReal`、`builtinLog2ArgsSig::vecEvalReal`、带小数位的 ROUND/TRUNCATE、CRC32、PI、RAND、SIGN 等。
- `int_to_decimal`、`truncate_integer`、`decimal_to_int`：整数/DECIMAL 家族的共享转换内核，分别处理 unsigned 位模式、负小数位十进制截断以及 DECIMAL 截断状态后的 CEIL/FLOOR 修正。
- `builtinConvSig`、`valid_prefix`、`format_radix`、`conv`：CONV 的列式包装、合法数字前缀识别、目标进制格式化和有符号/补码转换核心。其实现存在，但 `vectorized()` 明确为 `false`。
- `builtinTruncateDecimalSig::with_ret_decimal` 与 `builtinRoundWithFracDecSig::with_ret_decimal`：把返回类型的小数位上限写入 `MathBase.ret_decimal`。

## 执行流程

1. 调用方以 `Vec<ExprRef>` 构造某个签名；普通构造走 `MathBase::new`，测试或确定性 RAND 可用 `builtinRandSig::with_seed`，DECIMAL 精度相关签名可用 `with_ret_decimal`。
2. `vecEval*` 让参数表达式先填充 `Column`。一元函数通常直接复用 `result`；二元或三元函数为后续参数创建临时 `Column`。
3. 固定宽度多参数函数调用 `result.MergeNulls`，将各参数 NULL 位逐行做逻辑或；字符串 CONV 则在循环中检查三个输入列并调用 `AppendNull`。
4. 实现按 `input.NumRows()` 或结果切片长度逐行计算。NULL 行跳过；普通结果原地写回对应类型的可变切片。
5. 定义域失败按函数约定转为 NULL、告警或错误：对数追加告警并置 NULL；SQRT、ACOS、ASIN 只置 NULL；EXP、POW、COT 和有符号 ABS 的特定溢出直接返回 `Err`。
6. DECIMAL 运算调用 `MyDecimal::Round`、`DecimalAdd`、`DecimalSub` 或 `ToInt`，错误通过 `?` 转成 `EvalError::Decimal`。CEIL/FLOOR 在向零截断后根据符号和 `DecimalError::Truncated` 调整一个整数单位。
7. RAND 无参形式对结果列每行调用共享 `MysqlRng::Gen`；带种子首值形式为每行种子创建新 RNG，NULL 种子按 0 处理。

几个重要分支不能互换：`builtinLog2ArgsSig` 即使已将非法行标记为 NULL，仍按 Go 的赋值顺序写入对数比值；COT 的正切为零才报错，而非零正切若倒数成为 Inf/NaN 则保持原单元格并继续；ACOS/ASIN 使用严格的 `< -1 || > 1`，所以 NaN 不会被置 NULL。

## 数据与状态

`Chunk` 在这条遗留路径中只携带行数。`Column` 同时保存 NULL 位图及 INT、REAL、DECIMAL、STRING 专用缓冲区，`Resize*` 会重置类型和长度，`ReserveString` 则切换为追加式字符串输出。多参数函数依赖所有固定宽度列长度一致；`MergeNulls` 对长度不一致会断言失败。

每个签名的主要状态位于 `MathBase`：`args: Vec<ExprRef>` 是通过 `Arc<dyn LegacyExpression>` 保存的参数；`mysql_rng: Arc<MysqlRng>` 是 RAND 的共享可变随机状态；`ret_decimal: i32` 是 DECIMAL ROUND/TRUNCATE 的结果小数位上限。签名派生 `Clone` 时会克隆 `MathBase`，其中 RNG 的 `Arc` 也被共享，因此克隆不是独立随机序列。

除 RAND 状态外，求值结果都写入调用者提供的 `Column`，临时参数列仅在一次调用内存活。`EvalContext` 内部用 `Arc<Mutex<Vec<String>>>` 收集警告，克隆上下文仍共享同一告警列表。

## 依赖与调用关系

上游接线为 `pkg/expression/lib.rs` → `builtin_math_vec_kernel`。测试侧由 `expression_group_15` 再导出，直接调用者包括 `pkg/expression/builtin_math_vec_test.rs` 和 `pkg/expression/builtin_like_vec_15_aster_unit_test.rs`。RustCodeGraph 对目标文件报告 68 个符号，并显示该文件被 `lib.rs` 装配后供上述测试组使用；对仓库非测试 Rust 引用的补充搜索没有发现这些签名类型的生产调用点，故不能把它描述为正式查询执行器已经使用的向量路径。

主要下游关系如下：

- 所有签名 → `MathBase.args[n].VecEval*` → `LegacyExpression` 动态分发。
- 多参数固定宽度函数 → `Column::MergeNulls`；常量输出 → `ResizeInt64`/`ResizeFloat64`/`ResizeDecimal`；CONV → `ReserveString` 与追加接口。
- DECIMAL 函数 → `types_dependency::decimal::mydecimal::{MyDecimal, DecimalAdd, DecimalSub, ModeHalfUp, ModeTruncate, NewDecFromInt, NewDecFromUint}`。
- CRC32 → `crc32fast::hash`；RAND → `mathutil::NewWithSeed`/`MysqlRng::Gen`；角度及三角函数 → Rust `f64` 与 `std::f64::consts::PI`。
- 错误与告警 → `legacy_vectorized_runtime::{EvalError, EvalContext}`。

`pkg/expression/pb_to_expr_runtime.rs` 会把 PB 数学签名名称映射到 AST 函数名并通过 `NewFunctionBase` 构造正式表达式；它没有直接构造本文件的签名。这是当前“PB/规划主链”与“遗留向量 kernel”之间尚未接通的边界。

## 错误处理与边界

- 子表达式求值失败、DECIMAL 运算失败会立即经 `?` 返回，当前批次后续行不再处理；已经写入的前序结果不会回滚。
- 对数参数 `<= 0` 时追加 `"invalid argument for logarithm"` 并置 NULL；双参数 LOG 还拒绝底数 1。SQRT 负数、ACOS/ASIN 超出 `[-1, 1]` 只置 NULL，不追加告警。
- EXP 结果为 Inf/NaN、POW 结果非有限、COT 的正切恰为零时返回 `EvalError::DoubleOverflow`。`ABS(i64::MIN)` 返回 `EvalError::BigIntOverflow`。
- `round_float` 采用 ties-to-even，而 DECIMAL ROUND 使用 `ModeHalfUp`；两者是有意的类型语义差异。`builtinRoundWithFracIntSig` 先转 `f64` 再转回 `i64`，极大整数存在浮点精度风险，扩展测试应覆盖边界。
- 整数 TRUNCATE 仅对负小数位工作；小数位参数类型为 unsigned 时整个批次直接保持原值。`i64::MIN` 小数位直接清零；有符号值截断 19 位及以上、无符号值截断 20 位及以上也清零，从而避免 `10^n` 溢出。
- CONV 只接受绝对值为 2..=36 的源/目标进制；基数为 `i64::MIN`、越界基数返回 NULL。空合法前缀返回字符串 `"0"`；解析出的 `u64` 溢出返回 BIGINT 错误。负源基数启用有符号限幅，负目标基数要求带符号输出，否则按 64 位补码格式化。
- CONV 的合法前缀按字节迭代并把字节转为 `char` 检查进制数字，因此非 ASCII 输入不被当作合法数字；`format_radix` 固定输出大写 A-Z。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁或事务。每次 `vecEval*` 的临时 `Column` 在栈作用域内创建并在返回时释放；与 Go 版本使用 `bufAllocator.get/put` 不同，Rust 版本目前没有列缓冲池复用，因此行为相同但分配特征不同。

共享状态来自 `MathBase` 和 `EvalContext`。参数是 `Arc<dyn LegacyExpression + Send + Sync>`；警告列表由 Mutex 保护；RAND 的 `MysqlRng` 经 `Arc` 共享，实际同步保证由 `mathutil::MysqlRng` 提供。连续行与共享同一 `MathBase` 的克隆会推进同一随机序列，调用顺序因而是可观察语义。普通算术实现没有持久资源，错误返回也不需要清理外部资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/builtin_math_vec.go`。两端按相同签名名称组织，并保持主要流程：先列式求值参数、合并 NULL、逐行运算、把对数定义域问题记为 warning、把数值溢出作为错误返回；`builtinConvSig.vectorized()` 在两端都故意为 false，同时保留列式实现。

已验证的关键一致点包括：ACOS/ASIN 对 NaN 使用 Go 同样的严格范围比较；双参数 LOG 在置 NULL 后仍计算并写入比值；NULL RAND 种子按 0；整数/DECIMAL 的 CEIL、FLOOR、ROUND、TRUNCATE 分支方向；CONV 的有符号基数与非法基数行为。独立 Rust 测试明确以 Go 行为为断言来源。

仍需注意的实现层差异：Go 复用 `bufAllocator`，Rust 每次创建临时列；Go 的 EXP 溢出表达式使用参数的格式化文本，Rust 使用当前数值；Rust REAL ROUND 的辅助函数依赖 `round_ties_even`，而 DECIMAL 明确使用 `ModeHalfUp`；Rust 目前位于遗留运行时 kernel，未直接实现正式 `builtinFunc` 接口。后续接线时应以这些差异为兼容性审查点，不能只凭同名函数认定已经完全替代 Go 主链。

## 扩展指南

新增一元 REAL 数学函数时，优先在 `UnaryRealOp` 增加枚举分支、在 `unary_real` 定义域/计算 match 中实现行为，再通过 `define_signature!` 与 `unary_real_impl!` 注册签名。若函数需要额外参数、不同结果类型、独立状态或特殊错误顺序，应像 POW、CRC32、RAND 或 CONV 一样编写专用 `impl`，不要强塞进一元共享内核。

扩展时必须同步检查：NULL 是否应合并还是转默认值；非法输入是 NULL、warning 还是 error；非有限浮点结果是否允许；参数 unsigned 标志是否改变位模式解释；DECIMAL 舍入模式与返回 `ret_decimal`；批次中途错误后的部分写入语义。新增测试应放在独立文件，首选扩展 `pkg/expression/builtin_math_vec_test.rs`；若属于 Go 对齐的组合场景，也应同步 `pkg/expression/builtin_like_vec_15_aster_unit_test.rs`，不要把测试嵌入本源文件。

若目标是让新函数进入真实 SQL/PB 执行主链，还需在 `pkg/expression/builtin_math.rs` 的标量实现和签名选择、`pkg/expression/pb_to_expr_runtime.rs` 的 PB 名称映射及正式表达式适配层完成必要接线，并验证 `vectorized()` 的选择机制。对 CONV，只有解决注释所述 hybrid-type 向量匹配问题后才能把标志改为 true。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/builtin_math_vec.rs` 确认目标文件已索引；`node --file ...` 读取到目标文件 1..961 行及 68 个符号；`explore`/`query` 核对了 `unary_real`、各 `vecEval*`、`builtinConvSig`、`decimal_to_int` 以及 Go 同名符号和测试调用关系。
- Rust 源与装配：`pkg/expression/builtin_math_vec.rs`、`pkg/expression/legacy_vectorized_runtime.rs`、`pkg/expression/builtin_math.rs`、`pkg/expression/lib.rs`、`pkg/expression/pb_to_expr_runtime.rs`、`pkg/util/mathutil/rand.rs`。
- crate 配置：`pkg/expression/Cargo.toml`，确认 crate 名、`lib.rs` 入口以及 `crc32fast`、`mathutil`、`types-dependency` 依赖；该包没有 `pkg/expression/doc.go` 可供补充包契约。
- Go 对照：`pkg/expression/builtin_math_vec.go`，逐段核对签名、NULL、告警、溢出、DECIMAL、RAND 与 CONV 行为。
- 独立 Rust 测试：`pkg/expression/builtin_math_vec_test.rs` 覆盖 LOG/NULL/NaN、ROUND/TRUNCATE、ABS 溢出、CRC32/PI/SIGN/RAND；`pkg/expression/builtin_like_vec_15_aster_unit_test.rs` 进一步覆盖 ATAN2、POW/COT 溢出、DECIMAL CEIL/FLOOR、整数 TRUNCATE、种子 RAND 和禁用向量标志的 CONV。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终结构验证要求目标文件存在且固定二级标题恰为 11 个。

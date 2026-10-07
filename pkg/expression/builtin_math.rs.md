# `pkg/expression/builtin_math.rs`

## 文件定位

本文件属于 `astersql-expression` crate 的数学内建函数标量实现。`pkg/expression/lib.rs` 通过 `#[path = "builtin_math.rs"] mod builtin_math_kernel;` 将它编入 crate，并在测试配置下通过 `mod builtin_math { pub use crate::builtin_math_kernel::*; }` 暴露给独立测试 `pkg/expression/builtin_math_test.rs`。它与向量实现 `pkg/expression/builtin_math_vec.rs` 并列，负责承载 Go `pkg/expression/builtin_math.go` 中数学函数的 Rust 标量语义、签名选择元数据以及错误/告警模型。

需要注意当前接线边界：对 `builtin_math::*` 和本文件公开入口的仓库级 Rust 搜索未发现生产调用者，RustCodeGraph 对关键入口也未返回上游调用边；`lib.rs` 中面向 `builtin_math_kernel` 的再导出只存在于 `#[cfg(test)]` 模块。因此，本文件当前可确认是已编译的私有实现和迁移/测试基线，不能仅凭其 API 完整度断言生产表达式执行器已经使用这些标量函数。生产向量路径及真正的 pb/表达式分发需分别到 `builtin_math_vec.rs`、`builtin_core.rs` 等接线处继续核验。

## 核心职责

1. 用 `EvalType`、`FieldTypeMeta`、`FractionMetadata` 和 `ScalarFuncSignature` 表达构建数学函数时需要的精简类型信息，并由 `abs_signature`、`round_signature`、`ceil_signature`、`floor_signature`、`log_signature`、`atan_signature`、`rand_signature`、`truncate_signature` 选择具体实现签名。
2. 实现 ABS、ROUND、CEIL、FLOOR、LOG/LOG2/LOG10、RAND、POW、CONV、CRC32、SIGN、SQRT、三角函数、EXP 和 TRUNCATE 的标量计算。
3. 保留 MySQL/TiDB 关键语义：DECIMAL 定点运算、非法定义域返回 SQL NULL、对数告警、BIGINT/DOUBLE 溢出错误文本、负进制的有符号 CONV 规则，以及 RAND 的有状态序列和逐行种子路径。
4. 提供 `calculate_decimal_for_round_and_truncate`、`get_eval_type_for_floor_and_ceil`、`floor_and_ceil_unsigned` 等返回类型推导辅助逻辑，使构建阶段与求值阶段使用相同边界。

## 主要符号

- `MAX_DECIMAL_SCALE = 30`：ROUND/TRUNCATE 推导返回小数位数时的上限；`MAX_INT_WIDTH = 20`：FLOOR/CEIL 判断 DECIMAL 整数部分是否可能超出 BIGINT 表示范围的基准。
- `MathError::{Overflow, Decimal}` 与 `MathResult<T>`：统一包装带 `[types:1690]` 文案的数值溢出和底层 `DecimalError`；`From<DecimalError>` 使 DECIMAL 运算可用 `?` 传播。
- `EvalType`、`FieldTypeMeta`、`FractionMetadata`：分别表示求值类别、`flen/decimal/unsigned` 字段元数据和 ROUND/TRUNCATE 第二参数是缺失、动态还是常量（含常量 NULL）。
- `ScalarFuncSignature`：列出本文件支持的各个类型专用签名；`FixedMathFunction` 与 `fixed_signature` 为参数形态固定的函数建立直接映射。
- `abs_signature` 至 `truncate_signature`：构建期分发入口。其中 FLOOR/CEIL 先调用 `get_eval_type_for_floor_and_ceil`，RAND 根据是否有参数及参数是否常量区分共享序列与“每行首值”路径。
- `abs_*`、`round_*`、`ceil_*`、`floor_*`、`truncate_*`：整数、浮点和 `MyDecimal` 的分型实现。DECIMAL 使用 `ModeHalfUp` 或 `ModeTruncate`，而 CEIL/FLOOR 在截断结果上按符号补加或补减 1。
- `MathWarning`、`MathOutcome<T>` 与 `eval_log*`：把“非法对数参数”建模为 `None` 加告警；`log*` 便捷函数只保留可选值，会丢弃告警列表。
- `MysqlRand`：持有 `Arc<MysqlRng>`，`with_seed` 创建序列、`from_shared` 接收共享状态、`generate` 推进 RNG；`rand_with_seed_first_gen` 则每次新建 RNG 并只取第一项。
- `pow`、`cot`、`exp`：对非有限结果或零正切值转成 `MathError::Overflow`；`sqrt`、`acos`、`asin` 对定义域外值返回 `None`，同时允许 NaN 继续按浮点语义传播。
- `get_valid_prefix`、`conv`、`conv_binary_literal`：完成 2 到 36 进制的合法前缀截取、符号解释、饱和边界、格式化及二进制字面量桥接；内部依赖 `parse_radix_u64` 和 `format_radix_u64`。
- `crc32`、`sign`、`atan/atan2`、`cos/sin/tan`、`degrees/radians`、`pi`：无额外状态的直接标量运算。

## 执行流程

构建数学表达式时，调用方应先把参数字段信息整理为 `FieldTypeMeta`，再由对应的 `*_signature` 选出 `ScalarFuncSignature`。例如 DECIMAL 的 FLOOR/CEIL 会计算 `flen - decimal`：整数部分超过 `MAX_INT_WIDTH - 2` 时保留 DECIMAL，否则返回整数签名；ROUND/TRUNCATE 的返回小数位由 `calculate_decimal_for_round_and_truncate` 根据结果类型和第二参数的常量性决定。当前仓库尚未找到把这些 Rust 签名选择器接入生产 builder 的直接调用边，所以这是该文件定义的预期局部流程，而非已验证的端到端生产链。

求值阶段按签名进入对应分型函数。整数的恒等路径直接返回；可能溢出的路径使用 `checked_abs`、`checked_pow` 或有限性检查。DECIMAL 路径创建独立结果值并调用 `MyDecimal::Round`、`DecimalAdd` 或 `DecimalSub`。对数先检查定义域，再用 `MathOutcome` 返回值和告警。CONV 先规范化正负进制、检查范围，截取合法前缀，解析成 `u64`，按有符号模式饱和/补码处理，最后用目标进制的大写字符输出。TRUNCATE 的整数路径对负小数位计算 `10^n` 后执行“先除后乘”，指数过大或 `i64::MIN` 小数位直接归零。

RAND 有两条不同生命周期：`MysqlRand::generate` 在同一 `Arc<MysqlRng>` 上连续推进序列；非常量种子调用 `rand_with_seed_first_gen`，每行以该种子（NULL 当 0）重建 RNG，因此只返回该种子的第一项。

## 数据与状态

除 RAND 外，函数都不保存跨调用可变状态；输入值经局部变量转换后返回新值。`MyDecimal` 结果通过新建 `MyDecimal::default()` 写入，避免直接修改借用的输入。`MathOutcome<T>` 用 `Option<T>` 表示 SQL NULL，并把告警放在独立 `Vec<MathWarning>` 中；目前告警枚举只有 `InvalidArgumentForLogarithm`。

`MysqlRand` 是本文件唯一的长期状态对象。它使用 `Arc<MysqlRng>` 允许包装对象克隆和共享同一随机序列，实际推进由 `MysqlRng::Gen` 完成。代码没有显式锁、线程、异步任务或通道；是否可跨线程安全共享取决于 `MysqlRng` 自身的并发实现，本文件没有额外作出线程安全保证。

CONV 的中间表示是 `u64`。负输入通过 `wrapping_neg` 保留二进制补码；负 `from_base` 启用有符号输入解释并在 `i64` 边界饱和，负 `to_base` 则输出带符号结果。CRC32 以 `i64` 承载完整的无符号 32 位校验值。

## 依赖与调用关系

- crate 边界：`pkg/expression/Cargo.toml` 声明 crate 名为 `astersql-expression`，`lib.rs` 为入口，并把 Go 包映射标为 `pkg/expression`。
- 外部/工作区依赖：`mathutil::{MysqlRng, NewWithSeed}` 提供 MySQL 兼容 RNG；`types_dependency::decimal::mydecimal` 提供 `MyDecimal`、舍入模式和加减/构造函数；`types_dependency::field::{Round, Truncate}` 提供浮点 ROUND/TRUNCATE；`crc32fast::hash` 计算 CRC32。Cargo 清单直接声明 `crc32fast = "1"`、工作区路径依赖 `astersql-util-mathutil` 和 `astersql-types`。
- 模块入口：`pkg/expression/lib.rs` 私有挂载 `builtin_math_kernel` 与 `builtin_math_vec_kernel`；测试模块 `builtin_math` 再导出标量实现，`builtin_math_test.rs` 通过 `use crate::builtin_math::*` 使用它。
- 内部调用边：RustCodeGraph 确认 Rust `conv` 调用 `get_valid_prefix`、`parse_radix_u64`、`format_radix_u64`；`ceil_decimal` 调用 DECIMAL 加法与 `NewDecFromInt`。源码还直接表明 `floor_int_to_decimal` 复用 `ceil_int_to_decimal`，`round_decimal` 复用 `round_with_frac_decimal`，便捷 `log*` 复用相应 `eval_log*`。
- 上游边界：RustCodeGraph 的 `callers` 对关键签名选择器和 `conv` 未给出本文件测试之外的生产调用，仓库级 `rg` 同样未发现外部 Rust 调用；因此生产上游当前记为“未接线或未验证”，不能虚构为执行器已调用。

## 错误处理与边界

- `abs_int(i64::MIN)` 返回 BIGINT 溢出；`pow`、`exp` 的结果非有限，以及 `cot` 的正切为零或倒数非有限时返回 DOUBLE 溢出。错误显示格式由 `MathError::Display` 固定为 `[types:1690]... value is out of range ...`。
- DECIMAL 的舍入、转换和加减错误原样包装为 `MathError::Decimal`。CEIL/FLOOR 对 `DecimalError::Truncated` 不是失败，而是根据符号修正截断整数；其他 DECIMAL 错误继续传播。
- LOG/LOG2/LOG10 的非正输入、双参数 LOG 的非法底数（非正或 1）及非法真数返回 NULL 并生成告警。便捷 `log*` 会丢失告警，若调用层需要语句告警必须使用 `eval_log*`。
- SQRT 负数、ACOS/ASIN 超出 `[-1, 1]` 返回 NULL；NaN 不满足这些有序比较，因而继续产生 NaN，测试明确覆盖该行为。
- CONV 的基数必须在 2 到 36；非法基数返回 `Ok(None)`，没有合法数字前缀则返回字符串 `"0"`。解析超过 `u64` 时返回 BIGINT UNSIGNED 溢出，而负基数的有符号解释会先在 `i64` 边界饱和。
- ROUND/TRUNCATE 的小数位先钳制到下游可接受范围；整数 TRUNCATE 对无法表示的 `10^n` 返回 0。`round_with_frac_int` 经 `f64` 中转，超大整数的精度风险应与 Go 行为一起评估，不能擅自改成不同算法。

## 并发与资源生命周期

本文件不分配外部资源，不持有文件、网络、事务、任务或通道。大部分临时 `String`、`Vec`、`MyDecimal` 在函数返回后按 Rust 所有权规则释放。

`MysqlRand` 的 `Arc` 克隆只延长同一个 RNG 的生命周期，不复制随机状态；最后一个 `Arc` 释放时 RNG 被销毁。连续调用 `generate` 会改变共享 RNG 的序列位置，调用顺序因而属于可观察语义。代码未加锁，也没有在本文件证明并发调用的顺序或线程安全性；扩展时不应把“使用 Arc”等同于“线程安全”。`rand_with_seed_first_gen` 则不共享状态，每次调用的临时 RNG 在取得第一个值后立即释放。

## 与 Go 版本的对应关系

Go `pkg/expression/builtin_math.go` 是语义对照源，包含 `builtinAbs*Sig`、`builtinRound*Sig`、`builtinCeil*Sig`、`builtinFloor*Sig`、`builtinLog*Sig`、`builtinRand*Sig`、`builtinPowSig`、`builtinConvSig`、`builtinCRC32Sig` 和 `builtinTruncate*Sig` 等类型专用实现。Rust 将 Go 中分散在 function class、signature struct 和 `eval*` 方法里的逻辑压缩成枚举分发与普通函数，但保留主要分支：有/无符号整数、DECIMAL/REAL、单/双参数、RAND 常量/非常量种子以及 CEIL/FLOOR 返回类型差异。

独立 Rust 测试 `pkg/expression/builtin_math_test.rs` 核对了 Go 语义的关键边界：`i64::MIN` ABS 溢出、正负 CEIL/FLOOR、负小数位 ROUND/TRUNCATE、对数/根号/反三角定义域、固定种子随机序列、POW/EXP/COT 溢出、CONV 的正负进制、UTF-8 CRC32、NaN 传播和签名选择。Go 的完整测试面还包括 `pkg/expression/builtin_math_test.go`；Rust 的向量语义由 `builtin_math_vec.rs` 及 `builtin_math_vec_test.rs` 覆盖，不应把标量测试视为向量接线证据。

已观察到的结构差异是：Go 文件中的签名类型直接参与 Go 表达式框架，而本 Rust 文件当前只有测试再导出，未找到生产调用者。因此“局部函数与 Go 逻辑对齐”已有源码和测试证据，“已替代 Go 生产路径”则没有证据，保持未验证。

## 扩展指南

新增数学函数时，先确定它是否需要按参数类型、参数个数或常量性分流：固定形态应扩展 `FixedMathFunction`/`fixed_signature`，类型相关函数还应扩展 `ScalarFuncSignature` 及对应选择器。随后添加独立标量实现；涉及 DECIMAL 时继续使用 `MyDecimal` 和明确的舍入模式，涉及 NULL/告警时优先返回类似 `MathOutcome` 的结构，涉及溢出时保持 `MathError` 的类型名和表达式文本与 Go 一致。

若修改 FLOOR/CEIL 或 ROUND/TRUNCATE，必须同步审查 `get_eval_type_for_floor_and_ceil`、`floor_and_ceil_unsigned`、`calculate_decimal_for_round_and_truncate`，避免只改求值而遗漏返回元数据。修改 CONV 时同时覆盖前缀扫描、正负基数、`u64/i64` 边界和二进制字面量；修改 RAND 时必须区分共享序列和逐行首值，不能把两条路径合并。

测试应继续放在独立文件 `pkg/expression/builtin_math_test.rs`，并按需要同步 `builtin_math_16_aster_unit_test.rs`、Go `builtin_math_test.go` 以及向量测试 `builtin_math_vec_test.rs`，不要把测试嵌入生产源文件。若目标是接入生产执行链，还需在表达式 builder/pb 分发处增加显式接线并用调用边和端到端测试证明；仅新增本文件公开函数或枚举变体不足以证明可用。主要兼容风险是 MySQL 的 NULL/告警/溢出文本、有符号 CONV、DECIMAL 精度和 RAND 序列；性能风险集中在逐值字符串分配、二进制字面量展开以及误用标量路径替代向量执行。

## 验证依据

- 完整读取：`pkg/expression/builtin_math.rs`（949 行），确认常量、类型、公开/私有函数及无条件编译项；文件本身没有内嵌测试模块。
- crate 与模块证据：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`；确认 crate 名、依赖、私有模块挂载、测试再导出与独立测试注册。包目录没有 `doc.go`，因此无可读取的 Go 包契约文件。
- Go 对照：`pkg/expression/builtin_math.go` 及 `pkg/expression/builtin_math_test.go`；按签名族核对类型专用实现与测试面。
- Rust 测试：`pkg/expression/builtin_math_test.rs`；核对 ABS/ROUND/CEIL/FLOOR、定义域、RAND、溢出、CONV/CRC32、三角函数、TRUNCATE 和签名选择的断言。向量边界参考 `pkg/expression/builtin_math_vec.rs` 与 `builtin_math_vec_test.rs`，但未把它们当作本文件的生产调用证据。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/expression/builtin_math.rs --offset 1 --limit 500` 与 `--offset 500 --limit 500` 覆盖全文件；`callees conv`、`callees ceil_decimal` 验证关键下游边；关键 `callers` 查询无生产上游结果。
- 直接搜索：对 `builtin_math::`、各 `*_signature`、`eval_log`、`conv_binary_literal` 等进行仓库级 Rust 搜索，排除本源文件和直接测试后无结果，据此把生产接线状态明确标为未验证，而不是推测已支持。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定命令检查目标存在且恰有 11 个固定二级章节，并人工复核唯一新增生产物为本说明文件。

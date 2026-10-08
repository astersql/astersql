# `pkg/types/mydecimal.rs`

## 文件定位

本文件实现 AsterSQL 对 MySQL `DECIMAL/NUMERIC` 的定长精确十进制表示，是 Go `pkg/types/mydecimal.go` 的 Rust 对照实现。它并非由 `pkg/types/lib.rs` 直接声明为模块：`pkg/types/internal/decimal/lib.rs` 使用 `#[path = "../../mydecimal.rs"] pub mod mydecimal` 将其编入 `astersql-types-decimal`，根 crate 再通过 `pub use types_decimal as decimal` 暴露为 `astersql_types::decimal::mydecimal`。因此服务运行路径中的典型引用是 `pkg/session/runtime/row_codec.rs`、`pkg/session/runtime/relational_value.rs` 和 `pkg/executor/typed_hash_agg.rs` 对该重导出路径的使用。

直接归属的内部 crate 是 `pkg/types/internal/decimal/Cargo.toml`；它依赖 `num-bigint`、`num-traits` 和 `serde_json`。根 `pkg/types/Cargo.toml` 仅以路径依赖 `types-decimal` 接入该实现，没有控制本文件行为的 feature。

## 核心职责

- 用固定的 9 个 `i32` word 保存最多约 81 位十进制数字，每个 word 采用基数 `10^9`；同时保存整数位数、小数位数、结果 scale 和符号（`MyDecimal`、`maxWordBufLen`、`digitsPerWord`、`wordBase`）。
- 在外部文本、整数、浮点数、Parquet 有符号大端整数、MySQL 可排序 DECIMAL 二进制、JSON 内部字段对象和内存表示之间转换（`FromString`、`FromInt`、`FromUint`、`FromFloat64`、`FromParquetArray`、`FromBin`、`WriteBin`、`MarshalJSON`、`UnmarshalJSON`）。
- 实现小数点移位、舍入、比较、取负、加减乘除和取模，并在固定容量不足时以 `DecimalError` 区分截断、溢出、除零和坏输入（`Shift`、`Round`、`Compare`、`DecimalNeg`、`DecimalAdd`、`DecimalSub`、`DecimalMul`、`DecimalDiv`、`DecimalMod`）。
- 提供 SQL 执行所需的稳定编码和相等性键：`ToBin`/`FromBin` 对应 MySQL 二进制 DECIMAL，`ToHashKey` 去除无意义尾零并附加有效小数位数，使数值相同但 scale 不同的值形成同一数值键。

## 主要符号

- `RoundMode = i32` 与 `ModeHalfUp`、`ModeTruncate`、`ModeCeiling`：保留 Go API 的舍入模式编码。`ModeCeiling` 的实现特意保持 Go 的逐 word 行为，不能等同于任意精度库的一般 ceiling。
- `DecimalError`：`Truncated`、`Overflow`、`DivByZero`、`BadNumber`、`TruncatedWrongValue`、`InvalidJson` 六类可比较错误；同时实现标准 `Display` 和 `Error`。
- `MyDecimal { digitsInt, digitsFrac, resultFrac, negative, wordBuf }`：公开字段保持 Go 结构和序列化形状；派生 `Clone`、`Default`、`Eq`、`PartialEq`。`MyDecimalStructSize = 40` 是布局约束，并由 `pkg/types/mydecimal_test.rs` 检查。
- 内部表示辅助：`set_parts`/`set_unscaled` 写回固定 word 缓冲，`unscaled`/`word_value` 转成 `BigInt`，`fit_unscaled` 按容量截断或报溢出，`significant_integer_digits`/`significant_fraction_digits` 计算有效精度。
- 文本和数值 API：`String` 按 `resultFrac` half-up 后显示，`ToString` 按当前 `digitsFrac` 原样输出；`FromString` 支持符号、小数点和指数；`ToInt`/`ToUint` 在返回截断值的同时返回状态。
- 编码 API：`DecimalBinSize` 计算 `(precision, frac)` 的编码长度，`WriteBin` 追加写入，`FromBin` 返回应消耗的字节数及状态，`DecimalPeak` 从两字节头读取编码长度。
- 运算 API：`DecimalNeg` 返回副本；`DecimalAdd`/`DecimalSub` 经 `decimal_add_sub` 对齐 scale；`DecimalMul` 保留 word 级乘法及 guard digits；`DecimalDiv`/`DecimalMod` 分别产生商和余数。
- 构造辅助：`NewDecFromInt`、`NewDecFromUint` 是生产构造器；`NewDecFromFloatForTest`、`NewDecFromStringForTest` 明确为测试便捷入口；`NewMaxOrMinDec` 构造指定 precision/frac 的全 9 边界值。

## 执行流程

1. 文本解析从 `FromString` 开始：清空目标，读取可选符号、整数和小数前缀，在 9 位 word 容量内调用 `set_parts`；超长整数保留低 81 位并报 `Overflow`，小数容量不足则截断并报 `Truncated`。若有 `e/E`，`parse_exponent_best_effort` 解析指数并由 `Shift` 移动小数点；非空非法后缀保留已解析前缀并返回截断状态。
2. 显示时，`ToString` 根据 `digitsInt`/`digitsFrac` 逐 word 格式化，完整 word 补足 9 位；`String` 先调用 `round_into(..., resultFrac, ModeHalfUp, true)`，因此“存储的小数位”和“默认显示的小数位”可能不同。
3. `Shift` 先由 `digit_bounds` 找有效数字边界，计算移动后的整数/小数 word 数；超过九个 word 时优先丢弃低位小数并返回 `Truncated`，整数仍放不下则返回 `Overflow`。非 9 位倍数的移动由 `mini_left_shift`/`mini_right_shift` 在相邻 word 间搬运数字。
4. `Round`/`round_into` 按目标 scale 定位被丢弃数字，执行 truncate、half-up 或兼容性的 ceiling，处理跨 word 进位；进位挤满缓冲时更新整数位数并返回截断或溢出状态。
5. 加减先用 `align_values` 把两个数提升到相同 scale，再由 `decimal_add_sub` 调用 `fit_unscaled` 回填固定容量。乘法在 word 缓冲上累加乘积和进位，按整数/小数容量裁剪。除法按输入 scale、`frac_incr` 和可用 word 数放大分子后整除；取模先对齐 scale 再计算余数。
6. MySQL 二进制编码由 `WriteBin` 按 precision/frac 拆成部分 word 和完整 4 字节 word；负数按位取反，首字节再异或 `0x80`，使编码保持符号和排序语义。`FromBin` 逆转该过程，并验证每个 word 的取值范围及目标容量。
7. `ToHashKey` 以有效整数位和去尾零后的有效小数位调用 `ToBin`，最后附加有效小数位数；聚合去重等调用方因此不受表示 scale 的无意义差异影响。

## 数据与状态

`wordBuf: [i32; 9]` 是唯一数字载体。整数 word 在前，小数 word 在后；不足 9 位的末尾小数在 word 内左对齐。合法 word 范围是 `0..=999_999_999`。`digitsInt` 和 `digitsFrac` 描述当前表示，`resultFrac` 描述运算结果或 `String` 应保留的 scale，它不一定等于 `digitsFrac`。`negative` 只表示非零负数；`set_parts`、除法和乘法等路径会消除负零。

若某算法需要便捷的任意精度中间值，`BigInt` 只作为计算桥梁；结果仍必须通过容量检查落回九个 word。`word_scale` 把小数位向上扩到完整 word，`word_value` 因而会包括末 word 中的 guard digits；这对乘除和与 Go 的 word 级语义一致非常重要。

固定上限包括 9 个 word、每 word 9 位和最大 scale 30（`MAX_DECIMAL_SCALE` 用于部分公共 API 的参数/结果限制）。MySQL 二进制布局另由 `DIG2BYTES` 描述 0..9 位部分 word 所占字节数，最大临时编码缓冲为 40 字节。

## 依赖与调用关系

编译接线为 `pkg/types/internal/decimal/lib.rs` → 本文件 → `pkg/types/lib.rs` 的 `decimal` 重导出。直接下游依赖只有标准库、`num_bigint::{BigInt, Sign}`、`num_traits::{Signed, ToPrimitive, Zero}` 和 `serde_json::Value`；没有 I/O、存储或会话依赖。

RustCodeGraph 将 `DecimalMul` 的 Rust 定义解析到本文件，并确认其内部调用 `digits_to_words`、`clear`，引用 `maxWordBufLen`、`wordBase`、`notFixedDec` 和 `MAX_DECIMAL_SCALE`；对 `FromString` 的图查询确认其调用 `parse_exponent_best_effort`、`set_parts`、`Shift`、`IsZero` 和 `maxDecimal`。图中同时存在 Go 同名定义，查询时必须以 `pkg/types/mydecimal.rs` 路径区分。

代表性上游包括：`pkg/session/runtime/row_codec.rs` 从行编码恢复 `MyDecimal`；`pkg/session/runtime/relational_value.rs` 调用 `DecimalDiv` 和舍入处理 SQL 值；`pkg/executor/typed_hash_agg.rs` 使用 `DecimalDiv` 计算聚合结果；`pkg/types/parser_driver/value_expr.rs` 从 SQL 字面量调用 `FromString`；聚合、chunk、序列化和表达式模块则通过 `astersql_types::decimal::mydecimal` 或根 types 门面消费该类型。

## 错误处理与边界

- `Truncated` 表示仍产生可用结果但丢失低位或只消费合法前缀；`Overflow` 表示整数容量或目标 precision 不足。许多 API 会同时返回结果和错误，调用者不能把任意 `Err` 理解为“结果未写入”。
- `FromString` 对空输入、只有符号/小数点等返回 `TruncatedWrongValue`；对非法后缀通常保留合法前缀并返回 `Truncated`；极端指数可能饱和为全 9、清零或返回 `BadNumber`。非有限 `f64` 会清零并返回 `TruncatedWrongValue`。
- `ToBin`/`DecimalBinSize` 拒绝负 precision/frac、`frac > precision`、precision 超容量或 scale 超限。目标整数位不足可返回 `Overflow`，目标小数位不足可返回 `Truncated`；编码仍可能可供 `FromBin` 恢复裁剪后的值。
- `FromBin` 会拒绝空输入、非法 word 和整数 word 超容量；小数 word 超容量时只保留可容纳部分并返回 `Truncated`。该 Rust 实现显式拦截 Go 的畸形整数编码可能越界的路径。
- `DecimalDiv`/`DecimalMod` 在检查除零前清空输出，并预置结果 scale；除零返回 `DivByZero`，输出保持相应的零状态。`Round`、`Shift` 和算术均需保留 `resultFrac` 的兼容语义。
- `UnmarshalJSON` 先解析完整 JSON，再按输入顺序处理字段以保留 Go 对重复键的语义；`null`、空对象和未知字段产生默认值，非法字段类型、范围或数组长度返回 `InvalidJson`，且以临时 replacement 避免部分更新污染原值。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、全局可变状态或外部资源。常量表是只读的；每个 `MyDecimal` 自有固定数组，`Clone` 为值拷贝。修改型 API 使用 `&mut self` 或显式 `&mut MyDecimal` 输出参数，Rust 借用规则防止同一值在安全代码中被并发可变访问。

临时资源仅为栈上数组、`String`/`Vec<u8>` 和计算过程中的 `BigInt`，均按 Rust 所有权自动释放。固定 `wordBuf` 避免数值本体的动态扩容，但文本、JSON、hash key、二进制输出和大整数中间计算仍会分配；热路径扩展需关注这些分配。Go 测试曾通过修改 `wordBufLen` 模拟小容量，Rust 生产实现没有该全局钩子，`pkg/types/mydecimal_test.rs` 改为在真实 81 位边界覆盖相同分支，因此相关测试可并行地共享只读常量。

## 与 Go 版本的对应关系

主对照文件是 `pkg/types/mydecimal.go`，Rust 保留了其公开命名、字段顺序、错误分类、9 位 word 布局、显示 scale、MySQL 二进制编码和主要算法入口。`pkg/types/mydecimal_test.go` 是原始行为样例；`pkg/types/mydecimal_test.rs` 将其整数/浮点转换、round、shift、解析、hash/bin、比较、四则运算、JSON 等表驱动用例迁移为独立 Rust 测试。

实现手段并非逐行翻译：Rust 在 scale 对齐和部分转换中使用 `BigInt`，但舍入、移位和乘法保留必要的 word 级逻辑，以免丢失 Go 的 guard digits、进位和截断元数据。Rust 以 `Result<(), DecimalError>` 代替 Go 的包级错误值，并以值返回替代若干 Go 指针返回。对安全性有意收紧的一点是 `FromBin` 在索引固定数组前验证畸形编码的整数 word 数。

测试迁移也记录了差异：Go 的 `wordBufLen` 测试钩子是可变包级状态，Rust 始终使用生产上限 9，通过 81 位真实边界复现截断/溢出；`ModeCeiling` 不是重新定义的数学 ceiling，而是保持 Go 当前不完整的兼容行为。扩展时应以 Go 源码和两侧测试共同判定语义，不能仅凭 `BigInt` 的直觉行为替换现有 word 规则。

## 扩展指南

- 新增解析或格式化规则时，优先修改 `FromString`、`parse_exponent_best_effort`、`String`/`ToString`，并在独立的 `pkg/types/mydecimal_test.rs` 增加与 `pkg/types/mydecimal_test.go` 对应的前缀解析、指数、负零、尾零和 81 位边界用例；不要把测试写入本源文件。
- 新增舍入模式或调整 scale 时，同步审查 `round_into`、`Shift`、`String`、`DecimalDiv` 与 `resultFrac` 不变量。特别验证跨 9 位 word 的进位、负 scale、满缓冲进位和 `ModeCeiling` 的 Go 兼容性。
- 修改四则运算时，区分使用 `unscaled` 的精确小数位和使用 `word_value`/`word_scale` 的完整 word/guard-digit 语义；同步覆盖正负组合、零、别名输出、截断仍有结果、溢出和除零后输出状态。
- 修改二进制或 hash 编码时，以 `DecimalBinSize`、`WriteBin`、`FromBin`、`ToHashKey`、`HashKeySize` 为完整变更面，并检查 `pkg/session/runtime/row_codec.rs`、聚合 spill 序列化和分组/去重消费者；字节序、负数取反、首位符号翻转及尾零归一化属于兼容协议。
- 修改布局时必须同步 `MyDecimalStructSize`、所有结构序列化字段、内存估算调用方及布局测试。由于字段公开且 JSON 保存内部形状，字段类型/顺序/范围变化存在持久化与跨语言兼容风险。
- 性能优化应优先做基准验证；`pkg/types/mydecimal_benchmark_test.rs` 覆盖舍入、浮点转换、二进制编码和 hash key 热路径。不得为了减少 `BigInt` 分配而简化 Go 已覆盖的容量、guard digit 或错误优先级语义。

## 验证依据

- 源码全貌：`pkg/types/mydecimal.rs`（1809 行），重点符号为 `MyDecimal`、`FromString`、`Shift`、`Round`、`WriteBin`、`FromBin`、`DecimalAdd`/`Sub`/`Mul`/`Div`/`Mod`。
- 编译边界：`pkg/types/internal/decimal/lib.rs` 的 `#[path = "../../mydecimal.rs"]` 接线；`pkg/types/internal/decimal/Cargo.toml` 的 `num-bigint`、`num-traits`、`serde_json` 依赖；`pkg/types/Cargo.toml` 和 `pkg/types/lib.rs` 的路径依赖及 `decimal` 重导出。目标包没有 `pkg/types/doc.go`。
- Go 对照：`pkg/types/mydecimal.go`；Go 测试 `pkg/types/mydecimal_test.go` 和性能测试 `pkg/types/mydecimal_benchmark_test.go`。
- Rust 测试：`pkg/types/mydecimal_test.rs`（独立测试文件，覆盖转换、编码、舍入、比较、算术、移位、解析、JSON 和审计边界）；`pkg/types/mydecimal_9_aster_unit_test.rs`；性能面 `pkg/types/mydecimal_benchmark_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`query MyDecimal --limit 20` 定位本文件结构体和常量；`callees DecimalMul --limit 20` 与 `callees FromString --limit 20` 核实上述内部调用边。图查询同名 Go/Rust 符号时按文件路径消歧。
- 上游代码搜索核验：`pkg/session/runtime/row_codec.rs`、`pkg/session/runtime/relational_value.rs`、`pkg/executor/typed_hash_agg.rs`、`pkg/types/parser_driver/value_expr.rs`，证明该模块经 `astersql-types-decimal` 重导出参与行编解码、SQL 十进制运算和聚合执行。
- 本任务只新增说明文档，未运行 Cargo；交付结构检查要求文档存在且恰含本文 11 个固定二级标题。

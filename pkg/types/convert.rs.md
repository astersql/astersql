# `pkg/types/convert.rs`

## 文件定位

本文件实现 MySQL 兼容的标量类型转换规则，重点覆盖整数、浮点、字符串、科学计数法和二进制字面量的互转；启用 `types-integration` feature 时，还补充时间、Duration、DECIMAL 与 Binary JSON 的转换入口。源码位置是 [`pkg/types/convert.rs`](convert.rs)，但它并不是由 `pkg/types/lib.rs` 直接声明：`pkg/types/internal/scalar/lib.rs` 通过 `#[path = "../../convert.rs"] mod convert;` 编译本文件并以 `pub use convert::*` 导出，因此默认归属 `astersql-types-scalar` crate；根 `astersql-types` crate 再以 `pub use types_group_1 as scalar` 和若干显式再导出向上暴露标量能力。

`pkg/types/Cargo.toml` 将 `astersql-types-scalar` 作为 `types-group-1` 依赖，并同时组合 datum、time、decimal、JSON、metadata 等子 crate。真正声明 `types-integration` feature 的是 `pkg/types/internal/scalar/Cargo.toml`：该 feature 打开 decimal、file-group、json-binary 和 metadata 的可选依赖；时间 crate 是非可选依赖。由此，文件 28、602、643、921 行附近的条件编译代码属于 scalar crate 的集成形态，而不是根 crate 自己声明的 feature。

在应用主链中，本文件处于 SQL 表达式/Datum 与底层 Rust 数值表示之间。例如 RustCodeGraph 将 `pkg/expression/builtin_cast_vec.rs::cast_string`、`json_to_int` 和 `pkg/types/datum.rs::toSignedInteger` 标为 `StrToInt`、`ConvertFloatToInt` 等入口的上游；这些入口负责把 MySQL 的截断、警告、溢出裁剪语义落实为具体值和错误。

## 核心职责

1. **提供 MySQL 整数边界。** `IntegerUnsignedUpperBound`、`IntegerSignedUpperBound`、`IntegerSignedLowerBound` 将 MySQL 类型码映射到 Rust `u64`/`i64` 边界，并为 ENUM 固定使用 65535。未知或不适用类型会 panic，调用者必须先保证类型码合法。
2. **在转换失败时保留可用值。** `ConvertFloatToInt`、`ConvertIntToUint` 等函数不只返回错误，还通过 `ValueResult<T> = Result<T, ErrorWithValue<T>>` 携带裁剪后的边界值。这是 SQL 执行层在“报错、转 warning、继续使用裁剪值”之间决策的基础。
3. **解析 MySQL 风格数字前缀。** `StrToInt`、`StrToUint`、`StrToFloat` 先去除首尾空白，再由 `getValidIntPrefix`/`getValidFloatPrefix` 提取合法前缀；非法后缀交给 `Context::HandleTruncate` 决定是错误、warning 还是忽略。
4. **避免大整数经浮点中转丢精度。** `floatStrToIntStr` 和 `convertScientificNotation` 直接重排十进制字符并依据小数点后一位舍入，不先解析成 `f64`。这保证接近 `i64`/`u64` 极值的文本仍可精确判断。
5. **格式化受支持的标量值。** `ToString` 以 `Any` 下转型 bool、整数、浮点、`String`、`Vec<u8>`、`BinaryLiteral`；集成 feature 下再支持 `Time`、`Duration`、`MyDecimal`、`Enum`、`Set`、`BinaryJSON`。
6. **在集成形态下桥接复杂类型。** `integrated` 模块处理 DECIMAL 到无符号整数、文本/数字到时间类型，以及 Binary JSON 到整数、浮点和 DECIMAL 的分派。

## 主要符号

- 常量 `UnspecifiedLength: i32 = -1` 是未指定字段长度的哨兵；`maxUintStr` 与 `minIntStr` 是精确溢出比较和裁剪所需的十进制文本边界。
- 私有辅助 `mysql_type_name`、`overflow` 统一构造带目标 MySQL 类型名的越界错误；`ErrWithValue` 统一构造“值 + 共享错误”；`apply_truncate`、`handled_truncate` 统一进入 `Context::HandleTruncate`。
- `truncateStr(String, i32) -> String` 按 UTF-8 字节长度截断，但会向前退到字符边界，避免生成无效 Rust 字符串。
- `RoundFloat` 使用 `round_ties_even`；`ConvertFloatToInt`、`ConvertIntToInt`、`ConvertUintToInt`、`ConvertIntToUint`、`ConvertUintToUint`、`ConvertFloatToUint` 完成基本数值范围检查与裁剪。
- `convertScientificNotation` 展开 `e`/`E` 表示；`convertDecimalStrToUint` 在展开后依据小数第一位四舍五入，并在加一前检查上界。
- `StrToInt`、`StrToUint`、`StrToFloat` 是文本到基础数值的公开入口；`getValidIntPrefix`、`getValidFloatPrefix`、`roundIntStr`、`floatStrToIntStr` 是其解析流水线。
- `ToString(&dyn Any)` 是动态类型字符串化入口；私有 `format_float` 使用 `ryu` 的最短有限浮点表示，遇到指数形式再尝试展开。
- `integrated::ConvertDecimalToUint`、`StrToDateTime`、`StrToDuration`、`NumberToDuration`、`ConvertJSONToInt64`、`ConvertJSONToInt`、`ConvertJSONToFloat`、`ConvertJSONToDecimal` 仅在 `types-integration` 下存在，并由文件末尾 `pub use integrated::*` 导出。

文件没有自定义 struct、enum、trait 或 impl；状态通过入参、返回值及 `Context` 中的 flags/warning 处理器传递。

## 执行流程

### 文本转整数

`StrToInt(ctx, value, isFuncCast)` 先 `trim`，再调用 `getValidIntPrefix`。非 CAST 路径先以 `getValidFloatPrefix` 接受小数/科学计数法，再交给 `floatStrToIntStr` 做十进制字符级移位和舍入；CAST 路径只扫描可选首符号与连续数字。随后以 `i64::parse` 解析：成功时保留先前的截断 warning/错误，失败时按符号裁剪为 `i64::MIN` 或 `i64::MAX`。

`StrToUint` 使用同一前缀流水线，但特殊接受 `-0`、`-00` 等“负零”；其他负数返回值 0 和溢出错误。正数会移除 `+` 后解析，超出 `u64` 时返回 `u64::MAX` 与错误。

### 文本转浮点与整数舍入

`getValidFloatPrefix` 维护 `saw_dot`、`saw_digit` 和指数位置，仅允许开头或指数后的正负号、指数前一个小数点及一段指数。NUL 会把输入收缩到已识别前缀；末尾单独的 `e`/`E` 按 MySQL 行为退回指数前文本。空输入在函数 CAST 场景返回 `"0"` 且无错，其他空/非法输入依据 Context 处理截断。

`StrToFloat` 解析该前缀；若 Rust 解析结果为正负无穷，则裁剪为 `±f64::MAX` 并通过 `HandleTruncate` 返回。`roundIntStr` 只看小数第一位，执行十进制逐位进位；因此这是普通“四舍五入”，与 `RoundFloat` 的 ties-to-even 路径不同，调用者不可混用两者的舍入约定。

### 基础数值转换

所有 `Convert*` 数值函数先通过 `RoundFloat`（仅浮点入口）得到候选值，再比较目标上下界。越界分支返回最近边界和 `overflow` 错误。负数转无符号时，`Flags::AllowNegativeToUnsigned` 决定是直接裁剪到 0，还是先按 Rust/Go 的补码转换得到 `u64` 后再做目标上界检查。`ConvertFloatToUint` 还显式拒绝非有限值以及达到 `2^64` 的值，避免 `as u64` 的饱和行为掩盖溢出。

### 复杂类型转换（feature）

`StrToDuration` 去空白并排除负号、小数尾部来计算整数位长度；长度至少 12 时先尝试 DATETIME，成功返回 `(ZeroDuration, time, false)`，否则解析 Duration。Duration 截断错误经 Context 处理，返回三元组最后一位 `true` 表示有效结果位于 Duration 槽。

`NumberToDuration` 对超过 `TimeMaxValue` 的正数先尝试按 DATETIME 数字解析；失败才裁剪到最大 Duration。普通范围内按 `HHMMSS` 拆分，小时、分、秒非法则返回 `ZeroDuration` 与截断错误，负号最终施加到内部 duration 值。

`ConvertJSONToInt`/`Float`/`Decimal` 以 `TypeCode` 分派：对象、数组、opaque 和时间类 JSON 返回零值并触发截断； literal 的 false/true 映射 0/1，null 触发截断；数值直接转换；字符串复用 `StrToInt`、`StrToUint` 或 `StrToFloat`。整数入口继续使用目标 MySQL 类型边界，因此不是简单的 Rust cast。

## 数据与状态

- `Context` 是本文件唯一具有外部可观察状态的协作对象。`Flags` 控制负数转无符号是否合法以及截断如何处理；`HandleTruncate` 可能返回错误、吞掉错误或把它追加为 warning。函数本身不保存全局可变状态。
- `ValueResult<T>` 的错误分支包含 `ErrorWithValue<T>`。其中 `value` 是 SQL 兼容的回退/裁剪值，`error` 是共享错误；调用者若要继续执行，必须显式决定是否使用该值，不能把普通 `Err` 等同于“没有结果”。
- 数字前缀函数返回拥有所有权的 `String`，避免持有输入切片；科学计数法展开也创建新字符串。极大指数可能导致补零分配，但 `checked_add` 会先捕获小数点位置的 `i64` 算术溢出。
- `ToString` 只读借用 `&dyn Any`。`Vec<u8>` 使用 `String::from_utf8_lossy`，无效字节会替换为 U+FFFD；这与 Go 的 `string([]byte)` 可保留任意字节不同，是扩展或兼容检查时需要显式关注的语义差异。
- 条件编译决定可见类型集合：默认 scalar crate 不引入复杂类型；打开 `types-integration` 后，集成模块及 `ToString` 的复杂类型分支才被编译。

## 依赖与调用关系

### 上游

- `pkg/types/internal/scalar/lib.rs` 是真实模块入口，负责加载并再导出本文件。
- `pkg/types/lib.rs` 将 scalar 子 crate 作为 `crate::scalar` 暴露；`pkg/types/convert_test.rs` 正是通过 `use crate::scalar as conv` 测试默认转换入口。
- RustCodeGraph 的定向查询显示，`pkg/expression/builtin_cast_vec.rs::cast_string` 调用 `StrToInt`，`json_to_int` 调用数值转换入口；`pkg/types/datum.rs::ConvertToMysqlYear`、`toSignedInteger` 也位于这些入口上游。实际代码搜索还确认 `builtin_cast_vec.rs` 通过 `types_dependency::scalar::ConvertFloatToInt` 调用导出的 scalar API。
- feature 下的 `StrToDuration` 由 `pkg/types/internal/scalar/migration_aster_unit_test.rs::str_to_duration_matches_go_cases` 直接覆盖。根 datum 子 crate另有自己的 JSON 转换实现；不能因为符号同名就把 `pkg/types/internal/datum/lib.rs` 的实现误记为本文件的直接调用者。

### 下游

- 基础依赖来自 `crate::{BinaryLiteral, Context, ErrorWithValue, Flags, ValueResult, errors, mysql}`；在 scalar crate 中，这些名称由 `pkg/types/internal/scalar/lib.rs` 的再导出和相对路径模块提供。
- `StrToInt` 下调 `getValidIntPrefix`、`overflow`、`ErrWithValue`；`getValidIntPrefix` 下调 `getValidFloatPrefix`、`floatStrToIntStr`；`getValidFloatPrefix` 下调 `handled_truncate`。
- `ConvertFloatToInt` 下调 `RoundFloat`、`overflow`、`ErrWithValue`。`convertScientificNotation` 被 `convertDecimalStrToUint`、`floatStrToIntStr`、`ToString`/`format_float` 复用。
- `ToString` 的浮点格式依赖 `ryu::Buffer`；`ryu` 在 `pkg/types/internal/scalar/Cargo.toml` 中声明，而非根 `pkg/types/Cargo.toml`。
- 集成模块下调 time、decimal 和 json-binary 子系统提供的 `ParseTime`、`ParseDuration`、`ParseDatetimeFromNum`、`MyDecimal::From*`、`BinaryJSON::Get*` 等 API。

RustCodeGraph 的全库结果包含 Go 同名函数和部分通用同名符号；本文只把带 `pkg/types/convert.rs` 定义限定、且可由 Rust 模块装配或源码调用复核的边作为 Rust 调用关系。

## 错误处理与边界

- 整数类型边界函数对未知类型码使用 panic；它们是受约束的内部/公共工具，不是容错型解析器。
- 数值越界一般采用“裁剪值 + 错误”。例如有符号转换裁到上下界，浮点转无符号裁到 0 或目标上界，科学计数法转整数严重溢出裁到 `minIntStr`/`maxUintStr`。
- `Context::HandleTruncate` 是截断策略的唯一仲裁点。严格 Context 保留错误；IgnoreTruncateErr 或 TruncateAsWarning 可让同一输入成功返回回退值。`pkg/types/convert_test.rs::test_str_to_num` 明确覆盖空串、非法前缀、非法后缀和 `±1e649` 在不同 flags 下的结果。
- `truncateStr` 的 `flen` 被转换为 `usize`；约定只有 `UnspecifiedLength == -1` 或非负长度。其他负值不属于合法输入契约。
- `convertScientificNotation` 要求指数段可解析为 `i64`。指数文本非法返回空回退字符串与解析错误；小数点位置加指数溢出时返回 BIGINT range 错误。正常但非常大的指数仍会申请对应数量的零，调用端应避免让未受限用户输入绕过上层长度约束。
- `convertDecimalStrToUint` 仅按小数首位舍入；负数返回 0 与溢出错误；在舍入会越过上界时返回上界值。测试覆盖 `u64::MAX` 邻域。
- `getValidFloatPrefix` 对 NUL 的处理不把 NUL 后内容计为截断；这是与普通非法字符分支不同的兼容行为，测试用 `"1001001\0\0\0"` 固定了该约定。
- `ToString` 不支持的动态类型返回包含 `TypeId` 的错误。它只调用 `ryu::Buffer::format_finite`，因此调用者不应把 NaN/Infinity 当作已支持的稳定格式化输入。
- JSON 未知 TypeCode 返回显式错误；对象、数组、null 等已知但不适合数值转换的类型走截断策略，二者不可合并为同一种错误。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、channel、事务或 I/O 句柄；每次转换只操作栈上数值和本地拥有的字符串/缓冲区，因此函数本身可重入。`ryu::Buffer` 在每次格式化时局部创建，不跨调用共享。

并发可观察性仅来自调用方传入的 `Context`：`HandleTruncate` 可能通过其 warning appender 记录警告。源码只以共享引用调用 Context，但是否能跨线程共享、warning 的顺序与同步保证由 Context/WarningAppender 的实现约束，不由本文件新增保证。扩展本文件时不应引入全局缓存来改变这一无共享状态特征，除非同时证明 Context 和错误顺序语义不受影响。

所有返回的 `String`、`MyDecimal`、`Duration`、`Time` 和 `BinaryJSON` 派生值均由返回值所有者管理，没有手工释放或跨调用借用。错误使用 `SharedError` 共享所有权，其生命周期随 `ErrorWithValue` 或 warning 收集器延长。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/types/convert.go`，函数布局和主流程基本逐项对应：三组整数边界、六个基础数值转换、科学计数法、DECIMAL 字符串转无符号、`StrTo*`、Duration、JSON 和 `ToString` 均有同名或同职责实现。`pkg/types/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/types"` 也记录了移植归属。

已核对的关键一致性包括：

- `ConvertFloatToInt` 都先调用 ties-to-even 的 `RoundFloat` 并在上界浮点表示相等时允许返回上界；各整数类型边界及 ENUM=65535 一致。
- `StrToUint` 都只接受负零；`getValidFloatPrefix` 都接受 `1e+1`、把末尾 `e` 回退为指数前文本，并对非法后缀调用 Context 截断处理。
- `floatStrToIntStr` 都用十进制字符移动避免 `u64::MAX` 精度损失，并在严重溢出时返回 `minIntStr`/`maxUintStr`。
- `StrToDuration` 都以 12 位为 DATETIME 优先阈值；`NumberToDuration` 都按 `HHMMSS` 拆分并验证小时、分、秒。
- JSON 的对象/数组/时间类、literal、数值和字符串分支与 Go switch 对应。

需要保留并继续验证的 Rust 差异包括：

- Rust 的 `truncateStr` 会回退到 UTF-8 字符边界；Go 版本直接按字节切片，可能产生非 UTF-8 字节串。Rust `String` 类型无法表达后者，因此这里是有意的安全适配，不应误称为逐字节完全等价。
- Rust `ConvertFloatToUint` 用有限性检查、`2^64` 阈值和 `as u64`，Go 用 `big.Float.Uint64` 的精度状态；现有边界行为意图相同，但修改时必须对 `u64::MAX` 附近和非有限值做对照测试。
- Rust `ToString(Vec<u8>)` 使用 lossy UTF-8，Go `string([]byte)` 保留任意字节；同时 Rust 只列出 `i32` 而 Go 接受平台 `int`。这是可见兼容边界。
- 复杂类型代码被 `types-integration` 隔离；默认根 crate 的 JSON 转换实际还由 `pkg/types/internal/datum/lib.rs` 提供。不能只修改本文件 feature 分支就假定根 crate 的所有 JSON 行为已同步。

Go 测试 `pkg/types/convert_test.go` 是语义基线，Rust 独立测试 `pkg/types/convert_test.rs` 覆盖默认 scalar 路径；feature 集成分支另由 `pkg/types/internal/scalar/migration_aster_unit_test.rs` 提供直接测试。

## 扩展指南

- 新增基础数值类型或 MySQL 类型码时，先扩展三组 `Integer*Bound` 与 `mysql_type_name`，再检查所有 `Convert*` 调用是否需要该类型；同步在独立的 `pkg/types/convert_test.rs` 增加上下界、刚好越界、负数和错误内回退值用例。
- 修改字符串语法时，应把扫描规则放在 `getValidFloatPrefix`/`getValidIntPrefix`，把十进制重排放在 `floatStrToIntStr`/`convertScientificNotation`，不要先转 `f64`。至少覆盖空串、符号、小数点、指数、NUL、非法后缀、超长指数和 `i64/u64` 边界，并与 `pkg/types/convert_test.go` 同类表格逐项对照。
- 新增 `ToString` 类型前，判断它应在默认 scalar 层还是 `types-integration` 层，随后更新 `pkg/types/internal/scalar/Cargo.toml` 的可选依赖/feature（如需要）及 `test_convert_to_string`。需特别说明字节到字符串的编码策略。
- 修改截断或 warning 语义时，必须通过 `Context::HandleTruncate`，并同时测试 strict、IgnoreTruncateErr、TruncateAsWarning 三类 Context；不要仅返回普通错误而丢失裁剪值。
- 扩展 JSON 类型时，在 int、float、decimal 三个 switch 中检查是否都需要新分支，并同步根 datum 子 crate 的同名实现，避免 feature 集成路径与默认根 crate 分叉。
- 修改 Duration/Datetime 选择规则时，同步 `StrToDuration`、`NumberToDuration`、`pkg/types/internal/scalar/migration_aster_unit_test.rs` 和 Go 基线测试，尤其保留 `(Duration, Time, isDuration)` 三者的一致性。
- 测试逻辑应继续放在独立 `pkg/types/convert_test.rs` 或 scalar 的独立集成测试文件，不要嵌入 `convert.rs`。性能上重点关注科学计数法大指数补零、反复字符串分配及动态 `Any` 下转型；兼容性上重点关注舍入方式、unsigned 补码、warning 顺序和 UTF-8/原始字节差异。

## 验证依据

本说明依据以下本地事实完成，没有运行 Cargo：

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph 文件读取：`node --file pkg/types/convert.rs --offset 1 --limit 500` 与 `--offset 501 --limit 500`，覆盖源码 1–922 行并报告文件被 12 个文件使用。
- RustCodeGraph 精确查询：对 `StrToInt`、`getValidFloatPrefix`、`ToString`、`ConvertJSONToInt`、`StrToDuration`、`ConvertFloatToInt`、`floatStrToIntStr` 执行 `query --kind function --json`；再以 `callers/callees <symbol> --file pkg/types/convert.rs --json` 限定 Rust 定义。确认了表达式 CAST、Datum、测试上游，以及上述内部调用边；全库同名 Go/通用符号未作为 Rust 直接调用证据。
- 完整阅读：`pkg/types/convert.rs`；crate/模块边界 `pkg/types/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/internal/scalar/Cargo.toml`、`pkg/types/internal/scalar/lib.rs`。
- Go 对照：`pkg/types/convert.go` 全部 799 行；Go 测试符号与相关区段来自 `pkg/types/convert_test.go`。
- Rust 独立测试：`pkg/types/convert_test.rs`，重点核对 `test_convert_type`、`test_convert_to_string`、`test_str_to_num`、`test_round_int_str`、`test_get_valid_int`、`test_get_valid_float`、`test_convert_json_to_*`、`test_number_to_duration`、`test_str_to_duration`、`test_convert_scientific_notation`、`test_convert_decimal_str_to_uint`；集成 feature 的直接覆盖另见 `pkg/types/internal/scalar/migration_aster_unit_test.rs`。
- 结构验收使用任务指定命令，要求本文件存在且恰好命中 11 个固定二级标题。人工复核重点为：真实 `#[path]` 装配、feature 边界、值随错误返回、调用边、Go 差异及独立测试位置。

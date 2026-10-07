# `pkg/parser/test_driver/test_driver_helper.rs` 逻辑说明

## 文件定位

`pkg/parser/test_driver/test_driver_helper.rs` 是 `astersql-parser-test_driver` crate 的基础数值辅助模块。crate 入口 `pkg/parser/test_driver/lib.rs` 通过 `#[path = "test_driver_helper.rs"] mod test_driver_helper` 装入它，再以 `pub use test_driver_helper::*` 将本文件的符号提升为 crate 级 API。该 crate 的边界由 `pkg/parser/test_driver/Cargo.toml` 定义，包名为 `astersql-parser-test_driver`；本文件自身不直接使用 Cargo 外部依赖。

它不负责 SQL 主解析器的词法分析。虽然函数名 `isDigit` 与 `pkg/parser/lexer.rs` 中的局部辅助函数同名，本文件的真实调用方位于测试驱动 crate：`pkg/parser/test_driver/test_driver_mydecimal.rs` 使用字符和十进制幂辅助函数，`pkg/parser/test_driver/test_driver_datum.rs::DefaultTypeForValue` 使用整数位数辅助函数。`lib.rs` 将测试驱动进一步提供给依赖该 crate 的解析器及其他移植代码。

## 核心职责

本文件提供两组无状态、纯内存辅助逻辑：

- `isSpace`、`isDigit`、`myMin`、`pow10` 支撑 `MyDecimal` 的十进制文本扫描、定长 word 拆装和格式化。它们刻意保持 `pkg/parser/test_driver/test_driver_helper.go` 的窄语义，而不是替换为更宽泛的 Unicode 或通用数值工具。
- `Abs`、`uintSizeTable`、`StrLenOfUint64Fast`、`StrLenOfInt64Fast` 计算整数的十进制显示宽度。`pkg/parser/test_driver/test_driver_datum.rs::DefaultTypeForValue` 用其为 `i32`、`i64`、`u64` 字面值设置 `FieldType.flen`。

这些函数存在的原因是让 Rust 测试驱动复现 Go `test_driver` 的字面值类型推断和精简 `MyDecimal` 行为，而不依赖完整 TiDB/AsterSQL 运行时。

## 主要符号

- `pub fn isSpace(value: u8) -> bool`：仅接受 ASCII 空格 `0x20` 和水平制表符 `0x09`。换行、回车和其他 Unicode 空白均返回 `false`。调用点见 `test_driver_mydecimal.rs::MyDecimal::FromString`。
- `pub fn isDigit(value: u8) -> bool`：通过 `u8::is_ascii_digit` 判断 `b'0'..=b'9'`。调用点见 `MyDecimal::FromString` 对整数段和小数段的扫描。
- `pub fn myMin(a: i32, b: i32) -> i32`：返回两个 `i32` 的较小值。调用点见 `MyDecimal::ToString`，用于把当前剩余位数限制在单个十进制 word 的 `digitsPerWord` 范围内。
- `pub fn pow10(exponent: i32) -> i32`：计算 `10_f64.powi(exponent)` 后转换为 `i32`。调用点包括 `countLeadingZeroes`、`MyDecimal::FromString` 的整数位权和小数尾部补零。
- `pub fn Abs(value: i64) -> i64`：使用符号扩展、异或和 `wrapping_sub` 实现补码绝对值；`i64::MIN` 保留原位模式并仍为负数，以匹配 Go 两补码溢出结果。
- `pub static uintSizeTable: [u64; 21]`：下标 1 至 20 分别保存对应十进制位数的最大 `u64`；下标 0 是为直接返回下标而保留的哨兵，末项是 `u64::MAX`。
- `pub fn StrLenOfUint64Fast(value: u64) -> i32`：从阈值表下标 1 开始线性查找首个大于等于输入的阈值，并返回下标作为十进制位数。
- `pub fn StrLenOfInt64Fast(value: i64) -> i32`：负数先计一个负号字符，再把 `Abs(value)` 的位模式转为 `u64` 并交给 `StrLenOfUint64Fast`。因此 `i64::MIN` 得到 19 位数字加 1 位负号，共 20。

本文件没有类型、trait、`impl` 或条件编译项；上述 7 个函数和 1 个静态表全部为公开符号，并由 crate 根重新导出。

## 执行流程

十进制解析路径从 `MyDecimal::FromString` 开始：

1. `isSpace` 跳过输入开头的空格和制表符。
2. 处理可选正负号后，`isDigit` 分别扫描整数段和小数段。
3. 输入按每个 word 9 位拆分；整数段从低位向高位读取时，以 `pow10(inner_index)` 计算每位权重。
4. 不足 9 位的小数尾 word 使用 `pow10(digitsPerWord - inner_index)` 右侧补零。
5. 反向格式化时，`myMin(剩余位数, digitsPerWord)` 限制每轮从一个 word 取出的字符数。

整数默认类型路径从 `test_driver_datum.rs::DefaultTypeForValue` 开始：

1. `i32` 和 `i64` 分支调用 `StrLenOfInt64Fast`；`u64` 分支调用 `StrLenOfUint64Fast`。
2. 有符号版本为负号预留一个字符，并通过 `Abs` 取得用于计数的补码幅值。
3. 无符号版本按 `uintSizeTable` 从 1 位阈值逐项比较，返回首个命中的下标。
4. 返回值转换为 `isize`，写入 `FieldType::SetFlen`。

## 数据与状态

本文件不保存可变运行时状态。`isSpace`、`isDigit`、`myMin`、`pow10`、`Abs` 和两个长度函数只读取值参数并立即返回结果。

唯一的模块级数据 `uintSizeTable` 是固定长度的公开静态数组。算法依赖三个结构不变量：数组长度为 21；下标 1 至 20 的阈值单调递增；最后一个元素覆盖全部 `u64` 输入。下标 0 的值不会参与比较。破坏这些不变量可能导致错误位数，或者落入 `StrLenOfUint64Fast` 尾部的 `unreachable!`。

在调用侧，`MyDecimal` 的 `wordBuf`、位数和符号状态属于 `test_driver_mydecimal.rs::MyDecimal`，`FieldType` 的状态属于 `DefaultTypeForValue` 的可变参数；它们都不是本文件拥有的状态。

## 依赖与调用关系

crate 装配关系是 `pkg/parser/test_driver/lib.rs` → `test_driver_helper.rs`，随后 `pub use test_driver_helper::*` 使调用方可以通过 crate 根引用符号。`Cargo.toml` 声明的 `parser_charset`、`parser_format`、`parser_mysql`、`parser_types` 和 `hex` 服务整个 test_driver crate，但本文件仅使用 Rust 标准库的原生整数/浮点方法及运算符。

主要上游调用边经源码引用核对如下：

- `test_driver_mydecimal.rs::{countLeadingZeroes, MyDecimal::ToString, MyDecimal::FromString}` → `pow10`、`myMin`、`isSpace`、`isDigit`。
- `test_driver_datum.rs::DefaultTypeForValue` → `StrLenOfInt64Fast`、`StrLenOfUint64Fast`。
- `migration_aster_unit_test.rs::helper_lengths_match_go_boundaries` → 两个 `StrLen*` 函数，覆盖边界回归。
- `StrLenOfInt64Fast` → `Abs` → 补码运算，并调用 `StrLenOfUint64Fast` → `uintSizeTable`。

RustCodeGraph 的文件节点将本文件识别为 8 个符号；其全局名称搜索也显示仓库另有 `pkg/util/mathutil/math.rs` 的同名 `Abs`/`StrLen*` 实现。文档中的调用关系以本 crate 的 `crate::{...}` 导入和同目录源码引用为准，不能把 `pkg/types/field_type.rs` 调用 `mathutil::*` 的边误认为本文件调用边。

## 错误处理与边界

这些 API 不返回 `Result`，正常输入没有可恢复错误通道。关键边界如下：

- `isSpace` 是窄空白定义；若调用方期望跳过换行或全体 Unicode 空白，必须在上层明确处理，不能静默扩大本函数语义。
- `isDigit` 只接受 ASCII 数字，符合十进制字面量按字节扫描的调用方式。
- `pow10` 先在 `f64` 中计算再转换为 `i32`。Rust 的浮点到整数转换在超出范围时会饱和，在 `NaN` 时为 0；负指数会产生小于 1 的值并转换为 0。当前已核对调用点使用十进制 word 位宽相关的小范围非负指数，文档没有据此宣称任意指数都适用。
- `Abs(i64::MIN) == i64::MIN` 是有意保留的 Go 补码边界，不代表数学意义上的非负绝对值。随后转换为 `u64` 得到 `2^63`，使带符号长度函数仍正确返回 20。
- `StrLenOfUint64Fast(0) == 1`，因为 0 命中下标 1 的阈值 9；`u64::MAX` 命中下标 20。末尾 `unreachable!` 只在阈值表覆盖性被破坏时触发。

`migration_aster_unit_test.rs::helper_lengths_match_go_boundaries` 明确覆盖 0、9、10、`u64::MAX`、-1 和 `i64::MIN`。字符判断、`myMin` 与 `pow10` 没有同名独立测试；它们通过 `decimal_round_trips_go_supported_forms` 及 `MyDecimal::FromString`/`ToString` 路径获得间接覆盖。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、任务、线程、异步状态、文件句柄或网络资源。函数只操作按值复制的标量；`uintSizeTable` 是只读静态数据，因此多个线程并发读取不会产生竞争，也不需要初始化或清理协议。

生命周期仅体现为同步调用栈：辅助函数在调用期间计算并返回标量，不借用或持有调用方缓冲区。`MyDecimal` 和 `FieldType` 的可变更新都发生在上游模块，辅助函数不会保留对它们的引用。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/parser/test_driver/test_driver_helper.go`。Rust 逐项保留了 Go 的功能分组和命名：

- Go `isSpace` 的两个显式字节比较对应 Rust 的两个字节比较；Rust 没有改用 Unicode 空白判断。
- Go `isDigit` 的范围比较对应 Rust `u8::is_ascii_digit`，接受集合相同。
- Go `myMin(int, int) int` 对应 Rust `myMin(i32, i32) -> i32`；类型收窄与 Rust `MyDecimal` 的 `i32` 位数计算一致。
- Go `int32(math.Pow10(x))` 对应 Rust `10_f64.powi(exponent) as i32`。二者都经过浮点幂再转 32 位整数，但极端越界转换的语言细节不应外推为完全等价；当前真实调用域是小范围非负指数。
- Go `Abs` 的 `(n ^ y) - y` 对应 Rust 的异或加 `wrapping_sub`。显式 wrapping 避免 Rust 调试构建在 `i64::MIN` 上溢出 panic，并保留 Go 的位模式。
- 两边的 `uintSizeTable` 都有 21 项、冗余下标 0 和最大值末项。Go 使用无界 `for i := 1; ; i++`；Rust 使用数组长度限制并在理论不可达位置显式失败，正常输入结果一致。
- Go 长度函数返回平台 `int`，Rust 返回 `i32`；结果域仅 1 至 20，调用侧再转 `isize`，不会丢失有效结果。

相关 Go 使用点是 `test_driver_mydecimal.go` 和 `test_driver_datum.go`；Rust 同路径文件保持相同职责。仓库另有生产工具 `pkg/util/mathutil` 的近似实现和测试，但它不是本 test_driver 文件的真实实现位置。

## 扩展指南

扩展前先按职责选择接入点：

- 调整十进制文本可接受字符时，修改 `isSpace`/`isDigit` 的同时必须检查 `test_driver_mydecimal.rs::MyDecimal::FromString`，并对照 `test_driver_mydecimal.go`。建议在独立的 `migration_aster_unit_test.rs` 增加前导空白、非 ASCII、分隔符和无数字输入用例，不要把测试内嵌进本生产文件。
- 调整 word 位权或格式化位数时，检查 `pow10`、`myMin` 与 `digitsPerWord` 的组合，并覆盖 word 边界（8/9/10 位）、小数尾部补零、超长输入及科学计数法拒绝路径。浮点转整数是兼容风险，应避免在未核对 Go 行为时扩大指数域。
- 调整整数 `flen` 时，修改 `uintSizeTable`/`StrLen*` 后同步检查 `test_driver_datum.rs::DefaultTypeForValue` 和 `migration_aster_unit_test.rs::helper_lengths_match_go_boundaries`。至少覆盖每个 10 的幂前后、0、`i64::MIN`、`i64::MAX` 和 `u64::MAX`。
- 若新增公开辅助符号，应确认是否确实需要经 `lib.rs` 的通配再导出暴露；本 crate 当前没有 feature 门控。避免与 `pkg/util/mathutil` 的同名实现混淆或制造行为漂移。

兼容性风险主要是偏离 Go 的字节级解析、负最小值语义和显示宽度；性能风险较低，但 `StrLenOfUint64Fast` 当前最多比较 20 次，替换算法前应有可证明收益并保持所有边界。该模块是测试驱动基础层，错误结果会间接改变 AST 字面量类型元数据或十进制解析结果，影响面可能大于文件体量所示。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/parser/test_driver` 确认同 crate 文件集合；`node --file pkg/parser/test_driver/test_driver_helper.rs` 核对 94 行源码和 8 个符号；`query` 核对 `StrLenOfUint64Fast`、`StrLenOfInt64Fast`、`isSpace`、`pow10` 的同名实现位置。通用名称的全局 `explore/callers/callees` 结果存在跨包歧义，故调用边另以 crate 导入和源码引用消歧。
- 源码与装配：`pkg/parser/test_driver/lib.rs`、`test_driver_mydecimal.rs`、`test_driver_datum.rs`。
- crate 边界：`pkg/parser/test_driver/Cargo.toml`。
- Go 对照：`pkg/parser/test_driver/test_driver_helper.go`、`test_driver_mydecimal.go`、`test_driver_datum.go`。
- 独立测试：`pkg/parser/test_driver/migration_aster_unit_test.rs::helper_lengths_match_go_boundaries` 和 `decimal_round_trips_go_supported_forms`；`test_driver_test.rs` 不覆盖本文件辅助逻辑。
- 引用核对：对 `pkg/parser`、`pkg/types` 和 `pkg/util/mathutil` 执行符号搜索，区分 test_driver 的真实调用点与其他包的同名实现。

人工复核结论：该文件因 Go parser test_driver 移植而存在；运行时仅执行同步字符判断和整数计算；安全扩展必须同步 Go 对照、两个直接调用模块及独立迁移测试。未运行 Cargo 或代码测试，因为任务限定为纯文档分析；结构验收使用任务文件指定的 11 章节命令。

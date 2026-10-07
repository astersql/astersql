# `pkg/parser/test_driver/test_driver_mydecimal.rs`

## 文件定位

本文件是 `astersql-parser-test_driver` crate 内的轻量十进制定点数实现。crate 入口 `pkg/parser/test_driver/lib.rs` 通过 `#[path = "test_driver_mydecimal.rs"] mod test_driver_mydecimal` 装入模块，再以 `pub use test_driver_mydecimal::*` 导出其公开符号；因此它服务于 parser 测试驱动的字面值构造、类型推断和 SQL 文本还原，而不是 `pkg/types/mydecimal.rs` 中完整的运行时 DECIMAL 实现。

`pkg/parser/test_driver/Cargo.toml` 将该 crate 的库入口指定为 `lib.rs`，依赖 parser 的 charset、format、mysql、types 子 crate 和 `hex`。本文件自身只调用 crate 根再导出的 `isDigit`、`isSpace`、`myMin`、`pow10`，没有直接使用这些外部依赖。

## 核心职责

- 用固定的 9 个 `i32` word 表示最多 81 位十进制数字；每个 word 保存 9 位，范围约束为 `0 <= word < 10^9`（`MyDecimal::wordBuf`、`maxWordBufLen`、`digitsPerWord`）。
- 将普通十进制字节串解析为符号、整数/小数位数和 word 数组（`MyDecimal::FromString`）。
- 将内部 word 表示无舍入地格式化为 ASCII 十进制文本，并移除整数部分的前导零（`MyDecimal::ToString`、`removeLeadingZeros`、`String`）。
- 对精简测试驱动没有实现的能力显式 `panic`，尤其是超出 9 个 word、空/无数字输入和科学计数法；它不是完整 MySQL DECIMAL 运算库（`panicInfo`、`fixWordCntError`、`FromString`）。

## 主要符号

- `panicInfo: &str`：所有“TiDB 专属分支未实现”panic 的统一说明。
- `maxWordBufLen = 9`、`wordBufLen = 9`、`digitsPerWord = 9`、`digMask = 10^8`：定义容量、分组宽度和逐位格式化掩码。`maxWordBufLen` 决定数组类型，`wordBufLen` 用于运行时容量检查及零值扫描。
- `fixWordCntError(words_int, words_frac)`：校验整数 word 与小数 word 总数不超过 9。签名返回 `Result`，但当前实现成功时只返回原计数，超限直接 panic，不产生 `Err`。
- `countLeadingZeroes(index, word)`：从给定十进制位宽向下比较 `pow10`，计算首个有效整数 word 可裁掉的前导零数。
- `digitsToWords(digits)`：以 9 为单位向上取整，是解析与格式化共享的位数到 word 数映射。
- `MyDecimal`：公开字段依次为 `digitsInt`、`digitsFrac`、`resultFrac`、`negative`、`wordBuf`；派生 `Clone` 和 `Default`。这些字段共同描述定点数的显示精度、符号和分组数据。
- `String(&self)`：克隆值后调用可变接收者 `ToString`，再把保证为 ASCII 的字节转换为 Rust `String`。
- `stringSize(&self)`：按整数位、小数位再加符号/零/小数点预留量估算临时缓冲区。
- `removeLeadingZeros(&self)`：跳过全零整数 word，并修正首个非零 word 的有效整数位数。
- `ToString(&mut self)`：输出无舍入的十进制字节串。虽然接收 `&mut self` 以对齐 Go 方法形状，但当前函数只读字段。
- `FromString(&mut self, input)`：解析输入并写入当前实例，成功后令 `resultFrac = digitsFrac`。

所有上述常量、函数、类型和字段都通过 `lib.rs` 的通配再导出成为 crate 公共接口；文件内没有 trait、条件编译项或异步入口。

## 执行流程

解析路径 `MyDecimal::FromString`：

1. 跳过开头由 `isSpace` 判定的空白，随后消费一个可选的 `-` 或 `+`；只有负号会设置 `negative`。
2. 扫描连续整数数字；若遇到 `.`，再扫描连续小数数字。整数和小数合计为零时 panic。
3. `digitsToWords` 分别计算整数、小数所需 word，`fixWordCntError` 检查总数不超过固定容量；然后保存 `digitsInt` 和 `digitsFrac`。
4. 整数部分从右向左读取，以 `pow10(inner_index)` 累加每组最多 9 位，并从整数区末端向前写入 `wordBuf`，因此数组中的整数 word 保持高位组在前。
5. 小数部分从左向右读取；完整的 9 位组直接写入，不足 9 位的末组乘以相应的 10 次幂在右侧补零，使最高小数位仍处在 word 高位。
6. 若数字后紧跟 `e` 或 `E`，以 `panicInfo` 拒绝科学计数法；其他尾随字节当前不会报错。所有 9 个 word 都为零时清除负号，规范化 `-0`；最后同步 `resultFrac` 并返回 `Ok(())`。

格式化路径 `String -> ToString`：

1. `String` 克隆对象，`ToString` 按 `stringSize` 分配缓冲区，并经 `removeLeadingZeros` 得到首个有效整数 word 和实际整数位数。
2. 零值强制保留一个整数位；根据符号、小数点及有效位数截断缓冲区到精确输出长度。
3. 小数区从整数 word 之后开始，逐 word 用 `digMask` 从高位到低位输出，末组只输出 `digitsFrac` 声明的位数。
4. 整数区从低位 word 反向取模并倒序写入预留位置；没有有效整数位时写入小数点前的 `0`。

## 数据与状态

`wordBuf` 是值的主体，整数区占前 `digitsToWords(digitsInt)` 个 word，小数区紧随其后。整数首组允许不足 9 位；小数末组不足 9 位时在内部右补零。`digitsInt`/`digitsFrac` 保存输入声明的位数，格式化时整数前导零会被隐藏，但小数尾零按 `digitsFrac` 保留，例如回归测试中的 `00123.4500 -> 123.4500`。

`resultFrac` 在本精简实现中仅由 `FromString` 设为 `digitsFrac`，`ToString` 不读取它，也不执行注释所暗示的舍入。`negative` 在解析 `-` 时置位，并在全部 word 为零时清除。

调用者通常应从 `MyDecimal::default()` 开始解析。`FromString` 不先清空 `wordBuf`，也不会在正数或 `+` 输入时主动清除一个既有对象的 `negative`；复用已含数据的实例解析更短文本可能留下未覆盖 word，并影响“是否为零”的全缓冲扫描。这是当前源码边界，不应把该 API 描述为可无条件复用的通用解析器。

## 依赖与调用关系

下游依赖均来自 crate 根：`isSpace`/`isDigit` 负责词法判定，`pow10` 用于分组构造与补零，`myMin` 限制每次格式化的位数。内部调用边为：`FromString -> digitsToWords, fixWordCntError`，`fixWordCntError -> wordBufLen`；`String -> ToString`；`ToString -> stringSize, removeLeadingZeros, digitsToWords, myMin`；`removeLeadingZeros -> countLeadingZeroes -> pow10`。

直接上游位于同一 crate：

- `pkg/parser/test_driver/test_driver_datum.rs` 的 `Datum::GetMysqlDecimal`/`SetMysqlDecimal` 存取该类型，`DefaultTypeForValue` 用 `String().len()` 和 `digitsFrac` 填充 `TypeNewDecimal` 的显示宽度与小数位。
- `pkg/parser/test_driver/test_driver.rs` 的 `ValueExpr::Restore` 和 `ValueExpr::Format` 在 `KindMysqlDecimal` 分支调用 `String()`，把测试 AST 中的 DECIMAL 字面值还原为 SQL/展示文本。
- `pkg/parser/test_driver/migration_aster_unit_test.rs::decimal_round_trips_go_supported_forms` 直接覆盖解析到格式化的往返链。

RustCodeGraph 对常见名 `FromString` 报出的 `pkg/planner/core/expression_rewriter.rs`、`logical_plan_builder_runtime.rs` 和 `pkg/session/runtime/relational_value.rs` 调用，源码核验后分别指向 `types::MyDecimal`、`expression::types::MyDecimal` 和 `astersql_types::decimal::mydecimal::MyDecimal`，不是本测试驱动类型，不能作为本文件的应用主链证据。

## 错误处理与边界

- 空输入、只有空白、只有符号、只有小数点或其他不含数字的开头会进入 `panicInfo` 分支；API 不返回可恢复错误。
- 整数与小数组合超过 9 个 word（最多 81 位）时，`fixWordCntError` panic。其 `Result<_, String>` 目前没有实际 `Err` 路径。
- 紧随数字的 `e`/`E` 会 panic，明确表示科学计数法未实现；其他尾随内容被忽略，而不是严格拒绝。
- 输出依赖字段不变量：word 必须小于 `10^9`，位数必须与缓冲区布局匹配。字段全部公开，外部若构造不一致状态，索引或字符换算可能 panic/产生非数字输出；当前没有独立校验器。
- `String::from_utf8(...).expect(...)` 依赖 `ToString` 只生成 ASCII 的内部不变量；若该不变量被破坏会 panic。
- `countLeadingZeroes` 假定调用时位宽和 word 合法；它不是面向任意负数或任意 index 的防御式 API。

## 并发与资源生命周期

本文件不创建线程、锁、通道、任务、事务或外部资源。`MyDecimal` 是拥有自身定长数组的普通值；`Clone` 做完整值复制，`String` 在副本上格式化，因此不会修改原对象。解析需要 `&mut self`，Rust 借用规则阻止同一实例被并发写入；不同实例之间无共享可变状态。

`wordBufLen` 是不可变 `static usize`，不存在运行时更新或同步。格式化唯一的动态资源是局部 `Vec<u8>` 与最终 `String`，函数返回后由 Rust 所有权自动管理。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/parser/test_driver/test_driver_mydecimal.go`。Rust 保留了 Go 版的常量、字段顺序、9 位分组布局，以及 `fixWordCntError`、`countLeadingZeroes`、`digitsToWords`、`String`、`stringSize`、`removeLeadingZeros`、`ToString`、`FromString` 的控制流。符号、前导零、`.125`、小数尾零、9 位 word 边界和负零规范化语义保持一致。

语言层面的主要差异是：Go 的字段和辅助函数为包内可见，Rust 版本当前均为 `pub`；Go `String` 复制结构体值，Rust 显式 `clone`；Go 字节切片与 Rust `Vec<u8>`/`&[u8]` 对应；Go 的 `error` 与 Rust 的 `Result<(), String>` 在当前实现中都没有普通错误返回，未支持分支仍 panic。

两边都只实现测试驱动需要的解析/格式化子集。不要据此推断它具有 `pkg/types/mydecimal.go` 或 `pkg/types/mydecimal.rs` 的算术、舍入、溢出/截断错误体系。Go 文件的 `//go:build !codes` 构建约束在 Rust 文件中没有对应 feature；Rust crate 始终由其 Cargo 目标决定是否编译。

## 扩展指南

- 增加可接受输入形式时，修改入口应是 `MyDecimal::FromString`，并同步审查 `digitsToWords`/`fixWordCntError` 的容量语义。若支持指数或严格尾随校验，应明确选择返回 `Err` 还是继续沿用 panic，并与 Go 对照文件保持一致。
- 增加舍入或 `resultFrac` 行为时，修改 `String`/`ToString` 前先确定 Go 测试驱动的真实契约；当前 `ToString` 明确无舍入，不能仅凭 Go `String` 注释推导行为。
- 若允许复用对象解析，应在 `FromString` 开头有意重置 `negative`、位数元数据和 `wordBuf`，并增加“长值后解析短值”“负值后解析正值”“非零后解析零”的独立回归用例。
- 调整容量或 word 宽度时，必须联动 `maxWordBufLen`、`wordBufLen`、`digitsPerWord`、`digMask`、`pow10` 的范围以及 `i32` 安全性，并验证整数首组和小数末组边界。
- 测试逻辑应继续放在独立文件 `pkg/parser/test_driver/migration_aster_unit_test.rs`（或新增同目录独立 `*_test.rs`），不要嵌入生产源文件。至少覆盖普通往返、9/10 位跨 word、81 位容量边界、超限、无数字、指数、负零、前导/尾随字符和对象复用。
- 修改公开字段或导出面时，同时检查 `pkg/parser/test_driver/lib.rs`、`test_driver_datum.rs` 和 `test_driver.rs` 的消费点，以及 `pkg/parser/test_driver/test_driver_mydecimal.go` 的对齐要求。

## 验证依据

- 目标实现：`pkg/parser/test_driver/test_driver_mydecimal.rs`，核对了全部 304 行、所有常量/函数/字段和 `impl MyDecimal` 控制流。
- crate 边界：`pkg/parser/test_driver/Cargo.toml` 与 `pkg/parser/test_driver/lib.rs`，确认库入口、依赖和公开再导出。
- Rust 直接调用与测试：`pkg/parser/test_driver/test_driver_datum.rs`、`pkg/parser/test_driver/test_driver.rs`、`pkg/parser/test_driver/migration_aster_unit_test.rs`；现有往返用例覆盖 `0`、`-0`、正号、无整数位小数、整数前导零/小数尾零和跨多个 word 的值。`pkg/parser/test_driver/test_driver_test.rs` 不含 MyDecimal 用例。
- Go 对照：`pkg/parser/test_driver/test_driver_mydecimal.go`，逐段核对常量、内存布局、解析、格式化、未实现分支和负零处理；同目录未发现 MyDecimal 专用 Go 测试。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`query MyDecimal --kind struct` 精确定位本文件第 65 行类型，`query FromString`/`query ToString` 定位第 200/110 行方法，`node test_driver_mydecimal.rs::FromString` 给出内部 `digitsToWords`、`fixWordCntError` 及三个同名跨模块候选；随后读取候选源码排除了类型误连。`explore` 还确认本文件辅助调用边和 `test_driver_datum.rs` 的格式化消费点。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证恰有 11 个固定二级章节，并人工复核无整段源码复制、无无依据的“已支持”结论。

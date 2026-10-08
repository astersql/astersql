# `pkg/types/binary_literal.rs`

## 文件定位

该文件实现 MySQL `BIT` 与十六进制字面量的共享字节表示、解析、格式化、数值转换和比较，是类型系统中“SQL 字面量文本/整数”与“按大端保存的字节序列”之间的标量转换层。源码并不是由根 crate 直接声明：`pkg/types/internal/scalar/lib.rs` 通过 `#[path = "../../binary_literal.rs"] mod binary_literal` 挂载并 `pub use` 全部公开符号，因此实际编译归属是 `astersql-types-scalar`；根 `pkg/types/lib.rs` 再把该 crate 暴露为 `astersql_types::scalar`，并选择性再导出 `Context` 等上下文符号。

Cargo 边界由 `pkg/types/internal/scalar/Cargo.toml` 定义；本文件直接使用标准库、该 crate 的 `Context`/`ValueResult`/`errors`，以及 `hex = "0.4"`。`pkg/types/Cargo.toml` 则说明上层 `astersql-types` 依赖并命名该子 crate 为 `types-group-1`。文件没有条件编译项；测试分别从根 crate 的 `pkg/types/binary_literal_test.rs` 和标量子 crate 的 `pkg/types/binary_literal_1_aster_unit_test.rs` 挂载，测试逻辑未内嵌在生产文件中。

## 核心职责

- 用 `BinaryLiteral(Vec<u8>)` 保存不带文本前缀的原始字节，用 `BitLiteral(BinaryLiteral)` 与 `HexLiteral(BinaryLiteral)` 保留解析结果的 SQL 字面量类别。
- 把 `u64` 编码为大端字节，支持固定 1..=8 字节宽度或 `-1` 自动裁掉前导零；这是 BIT 列物理字节生成的公共入口（`NewBinaryLiteralFromUint`）。
- 接受 MySQL 形式的 `b'...'`、`B'...'`、`0b...`、`x'...'`、`X'...'` 与 `0x...`，验证语法后产出字节（`ParseBitStr`、`ParseHexStr`）。
- 提供十六进制显示、位串显示、字符串解释、大端 `u64` 转换，以及忽略前导零的数值比较。
- 在超过 `u64` 表示范围时把截断交给语句级 `Context::HandleTruncate`，由上下文决定忽略、记 warning，还是返回带裁剪值的错误。

## 主要符号

- `pub struct BinaryLiteral(pub Vec<u8>)`：公开 tuple struct，允许调用者直接构造或取出字节；实现 `AsRef<[u8]>` 和 `Deref<Target = [u8]>`，便于交给编码器或字节 API。派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`；这里的派生相等比较原始字节，与 `Compare` 的数值比较不是同一语义。
- `pub struct BitLiteral(pub BinaryLiteral)`、`pub struct HexLiteral(pub BinaryLiteral)`：类型标签包装；各自的 `ToString` 仅委托内部 `BinaryLiteral::ToString`。
- `ZeroBinaryLiteral() -> BinaryLiteral`：每次返回空 `Vec<u8>`，对应空字面量的零值；它不同于单字节零 `[0]`。
- `trimLeadingZeroBytes(&[u8]) -> &[u8]`：内部数值规范化辅助。空输入仍为空；非空且全零时保留最后一个零字节，所以 `[0,0]` 规范化为 `[0]`。
- `NewBinaryLiteralFromUint(u64, isize) -> BinaryLiteral`：`byteSize == -1` 时对完整 8 字节大端表示去前导零；固定宽度时截取低位的 `byteSize` 个字节。除 `-1` 或 `1..=8` 外直接 panic。
- `BinaryLiteral::String() -> String`：非空值输出 `0x` 加小写十六进制，空值输出空字符串。
- `BinaryLiteral::ToString() -> String`：用 `String::from_utf8_lossy` 解释原始字节；有效 UTF-8 原样返回，非法序列会被替换字符代替。
- `BinaryLiteral::ToBitLiteralString(bool) -> String`：逐字节生成 8 位二进制；请求裁前导零时，全零仍保留一位 `0`；空输入固定输出 `b''`。
- `BinaryLiteral::ToInt(Context) -> ValueResult<u64>`：先忽略前导零，再按大端折叠；有效载荷超过 8 字节时以 `u64::MAX` 为裁剪值调用截断策略。
- `BinaryLiteral::Compare(BinaryLiteral) -> i32`：双方先去前导零，先比较有效长度，再比较字典序，返回 `-1/0/1`。参数按值取得，调用会消费右值。
- 私有 trait `OrderingSignum`：把标准库 `Ordering` 映射为 Go 风格比较结果，只服务于 `Compare`。
- `ParseBitStr`/`NewBitLiteral` 与 `ParseHexStr`/`NewHexLiteral`：前者返回共享表示，后者在成功结果外包一层类别类型；错误类型为 `errors::SharedError`。

## 执行流程

BIT 解析从 `ParseBitStr` 开始。它先拒绝空输入；首字节为 `b`/`B` 时移除该字母并用 `trim_matches('\'')` 去掉两端所有单引号，前缀为小写 `0b` 时移除两字节，否则报格式错误。净载荷为空时返回 `ZeroBinaryLiteral()`；非空时要求所有字节只能是 ASCII `0`/`1`。随后把位数向上对齐到 8 的倍数，在左侧补零，按 8 位分块用二进制解析为 `u8`，保持高位字节在前。`NewBitLiteral` 只在成功后执行 `map(BitLiteral)`。

HEX 解析由 `ParseHexStr` 完成。空输入先报错；`x`/`X` 形式移除首字母并裁掉两端单引号，而且净载荷必须为偶数个字符；`0x` 形式只接受小写前缀，并允许奇数个十六进制字符。空载荷返回空字节；`0x` 奇数载荷先在左侧补 `0`，再由 `hex::decode` 校验并解码。`NewHexLiteral` 在成功后包装为 `HexLiteral`。

整数路径有两个方向。`NewBinaryLiteralFromUint` 先生成固定 8 字节大端数组，再按模式裁切；例如 `0x123, 1` 得到 `[0x23]`，固定宽度小于有效位数时会静默保留低位。反向的 `ToInt` 先去除前导零，空值返回 0，至多 8 个有效字节用“左移 8 位再或入下一字节”累积；超过 8 个有效字节则不继续解析，而把 `u64::MAX` 和包含 `String()` 表示的截断错误一起交给上下文。

显示和比较不改动对象：`String` 是十六进制诊断表示，`ToBitLiteralString` 是 SQL 位串表示，`ToString` 是字符解释；`Compare` 把字节视作无符号大端数值，而派生的 `PartialEq` 仍按完整字节向量比较。

## 数据与状态

核心状态只有拥有所有权的 `Vec<u8>`。字节顺序不单独存储，而由所有整数转换路径共同约定为大端：索引越靠前，数值权重越高。前导零会被解析保留（例如九位零产生两个零字节），只有数值构造的 `-1` 模式、`ToInt` 和 `Compare` 主动规范化；因此“原始表示”与“数值等价”必须区分。

空向量、单字节零和多个零字节是三种不同的原始状态：它们的 `String`/完整位串输出不同，但 `ToInt` 都为 0，`Compare` 也把非空全零规范化为单字节零。`ZeroBinaryLiteral()` 返回空向量，而 `NewBinaryLiteralFromUint(0, -1)` 因 `trimLeadingZeroBytes` 的全零规则返回 `[0]`。

`Context` 按值传给 `ToInt`，内部持有可克隆的标志、时区与 `Arc` warning handler（`pkg/types/context.rs`）。本文件不保存 Context，也没有全局可变状态。错误结果 `ValueResult<u64>` 可以携带 `u64::MAX` 这一裁剪值，调用者不能只看错误文本而忽略伴随值语义。

## 依赖与调用关系

下游依赖很窄：`hex::encode/decode` 负责十六进制转换；`crate::errors::New` 构造共享错误；`Context::HandleTruncate` 实施截断策略；其余是标准库的字节、字符串和排序操作。内部调用边为：`NewBinaryLiteralFromUint -> trimLeadingZeroBytes`，`ToInt -> trimLeadingZeroBytes/String/Context::HandleTruncate`，`Compare -> trimLeadingZeroBytes/OrderingSignum`，`NewBitLiteral -> ParseBitStr -> ZeroBinaryLiteral`，`NewHexLiteral -> ParseHexStr -> ZeroBinaryLiteral`，包装类型的 `ToString -> BinaryLiteral::ToString`。

上游入口包括：

- `pkg/types/parser_driver/value_expr.rs` 的 `new_bit_literal`/`new_hex_literal` 调用 `NewBitLiteral`/`NewHexLiteral`，并把它们放入解析器 `DriverHooks`；这是 SQL 解析阶段从字面量文本进入本文件的直接路径。
- `pkg/expression/chunk_executor.rs` 在标量和向量整数结果写入 MySQL `BIT` 列时，按字段位宽计算字节数并调用 `NewBinaryLiteralFromUint`。
- `pkg/types/datum.rs::convertToMysqlBit` 把字符串/字节 Datum 先经 `BinaryLiteral::ToInt` 转成无符号值，做 BIT 位宽裁剪后再用 `NewBinaryLiteralFromUint` 写回；这里把本文件接入类型转换和错误策略。
- `pkg/tablecodec/tablecodec.rs`、`pkg/util/rowcodec/decoder.rs` 与 `pkg/util/codec/codec.rs` 在 BIT 编解码路径构造或读取相同的大端字节表示；表达式计算还在 `pkg/expression/builtin.rs` 用 `BinaryLiteral(raw).ToInt(...)` 取得数值。

RustCodeGraph 对目标文件报告被 28 个文件使用，并能定位 `BinaryLiteral`、`ParseBitStr`、`ParseHexStr`、`NewBinaryLiteralFromUint` 等节点；本次对这些精确节点执行 `callers`/`callees` 未返回图边，因此上述跨文件关系进一步以实际导入和调用行核验，没有用同名 Go 或 parser test-driver 符号冒充本实现的调用边。

## 错误处理与边界

- `NewBinaryLiteralFromUint` 用断言约束宽度：仅允许 `-1` 或 `1..=8`，非法值是 panic，不是可恢复错误；固定宽度会截掉高位，不报告溢出。
- BIT 只接受大写/小写 `b'...'` 或小写 `0b...`；`0B` 被拒绝。净载荷必须为 ASCII 二进制位，Unicode 或其他数字都会报错。实现使用 `trim_matches`，不是严格检查恰好一对引号，因此扩展语法时应先决定是否保持这一兼容行为。
- HEX 只接受 `x`/`X` 引号形式或小写 `0x`；`0X` 被拒绝。引号形式要求偶数位，`0x` 形式允许奇数位并左补零；非法字符由 `hex::decode` 转成共享错误。
- 两种解析器把 `b''`/`B''`、`x''` 解释为空字节，但 `0b''` 会因引号不是二进制位而失败；空的 Rust `String` 在检查前缀前就失败，不发生越界索引。
- `ToInt` 只以去前导零后的长度判定溢出，因此九字节但首字节为零的值仍可正常转换。真正超过 8 个有效字节时，返回值取决于 Context：忽略或 warning 模式可得到 `Ok(u64::MAX)`，严格模式得到携带同一值的 `ErrorWithValue`。
- `ToString` 与 Go 有可观察差异：Rust 使用 lossy UTF-8，非法字节会变成 U+FFFD；Go 的 `string([]byte)` 可以无损携带任意字节。对任意二进制数据需要无损处理时应使用 `as_ref()`/`.0`，不能依赖 `ToString` round-trip。

## 并发与资源生命周期

文件没有锁、通道、异步任务、线程局部变量、事务或 I/O。每个解析/格式化调用只分配自己的 `String`/`Vec<u8>`；返回值拥有数据，不借用输入字符串。`trimLeadingZeroBytes` 是唯一返回借用切片的函数，生命周期受输入切片约束，不分配也不修改输入。

所有值类型都可独立克隆；本文件没有显式 `Send`/`Sync` 实现，也没有内部共享可变状态。`ToInt` 消费一个 `Context` 克隆；warning 模式可能通过 Context 内的共享 handler 产生外部可见副作用，但同步策略属于 `WarnAppender` 实现而非本文件。解析长字面量的时间与输出空间均为 O(n)；`ToBitLiteralString` 预留约输入字节数八倍的字符串容量，`ParseBitStr` 还会构造一份补齐后的文本。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/binary_literal.go`，独立 Go 测试是 `pkg/types/binary_literal_test.go`。Rust 保留了 Go 的三种类型、公开函数名称、接受的前缀大小写、位/十六进制补零规则、大端整数语义、非法宽度 panic、超过 8 有效字节时裁剪到最大无符号整数、忽略前导零的比较，以及空字面量行为。`pkg/types/binary_literal_test.rs` 的表驱动案例基本逐项复刻 Go 测试；`pkg/types/binary_literal_1_aster_unit_test.rs` 另验证标量子 crate 中的集成路径和严格截断错误。

实现形态上的主要映射是：Go 的切片别名变为拥有 `Vec<u8>` 的 tuple struct；Go 的包级 `ZeroBinaryLiteral` 变量变为每次构造新空向量的函数；Go `bytes.Compare` 变为 Rust 切片比较加私有 `OrderingSignum`；Go 的 `(uint64, error)` 变为能在错误中保留裁剪值的 `ValueResult<u64>`。

需要特别保留的差异是 Rust `ToString` 的 lossy UTF-8 行为，Go 版本没有该替换；此外 Rust `Compare` 按值取得右操作数，而 Go 切片参数复制的是切片头。错误文本由不同错误基础设施构造：Go 使用 `ErrTruncatedWrongVal`/`errors.Trace`，Rust 当前创建共享字符串错误，因此不要假设两侧错误类型或栈信息完全一致。测试中名为 `test_parse_hex_str_empty_error` 的 Rust/Go 对照用例实际上都再次调用 BIT 解析器，不能据此证明 HEX 空输入分支；该分支的事实来自 `ParseHexStr` 源码。

## 扩展指南

- 新增或收紧字面量语法应修改 `ParseBitStr`/`ParseHexStr`，并同步扩展独立的 `pkg/types/binary_literal_test.rs` 与 Go 对照 `pkg/types/binary_literal_test.go`；若影响解析器接线，还要覆盖 `pkg/types/parser_driver/value_expr_test.rs`。不要把测试写回生产文件。
- 改变整数宽度、补零或溢出规则时，应同时审查 `NewBinaryLiteralFromUint`、`ToInt`、`trimLeadingZeroBytes`，以及 BIT 列调用者 `pkg/expression/chunk_executor.rs`、`pkg/types/datum.rs`、`pkg/tablecodec/tablecodec.rs` 和 row/codec 解码路径。固定宽度截高位与 Context 截断策略是两个不同契约，不应混合。
- 改变比较或相等语义前，应明确是原始字节相等还是数值相等；`PartialEq` 与 `Compare` 当前故意可能给出不同结论。相应测试应放在 `pkg/types/binary_literal_test.rs`，并检查 Datum/比较测试中的使用。
- 若需要无损字节到文本转换，不应直接改变 `ToString` 而忽略 Go 兼容性；可考虑新增明确编码方式的 API，并补充非法 UTF-8 回归测试。现有有效 UTF-8 用例不足以覆盖这一风险。
- 若要减少大 BIT 字面量的峰值分配，可考虑不构造补齐后的完整 `String` 而按首块位数解析；必须保持左补零、字节边界与错误输入行为，并用独立基准或大输入测试证明收益。
- 新的公开符号需要从 `pkg/types/internal/scalar/lib.rs` 的现有 glob re-export 路径保持可达；若期望根 `astersql-types` 直接暴露，还需显式评估 `pkg/types/lib.rs` 的再导出策略和 Cargo 依赖边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/types/binary_literal.rs --offset 1 --limit 400` 读取了目标文件 252 行全貌；`query` 定位了目标 `BinaryLiteral`、`ParseBitStr`、`ParseHexStr`、`NewBinaryLiteralFromUint` 节点；对精确节点执行 `callers`/`callees` 无返回，故跨文件调用改由源码调用点核验。
- 生产源码：`pkg/types/binary_literal.rs`；模块与 crate 边界：`pkg/types/internal/scalar/lib.rs`、`pkg/types/internal/scalar/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`；截断策略：`pkg/types/context.rs`。
- 上游直接证据：`pkg/types/parser_driver/value_expr.rs` 与其 `Cargo.toml`、`pkg/expression/chunk_executor.rs`、`pkg/types/datum.rs`、`pkg/tablecodec/tablecodec.rs`、`pkg/util/rowcodec/common.rs`；仓库级 `rg` 同时核对了其他构造、转换和编解码调用点。
- 语义对照：`pkg/types/binary_literal.go`、`pkg/types/binary_literal_test.go`；Rust 独立测试：`pkg/types/binary_literal_test.rs`、`pkg/types/binary_literal_1_aster_unit_test.rs`。这些测试覆盖解析表、格式化、比较、整数构造与截断；HEX 空字符串测试误调用 BIT 解析器这一覆盖缺口已在文档中标出。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的结构命令，要求文档存在且固定二级标题恰好 11 个；并人工复核只有本说明文档与任务文件删除属于本任务变更，`plan.md` 未修改。

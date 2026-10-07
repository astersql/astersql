# `pkg/parser/test_driver/test_driver_datum.rs`

## 文件定位

本文件属于 `astersql-parser-test_driver` crate。crate 入口 [`lib.rs`](./lib.rs) 以私有模块 `test_driver_datum` 装载它，再用 `pub use test_driver_datum::*` 对外重导出全部公开符号；[`Cargo.toml`](./Cargo.toml) 表明该 crate 直接依赖 parser 的 charset、format、mysql、types 四个子 crate，以及用于十六进制编解码的 `hex`。它不是完整运行时的 `pkg/types::Datum`，而是解析器测试驱动使用的轻量值容器和字面量支持层。

在解析主链中，[`yy_parser.rs`](../yy_parser.rs) 的 `toHex`/`toBit` 把词法文本交给本文件的 `NewHexLiteral`/`NewBitLiteral`，成功后将带类别的值放入语义值；[`parser_semantic_support.rs`](../parser_semantic_support.rs) 再识别 `HexLiteral`、`BitLiteral` 和 `MyDecimal` 并构造 AST 表达式。另一条测试驱动链位于 [`test_driver.rs`](./test_driver.rs)：`ValueExpr::new` 先调用 `DefaultTypeForValue` 推断字段类型，再调用 `Datum::SetValue` 保存字面值，之后 `Restore`/`Format` 按 `Datum::Kind` 输出 SQL 文本。

## 核心职责

本文件有三组紧密相关的职责：

1. 用 `Datum { k, i, b, x }` 和 `Kind*` 判别常量统一保存 NULL、整数、浮点、字符串、字节、十进制、二进制字面量和动态载荷，并提供 Go 风格的访问器、setter 与构造函数。
2. 定义 `BinaryLiteral`、`BitLiteral`、`HexLiteral`，实现 BIT/HEX 文本解析及显示，使解析阶段保留源码字面量类别，而底层统一使用字节序列。
3. 通过 `DefaultTypeForValue` 为测试驱动字面值补齐 `types::FieldType` 的类型、显示长度、小数位、字符集、排序规则和标志位。

它刻意只覆盖 parser test driver 当前需要的类型集合。`KindMysqlDuration`、`KindMysqlEnum`、`KindMysqlSet`、`KindMysqlTime`、`KindMysqlJSON` 等判别值为与 Go 常量布局对齐而存在，但本文件没有相应的强类型存取器；[`test_driver.rs`](./test_driver.rs) 对这些 kind 的还原也明确返回未实现错误或 panic，不能据此推断完整运行时类型能力。

## 主要符号

- `KindNull` 至 `KindMysqlJSON`：值为 `0..=18` 的公开 `u8` 判别常量，与同路径 Go 文件保持编号一致。`KindBinaryLiteral` 表示解析得到的 BIT/HEX 字面量，`KindMysqlBit` 表示 BIT 列值，两者在 `GetValue` 中都恢复为二进制值。
- `Datum`：内部字段均为私有。`k` 保存 kind；`i` 复用保存 `i64`、`u64` 位模式和浮点位模式；`b` 保存字符串、原始字节或二进制字面量；`x: Option<Box<dyn Any>>` 保存十进制或其他动态对象。
- `DatumValue<'a>`：`GetValue` 的借用返回枚举。原始数值按值返回，字节、`MyDecimal` 和动态 `Any` 载荷借用 `Datum`，避免为模拟 Go `any` 而丢失载荷或强制复制。
- `Datum::{Kind, Get*, Set*, GetValue, SetValue}`：公开的 Go 风格 API。`SetValue(Box<dyn Any>)` 用运行时类型分派；无法识别的值进入 `KindInterface`。`GetMysqlDecimal` 和部分 `GetValue` 分支依赖内部 kind/载荷一致，不一致时会 panic。
- `NewDatum`、`NewBytesDatum`、`NewStringDatum`、`MakeDatums`：便利构造器。`NewDatum` 对 `Vec<Box<dyn Any>>` 做递归逐项包装，其余类型直接交给 `SetValue`。
- `BinaryLiteral(Vec<u8>)`、`BitLiteral(Vec<u8>)`、`HexLiteral(Vec<u8>)`：三个公开元组结构。`BinaryLiteral::{String, ToString, ToBitLiteralString}` 分别产生 `0x` 十六进制文本、按 UTF-8 损失替换的字符串、以及 `b'...'` 位串。
- `ParseBitStr`/`NewBitLiteral`：接受 `b'val'`、`B'val'`、`0bval`；将位数左侧补零到整字节，再按 8 位一组解析。
- `ParseHexStr`/`NewHexLiteral`：接受 `x'val'`、`X'val'`、`0xval`；引号形式强制偶数个十六进制数字，`0x` 形式允许奇数位并自动补一个前导零。
- `SetBinChsClnFlag`：统一设置 binary 字符集、binary 排序规则和 `mysql::BinaryFlag`。
- `DefaultFsp`：公开常量 `0`，对齐 MySQL 默认小数秒精度；本文件当前没有读取它的流程。
- `DefaultTypeForValue`：按动态类型推断 `FieldType`。私有辅助 `format_float_fixed` 专门对齐 Go `strconv.FormatFloat(..., 'f', -1, bits)` 所需的有限值和 `NaN`/`±Inf` 文本长度。

## 执行流程

字面量解析链如下：

1. [`yy_parser.rs`](../yy_parser.rs) 的 `toHex` 或 `toBit` 收到 lexer 文本，调用 `NewHexLiteral` 或 `NewBitLiteral`。
2. 构造器分别委托 `ParseHexStr` 或 `ParseBitStr`。解析函数先验证前缀和空输入，再规范化数字长度，最后生成 `BinaryLiteral(Vec<u8>)`；构造器只改变外层类别为 `HexLiteral`/`BitLiteral`。
3. 成功值作为 `Box<dyn Any>` 进入 parser 语义值；失败字符串被 `toHex`/`toBit` 包装为 lexer 错误并返回 `invalid` token。
4. [`parser_semantic_support.rs`](../parser_semantic_support.rs) 的 `semantic_value_expr` 对具体类别做 downcast，分别生成 `ExprNode::HexValue` 或 `ExprNode::BitValue`，因此相同字节不会丢失原始 SQL 类别。

`ValueExpr` 构造链如下：

1. [`test_driver.rs`](./test_driver.rs) 的 `ValueExpr::new` 接收动态值；若输入本身已是 `ValueExpr`，直接复用原节点。
2. 对新值调用 `DefaultTypeForValue`：NULL/布尔/数值/字节/各种 literal/十进制各自设置类型元数据；未知值设置 `TypeUnspecified` 和两个 `UnspecifiedLength`。
3. 同一动态值随后进入 `Datum::SetValue`。布尔被规范化为 `KindInt64` 的 `0/1`，BIT/HEX 包装类型被统一存成 `KindBinaryLiteral`，未知类型原样存入 `x`。
4. `ValueExpr::Restore` 和 `Format` 根据 kind 与 `FieldType` 标志决定输出。例如 `HexLiteral` 的类型推断加入 `UnsignedFlag`，使 `KindBinaryLiteral` 还原为 `x'...'`；普通 BIT literal 则用 `ToBitLiteralString(true)` 输出。

## 数据与状态

`Datum` 是可变、单所有者的数据盒，不维护跨实例全局状态。setter 会更新 `k` 和对应存储槽，但通常不会清理其他不再使用的槽；读取必须以 `k` 为准。明确清理动态载荷的只有 `SetNull`，它同时把 `x` 设为 `None`。因此安全扩展应保持“setter 设置正确 kind，getter 只读取该 kind 对应槽位”的不变量，不应把旧槽位内容当成当前值。

`u64` 通过 `as i64` 保存后再 `as u64` 取回，保留二进制位模式，包括 `u64::MAX`。`f64` 通过 `to_bits`/`from_bits` 保存；`f32` 先提升为 `f64` 再保存，因此 `GetFloat32` 是对应的逆向数值转换。字符串和字节共用 `b`，区别仅在 kind；`GetString` 使用 `String::from_utf8_lossy`，非法 UTF-8 会在文本视图中被替换，但 `GetBytes` 仍返回原字节。

二进制 literal 的公开元组字段允许调用方取得或移动字节。`ZeroBinaryLiteral` 是不可变空切片，仅表达共享的空值概念；实际空解析结果使用默认的空 `Vec<u8>`。`DatumValue` 中的借用生命周期绑定到 `&self`，动态载荷和字节切片不能比原 `Datum` 活得更久。

`DefaultTypeForValue` 会修改调用者传入的现有 `FieldType`，并非总是从零初始化。各分支只覆盖其负责的字段/标志；调用方若复用一个已有 `FieldType`，旧标志可能保留。例如函数为 `HexLiteral` 添加 `UnsignedFlag`，但字符串分支不主动清除它。因此常规用法是像 `ValueExpr::new` 一样从 `FieldType::default()` 开始；若要支持复用，需先明确并测试标志清理策略。

## 依赖与调用关系

上游关系：

- [`lib.rs`](./lib.rs) 重导出本文件公开 API，外部使用 crate 名 `parser_test_driver` 访问。
- [`yy_parser.rs`](../yy_parser.rs) 的 `toHex -> NewHexLiteral -> ParseHexStr` 与 `toBit -> NewBitLiteral -> ParseBitStr` 是 parser 主链的直接入口。
- [`test_driver.rs`](./test_driver.rs) 的 `ValueExpr::new -> DefaultTypeForValue` 和 `ValueExpr::new -> Datum::SetValue` 是测试驱动表达式构造入口；同文件的 `Restore`/`Format` 调用各类 getter 和 `BinaryLiteral::ToBitLiteralString`。
- [`parser_semantic_support.rs`](../parser_semantic_support.rs) 消费 `HexLiteral`、`BitLiteral`、`MyDecimal`，把 lexer 动态值转换为 AST 枚举。

下游关系：

- `crate::*` 提供同 crate 的 `MyDecimal`、`StrLenOfInt64Fast`、`StrLenOfUint64Fast`，以及由入口重导出的 `charset`、`mysql`、`types`。
- `parser_types::FieldType` 的 setter、`UnspecifiedLength` 和 parser mysql/charset 常量定义类型推断结果。
- `hex::encode` 支持 `BinaryLiteral::String`，`hex::decode` 支持 `ParseHexStr`。
- 标准库 `Any` 提供动态类型检查与 downcast；`Vec`/`Box` 承担载荷所有权。

RustCodeGraph 对本文件识别出 45 个符号，并确认 `NewBitLiteral -> ParseBitStr`、`NewHexLiteral -> ParseHexStr`、`NewDatum -> SetValue/MakeDatums`、`DefaultTypeForValue -> SetBinChsClnFlag/format_float_fixed` 等下游边。索引的同名 `callers` 查询没有返回上游边，所以上游入口另外由已索引的 `yy_parser.rs` 源码节点和精确引用检索确认，未把 `pkg/types` 中的同名实现混入本文件关系。

## 错误处理与边界

- `ParseBitStr("")` 与 `ParseHexStr("")` 返回明确错误；无法识别的前缀也返回错误。BIT 内容中的非 `0/1` 字符由 `u8::from_str_radix` 报错，HEX 非法字符由 `hex::decode` 报错，均转换为 `String`。
- `b''`/`B''`、`x''`/`X''` 和空数字的 `0b`/`0x` 会得到空 literal。BIT 位数不要求为 8 的倍数，左侧补零；引号 HEX 必须偶数位，非引号 `0xabc` 会规范化为字节 `[0x0a, 0xbc]`。
- 前缀判断区分大小写：引号形式接受小写或大写首字母，数字形式只接受小写 `0b`/`0x`，与当前 Go 实现一致。两种解析器使用 `trim_matches('\'')`，它会剥离两端任意数量的单引号，而不是严格验证恰好一对；调用者不能把这里当成完整 lexer 语法校验器。
- `BinaryLiteral::String` 对空值返回空串，而 `ToBitLiteralString` 对空值返回 `b''`；两者用途不同。`ToString` 和 `Datum::GetString` 都采用 UTF-8 损失替换，不保证字节到文本无损。
- `GetMysqlDecimal` 在 `x` 缺失或类型不符时 panic。`GetValue` 对未列举 kind 默认要求 `x` 存在，否则以 `"interface datum has no value"` panic；因此仅设置 `k` 或使用尚未实现的 kind 不能形成有效值。
- `SetValue` 支持 `i32`，用它近似 Go 的 `int` 分支；其他 Rust 整数宽度会进入动态 interface。`NewDatum` 的列表特例只识别精确类型 `Vec<Box<dyn Any>>`。
- `DefaultTypeForValue` 的字符串 `flen` 使用 UTF-8 字节长度，延续 Go `len(string)` 行为；未知动态类型不会报错，而是给出 unspecified 类型。特殊浮点数通过 `format_float_fixed` 得到 `NaN`、`+Inf`、`-Inf` 的正确显示长度。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。解析和类型推断都是同步、局部计算；除分配 `Vec`、`String`、`Box<dyn Any>` 外没有外部资源生命周期。

`Datum` 对 `b` 和 `x` 拥有所有权；`SetBytes`、literal setter 和 `SetInterface` 将输入移动进容器。`GetBytes`、`GetMysqlDecimal`、`GetInterface` 以及 `DatumValue` 的借用变体只在 `Datum` 借用期内有效。`GetBinaryLiteral` 和字符串 getter 会复制/分配，`BitLiteral::ToString`、`HexLiteral::ToString` 也因构造临时 `BinaryLiteral` 而克隆字节；在热路径扩展时需要留意这些分配，但当前文件没有共享可变状态引发的数据竞争。

`Box<dyn Any>` 未要求 `Send` 或 `Sync`，所以 `Datum` 的动态载荷不保证可跨线程传递或共享。若未来需要并发边界，不能仅给外层类型增加线程安全声明，必须同时约束载荷类型并审计 API 兼容性。

## 与 Go 版本的对应关系

直接对照文件是 [`test_driver_datum.go`](./test_driver_datum.go)。Rust 保留了 Go 的 kind 编号、`i/b/x` 紧凑布局思想、type switch 分派、BIT/HEX 规则、`FieldType` 推断顺序和公开函数命名，便于逐项核对迁移行为。

主要语言适配如下：

- Go 的 `any`/type switch 对应 Rust `Box<dyn Any>` 加 `is`/`downcast`；Go `nil` 对应传入的单元值 `()`，而不是 Rust 的空指针概念。
- Go `GetValue() any` 可直接返回 nil 或任意值；Rust 用 `DatumValue<'_>` 保留返回形状和借用关系。Rust 对 `KindNull` 显式返回 `DatumValue::Null`，并让未知 kind 的动态载荷保持可 downcast，相关迁移测试固定了这一行为。
- Go `[]byte`/别名切片对应三个 `Vec<u8>` 新类型；Rust getter 对二进制 literal 会克隆，因为不能像 Go 切片那样无条件按值共享底层数组。
- Go 的 `errors.Trace` 错误链简化为 `Result<_, String>`，保留失败而不保留结构化错误类型或堆栈包装。
- Go `int` 分支在 Rust 主要由 `i32` 表示；`DefaultTypeForValue` 和 `SetValue` 的支持类型集合必须同步维护，否则同一输入可能获得一种 `FieldType` 却被 `Datum` 存成 interface。
- Go `*MyDecimal` 对应拥有所有权的 `MyDecimal`；Rust `GetMysqlDecimal` 返回借用。`DefaultTypeForValue` 读取 `digitsFrac` 和 `String()`，保持长度与小数位语义。
- Go 的 `ZeroBinaryLiteral` 是可用作返回值的空 slice；Rust 静态量类型为 `&[u8]`，解析函数实际返回 `BinaryLiteral::default()`，因此该静态量当前只承担兼容命名而未接入返回路径。

## 扩展指南

新增一种 `Datum` 类型时，应最少同步以下位置：kind 常量；必要的存储槽和强类型 getter/setter；`SetValue` 的 downcast 分支；`GetValue`/`DatumValue` 的返回分支；`DefaultTypeForValue` 的类型、长度、小数位、字符集和 flag 规则；以及 [`test_driver.rs`](./test_driver.rs) 中 `ValueExpr::Restore`/`Format` 的输出分支。若该类型来自 lexer，还需同步 [`yy_parser.rs`](../yy_parser.rs) 的构造入口和 [`parser_semantic_support.rs`](../parser_semantic_support.rs) 的语义值转换。

新增或改变 BIT/HEX 语法时，应修改 `ParseBitStr`/`ParseHexStr`，保持 `New*Literal` 只是薄包装，并检查 lexer 对前缀、引号和错误位置是否已经做了更严格约束。不要把完整 SQL 词法验证悄悄塞入只有字节解码职责的函数，除非同时对齐 Go 行为和 parser 错误文本。

测试应继续放在独立文件，不能内嵌到本源文件。直接回归面是 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)：应增加有效/无效输入、kind 与载荷往返、默认 `FieldType`、特殊浮点和动态 interface 测试。若变更影响 `ValueExpr` 输出，还应同步 [`test_driver_test.rs`](./test_driver_test.rs) 或现有迁移测试；若影响 parser 入口，应在 `pkg/parser` 独立测试中覆盖 token 到 AST 的整条链。Go 对照行为变化时，同时核对 [`test_driver_datum.go`](./test_driver_datum.go)，不得用 `pkg/types` 的同名完整实现替代 test driver 的局部契约。

兼容性风险主要是 kind 编号变化、BIT/HEX 规范化变化、`FieldType` flag 残留或清理差异、动态类型集合不对称；性能风险主要是字符串/二进制 getter 的克隆与格式化分配。任何扩展都应先保持 Go 分支顺序和既有错误边界，再针对 Rust 所有权增加必要适配。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/parser/test_driver` 找到本 crate 的 13 个 Go/Rust 文件；`node --file pkg/parser/test_driver/test_driver_datum.rs` 读取目标文件 547 行及 45 个符号。
- RustCodeGraph 调用边：`callees NewBitLiteral` 确认 Rust `NewBitLiteral -> ParseBitStr`；`callees NewHexLiteral` 确认 `NewHexLiteral -> ParseHexStr`；`callees NewDatum` 确认 `NewDatum -> SetValue/MakeDatums`；`callees DefaultTypeForValue` 确认其调用 `SetBinChsClnFlag` 和 `format_float_fixed`。同名 `callers` 结果为空，故没有据此声称完整反向调用图。
- 已读生产源码：[`test_driver_datum.rs`](./test_driver_datum.rs)、[`lib.rs`](./lib.rs)、[`test_driver.rs`](./test_driver.rs)、[`yy_parser.rs`](../yy_parser.rs)、[`parser_semantic_support.rs`](../parser_semantic_support.rs)。
- 已读配置与 Go 对照：[`Cargo.toml`](./Cargo.toml)、[`test_driver_datum.go`](./test_driver_datum.go)。Cargo 证实 crate 名、入口、五项直接依赖和 Go 包映射；Go 文件用于逐分支核对 kind、存取、字面量解析和默认类型推断。
- 已读独立 Rust 测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`test_driver_test.rs`](./test_driver_test.rs)、[`go_merge_35_test.rs`](./go_merge_35_test.rs)。其中前者直接覆盖本文件的 BIT/HEX、Datum、NULL/interface、默认类型和特殊浮点；后两者覆盖消费本文件结果的 `ValueExpr` 格式化和 visitor 边界。同目录没有直接覆盖这些符号的 Go `*_test.go`，Go 语义以同路径实现为对照。
- 本任务是纯文档分析，按计划不运行 Cargo。最终使用任务指定命令验证本文档存在且恰含 11 个固定二级标题，并人工复核只新增本文档、未修改 Rust/Go/Cargo/`plan.md`。

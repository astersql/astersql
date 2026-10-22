# `pkg/util/logutil/hex.rs`

## 文件定位

`ProtoValue` 的 `Display` 实现统一定义空值、标量、字符串、字节、列表与嵌套消息的文本形式。

## 核心职责

- `ProtoField` 和 `ProtoValue` 把待展示内容显式建模成字段树，而不是执行 protobuf 编解码。
- `ProtoValue` 的 `Display` 实现统一定义空值、标量、字符串、字节、列表与嵌套消息的文本形式。
- `write_hex` 将每个字节拆成高、低半字节，映射到 `0123456789abcdef`，不插入前缀、分隔符或空格。
- `Hex` 返回借用输入的惰性 `Display` 适配器；`pretty_print` 则立即分配并返回 `String`。
- 消息渲染跳过名称以 `XXX` 开头的生成器内部字段，以保持 `pkg/util/logutil/hex.go` 的历史输出约定。

这里的职责仅是日志文本表示。它不验证 protobuf schema，不转义普通字符串，也不承诺输出可被反序列化。

## 主要符号

- `pub struct ProtoField { pub name: String, pub value: ProtoValue }`：一项有序消息字段。`ProtoField::new(name, value)` 接受 `Into<String>` 与 `Into<ProtoValue>`，便于从常用标量和字节类型构造。
- `pub enum ProtoValue`：封闭的展示值集合，包含 `Nil`、`Bool(bool)`、`I64(i64)`、`U64(u64)`、`String(String)`、`Bytes(Vec<u8>)`、`List(Vec<ProtoValue>)` 和 `Message(Vec<ProtoField>)`。
- `ProtoValue::message(fields)`：`Message` 变体的便捷构造器；字段顺序由传入的 `Vec` 保留。
- `From<bool/i64/u64/String/&str/&[u8]/Vec<u8>> for ProtoValue`：`ProtoField::new` 的输入适配层。切片输入会复制到新的 `Vec<u8>`，拥有所有权的 `Vec<u8>` 则直接移动。
- `impl fmt::Display for ProtoValue`：核心递归格式化器，决定全部公开输出语义。
- `fn write_hex(formatter, bytes)`：私有字节编码器，每个输入字节固定产生两个小写 ASCII 字符。
- `pub struct HexStringer<'a>(&'a ProtoValue)`：持有借用的惰性包装器，其 `Display` 直接委托给内部 `ProtoValue`。
- `pub fn Hex(&ProtoValue) -> HexStringer<'_>`：保留 Go 命名的公开入口，使用 `#[allow(non_snake_case)]`。
- `pub fn pretty_print(&ProtoValue) -> String` 与 `pub use pretty_print as prettyPrint`：立即渲染入口及 Go 风格别名。

文件没有模块级可变状态、trait 声明或条件编译项；唯一常量是 `write_hex` 内部的十六进制字符表 `DIGITS`。

## 执行流程

1. 调用者先用 `ProtoField::new`、`ProtoValue::message` 及各个 `From` 实现构造有序值树。
2. 调用 `Hex(value)` 时只创建 `HexStringer` 借用，不立即遍历或分配输出；真正写出发生在后续 `format!`、日志格式化或 `to_string()` 调用中。调用 `pretty_print(value)` 时则直接走 `value.to_string()`。
3. `ProtoValue::fmt` 按变体分派：`Nil` 写 `<nil>`；布尔和整数采用标准十进制 `Display`；字符串原样写出；字节交给 `write_hex`。
4. `List` 先写 `[`，递归渲染各元素并以单个空格分隔，最后写 `]`。空列表得到 `[]`。
5. `Message` 先写 `{`，按原始字段顺序遍历。名称以 `XXX` 开头的字段完全跳过；其他字段写成 `name:value`，值继续递归格式化，最后写 `}`。
6. 消息空格是否出现由原始字段索引而非“已输出字段数”决定。因此首字段若为 `XXX_Internal`、第二字段为 `Id`，结果是 `{ Id:7}`。`pkg/util/logutil/hex_test.rs::test_skipped_leading_xxx_field_preserves_go_spacing` 固定了这一看似特殊但与 Go 一致的行为。

## 数据与状态

值树全部由调用者拥有：字段名和普通字符串使用 `String`，字节使用 `Vec<u8>`，列表和消息字段使用 `Vec` 保序。`Clone`、`Debug`、`PartialEq` 派生只提供值复制、调试和比较能力，不改变展示语义。

`HexStringer<'a>` 只保存一个 `&'a ProtoValue`，所以不能比输入值存活更久，也不会复制整个值树。相比之下，`pretty_print` 总会创建完整输出 `String`。格式化过程不缓存结果；同一个值被多次格式化会重复遍历。

消息字段顺序和重复字段名均被原样保留。空字节数组输出空文本，所以消息中的空字节字段表现为 `EndKey:`；`Nil` 才表现为 `<nil>`。普通 `String` 不加引号且不转义，字段名也不校验，这些都是日志可读格式而非无歧义序列化格式的性质。

## 依赖与调用关系

向下调用关系为：`HexStringer::fmt -> ProtoValue::fmt`，`pretty_print -> ProtoValue::to_string -> ProtoValue::fmt`，`ProtoValue::fmt(Bytes) -> write_hex`；`List` 和 `Message` 分支递归调用子值的 `Display`。本文件仅使用 `std::fmt::{Formatter, Result, Display}` 及标准字符串/集合类型。

向上关系由源码引用核验：

- `pkg/util/logutil/lib.rs` 声明公开 `hex` 模块。
- `pkg/util/logutil/hex_test.rs` 覆盖 `Hex`、测试别名 `PrettyPrint`、字节和消息渲染。
- `pkg/util/logutil/main_test.rs` 将 `prettyPrint` 重导出成 `PrettyPrint`，供从 Go 测试机械迁移来的命名使用。
- `pkg/util/logutil/migration_aster_unit_test.rs::test_hex_matches_go_field_and_byte_formatting` 提供另一组 Go 对齐回归证据。
- `pkg/util/logutil/build.rs` 会收集相邻 `*_test.rs`，因此上述独立测试文件由 crate 的自定义测试注册器纳入；测试逻辑没有放进生产源文件。

RustCodeGraph 将 `ProtoValue`、`pretty_print` 与 `write_hex` 定位到本文件，并能展示文件源码；但对 `hex.rs::pretty_print` 的精确 callers 查询在本次检查中没有返回完成结果。调用者结论因此以仓库内精确 Rust 引用搜索补证，且不将图工具对同名 Go/Rust `Hex` 的宽泛结果当作本符号调用边。

## 错误处理与边界

所有格式化函数通过 `fmt::Result` 传播底层 `Formatter` 写入错误；`pretty_print` 写入内存 `String`，其公开返回类型不暴露可恢复错误。`write_hex` 把两个来自 ASCII 常量表的字节临时组成 UTF-8，并用 `expect("ASCII hex")` 转换；按 `DIGITS` 不变量该断言不会失败，若常量未来改成非 ASCII 内容则会 panic。

当前类型只覆盖布尔、64 位有/无符号整数、字符串、字节、列表、消息和空值，不支持浮点、枚举名、map、其他整数宽度或真实 protobuf 描述信息。嵌套格式化使用递归，极深的人工值树可能消耗大量栈；巨大的字节字段会产生两倍字节数的文本并增加日志量。普通字符串与字段名不转义，含空格、冒号、括号或控制字符时输出可能产生歧义。

跳过规则是大小写敏感的 `starts_with("XXX")`，会跳过任何此前缀的手工字段，而不只是真正的 protobuf 生成字段。保留原字段索引决定空格是与 Go 版逐字段循环完全对齐的兼容细节，修改它会改变既有日志文本。

## 并发与资源生命周期

本文件没有全局变量、锁、线程、异步任务、通道、文件句柄或网络资源。只要调用者持有的 `ProtoValue` 可按 Rust 类型规则共享，格式化本身只读且没有共享可变状态；`HexStringer` 的生命周期由其借用静态保证。

临时资源主要是输出缓冲：惰性 `HexStringer` 本身不分配，最终 formatter 决定存储；`pretty_print` 分配返回字符串。构造 `ProtoValue` 时，`&str` 与 `&[u8]` 转换会复制数据，拥有权版本则移动数据。所有资源均由普通所有权和作用域自动释放。

## 与 Go 版本的对应关系

两版 API 和覆盖范围并不等价。Go `Hex(msg proto.Message)` 保存真实 protobuf 消息，随后用 `reflect` 自动遍历 slice、struct、pointer 和其他值；Rust `Hex(&ProtoValue)` 只接受调用者显式构造的封闭值树，不读取 protobuf 元数据，也不会自动适配生成消息。Go `prettyPrint(io.Writer, reflect.Value)` 写入任意 writer，Rust `pretty_print` 固定返回 `String`，而 `Display` 才是可写入 formatter 的底层接口。

Go 测试通过 `main_test.go` 的 `PrettyPrint = prettyPrint` 暴露私有函数；Rust 测试以 `main_test.rs` 重导出 `prettyPrint` 模拟这一入口。

## 扩展指南

若要支持新标量类型，应在 `ProtoValue` 增加明确变体，并同步更新 `Display` 的穷尽匹配、所需的 `From` 实现以及独立的 `pkg/util/logutil/hex_test.rs`；还应选取 `hex.go` 可产生的对应值验证 Go 文本，而不是自行发明格式。若目标是真实 protobuf 自动适配，应单独设计从具体生成类型或统一 protobuf 反射接口到 `ProtoValue` 的边界，不能仅扩展 `Hex` 签名后声称与 Go 的 `proto.Message` 等价。

若要改变字节大小写、字段过滤、分隔空格、字符串转义或 nil 表示，必须把日志兼容性视为外部行为：同步 `hex_test.rs`、`migration_aster_unit_test.rs` 和 Go 对照断言，并评估下游日志解析器。性能优化应优先保留 `Display` 的流式写入；避免让 `Hex` 提前创建中间 `String`。对大字节字段增加截断时，必须显式定义阈值和标记，防止不同 key 产生不可区分的日志。

测试仍应放在相邻独立文件 `pkg/util/logutil/hex_test.rs` 或迁移回归文件中，不应内嵌到 `hex.rs`。新增 `*_test.rs` 会由 `pkg/util/logutil/build.rs` 的注册流程处理，需遵守其只支持普通、无 `Result` 返回值测试函数的约束。

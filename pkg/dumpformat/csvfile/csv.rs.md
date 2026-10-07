# `pkg/dumpformat/csvfile/csv.rs`

## 文件定位

源文件 [`csv.rs`](./csv.rs) 是 `astersql-dumpformat-csvfile` crate 内部的“单字段编码”实现。模块在 `pkg/dumpformat/csvfile/lib.rs` 中以私有 `mod csv` 引入，因此外部调用者不能直接调用本文件；同 crate 的 `pkg/dumpformat/csvfile/writer.rs` 通过 `csv::append_field` 将每个字段编码进一行缓冲区，再负责字段分隔、行终止和向 `std::io::Write` 输出。

它位于 Dumpling CSV 导出链的底层：`dumpling/export/writer_util.rs::writeCSVFile` 把导出配置映射成 `csvfile::Config`，创建 `csvfile::Writer`，随后由 `Writer::write_borrowed` 逐字段调用本文件的 `append_field`。本文件不负责查询、行迭代、文件轮转、对象存储写入或 I/O 错误处理。

crate 边界由 `pkg/dumpformat/csvfile/Cargo.toml` 定义：库入口是 `lib.rs`，唯一声明的依赖是同工作区的 `astersql-dumpformat`。本文件使用的 `FieldKind` 由该依赖定义并经 `csvfile.rs` 再导出；Base64 和十六进制编码均在本文件内实现，没有额外编码库依赖。

## 核心职责

- `append_field` 根据空值、字段种类和二进制格式决定输出形式。空值写 `Config::null_value`；数值原样写入；其余值在前后写完整的 `fields_enclosed_by`，并在中间执行二进制转换或转义。
- `append_escaped` 实现两套互斥的文本转义策略：配置了 `fields_escaped_by` 时使用反斜杠式逐字节转义；未配置转义符但配置了包围符时，将包围符的每个完整序列加倍；两者都没有配置时原样复制。
- `base64_encode` 把任意字节按 RFC 4648 常用 Base64 字母表编码，并为不足三字节的尾组补 `=`。它服务于 `FieldKind::Bytes + BinaryFormat::Base64` 分支。

本文件只做字节级格式化，不检查配置是否合法，也不判断最终文本是否构成无歧义 CSV。调用者必须保证分隔符、包围符、转义符和行终止符组合符合目标方言。

## 主要符号

- `pub(crate) fn append_field(dst: &mut Vec<u8>, val: Option<&[u8]>, kind: FieldKind, cfg: &Config)`：本文件唯一对 crate 内其他模块可见的入口。它追加到既有 `dst`，不清空缓冲区也不返回新缓冲区。`None` 是 SQL `NULL`；`Some(&[])` 是空但非空值，两者输出不同。
- `fn append_escaped(dst: &mut Vec<u8>, val: &[u8], cfg: &Config)`：私有文本转义器。反斜杠式模式只使用 `fields_escaped_by` 的第一个字节，并以包围符首字节为特殊字符；没有包围符时改用字段分隔符首字节。
- `fn base64_encode(data: &[u8]) -> String`：私有 Base64 编码器。每次合并最多三个输入字节为 24 位整数，依次映射四个 6 位索引，并按输入余数输出一个或两个 `=`。
- `HEX` 与 `T`：分别是 `append_field` 内的十六进制小写字母表和 `base64_encode` 内的 Base64 字母表，均为函数局部常量，不形成公共配置面。

文件中没有类型、trait、`impl`、条件编译项或可跨 crate 导出的 API；配置与枚举定义在 `pkg/dumpformat/csvfile/csvfile.rs`，行级状态在 `writer.rs::Writer<W>`。

## 执行流程

1. `writer.rs::Writer::write_borrowed` 校验行宽，清空可复用行缓冲区，在相邻字段之间追加 `Config::fields_terminated_by`，然后调用 `append_field`。
2. `append_field` 首先处理 `val == None`：只追加 `null_value` 并立即返回，因此空值不会被包围、转义或二进制编码。
3. 非空 `FieldKind::Number` 同样提前返回，原样追加输入字节，不添加包围符。数值格式的正确性由上游负责。
4. 其他非空字段先追加完整的 `fields_enclosed_by`。`FieldKind::Bytes` 且格式为 `HEX` 时，每个输入字节产生两个小写十六进制字符；格式为 `Base64` 时追加 `base64_encode` 的结果；其他组合进入 `append_escaped`。最后再次追加完整包围符。
5. `append_escaped` 若存在转义符，逐字节检查输入：NUL、回车、换行分别变成 `<esc>0`、`<esc>r`、`<esc>n`；转义符自身和特殊首字节变成 `<esc><原字节>`；其余字节直接写入。
6. 若没有转义符但存在包围符，函数从左向右寻找完整包围符序列并输出两份；索引按整个序列长度前进，因此匹配不会重叠。若两者皆空，直接复制值。
7. 字段返回后，`Writer` 继续组装该行，最终在 `flush_row` 追加 `lines_terminated_by` 并写向 sink。Dumpling 的 `writeCSVFile` 根据 `estimate_file_size` 决定何时停止当前文件，轮转不属于本文件。

## 数据与状态

本文件自身无持久状态。所有输出都累积在调用者传入的 `Vec<u8>` 中，配置通过不可变借用读取，输入值也只借用。`append_field` 的关键输入不变量如下：

- `Option<&[u8]>` 明确区分 SQL 空值和长度为零的非空值；空值占位符完全来自 `Config::null_value`。
- `FieldKind::Number`、`String`、`Bytes` 来自 `pkg/dumpformat/kind.rs::FieldKind`。只有 `Bytes` 会读取 `binary_format`；`Number` 永远原样输出，`String` 永远走文本转义。
- `fields_enclosed_by`、`fields_terminated_by` 可以是多字节序列。加倍模式匹配完整包围符，但反斜杠式模式只识别包围符或分隔符的首字节，这是与 Go 实现一致的既有协议。
- `fields_escaped_by` 即使包含多个字节也只取首字节；空配置切换到包围符加倍或原样模式。

内存方面，十六进制分支直接向目标缓冲区追加；文本转义同样直接追加。Base64 分支先创建一个新的 `String`，再把其字节复制进 `dst`，因此相较另外两个分支多一次临时分配。缓冲区容量管理和跨行复用由 `Writer::buf` 完成。

## 依赖与调用关系

上游调用链为：

`dumpling/export/writer_util.rs::writeCSVFile` → `pkg/dumpformat/csvfile/writer.rs::Writer::write_borrowed`（或表头路径 `write_header`）→ `csv.rs::append_field` → `append_escaped` / `base64_encode`。

`writeCSVFile` 还提供了应用语义：它用列类型生成 `FieldKind`，把 Dumpling 的 CSV 分隔符、包围符、行终止符、空值和方言二进制格式映射到本 crate，并将 SQL 行的借用字节交给 `write_borrowed`。`CSVObjectWriter` 再把 `std::io::Write` 适配到 Dumpling 的 `ObjectWriter`。

直接下游依赖只有：

- `crate::FieldKind`：决定数值、文本和二进制字段分支；原始定义见 `pkg/dumpformat/kind.rs`。
- `crate::BinaryFormat` 与 `crate::Config`：定义于 `pkg/dumpformat/csvfile/csvfile.rs`。
- 标准库的 `Vec<u8>`、切片、`String` 和基础位运算。

RustCodeGraph 的文件关系显示 `csv.rs` 被 `writer.rs` 使用；函数级 `callers/callees` 查询未返回边，因此上述精确函数调用关系由 `writer.rs` 的导入与调用点、以及 `writer_util.rs` 的构造和写入点交叉核验。

## 错误处理与边界

三个函数都不返回 `Result`，也不会主动报告非法配置或无效输入。可观察边界包括：

- `None` 直接输出空值标记；空的 `null_value` 会使空值不产生任何字节。
- 空数值、字符串或二进制值都是合法的非空输入。字符串/二进制值仍会写前后包围符；数值不写。
- 未配置包围符和转义符时，字符串中的字段分隔符与换行会原样输出，可能生成歧义记录；这是配置责任，不是本文件的错误。
- 反斜杠式模式只转义 NUL、CR、LF、转义符和一个特殊首字节，不会对包围符或分隔符的其余字节做序列级识别。
- 包围符加倍模式要求非空包围符后才进入循环，因此 `starts_with` 不会遇到空模式导致不前进。
- 手写 Base64 对空输入输出空串；一字节、两字节尾组分别补两个、一个 `=`。十六进制固定使用小写字符。
- 向 `Vec` 扩容可能因进程内存耗尽而终止，但该路径没有可恢复错误接口。真正的 I/O 错误出现在 `writer.rs::flush_row`，不在本文件内。

相关边界由 `pkg/dumpformat/csvfile/csv_test.rs` 覆盖：反斜杠转义、包围符加倍、空值与三种字段类型、HEX、Base64、无包围符的转义/原样输出、自定义转义符及多字节包围符。行宽错误、短写计数和空行属于相邻 `Writer` 的职责。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄或网络资源。函数通过 `&mut Vec<u8>` 获得输出缓冲区的独占借用，因此单次调用期间不能并发修改同一缓冲区；除此之外没有全局可变状态，局部常量可安全共享。

输入切片和配置只在调用期间借用，不会保存在函数外。`base64_encode` 的临时 `String` 在追加到 `dst` 后立即释放。跨字段、跨行的缓冲区生命周期由 `Writer` 管理：`write_borrowed` 每行先 `clear` 而保留容量，再由 `flush_row` 交给 sink。若要在多个线程中并行编码，应为每个线程或每个 writer 提供独立缓冲区和 sink，不能共享一个未经同步的 `Writer`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dumpformat/csvfile/csv.go`：

- Rust `append_field` 对应 Go `appendField`。Go 用 `isNull bool` 区分空值，Rust 将其收敛为 `Option<&[u8]>`；分支顺序和输出语义一致。
- Rust `append_escaped` 合并了 Go `appendEscaped` 与 `appendEscapedBackslash` 的逻辑。两边都优先选择反斜杠式转义，否则选择完整包围符加倍，再否则原样复制。
- Go HEX 使用 `fmt.Appendf(..., "%x", val)`，Rust 用小写查表直接追加；Go Base64 使用 `base64.StdEncoding.AppendEncode`，Rust 使用本地 `base64_encode`。目标字节格式相同，但 Rust 的 Base64 当前会产生临时 `String`。
- Go 用 `bytes.Index` 分段查找完整包围符；Rust 在当前位置用 `starts_with` 逐字节扫描。两者输出一致，长输入上的常数开销和搜索策略不同。
- 两边反斜杠式转义都只使用转义符首字节，并选择包围符首字节、否则字段分隔符首字节作为特殊字符。

`pkg/dumpformat/csvfile/csv_test.go` 与 `csv_test.rs` 对主要格式行为给出成对用例，包括反斜杠转义、引号加倍、空值、三类字段、HEX/Base64、无包围符模式、表头、行宽和大小统计。Rust 测试还显式覆盖自定义转义符、多字节包围符和短写计数。当前证据表明本文件是 Go 行为的直接移植，而不是功能简化版。

## 扩展指南

- 新增字段分类时，先更新共享的 `pkg/dumpformat/kind.rs::FieldKind`，再在 `append_field` 明确决定是否原样、包围、转义或二进制编码；同时更新独立的 `csv_test.rs` 和 Go 对照测试，避免 Rust/Go 分支漂移。
- 新增 `BinaryFormat` 时，在 `csvfile.rs::BinaryFormat`、`append_field` 和 Dumpling 的 `writer_util.rs::writeCSVFile` 配置映射处同步接线。测试至少应覆盖空输入、短尾组、非 ASCII 字节和与 Go 输出逐字节一致性。
- 修改转义规则时应优先改 `append_escaped`，并分别覆盖有转义符、仅有包围符、两者皆空三种模式；多字节包围符/分隔符与“只取首字节”的兼容约束必须显式决定，不能无意改变。
- 若优化 Base64 分配，可改为直接向 `dst` 追加，但必须保持标准字母表和 padding；应在 `csv_test.rs` 增加 0、1、2、3、跨组三字节及任意二进制用例。
- 若引入配置校验或可恢复编码错误，错误边界会从 `writer.rs` 的 I/O 层下沉到本文件，需要同步调整 `append_field` 签名、`Writer::write_borrowed`/`write_header` 的传播路径和 Dumpling 的 `csv_io_error` 映射。
- 性能修改应关注超长字段：当前包围符加倍逐字节检查，Base64 有临时分配，`Vec` 扩容取决于调用者容量。兼容风险主要是字节级输出变化，尤其是空值是否包围、数值是否转义、HEX 大小写和 Base64 padding。

测试逻辑应继续放在独立的 `pkg/dumpformat/csvfile/csv_test.rs`，不要内嵌回生产源文件；Go 语义变化则同步 `csv_test.go`。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/dumpformat/csvfile` 找到本目录 9 个 Go/Rust 文件；`node --file pkg/dumpformat/csvfile/csv.rs` 读取 115 行源码并报告文件被 `writer.rs` 使用；对 `append_field`、`append_escaped`、`base64_encode` 的 `query` 确认符号位置。三个符号的函数级 `callers/callees` 为空，故未把缺失图边当作不存在调用。
- 目标与模块源码：`pkg/dumpformat/csvfile/csv.rs`、`writer.rs`、`csvfile.rs`、`lib.rs`；共享字段类型：`pkg/dumpformat/kind.rs`。
- crate 与应用接线：`pkg/dumpformat/csvfile/Cargo.toml`、`pkg/dumpformat/Cargo.toml`、`dumpling/export/Cargo.toml`、`dumpling/export/writer_util.rs::columnKinds` 与 `writeCSVFile`。
- Go 对照：`pkg/dumpformat/csvfile/csv.go`、`writer.go`。
- 独立测试：`pkg/dumpformat/csvfile/csv_test.rs`、`csv_test.go`。这些测试是行为证据，本任务没有运行 Cargo，符合总计划对纯文档任务的限制。
- 人工事实复核重点：空值与空字节串的区分、数值提前返回、Bytes 的三种格式、两套转义策略、多字节配置在不同策略下的含义、Base64 padding、Dumpling 配置映射与文件轮转边界。

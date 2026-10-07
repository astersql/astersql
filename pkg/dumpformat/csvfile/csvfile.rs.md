# `pkg/dumpformat/csvfile/csvfile.rs`

## 文件定位

本文件是 `astersql-dumpformat-csvfile` crate 的公共配置契约层：定义二进制字段的文本表示方式 `BinaryFormat`、一行 CSV 的分隔/包围/转义配置 `Config`，并从父 crate `astersql-dumpformat` 重新导出列分类 `FieldKind`。crate 根 `pkg/dumpformat/csvfile/lib.rs` 通过 `pub use csvfile::*` 再导出这些符号，因此调用方通常直接写 `astersql_dumpformat_csvfile::{Config, BinaryFormat, FieldKind}`，不需要访问内部模块路径。

它不负责执行 CSV 编码或 I/O。直接消费这些定义的是同 crate 的 `csv.rs::append_field` 和 `writer.rs::Writer`；应用侧的已接线入口见 `dumpling/export/writer_util.rs::writeCSVFile`，该函数把 Dumpling 的导出配置转换成本文件的类型，再逐行写入对象存储 writer。

## 核心职责

- 用 `BinaryFormat` 明确 `FieldKind::Bytes` 的三种输出策略：原始 UTF-8/字节转义、十六进制和标准 Base64。
- 用 `Config` 聚合 CSV framing 所需的六项调用方自定义参数：字段分隔符、字段包围符、转义符、行终止符、NULL 文本和二进制格式。
- 通过 `pub use astersql_dumpformat::FieldKind` 统一 CSV writer 与其他 dumpformat 模块使用的列类型分类，避免在 CSV crate 中复制另一套枚举。
- 保持配置层无策略默认值：`Config::default()` 的五个字节向量均为空，`binary_format` 因 `BinaryFormat::default()` 为 `UTF8`；注释明确真正的 `Writer` 不替调用方补齐 CSV 分隔符或行终止符。

## 主要符号

- `pub use astersql_dumpformat::FieldKind`：公开转发 `pkg/dumpformat/kind.rs` 中的 `FieldKind::{Number, String, Bytes}`。`Number` 在编码时不包围、不转义；`String` 和 `Bytes` 进入文本/字节编码路径。
- `pub enum BinaryFormat { UTF8, HEX, Base64 }`：可复制、可克隆、可调试并支持默认构造。`#[default] UTF8` 使未指定格式时走 `csv.rs::append_escaped`；`HEX` 输出小写十六进制；`Base64` 输出带标准 `=` padding 的 Base64。
- `pub struct Config`：可克隆、可调试并支持默认构造。所有字段公开且由调用方直接构造：
  - `fields_terminated_by: Vec<u8>`：相邻字段之间插入的原始字节序列。
  - `fields_enclosed_by: Vec<u8>`：字符串/字节字段前后添加的包围序列；空值表示不包围。
  - `fields_escaped_by: Vec<u8>`：转义配置；当前编码实现只读取首字节，空值则在有包围符时改用包围序列倍增。
  - `lines_terminated_by: Vec<u8>`：每行末尾追加的原始字节序列。
  - `null_value: Vec<u8>`：`None` 字段原样写出的 NULL 标记。
  - `binary_format: BinaryFormat`：只影响 `FieldKind::Bytes` 的非 NULL 值。

## 执行流程

1. `dumpling/export/writer_util.rs::writeCSVFile` 从导出配置构造 `Config`：分隔符、包围符、行结束符和 NULL 标记均转为字节向量；`EscapeBackslash` 决定转义字节是否为反斜杠；方言经 `DialectBinaryFormatMap` 映射到 `BinaryFormat`。
2. 同一入口通过 `columnKinds` 把数据库列类型分类为 `FieldKind::{Bytes, Number, String}`，再把 `Config` 和分类向量按值交给 `writer.rs::Writer::new`。
3. `Writer::write_borrowed` 校验行宽，插入 `fields_terminated_by`，并针对每个字段调用 `csv.rs::append_field`。
4. `append_field` 对 NULL 直接写 `null_value`，对数字直接写原字节；其他类型先写包围符，再依 `binary_format` 进行小写十六进制、标准 Base64 或通用转义，最后写闭合包围符。
5. `Writer::flush_row` 追加 `lines_terminated_by` 并调用底层 `Write::write`；Dumpling 根据 `estimate_file_size()` 判断是否达到文件轮转阈值。文件轮转、metrics、迭代器关闭均由 `writeCSVFile` 的上层流程负责，不属于本配置文件。

表头走 `Writer::write_header`，所有列名强制按 `FieldKind::String` 使用同一 `Config` 编码，而不使用数据列的原始类型。

## 数据与状态

`BinaryFormat` 是无载荷的小枚举，`Copy` 语义使 writer 在匹配配置时不必移动或克隆它。`Config` 拥有全部 `Vec<u8>`，因此可表达非 UTF-8 字节以及多字节分隔符、包围符和行结束符；这也避免其生命周期依赖调用方临时字符串。

本文件没有全局状态、缓存或内部可变性。`Config` 被按值移入 `Writer` 并在多行之间保持不变；`Writer` 另行持有可复用行缓冲区和累计写入字节数。需要注意，派生的 `Default` 只提供类型级零值，不等同于可直接使用的常见 CSV 方言：空的行结束符和字段分隔符不会被自动替换为 `\n`、`,` 等常用值。

## 依赖与调用关系

- crate 边界：`pkg/dumpformat/csvfile/Cargo.toml` 声明包名 `astersql-dumpformat-csvfile`，库入口为 `lib.rs`，唯一直接依赖是路径依赖 `astersql-dumpformat = { path = ".." }`；本文件通过它取得 `FieldKind`。
- crate 内下游：`pkg/dumpformat/csvfile/csv.rs` 导入 `BinaryFormat`、`Config`、`FieldKind` 并实现字段编码；`pkg/dumpformat/csvfile/writer.rs` 导入 `Config`、`FieldKind` 并驱动逐行 framing 与 I/O。
- 生产上游：`dumpling/export/writer_util.rs::writeCSVFile` 构造 `CF`（`Config` 的别名）、映射 `BF`（`BinaryFormat` 的别名），并创建 `CW::new`；`WriteInsertInCsv` 调用该流程并负责关闭行迭代器。
- 测试上游：`pkg/dumpformat/csvfile/csv_test.rs` 通过 crate 根再导出的符号构造配置和 writer；`dumpling/export/parity_test.rs` 直接引用重新导出的 `FieldKind::String`，验证跨 crate 公共契约可见。

RustCodeGraph 对目标文件给出的文件级引用包括 `dumpling/export/writer_util.rs`，并确认 `append_field` 的调用者是 `writer.rs::write_borrowed` 与 `write_header`。由于 `Config` 名称在仓库内高度重名，具体调用边以带文件上下文的查询结果和上述直接源码证据共同消歧。

## 错误处理与边界

本文件只声明数据类型，不产生 `Result` 或主动校验。边界行为由直接消费者决定：

- `Config::default()` 不保证形成标准 CSV；调用方必须显式给出 framing 参数。
- `fields_escaped_by` 虽是 `Vec<u8>`，`csv.rs::append_escaped` 只使用首字节；多余字节不会参与转义。Go 注释声称其长度应不超过 1，但 Rust 类型和构造过程当前没有强制此约束。
- 使用反斜杠转义时，编码器只以包围符首字节为优先特殊字节；没有包围符时改取字段分隔符首字节。多字节包围/分隔序列的完整匹配只发生在无显式转义符的包围符倍增分支。
- `None` 无条件写 `null_value`，不增加包围符；`FieldKind::Number` 原样写值，不应用 `BinaryFormat` 或转义。
- 行宽不等于 `Writer` 的 `kinds.len()` 时，`writer.rs::write_borrowed` 返回 `io::ErrorKind::InvalidInput`，且在清空/写入行缓冲区之前失败。
- 底层 sink 错误由 `Writer` 以 `io::Result` 传播；Dumpling 再由 `csv_io_error` 转成应用错误。当前 `flush_row` 使用一次 `Write::write` 而非 `write_all`，短写被视为成功，累计值只增加 sink 实际接受的字节数；`csv_test.rs::short_write_counts_actual_bytes` 固化了这一当前行为。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。配置是拥有型普通值，可由调用方克隆；`BinaryFormat` 可复制。是否能跨线程使用取决于承载它们的上层类型和 sink，而非这里的显式并发协议。

`Writer` 的写方法要求 `&mut self`，其配置、scratch buffer、sink 和计数器属于单个 writer 实例，未提供共享并发写入。调用方拥有资源生命周期：Dumpling 创建 writer、逐行调用、检查轮转阈值并关闭迭代器；`Writer::close` 当前是无操作，也不 flush 或关闭底层对象。因此扩展资源管理时不能假设 `Config` 或 `close` 已提供同步、缓冲落盘或所有权回收保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dumpformat/csvfile/csvfile.go`。Rust 与 Go 保持以下一一对应：

- Go `BinaryFormatUTF8/HEX/Base64` 对应 Rust `UTF8/HEX/Base64`，声明顺序一致；两侧默认/零值语义均为 UTF8。
- Go `Config` 的六个公开字段对应 Rust 的六个公开字段，字符串 framing 参数在 Rust 中改用 `Vec<u8>`，NULL 值两侧都是字节容器。
- 两侧 writer 都不应用配置默认值，字段分类均来自共享 dumpformat 的 Number/String/Bytes 三类。
- `csv_test.go` 与 `csv_test.rs` 对齐验证反斜杠转义、包围符倍增、NULL/类型分支、HEX、Base64、空行、无包围符模式、表头、文件大小和行宽错误。

已观察到的 Rust 特有证据是：`Config` 由值移入泛型 `Writer<W>`，并额外暴露借用输入的 `write_borrowed` 以避免 Dumpling 每行复制扫描值；Go 使用 `*Config`、`io.Writer` 和 `[]sql.RawBytes`。这属于所有权与接口形态差异，当前输出语义由成对测试保持一致。

## 扩展指南

- 新增二进制编码方式时，必须同步修改本文件的 `BinaryFormat`、`csv.rs::append_field` 的匹配分支、Dumpling `config.rs::BinaryFormat`/`DialectBinaryFormatMap` 到 `writer_util.rs::writeCSVFile` 的映射，以及 Go 对照枚举和映射；在独立的 `csv_test.rs` 与 `csv_test.go` 增加相同输入输出测试。
- 新增 framing 配置项时，在本文件扩展 `Config`，并检查 `writer.rs`、`csv.rs` 和 `writer_util.rs` 的构造/消费路径。公开结构体字面量会受新增必填字段影响；如可安全默认，应明确其 `Default` 与 Go 零值的兼容语义。
- 若要校验“转义符最多一个字节”等约束，优先在构造边界引入显式校验并返回可定位错误，避免让公开字段与实际只读首字节的行为继续分离；同时覆盖空值、单字节、多字节和非 UTF-8 输入。
- 若修改短写策略、`close` 语义或底层 flush 行为，修改点在 `writer.rs` 而非本文件，并应更新独立测试 `csv_test.rs::short_write_counts_actual_bytes` 及 Go 对照行为。
- Rust 单元测试应继续放在独立的 `pkg/dumpformat/csvfile/csv_test.rs`，不要嵌入本生产文件；应用接线变化还应补充 Dumpling 侧独立测试，尤其是方言到二进制格式的映射与文件轮转边界。

兼容风险主要来自公共枚举/结构字段变化和默认值变化；正确性风险集中在多字节 framing 与转义优先级；性能风险集中在为每行/字段新增分配或编码中间字符串（当前 Base64 已返回 `String`，其他路径直接追加到复用 buffer）。

## 验证依据

- 目标源码：`pkg/dumpformat/csvfile/csvfile.rs`（`FieldKind` 再导出、`BinaryFormat`、`Config`）。
- crate 与模块边界：`pkg/dumpformat/csvfile/Cargo.toml`、`pkg/dumpformat/csvfile/lib.rs`。
- 直接实现：`pkg/dumpformat/csvfile/csv.rs::{append_field, append_escaped, base64_encode}`、`pkg/dumpformat/csvfile/writer.rs::Writer`。
- 生产调用链：`dumpling/export/writer_util.rs::{columnKinds, writeCSVFile, WriteInsertInCsv}`。
- Go 对照：`pkg/dumpformat/csvfile/csvfile.go`、`csv.go`、`writer.go`。
- 独立测试：`pkg/dumpformat/csvfile/csv_test.rs` 与 `csv_test.go`；跨 crate 公共契约另见 `dumpling/export/parity_test.rs`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/dumpformat/csvfile` 确认目标模块九个已索引文件；目标文件节点确认 33 行和文件级使用者；精确文件查询确认 `append_field <- {write_borrowed, write_header}`，并以 `writer_util.rs` 的构造源码消除全仓库同名 `Config` 的歧义。
- 结构验收使用任务规定的命令，确认目标文档存在且固定二级标题恰好为 11 个；本任务是纯文档分析，按计划不运行 Cargo。

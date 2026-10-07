# `pkg/dumpformat/parquetfile/column_value.rs`

## 文件定位

本文件属于 `astersql-dumpformat-parquetfile` crate，处在 SQL 文本协议原始字节与 Parquet 类型化列缓冲之间。crate 根模块 `pkg/dumpformat/parquetfile/lib.rs` 公开 `column_value` 模块并重新导出 `ColumnValue`；类型元数据来自相邻的 `column_type.rs`，目标缓冲来自 `column_buffer.rs`。

写出主链中，`pkg/dumpformat/parquetfile/writer.rs::ParquetWriter::parse_column_value` 调用 `parse_raw_column_value`，随后 `ParquetWriter::append_parsed` 用 `account_column_value_memory_bytes` 记账并以 `append_column_value` 写入按物理类型拆分的缓冲。`dumpling/export/writer_util.rs::write_parquet_group` 也直接调用解析入口，再自行将 `ColumnValue` 分派给 Arrow Parquet column writer。读取侧 `pkg/dumpformat/parquetfile/parser.rs::EstimateParquetReaderMemory` 复用内存估算函数计算已物化列值的成本。

`Cargo.toml` 将该目录定义为独立库 crate，直接依赖带固定 tag 的 AsterSQL `arrow-rs/parquet`；但本文件自身只使用 crate 内部类型和标准库。文件没有条件编译项，也不持有真正的 Parquet writer。

## 核心职责

1. 用 `ColumnValue` 表示与 Parquet 物理类型一一对应的中间值，隔离上游 `&[u8]` 与下游类型化缓冲。
2. `parse_raw_column_value` 按 `Column.column_type.physical` 和逻辑类型解析布尔、整数、浮点、字节、DECIMAL 与 TIMESTAMP，并用 `(Option<ColumnValue>, bool)` 同时表达值与 NULL definition-level 语义。
3. `parse_decimal_to_scaled_integer` 和 `to_fixed_len_two_complement` 完成 DECIMAL 的定标整数转换及固定宽度大端二补码编码。
4. `append_column_value` 检查物理类型与枚举变体是否匹配，再追加到 `ColumnBuffer` 的对应向量。
5. `account_column_value_memory_bytes` 为写入阈值和读取估算提供近似内存成本；`write_column_batch` 则从类型化缓冲重建一批 `ColumnValue`。

本文件不负责 schema 映射、NULL 输入的 required/optional 前置校验、definition level 的存储、row-group flush 或真正的 Parquet 序列化；这些分别由 `column_type.rs`、`writer.rs`/`writer_util.rs`、`column_buffer.rs` 及 Arrow writer 完成。

## 主要符号

- `pub enum ColumnValue`：七种受支持物理值的 tagged union：`Bool`、`Int32`、`Int64`、`Float32`、`Float64`、`Bytes`、`FixedBytes`。`Clone` 支持批量重建和写出，`PartialEq` 支持测试比较。
- `fn pow10(scale: i32) -> Result<i128>`：以 checked multiplication 计算 `10^scale`；负 scale 或 `i128` 溢出返回 crate `Error`。
- `pub fn parse_decimal_to_scaled_integer(input, scale) -> Result<i128>`：拆分符号、整数和小数部分，将数值乘以 `10^scale`；少于 scale 的小数补零，多出的位向零截断。
- `pub fn to_fixed_len_two_complement(value, width) -> Result<Vec<u8>>`：验证 1 至 16 字节宽度和有符号范围，截取 `i128::to_be_bytes()` 的低位，得到定宽大端二补码。
- `fn days_from_civil`、`fn days_in_month`、`fn parse_datetime_micros`：无时区日期时间解析辅助。它们验证日历与时钟字段，把 `YYYY-MM-DD HH:MM:SS[.fraction]` 当作 UTC-like/local-semantics 数值换算为 Unix 微秒。
- `pub fn parse_raw_column_value(raw, column)`：核心解析分派；成功的普通值返回 `(Some(value), false)`，允许 NULL 编码的非法 TIMESTAMP 返回 `(None, true)`。
- `pub fn append_column_value(buffer, column, value)`：按 `(PhysicalType, ColumnValue)` 成对匹配并追加，错配时报 `column value type mismatch`。
- `pub fn account_column_value_memory_bytes(value) -> i64`：标量返回物理宽度，两个字节变体返回 `size_of::<Vec<u8>>() + len`。
- `pub fn write_column_batch(buffer, column) -> Result<Vec<ColumnValue>>`：选择一个类型化向量并复制/克隆成枚举批次；`Int96` 被拒绝。它不是 Go `writeColumnBatch` 的真实 Arrow writer 分派。
- `parseRawColumnValue`、`appendColumnValue`、`parseDecimalToScaledInteger`、`toFixedLenTwoComplement`：仅转发到 snake_case API 的 Go 风格公开别名；仓库搜索未发现别名的生产调用者。

## 执行流程

主写入路径按以下顺序运行：

1. `ParquetWriter::write` 校验 writer 状态并进入 `parse_and_append_row`；逐列的 `parse_column_value` 先处理输入层的真正 `None`，required 列收到 NULL 会直接失败。
2. 非 NULL 原始字节交给 `parse_raw_column_value`。函数先尝试 UTF-8；非 UTF-8文本分支会得到空字符串并按相应解析错误处理，而 `ByteArray`/非 DECIMAL `FixedLenByteArray` 直接 clone 原字节，不依赖文本有效性。
3. 物理类型决定基本表示；逻辑 DECIMAL 会先定标，逻辑 TIMESTAMP 会先解析到微秒，再按 `TimeUnit::{Millis,Micros,Nanos}` 换算。可空 TIMESTAMP 的任何日期时间解析错误被降级为 NULL；required TIMESTAMP 保留错误。
4. `append_parsed` 为 optional 列追加 definition level：NULL 为 0，非 NULL 为 1。非 NULL 值先计入 `buffered_memory_bytes`，再由 `append_column_value` 放入对应类型向量。
5. 达到 row-group 内存阈值后，`writer.rs` 的 flush 路径把这些向量真正交给 Arrow Parquet writer；本文件的 `write_column_batch` 不参与该生产 flush。

DECIMAL 路径先验证语法，再以 `i128` 计算整数部分乘数和小数部分；`Int32`/`Int64` 分支随后进行窄化检查，`FixedLenByteArray` 分支则检查声明宽度与有符号范围并生成定长二补码。普通定长字节列要求输入长度严格等于 `type_length`。

## 数据与状态

本文件本身无全局可变状态。`ColumnValue::Bytes` 与 `FixedBytes` 拥有各自的 `Vec<u8>`；`parse_raw_column_value` 对原始字节执行 `to_vec()`，避免数据库驱动或调用者复用输入缓冲后污染待写数据。标量按值保存。

`Column` 提供所有分派元数据：`physical` 决定枚举变体与缓冲字段，`logical` 区分普通整数、DECIMAL 和 TIMESTAMP，`scale` 控制定标，`type_length` 控制定长编码，`timestamp_unit` 控制时间戳单位，`allows_null_encoding` 决定非法时间能否降级为 NULL。

NULL 不作为 `ColumnValue` 变体保存：`Option<ColumnValue>` 与单独的 `is_null` 标志用于调用边，最终 definition level 存在 `ColumnBuffer.definition_levels`。因此调用者必须保持“NULL 没有值、非 NULL 有匹配类型值”的不变量。

内存统计是阈值用途的近似值，不是 allocator 精确占用：数值只计物理宽度；字节向量计一个 `Vec` 头加当前长度，不计容量富余、枚举 discriminant、缓冲向量本身的扩容或 allocator 对齐。

## 依赖与调用关系

上游生产调用关系：

- `pkg/dumpformat/parquetfile/writer.rs::ParquetWriter::parse_column_value` → `parse_raw_column_value`。
- `ParquetWriter::append_parsed` → `account_column_value_memory_bytes` → `append_column_value`。
- `dumpling/export/writer_util.rs::write_parquet_group` → `parse_raw_column_value`，随后直接把返回的枚举变体转换为 Arrow Parquet 类型并写列。
- `pkg/dumpformat/parquetfile/parser.rs::EstimateParquetReaderMemory` → `account_column_value_memory_bytes`，用于读取端估算。

下游依赖关系：

- `parse_raw_column_value` → `parse_decimal_to_scaled_integer`、`to_fixed_len_two_complement`、`parse_datetime_micros`。
- `parse_datetime_micros` → `days_in_month`、`days_from_civil`。
- `append_column_value` 和 `write_column_batch` → `column_buffer.rs::ColumnBuffer` 的七个类型化值向量。
- 所有失败经 `lib.rs::Error(String)` 和 `Result<T>` 返回；本文件不直接依赖 `parquet` crate API。

RustCodeGraph 索引将本文件识别为 26 个符号，并显示它被 `writer.rs`、`parser.rs`、`file_parser.rs`、`dumpling/export/writer_util.rs` 及测试等文件引用；精确调用边又由上述源码位置核对。`write_column_batch` 的仓库调用只出现在 Rust 测试辅助中，因此不能把它描述成生产序列化入口。

## 错误处理与边界

- DECIMAL：拒绝负 scale、空串、空符号、多个小数点、非数字和 `i128` 算术溢出；不裁剪空白。多余小数位向零截断，这是与 Go `big.Rat` 路径对齐的序列化语义，而不是 SQL domain/precision 验证。
- DECIMAL 的 Rust 表示限于 `i128`，固定宽度也限于 16 字节；Go 对照使用任意精度 `big.Int`，因此超大精度是明确兼容边界。
- 整数窄化分别通过 `i32::try_from` 和 `i64::try_from` 检查；浮点遵循 Rust `parse`，错误原样包装成字符串。
- 布尔仅接受代码列出的大小写变体及 `0/1`；其他文本报错。
- TIMESTAMP 要求空格分隔日期与时间，月份、实际月天数、时分秒均验证；小数秒不能为空。只取前六位生成微秒，Nanos 再 checked 乘 1000。解析失败仅在 `allows_null_encoding` 时变成 NULL。
- 对文本类型，`from_utf8(raw).unwrap_or("")` 会让非法 UTF-8表现为“空文本解析失败”，而不是专门的 UTF-8错误；字节数组路径仍完整保留任意字节。
- 非 DECIMAL 定长字节必须有正宽度且长度精确匹配；DECIMAL 定宽还拒绝零宽、超过 16 字节和有符号范围溢出。
- `PhysicalType::Int96` 在解析和批量重建两处均显式不支持。
- `append_column_value` 将物理类型/枚举错配转为可恢复错误；它不回滚此前已经追加的其他列。`writer_behavior_test.rs` 明确锁定逐列写入中途失败后会留下部分状态的 Go 对齐行为。

## 并发与资源生命周期

所有函数都是同步函数，不创建线程、异步任务、锁、通道、文件句柄或网络资源。纯转换辅助只使用栈上局部值和新分配的 `Vec`；所有权返回给调用者，错误时由 Rust 自动释放临时分配。

`append_column_value` 通过 `&mut ColumnBuffer` 要求独占可变访问；并发控制由拥有 `ParquetWriter`/缓冲的上层负责。`account_column_value_memory_bytes` 和 `write_column_batch` 仅借用输入，后者对数值复制、对字节向量 clone，所以返回批次不借用缓冲。

资源生命周期的重要边界在调用方：`ParquetWriter` 累积值直到 row group flush，optional 列的 definition level 与非 NULL 值向量必须在同一生命周期内保持一致；flush 后由 writer 清空缓冲。本文件只提供值级转换和追加，不负责事务式回滚或关闭 Arrow column writer。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dumpformat/parquetfile/column_value.go`，Rust 测试还与 `column_value_conversion_test.go` 的边界用例逐项呼应。

- `parse_raw_column_value` 对应 Go `parseRawColumnValue`：物理类型分派、可空非法时间转 NULL、字节 clone、DECIMAL 窄化和定宽校验基本一致。
- Rust 的日期时间辅助复现 Go `time.Parse(time.DateTime, s)` 所需的无时区“as-if UTC”数值语义；Rust明确验证日历并截取到微秒，再按目标单位换算。
- `parse_decimal_to_scaled_integer` 对应 Go `parseDecimalToScaledInteger` 的乘 `10^scale` 后向零取整；差异是 Rust 使用 `i128` 而 Go 使用任意精度 `big.Rat`/`big.Int`。
- `to_fixed_len_two_complement` 对应 Go 同名函数；Rust利用 `i128::to_be_bytes()`，因此宽度上限为 16，Go 可处理更宽的大整数。
- `append_column_value` 将 Go 的运行时类型断言改为显式枚举匹配，错配返回 `Error` 而不是 panic。
- 内存估算保持“头部 + 数据长度”形状，但 Rust使用 `size_of::<Vec<u8>>()`，Go使用 slice header；它们是各自运行时的近似成本，并不承诺数值跨语言完全相同。
- Go `writeColumnBatch` 接收 `file.ColumnChunkWriter` 并实际调用 `WriteBatch`；Rust `write_column_batch` 只把 `ColumnBuffer` 重建为枚举向量。真实 Rust Parquet 写出在 `writer.rs` 和 `dumpling/export/writer_util.rs` 中另行分派，这是当前迁移结构上的显著差异。
- 四个 CamelCase Rust 函数只是 Go 命名兼容别名，不增加行为。

## 扩展指南

新增物理类型时，至少同步修改 `ColumnValue`、`parse_raw_column_value`、`append_column_value`、`account_column_value_memory_bytes`、`write_column_batch`、`ColumnBuffer`，并检查 `writer.rs` 与 `dumpling/export/writer_util.rs` 的真实 Arrow writer 分派；遗漏任何一处都会造成解析、内存阈值或写出行为不一致。对应测试应放在独立的 `column_value_conversion_test.rs`/`writer_behavior_test.rs`，不要内嵌到源文件。

扩展 DECIMAL 时应先决定是否解除 `i128`/16 字节限制，并与 Go 任意精度语义、`column_type.rs` 的 precision-to-physical-type 映射及 Arrow fixed byte array 要求共同验证。不能仅扩大 `to_fixed_len_two_complement` 的宽度而保留 `i128` 解析器。

扩展 TIMESTAMP 格式或单位时，应集中修改 `parse_datetime_micros` 与 `parse_raw_column_value` 的单位换算，同时保留 `adjusted_to_utc=false` 的本地语义约定、可空非法时间降级和溢出检查。新增日期边界、闰年、fraction、Millis/Micros/Nanos 用例，并与 Go `time.Parse` 结果核对。

若要让 `write_column_batch` 真正承担 Arrow 写入，需改变其签名并迁移/复用现有生产分派，同时明确 definition levels、column writer close 和错误上下文；当前函数名容易产生误解，不应在未接线前宣称等价于 Go。任何修改还应检查逐列失败后部分缓冲保留这一现有兼容行为及性能影响。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/dumpformat/parquetfile` 找到本文件及相邻实现/测试；`node --file .../column_value.rs` 读取 632 行源码并列出被 16 个文件使用；`query` 定位了核心函数及测试符号。`callers`/`callees` 命令未返回可用明细，因此调用边又以精确仓库搜索和源码位置核验，未据此猜测。
- 实现与 crate 边界：`pkg/dumpformat/parquetfile/column_value.rs`、`column_buffer.rs`、`column_type.rs`、`lib.rs`、`Cargo.toml`。
- 生产调用证据：`pkg/dumpformat/parquetfile/writer.rs` 的 `parse_column_value`/`append_parsed`，`dumpling/export/writer_util.rs::write_parquet_group`，`pkg/dumpformat/parquetfile/parser.rs::EstimateParquetReaderMemory`。
- Go 对照：`pkg/dumpformat/parquetfile/column_value.go`、`writer.go`、`column_value_conversion_test.go`。
- Rust 独立测试：`column_value_test.rs`、`column_value_conversion_test.rs`、`writer_behavior_test.rs`、`writer_test_helpers_test.rs`、`parser_test.rs`；覆盖空白 DECIMAL、截断、二补码边界、所有支持物理类型、字节所有权、非法/可空时间、Int96、内存估算及缓冲分派。
- 本任务是只读分析加文档，不运行 Cargo；完成条件采用任务文件指定的 11 章节结构检查，并人工核对本文没有把未接线的 `write_column_batch` 描述为真实 Parquet 写出。

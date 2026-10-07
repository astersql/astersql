# `pkg/dumpformat/parquetfile/type_converter.rs`

## 文件定位

本文件属于 `astersql-dumpformat-parquetfile` crate（`pkg/dumpformat/parquetfile/Cargo.toml` 的 `[lib] path = "lib.rs"`），位于 Parquet 物理列解码和导入行表示之间。crate 根模块 `pkg/dumpformat/parquetfile/lib.rs` 以 `pub mod type_converter` 暴露它；真实文件读取链由 `pkg/dumpformat/parquetfile/file_parser.rs` 的 `Column::next` 按物理类型调用 `convert_logical_int32`、`convert_logical_int64`、`int96_micros_with_rebase` 或 `convert_logical_bytes`，再把本文件的 `Datum` 交给 `FileParser`/`ImportParser`。

文件包含两组接口。`ConvertedInfo` 与 `convert_int32`、`convert_int64`、`convert_int96`、`convert_bytes` 是较早的 `parser::ConvertedType` 兼容层，现有调用证据主要来自 `parser_test.rs` 和 `type_converter_test.rs`；`ParquetColumnType` 与三个 `convert_logical_*` 函数保留 Arrow Parquet 的原生 `LogicalType`，是 `file_parser.rs` 当前生产读取路径。第 15–220 行整段均为行注释中的历史移植草稿，不参与编译；当前实现从第 221 行开始，不能把草稿中的 TiDB `types::Datum` setter 当成 Rust 运行时行为。

## 核心职责

- 将 Parquet `INT32`、`INT64`、`INT96`、`BYTE_ARRAY` 和 `FIXED_LEN_BYTE_ARRAY` 的物理值解释为本地 `Datum`，并保留有符号/无符号整数、DECIMAL 定标、日期、时间、时间戳和原始字节的语义。
- 在 DECIMAL 路径中解析大端二补码字节或给整数插入小数点，统一产出 `Datum::Decimal(String)`；`decimal_bytes_to_string` 不依赖固定宽度整数，因此可处理长字节序列。
- 在时间路径中执行微秒归一化、最近微秒舍入、解析器时区调整，以及由 `SparkRebaseMicrosLookup` 决定的 Spark legacy Julian/Gregorian rebasing。
- 对原生 Parquet logical annotation 做白名单分派；无法由给定物理类型安全表达的 logical type 返回 `unsupported_logical` 错误。物理类型与 logical type 的前置组合校验主要由相邻的 `file_parser::validate_parquet_logical_type` 完成，本文件仍承担运行时兜底。

本文件不负责读取页、definition level/null 判定、schema 校验、row group 生命周期或最终导入格式化；这些分别由 `file_parser.rs` 的列读取与验证逻辑承担。尤其 `Datum::Null` 由 `Column::next` 根据 definition level 产生，而不是由任一转换函数产生。

## 主要符号

- 常量 `JULIAN_DAY_OF_UNIX_EPOCH = 2_440_588` 与 `MICROS_PER_DAY = 86_400_000_000` 定义 INT96/日期换算基准。`MAXIMUM_DECIMAL_BYTES = 33` 保留 Go 直接写入 `MyDecimal` 的阈值语义，但当前可编译的字符串 DECIMAL 实现没有读取它。
- `Datum` 是转换后的值载体：`Null`、`UInt`、`Int`、`Decimal`、`TimeMicros`、`DurationMicros`、`Float32`、`Float64` 和 `Bytes`。其中 `TimeMicros` 同时承载日期与时间戳；`DurationMicros` 专门承载 Parquet TIME，从而保留舍入到 `24:00:00` 的进位。
- `ConvertedInfo { converted, scale, adjusted_to_utc, timezone_offset_seconds, spark_rebase }` 为旧兼容层携带 `parser::ConvertedType` 元数据；`ParquetColumnType { logical, spark_rebase }` 则由 `FileParser` 从列描述符和 Spark 文件元数据构建，时区直接作为参数传入 `convert_logical_int32`/`convert_logical_int64`。
- `magnitude_to_decimal` 将无符号 base-256 大端字节反复除以 10；`decimal_bytes_to_string` 先把二补码负数取反加一，再定标并恢复符号；`set_datum_from_decimal_bytes` 包装为 `Datum::Decimal`。`scaled_decimal` 对 `i64` 做同类定标，并借助 `i128::abs` 安全覆盖 `i64::MIN`。
- `convert_int32`、`convert_int64`、`convert_int96`、`convert_bytes` 是旧元数据分派接口；`setDatumFromDecimalByte`、`getStringFromParquetByte`、`newInt96`、`int96ToUnixMicros` 是保留 Go 命名的公开别名。
- `new_int96`、`int96_to_unix_micros`、`int96_to_unix_micros_rounded` 与 `int96_micros_with_rebase` 负责 INT96 的 8 字节日内纳秒加 4 字节儒略日编码、解码、微秒舍入和可选 rebase。`new_int96` 用 `div_euclid`/`rem_euclid` 正确拆分 Unix epoch 之前的负时间戳。
- `unsupported_logical` 统一构造错误，并把 Arrow `LogicalType::Unknown` 按 Go 的 Null logical type 命名为 `Null`。`time_as_duration` 校验 TIME 值属于一天，按 MILLIS/MICROS/NANOS 换算；UTC-adjusted 值根据 `chrono_tz::Tz` 变成本地墙钟时间。
- `convert_logical_int32` 支持 DECIMAL、DATE、TIME、signed/unsigned INTEGER 和无 annotation；`convert_logical_int64` 另支持 TIMESTAMP 的三种单位；`convert_logical_bytes` 支持 DECIMAL、无 annotation、BSON/JSON/String/Enum，以及仅限 fixed-length 的 UUID。

本文件没有 trait、`impl` 块或条件编译项；所有状态通过值或共享引用显式传入。

## 执行流程

1. `FileParser` 在打开 Parquet 文件时从 column descriptor 取得归一化 `LogicalType`，按 DATE/TIMESTAMP/INT96 和 Spark metadata 决定是否创建 `SparkRebaseMicrosLookup`，然后为每列保存 `ParquetColumnType`（`file_parser.rs` 的 `infos`/`Column::info`）。
2. `Column::next` 批量读取物理值并先处理 null definition level；非 null 值根据 Arrow `ColumnReader` 变体分派。布尔和浮点在调用处直接构造 `Datum`，整数和字节进入本文件的原生 logical 转换函数，INT96 进入 `int96_micros_with_rebase`。
3. `convert_logical_int32`：DECIMAL 调 `scaled_decimal`；DATE 可先按 Spark legacy 表重置日期，再乘 `MICROS_PER_DAY`；TIME 调 `time_as_duration`；无符号整数先把 `i32` 位模式重解释为 `u32` 后扩为 `u64`，避免符号扩展；其余允许的整数保持 `Int`。
4. `convert_logical_int64`：整数和 DECIMAL 类似；TIME 复用 `time_as_duration`；TIMESTAMP 按单位求整秒与余数。MILLIS/MICROS 可先 rebase，NANOS 明确跳过 rebase；纳秒使用 `(nanos + 500) / 1000` 舍入到最近微秒。若 `is_adjusted_to_u_t_c` 为真，则按“已舍入后的瞬间”查询地点偏移，因此跨 DST 边界时使用正确一侧的 offset。
5. `convert_logical_bytes`：DECIMAL 读取大端二补码并定标；允许的文本/二进制 annotation 复制为独立 `Vec<u8>`；UUID 只接受 fixed-length 路径。其他 annotation 统一报错。
6. INT96 的主链先从 little-endian 字段求 Unix 微秒。有 rebase 表时以截断微秒执行 `lookup.rebase`；无 rebase 时保留亚微秒的最近舍入。`Column::next` 随后为 INT96 维持历史 UTC 语义并应用解析器 location offset。

旧接口遵循相同的大方向，但使用 `ConvertedInfo` 中预先计算的固定秒偏移，且 `convert_int64` 对不属于 DECIMAL/TIME/TIMESTAMP/None 的分支回退到 `UInt`。新增生产行为应优先接入原生 `LogicalType` 路径，不能只修改旧接口及其单元测试。

## 数据与状态

转换器本身无全局可变状态。`Datum`、`ConvertedInfo` 和 `ParquetColumnType` 均为拥有型数据；转换函数按值接收标量、按共享引用读取列元数据。`SparkRebaseMicrosLookup` 被存放在 `Option` 中并以 `as_ref()` 借用，`int96_micros_with_rebase` 的注释明确要求复用缓存表，不为每个值重新分配时区字符串。

DECIMAL 字节转换会复制输入到局部 `Vec<u8>`，负数的取反加一和逐次除法只修改该副本，因此不会破坏 Arrow dictionary/shared buffer。最终字符串的小数位数由 scale 决定：位数不足时补前导零，scale 大于零时必插入小数点；负零不会加负号。旧整数 `scaled_decimal` 把负 scale 视为 0，而 `decimal_bytes_to_string` 对负 scale 返回错误，这一接口差异应在调用新入口时保留或显式解决。

TIME 的合法原始范围是 `[0, 一天对应的单位数)`；纳秒舍入可能精确进位到 `DurationMicros(MICROS_PER_DAY)`，该值表示 `24:00:00`，不能错误取模为午夜。UTC-adjusted TIME 在本地墙钟换算时会对一天取模。TIMESTAMP 使用 Euclidean 除法处理 epoch 之前的负值；乘法、微秒合成和部分 rebase 路径使用 checked arithmetic。

## 依赖与调用关系

上游生产调用关系由 `pkg/dumpformat/parquetfile/file_parser.rs` 直接证明：`Column::next` 调用 `convert_logical_int32`、`convert_logical_int64`、`convert_logical_bytes` 和 `int96_micros_with_rebase`；同文件构建 `ParquetColumnType`，并在 INT96 返回后追加 location offset。RustCodeGraph 的文件节点也报告本文件被 `file_parser.rs`、`parser_test.rs` 和 `pkg/dxf/importinto/conflict_resolution_test.rs` 使用。

下游依赖如下：

- `crate::parser::ConvertedType` 仅服务旧兼容层的逻辑类型枚举。
- `crate::spark_rebase::{SparkRebaseMicrosLookup, rebase_julian_to_gregorian_days}` 提供时间戳表驱动 rebase 与 DATE 的历法换算；`lookup.rebase` 的错误通过本 crate 的 `Result` 原样传播。
- `crate::{Error, Result}` 是 `lib.rs` 定义的字符串错误包装和统一结果别名。
- `parquet::basic::{LogicalType, TimeUnit}` 提供当前主链的无损 annotation；`chrono` 与 `chrono-tz` 计算真实时区（包括 DST）的 offset。
- `Cargo.toml` 声明直接依赖 `chrono = 0.4`、`chrono-tz = 0.10` 和带 tag 的 `astersql/arrow-rs` Parquet fork；本文件不直接使用 crate 的 `bytes` 或 `astersql-lightning-mydump` 依赖，后者在 `ImportParser` 适配层消费转换结果。

RustCodeGraph 对 `convert_logical_*` 的 callers/callees 精确查询在本次分析中超时且未输出边，因此调用关系没有据此推断，而是由索引文件使用列表和上述源码调用点交叉核验。

## 错误处理与边界

- `decimal_bytes_to_string` 拒绝空数组和负 scale；二补码最小值、任意长度、前导符号扩展及负零均由复制后的 magnitude 流程处理。`magnitude_to_decimal` 最终 `String::from_utf8(...).unwrap()` 只消费函数自身生成的 ASCII 数字，故该 unwrap 的 UTF-8 前提由局部不变量保证。
- `time_as_duration` 拒绝负 TIME 和达到/超过一天的值；时区查询不是唯一映射或超出 chrono 范围时返回 `timestamp out of range`。合法纳秒在乘法范围内，因为上界小于一天纳秒数。
- `convert_logical_int64` 对 TIMESTAMP_MILLIS rebase 前的 `* 1000`、秒到微秒以及余数合并做溢出检查，并返回 `timestamp overflow`；但 UTC offset 的最终加法和旧接口中的部分乘加不是 checked arithmetic，极端接近 `i64` 边界的输入存在溢出风险，扩展时应补充边界策略而非假定所有路径均已防护。
- 不支持的 annotation 由 `unsupported_logical` 统一失败。`Unknown`/Null 的 null 行在 `Column::next` 转换前成功返回 `Datum::Null`，但同列的非 null 行必须报 `unsupported parquet logical type Null`，对应 `parser_test.rs` 的逐物理类型回归。
- `convert_logical_bytes` 的 UUID 必须来自 fixed-length array；其余 UUID 或不在白名单内的 logical type报错。更早的 schema/物理组合错误由 `validate_parquet_logical_type` 报 `not applicable`，二者是互补防线。
- INT96 解码按格式读取 little-endian 字段；接口固定接收 `[u8; 12]`，长度不合法无法进入函数。无 rebase 路径根据日内纳秒余数 `>= 500` 向上舍入，可能跨日；有 rebase 路径先截到微秒以匹配 lookup 表粒度。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务、文件句柄或网络资源，函数都是同步纯计算（除局部分配）。它可被并发调用，前提是调用方提供的 `SparkRebaseMicrosLookup` 自身满足共享引用读取契约；这里没有内部可变共享状态。

每个 DECIMAL byte value 至少复制一次并分配十进制字符串；`magnitude_to_decimal` 的逐十进制位除法会随输入字节数和十进制位数增长，适合正确性路径但应关注超大 precision 的 CPU/分配成本。`convert_logical_bytes` 对普通字节也通过 `to_vec` 取得所有权，避免引用寿命越过 Arrow reader/dictionary buffer。时区对象 `chrono_tz::Tz` 是可复制值，rebase lookup 按引用复用；真正的批次缓存和 row 生命周期由 `Column.rows`/`FileParser` 管理。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/dumpformat/parquetfile/type_converter.go`。两端都按 Parquet 物理类型选择 setter/转换器，并覆盖 DECIMAL、DATE、TIME、TIMESTAMP、INT96、整数、浮点与字节；Rust 的 `file_parser.rs` 当前原生 logical 主链对应 Go 的 `getInt32Setter`、`getInt64Setter`、`getInt96Setter`、`getByteArraySetter` 和 `getFixedLenByteArraySetter`。

关键一致性包括：unsigned INT32 先按 `uint32` 重解释；TIME 限定在一天内且纳秒最近舍入可保留 `24:00:00`；TIMESTAMP 仅在 adjusted-to-UTC 时转换到 parser location；DATE/TIMESTAMP/INT96 按 Spark metadata 选择 rebase；INT96 无 rebase 时保留纳秒直到 TiDB 的微秒舍入语义；BYTE_ARRAY DECIMAL 不原地破坏 dictionary buffer。

表示层并非逐字复刻：Go 直接复用/写入 `types.Datum` 和 `types.MyDecimal`，超长 DECIMAL 才回退到字符串；Rust 当前统一产生轻量 `Datum::Decimal(String)`，因此 `MAXIMUM_DECIMAL_BYTES` 在可编译逻辑中未使用。Go setter 在工厂阶段返回闭包并可复用 DECIMAL 缓冲，Rust 由普通函数逐值返回拥有型 `Datum`。Rust 的 `ParquetColumnType` 保留 Arrow 原生 logical annotation，避免旧 `ConvertedType` 桥接丢失 signedness、NANOS、UUID 等信息。

验证语义的 Go 测试集中在 `parser_test.go`：`timestamp_and_decimal`、`int96_rounds_sub_microsecond_precision`、Spark legacy rebase、`decimal_with_nulls`、`dictionary_encoded_decimal`、`TestBinaryToDecimalStr` 等。Rust 对应证据在独立 `type_converter_test.rs` 的 INT96 舍入用例及 `parser_test.rs` 的 DECIMAL、DATE rebase、负 epoch INT96、TIME/UTC/DST、logical signedness/UUID/Null 等回归。两端数据表示不同，比较时应以导入后的 SQL/文本语义和错误边界为准，不能仅比较内部 enum 形状。

## 扩展指南

- 新增 logical annotation 或物理组合时，先在 `file_parser::validate_parquet_logical_type` 定义合法组合，再扩展对应的 `convert_logical_int32`、`convert_logical_int64` 或 `convert_logical_bytes`；若要维持旧公开接口兼容，再同步 `ConvertedInfo` 分支。不要只在注释草稿或 Go 风格别名中接线。
- 修改 DECIMAL 时同时覆盖正负二补码、零/负零、前导 `00`/`ff`、scale 为 0/大于位数/非法负数、`i64::MIN`、超长输入以及 dictionary 重用。相关 Rust 测试应放在独立的 `type_converter_test.rs` 或现有 `parser_test.rs`，不要嵌回源文件；Go 行为基线是 `parser_test.go::TestBinaryToDecimalStr` 和 dictionary decimal 用例。
- 修改 TIME/TIMESTAMP 时必须区分 wall-clock TIME、instant TIMESTAMP 和 INT96 历史 UTC 语义，并覆盖 epoch 前负值、半微秒边界、午夜进位、非整点 offset、DST 跳变及 `i64` 溢出。NANOS timestamp 当前故意跳过 Spark microsecond rebase；改变该规则需要同时验证 Go 的 `rebaseTimestampValue`。
- 新增 `Datum` 变体需要同步 `file_parser.rs` 到 `astersql_lightning_mydump::Datum` 的适配和所有消费方。性能改动应保留输入 buffer 不被修改的契约，并用现有 `benchmark_decimal_test.rs`/Go benchmark 对照分配与吞吐。
- 对错误文本的改动要同步依赖 substring 的回归测试，特别是 `unsupported parquet logical type Null`、`outside the valid range`、`timestamp overflow` 和 `timestamp out of range`。如果需要支持当前被拒绝的 annotation，应先确认 Arrow physical encoding 和 Go setter 的既有行为，不能把兜底错误静默改成原始字节。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 7,032 个 Rust 文件且覆盖 `pkg/dumpformat/parquetfile`；`files --filter pkg/dumpformat/parquetfile` 列出目标、Go 对照和测试；`node --file pkg/dumpformat/parquetfile/type_converter.rs` 读取完整 620 行并报告使用文件。精确 `query` 找到 `type_converter.rs::convert_int32` 与 `time_as_duration`；`TypeConverter`/`create_type_converter`/`convert_parquet_value` 无符号，说明本文件不是以转换器对象工厂命名。`callers`/`callees` 查询超时无输出，未作为正向调用边证据。
- 源码：完整读取 `pkg/dumpformat/parquetfile/type_converter.rs`；读取 `pkg/dumpformat/parquetfile/file_parser.rs` 的 `Column::next`、`FileParser` 列信息构建和 Spark rebase 接线；读取 `pkg/dumpformat/parquetfile/lib.rs` 的模块声明、错误类型和独立测试装配。目标目录不存在 `doc.go`。
- crate 边界：读取 `pkg/dumpformat/parquetfile/Cargo.toml`，确认 crate 名、lib 入口、chrono/chrono-tz/Parquet fork 依赖及 Go package porting metadata。
- Go 对照：完整读取 `pkg/dumpformat/parquetfile/type_converter.go`，并抽查 `pkg/dumpformat/parquetfile/parser_test.go` 中 timestamp/decimal、INT96 舍入、dictionary decimal 和 binary-to-decimal 用例。
- Rust 测试：读取 `pkg/dumpformat/parquetfile/type_converter_test.rs`；读取 `pkg/dumpformat/parquetfile/parser_test.rs` 中旧兼容转换、logical TIME/TIMESTAMP/INT96、signedness、UUID、Null、schema validation 和 DST 舍入用例。由于任务是纯文档分析，按计划未运行 Cargo。
- 结构校验使用任务指定命令，要求目标文档存在且恰有本页所示 11 个固定二级标题；交付前另以 `git diff --check` 和限定路径状态检查文档质量及唯一产物。

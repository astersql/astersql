# `pkg/dumpformat/parquetfile/column_type.rs`

## 文件定位

本文件是 `astersql-dumpformat-parquetfile` crate 的 SQL 列元数据到 Parquet 列描述的类型边界。crate 根模块 [`lib.rs`](./lib.rs) 以 `pub mod column_type` 暴露本模块，并重新导出 `Column`、`ColumnInfo`、`ColumnType`、`LogicalType`、`PhysicalType`、`TimeUnit`。生产写出链为 `writer::ParquetWriter::new` → `schema_builder::build_parquet_schema_from_columns` → `to_column_type`：前者接收查询结果的 `ColumnInfo`，本文件决定 Parquet physical/logical type，后两层再构造 schema、列缓冲和实际 Arrow Parquet writer。

[`Cargo.toml`](./Cargo.toml) 将本目录定义为独立 crate `astersql-dumpformat-parquetfile`，入口是 `lib.rs`；实际 Arrow `parquet` 类型是在 [`writer.rs`](./writer.rs) 中从本文件的中间表示转换而来。本文件自身只使用标准库，没有直接调用 Cargo 中的 `parquet` Git 依赖。

## 核心职责

- 用 `ColumnInfo` 承载来自 `database/sql`/driver 的列名、类型名、可空性、精度与小数位，并将类型名规范化后交给 `to_column_type` 分派。
- 用 `PhysicalType`、`LogicalType` 和 `ColumnType` 表达独立于 Arrow API 的 Parquet schema 中间表示，保留物理类型、逻辑注解、定长宽度和 DECIMAL 元数据。
- 对 DECIMAL 选择 `INT32`、`INT64` 或 `FIXED_LEN_BYTE_ARRAY`，并计算定长字节宽度；对不适合跨引擎 DECIMAL 的精度降级为 UTF-8 字符串。
- 用 `Column` 把原始元数据、映射结果、空值编码策略和时间单位组合起来，供 schema、缓冲、值转换和写出代码共同消费。注意：`allows_null_encoding` 与 `timestamp_unit` 不是本文件计算的，而由 [`schema_builder.rs`](./schema_builder.rs) 填充。
- 保留 `toColumnType`、`decimalColumnType`、`decimalFixedLengthBytesForPrecision` 三个 Go 风格别名；它们仅转发到 snake_case 实现，没有第二套逻辑。

## 主要符号

- `PhysicalType`：Parquet 物理类型枚举，包含 `Boolean`、`Int32`、`Int64`、`Float`、`Double`、`Int96`、`ByteArray`、`FixedLenByteArray`。`to_column_type` 当前只会产生其中的 `Int32`、`Int64`、`Double`、`ByteArray`、`FixedLenByteArray`；其余变体供 crate 的读取、手工 schema 或测试路径使用。
- `TimeUnit`：`Millis`、`Micros`、`Nanos` 三种单位。当前 SQL `TIMESTAMP`/`DATETIME` 映射固定生成 `Micros`。
- `LogicalType`：`None`、`String`、`Decimal { precision, scale }`、`Timestamp { adjusted_to_utc, unit }`、`Date`、`Time { adjusted_to_utc, unit }`。本文件的类型名映射当前生成 `None`、`String`、`Decimal`、`Timestamp`；`Date`/`Time` 变体由其他路径消费，不应误认为 `DATE`/`TIME` 类型名会映射到它们——现有 switch 将二者按字符串写出。
- `ColumnType`：完整映射结果。`type_length` 仅对 `FixedLenByteArray` 有实际宽度意义，普通类型使用 `-1`；`precision`、`scale` 仅对 DECIMAL 映射有业务意义，普通类型为 `0`。
- `ColumnInfo`：映射输入。`database_type_name` 是决策字段；`precision`、`scale` 仅在精确类型名为 `DECIMAL` 时参与本文件的映射；`name`、`nullable` 留给下游 schema 构造。
- `Column`：运行时列描述，组合 `ColumnInfo`、`ColumnType`、`allows_null_encoding` 和 `timestamp_unit`。
- `plain(physical)`：内部构造器，统一产生 `LogicalType::None`、`type_length = -1`、`precision = scale = 0` 的简单类型。
- `to_column_type(info)`：公开主入口，执行类型名清理、分支映射和未知类型兜底。
- `decimal_column_type(info)`：DECIMAL 专用入口，按精度选择物理表示并保留 scale。
- `decimal_fixed_length_bytes_for_precision(precision)`：按 `ceil((precision * log2(10) + 1) / 8)` 计算有符号二进制定长表示所需字节数，非正精度返回 `-1`。

## 执行流程

1. `schema_builder::build_parquet_schema_from_columns` 先调用 `validate_column_info` 校验列名和有效 DECIMAL 的 scale，再逐列调用 `to_column_type`。
2. `to_column_type` 对 `database_type_name` 执行 `trim()` 与 ASCII 大写转换。因此首尾空白和 ASCII 大小写不影响匹配，但诸如 `TIMESTAMP(6)` 并不会被剥离精度后缀；代码依赖 driver 按约定返回基础类型名。
3. 字符、文本以及 `DATE`、`TIME`、`SET`、`JSON`、`ENUM`、`NULL`、`GEOMETRY` 映射为 `ByteArray + String`；二进制/BLOB/BIT 映射为无逻辑注解的 `ByteArray`。
4. `TIMESTAMP`、`DATETIME` 映射为 `Int64 + Timestamp(adjusted_to_utc=false, Micros)`，输入的 precision/scale 不改变时间单位。整数族分为 `Int32` 和 `Int64`；`FLOAT` 与 `DOUBLE` 都写成 `Double`。
5. `DECIMAL` 转入 `decimal_column_type`：精度 `1..=9` 用 `Int32`，`10..=18` 用 `Int64`，`19..=38` 用按公式计算宽度的 `FixedLenByteArray`；精度 `<=0` 或 `>38` 改写为 `ByteArray + String`。
6. `UNSIGNED BIGINT` 不走通用 DECIMAL 分支，而固定映射为 9 字节的 `DECIMAL(20,0)`，以覆盖无符号 64 位整数的最大 20 位十进制范围。
7. 所有未列出的名称（包括 `NUMERIC`、`REAL`、`BOOL`、`INTEGER` 等 driver 不应报告的别名）均进入无逻辑注解 `ByteArray` 兜底。下游 `schema_builder` 将结果组装为 `Column`，`writer::parquet_schema` 再翻译成 Arrow Parquet 类型。

## 数据与状态

所有类型都是普通值对象：枚举和 `ColumnType` 可复制或克隆，`ColumnInfo`/`Column` 持有自有 `String` 和嵌套值，不借用外部缓冲。映射函数只读取 `&ColumnInfo` 并返回新的 `ColumnType`，没有全局变量、缓存、内部可变状态或 I/O。

关键不变量由构造约定和下游校验共同维持：普通类型的 `type_length` 为 `-1`；有效 DECIMAL 的逻辑注解与顶层 `precision`/`scale` 相同；`FixedLenByteArray` 必须有正宽度。最后一项由 `schema_builder::new_primitive_node` 再次检查。`ColumnInfo::nullable` 不影响类型映射；下游以它决定 required/optional，并对 timestamp 额外允许 NULL 编码以容纳非法 MySQL 时间值。

## 依赖与调用关系

RustCodeGraph 对精确符号的结果显示：`to_column_type` 的直接生产调用者是 [`schema_builder.rs`](./schema_builder.rs) 中的 `build_parquet_schema_from_columns`，直接测试调用者是 [`column_type_mapping_test.rs`](./column_type_mapping_test.rs)；`to_column_type` 下调 `plain` 和 `decimal_column_type`，后者再下调 `decimal_fixed_length_bytes_for_precision`。三个 Go 风格别名也分别调用对应 snake_case 函数。

下游数据依赖包括：`schema_builder.rs` 用 `ColumnType` 生成 schema 与 `Column`；[`column_buffer.rs`](./column_buffer.rs) 按 `PhysicalType` 分配强类型缓冲；[`column_value.rs`](./column_value.rs) 根据列描述解析输入值；`writer.rs` 把所有枚举映射为 Arrow `parquet::basic` 类型和逻辑注解。`lib.rs` 的公开再导出使 dumpling 等上层 crate 可构造 `ColumnInfo`，但类型映射仍通过 writer/schema_builder 链发生。

## 错误处理与边界

本文件的三个核心函数均返回普通值，不返回 `Result`、不 panic。未知类型不是错误，而是无逻辑注解的 `ByteArray`，这是面向未来 driver 类型名的兼容策略；无效或超过 38 位的 `DECIMAL` 也不是错误，而是 UTF-8 字符串。两者区别重要：只有精确的 `DECIMAL` 名称享受字符串逻辑注解，`NUMERIC` 等别名仍走普通二进制兜底。

DECIMAL scale 合法性不在 `decimal_column_type` 内检查；生产入口 `schema_builder::validate_column_info` 只在 precision 为 `1..=38` 时拒绝 `scale < 0` 或 `scale > precision`。直接调用公开的 `decimal_column_type` 可以构造语义异常的组合，因此新增调用者应优先走 schema builder，或自行执行同等校验。`decimal_fixed_length_bytes_for_precision` 对非正数返回 `-1`；该哨兵不能作为 `FixedLenByteArray` 的有效宽度。

类型名匹配仅做 ASCII 大写，不解析 SQL 声明文本。若调用者传入 `DECIMAL(10,2)`、`UNSIGNED MEDIUMINT` 或非 ASCII 别名，会按未知类型处理；这与注释中“输入来自 driver 的 `DatabaseTypeName()`”前提一致。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件句柄或网络连接。函数是确定性的只读变换，相同 `ColumnInfo` 会得到相同结果，可由多个线程并行调用；类型本身是否跨线程使用取决于所有字段的标准 `Send`/`Sync` 自动实现，而本文件没有手写并发保证。

资源生命周期从调用者传入的 `&ColumnInfo` 开始，仅在函数调用期间借用；结果拥有自己的标量和枚举数据。较大的列值缓冲与 Parquet writer 生命周期分别由 `column_buffer.rs` 和 `writer.rs` 管理，不属于本文件职责。

## 与 Go 版本的对应关系

直接对照文件是 [`column_type.go`](./column_type.go)。Rust 的 `to_column_type`、`decimal_column_type`、`decimal_fixed_length_bytes_for_precision` 分别逐分支对应 Go 的 `toColumnType`、`decimalColumnType`、`decimalFixedLengthBytesForPrecision`；Rust 额外提供同名风格转发函数，便于移植期调用。核心语义一致：driver 类型名先 trim/大写；未知别名退回 `BYTE_ARRAY`；时间戳总是微秒且 `isAdjustedToUTC=false`；FLOAT 写 DOUBLE；无符号 BIGINT 写 9 字节 `DECIMAL(20,0)`；DECIMAL 使用 9/18/38 位边界及相同宽度公式。

表示层存在语言差异：Go 的 `columnType.Logical` 是 Arrow schema 接口值且可为 `nil`，Rust 用封闭的 `LogicalType::None` 枚举；Go 的 `ColumnInfo` 和 `columnType` 定义在 `writer.go`，Rust 将这些数据结构集中在本文件并由 `lib.rs` 公开再导出；Rust 先生成自有中间表示，再在 `writer::parquet_schema` 中转换为 Arrow Rust 类型。

独立回归测试 [`column_type_mapping_test.rs`](./column_type_mapping_test.rs) 与 Go 的 [`column_type_mapping_test.go`](./column_type_mapping_test.go) 对齐，覆盖常用类型、明确不支持的别名、未知类型、时间单位、`UNSIGNED BIGINT`、DECIMAL 9/18/19/38 位边界以及 precision 0/39 的兜底。Rust 测试还直接验证宽度函数在 precision 0 与 2 时分别返回 `-1` 和 `1`。

## 扩展指南

- 新增或调整 SQL 类型映射时，修改 `to_column_type` 的唯一 switch，并同步 `column_type_mapping_test.rs` 与 Go 对照实现/测试；先确认 driver 的真实 `DatabaseTypeName()`，不要仅凭 SQL 别名表扩大专门分支。
- 新增 `PhysicalType` 或 `LogicalType` 变体时，必须同步检查 `writer::parquet_schema`、`column_buffer::new_column_buffer`、列值转换和 parser 路径的穷尽匹配；仅在本文件加枚举会造成编译失败或读写语义缺失。
- 调整 DECIMAL 边界或宽度公式时，同时验证 `decimal_column_type`、`decimal_fixed_length_bytes_for_precision`、定长二进制值编码和跨引擎兼容性。风险包括溢出、符号位宽不足、schema precision/scale 不一致以及旧 reader 无法读取超过 38 位的 DECIMAL。
- 调整时间类型时，要同步 `schema_builder` 的强制可空策略、`Column.timestamp_unit`、`column_value.rs` 的编码及 writer 的 Arrow logical type；改变 `adjusted_to_utc` 或单位会造成兼容性和时区语义变化。
- 测试必须继续放在独立的 `column_type_mapping_test.rs`，并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入；不要把测试内嵌回生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/dumpformat/parquetfile` 确认源、Go 对照和独立测试；`explore` 及精确 `query/node` 确认 `to_column_type`（第 268 行）、`decimal_column_type`（第 313 行）、`decimal_fixed_length_bytes_for_precision`（第 345 行）的定义与调用链。精确 `callers/callees` 子命令未输出边记录，因此调用关系又由已索引的 `explore` 结果和直接引用搜索交叉核对。
- 已读生产与边界文件：[`column_type.rs`](./column_type.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`schema_builder.rs`](./schema_builder.rs)、[`writer.rs`](./writer.rs)、[`column_buffer.rs`](./column_buffer.rs) 的直接符号引用。
- 已读 Go 对照与测试：[`column_type.go`](./column_type.go)、[`column_type_mapping_test.go`](./column_type_mapping_test.go)、[`column_type_mapping_test.rs`](./column_type_mapping_test.rs)。
- 本任务只生成文档，按计划不运行 Cargo。结构验证要求文档存在且恰好包含本页列出的 11 个固定二级标题；交付前另行运行该命令并检查退出码。

# `pkg/dumpformat/parquetfile/schema_builder.rs`

## 文件定位

本文件位于 `astersql-dumpformat-parquetfile` crate 的 schema 构建层。crate 根模块 [`lib.rs`](./lib.rs) 通过 `pub mod schema_builder` 暴露它；crate 内生产写出入口 [`writer.rs`](./writer.rs) 中的 `ParquetWriter::new` 调用 `build_parquet_schema_from_columns`，把 SQL 驱动提供的 `ColumnInfo` 转换为写出阶段统一使用的 `Column`，并生成一份可检查的轻量 `Schema`。crate 外的 [`../../../dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs) 也由 `parquet_columns` 直接调用该函数，将 Dumpling 表元数据规范化为 Parquet 写出列。

这里的 `Schema`/`PrimitiveNode` 不是 parquet-rs 的 `parquet::schema::types::Type`。当前 `ParquetWriter::new` 丢弃返回元组中的轻量 `Schema`，保留 `Vec<Column>`，再由 `writer.rs::parquet_schema` 构造实际交给 `SerializedFileWriter` 的 parquet-rs schema。因此，本文件承担“校验并规范化列元数据”的边界职责，而真实文件 schema 的最终物化仍在 `writer.rs`。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义：库入口是 `lib.rs`，直接依赖带固定 tag 的 AsterSQL `arrow-rs` Parquet 分支，但本文件本身只使用 crate 内的 `column_type` 类型和统一 `Error`/`Result`；底层 Parquet 类型的实际构造由 writer 层完成。

## 核心职责

1. `validate_column_info` 在类型映射前拒绝空列名，并保护有效精度范围内的 DECIMAL scale，防止产生 `scale < 0` 或 `scale > precision` 的非法逻辑类型。
2. `build_parquet_schema_from_columns` 保持输入列顺序，逐列调用 `to_column_type`，构造同时服务 schema 与值编码的 `Column` 列表。
3. 对映射为 `LogicalType::Timestamp` 的 TIMESTAMP/DATETIME，即使源列声明非空，也设置 `allows_null_encoding = true`，使非法 MySQL 时间值可在后续写出时编码成 NULL。
4. `new_primitive_node` 把 `Column` 投影为叶子节点，并拒绝宽度非正的 `FixedLenByteArray`。
5. 文件保留三个 Go 风格公开别名，供移植代码以原命名调用；它们不增加新行为。

## 主要符号

- `Repetition::{Required, Optional}`：轻量叶子节点的重复度。`Optional` 表示允许 definition level 0；它由 `Column::allows_null_encoding` 决定，而不只是照抄 `ColumnInfo::nullable`。
- `PrimitiveNode { name, repetition, physical, logical, type_length }`：一列的轻量 schema 描述，包含列名、空值能力、物理/逻辑类型和定长字节宽度。
- `Schema { name, fields }`：轻量根节点。`build_parquet_schema_from_columns` 固定把根名设为 `"schema"`，字段顺序与输入切片一致。
- `validate_column_info(&ColumnInfo) -> Result<()>`：元数据预检入口。DECIMAL 名称先 `trim` 再做不区分 ASCII 大小写的比较；scale 校验只应用于 `precision` 在 `1..=38` 内的 DECIMAL。
- `build_parquet_schema_from_columns(&[ColumnInfo]) -> Result<(Schema, Vec<Column>)>`：本文件的主入口。成功结果中的两个向量按同一输入顺序一一对应。
- `new_primitive_node(&Column) -> Result<PrimitiveNode>`：叶子节点构造器；`FixedLenByteArray` 的 `type_length` 必须大于零。
- `buildParquetSchemaFromColumns`、`validateColumnInfo`、`newPrimitiveNode`：分别直接转发到上述 snake_case 实现的 Go 风格别名。

## 执行流程

`ParquetWriter::new` 把调用方提供的 `&[ColumnInfo]` 传给 `build_parquet_schema_from_columns`。主函数先按输入长度为 `fields` 和 `columns` 预分配容量，然后依次执行：

1. 调用 `validate_column_info`。任一列失败即用 `?` 立即返回，后续列不再处理，也不返回部分结果。
2. 调用 [`column_type.rs`](./column_type.rs) 的 `to_column_type`，根据经 trim/大写规范化的数据库类型名选出 `ColumnType`。
3. 计算空值编码能力：普通列沿用 `ColumnInfo::nullable`；任何映射为 `LogicalType::Timestamp` 的列都强制允许 NULL。因此 Rust 中映射到 timestamp 的 TIMESTAMP 和 DATETIME 都覆盖这条兼容路径。
4. 构造 `Column`。若逻辑类型是 `Timestamp` 或 `Time`，提取其中的时间单位；其他类型的 `timestamp_unit` 填入 `TimeUnit::Micros` 作为默认占位。
5. 调用 `new_primitive_node` 生成轻量叶子节点。该函数校验定长字节宽度，根据 `allows_null_encoding` 选择 `Optional`/`Required`，并复制列名及逻辑类型。
6. 同步把叶子节点和 `Column` 追加到各自向量。循环成功后返回根名固定为 `schema` 的 `Schema` 以及 `Vec<Column>`。

crate 内生产 writer 随后用 `Vec<Column>` 创建列缓冲，并在 `writer.rs::parquet_schema` 中再次把相同的物理类型、逻辑类型、重复度、长度、precision 和 scale 转成 parquet-rs `Type`。Dumpling 路径则由 `WriteInsertInParquet` 调用 `parquet_columns`，再交给其本地 `parquet_schema` 构造 parquet-rs schema。两条路径都只消费返回元组中的 `Vec<Column>`，因此扩展字段语义时必须保证本文件的轻量投影与两个 writer 的最终物化保持一致。

## 数据与状态

本文件没有全局可变状态。所有输出都由输入切片派生：`ColumnInfo` 被克隆进 `Column`，列名和逻辑类型又被克隆进 `PrimitiveNode`；调用结束后，结果不借用输入数据。

重要不变量如下：

- `Schema.fields.len() == columns.len() == infos.len()`，且三个序列顺序一致；发生错误时函数不返回部分值。
- 根节点名称恒为 `schema`。
- `PrimitiveNode::repetition` 与对应 `Column::allows_null_encoding` 一致。
- timestamp/time 的单位来自映射后的 `LogicalType`；非时间类型的 `Column::timestamp_unit` 为 `Micros`，但不表示该列具有时间语义。
- `FixedLenByteArray` 只有在 `type_length > 0` 时才能成为 `PrimitiveNode`。现有映射中，有效高精度 DECIMAL 和 `UNSIGNED BIGINT` 会产生正宽度。
- 精度不在 `1..=38` 的 DECIMAL 不进入 scale 拒绝分支；`column_type::decimal_column_type` 会把它降级为 UTF-8 `ByteArray`。独立测试覆盖了 precision 为 0 和 39、scale 为 -1 的降级行为。

## 依赖与调用关系

RustCodeGraph 的精确符号结果给出以下直接调用边：

- 上游生产调用一：`writer.rs::ParquetWriter::new` → `build_parquet_schema_from_columns`。该调用生成规范化 `Column`，供 `new_column_buffers`、`writer.rs::parquet_schema` 和后续行值解析共同使用。
- 上游生产调用二：`dumpling/export/writer_util.rs::WriteInsertInParquet` → `parquet_columns` → `build_parquet_schema_from_columns`。`parquet_columns` 将 `TableMeta::ColumnInfos` 转为该 crate 的 `ColumnInfo`，丢弃轻量 `Schema`，把 `Vec<Column>` 交给 Dumpling 本地的 `parquet_schema` 和行写出逻辑。
- 主入口下游：`build_parquet_schema_from_columns` → `validate_column_info`、`column_type::to_column_type`、`new_primitive_node`，并实例化 `Column` 与 `Schema`。
- 内部构造：`new_primitive_node` 实例化 `PrimitiveNode`。
- 兼容别名：三个 camelCase 函数分别只调用对应的 snake_case 函数。
- 直接测试调用者包括 [`schema_builder_test.rs`](./schema_builder_test.rs) 的越界精度降级测试，以及 [`writer_core_test.rs`](./writer_core_test.rs) 的 schema、非法输入和无符号 BIGINT 场景。

`column_type` 是核心下游依赖：它定义 `ColumnInfo`、`Column`、`PhysicalType`、`LogicalType`、`TimeUnit` 及 `to_column_type`。`lib.rs` 定义字符串包装的统一 `Error` 和 `Result`。本文件不执行 I/O、不直接调用 parquet-rs，也不依赖 Cargo feature；`Cargo.toml` 中的 Parquet Git 依赖由同 crate 的 writer/reader 模块消费。

## 错误处理与边界

`validate_column_info` 返回两类带上下文的错误：空名称返回 `parquet column name is empty`；有效精度 DECIMAL 的非法 scale 返回包含列名、scale 和 precision 的消息。Rust 参数是 `&ColumnInfo` 或 `&[ColumnInfo]`，类型系统排除了 Go 版本中 `*ColumnInfo == nil` 的情况，因此 Rust 没有“nil 列信息”分支。

DECIMAL 校验刻意只覆盖 precision `1..=38`：超出范围的 DECIMAL 会由 `to_column_type` 降级为字符串，所以即使 scale 越界也不会交给 DECIMAL 逻辑类型构造。`schema_builder_test.rs` 明确锁定这一行为。

`new_primitive_node` 只对 `FixedLenByteArray` 检查正宽度，失败消息包含无效宽度。它不复验物理类型与逻辑类型的全部合法组合；这些组合主要由 `to_column_type` 维护，并由 `writer.rs::parquet_schema(...).build()` 在真实 parquet-rs schema 构造时进行最终校验。

与 Go 版本不同，Rust 主入口直接传播 `new_primitive_node` 的错误，没有额外包装 `build parquet schema for column <name>`；Rust 的轻量根 `Schema` 构造也是不可失败的，而 Go 的 `schema.NewGroupNode` 仍返回错误。调用者不得假定两种实现的错误文本和错误分层完全相同。

## 并发与资源生命周期

所有函数都是同步纯计算，既不创建线程、任务、锁、通道或事务，也不持有文件、网络连接或 parquet writer。局部 `Vec` 在失败时按 Rust 所有权规则自动释放；成功时其所有权随返回值转移给调用者。

公开数据类型均由拥有所有权的 `String`、`Vec` 和可克隆枚举组成，因此结果与输入生命周期解耦。并发安全性没有在本文件施加额外约束；是否跨线程移动最终由这些字段的自动 trait 和上层 writer 的泛型约束决定。性能上，函数对两个向量预分配准确容量，但每列会克隆一份 `ColumnInfo`，并为轻量节点再次克隆列名和逻辑类型。

## 与 Go 版本的对应关系

直接对照文件是 [`schema_builder.go`](./schema_builder.go)：Rust 的三个 snake_case 主函数分别对应 Go 的 `buildParquetSchemaFromColumns`、`validateColumnInfo`、`newPrimitiveNode`，camelCase 别名用于保留移植命名。

共同语义包括：保持列顺序；校验空名称及有效 DECIMAL 的 scale；用 `toColumnType`/`to_column_type` 映射类型；让 timestamp 逻辑类型无条件支持 NULL；根名为 `schema`；返回解析后的列描述供 writer 使用。Go 注释称 TIMESTAMP 和 DATETIME 可能携带非法 MySQL 值，Rust 通过两者都映射到 `LogicalType::Timestamp` 来实现相同结果。

实现差异包括：

- Go 输入是 `[]*ColumnInfo`，需要显式拒绝 nil；Rust 输入是值切片引用，没有对应空指针状态。
- Go 直接创建 Arrow Go 的 `schema.Node`/`GroupNode`；Rust 本文件先创建自有轻量 `PrimitiveNode`/`Schema`，真正的 parquet-rs `Type` 在 `writer.rs::parquet_schema` 中另行创建。
- Go 的内部 `column` 同时保存 `Repetition`；Rust 的 `Column` 保存 `allows_null_encoding`，重复度在轻量节点和最终 writer schema 中按需推导。
- Go 根据逻辑类型是否为 none 选择两个底层叶子构造器，并为构造错误添加列名上下文；Rust 统一填充自有节点字段，只单独校验定长宽度。

Go 回归测试 [`writer_core_test.go`](./writer_core_test.go) 通过实际 Arrow reader 验证物理类型、逻辑注解、definition level、非法 MySQL 零时间的 NULL 编码、输入错误和 9 字节无符号 BIGINT。Rust 的 [`writer_core_test.rs`](./writer_core_test.rs) 覆盖相应 schema 与写出行为，独立 [`schema_builder_test.rs`](./schema_builder_test.rs) 补充越界 DECIMAL 精度降级语义。

## 扩展指南

- 新增数据库类型映射时，主要修改点是 `column_type.rs::to_column_type`；同时检查 `build_parquet_schema_from_columns` 是否需要新的空值编码或单位派生规则，并确保 `new_primitive_node` 与 `writer.rs::parquet_schema` 对新类型表达一致。
- 新增 `LogicalType` 或 `PhysicalType` 变体时，必须同步 writer 的穷举映射、列缓冲和值编码逻辑，不能只扩展轻量 schema。否则文档层的节点可能可构造，而真实 parquet-rs schema 或写出路径不可用。
- 改动 DECIMAL 规则时，应同时审查 `validate_column_info`、`decimal_column_type`、定长宽度计算以及 Go 对照；特别要保留“有效精度才校验 scale、越界精度降级字符串”的现有契约，除非兼容策略明确改变。
- 改动 timestamp/DATETIME 行为时，应同步检查 `column_value` 的非法时间转换和 definition level 写出，避免 `allows_null_encoding` 与实际 NULL 行为分离。
- 测试逻辑应放在独立文件，不要内嵌到生产文件。优先扩展 `schema_builder_test.rs` 做构建器边界回归，扩展 `writer_core_test.rs` 做真实 writer/schema 联动验证，并与 `writer_core_test.go` 的同类用例保持语义一致。
- 兼容风险主要是已生成 Parquet 的类型注解、required/optional 层级和错误契约；性能风险主要来自每列克隆及重复 schema 物化。当前列数通常较小，但若改变对象结构或在热路径重复构建，应重新评估分配成本。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录及 `schema_builder.rs` 均在索引内。
- RustCodeGraph 查询：`query` 精确定位 `build_parquet_schema_from_columns`、`validate_column_info`、`new_primitive_node`；`node` 源码与 Trail 核对了主入口到校验、类型映射、叶子构造的下游边，以及 `ParquetWriter::new`、别名和测试的上游边；`explore` 另定位到 Dumpling 的 `parquet_columns` 生产调用，并以对应源码复核。
- 已核对生产源码：[`schema_builder.rs`](./schema_builder.rs)、[`column_type.rs`](./column_type.rs)、[`writer.rs`](./writer.rs)、[`lib.rs`](./lib.rs)、[`../../../dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs)。
- 已核对 crate 配置：[`Cargo.toml`](./Cargo.toml)。
- 已核对 Go 对照与测试：[`schema_builder.go`](./schema_builder.go)、[`writer.go`](./writer.go)、[`writer_core_test.go`](./writer_core_test.go)。
- 已核对 Rust 独立测试：[`schema_builder_test.rs`](./schema_builder_test.rs)、[`writer_core_test.rs`](./writer_core_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用固定章节结构命令，并人工复核本文能够回答文件存在理由、生产执行链、状态不变量、错误边界、Go 差异和安全扩展位置。

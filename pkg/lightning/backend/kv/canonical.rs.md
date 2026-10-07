# `pkg/lightning/backend/kv/canonical.rs`

## 文件定位

`canonical.rs` 位于 `astersql-lightning-backend-kv` crate 内，是 Lightning 自有的轻量数据模型与 `astersql-tablecodec` 所使用的 TiDB canonical 数据模型之间的适配层。crate 入口 `pkg/lightning/backend/kv/lib.rs` 以私有模块 `mod canonical` 装配本文件，再通过 `pub use canonical::*` 导出其中的公开项；crate 归属和直接依赖由 `pkg/lightning/backend/kv/Cargo.toml` 确认，其中 `encode` 提供 Lightning 的 `Datum`、`Column` 和 `ColumnType`，`tablecodec` 提供正式的行、索引、Handle 和 MySQL 类型编解码能力。

该文件处在两条主链的交界处：写入方向由 `pkg/lightning/backend/kv/base.rs` 的 `BaseKVEncoder::AddRecord` 调用本文件构造 Handle、行值、表元数据和索引元数据，再交给 `tablecodec` 生成 TiKV 兼容 KV；读取方向由 `pkg/lightning/backend/kv/kv2sql.rs` 的 `TableKVDecoder` 调用本文件解码行值和 canonical Datum。公开的 `toCanonicalDatum`、`fromCanonicalDatum` 还被 `pkg/dxf/importinto/conflict_resolution.rs` 跨 crate 用于冲突行重编码。

本文件不是独立存储引擎，也不持有会话或事务。它只完成确定性的值/元数据转换并调用 `tablecodec`；真正的 KV 写入由 `BaseKVEncoder::AddRecord` 通过 `Session::Txn().Set` 完成。

## 核心职责

1. 在 Lightning `encode::Datum` 与 `tablecodec::types::Datum` 之间双向转换，覆盖哨兵值、数值、字节/字符串、JSON、BIT、ENUM、SET、DECIMAL、时间和时长；入口为 `toCanonicalDatum` 与 `fromCanonicalDatum`。
2. 将轻量 `encode::Column` 映射为解码所需的 `types::FieldType`；入口为 `fieldType`，负责 MySQL 类型码、无符号标志、字符集、枚举元素和 BIT 宽度。
3. 使用 `tablecodec::EncodeRow` / `DecodeRowToDatumMap` 编解码 TiDB 行值；入口为 `encodeCanonicalRowWithMeta`、`encodeCanonicalRow` 和 `decodeCanonicalRow`。
4. 从轻量列/索引定义构造最小 `model::TableInfo` / `model::IndexInfo`，供索引键值生成和解码路径使用；入口为 `canonicalTableInfo` 与 `canonicalIndexInfo`。
5. 按普通隐式 RowID、整型主键 Handle、Common Handle 三种表模式生成 `kv::Handle`；入口为 `canonicalHandle`。

这些职责存在的原因是 Rust Lightning 层没有像 Go 版本那样始终持有完整的 `table.Table` 和原生 `types.Datum`。当 `TableDefinition::source_meta` 存在时，写行路径会优先使用真实持久化列 ID 和列属性；只有轻量定义可用时才使用本文件构造的最小元数据（见 `BaseKVEncoder::AddRecord`）。

## 主要符号

- `shared_error(error) -> String`：文件内统一的错误边界，把 `tablecodec`/类型解析错误转换为本 crate 通用的 `String`。它不增加上下文，调用者负责在更高层补充行或列语境。
- `pub fn toCanonicalDatum(&Datum) -> Result<types::Datum, String>`：公开的正向值转换。简单变体调用 canonical Datum 的 setter；JSON、DECIMAL、TIMESTAMP、DURATION 会解析文本，因此可失败。TIMESTAMP 和 DURATION 使用 `DefaultStmtNoWarningContext`，FSP 使用 `types::MaxFsp`；字符串、ENUM、SET 使用默认 collation。
- `enumName` / `setName`：`fromCanonicalDatum` 的内部辅助。ENUM 数字按 1-based 元素位置取名，0 或越界返回空名；SET 按位图和列定义顺序拼接名称。
- `pub fn fromCanonicalDatum(&types::Datum, Option<&Column>) -> Result<Datum, String>`：公开的反向值转换。由于 canonical kind 不能总能区分字符串/字节、BIT/二进制字面量、ENUM/SET 等 Lightning 语义，该函数优先使用可选列元数据消歧；未知 kind 返回错误。
- `pub(crate) fn fieldType(&Column) -> types::FieldType`：将 `ColumnType` 映射到 MySQL type code。`UInt` 增加 `UnsignedFlag`；`Bit` 和 `BinaryLiteral` 的 `Flen` 固定为 8；非空 charset 和 elements 被复制进字段类型。
- `pub(crate) fn encodeCanonicalRowWithMeta(&[Datum], &TableInfo, bool) -> Result<Vec<u8>, String>`：使用真实 `ColumnInfo.ID`、`Offset` 和生成列属性编码持久化行。整型 PK Handle 列以及非 stored 的生成列不写入行值。
- `pub(crate) fn encodeCanonicalRow(&[Datum], &[Column], bool) -> Result<Vec<u8>, String>`：轻量路径，将所有 datum 转换后用连续的 1-based 列 ID 编码。参数 `columns` 当前不参与编码决策，仅保留了与调用语义一致的签名。
- `pub(crate) fn decodeCanonicalRow(&[u8], &[Column]) -> Result<Vec<Datum>, String>`：按 1-based 列 ID 建立字段类型映射并解码；编码中缺失的目标列补 `Datum::Null`，存在的值按对应 `Column` 反向转换。
- `pub(crate) fn canonicalTableInfo(...) -> model::TableInfo`：构造列 ID/Offset 连续的最小表信息，并写入索引、`PKIsHandle`、`IsCommonHandle` 和 `CommonHandleVersion = 0`。
- `pub(crate) fn canonicalIndexInfo(&IndexDefinition) -> model::IndexInfo`：复制索引 ID、列 Offset、唯一性和主键属性；前缀长度统一为 `types::UnspecifiedLength`。
- `pub(crate) fn canonicalHandle(&TableDefinition, &[Datum], i64) -> Result<Box<dyn kv::Handle>, String>`：Handle 分派入口。Common Handle 优先于 `pk_is_handle`；否则根据整型主键或传入 `row_id` 返回 `IntHandle`。

对外可见性有意分层：只有两个 Datum 转换函数是跨 crate API；行编解码、元数据与 Handle 辅助均为 `pub(crate)`，服务于本 crate 的 `base.rs`、`kv2sql.rs` 和 `sql2kv.rs`。

## 执行流程

写入主流程如下（依据 `BaseKVEncoder::AddRecord`）：

1. `canonicalHandle` 检查 `TableDefinition` 的 Handle 模式。Common Handle 收集所有 `primary_key` 列并用 `codec::Encoder::EncodeKey` 编码；整型 PK Handle 找到主键列并要求 datum 为 `Int` 或 `UInt`；无显式 Handle 时直接使用分配的 `row_id`。
2. `tablecodec::EncodeRowKeyWithHandle` 使用该 Handle 生成记录键。
3. 若 `TableDefinition::source_meta` 存在，`encodeCanonicalRowWithMeta` 按真实列 ID 编码，并排除存在于 Handle 中的整型主键列及非存储生成列；否则 `encodeCanonicalRow` 以连续列 ID 编码全部 datum。两者均固定 UTC，并通过 `rowcodec::Encoder::new(new_format)` 选择行格式版本。
4. `BaseKVEncoder::AddRecord` 把记录 KV 写入当前会话事务。
5. 对每个轻量索引，调用 `toCanonicalDatum` 转换索引列值；真实 `TableInfo`/`IndexInfo` 可用时优先复用，否则通过 `canonicalTableInfo` / `canonicalIndexInfo` 构造。随后 `tablecodec::GenIndexKey` 和 `GenIndexValuePortal` 生成索引 KV 并写入事务。

读取主流程如下（依据 `TableKVDecoder::DecodeRawRowData` 和 `IterRawIndexKeys`）：

1. `decodeCanonicalRow` 先由每个 `Column` 构造 `FieldType`，再调用 `DecodeRowToDatumMap` 解码记录值。
2. 按目标列顺序查找 1-based 列 ID；缺失列补 NULL，存在值通过 `fromCanonicalDatum(value, Some(column))` 恢复 Lightning 类型。
3. `TableKVDecoder::DecodeRawRowData` 还会再次保留原始列 ID 到 datum 的映射、恢复默认值和 Handle 列；这些属于 `kv2sql.rs` 的上层职责，不在本文件内完成。
4. 重建索引时，`kv2sql.rs` 使用 `toCanonicalDatum`、`canonicalTableInfo` 与 `canonicalIndexInfo` 将恢复后的行重新送入 `tablecodec`。

跨 crate 的冲突处理流程见 `ImporterConflictCodec`：Common Handle 数据通过 `fromCanonicalDatum(..., None)` 转为 `DatumKey`；解码得到的可见行列再经 `toCanonicalDatum` 返回冲突处理层；重新编码前又把 canonical Datum 转回 Lightning Datum。

## 数据与状态

本文件自身没有全局可变状态。所有函数都从参数构造结果；唯一借用的全局对象是只读的 `types::DefaultStmtNoWarningContext`。时区在行和 Common Handle 编码中固定为 `tablecodec::time::UTC`，而文本时间解析使用默认无警告 statement context，这一组合是当前编码契约的一部分。

重要数据不变量如下：

- 轻量行格式使用连续的 1-based 列 ID；`decodeCanonicalRow`、`canonicalTableInfo` 与 `fieldType` 的调用方都依赖这一对应关系。
- `encodeCanonicalRowWithMeta` 不假设连续 ID，而是使用 `TableInfo.Columns[*].ID` 和 `Offset`。传入的 `datums` 必须覆盖每个将被持久化的列 Offset；代码直接索引，调用方需保证元数据与行布局匹配。
- Common Handle 中主键值的顺序等于 `table.columns` 中标记为 `primary_key` 的顺序；缺任一值即失败，不能退回 `row_id`。
- `pk_is_handle` 只接受 `Datum::Int` 或 `Datum::UInt`。`UInt` 通过 Rust `as i64` 转换，超过 `i64::MAX` 时按二进制位解释为负数；本文件没有额外范围检查。
- canonical `KindBytes` 的反向类型依赖列类型/charset：显式 String/Timestamp，或 Auto 且 charset 非 `binary` 时尝试 UTF-8；否则保留 bytes。
- `KindUint64` 配合 ENUM/SET 元数据时恢复名称；缺少/越界 ENUM 元素产生空名称，SET 只包含位图选中的已知元素。
- DURATION 解析返回 `is_null = true` 时，`toCanonicalDatum` 保持默认的 NULL datum；这是 `ParseDuration` 返回约定的直接传播。

转换会克隆字符串、字节、elements、列/索引元数据并为行值、列 ID、字段类型 map 和 Common Handle 临时分配容器。该文件不做缓存；上层 `BaseKVEncoder` 才负责复用行缓冲。

## 依赖与调用关系

直接依赖由 `pkg/lightning/backend/kv/Cargo.toml` 给出：

- `astersql-lightning-backend-encode`：`Datum`、`Column`、`ColumnType`，即 Lightning 输入/输出模型。
- `astersql-tablecodec`：`types::Datum`/`FieldType`、`model::TableInfo`/`IndexInfo`、`kv::Handle`、行编解码、索引编解码、时间和 MySQL 标志。
- `verification` 是该 crate 的依赖，但本文件没有直接使用。

RustCodeGraph 对目标文件报告 26 个符号，并显示它被 `pkg/lightning/backend/kv/base.rs`、`pkg/dxf/importinto/conflict_resolution.rs` 及相关测试/配置文件覆盖到；精确源码交叉检索确认的生产调用边包括：

- `BaseKVEncoder::AddRecord` → `canonicalHandle`、`encodeCanonicalRowWithMeta`/`encodeCanonicalRow`、`canonicalTableInfo`、`canonicalIndexInfo`、`toCanonicalDatum`。
- `TableKVDecoder` 及其 Handle/索引辅助 → `decodeCanonicalRow`、`fieldType`、`fromCanonicalDatum`、`toCanonicalDatum`、`canonicalTableInfo`、`canonicalIndexInfo`。
- `sql2kv.rs::encodeDatumList` → `encodeCanonicalRow`；`decodeDatumList` → `fromCanonicalDatum`。
- `ImporterConflictCodec::{DecodeRow, EncodeRow}` → `fromCanonicalDatum` / `toCanonicalDatum`。

主要下游调用边是 `tablecodec::EncodeRow`、`DecodeRowToDatumMap`、`codec::Encoder::EncodeKey`、`kv::NewCommonHandle` 以及 `types` 中的 JSON/DECIMAL/TIME/DURATION 解析和 datum getter/setter。RustCodeGraph 的单符号 `callers` 查询在本次环境中长时间无响应，以上精确边因此由已索引文件的 “used by” 结果、`node --file` 源码以及调用点检索共同核实，而不是把超时查询当作成功证据。

## 错误处理与边界

所有可恢复失败都统一返回 `Result<_, String>`。`shared_error` 仅保留下游错误的展示文本，因此调用者若需要表、列或原始行语境，应像 `BaseKVEncoder::Record2KV` 那样在上层包装。

明确的错误边界包括：

- JSON、DECIMAL、TIMESTAMP、DURATION 文本不合法时，`toCanonicalDatum` 返回解析错误，不生成行或索引 KV。
- `fromCanonicalDatum` 遇到需要文本语义但不是合法 UTF-8 的 bytes，或遇到未覆盖的 canonical kind 时返回错误。
- `tablecodec` 行编码/解码、Common Handle key 编码或 `NewCommonHandle` 校验失败时，原错误文本向上传播。
- Common Handle 缺少任一主键列时错误包含列名；`pk_is_handle` 没有主键标记、主键位置缺值、或 datum 不是整数时均明确失败。
- `decodeCanonicalRow` 将“列在编码中缺失”解释为 NULL，而不是错误；默认值、Handle 列和生成列的后续恢复由 `kv2sql.rs` 负责。
- `encodeCanonicalRowWithMeta` 对越界 `ColumnInfo.Offset` 使用直接索引，可能 panic；它依赖上层保证 `TableInfo` 与 datum 布局一致。扩展真实元数据路径时必须维持此先决条件或在该入口增加显式检查。
- `setName` 使用 `1_u64 << index`；当前隐含前提是 SET 元素数量不超过 64。若轻量 `Column` 允许更大集合，需要先定义越界位的兼容行为。

`pkg/lightning/backend/kv/canonical_test.rs` 当前直接覆盖整型主键选择非首列以及 Common Handle 缺列报错；`pkg/lightning/backend/kv/sql2kv_test.rs::TestExtendedDatumRoundTrip` 覆盖哨兵、JSON、二进制、BIT、ENUM、SET、DURATION 的行格式往返。错误分支并未全部逐项覆盖，新增类型或转换分支时不应只依赖现有两个直接测试。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel、事务所有权或文件/网络资源。函数参数均为共享借用，返回值拥有新分配的数据；`Box<dyn kv::Handle>` 的所有权移交调用者。因而同一函数可被多个调用者并发调用，只要 `tablecodec` 提供的只读默认 context 与编码 API 本身满足其线程安全契约。

事务生命周期完全位于上层：`BaseKVEncoder::AddRecord` 在本文件返回编码结果后写入 `SessionCtx` 的内存事务，`Record2KV` 再通过 `TakeKvPairs` 取走结果。转换或编码中途返回错误时，本文件没有需要回滚的局部资源；是否清理已写入的 KV 由会话/调用者控制。

资源成本主要来自逐行分配与克隆：Datum 转换、字段类型 `HashMap`、行值和列 ID `Vec`、Common Handle values/encoded buffer、以及表/索引元数据。热路径扩展应避免引入额外的逐列重复解析；若要缓存 `FieldType` 或元数据，缓存的所有权和表 schema 失效时机应放在持有表生命周期的 `BaseKVEncoder`/`TableKVDecoder`，而不是引入本文件全局状态。

## 与 Go 版本的对应关系

Go 同目录没有 `canonical.go`，因此本文件不是逐文件一一翻译，而是把 Go 版本由原生 TiDB 类型和表层隐式提供的行为显式化：

- Go `pkg/lightning/backend/kv/base.go::BaseKVEncoder.AddRecord` 直接调用 `e.table.AddRecord(...)`；表实现内部负责 Handle、记录行和索引 KV。Rust `BaseKVEncoder::AddRecord` 展开了这条路径，本文件的 `canonicalHandle`、行编解码和元数据构造承担其中的类型适配部分。
- Go `pkg/lightning/backend/kv/kv2sql.go::DecodeRawRowData` 委托 `tables.DecodeRawRowData`，并直接使用原生 `types.Datum`；Rust `TableKVDecoder::DecodeRawRowData` 通过 `decodeCanonicalRow` / `fromCanonicalDatum` 恢复 Lightning datum，再补默认值、Handle 和生成列。
- Go `DecodeHandleFromRowKey` / `DecodeHandleFromIndex` 直接返回 `kv.Handle`；Rust 的 `kv2sql.rs` 在 `tablecodec` Handle 与轻量 `Handle`/`DatumKey` 间还需经过本文件的 Datum 转换。
- Go 的列类型、真实列 ID、生成列属性和索引元数据来自 `model.TableInfo` / `table.Table`。Rust 在 `source_meta` 存在时同样优先采用真实元数据；`canonicalTableInfo`、`canonicalIndexInfo` 与连续 1-based ID 只为轻量 `TableDefinition` 提供最小兼容模型。
- Go 测试 `sql2kv_test.go::TestEncode`、`TestDecode`、`TestEncodeRowFormatV2` 和 `TestEncodeTimestamp` 证明的目标语义是 TiDB 记录键值、行格式版本和时间编码兼容；Rust 对应测试分散在 `canonical_test.rs`、`sql2kv_test.rs`、`kv2sql_test.rs`，而非同名 Go 文件的逐测试机械翻译。

因此，安全对齐 Go 行为时应比较最终 `tablecodec` 字节、Handle 选择、缺失列/default/generated-column 恢复和错误分支，不应仅比较函数名称或文件布局。尤其不要把轻量连续列 ID 路径误认为完整 `TableInfo` 路径的替代品。

## 扩展指南

新增或修改 Datum 类型时，至少同步以下位置：

1. 在 `pkg/lightning/backend/encode/encode.rs` 定义/确认 `Datum` 与 `ColumnType` 表达；在 `toCanonicalDatum` 和 `fromCanonicalDatum` 增加双向分支。
2. 在 `fieldType` 选择准确 MySQL type code、flag、charset/collation、Flen/Decimal 和 elements。若 canonical kind 存在歧义，必须定义列元数据缺失时的保守行为。
3. 扩展独立测试 `pkg/lightning/backend/kv/canonical_test.rs` 或同目录 `sql2kv_test.rs` / `kv2sql_test.rs`，不要把测试嵌入生产文件。至少覆盖往返、无列元数据、非法输入和实际行格式编码；涉及 Go 对齐时对照相应 Go 测试的最终行为。
4. 若类型会出现在 Common Handle 或索引，验证 `codec::Encoder::EncodeKey`、`GenIndexKey`、`DecodeIndexHandle` 以及 import-into 冲突路径可接受该类型。

修改行布局时，应分别审查真实元数据与轻量元数据两条路径。列 ID、Offset、PK-in-handle 排除规则、virtual/stored generated column 规则或 row format 开关发生变化时，首要接入点是 `encodeCanonicalRowWithMeta` / `encodeCanonicalRow` / `decodeCanonicalRow`，并同步 `BaseKVEncoder::AddRecord` 与 `TableKVDecoder::DecodeRawRowData` 的调用假设。兼容风险是旧行格式或非连续列 ID 被错误解码；性能风险是每行重复构造字段类型 map 和元数据。

修改 Handle 时，首要接入点是 `canonicalHandle`，同时检查 `kv2sql.rs::{Handle, DatumKey, fromCanonicalHandle}` 和 `pkg/dxf/importinto/conflict_resolution.rs`。需要覆盖：联合主键列顺序、缺失列、整数符号/溢出、Common Handle 编码字节、普通隐式 RowID，以及 clustered primary index 是否被跳过。

当前值得优先补强但不属于本文档任务的测试包括：非法 JSON/DECIMAL/TIME/DURATION、非 UTF-8 文本列、真实非连续列 ID、stored/virtual generated columns、`UInt > i64::MAX` 的 PK Handle、超过 64 个 SET 元素，以及 `encodeCanonicalRowWithMeta` 的 Offset 越界行为。若要改变这些边界，需先用 Go 实现或 `tablecodec` 契约确认期望，不能仅为避免错误而静默降级。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/lightning/backend/kv/canonical.rs`（完整 419 行；RustCodeGraph `node --file` 返回文件用途与源码）。
- crate 边界：`pkg/lightning/backend/kv/Cargo.toml`、`pkg/lightning/backend/kv/lib.rs`。
- Rust 上游调用：`pkg/lightning/backend/kv/base.rs`、`pkg/lightning/backend/kv/kv2sql.rs`、`pkg/lightning/backend/kv/sql2kv.rs`、`pkg/dxf/importinto/conflict_resolution.rs`。
- Rust 独立测试：`pkg/lightning/backend/kv/canonical_test.rs`、`pkg/lightning/backend/kv/sql2kv_test.rs`、`pkg/lightning/backend/kv/kv2sql_test.rs`、`pkg/dxf/importinto/conflict_resolution_test.rs`。
- Go 对照：`pkg/lightning/backend/kv/base.go`、`pkg/lightning/backend/kv/kv2sql.go`、`pkg/lightning/backend/kv/sql2kv.go`，以及 `sql2kv_test.go` / `kv2sql_test.go`。目录中不存在同名 `canonical.go`，因此按真实调用职责对照。
- 下游 API 定义：`pkg/tablecodec/tablecodec.rs` 中的 `EncodeRow`、`DecodeRowToDatumMap`、记录键与索引键函数，以及 `pkg/lightning/backend/encode/encode.rs` 中的 `Datum`、`ColumnType`、`Column`。
- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录中 `canonical.rs` 被识别为含 26 个符号，并报告被 `base.rs`、import-into 冲突处理和相关测试等文件使用。精确 `query` 唯一定位 `toCanonicalDatum` 与 `fromCanonicalDatum`；单符号 `callers` 命令在有限等待后无输出并被终止，因此调用边另由索引文件上下文和精确调用点检索复核。

人工复核结论：该文件存在是为了弥合 Lightning 轻量模型与 TiDB `tablecodec` 的类型/元数据差异；其运行路径覆盖记录 Handle、行值和索引元数据的写入适配，以及 KV→SQL 和冲突处理的反向适配。安全扩展必须同时维护双向 Datum、FieldType、真实/轻量元数据、Handle 与独立测试五个表面。

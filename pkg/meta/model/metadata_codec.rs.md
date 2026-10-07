# `pkg/meta/model/metadata_codec.rs`

## 文件定位

`metadata_codec.rs` 位于 `astersql-meta-model` crate 的 Group 1 模型层，是 `TableInfo` 与 `DBInfo` 的 Go 兼容 JSON 编解码入口。文件本身只有四个公开函数，不定义存储键、事务或模型字段；它把模型值与元存储所使用的 JSON 字节连接起来。

该文件由 `pkg/meta/model/internal/group1/lib.rs` 通过 `#[path = "../../metadata_codec.rs"] mod metadata_codec` 纳入 Group 1，并经 `pub use metadata_codec::*`、顶层 `pkg/meta/model/lib.rs` 的 `pub use ::group_1::*` 导出为 `astersql_meta_model::{EncodeTableInfo, DecodeTableInfo, EncodeDBInfo, DecodeDBInfo}`。`pkg/meta/model/Cargo.toml` 表明顶层 crate 名为 `astersql-meta-model`，并通过内部 `group-1` 路径依赖取得这套实现；这里没有条件编译项或 feature 分支。

## 核心职责

- `EncodeTableInfo` / `DecodeTableInfo` 在 `TableInfo` 与 JSON 字节之间转换。
- `EncodeDBInfo` / `DecodeDBInfo` 在 `DBInfo` 与 JSON 字节之间转换。
- 四个函数统一使用 `super::ast::metadata_json`，因此共享同一套 `serde`/`serde_json` 行为和字符串错误 ABI，而不是各自维护编码规则。
- 保留 Go 风格 PascalCase 名称，向元数据读写器和 InfoSchema 加载器提供稳定的迁移边界。

本文件不负责字段兼容规则；字段名、默认值、省略与未知值处理来自 `TableInfo`、`DBInfo` 及其嵌套类型的 `Serialize`/`Deserialize` 实现。它也不添加版本头或压缩。Domain 私有 catalog 的 `ASTERDDL2` 外层格式由 `pkg/domain/canonical_domain.rs::encode_catalog/decode_catalog` 处理，内部数据库和表 payload 才调用这里的函数。

## 主要符号

- `pub fn EncodeTableInfo(table: &TableInfo) -> Result<Vec<u8>, String>`：借用表模型，通过 `ast::metadata_json::encode` 生成紧凑 JSON 字节；不修改输入。
- `pub fn DecodeTableInfo(encoded: &[u8]) -> Result<TableInfo, String>`：借用任意字节切片，通过 `ast::metadata_json::decode` 构造一个拥有所有权的表模型。
- `pub fn EncodeDBInfo(database: &DBInfo) -> Result<Vec<u8>, String>`：数据库模型的编码入口，返回新分配的字节向量。
- `pub fn DecodeDBInfo(encoded: &[u8]) -> Result<DBInfo, String>`：数据库模型的解码入口，返回拥有所有权的结构体。

文件没有常量、类型、trait、`impl`、宏或私有辅助函数。四个 API 的下游实现分别是 `pkg/parser/ast/lib.rs::metadata_json::{encode, decode}`：前者要求 `T: Serialize` 并调用 `serde_json::to_vec`，后者要求 `T: DeserializeOwned` 并调用 `serde_json::from_slice`。

## 执行流程

编码流程如下：调用者把 `&TableInfo` 或 `&DBInfo` 交给公开入口；入口进行静态类型选择后直接转发给泛型 `metadata_json::encode`；`serde_json` 根据模型及嵌套类型上的序列化规则生成 `Vec<u8>`；成功字节再由调用者写入 KV 或嵌入其他容器。例如 `pkg/meta/reader.rs::TransactionMutator::{create_database, update_database, create_table, update_table}` 将结果写入 TiDB 兼容的 `DBs/DB:<id>` 或 `DB:<id>/Table:<id>` hash，`pkg/domain/canonical_domain.rs::publish_tidb_schema_metadata` 也用编码结果检测变化并发布元数据。

解码流程相反：调用者从快照、事务或 catalog 取得字节；入口把切片转发给 `metadata_json::decode`；`serde_json` 按目标类型构造完整模型；调用者随后补充不属于 JSON payload 的上下文。例如 `pkg/domain/canonical_domain.rs::read_catalog/decode_catalog` 在解出表后按所在数据库回填 `TableInfo.DBID`，`pkg/infoschema/issyncer/loader.rs::KvMetaReader` 再把 model 类型转换为 InfoSchema 类型。

四个函数均为单次同步调用，没有缓存、重试或部分成功状态。编码顺序和具体 JSON 形状完全由模型序列化实现决定。

## 数据与状态

输入状态只有不可变借用的模型或字节切片，输出是新拥有的 `Vec<u8>`、`TableInfo` 或 `DBInfo`。函数不保留全局状态，不修改调用者对象，也不访问存储。

持久化兼容的关键数据约束在模型定义中。例如 Go 的 `pkg/meta/model/db.go::DBInfo` 使用 `id`、`db_name`、`charset`、`collate`、`state`、`policy_ref_info` 等 JSON 名称，并明确排除 `Deprecated.Tables` 与 `TableName2ID`；Rust 的 `pkg/meta/model/db_test.rs::db_info_json_matches_go_field_names_and_omissions` 验证了这些名称、省略行为以及从 `{}` 解码出的默认值。表模型包含列、索引、外键、分区、TTL 等大量嵌套状态，`pkg/meta/model/table_test.rs::table_metadata_json_matches_go_field_names_and_embedding` 验证了若干嵌入字段和 Go 字段名。

`Vec<u8>` 没有额外 magic byte；调用方必须知道 payload 类型。若它被嵌入 Domain 私有 catalog，长度边界和 `ASTERDDL2` 魔数由外层 encoder/decoder 管理，而不是本文件。

## 依赖与调用关系

直接依赖只有 `super::{DBInfo, TableInfo}` 和 `super::ast::metadata_json`。后者在 `pkg/parser/ast/lib.rs` 中用 `serde_json::{to_vec, from_slice}` 实现，并把错误转成字符串。

RustCodeGraph 对四个入口的查询显示主要上游关系为：

- `EncodeTableInfo`：`pkg/meta/reader.rs::{create_table, update_table}`、`pkg/domain/canonical_domain.rs::{publish_tidb_schema_metadata, encode_catalog}`，以及若干 DDL/Session 测试夹具。
- `DecodeTableInfo`：`pkg/meta/reader.rs::{get_table, list_tables}`、`pkg/domain/canonical_domain.rs::{read_catalog, decode_catalog}`、`pkg/infoschema/issyncer/loader.rs::{GetTable, ListTables}`。
- `EncodeDBInfo`：`pkg/meta/reader.rs::{create_database, update_database}`、`pkg/domain/canonical_domain.rs::{publish_tidb_schema_metadata, encode_catalog}`，以及元数据往返测试。
- `DecodeDBInfo`：`pkg/meta/reader.rs::{get_database, list_databases}`、`pkg/domain/canonical_domain.rs::{read_catalog, decode_catalog}`、`pkg/infoschema/issyncer/loader.rs::{GetDatabase, ListDatabases}`。

因此它位于“模型定义 → JSON payload → TiDB 兼容 meta KV → Domain/InfoSchema”链路的序列化边界。RustCodeGraph 对泛型调用的 callee 解析未建立精确静态边，但源码中的四个函数体明确直接调用 `metadata_json::encode/decode`，该直接源码证据优先于缺失的图边。

## 错误处理与边界

`ast::metadata_json` 把 `serde_json::Error` 通过 `to_string()` 降格为 `String`，所以四个入口保留可读错误文本，但不保留结构化错误类型、分类或源链。调用方通常继续适配到自身错误类型：`pkg/meta/reader.rs::SnapshotMetaReader` 使用 `errors::new`，`canonical_domain.rs` 使用 `kv::errors::New`，InfoSchema loader 使用 `SyncError`。

编码只会在模型序列化失败时返回错误；本文件不做业务校验，也不检查 ID、名称或模型状态。解码会拒绝语法非法、类型不匹配或不满足模型反序列化约束的 JSON；它不检查空切片、magic byte、尾随业务字段或数据库/表归属关系。未知字段是否被忽略、缺失字段是否默认、未知枚举是否可保留，均取决于目标模型的 serde 实现和测试约束，不能从本薄封装单独推断。

调用方不应把解码成功等同于元数据语义有效。例如 `canonical_domain.rs::decode_catalog` 仍校验 catalog 头、长度、尾随字节和表所属数据库；`reader.rs::get_table` 仍先检查数据库存在性。

## 并发与资源生命周期

本文件无锁、无任务、无通道、无事务句柄、无 I/O，也没有共享可变状态；只要输入类型的只读序列化满足 Rust 类型约束，函数本身可由不同线程并发调用。每次编码独立分配并返回 `Vec<u8>`，每次解码独立构造拥有所有权的模型，临时 serde 状态在返回前释放。

事务和快照生命周期属于上游。`pkg/meta/reader.rs::TransactionMutator` 在已有事务内调用 codec，codec 成功不代表 KV `Set` 或提交成功；`pkg/domain/canonical_domain.rs::DdlMetadataService` 的 writer mutex 负责本地写者排序，codec 不参与加锁；`pkg/infoschema/issyncer/loader.rs` 的快照读取与 cache mutex 同样在 codec 外部。新增逻辑不得在这些薄入口中隐式打开事务或引入全局缓存，否则会破坏当前清晰的生命周期边界。

## 与 Go 版本的对应关系

当前 Go 树没有 `pkg/meta/model/metadata_codec.go` 或四个同名函数。Go 对照行为分散在 `pkg/meta/meta.go`：`Mutator::{CreateDatabase, UpdateDatabase, CreateTableOrView, UpdateTable}` 直接调用 `json.Marshal`，而 `IterDatabases`、`IterTables`、`ListDatabases`、`GetDatabase`、`ListTables`、`GetTable` 等直接调用 `json.Unmarshal`。Rust 四函数把这些重复的 JSON 操作集中为模型层 API，但目标 wire format 仍应与 Go `encoding/json` 和模型 tag 保持一致。

已验证的对齐证据包括：Go `pkg/meta/model/db.go::DBInfo` 与 Rust `pkg/meta/model/db_test.rs` 的字段名/忽略字段；Go `pkg/meta/model/table.go::TableInfo` 的 JSON tag 与 Rust `pkg/meta/model/table_test.rs` 的字段形状断言；`pkg/meta/model/internal/group1/migration_aster_unit_test.rs` 对数字枚举和未知枚举值的 metadata JSON 行为。差异是错误类型：Go 返回 `error`，Rust 此边界返回字符串；此外 Rust 的薄入口不复刻 Go 元存储函数内的存在性检查、键布局或事务逻辑，那些职责由 `pkg/meta/reader.rs` 等上游承担。

## 扩展指南

新增第三种元数据类型时，若它确实需要稳定的公开 codec，可按这四个函数的形态增加成对入口，并继续委托 `ast::metadata_json`；不要在入口中重复实现 serde 或混入 KV 键布局。若只是给 `TableInfo`/`DBInfo` 增加字段，通常应修改对应模型的 serde 标注和 Go 对照类型，而不是修改本文件。

任何 wire-format 变更都应同时检查：Go 模型 JSON tag；Rust 模型的 `Serialize`/`Deserialize`；`pkg/meta/model/db_test.rs` 或 `table_test.rs` 中的独立兼容测试；使用四个入口的 `pkg/meta/reader.rs`、`pkg/domain/canonical_domain.rs` 和 `pkg/infoschema/issyncer/loader.rs`。测试应保留在独立 `*_test.rs` 文件，不嵌入 `metadata_codec.rs`。

兼容风险主要是已有 KV 中旧 JSON 的可读性、Go/Rust 双向字段形状和默认值；正确性风险是错误地省略或重命名字段；性能风险是大表模型的完整 `Vec<u8>` 分配与全量解析。若要优化分配或流式处理，必须先证明所有调用者仍获得相同 JSON 字节语义，并评估 Go 侧 `IsTableInfoMustLoad` 对字段顺序的历史依赖，不能仅替换实现后假定兼容。

## 验证依据

- 源文件与导出路径：`pkg/meta/model/metadata_codec.rs`、`pkg/meta/model/internal/group1/lib.rs`、`pkg/meta/model/lib.rs`。
- crate 边界：`pkg/meta/model/Cargo.toml`（`astersql-meta-model` 顶层 crate、内部 group 依赖和 Go package 移植元数据）。
- 直接实现：`pkg/parser/ast/lib.rs::metadata_json::{encode, decode}`。
- 生产调用链：`pkg/meta/reader.rs`、`pkg/domain/canonical_domain.rs::{publish_tidb_schema_metadata, read_catalog, encode_catalog, decode_catalog}`、`pkg/infoschema/issyncer/loader.rs::KvMetaReader`。
- Go 对照：`pkg/meta/meta.go` 的 `json.Marshal/json.Unmarshal` 读写点，以及 `pkg/meta/model/{db.go,table.go}` 的 JSON tag。
- 独立 Rust 测试：`pkg/meta/model/db_test.rs::db_info_json_matches_go_field_names_and_omissions`、`pkg/meta/model/table_test.rs::table_metadata_json_matches_go_field_names_and_embedding`、`pkg/meta/model/internal/group1/migration_aster_unit_test.rs`；没有发现同名 `metadata_codec_test.rs`。
- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；文件查询识别出 5 个节点（文件加四个函数），符号查询确认四个公开签名，`explore` 给出编码/解码入口到 reader、Domain、InfoSchema 及测试夹具的调用者集合。
- 本任务是纯文档分析，按计划未运行 Cargo；交付前使用任务指定命令验证恰有 11 个固定二级章节，并人工核对没有把上游事务、外层 catalog 格式或模型 serde 规则误写成本文件职责。

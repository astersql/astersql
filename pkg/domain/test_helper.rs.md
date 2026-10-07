# `pkg/domain/test_helper.rs`

## 文件定位

`pkg/domain/test_helper.rs` 属于 `astersql-domain` crate（见 `pkg/domain/Cargo.toml`），由 `pkg/domain/lib.rs` 通过公开模块 `pub mod test_helper` 装配。它不参与 Domain 的后台运行主链，而是给测试代码提供 `DomainTestHelper` 扩展 trait，使测试能够替换当前 InfoSchema、按名称取得表/分区标识，以及枚举当前内存快照中的 schema 和表。

文件前半段第 23–142 行是被逐行注释掉的早期机械翻译草稿，不会进入编译产物；实际实现从 `use std::sync::Arc` 和 `pub trait DomainTestHelper` 开始。虽然用途是测试辅助，模块本身并未使用 `#[cfg(test)]` 隔离；只有同目录的 `test_helper_test.rs` 在 `lib.rs` 中受 `#[cfg(test)]` 控制。

## 核心职责

- `DomainTestHelper` 为 `Domain` 增加五个面向测试的便捷操作，避免测试直接操作 Domain 内部字段。
- `mock_info_cache_and_load_info_schema` 清空并重建容量为 16 的 InfoSchema 缓存，然后以时间戳 `0` 插入指定 `SchemaRef`。
- `must_get_table_info` 将库名、表名转换成 `CiString` 后查询当前 InfoSchema；“must” 表示查不到时直接 panic，而不是向调用者返回 `Result`。
- `must_get_table_id` 与 `must_get_partition_at` 复用表查询结果，分别提取表 ID 和按定义顺序选择的分区 ID。
- `fetch_all_schemas_with_tables` 将当前 InfoSchema 快照投影为 `(库原始名, [(表原始名, 表 ID)])` 的轻量列表。

这些能力只读取或替换内存中的元数据快照，不执行 DDL、不访问 KV，也不等待 schema 同步。

## 主要符号

- `pub trait DomainTestHelper`：本文件唯一公开类型，声明五个方法；调用方必须把 trait 引入作用域后才能在 `Domain` 上使用这些方法。
- `mock_info_cache_and_load_info_schema(&self, schema: SchemaRef)`：通过 `Domain::info_cache` 获得 `Arc<InfoCache>`，依次调用 `InfoCache::Reset(16)` 和 `InfoCache::Insert(schema, 0)`。`Insert` 的布尔返回值被有意忽略。
- `must_get_table_info(&self, database: &str, table: &str) -> Arc<TableInfo>`：调用 `Domain::info_schema` 取得最新快照，再调用 `InfoSchema::TableByName`；成功时从返回的 `Table` 元组结构取 `.0` 元信息，失败时 panic，消息包含原始参数 `table {database}.{table} does not exist`。
- `must_get_table_id(&self, database: &str, table: &str) -> i64`：调用 `must_get_table_info`，返回 `TableInfo::id`。
- `must_get_partition_at(&self, database: &str, table: &str, index: usize) -> i64`：调用 `must_get_table_info`，要求 `TableInfo::partition` 为 `Some`，再直接索引 `definitions[index]` 并返回 `id`。
- `fetch_all_schemas_with_tables(&self) -> Vec<(String, Vec<(String, i64)>)>`：调用 `InfoSchema::AllSchemas`，克隆 `DBInfo::name.original` 和每个 `TableInfo::name.original`，并复制表 ID。
- `impl DomainTestHelper for Domain`：把上述接口接到 `crate::domain::Domain`；没有为其他类型提供通用 blanket implementation。

## 执行流程

替换测试快照时，调用方把 `SchemaRef` 传给 `mock_info_cache_and_load_info_schema`。实现先取出 Domain 共享的 `InfoCache`，在写锁保护下清空旧缓存并把容量设为 16，再以 schema 自身的 `SchemaMetaVersion` 排序插入新快照；传入的 `schema_ts` 固定为 0。之后 `Domain::info_schema` 的 `GetLatest` 会返回这个缓存中的最新快照。

按名查询时，`must_get_table_info` 先构造两个 `CiString`。`CiString::new` 同时保存输入原文和小写副本；`InfoSchema::TableByName` 使用小写字段查找，所以库表名匹配不区分大小写。查询成功后直接返回共享的 `Arc<TableInfo>`；`must_get_table_id` 和 `must_get_partition_at` 都在此结果上继续提取字段。

枚举时，`fetch_all_schemas_with_tables` 只读取一次当前 `SchemaRef`，调用 `AllSchemas` 得到库列表，再逐库遍历其 `tables`。输出保留元数据中的原始名称拼写；该方法没有排序步骤，因此调用方不应把返回顺序当成契约。

## 数据与状态

- `SchemaRef` 是 `Arc<dyn InfoSchema>`，表示只读、可共享的元数据快照。
- `Domain` 持有 `Arc<InfoCache>`；`info_cache()` 克隆这个共享句柄，`info_schema()` 从缓存首项克隆最新 `SchemaRef`。若 Domain 尚未初始化且缓存为空，`info_schema()` 会以 `domain must be initialized before reading infoschema` panic。
- `InfoCache::Reset` 替换内部向量并设定容量，但不会在本方法内重置 `empty_schema_versions`、`first_known_schema_version` 等所有附加状态；随后 `Insert` 按 schema 版本维护降序缓存。
- `TableInfo`、`DBInfo` 和分区定义均来自 `astersql-infoschema` 的缓存视图。本文件返回共享表元数据或复制出来的字符串/整数，不修改表对象。
- 枚举结果是临时拥有的 `Vec`，名称通过 `clone` 复制；其后 Domain 的缓存变化不会反向修改已返回结果。

## 依赖与调用关系

直接依赖为标准库 `std::sync::Arc`、`astersql_infoschema::{CiString, SchemaRef, TableInfo}` 和 crate 内的 `domain::Domain`。`pkg/domain/Cargo.toml` 以路径依赖 `astersql-infoschema = ../infoschema` 提供这些元数据抽象；模块入口在 `pkg/domain/lib.rs`。

RustCodeGraph 对本文件给出的内部调用边为：`must_get_table_id -> must_get_table_info`、`must_get_partition_at -> must_get_table_info`。下游真实调用进一步落到 `Domain::info_cache`、`InfoCache::{Reset, Insert}`、`Domain::info_schema`、`InfoSchema::{TableByName, AllSchemas}` 和 `CiString::new`。

当前仓库的 Rust 上游调用集中在 `pkg/domain/test_helper_test.rs`：主测试调用表信息、表 ID、两个分区 ID 和 schema 枚举；两个 panic 测试分别调用缺失表查询和越界分区查询。代码搜索未发现 `mock_info_cache_and_load_info_schema` 的 trait 声明/实现之外调用，因此它当前缺少直接 Rust 回归覆盖。

## 错误处理与边界

- `must_get_table_info` 把 `TableByName` 的任何 `InfoSchemaError` 都折叠为固定 panic 消息，不保留原错误码；它适合测试断言，不适合可恢复的生产路径。
- `must_get_table_id` 继承缺表 panic。
- `must_get_partition_at` 有三类 panic 边界：Domain 未初始化、表不存在、表没有分区；`definitions[index]` 使用 Rust 直接索引，越界也会 panic，不会钳制索引或返回默认值。
- `mock_info_cache_and_load_info_schema` 不检查 `Insert` 返回值。由于刚刚 `Reset(16)`，正常容量下首次插入应有空间；若未来改变调用顺序或缓存语义，应增加断言或测试说明该不变量。
- `fetch_all_schemas_with_tables` 没有错误返回，也不会从持久化元数据重新加载；它只能反映调用瞬间的内存快照。
- 返回的 schema/table 顺序取决于 `AllSchemas` 及 `DBInfo::tables` 的底层迭代顺序，本文件没有稳定排序保证。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或外部连接。共享生命周期由 `Arc` 管理：查询返回的 `Arc<TableInfo>` 和取得的 `SchemaRef` 即使在缓存随后被替换后也可继续持有旧对象。

缓存变更的同步发生在 `InfoCache` 内部：`Reset` 和 `Insert` 获取其 `RwLock<CacheState>` 的写锁，普通 `GetLatest` 获取读锁。`mock_info_cache_and_load_info_schema` 分两次独立加写锁，因此“清空”和“插入”不是一个原子事务；并发读取者理论上可能在两步之间观察到空缓存并在 `Domain::info_schema` 中 panic。它应只在受控测试装配阶段使用，不能与正在读取 InfoSchema 的运行线程并发调用。

枚举和表查询操作使用某个已取得的 `SchemaRef` 快照；它们不会长期持有 InfoCache 锁，也不会修改该快照。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/test_helper.go`。Rust 的五个方法均能找到同名 Go 意图，但接口和行为并非全部一一等价：

- `MockInfoCacheAndLoadInfoSchema`：两版都执行容量 16 的 `Reset`，再以时间戳/版本参数 0 `Insert`；Rust 接受 `SchemaRef` 并通过公开访问器取得 cache。
- `MustGetTableInfo`：Go 接收 `*testing.T`，用 `require.Nil(t, err)` 让测试失败后返回 `tbl.Meta()`；Rust 不接收测试句柄，直接 panic，并返回 `Arc<TableInfo>` 缓存视图。两版都通过大小写不敏感名称查询。
- `MustGetTableID`：两版都复用表查询并读取 ID。
- `MustGetPartitionAt`：两版都直接按索引读取分区定义；Rust 额外显式区分“无分区”的 `expect("table is not partitioned")`，索引越界仍 panic。
- `FetchAllSchemasWithTables`：这是关键迁移差异。Go 接收 `meta.Reader`，转发到 `do.isSyncer.FetchAllSchemasWithTables`，返回完整 `[]*model.DBInfo` 和 `error`，相关 Go 测试还覆盖 failpoint 错误。Rust 不接收 reader、不调用 syncer、也不返回错误，而是枚举当前 InfoSchema 并投影成名称/ID 元组。因此 Rust 版本只是当前测试所需的轻量视图，不能视为 Go 持久化读取与错误语义的完整移植。

## 扩展指南

- 新增 Domain 测试 helper 时，应在 `DomainTestHelper` 声明和 `impl DomainTestHelper for Domain` 同步增加方法，并把回归测试放在独立的 `pkg/domain/test_helper_test.rs`，不要把测试内嵌回生产源文件。
- 若扩展按名查询，优先复用 `must_get_table_info`，从而保持大小写不敏感和一致的 must/panic 契约；若需要可恢复错误，应新增返回 `Result` 的独立方法，不要悄悄改变现有测试 helper。
- 若要完整对齐 Go 的 `FetchAllSchemasWithTables`，修改点不应只限于本方法签名：还需确认 Rust `isSyncer`/元数据 reader 的真实接口、保留完整 `DBInfo`、传播错误，并为 failpoint 或等价故障注入新增独立测试。当前轻量接口的调用方兼容性也需同时评估。
- 若要让 cache 替换可与并发读取安全共存，应在 `InfoCache` 层提供单次加锁的替换操作，而不是在 helper 中拼接 `Reset` 与 `Insert`；需同步验证版本排序、旧 `Arc` 生命周期和空窗口消失。
- 任何依赖枚举顺序的新断言都应先在 helper 内显式排序，或在测试中使用集合式比较，避免把底层 map/vector 顺序误当稳定 API。
- 当前最明显的覆盖缺口是 `mock_info_cache_and_load_info_schema`；为它补测试时应验证旧快照被清除、容量行为不会阻止首次插入、最新快照和 schema 版本可被正确读回。

## 验证依据

- 目标实现：`pkg/domain/test_helper.rs`，确认一个公开 trait、一个针对 `Domain` 的实现、五个可编译方法，以及不参与编译的注释草稿。
- 模块与 crate：`pkg/domain/lib.rs`（公开 `test_helper` 模块及独立 `#[cfg(test)] mod test_helper_test`）和 `pkg/domain/Cargo.toml`（`astersql-domain`、`astersql-infoschema` 路径依赖）。该目录没有 `pkg/domain/doc.go`。
- Rust 下游实现：`pkg/domain/domain.rs` 的 `Domain::{info_cache, info_schema}`，`pkg/infoschema/cache.rs` 的 `SchemaRef`、`InfoCache::{Reset, Insert, GetLatest}`，以及 `pkg/infoschema/infoschema.rs` 的 `CiString`、`InfoSchema::{TableByName, AllSchemas}`。
- Rust 测试：`pkg/domain/test_helper_test.rs` 验证大小写不敏感查询、表 ID、分区 ID、schema 枚举、缺表 panic 和分区索引越界 panic；没有直接覆盖 cache 替换 helper。
- Go 对照：`pkg/domain/test_helper.go`；`pkg/domain/db_test.go` 进一步证明 Go 的 `FetchAllSchemasWithTables` 读取元数据 reader、返回完整 DB 信息并传播注入错误。
- RustCodeGraph：`status` 报告索引含 7,032 个 Rust 文件；`explore` 确认两条 helper 内部调用边及 `test_helper_test.rs` 的上游测试调用。精确 `callers/callees` 命令未产生额外输出，未据此推断不存在调用，而是以 `explore` 与仓库范围代码搜索交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令验证目标文件存在且恰好包含十一个固定二级标题。

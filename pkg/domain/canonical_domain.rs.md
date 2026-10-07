# `pkg/domain/canonical_domain.rs`

## 文件定位

本文件属于 `astersql-domain` crate。`pkg/domain/Cargo.toml` 将 crate 根指定为 `lib.rs`；`pkg/domain/lib.rs` 以 `pub mod canonical_domain` 暴露模块，并在 crate 根重导出 `DdlMetadataChange`、`DdlMetadataService`、`InfoSchemaLoader`、`KvInfoSchemaLoader`、`LoadedInfoSchema` 和 `StorageHandle`。因此它不是独立 DDL 执行器，而是 Rust `Domain` 与规范 TiDB KV 元数据 ABI 之间的适配层。

在完整应用中，它同时承担三条边界：`DdlMetadataService` 把 `Domain::ddl_*` 请求原子写入 KV；`KvInfoSchemaLoader` 从当前版本或历史快照重建会话可见 `InfoSchema`；`StorageHandle` 让 Domain、统计模块和会话运行时共享同一存储实例并统一关闭。上层接线位于 `pkg/domain/domain.rs`，会话通常通过 `pkg/session/runtime.rs` 或 `pkg/testkit/mockstore.rs` 构造 `KvInfoSchemaLoader`。

本文件没有条件编译项，也没有内嵌测试。独立测试由 `pkg/domain/lib.rs` 的 `#[cfg(test)] #[path = "canonical_domain_test.rs"]` 装配，符合生产逻辑与测试分文件的约定。

## 核心职责

- 维护 `MetadataCatalog`：schema 版本、普通对象 ID 上界、数据库集合和 `(库名, 表名)` 到 `TableInfo` 的有序映射。
- 通过 `DdlMetadataService::mutate_with_kv` 串行化本进程写者，在一个 KV 事务内读取目录、执行业务变更、发布 TiDB 兼容元数据、递增 schema 版本并提交；无变化时回滚。
- 提供建删库表、重命名、截断、分区/placement/table-mode、列/索引/外键、TiFlash 副本、TTL 和物化视图日志等元数据操作。这些方法只修改目录和必要的伴随 KV 行，不承担完整 SQL 语义校验或物理数据搬迁。
- 同时维护私有快照键 `mDDL:canonical-catalog:v1` 与 Go/TiDB 可读的 `mSchemaVersionKey`、`mNextGlobalID`、`mDBs`、`mDB:<id>`、`mDiff:<version>` 元数据，使 Rust 写入可被既有 Go ABI 和 loader 消费。
- 从指定 KV 版本构建 v1 或共享缓存的 InfoSchema v2 快照，保留空数据库，并返回快照时间戳。
- 封装单一 `kv::Storage` 的共享访问、可选 region-split 能力与关闭排他性，并实现统计模块要求的 `StatsKvStorage`。

当前实现明确是“canonical”兼容桥：它发布的是要求全量重载的通用 schema diff，而不是 Go DDL 针对每种 action 生成的精细 diff；新增功能不能把这层当成完整 DDL job 状态机。

## 主要符号

- `DDL_CATALOG_KEY`：私有目录快照键 `mDDL:canonical-catalog:v1`。它是兼容/回退载荷，不取代 TiDB 的公开元数据哈希。
- `MetadataCatalog`：crate 内部目录。`version`、`next_id` 与 `tables` 私有，`databases` 对 crate 可见；`BTreeMap` 令编码与遍历顺序稳定。
- `DdlMetadataChange`：一次写操作的结果摘要。`old_tables`/`new_tables` 供 Domain 刷新统计和外部规则，删除的列/索引 ID 供清理，`schema_version` 是提交后的版本，`changed` 决定是否需要发布运行时变化。
- `DdlMetadataService`：规范写服务，内部只有 `Mutex<()> writer`。公开查询为 `replica_tables`、`database_names`；核心内部入口为 `mutate`、`mutate_with_kv`。
- `DdlMetadataService` 的公开变更 API：`create_database[_with_id]`、`drop_database[_with_ttl]`、`alter_table_ttl`、`create_table`、`create_materialized_view_log`、`set/update_tiflash_replica*`、`drop_tables`、`set_shard_row_id_bits`、`rename_tables`、`truncate_table`、`replace_partitions`、`set_table_partitioning`、`set_table_placement`、`set_table_mode`、`drop_table_items`、`replace_foreign_keys`、`add_columns`、`add_index`、`rename_index`、`set_index_visibility`、`change_column`、`modify_column`、`exchange_partition`。
- `tidb_string_key`、`tidb_hash_key`、`tidb_hash_prefix`：实现 TiDB `structure.TxStructure` 的 `m` 前缀与 mem-comparable 字段编码；`read_tidb_string`、`scan_tidb_hash` 是相应读取工具。
- `publish_tidb_schema_metadata`：比较前后目录，只写变化的 DB/Table 条目并删除消失条目。
- `allocate_id`、`assign_table_physical_ids`：分配表/分区物理 ID；保留系统 ID 不推进普通用户对象的 `next_id`。
- `index_visibility_equal_fold`：模拟 Go `strings.EqualFold` 的简单 Unicode folding，专用于原索引与 changing 临时索引联动。
- `KvInfoSchemaLoader`：规范读实现。`new` 构造 v1 loader，`new_v2` 创建共享 `infoschema_v2::Data` 并设置缓存容量，`load_at` 是共同实现。
- `InfoSchemaLoader: Send + Sync`：Domain 注入点，定义当前加载、时间戳快照加载和 keyspace 存在性查询。
- `LoadedInfoSchema`：将 `SchemaRef` 与读取使用的 KV 时间戳绑定。
- `read_catalog`、`build_info_schema`、`build_info_schema_v2`：依次完成元数据选择/解码、v1 构建和 v2 历史缓存装配。
- `Encoder`、`Decoder`、`encode_catalog`、`decode_catalog`：私有目录的 `ASTERDDL2` 小端长度前缀格式；解码校验截断、魔数、数据库引用和尾随字节。
- `StorageHandle`：`RwLock<Box<dyn kv::Storage + Send + Sync>>` 加可选 `SplittableStore`；`with_storage` 提供共享访问，`close` 排他关闭，并实现 `StatsKvStorage`。

## 执行流程

写入主流程如下：

1. `Domain::ddl_*` 方法在 `pkg/domain/domain.rs` 中通过 `StorageHandle::with_storage` 调用对应 `DdlMetadataService` 方法。
2. 公开方法规范化库表名并在闭包中执行业务检查；`mutate` 转交 `mutate_with_kv`。后者先取得本地 writer mutex，再 `Begin` KV 事务、记录 `StartTS`，并设置 `DiskFullOpt::AllowedOnAlmostFull`。
3. `read_catalog` 读取事务视图。若公开 `DBs` 哈希不存在且私有目录版本不旧于 `SchemaVersionKey`，可使用私有目录；一旦存在 Go/TiDB 元数据，就以公开元数据为准重建目录，防止同版本外部删除被旧私有快照覆盖。
4. 操作闭包更新目录并返回 `DdlMetadataChange`。`changed == false` 时补上当前版本并回滚，不发布版本或缓存变化。
5. 有变化时，所有 `new_tables` 及目录中对应表的 `UpdateTS` 设为事务开始时间；新版本取 `max(catalog.version, SchemaVersionKey) + 1`（饱和加法）。
6. `publish_tidb_schema_metadata` 写入/删除 Go 兼容 DB 与 Table 哈希条目；随后写 `Diff:<version>`。当前 diff 使用 `type=0` 且 `regenerate_schema_map=true`，要求消费者全量重建映射。
7. 若分配器前进则写 `NextGlobalID`，然后写 `SchemaVersionKey`、私有 `ASTERDDL2` 目录并提交。提交是跨会话可见性的边界。
8. Domain 的 `publish_ddl_metadata_change` 调用 `reconcile_from_committed_metadata`，重新加载已提交 InfoSchema 并更新统计目录；发布阶段失败时会再做一次 reconcile，使内存状态以持久化结果为准。

读取主流程为 `Domain::load_info_schema/load_snapshot_info_schema` → trait 方法 → `KvInfoSchemaLoader::load_at`。当前加载先取 `CurrentVersion("global")`，快照加载使用调用方时间戳；两者都在对应 `GetSnapshot` 上执行 `read_catalog`。v1 路径由 `build_info_schema` 把数据库和表转换为 `infoSchema`，v2 路径先构造相同完整视图，再把 DB/Table 注入共享 `Data` 并以 catalog version 和 start TS 创建 `InfoSchemaV2`。`LoadedInfoSchema.timestamp` 保存的正是该 KV 版本号。

典型操作还包含各自不变量：重命名先整体检查重复源/目标并沿 rename 链修正子表外键；截断重置表与分区物理 ID 和 auto-ID；删列重排 column offset、删除依赖索引并重算 index column offset；分区更新调用 `astersql-ddl::storage_class` 的规范化、校验和重建；物化视图日志在同一事务里同时写 purge-info 记录、基表反向链接和日志表。

## 数据与状态

持久化有两套表示但只有一个事务真相。公开 TiDB ABI 保存 schema version、全局 ID、数据库/表哈希和 diff；私有 `ASTERDDL2` 快照保存同一时点的完整 `MetadataCatalog`。`read_catalog` 的优先规则确保公开元数据一旦存在就是权威来源，私有快照主要覆盖空公开目录/兼容启动场景。表读取时会根据所属 `DBInfo.ID` 重写 `TableInfo.DBID`。

对象名称在服务入口和解码时转成 ASCII 小写，并用 `TableInfo.Name.L` 建键；原始显示名仍保存在模型的 `CIStr`。`next_id` 表示已使用最大普通 ID，`allocate_id` 先饱和加一再至少取 1。`assign_table_physical_ids` 保留 metadef 固定 ID，缺失分区定义但 `Partition.Num > 0` 时生成 `p0..pN`，并保证表与各分区 ID 不冲突。

`DdlMetadataChange` 是提交后给上层的增量视图，不是事务日志：无变化分支通常只返回当前 `schema_version`；有变化分支可同时含旧表和新表。调用方必须以 `changed` 判断是否刷新，不能仅凭 vectors 是否为空推断。例如建库会 `changed=true` 但没有 table 列表。

InfoSchema v1 每次从目录完整构建；v2 loader 的 `v2_data` 在多个快照间共享历史和表缓存。`build_info_schema` 先将所有数据库加入分组，因此没有表的库仍可被 `SHOW DATABASES` 发现。对于历史兼容数据中缺失 DBInfo、但仍有表的情形，构建器会从首表 `DBID` 推导至少为 1 的数据库 ID。

## 依赖与调用关系

主要上游是 `pkg/domain/domain.rs`：`Domain::new_with_storage_handle` 保存 `Arc<StorageHandle>` 和注入的 `Arc<dyn InfoSchemaLoader>`，并创建一个 `Arc<DdlMetadataService>`；`ddl_create_database` 等方法调用写服务，`init`/`reload`/snapshot 路径调用 loader。crate 根重导出后，`pkg/session/runtime.rs`、`pkg/session/runtime/session_factory.rs`、`pkg/testkit/mockstore.rs` 以及部分 RealTiKV 测试直接构造 `KvInfoSchemaLoader`。

主要下游依赖及用途为：

- `astersql-kv`：Storage、Transaction、Snapshot/Retriever、Key 编码、版本和共享错误；这是所有可见性与提交语义的边界。
- `astersql-meta-model`：DB/Table/Partition 模型和 Go 兼容编解码；`astersql-meta-metadef`：固定系统 ID 判断。
- `astersql-infoschema`：`SchemaRef`、v1 `infoSchema`、v2 `Data/InfoSchemaV2` 和模型到运行时表的转换。
- `astersql-parser-ast`：`CIStr` 名称；`astersql-ddl`：分区 storage-class 校验/重建；`astersql-tablecodec` 与 `astersql-types`：物化视图日志 purge 系统表行编码。
- `astersql-statistics-handle`：`StorageHandle` 实现 `StatsKvStorage`，让统计持久化复用同一 canonical storage。

RustCodeGraph 对目标符号的精确查询确认：`load_info_schema` 与 `load_snapshot_info_schema` 都调用 `load_at`；`load_at` 调用 `read_catalog`，再按配置调用 `build_info_schema` 或 `build_info_schema_v2`；`build_info_schema_v2` 复用 `build_info_schema`。写服务的直接生产调用点集中在 `pkg/domain/domain.rs` 的 `ddl_metadata.*` 接线处。

## 错误处理与边界

本文件统一返回 `kv::errors::SharedError`，Domain 再将其映射为 `DomainError::Ddl` 或 `DomainError::Store`。已存在/不存在库表、重复 rename 源或目标、无效 table-mode 转换、缺列/索引、表 ID 不匹配、无 TiFlash 配置、无效分区/交换条件等都会在操作闭包中返回错误；事务未提交，因此目录保持不变。

外部回调也在持久化前运行。`drop_database_with_ttl` 的 `before_drop` 或 `alter_table_ttl` 的 `apply` 失败会中止元数据修改；Domain 层负责 TTL 外部注册的补偿。`create_materialized_view_log` 还要求 `mysql.tidb_mlog_purge_info` 存在且使用整数主键，否则不写任何部分结果。

编码边界是显式失败的：KV 整数必须是 UTF-8 十进制；`Decoder` 拒绝截断、错误 `ASTERDDL2` 头、未知数据库引用和尾随字节；模型编解码错误均转为共享错误。`scan_tidb_hash` 无论遍历成功或失败都会显式 `Close` iterator。

锁 poisoning 采用 `expect`，因此本进程持锁线程 panic 后后续访问会 panic，而不是转成业务错误。`StorageHandle::close` 的底层关闭错误会传播；普通读取在关闭后表现取决于具体 storage 实现。`keyspace_exists` 当前只对精确字符串 `SYSTEM` 返回 true，且加载方法忽略传入 keyspace，这是真实限制，不代表多 keyspace 元数据隔离已由本文件实现。

版本递增和 ID 分配使用饱和算术，没有像 Go `GenGlobalID` 那样在本文件中显式检查 `MaxUserGlobalID`；扩展到极限值语义时必须补齐兼容决策。当前通用 diff 只触发全量 reload，不携带 action-specific 表 ID，性能和增量加载行为也不同于完整 Go DDL。

## 并发与资源生命周期

`DdlMetadataService::writer` 只串行化同一服务实例内的写者；它不能替代分布式 owner 或跨进程冲突控制。真正原子性和跨会话可见性来自 KV 事务 commit。锁覆盖 Begin、读取、业务闭包、发布元数据和 commit 的完整区间，因此慢回调或大目录编码会延长本地其他 DDL 的等待时间。

`InfoSchemaLoader` 要求 `Send + Sync`，`KvInfoSchemaLoader` 的 v2 `Data` 通过 `Arc` 共享。加载本身使用 storage snapshot，不持有写服务 mutex；当前版本与历史版本可并发读取，但一致性由所选 KV version 保证。

`StorageHandle::with_storage` 在操作期间持有 inner read lock，允许多个读/事务创建调用并发；`close` 获取 write lock，等待所有正在进行的共享访问结束后调用 `Storage::Close`。可选 region splitter 放在独立 RwLock 中，其 clone 不延长 inner storage 借用。`Domain` 将该句柄用 `Arc` 分发给统计、session factory 等组件；`pkg/domain/canonical_domain_test.rs` 验证 Domain 释放后共享句柄中的 storage 已被关闭。

本文件不创建后台线程、任务或通道。Domain 的 schema reload、TiFlash poller 和其他 worker 在 `pkg/domain/domain.rs` 管理；它们只通过这里的线程安全接口访问目录和存储。新增后台消费者应复用 `StorageHandle`/`InfoSchemaLoader`，不要另持第二个未经协调的 storage 所有者。

## 与 Go 版本的对应关系

Go 没有同名 `canonical_domain.go`；对应职责分散在 `pkg/domain/domain.go`、`pkg/meta/meta.go`、InfoSchema loader/builder 和 DDL 包。Rust 文件是移植后的收敛边界，而非逐函数同文件翻译。

- Go `Domain.store`、`InfoSchema`、`GetSnapshotInfoSchema`、`Reload` 和 `Close` 对应 Rust `StorageHandle`、注入的 `InfoSchemaLoader` 以及 `pkg/domain/domain.rs` 的 cache/reload/close 编排。两边都要求一个实例围绕单一 storage 管理 schema 生命周期。
- Go `meta.NewMutator` 在事务上设置 `AllowedOnAlmostFull`，并通过 `NextGlobalID`、`SchemaVersionKey`、`DBs`、`DB:<id>` 维护元数据；Rust `mutate_with_kv` 和 key helper 保留这些键与磁盘满策略。
- Go `GenSchemaVersion`/`GenGlobalID` 使用结构化事务的原子 `Inc`；Rust 在本地 writer mutex 下读取、计算并 `Set`，跨进程串行能力较弱，不能据此推断已等价实现完整分布式 DDL owner 协议。
- Go Domain 依赖 `issyncer` loader/builder 应用精细 `SchemaDiff`；Rust 当前发布 `regenerate_schema_map=true` 的通用 diff并从完整目录重建，以正确性优先换取更多加载成本。
- Go 的 schema 快照缓存会先尝试按 timestamp 命中；Rust `KvInfoSchemaLoader::load_snapshot_info_schema` 直接在给定版本读取，外层 Domain cache 决定是否需要调用它。
- Rust 私有 `ASTERDDL2` catalog 没有直接 Go 对等物；它是迁移期完整目录快照。测试证明当 Go 格式元数据存在或同版本发生删除时，loader 仍以公开 Go ABI 为准。
- `index_visibility_equal_fold` 明确复现 Go `strings.EqualFold` 的简单 Unicode 匹配，包括 Kelvin sign、long s、final sigma，而不把 Turkish dotted/dotless I 或 `ß` 当成同一简单折叠。

Go `pkg/domain/domain_test.go` 覆盖 Domain reload、关闭和外部 workload 行为；本文件更直接的 Rust 对照验证集中在 `pkg/domain/canonical_domain_test.rs`。涉及更完整 schema diff/loader 行为时，还应参照 `pkg/infoschema/issyncer/loader.go` 与其 Rust 移植，而不要把所有差异塞进本文件。

## 扩展指南

- 新增一种元数据变更时，优先增加 `DdlMetadataService` 方法并复用 `mutate`；只有需要在同一事务读写额外 KV 行时才用 `mutate_with_kv`。必须填好 `old_tables`、`new_tables`、removed IDs 和 `changed`，并在 `pkg/domain/domain.rs` 增加最小接线与发布后处理。
- 每项变更都要同时考虑私有 catalog、Go/TiDB DB/Table 哈希、`SchemaVersionKey`、`NextGlobalID`、diff 与 `UpdateTS`。只改内存 `MetadataCatalog` 或只写私有键会破坏 Go/Rust 互操作。
- 涉及表结构时应保持 `TableInfo` 内部引用一致：列 offset、索引列 offset、`MaxColumnID/MaxIndexID`、changing index、外键引用、DBID、AutoIDSchemaID、分区 ID/Num 以及 storage-class 派生状态都可能需要同步。
- 若引入 action-specific diff，应同步 `pkg/infoschema/issyncer` 的消费者并增加增量/全量回退测试；在此之前不要移除 `regenerate_schema_map=true`。性能评审应比较大 catalog 的扫描、完整编码和 v2 全量装载成本。
- 修改持久化编码必须保留旧格式读取或明确迁移；至少测试空目录、截断、坏魔数、尾随数据、未知 DB 引用和 Go ABI 优先规则。不得把测试放回生产 `.rs`，应扩展同目录 `canonical_domain_test.rs`。
- 新增 keyspace 支持需要同时改变 `keyspace_exists`、加载时的 storage/version 选择、Domain runtime 接线和隔离测试；不能只接受新的字符串。
- 修改锁范围或 storage 关闭顺序时，需验证写者串行、读快照一致性、关闭等待和 Domain drop 后拒绝新事务。若支持多个 writer 实例/进程，还必须依靠 KV 冲突或 owner 机制，而非当前进程内 mutex。
- 重点回归文件为 `pkg/domain/canonical_domain_test.rs`；按功能还应同步 `pkg/domain/domain_test.rs`、session DDL 独立测试或 InfoSchema loader 测试。兼容风险集中在 Go 元数据键/模型编码和错误语义，性能风险集中在全目录扫描/编码及全量 schema 重建。

## 验证依据

- RustCodeGraph：`status` 显示项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`query DdlMetadataService`、`query KvInfoSchemaLoader`、`query DdlMetadataChange`、`query mutate_with_kv` 定位目标定义。`node load_at` 明确给出 `load_info_schema/load_snapshot_info_schema → load_at → read_catalog + build_info_schema/build_info_schema_v2`；`node build_info_schema` 确认其被 `load_at` 和 `build_info_schema_v2` 调用。文件过滤命令未返回目标路径，因此按技能规则用精确源码读取补足未覆盖内容。
- 源码与 crate 边界：完整读取 `pkg/domain/canonical_domain.rs`；读取 `pkg/domain/Cargo.toml` 的 `[lib]`、运行依赖和 dev-dependencies；读取 `pkg/domain/lib.rs` 的模块、重导出与独立测试装配；目标包不存在 `pkg/domain/doc.go`。
- 生产调用证据：`pkg/domain/domain.rs` 的 `Domain` 字段、`new_with_storage_handle`、`publish_ddl_metadata_change`、各 `ddl_*` 接线、`load_info_schema/load_snapshot_info_schema` 和 storage 生命周期；跨 crate 构造点以 `rg` 核对到 `pkg/session/runtime.rs`、`pkg/session/runtime/session_factory.rs` 与 `pkg/testkit/mockstore.rs`。
- Go 对照：读取 `pkg/domain/domain.go` 的 `Domain`、`InfoSchema`、`GetSnapshotInfoSchema`、`Reload`、`Close`、`NewDomainWithEtcdClient`；读取 `pkg/meta/meta.go` 的键布局、`NewMutator`、全局 ID 与 schema version 语义。Go 中没有同路径同名文件，因此文档明确按职责映射而非虚构一对一实现。
- Rust 测试：读取 `pkg/domain/canonical_domain_test.rs`。直接证据包括 Go 格式元数据无私有 catalog 也可加载并被 Rust 更新、canonical storage 的加载/重载/快照/关闭链、`AllowedOnAlmostFull`、空数据库可见、保留系统 ID 不推进用户分配器、同版本 Go 元数据删除优先、changing index Unicode folding，以及 TTL 外部回调失败时元数据不删除。
- Go 测试：读取 `pkg/domain/domain_test.go` 的 schema reload、Domain close 和外部 workload 相关用例，作为生命周期与行为意图的旁证；本任务没有运行 Go 或 Cargo 测试，因为只新增分析文档且任务明确禁止 Cargo。
- 交付检查：使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节；另以 `git diff --check` 检查文档空白错误，并人工确认所有现状、限制和扩展建议都能回溯到上述源码、调用边、Cargo 或测试证据。

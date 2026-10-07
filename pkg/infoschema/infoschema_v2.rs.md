# [`pkg/infoschema/infoschema_v2.rs`](infoschema_v2.rs)

## 文件定位

该文件属于 `astersql-infoschema` crate（入口与依赖见 `pkg/infoschema/Cargo.toml`、`pkg/infoschema/lib.rs`），实现 `InfoSchema` trait 的 V2 版本。它把多个 schema 版本的库、表、分区和反向外键元数据保存在共享的 `Data` 中，再由 `infoschemaV2` 固定 `schema_meta_version` 与 `start_ts`，形成可并存的只读快照。

`pkg/infoschema/lib.rs` 将 `Data` 以 `V2Data`、将 `NewData` 以 `NewV2Data` 对外再导出，同时直接导出 `infoschemaV2`、`NewInfoSchemaV2`、`IsV2` 与 `IsSpecialDB`。生产接线有两条直接证据：`pkg/infoschema/builder.rs` 在全量加载和 schema diff 应用时更新共享 `Data`；`pkg/domain/canonical_domain.rs::KvInfoSchemaLoader::new_v2` 与 `build_info_schema_v2` 使用同一 `Arc<Data>` 构造不同 KV 时间点的快照。

## 核心职责

- 维护版本化元数据。`VersionedData` 分别按表 ID、库表名、库名、库 ID、分区 ID 和被引用父表保存历史；每次写入由 `insert_table`、`insert_schema`、`insert_partition`、`insert_fk` 保持 schema 版本降序且同版本覆盖。
- 提供快照可见性。`visible_table`、`visible_schema`、`visible_partition`、`visible_fk` 选择首个 `schema_version <= 快照版本` 的记录，并用 `tomb` 阻断已删除对象继续可见。
- 实现 `InfoSchema` 查询面。`infoschemaV2` 支持按名/ID 查库表、分区反查、遍历、特殊属性筛选、placement bundle/policy 与 masking policy 快照访问。
- 管理表对象缓存。`Data::table_cache` 是以 `(table_id, schema_version)` 为键的 `Sieve`；`TableByName` 和 `TableByID` 先查版本索引，再查缓存，未命中时把记录中的 `Table` 回填。
- 支持元数据生命周期操作。`add`/`remove`、`addDB`/`deleteDB`、`resetBeforeFullLoad` 和 `GCOldVersion` 分别承担增量写入、tomb 删除、全量重载隔离和旧版本压缩。

## 主要符号

- `TableCacheKey { table_id, schema_version }`：缓存身份同时包含物理表 ID 与实际记录版本，避免不同 schema 快照错误共享表实体。
- `TableRecord`、`SchemaRecord`、`PartitionRecord`、`ForeignKeyRecord`：四类版本记录；前三类保存对象或映射，`ForeignKeyRecord` 保存某父表在一个版本下的完整反向引用列表。
- `VersionedData`：所有历史索引的锁内载荷。`by_id` 与 `by_name` 是同一表历史的两套查询索引；`specials` 单独保存 information/performance/metrics/inspection 等特殊库及内存表，不走版本历史。
- `Data`：跨快照共享的可变后端。`inner: RwLock<VersionedData>` 保护索引，`table_cache: Sieve<...>` 管理缓存，`recent_min_ts: AtomicU64` 记录观察到的最小读时间戳，`temporary_table_ids` 跟踪全局临时表。
- `Data::add`：同时写表 ID/名称、分区及反向外键索引，然后在释放 `inner` 写锁后写缓存。反向外键列表会去重并按子库、子表、外键名的小写值排序。
- `Data::remove` / `remove_by_id`：追加表 tomb；删除子表时，根据删除前一版本的 `Table` 从父表反向外键列表中移除对应引用。
- `Data::GCOldVersion`：每轮最多从名称索引收集 1024 条旧记录，并用 `(table_id, schema_version)` 同步清理 ID 索引；反向外键历史保留 cut 之前的一个 pivot 记录。
- `Data::resetBeforeFullLoad`：为现存的所有派生索引在新版本追加 tomb，使旧快照仍可读，而新版本在重新加载前看不到旧对象。
- `infoschemaV2`：具体快照，除共享 `Data` 外还持有不可变的版本/时间戳，以及本快照的 placement、masking 状态。
- `impl InfoSchema for infoschemaV2`：对外契约实现；`IsV2` 固定返回 `true`，`GCOldVersion` 暴露共享后端压缩能力。
- `NewData`、`NewInfoSchemaV2`：分别构造共享后端和某版本快照；`IsSpecialDB` 判定四个内建特殊库名；`RefillOption`/`WithRefillOption` 当前仅是布尔包装构造器，未接入查表路径。

## 执行流程

1. 初始化时，`NewData` 创建空 `VersionedData`、默认 1 GiB 的 SIEVE 缓存、零值最小时间戳与空临时表集合。`Builder::InitWithDBInfos` 或 `canonical_domain::build_info_schema_v2` 先调用 `resetBeforeFullLoad(version)`，再用 `addDB` 和 `add` 写入该版本的库表。
2. 增量 DDL 时，`Builder::applyCreateSchema`/`refresh_schema` 写 `addDB`，`apply_table_ids` 写 `add`，`applyDropTable` 经 `remove_by_id` 追加 tomb，`applyDropSchema` 调用 `deleteDB`。因此旧快照和新快照共享存储但按版本看到不同结果。
3. `NewInfoSchemaV2(data, schema_meta_version, start_ts)` 固定一次读视图；`CloneAndUpdateTS` 保留版本、placement 和 masking 状态，只替换读时间戳。
4. 查表时，`TableByName`/`TableByID` 先调用 `keep_alive(start_ts)`，随后在 `by_name`/`by_id` 中选择可见记录。名称查询若命中特殊库则直接返回其内存表。普通表用记录版本组成 `TableCacheKey`：命中返回缓存；未命中则取记录携带的 `Table` 并回填缓存。
5. 查库、分区和外键时，`SchemaByName`/`SchemaByID`、`TableIDByPartitionID`、`GetTableReferredForeignKeys` 分别通过对应版本索引查询；`FindTableByPartitionID` 再组合表、库和具体 `PartitionDefinition`。
6. GC 时，调用者经 trait 的 `GCOldVersion` 进入 `Data::GCOldVersion`，在持有写锁期间按全局 1024 条上限压缩表历史并同步两套表索引。

## 数据与状态

所有历史向量均按 schema 版本降序。可见性不等同于“找到任意非 tomb 记录”：辅助函数先找到不晚于快照版本的第一条记录，再检查其 `tomb`；因此较新的删除会正确遮蔽更老的实体。

`addDB` 会清空 `DBInfo.tables` 后存入 schema 历史，表实体由独立表索引管理。这一分离使库元数据快照不会复制完整表集合。`schema_id_to_name` 另存 `(version, name, tomb)`，`SchemaByID` 先解析版本化名称，再通过名称索引取库对象。

`Data` 中的元数据历史、缓存和临时表集合跨所有 `infoschemaV2` 快照共享；`bundles`、`policies`、`masking_cache`、`masking_loaded` 与 `masking_loader` 则属于单个快照，构造后通过 builder-style 方法注入。`recent_min_ts` 只会由 0 变为首个时间戳或变得更小；本文件没有读取/重置它的公开接口，因此它在当前文件内只承担保活状态记录。

`specials` 使用 `HashMap::entry(...).or_insert(...)`：同名特殊库的后续登记不会覆盖首次登记。普通遍历基于 `HashMap`，除 `ListTablesWithSpecialAttribute` 的显式降序排序外，`AllSchemas`、`SchemaTableInfos` 和 `IterateAllTableItems` 不承诺稳定顺序。

## 依赖与调用关系

上游写入者主要是 `pkg/infoschema/builder.rs`：它把 `SchemaDiff` 和 `MetadataReader` 结果翻译为 `Data` 的版本写入；`NewBuilder` 还把配置的 schema cache size 传给 `SetCacheCapacity`。另一生产上游是 `pkg/domain/canonical_domain.rs`，其 `KvInfoSchemaLoader` 可选择 V2，并让连续加载的 catalog 共用历史与缓存。

上游读取者通过 `pkg/infoschema/infoschema.rs::InfoSchema` trait 使用快照，因此 session/planner/executor 不必依赖具体 V2 类型。`pkg/infoschema/lib.rs` 建立 crate 公开边界。RustCodeGraph 对目标文件给出的直接使用文件包括 `pkg/domain/canonical_domain.rs`、`pkg/infoschema/builder.rs` 及相应测试；对 `TableByName` 的 callee 结果确认其调用 `keep_alive`、`visible_table`、SIEVE `Get` 并构造 `TableCacheKey`。

下游 crate 依赖来自 `pkg/infoschema/Cargo.toml`：本文件直接使用同 crate 的 `infoschema` 数据模型与 `sieve`，并以 `astersql-infoschema-context` 的 `SpecialAttributeFilter`/`TableInfoResult` 实现特殊属性列表；结果中的数据库名通过 `astersql-parser-ast::NewCIStr` 构造。Cargo 中大量依赖位于 `target.'cfg(any())'`，不会成为当前常规构建的有效依赖，不能据此推断本文件已接入那些子系统。

## 错误处理与边界

`TableByName` 在普通索引找不到可见记录时返回 `InfoSchemaError { code: "ErrNoSuchTable", message: "schema.table" }`；可见记录意外没有 `Table` 时也返回同码错误，但消息仅为表名。按 ID 查询、查库、分区反查和外键查询使用 `Option` 或空列表表达不存在。`SchemaTableInfos` 即使 schema 不存在也返回 `Ok(empty)`，与 trait 注释和 `test_v2_basic` 一致。

所有标准库 `RwLock` 获取都使用 `expect(...)`；锁中毒会 panic，而不是转成 `InfoSchemaError`。`Data::remove_by_id` 在删除前版本找不到表时返回 `false`，调用者可区分“已追加 tomb”和“无可删除对象”。版本减一使用 `saturating_sub(1)`，避免最小 `i64` 下溢。

`GCOldVersion` 只压缩表和反向外键历史，没有在本函数中压缩 schema、schema ID 或分区历史；调用者不能把返回的删除数理解为所有索引的总回收数。`resetBeforeFullLoad` 只给已有索引追加 tomb，不清空 `specials`、缓存、临时表 ID 或快照附属策略。`RefillOption` 当前没有影响 `TableByName` 的分支，也没有 Go 版 context option 的语义。

## 并发与资源生命周期

`Arc<Data>` 定义共享生命周期：只要任一快照、Builder 或 loader 持有它，历史与缓存就继续存在。`inner` 的读操作可并发，所有版本写入、重置与 GC 串行占用写锁。`Data::add` 在写缓存前显式 `drop(data)`，`TableByName` 也在访问缓存前释放读锁，减少索引锁与 SIEVE 内部同步重叠；`TableByID` 的临时读 guard 则在语句结束后释放。

`temporary_table_ids` 使用独立 `RwLock<HashSet<i64>>`，不会为临时表状态阻塞全部版本索引。`recent_min_ts` 用 Acquire/AcqRel 的 CAS 循环无锁更新最小值。表缓存容量调整由 `SetCapacityAndWaitEvict` 完成，函数名及 `infoschemav2_cache_test.rs` 的事件断言表明调用会等待淘汰完成。

当前 Rust 实现采用单个 `RwLock<VersionedData>`；Go 对照采用多个原子指针指向的泛型 BTree，并对缺页加载使用全局 `singleflight.Group`。因此 Rust 的正确性边界是锁保护的进程内历史和已携带的 `Table`，不能假定具备 Go 版的无锁读、按需从 meta 加载或同等并发伸缩特性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/infoschema_v2.go`。概念映射如下：Go 的 `Data`、`infoschemaV2`、`tableItem`/`schemaItem`/`partitionItem`/`referredForeignKeyItem` 分别对应 Rust 的 `Data`、`infoschemaV2` 与四类 `*Record`；Go 的多个 BTree 比较器和 `search` 在 Rust 中由按版本降序的 `Vec` 加 `visible_*` 线性选择替代。

保留的语义包括：版本可见性与 tomb、ID/名称双索引、分区反查、反向外键增删、全量加载前的索引遮蔽、每轮最多 1024 条表历史 GC、SIEVE 缓存、特殊库优先查询、placement/masking 透传以及特殊属性表按降序分组。`pkg/infoschema/infoschema_v2_test.rs` 中的 reset 与全局 GC 上限用例，以及 `pkg/infoschema/infoschemav2_cache_test.rs` 的命中/未命中/驱逐计数，为这些行为提供 Rust 回归证据。

尚未一一移植的 Go 行为也必须视为边界：Go `NewInfoSchemaV2` 接受 autoid requirement 与 resource factory；Go `loadTableInfo` 可按需读取 meta 并由 `singleflight` 合并并发加载；Go `WithRefillOption` 返回携带 context value 的新 context；Go 文件还包含更多 Builder V2 辅助与缓存指标路径。Rust 构造器直接接收版本和时间戳，表数据随 `TableRecord` 保存，`RefillOption` 只是值类型。两者 API 和性能模型并非完全等价。

相关 Go 测试包括 `pkg/infoschema/infoschema_v2_test.go`、`pkg/infoschema/infoschemav2_cache_test.go` 与 `pkg/infoschema/test/infoschemav2test/v2_test.go`；相应 Rust 测试除两个同目录文件外，还有 `pkg/infoschema/test/infoschemav2test/v2_test.rs`，覆盖特殊库、分区、快照、缓存和 GC 场景。

## 扩展指南

- 新增版本化索引时，应同时定义记录类型、在 `VersionedData` 中增加容器、提供“同版本替换并降序”的插入函数和“先选版本再判 tomb”的可见性函数；还要同步 `resetBeforeFullLoad` 与 GC 策略，避免全量重载后派生索引泄漏旧状态。
- 修改表写入/删除时，必须保持 `by_id` 与 `by_name` 对称，并同步分区及反向外键派生索引。涉及重命名或换 ID 的逻辑应从 `pkg/infoschema/builder.rs::apply_table_ids`/`applyDropTable` 进入，不应绕过 `Data`。
- 改动缓存键或装载流程时，要维护“记录实际版本而非快照目标版本”这一不变量，并同步 `pkg/infoschema/infoschemav2_cache_test.rs`。若移植 Go 的懒加载/singleflight，需设计失败传播、并发去重、refill option 与缓存指标，而不能仅返回占位表。
- 增加 `InfoSchema` 能力时，应先更新 `pkg/infoschema/infoschema.rs` 的 trait，再在 V1/V2 分别实现；V2 专属方法则应评估是否需要通过 `pkg/infoschema/lib.rs` 再导出。
- Rust 测试必须继续放在独立测试文件：核心版本行为放 `pkg/infoschema/infoschema_v2_test.rs`，缓存事件放 `pkg/infoschema/infoschemav2_cache_test.rs`，跨 loader/DDL 的场景放 `pkg/infoschema/test/infoschemav2test/v2_test.rs`；同时对照对应 Go 测试，不能用简化断言替代 Go 的实际语义。
- 性能风险集中在单一 `RwLock`、历史向量线性扫描和 `HashMap` 全量遍历；任何优化都需保留快照隔离、tomb 遮蔽、双索引一致性及旧快照在 reset/GC 前后的可读性。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/infoschema` 确认目标及 Go/Rust 测试均在图中。
- RustCodeGraph 源码读取：`node --file pkg/infoschema/infoschema_v2.rs --offset 1/261/521/781` 覆盖目标文件全部 956 行，并列出其被 `pkg/domain/canonical_domain.rs`、`pkg/infoschema/builder.rs` 等文件使用。
- RustCodeGraph 符号/调用查询：`query NewInfoSchemaV2`、`query infoschemaV2`、`query GCOldVersion`；`callees TableByName` 验证 Rust 查表路径通向 `keep_alive`、`visible_table`、`TableCacheKey` 和 SIEVE `Get`。图对部分 Rust callers 未返回结果，因此直接读取上述生产接线文件补证。
- crate 与入口：`pkg/infoschema/Cargo.toml`、`pkg/infoschema/lib.rs`；trait 契约：`pkg/infoschema/infoschema.rs`；生产调用：`pkg/infoschema/builder.rs`、`pkg/domain/canonical_domain.rs`。
- Go 对照：`pkg/infoschema/infoschema_v2.go`；测试证据：`pkg/infoschema/infoschema_v2_test.rs`、`pkg/infoschema/infoschemav2_cache_test.rs`、`pkg/infoschema/test/infoschemav2test/v2_test.rs` 及同路径三个 Go 测试文件。
- 本任务仅新增说明文档，未运行 Cargo。交付前以任务给定命令验证文件存在且恰有 11 个固定二级标题，并人工复核符号名、路径、当前限制与扩展接入点均有上述源码证据。

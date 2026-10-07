# `pkg/infoschema/issyncer/loader.rs`

对应源码：[`loader.rs`](loader.rs)。

## 文件定位

本文件属于 `astersql-infoschema-issyncer` crate（入口与依赖见 `pkg/infoschema/issyncer/Cargo.toml`），由 `lib.rs` 的 `mod loader` 纳入并通过 `pub use loader::*` 重新导出。它位于元数据同步链的中段：`Syncer::ReloadWithContext` 取得存储时间戳并调用 `Syncer::LoadWithTS`，后者直接委托本文件的 `Loader::LoadWithTS`；加载结果随后交给 schema validator 更新版本，并由 `Syncer::postReload` 处理特定表缓存或连接清理（`syncer.rs:405-493`）。

文件同时承担三层职责：定义加载器所需的存储接口 `SchemaReader`/`SchemaStore`，实现缓存、增量与全量加载决策，以及把真实 `astersql_kv::Storage` 适配成能够读取 Go meta 编码的 `KvSchemaStore`/`KvMetaReader`。因此它不是单纯的算法工具，也不是独立持久化层；它把 InfoSchema Builder 与某个时间戳上的 KV 元数据连接起来。

## 核心职责

1. `Loader::LoadWithTS` 在一次调用中只使用 `startTS` 对应的同一个不可变 reader，确定最新“具有非空 diff 的版本”，并返回该版本的 `SchemaInfo`。
2. 优先复用 `InfoCache.byVersion`；未命中时，仅在非快照、已有基线、目标版本更新且版本差小于 `LoadSchemaDiffVersionGapThreshold`（10000）时尝试增量加载。
3. `tryLoadSchemaDiffs` 按版本顺序取得并应用 diff；Filter 或 cross-keyspace 规则可以跳过内容，但仍推进 Builder 的 schema 版本。增量失败不会直接终止加载，而是回退到全量路径。
4. `fetchAllSchemasWithTables` 全量枚举数据库和表。普通模式可应用 `Filter`，并固定注入 `information_schema` 与 `metrics_schema`；cross-keyspace 模式只接受 `metadef::SystemDatabaseID` 对应的系统库。
5. `KvMetaReader` 复现 Go meta 的 string/hash key 编码、JSON schema diff 解码和 DB/Table model 解码；`KvSchemaStore` 为每次加载创建指定时间戳的快照 reader。
6. 将成功结果同时写入本地 `LoaderCache` 和共享的 `astersql_infoschema::cache::InfoCache`，使同步器与 Domain/keyspace 侧读到同一版本。

## 主要符号

- `LoadSchemaDiffVersionGapThreshold: i64 = 10000`：决定是否允许尝试增量加载；等于或超过阈值时直接全量。
- `InformationSchemaID`、`MetricsSchemaID`：普通全量加载时注入两个内存虚拟库所用的负 ID，不与真实库 ID 冲突。
- `SchemaReader`：一个不可变元数据视图，公开 schema 版本、diff、数据库和表读取。默认实现返回空值，方便仅关注 keyspace 的轻量测试桩；生产 reader 必须覆盖实际能力。
- `SchemaStore: SchemaReader + Send + Sync`：长期共享的存储句柄，额外提供缓存表删除、keyspace、当前存储版本及 `Snapshot(start_ts)`。`Snapshot` 返回的 `SchemaReader` 本身不要求 `Send + Sync`，因为只在当前加载线程消费。
- `LoaderCache`：保存 `latest` 与 `byVersion: HashMap<i64, SchemaInfo>`。此 Map 没有本文件内的淘汰逻辑。
- `InfoCache`：以 `Mutex<LoaderCache>` 保护加载状态，并持有共享的 `astersql_infoschema::cache::InfoCache`。`publish` 优先复用共享缓存中版本一致的快照，否则由 `SchemaInfo::CompleteInfoSchema` 构建后插入。
- `Loader`：核心对象，字段是可选 `store`、可选 `filter`、`crossKS` 标志和共享 `cache`。`newLoader` 构造普通实例；`NewLoaderForCrossKS` 强制 cross-keyspace 且不设置 Filter。
- `Loader::LoadWithTS`：主入口，返回 `(SchemaInfo, cache_hit, old_version, Option<RelatedSchemaChange>)`。
- `skipLoadingDiff`/`skipLoadingDiffWithLatest`：先应用调用方 Filter，再应用 cross-keyspace 的保留表 ID 规则。
- `tryLoadSchemaDiffs`：用 `astersql_infoschema::builder::Builder` 从旧 schema 初始化并逐个 `ApplyDiff`，汇总物理表 ID 和动作类型。动作码 30、31 不加入 schema checker 变更集合。
- `fetchAllSchemasWithTables`：全量读取路径；普通模式注入虚拟库，cross-keyspace 缺系统库时返回错误。
- `BuilderReader`：把本地 `SchemaReader` 转为 Builder 所需的 `MetadataReader`，并将 `SyncError` 转成字符串错误。
- `KvMetaReader`：持有真实 KV `Snapshot`，设置内部请求来源与 3000 ms 读取超时，实现 Go meta key/value 解码。
- `GoDiff`、`GoAffected`：只用于 serde 反序列化 Go JSON 字段，再转换为共享 `SchemaDiff`/`AffectedOption`。
- `KvSchemaSource`、`KvSchemaStore`：隔离真实 `astersql_kv::Storage` API，并实现生产用 `SchemaReader`/`SchemaStore` 适配。

## 执行流程

`Loader::LoadWithTS(startTS, isSnapshot)` 的路径如下：

1. 验证存在 backing store；否则返回 `loader has no backing store`。
2. 调用 `SchemaStore::Snapshot(startTS)`。若适配器返回 reader，则该次调用的版本、diff、库表读取都绑定此 reader；轻量 store 返回 `None` 时直接使用 store 本身。
3. 通过 `MaxDiffVersion` 取得目标版本。`KvMetaReader` 先读 `SchemaVersionKey`；若当前版本大于零但对应 `Diff:<version>` 不存在，则目标版本回退一位，避免把尚无已提交 diff 的版本当作可加载版本。
4. 短暂锁住 `LoaderCache`，取得旧 `latest`、旧版本及目标版本缓存。若 `byVersion` 命中，则再次加锁更新 `latest`，发布共享快照，返回 `cache_hit=true`、旧版本字段为 0、无变更集合。
5. 若 `!isSnapshot` 且旧版本非零、目标版本更大、版本差小于 10000，则用旧 `latest` 调用 `tryLoadSchemaDiffs`。该函数以旧库信息初始化 Builder，遍历 `(usedVersion, newVersion]`：缺失 diff 被安全略过；应过滤的 diff 只推进版本；`RegenerateSchemaMap` 立即报错；其余 diff 经 `Builder::ApplyDiff` 应用并记录相关物理表。构建后再次从 reader 回填完整 DB model。成功则缓存、发布并返回增量变更。
6. 增量条件不满足或增量返回错误时，调用 `fetchAllSchemasWithTables`。cross-keyspace 读取系统库及其表；普通模式列出所有库、按 Filter 删去不需要的库、逐库列表，再加入两个内存虚拟库。
7. 全量结果以目标版本写入两层缓存并返回；全量的 `RelatedSchemaChange` 为 `None`。

真实 KV 路径中，`KvSchemaStore::Snapshot(start_ts)` 构造 `KvMetaReader`。reader 将 snapshot 标记为 internal meta 请求；`GetSchemaDiff` 从 string key 读取 JSON，`GetDatabase`/`GetTable` 从 hash field 读取并调用 `astersql_meta_model` 解码，列表操作以 hash 前缀扫描。`scan` 无论循环成功还是失败都会调用 iterator 的 `Close`。

## 数据与状态

- `SchemaInfo` 是加载器的值快照，包含版本、数据库及表；`latest` 和 `byVersion` 保存 clone，调用者不能通过返回值直接修改缓存内部状态。
- `RelatedSchemaChange` 仅由成功的增量路径生成；`PhyTblIDS` 与 `ActionTypes` 按 Builder 返回的 ID 数量一一扩展。全量、缓存命中和初次加载均返回 `None`。
- `InfoCache.state` 的临界区只覆盖查找或更新，不包住 KV IO 和 Builder 工作，避免在慢读取期间长期持锁；但并发加载可基于同一旧快照分别计算，最终 `latest` 由最后一次完成的写入决定，本文件没有版本 CAS。
- `byVersion` 是无界 `HashMap`；`changeSchemaCacheSize` 当前是 no-op，不能据此推断本地版本缓存会被缩容。
- `KvMetaReader` 的 snapshot 独占于 reader。`KvSchemaStore` 持有共享源，不拥有其关闭生命周期；每次非快照 trait 读取会通过 `current_meta_reader` 获取当时最新时间戳，而 `LoadWithTS` 的生产路径使用显式 `Snapshot(startTS)` 保证一次加载内一致。
- `GoDiff` 用 `#[serde(default)]` 容忍 Go JSON 中缺省字段；未知动作码通过 `ActionType::from_code` 保留为共享动作表示，而不是在反序列化阶段拒绝。

## 依赖与调用关系

上游主要调用边为 `Syncer::ReloadWithContext → Syncer::LoadWithTS → Loader::LoadWithTS`（`syncer.rs:421-473`）。`Syncer::InfoSchema` 调用 `Loader::latest`，`Syncer::ChangeSchemaCacheSize` 调用当前 no-op 的 `Loader::changeSchemaCacheSize`，`Syncer::postReload` 则根据增量结果调用 `delete_cached_table`。独立测试也直接构造 Loader 覆盖各分支。

下游关系包括：

- `Loader::LoadWithTS → SchemaStore::Snapshot/SchemaReader::MaxDiffVersion`，再进入缓存、`tryLoadSchemaDiffs` 或 `fetchAllSchemasWithTables`。
- `tryLoadSchemaDiffs → astersql_infoschema::builder::Builder::{InitWithDBInfos, ApplyDiff, Build}`，并经 `BuilderReader` 回调 `GetDatabase`/`GetTable`。
- `KvSchemaStore → KvSchemaSource → astersql_kv::Storage`；`KvMetaReader → astersql_kv::Snapshot`、`astersql_util_codec`、`serde_json`、`astersql_meta_model`。
- cross-keyspace 判断依赖 `metadef::SystemDatabaseID` 与 `metadef::IsReservedID`。

`Cargo.toml` 证明该 crate 直接依赖 `astersql-infoschema`、`astersql-kv`、`astersql-meta-model`、`astersql-util-codec`、`serde`、`serde_json` 和 `astersql-meta-metadef`；测试另依赖 mockstorage 与 parser AST。`lib.rs:199-223` 证明 loader 与 `deferfn`、`filter`、`mdl_check`、`syncer` 同属一个 crate，测试通过独立的 `loader_test.rs` 模块接入，而非内嵌在生产源文件。

## 错误处理与边界

- 无 store、cross-keyspace 缺系统库、DB 不存在、KV 读取/迭代失败、codec/JSON/model 解码失败和 Builder 应用失败都转换为 `SyncError`。
- `KvMetaReader::get` 只把 `IsErrNotFound` 转成 `Ok(None)`；其他 KV 错误保留为失败。`ListTables`/`GetTable` 在库不存在时明确报错，而非把不存在的库伪装为空库。
- `tryLoadSchemaDiffs` 对缺失 diff 继续推进最终版本，这与 Go 注释所述“生成 schema version 的事务已提交而 runDDLJob diff 尚未写入/失败时可安全跳过”一致；但遇到 `RegenerateSchemaMap` 会拒绝增量。
- 增量路径的任何错误都被 `LoadWithTS` 吞下并触发全量回退；只有全量路径也失败时调用者才收到错误。因此排障时不能仅凭最终成功判断增量曾经成功。
- `Mutex::lock().unwrap()` 在锁中发生 panic 后会因 poison 再次 panic；本文件未将该情况包装为 `SyncError`。
- `startTS` 被交给存储适配器，加载器自身不验证时间戳范围。`currentVersion` 将 `u64` 转为 `i64`，极端超出 `i64::MAX` 时会按 Rust `as` 语义截断；正常 TiDB 时间戳范围是其隐含前提。
- cross-keyspace 的 diff 规则只检查 `TableID` 与 `OldTableID`，不检查 `AffectedOptions`；这与 Go 侧禁止系统表进行涉及多表 ID 的相关 DDL 的前提一致。

## 并发与资源生命周期

`Loader` 可共享的基础来自 `Arc<dyn SchemaStore>`、`Arc<InfoCache>` 与 `Mutex<LoaderCache>`。缓存锁不会跨越存储读取；共享 InfoSchema cache 自行负责其内部同步。`SchemaStore` 必须 `Send + Sync`，但一次加载持有的 snapshot reader 不必跨线程，明确限制了资源移动范围。

本文件不创建线程、异步任务或通道。与 Go 全量路径不同，Rust 的 `fetchAllSchemasWithTables` 串行枚举数据库和表，没有 error group 或最高 128 的并发拉取；这降低了并发复杂度，也意味着大量 schema 下可能有不同的延迟特征。KV iterator 在 `scan` 末尾显式关闭，即使内部解码或 `Next` 失败也会先形成结果再 `Close`。`KvSchemaStore` 只保留共享 storage，不关闭它；snapshot 的释放随 `KvMetaReader` drop 完成。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/issyncer/loader.go`，相关测试为同目录 `loader_test.go` 与 `loader_test.rs`。已保持的主要语义包括：10000 的 diff 版本差阈值、缓存优先、只从已有 latest 向前增量、增量失败回退全量、按版本顺序应用 diff、跳过内容仍推进版本、`RegenerateSchemaMap` 拒绝增量、Filter 优先于 cross-keyspace 判断、cross-keyspace 只加载系统库/保留表 diff，以及普通全量加载始终包含两个内存 schema。

Rust 生产适配还对应 Go 的 `kv.Storage.GetSnapshot + meta.NewReader`：设置 3000 ms 读超时，读取 `SchemaVersionKey`/`Diff:<version>`、`DBs` 与 `DB:<id>` hash，并使用共享 model codec 保留完整 DB/Table metadata。Rust 测试用 `InMemoryStore` 代替 Go `mockstore/meta.Mutator`，覆盖初始全量、重复缓存命中、增量新表、cross-keyspace、BR Filter 及未知库等真实分支。

当前 Rust 并非 Go 文件的完整功能等价实现，扩展时必须正视这些差异：没有 `LoadMode` 和 v1/v2 切换；没有 schema diff commit timestamp/MVCC 查询；没有策略、资源组和 masking policy 的全量拉取；没有 repair mode、字符集修正、lazy v2 表信息或并发 schema 拉取；没有加载指标、日志和 failpoint；`changeSchemaCacheSize` 是占位；没有 AutoID client 与内部 SQL executor 初始化。Go 会围绕 snapshot history、v1/v2 数据对象和延迟释放做额外处理，Rust 当前只使用统一 Builder/共享 cache，不应在文档或调用方中宣称这些功能已移植。

## 扩展指南

- 新增元数据种类时，先扩展 `SchemaReader`，再同步 `BuilderReader`（若 Builder 需要）与 `KvMetaReader` 的真实 Go 编码读取；同时在 `KvSchemaStore` 的委托实现和 `loader_test.rs` 的 `InMemoryStore` 中补齐，避免默认空实现掩盖生产缺口。
- 修改增量判定或 diff 过滤时，应聚焦 `LoadWithTS`、`tryLoadSchemaDiffs`、`skipLoadingDiffWithLatest`，并同步 Rust 独立测试中普通、Filter、cross-keyspace 三类断言；还要对照 `loader.go`，避免动作码、空 diff 或版本推进语义漂移。
- 修改全量内容时，在 `fetchAllSchemasWithTables` 接入；必须保持普通模式的虚拟库注入和 cross-keyspace 系统库约束。若移植 Go 的 policy/resource-group/masking policy，应通过 Builder 的正式模型能力接线，不能只在 `SchemaInfo` 旁挂未消费数据。
- 若实现缓存容量或淘汰，应同时区分本地 `LoaderCache.byVersion` 与共享 `astersql_infoschema::cache::InfoCache`；当前 `changeSchemaCacheSize` 只占位，不能只改函数名义行为而遗漏两层缓存一致性。
- 若引入并行全量加载，需要证明 `SchemaReader`/snapshot 的线程安全边界；当前 trait 有意允许 snapshot reader 非 `Send + Sync`，直接在线程间共享会破坏接口约定。
- 测试应继续放在同目录独立文件 `pkg/infoschema/issyncer/loader_test.rs`，并尽量逐分支对齐 `loader_test.go`。生产 KV 编码变化还应增加针对 `KvMetaReader` 的独立测试，重点覆盖 not-found、损坏 JSON/model、缺库、iterator 关闭与同一 startTS 一致性。

兼容性风险主要是 Go meta 编码和动作码漂移；正确性风险集中在并发加载覆盖 `latest`、过滤后版本推进和增量错误回退；性能风险集中在无界 `byVersion` 与串行全量表扫描。任何扩展都应分别验证这三类风险。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11467 个文件、307296 个节点和 1848419 条边；`files --filter pkg/infoschema/issyncer` 确认目标 Rust/Go 源与独立测试均已索引。
- RustCodeGraph `node --file pkg/infoschema/issyncer/loader.rs --offset 1 --limit 1200`：读取目标文件完整 731 行，核对所有常量、trait、struct、impl 和函数。
- RustCodeGraph `query LoadWithTS`、`query tryLoadSchemaDiffs`、`query fetchAllSchemasWithTables`、`query KvMetaReader --kind struct`、`query KvSchemaStore --kind struct`：确认 Rust/Go 对应符号与位置。
- RustCodeGraph `node` 读取 `pkg/infoschema/issyncer/lib.rs` 与 `syncer.rs:340-519`：确认模块导出及 `ReloadWithContext`、validator、postReload 的直接调用链。
- `pkg/infoschema/issyncer/Cargo.toml`：核对 crate 边界、生产依赖和独立测试依赖。
- RustCodeGraph `node` 完整读取 `pkg/infoschema/issyncer/loader.go`：核对 Go 主流程、diff/full-load 行为及尚未移植的 v1/v2、并发、policy、指标等能力。
- RustCodeGraph `node` 读取 `pkg/infoschema/issyncer/loader_test.rs` 与 `loader_test.go`：核对首次全量、缓存命中、增量、cross-keyspace、BR Filter 的边界和预期。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前以任务指定命令验证本文恰含 11 个固定二级章节，并人工复核只新增本文件且未修改 Rust、Go、Cargo 或总计划。

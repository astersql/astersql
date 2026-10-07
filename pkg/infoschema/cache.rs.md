# `pkg/infoschema/cache.rs`

## 文件定位

本文件属于 `astersql-infoschema` crate，模块入口是 `pkg/infoschema/lib.rs` 中的 `pub mod cache`，并由 crate 根重新导出 `Data`、`InfoCache`、`NewCache` 和 `SchemaRef`。它位于元数据加载器与会话可见 `InfoSchema` 快照之间：`pkg/domain/domain.rs` 创建主 keyspace/附加 keyspace 的缓存，将加载完成的快照写入缓存，并在普通读取和历史 MVCC 读取时复用缓存；`pkg/infoschema/issyncer/loader.rs::InfoCache` 还把同一个缓存包装为 schema 同步器的共享快照层。

`pkg/infoschema/Cargo.toml` 将该目录定义为 `astersql-infoschema`，crate 根为 `lib.rs`。本文件自身只依赖 Rust 标准库以及同 crate 的 `infoschema::InfoSchema` trait，不直接依赖存储、元数据或指标 crate。

## 核心职责

- 用 `InfoCache` 保存有限数量的 `InfoSchema` 快照，并维持 schema version 降序排列（`CacheState::cache`，最新版本在索引 0）。
- 通过 `GetByVersion` 支持精确版本查询，以及在已知连续历史窗口内回退到不大于目标版本的最近快照。
- 通过 `GetBySnapshotTS` 按 schema 生效时间戳查询历史快照；只有相邻版本连续，或版本缺口全部登记在 `empty_schema_versions` 中时，才跨过缺口返回旧快照。
- 在 `Insert` 中处理同版本补时间戳、V2 对象刷新、V1/V2 代际替换、容量淘汰和过旧条目拒绝。
- 用 `Data::recent_min_ts` 汇总一轮中观测到的最小 schema 时间戳，并由 `GetAndResetRecentInfoSchemaTS` 原子取出并开启下一轮。
- 支持运行时扩缩容、重置、V1/V2 切换式 `Upsert`，以及空 schema version 集合的有界维护。

本模块不负责构造 `InfoSchema`，也不从存储加载 schema；这些工作由 `Domain`/loader 完成。Rust 版本也没有实现 Go `gcOldVersion` 的存储侧旧版本回收。

## 主要符号

- `pub type SchemaRef = Arc<dyn InfoSchema>`：共享、动态分派的只读 schema 快照引用。缓存和调用者通过克隆 `Arc` 共享对象，而不复制完整元数据。
- `pub struct Data { recent_min_ts: AtomicU64 }`：轻量 min-TS 水位。`new` 创建零值，`recent_min_ts` 以 Acquire 读取，`keep_alive(ts)` 用弱 CAS 仅在当前值为 0 或 `ts` 更小时降低水位。
- `SchemaAndTimestamp`：内部条目，组合 `SchemaRef` 和 `i64` 时间戳；时间戳 0 表示未知，不能用于 snapshot-TS 命中。
- `CacheState`：锁内状态，包含降序 `cache`、逻辑 `capacity`、`empty_schema_versions`、`first_known_schema_version`，以及 GC 检查窗口的版本/时间记录。
- `pub struct InfoCache`：以 `RwLock<CacheState>` 保护全部缓存结构；公开 `Data: Arc<Data>` 供外部报告/读取 min-TS。
- `NewCache(capacity)`：建立指定容量的空缓存。与 Go 不同，它不接收 `kv.Storage`。
- `ReSize`、`Size`/`Len`、`Reset`：调整容量、读取长度、重建空条目向量。`ReSize` 缩容时保留前面的最新条目；扩容保留全部现有条目。
- `Upsert(schema, schema_ts) -> impl FnOnce()`：用单个快照替换整个缓存，设置 `first_known_schema_version`，并返回在锁外释放旧 `Arc` 集合的闭包。
- `GetLatest`、`GetByVersion`、`GetBySnapshotTS`：三个查询入口；均返回克隆后的 `SchemaRef`，锁不随引用离开函数。
- `get_schema_by_timestamp_no_lock`：调用者已持有读锁时使用的线性 timestamp 查找核心。
- `Insert(schema, schema_ts) -> bool`：排序插入/更新核心；成功缓存返回 `true`，满容量且新条目比所有缓存条目都旧时返回 `false`。
- `InsertEmptySchemaVersion`、`GetEmptySchemaVersions`：登记和复制读取无 schema diff 的版本集合。
- `gcCheckInterval = 128`：schema version 至少推进 128 以上，且距上次检查超过 60 秒时，才刷新 GC 检查窗口。

## 执行流程

1. `Domain::new_with_storage_handle`（`pkg/domain/domain.rs`）按配置创建 `Arc<InfoCache>`；keyspace runtime 也各自创建缓存。配置容量会先取 `max(1)`，因而主调用链不会创建零容量缓存。
2. loader 取得完整或增量构建后的 `SchemaRef` 与 schema commit timestamp。`Domain` 的初始化协调、DDL 后重载、周期 schema reload、TiFlash 状态变化和 keyspace 初始化路径调用 `Insert`；`pkg/infoschema/issyncer/loader.rs::InfoCache::publish` 也先按版本复用已有快照，再调用 `Insert` 发布。
3. `Insert` 读取 `SchemaMetaVersion()`，持写锁并刷新 GC 检查窗口。随后用 `partition_point(entry.version > version)` 找到降序插入点。
4. 若同版本已存在：同为 V1 或同为 V2 时，已缓存 timestamp 为 0 且新 timestamp 非 0 就补齐时间戳；否则 V2 会更新对象，V1 保留原对象。V1/V2 代际不同则替换对象和时间戳。以上分支均返回 `true`。
5. 若是新版本：未满容量时在插入点插入；已满但插入点位于窗口内时插入并弹出末尾最旧条目；比整个满缓存都旧则拒绝并返回 `false`。第一条插入会建立 `first_known_schema_version`。
6. 普通最新 schema 读取调用 `GetLatest`。历史版本读取调用 `GetByVersion`：它找到第一个版本不大于目标的条目，精确相等即可返回；非首项回退还要求该版本不早于 `first_known_schema_version`，防止 `Upsert` 后错误穿越丢失的 DDL 历史。
7. `Domain::snapshot_info_schema` 先调用 `GetBySnapshotTS`。其线性扫描跳过未知 timestamp 和晚于请求时间的条目；最新条目一旦适用可直接返回，旧条目则必须位于两个 timestamp 边界之间且版本连续，或全部中间版本已标记为空。缓存未命中时 Domain 回退到持久化快照加载。
8. `InsertEmptySchemaVersion` 插入版本号；集合超过容量时排序并从最小版本开始删除，直到重新满足上限。

## 数据与状态

`cache` 同时假设 schema version 与 timestamp 的顺序一致，并以版本降序为物理顺序。该不变量支撑 `partition_point` 的插入/版本查询，也支撑 timestamp 查询只线性扫描到首个不安全缺口就停止。`timestamp == 0` 是“未知”而非真实时间点；它会阻断相关历史区间的 snapshot 查询，后续同版本非零 timestamp 可修正它。

`first_known_schema_version` 描述缓存仍可证明连续的历史下界，而不是当前最旧条目的简单别名。`Upsert` 会将它重设为新快照版本，避免随后插入更旧历史快照后，将中间未知版本错误回退到过旧对象。普通淘汰和 `ReSize` 不重新计算该字段。

`empty_schema_versions` 表示对应 DDL version 没有 schema diff，因此版本缺口并不代表元数据变化。它独立于 `cache` 保存；`Reset` 和 `Upsert` 只替换条目向量，没有清除此集合。`GetEmptySchemaVersions` 返回克隆，调用者不能绕过锁修改内部集合。

`last_check_version` 与 `last_check_time` 只记录何时满足 Go 侧 GC 触发门槛。当前 Rust `Insert` 达到门槛后仅更新这两个字段，不执行 GC。`Data` 又是独立的 `Arc` 原子状态，不受 `CacheState` 的读写锁保护。

## 依赖与调用关系

上游直接证据：

- `pkg/domain/domain.rs::Domain::new_with_storage_handle` 和 `Domain::acquire_keyspace_runtime` 调用 `NewCache`。
- `pkg/domain/domain.rs::Domain::snapshot_info_schema` 调用 `GetBySnapshotTS`，未命中时调用持久化 loader。
- `pkg/domain/domain.rs` 的元数据协调、reload 和后台 worker 调用 `GetLatest` 与 `Insert`，只在版本推进时发布新快照。
- `pkg/infoschema/issyncer/loader.rs::InfoCache::publish` 调用 `GetByVersion` 与 `Insert`，并通过 `from_shared`/`snapshots` 与 Domain 共享底层缓存。

下游依赖只有 `InfoSchema::SchemaMetaVersion` 与 `InfoSchema::IsV2`：前者决定排序和版本边界，后者决定同版本更新策略。`Arc` 管理快照所有权，`RwLock` 管理结构并发，`HashSet` 管理空版本，`AtomicU64` 管理 min-TS，`Instant`/`Duration` 管理 GC 检查节流。

RustCodeGraph 已索引 `pkg/infoschema/cache.rs`（29 个符号），并识别 `pkg/domain/domain.rs`、`pkg/infoschema/issyncer/loader.rs` 及独立测试中的引用；由于 `InfoCache`、`Insert` 等名称在 Go/Rust 多模块高度重名，通用 flow 查询产生大量歧义，精确调用关系又由上述直接引用逐项核对。

## 错误处理与边界

该 API 不返回 `Result`。查询未命中使用 `Option::None`；插入过旧条目使用 `false`；其他插入/更新使用 `true`。所有 `RwLock` 获取都以 `expect("infoschema cache lock poisoned")` 处理锁中毒，因此持锁线程 panic 后，后续访问会继续 panic，而不是恢复或向上返回错误。

重要边界包括：空缓存的 `GetLatest`/查询返回 `None`；目标版本高于最新版本时 `GetByVersion` 返回 `None`；满缓存拒绝窗口之外的更旧版本；timestamp 为 0 的条目不可按时间查询；版本缺口未全部登记为空时不得返回跨缺口快照；`capacity == 0` 时插入恒被拒绝，但 Domain 主路径用 `max(1)` 避免此配置。`schema_ts as i64` 未检查大于 `i64::MAX` 的值，转换后为负数，随后转回 `u64` 参与比较；调用者必须保证传入有效的 TiDB timestamp 范围。

`Reset` 只清空快照向量并改变容量，不重置空版本集合、历史下界或 GC 检查字段；`ReSize` 也只调整向量和容量。扩展调用方不应把它们理解为恢复整个对象到 `NewCache` 初始状态。

## 并发与资源生命周期

结构读取（`Size`、`GetLatest`、两个查询、集合读取）持共享读锁；插入、扩缩容、重置、空版本登记和 `Upsert` 持独占写锁。返回的 `SchemaRef` 是 `Arc` 克隆，释放锁后仍然有效。`Upsert` 将旧向量移出锁内状态，并把释放动作交给 `FnOnce + Send + 'static` 闭包，调用方可推迟昂贵析构；若不调用闭包，闭包被丢弃时捕获值仍会被释放。

`Data::keep_alive` 通过 Acquire/AcqRel CAS 在并发调用中维护最小值；`GetAndResetRecentInfoSchemaTS` 用 AcqRel `swap` 原子返回旧轮水位并写入 `now`。这比 Go 当前的先 `Load` 后 `Store` 更接近单一原子轮次边界。

当前 `Data` 与 `infoschema_v2::Data` 是两个不同类型。`pkg/infoschema/test/infoschemav2test/v2_test.rs::test_get_and_reset_recent_info_schema_ts` 明确记录：Rust `InfoCache::Data` 尚未自动接入 V2 的 `TableByName`/`TableByID` 读取路径，测试只能手动调用 `keep_alive`。因此不能声称生产读路径已完整提供 Go 的 GC safepoint 保活语义。

## 与 Go 版本的对应关系

主要对照文件为 `pkg/infoschema/cache.go`，独立行为测试分别是 `pkg/infoschema/test/cachetest/cache_test.go` 与其 Rust 对照 `cache_test.rs`。Rust 保留了降序窗口、版本回退、timestamp 0、版本缺口、空版本容量、扩缩容和过旧拒绝等核心分支；Rust 测试用 `Arc::ptr_eq` 验证返回的是同一个 schema 对象实例。

已确认的差异：

- Go `NewCache(store, capacity)` 保存 `kv.Storage`，达到 `gcCheckInterval`/一分钟门槛后异步调用 `gcOldVersion`，查询最老 schema version 并执行 `Data.GCOldVersion`；Rust 构造函数没有 store，门槛分支只更新时间/版本，注释也声明存储侧 GC 不在本 crate 中。
- Go 查询路径维护 infoschema cache hit/get 指标并写调试日志；Rust 本文件没有对应指标或日志调用，虽然 Cargo 中存在 metrics 子 crate。
- Go `Upsert` 返回的闭包会主动清空旧 V1/V2 对象，使错误持有陈旧引用的调用者暴露问题；Rust 闭包只 `drop(old)`。其他 `Arc` 持有者仍可安全使用旧对象，不会被主动破坏。
- Go `GetAndResetRecentInfoSchemaTS` 是 `Load` 后 `Store`；Rust 使用单次 `swap`。Rust `Len` 复用带读锁的 `Size`，而 Go `Len` 未加锁。
- 同版本 V1/V2 代际不同的分支，Rust 原位替换对象和 timestamp 后立即返回；Go 先替换对象后继续进入后续插入逻辑。扩展或修复时必须先确认期望的 Go 行为与现有测试，而不能只做逐行机械翻译。
- Rust 的轻量 `Data` 尚未与 `infoschema_v2::Data` 自动连接；对应 V2 Rust 测试明确将此列为移植限制。

## 扩展指南

- 修改排序、淘汰、版本回退或 timestamp 选择规则时，优先扩展独立文件 `pkg/infoschema/test/cachetest/cache_test.rs`，并同步核对 `cache_test.go` 的同名场景；测试源文件不要内嵌到生产文件。
- 新增调用入口时，应保持 `SchemaRef = Arc<dyn InfoSchema>` 的共享生命周期，不在持锁期间执行存储 I/O、复杂构建或用户回调。需要析构大批旧对象时沿用 `Upsert` 的锁外释放模式。
- 接入真实旧版本 GC 时，最可能修改 `NewCache`、`CacheState`/`InfoCache` 与 `Insert` 的节流分支，并需要明确存储依赖属于哪个 crate；不得仅根据现有注释宣称 GC 已完成。还应补充独立并发/失败测试，覆盖存储查询失败与后台任务生命周期。
- 将 `Data` 接入 V2 读路径时，应同时检查 `pkg/infoschema/infoschema_v2.rs` 的真实 `Data`/keep-alive 机制以及 `pkg/infoschema/test/infoschemav2test/v2_test.rs`，避免维护两个互不相通的 min-TS 水位。
- 调整 `Reset`/`ReSize`/`Upsert` 时，先决定是否也应清理 `empty_schema_versions`、`first_known_schema_version`、GC 窗口；这些当前保留状态属于兼容行为，不能顺带改变。
- 性能风险集中在线性 snapshot 扫描、写锁内向量搬移和空版本集合超限时的全量排序；默认缓存很小（Go 注释给出现行值 16），若扩大容量，应重新评估复杂度与锁竞争。
- 兼容风险集中在 `GetByVersion` 的历史窗口判定、timestamp 0 对旧区间的阻断、V1/V2 同版本更新规则，以及 Rust/Go `Upsert` 和 GC 行为差异。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/infoschema/cache.rs` 确认目标文件已索引；`node --file pkg/infoschema/cache.rs --offset 1 --limit 420` 读取完整 305 行和 29 个符号；`query InfoCache/NewCache/GetByVersion/GetBySnapshotTS/InsertEmptySchemaVersion` 核对 Rust/Go 对照符号。通用 callers/explore 因跨仓库重名产生噪声，未把其模糊结果作为唯一证据。
- 生产源码：`pkg/infoschema/cache.rs`（全部类型、常量与实现）、`pkg/infoschema/lib.rs`（模块和重新导出）、`pkg/infoschema/Cargo.toml`（crate 边界与依赖）。该包不存在 `pkg/infoschema/doc.go`，因此无可读取的最近包级 Go contract 文件。
- 上游调用：`pkg/domain/domain.rs`（构造、发布、latest 查询、snapshot 查询、keyspace cache）、`pkg/infoschema/issyncer/loader.rs`（共享包装、按版本复用与发布）。
- Go 对照：`pkg/infoschema/cache.go`（缓存、指标、store GC、Upsert 行为）。
- 独立测试：`pkg/infoschema/test/cachetest/cache_test.rs` 与 `cache_test.go`（插入、查询、淘汰、timestamp 0、缺口、扩缩容、空版本）；`pkg/infoschema/test/infoschemav2test/v2_test.rs::test_get_and_reset_recent_info_schema_ts` 与对应 Go 测试（min-TS/reset 及当前接线差异）；`v2_test.rs::test_issue_54926`（V1/V2 历史版本与 snapshot 查询）。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 与 11 章节计数命令进行结构验证，并人工复核本文只描述已由上述源码、调用点和测试支持的事实。

# `pkg/session/runtime/ttl_metadata.rs`

## 文件定位

本文件是 `astersql-session` crate 的 TTL 元数据适配层，由 `pkg/session/runtime.rs` 以 `pub mod ttl_metadata` 装配。它把 `InfoSchema`/`TableInfo` 中的标准表模型转换为 `astersql-ttl-ttlworker` 可调度的 `PhysicalTable` 和 `TtlSchedule`，并在新 TTL 任务落盘前把逻辑表键空间分成扫描范围。上游主链为 `pkg/session/runtime/ttl_timer.rs` 中的定时器同步/事件校验，以及 `pkg/session/runtime/ttl_runtime.rs` 中的手动触发、周期调度和 `run_ttl_tick_inner`。

`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`，本文件直接使用的 workspace crate 包括 `astersql-domain`、`astersql-domain-infosync`、`astersql-infoschema`、`astersql-meta-model`、`astersql-parser-{ast,duration,mysql}`、`astersql-sessionctx-vardef`、`astersql-ttl-cache` 和 `astersql-ttl-ttlworker`；没有为此文件设置条件编译分支。

## 核心职责

1. `collect_ttl_schedules` 从一个 `InfoSchema` 快照枚举已启用 TTL 的表，解析作业周期，并将每个分区展开为独立调度单元。
2. `physical_ttl_tables` 将单个 `TableInfo` 转换为一个或多个 worker `PhysicalTable`，包括物理 ID、键列、TTL 列、定义版本与过期偏移。
3. `split_ttl_scan_ranges` 把标准表元数据映射为 `astersql-ttl-cache` 的扫描模型，优先在兼容性允许时使用 TTL 索引切分，否则使用主键切分或安全退化到全范围。
4. `ttl_index_scan_version_check` 在滚动升级期间守护新的索引扫描任务格式：只有所有实例的 version 和 git hash 一致时才允许创建。

## 主要符号

- `pub struct TtlScanRanges { ranges, index }`：扫描切分结果。`ranges` 是持久化到 TTL task 的范围；`index` 为 `Some(ScanIndex)` 时记录索引 ID、名称、列和唯一性，下游扫描据此选择索引路径。
- `struct StorageRegions(Arc<StorageHandle>)` 与 `impl RegionProvider`：将 domain 存储接口 `TTLRegionRanges(start, end)` 适配为 cache splitter 需要的 `Vec<KeyRange>`；存储返回 `None` 时视为空列表。
- `ttl_index_scan_version_check() -> JobVersionCheckResult`：读取本地和集群 server info，用进程级 `OnceLock<Mutex<JobVersionChecker>>` 缓存判定。
- `split_ttl_scan_ranges(domain, table, expire_time)`：扫描计划的公开入口，返回范围和可选索引描述。
- `pub struct TtlSchedule { table, job_interval_seconds, job_interval_expression }`：保留调度的数值秒数与原始 duration 表达式，供 timer 同步使用。
- `collect_ttl_schedules(info_schema, now_seconds)`：全局调度发现入口。
- `collect_physical_ttl_tables(info_schema, now_seconds)`：在同一调度结果上去掉 interval 信息的便利入口，主要供运行时校验和测试使用。
- `physical_ttl_tables(schema, table, now_seconds)`：单表转换核心，也是相关独立测试的直接对象。

## 执行流程

**调度发现。** `collect_ttl_schedules` 调用 `InfoSchema::AllSchemas`，再对每个 schema 调用 `SchemaTableInfos`。缺失 `model_meta` 的表被忽略；没有 TTL 配置或 `Enable == false` 的表也被忽略。空 `JobInterval` 使用 `DefaultTTLJobInterval`，否则保留配置值，两者均经 `ParseDuration(...).as_secs()` 解析。然后调用 `physical_ttl_tables`，为每个物理表/分区生成 `TtlSchedule`。

**单表转换。** `physical_ttl_tables` 首先要求 TTL 已启用且表状态是 `StatePublic`；再将允许的 `TimeUnitType` 显式映射为 cache `TimeUnit`，把 `now_seconds` 转成 `i64` 后调用 `EvalExpireTime`。结果以 `now - expire` 得到 `expire_after_seconds`，用饱和减法保证不为负。键列选择顺序是：`PKIsHandle` 的主键列、`IsCommonHandle` 的 public primary index 全部列、或隐式 `_tidb_rowid`。已启用分区的表按 `Partition.Definitions` 展开，否则使用逻辑表 ID。

**范围切分。** `split_ttl_scan_ranges` 先从 `Domain::stats_table` 重读当前标准模型，然后判断旧主键 splitter 是否支持：必须只有一个键列，且类型为整数、BIT，或可按字节安全解码的 binary/ASCII/Latin1/特定 binary collation 字符串。它将列和索引属性完整映射到 cache 模型，校验 TTL 时间列存在，根据 `TTLStoreCount` 计算 split count，再按如下优先级选路：

1. `TTLEnableIndexScan` 打开且 `FindTTLIndex` 找到合格索引时，执行集群版本检查。
2. `AllowIndexScan` 调用 `SplitIndexScanRanges`，返回索引范围和 `ScanIndex`；`BlockJob` 拒绝新任务；`FallbackToPrimaryKey` 继续旧路径。
3. 若旧主键 splitter 不支持当前键，返回一个 `newFullRange()`；否则调用 `SplitScanRanges`。

`pkg/session/runtime/ttl_runtime.rs::run_ttl_tick_inner` 仅在新任务分支调用范围切分，然后将 `ranges` 和可选 index ID 交给 `PersistentJobStore::start_job_with_ranges`；接管已有任务时使用已持久化范围，不重新切分。

## 数据与状态

- 调度输入是调用者传入的 `InfoSchema` 引用和 `now_seconds`。本文件不保存 schema 快照；单次函数调用内使用同一引用，避免该转换过程自行切换版本。
- `PhysicalTable.table_id` 是父表 ID，`physical_id` 是扫描与任务状态的物理 ID；非分区表两者相同，分区表则每个 definition 使用自己的 ID 和名称。
- `definition_version` 来自 `TableInfo.UpdateTS`，`expire_after_seconds` 是已计算的非负偏移，worker 后续可用 `PhysicalTable::expire_time(now)` 得到水位。
- 扫描模型保留 column ID/名称/public/nullable/hidden/key kind，以及 index 的 unique/primary/public/invisible/global/multi-valued/columnar/conditional 和列偏移/前缀长度，供 `FindTTLIndex` 判定索引是否可安全分页。
- 唯一的进程级可变状态是 `CHECKER: OnceLock<Mutex<JobVersionChecker>>`。检查器对允许/回退结果缓存 10 秒，对已知版本不一致缓存 60 秒（实际规则定义于 `pkg/ttl/ttlworker/job_version_checker.rs::JobVersionChecker::check`）。

## 依赖与调用关系

上游直接调用关系（经文本引用核验）：

- `pkg/session/runtime/ttl_timer.rs::SqlTtlTimerHook::OnPreSchedEvent` 和定时任务线程重新收集 schedule，确认 timer 指向的物理表仍存在且可调度。
- `pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command`、TTL manager 的 timer 同步闭包与 `run_ttl_tick_inner` 调用 `collect_ttl_schedules`。
- `pkg/session/runtime/ttl_runtime.rs::run_ttl_tick_inner` 调用 `split_ttl_scan_ranges`，并把结果交给持久化任务存储。
- `collect_physical_ttl_tables` 还被 `ttl_runtime_test.rs` 及 `normal_ddl_create_table_test.rs` 用于从实际 domain InfoSchema 取物理表。

下游主要依赖：`InfoSchema::{AllSchemas, SchemaTableInfos}` 提供快照表模型；`EvalExpireTime` 处理 calendar interval；`CachePhysicalTable::{FindTTLIndex, SplitIndexScanRanges, SplitScanRanges}` 实现索引选择与范围算法；`Domain::storage_handle` 提供 store count 和 Region 边界；infosync 与 `JobVersionChecker` 提供滚动升级安全门。

RustCodeGraph 将本文件索引为 453 行，并解析出 `split_ttl_scan_ranges`、`collect_ttl_schedules`、`collect_physical_ttl_tables` 和 `physical_ttl_tables` 等符号；但该索引对这些符号的 `callers`/`callees` 都返回空边，因此上述具体调用者使用精确符号检索补证，不将图缺边解释为“无调用者”。

## 错误处理与边界

- 发现层的 `SchemaTableInfos` 错误、`JobInterval` duration 解析错误、表转换错误都立即以 `String` 传播；不会跳过单个坏表后继续。
- 无 TTL/已禁用 TTL/非 public 表是正常的空结果，不是错误。未支持的 interval unit、超出 `i64` 的时间、缺失 PK handle 列、缺失 public primary index 则是错误。
- 切分前若 domain 中表、键列或 TTL 时间列已消失，返回带语义的错误，防止用过期 metadata 创建任务。
- `TTLStoreCount` 失败是可降级错误：记 Warn 日志并用默认 split count。Region 定位、索引切分或主键切分错误则直接传播。与 Go `job_manager.go` 相比，这里索引切分错误不在本层自动回退主键扫描。
- 版本信息查询失败/缺失或列表无可验证实例时安全回退旧主键格式；确认 version/git hash 不一致时返回 `BlockJob` 错误，不会静默创建可能被旧 worker 误解的索引任务。
- `table.key_columns[0]` 假定 worker `PhysicalTable` 至少有一个键列；本文件自己生成的值总是 PK/common handle 列或 `_tidb_rowid`。若外部手工构造空 `key_columns` 并直接调用 splitter，会越界 panic，这是调用前置不变式。

## 并发与资源生命周期

该文件不创建线程、异步任务、通道或事务；调度器线程和 TTL worker 生命周期由 `ttl_timer.rs`/`ttl_runtime.rs` 拥有。`Arc<Domain>` 和 `Arc<StorageHandle>` 仅保证调用期间的共享所有权，`StorageRegions` 不持有独立的 store 资源。

版本检查器本身在 `astersql-ttl-ttlworker` 中标明为非线程安全，所以本文件用全局 `OnceLock` 只初始化一次，再用 `Mutex` 串行化 `check`。若某线程持锁时 panic，后续调用用 `poisoned.into_inner()` 继续使用缓存状态，而不因 poison 再次 panic。读表、读 store count 和读 Region 均是同步调用，这个函数在任务创建路径上完成后才返回。

## 与 Go 版本的对应关系

- Go `pkg/ttl/cache/infoschema.go::InfoSchemaCache.Update` 同样只纳入 TTL 已启用且 `StatePublic` 的表，并按分区 ID 展开物理表。Rust 没有在本文件内保存长期 cache，而是每次从传入的 `InfoSchema` 快照生成值对象。
- Go `pkg/ttl/cache/table.go::{NewPhysicalTable, EvalExpireTime, FindTTLIndex, SplitScanRanges, SplitIndexScanRanges}` 是 Rust 转换和分割逻辑的主要语义对照。具体算法已下沉到 `astersql-ttl-cache`，本文件负责把 canonical model 转成该 crate 的输入。
- Go `pkg/ttl/ttlworker/job_manager.go::lockNewJob` 在新任务事务中计算 expire time、选索引/主键范围并插入 task；Rust 把“元数据+切分”与 `PersistentJobStore::start_job_with_ranges` 分层，但新任务的实际行为链对应。
- Go `pkg/ttl/ttlworker/job_version_checker.go` 定义相同的三态结果和 10/60 秒缓存。Rust 这一层把 infosync `ServerInfo` 转成 worker 模型；Rust API 只返回已注册的具体 TiDB server，因此明确设置 `assumed: false`。
- 差异点：Go 的 expire time 结果保留 `time.Time` 及 session/global timezone 语义；此 Rust 路径传入 epoch 秒并保存非负 `expire_after_seconds`。Go 在索引范围切分失败时记日志并回退 PK；此 Rust 函数直接返回错误。这些都是当前代码事实，不应在扩展时无意更改。

## 扩展指南

- 新增 TTL interval unit 时，在 `physical_ttl_tables` 的显式 match 中增加映射，并在独立 `pkg/session/runtime/ttl_metadata_test.rs` 添加过期偏移回归；同时核对 Go `EvalExpireTime` 的 calendar/timezone 语义。
- 新增列类型或索引属性时，必须同步检查 `CacheColumn`/`CacheIndexInfo` 映射和 `astersql-ttl-cache::PhysicalTable::FindTTLIndex`；否则可能错选不能稳定分页的索引。
- 改变分区展开或 ID 语义时，同步更新 `ttl_metadata_test.rs::go_merge_43_ttl_metadata_schedules_each_partition_by_physical_id` 以及消费 `table_id`/`physical_id` 的 timer key、历史表和 task 持久化路径。
- 改变范围切分或索引回退策略时，应扩展独立 Rust 测试（优先 `ttl_metadata_test.rs`，端到端持久化行为在 `ttl_runtime_test.rs`），并对照 Go `pkg/ttl/cache/{split_test,table_test}.go` 和 `pkg/ttl/ttlworker/job_manager_test.go`。不要把测试内嵌到生产 `.rs` 文件。
- 改变版本门禁时，保持“未知则回退、已知混合版本则阻断”的升级安全语义，并同步 `pkg/ttl/ttlworker/job_version_checker_test.rs` 与 Go 测试。
- 若要改动 `split_ttl_scan_ranges` 的入参不变式，应首先决定是否在入口显式拒绝空 `key_columns`，而不是依赖索引越界；这会改变错误边界，需要新增回归测试。

## 验证依据

- 目标源码：`pkg/session/runtime/ttl_metadata.rs`，RustCodeGraph `node --file ... --offset 1 --limit 520` 返回完整 453 行及符号定义。
- crate/模块边界：`pkg/session/Cargo.toml` 和 `pkg/session/runtime.rs`。
- Rust 上游与端到端证据：`pkg/session/runtime/ttl_timer.rs`、`pkg/session/runtime/ttl_runtime.rs`、`pkg/session/runtime/ttl_metadata_test.rs`、`pkg/session/runtime/ttl_runtime_test.rs` 和 `pkg/session/runtime/normal_ddl_create_table_test.rs`。`ttl_metadata_test.rs` 直接验证 canonical table 转换、1 天过期偏移、24 小时作业周期、禁用 TTL 过滤以及每分区 physical ID 展开；`ttl_runtime_test.rs` 验证 schedule 进入 timer/worker 及持久化范围被扫描/接管。
- Go 对照：`pkg/ttl/cache/infoschema.go`、`pkg/ttl/cache/table.go`、`pkg/ttl/ttlworker/job_manager.go`、`pkg/ttl/ttlworker/job_version_checker.go`，以及相关 `split_test.go`、`table_test.go`、`job_manager_test.go`。
- 版本门禁的 Rust 实现与测试：`pkg/ttl/ttlworker/job_version_checker.rs` 和 `pkg/ttl/ttlworker/job_version_checker_test.rs`。
- RustCodeGraph 状态：索引包含 11,467 个文件（其中 7,032 个 Rust 文件）；`query` 能找到本文件四个核心函数，但针对所查函数的调用边为空，因此调用关系另用 `rg` 精确核对。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前通过任务指定的 11 节结构命令，并人工核对本文档可回答文件定位、运行方式与安全扩展点。

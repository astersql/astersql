# `pkg/domain/domain.rs`

## 文件定位

`domain.rs` 是 `astersql-domain` crate 的实例级运行时核心，而不是模块装配文件。`pkg/domain/lib.rs` 将这里的 `Domain`、`DomainConfig`、`DomainError`、`DomainStatsContext` 与 `SysProcesses` 重新导出；上层会话工厂据此创建每个 keyspace 的长期共享 `Arc<Domain>`。典型入口见 `pkg/session/runtime/session_factory.rs`：构造 `Domain::new_with_storage_handle`，安装 schema/DDL 协作者，再调用 `Domain::init`；`pkg/session/runtime/session.rs` 和 `pkg/testkit/mockstore.rs` 也直接创建并初始化它。

该文件处在存储与 SQL 会话之间：下接 `StorageHandle`、`InfoSchemaLoader`、统计 KV/AutoID、DDL 元数据服务，上接 session、planner、executor、server 和测试工具。`pkg/domain/Cargo.toml` 将其归入 `astersql-domain`，并声明 `astersql-infoschema`、`astersql-ddl`、`astersql-statistics-*`、`astersql-meta-*`、`astersql-kv`、cross-keyspace/server-info、resource-group 和 inference 等直接依赖。

## 核心职责

1. 管理 Domain 生命周期：`new*` 只装配状态，`init` 从持久化元数据重建 schema/统计目录，`start` 启动 DDL 与周期 worker，`close` 幂等停止并释放协作者，`Drop` 作为兜底关闭存储。
2. 维护当前及历史 InfoSchema：`info_schema` 读取最新缓存，`reload` 只发布不旧于当前版本的快照，`snapshot_info_schema` 按时间戳回退到 loader。
3. 作为 Rust DDL 的局部聚合层：公开数据库、表、列、索引、分区、TTL、TiFlash、副本/放置和物化视图日志等入口，并在 `publish_ddl_metadata_change` 后协调 InfoSchema、统计目录、AutoID 与外部工作负载状态。
4. 管理统计运行时：统计缓存与 KV 持久化、异步直方图加载、列使用记录、增量预刷/提交、自动 Analyze、历史统计、锁表统计、DDL 后统计目录迁移和 AutoID 对齐。
5. 承载实例级辅助状态：慢查询、全局配置和权限事件、系统变量刷新、实例计划缓存、资源组/RU/runaway、系统进程、server-id 与连接 ID、跨 keyspace 运行时，以及 inference/TTL/MLog worker。

## 主要符号

- `DomainConfig`：schema/stats 租约、InfoCache/统计容量、慢查询容量、server-id TTL 和 keyspace。默认 keyspace 为 `SYSTEM`；测试构造器把 schema/stats lease 设为零。
- `DomainError`：区分关闭、重复启动、未初始化、存储/AutoID/DDL/统计错误、keyspace/运行时冲突、server-id 和系统进程错误。`from_auto_id` 保留 RPC 重试耗尽的类型身份，其余 AutoID 错误折叠为存储错误。
- `DdlService` 与 `StartMode`：把真正的 DDL 生命周期、owner 身份、持久化任务提交和存储类转换状态抽象在 Domain 之外；默认方法明确表示可选能力未启用。
- `Domain`：核心共享对象。原子字段表达单值状态，`Mutex`/`RwLock` 保护复合状态，`Arc` 共享存储、缓存、统计句柄和外部管理器。
- `DomainStatsBackend`、`DomainStatsContext`、`DomainHistoricalStatsStore`：分别适配 statistics handle 后端、提供只读/运维视图，并把历史统计 worker 接到 Domain 的统计句柄。
- `CrossKeyspaceCoordinator`、`CrossKeyspaceRuntimeHandle`、`KeyspaceRuntimeHandle`：记录 keyspace 可见性、DDL 同步摘要和租约；句柄 `Drop` 负责释放 holder/延迟回收空闲运行时。
- `PendingAddIndexJob`：以 `mDDL:pending-add-index:v1:` 键空间编码可恢复的分区 ADD INDEX 进度；解码会拒绝错误 magic、截断数据、尾随字节和缺失索引。
- `PlanCache`：容量至少为 1 的进程内近似 LRU；`get` 会提升命中项，`snapshot` 为 INFORMATION_SCHEMA 读者提供稳定副本。
- `PrivilegeEvent`：支持 `all` 和 `users:<list>` 编解码；合并时全量刷新优先，用户列表以 `BTreeSet` 去重并稳定排序。
- `SysProcesses`/`SystemProcess`：按连接 ID 跟踪、列出和 kill 系统进程；重复注册与缺失 kill 分别返回明确错误。
- `is_analyze_table_sql`、`calculate_node_resource`、`random_duration`：文件级辅助入口，分别处理带注释的 ANALYZE TABLE 前缀、资源下界/上界和有界抖动时长。

## 执行流程

主生命周期如下：

1. `Domain::new` 包装存储为 `StorageHandle` 后转入 `new_with_storage_handle`。后者创建 InfoCache、statistics `Handle`/`KvStatsStore`、KV AutoID store、历史统计 worker，并初始化所有原子量、队列、锁和目录快照；此时尚不能读 `info_schema`。
2. 调用方安装可选的 DDL、schema coordinator、server-info、cross-KS、外部工作负载和资源组协作者。`pkg/session/runtime/session_factory.rs` 是生产接线证据，`DomainSchemaLoader` 则允许持久化 DDL 完成后回调 `Domain::reload`。
3. `init` 首先拒绝已关闭实例，然后以 `reconcile_from_committed_metadata_with_mode(false)` 从已提交元数据重建目录，成功后发布 `initialized=true`。因此失败不会留下“已初始化”假象。
4. `start` 要求已初始化且未关闭，并用原子交换拒绝重复启动。它先启动已安装的 `DdlService`；失败时回滚 `started`。若 DDL 不自行管理 schema 同步，则启动 `schema-reload`；另启动 server-info 同步（若安装）和 `tiflash-replica-progress`，最后初始化 inference provider。
5. DDL 入口通过 `DdlMetadataService` 变更规范存储，再由 `publish_ddl_metadata_change`/reconcile 路径刷新公开 schema 及统计目录。ADD INDEX 另有 `stage_pending_add_index`、`advance_pending_add_index`、`finish_pending_add_index` 的持久化 owner-handoff 流程。
6. `reload` 从当前 keyspace 加载完整 schema，仅在版本 `>=` 当前缓存时对齐 AutoID allocator 并插入缓存；无论新旧均增加诊断计数。快照读取先查 `InfoCache`，未命中再调用 `load_snapshot_info_schema`。
7. `close` 以 `closed.swap` 保证幂等：依次停止并 join worker、取出并停止 DDL、关闭 cross-KS/server-info/external-workload、解除资源组控制器、释放 server-id、关闭 inference，清除 started，执行一次性回调并唤醒 `wait_closed`。`Drop` 再调用 `close` 并关闭底层 `StorageHandle`。

## 数据与状态

`initialized`、`started`、`closed` 是生命周期不变量；合法顺序是构造 → 初始化 → 启动 → 关闭。`close_pair` 的条件变量使等待方看到完整关闭结果。schema 由 `info_cache` 保存，额外 keyspace 运行时按名称缓存在 `keyspace_runtimes`，每个 holder 名称只能获取一次。

统计状态分成三层：`stats_handle` 是内存工作集，`stats_store` 负责 KV 读写，`stats_catalog`/history/timeline 描述表及版本历史。`pending_stats_deltas`、`pending_stats_flush` 和 `stats_flush_lock` 把预刷与提交串行化；`dropped_stats_ids`、`persisted_histograms`、`pending_column_usage` 分别跟踪 GC、持久化直方图和列使用。`stats_auto_id_allocators` 与两组 next-id 映射保持不同 allocator kind 的连续性。

事件类状态使用队列或标志：慢查询同时保留按时长排序 Top-N 与定长 recent；权限事件 drain 时归并；全局配置事件一次性 drain；sysvar 请求用 `swap(false)` 消费。server-id 以 `ServerIdLease { id, expires_at }` 表示，连接 ID 使用高 16 位 server-id 与低 48 位本地递增值组合。

## 依赖与调用关系

上游直接证据包括：

- `pkg/session/runtime/session_factory.rs` 创建 keyspace Domain、调用 `init`，并在未转移所有权时由 `PreparedTarget::drop` 调用 `close`。
- `pkg/session/runtime/session.rs` 创建普通/测试 Domain，调用 `init`、`start(StartMode::Normal)` 与 `reload`。
- `pkg/session/runtime/normal_ddl_service.rs::DomainSchemaLoader::reload` 以 `Weak<Domain>` 回调 `Domain::reload`，避免 DDL 与 Domain 形成强引用环。
- `pkg/session/runtime/planning.rs` 消费 `stats_handle`、`stats_context` 和 `info_schema`；`pkg/session/runtime/dml.rs` 调用 DDL/统计目录入口；`pkg/session/runtime/scan_adapter_runtime.rs` 写入慢查询。

下游依赖集中在 `canonical_domain::{StorageHandle, InfoSchemaLoader, DdlMetadataService}`、`astersql_infoschema::InfoCache`、`astersql_statistics_handle`、`astersql_meta_autoid` 和 `astersql_kv`。可选边界通过 trait/弱引用注入：`DdlService`、`AutoAnalyzeExecutor`、schema coordinator、资源组状态提供者和多个 manager，使 Domain 不直接拥有 SQL 执行器实现。

RustCodeGraph 索引显示 `pkg/domain/domain.rs` 被 113 个文件使用；文件级精确检索确认生命周期主调用者位于 session factory/session/testkit，DDL reload 回边位于 normal DDL service。泛化方法名在全仓库高度重载，调用边结论因此只采用文件限定结果，不采用无文件约束的 `new`/`close` 候选。

## 错误处理与边界

存储与 loader 错误统一附着到 `DomainError::Store(String)`；DDL 和 worker 边界分别使用 `Ddl`/`Worker`，统计错误使用 `Stats`。生命周期前置条件返回 `NotInitialized`、`AlreadyStarted` 或 `Closed`。与此相对，锁 poisoning 多数通过 `expect` 暴露为进程内不变量破坏，而不是伪装成业务错误。

`info_schema` 在未初始化时会 panic，调用者必须先成功 `init`。`initialize_stats` 是特殊的尽力而为路径：捕获 panic 并转成 `DomainError::Stats`，且无论成功、错误还是 panic 都设置 `init_stats_done`，防止启动等待永久阻塞。周期 worker 内的部分外部错误被有意忽略/重试，例如 schema reload 和 TiFlash progress；这类循环不能作为一次调用成功的保证。

DDL/统计复合变更包含补偿与故障注入点，扩展时必须维持“持久化元数据、公开 InfoSchema、统计目录”之间的提交顺序。`reload` 不允许旧版本覆盖新缓存；pending ADD INDEX 的二进制格式是持久化兼容边界，不可随意改变 magic、字段顺序或大小编码。

## 并发与资源生命周期

共享只读所有权采用 `Arc`，可替换协作者采用 `RwLock<Option<Arc<_>>>`，需要批量修改的集合采用 `Mutex`；计数器和布尔状态使用明确的 Acquire/Release 或 AcqRel 顺序。统计 flush、pending ADD INDEX owner、跨 keyspace coordinator 各自有独立锁，避免把无关慢路径串在一个全局互斥量上。

`start_periodic_worker` 为每个任务创建命名线程，循环检查停止原子量并用 `park_timeout` 等待；`WorkerHandle::stop` 设置标志、unpark 并 join，保证 `close` 返回前线程退出。DDL 引用在 stop 后被取出，源码注释明确这是为了拆除“DDL session pool → session → Domain”的 `Arc` 环。

`CrossKeyspaceRuntimeHandle::drop` 递减租约；最后一个租约释放后启动延迟回收线程，并用 generation 防止旧定时器删除已重新获取的运行时。`KeyspaceRuntimeHandle::drop` 同步移除 holder。Domain 自身 `Drop` 会关闭资源，但正常所有者仍应显式调用 `close`，以便等待 worker 和触发回调。

## 与 Go 版本的对应关系

同路径 `pkg/domain/domain.go` 的 `Domain`、`NewDomain*`、`Init`、`Start`、`Reload`、`Close`、`InfoSchema`、统计循环和 `SysProcesses` 是主要语义来源。两版都把 Domain 作为“每 store/实例一个”的全局容器，区分 Init（DML 可用）与 Start（DDL/后台服务可用），维护 InfoSchema、DDL、统计、slow query、server-id/连接以及关闭顺序。

Rust 版不是 Go 文件的逐字段机械镜像：Go 使用 context/cancel、channel、WaitGroup、etcd session 和大量后台 loop；Rust 当前以 trait 注入、原子状态、命名线程、内存事件队列及规范 KV 服务表达已移植路径。Go `Reload` 委托 `isSyncer.Reload`，Rust `reload` 直接经 `InfoSchemaLoader` 加载并做版本保护与 AutoID reconcile。Go 的系统 session pool 由 Domain 直接拥有，Rust 则由 session 层注册 `AutoAnalyzeExecutor` 等边界，因此不能把 Go 的所有 loop 宣称为 Rust 已完整实现。

测试对应关系也不同：`pkg/domain/domain_test.go` 覆盖 Go 的 info、panic 恢复、TTL/external workload、sysvar loop 和 ANALYZE 识别；Rust 的 `pkg/domain/domain_test.rs` 目前只直接覆盖 `is_analyze_table_sql`。更广的 Rust 生命周期、持久化 DDL、跨 keyspace、统计初始化和补偿行为在独立的 `pkg/domain/canonical_domain_test.rs` 以及 session/executor 测试文件中验证。

## 扩展指南

- 新增生命周期资源时，应在 `Domain` 字段、`new_with_storage_handle` 初始化、`start`（若需启动）和 `close`（逆序停止）四处成对接线，并在独立测试文件验证重复 start/close、失败回滚和 Drop 路径。
- 新增 DDL 行为优先扩展 `DdlMetadataChange`/`DdlMetadataService` 和对应 `ddl_*` 门面，随后同步统计目录、AutoID、外部工作负载及 InfoSchema 发布；不要绕过 `publish_ddl_metadata_change` 直接改缓存。
- 新增统计行为应明确属于内存 handle、KV store、catalog history 还是 pending flush，并复用 `stats_flush_lock`/批次补偿语义；规划侧读取契约需同步检查 `pkg/session/runtime/planning.rs`。
- 修改 keyspace、server-id 或连接 ID 时必须维持租约唯一性、generation 防 ABA、低 48 位掩码和 `0` 表示无 server-id 的约定。
- 新测试必须放在独立 `*_test.rs` 文件。文件内基础单元行为可扩展 `pkg/domain/domain_test.rs`；持久化 schema/DDL/生命周期应扩展 `pkg/domain/canonical_domain_test.rs`；跨 session 的实际接线应放在 `pkg/session/**_test.rs`。不要把测试模块内嵌回 `domain.rs`。
- 兼容风险主要是 Go 行为差异和持久化格式；性能风险主要是扩大锁临界区、在周期线程执行阻塞 I/O，以及无界增长目录历史/事件队列。改动前应重新核对同路径 Go 符号和真实上游调用者。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/domain` 确认目标及 Go/Rust 测试；`node --file pkg/domain/domain.rs` 分段核对源文件；`query domain.rs::Domain` 定位 `Domain` 及统计相关结构。索引报告该文件被 113 个文件使用。
- 直接读取：`pkg/domain/domain.rs`、`pkg/domain/Cargo.toml`、`pkg/domain/lib.rs`、`pkg/domain/domain_test.rs`、`pkg/domain/canonical_domain_test.rs`、`pkg/domain/domain.go`、`pkg/domain/domain_test.go`、`pkg/domain/db_test.go`。
- 上游接线核验：`pkg/session/runtime/session_factory.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/dml.rs`、`pkg/testkit/mockstore.rs`。
- 关键测试事实：`canonical_analyze_sql_detection_matches_tidb_statement_prefixes` 验证注释/空白/大小写边界；`domain_uses_canonical_storage_and_infoschema_lifecycle` 验证规范存储与 schema 生命周期；`domain_initializes_statistics_after_schema_bootstrap_in_read_only_transaction` 验证统计初始化；pending ADD INDEX、cross-keyspace、external workload 和 inference 生命周期分别由 `pkg/domain/canonical_domain_test.rs` 中具名独立测试覆盖。
- 本任务为纯文档分析，未运行 Cargo；最终仅执行任务规定的 11 章节结构检查，并人工复核没有把 Go 独有能力写成 Rust 已支持事实。

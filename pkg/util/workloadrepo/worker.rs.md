# `pkg/util/workloadrepo/worker.rs`

## 文件定位

`worker.rs` 是 `astersql-util-workloadrepo` crate 的状态与生命周期中心。crate 由同目录 `Cargo.toml` 定义，当前直接外部依赖只有 `chrono`；`lib.rs` 将本文件的后端抽象、数据类型、全局启停入口和查询封装再导出，根 `Cargo.toml` 再以 `facade_util_workloadrepo` 引入该 crate，`pkg/lib.rs::util::workloadrepo` 对外形成兼容门面。

本文件本身不实现建表、采样、快照分配和分区维护的全部细节，而是定义这些模块共享的 `worker`、`repositoryTable`、`RepositoryBackend` 与运行状态，并在 `worker::startRepository` 中接线到 `table.rs` 的 `createAllTables`/`checkTablesExists`，在全局 `takeSnapshot` 中接线到 `snapshot.rs` 的 `worker::takeSnapshot`。当前 Rust 服务入口 `cmd/tidb-server/main.rs` 调用的是 `cmd/tidb-server/stubs.rs` 中的 `repository::SetupRepository`/`StopRepository`，代码搜索没有发现它直接调用本 crate 的同名函数；所以本文件应视为可独立测试和被门面导出的移植实现，而不是已经接通 Rust 服务器生命周期的完整后台服务。

## 核心职责

- 用 `repositoryTable` 描述源表、历史目标表、表类别、过滤条件及缓存 SQL；用 `defaultWorkloadTables` 给出与 Go 默认清单一致的 1 张元数据表、5 张快照表和 5 张采样表。
- 用 `RepositoryBackend` 隔离 SQL、列元数据、表/分区检查、实例标识、owner 判断和 etcd 风格 KV 操作，使 `table.rs`、`sampling.rs`、`snapshot.rs`、`housekeeper.rs` 不依赖具体 TiDB 会话类型。
- 用 `worker` 聚合后端、表清单和 `WorkerState`，负责初始化、启动前置检查、停止及配置目标切换。
- 用进程级 `WORKER` 槽位实现 `SetupRepository`、`StopRepository` 和手工 `takeSnapshot` 的共享入口。
- 用 `runQuery` 与 `execRetry` 统一调用后端；后者最多尝试 5 次并保留每次失败文本。

这里的“启动”是同步准备：填目标表名、owner 建表、确认所有表及未来分区就绪、读取实例 ID，最后置 `started=true`。它不创建 Go 版本中的长期采样、快照或 housekeeper goroutine。

## 主要符号

- `snapshotTable`、`samplingTable`、`metadataTable`：值分别为 0、1、2 的表类别标记。其他模块据此选择建表列、插入参数和任务集合。
- `repositoryTable`：crate 内部的表描述。`schema`/`table` 指向采集源，`destTable` 指向 `WORKLOAD_SCHEMA` 历史表，`whereClause` 是可选过滤，`createStmt` 允许元数据表携带固定 DDL，`insertStmt` 由采样或快照路径惰性构建并缓存。
- `ColumnDefinition`、`Value`、`Row`：后端边界上的列描述和最小查询数据模型。`Value` 覆盖空值、无符号/有符号整数、字符串和本地时区时间。
- `RepositoryBackend: Send + Sync`：共享后端契约。`execute`/`source_columns`/`table_exists`/`partitions` 支撑 SQL 与表管理，`instance_id` 支撑节点标识，`is_owner` 控制 DDL/维护职责，`etcd_available` 与 `kv_create`/`kv_get`/`kv_cas` 支撑全局快照号协调。
- `WorkerState`：受 `Mutex` 保护的启用、完成启动、实例 ID、采样间隔、快照间隔和保留天数。
- `worker`（公开别名 `WorkloadRepoWorker`）：以 `Arc<dyn RepositoryBackend>` 共享后端，以两个互相独立的 `Mutex` 管理表清单和状态。
- `defaultWorkloadTables()`：构造默认清单，并内置 `HIST_SNAPSHOTS` 的完整建表列契约。
- `initializeWorker(backend, workloadTables)`：返回新的 `Arc<worker>`；状态初始为未启用、未启动、实例 ID 为空，三个配置取 `const.rs` 默认值。
- `takeSnapshot()`、`SetupRepository()`、`StopRepository()`：围绕全局 `WORKER` 的公开入口。`init()` 当前是无操作占位。
- `runQuery()`、`execRetry()`：直接执行与五次重试封装。
- `worker::{readInstanceID, fillInTableNames, startRepository, start, stop, setRepositoryDest}`：实例生命周期主干；`enabled`、`started`、`instanceID`、`intervals` 是只读快照访问器。

## 执行流程

1. 调用方先通过 `initializeWorker` 注入一个满足 `RepositoryBackend` 的实现和表清单；若使用 `defaultWorkloadTables`，元数据、快照、采样三类表会同时进入清单。
2. `SetupRepository` 读取旧全局 Worker 的 `enabled` 状态，再用新 `Arc<worker>` 替换全局槽位。旧 Worker 已启用时，它立即调用新 Worker 的 `start`；否则只完成注册。
3. `setRepositoryDest` 先交给 `utils.rs::validateDest` 规范化。结果为 `"table"` 时调用 `start`，其余合法空目标调用 `stop`。
4. `start` 先把 `enabled` 设为 `true`。若已经 `started`，直接成功；否则必须有可用 etcd，再释放状态锁并调用 `startRepository(Local::now())`。
5. `startRepository` 先由 `fillInTableNames` 为没有显式目标名的源表补上 `HIST_<源表>`。若后端报告当前实例为 owner，则调用 `table.rs::worker::createAllTables` 创建缺失表和分区；随后无论是否 owner，都以 `checkTablesExists` 要求所有目标表存在且最后分区晚于次日。
6. 表就绪后，`readInstanceID` 仅在缓存为空时调用后端，成功后将 `started` 置为 `true`。任一步返回错误都会终止本次启动。
7. 手工全局 `takeSnapshot` 会克隆全局 Worker，拒绝未注册或未启用状态，然后进入 `snapshot.rs::worker::takeSnapshot`：后者通过 SQL 与 KV/CAS 分配全局 `SNAP_ID`。本入口返回分配出的 ID；它不在这里调用 `startSnapshot` 去采集所有快照表。
8. `stop` 把 `enabled` 和 `started` 都清为 `false`；`StopRepository` 还会把全局槽位置空。再次 `start` 会重新执行建表就绪检查和实例 ID读取（已有实例 ID 缓存不会清除）。

## 数据与状态

`workloadTables: Mutex<Vec<repositoryTable>>` 是可变表目录。启动时会补 `destTable`；`sampling.rs::samplingTable` 与 `snapshot.rs::snapshotTable` 会在持锁区间内惰性生成并缓存 `insertStmt`，之后复制 SQL 再释放锁执行。表类别是不变量：元数据表使用预置 `createStmt`，快照表附带 `SNAP_ID`，采样表只携带时间与实例信息。

`state: Mutex<WorkerState>` 将配置和生命周期状态集中串行化。`enabled` 表示目标配置已启用，`started` 表示同步基础设施准备完成，两者不可互换：`start` 在检查 etcd 和表就绪之前就先置 `enabled=true`，因此启动返回错误时可能出现 `enabled=true, started=false`。`instanceID` 是惰性缓存；`samplingInterval`、`snapshotInterval`、`retentionDays` 在本文件初始化，在相邻模块中更新或使用。

全局 `WORKER: OnceLock<Mutex<Option<Arc<worker>>>>` 只初始化一次锁，但锁中的 `Option` 可被替换或取走。`Arc` 允许调用方和全局槽位共享 Worker；`StopRepository` 只停止并移除全局引用，其他持有的 `Arc` 仍然有效。

## 依赖与调用关系

上游方面，`lib.rs` 明确再导出本文件的公开入口和类型；`sampling_test.rs`、`snapshot_test.rs`、`utils_test.rs` 直接用 `initializeWorker` 构造夹具，`worker_test.rs` 覆盖全局启停和核心生命周期。仓库搜索还显示根门面 `pkg/lib.rs::util::workloadrepo` 可暴露该 crate，但当前 Rust `cmd/tidb-server` 仍通过本地 stub 记录 Setup/Stop 事件，这一限制不能从 Go 主链类推消除。

下游方面：

- `startRepository -> fillInTableNames -> table.rs::{createAllTables, checkTablesExists}`；建表内部又使用 `RepositoryBackend`、分区生成函数及 `execRetry`。
- 全局 `takeSnapshot -> snapshot.rs::worker::takeSnapshot`；该路径使用后端 KV、`queryMaxSnapID` 与 `upsertHistSnapshot` 分配 ID。
- `setRepositoryDest -> utils.rs::validateDest -> start/stop`。
- `execRetry -> runQuery -> RepositoryBackend::execute`，没有休眠或退避。
- `worker` 还被 `sampling.rs`、`snapshot.rs`、`housekeeper.rs` 扩展同类型 `impl`，共同读取表目录、状态和后端。

RustCodeGraph `files --filter pkg/util/workloadrepo` 确认 worker 及上述相邻实现、Rust 测试、Go 对照均已索引；`node --file .../worker.rs` 给出了完整 325 行源码和跨文件使用信息。精确 `callers`/`callees` 命令本次没有输出，因此具体调用边又以符号搜索和相邻实现源码核验，未把空图结果当作“不存在调用”。

## 错误处理与边界

- 所有后端业务失败以 `Result<_, String>` 传播。`execRetry` 固定连续尝试 5 次，成功即返回；全部失败时用换行连接五条错误，没有延迟、分类或指数退避。
- 全局 `takeSnapshot` 对“没有全局 Worker”和“Worker 未启用”统一返回 `errWorkloadNotStarted`；真正的快照分配错误原样上抛，不像 Go 全局包装成 `errCouldNotStartSnapshot`。
- `start` 在检查失败前已设置 `enabled=true`，且不会回滚；调用方若需要重试或显式停用必须处理这个状态。它只检查 `etcd_available`，实际 KV 错误延后到快照路径暴露。
- 非 owner 不建表，但仍检查全部表和分区是否已就绪；未就绪立即返回 `"repository tables are not ready"`，没有 Go 后台循环的一秒重试。
- `readInstanceID` 只有成功后才填缓存；失败会保留空值，后续启动可再尝试。`stop` 不清空该缓存或已生成的 `insertStmt`。
- 所有标准库锁都使用 `lock().unwrap()`，线程持锁 panic 导致锁中毒时后续访问会 panic，而不是转换成 `String` 错误。`SetupRepository` 替换全局 Worker 后若新 Worker 启动失败，旧 Worker 已丢失且新 Worker 保留在槽位。
- `Value`/`Row` 是刻意收窄的查询模型；增加后端返回类型时，必须同步后端实现和消费方，不能假设它等价于 Go 的任意 `chunk.Row`。

## 并发与资源生命周期

`RepositoryBackend` 要求 `Send + Sync`，`worker` 又通过 `Arc` 共享，因此后端实现必须自行保证并发安全。`workloadTables` 与 `state` 分锁减少无关操作互斥；当前本文件没有同时持有两把实例锁的路径。全局槽位的锁仅用于取得或替换 `Arc`，全局 `takeSnapshot` 会在调用耗时快照逻辑前释放它。

本文件不生成线程、定时器或取消令牌。真正的逐表并发位于 `sampling.rs::startSample` 和 `snapshot.rs::startSnapshot` 的 scoped threads，housekeeper 也只返回一次性闭包；`start` 不会自动调度这些函数。因此 `started=true` 仅代表表和实例 ID 已准备好，不代表后台周期任务存活。

`SetupRepository` 会保留旧 Worker 是否启用这一配置意图，但直接替换全局 `Arc`，不会先对旧实例调用 `stop`；若外部仍持有旧 `Arc`，其状态和资源独立存在。`StopRepository` 对取出的实例调用 `stop`，当前 `stop` 只更新状态，没有需要 join 的线程或需要关闭的后端资源。

## 与 Go 版本的对应关系

Rust 默认表类别、表清单、`HIST_SNAPSHOTS` 列、五次查询重试、目标表名补全和主要命名均直接对照 `pkg/util/workloadrepo/worker.go`。`worker_test.rs::default_snapshot_metadata_ddl_matches_go_columns` 固定元数据列契约，`exec_retry_joins_all_five_errors_like_go` 固定五次错误合并语义。

重要差异如下：

- Go `init` 注册 `executor.TakeSnapshot` 和四个全局系统变量 hook；Rust `init()` 无操作，配置接线尚未发生。
- Go `SetupRepository(*domain.Domain)` 从 Domain 取得 etcd、owner factory 和系统会话池，并初始化单例；Rust 要求调用方先构造 `RepositoryBackend` 和表清单，再传入 `Arc<worker>`。
- Go `startRepository` 竞选 owner，并在循环中等待表就绪、启动 sample/snapshot/housekeeper 三个 goroutine；Rust 同步检查一次，不竞选 owner、不重试、不启动周期任务。
- Go `getSessionWithRetry` 从 session pool 无限重试并每秒等待；Rust 只克隆后端 `Arc`。
- Go `stop` 取消上下文、等待 goroutine、恢复 statement summary history 并关闭 owner；Rust 只清两个布尔状态。
- Go `takeSnapshot(ctx)` 对全局单例加锁并把底层失败映射为管理命令错误；Rust 释放全局锁后调用实例，并返回 `SNAP_ID` 或原始字符串错误。

因此扩展时应以 Rust 当前契约为事实；若目标是补齐 Go 行为，需要显式设计调度、取消、owner、会话池和系统变量集成，而不能仅在本文件中假设这些资源已经存在。

## 扩展指南

- 新增采集表：优先修改 `defaultWorkloadTables`，选择正确的表类别；如需过滤，填 `whereClause`。同步扩展独立的 `worker_test.rs` 默认清单/DDL断言，并根据类别在 `sampling_test.rs` 或 `snapshot_test.rs` 验证生成 SQL 和参数。
- 扩展后端能力或值类型：修改 `RepositoryBackend` 或 `Value` 时，需要同步所有实现，至少包括 `worker_test.rs::MemoryBackend` 及相邻测试中的轻量后端；同时评估 `Send + Sync`、锁粒度和错误文本兼容性。
- 改启停语义：集中修改 `start`、`startRepository`、`stop`、`setRepositoryDest` 和 `SetupRepository`，并覆盖启动失败后的 `enabled/started` 组合、替换已启用全局 Worker、重复启动、停止后重启及无 etcd 场景。
- 接入真正后台调度：不能把定时循环内嵌到本文件测试中；生产逻辑应与 `sampling.rs`、`snapshot.rs`、`housekeeper.rs` 协同，测试继续放在独立 `worker_test.rs` 或对应模块测试文件。需定义取消与 join 顺序、owner 迁移、panic/锁中毒策略，并对照 Go 的资源释放行为。
- 修改查询重试：`execRetry` 是建表和分区 DDL 的共同依赖。增加退避或错误筛选会影响启动延迟与可观测错误，必须补充成功前瞬时失败、五次耗尽和错误聚合顺序测试。
- 接入服务器主链：需要替换或绕过 `cmd/tidb-server/stubs.rs` 中的仓库占位，并提供真实 `RepositoryBackend`；这是当前文件之外的架构接线，不能仅以根门面存在作为完成证据。

## 验证依据

- 目标源码：`pkg/util/workloadrepo/worker.rs`，RustCodeGraph `node --file pkg/util/workloadrepo/worker.rs --offset 1 --limit 500` 完整读取 325 行；符号搜索确认常量、数据类型、trait、全局函数及 `impl worker`。
- crate/门面：`pkg/util/workloadrepo/Cargo.toml`、`pkg/util/workloadrepo/lib.rs`、根 `Cargo.toml` 的 `facade_util_workloadrepo` 依赖，以及 `pkg/lib.rs::util::workloadrepo` 再导出。
- 直接实现证据：`pkg/util/workloadrepo/table.rs` 的 `createAllTables`/`checkTablesExists`，`snapshot.rs` 的 `worker::takeSnapshot`，`sampling.rs` 的 scoped-thread 采样，`housekeeper.rs` 的 owner 分区维护，`const.rs` 的默认值和错误文案。
- Go 对照：完整阅读 `pkg/util/workloadrepo/worker.go`，核对默认表、Domain/session/owner 资源、系统变量注册、异步启动、停止和查询重试。
- 测试证据：`pkg/util/workloadrepo/worker_test.rs` 的 `TestRaceToCreateTablesWorker`、`TestGlobalWorker`、`TestAdminWorkloadRepo`、`TestStoppingAndRestartingWorker`、`TestSettingSQLVariables`、`default_snapshot_metadata_ddl_matches_go_columns`、`exec_retry_joins_all_five_errors_like_go`；另以 `sampling_test.rs`、`snapshot_test.rs`、`utils_test.rs` 的 `initializeWorker` 使用确认跨模块构造入口。Go 回归面参考 `pkg/util/workloadrepo/worker_test.go`。
- 应用接线：仓库搜索确认 `cmd/tidb-server/main.rs` 的 Setup/Stop 调用解析到 `cmd/tidb-server/stubs.rs`，未发现 Rust 主链直接调用本 crate 的同名入口。
- 本任务为纯文档分析，按计划不运行 Cargo；最终只运行任务指定的 11 章节结构检查，并人工复核上述限制没有被描述成已支持能力。

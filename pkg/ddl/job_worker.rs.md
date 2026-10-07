# `pkg/ddl/job_worker.rs`

## 文件定位

`job_worker.rs` 属于 `astersql-ddl` crate，由 [`pkg/ddl/lib.rs`](lib.rs) 以公开模块 `job_worker` 挂载。它位于 owner 侧 DDL 调度器与具体 DDL action 处理器之间：正常 Domain 在 [`pkg/session/runtime/normal_ddl_service.rs`](../session/runtime/normal_ddl_service.rs) 中创建 `General`、`AddIndex` 两个 `JobWorker`，[`JobScheduler::schedule_persisted`](job_scheduler.rs) 从 `mysql.tidb_ddl_job` 读取 wire `Job` 后选择 worker，并调用 `JobWorker::transit_persisted_job_step`；事务提交后再由 executor 的 `wait_synced` 执行 schema 同步屏障。

文件同时保留两层模型，阅读时不可混为一谈：

- 第 27–221 行的 `WorkerType`、`ReorgContext`、`JobContext`、轻量 `JobWorker::transit_one_job_step` 和若干 helper 使用 `crate::ddl::{Job, JobState}`，主要是内存状态机/兼容辅助模型；`schedule_persisted` 的注释明确说明生产 durable queue 不经过这条 metadata-free 路径。
- 第 223 行之后以 `astersql_meta_model::group_3::Job` 为 wire 模型，定义真实 SQL/KV session、owner lease、action executor 边界，以及持久化 job step、事务型索引回填和 reorg 初始化。这是当前正常 Domain 的生产执行骨架。

`pkg/ddl/Cargo.toml` 声明 crate 名为 `astersql-ddl`、`[lib] path = "lib.rs"`，并直接依赖 `astersql-meta`/`astersql-meta-model`、`astersql-kv`、`astersql-tablecodec`、`astersql-meta-metadef`、`astersql-config`/`astersql-config-kerneltype`、`astersql-metrics`、`astersql-ddl-placement` 和 `serde_json` 等本文件所需组件。源文件本身没有条件编译项。

## 核心职责

1. **表达 worker 类别与最小内存状态机。** `WorkerType::{General, AddIndex}` 为调度器隔离普通 DDL 和 reorg 工作；`transit_one_job_step` 演示 `None → Running → Done → Synced`、`Cancelling → Cancelled`，并在 Running 步通过 `SchemaVersionManager` 产生 `SchemaDiff`。
2. **定义持久化执行端口。** `DurableJobSession` 抽象同一 pooled SQL session 及其真实 KV transaction；`JobExecutionContext` 暴露具体 action 所需的 reorg、物化视图、placement、TTL、标签、modify-column 等资源；`JobLease` 提供 owner/取消/任期 fencing；`DurableJobExecutor` 将升级准入、恢复、action step 和 schema wait 留给正常 DDL executor。
3. **原子推进 durable job。** `transit_persisted_job_step` 在一个 DDL transaction 内重读并比对 `job_meta`、执行 action、计算 RU、重新编码/写回 job，并在每个关键阶段检查 lease，防止 ADMIN 变更、重叠 owner 或取消后的旧 worker 提交。
4. **执行事务型 ADD INDEX 回填。** `run_transactional_index_backfill` 仅接受 txn backend，逐 batch 执行索引写入，校验进度，处理 retryable KV 错误，并在独立事务中发布 reorg checkpoint。
5. **在真实资源上下文中初始化 reorg。** `initialize_persisted_index_reorg` 借助 `IndexReorgInitialization` 和 `with_execution_context` 调用 `persistent_actions::initialize_reorg_indexes`，只接管 `ReorgMeta` 与 index 元数据，不冒充后续 build/ingest/DXF/rollback/publication 阶段。
6. **提供局部 Go 对齐 helper。** `account_job_ru`、`choose_lease_time`、`build_placement_affects` 和轻量 `job_need_gc` 分别覆盖 RU、lease 上限、placement ID 映射和有限 GC 判断。

## 主要符号

- `WorkerType`：公开枚举。`General` 和 `AddIndex` 由 `JobScheduler` 持有；durable 调度依据 job 表的 `reorg` 列选择实例。
- `ReorgContext` / `JobContext`：轻量上下文，记录行数、warning、完成标志、资源组，以及 owner、MDL、step/error 状态。它们不等同于 durable 路径的 `PersistentReorgContext`。
- `JobWorker { worker_type, closed, version_manager, history }`：worker 状态。`close` 是本地拒绝新 step 的开关；`history` 仅服务轻量模型。生产完成态的 durable job 由 `NormalDdlExecutor::finish` 写入 KV/SQL history，而不是写入这个 `Vec<Job>`。
- `JobWorker::transit_one_job_step`：轻量状态迁移 API。Running 时 `update` 后立即 `unlock`；Done/Cancelled 时 `finish_job` 解锁并追加内存 history。
- `job_need_gc`：轻量 `crate::ddl::Job` helper，目前只将未取消的 `DropTable`、`TruncateTable` 判为需要 GC。完整 wire job 的生产判断是 [`delete_range::persistent_job_need_gc`](delete_range.rs)，覆盖更多 action、warning 和 multi-schema 情况。
- `account_job_ru`：当 job 为 `Cancelled`/`RollbackDone` 时清零 RU；否则仅 NextGen 按 `transaction_size * txn_kv_byte_weight` 累加。
- `choose_lease_time`：lease 为零或超过 maximum 时返回 maximum，否则原值返回。
- `build_placement_affects`：按位置把 `old_ids` 映射为 `AffectedOption { old_table_id, table_id: new_ids[index] }`；新 ID 不足会断言失败，多余新 ID 被忽略。
- `DurableJobSession`：生产 session trait。必需方法为 `query/begin/commit/rollback/with_transaction`；资源相关默认方法显式返回“unavailable”，避免无实现时假成功。实际实现为 [`SystemSessionLease`](../session/runtime/system_session.rs)。
- `TransactionOperation` / `ExecutionOperation`：`Send + 'static` boxed closure，分别让调用方在真实 KV transaction 或 `JobExecutionContext` 中执行一段操作并返回序列化字节。
- `JobExecutionContext`：action resource trait。多数方法有失败型默认实现；`restore_reorg` 是例外，它通过 `query(..., "get_handle")` 调用 `reorg::restore_reorg`。正常实现为 `ConcreteJobExecutionContext`。
- `JobLease`：`owner_epoch/is_owner/is_cancelled`。正常服务的 `Lease` 将其连接到 owner manager 与 scheduler cancellation；epoch 防止同一进程失主再获主后旧任务恢复提交。
- `DurableJobExecutor` / `DurableJobStep`：executor 的 `runnable/recover/step/wait_synced` 合同，以及 step 返回的 `schema_version`、`update_raw_args`、`removed`。`removed=true` 表示终态已在本事务删除 queue row、写入 history，worker 不得重建该 row。
- `JobWorker::transit_persisted_job_step`：durable 核心事务边界。
- `JobWorker::run_transactional_index_backfill`：txn reorg backend；不是完整 ADD INDEX handler。
- `IndexReorgInitialization` / `initialize_persisted_index_reorg`：共享 reorg 初始化 adapter 与入口。

## 执行流程

生产主链如下：

1. `normal_ddl_service` 的 `normal-ddl-scheduler` 线程在 owner 有效时取得 `SystemSessionLease`，调用 `JobScheduler::schedule_persisted`。
2. scheduler 查询 `mysql.tidb_ddl_job`，解码 Go-compatible job bytes，检查升级策略和 involving-schema 冲突，再依据 `reorg` 标志选择 general/reorg worker。
3. `transit_persisted_job_step` 先检查 `closed`、owner、cancellation，调用 `bind_owner_epoch` 清理跨任期瞬态缓存；随后让 executor `recover` 前任 owner 未同步的 schema version，并再次检查 lease。
4. session 开启事务。worker 保存进入事务前的 `job.ru`，在同一事务内按 `job_id` 重读 `job_meta`，要求字节与 scheduler 观察到的 `expected_bytes` 完全相等；不相等说明 ADMIN pause/cancel 或其他 owner 已修改 job，立即放弃。
5. `executor.step` 执行具体 action。正常实现 `NormalDdlExecutor` 位于 [`table_mode.rs`](table_mode.rs)，再分派到 `persistent_actions` 及各 `persistent_*` 文件，并负责终态删除队列、写 history、注册 delete range 等。
6. action 返回后再次检查 lease。非失败态且为 NextGen 时读取当前 transaction size 并通过 `account_job_ru` 累计；失败终态清零 RU。
7. 若 `DurableJobStep.removed == false`，按 `update_raw_args` 编码 wire job 并更新 `mysql.tidb_ddl_job.job_meta`。提交前再检查一次 lease，然后 commit。任一步骤失败都会 rollback，并把内存中的 `job.ru` 恢复为事务前值。
8. 已同步终态且 RU 大于零时，commit 之后更新 `metrics::ru_v2` 并调用 session reporter；这保证失败提交不会发布 RU。
9. scheduler 随后调用 `executor.wait_synced(job, schema_version, lease)`。因此 schema 同步发生在事务成功后，失败或尚未结束的 job 仍保留 involving-schema 冲突依赖。

事务型索引回填子流程：

1. `run_transactional_index_backfill` 检查 worker、owner epoch、`ReorgMeta`，拒绝 `IsDistReorg` 或非 `ReorgTypeTxn`，并要求 job 正在 `WriteReorganization`、action 为 add index/primary key、element 为 `_idx_` 且 ID 属于本次 index 集合。
2. 它读取并解码 durable job，比较 state/type/raw args/schema/table/schema state，固定本轮期望值；batch size 必须大于零。
3. 每批在新事务中再次检查 epoch 和 job bytes，调用 `backfill_index_batch`。adapter 返回的 `next_key` 必须严格前进且不越过 `end_key`。
4. 同一事务读取并原值写回 job table 的 KV row，使并发 ADMIN pause/cancel 与该 batch 形成写冲突；commit 前再次 fencing。只有包含 `TxnRetryableMark` 且不超过全局 `MaxRetryCnt` 的错误才 backoff 重试整个 batch。
5. batch commit 后合并计数/warning。checkpoint 使用另一个事务调用 `PersistentReorgHandler::stage_update`；发布失败时索引条目已提交，但依赖幂等重放，而内存 `start_key` 仅在 checkpoint commit 成功后推进。

## 数据与状态

- **轻量 job 状态：** `None/Running/Paused/Cancelling/Cancelled/Done/Synced` 由 `transit_one_job_step` 直接迁移；只有 Running 步生成 `SchemaDiff`。这是局部模型，不是所有 Go DDL action 状态机的完整复刻。
- **durable job 状态：** 使用 `astersql_meta_model::group_3::Job`，`NormalDdlExecutor` 处理 Running、Pausing/Paused、Cancelling/Rollingback、Done/Synced、Cancelled/RollbackDone；具体 schema state 由 `persistent_*` action 推进。
- **持久化位置：** 活跃 job 的权威值是 `mysql.tidb_ddl_job.job_meta`；终态由 executor 写入 history 并删除 queue row。`expected_bytes` 是乐观并发校验基线。
- **schema version：** `DurableJobStep.schema_version` 返回本步产生的版本；worker 只负责事务提交，scheduler/executor 的 `wait_synced` 负责 follower/MDL 屏障。包契约 [`pkg/ddl/doc.go`](doc.go) 要求集群同时最多存在相邻 N、N+1 两个版本，推进前必须确认同步。
- **RU：** RU 随 wire job 持久化；本事务失败时恢复 `ru_before_transaction`。Cancelled/RollbackDone 清零；成功 Synced 后才计入全局指标和资源组 reporter。
- **reorg progress：** `PersistentReorgContext.info.start_key/end_key/element/physical_table_id` 是可恢复 checkpoint；runtime row count 与 warnings 是运行期聚合。entry commit 和 checkpoint commit 分离，因此 replay 必须幂等。
- **owner epoch：** epoch 必须非零且整个回填阶段保持相等；`bind_owner_epoch` 让 session 侧缓存与单次 leadership tenure 绑定。
- **placement 映射：** `AffectedOption` 仅从 old/new table ID 两个切片按下标构造，长度前置条件由断言表达。

## 依赖与调用关系

上游调用者与装配：

- [`pkg/session/runtime/normal_ddl_service.rs`](../session/runtime/normal_ddl_service.rs)：创建 `JobScheduler(JobWorker::new(General), JobWorker::new(AddIndex))`，实现 `JobLease`，并在 owner 线程中周期调用 durable scheduler。
- [`JobScheduler::schedule_persisted`](job_scheduler.rs)：`transit_persisted_job_step` 的生产直接调用者；它负责队列加载、action 冲突准入、worker 选择及提交后的 `wait_synced`。
- [`initialize_persisted_index_reorg`](job_worker.rs) 的直接内部下游仍是 `transit_persisted_job_step`；相关正常运行时测试位于 `pkg/session/runtime/normal_ddl_index_reorg_initialization_test.rs`。
- `run_transactional_index_backfill` 当前可见直接引用集中在 `pkg/session/runtime/normal_ddl_test.rs` 的真实 session/fault 场景；源内注释也明确它只是 production stage entrypoint，backend 选择和完整 ADD INDEX 编排仍属于 action 层，不能把它描述为完整 handler。

主要下游：

- `astersql_meta::{decode_go_history_job, encode_go_ddl_job}`：Go wire job 兼容编解码。
- `NormalDdlExecutor`（`table_mode.rs`）与 `persistent_actions`/各 `persistent_*`：具体 action、状态迁移、history/delete-range。
- `SchemaVersionManager`（`schema_version.rs`）：仅用于轻量状态机；durable 路径通过 executor barrier 恢复和等待 schema version。
- `SystemSessionLease` / `ConcreteJobExecutionContext`（`pkg/session/runtime/system_session.rs`）：把 traits 落到真实 SQL session、KV transaction、独立 GC session、reorg adapter 和外部资源。
- `reorg::PersistentReorgHandler::stage_update`、`backfilling::{IndexBackfillBatch, BackfillResult}`：批次执行与 checkpoint。
- `astersql_kv`、`astersql_tablecodec`、`astersql_meta_metadef`：retry/backoff 以及触碰 `mysql.tidb_ddl_job` 对应 KV row 的 key/value。
- `astersql_config_kerneltype`、`astersql_config`、`astersql_metrics`：NextGen RU 开关、权重和指标发布。

RustCodeGraph 的文件查询显示 `job_worker.rs` 已索引 100 个符号；精确 `query` 找到 `JobWorker`、五个核心 trait/struct 以及上述公开方法。图的文件级反向边还指向 `persistent_actions.rs`、`persistent_masking_actions.rs`、`persistent_mview_out_of_place_cutover.rs` 和 `system_session.rs`，与 `JobExecutionContext` 的使用/实现关系一致。由于通用符号名 `worker` 会命中大量无关模块，调用关系以精确符号查询加 `rg` 的直接引用复核。

## 错误处理与边界

- 所有 durable API 统一返回 `Result<_, String>`；这保留错误文本但不提供类型化分类。只有事务型回填通过字符串包含 `astersql_kv::TxnRetryableMark` 判断是否可重试，扩展时不得把业务错误误标为 retryable。
- `transit_persisted_job_step` 在 begin 失败时也调用 rollback；事务闭包、commit 或 lease 检查失败时统一 rollback，并恢复 RU。它不会吞掉 `executor.step` 的基础设施错误。
- executor 合同约定：需要写进 `Job.error/error_count` 的 action 错误应由 executor 处理后以成功 transaction result 返回；`Err` 专用于必须放弃事务的 storage/lease/staging 失败。违反该约定会错误丢弃本应持久化的 job 状态。
- `DurableJobSession`、`JobExecutionContext` 的资源默认实现多数直接失败，错误文本以 `unavailable` 结尾；这是能力缺失保护，不是可用的 no-op。新 action 若调用新资源，生产 adapter 必须同步实现。
- owner 或 scheduler cancellation 在开事务前、action 前后、commit 前均被检查；回填还检查 epoch。它降低失主后错误提交风险，但不能替代 job row compare/write-conflict fencing。
- job row 缺失返回 `DDL job disappeared`；字节不一致返回 `job meta changed by others`。回填会额外验证解码后的关键字段，避免用陈旧 action/schema/reorg 参数继续写数据。
- backfill `next_key` 必须满足 `old_start < next_key <= end_key`，batch size 不能为零，element/type/state/backend 必须匹配，否则立即拒绝。
- `build_placement_affects` 对新 ID 数量不足使用 `assert!`，会 panic；调用方必须先满足长度不变量。Go 版本会因越界同样 panic，Rust 测试显式锁定该行为。
- 轻量 `count_for_error` 使用固定阈值 3，并且只返回错误；Go `countForError` 会写入 job error/error count、加载动态全局限制并在可回滚 Running job 超限时转 Cancelling，因此两者不能等价替换。

## 并发与资源生命周期

- 正常 DDL owner 在独立 `normal-ddl-scheduler` 线程运行；owner 变化会关闭旧 scheduler、重建两个 worker、重载 schema，并以新 epoch 重建 executor。`JobWorker::closed` 防止旧实例继续推进。
- `JobLease` 同时观察 owner manager 和 cancellation token；`owner_epoch` 是比单纯 `is_owner` 更强的任期隔离。`InitializationLease` 捕获初始 epoch，并把 epoch 变化映射为失主。
- `SystemSessionLease` 把 SQL 与 KV 元数据写绑定到同一 owning thread/session；其 `Drop` 会 rollback 未完成事务并把连接归还池，无法归还时关闭并销毁。
- `TransactionOperation`/`ExecutionOperation` 为一次性 closure，借用在返回前释放。`with_execution_context` 要求活动事务存在；某些资源操作（如 delete-range 注册、物化视图 build/prewrite）按 Go 语义另取独立 session/commit，trait 注释明确事务边界。
- durable job step 对 queue row 采用“事务内重读字节 + 更新/删除同一 row + 多次 lease 检查”；这是 owner failover、ADMIN pause/cancel 和并发调度之间的主要 fencing。
- 回填 batch 每次独立提交，以限制事务大小；checkpoint 再独立提交。发生 checkpoint 失败时，已写索引由后续幂等 replay 覆盖，不能提前更新内存 `start_key`。
- RU 指标和 reporter 只在 durable commit 成功且 job 已 Synced 后发布，避免重试/rollback 重复计费；事务中编码的 RU 才是可恢复权威值。
- 轻量 `history: Vec<Job>`、`ReorgContext` 没有内部锁，不适合跨线程共享；生产并发由 scheduler 的拥有关系、session affinity 和外层同步管理。

## 与 Go 版本的对应关系

主要对照文件是 [`pkg/ddl/job_worker.go`](job_worker.go)，相关 Go 测试为 [`pkg/ddl/job_worker_test.go`](job_worker_test.go)。对应关系如下：

- Rust `transit_persisted_job_step` 对齐 Go `(*worker).transitOneJobStep` 的关键事务语义：begin、事务内校验 job bytes、运行一步、owner/cancel commit fencing、更新 job、commit、返回 schema version。Go 还包含 failpoint、tracing、MDL registration、TopSQL、动态重试等待和更细的 rollback/reset 分支；Rust 的这些策略主要下沉到 `NormalDdlExecutor`、schema barrier 或尚未在本文件表达，不能宣称逐行等价。
- Rust `DurableJobExecutor::step` 对应 Go `runOneJobStep` 与 action dispatch 的可插拔边界；`wait_synced` 对应 `transitOneJobStepAndWaitSync` 后续的 `updateGlobalVersionAndWaitSynced`/`waitVersionSynced`。Rust worker 本身不直接更新 etcd global version。
- `account_job_ru` 对齐 Go `accountJobRU` 的 NextGen transaction-byte 计费，并补上失败终态清零；Go 的 reorg/DXF 专项计费链在其他模块，本 helper 只负责当前 transaction sample。
- `choose_lease_time` 与 Go `chooseLeaseTime` 的零值/上限选择一致。
- `build_placement_affects` 与 Go `buildPlacementAffects` 均按 old ID 长度索引 new ID；Rust 显式断言使 Go 的越界前置条件更清楚。Rust 在 old IDs 为空时返回空 `Vec`，Go 返回 `nil`，调用语义等价但表示不同。
- 轻量 `job_need_gc` **不是** Go `JobNeedGC` 的完整移植：Go 覆盖 drop schema/index/column/partition、modify column、add index、materialized view、multi-schema 等，并处理特殊 warning；生产 Rust 对应逻辑在 `delete_range::persistent_job_need_gc`。本 helper 仅覆盖其轻量 `ActionType` 已建模的 drop/truncate table。
- Rust `run_transactional_index_backfill` 对齐 Go `RunInNewTxn` 式逐批提交、retryable storage retry 和 reorg checkpoint，但明确拒绝 ingest/DXF backend；这些 backend 由更高层 action 选择。
- Rust 独立测试 `pkg/ddl/job_worker_test.rs` 覆盖 helper、RU rollback、owner/cancel 前置拒绝；Go `TestJobNeedGC` 和本文件前部大量 RU 集成子测试提供更广的原语义证据。Rust 还在 `pkg/session/runtime/normal_ddl_test.rs` 与 `normal_ddl_index_reorg_initialization_test.rs` 覆盖真实 session 回填和初始化边界。

## 扩展指南

- 新增或修改 DDL action 时，优先在 `persistent_actions`/相应 `persistent_*.rs` 接入 `NormalDdlExecutor::step`，不要把 action 业务堆进通用 worker。若需要新外部资源，在 `JobExecutionContext` 添加能力后同步实现 `ConcreteJobExecutionContext`，默认实现应继续 fail closed。
- 改动 durable job transaction 时必须维持：事务内重读与 `expected_bytes` 比较、action 后和 commit 前 lease 检查、失败 rollback、RU 内存恢复，以及 `removed` 防止重建已删除 queue row。
- 新增 reorg backend 不应静默落入 `run_transactional_index_backfill`。先在 action 层明确选择 txn/ingest/DXF，再为该 backend 建立独立进度、取消、幂等和计费证据。
- 修改 backfill checkpoint 时需同时验证 entry transaction 与 checkpoint transaction 的崩溃窗口；adapter 必须保证重复执行同一 key range 不破坏唯一性或计数，且 `next_key` 严格单调。
- 扩展 owner 相关缓存时应绑定 `owner_epoch`，并在任期变化清理；只检查布尔 `is_owner` 不足以防同进程重新当选后的旧任务。
- 调整 RU 时同步检查 wire job 编码、commit 失败恢复、Cancelled/RollbackDone 清零、Synced 后单次指标发布和资源组归属；同步更新 `job_worker_test.rs` 及正常 session 的 durable scheduler/reorg 测试。
- 调整 GC 时修改生产 `delete_range::persistent_job_need_gc` 及其独立测试；如果轻量 `crate::ddl::ActionType` 也支持该 action，再同步 `job_need_gc`，不要把后者误作生产权威。
- 修改 schema version 行为时同步检查 `table_mode::NormalDdlExecutor`、`schema_version.rs` 和 scheduler 的 `wait_synced` 调用；必须保留 `pkg/ddl/doc.go` 所述相邻版本同步不变量。
- 测试仍应放在独立文件。最近单元测试是 `pkg/ddl/job_worker_test.rs`，生产 adapter/真实事务场景还应扩展 `pkg/session/runtime/normal_ddl_test.rs`、`normal_ddl_index_reorg_initialization_test.rs` 或 `durable_scheduler_test.rs`，不要把测试嵌入 `job_worker.rs`。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/ddl/job_worker.rs` 命中目标且报告 100 个符号。使用 `node --file ... --offset/--limit` 阅读了完整 922 行；对 `JobWorker`、`DurableJobSession`、`JobExecutionContext`、`JobLease`、`DurableJobExecutor`、`transit_persisted_job_step`、`run_transactional_index_backfill`、`initialize_persisted_index_reorg` 及 helper 执行了精确 `query`。`callers` 命令在本地索引上长时间无结果后终止，故调用边以 `query` 和直接引用搜索交叉验证，没有把该未完成查询当作证据。
- 目标源码：`pkg/ddl/job_worker.rs`，完整核对 922 行，包括所有 enum、struct、trait、type alias、函数和四个 `impl JobWorker`；无 `cfg` 条目。
- crate/模块：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、`pkg/ddl/doc.go`。
- 生产上游与 adapter：`pkg/ddl/job_scheduler.rs` 的 `schedule_persisted`，`pkg/session/runtime/normal_ddl_service.rs` 的 owner 线程与 `Lease`，`pkg/session/runtime/system_session.rs` 的 `SystemSessionLease`/`ConcreteJobExecutionContext`。
- 下游状态与同步：`pkg/ddl/table_mode.rs` 的 `NormalDdlExecutor`，`pkg/ddl/delete_range.rs` 的 `persistent_job_need_gc`，`pkg/ddl/reorg.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/schema_version.rs`。
- Go 对照：`pkg/ddl/job_worker.go` 的 `JobNeedGC`、`accountJobRU`、`transitOneJobStep`、`countForError`、`runOneJobStep`、`updateGlobalVersionAndWaitSynced`、`buildPlacementAffects`；`pkg/ddl/job_worker_test.go` 的 RU 与 `TestJobNeedGC` 场景。
- Rust 测试：完整阅读 `pkg/ddl/job_worker_test.rs`；并通过直接引用定位 `pkg/session/runtime/normal_ddl_test.rs`、`normal_ddl_index_reorg_initialization_test.rs`、`durable_scheduler_test.rs` 等真实 session 覆盖面。本任务按计划为纯文档分析，未运行 Cargo。
- 人工复核结论：本文区分了轻量与生产路径，描述了 owner fencing、job row 乐观校验、事务/回填 checkpoint、schema wait 边界、RU 与失败恢复，并明确列出未完整对齐 Go 的 helper 和安全扩展入口，没有把默认报错能力或阶段性入口写成“已完整支持”。

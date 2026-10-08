# `pkg/session/runtime/modify_column_dist_backfill.rs`

源文件：[`modify_column_dist_backfill.rs`](./modify_column_dist_backfill.rs)

## 文件定位

本文件属于 `astersql-session` crate 的私有 `runtime::modify_column_dist_backfill` 模块；装配点是 `pkg/session/runtime.rs` 的 `mod modify_column_dist_backfill;`，crate 边界与依赖由 `pkg/session/Cargo.toml` 定义。它是会话运行时与 DXF（distributed execution framework）之间的持久化适配层：DDL owner 侧把 `MODIFY COLUMN` 所需的索引重建或临时索引合并提交为全局 task，executor 节点把持久化 subtask 转回本仓库的扫描、pipeline、云排序/导入和 merge 原语。

应用主链的直接入口在 `pkg/session/runtime/system_session.rs`：`ingest_modified_indexes` 在 `Job.reorg_meta.IsDistReorg` 时以 `merging = false` 调用 `run`，`merge_modified_indexes` 以 `merging = true` 调用同一入口；非分布式路径不经过本文件。后台节点由 `NodeService::start` 创建，跨 keyspace runtime 则由 `session_factory.rs` 调用 `register_target_runtime` 注册。该文件不创建 DDL job，也不推进 column schema state；它只执行既有 DDL job 的分布式 backfill 阶段。

文件通过 `#[path]` 私有装入 `modify_column_cloud_executor.rs` 为 `cloud`、装入 `modify_column_cloud_planner.rs` 为 `cloud_planner`。除测试辅助项外，对父模块可见的 API 是 `run`、`register_target_runtime`、`TaskRuntimeBinding`、`NodeService`、`ReadIndex`、`ImportControl`；`ConcreteSession::ImportNodeTaskTable` 是 crate 公共 session 方法。文件没有 feature 条件分支，只有 `#[cfg(test)]` 的观测辅助函数。

## 核心职责

1. `TaskTable` 将 DXF executor 的 task/subtask 表接口转发到 SQL-backed `storage::TaskManager`，并负责 task/subtask 状态类型转换、checkpoint/summary 持久化和 runtime 获取。
2. `run` 以稳定 task key 查找或创建 durable backfill task，初始化 scheduler、node/slot manager 和 `Planner`，同步调度到终态，验证历史 subtask 与 checkpoint，最后用 ReadIndex 步骤的聚合行数覆盖 DDL job 进度。
3. `Planner` 按物理表、TiKV region 和 executor 数量生成连续子任务范围；本地读索引/临时索引 merge 直接生成范围，云模式继续推进 `ReadIndex -> MergeSort -> WriteAndIngest` 并委托 `cloud_planner` 生成后两步计划。
4. `Extension` 把 task meta 解码成 `ReadIndex` 或 `cloud::CloudStep`；`ReadIndex` 实现实际读索引 pipeline、云对象写出、临时索引 merge、动态 batch/限速/资源调整与 checkpoint 更新。
5. `NodeService` 注册 backfill task executor、初始化节点资源和 task 元数据并启动 manager；`TaskRuntimeBinding` 在 NextGen 中借用正确 keyspace 的 Store/Domain，同时用 runtime handle 防止执行期间被空闲回收。
6. `ImportControl` 与 `PipelineGuard` 用析构收束取消、后台桥接线程、pipeline producer 和当前运行 pipeline 指针，避免错误返回时遗留工作线程。

## 主要符号

- `error`、`task`、`state`、`subtask`：内部协议适配函数。`task`/`subtask` 显式映射持久化 proto 状态；未知 task state 降为 `Modifying`，未知 subtask state 降为 `Paused`。
- `TaskTable(storage::TaskManager, Weak<Domain>)`：实现 `executor::TaskTable`。除普通查询/状态更新外，`AcquireTaskRuntime` 创建 `TaskRuntimeBinding`，`UpdateSubtaskSummaryJSON` 先校验 JSON，checkpoint 仅接受 `serde_json::Value`。
- `TaskMeta`：durable task JSON，包含编码后的 DDL `job`、index IDs、云 URI、估算行宽、是否 merge 临时索引和格式 `version`。`SubtaskMeta` 同时接受新 `row_start`/`row_end` 与 legacy `start-key`/`end-key`，键均为标准 Base64。
- `Checkpoint { next_key, row_count }`：subtask 的可恢复进度。`next_key` 是 Base64 编码的排他式下一扫描位置，`row_count` 是累计扫描数。
- `Extension`：实现 `executor::Extension`，只接受四个 backfill step；ReadIndex、MergeSort、WriteAndIngest、MergeTempIndex。它把 scheduler task 转换为本地 step executor，并把 write-conflict/region/not-leader 文本识别为可重试错误。
- `ReadIndex`：持有 Domain、task manager、Job、当前 pipeline、step resource、平均行宽、index IDs、merge/cloud 模式和共享写限速器。`new` 从 reorg meta 初始化 CPU 与速率；`import_control` 建立 framework/KV/local/cloud 四路取消桥。
- `ReadIndex::run_pipeline`：普通 ReadIndex 的主执行路径；校验 region 覆盖连续性，启动 scan/encode/ingest pipeline，按乱序 ack 的起始键排序推进 frontier，持久化 checkpoint/summary，并在云模式写出 external subtask meta。
- `impl StepExecutor for ReadIndex`：`TaskMetaModified` 更新 batch size，并只在本地模式更新最大写速率；`ResourceModified` 调整正在运行的 pipeline CPU；`RunSubtask` 恢复 checkpoint，非 merge 进入 pipeline，merge 则逐批事务调用 `modify_column_backfill::merge`。
- `run(...)`：owner 侧同步入口。它负责 task 去重/恢复、slot 预留、scheduler/balancer 循环、终态检查和 DDL job 行数回写。
- `Planner`：实现 `scheduler::Extension`。`ranges` 规划 record range 或 temporary-index range；`next_step` 决定本地一步、云三步或 merge 一步；`modify_meta` 应用动态 batch size/max write speed。
- `register_target_runtime` / `TaskRuntimeBinding`：跨 keyspace registry 与租约绑定。registry 只存 `Weak`；binding 持有 `RuntimeHandle` 到 `Release`/析构，并在每次检查时核对 Domain、Store 与 task keyspace。
- `NodeService`：每 Domain 的后台 executor 服务。`start` 注册 task type、创建并启动 `executor::Manager`；NextGen 非 SYSTEM keyspace 只初始化 SYSTEM task access，不创建本地 manager。`stop`/`Drop` 停止 manager。
- `read_index_for_test`、`closed_pipeline_workers_for_test`：仅测试编译可见，用于观测真实运行中的 ReadIndex/pipeline；测试逻辑仍位于独立 `*_test.rs`。
- `ConcreteSession::ImportNodeTaskTable`：为普通 import node 暴露 SQL-backed `TaskTable` trait object。

## 执行流程

### owner 提交与调度

`system_session.rs` 先确保 `NodeService` 已启动，再调用 `run`。`run` 根据 job ID、多 schema sequence、merge 标志构造 task key；NextGen 额外加 keyspace 前缀。它从当前 job 取得 reorg 配置，云读索引但 URI 为空会立即失败；平均行宽来自 `information_schema.tables.AVG_ROW_LENGTH`，查询或解析失败回退为 `0`。

`GetTaskByKeyWithHistory` 命中时复用旧 task ID，以支持 owner 重入/恢复；仅错误文本包含 `not found` 时才创建新 task。创建时写入 `TaskMeta`，并把期望并发限制在目标 scope 可用 CPU 内，启用 `PauseOnKVDiskFull`。随后 node manager 刷新节点、slot manager 更新资源并预留 slot；没有配额直接返回错误。

`run` 从事务元数据读取表并在回滚只读事务后展开物理分区 ID，再创建 `BaseScheduler<Planner>`。循环每 100 ms 刷新节点、执行一次 schedule、检查 task 终态并运行 balancer；节点被停止时主动关闭 scheduler、释放 slot 并失败。成功后读取当前入口 step 的历史 subtasks，要求全部 `Succeed` 且 checkpoint 能反序列化，最后读取 `BackfillStepReadIndex` 的聚合行数写回 `job`，返回 `done = true` 且 `next_key = request.task.end_key`。

### 计划生成与步骤推进

`Planner::ranges` 对普通 ReadIndex 先用 snapshot 找到每个物理表实际首尾 record key，再调用 `retry_region_plan` 和 `try_generate_plan_for_physical_table` 按 region/node 数切分，并在云模式为计划分配 timestamp。对临时索引 merge，它为每个 physical table/index ID 构造 temporary-index key range，再用 `generate_temporary_index_plan` 切分。所有键写入 `SubtaskMeta` 时使用 Base64；普通路径写 `row_start/row_end`，merge 路径写 legacy-compatible `start-key/end-key`。

`Planner::next_step` 从 `STEP_INIT` 进入构造时指定的入口：本地 ReadIndex 或 MergeTempIndex 随后结束；带云 URI 的 ReadIndex 依次进入 MergeSort、WriteAndIngest 后结束。`on_next_subtasks_batch` 对后两步调用 `cloud_plans`，其他步骤调用 `ranges`。NextGen 的 node count 取 task `max_node_count.max(1)`，普通模式取当前 nodes 数。

### executor 与 subtask

`NodeService::start` 以 server IP/port 形成 executor ID，server info 不可用时使用进程内 mock ID。注册的 task factory要求同时存在当前节点 binding 和可 downcast 的 `TaskRuntimeBinding`；成功时注入真实 `Extension`，否则注入始终报错的 `UnavailableExtension`/`UnavailableRuntime`，避免意外使用错误 Domain。

`Extension::GetStepExecutor` 解码 `TaskMeta` 和 Job。MergeSort/WriteAndIngest 包装为 `cloud::CloudStep`；ReadIndex/MergeTempIndex 返回 `ReadIndex`，后者以 `merge` 标志切换行为。普通 ReadIndex 的 `RunSubtask` 从 durable checkpoint 恢复；云 ReadIndex 为避免复用旧本地 checkpoint，从 subtask 起点重新初始化。非 merge 路径进入 `run_pipeline`；merge 路径每批显式 `BEGIN`，调用 `modify_column_backfill::merge`，失败尝试 `ROLLBACK`，成功 `COMMIT` 后才更新 checkpoint 与 summary，并拒绝不前进的结果。

`run_pipeline` 把子任务范围再次按当前 region 边界分割并要求无间隙、无尾部遗漏。pipeline 的结果可以乱序到达，但 `BTreeMap` 只从当前 frontier 连续消费；重复 ack、倒退或越界的 `next_key` 都失败。每次连续推进后，以 SQL 更新 `mysql.tidb_background_subtask.checkpoint` 并更新 row summary。云模式完成 pipeline 后将每个 index 的排序摘要写到对象存储，再替换 `subtask.Meta`；本地模式由 pipeline 直接 ingest。

## 数据与状态

- 持久状态位于 DXF 系统表：global task、background subtask、subtask `checkpoint`、`summary` 和 history。`TaskTable` 通过 `storage::TaskManager` 统一读写；`run` 的 task key 是重入幂等键。
- `TaskMeta.job` 是 `Job::encode(false)` 结果经 JSON value 包装后的 Go 兼容表示。动态修改先解码 Job，修改其 `reorg_meta`，再重编码回 task meta。
- `SubtaskMeta` 的 `physical_table_id` 决定扫描/merge 的物理表；`ts` 供分布式读计划携带版本。当前 `ReadIndex::RunSubtask` 不直接读取 `ts`，它由相关 pipeline/cloud 计划消费或保留在线协议中。
- checkpoint 只在已连续确认的 frontier 或已提交的 merge batch 后更新，因此不会越过未完成乱序批次。云 ReadIndex 的 external meta 发布发生在 pipeline 完成之后。
- `ReadIndex.resource` 与 `pipeline` 分别由 `Mutex` 保护；Job 用 `Arc` 共享，而 `ReorgMeta` 自身的 setter 提供运行时动态参数更新。`write_limiter` 在已创建的 import control 之间共享，所以限速更新对在途本地写入生效。
- `nodes()` 与 `target_domains()` 是 `OnceLock<Mutex<HashMap<...>>>` 进程级 registry，值以 `Weak` 保存，不应延长 NodeService、Store 或 Domain 生命周期。注册目标时会清理失效 weak entry。
- `TaskRuntimeBinding.released` 是幂等 release 标志；持有的 cross-keyspace `RuntimeHandle` 是防止目标 runtime 被回收的实际租约。

## 依赖与调用关系

上游直接调用边经源码引用确认：

- `runtime.rs -> mod modify_column_dist_backfill`。
- `system_session.rs::ingest_modified_indexes -> NodeService::start -> run(..., false, ...)`。
- `system_session.rs::merge_modified_indexes -> NodeService::start -> run(..., true, ...)`。
- `system_session.rs::start_dxf_worker` 负责服务启动，context/session 关闭路径调用 `NodeService::stop`。
- `session_factory.rs` 在构造跨 keyspace runtime 后调用 `register_target_runtime`。
- 测试直接调用 `ReadIndex::new/import_control`、`TaskRuntimeBinding::acquire`、`NodeService::start` 和测试观测辅助函数。

核心下游是 `astersql-dxf-framework-{storage,scheduler,taskexecutor,proto}`；`pkg/session/Cargo.toml` 显式声明这些 path dependency。数据面依赖 `astersql-ddl` 的 backfill/规划与 task key、`astersql-meta(-model)` 的表/Job、`astersql-kv` 的 snapshot/SST option、`modify_column_pipeline`、`modify_column_backfill`、cloud planner/executor/meta/store，以及 ingest control、resource manager workerpool、tablecodec、store coprocessor codec。标准库提供线程、原子量、互斥量、弱引用与进程级惰性 registry；`serde_json`/`base64` 负责 durable wire format。

RustCodeGraph `query` 精确定位了 `ReadIndex`、`NodeService`、`TaskRuntimeBinding` 和 `register_target_runtime`，`node --file ... --offset 958` 读取了 `run` 并报告目标文件被多个 DXF/session/DDL 文件使用。该索引当前对精确 `callers/callees` 查询未返回边，因此以上具体调用关系以直接源码引用补证，没有把空图结果解释为“无调用者”。

## 错误处理与边界

- 本文件将多数底层错误压平为 `executor::ExecutorError(String)` 或 `String`；调用方不能依赖结构化 cause。`Extension::IsRetryableError` 又以错误文本匹配 write conflict、region、not leader，新增错误包装时要防止改变重试分类。
- durable task 查找用错误文本 `not found` 判断创建分支；其他错误直接返回。相同 task key 会复用历史 task，不会重复创建。
- `Planner::ranges` 拒绝版本号 `0`，并通过 `retry_region_plan` 处理 region 变化；`run_pipeline` 进一步拒绝不连续、缺尾的 region 覆盖。空表不生成普通读范围。
- subtask 必须提供可解码的 end key 和 start/checkpoint；损坏 JSON/Base64、缺失 reorg meta、空云 URI、无 index group、内存除数溢出、无 slot、表缺失或非成功终态都会失败。
- ack 必须唯一且使 frontier 严格前进，不能超过 subtask end。merge batch 同样要求 `result.next_key > start`。这些检查避免无限循环或把 checkpoint 推过未处理数据。
- merge 事务失败时尝试回滚，但回滚错误被忽略并保留原始执行错误；提交失败时 checkpoint 不更新。pipeline checkpoint 的 SQL 更新和实际 KV ingest 不属于同一事务，崩溃一致性仍依赖底层 ingest 幂等性与 task 重试协议。
- `TaskMetaModified` 在 merge executor 上是 no-op，与 Go 当前行为一致；云模式只更新 batch size，不更新本地 write limiter。`ResourceModified` 在没有活跃 pipeline 时返回 `no subtask running`，由 framework 重试。
- `TaskRuntimeBinding` 对 released/closed Domain、实际 Store keyspace 不匹配、缺失目标 Domain 或 cross-keyspace manager 都显式失败。`UnavailableExtension` 是安全失败边界，不会降级到当前 keyspace 执行。
- `TaskTable::UpdateSubtaskCheckpoint` 只接受 JSON value，`UpdateSubtaskSummaryJSON` 会先解析 JSON；非法持久化格式不会写表。

## 并发与资源生命周期

`NodeService` 由 `Arc` 持有，进程 registry 只保存 `Weak`。相同 node ID 且旧服务未停止时 `start` 复用已有服务；新服务先 `InitMeta` 再 `Start`。`stop` 使用原子标志并停止 manager，`Drop` 再调用一次是幂等收束。NextGen 用户 keyspace 分支只取得 SYSTEM runtime/task manager 并返回 `manager: None`，与 Go `InitDistTaskLoop` 的服务边界一致。

`run` 是同步 scheduler 循环。slot 在启动前 reserve，在正常终态或显式节点停止时 unreserve；但初始化后其他 `?` 错误路径没有统一 guard，源码本身没有证明所有异常都会释放 slot，这是扩展时应重点审查的资源风险。balancer 与 scheduler 共享 `Arc` manager，循环以 100 ms sleep 避免忙等。

每个 `ReadIndex::import_control` 启动一个 5 ms 轮询桥接线程：framework context 完成或 KV context 取消时，同时取消 local token、KV context 和 cloud 标志。`ImportControl::drop` 先置 stop，再触发所有取消并 join 线程，保证 import 控制销毁前桥接线程已退出。`NativeWriteLimiter` 在等待前检查 KV cancellation，并保留 ingest control 的 typed cancellation error。

`PipelineGuard::drop` 先 shutdown pipeline、join producer，再仅在指针仍指向本次 pipeline 时清空 `ReadIndex.pipeline`，避免旧 guard 擦除后来替换的实例。pipeline ack 可乱序并发产生，主线程用 `BTreeMap` 串行提交连续进度。云 summaries 用 `Arc<Mutex<Vec<SortedMeta>>>` 聚合。

跨 keyspace `TaskRuntimeBinding` 持有 `RuntimeHandle`；`Release` 原子标记并 `take` handle，重复调用安全。registry 的 weak Domain 只有在 acquire 时升级成强引用，executor 完成/释放后不会由 registry 阻止 Domain 回收。

## 与 Go 版本的对应关系

主要 Go 对照是 `pkg/ddl/backfilling_dist_executor.go`、`backfilling_dist_scheduler.go`、`backfilling_read_index.go`、`backfilling_merge_temp.go` 和 `index.go`：

- Rust `TaskMeta`/`SubtaskMeta` 对应 Go `BackfillTaskMeta`/`BackfillSubTaskMeta`，`Extension::GetStepExecutor` 对应 `backfillDistExecutor.GetStepExecutor/newBackfillStepExecutor`，四个 step 的分派相同。
- `Planner::ranges/on_next_subtasks_batch/next_step/modify_meta` 对应 Go `LitBackfillScheduler` 的读索引/临时索引计划、`OnNextSubtasksBatch`、`GetNextStep`、`ModifyMeta`。本地一步、云三步、merge 一步的状态推进保持一致。
- `ReadIndex::TaskMetaModified` 对应 Go `readIndexStepExecutor.TaskMetaModified`：batch size 总是可改，最大写速率只在 local sort 更新；`ResourceModified` 都要求活跃 pipeline 并调整读写 worker。
- `run` 的 task key、任务复用、并发调整、`PauseOnKVDiskFull`、云 URI要求和最终 `GetSubtaskRowCount(BackfillStepReadIndex)` 对应 Go `index.go` 的 task 提交/等待与 `updateDistTaskRowCount` 意图。
- merge 路径对应 Go `mergeTempIndexExecutor.RunSubtask`，但 Rust 将每批事务和 durable checkpoint 放在本文件，实际临时索引回放复用 `modify_column_backfill::merge`；不能据此推断 Go executor 的 metrics、collector、表/index 初始化细节都已移植。
- Node/runtime 绑定是 Rust session 架构的局部接线；Go 使用 DDL 对象、session pool、task runtime Store 和全局 `InitDistTaskLoop`，没有与 Rust `Weak<Domain>` registry 完全同形的类型。

当前可见差异必须保留为差异而非已支持能力：Go executor 会重新解析 user table/index info、附加 profiling/diagnosis、拥有更丰富的 metrics 与通用 retry 分类；Rust `Extension` 主要依赖 Job、Domain 和 index ID，并以文本做有限重试判断。Go scheduler 的 retryable 判断排除特定“数据文件过多”错误，Rust委托 `LitBackfillScheduler::is_retryable_scheduler_message`；两者只有该辅助函数覆盖的消息语义可视为对齐。Go cloud/local pipeline 还有 backend context、分布式锁和更完整的 cleanup/summary 接口，Rust 的对应实现分散在 pipeline、cloud executor 与 import control 中。

相关 Go 测试包括 `pkg/ddl/backfilling_dist_scheduler_test.go` 的 local/global plan、region scan error、next step 和 task meta version，以及 `pkg/ddl/index_nokit_test.go` 的动态 task 参数。它们是迁移语义证据，不等于 Rust 测试已经覆盖同一组合。

## 扩展指南

- 新增 DXF step 时必须同步修改 `Extension::GetStepExecutor`、`Planner::on_next_subtasks_batch`、`next_step`、task/subtask meta 线格式和对应 cloud executor/planner；未知 step 当前应继续安全失败。测试应放在独立 `pkg/session/runtime/*_test.rs`，不要写入本生产文件。
- 调整范围切分/checkpoint 时保持三个不变量：region 覆盖连续、ack/checkpoint 严格前进、只提交连续完成 frontier。至少补独立测试覆盖 region 缺口/重叠、乱序 ack、重复 ack、尾部遗漏、恢复 checkpoint 和空物理表。
- 修改 task/subtask JSON 必须对照 Go struct tag 和历史任务兼容。新增字段应有明确 default/version 策略；不能移除 `start-key/end-key` fallback，除非已有 durable task 全部不可恢复的升级策略。
- 修改动态资源或限速时，同时审查 `Planner::modify_meta`、`ReadIndex::TaskMetaModified/ResourceModified`、pipeline tune 与 `NativeWriteLimiter`。兼容风险是运行中 task meta，性能风险是 CPU/内存分配、5 ms 取消轮询和按 ack 频繁 SQL 更新。
- 修改 NextGen keyspace 路由时，保持 SYSTEM task manager、目标 Store/Domain、runtime handle 三者一致；同步扩展 `masking_policy_dxf_foreign_runtime_checks_store_and_releases_holders` 与 NextGen SYSTEM 执行测试，覆盖错误 keyspace、release 幂等和 manager close。
- 修改 scheduler 生命周期时建议为 slot reservation 引入显式 RAII guard，并验证所有 init/schedule/balance/error 返回路径释放 reservation；同时保持 NodeService stop 能中断同步 loop。
- 修改云路径时同步阅读 `modify_column_cloud_{planner,executor,meta,store}.rs` 和各自独立测试，验证外部 meta 仅在成功后发布、取消能传播、索引组长度与内存除数有效。正确性风险集中在错误 checkpoint/范围与跨 keyspace 路由；兼容风险集中在 Go JSON/历史 task；性能风险集中在 region 数、SQL checkpoint 频率、worker 数和对象存储中间数据。

## 验证依据

- RustCodeGraph：`status` 确认索引可用（11,467 files、307,296 nodes、1,848,419 edges）；`query ReadIndex --kind struct`、`query NodeService --kind struct`、`query TaskRuntimeBinding --kind struct`、`query register_target_runtime --kind function` 精确定位主要符号；`node --file pkg/session/runtime/modify_column_dist_backfill.rs --offset 958 --limit 225` 核对 `run` 的 task/scheduler/row-count 主链。精确 `callers/callees` 查询未返回输出，调用边因此由源码引用搜索补证。
- 目标源与装配：`pkg/session/runtime/modify_column_dist_backfill.rs`、`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`；`pkg/session` 下没有 `doc.go`。
- Rust 直接接线：`pkg/session/runtime/system_session.rs`、`pkg/session/runtime/session_factory.rs`、`modify_column_pipeline.rs`、`modify_column_backfill.rs`、`modify_column_cloud_planner.rs`、`modify_column_cloud_executor.rs`、`modify_column_cloud_meta.rs`、`modify_column_cloud_store.rs`。
- 独立 Rust 测试：`pkg/session/runtime/import_sst_test.rs::read_index_task_meta_updates_live_physical_import_limiter` 验证在途 import 观察限速更新，`read_index_cancel_stops_existing_physical_write_limiter` 验证取消唤醒真实 limiter；`pkg/session/runtime/normal_ddl_masking_policy_test.rs` 覆盖真实 pipeline 资源缩容、`TaskRuntimeBinding` 的 Store/keyspace/holder/release/close，以及 NextGen SYSTEM executor 运行用户 keyspace MODIFY。
- Go 对照：`pkg/ddl/backfilling_dist_executor.go`、`backfilling_dist_scheduler.go`、`backfilling_read_index.go`、`backfilling_merge_temp.go`、`index.go`；Go 测试为 `backfilling_dist_scheduler_test.go` 和 `index_nokit_test.go` 中的计划、步骤、版本和动态参数用例。
- 人工事实复核：本文区分了 owner 调度、executor 执行、云后续步骤、临时索引 merge 和跨 keyspace runtime；未把 RustCodeGraph 的空调用边、Go 独有 metrics/cleanup 或未在本文件消费的 `SubtaskMeta.ts` 描述为已验证支持。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付验证只执行固定 11 章节结构检查、Markdown 源文件链接检查和目标 diff 范围检查。

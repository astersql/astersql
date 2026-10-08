# [`pkg/session/runtime/modify_column_pipeline.rs`](modify_column_pipeline.rs)

## 文件定位

本文件属于 `astersql-session` crate 的私有运行时模块：`pkg/session/runtime.rs` 以 `mod modify_column_pipeline;` 装配它，未向 crate 外公开。它不是 DDL 调度器，而是 `modify_column_dist_backfill.rs` 中 `ReadIndex::run_pipeline` 使用的本地执行流水线，把一个分布式 DDL read-index 子任务拆成“扫描并编码记录”和“写入本地 SST / 云端排序文件”两个并行阶段。

该路径位于持久化 DDL job 的 reorg/backfill 阶段。上游 `ReadIndex::run_pipeline` 负责把子任务键范围切成连续的 region 范围、保存 checkpoint、汇总结果并在全局排序模式下发布外部元数据；本文件只负责范围内的批处理、背压、工作者伸缩和停机。`pkg/session/Cargo.toml` 将该代码归入 `astersql-session`，其直接使用的工作区依赖包括 `astersql-ddl`、`astersql-kv`、`astersql-meta-model`、`astersql-ingestor-simplesst`、`astersql-resourcemanager-pool-workerpool`，并使用 `uuid` 与 `serde_json`。

## 核心职责

1. `Reader::HandleTask` 从 `ReadTask` 给出的物理表键范围开始，按 job 的 batch size 反复调用 `system_session::generate_index_backfill_records`，直到 `next_key` 到达范围末端；每批编码结果经容量为 1 的 `encoded` 通道交给写阶段。
2. `Writer::HandleTask` 根据 `cloud: Option<CloudWriteConfig>` 选择两种落盘语义：本地模式调用 `system_session::write_index_backfill_records` 做唯一键检查并物理导入；云端模式按 index group 把 KV 写入独立的 simple-SST writer，最终在 `Close` 合并每组 `SortedMeta`。
3. `Pipeline` 创建并持有两组 worker pool、三级有界通道、共享取消上下文及结束状态；`feed` 异步提交范围，`tune` 在线调整两级 worker 数，`finish` 有序排空并返回 operator 错误，`shutdown` 用于错误/取消路径的快速唤醒与回收。
4. `with_session` / `close_session` 为每个 worker UUID 在当前 OS 线程的 `thread_local!` 映射中惰性维护 `ConcreteSession`。这保证会话不跨线程移动，并在 worker `Close` 时释放。

## 主要符号

- `SESSIONS: RefCell<HashMap<Uuid, ConcreteSession>>`：线程局部会话表；键是 reader/writer 创建时分配的 UUID。`with_session` 首次访问时创建受限 SQL 会话，`close_session` 在 worker 关闭时移除。
- `failure`：把任意可显示错误规范化为 worker-pool 的文本 `pool::Error`；因此此层不保留具体错误类型。
- `ReadTask { physical, start, end }`：公开范围仅为父模块可见，描述物理表 ID 与半开键区间。其 `TaskMayPanic::RecoverArgs` 标记为 `ddl_backfill/tableScanWorker`。
- `WriteTask { start, records }`：文件私有的跨阶段所有权消息；保留原批次起点和编码后的 `IndexBackfillRecords`。
- `Ack { start, context }`：写成功后的确认，携带原起点与 `BackfillTaskContext`。上游用起点对乱序结果排序，再按 `context.next_key` 推进持久化 checkpoint。
- `Reader` / `Writer`：分别实现 `pool::Worker<ReadTask, pool::None>` 和 `pool::Worker<WriteTask, Ack>`。两者均有独立 UUID、共享 `Domain`、取消上下文和关闭计数器。
- `CloudWriteConfig`：云端写入所需的 `CloudStore`、对象前缀、索引顺序、单索引内存上限、TiKV key prefix 与共享汇总数组。索引向量同时规定 writer group 与 summary group 的位置对应关系。
- `Pipeline`：父模块可见的生命周期对象。主要入口为 `start`、`feed`、`tune`、`finish`、`shutdown`；`closed_workers` 仅在测试配置下用于观测缩容是否真正关闭 worker。

## 执行流程

1. `ReadIndex::run_pipeline` 验证 region ranges 连续覆盖子任务区间，构造 `ReadTask` 列表、`SSTImportOptions`，按是否配置 cloud storage 构造可选 `CloudWriteConfig`，再调用 `Pipeline::start`。
2. `Pipeline::start` 调用 `expected_ingest_worker_count(cpu.max(1), average_row_size, global_sort)` 计算 reader/writer 数量，创建容量均为 1 的 `input`、`encoded`、`results` 通道，启动 `ddl-table-scan` 与 `ddl-index-ingest` 两个 worker pool。全局排序时读写 worker 数相同；本地模式按平均行宽调整比例。
3. `Pipeline::feed` 另起生产线程顺序发送范围。通道容量为 1，所以任务生成、扫描编码、写入、确认之间自然形成背压，不会无界积压完整表数据。
4. reader 对每个范围先检查取消，再读取 `job.reorg_meta`，构造包含 schema/table/index、物理表范围、batch size、resource group 与 SQL mode 的 `IndexBackfillBatch`。扫描在显式 `BEGIN` 后执行，成功或失败均尝试 `ROLLBACK`；只读快照不会提交。
5. reader 要求 `records.context.next_key > start`，否则以“checkpoint did not advance”终止，防止死循环；成功批次发送 `WriteTask` 后继续下一个键段。
6. 本地 writer 在共享 `import_lock` 内开启事务，调用 `write_index_backfill_records`，成功 `COMMIT` 后发送 `Ack`，失败则尝试 `ROLLBACK`。这把在线唯一键检查与物理导入跨并发 chunk 串行化为不可分割的临界区。
7. 云端 writer 首次处理任务时按索引数创建 simple-SST writers；随后按 index ID 定位固定 group 并写入 KV。该分支不调用本地物理导入，批次结束即发送 `Ack`；worker `Close` 才关闭 writer 并把实际文件统计合并到共享 `summaries`。
8. 上游接收的 `Ack` 可以乱序到达；`ReadIndex::run_pipeline` 用 `BTreeMap` 从当前 frontier 连续消费，更新行数和 `mysql.tidb_background_subtask` checkpoint。全部范围确认后调用 `finish`，云端模式随后才依据已经合并的 summaries 写出子任务元数据。

## 数据与状态

- 键范围和 KV 均使用拥有所有权的 `Vec<u8>`；`WriteTask` 移动完整编码批次，不携带借用或跨线程 session/KV handle。`system_session::generate_index_backfill_records` 的注释进一步说明 writer 会从原 record key 重建 handle。
- `Pipeline::context` 是两级 worker 共用的 operator context，用于取消传播和保存首个 operator 错误；`SSTImportOptions::context` 是物理导入/限速器的另一类取消令牌，`shutdown` 会同时触发两者。
- `stopped: AtomicBool` 使 `finish` 与 `shutdown` 幂等竞争：第一个调用方执行实际关闭，后续 `finish` 只回报现有 operator 错误，后续 `shutdown` 直接返回。
- `closed_readers` / `closed_writers` 在各 worker 的 `Close` 中以 `AcqRel` 累加，仅测试接口读取；它们验证 `Tune(..., true)` 缩容确实等待被移除 worker 关闭。
- 云端 `summaries` 是按 `CloudWriteConfig.indexes` 位置对齐的共享向量。`Writer::Close` 在 mutex 下把每个 writer 的 close summary 合并进对应项；索引找不到 group 会立即报错，而不是写入错误分组。
- `options`、`average_row_size`、`global_sort` 保存在 `Pipeline` 中，分别支持取消物理写、重新计算并发和保持 local/cloud 计算规则一致。

## 依赖与调用关系

上游唯一直接生产调用链由源码引用确认：`modify_column_dist_backfill.rs::ReadIndex::run_pipeline` → `Pipeline::start` → `Pipeline::feed`，消费 `Pipeline::results`，完成后调用 `Pipeline::finish`；其 `PipelineGuard::drop` 在任意提前返回时调用 `shutdown` 并 join producer。`ReadIndex::ResourceModified` 通过保存的 `Arc<Pipeline>` 调用 `tune`。

下游主链为：`Reader::HandleTask` → `system_session::generate_index_backfill_records` → `Writer::HandleTask`。本地分支继续到 `system_session::write_index_backfill_records` 和 KV/SST import；云端分支继续到 `astersql_ingestor_simplesst::writer::WriterBuilder`、`CloudStore` 与 `SortedMeta::merge`。worker 数计算委托 `astersql_ddl::backfilling_txn_executor::expected_ingest_worker_count`，调度、通道和错误汇聚委托 `astersql_resourcemanager_pool_workerpool`。

模块入口 `pkg/session/runtime.rs` 只做私有装配。RustCodeGraph 的文件节点确认目标文件含 37 个索引符号，并报告来自 `modify_column_dist_backfill.rs` 及测试相关文件的使用；精确调用关系以源码中的上述直接引用为准。

## 错误处理与边界

- 缺失 `job.reorg_meta`、扫描 checkpoint 不前进、下游通道关闭、取消、cloud index group 缺失、mutex poisoning、session SQL/编码/导入/writer close 错误都会转换为 `pool::Error`，由共享 context/上游返回。
- reader 的只读事务始终以 `ROLLBACK` 收尾；扫描成功但 rollback 失败时 rollback 错误成为结果，扫描失败且 rollback 也失败时只打印 rollback 错误并保留原始扫描错误。
- 本地 writer 只有写入成功且 `COMMIT` 成功才发 `Ack`；失败路径 rollback，因而上游不会把未成功写入的范围推进到 durable frontier。`import_lock` 的 poisoning 也会阻止写入。
- cloud writer 每处理一个 records 分组将 `context.added_count` 加一；这与本地写函数内部的计数路径不同，扩展时不能假设它等于 KV 数或行数，必须保持上游当前只用 `scan_count` 推进 summary 的事实。
- `finish` 的关闭顺序是 input → release readers → encoded → release writers → cancel context。这样 writer `Close` 能在取消前发布最终 cloud summary。`shutdown` 面向失败路径，先取消物理写与 operator context，再关闭全部通道并 release worker，允许丢弃尚未确认的工作。
- `feed` 返回 `JoinHandle` 而不自行 join；其回收责任在 `PipelineGuard`。直接新增调用者时必须复制这一生命周期约束，否则可能遗留提交线程。

## 并发与资源生命周期

两个 worker pool 并行运行，每个具体 session 通过 `thread_local!` 固定在执行该 worker 的线程上。worker `Close` 删除对应 UUID 的 session；`Pipeline::Drop` 再以 `shutdown` 兜底，因此正常完成、错误返回与持有者遗忘显式关闭三条路径都有资源回收入口。

三个容量为 1 的通道限制内存峰值，并把慢 writer 的压力传回 reader 和 producer。`tune` 对两个 pool 分别调用 `Tune(target, true)`；`true` 表示缩容时等待被移除 worker 关闭。测试 `normal_ddl_masking_policy_test.rs` 的真实 ALTER TABLE 路径借助 `scanRecordExec` failpoint 暂停扫描，将 CPU 从 8 调到 2，并断言 reader/writer 各关闭 3 个 worker。

本地 writer 虽可有多个 worker，但共享 `Arc<Mutex<()>>` 把唯一键检查加物理导入串行化；扫描编码仍可并行，且有界通道仍限制驻留批次。云端 writer 不使用该锁，各 worker 持有自己的按索引 writer 数组，结束时才串行合并 summary mutex。`shutdown` 显式取消 `SSTImportOptions.context`，与 `modify_column_dist_backfill.rs::ImportControl` 的框架取消桥配合，唤醒可能阻塞在 store write limiter 的物理写。

## 与 Go 版本的对应关系

Go 对照不是同路径单文件，而分散在 `pkg/ddl/backfilling_operators.go`、`pkg/ddl/backfilling_read_index.go` 与 `pkg/ddl/backfilling_txn_executor.go`：

- Rust `Reader` 对应 Go `tableScanWorker` / `TableScanOperator`；两者都从 session pool/会话上下文扫描表范围，使用 `scanRecordExec` failpoint，并按 reorg batch 配置产生下游记录。Rust 将扫描结果编码成拥有所有权的 KV，且显式用短事务 `BEGIN`/`ROLLBACK`。
- Rust `Writer` 的 cloud 分支对应 Go `WriteExternalStoreOperator` 创建的每索引 simple-SST writers；本地分支对应 Go `IndexIngestOperator` / `indexIngestWorker`。Rust 当前用 `import_lock` 强制本地唯一检查与物理导入不可交错，这是其会话/KV 适配层下的必要局部接线。
- Rust `Pipeline::tune` 对应 Go `readIndexStepExecutor.ResourceModified`：两者都用 `expectedIngestWorkerCnt`/`expected_ingest_worker_count` 按 CPU、平均行宽和 global-sort 标记计算两级并发，再等待式调整 worker pool。
- Rust `finish` 的有序排空对应 Go `executeAndClosePipeline` 的成功关闭意图；Rust `PipelineGuard` + `shutdown` 对应 Go `currPipe` 生命周期和 defer 清理的失败路径。
- Go `expectedIngestWorkerCnt` 与 Rust 实现的比例表、未知行宽逻辑和 global-sort 等并发语义一致；`pkg/ddl/backfilling_txn_executor_test.go` 与 `pkg/ddl/backfilling_txn_executor_test.rs::test_expected_ingest_worker_count` 提供对照用例。

当前 Rust 文件不是逐行复刻 Go operator 类型：它把 `ConcreteSession` 适配、拥有所有权的编码批次、物理导入取消令牌和 cloud summary 合并集中在一个较窄的 session-runtime 流水线中。应以可观察语义与上游 checkpoint 约束对齐为准，不应把类型名差异解释为功能缺失。

## 扩展指南

- 修改扫描请求字段、batch 推进或 SQL mode/resource group 传播时，入口应放在 `Reader::HandleTask`，并同步检查 `system_session::generate_index_backfill_records`；必须保留 `next_key > start` 不变量和只读事务回滚。
- 修改本地重复键、锁或导入语义时，应改 `system_session::write_index_backfill_records`，而不是绕过 `Writer` 的 `import_lock`。需要增加独立的 `*_test.rs`，不要把测试内嵌进本生产文件。
- 新增 cloud index 分组或 writer 元数据时，应同步维护 `CloudWriteConfig.indexes`、`external_writers` 与 `summaries` 的同序关系，并覆盖 writer `Close` 失败、空索引组和多 worker summary 合并。
- 修改并发算法应优先改 `expected_ingest_worker_count` 及其独立测试 `pkg/ddl/backfilling_txn_executor_test.rs`，同时核对 Go `expectedIngestWorkerCnt` 与 `backfilling_txn_executor_test.go`，避免 Rust/Go 资源调度漂移。
- 修改关闭顺序或取消传播时，需要覆盖成功 `finish`、提前错误触发 `PipelineGuard::drop`、显式 framework cancel、blocked write limiter 和重复 stop。现有直接证据包括 `normal_ddl_masking_policy_test.rs` 的在线缩容场景，以及 `import_sst_test.rs::read_index_cancel_stops_existing_physical_write_limiter` 的取消唤醒场景。
- 性能风险主要来自扩大通道、缩短/移除本地导入锁或错误调整读写比例；兼容风险主要来自 checkpoint 在未 durable 前推进、cloud summary 未完全 close 即发布、或 SQL mode/resource group 丢失。

## 验证依据

- RustCodeGraph：`status` 显示项目索引含目标文件；`files --filter pkg/session/runtime` 列出该文件；`node --file pkg/session/runtime/modify_column_pipeline.rs --offset 1 --limit 400` 与尾段查询读取完整 426 行，并给出 37 个符号及使用文件。精确 `callers` 查询未产生边输出，因此调用边由源码引用补证，未据此臆造间接调用者。
- 目标与装配：`pkg/session/runtime/modify_column_pipeline.rs`、`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`。
- 直接入口与生命周期：`pkg/session/runtime/modify_column_dist_backfill.rs` 中 `ReadIndex::run_pipeline`、`PipelineGuard::drop`、`ResourceModified`、`closed_pipeline_workers_for_test`。
- 下游实现：`pkg/session/runtime/system_session.rs` 中 `IndexBackfillRecords`、`generate_index_backfill_records`、`write_index_backfill_records`；`pkg/ddl/backfilling_txn_executor.rs::expected_ingest_worker_count`。
- Go 对照：`pkg/ddl/backfilling_operators.go` 的 `tableScanWorker`、`indexIngestWorker` 与外部 writer operator；`pkg/ddl/backfilling_read_index.go` 的 pipeline 执行、关闭和资源调整；`pkg/ddl/backfilling_txn_executor.go::expectedIngestWorkerCnt`。
- 测试证据：`pkg/session/runtime/normal_ddl_masking_policy_test.rs` 验证真实 modify-column 扫描期间缩容关闭 worker；`pkg/session/runtime/import_sst_test.rs` 验证 framework 取消唤醒现有物理写限速器；`pkg/ddl/backfilling_txn_executor_test.rs` 与 Go `pkg/ddl/backfilling_txn_executor_test.go` 验证并发计算。未发现与目标文件同名的独立测试文件。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定标题结构检查、路径/符号复核和 diff 自审作为验证。

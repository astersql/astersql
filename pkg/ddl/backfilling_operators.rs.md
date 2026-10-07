# `pkg/ddl/backfilling_operators.rs`

## 文件定位

本说明对应源文件 [backfilling_operators.rs](backfilling_operators.rs)。它位于 `astersql-ddl` crate，模块由 `pkg/ddl/lib.rs` 以 `pub mod backfilling_operators` 公开。文件把在线 `ADD INDEX` 的存量数据回填拆成“范围切分 → 行扫描 → 索引写入 → 结果汇总”四段，并同时提供临时索引增量合并所需的任务、存储抽象和 worker。这里处理的是 DDL reorg 阶段的数据路径，不负责创建 DDL job、推进 schema state、更新 schema version 或持久化 reorg checkpoint。

当前 Rust 接线范围需要特别区分：`MergeTemporaryIndexWorker` 已被 `pkg/ddl/backfilling_merge_temp.rs::MergeTemporaryIndexExecutor::run_subtask` 调用；`run_add_index_pipeline` 和 `NewAddIndexIngestPipeline` 在 Rust 树中的直接调用者是独立测试 `backfilling_operators_async_test.rs`，尚未接到与 Go `ddlCtx.addIndexWithLocalIngest` 等价的完整生产入口。`MemoryIndexWriter` 也是测试/演示后端，不代表真实 TiKV 或 Lightning 引擎。

## 核心职责

- `TableScanTaskSource` 根据 checkpoint 和外部提供的 `KeyRange` 生成左闭右开扫描任务，使用 `TaskIdAllocator` 分配任务号。
- `TableScanWorker` 从调用者提供的 `IndexRecord` 切片中筛选任务范围内的行、按动态或静态 batch size 分块，并可在单索引条件下模拟 partial-index 条件下推。
- `IndexIngestWorker<W: IndexWriter>` 把 chunk 中的索引键值写入抽象 writer，保留扫描行数与实际索引写入数之间的区别。
- `IndexWriteResultSink` 累计扫描行数、逐结果执行 quota 检查、最终 flush，并读取 writer 的权威键数。
- `AddIndexIngestPipeline` 用线程、可关闭队列和共享取消/首错状态并发执行扫描与写入阶段；`run_add_index_pipeline` 是较简单的同步组合版本。
- `MergeTemporaryIndexWorker<S: TemporaryIndexStore>` 在原子 batch 中把临时索引变更应用到正式索引并删除临时键，对可重试错误缩小 batch 后重试。

这些职责由文件内真实符号支撑，但 Rust 实现以预先提供的 ranges/rows 和抽象 writer/store 为边界；Region 加载、事务 session、表达式求值、checkpoint 持久化、指标和真实 ingest 引擎仍只存在于 Go 对照或其他模块中。

## 主要符号

- `OperatorError`：统一表达取消、非法范围、写入/flush 失败和临时索引重试耗尽。`InvalidCheckpoint` 已定义，但当前 `adjust_start_key` 对越界 checkpoint 采用 Go 兼容的回退行为，并不会构造该错误。
- `TableScanTask` / `IndexRecord` / `IndexRecordChunk`：扫描任务、已编码索引记录以及扫描到写入阶段的传输单元。`table_scan_row_count` 记录过滤前扫描行数，`condition_pushed` 决定写入阶段能否省略 partial-index 复查。
- `TableScanTaskSource::{adjust_start_key, generate_tasks}`：应用 checkpoint、补齐无界 Region 边界并与整体回填范围求交；`physical_table_id` 只携带上下文，当前函数不据此加载 Region。
- `TableScanWorker::scan_records`：读取 `reorg_batch_size`（若有）并输出至少一个带 `done=true` 的 chunk；无匹配行时也输出空完成块。
- `IndexWriter`、`IndexIngestWorker::write_chunk`、`IndexWriteResultSink::{collect_results, flush}`：索引存储边界、逐 chunk 写入和终点汇总。单索引且扫描端已下推条件时才可完全跳过二次条件检查。
- `run_add_index_pipeline`：同步遍历 tasks/chunks 的参考组合函数，不创建线程、不执行最终 sink/flush。
- `BackfillOperatorContext` / `NewLocalWorkerCtx`：每条 pipeline 独立的取消位与“首个 operator 错误”容器；Go 风格方法名是为了移植接口可识别性。
- `TableScanOperator` / `IndexIngestOperator`：保存 stage 配置、开关状态和 worker 数；`TuneWorkerPoolSize` 影响下一次 `Execute` 捕获的并发度，最小为 1。
- `AddIndexIngestPipeline` / `NewAddIndexIngestPipeline`：异步 pipeline 的生命周期门面；`Execute` 打开 stage 并启动协调线程，`Close` join、关闭 stage 并传播结果，`Summary` 返回最终统计快照。
- `StageQueue<T>`：基于 `Mutex<VecDeque<T>> + Condvar` 的内部多生产者/多消费者队列，支持显式 close 唤醒等待者。
- `AddIndexPipelineFailpoints` 与五个路径常量：pipeline 实例级、单次故障注入配置，避免并发 DDL 任务共享全局一次性状态。
- `TemporaryIndexScanTask`、`TemporaryIndexRecord`、`TemporaryIndexMutation`、`TemporaryIndexStore`：临时索引扫描与原子写入边界。
- `MergeTemporaryIndexWorker::handle_one_range`：选取一批范围内记录，生成正式键 Set/Delete 与临时键 Delete，处理重试并计算 `next_key`。
- `MemoryIndexWriter`：使用 `BTreeMap` 保存数据并记录 flush/quota 调用的测试实现。

本文件没有 feature gate 或条件编译项；测试模块的条件编译在 `pkg/ddl/lib.rs` 中声明。

## 执行流程

同步 ADD INDEX 路径 `run_add_index_pipeline`：

1. 新建 `TaskIdAllocator`，调用 `TableScanTaskSource::generate_tasks`。checkpoint 等于整体结束键时直接得到空任务；越界 checkpoint 回退到原起点。
2. 对每个与 `[adjusted_start, end_key)` 相交的 Region 范围创建任务；空范围边界分别补成 `record_prefix` 和 `prefix_next(record_prefix)`。
3. `TableScanWorker::scan_records` 校验范围与 batch size，从内存 `rows` 选出 `[task.start, task.end)` 内记录并切块。条件下推时先过滤不匹配 partial index 的记录，但 `table_scan_row_count` 仍保留过滤前数量。
4. `IndexIngestWorker::write_chunk` 检查 chunk 自带错误，按条件规则写入 `IndexWriter`，返回扫描行数、写入字节数及 `done`。该同步助手到此结束，调用者若需要 quota 检查和 flush，必须再交给 sink。

异步 ADD INDEX 路径 `AddIndexIngestPipeline::{Execute, Close}`：

1. `Execute` 拒绝重复启动，依次打开 scan/ingest stage；ingest 打开失败时回滚关闭 scan。
2. 后台协调线程调用 `execute_add_index_pipeline`，预先生成全部任务并关闭 task queue；随后先启动 ingest workers，再启动 scan workers。
3. scan workers 从 task queue 取任务并向 chunk queue 发送；ingest workers 从 chunk queue 取块、共享同一个受锁保护的 writer，并向 result queue 发送统计。首个错误写入 `BackfillOperatorContext` 并广播取消。
4. 协调线程依次 join 全部 scan workers、关闭 chunk queue、join ingest workers、关闭并清空 result queue。任何取消都会在 flush 前返回 `Cancelled`。
5. 正常路径构造 `IndexWriteResultSink`，逐结果做 quota 检查并 flush，最后发布 `AddIndexPipelineSummary`。
6. 外层 `Close` join 协调线程，再关闭两个 stage；线程 panic 会记录写错误并对外返回 `Cancelled`。

临时索引合并路径：`MergeTemporaryIndexExecutor::run_subtask` 构造逐步前移的 `TemporaryIndexScanTask`，反复调用 `handle_one_range`。worker 每次最多选择 `batch_count.max(1)` 条记录；非 `skip` 记录先对正式索引 Set/Delete，再删除临时键，整个 mutation 列表由 `TemporaryIndexStore::apply_batch` 原子提交。成功后从最后扫描键的 `prefix_next` 继续；空批次直接把 `next_key` 设为任务终点。

## 数据与状态

所有键都以 `Key`（字节向量）按字典序比较，扫描区间统一为 `[start, end)`。`prefix_next` 从末字节进位；全为 `0xff` 时当前实现会把各字节归零并追加 `0`，扩展键编码时必须与该既有行为及 Go `PrefixNext` 语义一起核验。

checkpoint 是 `TableScanTaskSource::checkpoint_key` 的输入快照：文件本身不更新或持久化 checkpoint。任务号由每次 pipeline 新建的 `TaskIdAllocator` 分配。`IndexWriteResult.row_count` 是扫描行数而非成功写入索引条数，这是 partial-index 下 quota/progress 统计保持 Go 语义的关键不变量；`written_bytes` 才反映真实写入。

异步状态包括 `started/opened` 生命周期位、`BackfillOperatorContextInner.cancelled`、互斥保护的首错、三个 stage queue、受锁保护的 writer 和 summary。summary 仅在完整 sink 成功后一次性覆盖；错误或取消时可能仍保持默认值。

临时索引 worker 的 `batch_count` 在可重试错误时按一半退避，但无论成功或失败都会恢复进入函数时的原始值；`total_scan_count` 只在成功提交后增加。`skip` 记录计入扫描数与范围推进，但不生成 mutation，也不计入 `add_count`。

## 依赖与调用关系

crate 边界由 `pkg/ddl/Cargo.toml` 确认：包名为 `astersql-ddl`，库入口是 `lib.rs`。本文件直接使用标准库集合、原子量、互斥量、条件变量和线程，以及 crate 内 `backfilling::{Key, KeyRange}`、`backfilling_txn_executor::TaskIdAllocator`；它没有直接使用 Cargo 中声明的 TiKV、ingest、session 或 dist-task 依赖。

已核验的 Rust 调用边：

- `pkg/ddl/lib.rs` 公开模块，并在 `cfg(test)` 下装配 `backfilling_operators_test.rs` 与 `backfilling_operators_async_test.rs`。
- `pkg/ddl/backfilling_operators_async_test.rs::pipeline` 调用 `NewAddIndexIngestPipeline`；两个测试再调用调优、执行、关闭、summary 与 failpoint API。
- `pkg/ddl/backfilling_merge_temp.rs::MergeTemporaryIndexExecutor::new` 构造 `MergeTemporaryIndexWorker`，`run_subtask` 调用 `handle_one_range` 并校验 `next_key` 必须前进。
- `pkg/ddl/backfilling_test.rs` 复用 `IndexRecord`、`TableScanTask` 和 `TableScanWorker`；`backfilling_merge_temp_test.rs` 从更高层覆盖临时索引执行器。

RustCodeGraph 将目标识别为 116 个符号，并能定位上述主要定义；其 `callers/callees` 查询本次在限定 Rust 符号后长时间无输出，因此调用边由精确 `rg` 与相邻文件源码补齐。搜索没有发现 Rust 生产模块调用 `run_add_index_pipeline` 或 `NewAddIndexIngestPipeline`，故不能声称 Rust ADD INDEX 已接入完整 DDL 主链。

Go 侧真实上游包括 `backfilling.go::ddlCtx.addIndexWithLocalIngest` 和 `backfilling_read_index.go::readIndexStepExecutor`，它们构建同名 pipeline 后交给 operator 框架执行；这是架构位置的对照证据，而不是 Rust 已完成同等接线的证据。

## 错误处理与边界

- `generate_tasks` 和 `scan_records` 拒绝 `start >= end`；后者还拒绝有效 batch size 为 0。越界 checkpoint 不报错，而是回退到原始起点；等于 end 表示完成。
- `IndexIngestWorker::write_chunk` 优先传播 chunk 内错误，并把 writer 字符串错误映射为 `OperatorError::Write`；sink 分别映射 quota/write 和 flush 错误。
- 异步 pipeline 只保存首个 operator 错误，但外层结果可能统一为 `Cancelled`。例如扫描或写入 stage 触发错误后，context 保留具体 `Write`，协调线程看到取消后返回 `Cancelled`；flush 错误则直接返回具体 `Flush`。
- `Execute` 拒绝重复启动，`EnableFailpoint` 拒绝启动后修改；`Close` 在已启动但 handle 缺失时返回写错误。未启动直接 `Close` 是成功空操作。
- `StageQueue::send` 在 close 后返回 `false`；`recv` 会先清空已排队数据，再在“空且关闭”时返回 `None`。队列不设容量，因此缺少背压，输入很大时存在内存峰值风险。
- 多处锁使用 `expect`，poison 会 panic；worker panic 会在 join 处转换为 context 错误，但协调线程自身 panic 最终只映射为 `Cancelled`。
- 临时存储的 `Retryable` 在 `maximum_attempts.max(1)` 次后变为 `RetryExhausted`；`Fatal` 映射为 `Write`。当前实现没有 Go 路径中的时间退避，且只处理 trait 暴露的原子 batch 结果。

## 并发与资源生命周期

`BackfillOperatorContext` 使用 `Release/Acquire` 发布和读取取消状态，首错由 `Mutex<Option<OperatorError>>` 保护。故障注入的单次触发位使用 `swap(AcqRel)`；扫描完成计数用 `fetch_add(AcqRel)`。动态 reorg batch size 由 `AtomicUsize` 的 Acquire load 读取。

`Execute` 仅启动一个协调线程；协调线程内部按配置创建多个 scan 与 ingest OS thread。三个 `StageQueue` 用条件变量阻塞等待并通过 `close + notify_all` 终止。任务先整体入队，因此 task queue 不会等待生产者；chunk/result queue 依赖明确的 join 顺序和 close 顺序防止消费者永久等待。

所有 ingest workers 共享 `Arc<Mutex<W>>`，`write`、quota 检查、flush 和计数读取最终都串行进入 writer；提高 ingest worker 数可以并行处理外围逻辑，但不会并发执行同一 writer 的写调用。`Close` 是资源回收与结果可见性的同步点：它等待后台线程完成后才关闭 stage。调用者若只 `Execute` 而不 `Close`，无法获得完成错误或最终 summary；该类型没有自定义 `Drop` 自动 join。

临时索引 worker 自身是同步的，事务原子性由 `TemporaryIndexStore::apply_batch` 实现者保证。本文件不创建数据库事务、不持有 session，也不负责 owner 切换后的恢复；这些生产生命周期必须由上层执行器与存储实现提供。

## 与 Go 版本的对应关系

`pkg/ddl/backfilling_operators.go` 是主要语义来源。双方都保留了 task source、table scan、index ingest、result sink、临时索引 merge 以及 reader/writer 并发调优的分段模型。已对齐的细节包括：越界 checkpoint 回退、checkpoint 等于 end 表示完成；partial-index 条件下推只在单索引时允许跳过 TiDB 侧复查；进度采用过滤前 `tableScanRowCount`；sink 在结果耗尽后做 ingest/flush；临时索引按正式键变更后删除临时键，并在可重试事务错误时缩小 batch。

Rust 并非 Go 生产实现的等量替代：

- Go `TableScanTaskSource` 自行通过 store 加载 Region，关联真实 physical table 与 checkpoint operator；Rust 接收预制 `KeyRange`，`physical_table_id` 当前不参与加载。
- Go scan worker 从 session pool 获取 session、构建 DistSQL context、在事务中扫描 chunk、更新 checkpoint/metrics；Rust 只过滤调用者传入的 `Vec<IndexRecord>`。
- Go ingest worker 管理 Lightning writers、session context、表达式 checker、错误转换和 chunk pool；Rust 依赖简化的 `IndexWriter` trait，多个 worker 共享且串行锁住同一个 writer。
- Go pipeline 使用通用 `operator.AsyncPipeline` 与 worker pool，并由真实 local-ingest / dist-task 入口构建；Rust 自建 OS thread 和无界队列，ADD INDEX pipeline 当前只有测试直接调用。
- Go merge worker 在 `kv.RunInNewTxn` 内读取临时键、校验唯一键、退避重试并记录指标；Rust 要求调用者预先提供记录，只把 mutation 的原子应用交给 `TemporaryIndexStore`，没有退避计时和唯一键检查。
- Go failpoint 是进程级注入点；Rust 用每 pipeline 配置的同路径常量模拟一次性触发，便于并发测试但不是 Go failpoint 框架接线。

因此扩展时应以 Go 行为和 Rust 独立测试共同校验语义，不能因为同名 API 就假设存储、事务、checkpoint 或生产接线已经等价。

## 扩展指南

- 修改范围切分/checkpoint：优先改 `TableScanTaskSource::{adjust_start_key, generate_tasks}`，同步扩展 `backfilling_operators_test.rs`；重点覆盖空边界、相交裁剪、越界/完成 checkpoint 和 `start >= end`。若引入真实 Region 加载，应在上层注入抽象，避免把 DDL job/schema 生命周期塞进本文件。
- 修改 partial index 或行数语义：同时检查 `TableScanWorker::scan_records`、`IndexIngestWorker::write_chunk` 与 `IndexWriteResultSink`；必须保持“扫描进度”和“实际索引条目数”分离，并与 Go `IndexRecordChunk.tableScanRowCount`、`WriteChunk` 对照。
- 修改异步拓扑或并发调优：集中在 `StageQueue`、`execute_add_index_pipeline`、两个 operator 的 tuning 方法及 `Close` 顺序；在 `backfilling_operators_async_test.rs` 增加独立回归，验证取消时无死锁、队列关闭、首错保留、writer flush 次数和 summary 发布。
- 新增错误/failpoint：必须明确它发生在 scan、write 还是 flush 边界，以及外层 `Close` 错误与 `OperatorErr` 是否相同；保持 pipeline 实例隔离，避免并发任务互相消费故障状态。
- 修改临时索引合并：改 `MergeTemporaryIndexWorker::handle_one_range` 和相邻 `backfilling_merge_temp.rs` 接线，并在 `backfilling_merge_temp_test.rs` 覆盖 Set/Delete/skip、原子提交、retry/fatal、batch 恢复、`next_key` 严格前移与分区/全局索引统计。
- 接入真实 Rust ADD INDEX 主链前，需要另行设计 store/session/Region/checkpoint/ingest adapter，并从 DDL job 的 reorg 生命周期调用；不能把 `MemoryIndexWriter` 当作生产后端。兼容风险集中在 checkpoint 编码、partial-index 表达式、多索引条件、Go 错误分类和重启幂等性；性能风险集中在无界队列、预先装载全部 rows/tasks、OS thread 数及全局 writer mutex。

测试逻辑应继续放在同目录独立 `*_test.rs` 文件，并由 `lib.rs` 的 `cfg(test)` 模块装配，不应嵌入生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件，`files --filter pkg/ddl/backfilling_operators.rs` 命中目标；`node --file ...` 读取了 1–1316 行完整源码；`query` 定位 `OperatorError`、Rust/Go 两个 `NewAddIndexIngestPipeline`、`run_add_index_pipeline`、`handle_one_range` 和 `execute_add_index_pipeline`。限定 Rust 符号的 `callers/callees` 查询超过 90 秒无输出后中止，调用边改由精确文本搜索和相邻源码验证。
- Rust 源与配置：`pkg/ddl/backfilling_operators.rs`、`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、`pkg/ddl/backfilling.rs`、`pkg/ddl/backfilling_txn_executor.rs`、`pkg/ddl/backfilling_merge_temp.rs`。
- Rust 独立测试：`pkg/ddl/backfilling_operators_test.rs` 验证越界 checkpoint 回退和 partial-index 过滤后仍报告扫描行数；`pkg/ddl/backfilling_operators_async_test.rs` 验证并发调优、完整执行、quota/flush/summary 以及五个故障边界；`pkg/ddl/backfilling_merge_temp_test.rs` 验证上层临时索引初始化、全局/分区选择及 collector 语义。
- Go 对照：`pkg/ddl/backfilling_operators.go`；生产入口由 `pkg/ddl/backfilling.go::ddlCtx.addIndexWithLocalIngest` 和 `pkg/ddl/backfilling_read_index.go` 核验，相关故障注入测试入口见 `pkg/ddl/ingest/integration_test.go`。
- DDL 架构上下文：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`、`docs/agents/ddl/03-reorg-backfill.md`。这些文档只作导航，本文行为结论以源码和测试为准。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行任务规定的 11 章节结构检查，并人工复核链接、现状/Go 对照边界和未接线声明。

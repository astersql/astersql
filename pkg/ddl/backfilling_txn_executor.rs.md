# `pkg/ddl/backfilling_txn_executor.rs`

对应源码：[`backfilling_txn_executor.rs`](./backfilling_txn_executor.rs)。

## 文件定位

该文件属于 `astersql-ddl` crate 的在线 DDL 回填基础设施。`pkg/ddl/Cargo.toml` 通过 `[lib] path = "lib.rs"` 定义 crate 入口，`pkg/ddl/lib.rs` 以 `pub mod backfilling_txn_executor` 对外公开本模块，并在测试配置下以独立文件 `backfilling_txn_executor_test.rs` 接入单元测试。

当前 Rust 文件同时容纳四类小型基础能力：重组扫描的 `DistSQLContext` 构造、回填配置到简化会话状态的临时切换、事务回填执行器的内存模型，以及 ingest 流水线的 worker 配比和任务 ID 分配。它处在 DDL reorg/backfill 阶段，而不是 SQL DDL 语句提交、DDL job 持久化、schema state 推进或 schema version 同步的入口。就 DDL 生命周期问题而言，它服务于需要扫描/重写存量数据的 reorg 阶段；本文件自身不保存 checkpoint，不操作 `mysql.tidb_ddl_reorg`，也不实现取消后的回滚或 delete-range GC。

需要特别区分“模块名表达的目标角色”和当前接线事实：Go 同名文件的 `txnBackfillExecutor` 会建立异步 worker、channel 和事务回填器；Rust 的 `TxnBackfillExecutor` 目前只保存 worker 槽位及两个 FIFO 队列，尚未执行存储事务。仓库中的生产调用主要落在 `expected_ingest_worker_count` 和 `TaskIdAllocator`，而 `TxnBackfillExecutor`、两个 DistSQL context 构造器及简化的 `SessionContext` 当前只在独立测试中直接使用。

## 核心职责

- `new_default_reorg_dist_sql_context` 为单遍 DDL 重组扫描创建 DistSQL 上下文，强制 `NotFillCache = true`，避免一次性全表/索引扫描挤占前台查询的 TiKV block cache；同时打开 chunk RPC，并用同一 `WarnAppenderRef` 构建 warning handler 与 error context。
- `new_reorg_dist_sql_context_with_reorg_meta` 在默认上下文上补入持久化 `DDLReorgMeta.ResourceGroupName`，使请求归属相应资源组。当前 Rust 版本没有移植 Go 构造器设置时区、SQL mode 错误级别、KV client、内存追踪器等完整逻辑。
- `ReorgMeta` 和 `SessionContext::{initialize_for_reorganization, restore}` 描述并实现一组简化的“保存旧会话状态—应用回填参数—整体恢复”流程，其中 batch size 被规范为至少 1。
- `TxnBackfillExecutor` 管理同一 `BackfillerType` 的逻辑 worker 槽位、待处理 `ReorgBackfillTask` 队列、已完成 `BackfillResult` 队列及关闭状态。它提供可测试的容量上限、FIFO 与关闭语义，但没有调度循环。
- `expected_ingest_worker_count` 按 CPU/并发度、平均行宽和 global sort 开关估算 ingest 读写阶段并发；该函数被 DDL read-index 执行器和 session modify-column pipeline 的启动/动态调节路径实际调用。
- `TaskIdAllocator` 从 0 开始单调分配 `usize` ID；`backfilling_operators.rs` 用它为按键范围切分出的扫描任务编号，从而维持任务身份和结果归并依据。

## 主要符号

- `pub fn new_default_reorg_dist_sql_context(warn_handler: WarnAppenderRef) -> DistSQLContext<'static>`：基于 `DistSQLContext::default()` 覆盖 warning、chunk RPC、cache 和 error context 字段。`WarnHandler` 使用一次 `Arc` clone，`ErrCtx` 消费原引用。
- `pub fn new_reorg_dist_sql_context_with_reorg_meta(reorg_meta: &DDLReorgMeta, warn_handler: WarnAppenderRef) -> DistSQLContext<'static>`：复用默认构造器，仅进一步复制 `ResourceGroupName`。
- `pub const MAX_BACKFILL_WORKER_SIZE: usize = 16`：txn worker 槽位及“缺少行宽统计”分支的 reader/writer 上限；它不限制有行宽统计时的 ingest reader 数，也不限制 global sort 分支。
- `pub struct ReorgMeta`：持有 `concurrency`、`batch_size`、`max_write_speed`、strict mode、时区、资源组和云存储开关。默认值是并发 1、批量 256、不限速、非 strict、UTC、空资源组、不开云存储。当前文件只消费其中 strict mode、时区、资源组和 batch size；其余字段是供外围流程表达配置的状态。
- `pub struct SessionContext` / `pub struct SessionSnapshot(SessionContext)`：前者是四字段的简化回填会话状态，后者封装修改前的完整 clone；快照内部字段私有，恢复只能通过 `restore` 完成。
- `pub struct WorkerSlot { pub id, pub worker_type }`：逻辑 worker 描述。扩容时 ID 取当前 `workers.len()`，因此连续增长；缩容后再次扩容会复用尾部 ID。
- `pub enum ExecutorError { Closed, InvalidConcurrency }`：执行器本地的、无附加上下文的可比较错误。`Closed` 只由任务/结果入队返回，`InvalidConcurrency` 只由 `setup_workers(0)` 返回。
- `pub struct TxnBackfillExecutor`：私有字段包含固定的 `worker_type`、worker `Vec`、任务与结果 `VecDeque`、`closed` 标志。公开方法为 `new`、`setup_workers`、`adjust_worker_size`、`worker_count`、`send_task`、`take_task`、`push_result`、`result`、`close`、`is_closed`。
- `pub fn expected_ingest_worker_count(concurrency, average_row_size, global_sort) -> (usize, usize)`：返回 `(reader, writer)`。非 global sort 且有统计时，reader 比例按 `<=200 / <=500 / <=1000 / <=3000 / >3000` 字节映射为 `0.5 / 1 / 2 / 4 / 8`，writer 为至少 1 的原并发度。
- `pub struct TaskIdAllocator`：私有 `next_id` 记录下一个 ID；`new` 等价于默认值 0，`alloc` 先返回当前值再自增。

## 执行流程

1. 创建重组扫描上下文时，调用方传入 warning appender；默认构造器设置 `EnableChunkRPC`、`NotFillCache` 和 `ErrCtx`。若有持久化 job metadata，再由 `new_reorg_dist_sql_context_with_reorg_meta` 把资源组名覆盖到上下文。
2. 简化会话切换时，`initialize_for_reorganization` 先 clone 整个旧 `SessionContext`，再写入 `ReorgMeta` 的 strict mode、时区和资源组，并执行 `meta.batch_size.max(1)`。调用方持有返回的 `SessionSnapshot`，在回填结束路径调用 `restore` 原子式覆盖四个字段。
3. txn 执行器由 `TxnBackfillExecutor::new(worker_type)` 创建，此时 worker/任务/结果均为空且未关闭。`setup_workers(n)` 拒绝零并发，否则委托 `adjust_worker_size`；后者把目标裁剪为至多 16，先截断多余槽位，再按连续 ID 补齐。
4. `send_task` 将 `ReorgBackfillTask` 压入队尾，`take_task` 从队首取出；`push_result` 与 `result` 对结果做同样的 FIFO 操作。本文件没有把取出的任务交给 `Backfiller`，也没有把执行结果自动压回结果队列，这两步必须由外围逻辑完成。
5. `close(false)` 标记关闭并清空 worker，但保留已排队任务和结果供读取；`close(true)` 还丢弃两队列。关闭后入队会返回 `Closed`，但读取遗留队列仍被允许，重复关闭也是幂等赋值/清理。
6. ingest 路径在流水线启动或资源调节时调用 `expected_ingest_worker_count`。global sort 直接返回 `(concurrency, concurrency)`；无平均行宽时两侧按约半数分配并各自钳制到 1..=16；有统计时 reader 按行宽比例计算而 writer 保持至少 1。`pkg/ddl/backfilling_read_index.rs::run_subtask/resource_modified` 和 `pkg/session/runtime/modify_column_pipeline.rs::Pipeline::start/tune` 随后把结果写入各自流水线状态或 worker pool。
7. ADD INDEX 算子通过 `TableScanTaskSource::generate_tasks` 接收 `TaskIdAllocator`。同步 `run_add_index_pipeline` 和异步 `execute_add_index_pipeline` 都创建新 allocator，使一次流水线生成的任务从 0 连续编号。

## 数据与状态

`TxnBackfillExecutor` 的状态完全驻留内存，没有序列化、durable checkpoint 或重启恢复。`task_queue` 元素是 `backfilling.rs::ReorgBackfillTask`，包含物理表 ID、job ID、左闭右开键范围、任务 ID 和事务优先级；`result_queue` 元素是 `BackfillResult`，包含推进键、扫描/写入计数、warning 汇总及可选错误字符串。队列本身只维持插入次序，不校验键范围、任务 ID 唯一性或结果是否与已发送任务对应。

worker 池也只是 `Vec<WorkerSlot>`：槽位共享构造时固定的 `BackfillerType`，没有 session、线程句柄、channel、事务对象或 running/stopped 状态。`adjust_worker_size(0)` 合法并清空池，而首次设置用的 `setup_workers(0)` 明确报错；这是两个 API 的有意语义差异，独立测试已固定该行为。

`SessionSnapshot` 通过拥有一份 clone 来隔离恢复值，恢复时移动快照，无共享可变状态。`ReorgMeta.concurrency`、`max_write_speed`、`use_cloud_storage` 在本文件的会话初始化中不生效；不能仅因字段存在就推断这里执行了限速、并发调度或 global sort。

`TaskIdAllocator` 使用普通 `usize` 加一。它适用于单线程、单流水线内由可变借用串行分配；源码未处理整数溢出，也没有跨进程稳定性。任务 ID 的持久性若有需要，必须由上层任务元数据承担。

## 依赖与调用关系

直接 Rust 依赖只有三组：标准库 `VecDeque`；`astersql-distsql-context::{DistSQLContext, WarnAppenderRef, errctx}`；`astersql-meta-model::group_3::DDLReorgMeta`；以及同 crate 的 `backfilling::{BackfillResult, BackfillerType, ReorgBackfillTask}`。这些依赖均由 `pkg/ddl/Cargo.toml` 的 `astersql-distsql-context`、`astersql-meta-model` 和 crate 内模块边界支持，没有条件编译项。

已核实的上游调用关系如下：

- `pkg/ddl/backfilling_read_index.rs::run_subtask` 在建立 read-index pipeline 时调用 `expected_ingest_worker_count`，`resource_modified` 在资源变化时再次调用并更新已有 pipeline。
- `pkg/session/runtime/modify_column_pipeline.rs::Pipeline::start` 用该函数决定实际 `ddl-table-scan` 和 `ddl-index-ingest` worker pool 的初始大小，`Pipeline::tune` 用同一规则在线调节。
- `pkg/ddl/backfilling_operators.rs::TableScanTaskSource::generate_tasks` 在生成任务时调用 `TaskIdAllocator::alloc`；同步和异步 ADD INDEX 流水线入口分别创建 allocator。
- `pkg/ddl/lib.rs` 公开本模块；`backfilling_txn_executor_test.rs` 直接覆盖 DistSQL context、worker 配比、槽位调节、会话恢复和 ID 分配。

RustCodeGraph 的文件视图报告目标文件被 `pkg/ddl/backfilling_read_index.rs` 使用；由于跨 crate 的 `pkg/session` 调用未出现在该文件级摘要中，又通过全仓符号搜索及对应源码位置进行了补充核验。全仓 Rust 引用搜索未发现 `TxnBackfillExecutor`、`ReorgMeta`/简化 `SessionContext` 或两个 context 构造器的非测试调用，因此它们目前不能被描述为已接入完整 DDL 主链。

## 错误处理与边界

`setup_workers(0)` 返回 `ExecutorError::InvalidConcurrency`，而大于 16 的值静默截断到 16。动态 `adjust_worker_size` 不返回错误，零表示停掉所有逻辑 worker。任务或结果在关闭后入队返回 `ExecutorError::Closed`，不会改变队列；读取空队列返回 `None`，不区分“暂时为空”和“已关闭且耗尽”。

`close(false)` 保留队列是排空语义，`close(true)` 是丢弃语义；两者都不生成取消结果。因为没有真实 worker，本文件也没有 join 失败、事务提交失败、context cancellation 或 backpressure 错误。调用方若把该内存模型用于并发执行，必须另行定义同步、唤醒和失败传播，不能依赖这里提供。

worker 配比边界与 Go 一致地保留 global sort 的零并发 `(0, 0)`；调用方通常在进入函数前用 `max(1)` 规范 CPU，但辅助函数本身不替调用方修正。非 global sort 且无统计时，零并发会因 clamp 产生 reader 1、writer 2；有统计时会产生至少 `(1, 1)`。有统计的大行会令 reader 达到 `concurrency * 8`，不受 16 上限约束，调用方必须评估资源规模和 `usize`/到 `i32` 转换风险。

context 构造函数没有返回 `Result`，因为当前仅执行 clone/default/字段覆盖；与 Go 版本不同，它不解析时区，因此也不会在这里暴露非法时区错误。`SessionContext::initialize_for_reorganization` 同样不验证时区名称、资源组存在性或批量上限。

## 并发与资源生命周期

`TxnBackfillExecutor` 不实现 `Send`/`Sync` 层面的共享协议，也没有内部锁；所有变更方法都要求 `&mut self`，因此安全使用方式是由单一所有者串行驱动。`VecDeque` 提供 FIFO 容器语义，但不是 Go channel：没有容量、阻塞、select、关闭通知或生产者/消费者并发。

逻辑 worker 的生命周期由 `setup_workers/adjust_worker_size/close` 改变 `workers` 向量表示。缩容只是 drop `WorkerSlot`，不会等待正在执行的工作，因为槽位中根本没有线程或任务句柄。非强制关闭保留队列，强制关闭立即 drop 队列元素；两种关闭都会 drop 全部槽位。

实际 ingest 并发发生在调用者中：`modify_column_pipeline::Pipeline` 用 `WorkerPool`、有界 channel、原子停止标记和 mutex 管理读写阶段；`backfilling_read_index` 保存 pipeline running/closed 状态。`expected_ingest_worker_count` 只是纯计算规则，不拥有这些资源。

`WarnAppenderRef` 在 DistSQL context 中通过引用计数 clone 共享；context 的 `'static` 生命周期来自其拥有/共享的字段，而非借用调用栈数据。`SessionSnapshot` 和队列元素则通过所有权移动自然释放。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ddl/backfilling_txn_executor.go`，Rust 类型/函数沿用其命名与部分规则，但移植覆盖度不同：

- `maxBackfillWorkerSize = 16` 对应 `MAX_BACKFILL_WORKER_SIZE`；`expectedIngestWorkerCnt` 的 global sort、未知行宽默认值以及五档行宽比例被逐项保留。Rust 独立测试复用了 Go 测试的十组表格用例，并额外固定 global sort 零并发及“大行 reader 不截断”的语义。
- Go `taskIDAllocator` 与 Rust `TaskIdAllocator` 都从 0 开始先取值后自增。Rust 版本已被 `backfilling_operators.rs` 的任务切分生产路径使用。
- Go `newDefaultReorgDistSQLCtx` 设置 KV client、限速动作、killer、内存/CPU/runtime stats、UTC、TiFlash 默认值等完整请求上下文；Rust 默认构造器只显式设置 warning、chunk RPC、`NotFillCache` 和 `ErrCtx`。Go `newReorgDistSQLCtxWithReorgMeta` 还解析 location、按 SQL mode 设置错误级别且可失败；Rust 只复制资源组名。这是当前功能差异，不应描述为等价移植。
- Go `initSessCtx/restoreSessCtx` 操作真实 `sessionctx.Context`，保存并恢复 row encoder、SQL mode、除法精度、时区、type/error flags 和资源组；Rust `SessionContext` 是四字段模型，额外保存 batch size，但未连接真实 session。这同样是简化边界。
- Go `txnBackfillExecutor` 持有 context、reorg info、session pool、table、decode map、worker、wait group、有缓冲 task/result channel；按五种 backfiller 类型构造真实 worker 并启动 goroutine，关闭时关闭 channel、可强制停止并等待 worker。Rust 同名类型没有这些依赖，只建槽位和无界内存队列，因此不能执行 Go 的事务回填行为。
- Go `sendTask` 可被 context cancellation 中断；Rust `send_task` 只检查本地 `closed`。Go `resultChan` 暴露异步接收端；Rust `result` 是同步 pop。

Go 测试证据包括 `pkg/ddl/backfilling_txn_executor_test.go::TestExpectedIngestWorkerCnt` 的配比表，以及 `pkg/ddl/backfilling_test.go::TestReorgDistSQLCtxNotFillCache` 对两类 context 的 `NotFillCache` 约束。Go 的 session-context 测试还验证 SQL mode/location 等完整上下文行为，但这些字段并未全部进入当前 Rust 模型。

## 扩展指南

若只是调整 ingest 并发策略，应修改 `expected_ingest_worker_count`，同步更新独立的 `pkg/ddl/backfilling_txn_executor_test.rs::test_expected_ingest_worker_count`，并检查两个生产消费者：read-index 的启动/资源变化，以及 modify-column pipeline 的启动/tune。特别关注 global sort 零值、未知统计、阈值边界 200/500/1000/3000、大并发乘法和到 worker pool 参数类型的转换；配比增加会直接影响 CPU、内存和 goroutine/thread 数。

若补全 DistSQL context，应以 Go 的 `newDefaultReorgDistSQLCtx/newReorgDistSQLCtxWithReorgMeta` 为逐字段依据，将时区解析和 SQL-mode error levels 的失败显式纳入返回类型，并扩展 `reorg_dist_sql_contexts_do_not_fill_tikv_block_cache` 或新增同目录独立测试。无论增加哪些字段，`NotFillCache = true` 是前台负载隔离不变量。

若把 `TxnBackfillExecutor` 接入真实事务回填，不能仅在现有槽位上加“已运行”标志：需要明确 session pool、实际 `Backfiller` 构造、任务/结果 channel 容量、context cancellation、worker join、缩容安全点、错误结果和 checkpoint 的责任边界，并逐项对照 Go 五种 worker 分支。测试必须继续放在 `backfilling_txn_executor_test.rs` 或其他独立 `*_test.rs`，不得内嵌到生产源文件。接线还需追踪 DDL job 取消、owner failover 和持久化 reorg checkpoint；这些不是本文件现有能力。

若扩展任务编号，先确认 ID 是否仅作单次流水线内排序。需要跨重启或多生产者分配时，不应给当前 `usize` allocator 简单加锁后冒充持久化 ID，而应由 job/subtask metadata 提供稳定标识，并同步 `TableScanTaskSource::generate_tasks` 的契约与测试。

兼容性风险集中在三处：改变 worker 公式会改变资源占用和吞吐；改变非强制关闭的队列保留语义会影响排空；改变 ID 起点/连续性会影响结果关联。性能改动应保留 Go 规则，除非有明确的跨语言差异决策和基准证据。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust 文件在索引中完整覆盖 322 行。
- RustCodeGraph `explore "backfilling_txn_executor.rs transaction backfill executor"`：识别本文件核心符号、Go 对照符号以及 `expected_ingest_worker_count` 的 read-index 调用关系。
- RustCodeGraph `query`：精确定位 `TxnBackfillExecutor`、`expected_ingest_worker_count`、`new_default_reorg_dist_sql_context`、`TaskIdAllocator`，并区分 Rust 与 Go 同名实现。
- RustCodeGraph `node --file`：读取 `pkg/ddl/backfilling_txn_executor.rs` 全部 322 行、`pkg/ddl/backfilling_txn_executor_test.rs` 全部 130 行，以及 `backfilling_read_index.rs`、`backfilling_operators.rs`、`modify_column_pipeline.rs` 的直接调用片段。精确 `callers` 子命令曾连续无输出并被中止，因此调用边又以成功的 explore 结果和全仓符号搜索交叉核验。
- crate/模块证据：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`。
- Rust 数据类型与生产调用证据：`pkg/ddl/backfilling.rs`、`pkg/ddl/backfilling_read_index.rs`、`pkg/ddl/backfilling_operators.rs`、`pkg/session/runtime/modify_column_pipeline.rs`。
- Go 对照与测试证据：`pkg/ddl/backfilling_txn_executor.go`、`pkg/ddl/backfilling_txn_executor_test.go`、`pkg/ddl/backfilling_test.go`。
- Rust 独立测试证据：`pkg/ddl/backfilling_txn_executor_test.rs` 覆盖 cache 旁路与资源组、worker 配比、并发边界、会话恢复、batch size 下限及任务 ID；当前没有覆盖任务/结果 FIFO、关闭后入队、强制/非强制清理的测试。
- 本任务是纯文档分析，依计划不运行 Cargo；交付验证只执行文档结构检查、引用/事实复核和 diff 自审。

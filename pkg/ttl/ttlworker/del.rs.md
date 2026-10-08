# `pkg/ttl/ttlworker/del.rs`

## 文件定位

`del.rs` 是 `astersql-ttl-ttlworker` crate 的 TTL 删除侧核心实现，由 [`lib.rs`](lib.rs) 以 `pub mod del` 对外暴露。它位于“扫描出过期行的键”与“把删除结果记入 TTL job 统计”之间：上游 [`scan.rs`](scan.rs) 定义扫描任务和 `TtlStatistics`，本文件负责组装并执行批量 `DELETE`，同时保存可重试行。

当前完整应用接线在 [`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 的 `run_ttl_tick_inner`：它在扫描回调中构造 `DeleteTask`，调用 `do_delete`，将剩余行记入 `DeleteRetryBuffer`，并在扫描后轮询重试。RustCodeGraph 还确认本文件被 [`del_test.rs`](del_test.rs)、[`scan.rs`](scan.rs) 和 [`scan_test.rs`](scan_test.rs) 使用。

[`Cargo.toml`](Cargo.toml) 指定 crate 名为 `astersql-ttl-ttlworker`、库入口为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/ttl/ttlworker"` 记录 Go 对照包。`del.rs` 本身只使用标准库以及本 crate 的 `scan`/`session` 类型，没有直接调用 Cargo 中声明的外部 crate。

## 核心职责

1. `DeleteTask::delete_sql` 根据物理表、分区、键列数和批大小生成参数化 `DELETE LOW_PRIORITY ... WHERE ... IN (...) AND ttl_column < ... LIMIT ...`。
2. `DeleteTask::do_delete` 先获取过期谓词，再以 `DELETE_BATCH_SIZE` 为上限串行处理每批；限流或可重试执行失败的行被返回，不可重试失败直接记入错误统计。
3. `DeleteRetryBuffer` 以有界 FIFO 保存失败行，控制轮询间隔和最大重试次数，并将被淘汰、达到上限或停机清理的行统一记为最终失败。
4. `DeleteRateLimiter` 把具体限流策略留给运行时，本文件只保证在每批 SQL 前按行数申请令牌。

## 主要符号

- `DELETE_MAX_RETRY = 3`、`DELETE_RETRY_BUFFER_SIZE = 128`、`DELETE_RETRY_INTERVAL = 5s`：`DeleteRetryBuffer::default` 的上限、容量和间隔，对应 Go 的 `delMaxRetry`、`delRetryBufferSize`、`delRetryInterval`。
- `DELETE_BATCH_SIZE = 100`：`do_delete` 的固定分批上限。这是 Rust 当前常量，不是 Go 版运行时 `tidb_ttl_delete_batch_size` 的动态读取。
- `DeleteRateLimiter::wait_delete_token(&mut self, rows)`：每批删除前的可替换限流边界；错误被视为该批待重试，不终止后续批次。
- `DeleteTask`：一次删除输入，持有 `job_id`、`PhysicalTable`、键值行 `rows`、`expire_time` 和共享的 `Arc<TtlStatistics>`。结构及其字段均为公开 API。
- `DeleteTask::delete_sql(row_count)`：生成 SQL 模板；单键用 ``key IN (%?, ...)``，复合键用 ``(k1,k2) IN ((%?,%?), ...)``，有分区时加入 `PARTITION`。
- `DeleteTask::do_delete(session, limiter) -> Vec<Row>`：同步执行入口，返回仅包含需继续重试的行。
- `RetryItem`：私有队列元素，保存裁剪后的任务、失败行、已重试次数和入队时刻。
- `DeleteRetryBuffer`：公开重试管理器。`with_options` 支持测试注入容量、上限、间隔和时钟；`retry_interval`、`len`、`discarded_rows` 暴露状态；`record_task_result`、`retry_all`、`drain` 控制生命周期；`record` 为私有入队原语。
- `fmt::Debug for DeleteRetryBuffer`：输出队列与策略字段，但用 `finish_non_exhaustive` 不输出可调用时钟闭包。

## 执行流程

`DeleteTask::do_delete` 的顺序如下：

1. 调用 `WorkerSession::expiration_predicate(table, expire_time)` 获得当前会话可用的过期表达式及其参数。这一步失败时不执行 SQL，直接返回原始全部 `rows`。
2. 按 `DELETE_BATCH_SIZE` 将 `rows` 切成最多 100 行的连续批次。空任务不进入循环，返回空重试集。
3. 每批先调用 `wait_delete_token(batch.len())`。失败时把当前批复制到 `retry_rows`，然后 `continue` 处理后续批；因此单批限流失败不是整个任务的熔断信号。
4. 为当前批 clone 一份 `DeleteTask`，用 `delete_sql(batch.len())` 生成 SQL，将其中第一个 `FROM_UNIXTIME(%?)` 替换为会话返回的 `ExpirationPredicate.expression`。
5. 参数严格按 SQL 占位符顺序展平：先是每行的所有键列，最后是过期参数 `ExpirationPredicate.argument`。
6. 通过 `execute_with_ttl_job(job_id, sql, args)` 执行，使用户表语句在执行期带有 TTL job 归属，并由会话实现恢复先前状态。成功调用 `statistics.add_success`；`SessionError::NonRetryable` 调用 `add_error`；其他错误把该批放入返回的重试集。

`DeleteRetryBuffer` 的流程是：

1. `record_task_result` 忽略空的剩余行；非空结果以 `retry_count = 0` 进入 `record`。
2. `record` 先检查 `retry_count >= max_retry`；达到上限时增加 `discarded_rows` 和错误行统计，不再入队。否则在队列已满时从队首逐个淘汰旧项，对淘汰行做同样的最终失败记账，再将当前失败行写入 clone 后的 task 并压入队尾。
3. `retry_all` 在调用开始时快照队列长度，因而每个旧项在一次调用中最多执行一次，即使间隔为零且失败项立即重新入队。队首尚未到期时，立即返回它的剩余等待时间；已到期项出队并交给回调，回调仍返回行时以 `retry_count + 1` 再入队。
4. `drain` 不再执行删除；它清空队列，将每项的剩余行累加到 `statistics.error_rows` 和 `discarded_rows`。

## 数据与状态

`DeleteTask.rows` 的每个 `Row` 是按 `PhysicalTable.key_columns` 顺序排列的 `Vec<Datum>`。本文件不再校验行宽与键列数是否一致；它直接将所有 Datum 展平为 SQL 参数，因此上游构造者必须维持该不变量。`delete_sql(row_count)` 也信任 `row_count > 0` 且 `key_columns` 非空；生产路径中它只对 `do_delete` 切出的非空批次调用。

`statistics: Arc<TtlStatistics>` 与原始任务和所有 clone/重试任务共享。[`scan.rs`](scan.rs) 中的 `TtlStatistics` 使用 `AtomicU64` 以 `Relaxed` 顺序分别累加总行、成功行和错误行。本文件不增加总行数：总数由扫描侧在交付删除前记录。可重试错误在尚有再试机会时不增加错误行，只有不可重试、淘汰、超限或 `drain` 才做最终错误记账。

`DeleteRetryBuffer.items` 为 `VecDeque<RetryItem>`，队首是最早入队项。`in_time` 是注入时钟返回的单调时长；`saturating_sub` 使时钟回退时的 elapsed 饱和为零。`discarded_rows` 是缓冲区本地累计值，包含淘汰、超限和 drain 的行，不包含仍在队列中或已重试成功的行。

## 依赖与调用关系

上游主链可概括为：

`run_ttl_tick_inner` → `TtlScanTask::execute_with_checkpoint` 的行回调 → `DeleteTask::do_delete` → `DeleteRetryBuffer::record_task_result` → 扫描后的 `DeleteRetryBuffer::retry_all` / 取消时 `drain`。

该运行时接线还把心跳丢失、外部取消和 TTL 调度开关组合为 `cancel_delete`，交给 `ConfiguredDeleteRateLimiter`；限流器读取 `TTLDeleteRateLimit`，按 `rows / limit` 计算等待时间，并每 100ms 检查取消。扫描回调只要产生剩余行，便返回“必须先重试删除”错误，防止越过未完成批次提交扫描 checkpoint。

下游依赖是 [`session.rs`](session.rs) 的 `WorkerSession`、`PhysicalTable`、`Datum`/`Row` 和 `SessionError`，以及 [`scan.rs`](scan.rs) 的 `TtlStatistics`。`WorkerSession::expiration_predicate` 隔离全局时区下的过期表达式；`execute_with_ttl_job` 隔离带 job 归属的 SQL 执行与会话状态恢复。`DeleteTask` 忽略成功执行返回的行集，只使用 `Result` 的错误分类。

## 错误处理与边界

- 过期谓词构造失败：整个任务的行都返回重试，当次不更新成功/错误计数。
- 限流器错误：当前批返回重试，后续批仍会尝试。[`del_test.rs`](del_test.rs) 的 `delete_task_continues_after_retryable_and_limiter_errors` 验证了该行为。
- SQL 错误：只有 `SessionError::NonRetryable` 立即计入错误行；`Execute`、`TableChanged`、`TtlDisabled`、`ExpireIntervalChanged` 等其他变体在本层都进入重试行集。是否继续运行由上层取消/心跳条件和重试缓冲决定。
- SQL 标识符：分区名会把反引号翻倍；`schema`、`table`、`key_columns` 和 `ttl_column` 直接放入反引号中，本文件不做额外转义。它依赖上游物理表元数据的合法标识符保证。
- 参数和 SQL 形状：本文件没有显式校验键列非空、每行 Datum 数等于键列数，也没有防止外部直接以 `row_count = 0` 调用 `delete_sql`。扩展上游构造路径时必须保持这些先决条件。
- 缓冲边界：记录空结果不入队；超限判定为 `retry_count >= max_retry`。`retry_all` 遇到第一个未到期队首时不继续查看后续项，这是 FIFO 时间顺序的一部分。`max_size = 0` 时，空队列仍能接收第一项，因为淘汰循环还要求队列非空；生产默认值为 128。

## 并发与资源生命周期

`DeleteTask::do_delete` 是同步且串行的：它在调用者线程上逐批等待令牌和执行 SQL，本文件不创建线程、任务、通道、锁或事务。`session` 和 `limiter` 通过 `&mut dyn ...` 传入，使同一次调用中的状态变更按顺序发生。

`DeleteRetryBuffer` 也要求 `&mut self`，没有内部同步；它应由单一所有者调度。队列持有 `DeleteTask` 和行的所有权，出队成功后即释放；失败时用新的入队时刻重新入队。注入的 `get_time` 闭包被 `Arc` 持有并要求 `Send + Sync`，但这不使缓冲区自身成为并发队列。

`Arc<TtlStatistics>` 是原始任务与重试 clone 之间的共享资源，其原子计数支持多线程累加。运行时在正常重试结束、外部取消或心跳丢失时决定是继续 `retry_all` 还是 `drain`；`drain` 是队列内剩余行的终止点，保证它们不在统计中悬空。

## 与 Go 版本的对应关系

Go 对照文件是 [`del.go`](del.go)，相关测试是 [`del_test.go`](del_test.go)。主要映射为：

- `ttlDeleteTask` ↔ `DeleteTask`，`doDelete` ↔ `do_delete`；`ttlDelRetryItem` ↔ `RetryItem`，`ttlDelRetryBuffer` ↔ `DeleteRetryBuffer`。
- `newTTLDelRetryBuffer` ↔ `Default::default`，`RecordTaskResult` ↔ `record_task_result`，`recordRetryItem` ↔ `record`，`DoRetry` ↔ `retry_all`，`Drain` ↔ `drain`。
- 默认的 3 次重试、128 项缓冲和 5 秒间隔一致；两版都使用有界 FIFO，都在单次重试调用开始时固定循环次数，保证零间隔下同一项不会在一次调用中无限重试。
- 两版都在缓冲满时淘汰最旧项，在超过重试上限或 drain 时把剩余行记为错误，并通过复制任务保持原任务的 `rows` 不变。
- Go 的 SQL 由 `sqlbuilder.BuildDeleteSQL` 将 Datum 写入 SQL 文本；Rust 在本文件构造 `%?` 占位符和 `Vec<Datum>`，再由 `WorkerSession` 执行。Rust 另显式支持分区子句和复合键占位符形状。
- Go 每批读取动态的 `TTLDeleteBatchSize`；Rust 当前使用固定 `DELETE_BATCH_SIZE = 100`。
- Go `delRateLimiter::WaitDelToken(ctx)` 每批等待一个令牌，并从 context 感知取消；Rust trait 接受批行数，实际运行时 `ConfiguredDeleteRateLimiter` 按行数计算等待，以轮询闭包感知取消。
- Go `doDelete` 在 context 取消后停止新批次，并通过 defer 把尚未处理的行返回；Rust `do_delete` 本身没有 context，它在限流失败后继续扫描后续批，取消策略由 limiter 和上层循环承担。
- Go 包含 phase tracer、耗时 metric 和带 job/scan/table 字段的日志；Rust `del.rs` 未实现这些可观测性。Rust 额外暴露 `discarded_rows`，便于直接检查缓冲区最终放弃行数。
- Go 在构建删除 SQL 失败时把当前批记为错误并返回尚未处理的行；Rust 的 SQL 模板构造不返回 `Result`，而会话过期谓词构造失败时返回全部行重试。

因此本文件保留了 Go 的核心批删除、可重试/不可重试分流、FIFO 时序和统计记账语义，但并非完全机械等价；上述批量配置、取消位置、SQL 构造和可观测性差异是后续对齐时的重点。

## 扩展指南

- 改变 SQL 形状、键列处理或分区语法时，修改 `DeleteTask::delete_sql`，并在独立的 [`del_test.rs`](del_test.rs) 扩展单键、复合键、分区名转义、参数顺序和过期表达式用例；不要把 Rust 测试内嵌到 `del.rs`。
- 改变分批、错误分类、取消或统计语义时，主要接入点是 `DeleteTask::do_delete`。同时检查 [`session.rs`](session.rs) 的 `SessionError`/`WorkerSession` 契约、[`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 的 limiter 与重试循环，以及 [`pkg/session/runtime/ttl_worker_session_test.rs`](../../session/runtime/ttl_worker_session_test.rs) 的真实会话边界测试。
- 改变缓冲容量、重试次数、轮询顺序或停机记账时，修改 `DeleteRetryBuffer::{default, record, retry_all, drain}`，并扩展 `retry_buffer_matches_go_fifo_timing_retry_and_accounting` 与 `retry_defaults_and_zero_interval_match_go_contract`。必须保留“单次调用每个旧项最多重试一次”和“最终放弃恰好记账一次”的不变量。
- 若要与 Go 的动态 batch size、phase metric 或日志完全对齐，不应只在本文件加常量或打印；还需同步运行时配置来源、会话上下文和指标接线，并以 [`del.go`](del.go) 及 [`del_test.go`](del_test.go) 的当前行为为对照。
- 性能上，该路径会 clone 失败批的 Datum，入队时又 clone 一次 `rows`；放大 batch/缓冲容量前应评估内存峰值和失败时的重试放大。改变错误分类或 checkpoint 顺序时，还要评估重复删除、遗留过期行和统计偏差的兼容风险。

## 验证依据

- RustCodeGraph `status` 显示当前索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ttl/ttlworker/del.rs` 命中目标文件，`node --file ...` 列出完整 276 行与 28 个符号。
- RustCodeGraph 文件节点报告 `del.rs` 被 `del_test.rs`、`scan.rs`、`scan_test.rs` 使用；`query` 定位了 `DeleteTask`、`DeleteRetryBuffer`、`DeleteRateLimiter`、`delete_sql`、`do_delete`、`record_task_result`、`retry_all` 和 `drain`。精确 `callers` 查询在 60 秒内未返回，因此对运行时调用边另用局部 `rg` 与源码读取核对，未将超时当作不存在调用者的证据。
- 已读 Rust 源与边界：[`del.rs`](del.rs)、[`lib.rs`](lib.rs)、[`scan.rs`](scan.rs)、[`session.rs`](session.rs)、[`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs)。它们分别证明删除/重试实现、模块导出、原子统计、会话错误契约和完整应用接线。
- 已读 crate 边界 [`Cargo.toml`](Cargo.toml)，确认 crate 名、库入口、Go 移植元数据和依赖范围。
- 已读 Rust 独立测试 [`del_test.rs`](del_test.rs)：它覆盖物理分区 SQL、job ID 归属、可重试与限流错误后继续、参数顺序、FIFO 淘汰/时序/上限/drain 记账以及零间隔单次重试约束。运行时会话交界另由 [`pkg/session/runtime/ttl_worker_session_test.rs`](../../session/runtime/ttl_worker_session_test.rs) 调用 `do_delete` 验证。
- 已读 Go 对照 [`del.go`](del.go) 和 [`del_test.go`](del_test.go)，核对删除 worker 调度、批处理、可重试分流、FIFO 缓冲、停机最后尝试与 drain 语义，并明确列出 Rust 现有差异。
- 本任务为纯文档分析，按总计划与任务约束不运行 Cargo。交付时使用任务指定的结构命令确认本文件存在且恰好包含全部 11 个固定二级标题，并人工复核不将预期设计写成当前事实。

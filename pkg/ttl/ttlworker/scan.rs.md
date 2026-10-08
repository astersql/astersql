# `pkg/ttl/ttlworker/scan.rs`

## 文件定位

`scan.rs` 位于 `astersql-ttl-ttlworker` crate 的扫描/删除流水线前半段，由 [`lib.rs`](lib.rs) 以 `pub mod scan` 暴露。它把一个持久化 TTL 扫描分片描述成 `TtlScanTask`，按键范围分页查询已经过期的行，再把表主键行交给删除侧；本文件本身不执行 `DELETE`。上游的 `TaskManager::reschedule`（[`task_manager.rs`](task_manager.rs)）从 waiting 队列产出 `TtlScanTask`，下游删除实现（[`del.rs`](del.rs)）与扫描侧共享 `TtlStatistics`。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-ttl-ttlworker`，Go 对照包元数据是 `pkg/ttl/ttlworker`。当前普通依赖仅声明 `astersql-ttl-cache`；大批完整移植依赖被放在 Windows 条件目标下。因此本文只描述当前 `scan.rs` 的可见 Rust 行为，不把 Go 版会话池、指标、后台 goroutine 等尚未在此文件接线的能力表述为已具备。

## 核心职责

1. `TtlScanTask::scan_sql` 根据物理表、分区、TTL 水位、范围边界、可选 TTL 索引和续扫游标，生成参数化 `SELECT`。
2. `TtlScanTask::execute_with_checkpoint` 循环执行扫描 SQL，处理取消、错误率熔断、有限重试、删除回调、统计更新、持久游标回调和结束条件；`execute` 是不启用恢复游标与持久检查点的便捷入口。
3. `ScanIndex` 保存用于 `FORCE_INDEX`、投影和排序的索引布局；索引扫描仍补投影表主键，因为删除侧按表键删除。
4. `TtlStatistics` 用原子计数把扫描总行数与删除成功/错误行数连接起来，并支持接管任务时恢复计数。
5. `ScanWorker` 保存“当前任务/待领取结果”两个槽位，实现单任务调度状态机；它不创建线程，也不主动调用 `TtlScanTask::execute`。

## 主要符号

- `ScanIndex { id, name, columns, unique }`：持久化索引扫描布局。`unique == false` 时排序键会追加缺失的表主键列，以构成稳定、可继续的全序；`id` 在本文件内不参与 SQL 拼接，供外部元数据/持久化识别。
- `SCAN_TASK_EXECUTE_SQL_MAX_RETRY = 5`：允许初次执行之外再重试五次，即一页最多六次尝试（`execute_with_checkpoint` 的闭区间循环 `0..=5`）。
- `TtlStatistics`：私有的三个 `AtomicU64` 分别记录 total/success/error。`add_*` 累加，`snapshot` 取非事务快照，`reset` 清零，`restore` 恢复持久值，`error_rate_too_high` 判断熔断。
- `TaskTerminateReason`：结果原因包括 `Finished`、`Canceled`、`ErrorRateExceeded`、`TableChanged`、`Error`、`WorkerStop`。当前执行函数直接产生前三者和 `Error`；`TaskManager::report_finished` 对 `WorkerStop` 特殊处理为重新排队。`TableChanged` 变体在本文件执行路径中没有直接构造。
- `TtlScanTask`：不可变任务输入，包含作业/扫描 ID、`PhysicalTable` 快照、Unix TTL 水位、半开扫描范围、批大小和可选索引。
- `ScanResult`：终止摘要，携带身份、原因、可选 `SessionError` 与本次执行累计的 `scanned_rows`。
- `TtlScanTask::{scan_sql, execute, execute_with_checkpoint}`：SQL 生成与执行的公开入口。
- `TtlScanTask::{scan_columns, order_key, result}`：分别计算投影列、从返回行提取续扫排序键、统一构造结果的内部辅助函数。
- `ScanWorker::{could_schedule, schedule, finish, poll_result, current_task}`：单槽任务与结果的同步状态操作；调用方必须显式驱动执行和 `finish`。

## 执行流程

`scan_sql` 首先选择索引列或表主键作为扫描列，并补齐删除所需的所有表主键。排序列默认相同；非唯一索引额外追加主键以消除同索引值行之间的歧义。随后构造 `SELECT ... FROM schema.table`，有分区时加入 `PARTITION`，有索引时加入转义后的 `FORCE_INDEX`，并加入 TTL 谓词。`range_start` 使用 `>=`，`range_end` 使用 `<`；无索引时边界作用于完整排序键，有索引时当前实现把边界列写成 TTL 列。最后加入游标条件、`ORDER BY` 与至少为 1 的 `LIMIT`。

无索引续扫用行值比较 `(keys) > (...)`。索引续扫则按排序键逐级生成字典序 OR 条件，并为 `NULL` 单独使用 `IS NULL`/`IS NOT NULL`；参数按生成条件的顺序追加。索引名与分区名中的反引号会加倍转义，但 schema、表名及列名直接置于反引号中，依赖可信元数据名称。

`execute_with_checkpoint` 的主流程如下：

1. 通过 `WorkerSession::expiration_predicate` 捕获一次过期表达式和参数；失败直接返回 `Error`，且扫描数为 0。
2. 每页开始前检查 `canceled`，再检查共享统计的错误率。代码使用 `total > 10_000 && errors / total > 0.4`，即样本必须严格超过 10,000。
3. 用当前游标生成 SQL，以会话提供的表达式替换首个 `FROM_UNIXTIME(%?)`，并替换第一个过期参数。
4. 通过 `execute_with_ttl_job(job_id, ...)` 执行。一条语句返回后、处理结果或决定重试前再次检查取消，以保证语句边界取消不会把已经返回的行派发给删除侧。
5. `NonRetryable`、`TableChanged`、`TtlDisabled`、`ExpireIntervalChanged` 立即以 `Error` 返回；其他错误保存为 `last_error` 并重试，六次均失败后返回最后错误。Rust 当前没有 Go 版两秒重试等待。
6. 空结果表示完成。非空结果在索引扫描时转换为纯表主键行；无索引时直接克隆返回行。删除回调失败时不累计行数，也不推进游标。
7. 删除回调成功后先增加 total/scanned，再把本页最后一条完整扫描行交给 `checkpoint`。检查点失败会返回 `Error`，但该页已经派发且已计数；持久存储应继续保留上一次成功游标。
8. 检查点成功后以最后一行的排序键推进内存游标。返回数少于有效批大小时完成；等于批大小时继续查询下一页，可能再用一个空页确认结束。

## 数据与状态

扫描范围是 `[range_start, range_end)`，游标条件是严格大于上一页末项，从而避免重复读取。`batch_size == 0` 会被 `max(1)` 钳制为 1，SQL 和结束判断使用同一有效批大小。

返回行的列布局由 `scan_columns` 决定。索引扫描的 `order_key` 假定每个排序列都存在于投影中，并以 `unwrap` 取位置；表主键提取使用 `expect("table key is projected")`。该不变量由 SQL 投影构造保证，但若外部 `WorkerSession` 返回的行宽或次序不符合查询，索引访问仍可能 panic。`scan_sql` 也假定 `cursor.len()` 不超过排序列数；恢复持久游标的调用方必须保留同一索引布局。

`TtlStatistics` 使用 `Ordering::Relaxed`。单个计数的读写是原子的，但 `snapshot` 的三个值不是一致性事务快照，适合监控和近似熔断，不应用来证明 `total == success + error` 的瞬时强不变量。扫描只更新 total；删除侧的 `DeleteTask` 持有 `Arc<TtlStatistics>` 并更新 success/error。`restore` 是逐字段覆盖，调用方应在任务尚未并发更新时使用。

`ScanWorker` 的状态为 `(current, result)` 两个 `Option`。只有二者都为空才可调度；`finish` 清除 current 并保存结果，结果在 `poll_result` 取走前阻止下一次调度。类型本身没有锁，需由单线程所有权或外部同步保证安全。

## 依赖与调用关系

- 上游任务来源：`TaskManager::reschedule`（[`task_manager.rs`](task_manager.rs)）从 waiting 提升任务并返回 `Vec<TtlScanTask>`；`initial_managed_task` 等作业管理接线构造持久任务。
- 会话边界：`scan.rs` 依赖 [`session.rs`](session.rs) 的 `Datum`、`Row`、`PhysicalTable`、`SessionError` 与 `WorkerSession`。`execute_with_ttl_job` 临时设置 `SessionState::ttl_job_id`，执行后恢复；`expiration_predicate` 允许真实会话提供时区正确的过期表达式。
- 删除边界：`emit_delete(Vec<Row>)` 是扫描到删除的抽象通道。本文件只保证成功回调后计数；[`del.rs`](del.rs) 的 `DeleteTask` 持有共享统计并负责限流、拆批、DELETE 与重试行。
- 结果消费：`TaskManager::report_finished` 按 `(job_id, scan_id)` 找到运行任务；`WorkerStop` 回 waiting，其他原因均进入 finished，错误细节由 `ScanResult.error` 承载。
- 图证据：RustCodeGraph 的文件节点报告 `scan.rs` 被 `del.rs`、`del_test.rs`、`scan_test.rs` 使用；其方法级 `callers/callees` 查询未返回边，因此上述跨文件调用点由精确源码搜索核验，而不是据此推断不存在调用。

## 错误处理与边界

过期谓词创建失败、重试耗尽、不可重试会话错误、删除回调失败和检查点失败都统一映射到 `TaskTerminateReason::Error`，原始 `SessionError` 存入结果。`TableChanged`、`TtlDisabled`、`ExpireIntervalChanged` 虽然不重试，终止原因仍是 `Error` 而不是同名的 `TaskTerminateReason::TableChanged`；消费方要检查 `error` 才能区分。

取消不附带错误，发生在页前或语句返回边界都会得到 `Canceled`。错误率熔断同样不附带错误，且只读取共享统计；删除错误增长可能令尚在扫描的任务停止。空页或不足一批得到 `Finished`。

SQL 标识符处理并不完全一致：索引名和分区名显式转义反引号，schema、表、TTL 列和键列未做同样替换。这些字段预期来自已验证的数据库元数据，不应接受任意用户拼接字符串。范围 Datum 数量与列数量、游标布局和结果行布局也由调用契约约束，本文件没有返回可恢复错误来处理不匹配。

## 并发与资源生命周期

扫描执行是同步函数：会话以 `&mut dyn WorkerSession` 独占借用，删除和检查点通过 `FnMut` 回调串行调用。它不创建线程、异步任务、锁、通道或事务；资源所有权和阻塞策略由调用方实现。取消是轮询式 `Fn() -> bool`，不能主动中断正在阻塞的 `session.execute_with_ttl_job`；真实会话若要及时终止语句，需要在接口实现或外围 worker 中提供 kill/cancel 机制。

共享统计可跨线程使用原子更新；Relaxed 顺序只保证计数原子性，不建立其他数据的 happens-before 关系。`ScanWorker` 虽然可 `Clone`，克隆得到独立的任务/结果快照，而不是共享 worker；与 Go 的加锁后台 worker 不同，当前 Rust 类型只是状态容器。

检查点的资源时序是“删除派发成功 → 计数 → 持久检查点 → 推进内存游标”。因此检查点实现必须持久化完整排序行，并确保成功返回代表耐久；失败后任务恢复会从旧游标重扫已经派发的一页，删除链路必须容忍这种至少一次语义。

## 与 Go 版本的对应关系

Go 对照为 [`scan.go`](scan.go)，主要对应关系是：`ttlStatistics` ↔ `TtlStatistics`，`ttlScanTask` ↔ `TtlScanTask`，`doScanWithSession` ↔ `execute_with_checkpoint`，`ttlScanWorker` 的调度约束 ↔ `ScanWorker`。两端都在超过 10,000 行且错误率超过 40% 时停止，都只在删除任务成功派发后增加扫描总数，并都允许初次 SQL 加五次重试。

Rust 将 Go `sqlbuilder.ScanQueryGenerator` 的部分行为直接放进 `scan_sql`，以参数数组而不是完整 SQL 字面量执行；它还增加了显式 `ScanIndex`、耐久游标恢复与 `checkpoint` 回调。Rust 测试 `go_merge_43_scan_restarts_after_durable_cursor_and_stops_on_checkpoint_error` 表明该检查点行为属于后续对齐扩展，而不是当前 Go `scan.go` 中同名 API。

当前 Rust 不是 Go worker 的完整等价实现。Go `doScanWithSession` 还负责会话池获取/恢复、安全过期时间复核、指标 phase tracer、两秒重试间隔、任务/worker 双 context、`KillStmt` 重发以及后台 goroutine；Go `ttlScanWorker` 带锁、消息通道和生命周期循环。Rust 的这些职责仅由 `WorkerSession`/回调抽象部分承接，`ScanWorker` 不执行后台循环。Go 的终止原因只有 finished/error/workerStop，而 Rust 将取消和错误率等原因细分，并保留尚未由本执行函数构造的 `TableChanged`。

测试对应方面，Rust [`scan_test.rs`](scan_test.rs) 覆盖单槽调度、SQL/范围/游标、分区、索引投影、重试、派发计数、检查点失败和熔断；[`scan_integration_test.rs`](scan_integration_test.rs) 对照 Go [`scan_integration_test.go`](scan_integration_test.go) 的扫描中取消和语句边界取消。Go [`scan_test.go`](scan_test.go) 还覆盖真实 worker 生命周期、会话检查、重试等待和 kill-statement 等 Rust 当前抽象未直接实现的行为。

## 扩展指南

- 修改分页 SQL或索引顺序时，优先同时审查 `scan_sql`、`scan_columns`、`order_key`，保证投影列、排序列、游标宽度与删除主键提取保持一致；扩展 [`scan_test.rs`](scan_test.rs) 的 SQL 精确断言、NULL/唯一与非唯一复合索引案例。
- 修改重试、取消、熔断或计数时，接入 `execute_with_checkpoint`，保持语句返回后先检查取消、派发成功后再计数的 Go 顺序；同步单元测试及 [`scan_integration_test.rs`](scan_integration_test.rs) 的取消场景。
- 修改恢复语义时，明确检查点保存的是“完整扫描投影行”还是“排序键”。当前入口接受 `Option<Row>` 并直接传给 `scan_sql`，而回调接收页面最后一条完整返回行；索引包含额外投影时，持久层与恢复层必须采用兼容编码，并测试进程接管后的首条 SQL。
- 新增错误分类时，同时审查 `TaskTerminateReason`、`ScanResult.error` 与 `TaskManager::report_finished`，尤其不能误把应重排的 worker 停止当成永久完成。
- 若把 `ScanWorker` 扩展为真实并发 worker，应在独立源文件中实现线程/通道/锁与停止生命周期，并把测试放在同目录独立 `*_test.rs`，不要把测试内嵌到 `scan.rs`。还需决定 `Clone` 语义，避免误以为克隆共享状态。
- 性能风险主要来自每页字符串生成、索引游标 OR 条件随复合键宽度增长、返回行克隆及线性列名查找；优化时必须保留 NULL 排序、非唯一索引主键后缀和参数顺序的兼容性。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引含 11,467 个文件、307,296 个节点，目标目录可见 `scan.rs`（34 symbols）；`node --file pkg/ttl/ttlworker/scan.rs --offset 1 --limit 500` 读取了完整 465 行，并报告文件级使用者 `del.rs`、`del_test.rs`、`scan_test.rs`。对 `TtlScanTask`、`ScanWorker` 的 `query` 找到 Rust/Go 对照符号；方法名查询与 `callers/callees` 未返回可用边，已用源码搜索补足。
- 生产源码：完整阅读 [`scan.rs`](scan.rs)、[`scan.go`](scan.go)、[`session.rs`](session.rs)、[`task_manager.rs`](task_manager.rs)、[`del.rs`](del.rs)、[`lib.rs`](lib.rs) 与 [`Cargo.toml`](Cargo.toml)，核对 crate、上游任务、会话、删除统计和结果消费边界。
- 测试证据：完整阅读 [`scan_test.rs`](scan_test.rs) 与 [`scan_integration_test.rs`](scan_integration_test.rs)；用 Go [`scan_test.go`](scan_test.go) 和 [`scan_integration_test.go`](scan_integration_test.go) 核对调度、重试、取消及原实现的额外生命周期职责。
- 本任务是纯文档分析，没有运行 Cargo。交付前按任务文件执行固定十一章节结构命令，并人工复核文档只陈述上述源码与测试可验证的当前行为。

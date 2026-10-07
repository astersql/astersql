# `pkg/executor/distsql.rs`

## 文件定位

`pkg/executor/distsql.rs` 属于 `astersql-executor` crate；crate 根在 `pkg/executor/lib.rs` 中以 `pub mod distsql` 公开它，Cargo 清单是 `pkg/executor/Cargo.toml`。文件实现 DistSQL 索引读取的 Rust 侧模型：`IndexReaderExecutor` 直接读取索引结果，`IndexLookUpExecutor` 则执行“索引取 handle、再按 handle 回表”的双读流程。

当前实现通过本文件定义的 `DistSqlBackend`、`SelectResult` 抽象隔离真实存储访问。仓库搜索到的 Rust 直接使用点主要是 `pkg/executor/distsql_test.rs`、`pkg/executor/executor_pkg_test.rs`、`pkg/executor/table_readers_required_rows_test.rs` 和 `pkg/executor/test/issuetest/executor_issue_test.rs`；未发现生产构建器将这里的泛型执行器接到会话执行主链。因此它是可运行、可测试的移植实现，但不能据此断言已替代 Go `pkg/executor/distsql.go` 的生产路径。

## 核心职责

- 定义最小 DistSQL 数据协议：`Handle`、`Datum`、`Row`、`KeyRange`、`Request`、`SelectResult` 和 `DistSqlBackend`。
- 在 `IndexReaderExecutor::{Open, open, Next, Close}` 中完成逻辑索引范围到 KV 范围的转换、请求构造、流式取数，以及多范围有序读的本地归并排序。
- 在 `IndexLookUpExecutor::{Open, startWorkers, Next, Close}` 中建立一个索引线程和多个回表线程组成的有界 channel 流水线。
- 在 `indexWorker` 中扫描索引、解码 handle、按逐步放大的批次生成 `lookupTableTask`，并可用 `AdaptiveLimitController` 控制准入。
- 在 `tableWorker` 中按 handle 回表、恢复索引顺序，并检查缺行或可选 checksum 不一致。
- 提供范围内存记账、批大小计算、缺失 handle 计算、结果包装和 `IndexLookUpRunTimeStats` 等辅助能力。

## 主要符号

### 数据和边界抽象

- `Handle::{Int, Common, Partition}` 表达整型主键、编码 common handle 和带物理分区前缀的 handle；其 `Ord` 实现也让它可用作 `BTreeMap`/`BTreeSet` 键。
- `Datum` 与 `Row = Vec<Datum>` 是本文件的简化行表示；`KeyRange` 是 `[start, end)` 半开区间，`Request` 携带 ranges、顺序、并发度、计划 ID 和统计开关。
- `SelectResult` 规定按容量拉取、关闭以及报告 in-flight 代价；`DistSqlBackend` 承担范围编码、索引/表扫描、handle 解码、排序比较、checksum 和一致性报告。后端错误统一进入 `DistSqlError`。
- `DistSqlError` 区分后端失败、取消、已关闭、解码失败、索引/表不一致、无效计划和 worker panic；`Inconsistent` 保留期望数、实际数和缺失 handle。

### 执行器和任务

- `IndexReaderExecutor<B>` 保存索引范围、下推计划、顺序选项、流式结果及 merge 缓冲；`range_mem_tracker` 只对构造后的 key range 容量记账。
- `IndexLookUpExecutor<B>` 保存双读计划、分区范围、handle 解码规则、同步 channel、线程句柄、乱序完成任务缓冲、当前行缓冲、运行时统计和可选自适应准入控制器。
- `lookupTableTask` 是流水线所有权单元：索引侧写入 `handles`/`index_rows`/`row_idx`，表侧写入 `rows`、错误、完成时间和内存估计；`adaptive_limit_reservation` 必须在完成或异常路径释放。
- `indexWorker<B>` 负责 `fetchHandles`、handle 提取和任务派发；`tableWorker<B>` 负责 `executeTask` 与 `compareData`。
- `IndexLookUpRunTimeStats` 累计索引取 handle 和回表任务耗时/次数，并可保存关闭时的 `AdaptiveLimitSnapshot`；`String`、`Clone`、`Merge`、`Tp` 对齐 Go 运行时统计接口的命名。

### 关键辅助函数

- `buildKeyRanges` 按整表或 `partition_range_map` 调用 `DistSqlBackend::index_ranges`，生成带 `physicalTblID` 的范围组。
- `CalculateBatchSize` 从初始值按二倍增长到覆盖估算行数或达到上限；`needMergeSort` 仅在存在排序项且范围多于一个时返回真。
- `getIndexScanMaxInFlight`、`getSelectResultInFlightCost`、`getMergeSortSharedCoprRequestRateLimit`、`getMergeSortIndexScanConcurrency` 提供索引扫描调度参数，但本文件当前主流程没有全部消费这些辅助值。
- `GetLackHandles` 从 `obtained` 集合逐个移除期望 handle，因此重复的期望 handle 会被报告为缺失；`getDatumRow` 最多复制与字段元数据数量相同的列。
- `closeAll` 尝试关闭全部 `Closeable`，只返回第一个错误；`panic_error` 把线程 panic payload 转为 `WorkerPanic`。

## 执行流程

### IndexReader

1. `IndexReaderExecutor::Open` 若有 `access_conditions`，先经 `rebuildIndexRanges` 让后端重建逻辑范围。
2. `buildKVRangesForIndexReader` 调用 `DistSqlBackend::index_ranges` 将表 ID、索引 ID 和逻辑范围编码成物理 KV 范围，并通过 `consume_key_range_memory` 记账。
3. `open` 按 start key 排序范围，降序查询时反转。若 `needMergeSort(by_items, ranges.len())` 为真，则逐范围调用 `select`、每次最多拉 1024 行、关闭每个结果，最后由后端 `compare_rows` 全量排序并写入 `merged_rows`；否则创建单个流式 `result`。
4. `Next(capacity)` 优先排空 `merged_rows`，否则委托 `SelectResult::next`，并累计 `runtime_rows`；未 `Open` 或 dummy 执行器返回空行。
5. `Close` 关闭流式结果并清空 merge 缓冲。

### IndexLookUp

1. `IndexLookUpExecutor::Open` 调用 `buildTableKeyRanges`，再由 `open` 初始化运行时统计和有界 channel；非 dummy 执行器进入 `startWorkers`。
2. `startWorkers` 创建 `dist_sql_concurrency` 个 table worker。它们共享一个由 `Mutex` 保护的 `Receiver`，逐任务调用 `tableWorker::executeTask`，然后把结果送到 result channel。另一个 index worker 调用 `indexWorker::fetchHandles`；错误会包装为空任务写到结果 channel，并设置 `cancelled`。
3. `fetchHandles` 一次取得索引扫描结果，从 `batch_size` 开始切片。启用自适应控制时先 `ReserveLookup`；每批由 `decode_handle` 提取 handle、构造 `lookupTableTask` 并发送。传统路径将批大小指数增长到 1024，自适应路径改用 `SuggestedBatchSize`；`pushed_limit` 通过已扫描 key 数终止继续派发。
4. table worker 调用 `table_scan`。启用 `check_index_value` 时，`compareData` 检查数量和逐行 checksum；启用 `keep_order` 时，按原 handle 到位置的映射重排表行；强一致读要求返回行数等于 handle 数，弱一致读允许短缺。
5. `IndexLookUpExecutor::Next` 从 result channel 收集任务。`keep_order` 为真时，`getResultTask` 用 task ID 和 `pending` 暂存乱序完成任务；随后把任务行加入 `current`，按调用者容量返回。自适应 reservation 仅在该任务全部缓冲行被消费后调用 `CompleteLookup`，任务错误则调用 `AbortLookup`。
6. `Close` 先置取消标记并停止准入控制器，断开任务发送端，依次 join 索引线程与表线程，清空行缓冲；直接 IndexLookUp 配置下还把 controller snapshot 写入运行时统计。

## 数据与状态

- `IndexReaderExecutor` 的状态从未打开的 `result = None`，转到流式 `result` 或全量 `merged_rows` 二选一；`runtime_rows` 只统计实际由 `Next` 返回的行数。dummy 模式不创建请求。
- `IndexLookUpExecutor` 用 `next_task_id` 标识下一个可输出任务，用 `pending: BTreeMap<usize, lookupTableTask>` 保存提前完成的任务，用 `current` 保存已按任务顺序接收但尚未返回的行。
- `adaptive_current` 的四元组记录一个任务剩余未消费行、总行数、reservation 和 handle 数，使 `CompleteLookup` 与实际消费完成绑定，而不是与 worker 完成绑定。
- `cancelled: Arc<AtomicBool>` 在 worker 间发布停止状态；`stats: Arc<Mutex<_>>` 保护跨线程统计；`Arc<B>` 使一个后端实例能被所有 worker 安全共享。
- `consume_key_range_memory` 按 `KeyRange` 结构大小以及 start/end 向量的 capacity 估算占用，并用饱和加法和 `i64::MAX` 封顶。它只增加 tracker 计数；本文件没有对应释放，因此 tracker 的生命周期/重置由外层负责。
- `lookupTableTask::mem_usage` 只是表行列数之和的简化估算，当前 Rust 流程没有把该值接到 `Tracker`；不能把它等同于 Go 版本的完整 chunk、handle 与 reader 内存记账。

## 依赖与调用关系

- crate 装配：`pkg/executor/lib.rs` → `pub mod distsql`；测试装配：同文件的 `#[path = "distsql_test.rs"] mod distsql_test`。
- 本文件唯一直接外部 Rust 依赖是 `astersql_executor_internal_exec::adaptive_limit_controller::{AdaptiveLimitController, AdaptiveLimitSnapshot}` 和 `astersql_util_memory::tracker::Tracker`，它们都由 `pkg/executor/Cargo.toml` 的 workspace 路径依赖提供。
- `IndexReaderExecutor::Open` → `rebuildIndexRanges`/`buildKVRangesForIndexReader` → `DistSqlBackend::{rebuild_index_ranges,index_ranges}` → `IndexReaderExecutor::open` → `DistSqlBackend::select`。
- `IndexLookUpExecutor::Open` → `buildKeyRanges`/`open`/`startWorkers`；index thread → `DistSqlBackend::index_scan`/`decode_handle` → `table_tx`；table threads → `DistSqlBackend::table_scan`/`row_handle`/`checksum`/`report_inconsistency` → `result_tx`；调用线程通过 `Next` 消费。
- RustCodeGraph 索引能定位本文件符号（例如 `IndexLookUpExecutor`、`fetchHandles`、`executeTask`）和完整源码，但对这些 Rust 方法执行 `callers/callees` 未返回静态边。仓库级 `rg` 补充确认：生产模块仅公开该模块，直接构造证据位于前述独立测试；因此文档不虚构上游生产调用者。

## 错误处理与边界

- 所有后端可失败操作使用 `Result<_, DistSqlError>` 向上传播。`IndexReaderExecutor::open` 在分段 merge 路径中若拉取或排序失败会立即返回；已创建但尚未显式关闭的 trait object 只能依赖析构，因为该错误路径没有统一 `closeAll`。
- `sort_rows` 因标准排序比较器不能返回 `Result`，会记录首个 `compare_rows` 错误、暂以 `Ordering::Equal` 继续排序，排序结束后再返回该错误；发生错误时不能使用部分排序结果。
- channel 发送端或接收端断开映射为 `Closed`/流结束。index worker 的错误通过错误任务传递；table worker 错误写入当前任务并设置全局取消标记。
- `getHandleOffsets` 在没有显式 offsets 时取最后一列；输入类型长度为零则返回 `InvalidPlan("index result has no handle column")`。
- 强一致模式下回表缺行会调用 `report_inconsistency` 并返回带缺失 handle 的 `Inconsistent`；弱一致模式跳过这一数量检查。checksum 模式数量或值不同也返回 `Inconsistent`。
- `Close` 把 worker panic 映射为 `WorkerPanic`，但遇到第一个 join 错误会提前返回，后续线程可能尚未 join；扩展关闭逻辑时应特别保护“尽量收完全部资源”的性质。
- `capacity == 0` 时两个 `Next` 都返回空集合而不推进数据；`concurrency` 构造时至少为 1；批量和内存计算使用饱和运算避免整数溢出。

## 并发与资源生命周期

- `startWorkers` 使用容量 32 的 `sync_channel` 提供背压。一个 index thread 生产回表任务，多个 table threads 竞争同一个加锁 receiver，所有完成任务汇入单个 result channel。
- `Acquire` 读取和 `Release` 写入 `cancelled` 建立跨线程可见性。取消只在每个 table worker 接收下一任务前检查；已经进入后端 `table_scan` 的调用依赖后端自行返回，本文件没有可传递的取消 token。
- 主执行器通过 `JoinHandle` 拥有线程。正常生命周期必须是 `Open` → 若干 `Next` → `Close`；`Close` 先停止生产并丢弃 sender，再 join，避免 worker 永久等待新任务。
- `keep_order` 不限制回表并发，只在消费侧按 task ID 排序、在任务内按 handle 原顺序排序；代价是慢任务会让后续完成任务堆积在 `pending`。
- `SelectResult` 资源在 IndexReader 的 `Close`、merge 分段读取结束、`selectResultWithMeta::Close` 和 `selectResultList::Close` 中释放。后两个 `Close` 有意忽略底层关闭错误。
- 自适应准入 reservation 的所有权从 index worker 的局部变量移入 `extractedLookupTaskData`，再移入 `lookupTableTask`，最终在消费完成、任务错误或发送失败时完成/回滚；修改流程时必须维持“每份 reservation 恰好结算一次”。

## 与 Go 版本的对应关系

- 直接对照文件是 `pkg/executor/distsql.go`。Rust 保留了 Go 的 `IndexReaderExecutor`、`IndexLookUpExecutor`、`lookupTableTask`、`indexWorker`、`tableWorker`、`CalculateBatchSize`、`GetLackHandles`、`getDatumRow` 和运行时统计等名称与总体双读结构。
- Rust `CalculateBatchSize` 与 Go 的二倍增长/最大值规则一致；`pkg/executor/distsql_test.rs` 明确验证估算值小于初始值、超过最大值的行为。Rust `GetLackHandles` 也像 Go 一样消费已获得集合，所以重复期望 handle 会留下一个缺失项。
- Go 生产实现直接使用 TiDB 的 `kv.Request`、DAG protobuf、`distsql.SelectResult`、session/statement context、chunk、table metadata、collation、paging、replica read、内存 tracker 和运行时统计集合。Rust 将这些压缩为 `Request`、`Datum` 和 `DistSqlBackend`，尚未表达全部请求属性和类型语义。
- Go 的 IndexReader merge 路径使用多个 SelectResult 的排序封装；Rust 当前逐范围把所有行读入内存后调用 `sort_rows`。这保持结果排序意图，但流式性、峰值内存和关闭错误处理并不等价。
- Go IndexLookUp 延迟到首次 `Next` 才启动 worker，并使用可动态扩缩的 `workerPool`；Rust `Open` 立即启动固定数量 OS 线程。Go 用 context 取消和 `finished` channel，Rust 用原子取消标记与 sender 断开。
- Go 的分区 handle 判定取决于 global index、keep-order、额外物理表 ID 列及 push-down 模式；Rust `needPartitionHandle` 仅返回 `partition_mode`。Go 的 common handle 还处理 collation、截断和编码，Rust 委托后端解码。
- Go 一致性检查逐列比较真实索引值/表值并生成详细 consistency report；Rust 用数量和整行 checksum 近似。Go 内存释放、指标注册、索引使用报告、coprocessor paging 和 push-down 逻辑也未在本文件完整复刻。
- 因此新增功能必须以 Go 文件为语义基准逐项移植，不能只让简化后端测试通过就宣称与 Go 完全等价。

## 扩展指南

- 新增请求属性时，从 `Request` 和 `DistSqlBackend` 开始，分别检查 `IndexReaderExecutor::buildKVReq`、`IndexLookUpExecutor::{index_request,table_request}`，并同步 Go `RequestBuilder` 的对应设置；避免在 worker 中临时拼装导致两条读路径漂移。
- 扩展 handle/分区语义时，集中修改 `Handle`、`getHandle`/`decode_handle` 调用和 `needPartitionHandle`，并对照 Go 的 global index、common handle collation、`ExtraPhysTblID` 和 push-down 限制。
- 改动流水线时，必须审计 channel 断开、取消、所有 `JoinHandle`、错误任务传播，以及 reservation 在成功、空结果、发送失败、后端失败和提前关闭路径上的恰好一次结算。
- 改动保序或 LIMIT 时，同时检查任务 ID、`pending`、任务内行重排、`scanned_keys` 与 offset+count；性能风险主要是 pending 堆积、全量 index scan 以及 merge 路径全量物化。
- 改动一致性检查时，不应停留在 checksum 近似，应对照 Go `tableWorker::compareData` 的逐列类型/collation/截断语义和报告内容。
- 测试必须继续放在独立文件。直接单元测试优先扩展 `pkg/executor/distsql_test.rs`；执行器构造、关闭和并发行为可扩展 `pkg/executor/executor_pkg_test.rs`、`pkg/executor/table_readers_required_rows_test.rs`；Go 对照回归位于 `pkg/executor/distsql_test.go`。不要把 `#[cfg(test)]` 测试内嵌进本源文件。
- 若要接入真实生产主链，还需在 executor builder/adapter 层提供具体 `DistSqlBackend` 并验证真实存储请求；当前仓库没有该接线证据，应作为独立实现任务处理。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件、307296 个节点、1848419 条边；`node --file pkg/executor/distsql.rs` 覆盖 1–1406 行，并用于核对所有类型、函数和实现。精确 `query` 定位了 Rust `IndexLookUpExecutor`（424 行）、`fetchHandles`（918 行）、`executeTask`（1239 行）及两个执行器的 `Open`（316、476 行）；对应 `callers/callees` 查询无返回，故使用仓库搜索补证而未推测调用边。
- crate 与装配：读取 `pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`，确认 crate 名、路径依赖、`pub mod distsql` 和独立 `distsql_test` 模块；目标包下未发现 `doc.go`。
- Rust 测试：完整读取 `pkg/executor/distsql_test.rs`；另外通过引用搜索核对 `pkg/executor/executor_pkg_test.rs`、`pkg/executor/table_readers_required_rows_test.rs`、`pkg/executor/test/issuetest/executor_issue_test.rs` 的直接使用点。
- Go 对照：读取 `pkg/executor/distsql.go` 的任务结构、两个执行器、range 构造、worker 启停、取数/关闭、handle 解码、一致性检查和运行时统计；检查 `pkg/executor/distsql_test.go` 的测试清单，包括不一致索引、分区回表、自适应 LIMIT、统计和 push-down 等覆盖面。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，验证文档存在且恰有十一个固定二级标题；人工复核以上章节分别回答文件为何存在、如何运行、如何安全扩展，并明确当前未接线和简化语义的边界。

# `pkg/executor/index_merge_reader.rs`

## 文件定位

本文件属于 `astersql-executor` crate。crate 由 [`pkg/executor/Cargo.toml`](Cargo.toml) 声明，模块由 [`pkg/executor/lib.rs`](lib.rs) 中的 `pub mod index_merge_reader;` 公开。它实现 Index Merge 的运行时执行阶段：接收规划器已经选择的多条 partial 索引/表扫描路径，合并这些路径得到的行 handle，再按 handle 回表生成 SQL 执行器需要的 `chunk::Chunk`。

上游构建入口在 [`pkg/executor/builder.rs`](builder.rs) 的 `executorBuilder::buildIndexMergeReader` / `buildNoRangeIndexMergeReader`：先取得快照时间并构建 draft，再通过 `finalize_index_merge_reader` 接入具体运行时依赖。本文所述执行器本身不解析 SQL、选择索引或直接实现 KV 协议；这些操作经 `IndexMergeRuntimeContext`、`IndexMergePartialSource` 和 `IndexMergeTableReader` 注入。

## 核心职责

- `IndexMergeReaderExecutor` 管理 `Open`、首次 `Next` 时惰性启动 worker、逐块输出和 `Close` 回收的完整生命周期。
- 每条 `IndexMergePartialPlan` 启动一个 partial worker，从索引路径或表路径分批提取 `IndexMergeHandle`；批大小从 `MaxChunkSize` 开始，逐轮倍增至 `IndexLookupSize`。
- `indexMergeProcessWorker` 根据执行模式做普通并集去重、有序并集合并或分区交集，并应用 `PushedDownLimit`。
- `indexMergeTableScanWorker` 将 handle 分批交给最终表读取器，必要时按 handle 顺序重排结果，并把完成态交给主线程的 `Next`。
- 通过有界同步通道、取消句柄、结束信号、条件变量、内存计数和运行时统计控制背压、错误传播与资源释放。

当前文件是可运行实现，不是占位门面；但规划器、KV range、表达式、表和会话的具体类型被有意收敛到 `IndexMergeRuntimeContext` 及 `IndexMergeOpaque` 边界中。

## 主要符号

- `IndexMergeReaderExecutor`：公开执行器。构造参数固定执行模式（`keepOrder`、`isIntersection`、`partitionTableMode`、`hasGlobalIndex`）、partial 计划和下推 limit；`new` 要求 partial 计划非空。
- `IndexMergeRuntimeContext`：运行时适配接口，负责重建相关列 range、构造 KV range 与扫描源/回表读取器、分区 handle 校验、并发和批量参数、内存/统计挂接及 failpoint。其方法是本文件到 planner、KV、session 和 table 层的主要边界。
- `IndexMergePartialSource` / `IndexMergeTableReader` / `IndexMergeCancellation`：分别抽象 partial handle 流、最终行流和可取消阻塞操作；两类读取器都必须提供 `Close` 或 `Cancel` 能力。
- `IndexMergePartialPlan`、`IndexMergeRangeGroup`、`IndexMergePartialBatch`：描述一条 partial 路径、按物理表分组的 range，以及该路径一次产生的 handle 批次。
- `IndexMergeHandle`：以 `partition_id + encoded` 标识行，`order_keys` 为有序合并提供比较键；`MemUsage` 估算 handle 堆内存。
- `indexMergeTableTask`：贯穿 fetch、work、result 三段流水线的任务。其 `taskCompletion` 用 `Mutex + Condvar` 保存结果 chunk、游标、错误、完成位和内存占用。
- `partialTableWorker` / `partialIndexWorker`：从数据源读取 handle。索引 worker 复用表 worker 的批量循环，只改变分区 handle 校验语义。
- `indexMergeProcessWorker`：执行 `fetchLoopUnion`、`fetchLoopUnionWithOrderBy` 或 `fetchLoopIntersection`，然后把回表任务同时投递到 work 与 result 通道。
- `handleHeap` / `rowIdx`：保留与 Go 有序合并结构对应的公开数据结构；当前 Rust 有序路径实际在 `fetchLoopUnionWithOrderBy` 中收集、去重并调用 `sort_by`，`NewHandleHeap` 仅构造该结构。
- `intersectionProcessWorker` / `intersectionCollectWorker`：按分区桶并行统计 handle 覆盖的 partial 路径数，并统一裁剪下推 limit。
- `indexMergeTableScanWorker`：调用 `buildFinalTableReader`，拉取全部结果块，按需 `ReorderFinalRows`，校验 handle/行数后完成任务。
- `IndexMergeMemoryTracker` / `IndexMergeRuntimeStat`：原子维护内存字节、取 handle/合并/等待/回表耗时和任务数；统计支持 `String`、`Clone`、`Merge`、`Tp`。
- `finishSignal`、`sendUntilFinished`、`receiveUntilFinished`、`receiveWork`：实现结束感知、通道背压和避免永久阻塞的内部同步原语。

## 执行流程

1. builder 把物理 Index Merge 计划转换为 `IndexMergePartialPlan` 和运行时上下文，并调用 `IndexMergeReaderExecutor::new`。构造期只保存不可变配置，不创建线程。
2. `Open` 初始化可选统计；`rebuildRangeForCorCol` 只处理 `correlated_access` 路径；`buildPartialWorkerKVRanges` 要求返回的 range 组数与 partial 计划数严格相等。随后挂接内存跟踪器并建立三条有界通道：partial→process 的 fetch、process→table-scan 的 work、process→主线程的 result。
3. 第一次 `Next` 调用 `startWorkers`。它先为每条 partial 路径构建 source 并登记 cancellation，再各起一个 partial 线程；另起一个 process 线程、`IndexLookupConcurrency().max(1)` 个回表线程和一个负责 join 的 coordinator 线程。`isIntersection && keepOrder` 在启动前直接报错。
4. partial worker 反复调用 `NextHandles`，把批次包装为 `indexMergeTableTask`。非交集路径通过 `applyPartialLimit` 停止保留超过 `offset + count` 窗口的 handle；扫描结束总会调用 source 的 `Close`，关闭失败只记录日志，不覆盖主错误。
5. process worker 等到 fetch 中的 `Finished`：
   - 普通并集以 `(partition_id, encoded)` 去重，再应用 limit。
   - 有序并集先检查每个 handle 的 `order_keys` 数量，按 `byItemsDesc` 全局排序，再裁剪 limit。
   - 交集按 `parTblIdx % workerCount` 分桶，子线程记录每个 handle 出现过的 `partialPlanID` 集合；仅集合大小等于总路径数的 handle 被保留，最后应用 limit。
6. `dispatchHandles` 按 `IndexLookupSize` 分批。分区表使用局部索引时，`keepOrder` 模式逐 handle 派发，否则按 `partition_id` 聚批；全局索引或普通表走普通批处理。每个任务先送 work 通道供回表线程执行，再送 result 通道保持主线程可见的任务顺序。
7. 回表 worker 为任务构建 `IndexMergeTableReader`，登记其 cancellation，循环 `Next` 至空块并累加内存。`keepOrder` 时经上下文重排；启用 `ValidateFinalRowCount` 时，返回行数必须等于 handle 数。
8. 主线程 `Next` 从 result 通道取任务并等待其完成，使用任务内 chunk/row 游标最多填充 `MaxChunkSize` 行。任务完全消费后扣回其登记内存。收到 `Finished` 表示 EOF。
9. `Close` 先触发 `finishSignal`，取消已登记的 partial/回表读取器，再等待 coordinator；之后报告每条 partial 的索引使用、注册统计、卸下内存跟踪器并清空运行态，使执行器不再持有线程和通道。

## 数据与状态

构造后基本不变的状态包括 `ctx`、`partialPlans`、排序/交并模式、分区/全局索引标志和 pushed limit。`Open` 生成的运行态包括 range、`indexMergeShared`、结果接收端、内存 tracker 和统计；`startWorkers` 再设置 `workerStarted` 与 coordinator。

`indexMergeShared` 被所有线程以 `Arc` 共享；通道接收端因标准库 `Receiver` 不是 `Sync` 而包在 `Arc<Mutex<_>>` 中。任务的 `completion.state` 同时承担完成通知、错误槽、结果游标和内存释放凭据。handle 的唯一键是 `(partition_id, encoded)`：相同编码位于不同分区时不是同一行；有序比较先逐个比较 `order_keys` 并应用升降序，最终用 `IndexMergeHandle::Ord` 打破平局。

内存跟踪是增量记账而非所有权分配器：无序并集登记保留 handle，heap push/pop 登记 `rowIdx`，交集 worker 登记集合行项，回表任务登记 handle 容量和 chunk 内存；主线程消费完任务后抵扣 `taskState.memUsage`。扩展算法时必须保证新增保留结构的正负记账成对。

## 依赖与调用关系

上游关系为 `executorBuilder::buildIndexMergeReader` → `buildNoRangeIndexMergeReader` / `finalize_index_merge_reader` → `IndexMergeReaderExecutor`。RustCodeGraph 还确认模块被 `pkg/executor/test/indexmergereadtest/index_merge_reader_test.rs` 直接引用；`pkg/executor/lib.rs` 公开模块。

下游直接 Rust 依赖很窄：`astersql_errors::SharedError` 统一错误类型，`astersql_util_chunk::Chunk` 承载向 SQL 执行框架输出的列式行块，标准库的 `thread`、`mpsc`、`Arc`、`Mutex`、`Condvar` 和原子类型实现并发。更宽的 planner/KV/table/session 依赖通过 `IndexMergeRuntimeContext` 注入，因此 `Cargo.toml` 虽声明完整 executor crate 依赖，本文件直接 import 的 workspace crate 只有 errors 与 chunk。

关键内部调用边是：`Open` → range 重建/构造；`Next` → `startWorkers` → partial/process/table-scan worker；partial worker → `IndexMergePartialSource::NextHandles`；process worker → union/order/intersection 分支 → `dispatchHandles`；table-scan worker → `BuildFinalTableReader` → `IndexMergeTableReader::Next`；`Close` → `finishSignal::Finish` / `Cancellation::Cancel` / coordinator join。

## 错误处理与边界

- `new` 对空 partial 计划使用断言；调用方必须在构建期保证至少一条路径。
- `Open` 传播相关列 range、KV range 构建错误，并显式拒绝计划数与 range 组数不一致。
- `startWorkers` 拒绝 `intersection + keepOrder`；`fetchLoopIntersectionWithOrderBy` 同样 panic，表明该组合尚未实现而非静默降级。
- 有序分支要求 `order_keys.len() >= byItemsDesc.len()`；不足时返回 `ordered index merge task has insufficient order keys`。
- partial/process 错误被封装为已完成的错误任务，跨 fetch/result 通道送回 `Next`；worker panic 通过 `catch_unwind` 转换为带 worker 名的 `SharedError`，并在 process/partial panic 时输出 backtrace。table-scan panic 则完成当前任务，避免等待方永久阻塞。
- result 通道异常断开返回明确错误；等待中的任务若收到 finish，返回 `index merge task interrupted`。
- source/reader 的 `Close` 错误仅交给 `LogCloseError`，不会覆盖已经发生的读取错误。`Close` 本身当前总是返回 `Ok(())`，线程 join 的 panic 结果也被忽略，因为 worker 已有单独的错误转换路径。
- `ValidateFinalRowCount` 打开时，回表行数与 handle 数不等即报错；该检查可捕获回表遗漏或额外行。
- limit 计算用 `saturating_add` 防止 `offset + count` 在切片端溢出，但 `applyPartialLimit` 的 `limit.offset + limit.count` 是普通 `u64` 加法；给该路径引入极端 limit 时应补充溢出边界测试。

## 并发与资源生命周期

fetch/result 通道容量取 `LookupTaskChannelSize().max(1)`，work 通道容量固定为 1，形成显式背压。发送方使用 `try_send + finishSignal::Wait`，接收方使用短超时轮询，确保 `Close` 即使发生在通道满、源阻塞或任务等待期间也能推进退出。实际 I/O 阻塞依靠每个 source/reader 的 `IndexMergeCancellation::Cancel` 打断。

coordinator 的顺序保证为：join 全部 partial → 向 fetch 发送 `Finished` → join process → join 所有 table-scan worker。process 完成后设置 `workFinished`，回表 worker 在 work 通道暂时为空时据此退出；process 还向 result 发送 `Finished`，使主线程识别 EOF。`Close` 必须发生在使用结束或错误退出后，否则 memory tracker 与上下文统计仍保持挂接。

交集分支会临时再创建至多 `IntersectionConcurrency` 个线程，并按分区下标分桶以避免同一分区散落到不同交集 worker。共享统计和内存使用原子累加；任务完成态、接收端和 cancellation 列表分别由独立 mutex 保护。全局测试钩子 `IndexMergeCancelFuncForTest` 也是 mutex 保护的单一函数指针。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/executor/index_merge_reader.go`](index_merge_reader.go)。两版保留相同的核心命名和阶段：`IndexMergeReaderExecutor`、partial table/index worker、process worker、table-scan worker、union/order/intersection 分支、pushed limit、分区/全局索引、内存与运行时统计。Go 的 `Open`/`Next`/`Close` 和 Rust 的生命周期顺序也一致。

主要实现差异如下：

- Go 结构直接持有 TiDB 的 table、DAG request、physical plan、range、session/data reader builder 等具体类型；Rust 通过 `IndexMergeRuntimeContext` 和 `IndexMergeOpaque` 隔离这些实现，以便移植边界保持稳定。
- Go 使用 goroutine、channel、`sync.WaitGroup` 与 context；Rust 使用 OS thread、`sync_channel`、coordinator `JoinHandle`、`finishSignal` 和显式 cancellation。
- Go 的有序并集实现使用 `handleHeap` 做多路归并；当前 Rust `fetchLoopUnionWithOrderBy` 收集全部 handle 后去重并全局排序，虽然仍保留 `handleHeap` 类型。这会带来更高峰值内存与排序复杂度，是与 Go 尚未完全同构的性能差异，不能把 Rust 文档描述成流式堆归并。
- 两版都不支持 intersection 与 keep-order 组合；Go 留有未实现分支，Rust 在 `startWorkers` 返回错误并保留 panic 防线。
- Go 版包含更多特定 failpoint、coprocessor 泄漏防护和具体 runtime stats/plan ID 接线；Rust 把 failpoint 触发交给上下文，目前只在 partial worker 和 intersection worker 的关键点调用。

## 扩展指南

- 新增合并策略：在 `startIndexMergeProcessWorker` 增加明确分支，并实现独立 worker 方法；保持 fetch 完结、work/result 双投递、错误任务传播和 `workFinished`/`Finished` 信号不变量。测试应放在独立文件 [`pkg/executor/test/indexmergereadtest/index_merge_reader_test.rs`](test/indexmergereadtest/index_merge_reader_test.rs)，不要把测试嵌入本源文件。
- 将有序并集改回流式多路归并：主要修改 `fetchLoopUnionWithOrderBy`、`handleHeap` 和 `rowIdx`；需覆盖重复 handle、多个排序键与 DESC、limit 大于 1024、分区表和内存释放，并对照 Go `fetchLoopUnionWithOrderBy`。
- 支持 intersection + ORDER BY：必须同时修改 `startWorkers`、`startIndexMergeProcessWorker`、`fetchLoopIntersectionWithOrderBy`，定义去重后的全局顺序及 limit 时机；在语义明确前不可删除现有拒绝检查。
- 增加新的 KV/计划能力：优先扩展 `IndexMergeRuntimeContext` 或 source/reader trait，由 builder 的 `finalize_index_merge_reader` 提供实现，避免让本文件重新依赖具体 planner/session 类型。
- 调整分区或全局索引行为：重点复核 `dispatchHandles`、`executeTask` 的 `partition` 选择和 `(partition_id, encoded)` 唯一键；同步动态/静态裁剪、局部/全局索引测试。
- 修改并发或关闭流程：必须覆盖通道满、主线程提前返回、partial/process/table worker panic、读取器阻塞取消和重复关闭。Go 测试 [`pkg/executor/test/indexmergereadtest/index_merge_reader_test.go`](test/indexmergereadtest/index_merge_reader_test.go) 中的 panic、hang、error、goroutine leak 与 memory tracker 用例是行为清单；Rust `seq_executor_test.rs::test_index_merge_reader_close` 仅是较薄的关闭回归证据，不应替代真实流水线测试。
- 修改内存统计：逐项列出新结构的登记和释放点，并同步 `IndexMergeReaderMemTracker` 对照场景；防止只增加 `Consume(+)` 而遗漏任务消费、heap pop 或关闭路径的 `Consume(-)`。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `pkg/executor/index_merge_reader.rs`，文件被识别为 1624 行、183 个符号；`query IndexMergeReaderExecutor` 定位 Rust 主结构于第 389 行，`node --file` 分段核对了完整源码。索引报告的直接使用文件包含 `pkg/executor/test/indexmergereadtest/index_merge_reader_test.rs`。
- 源码事实：[`pkg/executor/index_merge_reader.rs`](index_merge_reader.rs) 的 `Open`、`startWorkers`、三种 process loop、`dispatchHandles`、`indexMergeTableScanWorker::executeTask`、`Next` 与 `Close`；crate/module 边界来自 [`pkg/executor/Cargo.toml`](Cargo.toml) 和 [`pkg/executor/lib.rs`](lib.rs)。目标包不存在 `pkg/executor/doc.go`。
- 构建入口：[`pkg/executor/builder.rs`](builder.rs) 的 `executorBuilder::buildIndexMergeReader`、`buildNoRangeIndexMergeReader` 及 `ExecutorBuilderDependencies::{build_no_range_index_merge_reader, finalize_index_merge_reader}`。
- Go 对照：[`pkg/executor/index_merge_reader.go`](index_merge_reader.go) 中同名执行器和 worker；其函数清单覆盖 Open/Next/Close、并集、有序并集、交集、回表、错误同步和统计。
- Rust 测试：[`pkg/executor/test/indexmergereadtest/index_merge_reader_test.rs`](test/indexmergereadtest/index_merge_reader_test.rs) 覆盖 handle 去重、分区 union、交集并发、ORDER BY/LIMIT、limit 超过 1024、intersection embedded limit 和动态/静态分区裁剪；[`pkg/executor/test/seqtest/seq_executor_test.rs`](test/seqtest/seq_executor_test.rs) 的 `test_index_merge_reader_close` 提供错误后关闭的轻量证据。
- Go 测试：[`pkg/executor/test/indexmergereadtest/index_merge_reader_test.go`](test/indexmergereadtest/index_merge_reader_test.go) 进一步覆盖内存 tracker、悲观锁、worker panic/hang/error、coprocessor goroutine 泄漏、竞争和历史 issue。本文只读取这些用例作为迁移语义依据，按任务要求未运行 Cargo 或代码测试。

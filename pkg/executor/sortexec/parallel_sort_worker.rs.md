# `pkg/executor/sortexec/parallel_sort_worker.rs`

## 文件定位

本文件属于 `astersql-executor-sortexec` crate 的并行全排序路径。crate 由同目录 `Cargo.toml` 定义、以 `lib.rs` 为入口；`lib.rs` 公开声明 `parallel_sort_worker` 模块，但没有在 crate 根重导出 worker 类型。该 crate 在当前清单中只为 `cfg(windows)` 声明其 AsterSQL 依赖，表明这组 Rust 移植代码当前按 Windows 目标接线。

在应用内的直接位置是：`sort.rs::SortExec::fetchParallel` 按执行并发度创建多个 `Arc<Mutex<parallelSortWorker>>`，轮询把 `DataChunk` 交给 `saveChunk`；`parallel_sort_spill_helper.rs::parallelSortSpillHelper::{spill, mergeAll}` 随后锁住各 worker，调用 `multiWayMerge` 取得每个 worker 的有序结果，再与磁盘 run 做最终归并。因此，本文件负责的是“一个 worker 内”的缓存、本地排序、取消检查和内存归并，不负责拉取执行器子节点、线程调度或磁盘 I/O。

## 核心职责

- `parallelSortWorker` 保存分配给本 worker 的未排序 `DataChunk`，同时通过共享 `MemoryTracker` 记账（`saveChunk`）。
- 将缓存 chunk 展平成 `Vec<Row>`，使用注入的 `RowComparator` 原地排序，并把每次排序得到的有序 run 放进 `localSortedRows`（`sortBatch`）。
- 在排序比较达到 `parallelSortCheckSignalCheckpoint`（20,000 次）时轮询共享 kill 标志，把取消从 `sort_by` 的不可失败比较器边界转换成 `Result`（`sortBatch`、私有 `SortCancelled`）。
- 将 worker 内多个有序 run 通过 `memorySource` 和 `newMultiWayMerger` 归并为一个有序 `Vec<Row>`，成功后归还 worker 已记账内存（`multiWayMerge`）。
- 提供清理、显式归还内存和行数统计能力（`reset`、`releaseMemory`、`rowCount`）。

本文件不创建工作线程或通道。尽管类型名保留 Go 的 worker 命名，Rust 上游当前由 `SortExec` 顺序喂数据，并通过 `Arc<Mutex<_>>` 与 spill helper 共享 worker。

## 主要符号

- `struct SortCancelled`：仅在本模块内部使用的 panic 载荷。它不是业务错误类型；用途是让取消信号穿过 `slice::sort_by` 的 `Ordering` 回调，再在 `sortBatch` 边界转成 `SortError("query interrupted")`。
- `parallelSortCheckSignalCheckpoint: usize = 20_000`：排序比较次数的取消检查间隔，对应 Go 的 `SignalCheckpointForSort` 数值。
- `pub struct parallelSortWorker`：worker 状态容器。公开字段只有 `chunks: Vec<DataChunk>` 与 `localSortedRows: Vec<Vec<Row>>`；比较器、批量阈值、kill 标志、内存追踪与计数均为模块私有。
- `parallelSortWorker::new(compare, max_rows, killed, mem)`：构造空 worker，并以 `max_rows.max(1)` 保存阈值，避免零阈值。比较器与两个共享状态都以 `Arc` 持有。
- `saveChunk(&mut self, chunk) -> Result<()>`：先以 Acquire 读取 `killed`；未取消时按 `DataChunk::memory_usage` 消费 tracker、累加 `memoryBytes`，最后保存 chunk。
- `sortBatch(&mut self) -> Result<()>`：空缓存直接成功；否则 `drain` 全部 chunk、展平行、排序并追加一个本地有序 run。
- `sortLocal(&mut self) -> Result<()>`：检查当前缓存行数并调用 `sortBatch`。现有条件 `rows >= maxRowsInBatch || !chunks.is_empty()` 等价于“只要存在 chunk 就排序”，所以当前 `maxRowsInBatch` 不会让 `saveChunk` 自动分批，也不改变最终 flush 时机。
- `multiWayMerge(&mut self) -> Result<Vec<Row>>`：先 flush 未排序 chunk，以 `mem::take` 取走全部本地 run，内存归并成功后释放 `memoryBytes` 并清零记账。
- `reset(&mut self)`：丢弃未排序 chunk 与已排序 run，归还全部 worker 记账并清零；比较次数不重置。
- `releaseMemory(&self, bytes)`：直接委托共享 tracker 的 `release`，不修改 `memoryBytes`。调用方若使用它，必须自行维护 worker 账本一致性；当前直接调用点未在目标链路中发现。
- `rowCount(&self) -> usize`：合计未排序 chunk 行数与所有本地有序 run 行数。

## 执行流程

1. `SortExec::fetchParallel` 根据 `concurrency` 构造 worker，传入同一比较器、kill 标志和内存 tracker；每个 worker 的阈值是 `maxChunkSize * 8`（`sort.rs`）。
2. 子执行器每产出一个 chunk，`fetchParallel` 轮询选中 worker、取得互斥锁并调用 `saveChunk`。worker 在保存前检查取消，随后完成内存记账。
3. 内存 tracker 超限时，`SortExec` 创建或复用 `parallelSortSpillHelper`。helper 的 `spill` 逐个锁住 worker 并调用 `multiWayMerge`；没有 spill 时，输入耗尽后的 `mergeAll` 也走同一 worker 汇总入口（`parallel_sort_spill_helper.rs`）。
4. `multiWayMerge` 调用 `sortLocal`。只要 `chunks` 非空，`sortBatch` 就把所有 chunk 的行移入一个向量；空 chunk 列表则不产生 run。
5. `sortBatch` 的比较闭包持续累加 `timesOfRowCompare`。进入一次比较时若先前计数已达到 20,000，则检查 kill：未取消便把计数清零并继续，已取消则抛出 `SortCancelled`。
6. 排序成功时，完整有序向量追加到 `localSortedRows`。`multiWayMerge` 取走所有 run，构造 `memorySource`；`newMultiWayMerger(...).collect()` 以最小堆从各 run 头部逐行产出全局有序结果（`multi_way_merge.rs`）。
7. 归并成功后，worker 向共享 tracker 归还累计的 `memoryBytes`，将账本清零，并把物化后的有序向量交回 spill helper。helper 可将其写成 `DiskRun`，或与既有磁盘 run 再做最终归并。

## 数据与状态

- `chunks` 是尚未排序的所有权缓存。`sortBatch` 用 `drain(..)` 移走其中元素；因此成功或排序期间发生取消后，该字段都会为空。
- `localSortedRows` 中每个 `Vec<Row>` 都是独立有序 run；只有 `multiWayMerge` 保证跨 run 的全局顺序。该方法用 `mem::take` 清空字段并把 run 所有权交给归并源。
- `compare: RowComparator` 是 `Arc<dyn Fn(&Row, &Row) -> Ordering + Send + Sync>`。实际 SQL 排序键、升降序和 NULL 规则由上游构造的闭包决定，本文件不解释行值。
- `maxRowsInBatch` 至少为 1，但当前只有 `sortLocal` 读取，而且非空判断会覆盖阈值判断；它不是实时缓存上限。
- `killed: Arc<AtomicBool>` 在保存 chunk 和周期性比较时以 Acquire 读取；发出取消的一侧应以 Release 或更强顺序写入。独立 Rust 测试正是用 Release 写入。
- `memTracker` 是共享原子 tracker；`memoryBytes` 是本 worker 的本地累计账本。`saveChunk` 同时增加两者，成功的 `multiWayMerge` 或 `reset` 同时释放 tracker 并清零本地账本。
- `timesOfRowCompare` 跨 `sortBatch` 调用保留，达到检查点后才清零；`reset` 当前也不清零它。
- `rowCount` 只统计仍由两个容器持有的行，不代表 tracker 字节数，也不包含已经从 `multiWayMerge` 返回给调用方的行。

## 依赖与调用关系

上游直接调用关系（源码调用点与 RustCodeGraph 文件级使用关系共同核对）：

- `sort.rs::SortExec::fetchParallel` → `parallelSortWorker::new`、`saveChunk`。
- `parallel_sort_spill_helper.rs::parallelSortSpillHelper::spill` → `parallelSortWorker::multiWayMerge`。
- `parallel_sort_spill_helper.rs::parallelSortSpillHelper::mergeAll` → `parallelSortWorker::multiWayMerge`。
- `parallel_sort_worker_test.rs::cancellation_raised_during_comparison_interrupts_sort` → `new`、`saveChunk`、`sortBatch`。

主要下游关系：

- `sortBatch` → `DataChunk::{memory_usage, num_rows}` 间接相关、`RowComparator`、标准库 `Vec::sort_by`、`AtomicBool::load` 与 `SortError`。
- `multiWayMerge` → `sortLocal` → `sortBatch`，再 → `multi_way_merge.rs::memorySource::new`、`newMultiWayMerger`、`multiWayMerger::collect`。
- `saveChunk`、`multiWayMerge`、`reset`、`releaseMemory` → `sort_util.rs::MemoryTracker::{consume, release}`。

RustCodeGraph 将本文件列为被 `sort.rs`、`parallel_sort_worker_test.rs` 等文件使用，并确认 `multiWayMerge` 到 `sortLocal`、`newMultiWayMerger` 的调用边；工具没有为若干方法返回精细 caller 边，因此上游方法调用以对应源码位置补充核验。图中还出现 `index_merge_reader.rs`、`topn.rs`、`planner/cardinality/selectivity.rs` 的文件级使用结果，但在目标符号的精确源码搜索中没有找到调用，不能据此宣称这些模块直接使用该 worker。

## 错误处理与边界

- `saveChunk` 在已取消时返回 `SortError("query interrupted")`，且不会消费内存或保存 chunk；取消发生在检查之后时，本次保存仍可能完成，后续排序检查再中断。
- `sortBatch` 对没有 chunk 的情况幂等返回 `Ok(())`。空 `DataChunk` 仍会在 `saveChunk` 记账其容器开销，排序后可能形成空 run。
- 因 `sort_by` 比较器不能返回 `Result`，取消使用专用 panic 载荷。`catch_unwind(AssertUnwindSafe(...))` 只拦截 `SortCancelled`；比较器或标准库产生的其他 panic 会由 `resume_unwind` 原样继续传播，避免把程序错误误报成查询取消。
- 取消发生在排序中时，chunk 已被 `drain` 到局部 `rows`；返回错误后这些行被丢弃，不会写入 `localSortedRows`。此时 `memoryBytes` 和共享 tracker 仍保留原记账，调用方必须通过 `reset` 或其更高层清理路径释放。
- `multiWayMerge` 只在归并完全成功后释放内存。若 `sortLocal` 或归并返回错误，释放语句不会执行；同时 `mem::take` 已把本地 run 移出 worker，故错误后的复用与清理需要格外谨慎。当前 `memorySource` 的合法分区访问本身通常不报错，但接口仍保留 `Result`。
- 成功归并返回的 `Vec<Row>` 已不再计入该 worker 的 `memoryBytes`。调用方会接管结果并在 spill helper 中按其自身磁盘/内存模型继续处理。
- `MemoryTracker::release` 会饱和到零，避免负数；这不能防止 `releaseMemory` 与 `memoryBytes` 账本脱节，所以扩展代码不应随意混用显式释放和 worker 的自动释放。

## 并发与资源生命周期

worker 自身不实现 `Send` 循环、不持有通道，也不生成线程。共享方式由上游决定：`SortExec`/spill helper 将它包装为 `Arc<Mutex<parallelSortWorker>>`；任何读取公开容器或调用可变方法的代码都应遵守同一互斥边界。锁中毒由调用方转换为 `SortError("parallel sort worker lock poisoned")`，不是本文件处理。

kill 标志和内存 tracker 可跨 worker 共享且内部使用原子操作。行容器、`memoryBytes` 与比较次数依靠 worker 的 `&mut self` 及外层 mutex 串行访问。生命周期通常是“构造 → 多次 `saveChunk` → 一次或多次 spill/最终 `multiWayMerge` → 可再次接收 chunk”；`multiWayMerge` 成功后容器与本地内存账本归零，可继续复用。`reset` 是提前丢弃数据并释放账本的显式清理入口，但本类型没有 `Drop` 实现，所以直接丢弃仍持有数据的 worker 不会通过本文件主动调用 tracker 的 `release`。

排序会把所有当前 chunk 展平并物化；归并又把输出完整收集为新的 `Vec<Row>`。因此峰值内存不只由 tracker 中已记账的输入决定，扩展大数据路径时需关注排序临时向量、归并输出以及 spill helper 接管之间的重叠。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/executor/sortexec/parallel_sort_worker.go`，保留了 worker、本地有序 run、多路归并、20,000 次比较检查信号以及内存追踪这些核心语义，但不是逐字段等价移植。

- Go worker 自己持有 `chunkChannel`、等待组、结果/结束通道，并由 `run`/`fetchChunksAndSort` 循环拉取数据；Rust 把分发循环放进 `SortExec::fetchParallel`，本 worker 是受 mutex 保护的同步状态对象。
- Go `maxSortedRowsLimit = maxChunkSize * 30`，在拉取期间达到阈值即 `sortBatchRows`，因此能产生多个本地 run；Rust 上游传 `maxChunkSize * 8`，但 `saveChunk` 不触发分批，`sortLocal` 的现有条件又会一次 flush 全部非空缓存。当前 Rust 阈值没有 Go 的分批限制效果。
- Go 的 `keyColumnsLess` 调用 `SQLKiller::HandleSignal` 并通过 panic 交给 worker `run` 的 recover；Rust 用 `AtomicBool` 和私有 `SortCancelled`，在 `sortBatch` 内将该特定 panic 转成普通 `SortError`。Go failpoint 注入与随机 worker 延迟在本文件没有对应实现。
- Go `multiWayMergeLocalSortedRows` 每 100 次归并循环再次检查 SQL killer；Rust `multiWayMerge` 的取消检查只发生在本地排序比较阶段，内存多路归并本身不轮询 `killed`。
- Go 保存的是 chunk iterator，并单独维护 `rowNumInChunkIters`、`totalMemoryUsage`、`sortedRowsIter` 与 merger；Rust 直接拥有 `DataChunk`/`Vec<Row>`，并让 helper 接收归并后的向量。
- Go `reset` 使用 tracker 的 `ReplaceBytesUsed(0)` 并清 merger/iterator；Rust `reset` 释放该 worker 累积的 `memoryBytes`，更适合 tracker 被多个 worker 共享的当前设计，但不会清比较次数。

Go 测试 `parallel_sort_test.go::TestParallelSort` 验证并行排序正确性并启用信号检查 failpoint；`TestFailpoint` 覆盖随机错误。`parallel_sort_spill_test.go::TestParallelSortSpillDisk` 覆盖内存与落盘组合。Rust 的直接测试 `parallel_sort_worker_test.rs::cancellation_raised_during_comparison_interrupts_sort` 验证比较期间置 kill 后确实返回中断错误；`parallel_sort_test.rs` 和 `parallel_sort_spill_test.rs` 另有执行器级全局排序、复用、错误传播与 spill 结果测试。Go 的通道调度、failpoint 和归并阶段取消不能由当前 Rust 直接测试推定为已支持。

## 扩展指南

- 若要恢复真正的批量本地排序，应在 `saveChunk` 后或上游分发点根据累计行数调用 `sortBatch`，并明确 `maxRowsInBatch` 是硬阈值还是软阈值；不要只修改 `sortLocal` 条件。同步扩展独立的 `parallel_sort_worker_test.rs`，验证跨多个 run 的 `multiWayMerge` 顺序和阈值边界。
- 若要增强取消语义，最直接的接入点是 `sortBatch` 的比较检查和 `multiWayMerge` 的收集循环。后者目前通过一次 `collect()` 完成；若加入周期检查，应保证中断后的 run 所有权和内存账本可恢复，并增加归并阶段取消测试。
- 若要调整内存追踪，必须同时维护 `saveChunk` 的 consume、`multiWayMerge`/`reset` 的 release 和失败路径。尤其不要让 `releaseMemory` 单独减少 tracker 而不更新 `memoryBytes`。应在独立测试中断言成功、取消、归并错误与 reset 后的 `bytes_consumed()`。
- 若改变比较行为，优先在 `sort_util.rs` 的比较器构造处修改 SQL 键语义；本文件只负责调用比较器。测试应同时覆盖本地排序和多路归并，避免比较器不满足全序导致堆与排序结果不一致。
- 若引入真实并发 worker 循环，线程、通道关闭、panic 传播和锁顺序应放在明确的调度层；本类型仍应保持单 worker 状态边界。Go 的 `run`/`fetchChunksAndSortImpl` 可作为行为参考，但不能省略 Rust 当前 spill helper 的 `Arc<Mutex<_>>` 协议。
- Rust 单元测试继续放在同目录独立文件 `parallel_sort_worker_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] mod parallel_sort_worker_test;` 接入，不应内嵌到生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/executor/sortexec` 找到目标 Rust/Go 文件与独立测试；`node --file pkg/executor/sortexec/parallel_sort_worker.rs --offset 1 --limit 500` 展示目标文件 132 行及文件级使用者。
- RustCodeGraph 符号/边查询：`query parallelSortWorker`、`query saveChunk`、`query sortBatch`、`query sortLocal`、`query multiWayMerge`、`query newMultiWayMerger`、`query memorySource`；`callees multiWayMerge` 确认到 `sortLocal` 和 `newMultiWayMerger`，`callees sortBatch` 确认检查点与 `SortError`。精细 caller 查询未返回结果的部分已用源码调用点补证，没有据此扩大结论。
- 目标源码：`pkg/executor/sortexec/parallel_sort_worker.rs`，核对全部常量、类型、字段和方法。
- crate 与模块边界：`pkg/executor/sortexec/Cargo.toml`、`pkg/executor/sortexec/lib.rs`、上层 `pkg/executor/Cargo.toml`。
- Rust 直接上下游：`pkg/executor/sortexec/sort.rs`、`parallel_sort_spill_helper.rs`、`multi_way_merge.rs`、`sort_util.rs`。
- Rust 测试：`pkg/executor/sortexec/parallel_sort_worker_test.rs`；并参考执行器级 `parallel_sort_test.rs`、`parallel_sort_spill_test.rs` 和 helper 测试 `parallel_sort_spill_helper_test.rs`。
- Go 对照与测试：`pkg/executor/sortexec/parallel_sort_worker.go`、`parallel_sort_test.go`、`parallel_sort_spill_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的命令校验目标文档存在且恰有 11 个固定二级标题，并人工复核上述职责、流程、差异和扩展风险均可回溯到列出的符号或文件。

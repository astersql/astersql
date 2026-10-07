# `pkg/executor/sortexec/topn.rs`

## 文件定位

对应源码：[topn.rs](./topn.rs)。本文件属于 `astersql-executor-sortexec` crate；crate 入口 `pkg/executor/sortexec/lib.rs` 将 `topn` 声明为公开模块，并重导出 `Limit`、`RankInfo`、`TopNExec`。它为 Rust 执行层提供 `ORDER BY ... LIMIT offset, count` 的物理 TopN 算子：上游 `pkg/executor/physical_plan_runtime.rs::execute_node` 遇到 `PhysicalTopN` 时构造它，再由 `drain_topn` 反复调用 `Next` 排空结果。

`pkg/executor/sortexec/Cargo.toml` 把该目录定义为独立 crate，`[lib]` 指向 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/executor/sortexec"` 声明 Go 对照包。其完整依赖目前仅配置在 `cfg(windows)` 下；本文件自身直接依赖同 crate 的 `multi_way_merge`、`sort`、`sort_util`、`topn_chunk_heap`、`topn_spill`、`topn_worker` 和 Rust 标准库。

## 核心职责

- 用 `TopNExec::new` 把子执行器、排序键、LIMIT 窗口、可选 Rank 信息和资源参数组装为可拉取的执行器。
- 普通路径 `fetchTopN` 将输入 chunk 轮询分给多个有界堆，每个堆最多保留 `Offset + Count` 个局部最优候选；达到内存阈值时导出有序 run，最终通过 `newMultiWayMerger` 做全局归并。
- Rank 路径 `fetchRankTopN` 当前全量读取并整体排序；`RankInfo` 仅作为选择该路径的开关，`prefixKeys` 与 `expectedCount` 在本文件中尚未参与候选扩展或截断计算。
- `applyLimitAndRank` 统一、严格地应用 `Offset/Count`，因此 Rank 路径也不会输出超出 SQL LIMIT 窗口的行。
- `Next` 负责惰性物化、分页输出和排序后的内联投影；`Close` 负责清理 worker、结果、spill 状态与 tracker。

## 主要符号

- `pub struct Limit { Offset, Count }`：SQL LIMIT 的零基偏移与最大输出行数；两字段均为 `usize`。
- `pub struct RankInfo { prefixKeys, expectedCount }`：为 Rank TopN 预留的前缀排序键和候选规模。当前运行逻辑只检查 `Option<RankInfo>` 是否存在，并未读取两个字段。
- `pub struct TopNExec`：核心状态机。公开字段只有 `limit`；其余字段保存子 `RowSource`、排序键/比较器、可选 Rank/投影配置、worker 与 tracker、spill helper、已物化结果和生命周期标志。
- `TopNExec::new(...) -> Self`：创建共享比较器、内存/磁盘 tracker、kill 标志及至少一个 worker。传入的 `concurrency == 0` 和 `max_chunk_size == 0` 都会被提升到 1；堆容量用 `Offset.saturating_add(Count)` 防止整数溢出。
- `SetColumnIdxsUsedByChild`：配置输出列下标。排序始终观察完整子行，投影仅发生在 `Next` 弹出最终行时。
- `Open`：幂等打开；先拒绝空 `byItems`，再调用 `child.open()`。
- `fetchTopN`、`fetchRankTopN`、`fetch`：分别实现普通 TopN、当前 Rank 路径以及只执行一次的惰性分派。
- `applyLimitAndRank`：处理 offset 越界、饱和加法和末尾截断。
- `Next(max_rows)`：自动 `Open`/`fetch`，单次最多返回 `max(max_rows, 1)` 与 `maxChunkSize` 的较小值；结果耗尽时返回空 chunk。
- `Close`、`Kill`：清理或取消执行。`IsSpillTriggered`、`GetMemTracker`、`GetDiskTracker`、`WorkerConcurrency`、`IsKilled` 提供状态观测。

本文件没有模块级常量、trait、条件编译项或内嵌测试。

## 执行流程

1. 构造阶段：`new` 从 `by_items` 生成 `RowComparator`，创建共享 tracker/kill flag，并为每个逻辑 worker 创建容量为 `Offset + Count` 的 `topNChunkHeap`。
2. 首次拉取：`Next` 调用 `Open`；空排序键在读取 child 前返回错误。随后 `fetch` 只在 `fetched == false` 时物化结果。
3. 普通 TopN：`fetchTopN` 若 `Count == 0` 立即返回且不读取输入；否则循环 `child.next()`，按 chunk 序号模 worker 数顺序调用 `topNWorker::run`。worker 逐行更新“最差候选在堆顶”的有界堆。
4. Spill：每处理一个 chunk 后检查 `memTracker.exceeded()`。首次超限才创建 `topNSpillHelper`；成功把状态从 `notSpilled` 改为 `needSpill` 的路径同步调用 `spill()`，将各 worker 堆排好序并导出 `DiskRun`。
5. 全局归并：输入耗尽后，`takeRuns` 取出先前 runs；各 worker 仍留在内存中的堆通过 `sortedRows` 形成附加 run。`diskSource` 与 `newMultiWayMerger` 合并所有有序 run，再把候选截到 `Offset + Count`。
6. Rank 路径：只要 `rankInfo.is_some()`，`fetchRankTopN` 就全量收集 child 行并按完整 `compare` 排序；当前没有使用 `RankInfo` 字段做 Go 版的前缀边界扫描。
7. 最终窗口与输出：`applyLimitAndRank` 切出 `[Offset, Offset + Count)`；`Next` 从 `VecDeque` 分页弹出，必要时按 `columnIdxsUsedByChild` 重排/裁剪列。
8. 关闭：`Close` 逐个锁定 worker 并 `reset`，清空结果和 helper，释放磁盘 tracker 当前计数，复位 `fetched/opened`，最后关闭 child。

## 数据与状态

`child` 是输入所有权边界，类型为 `Box<dyn RowSource>`。`byItems` 保留构造排序规则，`compare` 是可共享的比较闭包。普通路径的核心不变量是每个 `topNChunkHeap` 至多保留 `Offset + Count` 行；多个 worker 的局部候选集合包含全局 TopN 所需候选，最后必须归并才能建立全局顺序。

`workers` 是 `Vec<Arc<Mutex<topNWorker>>>`；worker 共享 `memTracker` 与 `killed`，但各自拥有独立堆。`spillHelper` 惰性创建并持有已导出的 runs。需要特别注意：`DiskRun` 名义上表示磁盘 run，但 `pkg/executor/sortexec/sort_util.rs::DiskRun` 当前实际把 `DataChunk` 保存在内存向量中，`diskTracker` 记录的是这些 run 的估算大小，不是真实临时文件 I/O。

`result: VecDeque<Row>` 是一次性物化后的输出队列；`fetched` 保证 child 只被完整处理一次，`opened` 保证 child 只被打开一次。`Close` 将二者复位，因此实例在 child 自身支持再次打开的前提下可重新进入生命周期。`rankInfo` 的两个字段当前是未消费状态，不能据此推断已实现 Go 的前缀 Rank 优化。

## 依赖与调用关系

已由 RustCodeGraph 索引确认的关键边包括：

- 上游：`pkg/executor/physical_plan_runtime.rs::execute_node -> TopNExec::new -> drain_topn -> TopNExec::Next`；测试与基准也直接构造并调用 `TopNExec`，包括 `executor_required_rows_test.rs`、`sortexec_pkg_test.rs`、`topn_spill_test.rs`、`rank_topn_test.rs` 和 `benchmark_test.rs`。
- 本文件内部：`Next -> Open -> fetch`；`fetch -> fetchTopN | fetchRankTopN -> applyLimitAndRank`；`fetchTopN -> child.next / topNWorker::run / topNSpillHelper::spill / newMultiWayMerger(...).collect`。
- 下游数据结构：`topNWorker::run -> topNChunkHeap::processChk`；堆以 `SortKey`/`RowComparator` 判断候选优劣并通过 `MemoryTracker` 登记增量。
- Spill：`topNSpillHelper::spill -> spillHeap -> topNChunkHeap::drainSorted -> DiskRun::add`；状态使用原子整数，worker 访问通过互斥锁串行化。
- 模块出口：`pkg/executor/sortexec/lib.rs` 重导出 `TopNExec`、`Limit`、`RankInfo`，使执行层无需依赖内部文件路径。

RustCodeGraph 对常见名称（尤其 `Next`）会混入其他语言/模块的同名符号，因此上游结论同时用文件路径限定的索引结果与 `physical_plan_runtime.rs` 源码接线交叉核对。

## 错误处理与边界

- `Open` 在 `byItems.is_empty()` 时返回 `SortError("topn requires at least one ordering item")`，且不会打开/读取 child。
- child 的 `open/next/close`、run 写入和多路归并错误均以 `Result` 和 `?` 原样向上传播。
- worker 或 spill 相关 `Mutex` 中毒时转换为带上下文的 `SortError`；不会 panic 解锁。
- `Count == 0` 时普通路径不读取 child、不 spill；Rank 路径当前仍会因先按 `rankInfo` 分派而全量读取和排序，之后输出为空，这是与普通路径不同的成本边界。
- `Offset >= rows.len()` 返回空结果；`Offset + Count` 使用饱和加法，切片终点再限制到实际长度。
- 投影列越界在 `Next` 返回包含列下标和实际行宽的错误。此前已经弹出的该行不会重新放回队列，因此错误后的继续拉取不保证重试该行。
- `Kill` 只设置共享原子标志；`topNWorker::run` 在处理每个 chunk 前检查。Rank 路径不经过 worker，普通路径完成输入后和结果排空阶段也不再次检查，所以它不是对任意阶段都立即生效的强取消。
- `Close` 会尝试关闭 child；重复调用仍再次调用 `child.close()`，仓库测试使用的 `VecRowSource` 支持这一行为，但通用幂等性最终取决于具体 child 实现。

## 并发与资源生命周期

`concurrency` 决定独立 worker/堆的数量，但当前 `fetchTopN` 在调用线程中按 chunk 轮询、同步执行 `worker.lock()?.run(chunk)`；本文件没有创建线程、任务或 channel。因此这里是“多分片堆”而非 Go 版真正的并行 worker 管线。`Arc<Mutex<_>>`、原子 tracker 与原子 kill/status 为共享和未来并发接线提供同步边界。

内存生命周期由每个 worker 在堆变化后向共享 `memTracker` 登记差量；spill 时 `drainSorted` 清空堆，helper 归还行估算内存并向 `diskTracker` 登记 run 大小。最终归并会消费 runs，但 tracker 不随 `takeRuns` 自动下降；`Close` 明确释放全部磁盘计数。worker 内存则由 `reset` 清堆并归还剩余计数。

spill 状态转换为 `notSpilled -> needSpill -> inSpilling -> notSpilled`。当前执行器在发现超限后同步 spill，没有后台 spill 与等待条件变量。`killed` 使用 Release 写和 Acquire 读，保证 worker 观察取消状态；`opened/fetched/result/spillHelper` 仅由 `&mut self` 路径修改。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/executor/sortexec/topn.go`。两版共同保留的主语义是：`TopNExec` 对应 `ORDER BY + LIMIT`；排序期间使用完整 child 行；内联投影只作用于最终输出；普通 TopN 使用容量为 `Offset + Count` 的堆；内存压力可触发 spill；多 run 通过多路归并得到全局顺序；最终严格执行 OFFSET/COUNT。

当前 Rust 版是较小的同步实现，以下差异不能当作已经对齐：

- Go `OpenSelf` 建立 channel、WaitGroup、多个 goroutine worker、语句级 tracker/fallback action；Rust 只建立多个受锁保护的堆并在调用线程轮询处理。
- Go 分“初始装载/建堆”和“更新堆”两个阶段，并记录各阶段 spill；Rust 每个 chunk 后统一判断 tracker，立即同步 spill。
- Go 的 Rank TopN 使用截断表达式、字符前缀、collator、前缀键和有序输入边界来减少读取；Rust `RankInfo` 只有 `SortKey`/数量字段，且当前两字段未使用，实际为全量排序。
- Go 的 `DiskRun` 路径对应真实临时存储与异步资源协调；Rust 当前 `DiskRun` 是内存 chunk 容器。
- Go 通过 result channel 流式产生结果并包含 panic 恢复；Rust 首次 `Next` 完整物化到 `VecDeque`，没有 panic 转错误层。

因此，Rust 当前能提供仓库已有测试覆盖的排序、窗口、投影和内存模拟 spill 行为，但不能声称具有 Go 版相同的并发度、Rank 前缀优化、真实磁盘落盘或语句上下文集成。

## 扩展指南

- 若补齐 Rank TopN，优先修改 `RankInfo`、`fetchRankTopN` 与 `fetch`，明确输入是否要求按前缀预排序，并实现/验证前缀等价（尤其字符串 collation、NULL 和字符前缀）；同步扩展独立文件 `pkg/executor/sortexec/rank_topn_test.rs`，不能只让现有“最终 LIMIT 正确”用例通过。
- 若引入真正并行 worker，应围绕 `fetchTopN`、`topNWorker` 与 `topNSpillHelper` 设计明确的停止、错误汇聚、channel 关闭和 join 顺序；同时保持 `Close` 在部分启动、错误、kill 和重复关闭时不泄漏资源。
- 若把 `DiskRun` 改为真实落盘，实际实现位置主要在 `sort_util.rs::DiskRun`、`topn_spill.rs` 和 `multi_way_merge.rs`；必须同步验证临时文件删除、读取错误、tracker 准确性及中途取消。
- 若改变 LIMIT/分页逻辑，修改 `applyLimitAndRank` 或 `Next` 时应同步 `sortexec_pkg_test.rs`、`topn_spill_test.rs`、`rank_topn_test.rs` 和 `pkg/executor/executor_required_rows_test.rs`，覆盖 count=0、offset 越界、溢出、空输入、多页与 spill。
- 若扩展内联投影，必须保持“先按完整行排序、后投影”的不变量，并为列越界与列重排补充独立测试；不要把测试嵌入本生产文件。
- 若调整取消语义，需决定 Rank、归并和结果排空阶段的检查点，并在测试中证明错误传播与 tracker/child 清理，而非仅观察 `IsKilled`。

兼容风险集中在排序比较（NULL、DESC、混合类型）、Rank 前缀等价、LIMIT 窗口和投影列映射；性能风险集中在 Rank 全量排序、首次 `Next` 全量物化、多 worker 实际串行、run/行克隆以及“磁盘”run 仍占内存。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且覆盖 `pkg/executor/sortexec/topn.rs`；`files --filter pkg/executor/sortexec` 列出该模块 43 个已索引文件；`node --file` 完整读取 `topn.rs`、`topn_worker.rs`、`topn_spill.rs`、`topn_chunk_heap.rs`、`sort_util.rs`；限定 `TopNExec::Next`/`fetchTopN` 的 `explore` 给出 `child.next -> fetchTopN/fetchRankTopN`、`fetch -> fetchTopN` 以及测试到 `Next` 的调用边。
- crate/入口：`pkg/executor/sortexec/Cargo.toml`、`pkg/executor/sortexec/lib.rs`。
- 应用接线：`pkg/executor/physical_plan_runtime.rs` 中 `PhysicalTopN -> TopNExec::new -> drain_topn -> Next`。
- Go 对照：`pkg/executor/sortexec/topn.go` 的 `TopNExec`、`OpenSelf`、`Next`、`fetchChunks`、Rank 前缀处理、spill 两阶段执行和结果生成。
- Rust 独立测试：`pkg/executor/sortexec/sortexec_pkg_test.rs` 覆盖 count=0、offset/count、后置投影、空输入、spill tracker 清理和重复关闭；`topn_spill_test.rs` 覆盖 spill 后窗口；`rank_topn_test.rs` 覆盖 Rank 路径仍严格应用 LIMIT；`pkg/executor/executor_required_rows_test.rs` 覆盖分页；`pkg/executor/benchmark_test.rs` 对比内联与外层投影。
- 人工复核结论：本文件存在于物理 TopN 接线中；普通路径依靠局部有界堆、同步 spill 与全局归并运行；安全扩展必须保留完整行排序、最终 LIMIT、错误传播和 Close 资源归还，并明确处理当前与 Go 的并发/Rank/磁盘差距。

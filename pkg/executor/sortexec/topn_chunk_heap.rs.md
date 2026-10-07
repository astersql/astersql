# `pkg/executor/sortexec/topn_chunk_heap.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-executor-sortexec`；该包由 `pkg/executor/sortexec/Cargo.toml` 定义为以 `lib.rs` 为入口的库，并作为根工作区成员注册。`pkg/executor/sortexec/lib.rs` 公开声明 `topn_chunk_heap` 模块，因此外部代码技术上可通过模块路径访问公开类型；不过包根没有重新导出该类型，仓库内实际调用也都把 `topNChunkHeap` 当作排序执行器的实现部件，而不是 `sortexec` 门面的主要类型。

在普通（非 Rank）TopN 主链中，它位于输入 chunk 与最终归并之间：`TopNExec::new` 为每个 `topNWorker` 创建一个堆，worker 将输入交给堆筛选，`TopNExec::fetchTopN` 再把各堆的有序候选与磁盘 run 合并。Rank 路径 `TopNExec::fetchRankTopN` 当前直接收集并排序全部行，不经过本文件。

## 核心职责

`topNChunkHeap` 以 `totalLimit = Offset + Count` 为容量，仅保留比较器定义下最优的至多 `totalLimit` 行。内部采用“最差候选位于根节点”的大根堆：堆未满时插入，堆满后只用严格优于根节点的新行替换根节点。这样，扫描完一个 worker 的输入后，留下的正是该 worker 的局部 TopN 候选，空间复杂度按候选行数而不是输入总行数增长（`topNChunkHeap::update`）。

它还负责维护候选行的估算内存总量，并提供两种有序导出方式：不破坏堆的 `sortedRows` 用于正常汇总，清空堆的 `drainSorted` 用于 spill。文件自身不负责 SQL LIMIT 的 offset 截取、跨 worker 的全局归并、磁盘写入、取消检查或内存配额决策；这些分别位于 `topn.rs`、`topn_spill.rs` 和 `topn_worker.rs`。

## 主要符号

- `topNChunkHeap`：唯一的模块级类型；字段均为私有。`rows: Vec<Row>` 保存候选，`totalLimit` 是候选上限，`compare: RowComparator` 决定完整排序语义，`memoryUsage` 汇总当前候选行的 `Row::memory_usage()`。
- `new(total_limit, compare)`：公开构造函数，按上限预留 `Vec` 容量，但初始行数与登记内存均为零。
- `worse(a, b)`：私有比较辅助；当 `rows[a]` 在比较器下大于 `rows[b]` 时返回真。
- `siftUp(i)` / `siftDown(i)`：私有二叉堆修复操作。前者用于尾部插入，后者用于根节点替换。
- `update(row)`：单行筛选入口。容量为零时直接丢弃；未满时加入并上浮；已满且新行严格更优时替换根并下沉。
- `processChk(chk)`：公开批处理入口，消费 `DataChunk.rows` 并逐行调用 `update`。
- `isFull()` / `len()` / `memoryUsage()`：公开只读查询。仓库搜索显示 `memoryUsage` 被 worker 使用，而 `isFull`、`len` 当前没有 Rust 调用者。
- `clear()`：清空候选并把登记内存归零，返回清空前的数值供上层 tracker 释放。
- `sortedRows()`：克隆候选并按比较器升序排序，不改变堆和内存登记。
- `drainSorted()`：取走候选、升序排序并将登记内存归零；这是 spill 的所有权转移入口。
- `compact()`：以 `max(totalLimit, rows.len())` 为最低容量调用 `Vec::shrink_to`；当前没有 Rust 调用者。

本文件没有 trait、enum、模块级常量、自由函数或条件编译项。命名沿用 Go 风格；`lib.rs` 为 crate 统一允许了 `non_snake_case` 和 `non_camel_case_types`。

## 执行流程

1. `TopNExec::new` 在 `pkg/executor/sortexec/topn.rs` 中创建共享比较器，并为每个 worker 构造容量为 `limit.Offset.saturating_add(limit.Count)` 的 `topNChunkHeap`。饱和加法避免 offset/count 加法溢出。
2. `TopNExec::fetchTopN` 轮询把输入 `DataChunk` 交给各 worker；`topNWorker::run` 先读取旧的 `memoryUsage`，然后调用 `processChk`。
3. `processChk` 取得 chunk 中每一行的所有权并调用 `update`。堆未满时，行从数组尾部通过 `siftUp` 上浮；堆已满时，只有 `compare(new, root).is_lt()` 才替换当前最差候选，并由 `siftDown(0)` 恢复大根堆性质。相等行不会替换，因此同键候选的具体保留者取决于到达顺序。
4. worker 用新旧 `memoryUsage` 的差额更新共享 `MemoryTracker`。若上层判断超限，`topNSpillHelper::spillHeap` 锁住 worker 并调用 `drainSorted`，再按 `spillChunkSize` 写成 `DiskRun`。
5. 未 spill 的结束路径由 `TopNExec::fetchTopN` 调用每个堆的 `sortedRows`。这些升序候选与已 spill 的 run 经 `newMultiWayMerger` 做全局归并，然后才由 `TopNExec` 截断到 `Offset + Count` 并应用最终 offset/count。
6. `TopNExec::Close` 调用 `topNWorker::reset`；后者通过 `clear` 取得并释放剩余登记内存。

## 数据与状态

核心不变量是：当 `totalLimit > 0` 时，`rows.len() <= totalLimit`，且非空堆的 `rows[0]` 是当前候选中的最差行。`siftUp` 比较子节点与父节点，`siftDown` 先选择两个子节点中更差的一个再与父节点比较，从而维持这一性质。

`memoryUsage` 只累计 `Row::memory_usage()` 的估算值；它不包含 `Vec<Row>` 自身的容量分配、比较器 `Arc` 或结构体开销。替换根节点时按“新行估算值减旧根估算值”更新，因此数值可以下降。`sortedRows` 克隆所有行，产生的临时副本内存没有计入该字段；`drainSorted` 通过 `mem::take` 转移原 `Vec`，随后直接把字段归零。

容量为零是显式边界：`update` 不访问根节点并立即返回，`processChk` 因而安全丢弃所有行。此时 `isFull()` 按 `len >= totalLimit` 返回真。正常 `TopNExec` 在 `Count == 0` 时更早从 `fetchTopN` 返回，不读取输入；该行为由 `sortexec_pkg_test.rs::topn_count_zero_does_not_read_or_spill_input` 覆盖。

## 依赖与调用关系

直接依赖全部来自 `crate::sort_util`：`Row` 提供值与内存估算，`DataChunk` 提供批量行所有权，`RowComparator` 是可跨线程共享的 `Arc<dyn Fn(&Row, &Row) -> Ordering + Send + Sync>`。实际 SQL 排序键、升降序和 NULL 位置由 `sort_util.rs::comparator/compare_rows` 封装，本文件只消费最终 `Ordering`。

直接上游调用边如下：

- `topn.rs::TopNExec::new → topNChunkHeap::new`；
- `topn_worker.rs::topNWorker::run → memoryUsage → processChk → update`；
- `topn.rs::TopNExec::fetchTopN → sortedRows`；
- `topn_spill.rs::topNSpillHelper::spillHeap → drainSorted`；
- `topn_worker.rs::topNWorker::reset → clear`。

RustCodeGraph 对 `update` 给出的内部边为 `processChk → update → siftUp/siftDown`，并识别 `topn.rs` 与 `topn_spill_test.rs` 对类型的导入。图索引没有完整列出若干方法调用边，因此又用 `rg` 对上述 `topn_worker.rs`、`topn_spill.rs` 和 `topn.rs` 的直接调用作了源码核对。Cargo 层面，`pkg/executor/sortexec/Cargo.toml` 将本目录定义为独立 crate；当前依赖表位于 `cfg(windows)` target 下，但本文件自身没有平台条件分支。

## 错误处理与边界

本文件所有方法都不返回 `Result`，没有主动构造错误。容量为零、空 chunk、空堆排序和重复调用 `clear`/`drainSorted` 都按空操作处理。堆满后，不优于根节点的行被直接丢弃；比较结果相等也不会替换。

安全性依赖调用方提供语义一致、近似全序的比较器。若比较闭包 panic，或比较关系不满足排序所需的一致性，panic/错误顺序不会在本文件被捕获。`sortedRows` 与 `drainSorted` 使用标准库 `sort_by`；下标访问集中在已检查的堆结构路径中，零容量由 `update` 的首个分支挡住。

取消、锁中毒和磁盘错误不在这里处理：worker 只在处理整个 chunk 前检查 `killed`，mutex 锁中毒由 `topn.rs`/`topn_spill.rs` 转换成 `SortError`，`DiskRun::add` 的失败也由 spill helper 传播。因此一个很大的 `processChk` 当前没有逐行取消检查。

## 并发与资源生命周期

`topNChunkHeap` 内部没有锁，也不自行创建线程或任务。每个 worker 独占一个堆；共享访问时，上层以 `Arc<Mutex<topNWorker>>` 包装 worker。`RowComparator` 本身要求 `Send + Sync`，但堆的可变状态仍必须由 worker/mutex 串行保护。

候选行的正常生命周期为 `new → processChk/update → sortedRows → clear`。spill 生命周期为 `new → processChk/update → drainSorted`；`drainSorted` 将行所有权转交给 spill helper，并把堆置空。`clear` 保留 `Vec` 容量以便复用；`compact` 可尝试收缩容量，但不会收缩到低于 `totalLimit`，且当前主链未调用它。`sortedRows` 不破坏原堆，因此后续 `Close/reset` 仍负责释放其内存 tracker 计数。

内存账本的所有权分为两层：堆只维护行估算值，worker 将差额消费到共享 tracker；spill helper 在 `drainSorted` 后按导出行重新求和并释放 tracker，`Close` 则通过 `clear` 释放仍在内存中的值。修改这些方法时必须同步审查 tracker 是否恰好消费和释放一次。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/sortexec/topn_chunk_heap.go`。两者共享的语义核心是：容量为 offset+count、堆顶为最差候选、只有更优新行才替换堆顶，最终按比较器顺序输出。

数据布局并非机械一一对应。Go 的 `topNChunkHeap` 用 `chunk.List` 保存行值、用 `[]chunk.RowPtr` 组织堆，并实现 `container/heap.Interface` 的 `Len/Less/Swap/Push/Pop`；被淘汰行仍可能留在 `rowChunks`，所以 Go 的 `doCompaction` 会重建 chunks 和指针以释放极端输入下的内存。Rust 直接把拥有所有权的 `Row` 放进 `Vec`，替换根会立即丢弃旧行，因此 `compact` 只处理 `Vec` 预留容量，且当前未接线。

Go 会先装载至少 `totalLimit` 行、初始化 row pointers，再 `heap.Init` 并弹出超额行；Rust 的 `update` 从第一行开始增量建堆。Go spill 会排序指针并分块写盘，且写盘循环每 100 行检查 SQL killer；Rust 由 `drainSorted` 排序 owned rows，分块和错误传播在 `topn_spill.rs`，目前没有等价的逐行/分块取消检查。Go 的 Rank TopN 与 heap 主链有更紧密的前缀加载逻辑，而当前 Rust `fetchRankTopN` 绕开本堆做全量排序。这些是现状差异，不应把 Go 的 compaction、kill 检查或 Rank 行为视为本文件已支持。

## 扩展指南

- 修改候选选择或稳定性时，入口是 `update`，并应增加独立 Rust 测试，覆盖升序/降序、相等键、容量 0/1、输入少于/等于/多于上限；不要把测试内嵌回生产文件。当前最近的独立测试文件是 `pkg/executor/sortexec/sortexec_pkg_test.rs` 和 `topn_spill_test.rs`。
- 修改堆算法时，应直接验证每次插入/替换后的“根为最差候选”不变量，并覆盖 `siftUp` 的左右层级和 `siftDown` 选择左右子节点的分支。复杂度目标应保持单行更新 `O(log totalLimit)`、常驻候选 `O(totalLimit)`。
- 修改内存估算时，必须同时检查 `Row::memory_usage`、`topNWorker::run/reset` 与 `topNSpillHelper::spillHeap`；尤其注意 `sortedRows` 的克隆峰值目前不进 tracker，以及 `drainSorted` 清零与 helper 释放之间的职责配对。
- 修改有序导出时，须保持 `sortedRows` 非破坏、`drainSorted` 转移所有权的区别，并同步验证 `topn_spill_test.rs::topn_spill_returns_only_requested_offset_window`、`sortexec_pkg_test.rs::topn_spill_releases_trackers_on_close` 和 `close_after_topn_spill_is_idempotent`。
- 若要对齐 Go 的 compaction、spill 中断或 Rank TopN，修改范围会跨越 `topn_chunk_heap.rs`、`topn_worker.rs`、`topn_spill.rs`、`topn.rs`；应以 Go 的 `doCompaction`、`spillHeap`、Rank 加载路径及 `topn_spill_test.go` 为依据，不能只扩展本类型的表面 API。
- `isFull`、`len`、`compact` 当前无调用者；新增调用前先确认其零容量语义和 tracker 影响，若没有保留它们的迁移目的，则不要假设它们已经参与运行主链。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/executor/sortexec/topn_chunk_heap.rs` 确认目标文件已索引；`node --file ... --offset 1 --limit 500` 读取完整 126 行和 15 个符号；`query topNChunkHeap/processChk/drainSorted` 及精确 `node` 查询确认类型、`processChk → update → siftUp/siftDown` 内部边。精确 callers 查询未返回完整方法边，因此没有据此臆造调用关系。
- Rust 源码：目标 `topn_chunk_heap.rs`；模块声明 `sortexec/lib.rs`；上游 `topn.rs`、`topn_worker.rs`、`topn_spill.rs`；基础类型与比较语义 `sort_util.rs`。`rg` 直接核对了 `new/processChk/memoryUsage/clear/sortedRows/drainSorted` 的调用点，并确认 `isFull/len/compact` 没有目标文件外的 Rust 调用。
- Cargo：`pkg/executor/sortexec/Cargo.toml` 核对包名、`lib.rs` 入口、Go 包元数据和 Windows target 依赖；根 `Cargo.toml` 核对 workspace member 与 facade 路径登记。
- Go 对照：`pkg/executor/sortexec/topn_chunk_heap.go`；调用与 spill 语义另核对 `topn.go`、`topn_spill.go`。相关 Go 回归位于 `topn_spill_test.go`（内存/多阶段 spill、失败注入、kill）和 `rank_topn_test.go`。
- Rust 测试：`topn_spill_test.rs` 直接构造 `topNChunkHeap` 并覆盖 spill/offset 窗口；`sortexec_pkg_test.rs` 覆盖 offset/count、投影、空输入、count=0、spill 后 tracker 释放及重复 Close；`rank_topn_test.rs` 证明当前 Rank 路径最终仍严格应用 LIMIT。仓库中未发现专门逐方法测试 `siftUp/siftDown/update` 的同名独立测试文件，这是后续修改堆逻辑时最直接的测试缺口。
- 本任务为只新增说明文档的分析任务，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰好包含 11 个固定二级章节，并人工复核没有把 Go 能力写成 Rust 已有行为。

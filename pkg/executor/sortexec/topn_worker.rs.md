# `pkg/executor/sortexec/topn_worker.rs`

## 文件定位

源文件为 [`topn_worker.rs`](./topn_worker.rs)。本文件属于 `astersql-executor-sortexec` crate；crate 根 `pkg/executor/sortexec/lib.rs` 以 `pub mod topn_worker` 装配它，但没有在 crate 根重新导出 `topNWorker`。因此它是排序执行器内部的 TopN 工作单元，而不是面向其他 crate 的稳定入口。`pkg/executor/sortexec/Cargo.toml` 的 `[package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/executor/sortexec`；当前依赖仅在 `cfg(windows)` 下声明，说明这套 Rust 移植目前受平台条件约束。

上层 `TopNExec` 在 `pkg/executor/sortexec/topn.rs` 中创建 worker、分发输入 chunk、汇总各 worker 的堆，并在需要时交给 `topNSpillHelper` 落盘。本文件只负责“单个 chunk 进入单个有界堆”这一局部步骤。

## 核心职责

- `topNWorker::run` 在处理 chunk 前检查共享取消标志，统计输入行数，把 chunk 交给 `topNChunkHeap::processChk`，并把堆占用的净变化登记到共享 `MemoryTracker`。
- `topNWorker::reset` 清空堆、归还仍由该堆持有的内存记账，并清零累计处理行数。
- worker 自身不决定排序规则、TopN 容量、spill 时机或最终 `Offset/Count` 截取；这些分别由构造时传入的 `topNChunkHeap`、`TopNExec::fetchTopN`、`topNSpillHelper` 和 `TopNExec::applyLimitAndRank` 决定。

## 主要符号

- `pub struct topNWorker`：保存 `heap: topNChunkHeap`、共享的 `Arc<AtomicBool>` 取消标志、共享的 `Arc<MemoryTracker>` 以及私有计数 `processedRows`。只有 `heap` 对 crate 外可见；类型本身虽为 `pub`，通常经 `crate::topn_worker` 在 crate 内使用。
- `pub fn new(heap, killed, tracker) -> Self`：注入已经配置好容量与比较器的堆，以及与执行器共享的取消和内存状态；计数从零开始。
- `pub fn run(&mut self, chk: DataChunk) -> Result<()>`：同步处理一个 chunk。成功表示该 chunk 的全部行已经送入有界堆并完成内存差额记账。
- `pub fn reset(&mut self)`：释放堆当前登记用量并恢复计数；不更换堆的容量/比较器，也不清除共享 `killed` 标志。
- `pub fn processedRows(&self) -> usize`：只读返回累计送入的行数。仓库搜索没有发现生产调用者或直接断言，当前是可观测性辅助接口。

文件没有模块级常量、trait、条件编译块或独立异步入口。

## 执行流程

1. `TopNExec::new`（`pkg/executor/sortexec/topn.rs`）按 `concurrency.max(1)` 创建 worker；每个 worker 得到独立 `topNChunkHeap`，容量为 `Limit.Offset.saturating_add(Limit.Count)`，但共享 `killed` 与 `memTracker`。
2. 非 Rank 路径 `TopNExec::fetchTopN` 从子执行器拉取 `DataChunk`，按轮询下标选择 worker，取得其 `Mutex` 后调用 `topNWorker::run`。
3. `run` 用 `Ordering::Acquire` 读取取消标志。若已经被 `TopNExec::Kill` 以 `Ordering::Release` 置位，立即返回 `SortError("query interrupted")`，chunk、计数和内存均不改变。
4. 未取消时，先记录 `heap.memoryUsage()`，再把 `chk.num_rows()` 加到 `processedRows`，随后由 `heap.processChk` 逐行维护“最差候选在堆顶”的容量受限大根堆。
5. 处理完成后以新旧堆用量之差调用 `tracker.consume`。堆未满时差额通常为正；替换较大行时差额也可能为负，`MemoryTracker::consume` 的原子加法会相应降低共享用量。
6. 上层在每次 `run` 后检查 `memTracker.exceeded()`，必要时调用 `topNSpillHelper::spill`。spill 通过 `heap.drainSorted()` 取走行并归还相应内存；输入结束后，上层把磁盘 run 与各 worker 剩余的 `heap.sortedRows()` 做多路归并。
7. `TopNExec::Close` 逐个锁定 worker 并调用 `reset`，然后清理结果、spill 与磁盘记账并关闭子执行器。

## 数据与状态

- `heap` 是 worker 独占的候选集合；其 `totalLimit` 和比较器由 `TopNExec::new` 配置。`processChk` 消耗传入 `DataChunk` 的所有权，容量为零时不会保留行。
- `processedRows` 统计已交给堆考察的输入行，而非当前保留行、输出行或 spill 行。即使行因堆已满且不够优而被丢弃，也计入该值；取消检查失败的 chunk 不计入。`reset` 把它清零，spill 不清零。
- `tracker` 是所有 worker 共享的原子计数器。worker 只登记堆保留数据的净变化，不登记传入 chunk 的瞬时容器开销；实际估值来自 `Row::memory_usage` 和 `topNChunkHeap::memoryUsage`。
- `killed` 同样由所有 worker 与 `TopNExec` 共享。它是粘性的：本文件及 `TopNExec::Close` 都不把它恢复为 `false`。

## 依赖与调用关系

RustCodeGraph 将本文件列为被 `pkg/executor/sortexec/topn.rs`、`topn_spill.rs`、`topn_spill_test.rs` 和 `sort_spill_test.rs` 使用。实际生产主链是：

`TopNExec::fetchTopN` → `topNWorker::run` → `topNChunkHeap::processChk` → `topNChunkHeap::update`。

资源清理主链是 `TopNExec::Close` → `topNWorker::reset` → `topNChunkHeap::clear` / `MemoryTracker::release`。spill 主链则由 `topNSpillHelper::spillHeap` 锁定相同 worker，调用其 `heap.drainSorted`，再更新共享内存和磁盘 tracker。

直接类型依赖来自 `sort_util.rs` 的 `DataChunk`、`MemoryTracker`、`Result`、`SortError`，以及 `topn_chunk_heap.rs` 的 `topNChunkHeap`；标准库依赖为 `Arc`、`AtomicBool` 和原子内存序。精确 RustCodeGraph callers/callees 命令本次没有返回边，所以上述调用关系以图的文件级 used-by 结果和具体调用点交叉核验。

## 错误处理与边界

- 唯一由 `run` 主动构造的错误是取消后的 `SortError("query interrupted")`；检查发生在修改任何 worker 状态之前。
- `topNChunkHeap::processChk` 当前不返回错误，因此一旦通过取消检查，`run` 的其余步骤没有显式失败分支。
- `processedRows += chk.num_rows()` 使用普通 `usize` 加法，没有饱和或溢出处理；正常执行规模下由平台地址空间构成实际上限，调试构建若理论上溢出会 panic。
- `reset` 的 `MemoryTracker::release` 使用饱和减法，避免共享计数降到零以下；若堆已被 spill 清空，`clear` 返回零，不会重复释放。
- worker 不处理 `Mutex` poison；锁错误由 `TopNExec` 与 `topNSpillHelper` 的调用点转换为 `SortError("topn worker lock poisoned")`。
- 本文件没有 panic 捕获。与 Go worker 的 `recover` 行为不同，Rust panic 会使外层 mutex poison，并由后续锁定路径报告错误。

## 并发与资源生命周期

尽管字段使用 `Arc`，当前 Rust `TopNExec::fetchTopN` 是同步轮询：同一调用线程依次锁定 worker 并调用 `run`，没有像 Go 版本那样为每个 worker 启动 goroutine。`Arc<Mutex<topNWorker>>` 仍允许 `TopNExec` 与 spill helper 安全共享 worker；互斥锁保证堆和 `processedRows` 的独占修改。

取消使用 Release/Acquire 配对，使 worker 在观察到 `true` 后停止接收后续 chunk。内存 tracker 内部使用 relaxed 原子计数；它提供线程安全的数值累计，但不承担其他数据的发布同步。堆内行的生命周期从 `run` 接管 chunk 开始，到被更优行替换、`drainSorted` 转入 `DiskRun`、或 `reset` 清空为止。`TopNExec::Close` 是正常归还剩余堆内存的入口；单独丢弃 worker 不会显式调用 tracker 的 `release`。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/sortexec/topn_worker.go`。两者都围绕独立 `topNChunkHeap` 处理 TopN 候选，并由 spill helper 持有 worker 列表，但实现形态有显著差异：

- Go `newTopNWorker` 注入 chunk channel、WaitGroup、错误 channel、finish channel、`TopNExec` 和测试 worker ID；Rust `new` 只注入堆、取消标志和 tracker。
- Go `run` 持续从 channel 拉取 chunk，保证每个 chunk 对 WaitGroup 调用 `Done`，退出时排空 channel，并用 `recover` 把 panic 发到错误 channel；Rust `run` 每次只同步处理调用者传入的一个 chunk，没有 channel、WaitGroup、错误 channel或 panic 恢复。
- Go 在 `fetchChunksAndProcessImpl` 中区分堆尚未装满时直接保存 chunk、装满后初始化指针堆并处理；Rust 的 `topNChunkHeap::processChk/update` 把这两条路径统一为逐行维护有界堆。
- Go 提供 `SlowSomeWorkers` 与随机失败注入，`topn_spill_test.go::TestTopNSpillDisk` 等测试验证并发 worker 变慢及 spill；Rust worker 没有对应 failpoint，Rust spill 测试验证的是同步 worker、spill 状态和结果窗口。
- Rust 额外显式检查共享 `AtomicBool` 并维护 `processedRows`；Go worker 的停止主要由 `finishChan`/SQL killer 所在的更大执行链协调，没有同名计数字段。

因此，Rust 当前保持了“每 worker 独立 TopN 候选堆、可被 spill helper 汇总”的核心语义，但没有移植 Go worker 的 goroutine 调度、channel 排空、panic 转换和 failpoint 时序测试。

## 扩展指南

- 若改变单 chunk 的接收、取消语义或记账顺序，修改 `topNWorker::run`，并在独立测试文件中增加直接 worker 测试；不要把测试内嵌到 `topn_worker.rs`。建议覆盖取消时状态不变、负内存差额、零容量堆和 `processedRows` 计数。
- 若改变候选淘汰或内存估值，应修改 `topn_chunk_heap.rs`/`sort_util.rs` 并同步验证 worker 的差额记账；避免在 worker 中复制堆算法。
- 若引入真正并行的 Rust worker，需同时设计输入 channel 的关闭、每个 chunk 的完成确认、错误/ panic 汇报、`TopNExec::Close` 等待，以及 spill 与 run 竞争同一 mutex 的策略；Go 文件可作语义参考，但不能直接假设其生命周期已存在于 Rust。
- 若允许执行器在 `Kill` 后复用，应明确在哪里把 `killed` 恢复为 `false`；仅调用现有 `reset` 不足以复用。
- 相关 Rust 回归应优先放在同目录独立测试模块：worker/spill 交互放 `topn_spill_test.rs`，执行器可见的取消、Limit 与结果行为放 `sortexec_pkg_test.rs`。涉及 Go 对齐时同步查看 `topn_spill_test.go`，但不要以 Go failpoint 测试代替 Rust 行为验证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/executor/sortexec` 确认目标及相邻实现/测试；`node --file pkg/executor/sortexec/topn_worker.rs --offset 1 --limit 260` 读取全部 55 行，并报告四个 used-by 文件；`query topNWorker` 定位 Rust/Go 类型、Go 方法和 Rust 测试构造点。精确 `callers/callees` 查询未产生输出，未据此声称符号级边。
- 源码：`pkg/executor/sortexec/topn_worker.rs`（目标类型和方法）、`topn.rs`（构造、轮询调用、spill 检查、汇总、Close/Kill）、`topn_chunk_heap.rs`（有界堆与内存变化）、`topn_spill.rs`（锁、drain、tracker 转移）、`sort_util.rs`（chunk、错误和 tracker 原子语义）、`lib.rs` 与 `Cargo.toml`（模块/crate/平台边界）。同目录未发现 `doc.go`。
- 测试：`topn_spill_test.rs::worker_with_rows` 直接构造并运行 worker；`oom_action_only_requests_spill_until_the_executor_runs_it`、`failed_spill_resets_status_for_a_future_attempt` 和 `topn_spill_returns_only_requested_offset_window` 覆盖 spill 协调及结果边界。`sortexec_pkg_test.rs` 覆盖 TopN 的 Limit/投影与其他排序 worker 的取消错误；仓库搜索未发现直接调用 `processedRows` 的测试。
- Go 对照：`topn_worker.go`、`topn.go` 的 worker 创建段，以及 `topn_spill_test.go::TestTopNSpillDisk`/`TestTopNSpillDiskFailpoint`，用于确认并发、channel、panic/failpoint 与 Rust 当前实现的差异。
- 本任务仅新增说明文档，不运行 Cargo；结构验证要求本文恰好包含任务规定的 11 个二级标题。

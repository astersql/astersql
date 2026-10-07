# `pkg/executor/sortexec/topn_spill.rs`

## 文件定位

本文件属于 `astersql-executor-sortexec` crate，由同目录 `lib.rs` 的 `pub mod topn_spill` 纳入模块树。它位于 `TopNExec` 的 worker 有界堆与最终多路归并之间：当 `topn.rs::TopNExec::fetchTopN` 发现共享内存 tracker 超限时，惰性创建 `topNSpillHelper`，把各 worker 堆导出为有序 run；输入结束后，`TopNExec` 取走这些 run，并与仍在内存中的 worker 行一起交给 `newMultiWayMerger`。

`pkg/executor/sortexec/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/executor/sortexec`，没有声明 feature，清单中的 workspace 依赖目前都位于 `cfg(windows)` 下。本文件自身只使用标准库同步原语和同 crate 的 `sort_util`、`topn_worker`。它不负责读取子执行器、维护 TopN 有界堆或应用 `LIMIT offset,count`；这些分别由 `TopNExec`、`topNWorker`/`topNChunkHeap` 和 `TopNExec::applyLimitAndRank` 完成。

## 核心职责

本文件包含两组职责：

1. `topNSpillHelper` 保存 worker、比较器、spill 状态和内存/磁盘 tracker，把每个 worker 堆的有序行按 `spillChunkSize` 分块写入 `DiskRun`，并把生成的 run 交给后续归并。
2. `topNSpillAction` 根据 tracker 是否超限以及 TopN 自身内存是否至少达到限额的十分之一，只把 helper 标记为 `needSpill`，不在回调内执行实际 spill。

这里的 `DiskRun` 是 `sort_util.rs` 中持有 `Vec<DataChunk>` 的内存结构，并不执行文件 I/O。因此“spill/磁盘”是与 Go 版相对应的接口语义；当前 Rust 文件不能据此视为已经具备真实临时文件、磁盘错误、I/O 背压或文件清理能力。

## 主要符号

- `topNSpillHelper`：公开 helper。`workers` 保存共享 worker；公开字段 `sortedRowsInDisk` 保存历次生成的 run；`compare`、原子 `status`、`tracker` 和 `diskTracker` 为内部状态。
- `topNSpillHelper::new(...) -> Self`：保存依赖，将 `status` 初始化为 `notSpilled`，run 列表初始化为空。
- `setNeedSpill(&self) -> bool`：通过 `compare_exchange(notSpilled, needSpill, AcqRel, Acquire)` 仲裁请求；只有第一个从 `notSpilled` 转换的调用返回 `true`。
- `isSpillNeeded(&self) -> bool`：以 Acquire 读取状态并判断是否为 `needSpill`。
- `spillHeap(&mut self, worker_id) -> Result<()>`：校验 worker 下标、取得 worker mutex、调用 `heap.drainSorted()`，按全局 chunk 大小建立一个 `DiskRun`，调整 tracker 后保存 run。
- `spill(&mut self) -> Result<()>`：把状态写成 `inSpilling`，依次 spill 所有 worker，并在普通成功或 `Err` 返回前恢复为 `notSpilled`。
- `takeRuns(&mut self) -> Vec<DiskRun>`：用 `mem::take` 转移全部历史 run，同时清空 helper 内的列表。
- `isSpillTriggered(&self) -> bool`：以 run 列表是否非空表示是否实际产生过 spill 数据。
- `compare(&self) -> RowComparator`：克隆并返回共享比较器；当前生产调用点未使用此访问器。
- `topNSpillAction`：公开的超限动作，保存 `Arc<Mutex<topNSpillHelper>>` 和触发用 tracker。
- `topNSpillAction::Action(&self) -> Result<()>`：tracker 超限时锁住 helper；只有 helper 自身 tracker 用量达到 `limit / 10` 才调用 `setNeedSpill`。

本文件没有 trait、模块级自定义常量、条件编译项或 `Drop` 实现。状态值和 `spillChunkSize` 来自 `sort_util.rs`。

## 执行流程

当前生产主链从 `TopNExec::fetchTopN` 开始。每个输入 chunk 轮询交给一个 `topNWorker::run`，worker 更新有界堆并把堆的内存增量计入共享 tracker。tracker 超限后，执行器第一次创建 helper；`setNeedSpill` 成功时同步调用 `spill`。

`spill` 先以 Release store 进入 `inSpilling`，随后按 worker 下标串行调用 `spillHeap`。后者锁住 worker，`drainSorted` 取出并清空堆中的有序行；空行直接成功返回。非空行按 `spillChunkSize.load(Relaxed).max(1)` 切片，每片克隆为 `DataChunk` 并加入一个新 `DiskRun`。run 建成后，helper 按所有行的 `Row::memory_usage` 从内存 tracker 释放字节，按 `run.memory_usage()` 增加磁盘 tracker，最后将 run 追加到 `sortedRowsInDisk`。

所有 worker 处理完成或遇到第一个可传播错误后，`spill` 都在返回前把状态恢复为 `notSpilled`。输入耗尽时，`TopNExec::fetchTopN` 通过 `takeRuns` 取走历史 run，再把各 worker 未 spill 的 `heap.sortedRows()` 包装成 run。所有 run 使用相同的 `RowComparator` 进入 `newMultiWayMerger(...).collect()`，得到全局有序候选，随后由执行器截断到 `offset + count` 并应用最终窗口。

`topNSpillAction::Action` 是另一种触发入口：它只做阈值判断和状态标记，实际写出必须由执行器或其他驱动者调用 `spill`。当前 Rust 生产主链直接检查 `memTracker.exceeded()`，仓库中该 action 的可见构造调用位于 `topn_spill_test.rs`。

## 数据与状态

状态机的实际转换是 `notSpilled -> needSpill -> inSpilling -> notSpilled`。重复请求在 `needSpill` 或 `inSpilling` 时会被 CAS 拒绝；是否曾实际 spill 不编码在状态中，而由 `sortedRowsInDisk` 是否非空表示。`sort_util.rs` 虽定义了 `spillTriggered` 常量，本文件并未使用它。

`spillHeap` 会消费 worker 堆：`topNChunkHeap::drainSorted` 返回排序行并清空堆，所以成功后不能再从该 worker 取得同一批候选。每个非空 worker 每轮最多生成一个 run；多 worker 的 run 不在本文件内合成，最终由 `TopNExec` 统一多路归并。正确性不变量是每个 run 内部已按同一比较器有序。

tracker 记账分成两步：内存 tracker 按原始行的 `Row::memory_usage` 释放，磁盘 tracker 按包含 `DataChunk` 固定开销的 `DiskRun::memory_usage` 消费，两者数值不要求相等。`MemoryTracker::release` 饱和减到零，不会出现负数。`takeRuns` 只转移 run，不减少磁盘 tracker；`TopNExec::Close` 统一释放磁盘 tracker 当前值并丢弃 helper。

`topNSpillAction` 的 `limit / 10` 使用整数除法。限额为 1 到 9 时阈值为 0；限额为负数时 `MemoryTracker::exceeded()` 恒为 false，因此不会触发。action 检查的超限 tracker 与 helper 内部用于计算 TopN 占用的 tracker可以是不同对象，这对应“全局超限、但只有 TopN 数据足够多才值得 spill”的意图。

## 依赖与调用关系

上游生产调用集中在 `topn.rs`：`TopNExec::fetchTopN` 调用 `topNSpillHelper::{new,setNeedSpill,spill,takeRuns}`，`TopNExec::IsSpillTriggered` 调用 `isSpillTriggered`。`lib.rs` 公开模块，但没有从 crate 根直接再导出 helper 类型。`topn_spill_test.rs` 直接构造 helper 和 action，覆盖状态、阈值、失败恢复与端到端结果窗口。

下游调用边包括：`topNWorker.heap.drainSorted()` 消费并排序导出堆；`DiskRun::{default,add,memory_usage}` 保存分块结果；`Row::memory_usage` 计算内存释放量；`MemoryTracker::{exceeded,bytes_limit,bytes_consumed,consume,release}` 提供阈值和记账；标准库的 `AtomicI32` 与外层 `Mutex` 负责同步。

RustCodeGraph 将目标列为 17 个符号，并定位了 `topNSpillHelper`、`spillHeap`、`takeRuns`、`topNSpillAction` 及 `DiskRun`/`MemoryTracker` 定义。精确 `callers/callees` 查询在本轮未产出可用结果，因此上述调用关系同时用已索引的 `topn.rs`、`topn_worker.rs`、`sort_util.rs` 和独立测试中的直接调用点复核。

## 错误处理与边界

公开写出路径使用 `sort_util::Result<T> = std::result::Result<T, SortError>`。worker 下标越界返回 `topn worker {id} is out of range`；worker mutex 中毒返回 `topn worker lock poisoned`；`DiskRun::add` 的错误通过 `?` 传播。action 的 helper mutex 中毒则返回 `topn spill helper lock poisoned`。

空 worker 或空堆不是错误：`spillHeap` 不生成 run，完全空的 `spill` 不会把 `isSpillTriggered` 变成 true。chunk 大小读取后至少取 1，避免 `chunks(0)` panic。`setNeedSpill` 是幂等仲裁而不是错误报告，状态不匹配时只返回 false。

`spill` 对普通 `Err` 会执行末尾的状态恢复，Rust 测试通过制造 worker 锁中毒验证失败后仍可再次 `setNeedSpill`。但 panic 会越过末尾 store，因此源码注释所说的“Go defer 包含 panic/error”只对普通错误成立，Rust 没有 `catch_unwind` 保证 panic 后恢复状态。另一个边界是部分成功：若前面的 worker 已生成 run并调整 tracker、后续 worker失败，本文件不会回滚先前副作用；调用者必须把这种错误视为非事务式失败。

当前 `DiskRun::add` 唯一可见的失败条件是空 chunk或已关闭；本文件跳过空行且新建的 run未关闭，所以正常路径不会触发，但未来真实磁盘实现可能增加 I/O 错误，届时需要定义已释放内存和部分 run 的补偿策略。

## 并发与资源生命周期

`status` 是原子整数：CAS 使用 AcqRel/Acquire，状态读取使用 Acquire，进入与退出 spill 使用 Release。原子只仲裁状态；`sortedRowsInDisk` 不是内部同步容器，修改方法要求 `&mut self`。需要跨线程共享时，调用方必须像 `topNSpillAction` 一样在整个 helper 外包 `Mutex`。

worker 各自通过 `Arc<Mutex<topNWorker>>` 保护。Rust 的 `spill` 不并发写出，而是一次锁一个 worker、串行处理；持有 worker 锁的代码若重入 helper 可能形成锁等待。action 持有 helper mutex 时仅做原子状态转换，不调用 `spill`，避免在该锁内再执行长时间写出。

`DiskRun` 当前没有文件句柄，helper 也没有 `close` 或 `Drop`。run 由 `takeRuns` 转移给归并器，或随 helper 丢弃；执行器 `Close` 负责重置 worker、释放磁盘 tracker 并清空 helper。若改为真实临时文件，必须为成功、普通错误、取消与 panic 路径明确关闭和删除责任，不能沿用当前仅靠内存析构的假设。

## 与 Go 版本的对应关系

Go 对照为 `pkg/executor/sortexec/topn_spill.go`。两版都保存 worker、已 spill run、内存/磁盘 tracker，并采用 `notSpilled`、`needSpill`、`inSpilling` 三阶段；action 都在整体 tracker 超限且 TopN 自身数据达到约十分之一限额时请求 spill，实际 TopN 结果最终都要与各 run 归并。

当前 Rust 是聚焦的简化移植，尚不具备 Go 实现的完整语义：

- Go 用 `sync.Cond` 保护状态，action 会等待正在进行的 spill，完成后 `Broadcast`；Rust 用原子 CAS，没有等待/唤醒协议。
- Go 为每个 worker 启动 goroutine 并通过 WaitGroup/error channel 收集结果；Rust 串行锁定 worker。
- Go 的 `DataInDiskByChunks` 写入真实临时存储、挂接 disk tracker并显式 `Close`；Rust `DiskRun` 仍在内存中保存 cloned rows。
- Go 复用临时 chunk channel、检查 `finishCh`、每 100 行检查 `SQLKiller`、捕获 panic、发送异步错误并含 failpoint；Rust helper没有这些通道、取消检查或 panic恢复。
- Go 在 action 数据不足时调用 fallback OOM action，并提供 `DefSpillPriority`；Rust `topNSpillAction` 没有优先级或 fallback，只返回 `Ok(())`。
- Go helper保存字段类型、临时文件前缀和观测用 consumed/limit；Rust只保存比较器和简化 tracker。

Go 测试 `topn_spill_test.go` 覆盖构堆阶段与更新阶段 spill、先内存后 spill、并发关闭、kill、真实磁盘分块、failpoint 错误与无泄漏检查。Rust 独立测试覆盖“不超限不请求”“action 只标记、由执行器 spill”“空 spill 不算触发”“错误后状态恢复”以及小内存下 `Offset/Count` 结果正确，但不能替代上述 Go 并发、取消和真实 I/O 意图。

## 扩展指南

若调整触发策略，应同时审查 `topNSpillAction::Action` 与 `TopNExec::fetchTopN`：当前生产主链绕过 action，单改 action 不会改变执行器行为。新增阈值、fallback 或优先级后，应在独立的 `topn_spill_test.rs` 增加限额边界、重复请求和全局/局部 tracker 分离用例；测试不要嵌入生产源文件。

若实现真实磁盘 spill，主要替换点是 `spillHeap` 中的 `DiskRun` 构建，以及 `sort_util::DiskRun`/`multi_way_merge::diskSource`。必须同步定义临时文件所有权、实际磁盘字节记账、部分写入失败补偿、取消/kill 检查、关闭与删除时机，并移植 Go 测试中的故障注入和无泄漏断言。

若引入 worker 并行与条件等待，必须保持：同一时刻只有一个有效 spill 请求；等待者在成功和所有错误路径都能被唤醒；每个 worker 堆只被消费一次；部分失败不会静默丢行；锁顺序不会让 action、worker 与执行器互相等待。Go 的 cond、WaitGroup、channel 和 `finishCh` 是直接语义依据，但 Rust 可以选择不同原语。

性能敏感点包括 `drainSorted` 的全量物化、每个 `part.to_vec()` 对行的克隆、所有 run 最终 `collect()` 为完整向量，以及串行 worker 写出。优化这些点时必须维持 run 内有序、全局比较器一致、TopN 最终窗口准确和 tracker 对称；应补充多 worker、多轮 spill、空 worker、升降序/NULL 以及错误发生在中间 worker 的回归测试。

## 验证依据

本次事实核对读取了：

- 目标源码：`pkg/executor/sortexec/topn_spill.rs`。
- crate 与模块边界：`pkg/executor/sortexec/Cargo.toml`、`pkg/executor/sortexec/lib.rs`；`pkg/executor` 下无 `doc.go` 可读。
- Rust 直接主链和数据结构：`topn.rs`、`topn_worker.rs`、`sort_util.rs`。
- Rust 独立测试：`topn_spill_test.rs`，覆盖 action 阈值、状态转换、空 spill、失败恢复和 `Offset/Count` 端到端行为。
- Go 对照与测试：`topn_spill.go`、`topn_spill_test.go`，用于核对并发、条件等待、kill、真实磁盘、fallback、故障注入和清理语义。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/sortexec` 将目标列为含 17 个符号的 Rust 文件。执行了目标文件 `node`、`topNSpillHelper`/`topNSpillAction`/`DiskRun`/`MemoryTracker` 查询、关键入口的 `callers/callees` 尝试，并通过相邻已索引源码的直接调用点补证。交付结构以任务指定命令验证文档存在且恰有 11 个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo 或代码测试。

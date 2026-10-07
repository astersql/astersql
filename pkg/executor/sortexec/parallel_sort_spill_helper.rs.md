# `pkg/executor/sortexec/parallel_sort_spill_helper.rs`

## 文件定位

本文件属于 `astersql-executor-sortexec` crate，由同目录 `lib.rs` 以 `pub mod parallel_sort_spill_helper` 纳入模块树。它位于并行排序的数据收集与最终输出之间：`SortExec::fetchParallel` 在多个 `parallelSortWorker` 累积输入、内存跟踪器超限时创建 `parallelSortSpillHelper`，触发中间 spill，并在输入耗尽后调用 `mergeAll` 生成最终有序行。该文件不负责 SQL 排序键的构造、输入拉取或结果分块；这些分别由 `sort_util::comparator`、`SortExec` 和 `SortExec::Next` 完成。

`pkg/executor/sortexec/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/executor/sortexec`，未声明 feature；清单中的 workspace 依赖目前都置于 `cfg(windows)` 下。本文件自身只使用标准库同步原语和 crate 内模块，因此它的直接依赖边不涉及外部 crate。

## 核心职责

`parallelSortSpillHelper` 有三项紧密相关的职责：

1. 用原子状态协调“未请求 spill、需要 spill、正在 spill”三个阶段，避免同一 helper 被重复标记；入口是 `setNeedSpill`、`spillStatus`。
2. 向每个 worker 请求 `multiWayMerge`，把 worker 的有序行按 `spillChunkSize` 分块装入 `DiskRun`，再通过 `mergeRuns` 把本轮多个 worker run 合并成一个全局有序 run；入口是 `spill`。
3. 输入结束时把历史 spill run 与各 worker 尚存的内存行合并，返回完整的 `Vec<Row>`；入口是 `mergeAll`。

这里的 `DiskRun` 是 `sort_util.rs` 中持有 `Vec<DataChunk>` 的内存对象，并不执行文件 I/O。因此“落盘”是与 Go 结构对应的接口语义；当前 Rust 实现不能据此视为已具备真实临时文件释放、I/O 背压或磁盘错误处理能力。

## 主要符号

- `parallelSortSpillHelper`：公开结构体。公开字段 `workers: Vec<Arc<Mutex<parallelSortWorker>>>` 保存参与者，`sortedRowsInDisk: Vec<DiskRun>` 保存历次 spill 的有序 run；`compare`、`status`、两个 tracker 和 `spillError` 为内部状态。
- `new(workers, compare, mem, disk) -> Self`：公开构造器。状态初始化为 `notSpilled`，磁盘 run 为空，保存比较器和共享 tracker。
- `setNeedSpill(&self) -> bool`：以 `compare_exchange(notSpilled, needSpill, AcqRel, Acquire)` 做唯一成功的状态转换；状态不是 `notSpilled` 时返回 `false`。
- `isSpillTriggered(&self) -> bool`：以 `sortedRowsInDisk` 是否非空判断是否实际产生过 run，不依赖状态码。
- `spillStatus(&self) -> i32`：以 Acquire 读取原子状态，供 spill action 和测试观察。
- `spill(&mut self) -> Result<()>`：公开 spill 主入口。串行锁住并清空每个 worker 的待归并数据，构造各 worker run，再把它们合成一条 run。
- `mergeRuns(&self, runs) -> Result<DiskRun>`：私有辅助函数，以 `diskSource` 和 `newMultiWayMerger` 合并多条 run，并重新按当前 `spillChunkSize` 分块。
- `mergeAll(&mut self) -> Result<Vec<Row>>`：公开收尾入口。必要时先 spill，然后消费历史 run，取出 worker 残留行，执行最终多路归并。
- `memoryTracker(&self) -> &Arc<MemoryTracker>`：公开只读访问器，供 `parallelSortSpillAction::executeAction` 判断可回收数据是否达到限额的十分之一。

本文件没有模块级常量、trait、条件编译项或 `Drop` 实现。`spillError` 只在 `spill` 开头读取，当前仓库没有对它赋值的路径，因此它尚不是一个实际工作的错误缓存机制。

## 执行流程

并行主链由 `sort.rs::SortExec::fetchParallel` 驱动：输入 chunk 轮询送入 worker；`memTracker.exceeded()` 后惰性创建 helper；`setNeedSpill` 成功时立即同步调用 `spill`。`spill` 先把状态写为 `inSpilling`，随后逐个取得 worker mutex，调用 `parallelSortWorker::multiWayMerge`。该调用会先完成 worker 本地排序，使用内存多路归并器生成一段有序 `Vec<Row>`，并归还 worker 在内存 tracker 上登记的字节数。

对每段非空 worker 结果，`spill` 按 `spillChunkSize.load(Relaxed).max(1)` 切片，逐块调用 `DiskRun::add`，再把该 run 的 `memory_usage` 计入 `diskTracker`。若本轮至少产生一条 run，`mergeRuns` 用相同的比较器把所有 worker run 合成一条全局有序 run并追加到 `sortedRowsInDisk`。闭包无论成功还是返回可传播错误，随后都把状态恢复为 `notSpilled`。

输入耗尽后，`mergeAll` 若观察到状态仍为 `needSpill`，先执行一次 `spill`。接着用 `mem::take` 消费已有 run，再逐个锁定 worker、调用 `multiWayMerge`，把非空结果包装成单 chunk run。无 run 时返回空向量；否则使用 `diskSource` 和 `newMultiWayMerger(...).collect()` 返回全局有序结果。`SortExec::fetchParallel` 将该向量转成输出队列，并把 helper 放回执行器以保留 spill 观察状态。

## 数据与状态

`workers` 中每个 `Arc<Mutex<_>>` 保护一个 worker 的 chunk、已排序批次及其内存记账。`spill`/`mergeAll` 调用 `multiWayMerge` 后，worker 通过 `mem::take` 移走本地有序批次、释放 `memoryBytes`，所以一次归并会消耗 worker 当前数据；调用者不能假设之后仍能从同一 worker 再读到这些行。

`sortedRowsInDisk` 按 spill 轮次保存 run。每次 `spill` 最多追加一条合并后的 run，`mergeAll` 则一次性取走全部 run。`diskTracker` 在每个 worker 临时 run 建成后按其估算内存用量递增；本文件不主动释放该计数，执行器的 `SortExec::Close` 统一释放 tracker 当前值。由于 `DiskRun` 当前仍持有内存数据，这一计数是模拟磁盘占用，不是实际文件字节数。

状态转换的当前事实是 `notSpilled → needSpill → inSpilling → notSpilled`。虽然 `sort_util.rs` 定义了 `spillTriggered`，本文件从未把 `status` 写成该值；“是否触发过”由 run 列表独立表达。`setNeedSpill` 只允许从 `notSpilled` 转换，因此正在 spill 或已被标记时的重复请求会失败。

排序正确性依赖每个输入 run 已按同一个 `RowComparator` 排序；`mergeRuns` 和最终归并不会重新全量排序错误的输入。`spillChunkSize` 为全局原子值，读取时至少取 1，避免 `slice::chunks(0)`。

## 依赖与调用关系

上游直接调用者如下：

- `sort.rs::SortExec::fetchParallel` 构造 helper，调用 `setNeedSpill`、`spill` 和 `mergeAll`；`SortExec::IsSpillTriggered` 调用 `isSpillTriggered`。
- `sort_spill.rs::parallelSortSpillAction::executeAction` 读取 `memoryTracker`，在 tracker 超限且 sort 数据达到限额十分之一时调用 `setNeedSpill` 与 `spill`，否则转交 fallback。当前生产 `SortExec` 主链没有构造该 action；仓库内可见的构造调用位于独立 Rust 测试。
- `parallel_sort_spill_helper_test.rs` 直接验证空 spill、锁中毒错误和状态恢复；`sort_spill_test.rs` 直接构造 helper 验证 spill action 阈值行为。

下游调用边为：`parallelSortWorker::multiWayMerge` 完成本地排序和内存归并；`DiskRun::{default, add, memory_usage}` 保存分块结果；`diskSource::new` 把 run 转成多路数据源；`newMultiWayMerger(...).collect()` 执行堆式归并；`MemoryTracker::consume` 登记模拟磁盘用量。所有这些符号都来自同一 crate 的 `parallel_sort_worker.rs`、`sort_util.rs` 和 `multi_way_merge.rs`。

RustCodeGraph 将本文件识别为 19 个符号，并能定位 `parallelSortSpillHelper`、`setNeedSpill`、`spillStatus`、`mergeRuns` 和 `mergeAll`；其本轮精确 `callers/callees` 查询未返回边，所以上述调用边另由 `sort.rs`、`sort_spill.rs` 的直接调用点和本文件调用表达式交叉核对。

## 错误处理与边界

公开操作使用 `sort_util::Result<T> = std::result::Result<T, SortError>`。worker mutex 中毒时，`spill` 和 `mergeAll` 都转换成固定错误 `parallel sort worker lock poisoned`；worker 本地排序中断、`DiskRun::add` 失败或多路数据源错误均通过 `?` 原样传播。`spill` 用局部闭包确保普通 `Err` 路径也会执行最终状态恢复，但 Rust panic 会越过恢复语句，因此它并不等价于 Go 对 panic 的 `recover` 与 defer 清理。

空输入不是错误：没有 worker 行时 `spill` 不追加 run，`isSpillTriggered` 保持 false；`mergeAll` 没有任何 run 时返回空向量。空 worker 结果会被跳过，从而避免 `DiskRun::add` 的“空 chunk”错误。`mergeRuns` 只在 `runs` 非空时由 `spill` 调用，但函数自身未拒绝空列表，空列表将产生空 `DiskRun`。

错误后已发生的副作用不会回滚。例如先前 worker 已被 `multiWayMerge` 消耗、tracker 已递增，随后 worker 锁或合并失败时，本文件没有事务式恢复。`spillError` 当前不会被设置，因而相同 helper 的后续调用不会自动重放先前错误。扩展错误处理时必须同时定义数据所有权、tracker 补偿和重试语义。

## 并发与资源生命周期

`status` 使用原子整数：`setNeedSpill` 的 AcqRel CAS 用于竞争请求的仲裁，读取使用 Acquire，进入与退出 spill 使用 Release store。原子状态只保护请求阶段；`workers`、`sortedRowsInDisk` 和 `spillError` 并非内部同步字段，所以会修改 helper 的操作要求 `&mut self`，共享场景还需像 `parallelSortSpillAction` 一样在 helper 外层使用 `Mutex`。

Rust 的 `spill` 并不并行处理 worker，而是按向量顺序持锁并归并；一次只持有一个 worker 锁。`mergeAll` 同样逐个处理。不要在持有 worker mutex 的外部代码中重入这些方法，否则可能造成锁等待或死锁。比较器为 `Arc<dyn Fn + Send + Sync>`，可以安全共享，但实际归并在当前线程同步执行。

`DiskRun` 没有文件句柄，helper 也没有 `close`/`Drop`。run 在 `mergeAll` 中被消费，或随 helper 丢弃；`SortExec::Close` 清空 helper 并重置磁盘 tracker。若未来替换为真实磁盘实现，必须补齐成功、错误、取消和 panic 路径的关闭/删除责任，并用独立测试检查临时文件泄漏，不能依赖当前的内存对象析构语义。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/sortexec/parallel_sort_spill_helper.go`。两版共有的骨架是：helper 保存已 spill 的有序数据；spill 前将状态设为 `inSpilling`，普通返回时恢复为 `notSpilled`；先排序/归并各 worker 数据，再按固定 chunk 大小写出；最终把历史 run 与残留数据做多路归并。Rust 独立测试 `parallel_sort_spill_helper_test.rs` 专门固定了“空 spill 不算触发”和“可传播错误后恢复状态”的行为。

当前 Rust 不是 Go 实现的完整等价移植，已核实的差异包括：

- Go 通过 `sync.Cond` 保护状态并在 spill 完成后 `Broadcast` 唤醒等待 goroutine；Rust 只有原子状态，没有等待/通知协议。
- Go 并发调用各 worker 的 `sortLocalRows`，再以 producer goroutine、缓冲 channel 和 consumer 写盘；Rust 逐 worker 同步执行，并把合并结果完整物化为 `Vec<Row>`。
- Go 的 `DataInDiskByChunks` 使用临时存储、挂接磁盘 tracker，并由 `close`/错误路径关闭；Rust `DiskRun` 是内存容器，没有真实 I/O 或文件清理。
- Go 监听 `finishCh`、用 stop channel 停止 producer、捕获 panic，并包含故障注入点；Rust helper 没有取消信号或 panic 恢复，查询 kill 只可能在 worker 本地排序检查点间接返回错误。
- Go 在成功写盘后 `releaseMemory`，拥有 `tmpSpillChunk` 复用、字节阈值和文件名前缀；Rust 依赖 worker `multiWayMerge` 释放自己的记账，没有临时 chunk 复用或文件命名。
- Go 每次 spill 直接追加一条 `DataInDiskByChunks`；Rust 先为每个 worker构造 run，再额外合并为单 run。Rust 的 `spillTriggered` 状态常量也未在本 helper 中使用。

Go 测试 `parallel_sort_spill_test.go` 覆盖真实磁盘路径、全量 spill、先内存后 spill、故障注入、文件泄漏和 issue 回归；这些不能由当前两个 Rust helper 单测替代。扩展 Rust 行为时应以这些 Go 测试意图为上限逐项移植，而不是因现有实现较简单而删减并发、取消或清理语义。

## 扩展指南

若新增 spill 触发策略，优先修改 `setNeedSpill`/状态模型以及 `sort.rs::fetchParallel` 或 `sort_spill.rs::parallelSortSpillAction::executeAction` 的接线，并在 `parallel_sort_spill_helper_test.rs`、`sort_spill_test.rs` 增加竞争与阈值测试。不要把测试嵌入本生产文件；本 crate 已在 `lib.rs` 中以独立 `#[cfg(test)] mod ..._test` 组织测试。

若实现真实磁盘 spill，主要替换点是 `DiskRun` 的创建/追加/读取与 `mergeRuns`、`mergeAll` 的数据源，必须同步明确：磁盘 tracker 是按实际写入量还是逻辑内存量计费；run 所有权何时转移；失败和取消时谁关闭并删除文件；最终归并能否流式输出而非收集整个 `Vec<Row>`。应移植 `parallel_sort_spill_test.go` 的磁盘正确性、部分 spill、错误注入和无泄漏用例。

若引入真正的 worker 并行与通知机制，需要同时审查锁顺序、状态转换、重复触发、取消和 panic 边界。Go 的 `sync.Cond`、WaitGroup、双 channel 与 `finishCh` 是直接语义证据；Rust 可选择不同原语，但不得省略“生产者退出后再销毁资源”和“错误后唤醒等待者”的行为保证。

性能敏感点包括两次完整物化（worker `Vec<Row>` 与 merger `collect`）、`part.to_vec()` 的行克隆、每轮把 worker run 再合成单 run，以及最终 `mergeAll` 再次完整收集。优化时必须保持稳定的排序键语义和各 run 有序不变量，并增加多 worker、多轮 spill、空 worker、升降序/NULL 和大数据量回归测试。

## 验证依据

事实核对覆盖以下路径：

- 目标源码：`pkg/executor/sortexec/parallel_sort_spill_helper.rs`。
- crate 与模块边界：`pkg/executor/sortexec/Cargo.toml`、`pkg/executor/sortexec/lib.rs`。
- Rust 直接入口与下游：`sort.rs`、`sort_spill.rs`、`parallel_sort_worker.rs`、`multi_way_merge.rs`、`sort_util.rs`。
- Rust 独立测试：`parallel_sort_spill_helper_test.rs`、`sort_spill_test.rs`；前者覆盖空 spill 与错误后状态恢复，后者覆盖 action 阈值和 fallback。
- Go 对照与回归：`parallel_sort_spill_helper.go`、`parallel_sort_spill_test.go`。

RustCodeGraph `status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/sortexec` 将目标列为含 19 个符号的 Rust 文件。执行了对 `parallelSortSpillHelper`、`setNeedSpill`、`isSpillTriggered`、`spillStatus`、`mergeAll`、`mergeRuns` 的 `query`，并对关键方法执行 `callers/callees`；后者没有产出调用边，因此使用局部源码调用点补证。最终交付以任务指定的结构命令验证文档存在且恰有 11 个固定二级标题；这是纯文档分析，按计划不运行 Cargo 或代码测试。

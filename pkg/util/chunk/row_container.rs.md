# `pkg/util/chunk/row_container.rs`

## 文件定位

本文件属于 `astersql-util-chunk` crate。crate 由 `pkg/util/chunk/Cargo.toml` 定义，`lib.rs` 通过 `#[path = "row_container.rs"]` 注册模块并 `pub use row_container::*`，因此这里的公开类型是 chunk crate 的直接 API。它位于列式 `Chunk`/`List` 与按行临时磁盘结构 `DataInDiskByRows` 之间：常态在内存中保存多个 Chunk，内存压力触发后把既有数据迁移到磁盘，并让后续读取和追加继续访问磁盘表示。

当前 Rust 应用侧的直接接线包括：`pkg/util/cteutil/storage.rs` 的 `StorageRC` 用 `RowContainer::New`、`Add`、`GetChunk`、`GetRow`、Tracker 和 `ActionSpill` 实现可溢写存储；`pkg/util/chunk/iterator.rs::NewIterator4RowContainer` 用行指针遍历容器；`pkg/util/chunk/row_container_reader.rs` 通过本文件实现的 `RowContainerSource` 异步预取行。RustCodeGraph 将本文件列为被 46 个文件引用，但其中包含测试和间接/同名匹配，具体调用关系应以可定位的上述源码为准。

## 核心职责

- `RowContainer`：维护单一的内存/磁盘状态，提供追加、计数、按 Chunk/`RowPtr` 读取、重置、关闭及用量 Tracker。`spillToDisk` 是状态切换的唯一核心实现。
- `SpillDiskAction`：把内存 Tracker 的超限回调转换为一次异步 spill；其他调用者在 spill 进行中等待，spill 后仍超限时调用 fallback。
- `SortedRowContainer`：在 `RowContainer` 上维护排序后的 `RowPtr` 数组，按多列比较函数排序，并在排序完成或排序失败后禁止继续追加；其 spill 顺序是“先排序，再把排序错误带入底层 spill”。
- `SortAndSpillDiskAction`：复用 `SpillDiskAction` 状态机，但调用排序容器的 spill，并用“容器内存大于外部限额的 10%”抑制过小分区产生过多临时文件。
- `RowContainerSource for RowContainer`：把容器适配给后台 Reader，按 Chunk 物化一组 `Row`。

## 主要符号

- 常量 `ErrCannotAddBecauseSorted`：排序后再次 `Add` 的稳定错误文本；`SignalCheckpointForSort = 10_240`：排序比较期间检查外部 kill/内存信号的周期。
- `rowContainerRecord { inMemory, inDisk, spillError }`：受 `RwLock` 保护的互斥状态。`inDisk.is_some()` 是已经进入磁盘模式的判据；`spillError` 保存迁移或预排序失败，供后续读写返回。
- `RowContainer`：`records`、`memTracker`、`diskTracker` 与可选 `actionSpill` 都经 `Arc` 共享。公开入口为 `New`、`ShallowCopyWithNewMutex`、`SpillToDisk`、`Reset`、计数方法、`Add`、`AllocChunk`、三组取行方法、Tracker getter、`Close` 和 spill-action getter。
- `spillStatus::{notSpilled, spilling, spilledYet}` 与 `baseSpillDiskAction`：状态、条件变量、只触发一次的原子标记、结束标记、测试等待计数、spill/enough 闭包以及 fallback 的组合。
- `SpillDiskAction`：`Action` 启动或等待 spill；`Reset` 允许下一轮再次触发；`SetFinished` 阻止容器关闭后再进入未 spill 状态；`WaitForTest` 只用于等待本动作启动的线程。
- `SortedRowContainerInner`：包含底层容器、可选 `rowPtrs`、升降序标志、键列、比较函数、专属 Tracker 和比较次数计数器。`rowPtrs: Some(_)` 同时代表“已经进入排序态”和“禁止追加”。
- `SortedRowContainer`：`Sort`、`SpillToDisk`、`Add`、排序读取、动作与计数入口。`SortAndSpillDiskAction` 只转发 `Action`/`WaitForTest`。
- 条件逻辑不是 `cfg` 编译分支，而是运行时 failpoint：`spillToDiskOutOfDiskQuota`、`testRowContainerDeadLock`、`errorDuringSortRowContainer`、`SignalCheckpointForSort`；`failpointBool` 仅把 `1/true/on` 识别为真。

## 执行流程

1. `RowContainer::New` 创建 `List`，创建无限额内存/磁盘 Tracker，把 List Tracker 挂到容器内存 Tracker，然后以 `inDisk = None`、无错误开始。
2. `Add` 获取记录写锁。若已有 `spillError` 则立即失败；磁盘模式下调用 `DataInDiskByRows::Add` 并把磁盘增量计入容器 Tracker，内存模式下交给 `List::Add`。
3. `SpillToDisk` 进入 `spillToDisk(None)`。该方法持有记录写锁，重复 spill 或动作已完成时直接返回；否则标记 `spilling`、递增全局 `memory::QueryForceDisk`，逐 Chunk 写入新的 `DataInDiskByRows`，每块之后调用内存 Tracker 的 `HandleKillSignal`。
4. 全部写入成功才 `List::Clear`；I/O 错误、failpoint panic 或调用方传入的预置错误被转为字符串保存。无论成功与否，都会安装 `inDisk`，同步磁盘字节，并最终把动作状态置为 `spilledYet`。因此“已 spill”不等于“spill 成功”，读写还必须检查 `spillError`。
5. `GetChunk`/`GetRow`/`GetRowAndAppendToChunkIfInDisk` 先传播持久化错误，再按 `inDisk` 选择磁盘读取或内存读取。`GetRowAndAlwaysAppendToChunk` 在内存路径显式 `AppendRow`，磁盘路径复用磁盘层返回的已追加 Chunk。
6. `SpillDiskAction::Action` 在 `notSpilled`、数据足够且首次触发时增加运行计数并启动线程；后续调用若见 `spilling`，在条件变量上等待。完成后若传入 Tracker 仍超限，则执行可选 fallback。
7. `SortedRowContainer::Add` 先确认 `rowPtrs` 尚未建立，预记每行 8 字节的指针容量，再追加到底层容器。`Sort` 枚举所有 Chunk/行生成 `RowPtr`，先发布 `Some(pointers)`，再执行不稳定排序；多键逐一比较，相等才检查下一键，`ByItemsDesc` 决定是否反转。
8. `SortedRowContainer::SpillToDisk` 捕获 `Sort` 返回的错误并传给底层 `spillToDisk`。这使排序 panic 被恢复为 `ChunkError`，但容器仍进入禁止追加、已安装磁盘句柄且后续读取返回该错误的状态。
9. `Reset` 在磁盘模式关闭文件、清零容器磁盘计量并重置已有动作；内存模式只重置 List。`Close` 标记动作完成、关闭磁盘、清空内存和错误并清零容器磁盘计量。排序容器关闭前还释放 `8 * NumRow` 的指针记账。

## 数据与状态

- 核心不变量是 `inDisk: None` 表示读写 `inMemory`，`Some` 表示所有后续数据访问走磁盘；成功迁移后内存 List 被清空。迁移失败仍会设置 `inDisk = Some`，并依靠 `spillError` 阻断继续读写。
- `RowPtr` 保存 `ChkIdx`/`RowIdx`，既是普通随机读地址，也是排序容器避免复制整行的排序载体。排序指针按每行 8 字节预记账，与两个 `u32` 字段一致。
- `RowContainer` 自身 `Clone` 会共享记录、Tracker 和动作槽。`ShallowCopyWithNewMutex` 当前只是 `self.clone()`，所以数据确实共享，但锁也共享。
- `GetChunk` 的内存分支通过 `CopyConstruct` 返回深拷贝；磁盘分支从磁盘物化。`GetRow` 返回的 `Row` 生命周期依赖底层 Chunk 的共享所有权设计，异步 Reader 额外持有源容器的 `Arc`。
- 排序是 `sort_unstable_by`，相等键之间没有稳定次序保证。`Sort` 幂等：`rowPtrs` 已为 `Some` 时直接成功返回；即使第一次排序 panic，指针数组已发布，后续也不会重新排序或允许追加。
- `timesOfRowCompare` 是宽松原子计数；达到检查点后清零并对 Tracker `Consume(1)`，用途是触发 Tracker 的信号检查，不代表真实新增 1 字节长期数据。

## 依赖与调用关系

- crate 内下游：`List`/`Chunk`/`Row`/`RowPtr` 提供内存数据与行寻址；`DataInDiskByRows` 提供按行临时磁盘读写；`CompareFunc` 提供排序键比较；`ChunkError`/`Result` 统一错误；`memory::Tracker` 和 `disk::NewTracker` 记账并传播 kill 信号。
- 标准库下游：`Arc` 管共享所有权，`RwLock`/`Mutex` 保护状态，`Condvar` 协调 spill 等待者，`AtomicBool`/`AtomicU32` 管一次触发和比较计数，`thread::spawn` 执行异步 spill，`catch_unwind` 把迁移/排序 panic 变成可返回错误。
- 外部 crate：`fail` 提供四个运行时注入点；`Cargo.toml` 直接声明 `fail`、`memory-crate`、`disk-crate`、`parser-mysql`、datum/field/scalar 类型等。`crossbeam-channel` 是相邻 Reader 的依赖，不由本文件直接调用。
- RustCodeGraph 的精确证据：`pkg/util/cteutil/storage.rs:165,224` 构造容器，`228-262` 转发写读，`305-315` 暴露 Tracker/动作；`pkg/util/chunk/iterator.rs:328-405` 通过计数与 `GetRow` 迭代；`pkg/util/chunk/row_container_reader.rs:40-49` 定义本文件实现的数据源 trait，`141-161` 在后台按块取行。
- 图查询 `callers spillToDisk --file pkg/util/chunk/row_container.rs` 还定位到本文件两个 `ActionSpill` 闭包、`row_container_test.rs` 以及排序执行路径；查询结果混有 Go 同名符号，不能据此断言所有列出的 Go 调用者也是 Rust 调用者。

## 错误处理与边界

- spill 中的 `DataInDiskByRows::Add` 错误与 panic 都保存为字符串，随后由 `Add`、`GetChunk`、`GetRow` 和组合取行方法返回 `ChunkError::Message`。原始错误类型和 panic backtrace不会保留。
- `Reset`/`Close` 直接传播磁盘 `Close` 错误；若关闭失败，函数会提前返回，后续状态清理可能未全部执行。调用者应处理返回值，不能把调用过 `Close` 等同于资源必然完整释放。
- `Sort` 恢复比较阶段 panic 并转为 `ChunkError::Message`；它内部对 `GetRow` 使用 `expect("in-memory row pointer")`，该 panic也会由外层捕获。排序前构建指针时的 `GetChunk` 错误则通过 `?` 正常传播。
- `GetSortedRow` 在尚未排序时返回明确错误；但已排序后的 `idx` 越界会因向量索引而 panic。`keyColumns`、`ByItemsDesc`、`keyCmpFuncs` 长度不一致或列下标非法也可能 panic，构造器不校验这些先决条件。
- `NumRowsOfChunk`、普通 `GetRow` 的地址边界由 `List`/`DataInDiskByRows` 决定，本文件不预检。`AllocChunk` 始终从内存 List 分配，不检查磁盘态或 spill 错误，因此它只是分配骨架，不代表该骨架已加入容器。
- failpoint 参数缺失或非真值都按 false 处理。`testRowContainerDeadLock` 为真时 `Add` 人为休眠一秒；它只用于验证锁时序。

## 并发与资源生命周期

- `records` 的单个 `RwLock` 保证 spill、追加、重置、关闭和读取不会观察到半切换状态。计数只持读锁；需要磁盘可变访问或可能修改记录的方法使用写锁。锁中毒处直接 `unwrap`，因此先前持锁 panic 可能令后续调用 panic。
- 动作状态与记录状态由不同锁保护。`spillToDisk` 在写记录前检查/设置动作状态，完成记录切换后释放记录锁，再发布 `spilledYet` 并唤醒条件变量等待者，避免等待者看到进行中状态却争用长期记录锁。
- 异步线程由 `SpillDiskAction::Action` 创建。`running` 与 `runningChanged` 只保证 `WaitForTest` 可等待线程结束；生产 `Close` 不 join 线程，而是用动作状态阻止新的 spill，并依赖共享 `Arc` 让已启动线程持有对象。
- `Reset` 关闭磁盘并允许动作再次触发；`Close` 清空数据并标记完成。没有实现 `Drop`，所以资源释放依赖显式 `Reset`/`Close` 或下层对象自身析构语义，安全扩展时不能假定离开作用域会执行这里的完整记账清理。
- Go 的 `ShallowCopyWithNewMutex` 为每个浅拷贝建立独立 `rLock`，spill 时锁住所有读锁以降低并发读争用；Rust 当前复制的是同一个 `Arc<RwLock<rowContainerRecord>>`，因此线程安全但不具备 Go 的独立读锁优化。这是实际结构差异，不应把源码注释中的“相同语义”扩展解释为相同锁性能。

## 与 Go 版本的对应关系

- 主体结构和行为逐项对应 `pkg/util/chunk/row_container.go`：内存 List/磁盘按行存储二选一、spill 错误持久化、`QueryForceDisk` 计数、每 Chunk kill-signal 检查、一次异步动作与 fallback、排序指针预记账、10% spill 阈值和四个 failpoint 均已移植。
- Rust 用 `Arc`、`RwLock`、原子量和闭包替代 Go 指针、`sync.RWMutex`、`sync.Once` 和接口 `spillHelper`；Go 的 `BaseOOMAction` fallback 在 Rust 中是一个显式闭包槽。
- Go 的磁盘 Tracker 通过子 Tracker `AttachTo` 汇总；Rust 在迁移和后续追加时手工计算/累加磁盘字节，并在 `Reset`/`Close` 清零。修改磁盘写入路径时必须继续保持增量记账一致。
- Go `GetChunk` 内存分支返回底层 Chunk 指针，Rust 返回 `CopyConstruct` 的拥有型副本；Rust API 因所有权要求产生了可观察的复制成本。Go `GetRowAndAlwaysAppendToChunk` 保证返回原 Chunk 指针，Rust 返回拥有型 `Chunk`。
- Go `Close` 会 detach 内存/磁盘 Tracker 并把内存 List 置空；Rust `Close` 清数据和磁盘计量，但保留 `List`、Tracker 对象及其现有挂接。上游 `StorageRC::Reopen` 因而显式关闭后新建容器，而不是仅 Reset。
- Go 测试还覆盖选择向量、Reader 与 spill 并发、关闭记账、阻塞唤醒和查询中断时延；当前 Rust 独立测试重点覆盖 spill/Reset、排序、Tracker 记账、排序失败后拒绝追加、磁盘配额错误和 failpoint 布尔解析，并未证明全部 Go 并发/性能场景完全等价。

## 扩展指南

- 修改内存/磁盘切换时，应集中改 `spillToDisk`，同时保持三项不变量：错误先保存再由读写传播；只有完整迁移成功才清内存；磁盘 Tracker 对首次迁移、后续 `Add`、`Reset`、`Close` 成对记账。同步扩展 `row_container_test.rs` 或 `row_container_4_aster_unit_test.rs`，不要把测试嵌入生产文件。
- 新增读取 API 时必须同时处理内存、磁盘和 `spillError` 三条路径；若 Reader 需要它，还要扩展 `RowContainerSource` 及 `row_container_reader.rs` 的独立测试。
- 新增排序键规则应在 `compareRows` 实现，并验证多键、升降序、相等键、空输入和非法配置。若要求稳定排序，需要明确更换 `sort_unstable_by` 的兼容性和内存/性能影响。
- 修改动作状态机时重点检查首次触发、并发第二次调用等待、spill 后 fallback、Reset 后重触发、Close 与在途线程的竞争；相应测试应串行化全局 failpoint。若要对齐 Go 的浅拷贝降争用设计，需要重新设计锁集合，不能仅调整 `ShallowCopyWithNewMutex` 名称或注释。
- 新增错误类型时避免继续仅字符串化，否则会失去可判别性；这可能影响与 Go 错误文本的兼容，应先补错误传播回归测试。
- 性能敏感修改应关注内存 `GetChunk` 深拷贝、`RowsOfChunk` 整块物化、排序期间反复 `GetRow` 加锁，以及磁盘逐行解码。本文档任务未运行基准或 Cargo，不能据此量化这些成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/chunk` 确认目标、模块、Go 对照与独立测试均已索引；`node --file pkg/util/chunk/row_container.rs` 阅读了 1-690 全文件；并读取 `row_container_test.rs`、`row_container_4_aster_unit_test.rs`、`row_container_reader.rs`、`iterator.rs`、`pkg/util/cteutil/storage.rs` 的直接证据。
- RustCodeGraph 调用查询：对 `spillToDisk` 使用 `query`、`callers --file pkg/util/chunk/row_container.rs`、`callees --file ...`；确认动作闭包、排序 spill、测试入口及下游磁盘/Tracker 操作。由于同名 Go/Rust 符号导致部分噪声，文中只采用了能回到具体 Rust 文件的边。
- crate/模块证据：`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/lib.rs`。
- Go 对照：`pkg/util/chunk/row_container.go`、`pkg/util/chunk/row_container_test.go`；Rust 测试：`pkg/util/chunk/row_container_test.rs`、`pkg/util/chunk/row_container_4_aster_unit_test.rs`。这些测试证明当前已有覆盖范围，不代表本次纯文档任务执行过测试。
- 人工复核结论：本文件存在是为了让大量 Chunk 在统一 API 下由内存原子切换到临时磁盘，并为排序和 Tracker 超限动作提供可复用生命周期；安全扩展的关键是状态切换、错误持久化、锁时序、资源记账与 Go 对照行为同时成立。

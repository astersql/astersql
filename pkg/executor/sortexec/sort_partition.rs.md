# `pkg/executor/sortexec/sort_partition.rs`

## 文件定位

本文件实现串行排序路径中的单个排序分区 `SortPartition`。它位于 `astersql-executor-sortexec` crate 内，由 `lib.rs` 以 `pub mod sort_partition` 暴露模块，但没有在 crate 根直接重导出类型。crate 的 Go 包映射是 `pkg/executor/sortexec`（`Cargo.toml` 的 `package.metadata.porting.go-package`）。

在完整执行链中，`sort.rs::SortExec::fetchUnparallel` 从子执行器读取 `DataChunk`，构造一个或多个 `Arc<Mutex<SortPartition>>`；达到内存阈值时先将当前分区落盘，再开启新分区。输入结束后，`multi_way_merge.rs::sortPartitionSource` 负责让各分区完成内部排序，并逐行读取它们，交给多路归并器生成全局有序结果。因此本文件是“输入 chunk”与“多路归并数据源”之间的可落盘分区边界，不负责整个 SQL 算子的打开、拉取或最终归并。

## 核心职责

- 用 `savedRows: Vec<Row>` 累积尚在内存中的行，并通过调用方提供的 `RowComparator` 原地排序（`add`、`sort`）。
- 在需要时将有序行按 `sort_util.rs::spillChunkSize` 分块写入一个 `DiskRun`，同时把用量从共享内存追踪器转移到共享磁盘追踪器（`spillToDisk`）。
- 使用单一 `cursor` 从内存行或已经物化的 `diskRows` 顺序读出已排序行（`getNextSortedRow`）。
- 维护关闭、排序、spill 状态与延迟错误，并响应共享的查询取消标志（`closed`、`sorted`、`spillStatus`、`spillError`、`killed`）。
- 在 `close` 中归还本分区登记的资源用量并清空所持行。

这里的 `DiskRun` 是 `sort_util.rs` 中由 `Vec<DataChunk>` 实现的当前 Rust 抽象，并非已验证的真实临时文件 I/O；`spillToDisk` 还把 run 克隆并展平到 `diskRows` 供读取。文档中的“磁盘/落盘”表示该抽象及磁盘用量账户，不能据此推断当前 Rust 版本已经执行真实文件写入。

## 主要符号

- `SortCancelled`：仅用于排序闭包内部的私有 panic 载荷。`sort` 借它从 `slice::sort_by` 的比较器深处中断，再将其转换成 `SortError("query interrupted")`。
- `pub struct SortPartition`：分区状态主体。`compare` 定义分区内的全序；`savedRows`/`diskRows` 保存两种阶段的行；`inDisk` 保留 `DiskRun`；`memoryBytes`/`diskBytes` 是本分区在共享 tracker 中登记的额度；`cursor` 是下一行位置。
- `new(compare, mem, disk) -> Self`：便捷构造器，创建永不自行置位的取消标志，然后委托 `newWithKiller`。
- `newWithKiller(compare, killed, mem, disk) -> Self`：完整构造器。初始状态为空、未排序、未关闭，`spillStatus = notSpilled`，两个用量计数均为零。
- `add(&mut self, chk: DataChunk) -> bool`：关闭后或状态达到 `inSpilling` 后拒绝追加；成功时先登记 `chk.memory_usage()`，再移动全部行到 `savedRows` 并令 `sorted = false`。
- `numRows(&self) -> usize`：返回 `savedRows.len() + diskRows.len().saturating_sub(cursor)`。正常写入/归并链上它用于判断尚未开始读取的当前分区是否非空；若对已部分读取、未落盘的分区调用，它不会从 `savedRows` 扣除游标，不能泛化为所有状态下的严格“剩余行数”。
- `memoryUsage(&self) -> i64`：返回本分区累计登记的内存字节，当前生产调用搜索未发现使用者。
- `sort(&mut self) -> Result<()>`：优先返回已保存的 `spillError`；未排序时执行可取消的原地排序，并缓存 `sorted = true`。
- `spillToDisk(&mut self) -> Result<()>`：完成排序、状态迁移、分块写 run、tracker 转账以及内存行释放。已是 `spillTriggered` 时幂等返回成功。
- `getNextSortedRow(&mut self) -> Result<Option<Row>>`：确保已排序，随后从 `diskRows`（存在 `inDisk` 时）或 `savedRows` 按游标克隆下一行；耗尽返回 `Ok(None)`。
- `setSpillError(&mut self, err)`：保存一个供后续 `sort` 返回的错误。限定 Rust 搜索未找到生产调用者，当前属于公开但未接线入口。
- `spillStatus(&self) -> i32`：以 Acquire 读取状态码。
- `close(&mut self)`：归还 tracker 用量、关闭 run、清空两类行并标记关闭；重复调用因计数已清零而基本幂等。

## 执行流程

1. `SortExec::fetchUnparallel` 以执行器比较器、共享取消位以及执行器级内存/磁盘 tracker 调用 `SortPartition::newWithKiller`。
2. 每个输入 chunk 到达时，执行器先用 tracker 总量和 chunk 估算量判断是否越过 `memLimit`。若当前分区已有行，调用 `spillToDisk`、保存该分区，并新建分区；否则调用 `add` 将 chunk 的行移入当前分区。
3. 输入结束后，非空的最后分区加入 `SortExec::partitions`。`sortPartitionSource::init` 逐个加外层互斥锁并调用 `sort`，保证所有来源内部有序。
4. `sort` 在每累计 `signalCheckpointForSort` 次比较后检查 `killed`。取消时比较器抛出 `SortCancelled`，外层 `catch_unwind` 只把这一载荷翻译为查询中断；其他 panic 通过 `resume_unwind` 继续传播。
5. `spillToDisk` 先调用 `sort`，再把状态写为 `inSpilling`。未关闭且非空时，按 `spillChunkSize.max(1)` 切分行，每块构造 `DataChunk` 并交给 `DiskRun::add`；每块之前检查取消位。
6. run 构造成功后，函数登记 `run.memory_usage()` 到磁盘 tracker、归还全部内存额度，保存 `inDisk` 和展平后的 `diskRows`，清空 `savedRows`，并把游标归零。无论关闭、空分区或内部写入结果如何，进入 spill 阶段后的尾部都会将状态写为 `spillTriggered`；但若前置 `sort()` 失败，函数在写入 `inSpilling` 前即返回，状态保持原值。
7. 多路归并器通过 `sortPartitionSource::next` 加锁调用 `getNextSortedRow`。每次成功读取使游标递增；所有行读完后 `None` 表示该归并路耗尽。
8. 执行器关闭或持有者清理时应调用 `close`，使 tracker 与行存储回到空状态。

## 数据与状态

核心状态可分为四组：

- 行数据：`savedRows` 是内存阶段的唯一排序对象；成功 spill 后它被清空，`inDisk` 和 `diskRows` 同时存在。读取路径以 `inDisk.is_some()` 选择 `diskRows`，而不是直接遍历 `DiskRun`。
- 读取位置：内存与 spill 后读取共用 `cursor`。构造和成功 spill 时为零，`getNextSortedRow` 每返回一行加一。`add` 不重置游标，所以设计前提是读取开始后不再追加；正常 `SortExec`/归并链满足这一阶段约束。
- 生命周期布尔量：任意成功追加令 `sorted = false`；成功排序令其为 true。`closed` 只在 `close` 置位，没有重新打开路径。关闭后仍可调用 `spillToDisk`，其行为是排序后成功空操作并最终进入 `spillTriggered`，这一语义由独立测试固定。
- spill 状态：`notSpilled(0) -> inSpilling(2) -> spillTriggered(3)` 是本文件实际写出的路径。`sort_util.rs` 还定义 `needSpill(1)`，但本类型不写入它；`add` 以 `>= inSpilling` 拒绝输入，因此 `needSpill` 本身不会阻止追加。

`memoryBytes` 使用整个 `DataChunk::memory_usage()` 的估算累计，包括 chunk/行/value 的估算开销；成功 spill 后清零。`diskBytes` 使用 run 中所有 chunk 的同类估算，并在 `close` 且 `inDisk` 存在时归还。tracker 使用原子计数并采用饱和式 release，但这些计数是资源核算，不拥有行数据。

## 依赖与调用关系

上游直接关系由限定源码搜索确认：

- `sort.rs::SortExec::fetchUnparallel` 调用 `newWithKiller`、`numRows`、`spillToDisk` 和 `add`，并把分区保存为 `Vec<Arc<Mutex<SortPartition>>>`。
- `multi_way_merge.rs::sortPartitionSource::{init,next}` 分别调用 `sort` 与 `getNextSortedRow`，把分区接入全局多路归并。
- `sort_spill.rs::sortPartitionSpillDiskAction::executeAction` 检查 `spillStatus`，尚未完成时调用 `spillToDisk`；该动作持有同样的 `Arc<Mutex<SortPartition>>`。
- `sort_partition_test.rs` 直接构造分区并验证空/关闭 spill；`sortexec_pkg_test.rs` 通过 `newWithKiller` 验证排序和 spill 中断。

下游全部来自同 crate 的 `sort_util`：`DataChunk`/`Row` 是数据模型，`RowComparator` 是共享比较闭包，`MemoryTracker` 负责额度，`DiskRun` 负责 run 抽象，`SortError`/`Result` 是错误边界，状态常量和两个阈值控制 spill 与取消检查。标准库依赖包括 `Arc`、`AtomicBool`、`AtomicI32` 和 panic 捕获 API。

`Cargo.toml` 声明库入口为 `lib.rs`，并把该 crate 的大量 AsterSQL 依赖限定在 `cfg(windows)`；本文件自身只通过 crate 内 `sort_util` 和标准库取类型。此处只陈述清单事实，不推断其他平台的完整构建可用性。

## 错误处理与边界

- 空分区 spill：`sort` 成功后进入 `inSpilling`，因 `savedRows.is_empty()` 返回 `errSpillEmptyChunk()`，随后状态仍落到 `spillTriggered`。`sort_partition_test.rs::empty_spill_matches_go_error_and_completes_state_transition` 同时固定错误文本和终态。
- 已关闭分区 spill：返回成功，不创建 run，终态仍为 `spillTriggered`；由 `spilling_a_closed_partition_is_a_successful_no_op` 覆盖。
- 查询取消：排序仅按比较次数周期检查；spill 写块时每块检查一次。两条路径都返回 `SortError("query interrupted")`，对应 `sortexec_pkg_test.rs` 的两个中断测试。
- panic：排序只截获类型为 `SortCancelled` 的内部载荷；比较器或排序实现的其他 panic 不会被吞掉。Rust 当前没有复刻 Go `sortNoLock`/`spillToDiskImpl` 对任意 panic 转 error 的宽泛恢复语义。
- 延迟错误：`sort` 会先返回 `spillError.clone()`，但 `setSpillError` 当前没有生产接线，`spillToDisk` 自身也不会把失败写回该字段。
- `DiskRun::add` 可报告空块或已关闭错误；本文件以 `chunk_size.max(1)` 且从非空 `savedRows` 切片，正常路径不会生成空块。错误通过 `?` 原样上抛。
- `add` 用布尔值而非 `Result` 表示生命周期拒绝；调用方 `SortExec` 将 false 转成 `errFailToAddChunk()`。
- 本文件没有为越界排序键负责；比较行为完全封装在传入的 `RowComparator` 中。

## 并发与资源生命周期

`SortPartition` 的可变字段没有内部互斥锁；生产持有者使用 `Arc<Mutex<SortPartition>>` 包裹它，`SortExec`、`sortPartitionSource` 和 spill action 都在调用前取得该锁。`spillStatus` 虽为原子量并使用 Acquire/Release，但这只使无锁读取状态码具备可见性，不允许绕过外层锁并发访问行、游标或 tracker 计数字段。`killed` 与 tracker 可跨线程共享，分别以原子布尔和原子整数实现。

排序期间持有调用方的分区锁，取消检查避免长排序无限忽略查询终止。spill 当前也是在同一外层锁保护下同步完成；本文件不创建线程、任务、通道或条件变量。与 Go 版本的 `sync.Mutex + sync.Cond` 不同，Rust 没有等待 spill 完成的条件广播机制。

资源阶段为：`add` 消费内存 tracker；成功 spill 先消费磁盘 tracker、再归还内存 tracker；`close` 归还仍登记的内存，并在 run 存在时归还磁盘额度和关闭 run。`SortPartition` 未实现 `Drop`，因此正确核销依赖显式 `close` 或上层对共享 tracker 的统一生命周期管理；仅释放 Rust 对象不会自动调用这里的 tracker `release`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/sortexec/sort_partition.go`。两者都保留“累积行 -> 分区内排序 -> 按 chunk spill -> 游标读出”的骨架、空分区错误、关闭后 spill 空操作、spill 状态以及周期性 kill 检查，Rust 测试还明确引用 Go 的中断测试意图。

当前 Rust 并非逐字段等价移植：

- Go 在分区内持有 `syncLock` 和保护状态的 `sync.Cond`；Rust 把互斥职责交给外层 `Arc<Mutex<_>>`，只将状态码做成原子量，没有等待/广播。
- Go 用字段类型、排序列、方向和比较函数在分区内部实现 `lessRow/keyColumnsLess`；Rust 接受预组装的 `RowComparator`，SQL 排序键语义主要位于 `sort_util.rs::compare_rows`。
- Go 的 `spillLimit`、`spillAction`、`hasEnoughDataToSpill` 和 tracker getter 属于分区；Rust 的动作包装位于 `sort_spill.rs`，阈值判断由执行器/动作层完成。
- Go 的 `chunk.DataInDiskByChunks` 配合 `dataCursor/reloadCursor` 分块读回；Rust 当前 `DiskRun` 保存在内存，并额外物化为 `diskRows` 后用 usize 游标读取。
- Go 对排序/spill 实现中的任意 panic 做 error 恢复；Rust 只把自身的取消 panic 转成 `SortError`。
- Go 的 `setError/checkError` 有执行链调用；Rust 只有 `setSpillError` 与 `sort` 的读取端，当前源码搜索未发现生产写入端。
- Go 的 `add` 估算为行槽开销加 chunk 内存；Rust 使用 `DataChunk::memory_usage()`。两者应保持账户收支自洽，但数值不应假定逐字节相等。

这些差异是当前代码事实。若后续要求完全复刻真实临时文件、异步 spill 或 Go 的错误/等待协议，应作为明确的行为移植任务处理，不能把现有抽象描述成已经支持。

## 扩展指南

- 改变分区切换或内存阈值时，优先检查 `sort.rs::fetchUnparallel` 与 `SortPartition::{add,numRows,memoryUsage}`，保持“读取后不追加”和 tracker 加减对称。相应测试应放在独立的 `sort_partition_test.rs` 或执行链测试文件，不要嵌入生产源文件。
- 改变排序/取消行为时，修改 `sort` 及 `sort_util.rs` 的 comparator 生成逻辑，并同步 `sortexec_pkg_test.rs` 中排序与 spill 中断用例。必须保留未知 panic 传播还是转换为错误，应先与 Go 合同明确对齐。
- 引入真实磁盘后端或流式读回时，修改 `spillToDisk`、`getNextSortedRow`、`close` 以及 `sort_util.rs::DiskRun`；避免继续同时保留完整 `inDisk` 与 `diskRows` 两份数据，并补充写入失败、部分 run、关闭和 tracker 归还测试。
- 若接通后台 spill，不能只依赖 `AtomicI32`：需要为 `savedRows`、`cursor`、错误和关闭状态建立一致的同步协议，并评估 Go 的条件变量等待/广播语义。`setSpillError` 的写入者、可见性和错误优先级也要一并定义。
- 增加状态时，应同时审查 `add` 的拒绝条件、`spillToDisk` 的幂等条件、`sort_spill.rs::executeAction` 以及独立状态测试，防止出现可继续追加但已承诺 spill 的中间态。
- 所有行为移植需继续与 `sort_partition.go` 逐项比对；Rust 生产修复应同步独立测试，并遵守仓库要求在可用代码顶部保留/添加相应版权声明。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/sortexec` 确认目标与 42 个相邻 Go/Rust 文件被索引；`node --file pkg/executor/sortexec/sort_partition.rs --offset 1 --limit 400` 返回目标文件全部 193 行和 22 个符号；`query` 精确定位 `newWithKiller`、`spillToDisk`、`getNextSortedRow`、`setSpillError`。精确 `callers/callees` 查询在 30 秒窗口没有返回结果，因此调用边由下面的限定源码搜索补证，未将图查询超时误写成“无调用者”。
- 目标实现：`pkg/executor/sortexec/sort_partition.rs`。
- crate 与模块边界：`pkg/executor/sortexec/Cargo.toml`、`pkg/executor/sortexec/lib.rs`。
- Rust 直接调用者/依赖：`pkg/executor/sortexec/sort.rs`、`multi_way_merge.rs`、`sort_spill.rs`、`sort_util.rs`；使用限定 `rg` 核对 `SortPartition` 及各方法引用。
- Go 对照：`pkg/executor/sortexec/sort_partition.go`，并通过 `sort.go`、`multi_way_merge.go`、`sort_spill.go` 核对其接线位置。
- 独立测试：`pkg/executor/sortexec/sort_partition_test.rs`；另读 `sortexec_pkg_test.rs` 中 `interrupted_during_sort_returns_query_interrupted` 和 `interrupted_during_spilling_returns_query_interrupted`，并定位对应 Go 测试 `sortexec_pkg_test.go::TestInterruptedDuringSort/TestInterruptedDuringSpilling`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构命令，并人工复核所有“当前支持”陈述均能回指上述代码或测试。

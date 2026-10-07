# `pkg/executor/sortexec/sort_util.rs`

## 文件定位

该文件是 `astersql-executor-sortexec` crate 的基础类型与公共工具层。crate 入口 `pkg/executor/sortexec/lib.rs` 通过 `pub mod sort_util` 装配本模块，并向 crate 使用者再导出 `DataChunk`、`Row`、`SortError`、`SortKey`、`SortValue`。上层 `sort.rs` 与 `topn.rs` 用这些类型构造排序执行器；`sort_partition.rs`、`parallel_sort_spill_helper.rs`、`topn_spill.rs` 和 `multi_way_merge.rs` 用本文件的比较器、内存计数、spill 状态和 `DiskRun` 完成分区排序、落盘模拟与归并。

`pkg/executor/sortexec/Cargo.toml` 声明 crate 名为 `astersql-executor-sortexec`、入口为 `lib.rs`，并把 Go 对照包标为 `pkg/executor/sortexec`。其实际依赖目前全部位于 `cfg(windows)` 条件段；本文件自身只依赖 Rust 标准库的比较、格式化、`Arc` 和原子类型，不直接调用这些外部 crate。

## 核心职责

- 定义排序数据模型：`SortValue` 表示可比较标量，`Row` 表示值向量，`DataChunk` 表示行批次，`SortKey` 描述列号、升降序和 NULL 位置。
- 建立统一排序语义：`compare_rows` 按键列表逐项比较，`comparator` 将键列表封装成可在线程间共享的 `RowComparator`。
- 提供资源核算模型：`memory_usage` 系列方法给出近似字节数，`MemoryTracker` 原子记录消耗与限额。
- 提供 spill 公共协议：状态常量、每块行数 `spillChunkSize`、错误构造器、内存中的 `DiskRun` 抽象及 chunk 游标。
- 保留 Go 移植所需的辅助载荷类型 `rowWithPartition`、`rowWithError`、`chunkWithMemoryUsage`，以及可恢复全局配置的测试辅助 `SetSmallSpillChunkSizeForTest`。

## 主要符号

- `spillChunkSize: AtomicUsize`：默认 1024 行。`sort_partition.rs`、`parallel_sort_spill_helper.rs` 和 `topn_spill.rs` 在写 run 时读取它；原子化允许测试临时替换。
- `signalCheckpointForSort = 10_240`：`SortPartition::sort` 每累计到该比较次数检查一次取消标志。
- `notSpilled`、`needSpill`、`inSpilling`、`spillTriggered`：数值依次为 0 到 3。不同 spill helper 使用其中适合自身的状态迁移，不能假设所有组件都以 `spillTriggered` 结束。
- `SortValue::{Null, Int, UInt, Float, Bytes}`：同类型按自然序比较；浮点使用 `total_cmp`，从而包含 NaN 和有符号零在内也有确定顺序。可对齐的数值跨类型会转换后比较；其余跨类型按 `kind()` 的固定序号兜底。
- `Row(Vec<SortValue>)`、`DataChunk { rows }`：轻量拥有型容器。`memory_usage` 是估算值，不是分配器精确统计。
- `SortKey { column, desc, nulls_first }`：`asc` 默认 NULLS FIRST，`desc` 默认 NULLS LAST；字段公开，调用方也可组合其他 NULL 规则。
- `compare_rows` / `RowComparator` / `comparator`：整个 Sort、TopN、堆和归并链共享的词典序入口。比较器是 `Arc<dyn Fn + Send + Sync>`。
- `SortError(String)` 与 `Result<T>`：排序子系统的轻量错误边界；`errSpillEmptyChunk`、`errFailToAddChunk` 保留 Go 错误文案。
- `MemoryTracker`：`consume` 增加计数，`release` 用 CAS 循环做饱和减法，`limit < 0` 表示无限制，`exceeded` 使用“达到或超过”语义。
- `DiskRun`：保存非空 `DataChunk` 列表、累计行数和关闭状态；提供追加、随机取 chunk、内存估算、关闭及展平消费。
- `dataCursor` / `NewDataCursor` / `reloadCursor`：按 chunk 和行定位的游标 API。当前 Rust 生产链没有调用 `reloadCursor`；`multi_way_merge.rs::diskSource::new` 会通过 `DiskRun::into_rows` 一次性展平 run，因此这组符号是尚未接入当前归并实现的 Go 对齐接口。
- `rowWithPartition`、`rowWithError`、`chunkWithMemoryUsage`：Go 并行流水线的数据载荷对照；在当前 Rust crate 中未发现生产调用。

## 执行流程

1. `SortExec::new` 或 `TopNExec::new` 接收 `Vec<SortKey>`，调用 `comparator` 把键列表捕获到共享闭包。
2. 比较两行时，`compare_rows` 按键顺序取列。越界列通过 `get(...).unwrap_or(&SortValue::Null)` 被视为 NULL，因此常量/无效键不会越界崩溃。
3. 一方为 NULL 时只依据 `nulls_first` 排序；两方均非 NULL 时调用 `SortValue::partial_cmp`，再仅对非 NULL 的结果按 `desc` 反转。首个非相等键立即决定结果，全部相等则返回 `Equal`。
4. 输入 `DataChunk` 进入分区或 worker 后，其估算值由 `MemoryTracker::consume` 记账。达到限额时，上层动作根据状态常量请求 spill。
5. spill 路径将已排序行按 `spillChunkSize` 分片，每片构造成非空 `DataChunk` 并由 `DiskRun::add` 保存；上层随后把相应估算值从内存 tracker 转到磁盘 tracker。
6. 当前 Rust 归并链由 `multi_way_merge.rs::diskSource::new` 调用 `DiskRun::into_rows`，把每个 run 展平成队列后进行堆归并。`dataCursor`/`reloadCursor` 描述的是逐 chunk 读取方式，但尚未进入该链。
7. 清理时，上层 `SortPartition::close` 调用 tracker 的 `release`，并对 run 执行 `close`；`DiskRun::close` 清空持有的数据、把行数归零并永久禁止后续 `add`。

## 数据与状态

排序值、行和 chunk 都拥有其内容；克隆 `Row` 或 `DataChunk` 会复制内部向量。`DiskRun::get_chunk` 也返回克隆，因此读取不会借用 run，但可能产生额外内存和复制成本。`DiskRun::into_rows` 消费 run，可避免对每行再次克隆；这正是当前磁盘归并源使用的路径。

`SortValue` 的总体顺序需要同时满足类型规则和键规则：NULL 位置由 `SortKey.nulls_first` 单独控制，不受 `desc` 反转；同类按值排序；非负 `Int` 与 `UInt` 可直接对齐；整数与浮点通过 `f64` 对齐，极大整数可能因浮点精度损失而比较为相等；负 `Int` 对 `UInt` 等无法对齐组合退回类型序。新增值类型必须同步考虑 `kind`、`PartialOrd` 和 `memory_usage`。

`MemoryTracker` 的 `consumed` 与 `limit` 是独立原子量。它提供线程安全计数，但不提供“检查限额并预留内存”的原子事务；调用者必须接受 `exceeded` 与随后动作之间状态可能变化。`release` 不允许结果低于零，但 `consume` 没有溢出保护，也不会拒绝负数，调用约定要求传入合理的正向消耗。

spill 状态码是跨模块协议。`SortPartition` 通常走 `notSpilled -> inSpilling -> spillTriggered`；TopN helper 则走 `notSpilled -> needSpill -> inSpilling -> notSpilled`，是否发生过 spill 由已保存 run 判断。状态值有序只在 `SortPartition::add` 的 `>= inSpilling` 判断中具有具体含义。

## 依赖与调用关系

直接上游包括：

- `sort.rs`：构造比较器和两个 tracker，并在串行/并行排序中传递 `DataChunk`、`Row`、`SortError`；添加分区失败时调用 `errFailToAddChunk`。
- `topn.rs` 与 `topn_chunk_heap.rs`：使用同一比较器维护 TopN 堆，并在结果阶段排序或归并。
- `sort_partition.rs`：消费 `DiskRun`、tracker、checkpoint、chunk 大小、状态码和空 spill 错误，是本模块资源/状态协议最完整的使用者。
- `parallel_sort_spill_helper.rs`、`topn_spill.rs`：按共享 chunk 大小生成 `DiskRun`，并在原子状态机保护下搬移内存/磁盘计数。
- `multi_way_merge.rs`：消费 `DiskRun`、`RowComparator` 和 `SortError`；当前以 `into_rows` 展平后归并。
- `lib.rs`：向 crate 外再导出五个核心数据与错误类型。

直接下游仅为标准库：`Ordering`、`Display`/`Formatter`、`Arc`、`AtomicI64`、`AtomicUsize`。RustCodeGraph 能定位本文件符号及 Go 同名符号，但对精确 `callers/callees` 查询未产出边；以上调用关系由限定在 `pkg/executor/sortexec` 的导入和调用点逐一核对。

## 错误处理与边界

- `compare_rows` 不返回错误：越界列按 NULL 处理，`SortValue::partial_cmp` 当前恒为 `Some`；保留 `unwrap_or(Equal)` 是防御性兜底。
- 空 chunk 不能写入 `DiskRun`，返回 `errSpillEmptyChunk()`；关闭后的 run 返回 `"disk run is closed"`；越界读取返回包含 chunk id 的错误。
- `reloadCursor` 在耗尽时返回 `Ok(false)`，正常装载下一 chunk 时返回 `Ok(true)`，并传播 `get_chunk` 的错误。
- `MemoryTracker::release` 对非正数直接忽略，并用 `saturating_sub` 防止欠账；但 `consume` 本身不校验参数和算术溢出。
- `DiskRun::close` 可重复调用；关闭后数据不可恢复。`into_rows` 不检查 `closed`，因为它消费所有权；关闭过的 run 只会得到空结果。
- `SetSmallSpillChunkSizeForTest(0)` 会把大小钳制为 1，恢复闭包只应调用一次。由于它修改进程级全局原子量，并行测试仍可能相互观察到临时值。

## 并发与资源生命周期

`RowComparator` 的闭包要求 `Send + Sync`，可由并行 worker 通过 `Arc` 共享；捕获的键列表只读。`MemoryTracker` 使用 Relaxed 原子顺序，因为它只承担计数/阈值观测，不发布其他数据。spill 状态本身位于上层 helper/partition，并使用 Acquire/Release 或 AcqRel 保证状态迁移的可见性。

`spillChunkSize` 的常规读取是 Relaxed，测试替换和恢复是 SeqCst。该设计保证原子访问但不隔离并行测试；安全扩展测试时应保存并执行返回的恢复闭包，并避免同时依赖不同 chunk 大小。

`DiskRun` 本身没有锁、也未声明内部并发协议；共享访问由上层 `Mutex<SortPartition>` 或 `Mutex<...SpillHelper>` 串行化。其生命周期是创建默认空 run、追加非空 chunk、供归并消费或由 owner 关闭。tracker 不嵌入 `DiskRun`，所以 run 的创建/关闭与内存、磁盘计数调整必须由上层成对完成。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/sortexec/sort_util.go`。两端一致保留了两个错误文案、`spillChunkSize = 1024`、比较检查点 10240、四个 spill 状态值，以及 `rowWithPartition`、`rowWithError`、`chunkWithMemoryUsage`、`dataCursor`、`NewDataCursor`、`reloadCursor` 的结构意图。

Rust 文件同时承担 Go 版本分散在其他组件中的基础能力：Go 的真实 SQL 值、行、chunk、tracker 和磁盘 chunk 由 `pkg/util/chunk`、`pkg/util/memory` 等包提供；Rust 当前以 `SortValue`/`Row`/`DataChunk`、`MemoryTracker` 和内存向量实现自包含模型。因此 Rust `DiskRun` 名为磁盘 run，但当前没有文件 I/O，`memory_usage` 也是近似核算，不能把它描述成 Go `chunk.DataInDiskByChunks` 的完整持久化等价物。

Go 的 `reloadCursor` 被 `multi_way_merge.go` 与 `sort_partition.go` 调用，逐 chunk 从 `DataInDiskByChunks` 恢复；Rust 当前生产路径改用 `DiskRun::into_rows` 和 `SortPartition.diskRows`，导致 Rust 游标接口尚未接线。Go 文件还包含 panic 恢复/日志与 failpoint 注入函数，Rust `sort_util.rs` 没有对应实现；Rust 的取消检查在 `sort_partition.rs::sort` 中通过原子 kill 标志和 panic 捕获完成。这些都是当前实现差异，不应推断为等价功能已经存在。

## 扩展指南

- 新增标量类型：修改 `SortValue`、`kind`、`PartialOrd`、`memory_usage`，并在独立测试文件中覆盖同类型、跨类型、NULL、升降序及特殊值。避免只为测试通过而简化 Go 比较语义。
- 修改排序键规则：以 `compare_rows` 为唯一入口，重点回归 `sort_test.rs` 的 NULL/降序、`parallel_sort_test.rs` 的多键与越界常量键、`rank_topn_test.rs` 的前缀键和 `benchmark_test.rs` 的有序性检查。
- 修改 spill chunk 或 run：同步检查 `sort_partition.rs`、`parallel_sort_spill_helper.rs`、`topn_spill.rs` 和 `multi_way_merge.rs`。若把 `DiskRun` 替换为真实磁盘资源，必须定义 I/O 错误、关闭/删除语义、tracker 归还时机，并决定接入现有 `dataCursor` 还是删除未接线接口；测试逻辑仍放在独立 `*_test.rs` 文件。
- 修改 tracker：保持 release 不产生负值，并明确是否需要比 Relaxed 更强的可见性或原子“检查并预留”操作；所有 owner 的 `close`/错误路径都要回归。
- 扩展测试辅助：不要在源文件内嵌测试。全局 `spillChunkSize` 的修改应始终通过恢复 guard/闭包，并考虑并行测试串扰。
- 对齐 Go 时应以 `sort_util.go`、`sort_partition.go` 和 `multi_way_merge.go` 的实际调用为证据；不要把 Go 的 failpoint、日志、真实磁盘 chunk 能力写成 Rust 已支持。

## 验证依据

- 源码与 crate：`pkg/executor/sortexec/sort_util.rs`、`pkg/executor/sortexec/lib.rs`、`pkg/executor/sortexec/Cargo.toml`。
- 生产调用证据：`sort.rs`、`topn.rs`、`topn_chunk_heap.rs`、`sort_partition.rs`、`parallel_sort_spill_helper.rs`、`topn_spill.rs`、`multi_way_merge.rs`、`sort_spill.rs`。
- Rust 独立测试证据：`sort_test.rs::serial_sort_honors_null_ordering_and_descending_values`，`parallel_sort_test.rs::parallel_sort_honors_multi_column_asc_desc_order` 与 `constant_ordering_items_do_not_change_parallel_sort_order`，`sort_spill_test.rs::disk_run_preserves_chunk_boundaries_and_rejects_after_close`，以及 `topn_spill_test.rs`、`sort_partition_test.rs`、`multi_way_merge_test.rs`、`benchmark_test.rs` 中对比较器、tracker、spill 状态和归并的使用。
- Go 对照：`pkg/executor/sortexec/sort_util.go`；游标调用由 `multi_way_merge.go` 与 `sort_partition.go` 交叉验证；小 chunk 测试辅助的 Go 实现在 `sort_partition.go::SetSmallSpillChunkSizeForTest`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query` 精确定位 Rust `compare_rows`（第 139 行）、`comparator`（第 177 行）、`reloadCursor`（第 378 行）、`SetSmallSpillChunkSizeForTest`（第 410 行），并定位 Go 同名对应项。精确 `callers/callees` 未返回结果，调用边改由上述限定目录源码搜索核验。
- 未运行 Cargo：本任务为纯文档分析，按计划以结构检查和源码事实复核验收。

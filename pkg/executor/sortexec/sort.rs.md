# `pkg/executor/sortexec/sort.rs`

## 文件定位

`sort.rs` 是 `astersql-executor-sortexec` crate 中全量排序执行器的核心编排文件。它把上游 `RowSource` 产出的 `DataChunk` 全部物化，依据 `SortKey` 生成的 `RowComparator` 做全局排序，再按调用方要求的 chunk 大小返回结果。crate 根 `pkg/executor/sortexec/lib.rs` 公开重导出 `RowSource` 和 `SortExec`；`pkg/executor/physical_plan_runtime.rs` 的 `PhysicalSort` 分支把子计划结果包装为 `VecRowSource`，调用 `SortExec::new`，并由 `drain_sort` 反复调用 `Next` 直至空 chunk 后 `Close`，这是当前可核验的 SQL 物理计划接线。

该文件本身不负责行值比较规则、分区持久化或多路归并的细节：比较器和基础数据类型来自 `sort_util.rs`，串行分区来自 `sort_partition.rs`，并行本地排序与落盘分别交给 `parallel_sort_worker.rs` 和 `parallel_sort_spill_helper.rs`，最终归并使用 `multi_way_merge.rs`。

`pkg/executor/sortexec/Cargo.toml` 声明包名为 `astersql-executor-sortexec`、库入口为 `lib.rs`，并记录 Go 对照包 `pkg/executor/sortexec`。清单中的 TiDB 子 crate 依赖当前全部位于 `cfg(windows)` 条件段；`sort.rs` 当前直接使用 crate 内模块和标准库，不应据此推断其他平台已经接通 Go 版的 executor/session context 能力。

## 核心职责

- 用 `RowSource` 抽象上游的 `open` / `next` / `close` 生命周期，并提供 FIFO 内存实现 `VecRowSource`。
- 在 `SortExec::new` 中固化排序键、比较器、并发度、输出 chunk 上限、内存阈值、内存/磁盘追踪器和共享 kill 标志；并发度与 chunk 大小均被钳制为至少 1。
- 在第一次 `Next` 时惰性执行完整拉取和排序；`concurrency == 1` 进入 `fetchUnparallel`，大于 1 进入 `fetchParallel`。
- 串行模式按内存阈值切换 `SortPartition`，必要时先排序并落盘，再用 `sortPartitionSource` 做全局多路归并。
- 并行模式轮询把 chunk 分发给多个 `parallelSortWorker`，内存超限时通过 `parallelSortSpillHelper` 落盘，最后由 `mergeAll` 合并所有磁盘 run 和残留内存数据。
- 提供 `Close`、`Kill`、spill 状态和资源追踪查询接口，使调用方能收尾、取消和观测执行。

## 主要符号

- `pub trait RowSource: Send`：上游数据源契约。`open`、`close` 默认成功且无操作；实现者必须提供 `next() -> Result<Option<DataChunk>>`，以 `None` 表示耗尽。`Send` 只规定类型可以跨线程转移，并不表示 `SortExec` 当前会为拉取动作创建线程。
- `pub struct VecRowSource`：用 `VecDeque<DataChunk>` 保存输入；`new` 将向量转换为队列，`next` 用 `pop_front` 保持 FIFO。它既用于独立测试，也用于 `physical_plan_runtime.rs` 当前的 `PhysicalSort` 接线。
- `pub struct SortExec`：保存子源、排序键与比较器、执行配置、两个 `MemoryTracker`、串行分区、可选并行 spill helper、已排序结果队列，以及 `fetched`、`opened`、`killed` 生命周期状态。
- `SortExec::new(...) -> Self`：由 `comparator(by_items.clone())` 构造比较器；`memLimit` 传给内存追踪器，磁盘追踪器以 `-1` 表示不设上限。
- `Open(&mut self)`：幂等打开；先拒绝空排序键，再调用 `child.open()`，成功后设置 `opened`。
- `fetchUnparallel(&mut self)`：串行收集、阈值切分、分区 spill 与最终归并入口。
- `fetchParallel(&mut self)`：worker 构造、轮询分发、内存超限 spill 与最终归并入口。这里的“并行”表示多 worker 数据结构和分片逻辑；该函数自身没有创建线程，锁也在当前调用栈中顺序取得。
- `fetch(&mut self)`：基于 `fetched` 的一次性物化门；仅在选定路径成功后将其设为 `true`。
- `Next(&mut self, max_rows)`：自动 `Open` 和 `fetch`，从 `result` 头部弹出至多 `max(max_rows, 1).min(maxChunkSize)` 行；耗尽时返回空 `DataChunk`。
- `Close(&mut self)`：关闭串行分区、清空结果和 helper、释放磁盘追踪量、复位 `fetched/opened`，最后传播 `child.close()` 的结果。
- `Kill(&self)`：以 Release 顺序把共享原子标志设为 `true`；拉取循环、分区排序/spill 和 worker 排序在各自检查点读取它。
- `IsSpillTriggered`、`GetPartitionListLen`、`GetMemTracker`、`GetDiskTracker`：观测接口，分别报告磁盘用量/helper 状态、串行分区数以及两个追踪器。

文件内没有模块级常量、条件编译项或额外 trait impl；条件依赖位于相邻 `Cargo.toml`，测试模块装配位于 `lib.rs`。

## 执行流程

1. 调用方用子 `RowSource`、排序键、并发度、最大 chunk 大小和内存限额构造 `SortExec`。比较器在构造期生成，但子源尚未打开。
2. 第一次 `Next(max_rows)` 调用 `Open`。空 `byItems` 立即报错；否则打开子源并记录 `opened = true`。后续 `Next` 不重复打开。
3. `fetch` 检查 `fetched`。若已经成功物化则直接复用 `result`；否则根据并发度选择路径。
4. 串行路径从空 `SortPartition` 开始。每取得一个 chunk，先检查 kill，再估算 `chunk.memory_usage()`。当限额非负、追踪用量加新 chunk 用量达到或超过限额、且当前分区非空时，当前分区 `spillToDisk`，加入 `partitions`，然后创建新分区。`add` 失败被转换为 `errFailToAddChunk()`。输入耗尽后保存最后一个非空分区，并通过 `newMultiWayMerger(sortPartitionSource, compare).collect()` 得到全局有序结果。
5. 并行路径创建 `concurrency` 个 worker，每个 worker 的本地批次阈值为 `maxChunkSize * 8`。输入 chunk 以 `index % workers.len()` 轮询分配；`saveChunk` 更新共享内存追踪。若追踪器超限，则惰性创建 helper，只有 `setNeedSpill` 成功把状态从 `notSpilled` 改为 `needSpill` 时才立即 `spill`。
6. 输入耗尽后，无论之前是否 spill，均确保存在 helper 并调用 `mergeAll`。helper 先处理遗留的 `needSpill`，再分别让 worker 完成本地排序和内存多路归并，把非空结果转成 run，与已有磁盘 run 做最终归并。
7. 成功物化后 `fetch` 设置 `fetched = true`。`Next` 从 `VecDeque<Row>` 前端取出本批结果；重复调用保持顺序且不会重新拉取子源，直至返回空 chunk。
8. `Close` 释放当前执行状态并关闭子源。独立测试 `parallel_sort_can_be_closed_and_reopened_without_stale_results` 证明：当子源的 `open` 自行恢复输入时，同一执行器可以在 `Close` 后再次完整执行。

## 数据与状态

- 排序配置：`byItems` 决定键列、升降序及 NULL 规则；`compare` 是其克隆后生成的共享闭包。`concurrency` 只在构造时钳制，之后决定整次执行路径；`maxChunkSize` 同时限制输出批大小，并参与并行 worker 的批次阈值计算。
- 资源配置：`memLimit < 0` 表示本文件不主动按限额切串行分区；非负值同时交给 `MemoryTracker` 的 `exceeded` 判定。`diskTracker` 没有硬限制，只记录估算用量。
- 串行状态：`partitions` 保存所有非空 `Arc<Mutex<SortPartition>>`。分区内部维护内存行、可选 `DiskRun`、读取游标、spill 状态和自身记账；`sort_partition.rs` 在落盘时从内存追踪转移到磁盘追踪，在 `close` 时释放两类用量。
- 并行状态：worker 以 `Arc<Mutex<_>>` 共享给 helper；每个 worker 保存未排序 chunk、已排序批次和自身内存计数。helper 保存 worker、磁盘 run、spill 原子状态和错误槽。`result` 则持有最终全量行，因此当前实现即使发生 spill，也会在返回首个结果前重新把最终排序结果收集进内存队列。
- 生命周期状态：`opened` 防止重复打开子源；`fetched` 防止重复物化；二者由 `Close` 复位。`killed` 在 `Kill` 后不会由 `Close` 或 `Open` 清回 `false`，所以被 kill 的实例不能安全地假定可复用，这是扩展或复用时必须保持可见的边界。
- 空输入不会创建最终分区，`result` 保持为空；`Next` 仍返回合法空 chunk。`max_rows == 0` 会按 1 处理，而不是返回零容量批次。

## 依赖与调用关系

已由源码和 RustCodeGraph 文件/符号查询核对的主要关系如下：

- 上游应用接线：`physical_plan_runtime.rs::execute_node` 的 `PhysicalSort` 分支 → `SortExec::new` → `drain_sort` → 循环 `SortExec::Next` → `SortExec::Close`。
- crate 出口：`sortexec/lib.rs` 的 `pub use sort::{RowSource, SortExec}` 让其他 executor 代码通过 `astersql_executor_sortexec` 使用该类型。
- 文件内部：`Next` → `Open` → `fetch` → `fetchUnparallel` 或 `fetchParallel`；`Close` → `SortPartition::close` 和 `RowSource::close`。
- 串行下游：`fetchUnparallel` → `RowSource::next`、`DataChunk::memory_usage`、`SortPartition::{newWithKiller,numRows,spillToDisk,add}`、`sortPartitionSource::new`、`newMultiWayMerger(...).collect`。
- 并行下游：`fetchParallel` → `parallelSortWorker::{new,saveChunk}`、`MemoryTracker::exceeded`、`parallelSortSpillHelper::{new,setNeedSpill,spill,mergeAll}`。
- 比较与值模型：`sort_util::{SortKey, comparator, RowComparator, Row, DataChunk}`。`sort_test.rs` 证明降序键仍保持 NULLS LAST；越界键在相关并行测试中表现为不影响有效键次序。

RustCodeGraph 对 `fetchUnparallel` 和 `fetchParallel` 给出了精确符号节点，但本次 `callers` / `callees` 查询没有返回边；因此上述细粒度调用边来自已索引源码中的实际调用点，而不是将空图结果解释为“没有调用者”。RustCodeGraph 曾报告 `sort.rs` 被大量文件关联，但该宽泛“used by”集合含名称级关联，不能都视为真实 `SortExec` 调用者。

## 错误处理与边界

- `Open` 对空排序键返回 `SortError("sort requires at least one ordering item")`；子源打开错误原样经 `?` 传播，且只有成功后才设置 `opened`。
- 子源 `next`、分区 spill/add、worker 保存、helper spill/merge 和多路归并错误均沿 `Result` 返回。并行独立测试 `parallel_sort_propagates_child_error_and_closes_once` 验证子源错误不被吞掉，随后显式 `Close` 只调用一次子源关闭。
- 查询被 kill 时，两个拉取循环立即返回 `query interrupted`；长时间排序不会每次比较都读原子，而由 `SortPartition::sort` 和 `parallelSortWorker::sortBatch` 按比较次数检查点中断。排序 API 不允许可失败比较器，两处实现用私有 panic 哨兵跨越 `sort_by`，只把自己的取消哨兵转换为 `SortError`，其他 panic 会继续展开。
- `Mutex` 中毒统一转换为带具体对象名称的 `SortError`，例如 `sort partition lock poisoned` 或 `parallel sort worker lock poisoned`。
- 串行阈值判断只在当前分区已有行时触发，因此单个大 chunk 即使超过限额仍先加入空分区；这是避免反复拒绝同一 chunk 的边界。`SortPartition::add` 若在已关闭或正在/已经 spill 状态下拒绝，执行器返回统一的 add-chunk 错误。
- `fetch` 只在完全成功后设置 `fetched`。如果中途报错，调用方应 `Close`；在不清理部分分区/worker 状态的情况下直接重试 `Next` 可能重复拉取或混入残留状态，当前接口没有承诺错误后可续跑。
- `Close` 在本地清理完成后返回 `child.close()` 的错误。它没有“只关闭一次”的保护；调用方若重复 `Close`，子源也会再次收到 `close`，是否允许由具体 `RowSource` 决定。

## 并发与资源生命周期

`Arc<AtomicBool>` 让执行器、串行分区和并行 worker 共享取消状态；写入使用 Release，读取使用 Acquire。`Arc<Mutex<...>>` 则允许分区/worker 被 helper 或未来并发调用共享，并把锁中毒转为普通错误。

不过当前 Rust `fetchParallel` 没有启动线程、任务或通道：主线程依次从子源拉取、轮询锁定单个 worker、触发 spill，最后同步归并。它保留了 Go 版多 worker 的数据分片与资源边界，但不是 Go 版 goroutine/channel 并发模型的等价实现。性能分析不能仅凭 `concurrency > 1` 断言实际并行加速。

内存资源由共享 `memTracker` 记账：串行分区在 `add` 时 consume、spill/close 时 release；worker 在 `saveChunk` 时 consume、`multiWayMerge`/`reset` 时 release。磁盘资源由分区或 helper 在创建 run 时 consume；串行分区 `close` 释放自身用量，执行器 `Close` 还会释放追踪器当前剩余总量并丢弃 helper。`Close` 是必要的资源终点；正常读到空 chunk 并不会自动关闭子源或清空追踪器。

`result` 拥有克隆/移动后的最终行，直到逐批弹出或 `Close` 清空。`spillHelper = None` 会丢弃 helper 及其 run；当前 `DiskRun` 的最终清理由其自身析构/实现承担，而本文件显式保证的是磁盘追踪数归零。独立串行测试验证 `Close` 后内存和磁盘 tracker 均为零。

## 与 Go 版本的对应关系

Rust `SortExec` 保留了 Go `pkg/executor/sortexec/sort.go` 的总体语义：物化 child、按多键升降序比较、内存压力下 spill、多个有序分区/run 的多路归并、查询取消、资源追踪以及 `Open`/`Next`/`Close` 生命周期。`sort_test.rs` 的串行 spill 用例明确镜像 Go `TestSortInDisk` 合同；`parallel_sort_test.rs` 的可执行部分覆盖 Go `TestParallelSort` 的复用意图和错误收尾核心合同。

仍存在重要实现差异：

- Go `SortExec` 嵌入 `exec.BaseExecutor`，从 session context 取得 executor concurrency、statement memory/disk tracker、临时存储开关和 SQL killer；Rust 构造器直接接收简化的 `RowSource`、并发度和内存限额。
- Go 并行路径具有 fetcher、多个 worker、result generator、channel、wait group 和 finish channel；Rust 当前同步执行，只借助 worker 列表组织分片排序。
- Go `Next` 流式从 result channel、内存 merger 或磁盘 merger 填充请求 chunk；Rust 先由 `collect()` 形成完整 `VecDeque<Row>`，首行延迟和峰值内存语义不同。
- Go 在 `Open` 中从表达式建立列索引与类型比较函数，并忽略常量表达式；Rust 的 `SortKey` 已是列索引式描述，相关测试把越界列视作全 NULL 常量键，但这不是完整表达式求值替代。
- Go spill 由 statement tracker action、后台协作和 failpoint 覆盖；Rust 以 `memLimit`/`MemoryTracker::exceeded` 同步触发，未接入 Go 的 session OOM action 或 failpoint 框架。
- Go 当前 `Open` 把 `IsUnparallel` 设为 false，生产路径默认并行；Rust 明确用构造参数 `concurrency == 1` 选择串行路径，`physical_plan_runtime.rs` 当前传 1。

因此该 Rust 文件是可执行的排序核心和当前物理计划运行时接线，但不能描述为已经逐项等价移植 Go 的 executor 集成与并发行为。

## 扩展指南

- 若增加排序键语义（collation、新值类型、NULL 排序策略），首先修改 `sort_util.rs` 的 `SortKey`/`comparator`，再同步 `sort_test.rs` 的串行边界测试和 `parallel_sort_test.rs` 的多键/常量键测试；必须保证串行分区、worker 与多路归并共享同一比较器，避免局部有序但全局次序不一致。
- 若改变串行 spill 阈值或分区切换，修改 `fetchUnparallel` 与 `sort_partition.rs`，并覆盖“单个超限 chunk”“恰好达到限额”“空输入”“多分区归并”和 `Close` 后 tracker 归零。测试逻辑应继续放在独立 `*_test.rs`，不要内嵌到生产文件。
- 若实现真实并行，接入点是 `fetchParallel`、`parallelSortWorker` 和 `parallelSortSpillHelper`。必须定义任务所有权、错误聚合、取消传播、锁顺序、关闭等待和 run 生命周期，并对照 Go 的 fetcher/worker/generator 模型；不能只在线程中包裹现有锁循环后宣称等价。
- 若希望降低首行延迟或避免最终结果二次全量驻留，需要把 `result: VecDeque<Row>` 改成可增量读取的 merger/迭代器状态，并相应重写 `fetch`/`Next`。这会影响错误出现时机、tracker 记账和 `Close` 清理，属于跨文件行为变更。
- 若增强错误后重试或执行器复用，需明确复位 `partitions`、helper、result、child 状态和 `killed`。当前 `Close` 不复位 kill 标志，不能沿用普通关闭后的复用假设。
- 若接入更完整的 executor/session 环境，应从 `physical_plan_runtime.rs` 的 `PhysicalSort` 分支和 `Cargo.toml` 依赖边界入手，保留 Go 版 tracker 与 SQL killer 语义；同时评估当前仅在 Windows 条件下声明依赖的 manifest 结构。
- 性能风险集中在全量 `collect()`、行克隆/转成 `DiskRun`、最终再次收集、互斥锁粒度和 `maxChunkSize * 8` 固定批次阈值。任何优化都应同时验证稳定排序要求（若有）、NULL/升降序、spill 前后结果一致性以及取消响应。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/sortexec` 找到 43 个文件，`sort.rs` 被识别为含 29 个符号的 Rust 文件。
- RustCodeGraph 源码/符号：读取 `pkg/executor/sortexec/sort.rs` 全部 289 行；精确查询得到 `sort.rs::fetchUnparallel`（第 112 行）和 `sort.rs::fetchParallel`（第 173 行）。对二者执行 `callers`/`callees` 未返回边，故文档仅用实际源码调用点补足关系并明确图限制。
- 应用入口证据：`pkg/executor/physical_plan_runtime.rs` 第 392—403 行的 `drain_sort`，以及第 1028—1033 行的 `PhysicalSort` 构造路径。
- crate 边界证据：`pkg/executor/sortexec/Cargo.toml` 与 `pkg/executor/sortexec/lib.rs`。
- 直接实现证据：`pkg/executor/sortexec/sort_partition.rs`、`parallel_sort_worker.rs`、`parallel_sort_spill_helper.rs` 和由 `sort.rs` 调用的 `multi_way_merge`/`sort_util` 符号。
- Go 对照证据：`pkg/executor/sortexec/sort.go`，重点核对 `SortExec` 字段、`Open`、`Next`、`Close`、串/并行 fetch、spill 和比较器建立流程。
- 独立测试证据：`pkg/executor/sortexec/sort_test.rs`；`pkg/executor/sortexec/parallel_sort_test.rs` 第 270 行后的可执行 Rust 测试。前者覆盖串行 spill、全局有序、输出批上限、tracker 释放、降序与 NULLS LAST；后者覆盖跨 worker 排序、关闭后重开、子源错误/关闭次数和常量排序项。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核没有把 Go 行为或索引空边误写成当前 Rust 已支持能力。

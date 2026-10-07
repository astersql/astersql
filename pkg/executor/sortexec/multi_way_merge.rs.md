# `pkg/executor/sortexec/multi_way_merge.rs`

## 文件定位

本文件位于 `astersql-executor-sortexec` crate 的排序结果合并层。crate 入口 `pkg/executor/sortexec/lib.rs` 以 `pub mod multi_way_merge` 导出该模块；`pkg/executor/sortexec/Cargo.toml` 将 crate 根指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/executor/sortexec"` 标明其 Go 对照包。

它不负责从 SQL 计划构造执行器，也不定义排序键语义；上游 `sort.rs`、`parallel_sort_worker.rs`、`parallel_sort_spill_helper.rs` 和 `topn.rs` 先生成若干各自有序的分区或 run，再由这里把它们合成一条全局有序行序列。行、比较器、错误和磁盘 run 分别复用 `sort_util.rs` 的 `Row`、`RowComparator`、`SortError`/`Result` 与 `DiskRun`。

## 核心职责

- 用 `multiWayMergeSource` 抽象“多个已排序分区”，使归并算法不区分内存批次、spill 后的 `DiskRun` 和串行路径的 `SortPartition`。
- 用手写最小堆让堆中每个未耗尽分区最多保留一个候选行；每次返回全局最小行，并从同一分区补入下一行。
- 将 source 初始化、分区越界、锁中毒、分区排序和取行错误通过 `Result` 原样传播。
- 提供逐行 `next` 和全量 `collect` 两种消费方式。对于总行数 `N`、非空分区数上限 `k`，建堆为至多 `O(k log k)`，持续归并为 `O(N log k)`，堆额外保存至多 `k` 行；`collect` 另行物化全部输出。

正确性的前提是：每个 source 分区必须已按同一个 `RowComparator` 单调有序。trait 本身无法检查此前提；调用方若传入乱序分区或不一致的比较规则，结果不会保证全局有序。

## 主要符号

- `multiWayMergeSource`：公开 trait，定义 `init(&mut self)`、`next(&mut self, partition_id)` 和 `getPartitionNum(&self)`。`next` 用 `Ok(None)` 表示指定分区耗尽。
- `memorySource`：公开内存适配器。`new(Vec<Vec<Row>>)` 把每个向量转成 `VecDeque<Row>`；`next` 从队首弹行，分区编号越界时报错。
- `diskSource`：公开 spill-run 适配器。`new(Vec<DiskRun>)` 调用每个 run 的 `into_rows()`，再转成队列。名称对应磁盘来源，但当前 Rust 实现在构造时把 run 行物化进内存，并不在归并过程中分块读取磁盘。
- `sortPartitionSource`：公开的共享分区适配器，持有 `Vec<Arc<Mutex<SortPartition>>>`。`init` 逐个加锁并调用 `SortPartition::sort`，`next` 再加锁调用 `getNextSortedRow`。
- `HeapRow`：私有堆元素，绑定一条 `Row` 和来源分区编号，保证弹出后知道应补充哪个分区。
- `multiWayMerger<S>`：公开泛型归并器，保存 source、`Vec<HeapRow>` 最小堆、共享比较器以及幂等初始化标志。
- `newMultiWayMerger`：公开构造函数，只组装空堆，不触碰 source。
- `less`、`push`、`pop`：私有堆操作；分别执行比较、上浮和下沉。
- `init`：公开且在成功后幂等；初始化 source，并从每个非空分区取首行建堆。
- `next`：公开逐行入口；惰性调用 `init`，先读取堆顶所属分区的后继，再移除当前堆顶并视需要补堆。
- `collect`：取得 merger 所有权，循环调用 `next`，把完整结果收集到 `Vec<Row>`。

文件没有模块级常量、条件编译项或 `unsafe` 代码。

## 执行流程

1. 调用方用同一排序比较器准备多个已排序输入，并选择 `memorySource::new`、`diskSource::new` 或 `sortPartitionSource::new`。
2. `newMultiWayMerger` 保存 source 和比较器，创建空堆，保持 `initialized = false`。
3. 第一次调用 `next`（或 `collect` 内第一次循环）触发 `init`：先执行 source 自身初始化，再枚举 `0..getPartitionNum()`，从每个分区取首行并通过 `push` 建立最小堆。空分区不入堆。
4. `next` 查看堆顶而不立即删除它，记住其分区编号，并先调用 `source.next(partition)` 获取后继。若该调用失败，错误直接返回，当前堆顶仍在堆中。
5. 后继读取成功后，`pop` 返回当前全局最小行；若存在后继，`push` 将它作为该分区的新候选。由“每个分区内部有序”和“堆顶是所有当前候选的最小值”共同维持全局有序不变量。
6. 堆为空时 `next` 返回 `Ok(None)`；`collect` 据此结束并返回全部行。

`push` 以父节点 `(i - 1) / 2` 逐级上浮；`pop` 用末元素替换根节点，再选择左右孩子中较小者逐级下沉。比较结果为相等时不会交换，因此只保证排序键顺序，不声明跨分区稳定排序契约。

## 数据与状态

`multiWayMerger` 的核心状态是不超过分区数的候选堆。`HeapRow.partition` 是回到 source 的唯一关联键；分区顺序和 `getPartitionNum` 在一次归并期间必须保持不变。`initialized` 只在 source 初始化及所有首行成功装堆后置为 `true`，成功后的重复 `init` 是空操作。

三种 source 都消费其内部状态：`memorySource`/`diskSource` 的 `pop_front` 会移除行，`sortPartitionSource::getNextSortedRow` 会推进分区游标。因此 merger 是单向流，不能倒退或重复读取。`collect(self)` 消费 merger，避免收集后再次使用同一状态。

`RowComparator` 是 `Arc<dyn Fn(&Row, &Row) -> Ordering + Send + Sync>`；本文件只调用它，不修改排序键。`diskSource` 构造时取得 `DiskRun` 所有权；`sortPartitionSource` 则通过 `Arc` 与外部共享分区所有权，通过 `Mutex` 串行访问每个分区。

## 依赖与调用关系

RustCodeGraph 对 `newMultiWayMerger` 的调用边显示以下生产入口：

- `sort.rs::fetchUnparallel`：用 `sortPartitionSource` 合并串行 Sort 的多个分区。
- `parallel_sort_worker.rs::multiWayMerge`：用 `memorySource` 合并 worker 内已经局部排序的批次。
- `parallel_sort_spill_helper.rs::mergeRuns`：用 `diskSource` 把多个 run 合为一个 run。
- `parallel_sort_spill_helper.rs::mergeAll`：用 `diskSource` 汇总 spill run 与 worker 残留结果。
- `topn.rs::fetchTopN`：用 `diskSource` 合并 TopN spill 候选，然后再应用 Offset/Count。

直接下游依赖为 `sort_partition.rs::SortPartition::{sort,getNextSortedRow}`，以及 `sort_util.rs` 中的 `DiskRun::into_rows`、`Row`、`RowComparator`、`SortError` 和 `Result`。标准库依赖仅有 `VecDeque`、`Arc`、`Mutex` 和 `std::mem::replace`。

RustCodeGraph 的泛名查询会混入仓库其他 `init`/`next`/`Merge` 符号，因此调用关系以上述目标文件限定结果及相邻调用点源码为准；没有把不相关的同名边计入本模块。

## 错误处理与边界

- 零分区、全部为空或完全耗尽时，堆为空，`next` 返回 `Ok(None)`，`collect` 返回空向量。
- `memorySource::next` 和 `diskSource::next` 对非法分区编号分别返回带编号的 `SortError`；不会 panic。
- `sortPartitionSource` 对非法编号、锁中毒、`sort` 失败和 `getNextSortedRow` 失败均返回错误。
- `next` 刻意在 `pop` 前读取后继。`multi_way_merge_test.rs::next_preserves_heap_top_when_source_errors` 注入一次 source 错误，验证首次报错后再次调用仍能返回原堆顶，保持 Go 版本“不因后继读取失败丢当前行”的语义。
- 后继成功后，代码以 `expect` 断言堆仍非空。该断言依赖函数内没有在读取后继与 `pop` 之间修改堆；正常 API 使用下成立。
- `init` 只有完全成功才设置 `initialized`。若 source 初始化或首行装堆中途失败，source 与 heap 可能已部分推进；再次调用 `init` 会重试，trait 没有规定回滚，可能产生重复候选或跳行。新增可重试 source 时必须明确处理这一边界，不能把失败后的 merger 默认视为可安全重试。
- 比较器必须形成适合堆使用的一致全序。文件不捕获比较器 panic，也不验证传递性；相等键之间的稳定顺序未承诺。

## 并发与资源生命周期

归并器本身是同步、单消费者对象：没有内部线程、异步任务或通道。`next(&mut self)` 的独占借用保证同一个 merger 不会被安全 Rust 同时推进。比较器虽因 `Send + Sync` 可跨线程共享，本文件仍只在调用线程执行比较。

`sortPartitionSource` 每次 `sort` 或取行仅在单个分区的 `Mutex` 保护下进行，锁守卫在该调用返回时释放；初始化按向量顺序逐个持有一把锁，不同时持有多把分区锁，因而本文件不形成跨分区锁顺序环。锁中毒转换为 `SortError`。

`memorySource` 与 `diskSource` 拥有其队列，随 merger 被消费或丢弃而释放。`sortPartitionSource` 持有 `Arc` 克隆，merger 生命周期结束只减少引用计数，不强制关闭共享 `SortPartition`。本文件不负责 memory/disk tracker 记账或临时资源清理，这些生命周期由 `SortPartition`、spill helper 及上层执行器管理。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/sortexec/multi_way_merge.go`。两版都具有 `multiWayMergeSource`、`memorySource`、`diskSource`、`sortPartitionSource`、`multiWayMerger` 和 `newMultiWayMerger`，核心算法都是“每分区一个候选的最小堆 + 弹出后从原分区补行”。Rust 的 `Option<Row>` 对应 Go 的空 `chunk.Row` 耗尽标记，`Result` 对应 Go 的 `(row, error)`。

主要实现差异如下：

- Go 用 `container/heap` 和 `multiWayMergeImpl` 实现 `Less/Len/Push/Pop/Swap`；Rust 把上浮、下沉直接实现在 `push`/`pop` 中。
- Go 各 source 的 `init` 直接填充 heap；Rust 统一由 merger 的 `init` 调用 source 初始化并拉取各分区首行。
- Go `memorySource` 保存 `chunk.Iterator4Slice`；Rust 保存拥有所有权的 `VecDeque<Row>`。
- Go `diskSource` 通过 `dataCursor`/`reloadCursor` 按 chunk 读取 `DataInDiskByChunks`，并显式处理重新加载后的异常空行；当前 Rust `diskSource::new` 通过 `DiskRun::into_rows` 一次性物化全部行，因此 IO、峰值内存和错误表面并不等价。
- Go `sortPartitionSource::init` 直接取得每个分区下一已排序行；Rust 会先逐分区调用 `SortPartition::sort`，再由统一首行拉取建堆。
- Go 通常由调用方显式 `init`；Rust `next` 惰性且幂等地调用 `init`。两版都在成功取得后继后才修改堆顶，保留错误时当前候选。

因此该 Rust 文件保留了归并行为主干，但不能据此声称完全复刻 Go 的增量磁盘读取和资源特征。

## 扩展指南

- 新增 source：实现 `multiWayMergeSource`，保证分区数在归并期间稳定、每分区按同一比较器有序、耗尽永久返回 `Ok(None)`，并明确 `init` 或首行读取失败后的重试语义。对应测试应放在独立测试文件，优先扩展 `pkg/executor/sortexec/multi_way_merge_test.rs`，不要内嵌到生产文件。
- 修改堆算法：重点维护“每个活跃分区至多一个候选”“堆顶读取后继失败时不被消费”两个不变量；增加空分区、单分区、多分区相等键、升降序和 source 错误的回归测试。
- 改进磁盘归并：接入点是 `diskSource::{new,next}`。若要贴近 Go 的增量读取，应保留 run/cursor 而非 `into_rows` 全量物化，并同步验证跨 chunk、空 chunk、读取错误、内存峰值和临时资源释放；调用方 `parallel_sort_spill_helper.rs` 与 `topn.rs` 也需评估。
- 改变排序语义：比较规则归属 `sort_util.rs::RowComparator`/`compare_rows`，本文件只维护堆序。任何 NULL、ASC/DESC 或跨类型次序变更，应同步排序工具测试和 Sort/TopN 端到端测试。
- 引入并行预取：当前 `&mut self` 同步协议没有背压或取消接口。若增加线程/任务/通道，需要定义 source 所有权、错误后的候选保留、取消、join 与内存记账，不能只在 `next` 内启动脱管任务。
- 性能风险：增大分区数会提高 `log k` 比较成本；`collect` 和当前 `diskSource` 会物化大量行。优化时必须同时验证顺序、错误语义和内存生命周期，不能以只通过空输入或零测试替代行为证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/executor/sortexec/multi_way_merge.rs --offset 1 --limit 500` 覆盖目标文件 238 行；针对 `newMultiWayMerger` 的 explore/caller 结果确认 `fetchUnparallel`、`multiWayMerge`、`mergeRuns`、`mergeAll`、`fetchTopN` 和回归测试调用边。
- 生产源码：`pkg/executor/sortexec/multi_way_merge.rs`、`lib.rs`、`sort_util.rs`、`sort_partition.rs`、`sort.rs`、`parallel_sort_worker.rs`、`parallel_sort_spill_helper.rs`、`topn.rs`。
- crate 配置：`pkg/executor/sortexec/Cargo.toml`，用于确认 crate 名称、根文件、Go 包映射和依赖边界；依赖表位于 `cfg(windows)` 目标段，本文件本身没有条件编译标记。
- Go 对照：`pkg/executor/sortexec/multi_way_merge.go`，用于核对三类 source、heap 协议、后继读取顺序及磁盘游标差异。仓库同目录未找到直接点名 `multiWayMerger` 的 Go `*_test.go`；Go 行为依据来自实现本身及其 Sort/TopN 调用点。
- 独立 Rust 测试：`pkg/executor/sortexec/multi_way_merge_test.rs`，覆盖后继读取首次失败后堆顶仍可返回；`lib.rs` 通过 `#[cfg(test)] mod multi_way_merge_test` 接入该测试，符合测试与生产源码分离约束。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 个固定章节结构检查，并人工复核本文件没有把 Go 的增量磁盘 IO 描述成当前 Rust 已支持能力。

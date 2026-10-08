# `pkg/util/extsort/disk_sorter.rs`

## 文件定位

本文件是 `astersql-util-extsort` crate 的磁盘外排实现，crate 入口 `pkg/util/extsort/lib.rs` 将它与 `external_sorter.rs` 一并公开。它实现 `ExternalSorter`、`Writer` 和 `Iterator` 定义的“多 Writer 写入 → 一次排序/压缩 → 多 Iterator 有序读取”契约，主要使用方是 Lightning 重复键检测链：`pkg/lightning/duplicate/detector.rs::Detector` 创建 Writer、触发 `sort` 并取得首末键，`pkg/lightning/duplicate/worker.rs::Worker::scan_task` 为扫描区间创建 Iterator。

crate 边界由 `pkg/util/extsort/Cargo.toml` 确认：运行时依赖只有 `serde`、`serde_json` 和 `tokio-util`，其中前两者保存 KV 直方图，`CancellationToken` 承担取消检查。文件没有条件编译项；独立测试由 `lib.rs` 以 `#[cfg(test)]` 引入 `disk_sorter_test.rs` 和 `disk_sorter_1_aster_unit_test.rs`，测试逻辑没有内嵌在生产文件中。

## 核心职责

- `open_disk_sorter` 创建或恢复工作目录：删除遗留 `.tmp`，读取全部 `.sst` 元数据，以最大文件号恢复分配器，并根据 `sorted` 标记选择 WRITING 或 SORTED 状态。
- `DiskSorterWriter` 将调用方切片复制到内存；达到 `writer_buffer_size` 时排序、单文件内去重并原子刷成 pending SST。
- `DiskSorter::do_sort` 按起始 key 排列文件，选择重叠深度过高的文件并执行压缩，最后原子写入 `sorted` 标记并发布只读文件列表。
- `MergingIter` 延迟打开可能覆盖当前最小 key 的文件，提供全局有序、跨文件去重的 `seek`/`first`/`next`/`last` 读取。
- `pick_compaction_files`、`split_compaction_files` 和 `build_compactions` 分别决定“是否压缩、哪些文件一组、每个压缩任务的键区间”，控制读取放大、并行度和单任务大小。

## 主要符号

- `DiskSorterOptions`：并发数、Writer 缓冲、重叠阈值、最大压缩深度和估算大小。`ensure_defaults` 将 0 值及小于 2 的深度恢复为默认值；默认值分别为可用 CPU 数、128 MiB、16、64 和 512 MiB。
- `KvStatsBucket` / `KvStats` / `KvStatsCollector`：按 `key.len() + value.len()` 累积直方图，桶上界为最后一个 key；JSON 保存在 `KV_STATS_PROP_KEY` 对应的属性值中，供压缩范围估算。
- `FileMetadata`：记录 `file_num`、包含式 `start_key`、排他式 `end_key`、`last_key` 和统计信息。`metadata` 用 `last_key + 0x00` 构造尽可能小的排他上界。
- `DiskSorterInner` / `DiskSorter`：`Arc` 共享的目录、选项、原子文件号、原子状态机，以及受 `Mutex`/`RwLock` 保护的 pending/ordered 文件集合。
- `SstWriter` / `write_file_atomic`：要求 key 严格递增，先写 `<number>.sst.tmp`，同步后 rename，再用回调返回元数据。这里的“SST”是本文件定义的 `EXTSORT1` 魔数和长度前缀格式，不是 Pebble SST。
- `SstReader` / `SstReaderPool` / `SstIter`：读取文件、按文件号共享 Reader、引用归零时关闭，并在 Iterator 关闭时执行 `unref`。
- `VecIterator` / `MergingIter`：前者在单文件内二分 seek，后者在多个半开键区间上合并；同 key 并列时以文件下标稳定决定暴露的值，`next` 会推进所有同 key 子迭代器。
- `Compaction`：一个 `[start_key, end_key)` 压缩任务及其重叠输入文件。
- 文件工具函数 `make_filename` / `parse_filename`：产生至少六位数字的 `.sst` 名并从合法后缀解析文件号。

## 执行流程

1. `open_disk_sorter` 选择调用方目录；空路径会创建系统临时目录。它扫描目录、清理 `.tmp`、通过 `read_file_metadata` 重建每个文件的键范围和直方图。存在 `sorted` 文件时将文件按 `start_key` 放入 `ordered_files`，否则放入 `pending_files`。
2. `ExternalSorter::new_writer` 只在 WRITING 状态且 token 未取消时创建 `DiskSorterWriter`。`put` 复制 key/value；新记录将超过预算时先 `flush_inner`。flush 对缓冲按 key 排序、去重、分配新文件号、原子落盘并把元数据加入 pending 列表。
3. `ExternalSorter::sort` 先用 CAS 从 WRITING 切到 SORTING；已排序时幂等返回，已有另一个排序调用时失败。`do_sort` 锁住 pending 列表，循环调用 `pick_compaction_files`，直到重叠深度低于阈值。
4. `compact_files` 先按重叠连通分量和 `max_compaction_depth` 分组，再按直方图估算的 `max_compaction_size` 切为 `Compaction`。每批最多启动 `concurrency` 个 scoped thread；只有当某输入文件的全部压缩引用完成后才删除该文件。
5. `run_compaction` 用 `MergingIter` 扫描任务半开区间，每 1000 条检查一次取消，跨文件去重后写出新文件。全部压缩完成后重新按 `start_key` 排序元数据。
6. `do_sort` 创建并同步 `sorted.tmp`，rename 为 `sorted`，发布 ordered 列表、清空 pending，并以 Release 顺序进入 SORTED 状态。失败时 `sort` 将状态退回 WRITING，允许重试。
7. `new_iterator` 只接受 SORTED 状态。`MergingIter::seek` 打开覆盖目标 key 的文件，`maybe_open_next_files` 仅继续打开起点不大于当前最小 key 的文件；`next` 同时推进当前重复 key 的所有来源；`last` 只需按 `last_key` 倒序找到第一个非空文件。

## 数据与状态

磁盘记录格式为 8 字节魔数 `EXTSORT1`，后跟重复的“小端 u64 key 长度、小端 u64 value 长度、key、value”。`read_file` 会把整个文件解码为 `Vec<KeyValue>`；`write_file_contents` 用 `BufWriter` 写完后调用 `sync_all`。因此当前 Rust 实现的内存占用与单个被读取/压缩文件或压缩输出规模相关，并非流式 Pebble SST 读取。

状态机常量为 WRITING(0)、SORTING(1)、SORTED(2)。WRITING 允许创建和使用 Writer；SORTING/SORTED 禁止继续 `put`。只有 SORTED 允许创建 Iterator。`sorted` 标记是 reopen 时判断最终状态的持久化依据；`.tmp` 是崩溃残留清理边界。`id_alloc.fetch_add(1) + 1` 保证共享 sorter 的 Writer/压缩任务不重复分配文件号。

文件范围统一使用 `[start_key, end_key)`。非空文件的 `end_key = last_key + 0x00`；空文件元数据为 `start_key=[]`、`last_key=[]`、`end_key=[0]`。`ordered_files` 必须按 `start_key` 非降序，`MergingIter::new` 用断言保护该不变量。

## 依赖与调用关系

上游直接证据如下：`pkg/lightning/duplicate/detector.rs::Detector::key_adder` 调用 trait 的 `new_writer`；`Detector::detect_inner` 调用 `sort`；`Detector::get_range_bounds` 调用 `new_iterator` 并读取首末键；`pkg/lightning/duplicate/worker.rs::Worker::scan_task` 为每个并行任务创建 Iterator 并按半开区间 seek/scan。具体 sorter 在 `pkg/lightning/duplicate/detector_test.rs` 和迁移测试中通过 `open_disk_sorter` 构造。

下游内部调用链为：`sort → do_sort → pick_compaction_files → compact_files → split_compaction_files/build_compactions → run_compaction → MergingIter/SstReaderPool → write_file_atomic`。普通读链为 `new_iterator → MergingIter → OpenIter → SstReaderPool::get → SstReader::new_iter → VecIterator`。错误合并依赖 `external_sorter.rs::join_errors`，用于同时保留读取失败和关闭失败。

RustCodeGraph 的文件索引显示本文件被多个模块和测试引用，并成功定位 `disk_sorter.rs::open_disk_sorter`、`pick_compaction_files`、`MergingIter` 节点；本次精确 `callers`/`callees` 查询未返回这些 Rust 节点的调用边，所以上述跨文件关系以 trait 调用处和直接源码引用为准，而不是据空图推断“无调用者”。

## 错误处理与边界

- I/O、整数长度转换和 JSON 错误通过 `external_sorter::Error` 向上传播；魔数错误、截断的 value 长度、关闭后的 Reader/Writer、错误状态 Writer、非法阶段调用和取消均产生明确错误。
- `SstWriter::set` 遇到非严格递增 key 后进入失败态；`close` 删除临时文件且返回原错误。成功路径在同步内容后 rename，避免把半写文件当成正式输入。
- `sort` 在失败后恢复 WRITING，但已经成功产生的新压缩输出或尚未删除的输入可能留在目录中；恢复逻辑会在下次打开时重新扫描正式 `.sst`。只有完成全部排序后才发布 `sorted` 标记。
- `MergingIter` 将子 Iterator 操作错误保存在 `error`，`take_error` 仅转移一次。打开后首次定位失败时，`join_errors` 可同时保留读取错误与清理错误；批量关闭时保留第一个关闭错误。
- `SstReaderPool::unref` 对不存在的文件号直接 panic，表示引用计数协议被破坏；这不是可恢复的外部输入错误。`MergingIter::new` 对未排序元数据同样断言失败。
- `unsafe_key`/`unsafe_value` 只允许在 `valid()` 为真时使用；实现会对无效位置索引并 panic，调用方必须遵守 trait 契约。
- `close` 当前是 no-op；`close_and_cleanup` 才递归删除工作目录，并把目录不存在视为成功。若空路径触发临时目录，调用方仍负责最终调用 cleanup。

## 并发与资源生命周期

`DiskSorter` 可 Clone，因为所有共享状态位于 `Arc<DiskSorterInner>`。文件号与状态分别由 `AtomicU64`/`AtomicI32` 管理；pending 文件列表由 `Mutex` 串行修改，排序期间持有该锁；ordered 列表用 `RwLock` 发布并供后续 Iterator 克隆快照。Writer 自身不共享，但多个 Writer 可在不同线程并发 flush。

压缩通过 `std::thread::scope` 启动受 `concurrency` 限制的批次；每个任务拥有独立 `SstReaderPool` 和输出记录向量。输入文件引用次数先按所有 Compaction 统计，某文件计数归零后才删除，避免重叠 Compaction 提前移除共享输入。线程 panic 会转换成 `external sort compaction worker panicked`。

`SstReaderPool` 的 map 和引用计数由一个 `Mutex` 保护；`get` 在锁内校验并读取新文件，因此正确但会串行化昂贵打开操作。`SstIter::close` 先关闭底层 Iterator，再执行 unref 回调；目前若两步都失败，匹配逻辑只返回底层错误，而 MergingIter 在“定位失败 + close 失败”场景通过外层 `join_errors` 保留两者。调用方应显式 `close` Iterator；类型未实现 Drop 自动归还引用。

取消是协作式的：创建 Writer/Iterator 时立即检查；排序开始前检查；压缩批次之间检查；单个压缩每 1000 条检查。取消 token 不会中断正在进行的一次文件读写或 `sync_all`。

## 与 Go 版本的对应关系

算法和接口结构直接对应 `pkg/util/extsort/disk_sorter.go`：`DiskSorterOptions` 默认值、WRITING/SORTING/SORTED 阶段、Writer 缓冲排序去重、`sorted` 标记恢复、最小打开集多路归并、扫描线选文件、连通分组、直方图切分和每 1000 条取消检查均有同名或蛇形命名对应。`pkg/util/extsort/disk_sorter_test.go` 的测试主题也在 Rust 的 `disk_sorter_test.rs` 中逐项出现。

当前实现仍有重要技术差异，扩展时不能只依据 Go 行号机械修改：

- Go 使用 Pebble `sstable.Writer/Reader`、8 MiB cache、`vfs.FS`，空目录名代表内存文件系统；Rust 使用自定义 `EXTSORT1` 文件、真实 `std::fs`，空路径代表系统临时磁盘目录。
- Go Reader/Iterator 流式访问 SST，Rust `read_file` 和 `run_compaction` 会将完整输入或输出收集进 Vec；大文件内存/性能特征不同。
- Go ReaderPool 在锁外创建 Reader 并处理并发重复打开；Rust 在单一 Mutex 内读取和校验整个文件。
- Go `Sort` 直接写状态，原实现失败后不会显式回退；Rust 用 CAS 防止并发 sort，并在失败后回退 WRITING。Rust 还用 `sorted.tmp` rename 持久化标记，而 Go 直接创建 `sorted`。
- Go 的 `Close` 释放 cache；Rust 没有 cache，因此 `close` no-op。两者的 cleanup 都删除目录/文件系统内容。
- Rust trait 以 `CancellationToken` 替代 Go `context.Context`，并增加 `take_error` 来转移装箱错误所有权；`JoinedError` 对应 Go `errors.Join`。

## 扩展指南

- 修改磁盘格式时应集中处理 `FILE_MAGIC`、`write_file_contents`、`read_file` 和 `read_file_metadata`，提供版本/兼容策略，并在独立 `disk_sorter_test.rs` 增加重开、损坏和截断文件用例；不能只改变 Writer。
- 修改去重优先级或键排序语义时需同时审查 `DiskSorterWriter::flush_inner`、`MergingIter::current_index/next` 和 `run_compaction`。这会影响 Lightning 重复检测的稳定输出，应同步运行 `external_sorter_test.rs` 的公共契约和 duplicate detector/worker 的目标测试。
- 修改压缩策略时应分别扩展 `test_pick_compaction_files`、`test_split_compaction_files`、`test_build_compactions`，覆盖相接但不重叠的半开边界、空统计、相同桶上界、极小/极大阈值，并与 Go 测试表保持一致。
- 修改并发或资源管理时需保持“文件号唯一、pending 发布受锁保护、输入引用归零后删除、所有打开 Iterator 最终 unref”四项不变量；建议优先补充独立测试而不是在生产文件中加入测试模块。
- 若要解决大文件内存放大，接入点是 `SstReader::new_iter`、`VecIterator`、`write_file_atomic` 和 `run_compaction`；替换为流式实现时必须保持 `Iterator` 错误所有权、seek、跨文件去重和原子 rename 契约。
- 增加新的调用方时依赖 `ExternalSorter` trait，而非内部 `Sst*` 类型；生命周期应是关闭全部 Writer 后调用 `sort`，再创建 Iterator，最后 `close_and_cleanup`。

## 验证依据

- 生产源码：`pkg/util/extsort/disk_sorter.rs`（完整 1402 行）、`pkg/util/extsort/external_sorter.rs`、`pkg/util/extsort/lib.rs`。
- crate/config：`pkg/util/extsort/Cargo.toml`，确认 crate 名、入口、依赖和 Go 包迁移元数据。
- Go 对照：`pkg/util/extsort/disk_sorter.go`，核对 Writer、ReaderPool、MergingIter、状态机和三段压缩算法。
- 独立测试：`pkg/util/extsort/disk_sorter_test.rs`、`pkg/util/extsort/disk_sorter_1_aster_unit_test.rs`、`pkg/util/extsort/disk_sorter_test.go`；覆盖公共外排契约、reopen、并发、SST 读写/错误、ReaderPool、归并、压缩和错误合并。
- 上游证据：`pkg/lightning/duplicate/detector.rs` 和 `pkg/lightning/duplicate/worker.rs` 的 trait 调用；测试构造证据见 `pkg/lightning/duplicate/detector_test.rs`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/util/extsort` 返回目标、trait、Go 对照和测试；`node --file` 读取目标全貌；`query` 定位 `disk_sorter.rs::open_disk_sorter`、`pick_compaction_files`、`MergingIter`。精确 callers/callees 查询无输出，该限制已在“依赖与调用关系”中说明并由直接源码搜索补证。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付前以任务给定命令验证恰有 11 个固定二级标题，并人工复查文件角色、流程、边界、Go 差异和扩展入口均有对应路径或符号证据。

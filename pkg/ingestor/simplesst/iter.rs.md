# `pkg/ingestor/simplesst/iter.rs`

## 文件定位

本文件属于 `astersql-ingestor-simplesst` crate；crate 入口 `pkg/ingestor/simplesst/lib.rs` 以 `pub mod iter` 暴露它。它位于简化 SST 管线的读取侧：`KVReader` 解码数据文件，`StatsReader` 解码范围属性文件，本文件在这些单文件 reader 之上提供有序多路归并。

当前 Rust 仓库中，`MergeKVIter`、`MergePropIter` 的直接使用主要在独立测试 `pkg/ingestor/simplesst/iter_test.rs`；RustCodeGraph 将本文件列为被 `pkg/ingestor/globalsort/merge.rs` 等文件使用，但逐符号文本核验没有找到这些构造器在 Rust 生产代码中的调用。相应的完整生产接线目前仍可在 Go 的 `pkg/ingestor/globalsort/merge.go`（`NewMergeKVIter`）和 `pkg/ingestor/globalsort/split.go`（`NewMergePropIter`）看到。因此，本文件应理解为已实现并受测的 Rust 移植 API，而不是已经确认接入全部 Rust ingest 主链的组件。

`pkg/ingestor/simplesst/Cargo.toml` 声明此 crate 的 Go 对照包为 `pkg/ingestor/simplesst`；当前依赖只在 `cfg(windows)` 下列出，文件自身主要依赖 crate 内部模块和 Rust 标准库。

## 核心职责

1. `MergeKVIter` 将多个各自按 key 升序的 SST 数据文件归并为一条全局升序 KV 流，并记录初始未读输入字节数。
2. 在启用热点检测时，`MergeKVIter::rebalance_hotspot` 周期性识别贡献严格超过本周期一半元素的 reader，为其开启并发预取，并在热点变化或消失时关闭旧 reader 的并发模式。
3. `LimitSizeMergeIter<T>` 用权重上限控制同时活跃的数据源数量，同时保持活跃源之间的堆归并顺序；它是 crate 内部的通用、内存化测试实现。
4. `MergePropIter` 将多个 `MultipleFilesStat` 中的统计文件按 `RangeProperty::FirstKey` 归并；每组只保持有限窗口的 reader 活跃，并在后台预开后续统计文件。
5. 所有迭代器把正常 EOF 当作源耗尽，把非 EOF 错误保存在迭代器中，供 `error`/`Error` 查询；显式 `close` 负责停止并发活动并释放 reader。

这些职责都依赖一个前提：每个输入文件自身必须有序；代码只做多路归并，不会检测或修复单文件内部乱序（见文件头说明、`HeapEntry::cmp` 和 `PropertyEntry::cmp`）。

## 主要符号

- `KVPair { Key, Value }`：归并输出的拥有型 KV。`MergeKVIter::key`、`value` 以借用切片暴露当前项。
- `HeapEntry<T>`：KV 最小堆节点，保存排序 key、reader 下标和载荷。其 `Ord` 反转比较以适配标准库最大堆；相同 key 以 reader 下标升序稳定决胜。
- `MergeKVIter`：公开 KV 归并状态机。主要入口为 `new`、`new_with_options`、`next`、`key`、`value`、`error`、`close`、`input_size`；大写方法与 `NewMergeKVIter` 是 Go 风格兼容别名。
- `get_concurrent_reader_concurrency`：用 `reader_memory_size / ConcurrentReaderBufferSizePerConc` 计算单热点 reader 的并发度，并封顶为 256；非正预算或非正单并发缓冲大小得到 0。
- `LimitSizeMergeIter<T>`：crate 私有带权窗口归并器。`open_available` 连续激活新源，且只要已有活跃源就不允许下一源使总权重超过 `limit`；首个源即使权重大于 limit 也会被打开，这是代码当前边界。
- `PropertyEntry`：范围属性堆节点，主排序键为 `FirstKey`，再以外层组下标和组内文件下标决胜。
- `PropertyGroup`：一个 `MultipleFilesStat` 的窗口与预开任务状态；`stop` 发出关闭信号、排空有界队列并 join 后台线程，`Drop` 提供兜底清理。
- `PropertySource`：统计文件路径、组内下标和可选 `StatsReader`。
- `MergePropIter`：公开属性归并状态机。`new` 建立各组窗口和预开线程，`fill_window` 补充活跃 reader，`next` 归并属性并替换耗尽源，`close` 统一停止线程与关闭 reader；`Drop` 自动调用 `close`。

## 执行流程

### KV 归并

1. `MergeKVIter::new_with_options` 检查路径非空，并要求可选 offsets 与 paths 等长。
2. 对每个路径读取对象长度，按 `file_size - offset`（饱和减法）累加 `input_size`，再通过 `KVReader::from_storage` 打开 reader。
3. 构造时预读每个 reader 的首个 KV。首读 EOF 的空文件不入堆；其他项以 `HeapEntry` 入堆。打开或首读出现非 EOF 错误时，函数关闭已经打开的 reader 并立即返回错误。
4. `next` 弹出全局最小项，将其设为 current，再从同一 reader 读取下一项补回堆。该 reader 到达 EOF 时立即关闭并清空槽位；非 EOF 错误写入 `self.error`。
5. 当热点计数达到检测周期，`rebalance_hotspot` 找到唯一可能的严格多数 reader；切换热点时先关闭旧热点并发模式，再尝试为新热点配置和开启并发读取，最后清零周期计数。
6. 堆为空、已关闭或已有错误时，后续 `next` 返回 false。调用者随后应检查 `error`，最后调用 `close`。

### 带权窗口归并

`LimitSizeMergeIter::new` 拒绝空源、源/权重长度不匹配、非正 limit 和非正权重。`open_available` 从 `next_source` 开始连续打开可容纳的源，将每个源首项放入 `BinaryHeap<Reverse<_>>`。`next` 弹出最小项并从同源补项；源耗尽时减去权重并继续打开后续源。源中携带的 `Err` 会被保存，迭代停止。`close` 清空活跃标志、权重和堆。

### 范围属性归并

1. `MergePropIter::new` 拒绝空统计集合，并按 `MultipleFilesStat::MinKey` 排序各组。
2. 每组以 `MaxOverlappingNum + 1`（不大于文件数）作为活跃窗口；当该值非正时允许整组文件同时活跃。文件对使用下标 1 的路径，即 stat 文件路径。
3. 窗口内首批 reader 由 `fill_window` 同步打开；剩余路径交给组内后台线程。后台线程为每个路径启动打开任务，并把一次性接收端放入有界队列。
4. `fill_window` 获取同步或预开结果，首读一个 `RangeProperty` 后放入全局属性堆；空文件直接关闭且不计 active，非 EOF 错误向上返回。
5. `next` 弹出最小属性，再推进其 source。源耗尽时关闭 reader、递减 active、设置 `closed_reader_after_next`，并补充同组窗口；非 EOF 错误记录到迭代器。
6. `close` 先对所有组发布 shutdown，再逐组排空队列和 join worker，随后关闭仍存活的 `StatsReader` 并清空堆。重复关闭是幂等的。

## 数据与状态

`MergeKVIter` 的关键不变量是：每个非空、未耗尽 reader 在堆中恰有一个候选项；`current_reader` 记录最近输出的来源，但当前文件未对外暴露它。`readers` 用 `Option<KVReader>` 表示 reader 是否仍存活，EOF 后槽位变为 `None`。`input_size` 是构造时快照，不随读取递减。

热点状态由 `hotspot_counts`、`hotspot_since_check`、`hotspot_period` 和 `current_hotspot` 组成。只有严格多数（`count * 2 > total`）才成为热点；并发度来自总 reader 内存预算，但当前实现只会给一个热点 reader 启用并发模式。

`LimitSizeMergeIter` 的 `active_weight` 是当前 active 源权重之和；已耗尽或报错源通过 `close_source` 精确减重。由于它按输入顺序打开连续窗口，调用者还必须保证未打开源不会包含小于当前窗口输出的值，否则限制窗口会破坏全局有序性。这与 Go 对属性文件“可从左到右处理”的前置条件一致。

`MergePropIter` 以 `groups[outer].active` 跟踪组内活跃 reader，以 `pending` 跟踪尚未进入窗口的 source。`current` 同时保存属性和 `(outer, inner)` 来源坐标。`closed_reader_after_next` 只描述最近一次 `next` 是否耗尽了某个底层属性 reader，供上层判断文件生命周期。

## 依赖与调用关系

上游 API 面：

- `pkg/ingestor/simplesst/lib.rs` 公开 `iter` 模块，并定义本文件使用的 `Error`、`Result`、`MemoryStorage`。
- Go 生产链中，`pkg/ingestor/globalsort/merge.go::mergeOverlappingFilesInternal` 构造 `NewMergeKVIter`，用 `InputSize` 计算写出分片大小，并以 `Next/Key/Value/Error/Close` 消费结果。
- Go 生产链中，`pkg/ingestor/globalsort/split.go` 构造 `NewMergePropIter`，使用当前属性、reader 坐标和底层 reader 关闭标志推进范围切分。
- 当前 Rust 直接行为证据集中在 `pkg/ingestor/simplesst/iter_test.rs`；未验证到 Rust 生产调用 `NewMergeKVIter` 或 `NewMergePropIter`。

下游依赖：

- `KVReader::from_storage`、`KVReader::next_kv`、`KVReader::enable_concurrent_read`、`switch_concurrent_mode` 和 `close` 实现 KV 读取与热点预取。
- `StatsReader::from_storage`、`next_prop` 和 `close` 实现属性读取。
- `RangeProperty::FirstKey` 是属性排序键；`MultipleFilesStat::{MinKey,Filenames,MaxOverlappingNum}` 决定组排序、文件路径与窗口大小。
- `BinaryHeap` 承担归并选择；`std::sync::mpsc::sync_channel`、线程、`Arc<AtomicBool>` 承担属性 reader 的有界预开和关闭协议。

RustCodeGraph 的文件级结果列出 `pkg/dxf/importinto/task_executor.rs`、`pkg/ingestor/globalsort/merge.rs`、若干测试为使用者；由于常见方法名可能产生粗粒度关联，本说明仅把经符号文本核验的构造器调用当作真实接线证据。

## 错误处理与边界

- 参数错误：KV 路径为空、offset 数量不匹配，以及属性统计为空/组内无文件，均返回 `Error::InvalidData`。`LimitSizeMergeIter` 还验证源、权重和 limit。
- EOF：构造首读或推进时的 `Error::Eof` 表示正常空文件/耗尽；reader 被关闭，错误不会暴露给调用者。
- 损坏或 I/O 错误：KV 推进中的非 EOF 错误保存在 `MergeKVIter::error`；属性推进或补窗错误保存在 `MergePropIter::error`。触发错误的那次 `next` 仍可能已经把刚弹出的 current 返回为 true，调用者必须循环结束后检查错误。
- 构造失败：KV 构造路径尽力关闭先前 reader；属性构造中 `fill_window` 失败会通过局部对象析构触发 `PropertyGroup::drop`，停止已启动任务。
- 关闭错误：两个公开迭代器都收集并返回第一个关闭错误，同时继续清理其余资源；重复 `close` 返回成功。`PropertyGroup::stop` 还把 worker panic 映射为 `InvalidData`。
- 访问边界：尚未定位 current 时，KV 的 `key`/`value` 返回空切片；属性的 `current_property`/`reader_index` 返回 `None`。这意味着空 key 不能单凭切片值区分“合法空 key”和“尚未定位”，调用顺序必须以 `next` 为准。
- 排序边界：重复 key 不在这里去重，堆会按 reader 下标依次返回；重复键策略属于更上层 writer/merge 逻辑。

## 并发与资源生命周期

`MergeKVIter` 本身是同步可变状态机，不提供并发调用保证。热点 reader 内部可以并发预取，但模式切换由 `next` 的单线程流程串行驱动。`close` 先关闭并发模式再关闭 reader，避免预取资源晚于 reader 生命周期；`MergeKVIter` 没有实现 `Drop`，调用者必须显式关闭，否则不能依赖本类型完成全部底层清理。

每个 `MergePropIter` 属性组拥有一个调度 worker，worker 再为待预开路径启动打开线程。队列容量为 `limit.min(count - limit)`；当没有剩余路径时容量为 0，但 worker 不发送任务。关闭时必须先设置所有组的原子 shutdown，随后排空接收队列，再 join worker；排空发生在 join 之前是为了释放可能阻塞在有界队列发送上的生产者。未能发送的成功 reader、关闭信号后的成功 reader以及排队但未消费的 reader都会被显式关闭。

`MergePropIter::Drop` 与 `PropertyGroup::Drop` 都提供兜底关闭。显式 `close` 仍是首选，因为它能把线程 panic 或 reader 关闭错误返回给调用者；析构路径会忽略这些错误。Go 对照还明确规定 `mergePropBaseIter.close` 不应与 `next` 并发，本 Rust 状态机通过 `&mut self` 在安全 Rust 调用层面表达相同排他性。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/ingestor/simplesst/iter.go`，测试位于 `iter_test.go`。

- Rust `HeapEntry`/`MergeKVIter` 对应 Go 通用 `mergeHeap`、`mergeIter` 和外层 `MergeKVIter` 的组合；Rust 将 KV 专用路径直接展开，并保留同名大写别名。
- 两端都按 readerMemorySize 和每并发缓冲大小计算热点并发度，封顶 256；都仅在某一路严格占周期多数时切换并发读取。
- Go `MergeKVIter` 还持有 `membuf.Pool`、设置 merge-sort 读取指标并接受 context/通用对象存储；Rust 当前使用 `MemoryStorage`，没有 context、指标或显式内存池。这些是当前移植边界，不能视为生产等价接线。
- Rust `LimitSizeMergeIter<T>` 使用预装入内存的 `Vec<Result<T>>`，主要用于验证带权窗口算法；Go 版本持有 reader opener 和真正的 reader 生命周期，泛化程度更高。
- Rust `MergePropIter` 把 Go 的两层 `mergePropBaseIter` + `limitSizeMergeIter` 结构展开为 groups/sources/全局堆。两端都按 `MinKey` 排组、按 `MaxOverlappingNum + 1` 打开组内窗口、异步预开剩余 stat reader，并暴露最近是否关闭底层 reader。
- Go 外层属性归并还用各组 `MaxOverlappingNum` 作为权重，并用 `maxMergeSortOverlapThreshold * 2` 限制同时打开的组；Rust 当前为每个统计组立即建立一个组内窗口，没有实现该外层总权重阈值。因此大规模多组场景的连接/内存上界并不完全等价。
- Rust 独立测试覆盖了 Go 的 `TestCorruptContent`、`TestOneUpstream`、`TestAllEmpty`、热点切换、带权归并、空属性文件和半途关闭意图，并额外验证属性预开关闭时的阻塞/错误清理。

## 扩展指南

- 若要把 KV 归并接入 Rust 生产主链，应从 `MergeKVIter::new_with_options` 和调用侧的存储抽象开始：当前签名绑定 `MemoryStorage`，还需明确 context/取消、指标、内存池以及 offset 必传语义。不要只调用四参数 `NewMergeKVIter` 就宣称与 Go 生产构造器等价。
- 修改排序或重复 key 行为时，应改 `HeapEntry::cmp`，并在 `pkg/ingestor/simplesst/iter_test.rs` 增加跨 reader 相同 key、空 key和稳定来源顺序测试；去重策略不应未经设计塞入本层。
- 修改热点策略时，应同步检查 `rebalance_hotspot`、`get_concurrent_reader_concurrency` 和 `KVReader` 并发模式协议，重点回归热点出现、消失、轮换、预算不足、reader 提前 EOF 与 close 后资源释放。
- 修改属性窗口或预开算法时，应同时维护 `PropertyGroup::stop`、`fill_window`、`MergePropIter::next/close`。必须覆盖队列满、打开成功/失败、空文件、worker panic、半途关闭和多组顺序；不能删除“先排空再 join”的死锁规避顺序。
- 若追求 Go 等价的大规模属性归并，需要补上外层组权重上限，而不是仅调整单组 `limit`；相应测试应构造多个 `MultipleFilesStat` 并断言总活跃 reader 上界与全局有序性。
- Rust 测试必须继续放在独立的 `pkg/ingestor/simplesst/iter_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] mod iter_test` 接入，不能内嵌到生产文件。
- 所有变更应重新核对 `pkg/ingestor/simplesst/iter.go` 与 `iter_test.go`，明确哪些差异是已知移植边界，哪些是回归。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ingestor/simplesst` 确认 Rust/Go 源与独立测试；`node --file pkg/ingestor/simplesst/iter.rs --offset 1 --limit 500` 和 `--offset 496 --limit 360` 覆盖本文件全部 800 行，并给出文件级使用者。
- 生产源：`pkg/ingestor/simplesst/iter.rs`（全部符号和流程）、`kv_reader.rs`/`stat_reader.rs`/`codec.rs`/`writer.rs`（由导入及被调用符号确认下游边界）、`lib.rs`（模块、错误、存储）。
- crate 声明：`pkg/ingestor/simplesst/Cargo.toml`（crate 名、入口和 Go package 映射）。
- Rust 测试：`pkg/ingestor/simplesst/iter_test.rs`，覆盖全局顺序、空输入、损坏尾部、单输入、热点切换、输入大小/并发上限、带权窗口、属性 reader 耗尽、异步预开错误和关闭清理。
- Go 对照：`pkg/ingestor/simplesst/iter.go` 与 `iter_test.go`；生产调用证据为 `pkg/ingestor/globalsort/merge.go` 和 `pkg/ingestor/globalsort/split.go`。
- 调用检索：对 `NewMergeKVIter|MergeKVIter|NewMergePropIter|MergePropIter|get_concurrent_reader_concurrency|LimitSizeMergeIter` 在相关 Rust/Go 目录执行精确 `rg`，用于消除 RustCodeGraph 对常见方法名形成的粗粒度关联。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证仅检查目标文件存在、固定十一个二级标题恰好各一个，并人工复核源码链接、当前接线状态与安全扩展说明。

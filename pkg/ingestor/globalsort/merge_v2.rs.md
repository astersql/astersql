# `pkg/ingestor/globalsort/merge_v2.rs`

## 文件定位

本文件属于 `astersql-ingestor-globalsort` crate（见同目录 `Cargo.toml`），由 `lib.rs` 以公开模块 `pub mod merge_v2` 暴露。它提供 V2 重叠文件归并入口 `MergeOverlappingFilesV2`：从若干可能有重叠键范围的 data/stat 文件中，按连续键窗口读出 KV、在内存中排序，再写成一个新的有序 data 文件和对应 stat 文件。

RustCodeGraph 将本文件识别为 8 个符号的 Rust 源文件，并确认 `merge_v2.rs::MergeOverlappingFilesV2` 与 Go 的 `merge_v2.go::MergeOverlappingFilesV2` 同名。对非测试 Rust 源码的调用搜索没有找到调用点；当前可见直接调用均在 `merge_v2_test.rs`、`sort_test.rs` 和 `merge_test.rs`。因此它是已公开且经过测试的归并能力，但不能仅凭本文件断言它已经接入 Rust 生产调度主链。

## 核心职责

- 用 `NewRangeSplitter` 根据 `MultipleFilesStat` 的范围属性，把整个 `[start_key, end_key)` 输入划分为若干连续 ranges group；空的 `end_key_of_group` 是最后一组的哨兵。
- 对每组 stat 文件调用 `get_read_ranges_from_props`，求出起止键在各 data 文件中的近似偏移，再由 `read_all_data` 只加载该半开区间。
- 调用 `MemKvsAndBuffers::build` 展平各文件缓冲，按 `KvPair.key` 对窗口内全部 KV 做升序排序，并依次写入同一个输出流。
- 用 `StreamSummary` 同步累计最小/最大键、行数、字节数和 `RangeProperty`，最后生成 stat 文件和 `WriterSummary`，并可通过 `OnWriterClose` 回调上报。
- 在进入循环和每次读文件期间响应 `CancellationToken`，并拒绝值为零的 `concurrency`。

该实现不会合并或消除相同 key；它只依据 key 排序并原样写出各输入 KV。重复键策略不属于本文件职责。

## 主要符号

- `FOUR_GIB: i64`：固定为 4 GiB。既作为 ranges group 的字节阈值，也作为单次 `read_all_data` 的内存上限；range-job 字节阈值同样传入 4 GiB。键数和 Region 相关阈值传入 `i64::MAX`，所以本入口实际主要按字节切组。
- `MergeOverlappingFilesV2(...) -> Result<String>`：唯一公开函数。成功结果是新 data 文件路径；stat 路径只通过 `WriterSummary` 暴露。参数语义如下：
  - `token`、`multi_file_stat`、`store`、`start_key`、`end_key` 决定取消、输入、存储和半开键范围。
  - `new_file_prefix` 先去掉末尾 `/`，再与 `writer_id` 拼成 `<prefix>/<writer_id>.data` 和 `.stat`。若 prefix 为空，当前格式仍会产生以 `/` 开头的路径。
  - `property_size_distance`、`property_keys_distance` 控制 Go 格式输出 stat 属性的切分频率。
  - `on_writer_close` 在 data/stat 均成功完成后同步调用一次。
  - `_part_size`、`_block_size`、`_write_batch_count`、`_check_hotspot` 当前未使用；前导下划线明确反映尚未接线。
  - `concurrency` 当前只要求大于零，并不传给排序器或读取器。

## 执行流程

1. 若 `concurrency == 0`，立即返回 `Error::InvalidArgument("merge concurrency must be positive")`。
2. 以 4 GiB/极大键数阈值构造 `RangeSplitter`。随后构造唯一输出 data/stat 路径，创建 data writer，并按存储格式和 property 距离初始化 `StreamSummary`。
3. 从 `current_start = start_key` 开始循环。每轮先检查取消，再调用 `SplitOneRangesGroup`。若组结束键为空，使用调用者的 `end_key`；否则使用切分器给出的键作为本轮 `current_end`。
4. 以 `[current_start, current_end]` 两个有序 seek key 和本组 stat 文件求每个 data 文件的起止偏移。Go 大端格式会解析 stat 属性；旧格式直接得到零偏移。
5. `read_all_data` 从各 data 文件的起始偏移读取，只保留 `[current_start, current_end)` 内的 KV，并以 4 GiB 为本轮内存上限。随后 `build` 把逐文件缓冲展平。
6. 对 `loaded.kvs` 按 key 升序排序，逐项调用 `StreamSummary::write` 写进同一个 data writer。`loaded` 是每轮新建的局部值，写完该轮后即释放，避免把所有窗口累积成表级 `Vec`。
7. 推进 `current_start = current_end`。若切分器以空结束键表示耗尽，则退出；否则继续处理下一连续窗口。
8. 成功路径依次关闭 splitter、完成 data writer、写 stat 文件、生成 `WriterSummary`、调用可选回调，最后返回 data 文件路径。

## 数据与状态

- 输入元数据是 `&[MultipleFilesStat]`。`RangeSplitter` 会复制并规范化它：Go 大端格式通过 stat reader 合并属性；旧测试格式可从内联属性或 data 文件派生属性（见 `split.rs::NewRangeSplitter`、`populate_properties`）。
- `current_start` 单调推进；每轮区间为半开区间，上一轮的 `current_end` 恰是下一轮的起点。这一不变量避免相邻窗口重复包含边界键。
- `SplitResult.data_files` 与 `stat_files` 是当前窗口仍活跃的配对文件。`get_read_ranges_from_props` 返回两行偏移，代码分别取 `offsets[0]` 和 `offsets[1]` 作为开始与估计结束偏移。
- `MemKvsAndBuffers` 保存本轮内存数据。`read_all_data` 失败时会清空它；`build` 将 `kvs_per_file` 展平成 `kvs`。排序只保证按 key 非降序；相等 key 的业务含义没有在本文件定义。
- `StreamSummary` 随输出流累计 `min`、`max`、总大小、总数和属性。旧格式写空 stat 内容；Go 大端格式写带 first/last key、size、keys、offset 的属性记录。
- 输出始终是单个 data/stat 文件对，而不是按 `part_size` 分片；这与文件注释“1 writer”一致。

## 依赖与调用关系

上游方面，`lib.rs` 公开 `merge_v2` 模块。RustCodeGraph 的文件关系报告本文件被测试文件使用；仓库精确搜索显示三个直接测试调用面：`merge_v2_test.rs::merge_v2_accepts_zero_write_batch_count_like_go`、`sort_test.rs::test_global_sort_local_with_merge_v2`，以及 `merge_test.rs` 中的流式/Go stat 偏移测试。非测试 Rust 源码中未找到 `MergeOverlappingFilesV2` 调用者，生产接线状态应视为“未验证/当前未发现”。Go 侧 `sort_test.go` 展示了调度式用法，但它不是 Rust 调用边。

主要下游边为：

- `MergeOverlappingFilesV2 -> NewRangeSplitter -> RangeSplitter::SplitOneRangesGroup/Close`（`split.rs`）：维护活跃文件集合并产生连续组边界。
- `MergeOverlappingFilesV2 -> get_read_ranges_from_props`（`reader.rs`）：Go stat 格式下最多启动 64 个 scoped 线程并行读取不同 stat 文件的属性偏移。
- `MergeOverlappingFilesV2 -> read_all_data -> read_one_file`（`reader.rs`）：逐文件读取、解码、范围过滤、取消检查和内存上限检查。虽然会估算读取并发值，当前 `read_one_file` 的 `_concurrency` 尚未实际并行化。
- `MergeOverlappingFilesV2 -> StreamSummary::{write,write_stats,finish}`（`merge.rs`）：编码 data 记录、生成 stat 属性和最终 `WriterSummary`。
- `Storage::{create,record_format}` 及其 writer 的 `finish`：抽象本地或对象存储 I/O；crate 的路径依赖由 `Cargo.toml` 声明，本文件本身只经 crate 内统一类型间接使用它们。

## 错误处理与边界

- `concurrency == 0` 是本文件主动增加的参数错误；`_write_batch_count == 0` 被明确接受，`merge_v2_test.rs` 固化了这一 Go 对齐行为。
- 取消在每组开始时检查，读取属性和 data 时也会检查，返回 `Error::Cancelled`。排序与逐项写出循环内部没有额外取消点，因此很大的单个窗口在这些阶段不会立即响应取消。
- `NewRangeSplitter`、切组、属性 seek、读取、单行写入、writer 完成和 stat 写入的错误均通过 `?` 原样向上传播。`read_all_data` 保证失败时清空部分内存结果。
- 空输入是合法的：仍创建空 data 文件和 stat 文件并返回路径；旧记录格式下 stat 内容为空。对应测试同时验证零 `write_batch_count`。
- 范围语义是 `[start_key, end_key)`；调用者必须提供有序边界。`get_read_ranges_from_props` 会拒绝降序 seek keys。
- 成功路径显式 `Close` splitter 并 `finish` writer；错误提前返回时，本函数没有 Go 版本的 deferred close，也没有调用回调。底层 writer 的 Drop 是否具有额外清理语义不在本文件中验证，因此不能宣称错误路径会完成或删除部分对象。
- 如果 data writer 已完成而 `write_stats` 失败，调用者会收到错误，但 data 文件可能已经存在；本文件没有事务性回滚或删除逻辑。

## 并发与资源生命周期

V2 只持有一个输出 writer，所有窗口依次写入，保证跨窗口输出顺序建立在单调窗口边界之上。窗口内部使用标准切片排序，当前为调用线程上的排序；`concurrency` 不控制它。唯一明确的内部并发来自 `get_read_ranges_from_props`：它对 stat 文件使用最多 64 个 scoped worker，通过原子下标分配文件，通过互斥量汇总列结果和首个错误；scope 结束前所有线程都会 join。

每轮新建 `MemKvsAndBuffers`，写完即离开作用域；内存峰值原则上受 4 GiB 读取限制、KV 容器开销和排序开销共同影响。`RangeSplitter` 跨轮保存属性迭代器、活跃文件集合和累计状态，成功结束时释放迭代器。data writer 与 `StreamSummary` 存活到全部窗口完成，最后依次完成 data、stat 和回调。回调是同步借用调用，函数返回前已经执行完毕。

## 与 Go 版本的对应关系

共同主干与 `merge_v2.go::MergeOverlappingFilesV2` 一致：4 GiB ranges group、`NewRangeSplitter`、连续范围的 stat seek、`readAllData`、窗口内按 key 排序、单 writer 写出，以及结束时上报 summary。Rust 测试还验证了 Go 大端 simplesst wire format 和较晚键窗口从正确属性偏移开始读取。

当前可见差异必须保留为迁移事实：

- Go 用 `sorty.MaxGor = concurrency` 控制并行排序；Rust 只校验非零，随后调用单线程 `sort_by`。
- Go 的 `rangesGroupSize` 可被 `mockRangesGroupSize` failpoint 覆盖；Rust 固定为 `FOUR_GIB`，因此常规小测试通常只覆盖最后一组，未直接逼出多组循环。
- Go writer 应用 `partSize`、`blockSize`、property 距离并通过 builder 安装回调；Rust 只应用 property 距离，始终写单文件，其他对应参数未接线。
- Go 复用 `membuf.Pool` 和 `loaded`，每轮显式清空；Rust 每轮创建新的 `MemKvsAndBuffers`，由作用域释放。
- Go 用 defer 尝试关闭 splitter 和 writer，并把关闭错误合入返回错误，同时记录耗时和错误；Rust 仅在完整成功路径显式关闭/完成，没有日志与耗时指标。
- Go 的 `checkHotspot` 在此函数体中同样没有直接使用；Rust 参数也未使用。`writeBatchCount` 的零值兼容由 Rust 独立回归测试确认。

## 扩展指南

- 若要真正支持排序并发，应修改 `MergeOverlappingFilesV2` 的排序阶段并保持全序结果；同步扩展独立的 `merge_v2_test.rs` 或 `sort_test.rs`，覆盖 `concurrency > 1` 与零值错误，不能只让参数“被读取”。需要评估并行排序额外内存和全局并发控制。
- 若要支持多输出分片、block size 或 part size，应从 writer 创建、`StreamSummary` 以及返回/回调契约整体接入，不能只使用现有下划线参数。需对照 Go writer builder，并验证 data/stat 路径数量、属性 offset、回调 summary 和失败清理。
- 若修改范围切分或内存上限，优先接入一个可测试的 ranges-group 阈值，并新增跨多个 group 的测试，检查相邻 `[start,end)` 无缺失、无重复且前一组内存已释放。
- 若增强取消，应在排序和写循环加入低开销检查，并测试取消后错误身份、回调不触发、部分对象处理策略。
- 若改变 Go wire format/stat seek，必须同步 `merge_test.rs::merge_v2_seeks_go_stat_offsets_for_later_key_window`，并复核 `reader.rs::get_read_ranges_from_props` 与 `merge.rs::StreamSummary::write_stats` 两端编码。
- 测试逻辑应继续放在同目录独立 `*_test.rs` 文件，通过 `lib.rs` 的 `#[path]` 模块接入，不应嵌入本生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ingestor/globalsort/merge_v2.rs` 确认目标文件含 8 个符号；`query MergeOverlappingFilesV2 --kind function` 同时定位 Go/Rust 同名函数；`node --file` 读取并核对目标、`split.rs`、`reader.rs`、`merge.rs` 的真实实现。精确 `callers/callees` 查询在本地索引上挂起且无输出，已停止，未把缺失输出当作调用证据。
- 源与 crate 边界：`pkg/ingestor/globalsort/merge_v2.rs`、`Cargo.toml`、`lib.rs`。
- 下游实现：`split.rs::{NewRangeSplitter,RangeSplitter::SplitOneRangesGroup,RangeSplitter::Close}`，`reader.rs::{get_read_ranges_from_props,read_all_data,MemKvsAndBuffers}`，`merge.rs::StreamSummary`。
- Go 对照：`pkg/ingestor/globalsort/merge_v2.go`；Go 使用场景参考 `sort_test.go`。
- Rust 回归证据：`merge_v2_test.rs` 验证零 write batch 与空输入；`sort_test.rs::test_global_sort_local_with_merge_v2` 验证成组归并后完整读回；`merge_test.rs` 验证流式排序结果及 Go stat offset seek。
- 仓库搜索：`rg` 确认 Rust 非测试源码中没有该函数调用点，并核对所有直接测试调用及模块声明。本文是纯文档分析，按计划未运行 Cargo。

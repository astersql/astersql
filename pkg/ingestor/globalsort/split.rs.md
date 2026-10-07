# `pkg/ingestor/globalsort/split.rs`

源码：[`split.rs`](./split.rs)

## 文件定位

本文件属于 `astersql-ingestor-globalsort` crate；`pkg/ingestor/globalsort/Cargo.toml` 将 `lib.rs` 设为 crate 入口，`lib.rs` 以 `pub mod split` 公开本模块。它位于全局排序产物和后续合并/导入计划之间：输入是外部存储上的 data/stat 文件对及其 `RangeProperty`，输出是连续的 ranges group、组内 range-job 边界和 TiKV Region 预分裂边界。

直接生产调用者有两类。`merge_v2.rs::MergeOverlappingFilesV2` 按本模块给出的 `[current_start, current_end)` 和文件集合分窗读取、排序并写回；`pkg/session/runtime/modify_column_cloud_planner.rs::Planner::cloud_plans` 用 `CalRangeSize` 计算任务粒度，再用 `NewRangeSplitter`/`SplitOneRangesGroup` 生成 ingest 子任务元数据。`testutil.rs::testReadAndCompare` 是共享测试辅助调用者。

## 核心职责

1. `CalRangeSize` 把每核内存预算、Region 目标大小和目标键数换算为 range job 的目标字节数与键数。
2. `NewRangeSplitter` 规范化 `MultipleFilesStat`，为生产 Go 大端 stat 格式建立流式多路归并迭代器；对旧格式或合成夹具，则补齐内联 properties 后排序。
3. `RangeSplitter::SplitOneRangesGroup` 单调扫描属性，同时维护三套独立阈值：ranges group、range job 和 Region split。它还计算当前组实际重叠的 data/stat 文件集合。
4. `PropertyMerge` 只为每个 stat reader 保留一个堆头，以 first key 顺序流式合并多文件属性，避免一次性载入全部 stat 文件。

这里不读取或合并完整业务 KV（旧格式 `populate_properties` 回退除外），也不直接执行 Region split；它只产出后续读取、调度和 split 所需的边界与文件清单。

## 主要符号

- `writeStepMemShareCount: f64 = 6.5`：与 Go 写阶段内存份额约定对应；`CalRangeSize` 以 `memPerCore / 6.5` 作为可用份额。
- `SIZE_OF_SLICE`：用 `size_of::<Vec<u8>>()` 近似一个 Go byte slice header；每个 KV 在 range-job 内存估算中计入两个切片头。
- `CalRangeSize(memPerCore, regionSplitSize, regionSplitKeys) -> (i64, i64)`：内存份额小于 Region 时，把一个 Region 均分为若干 range；否则把 range 大小向下对齐为 Region 大小的整数倍。非法的非正输入返回 `(0, 0)`。
- `SplitResult`：一次分组结果。`end_key_of_group` 是半开上界，空向量表示最终组；两组文件列表按原组/文件下标排序；两组 interior keys 均不包含组端点。
- `RangeSplitter`：有状态、逐次推进的切分器。公开方法为 `Close` 和 `SplitOneRangesGroup`，构造入口为 `NewRangeSplitter`。
- `ExhaustedHeapElem`/`will_exhaust`：通过反转 `Ord` 把标准 `BinaryHeap` 变成按 `last_key` 排序的小根堆，延迟淘汰已经不再与当前范围重叠的文件。
- `PropertyEntry`：把属性与 `(group_index, file_index)` 绑定，并用 `last_for_file` 标记文件的最后一段。
- `PropertyMerge`/`PropertyHead`：生产 stat 文件的 k 路归并器；`readers` 持有流式 reader，`heap` 持有每路当前最小属性，`failed` 保证首次错误后停止迭代。
- `populate_properties`：仅在非 `GoBigEndian64` 记录格式且 `FilePair.properties` 为空时，解码 data 文件并按每个 KV 构造最细粒度属性。

## 执行流程

构造阶段中，`NewRangeSplitter` 先克隆输入元数据。若 `Storage::record_format()` 为 `GoBigEndian64`，`PropertyMerge::new` 为每个 stat 文件打开 `StreamStatsReader`、读取首项并压入堆；否则遍历所有文件，调用 `populate_properties` 补属性，将条目按 `(first_key, group_index, file_index)` 排序。随后初始化三套计数器、待记录边界标志、active 文件表和耗尽堆。

每次 `SplitOneRangesGroup` 的主循环执行以下步骤：

1. 从全局有序属性流取下一项，把其 size/keys 同时累加到 group、job、Region 三套计数器。
2. 如果上一属性是上一文件的最后一段，把该文件及其 `last_key` 放入 `will_exhaust`；再把当前属性所属文件加入 active 表并更新“上一属性”状态。
3. 弹出所有满足 `last_key < current.first_key` 的文件。严格小于很重要：端点相等时两者仍可能在同一边界相接，不能提前排除。
4. 若上一轮已经达到 group 阈值，则当前 `first_key` 成为组上界。方法从 active 表移除已真正耗尽的文件，但返回的是上一轮保存的 active 快照，随后取走本组的 job/Region interior keys。
5. 若 job 或 Region 的“在下一属性记录”标志已置位，把当前 `first_key` 记作相应 interior key。
6. 检查 job 内存/键数和 Region 大小/键数阈值；达到任一阈值时清零对应计数器，并把边界记录延迟到下一属性，确保边界位于属性段之间。
7. 检查 group 大小/键数阈值；达到阈值时保存当前 active 文件快照、清零 group 计数，并在下一属性到来时返回。

属性流耗尽时，方法返回剩余 active 文件、空 `end_key_of_group` 和尚未取走的 interior keys，然后清空 active 表。再次调用会得到文件和边界均为空的最终结果。调用 `Close` 后则立即返回 `Error::Closed`。

## 数据与状态

`ranges_group_*` 决定一次调度/读取窗口的总量；`range_job_*` 决定窗口内部可并行处理的作业边界；`region_split_*` 决定后续 Region 分裂点。三者共享同一属性流但各自计数和重置，不应合并成一套阈值。

`active_data_files` 与 `active_stat_files` 的值保存原始 `(group_index, file_index)`，使 `clone_active_files` 能输出稳定的输入顺序，而不是 `HashMap` 的随机迭代顺序。路径同时是 map key，因此调用方必须保证各文件路径唯一；路径重复会合并条目。

`last_range_property_exhausted_file` 描述“刚消费的属性是否为其文件末段”，而不是当前堆头状态。此一拍延迟与文件淘汰逻辑配合，确保跨文件交错属性时不会把仍与组相交的文件过早移除。`take_range_job_keys` 和 `take_region_split_keys` 使用 `mem::take` 转移结果所有权并清空缓存。

`PropertyMerge` 的空间规模约为 reader 数量加每路一个 heap head；每个 `StreamStatsReader` 每次只解码一个长度前缀属性体。非生产格式回退会把全部派生属性放入 `Vec` 并排序，内存特征不同。

## 依赖与调用关系

上游入口：

- `merge_v2.rs::MergeOverlappingFilesV2 -> NewRangeSplitter -> SplitOneRangesGroup`：取得当前窗口文件和上界，再调用 `reader::get_read_ranges_from_props`、`read_all_data`，窗口排序写出后释放内存。
- `modify_column_cloud_planner.rs::Planner::cloud_plans -> CalRangeSize/NewRangeSplitter/SplitOneRangesGroup`：依据节点资源与 Region 配置计算 range 参数，把 interior keys 加上 start/end 端点后发布 ingest 计划。
- `testutil.rs::testReadAndCompare`：遍历所有 group，读回并拼接 KV，验证切分不会漏读或重复读取。

下游依赖：

- crate 根类型 `MultipleFilesStat`、`FilePair`、`RangeProperty`、`Storage`、`Result` 和 `Error`。
- `reader::StreamStatsReader`：通过 `Storage::open` 流式读取 Go 大端长度前缀 stat 记录，并产生 `FileProperty.range`。
- `Storage::read`、`decode_kvs` 和 `KvPair::encoded_size`：只服务于旧格式/合成数据的 `populate_properties` 回退。
- 标准库 `BinaryHeap` 实现属性多路归并和待耗尽文件的小根堆，`HashMap` 维护 active 文件。

Cargo 清单没有为本文件单独引入第三方 crate；核心类型来自同一 crate，相关 simplesst/存储抽象通过工作区本地依赖接入。

## 错误处理与边界

`NewRangeSplitter` 会传播 stat 文件打开/首项读取错误，以及回退路径的 data 读取或 KV 解码错误。`PropertyMerge::next` 传播后续读取错误；若同一 stat reader 的下一项 `first_key` 小于前一项，则返回 `Error::InvalidData("stat properties are not sorted")`，设置 `failed`，之后不再产出条目。

`Close` 当前只将 `closed` 置位并丢弃迭代器，依靠 Rust drop 释放 reader；它不像 Go 版本那样从底层 `MergePropIter.Close` 返回关闭错误。关闭后的分组调用返回 `Error::Closed`，重复关闭本身成功。

`CalRangeSize` 对三个非正参数显式返回 `(0, 0)`；`NewRangeSplitter` 本身不验证六个阈值。零或负阈值会使 `>= threshold` 很快成立，因此调用者应传入正的实际限制或 `i64::MAX` 表示近似禁用。计数从 `u64` 转成 `i64` 使用 `as`，极端大属性可能回绕；range-job 内存组合使用饱和加法/乘法，但 group 与 Region 的普通加法仍依赖合理元数据范围。

空属性输入合法：首次调用返回空最终组。达到阈值后若没有“下一属性”，不会制造无意义的尾端 interior key；最终组由空 end key 表示。`populate_properties` 只以 key+value 原始字节数估算 size，不含编码头，不能视为生产 stat 精度。

## 并发与资源生命周期

`RangeSplitter` 是带 `&mut self` 游标的单所有者状态机，没有内部锁、线程或异步任务；同一实例必须顺序调用，不能并发推进。每个返回的 `SplitResult` 拥有路径和 key 的副本，后续推进不会修改已经返回的结果。

生产构造时，所有 `StreamStatsReader` 从创建持续到迭代器耗尽、调用 `Close`，或 `RangeSplitter` 被 drop。每次只保留一条当前属性，适合大量 stat 文件，但打开 reader 的数量与文件数线性相关。`Close` 会 `take()` 掉 boxed iterator，立即触发其中 reader 的 drop；本文件不负责删除外部对象或关闭 `Storage` 本身。

`will_exhaust` 中的文件可能跨多次调用保留，直到扫描到严格更大的 `first_key`。active map 则跨组保留仍重叠的文件，只在确认耗尽或最终 EOF 时清理。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/ingestor/globalsort/split.go`：`ExhaustedHeapElem` 对应 `exhaustedHeapElem`，`RangeSplitter` 的三套阈值/计数、active 文件、延迟边界标志和耗尽堆均保持 Go 算法结构；`CalRangeSize` 的 6.5 份额、向上计算 range 数和按 Region 整数倍对齐也一致。Rust 独立测试 `split_test.rs` 对照 Go 的 GeneralProperties、OnlyOneGroup、SortedData、StrictCase、ExactlyKeyNum、3KFiles 和 CalRangeSize 场景。

已确认的实现差异：

- Go 构造器接收 `context.Context`、建立 logger，并委托 `simplesst.NewMergePropIter`；Rust 没有 context/logger，使用本地 `PropertyMerge` 与 `StreamStatsReader`。
- Go 方法以五个返回切片加 error 返回；Rust 用拥有所有权的 `SplitResult` 聚合结果。
- Go `Close` 显式关闭 prop iterator并可能报错；Rust 通过 drop 释放 reader，`Close` 仅改变状态。
- Rust 支持非 `GoBigEndian64` 的旧夹具回退，可用内联 properties 或从 data KV 派生属性；Go 生产实现始终读取 stat iterator。
- Rust `CalRangeSize` 对非正输入增加防御性 `(0, 0)`；Go 版本未作该检查。Rust 的 range-job 内存计算还使用饱和算术。
- Rust 的 `test_3k_files_range_splitter` 在没有 testing-storage URI 时与 Go 默认 CI 一样跳过；即使配置 URI，当前端口仍以 `unreachable!` 明示外部后端尚不可用，不能据此宣称已验证 3000 文件/64 GiB 场景。

## 扩展指南

修改分组算法时应优先保持“阈值在当前属性达到、边界在下一属性 first key 记录”的不变量，否则容易产生空区间或让 interior key 等于组上界。改变文件淘汰逻辑时，要同时检查 `last_for_file`、`will_exhaust` 的严格 `<` 比较和返回 active 快照的时序；对应回归应放在独立的 `split_test.rs`，可扩展 `test_range_splitter_strict_case` 与 `test_exhaustion_follows_previous_property`。

新增 stat 编码必须在 `NewRangeSplitter` 的 record-format 分支和 `reader::StreamStatsReader` 之间明确接线，并补充乱序、截断、空文件和多路同 key 测试。不要让生产格式退化为 `populate_properties` 全量读 data，否则会改变 I/O 与内存边界。

调整 range 内存模型时，应同步 `CalRangeSize`、`SIZE_OF_SLICE`/`SplitOneRangesGroup` 的估算公式、`modify_column_cloud_planner.rs` 的调用假设，以及 Rust/Go 两侧 `TestCalRangeSize`。若引入超大计数，还需明确 `u64 -> i64` 转换和 group/Region 加法的溢出策略。

若增加并行消费，应重新设计整个有状态游标，而不是在现有实例外共享可变引用；还需定义 reader 所有权、结果顺序、错误后的停止语义和 `Close` 竞态。所有 Rust 测试继续保留在 `split_test.rs`，不要嵌入生产源文件。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；目标文件被识别为由 `merge_v2.rs`、`modify_column_cloud_planner.rs` 和一个会话测试使用。
- RustCodeGraph `node --file pkg/ingestor/globalsort/split.rs`：核对全部 478 行、公开 API、内部 heap/iterator 和状态机流程。
- RustCodeGraph `query RangeSplitter`、`query SplitOneRangesGroup`、`query CalRangeSize`：核对 Rust/Go 对应符号及测试符号；精确 callers/callees 命令在当前索引未返回文本，因此调用边又由索引的 `used by` 结果和精确符号引用交叉确认。
- 已读直接证据：`pkg/ingestor/globalsort/Cargo.toml`、`lib.rs`、`reader.rs::StreamStatsReader`、`merge_v2.rs`、`testutil.rs`、`pkg/session/runtime/modify_column_cloud_planner.rs`。
- 已读对照与测试：`pkg/ingestor/globalsort/split.go`、`split_test.rs`，并以 `split_test.go` 的测试入口与调用位置核对 Go 覆盖面。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核源码链接、已知差异及未验证边界。

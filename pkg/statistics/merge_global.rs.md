# `pkg/statistics/merge_global.rs`

源码：[merge_global.rs](merge_global.rs)。本说明只描述当前文件及其直接装配、调用、Go 对照和独立测试证据。

## 文件定位

本文件属于 `astersql-statistics` crate。`pkg/statistics/Cargo.toml` 将 crate 根设为 `lib.rs`；`pkg/statistics/lib.rs` 以 `mod merge_global;` 装配本模块，再以 `pub use merge_global::*;` 导出唯一公开入口 `MergePartTopNAndHistToGlobal`。它把多个分区的 `TopN` 与 `Histogram` 快照合并为一份全局 TopN 和全局直方图，供分区表的全局统计使用。

直接生产调用见 `pkg/statistics/handle/runtime_stats.rs`：列统计传 `is_index = false`，索引统计传 `is_index = true`；调用方把返回的 TopN、桶、空值数和列总大小写回全局统计结构。该函数本身不负责读取存储、调度 ANALYZE 或发布统计，只处理已经规范化的内存统计对象。

## 核心职责

- 汇总所有非空分区直方图的桶内总行数、`NullCount` 和 `TotColSize`，并以第一个非空直方图提供的 ID、类型、版本构造结果（`MergePartTopNAndHistToGlobal`）。
- 将各分区 TopN 按值排序、按相同编码合并计数，并与各分区桶上界的 `Repeat` 流做有序归并，得到每个候选值的全局频次（`BucketCursor::group` 与入口函数的第一遍循环）。
- 用容量为 `num_top_n` 的最小堆选出全局 TopN；晋级值的桶上界重复量从直方图质量中扣除，未晋级的分区 TopN 值则转换成虚拟点桶重新并入直方图（`MergeRefs::mass`、`MergeRefs::merge_virtual`）。
- 在最多 `expected_buckets` 个桶的约束下，从右向左构造近似等深全局直方图；对于跨越切分点的输入桶，依据 `calcFraction4Datums` 的均匀分布假设拆分质量（`MergeRefs::build`）。
- 保持输入快照不变，并在两个可能很长的遍历中周期性响应 `SQLKiller`（公开函数注释、两个循环内的 `HandleSignal`）。

## 主要符号

- `type MergeResult<T> = Result<T, SharedError>`：本文件统一的可共享错误返回类型。
- `BucketRef { hist: u16, bucket: u16 }`：以两个 16 位索引定位某个来源直方图的某个桶。紧凑表示同时决定了分区数、单直方图桶数和虚拟直方图分块的容量边界。
- `Candidate { encoded, count, repeat }`：第一遍归并得到的候选值；`count` 是分区 TopN 计数与桶上界 Repeat 的合计，`repeat` 只记录其中来自桶的部分，便于晋级后从直方图扣除。
- `MergeContext { sc, location, is_index }`：集中提供 Datum 比较、TopN 编码和解码。索引模式直接把 Datum 字节作为编码；列模式使用时区相关的 `codec::EncodeKey`/`topNMetaToDatum`。
- `down`、`up`：可失败比较器版本的二叉最小堆下沉/上浮；同时服务桶游标堆和候选计数堆。
- `BucketCursor`：对每个分区当前最小桶上界做 k 路归并。`first` 跳过质量为零的桶，`advance` 弹出最小引用并推进同一分区，`group` 合并所有相同上界的 Repeat，同时把弹出的引用保存到 `refs` 供第二遍复用。
- `MergeRefs`：第二遍所需的统一引用流及状态。`hists` 用 `Cow<Histogram>` 同时容纳借用的输入直方图和自有虚拟直方图；`promoted` 标识全局 TopN 值；`remaining`/`effective` 保存被切分桶的左侧余量和临时有效上界。
- `MergeRefs::merge_virtual`：将未晋级 TopN 候选按最多 `u16::MAX` 个桶分块成虚拟直方图，再用双指针将虚拟引用与原始有序引用合并。
- `MergeRefs::build`：第二遍等深桶构造器，返回新 `Histogram`。
- `pub fn MergePartTopNAndHistToGlobal(...) -> MergeResult<(Option<TopN>, Histogram)>`：唯一公开 API；输入均为借用切片，返回新对象而不修改分区快照。

## 执行流程

1. 入口断言 `expected_buckets > 0`，检查分区数和单直方图桶数不超过 `u16::MAX`，并寻找第一个非空直方图作为输出元数据模板；完全没有直方图时返回错误。
2. 展平所有分区 TopN。索引以及编码顺序与 Datum 顺序一致的列类型直接按编码字节排序；`ENUM`、`SET`、`BIT` 列则先解码并按 Datum 语义排序，以编码作为平局决胜。相同编码的连续项合并计数。
3. 汇总输入直方图总质量、空值数和列大小；`BucketCursor` 为每个存在正质量桶的分区放入首个引用并建最小堆。若 TopN 流和桶流都为空，直接返回保留元数据的空直方图。
4. 第一遍同时遍历紧凑 TopN 流与桶上界流。两者头部值相等时同时消费，使候选 `count = Σ分区 TopN count + Σ相同上界 Repeat`；否则消费较小一侧。每 1024 次迭代检查一次取消信号。
5. 候选通过按 `count` 排序的有界最小堆保留前 `num_top_n` 个。若请求值等于运行时 `AnalyzeDefaultNumTopN`、候选数又多于容量，则丢弃计数小于 2 的偶然单例；随后构造并按编码排序全局 TopN。
6. 记录晋级编码集合及其桶 Repeat 总量。未晋级且计数大于零的原分区 TopN 项成为虚拟点桶；直方图目标质量等于原桶总质量减去晋级 Repeat，再加回这些未晋级 TopN 计数。
7. `MergeRefs::merge_virtual` 把虚拟点桶并入按上界排序的桶引用流。虚拟桶按 `u16::MAX` 大小分块，避免 `BucketRef.bucket` 溢出。
8. `MergeRefs::build` 从最大上界向最小上界扫描。累计质量达到当前等深阈值、当前桶至少达到目标质量约 80%，且仍需给尾桶留位置时关闭一个结果桶；关闭前合并相同有效上界的引用。
9. 对范围跨越当前切点的更左侧桶，完全落在右侧者整体消费，部分重叠者用 `calcFraction4Datums` 估算右侧质量并在 `remaining`/`effective` 中记录左侧余量。生成的桶先逆序保存，最后反转并以累计 Count 写入输出直方图。

## 数据与状态

输入 `Histogram.Buckets[*].Count` 是累计计数，`BucketCount` 才是单桶质量；输出也通过累计 `cum` 写入。`Repeat` 是桶上界这个点值的频次，不是整个桶质量。全局 TopN 晋级值拥有对应的点质量，因此 `MergeRefs::mass` 检测实际上界编码是否在 `promoted` 中，命中时从桶质量中减去 Repeat 并把 Repeat 清零，避免双重计数。

`BucketCursor.refs` 保存第一遍实际弹出的正质量桶引用，天然按上界有序，第二遍不再重建 k 路堆。桶在重叠扫描中被部分消费时，`remaining[ref]` 表示留给后续左侧结果桶的质量，`effective[ref]` 把该余量的有效上界收缩到切点；再次读取这种余量时 `fresh = false`，确保 Repeat 不会重复归属。

结果沿用第一个非空直方图的 `ID`、`Tp` 和 `LastUpdateVersion`，汇总所有输入的 `NullCount`、`TotColSize`，而输出桶 NDV 固定为 0。结果质量的重要不变量是：直方图非空质量加全局 TopN 总计数等于全部输入桶质量加输入 TopN 计数；结果桶数不超过 `expected_buckets`，每个已输出桶质量为正，且 `0 <= Repeat <= 单桶质量`。这些不变量由 `merge_global_test.rs` 的 `assert_merge_invariants` 和确定性模糊测试直接检查。

## 依赖与调用关系

上游直接关系：`pkg/statistics/lib.rs` 公开重导出本入口；`pkg/statistics/handle/runtime_stats.rs` 的运行时全局统计构建逻辑为列和索引分别准备 `Histogram`/`TopN` 切片后调用它。RustCodeGraph 的文件查询确认目标文件已索引为 28 个符号；精确源码搜索确认 Rust 生产调用集中在上述运行时统计文件，其他 Rust 引用主要是独立测试。

下游 crate 内依赖包括 `Histogram`/`NewHistogram`、`TopN`/`NewTopN`、`TopNMeta`/`topNMetaToDatum` 和 `calcFraction4Datums`。外部依赖由 `pkg/statistics/Cargo.toml` 声明：`stmtctx` 提供比较上下文与时区，`types` 提供 Datum/FieldType，`codec` 负责编码，`collate` 提供二进制排序规则，`sqlkiller` 传播取消，`vardef` 提供动态默认 TopN 容量，`chrono-tz` 保存时区，`astersql-errors` 提供 `SharedError`。

复杂度上，设总 TopN 项数为 T、分区数为 P、被遍历桶引用数为 B：排序约为 `O(T log T)`，第一遍桶归并约为 `O(B log P)`，候选堆更新约为 `O((T+B) log num_top_n)`；第二遍的重叠扫描可能反复查看更左引用，最坏可趋近二次复杂度。内存主要由 TopN 候选、引用流、晋级集合、切分状态和未晋级 TopN 的虚拟直方图占用。

## 错误处理与边界

- `expected_buckets <= 0` 是调用者编程错误，通过 `assert!` 触发 panic，而不是返回 `SharedError`。
- 没有任何 `Some(Histogram)`、分区数超过 `u16::MAX`、单直方图桶数超过 `u16::MAX`，或原分区数加虚拟直方图分块数超过 `u16::MAX` 时返回带上下文的显式错误。
- Datum 比较、TopN 解码、列值编码、分数估算和 `SQLKiller::HandleSignal` 的错误均通过 `?` 原样或包装成 `SharedError` 向上传播。非字节有序类型的排序闭包不能直接返回错误，因此暂存首个比较错误并在排序结束后返回。
- `Option` 输入允许缺失的分区 TopN/Histogram；空但存在的直方图合法，并可为只有 TopN 的列提供类型与元数据。零质量桶由 `BucketCursor::first` 跳过。
- `hist` 与 `bucket` 使用 `u16` 是明确容量契约；虚拟条目必须分块，不能直接把超过 65535 的位置截断。
- 桶重叠拆分依赖桶内均匀分布假设，因此它保证质量守恒而不保证恢复分区内未知的偏斜；宽桶和桶内热点会降低范围估算精度。

## 并发与资源生命周期

函数内部是同步、单线程、无锁计算，没有创建任务、线程或通道。输入通过共享借用读取；原直方图在 `MergeRefs.hists` 中以 `Cow::Borrowed` 保存，虚拟直方图以 `Cow::Owned` 保存，并都随函数调用结束释放。返回的 TopN 和 Histogram 完全由调用者拥有。

取消是唯一外部协作点：第一遍候选归并和第二遍桶构造均在第 0 次及之后每 1024 次主迭代调用 `SQLKiller::HandleSignal`。这降低逐项检查开销，同时意味着取消响应不是每个比较或内部重叠扫描步骤都检查。`StatementContext`、时区及运行时 `AnalyzeDefaultNumTopN` 在调用期间只读取；文件本身不提供跨调用同步，动态默认值的并发可见性由 `vardef` 的原子存取负责。

## 与 Go 版本的对应关系

直接对照为 `pkg/statistics/histogram.go` 的同名 `MergePartTopNAndHistToGlobal` 及其辅助类型/函数。两版都执行相同的两遍结构：展平、排序、压缩分区 TopN；用桶上界 k 路归并合成候选；有界堆选择全局 TopN；把未晋级 TopN 转为虚拟点桶；最后从右向左按等深阈值、同上界合并和重叠切分重建直方图。两版也共享 1024 次一次的取消检查、动态默认 TopN 单例过滤、16 位引用容量限制、质量/Repeat 所有权和 NDV 置零语义。

Rust 将 Go 的 `topNCursor` 直接折叠为紧凑向量索引，把 `bucketGroupCursor` 映射为 `BucketCursor`，把 `globalMergeRefs`/`buildGlobalHistogram` 合并到 `MergeRefs` 及其 `build` 方法；Go 的 `generic.NewBoundedMinHeap` 对应 Rust 的 `Vec<Candidate>` 加 `up`/`down`。Rust 用 `Option` 表示 nil 项、用 `Cow` 混合借用与自有直方图，并克隆 TopN 编码；Go 版本会输出分阶段统计日志，当前 Rust 文件没有等价日志。上述实现形式不同，但核心计数、切分与错误边界由 Rust 独立测试按 Go 测试案例与额外不变量覆盖。

Go `buildGlobalHistogram` 注释还记录一个当前算法限制：只有阈值触发点仍位于结果桶上界组时，第二阶段才会完整重聚同有效上界组；若阈值在更深处触发，同值质量可能跨切点成为桶内质量，降低点等值估算精度。Rust `MergeRefs::build` 采用同一条件，因此扩展时不能把这项精度问题误写为已解决。

## 扩展指南

- 修改候选排序或支持新 Datum 类型时，优先调整入口中的 `byte_ordered` 判定和 `MergeContext::{encode,decode,compare}`；同步扩展 `merge_global_test.rs::type_matrix` 与 `combined_merge_type_universe_and_encoded_order_contract`，尤其验证编码顺序与 Datum 顺序是否一致。
- 修改 TopN 选择策略时，保持候选总数与堆容量的区别、运行时 `AnalyzeDefaultNumTopN` 门控，以及晋级 Repeat 从直方图扣除的所有权规则；同步单例过滤、热点晋级和质量守恒测试。
- 修改桶引用表示时，必须一起审视入口容量检查、`BucketCursor`、`MergeRefs::merge_virtual` 的分块/索引计算以及大规模虚拟条目测试，避免静默整数截断。
- 修改等深切分时，集中在 `MergeRefs::build`、`mass`、`remaining` 和 `effective`；至少验证桶数上限、累计质量、Repeat 边界、相邻范围顺序、重叠质量守恒及 Go 版本同一算法限制。性能变更还应关注重叠扫描的最坏复杂度和 8192 分区规模案例。
- 新回归测试应继续放在独立的 `pkg/statistics/merge_global_test.rs`，由 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 接入，不应把测试写回生产源文件。若改变公开行为，还需同步核对 Go 的 `pkg/statistics/merge_global_test.go`、`merge_global_cases_test.go`、`merge_global_types_test.go` 及 `histogram_fuzz_test.go`，并审视 `handle/runtime_stats.rs` 的列/索引调用契约。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter pkg/statistics/merge_global.rs` 确认目标文件含 28 个符号；`query MergePartTopNAndHistToGlobal --kind function` 定位 Rust 入口与 Go 对照；`node --file ...` 读取完整 614 行实现；`callees MergePartTopNAndHistToGlobal` 确认入口到 `MergeContext`、`BucketCursor`、`MergeRefs`、`NewTopN`、`NewHistogram` 等直接边。名称消歧不足的 `callers`/方法查询没有被用于推断调用关系。
- Rust 源与装配：`pkg/statistics/merge_global.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/Cargo.toml`。
- 生产调用：`pkg/statistics/handle/runtime_stats.rs` 中列统计与索引统计两处 `astersql_statistics::MergePartTopNAndHistToGlobal` 调用。
- Go 对照：`pkg/statistics/histogram.go` 中 `topNCursor`、`bucketGroupCursor`、`flattenSortedTopN`、`selectGlobalTopN`、`globalMergeRefs`、`mergeVirtualTopN`、`buildGlobalHistogram` 和同名公开入口。
- Rust 独立测试：`pkg/statistics/merge_global_test.rs` 覆盖合并计数、Repeat 归属、输入不变、空输入/取消、虚拟分块、动态默认单例过滤、类型矩阵、索引编码路径、重叠切分、容量错误、生产规模和确定性模糊不变量。Go 相关测试证据位于 `pkg/statistics/merge_global_test.go`、`merge_global_cases_test.go`、`merge_global_types_test.go` 与 `histogram_fuzz_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定的 11 个固定二级标题结构检查、链接/路径检查和人工事实复核为验收。

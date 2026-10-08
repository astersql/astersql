# `pkg/statistics/handle/globalstats/topn.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-globalstats` crate，负责把同一列或索引在多个分区上的局部 `TopN` 候选合并成一个全局 `TopN`，并返回未入选的高频项供上层写回全局直方图。crate 入口 `pkg/statistics/handle/globalstats/lib.rs` 以 `pub mod topn` 声明本模块，并通过 `pub use topn::*` 再导出两个公开入口。

应用内的直接生产调用点位于 `pkg/statistics/handle/globalstats/global_stats.rs::merge_partition_stats_to_global`：它收集每个统计项的分区 `Histogram`/`TopN` 后，根据 `MergeOptions.version` 和 `MergeOptions.concurrency` 选择本文件的串行或分批入口，随后把返回的 overflow 计数累加到合并直方图的 `exact_counts`。因此本文件位于“分区统计已加载”与“全局 Histogram/TopN 已定型”之间，不负责存储读取、CMS/FMS 合并或最终持久化。

`pkg/statistics/handle/globalstats/Cargo.toml` 指定 `lib.rs` 为 crate 根，并用 `package.metadata.porting.go-package = "pkg/statistics/handle/globalstats"` 标明 Go 来源目录。清单中大量仓库依赖位于 `target.'cfg(any())'`（恒假条件）下；本文件当前实际只直接使用标准库 `HashMap`、`AtomicBool` 和同 crate 类型。

## 核心职责

- `merge_partition_top_n` 对全部分区执行一个 `[0, len)` 合并任务，适用于统计版本 1 或配置并发度小于 2 的上层分支。
- `merge_global_top_n_by_concurrency` 把分区下标切成有界批次，跨批保留同一累计计数器。尽管函数名和参数保留 “concurrency”，当前实现没有创建线程、任务或通道，而是在调用线程中逐批执行。
- 两个入口都先校验 `TopN` 与直方图分区数一致，把输入克隆进 `StatsWrapper`，再由 `TopNStatsMergeWorker::run_task` 汇总候选频次并从其他分区直方图的 `exact_counts` 移除、补回同值频次。
- 私有 `select_top_n` 统一完成候选排序、前 `n` 项截取、overflow 生成，以及最终 `TopN` 按编码字节升序排列。

本文件不直接解释直方图桶边界。当前 Rust 模型通过 `Histogram.exact_counts` 表示可精确移除的值；真正的移除、版本分支和取消检查在相邻 `merge_worker.rs` 中完成。

## 主要符号

- `pub fn merge_partition_top_n(top_ns: &[TopN], n: usize, histograms: &mut [Histogram], version: i64, cancelled: &AtomicBool) -> Result<(Option<TopN>, Vec<TopNMeta>), String>`：串行公开入口。成功时返回可选全局 `TopN` 和未入选项；工作器成功后才用 `clone_from_slice` 把被移除精确计数后的直方图整体回写给调用者。
- `pub fn merge_global_top_n_by_concurrency(top_ns: &[TopN], n: usize, histograms: &mut [Histogram], version: i64, concurrency: usize, batch_size: usize, cancelled: &AtomicBool) -> Result<(Option<TopN>, Vec<TopNMeta>), String>`：分批公开入口。`concurrency` 当前仅要求大于零；实际批宽由 `batch_size.max(1).min(256)` 决定。
- `fn select_top_n(counter: HashMap<Vec<u8>, f64>, n: usize) -> (Option<TopN>, Vec<TopNMeta>)`：内部选择器。它把浮点累计值钳制到非负后转换为 `u64`，先按“频次降序、编码升序”排序并在 `n` 处分割，再将保留项按编码升序重排。
- `StatsWrapper`、`TopNMergeTask`、`TopNStatsMergeWorker`、`TopN`、`TopNMeta`、`Histogram` 均定义在 `pkg/statistics/handle/globalstats/merge_worker.rs` 并由 crate 根再导出；本文件没有自定义类型、trait、常量、`impl` 或条件编译项。

公开 API 是两个 `pub fn`；`select_top_n` 仅供本模块两条路径共享。固定批次上限 `256` 当前是函数体内字面量，不是本文件的公开常量。

## 执行流程

串行路径 `merge_partition_top_n`：

1. 比较 `top_ns.len()` 与 `histograms.len()`，不一致立即报错。
2. 若所有局部 `TopN::total_count()` 都为零，直接返回 `(None, [])`，不克隆或修改直方图。
3. 克隆两组输入构造 `StatsWrapper`，创建空 `HashMap<Vec<u8>, f64>` 计数器和借用调用方取消标志的工作器。
4. 调用 `run_task(TopNMergeTask { start: 0, end: len }, version)`。工作器逐个局部 TopN 值累加其计数；一个编码首次出现时，还会扫描其他分区，从未包含该值的直方图 `exact_counts` 中移除并补加频次。`version >= 2` 时跳过首次发现值所在分区的直方图。
5. 若响应含错误，原调用方直方图尚未回写，函数直接返回该错误；否则消费工作器取得计数器，将 wrapper 中的直方图回写，再调用 `select_top_n`。

分批路径 `merge_global_top_n_by_concurrency`：

1. 先拒绝 `concurrency == 0`，再校验两组分区数；零分区输入返回 `(None, [])`。
2. 克隆输入，初始化空计数器，把 `batch_size` 钳制到 `[1, 256]`。
3. 依次对 `[start, end)` 批次新建工作器；上一批通过 `result()` 交出的计数器移动到下一批，因此同一编码的“首次出现”判定跨批保持一致。
4. 任一批失败就返回错误且不回写调用方直方图；全部批次成功才整体回写并调用 `select_top_n`。

选择阶段将所有候选转换为 `TopNMeta`。超过 `n` 的尾部成为 overflow；保留部分再按编码升序，以维持 `TopN` 后续二分查找所需顺序。只要候选计数器非空，即使 `n == 0` 也返回 `Some(TopN { values: [] })`，并把所有候选放入 overflow。

## 数据与状态

输入 `top_ns` 是只读切片；输入 `histograms` 是调用方可变切片，但两个入口先操作其克隆，只有完整成功后才一次性回写。这形成近似事务式边界：长度校验、取消或工作器错误不会把部分移除结果暴露给调用方。

累计状态是 `HashMap<Vec<u8>, f64>`：键是统计值的编码字节，值同时包含局部 TopN 的整数计数和从 `Histogram.exact_counts` 补出的浮点计数。`select_top_n` 使用 `count.max(0.0) as u64` 生成最终计数；这会把负数钳制为零，并按 Rust 浮点到整数转换规则截去小数部分。当前上游测试数据使用非负整数语义的浮点数。

关键不变量包括：

- 每个 `TopN` 与同下标 `Histogram` 表示同一分区，故两个切片长度必须相等。
- 同一编码第一次进入全局计数器时才扫描全部分区直方图，避免重复移除和重复补计。
- 频次排名相同时按编码升序决定谁进入前 `n`，使结果确定；最终保留项无论频次如何都按编码升序保存。
- overflow 保持选择排序后的顺序，即频次降序、同频编码升序；上层 `global_stats.rs` 将其计数写回全局直方图。

## 依赖与调用关系

上游生产调用边为：

`merge_partition_stats_to_global`（`global_stats.rs`）→ `merge_partition_top_n`（`version == 1` 或 `concurrency < 2`）/ `merge_global_top_n_by_concurrency`（其余情况）→ `TopNStatsMergeWorker::run_task`（`merge_worker.rs`）→ `Histogram::remove_value`。

两个公开入口随后都调用 `select_top_n`。上层取得 `(top, popped)` 后，把 `popped` 逐项累加到合并直方图 `exact_counts`，并把 `top` 写入 `GlobalStats.top_ns`。crate 根将本模块 API 再导出，因此测试通过 `crate::merge_partition_top_n` 等路径调用。

RustCodeGraph 的文件节点确认 `topn.rs` 有 141 行并识别出 `topn_test.rs` 的直接使用；精确 `query` 唯一定位了两个公开函数。当前索引的 `callers`/`callees` 子命令未返回边文本，因此生产调用边又以 `rg` 对 `global_stats.rs` 的具体调用点复核，不能把图输出缺失解释为没有生产调用者。

下游没有网络、磁盘、异步 runtime 或外部 crate 调用。算法成本主要来自工作器的候选遍历和“首次编码 × 全分区”扫描，以及 `select_top_n` 对全部不同编码的排序；分批入口并未降低总扫描复杂度，也未实际并行。

## 错误处理与边界

- 两个入口都以 `Err("topN/histogram partition count mismatch: ...")` 拒绝切片长度不一致，防止工作器按 TopN 下标访问不存在的直方图。
- 分批入口额外以 `Err("merge concurrency must be positive")` 拒绝零并发；大于零的具体值目前不控制执行并行度。
- `batch_size == 0` 被提升为 1，大于 256 被压到 256，不报错。
- `TopNStatsMergeWorker::run_task` 可返回 `"invalid TopN merge task range"` 或 `"query interrupted"`；本文件原样传播字符串。当前两个入口自己构造的范围均合法，范围错误主要是工作器防御性边界。
- 串行入口以“所有 TopN 总计数为零”判空；分批入口只对“分区数量为零”提前判空。如果存在非空分区列表但所有 TopN 都空，分批路径仍会完成任务并调用 `select_top_n`；计数器为空时当前实现返回 `Some(TopN { values: [] })`。这是两条路径可观察到的边界差异。
- `n` 可以是零或大于候选数：前者返回空但存在的 TopN（候选非空时）与全量 overflow，后者保留全部候选。
- 出错前对 wrapper 克隆所做的直方图移除不会写回调用方；成功时才用 `clone_from_slice` 提交全部变化。

## 并发与资源生命周期

本文件不产生真正并发。`merge_global_top_n_by_concurrency` 的名称沿袭并发合并接口，但函数内部是普通 `while` 循环；没有线程池、锁、channel、future 或 task。`concurrency` 除零值校验外不参与批次数或调度计算，批宽由调用方传入的 `batch_size` 独立决定。上层 `global_stats.rs` 当前用 `(partition_count / concurrency).clamp(1, 256)` 计算该参数，因此并发度只通过调用方间接影响批宽。

取消使用共享 `AtomicBool`。工作器以 `Ordering::Acquire` 在每个分区、每个 TopN 值和扫描每个其他分区时轮询；本文件只借用标志，不设置或拥有它。取消是协作式的，并在检查点返回字符串错误，不涉及线程 join 或清理回调。

`StatsWrapper` 拥有输入的完整克隆，工作器在一个批次内独占可变借用它。每批结束用 `worker.result()` 消费工作器、释放该借用并转移计数器，下一批才能重新借用 wrapper。函数退出时临时 wrapper 和计数器按 RAII 释放；成功回写会克隆 wrapper 中的直方图元素，因此峰值内存至少包含调用方直方图与工作副本。

## 与 Go 版本的对应关系

Cargo 元数据把整个 crate 对应到 Go 包 `pkg/statistics/handle/globalstats`，但当前 Go 主线已没有与这两个 Rust 入口同名的一对一实现。`global_stats.go` 和 `global_stats_async.go::dealHistogramAndTopN` 都调用 `pkg/statistics/histogram.go::MergePartTopNAndHistToGlobal`，由该函数联合处理分区 TopN 和真实直方图桶；Go 基准 `topn_bench_test.go` 也直接覆盖该联合实现。Rust 当前仍采用 `merge_worker.rs` 的简化 `exact_counts` 模型和串行/分批入口，因此只能视为相同业务目标下的迁移实现，不能声称已与最新 Go 算法、桶语义或并行行为完全同构。

选择器语义可与 `pkg/statistics/cmsketch.go` 对照：Go 的 `GetMergedTopNFromSortedSlice` 先用 `SortTopnMeta` 按计数降序、同频编码升序排列，截取前 `n`，再用 `TopN.Sort()` 把保留项按编码排序。Rust `select_top_n` 保持该顺序约束，并与 Go 一样允许 `n == 0` 时对非空候选构造一个空 TopN 对象而不是返回 `nil`。

Rust `topn_bench_test.rs` 保留了旧式串行/“并发”包装和 256 批次上限，用于验证本 crate 当前接口；Go 的同名基准文件已经改测重叠键与互斥键两种输入下的 `MergePartTopNAndHistToGlobal`。扩展时应以当前 Go 联合算法为兼容基线，而不是从 Rust 函数名推断 Go 仍有对应工作池实现。

## 扩展指南

- 若改变候选聚合、版本处理或直方图补计规则，应首先修改 `merge_worker.rs::TopNStatsMergeWorker::run_task`，并同步 `topn_test.rs` 中跨分区重复键、从直方图补计和取消用例；不要只改 `select_top_n` 掩盖计数来源问题。
- 若改变排名或 TopN 存储顺序，应修改 `select_top_n`，同时保持“选择按频次、存储按编码”的两阶段约束，并同步 `merged_topn_is_encoded_sorted_after_frequency_selection`、零上限和同频稳定性测试。破坏编码顺序可能影响依赖二分查找的消费者。
- 若要实现真实并发，需重新设计 wrapper/直方图的独占修改与计数器归并；当前跨批共享计数器保证每个编码只做一次全分区补计。并行拆分若各自把同一编码当作首次出现，会重复移除或重复累计。还应重新定义 `concurrency` 与 `batch_size` 的关系，并增加确定性、取消、部分失败不回写和多批重叠键测试。
- 若要追平最新 Go `MergePartTopNAndHistToGlobal`，范围会跨越本文件和当前简化 `Histogram` 数据模型，应先对照 `pkg/statistics/histogram.go` 的两阶段联合算法及 `merge_global_*_test.go`，不能把真实桶合并简化为 `exact_counts` 删除。
- 新增 Rust 测试应继续放在独立的 `topn_test.rs` 或 `topn_bench_test.rs`，不要内嵌到生产文件。至少覆盖空输入、全空 TopN、长度不匹配、不同 `version`、批宽上下界、取消、同频排序、`n == 0`、`n` 大于候选数和失败时调用方直方图不变。

性能风险集中于全量克隆、每个首次候选扫描所有分区和全排序；兼容风险集中于 Go 主线已演进到联合桶算法以及串行/分批路径的全空输入差异。任何优化都应先固定这些可观察语义，再用重叠键和互斥键输入分别评估。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/statistics/handle/globalstats` 确认目标、模块、测试和 Go 对照文件；`node --file .../topn.rs` 读取完整 141 行并显示测试使用关系；`query merge_partition_top_n` 与 `query merge_global_top_n_by_concurrency` 均唯一定位到本文件。`callers`/`callees` 命令没有输出边文本，故调用边由下列源码检索补证。
- 生产源码：`pkg/statistics/handle/globalstats/topn.rs`；直接上游 `global_stats.rs::merge_partition_stats_to_global`；直接下游 `merge_worker.rs::{TopNStatsMergeWorker::run_task, Histogram::remove_value}`；模块装配与再导出 `globalstats/lib.rs`。
- crate 边界：`pkg/statistics/handle/globalstats/Cargo.toml` 的包名、`lib.rs` 路径、Go 包迁移元数据及恒假依赖分组。
- Rust 测试：`topn_test.rs` 覆盖重复键累加、无/有直方图、长度不匹配、取消、最终编码排序和零上限；`topn_bench_test.rs` 覆盖串行与分批包装、批宽计算及工作池参数校验辅助逻辑。
- Go 对照：`pkg/statistics/handle/globalstats/global_stats.go`、`global_stats_async.go`、`topn_bench_test.go`；选择器语义来自 `pkg/statistics/cmsketch.go::{SortTopnMeta, GetMergedTopNFromSortedSlice}`；当前生产联合算法位于 `pkg/statistics/histogram.go::MergePartTopNAndHistToGlobal`。
- 使用 `rg` 复核两个 Rust 入口只在 `global_stats.rs`、Rust 单元测试和 Rust 基准辅助代码中出现，并确认 Go 全局统计调用已转向 `MergePartTopNAndHistToGlobal`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另行执行任务指定的 11 章节结构命令，并人工复核唯一新增生产物、无 Rust/Go/Cargo/`plan.md` 修改。

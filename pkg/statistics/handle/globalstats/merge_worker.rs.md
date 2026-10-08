# `pkg/statistics/handle/globalstats/merge_worker.rs`

## 文件定位

[`merge_worker.rs`](./merge_worker.rs) 位于 `astersql-statistics-handle-globalstats` crate，crate 入口 [`lib.rs`](./lib.rs) 通过 `pub mod merge_worker` 声明模块并以 `pub use merge_worker::*` 再导出其公共类型。该文件提供一套轻量的分区 TopN 合并数据模型，以及按分区下标区间累计候选值频次的工作器；直接上游是同 crate 的 [`topn.rs`](./topn.rs)，再上一层是 [`global_stats.rs`](./global_stats.rs) 中的全局统计合并流程。

当前仓库存在两条需要区分的实现链：

- 本 crate 的 `merge_partition_stats_to_global` 使用本文件的 `TopNStatsMergeWorker`，属于简化的 Rust 全局统计模型。
- 实际 Go 同路径的同步和异步入口已经调用 `pkg/statistics` 中的 `MergePartTopNAndHistToGlobal`；其 Rust 对应实现在 `pkg/statistics/merge_global.rs`，不是本文件。当前 Go 主线同目录已不存在 `merge_worker.go`。

因此，本文件不是当前 Go 生产主链的逐行镜像；它保留了旧版 Go TopN 工作器的核心“跨分区 TopN 累加并从直方图补数”语义，供本 crate 的 Rust API 和独立测试使用。

## 核心职责

文件承担三类职责：

1. 定义合并所需的最小统计结构：`Bucket`、`Histogram`、`TopNMeta`、`TopN`、`CmSketch`、`FmSketch` 和 `StatsWrapper`。
2. 定义任务/响应协议：`TopNMergeTask { start, end }` 表示半开区间，`TopNMergeResponse { error }` 以可选字符串传递失败。
3. 由 `TopNStatsMergeWorker::run_task` 将任务区间内各分区 TopN 的计数汇入 `counter`；一个编码值第一次出现时，还扫描全部分区，从“不含该值 TopN”的分区直方图 `exact_counts` 中移除并补加精确频次。

关键不变量是：每个编码值的直方图补数只在它首次进入 `counter` 时执行一次。后续在其他分区 TopN 再次遇到同一编码值，只累加该 TopN 项本身的 `count`，避免重复从直方图扣除。

## 主要符号

- `Bucket { count: i64, ndv: i64 }`：简化的直方图桶，只保留累计行数与桶内 NDV；本文件不直接读写其字段。
- `Histogram { id, ndv, buckets, exact_counts }`：简化直方图。`exact_counts: HashMap<Vec<u8>, f64>` 是工作器真正访问的精确值频次表。
- `Histogram::remove_value(&mut self, value: &[u8]) -> f64`：从 `exact_counts` 消费一个编码值；不存在时返回 `0.0`，使调用者无需区分缺失与零频次。
- `TopNMeta { encoded, count }`：一个 TopN 候选值及其无符号频次。
- `TopN { values }`：分区 TopN 容器。`total_count` 汇总频次；`contains` 线性判断某编码值是否已由该分区 TopN 覆盖。
- `CmSketch::merge`：按编码键相加计数。它不参与 `run_task`，而由上层全局统计合并复用。
- `FmSketch::merge`：用较大 NDV 合并估计值。它同样是本文件提供给上层的统计基元，不参与工作器循环。
- `StatsWrapper { histograms, top_ns }`：按分区下标对齐的直方图和 TopN 列表。`run_task` 假定两者可用相同 `part` 下标访问。
- `TopNMergeTask { start, end }`：工作范围 `[start, end)`。
- `TopNMergeResponse { error }`：成功时为默认值 `error: None`；失败时保存固定错误文本。
- `TopNStatsMergeWorker { wrapper, counter, cancelled }`：持有对可变统计快照的独占借用、跨批次累计表以及协作取消标记。
- `TopNStatsMergeWorker::run_task(task, version)`：核心入口；校验任务范围、累计 TopN、补取并移除直方图频次。
- `TopNStatsMergeWorker::result(self)`：消费工作器并转移 `counter` 所有权，供上层选择最终 TopN。

除标准库导入外，文件没有条件编译项、trait 或模块级常量。所有上述结构和方法都是 `pub`；字段也公开，调用者可以自行构造测试或合并输入。

## 执行流程

`run_task` 的执行顺序如下：

1. 验证区间。若 `start > end`，或 `end` 超出 `wrapper.top_ns.len()`，立即返回 `"invalid TopN merge task range"`。
2. 遍历 `index in start..end`。每个分区开始前以 `Ordering::Acquire` 读取 `cancelled`；为真则返回 `"query interrupted"`。
3. 克隆当前分区的 `TopN.values`。这使后续对 `wrapper.histograms` 的可变操作不会与对 `top_ns[index]` 的借用冲突，代价是复制编码字节及候选列表。
4. 对每个 `TopNMeta` 再检查一次取消标记。
5. 先判断编码值是否首次出现，再将当前 TopN 计数以 `f64` 加入 `counter`。
6. 若该编码已出现，直接处理下一个候选；该值的跨分区直方图补数已由首次遇见者完成。
7. 若首次出现，遍历全部分区 `part`，循环内继续检查取消标记。
8. 当 `version >= 2 && part == index` 时跳过候选来源分区；无论版本如何，只要 `top_ns[part].contains(encoded)` 为真，也跳过该分区，避免把已由 TopN 表示的值再次从直方图计入。
9. 对剩余分区调用 `histograms[part].remove_value(encoded)`，把返回频次累加进同一 `counter` 项。即使值不存在，返回零也保持流程一致。
10. 全部任务完成后返回默认成功响应。上层随后调用 `result` 取得累计表。

串行入口 `topn::merge_partition_top_n` 创建覆盖全部分区的单个任务。`topn::merge_global_top_n_by_concurrency` 虽然名称含 “concurrency”，当前实现实际按 `batch_size` 顺序创建多个工作器，并在批次之间搬移同一 `counter`；参数 `concurrency` 只做非零校验，没有创建线程。

## 数据与状态

`StatsWrapper.top_ns` 与 `StatsWrapper.histograms` 以相同分区顺序组织。`run_task` 只显式验证任务范围相对于 `top_ns` 合法，没有验证两组向量长度相等；正常入口在 `topn.rs` 中先检查长度一致。

`counter` 的键是拥有所有权的 `Vec<u8>`，值是 `f64`：

- TopN 的 `u64` 频次通过 `as f64` 转换后累加。
- 直方图 `exact_counts` 本身也是 `f64`，可直接累加。
- 上层 `select_top_n` 最终以 `count.max(0.0) as u64` 转回整数，再按频次降序选择全局 TopN。

直方图修改是消费式的：`remove_value` 从 `exact_counts` 删除键。成功返回后，调用者看到的 `wrapper.histograms` 已不再包含被提升为全局 TopN 候选的精确值。`topn.rs` 会把修改后的直方图克隆回调用方切片；未进入最终 TopN 的候选则作为 overflow 由更上层写回合并直方图。

`Bucket`、`Histogram.id/ndv/buckets`、`CmSketch` 和 `FmSketch` 是为上层合并提供的状态，但本工作器只操作 `top_ns`、`exact_counts`、`counter` 和 `cancelled`。

## 依赖与调用关系

直接依赖均来自标准库：

- `std::collections::HashMap`：保存精确频次、Sketch 计数与工作器累计表。
- `std::sync::atomic::{AtomicBool, Ordering}`：读取协作取消信号。

模块与调用链由源码确认如下：

```text
global_stats::merge_partition_stats_to_global
  ├─ version == 1 或 concurrency < 2
  │    └─ topn::merge_partition_top_n
  │         └─ TopNStatsMergeWorker::run_task
  └─ 其他情况
       └─ topn::merge_global_top_n_by_concurrency
            └─ TopNStatsMergeWorker::run_task（逐批顺序调用）

TopNStatsMergeWorker::run_task
  ├─ AtomicBool::load
  ├─ TopN::contains
  └─ Histogram::remove_value
```

`Cargo.toml` 将该目录定义为独立库 crate，`lib.rs` 是 crate 根。其大量业务依赖位于 `[target.'cfg(any())'.dependencies]`，该配置恒假，因而本文件当前实际只依赖标准库；普通 `[dev-dependencies]` 仅列出 `astersql-planner-core`。RustCodeGraph 对 `run_task` 的精确 callers/callees 查询未返回边，因此上述调用关系以同 crate 源码中的显式调用为准。

当前 Go 主链的直接入口证据是 `global_stats.go` 与 `global_stats_async.go` 对 `statistics.MergePartTopNAndHistToGlobal` 的调用；它们不调用本文件或本 crate 的 `TopNStatsMergeWorker`。

## 错误处理与边界

`run_task` 只产生两类错误字符串：

- 非法任务范围：`start > end` 或 `end > top_ns.len()`。
- 查询取消：任一取消检查观察到 `true`。

边界行为如下：

- `start == end` 合法，循环为空并成功返回。
- 空 TopN 值列表不会修改状态；上层串行入口还会在所有 `total_count() == 0` 时提前返回 `None`。
- `Histogram::remove_value` 对不存在的键返回零，不把缺失视为错误。
- `version >= 2` 会跳过候选来源分区的直方图；`version < 2` 仍可能检查来源分区，但若其 TopN 包含该值，`TopN::contains` 分支同样会跳过。
- 直接构造 `StatsWrapper` 且 `histograms.len() < top_ns.len()` 可能在 `histograms[part]` 处 panic；公开上层入口通过长度检查规避此风险。
- `u64 -> f64 -> u64` 在极大计数下可能损失整数精度；当前实现没有溢出或精度诊断。
- `TopN::contains` 是线性扫描，首次遇见的每个不同候选都可能扫描所有分区及各自 TopN，输入很大时存在明显 CPU 成本。

取消并不回滚：若在处理中途观察到取消，先前已经写入 `counter` 或从 `exact_counts` 删除的值仍保留。上层收到错误后应丢弃该次工作数据，不能把部分结果当作成功结果继续提交。

## 并发与资源生命周期

`TopNStatsMergeWorker` 通过 `&mut StatsWrapper` 获得独占可变借用，所以单个实例不能被多个线程同时调用。`counter` 也没有互斥保护。当前 `merge_global_top_n_by_concurrency` 只是顺序分批执行，不存在实际并行 worker、任务通道或响应通道。

`cancelled` 是共享 `AtomicBool` 引用。工作器使用 Acquire 读取，允许外部线程发布取消信号；检查点位于每个任务分区、每个 TopN 候选和每个被扫描分区的循环入口，因此取消延迟取决于单次克隆、哈希操作与 `contains` 扫描耗时。

资源生命周期由所有权明确限定：

- `TopN.values.clone()` 产生的临时候选在该分区循环结束后释放。
- `remove_value` 立即释放哈希表中被移除的键和值。
- `result(self)` 消费工作器，使对 `StatsWrapper` 的可变借用结束，并把累计表转移给调用方。
- `topn.rs` 在批次模式下将 `counter` 移入下一个工作器，从而跨批次保留去重状态。

旧版 Go `topnStatsMergeWorker` 曾由多个 goroutine 共享，使用全局 `mu` 保护 counter、按直方图分片的 `shardMutex` 保护删除，并用 task/response channel 协调。当前 Rust 文件没有移植这些并发原语；如果未来真正并行化，不能直接共享现有 `&mut StatsWrapper` 和裸 `HashMap`。

## 与 Go 版本的对应关系

最接近的逐符号对照来自提交 `a17d9ca122` 的父版本 `pkg/statistics/handle/globalstats/merge_worker.go`：

- Go `StatsWrapper.AllHg/AllTopN` 对应 Rust `StatsWrapper.histograms/top_ns`。
- Go `TopnStatsMergeTask.start/end` 对应 Rust `TopNMergeTask.start/end`。
- Go `TopnStatsMergeResponse.Err` 对应 Rust `TopNMergeResponse.error`。
- Go `topnStatsMergeWorker.Run` 的核心循环对应 Rust `run_task`：累加 TopN，同值首次出现时扫描其他分区，跳过已含该值的 TopN，并从直方图移除后补计数。
- Go `Result` 对应 Rust `result`。

但 Rust 版本是有意简化的数据模型，不能视为完全等价移植：

- Go 通过 `DatumMapCache`、列类型、时区和 `isIndex` 解码值，再用 `EqualRowCount` 与 `BinarySearchRemoveVal` 操作真实直方图；Rust 直接查询 `exact_counts`。
- Go worker 消费 channel 任务并支持多个 goroutine；Rust 接受单个值任务并同步执行。
- Go 使用互斥锁保护共享 counter 与分区直方图；Rust 依赖独占借用，没有锁。
- Go 通过 `SQLKiller.HandleSignal` 返回具体错误；Rust 只读取布尔值并生成固定字符串。
- Go 的工作器类型和字段多为包内私有；Rust 类型及字段全部公开。

当前 Go 主线在 `a17d9ca122`（`statistics: replace separate TopN merge with combined TopN+histogram merge`）之后删除了同目录 `topn.go`/`merge_worker.go`，改由 `pkg/statistics/histogram.go::MergePartTopNAndHistToGlobal` 在两阶段算法中同时合并 TopN 与直方图。当前 Rust 的对应生产实现是 `pkg/statistics/merge_global.rs::MergePartTopNAndHistToGlobal`。所以本文件反映的是历史 Go 算法和本 crate 现有简化 API，而非当前 Go 算法的完整状态。

## 扩展指南

安全扩展时优先按目的选择接入点：

- 改变单个候选跨分区补数规则：修改 `TopNStatsMergeWorker::run_task`，同步覆盖 `topn_test.rs` 中“从 histogram 补回并移除”的用例。
- 改变精确值在直方图中的存储或删除方式：修改 `Histogram` 与 `remove_value`，并检查 `global_stats.rs` 对 `exact_counts` 的合并和 overflow 写回。
- 改变最终 TopN 排序、截断或 overflow：修改 `topn.rs::select_top_n`，而不是把选择逻辑塞入工作器。
- 增加真正并发：需要重新设计 `StatsWrapper` 分区所有权或引入细粒度同步，同时保证“首次补数”判断与 counter 插入原子化；还需明确任务出错后的取消、汇总和部分状态废弃策略。
- 对齐当前 Go 生产算法：应优先评估复用 `pkg/statistics/merge_global.rs::MergePartTopNAndHistToGlobal`，避免继续扩展这套历史简化模型。

测试必须放在独立文件，不要内嵌到 `merge_worker.rs`。直接回归面是同目录 [`topn_test.rs`](./topn_test.rs)；至少覆盖非法区间、空区间、取消发生在各层循环、重复键只补一次、v1/v2 分支、直方图缺键、向量长度不匹配的入口防护，以及批次间 counter 延续。若改动要声称与当前生产统计等价，还必须同步检查 `pkg/statistics/merge_global_test.rs` 及对应 Go 的 `pkg/statistics/merge_global*_test.go`。

兼容性风险主要是计数重复/漏计、直方图与 TopN 同时保留同一质量、编码排序不变量被破坏，以及错误后暴露部分状态；性能风险主要来自克隆 `TopN.values`、线性 `contains` 和“不同候选数 × 分区数 × 分区 TopN 长度”的扫描复杂度。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/statistics/handle/globalstats`：确认目标文件、模块入口、`topn.rs`、`global_stats.rs` 与独立测试文件均在索引中。
- RustCodeGraph `node --file pkg/statistics/handle/globalstats/merge_worker.rs`：读取目标文件 1–168 行，确认全部 18 个符号及实现。
- RustCodeGraph `node --file .../topn.rs` 与 `node --file .../global_stats.rs`：确认 `run_task` 的两条直接调用路径及更上层合并分支。
- RustCodeGraph 对目标 `run_task`/`TopNStatsMergeWorker` 的精确 `callers`、`callees` 查询：未返回静态边；调用关系随后由上述源文件显式调用补证。
- [`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)：确认 crate 名称、恒假目标依赖、模块声明和公共再导出。
- [`topn_test.rs`](./topn_test.rs)：确认重复键累加、无直方图、有直方图补数/删除、长度不匹配、取消、编码排序与 `n == 0` 等边界。
- 当前 Go [`global_stats.go`](./global_stats.go) 和 [`global_stats_async.go`](./global_stats_async.go)：确认生产入口已转用 `statistics.MergePartTopNAndHistToGlobal`。
- `git show a17d9ca122^:pkg/statistics/handle/globalstats/merge_worker.go` 与同版本 `topn.go`：确认历史 Go 工作器的字段、锁、通道和循环语义。
- `pkg/statistics/merge_global.rs` 与当前 `pkg/statistics/histogram.go`：确认当前 Rust/Go 生产算法的真实位置。

本任务只生成说明文档，未运行 Cargo 或代码测试。结构验证要求目标文件存在，且上述十一个固定二级标题各出现一次。

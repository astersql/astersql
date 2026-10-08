# `pkg/statistics/estimate.rs`

## 文件定位

本文件属于 `astersql-statistics` crate 的采样统计构建路径，负责把样本中的不同值数量（NDV）和 singleton 数量外推为全表 NDV。模块由 [`pkg/statistics/lib.rs`](lib.rs) 通过 `mod estimate` 装配，并以 `pub use estimate::*` 重导出其中可见符号；crate 与 Go 包 `pkg/statistics` 的对应关系记录在 [`pkg/statistics/Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 中。

它不是独立的统计收集入口。生产调用位于 [`pkg/statistics/cmsketch.rs`](cmsketch.rs)：`NewCMSketchAndTopN` 先从样本构造 `topNHelper`，再调用本文件的 `calculateEstimateNDV`，其结果继续参与 CMSketch 默认频数和 TopN 的构建。对应 Go 实现在 [`pkg/statistics/estimate.go`](estimate.go)，而辅助结构在 Go 中定义于 [`pkg/statistics/cmsketch.go`](cmsketch.go)。

## 核心职责

- `dataCnt` 保存一个编码值及其在样本中的频数，是 `topNHelper.sorted` 的元素类型。
- `topNHelper` 汇总样本不同值、样本量、singleton 数以及 TopN 构建所需的附加状态；本文件的估算逻辑只读取 `sorted`、`sampleSize` 和 `singletonItems`。
- `calculateEstimateNDV` 处理“全是 singleton”“没有 singleton”两个特殊分支，并为混合分布调用 GEE 估算器，同时返回样本到全表的整数缩放比。
- `EstimateNDVByGEE` 实现 Charikar 等人的 GEE 公式，将估计值四舍五入后夹在样本 NDV 与全表行数之间。

该文件只做确定性的纯计算，不采样、不编码值、不构造 CMSketch，也不保存跨调用状态。

## 主要符号

- `pub(crate) struct dataCnt { data: Vec<u8>, cnt: u64 }`：crate 内部样本频数记录。`data` 是列值或索引键的编码字节，`cnt` 是样本内出现次数。结构派生 `Clone` 与 `Debug`，但不定义比较或排序规则；排序由 `cmsketch.rs::newTopNHelper` 完成。
- `pub(crate) struct topNHelper`：crate 内部构建上下文。`sorted` 按频数降序保存样本不同值；`sampleSize` 是样本行数；`singletonItems` 是频数为 1 的不同值数；`sumTopN` 与 `actualNumTop` 供后续 TopN 启用和截取使用，当前文件不读取后二者。
- `pub(crate) fn calculateEstimateNDV(&topNHelper, u64) -> (u64, u64)`：crate 内部编排函数，返回 `(estimated_ndv, scale_ratio)`。`sample_ndv` 直接取 `helper.sorted.len()`。
- `pub fn EstimateNDVByGEE(sample_ndv, singleton_items, sample_size, row_count) -> u64`：公开 GEE 算法入口，经 `lib.rs` 重导出。命名保留 Go API 风格；它要求样本量和样本 NDV 非零，并要求 `row_count >= sample_ndv`。

## 执行流程

生产主链为 `cmsketch.rs::NewCMSketchAndTopN → newTopNHelper → estimate.rs::calculateEstimateNDV → EstimateNDVByGEE`：

1. `NewCMSketchAndTopN` 对空表或空样本提前返回；随后把 `row_count` 提升到至少 `sample.len()`，建立本文件所依赖的非空与行数下界。
2. `newTopNHelper` 对编码字节计频、统计 singleton、稳定排序，并计算 TopN 附加字段。
3. `calculateEstimateNDV` 计算 `sample_size`、`sample_ndv`、`singleton_items`，以及整数除法 `scale_ratio = row_count / sample_size`。
4. 若 `singleton_items == sample_size`，样本每行都是不同值，函数将列视为近似唯一，直接返回 `(row_count, 1)`，避免再按比例放大频数。
5. 若 `singleton_items == 0`，认为不同值已由样本覆盖，返回 `(sample_ndv, scale_ratio)`。
6. 其余混合分布进入 GEE：`sample_ndv + (sqrt(row_count / sample_size) - 1) * singleton_items`。浮点结果加 `0.5` 后转为 `u64` 实现正数范围内的四舍五入，再先取不小于 `sample_ndv`，并在 `row_count > 0` 时取不大于 `row_count`。
7. 调用方使用 NDV 和缩放比计算 `default_value`，再将高频值放进 TopN、其余值写入 CMSketch；这部分不在本文件内。

## 数据与状态

所有输入都通过值或共享引用传入，函数不修改 `topNHelper`。`Vec<u8>` 的所有权由 `dataCnt` 持有，但估算阶段只读取向量长度和计数字段，不读取或复制编码内容。

`scale_ratio` 是 `u64` 整数除法而非浮点倍率。正常生产入口已保证 `row_count >= sample_size > 0`，因此比例至少为 1；若绕过入口直接传入不满足约束的 helper，`calculateEstimateNDV` 本身不会完整验证这些条件。NDV 的中间公式使用 `f64`，最终结果恢复为 `u64`；精度和舍入语义是与 Go 实现对齐的一部分。

`sumTopN` 和 `actualNumTop` 虽定义在本文件中，却属于同一 helper 的后续消费状态：前者决定是否启用 TopN，后者决定从 `sorted` 中抽取多少项。扩展估算器时不应误认为它们当前参与 GEE。

## 依赖与调用关系

本文件没有 `use` 声明，计算只依赖 Rust 标准库提供的 `Vec`、整数/浮点运算、`f64::sqrt`、`max` 和 `min`。因此 `Cargo.toml` 没有为该文件单独引入外部算法依赖。

直接上游是 [`pkg/statistics/cmsketch.rs`](cmsketch.rs) 的 `newTopNHelper` 与 `NewCMSketchAndTopN`。RustCodeGraph 对 `calculateEstimateNDV` 的 callee 解析出 `EstimateNDVByGEE`；对后者未发现进一步函数调用边。由于 Go 与 Rust 存在同名符号，RustCodeGraph 的 callers 查询有名称歧义，使用仓库搜索复核后，生产 Rust 引用仅见 `cmsketch.rs`，测试引用见 [`pkg/statistics/estimate_test.rs`](estimate_test.rs) 与 [`pkg/statistics/statistics_aster_unit_test.rs`](statistics_aster_unit_test.rs)。

直接下游结果进入 `cmsketch.rs::calculateDefaultVal` 与 `buildCMSAndTopN`：估计 NDV 决定未充分观测值的默认频数分母，缩放比决定重复样本和 TopN 频数如何放大。因此公式或边界变化会影响优化器消费的统计数据，即使本文件本身不接触优化器。

## 错误处理与边界

`EstimateNDVByGEE` 不返回 `Result`，而用 `assert!` 拒绝三类编程错误：`sample_size == 0`、`sample_ndv == 0`、`row_count < sample_ndv`。断言消息与 Rust 测试的 `#[should_panic]` 期望一致。`calculateEstimateNDV` 在计算 `scale_ratio` 时先做除法，所以零 `sampleSize` 会先触发整数除零 panic；生产入口通过空样本提前返回避免该路径。

合法输入的结果满足 `estimated_ndv >= sample_ndv`，并在正的 `row_count` 下满足 `estimated_ndv <= row_count`。全 singleton 分支固定返回缩放比 1；无 singleton 分支保留整数缩放比。当前实现没有显式检查 `singleton_items <= sample_ndv`、`sample_ndv <= sample_size` 或 `sorted` 的排序/计数一致性，这些是 helper 构造方维护的不变量。

与 Go 版本存在一个重要失败语义差异：Go 在 `intest.Assert` 之后仍对零 `sampleSize` 或零 `sampleNDV` 防御性返回 0，便于关闭内部断言的构建继续运行；Rust 使用始终生效的 `assert!`，非法输入总是 panic。扩展或对外暴露调用路径时必须保留或明确调整这一差异，不能把 panic 当作可恢复错误。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或 I/O。两个函数都只在调用栈上计算；共享引用 `&topNHelper` 仅在调用期间借用，返回值不引用输入。

堆资源只存在于调用方拥有的 `topNHelper.sorted` 和各 `dataCnt.data` 中，本文件既不分配这些集合，也不转移或释放它们。由此，函数可被不同线程分别调用，但线程安全性最终取决于调用方如何共享 helper；本文件没有全局可变状态。

## 与 Go 版本的对应关系

[`pkg/statistics/estimate.go`](estimate.go) 与 Rust 文件在两个函数的分支、GEE 公式、加 `0.5` 舍入、样本 NDV 下界和全表行数上界上逐项对应。Rust 的 `dataCnt` 对应 Go 中 `histogram.go` 的同名结构，`topNHelper` 对应 Go 中 `cmsketch.go` 的同名结构；Rust 为便于模块可见性将这两个辅助结构放到了 `estimate.rs`，并由 `cmsketch.rs` 导入。

Go 的 `calculateEstimateNDV` 由 `cmsketch.go::NewCMSketchAndTopN` 调用，Rust 保持相同接线。Go 的 `TestEstimateNDVByGEE` 位于 [`pkg/statistics/cmsketch_test.go`](cmsketch_test.go)，Rust 对应测试独立放在 `estimate_test.rs`，符合源码与测试分文件的仓库约定；`statistics_aster_unit_test.rs` 另有范围性质的补充检查。

已确认的差异包括：Rust 对非法零值始终 panic，而 Go 在非 `intest` 构建可走防御性返回；Rust `newTopNHelper` 对同频数据始终用编码字节作次级排序，Go 仅在 `StabilizeV1AnalyzeTopN` failpoint 下强化该顺序。后一差异发生在上游 helper 构造，不改变本文件公式，但可能影响 TopN 项目的顺序，不应归因于 GEE。

## 扩展指南

- 修改 GEE 公式、舍入或夹紧规则时，首先同步 `EstimateNDVByGEE`，并在独立的 `estimate_test.rs` 增加表格边界；同时对照 `estimate.go` 和 `cmsketch_test.go`，避免无意偏离 Go 行为。
- 修改全 singleton、无 singleton 或缩放策略时，调整 `calculateEstimateNDV`，并补充 helper 级测试。应覆盖 `row_count/sample_size` 的整数截断以及两个特殊分支返回的比例。
- 新增 helper 字段时，要同步检查 `cmsketch.rs::newTopNHelper` 的构造、`calculateDefaultVal`、`buildCMSAndTopN` 及 Go 同名结构；不要在本文件内嵌测试，继续使用 `estimate_test.rs`。
- 若需要把非法输入改为可恢复错误，API、所有调用点和 Go 兼容语义都会变化；应显式设计 `Result`/错误类型，而不是仅删除断言。
- 性能上该文件的公式为常数时间；`sorted.len()` 不遍历数据。不要为估算器引入对 `sorted` 或 `data` 的重复扫描，样本计频和排序已由上游完成。
- 兼容性复核应关注 NDV 对 `calculateDefaultVal` 分母以及 TopN/CMSketch 频数放大的连锁影响，而不能只验证公式返回值。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`explore "pkg/statistics/estimate.rs ..."` 读取目标文件及符号；`query calculateEstimateNDV` 确认 Go/Rust 同名定义；`callees` 确认 Rust `calculateEstimateNDV → EstimateNDVByGEE`，且 GEE 无项目内下游调用。
- 源码与装配：[`pkg/statistics/estimate.rs`](estimate.rs)、[`pkg/statistics/lib.rs`](lib.rs)、[`pkg/statistics/cmsketch.rs`](cmsketch.rs)、[`pkg/statistics/Cargo.toml`](Cargo.toml)。
- Go 对照：[`pkg/statistics/estimate.go`](estimate.go)、[`pkg/statistics/cmsketch.go`](cmsketch.go)、[`pkg/statistics/histogram.go`](histogram.go)。
- 测试证据：[`pkg/statistics/estimate_test.rs`](estimate_test.rs) 覆盖公式结果、四舍五入/上下界、三个 panic 前置条件以及两个特殊分支；[`pkg/statistics/statistics_aster_unit_test.rs`](statistics_aster_unit_test.rs) 补充估计范围；[`pkg/statistics/cmsketch_test.go`](cmsketch_test.go) 是 Go 公式与非法输入的对照测试。
- 按任务约束，本次为纯文档分析，未运行 Cargo 或代码测试；完成判据使用任务指定的 11 节结构检查，并人工复核文件定位、运行链、边界和扩展点均有上述源码证据。

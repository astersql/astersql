# `pkg/executor/aggfuncs/func_stddevsamp.rs`

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate（crate 根为 [`lib.rs`](./lib.rs)），由 `lib.rs` 以 `pub mod func_stddevsamp` 公开。它是 Rust 侧 `STDDEV_SAMP`（样本标准差）的最终数值计算内核：复用 [`func_varpop.rs`](./func_varpop.rs) 已累计或合并好的方差状态，再对样本方差开平方。

文件刻意很薄：它不读取表达式、不遍历 executor 的行、不分配或合并部分结果，也不把结果写入 chunk。`builder.rs` 已能把 `FunctionName::StddevSamp` 区分为普通、original DISTINCT、partial DISTINCT 三种 `AggImplementation` 元数据变体，但 RustCodeGraph 对本文件两个函数的调用者查询只找到测试，未找到生产代码把这些函数接入 `AggFunc::append_final_result_to_chunk`。所以当前可确认的是“计算内核和构建元数据存在”，不能据此声称 Rust 生产执行主链已完整接通。

## 核心职责

- `stddev_sample`：对普通 `VarianceState` 调用 `sample_variance()`，仅在至少有两个有效值时取得以 `N-1` 为分母的样本方差，然后用 `f64::sqrt` 得到样本标准差。
- `stddev_sample_distinct`：对 `DistinctVariance` 做同样的最终计算；去重、NULL 过滤和跨 partial result 合并均由该状态类型负责，而非本文件负责。
- `StddevSamp4Float64` 与 `StddevSampDistinctFloat64`：为上述两类部分结果提供领域名称，使调用方能够表达 STDDEV_SAMP 用的是普通方差状态还是 DISTINCT 方差状态。它们只是类型别名，不引入新布局或新行为。

本文件不定义模块级常量、结构体、trait、`impl`、条件编译项或私有函数；四个顶层项全部是公开 API。

## 主要符号

- `pub fn stddev_sample(state: &VarianceState) -> Option<f64>`：只借用状态，不修改它。其实现等价于 `state.sample_variance().map(f64::sqrt)`；`None` 保持为 `None`，`Some(variance)` 才开平方。
- `pub fn stddev_sample_distinct(state: &DistinctVariance) -> Option<f64>`：同样只借用状态。`DistinctVariance::sample_variance()` 会先通过 `state()` 把去重集合重建为普通 `VarianceState`，再计算样本方差。
- `pub type StddevSamp4Float64 = VarianceState`：普通 Float64 样本标准差的部分结果别名；底层字段为 `count`、`sum`、`variance`，定义在 `func_varpop.rs`。
- `pub type StddevSampDistinctFloat64 = DistinctVariance`：DISTINCT Float64 样本标准差的部分结果别名；底层以 `HashMap<u64, f64>` 保存去重值，并单独维护 NaN 键负载计数。

## 执行流程

普通路径的完整状态流程由相邻模块共同组成：

1. 调用方创建默认 `VarianceState`；初始 `count`、`sum`、`variance` 均为零。
2. `VarianceState::update` 跳过 `None`，对有效 `f64` 增加计数和总和，并从第二个值起用在线公式更新可合并的中间方差量。
3. 并行或分区聚合时，`VarianceState::merge` 把 source 合入 destination，并用两个分区均值的差修正跨分区方差。
4. 最终调用 `stddev_sample`。`VarianceState::sample_variance` 在 `count <= 1` 时返回 `None`；否则返回 `variance / (count - 1)`。
5. `stddev_sample` 对该值开平方并返回 `Some(f64)`。

DISTINCT 路径先由 `DistinctVariance::update` 过滤 NULL 并按浮点键语义去重，`merge` 将另一部分结果的值重新插入目标集合；`stddev_sample_distinct` 最终触发“去重集合 → 临时 `VarianceState` → 样本方差 → 平方根”的流程。该设计保证跨 partial result 的重复值仍只参与一次计算。

## 数据与状态

`VarianceState` 的不变量由 `func_varpop.rs` 维护：`count` 只统计非 NULL 输入，`sum` 是这些值的和，`variance` 是可增量更新、可跨分区合并的中间量，最终除以 `count-1` 才成为样本方差。本文件不复制这些字段，只读取派生结果，因此不会改变部分结果生命周期。

`DistinctVariance` 使用浮点位模式作为普通值的键，显式让 `+0.0` 与 `-0.0` 共用键；NaN 每次插入都分配新键，以贴合 Go map 对 NaN 不相等的行为。它在求值时重新遍历 `HashMap`，所以遍历顺序不稳定，浮点舍入的最低位可能受累加顺序影响；现有本地测试对固定小样本使用精确值或 `f64::EPSILON` 容差。

两个类型别名不创建包装类型，因而其尺寸、所有权、`Send` 能力和方法集合完全继承底层状态。普通状态为固定大小；DISTINCT 状态的内存随唯一键数量增长。本文件没有单独的内存记账逻辑。

## 依赖与调用关系

直接下游只有 crate 内部的 `crate::func_varpop::{VarianceState, DistinctVariance}`、各自的 `sample_variance()`，以及标准库 `f64::sqrt`。目标文件本身不直接使用 `Cargo.toml` 中的外部 crate；`Cargo.toml` 将其归入包 `astersql-executor-aggfuncs`，并通过 `[lib] path = "lib.rs"` 装配。

模块入口 `lib.rs` 公开 `func_stddevsamp`，并在 `#[cfg(test)]` 下装配独立测试 `func_stddevsamp_test.rs`。RustCodeGraph 查询显示：

- `stddev_sample` 的直接调用者是 `func_stddevsamp_test.rs` 中三个测试。
- `stddev_sample_distinct` 的直接调用者包括 `func_stddevsamp_test.rs`、`func_distinct_agg_test.rs` 和 `go_scenario_coverage_test.rs` 中的测试。
- `builder::build` 遇到 `FunctionName::StddevSamp` 时调用 `build_variance`，后者按 `has_distinct` 和聚合阶段选择 `AggImplementation::StddevSamp`、`StddevSampOriginalDistinct` 或 `StddevSampPartialDistinct`；`AggMode::Dedup` 被拒绝。

索引没有给出这些 `AggImplementation` 变体到本文件函数的生产调用边；扩展或排障时应把这视为待接线/待验证边界，而不是假定聚合 executor 已调用这里。

## 错误处理与边界

两个函数都不返回 `Result`，也不自行产生业务错误。空输入、全 NULL 输入和仅一个有效值统一通过 `Option::None` 表示 SQL NULL；至少两个有效值才返回 `Some`。NULL 的过滤发生在 `VarianceState::update` 或 `DistinctVariance::update`，不是在本文件中。

数学边界沿用 `f64` 语义：本文件不检查溢出、无穷大、NaN 或负零，也不对浮点误差做钳制。如果上游 `sample_variance()` 给出 NaN 或无穷大，`sqrt` 的 IEEE-754 结果会直接传播。DISTINCT 状态在 NaN 键负载耗尽时包含断言，但该断言位于 `DistinctVariance::update`，不在最终求值函数内。

现有测试明确覆盖：零个或一个有效值返回 `None`；NULL 不参与计数；样本 `[1, 3]` 使用 `N-1` 分母并得到 `sqrt(2)`；合并两个 partial result 后仍得到与 Go fixture 一致的结果；DISTINCT 合并后跨分区重复值只计一次。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络资源。两个求值函数都接收共享引用，普通路径只读固定状态；DISTINCT 路径会在函数内部创建一个临时 `VarianceState` 并在返回时释放，但不会修改原 `DistinctVariance`。

状态的创建、更新、合并和销毁由调用方拥有。并行聚合应让每个 worker 更新自己的可变 partial result，再通过 `merge(&mut destination, &source)` 汇总；不应在没有外部同步的情况下并发修改同一状态。类型本身没有内部锁，也没有跨调用缓存。DISTINCT 最终求值为与唯一值数量线性相关的临时重算；若未来把它放入高频或滑动窗口路径，需要评估该成本与额外内存。

## 与 Go 版本的对应关系

Go 对照文件 [`func_stddevsamp.go`](./func_stddevsamp.go) 定义三个执行器类型：普通 `stddevSamp4Float64`、original DISTINCT 和 partial DISTINCT。三者在 `AppendFinalResult2Chunk` 中读取部分结果，`count <= 1` 时向目标列追加 NULL，否则计算 `sqrt(variance/(count-1))` 并直接追加 Float64。

Rust 将共同计算折叠为两个自由函数：普通路径的 `VarianceState::sample_variance` 对应 Go 的计数判断与除法，`stddev_sample` 对应最后的 `math.Sqrt`；两个 Go DISTINCT 执行器共享的最终语义在 Rust 中由一个 `stddev_sample_distinct` 表达。Rust 别名对应 Go 的嵌入状态，但不是带 ordinal 和 chunk 输出行为的执行器结构体。

因此数学语义与 Go 对齐，但接口层级不同：Go 文件已经实现 `AppendFinalResult2Chunk` 并写 chunk；本 Rust 文件只返回 `Option<f64>`。Go 测试 `TestMergePartialResult4Stddevsamp` 和 `TestStddevsamp` 的输入/期望值已在独立 Rust 测试中复现；Rust 还额外明确验证了单值、NULL 和 DISTINCT 跨部分结果去重。

## 扩展指南

若只调整最终数学定义或边界，应修改 `stddev_sample` / `stddev_sample_distinct`，并同步独立文件 [`func_stddevsamp_test.rs`](./func_stddevsamp_test.rs)；不要把测试嵌入生产 `.rs` 文件。任何变更都应同时比较 Go 的 `func_stddevsamp.go` 与 `func_stddevsamp_test.go`，避免偏离 `N-1` 分母、NULL 或 DISTINCT 语义。

若调整累计、合并、浮点键或 NaN/零值语义，真正的修改点在 `func_varpop.rs`，并会同时影响 VAR_POP、VAR_SAMP、STDDEV_POP 和 STDDEV_SAMP；需要扩大回归到相邻独立测试，而不能只改本文件。若要完成生产执行接线，应从 `builder.rs` 的三个 `AggImplementation::StddevSamp*` 变体追到具体 `AggFunc` 实例化和 `append_final_result_to_chunk`，补齐类型擦除、状态分配/更新/合并、chunk NULL/Float64 输出以及 spill/内存记账，并用执行器级测试证明，而不是仅依赖计算函数单测。

兼容性风险主要有三类：改变少于两个有效值时的 NULL 行为会破坏 SQL 语义；改变 DISTINCT 浮点键规则会影响 `±0` 与 NaN；改变累计或开方次序会带来浮点结果差异。性能风险集中在 DISTINCT 最终重建状态的 O(U) 时间与 O(U) 集合存储，其中 U 为唯一值数。

## 验证依据

- 目标源码：`pkg/executor/aggfuncs/func_stddevsamp.rs`，确认两个公开函数、两个公开别名及无条件编译分支。
- 状态实现：`pkg/executor/aggfuncs/func_varpop.rs`，核对 `VarianceState::{update, merge, sample_variance}`、`DistinctVariance::{update, merge, state, sample_variance}` 的实际逻辑。
- crate 边界：`pkg/executor/aggfuncs/Cargo.toml` 与 `pkg/executor/aggfuncs/lib.rs`，核对包名、crate 根、公开模块和独立测试装配。
- 构建接线：`pkg/executor/aggfuncs/builder.rs`，核对 `FunctionName::StddevSamp`、三个 `AggImplementation` 变体和 `build_variance` 的模式选择。
- RustCodeGraph：检查索引状态后查询目标文件、`stddev_sample`、`stddev_sample_distinct` 及 callers/callees；直接调用边仅落在上述 Rust 测试，未发现目标函数到生产 `AggFunc` 的接线。
- Rust 测试：`pkg/executor/aggfuncs/func_stddevsamp_test.rs`，以及 DISTINCT 并行场景的 `func_distinct_agg_test.rs`、`go_scenario_coverage_test.rs`。
- Go 对照：`pkg/executor/aggfuncs/func_stddevsamp.go` 与 `pkg/executor/aggfuncs/func_stddevsamp_test.go`，核对最终 chunk 输出、`N-1` 分母和合并 fixture。
- 本任务仅做文档分析，按计划不运行 Cargo；交付验证只执行固定十一章节的结构检查，并人工复核以上事实链。

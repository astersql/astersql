# `pkg/executor/aggfuncs/func_varpop.rs`

源文件：[func_varpop.rs](./func_varpop.rs)

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；crate 根在 `pkg/executor/aggfuncs/lib.rs` 中以 `pub mod func_varpop` 暴露它，`pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认了这一边界。它不负责解析 SQL 表达式或直接读写结果 `Chunk`，而是提供 `VAR_POP`、`VAR_SAMP`、`STDDEV_POP`、`STDDEV_SAMP` 共用的浮点方差部分状态和合并算法。

该状态已经接入 Rust 聚合框架：`pkg/executor/aggfuncs/aggfuncs.rs` 的部分结果绑定逻辑把四种普通方差/标准差实现绑定为 `VarianceState`，把 original/partial 两阶段的 DISTINCT 变体绑定为 `DistinctVariance`；同文件的 merge 分派再调用两种状态各自的 `merge`。因此这里是聚合执行过程中“累计、跨分区合并、最终统计量计算”的数值核心，不是尚未接线的占位文件。

## 核心职责

1. `VarianceState` 保存非 DISTINCT 浮点输入的 `count`、`sum` 和可合并中间方差量 `variance`，跳过 SQL `NULL` 对应的 `None`。
2. `calculate_intermediate` 用在线公式逐值累计平方离差和，避免保存全部输入或为普通路径做第二次扫描。
3. `calculate_merge` 用两个分区的均值差补偿项合并部分结果，使 hash/stream 聚合的分区结果可组合。
4. `DistinctVariance` 用 `HashMap<u64, f64>` 模拟 Go `map[float64]` 的键语义，随后把去重后的值灌入普通状态以计算总体/样本方差。
5. 本文件同时暴露 `population_variance` 和 `sample_variance`，让 `func_varsamp.rs`、`func_stddevpop.rs`、`func_stddevsamp.rs` 复用同一份累计状态。

这里的 `variance` 不是最终 SQL 返回值，而是与 `Σ(x-μ)²` 等价的中间量；总体方差除以 `n`，样本方差除以 `n-1`。这一不变量同时体现在 `VarianceState::{population_variance,sample_variance}` 和 Go `func_varpop.go` / `func_varsamp.go` 的最终输出逻辑中。

## 主要符号

- `pub struct VarianceState { count, sum, variance }`：普通浮点方差的部分结果。三个字段是 `pub(crate)`，供 crate 内 merge、spill 和测试读取；对 crate 外只公开方法。
- `VarianceState::reset()`：以 `Default` 覆盖整个状态，恢复 `(0, 0.0, 0.0)`。
- `VarianceState::count()`：只读暴露有效输入数。
- `VarianceState::update(values)`：接受任意 `IntoIterator<Item = Option<f64>>`，过滤 `None`，更新计数、和与中间方差。
- `VarianceState::merge(source)`：把另一个分区合入当前状态；空源不变，空目标直接复制，双方非空时调用 `calculate_merge`。
- `VarianceState::population_variance()`：`count == 0` 时返回 `None`，否则返回 `variance / count`。
- `VarianceState::sample_variance()`：`count <= 1` 时返回 `None`，否则返回 `variance / (count-1)`。
- `calculate_intermediate(count, sum, input, variance)`：单点在线更新函数。调用前的 `count` 和 `sum` 已包含当前 `input`。
- `calculate_merge(...)`：合并两个非空状态的纯函数；调用者负责空状态分支和之后的 `count/sum` 累加。
- `pub struct DistinctVariance { values, next_nan_payload }`：DISTINCT 部分结果。`values` 保存人工构造的等价键及原始浮点值；`next_nan_payload` 为每次 NaN 插入分配不同键。
- `DistinctVariance::{reset,update,merge,state,population_variance,sample_variance}`：分别处理生命周期、去重、跨分区并集、转成普通方差状态及最终结果。
- `VarPop4Float64`、`VarPopOriginal4DistinctFloat64`、`VarPopPartial4DistinctFloat64`：与 Go 实现阶段命名对齐的公开类型别名，不引入额外状态或行为。

本文件没有 trait、模块级常量或条件编译项。

## 执行流程

普通路径的主流程如下：

1. `aggfuncs.rs` 为 `VarPop`、`VarSamp`、`StddevPop` 或 `StddevSamp` 绑定默认 `VarianceState`。
2. 求值层把一批可空 `f64` 交给 `VarianceState::update`。迭代器的 `flatten` 丢弃 `None`；每个有效值先递增 `count`、累加 `sum`。
3. 第一个有效值不计算分母含 `count-1` 的公式；从第二个值起，`calculate_intermediate` 令 `delta = count * input - sum`，累计 `delta² / (count * (count-1))`。
4. 并行或分区聚合需要汇总时，`aggfuncs.rs` 的 merge 分派调用 `VarianceState::merge`。双方非空时，`calculate_merge` 根据两边均值差增加跨分区补偿项，然后目标状态累加源的 `count` 与 `sum`。
5. `VAR_POP` 调 `population_variance`；`VAR_SAMP` 调 `sample_variance`；两个标准差模块在相应方差结果上再调用 `sqrt`。

DISTINCT 路径先由 `DistinctVariance::update` 建键：普通非零值使用 `to_bits()`，正负零统一为键 `0`，每次 NaN 使用递增的人造 payload。`merge` 重新把源集合的值走一遍 `update`，从而在目标侧重建相同语义的并集。最终 `state()` 迭代去重值并调用普通 `VarianceState::update`，再由总体或样本方法收尾。

## 数据与状态

`VarianceState` 的核心不变量是：`count` 只统计非空输入，`sum` 是这些输入的和，`variance` 是可按在线/合并公式继续组合的平方离差中间量。默认值和 `reset` 都表示空集合；复制状态是值复制，因为该类型为 `Copy` 且不拥有堆资源。

`DistinctVariance` 拥有一个堆分配的 `HashMap`。键和值分离是为了同时保留去重语义和实际参与计算的 `f64`：

- `0.0 == -0.0`，所以二者强制使用同一键，符合 Go 浮点 map 的相等语义。
- NaN 与自身不相等；Go map 中重复插入 NaN 不会命中既有键，因此 Rust 为每次 NaN 插入分配不同键。
- 其他值按 IEEE-754 位模式建键；相等的普通值会覆盖同一项。

`HashMap` 的迭代顺序不稳定，因此 DISTINCT 的求和与在线累计顺序不承诺固定；数学结果相同，但极端浮点数据可能出现舍入末位差异。`pkg/executor/aggfuncs/spill_serialize_helper.rs` 为两种状态实现 `SpillState`：普通状态按 `count/sum/variance` 顺序序列化；DISTINCT 状态序列化所有值，读回时重置集合和 NaN 计数器并重新执行 `update`。

## 依赖与调用关系

本文件唯一直接的标准库依赖是 `std::collections::HashMap`，没有直接使用 `Cargo.toml` 中的其他 workspace crate。它通过 crate 内部调用参与更大的执行链：

- 上游构造与合并：`pkg/executor/aggfuncs/aggfuncs.rs` 为对应 `AggImplementation` 绑定 `VarianceState`/`DistinctVariance`，并在部分结果合并分派中调用 `merge`。
- 模块装配：`pkg/executor/aggfuncs/lib.rs` 公开 `func_varpop`，并把独立测试模块纳入 `#[cfg(test)]`。
- 复用者：`func_varsamp.rs` 把两种状态重新导出为样本方差类型；`func_stddevpop.rs` 与 `func_stddevsamp.rs` 调总体/样本方差后开方。
- 持久化：`spill_serialize_helper.rs` 读取 crate 可见字段，完成 spill 往返。
- 内部调用边：`VarianceState::update -> calculate_intermediate`，`VarianceState::merge -> calculate_merge`，`DistinctVariance::{population_variance,sample_variance} -> state -> VarianceState::update`。

RustCodeGraph 的目标文件查询确认了 `VarianceState`、`DistinctVariance`、`calculate_intermediate`、`calculate_merge` 和 `state`；图结果明确列出两个计算函数分别由 `update`、`merge` 调用，`state` 由 DISTINCT 两个最终计算方法调用。精确 `callers` 子命令在本次环境中超过 60 秒没有返回，因此跨文件调用关系又以相邻 Rust 源码逐项核验。

## 错误处理与边界

这套 API 不返回 `Result`；普通数值运算遵循 Rust `f64` 的 IEEE-754 传播规则，NaN 和无穷值不会被转成业务错误。空普通/DISTINCT 集合的总体方差返回 `None`；样本方差还要求至少两个有效（或去重后的）值。`None` 输入被静默跳过，对应 SQL NULL 不参与聚合。

两个公式函数本身不验证前置条件：`calculate_intermediate` 要求 `count > 1`，`calculate_merge` 要求两侧计数均非零。安全入口分别由 `update` 和 `merge` 先做条件判断；绕过入口直接用非法计数调用公开函数可能发生整数除零 panic 或产生无效浮点结果。

DISTINCT 的 NaN 人造键最多使用 52 位 payload；耗尽时 `assert!` 会 panic，理论阈值是同一状态插入超过 `0x000f_ffff_ffff_ffff` 个 NaN。`count` 和合并后的整数乘法没有显式溢出保护；这与正常执行可达到的数据规模相距很大，但调用者不能把任意不可信的伪造状态当作已验证输入。普通状态与 Go 路径一样不显式过滤 NaN/Infinity。

## 并发与资源生命周期

两种状态都要求调用方持有 `&mut self` 才能更新、合并或重置；文件内没有锁、原子变量、线程、异步任务、通道或事务。并行聚合的并发隔离由上层为各分区分配独立 partial result 实现，汇总阶段再把源状态串行合入目标状态。本文件不提供同一实例的共享可变访问协议。

`VarianceState` 没有堆资源，`reset` 是常数时间。`DistinctVariance` 的内存随不同键数量增长；`reset` 的 `clear` 会移除元素但通常保留 `HashMap` 容量，随后把 NaN 计数器重置为 1。`aggfuncs.rs` 在 DISTINCT merge 前后比较容量并估算内存增量；spill 反序列化也按集合容量返回内存占用。源码没有收缩容量或显式内存上限，资源控制应由聚合框架的内存跟踪与 spill 机制承担。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/aggfuncs/func_varpop.go`，对应测试为 `func_varpop_test.go`。语义对应如下：

- Rust `VarianceState` 对应 Go `partialResult4VarPopFloat64`，字段和空状态一致。
- Rust `calculate_intermediate` 与 Go `calculateIntermediate` 使用代数等价的在线公式；Rust `calculate_merge` 与 Go `calculateMerge` 使用代数等价的两组均值差补偿公式。
- Rust `update`、`merge`、`population_variance` 分别承载 Go `UpdatePartialResult`、`MergePartialResult`、`AppendFinalResult2Chunk` 中的数值部分。
- Rust `DistinctVariance` 对应 Go `partialResult4VarPopDistinctFloat64.valSet`；Rust 最终通过普通在线状态计算，Go `calculateDistinctFloat64Variance` 则先求均值再二次遍历集合，算法路径不同但目标统计量相同。
- Go 类型实现完整 `AggFunc` 生命周期，包括表达式 `EvalReal`、错误包装、`Chunk` 输出、精确内存常量和序列化入口；Rust 把通用绑定/merge 放在 `aggfuncs.rs`，spill 放在 `spill_serialize_helper.rs`，本文件本身只提供类型与数值方法。不能把 Go 文件中的完整接口误认为均定义在此 Rust 文件内。
- Go original/partial DISTINCT 使用不同 wrapper 类型；Rust 的两个阶段名只是同一 `DistinctVariance` 的类型别名，阶段差异由上层 `AggImplementation` 枚举保留。

Go `TestVarpop` 验证空输入及 `0..4` 的总体方差 2，`TestMergePartialResult4Varpop` 验证部分结果合并，`TestMemVarpop` 还验证 Go 的内存增量。Rust `func_varpop_test.rs` 覆盖空值、NULL 跳过、reset、分区合并以及正负零 DISTINCT；内存/序列化对应证据在 `spill_helper_test.rs`，样本与标准差复用证据在各自独立测试文件中。

## 扩展指南

修改普通方差算法时，应同时维护 `VarianceState::update`、`merge`、两个最终计算方法以及 `calculate_intermediate`/`calculate_merge` 的前置条件，并同步独立测试 `pkg/executor/aggfuncs/func_varpop_test.rs`。若改变字段布局或含义，还必须同步 `spill_serialize_helper.rs` 的字段顺序和 `spill_helper_test.rs` 的往返断言；否则落盘状态会与内存状态不兼容。

修改 DISTINCT 等价关系时，应集中在 `DistinctVariance::update`，并检查 `merge` 与 spill 读回仍通过同一路径保持一致。至少补充重复普通值、正负零、多个 NaN、空集合、跨分区重复值及极端浮点值测试。若要保证确定性舍入，需显式规定迭代顺序或采用顺序无关/补偿求和算法，并评估与 Go map 无序遍历语义的兼容性及排序成本。

增加新的方差派生函数时，优先复用 `population_variance` 或 `sample_variance`，像现有标准差模块一样只实现收尾转换；同时在 `aggfuncs.rs` 的状态绑定、merge 分派和 spill 类型覆盖中确认新枚举路径。Rust 单元测试必须继续放在同目录独立 `*_test.rs` 文件，不能内嵌回生产源码。

兼容性风险主要是 SQL NULL/少行返回规则、DISTINCT 的 `±0`/NaN 语义和 spill 布局；正确性风险主要是在线公式调用时序及合并方向；性能风险主要是 DISTINCT 集合的 O(k) 内存、最终 O(k) 扫描及潜在的确定性排序开销。

## 验证依据

- 源码与装配：`pkg/executor/aggfuncs/func_varpop.rs`、`lib.rs`、`aggfuncs.rs`、`Cargo.toml`。
- 直接复用与持久化：`func_varsamp.rs`、`func_stddevpop.rs`、`func_stddevsamp.rs`、`spill_serialize_helper.rs`。
- Rust 独立测试：`func_varpop_test.rs`、`func_varsamp_test.rs`、`func_stddevpop_test.rs`、`func_stddevsamp_test.rs`、`func_distinct_agg_test.rs`、`spill_helper_test.rs`、`go_scenario_coverage_test.rs`。
- Go 对照：`pkg/executor/aggfuncs/func_varpop.go`、`func_varpop_test.go`，并通过同目录 `func_varsamp.go`、`func_stddevpop.go`、`func_stddevsamp.go` 核对复用关系。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`explore`/`query`/`node VarianceState` 定位目标符号，并返回 `update -> calculate_intermediate`、`merge -> calculate_merge`、最终方法 `-> state` 的调用证据。精确 `callers func_varpop.rs::calculate_intermediate` 超过 60 秒无输出后中止，跨文件关系改由上述相邻源码核验。
- 结构验收要求：文档必须存在，且恰好包含任务规定的十一个二级标题；本任务是纯文档分析，按计划不运行 Cargo。

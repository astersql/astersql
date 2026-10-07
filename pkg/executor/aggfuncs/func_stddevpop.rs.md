# `pkg/executor/aggfuncs/func_stddevpop.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate（见同目录 `Cargo.toml`，库入口为 `lib.rs`），由 `lib.rs` 的 `pub mod func_stddevpop;` 对外公开。它是 Rust 侧 `STDDEV_POP`（总体标准差）的轻量收尾模块：不自行读取 SQL 行、维护聚合器或实现 `AggFunc`，而是在 `func_varpop.rs` 已完成累计/合并的方差状态上计算最终数学值。

在较完整的聚合链中，`builder.rs::build` 把 `FunctionName::StddevPop` 交给 `build_variance`，后者按普通或 DISTINCT、原始或部分聚合阶段选择 `AggImplementation::StddevPop`、`StddevPopOriginalDistinct` 或 `StddevPopPartialDistinct`。`aggfuncs.rs::BuiltAggFunc::spill_function` 随后为这些实现绑定 `VarianceState` 或 `DistinctVariance`。不过，截至本次核验，生产 Rust 代码没有直接引用本文件的两个求值函数；直接调用者均为测试。因此，本文件已提供最终数学收尾能力，但从类型擦除状态到 `AggFunc::append_final_result_to_chunk` 的完整生产输出接线在当前证据中尚未实现或至少尚未接到这里。

## 核心职责

- `stddev_population`：调用 `VarianceState::population_variance` 取得 `Σ(x-μ)² / N`，再以 `f64::sqrt` 求总体标准差；空状态沿 `Option` 保持为 `None`。
- `stddev_population_distinct`：对 `DistinctVariance` 的去重值集合执行相同的“总体方差后开方”收尾。去重、NULL 跳过和分区合并均由 `DistinctVariance` 实现，本函数不重复这些逻辑。
- `StddevPop4Float64` 与 `StddevPopDistinctFloat64`：给复用的方差状态赋予 STDDEV_POP 语义名称，表明普通和 DISTINCT 路径的部分结果布局分别与 `VarianceState`、`DistinctVariance` 完全相同。

文件刻意不承担表达式求值、状态更新、merge、spill 编解码、输出 chunk 写入或错误封装；这些职责位于 `func_varpop.rs`、`builder.rs`、`aggfuncs.rs` 及聚合执行器中。

## 主要符号

- `pub fn stddev_population(state: &VarianceState) -> Option<f64>`：普通总体标准差收尾入口。它只借用状态，不改变 `count`、`sum` 或中间 `variance`。
- `pub fn stddev_population_distinct(state: &DistinctVariance) -> Option<f64>`：DISTINCT 总体标准差收尾入口。内部会经 `DistinctVariance::population_variance` 临时构造普通 `VarianceState`，计算去重集合的总体方差。
- `pub type StddevPop4Float64 = VarianceState`：普通 Float64 路径的语义别名，不是新类型，因而没有额外校验、方法或内存布局。
- `pub type StddevPopDistinctFloat64 = DistinctVariance`：DISTINCT Float64 路径的语义别名，同样不形成独立类型边界。

本文件没有模块级常量、结构体、trait、`impl`、条件编译项或私有辅助函数；两个函数和两个类型别名均为公开符号。

## 执行流程

普通路径的状态阶段由 `VarianceState::update` 跳过 `None`，累计 `count`、`sum` 和可合并的中间方差；并行/分区结果由 `VarianceState::merge` 使用两组均值差修正后合并。调用 `stddev_population` 时，流程只有两步：先由 `population_variance` 在 `count == 0` 时返回 `None`，否则返回 `variance / count`；再用 `Option::map(f64::sqrt)` 只对存在的结果开平方。

DISTINCT 路径由 `DistinctVariance::update` 先按浮点键语义去重并跳过 NULL，`merge` 把另一分区的值重新插入以跨分区去重。`stddev_population_distinct` 调用 `DistinctVariance::population_variance`；后者通过 `state()` 把去重后的值灌入新的 `VarianceState`，再执行普通总体方差计算，最后由本文件开平方。

构建与 spill 侧的已验证路径是：`builder.rs::build` → `build_variance` → `AggImplementation::StddevPop*` → `aggfuncs.rs::BuiltAggFunc::spill_function`，普通变体绑定 `VarianceState`，DISTINCT 变体绑定 `DistinctVariance`。恢复后的通用 merge 路径也在 `aggfuncs.rs::merge_spilled_partial_result` 中分别调用这两个状态的 `merge`。但该链只证明状态选择、spill 和 merge；本次搜索没有找到生产代码调用 `stddev_population*` 写出最终 chunk，因此不能把最终输出链描述为已接通。

## 数据与状态

`VarianceState` 的三个字段由 `func_varpop.rs` 定义：`count: i64` 是非 NULL 输入数，`sum: f64` 是输入和，`variance: f64` 是可合并的中间离差平方和。总体方差在收尾时除以 `count`，与样本标准差使用 `N-1` 的语义不同。状态更新使用在线公式，merge 使用两侧均值差补偿，无需保存所有普通输入。

`DistinctVariance` 保存 `HashMap<u64, f64>` 和 `next_nan_payload`。它将 `+0.0` 与 `-0.0` 归为同一键，而每次 NaN 插入生成不同键，以对齐 Go map 的相关行为；调用 `state()` 时才遍历去重后的值重新计算方差。因此 DISTINCT 的内存随唯一键数量增长，最终计算也有一次与唯一值数成正比的遍历。

本文件自身没有可变全局状态或缓存。两个函数都只接收共享引用并返回新的 `Option<f64>`；类型别名不会复制状态或改变所有权。

## 依赖与调用关系

直接源码依赖只有 `crate::func_varpop::{DistinctVariance, VarianceState}` 和标准库的 `f64::sqrt`。`Cargo.toml` 没有为本文件声明专属外部依赖或 feature；模块随 `astersql-executor-aggfuncs` 的普通库入口编译。

上游直接调用经 RustCodeGraph 与全仓文本搜索核验为测试：`func_stddevpop_test.rs` 同时调用普通和 DISTINCT 函数；`func_distinct_agg_test.rs`、`go_scenario_coverage_test.rs` 调用 DISTINCT 函数。生产侧的间接相关接线包括 `builder.rs::build_variance` 对实现枚举的选择，以及 `aggfuncs.rs` 对方差状态的 spill 与恢复后 merge；这些代码没有直接调用本文件函数。

下游调用为 `VarianceState::population_variance` 或 `DistinctVariance::population_variance`，之后调用 `f64::sqrt`。更深一层，DISTINCT 方法调用 `DistinctVariance::state`，后者通过 `VarianceState::update` 重算去重集合的方差。

## 错误处理与边界

两个函数均不返回 `Result`，也不主动产生业务错误。无有效非 NULL 输入时，底层 `population_variance` 返回 `None`，`Option::map` 不执行开方，对应 SQL 聚合结果 NULL。单个有效值的总体方差为零，因而总体标准差为 `Some(0.0)`；这与样本标准差至少需要两个值不同。

NULL 的过滤不在本文件发生，而在 `VarianceState::update` 与 `DistinctVariance::update` 的迭代器 `flatten` 中完成。DISTINCT 重复值在状态层去重；跨 partial result 合并后仍去重。浮点 NaN、无穷大以及在线运算可能产生的 NaN/舍入误差没有在本文件被规范化或转成错误，`sqrt` 按 Rust `f64` 语义传播。理论上中间方差受浮点误差影响可能出现极小负数，此处也不会钳制为零。

类型别名无法防止调用者把 VAR_POP 状态与 STDDEV_POP 状态混用，因为它们在 Rust 类型系统中是同一类型。安全性依赖 builder/执行器保持实现枚举与状态语义一致。

## 并发与资源生命周期

函数仅通过共享引用读取状态，没有锁、原子量、线程、异步任务、通道、事务或 I/O；它们本身可在调用者保证状态不被并发修改时并行读取。普通收尾不分配堆内存，只进行除法和开方。

部分结果的所有权和生命周期由聚合框架管理：`aggfuncs.rs` 以 `Box<dyn Any + Send>` 保存类型擦除状态，spill 绑定通过 `StateSerializer<T>` 复制、序列化和恢复状态。普通状态是固定大小；DISTINCT 状态拥有 `HashMap`，merge 可能扩容并由通用 merge 逻辑计算容量增量。`stddev_population_distinct` 为求结果临时创建一个 `VarianceState`，但不会消费或清空原去重集合。

本文件不保证对同一个可变状态的同步访问，也不持有任何需显式释放的资源；状态离开其拥有者作用域时由 Rust 自动析构。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_stddevpop.go`。Go 的 `stdDevPop4Float64.AppendFinalResult2Chunk` 将 `partialResult4VarPopFloat64` 的中间方差除以 count 后开方；count 为零时向 chunk 追加 NULL。Rust 的 `stddev_population` 对应这段数学和空值语义，但返回 `Option<f64>`，不直接写 chunk，也没有 `AggFuncUpdateContext` 或 `error` 返回。

Go 为 DISTINCT 的 original/partial 阶段定义两个包装类型，并各自实现相同的 `AppendFinalResult2Chunk`：调用 `calculateDistinctFloat64Variance`，空集合写 NULL，否则计算 `sqrt(variance/count)`。Rust 将两阶段统一为 `DistinctVariance` 状态和 `stddev_population_distinct` 函数；`builder.rs::build_variance` 仍保留 `StddevPopOriginalDistinct` 与 `StddevPopPartialDistinct` 的阶段枚举，但 spill 时两者绑定同一具体状态。

Go 测试 `func_stddevpop_test.go::TestMergePartialResult4Stddevpop` 的 0..5、2..5 及合并结果，被 Rust 测试 `merge_partial_result_matches_go_fixture` 逐项复现；`TestStddevpop` 的 NULL/空输入意图由 `stddev_population_skips_null_and_returns_null_for_empty_input` 覆盖。Rust 另测了 DISTINCT 跨分区去重以及“标准差等于总体方差平方根”。主要迁移差异是 Rust 文件目前只实现状态到数值的收尾函数，尚无直接证据表明它们已接到生产 `AggFunc::append_final_result_to_chunk`；Go 文件则是完整的最终输出实现。

## 扩展指南

若修改 STDDEV_POP 数学或浮点边界策略，应优先修改 `stddev_population` 与 `stddev_population_distinct`，并同步独立文件 `func_stddevpop_test.rs`；若策略也影响方差状态、NULL、NaN、零值或 merge，则应改 `func_varpop.rs` 并同步 `func_varpop_test.rs`。不要在本文件复制状态更新公式，否则普通、DISTINCT、VAR_POP 与 STDDEV_POP 容易漂移。

若要完成生产最终输出接线，应从 `builder.rs` 的 `AggImplementation::StddevPop*` 和聚合 `AggFunc` 构造/实现处接入本文件函数，确保普通、original DISTINCT、partial DISTINCT 均在最终阶段写出 `Option` 对应的值或 NULL；同时增加独立测试验证 builder 到 `append_final_result_to_chunk` 的端到端路径，而不把测试嵌入生产源文件。还需复查 spill 恢复后的最终收尾、输出列 ordinal、返回类型及内存记账。

兼容性上须保持总体分母为 `N`、空输入为 NULL、DISTINCT 跨分区去重以及与 Go 浮点行为一致。性能上，普通路径应继续复用 O(1) 状态；DISTINCT 路径当前最终计算为 O(U)（U 为唯一值数），若优化缓存必须保证 merge/update 后缓存失效并维持 Go 的 ±0 与 NaN 键语义。类型别名若改为新类型会影响 builder、spill、类型擦除 downcast 和内存布局，不能只在本文件局部修改。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 23 行模块。
- RustCodeGraph `node --file`：阅读了 `func_stddevpop.rs`、`func_varpop.rs`、`func_stddevpop_test.rs`、`builder.rs`、`aggfuncs.rs`、`lib.rs`，确认符号定义、状态算法、构建选择、spill/merge 接线及测试模块声明。
- RustCodeGraph `query StddevPop`、`query stddev_population` 与 `explore`：确认两个公开函数、Go 对照类型/方法、builder 枚举和测试调用者。精确 `callers` 查询在本环境中长时间无输出，因此又以全仓 `rg` 交叉核验直接引用集合。
- Cargo 与模块边界：`pkg/executor/aggfuncs/Cargo.toml` 的 package 名为 `astersql-executor-aggfuncs`、库入口为 `lib.rs`；`lib.rs` 公开 `func_stddevpop` 并在 `#[cfg(test)]` 下声明独立的 `func_stddevpop_test`。
- Go 对照：阅读 `func_stddevpop.go` 与 `func_stddevpop_test.go`，核对普通/DISTINCT 最终值、空输入 NULL、总体分母和 merge fixture。
- Rust 测试证据：`func_stddevpop_test.rs` 覆盖普通 merge、NULL/空输入、DISTINCT 跨 partial 去重和方差开方关系；`func_distinct_agg_test.rs` 与 `go_scenario_coverage_test.rs` 额外覆盖 DISTINCT 方差/标准差场景。本任务依计划不运行 Cargo，测试内容仅作源码事实证据。
- 全仓引用复核：`rg` 显示 `stddev_population*` 的 Rust 直接引用只存在于上述测试文件，故生产最终输出接线被明确标为未验证/当前未见，而没有据 Go 实现推断 Rust 已完整接线。

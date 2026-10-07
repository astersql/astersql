# `pkg/executor/aggfuncs/func_count.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate。该 crate 以 `pkg/executor/aggfuncs/lib.rs` 为入口，`lib.rs` 通过 `pub mod func_count` 公开本模块；`pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/executor/aggfuncs"` 分别确认了 crate 边界与 Go 移植来源。

它提供普通 `COUNT` 的轻量状态机，而不是 `COUNT(DISTINCT ...)`；后者位于 `func_count_distinct.rs`。当前文件只依赖公共部分结果类型 `aggfuncs::PartialResult4Count` 和错误类型 `aggfuncs::AggError`，没有表达式求值、chunk 写出或 spill 序列化代码。

需要区分实现与接线现状：`builder.rs::build_count` 会把非 DISTINCT 的 `Complete`/`Partial1` 模式选为 `AggImplementation::CountOriginal(ValueKind)`，把 `Partial2`/`Final` 选为 `AggImplementation::CountPartial`；但本文件的 `CountAggregator` 没有实现 `AggFunc` 或 `SlidingWindowAggFunc`。仓库内对 `CountAggregator` 的直接引用目前均在独立 Rust 测试中，因而仅凭本文件不能断言这些 builder 枚举已实例化为生产执行器对象。

## 核心职责

- 用一个 `i64` 保存 COUNT 部分结果，空状态和 reset 后状态均为 `0`（`CountAggregator::default`、`reset`）。
- 对已经求值的 `Option<T>` 序列统计非 `NULL` 项：`Some(_)` 加一，`None` 跳过（`update`）。泛型 `T` 的内容不会被读取，COUNT 只关心是否为空。
- 接收上游产生的可空 `i64` 部分计数并求和，供两阶段聚合语义使用（`update_partial`）。
- 把另一个同型累加器合并进当前累加器（`merge`）。
- 在窗口边界移动时先撤销移出窗口的非空行，再加入新进入窗口的非空行（`slide`）。
- 公开与 Go 各求值类型命名对齐的类型别名，使调用侧可保留阶段/类型语义而复用同一状态机。

## 主要符号

- `DEF_PARTIAL_RESULT_4_COUNT_SIZE: i64`：`size_of::<PartialResult4Count>()`，即一个 `i64` 部分结果的固定尺寸。其用途是与 Go 的 `DefPartialResult4CountSize` 内存记账语义对齐；本文件自身不执行内存记账。
- `CountAggregator { count: PartialResult4Count }`：唯一有状态的结构体。字段私有，外部只能经方法读取或改变；派生 `Clone`、`Copy`、`Default`、`Eq` 等 trait。
- `CountOriginal4Int`、`CountOriginal4Real`、`CountOriginal4Decimal`、`CountOriginal4Time`、`CountOriginal4Duration`、`CountOriginal4Json`、`CountOriginal4VectorFloat32`、`CountOriginal4String`：原始输入阶段的公开类型别名，全部指向 `CountAggregator`。
- `CountPartial`：部分聚合输入阶段的公开类型别名，同样指向 `CountAggregator`。
- `value(&self) -> i64`：只读返回当前计数。
- `reset(&mut self)`：把状态置零。
- `update<T>(..., Option<T>) -> Result<(), AggError>`：统计非空原始值。
- `update_partial(..., Option<i64>) -> Result<(), AggError>`：累加非空 partial count。
- `merge(&mut self, source: &Self) -> Result<(), AggError>`：通过 `update_partial([Some(source.count)])` 合并一个来源状态。
- `slide<T, U>(outgoing, incoming) -> Result<(), AggError>`：撤销离开窗口的非空值，再复用 `update` 加入进入窗口的值。`T` 与 `U` 可以不同，因为方法只观察 `Option` 的空值标志。

本文件没有模块级可变静态量、trait 定义、条件编译项、异步函数或 `unsafe` 代码。

## 执行流程

原始输入路径以 `CountAggregator::default()` 创建计数为零的状态。调用者先完成表达式求值并把每个结果表示为 `Option<T>`，再调用 `update`；方法顺序扫描输入，只对 `Some` 执行 `wrapping_add(1)`，最终由 `value` 读取计数。类型差异在进入本状态机前已经被消解，这也是多个 `CountOriginal4*` 别名共享实现的原因。

两阶段路径中，上一阶段的计数以 `Option<i64>` 进入 `update_partial`。该方法借助 `flatten()` 丢弃 `None`，对其余值逐个执行回绕加法。若上游已经持有另一个 `CountAggregator`，`merge` 读取其私有 `count` 并把它作为单个非空 partial 输入复用上述路径；来源状态不会被修改。

滑动窗口路径由 `slide` 完成：第一轮扫描 `outgoing`，每遇到一个 `Some` 就 `wrapping_sub(1)`；第二步调用 `update(incoming)`，为进入窗口的每个 `Some` 加一。因此正常调用必须保证当前状态确实包含所有要移出的非空行；该方法不保存行历史，也不验证窗口边界一致性。

## 数据与状态

全部运行时状态只有 `CountAggregator::count`，其别名 `PartialResult4Count` 在 `aggfuncs.rs` 中定义为 `i64`。没有堆集合、缓存、表达式、行引用或生命周期参数；`CountAggregator` 可按值复制，复制后两个计数器独立演化。

`Option<T>` 是本文件的 NULL 边界：`None` 永远不改变计数，`Some` 的载荷不参与比较、哈希或转换。`update_partial` 对输入值不做非负校验，因此可累加负数；`slide` 也可能把零减为 `-1`。这符合当前低层状态机“机械执行计数增减”的契约，上层负责提供一致的 partial 与窗口差分。

所有加减均使用 `wrapping_add`/`wrapping_sub`，溢出按二进制补码回绕。`func_count_test.rs::count_wraps_like_go_int64_arithmetic` 验证 `i64::MAX + 1 == i64::MIN`，也验证从零移出一个非空值会得到 `-1`。

## 依赖与调用关系

直接下游依赖仅有：

- `crate::aggfuncs::PartialResult4Count`：实际为 `i64`，定义部分结果的表示。
- `crate::aggfuncs::AggError`：所有变更方法的返回错误类型；当前实现不构造错误。
- `std::mem::size_of`：计算固定部分结果尺寸。

模块入口是 `lib.rs::func_count`。实现选择入口是 `builder.rs::build` → `build_count`：前者在 `FunctionName::Count` 分支进入后者，后者根据 `AggMode`、DISTINCT、参数数量和 `ValueKind` 选择枚举变体。`func_count.rs` 与 builder 之间当前没有直接构造调用边；类型别名也未被 builder 直接引用。

RustCodeGraph 把本文件列为被 8 个文件使用，但对 `CountAggregator::{update, update_partial, merge, slide}` 的精确 `callers`/`callees` 查询未返回静态边。使用普通文本交叉核验后，生产目录内的直接符号引用除定义自身外位于 `func_count_test.rs`、`aggfunc_test.rs`、`go_scenario_coverage_test.rs` 和 `main_test.rs`。因此当前可证实的上游是测试，生产执行接线仍未由直接代码引用证明。

## 错误处理与边界

`update`、`update_partial`、`merge` 和 `slide` 均返回 `Result<_, AggError>`，但当前分支没有任何 `Err` 路径；返回类型为将来接入真实求值错误或统一聚合接口保留兼容形状。调用者仍应传播 `Result`，不可据此假设未来永远无错。

空输入是正常情况：`update([])`、`update_partial([])` 和空的滑动差分保持原状态。NULL 也不是错误：原始值 `None` 不计数，partial 的 `None` 不参与合并。方法不检查负 partial、重复 partial、窗口下溢或算术溢出；回绕行为是显式设计而非 debug/release 构建差异。

与 SQL 最终输出有关的行为不在本文件内：这里没有把计数追加到 chunk 的方法，也没有决定空组最终显示什么。Go 的 `baseCount.AppendFinalResult2Chunk` 会追加 `int64`，但 Rust 等价生产写出路径不能从当前文件得到验证。

## 并发与资源生命周期

本文件没有锁、原子量、通道、任务、事务或 I/O。状态由调用者以 `&mut self` 独占更新，Rust 借用规则阻止同一实例被两个安全调用同时修改；若执行器并行聚合，应给每个分区独立的累加器，再通过 `merge` 汇总，而不是共享可变实例。

`CountAggregator` 不拥有堆资源，创建与销毁没有显式清理工作。`reset` 允许复用同一实例而不分配新状态；`merge` 只读取来源并修改目标。`DEF_PARTIAL_RESULT_4_COUNT_SIZE` 仅覆盖一个 `i64` 的固定状态，不包含外部容器或执行器持有该状态的开销。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/executor/aggfuncs/func_count.go`。两端都以 `int64` 保存 `partialResult4Count`，零值是空状态，并把 NULL 排除在 COUNT 之外。Rust 的 `DEF_PARTIAL_RESULT_4_COUNT_SIZE` 对应 Go 的 `DefPartialResult4CountSize`；`reset` 对应 `baseCount.ResetPartialResult`；`value` 表达 Go 最终读取 `*partialResult4Count` 的状态，但不包含 Go 的 chunk 写出。

Go 为 Int、Real、Decimal、Time、Duration、JSON、VectorFloat32 和 String 分别定义结构及 `UpdatePartialResult`/`Slide`，因为每种实现要调用不同的 `Eval*`。Rust 注释明确说明求值发生在进入累加器之前，所以各 `CountOriginal4*` 只是同一 `CountAggregator` 的别名。该抽象保留 NULL/计数/滑动算法，却没有复制 Go 的表达式错误传播：Go 的 `Eval*` 可能返回错误，Rust 当前收到的已经是 `Option<T>`。

Go `countPartial.UpdatePartialResult` 对非 NULL 的 `EvalInt` 结果求和，`MergePartialResult` 把来源计数加到目标；对应 Rust 的 `update_partial` 与 `merge`。Go 使用普通 `int64` 加减，Rust 用显式 wrapping 运算固定补码回绕语义，并由 `func_count_test.rs` 覆盖边界。

Rust 当前文件未移植 Go `baseCount` 的 `AllocPartialResult`、最终 chunk 写出、spill 序列化/反序列化，也未实现 Go 的聚合 trait 接口。扩展或接线时不能把这些职责误认为已经由 `CountAggregator` 提供。

## 扩展指南

若新增一种原始值类型且仍只需要 COUNT 非 NULL，优先增加语义明确的 `CountOriginal4Xxx` 类型别名，并在 `builder.rs::value_kind`、`build_count` 与对应 builder 独立测试中同步类型选择；无需复制 `update`。如果新类型在进入状态机前不能可靠表示为 `Option<T>`，应在求值/适配层解决，而不是让 COUNT 检查载荷内容。

若把本状态机接入真实聚合执行器，最可能修改的位置是 `CountAggregator` 的 trait impl 或专门适配器，同时需要补齐：部分结果分配与 reset、原始表达式求值错误、final chunk 写出、spill 编解码、固定内存增量以及滑动窗口 trait。接线应复用 `builder.rs` 既有 `CountOriginal`/`CountPartial` 枚举选择，不应另建平行工厂。

行为变更必须同步独立测试文件，不能把测试内嵌进 `func_count.rs`。核心目标为 `func_count_test.rs`；通用 update/merge/reset 契约在 `aggfunc_test.rs`，Go 场景映射在 `go_scenario_coverage_test.rs`，builder 模式选择在 `builder_test.rs`。应覆盖 NULL、空输入、partial 的 NULL 和负值、多个 partial 合并、reset 后复用、滑动窗口移入/移出不对称以及 `i64` 边界回绕。

兼容性风险主要是改变 NULL 跳过规则、阶段选择或回绕语义；正确性风险主要是窗口调用者传入与当前状态不一致的 outgoing 集合；性能风险主要来自把当前零分配线性扫描改成额外收集或动态分派。若增加错误分支，还要检查现有调用者的 `unwrap`/传播策略。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/aggfuncs/func_count.rs` 确认目标文件已索引且含 20 个符号。
- RustCodeGraph `node --file pkg/executor/aggfuncs/func_count.rs`：核对常量、结构体、九个类型别名及五个方法的完整实现。
- RustCodeGraph `query CountOriginal4Int64`/`query CountPartial4Int64`：前者按近似名定位到 Rust `CountOriginal4Int` 和 Go `countOriginal4Int`，后者无结果，促使改用精确源码符号；`query PartialResult4Count` 定位到 `aggfuncs.rs` 的 `i64` 别名。
- RustCodeGraph `callers`/`callees` 对 `CountAggregator` 及其 `update`、`update_partial`、`merge`、`slide` 未返回静态边；随后以索引的 used-by 信息和 `rg` 直接引用交叉核验，未把缺失图边解释成生产接线。
- 已读 Rust 路径：`func_count.rs`、`aggfuncs.rs`（`PartialResult4Count` 和公共 trait）、`builder.rs`（`build`/`build_count`）、`lib.rs`、`func_count_test.rs`、`aggfunc_test.rs`、`go_scenario_coverage_test.rs`、`main_test.rs`。
- 已读配置与 Go 对照：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/func_count.go`。目标包不存在 `doc.go`。
- 关键测试事实：`func_count_test.rs::count_handles_null_partial_merge_slide_and_multi_distinct` 覆盖 NULL、partial 和 slide；`count_wraps_like_go_int64_arithmetic` 覆盖加减回绕；`aggfunc_test.rs::generic_aggregate_partial_final_and_reset_contract_matches_go` 覆盖 update/merge/reset；`go_scenario_coverage_test.rs::test_merge_partial_result4_count` 覆盖 Go 命名场景。
- 本任务是纯文档分析，按计划未运行 Cargo。最终仅用任务规定的 shell 命令验证文档存在且固定二级标题恰好为 11 个。

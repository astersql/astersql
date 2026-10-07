# `pkg/executor/aggfuncs/func_sum_int.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate 的整数 `SUM` 状态实现。crate 入口 `pkg/executor/aggfuncs/lib.rs` 以 `pub mod func_sum_int` 注册本模块；`pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `package.metadata.porting.go-package = "pkg/executor/aggfuncs"` 表明它是 Go `pkg/executor/aggfuncs` 的 Rust 移植边界之一。

这里处理已经求值为 `Option<i64>` 或 `Option<u64>` 的输入流，不负责表达式求值、从 `chunk.Row` 取值或向结果 chunk 写列。普通与 `DISTINCT` 状态会被 `pkg/executor/aggfuncs/builder.rs` 的 `build_sum_int` 选择为 `AggImplementation::{SumInt, SumUint, SumDistinctInt, SumDistinctUint}`；但当前通用 spill 工厂 `BuiltAggFunc::spill_function` 只将两个 `DISTINCT` 变体绑定到本文件的具体类型。因而应把本文件理解为整数求和算法和部分状态的实现，而不是 Go 聚合接口的完整逐方法复刻。

## 核心职责

- `SumInt` 与 `SumUint` 保存普通整数和及非 `NULL` 行数，支持重置、批量更新、部分结果合并和滑动窗口更新。
- `SumDistinctInt64` 与 `SumDistinctUint64` 用 `HashSet` 去重，支持更新、合并、最终检查求和，并以集合 capacity 的变化报告近似内存增量。
- 四个 `DEF_PARTIAL_RESULT_*_SIZE` 常量用 `size_of` 暴露状态结构的固定栈内大小；集合桶的动态堆内存不包含在固定大小中。
- 所有整数加减都走 `checked_add`/`checked_sub`，把算术溢出转换为 `AggError`，而不是采用 Rust release 模式的回绕行为。
- `Sum*Original` 四个别名保持“原始输入阶段”命名兼容，但没有创建新状态或改变算法。

## 主要符号

- `DEF_PARTIAL_RESULT_4_SUM_INT64_SIZE`、`DEF_PARTIAL_RESULT_4_SUM_UINT64_SIZE`：分别是 `PartialResult4SumInt64` 和 `PartialResult4SumUint64` 的固定大小。两者实际是 `pkg/executor/aggfuncs/aggfuncs.rs` 中 `SumPartialResult<i64/u64>` 的类型别名，字段为 `value` 与 `not_null_row_count`。
- `DEF_PARTIAL_RESULT_4_SUM_DISTINCT_INT64_SIZE`、`DEF_PARTIAL_RESULT_4_SUM_DISTINCT_UINT64_SIZE`：两个集合状态结构自身的固定大小；不代表集合已分配桶的总内存。
- `SumInt`：有符号累加器。公开方法为 `reset`、`value`、`count`、`update`、`merge`、`slide`；私有 `add` 实现首个非空值直接赋值和后续检查加法。
- `SumUint`：无符号累加器，公开接口与 `SumInt` 对称；更新逻辑直接写在 `update` 中。
- `SumDistinctInt64`：持有 `pub(crate) values: HashSet<i64>`。`update` 去除 `None` 并插入集合，`value` 对唯一值进行检查加法。
- `SumDistinctUint64`：持有 `pub(crate) bit_patterns: HashSet<i64>`。它把 `u64` 以 `as i64` 保存为相同比特模式的键，求和时再转回 `u64`；例如 `u64::MAX` 的键是 `-1`。
- `SumIntOriginal`、`SumUintOriginal`、`SumDistinctInt64Original`、`SumDistinctUint64Original`：仅为上述类型的公开别名。

## 执行流程

普通有符号更新从 `SumInt::update` 开始：迭代器先通过 `flatten` 跳过 `None`，每个值交给 `add`。首个非空值直接建立 `value` 和计数；后续值以 `checked_add` 累加，成功后才递增计数。`value` 依据 `not_null_row_count != 0` 决定返回 `Some(sum)` 还是 `None`。

`SumUint::update` 执行相同流程，只是累加类型和错误文本为无符号版本。普通 `merge` 都先忽略空 source；若 destination 为空则克隆整个 source 状态；否则先检查两个部分和的加法，再合并非空行数。

`slide` 的顺序是本文件的重要不变量：先遍历 outgoing，用检查减法移出旧窗口非空值并递减计数；再调用 `update(incoming)` 加入新窗口值。这个顺序避免“旧值尚未移出时先加入新值”导致仅在中间状态溢出。Go 测试 `TestSlideSumUintProcessOutWindowFirstToAvoidOverflow` 和 `TestSlideSumIntProcessOutWindowFirstToAvoidOverflow` 分别用 `MAX - 1` 滑到 `2` 验证该约束。

`DISTINCT` 更新先构建唯一值集合，不在插入阶段做算术；`merge` 等价于把 source 集合的元素再次喂给 destination 的 `update`。最终 `value` 才用 `try_fold` 做检查求和，空集合返回 `Ok(None)`。无符号版本只在集合键表示上使用 `i64`，算术始终转换回 `u64` 后执行。

## 数据与状态

普通状态的 `value` 与 `not_null_row_count` 来自共享泛型 `SumPartialResult<T>`。计数的语义不是 DISTINCT 基数，而是已纳入普通聚合的非空输入行数；它主要用于区分 SQL 空输入的 `NULL` 与数值和为零。`reset` 用 `Default` 一次性恢复二者。

DISTINCT 状态没有单独计数：集合是否为空直接决定最终结果是否为 `NULL`。`reset` 用新的 `HashSet` 替换旧集合，因此释放旧 capacity；独立测试 `distinct_reset_releases_set_capacity_like_go` 验证重置后再次插入会重新产生正内存增量。

`update`/`merge` 返回的 DISTINCT 内存增量按 `(new_capacity - old_capacity) * size_of::<i64>()` 计算。这只跟踪键槽容量的变化，并不声称覆盖 `HashSet` 控制块、哈希表实现开销或 allocator 元数据。固定大小常量则只覆盖状态值本身，两类口径不可相加后当作精确进程内存。

## 依赖与调用关系

直接依赖只有标准库的 `HashSet`、`size_of`，以及 `crate::aggfuncs::{AggError, PartialResult4SumInt64, PartialResult4SumUint64}`。本文件本身不直接使用 `Cargo.toml` 中的表达式、chunk、类型系统或序列化 crate。

上游选择链为 `builder::build` 遇到 `FunctionName::SumInt` 后调用 `build_sum_int`，再依据 `return_type.unsigned` 与 `has_distinct` 选择四种 `AggImplementation`。该选择函数只排除 `AggMode::Dedup`，没有按 Complete/Partial 阶段生成不同的整数状态类型。

RustCodeGraph 的文件节点显示本文件被 `aggfuncs.rs`、`spill_serialize_helper.rs` 和 `spill_helper_test.rs` 使用。具体接线中，`BuiltAggFunc::spill_function` 为 `SumDistinctInt`/`SumDistinctUint` 创建 `StateSerializer`；`merge_spilled_partial_result` 下转并调用两个 DISTINCT 类型的 `merge`；`SpillState` 实现读取、写入它们的集合。普通 `SumInt`/`SumUint` 不在该 spill match 中，且本文件没有实现统一的行级聚合 trait，因此不能从现有证据推断它们已覆盖 Go 的完整执行链。

## 错误处理与边界

- 空输入或全 `NULL` 输入：普通状态计数保持零，DISTINCT 集合为空，最终都返回 `None`。
- 重复值：普通求和全部计入；DISTINCT 只保留一次。不同部分结果之间的重复也会在 `merge` 时消除。
- 有符号或无符号越界：普通 `update`、`merge`、`slide` 以及 DISTINCT 最终 `value` 都返回 `AggError`。状态更新顺序使当前算术失败时不会写入失败的和；但批量调用在较早元素已成功后、较晚元素失败时不会事务式回滚整个批次。
- `slide` 假定 outgoing 确实属于当前状态。若调用方传入不一致的窗口差量，检查减法可报错；计数减法本身没有显式下溢检查，因此调用方必须维持“移出非空数不超过当前计数”的不变量。
- DISTINCT 求和遍历 `HashSet`，迭代顺序不稳定，但整数检查加法在所有元素的数学和可表示时结果一致；若最终和不可表示则返回错误，不提供固定的首个溢出元素顺序。
- `SumDistinctUint64` 的 `u64 <-> i64` 转换仅用于保留 64 位比特模式和相等性，不能把集合键当有符号数排序或运算。

## 并发与资源生命周期

所有更新方法都需要 `&mut self`，类型内部没有锁、原子量、线程或异步任务。并行聚合应由上层为每个 worker 分配独立部分状态，最后通过 `merge` 汇总；本文件不负责共享状态同步。

普通状态只有内联标量，`reset` 不涉及外部资源。DISTINCT 状态拥有 `HashSet` 的堆分配；克隆会复制集合，`reset` 会丢弃旧集合并释放其容量，drop 时由 Rust 自动回收。spilling 只在模块外通过 `SpillState` 为两个 DISTINCT 类型实现：写出集合元素，读回前先 `reset`，再以 `update` 重建集合并报告容量增量。无文件句柄、网络连接、事务或通道由本文件持有。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_sum_int.go`。Rust 的 `SumInt`/`SumUint` 对应 Go 的 `sumInt`/`sumUint` 及其 `partialResult4Sum*`；两个 DISTINCT 结构对应 `sumDistinctInt64`/`sumDistinctUint64` 的集合状态。固定大小常量、跳过 `NULL`、首值赋值、检查加减、空结果写 `NULL`、DISTINCT 合并以及无符号值用 `int64` 比特模式去重等语义保持一致。

Rust 与 Go 的职责边界尚不完全相同。Go 类型直接实现 `AllocPartialResult`、`ResetPartialResult`、`UpdatePartialResult`、`MergePartialResult`、`AppendFinalResult2Chunk`、spill 序列化/反序列化和普通 SUM 的 `SlidingWindowAggFunc`，其中表达式求值错误会直接传播。Rust 本文件接收已经变成 `Option<i64/u64>` 的值，不读取行、不写 chunk；普通状态也未在此处接入 spill。Rust 的 DISTINCT spill 由 `spill_serialize_helper.rs` 和 `aggfuncs.rs` 的通用适配层承担。

Go 集合使用 `set.Int64SetWithMemoryUsage` 返回实现定义的内存变化；Rust 用 `HashSet` capacity 乘键大小近似。两者都在 reset 时创建新集合，但内存数值口径不保证逐字节相等。Rust 独立测试覆盖结构固定大小和 reset 容量行为，其他测试覆盖跨部分去重、无符号 `u64::MAX` 比特模式与 spill 往返。

## 扩展指南

若修改普通整数求和算法，应同时检查 `SumInt::{add, update, merge, slide}` 与 `SumUint::{update, merge, slide}`，保持“算术成功后才提交状态”和“滑窗先移出后加入”两个约束，并在独立的 `pkg/executor/aggfuncs/func_sum_int_test.rs` 中增加边界测试；不要把测试内嵌回生产文件。溢出、全 NULL、零和但非空、部分结果一端为空，以及批量中途失败后的状态都值得显式覆盖。

若修改 DISTINCT，需同步检查 `update`、`merge`、`value`、两个固定大小常量，以及 `spill_serialize_helper.rs` 的 `SpillState` 实现和 `aggfuncs.rs` 的恢复合并分派。改变无符号键表示会影响已有 spill 数据格式和 `u64::MAX -> -1` 契约，属于兼容性风险；改变内存增量算法会影响上层内存追踪，属于资源控制风险。

若要补齐普通状态的执行器接线，应以 Go 完整接口为行为依据，在现有通用聚合抽象中接入，而不是仅在本文件增加表面方法。需要同时核对 builder 选择、行值求值、最终结果类型、spill 编解码、滑窗调度及内存记账，并扩展独立测试。性能上应避免为非 DISTINCT 路径引入集合或额外分配，也不要为了确定遍历顺序而对 DISTINCT 集合排序，除非 SQL 语义确有要求。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可被完整定位。
- RustCodeGraph 文件节点：读取 `pkg/executor/aggfuncs/func_sum_int.rs` 全部 272 行，并确认使用者为 `aggfuncs.rs`、`spill_serialize_helper.rs`、`spill_helper_test.rs`；精确 `slide` 符号查询定位到有符号第 103 行和无符号第 175 行。对精确符号运行 `callers`/`callees` 未返回调用边，因此本文没有虚构函数级调用者。
- crate 与入口：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/lib.rs`；共享状态与接线：`pkg/executor/aggfuncs/aggfuncs.rs`；实现选择：`pkg/executor/aggfuncs/builder.rs`；DISTINCT spill：`pkg/executor/aggfuncs/spill_serialize_helper.rs`。
- Go 对照：`pkg/executor/aggfuncs/func_sum_int.go` 全部 480 行；滑窗回归：`pkg/executor/aggfuncs/func_sum_test.go` 的两个 `TestSlideSum*ProcessOutWindowFirstToAvoidOverflow`。
- Rust 测试证据：`pkg/executor/aggfuncs/func_sum_int_test.rs`（固定大小、reset 容量）；`func_distinct_agg_test.rs`（跨部分去重合并）；`go_scenario_coverage_test.rs`（普通部分和合并、DISTINCT）；`spill_helper_test.rs`（DISTINCT spill 往返与无符号比特模式）。本任务按计划不运行 Cargo，结论来自静态源码、索引关系和既有测试意图。

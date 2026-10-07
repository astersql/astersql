# `pkg/executor/typed_hash_agg.rs`

## 文件定位

本文件实现 Rust typed 物理执行链中的哈希聚合节点 `TypedHashAgg`。`pkg/executor/lib.rs` 将其公开为 `typed_hash_agg` 模块；`pkg/executor/builder.rs` 的 `build_typed_physical_plan` 在识别到 `PhysicalHashAgg` 时先递归构造唯一子节点，再把计划中的 `AggFuncs`、`GroupByItems` 和计划上下文交给 `TypedHashAgg::new`。因此它位于“规范化物理计划 → typed executor → child chunk”链路中，而不是 Go 执行器的通用 `aggregate.HashAggExec` 本体。

该类型实现 `crate::adapter::ExecExecutor`，遵循 `Open`、反复 `Next`/`NextWithContext`、`Close` 的拉取协议。crate 边界由 `pkg/executor/Cargo.toml` 的 `astersql-executor` 定义；本文件直接依赖表达式、聚合描述符、类型/Datum、chunk、codec、collation、错误和 `astersql-executor-aggfuncs` 等同 workspace crate，且不受 `nextgen` feature 条件编译控制。

## 核心职责

- `TypedHashAgg::new` 在执行前验证聚合函数种类、参数个数、模式和 `DISTINCT` 限制，并从每个 `AggFuncDesc.RetTp` 建立输出 schema。
- `prepare` 增量拉取子执行器的 chunk，但会在第一次输出前消费完全部输入；每行由 `update_row` 编码分组键并更新该组的聚合状态。
- `new_group` 为每个新分组、每个聚合描述符分配独立状态；普通聚合使用本文件的枚举状态，`MAX_COUNT`/`MIN_COUNT` 复用 `aggfuncs::func_max_min_count` 的类型化 evaluator 和 `PartialResult`。
- `append_group`/`finish_aggregate` 把内部状态物化成输出 Datum；`next_inner` 按下游 chunk 容量分页返回分组。
- 生命周期方法清空可复用状态、传播子节点的外键与扫描统计接口，同时明确聚合后不再保留输入行的 record identity 或锁键。

当前实现是串行、内存驻留、阻塞式聚合。它支持 `COUNT`、`SUM`、`SUM_INT`、`AVG`、`FIRST_ROW`、`MAX`、`MIN`、`MAX_COUNT` 和 `MIN_COUNT` 的受限 typed 路径；不应把 Go `HashAggExec` 已有的并行 worker、内存跟踪和磁盘 spill 能力推断为本文件现状。

## 主要符号

- `AggregateValue`：单个聚合的值状态。`Count(i64)` 保存计数；`Sum { value, count }` 同时服务 SUM 与 AVG；`First { seen, value }` 区分“尚无输入”与首值本身为 NULL；`Extremum(Datum)` 保存 MAX/MIN 当前极值；`CountExtrema { function, state }` 保存可处理具体数据类型的 evaluator 与其动态 partial state。
- `AggregateState`：把 `AggregateValue` 与可选的 `HashSet<Vec<u8>>` 组合；后者仅在 `AggFuncDesc.HasDistinct` 为真时创建，用编码后的实参元组去重。
- `GroupState`：一个分组的全部聚合状态，顺序与 `TypedHashAgg.functions` 一一对应。
- `TypedHashAgg`：唯一公开类型。`child` 是上游执行器；`functions`/`group_by`/`context` 来自物理计划；`schema` 是返回列类型；`groups` 保存稳定的分组输出顺序；`group_indexes` 从编码键映射到 `groups` 下标；`output_index` 支持分页；`prepared/opened/closed` 管理生命周期。
- `TypedHashAgg::new`：公开构造入口，返回 `Result<Self, String>`。它拒绝未知聚合、`DedupMode`、缺失必需参数，以及不受支持的 count-extrema 类型或 DISTINCT。
- `new_group`：内部状态工厂。对 `MAX_COUNT`/`MIN_COUNT` 再次构造 evaluator；这里的 `expect` 依赖 `new` 已完成同参数验证这一不变量。
- `check_cancel`：若 `ExecutionContext.sql_killer` 存在，则调用 `HandleSignal`，把 kill 信号作为执行错误传播。
- `prepare`/`update_row`：输入消费和逐行归组主流程。
- `evaluated_arguments`/`update_aggregate`/`finish_aggregate`：实参求值、状态转移、最终 Datum 生成三层辅助函数。
- `impl ExecExecutor for TypedHashAgg`：对接统一执行器协议；`ChunkConfig` 继承子节点容量但替换字段类型，`Detach` 明确返回 `None`。

## 执行流程

1. `pkg/executor/builder.rs::build_typed_physical_plan` 要求 `PhysicalHashAgg` 恰有一个 child，递归构建 child 后调用 `TypedHashAgg::new`。构造阶段即拒绝无法安全执行的聚合描述符。
2. `Open` 先打开 child，再清空 `groups`/`group_indexes`，重置输出游标与状态位。重复使用同一 executor 时不会继承上一轮分组。
3. 首次 `Next` 或 `NextWithContext` 进入 `next_inner`。未打开或已经关闭会立即报错；输出 chunk 总是先 `Reset`。
4. `prepare` 循环创建并复用 child chunk，按是否带上下文选择 `child.NextWithContext` 或 `child.Next`。每次拉取前以及每行处理前都检查取消；每个输入页后调用 `child.TakeLockKeys` 并丢弃结果，因为聚合输出不再对应具体 record identity。
5. `update_row` 在计划的 eval context 中逐个求值 `group_by`，以 `EncodeKey(eval.Location(), ..., group_values)` 生成分组键。新键追加 `GroupState` 并写入 `group_indexes`；已有键直接取得稳定下标。
6. 对每个 `(AggFuncDesc, AggregateState)` 调用 `update_aggregate`。count-extrema 直接委托 evaluator；其他函数先求值全部参数，除 `FIRST_ROW` 外遇任一 NULL 就跳过，再可选做 DISTINCT 编码去重，最后更新对应状态。
7. 输入耗尽后，如果没有任何分组且没有 GROUP BY，`prepare` 人工加入一个空组，使标量 `COUNT` 等仍输出一行；有 GROUP BY 的空输入保持零行。随后将 `prepared` 置真，后续分页不再重读 child。
8. `next_inner` 按 `groups` 插入顺序调用 `append_group`，直至输出 chunk 满或所有分组完成。若聚合函数列表为空，使用 virtual row 表示分组存在；否则 `finish_aggregate` 每列生成一个 Datum。
9. `Close` 幂等地标记关闭、释放分组容器并关闭 child；关闭后的 `Next` 被拒绝。

各聚合的关键转移如下：普通阶段 `COUNT` 每个非 NULL 参数元组加一，Final/Partial2 则累加输入 partial count；SUM 通过 `calculateSum` 累积；Final/Partial2 的 AVG 从第一个参数累计 count、第二个参数累计 sum；`FIRST_ROW` 只写第一次输入（包括 NULL）；MAX/MIN 使用参数类型的 collation 比较候选值；count-extrema 由共享 evaluator 统计极值的出现次数。

## 数据与状态

分组键和 DISTINCT 键都由 `astersql_util_codec::EncodeKey` 按会话 location 编码为 `Vec<u8>`，避免直接以异构 Datum 做哈希键。`group_indexes` 负责 O(1) 查找，`groups` 负责稳定输出顺序；代码没有依赖 `HashMap` 的遍历随机性。每个 group 的 `aggregates` 必须与 `functions` 等长且同序，这是 `new_group`、`update_row` 与 `append_group` 通过迭代 zip 共享的核心不变量。

空 Datum（`Datum::default()`）同时作为尚无 SUM/极值/首行结果时的 NULL 表示。AVG 额外用 `count == 0` 判断 NULL 结果；浮点结果做 `f64 / count`，十进制结果调用 `DecimalDiv` 并使用 eval context 的除法精度增量，其他累计值 kind 返回 NULL。COUNT 使用 `wrapping_add`，因此本文件不把整数溢出转换为错误。

内存规模随“分组数 × 聚合状态”增长，并额外包含每个 DISTINCT 聚合的已见参数集合以及分组键。输入 chunk 会复用，输入行和锁键不会长期保存，但实现没有显式 memory tracker、内存上限或 spill。`prepared` 表示 child 已完全归约，`output_index` 只描述已经输出的 group 数量；输出结束后继续 `Next` 会得到空 chunk。

## 依赖与调用关系

上游直接调用边为：`BuildTypedPhysicalPlan`/`BuildTypedPhysicalPlanWithBindings` → `build_typed_physical_plan` → `TypedHashAgg::new`，证据在 `pkg/executor/builder.rs`。运行时由 `ExecExecutor` 消费方调用 `Open`、`NewChunk`、`NextWithContext`/`Next` 和 `Close`；本文件没有独立线程入口。

主要下游依赖为：

- `child: Box<dyn ExecExecutor>`：提供输入、chunk 容量、外键级联/检查与扫描行数。
- `astersql_expression::ExprBox::Eval`：计算 GROUP BY 和聚合参数。
- `astersql_util_codec::EncodeKey`：编码分组和 DISTINCT 复合键。
- `astersql_expression_aggregation::calculateSum`：实现 SUM/AVG 的类型化加法。
- `Datum::Compare` 与 `astersql_util_collate::GetCollator`：实现遵循参数 collation 的 MAX/MIN。
- `astersql_executor_aggfuncs::func_max_min_count::{build_count_extrema_function, CountExtremaAgg}`：提供 MAX_COUNT/MIN_COUNT 的类型分派、partial state 更新和最终计数。
- `astersql_types::decimal::mydecimal::DecimalDiv`：完成十进制 AVG。

`pkg/executor/Cargo.toml` 明确声明以上 workspace crate 依赖；`pkg/executor/lib.rs` 公开生产模块并仅在 `cfg(test)` 下装入 `typed_hash_agg_test.rs`，保持 Rust 生产逻辑与测试分文件。

## 错误处理与边界

构造错误使用字符串，再由 builder 映射为 `BuildError`。参数边界包括：Final/Partial2 COUNT 至少一个参数；AVG 在 Final/Partial2 至少两个参数、其他模式至少一个；其余已支持函数至少一个参数；所有函数拒绝 `DedupMode`；MAX_COUNT/MIN_COUNT 拒绝 DISTINCT 及 evaluator 无法映射的参数类型。未知函数直接报告“不支持”，不会落入空实现。

运行错误统一为 `AdapterResult`。表达式求值、键编码、sum、比较、十进制除法、count-extrema evaluator 和 child 的错误都转换或原样向上返回。`Next` 的状态前置条件由 `!opened || closed` 检查；`Close` 幂等。取消只在 `NextWithContext` 提供 `ExecutionContext` 时生效，并在拉取 chunk 前、逐行前和逐组输出前检查；单次表达式或聚合函数内部没有额外取消点。

NULL 规则需要特别保持：除 `FIRST_ROW` 外，只要已求值参数中有 NULL 就跳过整次更新；`FIRST_ROW` 必须能返回首行的 NULL。空标量输入创建空组，空分组输入不创建组。DISTINCT 在 NULL 过滤后执行。MAX/MIN 的比较使用第一个参数的类型和 collation。

当前边界还包括：不支持 detach；聚合输出锁键永远为空；不保存 child 锁键；不提供 spill、并行执行、显式内存记账或 runtime stats；输出顺序是首次见到分组的顺序，但 SQL 无 ORDER BY 时调用方不应把它视为排序契约。

## 并发与资源生命周期

`TypedHashAgg` 自身不创建线程、任务或通道，也未声明内部同步原语。它通过 `&mut self` 串行更新所有状态，表达式上下文和 child 仍遵循 `ExecExecutor` 注释所述的 session/thread-bound 约束。`ExecutionContext` 中共享的 `SQLKiller` 是唯一显式跨调用协作对象。

资源生命周期是：`new` 只保存计划与 child → `Open` 打开 child 并初始化本轮状态 → 第一次 `Next` 完整消费 child 并持有所有聚合状态 → 多次 `Next` 分页物化结果 → `Close` 清空分组并关闭 child。`prepare` 成功前若发生错误，`prepared` 保持 false；调用方仍应走 `Close` 释放 child。`Close` 清空 `groups` 和 `group_indexes`，但不会单独清理每个 evaluator，Rust 所有权会随容器释放其动态 state。

外键相关方法大多透传 child，因为本节点不产生写行为；`IsWriteExecutor` 和 `CalculateNoDelay` 均为 false。`TakeLockKeys` 返回空而不是透传，`ScannedRows` 则透传 child。`Detach` 返回 `None`，避免把依赖 session 表达式上下文的聚合状态移交给 detached result set。

## 与 Go 版本的对应关系

最近的 Go 行为对照是 `pkg/executor/aggregate/agg_hash_executor.go::HashAggExec`，而非同名同路径文件。两者都以编码 group key 映射 partial state，先从 child 拉取 chunk、逐行更新，再分页输出；Go `unparallelExec` 同样在空输入且无 GROUP BY 时加入空组，并使用独立 group key 顺序数组避免遍历哈希集合造成随机分页。Rust 的 `groups + group_indexes` 对应 Go 的 `groupKeys/groupSet + partialResultMap`。

语义对齐点还包括 `Open` 重置状态、`Next` 重置输出 chunk、空标量聚合一行/空分组聚合零行，以及聚合函数分配 partial state 后更新并生成最终值。MAX_COUNT/MIN_COUNT 的具体语义由 Rust `pkg/executor/aggfuncs/func_max_min_count.rs` 移植层承担；Go 测试 `pkg/executor/aggfuncs/func_max_min_count_test.go` 证明 NULL、重复极值、类型覆盖和 partial merge 预期，Rust typed 测试则覆盖真实扫描与空输入结果。

重要差异是 Go `HashAggExec` 同时具有串行与多 worker 并行路径、memory/disk tracker、OOM spill、runtime/hash state stats、failpoint 和更通用的 `AggFunc` 集合；Rust `TypedHashAgg` 只实现聚焦的串行内存路径和白名单函数。Go 的 child/context 接口、锁处理和错误类型也不同。因此扩展时应对齐相关 Go 行为，但不能简单宣称两者功能等价。

## 扩展指南

新增普通聚合函数时，至少同步修改 `TypedHashAgg::new` 的验证白名单和参数约束、`AggregateValue`、`new_group`、`update_aggregate` 与 `finish_aggregate`；若可复用 `astersql-executor-aggfuncs` evaluator，应像 count-extrema 一样让构造时验证与运行时分配使用完全相同的参数，避免 `expect` 不变量失效。新增模式支持时要明确 Complete/Partial1/Partial2/Final 的输入列布局和输出布局，尤其 AVG 的 count/sum 双输入以及 COUNT 的 partial count 累加。

改变分组或 DISTINCT 编码时必须保留 SQL 类型、时区/location、collation 和 NULL 语义，并评估编码兼容性与内存放大。增加 spill、并行或内存限制不是局部枚举改动：需要设计 group state 可移动/可合并性、child 拉取与取消同步、错误汇聚、顺序/分页、资源回收和统计接口，且应以 Go worker/spill 实现为行为参考而不是直接简化。

回归测试应继续放在独立的 `pkg/executor/typed_hash_agg_test.rs`，不要嵌入生产文件。现有测试覆盖跨输入页归组和输出分页、空标量 COUNT、消费 child 前取消、MAX_COUNT/MIN_COUNT 的真实扫描及空输入；新增函数至少应补 NULL、DISTINCT、空输入、分组、多页输出、支持的 mode、错误传播和类型/collation 边界。若修改共享 count-extrema evaluator，还应同步检查 `pkg/executor/aggfuncs/func_max_min_count_test.go` 对应的 Go 语义证据和 Rust aggfuncs 自身独立测试。

## 验证依据

- 源文件：`pkg/executor/typed_hash_agg.rs`，核对了所有模块级 enum/struct、`TypedHashAgg` 全部方法、三个自由辅助函数及完整 `ExecExecutor` 实现；文件无条件编译项。
- 模块与构建入口：`pkg/executor/lib.rs` 的 `pub mod typed_hash_agg`/`#[cfg(test)] mod typed_hash_agg_test`；`pkg/executor/builder.rs` 的 `BuildTypedPhysicalPlan`、`BuildTypedPhysicalPlanWithBindings`、`build_typed_physical_plan` 及 `PhysicalHashAgg` 分支。
- crate 与接口：`pkg/executor/Cargo.toml`、`pkg/executor/adapter.rs::ExecExecutor`、`ExecutionContext`、`ChunkConfig`、`SchemaColumn`。
- 聚合依赖：`pkg/expression/aggregation/aggregation.rs::AggFunctionMode`；`pkg/executor/aggfuncs/func_max_min_count.rs::CountExtremaAgg` 和 `build_count_extrema_function`。
- Rust 独立测试：`pkg/executor/typed_hash_agg_test.rs` 的四项测试，分别覆盖跨页分组/分页、空标量输入、取消、count-extrema 真实扫描与空输入。
- Go 对照：`pkg/executor/aggregate/agg_hash_executor.go::HashAggExec`、`Open`、`Next`、`unparallelExec`、`execute`、`getPartialResults`，以及 `pkg/executor/aggfuncs/func_max_min_count_test.go`。
- RustCodeGraph：在项目根运行 `rustcodegraph status`，索引报告 11,467 个文件、307,296 个节点、1,848,419 条边；随后 `files --filter pkg/executor/typed_hash_agg` 返回无匹配，`query TypedHashAggExecutor --json` 返回空数组。由于目标文件未被当前图命中，调用边改由上述 builder/module 源码和 `rg` 精确引用核验；本文未把图中不存在的边当作已验证事实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以指定命令确认目标文件存在且恰有 11 个固定二级标题，并人工复核文档只陈述可由以上源码、配置和测试支持的现状。

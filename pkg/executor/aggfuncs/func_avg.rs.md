# `pkg/executor/aggfuncs/func_avg.rs`

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate 内，由 `pkg/executor/aggfuncs/lib.rs` 的 `pub mod func_avg` 声明纳入编译，提供 AVG 聚合所需的四类可复用状态：普通 Float64、普通 Decimal、DISTINCT Float64 和 DISTINCT Decimal。crate 边界由 `pkg/executor/aggfuncs/Cargo.toml` 定义；本文件直接依赖同 crate 的 `AggError`、`func_sum::Decimal`、`DistinctFloatSum` 与 `DistinctDecimalSum`，没有直接依赖表达式求值、`Row` 或 `Chunk`。

它处在“已经求值的输入/部分结果”与“最终平均值”之间：调用者把 `Option<T>` 形式的值或 `(count, sum)` 交给状态对象，状态对象负责跳过 NULL、累计、合并、滑窗撤销及求最终值。`builder.rs::build_avg` 能选择 AVG 对应的 `AggImplementation` 枚举；但截至当前代码，普通 `FloatAvg`/`DecimalAvg` 并未直接实现 `aggfuncs.rs::AggFunc`，因此本文件本身不是从表达式求值到输出 `Chunk` 的完整执行器。明确可见的生产接线是 DISTINCT 状态的 spill：`BuiltAggFunc::spill_function` 创建 `DistinctFloatAvg`/`DistinctDecimalAvg`，`merge_spilled_partial_result` 合并恢复状态，`spill_serialize_helper.rs` 为二者实现 `SpillState`。

## 核心职责

- 用 `FloatAvg { sum, count }` 和 `DecimalAvg { sum, count }` 保存普通 AVG 的二元部分状态；`Option::None` 表示 SQL NULL，不进入和与计数。
- 接受原始值与已经聚合的 `(count, sum)`。后者用于分区/两阶段聚合，字段顺序固定为“计数在前、和在后”，见 `update_partial` 与 `partial_result`。
- 通过 `merge` 把源状态加入目标状态；通过 `slide` 先加入窗口尾部的新值，再撤销窗口头部的旧值。
- 用 `DistinctFloatAvg`、`DistinctDecimalAvg` 复用 DISTINCT SUM 的集合、去重、合并和内存增量逻辑，最终以去重元素数为分母。
- 暴露两项固定状态大小常量，并以 Original/Partial/HighPrecision/DISTINCT 类型别名保留与 Go 实现的阶段命名对应关系。

## 主要符号

- `DEF_PARTIAL_RESULT_4_AVG_DECIMAL_SIZE`、`DEF_PARTIAL_RESULT_4_AVG_FLOAT64_SIZE`：分别是 `size_of::<DecimalAvg>()` 与 `size_of::<FloatAvg>()`，表示普通状态结构的固定内存大小。DISTINCT 的动态集合内存由底层 DISTINCT SUM 更新/合并方法另行返回。
- `FloatAvg`：私有字段 `sum: f64`、`count: i64`。公开方法包括 `reset`、`update`、`update_standard`、`merge`、`update_partial`、`slide`、`result`、`partial_result`；内部 `add_partial` 统一执行 `sum += ...` 与 `count += ...`。
- `DecimalAvg`：私有字段 `sum: Decimal`、`count: i64`。接口形状与 `FloatAvg` 对应，但所有可能触发 Decimal 运算失败的方法返回 `Result<_, AggError>`，内部 `add_partial` 使用 `Decimal::checked_add`。
- `DistinctFloatAvg`：只包装 `pub(crate) sum: DistinctFloatSum`。`update`/`merge` 返回动态容量增长的字节数；`result` 使用 `DistinctFloatSum::value()` 与 `len()`。
- `DistinctDecimalAvg`：包装 `DistinctDecimalSum`；最终求和可能失败，故 `result(result_scale)` 返回 `Result<Option<Decimal>, AggError>`。
- `AvgOriginal4Decimal`、`AvgPartial4Decimal`、`AvgOriginal4Float64`、`AvgOriginal4Float64HighPrecision`、`AvgPartial4Float64` 及四个 DISTINCT 别名：都是零成本类型别名，不引入不同字段或不同算法。尤其 HighPrecision 当前仍等同 `FloatAvg`，不是补偿求和实现。

## 执行流程

普通 Float64 路径如下：

1. `default` 或 `reset` 得到 `(sum=0, count=0)`。
2. `update` 委托给 `update_standard`，迭代器经 `flatten()` 丢弃 NULL；每个非 NULL 值通过 `add_partial(value, 1)` 顺序加入。
3. 两阶段输入调用 `update_partial`，逐项把 `(count, sum)` 原样加入；分区状态调用 `merge`，先取源的 `partial_result()`，再一次加入目标。
4. 滑动窗口 `slide(outgoing, incoming)` 严格先处理 incoming、后处理 outgoing；移出值等价于加入 `(-value, -1)`。这一先后次序会影响浮点舍入结果。
5. `result` 在 `count == 0` 时返回 `None`，否则返回 `sum / count as f64`。

Decimal 路径保持相同步骤，但加法、减法、除法分别使用 `checked_add`、`checked_sub`、`checked_div_i64` 并传播 `AggError`。`result(result_scale)` 只在 `count != 0` 时除法，避免空输入触发除零；实际 Decimal 实现会采用 `max(result_scale, sum.scale())` 并执行整数定点除法。

DISTINCT 路径把全部状态工作委托给对应 DISTINCT SUM：`update` 插入非 NULL 唯一值，`merge` 合并源集合并跨分区去重，`result` 先求唯一值之和再除以唯一元素数。空集合时底层 `value()` 返回 `None`，不会执行除法。

## 数据与状态

普通状态只有 `sum` 和 `count`，没有堆分配、集合或外部句柄。`count` 的语义是累计进状态的权重；原始行路径每个有效值增加 1，partial 路径则允许任意 `i64` 计数。代码没有校验计数非负，也没有忽略 `count == 0` 的非零 sum：`func_avg_test.rs` 明确验证 `(0, 5.0)` 后再加入一个值会得到 `(1, 6.0)`。因此调用者必须保证部分状态合法，不能把 `count == 0` 误解为“该 partial 一定为空”。

Float64 状态保留 IEEE-754 行为和输入顺序，不做 Kahan 等补偿；`[1e16, 1, -1e16]` 的回归期望为 0。计数使用普通 `i64 +=`/`-=`，没有显式 checked 或 wrapping 保护。

Decimal 是 `func_sum.rs` 中的定点值 `(i128 coefficient, u32 scale)`。加减前对齐 scale，系数/scale 溢出转为 `AggError`。DISTINCT Decimal 按规范化后的数值键去重，因此不同尾随零表示可视为同一值；DISTINCT Float 用线性 `Vec<f64>` 检查 `==`，使 `+0/-0` 合并，而 NaN 因不等于自身可重复保留。两种 DISTINCT 状态的 `update`/`merge` 返回容量或键数据增长值，供上层内存追踪。

## 依赖与调用关系

上游与装配关系：

- `lib.rs` 公开 `func_avg` 模块，并在 `#[cfg(test)]` 下装配 `func_avg_test`、`func_distinct_agg_test`、`go_scenario_coverage_test` 与 spill 测试。
- `builder.rs::build_avg` 按 `AggMode`、返回类型及 `has_distinct` 选择 `AvgOriginal*`/`AvgPartial*` 枚举；`windowing_use_high_precision` 只记录在 `AvgOriginalFloat64 { high_precision }` 变体中。
- `aggfuncs.rs::BuiltAggFunc::spill_function` 仅为 DISTINCT AVG 绑定本文件状态；`aggfuncs.rs::merge_spilled_partial_result` 通过类型擦除后的 downcast 调用其 `merge`。
- `spill_serialize_helper.rs` 的 `delegate_sum_spill!` 把 DISTINCT AVG 的读写委托给内部 DISTINCT SUM 状态，因此落盘格式与对应 DISTINCT SUM 一致。

下游依赖：

- `AggError` 来自 `aggfuncs.rs`，是 Decimal 运算错误的统一载体。
- `Decimal` 提供 checked 定点加、减、按整数除；`DistinctFloatSum` 和 `DistinctDecimalSum` 提供集合、求和、元素数、合并和动态内存增量。
- 本文件不持有 `EvalContext`、表达式、输入行或输出列；这些层面的求值、最终类型舍入和 `Chunk` 写入仍需上层适配。RustCodeGraph 对四个核心 struct 的 `callers/callees` 查询没有产生类型级调用边；文件级索引显示直接使用方包括 `aggfuncs.rs` 和 spill 相关测试，文本核验补充了上述精确接线。

## 错误处理与边界

- 所有输入 API 用 `Option` 表示 NULL，并通过 `flatten()` 忽略 NULL。全 NULL/空输入保持 `count == 0` 或空 DISTINCT 集合，结果为 `None`。
- Float64 更新、合并、滑窗和求值不返回错误；NaN、无穷大、舍入误差及异常 partial 会按原生 IEEE-754/算术继续传播。
- Decimal 的 scale 扩展、系数加减和最终除法均可能返回 `AggError`；方法使用 `?` 原样传播，未吞掉错误。由于先改 sum、后改 count，成功加法后计数才变化；滑窗中若处理中途失败，之前已经成功加入/移除的元素不会回滚，因此调用者不能假定错误时状态具备事务性。
- `result` 只以 `count != 0` 判断普通状态是否有值；负计数仍会参与除法。`update_partial` 也不验证 count 与 sum 的一致性。
- `DistinctFloatAvg::result` 在 `value()` 为 `Some` 时以 `len()` 为分母；按当前底层不变量，非空求和与正元素数同步。`DistinctDecimalAvg` 同样依赖 values/keys 的同步维护。
- 与 Go 相比，Rust `Decimal::checked_div_i64` 是简化定点整数除法；本文件没有 Go `AppendFinalResult2Chunk` 中的 `DivPrecisionIncrement`、返回类型 decimal 位处理和 `ModeHalfUp` 舍入，因此不能把 `result_scale` 视为完整复刻了 Go 最终格式化语义。

## 并发与资源生命周期

所有状态都通过 `&mut self` 更新，没有内部锁、原子变量、异步任务、通道或线程。`FloatAvg`/`DecimalAvg` 是 `Copy` 的值状态；DISTINCT 状态拥有各自的 `Vec` 数据并通过 `Clone` 深复制。并行聚合应由执行器为各 worker/分区分配独立状态，再在受控阶段调用 `merge`，不得让多个线程无同步地共享同一可变实例。

`reset` 复用状态对象：普通 AVG 用默认值整体覆盖；DISTINCT SUM 清空向量但通常保留 capacity，后续内存增量是 capacity 变化量而不是对象完整占用。spill 生命周期中，上层 `StateSerializer` 克隆模板、写入字节列、恢复状态，然后由 `merge_spilled_partial_result` 合并；本文件自身不管理文件、缓冲区或释放动作。固定大小常量只覆盖普通状态栈内字段，不代表 DISTINCT 堆内存总量。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_avg.go`：

- Rust `FloatAvg` 对应 Go `partialResult4AvgFloat64` 加 `avgOriginal4Float64HighPrecision`、`avgOriginal4Float64`、`avgPartial4Float64` 的核心 sum/count 算术；Rust `DecimalAvg` 对应 `partialResult4AvgDecimal`、`avgOriginal4Decimal`、`avgPartial4Decimal` 的核心算术。
- Rust `slide` 与 Go 一致地先加入 `[lastEnd, lastEnd+shiftEnd)`，再移出 `[lastStart, lastStart+shiftStart)`；NULL 均跳过。
- Go partial 更新逐行先取 sum、再取 count；Rust 已将表达式求值后的数据压缩为 `Option<(count, sum)>`，因此表达式错误与两个参数分别为 NULL 的分支不在本文件内。
- Go 普通 Decimal merge 在源 `count == 0` 时直接返回，不加入源 sum；Rust `DecimalAvg::merge` 无此过滤，且测试刻意保留“零 count 非零 sum”状态。Float Go merge 不过滤零 count，与 Rust一致。这是需要上层/兼容测试关注的语义差异。
- Go DISTINCT 使用内存感知 set/map；Rust通过 DISTINCT SUM 的向量与规范化键复用相同目标语义，并返回动态内存增量，但数据结构和复杂度不同：Rust Float 去重/Decimal 常规更新是线性查找。
- Go 基类负责分配、重置、表达式求值、最终写 Chunk、spill 及 Decimal 最终舍入；Rust 本文件只承载独立状态算法。当前 DISTINCT spill 已有接线，普通 AVG 的统一 `AggFunc`/spill 适配不能从本文件或搜索结果确认，故记为未接线而非已支持。
- Go 的 high-precision Float 类型与普通路径当前也执行顺序 `sum += input`；Rust 的 HighPrecision 类型别名保持了这一现状。

## 扩展指南

- 若改变普通 AVG 算法，应同步修改 `FloatAvg`/`DecimalAvg` 的 `update`、`update_partial`、`merge`、`slide` 与 `result`，保持原始、partial、合并、窗口四条路径的同一不变量；测试放在独立的 `pkg/executor/aggfuncs/func_avg_test.rs`，不要内嵌到源文件。
- 若新增真正的高精度 Float 算法，应把 `AvgOriginal4Float64HighPrecision` 从别名拆成独立状态，并同时检查 `builder.rs::AggImplementation::AvgOriginalFloat64 { high_precision }` 的运行时分派；浮点加法顺序是兼容性风险，必须新增与 Go 的明确差异/一致性用例。
- 若补全普通 AVG 执行器接线，需要在统一 `AggFunc` 适配层处理表达式求值、状态分配、最终 Chunk 输出、返回类型与 Decimal 舍入，并为 spill 序列化/恢复增加成对实现；只修改本文件的累加器不足以证明 SQL 路径可用。
- 若改变 DISTINCT 表示或哈希规则，应同步 `func_sum.rs`、`spill_serialize_helper.rs`、`aggfuncs.rs::spill_function`、`merge_spilled_partial_result` 以及 spill/并行 DISTINCT 测试。特别要覆盖 Decimal 尾随零、Float `+0/-0`、NaN、跨分区重复值和内存增量。
- 若改变 partial 协议，必须保持 `(count, sum)` 的顺序并核对 Go `avgPartial4*::UpdatePartialResult`、分区 merge 和序列化格式；异常/零/负 count 的处理需先形成显式兼容决策。
- 性能上，普通路径是 O(n) 且 O(1) 状态；当前 DISTINCT 的查重可能达到 O(n²)。优化为哈希结构时要保持 Go 的 Float/Decimal 相等语义及可复现的 spill 格式。

## 验证依据

- 目标源码：`pkg/executor/aggfuncs/func_avg.rs`（236 行），核对了两项常量、四个状态类型、全部方法、九个阶段别名及无条件编译项的事实。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/executor/aggfuncs` 确认目标、模块入口、Go 对照和测试均已索引；`node --file` 读取目标、`lib.rs`、`aggfuncs.rs`、`builder.rs`、spill helper 与测试；对 `FloatAvg`、`DecimalAvg`、`DistinctFloatAvg`、`DistinctDecimalAvg` 执行了 `query`、`callers`、`callees`，类型级调用边为空，因此又以文件使用关系和精确符号文本搜索核验接线。
- crate 与装配：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/lib.rs`、`pkg/executor/aggfuncs/builder.rs`、`pkg/executor/aggfuncs/aggfuncs.rs`、`pkg/executor/aggfuncs/spill_serialize_helper.rs`。
- 直接依赖：`pkg/executor/aggfuncs/func_sum.rs`，核对 Decimal checked 运算、Float/Decimal DISTINCT 相等语义、动态内存增量与求和行为。
- Go 对照：`pkg/executor/aggfuncs/func_avg.go`、`pkg/executor/aggfuncs/func_avg_test.go`，核对原始/partial/merge/slide、NULL、最终结果、内存与 benchmark 关注点。
- Rust 独立测试：`pkg/executor/aggfuncs/func_avg_test.rs` 验证 partial 合并、NULL、顺序浮点、零 count 非零 sum；`go_scenario_coverage_test.rs` 验证普通合并与 Decimal AVG；`func_distinct_agg_test.rs` 验证跨分区 DISTINCT、空集与 Decimal；spill 的接线还由 `spill_helper_test.rs` 和 `pkg/executor/aggregate/agg_spill_test.rs` 引用。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证文档存在且固定二级标题恰好为 11 个，并人工复核没有把类型别名、构建器枚举或源码注释误写成完整运行时接线。

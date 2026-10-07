# `pkg/executor/aggfuncs/func_sum.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；crate 根由 `pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/aggfuncs/lib.rs` 通过 `pub mod func_sum` 将其公开。它承载 SUM 的浮点、Decimal、DISTINCT 浮点、DISTINCT Decimal 状态算法，并提供一个供多个聚合模块复用的简化定点 `Decimal` 类型。整数 SUM 位于相邻的 `func_sum_int.rs`，不属于本文件。

本文件接收的是已经求值后的 `Option<f64>` 或 `Option<Decimal>`，不直接持有表达式、输入 `Row`、输出 `Chunk` 或 SQL 上下文。完整聚合链的上层入口在 `builder.rs::build_sum` 与 `aggfuncs.rs`：构建器按聚合阶段、返回类型和 DISTINCT 标志选择 `AggImplementation`，统一聚合层为 DISTINCT 状态建立 spill/merge 接线。普通 `FloatSum`、`DecimalSum` 在当前搜索结果中主要作为可复用状态及独立测试对象；不能仅凭 `SumFloat64`/`SumDecimal` 枚举存在就断言本文件已经实现 Go 版本完整的表达式求值、最终舍入和 Chunk 写入。

## 核心职责

- `Decimal` 用 `(coefficient: i128, scale: u32)` 表示定点数，负责 scale 对齐、checked 加减和按整数相除。
- `FloatSum` 与 `DecimalSum` 保存普通 SUM 的“和 + 非 NULL 行数”，支持清空、批量更新、分区合并、取值和滑动窗口更新。
- `DistinctFloatSum` 与 `DistinctDecimalSum` 保存去重后的值，支持跨分区合并、最终求和，以及向上层报告动态内存增长量。
- 七个 `Sum4*` 类型别名保留 Float/Decimal、普通/DISTINCT、Original/Partial/HighPrecision 的阶段命名，但别名不产生新的状态布局或算法。
- DISTINCT 状态可由 `spill_serialize_helper.rs` 序列化和恢复；Decimal DISTINCT 同时保存 key 与 value，保证 spill 后仍按原去重键合并。

## 主要符号

- `Decimal`：公开构造器 `new` 和访问器 `coefficient`、`scale`；内部 `normalized_key` 去掉尾随零，`checked_rescale` 只允许无损放大 scale；`checked_add`、`checked_sub`、`checked_div_i64` 返回 `Result<_, AggError>`。
- `FloatSum { sum, count }`：`update`/`update_standard` 顺序累加非 NULL 浮点值；`merge` 合并部分状态；`slide` 先加入 incoming、再减去 outgoing；`value` 在 `count == 0` 时返回 `None`。
- `DecimalSum { sum, count }`：与 `FloatSum` 具有相同状态语义，但加减可能失败；`add_partial` 是 `pub(crate)` 的 `(sum, count)` 合并入口。当前精确引用搜索未发现文件外调用者，不能把它写成已接线的 AVG 路径。
- `DistinctFloatSum { values }`：用 `Vec<f64>` 保存唯一值，`update` 以 `==` 线性查重，`merge` 复用 `update`，`value` 对向量求和。
- `DistinctDecimalSum { values, keys }`：普通 `update` 以 `Decimal::normalized_key` 判断数值相等；`insert_keyed` 接收已经计算好的 key，供 spill 恢复和跨状态合并使用；`merge` 按 key/value 对插入；`value` 用 `Decimal::checked_add` 求和。
- `Sum4Float64`、`Sum4Float64HighPrecision`、`Sum4Decimal`、`Sum4PartialDistinctFloat64`、`Sum4OriginalDistinctFloat64`、`Sum4PartialDistinctDecimal`、`Sum4OriginalDistinctDecimal`：均为类型别名。HighPrecision 当前仍是 `FloatSum`，没有补偿求和状态。

## 执行流程

普通 Float 路径从默认 `(sum=0, count=0)` 开始。`update` 遍历输入并用 `flatten()` 跳过 NULL，对每个有效值按输入顺序执行 `sum += value`，同时以 wrapping 加法增加计数。`merge` 忽略空源状态；空目标直接复制源，非空目标则把源 sum/count 加入。`value` 仅以计数是否为零决定输出 `None` 或 `Some(sum)`。滑窗 `slide(outgoing, incoming)` 先调用 `update(incoming)`，之后顺序减去 outgoing 并递减计数，因此浮点舍入次序与 Go 浮点 SUM 的先入后出顺序一致。

Decimal 路径保持相同框架。首个非 NULL 值直接赋给 `sum`，后续值先将双方 scale 对齐到较大者，再执行 checked 系数加法；成功后才增加计数。`merge` 对空源直接成功返回，对空目标整体复制，否则执行 checked 加法。`slide` 先完整调用 `update(incoming)`，再逐项 checked 减去 outgoing。任何一步出错都立即返回 `AggError`，此前已经成功执行的状态变化不会自动回滚。

DISTINCT 路径先收集唯一值，最终才求和。Float 更新用 `existing == value` 查重；Decimal 更新将尾随零规范化后比较，例如 `1.0` 与 `1.00` 共用一个逻辑键。跨分区 `merge` 再次按目标集合查重，避免分区间重复。空集合返回 SQL NULL 对应的 `None`；非空 Decimal 集合从首值开始 checked fold。spill 时 `SpillState` 将 Float 值序列或 Decimal 的 key/value 对写入缓冲，读回后重建集合，然后由 `aggfuncs.rs::merge_spilled_partial_result` 合并。

## 数据与状态

普通状态的关键不变量是：`count` 表示参与累加的非 NULL 项数，`count == 0` 时 `value` 返回 `None`。代码不校验调用者提供的部分计数是否非负或是否与 sum 一致；计数使用 `wrapping_add`/`wrapping_sub`，极端溢出会回绕而不是报错。Float 使用原生 IEEE-754 运算，NaN、无穷、舍入和输入顺序都可影响结果。

`Decimal` 不是 `types::MyDecimal`，而是本 crate 的简化定点结构。不同 scale 的加减先把较小 scale 的系数乘以 10 的幂；幂、乘法或加减溢出都会报错。`checked_div_i64` 拒绝零除数，但采用整数除法，且只把 scale 放大到 `max(result_scale, self.scale)`，没有在本文件中实现 Go 最终输出阶段的 HalfUp 舍入。

两种 DISTINCT 状态均拥有堆分配的 `Vec`。Float 的内存增量按 `values.capacity()` 的增长乘 `size_of::<u64>()` 计算。Decimal 常规 `update` 只返回 values 容量增长，不计新生成 key 的字节；`insert_keyed` 则返回 key 长度加 values 容量增长，且 `merge` 汇总每次插入值。因此这些返回值是当前路径定义的增量计量，不是状态的完整堆内存总量。`DistinctDecimalSum` 依赖 `keys` 与 `values` 保持同长度、同索引配对；字段只对 crate 内公开。

## 依赖与调用关系

上游装配与调用证据如下：

- `lib.rs` 公开 `func_sum`，并在 `#[cfg(test)]` 下独立装配 `func_sum_test.rs`、`func_distinct_agg_test.rs`、`go_scenario_coverage_test.rs` 和 spill 测试。
- `builder.rs::build_sum` 将非 DISTINCT Decimal 选为 `AggImplementation::SumDecimal`，其他普通返回类型选为 `SumFloat64 { high_precision }`；DISTINCT 则根据阶段和类型选择四个 Original/Partial 变体。
- `aggfuncs.rs::BuiltAggFunc::spill_function` 对 DISTINCT Decimal/Float 分别构造 `DistinctDecimalSum`、`DistinctFloatSum`；`merge_spilled_partial_result` 通过 downcast 后调用各自 `merge`。
- `spill_serialize_helper.rs` 为 `Decimal` 实现 `SpillElement`，为两个 DISTINCT SUM 状态实现 `SpillState`；`DistinctDecimalSum::insert_keyed` 是恢复 key/value 对的入口。
- `func_avg.rs` 直接复用 `Decimal`、`DistinctFloatSum`、`DistinctDecimalSum`，因此修改定点算术或 DISTINCT 语义会同时影响 AVG；`func_count_distinct.rs`、`func_first_row.rs`、`func_max_min.rs`、`func_percentile.rs`、`func_value.rs` 也把 `Decimal` 用作值类型。
- crate 的直接生产依赖只有 `crate::aggfuncs::AggError` 与标准库 `size_of`；crate 级外部依赖及可选功能边界记录于 `Cargo.toml`，本文件没有条件编译项。

RustCodeGraph 文件级结果显示该文件被 `aggfuncs.rs`、`func_group_concat.rs`、`func_percentile.rs`、`spill_serialize_helper.rs`、spill 测试等 13 个文件使用；精确类型查询找到了四个核心状态，但对重名的 `update`/`merge` 执行 callers/callees 没有返回可消歧方法边，所以以上精确接线以已索引源码引用和文本符号检索交叉核验。

## 错误处理与边界

- 所有更新方法都把 `None` 当作 SQL NULL 并跳过；空输入和全 NULL 输入保持空状态，最终为 `None`。
- Float 普通与 DISTINCT 路径没有 `Result`。NaN 可进入集合；因为 NaN 不等于自身，同一 NaN 位模式重复插入仍会形成多个元素，最终求和为 NaN。`+0.0 == -0.0`，二者会合并。
- Decimal scale 只能无损放大，不能缩小；10 的幂、系数缩放、加法、减法以及 DISTINCT 最终求和溢出均包装成 `AggError`。除数为零也返回 `AggError`。
- 普通 Decimal 的更新、合并和滑窗不是事务操作：批处理中后项失败时，前面成功的项仍留在状态内；滑窗 incoming 阶段溢出时，outgoing 尚未移除。
- DISTINCT Decimal 常规更新用格式化后的规范化 tuple 字节作为内部 key，而 spill 恢复接受外部已编码 key。`merge` 以保存的 key 为准；调用 `insert_keyed` 的上层必须确保 key 与 value 的相等语义一致。
- `reset` 清空 DISTINCT 向量但保留 capacity；这会影响后续报告的内存增量。普通 `reset` 则整体替换为默认状态。
- 本文件没有 Go `AppendFinalResult2Chunk` 的 Decimal 返回类型检查、scale 选择和 HalfUp 舍入，也没有表达式求值错误；这些边界属于完整执行适配层，不能由本状态实现替代。

## 并发与资源生命周期

所有可变操作都要求 `&mut self`，文件内没有锁、原子变量、异步任务、线程、通道或外部句柄。并行 HashAgg 应为各 worker/分区持有独立状态，在汇总阶段显式调用 `merge`；本文件不允许无同步地共享可变实例。`FloatSum`、`DecimalSum` 和 `Decimal` 为 `Copy` 值状态，DISTINCT 状态的 `Clone` 会复制其向量内容。

普通状态没有堆资源。DISTINCT 状态拥有 `Vec` 的容量，`reset` 后逻辑元素被清空但分配通常保留，最终由 Rust 所有权在状态析构时释放。spill 生命周期由上层 `StateSerializer` 管理：复制状态、写入字节缓冲、恢复新状态并合并；本文件只实现值/状态的转换接口，不管理 spill 文件或 I/O。部分更新失败不会提供回滚快照，调用者应丢弃或明确处理已部分修改的状态。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/aggfuncs/func_sum.go`，对应测试为 `func_sum_test.go`：

- `FloatSum` 对应 Go `partialResult4SumFloat64` 的 `val` 与 `notNullRowCount` 及 `baseSum4Float64`/`sum4Float64` 的核心更新、合并和滑窗算术；两者都跳过 NULL、顺序执行 `sum += value`，并在零有效行时输出 NULL。
- `DecimalSum` 对应 `partialResult4SumDecimal` 与 `sum4Decimal` 的核心 DecimalAdd/DecimalSub 状态逻辑。首个值直接赋值、源为空时合并不改变目标、滑窗先加 incoming 再减 outgoing，均与 Go 当前路径对应。
- Go 的 `AppendFinalResult2Chunk` 会检查返回类型、按目标小数位执行 `ModeHalfUp` 舍入并写入 Chunk；Rust 本文件只返回简化 `Decimal`，因此最终格式化语义尚不等价。
- `DistinctFloatSum` 对应 `set.Float64SetWithMemoryUsage` 的目标语义，包含 `+0/-0` 合并与 NaN 不自等；但 Rust 使用线性 `Vec`，迭代/求和顺序和性能特征不等同于 Go map。
- `DistinctDecimalSum` 对应 `StringToDecimalMapWithMemoryUsage` 与 `MyDecimal::ToHashKey` 的目标语义，尾随零不同但数值相同的 Decimal 只保留一个。Rust 的常规 key 是规范化 tuple 的调试字符串，不是 Go wire hash；spill 则保存其收到的 key/value 对。
- Go 类型还直接实现分配、重置、表达式求值、Chunk 输出、spill 序列化/反序列化；Rust 将状态、构建枚举和 spill 适配拆散在 `func_sum.rs`、`builder.rs`、`aggfuncs.rs`、`spill_serialize_helper.rs`。当前明确接线集中在 DISTINCT spill，普通状态的完整 SQL 执行接线未由直接引用证明。
- `Sum4Float64HighPrecision` 在 Rust 中只是 `FloatSum` 别名，仍保留 Go 当前简单顺序累加效果，不应解释为 Kahan 或其他补偿算法。

## 扩展指南

- 修改普通 SUM 时应同时检查 `update`、`merge`、`slide`、`value` 和 `reset`，保持 NULL、空源、空目标与先 incoming 后 outgoing 的行为一致；回归测试放在独立的 `pkg/executor/aggfuncs/func_sum_test.rs`。
- 若完善普通 SUM 的 SQL 执行接线，需要同步审查 `builder.rs::build_sum`、统一 `AggFunc` 状态分配/更新/最终输出层及 spill 协议。只增加类型别名或构建枚举不能证明 SQL 路径可用。
- 若引入真正高精度 Float SUM，应将 `Sum4Float64HighPrecision` 拆成独立状态，并为普通更新、merge、slide、窗口开关和浮点顺序兼容性建立独立测试；结果变化属于 SQL 兼容风险。
- 修改 `Decimal` 必须联动检查 `func_avg.rs`、`func_count_distinct.rs`、`func_first_row.rs`、`func_max_min.rs`、`func_percentile.rs`、`func_value.rs` 及 `spill_serialize_helper.rs`。尤其要覆盖 scale 对齐、正负数、溢出、除零、序列化往返和 Go 最终舍入差异。
- 修改 DISTINCT 数据结构或 key 规则时应同步 `aggfuncs.rs::spill_function`、`merge_spilled_partial_result` 和 spill codec；测试至少覆盖跨分区重复、空集、NULL、Float `+0/-0`、NaN、Decimal 尾随零和恢复后再次 merge。
- 性能上普通路径为 O(n) 时间/O(1) 状态；当前 DISTINCT 线性查重在 n 个唯一值时可能达到 O(n²)。替换为哈希结构时必须保留 Go 的浮点相等语义、Decimal 规范化语义、内存增量契约及 spill 可恢复性。
- 错误路径测试应验证状态是否已部分修改，因为当前批量 update/slide 不回滚；若未来要求原子性，需要在接口和调用者层明确新增事务式状态管理，而不能只改错误文本。

## 验证依据

- 目标源码：`pkg/executor/aggfuncs/func_sum.rs`（364 行），核对了 1 个值类型、4 个状态类型、全部方法、7 个类型别名、无条件编译项及可见性。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/executor/aggfuncs/func_sum.rs` 确认目标被索引；`node --file ... --offset 1 --limit 500` 读取全文件；`query` 找到 `FloatSum`、`DecimalSum`、`DistinctFloatSum`、`DistinctDecimalSum`；`callers`/`callees` 对这些类型及重名方法未输出可消歧边，因此用图给出的文件级使用关系和精确文本引用补证。
- crate 与模块：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/lib.rs`。
- 生产接线：`pkg/executor/aggfuncs/builder.rs::build_sum`、`pkg/executor/aggfuncs/aggfuncs.rs::{BuiltAggFunc::spill_function, merge_spilled_partial_result}`、`pkg/executor/aggfuncs/spill_serialize_helper.rs`、`pkg/executor/aggfuncs/func_avg.rs`。
- Go 对照：`pkg/executor/aggfuncs/func_sum.go`、`pkg/executor/aggfuncs/func_sum_test.go`，核对普通/DISTINCT Float 与 Decimal 的更新、合并、滑窗、内存计量、最终输出和 spill 结构；整数专用滑窗测试只作为相邻边界证据，不归入本文件实现。
- Rust 独立测试：`pkg/executor/aggfuncs/func_sum_test.rs` 验证普通 merge/slide、顺序浮点、Decimal 中间溢出、Float 键相等和 Decimal 规范化；`func_distinct_agg_test.rs` 验证跨分区 DISTINCT SUM 与空集；`go_scenario_coverage_test.rs` 覆盖 Go 场景；`spill_helper_test.rs` 验证 DISTINCT Float/Decimal 往返和恢复后合并。
- 本任务只新增说明文档，未修改 Rust/Go/Cargo，按计划不运行 Cargo。交付时运行任务指定的 11 标题结构命令，并人工复核“文件为何存在、状态如何运行、完整接线边界、如何安全扩展”均有直接证据。

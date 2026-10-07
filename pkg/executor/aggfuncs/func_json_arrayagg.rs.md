# `pkg/executor/aggfuncs/func_json_arrayagg.rs`

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate，由 `pkg/executor/aggfuncs/lib.rs` 以 `pub mod func_json_arrayagg` 公开。它提供 Rust 侧 `JSON_ARRAYAGG(expr)` 的一个轻量有序部分状态 `JsonArrayAgg`：收集已经转换成 `SpillValue` 的元素、合并两份状态、重置状态并暴露结果切片。

需要区分“模块已公开”和“完整执行链已接通”。`pkg/executor/aggfuncs/builder.rs::build` 会把 `FunctionName::JsonArrayAgg` 选择为 `AggImplementation::JsonArrayAgg`，但当前生产代码没有把该枚举分支构造成或调用本文件的 `JsonArrayAgg`；RustCodeGraph 显示该结构体的直接使用者是测试文件。因此，本文件目前是可执行、受测试的状态逻辑，而不是 Go `jsonArrayagg` 执行器接口的完整替代品。

## 核心职责

- `JsonArrayAgg::update` 按迭代器顺序把异构 JSON 可表示值追加到组内状态，同时返回与 Go `getValMemDelta` 相同意图的逐值内存增量。
- `JsonArrayAgg::merge` 把来源状态完整追加到目标末尾，保持“目标已有元素在前、来源元素在后”的相对顺序。
- `JsonArrayAgg::result` 把空状态映射为 `None`，对应聚合无输入时结果为 SQL `NULL`；非空状态返回借用切片，避免复制。
- `JsonArrayAgg::reset` 清空逻辑内容以便复用同一状态。

本文件不负责表达式求值、SQL 类型到 JSON 值的转换、最终 `BinaryJSON` 构造、写入结果 chunk，也不负责 spill 编解码；这些职责在 Go 对照实现或 Rust 的其他类型/模块中。

## 主要符号

- `pub struct JsonArrayAgg { entries: Vec<SpillValue> }`：唯一公开类型。字段私有，调用者只能通过方法维护有序状态；派生 `Clone`、`Debug`、`Default`、`PartialEq`，便于创建空状态、复制来源状态和测试比较。
- `pub fn update(&mut self, values: impl IntoIterator<Item = SpillValue>) -> i64`：批量消费输入值。每个值先经 `value_memory_delta` 计量，再被移动进 `entries`。
- `pub fn merge(&mut self, s: &Self)`：克隆 `s.entries` 后追加。来源保持不变，但目标为每个变长值分配/复制拥有的数据。
- `pub fn reset(&mut self)`：调用 `Vec::clear`；长度归零，但通常保留已分配容量。
- `pub fn result(&self) -> Option<&[SpillValue]>`：空向量返回 `None`，否则返回只读切片。
- `fn value_memory_delta(value: &SpillValue) -> i64`：私有内存计量函数。每个值固定加 `DEF_INTERFACE_SIZE`，定长类型加对应常量，字符串按 `len()`，`BinaryJson.Value` 和 `Opaque.Buf` 按有效长度再加一个类型码字节。

文件没有模块级常量、trait、条件编译项或错误类型。

## 执行流程

1. 上游先把表达式结果转换成 `SpillValue`；本文件不接受原始 row 或表达式上下文。
2. `update` 顺序遍历输入。对每个元素计算内存增量，将增量累加到局部 `i64`，再把元素追加到 `entries`。
3. 分阶段聚合需要组合状态时，目标调用 `merge(&source)`；目标原序列保持为前缀，来源序列成为后缀，来源本身不变。
4. 求最终逻辑结果时调用 `result`。没有元素得到 `None`；有元素得到与内部顺序一致的借用切片。把该切片编码为 SQL JSON 值并写入结果列不在本文件内。
5. 状态复用时调用 `reset`，之后 `result` 再次为 `None`。

顺序是本实现的关键不变量。`pkg/executor/aggfuncs/func_json_arrayagg_test.rs::json_arrayagg_preserves_order_memory_accounting_and_merge_order` 验证更新顺序、目标/来源合并顺序、来源不变和重置语义。

## 数据与状态

`entries` 是状态的唯一可变数据，元素类型定义在 `pkg/executor/aggfuncs/aggfuncs.rs::SpillValue`，覆盖 `Bool`、有符号/无符号整数、浮点、字符串、`BinaryJSON`、`Opaque`、`Time` 和 `Duration`。该枚举没有 `Null` 变体；因此与 Go 会把 `nil` 直接放入 `[]any` 不同，本 API 本身不能表示 JSON `null` 输入。

内存返回值只描述本次 `update` 的值口径，不含 `Vec` 结构、容量增长或 allocator 开销。字符串和字节缓冲按当前有效长度 `len()` 计，而 `SpillValue::memory_usage` 的通用辅助方法使用 `capacity()`；调用方不能把两者当成同一统计口径。`merge` 不返回内存增量，即使其克隆会产生实际分配。

`reset` 保留 `Vec` 容量，所以逻辑状态为空不等于释放了历史峰值容量。`result` 返回的切片生命周期绑定到 `&self`；在借用结束前不能可变更新同一状态。

## 依赖与调用关系

直接依赖全部来自 `crate::aggfuncs`：`SpillValue` 以及 `DEF_BOOL_SIZE`、`DEF_INT64_SIZE`、`DEF_UINT64_SIZE`、`DEF_FLOAT64_SIZE`、`DEF_TIME_SIZE`、`DEF_DURATION_SIZE`、`DEF_INTERFACE_SIZE`。`SpillValue` 的 `BinaryJson`、`Opaque`、`Time`、`Duration` 间接来自 `astersql-util-serialization`；该依赖由 `pkg/executor/aggfuncs/Cargo.toml` 声明。

模块入口是 `pkg/executor/aggfuncs/lib.rs`。RustCodeGraph 的直接使用证据包括 `func_json_arrayagg_test.rs` 和 `go_scenario_coverage_test.rs`；后者直接创建、更新、合并、重置 `JsonArrayAgg`。`builder.rs::build` 的 JSON 分支拒绝 `AggMode::Dedup` 并返回 `AggImplementation::JsonArrayAgg`，但没有到本结构体方法的静态调用边。

spill 路径也要避免混淆：`spill_serialize_helper.rs::serialize_json_array` 和 `spill_deserialize_helper.rs::deserialize_json_array` 操作的是 `aggfuncs.rs::JsonArrayPartialResult`，不是本文件的 `JsonArrayAgg`。两者都持有 `Vec<SpillValue>`，语义相关但当前是不同类型，不能据此声称本类型已经完成 spill 接线。

## 错误处理与边界

本文件的方法均不返回 `Result`，也没有显式 panic 分支。类型约束把输入限制在 `SpillValue` 已支持的九种变体，故 `value_memory_delta` 是穷尽匹配。

主要边界如下：空输入使 `update` 返回 `0` 且不改变状态；空状态的 `result` 为 `None`；重复调用 `reset` 安全；合并空来源不改变目标；将状态与自身合并在借用规则下不能直接写成 `state.merge(&state)`。极端情况下，累计 `i64`、长度到 `i64` 的转换或内存分配可能溢出/失败，本文件没有额外保护。

Go 实现会传播表达式求值、JSON 值转换、最终 JSON 构造及不支持类型错误；Rust 本文件因不执行这些步骤，没有对应错误通道。这是当前职责边界，不应解释为完整执行器不会出错。

## 并发与资源生命周期

`JsonArrayAgg` 没有锁、原子变量、任务、通道或共享所有权。所有修改都要求 `&mut self`，并发隔离由调用方和 Rust 借用规则负责；文件未声明跨线程共享协议。

输入 `SpillValue` 在 `update` 中转移所有权，随 `JsonArrayAgg` 生命周期保存；`merge` 克隆来源元素，因此两个状态之后独立拥有数据；`result` 仅借用，不延长状态生命周期；`reset` 丢弃元素但保留向量容量；结构体析构时由 Rust 递归释放向量及元素拥有的缓冲。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_json_arrayagg.go`。`partialResult4JsonArrayagg.entries []any` 对应 Rust 的有序元素向量；`ResetPartialResult` 对应 `reset`；`MergePartialResult` 的 `dst = append(dst, src...)` 对应目标调用 `merge(source)`；空 partial 最终写 `NULL` 的判断对应 `result() == None`；`getValMemDelta` 的值统计意图对应 `value_memory_delta`。

Rust 当前只覆盖上述状态核心，未等价覆盖以下 Go 行为：

- `AllocPartialResult` 返回 partial 结构和切片基础内存；Rust `Default` 不返回该内存增量。
- `UpdatePartialResult` 逐 row 求值、调用 `getRealJSONValue`、接受 `nil` 并传播转换/类型错误；Rust `update` 只接受已转换且非 null 的 `SpillValue`。
- `AppendFinalResult2Chunk` 调用 `CreateBinaryJSONWithCheck` 并写 chunk；Rust `result` 只返回切片。
- `SerializePartialResult`、`DeserializePartialResult` 和 `deserializeForSpill` 由 Go 类型直接实现；Rust spill helper 当前接到另一个 `JsonArrayPartialResult`。
- Go merge 返回零内存增量且通常只复制 interface 槽位；Rust merge 深克隆拥有型 `String`/字节缓冲，实际成本不同但同样不报告增量。

Go 测试 `func_json_arrayagg_test.go` 覆盖整数、浮点、字符串、JSON、日期、时长、decimal 转换、NULL、合并和内存统计。Rust 独立测试覆盖现有 `SpillValue` 变体的计量、顺序、合并和重置，但没有证明上述完整 Go 执行器接口已经移植。

## 扩展指南

若增加 `SpillValue` 变体，必须同步修改 `value_memory_delta`，并在独立文件 `pkg/executor/aggfuncs/func_json_arrayagg_test.rs` 增加计量和顺序断言；还应检查 spill 编解码 helper 的同类匹配。不要把测试内嵌进生产 `.rs` 文件。

若要把本状态接入生产执行链，应在 `AggImplementation::JsonArrayAgg` 的实际分派层明确选择统一的 partial 类型，解决 `JsonArrayAgg` 与 `JsonArrayPartialResult` 的重复表示，并补齐表达式求值、NULL 表示、JSON 转换、最终编码、错误传播和 spill 往返测试。不能只依据 builder 枚举存在就认为接线完成。

若调整 merge，必须保留分区合并的顺序契约，并决定克隆成本是否计入内存跟踪；若调整计量口径，应与 Go `getValMemDelta` 和 `func_json_arrayagg_test.go::jsonArrayaggMemDeltaGens` 对齐，特别关注 `len`/`capacity`、JSON 类型码、NULL、decimal 到浮点转换和向量容量。

兼容性风险主要是 SQL NULL/JSON null 区分、输入顺序、跨 partial 合并顺序和类型转换；性能风险主要是逐元素深克隆、向量扩容以及计量值与实际容量偏差。

## 验证依据

- 目标源码：`pkg/executor/aggfuncs/func_json_arrayagg.rs`，符号 `JsonArrayAgg::{update, merge, reset, result}` 与 `value_memory_delta`。
- crate/模块边界：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/lib.rs`。
- 公共状态类型：`pkg/executor/aggfuncs/aggfuncs.rs::SpillValue`、`JsonArrayPartialResult`。
- 构建入口：`pkg/executor/aggfuncs/builder.rs::build`、`AggImplementation::JsonArrayAgg`；`builder_test.rs::build_rejects_dedup_json_and_invalid_percentile_modes`。
- Rust 独立测试：`pkg/executor/aggfuncs/func_json_arrayagg_test.rs`；补充场景位于 `pkg/executor/aggfuncs/go_scenario_coverage_test.rs`。
- Go 对照与测试：`pkg/executor/aggfuncs/func_json_arrayagg.go`、`pkg/executor/aggfuncs/func_json_arrayagg_test.go`。
- spill 邻接证据：`pkg/executor/aggfuncs/spill_serialize_helper.rs::serialize_json_array`、`spill_deserialize_helper.rs::deserialize_json_array`。
- RustCodeGraph 查询：`status` 确认索引含 11,467 个文件；`query JsonArrayAgg`、`query json_arrayagg`、`node` 读取上述符号/文件，调用关系未发现生产代码到本结构体方法的静态边，直接使用落在测试文件。由于方法名 `update`/`merge` 在全仓高度重载，结论以带文件的节点、精确文本引用和模块入口交叉核验。

本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工核对文档未把未接线能力写成已支持。

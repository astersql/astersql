# `pkg/executor/aggfuncs/func_value.rs`

## 文件定位

源码：[func_value.rs](./func_value.rs)。

本文件属于 `astersql-executor-aggfuncs` crate（见同目录 `Cargo.toml`），由 `lib.rs` 以 `pub mod func_value` 暴露。它实现 `FIRST_VALUE`、`LAST_VALUE`、`NTH_VALUE` 三类窗口取值状态机，以及这些状态机共享的带 SQL NULL 状态和值内存记账的 `ValueEvaluator<T>`。

在窗口函数构建链中，`builder.rs::build_window_function` 会把 `FunctionName::{FirstValue, LastValue, NthValue}` 映射为相应的 `AggImplementation` 元数据，并以返回字段类型选择 `ValueKind`。当前仓库搜索未发现生产 Rust 代码把这些元数据进一步实例化为本文件的 `FirstValue`、`LastValue` 或 `NthValue`；直接使用者是独立测试 `func_value_test.rs` 与 `window_func_test.rs`。因此，本文件已有可执行并受测试的局部状态机，但其完整 Rust 执行器接线不能仅凭现有代码认定为完成。

## 核心职责

- 用 `ValueEvaluator<T>` 区分三种状态：尚未求值、已求值且值为 SQL NULL、已求值且有非 NULL 值。这个区分由 `evaluated: bool` 与 `value: Option<T>` 共同表达，不能只看 `Option<T>`。
- 用 `ValueMemory::retained_size` 计算替换持有值前后的额外内存差，供调用方累计内存变化。固定宽度类型返回 0；字符串、二进制 JSON 和 Float32 向量按其拥有的负载字节数计算。
- 按批次消费 `&[Option<T>]`：`FirstValue<T>` 只接受整个分区的第一行，`LastValue<T>` 每个非空批次覆盖为末行，`NthValue<T>` 跨批次累计行数并只在第 N 行落入当前批次时取值。
- 提供与 Go 类型专用求值器相对应的 Rust 类型别名，并由 `evaluate_float32` 保留 Go 中从 `float64` 求值结果窄化为 `float32` 的行为。

## 主要符号

- `ValueMemory`：公开 trait，仅包含 `retained_size(&self) -> usize`。`fixed_value_memory!` 为 `i64`、`f32`、`f64`、`Decimal`、`TimeValue`、`DurationValue` 实现零额外内存；`String` 使用字节长度，`BinaryJson` 使用 `value.len()`，`VectorFloat32` 使用元素数乘 `size_of::<f32>()`。
- `ValueEvaluator<T>`：公开泛型结构体，字段私有。`evaluate` 替换值、设置 presence 并返回 `after - before`；`reset_presence` 只清除 presence；`result` 返回双层可选值 `Option<Option<&T>>`。
- `Value4Int`、`Value4Float32`、`Value4Float64`、`Value4Decimal`、`Value4Time`、`Value4Duration`、`Value4String`、`Value4Json`、`Value4VectorFloat32`：覆盖 Go `value4*` 类型族的公开别名。
- `evaluate_float32`：接受 `Option<f64>`，对非 NULL 值执行 `as f32` 后交给 `ValueEvaluator<f32>::evaluate`。
- `FirstValue<T>`：保存一个私有求值器；`update` 在首次遇到非空批次时取批次首元素，此后的批次不再覆盖。
- `LastValue<T>`：保存一个私有求值器；每个非空批次由 `update` 取末元素并覆盖旧值。
- `NthValue<T>`：除求值器外保存 1-based 的 `nth` 与累计的 `seen_rows`；`new`、`update`、`reset` 和 `result` 共同维护跨批次定位。

文件没有条件编译项、异步函数、锁、通道或 unsafe 代码。泛型窗口状态机要求 `T: ValueMemory`，需要从输入行复制值的 `update` 还要求 `T: Clone`。

## 执行流程

1. 调用方为一个分区创建 `FirstValue::default()`、`LastValue::default()` 或 `NthValue::new(nth)`；初始 `ValueEvaluator` 的 `evaluated` 为 false。
2. 每次传入一个批次时，输入元素用 `Option<T>` 表示 SQL 值，其中 `None` 是 SQL NULL，而空切片表示本批没有行。
3. `FirstValue::update` 若已经求值立即返回 0；否则只在切片有首元素时复制该元素并求值。因此“第一行是 NULL”仍会锁定首行，后续非 NULL 行不会覆盖它。
4. `LastValue::update` 只在切片非空时复制最后一个元素。后续非空批次会覆盖旧值，所以最终结果是所有已见行的最后一行。
5. `NthValue::update` 先拒绝 `nth == 0`。对有效 N，它判断 `nth` 是否处于 `(seen_rows, seen_rows + rows.len()]`，命中时以 `nth - seen_rows - 1` 计算当前批次的零基下标；随后无论命中与否都累加 `seen_rows`。
6. 三种状态机命中目标行时调用 `ValueEvaluator::evaluate`：先计算旧值和新值的保留内存，再替换值、标记 presence，最后返回有符号差值。
7. `result` 以借用形式返回结果，不复制负载。`None` 表示目标行尚不存在；`Some(None)` 表示目标行存在且为 SQL NULL；`Some(Some(&T))` 表示非 NULL 值。
8. 分区结束后调用 `reset` 可开始下一分区。重置不会释放已有值缓冲；下一次求值的内存差仍相对于该旧值计算。

## 数据与状态

`ValueEvaluator<T>` 的关键不变量是 `evaluated` 与 `value` 正交：`evaluated == false` 时旧 `value` 可能仍然存在，因为 `reset_presence` 有意复用它；`evaluated == true && value.is_none()` 才表示已取得 SQL NULL。调用方必须通过 `result` 而不是直接把 `value == None` 当成“未求值”。

内存返回值是 `i64` 差值，允许为负。`func_value_test.rs::value_evaluators_cover_every_go_specialization_and_memory_rule` 验证字符串从 5 字节替换为 2 字节返回 -3，再替换为 NULL 返回 -2；JSON 与向量也验证缩短时的负增量。该值只覆盖求值器结构体之外的拥有型负载，不包括结构体自身、容器容量或分配器开销。

`NthValue<T>::seen_rows` 是跨批次的单调累计计数，在 `reset` 时归零。`nth` 构造后不变且按 1 起算；0 被作为无效值处理。代码把 `rows.len()` 转为 `u64` 并执行普通加法，没有额外的溢出保护。

## 依赖与调用关系

- 本文件从 `func_max_min.rs` 复用 `BinaryJson`、`DurationValue`、`TimeValue`、`VectorFloat32`，从 `func_sum.rs` 复用 `Decimal`；标准库只使用 `std::mem::size_of`。
- `lib.rs` 公开模块，但没有在 crate 根再次 `pub use` 这些具体符号；外部使用路径是 `astersql_executor_aggfuncs::func_value::*`。
- `builder.rs::build_window_function` 是最近的上游生产接线证据：它识别三个窗口函数并生成 `AggImplementation::{FirstValue, LastValue, NthValue}`，其中 N 来自第二个常量参数的 `as_u64()`。
- RustCodeGraph 对本文件建立了 28 个符号，但针对 `ValueEvaluator`、三个窗口类型、`evaluate_float32` 和重载 `update` 的精确 callers/callees 查询没有返回局部调用边。仓库级文本搜索进一步确认，生产 Rust 代码只有 `builder.rs` 中同名实现枚举，直接状态机调用位于测试中。
- `func_value_test.rs` 验证 NULL/presence、类型别名、Float32 窄化、拥有型内存差、分批更新和 reset；`window_func_test.rs` 从窗口函数场景覆盖 Go 测试中的主要类型和 first/last/nth 行为。
- 同目录 `Cargo.toml` 将本目录定义为独立 library crate，库入口为 `lib.rs`；本文件本身没有直接使用该清单中的外部 workspace crate，因为值类型来自 crate 内相邻模块。

## 错误处理与边界

本文件的 API 不返回 `Result`，也不执行表达式求值、类型检查或向结果 chunk 写入，因此没有可传播的运行时错误。与 Go 版本相比，表达式求值错误边界被留在本状态机之外；调用者必须先产生类型正确的 `Option<T>`。

明确边界如下：空批次不改变任何状态；`FirstValue` 的首行即使是 NULL 也算已求值；`LastValue` 会被每个后续非空批次覆盖；`NthValue(0)` 永不求值且 `result` 永远为 `None`；行数不足时 `NthValue::result` 为 `None`；命中行是 NULL 时返回 `Some(None)`。`evaluate_float32` 使用 Rust `as` 窄化，遵循 Rust 的浮点转换语义，不报告精度丢失。

内存大小计算存在刻意的口径限制：`String` 使用当前长度而非 capacity，`BinaryJson` 不计 `type_code` 与 Vec 自身，向量按 `len * 4` 计负载。若调用方把返回值当作完整堆占用会低估固定元数据和预留容量。

## 并发与资源生命周期

三个状态机都通过 `&mut self` 更新，没有内部同步或共享可变状态；线程安全性只取决于具体 `T` 及外部所有权方式。它们适合由单个窗口分区的执行上下文独占，不应在没有外部同步的情况下并发修改同一实例。

值在命中时通过 `Clone` 进入求值器并由其拥有，结果只以受 `&self` 生命周期约束的引用暴露。对于字符串、JSON 和向量，这个所有权避免结果依赖输入批次缓冲的后续变化。`reset` 只清除逻辑 presence，不主动 drop 旧值；旧负载持续到下一次 `evaluate` 替换或整个状态机析构，这与测试中 reset 后首次更新可能返回 0 或负增量的复用语义一致。

本文件不创建任务、线程、锁、文件句柄、网络连接或事务，也没有显式清理协议；资源释放依赖 Rust 所有权和析构。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_value.go`。Rust `ValueMemory` 加各 `Value4*` 别名对应 Go 的 `valueEvaluator` 与 `value4Int/value4Float32/...` 类型族；Rust 将 Go 各类型重复的求值器存储逻辑统一为泛型，但不包含 Go 的 `expression.Expression.evaluateRow` 和 `chunk.Append*` 接口。

`FirstValue` 对应 Go `firstValue` 与 `partialResult4FirstValue`：Go 用 `gotFirstValue` 区分空窗口与首行 NULL，Rust 用 `evaluated`。`LastValue` 对应 `lastValue` 与 `gotLastValue`。`NthValue` 对应 `nthValue` 与 `partialResult4NthValue.seenRows`，两端都按 1-based N 跨批次定位，并把 N=0 或行数不足视为无结果。

内存差口径基本对齐：固定宽度类型为 0，字符串按长度，JSON 按二进制负载长度，向量按估算负载。Go 在 JSON 和向量求值后显式深拷贝以脱离 chunk 缓冲；Rust 输入已是拥有型 `T`，`update` 再 Clone 后保存。Go 还报告 partial-result 与 evaluator 结构体的初始大小，而本文件的更新 API只报告替换值的负载差；结构体初始内存需要由更上层另行计算，当前 Rust 生产接线未显示这一部分。

Go `AppendFinalResult2Chunk` 在未命中时直接向结果列追加 NULL；Rust 仅返回三态借用，由尚未发现的上层适配器负责写结果。Go 的表达式求值可能返回错误，Rust 本文件不承担这一职责。这些是职责边界差异，不应把 Rust 局部状态机描述成 Go 完整执行对象的等价替换。

Go 测试 `func_value_test.go::TestMemValue` 覆盖各类型的初始内存与更新内存差；Rust 的 `func_value_test.rs` 聚焦负载差和生命周期，`window_func_test.rs::test_window_functions` 覆盖窗口结果行为。Rust 测试没有验证生产执行器从 `AggImplementation` 实例化本状态机或最终写入 chunk。

## 扩展指南

- 新增一种值类型时，应先在 `ValueMemory` 中明确其额外内存口径，再添加相应 `Value4*` 别名；若构建器可选择该类型，还要同步 `builder.rs::ValueKind`、`value_kind` 与生产实例化接线。拥有缓冲的类型必须确保保存的是独立拥有或正确 Clone 的数据。
- 修改 NULL 或空窗口语义时，要同时检查 `ValueEvaluator::result`、三个 `update/result/reset` 组合，并在独立的 `func_value_test.rs` 增加覆盖；不要把测试嵌入生产源文件。
- 修改批次定位时，重点保护 `NthValue::update` 的区间条件和 `nth - seen_rows - 1` 换算，并覆盖目标位于首批、后续批、恰好边界、越界及 N=0 的用例。
- 修改内存记账时，应验证从大值到小值、值到 NULL、reset 后复用以及 JSON/向量深拥有；同时与 Go `func_value.go` 和 `func_value_test.go` 的口径对照，避免把容量、长度和结构体大小混在同一增量中。
- 若要完成生产接线，最可能的入口是消费 `builder.rs::AggImplementation` 的执行器工厂。接线必须补充类型分派、表达式求值错误传播、结果 chunk 写入和 partial-result 初始大小记账，并新增独立集成测试；仅让本文件单元测试通过不足以证明窗口 SQL 路径可用。
- 性能风险主要来自逐行/逐批 Clone 大字符串、JSON 或向量，以及错误的内存差导致内存控制失真；兼容风险主要是改变 NULL presence、Float32 窄化或 NTH_VALUE 的 1-based 规则。

## 验证依据

- RustCodeGraph：`status` 确认索引包含本仓库；`files --filter pkg/executor/aggfuncs/func_value.rs` 定位目标文件；`node --file ... --offset 1 --limit 500` 读取完整 254 行与 28 个符号；`query` 定位 `ValueEvaluator`、`FirstValue`、`LastValue`、`NthValue`、`evaluate_float32`；精确 `callers/callees` 未返回这些泛型局部边，已作为图谱限制处理。
- Rust 源码：`pkg/executor/aggfuncs/func_value.rs`；模块入口 `pkg/executor/aggfuncs/lib.rs`；构建元数据接线 `pkg/executor/aggfuncs/builder.rs::build_window_function`。
- crate 边界：`pkg/executor/aggfuncs/Cargo.toml`，package 为 `astersql-executor-aggfuncs`，library 入口为 `lib.rs`。
- Rust 测试：`pkg/executor/aggfuncs/func_value_test.rs` 与 `pkg/executor/aggfuncs/window_func_test.rs`。
- Go 对照：`pkg/executor/aggfuncs/func_value.go`；Go 内存测试 `pkg/executor/aggfuncs/func_value_test.go::TestMemValue`。
- 仓库搜索：除本文件和两个直接测试外，`FirstValue/LastValue/NthValue/ValueEvaluator/Value4*/evaluate_float32` 的 Rust 命中只有 `builder.rs` 的同名枚举与元数据构建，未找到生产状态机实例化点。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前使用任务指定命令检查文档存在且固定二级标题恰好为 11 个，并人工复核源码链接、当前接线限制与扩展风险。

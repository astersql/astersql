# `pkg/executor/aggfuncs/spill_serialize_helper.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate，模块由 `pkg/executor/aggfuncs/lib.rs` 声明，其 `SerializeHelper` 被 crate 根重导出。它位于哈希聚合的内存 partial result 与 spill chunk 中字节列之间：向下调用 `astersql-util-serialization` 中与 Go 兼容的基础编码器，向上由 `aggfuncs.rs` 的 `Serializer`/`StateSerializer<T>` 和 `pkg/executor/aggregate/agg_hash_partial_worker.rs` 的落盘流程使用。

`pkg/executor/aggfuncs/Cargo.toml` 将 crate 入口定为 `lib.rs`，并以普通 workspace 路径依赖引入 `astersql-util-serialization`；本文件没有条件编译项。它只描述 partial data，聚合函数配置仍由 `StateSerializer<T>::template` 和构建器保留。

## 核心职责

- 维护一个可复用的 `Vec<u8>`，在每个 partial result 开始时清空长度但保留容量，减少连续落盘时的分配。
- 实现 COUNT、MAX/MIN、AVG、SUM、GROUP_CONCAT、位聚合、JSON 聚合和 FIRST_ROW 的旧式类型化序列化入口，保持 Go 同路径编码字段顺序。
- 实现通用 `SpillState` 协议所需的元素编码、集合编码、状态恢复和恢复后堆内存计费，覆盖 DISTINCT、APPROX_COUNT_DISTINCT、方差、PERCENTILE、FIRST_ROW、GROUP_CONCAT 及原生 SQL 值。
- 处理 Go 协议中的特殊表示：浮点 DISTINCT 的位模式、decimal 精度、Time/Duration 内部位、VectorFloat32 零拷贝格式，以及不支持的 PERCENTILE 空状态。

## 主要符号

- `pub struct SerializeHelper { buffer: Vec<u8> }`：单个可变帮助器；`new`/`Default` 创建容量 64 的 buffer，`reset` 仅清空长度，`put_bool` 追加状态位。
- `serialize_count`、`serialize_max_min_*`、`serialize_avg_*`、`serialize_sum_*`、`serialize_group_concat`、`serialize_bit_func`、`serialize_json_*`、`serialize_first_row_*`：旧式公开 API；返回值是借用 `self.buffer` 的切片，下一次调用会覆盖其内容。
- `serialize_spill_value`：把 `SpillValue` 的 Bool/Int64/Uint64/Float64/String/BinaryJson/Opaque/Time/Duration 变体分派给 `SerializeInterface`，用于 JSON 聚合容器。
- `serialize_count_extrema<T: CountValue>`：依次写 `is_null`、`count` 和由 `CountValue::write` 编码的值。
- `serialize_state<T: SpillState>`：通用入口，重置 buffer 后委托 `T::write_spill`；它是 `StateSerializer<T>` 的实际字节生成器。
- `pub trait SpillElement`：集合元素编解码约束，要求 `Clone + Send + 'static`，并可通过 `heap_bytes` 报告变长负载。`scalar_element!` 为 `i64`/`f64` 生成直接实现。
- `write_elements`/`read_elements`：通用集合协议，先写/读有符号元素数，再逐项处理；读取端拒绝负计数。
- `SpillState` 实现组：为 `CountDistinct<T>`、`CountDistinctMulti`、`ApproxCountDistinct`、distinct SUM/AVG、整数 distinct SUM、`VarianceState`、`DistinctVariance`、`Percentile<T>`、`FirstRow<T>`、`GroupConcat`、`Vec<T>` 以及原生向量 partial result 定义完整往返协议。
- `pub struct NullPercentile`：不支持的 PERCENTILE 输入的无数据占位状态；`has_spill_state` 返回 `false`，因此上层写 NULL 而非字节。
- `clone_native_vector`：用向量自身的零拷贝序列化/反序列化格式制造拥有式副本，避免 partial state 共享底层字节。

## 执行流程

1. `PartialResultSpill` 在 `agg_hash_partial_worker.rs` 中持有一个 `SerializeHelper`，遍历分区键和聚合 partial result 时，对每个聚合函数调用 `Serializer::serialize_partial_result`。
2. 通用路径的 `StateSerializer<T>` 将类型擦除的 `PartialResult` 向下转型为 `T`。若 `has_spill_state()` 为真，它调用 `SerializeHelper::serialize_state`并把结果追加到对应 chunk 列；否则写 NULL。
3. `serialize_state` 清空旧长度，把 buffer 所有权临时交给 `T::write_spill`。集合类通常先写元素数，再由 `SpillElement::write_element` 连续写入；固定状态则按 Go 字段顺序写入。
4. chunk 达到行数/已用字节阈值后，partial worker 将它刷到 `DataInDiskByChunks`。因为列追加会拷贝字节，帮助器可在下一个状态立即复用 buffer。
5. 恢复时 `StateSerializer<T>::deserialize_partial_result` 从 template 复制一个状态，`DeserializeHelper::deserialize_state` 向 `T::read_spill` 传入 `PosAndBuf`。实现覆盖容器，重建去重结构，并返回恢复的可变堆内存计费；上层再加 `fixed_spill_memory()`。

旧式 `serialize_*` 方法也遵循“reset→按固定顺序写字段→返回 buffer 切片”；对应读端在 `spill_deserialize_helper.rs`。

## 数据与状态

- `SerializeHelper::buffer` 是唯一内部可变状态。`std::mem::take` 避免追加时复制；编码函数返回同一所有权链上的 `Vec<u8>`。长值扩容后的 capacity 会留作后续复用。
- 多字段状态必须保持顺序：MAX/MIN 为 `is_null, value`；AVG 为 `sum, count`；SUM 为 `value, not_null_row_count`；FIRST_ROW 为 `is_null, got_first_row, value`；方差为 `count, sum, variance`。
- 集合以 Go `SerializeInt` 格式的长度为前缀。`Vec<u8>` 和 `String` 的 `heap_bytes` 返回负载长度；普通标量默认为 0，固定内存由上层的 `fixed_spill_memory` 补充。
- `u64` 的 `SpillElement` 不是一般无符号整数协议；它特指 COUNT DISTINCT REAL 保存的 `f64::to_bits()`，写入时重构浮点值，读取时再还原位模式。
- `ApproxCountDistinct` 的格式自包含，不再写外层数量；它会消费剩余全部 buffer。`NullPercentile` 则没有实体字节。
- FIRST_ROW 需区分三态：尚未见到行、第一行为 NULL、第一行为非 NULL。向量变体仅在第三态写入向量体，以避免空值与空向量混淆。

## 依赖与调用关系

- 直接依赖：`use crate::aggfuncs::*` 引入 partial result 类型和 `SpillState`；`astersql_util_serialization` 提供 `Serialize*`/`Deserialize*`、`PosAndBuf` 和 SQL 值类型。Cargo manifest 明确声明了后者。
- 上游调用：RustCodeGraph 显示该文件被 `aggfuncs.rs` 和 `aggregate/agg_hash_partial_worker.rs` 使用。前者的 `StateSerializer<T>::serialize_partial_result` 调用 `serialize_state`；后者的 `PartialResultSpill::spill_maps` 为每个 partial result 调用 serializer，并将完整 chunk 刷盘。
- 下游调用：所有字节操作最终落到 serialization crate。反向恢复路径为 `spill_deserialize_helper.rs::deserialize_state`→`SpillState::read_spill`。
- 状态注册：`BuiltAggFunc::spill_function` 为构建器选择的聚合实现创建 `StateSerializer<T>` 和同类型 template；因此本文件中新增 `SpillState` 实现本身不会自动使类型可落盘，还需在该注册表接线。
- 测试入口：`spill_helper_test.rs` 是主要独立 Rust 回归文件，`func_max_min_count_test.rs` 额外验证 count-extrema 的 spill 和内存计费；Go 对照测试为 `spill_helper_test.go`。

## 错误处理与边界

本文件的公开序列化方法不返回 `Result`；它们假定输入是已构造的合法 partial state。格式或类型不变式被破坏时采用 panic，不尝试在这一层恢复：

- `read_elements` 对负的集合长度 `assert!`；`Vec<u8>` 和向量读取根据声明长度切片，截断字节会越界 panic。
- decimal 内部转换使用 `expect`，依赖系数、scale 和 Go `MyDecimal` 的可表示范围；原生向量零拷贝解码也在格式错误时 panic。
- `ApproxCountDistinct::read_and_merge` 的错误被转为 panic。partial worker 的上层路径使用 `catch_unwind`做清理边界，但本帮助器本身不包含 I/O 错误处理。
- 序列化值借用帮助器内存；调用者不能在再次使用同一 helper 后继续依赖旧切片。chunk 的 `AppendBytes` 是实际持久化边界。
- 集合长度和长度前缀在从有符号值转 `usize` 前只有 `read_elements` 显式检查；例如 `Vec<u8>::read_element` 信任已验证的内部 spill 数据，不是面向不可信网络输入的解码器。

## 并发与资源生命周期

`SerializeHelper` 的方法要求 `&mut self`，因此同一实例不能被两个并发编码操作同时使用。它不含锁、通道、后台任务或 I/O 句柄；并发隔离由上层 worker 的每 worker/helper 所有权提供。`SpillElement` 和 `SpillState` 的 `Send + 'static` 约束使 partial state 能跨 worker 转移，但不意味着 helper 是共享式同步对象。

buffer 在 helper 存活期内保留 capacity，helper 被 drop 时一次性释放。`SpillState::read_spill` 通常先清空/替换目标容器，再重建拥有式数据；`spill_helper_test.rs::round_trip_state` 在恢复后重置源 chunk，专门验证字符串、向量和 decimal 不借用 chunk 存储。`fixed_spill_memory` 与 `read_spill` 的返回值共同支撑上层内存 tracker；新容器必须避免重复计费固定头和动态负载。

## 与 Go 版本的对应关系

Go 直接对照文件是 `pkg/executor/aggfuncs/spill_serialize_helper.go`。两者都以 64 字节初始 buffer，在每次调用前复用容量，并通过同名 serialization primitives 保持字段布局。旧式 Rust `serialize_*` 与 Go `serializePartialResult4*` 一一对应，包括 nil/空 GROUP_CONCAT buffer 的 bool 标志、JSON 元素逐项编码、FIRST_ROW 的双状态位，以及向量只在真实非 NULL 值时写负载。

Rust 版在同一文件中又引入 `SpillElement`/`SpillState` 泛型抽象，用 trait 实现取代 Go 中多个专用 set/slice 循环，但线上格式仍对齐 Go：集合先写数量，distinct decimal 写 key/value 对，方差写三个标量，APPROX_COUNT_DISTINCT 直接使用自带格式。`Percentile<T>::read_spill` 对 Time/Duration 特意按每项 8 字节计费，是源码注释明示的 Go 兼容行为，而不是 Rust 类型的实际 `size_of`。

迭代顺序不是语义保证：Go map 和 Rust HashMap/HashSet 都可以以不确定顺序写入 DISTINCT 元素；正确性依赖恢复后的集合语义，不依赖字节级稳定排序。对照测试为 Go `spill_helper_test.go` 与 Rust `spill_helper_test.rs`；后者还验证通用 `StateSerializer<T>` 新路径。

## 扩展指南

1. 新增固定字段 partial result 时，优先为状态实现 `SpillState`，明确与 Go 完全一致的字段顺序，并在 `BuiltAggFunc::spill_function` 注册对应 template。若该类型仍使用旧 `Serializer` 实现，则还需在 `SerializeHelper` 与 `DeserializeHelper` 各加成对方法。
2. 新增可复用的集合元素时，实现 `SpillElement::{write_element, read_element}`；只有变长或间接分配数据才在 `heap_bytes` 返回额外负载。确保读写顺序对称，且恢复值拥有自己的存储。
3. 新增容器时，确定 `fixed_spill_memory` 和 `read_spill` 各自计费的边界，特别是 capacity、元素头和变长负载。如果 Go tracker 使用与 Rust `size_of` 不同的常量，应显式保留 Go 语义并加注释。
4. 特殊空值/三态值不应依赖默认负载来隐式区分。仿照 FIRST_ROW VectorFloat32 显式写标志位，并只在有真实负载时写值。
5. 同步更新独立测试 `pkg/executor/aggfuncs/spill_helper_test.rs`：至少覆盖空状态、边界值、大负载导致的 buffer 扩容、经 chunk 的真实往返、源 chunk 重置后的所有权和内存计费。若修改 Go 兼容格式，还必须同步 `spill_helper_test.go` 中的对照情形。

主要兼容风险是字段顺序、长度前缀、nil/空差异或特殊类型格式变化导致旧 spill 数据无法恢复；正确性风险是重建去重容器时改变 NaN/位模式或键语义；性能风险是不必要的中间拷贝、过度保留大 buffer capacity 以及内存计费不准。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；本次查询时索引可用。
- RustCodeGraph `node --file pkg/executor/aggfuncs/spill_serialize_helper.rs --offset 1/521`：读取完整 935 行源码，并确认图中直接使用文件为 `aggfuncs.rs` 和 `aggregate/agg_hash_partial_worker.rs`。
- RustCodeGraph `query SerializeHelper --kind struct` 与 `query SpillState --kind trait`：核对 Rust/Go 对照符号以及 trait 在 `aggfuncs.rs` 的定义位置。宽泛 `explore` 输出同时显示 `spill_maps`、`serialize_state`、`write_spill`/`read_spill` 相关边；结论又由下列源码局部核验。
- 已读边界/接线文件：`pkg/executor/aggfuncs/Cargo.toml`、`lib.rs`、`aggfuncs.rs`、`spill_deserialize_helper.rs`、`pkg/executor/aggregate/agg_hash_partial_worker.rs`。
- 已读 Go 对照：`pkg/executor/aggfuncs/spill_serialize_helper.go`；已检索 Go 回归 `spill_helper_test.go`。
- 已读/检索独立 Rust 测试：`spill_helper_test.rs`、`spill_deserialize_helper_test.rs`、`func_max_min_count_test.rs`。关键用例包括 `spill_count_round_trips_through_chunk_storage`、`spill_long_string_round_trip_preserves_null_and_payload`、`spill_distinct_count_preserves_int_real_decimal_duration_string_and_multi_keys`、`spill_approximate_count_and_variance_preserve_full_partial_state`、`spill_percentile_preserves_all_five_typed_samples_and_memory_charges`、`spill_first_vector_preserves_unseen_null_empty_and_nonempty_flags` 和 `spill_native_sql_values_keep_full_decimal_precision_time_bits_and_vectors`。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验命令和结果在任务交付时记录。

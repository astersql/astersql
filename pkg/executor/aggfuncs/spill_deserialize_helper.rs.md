# `pkg/executor/aggfuncs/spill_deserialize_helper.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate。`pkg/executor/aggfuncs/Cargo.toml` 以 `lib.rs` 为库入口，并直接依赖 `astersql-util-serialization`；`lib.rs` 将本文件声明为 `spill_deserialize_helper` 模块，并把 `DeserializeHelper` 重新导出到 crate 根。它位于聚合执行器的 spill 恢复边界：接收已经从落盘数据装入 `chunk::Column` 的逐行字节串，把每一行还原成某一种聚合函数的 partial result（聚合中间状态）。与它成对的编码端是 [`spill_serialize_helper.rs`](spill_serialize_helper.rs)。

真实接线入口在 [`aggfuncs.rs`](aggfuncs.rs) 的 `deserialize_partial_result_common` 和 `StateSerializer::deserialize_partial_result`：前者用目标列及 `Chunk::NumRows()` 创建本辅助器，逐行收集 `PartialResult` 并汇总内存增量；后者通过通用 `SpillState` 路径恢复复杂状态。计数型极值聚合另由 [`func_max_min_count.rs`](func_max_min_count.rs) 调用 `deserialize_count_extrema`。因此，本文件是“字节协议到内存中间态”的适配层，不负责触发 spill、文件 I/O、选择聚合实现或合并最终 SQL 结果。

## 核心职责

1. `DeserializeHelper::new` 保存列引用和调用方给出的有效行数，但不立即检查列的实际行数；这一延迟行为由 `spill_deserialize_helper_test.rs::constructor_defers_row_count_validation_like_go` 固定。
2. 私有 `next` 统一管理行游标：到达 `total_row_count` 时返回 `None`；否则通过 `PosAndBuf::Reset` 把当前列行复制到可复用缓冲、从字节偏移 0 调用具体解码闭包，成功返回后才把 `read_row_index` 加一。
3. 为 COUNT、MAX/MIN、AVG、SUM、BIT 聚合、GROUP_CONCAT、JSON 聚合和 FIRST_ROW 提供与各自序列化字段顺序对称的类型化方法。
4. 为新增的通用状态体系提供 `deserialize_state<T: SpillState>`，把具体集合、distinct、percentile、variance 等状态的协议交给 `T::read_spill`；为 MAX/MIN COUNT 家族提供 `deserialize_count_extrema<T: CountValue>`。
5. 在需要的路径返回堆内存增量。目前专用接口中 `deserialize_json_object` 计算键、值及 `HashMap` 容量增长；通用状态接口直接透传 `read_spill` 的内存估算。

## 主要符号

- `DeserializeHelper<'a>`：持有 `&'a Column`、下一待读取的 `read_row_index`、调用方声明的 `total_row_count`，以及跨行复用的 `serialization::PosAndBuf`。列借用保证辅助器不能活得比输入列更久；其字段均为私有，调用方只能按公开方法顺序消费。
- `DeserializeHelper::new(column, row_count) -> Self`：构造顺序读取器。`row_count` 是唯一终止依据，构造阶段不访问 `column`。
- `DeserializeHelper::next<T>(...) -> Option<T>`：所有专用解码方法的共同状态机。它只表达“是否还有逻辑行”，不返回解码错误。
- 标量/固定结构方法：`deserialize_count`、`deserialize_bit_func`，以及 `deserialize_max_min_*`、`deserialize_avg_*`、`deserialize_sum_*`。MAX/MIN 读取 `is_null` 后读值；AVG 读取 `sum, count`；SUM 读取 `value, not_null_row_count`，顺序必须与同名 `serialize_*` 保持一致。
- `deserialize_group_concat`：读取“是否存在 buffer”的布尔标志，可选地读取 `Cursor<Vec<u8>>`，并无条件把 `values_buffer` 重置为空游标。
- `deserialize_json_array`：直到当前行缓冲末尾为止，连续读取带类型标签的 `SpillValue`，然后追加到目标 `entries`，不会先清空已有数组。
- `deserialize_json_object`：先完整解析当前行的 `(String, SpillValue)` 对，再清空目标 map 并重建；返回 `(是否读到行, 内存增量)`。若没有下一行，也会清空目标 map。
- `deserialize_first_row<T>` 与十个 `deserialize_first_row_*` 包装器：共同读取 `is_null`、`got_first_row` 和类型值，覆盖 int、float、decimal、string、time、duration、JSON、enum、set。
- `deserialize_spill_value`：把 `serialization::DeserializedInterface` 的九个允许变体一一映射为 crate 内的 `SpillValue`。
- `value_memory_delta`：按 `SpillValue` 变体计算 JSON object 条目值的记账量，包含 `DEF_INTERFACE_SIZE`；字符串和二进制载荷按实际长度计，固定宽度值使用对应常量。
- `deserialize_count_extrema<T: CountValue>`：按 `is_null, count, value` 恢复 `CountPartial<T>`，值的协议由 `T::read` 决定。
- `deserialize_state<T: SpillState>`：调用 `destination.read_spill` 原地恢复通用状态，并把其返回值解释为堆内存增量。

## 执行流程

上游通常先把 spill 数据装入 `Chunk`。`deserialize_partial_result_common(source, ordinal, closure)` 选择 `source.Column(ordinal)`，以 `source.NumRows()` 构造辅助器，并反复调用聚合实现提供的闭包。闭包创建目标 partial result，再调用本文件某个类型化方法；方法内部进入 `next`，后者检查行数、重置 `PosAndBuf`、依协议顺序读取字段、推进一行。闭包把成功恢复的状态包装为 `PartialResult`，公共循环累计状态和内存增量。辅助器耗尽后，类型化方法返回 `false`/`None`，公共循环退出，并断言恢复数量恰等于源 chunk 行数。

固定结构的协议必须严格对称。例如 FIRST_ROW 每行依次消费两个布尔标志和一个类型值；SUM(int64) 依次消费和与非空行计数；这些顺序可由同目录 `spill_serialize_helper.rs` 中的对应 `serialize_*` 反向核验。JSON_ARRAYAGG 和 JSON_OBJECTAGG 没有条目数前缀，而是以当前行字节缓冲的末尾作为终止条件；每个值自身带 interface 类型码。GROUP_CONCAT 以布尔值区分 Go 的 nil buffer 与存在的 bytes buffer。

当前生产接线以两条泛型路径为主：`StateSerializer` 调 `deserialize_state`，MAX/MIN COUNT 实现调 `deserialize_count_extrema`。其余公开专用方法仍构成与 Go helper 对齐的协议 API，并在 `spill_helper_test.rs` 中通过真实 `SerializeHelper -> Column -> DeserializeHelper` 往返覆盖。

## 数据与状态

`read_row_index` 是单调递增的“下一行”下标，初始为 0，只有解码闭包正常返回后才增加；`total_row_count` 构造后不变。`position_and_buffer` 在行之间复用对象，但 `Reset` 会把该行 `Column::GetBytes(idx)` 复制进其 `Buf` 并把 `Pos` 归零，所以恢复出的拥有型字符串、向量或 JSON 不应借用下一次重置会覆盖的缓冲。辅助器自身没有回退、随机访问或重复读取接口。

目标状态的覆盖规则并不完全一致：标量和固定结构覆盖字段；JSON array 使用 `extend`，适合把该行条目追加到调用方准备的状态；JSON object 先 `clear` 后重建；GROUP_CONCAT 把临时 `values_buffer` 清空并替换可选结果 buffer；`deserialize_state` 的覆盖/合并语义由每个 `SpillState::read_spill` 实现决定。扩展时不能假定所有方法都从默认值开始，也不能擅自统一这些行为。

内存返回值为记账信息而非所有权句柄。JSON object 仅在新 key 插入时累计 key 长度、值内存和 map 容量增长；重复 key 覆盖不增加记账，与 Go 的 `SetExt` 插入标志语义对应。通用状态则由状态实现报告 heap 字节，公共调用方再加 `fixed_spill_memory()`。固定标量方法返回布尔值，不单独报告堆内存。

## 依赖与调用关系

上游关系：

- `lib.rs` 公开再导出 `DeserializeHelper`，并在测试配置中挂载两个独立测试模块。
- `aggfuncs.rs::deserialize_partial_result_common` 构造辅助器并提供逐行循环、结果数量断言及内存求和。
- `aggfuncs.rs::StateSerializer::deserialize_partial_result` 调用 `deserialize_state`，用于实现了 `SpillState` 的复杂聚合状态。
- `func_max_min_count.rs` 的反序列化实现调用 `deserialize_count_extrema`。
- `spill_helper_test.rs` 直接调用各专用方法，验证与序列化端的字节协议及耗尽行为。

下游关系：

- `astersql_util_serialization::chunk::Column` 提供每行字节；`PosAndBuf::Reset` 建立行内游标。
- `astersql_util_serialization::Deserialize*` 系列解析固定宽度值、长度前缀对象和带类型标签 interface。
- `crate::aggfuncs::*` 提供所有 partial-result 类型、`SpillValue`、内存常量和 `SpillState` trait。
- `std::io::Cursor` 表示 GROUP_CONCAT 的字节缓冲；`std::collections::hash_map::Entry` 区分 JSON object 的首次插入和覆盖；`size_of::<(String, SpillValue)>()` 用于估算 map 扩容槽位。

RustCodeGraph 的索引状态显示目标文件已纳入索引，并把 `DeserializeHelper` 定位在本文件第 33 行；其符号轨迹显示由 `aggfuncs.rs` 导入。对方法名的图查询未返回稳定结果，因此方法级调用边以上述 `rg` 精确引用和源码读取为依据。

## 错误处理与边界

正常终止不是 `Result`：读到 `total_row_count` 后，布尔接口返回 `false`，`deserialize_count_extrema` 返回 `None`，`deserialize_state` 返回 `(false, 0)`。除 JSON object 在耗尽时清空目标 map 外，固定结构方法在无下一行时不修改目标。`deserialize_partial_result_common` 将提前终止视为内部协议错误，并以 `assert_eq!` 触发 panic。

本文件信任序列化协议和构造参数，不校验 `row_count <= Column` 实际行数，也不验证固定结构是否恰好消费整行。底层 `deserialization_util.rs` 会在缓冲截断、负长度、长度溢出、非法 bool、未知 interface 类型码或非法向量编码时 panic；这些异常不会在此转成可恢复错误。若 `row_count` 大于可用行数，访问越界同样会在底层暴露。JSON 循环以 `Pos < Buf.len()` 为条件，因此残缺尾部最终也会在具体解码器中 panic，而不会被静默忽略。

二进制标量使用本机字节序，长度前缀中的 Go `int` 对应 Rust `isize`；这意味着 spill 字节格式的跨架构持久兼容性不能从本文件推断。该层面向同一实现生命周期内的内部 spill 协议，而不是有版本号的外部存储格式。

## 并发与资源生命周期

`DeserializeHelper` 是有状态的顺序读取器，公开解码方法均要求 `&mut self`；同一个实例不能被多个线程同时消费，也没有锁、原子量、任务或通道。`Column` 以不可变引用借入，其生命周期参数确保列在辅助器存活期间有效。不同线程若各自拥有独立辅助器和满足 trait 约束的输入，可由更上层并行，但本文件不建立这种调度保证。

`PosAndBuf` 对象跨行复用，但其 `Buf` 每次 `Reset` 都复制当前行数据；调用闭包只在 `next` 内短暂获得 `&mut PosAndBuf`，不能把游标引用泄漏到下一行。恢复后的目标 partial result 拥有其字符串、容器和字节内容，测试 `round_trip_state` 在恢复后重置源 chunk，再验证结果，证明复杂状态不应依赖源列缓冲的继续存活。资源释放依赖 Rust 所有权和 `Drop`，本文件没有显式文件句柄或清理阶段。

## 与 Go 版本的对应关系

直接对照文件是 [`spill_deserialize_helper.go`](spill_deserialize_helper.go)。Rust 的四个字段与 Go `deserializeHelper` 的 `column/readRowIndex/totalRowCnt/pab` 一一对应；`new` 同样只保存 `rowNum`；每个传统专用方法同样先检查行数、Reset、按固定顺序解码、递增行号并返回成功标志。`aggfuncs.rs::deserialize_partial_result_common` 也保留 Go 版本“循环至 nil、累加内存、最终检查结果数”的意图。

Rust 用私有泛型 `next` 消除了 Go 文件中重复的边界与递增样板；用 `deserialize_first_row<T>` 合并 FIRST_ROW 的公共头；用 `SpillState`/`SpillElement` 承载 Go 中 distinct 集合、percentile、variance、vector 等大量专用反序列化逻辑。因此不能仅凭本文件比 Go 文件短就判断功能缺失，相关具体协议主要位于 `spill_serialize_helper.rs` 的 trait 实现。MAX/MIN COUNT 也被抽象为 `CountValue::read`。

可见差异包括：Rust JSON object 使用标准 `HashMap::entry` 和容量差计算近似 Go `map`/`SetExt` 的内存增量；Rust JSON array 延续 Go 的追加语义；两边 JSON object 都在读取前重置 map；两边均以 panic 处理不可恢复的内部字节协议错误。当前 Rust 传统 FIRST_ROW 专用包装器没有 Go 文件里的 vector 包装器，但 vector 的生产路径通过 `SpillState for FirstRowPartialResult<VectorFloat32>` 实现，并由 `spill_helper_test.rs` 覆盖。Go 对照是语义基线，新增或调整协议时必须同步检查两端字段顺序、nil/空值含义和内存记账。

## 扩展指南

新增固定布局聚合状态时，应同时修改 `SerializeHelper` 和 `DeserializeHelper`，确保字段顺序、条件字段和类型宽度完全对称；在聚合实现的 `deserialize_partial_result` 中通过 `deserialize_partial_result_common` 接线，并把测试放在同目录独立 `*_test.rs` 文件中，不要内嵌到生产源文件。若状态适合通用体系，优先实现 `SpillState`（必要时实现 `SpillElement`），复用 `StateSerializer`，避免继续扩大专用方法矩阵。

新增 `SpillValue` 变体至少需要同步 `serialize_spill_value`、`deserialize_spill_value`、`value_memory_delta`、相关内存常量与 JSON 聚合往返测试；漏掉任何一处会造成编译穷尽性错误或协议/记账不一致。修改 JSON object 时要特别保留重复 key 的覆盖语义和只对首次插入计费的不变量。修改 GROUP_CONCAT 时要区分不存在 buffer 与存在空 buffer，并保持 `values_buffer` 的重建规则。

安全扩展的最小测试应覆盖：多行顺序消费与耗尽后不再修改目标；空行/空集合；长字符串触发缓冲扩容；每种 interface 标签；截断或非法标签的预期 panic（若协议仍定义为不可恢复）；源 chunk 清空后结果仍有效；返回的内存增量。与 Go 新增功能对齐时，还应在 `spill_deserialize_helper.go`、相关 `func_*.go` 和 `spill_helper_test.go` 中核对原始意图，而不能把 Rust 的现状反推为 Go 规范。

## 验证依据

- RustCodeGraph：`status` 报告索引包含 7,032 个 Rust 文件；`files --filter pkg/executor/aggfuncs/spill_deserialize_helper.rs` 命中目标文件；`query DeserializeHelper --kind struct` 和 `node DeserializeHelper` 定位本文件第 33 行并显示 `aggfuncs.rs` 的导入轨迹。`explore`、按文件 `node` 及部分方法级查询没有产生可用输出，故调用边另由源码与 `rg` 复核。
- 源文件：`pkg/executor/aggfuncs/spill_deserialize_helper.rs`（完整 563 行）、配对编码器 `spill_serialize_helper.rs`、公共入口 `aggfuncs.rs`、专用调用方 `func_max_min_count.rs`、模块入口 `lib.rs`。
- crate 边界：`pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"`、`astersql-util-serialization` 直接依赖和本地聚合相关依赖。
- Go 对照：`pkg/executor/aggfuncs/spill_deserialize_helper.go` 与 `aggfuncs.go::deserializePartialResultCommon`；重点核对了构造延迟、逐行游标、固定字段顺序、JSON/GROUP_CONCAT/FIRST_ROW 语义和内存返回值。
- 独立 Rust 测试：`spill_deserialize_helper_test.rs` 固定构造不提前校验；`spill_helper_test.rs` 覆盖序列化/反序列化真实列协议、行耗尽、长字符串、JSON、GROUP_CONCAT、通用 `SpillState`、源 chunk 重置后的所有权以及工厂接线。
- 错误边界：`pkg/util/serialization/deserialization_util.rs` 的 `PosAndBuf::Reset`、`take`、`deserializeBuffer`、`DeserializeBool` 和 `DeserializeInterface` 证明复制/游标推进、截断及非法标签 panic 行为。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务文件指定的命令验证本文恰含 11 个固定二级章节，并人工检查唯一新增生产物、真实符号引用、无运行能力臆测及测试文件分离建议。

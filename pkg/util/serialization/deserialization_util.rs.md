# `pkg/util/serialization/deserialization_util.rs`

## 文件定位

本文件是 `astersql-util-serialization` crate 的读取侧实现，与同目录的 `serialization_util.rs` 成对定义聚合中间状态（partial result）在 spill（内存压力下落盘）后的二进制恢复协议。crate 根 `lib.rs` 将本模块的公开项全部再导出；`Cargo.toml` 表明它直接依赖 `astersql-util-chunk`、`astersql-types-datum` 和 `astersql-types-json-binary`，分别提供列字节、SQL 数据类型与 Binary JSON/Opaque 类型。

应用侧的主要接入点是 `pkg/executor/aggfuncs/spill_deserialize_helper.rs`：`DeserializeHelper::next` 用 `PosAndBuf::Reset` 选中 spill 列的一行，再把同一个游标交给具体 `Deserialize*` 函数。`pkg/executor/aggfuncs/spill_serialize_helper.rs`、`pkg/executor/aggfuncs/func_max_min_count.rs` 与 `pkg/executor/aggregate/agg_spill.rs` 也直接按字段顺序读取聚合状态。因此，本文件不是通用持久化格式框架，而是与写入侧及 Go 实现共同约束的、面向聚合 spill 的本机格式解码器。

## 核心职责

- 用 `PosAndBuf` 保存当前行的自有字节副本和下一次读取的字节偏移，并在每次成功取值后推进 `Pos`。
- 用私有 `take<const N: usize>` 按本机字节序恢复定长标量；用私有 `deserializeBuffer` 恢复“Go `int`/Rust `isize` 长度前缀 + 载荷”的变长字段。
- 按写入侧的固定字段顺序重建 `MyDecimal`、`Time`、`Duration`、`VectorFloat32`、`BinaryJSON`、`Opaque`、`Set` 和 `Enum`。
- 用 `DeserializeInterface` 解释一字节类型码，并返回 Rust 的封闭枚举 `DeserializedInterface`，使调用方可以穷尽匹配 Go `interface{}` 支持的九种具体类型。
- 对截断、负长度、位置/长度溢出、非法布尔、非法 UTF-8、非法向量载荷和未知 interface 类型码采取 panic；API 没有可恢复的 `Result` 通道。

## 主要符号

- `pub type GoTimeDuration = i64`：表达 Go `time.Duration` 的纳秒级有符号 64 位布局。
- `pub struct PosAndBuf { pub Buf: Vec<u8>, pub Pos: i64 }`：所有读取函数共享的有状态游标。字段公开，允许聚合层为自定义复合状态直接读取或推进缓冲。
- `PosAndBuf::Reset(&mut self, col: &chunk::Column, idx: usize)`：把 `col.GetBytes(idx)` 复制到 `Buf` 并把 `Pos` 归零；`DeserializeHelper::next` 每读取一行调用一次。
- `take<const N: usize>(&mut PosAndBuf) -> [u8; N]`：私有定长读取原语，检查非负位置、加法溢出和切片边界，成功后将 `Pos` 设置为区间末端。
- `deserializeBuffer(&mut PosAndBuf) -> &[u8]`：私有变长读取原语，先调用 `DeserializeInt` 读本机宽度长度，再验证长度非负及缓冲范围，返回借用切片并推进游标。
- `DeserializeByte/Bool/Int/Int8/Uint8/Int32/Uint32/Uint64/Int64/Float32/Float64`：公开标量读取函数。整数和浮点使用 `from_ne_bytes`，`DeserializeBool` 只接受 `0` 或 `1`。
- `DeserializeMyDecimal`：依次读取三个 `i8` 元数据字段、一个布尔符号和九个 `i32` word，构造 40 字节逻辑布局的 `types::MyDecimal`。
- `DeserializeTime`、`DeserializeTimeDuration`、`DeserializeTypesDuration`：分别恢复打包的 `CoreTime(u64)`、纳秒 `i64`，以及 `Duration + Go int Fsp`；Fsp 最终需能转换为 `i32`。
- `DeserializeVectorFloat32`：读取长度前缀载荷并复制为独立 `Vec<u8>`，再交给 `types::ZeroCopyDeserializeVectorFloat32`；解析错误转为 panic。
- `DeserializeJSONTypeCode`、`DeserializeBinaryJSON`、`DeserializeOpaque`：读取一字节类型码及独立复制的长度前缀载荷。
- `DeserializeString`、`DeserializeBytesBuffer`：分别把变长载荷复制为 UTF-8 `String` 和位置为 0 的 `Cursor<Vec<u8>>`。
- `DeserializeSet`、`DeserializeEnum`：均按 `u64 Value` 后接长度前缀名称的次序恢复。
- `pub enum DeserializedInterface` 与 `DeserializeInterface`：类型码 `0..=8` 对应 `Bool`、`Int64`、`Uint64`、`Float64`、`String`、`BinaryJSON`、`Opaque`、`Time`、`Duration`；类型码来自 `common_util.rs`。

## 执行流程

1. 聚合恢复层构造 `DeserializeHelper`，其内部持有 `PosAndBuf::default()`；读取某行时，`DeserializeHelper::next` 调用 `Reset`，把该行列数据复制进游标。
2. 对定长字段，公开 `Deserialize*` 函数调用 `take<N>`：将 `Pos` 转成 `usize`，计算 `[start, end)`，检查该范围存在，复制出定长数组，然后推进位置并用本机字节序解释数组。
3. 对字符串、JSON 值、Opaque、向量和 bytes buffer，`deserializeBuffer` 先读取 `DeserializeInt` 长度，再返回对应载荷区间；外层函数通常复制载荷，使返回对象不借用 `PosAndBuf`。
4. 对复合类型，函数严格复现写入侧顺序。例如 `DeserializeMyDecimal` 连续消费 40 字节逻辑字段，`DeserializeBinaryJSON` 先读类型码再读长度前缀 Value，`DeserializeTypesDuration` 先读纳秒值再读 Fsp。
5. 对异构值，`DeserializeInterface` 先消费一字节类型码，再分派到相应函数并包装为 `DeserializedInterface`。`spill_deserialize_helper.rs::deserialize_spill_value` 随后把该枚举转换为执行器的 `SpillValue`。
6. 返回后，调用方继续用同一 `PosAndBuf` 读取该行后续字段；读完整行后，下一次 `DeserializeHelper::next` 会用另一行重新 `Reset`。

## 数据与状态

唯一可变状态是 `PosAndBuf`。`Buf` 拥有当前待解析字节，`Pos` 是从 0 开始的字节偏移；成功读取的核心不变量是 `0 <= Pos <= Buf.len()`，且 `Pos` 恰好增加当前字段在线格式所占字节数。`migration_aster_unit_test.rs::primitive_round_trips_preserve_go_native_bytes_and_position` 和 `structured_values_round_trip_with_go_field_order` 都验证最终位置等于缓冲长度。

格式使用本机字节序和本机指针宽度的 `isize` 作为 Go `int` 对照，因此它依赖写入端与读取端具有相同的端序和位宽，不是跨架构、跨语言网络协议。类型码和定长宽度定义在 `common_util.rs`；写入顺序定义在 `serialization_util.rs`，任何一侧单独改动都会造成协议漂移。

复合值的拥有关系是显式的：字符串、JSON Value、Opaque Buf、向量载荷和 `Cursor` 都从游标载荷复制后返回；`Set`/`Enum` 的名称由 `DeserializeString` 拥有。`Reset` 同样复制 `chunk::Column` 的行字节，因此后续列存储释放或游标重用不会使已返回对象悬垂，代价是每行与变长值可能发生额外分配和复制。

## 依赖与调用关系

下游依赖如下：

- `crate::chunk::Column::GetBytes`：供 `PosAndBuf::Reset` 定位某一行的序列化字节。
- `crate::types`：提供 `MyDecimal`、`CoreTime`/`Time`、`Duration`、`VectorFloat32`、`BinaryJSON`、`Opaque`、`Set` 和 `Enum`；向量还调用 `ZeroCopyDeserializeVectorFloat32`。
- `crate::common_util::*`：提供 interface 类型码 `BoolType..DurationType`。本文件没有直接使用长度常量，而由定长 Rust 类型推导读取宽度。
- `std::io::Cursor`：承载 Go `bytes.Buffer` 对应的返回值。

上游直接证据来自精确引用搜索：

- `pkg/executor/aggfuncs/spill_deserialize_helper.rs` 使用几乎全部结构化读取函数；其 `DeserializeHelper::next` 建立“每行 Reset、按序读取”的主链，`deserialize_spill_value` 调用 `DeserializeInterface`。
- `pkg/executor/aggfuncs/func_max_min_count.rs` 通过 `CountValue::read` 为各数值、时间、字符串、JSON、向量、Enum 和 Set 类型选择解码器。
- `pkg/executor/aggfuncs/spill_serialize_helper.rs` 的 `SpillElement::read_element` 和多个 `SpillState::read_spill` 用这些原语恢复集合及聚合状态。
- `pkg/executor/aggregate/agg_spill.rs::SpillEntry::read_spill` 使用 `DeserializeInt/Uint64/Bool/Float64` 恢复另一条聚合 spill 状态链。

RustCodeGraph 的文件节点报告本文件被 8 个文件使用，并能解析 `DeserializeInterface` 到各具体解码函数的内部 callee 边；但其符号级外部 callers 查询为空，因此外部调用关系以上述 `rg` 精确引用结果补证，而不把空图结果解释为“无调用者”。

## 错误处理与边界

本模块把坏数据视为内部协议破坏并 panic，而不是返回业务错误：

- `take` 对负 `Pos`、位置加法溢出、截断切片和末端无法表示为 `i64` 分别通过 `expect` 失败。
- `deserializeBuffer` 还拒绝负长度、长度加法溢出和超过 `Buf.len()` 的载荷。长度前缀本身也必须完整。
- `DeserializeBool` 比 Go 的原始内存读取更严格，只接受规范字节 `0`、`1`；其他值 panic。
- `DeserializeTypesDuration` 要求以 Go `int` 编码的 Fsp 可表示为 `i32`。
- `DeserializeString` 要求载荷是合法 UTF-8；Go 的 `string([]byte)` 可容纳任意字节，因此这是 Rust API 的额外约束。
- `DeserializeVectorFloat32` 在载荷解码失败时 panic；此时长度前缀和载荷区间已经从游标消费。`go_merge_32_test.rs` 覆盖非法向量载荷，`go_merge_34_test.rs` 覆盖零值及普通向量往返。
- `DeserializeInterface` 对未知类型码保留 Go 文案 `Invalid data type happens in agg spill deserializing!`。类型码会先被消费，随后才 panic。
- `PosAndBuf::Reset` 将行索引边界行为交给 `Column::GetBytes`；本文件不自行验证 `idx`。

## 并发与资源生命周期

文件内没有锁、原子变量、异步任务、通道或共享全局可变状态。所有读取都要求 `&mut PosAndBuf`，Rust 借用规则阻止同一游标被两个解码过程并发推进；不同线程可以各自使用独立游标，但本模块不提供并发协调。

`PosAndBuf`、返回的 `Vec`/`String`/`Cursor` 和复合类型均由调用方按普通 Rust 所有权管理，没有显式 `close`。`Reset` 会替换并释放旧 `Buf`（若无其他所有者），`Cursor<Vec<u8>>` 从位置 0 开始代表 Go `bytes.Buffer` 的未读内容。向量、JSON、Opaque 等复制载荷的策略隔离了返回值与游标下一次 `Reset` 的生命周期；频繁恢复大变长值时应把复制成本纳入性能评估。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/serialization/deserialization_util.go`，写入协议对照是同目录 Go/Rust 两版 `serialization_util` 和 `common_util`。Rust 版本保留了 Go 的函数族、字段顺序、类型码、native-endian 标量布局、`int` 长度前缀和未知 interface 类型 panic 文案。

实现层面的差异必须视为兼容边界：

- Go 通过 `unsafe.Pointer` 从切片直接解释标量和结构体；Rust 用边界检查后的定长数组与 `from_ne_bytes`，并逐字段恢复 `MyDecimal`、`Time` 和 `Duration`，避免依赖 Rust 结构体内存布局。
- Go `PosAndBuf.Reset` 保存 `Column.GetBytes` 返回的切片视图；Rust 因 `Buf: Vec<u8>` 而复制该行。Go 的变长值函数也按类型选择复制，Rust 对所有返回的变长对象建立独立所有权。
- Go 的 bool 是原始内存读取，Rust 明确限定 `0/1`；Go 字符串允许任意字节，Rust `String` 要求 UTF-8。这两点对由配套写入函数产生的数据等价，但对手工构造或损坏数据的失败行为不同。
- Go 的 `DeserializeInterface` 返回 `any`；Rust 返回 `DeserializedInterface`，调用方可穷尽匹配九种协议类型。
- Go `types.Duration.Fsp` 是 `int`；当前 Rust 类型字段是 `i32`，所以 Rust 读取后执行有界转换。

Go 侧聚合行为回归主要位于 `pkg/executor/aggfuncs/spill_helper_test.go`，覆盖大量 partial result 的序列化/恢复与向量往返；Rust 的直接协议回归独立放在 `pkg/util/serialization/migration_aster_unit_test.rs`、`go_merge_32_test.rs`、`go_merge_34_test.rs`，符合生产源码与测试分离约束。

## 扩展指南

新增标量或复合类型时，应先确定它是否属于既有字段协议还是 `interface` 新变体，然后保持以下同步点：

1. 在 `serialization_util.rs` 与本文件同时增加完全对称的写入/读取顺序；如果 Go 已有对应实现，还要逐字段核对 `serialization_util.go` 和 `deserialization_util.go`，不要依赖 Rust 结构体内存布局。
2. 若扩展 `SerializeInterface`，同时在 `common_util.rs` 分配稳定类型码、在 `DeserializedInterface` 增加变体、在 `DeserializeInterface` 增加分支，并更新 `spill_deserialize_helper.rs::deserialize_spill_value` 及其目标 `SpillValue`。
3. 在独立测试文件中增加：配套写入后的往返、准确的 `Pos` 增量、最短/空值、截断输入、非法标签或非法载荷，以及与 Go 字节布局的固定样例。不要把测试嵌入本生产文件。
4. 变长格式应复用或等价遵循 `deserializeBuffer` 的长度检查；若希望从 panic 改为 `Result`，这会改变全部公开函数和 spill 调用链，必须作为协议/API 迁移整体设计，不能只改一个函数。
5. 修改 `PosAndBuf` 或 native-endian/`isize` 规则前，要评估历史 spill 数据、32/64 位与不同端序环境兼容性。当前格式只适合配套读写端运行于相容本机架构。
6. 对大载荷优化复制时，要同时证明返回值在下一次 `Reset` 后仍有效，并检查 `DeserializeHelper` 的逐行复用模式；不能用借用优化破坏现有所有权边界。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/util/serialization` 列出目标、Go 对照、crate 根和三个独立 Rust 测试；`node --file pkg/util/serialization/deserialization_util.rs --offset 1 --limit 500` 读取了完整 298 行及“used by 8 files”文件关系；`query DeserializeInterface`、`query DeserializeMyDecimal` 消除了 Go/Rust 同名歧义；`callees deserialization_util.rs::DeserializeInterface` 验证其到具体解码器的分派边。符号级外部 callers 未返回边，故使用精确引用搜索补足。
- 源码与配置：`pkg/util/serialization/deserialization_util.rs`、`serialization_util.rs`、`common_util.rs`、`lib.rs`、`Cargo.toml`。
- Go 对照：`pkg/util/serialization/deserialization_util.go`、`serialization_util.go`、`common_util.go`；聚合行为测试 `pkg/executor/aggfuncs/spill_helper_test.go`。
- Rust 上游调用：`pkg/executor/aggfuncs/spill_deserialize_helper.rs`、`spill_serialize_helper.rs`、`func_max_min_count.rs`、`pkg/executor/aggregate/agg_spill.rs`。
- Rust 独立测试：`pkg/util/serialization/migration_aster_unit_test.rs` 验证类型码、标量/结构体往返、位置推进、`Reset`、interface 全分支、未知类型码和截断；`go_merge_32_test.rs` 验证向量精确推进与非法载荷；`go_merge_34_test.rs` 验证零值和普通向量往返。
- 按任务约束未运行 Cargo。本任务的交付验证是固定 11 章节的结构检查，并辅以人工复核：本文说明了文件存在原因、真实执行主链、安全扩展同步点，以及已知兼容和性能边界。

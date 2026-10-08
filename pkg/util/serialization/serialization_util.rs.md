# `pkg/util/serialization/serialization_util.rs`

## 文件定位

本文件是 `astersql-util-serialization` crate 的写入侧实现。crate 根 `pkg/util/serialization/lib.rs` 通过 `mod serialization_util` 引入它，并用 `pub use serialization_util::*` 将所有公开编码函数暴露给聚合执行代码；同 crate 的 `deserialization_util.rs` 是逐字段对称的读取侧。`pkg/util/serialization/Cargo.toml` 表明该 crate 直接依赖 chunk、datum 和 binary JSON 三个内部 crate，没有 feature 分支；本文件本身也没有条件编译项。

它位于聚合中间状态落盘（spill）的基础编码层，而不是通用、跨机器的持久化协议层。RustCodeGraph 的文件关系显示直接使用者包括 `pkg/executor/aggfuncs/spill_serialize_helper.rs`、`pkg/executor/aggfuncs/func_max_min_count.rs` 和 `pkg/executor/aggregate/agg_spill.rs`，另有向量回归测试 `pkg/util/serialization/go_merge_34_test.rs`。

## 核心职责

- 将标量按本机字节序追加到已有 `Vec<u8>`，对应 `SerializeBool`、`SerializeInt*`、`SerializeUint*` 和 `SerializeFloat*`。
- 将字符串、原始字节、向量及结构化 SQL 值编码成读取侧可恢复的字段序列。变长值统一经私有函数 `serializeBuffer` 写入一个 `isize` 长度前缀，再追加载荷。
- 维持 Go 版 spill 格式的字段顺序：例如 `SerializeMyDecimal` 写 4 个元字段后写 9 个 word，`SerializeTypesDuration` 写纳秒时长后写 Fsp。
- 通过 `SerializeInterface` 模拟 Go 的类型 switch：先写 `common_util.rs` 定义的单字节类型码，再写具体载荷；用于 JSON 聚合等异构 spill 值。
- 始终在调用者传入的缓冲末尾追加并返回缓冲，使上层能连续拼接多个状态字段。

## 主要符号

- `pub type GoTimeDuration = i64`：Go `time.Duration` 的 Rust 表示，单位语义为纳秒。
- `fn serializeBuffer(value: &[u8], buf: Vec<u8>) -> Vec<u8>`：唯一的私有函数；把 `value.len()` 转成 `isize`，调用 `SerializeInt` 写长度，然后追加原字节。
- `SerializeByte`、`SerializeBool`、`SerializeInt`、`SerializeInt8`、`SerializeUint8`、`SerializeInt32`、`SerializeUint32`、`SerializeInt64`、`SerializeUint64`、`SerializeFloat32`、`SerializeFloat64`：公开定长原语编码器。整数和浮点使用 `to_ne_bytes`；布尔明确写为 `0` 或 `1`。
- `SerializeMyDecimal`：依次写 `digitsInt`、`digitsFrac`、`resultFrac`、`negative` 和 `wordBuf[9]`。前三项转换为 `u8`，负号写为 0/1，word 按本机字节序写入；调试构建检查 `MyDecimalStructSize == 40`。
- `SerializeTime`、`SerializeGoTimeDuration`、`SerializeTypesDuration`：分别编码打包的 `CoreTime(u64)`、纳秒 `i64`、以及 `Duration + Fsp(isize)`。
- `SerializeVectorFloat32`：写长度前缀和向量的 `ZeroCopySerialize()` 载荷；`SerializedSize() == 0` 时显式构造 `[]` 零向量，保持 Go 的零值线格式。
- `SerializeJSONTypeCode`、`SerializeBinaryJSON`、`SerializeOpaque`：类型码均占一字节；后两者再写长度前缀载荷。
- `SerializeSet`、`SerializeEnum`：先写 `Value(u64)`，再写名称的 UTF-8 字节。
- `SerializeString`：把 UTF-8 字节交给 `serializeBuffer`。
- `SerializeBytesBuffer`：只编码 `Cursor<Vec<u8>>` 当前 position 之后的未读区间，对齐 Go `bytes.Buffer.Bytes()` 的剩余内容语义。
- `SerializeInterface`：接受 `&dyn Any`，依次识别 `bool`、`i64`、`u64`、`f64`、`String`、`BinaryJSON`、`Opaque`、`Time`、`Duration`；类型码为 `BoolType` 到 `DurationType`（0 到 8）。不支持的具体类型会 panic。

## 执行流程

1. 上层聚合代码取得或复用一个 `Vec<u8>`。`spill_serialize_helper.rs` 的 `SerializeHelper` 会先重置内部逻辑长度，再用 `std::mem::take` 把缓冲所有权交给本文件的函数。
2. 定长值直接转成本机字节序字节并追加。复合定长值按 Go 字段顺序连续调用基础编码器，例如 `SerializeTypesDuration` 先写 `Duration`，再把 `Fsp` 转为 `isize` 写入。
3. 变长值进入 `serializeBuffer`：写 `isize` 长度，再追加载荷。字符串、SET/ENUM 名称、JSON/Opaque 内容、向量和 Cursor 剩余字节都遵循该模式。
4. `SerializeInterface` 先通过 `Any::downcast_ref` 确定具体类型，写入相应的 0..8 类型码，再委派给具体编码器；第一个匹配分支立即返回。
5. 返回的 `Vec<u8>` 被调用者继续追加其他字段，或作为聚合 partial result 的 spill 字节交给落盘路径。恢复时 `deserialization_util.rs` 的同名对称函数通过 `PosAndBuf` 按相同顺序推进位置。

典型上游有三类：`SerializeHelper` 编码 MAX/MIN、AVG、SUM、FIRST_ROW、GROUP_CONCAT、JSON 聚合等 partial result；`func_max_min_count.rs` 的 `CountValue::write` 为不同 SQL 类型选择具体编码器；`aggregate/agg_spill.rs` 则用原语编码组、行和聚合状态。

## 数据与状态

本文件没有全局可变状态。每次调用都消费一个 `Vec<u8>` 并返回它；已有前缀保持不变，新数据只追加到末尾。结构化值大多借用输入，`Time`、`Duration` 等小型 Copy 值按值传入；`SerializeOpaque` 按值接收，`SerializeInterface` 为调用它而克隆 `Opaque`。

格式不变量如下：定长标量宽度由 Rust 本机类型决定；变长载荷前缀宽度等于 `size_of::<isize>()`；接口类型码和 JSON/Opaque 类型码各一字节；字段顺序必须与 `deserialization_util.rs` 完全对称。该格式使用 native endian 和 native word size，因此不能推断为跨架构稳定格式。`SerializeVectorFloat32` 还保证 Rust 默认空向量不会产生缺失载荷，而会编码合法的 `[]` 向量线格式。

## 依赖与调用关系

内部依赖：

- `crate::common_util::*` 提供接口类型码与布局常量；本文件实际直接使用 0..8 的接口码。
- `crate::types` 是 `lib.rs` 对 datum 和 binary JSON crate 的再导出，提供 `MyDecimal`、`Time`、`Duration`、`VectorFloat32`、`BinaryJSON`、`Opaque`、`Set` 和 `Enum`。
- 标准库 `std::any::Any` 支撑动态类型分派，`std::io::Cursor` 表示带当前位置的字节缓冲。

上游关系：RustCodeGraph 将本文件标为由 `spill_serialize_helper.rs`、`func_max_min_count.rs`、`agg_spill.rs` 与 `go_merge_34_test.rs` 使用。文本引用进一步确认，`SerializeHelper::serialize_spill_value` 调用 `SerializeInterface`，GROUP_CONCAT 路径调用 `SerializeBytesBuffer`，各 typed partial result 调用相应具体编码器；`CountValue::write` 和 `SpillState::write_spill` 也直接复用这些函数。

下游关系：RustCodeGraph 的 `callees SerializeInterface` 显示 Rust 实现委派给 `SerializeBool`、`SerializeInt64`、`SerializeUint64`、`SerializeFloat64`、`SerializeString`、`SerializeBinaryJSON`、`SerializeOpaque`、`SerializeTime` 和 `SerializeTypesDuration`。所有变长编码最终收敛到 `serializeBuffer`，所有解码契约则由 `deserialization_util.rs` 消费。

## 错误处理与边界

API 不返回 `Result`，因为对合法内存值的普通编码没有可恢复错误。边界失败以 panic 或调试断言体现：

- `serializeBuffer` 在 `usize -> isize` 超界时以 `expect("buffer length fits Go int")` panic。
- `SerializeBytesBuffer` 在 `u64 position -> usize` 超界时 panic；若 Cursor position 大于底层向量长度，切片表达式会 panic。
- `SerializeInterface` 对未列入 Go type switch 的类型 panic，固定文案为 `Agg spill encounters an unexpected interface type!`。
- `SerializeVectorFloat32` 构造 `[]` 零向量时使用 `expect("zero vector is valid")`；这依赖空向量文本始终是合法输入。
- `SerializeMyDecimal` 对前三个带符号字段使用 `as u8`，其位模式与 Go 内存布局一致；反序列化以 `i8` 读取。40 字节总布局只由 `debug_assert_eq!` 在调试构建检查。
- `SerializeTypesDuration` 把 `Fsp: i32` 转为 `isize`；当前支持的平台不会丢失 i32，但读取侧仍负责把结果检查转换回 i32。

格式没有版本号、校验和或边界标签；调用者和读取者必须使用完全相同的字段顺序。持久化跨进程、跨架构或不可信输入不是该写入 API 自身解决的边界。

## 并发与资源生命周期

所有函数均为无共享状态的同步纯追加操作，没有锁、线程、异步任务、通道或事务。并发安全性由所有权模型提供：每次调用独占传入的 `Vec<u8>`，借用的输入只读；不同线程可以独立调用，但同一缓冲必须由调用者串行转移所有权。

资源生命周期集中在缓冲复用。`SerializeHelper` 通过 `std::mem::take` 暂时移出内部缓冲，再接回返回值，避免同时可变借用并保留容量复用机会。追加可能触发 `Vec` 重新分配，时间复杂度与写入字节数成正比。`SerializeBytesBuffer` 不改变 Cursor 的 position，`SerializeInterface` 仅在 Opaque 分支为满足按值 API 产生一次载荷克隆；其余结构化输入通常不额外复制对象，但最终字节都会复制进输出缓冲。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/serialization/serialization_util.go`。Rust 保留了 Go 的公开函数命名、字段顺序、类型码顺序、native-endian/native-word-size 格式和不支持接口类型时的 panic 文案。Go 用 `unsafe.Pointer` 把值的内存表示追加到 `[]byte`；Rust 对标量改用安全的 `to_ne_bytes`，对 `MyDecimal` 显式逐字段编码，从而避免整体结构体转存时把 Rust 布局填充当成协议，但仍产生 Go 期望的 40 字节顺序。

存在几项实现层差异但语义保持一致：Go `int` 对应 Rust `isize`；Go `time.Duration` 对应 `i64`；Go `string` 的 type switch 对应 Rust 拥有所有权的 `String`，不是 `&str`；Go `bytes.Buffer.Bytes()` 返回未读区间，Rust 用 Cursor position 手动切片；Go 使用全局 `types.ZeroVectorFloat32`，Rust在空值分支解析 `[]` 得到等价零向量。Go `SerializeBinaryJSON` 调 `SerializeByte` 写类型码，Rust 调专用的 `SerializeJSONTypeCode`，二者当前均写一个字节。

`migration_aster_unit_test.rs` 对标量、本机宽度、长度前缀、Cursor 剩余区间、结构化字段顺序、九种接口类型及 panic 文案进行 Rust 回归；`go_merge_34_test.rs` 单独验证空/非空 VectorFloat32 的往返。这些测试位于独立文件，符合生产源码不内嵌测试的仓库约束。

## 扩展指南

- 新增定长原语时，应在本文件增加对称的 `SerializeXxx`，在 `deserialization_util.rs` 增加严格对称读取，并视需要在 `common_util.rs` 增加宽度常量；不能只补写入侧。
- 新增变长类型应复用 `serializeBuffer`，明确长度是字节数而非元素数，并为长度转换和空值给出确定语义。
- 扩展 `SerializeInterface` 时，必须同步类型码常量、`DeserializeInterface`/`DeserializedInterface`、Go `SerializeInterface`/`DeserializeInterface` 以及上层 `SpillValue` 映射。类型码是既有线格式的一部分，只能追加稳定编号，不能重排 0..8。
- 修改 `MyDecimal`、`Time`、`Duration`、JSON、SET/ENUM 或向量字段顺序前，必须核对 Go 对照与读取侧；这类变更会破坏已有 spill 字节兼容性。
- 性能改动优先保持调用者的缓冲复用模式；若改变函数接收/返回 `Vec<u8>` 的所有权方式，应同步检查 `SerializeHelper` 中所有 `std::mem::take` 调用和聚合 spill 适配器。
- 测试应继续放在独立文件。基础与结构化格式扩展 `pkg/util/serialization/migration_aster_unit_test.rs`；VectorFloat32 语义扩展 `go_merge_34_test.rs`；涉及具体聚合状态时同步最近的 `pkg/executor/aggfuncs/*_test.rs` 或 `pkg/executor/aggregate/*_test.rs`，并至少覆盖往返、空值/零值、截断或非法标签边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/util/serialization` 确认模块文件集合；`node --file pkg/util/serialization/serialization_util.rs --offset 1 --limit 500` 读取全部 245 行及 4 个使用文件；`query` 核对 `SerializeInterface`、`SerializeMyDecimal`、`SerializeVectorFloat32`、`SerializeBytesBuffer` 和 `serializeBuffer` 的 Go/Rust 对照；`callees SerializeInterface` 核对九个具体编码委派。精确 `callers` 未返回边，因此用文本引用补足上游证据。
- 生产源码与边界：`pkg/util/serialization/serialization_util.rs`、`common_util.rs`、`deserialization_util.rs`、`lib.rs`、`Cargo.toml`。
- Go 对照：`pkg/util/serialization/serialization_util.go`。
- 直接调用者：`pkg/executor/aggfuncs/spill_serialize_helper.rs`、`pkg/executor/aggfuncs/func_max_min_count.rs`、`pkg/executor/aggregate/agg_spill.rs`。
- 独立测试：`pkg/util/serialization/migration_aster_unit_test.rs`、`pkg/util/serialization/go_merge_34_test.rs`。当前目录没有 `doc.go`，所以没有额外包契约文件可读。
- 本任务是纯文档分析，没有修改或运行 Rust/Go 代码，也按计划未运行 Cargo；结构验证单独执行并记录退出状态。

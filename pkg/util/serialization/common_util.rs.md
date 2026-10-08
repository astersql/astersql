# `pkg/util/serialization/common_util.rs`

## 文件定位

`common_util.rs` 属于 `astersql-util-serialization` crate，是序列化与反序列化两侧共享的线格式常量表。模块本身在 `pkg/util/serialization/lib.rs` 中以私有模块 `common_util` 挂载，但其中所有公开项又通过 `pub use common_util::*` 从 crate 根导出，因此调用方使用的是 `astersql_util_serialization::BoolType`、`IntLen` 等路径，而不是直接访问模块。

该文件不执行 I/O，也不自行编解码数据；它定义两类协议事实：`SerializeInterface`/`DeserializeInterface` 使用的 9 个异构值类型码，以及本机布局下各标量和结构的字节宽度。直接运行链位于同 crate 的 `serialization_util.rs` 与 `deserialization_util.rs`，主要业务入口则是 `pkg/executor/aggfuncs` 的聚合中间结果 spill（落盘与恢复）逻辑。

## 核心职责

1. 固定异构 interface 载荷的标签映射：`BoolType` 到 `DurationType` 依次为 `0..=8`。标签值属于持久化字节格式，编码端和解码端必须同步。
2. 用 `size_of::<T>()` 暴露本机 Rust 类型的定长宽度，例如 `IntLen`、`Uint64Len` 和 `TimeLen`，对齐 Go 版本的 `unsafe.Sizeof` 结果。
3. 明确两个单字节协议头：`InterfaceTypeCodeLen` 与 `JSONTypeCodeLen` 均为 `1`。
4. 通过 crate 根统一导出这些常量，让序列化、反序列化以及依赖 crate 共享同一份格式定义。

该文件不是跨机器、跨架构的可移植格式定义：`IntLen`、`UnsafePointerLen` 取决于目标指针宽度，配套读写函数还使用本机字节序。它描述的是与当前 Go spill 格式对齐的进程本机布局。

## 主要符号

- interface 类型码（均为 `pub const ...: i64`）：`BoolType = 0`、`Int64Type = 1`、`Uint64Type = 2`、`FloatType = 3`、`StringType = 4`、`BinaryJSONType = 5`、`OpaqueType = 6`、`TimeType = 7`、`DurationType = 8`。`serialization_util.rs::SerializeInterface` 将它们转换为 `u8` 后写入首字节；`deserialization_util.rs::DeserializeInterface` 读出一个字节并转换为 `i64` 后匹配。
- 协议头宽度：`InterfaceTypeCodeLen = 1`、`JSONTypeCodeLen = 1`。Go 的 `DeserializeInterface` 显式用前者推进游标；当前 Rust 实现通过 `DeserializeByte` 达到同样的一字节推进效果。`JSONTypeCodeLen` 记录 `types::JSONTypeCode` 的线格式宽度。
- 单字节宽度：`BoolLen`、`ByteLen`、`Int8Len`、`Uint8Len`。它们由 `size_of` 计算；迁移测试直接断言 `BoolLen` 和 `ByteLen` 为 `1`。
- 整数与浮点宽度：`IntLen` 对应 Go `int` 与 Rust `isize`，其余为 `Int32Len`、`Uint32Len`、`Int64Len`、`Uint64Len`、`Float32Len`、`Float64Len`。
- 结构与 ABI 宽度：`TimeLen = size_of::<types::Time>()`，`TimeDurationLen = size_of::<i64>()` 对应 Go 纳秒 `time.Duration`，`UnsafePointerLen = size_of::<*const ()>()` 对应 Go `unsafe.Pointer`。

文件没有类型、trait、函数、`impl` 或条件编译项；全部行为都在编译期常量求值阶段完成。

## 执行流程

interface spill 编码流程如下：

1. `pkg/executor/aggfuncs/spill_serialize_helper.rs::serialize_spill_value` 根据 `SpillValue` 变体把具体值交给 `SerializeInterface`。
2. `pkg/util/serialization/serialization_util.rs::SerializeInterface` 对 `dyn Any` 下转型，选择本文件对应的类型码并将其作为一个字节追加到缓冲。
3. 同一分支随后调用具体 `Serialize*` 函数追加载荷；例如 `StringType` 后接本机宽度的长度前缀和 UTF-8 字节，`DurationType` 后接纳秒 `i64` 与 Fsp。
4. 恢复时，`pkg/executor/aggfuncs/spill_deserialize_helper.rs::deserialize_spill_value` 调用 `DeserializeInterface`；后者先读取一字节类型码，再用本文件常量选择对应 `Deserialize*` 函数，最后映射回 `SpillValue`。

固定宽度常量不参与上述所有函数的逐次寻址：Rust 的固定标量读写多由 `to_ne_bytes`、`from_ne_bytes` 和泛型 `take::<N>` 从类型本身推导宽度。它们仍是对外可见的布局契约，并由测试验证；仓库中还存在 join 内存估算的 Go 调用点，而对应 Rust join 文件中的相关引用目前仅出现在注释中。

## 数据与状态

本文件只包含不可变编译期常量，没有堆分配、全局可变状态或运行时缓存。类型码的顺序是最重要的不变量：已有落盘数据中的首字节必须始终解释为同一种载荷；在中间插入或重排常量会让旧数据被错误分派。

宽度常量表达当前编译目标的 ABI。`IntLen` 和 `UnsafePointerLen` 在 32 位与 64 位目标上可能不同；`TimeLen` 依赖 `types::Time` 的实际 Rust 布局。`migration_aster_unit_test.rs::constants_match_go_type_codes_and_native_layout` 当前验证 `TimeLen == 8`、`TimeDurationLen == 8`，并让 `IntLen` 与 `size_of::<isize>()` 保持一致。类型码与宽度均使用 `i64`，与 Go 文件把长度转换为 `int64` 的公开形态相符。

## 依赖与调用关系

- 下游依赖：唯一源码依赖是 crate 根再导出的 `crate::types`，用于计算 `types::Time` 的大小；标准库依赖为 `std::mem::size_of`。
- crate 边界：`pkg/util/serialization/Cargo.toml` 声明 crate 名 `astersql-util-serialization`，并通过本地路径依赖 `astersql-types-datum`、`astersql-types-json-binary` 与 `astersql-util-chunk`；本文件实际只使用前述 `types::Time`。
- 同 crate 消费者：`serialization_util.rs::SerializeInterface` 使用 9 个类型码写标签；`deserialization_util.rs::DeserializeInterface` 使用同一集合匹配标签。`lib.rs` 将常量从 crate 根公开重导出。
- Rust 业务上游：`pkg/executor/aggfuncs/spill_serialize_helper.rs::serialize_spill_value` 与 `pkg/executor/aggfuncs/spill_deserialize_helper.rs::deserialize_spill_value` 将 JSON 聚合等异构 partial result 接到 interface 编解码链。`pkg/executor/aggfuncs/Cargo.toml` 以路径依赖连接本 crate。
- 更广的序列化调用：`pkg/executor/aggregate/agg_spill.rs` 使用同 crate 的标量编解码函数保存聚合状态，但它定义自己的值标签 `0..=5`，没有使用本文件的 interface 类型码，不能把两套标签协议混为一谈。
- 门面导出：`pkg/lib.rs` 的 `util::serialization` 再导出 serialization facade，使上层可经整库门面访问该 crate。

RustCodeGraph 能确认目标文件已索引并读取其源码，但索引把该文件统计为仅 1 个符号，精确查询 `InterfaceTypeCodeLen`、`TimeLen` 没有节点；因此上述常量调用边由精确 `rg` 引用结果和对应源码复核，而不是由缺失的图边推断。

## 错误处理与边界

本文件本身没有返回值、错误类型或 panic 路径。它定义的值会影响相邻实现的失败边界：

- `SerializeInterface` 遇到 9 种支持类型之外的值会 panic，文案为 `Agg spill encounters an unexpected interface type!`。
- `DeserializeInterface` 遇到 `0..=8` 之外的标签会 panic，文案为 `Invalid data type happens in agg spill deserializing!`。
- 缓冲截断、负长度、位置溢出等检查位于 `deserialization_util.rs` 的 `take`/`deserializeBuffer`，不是本文件职责。

类型码必须可表示为一个字节。当前最大值为 8，满足 `SerializeInterface` 的 `as u8` 转换；若将来新增超过 `u8::MAX` 的码，当前转换会截断而不是返回错误。新增类型还必须保证标签与载荷函数一一对应，并保持既有编号不变。

宽度与本机字节序使格式具有架构边界：在不同指针宽度或端序的进程之间交换这类缓冲没有得到当前代码或测试的兼容保证。文档和扩展不应把它描述成稳定的跨平台网络协议。

## 并发与资源生命周期

所有常量均为只读编译期值，可被任意线程并发访问，不涉及锁、原子量、通道、任务或事务。该文件不拥有缓冲和文件句柄，也不控制 spill 文件生命周期。

资源生命周期发生在调用侧：`SerializeHelper` 复用自己的 `Vec<u8>` 容量，`PosAndBuf` 持有恢复缓冲与当前偏移，spill 层负责落盘、读取和清理。本文件只约束这些缓冲中标签与定长字段的解释方式；修改常量会影响所有并发 spill 任务产生或读取的数据，但不会在本文件内引入同步行为。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/serialization/common_util.go`。Rust 保留了 Go 的公开符号命名和常量序列：Go 通过 `iota` 生成 `0..=8`，Rust 显式写出相同数值；Go 用 `unsafe.Sizeof`，Rust用 `size_of::<T>()`。`TimeDurationLen` 在 Go 中取 `time.Duration` 大小，在 Rust 中以其语义等价的纳秒 `i64` 计算。`UnsafePointerLen` 则以 `*const ()` 对齐指针宽度。

存在三点实现差异：

1. Go 的 `JSONTypeCodeLen` 写作 `int64(types.JSONTypeCode(1))`，Rust 直接固定为 `1`；两者表达的线格式宽度相同。
2. Go `DeserializeInterface` 读取标签后用 `InterfaceTypeCodeLen` 推进位置；Rust 调用 `DeserializeByte`，因此同样推进一字节但没有直接引用该常量。
3. Go 的标签由无类型 `iota` 常量产生，Rust 明确使用 `i64` 并在写入时转成 `u8`。

`migration_aster_unit_test.rs` 是当前最直接的 Rust/Go 迁移证据：它校验类型码顺序、本机宽度、全部 9 种 interface 值往返，以及未知标签和不支持值的 panic 文案。Go 目录没有独立的 `*_test.go`；相关真实 Go 消费点位于 `pkg/executor/aggfuncs/spill_serialize_helper.go` 与 `spill_deserialize_helper.go`。

## 扩展指南

若要新增一种 interface 载荷，最小安全改动集合是：

1. 在既有 `DurationType = 8` 之后追加新类型码，禁止重排或复用旧编号。
2. 同步修改 `serialization_util.rs::SerializeInterface`、`deserialization_util.rs::DeserializedInterface` 与 `DeserializeInterface`，并补齐具体载荷的对称编解码函数。
3. 若业务通过 `SpillValue` 使用新类型，同步更新 `serialize_spill_value`、`deserialize_spill_value` 以及内存估算分支。
4. 在独立测试文件 `pkg/util/serialization/migration_aster_unit_test.rs` 扩展类型码数组与 interface 往返用例，并增加非法或边界载荷测试；不要把测试嵌入 `common_util.rs`。
5. 同步更新 Go 对照常量与 type switch，或明确记录 Rust 独有格式为何不会与 Go 数据互换。

修改宽度常量前应先确认对应 `Serialize*`/`Deserialize*` 的实际字段布局，而不是只改常量。尤其应评估旧 spill 数据兼容性、32/64 位差异、端序、`types::Time` 布局和内存记账偏差。若目标是跨进程或跨架构持久化，应另行定义固定端序和固定宽度协议，不能继续依赖 `size_of::<isize>()`。

## 验证依据

- 目标源码：`pkg/util/serialization/common_util.rs`，确认 9 个类型码、16 个宽度/协议常量以及唯一的 `types::Time` 依赖。
- 模块与 crate：`pkg/util/serialization/lib.rs`、`pkg/util/serialization/Cargo.toml`，确认私有模块、crate 根重导出、本地类型依赖和 Go package 元数据。
- 对称实现：`pkg/util/serialization/serialization_util.rs::SerializeInterface`、`pkg/util/serialization/deserialization_util.rs::{DeserializeInterface, DeserializedInterface}`，确认类型码的写入和分派。
- 业务调用：`pkg/executor/aggfuncs/spill_serialize_helper.rs::serialize_spill_value`、`pkg/executor/aggfuncs/spill_deserialize_helper.rs::deserialize_spill_value`，确认聚合 spill 上游入口；`pkg/executor/aggregate/agg_spill.rs` 用于区分另一套局部标签协议。
- Go 对照：`pkg/util/serialization/common_util.go`、`serialization_util.go::SerializeInterface`、`deserialization_util.go::DeserializeInterface`，以及 `pkg/executor/aggfuncs/spill_serialize_helper.go`、`spill_deserialize_helper.go` 的真实消费点。
- 独立测试：`pkg/util/serialization/migration_aster_unit_test.rs` 的 `constants_match_go_type_codes_and_native_layout`、`interface_dispatch_round_trips_every_go_supported_variant`、两个非法 interface panic 测试和截断缓冲测试。`go_merge_32_test.rs`、`go_merge_34_test.rs` 只覆盖向量缓冲，不直接验证本文件常量。
- RustCodeGraph：`status` 显示当前索引含目标目录 10 个文件；`files --filter pkg/util/serialization` 确认 Rust/Go 对照文件；`node --file pkg/util/serialization/common_util.rs` 读取 76 行完整源码。精确常量查询未返回节点，因此调用引用另以 `rg` 核对。
- 结构验证按任务指定命令执行；本任务是纯文档分析，按总计划不运行 Cargo。

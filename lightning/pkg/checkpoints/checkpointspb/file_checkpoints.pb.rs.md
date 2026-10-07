# `lightning/pkg/checkpoints/checkpointspb/file_checkpoints.pb.rs`

## 文件定位

本文件是 Lightning 文件型 checkpoint 的 protobuf 线协议实现，位于独立 crate `astersql-lightning-pkg-checkpoints-checkpointspb` 中。crate 边界由同目录 `Cargo.toml` 的 `[lib] path = "lib.rs"` 确定；`lib.rs` 通过 `#[path = "file_checkpoints.pb.rs"]` 装入本模块并 `pub use file_checkpoints_pb::*`，所以业务层实际从 crate 根使用这里的类型，而不是直接引用本文件模块。

它不是 checkpoint 状态机或持久化后端。上层 `lightning/pkg/checkpoints/checkpoints.rs` 持有 `checkpointspb::CheckpointsModel`，在文件 checkpoint 的加载路径调用 `Unmarshal`，在 `file_cp_save` 及初始化、更新、插入 engine 等保存路径调用 `Marshal`；本文件只负责内存模型与 protobuf 字节之间的转换。源码注释明确说明它是 Go `file_checkpoints.pb.go` 的手写 wire-compatible 镜像，并保留 gogo/protobuf 旧接口。

## 核心职责

1. 定义从任务到 chunk 的五层 checkpoint 数据模型：`CheckpointsModel`、`TaskCheckpointModel`、`TableCheckpointModel`、`EngineCheckpointModel`、`ChunkCheckpointModel`。
2. 为每种消息实现 `Marshal`、`MarshalTo`、`MarshalToSizedBuffer`、`Size`、`Unmarshal`，覆盖 varint、zigzag32、fixed64/sfixed64、length-delimited、packed repeated 及 map-entry 编码。
3. 通过对 map 键排序，使基于 Rust `HashMap` 的输出字节稳定；这也是文件持久化和跨语言比对的关键条件。
4. 复刻 gogo/protobuf 的兼容表面：`Reset`、`String`、`ProtoMessage`、`Descriptor`、`XXX_Unmarshal`、`XXX_Marshal`、`XXX_Merge`、`XXX_Size`、`XXX_DiscardUnknown`，并嵌入 939 字节 gzip descriptor。
5. 在解码时校验字段号、wire type、长度和 varint 溢出，跳过未知字段，但不保存未知字段原始字节。

## 主要符号

- `PROTO_GOGO_PACKAGE_IS_VERSION_3` 与 `FILE_DESCRIPTOR_E56085BB94E0B973`：分别保留 Go 生成物的版本断言形状和压缩 `FileDescriptorProto`。`init()` 在 Rust 中是空操作，因为没有 Go protobuf 的全局类型注册表。
- `Error` / `Result<T>`：模块自有解析错误，包括 `InvalidLength`、`IntOverflow`、`UnexpectedEof`、非法 tag/wire type、字段 wire type 不匹配以及目标缓冲区过小；三个 `ERR_*` 字符串保留 Go 错误文本。
- `CheckpointsModel`：顶层快照，`Checkpoints: HashMap<String, TableCheckpointModel>` 以表名为键，`TaskCheckpoint: Option<TaskCheckpointModel>` 表达 Go 指针的可空性。
- `TaskCheckpointModel`：任务 ID、源目录、backend、Importer/TiDB/PD 地址、排序目录与 Lightning 版本等恢复上下文。
- `TableCheckpointModel`：表 hash、状态、`Engines`、表 ID、KV 统计/校验和、表结构字节及三类 allocator 基值。proto 字段 4 已保留且不可复用。
- `EngineCheckpointModel`：engine 状态与以 `$path:$offset` 为键的 chunk map。
- `ChunkCheckpointModel`：文件路径及逻辑/物理偏移、行号边界、列置换、KV 统计/校验和、时间戳、文件类型、压缩类型、排序键和文件大小。
- `ProtoMerge::merge_from`：实现 proto3 合并规则。非零标量和非空字符串/bytes 覆盖，repeated 追加，map 按键覆盖，已有嵌套任务消息递归合并。
- 编码辅助：`append_varint`、`append_tag`、各类 `append_*_field`、`encode_zigzag32`；长度计算由 `sovFileCheckpoints` 和各类 `size_*` 完成。
- 解码辅助：`Reader`、`begin_field`、`read_string`、`read_bytes_field`、`read_len_delim`、`skip_unknown` 和公开的 `skipFileCheckpoints`。

## 执行流程

写入流程从上层模型的 `Marshal()` 开始：先用 `Size()` 预估容量，再由 `encode_to` 按 proto 字段号写入。普通 proto3 零值被省略；嵌套消息先编码到局部 `Vec<u8>`，然后作为长度定界字段附加；map 被展开为含 key/value 的 entry 子消息。`CheckpointsModel.Checkpoints` 和 `EngineCheckpointModel.Chunks` 按字符串键排序，`TableCheckpointModel.Engines` 按 `i32` 键排序，随后 engine 键经 `encode_zigzag32` 写出。`ChunkCheckpointModel.ColumnPermutation` 使用 packed `int32`，负数按 Go 的符号扩展结果占用 varint 字节。

`MarshalTo` 要求调用方缓冲区至少为 `Size()`，并把编码放在缓冲区开头；`MarshalToSizedBuffer` 则经 `copy_marshal_sized` 把结果放在传入切片末尾，复刻 Go sized-buffer 的位置约定。`XXX_Marshal` 保留 `deterministic` 参数，但 Rust 实现忽略其值；由于所有 map 已显式排序，两条路径仍生成稳定结果。

读取流程从 `Unmarshal(&[u8])` 开始。`Reader` 维护只读切片和游标，`begin_field` 解出 tag 并拒绝非 group 消息中的 end-group 或非正字段号。每个消息的 `match field` 检查预期 wire type并赋值；嵌套消息和 map entry 用局部 `Reader` 递归解析。重复标量以后出现者为准，`ColumnPermutation` 同时接受 unpacked 与 packed 形式并追加，顶层 `TaskCheckpoint` 在已有对象上继续反序列化。未知字段回退到字段起点后交给 `skipFileCheckpoints`，按 wire type 0/1/2/3/4/5 跳过。

## 数据与状态

状态层级为 `CheckpointsModel -> TableCheckpointModel -> EngineCheckpointModel -> ChunkCheckpointModel`，另有可选 `TaskCheckpointModel` 保存任务公共元数据。所有类型均派生 `Clone`、`Debug`、`Default`、`PartialEq`；默认值就是 proto3 零值，`Marshal` 会省略它们。Rust map 的值是直接持有的结构体，Go 对照则是指针值；Rust 顶层仅用 `Option` 保留任务消息的 nil/存在差异。

线协议字段号以 `file_checkpoints.proto` 为准。值得特别保护的不变量包括：表的 `status` 是字段 3、`engines` 是字段 8 且键为 `sint32`；chunk 的列置换是字段 12、时间戳是字段 13 的 `sfixed64`，`sort_key`/`file_size`/`real_pos` 使用 16/17/18 号双字节 tag；fixed64/sfixed64 都采用 8 字节小端布局。`Size()` 必须与 `Marshal()` 的实际长度完全相等，否则上层预分配和 `MarshalTo` 会失效。

反序列化不是“先清空再替换”：空输入保留接收者已有字段，repeated 继续追加，已有 `TaskCheckpoint` 会被增量更新。map entry 内若 value 字段重复，则每次先创建默认 value，最终只保留最后一条消息；`file_checkpoints.pb_test.rs` 专门锁定了这一 gogo 行为。

## 依赖与调用关系

该 crate 的 `Cargo.toml` 未声明第三方运行时依赖；本文件只使用标准库 `HashMap` 和 `fmt`，protobuf 编解码完全手写。`lib.rs` 是唯一模块入口和公开重导出层。

RustCodeGraph 索引显示本文件被 12 个文件使用。业务主链中，`lightning/pkg/checkpoints/checkpoints.rs` 导入该 crate：创建/初始化文件数据库时构造五层模型，`file_cp_save` 调用顶层 `Marshal` 后交给外部存储写入，打开或读取 checkpoint 时调用顶层 `Unmarshal` 恢复模型；初始化、`InsertEngineCheckpoints`、`Update` 等修改最终汇入保存路径。图查询还显示 `Marshal` 被 checkpoint 初始化、engine 插入、更新及 SQL checkpoint 测试调用，`Unmarshal` 被 checkpoint `Get` 和真实 TiKV 导入测试辅助调用。

本文件内部的主要调用边为：`Marshal -> Size + encode_to`，父消息 `encode_to -> 子消息 Marshal`，`MarshalToSizedBuffer -> Marshal + copy_marshal_sized`；`Unmarshal -> begin_field/read_*`，遇到未知字段时 `skip_unknown -> skipFileCheckpoints`。`encodeVarintFileCheckpoints` 和 `sozFileCheckpoints` 都调用 `sovFileCheckpoints`，主要作为 Go 生成辅助接口的兼容面存在。

## 错误处理与边界

所有可能失败的公开编解码入口返回模块 `Result<T>`。目标切片过小时，`MarshalTo`/`copy_marshal_sized` 返回 `Error::Other("buffer too small")`；读取不足返回 `UnexpectedEof`；超过 64 位的 varint 返回 `IntOverflow`；非法字段号、非法 wire type、字段类型不匹配和意外 group 结束都有独立错误变体，`Display` 尽量保持 Go 文案。

未知字段是向前兼容边界：合法未知字段会被跳过，但 `XXX_DiscardUnknown` 是空操作，因为原始未知字段从未缓存。group 由 `skipFileCheckpoints` 用 `depth` 追踪；顶层无匹配 start-group 的 end-group 返回 `UnexpectedEndOfGroup`。

字符串解码使用 `String::from_utf8_lossy`。因此无效 UTF-8 会被替换字符规范化，而 Go `string` 能原样容纳任意字节；这是当前实现可观察的跨语言差异，不能在未补兼容测试前宣称无效 UTF-8 字节级往返。另一个接口差异是 `String()` 使用 Rust `Debug`，并非 Go 的 `proto.CompactTextString`；`deterministic` 参数也只保留形状。扩展时应区分“wire 兼容”与“所有反射/文本 API 完全等价”。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、文件描述符或网络连接。每次编码和解码都由调用者独占 `&self` 或 `&mut self`；并发共享策略由上层 `FileCheckpointsDB` 决定，本模块不提供内部同步。

编码期间会分配顶层 `Vec<u8>`，每个嵌套消息和 map entry 还会产生临时缓冲区；map 排序会分配键数组并执行排序。`MarshalToSizedBuffer` 当前也不是原地倒序编码，而是先完整 `Marshal` 再复制，因此峰值内存包含临时完整消息。解码的 `Reader` 借用输入切片，只在字符串、bytes、map 和嵌套模型落地时取得所有权；函数返回后不保留输入借用。descriptor 是进程静态只读字节，无需释放。

## 与 Go 版本的对应关系

权威 schema 是同目录 `file_checkpoints.proto`，Go 基准是 `file_checkpoints.pb.go`。五个消息、字段号、字段 wire 类型、939 字节 descriptor 及 `XXX_*` 方法均与 Go/gogo 生成物对应。Rust 显式实现 Go 生成器的重要语义：proto3 零值省略、`sint32` map 键 zigzag32、负 `int32` repeated 的符号扩展、fixed64 小端布局、未知字段跳过、空输入保留旧值，以及重复 map value 以最后一个完整消息替换而非递归合并。

差异也必须保留在认知中：Go map value 与任务字段是指针，Rust map value 是值且仅任务字段为 `Option`；Go `init` 注册消息和文件，Rust `init()` 无操作；Go deterministic 路径委托 protobuf runtime，Rust无条件使用排序后的手写编码；Go 可保存任意字节字符串，Rust进行有损 UTF-8 转换；Go 的反射和 compact text 能力并未完整移植。这里的目标是业务所需线协议与兼容方法，而不是提供完整 protobuf runtime。

## 扩展指南

新增或修改字段时，首先修改/核对 `file_checkpoints.proto` 和 Go 生成物，再同步 Rust 结构字段、对应类型的 `encode_to`、`Size`、`Unmarshal` 和 `ProtoMerge`；高于 15 的字段要使用正确的多字节 tag 长度计算。map 还必须同步键编码、稳定排序、entry 长度和重复 value 行为。不得复用 `TableCheckpointModel` 已保留的字段 4。

更改 wire helper 时至少同步独立测试 `file_checkpoints.pb_test.rs` 与 `parity_test.rs`：前者覆盖重复 map value 的替换语义，后者覆盖完整嵌套 round-trip、零值/空输入、负 engine key、packed/fixed 字段、截断和非法 wire type、descriptor、未知字段、`XXX_*` 合并以及两种目标缓冲区放置规则。业务接线变化还应检查 `lightning/pkg/checkpoints/checkpoints_file_test.rs` 和 `checkpoints_test.rs`，但测试逻辑应继续放在独立文件，不能内嵌到本生产文件。

性能风险主要来自 `MarshalToSizedBuffer` 的整消息临时分配、嵌套消息逐级分配和 map 键排序；若优化为原地倒序写入，必须保持稳定字节输出与缓冲区放置契约。兼容性风险集中在字段号/wire type、signed 转换、`Size` 一致性、重复字段合并规则及未知字段行为。若要修复无效 UTF-8 或完整反射兼容，应先增加与 Go 实际字节输出直接比较的测试，不能仅凭 Rust round-trip 判断成功。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter lightning/pkg/checkpoints/checkpointspb` 确认源、Go 对照、入口和两份独立测试均已索引；目标文件节点显示 1648 行、91 个符号并被 12 个文件使用。
- RustCodeGraph：对 `file_checkpoints.pb.rs`、五个模型及 `Marshal`/`Unmarshal` 的 `explore`/`node` 查询确认内部调用链；图中可见 `sovFileCheckpoints` 的 8 个调用点、`skip_unknown -> skipFileCheckpoints`、上层 `checkpoints.rs` 的 `file_cp_save` 保存链，以及 `Get` 的解码链。
- 已读源码与配置：`file_checkpoints.pb.rs`、`Cargo.toml`、`lib.rs`、`file_checkpoints.proto`、Go 对照 `file_checkpoints.pb.go`。
- 已读独立测试：`file_checkpoints.pb_test.rs`、`parity_test.rs`；并通过 RustCodeGraph 检查业务测试与调用入口 `checkpoints.rs`、`checkpoints_file_test.rs`、`checkpoints_test.rs`。
- 按任务约束未运行 Cargo；本任务只新增说明文档。结构验证要求本文件存在且恰有上述 11 个固定二级标题。

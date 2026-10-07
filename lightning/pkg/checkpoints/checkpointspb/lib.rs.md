# `lightning/pkg/checkpoints/checkpointspb/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-lightning-pkg-checkpoints-checkpointspb` 的 crate 根。`lightning/pkg/checkpoints/checkpointspb/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，并用 `package.metadata.porting.go-package = "lightning/pkg/checkpoints/checkpointspb"` 声明对应的 Go 包。

它不是 protobuf 编解码实现本体，而是一个只有 29 行的门面：第 16—20 行通过 `#[path = "file_checkpoints.pb.rs"] mod file_checkpoints_pb;` 纳入实现，再用 `pub use file_checkpoints_pb::*;` 把实现的公共 API 提升到 crate 根。生产调用方因此引用 `checkpointspb::CheckpointsModel`，不需要知道实现文件名。

## 核心职责

1. 将 `file_checkpoints.pb.rs` 固定为本 crate 的唯一生产模块，并隐藏内部模块名 `file_checkpoints_pb`。
2. 全量重导出 protobuf 镜像类型、错误类型和兼容函数，维持与 Go `checkpointspb` 包相近的平坦命名空间。
3. 在 crate 级允许生成式接口需要的命名和未使用项，包括 Go 风格的 `Marshal`、`TaskId` 等；这些 `#![allow(...)]` 只放宽 lint，不改变运行行为。
4. 在 `cfg(test)` 下从独立文件挂载 `parity_test.rs` 与 `file_checkpoints.pb_test.rs`，符合“生产源码和 Rust 测试不放在同一文件”的仓库约束。

## 主要符号

- `file_checkpoints_pb`：私有模块，借助 `#[path]` 映射到 `file_checkpoints.pb.rs`；外部 crate 不能通过该模块名访问实现。
- `pub use file_checkpoints_pb::*`：本文件唯一的公开 API 接线。当前重导出五层模型 `CheckpointsModel`、`TaskCheckpointModel`、`TableCheckpointModel`、`EngineCheckpointModel`、`ChunkCheckpointModel`，以及 `Error`、`Result<T>`、descriptor/错误常量、`init`、varint/skip 辅助函数和各模型的 protobuf 兼容方法。
- `parity_test`：仅测试构建可见，验证 Rust 公共契约与 Go gogo/protobuf 生成物的线协议及遗留接口一致。
- `file_checkpoints_pb_test`：仅测试构建可见，集中验证重复 map value 等解码边界。

本文件没有自定义常量、结构体、trait、函数或 `impl`；上述公共符号的定义都在 `file_checkpoints.pb.rs`。

## 执行流程

编译期流程如下：

1. Cargo 以本文件为库入口。
2. 编译器按 `#[path]` 读取 `file_checkpoints.pb.rs`，在私有模块中编译五类检查点消息及其编解码实现。
3. glob re-export 将该模块所有 `pub` 项暴露到 crate 根。
4. `lightning/pkg/checkpoints/checkpoints.rs` 以 `use astersql_lightning_pkg_checkpoints_checkpointspb as checkpointspb` 引入门面，并构造这些模型。
5. 文件检查点恢复时，`newFileCheckpointsDB` 读取完整快照并调用 `CheckpointsModel::Unmarshal`；保存时，`file_cp_save` 调用 `CheckpointsModel::Marshal`，再交给外部存储写入。
6. 只有执行本 crate 的测试构建时，两个 `cfg(test)` 模块才进入编译和测试发现过程；生产依赖不会携带测试模块。

## 数据与状态

门面自身不保存数据或可变状态。它导出的数据层级来自 `file_checkpoints.pb.rs`：

- `CheckpointsModel` 持有“表名到 `TableCheckpointModel`”的 `HashMap`，以及可选任务元数据。
- `TableCheckpointModel` 持有表状态、engine map、表 ID、KV 统计、表信息和三类自增基值。
- `EngineCheckpointModel` 持有 engine 状态及“chunk key 到 `ChunkCheckpointModel`”的 map。
- `ChunkCheckpointModel` 保存文件路径、逻辑/物理偏移、行号水位、列置换、KV 校验和、时间戳、文件类型和压缩信息。

持久化不变量由实现文件保证：proto3 零值省略；map 编码前按 key 排序以稳定字节输出；未知字段被跳过而不保存；`XXX_Merge` 对标量采用非零覆盖、对 repeated 追加、对 map 按键覆盖、对嵌套消息递归合并。

## 依赖与调用关系

向下依赖只有同目录的 `file_checkpoints.pb.rs`；其实现仅使用标准库的 `HashMap`、格式化与错误 trait，`Cargo.toml` 没有声明第三方依赖。测试依赖也通过本文件的子模块直接共享 `super::*` 公共面。

RustCodeGraph 将 `file_checkpoints.pb.rs` 标为被 12 个文件使用。明确的生产主链是：

`lightning/pkg/checkpoints/checkpoints.rs::FileCheckpointsDB` → crate 根重导出的模型 → `CheckpointsModel::{Unmarshal, Marshal}` → 外部存储的读取/写入。

其中 `FileCheckpointsDB` 用 `TaskCheckpointModel` 初始化任务信息，用 table/engine/chunk 模型把运行时检查点转成整份 protobuf 快照；恢复时再把模型重建为运行时的有序 chunk 列表。`lightning/pkg/checkpoints/Cargo.toml` 以路径依赖显式连接本 crate，说明该门面属于 Lightning 文件检查点后端的协议层，而不是独立的调度或存储层。

## 错误处理与边界

本文件不捕获或转换错误；glob re-export 直接把实现的 `Error` 和 `Result<T>` 暴露给调用方。实现区分长度非法、varint 溢出、提前 EOF、非法 tag/wire type、错误 wire type、非 group 消息遇到 end-group，以及目标缓冲区过小等情况。`checkpoints.rs::file_cp_save` 将编码错误转换为上层 `Error`；`newFileCheckpointsDB` 对解码失败仅调用 `zap::Error` 构造日志字段后继续返回当前 `cpdb`，没有把该错误向调用方传播。扩展或排障时不能假定损坏的检查点文件一定使构造函数失败。

关键边界由独立测试锁定：空消息编码为空；空输入反序列化保留接收者已有字段；负数、零和正数 engine key 经 zigzag round-trip；未知字段被跳过；截断载荷与非法 wire type 返回错误；重复 map entry 中同一 value 字段以最后一条消息为准；`MarshalTo` 与 `MarshalToSizedBuffer` 的缓冲区放置语义不同且均与既定生成接口对齐。

门面的风险边界是 `pub use ...::*`：实现中新增加的任何 `pub` 项都会自动成为 crate 公共 API，而删除或改名会直接影响所有消费者。

## 并发与资源生命周期

本文件及 protobuf 模型不创建线程、任务、锁、通道、事务或 I/O 资源；编码和解码都由调用者同步触发，模型的所有权与缓冲区生命周期遵循普通 Rust 借用规则。

并发一致性属于上游 `FileCheckpointsDB`：它以 `Mutex<()>` 串行化内存模型修改和整份快照保存，并在持锁期间调用本 crate 导出的序列化 API。该锁不是本门面提供的能力，也不能由模型自身防止多个调用者并发覆盖外部文件。descriptor 是静态字节数组，`init()` 在 Rust 中为空操作，不注册 Go 式全局 protobuf registry，因此不存在注册资源的初始化或清理周期。

## 与 Go 版本的对应关系

Go 对照文件是 `lightning/pkg/checkpoints/checkpointspb/file_checkpoints.pb.go`。两侧均公开五个同名模型，并提供 `Reset`、`String`、`ProtoMessage`、`Descriptor`、`XXX_Unmarshal`、`XXX_Marshal`、`XXX_Merge`、`XXX_Size`、`XXX_DiscardUnknown`、`Marshal`、`Size` 与 `Unmarshal` 等生成式接口。Rust 门面模拟的是 Go 包的平坦可见性，而具体兼容实现位于 `file_checkpoints.pb.rs`。

并非所有机制都逐字等同：Go `init()` 会向 gogo/protobuf 全局注册类型，Rust 的 `init()` 是为接口形状保留的空函数；Go 生成物由 protobuf runtime 辅助，Rust 当前实现是手工编解码；Rust 的 `String()` 使用 `Debug`，未知字段被跳过且不缓存。兼容目标是现有字段、wire format、默认值、错误语义和调用接口，不能据此推断拥有完整的 Go protobuf 反射运行时。

## 扩展指南

- 若 `.proto` 增加字段或消息，应先同步 `file_checkpoints.pb.rs` 的模型、字段号、wire type、`Size`、编码、解码、合并和 descriptor，再补充独立测试；通常无需改动本门面，因为 glob re-export 会自动公开新 `pub` 项。
- 若增加新的实现文件，必须明确它是否应成为生产模块及是否应重导出；不要仅把文件放在目录中，因为当前 crate 根只接线 `file_checkpoints.pb.rs`。
- 若改变公共符号可见性或名称，应同时检查 `lightning/pkg/checkpoints/checkpoints.rs`、其同目录 Rust 测试及 Go 对照，特别注意 glob re-export 会扩大兼容面。
- 协议变更至少同步 `parity_test.rs` 和 `file_checkpoints.pb_test.rs`；业务转换或持久化语义变更应同步 `lightning/pkg/checkpoints/checkpoints_test.rs`、`checkpoints_sql_test.rs` 或现有的相邻独立测试，而不是把测试写进 `lib.rs`。
- 性能敏感修改应保持 map 的确定性排序和 `Size`/实际编码长度一致；兼容性敏感修改应覆盖旧 `XXX_*` 入口、未知字段、重复 map value、截断输入和缓冲区边界。

## 验证依据

- 目标入口：`lightning/pkg/checkpoints/checkpointspb/lib.rs`，确认其只有 crate lint、一个 `#[path]` 生产模块、一次公开重导出和两个独立测试模块。
- crate 声明：`lightning/pkg/checkpoints/checkpointspb/Cargo.toml`；直接消费者声明：`lightning/pkg/checkpoints/Cargo.toml`。
- 实现依据：`lightning/pkg/checkpoints/checkpointspb/file_checkpoints.pb.rs` 中五个模型、`Error`、`ProtoMerge`、兼容方法宏、`Marshal`/`Unmarshal`、稳定 map 编码和未知字段跳过逻辑。
- Go 对照：`lightning/pkg/checkpoints/checkpointspb/file_checkpoints.pb.go` 中同名模型及 gogo/protobuf 生成接口。
- 生产调用证据：`lightning/pkg/checkpoints/checkpoints.rs` 的 `FileCheckpointsDB`、`newFileCheckpointsDB`、`file_cp_save`、`Initialize`、`Get`、`InsertEngineCheckpoints` 和 `Update`。
- 测试证据：`lightning/pkg/checkpoints/checkpointspb/parity_test.rs` 与 `file_checkpoints.pb_test.rs`；业务层另有 `lightning/pkg/checkpoints/checkpoints_test.rs` 等独立测试使用重导出模型。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter lightning/pkg/checkpoints/checkpointspb` 找到门面、实现、Go 对照和两个 Rust 测试；`node --file` 确认门面源码及实现/消费者片段；文件使用关系显示实现被 `checkpoints.rs` 和相邻测试等 12 个文件引用。
- 本任务按计划只做文档分析，未运行 Cargo；最终以固定 11 个二级标题的结构检查作为交付验证。

# `pkg/objstore/noop.rs`

## 文件定位

本文件说明的源码是 [`pkg/objstore/noop.rs`](./noop.rs)，它实现 `astersql-objstore` crate 的空操作对象存储后端。模块由 `pkg/objstore/lib.rs` 的 `pub mod noop` 暴露，统一工厂 `pkg/objstore/storage.rs::New` 在收到 `StorageBackend::Noop` 时调用 `newNoopStorage`，再包装成 `Arc<dyn Storage>` 返回。因此它位于对象存储抽象层的后端分发端，而不是一个真正保存对象的介质。

`pkg/objstore/Cargo.toml` 将 crate 根设为 `lib.rs`，且没有控制 noop 的 feature；该实现随 crate 一起编译。它只依赖标准库、`anyhow::Result`，以及同 crate 的 `storage` 模块接口。

## 核心职责

- `NoopStorage` 实现完整的 `Storage` 操作面，使上层可以选择“不做外部存储 I/O”而无需为每个调用点增加分支。
- 所有写入、删除、重命名、遍历和关闭操作都立即成功且不保存状态；整文件读取返回空字节，存在性检查恒为 `false`，URI 恒为 `noop:///`。
- `Open` 和 `Create` 分别返回空读写器，使依赖流式接口的通用代码仍能执行。
- 本实现不是内存存储：写入后不能读回，也没有对象目录。需要可观察存储语义时应使用 `pkg/objstore/memstore.rs` 等真实后端。

## 主要符号

- `pub struct NoopStorage`：零字段、零大小的存储实现；通过 `as_any` 支持 `Storage: Any` 的运行时类型访问。
- `pub fn newNoopStorage() -> NoopStorage`：构造入口。名称保留 Go 风格；`storage.rs::New` 的 noop 分支和独立回归测试直接调用它。
- `impl Storage for NoopStorage`：实现 `DeleteFile`、`DeleteFiles`、`WriteFile`、`ReadFile`、`FileExists`、`Open`、`WalkDir`、`URI`、`Create`、`Rename`、`PresignFile` 和 `Close`。未覆盖的 `CopyFrom` 与 `is_strong_consistent` 使用 `Storage` trait 默认实现。
- `struct NoopReader`：模块私有空读取器，实现标准库 `Read`、`Seek` 和 crate 内 `ObjectReader`。
- `pub struct NoopWriter`：公开空写入器，实现 `ObjectWriter`；公开性与 Go 的 `NoopWriter` 对齐，调用者可单独把它当作丢弃写入目标。

## 执行流程

1. `pkg/objstore/storage.rs::New` 匹配到 `StorageBackend::Noop`，执行 `Arc::new(newNoopStorage())`，向上层返回 `StorageRef`。
2. 一次性操作直接给出固定结果：修改类操作返回 `Ok(())`，`ReadFile` 返回空 `Vec<u8>`，`FileExists` 返回 `false`，`PresignFile` 返回空字符串。
3. `WalkDir` 不调用传入的 callback，表示目录中没有对象；`URI` 返回规范标识 `noop:///`。
4. `Open` 忽略上下文、对象名和 `ReaderOption`，返回新的 `NoopReader`。其 `read` 返回输出缓冲区长度但不改写缓冲区；`ReadDataInRange` 因而能通过 `read_exact` 并保留调用者缓冲区原值。
5. `Create` 忽略上下文、对象名和 `WriterOption`，返回新的 `NoopWriter`。每次 `write` 报告消费了全部输入字节，`close` 成功，但不会提交任何数据。

## 数据与状态

三个类型均无字段，不保存对象、游标、缓冲区、配置或关闭标记。每次 `Open`/`Create` 都只产生新的无状态值；不同实例之间没有共享数据。

`NoopReader::get_file_size` 恒为 `0`。`seek` 对 `SeekFrom::Start(offset)` 原样返回无符号偏移；对 `End`/`Current` 的有符号偏移取 `max(0)` 后转换为 `u64`，且不保存位置。这里的返回值只是兼容接口的应答，不代表后续读取位置。

## 依赖与调用关系

上游直接调用边由 RustCodeGraph 和源码共同确认：

- `pkg/objstore/storage.rs::New -> pkg/objstore/noop.rs::newNoopStorage`：生产构造路径；`StorageBackend::Noop` 是唯一生产分发分支。
- `pkg/objstore/helper_2_aster_unit_test.rs::noop_and_range_read_keep_go_edge_cases -> newNoopStorage`：直接行为测试。
- `pkg/objstore/lib.rs` 公开 `noop` 模块，并在测试夹具中通过 `pub use crate::noop::*` 引入其符号。

下游没有网络、文件系统或异步运行时调用。实现只使用 `std::io::{Read, Seek, SeekFrom}`、`std::time::Duration`、`anyhow::Result`，并履行 `pkg/objstore/storage.rs` 中的 `Storage`、`ObjectReader`、`ObjectWriter` 契约。`pkg/objstore/parse.rs` 负责把 `noop://` 解析为工厂可识别的后端，本文件不参与 URI 解析。

## 错误处理与边界

本文件显式实现的方法都不产生错误，也不检查 `Context` 是否取消；名称、选项、持续时间和数据参数均被忽略。由此带来以下边界：

- `ReadFile` 的“成功且为空”和 `FileExists == false` 是两个独立固定结果，不能据此推断曾写入的数据。
- `NoopReader::read` 返回 `output.len()` 却不填充输出切片。这是为保持 Go `noopReader.Read` 的既有语义而做的非常规行为；调用者必须把缓冲区保持原值视为当前契约，不能把返回长度理解为获得了有效对象内容。
- `WalkDir` 不执行 callback，因此 callback 自身可能返回的错误永远不会出现。
- `PresignFile` 成功返回空字符串，而非可访问 URL。
- `NoopStorage` 没有覆写 `Storage::CopyFrom`，该方法会使用默认实现并返回 `copy is not supported by noop:///`；因此“noop 操作均成功”不包含跨存储复制。`is_strong_consistent` 同样沿用默认值 `false`。
- `SeekFrom::End`/`Current` 的负偏移在 Rust 中被钳制为 `0`，与 Go 读取器直接返回有符号 `offset` 不完全相同；当前范围读辅助函数会在打开读取器前拒绝负的起始偏移。

## 并发与资源生命周期

`Storage` 要求 `Send + Sync`，`ObjectReader`/`ObjectWriter` 要求 `Send`；这些无字段类型天然满足约束。实现没有锁、原子量、通道、后台任务、异步 future 或外部句柄，因此并发调用间不会共享可变状态，也不存在真实资源竞争。

`Open`/`Create` 返回的对象生命周期完全由其 `Box<dyn ...>` 所有权管理。读取器和写入器的 `close`、存储的 `Close` 都是幂等空操作；代码不记录“已关闭”状态，关闭后继续调用仍会按固定规则成功。丢弃对象也无需额外清理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/noop.go`。Rust 保留了 Go 的 `noopStorage`/`noopReader`/`NoopWriter` 三类角色、`newNoopStorage` 构造名，以及各方法的固定返回语义：删除和写入成功、整文件读取为空、存在性为假、遍历为空、URI 为 `noop:///`、预签名结果为空、流式读写报告消费完整缓冲区且关闭成功。

类型系统差异包括：Go 构造器返回 `*noopStorage`，Rust 返回零大小值并由工厂包入 `Arc`；Go `Open` 返回值形式的 `noopReader`，Rust 返回装箱 trait object；Go `Seek` 使用 `int64` 并直接返回传入偏移，Rust 必须满足 `std::io::Seek` 的 `u64` 返回类型，因此负的相对偏移被钳制为零。Rust 的 `Storage` 还提供 `CopyFrom` 和 `is_strong_consistent` 默认方法，本文件没有为 noop 添加 Go 文件中不存在的专用行为。

## 扩展指南

- 若要改变 noop 的固定存储结果，修改 `impl Storage for NoopStorage` 中对应方法，并首先确认仍需与 `pkg/objstore/noop.go` 保持一致；不要把需要保存状态的需求塞入本后端，优先选择或扩展 `memstore.rs`。
- 若要改变范围读语义，需同时审查 `NoopReader::{read, seek}`、`pkg/objstore/storage.rs::ReadDataInRange` 以及 `pkg/objstore/helper_2_aster_unit_test.rs::noop_and_range_read_keep_go_edge_cases`。尤其不能把 `read` 改成返回 `0` 而不调整预期，否则 `read_exact` 会得到 EOF。
- 若增加 `Storage` trait 方法，应明确 noop 是固定成功、固定空值，还是沿用默认的不支持错误，并在独立 `*_test.rs` 文件中增加回归；不要把测试内嵌进 `noop.rs`。
- URI 或工厂接线变化还需同步检查 `storage.rs::New`、`parse.rs`、`parse_test.rs` 及 Go 对照。兼容风险主要是上层依赖“成功但不产生副作用”的控制流；性能风险当前仅为 trait object 和 `Arc` 的固定开销，无 I/O 或随数据量增长的内存占用。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 `pkg/objstore/noop.rs`（147 行、24 个符号）；`node --file` 核对了文件全貌，精确节点 trail 确认 `storage.rs::New` 与 `helper_2_aster_unit_test.rs::noop_and_range_read_keep_go_edge_cases` 调用 `newNoopStorage`。
- 生产源码：`pkg/objstore/noop.rs`；接口与工厂：`pkg/objstore/storage.rs`；模块入口：`pkg/objstore/lib.rs`；crate 声明：`pkg/objstore/Cargo.toml`。
- Go 对照：`pkg/objstore/noop.go` 与 `pkg/objstore/storage.go::{New, ReadDataInRange}`。
- 独立 Rust 测试：`pkg/objstore/helper_2_aster_unit_test.rs::noop_and_range_read_keep_go_edge_cases` 验证 noop 范围读返回缓冲区长度但不改变原内容，并验证负起始偏移报错；`pkg/objstore/parse_test.rs::{test_parse_backend,test_format_backend_url}` 验证 `noop://` 后端解析和规范 URI `noop:///`。同目录不存在专门的 `noop_test.rs` 或 `noop_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以规定命令验证本文恰有十一个固定二级标题。

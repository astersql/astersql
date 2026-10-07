# `pkg/objstore/compressedio/reader.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-objstore-compressedio`，包入口 `pkg/objstore/compressedio/lib.rs` 将 `reader` 模块及其公开项重新导出。它位于对象存储与导入链路的流式解压边界：调用方提供实现了 `Read + Send` 的压缩字节源，本文件依据 `CompressType` 返回对应的解压 `Read`。包的格式枚举和 `DecompressConfig` 定义在 `pkg/objstore/compressedio/def.rs`；直接外部依赖由 `pkg/objstore/compressedio/Cargo.toml` 声明为 `flate2`、`snap` 和 `zstd`。

公开入口只有 `new_reader`。RustCodeGraph 索引显示它被导入文件扫描、会话层 `LOAD DATA`/导入压缩处理、对象存储测试以及 RealTiKV 导入测试使用，例如 `pkg/importsdk/file_scanner.rs::open`、`pkg/session/runtime/load_data.rs::decode_load_data_file`、`pkg/session/runtime/import_compression.rs::execute_import_compression` 和 `tests/realtikvtest/importintotest3/harness.rs::decode_import_file`。文件本身不识别文件后缀、不打开对象存储，也不管理外层文件句柄；压缩类型选择和源 Reader 生命周期由调用方负责。

## 核心职责

- `new_reader` 把统一的 `CompressType` 分派到 Gzip、Snappy 或 Zstd 解码器；`NoCompression` 返回 `Ok(None)`，与 Go `NewReader` 返回 `nil, nil` 的约定对齐。
- Gzip 在构造阶段检查头部，并使用 `MultiGzDecoder` 连续读取拼接的多个 gzip member。
- Snappy 使用 framed stream 解码器 `snap::read::FrameDecoder`，实际格式错误通常在后续 `read` 时暴露。
- Zstd 根据 `DecompressConfig::zstd_decode_concurrency` 选择执行模型：值恰为 `1` 时由调用线程惰性解码；其他值（包括默认值 `0`、负数和大于 `1`）均由一个后台线程解码。
- 异步 Zstd 路径负责把后台线程的解压数据、错误和 EOF 转换成标准 `Read` 语义，同时用有界通道施加背压。

这些职责的边界可由 `reader.rs::new_reader`、`LazyZstdReader::read`、`AsyncZstdReader::read` 和 `decode_zstd` 直接复核。

## 主要符号

- `type SendReader = Box<dyn Read + Send>`：公开函数输入和输出中的动态 Reader 载体。`Send` 是必要约束，因为异步 Zstd 分支会把压缩源移入新线程。
- `LazyZstdReader { source, decoder }`：同步 Zstd 状态机。`source: Option<SendReader>` 在第一次读取时被取走，`decoder: Option<SendReader>` 保存初始化后的 `ZstdDecoder`。
- `LazyZstdReader::new(source)`：只保存源，不解析 Zstd 帧；因此构造成功不代表输入有效。
- `impl Read for LazyZstdReader`：首次 `read` 时构造解码器，此后直接委托。内部的 `BrokenPipe` 分支保护“源已被取走但解码器仍不存在”的失效状态。
- `DecodeMessage::{Data, Error, Eof}`：后台工作线程到消费线程的完整协议。`Data` 拥有一块已解压字节，`Error` 携带原始 `io::Error`，`Eof` 表示正常结束。
- `AsyncZstdReader { receiver, current, finished }`：异步消费端；`current` 保存当前数据块的游标，`finished` 保证 EOF 或错误后不再阻塞等待消息。
- `AsyncZstdReader::new(source)`：建立容量为 `1` 的同步通道并启动一个未命名、未保留 `JoinHandle` 的线程执行 `decode_zstd`。
- `impl Read for AsyncZstdReader`：先耗尽 `current`，再阻塞接收下一条消息；空输出缓冲区立即返回 `0`，不会等待通道。
- `decode_zstd(source, sender)`：工作线程入口。创建 `ZstdDecoder`，用固定的 32 KiB 临时缓冲循环读取并发送拥有所有权的块。
- `pub fn new_reader(compress_type, cfg, reader) -> io::Result<Option<SendReader>>`：唯一公开工厂；外层 `Result` 表示可在构造时发现的错误，`Option` 表示是否需要解压包装。

## 执行流程

1. 调用方先确定 `CompressType`，构造 `Box<dyn Read + Send>`，再调用 `new_reader`。本文件不会自动探测魔数或后缀。
2. `Gzip` 分支构造 `MultiGzDecoder`，立即通过 `header()` 验证首个 gzip 头。无头时返回 `InvalidData("invalid gzip header")`；成功时返回能跨 member 连续读取的解码器。
3. `Snappy` 分支直接返回 `FrameDecoder`。解码发生在调用者后续读取时。
4. `Zstd` 且并发度等于 `1` 时返回 `LazyZstdReader`。第一次 `read` 才把 `source` 移入 `ZstdDecoder::new`，随后同一调用线程持续拉取解压字节。
5. 其他 Zstd 配置返回 `AsyncZstdReader`。构造时立即启动线程；线程可在调用者第一次读取前消费管道式输入。
6. `decode_zstd` 每次最多读取 32 KiB 解压数据，将实际长度复制进新的 `Vec<u8>` 后发送。容量为 `1` 的通道允许至多一条待消费消息，使生产者受消费者速度约束。
7. `AsyncZstdReader::read` 先读取当前 `Cursor<Vec<u8>>`；耗尽后阻塞在 `recv()`。收到数据便替换游标并继续循环，收到错误便返回一次错误并进入完成态，收到 EOF 或通道断开便返回 `0`。
8. `NoCompression` 不返回原始输入，而返回 `Ok(None)`；调用方必须保留或直接使用原始 Reader。`pkg/importsdk/file_scanner.rs::open` 在调用工厂前已单独处理这一分支。

## 数据与状态

本文件不持有全局可变状态。每个 Reader 实例独占其输入源和解码状态。

同步 Zstd 的核心不变量是：正常状态下 `source` 与 `decoder` 只有一个拥有输入；第一次成功或失败地 `take()` 后，源不再可恢复。成功初始化后所有读取都经 `decoder`。异步 Zstd 的核心不变量是：`current` 只表示最近收到且尚未读完的数据块；`finished == true` 后后续非空读取稳定返回 EOF，不再访问通道。

异步路径每个实例创建一个后台线程、一个容量为 `1` 的通道、一个 32 KiB 工作缓冲，并对每个已解压块分配一个恰好为有效长度的 `Vec<u8>`。这限制了排队数据量，但仍有逐块复制和分配成本。`zstd_decode_concurrency` 并不创建指定数量的线程：当前 Rust 实现只区分“恰为 1”与“非 1”，非 1 路径始终只有一个解码线程。

## 依赖与调用关系

上游关系：

- `pkg/objstore/compressedio/lib.rs` 通过 `pub use reader::*` 暴露 `new_reader`。
- `pkg/importsdk/file_scanner.rs::open` 为压缩导入文件包装解码器。
- `pkg/session/runtime/load_data.rs::decode_load_data_file` 用它解码 `.gz` 的 `LOAD DATA` 内容。
- `pkg/session/runtime/import_compression.rs::execute_import_compression` 对导入存储流选择 Gzip/Snappy/Zstd，并以并发度 `1` 读取。
- `tests/realtikvtest/importintotest3/harness.rs::decode_import_file` 用它解码集成测试 fixture。
- RustCodeGraph 还把 `pkg/objstore/objectio/writer_test.rs` 中的对象存储往返测试和本目录独立测试识别为直接调用者。

下游关系：

- `flate2::read::MultiGzDecoder` 实现多 member Gzip。
- `snap::read::FrameDecoder` 实现 Snappy framed 格式。
- `zstd::stream::read::Decoder` 实现 Zstd 流式解码。
- 标准库 `Read` 定义统一拉取接口；`std::thread::spawn` 和 `std::sync::mpsc::sync_channel(1)` 实现异步 Zstd 的生产者/消费者边界。

Cargo 边界由 `pkg/objstore/compressedio/Cargo.toml` 确认；`pkg/objstore/objectio/Cargo.toml` 和 RealTiKV 导入测试的 Cargo manifest 以路径依赖接入该 crate。源码当前没有 feature 条件编译项。

## 错误处理与边界

- Gzip 首头非法时，`new_reader` 立即返回 `io::ErrorKind::InvalidData`；后续 member 损坏或数据截断仍在 `read` 时传播。多 member 行为由 `MultiGzDecoder` 保证。
- Snappy 没有构造期校验；`FrameDecoder::read` 的错误原样返回。
- 同步惰性 Zstd 在首次读取时传播 `ZstdDecoder::new` 的错误，之后传播解码读取错误。`BrokenPipe("zstd source is unavailable")` 是内部状态保护，正常公开调用路径不应触发。
- 异步 Zstd 把解码器构造错误和读取错误包装为 `DecodeMessage::Error`；消费端返回该错误并置 `finished = true`。错误后的后续读取表现为 EOF，不会重复返回同一错误。
- 若工作线程在显式发送消息前异常退出或 sender 被丢弃，`recv` 断开被消费端当作 EOF；线程 panic 不会经 `io::Error` 暴露，也没有 join 结果可检查。
- 若消费端提前丢弃，工作线程下一次发送失败即返回；EOF/错误消息发送失败也被有意忽略，因为已不存在消费者。
- `read(&mut [])` 按 `Read` 约定立即返回 `Ok(0)`。这不是流结束信号，且实现不会因此接收新消息。
- `NoCompression` 的 `None` 是 API 契约而非错误。传入的 Reader 随 `new_reader` 调用被消费并在该分支结束时丢弃，因此需要原流的调用方应像现有调用点一样先绕过工厂。

## 并发与资源生命周期

Gzip、Snappy 和并发度为 `1` 的 Zstd 都在调用 `read` 的线程上执行；它们没有内部任务或通道。惰性 Zstd 把创建解码器推迟到第一次读取，使构造本身不阻塞于管道输入。

非 1 Zstd 在 `AsyncZstdReader::new` 时立即把源移交后台线程。这一点允许无缓冲/零容量管道的生产者在调用者开始读取解压结果之前继续推进；`pkg/objstore/objectio/writer_test.rs::test_new_compress_reader` 专门验证了默认配置的后台消费与并发度 1 的同步消费差异。

通道容量为 `1`，所以后台线程最多领先消费端一个 `DecodeMessage`；当消费者变慢时，工作线程阻塞于 `send`，形成自然背压。Reader 被丢弃时 receiver 关闭，工作线程在当前解码读取或下一次发送之后退出。由于没有保存 `JoinHandle`，析构不会等待线程完成；底层 source 最终由工作线程释放。该实现没有取消令牌，若底层 `Read` 永久阻塞，丢弃消费端不能主动中断该读取。

`AsyncZstdReader` 和 `LazyZstdReader` 都不提供 `Clone`、`Seek` 或显式 `close`。资源释放依赖 Rust drop；需要关闭外层对象存储句柄时，应由持有该句柄的更高层包装处理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/compressedio/reader.go` 和 `def.go`。格式分派一致：Gzip、Snappy、Zstd 返回解码 Reader，默认/无压缩返回 `nil`；Rust 用 `Result<Option<...>>` 表达 Go 的 `(io.Reader, error)` 加 `nil`。

Gzip 语义刻意对齐 Go：Go `gzip.NewReader` 构造期检查头部且默认启用 multistream；Rust 因此选择 `MultiGzDecoder` 并显式检查 `header()`。`reader_test.rs` 的非法头测试和拼接 member 测试记录了这两个兼容要求。

Go 对 Zstd 仅在 `ZStdDecodeConcurrency > 0` 时传入 `WithDecoderConcurrency`；其配置注释规定“不为 1 时异步”。Rust 复刻调用方可观察的同步/异步分界：`== 1` 同步，其他值异步。不过 Rust 非 1 分支只启动一个后台解码线程，并未按正整数值创建对应数量的 Zstd worker，因此吞吐和 CPU 并行度不应被描述为与 Go 实现完全等价。

Go `zstd.NewReader` 的构造/预读细节由上游库决定；Rust 同步分支特意惰性初始化，以避免在构造阶段消费管道。两端已由对象存储管道测试验证关键可观察行为，但所有损坏流的精确报错文本并未证明逐字一致。

## 扩展指南

- 新增压缩格式时，应同时扩展 `pkg/objstore/compressedio/def.rs::CompressType`、本文件 `new_reader` 的分派、对应 writer，并同步 Go 的 `def.go`/`reader.go` 语义；测试应放在独立的 `reader_test.rs` 或其他同目录独立测试文件，不要嵌入生产源码。
- 调整 Gzip 实现时必须保留构造期非法头报错和多 member 读取；同步更新 `invalid_gzip_header_is_rejected_when_reader_is_created` 与 `gzip_reader_consumes_all_members_like_go`。
- 修改 Zstd 配置含义、块大小或通道容量时，重点检查管道死锁、首读延迟、峰值内存、逐块分配以及提前丢弃 Reader 后的线程退出。同步维护 `pkg/objstore/objectio/writer_test.rs::test_new_compress_reader` 和 `migration_aster_unit_test.rs::zstd_sync_decode_configuration_round_trips`。
- 若要真正实现数值型多 worker 并发，不能只增加线程数：需要确认 zstd 帧能否安全分区、输出顺序、错误仲裁和取消方式，并以 Go `WithDecoderConcurrency` 的可观察行为为兼容基线。
- 若要增加显式取消或等待线程结束，应在 `AsyncZstdReader` 中加入生命周期协议，并考虑底层阻塞 `Read` 无法仅靠关闭 receiver 中断的问题。
- 改变公开签名时要审查所有 RustCodeGraph 调用者，尤其是导入 SDK、session runtime 和 RealTiKV harness；`NoCompression` 的 `None` 约定不能静默改成返回原 Reader。

## 验证依据

- 生产源码：`pkg/objstore/compressedio/reader.rs`（`SendReader`、`LazyZstdReader`、`DecodeMessage`、`AsyncZstdReader`、`decode_zstd`、`new_reader`）。
- 模块与类型：`pkg/objstore/compressedio/lib.rs`、`pkg/objstore/compressedio/def.rs`。
- crate 边界：`pkg/objstore/compressedio/Cargo.toml`；工作区成员及 facade 声明见根 `Cargo.toml`。
- Go 对照：`pkg/objstore/compressedio/reader.go`、`pkg/objstore/compressedio/def.go`；对象存储接线参考 `pkg/objstore/compress.go`。
- 独立 Rust 测试：`pkg/objstore/compressedio/reader_test.rs` 覆盖非法 Gzip 头和多 member；`pkg/objstore/compressedio/migration_aster_unit_test.rs` 覆盖三格式往返、同步 Zstd 和无压缩 `None`；`pkg/objstore/objectio/writer_test.rs` 覆盖对象存储往返与 Zstd 管道同步/异步差异。
- 调用点证据：`pkg/importsdk/file_scanner.rs`、`pkg/session/runtime/load_data.rs`、`pkg/session/runtime/import_compression.rs`、`tests/realtikvtest/importintotest3/harness.rs`。
- RustCodeGraph：`status` 显示索引含 `pkg/objstore/compressedio/reader.rs`；`files --filter pkg/objstore/compressedio` 列出源码和独立测试；`explore 'pkg/objstore/compressedio/reader.rs new_reader LazyZstdReader AsyncZstdReader decode_zstd'` 返回完整目标源码、`new_reader` 的 10 个调用者及 `decode_zstd` 的内部调用关系。精确 `callers/callees/node` 命令本次退出成功但未产生文本，因此调用关系又以索引 `explore` 结果和上述调用点源码交叉核验。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令校验文档存在且恰有 11 个固定二级标题，并人工复核未把测试写入生产源码、未把未验证行为写成现状。

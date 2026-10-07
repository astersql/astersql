# [`pkg/objstore/compress.rs`](./compress.rs)

## 文件定位

`compress.rs` 是 `astersql-objstore` crate 的存储装饰层，由 `pkg/objstore/lib.rs` 以 `pub mod compress` 公开。它不实现某一个对象存储后端，而是在任意 `storeapi::Storage` 之上叠加 Gzip、Snappy 或 Zstd 的透明压缩读写。完整文件 API 通过 `WriteFile`/`ReadFile` 在内存中转换，流式 API 则在 `Create`/`Open` 返回的 writer/reader 外加包装。

crate 边界和依赖由 `pkg/objstore/Cargo.toml` 确认：此文件使用内部 `objectio` 与 `storeapi` crate，并直接依赖 `anyhow`、`flate2`、`snap` 和 `zstd`。目标包下没有 `doc.go`；模块入口证据来自 `pkg/objstore/lib.rs`。

## 核心职责

- `WithCompression<S>` 保存底层 `Storage`、`CompressType` 和 `DecompressConfig`，实现一个与后端无关的压缩装饰器。
- `CompressionStorage<S>` 把“原样存储”与“压缩装饰存储”收敛为同一个具体返回类型，使 `with_compression` 能在无压缩时避免多余包装。
- 只有数据通道被改写：`WriteFile`、`ReadFile`、`Create` 和 `Open` 处理压缩/解压；存在性检查、删除、遍历、URI、重命名、预签名和关闭均委派给底层存储。
- `CompressReader` 适配解压流和 `objectio::Reader` 的 `Read + Seek + close + file_size` 合同，明确限制 seek 和文件大小能力。
- `new_limited_intercept_reader` 允许在解压前限制从底层读取的字节数；因而正数 `limit` 针对的是压缩输入流，不是解压后的输出长度。

## 主要符号

- `HARDCODED_CHUNK_SIZE: usize = 5 * 1024 * 1024`：`Create` 路径的固定 5 MiB 分块大小，对应 Go 侧 `s3like.HardcodedChunkSize`。
- `WithCompression<S: storeapi::Storage>`：核心装饰器。`new` 接收所有配置；`inner` 借用底层存储；`into_inner` 消耗装饰器并取回底层存储。
- `CompressionStorage<S>` 与 `with_compression`：`NoCompression` 产生 `Plain(S)`，其他类型产生 `Compressed(WithCompression<S>)`；枚举的 `Storage` 实现仅按变体分派所有方法。
- `impl Storage for WithCompression<S>`：定义整文件与流式读写的真实行为；即使直接用 `WithCompression::new(..., NoCompression, ...)` 构造，其四个数据通道也会短路或返回原 reader/writer。
- `SharedObjectReader(Rc<RefCell<Box<dyn objectio::Reader>>>)`：让解压器持有 `Read` 视图，同时让外层 `CompressReader` 保留对同一底层 reader 的 seek/close 控制。
- `CompressReader { reader, source }`：`Read` 委派给解压流，只接受 `SeekFrom::Current(0)`，`close` 关闭原 reader，`file_size` 恒返回 `Unsupported`。
- `intercept_decompress_reader`：普通解压拦截入口；`NoCompression` 保持 reader 的原有功能与具体类型包装层数。
- `new_limited_intercept_reader`：负数拒绝，0 委派给普通拦截，正数用 `Read::take` 限制底层输入后再按需解压。
- `SharedBuffer(Arc<Mutex<Vec<u8>>>)`：`WriteFile` 整文件压缩的共享内存目标；`bytes` 克隆最终压缩字节。
- `new_decompress_reader`：内部 codec 工厂。Gzip 使用 `MultiGzDecoder` 并立即校验 header，Snappy 使用 framed decoder，Zstd 使用 `zstd::stream::read::Decoder`，无压缩返回 `None`。

## 执行流程

1. 调用者用 `with_compression` 选择透传或装饰存储，也可直接构造 `WithCompression` 以便后续取回 inner storage。
2. `WriteFile` 在开启压缩时创建 `SharedBuffer`，通过 `objectio::compressedio::new_writer` 写入全部明文，显式 `close` 以完成压缩帧，然后把完整压缩结果一次交给底层 `WriteFile`。
3. `ReadFile` 先从底层取回全部压缩字节，用 `new_decompress_reader` 包装 `Cursor<Vec<u8>>`，再 `read_to_end` 产生明文。
4. `Create` 先创建底层 writer；开启压缩时将它交给 `objectio::new_buffered_writer`，该 writer 在 5 MiB 分块上压缩并上传，关闭时刷出尾块并关闭底层 writer。
5. `Open` 先获得底层 `objectio::Reader`，再调用 `intercept_decompress_reader`。解压器从 `SharedObjectReader` 读取，返回的 `CompressReader` 向调用者暴露解压字节，但当前位置查询反映的是底层压缩流的偏移。
6. 限长读取中，`limit < 0` 立即失败，`limit == 0` 与普通拦截相同，`limit > 0` 则先以 `take(limit)` 截断底层字节，然后选择透传或解压。

## 数据与状态

`WithCompression` 的状态在构造后不变：底层存储、codec 类型和可克隆的解压配置。`CompressionStorage` 的变体也在构造时固定。每次 `WriteFile` 会新建一个与压缩输出同级大小的 `Vec<u8>`，随后 `bytes()` 再克隆一份交给底层；`ReadFile` 同时持有底层压缩 `Vec` 和逐步增长的解压输出 `Vec`。因此这两个 API 是整文件内存路径，大对象应优先走 `Create`/`Open`。

`SharedObjectReader` 通过 `Rc<RefCell<_>>` 在单线程内共享可变 reader：每次 read/seek 都临时取得可变借用。`CompressReader` 的 `reader` 持有解码状态，`source` 持有底层资源句柄。它没有缓存“已解压字节数”，所以 `SeekFrom::Current(0)` 查询的不是逻辑明文位置。

`DecompressConfig::zstd_decode_concurrency` 在本文件的 `new_decompress_reader` 中只被读取到局部下划线变量，没有改变 decoder；这与 `pkg/objstore/compressedio/reader.rs::new_reader` 会根据该值选择同步/后台 Zstd reader 的行为不同，属于当前实现事实。

## 依赖与调用关系

上游方面，RustCodeGraph 将本文件标记为被 13 个文件使用，但对四个精确入口的 `callers` 查询未返回静态调用边。原始引用搜索确认，当前可见的 Rust 直接使用主要在测试中：

- `pkg/objstore/compress_test.rs` 直接构造 `WithCompression`，覆盖整文件 Gzip 读写、多 member Gzip 以及非法 header。
- `pkg/objstore/objectio/writer_test.rs::get_store` 调用 `with_compression`，在对象 writer 测试中同时支持透传与压缩存储。
- `pkg/objstore/azblob_1_aster_unit_test.rs` 用 `WithCompression` 做组合行为校验。

Go 主链则明确有生产调用：`pkg/lightning/mydump/parser.go` 和 `reader.go` 用 `WithCompression`，`loader.go` 用 `NewLimitedInterceptReader`，`pkg/executor/load_data.go` 用 `InterceptDecompressReader`，`dumpling/export/writer_util.go` 用 `WithCompression(...).Create`。因此 Rust 文件是这一 Go 合同的移植实现，但本次证据不支持声称它已接入对应 Rust 生产主链。

下游方面，存储操作全部依赖 `storeapi::Storage`；流式句柄依赖 `objectio::{Reader, Writer}`；压缩写路径依赖 `objectio::compressedio::new_writer`、`objectio::new_buffered_writer`；解压路径直接调用 `flate2::MultiGzDecoder`、`snap::FrameDecoder` 和 `zstd::stream::read::Decoder`。`lock_unpoisoned` 来自 `crate::azblob`，用于处理 `SharedBuffer` 的 mutex 中毒。

## 错误处理与边界

- 所有底层 `Storage` 错误和 `io::Error` 通过 `anyhow::Result` 与 `?` 原样向上传播；本文件不重试、不回滚已写入的块。
- 压缩/解压工厂意外返回 `None` 时，整文件写路径报 `compression writer is unavailable`，读路径报 `decompression reader is unavailable`。正常的非 `NoCompression` 枚举分支均应产生 codec。
- Gzip 在构造 decoder 时检查 header，故 `Open` 对显然非法输入立即失败；后续 CRC、截断或 Snappy/Zstd 解码错误可在读取过程中才出现。
- `MultiGzDecoder` 读取连接的多个 Gzip member，与 Go gzip reader 默认 multistream 行为对齐。
- `new_limited_intercept_reader` 拒绝负数；若正数限制在一个压缩帧中间截断，解压读取应从 codec 获得截断/数据错误，本层不尝试补齐。
- 包装后只允许 `SeekFrom::Current(0)`；任何实际移动都返回 `InvalidInput`。`file_size` 固定返回 `Unsupported`，因为本层既没有明文大小元数据，也不把压缩对象大小冒充为逻辑大小。
- `Create` 只有在成功创建底层 writer 后才加包装；调用者必须调用 writer `close` 才能刷出压缩尾块并完成对象。

## 并发与资源生命周期

`WithCompression<S>` 可满足 `Storage: Send + Sync` 是因为其持有的 `S` 必须满足该约束，而配置是普通值。每次方法调用创建自己的 codec 和缓冲，装饰器本身没有跨请求的可变压缩状态。

`SharedBuffer` 用 `Arc<Mutex<Vec<u8>>>` 是因为压缩 writer 持有缓冲的克隆，而 `WriteFile` 在 writer 关闭后还需读取该缓冲。`lock_unpoisoned` 避免 mutex 曾中毒导致二次 panic。当前代码本身不启动线程，该结构的线程安全性主要用于满足共享 writer 的所有权需求。

`SharedObjectReader` 故意使用 `Rc<RefCell<_>>`，所以单个解压 reader 不是跨线程共享对象；`objectio::Reader` 也没有 `Send`/`Sync` 约束。`RefCell` 依赖非重入的顺序 read/seek，同一时刻发生重叠可变借用会 panic。正常 codec 调用是同步顺序的。

资源关闭不依赖本文件自定义的 `Drop`：整文件压缩显式关闭 codec，流式 writer 由调用者关闭，`CompressReader::close` 显式关闭底层 reader，`WithCompression::Close` 转发到底层 storage。丢弃流式 writer/reader 不能视为已完成协议级 close。

## 与 Go 版本的对应关系

Rust `WithCompression`/`with_compression`、`intercept_decompress_reader`、`new_limited_intercept_reader` 和 `CompressReader` 分别对应 `pkg/objstore/compress.go` 的 `withCompression`/`WithCompression`、`InterceptDecompressReader`、`NewLimitedInterceptReader` 和 `compressReader`。两边共同的核心合同包括：无压缩透传；整文件先压缩后存储/先读取后解压；流式 reader 仅允许查询底层当前偏移；不支持文件大小；负限制报错；正限制在解压前生效。

已确认的差异与迁移边界是：

- Go `WithCompression` 在 `NoCompression` 时直接返回原 interface；Rust 为保持具体返回类型，用 `CompressionStorage::Plain` 表达同样语义。直接使用 Rust `WithCompression::new` 则仍会存在一层装饰器，但关键方法内部短路。
- Go `Create` 会检查底层 writer 是否已是 `*objectio.BufferedWriter` 并先 `GetWriter()` 解包，避免双层缓冲；Rust `Create` 直接包装返回的 `Box<dyn Writer>`，没有等价的运行时解包。若底层 Rust 后端已返回 `BufferedWriter`，分块、统计或 close 时序可与 Go 不同，扩展时必须专门验证。
- Go 通用 `compressedio.NewReader`，其 Zstd 实现使用 `DecompressConfig` 的并发度；本 Rust 文件直接构造同步 `zstd::stream::read::Decoder` 并忽略配置的行为效果。
- Go 文件后半还定义 `flushStorageWriter` 及访问统计逻辑；Rust `compress.rs` 没有这组符号，流式写的缓冲与可选 `AccessStats` 能力在 `pkg/objstore/objectio/writer.rs` 中实现。
- Go 返回 PingCAP 分类错误（如 `ErrStorageInvalidConfig`、`ErrUnsupportedOperation`）；Rust 用 `anyhow` 与 `io::ErrorKind::{InvalidInput, Unsupported, InvalidData}` 表达对应边界，错误类型身份并非一一相同。

`pkg/objstore/compress_test.go::TestWithCompressReadWriteFile` 是 Rust `test_with_compress_read_write_file` 的主要对照。Rust 测试另外锁定了 Go 默认的 Gzip multistream 语义和非法 Gzip header 在 `Open` 阶段立即拒绝的行为。

## 扩展指南

- 增加压缩格式时，至少同步 `objectio::CompressType`、`new_decompress_reader`、`objectio::compressedio::new_writer` 与 buffered-writer 的压缩 buffer；同时更新 Go 对照和独立的 `pkg/objstore/compress_test.rs`，不应只让整文件或只让流式路径支持新格式。
- 改动解压配置时，需决定 `compress.rs::new_decompress_reader` 是否改为复用 `compressedio::new_reader`。需特别评估后者的 `SendReader` 与后台线程约束，并为 Zstd `zstd_decode_concurrency == 1` 和默认值分别添加测试。
- 扩展 seek 必须先定义逻辑偏移还是压缩偏移。普通 codec 不支持任意明文 seek；安全方案通常需重建 decoder 并从已知 frame/index 边界恢复，而不是直接转发底层 seek。
- 修改 limit 语义时，先确认它是“最多读取压缩字节”还是“最多产生明文字节”；当前是前者。需在独立测试文件中覆盖负数、0、恰好 frame 边界和截断 frame。
- 修改 `Create` 时必须验证底层已是 `BufferedWriter` 的情形，并与 Go 的解包意图对齐；关注是否双重分块、访问统计是否重复、尾块与底层 close 的顺序。
- 整文件 API 的内存占用与额外克隆是性能风险。若要优化 `SharedBuffer::bytes` 的克隆，需保证 codec 已 close 且无其他强引用，并保留错误时不向底层提交不完整对象的不变量。
- 测试仍应位于独立 `pkg/objstore/compress_test.rs`，不要内嵌回生产文件；需跨 writer 层验证时可同步 `pkg/objstore/objectio/writer_test.rs`。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个符号、1848419 条边；`node --file pkg/objstore/compress.rs --offset 1 --limit 520` 读取了全部 434 行。
- RustCodeGraph 符号查询确认 `WithCompression`、`with_compression`、`CompressReader`、`intercept_decompress_reader`、`new_limited_intercept_reader` 和 `new_decompress_reader`的定义位置；对后四者执行了 `callers`/`callees`，精确边输出为空，因此另以原始引用搜索补齐调用证据。
- 已读 Rust 源码/配置：`pkg/objstore/compress.rs`、`pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`、`pkg/objstore/compressedio/def.rs`、`pkg/objstore/compressedio/reader.rs`、`pkg/objstore/objectio/interface.rs`、`pkg/objstore/objectio/writer.rs` 和 `pkg/objstore/storeapi/storage.rs`。
- 已读独立 Rust 测试：`pkg/objstore/compress_test.rs`、`pkg/objstore/objectio/writer_test.rs` 的压缩存储入口与 Zstd 解码段，以及 `pkg/objstore/azblob_1_aster_unit_test.rs` 的组合验证段。
- 已读 Go 对照：`pkg/objstore/compress.go`、`pkg/objstore/compress_test.go`；并搜索到 `pkg/lightning/mydump/{parser,reader,loader}.go`、`pkg/executor/load_data.go` 和 `dumpling/export/writer_util.go` 的生产调用点。
- 本任务只新增文档，按计划不运行 Cargo；最终用任务指定的 `test` + `rg -c` 命令确认文件存在且恰好包含 11 个固定二级章节。

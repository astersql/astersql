# `pkg/ingestor/ingestctrl/compress.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate。crate 入口 `pkg/ingestor/ingestctrl/lib.rs` 通过 `pub mod compress` 公开该模块；`pkg/ingestor/ingestctrl/Cargo.toml` 以普通依赖引入同一工作区内的 `astersql-util-compress`，本文件据此复用全局 gzip Reader/Writer 对象池。

从完整应用链路看，Go 对照实现用于 local backend 创建导入服务 gRPC 连接时的消息压缩；具体入口是 `pkg/ingestor/ingestctrl/local.go::makeConn`。不过当前 Rust 代码的事实不同：仓库内对 `gzipCompressor`、`gzipDecompressor` 的 Rust 引用只出现在 `compress.rs` 自身和独立测试 `compress_test.rs`，`pkg/ingestor/ingestctrl/local.rs` 尚无对应连接压缩接线。因此，本文件当前是已公开、可独立测试的 gzip 适配层，而不是已经进入 Rust ingest 运行主链的组件。

## 核心职责

- `gzipCompressor::Do` 将一段完整的原始字节压缩成 gzip 数据，再完整写入调用方提供的 `std::io::Write`。
- `gzipDecompressor::Do` 读尽调用方提供的 `std::io::Read`，解压完整 gzip 数据并返回原始字节。
- 两个 `Type` 方法都返回协议标识 `"gzip"`，与 Go 的 gRPC 压缩器/解压器命名保持一致。
- 编解码器实例不在结构体中保存，而是从 `pkg/util/compress/gzip.rs` 的 `GzipWriterPool`、`GzipReaderPool` 获取并在本次调用结束前归还，以复用底层对象。

本文件不负责选择压缩算法、协商 gRPC 选项、限制输入大小或管理网络连接；这些应由上层完成。当前 Rust 版本也没有实现某个 gRPC compressor trait。

## 主要符号

- `pub struct gzipCompressor;`：无字段的公开零大小类型。调用方以值或引用调用其固有方法；名称沿用 Go 版本，因此由 crate 根部的 `non_camel_case_types`/`non_snake_case` 宽免承载非惯用 Rust 命名。
- `gzipCompressor::Do(&self, writer: &mut dyn Write, payload: &[u8]) -> std::io::Result<()>`：压缩单个完整 payload，并用 `write_all` 保证成功时全部压缩字节已交给输出 writer。
- `gzipCompressor::Type(&self) -> &'static str`：返回静态字符串 `"gzip"`，不分配内存。
- `pub struct gzipDecompressor;`：无字段的公开零大小类型，与压缩器成对出现。
- `gzipDecompressor::Do(&self, reader: &mut dyn Read) -> std::io::Result<Vec<u8>>`：读取、解压并拥有返回缓冲区。
- `gzipDecompressor::Type(&self) -> &'static str`：返回静态字符串 `"gzip"`。

文件没有模块级常量、trait、条件编译分支或自定义错误类型。所有失败直接使用 `std::io::Error`。

## 执行流程

压缩路径 `gzipCompressor::Do`：

1. 调用 `GzipWriterPool.get()` 取得一个 `GzipWriter`；池为空时由依赖 crate 创建新实例。
2. 调用 `GzipWriter::compress(payload)`。依赖实现会为本次输入重建 `flate2::write::GzEncoder<Vec<u8>>`，写入全部 payload，调用 `try_finish` 完成 gzip 尾部，并克隆出压缩结果。
3. 在检查压缩结果之前先调用 `GzipWriterPool.put(compressor)`，所以压缩成功或失败都会归还已取得的对象。
4. 对压缩结果应用 `?`；成功时调用目标 writer 的 `write_all`，失败时原样返回 I/O 错误。

解压路径 `gzipDecompressor::Do`：

1. 先用 `reader.read_to_end` 将全部压缩输入读入 `compressed`；若输入读取失败，此时尚未获取池对象，错误直接返回。
2. 从 `GzipReaderPool` 取得 reader，以 `reset(compressed)` 将本次拥有的压缩缓冲区绑定到新的 `flate2::read::GzDecoder`。
3. 调用解码器的 `read_to_end` 把全部原始数据写入 `output`，并把成功结果映射为该向量。
4. 无论解码成功还是失败，都先把解码器放回 `GzipReaderPool`，再返回保存的结果。

`Type` 路径没有状态与分支，仅返回协议名。

## 数据与状态

两个公开类型都是无字段零大小类型，本文件自身没有每实例状态。一次调用内的状态全部是局部变量：压缩侧持有池对象和完整压缩 `Vec<u8>`，解压侧同时可能持有完整压缩输入、解码器内部的输入所有权以及持续增长的完整输出。

全局可变状态位于依赖 `pkg/util/compress/gzip.rs`：`GzipWriterPool` 和 `GzipReaderPool` 都是 `LazyLock<ReusablePool<_>>`，池内部以 `Mutex<Vec<T>>` 保存闲置对象。`get` 从尾部弹出或调用工厂创建，`put` 将对象压回；互斥锁中毒时依赖实现通过 `into_inner` 继续使用池。

当前接口没有输入大小上限，也不做流式输出。压缩至少形成一份完整压缩结果后才写入目标；依赖的 `GzipWriter::compress` 还会克隆编码器内部缓冲。解压先读尽压缩输入，再建立解码器并累积完整输出。因此处理不可信或超大输入时，调用方必须在进入本层前实施大小约束，否则存在显著峰值内存和解压膨胀风险。

## 依赖与调用关系

上游关系：

- `pkg/ingestor/ingestctrl/lib.rs` 声明 `pub mod compress`，使符号可通过 crate 模块路径访问。
- `pkg/ingestor/ingestctrl/compress_test.rs` 直接构造两个零大小类型，覆盖 `Type`、往返互操作和重复调用。
- RustCodeGraph 的目标文件查询识别出本文件 7 个符号；对两个结构体的 callers/callees 查询未返回 Rust 生产调用边。随后以精确 `rg` 搜索全部 `*.rs`，除本文件外只命中 `compress_test.rs`，证实当前没有 Rust 生产调用者。
- Go 生产调用者是 `pkg/ingestor/ingestctrl/local.go::makeConn`：当 `compressionType` 为 `CompressionGzip` 时，把 `gzipCompressor` 和 `gzipDecompressor` 传给 gRPC dial options。

下游关系：

- `std::io::{Read, Write}` 定义调用边界以及 `read_to_end`/`write_all` 的 I/O 错误语义。
- `astersql_util_compress::{GzipReaderPool, GzipWriterPool}` 提供池和实际 gzip 编解码；该 crate 的 `pkg/util/compress/Cargo.toml` 使用 `flate2 = "1"`。
- 本文件不直接依赖 `flate2`，也不直接依赖 Rust gRPC 库；算法实现与传输层接线分别被留在下游工具 crate和未来上层接线中。

## 错误处理与边界

- 压缩错误来自 `GzipWriter::compress` 内的写入或 `try_finish`，通过 `?` 原样传播；池对象已经归还。之后目标 writer 的 `write_all` 也可失败，此时压缩器同样已经归还。
- 解压前读取输入的错误由首个 `read_to_end` 直接传播。获得解码器后，非法、截断或 I/O 层报告的 gzip 错误由解码 `read_to_end` 返回；实现先保存结果、归还对象，再向调用方返回错误。
- 空 payload 的压缩由下游 gzip 编码器处理，仍应产生合法 gzip 流；现有目标测试未单独断言空输入。
- `gzipDecompressor::Do` 不接受未压缩数据作为成功结果，也不自行探测算法。它也不验证尾随数据策略或多成员 gzip 的特定兼容契约；这些行为取决于 `flate2::read::GzDecoder`，当前独立测试只验证单成员、有效数据。
- 方法不会把错误转换为 crate 根部的 `ingestctrl::Error`；若未来接入返回该错误的上层 API，应由接线层利用已有的 `From<std::io::Error>` 或显式映射。
- `write_all` 只保证向传入 writer 完整写入压缩字节，不负责 flush、关闭或持久化该 writer。

## 并发与资源生命周期

零大小适配器没有共享的实例字段，`Do` 只使用调用栈上的局部数据；同一个适配器值可被多个调用路径独立使用，但方法签名本身没有声明 `Send + Sync` trait 边界。真正共享的是两个全局池，`ReusablePool` 用 `Mutex` 串行化短暂的 get/put 操作；压缩和解压工作在锁外完成，所以长时间编解码不会持续占用池锁。

资源生命周期是“获取—执行—归还”。本文件没有使用 RAII guard，但当前控制流特意在检查压缩结果之前归还 writer，并在返回解压结果之前归还 reader，从而覆盖编解码错误。解压输入在 `reset` 后转移进池化 reader；reader 返回池时仍持有本次 decoder 和输入缓冲，直到下一次 `reset` 替换它，因此池复用可以保留一部分内存。进程结束前全局池一直存活，没有显式清空或容量上限。

代码不创建线程、异步任务、通道、锁、事务或取消令牌；调用是同步且阻塞的。上层若在异步运行时调用，需自行避免在延迟敏感的 executor 线程上处理大 payload。

## 与 Go 版本的对应关系

Rust 的两个类型、`Do`/`Type` 方法名、`"gzip"` 标识以及 Reader/Writer 池复用意图直接对应 `pkg/ingestor/ingestctrl/compress.go`。`compress_test.rs` 也对应 `compress_test.go` 的两个往返测试和四组 benchmark 名称。

已验证的关键差异如下：

- Go 通过编译期断言确认类型实现 `grpc.Compressor`/`grpc.Decompressor`，并由 `local.go::makeConn` 实际注册；Rust 只有固有方法，没有 gRPC trait 实现，也没有 Rust 生产调用者。
- Go 压缩器把池化 `gzip.Writer` reset 到调用方 writer，直接流式写入并 `Close` 完成尾部；Rust 下游 `GzipWriter::compress` 先在内存中生成完整 `Vec<u8>`，再由本文件 `write_all`。
- Go 解压器把池化 reader 直接 reset 到调用方 reader，`io.ReadAll` 后 defer `Close` 和归还；Rust 先读尽压缩 reader 到 `Vec<u8>`，`reset` 不返回错误，解码错误在后续读取时出现，也没有独立 close 操作。
- Go 测试用 `crypto/rand` 生成 1 MiB 输入；Rust 测试用确定性公式生成同量级输入，便于复现。
- Go benchmark 使用测试框架的 `b.N` 和标准 gRPC 实现作对照；Rust 的同名函数是普通 `#[test]`，自定义路径只循环 4 次，对照路径直接调用相同的池化工具实现。因此它们验证可执行性和互操作，不构成与 Go benchmark 等价的性能测量。

这些差异意味着“格式互操作”已有测试证据，但“gRPC 接线等价”“流式内存特征”和“性能等价”目前都不能声称已完成。

## 扩展指南

- 接入 Rust ingest 连接链路时，应优先修改上层连接工厂并为本类型实现所选 Rust gRPC 库要求的明确 trait；同时新增独立测试验证配置为 gzip/none/非法值时的连接选项。不要仅依赖公开模块就宣称已接线。
- 若增加压缩算法，不应让 `Type` 与实际编码器分叉。可抽取明确的算法接口或枚举，但需要同步验证协议名、双方互操作和不支持算法的错误路径。
- 若要降低峰值内存，应从 `gzipCompressor::Do`/`gzipDecompressor::Do` 与 `pkg/util/compress/gzip.rs` 的 API 一起设计流式读写，特别注意结束 gzip member、部分写、输入读取错误以及对象归还；仅在本文件增加分块循环无法消除下游 `compress` 的整块缓冲。
- 若引入输入/输出限额，应在分配和解压膨胀之前强制执行，并增加压缩炸弹、截断流、无效 header、空输入、writer 部分失败等回归测试。
- 修改本文件行为时同步更新独立的 `pkg/ingestor/ingestctrl/compress_test.rs`，不得把测试嵌入生产源文件；需要核对 Go 语义时同时参考 `compress_test.go`。若改变公共池语义，还应更新 `pkg/util/compress/migration_aster_unit_test.rs`。
- 性能敏感改动需关注完整缓冲、额外 clone、全局池无界保留和 Mutex 争用；兼容性改动需保持标准 gzip 格式及 `"gzip"` 协议标识。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ingestor/ingestctrl/compress.rs` 确认目标已索引；`node --file ...` 读取目标、模块入口、Rust/Go 对照和测试；`query gzipCompressor`/`query gzipDecompressor` 定位两种语言定义；结构体 callers/callees 查询没有返回 Rust 生产边。
- 目标实现：`pkg/ingestor/ingestctrl/compress.rs`，重点符号为 `gzipCompressor::{Do, Type}` 与 `gzipDecompressor::{Do, Type}`。
- crate 与模块边界：`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/ingestctrl/lib.rs`。
- 下游实现：`pkg/util/compress/gzip.rs`、`pkg/util/compress/lib.rs`、`pkg/util/compress/Cargo.toml`，用于核对 `ReusablePool`、`GzipWriter::compress`、`GzipReader::reset` 及 `flate2` 依赖。
- Go 对照与生产接线：`pkg/ingestor/ingestctrl/compress.go`、`pkg/ingestor/ingestctrl/local.go::makeConn`。
- 独立测试：`pkg/ingestor/ingestctrl/compress_test.rs`、`pkg/ingestor/ingestctrl/compress_test.go`；补充池语义测试位于 `pkg/util/compress/migration_aster_unit_test.rs`。
- 精确文本复核：对 `pkg/**/*.rs` 搜索 `gzipCompressor`、`gzipDecompressor`、`WithCompressor`、`WithDecompressor`，只发现目标实现和 `compress_test.rs`，据此将 Rust 生产接线状态记录为“尚未接线”，而不是推断已有调用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行固定十一章节的结构验证，并人工复核本文没有把 Go 调用关系写成 Rust 当前事实。

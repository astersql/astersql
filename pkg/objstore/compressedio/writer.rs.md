# `pkg/objstore/compressedio/writer.rs`

## 文件定位

本文件属于 `astersql-objstore-compressedio` crate 的压缩写出层。crate 入口 [`lib.rs`](lib.rs) 将 `writer` 声明为公开模块并重导出其 API；[`Cargo.toml`](Cargo.toml) 表明该 crate 直接依赖 `flate2`、`snap` 和 `zstd`，分别承载 Gzip、Snappy framed stream 和 Zstandard 的编码实现。

它位于“明文字节 → 流式压缩器 → 任意 `std::io::Write` 输出”的边界。直接入口是 `new_writer`；上层既可像 [`../compress.rs`](../compress.rs) 的 `WithCompression::WriteFile` 一样直接压缩整块数据，也可由 [`buffer.rs`](buffer.rs) 的 `new_buffer` 包成可复用压缩缓冲，再进入 [`../objectio/writer.rs`](../objectio/writer.rs) 的分块上传链。

## 核心职责

- 用 `Writer` trait 将标准库 `Write`、本 crate 的 `Flusher` 和显式 `close` 合并成统一的压缩写入接口。
- 为 Gzip、Snappy、Zstd 三种第三方编码器提供相同的动态分发形态 `Box<dyn Writer>`。
- 用 `Option<Encoder>` 表达打开/关闭状态，使 `close` 幂等，并让关闭后的 `write`/`flush` 返回明确的 `BrokenPipe`。
- 在关闭时执行各格式所需的收尾：Gzip 写出 footer，Snappy 刷出缓冲，Zstd 写出帧结束标记。
- 将 `CompressType::NoCompression` 明确留给上层明文路径，而不是在这里制造透传 writer。

本文件不负责选择对象存储后端、分块、上传、读取或解压；这些职责分别位于 `objstore`、`objectio::BufferedWriter`、`buffer.rs` 和 `reader.rs`。

## 主要符号

- `pub trait Writer: Write + Flusher`：公开能力边界。调用者可通过标准 `write`/`write_all` 写明文，通过 `Flusher::flush` 推送压缩器内部缓冲，并通过 `close(&mut self)` 完成压缩流。
- `GzipWriter<W: Write>`：私有包装器，持有 `Option<GzEncoder<W>>`。`close` 对编码器执行 `try_finish`。
- `SnappyWriter<W: Write>`：私有包装器，持有 `Option<FrameEncoder<W>>`。`close` 先取得所有权，再执行 `flush`。
- `ZstdWriter<W: Write>`：私有包装器，持有 `Option<ZstdEncoder<'static, W>>`。`close` 调用会消费编码器的 `finish`。
- `open_mut<T>`：三个包装器共享的打开态检查；`None` 被映射为 `io::ErrorKind::BrokenPipe`，消息为 `compressed writer is closed`。
- `pub fn new_writer(CompressType, Box<dyn Write>) -> Option<Box<dyn Writer>>`：公开工厂。Gzip 使用默认压缩级别，Snappy 使用 framed encoder，Zstd 使用级别 `0`，无压缩返回 `None`。

三个包装器的 `Write::flush` 都显式转发给 `Flusher::flush`，避免与同名 trait 方法产生歧义；实际刷新最终落到各第三方编码器。

## 执行流程

1. 上层按配置得到 `CompressType`，把真实输出端装箱为 `Box<dyn Write>` 后调用 `new_writer`。
2. 工厂按类型创建对应编码器并包进 `Some`；`NoCompression` 不创建编码器，返回 `None`。Zstd 构造可能失败，此时打印错误并返回 `None`。
3. 调用者写入明文时，包装器的 `Write::write` 通过 `open_mut` 取得打开的编码器，再由编码器产生格式化压缩字节并写向底层输出端。
4. 调用 `flush` 时，只刷新编码器当前可输出的数据；这不能替代 `close`，尤其不能假定压缩流的尾部已经完整写出。
5. 调用 `close` 时，包装器先用 `Option::take` 将状态永久切换为关闭，再执行格式相关的收尾。第二次 `close` 因字段已为 `None` 而直接成功。
6. 关闭后的 `write` 或 `flush` 经 `open_mut` 返回 `BrokenPipe`，不会再次访问已经消费或丢弃的编码器。

两条已接线的上层路径是：

- `WithCompression::WriteFile` → `new_writer` → `write_all` → `close` → 将完整压缩字节交给底层存储。
- `objectio::BufferedWriter` → `new_intercept_buffer` → `compressedio::new_buffer` → `new_writer`；缓冲按容量刷新并上传压缩块，最终 `BufferedWriter::close` 先关闭压缩缓冲以生成尾部，再上传剩余字节并关闭对象 writer。

## 数据与状态

每个具体 writer 只有一个核心状态：`encoder: Option<...>`。`Some` 表示流仍打开，`None` 表示已经调用过 `close`（包括收尾过程返回错误的情况）。由于 `close` 在调用第三方收尾函数之前执行 `take`，一旦收尾失败，该实例仍保持关闭，不能重试同一个编码器。

输入数据以借用切片传入，编码器按流式协议处理；本文件自身不保存完整明文或压缩结果。压缩输出存放在哪里由注入的 `Box<dyn Write>` 决定：在 `buffer.rs` 中是共享字节缓冲，在 `compress.rs` 中也是用于整块落盘前收集结果的共享缓冲。

工厂配置是固定的：Gzip 为 `Compression::default()`，Zstd 级别为 `0`，Snappy 使用 `FrameEncoder` 的默认配置。本文件没有暴露级别、字典或编码并发度参数。

## 依赖与调用关系

上游直接证据：

- [`buffer.rs`](buffer.rs) 的 `new_buffer` 调用 `new_writer`，并由 `Buffer::{write,flush,close}` 转发到返回的 trait object。
- [`../compress.rs`](../compress.rs) 的 `WithCompression::WriteFile` 直接调用 `new_writer`，完整执行 `write_all` 和 `close` 后才读取压缩结果。
- [`../objectio/writer.rs`](../objectio/writer.rs) 的 `new_intercept_buffer` 在有压缩配置时创建 `compressedio::Buffer`，其 `BufferedWriter::close` 负责关闭压缩流、上传尾块、再关闭底层对象 writer。

下游依赖：

- `flate2::write::GzEncoder` 与 `flate2::Compression`：Gzip 流编码和默认压缩级别。
- `snap::write::FrameEncoder`：Snappy framed stream 编码；它与读取侧使用的帧解码格式配套。
- `zstd::stream::write::Encoder`：Zstd 流编码、构造失败和显式 `finish`。
- `std::io::{Write, Error, ErrorKind}`：统一写入协议及关闭态错误。
- `super::{CompressType, Flusher}`：格式选择和 crate 内刷新能力。

RustCodeGraph 索引将 `writer.rs` 识别为 20 个符号，并显示它由 25 个索引文件间接使用；精确的业务接线以上述源码入口为准。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/objstore/compressedio`，确认其 Go 对照边界。

## 错误处理与边界

- 底层输出端或编码器的 `write`、`flush`、`try_finish`、`finish` 错误均以 `io::Result` 原样向上传播，本层不重试、不改写上下文。
- 关闭后的 `write`/`flush` 稳定返回 `BrokenPipe`；重复 `close` 返回 `Ok(())`。
- 收尾失败后编码器已经被 `take`，后续重复 `close` 仍返回成功，因此调用者必须保留并处理第一次 `close` 的错误。
- Zstd 是唯一可能在工厂阶段失败的分支；当前 API 只打印到标准错误并返回 `None`，因此 `None` 同时可能表示“明确选择无压缩”或“Zstd 初始化失败”。上层若预期压缩，必须结合原始 `CompressType` 将 `None` 转成错误；`WithCompression::WriteFile` 已这样处理。
- `NoCompression` 返回 `None` 是与 Go `nil` writer 对齐的既定语义，不应直接传给 `compressedio::Buffer` 的写/刷/关操作；相关 Rust 测试明确验证这些误用会 panic。正常上层在 `objectio::new_intercept_buffer` 中选择独立的 `PlainBuffer`。
- `flush` 只表示将当前编码器缓冲推向底层，不等价于生成完整、可独立解码的最终流；需要完整输出时必须调用 `close`。
- 本文件未实现 `Drop` 来报告收尾错误。调用者不能依赖离开作用域代替显式 `close`，因为析构无法把 I/O 错误返回给业务层。

## 并发与资源生命周期

`Writer` 没有 `Send` 或 `Sync` 约束，三个包装器也没有锁、线程或异步任务；单个实例应由持有它的调用链串行使用。是否能跨线程取决于具体底层类型，但公开的 `Box<dyn Writer>` 并未承诺这种能力。

资源生命周期为“构造 → 多次 write/flush → 一次有效 close → 关闭态”。`Option::take` 保证底层编码器最多执行一次收尾并随后释放。Gzip 和 Zstd 的 close 明确生成格式尾部；Snappy close 刷新 framed encoder 后，局部编码器在函数结束时析构。所有分支都不会关闭一个抽象的远端对象存储 writer；它们只完成压缩层，外层 `objectio::BufferedWriter` 随后负责上传尾块和关闭远端 writer。

`buffer.rs` 为收集输出使用 `Arc<Mutex<Vec<u8>>>`，但该同步属于输出容器而非本文件的编码器。它允许 `Buffer` 与编码器共同持有字节区，不意味着压缩 writer 可以被并发调用。

## 与 Go 版本的对应关系

直接对照文件是 [`writer.go`](writer.go)：Go 的 `Writer` 组合 `io.WriteCloser` 与 `Flusher`，Rust 的 `Writer: Write + Flusher` 再增加显式 `close`，能力集合一致。两边工厂都按 `Gzip`、`Snappy`、`Zstd` 创建对应流式编码器，其他值（包括 `NoCompression`）返回空值。

主要实现差异如下：

- Go 直接返回第三方库实现；Rust 增加三个私有包装器，以统一 Rust 编码器不同的结束 API，并显式记录关闭态。
- Go 的 Zstd 构造错误通过结构化日志告警后返回 `nil`；Rust 当前通过 `eprintln!` 输出并返回 `None`。两者都不从工厂单独返回错误，但日志设施不同。
- Rust 明确定义了重复 close 成功、关闭后 write/flush 为 `BrokenPipe`；Go 侧这些细节由各第三方 writer 自身决定，不能假定错误文本完全相同。
- Go 使用 `klauspost/compress` 的 gzip/snappy/zstd；Rust 分别使用 `flate2`、`snap`、`zstd`。兼容目标是流格式和上层行为，而不是压缩字节逐字节相同。

[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `compressed_buffer_round_trips_all_go_formats` 验证三种格式在 write/flush/close 后均可由对应 reader 还原原文，`no_compression_factories_match_go_nil_semantics` 验证无压缩空值语义。[`../objectio/writer_test.rs`](../objectio/writer_test.rs) 的 `test_compress_reader_writer` 进一步覆盖对象存储写入、磁盘直接解压和 Storage 读取链。

## 扩展指南

新增压缩格式时，应同步完成以下最小闭环：

1. 在 `def.rs::CompressType` 及解析/后缀映射中加入稳定枚举值，并与 Go `def.go` 保持数值和配置名兼容。
2. 在本文件增加私有包装器，明确区分普通 `flush` 和最终 `close`，继续使用可观察的关闭态；然后扩展 `new_writer` 分支。
3. 在 `reader.rs::new_reader` 增加配套解码路径，确保实际流格式互通。
4. 在独立测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 扩展全格式往返表，并在 [`../objectio/writer_test.rs`](../objectio/writer_test.rs) 扩展端到端对象写读覆盖；不要把测试内嵌到本生产文件。
5. 对构造失败、close 失败、重复 close、关闭后 write/flush 建立独立回归测试。当前现有测试覆盖格式往返和无压缩空值，但没有直接锁定全部关闭态语义。

若仅需新增压缩级别或字典配置，最可能改动 `new_writer` 的签名和三个构造分支；这会穿透 `buffer::new_buffer`、`objectio::new_intercept_buffer` 和 `WithCompression` 配置链，应避免静默改变现有默认压缩比、CPU 消耗或产物兼容性。若要让 Zstd 构造错误可区分于无压缩，优先考虑把工厂改为 `io::Result<Option<Box<dyn Writer>>>`，并同步所有直接调用者，而不是继续扩大 `None` 的含义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/objstore/compressedio` 列出本 crate 的 12 个 Go/Rust 文件；`node --file pkg/objstore/compressedio/writer.rs --offset 1 --limit 240` 返回完整 153 行及 20 个符号；`query new_writer --kind function --json`、`query open_mut --kind function --json`、`query GzipWriter/SnappyWriter/ZstdWriter --kind struct` 确认主要符号及位置。
- 源码：[`writer.rs`](writer.rs)；直接上游 [`buffer.rs`](buffer.rs)、[`../compress.rs`](../compress.rs)、[`../objectio/writer.rs`](../objectio/writer.rs)；crate 边界 [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)；公共定义 [`def.rs`](def.rs)。
- Go 对照：[`writer.go`](writer.go)、[`buffer.go`](buffer.go)、[`def.go`](def.go)。
- 独立测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`buffer_test.rs`](buffer_test.rs)、[`../objectio/writer_test.rs`](../objectio/writer_test.rs)，以及外部包风格的 Go 对照测试 [`../objectio/writer_test.go`](../objectio/writer_test.go)。
- 人工事实复核：文档分别回答了文件为何存在（统一三种压缩 writer）、如何运行（工厂—写入—刷新—关闭）、如何安全扩展（同步枚举、writer、reader、上层配置与独立测试），并区分已验证行为和当前未直接测试的关闭态细节。
- 本任务为纯文档分析，依照任务约束未运行 Cargo 或代码测试；交付验证只检查文档结构、路径链接和 diff 范围。

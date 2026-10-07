# `pkg/objstore/compressedio/buffer.rs`

源文件：[buffer.rs](./buffer.rs)。本说明只描述当前实现及其直接接线，不把未出现于代码中的能力视为已支持。

## 文件定位

本文件位于 `astersql-objstore-compressedio` crate，定义压缩输出使用的内存缓冲 `Buffer` 及工厂 `new_buffer`。crate 根 `pkg/objstore/compressedio/lib.rs` 公开 `buffer` 模块并再导出其公开项；`pkg/objstore/compressedio/Cargo.toml` 表明该 crate 直接依赖 `flate2`、`snap` 和 `zstd`，具体编码器由同 crate 的 `writer.rs` 封装。

在对象存储写入链中，`pkg/objstore/objectio/writer.rs::new_intercept_buffer` 对 `NoCompression` 使用 `PlainBuffer`，对 Gzip、Snappy、Zstd 调用本文件的 `new_buffer`。因此本文件不是上传器，也不决定对象路径；它位于分块写入器与压缩编码器之间，收集编码器生成的压缩字节，供 `BufferedWriter::upload_chunk` 复制、清空并交给底层对象存储 `Writer`。

## 核心职责

- `SharedBuffer` 让压缩 `Writer` 和外层 `Buffer` 共同持有同一个 `Vec<u8>`：前者追加编码结果，后者读取长度、获取快照或清空内容。
- `Buffer` 将 `Write`、显式 `flush`/`close`、长度/容量查询、快照、清空和“当前为压缩路径”的标志组合为对象写入层所需的缓冲接口。
- `new_buffer` 按调用方给定的 `chunk_size` 预分配底层字节区，并通过 `writer.rs::new_writer` 为 `CompressType` 创建编码器。

本文件只管理内存与压缩器生命周期，不负责解压、重试、异步上传、加密或持久化。`chunk_size` 是上层分块阈值和记录值，不是底层 `Vec` 永不增长的硬上限。

## 主要符号

- `struct SharedBuffer(Arc<Mutex<Vec<u8>>>)`：私有共享容器。`Clone` 只克隆 `Arc`，所以编码器和 `Buffer` 访问同一字节区。
- `SharedBuffer::with_capacity(capacity)`：以指定初始容量创建空 `Vec`。
- `SharedBuffer::lock()`：取得互斥锁；锁若被毒化，使用 `PoisonError::into_inner` 继续访问已有数据，而不是再次 panic。
- `impl Write for SharedBuffer`：`write` 在锁内把输入追加到 `Vec`；`flush` 恒返回成功，因为该层没有额外缓存，真正需要刷新的状态在压缩编码器中。
- `pub struct Buffer`：包含 `buffer: SharedBuffer`、`writer: Option<Box<dyn Writer>>` 和构造参数 `capacity: usize`。字段不公开，调用者只能通过方法维护不变量。
- `Buffer::{len,is_empty,cap}`：分别报告当前压缩字节数、是否为空和构造时声明的分块容量。
- `Buffer::reset()`：清空当前可见压缩字节，保留分配和同一个压缩 `Writer`。
- `Buffer::{flush,close}`：要求 `writer` 存在，分别刷新编码器内部数据和结束压缩流；底层错误原样作为 `io::Result` 返回。
- `Buffer::compressed()`：恒为 `true`，供对象写入层选择压缩缓冲的循环行为。
- `Buffer::bytes()`：在锁内克隆当前字节，返回独立快照。
- `impl Write for Buffer`：把明文 `write` 转发给压缩 `Writer`；trait 的 `flush` 明确转发给固有方法 `Buffer::flush`。
- `pub fn new_buffer(chunk_size, compress_type)`：构造共享字节区，把其 clone 装箱交给 `new_writer`，再把原共享句柄、可选编码器和容量记录组装为 `Buffer`。

## 执行流程

1. `pkg/objstore/objectio/writer.rs::new_buffered_writer` 创建 `BufferedWriter`，其 `new_intercept_buffer` 仅在压缩类型不是 `NoCompression` 时调用 `new_buffer`。
2. `new_buffer` 创建空的 `SharedBuffer`，并将一个共享句柄作为 `Box<dyn Write>` 传给 `new_writer`。Gzip、Snappy 或 Zstd 编码器由此把输出写回同一 `Vec<u8>`。
3. 上层调用 `Buffer::write` 时，输入是未压缩字节；调用被转发给编码器，编码器可能立即或延迟向 `SharedBuffer` 写入压缩帧。
4. `objectio::BufferedWriter::write_inner` 依据 `len()` 与 `cap()` 判断是否需要切块。压缩器可能暂存数据或产生超过声明容量的头部，因此上层用 `compressed() == true` 继续推进，并使用饱和减法规避容量下溢。
5. 需要上传一块时，上层调用 `flush()` 促使编码器把内部数据写入共享区，随后 `bytes()` 取得拥有所有权的快照，`reset()` 清空共享区，最后将快照写给底层对象存储 writer。
6. 写入结束时，`BufferedWriter::close` 先调用 `Buffer::close`，让编码器写出尾部帧，再上传剩余字节并关闭底层 writer。遗漏 `close` 可能留下不完整的压缩流。

`reset` 只清除已经取走的输出，并不重建编码器；后续产物仍属于同一连续压缩流。单个上传分块因此不保证可独立解压，按原顺序拼接后的完整流才是对外语义。

## 数据与状态

核心状态是一个由 `Arc<Mutex<_>>` 保护的可增长 `Vec<u8>`。`Arc` 解决编码器持有输出 sink 时外层仍需观察它的问题，`Mutex` 提供安全的内部可变访问；每个查询、克隆、清空或追加动作只在对应操作期间持锁。

`capacity` 保存传入的 `chunk_size`，`cap()` 不查询 `Vec::capacity()`。这使对象写入层拥有稳定的目标分块大小，但底层 `Vec` 在压缩头部或输出较大时仍可扩容。`reset()` 调用 `Vec::clear()`，长度归零但通常保留分配以供复用。

`writer: Option<Box<dyn Writer>>` 表示编码器可能不存在：`new_writer(NoCompression, ...)` 返回 `None`，Zstd 编码器构造失败时也返回 `None`。正常对象写入接线在进入本文件前排除了 `NoCompression`；直接调用 `new_buffer` 则必须自行保证压缩类型和编码器构造有效。

## 依赖与调用关系

上游直接调用边经 RustCodeGraph 核对为：

- `pkg/objstore/objectio/writer.rs::new_buffered_writer` → `new_intercept_buffer` → `compressedio::new_buffer`。
- 独立测试 `buffer_test.rs` 与 `migration_aster_unit_test.rs` 直接调用 `new_buffer`；`pkg/objstore/objectio/migration_aster_unit_test.rs` 通过 `new_buffered_writer` 覆盖真实 Gzip 分块关闭链。

下游关系为：

- `new_buffer` → `SharedBuffer::with_capacity`，并调用 `pkg/objstore/compressedio/writer.rs::new_writer` 创建具体编码器。
- `Buffer::flush` 通过 `Flusher::flush` 分派到编码器；`Buffer::write` 和 `Buffer::close` 分别调用 `Writer::write` 与 `Writer::close`。
- `SharedBuffer::write` 最终调用标准库 `Vec<u8>` 的 `Write` 实现追加编码结果。

`lib.rs` 将本文件 API 再导出；实际压缩算法来自 Cargo 中的 `flate2`、`snap`、`zstd` 依赖，但本文件不直接绑定任何具体编码器类型。

## 错误处理与边界

- 编码器的 `write`、`flush`、`close` 错误不在本层包装，直接以 `std::io::Result` 向上传播。共享 `Vec` 的 `write` 和空 `flush` 正常情况下成功。
- `write`、`flush`、`close` 对 `writer == None` 使用 `expect("compressed writer is nil")`，会 panic。`buffer_test.rs` 分别锁定了 `NoCompression` 下三条 panic 路径，与 Go 中对 nil interface 调用方法的行为一致。
- 正常接线由 `objectio::new_intercept_buffer` 把无压缩流量导向 `PlainBuffer`，所以该 panic 是直接误用工厂时的兼容边界，不是正常无压缩上传流程。
- `new_writer` 的 Zstd 初始化失败也会得到 `None`，因此失败不是在 `new_buffer` 返回时显式报告，而会在首次写/刷/关时 panic；这是当前 API 的已知限制，扩展时不能误写成可恢复的构造错误。
- 锁毒化被恢复而非上报；这保持数据可访问，但不证明发生 panic 前的缓冲内容具备完整压缩帧语义。
- `bytes()` 执行全量复制，避免返回值在随后 `reset()` 后失效，代价是每次上传产生与当前块大小成正比的复制和临时内存。
- 本类型未实现 `Drop` 自动关闭。调用方必须显式 `close()` 才能保证 Gzip/Zstd 等格式的尾部完成；关闭后继续写的具体错误由 `writer.rs` 返回（当前包装器为 `BrokenPipe`）。

## 并发与资源生命周期

`SharedBuffer` 本身允许两个所有者安全地访问同一 `Vec`，这是编码器输出与外层读取之间的所有权桥梁，不等价于允许多个业务线程并发操作整个 `Buffer`。修改编码器状态的方法需要 `&mut self`，且 `Box<dyn Writer>` 没有声明 `Send`/`Sync` 约束；调用方不应把 `Buffer` 当作可并发写入的公共队列。

锁的临界区很短，但 `bytes()` 在持锁期间克隆整块数据。`reset()` 与编码器共享同一 sink，因此正确顺序由上层保证：先让需要上传的数据产出并获取快照，再清空；最终先 `close` 生成尾部，再上传尾块。当前 `objectio::BufferedWriter` 同步执行这些步骤，没有后台任务或通道。

生命周期从 `new_buffer` 开始，经多轮 `write` → `flush` → `bytes` → `reset` 复用同一编码器，最终以 `close` 结束。`Arc` 在 `Buffer` 与编码器均释放后回收底层字节区；本文件不持有文件描述符或网络连接。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/compressedio/buffer.go`：Go 的 `Buffer` 内嵌 `*bytes.Buffer`，保存 `Writer` 和固定的 `cap`；Rust 用 `SharedBuffer` 加 trait object 表达同一关系。两者均让 `Write` 进入压缩器，让编码结果落到可读取/清空的底层字节缓冲，并提供 Len/Cap/Reset/Flush/Close/Compressed。

对应关系与差异如下：

- Go `NewBuffer` 的 `bytes.Buffer` 和 Rust `SharedBuffer::with_capacity` 都以 `chunkSize` 预分配；两边的公开容量语义都来自单独保存的构造参数。
- Go `Bytes()` 借用底层切片；Rust `bytes()` 返回 `Vec<u8>` 克隆，以满足随后清空和 trait object 边界下的所有权要求，因此 Rust 多一次显式复制。
- Go 的嵌入指针让编码器和外层天然引用同一 buffer；Rust 用 `Arc<Mutex<Vec<u8>>>` 达成共享并处理内部可变性。
- Go `errors.Trace` 包装 `Write` 错误；Rust 保留原始 `io::Error`，没有附加调用栈语义。
- Go `NewWriter` 对未知/无压缩类型返回 nil，Rust 返回 `None`；直接对 nil/None writer 写、刷、关都会失败为 panic。对象写入层两种语言都在此前为无压缩选择 plain buffer。
- Rust 的 `bytes()` 是快照且 `cap()` 是声明容量，不能据此推断与 Go 的切片别名或 `bytes.Buffer.Cap()` 动态容量完全相同。上层 Rust 代码已针对压缩头部超过声明容量使用 `saturating_sub`。

仓库中未发现本目录下独立的 Go `buffer_test.go`；Go 行为依据 `buffer.go`、`writer.go` 与 `objectio/writer.go` 的直接实现核对，Rust 回归证据来自下节列出的独立测试文件。

## 扩展指南

- 新增压缩格式时，应先在 `def.rs::CompressType`、`writer.rs::new_writer` 及对应 Go 文件中建立一致映射；本文件通常无需算法分支，但必须验证新编码器的 flush、finish/close、错误和分块连续流语义。
- 若要让构造失败可恢复，最合适的入口是将 `new_buffer` 改为返回 `io::Result<Buffer>`（并同步 `new_writer` API 与所有调用者）；不要仅把 `expect` 改成静默丢数据。该变更属于跨文件 API 调整。
- 若改变 `reset`，必须保持“清除已上传输出但不破坏正在进行的压缩流”这一调用契约，并同步检查 `objectio::BufferedWriter::upload_chunk`。若目标改为每块独立压缩，则需要每块关闭并重建编码器，不能只清空 `Vec`。
- 若优化 `bytes()` 的复制，需要同时设计快照在 `reset` 后的所有权，避免底层上传仍读取已被复用或改写的内存；性能测试应关注块大小、压缩比、峰值内存和锁持有时间。
- 生产逻辑修改后，测试仍应放在独立文件，不嵌入 `buffer.rs`。至少同步 `pkg/objstore/compressedio/buffer_test.rs`（nil/None 边界）、`migration_aster_unit_test.rs`（三种格式往返与 reset）以及涉及分块链的 `pkg/objstore/objectio/migration_aster_unit_test.rs`。
- 保持 Go/Rust 对齐时还应复查 `pkg/objstore/compressedio/buffer.go`、`writer.go` 和 `pkg/objstore/objectio/writer.go`，尤其是错误传播和 close 时尾块上传顺序。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`node --file pkg/objstore/compressedio/buffer.rs` 覆盖源文件全部 129 行。
- RustCodeGraph 符号证据：`node new_buffer` 显示其调用 `SharedBuffer::with_capacity`，并被 `objectio::new_intercept_buffer`、`buffer_test.rs`、`migration_aster_unit_test.rs` 等调用；`node/callees new_intercept_buffer` 确认 `new_intercept_buffer → new_buffer`；`node new_buffered_writer` 确认 `new_buffered_writer → new_intercept_buffer` 以及对象写入测试调用者。
- 已读生产文件：`pkg/objstore/compressedio/buffer.rs`、`lib.rs`、`writer.rs`、`def.rs`、`Cargo.toml`，以及直接上游 `pkg/objstore/objectio/writer.rs`。
- 已读 Go 对照：`pkg/objstore/compressedio/buffer.go`、`writer.go`、`def.go`、`pkg/objstore/objectio/writer.go`。
- 已读独立测试：`pkg/objstore/compressedio/buffer_test.rs`、`migration_aster_unit_test.rs`，以及 `pkg/objstore/objectio/migration_aster_unit_test.rs` 的 Gzip 真实压缩与 close 尾部验证。
- 现有测试表达的边界：三种 Go 对应格式写入、flush/close 后均能解压还原；reset 清空可见输出并保留声明容量；NoCompression 的直接 write/flush/close 均 panic；对象写入链 close 后的 Gzip 数据包含可解压尾部。本任务是纯文档分析，按计划不运行 Cargo。

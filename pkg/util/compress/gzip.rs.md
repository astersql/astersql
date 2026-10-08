# `pkg/util/compress/gzip.rs`

## 文件定位

该文件是 `astersql-util-compress` crate 的 gzip 对象池与编解码封装实现。crate 入口 `pkg/util/compress/lib.rs` 通过 `pub mod gzip` 声明模块，并用 `pub use gzip::*` 将本文件的公开符号再导出。`pkg/util/compress/Cargo.toml` 将 crate 命名为 `astersql-util-compress`，只声明 `flate2 = "1"` 这一个直接依赖，并用 `package.metadata.porting.go-package = "pkg/util/compress"` 标记 Go 对照包。

当前已确认的生产上游是 `pkg/ingestor/ingestctrl/compress.rs`：其 `gzipCompressor::Do` 和 `gzipDecompressor::Do` 分别从两个全局池获取对象，用于 ingest 控制面的 gzip 压缩和解压。本文件不处理文件路径、网络协议协商或存储 I/O，只提供内存中的 gzip 字节转换和对象存取。

## 核心职责

1. `ReusablePool<T>` 使用 `Mutex<Vec<T>>` 保存已归还对象，在池空时调用函数指针工厂生成新对象，模仿 Go `sync.Pool` 的 `New`/`Get`/`Put` 使用形式。
2. `GzipWriter` 将一个完整的输入切片压缩为完整 gzip 字节向量。
3. `GzipReader` 持有可替换输入的 gzip decoder，并实现 `std::io::Read`，使调用方可用 `read`/`read_to_end` 消费解压结果。
4. `GzipWriterPool` 与 `GzipReaderPool` 通过 `LazyLock` 提供进程级延迟初始化的共享池。

对象池当前复用的是 `GzipWriter`/`GzipReader` 封装对象；`GzipWriter::compress` 每次都用新的 `GzEncoder<Vec<u8>>` 替换旧 encoder，因此不能将当前实现描述为复用 writer 的底层缓冲区或 deflate 状态。

## 主要符号

- `pub struct ReusablePool<T> { items: Mutex<Vec<T>>, new: fn() -> T }`：通用的进程内对象池。`items` 是后进先出容器，`new` 是池空时的同步工厂函数。
- `pub const fn ReusablePool::new(new: fn() -> T) -> Self`：构造空池，不预创建编解码器。
- `pub fn ReusablePool::get(&self) -> T`：锁定 `items`，弹出末尾对象；若没有可用对象，则调用 `new` 生成对象。锁中毒时使用 `PoisonError::into_inner` 继续操作原容器。
- `pub fn ReusablePool::put(&self, item: T)`：锁定并将对象推入 `Vec` 末尾；同样会在锁中毒后恢复内部容器。
- `pub struct GzipWriter`：内含 `GzEncoder<Vec<u8>>`。私有 `GzipWriter::new` 创建一个以空 `Vec` 为占位输出的默认压缩级 encoder。
- `pub fn GzipWriter::compress(&mut self, input: &[u8]) -> io::Result<Vec<u8>>`：重建 encoder，写入全部输入，用 `try_finish` 完成 gzip 尾部，再 clone encoder 的内部 `Vec`。
- `pub struct GzipReader`：以 `Option<GzDecoder<Cursor<Vec<u8>>>>` 表示“尚未绑定输入”或“正在解码”。私有 `GzipReader::new` 创建 `decoder: None` 的状态。
- `pub fn GzipReader::reset(&mut self, input: Vec<u8>)`：取得压缩字节所有权，以 `Cursor<Vec<u8>>` 包装，然后替换当前 decoder。
- `impl Read for GzipReader`：有 decoder 时直接转发到 `GzDecoder::read`；没有 decoder 时返回 `Ok(0)`，对 `Read` 调用者表示 EOF。
- `pub static GzipWriterPool` / `pub static GzipReaderPool`：分别以 `GzipWriter::new` 和 `GzipReader::new` 作为工厂的全局延迟初始化池。

文件中没有 trait 声明、宏、条件编译分支或模块级常量；仅有宽松 lint 属性 `dead_code`、`non_snake_case` 和 `non_upper_case_globals`。

## 执行流程

压缩路径以 `pkg/ingestor/ingestctrl/compress.rs::gzipCompressor::Do` 为例：

1. 调用 `GzipWriterPool.get()`；池中有对象则弹出，否则调用 `GzipWriter::new`。
2. 调用 `GzipWriter::compress(payload)`。该方法新建默认级别的 `GzEncoder<Vec<u8>>`，`write_all` 写完全部 payload，`try_finish` 写完 gzip 流，再 clone 出完整结果。
3. 上游在检查 `compress` 结果前将 writer 归还池，因此 Rust 生产调用路径在压缩报错时也会归还封装对象。
4. 成功时，上游用 `Write::write_all` 将完整 gzip 字节写入其目标 writer。

解压路径以 `gzipDecompressor::Do` 为例：

1. 上游先将输入 reader 全部读入 `Vec<u8>`，因此该链路不是端到端流式解压。
2. 通过 `GzipReaderPool.get()` 取得 reader，再用 `reset(compressed)` 替换旧 decoder 和旧输入。
3. `read_to_end` 反复调用本文件的 `Read::read`，而后者转发给 `flate2::read::GzDecoder`。
4. 上游保存读取结果，先将 reader 归还池，再返回成功数据或 I/O 错误；因此解压失败也不会跳过归还。

## 数据与状态

- 池状态是 `Vec<T>`，按 LIFO 顺序复用。代码没有容量上限、清理策略或 Go `sync.Pool` 那样的 GC 丢弃语义；每次 `put` 都会增加容器中的对象，直到后续 `get` 取走或进程结束。
- writer 的状态是一个 encoder。每次 `compress` 先替换它，所以上一次内容不会混入新 gzip 流；返回时对内部结果执行 clone，返回的 `Vec` 与池中 writer 后续的状态相互独立。
- reader 的初始状态是 `None`；`reset` 会丢弃之前 decoder 及其剩余输入，并拥有新 `Vec<u8>`。未 `reset` 就读取不会报“未初始化”，而是立即 EOF。
- API 以整个 `&[u8]` 压缩或整个 `Vec<u8>` 重置，上游生产解压器又会将解压结果读入新 `Vec`，所以大输入的峰值内存必须同时考虑压缩输入、decoder 持有的输入和解压输出。

## 依赖与调用关系

下游依赖如下：

- `flate2::write::GzEncoder<Vec<u8>>` 和 `flate2::Compression::default()` 负责 gzip 压缩。
- `flate2::read::GzDecoder<Cursor<Vec<u8>>>` 负责 gzip 解压。
- `std::io::{Read, Write}` 提供读写契约，`Cursor<Vec<u8>>` 将内存字节暴露为可读输入。
- `std::sync::{LazyLock, Mutex}` 分别管理全局初始化与池容器的并发访问。

上游调用边由 RustCodeGraph 文件/符号查询和精确引用搜索交叉确认：

- `pkg/ingestor/ingestctrl/compress.rs::gzipCompressor::Do` 调用 `GzipWriterPool.get` 和 `GzipWriter::compress`，然后调用 `GzipWriterPool.put`。
- `pkg/ingestor/ingestctrl/compress.rs::gzipDecompressor::Do` 调用 `GzipReaderPool.get`、`GzipReader::reset` 和 `Read::read_to_end`，然后调用 `GzipReaderPool.put`。
- `pkg/ingestor/ingestctrl/compress_test.rs` 直接使用两个池验证 ingest 压缩器/解压器；`pkg/util/compress/migration_aster_unit_test.rs` 是本 crate 的直接独立测试。

本文件与 `pkg/objstore/compressedio/writer.rs` 中同名的私有泛型 `GzipWriter<W>` 无调用关系；后者是 objstore 的另一套流式 writer，不应因名称相同而混入本模块的调用图。

## 错误处理与边界

- `GzipWriter::compress` 用 `?` 原样传播 `write_all` 和 `try_finish` 的 `io::Error`。当前输出是 `Vec<u8>`，但 API 仍保留通用 I/O 失败契约。
- `GzipReader::reset` 本身无返回错误；`GzDecoder::new` 也在这一层不校验并返回 gzip 头错误。无效头、损坏或截断输入的错误由后续 `Read::read`/`read_to_end` 传播。
- reader 未调用 `reset` 时返回 EOF，这会让 `read_to_end` 成功产生空结果；调用方必须自行保证先绑定输入。
- 池不检查重复 `put`、归还对象的状态或归还者身份；这些都是调用方约定。获取对象后若提前返回而没有 `put`，仅会失去复用机会，不会破坏其他池对象。
- 解码器类型是 `flate2::read::GzDecoder` 而非 `MultiGzDecoder`。本文件和直接测试未建立“连接的多个 gzip member 全部解码”契约；若扩展此行为，必须先增加独立回归测试，不能从当前类型选择推定已支持。

## 并发与资源生命周期

`LazyLock` 保证每个全局池在首次访问时完成一次初始化。`Mutex` 只保护短暂的 `Vec` 弹出/推入操作；`get` 返回所有权后，压缩或解压不持有池锁，不同线程可以并行使用各自取得的对象。若 mutex 因其他持锁线程 panic 而中毒，`get`/`put` 不会再 panic，而是取回内部容器继续工作；这表示实现接受容器内对象在 panic 后仍可被复用。

对象生命周期是“全局池 -> `get` 转移所有权 -> 调用方独占使用 -> `put` 转回池”。类型没有 RAII guard，因此自动归还不由本文件保证。已检查的 ingest Rust 调用方会在返回具体编解码错误前手动 `put`，但 panic 仍可跳过手动归还。池内对象与全局 static 同寿命，除非被后续 `get` 取出并丢弃。

## 与 Go 版本的对应关系

Go 基准文件是 `pkg/util/compress/gzip.go`。符号对应为：Go `GzipWriterPool` ↔ Rust `GzipWriterPool`，Go `GzipReaderPool` ↔ Rust `GzipReaderPool`，Go `sync.Pool.New` ↔ Rust `ReusablePool::new` 保存的工厂函数。Go writer 工厂返回 `gzip.NewWriter(io.Discard)`，Rust `GzipWriter::new` 则以空 `Vec` 创建占位 encoder；Go reader 工厂返回 `&gzip.Reader{}`，Rust 以 `decoder: None` 表示类似的未绑定状态。

实际使用语义不是逐项完全等价：

- Go `sync.Pool` 允许运行时在 GC 时丢弃缓存对象；Rust `Mutex<Vec<T>>` 会强持有所有已归还对象，且无上限。
- Go writer 由上游 `gzipCompressor::Do` 调用 `Reset(w)` 复用 writer，再 `Write` 和 `Close`；Rust `GzipWriter::compress` 每次重建 encoder，压缩到内存 `Vec`，之后由上游再写入目标。
- Go `gzip.Reader.Reset(r)` 返回 error，Go ingest 调用方可在开始 `ReadAll` 前处理无效 gzip 头；Rust `GzipReader::reset(Vec<u8>)` 无错误返回，解码错误延迟到读取时。
- Go ingest 解压器在成功 reset 后会在 defer 中 `Close` reader 再归还；Rust wrapper 没有显式 close 方法，`reset` 或 drop 时替换/释放 decoder。
- Go 包本身只定义两个池，实际的 `Reset`/`Write`/`Close` 流程在 `pkg/ingestor/ingestctrl/compress.go`；Rust 为了提供所有权友好的 API，在本文件内增加了 `GzipWriter`/`GzipReader` 封装和 `ReusablePool<T>` 实现。

对照测试是 `pkg/ingestor/ingestctrl/compress_test.go` 和 `pkg/ingestor/ingestctrl/compress_test.rs`：两者都验证 `"gzip"` 类型标识、压缩结果可解码以及解压往返一致。Rust crate 还有 `pkg/util/compress/migration_aster_unit_test.rs`，覆盖 gzip 魔数、包含空字节/非 ASCII 载荷的往返，以及归还后处理不同载荷。

## 扩展指南

- 要改变压缩级别或 encoder 策略，接入点是 `GzipWriter::new` 和 `GzipWriter::compress`。同步扩展 `pkg/util/compress/migration_aster_unit_test.rs`，并用 Go 可读的 gzip 流验证格式兼容；不应将测试内嵌到 `gzip.rs`。
- 要真正复用 writer 缓冲或减少结果 clone，需要重新设计 `GzipWriter` 的输出取回/重置边界，并验证失败后的对象是否可安全归还。这会影响 `pkg/ingestor/ingestctrl/compress.rs::gzipCompressor::Do`。
- 要支持流式输入或避免整个压缩输入的内存拷贝，需要改变 `GzipReader` 对 `Cursor<Vec<u8>>` 的固定持有方式，同时调整 ingest `gzipDecompressor::Do` 的“先读尽压缩输入”流程。
- 要改变无效头的报错时机、未 reset 读取语义或多 member 行为，应改动 `GzipReader::reset`/`Read::read` 并先在独立测试文件中增加无效、截断、空输入和连接 member 用例，同时检查 Go `gzip.Reader.Reset` 和 ingest 调用方的兼容期望。
- 要限制长寿命进程的池内存，接入点是 `ReusablePool::put`，可考虑容量上限或丢弃策略。需添加并发取还、超容量和 panic/锁中毒后行为测试，并明确与 Go `sync.Pool` 的语义差异。
- 任何公开 API 改动都要同步检查 `pkg/util/compress/lib.rs` 的全量再导出、`pkg/ingestor/ingestctrl/Cargo.toml` 的 crate 依赖和上述两个 Rust 独立测试文件。性能变更还应对照 `pkg/ingestor/ingestctrl/compress_test.go` 保留的 benchmark 意图。

## 验证依据

- RustCodeGraph `status`：项目索引可用，包含 7,032 个 Rust 文件；`files --filter pkg/util/compress` 确认本 crate 的 `gzip.rs`、`lib.rs`、`migration_aster_unit_test.rs` 和 Go 对照 `gzip.go`。
- RustCodeGraph `node --file pkg/util/compress/gzip.rs --offset 1 --limit 260`：核对全部 126 行源码、`ReusablePool<T>`、`GzipWriter`、`GzipReader`、`Read` 实现和两个全局池。
- RustCodeGraph `query GzipReader`、`query GzipWriter` 以及对 `pkg/ingestor/ingestctrl/compress.rs` / `compress_test.rs` 的 file node 查询：用于消除 objstore 同名类型歧义，并确认 ingest 上游的 get/compress/reset/read/put 调用链。图对 `LazyLock` static 的 `callers` 无法直接解析，因此按计划用精确 Rust 引用搜索补齐这两条边。
- 已读生产/装配/配置路径：`pkg/util/compress/gzip.rs`、`pkg/util/compress/lib.rs`、`pkg/util/compress/Cargo.toml`、`pkg/ingestor/ingestctrl/compress.rs`、`pkg/ingestor/ingestctrl/Cargo.toml`。
- 已读 Go 对照路径：`pkg/util/compress/gzip.go`、`pkg/ingestor/ingestctrl/compress.go`。
- 已读独立测试路径：`pkg/util/compress/migration_aster_unit_test.rs`、`pkg/ingestor/ingestctrl/compress_test.rs`、`pkg/ingestor/ingestctrl/compress_test.go`。它们提供往返、gzip 魔数、对象归还后再用和 ingest 协议类型标识的行为证据；本文档任务按明确范围未运行 Cargo 测试。

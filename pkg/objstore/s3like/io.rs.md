# [`pkg/objstore/s3like/io.rs`](./io.rs)

## 文件定位

本文件是 `astersql-objstore-s3like` crate 的流式 I/O 适配层，把 S3 兼容客户端返回的对象正文包装成 `objectio::Reader`，并把并发分片上传器包装成 `objectio::Writer`。模块入口 `pkg/objstore/s3like/lib.rs` 以 `#[path = "io.rs"] mod io_impl` 装入本文件并公开再导出，因此 `S3ObjectReader`、`AsyncWriter` 可由 crate 的 `Storage` 实现直接使用；它不是网络 SDK，也不解析 HTTP Range，本层所需的 `RangeInfo` 和底层 `Storage::open` 均由 `pkg/objstore/s3like/store.rs` 提供。

crate 边界由 `pkg/objstore/s3like/Cargo.toml` 定义：包名为 `astersql-objstore-s3like`，本文件直接使用其中声明的 `anyhow`、`objectio`、`os_pipe`、`prefetch`、`storeapi`，并通过 crate 内部接口依赖 `Storage`、`ReadCloser`、`Uploader`。在完整对象存储链路中，上游是 `Storage::Open` / `Storage::Create`，下游是 `PrefixClient::GetObject`、预取包装器、对象访问统计和具体 multipart uploader。

## 核心职责

1. `S3ObjectReader` 维护对象内绝对位置和原始 Range 边界，在标准 `Read + Seek` 接口之上提供失败重开、取消感知、读流量统计、短距离顺序跳过和长距离 Range 重开。
2. `S3ObjectReader` 将闭区间 `RangeInfo { Start, End, Size }` 转成每次最多可读的字节数，保证一次读取不越过当前 Range 的 `End`；对象末尾在 Rust `Read` 语义下表现为 `Ok(0)`。
3. `AsyncWriter` 用操作系统管道连接调用线程与后台上传线程：调用者向管道写端写入，后台线程把管道读端交给 `Uploader::Upload`，关闭时先发送 EOF，再等待上传结束并传播错误。
4. 两个适配器分别实现 `objectio::Reader` 与 `objectio::Writer`，使 S3-like 后端能够进入仓库统一的对象 I/O 缓冲、压缩和统计层；本文件本身不选择分片大小、并发度或写缓冲大小，这些决策在 `Storage::Create` 中完成。

## 主要符号

- `pub struct S3ObjectReader`：持有 `Arc<Storage>`、对象名、当前 `Box<dyn ReadCloser>`、逻辑位置 `pos`、`RangeInfo`、创建时的 `storeapi::Context` 和 `prefetchSize`。`storage` 使用 `Arc`，是为了让 reader 独立拥有可用于后续重开的存储句柄。
- `S3ObjectReader::new(...) -> Self`：crate 内构造器；将初始 `pos` 设为 `rangeInfo.Start`。生产调用点是 `Storage::Open`，测试也可在同 crate 内直接构造。
- `S3ObjectReader::reopen(&mut self)`：忽略旧流关闭错误，按当前 `pos` 和原 Range 的结束边界重新调用 `Storage::open`；当 `End + 1 == Size` 时把结束参数改为 `0`，遵循 `open` 中“0 表示读到 EOF”的约定。若启用了预取，用新响应的 `RangeSize()` 重新套 `prefetch::reader::NewReader`。
- `S3ObjectReader::discard_exact(&mut self, amount)`：用 8 KiB 栈缓冲反复调用自身 `Read` 丢弃指定字节；提前读到 `0` 时返回 `UnexpectedEof`。因为走的是自身 `Read`，跳过的数据同样参与重试、位置推进和访问统计。
- `impl Read for S3ObjectReader`：把请求裁剪到当前 Range 剩余长度；成功时记录读取字节并推进 `pos`；失败时在 context 未取消且本次调用重试数小于 `MAX_ERROR_RETRIES`（当前为 3）时记录可重试错误并重开。
- `struct EmptyBody(Cursor<Vec<u8>>)`：仅供文件内部使用的空 `ReadCloser`。seek 到或越过对象尾时替代真实正文，确保后续读取稳定返回 `0`，而不发送 S3 不接受的零长度 Range 请求。
- `impl Seek for S3ObjectReader`：处理 `Start`、`Current`、`End` 三类定位，拒绝 `Start(u64)` 无法转成 `i64` 和最终负偏移；相同位置不动作，尾部越界钳制到 `Size`，最多 64 KiB 的前向移动走丢弃读取，其余位置重新打开 Range。
- `impl objectio::Reader for S3ObjectReader`：`close` 转发到底层 `ReadCloser::close`，`file_size` 返回 `rangeInfo.Size`。
- `pub struct AsyncWriter`：拥有可取出的 `Option<os_pipe::PipeWriter>` 和 `Option<JoinHandle<anyhow::Result<()>>>`。`Option` 让关闭过程能取得所有权，并使第二次关闭不再重复 join。
- `AsyncWriter::new(ctx, uploader)`：创建 OS pipe，spawn 一个线程，在该线程内执行 `uploader.Upload(&ctx, &mut reader)`；context 和 uploader 一并移入线程。
- `impl objectio::Writer for AsyncWriter`：`write` 向 pipe 写端转发，关闭后写入返回 `BrokenPipe`；`close` 先 drop 写端使读端看到 EOF，再 join 后台线程，分别把线程 panic 和 uploader 的 `anyhow::Error` 映射为 `io::Error`。

## 执行流程

读取主流程如下：

1. `Storage::Open` 从 `ReaderOption` 取得起止偏移与预取大小，调用 `Storage::open` 发起首次 `GetObject` 并得到 `RangeInfo`，可选地先包装预取 reader，然后构造 `S3ObjectReader`。
2. 调用 `Read::read` 时，reader 用 `End + 1 - pos`（wrapping 算术与现有 Go `int64` 迁移语义一致）算出 Range 剩余量，并将本次输出切片限制在该长度内；剩余量为零直接返回 `Ok(0)`。
3. 底层读取成功后，`AccessStats::rec_read` 记录实际字节数，`pos` 同步增加。底层读取失败时，若 context 已取消或已进行 3 次重试，则记录一次 0 字节读取并返回该错误。
4. 可重试分支先调用 `RecordRetryableError`，再从当前 `pos` 重开。重开成功后继续同一次读取；重开失败时刻意返回原始读取错误，而不是用次生的 open 错误覆盖它。每次公开 `read` 调用的重试计数从零开始。
5. `seek` 先换算绝对偏移。相同位置直接返回；目标达到或超过 `Size` 时关闭当前流、换成 `EmptyBody` 并把位置钳制到 `Size`；向前且距离不超过 `MAX_SKIP_OFFSET_BY_READ`（64 KiB）时用 `discard_exact` 消耗现有连接；后退或远距离前进则关闭当前流，并以 `(realOffset, 0)` 重新 `Storage::open`。

写入主流程如下：

1. `Storage::Create` 在未给 option 或 `Concurrency <= 1` 时走客户端 `MultipartWriter`；只有 `Concurrency > 1` 才从 `PrefixClient::MultipartUploader` 取得 uploader 并调用 `AsyncWriter::new`。
2. `AsyncWriter::new` 建立 pipe 后立即启动一个后台线程。`Storage::Create` 再将它包进 `objectio::NewBufferedWriter`，缓冲大小取正数 `PartSize`，否则取默认 `WriteBufferSize`；因此多数小写入先由外层缓冲接收。
3. 外层缓冲向 `AsyncWriter::write` 刷数据时，pipe 提供有界的内核级背压；后台 uploader 同时从读端消费并完成具体分片上传。
4. `AsyncWriter::close` drop 写端，使 uploader 的读取循环看到 EOF，然后 join 线程。正常完成返回 `Ok(())`，上传错误传回调用者，线程 panic 转成消息为 `multipart uploader panicked` 的 `io::Error`。

## 数据与状态

`S3ObjectReader` 的关键不变量是：`pos` 表示对象内的绝对逻辑位置，而不是当前响应 body 内的相对位置；正常读成功和短距离 seek 的丢弃读取都推进它，Range 重开后则显式设置为目标位置。`rangeInfo.End` 是包含端点，`Size` 是完整对象大小，所以剩余量使用 `End + 1 - pos`；`Storage::open` 负责验证服务器实际返回的范围与请求一致。本文件使用 wrapping 加减保持既有迁移语义，相关溢出行为由 `migration_aster_unit_test.rs::seek_current_overflow_reports_go_wrapped_offset` 固定；但 `SeekFrom::Start(u64)` 到 `i64` 的转换单独做了溢出检查。

reader 保存的是创建时传入的 `storeapi::Context`，后续调用 `Read`/`Seek` 不再接收新 context。这个 context 的取消状态只控制“读错误后是否继续重试”；它不会在本文件内主动终止一次已经进入底层的同步读取或 open。`prefetchSize == 0` 表示关闭预取，大于零时首次打开、失败重开和 seek 重开都会建立新的预取包装器。

`AsyncWriter` 只保存 pipe 写端和后台线程句柄，不另设共享错误字段；上传结果由 `JoinHandle<Result<()>>` 独占传回，因此没有 Go 版本中 goroutine 写 `err`、关闭线程读 `err` 的共享可变状态。写入调用提供的 `objectio::Context` 被忽略，实际上传始终使用构造时捕获的 `storeapi::Context`。第一次 close 会消费两个 `Option`，第二次 close 返回成功；close 后 write 明确返回 `BrokenPipe`。

## 依赖与调用关系

上游生产调用边经 `pkg/objstore/s3like/store.rs` 核验：

- `Storage::Open -> Storage::open -> S3ObjectReader::new`。返回值被擦除为 `Box<dyn objectio::Reader>`，上层只依赖统一 `Read + Seek + close + file_size` 契约。
- `Storage::Create -> PrefixClient::MultipartUploader -> AsyncWriter::new`，仅用于 `WriterOption.Concurrency > 1`；其结果随后进入 `objectio::NewBufferedWriter`。
- `pkg/objstore/s3like/lib.rs` 再导出本文件符号，并在独立的 `io_test.rs`、`migration_aster_unit_test.rs` 中装配测试模块。

下游调用边为：`S3ObjectReader::reopen/seek -> Storage::open -> PrefixClient::GetObject`；`S3ObjectReader::read -> objectio::recording::AccessStats::rec_read` 和 `RecordRetryableError`；预取启用时三条打开路径都调用 `prefetch::reader::NewReader`；`AsyncWriter` 后台线程调用 `Uploader::Upload`。`Uploader` 在 `pkg/objstore/s3like/interface.rs` 中要求 `Send + Sync`，而线程对 uploader 取得所有权；`S3ObjectReader` 本身没有声明供多个线程同时共享的同步接口，应由持有者串行使用其可变 `Read/Seek` 方法。

RustCodeGraph 的文件查询确认 `pkg/objstore/s3like/io.rs` 已索引、共 21 个符号，并识别到 `S3ObjectReader` / `AsyncWriter` 及 Go 同名对应类型；其精确 Rust `impl` callers/callees 查询发生节点歧义，因此上述具体调用边进一步以 `store.rs`、`lib.rs` 和精确符号搜索核验，没有把图工具的模糊结果当作事实。

## 错误处理与边界

- 读到当前 Range 末尾返回 `Ok(0)`；这是 Rust `Read` 的 EOF 表达。单次读取绝不会向底层传入超过当前闭区间末端的输出切片。
- 所有底层 `read` 错误都进入同一重试判定；context 取消或三次重试耗尽后返回最后一次读错误。重开失败时返回触发重开的原读错误，`io_test.rs::read_preserves_original_error_when_reopen_fails` 明确锁定该行为。
- `reopen` 会忽略旧 reader 的 close 错误；seek 到/越过 EOF 也忽略关闭错误。相反，远距离或后退 seek 的显式 close 失败会立即返回，不再 open 新 Range。
- 短距离前向 seek 必须准确丢弃全部字节；若底层提前返回 EOF，`discard_exact` 返回 `UnexpectedEof`，由 `io_test.rs::short_seek_reports_eof_when_no_byte_was_discarded` 覆盖。
- 负目标偏移返回 `InvalidInput`；`SeekFrom::Start` 超过 `i64::MAX` 也返回 `InvalidInput`。目标大于 `Size` 不报错，而是钳制到文件尾；这一点与 Go 实现一致。
- `Storage::open` 或预取构造涉及的具体服务端 Range 合法性由 `store.rs` 负责。本文件在 seek 重开成功后用服务器返回的 `RangeInfo` 替换旧值；读错误重开则保持原始逻辑 Range，只使用新范围的大小配置预取。
- `AsyncWriter::new` 可能因 pipe 创建失败而返回 `io::Error`。后台 uploader 出错会使线程返回错误；读端随线程退出后，继续写 pipe 也可能先观察到 `BrokenPipe`。如果调用者能够进入 close，join 会传播 uploader 错误；线程 panic 有独立错误消息。
- 本文件没有为 reader 或 writer 实现 `Drop` 自动收尾。上层必须显式调用 `objectio::Reader::close` / `objectio::Writer::close`；尤其 writer 若只被 drop，虽然 pipe EOF 会让后台线程最终结束，但不会 join，也无法向调用者报告上传结果。

## 并发与资源生命周期

`S3ObjectReader` 的生命周期是“首次 `Storage::open` 获得 body—零到多次读/seek—显式 close”。失败重开会先尝试关闭旧 body，再替换为新 body；远距 seek 只有在旧 body 成功关闭后才建立新 body；EOF 占位会释放原网络 body 并持有一个纯内存空 cursor。`Arc<Storage>` 仅解决重开期间存储实例的所有权，不把 reader 变成可并发读写对象。

`AsyncWriter` 的生命周期是“创建 pipe—spawn uploader—写端持续供数—drop 写端发送 EOF—join uploader”。先 drop 写端再 join 的顺序是关键，否则 uploader 可能永远等待输入而 close 永远等待线程。OS pipe 不会把整对象无限堆积在内存中：生产者超过内核缓冲能力时会阻塞，形成背压；真正的 multipart 并行度和分片调度属于具体 `Uploader`，不由这个线程包装器实现。

构造时的 context 随后台闭包移动并至少存活到上传线程结束。`AsyncWriter::close` 同步等待线程，因此调用者在成功或错误返回后知道 uploader 已退出；但若不显式 close，则没有 join 保证。`S3ObjectReader` 的预取包装器可能自行管理缓冲资源，本文件通过替换/关闭 `ReadCloser` 控制其边界，未自行创建额外任务。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/objstore/s3like/io.go`，入口对照为同目录 `store.go`：

- Rust `S3ObjectReader` 对应 Go `s3ObjectReader`，字段 `storage/name/reader/pos/rangeInfo/ctx/prefetchSize` 一一对应；`read` 的三次重试、从当前偏移重开、尾端参数 `0`、64 KiB 内顺序丢弃、远距 Range Get、越界 seek 钳制到 `Size` 均保留。
- Go `Read` 用 `(0, io.EOF)` 表达末尾，Rust 按 `std::io::Read` 约定用 `Ok(0)`；Go 会显式排除 `io.EOF` 重试，Rust 的普通 EOF 不以错误出现。Rust 还显式拒绝 `SeekFrom::Start` 的 `u64 -> i64` 溢出，因为标准库接口类型不同。
- Go 的短 seek 用 `io.CopyN(io.Discard, r, delta)`；Rust 用 `discard_exact` 循环自身 `read`，保持同样的“必须丢弃足量，否则报 EOF”约束。
- Go `asyncWriter` 保存 pipe 两端、`WaitGroup`、uploader、共享错误和对象名，并由 `start` 启 goroutine；Rust `AsyncWriter::new` 直接 spawn 线程，只保留写端和 join handle。Rust 不需要单独的锁/等待组或共享错误槽，上传结果随 join 返回；日志中的文件名也不在本层保存。
- Go 上传失败会 `CloseWithError` 读端，使生产侧可能看到该错误，并在 `Close` 等待后返回 `s.err`；Rust 依靠 uploader 线程退出导致 pipe 断开，write 侧通常只得到 pipe 的 `BrokenPipe`，最终 uploader 的原始错误由 close/join 返回。两者的交付契约都要求 close 报告后台上传失败，`s3store` 的 Go/Rust `TestMultiUploadErrorNotOverwritten` / `test_multi_upload_error_not_overwritten` 分别验证这一点。
- Rust 独立测试还固定了迁移时新增的明确边界：重开失败不覆盖原读错误，以及 `SeekFrom::Current` 使用 wrapping 的 Go `int64` 对齐行为。

## 扩展指南

若要修改读取重试策略，优先改 `S3ObjectReader::read` 和 `reopen`，同时检查 `store.rs` 中 `MAX_ERROR_RETRIES`、`RecordRetryableError` 与 `Storage::open` 的 Range 校验。必须在独立测试文件中扩展 `pkg/objstore/s3like/io_test.rs` 或现有的 `pkg/objstore/s3store/s3_test.rs`；不要把测试嵌入 `io.rs`。至少保留“取消后不重试”“三次重试后返回错误”“重开失败保留原错误”“成功后下一次 read 的计数重新开始”四类契约。

若要调整 seek 策略，接入点是 `Seek for S3ObjectReader` 与 `discard_exact`。改变 64 KiB 阈值需同步评估请求次数、顺序读取成本、预取丢弃成本，并更新 `store.rs` 中常量和 `test_open_seek` 对 GetObject Range 调用的断言；新增偏移算法要继续处理负数、`u64 -> i64` 溢出、wrapping 的 Current/End 语义和 EOF 钳制。

若要改变异步上传的取消、错误或并发行为，接入点是 `AsyncWriter::new/write/close`，并应先确认 `Uploader::Upload` 的各实现和 `Storage::Create` 外层 `NewBufferedWriter` 的 flush/close 顺序。尤其要避免在 join 前保留 pipe 写端造成死锁。新增验证应放在独立测试中，覆盖 uploader 提前失败、线程 panic、close 两次、close 后 write、未写数据 close 以及 context 取消；同时对照 Go `asyncWriter` 是否需要同步行为变化。

性能风险主要来自把短 seek 阈值设得过大、重复创建预取器、pipe/外层缓冲组合造成额外复制或阻塞；兼容风险主要来自改变 EOF 表达、Range 端点约定、重试时返回哪个错误，以及 close 是否等待并传播上传失败。任何公开行为变化都应同步核对 `pkg/objstore/s3like/io.go` 和相应 Go 测试，不能为了 Rust 测试方便而简化 Go 逻辑。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用；`files --filter pkg/objstore/s3like/io.rs` 确认目标文件；`node --file pkg/objstore/s3like/io.rs --offset 1 --limit 360` 返回完整 266 行源码与 21 个符号；`query S3ObjectReader --kind struct`、`query AsyncWriter --kind struct` 同时定位 Rust 类型和 Go 对应类型。精确 `impl` callers/callees 查询存在节点歧义，故未用其模糊输出支撑具体调用结论。
- 源与模块：`pkg/objstore/s3like/io.rs`（全部类型、impl 和条件项；本文件没有条件编译项）、`pkg/objstore/s3like/lib.rs`（模块装入、公开再导出及独立测试装配）、`pkg/objstore/s3like/store.rs`（`Storage::Open`、`Storage::open`、`Storage::Create`、常量和 `RangeInfo`）、`pkg/objstore/s3like/interface.rs`（`Uploader` / `PrefixClient`）、`pkg/objstore/objectio/interface.rs`（统一 `Reader` / `Writer` 契约）。
- crate：`pkg/objstore/s3like/Cargo.toml` 核对包名、库入口和 `objectio`、`os_pipe`、`prefetch`、`storeapi` 等依赖；无 feature 开关声明。
- Rust 测试：`pkg/objstore/s3like/io_test.rs` 验证重开失败保留原读错误和短 seek 提前 EOF；`pkg/objstore/s3like/migration_aster_unit_test.rs` 验证 wrapping 偏移、读/seek/EOF 与同步/并发创建路径；`pkg/objstore/s3store/s3_test.rs` 验证慢读、短/长/后退/越界 seek、取消、重试上限和重置、并发上传错误传播。
- Go 对照：`pkg/objstore/s3like/io.go`、`pkg/objstore/s3like/store.go`；相关回归位于 `pkg/objstore/s3store/s3_test.go`，包括 `TestOpenSeek`、`TestS3RangeReaderRetryReadAndUnRetryableCase`、`TestS3ReaderWithRetryEOF`、`TestS3ReaderWithRetryFailed`、`TestS3ReaderResetRetry`、`TestMultiUploadErrorNotOverwritten`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务规定的 shell 命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核所有重要结论均能回溯到上述符号或文件。

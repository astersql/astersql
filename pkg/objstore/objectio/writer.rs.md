# `pkg/objstore/objectio/writer.rs`

## 文件定位

本文件属于 `astersql-objstore-objectio` crate。crate 入口 `pkg/objstore/objectio/lib.rs` 将本模块的公开项全部重导出，并同时重导出 `compressedio`、`CompressType`、`recording` 以及 `Context`/`Writer` 接口。因此，上层对象存储后端通过 `objectio::new_buffered_writer`、`objectio::new_uploader_writer` 或 Go 风格别名构造这里的写入包装器，而不直接依赖私有缓冲类型。

它位于“存储后端创建的底层 `Writer`”与“调用方连续写入的字节流”之间：把输入按目标块大小同步切分，可选地先压缩，再把已形成的块交给底层 `Writer::write`；最终由 `Writer::close` 刷出尾块并关闭底层对象。`pkg/objstore/objectio/interface.rs` 对 `Writer` 的契约也明确说明：一次 `write` 可能同步上传满块，`close` 负责末块和对象完成。

实际接线可见：

- `pkg/objstore/azblob.rs` 的 `Create` 用 `new_uploader_writer` 包装 `AzureBlockUploader`；块大小来自 `WriterOption::PartSize` 或 Azure 默认值。
- `pkg/objstore/gcs.rs` 的 `Create` 在并发度大于 1 时，用 `new_buffered_writer` 包装 multipart `GCSWriter`。
- `pkg/objstore/s3like/store.rs` 的 `Create` 始终用 `NewBufferedWriter` 包装同步 multipart writer 或异步 uploader，并传入访问统计。
- `pkg/objstore/compress.rs` 的 `CompressionStorage::Create` 在启用压缩时，用固定块大小和相应压缩类型包装下层 writer。

## 核心职责

1. 通过私有 `InterceptBuffer` 抹平明文 `PlainBuffer` 与 `compressedio::Buffer` 的差异，使分块算法只依赖写入、刷新、关闭、长度、容量、取字节、重置和“是否压缩”这些能力。
2. 在 `BufferedWriter::write_inner` 中把任意长度输入填入固定目标容量的缓冲；只有“当前缓冲长度 + 剩余输入长度”严格大于容量时才触发上传。
3. 在压缩模式下按“压缩后已产出的字节数”而非明文输入量判断块边界，并允许压缩器继续消费输入直到压缩缓冲需要上传。
4. 在 `BufferedWriter::close` 中结束压缩流、上传尾部字节，再关闭底层 writer，从而写出 gzip/zstd 等格式的结尾数据。
5. 可选地通过 `AccessStats::rec_write` 记录本层成功接受的明文字节数，而不是上传后的压缩字节数。
6. 提供 snake_case API 与 Go 风格兼容别名，方便 Rust 调用和逐步迁移既有 Go 接口。

本文件不负责创建远端 multipart 会话、并行调度分片、重试、取消判定或持久化协议；这些行为由传入的底层 `Writer` 及各存储后端实现。

## 主要符号

- `pub struct EmptyFlusher`：没有中间缓冲的 writer 可使用的空刷新器。`flush` 恒返回 `Ok(())`，`Flush` 只是 Go 风格转发。
- `trait InterceptBuffer`：模块私有的缓冲抽象。`write` 接受明文；`bytes` 返回当前输出快照；`compressed` 决定填满一次输入后是否重新计算压缩输出长度。
- `struct PlainBuffer { data, capacity }`：无压缩路径。`write` 将全部输入追加到 `Vec<u8>`，`flush`/`close` 为空操作，`bytes` 克隆数据，`reset` 只清空长度并保留分配。
- `impl InterceptBuffer for compressedio::Buffer`：把本地抽象逐项转发到压缩 crate。其 `len` 是已产出的压缩字节数，`cap` 是配置的目标容量，`close` 会让编码器写出格式尾部。
- `fn new_intercept_buffer(chunk_size, compress_type)`：`NoCompression` 选择 `PlainBuffer`，其余类型调用 `compressedio::new_buffer`。
- `pub struct BufferedWriter`：持有唯一的缓冲 `buf`、底层 `Box<dyn Writer>` 和可选共享统计 `Option<Arc<AccessStats>>`。三个字段均为私有。
- `BufferedWriter::write_inner`：分块状态机的核心，返回本次调用接受的输入字节数。
- `BufferedWriter::upload_chunk`：空缓冲直接成功；否则复制输出、先重置缓冲，再调用底层 `Writer::write`。
- `BufferedWriter::get_writer` / `GetWriter`：只读暴露底层 `&dyn Writer`，不允许绕过包装器可变写入。
- `impl Writer for BufferedWriter`：公开运行时入口。`write` 调用 `write_inner` 并记流量；`close` 结束缓冲、上传尾块、关闭底层 writer。
- `new_uploader_writer` / `NewUploaderWriter`：不带统计地构造并擦除为 `Box<dyn Writer>`。
- `new_buffered_writer` / `NewBufferedWriter`：保留具体 `BufferedWriter` 类型并允许传入统计对象。

本文件没有模块级常量、枚举、泛型或条件编译项。

## 执行流程

构造流程如下：

1. 上游先创建一个实现 `objectio::Writer` 的实际存储 writer。
2. `new_buffered_writer` 调用 `new_intercept_buffer`。无压缩时分配容量为 `chunk_size` 的 `Vec`；压缩时创建持有 gzip、Snappy 或 zstd 编码器的 `compressedio::Buffer`。
3. 工厂把缓冲、底层 writer 和可选 `Arc<AccessStats>` 组装进 `BufferedWriter`；`new_uploader_writer` 再将其装箱为 trait object。

一次 `write(ctx, data)` 的流程如下：

1. `write_inner` 保存 `written_total = 0`，然后检查 `buf.len() + data.len() > buf.cap()`。
2. 若剩余数据不能全部容纳，计算 `to_fill = cap.saturating_sub(len)`。使用饱和减法是为了处理压缩器头部已经使输出长度大于配置容量的情况，避免无符号下溢。
3. `to_fill > 0` 时，只把恰好可填充的输入片段交给缓冲，并按缓冲实际接受量推进计数和输入切片。
4. 若是压缩缓冲，立即回到循环顶部重新观察压缩后输出长度；压缩率可能使输出仍小于容量，不能像明文一样假定已经填满。
5. 需要上传时调用缓冲 `flush`，但刻意忽略该返回值；随后 `upload_chunk` 复制当前输出、清空缓冲并同步调用底层 `Writer::write(ctx, chunk)`。
6. 循环结束后，把所有剩余输入写入缓冲并返回本次接受的明文字节总数。
7. 外层 `Writer::write` 仅在 `write_inner` 成功时把返回值计入 `AccessStats`；任何错误按 0 计数，然后原样返回结果。

`close(ctx)` 的顺序是：忽略 `buf.close()` 的结果，让压缩器尽力写出尾部；调用 `upload_chunk` 上传全部尾部输出；只有上传成功才调用底层 `writer.close(ctx)`。因此尾块上传错误会短路底层关闭，而底层关闭错误会在尾块已上传后返回。

严格大于比较带来一个重要不变量：输入恰好填满缓冲时不会立即上传，而是等下一次造成溢出的写入或 `close`。`migration_aster_unit_test.rs::exact_capacity_waits_until_close_like_go` 固定了这一行为。

## 数据与状态

`BufferedWriter` 的可变状态只有缓冲内容和底层 writer 自身状态；没有显式的“已关闭”“上传中”或块序号字段。明文路径中，`PlainBuffer::capacity` 是逻辑容量，`Vec` 实际上仍可增长，但分块循环保证正常正容量配置下不会把明文缓冲写过该边界。压缩路径中，容量是上传目标而非硬内存上限：压缩头或一次编码输出可能暂时超过它，代码会在下一轮先上传现有输出。

`upload_chunk` 调用 `bytes()` 得到拥有所有权的 `Vec<u8>` 快照，然后在底层写入前执行 `reset()`。这避免上传期间继续占用缓冲内容，也让下一块复用既有分配；代价是底层写入失败后，本层不保留失败块，不能由同一个 `BufferedWriter` 自动重试。重试或中止策略必须由底层 writer 或更上层负责。

统计对象用 `Arc<AccessStats>` 共享，其中写流量由 `AtomicU64` 以 `Relaxed` 顺序累加。统计值代表成功完成本层 `write` 调用时接受的原始输入量：不等于压缩输出大小，也不在 `close` 时为压缩尾部额外计数。若一次调用先接受了部分数据、随后上传失败，`write_inner` 只能返回 `Err`，外层会记 0；这与 Go 在 `uploadChunk` 失败分支返回 0 的可观察语义一致。

`chunk_size` 没有在本文件内校验。调用方必须传入正数；对于非空输入和零容量，分块循环无法消费输入，也没有非空块可上传，会持续循环。现有后端调用点均从正的默认块大小、最小块大小或有效的 `PartSize` 形成容量，但新增调用者不能依赖构造器替自己验证。

## 依赖与调用关系

crate 边界由 `pkg/objstore/objectio/Cargo.toml` 定义：生产依赖中，本文件直接使用 `astersql-objstore-compressedio` 与 `astersql-objstore-recording`；`Context` 和 `Writer` 来自同 crate 的 `interface.rs`。`http` 和 `tokio` 属于 crate 其他接口所需，不是本文件的直接依赖。压缩往返测试所需的 `flate2`、`snap`、`zstd` 等位于 dev-dependencies。

主要内部调用边为：

```text
new_uploader_writer
  -> new_buffered_writer
     -> new_intercept_buffer
        -> PlainBuffer | compressedio::new_buffer

Writer::write for BufferedWriter
  -> write_inner
     -> InterceptBuffer::write/flush
     -> upload_chunk
        -> InterceptBuffer::bytes/reset
        -> underlying Writer::write
  -> AccessStats::rec_write

Writer::close for BufferedWriter
  -> InterceptBuffer::close
  -> upload_chunk
  -> underlying Writer::close
```

RustCodeGraph 将 `writer.rs` 标为至少被 `pkg/objstore/azblob.rs`、`pkg/objstore/compress.rs`、`pkg/objstore/gcs.rs`、`pkg/objstore/objectio/migration_aster_unit_test.rs` 和 `writer_test.rs` 等文件使用；精确路径搜索还确认 `pkg/objstore/s3like/store.rs` 使用 `NewBufferedWriter`。这些上游决定对象路径、multipart 策略和并发度，本文件只以同步 `Writer` 调用把分块交给它们。

下游压缩语义来自 `pkg/objstore/compressedio/buffer.rs` 和 `writer.rs`：`Buffer` 通过共享字节区收集编码器输出，`flush` 推出中间数据，`close` 为 gzip/zstd 写出结束标记。访问统计实现位于 `pkg/objstore/recording/recording.rs::AccessStats::rec_write`。

## 错误处理与边界

- `PlainBuffer::write` 总是接受完整切片；压缩缓冲的写错误通过 `?` 原样向上传播，并保留此前已累计的局部计数只供函数内部使用。公开返回类型为 `io::Result<usize>`，出错时不会把部分计数交给调用者。
- 中间分块处的 `buf.flush()` 错误被忽略；`close` 中的 `buf.close()` 错误也被忽略。这是对 Go `writer.go` 的显式保持：最终可见错误由随后上传或底层关闭路径报告。新增压缩实现若只在 flush/close 返回错误而未使后续上传失败，错误会被吞掉。
- `upload_chunk` 对空缓冲是 no-op；非空时忽略底层 `write` 返回的字节数，只检查错误。因此底层 writer 必须遵守“无错误即完整消费块”的约定；返回短写且无错误会造成静默数据丢失。
- 尾块上传失败时不调用底层 `close`，由 `migration_aster_unit_test.rs::upload_failure_is_returned_and_does_not_close_sink` 覆盖。尾块上传成功、底层关闭失败时，关闭错误原样返回，由 `underlying_close_failure_is_returned_after_tail_upload` 覆盖。
- 本层不调用 `Context::check`。只在实际上传和关闭时把同一 `Context` 传给底层；纯缓冲写即使 context 已取消也可成功。两个取消测试固定了“透传而非提前拒绝”的契约。
- 空输入不会触发上传。恰好满容量也不会立即上传。零容量对非空输入不安全；超大长度相加理论上还受 `usize` 溢出规则约束，本文件没有显式防护。
- 没有显式幂等关闭保证，也没有阻止关闭后继续写的统一状态；具体结果取决于明文/压缩缓冲和底层 writer。调用方应遵守一次完成生命周期：若干次 `write`，随后一次 `close`。

## 并发与资源生命周期

`Writer::write` 和 `Writer::close` 都要求 `&mut self`，所以一个 `BufferedWriter` 的分块状态天然按调用顺序串行修改；类型没有提供供多个任务并发调用同一实例的内部调度。每次满块上传发生在调用线程/任务上，并在底层 `Writer::write` 返回前阻塞本次写入。GCS、Azure、S3-like 底层 writer 可以自行并行上传，但那是包装层以下的实现细节。

压缩 `Buffer` 的底层字节区使用 `Arc<Mutex<Vec<u8>>>`，是因为编码器与缓冲对象同时持有该容器；本文件仍通过独占 `&mut` 顺序驱动编码器。`PlainBuffer` 不加锁。唯一面向跨线程共享的本文件字段是 `Arc<AccessStats>`，其计数器为原子类型。

资源生命周期为“构造缓冲与底层 writer → 多次写入/可能上传 → 关闭压缩器 → 上传尾块 → 关闭底层 writer”。`BufferedWriter` 没有 `Drop` 实现；若调用方丢弃它而不调用 `close`，缓冲尾块、压缩格式尾部以及底层 multipart 完成动作都没有保证执行。上传失败时缓冲已经重置，且底层不会由本层自动关闭或 abort；调用方应把错误视为该 writer 生命周期失败。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/objectio/writer.go`。主要映射如下：

- Go `interceptBuffer` 对应 Rust `InterceptBuffer`；Go `plainBuffer` 嵌入 `bytes.Buffer`，Rust `PlainBuffer` 用 `Vec<u8>` 和独立逻辑容量表达同样行为。
- Go `BufferedWriter.Write` / `write0` / `uploadChunk` / `Close` / `GetWriter` 分别对应 Rust 的 `Writer::write` / `write_inner` / `upload_chunk` / `Writer::close` / `get_writer`。
- Go `NewUploaderWriter` 与 `NewBufferedWriter` 对应同名 Rust snake_case 工厂及 Go 风格别名。
- 两版都只在“当前长度 + 输入长度严格大于容量”时上传；都在压缩路径写入一段后 `continue`，重新按压缩输出长度判断；都忽略中间 flush 与关闭压缩缓冲的错误；都在上传前重置缓冲；都只在尾块上传成功后关闭底层 writer。
- Go 用可为 nil 的 `*recording.AccessStats`，Rust 用 `Option<Arc<AccessStats>>`；`AccessStats::rec_write` 对 `None` 为 no-op。Rust 失败时取 0 计数，对齐 Go 上传失败返回 0 的主要路径。
- Rust 特有的 `saturating_sub` 处理“压缩器头部输出大于配置容量”的情况，避免 Go 整数减法机械翻译成 `usize` 后下溢。这不改变正常容量下的分块意图。
- Rust 公开工厂接收 `Box<dyn Writer>`，反映所有权；`get_writer` 只返回共享借用。Go 接口值没有相同的静态所有权限制。
- Rust 保留 `New*`、`GetWriter`、`Flush` 别名以兼容迁移中的 Go 命名，同时提供惯用 snake_case API。

`pkg/objstore/objectio/writer_test.go` 与 `writer_test.rs` 都通过本地存储验证明文完整写入以及 Gzip/Snappy/Zstd 往返；Rust 额外的 `migration_aster_unit_test.rs` 直接固定细粒度分块、错误、统计和 context 透传语义。

## 扩展指南

新增压缩类型时，应先在 `pkg/objstore/compressedio` 完成 `CompressType`、编码器及 `Buffer` 行为，再确认 `new_intercept_buffer` 的“仅 `NoCompression` 走明文，其余走压缩缓冲”仍成立。必须新增独立测试文件中的往返与尾帧测试，尤其验证 `flush`、`close`、`bytes`、`reset` 和容量超过边界时不会死循环或丢数据。

调整分块策略时，最可能修改 `write_inner` 和 `upload_chunk`。需要同步覆盖：小于容量、恰好容量、跨一个/多个块、压缩后小于或大于目标容量、空写、底层短写、上传失败以及 close 尾块。不能把 Rust 测试内嵌进 `writer.rs`；现有最近单元测试位置是 `pkg/objstore/objectio/migration_aster_unit_test.rs`，端到端对应测试是 `writer_test.rs`，Go 对照测试是 `writer_test.go`。

若要增加取消检查，必须先决定是否有意改变 Go 兼容语义。当前 contract 是缓冲阶段不检查，真正上传和关闭时交给底层判断；在本层提前 `ctx.check()` 会让已取消 context 下的纯内存写从成功变为失败。

若要支持可靠重试，应首先改变“上传前 reset”或定义可重放块的所有权，同时明确底层短写处理、重复上传和 multipart abort。仅在错误后再次调用现有 writer 不能保证恢复丢失块。

若要接受外部可控块大小，应在构造边界验证 `chunk_size > 0`，并检查 Azure、GCS、S3/KS3 的最小分片要求。修改公开构造签名还需同步 `lib.rs` 重导出使用者以及上述后端调用点。性能评估应关注 `bytes()` 每块克隆、同步上传造成的背压、压缩输出短时越过容量，以及块大小对内存和 multipart 请求数的权衡。

## 验证依据

本说明基于以下直接证据：

- 目标实现：`pkg/objstore/objectio/writer.rs`；RustCodeGraph 显示 268 行、42 个索引符号，并给出该文件被 Azure、压缩包装、GCS 和相关测试等文件使用。
- 接口与模块边界：`pkg/objstore/objectio/interface.rs`、`lib.rs`、`Cargo.toml`。crate 名为 `astersql-objstore-objectio`，直接生产依赖包括 compressedio 与 recording 两个本地 crate。
- 下游实现：`pkg/objstore/compressedio/buffer.rs`、`pkg/objstore/compressedio/writer.rs`、`pkg/objstore/recording/recording.rs`。
- Rust 上游调用点：`pkg/objstore/azblob.rs::Create`、`pkg/objstore/gcs.rs::Create`、`pkg/objstore/s3like/store.rs::Create`、`pkg/objstore/compress.rs::CompressionStorage::Create`。
- Go 对照：`pkg/objstore/objectio/writer.go`；调用点搜索还核对了 Azure、GCS、S3-like、KS3 和压缩包装的 Go 接线。
- 独立测试：`pkg/objstore/objectio/migration_aster_unit_test.rs` 覆盖分块、精确容量、上传和关闭错误、取消 context、统计与 Gzip 尾部；`writer_test.rs` 及 `writer_test.go` 覆盖本地对象完整写入和三种压缩格式往返。
- RustCodeGraph 查询包括 `status`、目标目录 `files`、目标文件与上述接口/测试/依赖文件的 `node --file`、以及 `BufferedWriter`、`new_buffered_writer`、`new_uploader_writer`、`write_inner`、`upload_chunk`、`rec_write` 的 `query`。常见方法名的全局 callers/callees 结果存在同名噪声，因此跨文件上游边以精确路径搜索和对应调用点源码复核为准。

本任务是纯文档分析，按计划不运行 Cargo。结构验收应确认该文件存在，并且恰好包含任务要求的十一个固定二级标题；人工复核重点是调用链、Go 差异、错误丢块边界和安全扩展位置均能回溯到上述源码与测试。

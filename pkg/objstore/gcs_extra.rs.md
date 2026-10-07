# `pkg/objstore/gcs_extra.rs`

## 文件定位

本文件属于 `astersql-objstore` crate，由 [`pkg/objstore/lib.rs`](lib.rs) 以 `pub mod gcs_extra` 公开。它是 GCS 后端的分片上传辅助层：将同步的 `objectio::Writer` 接口桥接到 `object_store::MultipartUpload` 的异步 API，并保留一组与 Go XML multipart 实现对应的数据结构和 HTTP 传输默认值。真正的存储后端选择、路径组装和 `Storage::Create` 入口在 [`pkg/objstore/gcs.rs`](gcs.rs) 中。

crate 边界由 [`pkg/objstore/Cargo.toml`](Cargo.toml) 定义：本文件直接使用 `anyhow`、`bytes`、带 `gcp` feature 的 `object_store`、本地 `objectio` 与 `storeapi` crate，并通过 `ObjectStorageCore` 复用 `azblob.rs` 中的专用 Tokio runtime。

## 核心职责

- `GCSWriter` 在构造时立即创建 multipart upload，每次 `write` 将收到的完整字节片作为一个 part 上传，`close` 则完成或撤销该 upload。
- 守住 GCS multipart 约束：part size 必须在 5 MiB 至 5 GiB 之间，worker 数必须为正，part 数不得超过 `storeapi::MaxUploadParts`。
- 保留首个分片错误，使后续 `write` 和 `close` 稳定返回失败；任一分片或 finalize 失败时尝试 abort，避免未完成分片持续占用资源/计费。
- 提供 Go 对照所需的 multipart 响应/请求 DTO 与 transport 配置值；这些类型目前没有接入 Rust 的 `GCSWriter` 主流程。

## 主要符号

- `GCS_MINIMUM_CHUNK_SIZE` / `GCS_MAXIMUM_CHUNK_SIZE`：5 MiB 和 5 GiB 的边界；`new_with_attributes` 以包含上下界的区间检查。
- `GCS_MAXIMUM_PARTS`：`storeapi::MaxUploadParts` 的本地别名；`current_part` 从 1 开始，因而允许 1..=10,000，第 10,001 片返回 `ErrExceedMaxUploadParts`。
- `DEFAULT_RETRY` / `DEFAULT_SIGNED_URL_EXPIRY`：分别对应 Go 的 3 次重试与 6 小时签名 URL 有效期；当前 Rust 上传路径未引用这两个常量。
- `GCSWriter`：核心状态机。`context` 是构造时保留的取消上下文，`core` 持有 object store 与 runtime，`upload` 的 `Option` 表示 multipart handle 是否仍可用，`current_part`/`total_size` 记录进度，`closed` 保证关闭幂等，`stage_error` 缓存首个上传错误。
- `GCSWriter::new` / `new_with_attributes`：公开构造器。后者校验参数，构造 `ObjectStorageCore`，再用 `put_multipart_opts` 创建 upload；`new` 仅传入空 `Attributes`。
- `GCSWriter::upload_part`：内部单片上传入口。它先检查 part 上限与构造时 context，再复制数据到 `Bytes`、同步等待 `put_part`，只在成功后累加编号与字节数。
- `GCSWriter::finish`：内部关闭状态机；已关闭时直接成功，有暂存错误时 abort，零 part 时不 complete 也不 abort，否则检查两个 context 并 complete。
- `impl objectio::Writer for GCSWriter`：对外暴露 `write` / `close`。`write` 将 `anyhow` 错误折叠为 `io::Error`；`close` 用 `MultipartCloseError` 保留完整 error source 链。
- `impl Drop for GCSWriter`：未显式关闭且仍有 upload handle 时执行最佳努力 abort，abort 错误不能从 `drop` 传出。
- `InitiateMultipartUploadResult`、`Part`、`CompleteMultipartUpload`：Go XML API 模型的 Rust 数据形状；`sort_parts` 按 `part_number` 稳定升序排列完成请求。当前主流程交由 `object_store` 内部管理 part 结果，不直接使用这三个类型。
- `TransportConfig` / `create_transport`：表达 Go `http.Transport` 的超时和连接池默认值；返回值是配置摘要，并不创建 Rust HTTP client。
- `wrap_upload_for_test`：仅在 `cfg(test)` 下存在，用故障注入 wrapper 替换活跃 upload handle。

## 执行流程

1. `GCSStorage::Create` 首先检查调用 context。当 `WriterOption::Concurrency > 1` 时，它把 `PartSize` 至少提升到 5 MiB，传入对象路径、并发度和 storage-class 属性创建 `GCSWriter`，再用 `objectio::new_buffered_writer` 按 part size 切块。并发度不大于 1 时使用非 multipart 的 `ObjectStoreWriter`。
2. `new_with_attributes` 同步创建两 worker 线程的 Tokio runtime，然后 `block_on(ObjectStore::put_multipart_opts)` 初始化 multipart session。初始化失败则不返回 writer。
3. 外层 buffered writer 把每个完整块交给 `GCSWriter::write`。`write` 先拒绝已关闭 writer 或重放 `stage_error`，再检查本次调用 context，最后进入 `upload_part`。
4. `upload_part` 检查 part 数与构造时 context，将输入复制为拥有所有权的 `Bytes`，并在专用 runtime 上阻塞等待 `put_part`。成功后才更新 `current_part` 和 `total_size`；失败则由 `write` 存入 `stage_error`。
5. `close` 调用 `finish`并取走 upload handle。若没有上传过 part，按 Go 语义直接成功；若有 part，两个 context 均未取消时才调用 `complete`。
6. 暂存错误、context 取消或 `complete` 失败都进入 abort。abort 成功时返回原错误；abort 也失败时在原错误上追加清理失败上下文。最后置 `closed = true`，使重复 `close` 幂等。
7. 若调用者没有 `close`，`Drop` 会尝试 abort 仍在 `upload` 字段中的 session。

## 数据与状态

`GCSWriter` 的关键不变式是：`current_part` 始于 1，只有 part 上传成功才递增；`total_size` 同样只统计成功的输入。`upload: Some(_)` 表示尚可 complete/abort，`finish` 一开始就 `take`，防止重复终结。`closed` 是对外的终态标记；一旦置位，`close` 幂等成功，`write` 返回 `BrokenPipe`。

`stage_error: Option<Arc<io::Error>>` 使同一失败能被后续调用共享，不会再发起远程上传。`part_size` 和 `workers` 保留已校验配置并通过 getter 可观测；本文件内不使用 `part_size` 自行切块，也不根据 `workers` 调度并行上传，切块由 `gcs.rs` 的 buffered writer 完成。

`Part` 和 `CompleteMultipartUpload` 是纯内存 DTO；`sort_parts` 只改变 `parts` 的顺序。`TransportConfig` 也是值对象，`max_idle_connections_per_host` 在每次构造时由当前可用并行度加 1 计算，查询失败则以 1 个 CPU 为基数，结果为 2。

## 依赖与调用关系

上游主调用边是 `GCSStorage::Create -> GCSWriter::new_with_attributes -> ObjectStore::put_multipart_opts`（`gcs.rs:442-466`）。返回的 writer 被 `objectio::new_buffered_writer` 包装，最终以 `Box<dyn objectio::Writer>` 经 `storeapi::Storage` 边界交给上层。访问记录启用时，还会再被 `RecordingWriter` 包装。

下游依赖包括：

- `objectio::Context::check`：在写入和完成前观测取消；清理 abort 刻意不受已取消 context 阻断。
- `ObjectStorageCore`：来自 `azblob.rs:330-349`，持有 `Arc<dyn ObjectStore>` 和两工作线程 Tokio runtime，让同步 writer 可用 `block_on` 执行 multipart future。
- `object_store::MultipartUpload`：提供 `put_part`、`complete`和 `abort`；远程协议、part ETag 与完成请求由该依赖封装。
- `storeapi::MaxUploadParts` / `ErrExceedMaxUploadParts`：统一对象存储层的 part 数上限与错误身份。

RustCodeGraph 将目标文件索引为 37 个符号；对 `GCSWriter`、`create_transport` 和 `sort_parts` 的通用 callers/callees 查询未返回可用边，因而上述精确边又用符号引用搜索与源文件行为复核。`InitiateMultipartUploadResult`、`Part`、`CompleteMultipartUpload`、`sort_parts`、`TransportConfig`、`create_transport`、`DEFAULT_RETRY` 和 `DEFAULT_SIGNED_URL_EXPIRY` 在当前 Rust 仓库中没有目标文件外的精确引用。

## 错误处理与边界

构造阶段会拒绝越界 part size 和零 worker，并为 runtime 创建、multipart 初始化增加上下文。这些失败发生在 writer 返回之前。

写入阶段的边界依次是：已关闭、既往 staging 错误、调用方 context 取消、part 数超限、构造时 context 取消、upload handle 缺失、`put_part` 失败。首个这类运行时失败被保存在 `stage_error`，保证不会部分恢复后继续上传。

关闭阶段保留主错误优先级：abort 失败只作为附加 context，`MultipartCloseError::source` 仍暴露原 `anyhow::Error` 链。零 part 关闭是特例：不调用 complete 或 abort，与 Go `Close` 中的空列表语义一致。`Drop` 是安全网而非可观测接口，因此无法报告 abort 失败；对计费/清理有严格要求的调用者必须显式调用 `close`并处理错误。

## 并发与资源生命周期

`GCSWriter` 是可变借用的同步 writer，接口本身不允许同一实例的并发 `write`。虽然保存 `workers` 且只在上层 `Concurrency > 1` 时启用，当前每次 `write` 都立即 `block_on(put_part)`，所以 part 在该 writer 内串行提交，`workers` 不是实际调度器。专用 Tokio runtime 有两个 worker 线程，但这不等于同时上传多个 part。

multipart session 的所有权由 `upload: Option<Box<dyn MultipartUpload>>` 管理。正常路径为初始化→多次 put→complete；失败路径为初始化→部分 put/取消/finalize 失败→abort；遗忘关闭则为初始化→`Drop` 最佳努力 abort。`finish` 在远程操作前取走 handle，并在所有分支后设置 `closed`，避免重复 complete/abort。

取消上下文有两层：构造时 `self.context` 与每次 `write`/`close` 传入的 `ctx`。上传前两者的相应检查都可阻止 I/O；一旦需要清理，abort 直接使用 upload handle，不再被已取消 context 短路。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/objstore/gcs_extra.go`](gcs_extra.go)。两版共同保留 5 MiB/5 GiB 分片范围、最多 10,000 parts、零 part 关闭成功、失败时 abort、原错误与 abort 错误同时可见，以及 30s connect/keep-alive、90s idle、10s TLS handshake、1s expect-continue、100 全局 idle 连接和 `CPU + 1` 每主机 idle 连接的默认意图。

实现机制存在重要差异：

- Go 直接实现 GCS XML multipart：生成 V4 signed URL，发送 POST/PUT/DELETE，解析 upload ID 和 ETag，排序 part 后序列化 XML。Rust 把这些协议细节委托给 `object_store` crate。
- Go 创建 `workers` 个 goroutine，通过 channel 并行上传，每个 `xmlMPUPart` 最多尝试 3 次；Rust 保存并校验 `workers`，但当前串行上传，也没有在本层使用 `DEFAULT_RETRY`。底层 `object_store` 是否重试属于其自身配置，本文件没有证据可将其视为 Go 的等价三次重试。
- Go `Write` 只复制数据并入队，worker 异步记录错误；Rust `write` 在返回前等待当前 part 上传结束，因而错误更早向调用者暴露。
- Go DTO 带 XML tag 并直接参与协议；Rust 同名 DTO 不带 serde/XML 标注，当前仅是兼容形状。Rust `create_transport` 也只返回参数摘要，没有对应 Go 的 `localAddr`、环境代理和真实 dialer/transport 对象。
- Go 对 worker 数未在构造器显式拒绝 0；Rust 显式返回 `parallel worker count must be positive`，避免一个无消费者的配置。

因此，本文件是“保持对外约束与清理语义、替换内部协议实现”的移植，不能将其视为 Go 并行度和重试行为的逐行等价实现。

## 扩展指南

- 改变 multipart 初始化、对象属性或 GCS 创建策略时，同时检查 `GCSWriter::new_with_attributes` 和 `GCSStorage::Create`；不要在 writer 内重复外层 buffered writer 的切块职责。
- 实现真正并行 part 上传时，需围绕 `workers`、part 编号/完成顺序、首错误竞态、context 取消、内存上限与 close 等待设计明确状态机；应以 Go `readChunk`/`appendMPUPart`/`Close` 的语义为对照，但不应绕过 `object_store::MultipartUpload` 的契约。
- 增加重试时，必须明确由本层还是 `object_store` 配置负责，避免双层重试放大流量；同时定义哪些错误可重试、重试是否保持 part number 幂等。
- 修改 `finish` 或 `Drop` 时，必须保持“错误后 abort”、“取消不阻断 abort”、“主错误 source 链不丢失”和“终结操作最多一次”。需同步扩展 [`pkg/objstore/gcs_test.rs`](gcs_test.rs) 的故障注入用例。
- 修改 part size/part count 边界时，同步扩展 `gcs_test.rs` 中 10,001 part 用例和 [`pkg/objstore/azblob_1_aster_unit_test.rs`](azblob_1_aster_unit_test.rs) 中的最小分片用例，并与 `storeapi` 错误身份保持一致。
- 若要让 DTO 或 `TransportConfig` 参与真实路径，应先补独立 Rust 测试，验证 part 排序、XML/序列化合同、CPU fallback 和各超时值；不要把测试嵌入生产文件。
- 新增与本文件直接相关的 Rust 回归应放在同目录的独立 `gcs_test.rs` 或新的 `gcs_extra_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] #[path = ...] mod ...` 接线；不应在 `gcs_extra.rs` 内增加内联测试模块。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/objstore` 确认目标 Rust/Go/测试文件均已索引。
- RustCodeGraph `node --file pkg/objstore/gcs_extra.rs --offset 1 --limit 260` 与 `--offset 258 --limit 100`：覆盖目标文件全部 312 行、37 个索引符号及源码。
- RustCodeGraph `query`：确认 `GCSWriter`、`new_with_attributes`、`upload_part`、`MultipartCloseError`、`InitiateMultipartUploadResult`、`sort_parts` 和 `ObjectStorageCore` 的定义位置；`node ObjectStorageCore` 及 `azblob.rs:326-380` 核对 runtime 所有权与构造参数。
- RustCodeGraph `callers`/`callees` 已对 `GCSWriter`、`create_transport` 和 `sort_parts` 执行，该 CLI 对这些通用/方法名未返回可用边；因而使用 `rg` 的精确符号引用补充确认 `gcs.rs:36,452-460` 的唯一生产构造边与无外部引用的兼容符号。
- 生产路径：`pkg/objstore/gcs.rs:420-483`、`pkg/objstore/objectio/interface.rs:21-113`、`pkg/objstore/azblob.rs:326-380`、`pkg/objstore/lib.rs`、`pkg/objstore/Cargo.toml`。
- Go 对照：`pkg/objstore/gcs_extra.go:40-222,224-355,357-442`，核对 worker/channel、错误保留、XML DTO、分片重试、abort 与 transport 默认值。
- Rust 独立测试：`pkg/objstore/gcs_test.rs:453-652` 覆盖 staging/finalize/abort 故障、原错误链、空/非空成功、取消后仍 abort 与第 10,001 part；`pkg/objstore/azblob_1_aster_unit_test.rs:136-175` 覆盖最小 part size 和实际内存存储写回。Go 回归 `pkg/objstore/gcs_test.go:719-790` 用于对照 abort-on-error 意图。
- 本任务仅生成文档，按计划不运行 Cargo；结构验证要求目标文件存在且恰好包含上述 11 个固定二级标题。

# `pkg/objstore/s3store/interface.rs`

## 文件定位

本文件是 `astersql-objstore-s3store` crate 的底层 S3 协议边界。`pkg/objstore/s3store/lib.rs` 通过 `#[path = "interface.rs"] mod interface;` 装入并用 `pub use interface::*` 对外再导出，因此这里的公开请求/响应类型、`S3API` trait、`AwsS3Api`、错误辅助和响应体适配器共同构成高层 S3 客户端可见的基础 API。

在运行链上，`pkg/objstore/s3store/store.rs::build_api` 用已配置的 `aws_sdk_s3::Client`、共享 Tokio `Runtime` 和可选访问统计构造 `AwsS3Api`；`pkg/objstore/s3store/client.rs::S3Client` 再以 `Arc<dyn S3API>` 持有该实现，完成桶前缀、权限检查、对象读写和分片上传等更高层语义。测试或兼容后端可注入其他 `S3API` 实现而不发起真实网络请求。

该文件本身没有条件编译项。测试装配位于 `lib.rs` 的 `#[cfg(test)]` 模块声明中，独立测试文件是 `pkg/objstore/s3store/interface_test.rs`，没有把测试逻辑内嵌进生产源文件。

## 核心职责

1. 用 Rust 自有 DTO 表达当前 s3store 所需的 S3 请求与响应字段，例如 `ListObjectsV2Input`、`GetObjectOutput`、`PutObjectInput` 和分片上传相关类型，隔离高层代码与 AWS SDK builder 类型。
2. 定义对象安全的同步抽象 `S3API: Send + Sync`。除辅助方法 `unsupported` 外，各操作都有默认“未实现”错误，使轻量 mock 只需覆盖关心的方法；真实网络实现由 `impl S3API for AwsS3Api` 提供。
3. 通过 `run_cancellable` / `AwsS3Api::run` 把同步调用端桥接到 Tokio future，并让 `storeapi::Context` 的取消信号与在途 SDK 请求竞争。
4. 将 `aws_sdk_s3` 的 builder、响应和错误转换为本地 DTO、`anyhow::Error` 与可抽取的 `S3Error`；读写类请求还分别记录 GET/PUT 访问统计。
5. 用 `AwsBody` 将异步 `ByteStream` 暴露为同步 `ReadCloser`，保持分块惰性读取；`MemoryBody` 为内存响应和测试提供相同关闭契约。
6. 为兼容 S3 端点提供请求校验支持：`PutObject`、`UploadPart` 可附加 Base64(MD5)，`DeleteObjects` 在 SDK 无法直接设置 XML `Content-MD5` 时选择 CRC32 checksum 路径。

## 主要符号

- `run_cancellable<F: Future>(runtime, ctx, future) -> Result<F::Output>`：调用前先执行 `ctx.check()`；进入 runtime 后用 `tokio::select!` 在 future 完成和 `ctx.wait_cancelled()` 之间竞速。外层 `Result` 表示取消/上下文失败，成功值仍可能是 SDK 自己的 `Result`，因此调用点通常连续使用 `?` 和 `map_err`。
- `RequestOptions { content_md5 }`：当前唯一的单请求开关。`S3Client::request_options`、同步/并行分片上传路径根据 `s3_compatible` 设置它。
- 请求/响应 DTO：
  - 桶与列举：`HeadBucketInput`、`ListObjectsInput/Output`、`ListObjectsV2Input/Output`、`ListedObject`；v1 保留 `marker`，v2 保留 `continuation_token` 与 `start_after`。
  - 对象操作：`GetObjectInput/Output`、`PutObjectInput`、`DeleteObjectInput`、`DeleteObjectsInput`、`HeadObjectInput/Output`、`CopyObjectInput`。
  - 分片上传：`CreateMultipartUploadInput/Output`、`UploadPartInput/Output`、`CompletedPart`、`CompleteMultipartUploadInput`、`AbortMultipartUploadInput`。
  - 对象锁：`GetObjectLockConfigurationInput`。
- `S3Error`、`api_error`、`error_code`：把 code/message 保存为可下转的错误，并沿 `anyhow` 错误链查找 code。字段私有，调用者通过 `error_code` 判断 `NoSuchKey`、`NotFound` 等分支。
- `MemoryBody`：以 `Cursor<Vec<u8>>` 顺序读取；`close` 只设置关闭标志，关闭后读取返回 `BrokenPipe`。
- `AwsBody`：持有 SDK `ByteStream`、共享 runtime、当前 chunk 的 `Cursor` 与关闭标志；读取时先消费当前 chunk，耗尽后用 runtime 拉取下一 chunk，EOF 返回 0，SDK 流错误转换为 `io::Error`。`close` 同时标记关闭、替换为空流并清空当前缓冲。
- `S3API`：覆盖 HeadBucket、ListObjects v1/v2、Get/Put/Delete、Head/Copy、multipart 生命周期、对象锁查询、预签名 GET 和桶区域探测。`presign_get_object`、`bucket_region` 是 Rust 侧高层调用所需的扩展，不在同路径 Go `S3API` 方法集中。
- `AwsS3Api { client, runtime, access_rec }`：真实适配器。`record_get` / `record_put` 用合成的 HTTP method 调用 `objectio::recording::AccessStats::rec_request`；`run` 统一委托给 `run_cancellable`。
- 内部辅助 `sdk_error`、`content_md5`、`optional`：分别负责常见错误码识别、MD5 的 Base64 编码、以及按 `Option<String>` 条件应用 SDK builder 字段。

## 执行流程

典型请求从 `S3Client` 开始：高层先将桶前缀、对象名、HTTP Range、ACL/SSE/存储类或 multipart 状态组装成本文件 DTO，再调用 `Arc<dyn S3API>` 的对应方法。真实实例进入 `AwsS3Api` 后按以下顺序执行：

1. `record_get` 或 `record_put` 记录访问类型；这一步在 SDK future 启动前发生。
2. 从 `self.client.<operation>()` 获得 AWS SDK builder，写入必需字段，并由 `optional` 写入存在的可选字段。
3. 对 `PutObject` / `UploadPart`，若 `RequestOptions::content_md5` 为真，使用 `content_md5` 设置请求头；`DeleteObjects` 则构建 `ObjectIdentifier` 和 `Delete`，并在相同选项下选择 CRC32 checksum。
4. 将 `request.send()` 交给 `AwsS3Api::run`。`run_cancellable` 先拒绝已取消上下文，再阻塞 runtime，直至 SDK future 完成或上下文取消。
5. SDK 错误经 `sdk_error(operation, error)` 转换；成功响应则投影到本地 DTO。列举只保留 key/size 和分页字段，HeadObject 只保留复制状态，对象锁查询只返回是否为 `Enabled`。
6. `GetObject` 不预读完整对象，而是把 `output.body` 包装成 `AwsBody`，同时保留 `content_length` 和 `content_range`。调用者随后同步读取，`AwsBody::read` 按需拉取 SDK chunk。

multipart 主链由 `client.rs` 编排而非本文件编排：Create 返回 `upload_id`；每次 UploadPart 返回可选 ETag；调用端按 part number 形成 `CompletedPart` 列表后 Complete，任一并行 part 失败时调用 Abort。本文件只负责把这些阶段准确映射到 AWS SDK。

`presign_get_object` 先检查上下文并创建带过期时间的 `PresigningConfig`，然后直接在 runtime 中生成 URL；它没有使用 `run_cancellable`，因此只保证开始前的取消检查。`bucket_region` 发起 HeadBucket：成功时取响应 region；失败时优先读取 `x-amz-bucket-region` 响应头以接受重定向携带的区域，否则返回规范化诊断错误。

## 数据与状态

所有请求 DTO 都拥有 `String` / `Vec<u8>`，从而可以安全跨越 trait 调用和 SDK future 生命周期；代价是 `PutObject` 与 `UploadPart` 构造 `ByteStream` 时会克隆 body。列举输出将 SDK 对象简化为 `ListedObject { key, size }`，缺失 SDK 字段按空字符串或 0 处理；分页截断字段缺失时按 `false` 处理。

`AwsS3Api` 可克隆：AWS client 与 runtime 本身可共享，访问统计使用 `Option<Arc<AccessStats>>`。它不保存某次请求的可变状态；每个方法创建自己的 builder。`S3API: Send + Sync` 允许 `Arc<dyn S3API>` 被 `client.rs::MultipartUploader` 的 scoped worker 线程并发调用。

响应体是例外的有状态对象：`MemoryBody.inner` / `AwsBody.current` 保存当前读位置，`closed` 是单调的关闭状态。它们通过 `&mut self` 读取和关闭，不在内部加锁，也没有声明可并发读；所有权和可变借用负责串行化单个 body 的使用。`AwsBody` 在 close 后丢弃流和缓冲，后续读取稳定返回 `BrokenPipe`。

## 依赖与调用关系

- 上游装配：`pkg/objstore/s3store/lib.rs` 装入并公开再导出本模块；`pkg/objstore/s3store/store.rs::build_api` 是真实 `AwsS3Api` 的构造点。
- 主要调用者：`pkg/objstore/s3store/client.rs::S3Client` 消费对象操作和错误码；同文件 `MultipartWriter` / `MultipartUploader` 消费 multipart API 与 `RequestOptions`。`pkg/objstore/s3store/ks3.rs` 也通过 `&dyn S3API` 复用 v1 列举、复制和错误码语义。RustCodeGraph 将目标文件标为被 9 个文件使用，并点名 `objectio/interface.rs`、`ossstore/client.rs`、`s3store/client.rs` 及相关测试；精确 `rg` 进一步确认了上述生产调用点。
- 下游 crate：`aws-sdk-s3` 提供 client、builder、类型和 `ByteStream`；`tokio` 提供 runtime/select；`storeapi::Context` 提供取消；`prefetch::reader::ReadCloser` 定义返回 body 契约；`objectio::recording` 记录访问；`anyhow` 承载跨层错误；`md-5` 与 `base64` 生成 Content-MD5；`http` 只用于构造统计请求元数据。
- crate 边界由 `pkg/objstore/s3store/Cargo.toml` 明确：包名为 `astersql-objstore-s3store`，库入口是 `lib.rs`，并以路径依赖连接 `objectio`、`prefetch`、`s3like`、`storeapi`。该 manifest 没有定义 feature，因此本文件的行为不由 crate feature 分叉。
- `unsupported` 是 trait 的泛型辅助默认方法，但当前每个公开操作都各自返回明确的默认错误；真实 `AwsS3Api` 覆盖所有操作。

## 错误处理与边界

`run_cancellable` 区分三层失败：开始前 `ctx.check()` 失败、等待中的取消返回 `operation cancelled`、SDK future 自身返回服务/传输错误。调用者必须注意其返回的是 `Result<F::Output>`，不能把“future 已完成”和“SDK 请求成功”混为一层。

`sdk_error` 目前通过错误显示文本识别 `NoSuchBucket`、`NoSuchKey`、`NotFound`、`AccessDenied`、`BucketAlreadyExists`；未匹配时以操作名作为 code。这个做法支持 `client.rs` 的缺失对象分支，但并不是完整的 AWS modeled error 分类，新增依赖错误码的业务分支前应先验证并扩展该映射。`error_code` 会遍历整个 `anyhow` chain，因此额外上下文包装不会自动丢失 `S3Error`。

builder 构建失败（例如无效 `ObjectIdentifier` / `Delete`）直接转为 `anyhow`，尚未发起网络请求。SDK 输出缺失可选字段时，多数映射选择默认值而不是报错；特别是 CreateMultipartUpload 缺失 upload id 会产生空字符串，调用端应把服务返回字段完整性作为兼容风险看待。

`presign_get_object` 只做调用前 `ctx.check()`，生成期间不会监听后续取消。`bucket_region` 在失败响应含非空 `x-amz-bucket-region` 时返回该 region，即使原 HeadBucket 返回错误；这正是跨区域重定向探测路径。其对 `newBucketRegionDetectionRetryer().IsErrorRetryable(...)` 的调用结果被丢弃，只保留与既有重试诊断路径的接线事实，不能据此宣称这里执行了重试。

流式读取边界包括：空目标 buffer 立即返回 0；流自然结束返回 0；SDK chunk 错误成为 `io::Error::other`；close 后读取返回 `BrokenPipe`。`MemoryBody::close` 不清除内存，而 `AwsBody::close` 会清空当前 chunk 并替换底层流。

## 并发与资源生命周期

`AwsS3Api` 的共享生命周期由 `Arc` 管理；`Runtime` 也存于 `Arc`，既用于发起请求，也必须一直存活到 `AwsBody` 读完，因为 body 的每次补充读取都调用同一个 runtime。返回 `GetObjectOutput` 后，即使原请求方法已经结束，`AwsBody` 仍持有 runtime 和 `ByteStream`，保证响应流所需资源不会提前释放。

取消采用协作式竞争：`tokio::select!` 在 future 和 `Context::wait_cancelled` 中先完成者胜出。取消分支返回后 future 被丢弃，从调用方视角在途操作终止；本文件不提供独立后台任务、重试循环或 join handle。`interface_test.rs::in_flight_s3_request_stops_on_context_cancellation` 使用永久 pending future 和另一线程触发 `Context::cancel`，验证该生命周期边界。

trait 的 `Send + Sync` 约束支持共享服务实例并发调用；具体并行分片策略在 `client.rs::MultipartUploader` 中。单个 `AwsBody` / `MemoryBody` 依靠独占可变借用顺序访问，没有内部同步保证。调用者应显式 close 不再需要的 body；`AwsBody::close` 可主动释放尚未消费的 SDK 流和 chunk 缓冲。

## 与 Go 版本的对应关系

同路径 `pkg/objstore/s3store/interface.go::S3API` 定义 14 个 AWS SDK v2 操作，Rust trait 保留了相同的 Head/List/Get/Put/Delete/Copy/multipart/object-lock 方法集合；`ListObjectsInput.marker` 明确保留 Go v1 列举分页契约，`interface_test.rs::s3_api_exposes_go_list_objects_v1_contract` 验证该方法存在及默认错误。

两版的抽象层次不同。Go trait 直接使用 `context.Context`、`s3.*Input/Output` 和可变参数 `func(*s3.Options)`，真实 `*s3.Client` 可直接满足接口；Rust 为对象安全和同步上层代码定义自有 DTO、`RequestOptions` 与 `ReadCloser`，再用显式 `AwsS3Api` 适配 AWS SDK future。Rust trait 还增加 `presign_get_object` 与 `bucket_region`，对应 Rust 高层所需能力，而不是声称它们属于 Go 同路径接口。

错误判断也采用不同机制：Go `client.go` 通过 `errors.As(..., smithy.APIError)` 读取 modeled code；Rust 用 `S3Error` / `error_code` 并由 `sdk_error` 从 SDK 错误文本识别有限的常见 code。请求选项方面，Go 的 `withContentMD5` 是 SDK option function；Rust 将其压缩成 `RequestOptions::content_md5`。Put/UploadPart 可直接设置 Content-MD5，但 DeleteObjects 因 Rust SDK 不暴露序列化 XML body 而使用 CRC32 checksum，这是已记录的实现差异，不能视为字节级完全相同。

Go 请求天然异步感知传入 context；Rust 通过共享 runtime 上的 `run_cancellable` 恢复取消语义。Rust `AwsBody` 则负责把异步 `ByteStream` 转成 Go `io.ReadCloser` 风格的同步接口。Go 接口没有默认实现，Rust 的默认错误是为局部 mock/迁移提供的便利，不代表操作在生产实现中缺失；`AwsS3Api` 已逐项覆盖。

## 扩展指南

新增 S3 操作时，应同时修改本文件的输入/输出 DTO、`S3API` 方法及默认错误，并在 `impl S3API for AwsS3Api` 中完成 builder、取消、错误映射和统计分类。随后同步检查 `pkg/objstore/s3store/client.rs` 的高层接线、`pkg/objstore/s3store/store.rs` 的构造需求、所有本地 mock 实现，以及独立测试文件；若要保持 Go 对齐，还需核对 `interface.go` 和相应 Go 调用点。不要把单元测试加入本生产文件。

新增请求字段时优先沿用 `optional` 的条件应用模式，明确字段缺失的默认语义，并确认 enum 字符串经 AWS SDK `from` 后对目标兼容端点有效。新增写操作必须决定它属于 `record_put` 还是其他统计类别，并判断 `s3_compatible` 下是否需要 payload 校验。涉及大对象时应避免延续当前 `Vec<u8>` clone 模式造成额外峰值内存，必要时设计新的流式输入契约而不是悄悄改变现有 DTO。

新增错误分支时不要仅在高层匹配字符串；先扩展/验证 `sdk_error` 能稳定产出结构化 code，并在 `interface_test.rs` 或相邻独立测试中覆盖包装后的 `error_code`。修改响应 body 时必须保持 `Read` 的 EOF、关闭后错误和 SDK 流错误传播契约，并同步 `client_1_aster_unit_test.rs` 中 `MemoryBody` / `AwsBody` 测试。

修改取消行为时要分别覆盖“调用前已取消”和“在途取消”。如果把 `presign_get_object` 也改为在途可取消，应通过统一 helper 实现并证明 SDK presign future 的生命周期；如果调整 runtime 所有权，必须保证已返回的 `AwsBody` 仍能继续拉取数据。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、目标 Rust 文件已索引；`files --filter pkg/objstore/s3store` 列出目标、Go 对照、模块入口和测试；`node --file pkg/objstore/s3store/interface.rs` 完整读取 1–1004 行，并报告该文件被 9 个文件使用。对 `S3API`、`AwsS3Api`、`run_cancellable`、`MemoryBody`、`AwsBody` 的 `query` 找到目标定义及 `store.rs::build_api` / body 测试等候选；`callers/callees` 对这些 trait/struct 名称没有返回边，因此使用精确 `rg` 补充实际使用点，没有把空图结果解释为“无调用者”。
- 生产源码与装配：读取 `pkg/objstore/s3store/interface.rs`、`pkg/objstore/s3store/lib.rs`、`pkg/objstore/s3store/client.rs`、`pkg/objstore/s3store/store.rs`、`pkg/objstore/s3store/ks3.rs` 的直接相关符号；读取 `pkg/objstore/s3store/Cargo.toml` 和 `pkg/objstore/{Cargo.toml,lib.rs}` 核对 crate 与路径依赖边界。本包没有 `doc.go`，最近的模块契约由 `lib.rs` 提供。
- Go 对照：读取 `pkg/objstore/s3store/interface.go` 的完整方法集，以及 `client.go`、`store.go` 的直接使用和真实 SDK 构造路径。
- 独立测试：读取 `pkg/objstore/s3store/interface_test.rs`（在途取消、ListObjects v1 默认契约）、`pkg/objstore/s3store/client_1_aster_unit_test.rs`（`MemoryBody` / `AwsBody` 读与 close 语义）和 `pkg/objstore/s3store/client_test.rs`（错误码、Range、分页、批量删除等 DTO/trait 使用）。这些是静态事实证据；按任务约束未运行 Cargo。
- 人工复核结论：本文件存在于高层前缀客户端与异步 AWS SDK 之间，统一可注入操作集合、同步取消桥、响应体、错误码和兼容请求选项；安全扩展必须同步 trait、真实适配器、mock/高层调用和独立测试，并关注取消、流生命周期、错误分类及 payload 内存成本。

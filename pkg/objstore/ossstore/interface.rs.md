# `pkg/objstore/ossstore/interface.rs`

## 文件定位

本文件是 `astersql-objstore-ossstore` crate 的底层 OSS SDK 适配层。模块入口 [`lib.rs`](lib.rs) 将其声明为私有 `interface` 模块后通过 `pub use interface::*` 重导出公开项；[`Cargo.toml`](Cargo.toml) 表明该 crate 直接依赖启用 `blocking`、`rust-tls` 的 `ali-oss-rs 0.2.5`，并依赖仓库内的 `storeapi`、`objectio`、`prefetch` 和 `s3like` crate。

它位于高层 [`Client`](client.rs) 与阿里云 SDK 之间：`Client` 把带桶前缀的对象操作翻译成这里定义的请求类型并调用 `API` trait；[`store.rs`](store.rs) 的 `NewOSSStorage` 经 `build_api` 构造 `AliyunOssApi`，再将数据 API 和独立的公网预签名 API 注入 `Client::with_presign_api`。因此本文件处理 SDK 形状、重试、取消、错误码、响应元数据和访问统计，不负责对象键前缀、权限策略或存储生命周期的高层编排。

文件没有条件编译项；测试由 [`lib.rs`](lib.rs) 中的 `#[cfg(test)] mod interface_test` 以独立文件 [`interface_test.rs`](interface_test.rs) 接入。

## 核心职责

1. 定义 `API: Send + Sync` 以及对象 CRUD、列举、预签名和分片上传所需的 Rust 请求/响应 DTO，使高层 `Client` 可用 trait object 注入真实实现或测试替身。
2. 用 `AliyunOssApi` 将这些抽象逐项映射到 `ali_oss_rs::blocking` 的 bucket、object、multipart API；每次调用都从 `CredentialsProvider` 获取当前凭证并复用同一个 blocking HTTP client。
3. 在 `execute` 中统一执行上下文取消检查、`OssRetryer` 可重试分类和退避等待，并将等待切成最多 50 ms 的小段以便及时响应取消。
4. 通过 `OssServiceError`、`sdk_error`、`api_error` 和 `error_code` 保留可由上层识别的 OSS 服务错误码，同时保留非服务错误的结构化错误链。
5. 弥补 `ali-oss-rs 0.2.x` 缓冲下载不暴露响应头的差异：Range 下载先 HEAD 取得对象总长度，再由 `content_range_for_download` 重建并校验 Go 路径依赖的 `Content-Range`。
6. 用 `record` 向可选的 `objectio::recording::AccessStats` 记录逻辑 HTTP 请求方法；预签名只生成 URL，不记录网络请求。

## 主要符号

- 请求/响应 DTO：`ListObjectsV2Input`、`ListedObject`、`ListObjectsV2Output`、`GetObjectInput`、`GetObjectOutput`、`PutObjectInput`、`DeleteObjectInput`、`DeleteObjectsInput`、`HeadObjectInput`、`CopyObjectInput`、`CreateMultipartUploadInput/Output`、`UploadPartInput/Output`、`CompletedPart`、`CompleteMultipartUploadInput`、`AbortMultipartUploadInput` 和 `ListPartsInput`。它们刻意使用拥有所有权的 `String`/`Vec<u8>`，方便跨 trait object、重试闭包及分片工作线程传递。
- `API: Send + Sync`：公开的底层能力边界，共 14 个操作。所有方法都有返回“operation ... is not implemented”的默认实现，使 mock 可以只覆盖当前测试所需的方法；默认实现不是生产兜底，真实后端由 `impl API for AliyunOssApi` 全量实现。
- `AliyunOssApi`：真实 SDK 适配器，持有 `Arc<dyn CredentialsProvider>`、endpoint、region、共享 `reqwest::blocking::Client` 和可选访问统计器。`new` 创建 HTTP client；`client` 用最新凭证为单次操作创建 SDK client，并仅在 token 非空时附加 STS token。
- `AliyunOssApi::execute<T>`：除 `presign_get_object` 外所有真实 OSS 操作的统一执行器。它最多执行 `OssRetryer::MaxAttempts()` 次，首轮与每段退避前都调用 `storeapi::Context::check`。
- `content_range_for_download`：解析 `bytes=<start>-<end?>`，将请求结束位置裁剪到对象末尾，核对实际下载长度，并生成 `bytes start-end/total`。无 Range 时返回 `None`。
- `GetObjectOutput::from_bytes` 与 `MemoryBody`：把 SDK 返回的完整缓冲区包装成 `prefetch::reader::ReadCloser`。`MemoryBody::close` 只标记关闭，关闭后读取返回 `BrokenPipe`。
- `OssServiceError`、`api_error`、`error_code`、`sdk_error`：服务错误码桥。`ApiError` 被转换为本地错误类型；SDK/reqwest/io 等其他错误用 operation context 包装但不扁平化，供重试器按具体类型判断。
- `put_options`：把分片创建输入中的 SSE 算法、KMS key ID 和存储类型转换为 SDK `PutObjectOptions`；全部为空时返回 `None`。

公开 API 包括上述 DTO、`API`、`AliyunOssApi::new`、`MemoryBody::new`、`api_error` 和 `error_code`。`OssServiceError`、`content_range_for_download` 仅 crate 内可见；`sdk_error`、`put_options`、`client`、`record`、`execute` 是文件内部实现。

## 执行流程

真实后端的主链如下：

1. [`store.rs`](store.rs) 的 `NewOSSStorage` 解析凭证、region 与 endpoint，调用 `build_api`；后者调用 `AliyunOssApi::new`。数据访问与预签名分别可使用内网/公网 endpoint，随后通过 `Client::with_presign_api` 注入。
2. [`client.rs`](client.rs) 的 `Client` 为对象名加桶前缀、生成 Range 或分片参数，并通过 `Arc<dyn API>` 调用本文件的抽象。例如 `Client::GetObject` 形成 `GetObjectInput`，`MultipartWriter`/`MultipartUploader` 依次调用 initiate、upload、complete，失败时 uploader 会尝试 abort。
3. `AliyunOssApi` 方法先 `ctx.check()`，再按真实 HTTP 动作调用 `record`；随后 `execute` 在每次尝试中调用 `client()` 获取当前凭证并执行 blocking SDK 请求。可重试错误进入退避，不可重试错误或最后一次失败直接返回。
4. SDK `ApiError` 由 `sdk_error` 转成含 code/message 的 `OssServiceError`；上层可用 `error_code` 区分 `NoSuchKey`。`is_bucket_exist` 在本层额外把 `NoSuchBucket` 转成 `Ok(false)`。
5. 结果被转换为本地 DTO：列举结果转换 key/size/pagination；分片结果转换 upload ID 或 ETag；完整对象下载返回内存 `ReadCloser`。

Range 下载有额外分支：存在 `input.range` 时先记录并执行 HEAD 得到总长度，再记录并执行 GET 到缓冲区，最后 `content_range_for_download` 校验请求、总长度与下载字节数一致；无 Range 时只执行 GET，`content_range` 为 `None`。

分片上传映射为：`initiate_multipart_upload` 先经 `put_options` 转换可选 SSE/存储类；`upload_part` 和 `complete_multipart_upload` 将有符号分片号校验转换为 `u32`；`complete` 将 `(part_number, etag)` 列表交给 SDK；`abort` 清理会话；`list_parts` 把 SDK 分片号转回 `i32`。

## 数据与状态

`AliyunOssApi` 本身没有每请求可变状态：endpoint、region 和 HTTP client 在构造后保持不变，动态凭证通过 `Arc<dyn CredentialsProvider>` 在 `client()` 时获取快照。`access_rec` 也是共享的可选 `Arc`。这使同一实例可通过 `API: Send + Sync` 被多个调用方/上传线程共享。

各 DTO 是一次调用的值对象。上传 body 当前为 `Vec<u8>`，重试闭包每次尝试会 clone body；下载使用 SDK 的 `get_object_to_buffer`，因此整个对象或 Range 先驻留内存，再由 `MemoryBody` 顺序读取。`MemoryBody` 内部是 `Cursor<Vec<u8>>` 与 `closed: bool`，没有内部锁，也不声明 `Clone`；它作为每个响应独占的 body 使用。

访问统计的计数粒度是适配器发出的请求：普通操作通常记一次；Range GET 会分别记录 HEAD 和 GET。重试发生在同一次 `record` 之后，所以当前统计表示逻辑适配器动作而非每次重试尝试。预签名不发网络请求，因而不计数。

分片号边界在发送前检查：负数或无法转成 `u32` 的 `part_number` 返回错误；列举返回的过大 `u64` size/part number 则分别饱和为 `i64::MAX`/`i32::MAX`。`list_objects_v2` 将小于 1 的 `max_keys` 提升为 1，正常转换失败时使用 1000。

## 依赖与调用关系

上游调用关系：

- [`store.rs`](store.rs)：`NewOSSStorage → build_api → AliyunOssApi::new`，并调用 `bucket_location` 探测真实 region。
- [`client.rs`](client.rs)：`Client` 持有 `Arc<dyn API>`；权限探测、Get/Put/Delete/List/Copy/Presign 以及两种 multipart 实现均调用本文件 trait。`PrefixClient::GetObject` 再被 `s3like::Storage` 的读取/open 路径调用，`PrefixClient::MultipartWriter` 被创建对象路径调用。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和其他独立测试：实现只覆盖所需方法的 `MockApi`，验证请求映射、错误码与分片顺序；[`interface_test.rs`](interface_test.rs) 直接覆盖 Range 元数据重建。

下游依赖：

- `ali_oss_rs::blocking::{bucket, object, multipart}` 提供真实 OSS 调用与预签名；`ali_oss_rs` 的公共枚举还用于校验 SSE 算法和 StorageClass。
- `reqwest::blocking::Client` 是共享传输客户端，`http::Method` 和 `objectio::recording::AccessStats` 用于访问统计。
- `storeapi::Context` 提供取消检查；`OssRetryer`（[`retry.rs`](retry.rs)）提供最大尝试数、错误分类和退避时长。
- `prefetch::reader::ReadCloser` 是下载 body 必须满足的接口；`anyhow` 承载上下文和可下转的错误链。

RustCodeGraph 的直接节点证据包括：`execute` 被 `is_bucket_exist`、`bucket_location`、head/get/put/copy/delete/list 及全部 multipart 网络方法调用；`content_range_for_download` 被本文件的 `get_object` 调用；`Client` 的 `PrefixClient::GetObject` 上游为 `s3like::store::{doReadFile, open}`，`PrefixClient::MultipartWriter` 上游为 `s3like::store::Create`。

## 错误处理与边界

- `API` 默认方法总是返回明确的未实现错误。新增 mock 可利用这一点，但生产实现遗漏方法会在运行时失败，而非编译期强制实现；扩展 trait 时必须同时审查真实实现和测试替身。
- `execute` 在操作前及退避的每个最多 50 ms 分段前检查取消。取消错误立即传播；仅在仍有剩余尝试且 `OssRetryer::IsErrorRetryable` 为真时重试。`RetryDelay` 自身失败也直接传播。
- `sdk_error` 仅把 SDK 的 `ApiError` 映射为 `OssServiceError`；其余结构化错误保留并增加 operation 名称。不要改成字符串化错误，否则 [`retry.rs`](retry.rs) 的类型/状态判断和 `error_code` 会失效。
- `is_bucket_exist` 只吞掉服务码 `NoSuchBucket`；其他错误保留。对象不存在的 `NoSuchKey` 由上层 [`client.rs`](client.rs) 根据调用语义转换为“不存在”或权限检查成功。
- `presign_get_object` 先把 `Duration::as_secs()` 转为 `u32`，超出 SDK 支持范围时以带上下文错误返回。该路径执行 `ctx.check()`，但不经过 `execute`，所以没有本层重试。
- Range 只接受 `bytes=<非负起点>-<可选非负终点>`。空对象、起点越过有效末尾、数字解析失败、算术溢出或 SDK 返回字节数与推导长度不符都会失败；当前不支持 suffix range（如 `bytes=-10`）或多段 Range。
- `get_object_to_buffer` 和上传 body clone 带来与对象/分片大小成正比的内存成本；这是当前实现事实，扩展大对象路径时不能假设它已是网络流式下载。
- `MemoryBody::close` 幂等地设置标志；close 后读取稳定返回 `BrokenPipe`。其 `content_length` 来自缓冲区长度，而 Range 的总长只体现在 `content_range`。
- 分片上传参数中的非法 SSE/StorageClass 值在 SDK 枚举转换阶段失败；负分片号在网络前失败。`list_parts` 当前不暴露分页参数，且输入 DTO 没有 marker/max-parts 字段。

## 并发与资源生命周期

`API: Send + Sync`、`Arc<dyn CredentialsProvider>` 和可 clone 的 blocking HTTP client 允许 `AliyunOssApi` 被共享。真正的并发分片调度位于 [`client.rs`](client.rs) 的 `MultipartUploader`：多个 scoped worker 共享同一 `Arc<dyn API>`，每个 worker 调用这里的 `upload_part`。因此本文件的实现不能引入无同步的内部可变状态。

每次真实操作临时创建一个 `ali_oss_rs::blocking::Client`，但底层复用 `reqwest::blocking::Client`；SDK client 在调用结束后释放。凭证不缓存在本文件中，生命周期由 provider/`CredentialRefresher` 管理，故刷新后的凭证会在下一次尝试（包括重试）重新读取。

`execute` 是同步阻塞的：SDK 调用和 `std::thread::sleep` 都占用当前线程；取消只在请求尝试之间和退避小段之间观察，无法强制中断已进入的 blocking SDK 请求。范围下载缓冲区由 `GetObjectOutput` 拥有，调用方须调用 body 的 `close`；内存仍随 body drop 才释放。

multipart session 的生命周期由上层 `Client` 编排：initiate 返回 `upload_id`，upload 产生 ETag，complete 提交；并发 uploader 在上传或 complete 失败时尽力 abort。`API` 的 `abort_multipart_upload` 自身只执行请求，不保证调用方一定触发；流式 `MultipartWriter` 当前 close 只 complete，drop 或写失败不在本文件自动 abort。

## 与 Go 版本的对应关系

Go 的 [`interface.go`](interface.go) 只声明 SDK 子集 `API`，请求/响应直接使用 `alibabacloud-oss-go-sdk-v2/oss` 类型；Rust 文件同时承担 trait、等价 DTO 和真实 `ali-oss-rs` 适配实现，这是两种 SDK 形态不同造成的额外层次。两端都覆盖 bucket existence、presign、head/get/put/copy/delete/list 与完整 multipart 操作集合。

[`client.go`](client.go) 与 [`client.rs`](client.rs) 的高层语义保持对应：对象键加前缀、`GetHTTPRange`、`NoSuchKey` 判断、空批量删除短路、分页参数、SSE/KMS/StorageClass 及从 1 开始的分片号。独立 Rust 测试 `migration_aster_unit_test.rs` 具体核对这些请求映射与分片顺序。

主要实现差异如下：

- Go 在 `store.go` 将 retryer 和 response handler 直接配置到官方 SDK client；Rust 在本文件的 `execute`/`record` 显式实现重试、取消轮询和访问计数。
- Go GET 响应原生提供 body、`ContentLength`、`ContentRange`；`ali-oss-rs 0.2.x` 的 buffer API丢失响应头，Rust 对 Range 额外 HEAD 并由 `content_range_for_download` 重建 `Content-Range`。`interface_test.rs` 验证封闭/开放结束范围和长度不一致错误。
- Go SDK 接收 reader 并可返回流式 body；当前 Rust 上传 DTO 和 GET 实现使用内存 `Vec<u8>`。高层并发 uploader 仍按分片有界流水读取，但每个分片在本文件边界是拥有的缓冲区。
- Go `API` 没有 `GetBucketLocation` 方法，因为构造路径直接使用具体 SDK client；Rust 将 `bucket_location` 放入 trait，让 `store.rs` 的 region 探测也复用 `AliyunOssApi`。
- Go SDK 自己完成 uploader 的并发与 abort 行为；Rust 在 `client.rs` 显式实现 worker、排序、complete 和失败后的尽力 abort。本文件只提供对应原子操作。

这些是当前代码差异，不应据此推断两端在未覆盖的 SDK 细节（例如所有 header、分页极限或网络中断时机）完全等价。

## 扩展指南

新增 OSS 原子操作时，先在本文件增加专用输入/输出 DTO 和 `API` 方法，再在 `impl API for AliyunOssApi` 完成 SDK 映射；随后在 [`client.rs`](client.rs) 接入高层桶前缀和 `s3like` 语义。若操作需要测试替身，放在独立的 `*_test.rs` 或 [`mock/api_mock.rs`](mock/api_mock.rs)，不要把测试内嵌到生产文件。

安全扩展时需同步检查：

- 网络操作是否应调用 `record`，使用哪个 HTTP method，是否需要把一次逻辑操作拆成多次实际请求；访问统计兼容性与计数口径可能变化。
- 是否应进入 `execute`，服务错误能否经 `sdk_error` 保留 code，是否需要像 `NoSuchBucket` 一样在本层做语义转换。不要绕开 `Context::check`。
- 新字段能否被 `ali-oss-rs 0.2.5` 表达；枚举/整数转换必须在网络前验证，避免静默截断。若增加列表分页，DTO、SDK 请求和 Go 对照测试应一起更新。
- body 生命周期与内存：若改为真正流式 GET/PUT，需要仍满足 `ReadCloser`、重试可重放性、取消和共享线程安全；不能简单把一次性 reader 放进会重试的闭包。
- multipart 新选项应落在 `CreateMultipartUploadInput`/`put_options`，并同步 [`client.rs`](client.rs) 的两个创建入口与独立测试。新增 trait 方法后必须实现真实后端；默认未实现方法会让遗漏只能在运行时暴露。
- 修改 Range 规则时同时更新 `content_range_for_download`、`AliyunOssApi::get_object` 和 [`interface_test.rs`](interface_test.rs)，并与 Go `client.GetObject` 返回的 `ContentLength`/`ContentRange` 契约核对。

兼容性风险集中在公开 DTO/trait 签名、错误码可下转性和 Go 可见响应字段；性能风险集中在缓冲下载、上传 body clone、Range 的额外 HEAD 以及同步退避占线程。任何变更都应以最小的独立 Rust 测试覆盖错误和边界分支，并保留 Go 测试意图。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/objstore/ossstore` 确认目标 Rust/Go/测试文件均已索引。
- 文件与符号查询：`node --file pkg/objstore/ossstore/interface.rs` 阅读全部 870 行；`node interface.rs::API`、`node interface.rs::AliyunOssApi`、`node interface.rs::execute`、`node interface.rs::get_object`、`node interface.rs::content_range_for_download` 核对声明、源码与调用轨迹。
- 关键图边：`NewOSSStorage → build_api → AliyunOssApi::new`；所有真实网络方法（预签名除外）`→ execute`；`get_object → content_range_for_download`；`s3like::store::{doReadFile, open} → PrefixClient::GetObject`；`s3like::store::Create → PrefixClient::MultipartWriter`。
- crate/模块证据：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`store.rs`](store.rs)、[`client.rs`](client.rs)、[`retry.rs`](retry.rs)。目标包没有 `doc.go`；最近的模块契约由这些入口与 manifest 给出。
- Go 对照：[`interface.go`](interface.go)、[`client.go`](client.go)、[`store.go`](store.go)、[`retry.go`](retry.go)。
- 独立测试证据：[`interface_test.rs`](interface_test.rs) 验证 Range 元数据重建和长度不一致错误；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 验证 trait mock、错误码、请求映射、分页、SSE/存储类和 multipart 顺序。测试接线在 [`lib.rs`](lib.rs) 的 `#[cfg(test)]` 模块中。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构检查、`git diff --check`、变更范围检查和人工事实复核。

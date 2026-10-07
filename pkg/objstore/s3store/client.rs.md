# `pkg/objstore/s3store/client.rs`

## 文件定位

本文件是 `astersql-objstore-s3store` crate 的带桶前缀高层客户端实现。crate 入口 `pkg/objstore/s3store/lib.rs` 将它声明为 `client` 模块并公开再导出；`pkg/objstore/s3store/Cargo.toml` 表明该 crate 直接依赖 `s3like`、`storeapi`、`objectio`、AWS SDK 相关 crate 与 `anyhow`。

生产构造链位于 `pkg/objstore/s3store/store.rs::NewS3Storage`：该函数先创建底层 `AwsS3Api`，再用桶名、规范化前缀、S3 配置和“是否为兼容 S3”标志构造 `S3Client`，完成权限探测后交给 `s3like::NewStorage`。因此本文件位于 AWS/兼容 S3 SDK 适配层与统一对象存储门面之间：向下只依赖 `S3API`，向上实现 `s3like::PrefixClient`。

## 核心职责

- 把调用方使用的逻辑对象名通过 `storeapi::BucketPrefix::ObjectKey` 映射成真实桶内 key，并把 `S3API` 的响应转换为 `s3like` 的公共响应类型。
- 实现桶、列举、读取、写入/删除四类权限探测；其中读取探测把 `NoSuchKey` 解释为“请求已到达对象层，因而权限有效”。
- 提供 Get、Put、单删、批删、Head、存在性检查、分页列举、服务端复制和预签名下载 URL。
- 提供两条 multipart 写入路径：`MultipartWriter` 是调用方每次 `write` 对应一个 part 的同步状态机；`MultipartUploader` 自己切块并按配置并行上传。
- 对 S3 兼容服务的 Put、批删和 UploadPart 请求设置 Content-MD5 选项，并记录 Put/Head/List 三类 API 指标。

这些职责的边界由 `S3Client` 的固有方法及其 `s3like::PrefixClient` 实现共同确定；本文件不负责凭证、region、endpoint、重试器或高层读取重试，这些分别在 `store.rs`、`retry.rs` 和 `pkg/objstore/s3like/store.rs`。

## 主要符号

- `S3Client { svc, bucket_prefix, options, s3_compatible }`：可克隆的高层客户端。`svc: Arc<dyn S3API>` 允许生产 SDK 与测试 mock 共用；`bucket_prefix` 负责命名空间映射；`options` 保存 ACL、SSE、KMS key、storage class 等上传属性；`s3_compatible` 决定是否请求 Content-MD5。
- `S3Client::new<T>` / `S3Client::from_dyn`：分别接受具体 `Arc<T>` 和已经擦除类型的 `Arc<dyn S3API>`。两者仅装配状态，不发网络请求。
- `request_options`、`non_empty`：前者将兼容模式投影为 `RequestOptions.content_md5`；后者只把非空配置字符串转换为 `Some`。
- `CheckBucketExistence`、`CheckListObjects`、`CheckGetObject`、`CheckPutAndDeleteObject`：`s3like::CheckPermissions` 调用的四个权限入口。
- `GetObject`、`PutObject`、`DeleteObject(s)`、`HeadObject`、`IsObjectExists`、`ListObjects`、`CopyObject`、`PresignObject`：对象操作入口。`GetObject` 将半开区间转换为 HTTP Range；`ListObjects` 保留 Go 的 `isize as i32` 强制转换语义；`CopyObject` 用 `join_copy_source` 清理源路径的空段和点段。
- `buildPutObjectInput`：集中装配 Put 的 bucket、key、body、ACL、SSE、KMS key 与 storage class。注意 bucket 取自传入的 `options.Bucket`，而 key 仍取当前 `bucket_prefix`。
- `withContentMD5`：把传入请求选项强制改为 Content-MD5；当前主要是与 Go 命名/能力对齐的公开辅助函数，客户端内部通常直接使用 `request_options` 或构造 `RequestOptions`。
- `MultipartWriter` 及 `objectio::Writer` 实现：保存 create 响应和已成功 part 列表；`write` 上传下一编号 part，`close` 提交当前列表。
- `MultipartUploader` 及 `s3like::Uploader` 实现：`Upload` 调用内部 `upload`，并由 `normalize_upload_error` 将包含 `MaxUploadParts` 的 SDK 错误统一成 `storeapi::ErrExceedMaxUploadParts`。
- `impl s3like::PrefixClient for S3Client`：薄转发适配层；把本文件的确定存在响应包装成 trait 需要的 `Option`，使 `s3like::Storage` 能以 trait object 使用客户端。

## 执行流程

1. `store.rs::NewS3Storage` 构造 `S3Client`，随后 `s3like::CheckPermissions` 根据请求的 `storeapi::Permission` 逐项调用四个检查方法；任一步失败即带权限名返回。
2. 普通对象操作先用 `bucket_prefix.ObjectKey(name)` 生成 key，再构造本 crate `interface.rs` 定义的输入结构并调用 `S3API`。Get 额外通过 `storeapi::GetHTTPRange` 生成 Range；Head/List/Put 在调用前记录对应指标。
3. `CheckPutAndDeleteObject` 先保存 Put 结果，无论 Put 成败都执行 Delete。若 Put 失败，最终保留 Put 错误；只有 Put 成功时 Delete 结果才决定返回值。这对应 Go 的 `defer` 清理优先级。
4. `MultipartWriter` 构造时立即执行 CreateMultipartUpload。每次 `write` 先检查最大 part 数和 context，再以从 1 开始的序号上传；只有成功响应才加入 `complete_parts`。`close` 检查 context 后提交当前 part 列表。
5. `MultipartUploader::upload` 校验 `part_size` 与 `concurrency`，`part_size == 0` 时读取 `s3like::HardcodedChunkSize`。它先将 reader 全部切块并保存在内存，超过 `storeapi::MaxUploadParts` 时在发起 multipart 前失败。
6. uploader 对零块或一块数据直接 Put；两块以上先 CreateMultipartUpload，再按 `concurrency` 大小分批。每批用 `std::thread::scope` 并行 UploadPart，批次之间顺序推进。任一 worker 失败或 panic 时尝试 Abort 并返回原上传错误；全部成功后按 part number 排序并 Complete。
7. 上层 `pkg/objstore/s3like/store.rs::Storage` 将这些原语组成用户操作：`WriteFile` 用 Put，`ReadFile/Open` 用 Get，`WalkDir` 用 List，`Create` 在并发度不大于 1 时选 `MultipartWriter`，否则用 `MultipartUploader` 包装成 `AsyncWriter`。

## 数据与状态

`S3Client` 本身除共享 `Arc<dyn S3API>` 外都是不可变配置，克隆客户端只增加底层 API 的引用计数并复制前缀与配置。所有逻辑名称都应先经过同一个 `BucketPrefix`，这是避免跨前缀访问的主要不变量；复制操作的源 key 使用 `params.FromLoc`，目标 key 使用当前客户端前缀。

`MultipartWriter.complete_parts` 是可变上传状态。part number 恒等于“成功 part 数 + 1”，所以失败 part 不入表，下一次写会复用该序号；`close` 克隆列表交给 Complete。它没有 `closed` 标志，因而与 Go 版本一致，close 后仍可继续写并再次 close；测试 `multipart_writer_preserves_go_post_close_behavior` 明确固定了这一行为。

`MultipartUploader` 自身只保存目标 bucket/key、part size、并发度和兼容标志；一次 `Upload` 的 chunks、create 响应和 completed parts 都是调用栈内状态。当前实现会先物化整个输入为 `Vec<Vec<u8>>`，所以峰值内存至少与对象大小同阶，另有 worker 输入克隆的瞬时开销；这与 Go `manager.Uploader` 的流式缓冲池实现并不相同。

## 依赖与调用关系

上游直接证据如下：

- `pkg/objstore/s3store/store.rs::NewS3Storage` 调用 `S3Client::new`、`s3like::CheckPermissions` 和 `s3like::NewStorage`，是生产装配入口。
- `pkg/objstore/s3like/permission.rs::CheckPermissions` 调用四个权限探测方法。
- `pkg/objstore/s3like/store.rs::Storage` 通过 `PrefixClient` 调用 Put/Get/Delete/Head/List/Copy/Presign 和两类 multipart 构造器，再向 `storeapi::Storage` 暴露统一接口。
- RustCodeGraph 的文件关系显示本文件还被 `pkg/objstore/s3store/client_test.rs` 直接覆盖；图查询也确认 `MultipartUploader::Upload` 的调用者包含本文件的转发方法和 multipart 边界测试。

下游依赖集中在 `crate::S3API`：`head_bucket`、`get_object`、`put_object`、`delete_object(s)`、`head_object`、`list_objects_v2`、`copy_object`、`presign_get_object` 以及 create/upload/complete/abort multipart。公共数据契约来自 `storeapi`（context、权限、前缀、Range、part 上限）、`s3like`（trait、响应与指标）和 `objectio::Writer`。本文件没有直接依赖 AWS SDK 具体 client，因此测试可用 `MockS3` 精确记录请求。

## 错误处理与边界

- 除明确映射外，`S3API` 的 `anyhow::Error` 原样向上传播；Writer 边界通过 `io::Error::other` 转成 I/O 错误。
- `CheckGetObject` 只吞掉错误码 `NoSuchKey`；`IsObjectExists` 只把 `NotFound`、`NoSuchBucket`、`NoSuchKey` 映射为 `false`，如 `AccessDenied` 仍返回错误。
- `DeleteObjects` 对空列表直接成功且不发请求；非空列表的单次最大条数由上层 `s3like::Storage::DeleteFiles` 以 1000 条分批，本方法自身不再次分批。
- Get 的范围采用 `[start_offset, end_offset)` 语义；具体字符串由 `storeapi::GetHTTPRange` 产生。本文件只转发响应元数据，完整性和 Content-Range 校验在 `s3like::Storage::open`。
- `ListObjects` 将 `max_keys` 直接转为 `i32`，可能按 Rust `as` 规则截断/回绕；`copy_source_and_large_list_limit_match_go_conversions` 以 `i32::MAX + 1 → i32::MIN` 固定了 Go 对齐行为，扩展时不应擅自改为拒绝溢出。
- 同步 Writer 在第 10001 个 part 发网络请求前返回 `ErrExceedMaxUploadParts`。并行 uploader 既在切块阶段主动检查，也用字符串匹配规范化 SDK 各阶段的 `MaxUploadParts` 错误。
- uploader 参数要求 `part_size >= 0`、`concurrency > 0`，最终 chunk size 不能为零；显式 part size 还必须能转换为 `usize`。
- 并行 part 失败时 Abort 的错误被有意忽略，以保留原上传错误；同步 `MultipartWriter` 的 write/close 失败不会自动 Abort，调用方若需要清理必须另行设计。
- `CheckPutAndDeleteObject` 的 Delete 会在 Put 失败后照常执行，但清理错误不会覆盖 Put 错误；Put 成功时即使 Delete 返回 `NoSuchKey`，该错误仍返回。独立测试分别覆盖了这两种优先级。

## 并发与资源生命周期

`Arc<dyn S3API>` 让克隆客户端、Writer 和 Uploader 共享同一底层 SDK/运行时；`S3API` trait 的线程安全约束支持 worker 并行调用。`S3Client` 没有锁，操作间没有本地串行化。

同步生命周期是 Create → 多次 UploadPart → Complete。只有成功 part 才进入完成列表，且该列表天然有序；context 在每次 write 和 close 开头检查。当前对象被丢弃时没有 Drop/Abort 钩子，调用方提前放弃会留下由服务端生命周期策略处理的未完成上传。

并行生命周期是“读完整个 reader → Create → 分批并行 UploadPart → 排序 → Complete”，失败路径是“尝试 Abort → 返回原错误”。`std::thread::scope` 保证每批 worker 在离开作用域前全部 join，不会产生脱离调用栈的后台线程；但后续批次必须等待前一批全部结束，实际同时运行的 worker 不超过 `concurrency`。worker panic 被转换为 `multipart worker panicked`。

`GetObject` 返回的 body 所有权移交给上层 `s3like::GetResp`；关闭和读取重试不在本文件进行。`PresignObject` 只生成字符串，没有长驻资源；临时凭证导致实际有效期短于请求有效期的风险仍由上游凭证语义决定。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/objstore/s3store/client.go`。Rust 保留了 `s3Client` 的字段含义、权限检查、前缀拼接、配置字段映射、错误码分类、API 指标、CopySource 的 `path.Join` 清理语义、从 1 开始的 part number，以及 Put/Delete 错误优先级。Rust 的 `join_copy_source` 明确模拟 `path.Join`；相关点段与前导斜线行为由两套 Rust 测试覆盖。

主要实现差异如下：

- Go 直接使用 AWS `manager.Uploader`，配置 `PartSize`、`Concurrency` 和缓冲池；Rust 在本文件中自行读尽、切块、分批启动 scoped threads。因此对外目标语义相同，但内存占用、流水并行和极大对象表现不等价。
- Go 的 Content-MD5 辅助函数修改 SDK middleware stack；Rust 把需求抽象成 `RequestOptions.content_md5`，真正的 SDK 处理位于本 crate 的 `interface.rs`。
- Go `PresignObject` 要求 `svc` 能下转为具体 `*s3.Client`；Rust 将 `presign_get_object` 纳入 `S3API`，因而保持 mockable，具体限制由底层实现决定。
- Go 权限清理会对部分 Delete 错误记录警告；Rust 本层没有对应日志，但返回值优先级与测试固定的 Go 行为一致。
- Go `multipartWriter` 同样没有 close 后禁止写入或失败自动 abort 的状态；Rust 测试特意保留这一点，而不是引入更严格但不兼容的状态机。

## 扩展指南

- 新增单对象请求属性时，优先在 `buildPutObjectInput` 和 `MultipartWriter` 的 Create 输入两处同步接线；若并行 uploader 的 multipart Create 也应继承该属性，还必须修改 `MultipartUploader::upload`。当前并行 Create 使用默认属性，不能假定它已继承 `options`。
- 新增 S3 API 能力应先扩展 `pkg/objstore/s3store/interface.rs::S3API` 及其 AWS/mock 实现，再在 `S3Client` 暴露；若属于所有 S3-like 后端能力，还需同步 `pkg/objstore/s3like/interface.rs::PrefixClient` 和 `s3like::Storage`。
- 修改 key、Range、分页或 CopySource 规则时，应同步 `pkg/objstore/s3store/client_test.rs` 与 `client_1_aster_unit_test.rs`，并与 `client_test.go` 的既有意图对照。不要把测试嵌入本生产文件。
- 修改 multipart 行为时至少覆盖：part 上限、失败 part 不提交、完成列表排序、worker panic、Abort 失败、context 取消、零/单块 Put 快路径、Content-MD5 和 close 后行为。若要改成流式 bounded-memory 实现，应把“与 Go uploader 的内存/流水差异”作为兼容与性能验收项，而不是只验证最终对象内容。
- 更改错误映射时保持精确错误码判断；不要把权限错误误判为对象不存在，也不要用宽泛字符串匹配扩展非 `MaxUploadParts` 错误。
- 修改并发模型前确认 `S3API`、context 和上层 `AsyncWriter` 的线程安全/取消契约；特别要防止 detached worker 在调用返回后继续访问 reader 或 context。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `client.rs` 被识别为 742 行、56 个符号，并列出 `store.rs` 与独立测试等直接使用者。
- RustCodeGraph 源码/关系查询：读取了 `pkg/objstore/s3store/client.rs` 全部 742 行，并查询了 `S3Client`、`MultipartUploader`、`PrefixClient`、`CheckPermissions`、`NewStorage`、`Upload` 相关调用关系。关键链路包括 `NewS3Storage → CheckPermissions/NewStorage`、`Storage → PrefixClient` 各对象操作，以及 `MultipartUploader::Upload → upload → S3API multipart 方法`。
- crate 与模块边界：`pkg/objstore/s3store/Cargo.toml`、`pkg/objstore/s3store/lib.rs`、`pkg/objstore/s3store/store.rs`。
- 公共接口与上层调用：`pkg/objstore/s3like/interface.rs`、`pkg/objstore/s3like/permission.rs`、`pkg/objstore/s3like/store.rs`。
- Go 对照：`pkg/objstore/s3store/client.go` 与 `pkg/objstore/s3store/client_test.go`。
- Rust 独立测试：`pkg/objstore/s3store/client_test.rs` 覆盖权限、Range、删除、存在性、列举、MD5、复制与 part 上限；`pkg/objstore/s3store/client_1_aster_unit_test.rs` 覆盖 Go 转换语义、错误优先级、成功 part 列表、close 后行为以及并行完成排序。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只执行固定 11 章结构检查，并人工检查文档能回答文件位置、运行方式、状态生命周期和安全扩展点。

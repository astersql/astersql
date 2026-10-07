# `pkg/objstore/ossstore/client.rs`

## 文件定位

本文件是 `astersql-objstore-ossstore` crate 的高层 OSS 客户端适配层。crate 入口在 [`lib.rs`](lib.rs)，由其私有声明 `mod client` 并通过 `pub use client::*` 导出本文件的公开符号。它不直接实现 HTTP、签名或重试；这些工作由 [`interface.rs`](interface.rs) 中的 `API` trait 及 `AliyunOssApi` 完成。本文件负责把统一对象存储语义转换成 OSS 请求模型，并把 `Client` 实现为 `s3like::PrefixClient`，从而接入 [`../s3like/store.rs`](../s3like/store.rs) 的读写、列举、复制和创建 Writer 主链。

生产入口是 [`store.rs`](store.rs) 的 `NewOSSStorage`：该函数构造数据访问 API 与公网预签名 API，调用 `Client::with_presign_api`，执行 `s3like::CheckPermissions`，最后将客户端交给 `s3like::NewStorage`。`new_oss_storage_for_test` 则通过 `Client::from_dyn` 注入测试 API。`Cargo.toml` 表明本文件属于 `astersql-objstore-ossstore`，直接依赖同工作区的 `storeapi`、`s3like`、`objectio` 和 `prefetch`；具体 OSS SDK 依赖 `ali-oss-rs` 被封装在 `interface.rs`，没有从本文件泄漏到上层接口。

## 核心职责

- `Client` 保存一个带 Bucket/Prefix 的逻辑位置，把调用方的相对对象名统一经 `BucketPrefix::ObjectKey` 转为完整 OSS Key；权限探测的 List 前缀使用 `PrefixStr`。
- `CheckBucketExistence`、`CheckListObjects`、`CheckGetObject`、`CheckPutAndDeleteObject` 将统一权限检查映射到最小 OSS 操作，并区分“对象不存在”和真正的权限/服务错误。
- `GetObject`、`PutObject`、`DeleteObject(s)`、`HeadObject`、`IsObjectExists`、`ListObjects`、`CopyObject` 与 `PresignObject` 完成普通对象操作的请求/响应转换。
- `MultipartWriter` 提供调用方逐片写入的同步 `objectio::Writer`；私有 `MultipartUploader` 从任意 `Read` 流按配置切片，用有界队列和工作线程并发上传。
- `impl s3like::PrefixClient for Client` 是适配边界：它只转发到同名固有方法，并在上层接口要求可选能力时包装成 `Some`。

## 主要符号

- `NO_SUCH_KEY`：值为 `NoSuchKey`。`CheckGetObject` 将它视为权限检查成功，`IsObjectExists` 将它映射为 `false`；其他错误原样传播。
- `DEFAULT_MULTIPART_SIZE` / `DEFAULT_MULTIPART_CONCURRENCY`：并发上传参数非正数时采用 6 MiB 和 3 个 worker，对齐 Go OSS uploader 默认值。
- `Client { svc, presign_svc, bucket_prefix, options }`：`svc` 执行对象数据操作，`presign_svc` 生成可供外部消费的 URL。两个字段都是 `Arc<dyn API>`，可被客户端克隆并共享。`bucket_prefix` 决定物理桶与键前缀，`options` 为分片上传保留 SSE、KMS Key 和 StorageClass。
- `Client::new` / `Client::from_dyn`：分别接收具体 `API` 类型和 trait object；两者默认让数据与预签名请求共用同一 API。`Client::with_presign_api` 允许生产构造器为预签名 URL 使用独立的公网 endpoint。
- `non_empty`：将空配置字符串转成 `None`，非空字符串复制成 `Some(String)`，用于创建 multipart 会话时避免发送空的 SSE/存储类字段。
- 私有 `MultipartWriter`：持有 API、创建会话响应及按写入顺序积累的 `CompletedPart`。它实现 `objectio::Writer::write/close`。
- 私有 `MultipartUploader`：持有 API、完整 Bucket/Key、S3 配置以及 part size/concurrency。它实现 `s3like::Uploader::Upload`。

公开对象方法的关键语义如下：`GetObject` 用 `storeapi::GetHTTPRange` 生成可选 Range，并返回 body、长度与 Content-Range；`ListObjects` 同时传递 continuation token 与可选 `start_after`，但不剥离返回对象 Key 的仓库前缀；`CopyObject` 分别使用源位置和当前客户端位置拼接源/目标 Key；`HeadObject` 当前仅验证请求成功并返回空的 `HeadObjectResp::default()`，没有暴露底层对象元数据。

## 执行流程

普通请求路径为：`s3like::Storage` 调用 `Arc<dyn PrefixClient>` → 本文件的 `PrefixClient` 实现 → `Client` 固有方法拼接 Bucket/Key 和请求字段 → `Arc<dyn API>` → `interface.rs` 的 `AliyunOssApi`。`PutObject`、`HeadObject`/`IsObjectExists`、`ListObjects` 还分别调用 `s3like::RecordAPICall` 记录对应 OSS API；其他操作在本层没有同类计数。

权限检查由 `store.rs::NewOSSStorage` 在暴露 Storage 前触发。Bucket 检查把 API 返回的 `false` 转成 `s3like::ErrNoSuchBucket`。List 检查只请求一个 Key。Get 检查使用随机探测 Key：成功时关闭响应 body，`NoSuchKey` 也表示调用方具备发起 Get 的能力。Put/Delete 检查无论 Put 成败都会尝试删除同一探测 Key；若两者都失败，返回 Put 错误，若仅 Delete 失败则返回 Delete 错误，并对删除失败记录 warning。

同步分片流程是：`Client::MultipartWriter` 先调用 `initiate_multipart_upload`，写入 SSE/KMS/StorageClass 配置；每次 `write` 以当前成功分片数加一作为序号，上传整段传入切片，成功后保存 ETag；`close` 按保存顺序调用 `complete_multipart_upload`。此对象没有 closed 状态，因此当前实现允许 Close 后继续 Write、再次 Close；`migration_aster_unit_test.rs::multipart_writer_allows_write_and_complete_after_close_like_go` 明确固定了这一 Go 对齐行为。

并发上传流程是：`MultipartUploader::Upload` 先检查 context，规范化 part size/concurrency，再创建 multipart 会话；生产者循环从 `Read` 填满一个 part（末片可较短），通过容量等于 worker 数的 `sync_channel` 施加背压；作用域线程消费 `(part_number, body)` 并调用 `upload_part`。所有 worker 退出后，成功分片按序号排序再 Complete。读取或上传失败时，或者 Complete 返回错误时，会尽力 Abort；Abort 的错误被忽略，主错误保持不变。空输入仍会创建会话并以空 parts 执行 Complete。

## 数据与状态

`Client` 本身只含 `Arc` 和可克隆值，派生 `Clone` 后共享 API 实例而复制 Bucket/Prefix 与 options。对象内容在普通 `PutObject` 中复制为 `Vec<u8>`；Get body 则以 `prefetch::reader::ReadCloser` 所有权交给上层，只有权限探测路径在本文件主动关闭它。

`MultipartWriter.complete_parts` 是单 writer 独占的有序向量，只有上传成功才追加；分片序号由其长度计算。失败的 `write` 不追加状态，`close` 也不会自动 Abort。`MultipartUploader` 的每个 part 都是独立 `Vec<u8>`；内存上界主要由正在填充的一个 part、有界队列和各 worker 持有的 part 构成，因此随 `part_size × concurrency` 增长，而不是缓存整个对象。

并发 uploader 用 `Mutex<Vec<CompletedPart>>` 汇总结果、`Mutex<Option<anyhow::Error>>` 保存首个可见失败，以及 `Mutex<Receiver<_>>` 在多个 worker 间共享标准库单消费者接收端。完成列表的插入顺序不确定，因此 Complete 前必须 `sort_by_key(part_number)`。`options` 只在创建 multipart 会话时读取；运行中不会修改客户端配置。

## 依赖与调用关系

上游生产调用者是 `store.rs::NewOSSStorage` 和测试构造器 `new_oss_storage_for_test`。前者用 `Client::with_presign_api` 区分内部数据 endpoint 与公网签名 endpoint；后者用 `Client::from_dyn`。`s3like::NewStorage` 把 `Client` 擦除为 `PrefixClient` 后，`s3like::Storage` 的 `Create` 根据 `WriterOption.Concurrency` 选择同步 `MultipartWriter`（未配置或并发度 ≤ 1）或 `MultipartUploader` 加 `AsyncWriter`（并发度 > 1）。

直接下游包括：`storeapi::BucketPrefix` 的 `PrefixStr/ObjectKey` 和 `GetHTTPRange`；`s3like` 的统一响应、权限、指标及 uploader trait；`objectio::Writer`；以及本 crate `API` 的 bucket/object CRUD、预签名和 multipart 方法。`API: Send + Sync` 使同一个实现可以安全放入 `Arc` 并跨 worker 使用；真实实现 `AliyunOssApi` 再负责签名、HTTP、重试和 context 检查。

RustCodeGraph 将 `client.rs` 标为 57 个符号，并显示它被 `store.rs`、`interface.rs`、`client_test.rs`、`migration_aster_unit_test.rs` 以及上层 objstore 文件使用。精确查询还定位到本文件两层同名 `MultipartWriter`/`MultipartUploader`（固有方法与 trait 转发方法）、私有状态结构，以及 `store.rs::NewOSSStorage`；源码检索确认生产构造边为 `NewOSSStorage → Client::with_presign_api → s3like::NewStorage`。

## 错误处理与边界

所有服务错误以 `anyhow::Result` 传播；只有 `NoSuchKey` 在权限探测和存在性查询中被特判。`CheckGetObject` 忽略 body 的 `close` 错误。`CheckPutAndDeleteObject` 总是清理探测对象，删除错误会告警，并按“Put 错误优先”返回。批量删除空切片直接成功且不访问 API。

`ListObjects` 将 `isize max_keys` 直接 `as i32` 转换，越界时保持 Rust 的截断语义；`migration_aster_unit_test.rs::list_max_keys_uses_go_int32_conversion` 以 `i32::MAX + 1 → i32::MIN` 固定了与 Go `int32` 转换一致的行为。`MultipartUploader` 对正的过大 `part_size` 使用 `usize::try_from` 报错，但不会预先校验 OSS 最小/最大分片限制或内存可承受范围；这些限制最终可能由分配行为或下游 API 体现。worker 数从正 `i32` 转为 `usize` 后至少为 1。

并发上传的 Abort 是 best effort；Abort 自身失败不会替换原来的读取、上传或 Complete 错误。相反，同步 `MultipartWriter` 在 UploadPart 或 Complete 失败时不自动 Abort，这与对应 Go 包装器一致，但可能留下需由 OSS 生命周期规则回收的未完成会话。内部 Mutex 均使用 `unwrap`，若 worker panic 导致锁中毒，当前代码会 panic 而非转为 `anyhow::Error`。分片计数分别使用 `wrapping_add`（writer）和 `saturating_add`（uploader），未主动执行 OSS 分片总数上限检查。

## 并发与资源生命周期

`Client` 通过 `Arc<dyn API>` 共享底层连接与凭证状态；`API` 的 `Send + Sync` 约束支持跨线程调用。并发 uploader 使用 `std::thread::scope`，因此所有 worker 必须在 `Upload` 返回前结束，且可以安全借用 `self`、context 和 create 响应。容量等于 worker 数的同步通道限制排队分片；生产者遇到满队列时检查共享失败状态并 `yield_now`，不会无限扩充队列。发送端在作用域末尾显式 drop，使 worker 在队列耗尽后退出。

`Upload` 入口调用一次 `ctx.check()`；之后同一个 context 传给每个 API 调用，实际取消响应依赖 `API` 实现。同步 `MultipartWriter` 不在本层预检 context，而是把 context 直接委托给 API；测试替身可选择忽略取消，这正是 `multipart_writer_delegates_cancelled_context_to_api_like_go` 所验证的边界。

Get 返回的 body 生命周期属于调用方。权限 Get 是例外：本层立即关闭。Multipart 会话从 initiate 成功开始，到 Complete 成功或并发 uploader 尝试 Abort 为止；同步 writer 被直接丢弃、写片失败或 Complete 失败都没有 Drop 清理逻辑。预签名 API 与数据 API 可分离，但都随最后一个 `Arc` 释放。

## 与 Go 版本的对应关系

直接对照文件是 [`client.go`](client.go)，测试对照是 [`client_test.go`](client_test.go)。Rust 保留了 Go `client` 的字段角色、公开方法名、BucketPrefix 拼接、权限探测、Range/List/Copy 映射、同步 multipart writer 的分片顺序及 Close 后可继续使用等语义。`client_test.rs` 与 `migration_aster_unit_test.rs` 分别覆盖错误映射和迁移边界，并没有把测试嵌入生产源文件。

主要实现差异来自 SDK：Go `multipartUploader.Upload` 委托阿里云 SDK `Uploader.UploadFrom`；Rust 为保持默认分片大小、并发度、流式背压、错误 Abort 与顺序 Complete，在本文件自行实现工作线程流水线。Go 的请求/响应使用 OSS SDK 类型，Rust 用 `interface.rs` 定义的 SDK 无关输入输出，再由 `AliyunOssApi` 转换。Go `client.options` 是指针，Rust 保存 `S3` 值副本；因此客户端构造后的外部配置变更不会影响 Rust 实例。

Go 预签名方法记录了临时凭证有效期可能短于请求过期时间的 TODO；Rust 的 `PresignObject` 同样仅把 `expire` 交给 API，没有在本层校验凭证剩余寿命。生产 `store.rs` 通过独立公网 `presign_svc` 保留了 URL 可从 VPC 外使用的意图。Rust 的 `HeadObject` 与 Go 一样仅返回空响应。Rust `CheckPutAndDeleteObject` 虽不用 Go `defer`，但先保存 Put 结果、随后无条件 Delete，返回优先级与 Go 一致。

## 扩展指南

新增普通 OSS 操作时，先在 `interface.rs::API` 增加 SDK 无关请求/响应与真实实现，再在 `Client` 固有方法完成 Bucket/Prefix 和统一类型转换，最后同步扩展 `s3like::PrefixClient`（若属于跨后端能力）及 `s3like::Storage`。不要让 `ali-oss-rs` 类型穿过本 crate 的高层边界。任何新的相对对象名必须明确经哪个 `BucketPrefix` 拼接，跨桶操作尤其要区分源位置与目标位置。

调整权限语义时同步检查 `s3like::CheckPermissions` 和 `client_test.rs::test_client_permission`，保持 `NoSuchKey`、清理探测对象和双重失败优先级。扩充 Head 元数据需同时修改 `API::head_object` 返回类型、`s3like::HeadObjectResp` 及 Go 对齐测试，不能只填本层默认对象。

修改 multipart 时应分别覆盖两条路径：同步 writer 的序号、ETag、重复 Close、失败后会话生命周期；并发 uploader 的默认参数、短读/EOF、读取失败、任一 worker 上传失败、乱序完成后的排序、Complete 失败和 Abort 失败。对应独立测试优先放在 `client_test.rs`；需要锁定 Go 迁移细节时补在 `migration_aster_unit_test.rs`，不要把测试写回 `client.rs`。增大默认 part size/concurrency 或改变队列策略前评估 `part_size × concurrency` 的峰值内存、线程数、OSS 分片限制及取消延迟。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `client.rs` 含 57 个符号；`files --filter pkg/objstore/ossstore` 确认本 crate 的 Rust/Go 源与独立测试；`node --file pkg/objstore/ossstore/client.rs --offset 1 --limit 520` 和 `--offset 521 --limit 200` 覆盖目标文件全部 668 行；`query MultipartWriter`、`query MultipartUploader`、`query CheckBucketExistence`、`query OssStorage` 用于消除同名符号并定位接口及构造入口。图的 `callers/callees` 命令本轮未返回可用文本，因此调用边又由下列直接源码位置交叉核验。
- Rust 源码：[`client.rs`](client.rs)；crate/构造边界 [`lib.rs`](lib.rs)、[`store.rs`](store.rs)、[`interface.rs`](interface.rs)；上层消费边 [`../s3like/interface.rs`](../s3like/interface.rs)、[`../s3like/store.rs`](../s3like/store.rs)、[`../s3like/permission.rs`](../s3like/permission.rs)。目标目录没有 `doc.go`。
- Rust 独立测试：[`client_test.rs`](client_test.rs) 验证权限、Range、删除、存在性、List、Copy、预签名及并发 uploader 的默认值/失败 Abort；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 验证 List 整数转换、SSE/存储类、分片顺序、重复 Close 与 context 委托。
- Go 对照：[`client.go`](client.go) 与 [`client_test.go`](client_test.go)，用于核对方法职责、错误语义、SDK uploader 行为和预签名限制。
- Cargo 边界：[`Cargo.toml`](Cargo.toml) 声明 crate 名、`lib.rs` 入口、关闭自动测试，以及 `objectio`/`s3like`/`storeapi` 工作区依赖和 `ali-oss-rs` blocking/rust-tls 特性。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的正则结构检查，要求固定的十一个二级标题恰好各出现一次，并人工复核没有把推测写成已支持行为。

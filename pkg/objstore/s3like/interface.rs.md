# `pkg/objstore/s3like/interface.rs`

## 文件定位

`interface.rs` 是 `astersql-objstore-s3like` crate 的后端无关契约层。`pkg/objstore/s3like/lib.rs` 以 `mod interface` 装入并 `pub use interface::*` 重导出本文件的 API，因此 S3、OSS 和上层对象存储逻辑通过 `s3like::*` 使用它，而不直接依赖具体 SDK。crate 边界由 `pkg/objstore/s3like/Cargo.toml` 确认：本文件直接使用 `anyhow`、`objectio`、`prefetch` 和 `storeapi`。

它不执行网络请求，也不实现重试、分页或分片算法；它定义请求/响应数据、`Uploader` 和 `PrefixClient` trait，并为 `Box<T>` 提供透明转发。真实实现位于 `pkg/objstore/s3store/client.rs` 与 `pkg/objstore/ossstore/client.rs`；主要消费者是 `pkg/objstore/s3like/store.rs`、`permission.rs` 和 `io.rs`。

## 核心职责

- 用 `PrefixClient: Send + Sync` 统一桶存在性与权限探测、对象 CRUD、Head、分页列举、服务端复制、分片写入和预签名 URL 能力。“Prefix”的含义是具体客户端已绑定桶和公共前缀，调用者传入的 `name`/`extraPrefix` 是相对于该前缀的对象名。
- 用 `GetResp`、`HeadObjectResp`、`Object`、`ListResp` 和 `CopyInput` 隔离不同云 SDK 的结构差异，使 `Storage` 能以统一数据模型处理 Range、复制状态和分页。
- 用 `Uploader: Send + Sync` 把“从 `Read` 持续取数并上传”抽象为单一操作，供 `io.rs::AsyncWriter` 在后台线程中执行。
- 保留 Go API 的命名与形状，同时用 `Result`、`Option`、trait object 和 `Duration` 表达 Rust 侧错误、可缺值与动态分派。

## 主要符号

- `OSSProvider` / `KS3SDKProvider`：值分别为 `"oss-sdk"` 和 `"ks3-sdk"` 的 provider 标识。`KS3SDKProvider` 被 `pkg/objstore/s3store/ks3.rs` 用于派生 KS3 配置；两个常量也在 Go 同路径文件中定义。
- `ErrNoSuchBucket: &str`：桶不存在的标准文案，`pkg/objstore/ossstore/client.rs::CheckBucketExistence` 将它包装为 `anyhow::Error`。它是文本常量，不是可按类型 downcast 的错误值。
- `GetResp`：`Body: Box<dyn ReadCloser>` 拥有响应流；`IsFullRange` 选择元数据解析路径；全量响应使用 `ContentLength`，部分响应使用 `ContentRange`。`store.rs::open` 会校验返回 Range 与请求的半开区间 `[startOffset, endOffset)` 一致。
- `HeadObjectResp`：仅保留 `ReplicationStatus`；`Storage::FileSynced` 将 `COMPLETE`/`COMPLETED`/`REPLICA` 视为完成，`PENDING` 视为未完成，其他值报错。
- `Object` 与 `ListResp`：分别表示对象键/字节数和一页结果。`NextContinuationToken` 携带下页令牌，`IsTruncated` 决定是否继续，两者由 `Storage::WalkDir` 消费。
- `CopyInput`：`FromLoc: storeapi::BucketPrefix` 定位源桶/前缀，`FromKey` 和 `ToKey` 是源、目标相对键。`Storage::CopyFrom` 从 `BucketPrefixProvider` 和 `CopySpec` 组装它。
- `Uploader::Upload(&self, ctx, reader)`：同步 trait 方法，实现可自行分块和并发；S3 与 OSS 的 `MultipartUploader` 均实现该方法。
- `PrefixClient`：本文件的主契约。`PresignObject` 是唯一带默认体的方法，默认返回 `S3-compatible storage does not support PresignFile`；S3/OSS 真实客户端会覆写它。
- `impl<T: PrefixClient + ?Sized> PrefixClient for Box<T>`：完整转发所有方法，包括默认方法 `PresignObject`，保证 `Box<dyn PrefixClient>` 本身也满足泛型 trait bound，且不改变底层实现的返回值或错误。

## 执行流程

1. 具体后端在 `s3store/client.rs` 或 `ossstore/client.rs` 中实现 `PrefixClient`，将 trait 方法转发到自身的 SDK 适配方法，并把必有的响应包成 `Some(...)`。
2. `s3like::NewStorage` 接收 `C: PrefixClient + 'static`，把它放入 `Arc<dyn PrefixClient>`；后续的 `WriteFile`、`ReadFile`、`WalkDir`、`Open`、`Create`、`CopyFrom` 和 `PresignFile` 只依赖本契约。
3. 读取时，`GetObject` 返回 `GetResp`。全量读取使用 `ContentLength`，Range 读取解析 `ContentRange`；对象体由 `ReadCloser` 流式消费并显式 `close`。
4. 列举时，`WalkDir` 首页传 `startAfter`，后续页传 `NextContinuationToken`，遍历 `Objects` 并在 `IsTruncated == false` 时结束。
5. 写入时，`Storage::Create` 在并发度不大于 1 时请求 `MultipartWriter`；否则请求 `MultipartUploader`，由 `AsyncWriter` 用管道把前台写入流交给后台 `Upload`。
6. 权限检查时，`permission.rs::CheckPermissions` 按输入顺序将四种 `storeapi::Permission` 映射到对应的 `Check*` 方法，第一个错误会包装权限名并短路返回。

## 数据与状态

本文件不保存全局可变状态。三个 provider/错误常量是静态字符串；响应结构都是按次所有的值。

`GetResp::Body` 是不可克隆的所有权资源，与元数据共同描述一次 Get；`ContentLength`/`ContentRange` 的 `Option` 并非任意可缺：当 `IsFullRange` 为 true 时，上层要求 `ContentLength` 存在；否则要求 `ContentRange` 可解析。`ListResp` 的分页不变量是：只要 `IsTruncated` 为 true，实现就应提供能定位下一页的 continuation token。

trait 方法中若干结果是 `Result<Option<T>>` 或 `Option<Box<...>>`，但当前 `Storage` 路径在“无错误但返回 `None`”时使用 `expect` 视为合同违反并 panic。当前 S3/OSS 适配层都将成功值包为 `Some`，没有把 `None` 当成正常的“不存在”信号。

## 依赖与调用关系

- 上游装配：`pkg/objstore/s3like/lib.rs` 公开重导出所有符号。`pkg/objstore/s3like/mock/client_mock.rs` 基于同一 `PrefixClient` 生成 mockall 测试替身。
- 上层消费：`store.rs::Storage` 以 `Arc<dyn PrefixClient>` 共享客户端；`permission.rs::CheckPermissions` 使用权限子集；`io.rs::AsyncWriter` 拥有 `Box<dyn Uploader>`。
- 下游类型：`storeapi::Context` 携带取消/截止语义，`storeapi::BucketPrefix` 描述桶与前缀，`objectio::Writer` 是同步分片 writer 返回类型，`prefetch::reader::ReadCloser` 合并 `Read` 与显式关闭能力。`anyhow::Result` 是所有可失败操作的统一错误通道。
- 实现者：`s3store/client.rs::S3Client` 和 `ossstore/client.rs::Client` 实现完整 `PrefixClient`；两者的内部 `MultipartUploader` 实现 `Uploader`。测试中还有 `s3like/migration_aster_unit_test.rs::MockClient`、`io_test.rs::ReopenFailClient` 和 mock crate 的替身实现。

RustCodeGraph 索引将 `PrefixClient` 定位在第 97 行、`Uploader` 定位在第 89 行、`GetResp` 定位在第 39 行；但本次 `callers`/`callees` 命令未返回边，因此上述调用关系用实现文件中的 trait impl 和消费点直接核验，不将缺失的图边解读为“无调用者”。

## 错误处理与边界

- trait 本身不吞掉错误；除无错误通道的 `MultipartUploader` 构造外，所有可失败操作均返回 `anyhow::Result`。`Box<T>` 实现也原样转发。
- `PresignObject` 的默认实现是显式“不支持”错误，而非空 URL 或 panic。新后端若未支持预签名，可安全继承该默认行为。
- `GetObject`、`HeadObject`、`ListObjects`、`MultipartWriter` 的成功 `None` 和 `MultipartUploader` 的 `None` 在类型上可表示，但当前主路径会 panic；实现者必须避免这一结果，或在改变契约时同步修改消费者。
- `GetResp` 的 Range 元数据错误由 `Storage::open` 报告，而非在数据结构构造时校验。`endOffset == 0` 按当前注释和实现表示读到对象末尾。
- `ErrNoSuchBucket` 与 Go 的 sentinel error 不完全等价：Rust 侧只能按文本约定构造/识别，不应文档化为强类型错误。

## 并发与资源生命周期

`PrefixClient` 和 `Uploader` 都要求 `Send + Sync`，使实现可跨线程传递并通过 `Arc<dyn PrefixClient>` 共享。这是实现者的线程安全承诺：SDK 客户端或内部可变状态必须满足相应同步约束。

`GetResp::Body` 的所有权随响应移交给读取路径。`Storage::doReadFile` 在每次读尽尝试后调用 `close`；`Storage::Open` 则将 body 交给 `S3ObjectReader`，由 reader 关闭路径管理。新实现不能假定 `Drop` 已等价于 SDK 所需的显式 close。

`MultipartUploader` 仅构造并返回可拥有的 uploader；真实上传生命周期在 `io.rs::AsyncWriter`：创建 pipe，spawn 线程调用 `Upload`，close 时先 drop 写端以产生 EOF，再 join 线程并传播上传错误或 panic。`Context` 以引用传入普通操作，而异步 writer 创建时会取得可移入线程的所有权值。

## 与 Go 版本的对应关系

Go 基准文件是 `pkg/objstore/s3like/interface.go`。Rust 保留了常量、五个数据类型、`Uploader` 与 `PrefixClient` 的主体方法集，也保留 Go 式公开命名，因而使用 `#![allow(non_snake_case, non_upper_case_globals)]`。具体映射包括：`io.ReadCloser` 对应 `Box<dyn ReadCloser>`，指针字段对应 `Option`，Go `error` 对应 `anyhow::Result`，Go interface 对应 trait object。

需要明确的差异有：

- Go `ErrNoSuchBucket` 是 `errors.New(...)` 创建的 error 值，Rust 对应项是 `&str`。
- Go 返回 `*GetResp`/`*HeadObjectResp`/`*ListResp` 及可为 nil 的 writer/uploader；Rust 用 `Option` 暴露同类状态，但现有主路径把 `None` 当成合同违反。
- Go `ListObjects` 的 `maxKeys` 是 `int`，Rust 为 `isize`；Go uploader 的 concurrency 是 `int`，Rust 为 `i32`。具体 SDK 适配层负责边界转换。
- Go 注释明确 `Upload` 应在独立 goroutine 运行；Rust trait 本身保持同步，由 `AsyncWriter` 的 OS 线程实现这一并发语义。
- Rust `PrefixClient` 多出 `PresignObject`；Go 的同路径 interface 未声明此方法，Go S3/OSS 具体 client 各自提供预签名方法。Rust 把该能力纳入统一 trait，并用默认不支持维持兼容。
- Rust 额外实现 `PrefixClient for Box<T>`；Go interface 值天然可持有指针实现，不需要等价转发层。

## 扩展指南

1. 新增 S3-like 后端时，实现全部必需 `PrefixClient` 方法与 `Uploader`，保证实现类型 `Send + Sync`；仅在真正支持预签名时覆写 `PresignObject`。
2. 新增 trait 方法时，必须同步修改 `impl PrefixClient for Box<T>`、S3/OSS 实现、`pkg/objstore/s3like/mock/client_mock.rs` 以及所有独立测试替身；否则编译或动态分派将不完整。
3. 修改 Get/Range 契约时，同步检查 `store.rs::open`、`doReadFile`、`io.rs::S3ObjectReader`、`io_test.rs` 和 `migration_aster_unit_test.rs`，特别是 `endOffset == 0`、空对象、Content-Range 解析与 body close/reopen 语义。
4. 修改列举契约时，保持 `startAfter` 仅用于首页、token 用于后续页、`IsTruncated` 控制终止，并扩展 `walk_dir_paginates_trims_prefix_and_skips_empty_directories`。
5. 修改分片上传时，同时验证同步 `MultipartWriter` 和并发 `MultipartUploader` 两路，包括部件大小、并发度、Context 取消、EOF、abort/complete 和后台线程错误传播。
6. Rust 单元测试应继续放在独立文件，而不嵌入 `interface.rs`。首选扩展 `pkg/objstore/s3like/migration_aster_unit_test.rs`、`io_test.rs`、`permission_test.rs` 或 mock crate 的 `migration_aster_unit_test.rs`；SDK 转换边界则在 `s3store/client_test.rs`、`client_1_aster_unit_test.rs` 或 `ossstore/client_test.rs` 中验证。

主要兼容性风险是改变 Go 对齐的方法语义或 `None` 契约；正确性风险是 Range/分页元数据不一致；性能风险是将流式上传退化为全量缓冲、破坏并发限制或无界分片。

## 验证依据

- 目标源文件：`pkg/objstore/s3like/interface.rs`，核对了 3 个常量、5 个数据结构、2 个 trait、`PresignObject` 默认实现和 `Box<T>` 全量转发实现；文件无条件编译项。
- RustCodeGraph：`status` 显示目标在已索引的 `pkg/objstore/s3like` 23 个文件中，目标文件有 39 个符号；`query PrefixClient --kind trait`、`query Uploader --kind trait`、`query GetResp --kind struct` 分别确认本文件的 trait/结构体节点。`explore`、`node --file`、`callers`、`callees` 本次未返回可用内容，调用边因此改由源码引用点核验。
- crate/模块边界：`pkg/objstore/s3like/Cargo.toml` 与 `pkg/objstore/s3like/lib.rs`。
- 主要消费路径：`pkg/objstore/s3like/store.rs`、`permission.rs`、`io.rs`；真实实现：`pkg/objstore/s3store/client.rs`、`pkg/objstore/ossstore/client.rs`。
- Go 对照：`pkg/objstore/s3like/interface.go`；Go SDK 行为测试参考：`pkg/objstore/s3store/client_test.go` 与 `pkg/objstore/ossstore/client_test.go`。
- Rust 独立测试：`pkg/objstore/s3like/migration_aster_unit_test.rs` 验证权限短路、Range 读取、分页、复制状态和两种分片写入路径；`io_test.rs` 验证 reopen 错误与预签名前置校验；`permission_test.rs` 验证权限映射；`mock/migration_aster_unit_test.rs` 验证 mock 返回值、错误及 multipart 组件。SDK 边界由 `s3store/client_test.rs`、`client_1_aster_unit_test.rs` 和 `ossstore/client_test.rs` 覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用任务指定的 11 章结构命令完成机械验证，并人工复核每项结论均可回溯到上述符号、实现或测试。

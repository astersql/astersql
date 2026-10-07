# `pkg/objstore/storeapi/storage.rs`

## 文件定位

本文件是 `astersql-objstore-storeapi` crate 的核心契约文件。crate 入口
`pkg/objstore/storeapi/lib.rs` 公开 `storage` 模块并通过 `pub use storage::*`
重导出这里的全部公共符号；`pkg/objstore/storeapi/Cargo.toml` 则把该 crate
限定为一个很薄的对象存储 API 层，只直接依赖 `anyhow`、AWS Smithy 重试/HTTP
类型、`objectio` 和 `uuid`。

它位于“调用方所需的统一文件语义”和“本地文件、S3-like、GCS、Azure、HDFS
等具体后端”之间：上层通过 `Storage`/`StorageRef` 操作对象，下层实现 trait。
例如 `pkg/objstore/local.rs::LocalStorage`、`pkg/objstore/memstore.rs::MemStorage`、
`pkg/objstore/s3like/store.rs::Storage`、`pkg/objstore/gcs.rs::GCSStorage`、
`pkg/objstore/azblob.rs::AzureBlobStorage` 和 `pkg/objstore/hdfs.rs::HDFSStorage`
都实现了这里的 `Storage`。本文件不选择后端、不解析存储 URI，也不执行网络
请求；这些职责属于 `pkg/objstore/storage.rs` 及各后端 crate/模块。

## 核心职责

1. 用 `Storage` 定义完整文件读写、流式打开/创建、存在性检查、删除、遍历、
   重命名、预签名 URL 和关闭资源的统一行为面。
2. 用 `ReaderOption`、`WriterOption`、`WalkOption`、`Options` 和 `CopySpec`
   在不绑定具体后端的前提下传递范围读取、分片、列举、权限检查、重试和复制参数。
3. 用 `Uploader`、`Copier`、`StrongConsistency`、`Retryer` 和
   `ReadSeekCloser` 表达可选能力或辅助协议；能力是否可用仍由具体后端决定。
4. 用 `Prefix`/`BucketPrefix` 集中维护对象 key 的前缀不变量，并用
   `GetHTTPRange` 把应用的半开区间转换为 HTTP Range 头。
5. 统一权限探测名、权限探测临时 key，以及多段上传的 10,000 分片上限和可
   类型识别的错误哨兵。

## 主要符号

- `Permission` 及常量 `AccessBuckets`、`ListObjects`、`GetObject`、
  `PutObject`、`PutAndDeleteObject`：枚举创建后端时要验证的权限；`as_str`
  保持 Go 字符串值，其中 `AccessBuckets` 的值是单数形式 `AccessBucket`。
- `Storage: Send + Sync`：主接口。完整文件方法是 `WriteFile`/`ReadFile`；
  流式方法是 `Open -> Box<dyn objectio::Reader>` 和
  `Create -> Box<dyn objectio::Writer>`；其余方法覆盖检查、删除、遍历、URI、
  重命名、预签名和资源释放。`AccessRequestSnapshot` 是带默认实现的 Rust
  扩展点，不支持统计的后端返回 `None`。
- `StorageRef = Arc<dyn Storage>`：供并行编码、排序等工作线程共享同一后端的
  线程安全句柄；trait 自身的 `Send + Sync` 是该别名可跨线程共享的前提。
- `WalkOption`：用 `SubDir`、`SkipSubDir`、`ObjPrefix`、`ListCount`、
  `IncludeTombstone`、`StartAfter` 控制列举范围和分页。字段是请求提示，实际
  支持程度由后端决定；例如注释明确 `SkipSubDir` 当前只对本地存储有效。
- `ReaderOption`：`StartOffset` 为包含端、`EndOffset` 为排除端；正数
  `PrefetchSize` 请求预取 reader。`WriterOption` 提供并发数与分片大小。
- `Options`：后端构造的公共配置，包含凭据传播标志、无凭据标志、Smithy HTTP
  客户端、权限列表、可注入 S3 `Retryer`、对象锁检查开关和共享
  `AccessStats`。
- `Retryer: Send + Sync`：返回 AWS Smithy `RetryConfig`，并可覆盖
  `retry_classifier`；默认分类器为 `None`。真实实现之一是
  `pkg/objstore/s3store/retry.rs::S3StandardRetryer`。
- `Uploader`：按顺序接受 `UploadPart`，最终由 `CompleteUpload` 提交；
  `Copier::CopyFrom` 用 `CopySpec { From, To }` 描述从另一个 `Storage` 到当前
  存储的复制。接口没有承诺事务性或失败后的自动清理。
- `StrongConsistency`：仅以 `MarkStrongConsistency` 标记实现的读、写、遍历
  一致性能力。已知实现包括 GCS、Azure、KS3 和 S3-like 存储。
- `NewPrefix`、`Prefix::{JoinStr,ObjectKey,ToPath,String,as_str}` 与
  `NewBucketPrefix`、`BucketPrefix::{ObjectKey,PrefixStr}`：规范化和复用 bucket
  内 key 前缀。
- `GetHTTPRange`：把 `[startOffset, endOffset)` 转成 HTTP 两端闭区间，或生成
  从起点到对象末尾的范围。
- `GenPermCheckObjectKey`：生成 `perm-check/<UUID v4>`，避免并发权限探测相互
  覆盖。
- `MaxUploadParts = 10_000`、`ExceedMaxUploadParts` 和
  `ErrExceedMaxUploadParts`：S3/GCS/OSS 共用的单对象分片上限及可穿过
  `io::Error`/`anyhow` 后继续按类型识别的零尺寸错误值。

## 执行流程

典型的存储使用流程如下：

1. 上层构造 `Options`，指定是否传播凭据、需探测的 `Permission`、HTTP 客户端
   与可选重试器；后端构造逻辑读取这些字段并创建具体 `Storage`。
2. 调用方持有具体类型或 `StorageRef`。小对象走 `WriteFile`/`ReadFile`；流式
   数据通过 `Create` 获得 writer 或通过 `Open` 获得 reader。调用方负责按
   `objectio` 接口完成/关闭流。
3. 范围读取时，后端从 `ReaderOption` 取出包含起点和排除终点，再调用
   `GetHTTPRange`：`end > start` 生成 `bytes=start-(end-1)`；`(0, 0)` 表示
   完整读取且不发送 Range；其他情况生成 `bytes=start-`。OSS 的直接调用点是
   `pkg/objstore/ossstore/client.rs::GetObject`，S3 的对应调用点位于
   `pkg/objstore/s3store/client.rs`。
4. 权限检查用 `GenPermCheckObjectKey` 产生隔离的临时对象名。例如 OSS
   `CheckGetObject` 与 `CheckPutAndDeleteObject` 分别在读取探测和写删探测中使用；
   S3 客户端也使用同一生成器。
5. 列举由 `WalkDir` 将可再次传给 `Open` 的相对路径及大小逐个交给回调；回调
   返回错误时，该错误通过 `Result` 终止遍历。
6. 多段上传实现必须在追加分片时检查 `MaxUploadParts`。S3 客户端在超过上限时
   返回 `ErrExceedMaxUploadParts`，使上层可以区别容量上限与普通传输错误。

前缀的独立流程是：`NewPrefix` 去掉输入两端所有 `/`；空值保持空字符串，非空
值追加恰好一个尾 `/`。`JoinStr` 先规范化右值再直接拼接，`ObjectKey` 则保留
传入对象名原貌，因此以 `/` 开头的对象名会有意产生双斜杠。

## 数据与状态

本文件几乎没有可变全局状态。`Permission`、各 Option、`CopySpec`、`Prefix`
和 `BucketPrefix` 都是值对象；`Prefix` 的关键不变量是“空，或无前导斜杠且有
一个尾斜杠”。该不变量由 `NewPrefix` 建立，但 `Prefix(pub String)` 的字段公开，
外部仍可绕过构造函数创建非规范值，扩展代码不应依赖编译器强制该不变量。

共享状态存在于引用字段中：`StorageRef` 用 `Arc` 共享后端；`Options::S3Retryer`
和 `Options::AccessRecording` 也用 `Arc` 共享重试策略及访问计数器；Smithy
`SharedHttpClient` 自带共享句柄语义。本文件只携带这些句柄，不负责更新计数、
管理连接池或实现锁。

`ReaderOption` 使用 `Option<i64>` 保留 Go 指针的“未提供”状态，而不是用零值
替代；`Storage::{Open,Create,WalkDir}` 本身又接受可选 option，从而区分“整个
选项对象缺失”和“对象存在但字段为默认值”。`WalkDir` 的 `FnMut` 回调允许调用方
在遍历过程中积累状态，但调用是同步借用，不能被存储实现长期保存。

## 依赖与调用关系

向下依赖：

- `objectio::Context` 被重导出并贯穿所有可能阻塞的操作，用于传播取消/超时；
  `objectio::Reader`/`Writer` 是流式 I/O 的实际接口。
- `anyhow::Result` 统一方法错误通道，使具体 SDK、文件系统、回调和哨兵错误能
  保留错误链。
- `aws_smithy_types::retry::RetryConfig` 与
  `aws_smithy_runtime_api` 的共享 HTTP 客户端/分类器形成 S3 重试接线边界。
- `uuid::Uuid::new_v4` 是权限探测 key 的唯一性来源；`AccessStats` 是访问记录
  的共享状态类型。

向上调用和实现关系：

- `pkg/objstore/storeapi/lib.rs` 重导出本文件；Cargo 搜索显示 objstore、
  s3store、ossstore、s3like、DXF import-into、ingestor、executor、DDL、session、
  BR metautil/config 和 dumpling 等 crate 直接依赖该 API crate。
- `pkg/objstore/storage.rs` 负责依据后端配置返回对象存储句柄；
  `pkg/objstore/locking.rs` 广泛使用 `StorageRef` 与 `WalkOption` 实现远程锁文件。
- `pkg/objstore/compress.rs` 包装一个 `Storage` 并转发/改变读写行为，说明 trait
  也是装饰器边界，不只是云 SDK 边界。
- `pkg/objstore/ossstore/store.rs` 使用 `NewPrefix`/`NewBucketPrefix` 创建规范化
  位置；`ossstore/client.rs` 使用范围和权限辅助函数。
- `pkg/objstore/s3store/client.rs` 使用范围、权限和分片上限符号，并把超过限制的
  SDK/本地条件归一为类型化哨兵。

RustCodeGraph 的文件索引报告本文件有 70 个符号并被 10 个已索引文件直接使用；
精确 `query` 能区分 Rust/Go 的 `NewPrefix`、`NewBucketPrefix`、`GetHTTPRange`
和 `GenPermCheckObjectKey`。本次索引的通用 `callers/callees` 命令未能按符号 ID
消歧，因而具体边以上述源码引用搜索复核，而没有采用其无关同名结果。

## 错误处理与边界

- 除 `Storage::Close` 外，可能失败的 I/O、回调、上传、复制和 reader 关闭操作
  都返回 `anyhow::Result`；接口不规定重试、幂等或错误种类，具体后端必须保留
  足够上下文。
- `WriteFile` 明确要求完整文件写入具有原子语义；`DeleteFiles`、`Rename`、
  `CopyFrom` 和分片上传没有跨后端统一的原子保证，不能从方法名推导事务性。
- `PresignFile` 是能力边界：S3 可返回预签名 URL、本地可返回文件名，而 Azure、
  HDFS 等不支持后端应返回错误。
- `GetHTTPRange` 不校验负偏移或 `end < start`。除 `(0,0)` 外，所有
  `end <= start` 都退化为 `bytes=start-`；调用方必须在需要时先做参数校验。
- `Prefix::ObjectKey` 不清理对象名的前导 `/`，双斜杠是为保持 Go 兼容而保留的
  既有行为；它也不验证 bucket 或路径字符合法性。
- `GenPermCheckObjectKey` 依赖随机 UUID 降低碰撞风险，但本文件不负责删除探测
  对象；实际权限检查流程必须在失败路径上完成后端清理。
- `ExceedMaxUploadParts` 实现 `std::error::Error` 且可复制；上游应优先按类型链
  检测，而不是只比较错误字符串。`dumpling/export` 和 S3 测试已有这种用法。

## 并发与资源生命周期

`Storage: Send + Sync` 与 `StorageRef = Arc<dyn Storage>` 允许多个工作线程共享
同一个后端，但这只规定实现必须线程安全，并不保证操作顺序、对象级互斥或批量
原子性。具体后端若持有客户端、连接池或计数器，必须在内部同步。

`Context` 以借用形式传入每次操作；`Open` 的注释允许部分实现把给定上下文保存
为 reader 的内部上下文，因此调用方不应在流尚未结束时主动使相关取消/超时状态
失效。`Create`/`Open` 返回的 trait object 由调用方持有，其完成、刷新和关闭由
`objectio` 的 writer/reader 生命周期约束；`Storage::Close` 则释放后端级连接或
句柄，接口未提供“关闭后仍可调用”的保证。

`Uploader` 用 `&mut self` 串行修改单个上传会话的分片状态；并发上传能力由
`WriterOption::Concurrency` 和具体 writer 实现管理，不能把同一 uploader 的
可变引用并发共享。`WalkDir` 的 `&mut dyn FnMut` 同样表示单次遍历串行调用回调。
权限探测通过每次生成新 UUID 避免并发检查写入同一 key。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/storeapi/storage.go`，Rust 总体保留了 Go 的公开命名
和语义：权限字符串、`WalkOption`/`ReaderOption`/`WriterOption`、`Uploader`、
`Copier`、`CopySpec`、`Storage`、`Options`、前缀规范化、HTTP Range 转换、
权限探测 key 与 10,000 分片限制均一一对应。

关键语言映射包括：

- Go `context.Context` 对应 `objectio::Context` 借用；Go `error` 对应
  `anyhow::Result`。
- Go 的 `*ReaderOption` 及内部 `*int64` 分别对应外层 `Option<&ReaderOption>`
  和字段 `Option<i64>`；语义上的“未设置”没有丢失。
- Go interface 对应 Rust trait object；Go 中天然可共享的 interface 在 Rust 中
  通过 `Send + Sync` 和 `Arc<dyn Storage>` 显式表达。
- Go `aws.Retryer` 字段在 Rust 中收窄为本地 `Retryer` trait，并直接产出 Smithy
  重试配置/分类器，以适配 Rust AWS SDK。
- Go 的 `recording.AccessStats` 指针对应 Rust `Arc<AccessStats>`；Go HTTP
  客户端指针对应 Smithy `SharedHttpClient`。
- Go `errors.New` 哨兵在 Rust 中是实现 `Error` 的类型化零尺寸值，因此可经包装
  后用类型向下检查。

Rust 还增加了 `Storage::AccessRequestSnapshot`、`StorageRef`、`Prefix::as_str`
和独立 `ReadSeekCloser` 抽象；它们是 Rust 接线需要，并非 Go 文件中的同名公开
成员。字段名和方法名有意保留 Go 风格，文件级 `allow(non_snake_case,
non_upper_case_globals)` 为此关闭相应 lint。

`pkg/objstore/storeapi/storage_test.rs` 直接复刻 Go `storage_test.go` 的前缀与范围
案例；`migration_aster_unit_test.rs` 另覆盖 `ToPath`、bucket 前缀、权限常量、
`end == start` 和 UUID 唯一性/格式，因此是 Rust 特有接线的补充证据。

## 扩展指南

- 新增后端：实现完整 `Storage`，并按能力选择实现 `StrongConsistency` 或
  `Copier`；测试应放在同目录独立 `*_test.rs` 文件，不要内嵌到生产源文件。
  至少验证原子写、范围边界、遍历回调错误、覆盖写、关闭、预签名不支持路径及
  并发共享。若新增公开实现文件，还要同步 crate 模块/Cargo 接线。
- 新增通用配置：优先扩展 `Options` 或对应细粒度 Option，并同步所有结构体字面量
  和 Go `Options`。注意新增非可选字段会破坏大量后端及测试构造；优先提供明确
  默认值并评估凭据泄露、重试放大和连接复用风险。
- 修改范围读取：以 `ReaderOption` 的半开区间为唯一上层契约，在
  `GetHTTPRange` 做协议转换；同步 `storage_test.rs`、
  `migration_aster_unit_test.rs`、Go `storage_test.go` 以及 S3/OSS/GCS 后端测试。
- 修改前缀：从 `NewPrefix`、`Prefix::join` 和 `ObjectKey` 入手，并保留或明确迁移
  双斜杠兼容行为。同步 Prefix/BucketPrefix 两组 Rust 独立测试与 Go 测试。
- 修改分片限制：同时检查 `MaxUploadParts`、`ErrExceedMaxUploadParts`、S3/GCS/OSS
  writer 以及 dumpling 的错误注释/检测逻辑；风险包括超出云服务硬限制、分片大小
  与内存/吞吐变化、错误类型在包装中丢失。
- 新增 trait 方法时，优先提供可证明安全的默认实现，否则所有真实后端、装饰器、
  mock 和测试替身都必须同步。兼容性审查应覆盖 Cargo 搜索列出的直接依赖 crate。

## 验证依据

- 源码与模块：`pkg/objstore/storeapi/storage.rs`（342 行、70 个索引符号）、
  `pkg/objstore/storeapi/lib.rs`、`pkg/objstore/storeapi/Cargo.toml`。
- Go 对照：`pkg/objstore/storeapi/storage.go` 与
  `pkg/objstore/storeapi/storage_test.go`。
- Rust 独立测试：`pkg/objstore/storeapi/storage_test.rs`、
  `pkg/objstore/storeapi/migration_aster_unit_test.rs`；后者验证权限值、前缀路径、
  相等范围和 UUID，前者验证 Go 基准案例。
- 真实实现/调用点：`pkg/objstore/local.rs`、`memstore.rs`、`hdfs.rs`、`gcs.rs`、
  `azblob.rs`、`s3like/store.rs`、`s3store/client.rs`、`s3store/retry.rs`、
  `ossstore/client.rs`、`ossstore/store.rs`、`compress.rs`、`locking.rs`。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点；`files --filter
  pkg/objstore/storeapi` 找到 6 个相关文件；`node --file ...storage.rs` 完整读取
  1–342 行；`query` 精确定位 Rust/Go 同名辅助函数。调用图消歧失败的限制已在
  “依赖与调用关系”说明，相关边改由 `rg` 的精确符号引用和实现搜索核验。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前执行任务指定的结构命令，
  确认目标文件存在且恰好包含本页 11 个固定二级章节，并检查 Git 差异仅包含本
  文档及完成后删除的任务文件。

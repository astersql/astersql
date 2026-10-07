# `pkg/objstore/s3like/store.rs`

## 文件定位

本文件位于 `astersql-objstore-s3like` crate（`pkg/objstore/s3like/Cargo.toml`），是 S3 协议兼容对象存储的高层存储适配器。模块入口 `pkg/objstore/s3like/lib.rs` 将本文件声明为私有 `store` 模块后完整再导出，因此 `Storage`、`NewStorage`、`S3BackendOptions`、`ParseRangeInfo` 等成为 crate 的公共 API。

它处在两层抽象之间：下层是 `pkg/objstore/s3like/interface.rs` 定义的 `PrefixClient`，由 AWS S3、阿里云 OSS 等具体客户端实现；上层是 `pkg/objstore/storeapi/storage.rs` 定义的统一 `storeapi::Storage`。RustCodeGraph 对本文件的文件节点显示直接使用方包括 `pkg/objstore/s3store/store.rs`、`pkg/objstore/ossstore/store.rs`、`pkg/objstore/s3store/s3_test.rs`、`pkg/objstore/objectio/writer_test.rs` 和 `pkg/dxf/importinto/collect_conflicts.rs`。其中前两个生产调用方分别在创建真实 SDK 客户端、完成权限检查或凭证初始化后调用 `s3like::NewStorage`，所以本文件不负责网络客户端的构造与鉴权，而负责统一对象语义、范围校验、分页、重试、缓冲和配置转换。

文件没有 feature 或平台条件编译项；测试由 `lib.rs` 通过 `#[cfg(test)]` 引入独立文件，未与生产实现混写。

## 核心职责

1. `Storage` 保存 `Arc<dyn PrefixClient>`、规范化的 `BucketPrefix`、S3 protobuf 风格配置和可选访问统计，将底层客户端适配为 `storeapi::Storage`。
2. 提供整对象读写、单个/批量删除、存在性检查、复制状态查询、服务端复制、分页列举、范围读取、分片写入、非原子重命名和预签名下载。
3. 在高层补充底层 SDK 无法完整覆盖的策略：`ReadFile` 的 HTTP/2 中断重试、正文读取重试、错误上下文、访问字节统计、Range 响应一致性校验和空目录占位过滤。
4. 用 `S3BackendOptions` 承接命令行/URL 配置，校验 endpoint 与静态凭证成对性，再写入 `backuppb::S3`；同时决定 path-style 与 virtual-host-style 寻址。
5. 解析 HTTP `Content-Range` 并以 `RangeInfo` 保存闭区间和对象总长，供本文件的 `open` 以及 `S3ObjectReader` 使用。

`MarkStrongConsistency` 和 `Close` 是有意的空实现：前者把现代 S3 的强一致性能力暴露为 marker，后者说明该包装层本身没有独立连接、线程或句柄需要关闭。它们不是未完成的资源管理逻辑。

## 主要符号

- 全局参数与限制：`HardcodedChunkSize` 和 `WriteBufferSize` 默认均为 5 MiB；本文件实际在 `Create` 中读取 `WriteBufferSize`，`HardcodedChunkSize` 是向 crate 使用方保留的公共兼容参数。`MAX_ERROR_RETRIES = 3` 控制正文读取尝试次数，`s3DeleteObjectsLimit = 1000` 控制每批删除数量。`MAX_SKIP_OFFSET_BY_READ` 在同 crate 的读取器实现中使用。两个可变全局均为 `static mut`，访问或修改需要调用方自行保证同步。
- 配置键：`S3AccessKey`、`S3SecretAccessKey`、`S3RoleARN`、`S3ExternalID` 是公开查询参数名；`s3.*` 私有常量供 `DefineS3Flags` 与 `ParseFromFlags` 共用。
- `Storage`：可克隆的存储句柄。克隆会共享 `s3Cli` 和 `accessRec` 的 `Arc`，但复制 `BucketPrefix` 与 `backuppb::S3` 值。
- `NewStorage<C: PrefixClient + 'static>`：把具体客户端封装进 `Arc<dyn PrefixClient>`。AWS 路径由 `pkg/objstore/s3store/store.rs` 调用，OSS 路径由 `pkg/objstore/ossstore/store.rs` 调用。
- `BucketPrefixProvider`：`CopyFrom` 所需的窄接口；`Storage` 自身实现它，以便获取源 bucket/prefix。
- `Storage` 固有方法：`GetOptions`、`GetBucketPrefix`、`WriteFile`、`ReadFile`/`doReadFile`、`DeleteFile(s)`、`FileExists`、`FileSynced`、`WalkDir`、`URI`、`Open`/`open`、`Create`、`Rename`、`PresignFile`、`CopyFrom` 和 `Close`。
- trait 适配：`impl storeapi::StrongConsistency for Storage` 和 `impl storeapi::Storage for Storage` 主要把统一 trait 调用转发到同名固有方法；trait 的 `Open`/`Create` 会克隆借用的 `Context`，`WalkDir` 会桥接动态回调。
- `S3BackendOptions`：保留 endpoint、region、存储类型、SSE/KMS、ACL、静态/临时凭证、provider、寻址方式、AssumeRole、profile 和对象锁配置。`Apply` 写入 `backuppb::S3`，但当前不会把 `UseAccelerateEndpoint` 和 `ObjectLockEnabled` 写入目标；前者只参与寻址决策，后者由具体后端构建流程使用。
- `DefineS3Flags`/`ParseFromFlags`：定义并读取十个字符串 flag。`ParseFromFlags` 将 `ForcePathStyle` 初始化为 `true`；AccessKey、SecretAccessKey、SessionToken、加速端点和对象锁不是这里定义的 `s3.*` flag。
- `RangeInfo { Start, End, Size }`：`Start`、`End` 是包含端点，`Size` 是完整对象长度；`RangeSize` 用 wrapping 运算模拟 Go `int64` 溢出行为。
- `ParseRangeInfo`：只接受严格匹配 `bytes <start>-<end>/<size>` 的十进制非负数字格式。`isCancelError` 以错误字符串是否包含 `context canceled` 判断取消。

## 执行流程

构造链从具体后端开始：`s3store::NewS3Storage` 或 OSS 构造逻辑创建实现 `PrefixClient` 的客户端和 `BucketPrefix`，再调用 `NewStorage`。上层持有 `Storage` 或 `dyn storeapi::Storage` 后，CRUD 调用被转发到底层客户端。

整文件读取分两层。`ReadFile` 调用 `doReadFile`；成功后记录读字节。若整个调用返回 HTTP/2 connection aborted，则休眠 10 ms 并最多重试五次。每次 `doReadFile` 最多进行三次 `GetObject(ctx, file, 0, 0)`：读尽 `Body` 后无论读成功与否都调用 `close`；正文错误若是 deadline 或 context canceled 立即返回，否则通过 `RecordRetryableError` 记数后重取对象。`GetObject` 本身失败不会进入正文重试，而会立即附加 bucket/key 上下文返回。

范围读取由 `Open` 解析 `ReaderOption`：缺省起止偏移均为 0，负的预取大小归零。私有 `open` 发起 `GetObject`；完整响应必须给出 `ContentLength`，空对象使用 `(0,0,0)`，非完整响应则调用 `ParseRangeInfo`。随后验证请求的半开区间 `[startOffset,endOffset)` 与响应的闭区间一致。验证通过后，可选用 `prefetch::reader::NewReader` 包装正文，最终构造 `S3ObjectReader`；后者持有克隆的 `Storage`，可在 seek 或可重试读错时重新打开对象。

`WalkDir` 将 `SubDir` 与 `ObjPrefix` 合成额外前缀，`ListCount <= 0` 时每页请求 1000 项。第一页带 `StartAfter`，后续页清空它并传 `NextContinuationToken`。每个返回 key 会移除客户端公共前缀及紧随其后的 `/`；大小不大于零且以 `/` 结尾的目录占位对象被跳过。用户回调的首个错误立即终止遍历。

写入有两条路径。`Create` 在未给选项或 `Concurrency <= 1` 时调用同步 `MultipartWriter`；并发度大于 1 时用 `MultipartUploader(name, PartSize, Concurrency)` 构造 `AsyncWriter`。两条路径最后都由 `objectio::NewBufferedWriter` 包装，缓冲大小优先采用正的 `PartSize`，否则读取全局 `WriteBufferSize`，压缩类型固定为 `NoCompression`，并传入访问统计。`WriteFile` 则直接 `PutObject` 并在成功后记写字节。

`Rename` 依次执行完整读取、新键完整写入、旧键删除；任一步失败即返回，所以它既不是服务端复制，也不是原子操作。`CopyFrom` 才使用 `CopyObject`，源位置由 `BucketPrefixProvider` 给出。`DeleteFiles` 用 `chunks(1000)` 顺序删除，任一批失败即停止。

配置链是 `DefineS3Flags` 注册默认值，调用方覆盖 flag 后由 `ParseFromFlags` 填入 `S3BackendOptions`，再由 `SetForcePathStyle` 结合 provider、加速端点、AWS 域名/RoleARN 以及原始 URL 中是否显式指定 force-path-style 作调整，最后 `Apply` 校验并写入 `backuppb::S3`。

## 数据与状态

`Storage` 没有内部可变业务状态；共享可变性位于 trait 对象实现和 `AccessStats` 内部。`options` 是构造时的快照，`GetOptions` 只返回不可变引用。`bucketPrefix` 同时用于 URI 展示、列举结果裁剪和读错误中的完整对象键说明，但真正给对象名增加前缀是具体 `PrefixClient` 的职责。

读取统计仅在 `ReadFile` 最终成功后以完整数据长度递增；写统计在 `WriteFile` 成功后递增，而流式 `Create` 的统计由 `NewBufferedWriter` 处理。失败重试不会在本文件重复计入成功读字节。`URI` 直接格式化为 `s3://{Bucket}/{PrefixStr}`，是否有尾斜杠取决于 `BucketPrefix` 的规范化结果。

`RangeInfo` 明确区分响应闭区间 `[Start, End]` 与调用 API 的半开区间 `[startOffset, endOffset)`。`endOffset == 0` 表示不约束请求终点。空对象特判避免产生 `End = -1`。`RangeSize` 使用 `wrapping_add`/`wrapping_sub`，在极端非法值上也保持 Go `int64` 的回绕语义，而不是 debug build panic。

进程级 `WriteBufferSize` 与 `HardcodedChunkSize` 是无锁 `static mut`。本文件只在创建 writer 时读取前者，因此更改只影响之后创建的 writer，不会改变已创建 writer；并发写这些全局值会产生 Rust 层面的安全风险，不能把它们当成线程安全的动态配置。

## 依赖与调用关系

上游生产调用边：

- `pkg/objstore/s3store/store.rs` 在完成 bucket、region、权限和可选对象锁探测后，以 `S3Client` 调用 `s3like::NewStorage`；测试构造器也走同一入口。
- `pkg/objstore/ossstore/store.rs` 以 OSS `Client` 调用 `s3like::NewStorage`，`OSSStore` 包装并复用本文件的高层行为。
- 统一调用方通过 `storeapi::Storage` 使用读写、列举、writer、rename、presign 等能力；RustCodeGraph 文件节点还确认了 DXF 冲突收集与 objectio writer 测试的直接引用。

主要下游调用边：

- `PrefixClient`：`PutObject`、`GetObject`、`DeleteObject(s)`、`HeadObject`、`IsObjectExists`、`ListObjects`、`CopyObject`、`MultipartWriter`、`MultipartUploader`、`PresignObject`。
- `storeapi`：`Context`、`BucketPrefix`、`CopySpec`、`WalkOption`、`ReaderOption`、`WriterOption`、`Storage` 与 `StrongConsistency`。
- `objectio`：统一 `Reader`/`Writer`、`NewBufferedWriter` 和 `recording::AccessStats`。
- crate 内部：`S3ObjectReader`、`AsyncWriter`、`IsHTTP2ConnAborted`、`IsDeadlineExceedError` 与 `RecordRetryableError`。
- 外部 crate：`anyhow` 负责错误传播，`regex`/`LazyLock` 负责一次编译的 Content-Range 正则，`url` 负责 endpoint 完整解析，`prefetch` 负责可选读预取，`fail` 提供 `read-s3-body-failed` 测试故障点。

`Cargo.toml` 还列出 `metricscommon`、`mockall`、`os_pipe`、`prometheus`、`tracing` 等 crate 级依赖，它们由同 crate 其他模块或测试使用，不能据此推断本文件直接调用了它们。`s3like_mock` 是 dev-dependency。

## 错误处理与边界

- `PrefixClient::GetObject`、`HeadObject`、`ListObjects`、`MultipartWriter` 的返回类型允许 `Ok(None)`；本文件把这种违反当前客户端契约的情况视为程序错误并 `expect`，不是可恢复的远端错误。新增客户端必须保证成功时返回 `Some`。`MultipartUploader` 同样要求并发路径返回 `Some`。
- `doReadFile` 给 Get 失败和正文失败附加 bucket/key；deadline 与包含 `context canceled` 的错误不重试，普通正文错误最多三次。错误分类部分依赖字符串，若底层文案变化可能失效。
- 外层 HTTP/2 重试只识别 `IsHTTP2ConnAborted`，最多额外五轮；它与内层三次正文尝试相乘，最坏情况下可能发生多次网络读取。固定 sleep 是同步阻塞当前线程。
- `FileSynced` 仅把 `COMPLETE`、`COMPLETED`、`REPLICA` 视为完成，`PENDING` 视为未完成；`FAILED`、空字符串和未知值均返回带状态的错误。
- `ParseRangeInfo` 不接受通配总长、空格变体、负数或溢出 `i64` 的数字；解析错误会点名 start/end/size。它只解析格式，不验证 `Start <= End < Size`，该语义依赖响应提供者和 `open` 的请求区间校验。
- `open` 对完整响应缺失 `ContentLength`、部分响应缺失/非法 `ContentRange`、响应区间不匹配均报错。返回错误前不会显式关闭已经取得的 `Body`，具体释放依赖 trait 对象析构行为，这是扩展客户端时应关注的资源边界。
- `Apply` 要求非空 endpoint 同时具备合法 scheme 与 host；无 profile 时 AccessKey/SecretAccessKey 必须同时为空或同时存在。有 profile 时允许部分/显式凭证，保持命令行覆盖凭证链的 Go 语义。
- `PresignFile` 拒绝零时长；`Duration` 本身不能表示负数。底层未覆盖默认 `PresignObject` 时返回“不支持”错误。
- `Rename` 的写成功、删失败会同时保留新旧对象；读成功、写失败则旧对象不变。调用方不能依赖原子性。

## 并发与资源生命周期

`Storage` 可克隆且实现的上层 trait 要求 `Send + Sync`；`PrefixClient` 本身也要求 `Send + Sync`，因此多个 `Storage` 克隆共享同一客户端。`AccessStats` 通过 `Arc` 共享。`options` 和 `bucketPrefix` 按值克隆，没有锁。

同步写路径的 multipart 生命周期由底层 `objectio::Writer` 管理；并发路径由 `AsyncWriter` 和 `Uploader` 管理后台上传与关闭时的错误汇总。本文件始终再包一层缓冲 writer，调用者必须执行 `close` 才能完成尾块刷新和 multipart 收尾。`Storage::Close` 不替代 writer 的 `close`。

读取正文在 `doReadFile` 中读尽后立即调用 `Body.close`，且忽略 close 错误，因为此时主要结果由正文读取决定。流式 `Open` 把 `Body` 所有权交给预取层或 `S3ObjectReader`；其关闭、重开与 seek 生命周期由这些 reader 实现负责。预取缓冲只在 `PrefetchSize > 0` 时启用。

`WalkDir`、`DeleteFiles` 和 `ReadFile` 的控制循环均是同步顺序执行；本文件不会并行删除或列举分页。`ReadFile` 的 `thread::sleep` 会阻塞执行线程，不适合直接放在要求全异步、不可阻塞的执行器线程上。

全局 `static mut` 参数没有并发保护。若需要运行时调参，应改为同步原语或把配置下沉到实例，而不是从多个线程直接读写这些变量。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/objstore/s3like/store.go`。Rust 保留了 Go 的公开命名和主流程：`Storage`/`NewStorage`、三层读重试意图、每批 1000 个删除、复制状态映射、`WalkDir` 的 StartAfter/ContinuationToken 切换、Range 解析与校验、同步/并发 multipart 分支、读写删式 `Rename`、path-style 决策以及 flag 到 protobuf 配置的映射。

为匹配 Go 的边界语义，Rust `RangeInfo::RangeSize` 明确使用 wrapping 算术；`S3BackendOptions::Apply` 区分缺 scheme 和缺 host；profile 非空时跳过静态密钥成对校验；endpoint 只去掉一个末尾 `/`，与 Go `strings.TrimSuffix` 一致。

当前实现存在以下语言适配差异，需要按事实理解而非推断为等价源码：

- Go `CopyFrom` 接受任意 `storeapi.Storage` 并运行时检查 `GetBucketPrefix`；Rust 直接要求 `&dyn BucketPrefixProvider`，通过类型边界提前限制调用者，但没有复刻 Go 的“错误类型 + 实际类型”失败分支。
- Go 保存 `*backuppb.S3`，Rust 保存克隆后的 `backuppb::S3` 值；构造后外部修改原配置不会反映到 Rust `Storage`。
- Go 的预签名通过运行时可选接口断言，Rust 将默认不支持实现放到 `PrefixClient::PresignObject`，所以本文件可以直接调用。
- Go 的异步上传在本文件内以 pipe、wait group 启动；Rust 把对应机制封装进 crate 的 `AsyncWriter`。行为入口仍由 `Concurrency > 1` 选择。
- Go 会记录 HTTP/2 重试 warning，并包含 DXF 随机错误注入；Rust 本文件保留固定重试与 failpoint，但没有等价日志和随机注入。
- Go 错误带 PingCAP error 类别；Rust统一为 `anyhow::Error` 文本和上下文，因此调用方不应假设可进行完全相同的错误类型匹配。

`pkg/objstore/s3like/store_test.rs` 单独验证 endpoint 缺 scheme/host 的精确错误；`migration_aster_unit_test.rs` 验证 Go 迁移语义；具体 AWS 适配的更完整回归位于 `pkg/objstore/s3store/s3_test.rs` 和 `s3_flags_test.rs`。Go 同目录没有独立 `store_test.go`，相关 Go 行为测试主要位于具体后端目录；`permission_test.go` 只覆盖权限模块。

## 扩展指南

- 新增 S3-compatible provider：在具体 crate 实现完整 `PrefixClient`，确保所有 `Result<Option<_>>` 方法成功时返回 `Some`，然后经 `NewStorage` 接线。优先在具体后端测试验证 key 前缀、SDK 请求参数和错误映射，再在 s3like 独立测试验证共享行为。
- 新增存储操作：先判断是否属于所有后端共享的 `storeapi::Storage`；若是，需要同步修改 `pkg/objstore/storeapi/storage.rs`、本文件的固有方法和 trait 转发、`PrefixClient` 及各具体客户端。若只属于底层 S3 能力，应优先扩展 `PrefixClient`，避免在本文件耦合具体 SDK 类型。
- 修改读重试：同时审查 `ReadFile`、`doReadFile`、`S3ObjectReader` 及 `retry.rs`，避免不同层重复放大请求；补充 deadline、cancel、HTTP/2 abort、正文读错、close 生命周期与统计不重计测试。测试应放在独立的 `store_test.rs`、`migration_aster_unit_test.rs` 或具体后端测试文件，不能内嵌回生产文件。
- 修改 Range/Open：保持调用端半开区间与 HTTP 闭区间的转换，覆盖空对象、全量响应无 Content-Range、缺 ContentLength、非法/溢出数字、区间不匹配、预取和 seek/reopen。直接测试入口是 `ParseRangeInfo`、`Storage::open` 和 `Storage::Open`。
- 修改上传：保持 `Concurrency <= 1` 与并发分支、`PartSize` 同时控制 uploader 和缓冲大小、writer 必须 close 的契约；回归同步 multipart、并发分片、close 错误、非整分片尾块和 AccessStats。
- 增加 flag/配置字段：同步 `S3BackendOptions`、`DefineS3Flags`、`ParseFromFlags`、`Apply`、`backuppb::S3` 适配结构、具体后端配置转换和 `s3_flags_test.rs`。确认字段究竟是共享 protobuf 字段还是仅影响构造阶段，避免像 `UseAccelerateEndpoint` 一样误写到无对应字段的目标。
- 修改列举：保留首轮 `StartAfter` 与后续 continuation token 的互斥关系、公共前缀裁剪以及目录占位过滤；补充分页中途错误和回调错误的停止行为。
- 若要允许动态修改默认块大小，应先消除 `static mut`，设计实例级或原子配置，并评估公共 API 兼容性和并发性能。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/objstore/s3like` 确认 Rust/Go 源与测试均已索引；`node --file pkg/objstore/s3like/store.rs --offset 1 --limit 500` 和 `--offset 500 --limit 260` 读取完整 716 行及 79 个符号，文件节点给出五个直接使用文件；`query NewStorage`、`query ParseRangeInfo`、`query S3BackendOptions`、`query isCancelError` 用于消歧 Rust/Go 对应符号。当前 CLI 的 `callers`/`callees` 查询未返回可用明细，因此调用关系又以索引文件节点和直接入口源码核验，没有把空图结果写成事实。
- 生产源码：完整阅读 `pkg/objstore/s3like/store.rs`；读取 `interface.rs` 核对 `PrefixClient`、响应和上传抽象；读取 `lib.rs` 核对模块再导出和独立测试装配；读取 `pkg/objstore/storeapi/storage.rs` 的统一 trait 与选项定义；读取 `pkg/objstore/s3store/store.rs`、`pkg/objstore/ossstore/store.rs` 中 `NewStorage` 的生产接线。
- crate 边界：读取 `pkg/objstore/s3like/Cargo.toml`，确认 crate 名、`lib.rs` 入口、直接依赖、mock dev-dependency 与 Go 包迁移元数据；未发现 feature 声明。
- Go 对照：阅读 `pkg/objstore/s3like/store.go` 的构造、配置、CRUD、重试、列举、Range、Create、Rename、Presign 和 Close 实现，逐项核对 Rust 迁移行为与差异。
- Rust 测试：读取 `pkg/objstore/s3like/store_test.rs`；读取 `migration_aster_unit_test.rs` 中 Range/配置、wrapping、flags/path-style、1000 分批删除、复制状态、分页、Open 重试/seek、同步与并发上传用例；读取 `pkg/objstore/s3store/s3_test.rs` 和 `s3_flags_test.rs` 中真实适配的 URI、请求参数、统计、错误、Range、WalkDir、上传和配置覆盖。测试证据说明这些行为已有独立测试，但本次是纯文档任务，按计划未运行 Cargo。
- 人工复核：本文分别回答了文件存在目的、构造与读写运行链、状态/资源生命周期、Go 对齐差异、错误边界及安全扩展时需修改的符号和独立测试位置；所有“当前支持”结论均可回溯到上述符号或测试。

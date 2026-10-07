# `pkg/objstore/gcs.rs`

源文件：[`gcs.rs`](./gcs.rs)

## 文件定位

本文件是 `astersql-objstore` crate 的 Google Cloud Storage（GCS）后端实现。crate 根模块在 [`lib.rs`](./lib.rs) 中以 `pub mod gcs` 暴露它；统一接口来自独立子 crate [`storeapi/storage.rs`](./storeapi/storage.rs) 的 `storeapi::Storage` 与 `storeapi::StrongConsistency`。它位于 URI/后端配置解析与 `object_store` GCP 客户端之间：调用者提交 `GCSConfig` 和相对对象名，本文件负责构造客户端、规范化对象键，并把统一存储操作转成 `object_store::ObjectStore` 调用。

当前可确认的应用入口是 [`pkg/session/runtime/load_data.rs`](../session/runtime/load_data.rs) 的远端 `LOAD DATA` GCS 分支：`ParseBackend` 产生 `StorageBackend::Gcs` 后，代码调用 `new_gcs_storage`，再用 `WalkDir` 和 `ReadFile` 枚举、读取输入对象。RustCodeGraph 还显示本文件被 `pkg/objstore/parse_test.rs` 和 `tests/realtikvtest/importintotest4/recorded_summary_harness.rs` 引用。该实现不是门面或桩，但通用 [`storage.rs`](./storage.rs) 的另一套兼容工厂通过 `external_factory` 注入云后端，不能据此假定所有 Rust 云存储构造路径都会自动进入本文件。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义：包名为 `astersql-objstore`，依赖开启 `gcp` feature 的 `object_store = 0.14`，并依赖本地 `objectio`、`storeapi` 子 crate；异步 GCS API 由本文件持有的 `ObjectStorageCore` runtime 同步等待。

## 核心职责

- `GCSBackendOptions` 读取四个 GCS flag，并把 endpoint、存储类、ACL 和凭据文件内容应用到 `GCSConfig`。
- `new_gcs_storage` 校验配置，用 `GoogleCloudStorageBuilder` 创建真实 GCS `ObjectStore`，按选项处理签名/凭据保留，并安装 GET 预签名闭包。
- `GCSStorage` 实现完整 `storeapi::Storage` 操作面：整对象读写、存在性检查、幂等删除、范围读取、遍历、流式创建、重命名、URI 与预签名 URL。
- `object_name`/`object_path` 把存储前缀和调用者给出的逻辑名映射为实际对象键；所有对象操作均复用这一映射。
- `AccessRecorder` 及读写包装器提供请求数和有效载荷字节数的线程安全近似统计。
- `should_retry` 对网络断开、部分 HTTP/2 文本错误和 401 文本错误作可重试分类；它是公开的分类器，但本文件没有显式把它安装到 `object_store` 客户端。

## 主要符号

- 四个公开 option 常量 `GCS_ENDPOINT_OPTION`、`GCS_STORAGE_CLASS_OPTION`、`GCS_PREDEFINED_ACL_OPTION`、`GCS_CREDENTIALS_FILE_OPTION` 是 `FlagSet` 的键；`GCS_CLIENT_COUNT = 16` 保留 Go 侧客户端数量的可观察语义。
- `GCSConfig` 保存 endpoint、bucket、prefix、storage class、predefined ACL 和凭据 JSON。结构体可克隆、序列化/反序列化，既是构造输入，也是 `GCSStorage::options` 返回的只读配置。
- `GCSBackendOptions::{parse_from_flags, apply}` 分别完成 flag 取值与配置落地。`apply` 只在 `credentials_file` 非空时读取文件，读取失败附加具体文件路径。
- `AccessRecorder` 用四个 `AtomicU64` 记录读请求、写请求、读字节、写字节；`snapshot` 返回 `(read_requests, write_requests, read_bytes, write_bytes)`。
- `RecordingReader` 包装 `objectio::Reader`，维护当前位置、总大小和“当前 range 是否已记请求”状态；有效 seek 会让下一次非 EOF 读取重新计一次请求。
- `RecordingWriter` 累积成功写入的字节数，并仅在底层 `close` 成功后记一次写请求；`recorded` 防止重复关闭重复计数。
- `Presigner` 是 `Fn(&object_store::path::Path, Duration) -> anyhow::Result<String> + Send + Sync` 的闭包类型。
- `GCSStorage` 持有配置、共享 `ObjectStorageCore`、可选 recorder、可选 presigner 和语义客户端数。`with_store` 是注入任意 `ObjectStore` 的构造入口，主要用于独立测试；`new_gcs_storage` 是真实 GCS 构造入口。
- `GCSStorage::{copy_from, reset, mark_strong_consistency}` 是 `Storage` trait 以外的公开辅助能力。`copy_from` 同一底层 store 时调用服务端风格 `core.copy`，不同 store 时退化为内存中的 `get` 后 `put`。
- `impl storeapi::Storage for GCSStorage` 提供 `AccessRequestSnapshot`、`WriteFile`、`ReadFile`、`FileExists`、`DeleteFile(s)`、`Open`、`WalkDir`、`URI`、`Create`、`Rename`、`PresignFile` 和 `Close`。
- `should_retry` 先匹配若干 `io::ErrorKind`，再匹配已知错误消息片段；普通错误返回 `false`。

本文件没有条件编译项；测试专用注入和 multipart 故障包装位于独立的 [`gcs_test.rs`](./gcs_test.rs) 以及 [`gcs_extra.rs`](./gcs_extra.rs)。

## 执行流程

1. 配置解析时，`GCSBackendOptions::parse_from_flags` 从 `FlagSet` 取得四项字符串；`apply` 拷贝普通字段，并在配置了路径时把整个凭据文件读入 `credentials_blob`。
2. `new_gcs_storage` 首先拒绝空 bucket；当 `StorageOptions.send_credentials` 为真时，还要求调用者已经提供 `credentials_blob`。随后从环境创建 `GoogleCloudStorageBuilder`，覆盖 bucket、可选 endpoint、显式服务账号 JSON，或在 `no_credentials` 时跳过签名。
3. builder 成功后，构造结果被同时作为 `ObjectStorageCore` 的 store 和预签名闭包的具体 GCS signer。若不需要向下游发送凭据，函数在保存配置前清空 `credentials_blob`；这只影响 `GCSStorage::options` 中保留的配置，不撤销已交给 builder 的凭据。
4. 普通对象操作先调用 `Context::check`，再用 `object_path` 定位对象。`WriteFile` 通过 `put_with_context` 写完整内容；`ReadFile` 通过 `get` 读完整内容；`FileExists` 把 not-found 转成 `false`；`DeleteFile` 请求忽略不存在错误。
5. `Open` 先 `head` 获得总长度，再把 `ReaderOption` 解释为 `[start, min(end,total))`，创建支持 seek 的 `ObjectStoreReader`。启用 recorder 时，`Open` 的属性请求先计一个零字节读请求，随后各个新 range 的首次 `read` 再计请求和字节。
6. `WalkDir` 组合 `config.prefix`、`WalkOption.SubDir` 和 `ObjPrefix`，列出对象后按完整 location 排序；它移除存储前缀，把相对名和大小交给回调，并在本地再次过滤 `name <= StartAfter` 的项。
7. `Create` 在 `WriterOption.Concurrency > 1` 时选择 multipart：分片大小至少为 `GCS_MINIMUM_CHUNK_SIZE`，底层是 [`gcs_extra.rs`](./gcs_extra.rs) 的 `GCSWriter`，外层再加无压缩缓冲；其他情况使用单对象 `ObjectStoreWriter`。两条路径都会应用 `put_attributes`，启用 recorder 时还会再包一层 `RecordingWriter`。
8. `Rename` 严格按 `ReadFile -> WriteFile -> DeleteFile` 顺序执行，因此不是原子 rename；前两步产生的流量会进入 recorder，目标写入也会重新应用当前 storage class。任一步失败即停止，可能同时保留源对象和已写目标对象。
9. `PresignFile` 检查 context 后调用构造时安装的 signer。通过 `with_store` 构造的实例没有 signer，会返回 `GCS signer is unavailable`。

## 数据与状态

`GCSStorage.config` 是构造完成后的配置快照。bucket 决定客户端目标，prefix 参与每一个对象键；`storage_class` 被 `put_attributes` 转为写入属性。`predefined_acl` 虽然被解析和保留，但当前 `put_attributes` 并未使用它，因此不能把 Go 侧写 ACL 的行为视为 Rust 已支持。

`core` 包含 `Arc<dyn ObjectStore>` 和同步等待异步请求的 runtime。对象本身不维护目录索引、缓存或事务状态；列表、head、get、put、copy、delete 均交给后端。`client_count` 初始化和 `reset` 后都是 16，但 Rust 代码没有 16 个显式 client/handle，也没有轮询索引；该字段只表达兼容语义。

统计状态独立于实际 GCS SDK 指标。计数采用 `Ordering::Relaxed`，保证每个计数器的原子性而不提供跨字段一致快照，所以并发期间 `snapshot` 的四个值可能来自略有差异的时刻。它只统计本层观察到的请求/有效载荷，不等同于网络层重试、协议开销或服务端计费字节。

`RecordingReader.position` 和 `requested` 跟随单个可变 reader；seek 到不同且小于文件总长的位置会把 `requested` 清零。seek 到 EOF 之后的空读不产生新的 range 请求。`RecordingWriter.bytes` 只累加底层成功接受的字节，只有成功关闭才提交统计。

## 依赖与调用关系

上游关系：

- [`lib.rs`](./lib.rs) 公开 `gcs` 模块，并把独立测试文件以 `#[path = "gcs_test.rs"]` 接入 crate 测试。
- [`parse.rs`](./parse.rs) 接受 `gs://`/`gcs://` URI，形成 `StorageBackend::Gcs`；它的配置字段随后可映射为本文件的 `GCSConfig`。
- [`pkg/session/runtime/load_data.rs`](../session/runtime/load_data.rs) 是已核实的生产调用者：它在远端 `LOAD DATA` 中构造匿名 GCS 客户端，对 glob 路径先 `WalkDir`，再逐项 `ReadFile`。
- `storeapi::Storage` 的消费者通过 trait 调用本文件的读写、枚举、创建和预签名能力；`storeapi::StrongConsistency` 则把 GCS 标记为强一致后端。

下游关系：

- [`azblob.rs`](./azblob.rs) 提供共享的 `ObjectStorageCore`、`ObjectStoreReader`、`ObjectStoreWriter`、路径拼接/裁剪、not-found 判断和 storage-class 属性构造。
- [`gcs_extra.rs`](./gcs_extra.rs) 提供 multipart `GCSWriter` 与 5 MiB 最小分片限制；其上传失败、完成失败或 context 取消时负责 abort，`Drop` 也会尽力清理未完成上传。
- `object_store::gcp::GoogleCloudStorageBuilder` 创建真实客户端，`object_store::signer::Signer` 生成签名 GET URL。
- `objectio::Context` 提供取消检查；`objectio::Reader`/`Writer` 是流式边界；`storeapi::{ReaderOption, WriterOption, WalkOption, CopySpec}` 定义接口参数。

RustCodeGraph 对 `gcs.rs` 的文件级引用结果包含 `pkg/session/runtime/load_data.rs`、`pkg/objstore/parse_test.rs` 和 `tests/realtikvtest/importintotest4/recorded_summary_harness.rs`。图查询对常见符号名的全局 `explore` 噪声较大，因此应用主链结论同时以目标文件的精确 `node` 输出和调用点源码复核，没有扩展为未证实的全仓调用关系。

## 错误处理与边界

- `with_store` 和 `new_gcs_storage` 都拒绝空 bucket；前者不会验证 endpoint、prefix、ACL 或凭据格式。
- `GCSBackendOptions::apply` 对凭据文件使用 `anyhow::Context` 加入文件名。空路径不会清空既有 `credentials_blob`。
- `new_gcs_storage` 在 `send_credentials=true` 且 blob 为空时立即失败；builder 失败被包装为 `failed to create GCS client`。`no_credentials` 仅在没有显式 blob 时选择跳过签名。
- 大多数 I/O 方法在发出后端请求前执行 `ctx.check()`；`DeleteFiles` 逐项调用 `DeleteFile`，首个非忽略错误即终止，既不并行也不保证原子性。
- `ReadFile` 为失败添加逻辑对象名；其他 core 错误大多原样传播。`FileExists` 只吞掉可识别的 not-found，权限、网络等错误仍返回调用者。
- `Open` 会把结束偏移裁到文件长度，但没有在本层显式拒绝负起点或起点大于终点，具体结果由 `ObjectStoreReader` 构造/读取逻辑决定。`ReaderOption.PrefetchSize` 当前未使用。
- `WalkDir` 忽略 `SkipSubDir`、`ListCount` 和 `IncludeTombstone`；callback 的任意错误立即中止并返回。列表先整体收集和排序，超大前缀会有内存与排序成本。
- `copy_from` 只有底层 `Arc` 指针相同时走 `core.copy`；不同实例即使指向同一真实 bucket，也会整对象读入内存再写出，并且该退化路径未通过 recorder 包装。
- `Rename` 非原子且可能出现部分完成；`PresignFile` 对测试注入实例明确不可用；`Close` 当前为空操作。
- `should_retry` 的结构化判断仅覆盖五种 `io::ErrorKind`，其余依赖字符串包含匹配，可能随下游错误文本变化而漏判或误判；本文件没有直接消费该函数。

## 并发与资源生命周期

`storeapi::Storage` 要求 `Send + Sync`。`GCSStorage` 通过 `Arc<dyn ObjectStore>`、共享 runtime、`Arc<Presigner>` 与原子 recorder 支持并发借用；配置在构造后只读。`AccessRecorder` 的 relaxed 原子操作适合累计计数，但不构成业务同步原语。

单对象 reader/writer 通过 `&mut self` 串行推进各自位置和累计字节。`Create` 的 `Concurrency > 1` 会选择 multipart/缓冲路径，但直接证据中 `GCSWriter` 的 `workers` 只是校验并保存，单次 `upload_part` 使用 runtime 阻塞等待；不应仅凭字段名声称本文件本身启动了对应数量的上传任务。

multipart 资源的真正生命周期在 [`gcs_extra.rs`](./gcs_extra.rs)：正常非空 close 执行 complete；分片/完成/context 失败执行 abort；空写 close 不 complete 也不 abort；未关闭即 drop 时尽力 abort。相关故障测试验证即使调用 context 已取消，清理仍不被取消阻断。

真实客户端由 `Arc` 引用计数管理。与 Go 版不同，Rust `reset` 不销毁并重建连接，只恢复语义计数并继续使用 `object_store` 内部 HTTP 池；`Close` 为空操作。因而调用者无需、也无法通过 `Close` 主动关停 16 个句柄，资源最终随所有共享引用释放。

## 与 Go 版本的对应关系

直接对照文件是 [`gcs.go`](./gcs.go)，测试对照是 [`gcs_test.go`](./gcs_test.go)。两版都提供 GCS 配置、前缀拼接、完整读写、存在检查、幂等删除、范围 reader、排序/过滤后的遍历、流式 writer、读写删式 rename、预签名 GET、强一致 marker、重试分类和访问统计；Rust 的 [`gcs_test.rs`](./gcs_test.rs) 延续了读写删、1000 对象遍历、seek/range、multipart、并行访问、context 取消、批量删除、统计和重试判定意图。

已验证差异如下：

- Go `GCSStorage` 并行创建 16 个 `storage.Client`/`BucketHandle` 并轮询使用，`Reset` 会关闭并重建它们，`Close` 会取消 client context 并关闭客户端；Rust 依赖 `object_store` 的共享 HTTP 池，`client_count` 只保留数值语义，`reset` 不重建，`Close` 无操作。
- Go 可从默认 Google 凭据发现流程回填 JSON，并支持自定义 HTTP client、权限检查和 SDK retry 安装；Rust builder 从环境读取配置，但本文件只显式处理 service-account blob、skip-signature、endpoint 和 bucket，没有实现 `StorageOptions` 中这些 Go 构造细节的全部等价面。
- Go 的 writer 同时设置 `StorageClass` 和 `PredefinedACL`；Rust 的 `put_attributes` 当前只使用 storage class，ACL 仅存储未应用。
- Go 的 `Open` 保留 `PrefetchSize` 并可创建预取 reader；Rust 忽略该字段。Go 列举把 `StartAfter` 下推为 query start offset，Rust 先 list/sort 再本地过滤。
- Go `CopyFrom` 只接受 `*GCSStorage` 并用 GCS copier；Rust 的公开 `copy_from` 类型上已限定 GCS，但只在共享同一 `ObjectStore` 实例时 `copy`，否则 get+put。
- Go 测试用 fake GCS server 验证更多 SDK 行为；Rust 单元测试主要用 `InMemory`/`LocalFileSystem`，因此不能把它们视为真实 GCS 认证、ACL、权限或传输层兼容性的证明。
- Go seek 到 EOF 后读返回 `EOF` error；Rust `std::io::Read` 语义为成功返回 0，Rust 测试已按该语义断言。

## 扩展指南

- 新增配置项时，同时修改 `GCSConfig`、`GCSBackendOptions`、对应 option 常量、`parse_from_flags`/`apply`、URI 解析映射和真实 builder；若字段影响下游凭据传播，还要复核 `new_gcs_storage` 清理 blob 的时机。
- 要补齐 predefined ACL，应从 `put_attributes` 或 `object_store` GCP 专用写选项接入，并让 `WriteFile`、单写 writer、multipart writer 三条写路径保持一致；先确认 `object_store 0.14` 的 GCP 能力，不要仅保留配置字段便宣称支持。
- 要支持 prefetch、分页或 skip-subdir，应分别在 `Open` 和 `WalkDir` 接入；必须保持 callback 收到的路径可直接交给 `Open`，并覆盖 prefix 有/无尾斜杠、`SubDir`、`ObjPrefix`、`StartAfter` 组合。
- 修改访问统计时要区分逻辑调用、属性请求、range GET、seek 后重开和网络重试。同步扩展 `test_gcs_access_recording`，并保持并发情况下的原子计数语义。
- 修改 `Create` 时要同时检查 [`gcs_extra.rs`](./gcs_extra.rs) 的 5 MiB～5 GiB 分片约束、10000 分片上限、complete/abort/drop 清理和错误 source 链；测试必须留在独立 [`gcs_test.rs`](./gcs_test.rs)，不要内嵌回生产文件。
- 要改变 rename 或跨 store copy，应先确定是否允许大对象整量驻留内存、目标已存在时的覆盖语义，以及失败后源/目标的可见状态；这两项目前都不是事务操作。
- 与 Go 对齐的新行为应先从 [`gcs.go`](./gcs.go) 和 [`gcs_test.go`](./gcs_test.go) 提取原测试意图，再在独立 Rust 测试中覆盖；真实 GCS 专属行为不能只靠 `InMemory` 证明。

主要兼容风险是 ACL、凭据发现、HTTP/权限选项、prefetch 和客户端生命周期仍与 Go 不完全等价；主要性能风险是 `WalkDir` 全量收集排序、跨 store copy 与 rename 整对象入内存；主要正确性风险是 rename 部分完成、字符串式 retry 分类以及范围参数未在本层完整校验。

## 验证依据

本说明依据以下直接证据完成：

- RustCodeGraph `status`：索引包含 11467 个文件、307296 个节点和 1848419 条边；目标 `pkg/objstore/gcs.rs` 与 Rust/Go 对照文件均在索引中。
- RustCodeGraph `files --filter pkg/objstore`：确认 `gcs.rs`、`gcs_extra.rs`、`gcs_test.rs`、`gcs.go`、`gcs_test.go`、crate 入口和两个接口子 crate 的文件布局。
- RustCodeGraph `node --file pkg/objstore/gcs.rs --offset 1 --limit 500` 及 `--offset 480 --limit 90`：阅读全文 542 行，核对全部常量、结构体、impl、trait 方法和 `should_retry`，确认无条件编译项。
- RustCodeGraph `query GCSStorage --kind struct` 与 `query new_gcs_storage --kind function`：区分 Rust/Go 同名实现并定位真实构造函数与测试。
- RustCodeGraph 对 [`lib.rs`](./lib.rs)、[`gcs_extra.rs`](./gcs_extra.rs)、[`storeapi/storage.rs`](./storeapi/storage.rs)、[`parse.rs`](./parse.rs) 和 [`pkg/session/runtime/load_data.rs`](../session/runtime/load_data.rs) 的精确 `node` 读取：核对模块暴露、下游 multipart 生命周期、trait 契约、URI 配置和生产调用点。
- [`Cargo.toml`](./Cargo.toml)：核对 crate 名、`object_store` 的 `gcp` feature、本地 `objectio`/`storeapi` 依赖以及测试不自动发现的边界；目标包不存在 `doc.go`。
- RustCodeGraph 分段读取 [`gcs_test.rs`](./gcs_test.rs) 全部 652 行：核对基本 I/O、范围与 seek、multipart、并行读写、context、批量删除、统计、失败 abort、空上传、取消和最大分片数边界。
- RustCodeGraph 分段读取 [`gcs.go`](./gcs.go) 与 [`gcs_test.go`](./gcs_test.go)：核对 Go 的 16 客户端轮询/重置/关闭、ACL、凭据、prefetch、遍历、rename、预签名、retry 和 fake server 测试语义。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的正则结构检查，要求目标文件存在且恰好包含上述 11 个固定二级标题；同时人工复查文档没有把未接线能力、Go 行为或测试替身行为写成 Rust 已实现事实。

# `br/pkg/restore/log_client/stubs.rs`

## 文件定位

`stubs.rs` 是 Rust crate `astersql-br-pkg-restore-log-client` 的本地兼容边界，由 [`lib.rs`](lib.rs) 通过 `pub mod stubs` 挂载。它不对应 Go 包中的同名源文件，也不是一个可独立部署的 PD、TiKV 或对象存储客户端；它把 Go 版 `br/pkg/restore/log_client` 从多个外部包取得的类型和接口，集中压缩为 Rust 迁移阶段可编译、可注入、可断言的最小实现。

[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 指向 `br/pkg/restore/log_client`，并明确说明 arm64 Darwin 路径不引入 `kvproto`、`grpcio`、`kv` 或 `domain`，而优先使用精简的路径依赖与本地桩。因此，本文件位于日志恢复客户端内部，但承担的是跨越协议消息、存储、checkpoint、stream、SQL session、PD、Importer 和 Region split 的适配职责。

文件已带 `// Copyright 2026 AsterSQL.` 与原 PingCAP Apache License。它是生产 crate 会编译的模块，却主要为移植代码和独立测试提供替身；不能因其位于非测试文件中，就把其中的空操作或固定返回值解释为真实集群能力。

## 核心职责

本文件的职责可以分为六组：

1. 定义跨模块通用错误与取消上下文：`Result<T>`、`Error`、`berrors`、`Context`。
2. 提供 Go/kvproto 形状兼容的数据类型：`metapb`、`errorpb`、`import_sstpb`、`encryptionpb`、`kvrpcpb`、`backuppb`。
3. 提供可观察的内存替身：`storeapi::MemStorage`、`checkpoint::MemLogMetaManager`、`glue::MemSession`、`pd::MemPdClient`、`importclient::MemImporterClient`、`split_client::MemSplitClient`。
4. 提供日志恢复控制流所需的薄辅助：`utils_retry`、`grpc_status`、`multierr`、`stream::MetadataHelper`、`metautil::MetaWriter`、`conn::GetAllTiKVStoresWithRetry`。
5. 为尚未接入真实后端的边界保留 API 形状：`rawkv`、`tidbutil`、`encryption`、`failpoint`、日志/指标/summary 等模块。
6. 将少量已有 Rust 实现再导出到本 crate 的命名空间，例如 `tablecodec` 直接再导出 `astersql_br_pkg_restore_utils::stubs::tablecodec::*`，`backuppb::File` 实现外部 trait `AppliedFile`。

这些职责共同让 `client.rs`、`import.rs`、`import_retry.rs`、`migration.rs`、`log_file_manager.rs` 等文件可以按接近 Go 的名字表达流程，同时把平台相关或尚未移植的依赖隔离在一个明确位置。

## 主要符号

- `pub type Result<T>` 与 `Error { msg, code }`：统一本 crate 的错误返回。`Error::Annotate`/`Wrap` 保留 code 并添加文本上下文，`IsCode` 做精确业务码判断；`From<astersql_errors::SharedError>` 只保留文本，不保留原错误链或分类。
- `Context`：以 `Arc<Mutex<Option<Error>>>` 保存显式取消原因，并可通过 `WithCancellationSource` 接入外部取消查询；`Err` 先检查本地状态，再查询 source，`Done` 等价于 `Err().is_some()`。
- `backuppb` 及相关协议模块：提供 `File`、`Metadata`、`BackupMeta`、`Migration`、`ApplyRequest/ApplyResponse`、Region/Store 等字段形状。`backuppb::File` 的 `AppliedFile` 实现把文件名、CF、大小、键范围、region ID 与重写规则暴露给 restore-utils。
- `storeapi::Storage` / `MemStorage`：抽象 `FileExists`、`ReadFile`、`WriteFile`、`WalkDir`；内存实现用共享 map 保存字节，目录遍历按字典序稳定返回。
- `utils_retry::RetryState` 与 `WithRetryV2`：前者维护尝试次数和指数退避值；后者在测试路径立即重试，不 sleep，并支持一个“命中即停止”的错误分类函数。
- `checkpoint::LogMetaManager` / `MemLogMetaManager`：装载 `(LogRestoreKeyType, LogRestoreValueMarshaled)` 列表、记录 `RestoreProgress`，并可返回共享对象存储。
- `stream::TableMappingManager`、`MetadataHelper`、`IngestedSSTsGroupExt`：承载 ID 映射、元数据解析和 ingested SST 分组所需的最小状态与操作。
- `glue::Session` / `MemSession`、`kv::Storage` / `MemKVStorage`、`domain::Domain`：提供 SQL session、KV storage 和 infoschema 的最小接口面，供 `LogClient` 初始化、ID map 和 schema 处理调用。
- `pd::Client` / `MemPdClient` 与 `pdhttp::Client`：前者返回 cluster ID 和 store 列表；后者可选注入 `ReplicateConfigClient` 后端。
- `importclient::ImporterClient` / `MemImporterClient`：模拟关闭连接、清理文件和应用 KV 文件，并记录调用次数；`fail_apply` 注入传输层错误，`apply_pb_error` 注入响应内协议错误。
- `split_client::SplitClient` / `MemSplitClient`：提供按 ID 查 Region 和范围扫描；`CheckRegionEpoch` 比较 region ID 与 epoch，`PaginateScanRegion` 当前只转发一次 `ScanRegions`。
- `tidbutil::WorkerPool`、`ErrorGroup`、`rawkv::RawKVBatchClient`、`failpoint::Inject`：保持上层签名，但分别同步执行闭包、固定成功、仅设置关闭标记/空写入、完全不触发回调，属于明确的简化实现。

## 执行流程

该文件自身没有统一入口，运行方式由各调用模块选择，典型路径如下：

1. `lib.rs` 将 `stubs` 声明为公开模块，其他生产文件通过 `crate::stubs::*` 引入所需边界。
2. 构造日志恢复客户端时，`client.rs::NewLogClient` 接收 `Arc<dyn pd::Client>` 和 `pdhttp::Client`；测试辅助再注入 `MemStorage`、`MemSession`、`MemLogMetaManager`、`MemSplitClient` 或 `MemImporterClient`。
3. 文件发现和迁移读取经 `storeapi::Storage` 进入 `MemStorage`，`WriteFile` 保存完整字节，`ReadFile` 返回副本，`WalkDir` 根据规范化 prefix 筛选并排序对象名。
4. 日志元数据路径保留 `stream::MetadataHelper::ReadFile` 的完整参数形状，但该默认桩当前忽略 storage/path/offset 并返回空字节；测试中的真实读取由 `export_test.rs::FakeStreamMetadataHelper` 补足。`ParseToMetadata` 则委托 stream crate 校验并用 JSON 补齐 V1 分组，`metautil::MetaWriter` 以 `BackupMeta::Marshal` 的本地二进制格式写回对象存储。
5. checkpoint 路径从 `MemLogMetaManager::LoadCheckpointData` 顺序回放内存记录；回调任一失败即用 `?` 停止。保存进度时锁住 `progress` 并覆盖当前值。
6. 导入路径把请求发送给 `ImporterClient::ApplyKVFile`。内存实现优先返回 `fail_apply`，否则增加 `applied` 计数，并可能把 `apply_pb_error` 放进成功响应，由 `import_retry.rs` 区分传输错误和协议错误。
7. Region 重试路径通过 `SplitClient` 查询或扫描 Region；`MemSplitClient` 从 `by_id` 或 `regions` 克隆结果。当前 `PaginateScanRegion` 不实现真正的多页循环，只转发调用。
8. 关闭或收尾路径调用 Importer/RawKV 的关闭方法、checkpoint runner 的 `WaitForFinish`、metadata helper 的 `Close` 等；部分方法仅保留可观察标志，部分为空操作。

## 数据与状态

主要可变状态都显式放在共享锁中：

- `Context.cancelled` 是 `Arc<Mutex<Option<Error>>>`，克隆 Context 后仍共享取消状态；`source` 也是共享的只读闭包。
- `metrics::Counter`/`Gauge` 使用原子值保存计数或浮点位模式，适合跨线程读取；`Histogram::Observe` 和多数日志/summary 入口不保存事件。
- `MemStorage.files` 是 `Arc<Mutex<HashMap<String, Vec<u8>>>>`，克隆存储句柄会共享同一对象集合，读操作返回字节副本。
- `MemLogMetaManager.data` 与 `progress`、`MemImporterClient` 的关闭/计数/错误注入字段、`MemSplitClient` 的 Region 集合都由 `Mutex` 保护；trait 同时要求 `Send + Sync`。
- `RetryState` 是调用方独占的值状态：每次 `ExponentialBackoff` 先返回当前等待时间，再增加 attempt 并把下一次等待翻倍，封顶于 `max_backoff`。
- `TableMappingManager`、`SchemasReplace`、`BackupMeta` 等结构以拥有式集合保存映射或元数据；许多 protobuf 风格 getter 会克隆 `Vec<u8>` 或整个集合，强调测试稳定性而非零拷贝性能。

关键不变量是“共享句柄看见同一内存状态”和“测试结果可重复”。`WalkDir` 主动排序，计数器使用原子操作，错误注入字段每次读取均克隆但不会自动消费；因此一次注入会持续影响后续调用，除非测试显式清空。

## 依赖与调用关系

直接外部依赖很少：顶层使用标准库 `HashMap`、`Arc`、`Mutex`、`Duration`，并从 `astersql-br-pkg-restore-utils` 引入 `AppliedFile`；部分模块使用 `astersql-errors::SharedError`、`serde`/`serde_json`、`sha2`，以及 workspace 内 stream/checkpoint/restore/utils/iter crate。依赖边界以 [`Cargo.toml`](Cargo.toml) 为准。

上游调用者覆盖几乎整个本 crate：

- `client.rs` 使用 `backuppb`、checkpoint、domain、glue、Importer、KV、PD/PD HTTP、RawKV、storage、stream、worker pool、`Context/Error/Result`。
- `import.rs` 使用协议类型、错误码、连接查询、Importer、Region split、重试、日志与指标。
- `import_retry.rs` 使用 gRPC/Region 错误形状、`multierr`、`RetryState` 和 split client。
- `migration.rs` 与 `log_file_manager.rs` 使用 storage、backup metadata、stream metadata helper 和加密占位。
- `log_split_strategy.rs`、`flow_control.rs`、`id_map.rs`、`batch_meta_processor.rs` 等使用 checkpoint、PD HTTP、tablecodec、session/domain 或基础常量。

RustCodeGraph 查询能定位 `MemImporterClient`（`stubs.rs:2503`）、`WithRetryV2`（`stubs.rs:1348`）、`MemLogMetaManager`（`stubs.rs:1586`）与 `PaginateScanRegion`（`stubs.rs:2690`）；当前索引对这些符号没有输出 callers/callees 边，因此调用关系以生产文件中的 `crate::stubs` import 与独立测试引用交叉核对，而不是臆造图边。

## 错误处理与边界

- `Error` 只有字符串和可选静态 code；`Annotate` 保留 code，但 `From<SharedError>`、JSON/存储转换通常只保留文本。扩展错误分类时需避免在转换处意外丢失 code。
- `MemStorage::ReadFile` 对不存在对象返回 `file not found: <name>`；写入会无条件覆盖；`WalkDir` 的 prefix 去掉尾部 `/` 后匹配同名对象或其子路径。
- 所有 `Mutex::lock()` 都直接 `unwrap()`，锁中毒会 panic，而不是转成 `Error`。这是测试桩边界，不是生产级容错策略。
- `WithRetryV2` 忽略传入 Context，并为每次尝试创建新的 `Context::Background()`；不等待 backoff。`max_retry <= 0` 时闭包不会执行，返回 `retry exhausted`。
- `PrefixNextKey` 从末尾执行无符号加一并传播进位；全 `0xff` 输入会得到原长度的全零再追加一个零，调用者不应自行假定它与所有真实 codec 的前缀上界算法完全等价。
- `BackupMeta::Unmarshal/Marshal`、`MetadataHelper::ParseToMetadata` 等只支持本文件实现的序列化路径；兼容性检查 `CheckBackupMetaCompatibilityFromBytes` 当前固定成功。
- `grpc_status::FromError` 依据错误消息文本识别状态，`multierr` 也只提供最小聚合能力；不能替代真实 gRPC status/error chain。
- 多个方法明确为空或固定成功，包括日志方法、部分 summary/metrics、`CheckpointRunner::WaitForFinish`、`TableMappingManager::CleanTempKV`、`RawKVBatchClient::Put`、`failpoint::Inject`、`ErrorGroup::Wait`。调用方不能把这些返回值当作远端副作用已经发生的证据。
- `PaginateScanRegion` 名称保留 Go 语义，但当前没有分页；`MemSplitClient::ScanRegions` 也不根据 start/end/limit 过滤。需要真实分页行为时必须扩展实现和独立测试。

## 并发与资源生命周期

`Storage`、`LogMetaManager`、`Session`、`Glue`、`kv::Storage`、`pd::Client`、`ImporterClient`、`SplitClient` 等 trait 都要求 `Send + Sync`，使上层可以放入 `Arc<dyn Trait>`。内存实现通过 `Mutex` 或原子类型实现共享可变状态，没有后台线程、异步 runtime 或网络连接。

资源生命周期由上层显式驱动：`MemStorage` 随最后一个 `Arc` 释放；`MemImporterClient::CloseGrpcClient` 只把 `closed` 设为 `true`；`RawKVBatchClient::Close` 只改变布尔标志；`MetadataHelper::Close` 和 `CheckpointRunner::WaitForFinish` 为空操作。`tidbutil::WorkerPool::ApplyOnErrorGroup` 在调用线程同步执行闭包，`ErrorGroup::Wait` 立即成功，因此它们不提供真实并发、排队、取消传播或错误汇聚。

测试若需要验证竞争、阻塞或关闭时序，应使用独立测试夹具（例如 `export_test.rs::FakeStreamMetadataHelper` 的 gate），而不应把同步桩误认为生产并发模型。新增共享状态必须继续满足 `Send + Sync`，缩短持锁区间，并增加独立 `*_test.rs` 覆盖克隆句柄、错误注入和关闭后的行为。

## 与 Go 版本的对应关系

Go 目录中没有 `stubs.go`。Go 的 `client.go`、`import.go`、`migration.go`、`log_file_manager.go` 等直接依赖真实的 `context`、`berrors`、`kvproto` 消息、PD client、external storage、checkpoint、stream、glue/domain 和 importer client；Rust 文件把这些分散依赖的“调用面”收拢到一处。

对应关系以名称和上层控制流为主，不表示实现强度相同：

- `Error::Trace/Annotate/Wrap/Cause/IsCode` 模拟 PingCAP errors/berrors 的常见调用形状，但没有完整 cause chain。
- `Context::Background/Err/Done` 对应 Go context 的查询形状，但不提供 deadline 定时器或可等待 channel。
- protobuf 风格字段和 `Get*` 方法便于与 Go 生成类型对照，但 Rust 结构并非 kvproto 生成代码。
- `Storage`、`PD`、`ImporterClient`、`SplitClient` trait 对应 Go 接口依赖；`Mem*` 类型对应测试替身，不对应线上实现。
- `RetryState`、`PaginateScanRegion`、worker pool 和 failpoint 只覆盖当前 Rust 调用方测试所需片段，分别省略 sleep/上下文传播、真实分页、并发调度和故障注入。

因此，核对移植语义时应先看同目录 Go 业务文件与测试，例如 `client.go`/`client_test.go`、`import.go`/`import_test.go`、`import_retry.go`/`import_retry_test.go`、`migration.go`/`migration_test.go`、`log_file_manager.go`/`log_file_manager_test.go`；不要寻找不存在的同名 Go 桩文件。

## 扩展指南

1. 若新增上层依赖，先判断应接入真实 workspace crate 还是本地桩。只在平台隔离或测试注入确有需要时扩展 `stubs.rs`，避免继续扩大“万能兼容层”。
2. 修改 trait 时同步检查所有生产调用者与实现者；重点搜索 `crate::stubs::<module>`、`Arc<dyn Trait>` 以及同目录独立 `*_test.rs`。Rust 单元测试必须继续放在独立文件中，不能嵌回 `stubs.rs`。
3. 增加协议字段时对照 Go 业务文件实际读取/写入的字段，并验证默认值、克隆语义和序列化格式；不要仅凭 kvproto 名称补齐未使用字段。
4. 强化 `WithRetryV2`、`PaginateScanRegion`、worker pool、Context 或 failpoint 时，要明确是否从测试桩升级为真实语义，并覆盖取消、零重试、分页边界、错误停止条件和资源关闭。
5. 扩展 `MemStorage`/`MemLogMetaManager`/`MemImporterClient`/`MemSplitClient` 时，保留确定性和可注入性；错误应可由测试精确分类，遍历顺序应稳定，共享状态应有清晰锁边界。
6. 最接近的回归文件是 `client_test.rs`、`import_test.rs`、`import_retry_test.rs`、`migration_test.rs`、`log_file_manager_test.rs`、`log_split_strategy_test.rs`、`flow_control_test.rs` 与 `parity_test.rs`。Go 语义核对使用对应的 `*_test.go`。若修改 Rust 逻辑，还应遵守仓库要求先 `cargo fmt --all`，但本次纯文档任务不运行 Cargo。
7. 若把桩替换为外部 Rust 依赖，必须遵守仓库的上游移植、提交、打 tag 和统一 Git tag 依赖规则，不能复制到 `vendor/`/`third_party/` 或用本地 `[patch]` 覆盖。

## 验证依据

- 源文件：`br/pkg/restore/log_client/stubs.rs`，完整检查了 2749 行及其模块/公开符号清单；关键实现位于 `Error`、`Context`、`storeapi`、`utils_retry`、`checkpoint`、`stream`、`glue`、`pd`、`importclient`、`split_client`。
- crate 边界：`br/pkg/restore/log_client/Cargo.toml` 与 `br/pkg/restore/log_client/lib.rs`；后者证明 `stubs` 在生产模块之前公开挂载，测试则通过独立 `#[cfg(test)] #[path = "*_test.rs"]` 文件接入。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`query` 定位到 `MemImporterClient`、`WithRetryV2`、`MemLogMetaManager`、`PaginateScanRegion` 的本文件定义。`explore/node --file` 及精确 `callers/callees` 未返回边，故未把缺失图边写成已验证调用关系。
- 生产调用证据：`client.rs`、`import.rs`、`import_retry.rs`、`migration.rs`、`log_file_manager.rs`、`log_split_strategy.rs`、`flow_control.rs`、`id_map.rs`、`batch_meta_processor.rs` 对 `crate::stubs` 的导入与使用。
- Rust 测试证据：`client_test.rs` 使用 `MemStorage`/`MemLogMetaManager`，`import_test.rs` 使用 `MemImporterClient`/`MemSplitClient`，`import_retry_test.rs` 验证错误分类与 Region 扫描，`migration_test.rs`/`log_file_manager_test.rs` 使用内存存储，`log_split_strategy_test.rs` 使用 checkpoint 状态，`flow_control_test.rs` 验证取消和关闭，`parity_test.rs` 集中验证 Go/Rust 控制流对齐。
- Go 对照：同目录 `client.go`、`import.go`、`import_retry.go`、`migration.go`、`log_file_manager.go` 及其 `*_test.go`；目录中不存在 `stubs.go`，所以本文把对应关系描述为外部依赖面的聚合替身，而非逐文件翻译。
- 本文只陈述当前可见行为；真实 PD/TiKV、gRPC、加密、failpoint、并发 worker 和完整 protobuf 兼容性均未由该文件实现或验证。

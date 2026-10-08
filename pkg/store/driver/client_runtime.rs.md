# `pkg/store/driver/client_runtime.rs`

## 文件定位

本文件属于 `astersql-store-driver` crate（见 `pkg/store/driver/Cargo.toml`），位于 TiDB 风格的同步存储接口与官方异步 `tikv-client` 之间。它不实现事务、快照或 Coprocessor 协议本身，而是集中持有一个多线程 Tokio runtime 和一个 `tikv_client::TransactionClient`，让 `tikv_driver.rs` 与 `kv_adapter.rs` 可以从同步方法中执行异步 PD/TiKV 请求。

模块由 `pkg/store/driver/lib.rs` 公开，并再导出 `ClientConfig`、`ClientRuntime`、`ClientRuntimeError` 和 `KeyspaceConfig`。生产创建入口是 `TiKVDriver::OpenWithOptions`：该函数解析 PD 地址、超时、TLS 和 keyspace 后调用 `ClientRuntime::connect`，再把结果包装为 `Arc<RwLock<ClientRuntime>>`，供 `OfficialBackend`、事务锁解析器及 KV 适配器共享（`pkg/store/driver/tikv_driver.rs:540-568`）。

## 核心职责

1. `ClientConfig` 将驱动层的 PD 地址、RPC 超时、TLS 文件和 keyspace 模式转换为 `tikv_client::Config`，同时把 gRPC 最大解码消息尺寸提升为 `usize::MAX`，避免官方客户端默认 4 MiB 上限拒绝合法的大值或扫描响应（`GRPC_MAX_DECODING_MESSAGE_SIZE`、`ClientConfig::tikv_config`）。
2. `ClientRuntime::connect` 校验配置、创建唯一的多线程 Tokio runtime，并按 API/keyspace 模式连接官方 `TransactionClient`。
3. `current_timestamp`、`lock_waits` 和 `resolve_locks` 提供同步命令边界；`runtime` 与 `transaction_client` 为 crate 内事务/快照适配器提供受关闭状态保护的底层能力。
4. `close` 与 `Drop` 统一释放客户端引用并停止 runtime；关闭之后所有需要这些资源的入口都返回 `ClientRuntimeError::Closed`。

本文件不管理 Store 缓存、Safe Point、RegionCache，也不保存具体事务/快照句柄；这些职责分别在 `tikv_driver.rs`、Coprocessor store 和 `kv_adapter.rs` 中。

## 主要符号

- `GRPC_MAX_DECODING_MESSAGE_SIZE: usize`：固定为 `usize::MAX`，由 `ClientConfig::tikv_config` 写入官方客户端配置。
- `KeyspaceConfig`：公开枚举。`ApiV1` 是默认模式，不添加 keyspace 前缀；`ApiV2(String)` 让客户端按名称加载并编码 keyspace；`ApiV2NoPrefix` 调用专门构造函数，适用于服务端已经处理前缀的 API V2 场景。
- `ClientConfig`：公开构造配置，但字段私有。`new` 收集 PD 地址并采用官方默认超时；`with_timeout`、`with_tls`、`with_keyspace` 是消费并返回 `Self` 的 builder 方法；`validate` 和 `tikv_config` 仅在 crate 内使用。
- `ClientRuntimeError`：公开错误枚举，区分配置错误、runtime 创建失败、官方客户端错误、锁退避后仍有活锁以及已关闭状态。`io::Error` 和 `tikv_client::Error` 通过 `#[from]` 保留为错误源。
- `TimestampSource` / `OfficialTimestampSource`：crate 内可替换的时间戳来源。生产实现调用 `TransactionClient::current_timestamp` 并用 `TimestampExt::version` 转成 TiDB 使用的 `u64` TSO；trait 的主要目的还包括让独立测试注入失败。
- `ClientRuntime`：公开生命周期对象，三个 `Option` 字段分别持有 runtime、客户端和时间戳来源。`Option::take` 同时表达所有权释放与“已经关闭”的状态转换。
- `ClientRuntime::connect`：公开生产构造入口；`lock_waits`、`current_timestamp`、`resolve_locks`、`close` 是同步操作；`runtime` 和 `transaction_client` 是 crate 内适配接口；`from_timestamp_source_for_test` 只在 `cfg(test)` 下存在。
- `Drop for ClientRuntime`：兜底调用幂等的 `close`，忽略其当前恒为成功的返回值。

## 执行流程

### 建立连接

1. `ClientRuntime::connect` 先调用 `ClientConfig::validate`，拒绝空 PD 列表、空白 PD 地址以及空的 API V2 keyspace 名称。
2. 通过 `Builder::new_multi_thread().enable_all().build()` 创建 runtime。构建失败转为 `ClientRuntimeError::Runtime`。
3. `ClientConfig::tikv_config` 从官方默认配置出发，覆盖超时和解码上限，按需加入 CA、证书和私钥；只有 `ApiV2(name)` 调用 `with_keyspace(name)`。
4. `ApiV2NoPrefix` 选择 `TransactionClient::new_with_config_api_v2_no_prefix`；`ApiV1` 与普通 `ApiV2` 选择 `TransactionClient::new_with_config`。两者都由该 runtime `block_on` 完成异步连接。
5. 成功后保存 runtime、一个客户端克隆和持有原客户端的 `OfficialTimestampSource`。这些客户端克隆共享官方客户端的内部 PD/Region 状态，而不是为每个同步请求重新连接。

### 同步命令

- `current_timestamp` 依次确认 runtime 与时间戳来源仍存在，再由来源执行异步请求；生产来源返回 `Timestamp::version()`，而不是直接暴露 physical/logical/suffix 字段。
- `lock_waits` 在同一 runtime 上调用 `TransactionClient::get_lock_waits`，并逐字段转换为 crate 公开的 `WaitForEntry`。`OfficialBackend::lock_waits` 是其直接生产调用者（`tikv_driver.rs:346-357`）。
- `resolve_locks` 把 `astersql_store_copr::TransactionLock` 逐字段转换为 `ProtoLockInfo`，其中当前输入模型没有承载的 `shared_lock_infos` 明确置空。随后以调用者 start TS 和无抖动退避策略调用官方客户端。退避参数为初始 2 ms、单次上限 3000 ms、17 次，源码说明总预算约 22 秒，用来对齐 Go `CopNextMaxBackoff` 的 20 秒量级。若返回的活锁列表非空，则返回数量化的 `LocksStillLive`，不把“仍有活锁”误报为成功。
- `runtime` 与 `transaction_client` 先检查关闭状态。`kv_adapter.rs` 使用它们执行事务开始、快照读写和分页扫描；例如 `begin_transaction` 在共享 runtime 上 `block_on(client.begin_with_options(...))`，`ClientSnapshot::new` 则由同一客户端创建快照。

### 关闭

`close` 按时间戳来源、客户端、runtime 的顺序 `take` 所有权，最后对 runtime 执行一秒 `shutdown_timeout`。再次调用时三个字段已经为空，因此直接成功。若调用方没有显式关闭，`Drop::drop` 走同一流程。

## 数据与状态

`ClientConfig` 是连接前的不可共享值对象；builder 方法移动 `Self`，避免半更新配置被同时观察。其默认 keyspace 为 `ApiV1`，默认超时来自 `tikv_client::Config::default().timeout`，但生产入口会用驱动的 PD server timeout 覆盖它（`tikv_driver.rs:551-558`）。TLS 路径按值克隆进官方配置。

`ClientRuntime` 的运行状态由三个 `Option` 表达：正常生产实例同时具有 runtime、client 和 timestamp source；测试构造实例有 runtime 与 timestamp source、没有 client；关闭实例三者均为空。公开命令分别检查自己实际需要的字段，因此测试实例可以验证时间戳错误传播，却不能取得事务客户端。代码没有额外的布尔 `closed` 标志，资源是否存在就是状态真相。

生产中 `ClientRuntime` 通常位于 `Arc<RwLock<_>>` 内。读操作持有读锁，显式关闭需要写锁和 `&mut self`；`transaction_client` 返回廉价克隆，让具体事务/快照句柄可以在释放 runtime 的读锁后继续持有共享客户端状态。

## 依赖与调用关系

上游关系如下：

- `pkg/store/driver/lib.rs` 声明模块并公开再导出四个主要类型。
- `TiKVDriver::OpenWithOptions` 构造 `ClientConfig` 和 `ClientRuntime`；有 keyspace 名称时选择 `ApiV2`，否则保持 `ApiV1`（`tikv_driver.rs:540-568`）。
- `OfficialBackend` 使用 `current_timestamp` 和 `lock_waits` 支撑 `TikvStore::Begin`、`CurrentVersion` 与 `GetLockWaits`；`OfficialTransactionLockResolver` 把 Coprocessor 遇锁路径接到 `resolve_locks`（`tikv_driver.rs:312-357`）。
- `pkg/store/driver/kv_adapter.rs` 大量借用 `runtime()` 执行官方异步事务/快照方法，并通过 `transaction_client()` 创建事务和快照。这证明 runtime 是 store 级共享资源，而非每请求创建。

下游直接依赖是 `thiserror`、`tokio::runtime` 和带固定 tag `v0.4.2-aster.10` 的 `astersql/client-rust` `tikv-client`（`pkg/store/driver/Cargo.toml`）。本文件还依赖本 crate 的 `TlsConfig`、`WaitForEntry`，以及 `astersql-store-copr` 提供的中立锁结构。Cargo 清单只启用 Tokio 的 `rt-multi-thread` feature；网络、TLS 和客户端行为由 `tikv-client` 及其配置封装。

RustCodeGraph 的文件节点将 `client_runtime.rs` 识别为 39 个符号，并显示其被 `tikv_driver.rs`/`kv_adapter.rs` 所在的存储链使用；精确源码查询进一步确认 `current_timestamp` 经 `OfficialBackend` 支撑 `TikvStore::Begin` 和 `TikvStore::CurrentVersion`，而 `lock_waits` 也经 `OfficialBackend` 进入 Store 状态查询。

## 错误处理与边界

- 配置校验发生在 runtime 创建和网络连接之前。当前仅检查地址集合/字符串和 API V2 名称，不解析 endpoint URL，也不主动验证 TLS 文件存在；后两类错误留给官方客户端连接阶段报告。
- runtime 创建错误与客户端错误保留其来源；驱动层通常再把它们的字符串包装为 `DriverError::Backend`，Coprocessor 锁解析路径包装为 `BatchError::OtherResponse`。
- `lock_waits` 的单次官方调用失败会向 `OfficialBackend` 返回错误；更上层 `TikvStore::GetLockWaits` 会跳过失败响应。这与 Go 版本逐 Store 查询时记录并跳过失败/空响应的可见结果相近，但 Rust 当前只从其单个官方客户端入口取得一组结果。
- `resolve_locks` 只在官方客户端确认没有活锁时成功。当前转换把 `shared_lock_infos` 设为空，因此若上游未来要保留共享锁的嵌套信息，必须同时扩展 `astersql_store_copr::TransactionLock` 与这里的转换。
- 一旦 `close` 取走资源，`current_timestamp`、`lock_waits`、`resolve_locks`、`runtime` 和 `transaction_client` 都拒绝继续工作。`close` 本身不等待任意长时间，只给 runtime 一秒关闭窗口。
- 本文件不捕获锁中毒；持有 `Arc<RwLock<ClientRuntime>>` 的上游负责把 `PoisonError` 转为自己的错误类型。

## 并发与资源生命周期

Tokio runtime 在每个生产 `ClientRuntime` 构造时创建一次，并通过 `enable_all` 启用所需驱动；后续所有同步调用都复用它。`ClientRuntime` 自身不包含内部锁，线程间共享由上游 `Arc<RwLock<ClientRuntime>>` 提供。常规操作取得读锁，因此多个调用可共同借用边界；关闭要求可变引用，生产共享形态下应在取得写锁且没有读者后执行。

`TransactionClient` 的克隆共享其内部连接和路由状态。`connect` 保存两个克隆，是因为 `OfficialTimestampSource` 需要拥有客户端，而事务/快照适配器也要能取得客户端；关闭时先销毁时间戳来源，再销毁显式客户端，确保本对象持有的客户端句柄在 runtime 停止前释放。

`resolve_locks` 在持有 runtime 读借用时同步等待完整的检查事务状态、异步提交处理和 ResolveLock 流程；其约 20 秒级退避可能长期占用上游读锁，但不会创建请求级 runtime。`shutdown_timeout(Duration::from_secs(1))` 给已派生任务有限的清理时间，之后由 Tokio 的关闭语义终止 runtime；本文件没有自建线程、通道或后台任务句柄。

## 与 Go 版本的对应关系

Go 同目录没有独立的 `client_runtime.go`；对应职责分散在 `pkg/store/driver/tikv_driver.go` 以及 `client-go` 的 `KVStore`、PD client 和 RPC client 中。Rust 文件是为同步 Rust 移植新增的集中边界，不应按文件名寻找一一对应实现。

- Go `TiKVDriver.OpenWithOptions` 创建 PD client、按空/非空 keyspace 选择 API V1/V2 codec、创建 RPC client 与 `tikv.KVStore`；Rust `OpenWithOptions` 把地址、TLS、超时和 keyspace 收敛到 `ClientConfig`，再由 `ClientRuntime::connect` 创建官方事务客户端。
- Go `tikvStore.Begin` 委托 `KVStore.Begin`，`CurrentVersion` 委托 `KVStore.CurrentTimestamp`；Rust 的 `TikvStore::Begin`/`CurrentVersion` 经 `OfficialBackend::current_timestamp` 进入本文件，完整事务对象则由 `kv_adapter::begin_transaction` 使用 `transaction_client` 和共享 runtime 创建。
- Go `GetLockWaits` 遍历 RegionCache 中的 TiKV Store，逐个发送 `CmdLockWaitInfo`，对错误或空响应继续处理；Rust `ClientRuntime::lock_waits` 调用官方客户端的 `get_lock_waits` 并做字段转换，上层仍保留“失败响应不阻断汇总”的行为，但请求拓扑由 client-rust 封装。
- Go Coprocessor 遇锁路径用 `ResolveLocksWithOpts` 检查锁状态，并使用 `CopNextMaxBackoff = 20000` 的快速退避预算（`pkg/store/copr/coprocessor.go:2680-2730`）；Rust `resolve_locks` 调用 client-rust 的完整锁解析流程，以约 22 秒无抖动预算对齐量级。两者并非逐行相同，Rust 当前还把退避耗尽后的活锁数量显式转为 `LocksStillLive`。
- Go Store 的 `Close` 也防止重复关闭；Rust 的 `ClientRuntime::close` 同样幂等，但只负责官方客户端/runtime 子资源，Store 缓存、GC worker、Coprocessor store 等仍由 `TikvStore` 生命周期管理。

## 扩展指南

- 新增连接选项时，优先扩展 `ClientConfig` 的私有字段、builder 方法、`validate` 和 `tikv_config`，并同步 `TiKVDriver::OpenWithOptions` 的接线。若选项改变 API 模式，必须同时检查 `connect` 的构造函数分支。
- 新增同步客户端命令时，沿用“先取所需 `Option`，缺失则返回 `Closed`，再在共享 runtime 上 `block_on`”的模式；不要为单次调用创建新 runtime。返回值若属于 crate 的公开模型，应在本文件边界显式转换，避免向上层泄漏不稳定的 protobuf 类型。
- 修改关闭顺序或超时前，要审计 `kv_adapter.rs` 中所有 `runtime()`/`transaction_client()` 使用点以及 `Arc<RwLock<_>>` 的锁持有范围，防止关闭与长 RPC 形成饥饿或意外中断。
- 扩展锁字段或退避策略时，应同步 `astersql_store_copr::TransactionLock`、`OfficialTransactionLockResolver`、client-rust tag 和 Go `ResolveLocksWithOpts` 路径；共享锁信息尤其不能只在本文件单边补字段。
- 测试必须继续放在独立的 `pkg/store/driver/client_runtime_test.rs`，不要内嵌到源文件。配置校验、keyspace 分支、锁转换/活锁错误目前缺少直接单元覆盖，是新增相应行为时最应补齐的用例；真实连接测试需要 `REAL_TIKV_PD`，应继续保持显式忽略而非成为默认单测依赖。
- `tikv-client` 是带 tag 的外部 Git 依赖。若扩展需要上游能力，必须在独立 client-rust 仓库移植、提交并发布新 tag，再统一更新 Cargo manifest；不要使用本地 `[patch]` 或复制依赖源码。

## 验证依据

- 目标源码：`pkg/store/driver/client_runtime.rs`，核对全部 290 行以及常量、枚举、配置、错误、时间戳来源、runtime 方法和 `Drop` 实现。
- crate 边界：`pkg/store/driver/Cargo.toml` 与 `pkg/store/driver/lib.rs`，核对 crate 名、固定的 client-rust tag、Tokio feature、模块声明和公开再导出。
- Rust 直接入口/调用者：`pkg/store/driver/tikv_driver.rs:312-357`、`:540-568`、`:730-810`、`:930-995`；`pkg/store/driver/kv_adapter.rs:720-875`、`:1025-1070`，核对共享所有权、生产构造、锁解析、时间戳、事务及快照接线。
- 独立 Rust 测试：`pkg/store/driver/client_runtime_test.rs`，核对解码上限、客户端错误传播、显式关闭后的拒绝，以及被忽略的真实 PD 连接、原始时间戳转换和 TSO 单调性测试。测试未直接覆盖重复关闭、TLS、三种 keyspace 构造分支、配置校验和锁解析。
- Go 对照：`pkg/store/driver/tikv_driver.go:90-240`、`:400-485`，以及 `pkg/store/copr/coprocessor.go:80`、`:1660-1705`、`:2680-2730`，核对 Open/API 模式、事务时间戳、锁等待、关闭及 `ResolveLocksWithOpts` 退避语义。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/driver` 找到目标源文件与独立测试；`node --file pkg/store/driver/client_runtime.rs --offset 1 --limit 260` 及 `--offset 261 --limit 80` 返回完整源码并标识 39 个符号；对 `ClientRuntime`、`ClientConfig` 和存储驱动调用链的查询确认了上述直接生产接线。由于通用符号名存在大量同名结果，调用关系以精确文件节点和相邻调用源码交叉核验。
- 本任务是纯文档分析，按计划未运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题。

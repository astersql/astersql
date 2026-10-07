# `br/pkg/backup/prepare_snap/env.rs`

## 文件定位

`env.rs` 是 `astersql-br-pkg-backup-prepare-snap` library crate 的环境与协议适配层。crate 入口 `br/pkg/backup/prepare_snap/lib.rs` 以 `pub mod env` 挂载并 `pub use env::*` 重导出本文件；`Cargo.toml` 将该 crate 映射到 Go 包 `br/pkg/backup/prepare_snap`，但当前 `[dependencies]` 为空。

它位于 `prepare.rs` 状态机与 PD、TiKV、RegionCache、StoreManager、PrepareSnapshot 双向流之间。`prepare::Preparer` 持有 `Arc<dyn Env>`，在 `PrepareConnections` 中枚举并连接 store，在 `workOnPendingRanges` 中重新装载 region，在 `streamOf` 中懒建流。本文件因此定义主流程所依赖的边界，而不负责 WaitApply 完成判定、lease 驱动或 Finalize。

当前 Rust 文件没有接入真实 `kvproto`、`tikv-client` 或 gRPC 依赖；协议结构、上下文和外部客户端均为 crate 内本地抽象。Go 生产入口 `br/pkg/task/operator/prepare_snap.go` 会构造真实 `CliEnv` 和 `RetryAndSplitRequestEnv`，当前 Rust 对应文件 `br/pkg/task/operator/prepare_snap.rs` 则使用 `stubs::NewPreparer`，没有直接依赖本 crate。故这里已经具备可测试的控制流语义，但不能据此宣称 Rust 运维入口已经完成真实集群接线。

## 核心职责

1. 以 `Env`、`RegionCacheLike`、`StoreManagerLike`、`PrepareClient`、`Region` trait 隔离外部系统，使 `Preparer` 与内存测试桩共享同一接口。
2. 提供 `Context`/`CancelFunc` 的最小取消传播模型，供连接重试和上层状态机检查取消。
3. 定义 prepare-snapshot 所需的最小 `brpb`、`metapb`、`errorpb` 数据形状，并为请求提供稳定的 protobuf wire-size 近似计算。
4. 由 `CliEnv` 完成 store 过滤、客户端适配和 region range 查询的组合。
5. 由 `gRPCGoAdapter` 将同方向的 `Send` 或 `Recv` 串行化，同时允许收发并行。
6. 由 `RetryAndSplitRequestEnv` 为建连增加退避重试，并由 `SplitRequestClient` 对过大的 `WaitApply` 请求拆包。

## 主要符号

- 常量 `regionCacheMaxBackoffMs = 60000`：保留 Go/TiKV region 查询最大退避语义；本地 `CliEnv` 仅读取该值，并未构造真实 TiKV backoffer。
- 常量 `maxRequestSize = 1 MiB`：`RetryAndSplitRequestEnv::ConnectToStore` 创建拆包客户端时使用的阈值。
- `Context`、内部 `CancellationState`、`CancelFunc`：`background` 创建根上下文；`with_cancel` 建立父链；`is_cancelled` 沿父链查询；`err` 将取消映射为 `Error("context canceled")`。
- `errorpb::Error`：只保存 `Message`。
- `metapb::{StoreLabel, Store, RegionEpoch, Region}`：只保存过滤、路由、日志和大小估算所需字段；`Region::Size` 估算非默认字段的 wire size。
- `brpb::{PrepareSnapshotBackupRequestType, PrepareSnapshotBackupEventType}`：分别表示 `Unknown/UpdateLease/WaitApply/Finish` 请求和 `Unknown/UpdateLeaseResult/WaitApplyDone` 事件。
- `brpb::{PrepareSnapshotBackupRequest, PrepareSnapshotBackupResponse}`：请求携带类型、regions、lease；响应携带事件、可选 region/错误和上一租约有效标记。请求的 `Size` 是拆包依据。
- `IsTiFlash`：识别 `engine=tiflash` 与 `engine=tiflash_compute` label。
- `StringifyRangeOf`：crate 内输出大写十六进制半开区间，空 end 显示为 `inf`。
- `Env`：主环境接口，公开 `ConnectToStore`、`GetAllLiveStores`、`LoadRegionsInKeyRange`。
- `PrepareClient`：双向流的 `Send`/`Recv` 接口，方法接收 `&self` 以容许收发并发。
- `SplitRequestClient`：只拆分过大的 `WaitApply`；`Recv` 直接委托。
- `Region`：向调度逻辑提供 region meta 与 leader store ID。
- `RegionCacheLike`、`StoreManagerLike`：分别抽象 store/region 查询和 prepare stream 建连。
- `CliEnv`：组合 `Cache` 与 `Mgr` 的基础 `Env` 实现。
- `AdaptForGRPCInTest`、`gRPCGoAdapter`：用独立 send/recv mutex 包装客户端。
- `BackoffStrategy`、`ConstantBackoff`、`LimitedBackoff`：重试策略接口、默认固定等待策略及可控测试策略。
- `WithRetryV2`：执行、累计错误、响应取消并按策略等待的重试循环。
- `RetryAndSplitRequestEnv`：装饰任意 `Env`；只增强建连，其他查询原样委托。

## 执行流程

`prepare::New` 将 `Arc<dyn Env>` 保存到 `Preparer`。准备连接时，`Preparer::PrepareConnections` 调用 `GetAllLiveStores`，逐 store 调用 `ConnectToStore`，再把返回的 `PrepareClient` 交给 `prepareStream::InitConn`。重试失败区间时，`Preparer::workOnPendingRanges` 调用 `LoadRegionsInKeyRange`，按 `Region::GetLeaderStoreID` 聚合 `WaitApply` 请求并发送。

典型组合为 `RetryAndSplitRequestEnv { Env: CliEnv, ... }`：

1. `ConnectToStore` 选择注入的 `GetBackoffStrategy`，未注入时使用每次 10 秒的 `ConstantBackoff`。
2. `WithRetryV2` 反复调用内层 `Env::ConnectToStore`。成功立即返回；失败按 `; ` 累计错误，取消时返回已累计错误，未取消则消费下一次退避并以最多 10 ms 的睡眠片段等待。
3. 建连成功后，用固定 1 MiB 阈值构造 `SplitRequestClient`。
4. `Preparer` 发送请求时，非 `WaitApply` 或未超阈值的请求直接透传；超阈值 `WaitApply` 被分片发送。

`CliEnv` 的三个分支分别为：`GetAllLiveStores` 从 cache 取 store 后删除 TiFlash；`ConnectToStore` 调用 manager 并再套 `gRPCGoAdapter`；`LoadRegionsInKeyRange` 在 end key 为空时替换为九个 `0xff`，再委托 cache 查询。

## 数据与状态

`Context` 的每个实例持有一个 `Arc<CancellationState>`。子节点保存父节点 `Arc`，取消只写当前节点的 `AtomicBool`；查询沿父链上溯，因此父取消可传播到所有后代，子取消不反向影响父节点。这里没有 deadline、value 或取消原因模型。

`SplitRequestClient` 本身不保存进度，只持有底层客户端和阈值。拆包时先 clone `Regions`，子请求保持 `WaitApply` 类型、将 `LeaseInSeconds` 置零。大小由本地 protobuf 近似函数计算；它不是实际序列化结果的逐字节保证。

`gRPCGoAdapter` 持有两把独立 `Mutex<()>`：`sendMu` 只保护发送，`recvMu` 只保护接收。锁 poisoned 时通过 `PoisonError::into_inner` 恢复 guard，使一次内部 panic 不永久废弃该方向。

`LimitedBackoff::NextBackoff` 以饱和减法消费 `remaining`；`ConstantBackoff` 用 `i16::MAX` 表达近似无限次数。`WithRetryV2` 自己持有所有失败的拼接结果，不修改传入 `Context`。

## 依赖与调用关系

上游直接调用者是 `br/pkg/backup/prepare_snap/prepare.rs`：

- `Preparer::PrepareConnections` → `Env::GetAllLiveStores`、`Env::ConnectToStore`；
- `Preparer::streamOf` → `Env::ConnectToStore`；
- `Preparer::workOnPendingRanges` → `Env::LoadRegionsInKeyRange`；
- `Preparer::pushWaitApply` → `Region::GetLeaderStoreID`、`Region::GetMeta`。

本文件内部的主要调用链为：

- `RetryAndSplitRequestEnv::ConnectToStore` → `WithRetryV2` → 内层 `Env::ConnectToStore` → `SplitRequestClient`；
- `CliEnv::ConnectToStore` → `StoreManagerLike::ConnectPrepareClient` → `AdaptForGRPCInTest`；
- `CliEnv::GetAllLiveStores` → `RegionCacheLike::GetAllStores` → `IsTiFlash`；
- `CliEnv::LoadRegionsInKeyRange` → `RegionCacheLike::LoadRegionsInKeyRange`；
- `SplitRequestClient::Send` → `PrepareSnapshotBackupRequest::Size` / `metapb::Region::Size` → 底层 `PrepareClient::Send`。

外部 crate 依赖当前为零；仅使用标准库的 `Arc`、`Mutex`、atomic、thread 和 time，以及同 crate 的 `crate::errors::{Error, Result}`。真实 Go 实现则依赖 client-go RegionCache、BR StoreManager、kvproto、grpc-go、PingCAP errors/log 和通用重试工具。

## 错误处理与边界

- `CliEnv` 使用 `?` 原样传播 cache 与 manager 错误；错误上下文由调用它的 `Preparer` 添加。`env_test.rs::cli_env_preserves_store_manager_errors` 固定了 manager 错误不被此层改写的契约。
- `SplitRequestClient` 在任一子请求发送失败时立即返回该错误，不继续发送余下 region；成功时不会额外发送空请求。
- 拆包算法保证超大单 region 也能单独发出，避免零进展循环；只对 `WaitApply` 生效，`UpdateLease`、`Finish` 和 `Unknown` 保持原请求边界。
- `WithRetryV2` 在一次调用成功后丢弃历史错误；持续失败时用 `; ` 返回全部错误。若进入循环前剩余次数不大于零，则返回 `retry failed`。取消发生在失败之后或等待期间时，返回已有失败集合，而不是以 `context canceled` 替换它。
- `Context::err` 不区分 deadline exceeded 与主动取消；当前协议 stub 也不保留完整 protobuf unknown fields、编码器行为或网络状态。
- 空 end key 被替换为九个 `0xff`，这是对 client-go issue 兼容路径的复刻；调用者若需要真正无界范围，必须保留这一约定。

## 并发与资源生命周期

`Env`、`PrepareClient`、`RegionCacheLike`、`StoreManagerLike` 均要求 `Send + Sync`（`Region` 至少要求 `Send`），便于 `Preparer` 的流线程共享对象。`PrepareClient` 的 `&self` API 允许一个线程发送、另一个线程接收；`gRPCGoAdapter` 刻意采用两把锁，使同方向串行但两个方向不互相阻塞。

`Context` 使用 `SeqCst` atomic 读写取消状态，父链由 `Arc` 保活，无后台线程和显式回收动作。`CancelFunc::cancel` 可重复调用，结果保持已取消。

`WithRetryV2` 是同步阻塞循环：退避期间当前线程睡眠，并以 10 ms 为最大轮询片段检查取消。默认策略近似无限重试，所以生产调用者必须确保父 `Context` 最终可取消，否则持续建连失败会长期占用线程。

本文件不拥有底层网络连接的关闭协议；成功返回的客户端生命周期由 `prepareStream`/`Preparer` 管理。拆包客户端和 gRPC adapter 都通过 `Arc` 共享底层客户端，drop 只减少引用计数。

## 与 Go 版本的对应关系

`Env`、`PrepareClient`、`SplitRequestClient`、`Region`、`CliEnv`、`gRPCGoAdapter` 和 `RetryAndSplitRequestEnv` 均直接对应 `br/pkg/backup/prepare_snap/env.go` 的同名概念。1 MiB 阈值、仅拆 `WaitApply`、独立 send/recv 锁、过滤 TiFlash、空 end key 替换以及默认 10 秒重连退避均保持 Go 意图。

主要差异如下：

- Go `CliEnv` 持有真实 `*tikv.RegionCache` 和 `*utils.StoreManager`；Rust 改为可注入 trait，并使用本地协议结构。
- Go `GetAllLiveStores` 通过 PD `WithExcludeTombstone()` 查询；Rust 只能相信 `RegionCacheLike::GetAllStores` 已经给出合适集合，再过滤 TiFlash，接口本身没有 tombstone 选项。
- Go `ConnectToStore` 创建真实 kvproto `PrepareSnapshotBackup` stream，并只为建流失败补充错误上下文；Rust manager trait 直接返回 `PrepareClient`，无法在本层重建该错误分类。
- Go region 查询使用 `tikv.NewBackoffer`；Rust 只保留常量并直接委托。
- Go `utils.WithRetryV2` 还带通用工具层行为及日志；Rust 本地版本只复刻次数、等待、取消和多错误聚合，连接失败也没有 Go 的 warning 日志。
- Go 生产运维入口已经实例化该组合；当前 Rust 运维入口仍走独立桩接线。因此 Rust 的语义覆盖主要由 `env_test.rs`、`prepare_test.rs` 和 `parity_test.rs` 提供。

Go `prepare_test.go::TestRetryEnv` 与 Rust `prepare_test.rs::test_retry_env` 都验证首次连接失败、第二次成功；Go `TestSplitEnv` 与 Rust `test_split_env`/`contract_boundary_split_requests` 都验证大请求拆分、超大单 region 进展、小请求透传和 region 守恒。

## 扩展指南

- 接入真实 Rust PD/TiKV 客户端时，优先实现 `RegionCacheLike`、`StoreManagerLike`、`PrepareClient` 和 `Region`，不要让 `Preparer` 直接依赖具体 SDK。同步补充独立测试文件，且确认 tombstone 排除、错误注解和连接关闭语义与 Go 一致。
- 替换本地 `brpb/metapb/errorpb` 前，先用真实编码器校验 `Size` 和拆包阈值；尤其要覆盖 unknown/default 字段、单 region 超限和中途发送失败，避免消息超限或部分发送被误判为成功。
- 修改拆包算法应同步更新 `env_test.rs`、`prepare_test.rs::test_split_env` 与 `parity_test.rs::contract_boundary_split_requests`；保持非 `WaitApply` 透传和 region 不丢不重的约束。
- 修改重试或取消语义应同步覆盖零次数、多错误顺序、失败后取消、等待中取消和默认策略可终止性。若改成异步 runtime，需重新审视阻塞 sleep 与 trait 的 `Send + Sync` 边界。
- 扩充 `Context` 时应明确 deadline、value、取消原因以及父子传播方向，不能悄悄改变当前 `context canceled` 文案和“返回累计业务错误”的 `WithRetryV2` 契约。
- 将本 crate 接到 `br/pkg/task/operator/prepare_snap.rs` 属于跨 crate 生产接线，需要另行核对 Cargo 依赖、真实网络资源回收和完整运维流程；本文件现状不能作为该接线已完成的证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/backup/prepare_snap/env.rs` 读取了目标文件 1–669 行；`query Env`、`query WithRetryV2` 定位到本文件符号；`explore` 给出 `Env`、取消与 prepare 测试调用面。精确 `callers/callees` 命令在本地超时且无输出，随后用限定在 `br/pkg/backup/prepare_snap` 的 `rg` 补齐直接调用边。
- 源与 crate 边界：`br/pkg/backup/prepare_snap/env.rs`、`lib.rs`、`Cargo.toml`。
- Rust 上游与测试：`prepare.rs` 的 `Preparer`/`PrepareConnections`/`workOnPendingRanges`/`streamOf`；`env_test.rs`；`prepare_test.rs` 的 `test_retry_env`、`test_split_env`、取消/TiFlash/多错误测试；`parity_test.rs` 的拆包与重试契约。
- Go 对照：`br/pkg/backup/prepare_snap/env.go`、`prepare.go`、`prepare_test.go`；生产接线对照 `br/pkg/task/operator/prepare_snap.go`。
- Rust 生产接线限制：`br/pkg/task/operator/prepare_snap.rs` 与根 `Cargo.toml` workspace 成员记录。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求文档存在且恰有十一个规定的二级标题。

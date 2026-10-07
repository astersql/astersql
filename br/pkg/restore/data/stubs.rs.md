# `br/pkg/restore/data/stubs.rs`

## 文件定位

`stubs.rs` 属于 Cargo crate `astersql-br-pkg-restore-data`；crate 根 `br/pkg/restore/data/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 装入它，并以 `pub use stubs::*` 重导出。`br/pkg/restore/data/Cargo.toml` 没有外部依赖，并明确记录 arm64 Darwin 构建不接入 `kvproto`、`grpcio`、KV 或 domain，因此本文件承担恢复数据代码与 PD、TiKV、gRPC、glue、重试、store watcher、worker pool 等外部边界之间的本地替身层。

它不是恢复计划算法的实现位置。实际六阶段恢复流程、计划构造和 flashback 调度位于 `br/pkg/restore/data/data.rs`，region 一致性与选主算法位于 `recover.rs`；本文件只提供这些实现可调用的最小接口、消息结构、错误模型和内存实现。模块注释也明确要求不能把 `Mem*` 类型解释为已连接真实集群的生产实现。

## 核心职责

本文件集中完成五类适配工作：

1. 用 `Error`、`Result`、`Context`、`berrors` 和 EOF 哨兵表示错误分类、恢复阶段与取消传播。
2. 用 `metapb`、`recovpb` 的本地结构，以及 `RecoverDataClient`、两个流 trait、`ClientFactory` 和 `Conn`，模拟读取 region 元数据与发送恢复指令所需的 gRPC 面。
3. 用 `Mgr`、`PDClient`、`TikvStorage`、`FlashbackRpc`、`Progress` 及其 `Mem*` 实现承接管理器、PD、存储、flashback 和进度边界，并允许测试注入错误、计数和客户端工厂。
4. 用 `StoreWatcher`/`StoreWatchCallback`、`BackoffStrategy`/`WithRetry*`、`WorkerPool`/`ErrorGroup` 提供恢复主流程需要的监视、重试和有限并发语义。
5. 用空操作 `log::{Info, Warn, Error, Debug}` 保留日志调用形状，但不产生真实日志。

## 主要符号

- `MaxStoreConcurrency = 128` 与 `gRPCBackOffMaxDelay = 3s`：分别表示并发默认上限和 gRPC 退避配置占位；后者在本地工厂中不建立真实 gRPC 连接。
- `Error { msg, code, stage }` 与 `Result<T>`：消息用于展示，`code` 支持 BR 分类和精确 EOF 判断，`stage` 供 `data.rs::atStage`/`isRetryErr` 判断恢复阶段。`Trace` 原样返回；`Annotate`/`Annotatef` 加消息前缀并保留结构化字段。
- `Context`：以 `Arc<ContextState>` 共享状态；`WithCancel` 建立只向子节点传播的父链，`cancel` 保存原因，`Err` 沿父链查询，`Done` 仅返回布尔值。它不实现 Go context 的 deadline 或 value。
- `metapb::{Store, StoreLabel}` 与 `recovpb::{RegionMeta, ReadRegionMetaRequest, RecoverRegionRequest, RecoverRegionResponse}`：只覆盖 `data.rs`/`recover.rs` 使用的 protobuf 字段和 getter，不是生成的 protobuf 类型。
- `Progress`/`MemProgress`、`Conn`/`MemConn`：前者通过原子计数记录推进，后者通过原子布尔值记录幂等关闭。`MemProgress.closed` 当前未被 `Progress` trait 的方法改变。
- `RegionMetaStream`、`RecoverRegionStream`、`RecoverDataClient` 与 `ClientFactory`：抽象服务端读流、客户端写流、TiKV recover-data 客户端，以及“地址到 client+connection”的可注入构造函数。
- `PDClient`/`MemPDClient`、`TikvStorage`/`MemStorage`、`FlashbackRpc`/`MemFlashback`：分别提供 store 快照、空存储句柄和两阶段 flashback 调用。`MemFlashback` 记录次数、完成 region 数、最后一次 flashback 的 `start_ts`，并支持错误注入。
- `Mgr`/`MemMgr`：聚合 PD、存储、flashback 和可选 `ClientFactory`；`RecoverBaseAllocID` 在成功时记录最大分配 ID，在失败时返回预注入错误。
- `StoreWatchCallback`、`MakeCallback`、`StoreWatcher`：`Step` 从 PD 拉取 store，只对首次出现的 ID 触发 `on_new`；`notify_reboot` 是显式测试辅助。`on_disconnect` 在当前实现中不会由 `Step` 触发。
- `BackoffStrategy`、`RecoveryBackoffStrategy`、`FlashBackBackoffStrategy`、`WithRetryV2`、`WithRetry`：恢复策略最多 16 次并依谓词决定是否继续，flashback 策略最多 3 次且所有错误均可重试；默认延迟为零。重试耗尽时将历次错误消息合并，并保留最后一个错误的 `code`/`stage`。
- `WorkerPool` 与 `ErrorGroup`：用 OS 线程、互斥计数槽和子 `Context` 模拟 Go worker pool 与 `errgroup.WithContext`。`ErrorGroup::Wait` join 全部线程并优先返回首个任务错误。
- `eof`/`is_eof`：以稳定代码 `io.EOF` 表示流结束，不做消息子串匹配。

## 执行流程

主调用链由 `data.rs` 驱动：`RecoverData` 先把恢复工作交给 `WithRetryV2(NewRecoveryBackoffStrategy(isRetryErr))`。每次尝试创建 `Recovery`，随后 `ReadRegionMeta` 使用 `ErrorGroup::WithContext` 和 `WorkerPool` 为各 store 启动任务；任务经 `Mgr::ClientFactory` 获取 `RecoverDataClient`/`Conn`，循环 `RegionMetaStream::Recv`，以 `is_eof` 结束，并由 `data.rs::ConnCloser::drop` 调用 `Conn::Close`。

元数据汇总后，`data.rs`/`recover.rs` 构造恢复计划，并经 `Mgr::RecoverBaseAllocID` 上收最大 ID。`RecoverRegions` 再以相同并发框架按 store 建立 `RecoverRegionStream`，逐条 `Send`，最后 `CloseAndRecv`。接着 `PrepareFlashbackToVersion` 通过 `WithRetry(NewFlashBackBackoffStrategy)` 调用 `FlashbackRpc::SendPrepareFlashbackToVersionRPC`；`FlashbackToVersion` 调用另一个 flashback RPC。进度由 `Progress::Inc` 在各阶段推进。

`SpawnTiKVShutDownWatchers` 在 `data.rs` 中构造 `StoreWatcher`。当前 watcher 的正常 `Step` 只报告新 store；重启事件只有显式调用 `notify_reboot` 才产生，所以它不能等价替代 Go watcher 的完整拓扑变化检测。

## 数据与状态

共享状态主要通过 `Arc`、`Mutex` 和顺序一致原子变量实现。`Context` 的取消原因、`MemMgr` 的最大分配 ID/错误/工厂、`MemPDClient.stores`、`MemFlashback` 的可选错误、`StoreWatcher.known` 以及 `ErrorGroup` 的首错、线程句柄和槽计数均受互斥锁保护；调用方通常取得克隆快照或 `Arc`，不在锁外持有内部借用。

`RecoveryBackoffStrategy.attempts` 和 `FlashBackBackoffStrategy.attempts` 只由持有 `&mut dyn BackoffStrategy` 的重试循环修改。`combine_errors` 按发生顺序用 `; ` 合并消息，但结构化分类采用最后一个错误。`MemFlashback` 在检查注入错误前先递增调用计数，flashback 分支还先记录 `start_ts`，因此失败尝试也会出现在观测值中。

本地消息使用拥有所有权的 `String`/`Vec<u8>`，工厂和 trait object 使用 `Box`/`Arc`。没有数据库事务、持久化状态、网络 socket 或真实 protobuf 生命周期。

## 依赖与调用关系

直接标准库依赖仅有 `HashMap`、线程、`Arc`、`Mutex`、原子变量和 `Duration`；`Cargo.toml` 的 `[dependencies]` 为空。crate 根 `lib.rs` 将本模块公开，生产候选文件 `data.rs` 直接导入 `Context`、错误与并发/重试/管理器接口，`recover.rs` 使用 `berrors`、`recovpb::RegionMeta`、`Error` 和 `Result`。

上游测试包括 `br/pkg/restore/data/parity_test.rs`、`data_test.rs` 和 `key_test.rs`。其中 `parity_test.rs` 自定义流、客户端与连接实现并注入 `ClientFactory`，验证元数据读取、恢复流、连接关闭、重试聚合、取消和 flashback；`data_test.rs` 用 `MemMgr`/`MemProgress` 验证 region 去重、最大 ID 与计划数量；`key_test.rs` 复用 `recovpb::RegionMeta` 测键范围逻辑。

RustCodeGraph 索引状态显示 7,032 个 Rust 文件；精确查询把本文件的 `WithRetryV2`、`ErrorGroup`、`StoreWatcher`、`MemMgr` 和 `RecoverDataClient` 与仓库其他同名符号区分开。图的泛化 `explore` 查询因常见符号名产生大量跨仓库候选，所以最终调用关系以精确符号查询、`data.rs` 导入/调用点和上述测试实现交叉核验。

## 错误处理与边界

`Error` 是轻量替身，不带 backtrace、错误链或动态类型；`Trace` 不增加信息，`Annotatef` 也不解析格式参数。三个 `berrors` 构造器仅覆盖当前 recover 路径使用的错误码。EOF 必须通过 `code == Some("io.EOF")` 识别；普通消息即使包含 `EOF` 也不是流结束。

重试循环总是先执行一次操作，失败后才检查取消；因此已取消的 context 仍会产生一次操作错误，并返回已收集的操作错误而非 context 错误。不可重试错误由恢复策略把尝试数直接推到上限；零延迟表示立即重试。没有错误且初始剩余次数为零时返回 `retry attempts exhausted`。

多个 `Mutex::lock().unwrap()` 意味着持锁线程 panic 会导致后续访问 panic。`ErrorGroup` 忽略子线程 panic 的 join 错误，只返回记录的任务错误或取消原因。`StoreWatcher::Step` 不检测下线、重启时间戳或 TiFlash 过滤；`MemPDClient` 也没有 Go mock PD client 的 `Close`。这些均是明确的桩边界，而非完整生产语义。

## 并发与资源生命周期

`ErrorGroup::Go` 每个任务创建一个 OS 线程；线程以互斥槽计数自旋并 `yield_now`，取得槽后执行闭包，结束时释放槽。首个 `Err` 会取消子 context 并保存首错，等待槽位的后续线程看到取消后直接退出；已经运行的闭包必须自行观察传入的子 context。`Wait` 会 drain 并 join 所有句柄，因此 group 不能复用同一批句柄。

需要注意：`WorkerPool::New(limit, ...)` 虽保存了 `limit.max(1)`，但 `ApplyOnErrorGroup` 没有把该值写入 `ErrorGroup`；实际限流仍由 `ErrorGroup.limit` 决定，默认是 128，除非调用方显式消费 `with_limit` 返回的新 group。故不能仅依据 `WorkerPool` 构造参数断言真实并发度。

连接释放由调用侧 `data.rs::ConnCloser` 的 `Drop` 保证，`MemConn::Close` 自身幂等。后台 watcher 由 `data.rs` 创建线程并依靠 `Context` 取消退出；本文件没有 join handle。`wait_or_cancel` 对非零延迟最多每 10ms 检查一次取消。原子观测统一使用 `SeqCst`，偏保守但让测试跨线程断言清晰。

## 与 Go 版本的对应关系

Go 同目录没有 `stubs.go`；Rust 文件是把 Go `data.go` 的多项外部依赖收拢成一个本地兼容面。对应关系包括：`context.Context` → `Context`；`errors`/`br/pkg/errors` → `Error`/`berrors`；`kvproto/metapb` 和 `recoverdatapb` → 本地 `metapb`/`recovpb`；`glue.Progress` → `Progress`；`conn.Mgr` → `Mgr`；gRPC client/stream/connection → `RecoverDataClient`、流 trait、`Conn` 和 `ClientFactory`；`utils.WithRetry*`/backoff → 本地重试实现；`storewatch` → `StoreWatcher`；`util.WorkerPool` 与 `errgroup` → `WorkerPool`/`ErrorGroup`。

Go `br/pkg/utils/retry.go` 同样在每次操作失败后累计全部错误，并在 context 完成时返回操作错误集合；`parity_test.rs` 锁定了立即重试、16 次耗尽聚合和已取消 context 仍先调用一次等契约。Go `br/pkg/utils/storewatch/watching.go` 会识别新增、下线和启动时间戳变化，并清理消失 store；Rust 桩只实现新增检测与手动 reboot。Go `pkg/util/worker_pool.go` 在提交任务前获取 worker，真实受构造 limit 约束；Rust `WorkerPool.limit` 当前只是记录意图。Go 使用真实 PD/TiKV、generated protobuf、gRPC 和 range task runner，而 Rust `Mem*` 实现不具备这些能力。

## 扩展指南

若要接入真实网络边界，优先为现有 `PDClient`、`RecoverDataClient`、`RegionMetaStream`、`RecoverRegionStream`、`Conn`、`FlashbackRpc` 和 `Mgr` 提供独立适配实现，并通过 `ClientFactory`/`Mgr` 注入；不要把网络逻辑塞入 `Mem*` 类型，也不要迫使 `data.rs` 改写恢复流程。接 protobuf 时需逐字段核对 `metapb`/`recovpb` 本地结构与生成类型，尤其是默认值、getter、枚举和 wire 错误语义。

若完善 watcher，应在 `StoreWatcher::Step` 中保存足够的状态（至少 store 状态、启动时间戳和消失集合），补齐 disconnect/reboot/retain 与 TiFlash 过滤；同步对照 `br/pkg/utils/storewatch/watching.go`。若修正并发限制，应让 `WorkerPool::ApplyOnErrorGroup` 真正取得槽，或确保构造 `ErrorGroup` 时应用 pool limit，并增加独立线程并发峰值测试。

本仓库要求 Rust 测试与源文件分离。修改这些契约时应更新同目录 `parity_test.rs` 或 `data_test.rs`，不要在 `stubs.rs` 内增加 `#[cfg(test)]` 模块。错误/重试修改至少覆盖不可重试、取消、耗尽和多错误顺序；连接/并发修改至少覆盖关闭幂等、首错取消、全部线程 join 和限制生效。接入真实外部 Rust 依赖时还必须遵循仓库规则，在独立上游仓库移植、发布 tag，再通过统一 tag 的 Git 依赖引用。

## 验证依据

- 源文件：`br/pkg/restore/data/stubs.rs`（完整 946 行；符号、字段、实现、注释和边界）。
- crate 与入口：`br/pkg/restore/data/Cargo.toml`、`br/pkg/restore/data/lib.rs`。
- 直接 Rust 调用方：`br/pkg/restore/data/data.rs`、`br/pkg/restore/data/recover.rs`。
- Rust 独立测试：`br/pkg/restore/data/parity_test.rs`、`br/pkg/restore/data/data_test.rs`、`br/pkg/restore/data/key_test.rs`。
- Go 对照：`br/pkg/restore/data/data.go`、`br/pkg/restore/data/data_test.go`、`br/pkg/utils/retry.go`、`br/pkg/utils/backoff.go`、`br/pkg/utils/storewatch/watching.go`、`pkg/util/worker_pool.go`。
- RustCodeGraph：运行 `rustcodegraph status`，并查询/定位 `WithRetryV2`、`ErrorGroup`、`StoreWatcher`、`MemMgr`、`RecoverDataClient`；索引含 7,032 个 Rust 文件。精确调用结论再由源码导入与调用点复核。
- 结构验证按任务命令检查本文件存在且固定二级标题恰为 11 个；本任务为纯文档分析，依计划不运行 Cargo。

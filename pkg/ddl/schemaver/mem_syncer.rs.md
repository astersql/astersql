# `pkg/ddl/schemaver/mem_syncer.rs`

## 文件定位

[`mem_syncer.rs`](mem_syncer.rs) 属于 `astersql-ddl-schemaver` crate；crate 入口 [`lib.rs`](lib.rs) 将该模块的公开项重新导出。它提供 [`Syncer`](syncer.rs) 协议的纯内存实现，模拟真实 `etcdSyncer` 的“节点上报版本、owner 发布版本、等待节点追上、session 失效/重启”接口，主要用于无需 etcd 的测试和本地推演。

当前 Rust 接线需要谨慎区分两层事实：`MemSyncer` 已能作为 `Syncer` trait object 被通用 DDL schema barrier 使用，但仓库中 `NewMemSyncer` 的 Rust 直接调用者只出现在 `mem_syncer_test.rs`、`syncer_test.rs` 等独立测试；未在非测试 Rust 文件中找到选择它作为生产后端的构造调用。对应 Go 实现则由 `pkg/ddl/ddl.go:newDDL` 在 `EtcdCli == nil` 的本地存储/测试路径中选用。

## 核心职责

- `MemSyncer` 以进程内原子值、映射、锁和同步通道复现 `Syncer` 的可观察协议，而不持久化任何 etcd 键。
- 非 MDL 模式只维护一个节点版本 `selfSchemaVersion`；MDL 模式通过 `mdlSchemaVersions[jobID]` 隔离不同 DDL job 的版本。
- `OwnerUpdateGlobalVersion` 只发送一条空 `WatchResponse` 作为“全局版本已变化”的唤醒信号，并不保存传入的版本值。
- `WaitVersionSynced` 每 2 ms 检查一次内存版本，直到达到 `latestVer` 或 `Context` 取消/超时；成功固定汇报一个参与节点。
- `Done`、`CloseSession` 和 `Restart` 模拟 etcd session 生命周期；若调用方需要真实租约、节点枚举或路径清理，应使用 `syncer.rs` 中的 `etcdSyncer`。

## 主要符号

- `checkVersionsInterval: Duration`：轮询间隔，固定为 2 ms，对齐 Go `checkVersionsInterval`。
- `MemSyncer`：核心状态容器。字段均为内部实现细节；类型本身公开，以便测试执行 `CloseSession` 等专用操作。
- `new_watch_channel() -> (SyncSender<WatchResponse>, WatchChan)`：创建容量为 1 的同步通道，并将接收端包装为可克隆的 `WatchChan`。
- `NewMemSyncer() -> Arc<MemSyncer>`：以默认状态构造共享实例。虽然命名沿用 Go API，但返回具体类型而非 `Arc<dyn Syncer>`。
- `Default::default`：将非 MDL 版本置 0、MDL 映射置空，创建 watch 通道和未完成的 `DoneSignal`。
- `Init`：清空 MDL 映射，替换 watch 通道与 session；刻意不重置 `selfSchemaVersion`。
- `UpdateSelfVersion`：根据全局 `IsMDLEnabled()` 选择按 job 写入或覆盖单一原子版本，并支持 `MOCK_UPDATE_MDL_ERROR` 错误注入。
- `OwnerUpdateGlobalVersion`：对容量 1 的通道执行非阻塞 `try_send`；满通道时丢弃本次通知但仍返回成功。
- `WaitVersionSynced`：按当前 MDL 开关选择版本来源，轮询直到达标，成功返回 `SyncSummary { ServerCount: 1, AssumedServerCount: 0 }`。
- `Syncer for MemSyncer`：逐项转发到同名固有方法，使实例可注入使用 `Arc<dyn Syncer>` 的通用逻辑。

## 执行流程

典型测试流程从 `NewMemSyncer` 开始，随后调用 `Init`。初始化会先清空按 job 版本，再创建新的容量 1 watch 通道；旧发送端被放入 `retiredGlobalVerSenders`，使旧接收端保持“仍连接但没有消息”的状态，复现 Go 替换 channel 时旧 channel 未关闭的语义。最后用新的 `DoneSignal` 替换 session 信号。

节点加载 schema 后调用 `UpdateSelfVersion(context, jobID, version)`。若错误注入已开启，函数在改状态前返回 `SyncError("mock update mdl to etcd error")`；否则 MDL 开启时更新 `mdlSchemaVersions[jobID]`，未开启时以 Release 顺序写入 `selfSchemaVersion`。

owner 侧调用 `OwnerUpdateGlobalVersion` 时，传入版本本身被忽略，只尝试向 watch 通道投递空响应。容量已满不会阻塞，也不会报错。随后通用 DDL 链可调用 `WaitVersionSynced`：它先处理指定 job 的 2 秒慢检查注入，再循环执行 `Context::Wait(2ms)`；每次唤醒后读取相应版本，达到或超过目标即成功，否则继续。`pkg/ddl/schema_version.rs:NormalDdlSchemaBarrier::wait` 展示了协议级生产顺序：发布全局版本、等待同步、再次检查 owner lease、清理 MDL 信息；这是 `Syncer` 的上游调用链，不等于当前 Rust 生产构造路径已经选择 `MemSyncer`。

## 数据与状态

- `selfSchemaVersion: AtomicI64`：非 MDL 模式的唯一版本，写使用 `Ordering::Release`、读使用 `Ordering::Acquire`。`Init` 不清零，因此重复初始化后仍可满足先前版本目标。
- `mdlSchemaVersions: Mutex<HashMap<i64, i64>>`：MDL 模式下 job ID 到版本的映射；同一 job 的后续写入直接覆盖。`Init` 会清空它。
- `globalVerSender` / `globalVerCh`：当前观察通道的发送端和接收端包装；读写锁允许从共享引用替换它们。
- `retiredGlobalVerSenders`：持有历次初始化替换掉的发送端。其目的不是再发送事件，而是避免旧 `WatchChan` 观察到断开；代价是每次 `Init` 增长一个 sender，直至 `MemSyncer` 被释放。
- `mockSession: RwLock<DoneSignal>`：当前 session 的共享完成标志。`CloseSession` 关闭当前信号，`Restart` 与 `Init` 都替换为新信号；已经取得的旧 `DoneSignal` 不会随替换恢复。
- `IsMDLEnabled`、`MOCK_OWNER_SLOW_JOB`、`MOCK_UPDATE_MDL_ERROR` 定义在 `syncer.rs`，是进程级共享配置，因此相关测试使用 `TEST_CONFIG_LOCK` 串行修改。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：直接依赖 `astersql-domain-serverinfo`，用于满足 `SetServerInfoSyncer` 的签名；`etcd-client` 与 `tokio` 属于整个 schemaver crate 的真实同步器实现，`mem_syncer.rs` 本身不直接调用二者。工作区根 `Cargo.toml` 注册该 crate，`pkg/ddl/Cargo.toml` 依赖它。

上游协议在 `syncer.rs:Syncer` 定义。`pkg/ddl/schema_version.rs` 的 `NormalDdlSchemaBarrier` 持有注入的 `Arc<dyn Syncer>`，其 `wait`/`recover` 会调用 `OwnerUpdateGlobalVersion` 和 `WaitVersionSynced`。目标文件的 trait impl 因而能进入这条通用链，但代码搜索未发现非测试 Rust 调用 `NewMemSyncer` 完成具体注入。

下游依赖均来自同 crate：`Context` 提供取消与超时等待，`DoneSignal` 表示 session 结束，`WatchChan`/`WatchResponse` 模拟 watch，`SyncSummary` 描述同步节点数，`SyncError` 承载错误。`SetServerInfoSyncer`、`WatchGlobalSchemaVer`、`SyncJobSchemaVerLoop` 和 `Close` 在内存实现中为空操作，这是单节点模拟边界，而非真实集群能力。

## 错误处理与边界

- `Init`、`Restart` 和正常更新路径没有外部 I/O，通常返回 `Ok(())`；可预期的业务错误只有 `UpdateSelfVersion` 的测试注入，以及 `WaitVersionSynced` 的 context 取消/超时。
- 所有标准锁都用 `expect` 处理 poisoned lock；持锁线程 panic 会导致后续访问也 panic，而不是转换成 `SyncError`。
- `WaitVersionSynced` 至少先等待一个 2 ms 周期再检查版本。即使版本已满足，已取消或零超时 context 也会先返回错误。
- MDL 模式仅检查传入 `jobID` 的槽位；其他 job 达标不能推进当前等待。非 MDL 模式则忽略 `jobID`。
- `_checkAssumedSvr` 被忽略，成功摘要固定为单节点且 assumed 数为 0；该实现不能验证多节点或跨 keyspace assumed-server 语义。
- `OwnerUpdateGlobalVersion` 不保存 `_version`，容量为 1 的通道合并连续通知，并把“通道已满”视作成功；消费者必须在收到通知后从自己的权威来源加载版本，不能从空响应推断版本值。
- 目标版本比较使用 `>=`，允许节点已领先时立即在下一轮检查成功；没有强制版本单调写入，测试调用者仍可写入更小值。

## 并发与资源生命周期

`Arc<MemSyncer>` 允许多线程共享；原子字段处理非 MDL 高频版本值，MDL 映射与可替换资源分别受 `Mutex`/`RwLock` 保护。`UpdateSelfVersion` 和 `WaitVersionSynced` 在 MDL 模式下会短暂串行访问映射，等待循环不会跨休眠持锁。watch 发送采用 `try_send`，owner 永不因消费者滞后阻塞。

`Init` 更换当前通道时先把旧 sender 克隆存入 retired 列表，再替换 sender 和 receiver；因此之前克隆出的 `WatchChan` 会超时而非返回断开。该生命周期由 `mem_syncer_test.rs:init_keeps_previous_global_version_channel_open_like_go` 专门验证。`CloseSession` 只关闭当前 `DoneSignal`，`Restart` 只重建 session，不触碰版本或 watch 通道。`Close` 是空操作，所有内存资源最终依靠 `Arc` 和字段析构释放。

## 与 Go 版本的对应关系

直接对照文件是 [`mem_syncer.go`](mem_syncer.go)。两者保持以下核心语义：2 ms 检查周期；MDL 按 job 保存版本、非 MDL 保存单一版本；`Init` 不重置非 MDL 版本；更新错误和 owner 慢检查可注入；全局通知容量为 1 且不阻塞；等待以 `>= latestVer` 为成功条件；成功节点数为 1；session 可关闭和重启；后台循环、serverinfo 注入和 `Close` 为空操作。

Rust 为适应所有权和线程安全做了结构性改写：Go 的 `sync.Map` 对应 `Mutex<HashMap<...>>`，原子函数对应 `AtomicI64`，channel 字段拆为受锁保护的 sender/receiver，`DoneSignal` 取代可关闭 channel。Rust 额外保存 retired sender，显式维持 Go 旧 channel 未关闭的效果。Go failpoint 对应 `syncer.rs` 的全局原子开关。

已确认的接线差异是：Go `NewMemSyncer() Syncer` 返回接口，并在 `pkg/ddl/ddl.go:newDDL` 的 nil-etcd 分支直接使用；Rust `NewMemSyncer() -> Arc<MemSyncer>` 返回具体类型，当前非测试 Rust 搜索未发现直接构造调用。因此本文不声称 Rust 的无 etcd DDL 启动路径已经完成同等接线。

## 扩展指南

- 若要让 Rust 无 etcd DDL 启动路径实际使用此实现，应在负责构造 DDL/schema barrier 的位置注入 `Arc<dyn Syncer>`，不要把 backend 选择逻辑塞进 `mem_syncer.rs`；同时增加独立测试证明从构造入口走到发布和等待链。
- 若新增 `Syncer` 方法，应同时更新 `syncer.rs:Syncer`、`MemSyncer` 的固有方法与 trait 转发、真实 `etcdSyncer` 实现，并在 `mem_syncer_test.rs` 或 `syncer_test.rs` 中加入独立测试；测试逻辑不要内嵌到生产文件。
- 若改变初始化语义，必须保留或明确更新两项 Go 兼容约束：`selfSchemaVersion` 是否跨 `Init` 保留，以及旧 watch channel 是否保持打开。相关回归分别位于 `syncer_test.rs:mem_syncer_init_preserves_non_mdl_self_schema_version_like_go` 和 `mem_syncer_test.rs:init_keeps_previous_global_version_channel_open_like_go`。
- 若改变等待策略，应覆盖：已达标、低于目标、context 取消/超时、MDL job 隔离、慢 job 注入，以及 `>=` 边界。降低轮询间隔会增加锁/CPU 压力；改成条件变量或通知机制时须防止错过先于订阅发生的版本更新。
- 若让 `Close` 或空操作方法承担真实资源清理，必须说明幂等性和并发调用契约，并避免破坏当前测试替身“无外部副作用”的定位。

## 验证依据

- 源码与协议：`pkg/ddl/schemaver/mem_syncer.rs`、`pkg/ddl/schemaver/syncer.rs`（`Context`、`DoneSignal`、`WatchChan`、`SyncSummary`、`Syncer` 及测试开关）、`pkg/ddl/schemaver/lib.rs`。
- crate 与上游：`pkg/ddl/schemaver/Cargo.toml`、工作区 `Cargo.toml`、`pkg/ddl/Cargo.toml`、`pkg/ddl/schema_version.rs:NormalDdlSchemaBarrier::{wait,recover}`。
- Go 对照：`pkg/ddl/schemaver/mem_syncer.go`、`pkg/ddl/schemaver/syncer.go`、`pkg/ddl/ddl.go:newDDL`、`pkg/ddl/schema_version.go:waitVersionSynced`、`pkg/ddl/job_worker.go` 的全局版本发布路径。
- 独立测试：`pkg/ddl/schemaver/mem_syncer_test.rs`；`pkg/ddl/schemaver/syncer_test.rs` 中 canonical session/watch、跨 `Init` 保留非 MDL 版本、MDL job 分流和注入错误用例；Go 的 `pkg/ddl/schemaver/syncer_test.go` 用于核对真实 etcd 同步器的协议预期。
- RustCodeGraph：`status` 确认索引含 11,467 个文件并覆盖 `pkg/ddl/schemaver/{mem_syncer.rs,mem_syncer.go}`；`query MemSyncer --json` 定位 Rust/Go 类型及构造器。自然语言 `explore` 与部分精确 callers/callees 在当前 CLI 未返回可用结果，调用边因此又以 `rg` 和相邻源码核验；未把缺失图结果当作已验证事实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工检查无整段源码复制、无 Rust 生产接线臆测。

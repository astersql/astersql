# `br/pkg/backup/store.rs`

## 文件定位

`store.rs` 属于 Cargo 包 `astersql-br-pkg-backup`，由同目录 [`lib.rs`](./lib.rs) 以 `pub mod store` 注册并通过 `pub use store::*` 平铺导出。它位于 BR 备份编排层和单个 TiKV store 的 Backup 流之间：上游 [`client.rs`](./client.rs) 的 `Client::BackupRanges`/`Client::RunLoop` 负责全局进度与多 store 轮次，本文件负责把一份 store 请求拆分、并发发送、逐包转发，并把 PD 拓扑变化转换为重试策略。

该 crate 的 [`Cargo.toml`](./Cargo.toml) 将 Go 包映射声明为 `br/pkg/backup`，且只直接依赖 `serde`、`serde_json`；Backup RPC、PD、context、工作池、退避和 store watcher 均来自本 crate 的 [`stubs.rs`](./stubs.rs)，因此当前 Rust 实现是可测试的本地移植边界，不等同于已接入真实 `kvproto/grpcio/pd-client` 网络栈。

## 核心职责

- `BackupSender`、`BackupRetryPolicy`、`ResponseAndStore` 定义 `client.rs` 与 store 发送层之间的数据契约。
- `SplitBackupReqRanges` 按目标并发度均衡切分 `BackupRequest.SubRanges`，保留请求的其他字段和子区间顺序。
- `startBackup` 为单个 store 建立超时 context 和工作池，对每个分片执行带 `NewBackupSSTBackoffStrategy` 的 `doSendBackup`，并把响应附上 `StoreID` 后发送给上层。
- `doSendBackup` 在 Backup RPC 建流阶段使用 `ResourceConcurrentLimiter`，随后读取流直到结束或出错，并保证尝试 `CloseSend`。
- `StartTimeoutRecv`/`timeoutRecv` 监控“连续多久没有成功转发响应”；超时会以 `receive a backup response timeout` 取消派生 context。
- `ObserveStoreChangesAsync` 周期观察 PD store：重启或断连产生 `All=true`，新注册 store 产生 `One=<store id>`，供 `RunLoop` 决定重发范围。

## 主要符号

- `pub struct BackupRetryPolicy { One: u64, All: bool }`：`All` 表示整轮所有 store 重试；否则 `One` 指定单个 store。`client.rs::RunLoop` 优先处理 `All`，并仅在 `One != 0` 时执行单 store 路径。
- `pub trait BackupSender::SendAsync(...)`：异步发送抽象。生产实现是 `client.rs::MainBackupSender`；它在线程中调用 `startBackup`，非取消错误通知 `One=storeID`，最后发送 `None` 作为该 store 的结束哨兵。
- `pub struct ResponseAndStore` 及 `GetResponse`/`GetStoreID`：把 `BackupResponse` 与来源 store 绑定，供 `CollectStoreBackupsAsync` 和 `OnBackupResponse` 聚合、分类错误及推进全局进度。
- `pub struct timeoutRecv`：公开但采用非惯用小写名称的看门狗类型；内部字段私有，包含父取消标志、带 cause 的 cancel、容量为 1 的刷新发送端、循环线程句柄和幂等停止标志。
- `pub static TimeoutOneResponse`：生产默认单响应等待上限，一小时。`TIMEOUT_OVERRIDE` 与 `set_timeout_one_response_for_test` 是进程级测试覆盖入口。
- `pub fn StartTimeoutRecv(...)`：返回派生 `Context` 和共享看门狗；内部启动父取消观察线程与超时循环线程。
- `pub fn doSendBackup(...)`：单请求、单流发送/接收函数；响应通过闭包交给编排层。
- `pub fn startBackup(...)`：单 store 的核心编排函数。虽然公开导出，生产调用点是 `MainBackupSender::SendAsync`。
- `pub fn ObserveStoreChangesAsync(...)`：启动后台观察线程，不返回 `JoinHandle`。
- `pub fn SplitBackupReqRanges(...)`：纯拆分函数；空 `SubRanges` 或 `count <= 1` 返回原请求，分片数不会超过子区间数。

## 执行流程

1. `Client::BackupRanges` 创建 `BackupRetryPolicy` 通道，启动 `ObserveStoreChangesAsync`，再构造 `MainBackupLoop`。
2. `Client::RunLoop` 获取当前未完成 ranges 和存活 store，为每个 store 建响应通道并调用 `BackupSender::SendAsync`。`MainBackupSender` 为每个 store 启动线程并调用 `startBackup`。
3. `startBackup` 先检查父 context；随后用 `SplitBackupReqRanges` 将 `SubRanges` 分成至多 `concurrency` 份，调用 `StartTimeoutRecv`，并以 `WorkerPool::new(concurrency, "store_backup")` 调度分片任务。桩工作池把零并发提升为 1。
4. 每个任务用 `WithRetry(..., NewBackupSSTBackoffStrategy())` 包裹 `doSendBackup`。重试闭包每次克隆同一分片请求；任一任务最终失败会由 `wait_jobs` 聚合为 `startBackup` 的错误。
5. `doSendBackup` 以 `SubRanges.len() + 1` 为资源量执行 `Acquire`，调用 `BackupClient::Backup` 后立即 `Release`；此限流覆盖建流阶段，而非整个响应流生命周期。之后循环 `Recv`：`None` 表示正常结束，`Some(resp)` 调用回调，错误直接向上传播。退出读循环后无论成功失败都尝试 `CloseSend`，但忽略其错误。
6. `startBackup` 的响应闭包先应用测试 failpoint，再检查 context，把 `Some(ResponseAndStore)` 发送到 store 私有通道；只有发送成功后才 `Refresh` 看门狗。这里使用的标准 `mpsc::channel` 是无界通道，正常发送不会等待消费者腾出容量。
7. 所有分片结束后，`startBackup` 先 `wait_jobs`，再 `timeoutRecv::Stop`，最后返回聚合结果。结束哨兵不由本文件发送，而由 `MainBackupSender` 在 `startBackup` 返回后发送。
8. 与发送链并行，`ObserveStoreChangesAsync` 先做一次基线 `watcher.Step`；之后默认每 30 秒清空本轮标志并再次 `Step`。重启/断连优先产生一次全量重试；若没有全量事件，则逐个通知新注册 store。

## 数据与状态

`BackupRequest` 是拆分输入。`SplitBackupReqRanges` 克隆整个请求，仅用连续切片替换每份请求的 `SubRanges`；余数优先分配给前面的分片，所以 10 个区间拆成 3 份得到 `4/3/3`。`StartKey`、`EndKey` 及其他字段随 clone 保留。

`ResponseAndStore` 是发送层到全局处理层的消息载荷；通道元素使用 `Option<ResponseAndStore>`，其中 `Some` 是数据，`None` 是生产者结束协议。`BackupRetryPolicy` 是另一条控制通道，拓扑观察器和发送失败路径都是生产者，`RunLoop` 是消费者。

超时状态由多个同步原语共同维护：`AtomicBool parent_done` 控制循环退出，`Mutex<Option<SyncSender<()>>>` 允许 `Stop` 通过丢弃发送端关闭刷新通道，`Mutex<Option<JoinHandle<()>>>` 保证至多 join 一次，`AtomicBool stopped` 使 `Stop` 幂等。刷新通道容量为 1，`try_send` 会合并密集刷新信号，而不会阻塞响应处理。

`ObserveStoreChangesAsync` 在线程内维护 `sendAll: AtomicBool` 与 `HashMap<u64, ()>` 去重新 store ID；每个 tick 前清空，只发布当前观察步产生的变化。测试 failpoint 可把 tick 从 30 秒缩短到 100 毫秒。

## 依赖与调用关系

上游主链是 `client.rs::Client::BackupRanges` → `ObserveStoreChangesAsync`/`Client::RunLoop` → `MainBackupLoop.BackupSender.SendAsync` → `MainBackupSender::SendAsync` → `startBackup`。响应反向经过 store 私有通道 → `MainBackupLoop::CollectStoreBackupsAsync` → 全局通道 → `Client::OnBackupResponse`。重试策略由本文件观察器或 `MainBackupSender` 写入 `StateNotifier`，再由 `RunLoop` 消费。

本文件的直接内部调用边为：`startBackup` → `SplitBackupReqRanges`、`StartTimeoutRecv`、`WithRetry`、`doSendBackup`、`wait_jobs`；`StartTimeoutRecv` → `Context::WithCancelCause`、`loop_timeout`；`ObserveStoreChangesAsync` → `storewatch::MakeCallback`、`storewatch::New`、`Watcher::Step`。

下游类型与服务边界来自 `limit.rs::ResourceConcurrentLimiter` 和 `stubs.rs`：`backuppb::{BackupClient, BackupRequest, BackupResponse}`、`Context`、`PdClient`、`WorkerPool`、`utils::WithRetry`、`storewatch`、`failpoint`。RustCodeGraph 的文件级索引还显示 `store.rs` 被 `client_test.rs`、`store_test.rs`、`restore/snap_client/import.rs`、`restore/split/client.rs` 和 `cmd/tidb-server/stubs.rs` 使用；生产备份主链则由上述 `client.rs` 的真实符号引用进一步确认。

## 错误处理与边界

- 父 context 在 `startBackup` 入口已取消时立即返回其错误；读流期间每轮也检查派生 context。
- `stubs.rs::Context::WithCancelCause` 只复制创建瞬间的父错误，不会自动传播后续父取消；`StartTimeoutRecv` 的观察线程只设置 `parent_done`，最终要由 `Stop` 取消派生 context。因而真实 Go context 的即时级联取消在当前桩实现中未完全复现，卡在底层 `Recv` 时尤其需要注意。
- Backup 建流错误、`Recv` 错误、响应回调错误都会从 `doSendBackup` 原样向上传播；`WithRetry` 决定哪些发送失败值得重试，最终错误由 `wait_jobs` 返回。
- `respCh` 接收端断开被转换为 `Error("resp channel closed")`，可结束该分片并向上传播。
- 超时错误文本固定为 `receive a backup response timeout`，用于派生 context 的 cause。`Stop` 自身使用无 cause 的取消，测试只要求派生 context 已取消。
- `CloseSend` 的失败被有意忽略；初始或周期 `watcher.Step` 的失败也被忽略并留待下一轮。Rust 版本当前没有 Go 版本对应的结构化日志输出。
- failpoint 可注入可重试/不可重试建流错误以及响应内的 timeout/storage/read-write/region 错误。这些是测试路径，不应描述为生产错误分类实现。
- `SplitBackupReqRanges` 不校验区间本身是否合法，只保证分组、覆盖和顺序；`count <= 1` 明确表示不拆分。
- crate 使用精简 `stubs`，真实 gRPC 状态码、PD 客户端行为和 Go `context.Context` 的完整传播语义不在本文件中得到证明。

## 并发与资源生命周期

`StartTimeoutRecv` 创建两个后台线程：父 context 观察线程每 5 毫秒轮询，超时循环线程阻塞在 `recv_timeout`。`Stop` 的顺序是：用 `stopped.swap` 去重、丢弃刷新 sender、置 `parent_done`、join 超时线程、取消派生 context；这避免 `Refresh` 与退出线程长期竞态。父观察线程没有句柄，但会在父取消或 `parent_done` 置位后退出。

`startBackup` 的每个请求分片是独立工作任务；`WorkerPool` 约束并发并由 `wait_jobs` 等待全部任务。共享对象均经 `Arc` 传递，响应通道可由多任务并发写入。资源 limiter 的令牌在 `Backup` 调用返回后立即释放，即卡住的 `Recv` 不继续占用该令牌。

`ObserveStoreChangesAsync` 是脱离调用者的后台线程，生命周期只由传入 context 控制。与 Go 的 ticker + `select` 不同，Rust 线程在 `sleep(tickInterval)` 期间不能立刻响应取消，默认最迟可能在约 30 秒后退出；函数也不提供 join。这是扩展关闭流程时必须保留或明确改进的生命周期差异。

## 与 Go 版本的对应关系

直接对照文件为 [`store.go`](./store.go)。Rust 保留了 Go 的主要符号、字段命名和流程：请求均衡拆分、`len(SubRanges)+1` 限流、Backup 流逐包回调、每包刷新超时、工作池 + 退避、拓扑事件映射为 `All`/`One`，以及先等待任务再停止看门狗。

实现层差异包括：Go 使用真实 `context`、gRPC/kvproto、PD client、`errgroup`、TiDB worker pool 和日志；Rust 使用 `stubs.rs` 的本地 trait/类型、OS 线程与 `std::sync::mpsc`。Go 的 `timeoutRecv.refresh` 是普通 channel，Rust 使用容量 1 的同步通道并以 `try_send` 合并刷新；Go 的 store watcher 通过 ticker `select` 可立即响应取消，Rust 使用 `thread::sleep`；Go 以 `io.EOF` 表示正常流结束，Rust `BackupStream::Recv` 以 `Ok(None)` 表示结束。Rust 另有 `TIMEOUT_OVERRIDE`，避免测试直接修改全局 `TimeoutOneResponse`。

Go 的注释指出未来合并 SST 功能可能把 `SortedSubRangesGroups` 计入 `reqRangeSize`；Rust 当前与 Go 现状一致，只计算 `SubRanges + 1`。不能据此宣称已支持该未来能力。

## 扩展指南

- 改变请求拆分策略时，修改 `SplitBackupReqRanges`，并同步 `client_test.rs` 中 `test_split_backup_req_ranges`、表驱动拆分测试及 Go `client_test.go` 的对应期望；必须保持所有子区间恰好覆盖一次且顺序不变。
- 改变发送、退避或响应转发时，优先在 `doSendBackup`/`startBackup` 接入，并同步独立的 `store_test.rs`、`parity_test.rs`；不要把测试嵌入生产文件。需要明确 limiter 覆盖阶段、`CloseSend` 错误策略和 `None` 结束哨兵仍由 `MainBackupSender` 负责。
- 新增重试原因或拓扑事件时，同时审查 `BackupRetryPolicy`、`ObserveStoreChangesAsync`、`client.rs::MainBackupSender` 和 `client.rs::RunLoop`，防止生产者发出消费者不识别的状态。
- 改变 timeout/context 实现时，应保留 `Stop` 幂等、线程可退出、首包和包间超时都能取消派生 context；同步 `store_test.rs` 的首包、非首包、父取消和主动停止用例。
- 接入真实网络依赖不能只改本文件：当前 Cargo manifest 明确依赖本地 `stubs`。应按仓库外部 Rust 依赖规则在上游仓库移植并发布 tag，再统一更新 Cargo Git 依赖；不得在本仓库复制 vendor 或使用本地 `[patch]`。
- 性能风险集中在分片数量、请求 clone、线程数量、通道背压和 30 秒轮询；兼容风险集中在 Go/Rust 的取消传播、gRPC 错误分类与 watcher 事件顺序。任何改动都应先与 `store.go` 的实际增量对齐。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`files --filter br/pkg/backup` 确认目标、Go 对照和测试均已索引；`node --file br/pkg/backup/store.rs --offset ...` 读取完整 526 行并报告 5 个使用文件；`query` 分别确认 `doSendBackup`、`startBackup`、`ObserveStoreChangesAsync`、`SplitBackupReqRanges`、`StartTimeoutRecv` 的 Rust/Go 定义。精确 `callers/callees --file` 在 30 秒内未返回，调用边改由直接引用核验。
- 生产源码与配置：`br/pkg/backup/store.rs`、`br/pkg/backup/client.rs`、`br/pkg/backup/lib.rs`、`br/pkg/backup/limit.rs`、`br/pkg/backup/stubs.rs`、`br/pkg/backup/Cargo.toml`。
- Go 对照：`br/pkg/backup/store.go`；上游行为与拆分/观察测试对照还读取了 `br/pkg/backup/client_test.go`。
- Rust 独立测试：`br/pkg/backup/store_test.rs` 覆盖首包与包间超时、父取消、`Stop` 取消；`br/pkg/backup/client_test.rs` 覆盖 watcher 的无变化/新 store/断连和多组拆分形状；`br/pkg/backup/parity_test.rs` 覆盖底层错误传播、响应流关闭、看门狗停止和响应载荷。
- 本任务是只读分析加文档，不运行 Cargo。结构验收应确认本文恰有任务要求的 11 个二级标题。

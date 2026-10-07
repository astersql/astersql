# `br/pkg/streamhelper/flush_subscriber.rs`

## 文件定位

本文件实现 `astersql-br-pkg-streamhelper` crate 的 TiKV log-backup flush 事件订阅管理器。模块由 [`lib.rs`](./lib.rs) 以 `flush_subscriber` 子模块装入并整包再导出；crate 边界和 `spans` 路径依赖见 [`Cargo.toml`](./Cargo.toml)。生产侧直接持有者是 [`CheckpointAdvancer`](./advancer.rs) 的 `subscriber: Mutex<Option<FlushSubscriber>>`：成为 owner 时由 `SpawnSubscriptionHandler` 创建，周期性 `subscribeTick` 调用拓扑更新、错误处理和错误汇总，停止时由 `stopSubscriber` 清理。

它位于“TiKV 各 Store 的 flush 流”与“CheckpointAdvancer 的区间检查点状态”之间：每个 Store 对应一条后台接收线程，RPC 返回的 flush 区间被转换为 `spans::Valued` 后汇入一个共享的 MPSC 通道。这里负责订阅和转发，不负责合并区间或计算全局最小检查点；后续合并语义属于 `spans` 与 advancer。

## 核心职责

- `UpdateStoreTopology` 以 `Env::Stores` 的当前结果为准，使 `subscriptions` 与 Store 集合收敛：新 Store 建立订阅，`BootAt` 变化的 Store 视为重启并重建，消失的 Store 被关闭并移除。
- `connect` 经 `Env::GetLogBackupClient` 和 `LogBackupClient::SubscribeFlushEvents` 建立流，为每条流启动独立线程，并把每个 flush 事件转换成 `Valued` 发送到共享通道。
- `HandleErrors` 检查各订阅的 `pendingError`；除错误文本包含 `unimplemented` 的情况外，先调用 `Env::ClearCache`，再重新连接。
- `PendingErrors` 将各 Store 的错误聚合为单个 `Result<(), String>`，供 `CheckpointAdvancer::subscribeTick` 报告当前订阅健康状态。
- `Clear`、`Drop` 和 `Subscription::close` 负责停止线程、等待退出以及在终止整个订阅器时关闭事件发送端。

该实现是真实可运行的 Rust 订阅路径，不是兼容门面；但它依赖 `Env` 提供的 RPC/拓扑边界，且与 Go 版本仍有明确的简化差异，详见“与 Go 版本的对应关系”。

## 主要符号

- `clearSubscriberTimeOut: Duration`：与 Go 常量同为 60 秒，但当前 Rust `Clear`/`close` 没有使用它限制 `JoinHandle::join`，因此只是保留的公开常量。
- `subscriptionIdleTimeout: Duration`：默认 10 分钟；后台线程在此期间没有收到任何批次就记录错误并退出。零值由 `connect` 的超时分支解释为禁用空闲检查。
- `SubscriberConfig = Box<dyn FnMut(&mut FlushSubscriber) + Send>`：构造期配置闭包。`WithSubscriptionIdleTimeout` 修改实例超时；`WithMasterContext(())` 目前为空操作，仅保留 Go API 的形状。
- `Subscription`：单 Store 的内部状态，保存 `storeBootAt`、跨线程共享的 `pendingError`、停止通道发送端和后台 `JoinHandle`。它不是公开 API。
- `FlushSubscriber`：公开管理器。`env: Arc<dyn Env>` 提供所有外部能力；`subscriptions` 保存 Store 状态；`eventsTunnel` 与 `eventsRx` 分别保存共享事件通道的发送端和可一次性取走的接收端；两个端点放在 `Mutex<Option<_>>` 中以支持关闭或转移所有权。
- `NewSubscriber(env, config)`：建立无界 MPSC 通道、空订阅表和默认超时，然后按顺序应用配置。
- `UpdateStoreTopology`、`HandleErrors`、`PendingErrors`：生产维护循环的三个公开阶段。
- `PushEvent`：允许调用者直接向共享事件通道注入一个 `Valued`；发送端已由 `Drop` 取走或接收端已断开时返回字符串错误。当前生产调用链未检出直接调用者。
- `TakeEventsRx`：一次性转移接收端所有权，第二次调用返回 `None`，从结构上阻止两个消费者竞争同一流。
- `SubscriptionCount`：返回当前登记数，主要供拓扑测试断言。
- `EventFromFlush`：把原始起止 key 和 checkpoint 复制为 `Span`/`Valued`；`connect` 的后台线程调用它完成事件映射。
- `Subscription::close`：发送停止信号并同步 `join` 后台线程；重复调用安全，因为两个句柄都用 `Option::take` 消耗。

## 执行流程

1. `CheckpointAdvancer::SpawnSubscriptionHandler` 调用 `NewSubscriber(self.env.clone(), Vec::new())`，把实例放入 advancer 的互斥槽位。构造器同时建立唯一的事件通道，但生产代码目前没有在本文件外检出 `TakeEventsRx` 的非测试调用；这是当前接线事实，不能据此假定事件已自动合并进 advancer。
2. `CheckpointAdvancer::subscribeTick` 持有 subscriber 锁调用 `UpdateStoreTopology`。后者先执行 `env.Stores()`；失败时立即返回错误且不修改已有订阅。
3. 对每个 Store：若 ID 尚未登记，则 `addSubscription` 创建状态并调用 `connect`；若同 ID 的 `BootAt` 改变，则先 `removeSubscription` 等待旧线程退出，再新建；否则复用现有订阅。完成扫描后，再删除不在最新 Store 集合中的旧订阅。
4. `connect` 先关闭旧线程并清空旧错误，然后取得 client、订阅 flush 流。前台任一步失败都会把错误写入 `pendingError` 并且不启动线程。
5. 成功连接后，`connect` 克隆共享事件发送端，建立停止通道并启动线程。线程每次最多阻塞 50ms，以便响应停止信号；收到一批事件时刷新 `last_activity`，逐个调用 `EventFromFlush` 并向共享通道发送。
6. 流断开时线程记录包含 Store ID 的错误；持续空闲超过配置值时记录 `no activity` 错误；共享输出接收端断开时线程直接退出，不额外写入订阅错误。
7. 同一维护 tick 随后调用 `HandleErrors`。可重试错误触发 `ClearCache` 和 `connect`；包含 `unimplemented` 的错误保留原状态，让调用方知道该 Store 不支持 flush 订阅。最后 `PendingErrors` 汇总仍存在的错误。
8. 停止时，`Clear` drain 整个映射并逐项 `close`；`Drop` 在此基础上取走 `eventsTunnel` 的发送端，使已取出的接收端在所有线程发送端消失后观察到断开。

## 数据与状态

`subscriptions: HashMap<u64, Subscription>` 的 key 是 Store ID。`storeBootAt` 是同 ID Store 实例的世代标识：ID 相同但启动时间变化必须废弃旧流。拓扑更新由 `&mut self` 串行修改映射，因此映射本身不需要内部锁。

每个 `pendingError: Arc<Mutex<Option<String>>>` 同时被管理线程与对应后台线程访问。连接前被清空；拨号/订阅失败、流断开或空闲超时写入 `Some(error)`；成功重连后恢复为 `None`。错误是状态快照而非队列，后写会覆盖前写，`PendingErrors` 则在 Store 维度拼接当前快照。

`eventsTunnel` 是无界 `std::sync::mpsc` 发送端，所有订阅线程克隆后汇入同一队列；它保持各生产者各自的发送顺序，但不承诺不同 Store 间的全局顺序。`eventsRx` 只能由 `TakeEventsRx` 取出一次。`EventFromFlush` 为起止 key 分配新的 `Vec<u8>`，checkpoint 原样写入 `Value`。

`subscriptionIdleTimeout` 在创建线程时按值复制，因此修改管理器字段只影响之后建立或重建的订阅。收到空批次也会刷新 `last_activity`，因为刷新发生在遍历 `events` 之前；这与“流有应用层活动”的判定一致。

## 依赖与调用关系

上游调用边由 `advancer.rs` 的实际引用确认：

- `CheckpointAdvancer::SpawnSubscriptionHandler -> NewSubscriber`；
- `CheckpointAdvancer::subscribeTick -> UpdateStoreTopology -> HandleErrors -> PendingErrors`；
- `CheckpointAdvancer::stopSubscriber -> FlushSubscriber::Clear`。

测试上游包括 `flush_subscriber_test.rs`、`subscription_test.rs`、`advancer_test.rs` 和 `parity_test.rs`，它们还直接使用 `TakeEventsRx`、`SubscriptionCount` 与超时配置。

下游调用边为：

- `UpdateStoreTopology -> Env::Stores`，其中 `Env` 在 `advancer_env.rs` 中组合了 `TiKVClusterMeta`、`LogBackupService`、`StreamMeta`、`RegionLockResolver` 和 flush 间隔读取能力；本文件只用到前两类能力及缓存清理。
- `connect -> Env::GetLogBackupClient -> LogBackupClient::SubscribeFlushEvents`，得到 `Receiver<Vec<FlushEvent>>` 风格的流边界。
- `connect -> EventFromFlush -> spans::Span / spans::Valued`；`spans` 来自 Cargo 路径依赖 `astersql-br-pkg-streamhelper-spans`。
- `addSubscription` 与 `HandleErrors -> Env::ClearCache`。前者忽略清缓存错误，后者也忽略错误并继续尝试连接。

RustCodeGraph 的文件节点显示本文件被 `advancer.rs` 及测试等多处引用；精确的 Rust 方法调用者查询没有返回方法级边，因此上述生产调用边以 `advancer.rs` 的直接符号引用交叉核验。

## 错误处理与边界

- `UpdateStoreTopology` 只向上传播 `Stores()` 的错误；单 Store 的连接失败被收进该 Store 的 `pendingError`，不会中止其他 Store 的拓扑对齐。
- `connect` 在 `GetLogBackupClient` 或 `SubscribeFlushEvents` 失败时保存原字符串；没有像 Go 版本一样附加分阶段上下文。后台断开和空闲错误会主动补充 Store ID。
- `HandleErrors` 用不区分大小写的字符串包含判断识别 `unimplemented`。它不像 Go 的 gRPC status code / `multierr` 判定那样结构化，因此包含该单词的非 gRPC 错误也会被视为不可重试，反之改写后的错误文本可能误判。
- `PendingErrors` 的 HashMap 遍历顺序不稳定，聚合错误字符串的 Store 顺序不应作为 API 或测试不变量。
- `Mutex::lock().unwrap()` 会在锁中毒时 panic；后台线程 panic 不会被转换为 `pendingError`，而 `close` 又忽略 `join` 返回的 panic 结果。这不同于 Go `listenOver` 的 recover 与错误上报。
- 事件发送失败只表示共享接收端已关闭；后台线程安静退出。直接 `PushEvent` 则把发送错误返回给调用者。
- `clearSubscriberTimeOut` 当前未参与清理；若 RPC 接收线程不能在最多 50ms 的轮询周期内观察到 stop，`close` 的同步等待仍没有单独超时保护。

## 并发与资源生命周期

一个成功订阅对应一个 OS 线程、一个停止通道接收端、一个流接收端、一个共享事件发送端克隆和一个 `JoinHandle`。管理面通过 `&mut FlushSubscriber` 串行执行拓扑及重连；数据面线程只共享事件发送端与 `pendingError`。

重连严格执行 `subscription.close()`、清错误、拨号、再启动新线程，避免同一 `Subscription` 同时保留两条活动流。拓扑重启路径更彻底：从 HashMap 移除旧对象、等待线程、创建新对象。`Subscription::close` 先发送 stop，再 `join`，保证返回时旧线程已结束；线程的 `recv_timeout` 轮询上限为 50ms，因此正常停止响应有界于该轮询粒度加调度时间。

事件通道的生命周期比单条订阅长：移除 Store 不关闭共享通道；只有 `FlushSubscriber::Drop` 清理全部线程并取走管理器持有的发送端。Rust 类型没有实现标准 `Drop` trait，公开的 `Drop()` 只是普通方法；若调用者直接丢弃 `FlushSubscriber` 而未先调用 `Clear`/`Drop`，`Subscription` 也没有标准析构实现，后台线程及其发送端可能继续存活。因此安全扩展或新调用点必须显式保留清理路径。

## 与 Go 版本的对应关系

直接对照文件为 [`flush_subscriber.go`](./flush_subscriber.go)，主要语义对应如下：

- 两端都以 Store ID 建表，以 `BootAt` 检测重启，聚合单 Store 错误，跳过对 `Unimplemented` 的自动重试，并用一个共享输出通道汇总事件。
- Rust 将 Go 的 `LogBackupService + TiKVClusterMeta` 两个构造参数合并成 `Arc<dyn Env>`；`WithMasterContext` 在 Rust 中是 `()` 参数的空操作，没有取消传播或父上下文语义。
- Go 的输出通道容量为 1024；Rust 使用无界 MPSC，因此不会因消费者慢而阻塞生产者，但可能在消费滞后时无界增长内存。
- Go 在发送前通过 `codec.DecodeBytes` 解码 TiKV 编码的起止 key，坏 key 会记录并跳过；Rust `EventFromFlush` 直接复制 `StartKey`/`EndKey`。因此两端 key 表示是否等价取决于 Rust `SubscribeFlushEvents` 边界是否已解码，本文件本身没有验证或转换。
- Go 为每次订阅生成 client UUID，并向 gRPC 请求传入；Rust 的抽象 `SubscribeFlushEvents()` 不接收 client ID。Go 还有指标、结构化日志和 panic recover，Rust 当前均未实现。
- Go `Clear` 通过 60 秒 context 限制等待，Rust 的同名常量未接入；Go 的取消能打断阻塞发送/RPC，Rust 通过每 50ms 轮询 stop 通道协作退出。
- Go 的拓扑新增后才连接且订阅创建时不清缓存；Rust `addSubscription` 连接后无条件调用一次 `ClearCache`。Go 在订阅 RPC 建立失败时立即清缓存，Rust 则把错误留给后续 `HandleErrors` 清理并重试。
- Go 的不可重试判断检查 gRPC code，Rust 使用错误字符串。Go 使用 `multierr` 保留结构化 error，Rust返回拼接后的 `String`。

Go 测试 `subscription_test.go` 与 Rust 的 `subscription_test.rs` 共同验证基本事件覆盖、瞬时连接错误恢复、unsupported Store、Store 移除、部分 Store 回退到 collector、后台错误和空闲超时。Rust 专用 `flush_subscriber_test.rs` 另以更小用例验证连接重试、通道关闭、实时转发和空闲错误。

## 扩展指南

- 增加拓扑规则（例如过滤 Store 类型或状态）应修改 `UpdateStoreTopology`，并在独立的 `subscription_test.rs` 增加新增、移除、同 ID 重启的回归；不要把测试内嵌进生产源文件。
- 改变重试分类应优先把 `pendingError: String` 升级为可保留错误类别的类型，并同步修改 `connect`、`HandleErrors`、`PendingErrors`。必须覆盖 `Unimplemented` 不重试、瞬时错误清缓存后恢复及多 Store 错误聚合，避免继续依赖脆弱的文本匹配。
- 接入真正的 master context 或异步 runtime 时，需要同时重审 `WithMasterContext`、`connect`、`Subscription::close` 和 `Clear`；核心不变量是重连前旧 worker 已终止，且停止不能无限等待。可参照 Go 的 context/timeout 语义，但不能只保留 API 外形。
- 若补齐 Go 的 key 解码，转换位置应在后台线程接到事件之后、构造 `Valued` 之前；要用独立测试覆盖空 key、合法编码 key、损坏 key，以及坏事件不影响同批其他事件。
- 若改变通道容量或消费模型，需要评估背压与内存：无界通道适合不阻塞 Store worker，但消费者停滞会积压；有界通道则必须确保停止信号能打断阻塞发送。
- 为类型实现标准 `Drop` trait 或引入 RAII guard 时，要避免与现有普通方法 `Drop()` 混淆，并验证显式清理、隐式析构、重连和接收端提前关闭都不会泄漏或死锁。
- 对任何生产事件接线修改，至少同步 `flush_subscriber_test.rs` 与 `subscription_test.rs`；涉及 advancer 生命周期时还应同步同目录 `advancer_test.rs`。Rust 单元测试应继续保存在独立测试文件中。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`node --file br/pkg/streamhelper/flush_subscriber.rs --offset 1 --limit 260` 与后续 261–293 行读取了完整实现；`query` 确认 `FlushSubscriber`、`NewSubscriber` 及 Go 同名符号。方法级 `callers` 未返回可靠边，因此调用边再以直接符号搜索验证。
- 生产源码：`br/pkg/streamhelper/flush_subscriber.rs`（完整 293 行）、`br/pkg/streamhelper/lib.rs`（模块声明与再导出）、`br/pkg/streamhelper/advancer_env.rs`（`Env` 组合契约）、`br/pkg/streamhelper/advancer.rs`（持有、创建、周期维护与停止调用边）。目标包没有 `doc.go`。
- crate 配置：`br/pkg/streamhelper/Cargo.toml`，确认 crate 名、library 入口、porting 元数据及 `spans` 路径依赖；本模块没有 feature 条件编译项。
- Go 对照：`br/pkg/streamhelper/flush_subscriber.go`（拓扑、gRPC、错误分类、key 解码、空闲 watcher、清理与通道语义）和 `br/pkg/streamhelper/subscription_test.go`（对应行为边界）。
- Rust 测试：`br/pkg/streamhelper/flush_subscriber_test.rs`（四个聚焦用例）、`br/pkg/streamhelper/subscription_test.rs`（完整订阅契约），并通过直接引用确认 `advancer_test.rs`、`parity_test.rs` 的上游使用。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令校验本文恰含 11 个固定二级章节，并人工检查所有行为结论均对应上述源码、调用边或对照测试。

# `pkg/timer/tablestore/notifier.rs`

## 文件定位

本文件实现 `astersql-timer-tablestore` crate 的 etcd 定时器变更通知器，是表存储完成持久化写入后向集群其他节点广播 create、update、delete 事件的边界层。crate 入口 [`lib.rs`](./lib.rs) 将 `notifier` 声明为私有模块并再导出其公开项；[`store.rs`](./store.rs) 的 `NewTableTimerStore` 在传入 `Arc<dyn EtcdClient>` 时构造 `NewEtcdNotifier`，否则退回内存通知器。

该文件不直接依赖具体 etcd SDK，也不负责 JSON 编解码。它通过公开 trait `EtcdClient` 描述租约、keep-alive、前缀监听和批量写入；当前应用侧实现位于 `pkg/session/runtime/ttl_timer_etcd.rs::RealTimerEtcdClient`。这种分层让本 crate 的直接业务依赖保持为 `astersql-timer-api`，与 [`Cargo.toml`](./Cargo.toml) 一致。

## 核心职责

1. `NewEtcdNotifier` 为指定 `cluster_id` 建立 `/tidb/timer/cluster/{cluster_id}/notify/` 前缀下的进程唯一键，并启动一个发送线程。
2. `EtcdNotifier::Notify` 把合法的 API 事件转换为 `EtcdNotifyEvent`，压入共享队列，并以容量为 1 的唤醒通道合并连续唤醒。
3. `notifyLoop` 维护 60 秒 TTL 的 lease，按至少 1 秒的间隔批量调用 `EtcdClient::put_events`；通知键由 lease 自动回收。
4. `EtcdNotifier::Watch` 将集群前缀交给适配器监听，并创建一个辅助线程，使 notifier 关闭或调用方 `Context` 取消时同步取消适配器 watch。
5. `EtcdNotifier::Close` 幂等地阻止新操作、唤醒发送线程，并等待所有发送/监听辅助线程退出。

本文件提供的是变更提示而不是持久化事实源：事件写入失败不会回滚已经成功的表事务，消费者仍应从 timer 表读取权威状态。

## 主要符号

- `notifyTimeout`（20 秒）、`minNotifyInterval`（1 秒）、`etcdNotifyKeyTTLSeconds`（60 秒）：分别约束单次写入、相邻批次发送节流和通知键租约寿命。
- `watchTimerEventCreate`、`watchTimerEventUpdate`、`watchTimerEventDelete`：与 Go 线格式一致的三个事件字符串。
- `EtcdNotifyEvent { tp, timer_id, timestamp }`：跨 crate 暴露给适配器的单事件载荷。`toWatchEvent` 验证非空 timer ID 和事件类型，再生成 API 事件；`timestamp` 不进入 `WatchTimerEvent`。
- `newNotifyEvent(tp, timer_id)`：把 API 事件类型编码成字符串并记录 Unix 秒时间戳；未知类型返回 `TimerError`。
- `EtcdClient: Send + Sync`：具体 etcd 传输的注入边界。`grant`/`keep_alive` 管租约，`watch_prefix` 返回已经解码的 `WatchTimerChan`，`put_events` 负责批量线格式和带 lease 写入。
- `State`：受 `Mutex` 保护的关闭状态、待发送队列、唯一发送线程句柄和所有 watch 辅助线程句柄。
- `NotifierInner`：由各线程共享的客户端、前缀、实例键、状态、唤醒通道和原子关闭位。
- `EtcdNotifier`：仅持有 `Arc<NotifierInner>`，实现 `api::TimerWatchEventNotifier`。
- `NEXT_KEY`：进程内单调原子序号；与进程 ID 和纳秒创建时间共同降低通知键碰撞概率。
- `notifyLoop`、`takeEvents`、`sendEvents`：发送线程的租约恢复、队列转移和实际提交三个阶段。
- `NewEtcdNotifier`、`EtcdNotifyEvent` 和 `EtcdClient` 是经 `lib.rs` 再导出的公开接口；其余状态类型和辅助函数仅在模块内可见。

## 执行流程

构造路径如下：`store.rs::NewTableTimerStore` 根据可选 etcd 客户端选择 `NewEtcdNotifier`；构造函数生成集群前缀和本实例唯一键，创建容量为 1 的 `sync_channel`，初始化队列容量为 8，随后生成 `notifyLoop` 线程并把句柄写回 `State`。

写入路径如下：

1. `store.rs::TableTimerStoreCore::Create` 在插入及读取自增 ID 成功后通知 create；`Update` 在事务提交成功后通知 update；`Delete` 仅在 `ROW_COUNT()>0` 时通知 delete。
2. `Notify` 调用 `newNotifyEvent`。未知类型被丢弃；合法事件在持锁期间追加到 `State.events`，释放锁后尝试非阻塞发送唤醒信号。
3. `notifyLoop` 每 20 毫秒检查唤醒或关闭。收到唤醒后，如果距上一批不足 1 秒，就以 10 毫秒粒度等待剩余时间，同时持续检查关闭位。
4. 首次发送或 keep-alive 通道断开后，循环依次调用 `grant(60)` 和 `keep_alive(lease_id)` 建立新租约；成功后调用 `sendEvents`。
5. `sendEvents` 通过 `takeEvents` 原子地取走整个队列，然后以本实例键、当前 lease 和 20 秒超时调用 `put_events`。应用适配器 `RealTimerEtcdClient::put_events` 将其编码为 `{ "events": [...] }` JSON，并使用 `PutOptions::with_lease` 写入 etcd。

监听路径由 `Watch` 发起：关闭前，它创建独立的可取消 `watch_ctx`，调用 `EtcdClient::watch_prefix` 获取事件通道，并生成辅助线程监视 notifier 原子关闭位和调用者 context；任一条件成立就调用 `cancel.cancel()`，实际前缀 watch、PUT 过滤和 JSON 解码由 `RealTimerEtcdClient::watch_prefix` 完成。关闭后调用 `Watch` 不再接触 etcd，而是借助已关闭的内存通知器返回一个已断开的通道。

## 数据与状态

每个 notifier 实例拥有一个稳定的 `key_prefix` 和一个唯一 `key`。同一集群的所有节点监听相同前缀，但各发送实例写自己的键，所以一次 PUT 可以被所有监听者观察，而不同实例不会互相覆盖键值。键值是一批 `EtcdNotifyEvent`；事件顺序等于它们在 `State.events` 锁内入队的顺序。

待发送事件使用 `Vec<EtcdNotifyEvent>` 聚合。容量为 1 的 `wake_tx` 只表示“有工作”，不记录事件数；即使多次 `try_send` 因通道已满失败，事件仍保留在队列中并由已有唤醒批量取走。`takeEvents` 使用 `std::mem::take` 将共享队列替换为空 `Vec`，从而缩短持锁时间。

关闭状态有两份：`State.closed` 用于在互斥区内串行化 `Watch`、`Notify`、`Close`；`NotifierInner.closed: AtomicBool` 供不持锁的后台线程快速检查。`Close` 在同一临界区中先设置两者，再移出线程句柄，保证只有第一次关闭负责 join。

## 依赖与调用关系

上游主链为 `NewTableTimerStore` → `NewEtcdNotifier`，以及表写操作 → `TimerWatchEventNotifier::Notify`。`TableTimerStoreCore::Watch` 和 `Close` 分别直接转发到 notifier。更上层的 timer runtime 通过 `TimerStore` 接口订阅事件，而不依赖这里的 etcd 细节。

下游直接依赖只有 `astersql_timer_api` 与标准库同步原语。API crate 的 `TimerWatchEventNotifier` 规定 `Watch`、`Notify`、`Close` 三个方法；`Context`、`WatchTimerChan`、事件类型和 `TimerError` 也来自该 crate。`Cargo.toml` 将 `astersql-timer-api` 声明为非可选依赖，并没有直接声明 etcd 或 serde，印证传输与线格式被移到适配器层。

应用接线位于 `pkg/session/runtime/ttl_timer_etcd.rs`：`RealTimerEtcdClient` 使用 `etcd_client::Client` 实现本文件的 trait，`encode_timer_notify_message` 保留 Go 的 `events/tp/timer_id/timestamp` JSON 形状，`decode_timer_notify_message` 将可识别且 timer ID 非空的项转换为 API 响应。该适配器还给租约与 watch 分别创建线程和 Tokio current-thread runtime，并在自身 `Drop` 时停止、join 它们。

RustCodeGraph 对 `notifier.rs` 的文件查询显示它被 10 个文件引用，并将 `NewEtcdNotifier`、`notifyLoop`、`sendEvents` 同 Go 对应符号一起索引；精确调用点又由 `rg` 核对到 `store.rs`、`sql_test.rs` 和 `ttl_timer_etcd.rs`。

## 错误处理与边界

- `EtcdNotifyEvent::toWatchEvent` 拒绝空 timer ID 和未知字符串；`newNotifyEvent` 拒绝未知 API 事件类型。两者返回带上下文文本的 `TimerError`。
- `Notify` 的接口没有返回值，因此事件构造失败、关闭后通知和唤醒通道发送失败都不会向调用者传播。唤醒失败通常代表已有唤醒待消费，而非队列丢失。
- `grant` 或 `keep_alive` 失败时，`notifyLoop` 保留共享事件队列但消耗当前唤醒；若没有后续 `Notify`，循环不会主动重试这批事件。新增重试策略时必须明确退避和关闭响应性。
- `sendEvents` 先清空队列，再忽略 `put_events` 的错误；因此写入失败会丢失该批通知。这与通知为非权威提示的定位一致，也与 Go 实现“记录错误但不回填队列”的语义一致。
- keep-alive 通道断开会把 `lease_id` 重置为 0；下一次收到通知时才申请新 lease。通道中的正常 `()` 只证明连接仍活跃，不触发写入。
- `Mutex::lock` 使用 `expect`，锁中毒会 panic；线程 `join` 结果被忽略。`SystemTime` 早于 Unix epoch 时用零时间兜底。
- `Watch` 不在本层解析或过滤 etcd 数据；畸形 JSON、非 PUT 事件、未知事件项的处理取决于 `EtcdClient` 实现。当前 `RealTimerEtcdClient` 忽略非 PUT 和无法解码的消息，并在一条消息内过滤无效事件项。

## 并发与资源生命周期

`EtcdNotifier` 通过 `Arc` 共享，`EtcdClient: Send + Sync` 保证传输对象可跨线程。所有队列和线程句柄受同一 `Mutex<State>` 保护；耗时的 lease/PUT、睡眠和 join 均不在该锁内执行。原子关闭位使用 Release 写和 Acquire 读，确保后台线程在观察关闭后也能看到此前状态更新。

每个 notifier 恰有一个 `notifyLoop`。每次成功的 `Watch` 增加一个短轮询辅助线程；真正的 etcd watch 线程由适配器拥有。辅助线程每 10 毫秒观察调用方取消或全局关闭，随后取消 `watch_ctx` 并退出。`Close` 先发布关闭、用唤醒信号缩短发送线程的阻塞，再依次 join 发送线程和所有辅助线程；重复调用立即返回。

这里没有 `Drop for EtcdNotifier`，因此调用方必须显式执行 `TimerStore::Close`/`TimerWatchEventNotifier::Close` 才能得到确定的线程回收。仅丢弃最后一个外部 `Arc` 不足以停止发送线程，因为线程自身持有 `Arc<NotifierInner>`。

节流按发送尝试计算：`last_notify` 在 lease 获取之前更新。它限制批次尝试频率而不是事件产生频率；连续事件会在共享队列中合并。关闭发生在节流等待期间时，循环最多经过当前 10 毫秒睡眠片段后退出，尚未发送的队列不会在关闭阶段强制 flush。

## 与 Go 版本的对应关系

同路径 [`notifier.go`](./notifier.go) 是主要语义基准。Rust 保留了 20 秒写入超时、1 秒最小间隔、60 秒 lease TTL、三个事件字符串、每实例唯一键、容量为 1 的合并唤醒、批量发送、lease 断线重建以及 `Close` 等待工作协程等设计。

主要结构映射为：Go `notifyEvent` ↔ Rust `EtcdNotifyEvent`，`etcdNotifier` ↔ `EtcdNotifier`/`NotifierInner`/`State`，`notifyLoop`/`takeEvents`/`sendEvents` ↔ 同名 Rust 辅助函数，Go `context + cancel + WaitGroup` ↔ Rust 原子关闭位、可取消 `api::Context` 和保存后 join 的 `JoinHandle`。

实现边界有意不同：Go 文件直接持有 `clientv3.Client`、执行 JSON 编解码并记录日志；Rust 文件通过 `EtcdClient` 抽象这些工作，真实 etcd 与 JSON 逻辑在 `pkg/session/runtime/ttl_timer_etcd.rs`。Go 使用 UUID 生成键和 watcher ID，Rust 键使用 PID、创建纳秒和 `NEXT_KEY`，且本层不记录 watcher 日志。

Go 的 `Watch` 自己创建输出通道并逐条解析 etcd 响应；Rust 的 `Watch` 直接返回适配器通道，只负责生命周期联动。当前 Rust 测试覆盖关闭传播和 wire shape，但没有等价于 Go `TestEtcdNotifier` 的真实 etcd 双实例端到端测试，因此完整跨节点广播仍以 Go 集成测试为直接基准，而不是声称已由 Rust 本地测试完全证明。

## 扩展指南

- 新增事件类型时，应同步修改 API 的 `WatchTimerEventType`、本文件的字符串常量、`newNotifyEvent`、`EtcdNotifyEvent::toWatchEvent`，以及 `ttl_timer_etcd.rs` 的编码/解码匹配；还要同步 Go 线格式，避免混合版本集群丢弃事件。
- 调整 JSON 结构时不要在本文件中引入 serde 或具体 etcd SDK；应保持 `EtcdClient` 边界，并在 `pkg/session/runtime/ttl_timer_etcd_test.rs` 增加 Go 兼容的编码、解码和无效项测试。
- 调整发送可靠性时，修改 `notifyLoop`/`sendEvents` 并明确失败后是否重排队、重试、去重。通知可能重复或丢失，消费端不应把它升级为权威状态；无限重试还可能阻塞关闭或造成内存增长。
- 调整批处理或节流时，应覆盖高并发 `Notify`、单次唤醒合并、顺序保持、grant/keep-alive/put 失败和关闭期间未发送事件。独立 Rust 测试应继续放在现有 `sql_test.rs` 或新增同目录 `notifier_test.rs`，并从 `lib.rs` 以 `#[cfg(test)] #[path = ...]` 接入，不能把测试内嵌进生产文件。
- 调整 `Watch` 生命周期时，应保持调用方 context 取消只终止自己的 watcher，而 `Close` 终止全部 watcher且关闭后不创建新 etcd watch。现有 `sql_test.rs::test_etcd_notifier_close_cancels_watchers` 是最接近的回归入口。
- 若为 notifier 增加自动资源释放，需要特别处理发送线程持有内部 `Arc` 的自引用生命周期，不能简单依赖外部 `EtcdNotifier` 的 `Drop`。

## 验证依据

- 生产源码：`pkg/timer/tablestore/notifier.rs`（全部 325 行），包括 `EtcdNotifyEvent`、`EtcdClient`、`NewEtcdNotifier`、`notifyLoop`、`sendEvents` 和 `TimerWatchEventNotifier` 实现。
- crate 与接线：`pkg/timer/tablestore/Cargo.toml`、`pkg/timer/tablestore/lib.rs`、`pkg/timer/tablestore/store.rs`、`pkg/timer/api/store.rs::TimerWatchEventNotifier`。
- 真实适配器与线格式：`pkg/session/runtime/ttl_timer_etcd.rs`；Rust wire-shape 测试：`pkg/session/runtime/ttl_timer_etcd_test.rs::go_merge_43_timer_etcd_notice_matches_go_wire_shape`。
- Rust notifier 测试：`pkg/timer/tablestore/sql_test.rs::test_etcd_notifier_close_cancels_watchers`，验证关闭现有 watcher、关闭后返回断开通道且不新增底层 watch。
- Go 对照与端到端行为：`pkg/timer/tablestore/notifier.go`；`pkg/timer/store_intergartion_test.go::TestEtcdNotifier`/`runNotifierTest` 验证同实例及双实例广播、批量顺序、单 watcher 取消、整体关闭和关闭后忽略通知。
- RustCodeGraph：`status` 显示当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/timer/tablestore` 找到 Rust/Go 源与独立测试；`node --file pkg/timer/tablestore/notifier.rs` 展示完整源并报告 10 个引用文件；`query NewEtcdNotifier`、`query notifyLoop`、`query sendEvents` 均返回 Rust 与 Go 对应符号。`callers`/`callees` 未输出边明细，因此又以精确 `rg` 核对上述调用点。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证为固定十一章节结构检查、路径/链接核对和 diff 人工复核。

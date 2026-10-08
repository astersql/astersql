# `pkg/session/runtime/ttl_timer_etcd.rs`

## 文件定位

本文件属于 `astersql-session` crate 的会话运行时内部模块，由 `pkg/session/runtime.rs` 以私有模块 `mod ttl_timer_etcd` 装配。它不是 TTL 业务调度器，而是一个边界适配器：把 `etcd_client::Client` 的异步 lease/watch/put API 包装成 `astersql_timer_tablestore::EtcdClient` 所要求的同步 Rust trait，并维持与 Go 定时器通知相同的 JSON 线协议。

实际接线从 `pkg/session/runtime/ttl_runtime.rs` 的 `EtcdTtlWatchTransport::timer_notifier` 开始：它构造 `RealTimerEtcdClient`，再由 `start_domain_ttl_job_manager_with_interval` 传入 `astersql_timer_tablestore::NewTableTimerStore`。因此该适配器位于“Session/Domain 启动 TTL timer runtime”与“TableTimerStore 跨节点通知”之间，不直接处理 SQL、TTL 扫描或 timer 持久化。

## 核心职责

- `RealTimerEtcdClient` 保存可 clone 的 etcd client、keyspace namespace、全局停止标志和已启动线程句柄，实现定时器 table store 定义的 `EtcdClient` 边界。
- `grant` 和 `put_events` 把同步 trait 调用转成当前线程上的单线程 Tokio runtime 操作，并对 etcd I/O 施加超时。
- `keep_alive` 和 `watch_prefix` 为长生命周期流各启动专用 OS 线程，用通道把 lease 存活信号或 `WatchTimerResponse` 传回上层。
- `encode_timer_notify_message` / `decode_timer_notify_message` 负责 `{ "events": [...] }` 线格式，将 `create`/`update`/`delete` 映射到 timer API 的事件位标志。
- `Drop` 在最后一个适配器实例销毁时发布停止信号并 join 所有已跟踪 worker，防止后台线程脱离资源所有者。

## 主要符号

- `RealTimerEtcdClient { client, namespace, stopped, workers }`：`pub(super)` 结构体，只向 `runtime` 父模块可见。`client` 是 etcd 连接句柄；`namespace` 是所有定时器 key 的前缀；`stopped` 供多个 worker 共享关闭状态；`workers` 持有 OS 线程句柄。
- `RealTimerEtcdClient::new(client, namespace)`：构造器，初始停止位为 `false`，worker 列表为空。
- `key(&self, key)`：以字符串直接拼接 `namespace + key`；调用方必须保证 namespace 和业务 key 的斜杠边界正确。
- `runtime()`：每次创建启用全部 driver 的 current-thread Tokio runtime，错误被转为带 `start timer etcd runtime` 上下文的 `String`。
- `track(handle)`：在加入新 worker 前清理已结束 worker；已结束线程会被 join，未结束线程继续保留。
- `decode_timer_notify_message(value)`：解析 JSON，要求顶层 `events` 为数组；每项要有非空字符串 `timer_id` 且 `tp` 为三种已知值才转换，其他项被过滤；`timestamp` 不进入 API 响应。
- `encode_timer_notify_message(events)`：将 `EtcdNotifyEvent` 序列化为含 `tp`、`timer_id`、`timestamp` 的事件数组。
- `impl EtcdClient for RealTimerEtcdClient`：对外的四个行为入口为 `grant`、`keep_alive`、`watch_prefix` 和 `put_events`。
- `impl Drop for RealTimerEtcdClient`：设置 `stopped` 后排空并 join `workers`。文件无 feature gate、宏生成符号或模块级常量；固定超时目前直接写在方法中。

## 执行流程

1. Session 启动路径在 `pkg/session/runtime/session.rs` 用真实 etcd client 和 namespace 创建 `EtcdTtlWatchTransport`。TTL job manager 初始化 timer runtime 时，`EtcdTtlWatchTransport::timer_notifier` 创建 `RealTimerEtcdClient`，`NewTableTimerStore` 再用它创建 `EtcdNotifier`。
2. 当 timer store 发生 create/update/delete 时，`pkg/timer/tablestore/notifier.rs` 的通知循环累积 `EtcdNotifyEvent`。首次发送前它调用本文件的 `grant(60)` 获取 lease，再调用 `keep_alive(lease_id)` 启动保活线程。
3. `keep_alive` 先启动名为 `ttl-timer-etcd-lease` 的线程，在5 秒内建立 etcd keep-alive keeper/stream，并通过容量 1 的 ready 通道将初始化成败同步回传。成功后大约每 10 秒发一次 keep-alive，在5 秒写超时和 3 秒回复超时内要求成功；每次成功向返回的 `mpsc::Receiver<()>` 发存活信号，失败则断开通道，上层 notifier 因此会废弃 lease 并重建。
4. 批量通知通过 `put_events` 编码，以 notifier 传入的超时（当前为 20 秒）将 namespaced key/value 写入 etcd，并绑定当前 lease。
5. 监听方调用 `watch_prefix`。本文件启动 `ttl-timer-etcd-watch` 线程，在5 秒内创建 prefix watch，之后以 100 毫秒超时轮询 etcd stream，使 worker 能定期检查全局 `stopped` 和调用者 `Context`。
6. watch 只处理未取消响应中的 `Put` 事件。对每个 KV，成功解码后把一个 `WatchTimerResponse` 发入 unbounded crossbeam 通道；接收端丢弃、stream 结束/取消/报错或 context 取消都会结束 worker。
7. `RealTimerEtcdClient` 销毁时将 `stopped` 置位，watch 最多经过一次 100 毫秒轮询即可观察关闭；lease worker 在内层 100 毫秒 sleep 间隔也会重新检查。随后 `Drop` 同步等待所有 worker 退出。

## 数据与状态

etcd key 是 `namespace` 与 table-store notifier 生成的 `/tidb/timer/cluster/{cluster_id}/notify/{unique-id}` 直接拼接的结果；watch 使用同样的 namespaced `/tidb/timer/cluster/{cluster_id}/notify/` 前缀。通知 value 是 JSON 对象，其 `events` 是批量数组，单项含字符串 `tp`、字符串 `timer_id` 和 Unix 秒 `timestamp`。接收端只把前两者映射为 `WatchTimerEvent`，时间戳是线协议兼容字段，不影响本地事件语义。

`stopped: Arc<AtomicBool>` 是所有长生命周线程的单向状态：构造时为 `false`，只在 `Drop` 以 Release 语义写入 `true`，worker 以 Acquire 语义读取，不支持重启。`workers: Mutex<Vec<JoinHandle<()>>>` 是资源所有权账本；`track` 逐次回收已完成线程，`Drop` 回收剩余线程。

通道承担三种不同含义：`ready_receiver` 是 keep-alive 启动栅栏；`mpsc::Receiver<()>` 的断开是 lease 不再存活的信号；`WatchTimerChan` 是多批 timer 事件的消费界面。watch 通道是 unbounded，因此慢消费者不会反压 etcd worker，但持续积压会带来内存增长风险。

## 依赖与调用关系

`pkg/session/Cargo.toml` 将本文件归入 `astersql-session` library，并显式声明 `astersql-timer-api`、`astersql-timer-tablestore`、`tokio`、`etcd-client` 和 `crossbeam-channel`。直接依赖分工如下：

- `astersql_timer_tablestore::{EtcdClient, EtcdNotifyEvent}` 定义适配器 trait 和写入载荷；`pkg/timer/tablestore/notifier.rs` 是本适配器的直接上层消费者。
- `astersql_timer_api` 提供可取消 `Context`、事件类型常量、`WatchTimerEvent`、`WatchTimerResponse` 和 `WatchTimerChan`。
- `etcd_client` 提供 lease grant/keep-alive、prefix watch、PUT event 分类、带 lease 的 put 及 TLS 能力。
- `tokio` 在同步 trait 边界内驱动 etcd future 和 I/O 超时；`std::thread` 承载长生命周 future；标准 `mpsc` 传递 ready/keep-alive，crossbeam 通道匹配 timer API 类型。

上游唯一生产构造点是 `pkg/session/runtime/ttl_runtime.rs` 的 `EtcdTtlWatchTransport::timer_notifier`；再上游是 `start_domain_ttl_job_manager_with_interval` 创建 `NewTableTimerStore`。RustCodeGraph 对 `RealTimerEtcdClient` 和 `decode_timer_notify_message` 的精确符号查询成功，但 callers/callees 未返回边，所以这些关系以上述模块声明和直接构造/调用点核对，不以空图结果推断“无调用”。

## 错误处理与边界

`grant`、`keep_alive` 初始化和 `put_events` 保留错误返回边界，将 runtime 创建、启动线程、超时和 etcd 错误分别加上操作上下文。`keep_alive` 不会在返回成功前隐藏建链失败；它最多等待 5 秒 ready 回执，内部初始化错误通过嵌套 `Result` 传出。建链后的失败则通过 worker 退出和返回通道断开表示。

`watch_prefix` 的 trait 签名不能返回 `Result`，因此创建 OS 线程失败、Tokio runtime 失败、watch 建立超时/失败都只表现为 sender 被丢弃后 receiver 断开，本文件不记录日志。已建立 stream 的 canceled/EOF/error 也终止而不自动重连；重新创建 watch 由更上层的生命周期负责。

JSON 整体非法或缺少数组形式的 `events` 时，`decode_timer_notify_message` 返回错误，watch 路径忽略该 KV；单个事件的空/missing `timer_id`、未知 `tp` 或类型不匹配仅过滤该项，同批有效项仍会传递，也可能传递空 `Events`。非 PUT etcd 事件始终被忽略。

`workers` 锁中毒在 `track`/`Drop` 中会 panic，这是显式的不可恢复内部不变量。连接地址、TLS、认证、namespace 正确性以及 notifier 批处理/重试策略不属于本文件责任。

## 并发与资源生命周期

每个 `RealTimerEtcdClient` 共享一个原子停止标志，但每次 `keep_alive` 和成功 spawn 的 `watch_prefix` 都创建独立 OS 线程和独立 current-thread Tokio runtime。这避免同步 trait 调用者必须持有 Tokio handle，代价是每个流都有线程/runtime 开销。`etcd_client::Client` 被 clone 进入各 worker，namespace 在启动前计算成 worker 拥有的 key。

`track` 在同一 mutex 下 drain 句柄、join 已完成线程、重建 active 列表并添加新句柄。它不 join 活跃 worker，因此正常调用不会等待长流退出。`Drop` 则先发布停止位，再持锁 drain，最后逐个 join；在正常路径下，watch 的 100 毫秒超时和 keep-alive 的短 sleep/网络超时为关闭提供有限检查点。

`watch_prefix` 同时观察适配器生命周期与单个调用者 context：任一取消都退出。其 crossbeam sender 随 worker 退出而丢弃，接收方可通过断开识别流结束。keep-alive 中若上层丢弃 receiver，下一次发信号会失败并终止 worker。

## 与 Go 版本的对应关系

Go 的业务对照是 `pkg/timer/tablestore/notifier.go`，而非 `pkg/session` 下的同名文件。Go `etcdNotifier` 把通知缓冲、一秒节流、60 秒 lease、keep-alive、20 秒 PUT、prefix watch 和 JSON 转换放在同一类中。Rust 将其拆成两层：`pkg/timer/tablestore/notifier.rs` 保留事件聚合、lease 重建、节流和 notifier 关闭；本文件仅提供真实 etcd 传输与线协议。

对齐点包括：通知路径 `/tidb/timer/cluster/{clusterID}/notify/`、每进程/通知器独立 key、`create|update|delete` 字符串、`timer_id` 与 `timestamp` 字段、只处理 etcd PUT、按前缀 watch、lease 绑定以及关闭时等待 worker。`pkg/session/runtime/ttl_timer_etcd_test.rs` 明确检查 Rust 编码产物的 Go 字段形状和三种事件映射中的 create/update/delete 子集。

当前可观察差异必须保留为兼容风险：Go JSON unmarshal 遇到缺失 `events` 会得到空切片响应，Rust 解码器则返回 `timer notice missing events`并丢弃该 KV；Go 对无效单项记错误日志，Rust 静默过滤；Go watch 同时直接 select 调用 context 和 notifier context，Rust 以 100 毫秒轮询提供取消响应。Go 也会在 watch 建立后记录生命周期和解码错误，本 Rust 适配器当前没有等价日志。

## 扩展指南

- 新增或改动线协议字段时，首先同步 `encode_timer_notify_message` 和 `decode_timer_notify_message`，然后检查 `pkg/timer/tablestore/notifier.rs` 的 `EtcdNotifyEvent` 以及 Go `notifyMessage`/`notifyEvent`。必须扩展独立测试 `pkg/session/runtime/ttl_timer_etcd_test.rs`，覆盖双向兼容、缺字段、未知类型和混合有效/无效批次；不要把测试内嵌到生产文件。
- 改动 lease 节奏、超时或重连策略时，本文件与 `pkg/timer/tablestore/notifier.rs` 是一个不可分割的协议：下层通道断开代表 lease 失效，上层据此将 `lease_id` 归零。应用可控 fake `EtcdClient` 测试 grant/keep-alive/put 失败，并在需要真实语义时扩展 `pkg/session/runtime/ttl_runtime_test.rs` 的 real-etcd 场景。
- 改动 namespace/key 组合时，不能只修改 `key()`；还需与 `EtcdNotifier::NewEtcdNotifier` 生成的以 `/` 开头的 prefix 以及 `EtcdTtlWatchTransport` 中其他 etcd key 保持一致，并测试空 namespace、带尾斜杠 namespace 和 keyspace namespace，避免跨 keyspace 监听或双斜杠不兼容。
- 若要增加可观测性，最合适的接入点是 `watch_prefix` 的 spawn/runtime/watch/decode 失败分支、`keep_alive` 稳态退出分支和 `track` 的已完成 worker 回收；日志不应包含未脱敏的完整 payload。
- 若要改为共享 Tokio runtime 或增加有界 watch 通道，需要重新证明同步 trait 不会在 runtime 内嵌 `block_on`、慢消费者不会阻塞 etcd 保活，以及 `Drop` 不会在持锁期间与 worker 形成循环等待。

## 验证依据

- 源码全貌：`pkg/session/runtime/ttl_timer_etcd.rs`，核对 `RealTimerEtcdClient`、两个 JSON helper、四个 `EtcdClient` 方法和 `Drop`。
- 模块与 crate：`pkg/session/runtime.rs` 声明生产模块和独立测试模块；`pkg/session/Cargo.toml` 声明 `astersql-session` 库边界及 timer API/table store、Tokio、etcd client、crossbeam 依赖。`pkg/session` 下未找到 `doc.go`，因此包契约以 `runtime.rs` 的模块注释和实际接线为准。
- 上下游：`pkg/session/runtime/session.rs` 创建 `EtcdTtlWatchTransport`；`pkg/session/runtime/ttl_runtime.rs` 的 `timer_notifier` 构造本适配器，`start_domain_ttl_job_manager_with_interval` 将其传入 `NewTableTimerStore`；`pkg/timer/tablestore/notifier.rs` 定义 trait、lease 管理、批处理和重建语义；`pkg/timer/api/store.rs` 定义 context 和 watch 数据结构。
- Go 对照：`pkg/timer/tablestore/notifier.go` 的 `notifyEvent`、`etcdNotifier.Watch`、`notifyLoop`、`newLease`、`takeEvents`、`sendEvents` 和 `Close`。
- 测试证据：`pkg/session/runtime/ttl_timer_etcd_test.rs::go_merge_43_timer_etcd_notice_matches_go_wire_shape` 覆盖编码字段、create/update 解码和混合未知/delete 批次；`pkg/timer/tablestore/sql_test.rs::test_etcd_notifier_close_cancels_watchers` 验证 notifier 关闭会取消已有 watcher 且不再建立新 watch；`pkg/session/runtime/ttl_runtime_test.rs::go_merge_43_ttl_two_domains_real_etcd_timer_event` 以两个 Domain 的真实 etcd transport 覆盖跨节点 timer 事件与接管路径。
- RustCodeGraph：`status` 显示仓库索引可用；`query RealTimerEtcdClient --limit 20` 定位结构体到目标文件；`query decode_timer_notify_message --limit 20` 定位函数和测试引用。`files --filter pkg/session/runtime/ttl_timer_etcd` 与 callers/callees 未给出可用文件/调用边，因此调用关系改由上述直接源码搜索核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证仅执行任务指定的结构检查，并人工复核了“为何存在、如何运行、如何安全扩展”三项。

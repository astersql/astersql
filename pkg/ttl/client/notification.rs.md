# `pkg/ttl/client/notification.rs`

## 文件定位

本文件是 `astersql-ttl-client` crate 的通知通道实现，源码为 [`notification.rs`](notification.rs)，由 [`lib.rs`](lib.rs) 公开为 `notification` 模块并再导出全部公开项。它与 [`command.rs`](command.rs) 的请求/响应命令通道并列：通知通道只按类型发布事件，不分配请求 ID，也不等待响应。`Cargo.toml` 将该 crate 映射到 Go 包 `pkg/ttl/client`；当前清单没有常规外部依赖，通知实现直接复用同 crate 的上下文、内存 etcd 和 mock 基础设施。

在完整应用的现有接线中，Go 版 [`notification.go`](notification.go) 被 [`../ttlworker/job_manager.go`](../ttlworker/job_manager.go) 使用：创建 TTL 作业后发送 `scan` 通知，各节点的 manager loop 收到通知后重新调度扫描任务。仓库内 Rust 生产代码尚未调用 `new_notification_client`、`new_mock_notification_client` 或 `NotificationClient`；Rust 实现目前由 [`command_test.rs`](command_test.rs) 直接验证。因此，本文件已经提供可用的 Rust 通知抽象和进程内后端，但不能据此声称 Rust TTL worker 主链已经接入。

## 核心职责

- 以 `TTL_NOTIFICATION_PREFIX` 统一通知键空间，实际键为 `/tidb/ttl/notification/<notification_type>`。
- 用 `NotificationClient` 规定发布 `notify` 和按类型订阅 `watch_notification` 两个操作，并通过 `Send + Sync` 允许客户端在并发组件间共享。
- 为 `EtcdClient` 实现带一秒租约的键写入及 watch 事件转换；这里只转发 `Put`，忽略 `Delete`。
- 为 `MockClient` 实现与 Go mock 对齐的、按通知类型分组的容量为 8 的订阅通道；mock 发送空事件，不保留传入的 `data`。
- 用 `NotificationReceiver` 隐藏标准库接收端，同时提供阻塞、限时和非阻塞三种消费方式。

## 主要符号

- `TTL_NOTIFICATION_PREFIX: &str`：公开键前缀；真实客户端发布和订阅必须用相同的 `notification_type` 才会命中同一精确键。
- `NotificationEvent { pub data: String }`：Rust 接收端看到的载荷。真实后端从 etcd 字节值宽容解码；mock 固定产生 `Default`，即空字符串。
- `NotificationReceiver`：持有私有的 `mpsc::Receiver<NotificationEvent>`，公开 `recv`、`recv_timeout`、`try_recv`。它没有 `Clone`，单次订阅由单个接收端消费。
- `NotificationClient: Send + Sync`：对象安全的公开 trait。`notify(&ClientContext, &str, &str) -> Result<(), ClientError>` 返回发布错误；`watch_notification(ClientContext, &str) -> NotificationReceiver` 立即返回订阅句柄，不以 `Result` 报告注册错误。
- `new_notification_client(Arc<EtcdStore>) -> Arc<dyn NotificationClient>`：以共享 `EtcdStore` 构造 `EtcdClient`，并擦除为 trait object。
- `new_mock_notification_client() -> Arc<MockClient>`：构造 mock 的具体类型；调用者仍可通过已实现的 `NotificationClient` trait 使用它。
- `impl NotificationClient for EtcdClient`：真实语义实现，依赖 `EtcdStore::put` 与 `EtcdStore::watch`。
- `impl NotificationClient for MockClient`：测试语义实现，依赖 `MockState::notification_watchers`、`MockNotificationWatcher` 和 `MockClient::next_watcher_id`。

## 执行流程

真实发布流程从 `EtcdClient::notify` 开始：先拼接前缀和类型，再把 `data` 转成字节，调用 `EtcdStore::put(ctx, key, value, Some(1))`。`put` 先检查 `ClientContext`，再处理注入的后端失败、清理过期项、写入带一秒过期时间的值，并向精确匹配该键的 store watcher 投递 `Put` 事件。

真实订阅流程由 `EtcdClient::watch_notification` 注册 `EtcdStore::watch(ctx.clone(), key, false)`，其中 `false` 表示精确键而非前缀匹配。函数另建一个无界 `mpsc` 通道并启动转发线程。线程每 20 ms 检查上游事件：`Put` 被转换为 `NotificationEvent`；其他事件和超时被忽略；上游断开、下游 receiver 被丢弃或上下文取消时退出。随后原函数把下游 receiver 包装为 `NotificationReceiver` 返回。

mock 订阅由 `MockClient::watch_notification` 创建容量 8 的 `sync_channel`，取得 watcher ID，并把 sender 追加到对应类型的 watcher 列表。mock 发布由 `MockClient::notify` 在持锁状态下查找该类型：没有 watcher 时立即成功；有 watcher 时依序 `try_send` 空事件。遇到已断开的 watcher 就跳过；遇到第一个已满通道时，将它及后续 sender 复制到 `unsent`，释放全局状态锁后启动线程，每 10 ms 重试，直至发送成功、通道断开或上下文取消。

## 数据与状态

真实路径的持久状态属于 [`command.rs`](command.rs) 的 `EtcdStore`：键值保存在 `EtcdState::entries`，可选的 `expires_at` 在后续 store 操作时由 `purge_expired` 惰性清理；watcher 保存在 `EtcdState::watchers`。一秒租约限制键的存留时间，但 `EtcdStore` 不会仅因时间到达主动生成 `Delete` 事件。

mock 路径把 `HashMap<String, Vec<MockNotificationWatcher>>` 保存在共享的 `MockState` 中。键是未经前缀拼接的 `notification_type`，每次订阅都会追加一个 watcher。`MockNotificationWatcher.id` 会被分配，但本文件不使用它移除 watcher；这与 Go mock 在其生命周期内保留通知 watcher 的行为一致。事件本身无序号、时间戳、发送者或确认状态，同一类型的连续写入仅依靠 watch 投递表达变化。

## 依赖与调用关系

向下依赖均来自 [`command.rs`](command.rs)：`ClientContext` 提供取消/截止时间检查，`ClientError` 表示取消、超时或后端错误，`EtcdStore` 提供带租约的 put 和 watch，`EtcdEventKind` 区分 `Put`/`Delete`，`EtcdClient` 与 `MockClient` 承载两套实现。标准库依赖为 `Arc`、`mpsc`、线程和 `Duration`。

RustCodeGraph 将目标文件识别为 20 个符号，并显示文件级关系 `notification.rs` 被 `command.rs` 使用；精确 callers/callees 查询没有解析出 trait object 的动态调用。`rg` 补查显示 Rust 侧直接调用者仅为 [`command_test.rs`](command_test.rs) 中的两个通知测试。Go 对照主链则是 `JobManager.appendLockedJob -> NotificationClient.Notify("scan", id)`，以及 `JobManager.loop -> WatchNotification("scan") -> taskManager.rescheduleTasks`；watch channel 意外关闭时会重新订阅。Go 集成测试 [`../ttlworker/job_manager_integration_test.go`](../ttlworker/job_manager_integration_test.go) 的 `TestTriggerScanTask` 验证提交作业后确实收到通知。

## 错误处理与边界

`EtcdClient::notify` 原样传播 `EtcdStore::put` 的 `ClientError`，包括发布前已取消/超时和注入的后端失败。`watch_notification` 没有错误返回值；注册后发生的取消或通道断开只能体现为转发线程退出，最终使 `NotificationReceiver` 返回标准库的 `Disconnected`。真实事件值使用 `String::from_utf8_lossy`，无效 UTF-8 不会报错，而会被替换字符修复。

真实订阅忽略所有非 `Put` 事件。它使用精确键 watch，所以不同通知类型互不串扰；空类型仍会映射到公共前缀本身，代码没有额外校验类型格式。发布是覆盖同一键的最新值，而不是持久队列；消费者应把通知视为“状态已变化”的提示，不能用它保证每个业务事件被永久保存或确认。

mock 有一个刻意保留的边界：无 watcher 时在检查上下文之前返回 `Ok(())`；有 watcher 时才可能因上下文取消返回错误。遇到满通道后的异步重试不会把后来发送失败反馈给原 `notify` 调用，且返回 `Ok(())` 只表示同步阶段未报错。内部 `Mutex::lock().unwrap()` 在锁中毒时会 panic，这不是 `ClientError` 路径。

## 并发与资源生命周期

真实客户端和 mock 都通过 `Arc` 共享；trait 的 `Send + Sync` 约束允许跨线程持有。每次真实订阅至少产生两个后台活动：`EtcdStore::watch` 启动一个线程，在上下文结束后从 store 删除 watcher；本文件再启动一个转换线程，轮询上游事件并向下游无界通道发送。取消 `ClientContext`、丢弃 `NotificationReceiver` 或 store sender 断开都会最终终止转换线程，但没有 join 句柄，退出是异步的。

mock 的 watcher 通道容量固定为 8。在容量未满时，`notify` 持有 `MockState` 锁并同步发送；容量满后先克隆剩余 sender，再释放锁并异步重试，避免等待消费者时长期占用全局锁。这个后台线程同样没有 join 句柄。取消传给 `watch_notification` 的上下文不会注销 mock watcher；只有 receiver 丢弃后 sender 才会表现为断开，而列表项仍保留到整个 `MockClient` 销毁。

## 与 Go 版本的对应关系

Rust `NotificationClient`、`new_notification_client`、`notify` 和 `watch_notification` 分别对应 Go `NotificationClient`、`NewNotificationClient`、`Notify` 和 `WatchNotification`。两版共享完全相同的键前缀；真实发布都使用一秒租约，真实订阅都 watch 拼接后的精确键。Rust 用本地 `EtcdStore` 模拟 etcd，并把 etcd watch 响应收敛为仅含文本的 `NotificationEvent`；Go 直接返回 `clientv3.WatchChan`，因此包含更完整的 etcd 响应和错误信息。

mock 对齐点更明确：两版均按类型保存 watcher、每个 watcher 容量为 8、无 watcher 时成功、发布空事件而忽略 `data`，并在第一个阻塞 watcher 处把剩余投递转入后台。Go 后台 goroutine 对每个剩余 channel 在“上下文取消”和“阻塞发送”之间选择；Rust 以 10 ms `try_send` 重试近似这一可取消等待。两版 mock 都不因订阅上下文结束而主动移除 notification watcher。

差异也必须保留在使用预期中：Go 真实 watch 直接暴露 etcd channel 的关闭和错误响应；Rust 转换层忽略 `Delete`，且无法把异步 watch 错误作为结构化 `ClientError` 交给调用者。Go TTL worker 已把该接口接入生产调度循环，而仓库当前 Rust TTL worker 没有对应调用边。

## 扩展指南

若增加新的通知类型，优先保持 `TTL_NOTIFICATION_PREFIX` 不变，并在实际生产调用方用稳定、双方一致的类型字符串；通知载荷若承载结构化数据，应在 `NotificationEvent` 和真实转换处定义兼容编码，同时决定 mock 是否继续按 Go 语义丢弃载荷。修改租约、键格式、事件筛选或精确/前缀 watch 时，必须同步核对 Go [`notification.go`](notification.go) 和 TTL worker 的容错重订阅逻辑。

若要增强可靠性或暴露 watch 错误，应从 `NotificationReceiver`/`NotificationEvent` 的 API 设计入手，而不是只改后台线程；需明确 channel 关闭、取消、错误、积压和重复通知的契约。改变 mock watcher 清理或背压策略会偏离 Go 测试替身语义，必须有明确迁移动机。

相关测试应继续放在独立文件 [`command_test.rs`](command_test.rs)，不要内嵌到生产源文件。至少覆盖：一秒租约、无 watcher 且上下文已取消、空 mock 事件、容量 8 后的背压与取消、不同类型隔离、真实 `Put` 转发/`Delete` 忽略、无效 UTF-8 和 receiver 丢弃后的线程退出。若 Rust TTL worker 将来完成接线，还应在其独立集成测试中复刻 Go `TestTriggerScanTask` 的“提交作业触发重新调度”场景。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ttl/client` 收录目标源、Go 对照和独立 Rust/Go 测试；`node --file pkg/ttl/client/notification.rs` 核对了完整 208 行及文件级使用关系；`query Notification`、`query new_notification_client`、`query watch_notification` 核对了主要符号。精确 callers/callees 未产出边，故没有据此推断无调用。
- 目标实现：[`notification.rs`](notification.rs) 的 `NotificationReceiver`、`NotificationClient`、两套构造函数及 `EtcdClient`/`MockClient` impl。
- crate 和直接依赖：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`command.rs`](command.rs) 中的 `ClientError`、`ClientContext`、`EtcdStore::put`/`watch`、`MockState` 和 `MockNotificationWatcher`。
- Go 语义与应用位置：[`notification.go`](notification.go)、[`../ttlworker/job_manager.go`](../ttlworker/job_manager.go) 的订阅、重订阅和作业创建后通知路径，以及 [`../ttlworker/job_manager_integration_test.go`](../ttlworker/job_manager_integration_test.go) 的 `TestTriggerScanTask`。
- Rust 独立测试：[`command_test.rs`](command_test.rs) 的 `etcd_notification_expires_after_go_one_second_lease` 与 `mock_notification_keeps_go_empty_event_and_no_watcher_cancel_semantics`。本任务是纯文档分析，按计划没有运行 Cargo。

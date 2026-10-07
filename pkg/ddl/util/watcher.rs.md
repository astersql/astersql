# `pkg/ddl/util/watcher.rs`

## 文件定位

本文件属于 `astersql-ddl-util` crate（`pkg/ddl/util/Cargo.toml`），由 `pkg/ddl/util/lib.rs` 通过 `#[path = "watcher.rs"] mod watcher_impl` 编入，并由 `pub use watcher_impl::*` 对外导出。它提供一个面向 crate 内存版 etcd 抽象的、线程安全的精确路径监听器。

当前接线状态需要特别区分：该 API 已进入 crate 的公开表面，但仓库 Rust 源码中没有发现 `astersql_ddl_util::NewWatcher`、本文件 `Watcher` trait 或其附加观测方法的调用点。DDL schema-version 主链在 `pkg/ddl/schemaver/syncer.rs` 中定义并使用另一套本地 `Watcher`，其 `Context`、`EtcdClient` 和 `WatchChan` 类型也不同。因此，本文件目前是可复用的工具实现，不是已验证接入 schema 同步主链的实现。

## 核心职责

- `Watcher` trait（`watcher.rs:28`）定义取得当前事件通道、同步建立监听、异步重建监听，以及读取最近错误和重建耗时的接口。
- `watcher`（`watcher.rs:66`）用 `Arc<RwLock<WatcherState>>` 保存共享状态，使 trait 对象本身可克隆、跨线程使用。
- `Watch`（`watcher.rs:85`）在调用线程中检查取消状态并注册精确路径 watch，然后原子地发布成功通道或记录错误。
- `Rewatch`（`watcher.rs:104`）先清除当前通道和错误，再启动独立后台线程注册新 watch，并记录注册耗时与结果。
- 本文件只管理“当前通道引用”和诊断状态；事件生成、精确键匹配、失败注入与发送端清理由 `pkg/ddl/util/util.rs` 的 `EtcdClient`/`EtcdState` 实现。

## 主要符号

- `pub trait Watcher: Send + Sync`：公开动态分派边界。`WatchChan` 返回 `Option<WatchChannel>`；`Watch` 借用取消令牌和客户端；`Rewatch` 接管二者的所有权，以便移动到 `'static` 后台线程；`LastError`、`LastRewatchDuration` 暴露诊断快照。
- `struct WatcherState`：内部状态聚合，字段为 `channel`、`last_error`、`last_rewatch_duration`。其 `Default` 将三者初始化为 `None`。
- `pub struct watcher`：公开但采用 Go 风格小写名的具体实现；`Clone` 只克隆内部 `Arc`，不同实例仍观察和修改同一状态。
- `pub fn NewWatcher() -> Arc<dyn Watcher>`：公开构造入口，返回默认 `watcher` 的 trait object，隐藏具体状态布局。
- `impl Watcher for watcher`：实现全部状态读取与 watch/rewatch 流程。锁中毒时均使用 `PoisonError::into_inner` 继续访问，而不是 panic。

文件没有模块级常量、枚举、条件编译项或内嵌测试模块。

## 执行流程

同步 `Watch` 的流程如下：

1. 调用 `CancellationToken::check`；若调用前已取消，直接得到 `DdlUtilError::Cancelled`。
2. 未取消时调用 `EtcdClient::watch(path)`。该实现向 `EtcdState.watchers` 注册精确路径的 `mpsc::Sender`，并返回包装共享接收端的 `WatchChannel`（`pkg/ddl/util/util.rs:329`）。
3. 取得写锁。成功时把新通道放入 `channel` 并清除 `last_error`；失败时清空 `channel` 并保存错误。

异步 `Rewatch` 的流程如下：

1. 在调用线程取得写锁，将 `channel` 和 `last_error` 清空；`last_rewatch_duration` 保留上一次完成值。
2. 克隆状态 `Arc`，把取消令牌、客户端和路径移动进 `thread::spawn` 创建的脱离式线程。
3. 后台线程启动计时，执行与 `Watch` 相同的“取消检查后注册”组合。
4. 取得写锁，先写入本次耗时，再发布新通道并清除错误，或保持无通道并记录错误。

`WatchChan`、`LastError`、`LastRewatchDuration` 都取得读锁并返回克隆或复制出的瞬时快照；调用者不会持有内部锁。

## 数据与状态

`WatcherState` 的三个字段有如下含义和不变量：

- `channel: Option<WatchChannel>`：`Some` 表示最近一次已完成的注册成功；默认、重建窗口和最近一次注册失败时为 `None`。
- `last_error: Option<DdlUtilError>`：记录最近一次已完成 watch/rewatch 的错误。成功会清除它；`Rewatch` 一开始也会清除旧错误，因此后台线程完成前无法区分“正在重建”与“尚未使用”。
- `last_rewatch_duration: Option<Duration>`：只由后台重建线程更新；普通 `Watch` 不修改它，发起新 `Rewatch` 也不会先清零，所以重建期间可能仍读到上一次耗时。

`WatchChannel` 自身可克隆，多个克隆共享一个 `Mutex<mpsc::Receiver<WatchEvent>>`（`pkg/ddl/util/util.rs:158`），因此它们竞争消费同一事件流，而不是广播订阅。清除 `WatcherState.channel` 只释放状态持有的克隆；调用者已取得的克隆仍可能保持旧接收端存活。

## 依赖与调用关系

上游装配边是 `pkg/ddl/util/lib.rs` → `watcher.rs`，并通过 crate 根重导出 `NewWatcher`、`Watcher` 和 `watcher`。`pkg/ddl/util/Cargo.toml` 未声明外部依赖，本文件只使用标准库和同 crate 类型。

主要下游边为：

- `NewWatcher` → `watcher::default` → `WatcherState::default`。
- `Watch`/`Rewatch` → `CancellationToken::check` → `EtcdClient::watch`。
- `EtcdClient::watch` → `mpsc::channel`，并把发送端登记到 `EtcdState.watchers`。
- `WatchChan` → `WatchChannel::clone`；事件消费由调用者后续调用 `recv`/`try_recv` 完成。
- 全部状态访问 → `RwLock::read`/`write`；`Rewatch` 额外依赖 `thread::spawn` 和 `Instant::elapsed`。

RustCodeGraph 将文件识别为 16 个符号，并确认 `NewWatcher` 调用本文件的 `default`；图查询没有给出本文件 API 的外部调用边。全仓 `rg` 同样只找到定义与 `lib.rs` 装配。`pkg/ddl/schemaver/syncer.rs` 的同名方法属于另一个 `Watcher`，不是本文件的调用关系。

## 错误处理与边界

- 接口不返回 `Result`；取消和 etcd 注册失败通过 `channel = None` 与 `LastError` 旁路报告。调用者必须同时检查通道和错误，不能把 `None` 单独解释成失败。
- `CancellationToken::check` 只在注册前检查一次。注册成功后取消令牌不会自动注销已登记的发送端，也不会关闭接收端。
- `EtcdClient::watch` 当前只支持精确路径，不支持前缀和起始 revision；此限制来自 `pkg/ddl/util/util.rs:329`。
- `Rewatch` 返回时只保证旧的内部通道引用已清除且线程已尝试启动，不保证新监听已经注册完成；接口没有 join handle、完成通知或超时控制。
- 连续或并发调用多个 `Rewatch` 没有 generation/epoch 防护。较早启动的线程可能较晚完成并覆盖较新调用的结果与诊断字段；源码未建立“最后一次调用必定获胜”的不变量。
- 锁中毒会保留并继续使用内部数据。这增强了可用性，但中毒前写操作是否完成需由上层语义自行承担，本文件不会重置状态。

## 并发与资源生命周期

共享状态生命周期由 `Arc` 管理：构造器返回的 trait object、`watcher::clone` 以及每个重建线程都可延长状态存活期。读操作可并发，写操作串行；网络抽象调用在加写锁之前完成，避免注册期间阻塞状态读取。

每次 `Rewatch` 创建一个未保存句柄的 OS 线程。线程捕获 `CancellationToken`、`EtcdClient`、路径和状态 `Arc`，完成一次注册和状态写回后退出。不存在显式 shutdown、join 或线程数量限制，频繁重建可能产生短时线程堆积。

`EtcdClient` 是进程内共享状态的 `Arc<Mutex<EtcdState>>`。watch 发送端保留在客户端状态中；当所有接收端被丢弃后，发送端通常要等到匹配路径的下一次通知发送失败时才会由 `retain` 移除（`pkg/ddl/util/util.rs::notify`）。因此本文件清空通道不等于立即从客户端注销 watcher。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/util/watcher.go`。两版共同保留了 `Watcher`/`watcher`/`NewWatcher`、读写锁保护当前通道、同步 `Watch`，以及“`Rewatch` 先置空旧通道、再异步建立新监听”的总体顺序。

Rust 版存在明确的移植扩展：

- Go 的 `clientv3.Client.Watch` 直接返回通道；Rust 内存客户端返回 `Result<WatchChannel, DdlUtilError>`，所以 Rust trait 增加 `LastError`，并以 `Option` 表示通道缺失。
- Rust `Rewatch` 为满足线程所有权要求按值接收可克隆的令牌、客户端和 `String`；Go goroutine 捕获借用语义下的 `context.Context`、客户端指针和字符串。
- Go 在调用 `Rewatch` 时开始计时，并把耗时写入 `DeploySyncerHistogram`，完成后记录日志；Rust 在线程体开始计时，只保存在 `last_rewatch_duration`，没有指标或日志。因此两者的耗时窗口和可观测性不等价。
- Go context 可持续控制 etcd watch 生命周期；Rust 取消令牌只在注册前采样一次，不能取消已注册监听。
- Go 文件没有错误观测字段；Rust 额外明确了注册失败状态，但当前没有外部调用或测试证明上层消费方式。

相关 Go 测试中没有发现直接针对 `pkg/ddl/util/watcher.go` 的独立单元测试。`pkg/ddl/serverstate/syncer_test.go` 和 `pkg/ddl/jobsubmit/submit_test.go` 出现的 `WatchChan`/`Rewatch` 验证的是 server-state 接口或 fake，不直接覆盖本实现。

## 扩展指南

- 若要把本实现接入 schema-version 或 server-state 主链，先统一两边的 context/client/channel 类型；不要仅凭同名方法替换 `pkg/ddl/schemaver/syncer.rs::Watcher`。接线后应在调用者 crate 的独立测试文件验证初始化、断线重建和取消生命周期。
- 若要保证并发重建的“最后调用获胜”，应在 `WatcherState` 增加递增 generation，并让后台线程提交结果前核对 generation；同时增加独立 `watcher_test.rs`，用可控延迟客户端复现乱序完成。
- 若要支持真正注销，需先在 `EtcdClient`/`WatchChannel` 层设计注册 ID 或取消句柄，再由 `Watch`/`Rewatch` 在替换通道时释放；仅把 `Option` 设为 `None` 不足以保证注销。
- 若增加前缀、revision 或过滤选项，应修改 `Watcher` trait 与 `EtcdClient::watch` 的契约，并测试历史丢失、重复/漏事件和精确键兼容性。
- 若保留每次重建一个线程的模型，应评估调用频率、线程创建成本和进程退出行为；高频场景更适合复用执行器或返回可等待句柄。
- 测试必须放在独立文件而非 `watcher.rs` 内。建议新增 `pkg/ddl/util/watcher_test.rs`，并在 `lib.rs` 以 `#[cfg(test)] #[path = "watcher_test.rs"] mod watcher_test;` 接入；覆盖成功监听、预取消、注入 watch 失败、重建窗口、耗时更新、旧通道克隆以及并发重建顺序。

## 验证依据

- 源码与装配：`pkg/ddl/util/watcher.rs`、`pkg/ddl/util/lib.rs`、`pkg/ddl/util/Cargo.toml`。
- 下游类型和行为：`pkg/ddl/util/util.rs` 中的 `DdlUtilError`、`CancellationToken`、`WatchChannel`、`EtcdClient::watch` 与 `EtcdClient::notify`。
- Go 对照：`pkg/ddl/util/watcher.go`。
- 相邻实际主链与测试边界：`pkg/ddl/schemaver/syncer.rs`、`pkg/ddl/schemaver/syncer_test.rs`、`pkg/ddl/serverstate/syncer_test.rs`、`pkg/ddl/serverstate/syncer_test.go`、`pkg/ddl/jobsubmit/submit_test.go`、`pkg/ddl/util/util_test.rs`。这些路径未提供本文件的直接 Rust 测试。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ddl/util/watcher.rs` 识别本文件及 16 个符号；`node --file ...` 核对完整 151 行；`query NewWatcher` 区分 Go/Rust 及其他同名实现；`node pkg/ddl/util/watcher.rs::NewWatcher` 确认其到 `default` 的调用边；`node pkg/ddl/util/watcher.rs::Watcher` 核对 trait 与具体 struct。`explore` 和外部 `callers/callees` 未返回可用调用边，故调用状态由模块装配与全仓 `rg` 交叉验证。
- 人工复核结论：该文件存在是为 `astersql-ddl-util` 提供 Go 对齐的可共享 watch 抽象；其运行核心是同步发布或异步清空后重建通道；安全扩展的首要风险是当前未接线、取消不持续、旧通道不等于注销，以及并发重建结果可能乱序覆盖。

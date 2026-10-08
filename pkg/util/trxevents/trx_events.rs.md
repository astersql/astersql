# `pkg/util/trxevents/trx_events.rs`

## 文件定位

本文件是 `astersql-util-trxevents` crate 的业务实现文件，定义事务运行期事件的类型标签、动态载荷信封和回调类型。crate 入口 `pkg/util/trxevents/lib.rs` 通过 `pub mod trx_events` 声明模块，并以 `pub use trx_events::*` 对外重导出本文件 API；根 facade `pkg/lib.rs` 的 `util::trxevents` 又重导出该 crate。它是 Go 文件 `pkg/util/trxevents/trx_events.go` 的 Rust 迁移版，当前只定义 `CopMeetLock` 一种事件。

## 核心职责

- 用 `EventType` 和 `EventTypeCopMeetLock` 为事务事件分类，保持 Go `int`/`iota` 的现有编号语义。
- 用 `TransactionEvent` 将私有类型标签与 `Any` 动态载荷捆绑，避免调用方直接构造标签与载荷不匹配的信封。
- 用 `WrapCopMeetLock` 创建遇锁事件，用 `TransactionEvent::GetCopMeetLock` 按标签取回载荷。
- 用 `EventCallback` 表示消费事务事件的可捕获环境闭包。本文件只定义事件协议，不负责产生锁事件、调度回调或记录日志。

## 主要符号

- `pub type EventType = isize`：事件标签别名。`isize` 是对 Go 平台宽度 `int` 的机械对应，不是稳定的跨进程序列化格式。
- `pub const EventTypeCopMeetLock: EventType = 0`：唯一已定义事件编号，对应 Go `iota` 首值。
- `pub struct CopMeetLock`：遇锁载荷；公开字段 `LockInfo: Option<Box<ProtoLockInfo>>` 保留 Go `*kvrpcpb.LockInfo` 可为 `nil` 的语义。`ProtoLockInfo` 是 `tikv_client_proto::kvrpcpb::LockInfo` 的本地别名。
- `pub struct TransactionEvent`：事件信封；`inner: Option<Box<dyn Any>>` 保存动态载荷，`eventType: EventType` 保存标签，两个字段均为私有。
- `TransactionEvent::GetCopMeetLock(&self) -> Option<&CopMeetLock>`：非拥有式查看遇锁载荷，返回引用的生命期受 `TransactionEvent` 借用限制。
- `WrapCopMeetLock(Option<Box<CopMeetLock>>) -> TransactionEvent`：唯一公开构造路径，设置标签并把具体载荷擦除为 `dyn Any`。
- `pub type EventCallback = Box<dyn Fn(TransactionEvent)>`：按值消费信封的闭包别名；类型本身没有 Go 函数值的 `nil` 状态，如需可选回调应在上层使用 `Option<EventCallback>`。

## 执行流程

1. 产生方将可选的 `Box<CopMeetLock>` 传给 `WrapCopMeetLock`。
2. `WrapCopMeetLock` 固定写入 `EventTypeCopMeetLock`，并在载荷存在时将 `Box<CopMeetLock>` 上转为 `Box<dyn Any>`；输入 `None` 则保留为空载荷。
3. 事件按值传入 `EventCallback`，信封的所有权随调用转移给回调。
4. 消费方调用 `GetCopMeetLock`。方法先比较 `eventType`；标签不匹配时直接返回 `None`。
5. 标签匹配时，方法仅在 `inner` 存在且 `downcast_ref::<CopMeetLock>()` 成功时返回 `Some(&CopMeetLock)`；空载荷或实际类型不匹配均返回 `None`。

Go 完整主链的直接证据为：`pkg/distsql/distsql.go:75-89` 创建回调并放入 `kv.ClientSendOption.EventCb`，`pkg/store/copr/coprocessor.go:2674-2687` 在 coprocessor 遇锁时包装并投递事件。Rust 主链尚未等价接通，不应将这条 Go 调用链视为已经执行的 Rust 路径。

## 数据与状态

`TransactionEvent` 的不变量是“标签与动态载荷类型一致”。该不变量由私有字段和公开包装函数共同维护：crate 外部不能直接改写 `eventType` 或 `inner`。当前有效状态包括“标签 0 + `CopMeetLock` 载荷”和“标签 0 + 空载荷”；后者用于表达 Go 传入 typed nil 的情形。

`CopMeetLock::LockInfo` 还有独立的可空层，因此“有 `CopMeetLock` 事件但无 `LockInfo`”与“整个 `CopMeetLock` 载荷为空”是两种不同状态。本文件没有全局可变状态、序列化逻辑或事件队列。

## 依赖与调用关系

- 标准库 `std::any::Any` 提供运行时类型擦除与下转。
- `tikv_client_proto::kvrpcpb::LockInfo` 是唯一外部业务类型。`pkg/util/trxevents/Cargo.toml` 将其声明为来自 `astersql/kvproto` 标签 `v0.0.2-aster.20260929` 的 `kvproto` 依赖，关闭默认 feature 并启用 `protobuf-codec`。
- 工作区根 `Cargo.toml` 收录该 crate，并以 `facade_util_trxevents` 别名供 `pkg/lib.rs` 重导出。`pkg/distsql/Cargo.toml` 直接依赖它；`pkg/store/copr/Cargo.toml` 将它声明为可选依赖。
- RustCodeGraph 对 `WrapCopMeetLock`/`GetCopMeetLock` 未返回 crate 外 Rust 调用边；`rg` 也只在独立迁移测试中找到真实 API 调用。特别地，`pkg/kv/lib.rs:356-358` 仍定义独立的 `trxevents::EventCallback = fn()` 桩，`pkg/kv/kv.rs:305-313` 使用的是该桩，而不是本文件的 `EventCallback`。因此当前 Rust 状态是“API 已迁移并可导出，完整 distsql/KV/coprocessor 运行时链路未接通”。

## 错误处理与边界

本文件没有 `Result`、显式错误类型或 I/O。`GetCopMeetLock` 将非目标标签、空载荷和下转失败统一收敛为 `None`，不会像 Go 在“标签匹配但 `inner` 非 `*CopMeetLock`”时的单值类型断言那样 panic。由于信封字段私有，正常 crate 外调用无法制造这种错配；若未来增加内部构造路径，必须继续维护这一约束。

`WrapCopMeetLock(None)` 和 `CopMeetLock { LockInfo: None }` 都是允许的边界值，前者使 `GetCopMeetLock` 返回 `None`，后者返回存在的事件但其锁信息为空。调用方不应把这两种状态混同。

## 并发与资源生命周期

本文件不创建线程、任务、通道或锁。`TransactionEvent` 和其 `Box` 载荷在普通 Rust 所有权规则下生存：包装时将载荷所有权移入信封，传入 `EventCallback` 时再移入闭包调用，闭包返回后若未另行保存数据则自动释放。`GetCopMeetLock` 只借用载荷，不延长它的生命期。

`Box<dyn Fn(TransactionEvent)>` 没有 `Send` 或 `Sync` 约束，`Box<dyn Any>` 也没有 `Send` 约束，因此这些 API 不承诺可跨线程移动或共享。Go `pkg/distsql/distsql.go:76` 特别提醒回调可能不在同一 goroutine 执行；将 Rust 路径接入多线程调度前，必须先根据实际调用链评估是否要增加 `Send + Sync + 'static` 边界，不能仅凭当前类型别名假定线程安全。

## 与 Go 版本的对应关系

`pkg/util/trxevents/trx_events.go` 是逐项对照源：Go `EventType = int` 对应 Rust `isize`，`iota` 首值对应常量 `0`，`*kvrpcpb.LockInfo` 对应 `Option<Box<ProtoLockInfo>>`，`any` 对应 `Option<Box<dyn Any>>`，`func(TransactionEvent)` 对应 `Box<dyn Fn(TransactionEvent)>`。公开符号保留 Go 命名，并由 `pkg/util/trxevents/lib.rs` 的 lint 允许项容纳非 Rust 风格的大小写。

有三处语义差异需显式保留在评审视野内：

1. Go 的 `e.inner.(*CopMeetLock)` 在标签与实际非空类型不匹配时 panic，Rust `downcast_ref` 返回 `None`。
2. Go 函数值可为 `nil`，Rust `EventCallback` 本身不可空；可空性必须由上层 `Option` 表达。
3. Go 回调可在其他 goroutine 执行，而当前 Rust trait object 没有 `Send`/`Sync` 保证。

`pkg/util/trxevents/migration_aster_unit_test.rs` 是 Rust 的独立测试文件，覆盖常量编号、`LockInfo` 字段保留、typed nil 语义和回调执行。同目录没有 Go 专用测试；Go 上游使用与传递证据位于 `pkg/distsql/distsql.go`、`pkg/kv/kv.go` 和 `pkg/store/copr/coprocessor.go`。

## 扩展指南

新增事件种类时，应在本文件成组增加“唯一稳定标签 + 具体载荷结构 + 包装函数 + 取出方法”，不要对已发布标签重排编号。构造路径应继续集中在本模块，以保证 `eventType` 与 `Any` 载荷一致；若改为 enum，需同时评估 Go 公开 API 对齐和现有调用方迁移成本。

相关测试应放在独立的 `pkg/util/trxevents/migration_aster_unit_test.rs`，不要内嵌进本生产文件。至少需覆盖新标签值、正确载荷往返、非匹配 getter、空载荷和回调传递。若把该 crate 接入 Rust distsql/KV/coprocessor 主链，还应替换 `pkg/kv/lib.rs` 的同名桩并在相应独立测试中验证遇锁时的生产、转发、消费顺序及跨线程边界。主要兼容风险是标签号变动、typed nil 差异和 Go/Rust 下转失败行为不同；性能风险是每个事件的堆分配与动态分发。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/trxevents` 确认该目录的 Rust/Go 源和独立测试。
- RustCodeGraph `node --file pkg/util/trxevents/trx_events.rs` 核对了全部 94 行实现；`query` 确认 Rust/Go 的 `WrapCopMeetLock`、`GetCopMeetLock`、`TransactionEvent` 和 `CopMeetLock` 对应符号。`callers`/`callees` 未为两个 Rust 方法返回调用边，因此又用 `rg` 核查未覆盖的引用。
- 已读源码：`pkg/util/trxevents/trx_events.rs`、`pkg/util/trxevents/lib.rs`、`pkg/util/trxevents/trx_events.go`、`pkg/util/trxevents/migration_aster_unit_test.rs`、`pkg/distsql/distsql.go:60-94`、`pkg/store/copr/coprocessor.go:2665-2699`、`pkg/kv/kv.go:350-370`、`pkg/kv/kv.rs:295-314`、`pkg/kv/lib.rs:345-358` 和 `pkg/lib.rs:2160-2174`。
- 已读配置：`pkg/util/trxevents/Cargo.toml`、根 `Cargo.toml`、`pkg/distsql/Cargo.toml` 与 `pkg/store/copr/Cargo.toml`，用于核对 crate 成员、facade 别名、直接/可选依赖和 kvproto tag。
- 行为证据来自独立 Rust 测试中的四项用例：`cop_meet_lock_event_type_matches_go_iota`、`wrap_cop_meet_lock_preserves_the_lock_payload`、`wrap_nil_cop_meet_lock_returns_nil_like_go_typed_nil` 和 `event_callback_receives_the_wrapped_event`。按任务约束未运行 Cargo；本文档不声称已通过代码编译或测试。

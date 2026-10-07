# `pkg/ddl/serverstate/mem_syncer.rs`

## 文件定位

[`mem_syncer.rs`](./mem_syncer.rs) 属于 `astersql-ddl-serverstate` crate。crate 入口 [`lib.rs`](./lib.rs) 公开 `mem_syncer` 与 `syncer` 两个模块并重新导出其公开项；[`Cargo.toml`](./Cargo.toml) 将 Go 对照包标为 `pkg/ddl/serverstate`，唯一常规依赖是 `astersql-ddl-schemaver`（由同 crate 的 `syncer.rs` 用于同步上下文和 etcd transport）。

该文件提供 `Syncer` trait 的进程内实现，定位是单进程测试/本地替身，不是 schema 版本同步器 `pkg/ddl/schemaver/mem_syncer.rs`。当前 Rust 生产接线在 `pkg/session/runtime/session_factory.rs:371-378` 构造 `EtcdSyncer::with_client`；仓库内直接构造 `new_mem_syncer` 的 Rust 调用点只有独立测试 `syncer_test.rs:107-123`。因此不能把本实现描述为已经接入 Rust 正常 session 主链。

## 核心职责

- `MemSyncer` 在进程内保存全局 server state，并以容量为 1 的 `std::sync::mpsc::sync_channel` 模拟状态变更 watch。
- 它实现 `syncer.rs:512-530` 定义的全部 `Syncer` 方法，使依赖方可以通过 `Arc<dyn Syncer>` 在内存实现和 `EtcdSyncer` 之间替换。
- `set_mock_upgrading_state` 模拟 Go failpoint `mockUpgradingState`：开启后更新共享状态但不投递 watch 事件。
- 状态用途不是执行 DDL 状态机本身，而是给上层升级准入逻辑提供“是否 upgrading”的判断；例如 `pkg/ddl/normal_policy.rs:31-70` 通过 `Syncer::is_upgrading_state` 决定暂停、放行或恢复 DDL job。

## 主要符号

- `CLUSTER_STATE: OnceLock<RwLock<StateInfo>>`：进程级、惰性初始化的唯一状态槽；不属于某个 `MemSyncer` 实例。
- `MOCK_UPGRADING_STATE: AtomicBool`：进程级测试开关，使用 Release 写和 Acquire 读。
- `cluster_state() -> &'static RwLock<StateInfo>`：首次访问时以 `StateInfo::new(STATE_NORMAL_RUNNING)` 初始化全局状态。
- `set_mock_upgrading_state(enabled: bool)`：公开测试控制入口；调用方有责任在测试后恢复开关，避免影响同进程其他测试。
- `MemSyncer`：包含 `global_sender`、`global_watcher` 和预留的 `mock_session`。字段均通过锁提供内部可变性，因此 trait 方法只需 `&self`。
- `MemSyncer::default()`：创建尚未初始化的实例；此时 sender/session 为空，watcher 内是已断开的默认 receiver。
- `new_mem_syncer() -> Arc<dyn Syncer>`：公开工厂，立即擦除具体类型，鼓励按 `Syncer` 接口使用。
- `impl Syncer for MemSyncer`：实现 `init`、`update_global_state`、`get_global_state`、`is_upgrading_state`、`watch_chan`、`rewatch`。

## 执行流程

1. `new_mem_syncer` 创建默认实例，但不隐式调用 `init`。
2. `init` 分别创建容量为 1 的状态事件通道和占位 session 通道，把 sender/session 保存进互斥锁，并用 `Watcher::replace` 安装状态 receiver；随后触发 `cluster_state` 的惰性初始化。
3. 普通 `update_global_state` 先检查全局 mock 开关；关闭时要求 sender 已初始化，向 watch 通道发送一个默认 `WatchResponse`，发送成功后才取得全局写锁并替换 `StateInfo`。
4. 消费者通过 `watch_chan` 取得共享 receiver 的句柄，再用 `WatchChannel::recv` 或 `recv_timeout` 等待事件；事件本身不携带状态值，消费者需要调用 `get_global_state` 读取共享快照。
5. `is_upgrading_state` 直接比较快照中的 `state` 与 `STATE_UPGRADING`；`rewatch` 对内存后端无工作可做，保持空操作。

mock 分支在第 3 步直接写入状态并返回，不要求 `init`，也不会产生 watch 事件。独立 Rust 测试 `syncer_test.rs:107-123` 覆盖了正常路径的初始化、升级态写入、事件到达、状态读取和升级态判断。

## 数据与状态

`StateInfo`、`STATE_NORMAL_RUNNING`（空串）和 `STATE_UPGRADING`（`"upgrading"`）来自 `syncer.rs:37-40,139-152`。`get_global_state` 返回 clone，因此调用方不能越过同步器直接修改共享值。

`CLUSTER_STATE` 是静态单例：新建、丢弃或重新初始化 `MemSyncer` 都不会把它恢复为正常态；不同实例会观察和覆盖同一状态。这与 Go 注释“状态不受 DDL close 影响”的意图一致，也意味着测试必须显式恢复状态，不能假定每个实例隔离。

`global_sender` 与 `mock_session` 是实例状态。`init` 再次调用会替换它们以及 watcher 的 receiver，并丢弃旧通道中未消费的事件。`mock_session` 当前没有后续读写，只通过同时保存 sender/receiver 保持占位通道存活。

## 依赖与调用关系

直接依赖全部来自同 crate 的 `syncer` 模块：`Syncer` 定义接口，`SyncContext`/`SyncError` 定义调用契约，`StateInfo` 与状态常量定义载荷，`Watcher`/`WatchChannel`/`WatchResponse` 封装通道。标准库提供 `Arc`、`Mutex`、`RwLock`、`OnceLock`、原子变量和同步 mpsc。

RustCodeGraph 对目标文件报告 12 个符号，并显示文件被 `pkg/ddl/serverstate/syncer_test.rs` 与 `pkg/session/runtime/normal_ddl_test.rs` 的依赖图触达；精确 callers/callees 查询没有产出 trait 动态分派边。源码级 `rg` 进一步确认：`new_mem_syncer` 的唯一 Rust 构造调用在 `syncer_test.rs:109`，`normal_ddl_test.rs` 使用的是 `EtcdSyncer`。上层对抽象的实际消费包括 `pkg/ddl/normal_policy.rs:24-38` 和 `pkg/session/runtime/normal_ddl_service.rs:261-288`。

Go 主链在 `pkg/ddl/ddl.go:747-764` 有额外接线：`newDDL` 遇到 nil etcd client（注释说明为测试用 localstore）时构造 `serverstate.NewMemSyncer`，否则构造 etcd 实现。Rust 当前没有对应的内存后端工厂分支。

## 错误处理与边界

- 未 `init` 的普通更新返回 `SyncError::NotInitialized`；receiver 被关闭时发送失败映射为 `SyncError::WatchClosed`。
- `get_global_state` 和 `is_upgrading_state` 会惰性创建默认状态，因此它们在未 `init` 时仍可成功；mock 更新也可在未初始化时成功。这是需要测试和调用方明确理解的不对称边界。
- `SyncContext` 参数在所有方法中均未使用；内存实现没有取消、deadline、重试、后端错误或 JSON 编解码路径。
- 所有锁操作都使用 `unwrap`。若持锁线程 panic 导致锁中毒，后续调用也会 panic，而不是返回 `SyncError`。
- 状态通道容量只有 1，且 `send` 为阻塞调用：消费者长期不读取时，第二次普通更新会阻塞。通知先于状态写入，因此收到事件与随后读到新状态之间不存在由该实现显式建立的原子快照；调用方不应把空 `WatchResponse` 当作状态载荷。
- `rewatch` 是有意的空操作；不要期待它重建通道。需要重建时只能重新 `init`，并接受旧队列被替换的语义。

## 并发与资源生命周期

`MemSyncer` 满足 `Syncer: Send + Sync`：sender/session 用 `Mutex` 保护，全局状态用 `RwLock` 保护，mock 开关用原子变量保护。`watch_chan` 返回的句柄共享 `Watcher` 内部同一个 `Arc<Mutex<Receiver<_>>>`，多个消费者不是广播订阅，而是竞争消费同一事件流；同时只有一个线程能阻塞在 receiver 锁内。

文件不创建线程，也没有显式 `Drop`。实例销毁时其 sender/session 随字段销毁；但 `CLUSTER_STATE` 和 mock 开关持续到进程结束。重新 `init` 时 `Watcher::replace` 修改共享 receiver，因此此前取得的 `WatchChannel` 句柄也会看到新 receiver，而不是永久绑定旧订阅。

原子开关的 Acquire/Release 只同步开关本身；状态一致性由 `RwLock` 独立保证。全局开关与全局状态使并行测试可能互相干扰，新增测试应串行化相关切换或用作用域 guard 确保清理。

## 与 Go 版本的对应关系

直接对照文件是 [`mem_syncer.go`](./mem_syncer.go)。两版都使用跨实例全局状态、容量 1 的 watch 通道、独立构造与 `Init`、通知后更新状态、升级态字符串比较，以及空操作 `Rewatch`。Rust 的 `set_mock_upgrading_state` 对应 Go failpoint `mockUpgradingState` 的“只写状态、不通知”分支。

已确认的差异如下：

- Go 用 `atomicutil.Pointer[StateInfo]` 保存指针，Rust 用 `RwLock<StateInfo>` 保存值并在读取时 clone。
- Go 的全局指针在 `Init` 中首次创建；初始化前读取会解引用 nil，初始化前向 nil channel 发送会阻塞。Rust 通过 `OnceLock` 惰性提供默认值，并把未初始化普通更新转换为 `NotInitialized`。
- Go 的 failpoint 是测试框架注入且局部作用于调用；Rust 是进程级公开 `AtomicBool`，清理责任更显式、并行污染风险也更高。
- Go `UpdateGlobalState` 接收 `*StateInfo`，Rust 接收所有权值；Rust 因而不会保存调用者可继续修改的同一对象。
- Go `newDDL` 的 localstore 分支已接线内存 server-state syncer；Rust 当前直接生产接线只找到 `EtcdSyncer`，内存实现用于 `syncer_test.rs`。

Go 的 `syncer_test.go:42-123` 主要验证 etcd 实现的 watch 与缓存刷新语义；Rust 的同目录独立测试额外用 `MemSyncer` 验证内存路径，但尚未覆盖 mock、未初始化、通道背压、重复初始化和多实例共享状态。

## 扩展指南

- 新增状态写入行为应优先修改 `impl Syncer for MemSyncer`，并与 [`syncer.rs`](./syncer.rs) 中的 trait、`EtcdSyncer` 以及 Go [`mem_syncer.go`](./mem_syncer.go) 同步核对，不能只让内存测试替身通过。
- 若 watch 事件需要携带真实 key/value，应调整 `update_global_state` 构造 `WatchResponse` 的方式，并同步 `syncer_test.rs`；同时决定事件与状态提交的顺序及一致性契约。
- 若需要广播给多个观察者，不能直接复制 `WatchChannel`；当前模型是共享单 receiver。应在 `Watcher`/trait 层设计订阅语义，并为多消费者、慢消费者与关闭路径增加独立测试。
- 若接入 Rust localstore 主链，应在 session/DDL 装配层显式选择后端，并补一条证明选型和升级准入行为的独立测试；不要在本文件中隐藏环境判断。
- 测试必须放在独立的 `syncer_test.rs`（或新的同目录 `*_test.rs`）中，不要把 `#[cfg(test)]` 测试嵌入本生产文件。最低补测建议包括：未初始化错误、mock 无事件且状态改变、重复 init、多实例共享、容量 1 背压与开关清理。
- 改动静态状态或 mock 开关时要评估测试并行性；改动通道容量/阻塞方式时要评估死锁、事件丢失和 Go 行为兼容性。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ddl/serverstate` 确认目标、Go 对照与独立测试；`node --file pkg/ddl/serverstate/mem_syncer.rs` 读取完整 120 行及 12 个符号；对 `new_mem_syncer`、`cluster_state`、`update_global_state`、`set_mock_upgrading_state` 执行 `query`，并对关键符号执行 `callers`/`callees`（动态 trait 边无输出，已用源码引用补证）。
- 已读生产/边界文件：`pkg/ddl/serverstate/mem_syncer.rs`、`syncer.rs:37-40,443-530`、`lib.rs`、`Cargo.toml`、`pkg/ddl/normal_policy.rs:24-70`、`pkg/session/runtime/session_factory.rs:350-439`、`pkg/ddl/ddl.go:747-764`、`pkg/ddl/doc.go`。
- 已读对照与测试：`pkg/ddl/serverstate/mem_syncer.go`、`syncer_test.rs:95-123`、`syncer_test.go:42-123`；全仓 Rust 引用搜索用于区分 `MemSyncer` 和实际接线的 `EtcdSyncer`。
- 人工复核结论：本文件存在的原因是为 `Syncer` 提供单进程内存替身；运行核心是“共享状态 + 单接收队列”；安全扩展必须同时处理全局状态隔离、阻塞/通知顺序、trait 两种实现与 Go 对照。

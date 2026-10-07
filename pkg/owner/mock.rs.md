# `pkg/owner/mock.rs`

## 文件定位

`pkg/owner/mock.rs` 属于 `astersql-owner` crate，由 [`pkg/owner/lib.rs`](lib.rs) 公开为 `mock` 模块并整体再导出。它实现与 etcd 版本相同的 `Manager` trait，但把选主状态放在进程内，供本地存储和不依赖真实 etcd 的测试/运行路径使用。真实 etcd 选主、`Manager` 契约、`Context`、`Listener`、`OpType` 与 `OwnerError` 定义在 [`pkg/owner/manager.rs`](manager.rs)，共享的进程内 Owner 表位于 [`pkg/owner/mock_owner_state.rs`](mock_owner_state.rs)。

crate 边界由 [`pkg/owner/Cargo.toml`](Cargo.toml) 确认：运行时依赖 `async-trait`、`tokio`、`tokio-util`、`tracing`，同时仍保留 `etcd-client` 给同 crate 的真实实现；本文件自身不访问网络或 etcd。RustCodeGraph 对 `NewMockManager` 的调用结果显示，它除 owner crate 测试外，还被 `pkg/session/runtime/session.rs::from_storage_for_test`、`pkg/session/runtime/crossks_owner.rs::new` 及若干 session/runtime 测试构造，说明它是测试/本地服务接线中的可替换 Owner 实现，而不是独立进程入口。

## 核心职责

1. `NewMockManager` 构造实现 `Manager` 的 `MockManager`，用存储 UUID 与 owner key 确定独立竞选域，并把进程级 `OpType` 重置为 `OpNone`。
2. `try_become_owner`、`retire_owner_if_current` 和 `campaign_loop` 在进程内模拟“抢占空位、持有、退位、再次参选”，并在真实状态转换成功时通知 `Listener`。
3. `CampaignOwner`、`CampaignCancel`、`ResignOwner`、`Close` 管理后台任务与取消生命周期；`ID`、`IsOwner`、`GetOwnerID` 等方法提供与真实 `Manager` 一致的调用面。
4. `mock_owner_op_value`/`SetOwnerOpValue` 模拟无 etcd client 时的 Owner 操作值。该值刻意是进程级共享状态，不按 owner path 隔离；`manager.rs::GetOwnerOpValue` 在 `client == None` 时读取它。

本文件不模拟 etcd lease、revision、watch、事务比较或网络错误；`ForceToBeOwner` 也是与 Go mock 对齐的空操作。因此它适合验证 Owner 协调契约和本地并发，不可用来证明 etcd 行为。

## 主要符号

- `MOCK_OWNER_OP_VALUE: LazyLock<StdMutex<OpType>>`：进程级操作值，初值为 `OpNone`。使用标准库互斥锁是因为读写路径本身不需要异步等待。
- `mock_owner_op_value(_owner_path: &str) -> OpType`：crate 内读取入口；参数故意不参与寻址，保持 Go `mockOwnerOpValue` 的全局语义。
- `set_mock_owner_op_value(op: OpType)`：内部写入口；构造新 manager 和 `SetOwnerOpValue` 都会调用。
- `Storage: Send + Sync`：仅暴露 `UUID() -> String` 的最小身份接口，使构造器无需依赖完整 KV storage 类型。
- `MockCampaign`：把一次后台竞选的 `CancellationToken` 与 `JoinHandle<()>` 绑定，支持取消后等待任务退出。
- `MockManagerInner`：共享 `id`、`store_id`、`key`、父上下文、监听器、当前竞选任务、退位通知、关闭标志和 tenure `epoch`。
- `MockManager`：`Arc<MockManagerInner>` 的可克隆外壳，实现 `Manager`。
- `NewMockManager(...) -> Arc<dyn Manager>`：公开工厂。`store == None` 时使用固定的 `mock_store_id`；所有无 store 且 owner key 相同的实例会进入同一竞选域。
- `selector()`：生成指向 `MockGlobalStateEntry.OwnerKey(store_id, key)` 的选择器；状态读写最终由 `mock_owner_state.rs` 的互斥映射串行化。
- `try_become_owner()`：先避免重复抢占，再递增 epoch，随后原子 `SetOwner`；只有设置成功才调用 `OnBecomeOwner`。
- `retire_owner_if_current()`：只在 `UnsetOwner(id)` 成功时调用 `OnRetireOwner`，避免非 Owner 产生虚假退位事件。
- `campaign_loop()`：每秒尝试一次抢占，同时响应单次 campaign 取消、父上下文取消和主动退位通知。

## 执行流程

构造阶段：`NewMockManager` 先把全局操作值重置为 `OpNone`，从可选 `Storage::UUID` 或默认值取得 `store_id`，保存 owner key 和取消上下文，并初始化空监听器、空 campaign、未关闭状态和 epoch 0。

启动阶段：调用 `CampaignOwner` 时先检查 `closed` 和父上下文；任一已结束都会返回 `OwnerError::Closed`。随后锁住 `campaign`：若已有未结束任务则幂等返回；否则同步执行一次 `try_become_owner`，再从父上下文创建 child token 并 `tokio::spawn` 后台循环。这个首次同步尝试让调用返回前即可建立本地 Owner；后台循环丢弃 `tokio::interval` 的立即首 tick，之后维持约一秒节奏。

竞选阶段：`try_become_owner` 若已经是 Owner 直接返回；否则先以 `AcqRel` 增加 epoch，再调用全局选择器的 `SetOwner(id)`。共享状态只在槽位为空时写入，因此同一 `(store_id, owner_key)` 只有一个实例成功；成功者记录日志并读取、克隆监听器后调用 `OnBecomeOwner`。抢占失败的实例保留递增后的本地 epoch，但 `IsOwner` 仍为 false；调用方必须联合 `IsOwner` 与 epoch 判断 tenure，不能把非零 epoch 单独视为持有权。

退位与重选：`RetireOwner` 直接执行条件卸任。`ResignOwner` 先同步卸任，再 `notify_one` 唤醒竞选循环；循环收到通知后再次做条件卸任（通常无效果），等待一秒给其他实例抢占机会，然后恢复正常 tick。独立测试 `manager_1_aster_unit_test.rs::mock_managers_compete_resign_and_notify_like_go` 验证首节点退位、监听器事件以及第二节点最终接管。

停止阶段：`CampaignCancel` 从互斥槽中取走任务，触发其 child token 并等待 join；循环在取消或父上下文结束时先卸任再退出。`Close` 通过 `closed.swap(true)` 保证只有第一次生效，依次取消父上下文、停止 campaign、再次条件卸任。`BreakCampaignLoop` 按 Go mock 的特例直接调用 `Close`，因而不是可恢复的“仅停止循环”。

## 数据与状态

Owner 归属不保存在 `MockManagerInner`，而保存在 `mock_owner_state.rs` 的进程级 `MockGlobalStateEntry.current_owner: Mutex<HashMap<OwnerKey, String>>`。`OwnerKey` 由 `store_id` 和 owner 类型/key 组成；`SetOwner`、`UnsetOwner`、`IsOwner` 在同一锁下操作，提供本进程内的原子比较与更新。不同 store 或不同 key 相互隔离，相同二元组相互竞争。

`listener` 使用异步 `RwLock<Option<Arc<dyn Listener>>>`，状态转换时先克隆 `Arc` 再调用同步回调；回调执行期间不持有 listener 锁。`campaign` 用异步 `Mutex<Option<MockCampaign>>` 保证启动、取消和替换任务互斥。`resign: Notify` 是事件通知而非状态存储；多个在消费前合并的通知不保证逐个对应循环迭代，但每次 `ResignOwner` 已同步执行卸任。

`closed: AtomicBool` 使关闭幂等并阻止后续竞选。`epoch: AtomicU64` 在每次非 Owner 的竞选尝试前递增，以 `AcqRel/Acquire` 发布和读取 tenure。它是实例局部单调计数，不是集群 revision，也不会因普通退位归零。

操作值与 Owner 归属的隔离规则不同：`MOCK_OWNER_OP_VALUE` 对整个进程只有一个槽位，忽略 owner path；新建任何 `MockManager` 都会把它重置成 `OpNone`。`mock_test.rs::mock_owner_op_value_is_process_global_like_go` 专门验证跨路径共享以及第二次构造导致重置。

## 依赖与调用关系

上游公开入口是 `lib.rs` 的 `pub use mock::*`。RustCodeGraph 的 `NewMockManager` 调用者包含 owner crate 的 `manager_test.rs`、`manager_1_aster_unit_test.rs`，以及 session runtime 的测试存储、跨 keyspace owner 和多项 DDL 服务测试。由于工厂返回 `Arc<dyn Manager>`，上游通常只依赖 `manager.rs::Manager` 契约，不需要知道内部类型。

下游关键关系如下：

- `NewMockManager → set_mock_owner_op_value`，并构造 `MockManagerInner`。
- `CampaignOwner → try_become_owner → selector → MockGlobalStateSelector::{IsOwner, SetOwner}`。
- `CampaignOwner → tokio::spawn → campaign_loop`；循环按分支调用 `retire_owner_if_current` 或 `try_become_owner`。
- `RetireOwner`、`ResignOwner`、`Close` 与取消分支都汇入 `retire_owner_if_current → MockGlobalStateSelector::UnsetOwner`。
- 成功成为/退出 Owner 后分别调用 `Listener::OnBecomeOwner`/`OnRetireOwner`。
- `manager.rs::GetOwnerOpValue(None, owner_path) → mock_owner_op_value`；`MockManager::SetOwnerOpValue → set_mock_owner_op_value`。

RustCodeGraph 的精确 `callers/callees` 子命令未为这些 async trait 方法返回记录，但同一索引的 `explore` 和文件节点明确给出了上述文件、符号和内部引用；因此这里不把索引未显示的动态 trait 调用扩写成确定的生产调用链。

## 错误处理与边界

公开的显式错误很少：关闭后或父上下文已取消时启动竞选返回 `OwnerError::Closed`；非 Owner 调用 `GetOwnerID` 返回 `OwnerError::NoLeader`。`SetOwnerOpValue`、`ResignOwner` 和 `ForceToBeOwner` 当前总是返回 `Ok(())`，没有网络失败面。

标准库互斥锁中毒会通过 `expect(...)` panic，包括全局操作值与全局 Owner 映射；这是测试替身的进程内故障策略，不转换成 `OwnerError`。`CampaignCancel` 有意忽略后台任务的 join 结果（`let _ = ...await`），所以任务 panic 不向调用者传播。监听器回调是同步 trait 方法；若回调 panic，当前异步任务也会 panic。

边界语义包括：`CampaignOwner` 的 TTL 参数未使用；`ForceToBeOwner` 不做强占；`GetOwnerID` 只允许当前实例查询自身身份，并不返回另一个实例的 ID；新建 manager 会重置进程全局操作值；`Storage::UUID` 相同且 owner key 相同的实例必然竞争。若要测试 etcd 的 lease、watch、revision 或 compare-failed，应使用 `OwnerManager` 及 `manager_test.rs` 的嵌入式 etcd 场景。

## 并发与资源生命周期

`MockManager` 的克隆共享同一个 `Arc<MockManagerInner>`。Owner 映射的互斥锁保证单进程唯一性，campaign 互斥锁保证同一实例最多维护一个未结束的后台任务，原子关闭位保证资源只清理一次。锁顺序通常是先短暂持有 campaign/listener 锁，再访问全局状态；状态转换回调前 listener guard 已释放，没有在回调期间持有该锁。

每次有效 `CampaignOwner` 创建一个 child cancellation token 和一个 Tokio task，并把两者存入 `MockCampaign`。`CampaignCancel` 是正常回收入口：取走句柄、取消、等待退出；循环负责在退出前释放 Owner。`Close` 还取消共享父 `Context`，因此关闭不可逆，之后不能重新启动。只调用 `RetireOwner` 不会停止循环，下一次 tick 仍可能重新当选；`ResignOwner` 也明确设计为让其他竞争者获得约一秒窗口后继续参选。

由于 `Notify` 不排队计数、ticker 使用 `MissedTickBehavior::Delay`，负载或调度延迟只会推迟后续尝试，不会补发密集 tick。这里保证的是单进程测试语义，不是跨进程容错或公平选举。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/owner/mock.go`](mock.go)。字段和路径大体一一对应：`id/storeID/key/context/listener`、进程级 `mockOwnerOpValue`、`MockGlobalStateEntry.OwnerKey`、约一秒竞选节奏、become/retire 回调、`ForceToBeOwner` 空操作，以及 `BreakCampaignLoop` 委托 `Close` 的已知契约偏离。

Rust 使用 `CancellationToken + JoinHandle` 代替 Go 的 `context.CancelFunc + WaitGroup + channel`，用 `Mutex<Option<MockCampaign>>` 使重复 `CampaignOwner` 幂等；Go 每次调用都会启动 goroutine。Rust `ResignOwner` 在发通知前同步卸任，而 Go 仅向 `resignDone` 发送，由循环退位。Rust 在 spawn 前立即尝试一次，再丢弃 interval 的立即 tick；Go 在 goroutine 的 `default` 分支首次尝试。两者随后都给其他管理器约一秒接管窗口。

Rust 额外维护 `OwnerEpoch`，在发布所有权前增加 tenure，用于拒绝旧任期工作；Go `mock.go` 没有对应字段。Rust `Close` 显式再次条件卸任并以原子位幂等，Go 依赖取消后的循环退位并等待 `WaitGroup`。Rust 当前没有移植 Go `SetOwnerOpValue` 中的 `MockNotSetOwnerOp` failpoint；现有 Rust 测试验证正常初始化/更新，而不是该注入分支。以上差异应被视为当前实现事实，修改时需决定是保持 Rust 加强语义还是进一步贴合 Go。

## 扩展指南

新增选主行为时，优先保持 `Manager` trait 调用面不变，并在 `MockManager` 的对应方法中实现；若行为依赖全局归属，应扩展 `mock_owner_state.rs` 的原子选择器，而不是在 `MockManagerInner` 中另建可能竞争的数据副本。改变竞选节奏、退位顺序或取消策略时，应同时检查 `campaign_loop`、`CampaignOwner`、`CampaignCancel`、`ResignOwner` 和 `Close`，确保退出路径仍会条件卸任且只产生一次真实状态转换回调。

新增状态必须先决定隔离维度：Owner 归属按 `(store_id, owner_key)` 隔离，而 `OpType` 按 Go 语义是进程全局。不要默认所有 mock 状态都按 key 隔离。涉及 tenure 的改动还需维持“先发布新 epoch，再发布 Owner”的顺序，并补测失败抢占、重新获取和旧工作拒绝场景。

测试逻辑必须继续放在独立文件，不嵌入 `mock.rs`。最接近的回归位置是 [`pkg/owner/mock_test.rs`](mock_test.rs)（全局 OpValue）、[`pkg/owner/manager_1_aster_unit_test.rs`](manager_1_aster_unit_test.rs)（竞争、退位、监听器、epoch）和 [`pkg/owner/manager_test.rs`](manager_test.rs)（`Manager` 公共契约与真实 etcd 对照）。若改动追求 Go 兼容，还应同步审阅 `pkg/owner/mock.go` 与 `pkg/owner/manager_test.go::TestGetOwnerOpValueBeforeSet`。主要风险是并发时重复回调、关闭后任务泄漏、错误的 key 隔离、旧 tenure 工作被误接受，以及为了“简化 mock”而偏离 Go 的可观察行为。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，`pkg/owner` 下的 `mock.rs`、`manager.rs`、`mock_owner_state.rs` 及相关测试均已索引；目标文件节点显示 278 行、38 个符号。
- RustCodeGraph `query/node/explore`：核对 `NewMockManager`、`MockManager`、`try_become_owner`、`retire_owner_if_current`、`campaign_loop` 及完整 `Manager` 实现；索引给出的内部流包括 `campaign_loop → try_become_owner` 和 `campaign_loop → retire_owner_if_current`，并列出 session/runtime 与 owner 测试调用者。
- [`pkg/owner/mock.rs`](mock.rs)：构造、竞选、退位、关闭、监听器、epoch 和 OpValue 的直接实现依据。
- [`pkg/owner/mock_owner_state.rs`](mock_owner_state.rs)：按 `(store_id, owner_type)` 隔离的互斥映射，以及 `SetOwner/UnsetOwner/IsOwner` 的原子条件更新依据。
- [`pkg/owner/manager.rs`](manager.rs)：`Manager`/`Listener`/`OwnerError`/`OpType` 契约及 `GetOwnerOpValue(None) → mock_owner_op_value` 依据。
- [`pkg/owner/Cargo.toml`](Cargo.toml) 与 [`pkg/owner/lib.rs`](lib.rs)：crate 依赖、模块公开边界及独立测试装配依据。
- [`pkg/owner/mock.go`](mock.go) 与 [`pkg/owner/manager_test.go`](manager_test.go)：Go 构造、竞选、退位、关闭、空操作和全局 OpValue 语义的对照依据。
- [`pkg/owner/mock_test.rs`](mock_test.rs)、[`pkg/owner/manager_1_aster_unit_test.rs`](manager_1_aster_unit_test.rs)、[`pkg/owner/manager_test.rs`](manager_test.rs)：跨路径共享值、竞争接管、监听器、epoch、先读后写等边界的独立 Rust 测试证据。本任务按计划不运行 Cargo，未重新执行这些测试。

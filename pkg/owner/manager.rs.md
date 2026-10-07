# `pkg/owner/manager.rs`

## 文件定位

`manager.rs` 是 `astersql-owner` crate 中基于 etcd 的 Owner 选举实现，同时提供 Owner key 辅助操作和一个基于 etcd lease 的分布式锁。crate 入口 `pkg/owner/lib.rs` 公开 `manager` 模块并再导出本文件的公开项；`pkg/owner/Cargo.toml` 将库入口设为 `lib.rs`，直接依赖 `etcd-client`、`tokio`、`tokio-util`、`async-trait`、`thiserror` 和 `tracing`。

Owner 是需要单实例执行的后台服务的领导权抽象。当前 Rust 生产接线包括：`pkg/session/runtime/session.rs` 为普通 DDL 构造 `NewOwnerManager`，并用 `AcquireDistributedLock` 串行化 bootstrap/升级；`pkg/session/runtime/normal_ddl_service.rs` 启动竞选、升级时强制接管，并用 `OwnerEpoch` 拒绝前一任 Owner 启动的工作；`pkg/session/runtime/ttl_runtime.rs` 为 TTL job manager 创建 Owner；`pkg/session/runtime/crossks_runtime.rs` 为 cross-keyspace DDL 创建 Owner；`pkg/autoid_service/autoid.rs` 为 AutoID 服务注册监听器并竞选。

本文件不是薄门面：它持有 etcd lease、竞选 key、后台任务和状态回调的完整生命周期。无 etcd 的本地实现位于 `pkg/owner/mock.rs`；本文件的 `GetOwnerOpValue(None, ...)` 仅把查询转接到该 mock 状态。

## 核心职责

1. `Manager` trait 定义统一的 Owner 生命周期：查询身份、发起/中止竞选、退位、关闭、读取或修改 Owner 附加操作值，以及升级期间的强制接管。
2. `OwnerManager` 用 etcd lease + election campaign 维持单 Owner：lease 保活，竞选成功后监听自己的 leader key，key 删除或 session 丢失时退位并重建会话再竞选。
3. `OwnerEpoch` 为每次本地成功任期提供单调递增编号；调用方可把“仍是 Owner”与“仍是同一任期”同时作为继续工作的条件（见 `normal_ddl_service.rs`）。
4. `get_owner_info`、`GetOwnerKeyInfo`、`SetOwnerOpValue`、`DeleteOwnerKeyByID` 和 value 编解码函数维护 election key 的身份与操作字节，并以 revision/CAS 避免并发覆盖。
5. `AcquireDistributedLock` 创建独立 lease、保持 lease 存活并返回 `DistributedLock` 句柄；显式 `release` 解锁并撤销 lease，遗弃句柄则停止保活，让锁最终按 TTL 释放。
6. `ListenersWrapper` 将成为 Owner/退位事件按输入顺序广播给多个监听器。

## 主要符号

- `Context = CancellationToken`：Rust 对 Go `context.Context` 取消语义的局部替代；`run_with_context` 让一次异步操作与取消竞争。
- `OwnerError` / `Result<T>`：区分 etcd 错误、取消、无 leader、身份不符、非 Owner 退位、已关闭、CAS 失败、watch 关闭/取消和后台任务错误。当前 `Join` 变体未在本文件构造。
- `Listener`：同步回调接口，含 `OnBecomeOwner` 与 `OnRetireOwner`；回调在异步状态切换函数内直接执行，因此实现者不应阻塞。
- `Manager`：面向 DDL、TTL、AutoID 等调用方的异步对象安全接口。`as_any` 只用于测试向下转型；`OwnerEpoch` 默认返回 `0`，真实与 mock manager 可覆盖。
- `DDLOwnerChecker`：只暴露 `IsOwner` 的窄接口；`OwnerManager` 将其转发到 `Manager::IsOwner`。
- `OpType::{OpNone, OpSyncUpgradingState}`：Owner value 的尾部操作字节。未知字节通过 `From<u8>` 回落为 `OpNone`。
- `ManagerSessionTTL` / `SetManagerSessionTTL`、`WaitTimeOnForceOwner` / `SetWaitTimeOnForceOwner`：由原子变量保存的进程级配置与测试调节点，默认分别为 60 秒和 5 秒。
- `SessionRuntime`：一份 lease 的 ID、停止令牌、丢失信号与 keep-alive 任务。
- `CampaignRuntime`：一次后台竞选循环的停止令牌与任务句柄。
- `OwnerManagerInner`：共享 `id`、election `key`、根取消令牌、日志提示、etcd client、当前 `LeaderKey`、任期编号、lease ID、session/campaign、listener 和关闭标志。
- `OwnerManager` / `NewOwnerManager`：`Arc<OwnerManagerInner>` 的可克隆句柄；构造函数返回 `Arc<dyn Manager>`，所有 clone 共享同一生命周期。
- `OwnerInfo` / `get_owner_info`：当前最早创建的 election key 及其 owner ID、操作值、集群 revision、key mod revision。
- `DistributedLock` / `AcquireDistributedLock`：持锁句柄及申请入口；`retry_lock_operation` 提供最多 10 次、按尝试次数线性增长的等待。
- `ListenersWrapper` / `NewListenersWrapper`：多监听器的顺序广播包装。

## 执行流程

正常竞选从 `CampaignOwner` 开始：先用 `check_open` 拒绝已关闭或根 context 已取消的 manager，取调用方 TTL 或全局默认值，经 `ensure_session` 创建 lease；若已有未结束的 campaign task 则幂等返回，否则创建根令牌的 child token 并 spawn `campaign_loop`。

`start_session` 总是先 `stop_session`。它最多三次调用 `lease_grant`，成功后取得 keeper/stream，按 `max(TTL/3, 1 秒)` 周期发送 keep-alive 并读取应答；发送失败、stream 结束或应答 TTL 非正时触发 `lost`。成功安装 `SessionRuntime` 后再发布 `session_lease`；连续失败则返回最后一个 etcd 错误。

`campaign_loop` 每轮取得 `(lease_id, session_lost)` 快照，再在“循环取消、session 丢失、etcd campaign 完成”三者间竞争。campaign 成功且带 `LeaderKey` 后，`become_owner` 在写锁内递增 epoch、保存 leader，再通知 listener；随后 `watch_owner` 从 `leader.rev() + 1` 监听该 key。删除事件、循环取消或 session 丢失会结束 watch，随后 `retire_if_owner` 清空 leader 并通知退位。session 丢失时重建 lease；其他失败短暂退避后重试。循环退出前再次确保退位。

停止路径分层：`BreakCampaignLoop` 取消并等待 campaign task，但保留 session；`CampaignCancel` 再停止 keep-alive 并 revoke lease；`Close` 用原子 `swap` 保证完整取消只执行一次，此后 `CampaignOwner` 和 `ForceToBeOwner` 会返回 `Closed`。`ResignOwner` 仅允许当前持有 `LeaderKey` 的节点调用，并为 etcd resign 叠加 5 秒超时和调用方取消。

升级强制接管由 `ForceToBeOwner` 启动新 session，最多执行三轮“等待后尝试”。`try_to_be_owner_once` 读取 election 前缀，在同一事务中删除非当前 lease 的候选 key并写入本候选 key，然后在 5 秒内 campaign。该方法只记录每次失败并最终返回 `Ok(())`，所以返回成功表示流程已执行完，并不单独证明已经成为 Owner；生产调用随后仍调用 `CampaignOwner`。

Owner 信息读取由 `get_owner_info` 完成：以前缀、create revision 升序、limit 1 获取当前 leader，最多三次并带单次 5 秒超时；空结果是 `NoLeader`。`SetOwnerOpValue` 先验证当前值和 owner ID，再以 key 的 `mod_revision` 做 CAS 并把新值绑定到当前 session lease；事务比较失败映射为 `CompareFailed`。

分布式锁流程是 grant lease、建立 keep-alive、重试 `client.lock`。获得锁后返回持有 server 返回锁 key 的句柄；申请最终失败会取消并等待保活任务、尝试 revoke lease。`release` 依次请求 unlock、停止并等待保活、revoke lease，然后优先传播 unlock 错误、否则传播 revoke 错误。

## 数据与状态

Owner 身份的权威远端状态是 election 前缀下按 create revision 最早的 key；本地快速状态是 `leader: RwLock<Option<LeaderKey>>`。`IsOwner` 使用 `try_read`，锁竞争时保守返回 `false` 而不阻塞。`OwnerEpoch` 也用 `try_read`：只有读取成功且 leader 存在才返回原子 epoch，否则返回 `0`；每次 `become_owner` 都递增，即使 etcd 复用相同 key/revision，也能区分本地任期。

`session_lease` 是供 `SetOwnerOpValue` 和强制接管使用的原子快照；`session: Mutex<Option<SessionRuntime>>` 才拥有 lease 保活任务。`campaign: Mutex<Option<CampaignRuntime>>` 序列化竞选任务的安装/停止，避免重复 spawn。`closed` 是不可逆关闭位；根 `Context` 取消也被 `check_open` 当作关闭。

Owner value 采用 `owner_id + "_" + 单字节 op`。`split_owner_values` 仅在下划线分隔结果恰有两段时读取第二段首字节；多余分隔段不解析 op，空或未知操作字节回落 `OpNone`。由于 owner ID 本身若含下划线会被截成第一段，调用方必须维持与 Go 格式相同的 ID 约束。

全局 TTL 和强制接管等待时间使用 relaxed 原子读写，仅要求独立值可见，不建立其他状态的 happens-before。listener 由异步 `RwLock<Option<Arc<dyn Listener>>>` 保存；每次通知前 clone `Arc`，回调期间不持 listener 锁。

## 依赖与调用关系

下游依赖以 `etcd_client::Client` 为中心：lease grant/keep-alive/revoke 管 session，`campaign`/`resign` 管 election，`watch` 感知 leader key 删除，`get`/`txn`/`delete` 管 Owner 信息，`lock`/`unlock` 管分布式锁。Tokio 提供任务、sleep、timeout、select 和锁；`CancellationToken` 形成根取消、竞选取消、session-lost 三类信号。

RustCodeGraph 对本文件识别 115 个符号，并显示其被 12 个已索引文件引用。精确 callees 证据包括：`CampaignOwner` 调用 `check_open`、`ensure_session`、`campaign_loop` 并构造 `CampaignRuntime`；`get_owner_info` 调用 `run_with_context`、`split_owner_values` 并构造 `OwnerInfo`；`AcquireDistributedLock` 调用 `retry_lock_operation`、`run_with_context` 并构造 `DistributedLock`。图的 `callers` 查询未返回调用边，因此上游位置由源码引用补证，而不是据此推断“无调用者”。

生产上游的具体关系如下：

- `pkg/session/runtime/session.rs` → `NewOwnerManager`、`AcquireDistributedLock`：普通 DDL Owner 与 bootstrap 升级锁。
- `pkg/session/runtime/normal_ddl_service.rs` → `CampaignOwner`、`ForceToBeOwner`、`SetOwnerOpValue`、`OwnerEpoch`：DDL 生命周期、升级状态与任期隔离。
- `pkg/session/runtime/ttl_runtime.rs` → `NewOwnerManager`、`CampaignOwner`：TTL job leader。
- `pkg/session/runtime/crossks_runtime.rs` / `crossks_owner.rs` → `NewOwnerManager` / `CampaignOwner`：cross-keyspace DDL leader。
- `pkg/autoid_service/autoid.rs` → `NewOwnerManager`、`SetListener`、`CampaignOwner`：AutoID leader 切换。

Cargo 中直接声明 `astersql-owner` 的 crate 还包括 `pkg/session`、`pkg/domain`、`pkg/ddl`、`pkg/ddl/ingest`、`pkg/resourcegroup/runaway` 等；依赖声明只能证明 crate 边界，不能替代具体符号调用证据。

## 错误处理与边界

- etcd 客户端错误由 `OwnerError::Etcd` 透明包装；context 取消通常映射为 `Cancelled`，根 context 已取消的入口则由 `check_open` 映射为 `Closed`。
- `get_owner_info` 的三次尝试只保存 etcd 错误；timeout 分支清空 `last_error`，三次后返回 `Cancelled`。因此 `Cancelled` 也可能代表连续操作超时，而不只代表令牌显式取消。
- `watch_owner` 对 watcher stream 结束返回 `WatcherClosed`，对 server 取消返回带 key/reason 的 `WatchCancelled`，DELETE、session 丢失和本地取消则正常返回；上层统一执行退位。
- `RetireOwner` 无条件清空 leader 并通知 listener，即使此前不是 Owner；内部热路径使用 `retire_if_owner` 避免重复通知。外部调用若依赖事件严格成对，应自行确保仅在持有身份时调用。
- `SetOwnerOpValue` 对同值更新幂等；非当前 owner 返回 `OwnerInfoNotMatch`；并发 revision 变化返回 `CompareFailed`。它使用当前原子 lease ID，正确调用前提是 manager 已有有效 session。
- `DeleteOwnerKeyByID` 是 best-effort：get/delete 出错、取消或找不到匹配项都静默返回，且只删除第一个 owner value 匹配 ID 的 key。
- `ForceToBeOwner` 忽略传入 `_ctx`，使用 manager 根 context/session；三次单次尝试均失败也只写 warning 并返回成功。这与“之后通过正常 campaign/IsOwner 确认结果”的用法绑定，不能把返回值当作领导权证明。
- `retry_lock_operation` 要求重试次数大于零，否则 panic。`AcquireDistributedLock` 对 lease grant/keep-alive 初始化阶段的错误直接传播；若 keep-alive 任务后来自行退出，句柄没有额外 lost 信号，锁的实际有效期由 etcd lease 决定。
- `DistributedLock::release` 消耗 `self`，防止同一句柄重复显式释放；若 unlock 失败仍会执行保活停止和 revoke。直接 drop 会 cancel + abort 保活任务，但不会主动 unlock/revoke，需等待 lease TTL。

## 并发与资源生命周期

所有 `OwnerManager` clone 共享一个 `Arc<OwnerManagerInner>`。竞选与 session 分别由 Tokio `Mutex<Option<...>>` 串行管理；状态读取使用 `RwLock` 或原子。`CampaignOwner` 在持有 campaign mutex 时判断旧任务是否结束并安装新任务，重复调用对活跃任务幂等。停止时取走句柄、发取消信号并 await，保证返回前竞选任务已经退出；session 停止同样等待 keep-alive 结束后 revoke。

竞选任务和 keep-alive 任务各自持有取消令牌。session keep-alive 的失败通过 `lost` 通知竞选循环；竞选循环负责退位、重建 session 并重新进入 campaign。`Close` 的原子幂等只覆盖 manager 主生命周期，不自动由 `Drop` 触发，因此拥有者必须显式调用 `Close`（生产服务的生命周期封装负责这一点）。

listener 回调按状态变更线程同步执行。`ListenersWrapper` 也串行、按 vector 顺序调用；某个 listener 阻塞或 panic 会阻断后续 listener，本文件没有隔离或恢复层。

分布式锁的 keep-alive 任务由句柄拥有。正常 `release` 等待任务停止后 revoke；异常 drop 立即 abort，避免 Tokio `JoinHandle` 默认 detach 后继续续租。测试 `test_acquire_distributed_lock` 验证同 client、跨 client 的互斥，以及遗弃句柄后短 TTL 到期可重新获取。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/owner/manager.go`，行为测试分别在 `pkg/owner/manager_test.go` 与 Rust 的 `pkg/owner/manager_test.rs`。主要同构关系为：Go `ownerManager` ↔ Rust `OwnerManagerInner`/`OwnerManager`，Go `concurrency.Session` ↔ `SessionRuntime`，Go `campaignLoop`/`campaignAndWatch` ↔ Rust `campaign_loop`/`watch_owner`，Go `getOwnerInfo`/value codec/CAS ↔ 同名 snake_case 辅助函数，Go `AcquireDistributedLock` 返回 release closure ↔ Rust `DistributedLock` RAII 句柄。

已验证的共同语义包括：默认 60 秒 session TTL；竞选成功通知 become、key 删除或停止时通知 retire；session/lease 失效后重建并重选；按 create revision 选择 leader；Owner value 的下划线格式；同值 op 更新幂等且不同 owner/revision 竞态失败；强制接管删除其他候选 key；分布式锁跨 client 互斥；多 listener 顺序广播。

Rust 的明确差异/扩展：

- `OwnerEpoch` 是 Rust trait 与调用链增加的任期防陈旧机制，Go `Manager` 没有该方法。
- Rust `Close` 有不可逆 `closed` 标志且幂等；Go `Close` 等同 `CampaignCancel`，接口注释虽禁止关闭后复用，但结构上没有同等关闭位。
- Rust `CampaignOwner` 会抑制重复活跃 task；Go 每次调用都会建立新的 goroutine，常规调用约束避免重入。
- Go session 刷新使用公共工具并支持无限重试路径；Rust lease grant 每次最多三次，循环内重建失败会退出 campaign loop，而不是无限刷新。
- Go `ForceToBeOwner` 的兼容背景、三轮等待/清旧 key/新 campaign 结构被保留；Rust 同样不把每轮失败上抛，但传入 context 未参与执行。
- Go 分布式锁返回 closure，并由 etcd concurrency session 管保活；Rust 显式保存 lease/keep-alive task，提供 `release(self)` 与 Drop 兜底。
- Go 含 metrics、failpoint、环境变量 TTL 初始化和 panic recovery；当前 Rust 文件没有对应 metrics/failpoint/env 初始化，也没有 campaign task panic 恢复逻辑。这些是迁移差异，不能在 Rust 文档中声称已支持。

## 扩展指南

新增 Manager 能力时先判断是否属于所有实现共有契约：若是，应同时更新 `Manager`、`OwnerManager`、`pkg/owner/mock.rs` 以及所有测试替身；默认方法只能用于保守安全的降级。涉及任期有效性的 DDL 行为应复用 `OwnerEpoch`，并在 `pkg/session/runtime/normal_ddl_service.rs` 的 lease/barrier 边界检查，不能只判断一次 `IsOwner`。

修改竞选/session 行为时，优先落在 `start_session`、`campaign_loop`、`watch_owner`、`BreakCampaignLoop`/`CampaignCancel` 的既有边界，保持“成为后记录 leader 再通知、失去后清 leader 再通知”“停止任务必须 await”“lease 失效必须退位”的不变量。应同步扩展独立测试 `pkg/owner/manager_test.rs`，覆盖 session close/revoke、多人竞选、watch revision、快速取消和 listener 事件顺序；测试逻辑应继续与 `manager_test.go` 对齐，不把测试内嵌回生产文件。

修改 Owner value 格式时必须同时审查 `split_owner_values`、`join_owner_values`、`get_owner_info`、`SetOwnerOpValue`、mock 状态和 Go 兼容性；还应扩展 `pkg/owner/manager_1_aster_unit_test.rs` 的编解码边界。格式变更存在滚动升级兼容风险，尤其是未知 op、含下划线 ID 和新旧节点共同读写。

修改强制接管时必须保留“多实例调用前先持分布式锁”的外部约束（生产锁接线在 `pkg/session/runtime/session.rs`），并评估 get 与 txn 之间新候选加入的竞态。修改分布式锁时应保持失败清理和 Drop 不续租，测试显式释放、取消、重试耗尽、遗弃后 TTL 回收；性能上关注每把锁一个 keep-alive task、500ms 线性退避和最多十次尝试的累计延迟。

错误语义若需增强，应优先让 timeout 与显式取消可区分，并决定 `ForceToBeOwner` 是否需要报告三次失败；这会改变现有调用契约，必须同步 Go 对照分析和上游错误处理，而不是只改枚举。

## 验证依据

- 源码全貌：`pkg/owner/manager.rs`（986 行）；核对了常量、`OwnerError`、traits、`OwnerManager` 私有辅助函数与 `Manager` 实现、Owner 信息函数、分布式锁、listener 包装器。
- crate 边界：`pkg/owner/Cargo.toml`、`pkg/owner/lib.rs`；确认库入口、直接依赖、公开再导出及独立测试装配。`pkg/owner/doc.go` 不存在。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件，本文件被识别为 115 个符号并被 12 个文件使用；执行了 `files --filter pkg/owner`、文件 `node` 全量读取、主要符号 `query`，以及 `NewOwnerManager`、`CampaignOwner`、`AcquireDistributedLock`、`get_owner_info` 的 `callers`/`callees`。`callers` 无输出，故上游调用以 `rg` 和对应源码上下文补证。
- Rust 上游：`pkg/session/runtime/session.rs`、`normal_ddl_service.rs`、`ttl_runtime.rs`、`crossks_runtime.rs`、`crossks_owner.rs`、`pkg/autoid_service/autoid.rs`。
- Go 对照：`pkg/owner/manager.go`、`pkg/owner/manager_test.go`；核对接口、session/campaign/watch、强制接管、value CAS、分布式锁与 listener 行为。
- Rust 测试：`pkg/owner/manager_test.rs` 验证锁重试、强制接管、session/revoke 恢复、epoch、op CAS、多节点交接、watch、快速取消、锁生命周期和 listener；`pkg/owner/manager_1_aster_unit_test.rs` 验证 value codec、`OpType` 和 listener 广播。测试由 `pkg/owner/lib.rs` 以独立文件模块装配。
- 本任务为纯文档分析，按计划不运行 Cargo；最终结构验证要求文档存在且恰有 11 个固定二级标题。

# `br/pkg/streamhelper/advancer_daemon.rs`

## 文件定位

本文件位于 `astersql-br-pkg-streamhelper` crate 内，由 `br/pkg/streamhelper/lib.rs` 以 `pub mod advancer_daemon` 挂载并用 `pub use advancer_daemon::*` 重新导出。它不是独立的守护循环实现，而是把同 crate 中的 `CheckpointAdvancer` 补充为一组与 Go `daemon.Interface` 同名的生命周期方法，并提供日志备份 owner 的固定标识与可观测状态。

真正负责竞选、定时 tick 和失主取消的 Rust 实现在独立 crate `br/pkg/streamhelper/daemon`。截至当前代码，本文件没有依赖该 crate，也没有为 `CheckpointAdvancer` 实现 `daemon::Interface`；仓库搜索只发现测试直接调用这些固有方法。因此，它目前是生命周期适配层的局部移植，而不是已经接入完整应用守护链的证明。

## 核心职责

- `OnStart` 从 `Env` 读取并应用初始任务事件，使 advancer 在成为 owner 前也能获知任务。
- `OnBecomeOwner` 标记当前进程持有 advancer owner 角色，创建 flush 订阅器，并刷新 TiKV 日志备份 flush 间隔。
- `OnTick` 在每轮推进前刷新 flush 间隔，再调用 `CheckpointAdvancer::tick` 执行实际检查点推进。
- `OnStop` 清除 owner 标志，关闭全局检查点存储边界并停止订阅器。
- `Name`、`ownerPrompt`、`ownerPath` 及三个 `OwnerManager*` 辅助函数保持守护名称、竞选提示串、etcd 路径和实例 ID 的协议值。

本文件不实现 region 扫描、检查点聚合、元数据监听细节或 owner 竞选循环；这些逻辑分别位于 `advancer.rs`、`advancer_cliext.rs`、`flush_subscriber.rs` 和 `daemon/owner_daemon.rs`。

## 主要符号

- `pub const ownerPrompt: &str = "log-backup"`：Go owner manager 使用的角色提示串。
- `pub const ownerPath: &str = "/tidb/br-stream/owner"`：日志备份 owner 的 etcd 竞选路径。
- `static ADVANCER_OWNER: AtomicBool`：进程级 owner 状态，初始为 `false`，不是每个 `CheckpointAdvancer` 实例各自持有。
- `IsAdvancerOwner() -> bool`：以 `SeqCst` 顺序读取全局 owner 状态。
- `CheckpointAdvancer::OnTick(&self) -> Result<(), String>`：忽略配置刷新错误，但原样返回 `tick()` 的推进错误。
- `CheckpointAdvancer::OnStart(&self)`：调用 `StartTaskListener()`，并刻意忽略启动监听失败。
- `CheckpointAdvancer::OnBecomeOwner(&self)`：先发布 owner 状态，再安装订阅器并尝试刷新配置；无返回值。
- `CheckpointAdvancer::Name(&self) -> &'static str`：固定返回 `LogBackup::Advancer`。
- `CheckpointAdvancer::OnStop(&self)`：清 owner 状态、关闭外部检查点存储边界、停止订阅器。
- `OwnerManagerForLogBackupId() -> String`：每次用 `Uuid::new_v4()` 生成新的竞选身份字符串。
- `OwnerManagerPath()` / `OwnerManagerPrompt()`：公开返回上述两个静态协议值，主要供调用方和测试核验。

## 执行流程

1. 启动阶段调用 `OnStart`。它委托 `advancer.rs::StartTaskListener` 获取任务快照并经 `onTaskEvent` 建立 `task`、任务范围、检查点与 GC 阻塞状态。失败不会从 `OnStart` 传播。
2. 外部判定成为 owner 后调用 `OnBecomeOwner`。该方法把 `ADVANCER_OWNER` 设为 `true`，通过 `SpawnSubscriptionHandler` 创建 `FlushSubscriber`，随后调用 `refreshLogBackupFlushInterval` 更新 resolve-lock 与推进阈值；刷新失败被忽略。
3. 每轮调用 `OnTick` 时再次刷新 flush 间隔，然后进入 `tick`。`tick` 在无任务或暂停时成功空转；否则依次执行 optional 与 important 推进路径，并把两侧错误合并为字符串返回。
4. 失主或关停时调用 `OnStop`。它先把全局 owner 状态设为 `false`，再调用 `closeGlobalCheckpointStorage` 和 `stopSubscriber`。当前 `closeGlobalCheckpointStorage` 在未配置存储时是空操作；`stopSubscriber` 在持锁状态下取走订阅器并执行 `Clear`。
5. owner 竞选所需的身份、路径和提示串可分别通过 `OwnerManagerForLogBackupId`、`OwnerManagerPath`、`OwnerManagerPrompt` 获取；本文件不会把它们组装成 manager。

`daemon/owner_daemon.rs` 展示了预期外层顺序：`CampaignOwner -> OnStart`，首次 owner tick 时 `OnBecomeOwner -> OnTick`，失主时取消 owner 上下文。不过当前 `CheckpointAdvancer` 尚未实现该 crate 的 trait，所以这条链是相邻实现的设计证据，不是本文件已完成接线的调用边。

## 数据与状态

本文件自己持有的唯一可变状态是进程级 `ADVANCER_OWNER`。它使用 `AtomicBool` 和 `Ordering::SeqCst`，保证不同线程对 owner 标志的读写有全序；但该布尔值不包含 lease、epoch 或实例身份，多个 `CheckpointAdvancer` 实例会共享并相互覆盖它，因此只能作为本进程的简化观测值，不能代替 owner manager 的权威判断。

其余状态属于 `CheckpointAdvancer`：`task`、`taskRange`、配置、最近检查点、检查点区间树与 `subscriber` 等分别由 `Mutex` 或原子量保护。`OnBecomeOwner` 会替换 `subscriber`；`OnStop` 会将其取出并清理。`OwnerManagerForLogBackupId` 不缓存 UUID，每次调用都产生不同字符串，调用方若需要稳定的竞选身份必须自行保存一次生成结果。

## 依赖与调用关系

直接依赖只有标准库 `AtomicBool`、外部 crate `uuid::Uuid` 和本 crate 的 `CheckpointAdvancer`。`Cargo.toml` 为 `uuid` 开启 `v4` feature，这正是 `Uuid::new_v4` 可用的依赖依据；crate 入口 `lib.rs` 对外重新导出本文件的所有公开符号。

下游调用边为：`OnStart -> StartTaskListener`，`OnBecomeOwner -> SpawnSubscriptionHandler + refreshLogBackupFlushInterval`，`OnTick -> refreshLogBackupFlushInterval + tick`，`OnStop -> closeGlobalCheckpointStorage + stopSubscriber`，`OwnerManagerForLogBackupId -> Uuid::new_v4`。其中实际推进、订阅锁与任务状态都在 `advancer.rs` 中实现。

RustCodeGraph 显示 `IsAdvancerOwner` 与 `OwnerManagerForLogBackupId` 的直接调用者仅为 `advancer_daemon_test.rs`，整个文件另被 `parity_test.rs` 使用；普通源码搜索也未发现生产 Rust 调用者或 `daemon::Interface` 实现。Go 侧的生产接线则由 `daemon/owner_daemon.go` 调用 `OnStart`、`OnBecomeOwner`、`OnTick`，并由 `OwnerManagerForLogBackup` 构造真实 `owner.Manager`。

## 错误处理与边界

- `OnTick` 只传播 `tick()` 的 `String` 错误；本轮配置刷新失败被丢弃，因此调用者不能据此区分“配置仍沿用旧值”和“刷新成功”。
- `OnStart` 忽略 `StartTaskListener` 的错误，保持生命周期入口本身无返回值；失败后 advancer 可能仍没有任务，后续 `tick` 会按无任务路径成功空转。
- `OnBecomeOwner` 同样忽略刷新错误，而且在创建订阅器之前就发布 owner 标志；读取 `IsAdvancerOwner` 为真不代表订阅拓扑已成功更新。
- `OnStop` 无返回值，`closeGlobalCheckpointStorage` 当前为空操作，订阅器的 `Clear` 也没有向本层暴露失败。
- `Mutex::lock().unwrap()` 等下游实现可能在锁中毒时 panic；与 Go `OnTick` 使用 `PanicToErr` 将 panic 转成错误不同，Rust 本文件没有 panic 捕获和耗时指标记录。
- UUID、owner 路径和提示串只提供构件，没有校验 etcd 客户端、lease 或竞选状态；不要把 `IsAdvancerOwner` 当成跨进程一致的选主结果。

## 并发与资源生命周期

owner 标志的 `SeqCst` 原子访问适合跨线程读取，但它是全局单比特状态，无法区分重复成为 owner、旧 owner 会话延迟停止或多实例并存。当前 `OnBecomeOwner`/`OnStop` 也没有上下文参数：Go 版本会监听 owner context 的 `Done` 并自动调用 `OnStop`，Rust 版本必须由外层显式停止，否则 owner 标志和订阅器会继续保留。

订阅器生命周期为 `OnBecomeOwner` 创建、`OnTick` 使用、`OnStop` 清理。`advancer.rs::subscribeTick` 在更新拓扑和处理错误期间持有 `subscriber` 锁；`stopSubscriber` 取得同一把锁，所以会等待正在执行的订阅 tick 完成。`advancer_test.rs::test_owner_dropped` 对这一互斥顺序进行了并发回归验证，并确认停止后仍可用轮询路径推进检查点。

任务监听在当前 Rust 端口中是同步地消费 `Env::Begin` 提供的批次，不会像 Go 版本那样由本文件持有可取消后台 goroutine。资源扩展时应明确由谁拥有取消令牌、join handle 和清理顺序，避免只切换 `ADVANCER_OWNER` 而遗留后台任务。

## 与 Go 版本的对应关系

Rust 的 `ownerPrompt`、`ownerPath`、`Name` 以及四个生命周期方法与 `advancer_daemon.go` 同名并保持主要顺序。`OnTick` 都执行实际 `tick`，`OnStart` 都启动任务监听，成为 owner 都启动订阅与配置刷新，停止都关闭检查点存储和订阅。

当前仍有明确差异：

- Go `OnTick(ctx)` 记录 tick 耗时并用 `PanicToErr` 恢复 panic；Rust 无 context、指标与 panic 转换，并额外在每轮同步刷新配置。
- Go `OnBecomeOwner(ctx)` 设置 owner 指标、启动订阅和独立配置更新 goroutine，并在 ctx 取消时自动 `OnStop`；Rust 仅设置全局原子、同步创建订阅器并单次刷新。
- Go `OnStop` 重置三个指标，在 `taskMu` 下关闭全局存储，再停止订阅；Rust 没有这些指标，本地存储关闭目前为空操作，锁结构也不同。
- Go `OwnerManagerForLogBackup(ctx, etcdCli)` 返回真实 `owner.Manager`，用一次 UUID、提示串和路径参与 etcd 竞选；Rust 只分别提供 UUID、路径和提示串，没有 manager 工厂。
- Go 的 `CheckpointAdvancer` 满足 `daemon.Interface` 并接入 `OwnerDaemon`；Rust 本文件提供固有方法，但没有实现独立 daemon crate 的 `Interface`，签名也不兼容。

因此扩展或审计时，应以这些差异为迁移状态，而不能仅因名字一致就推断行为完全等价。

## 扩展指南

- 若要完成生产接线，优先在 crate 依赖边界上设计 `CheckpointAdvancer` 到 `daemon::Interface` 的适配，并明确 `Context`、错误类型、可变接收者与 `Arc` 所有权；同时增加独立测试覆盖 `Begin -> OnStart -> OnBecomeOwner -> OnTick -> 失主清理`，不能只测试直接方法调用。
- 若要实现真实 owner manager，应新增等价于 Go `OwnerManagerForLogBackup` 的工厂并复用一次生成的 UUID；同步验证 `ownerPrompt`、`ownerPath` 与 etcd lease/竞选实现，而不是扩大 `IsAdvancerOwner` 的职责。
- 修改启动或停止逻辑时，必须同步 `advancer_daemon_test.rs`，并关注 `advancer_test.rs::test_owner_dropped` 的锁等待不变量；测试逻辑保持在独立测试文件，不嵌入本源文件。
- 修改 `OnTick` 错误策略时，同时核对 `CheckpointAdvancer::tick` 的“无任务/暂停成功空转”和 optional/important 错误合并语义，以及 Go 的 panic 恢复与 daemon 循环“记录错误但继续”契约。
- 引入后台配置刷新或上下文取消时，需规定重复 `OnBecomeOwner`、多次 `OnStop`、旧会话迟到取消和线程退出的幂等性；资源必须在停止返回前可证明已清理。
- 改动两个协议字符串会影响 owner 竞选兼容性；除非有迁移方案，不应把它们当作普通展示文本修改。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；`node --file br/pkg/streamhelper/advancer_daemon.rs` 覆盖目标文件 74 行，并报告使用者为 `advancer_daemon_test.rs` 与 `parity_test.rs`。
- RustCodeGraph `node/callers/callees`：`IsAdvancerOwner` 读取 `ADVANCER_OWNER`，调用者为生命周期测试；`OwnerManagerForLogBackupId` 的调用者为 UUID 契约测试，并调用 `Uuid::new_v4`。
- 源码：`br/pkg/streamhelper/advancer_daemon.rs`、`advancer.rs`、`advancer_cliext.rs`、`lib.rs`、`daemon/interface.rs`、`daemon/owner_daemon.rs`。
- crate 配置：`br/pkg/streamhelper/Cargo.toml` 声明库入口和 `uuid` v4 依赖；`br/pkg/streamhelper/daemon/Cargo.toml` 证明守护框架是独立 crate，主 streamhelper manifest 当前不依赖它。
- Go 对照：`br/pkg/streamhelper/advancer_daemon.go`、`daemon/interface.go`、`daemon/owner_daemon.go`；相关 Go 回归见 `advancer_test.go::TestOwnerDropped` 及生命周期调用段落。
- Rust 测试：`advancer_daemon_test.rs` 验证任务快照、owner 标志、订阅清理、协议字符串和 UUID；`parity_test.rs` 验证名称、tick、owner 生命周期与协议值；`advancer_test.rs::test_owner_dropped` 验证停止等待进行中的订阅 tick。
- 本任务为纯文档分析，按计划未运行 Cargo；交付结构验证只检查目标文档存在且固定二级标题恰为 11 个。

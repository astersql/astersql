# `br/pkg/backup/prepare_snap/prepare.rs`

## 文件定位

本文件是 workspace 成员 crate `astersql-br-pkg-backup-prepare-snap` 的核心状态机实现。crate 入口 `br/pkg/backup/prepare_snap/lib.rs` 将它声明为 `prepare` 模块，并重新导出 `New` 与 `Preparer`；`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 Go 包 `br/pkg/backup/prepare_snap`。它负责进入快照备份窗口前的 TiKV `WaitApply` 协调，以及窗口结束时的流清理，不负责真正读取或写出备份数据。

当前 Rust 接线需与 Go 的生产位置区分：根 `Cargo.toml` 把本 crate 纳入 workspace，但仓库内没有其他 `Cargo.toml` 声明对 `astersql-br-pkg-backup-prepare-snap` 的依赖。现有 Rust operator 文件 `br/pkg/task/operator/prepare_snap.rs` 从同 crate 的 `stubs.rs::NewPreparer` 获取另一套桩 `Preparer`，并未调用本文件的 `prepare::New`。因此，本文件目前是独立、可测试的移植实现，而不是 Rust operator 已接通的生产入口。

## 核心职责

- `DriveLoopAndWaitPrepare` 驱动“建立所有存活 TiKV store 的 prepare 流 → 查询当前 region 覆盖 → 按 leader 发送 `WaitApply` → 消费完成/错误事件 → 补齐空洞”的状态机，直到整个键空间都被成功 region 覆盖。
- `PrepareConnections` 先收集所有连接，再初始化每 store 的 `prepareStream`，避免在尚未完成全量拨号时提前激活部分 store 的 lease。
- `onEvent` 与 `removePendingRequest` 只接受 region id、epoch 都匹配的 `WaitApplyDone`，丢弃陈旧响应；失败 region 进入退避重试，成功 region 进入有序覆盖集合。
- `checkHole`、`workOnPendingRanges`、`pushWaitApply` 和 `sendWaitApply` 共同完成全键空间空洞检测、按最新 PD 视图重新装载 region、按 leader store 批量路由，以及重试次数限制。
- `Finalize` 并行关闭所有 per-store 流，并在关闭期间持续处理、最后排空事件，确保 lease 过期或连接错误不会被静默吞掉。

## 主要符号

- `defaultMaxRetry = 60`、`defaultRetryBackoff = 5s`：组合起来约束持续失败场景的总重试时间量级；一批失败区间只消耗一次重试机会。
- `defaultLeaseDur = 120s`：`New` 写入 `Preparer::LeaseDuration`，创建流时再传给 `prepareStream::new`。
- `pendingRequests = HashMap<u64, PrepareSnapshotBackupRequest>`：以 leader store id 为键聚合 `WaitApply` 请求。
- `rangeOrRegion { id, startKey, endKey }`：统一描述真实 region 与纯空洞；`id == 0` 表示空洞，空 `endKey` 表示正无穷。
- `Preparer`：状态机主体。`env` 提供 PD/TiKV 边界；`inflightReqs` 跟踪尚未确认的 region；`failed` 保存待重试区间；`waitApplyDoneRegions` 按起始键排序保存成功覆盖；`clients` 缓存每 store 的流；`event_tx/event_rx` 汇聚后台事件。
- `New(Arc<dyn Env>) -> Preparer`：唯一构造入口，创建容量 128 的同步事件通道并设置默认重试与 lease 参数。
- 对外方法：`DriveLoopAndWaitPrepare`、`Finalize`、`PrepareConnections`、`AdvanceState`、`WaitAndHandleNextEvent`；后两者和 `wait_apply_finished` 主要支持分步驱动和测试观察。
- 内部方法：`batchEvents`、`removePendingRequest`、`onEvent`、`checkHole`、`workOnPendingRanges`、`sendWaitApply`、`streamOf`、`createAndCacheStream`、`pushWaitApply`。

## 执行流程

1. 调用方通过 `New` 创建实例，并必须在启动前设置 `RetryBackoff`、`RetryLimit`、`LeaseDuration` 与可选 `AfterConnectionsEstablished`；这些配置没有运行期并发修改协议。
2. `DriveLoopAndWaitPrepare` 清零 `retryTime`，调用 `PrepareConnections`。后者通过 `Env::GetAllLiveStores` 枚举 store、逐个 `ConnectToStore`，全部拨号完成后才逐个 `createAndCacheStream`。
3. `createAndCacheStream` 构造 `prepareStream` 并调用 `InitConn`。相邻文件 `stream.rs` 显示，`InitConn` 会先完成 `UpdateLease` 握手，再启动收包和续约后台线程；初始化失败的流不会写入 `clients`。
4. 所有连接建立后执行 `AfterConnectionsEstablished`，随后 `AdvanceState` 检查状态。初始成功集合为空，因此 `checkHole` 返回整个键空间空洞 `[empty, +inf)`。
5. `workOnPendingRanges` 对每个空洞调用 `Env::LoadRegionsInKeyRange`，`pushWaitApply` 将 region 按 leader store 聚合，同时用 region id 登记 `inflightReqs`；`sendWaitApply` 确保相应流存在并发送请求。
6. `WaitAndHandleNextEvent` 阻塞等事件或重试截止时间。收到一个事件后，`batchEvents` 非阻塞取尽当前缓冲，再依次调用 `onEvent`，最后统一 `AdvanceState`。
7. 对 `eventWaitApplyDone`，只有 `removePendingRequest` 验证 id 与 epoch 后才修改状态。失败响应加入 `failed` 并重置退避截止时间；成功响应按 `startKey` 写入 `waitApplyDoneRegions`。`eventMiscErr` 直接作为不可恢复错误上抛，未知事件类型返回 `unsupported`。
8. 当 `inflightReqs` 与 `failed` 都为空时，`AdvanceState` 再次检查覆盖。存在空洞就重新进入步骤 5；无空洞则设置 `waitApplyFinished = true`，主循环返回，此后集群仍处于 prepare/lease 窗口。
9. 调用方完成快照操作后必须调用 `Finalize`。它移出全部 `clients`，每条流在线程中执行 `prepareStream::Finalize`；主线程同时消费事件。所有线程结束后先排空缓冲事件再成功返回，任一流错误、事件错误或 context 取消都会提前返回错误。

## 数据与状态

- `inflightReqs` 的键是 region id，值保留发送时的完整 region 元数据。响应只有在 id 存在且 epoch 的 `(Version, ConfVer)` 相等时才能删除该项；缺失 epoch 与显式零 epoch 都规范化为 `(0, 0)`，保持 Go protobuf getter 的零值语义。
- `failed` 同时承载明确失败的 region 和覆盖扫描生成的空洞。`workOnPendingRanges` 使用 `mem::take` 清空旧集合，再依照当前 region 拓扑重新解析，因此 region split/merge 后不会继续发送陈旧边界。
- `waitApplyDoneRegions` 使用 `BTreeMap<startKey, rangeOrRegion>`。同一 `startKey` 的新成功项覆盖旧项；`checkHole` 从空起始键递增扫描，并在最后一个非空 `endKey` 后补尾部空洞。该算法要求成功区间按起点有序，但不会主动拒绝重叠区间。
- `retryTime` 在每批待处理区间开始前递增，超过 `RetryLimit` 才返回 `retryLimitExceeded`；`nextRetryAt` 表示失败聚合后的截止时间，后来的失败会把截止时间重置为“当前时刻 + RetryBackoff”。
- `clients` 在正常驱动期间按 store id 缓存流；重试发现新 leader store 时，`streamOf` 可懒建立额外流。`Finalize` 用 `mem::take` 消耗整个 map，调用后实例不再保有这些流。
- `waitApplyFinished` 只表示当前成功集合已经覆盖全键空间，不表示 lease 已释放；正常模式恢复以 `Finalize` 成功为准。

## 依赖与调用关系

- 上游 API 面：`lib.rs` 公开 `New`/`Preparer`。RustCodeGraph 将 `DriveLoopAndWaitPrepare` 的直接内部边解析为 `PrepareConnections → AdvanceState → WaitAndHandleNextEvent`，并将 `WaitAndHandleNextEvent` 的下游解析为 `batchEvents`、`onEvent`、`AdvanceState`、`workOnPendingRanges`。
- 当前接线限制：全仓 Cargo manifest 搜索仅在本 crate 自己的 `Cargo.toml` 找到包名；Rust operator 的 `pauseAdminAndWaitApply` 调用的是 `task/operator/stubs.rs::NewPreparer`。若要进入真实 Rust BR 主链，需要显式添加 crate 依赖和环境适配，不能仅假设同名逻辑已经接线。
- `env.rs` 提供 `Context`、`Env`、`PrepareClient`、`Region` 及本地 `brpb/metapb` 数据结构。`Env` 的三个关键下游分别是 `GetAllLiveStores`、`ConnectToStore`、`LoadRegionsInKeyRange`；`CliEnv` 还负责过滤 TiFlash 并组合 region cache 与 store manager。
- `stream.rs` 提供 `prepareStream` 和事件类型。它负责 `UpdateLease` 握手、周期续约、响应到 `event` 的转换与 `Finish` 清理；本文件只调度流并解释事件。
- `errors.rs` 提供本 crate 的 `Error/Result` 及 `retryLimitExceeded`、`unsupported`。本文件在每层边界追加动作和 store/range 上下文。
- Go 对照为同目录 `prepare.go`；Rust 独立测试为 `prepare_test.rs`，契约补充为 `parity_test.rs`，Go 回归为 `prepare_test.go`。

## 错误处理与边界

- 获取 store、拨号、建流、加载 region、发送请求均立即返回带上下文的错误；失败的 `createAndCacheStream` 不缓存半初始化流。
- `eventMiscErr` 被视为不可恢复错误，并附带 store id。未知事件类型返回 `unsupported`；缺少 region 的 `eventWaitApplyDone` 被忽略。
- 陈旧或 epoch 不匹配的响应只被丢弃，不会清除当前 inflight，也不会污染成功覆盖。该行为避免 split/merge 或重试期间旧响应提前宣布完成。
- 单 region 的业务错误不是立即终止条件：它进入 `failed`，等待退避后按当前拓扑重载。持续失败在 `retryTime > RetryLimit` 时终止。
- 事件通道断开在主驱动中是错误；在 `Finalize` 中，所有 producer 已结束后的断开或排空则是正常结束信号。
- `Context` 取消在等待循环和 `Finalize` 中被轮询并上抛。`PrepareClient::Send/Recv` 本身是否及时响应取消由环境/流实现保证。
- 全覆盖判断把空 `endKey` 当作正无穷。修改键范围语义时必须同步检查 `checkHole` 与 `Env::LoadRegionsInKeyRange` 的空上界转换，避免尾部数据漏备份。
- `Finalize` 报错不应被调用方忽略，因为失败可能意味着 lease 未清理或关闭期间发现不可恢复流错误。

## 并发与资源生命周期

- `Preparer` 的状态字段由 `&mut self` 方法串行修改；注释与 Go 约束一致，驱动开始后不应把同一实例交给其他线程并发调用。公开配置也必须在启动前设置。
- 事件通道是容量 128 的 `sync_channel`。每条 `prepareStream` 的后台线程共享 `event_tx`，主状态机通过 `Mutex<Receiver<event>>` 串行接收；容量上限提供反压，但事件消费者停止时 producer 可能阻塞。
- 每 store 流的具体后台生命周期在 `stream.rs`：`InitConn` 启动收包/续约线程，`Finalize` 停止并 join 后台循环、发送 `Finish`、排空服务端流。
- `Finalize` 对各流并行执行，随后由汇聚线程 join 所有句柄并传回首个记录的错误。主线程在等待期间每 20ms 检查 context 和事件，避免关闭操作与满事件通道互相等待。
- `Finalize` 会取得 `clients` 所有权；无论成功与否，原 map 已被清空。调用方若在失败后重试 `Finalize`，本文件不会自动重建或恢复已移出的流。
- `AfterConnectionsEstablished` 在全部拨号与流初始化之后、首次 `AdvanceState` 之前同步执行；耗时或阻塞钩子会直接延迟状态机。

## 与 Go 版本的对应关系

- Rust 的 `New`、`Preparer` 字段、驱动/事件/空洞/重试/发送/建流/Finalize 方法与 `prepare.go` 同名或一一对应；默认重试次数、退避、lease 和事件缓冲容量也保持一致。
- Go 用 `btree.BTreeG` 保存成功区间，Rust 用 `BTreeMap<Vec<u8>, rangeOrRegion>`；两者都按 `startKey` 扫描空洞。Rust 同起点覆盖旧值，对应 Go `ReplaceOrInsert`。
- Go 用 `time.Timer` 和可空 retry channel，Rust 用 `Option<Instant>` 加短超时轮询；两者都在新失败到达时重置退避，使一批失败聚合后一起重试。
- Go 用 channel/select 和 `errgroup`，Rust 用 `SyncSender/Receiver`、线程与 join 汇聚。Rust `Finalize` 明确在所有流完成后排空已缓冲事件，保留 Go “关闭前继续处理事件”的控制流意图。
- Rust 额外规范化可选 epoch，显式复现 Go 生成 getter 将 nil epoch 与零值 epoch 视作相同的行为；`prepare_test.rs::test_missing_and_zero_epoch_match_like_go_getters` 固化了这个兼容点。
- Rust 当前没有移植 Go 的 `MarshalLogObject` 与 zap 日志；这影响可观测性，不改变状态机结果。Go `PrepareConnectionsErr` failpoint 也未出现在本文件，Rust 测试通过 mock 环境注入相应错误。
- Go 生产包被 BR 流程直接使用；本 Rust crate 当前未被其他 manifest 依赖，operator 仍走独立桩实现。这是接线状态差异，而非本状态机内部语义差异。

## 扩展指南

- 新增事件类型时，在 `stream.rs::convert_to_event` 与本文件 `onEvent` 同步添加映射和状态转移，并在 `prepare_test.rs` 独立增加成功、错误、陈旧响应和 Finalize 期间到达事件的用例。
- 调整 region 成功判定或支持 split/merge 新语义时，重点修改 `removePendingRequest`、`checkHole`、`workOnPendingRanges`；必须保留“旧 epoch 不得清除新 inflight”和“空 endKey 为正无穷”两个兼容约束。
- 调整重试策略时，以 `RetryLimit` 的批次计数、失败聚合时钟重置和 context 取消为整体修改面，并同步 Rust `test_fail_due_to_err`、`test_error`、`parity_test.rs::contract_error_retry_limit` 及 Go `TestFailDueToErr`/`TestError` 的意图。
- 改动连接建立顺序时要保持 `PrepareConnections` 的两阶段结构；`test_connection_delay`/Go `TestConnectionDelay` 验证拨号未全部完成前不能让部分 store 提前进入有效 lease。
- 改动资源关闭时同时审查本文件 `Finalize` 与 `stream.rs::stopClientLoop`，并同步 `test_lease_timeout`、`test_many_messages_when_finalizing` 和 `contract_resource_finalize_clears_lease`。兼容风险是遗留 lease，性能风险是事件反压或 join 等待造成关闭停顿。
- 若把本 crate 接入 Rust operator，应新增明确的 Cargo 依赖，实现真实 `RegionCacheLike`/`StoreManagerLike`，并替换 `task/operator/stubs.rs::NewPreparer` 路径；接线测试需要证明 operator 的 prepare、备份窗口保持及 cleanup 都实际调用本 `Preparer`。
- Rust 测试逻辑必须继续放在独立的 `prepare_test.rs`/`parity_test.rs`，不要内嵌进生产源文件；同时保持与 `prepare_test.go` 的场景和断言意图对齐。

## 验证依据

- RustCodeGraph：`status` 显示目标文件已索引；`node --file br/pkg/backup/prepare_snap/prepare.rs` 核对了 578 行完整实现；`query` 定位 `DriveLoopAndWaitPrepare`、`WaitAndHandleNextEvent`、`PrepareConnections`；`callees` 验证了驱动、事件、推进、空洞和重试方法之间的调用边。
- crate 与接线：读取 `br/pkg/backup/prepare_snap/Cargo.toml`、`lib.rs` 和根 `Cargo.toml`；全仓检索包名只命中本 crate manifest。读取 `br/pkg/task/operator/prepare_snap.rs` 与 `stubs.rs::NewPreparer`，确认当前 operator 使用另一套桩入口。
- 直接依赖：读取 `env.rs` 的 `Env`/`PrepareClient`/`CliEnv`，以及 `stream.rs` 的 `prepareStream::InitConn`、`Finalize`、`client_loop`、`convert_to_event`，核对连接、lease、事件与关闭边界。
- Go 对照：读取 `br/pkg/backup/prepare_snap/prepare.go` 全部状态机，逐项核对常量、字段、方法和异常路径。
- 测试证据：读取 `prepare_test.rs` 的基础成功、epoch 零值、持续/瞬时失败、lease 超时、连接延迟、钩子时序、Finalize 压力和 context 取消用例；读取 `parity_test.rs` 的正常、重试上限和资源清理契约；读取 `prepare_test.go` 对应 Go 用例。按任务约束，本次纯文档分析未运行 Cargo 或测试。

# `br/pkg/restore/import_mode_switcher.rs`

## 文件定位

该文件属于 `astersql-br-pkg-restore` library crate；crate 清单是 [`br/pkg/restore/Cargo.toml`](Cargo.toml)，模块由 [`br/pkg/restore/lib.rs`](lib.rs) 通过 `pub mod import_mode_switcher` 挂载并以 `pub use import_mode_switcher::*` 扁平导出。它位于 BR 恢复的准备与收尾边界：在批量写入 SST 前把非 TiFlash 的 TiKV 切入 Import 模式并暂停 PD 调度，在恢复结束后切回 Normal 模式并执行调度恢复闭包。

Rust 生产接线位于 [`br/pkg/task/restore_lifecycle.rs`](../task/restore_lifecycle.rs)：`RestoreLifecycle::Start` 创建 `ImportModeSwitcher`，按是否有 key ranges 选择 `FineGrainedRestorePreWork` 或 `RestorePreWork`；`RestoreSession::drop` 通常调用 `RestorePostWork`，而 checkpoint 保留现场时只调用 `StopRefreshing`。因此本文件不是仅供测试的兼容门面，而是 Rust restore 生命周期的资源管理组件。

## 核心职责

1. `SkipTiFlash` 与 `GetAllTiKVStores` 从 PD store 列表中过滤 `engine=tiflash`（键和值均忽略 ASCII 大小写），保证模式切换只发往应参与 SST 导入的 TiKV。
2. `ImportModeSwitcher` 维护 Normal/Import 刷新状态。`GoSwitchToImportMode` 先同步完成一次 Import 切换，再启动周期刷新线程，避免 TiKV 自动退回 Normal；`SwitchToNormalMode` 先停止并等待刷新线程，再同步切回 Normal。
3. `RestorePreWork`、`FineGrainedRestorePreWork` 和 `RestorePostWork` 把 TiKV 模式切换与 PD scheduler 暂停/恢复组合成恢复前后处理协议。
4. `GrpcImportSstSwitcher` 实现 `ImportSstSwitcher` 网络边界，为每个 store 建立 grpcio channel，并调用 kvproto ImportSST `SwitchMode` RPC。

核心不变量是：后台刷新不能与 Normal 收尾并行；离线恢复的 scheduler 撤销动作即使模式恢复失败也仍要尝试；在线恢复不应由 `RestorePostWork` 切换为 Normal。

## 主要符号

- `pub fn SkipTiFlash(&metapb::Store) -> bool`：store 保留谓词。仅当存在 `engine=tiflash` 标签时返回 `false`。
- `pub fn GetAllTiKVStores(...) -> Result<Vec<metapb::Store>>`：调用 `PdClient::GetAllStores` 后应用调用方提供的过滤谓词；模式切换路径传入 `SkipTiFlash`。
- `pub struct ImportModeSwitcher`：持有 `Arc<dyn PdClient>`、`Arc<dyn ImportSstSwitcher>`、刷新间隔、互斥锁、取消函数、唤醒发送端和共享 `WaitGroup`。字段私有，状态只能通过方法转换。
- `pub fn NewImportModeSwitcher(...) -> ImportModeSwitcher`：构造初始 Normal 状态；`cancel` 与 `refresh_wake` 均为 `None`。
- `StopRefreshing(&mut self)`：停止并唤醒刷新线程后等待退出，但不切 Normal、不恢复 scheduler，专供 checkpoint 重试保留暂停状态。
- `SwitchToNormalMode(&mut self, &Context) -> Result<()>`：幂等收尾；没有 `cancel` 时直接成功，否则取消、唤醒、等待，再调用 `switchTiKVMode(Normal)`。
- `switchTiKVMode(&self, ..., SwitchMode) -> Result<()>`：内部扇出。读取并过滤 stores，以 store 数量创建 worker pool，通过 `ErrorGroup` 并发调用 transport；任务地址来自 `Store::GetAddress`。
- `GoSwitchToImportMode(&mut self, &Context) -> Result<()>`：幂等进入 Import。首次调用创建子 context 和唤醒 channel，先执行同步 Import 切换，成功后登记 wait count 并启动刷新线程。
- `RestorePreWork(...)`：在线路径返回 no-op undo 和 `None`；离线路径可选切 Import，随后调用 `ConnMgr::RemoveSchedulersWithConfig`。
- `FineGrainedRestorePreWork(...)`：可选切 Import，读取原始 PD 配置，按 key ranges 暂停调度，把返回的 `rule_id` 写入用于构造 undo 的配置副本，同时把未修改的原始配置返回给调用方。
- `RestorePostWork(...)`：无返回值的尽力而为收尾；必要时替换已取消的 context，离线时尝试切 Normal，最后无条件尝试 scheduler undo。
- `pub struct GrpcImportSstSwitcher`：包含共享 grpcio `Environment` 和可选 TLS credential 工厂；其 `SwitchMode` 是真实网络实现。

本文件没有模块级常量、enum、条件编译项或内嵌测试；测试独立放在 [`br/pkg/restore/import_mode_switcher_test.rs`](import_mode_switcher_test.rs)。

## 执行流程

离线恢复的主流程如下：

1. `RestoreLifecycle::Start` 创建 switcher，并调用普通或细粒度 pre-work。
2. pre-work 若允许切换，调用 `GoSwitchToImportMode`。该方法在 `mu` 下检查幂等状态，创建可取消的后台 context 和显式唤醒 channel。
3. `GoSwitchToImportMode` 立即调用 `switchTiKVMode(Import)`；`switchTiKVMode` 从 PD 获取所有 stores、过滤 TiFlash、按地址并发调用 `ImportSstSwitcher::SwitchMode`。首次切换失败会返回错误，恢复主体不会启动。
4. 首次切换成功后启动线程。线程等待 `switch_mode_interval`；每次超时重新向所有目标 store 发送 Import。单轮刷新失败只记录告警，线程继续下一轮。
5. pre-work 再暂停全部 scheduler，或针对 key ranges 安装 pause rule，并把对应 `UndoFunc` 交给 `RestoreSession`。
6. `RestoreSession` 生命周期结束时，正常路径由 `Drop` 调用 `RestorePostWork`。后者先让 `SwitchToNormalMode` 取消并唤醒刷新线程，`WaitGroup::Wait` 确认线程退出后，再向 stores 发送 Normal，最后调用 scheduler undo。
7. snapshot checkpoint 路径把 `restore_schedulers` 设为 `false`，析构时改走 `StopRefreshing`，只结束本操作的刷新线程，刻意保留集群模式/调度暂停状态供重试继续使用。

在线路径中，`RestorePreWork` 不切模式也不摘 scheduler；`RestorePostWork(is_online=true)` 跳过 Normal 切换，但仍调用传入的 undo。细粒度函数自身没有 `is_online` 参数，上层以 `import && !online` 控制是否切 Import。

## 数据与状态

`ImportModeSwitcher` 用 `cancel: Option<CancelFunc>` 同时表示状态和持有刷新线程的取消能力：`None` 表示没有活动刷新线程，`Some` 表示已开始 Import 生命周期。`refresh_wake: Option<Sender<()>>` 用于打断 `recv_timeout`，避免收尾等待完整刷新间隔。`wg` 只在首次同步切换成功、即将 spawn 前 `Add(1)`，线程退出时 `Done()`。

`mu: Mutex<()>` 串行化 `GoSwitchToImportMode`、`SwitchToNormalMode` 和 `StopRefreshing` 的状态转换。三个方法都持锁直到相应的停止等待或初始切换完成，因此同一实例不会重复 spawn，也不会在旧刷新线程尚未退出时开始 Normal RPC。

store 数据只在每轮 `switchTiKVMode` 时从 PD 重新获取，不在 switcher 内缓存；这使周期刷新能够看到 store 拓扑变化。每个 worker 捕获 store 地址、transport `Arc` 和 error-group context，不借用循环局部数据。

细粒度 pre-work 对 `ClusterConfig` 使用两个值：`origin_cfg` 原样返回；其 clone 写入 `RuleID` 后只用于 `MakeFineGrainedUndoFunction`。调用者由此既能保留原配置快照，又能得到精确撤销 pause rule 的闭包。

## 依赖与调用关系

RustCodeGraph 对本文件确认的关键内部调用边为：

- `SwitchToNormalMode -> switchTiKVMode -> ImportSstSwitcher::SwitchMode`；
- `GoSwitchToImportMode -> switchTiKVMode -> ImportSstSwitcher::SwitchMode`；
- `RestorePreWork -> GoSwitchToImportMode`；
- `FineGrainedRestorePreWork -> GoSwitchToImportMode`；
- `RestorePostWork -> SwitchToNormalMode`。

直接 Rust 上游是 [`br/pkg/task/restore_lifecycle.rs`](../task/restore_lifecycle.rs) 的 `RestoreLifecycle::Start`、`RestoreSession::drop`；crate 内测试还由 `import_mode_switcher_test.rs` 和 `parity_test.rs` 调用这些公开符号。RustCodeGraph 的同名查询会同时返回 Go 与 Rust 节点，因此本文只把源码直接调用核验过的 Rust 文件列为 Rust 上游。

下游抽象均来自 [`br/pkg/restore/stubs.rs`](stubs.rs)：`PdClient` 提供 store discovery；`ImportSstSwitcher` 隔离模式传输；`ConnMgr` 提供 scheduler 配置、暂停与 undo；`Context`/`CancelFunc`、`ErrorGroup`、`WorkerPool`、`WaitGroup` 提供取消和并发语义。真实 transport 直接依赖 `grpcio` 与 `kvproto`，两者均声明于本 crate 的 `Cargo.toml`；kvproto 固定引用 tag `v0.0.2-aster.20260929`。

## 错误处理与边界

- 获取 stores、首次 Import 切换、普通/细粒度 scheduler 暂停均返回 `Result` 并向上阻止恢复主体启动；错误通过 `Error::Trace` 或原始 `Result` 保留。
- `switchTiKVMode` 在提交每个任务前检查 error-group context；任一 worker 错误会由 `ErrorGroup::Wait` 返回。已经提交的任务仍会被 join。
- 首次 Import 切换失败时，代码刻意保留 `cancel=Some`，与 Go 状态机一致；随后可通过 `SwitchToNormalMode`/`StopRefreshing` 清理。此时尚未 `wg.Add(1)`，所以等待会立即返回。
- 周期刷新失败只告警，不终止线程；瞬时 store/RPC 故障由下一周期再次尝试。
- `RestorePostWork` 是尽力而为 API：若输入 context 已取消，改用 `Context::Background`；Normal 切换失败不阻止 undo，undo 失败也只告警。调用者不能通过返回值获知这两类收尾失败，只能依赖日志。
- `GrpcImportSstSwitcher::SwitchMode` 在开始和轮询期间检查 context。连接等待最多五秒；超时产生带地址的错误。RPC 使用父操作 context 的生命周期，取消时显式 `response.cancel()`。异步 future 以 10ms sleep 轮询，属于同步适配边界。
- 空 store 列表会创建容量为零的 worker pool但不提交任务，`ErrorGroup::Wait` 成功返回；这表示“没有目标”而非错误。
- `Mutex::lock` 使用 `unwrap`，锁中毒会 panic；worker panic 则由 `ErrorGroup` 转换为 `worker panicked` 错误。

## 并发与资源生命周期

模式切换具有两层并发：单个 `ImportModeSwitcher` 由互斥锁串行化状态转换；每一轮 `switchTiKVMode` 则按 store 数量创建 worker pool，并行完成跨 store RPC。共享客户端和 transport 都以 `Arc<dyn ... + Send + Sync>` 持有。

刷新线程只在首次 Import RPC 全部成功后创建。它拥有 background context、receiver、PD client、transport 和 `WaitGroup` 的 clone；退出条件是 context 已取消、收到 wake 消息或发送端断开。`SwitchToNormalMode` 和 `StopRefreshing` 同时调用 cancel 与 wake，保证正在长间隔睡眠的线程可以立即退出，然后在持锁状态下等待 `wg` 归零。

真实 gRPC 实现为每次 store/mode 调用新建 channel。channel、client 和 response 都是方法局部值，调用结束即释放；Rust 版本没有 Go 代码中显式 `connection.Close()` 的独立错误日志。TLS credentials 通过闭包按 channel 创建，避免复用可能带内部状态的 credential 实例。

资源所有权的最外层由 `RestoreSession` RAII 保证：正常析构恢复 Normal 和 schedulers；checkpoint 分支只停止刷新。若未来新增提前返回路径，应继续让会话对象覆盖该路径，不能绕过 `Drop` 后手工遗留后台线程。

## 与 Go 版本的对应关系

直接对照文件是 [`br/pkg/restore/import_mode_switcher.go`](import_mode_switcher.go)。Rust 保留了 Go 的主要顺序和分支：过滤 TiFlash、按 store 并发切换、五秒连接边界、初次同步 Import、周期刷新、取消后等待、在线 no-op pre-work、细粒度 rule undo，以及 post-work 即使模式恢复失败仍执行 scheduler undo。

主要实现差异如下：

- Go switcher 直接持有 `pd.Client` 和 `*tls.Config`，并在 `switchTiKVMode` 中构造 gRPC；Rust 将两者拆成 `PdClient` 与 `ImportSstSwitcher` trait，真实传输由 `GrpcImportSstSwitcher` 实现，测试可注入 recording transport。
- Go ticker 可由 context 取消立即唤醒；Rust 的 `recv_timeout` 本身不知道 context，所以增加 `refresh_wake` channel 以维持同样的即时退出语义。
- Go goroutine defer 同时 `wg.Done()` 和停止 ticker；Rust 没有持久 ticker，线程退出前显式 `wg.Done()`。
- Go 网络代码设置最大三秒 reconnect backoff、阻塞 dial 和非临时错误快速失败；Rust grpcio builder设置三秒最大 reconnect backoff，并用 connectivity future 实现五秒连接等待。
- Go 显式关闭连接并仅记录 close 错误；Rust 依靠局部 channel drop，没有可观测的显式 close 错误分支。
- Rust 增加 `StopRefreshing`，服务于 `restore_lifecycle.rs` 的 checkpoint 语义；该公开方法在当前 Go 对照文件中没有同名实现。
- Rust `RestorePreWork` 的配置返回类型是 `Option<ClusterConfig>`，用 `None` 表示 Go 的 nil；细粒度 Rust 返回拥有所有权的 `ClusterConfig`，而 Go 返回指针。

Go 测试 [`br/pkg/restore/import_mode_switcher_test.go`](import_mode_switcher_test.go) 使用真实 gRPC mock 和 fake PD HTTP 检查初次/周期切换及调度配置恢复。Rust 独立测试保留这些意图，并补充取消唤醒、在线收尾和真实 grpcio transport 验证。

## 扩展指南

- 新增模式切换策略时，优先扩展 `ImportSstSwitcher` 或 `GrpcImportSstSwitcher::SwitchMode`，保持 `ImportModeSwitcher` 的状态机与测试 transport 可替换性。若改变超时、重试或连接复用，需评估每轮 store 数量带来的连接/线程成本，并同步 Go 语义说明。
- 新增 store 过滤规则应集中在 `SkipTiFlash`/`GetAllTiKVStores` 附近，并在 `parity_test.rs` 增加标签大小写、混合 store 和空列表边界；不要让上层生命周期自行复制过滤逻辑。
- 改动进入/退出状态机时必须同时覆盖：重复进入 Import、重复切 Normal、首次切换失败后清理、长刷新间隔下立即取消、周期刷新错误后继续运行。测试应继续放在独立的 `import_mode_switcher_test.rs`，不要嵌回生产文件。
- 改动 pre/post-work 协议时同步检查 `RestoreLifecycle::Start`、`RestoreSession::drop` 的 checkpoint 分支，以及普通/细粒度 scheduler undo 的所有权。特别注意 `RestorePostWork` 当前不返回错误；若要让错误可见，会改变调用契约和析构路径的可处理能力。
- 增加 TLS 或认证选项时由生命周期 factory 构造 `GrpcImportSstSwitcher.credentials`，不要把环境配置重新耦合进模式状态机。
- Go 对齐修改应同步检查 `import_mode_switcher.go`、`import_mode_switcher_test.go`；Rust 验证至少更新 `import_mode_switcher_test.rs`，涉及总体公开契约时再更新 `parity_test.rs` 和 `br/pkg/task/restore_lifecycle_test.rs`。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`node --file br/pkg/restore/import_mode_switcher.rs` 读取了完整 342 行源码。
- RustCodeGraph `explore`/`query`：确认 `ImportModeSwitcher`、构造函数、pre/post-work、`GrpcImportSstSwitcher` 符号，以及 `SwitchToNormalMode -> switchTiKVMode -> SwitchMode` 等内部调用边。由于 Go/Rust 同名符号会混合，Rust 上游另以直接源码引用核验。
- 生产源码与边界：`br/pkg/restore/import_mode_switcher.rs`、`br/pkg/restore/lib.rs`、`br/pkg/restore/stubs.rs`、`br/pkg/task/restore_lifecycle.rs`。
- crate/依赖：`br/pkg/restore/Cargo.toml`，确认 library 根为 `lib.rs`，真实网络依赖为 grpcio 与带发布 tag 的 kvproto。
- Go 对照：`br/pkg/restore/import_mode_switcher.go`。
- 独立测试：`br/pkg/restore/import_mode_switcher_test.rs`、`br/pkg/restore/import_mode_switcher_test.go`、`br/pkg/restore/parity_test.rs`。它们分别验证初始与周期 Import、Normal 收尾、长睡眠即时唤醒、在线模式跳过 Normal、TiFlash 过滤、细粒度 undo 与真实 gRPC request。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务文件规定的命令确认本文恰有 11 个固定二级标题，并用 Git 暂存检查保证提交仅含本文档。

# `br/pkg/task/operator/prepare_snap.rs`

## 文件定位

本文对应源码为 [`prepare_snap.rs`](./prepare_snap.rs)。该文件属于 Cargo 包 `astersql-br-pkg-task-operator`（清单见 `br/pkg/task/operator/Cargo.toml`），由 `br/pkg/task/operator/lib.rs` 以 `pub mod prepare_snap` 挂载并通过 `pub use prepare_snap::*` 平铺导出。命令层 `br/cmd/br/operator.rs::newPrepareForSnapshotBackupCommand` 在解析 `PauseGcConfig` 后直接调用 `AdaptEnvForSnapshotBackup`，因此它是 `pause-gc-and-schedulers` / `prepare-for-snapshot-backup` 两个兼容命令名背后的 Rust 业务入口。

本文件不是完整网络实现：PD、TiKV Store、GC safepoint 和 prepare-snapshot 协议均通过 `crate::stubs` 中的本地接口或桩类型访问。特别是 `dialPD` 在没有 `DIAL_HOOKS.dial_pd` 时必定拒绝连接，而不是建立真实 PD 连接；因此当前 Rust 路径的可运行形态主要是测试注入，不能把它描述为已经接通真实集群。

## 核心职责

`AdaptEnvForSnapshotBackup` 为快照备份建立一个持续到调用方取消为止的保护窗口，协调三项条件：

1. `pauseGCKeeper` 注册并维持 BR service safepoint，避免 GC 越过备份时间点。
2. `pauseAdminAndWaitApply` 驱动 `Preparer`，等待 TiKV prepare 完成；它的连接完成回调放行 scheduler 分支。
3. `pauseSchedulerKeeper` 移除 PD schedulers，并在退出时调用 undo 恢复。

三项均调用 `ReadyL` 后，就绪监视线程执行可选 `OnAllReady`，关闭异常退出 dump 标志，并由 `hintAllReady` 打印兼容旧 operator 探测逻辑的固定日志。任一 worker 返回错误都会记录错误并触发共享取消；所有线程退出后执行 `OnExit`、关闭 PD/Store 管理器并返回记录到的首个错误。

## 主要符号

- `createStoreManager(pd, cfg) -> Result<Arc<StoreManager>>`：公开构造边界。优先调用 `DIAL_HOOKS.create_store_manager`；默认路径验证 TLS、构造与 Go keepalive 字段对应的 `KeepaliveParams`，然后创建本地 `StoreManager` 桩。`force_flush.rs` 也复用该函数。
- `dialPD(cfg) -> Result<Arc<PdController>>`：公开 PD 拨号边界。测试 hook 存在时转发；否则验证 TLS 和 PD 地址后明确返回 “failed to dial PD”。`force_flush.rs` 同样复用它。
- `AdaptEnvForSnapshotBackupContext`：四条线程共享的状态容器，持有 `pdMgr`、`kvMgr`、`PauseGcConfig`、就绪计数、预期计数（固定为 3）、取消标志和 worker 错误槽。
- `AdaptEnvForSnapshotBackupContext::Close`：依次关闭 PD controller 与 StoreManager。
- `ReadyL`：用 `SeqCst` 原子加一记录一个阶段就绪，并输出组件名。
- `cleanUpWith` / `cleanUpWithRetErr`：清理包装器。当前 Rust 版只读取 TTL 以保留接口语义，并未真正建立超时上下文；后者把清理错误追加到已有错误文本。
- `hintAllReady`：输出 `Schedulers are paused.`、`GC is paused.`、`All ready.` 三条固定英文日志。
- `AdaptEnvForSnapshotBackup(ctx, cfg) -> Result<()>`：公开主入口，负责建连、启动/汇合线程、回调、关闭资源和错误返回。
- `pauseAdminAndWaitApply`：配置并运行 `Preparer`，连接完成后释放 scheduler 栅栏，成功后等待取消并 `Finalize`。
- `pauseGCKeeper`：选择 safepoint ID/TS、启动 keeper、等待取消，再以 TTL=0 清除 safepoint。
- `pauseSchedulerKeeper`：移除 schedulers、等待取消并执行可选 undo。

## 执行流程

1. 主入口先将 `DUMP_GOROUTINE_WHEN_EXIT` 设为 `true`，调用 `dialPD`，写入 scheduler pause TTL，复验 TLS，并通过 `createStoreManager` 建立 Store 管理对象。
2. 它创建共享上下文和 `connections_established` 原子栅栏，然后启动 GC、scheduler、prepare 三条 worker 线程。
3. GC 分支无需等待栅栏：若 `SafePoint == 0`，先从 PD 读取最小 resolved TS；随后注册 safepoint 并报告 `pause_gc` 就绪。
4. scheduler 分支轮询 `connections_established`。若等待期间已取消则直接退出，避免执行没有配对 undo 的移除操作；栅栏打开后才移除 schedulers 并报告就绪。
5. prepare 分支把 TTL 写入 `Preparer.LeaseDuration`；`AfterConnectionsEstablished` 回调将栅栏设为 `true`。`DriveLoopAndWaitPrepare` 成功后报告 `pause_admin_and_wait_apply` 就绪，失败时先尝试 `Finalize` 再返回错误。
6. 第四条监视线程等待 `ready >= 3` 或取消。三项齐备时依次调用 `OnAllReady`、清除 dump 标志并打印固定 ready 日志。
7. 三个 keeper 都会在共享取消标志变为 `true` 前保持阻塞。任一 worker 出错会把错误推入 `run_errs` 并设置该标志；正常路径由调用方 `Context::Cancel` 或命令层共享取消标志结束窗口。
8. 主线程 join 所有 worker 和监视线程后执行 `OnExit`、`Close`，最后返回 `run_errs` 中的第一个错误；无错误则返回成功。

## 数据与状态

- `ready: AtomicUsize` 与 `ready_expected = 3` 构成单向就绪计数。每个 keeper 在其保护动作成功后恰调用一次 `ReadyL`；监视线程只在计数达到阈值时发布全局 ready。
- `cancel: Arc<AtomicBool>` 来自 `stubs::Context::cancellation_flag`，既承载外部取消，也承载内部首错取消。它没有区分取消来源；最终结果由 `run_errs` 是否非空决定。
- `connections_established: AtomicBool` 是 prepare 到 scheduler 的单向栅栏，保证先建立 prepare 连接、后摘除 scheduler。
- `run_errs: Mutex<Vec<Error>>` 汇集并发错误；返回顺序取决于线程取得锁的先后，只承诺返回向量中的第一个，而不是严格的时间首错。
- `BRServiceSafePoint` 的 TTL 取 `cfg.TTL.as_secs() as i64`，ID 为空时由 `MakeSafePointID` 生成，`BackupTS` 为零时改用 `PdController::GetMinResolvedTS`。退出时使用相同 ID、TTL=0、BackupTS=0 清理。
- `PauseGcConfig` 定义在 `config.rs`，默认 TTL 为 120 秒；`SafePointID` 注释限定为测试注入字段，生产路径应为空。

## 依赖与调用关系

上游调用链为 `br/cmd/br/operator.rs::newPrepareForSnapshotBackupCommand` → `operator::AdaptEnvForSnapshotBackup`。模块入口 `lib.rs` 公开再导出该符号。RustCodeGraph 还确认 `parity_test.rs::contract_resource_cleanup` 直接调用主入口，并确认 `force_flush.rs` 直接调用 `dialPD` 与 `createStoreManager`。

主入口的直接下游是 `dialPD`、`createStoreManager`、`pauseGCKeeper`、`pauseAdminAndWaitApply`、`pauseSchedulerKeeper`、`hintAllReady` 以及上下文的 `Close`。这些函数进一步依赖 `stubs.rs` 的 `PdController`、`StoreManager`、`PDClient`、`MemGCManager`、`StartServiceSafePointKeeper`、`Preparer` 和全局 `DIAL_HOOKS`。

Cargo 清单声明该 crate 是 `lib.rs` 驱动的 library；目标文件自身的并发与时间设施来自标准库，正则、序列化等清单依赖并非本文件直接使用。由于真正 PD/TiKV 客户端能力被 `stubs.rs` 截断，清单中的 `astersql-metaservice` / `astersql-objstore` 也不是本文件的直接网络实现依据。

## 错误处理与边界

- `dialPD` 的空 PD 地址和“配置了地址但没有 hook”是两个明确失败边界；后者刻意避免假成功。TLS 转换错误向上传播，StoreManager 路径额外添加 `invalid tls config` 上下文。
- 三条 worker 中任一返回错误，包装闭包都会记录错误并设置取消标志，使其他 keeper 进入清理。线程 panic 的 `join` 结果目前被丢弃，因此 panic 不会写入 `run_errs`，这是与普通 `Result` 错误不同的边界。
- `pauseAdminAndWaitApply` 的 prepare 失败路径会执行 `Finalize`，但忽略该次清理错误；正常取消后的 `Finalize` 错误也只写日志，不改变成功结果。
- `pauseGCKeeper` 会传播读取最小 resolved TS、启动 keeper和撤销 safepoint的错误；撤销错误可与已有 `err_out` 合并，不过当前调用路径开始时 `err_out` 为空。
- `pauseSchedulerKeeper` 传播移除 scheduler 的错误；undo 错误只写日志。返回 `None` undo 的桩路径仍会标记 ready 并等待取消。
- `cleanUpWith*` 虽读取 TTL，但没有像 Go 版 `context.WithTimeout` 那样强制清理超时。扩展网络实现时不能把当前函数当作已有超时保障。

## 并发与资源生命周期

该实现使用 OS 线程而非异步运行时。三个 keeper 与一个 ready 监视线程共享 `Arc` 状态，并以 5ms sleep 轮询原子标志。所有原子操作使用 `SeqCst`，状态可见性直观，但轮询会产生固定唤醒开销。

资源顺序是：PD 创建 → StoreManager 创建 → worker 启动 → ready 发布 → 外部/内部取消 → 各 keeper 清理 → join → `OnExit` → `PdController::Close` / `StoreManager::Close`。scheduler 只有在 prepare 连接回调后才被移除；GC keeper 独立先行。主线程必须等所有 keeper 结束，因此在没有外部取消且没有错误时按设计长期阻塞。

`parity_test.rs::contract_resource_cleanup` 通过 hook 注入内存 PD/Store，等待 `OnAllReady` 后确认 `OnExit` 尚未发生，再取消上下文，并断言 ready 与 exit 回调均发生。该测试还清理全局 `DIAL_HOOKS`，避免跨测试污染。全局 hook 本身由单个 `Mutex` 保护，但依赖它的测试仍应串行或严格成对清理。

## 与 Go 版本的对应关系

Rust 文件逐函数对应 `br/pkg/task/operator/prepare_snap.go`：两版都有 PD/Store 创建、三项 keeper、连接建立后再暂停 scheduler、三项 ready 后回调和固定日志、取消后恢复资源的主结构。Rust CLI 与 Go CLI 也都把两个兼容命令名接到同一个入口。

关键差异必须保留在认知中：

- Go `dialPD` 使用真实 `pdutil.NewPdController`，Rust 无 hook 时必定失败；Go `StoreManager`、GC manager 和 `preparesnap.New` 是真实组件，Rust 对应项目前是本地桩。
- Go 使用 `errgroup.WithContext`，首错自动取消派生 context；Rust 通过共享原子取消和 `Mutex<Vec<Error>>` 模拟，且线程 panic 不会成为返回错误。
- Go 用 `WaitGroup` 等三项 ready；Rust 用原子计数轮询。Go 的 `SkipReadyHint` failpoint 在 Rust 目标实现中不存在。
- Go 清理包装器用 `context.WithTimeout(cfg.TTL)`；Rust 只保留 TTL 读取，没有真正的超时取消。
- Go prepare 环境包含 region cache、retry/split request env，并在退出时关闭 cache；Rust `Preparer` 桩没有这些资源。
- Rust 在空 `SafePointID` 时主动生成 ID；当前 Go 函数直接使用传入 ID。Rust 测试通过显式 `sp-test` 避开了这项差异。

## 扩展指南

接通真实集群时，最先应替换的是 `dialPD`、`createStoreManager` 及 `stubs.rs` 中 GC/Preparer 边界，而不是改弱主入口的失败策略。新增能力应继续保持“prepare 建连后才移除 scheduler”“三项保护成功后才发布 ready”“取消后先清理 keeper、再关闭管理器”三条不变量。

若增加第四个就绪组件，必须同步修改 `ready_expected`、启动/错误取消/join 逻辑以及 ready 测试；更稳妥的演进方向是让线程集合和期望数从同一处派生，避免计数漂移。若调整错误策略，要明确 cleanup 错误是否覆盖主错误，并为线程 panic 建立可观察的返回路径。

测试应放在独立文件，不嵌入 `prepare_snap.rs`。最接近的 crate 测试是 `br/pkg/task/operator/parity_test.rs`；应扩展其 hook 驱动用例以覆盖连接失败、最小 resolved TS 失败、scheduler 移除/undo 失败、prepare/Finalize 失败和并发取消。真实 PD/TiKV 行为应放到 `tests/realtikvtest/brietest/operator_test.rs`，但该文件当前调用的是 `tests/realtikvtest/brietest/harness.rs` 内的同名模拟函数；在把它作为目标实现的回归证据前，需要先完成真实 crate 接线。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/task/operator/prepare_snap.rs` 确认目标文件有 13 个符号；`explore`/`query`/`callers`/`callees` 确认主入口到三项 keeper 的调用边、`ReadyL` 的三个调用者、`contract_resource_cleanup` 测试调用者，以及 `force_flush.rs` 对 `dialPD`/`createStoreManager` 的复用。
- 目标源码：`br/pkg/task/operator/prepare_snap.rs`，核对全部 344 行、公开 API、线程、原子状态、清理和错误分支。
- crate 与入口：`br/pkg/task/operator/Cargo.toml`、`br/pkg/task/operator/lib.rs`、`br/cmd/br/operator.rs`。
- 直接配置/依赖：`br/pkg/task/operator/config.rs::PauseGcConfig` 与 `br/pkg/task/operator/stubs.rs` 中 `Context`、`PDClient`、`PdController`、`StoreManager`、`GCManager`、`Preparer`、`DIAL_HOOKS`。
- Go 对照：`br/pkg/task/operator/prepare_snap.go` 与 `br/cmd/br/operator.go`。
- 独立测试：`br/pkg/task/operator/parity_test.rs::contract_error_paths` 验证无 hook 的拨号失败；`contract_resource_cleanup` 验证 hook 路径、ready/exit 顺序、取消和默认 StoreManager 关闭。`tests/realtikvtest/brietest/operator_test.{go,rs}` 用于理解预期集群行为，但 Rust 版经 `harness.rs` 同名模拟层执行，不算本目标文件的直接集成覆盖。
- 结构校验按任务给定命令执行；本任务为纯文档分析，按计划不运行 Cargo。

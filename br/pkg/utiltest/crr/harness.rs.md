# `br/pkg/utiltest/crr/harness.rs`

## 文件定位

`harness.rs` 属于 Cargo crate `astersql-br-pkg-utiltest-crr`（见同目录 `Cargo.toml`），是 CRR（跨区域复制）本地测试设施的组合层。它不实现真实 CRR 协议，而是把 `PDSim`、`FlushSim`、`CRRWorker`、上下游 `Storage` 和 `CheckpointAdvancer` 接成可确定性驱动的测试环境。模块由 `lib.rs` 声明为 `pub mod harness`，并通过 `pub use harness::*` 暴露给 crate 使用者。

该文件位于 `br/pkg/utiltest`，服务于 BR 日志备份、复制和 checkpoint 行为验证，不在数据库 SQL 请求主链中。RustCodeGraph 的文件关系显示它被 `br/pkg/utiltest/crr/harness_test.rs`、`br/pkg/utiltest/crr/parity_test.rs` 和 `br/pkg/restore/log_client/log_file_manager.rs` 引用；其中可确认的直接运行用例位于前两个独立测试文件，后者属于索引记录的文件依赖，未在本次范围内确认存在对公开构造函数的直接调用。

## 核心职责

1. `NewLocalTestHarnessWithTestContext` / `newLocalTestHarness` 创建互相隔离的上游、下游本地目录和存储，并构造共享同一个 `PDSim` 的 flush 模拟器与 checkpoint advancer。
2. `new_event_channel`、`NewCRRUpstreamStorage` 与 `NewCRRWorker` 被组合成事件驱动复制链：对包装后上游的成功写入发出路径事件，worker 拉取事件后再从原始上游读取并写入下游。
3. `Tick`、`PullMessages`、`Replicate` 和 `UploadGlobalCheckpoint` 暴露离散、可由测试逐步控制的状态推进点，避免依赖不可控的完整后台系统。
4. `AssertDownstreamCanRestoreTo` 验证目标 TSO 不超过全局 checkpoint，且目标之前每个 flush 的 backupmeta 及其引用的 log 文件均已复制并可读取。
5. `Close` 与 `Drop` 关闭存储并删除临时根目录；构造中途失败也显式关闭已经创建的存储资源。

## 主要符号

- `pub struct TestHarness`：线束聚合对象。私有字段 `upstream_storage`、`downstream_storage` 保留未包装存储以执行关闭，`_base_dir` 保存清理目标；公开字段 `PDSim`、`FlushSim`、`CRRWorker`、`Upstream`、`Downstream`、`Advancer` 允许测试直接施加操作或观察状态。
- `pub fn NewLocalTestHarnessWithTestContext(ctx, tc, boundaries) -> Result<TestHarness>`：公开构造入口。用测试 seed、进程 ID 和进程内 `AtomicU64` 序号生成唯一临时目录，然后委托私有构造器。
- `fn newLocalTestHarness(...)`：真正的装配函数，按“目录→存储→PD→事件通道/worker/upstream→advancer→flush simulator”的顺序建立对象。
- `fn start_task_listener(advancer, env)`：同步调用 `StreamMeta::Begin` 取得已有任务事件；`EventAdd` 调用 `SetTask`，`EventPause`/`EventResume` 切换暂停状态，未知事件忽略。
- `TestHarness::Tick(&self, ctx) -> Result<u64>`：调用 `Advancer.OnTick()`，成功后返回 `PDSim.GlobalCheckpoint()`；当前 Rust 接口不向 `OnTick` 传递 `ctx`。
- `TestHarness::PullMessages(&mut self, limit) -> i32` 与 `Replicate(&mut self, ctx, limit) -> Result<i32>`：分别委托 `CRRWorker::PullMessages` 和 `ReplicateBuffered`。底层约定 `limit <= 0` 表示处理当前所有可用项，复制从缓冲尾部开始，即最新事件优先。
- `TestHarness::UploadGlobalCheckpoint(&self, ctx, checkpoint)`：以 `defaultTaskName` 调用 `PDSim` 的 `StreamMeta::UploadV3GlobalCheckpointForTask`；当前 Rust 实现不使用传入的 `ctx`。
- `TestHarness::AssertDownstreamCanRestoreTo(&self, ctx, tso)`：恢复可用性断言的核心入口。
- `TestHarness::Close` 与 `impl Drop for TestHarness`：显式和 RAII 两条清理路径；重复调用依赖 `Storage::Close` 与 `remove_dir_all` 的容忍性，目录删除错误被有意忽略。
- `assertReadableFile`、`extractDataFilePaths`、`parseBackupMetadata`：分别封装文件可读性错误、从 metadata 提取非空数据路径、用 `MetadataHelper` 解码 backupmeta。

## 执行流程

构造流程如下：

1. `NewLocalTestHarnessWithTestContext` 用 `tc.Seed()`、`process::id()` 和 `NEXT_HARNESS_ID.fetch_add(Relaxed)` 生成 `crr-harness-<seed>-<pid>-<id>` 根目录。
2. `newLocalTestHarness` 创建 `upstream/`、`downstream/` 子目录，并分别构造 `LocalStorage`。上游先创建；后续下游或 PD 创建失败时关闭已成功创建的存储。
3. `NewPDSimWithTestContext` 将 `boundaries` 物化为 fake cluster 的 region/store 布局，并绑定默认任务名。
4. `new_event_channel` 建立有界同步通道；`CRRUpstreamStorage` 持有发送端，`CRRWorker` 持有接收端及原始上下游存储。`FlushSim` 被刻意配置为向包装后的 `Upstream` 写入，因此 log 和 backupmeta 写成功后都会产生复制事件。
5. `NewCommandCheckpointAdvancer(pd.clone())` 创建推进器；`start_task_listener` 先同步注入 `Begin` 返回的现有任务状态，再调用 `SpawnSubscriptionHandler` 处理后续订阅事件。
6. 测试通常调用 `FlushSim.FlushStore` 生成 log 和 backupmeta，调用 `PullMessages` 将路径事件移入 worker 缓冲，再调用 `Replicate` 把文件写到下游。之后可用 `UploadGlobalCheckpoint` 直接设置任务全局点，或用 `Tick` 让 advancer 计算并推进。
7. `AssertDownstreamCanRestoreTo` 先拒绝高于当前全局 checkpoint 的目标；随后遍历 `FlushSim.RecordsUpTo(tso)`，检查每个 metadata 文件可读、文件名解析出的 `FlushTS` 与记录一致、metadata 可解码，并检查 `FileGroups` 中每个非空 log 路径在下游可读。

## 数据与状态

- 目录状态：每个 harness 拥有独立的 `_base_dir`，其下固定分为 `upstream` 和 `downstream`。`harness_test.rs::local_harnesses_with_the_same_seed_use_independent_temp_dirs` 证明同 seed 的两个实例 URI 不同，关闭一个实例不会删除另一个实例的存储。
- 共享所有权：存储、`PDSim` 和包装上游使用 `Arc`。原始上游同时交给 worker 读取，包装上游交给 `FlushSim` 和测试写入；下游同时由 worker 写入和公开 `Downstream` 读取。
- 事件状态：worker 的接收端和本地 `buffer` 位于 `CRRWorker`；`PullMessages` 非阻塞拉取，跳过空路径，通道断开后释放 receiver。`ReplicateBuffered` 只在复制成功后截掉相应尾部事件，因此错误会保留尚未成功复制的事件。
- checkpoint 状态：`PDSim` 内部用互斥锁保护任务名与 `global_checkpoint`，上传禁止回滚；`Advancer` 通过 `StreamMeta` 读取 region/task 状态并更新同一个模拟 PD。
- flush 记录：`FlushSim` 内部保存按 sequence 排序的 `FlushRecord`；`RecordsUpTo(tso)` 决定恢复断言检查哪些 metadata。文件名中的 flush TSO 还会由 `ParseName` 反向验证。
- metadata 路径规则：`extractDataFilePaths` 优先使用 `DataFileGroup.Path`；仅当该字段为空时才遍历 `DataFilesInfo[*].Path`，并过滤空路径。这与 `parity_test.rs` 验证的 Go wire shape 一致。

## 依赖与调用关系

上游调用者主要是 crate 的独立测试：

- `harness_test.rs` 调用 `NewLocalTestHarnessWithTestContext` 和 `Close`，验证临时目录隔离及资源生命周期。
- `parity_test.rs::go_rust_public_contract_matches` 通过公开构造函数执行 flush、pull、replicate、checkpoint 上传和恢复断言的端到端路径，并验证 checkpoint 落后目标时失败。
- `lib.rs` 将本模块公开再导出，使调用者可直接从 crate 根使用这些符号。

主要下游依赖为：

- `crate::pd_sim::{PDSim, NewPDSimWithTestContext}`：region/TSO/checkpoint 状态与 `StreamMeta` 环境。
- `crate::flush_sim::{FlushSim, NewFlushSimWithTestContext}`：生成 flush 记录、log 与 backupmeta。
- `crate::crr_sim::{CRRWorker, CRRUpstreamStorage, new_event_channel, ...}`：写后发事件、拉取及复制。
- `astersql_br_pkg_streamhelper::{CheckpointAdvancer, StreamMeta, EventType, ...}`：任务监听与 checkpoint 推进。
- `astersql_br_pkg_stream::MetadataHelper`、`astersql_br_pkg_stream_backupmetas::ParseName`：metadata 内容及文件名解析。
- `crate::stubs::{LocalStorage, Storage, Context, Error}`：本地文件存储和轻量错误/上下文抽象。

`Cargo.toml` 表明该 crate 只依赖已移植的 slim BR crates、fakecluster、`rand` 与 serde；注释明确此路径不依赖真实 kv/domain/kvproto/grpcio/objstore，因此该线束只能证明本地替身之间的契约，不能替代真实 TiKV 或对象存储集成测试。

## 错误处理与边界

- 目录创建和 `LocalStorage::new` 错误附带具体路径；下游创建失败会先关闭上游，PD 创建失败会关闭上下游。
- `start_task_listener` 对 `Begin` 错误静默返回，保持 Go 的宽松启动行为；这意味着构造可能成功但 advancer 没有初始任务。新增严格启动需求时必须同时调整 Go 对照与测试，不能只在 Rust 侧改变语义。
- `Tick` 将 advancer 的字符串错误映射到本地 `Error`；`UploadGlobalCheckpoint` 在错误中加入 checkpoint 数值。PDSim 还会拒绝未知任务和 checkpoint 回滚。
- `AssertDownstreamCanRestoreTo` 的首要不变量是 `GlobalCheckpoint >= tso`。之后任何缺失 metadata、非法 metadata 文件名、文件名 `FlushTS` 不匹配、解码失败或被引用 log 不可读都会立即返回带路径上下文的错误。
- metadata 中空 `Path` 不会成为读取目标；group path 非空时不会再遍历其 `DataFilesInfo`。修改 wire shape 时要保留这项优先级兼容性。
- `Close` 不返回错误，且忽略目录删除失败；它适合作为测试清理，不适合作为需要确认持久化资源彻底释放的生产 API。
- `ctx` 在构造函数、`Tick`、`UploadGlobalCheckpoint` 中部分或全部未使用；取消语义只在实际接收 `Context` 的存储/worker 操作中生效，不能假定所有线束操作都可取消。

## 并发与资源生命周期

`NEXT_HARNESS_ID` 使用 `AtomicU64` 的 `Relaxed` 排序；它只负责同进程命名去重，不承担跨线程状态同步，因此无需更强内存序。进程 ID 加原子序号使同 seed 并发或连续创建的 harness 不共享根目录。

复制链使用 `std::sync::mpsc::sync_channel`（由 `new_event_channel` 创建）提供有界背压。包装上游写成功后发送事件；通道满时 `crr_sim.rs::emitNewVersionEvent` 短睡重试，直到成功、上下文取消或接收端断开。harness 本身不创建复制后台线程，`PullMessages` 和 `Replicate` 需由调用者显式驱动，因而测试执行顺序可控。

`CheckpointAdvancer::SpawnSubscriptionHandler` 启动任务订阅处理；本文件没有保存或显式 join 该处理器的句柄，生命周期由 advancer 实现负责。构造阶段先同步 `start_task_listener` 再启动订阅，避免初始任务尚未注入时推进。

`Close` 先关闭两个原始存储，再递归删除根目录；`Drop` 再次调用 `Close`，保证提前返回或 panic 展开时尽可能清理。公开 `Arc` 克隆若在 harness 销毁后仍被外部持有，能否继续操作由具体 `Storage::Close` 实现决定，调用者不应依赖这种用法。

## 与 Go 版本的对应关系

Rust 文件直接对应 `br/pkg/utiltest/crr/harness.go`，保留了 `TestHarness` 聚合字段、构造顺序、worker 委托方法、checkpoint 上传、metadata/log 恢复断言和 Close 时关闭存储的主体逻辑。错误文本也尽量保留 Go 中的操作与路径上下文。

已确认的差异如下：

- Go 用 `tc.T.TempDir()` 获得测试框架管理的目录，并通过 `tc.T.Cleanup(h.Close)` 注册清理；Rust 用系统临时目录加 seed/PID/原子 ID，自身保存 `_base_dir`，由 `Close`/`Drop` 删除。
- Go `TestHarness` 保存 `tc`；Rust 不保存，因为构造 `FlushSim` 后不再需要该引用。
- Go 直接调用 `advancer.StartTaskListener(ctx)`；Rust 以本地 `start_task_listener` 同步复刻 `Begin` 后的 Add/Pause/Resume 处理，再调用无 context 参数的 `SpawnSubscriptionHandler`。
- Go 的 `Tick`/上传 API 把 `context.Context` 继续传给下游；Rust 对应底层签名不接收 context，因此形参命名为 `_ctx`。
- Go `Close` 只关闭存储，临时目录由 Go 测试框架负责；Rust `Close` 还删除整个根目录，并有 `Drop` 兜底。

这些差异属于当前 Rust 测试替身和运行时接口的适配，不应被描述为真实 BR 生产实现已完全移植。

## 扩展指南

- 新增一个线束组件时，应在 `newLocalTestHarness` 中按依赖顺序构造，并为每个可能失败的步骤补齐已创建资源的释放路径；公开字段是否必要应由测试是否需要直接驱动决定。
- 扩展复制流程时，优先修改 `crr_sim.rs` 的 worker/upstream 行为，harness 只负责接线；同步更新独立的 `parity_test.rs`，不要把测试内嵌回 `harness.rs`。
- 改动恢复可用性判定时，入口是 `AssertDownstreamCanRestoreTo`，辅助逻辑分别在 `assertReadableFile`、`extractDataFilePaths`、`parseBackupMetadata`。必须同步核对 Go `harness.go` 及 metadata wire shape，特别是 `DataFileGroup.Path` 优先级和文件名 `FlushTS` 校验。
- 改动 task 监听时，需同时核对 `StreamMeta::Begin` 事件、`EventType` 分支和 `CheckpointAdvancer` 的订阅生命周期，并增加独立测试覆盖 Begin 失败、Pause/Resume、重复 Add 等边界。
- 改动临时目录或清理策略时，应扩展 `harness_test.rs`，至少保持同 seed 多实例隔离、关闭一个不影响另一个、显式 Close 与 Drop 均可安全清理。
- 若要宣称真实存储兼容，需另加真实对象存储或 TiKV 集成验证；当前 crate 的本地 `stubs::Storage` 与 Cargo 依赖边界不足以证明该结论。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust 文件；`files --filter br/pkg/utiltest/crr` 确认该模块文件集合；`node --file br/pkg/utiltest/crr/harness.rs` 读取完整 325 行源码并报告三个引用文件；`node --symbols-only` 列出 `TestHarness`、三个自由函数、六个公开方法、`Drop` 和三个辅助函数。精确 `callers`/`callees` 查询未返回可用结果，因此调用结论另由实际调用点核验。
- 目标源码：`br/pkg/utiltest/crr/harness.rs`，核对构造、task listener、委托方法、恢复断言和清理逻辑。
- crate 边界：`br/pkg/utiltest/crr/Cargo.toml` 与 `br/pkg/utiltest/crr/lib.rs`，核对包名、依赖、模块声明、再导出及独立测试模块。
- 直接实现依赖：`br/pkg/utiltest/crr/crr_sim.rs`、`flush_sim.rs`、`pd_sim.rs`，核对事件缓冲/最新优先复制、flush 记录、checkpoint 锁与上传约束。
- Go 对照：`br/pkg/utiltest/crr/harness.go`，核对 API、装配顺序、错误语义、metadata 路径提取和生命周期差异。
- 独立 Rust 测试：`br/pkg/utiltest/crr/harness_test.rs` 验证目录隔离；`br/pkg/utiltest/crr/parity_test.rs::go_rust_public_contract_matches` 验证 flush→pull→replicate→upload→restore 正常链及 checkpoint 落后错误。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构校验要求本文恰好具有以上 11 个固定二级章节。

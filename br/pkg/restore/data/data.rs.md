# `br/pkg/restore/data/data.rs`

## 文件定位

`data.rs` 是 `astersql-br-pkg-restore-data` crate 中 TiKV 数据恢复流程的 Rust 移植实现，由同目录 `lib.rs` 以 `pub mod data` 装配并通过 `pub use data::*` 重导出。`br/pkg/restore/data/Cargo.toml` 将它声明为 library crate，`package.metadata.porting.go-package` 指向 Go 包 `br/pkg/restore/data`；根 `Cargo.toml` 将该 crate 列为 workspace 成员。

当前 Rust 侧的边界必须如实理解：该 crate 的 `[dependencies]` 为空，源文件通过同 crate 的 `stubs.rs` 获得 `Mgr`、`RecoverDataClient`、`FlashbackRpc`、`WorkerPool` 等接口或本地模型。仓库 Rust 搜索中，`RecoverData` 只由本 crate 的 `parity_test.rs` 引用，未找到 Rust BR 命令主链的生产调用者。因此它已承载恢复算法和可测试契约，但不能仅凭当前代码声称已接管 Go BR 的实际生产恢复路径。

## 核心职责

本文件把数据恢复组织为一个带阶段语义的六步流程（`RecoverData` / `doRecoveryData`）：从所有 TiKV 收集 region meta，生成按 store 分组的恢复计划，把已观测到的最大 ID 回写给 PD，向各 TiKV 发送恢复指令，准备 flashback，最后执行 flashback。

它同时负责四类跨阶段契约：

- 用 `RecoveryStage`、`recoveryError`、`stage_err`、`atStage` 把错误与失败阶段关联，供 `isRetryErr` 判定整体流程是否可重试。
- 用 `Recovery`、`StoreMeta` 保存一次恢复的输入、中间元数据、恢复计划、最大分配 ID 和运行时依赖。
- 用 `ErrorGroup`、`WorkerPool`、`Arc<Mutex<_>>`、`Condvar` 协调并发收集和下发，并用 `CancelOnDrop`、`ConnCloser` 固定取消与连接关闭责任。
- 用 `SpawnTiKVShutDownWatchers` 观测恢复期间的 store 重启，对已有计划的 store 重新发送恢复请求。

## 主要符号

- `RecoveryStage`：稳定的整数阶段码，从 `StageUnknown = 0` 到 `StageFlashback = 5`。`String` / `Display` 提供与 Go 版对齐的日志文案。
- `recoveryError { error, atStage }`：内部阶段错误包装。转为 `stubs::Error` 时保留原错误文本，并把阶段写入 `Error.stage`。
- `atStage(&Error)` / `isRetryErr(&Error)`：前者将 `Error.stage` 映射回枚举；后者允许元数据收集、计划、PD ID 重置和 region 恢复阶段重试，明确禁止 flashback 失败与未知错误重试。
- `RecoverData(...) -> Result<i32>`：公开总入口，用 `WithRetryV2` 和 `NewRecoveryBackoffStrategy` 粗粒度重跑 `doRecoveryData`；成功值是去重后的 region 数。
- `StoreMeta { StoreId, RegionMetas }` / `NewStoreMeta`：某一 store 回报的 region peer 元数据集合。
- `Recovery`：一次恢复的可变状态聚合。`allStores`、`StoreMetas`、`RecoveryPlan`、`MaxAllocID` 是算法状态；`mgr`、`progress`、`concurrency` 是运行依赖；`watcher_tick`、`spawn_watcher` 是 Rust 测试可控钩子。
- `ReadRegionMeta`：为每个 store 建立注入的 recovery client，读取流直到精确 EOF，再把 `StoreMeta` 通过共享队列交回聚合端。
- `GetTotalRegions`：按 `RegionId` 去重，不把同一 region 在多 store 的 peer 重复计数。
- `MakeRecoveryPlan`：聚合 peer，更新 `MaxAllocID`，调用 `SortRecoverRegions`、`CheckConsistencyAndValidPeer`、`LeaderCandidates`、`SelectRegionLeader`，生成 `store_id -> RecoverRegionRequest[]` 计划。
- `RecoverRegionOfStore` / `RecoverRegions`：前者向单个 store 流式发送计划并 `CloseAndRecv`；后者只对 `RecoveryPlan` 中的 store 并发执行前者。
- `SpawnTiKVShutDownWatchers`：在独立线程中调用 `StoreWatcher::Step`，维护重启 store ID 集合并重放该 store 的计划。
- `PrepareFlashbackToVersion` / `FlashbackToVersion`：通过 `Mgr::GetFlashback()` 得到 RPC 边界，对空起止键表示的全键空间执行 prepare 和 flashback。
- `CancelOnDrop` / `ConnCloser`：RAII 守卫，分别在离开作用域时取消子 context 和关闭连接。
- `getStoreAddress`：按 ID 线性扫描 store 列表；未找到时记录错误但返回空字符串，而不是直接返回 `Result`。

## 执行流程

1. `RecoverData` 先建立恢复退避策略，将 `doRecoveryData` 交给 `WithRetryV2`。每次尝试都 clone store 列表与 `Arc` 依赖，因而重试会重建整个 `Recovery` 并重跑前置阶段。
2. `doRecoveryData` 创建子 context 及 `CancelOnDrop`，然后 `NewRecovery` 预分配与 store 数相同的 `StoreMetas`，并初始化空计划。
3. `ReadRegionMeta` 以 `min(store_count, MaxStoreConcurrency)` 为工作池/错误组上限。每个任务从 `Mgr::ClientFactory` 创建 client 和 connection，按 store ID 读流，把完整 `StoreMeta` 推入 `VecDeque`并通知条件变量。聚合端取满 store 数或观察到 context 结束，每轮推进 progress，最终由 `ErrorGroup::Wait` 返回工作任务错误。
4. `GetTotalRegions` 保存当次结果计数。`MakeRecoveryPlan` 把所有 peer 按 region ID 聚合，将 store ID、region ID、peer ID 的最大值累计到 `MaxAllocID`；再对键空间和 peer 一致性进行检查。无效 region 的每个 peer 收到 tombstone 请求；有效 region 从 leader candidates 中按 store 已选 leader 计数做平衡选择，只向选中 store 加入 `AsLeader` 请求。
5. `Mgr::RecoverBaseAllocID` 在恢复 region 前提升 PD 分配基线，避免新 ID 与恢复出的旧 ID 冲突。
6. `SpawnTiKVShutDownWatchers` 开始后台观测；`RecoverRegions` 随后按 store 并发发送计划。`RecoverRegionOfStore` 该路径在所有请求发送成功且收到关闭回复后才推进 progress。
7. `PrepareFlashbackToVersion(ctx, resolveTS, restoreTS.wrapping_sub(1))` 带 flashback 专用退避执行 prepare；随后 `FlashbackToVersion(ctx, resolveTS, restoreTS)` 把 `commitTS.wrapping_sub(1)` 作为起始版本边界下发。两者均成功后才推进对应 progress（prepare 则无论成败都在退避返回前调用一次 `Inc`）。

## 数据与状态

`StoreMetas` 是按“收到的队列顺序”写入预分配槽位，不依赖 `allStores` 原顺序；真实归属始终由每项 `StoreMeta.StoreId` 表达。`GetTotalRegions` 只按 `RegionId` 去重。

`RecoveryPlan` 只包含需要执行动作的 store：有效 region 通常只为选中 leader 的 store 增加请求，无效 region 则为其所有 peer 所在 store 增加 tombstone 请求。所以计划 map 的 store 数小于在线 store 数是合法状态；`data_test.rs` 用 3 个 store 验证计划仅含 2 个 store。

`MaxAllocID` 不只查 region/peer ID：每个 `StoreMeta` 先以 `StoreId` 作为局部上界，再与所有 `RegionId` / `PeerId` 比较，最后汇总全局最大值。

`RecoveryStage` 整数值被写入通用 `Error.stage`，这是 Rust 对 Go `errors.As(recoveryError)` 的本地替代。纯错误文本中即使出现阶段名也不会被识别；`parity_test.rs::recovery_stage_is_not_inferred_from_plain_error_text` 锁定了这一点。

## 依赖与调用关系

模块内部主调用链由 RustCodeGraph 与源码共同确认为：

`RecoverData -> doRecoveryData -> ReadRegionMeta -> MakeRecoveryPlan -> Mgr::RecoverBaseAllocID -> SpawnTiKVShutDownWatchers -> RecoverRegions -> PrepareFlashbackToVersion -> FlashbackToVersion`。

关键下游分为三组：

- 计划算法来自同 crate `recover.rs`：`SortRecoverRegions`、`CheckConsistencyAndValidPeer`、`LeaderCandidates`、`SelectRegionLeader`。
- 运行时边界来自 `stubs.rs`：`Mgr` 提供 PD、client factory 和 flashback 能力；`RecoverDataClient` 提供元数据读流与 region 恢复写流；`Progress` 承接进度；`Context`、`ErrorGroup`、`WorkerPool`、`StoreWatcher` 承接取消和并发。
- flashback 路径通过 `FlashbackRpc` trait 动态派发；`_flashback_bound` 只用于在编译期保留 trait 边界的引用，不参与运行流程。

上游 Rust 侧目前只有 `data_test.rs` / `parity_test.rs` 直接构造 `Recovery` 或调用 `RecoverData`；与之相对，Go 的 `data.go` 是 BR 包中的真实恢复实现，依赖 `conn.Mgr`、kvproto gRPC、`rangetask`、DDL flashback RPC 和 store watcher。

## 错误处理与边界

- `doRecoveryData` 在每个主阶段立即返回，并用 `stage_err` 标注失败点。收集、计划、PD ID 和 region 恢复失败可触发外层整体重试；flashback 阶段不重试，以避免重复执行高风险数据变换。
- `ReadRegionMeta` 只把 `is_eof` 识别的哨兵错误当作正常流结束，其他 `Recv` 错误使用 `Error::Trace` 传播。`parity_test.rs::eof_detection_does_not_use_substring_matching` 确认包含“EOF”文本的无关错误不得被误判。
- `Mgr::ClientFactory()` 未配置时，读取和恢复建连都显式返回错误。`getStoreAddress` 却在未知 store 时只记录日志并返回空串；实际失败会延后到 client factory，扩展时不能忽略这一契约。
- `MakeRecoveryPlan` 会向上传播键空间不连续、peer 不一致或有效 region 没有 leader candidate 等错误。无效/tombstone region 不是计划错误，而是转为 tombstone 请求。
- `restoreTS.wrapping_sub(1)` 和 `commitTS.wrapping_sub(1)` 显式模拟 Go `uint64` 减法的回绕；当时间戳为 0 时结果是 `u64::MAX`，`parity_test.rs::zero_commit_ts_uses_wrapping_previous_version` 已固定该边界。
- `run_on_full_range` 当前只构造一个空起止键的全范围 `KeyRange`并调用 handler 一次，且忽略 `_concurrency`。它不等价于 Go `rangetask.NewRangeTaskRunner` 的 region 分片调度与并发执行；在真实存储接线前必须专门验证这一差异。

## 并发与资源生命周期

`ReadRegionMeta` 和 `RecoverRegions` 都以 `ErrorGroup::WithContext` 将首个任务错误与组取消关联，并把任务数限制为 store 数与 `MaxStoreConcurrency` 的较小值。`ReadRegionMeta` 的生产者通过 `Arc<Mutex<VecDeque<StoreMeta>>>` 传递结果，`Condvar` 用于唤醒消费者；消费者每 50 ms 超时检查取消状态，避免永久阻塞在空队列。

`RecoverRegions` 为跨线程移动而 clone 计划和 `Arc` 依赖，在每个任务中构造仅保留必要字段的临时 `Recovery`，避免跨线程共享可变 `self`。这些临时实例的 `StoreMetas` / `RecoveryPlan` 为空，只用于调用 `RecoverRegionOfStore`。

`ConnCloser` 覆盖元数据读取和 region 恢复两类连接，无论正常返回还是 `?` 提前退出都调用 `Close`。`parity_test.rs::contract_resource_cleanup_on_recover_data` 通过计数连接守卫验证了 3 个读取连接加至少 2 个恢复连接均被关闭。`CancelOnDrop` 则确保 `doRecoveryData` 所有返回路径都取消子 context，不反向取消父 context。

watcher 线程持有 clone 后的 context、计划和 manager，以 `watcher_tick` 周期睡眠。它只在两次取消检查之间可能最多延迟一个 tick 退出；`doRecoveryData` 结束时 `CancelOnDrop` 会使其最终终止。`spawn_watcher = false` 与短 `watcher_tick` 仅是 Rust 测试钩子，Go `Recovery` 没有这两个字段。

## 与 Go 版本的对应关系

Rust 的类型、方法名、六步顺序、阶段重试分类、`MaxAllocID` 计算、leader/tombstone 计划、store 重启后重放、以及 flashback 的 `commitTS-1` 边界均直接对应 `br/pkg/restore/data/data.go`。`data_test.rs` 复制 Go `data_test.go` 的三 store/三 region 数据，同样断言 3 个唯一 region、`MaxAllocID == 0x176f` 和 2 个计划 store。

实现上的重要差异为：

- Go `newRecoveryClient` 直接配置 gRPC backoff、TLS 和 keepalive；Rust 从 `Mgr::ClientFactory` 获取 trait object 客户端和连接，具体传输由注入者负责。
- Go 用 channel + `errgroup` 收集元数据；Rust 用 `Mutex<VecDeque<_>>` + `Condvar` + 定时取消检查表达同类流程。
- Go 用 goroutine/ticker 运行 watcher；Rust 用 `std::thread::spawn` / `sleep`。Rust 的 `rebootStores` 由 `Mutex<HashSet<_>>` 保护，而 Go map 由 watcher 同一 goroutine 消费。
- Go flashback 通过 TiKV `rangetask` 对 region 分片并应用 `concurrency`；Rust `run_on_full_range` 目前是单次全范围 trait 调用，不消费并发参数。
- Go 用具体的 `conn.Mgr`、PD 客户端、kvproto 和 TiKV storage；Rust crate 目前依赖为空，同名边界来自本地 stubs。这是迁移状态差异，不是可忽略的语法改写。

## 扩展指南

- 改变恢复顺序或新增阶段时，必须同步更新 `RecoveryStage`、`String`、`atStage`、`isRetryErr`、`doRecoveryData` 的包装点，并在独立 `data_test.rs` / `parity_test.rs` 中增加可重试与不可重试回归。不要把 Rust 单元测试内嵌回 `data.rs`。
- 修改计划时以 `MakeRecoveryPlan` 为接入点，同时审查 `recover.rs` 的排序、一致性、candidate 和选主契约。必须保留 `MaxAllocID` 的 store/region/peer 三类 ID 上界，并验证计划不会对无需操作的 store 生成空任务。
- 接入真实 gRPC/PD/TiKV 时，应优先实现 `stubs.rs` 已定义的 trait 边界，不要在本文件内复制外部客户端。任何新外部 Rust 依赖都需遵守仓库的上游移植、tagged Git 依赖与可复现性要求。
- 将 flashback 扩展为真实 region 调度时，最可能修改 `run_on_full_range`、`PrepareFlashbackToVersion`、`FlashbackToVersion` 和 `FlashbackRpc`。应为范围分片、并发上限、部分完成计数、取消及重试边界增加独立测试，并与 Go `rangetask` 行为对照。
- 改动并发代码时，保留 `ConnCloser` / `CancelOnDrop` 的异常路径释放保证，并在 `parity_test.rs` 中继续断言连接关闭次数、取消传播方向和进度推进。watcher 的 tick 与启停钩子只用于可测试性，不应改变默认 30 秒生产语义。
- 任何语义改动都应对照 `data.go` 和 `data_test.go`；若有意与 Go 分歧，需在文档和 parity 测试中明确记录可观测差异，而不是为了让测试通过而简化流程。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 7,032 个 Rust 文件；`files --filter br/pkg/restore/data` 确认该目录的 Rust/Go 源文件、测试和模块边界。
- RustCodeGraph `node --file br/pkg/restore/data/data.rs` 用于阅读目标文件全部 774 行与 32 个符号；`explore` 确认 `RecoverData -> doRecoveryData`、`doRecoveryData -> ReadRegionMeta/MakeRecoveryPlan/RecoverRegions/PrepareFlashbackToVersion/FlashbackToVersion` 以及 `RecoverRegionOfStore` 的主要内部调用边。精确 `callers/callees` 对部分 Go 风格 Rust `impl` 方法未返回可解析结果，本文档对这些边以文件内源码复核，没有推测外部调用者。
- crate 与装配证据：`br/pkg/restore/data/Cargo.toml`、`br/pkg/restore/data/lib.rs`、根 `Cargo.toml`。这些文件确认 library crate、空依赖集、Go 包映射、workspace 成员身份和 `data::*` 重导出。
- Go 对照：`br/pkg/restore/data/data.go`（六步顺序、gRPC、errgroup、watcher、rangetask、计划算法）与 `br/pkg/restore/data/data_test.go`（唯一 region 数、最大 ID、计划 store 数）。
- Rust 测试证据：`br/pkg/restore/data/data_test.rs` 验证 Go 基础用例和错误文本/阶段保留；`br/pkg/restore/data/parity_test.rs` 验证退避重试、`uint64` 回绕、子 context 取消、EOF 哨兵、一致性检查、恢复计划、连接关闭、RPC 调用、进度和取消错误。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证仅要求目标文档存在且上述固定二级章节恰好为 11 个。

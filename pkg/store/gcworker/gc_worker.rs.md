# `pkg/store/gcworker/gc_worker.rs`

## 文件定位

本文件是 `astersql-store-gcworker` crate 的核心实现，crate 入口 `pkg/store/gcworker/lib.rs` 以 `pub mod gc_worker` 导出它。它把 TiDB/AsterSQL 侧 GC 的调度策略与外部设施隔离开：`GCWorker` 实现领导者竞选、周期调度、安全点计算、锁解析和 delete-range 编排，`GCWorkerRuntime`/`GCSession` 则把 PD、Oracle、系统表、TiKV RPC、DDL delete-range 表和外部工作负载控制器抽象成可替换边界。

`pkg/store/gcworker/Cargo.toml` 声明 crate 名为 `astersql-store-gcworker`，并用 `[package.metadata.porting].go-package = "pkg/store/gcworker"` 标明对应 Go 包。当前 Rust 文件不是薄门面：后台线程、状态机、并发删除、格式兼容和主要公共 API 都在这里；具体生产运行时如何实现两个 trait 不在本文件中。仓库顶层 `pkg/lib.rs` 通过 `facade_store_gcworker` 再导出该 crate，但当前 Rust 仓库内除本 crate 的 `lib.rs` 和独立测试外，没有检索到构造 `GCWorker` 的生产调用点，因此“已由完整服务器启动链实际接线”不能从本文件证实。

## 核心职责

1. **单领导者调度**：`Start` 启动 tick 线程，`checkLeader` 通过系统表中的 UUID、描述和租约选主或续租，只有领导者进入 `leaderTick`。
2. **安全点准备**：`prepare` 在事务中检查 `tikv_gc_enable`、运行间隔和 lifetime；`calcNewTxnSafePoint` 计算 `now - lifetime`，再调用 `AdvanceTxnSafePoint`，成功后持久化本轮开始时间与安全点时间。
3. **执行 GC 阶段链**：`runGCJob` 按“同步等待 → `resolveLocks` → `deleteRanges` → `redoDeleteRanges` → `broadcastGCSafePoint` → `notifyGCV2AfterGC`”执行。只有广播成功后才通知外部 GCV2 控制器。
4. **适配多种存储/keyspace 模式**：RaftKV v2 使用 `DeleteRangeRaftV2`，其他引擎对所有适用 Store 发 `UnsafeDestroyRange`；null keyspace 会分页枚举 keyspace 并跳过禁用或 keyspace-level-GC 的空间；unified GC 调度分支只做 delete-range/redo，安全点由外部流程推进。
5. **维护兼容配置和观测**：负责系统表默认值、状态变量映射、Go 格式的时间/时长编码，并通过 `RecordEvent`/`RecordFailure` 记录阶段结果。

## 主要符号

- `GCError`、`GCResult<T>`：字符串消息型统一错误及结果别名；本文件的 trait 边界和编排函数均使用它们。
- `KeyRange`、`DelRangeTask`：半开键区间和带 job/element ID 的 delete-range 任务；`LabelRule` 用区间匹配 placement/label 规则。
- `StoreInfo`/`StoreState`、`KeyspaceInfo`、`SafePointAdvance`：分别承载 Store 过滤、keyspace 枚举和安全点推进结果。
- `GCSession`：最小系统表会话接口。`SessionGuard` 在 `Drop` 时回滚仍活跃的事务并关闭会话，避免异常路径泄漏事务或会话。
- `GCWorkerRuntime`：文件最重要的依赖倒置边界，覆盖时钟/TSO、PD 安全点、Store 和 keyspace 查询、锁解析、区间删除、规则清理、指标事件以及可选 `ExternalWorkloadManager`。
- `CancellationToken`：基于 `AtomicBool + Condvar` 的协作取消对象；`Cancel` 唤醒等待者，`Wait` 同时承担可取消睡眠，`Check` 把取消转为 `GCError`。
- `gcConcurrency { v, isAuto }`：区分固定并发和自动并发。`getGCConcurrency` 自动模式优先取可用 Store 数，固定模式从系统表读值并钳制到 `[1, 128]`。
- `WorkerState`：互斥保护 `gc_is_running`、`last_finish`、`has_finished_first_gc_job`，用于防止作业重入及限制调度频率。
- `GCWorker`：公开字段是 `uuid`、`desc`、`keyspaceID`；私有字段保存 runtime、状态、取消令牌和后台线程句柄。
- `NewGCWorker`：校验存储为 TiKV，以当前版本生成 UUID，以 host/PID/启动时间生成描述并注册统计。
- `Start`/`Close`/`Drop`：管理后台 tick 线程；`Start` 幂等并等待线程完成首次 tick，`Close` 取消并 join，最后一个共享 handle 的 `GCWorker` 被丢弃时自动关闭。
- `RunGCJob`、`RunDistributedGCJob`、`RunResolveLocks`：无需启动周期 worker 的公开入口。当前 `RunGCJob` 直接委托 `RunDistributedGCJob`；后者推进 txn safe point、等待缓存同步、解析锁、广播 GC safe point；`RunResolveLocks` 只解析锁。
- `MockGCWorker`/`NewMockGCWorker`：供外部测试复用真实 `deleteRanges` 路径，而不是另写一套删除算法。
- `format_system_time`/`parse_system_time`、`format_duration`/`parse_duration`：系统表线格式转换。时间写成 Go `GCTimeFormat` 的 UTC 文本，同时兼容旧 Rust 端 Unix 毫秒数字；duration 接受 Go 风格复合和小数单位。

## 执行流程

后台路径从 `Start` 开始。它只在 `handle` 为空时 spawn `start`，并通过零容量 channel 等待后台线程执行一次 `tick` 后才返回。`start` 每 60 秒调度一次，同时轮询 GC 作业完成 channel；收到结果后清除 `gc_is_running`、更新完成时间和“首个作业已完成”标志，失败则记为 `run_gc_job`。

一次 `tick` 先调用 `checkLeader`。若当前 UUID 已是 leader，则在同一事务内续租 120 秒；否则回滚第一次读取事务，再开启事务检查租约，只有缺失或过期才写入 UUID、描述和新租约并提交。非 leader 只记录 `not_leader`，竞选错误记录 `check_leader`。

领导者进入 `leaderTick`：已有作业运行时直接返回；先取得并发配置。若 `IsUnifiedGC()`，`runKeyspaceGCJobInUnifiedGCMode` 检查等待窗口和运行间隔，异步执行 `runKeyspaceDeleteRange`，随后保存 last-run 时间。普通模式调用 `prepare`；只有启用、已到运行间隔、事务安全点实际前进且不处于等待窗口时，才设置运行标志并 spawn `runGCJob`。

`prepare` 的事务不变量是：仅 `checkPrepare` 返回 `Some(safe_point)` 时提交；禁用、未到间隔、无法推进或任意错误都回滚。`checkPrepare` 将缺失配置补默认值，lifetime 小于 10 分钟时改写为下限；目标 TSO 是 Oracle 当前时间减 lifetime。`ErrDecreasingTxnSafePoint` 和没有前进都使本轮安静跳过。

`runGCJob` 先可取消地等待 `txnSafePointSyncWaitTime`，然后：

1. `resolveLocks(safe_point, concurrency.v)` 用 `safe_point.wrapping_sub(1)` 作为最大锁版本。非 null keyspace 处理默认全区间；null keyspace 先处理 `NullKeyspaceRanges`，再以 50 条一批枚举 keyspace，跳过 disabled 和 `keyspace_level_gc` 项。各区间尽量继续，任一失败最终汇总为“不完全成功”。
2. `deleteRanges` 加载 safe point 前的任务。RaftKV v2 走专用 API；否则 `doUnsafeDestroyRangeRequest` 并发请求所有适用 TiKV Store。数据删除成功后依次清理 placement 规则、label 规则并标记任务完成。
3. `redoDeleteRanges` 取 24 小时延迟窗口之前的完成记录，再次发 UnsafeDestroyRange，成功后删除 done record。
4. `broadcastGCSafePoint` 调用 `AdvanceGCSafePoint`；成功返回实际的新安全点。随后 `notifyGCV2AfterGC` 仅对 keyspace-level GC 的外部 manager 工作：`master`/`ttl`/`gcv2` 先 recycle，`master`/`ttl` 再按当前 lifetime register；这些通知失败是 best effort，仅记事件，不推翻已完成的 GC。

unified GC 的 `runKeyspaceDeleteRange` 从 runtime 读取现有 GC safe point；零或读取失败均直接跳过，只记录过旧安全点事件并执行 delete-range/redo，不在此路径解析锁或广播安全点。

## 数据与状态

持久状态存放在 GC 系统表键中：leader UUID/描述/租约、last-run、run interval、lifetime、safe point、并发度、enable、mode、scan-lock mode 和 auto-concurrency。`Stats` 只把 leader UUID/描述/租约、last-run 和 safe point 映射为 `tidb_gc_*` 状态变量；单个读取失败会被忽略，其他项仍返回。

默认策略包括：GC 默认启用；run interval 和 lifetime 均为 10 分钟；并发度默认 2、范围 1..=128；默认 mode 是 distributed；自动并发默认开启。`checkUseDistributedGC` 会补写缺失 mode，但当前无论配置值为何都返回 `true`，因此 mode 是兼容字段而非真实分支开关。

内存状态分三层：`WorkerState` 控制作业串行和节流；`CancellationToken` 在线程间共享终止状态；`handle` 确保后台线程最多一个。`placement_cache` 在 `deleteRanges` 中收集已清理 ID，但当前函数结束前没有读取它，因此不能将其描述为跨任务去重机制。`keyspaceID` 在构造时保存，但本文件的调度逻辑未直接读取该字段。

Store 过滤由 `needsGCOperationForStore` 决定：Tombstone、`tiflash` 和 `tiflash_compute` 不参与；空 engine 或 `tikv` 参与；其他活跃 engine 返回错误，避免静默跳过未知存储。delete-range 实际并发上限是 `max(configured / 4, 1)`；自动模式再与 `max(range_num / 100000, 1)` 取较小值，固定模式只用前者。

## 依赖与调用关系

直接标准库依赖包括线程、channel、`Arc<Mutex<_>>`、原子变量、条件变量、时间和集合。唯一在源码中直接具名的工作区外部接口是 `astersql_extworkload::Manager`，用于 GC 成功后的 GCV2 回收/注册；Cargo 还声明 DDL、session、kv、metrics、tablecodec 等依赖，但它们在本文件中由 `GCWorkerRuntime` 隔离，具体适配实现不应臆测为本文件的直接调用。

主要内部调用边为：

`Start → start → tick → checkLeader → leaderTick → prepare/runKeyspaceGCJobInUnifiedGCMode → runGCJob/runKeyspaceDeleteRange`。

普通 GC 的下游边为：

`runGCJob → resolveLocks → deleteRanges → redoDeleteRanges → broadcastGCSafePoint → notifyGCV2AfterGC`。

delete-range 的下游边为：

`deleteRanges → LoadDeleteRanges → DeleteRangeRaftV2 | doUnsafeDestroyRangeRequest → GCPlacementRules → GCLabelRules → CompleteDeleteRange`；redo 则为 `LoadDoneDeleteRanges → doUnsafeDestroyRangeRequest → DeleteDoneRecord`。

RustCodeGraph 将该文件索引为 197 个符号，并标出 11 个使用文件；精确查询确认 Rust/Go 两份 `NewGCWorker`、`leaderTick`、`runGCJob`、`checkLeader` 和 `RunDistributedGCJob`。仓库文本检索显示 Rust 生产侧主要由 `pkg/store/gcworker/lib.rs` 及 `pkg/lib.rs` 导出；同目录 `gc_worker_test.rs` 是目前最直接、最完整的行为调用者。

## 错误处理与边界

构造时非 TiKV storage 是硬错误。`createSession` 则不是有界重试：创建失败会记录 `create_session`，每 10ms 重试，取消后 panic；因此 runtime 必须保证会话最终可建，或调用方必须接受取消期间 panic 的边界。`SessionGuard` 对未提交事务自动回滚并始终关闭会话，降低早退风险。

调度层多采用“记录并等待下个 tick”：`tick` 不向上抛错；后台作业错误通过 channel 回传并记录。`runGCJob` 的四个主要阶段用 `stageError` 增加阶段观测且立即停止后续步骤，尤其广播失败时不会通知 GCV2。相反，单个 delete-range 项失败会记录并继续其他项，外层 `deleteRanges` 仍返回 `Ok(())`；规则清理失败也只中断当前任务后续步骤。调用者不能仅凭 `deleteRanges` 返回值断言每个区间都成功，必须结合失败指标/事件和 redo 机制。

`doUnsafeDestroyRangeRequest` 收集所有 Store 错误，任一个失败就返回聚合消息。未知 live Store engine 是错误；TiFlash 和 Tombstone 是有意跳过。时间解析严格校验日期、时钟、小数和时区，并检查溢出；时长不支持负值，支持 `ns/us/µs/μs/ms/s/m/h` 复合项，结果超过 `u64` 纳秒时报错。

值得注意的数值边界是 `resolveLocks(0, ...)` 使用 `wrapping_sub(1)` 得到 `u64::MAX`；独立 Rust 测试明确固定了这一当前行为。keyspace 分页用 `checked_add(1)`，最后 ID 为 `u32::MAX` 时安全终止。安全点禁止倒退；没有前进返回 0/None，避免错误地持久化或启动作业。

## 并发与资源生命周期

`Start` 在持有 handle 锁时完成一次性 spawn，释放锁后等待启动握手；重复调用不会创建第二条线程。主循环最多每 100ms 检查一次完成通知/取消，也会被 Condvar 立即唤醒。`Close` 先发布取消，再取走并 join handle；`Drop` 仅在最后一个共享 handle 引用上调用 `Close`，避免某个 clone 提前关闭共同线程。

每轮普通 GC 由独立线程执行，`gc_is_running` 防止重叠；完成 channel 是清除此标志的唯一正常路径。unified 分支也 spawn 作业，但该函数没有像普通分支一样显式将 `gc_is_running` 设为 true；实际重复触发主要依赖 `needsToWait`、run interval 和 last-run 持久化约束，这是扩展时应特别审查的差异。

`forEachRange` 使用 scoped threads 和 `AtomicUsize` 分配任务下标，线程数被限制为至少 1、至多任务数（空列表仍创建至多一个立即退出的线程）。每个非 RaftKV v2 区间还会在 `doUnsafeDestroyRangeRequest` 内对 Store 再做一层并发，因此峰值 fan-out 近似“区间 worker 数 × Store 数”；修改并发算法时要评估 RPC、内存和线程开销。

取消是协作式的：调度等待、锁解析和 range RPC 可观察 token，但普通 Rust 线程不能被强制终止。`Close` 的 join 时间取决于 runtime 方法是否及时响应传入的取消令牌和超时。`unsafeDestroyRangeTimeout` 与通用 `gcTimeout` 均为 5 分钟，但本文件实际把前者传给 RPC；`gcTimeout` 在本文件中未使用。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/store/gcworker/gc_worker.go`，Rust 保留了 Go 风格符号名、系统表键、默认值以及主流程：`NewGCWorker`、`start/tick/leaderTick`、`prepare/checkPrepare`、安全点推进、`runGCJob`、delete-range/redo、`resolveLocks`、`checkLeader` 和三个公开一次性 API 均有直接对应。Rust 用 `GCWorkerRuntime` 集中代替 Go 版散落的 `kv.Storage`、PD client、session、DDL util 和 TiKV client 调用，以便独立测试。

关键语义保持包括：leader lease 和系统表事务、10 分钟 lifetime 下限、自动/固定并发、RaftKV v2 与 UnsafeDestroyRange 分支、null/multi-keyspace 锁解析、GCV2 通知角色顺序，以及 Go 时间/`time.Duration` 文本格式。`gc_worker_test.rs` 的测试名大量沿用 Go `gc_worker_test.go`，例如 `TestPrepareGC`、`TestLeaderTick`、`TestConcurrentDeleteRanges`、keyspace 批处理系列、`TestRunGCJobAPI` 和 `TestCalcDeleteRangeConcurrency`。

不能把 Rust 视为逐行等价。Go 文件约 2011 行，包含具体 region GC、placement/label SQL 操作和生产组件接线；Rust 文件约 1660 行，把这些动作折叠到 runtime trait。Go 的 `RunGCJob` 与中心化路径历史细节更丰富，而当前 Rust `RunGCJob` 直接调用 `RunDistributedGCJob`；Rust `checkUseDistributedGC` 也恒真。上述差异是当前实现事实，新增兼容行为时应先对照 Go 同名函数，而不是仅扩充 trait mock 或用简化逻辑让测试通过。

## 扩展指南

- 修改调度条件时，优先落在 `leaderTick`、`checkPrepare` 或 unified 分支，并同步检查 `WorkerState` 的设置/清除、last-run 的持久化时机和 leader 事务语义。
- 新增外部能力应先判断是否属于 `GCWorkerRuntime` 边界；生产适配和 `gc_worker_test.rs::FakeRuntime` 必须同时实现。不要把具体 PD/session/DDL 客户端重新耦合进算法层。
- 调整安全点必须维护三条不变量：txn safe point 不倒退；锁解析上限与 safe point 的关系明确；只有全局 GC safe point 广播成功后才能触发依赖该点的 GCV2 通知。
- 扩展 delete-range 时保持“删除数据成功后才清理规则并完成记录”的顺序；明确单项失败是继续还是终止，并让返回值和观测语义一致。改变 fan-out 前评估区间与 Store 的嵌套并发。
- 调整 keyspace 枚举时覆盖空列表、禁用空间、keyspace-level GC、恰好 50 条、跨批次以及 `u32::MAX`；对应独立测试已集中在 `pkg/store/gcworker/gc_worker_test.rs`，不要把测试内嵌回生产文件。
- 改动系统表时间/时长格式时必须同时验证 Go 兼容格式和旧 Rust 数字时间的读取路径，避免升级后无法读取已有行。
- 公开行为要与 `pkg/store/gcworker/gc_worker.go` 和 `pkg/store/gcworker/gc_worker_test.go` 对齐。若 Rust 选择不同抽象，应记录差异，但不能删减 Go 的安全、错误或生命周期语义。

## 验证依据

- 源码：`pkg/store/gcworker/gc_worker.rs`（1660 行），核对了常量、数据类型、两个 trait、`GCWorker` 全部调度/GC 路径、公开便捷 API、时间与时长解析函数。
- crate 边界：`pkg/store/gcworker/Cargo.toml`、`pkg/store/gcworker/lib.rs`；确认 crate 名、Go 包映射、直接导出、依赖清单和独立测试模块声明。
- RustCodeGraph：`status` 显示索引含 11467 个文件/307296 个节点/1848419 条边；`files --filter pkg/store/gcworker` 确认目标 Rust、Go 和测试文件被索引；`node --file ...` 分段读取目标与测试源码；`query` 核对 `NewGCWorker`、`leaderTick`、`runGCJob`、`resolveLocks`、`deleteRanges`、`checkLeader`、`RunDistributedGCJob`。自然语言 `explore` 和限定 callers/callees 在 30 秒窗口内未返回，因此调用关系以已索引源码节点及精确符号查询交叉确认。
- Rust 测试：`pkg/store/gcworker/gc_worker_test.rs` 验证配置准备、状态变量、Store 过滤、并发删除、RaftKV v2 分支、leader、null/multi-keyspace 锁解析、公开 API、pending txn、placement/label 规则、并发计算和 GCV2 成功/失败顺序；`pkg/store/gcworker/main_test.rs` 验证测试环境默认常量。
- Go 对照：`pkg/store/gcworker/gc_worker.go` 与 `pkg/store/gcworker/gc_worker_test.go`，核对同名流程、常量、边界与测试意图。本文未运行 Cargo，符合纯文档任务约束；结构校验结果在任务交付时单独记录。

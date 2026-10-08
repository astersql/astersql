# `pkg/util/memory/arbitrator.rs`

## 文件定位

本文件是 `astersql-util-memory` crate 的进程内内存仲裁核心。crate 入口 `pkg/util/memory/lib.rs` 以 `pub mod arbitrator; pub use arbitrator::*;` 暴露这里的 API；`pkg/util/memory/Cargo.toml` 将 crate 根设为 `lib.rs`，并声明 `mem-arbitrator` feature（当前 `lib.rs` 并未用该 feature 条件编译本模块）。仲裁器位于运行时内存采样与查询/算子内存池之间：`pkg/util/memory/global_arbitrator.rs` 创建 `MemArbitrator`、启动自动轮询并调用 `HandleRuntimeStats`，`pkg/util/memory/tracker.rs` 用 root-pool API 申请查询配额，`pkg/session/runtime.rs` 与 `pkg/session/runtime/control.rs` 使用 await-free 分片预算。

它解决的是“进程硬限制之下，多个查询如何记账、等待、取消及在 OOM 风险时止损”的问题，不负责采集操作系统内存，也不直接实现查询终止；后者通过 `ArbitrateHelper` 和 `KillEventChan` 交给会话侧完成。

## 核心职责

- 维护硬限制、软限制、已分配配额、实际堆占用和未受控内存估计，核心状态集中在 `MemArbitrator`。
- 为每个查询/任务维护 `RootPoolEntry`，由 `EmplaceRootPool`、`RestartEntryByContext`、`RequestQuota`、`ResetRootPoolByID` 和 `RemoveRootPoolByID` 管理生命周期。
- 在 `disable`、`standard`、`priority` 三种 `ArbitratorWorkMode` 下执行不同的准入与回收策略；priority 模式会优先处理高优先级请求，并通过 `cancel_lower_priority` 取消低优先级上下文。
- 用 `HandleRuntimeStats` 判断内存风险和 OOM 风险，必要时由 `cancel_for_oom` 按优先级、root 配额和实际 heap-inuse 选择受害者。
- 提供不绑定 root pool 的 256 分片 await-free 预算、SQL digest 最近峰值缓存、动态 buffer/pool 初始容量及堆放大率调优。
- 记录 `ExecMetricsCounter`，使任务成功/失败、取消原因、风险进入、await-free 扩缩容等行为可诊断。

## 主要符号

- `ArbitrateResult` 与 `ArbitrateOk`/`ArbitrateFail`：一次配额请求的二值结果。
- `ArbitrationPriority`：LOW、MEDIUM、HIGH 三档；`ArbitrationWaitAverse` 是任务计数数组的第四个桶，而不是第四种优先级。
- `ArbitratorWorkMode`：standard 正常按队列处理；priority 支持优先级排序与低优先级取消；disable 跳过硬限制准入但仍进行配额记账。
- `SoftLimitMode`：Disable 回落到 `oomRisk()`，Specified 使用绝对值或比例，Auto 配合实际堆/配额比动态调整。
- `ArbitratorStopReason`：区分 OOM kill、wait-averse、standard 和 priority 的 quota cancel。
- `ConcurrentBudget`：原子容量/用量桶。`ConsumeQuota` 在正向请求超额时返回 `BudgetExhausted`，但用量已经增加；调用者必须据此扩容或回滚。`Reserve` 的参数是目标容量，且不会降到当前用量以下。
- `TrackedConcurrentBudget`：在 `ConcurrentBudget` 之外记录实际 heap-inuse，用于 await-free 路径的未受控内存估算。
- `ArbitrateHelper`、`CancelReceiver`、`ArbitrationContext`：连接仲裁器和会话终止机制。上下文同时观察 helper 的 `Done()`、显式取消通道和内部 `stopped` 标志，`stop` 只通知 helper 一次。
- `MemArbitrator`：总状态机；原子字段承载高频数值/标志，`Mutex`/`RwLock` 保护复合状态、队列、缓存与上下文。
- `RootPoolHandle`、`RootPoolEntry`、`PendingRequest`：root pool 的稳定句柄、配额记录和同步等待请求；均为本模块内部数据布局，外部只能持有 handle。
- `ArbitratorRuntimeStats`、`MemUsage`、`PoolAllocProfile` 与各类 metrics：运行时输入、helper 观测、调优结果和诊断快照。
- `NewMemArbitrator`、`NewArbitrationContext`、`memHangRisk`：主要构造/辅助入口；`memHangRisk` 在释放速度低于阈值或等待超过 5 秒时返回真。

## 执行流程

1. `pkg/util/memory/global_arbitrator.rs` 调用 `NewMemArbitrator(limit)`；非正 limit 使用 `DefMaxLimit`，正值裁剪到该上限。默认工作模式是 disable，默认软限制是硬限制的 95%。全局初始化随后以 10ms tick 调用 `StartAutoRun`。
2. tracker 为 uid 调用 `EmplaceRootPool`，再用带 helper、优先级、wait-averse 等属性的 `ArbitrationContext` 调用 `RestartEntryByContext`。已有但未运行的 entry 可复用；正在运行的 entry 拒绝重复 restart。
3. `RequestQuota` 对非正请求直接归还 root 配额；正请求先验证 entry、context 和运行状态，再把 `PendingRequest` 放入队列、增加等待字节与任务桶并触发 `RunOneRound`。调用线程通过 `Condvar` 等待，期间每 10ms 重试一轮并持续检查取消状态。
4. `RunOneRound` 用 `round_mutex` 串行化轮次，先更新堆放大率、清理闲置 context cache、刷新 pool 中位配额和 priority buffer；随后 standard/disable 从队首取请求，priority 选择最高优先级请求。
5. `execute_pending` 在 disable 模式直接记账，否则调用 CAS 循环 `allocate`。priority 请求失败且并非 wait-averse 时，`cancel_lower_priority` 先选择更低优先级、较大配额的运行任务并等待其在 20 秒窗口内释放；其他失败按模式调用 `ArbitrationContext::stop`。完成时更新 root 配额、metrics、等待量和任务计数，并唤醒 Condvar。
6. `ResetRootPoolByID` 归还配额、清除 kill/cancel 状态并把 uid 留在 context cache 供近期堆观测；`RemoveRootPoolByID` 才真正删除 entry。可选 tune 会把较大历史峰值加入最近 30 秒窗口，供 `SuggestPoolInitCap` 计算中位桶。
7. 全局采样器调用 `HandleRuntimeStats`。它合并 heap-inuse 与 off-heap，更新 tracked/out-of-control 估算和风险状态：进入内存风险有滞回规则，达到硬超限立即进入 OOM，否则在 OOM 阈值持续约 1 秒后进入 OOM。`cancel_for_oom` 随后按低优先级优先、同优先级较大 root quota 优先选择上下文，直到预计回收量覆盖 `heap_inuse - memRisk()`。
8. await-free 路径由 uid 哈希到固定分片。`ConsumeQuotaFromAwaitFreePool` 先消费本地 budget；若超额，再按 deficit 与 `PoolAllocUnit` 取较大值向总仲裁器扩容。处于 OOM 风险、实际内存不留余量或总配额不足都会拒绝扩容。`ShrinkAwaitFreePool` 仅回收 used 不大于零的分片容量。
9. digest cache 以 30 秒为槽，保留当前与前一有效槽的最大值；超过容量时按峰值从小到大删除，收缩到 limit 的一半。

## 数据与状态

硬限制 `limit`、总配额 `allocated`、堆指标、风险标志、近似时间和工作模式使用原子变量。`allocate` 的不变量是：非 priority 保留量为零；priority 下 `allocated + request` 不得超过 `limit - reserved_buffer - out_of_control`。`release` 饱和到零，避免负记账。

`roots` 是 uid 到 `RootPoolEntry` 的唯一映射。entry 的 `quota` 是当前由仲裁器保留的字节，`max_quota` 用于生命周期峰值，`running` 与 `context` 共同决定能否申请。`pending` 保存尚未决议的请求；`waiting_alloc` 和 `tasks` 必须在请求成功、失败或取消后同步扣减。

`under_kill` 和 `under_cancel` 保存 20 秒内已发出停止信号但尚未完全释放的预计回收量，防止同一缺口重复选取过多受害者。`context_cache` 在 reset 后保留 uid，空闲 10 分钟才由 `cleanup_context_cache` 清除，但不会因此删除 root entry。

`reserved_buffer` 只在 priority 模式生效，取最近两个 30 秒有效窗口内活跃上下文/大任务的最大 heap 值。`pool_consumption` 同样使用三个循环槽，并从最近两个槽求中位桶。`digest_profiles` 的键为 digest id，id 0 是无效值。`await_free` 固定为 256 个分片，`await_free_index` 依赖分片数为 2 的幂这一隐含不变量。

## 依赖与调用关系

上游直接证据：

- `pkg/util/memory/global_arbitrator.rs` 调用 `NewMemArbitrator`、`StartAutoRun` 和 `HandleRuntimeStats`，是进程级生命周期与采样入口。
- `pkg/util/memory/tracker.rs` 调用 `EmplaceRootPool`、`RestartEntryByContext`、`RequestQuota` 和 `ResetRootPoolByID`，把 tracker 生命周期映射成 root pool 生命周期。
- `pkg/session/runtime.rs`、`pkg/session/runtime/control.rs` 和 `pkg/session/runtime/scan_adapter_runtime.rs` 使用 `ConsumeQuotaFromAwaitFreePool`/`GetAwaitFreeBudgets`；tracker 的小额路径也使用该池。
- RustCodeGraph 的文件节点报告本文件被 18 个文件使用，其中包括 executor、DDL ingest、session runtime 和 memory 子模块；精确引用以以上受限 `rg` 结果为准。

下游依赖主要是标准库的 atomics、`Arc`、`Mutex`、`RwLock`、`Condvar`、线程和时间；业务依赖只有 crate 内 `calcRatio`/配额常量，以及经 `lib.rs` 再导出的 `sqlkiller::KillEventChan`。Cargo 中的 `sqlkiller_crate` 提供取消事件，其他系统探针依赖由 `global_arbitrator.rs`/相邻模块消费，而不是本文件直接调用。

## 错误处理与边界

本模块的热路径主要用结果值而非通用错误类型：无效/不可用 root、已停止 context、分配失败返回 `ArbitrateFail`；重复启动/停止、无效模式、无变化的 limit 返回 `false`。`EmplaceRootPool*` 目前签名为 `Result<_, String>`，但当前实现没有实际错误分支，不能据此假设未来永不失败。

锁中毒通过 `expect(...)` 触发 panic；非法 `ArbitrationPriority::String` 或 `ArbitratorWorkMode::String` 编码也 panic，而未知停止原因返回 `UNKNOWN`。`ArbitratorWorkMode::from_text` 对未知文本降级为 disable。时间倒退在 `memHangRisk` 中通过 `unwrap_or_default` 当作零时长处理。

关键边界包括：请求小于等于零视为释放；digest id 0 被忽略；limit 非正构造值回落到最大值；软限制绝对值裁剪到硬限制；风险阈值分别为 90%/95%；OOM 非硬超限需要持续约 1 秒；优先级回收最多等待 20 秒；context cache 空闲 10 分钟；budget 超额错误不会自动撤销已经增加的 `used`。

## 并发与资源生命周期

`MemArbitrator` 允许多线程共享。高频标量使用 Acquire/Release 或 AcqRel；复合集合用独立锁降低无关路径冲突。`round_mutex` 保证同一时刻只有一个仲裁轮次，避免多个请求重复出队/回收。请求方和执行轮次通过 `Arc<(Mutex<Option<ArbitrateResult>>, Condvar)>` 交接结果。

`ArbitrationContext::stop` 以 `AtomicBool::swap` 保证 helper 至多收到一次 Stop；可用性还取决于 helper 自身 Done 事件和外部 `cancel_ch`。reset 只结束 root 的运行态并清空 context，不调用 helper 的 `Finish`；调用方必须按自己的会话生命周期处理 Finish。remove 则删除所有仲裁记录并归还剩余配额。

`StartAutoRun` 用 `Arc::downgrade` 启动后台线程，避免 worker 强持有仲裁器；`StopAutoRun` 清除运行标志、join 线程并补跑最后一轮。`Drop` 未自动调用停止，因此持有者在可回收前应显式 `StopAutoRun`；不过 weak upgrade 失败也会使线程退出。测试 `go_merge_24_priority_request_waits_for_cancelled_pool_to_release_quota` 与 `go_merge_24_cancelled_waiter_cleans_pending_accounting` 验证了等待、释放和取消清账行为。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/memory/arbitrator.go`，主要命名和语义对应：`MemArbitrator`、三种 work mode、三档 priority、soft limit、root pool 生命周期、digest profile、await-free budget、risk/cancel 原因和 `ArbitrateHelper` 均保留。Rust 的独立测试注释与测试名明确映射 `TestMemArbitrator*`、`TestBasicUtils` 和 Go merge 场景。

Rust 不是 Go 文件的逐行同构：Go 使用分片 entry map、quota shard、订阅/唤醒与更完整的 GC、持久化、profiling 和周期 action；Rust 当前以 `HashMap + VecDeque + Condvar` 和轻量 `StartAutoRun` 实现核心可用路径。Go `AutoRun`/`executeTick` 的完整运行时控制面在 Rust 中由 `global_arbitrator.rs` 的采样与本文件轮询共同承担。Rust 的 `ConcurrentBudget` 直接用 atomics，root handle 也从 Go 指针式 entry 封装为 uid 句柄。

因此扩展时应以 Go 行为作为语义基准，但只能声称 Rust 已实现本文件实际存在的路径；Go 中的 GC action、持久化策略或更复杂分片算法不能当作 Rust 当前能力。已验证的关键等价性包括：Reserve 是绝对容量、30 秒 digest 窗口、soft OOM 延迟、优先级选择、zero-quota context 仍可因 heap 使用被 kill，以及 await-free 在 OOM 余量不足时拒绝增长。

## 扩展指南

- 新增工作模式、优先级或停止原因时，同步更新值类型、字符串映射、`execute_pending`/`RunOneRound` 分支、metrics 数组长度与独立测试；避免让新编码越界后静默漏记。
- 修改 root pool 准入/回收时，从 `RequestQuota`、`execute_pending`、`cancel_lower_priority` 和 `ResetRootPoolByID` 入手，并保持 `waiting_alloc`、`tasks`、root quota 与 `allocated` 四组记账一致。并发回归测试应放在 `pkg/util/memory/arbitrator_test.rs`，不要内嵌进生产文件。
- 修改风险算法时同时检查 `HandleRuntimeStats`、`update_tracked_heap_avoidance`、`cancel_for_oom` 和 `global_arbitrator.rs` 的采样输入；必须覆盖 off-heap、滞回、持续 1 秒与硬超限立即触发。
- 修改 await-free 分片数时，要保持 2 的幂或改写 `await_free_index` 的位与索引逻辑；还要验证容量归还到总 `allocated`，以及超额 `ConsumeQuota` 已先增加 used 的契约。
- 修改调优窗口/缓存时，联动 buffer、pool median、digest 和 mem magnification 的槽轮转边界，尤其测试时间跨槽和旧峰值过期。
- 与 Go 同步功能时先比较 `pkg/util/memory/arbitrator.go` 的对应方法和 `pkg/util/memory/arbitrator_test.go`；Rust 测试继续使用 `arbitrator_test.rs` 或现有独立补充测试文件。性能风险集中在全局 `roots`/`pending` 锁、轮次内 helper 回调、候选排序和 10ms 轮询，不应在持锁区增加阻塞 I/O。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/util/memory/arbitrator.rs`；`files --filter pkg/util/memory` 定位了 Rust/Go 源与测试；`node --file pkg/util/memory/arbitrator.rs` 分段读取全部 1993 行并报告 18 个使用文件；`query MemArbitrator --kind struct` 同时定位 Rust 与 Go 定义。精确 `callers RequestQuota` 查询在本地索引中未及时返回，已停止，调用关系改由限定 Rust 路径的 `rg` 验证。
- 源与 crate：`pkg/util/memory/arbitrator.rs`、`pkg/util/memory/lib.rs`、`pkg/util/memory/Cargo.toml`。
- 直接 Rust 调用方：`pkg/util/memory/global_arbitrator.rs`、`pkg/util/memory/tracker.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`。
- Go 对照：`pkg/util/memory/arbitrator.go`；Go 回归依据：`pkg/util/memory/arbitrator_test.go`。
- Rust 独立测试：`pkg/util/memory/arbitrator_test.rs` 与 `pkg/util/memory/arbitrator_2_aster_unit_test.rs`。覆盖枚举/阈值、soft limit、budget、context 单次停止、root 生命周期、优先级回收与等待、OOM 受害者、digest 窗口、await-free、动态放大率和缓存清理。
- 本任务是纯文档分析，按计划不运行 Cargo；交付只执行固定 11 章节的结构检查，并人工核对上述路径和符号引用。

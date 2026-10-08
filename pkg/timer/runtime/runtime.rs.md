# `pkg/timer/runtime/runtime.rs`

## 文件定位

`runtime.rs` 是 `astersql-timer-runtime` crate 的分组定时器调度核心。crate 入口 `pkg/timer/runtime/lib.rs` 将本文件声明为 `runtime` 模块，并把同 crate 的 `cache.rs` 与 `worker.rs` 分别作为内存状态层和 Hook 执行层；`pkg/timer/runtime/Cargo.toml` 则表明它通过 `timer-api` 依赖定时器存储、条件、记录、Hook 与可取消上下文，并直接使用 `crossbeam-channel`、`chrono` 和 `uuid`。

应用中的已接线入口位于 `pkg/session/runtime/ttl_runtime.rs:920-1010`：TTL Job Manager 创建表存储，以 TTL timer key 前缀设置 `TimerCond`，注册 `SqlTtlTimerHook` 工厂，调用 `NewTimerRuntimeBuilder(...).Build()` 后启动运行时。`DomainTtlTimerRuntime::drop`（同文件 `538-575`）停止运行时，并关闭存储与会话池。因此本文件不是通用进程入口，而是把某一 `groupID` 下、满足基础条件的一组 timer 驱动起来的可复用运行时。

## 核心职责

- 通过 `TimerRuntimeBuilder` 绑定 `groupID`、`TimerStore`、查询条件与按 `HookClass` 注册的工厂（`runtime.rs:76-131`）。
- 由 `TimerGroupRuntime::Start` 拉起单个可恢复主循环；启动时全量加载，运行中周期全量刷新、批量处理 Watch 事件、刷新等待关闭的事件，并计算下一次触发扫描时间（`165-305`）。
- 从 `TimersCache` 选出已到尝试时间的 Idle timer，按 `nextEventTime` 重新排序，懒创建对应 Hook Worker，并以有界通道投递 `TriggerEventRequest`（`340-460`）。
- 消化 Worker 响应，维护 `Idle → Triggering → WaitTriggerClose` 的本地处理状态，或在失败时回到 Idle 并安排重试（`486-520`）。
- 在存储 Watch 不可用或断开时保持正确性：不可用时使用永不就绪的 channel，断开后等待 `reWatchInterval` 重订阅，另有每分钟全量刷新兜底（`45-58`, `251-305`, `570-619`）。

本文件只负责调度和内存协调；具体的 schedule 计算与缓存排序在 `cache.rs`，存储接口和 Hook 契约在 `timer-api`，PreSched/落库/OnSched 流程在 `worker.rs:359` 起实现。

## 主要符号

- 周期常量：`fullRefreshTimersInterval=60s`、`maxTriggerEventInterval=60s`、`minTriggerEventInterval=1s`、`reWatchInterval=5s`、`batchProcessWatchRespInterval=1s`、`retryBusyWorkerInterval=5s`、`checkWaitCloseTimerInterval=10s`（`45-58`）。它们共同限定一致性刷新、触发节流、Watch 恢复和忙 Worker 退避。
- `Counter(Arc<AtomicU64>)`：使用 Relaxed 原子顺序统计全量/增量刷新次数；只承担观测计数，不同步其他状态（`60-74`）。
- `NewTimerRuntimeBuilder(groupID, store)`：创建响应容量为 `workerRespChanCap` 的有界通道，并初始化条件、工厂表、缓存、Worker 表、可注入时钟、运行状态和计数器（`81-103`）。
- `TimerRuntimeBuilder::{SetCond, RegisterHookFactory, Build}`：消费式 builder API。`SetCond` 替换基础条件；重复注册同一 class 时 HashMap 的新工厂覆盖旧工厂；`Build` 返回可克隆的 `TimerGroupRuntime`（`105-132`）。
- `TimerGroupRuntime`：公开句柄，仅持有 `Arc<RuntimeInner>`；克隆句柄共享存储、缓存、Worker、通道与生命周期状态（`134-155`）。
- `RuntimeInner`：内部共享状态。`cond`/`nowFunc` 用 `RwLock`，工厂、缓存、Worker 和 `RunState` 用 `Mutex`；`workerRespSender/Receiver` 连接 Worker 与主循环（`140-155`）。
- `RunState`：保存主循环 `Context`、取消句柄和线程 `JoinHandle`（`157-163`）。
- 生命周期 API：`Start` 幂等启动，`Running` 同时要求 context 与 cancel 存在，`Stop` 取消并 join 主循环，再逐个 join 已创建 Worker（`165-234`）。停止后 `ctx` 仍保留而 `cancel` 被取走，因此同一实例的后续 `Start` 会因 `ctx.is_some()` 直接返回；当前实现是一次启动生命周期。
- 调度 API：`fullRefreshTimers`、`tryTriggerTimerEvents`、`getNextTryTriggerDuration`、`handleWorkerResponse`、`partialRefreshTimers`、`tryCloseTriggeringTimers`、`createWatchTimerChan`、`batchHandleWatchResponses`、`buildTimerIDsCond`（`315-639`）。公开可见性也供独立测试直接验证细分行为。
- 测试/观测辅助：`setNowFunc` 同时替换运行时与缓存时钟；`cacheUpdate`、`cacheSetProcStatus`、`cacheProcStatus`、`cacheNextTryTime` 以及两个刷新计数读取器暴露受控状态检查（`641-712`）。
- `sleep`：以至多 10ms 的小段睡眠实现可取消等待，供 panic 后重启间隔使用（`715-724`）。

## 执行流程

1. 调用者用 `NewTimerRuntimeBuilder` 创建运行时，按需设置基础 `Cond` 并注册一个或多个 Hook 工厂，最后 `Build`。TTL 实际入口的条件是 key 前缀，工厂产出 `SqlTtlTimerHook`（`pkg/session/runtime/ttl_runtime.rs:972-1004`）。
2. `Start` 在 `runState` 锁内检查是否已有 context，创建可取消上下文并保存 cancel，然后生成主循环线程执行 `runRecoverLoop`（`runtime.rs:165-181`）。
3. `runRecoverLoop` 用 `catch_unwind` 包住 `loopOnce`。首次立即运行；发生 panic 后累计次数并可取消地等待 `retryLoopWait`（默认 10s），随后用同一运行时状态重新进入循环；正常返回或取消则退出（`236-249`）。
4. `loopOnce` 创建 Watch、初始化各周期时间点，先执行一次 `fullRefreshTimersWithContext`，再以 5ms 轮询节奏协调五类工作（`251-305`）：
   - 每 60s 从 Store 全量 `List` 并用 `TimersCache::fullUpdateTimers` 对齐缓存；
   - 每 10s 刷新 `WaitTriggerClose` ID，使外部完成的事件回到可调度状态；
   - 每 1s 合并 Watch create/update/delete 响应，避免逐事件访问 Store；
   - 到达 `nextTry` 时扫描并投递 timer，随后按缓存中最早尝试点和 1s/60s 边界计算下次扫描；
   - 持续排空 Worker 响应；Watch 断开时换成 never channel，5s 后重新订阅。
5. `tryTriggerTimerEvents` 只遍历缓存中 `procIdle` 的排序项。遇到尚未到 `tryTriggerTime` 的首项就终止；对“存储状态为 Idle，但被禁用、没有下一事件、或下一事件仍在未来”的条目只越过而不触发。其余候选按 `nextEventTime` 从早到晚排序，`None` 优先（`340-368`）。
6. 每个候选通过 `ensureWorker` 按 `HookClass` 查找或懒创建 Worker。没有注册工厂则跳过。空 `EventID` 生成 UUID simple 字符串；请求成功入队后缓存转为 `procTriggering`。通道满或断开时不阻塞主循环，而把下次尝试推迟 5s（`370-411`）。
7. Worker 在 `worker.rs` 中执行 Hook 的 Start、PreSched、存储更新与 OnSched，响应回到共享 channel。`handleWorkerResponse` 可先应用新的 timer 记录或删除通知；成功转入 `procWaitTriggerClose`，失败转回 `procIdle`，若给出 `retryAfter` 则更新下次尝试时间（`486-520`）。
8. 成功事件由外部关闭后，周期性的 `tryCloseTriggeringTimers` 从 Store 增量刷新 `waitCloseTimerIDs`。缓存发现 EventID 改变时回到 Idle，完成一次触发循环（`552-568`；状态复位依据 `cache.rs:211-234`）。

## 数据与状态

`TimerGroupRuntime` 的所有克隆共享一份 `RuntimeInner`。基础选择条件是 `Arc<dyn Cond>`；增量查询由 `buildTimerIDsCond` 构造成 `And(base, Or(TimerCond{ID}...))`，所以 Watch 或等待关闭刷新不会越过调用者设定的分组范围（`621-639`）。`partialRefreshTimersWithContext` 对 Store 返回结果取 ID 集合：请求但未返回的 ID 会从缓存删除，返回记录再批量更新（`528-550`）。

缓存状态机由 `cache.rs` 定义并由本文件驱动：`procIdle` 才参与触发排序；请求成功投递后为 `procTriggering`；Worker 成功后为 `procWaitTriggerClose` 并加入 `waitCloseTimerIDs`；失败回 Idle。缓存同时维护按 `nextTryTriggerTime` 排序的 ID 向量，保证扫描能在首个未来时间处提前停止（`cache.rs:146-191, 211-329`）。

`getNextTryTriggerDuration` 先用 `now-lastTryTriggerTime` 抵扣最大/最小间隔，再与缓存最早 `tryTriggerTime-now` 取小值，最终不低于剩余最小间隔；时钟倒退或负区间通过 `to_std().unwrap_or(Duration::ZERO)` 饱和为零（`runtime.rs:462-484`）。这既保证平时不高于 60s 扫描一次，也避免刚扫描后因大量变更高频空转；若距离上次扫描已超过 60s，立即返回零。

全量/增量计数在发起 Store List 前增加，因此包含失败尝试。计数采用 Relaxed 原子，只保证独立数值读写的原子性。`nowFunc` 是测试可注入时钟；设置时必须同步给缓存，否则运行时的到期判断与缓存 schedule 计算会使用不同时间基准，`setNowFunc` 已维护这一不变量（`641-653`）。

## 依赖与调用关系

上游直接生产调用者是 `pkg/session/runtime/ttl_runtime.rs`：它负责选择 TTL owner 生命周期、创建 `TimerStore`、限定 TTL key 前缀、注册 TTL Hook，并持有运行时直到失去 owner 或管理器销毁。RustCodeGraph 的文件节点也把该文件和 `pkg/timer/runtime/runtime_test.rs` 列为目标文件的两个直接使用方。

下游关系如下：

- `crate::api` / `astersql-timer-api`：提供 `TimerStore::{List, Watch, WatchSupported}`、`TimerRecord`、条件组合 `And/Or/TimerCond`、`Context`、Hook 工厂以及 Watch 事件类型（导入见 `runtime.rs:23-27`，crate 声明见 `Cargo.toml`）。
- `crate::cache::TimersCache`：计算并维护 timer 的下一事件/尝试时间、本地处理状态、全量及增量合并、等待关闭集合（`runtime.rs:28-31`; `cache.rs:146-329`）。
- `crate::worker`：`newHookWorker` 创建每个 HookClass 的有界请求队列和线程；`TriggerEventRequest/Response` 是运行时与 Worker 的消息协议（`runtime.rs:32-34`; `worker.rs:51-113, 187-351`）。
- `crossbeam-channel`：Worker 请求和响应均为有界 channel；Watch 不可用或断开时使用 `never()` receiver。运行时通过非阻塞 `try_send`/`try_recv` 保持主循环可推进。
- `uuid`：只在 timer 尚无 EventID 时生成新事件标识；已有 EventID（例如恢复中的 Trigger 状态）会被复用（`376-380`）。
- `chrono`：把 `std::time::Duration` 与 API 的 `Timestamp` 做加减和非负转换。

RustCodeGraph 对 `loopOnce` 的调用边确认了 `fullRefreshTimersWithContext`、`tryTriggerTimerEvents`、`getNextTryTriggerDuration`、`handleWorkerResponse`、`tryCloseTriggeringTimersWithContext`、`createWatchTimerChan`、`batchHandleWatchResponsesWithContext` 等下游；对 `tryTriggerTimerEvents` 确认了 `now`、`ensureWorker`、`context` 和 `TriggerEventRequest` 构造关系。

## 错误处理与边界

- Store `List` 失败时，全量刷新直接保留现有缓存，增量刷新返回 `false`；没有将瞬时存储错误误解释为“所有 timer 被删除”（`322-338`, `528-550`）。错误值当前未记录或上抛，这是与 Go 版日志能力的明确差异。
- 未注册 `HookClass` 时 `ensureWorker` 返回 `None`，timer 保持原状态并等待后续扫描；注册工厂但 Worker channel 满或断开时推迟 5s，避免阻塞整个分组（`414-460`, `387-411`）。
- Worker 响应到达时若 timer 已不在缓存，立即忽略。若响应携带删除记录，删除后再次检查存在性，避免随后写入无效状态（`487-508`）。
- 空 ID 集合的增量刷新直接返回 `false`，不会构造空 OR 或访问 Store（`529-537`）。Watch 未识别的事件类型被忽略；create/update 通过 Store 重新读取权威记录，而不是信任 Watch payload（`585-619`）。
- `Mutex`/`RwLock` 中毒统一通过 `poisoned.into_inner()` 继续访问，主循环 panic 由外层恢复；这提高存活性，但被 panic 中断的跨字段不变量必须由后续刷新修复，不能把“继续运行”视作状态一定未受影响。
- `chrono::Duration::from_std(...).expect(...)` 用于两个编译期/响应 Duration 转换点；标准 Duration 超过 chrono 可表示范围会 panic，并进入恢复外壳（若发生在主线程直接调用公开方法，则由调用者环境处理）（`399-402`, `513-517`）。
- `Start` 对重复启动幂等，但当前实例停止后不可再次启动；`Stop` 可重复调用，第二次没有主循环 join，仍会遍历 Worker，而 Worker 的 join handle 已被取走时是无操作（`165-234`, `worker.rs:340-351`）。

## 并发与资源生命周期

一个 `TimerGroupRuntime` 对应一个主循环线程；正常主循环路径会为每个实际遇到且已注册的 `HookClass` 懒创建并缓存一个 `HookWorker`。`workers` HashMap 的查找与插入分别加锁而非在同一临界区内完成；当前生产调用由单个主循环串行发起，但若外部克隆句柄并发直接调用公开的 `tryTriggerTimerEvents`，两个调用可能同时未命中并各自创建 Worker，最后只有一个留在 map 中。扩展并发入口时必须先封闭这个竞态。请求队列有界，主循环使用 `try_send`，因此背压表现为 timer 的 5s 延迟重试，而不是线程阻塞。

生命周期顺序是：builder 初始化共享状态 → `Start` 创建取消上下文和主线程 → `ensureWorker` 按需派生共享该 context 的 Worker → `Stop` 触发取消 → join 主线程 → 收集 Worker 克隆并逐个 `stopAndJoin`。先取消再等待使 Worker 的 `recv_timeout` 和 panic 重试等待都能退出。TTL 包装器随后关闭 Store 和 session pool（`pkg/session/runtime/ttl_runtime.rs:570-575`），避免 Worker 在依赖资源关闭后仍运行。

锁粒度方面，缓存扫描先在锁内复制候选记录，再释放锁并投递 Worker；这避免通道操作期间长期持有缓存锁（`340-398`）。Worker 响应更新在一次缓存锁作用域内完成，保持记录替换、状态迁移和重试时间相互一致（`486-520`）。`Stop` 在释放 `runState` 锁后才 join 主线程，避免主线程若读取 context 时与停止方互锁（`208-234`）。

主循环当前不是 channel-select，而是每轮非阻塞排空响应、读取至多一个 Watch 响应，再睡眠 5ms。其资源/性能特征是空闲时约每 5ms 唤醒一次；Watch 高吞吐时单轮只取一个响应，但 1s 批窗口会聚合已取得的事件。扩展时需评估该轮询模型对延迟、CPU 和积压的影响。

## 与 Go 版本的对应关系

直接对照 `pkg/timer/runtime/runtime.go`，Rust 保留了 builder、分组运行时、全量/增量刷新、触发优先级、Worker 懒创建、Watch 批处理/重连、等待关闭刷新、panic 恢复以及触发间隔上下界等核心语义。Rust 独立测试 `runtime_test.rs` 的测试主题也逐项对应 Go 的 `runtime_test.go`：启动停止、Worker 创建、触发/优先级、响应处理、刷新、Watch、端到端流程与 panic 恢复。

主要实现差异如下：

- Go 用 `select`、ticker/timer 和 WaitGroup 驱动事件循环；Rust 用 `Instant` 截止时间、crossbeam 非阻塞收发和 5ms sleep 轮询。目标行为相同，但唤醒与批处理时序不是逐指令等价。
- Go 在测试环境把最小触发间隔和 Watch 批周期降到 1ms；Rust 常量固定为 1s，测试通过直接调用细分 API、可注入时钟或等待真实周期验证。
- Go 使用 Prometheus counter、结构化日志和 `TimerClient` 字段；Rust 本文件用本地原子 `Counter`，没有日志字段，创建 Worker 时按 Store 构造 `NewDefaultTimerClient`。因此指标名称、错误日志和运维可观测性尚未一一移植，但调度数据路径存在。
- Go 由共享 WaitGroup 等待主循环与 Worker；Rust 为主循环和每个 Worker 保存独立 `JoinHandle`，`Stop` 显式逐一等待。
- Go 的新 EventID 是 UUID 原始字节的 hex 字符串；Rust `Uuid::simple().to_string()` 同样产生无连字符的 32 位十六进制文本。
- Go 的空闲 Watch 使用包级永不关闭 channel；Rust 使用 `crossbeam_channel::never()`。Go 在 Watch 断开时重置 timer，Rust记录 `rewatchAt`，语义均为约 5s 后重订阅。
- Rust 的 `handleWorkerResponse` 在应用可选的新记录后额外复查 timer 是否仍存在，显式保护删除响应路径；Go 后续缓存写入对缺失 ID 本身为无操作，结果一致。

当前 Rust 测试另含 `go_merge_43_timer_watch_update_triggers_manual_request_promptly`，验证 Watch 更新能及时唤醒长周期 timer 的手动请求；它记录了后续 Go 合并语义在 Rust 侧的回归覆盖（`runtime_test.rs:653-683`）。

## 扩展指南

- 新增 timer 筛选维度：优先扩展 `timer-api` 的 `Cond`/`TimerCond` 与 Store 实现，再通过 `TimerRuntimeBuilder::SetCond` 注入；务必保持 `buildTimerIDsCond` 的 `And(base, Or(ids))` 结构，防止增量刷新越界。同步扩展 `runtime_test.rs::test_close_waiting_close_timers` 或 Watch 批处理测试验证组合条件。
- 新增 Watch 事件类型：在 `batchHandleWatchResponsesWithContext` 明确决定它属于权威记录重读、直接删除还是无需缓存动作，并在独立的 `pkg/timer/runtime/runtime_test.rs` 增加批处理及冲突顺序回归；不要把测试写入生产文件。
- 调整触发公平性/优先级：修改 `tryTriggerTimerEvents` 的候选过滤或排序，同时检查 `TimersCache::iterTryTriggerTimers` 的 `tryTriggerTime` 顺序不变量；同步 `test_try_trigger_timer` 与 `test_try_trigger_time_priority`，并关注大量同 class timer 对有界 Worker channel 的饥饿风险。
- 增加 Worker 并发或队列策略：接入点是 `ensureWorker`、`newHookWorker` 和 `workerRecvChanCap`。必须保留“成功入队后才转 Triggering”的原子语义，以及队列满时不阻塞主循环的背压行为；需要在 runtime 与 worker 的独立测试文件分别覆盖。
- 改造主循环为阻塞 select/更低频轮询：需要同时保留取消、全量刷新、WaitTriggerClose、Watch 批窗口、重订阅截止时间、Worker 响应后重算 nextTry、panic 恢复七类唤醒源。性能收益应与 Watch 积压和触发延迟一起基准验证。
- 补齐 Go 可观测性：可在 `Counter` 或错误分支接入 crate 统一的 metrics/logging，但不得让观测失败改变 Store 错误时保留缓存、Worker 忙时退避等行为；同时核对 `Cargo.toml` 依赖与上层 dashboard/标签兼容性。
- 若要支持 Stop 后 Restart，需同时重置 `RunState.ctx`、处理旧 Worker map/已取走的 JoinHandle 和响应 channel 中的陈旧消息；只放宽 `Start` 的判断会复用已退出 Worker，是不安全的。应新增独立生命周期回归测试后再改变契约。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/timer/runtime/runtime.rs` 返回该文件，`node --file ...` 显示 724 行、61 个符号，并标识直接使用方 `pkg/session/runtime/ttl_runtime.rs` 与 `pkg/timer/runtime/runtime_test.rs`。
- RustCodeGraph 符号查询：`query TimerGroupRuntime --kind struct` 与 `query NewTimerRuntimeBuilder --kind function` 均定位到本文件；`query tryTriggerTimerEvents --kind function` 和 `query loopOnce --kind function` 定位核心流程。`callees runtime.rs::loopOnce` 确认刷新、触发、响应、Watch 与收尾调用边；`callees runtime.rs::tryTriggerTimerEvents` 确认时钟、Worker、上下文与请求构造关系。工具对同名 `runtime.rs` 会返回额外候选，本文只采用路径明确为 `pkg/timer/runtime/runtime.rs` 的边。
- 已读生产路径：`pkg/timer/runtime/runtime.rs` 全文；`pkg/timer/runtime/lib.rs`；`pkg/timer/runtime/Cargo.toml`；直接下游 `pkg/timer/runtime/cache.rs:140-340`、`pkg/timer/runtime/worker.rs:40-125,180-380`；直接上游 `pkg/session/runtime/ttl_runtime.rs:520-575,920-1069`。
- Go 对照：`pkg/timer/runtime/runtime.go` 全文，并核对 `pkg/timer/runtime/runtime_test.go` 的测试清单（13 个 `Test*`，覆盖生命周期、触发、刷新、Watch、完整流程和 panic 恢复）。
- Rust 测试：`pkg/timer/runtime/runtime_test.rs` 全文，共 14 个 `#[test]`，除与 Go 测试主题对应外，还覆盖 Watch 手动触发及时性。关键边界证据包括 Store List 失败保留缓存、busy Worker 延迟重试、nextTry 上下界、Watch 断线 5s 重连、panic 重试可被 Stop 立即取消。
- 本任务是纯文档分析，按计划未运行 Cargo，也未把测试逻辑嵌入生产源文件。交付前运行任务指定的 11 章节结构命令，并人工复核所有路径、符号与行为陈述均可由上述源码或测试反查。

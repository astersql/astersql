# `pkg/timer/runtime/cache.rs`

## 文件定位

`cache.rs` 是 `astersql-timer-runtime` crate 的内存调度缓存实现，由 crate 入口 `pkg/timer/runtime/lib.rs` 以 `pub mod cache` 暴露。它位于持久化 `TimerStore` 与 `TimerGroupRuntime` 的触发循环之间：存储记录通过全量刷新、增量刷新或 worker 响应进入缓存；运行时再按缓存维护的时间顺序寻找可触发定时器。该文件不负责持久化、不创建 worker，也不执行 hook。

crate 边界由 `pkg/timer/runtime/Cargo.toml` 确定：包名是 `astersql-timer-runtime`，定时器领域类型来自同 workspace 的 `astersql-timer-api`（在代码中经 `crate::api` 再导出），本文件直接使用 `chrono` 处理时间，并仅使用标准库集合与 `Arc`。模块入口还把独立测试文件 `cache_test.rs` 和 `cache_1_aster_unit_test.rs` 挂到 `cfg(test)` 下；测试逻辑没有内嵌在本生产文件中。

## 核心职责

本文件承担四组职责：

1. 为每个 `TimerRecord` 保存一份快照，并派生 `nextEventTime`、作为排序键的 `nextTryTriggerTime`、运行时处理状态和关联事件 ID（`TimerCacheItem`，第 65–145 行）。
2. 维护 `timerID → item`、按尝试触发时间排序的 ID 列表，以及等待事件关闭的 ID 集合三个相互关联的索引（`TimersCache`，第 147–159 行）。
3. 实现单条、批量和全量同步，拒绝陈旧更新，在记录变化时重算调度时间，并维持排序和等待关闭状态（`updateTimer`、`partialBatchUpdateTimers`、`fullUpdateTimers`）。
4. 向 runtime 提供有序扫描、处理状态迁移、重试时间调整和时区偏移变化判断（`iterTryTriggerTimers`、`setTimerProcStatus`、`updateNextTryTriggerTime`、`locationChanged`）。

缓存保存的是 `TimerRecord::clone()` 得到的快照，而不是调用者对象的借用；这使缓存内容不会随后被调用者原地修改。它是调度加速与运行时状态层，不是记录的最终事实来源；全量/增量刷新仍以 `TimerStore` 返回结果为准。

## 主要符号

- `RuntimeProcStatus = i8` 及 `procIdle`、`procTriggering`、`procWaitTriggerClose`：与 Go `int8`/`iota` 状态对应。`Idle` 可参加触发扫描，`Triggering` 表示请求已交给 worker，`WaitTriggerClose` 表示 worker 已成功、等待持久化记录中的 `EventID` 发生变化。
- `NowFn = Arc<dyn Fn() -> Timestamp + Send + Sync + 'static>`、`systemNow()`：可在线程间共享的时钟注入点；生产默认读取 UTC 当前时间，测试可用固定时钟。
- `farFuture()`：返回 2999-01-01 UTC，作为“当前没有可尝试时间”的排序哨兵。`goZeroTime()` 返回公元 1 年 UTC，用于模拟 Go 值类型 `time.Time{}`。
- `TimerCacheItem`：包含 `initialized`、公开的 `timer`、`nextEventTime`、`nextTryTriggerTime`、`procStatus`、`triggerEventID`。私有 `initialized` 明确区分“尚未写入记录”与“记录 ID 为空”，不能通过业务 ID 推断初始化状态。
- `TimerCacheItem::update(&mut self, &TimerRecord, &NowFn) -> bool`：单条快照和派生时间的核心更新函数；返回值只表示缓存内容是否接受并重算，不等价于记录是否应持久化。
- `TimersCache`：缓存容器。`items` 是主索引，`sorted` 是按 `nextTryTriggerTime` 升序的 ID 向量，`waitCloseTimerIDs` 是状态辅助索引，`nowFunc` 是共享时钟。
- `newTimersCache()` / `TimersCache::with_now()` / `setNowFunc()`：分别创建系统时钟缓存、创建注入时钟缓存、替换已有缓存的时钟。
- 查询接口 `item()`、`waitCloseTimerIDs()`、`sortedTimerIDs()`、`tryTriggerTimerIDs()`、`hasTimer()`：其中前三者分别返回条目引用、等待关闭集合引用、排序 ID 副本；`tryTriggerTimerIDs()` 只保留 `procIdle`；`hasTimer()` 检查主索引。
- 更新接口 `updateTimer()`、`removeTimer()`、`partialBatchUpdateTimers()`、`fullUpdateTimers()`：负责索引一致性及全量对账。
- 调度接口 `setTimerProcStatus()`、`updateNextTryTriggerTime()`、`iterTryTriggerTimers()`：负责状态/重试时间变化与有序只读遍历。
- 私有 `resort()`：确保 ID 位于 `sorted`，随后依据主索引中的 `nextTryTriggerTime` 对整个向量稳定排序。
- `locationChanged()`：公开的时区变化判定；`locationOffset()` 是其私有偏移提取辅助函数。
- `runtimeProcStatus`、`timerCacheItem`、`timersCache`：保留 Go 风格名称的类型别名，用于迁移兼容；实际 Rust 类型分别是 `RuntimeProcStatus`、`TimerCacheItem`、`TimersCache`。

## 执行流程

单条记录进入缓存时，`updateTimer` 先以 ID 查找或创建 `TimerCacheItem::empty()`，再调用条目的 `update`：

1. 已初始化条目拒绝更低 `Version`；相同版本且 `Location` 的当前 UTC 偏移未变化也直接返回 `false`。
2. 接受更新后克隆整条记录，清空 `nextEventTime`，并把 `nextTryTriggerTime` 重置为 2999 年哨兵。
3. 记录启用时调用 `TimerRecord::NextEventTime()`；只有返回 `Ok((next, true))` 才保存计算结果。正在进行的手动请求优先把 `nextEventTime` 覆盖为注入时钟的当前值，使其立即可触发。
4. 持久化事件状态为 `SchedEventIdle` 时，以有效 `nextEventTime` 作为排序键；为 `SchedEventTrigger` 时，以 `EventStart` 为排序键，缺失的 Rust `Option` 映射成 Go 零时间而非 2999 年哨兵；其他状态保持远未来。
5. 若条目发生变化，`updateTimer` 调用 `resort`。无论记录版本是否被接受，它随后都会检查本地是否处于 `procWaitTriggerClose`：只要缓存关联的 `triggerEventID` 与新记录 `EventID` 不同，就迁回 `procIdle` 并从等待关闭集合移除。

批量增量更新通过 `partialBatchUpdateTimers` 逐条复用上述流程，并用逻辑或累计“是否至少有一条变化”。全量更新 `fullUpdateTimers` 先建立远端 ID 集合，删除本地主索引中远端不存在的条目，再执行批量更新；删除同时清理排序向量和等待关闭集合。

触发循环调用 `iterTryTriggerTimers` 按 `sorted` 顺序遍历，只将 `procIdle` 条目交给回调。回调返回 `false` 即提前停止，这使 `TimerGroupRuntime::tryTriggerTimerEvents` 能在遇到首个未来时间时停止扫描，也使 `getNextTryTriggerDuration` 能只查看最早候选。worker 通道接收成功后 runtime 调用 `setTimerProcStatus(..., procTriggering, eventID)`；worker 成功响应后进入 `procWaitTriggerClose`，失败则回到 `procIdle`，并可通过 `updateNextTryTriggerTime` 安排重试。

## 数据与状态

核心一致性关系如下：

- `items` 是所有有效缓存条目的权威内存集合；`sorted` 中每个 ID 应能在 `items` 找到，正常更新路径保证每个 ID 至多加入一次。
- `sorted` 包含所有条目，而非只包含可触发条目；是否对外可见由 `procStatus == procIdle` 在遍历时过滤。排序键相同的条目由 Rust 稳定排序保留先前相对顺序，但调用方不应把同一时间点的 ID 顺序当成业务契约。
- `waitCloseTimerIDs` 应恰好跟踪通过 `setTimerProcStatus` 设置为 `procWaitTriggerClose` 的缓存 ID。删除条目或迁往其他状态时同步移除。
- `nextEventTime` 是从调度策略或手动请求派生的业务触发时刻；`nextTryTriggerTime` 是 runtime 下一次尝试该记录的时刻，可能因 worker 忙或失败重试而晚于业务时刻。
- `updateNextTryTriggerTime` 对持久化状态为 `SchedEventIdle` 的记录施加下限：缺少 `nextEventTime`，或请求时间早于它，均忽略更新。因此无有效调度点的 Idle 记录不会被人工从远未来提前；等于或晚于事件时间可以接受。非 Idle 记录不受此下限约束。
- `procStatus` 是仅存在于 runtime 的进程内状态，和 `TimerRecord.EventStatus` 不是同一状态机：前者描述本地处理阶段，后者描述存储中的调度事件状态。
- `initialized` 是必要的独立不变量：空字符串 ID 也是合法缓存键。`cache_test.rs::test_empty_id_same_version_update_is_ignored` 验证第一次空 ID 更新可接受，而同版本第二次必须拒绝。

## 依赖与调用关系

上游主要位于 `pkg/timer/runtime/runtime.rs` 的 `TimerGroupRuntime`：

- `NewTimerRuntimeBuilder` 把 `newTimersCache()` 放进 `RuntimeInner.cache: Mutex<TimersCache>`。
- `fullRefreshTimersWithContext` 从 `TimerStore::List` 获取记录后调用 `fullUpdateTimers`。
- `partialRefreshTimersWithContext` 对存储未返回的目标 ID 调用 `removeTimer`，再调用 `partialBatchUpdateTimers`；Watch 的 create/update 事件经这里进入缓存，delete 事件直接调用 `removeTimer`。
- `tryTriggerTimerEvents` 通过 `iterTryTriggerTimers` 按序收集到期记录；成功投递 worker 后设为 `procTriggering`，通道忙或断开时用 `updateNextTryTriggerTime` 延迟重试。
- `handleWorkerResponse` 可调用 `updateTimer` 或 `removeTimer`，随后按成功/失败设置 `procWaitTriggerClose` 或 `procIdle`，失败且带 `retryAfter` 时更新重试时间。
- `tryCloseTriggeringTimersWithContext` 克隆 `waitCloseTimerIDs()` 并对这些 ID 做增量刷新，最终由 `updateTimer` 检测 `EventID` 变化完成收尾。
- `getNextTryTriggerDuration` 用 `iterTryTriggerTimers` 的首个 Idle 条目压缩下一次扫描间隔；`setNowFunc` 同步替换 runtime 与缓存的时钟。

下游领域行为来自 `crate::api`：`TimerRecord::NextEventTime()` 计算调度时间，`ManualRequest::IsManualRequesting()` 判定即时请求，`TimerLocation` 提供命名或固定时区，`Timestamp` 承载带固定偏移的时间。`chrono` 用于系统时间、哨兵/零值构造和命名时区当前偏移；`HashMap`、`HashSet`、`Vec` 分别实现主索引、集合索引和有序 ID 容器。

RustCodeGraph 的精确 Trail 还确认：`partialBatchUpdateTimers → updateTimer`，`updateTimer → TimerCacheItem::update / resort / setTimerProcStatus`，`fullUpdateTimers → removeTimer / partialBatchUpdateTimers`。`iterTryTriggerTimers` 在 runtime 中由触发扫描与下次等待时长计算调用；索引对该泛型闭包调用的 caller 解析不完整，因此这两条上游关系以 `runtime.rs` 第 341–483 行的直接源码为准。

## 错误处理与边界

本文件的公共更新 API 不返回 `Result`。调度表达式解析或下一事件计算失败时，`TimerCacheItem::update` 丢弃错误并保持 `nextEventTime = None`、`nextTryTriggerTime = farFuture()`；这表示该条目继续存在但暂不触发。调用者若需要暴露错误，应在写入 `TimerRecord` 或调用 `NextEventTime` 的更上层完成校验，不能从这里区分“没有下一事件”和“计算失败”。

不存在的 ID 是幂等边界：`removeTimer` 返回 `false`；`setTimerProcStatus` 和 `updateNextTryTriggerTime` 静默返回；`item` 返回 `None`。版本边界为：旧版本拒绝；同版本仅当 `locationChanged` 为真才接受；新版本接受。时区比较只比较“当前”UTC 秒偏移，不比较时区名称或完整夏令时规则，因此同偏移的不同位置被视为未变化，而命名时区在未来转换规则上的差异不会在当前偏移相同时触发更新。

`farFuture()` 和 `goZeroTime()` 内的日期构造使用 `expect`，依赖 chrono 能表示固定常量；这些是编程不变量而非输入错误。`resort` 使用 `items[left]`/`items[right]` 索引，如果内部索引被绕过本文件的方法破坏会 panic；字段私有化使正常外部调用无法制造该状态。`iterTryTriggerTimers` 对意外缺失的 ID 则选择 `continue`，具有防御性。

## 并发与资源生命周期

`TimersCache` 自身没有内部锁，也不会创建线程、通道或异步任务；所有修改方法都要求 `&mut self`。生产环境的并发边界在 `RuntimeInner.cache: Mutex<TimersCache>`，runtime 在全量刷新、Watch 批处理、触发扫描和 worker 响应处理时持锁访问。因此扩展本文件时应保持回调短小，尤其不要在 `iterTryTriggerTimers` 回调内做阻塞 I/O；当前 runtime 只在锁内筛选/克隆记录，随后释放锁再向 worker 投递。

`NowFn` 使用 `Arc + Send + Sync`，可随 runtime 跨线程共享；`setNowFunc` 替换的是后续计算使用的函数，已有条目的派生时间不会自动重算，需有记录更新/刷新才会变化。条目生命周期始于首次 `updateTimer`，可被增量删除、Watch 删除、worker 返回删除或全量对账删除；删除时三个索引同步清理。等待关闭生命周期从成功 worker 响应设置 `procWaitTriggerClose` 开始，到存储刷新观察到不同 `EventID`、显式换状态或删除条目结束。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/timer/runtime/cache.go`，Rust 保留了相同的状态常量、核心类型和方法名，主要行为逐项对应：版本/Location 去重、记录克隆、下一事件计算、手动请求立即触发、Idle/Trigger 排序键、全量删缺失、等待关闭集合、重试下限、有序 Idle 遍历和 Location 偏移比较。

实现层差异如下：

- Go `timerCacheItem.timer` 是可空指针；Rust 用非可空的 `TimerRecord` 加私有 `initialized` 表达相同状态，避免把空 ID 错当未初始化。
- Go `nextEventTime` 是 `*time.Time`；Rust 是 `Option<Timestamp>`。Go 的 `EventStart` 是值字段，Rust 是 `Option`，所以 Rust 对 Trigger 且缺失 `EventStart` 显式使用 `goZeroTime()`，由 `test_trigger_without_event_start_uses_go_zero_time` 固化语义。
- Go 用 `container/list` 及条目内 `sortEle` 做局部移动；Rust 为避免安全代码中的自引用节点，用 `Vec<String>` 保存 ID，每次变化执行全量稳定排序。语义一致，但更新时间复杂度从 Go 的定位后线性移动变为 Rust 的向量排序，定时器规模较大或更新频繁时需关注性能。
- Go `timeutil.Zone` 与 Rust `locationOffset` 都以偏移比较 Location；Rust 的命名时区偏移通过 `Utc::now()` 取得，因此同样是比较当前有效偏移，而非名称身份。
- Go 构造函数返回指针；Rust 返回拥有所有权的 `TimersCache`，由上层 `Mutex` 管理共享可变访问。

`pkg/timer/runtime/cache_test.go` 与 `cache_test.rs` 的对应测试覆盖更新、排序、全量刷新和 Location。Rust 另有两项明确补齐 Go 值/指针语义的回归：Trigger 缺失开始时间映射 Go 零值，以及空 ID 的初始化判定。

## 扩展指南

新增缓存行为时优先选择以下接入点：记录字段影响派生触发时间或去重条件时修改 `TimerCacheItem::update`；新增全局辅助索引或处理阶段时同时修改 `TimersCache`、`setTimerProcStatus`、`removeTimer` 和 `fullUpdateTimers`；改变重试约束时修改 `updateNextTryTriggerTime`；改变触发候选选择时修改 `iterTryTriggerTimers`，并同步检查 `runtime.rs::tryTriggerTimerEvents` 与 `getNextTryTriggerDuration` 的提前停止假设。

必须维持三个索引的一致性、`nextTryTriggerTime` 升序、非 Idle 不参与遍历、WaitClose 集合与状态一致、陈旧版本不覆盖新快照、缓存持有独立记录副本等不变量。若引入新状态，不能只添加常量，还要定义它是否进入 `waitCloseTimerIDs`、是否参与触发扫描、EventID 变化时如何迁移，以及 worker 响应如何设置它。

测试应继续放在独立文件。Go 对齐行为优先扩展 `pkg/timer/runtime/cache_test.rs` 并与 `pkg/timer/runtime/cache_test.go` 相应场景核对；AsterSQL 特有或跨 cache/worker/runtime 的回归可放在 `pkg/timer/runtime/cache_1_aster_unit_test.rs`。若修改公开接口，还需检查 `pkg/timer/runtime/runtime_test.rs`。重点边界包括：相同/更旧版本、同版本不同时区偏移、无效策略、禁用记录、手动请求、缺失 EventStart、空 ID、不存在 ID、相同排序键、状态迁移、全量删除和重试时间下限。

性能方面，新增频繁更新路径前应评估 `resort` 的全量排序成本；若改为树或堆，需要同时保留按 ID 更新/删除、稳定或明确定义的同键顺序、提前停止遍历以及测试可观察性。兼容方面，不要把 Location 名称比较替换偏移比较，也不要把 Trigger 的缺失 `EventStart` 改回远未来，除非 Go 基准行为同步改变并有迁移依据。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`pkg/timer/runtime/cache.rs` 被索引为 38 个符号。通过 `node --file` 阅读了目标文件全部 360 行，并查询了 `TimersCache`、`updateTimer`、`iterTryTriggerTimers`、`fullUpdateTimers`、`setTimerProcStatus`、`updateNextTryTriggerTime` 的定义与 Trail。
- 目标源码：`pkg/timer/runtime/cache.rs`。关键证据包括 `TimerCacheItem::update`（第 101–144 行）、`TimersCache`（第 148–159 行）、单条/批量/全量更新（第 216–272 行）、状态与重试（第 275–308 行）、有序遍历/重排（第 311–338 行）、Location 比较（第 341–360 行）。
- 上游调用：`pkg/timer/runtime/runtime.rs`。核对了缓存构造（第 81–103 行）、全量刷新（第 315–338 行）、触发扫描与 worker 投递（第 340–412 行）、worker 响应（第 486–520 行）、增量/Watch/等待关闭刷新（第 522–619 行）及测试辅助入口（第 641–701 行）。
- crate 与模块边界：`pkg/timer/runtime/Cargo.toml`、`pkg/timer/runtime/lib.rs`；前者确认依赖和 Go 包元数据，后者确认 `cache` 的公开模块位置及独立测试挂载。
- Go 对照：`pkg/timer/runtime/cache.go` 全文件，特别是 `timerCacheItem.update`、`timersCache`、`resort`、`locationChanged`；相关基准测试为 `pkg/timer/runtime/cache_test.go`。
- Rust 测试：`pkg/timer/runtime/cache_test.rs` 覆盖更新去重、无效策略/禁用/手动请求、状态迁移、Go 零时间、空 ID、排序、全量刷新和 Location；`pkg/timer/runtime/cache_1_aster_unit_test.rs` 补充固定时钟下的 Go 对齐及 runtime/worker 联动证据。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构验证要求文档存在且恰有本文 11 个固定二级标题；同时人工复核所有行为陈述均可回指上述源码、调用边或测试，没有把错误吞并解释成上层已处理，也没有把未实现能力写成已支持。

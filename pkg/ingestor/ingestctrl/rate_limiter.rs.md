# `pkg/ingestor/ingestctrl/rate_limiter.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；crate 根由 `pkg/ingestor/ingestctrl/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 通过 `pub mod rate_limiter` 暴露本模块，并提供这里使用的 `CancellationToken`、`Error` 与 `Result`。模块负责两个层次的流量控制：可复用的 `TokenBucket` 令牌桶，以及按 TiKV Store 隔离的 `ingestLimiter`（每秒请求数与在途请求数的组合限制）。

当前 Rust 接线并不完整等同于 Go：代码搜索显示 `ingestLimiter`/`newIngestLimiter` 的 Rust 引用仅在本文件和 `rate_limiter_test.rs`，尚未被 Rust ingest 生产流程构造；但 `TokenBucket` 已被 `localhelper.rs` 生产代码复用，用于 Region split/scatter 限速和 `storeWriteLimiter` 的按 Store 写限速。因此，本文件不是纯桩，却包含“已接线的底层组件”和“已实现但尚未接入生产主链的 ingest 门面”两部分。

## 核心职责

- `TokenBucket` 维护单调时钟驱动的令牌余额，支持满桶初始化、按时间补充、热更新和按数量计算等待时间。
- `ingestLimiter` 以 `storeID` 为键惰性创建独立状态，使不同 Store 的 burst、速率余额和在途计数互不影响。
- `Acquire` 先取得 QPS 令牌，再占用并发槽；等待过程轮询 `CancellationToken`，避免取消后永久挂起。
- `Release` 归还在途槽并唤醒等待者；速率令牌不会因请求完成而归还。
- `Burst`、`NoLimit`、`getEventLimit` 和 `getRateBurst` 提供与 Go 配置语义相对应的容量计算和快速判断。
- `TokenBucket` 还作为 `localhelper.rs` 的共享实现，支撑 `splitAndScatterRegionInBatches` 和 `storeWriteLimiter::{WaitN, UpdateLimit}`。

## 主要符号

- `ratePerSecMultiplier: usize = 1000`：把“一次请求”换算为 1000 个内部令牌，使小于 1 QPS 的配置仍能用整数粒度对应 Go 的 `rate.Limiter` 设置。
- `TokenBucket { rate, burst, tokens, updated }`：crate 内可见的令牌桶。`rate` 为每秒补充令牌数，`burst` 为容量，`tokens` 为当前余额，`updated` 为上次结算时间。
- `TokenBucket::new(rate, burst)`：创建时令牌为满桶，允许首批请求立即形成 burst。
- `TokenBucket::update(rate, burst)`：先按旧速率补充到当前时刻，再替换配置，并把现有余额截断到新容量；`localhelper.rs::storeWriteLimiter::UpdateLimit` 用它热更新所有已创建 Store 桶。
- `TokenBucket::reserve_delay(count) -> Result<Option<Duration>>`：余额足够时扣除并返回 `None`；不足时不扣除，返回补齐差额所需时间；请求超过 burst 或速率非正时返回 `InvalidArgument`。
- `ingestLimiterPerStore`：每个 Store 的私有状态，包含受 `Mutex` 保护的 `in_flight`、用于唤醒等待者的 `Condvar` 和独立 `TokenBucket`。
- `ingestLimiter`：公开类型，保存取消令牌、规范化后的并发/QPS 上限，以及 `Mutex<HashMap<u64, Arc<...>>>` 形式的 Store 状态表。
- `newIngestLimiter(token, maxReqInFlight, maxReqPerSec)`：构造器；负并发和负速率均钳为 0，分别解释为“不限制”。
- `ingestLimiter::per_store(storeID)`：持有映射锁完成惰性插入，并用 `Arc` 把 Store 状态交给后续等待逻辑。
- `ingestLimiter::{Acquire, Release, Burst, NoLimit}`：组合限流的主要 API。
- `getEventLimit(ratePerSec)`：计算内部每秒令牌数，采用乘 1000 后截断、最小为 1 的规则。
- `getRateBurst(ratePerSec)`：QPS 向上取整并保证至少为 1，作为以“请求”为单位的外部 burst。

文件中没有 trait、宏或条件编译项。`TokenBucket` 与 `getEventLimit` 为 `pub(crate)`，`ingestLimiter`、构造器、常量、`getRateBurst` 及组合 API 为公开可见；`ingestLimiterPerStore`、`per_store`、`refill` 为模块内部实现。

## 执行流程

1. `newIngestLimiter` 保存取消令牌，把负上限归零，并创建空的 Store 映射；此时不分配任何单 Store 状态。
2. `Acquire(storeID, n)` 在完全不限速或 `n == 0` 时立即成功。否则调用 `per_store`：首次访问该 Store 时创建满桶，桶速率为 `getEventLimit(maxReqPerSec)`，容量为 `getRateBurst(maxReqPerSec) * 1000`。
3. 若 QPS 上限大于 0，`Acquire` 循环检查取消、锁定令牌桶并调用 `reserve_delay(n * 1000)`。余额不足时每次最多休眠 10ms，然后重新计算；余额足够时一次性扣除并进入下一阶段。
4. 若并发上限大于 0，`Acquire` 锁定 `in_flight`。当剩余槽不足以容纳 `n` 时，它检查取消并通过 `Condvar::wait_timeout` 最多等待 10ms；条件满足后增加 `in_flight`。
5. 调用方完成请求后以相同 `storeID` 和权重调用 `Release`。函数找到已有 Store 状态，减少 `in_flight`，再 `notify_all` 让所有该 Store 的等待者重新竞争；不存在的 Store 或并发未启用时直接返回。
6. `Burst` 把两个约束折算为一次可接受的请求量：均关闭时为 `usize::MAX`，仅一个启用时取该限制，两者均启用时取并发上限与速率 burst 的较小值。

`TokenBucket` 的独立生产路径位于 `localhelper.rs`：`splitAndScatterRegionInBatches` 按最大 split 次数创建一个桶，分批调用 `SplitAndScatter` 前等待令牌；`storeWriteLimiter::WaitN` 为每个 Store 惰性建桶、按 burst 分块消费，`UpdateLimit` 则调用 `TokenBucket::update` 热更新现存桶。

## 数据与状态

`TokenBucket` 的四个字段必须在同一把外层 `Mutex` 下访问；类型本身不负责同步。`updated: Instant` 使用单调时间，避免系统墙钟调整破坏补充量。`refill` 的不变量是 `0 <= tokens <= burst`；`update` 缩小 burst 时立即丢弃超出新容量的余额。

`ingestLimiter.limiters` 的键是 TiKV `storeID`，值由 `Arc` 共享。映射只增不删，因此一个 limiter 生命周期内访问过的 Store 状态会保留到整个 limiter 被释放。每个 Store 的速率锁与并发锁独立，不同 Store 除首次查表的短临界区外不会共享热点状态。

并发计数的不变量是 `in_flight <= maxReqInFlight`。一次 `Acquire(..., n)` 和对应 `Release(..., n)` 使用权重 `n`；调用方必须精确配对。速率令牌代表请求额度，成功扣除后不会由 `Release` 恢复。配置字段在构造后不可变；需要动态更新的是 `localhelper.rs` 中单独的 `storeWriteLimiter`，不是 `ingestLimiter`。

## 依赖与调用关系

本文件的直接外部依赖均来自标准库：`HashMap`、`Arc`、`Mutex`、`Condvar`、`Duration` 与 `Instant`。crate 内依赖是 `lib.rs` 定义的 `CancellationToken`、`Error` 和 `Result`；`Cargo.toml` 没有为本文件引入专用第三方限流库，Rust 版以标准同步原语自行实现 Go 的 `semaphore.Weighted` 与 `rate.Limiter` 语义。

RustCodeGraph 将 `rate_limiter.rs` 标为被多个同 crate 文件关联，但精确引用搜索表明生产调用集中在 `localhelper.rs` 对 `TokenBucket`、`getRateBurst` 和 `ratePerSecMultiplier` 的使用。已验证的下游边包括：

- `Acquire -> per_store -> TokenBucket::new`；
- `Acquire -> CancellationToken::check`、`TokenBucket::reserve_delay`、`Condvar::wait_timeout`；
- `reserve_delay -> refill`；
- `Release -> Condvar::notify_all`；
- `localhelper.rs::splitAndScatterRegionInBatchesWithLimiter -> TokenBucket::reserve_delay`；
- `localhelper.rs::storeWriteLimiter::WaitN -> TokenBucket::reserve_delay`；
- `localhelper.rs::storeWriteLimiter::UpdateLimit -> TokenBucket::update`。

Go 生产代码中，`local.go::ImportEngine` 会调用 `newIngestLimiter`；当前 Rust `local.rs` 没有对应引用。这是当前接线缺口，不应把 Go 调用边误写成已存在的 Rust 调用边。

## 错误处理与边界

- 任一 `Mutex`/`Condvar` 因持锁线程 panic 而 poison 时，`Acquire`、`per_store` 或令牌桶调用返回 `Error::Poisoned`。`Release` 是无返回值清理 API，映射锁或计数锁 poison 时静默返回，可能无法释放额度；调用方无法从该函数直接获知失败。
- `CancellationToken::check` 可令速率等待或并发等待返回 `Error::Cancelled`。检查粒度由最多 10ms 的短睡/超时等待决定，不承诺即时中断。
- `reserve_delay` 拒绝 `count > burst`，并在 `rate <= 0` 且又确实需要等待时拒绝继续；正常 `ingestLimiter` 只在正速率下调用它。
- `Acquire` 使用 `saturating_mul(1000)` 避免 `n` 换算令牌时整数溢出，但饱和值通常会超过 burst，从而形成明确的 `InvalidArgument`。
- `n > maxReqInFlight` 时并发等待永远不可能自行满足；与 Go 的加权信号量相同，只能等待取消。`rate_limiter_test.rs::oversized_concurrency_acquire_waits_for_cancellation` 固化了该行为。
- `Release` 若释放量大于当前持有量，会通过 `checked_sub(...).expect(...)` panic，以匹配 Go `semaphore.Weighted.Release` 的误用检测；不存在 Store 时则是幂等空操作。
- `newIngestLimiter` 将负配置钳为 0，这是 Rust 侧的防御性规范化；Go 构造器原样保留输入。扩展时不能无意改变这一差异。
- `getEventLimit` 对极小正速率至少返回 1 个内部令牌/秒；`getRateBurst` 对零及负值也至少返回 1，但只有对应限制启用时该结果才参与实际限流。

## 并发与资源生命周期

所有等待均为同步阻塞：速率阶段用 `std::thread::sleep`，并发阶段用 `Condvar::wait_timeout`，没有异步任务或后台补充线程。令牌在调用线程下一次进入 `refill` 时按时间惰性生成，因此 limiter 本身不需要启动、停止或 join。

锁顺序是：`per_store` 短暂持有映射锁，返回 `Arc` 后释放；速率阶段只持有该 Store 的 `limiter` 锁；并发阶段只持有 `in_flight` 锁。`Release` 先查表并克隆 `Arc`，显式释放映射锁后才锁 `in_flight`。这一顺序避免同时持有全局映射锁与单 Store 计数锁。

`notify_all` 不提供 FIFO 公平性；被唤醒线程和每 10ms 超时醒来的线程重新检查条件。令牌桶同样没有排队公平性，多个等待者会竞争互斥锁。`CancellationToken` 被 limiter 持有并可由外部克隆取消；取消不会自动清除已取得的并发槽，已成功 `Acquire` 的调用方仍必须 `Release`。

Store 状态随 `ingestLimiter` 一同销毁，没有显式关闭方法。`Arc` 保证即使状态已从映射锁作用域取出，当前操作期间仍保持有效；当前实现不会从映射删除条目。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/ingestor/ingestctrl/rate_limiter.go`，核心结构一一对应：Go 的 `sync.Map` 对应 Rust 的 `Mutex<HashMap<...>>`，`semaphore.Weighted` 对应 `Mutex<usize> + Condvar`，`rate.Limiter` 对应 `TokenBucket`，`context.Context` 对应 `CancellationToken`。两版均按 Store 隔离、先等待速率再获取并发权重，并以 `min(rate burst, concurrency)` 计算组合 burst。

速率换算保持 Go 的关键公式：内部速率是 `max(1, trunc(rate * 1000))`，桶容量是 `ceil(rate) * 1000`，所以 0.5 QPS 可表现为约每两秒一个请求，同时初始仍允许至少一个请求 burst。Rust 独立测试 `event_limit_has_the_same_minimum_as_go` 专门覆盖极小速率和截断规则。

需要注意的实现差异：Rust 以最多 10ms 的轮询实现可取消等待，Go 库直接接收 context；Rust 的构造器钳制负值，Go 原样保存；Rust 的 `Acquire(n == 0)` 在建 Store 状态前返回，而 Go 会走一次惰性建表；Rust 锁 poison 有显式 `Error::Poisoned` 分支，Go 锁没有相同错误模型。Rust `Release` 的超量释放 panic、超大并发权重等待取消，以及 `Burst`/`NoLimit` 真值表都有独立测试锁定 Go 语义。

迁移状态方面，Go `local.go::ImportEngine` 已构造并使用 `ingestLimiter`，Rust 对应主链尚未接入；Rust 当前生产价值主要来自 `TokenBucket` 被 `localhelper.rs` 复用。后续接线时应移植真实的获取/释放生命周期，不能仅因本模块单测通过就宣称 ingest 请求已经受此门面限制。

## 扩展指南

- 若改变令牌算法、精度或 burst 公式，优先修改 `TokenBucket::{refill, reserve_delay}`、`getEventLimit`、`getRateBurst`，并同步检查 `localhelper.rs` 的 split/scatter 与 Store 写限速两条现有生产路径，避免只修 `ingestLimiter` 测试。
- 若增加动态 ingest 配置，需要决定是在现有 `ingestLimiter` 上加入同步更新，还是复用 `storeWriteLimiter::UpdateLimit` 的模式；必须规定缩容时已有 `in_flight` 和令牌余额如何处理。
- 若将 `ingestLimiter` 接入 Rust `ImportEngine`，应从 Go `local.go` 的构造、`Acquire`、defer/失败清理和 `Release` 位置逐一对齐，并为成功、发送失败、取消和重试路径验证权重成对释放。
- 若改变等待机制，应保留取消上界、Store 隔离和无后台线程泄漏的性质；改成异步 API 会影响所有同步调用点，不能只替换 `sleep`。
- 若改变释放语义，需保留或有意识调整“超量释放 panic”和“未知 Store 空操作”契约，并同步 `over_release_panics_like_go`。
- 测试仍应放在独立的 `pkg/ingestor/ingestctrl/rate_limiter_test.rs`，不要内嵌到源文件。至少同步覆盖 `TestConcurrencyLimit`、`TestRateLimit`、两个取消测试、`TestIngestLimiterBurst`、极小速率、超大权重和超量释放；`TokenBucket::update` 的生产行为还应联动 `localhelper_test.rs` 中 Store 写 limiter 的热更新/禁用测试。
- 性能风险主要在全局 Store 映射锁、10ms 轮询延迟、`notify_all` 惊群和每 Store 状态只增不删；兼容风险主要在 Go 的截断/向上取整、初始满桶、获取顺序及错误/取消时机。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/ingestor/ingestctrl` 确认目标、Go 对照和独立测试均已覆盖。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/rate_limiter.rs --offset 1 --limit 500`：核对了全部 223 行、所有常量/类型/方法及模块内部调用。
- RustCodeGraph `explore` 与精确符号查询：确认 `newIngestLimiter`、`Acquire`、`Release`、`Burst`、`NoLimit`、`getEventLimit`、`getRateBurst`，以及 `Acquire -> per_store/reserve_delay`、`reserve_delay -> refill` 等边；通用 `Acquire` 名称存在跨仓库歧义，因此没有把非本文件同名边计入结论。
- RustCodeGraph `node` 读取 `pkg/ingestor/ingestctrl/localhelper.rs`：确认 split/scatter 对 `TokenBucket::reserve_delay` 的调用，以及 `storeWriteLimiter::{WaitN, UpdateLimit}` 对 `reserve_delay/update` 的调用。
- `pkg/ingestor/ingestctrl/Cargo.toml` 与 `lib.rs`：确认 crate 名称、根文件、模块公开关系、统一错误/取消类型；本包没有 `doc.go`。
- `pkg/ingestor/ingestctrl/rate_limiter.go`：核对 Go 数据结构、QPS/并发获取顺序、burst 公式和错误/取消模型。
- `pkg/ingestor/ingestctrl/rate_limiter_test.rs` 与 `rate_limiter_test.go`：核对同 Store 阻塞、跨 Store 隔离、初始 burst、速率等待、取消、组合 burst；Rust 额外验证极小速率、超大权重等待取消和超量释放 panic。
- 全仓精确引用搜索：Rust 生产代码只有 `localhelper.rs` 引用 `TokenBucket`/换算辅助，未发现 `newIngestLimiter` 或 `ingestLimiter` 的生产调用，因而将完整门面标记为尚未接线。
- 本任务为纯文档分析，按计划不运行 Cargo；最终仅执行固定十一章节的结构验证，并人工复核没有把 Go 调用边或预期设计写成当前 Rust 事实。

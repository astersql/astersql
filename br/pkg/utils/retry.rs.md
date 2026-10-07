# `br/pkg/utils/retry.rs`

## 文件定位

`retry.rs` 是 `astersql-br-pkg-utils` crate 中的通用重试执行层。crate 入口 `br/pkg/utils/lib.rs` 通过 `#[path = "retry.rs"] pub mod retry` 挂载它，并在 crate 根再导出 `WithRetry`、`WithRetryV2`、`WithRetryReturnLastErr`、`FallBack2CreateTable`、`VerboseRetry` 和 `GiveUpRetryOn`。退避策略本身不在本文件定义，而由相邻的 `br/pkg/utils/backoff.rs::BackoffStrategy` 提供；本文件负责消费策略、等待、聚合错误，并提供 TiKV Backoffer 适配器和策略装饰器。

该模块对应 Go 文件 `br/pkg/utils/retry.go`，但 Rust 当前接线仍处于迁移期：RustCodeGraph 将生产文件的直接使用面主要识别为 crate 入口，仓库检索也显示不少 BR 子模块仍使用各自 `stubs.rs` 中的局部 `WithRetry` 实现。因而本文件当前最明确的可执行契约来自同 crate 的独立测试 `br/pkg/utils/retry_test.rs` 与 `br/pkg/utils/backoff_test.rs`，不能把 Go 侧所有生产调用者直接视作 Rust 调用者。

## 核心职责

1. `WithRetry` / `WithRetryV2` 在 `BackoffStrategy::RemainingAttempts()` 大于零时重复执行闭包；成功立即返回，失败则把错误交给 `NextBackoff` 并进行可取消等待。
2. `WithRetryV2` 保留每次失败，通过 `astersql_errors::Join` 返回聚合错误；`WithRetryReturnLastErr` 则只返回最后一次失败，面向只关心最终原因的调用点。
3. `AdaptTiKVBackoffer` / `RetryWithBackoff` 把固定的类 TiKV `Backoffer` 与调用者请求的毫秒退避合并，提供累计时长、最大时长和并发退避请求收敛能力。
4. `VerboseRetry` 在不改变内层策略决定的前提下记录尝试次数、下一次等待和关联 UUID；`GiveUpRetryOn` 在命中指定致命错误后让策略立即耗尽。
5. `FallBack2CreateTable` 识别根因是否为 parser terror 的 `ErrInvalidDDLJob`，供 DDL 兼容回退决策使用。

## 主要符号

- `RetryableFunc<'a>`：无上下文参数、成功值为 `()` 的可变闭包，错误统一为 `SharedError`。
- `RetryableFuncV2<'a, T>`：接收 `&Context` 并返回任意 `T` 的可变闭包。`WithRetryV2` 要求 `T: Default`，因为零次尝试时要返回 Rust 的默认值以模拟 Go 的 `*new(T), nil`。
- `WithRetry`：薄封装，把无参数闭包适配为 `WithRetryV2` 的 `()` 返回值版本。
- `WithRetryV2`：核心循环；收集全部错误、检查取消、获取下一次退避，并调用 `Context::wait_cancelled_timeout`。
- `WithRetryReturnLastErr`：核心循环的“仅末错”变体；首次执行前检查取消，每次失败会调用私有 `sample_log_retry`。
- `Cancelled`：私有占位错误，固定显示为 `context canceled`；只在 `WithRetryV2` 已取消但错误集合意外为空的兜底路径，以及 `WithRetryReturnLastErr` 进入循环前已取消时使用。
- `FallBack2CreateTable`：通过 `Cause` 解包，向下转型为 `TerrorError` 后比较 `ErrInvalidDDLJob`。
- `Backoffer`：本地 TiKV Backoffer 替身；`Backoff` 当前忽略 reason/error，固定睡约 100ms 并累加 `total_sleep_ms`。
- `RetryWithBackoff`：混合退避状态，保存内部 `Backoffer`、外层累计时长、上限、基础错误和下一次请求时长。
- `AdaptTiKVBackoffer`：构造 `RetryWithBackoff`；返回值为所有权值而非 Go 的指针。
- `VerboseBackoffStrategy` / `VerboseRetry`：私有装饰器及公开工厂，为内层策略增加带 `groupID` 的日志。
- `FailedOnErr` / `GiveUpRetryOn`：私有装饰器及公开工厂，用 `Cause` 与 `errors::ErrorEqual` 判定致命错误并永久置位 `failed`。

## 执行流程

`WithRetryV2` 的顺序是：先读取剩余次数；调用业务闭包；成功则直接返回；失败则把错误追加到 `all_errors`；若上下文已取消则立刻 Join 已有错误；否则用最新错误调用 `NextBackoff`，再等待“取消或超时”。等待期间取消同样返回已聚合错误。策略耗尽后，存在历史错误则返回 Join 结果；一次都未执行则返回 `T::default()`。这里的循环次数完全由策略状态推进决定，通常由 `NextBackoff` 更新策略计数。

`WithRetryReturnLastErr` 在进入循环前先拒绝已取消上下文。循环中成功即返回；失败则克隆保存 `last_err`，计算退避并记录 Info 日志；取消发生在退避前或等待期间时都返回本次错误。策略耗尽后返回末错；零次尝试则返回成功。

`RetryWithBackoff` 的调用协议分两步：一个或多个调用者通过 `RequestBackOff(ms)` 登记等待意图，互斥区内只保留最大值；单一可变调用者随后执行 `BackOff()`，原子式取出并清零该值，先检查当前累计睡眠是否已超过上限，再实际睡眠并累加外层计数。`Inner()` 允许需要 TiKV 风格接口的代码直接操作内部 Backoffer，其睡眠计入 `TotalSleepInMS()`。

`GiveUpRetryOn` 不自行驱动重试。外层循环把错误传给 `NextBackoff` 时，它先对根因逐个做 `ErrorEqual`；命中后返回零时长并置位，下一次循环条件读取 `RemainingAttempts()` 得到 0，从而停止。未命中时完全委托内层策略。

## 数据与状态

`WithRetryV2` 的 `all_errors: Vec<Option<SharedError>>` 按发生顺序保存错误，供 `Join` 构造多错误结果；`WithRetryReturnLastErr` 只保存一个 `Option<SharedError>`。两者没有全局可变状态，每次调用拥有独立闭包与策略对象。

`RetryWithBackoff::totalBackoff` 记录通过本适配器实际消费的外层毫秒数；`Backoffer::total_sleep_ms` 记录内部 Backoff 的累计值；`TotalSleepInMS` 将二者相加。`maxBackoff` 是总睡眠上限配置，`baseErr` 是超限错误的注解基础。`nextBackoff: Mutex<i32>` 是并发生产、串行消费的槽位；相同周期内多次请求取最大值而不是求和。字段 `mu: Mutex<()>` 当前没有参与任何临界区，是为了保持与 Go 结构对称的未使用字段。

`VerboseBackoffStrategy::groupID` 在每次调用 `VerboseRetry` 时生成一次，贯穿该装饰器生命周期。`FailedOnErr::failed` 是单向状态：一旦命中致命错误便不再恢复；`failedOn` 使用 `Arc<Vec<SharedError>>` 持有只读错误集合。

## 依赖与调用关系

- 上游模块：`br/pkg/utils/lib.rs` 声明并再导出本模块；同 crate 的 `retry_test.rs`、`backoff_test.rs` 直接调用核心 API。RustCodeGraph 的文件关系还列出 `br/pkg/task/operator/crr_checkpoint_test.rs` 与 `br/pkg/utils/parity_test.rs`，但主要核心行为断言集中在前述两个测试文件。
- 策略依赖：`br/pkg/utils/backoff.rs::BackoffStrategy` 提供 `NextBackoff` 与 `RemainingAttempts`；`RetryState` 等具体策略通过这两个方法控制循环是否继续以及等待多久。
- 取消依赖：`br/pkg/utils/stubs.rs::context::Context` 用 `AtomicBool + Mutex + Condvar` 实现取消与超时等待；`cancel()` 会 `notify_all`，所以长退避可立即中断。
- 错误依赖：`astersql-errors` 提供 `SharedError`、`Join`、`Cause`、`Annotate`、`ErrorEqual`；`astersql-parser-terror` 与 `astersql-errno` 提供 DDL 错误类型和代码。
- 可观测性依赖：`astersql-br-pkg-logutil` 记录重试事件，`uuid` 为 verbose 装饰器生成关联 ID。
- 当前生产接线边界：仓库中若干备份/恢复模块包含同名局部桩或自有重试函数；例如 `br/pkg/restore/data/stubs.rs` 和 `br/pkg/backup/prepare_snap/env.rs` 各自定义 `WithRetryV2`。扩展调用面前应先确认导入指向本 crate，而不能只凭同名符号判断。

## 错误处理与边界

- `WithRetryV2` 的正常失败结果是所有已发生错误的 Join，顺序与执行顺序一致；`WithRetryReturnLastErr` 刻意丢弃早期错误。
- 上下文取消不会覆盖已收集的业务错误：`WithRetryV2` 返回聚合业务错误；`WithRetryReturnLastErr` 在执行过闭包后返回当次业务错误。仅后者在首次执行前已取消时返回 `Cancelled`。
- 零次尝试是有意保留的 Go 兼容边界：三个入口都不执行闭包且返回成功；对 `WithRetryV2<T>` 是 `Ok(T::default())`。新增非 `Default` 返回类型无法直接使用当前签名。
- `BackOff` 在睡眠前检查的是“已经累计的时长是否大于上限”，不是“累计值加本次请求是否将超过上限”。因此某次睡眠可以跨过上限，下一次 `BackOff` 才报错；`retry_test.rs::test_retry_adapter` 固化了这一行为。
- 互斥锁 poisoned 时使用 `expect` 直接 panic；负的 `ms`/`max_sleep_ms` 没有显式校验，随后向 `u64` 转换可能产生异常巨大的 Duration。调用者必须只传非负毫秒值。
- 本地 `Backoffer` 是简化实现，不解析退避原因或错误，也不检查上下文取消；不能据此声称已具备真实 `tikv::Backoffer` 的完整分类、抖动或预算行为。
- `FallBack2CreateTable` 只接受根因可向下转型为 `TerrorError` 且 code 相等的情况；其他包装或同文案错误均返回 false。

## 并发与资源生命周期

重试闭包和策略都被单次调用独占，函数本身不创建后台任务。退避等待发生在当前线程，但 `Context::wait_cancelled_timeout` 使用条件变量，因此取消方可以从另一线程唤醒等待；`retry_test.rs::cancellation_interrupts_backoff_wait` 验证 5 秒等待能在取消后 1 秒内结束。

`RetryWithBackoff::RequestBackOff(&self)` 可被多个 scoped thread 并发调用，`nextBackoff` 的 Mutex 保证最大值合并安全；`BackOff(&mut self)` 需要独占可变借用，设计上由协调者串行消费。内部 `bo` 和 `totalBackoff` 不受该 Mutex 保护，不能绕过类型约束共享可变引用。`FailedOnErr::failed` 用 Mutex 支持通过共享引用读取剩余次数，但该 trait 对象本身没有声明 `Send`/`Sync`，不要据此推导跨线程共享能力。

所有状态随策略对象或函数栈退出而释放；没有显式线程、通道、文件句柄或网络连接需要清理。日志装饰器 UUID 只在装饰器存活期间用于关联日志。

## 与 Go 版本的对应关系

主循环、零尝试成功、错误聚合、末错模式、DDL 错误识别、混合退避的最大请求合并，以及致命错误装饰器均以 `br/pkg/utils/retry.go` 为语义来源。`br/pkg/utils/retry_test.go` 的两个用例在 Rust `retry_test.rs` 中分别对应 `test_retry_adapter` 和 `test_fail_now_if`，包括约 100ms 内层睡眠、并发请求取 48ms、跨过 200ms 上限后下一次失败，以及多层 annotate 后仍能识别致命错误。

需要注意的差异如下：

- Go `WithRetryV2[T any]` 可为任意类型生成零值；Rust 以 `T: Default` 近似这一能力。
- Go 用 `multierr.Append` 累加错误，Rust 用 `Vec<Option<SharedError>> + Join`；测试通过错误序列验证可观察结果。
- Go 用 `select { ctx.Done(), time.After(...) }`；Rust 使用本地 Context 的 Condvar 等待，保留可取消语义。
- Go `WithRetryReturnLastErr` 首次取消返回原始 `ctx.Err()`；Rust 返回文案等价的私有 `Cancelled`。
- Go 使用采样日志工厂（每分钟最多三条）；Rust `sample_log_retry` 当前每次失败都写 Info，不具备相同采样限流。
- Go `RetryWithBackoff` 包装真实 `tikv.Backoffer`；Rust `Backoffer` 是固定约 100ms 的本地替身，功能明显更窄。
- Go `VerboseRetry` 接受可选 logger，正数剩余次数写 Debug；Rust 不接受 logger，使用全局日志且正数分支也写 Warn。
- Go `failedOnErr.failed` 依靠策略的串行使用；Rust 用 Mutex 包装，并用 `ErrorEqual` 对齐 Go 的错误身份语义。

## 扩展指南

- 修改重试循环、取消时机或错误返回形态时，优先改 `WithRetryV2`，再确认 `WithRetry` 的适配和 `WithRetryReturnLastErr` 的差异仍为有意设计；同步扩展独立测试 `br/pkg/utils/retry_test.rs` 或 `br/pkg/utils/backoff_test.rs`，不要把测试写进生产文件。
- 新增策略应实现 `br/pkg/utils/backoff.rs::BackoffStrategy`，明确 `NextBackoff` 是否推进剩余次数。若 `RemainingAttempts` 永远为正且闭包永不成功，循环将无限运行。
- 扩展 TiKV 适配器前应决定是继续维护本地替身，还是接入真实客户端能力；必须验证取消、总预算、错误分类、抖动和首次睡眠，不能只扩展 `Backoffer::Backoff` 的参数表面。
- 若要修复超限检查为“本次睡眠前预测”，这是可观察行为变更，需同时更新 Go 对照结论和 `test_retry_adapter`，并评估现有调用者是否依赖当前可跨限一次的语义。
- 若增强并发使用，需把 `bo`、`totalBackoff` 与 `nextBackoff` 作为一个一致状态整体设计；当前只锁下一次请求值，不能安全支持多个消费者同时执行 `BackOff`。
- 新增公开入口后同时更新 `br/pkg/utils/lib.rs` 的再导出列表，并用仓库检索确认调用者没有误用同名局部桩。
- 性能风险主要来自同步 sleep、错误向量随尝试次数线性增长以及逐次日志；高频或高尝试次数场景应评估内存、线程占用和日志量。

## 验证依据

- Rust 源码：`br/pkg/utils/retry.rs`（完整 377 行），核对全部类型别名、函数、结构体、trait 实现和边界分支。
- crate 边界：`br/pkg/utils/Cargo.toml` 与 `br/pkg/utils/lib.rs`，确认 crate 名、依赖、模块声明、公开再导出及独立测试挂载。
- 策略与取消：`br/pkg/utils/backoff.rs::BackoffStrategy`、`RetryState`，以及 `br/pkg/utils/stubs.rs::context::Context::wait_cancelled_timeout` / `cancel_state`。
- Rust 测试：`br/pkg/utils/retry_test.rs`（适配器、致命错误、零尝试、取消中断）与 `br/pkg/utils/backoff_test.rs`（成功、致命错误、耗尽、Join 顺序和末错模式）。
- Go 对照：`br/pkg/utils/retry.go`、`br/pkg/utils/retry_test.go`；两者用于核对 API 意图和可观察差异，而非推断 Rust 已完成全部生产接线。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`node --file br/pkg/utils/retry.rs` 确认文件全貌及使用文件；对 `WithRetryV2`、`WithRetry`、`WithRetryReturnLastErr`、`AdaptTiKVBackoffer`、`FallBack2CreateTable`、`VerboseRetry`、`GiveUpRetryOn` 执行带 `--file` 的 callers/callees 查询。图中 callees 确认 `WithRetry -> WithRetryV2`、末错版本调用 `sample_log_retry`、三个公开工厂分别构造其包装类型；callers 未返回可靠的生产调用边，因此另用精确仓库检索核对当前接线，并在本文明确这一限制。
- 本任务是纯文档分析，按计划未运行 Cargo；结构验证应确认目标文件存在且恰有 11 个固定二级标题。

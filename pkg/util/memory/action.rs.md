# `pkg/util/memory/action.rs`

## 文件定位

本文件属于 `astersql-util-memory` crate 的内存配额动作层，由 [`lib.rs`](lib.rs) 以 `pub mod action` 暴露。它定义一套共享、线程安全的 `ActionOnExceed` 接口，以及日志告警、查询取消和 fallback 链所需的公共实现；上游在内存 `Tracker` 超过配额时选择并触发这些动作。

当前仓库存在一个重要迁移边界：本文件的接口使用 `&self`、`Arc<Mutex<dyn ActionOnExceed + Send>>`，而 [`tracker.rs`](tracker.rs) 仍定义另一套使用 `&mut self`、`Box<dyn ActionOnExceed>` 的同名接口。会话运行时通过 [`pkg/session/runtime.rs`](../../session/runtime.rs) 的 `RuntimePanicOnExceed` 将本文件的 `PanicOnExceed` 适配给后者；不能把两套同名 trait 当成同一个类型。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名是 `astersql-util-memory`，库入口为 `lib.rs`；本文件直接依赖 crate 内再导出的 `logutil_crate` 和 `sqlkiller_crate`，并读取同 crate 的 `tracker::Tracker`。`mem-arbitrator` feature 不改变本文件的条件编译，本文件自身没有 `#[cfg]` 分支。

## 核心职责

1. `ActionOnExceed` 规定超限动作必须能执行、挂接/读取 fallback、报告优先级并维护完成状态（`action.rs:50-57`）。
2. `actionWithPriority` / `NewActionWithPriority` 在不改变底层行为的前提下覆盖排序优先级，其余方法全部转发给被包装动作（`action.rs:60-94`）。
3. `BaseOOMAction` 保存公共的 fallback 指针和 finished 标志，读取 fallback 时原地跳过已经完成的链节点（`action.rs:98-149`）。
4. `LogOnExceed` 保证同一实例只输出一次超限告警或测试钩子（`action.rs:175-246`）。
5. `PanicOnExceed` 保证日志/钩子只执行一次，但每次调用都向 `SQLKiller` 写入 `QueryMemoryExceeded` 并处理信号；`HandleSignal` 返回错误时以 panic 传播（`action.rs:248-333`）。

这些动作不负责检测配额是否超限，也不修改 `Tracker` 的计数；超限判断和动作编排属于 tracker/会话层。

## 主要符号

- `lock_unpoisoned<T>`：统一获取互斥锁。遇到 poison 时取回内部守卫继续执行，而不是再次 panic（`action.rs:35-40`）。
- `ActionHandle = Arc<Mutex<dyn ActionOnExceed + Send>>`：可在线程和 fallback 链之间共享的动态动作句柄（`action.rs:42-44`）。trait 自身已经要求 `Send`，别名中的 `+ Send` 重申了该边界；它没有要求 `Sync`，同步由外层 `Mutex` 提供。
- `ActionOnExceed`：公开 trait。`Action` 接收只读 `Tracker`；fallback 方法使用共享句柄；`GetPriority` 用于链排序；finished 方法用于从链中惰性摘除动作（`action.rs:50-57`）。
- `NewActionWithPriority(action, priority) -> actionWithPriority`：公开构造函数返回私有具体类型。包装器覆盖 `GetPriority`，并在持有底层动作互斥锁时转发其他方法（`action.rs:60-94`）。
- `BaseOOMAction`：由 `Mutex<Option<ActionHandle>>` 和 `AtomicBool` 组成，`Default` 创建空 fallback、未完成状态（`action.rs:98-110`）。
- `BaseOOMAction::{SetFallback, GetFallback, TriggerFallBackAction}`：写链头、压缩已完成节点、触发第一个未完成节点（`action.rs:112-149`）。
- `DefPanicPriority`、`DefLogPriority`、`DefSpillPriority`、`DefCursorFetchSpillPriority`、`DefRateLimitPriority`：值依次为 `0..=4`。在 Go/现有 tracker 排序语义中，数值越大越靠前；因此 panic 是最低默认优先级，限速是最高默认优先级（`action.rs:151-160`；`tracker.rs:515-535`）。
- `LogHook`：接收连接 ID 的 `Send + Sync` 闭包，仅供模块内部字段和公开 setter 使用（`action.rs:162-163`）。
- `memory_exceeded_fields`：从 tracker 提取 `label`、`consumed`、`quota` 和字符串化 tracker，供两个动作构造日志字段（`action.rs:165-173`）。
- `LogOnExceed::{new, SetLogHook}`：保存连接 ID、一次性状态、可选 hook 和公共 fallback 状态（`action.rs:177-205`）。
- `PanicOnExceed::{new, SetLogHook}`：保存共享 `SQLKiller`、连接 ID、一次性状态、可选 hook 和公共 fallback 状态（`action.rs:250-284`）。`Default` 的 `Killer` 为 `None`，只适合随后补齐或用于构造；直接执行会 panic。

## 执行流程

### fallback 链

1. 调用方用 `SetFallback` 替换当前链头。
2. `GetFallback` 克隆链头句柄，并在其互斥锁下读取 `IsFinished`。
3. 若节点仍存活，立即返回该句柄；若已完成，则读取该节点自己的 fallback，以它替换当前链头并继续循环。
4. `TriggerFallBackAction` 对最终得到的首个未完成节点调用 `Action`；空链不执行任何操作（`action.rs:130-148`）。

该过程会压缩已完成的链前缀，但不会遍历或删除首个未完成节点之后的内容。独立测试 `fallback_chain_skips_finished_actions_and_triggers_first_live_action` 构造三个节点，确认前两个完成后返回并触发第三个（`action_1_aster_unit_test.rs:109-128`）。

### 一次性日志动作

1. `LogOnExceed::Action` 先锁住 `acted`；已执行过就直接返回。
2. 首次调用在锁内把 `acted` 置为 `true`。
3. 若配置了 hook，则传入 `ConnID`；否则用 `BgLogger` 记录 `Warn` 级别的 `memory exceeds quota` 及四个 tracker 字段（`action.rs:207-225`）。

### 查询取消动作

1. `PanicOnExceed::Action` 锁住 `acted`。首次调用执行 hook，或记录包含 tracker 会话 ID 及公共超限字段的警告；随后把 `acted` 置为 `true`。
2. 从 `Killer` 取出 `SQLKiller`。若为 `None`，以明确的 `expect` 消息 panic，以模拟 Go nil 指针失败。
3. 每次调用都发送 `QueryMemoryExceeded`，再调用 `HandleSignal`；若返回错误，执行 `panic!("{error}")`（`action.rs:286-311`）。
4. 会话执行入口在 [`pkg/session/runtime/dispatch.rs`](../../session/runtime/dispatch.rs) 创建 statement tracker；当全局 OOM action 为 cancel 时构造 `PanicOnExceed::new`，包入 `RuntimePanicOnExceed` 后交给 tracker（`dispatch.rs:4009-4027`）。

## 数据与状态

- fallback 所有权由 `Arc` 共享，链边存放在每个 `BaseOOMAction.fallbackAction` 的 `Mutex<Option<_>>` 中。替换链头会释放旧 `Option` 持有的引用；只要其他 `Arc` 仍存在，动作对象就不会销毁。
- `finished: AtomicBool` 使用 `SeqCst` 读写，为跨线程的 finished 可见性提供最强顺序保证（`action.rs:119-126`）。finished 只会由 `false` 变为 `true`，没有重置 API。
- `LogOnExceed.acted` 与 `PanicOnExceed.acted` 分别由独立 `Mutex<bool>` 保护。二者的 hook 也在独立互斥锁中；setter 替换旧 hook，没有清空 hook 的公开方法。
- `ConnID` 用于 hook；`PanicOnExceed` 默认日志中的 `conn` 字段来自 `tracker.SessionID.Load()`，不是结构体的 `ConnID`（`action.rs:291-299`）。
- 优先级包装器保存原始 `ActionHandle`，因此包装前后观察到同一个底层 finished/fallback/执行状态；只有 `GetPriority` 的返回值属于包装器自身。

## 依赖与调用关系

上游与接线：

- `lib.rs:23-24` 将模块公开为 `astersql_util_memory::action`；`lib.rs:51-53` 把独立测试文件挂入 crate 测试构建。
- `pkg/session/runtime.rs:430-457` 的 `RuntimePanicOnExceed` 是当前生产桥接点：`Action`、优先级和 finished 状态转发到本文件接口，但适配器的 fallback setter/getter 分别丢弃输入和固定返回 `None`。
- `pkg/session/runtime/dispatch.rs:4017-4027` 根据 `tidb_mem_quota_query` 和 OOM action 配置创建 statement tracker，并在 cancel 分支实例化本文件的 `PanicOnExceed`。
- RustCodeGraph 对 `action.rs` 的文件查询显示 18 个使用文件；精确符号查询把直接覆盖定位到 `action_1_aster_unit_test.rs`，并确认 `PanicOnExceed` 被 `pkg/session/runtime.rs` 导入。由于同仓存在 Go/Rust 重名符号，调用关系需结合上述精确文件位置判读。

下游依赖：

- `tracker::Tracker` 提供标签、消费字节、限额、字符串描述和 `SessionID`，仅作为日志与 hook 的上下文。
- `logutil::log::{BgLogger, LogField, LogLevel}` 提供后台结构化日志。
- `sqlkiller::{SQLKiller, QueryMemoryExceeded}` 提供查询取消信号与错误转换。
- 标准库 `Arc`、`Mutex`、`AtomicBool` 实现共享所有权和同步；本文件不启动线程、不创建异步任务，也不执行 I/O（日志后端行为除外）。

## 错误处理与边界

- 所有本文件内部互斥锁都经 `lock_unpoisoned` 获取，因此 hook panic 或其他持锁 panic 造成 poison 后，后续调用仍继续使用保存的状态。调用者不能依赖 poison 阻止后续动作。
- `PanicOnExceed::default()` 没有 killer；调用其 `Action` 会在 `expect` 处 panic。生产路径应使用 `PanicOnExceed::new(Arc<SQLKiller>, conn_id)`。
- `SQLKiller::HandleSignal` 的 `Err` 被转换成字符串 panic，而不是作为 `Result` 返回；这是该接口的有意控制流，也是 Rust 测试用 `catch_unwind` 验证的行为（`action_1_aster_unit_test.rs:159-186`）。
- `LogOnExceed` 首次 hook 或日志调用若 panic，`acted` 已经先置为 `true`；恢复后再次调用不会重试。`PanicOnExceed` 则在 hook/日志之后才写 `acted = true`，因此 hook/日志本身 panic 时该标志仍为 `false`，下次会重试首次日志路径。
- `GetFallback` 假定 fallback 链无环。若形成只含 finished 节点的环，循环无法收敛；API 没有环检测。扩展代码必须避免把某节点直接或间接设为自己的 fallback。
- `TriggerFallBackAction` 不自动把当前动作标记完成，也不捕获 fallback 的 panic；何时完成、何时委托由具体动作实现决定。
- 当前生产适配器 `RuntimePanicOnExceed` 不保留本文件的 fallback 链，因此不能假设在会话 cancel 路径上调用本文件 `SetFallback` 会生效（`pkg/session/runtime.rs:440-444`）。

## 并发与资源生命周期

- `ActionHandle` 允许多个线程共享同一动作，方法调用通过对象外层 `Mutex` 串行化；动作内部状态还各自使用锁或原子变量。实现新的动作时仍必须满足 `Send` 并自行保证从其他持有方式调用时的线程安全语义。
- `LogOnExceed::Action` 在持有 `acted` 锁期间调用 hook/日志，因此同一实例的并发调用严格一次生效。12 个线程并发调用的测试确认 hook 计数为 1（`action_1_aster_unit_test.rs:130-157`）。
- `PanicOnExceed::Action` 同样在 `acted` 锁期间执行 hook、发信号和处理信号。即使 `HandleSignal` panic，`lock_unpoisoned` 也允许后续调用重新取得 poisoned 锁；测试连续捕获两次 panic，确认 hook 仍只调用一次且 kill 信号保持为 `QueryMemoryExceeded`（`action_1_aster_unit_test.rs:159-186`）。
- `BaseOOMAction::GetFallback` 每轮只短暂持有自己的 fallback 锁，随后分别锁 fallback 对象读取状态和下一跳；它不会同时持有当前 `BaseOOMAction.fallbackAction` 与 fallback 对象锁。仍应避免在动作方法内反向取得已由调用方持有的同一个 `ActionHandle`，否则非重入 `Mutex` 可能死锁。
- 本文件没有显式析构逻辑。动作与 hook 捕获资源在最后一个 `Arc`/`Box` 引用释放时按 Rust RAII 销毁；fallback 环还会形成 `Arc` 强引用环并泄漏，因此无环也是资源生命周期约束。

## 与 Go 版本的对应关系

直接对照文件是 [`action.go`](action.go)。Rust 保留了 Go 的主要公共语义：`ActionOnExceed` 六个方法、优先级包装、finished 节点跳过、五档默认优先级、一次性日志，以及 `PanicOnExceed` 每次发送 kill 信号并 panic 传播 `HandleSignal` 错误。

主要实现差异如下：

- Go 接口值和指针由 GC 管理；Rust 版本用 `Arc<Mutex<dyn ...>>` 表达共享动态动作，并用 `Option` 表达 Go 的 nil fallback。
- Go `BaseOOMAction.finished` 是 `int32` 原子；Rust 是 `AtomicBool`，两者都只表达二态完成标志。Go 的 fallback 字段未单独加锁；Rust 为它增加 `Mutex`。
- Go `actionWithPriority` 通过匿名嵌入自动转发接口；Rust 显式实现并转发每个 trait 方法。
- Go 日志构造 `errMemExceedThreshold` 错误并交给 zap；Rust 直接输出 `label/consumed/quota/tracker` 结构化字段，没有在本文件构造对应的 `dbterror`。
- Go 的两个动作使用同一个结构体 mutex 保护 `acted` 及整个 `Action`；Rust 分开保护 `acted` 和 hook，但 `Action` 同样在 `acted` 锁内执行 hook/日志。Rust setter 可与 `Action` 并发安全地替换 hook，而 Go `SetLogHook` 本身不加锁。
- Go 测试 `tracker_test.go:230-247` 验证 fallback 链会跳过连续 finished 节点；Rust 的 `action_1_aster_unit_test.rs:109-128` 复现该核心意图，并额外覆盖并发一次性 hook、优先级包装和 SQLKiller 信号。
- 当前 Rust tracker 另有尚未合并的旧式 action trait；Go 包内只有一套 `ActionOnExceed`。这是迁移状态差异，不应据此扩写或删除任一接口。

## 扩展指南

- 新增超限动作时，实现本文件 `ActionOnExceed`，优先复用 `BaseOOMAction` 保存 fallback/finished 状态，并在独立的 `*_test.rs` 文件中覆盖首次执行、重复执行、fallback、完成状态和并发行为；不要把测试写回生产源文件。
- 新增默认优先级时，应同时检查 `tracker.rs` 的降序合并规则、会话适配层和 Go 对照常量，验证相同优先级时的稳定顺序及与既有 spill/rate-limit 动作的相对位置。
- 修改 `GetFallback` 时必须保持“跳过连续 finished 前缀并压缩链”的行为，并增加空链、全 finished 链、长链和并发 `SetFinished` 的测试；若要支持环，应明确引入身份集合或弱引用，不能只给循环增加次数上限。
- 修改日志字段或 killer 行为时，应同步核对 `action.go`、`sqlkiller` 的错误身份，以及 `pkg/session/runtime/dispatch.rs` 的 OOM action 选择。错误兼容风险主要是 panic 文本/错误类型和日志字段变化。
- 若要统一本文件与 `tracker.rs` 的两套 trait，需要把它视为跨模块重构：同时处理 `Box` 与 `Arc<Mutex>` 所有权、`&mut self` 与内部可变性、fallback 保真及全部 tracker 实现。尤其要消除 `RuntimePanicOnExceed` 当前丢弃 fallback 的局部接线，不能仅改类型别名。
- 性能上应关注热路径锁竞争：hook、日志和 `HandleSignal` 当前都在 `acted` 锁内执行；若移出锁，必须先设计一次性状态机，证明不会重复日志或发生 killer 顺序回归。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件完整索引为 333 行，并报告被 18 个文件使用。
- RustCodeGraph 源码/符号查询：`node --file pkg/util/memory/action.rs`；`query ActionOnExceed`、`NewActionWithPriority`、`GetFallback`、`TriggerFallBackAction`、`LogOnExceed`、`PanicOnExceed`；以及对 `BaseOOMAction`、`LogOnExceed`、`PanicOnExceed` 等执行的 callers/callees 查询。重名导致部分图查询无精确调用输出，因此又以精确文件节点和文本引用核验接线。
- 已读生产源码：`pkg/util/memory/action.rs`、`pkg/util/memory/tracker.rs`、`pkg/util/memory/lib.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs`。
- 已读 crate 配置：`pkg/util/memory/Cargo.toml`，确认包名、库入口、feature 和 `logutil`/`sqlkiller` 路径依赖。
- 已读 Go 对照：`pkg/util/memory/action.go`；已读 Go 测试证据：`pkg/util/memory/tracker_test.go:230-266`。
- 已读独立 Rust 测试：`pkg/util/memory/action_1_aster_unit_test.rs`，覆盖优先级包装、finished fallback 跳过、并发一次性日志和 SQLKiller panic 路径。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收通过后，文档应恰好包含本页所列 11 个固定二级标题。

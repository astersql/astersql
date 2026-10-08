# [`pkg/util/syncutil/mutex_deadlock.rs`](./mutex_deadlock.rs)

## 文件定位

本文件是 `astersql-util-syncutil` crate 的“启用死锁检测”锁实现，对应 Go 的 `pkg/util/syncutil/mutex_deadlock.go`。crate 边界由 `pkg/util/syncutil/Cargo.toml` 定义；唯一外部依赖是启用了 `deadlock_detection` feature 的 `parking_lot 0.12`。

`pkg/util/syncutil/lib.rs` 始终公开 `mutex_deadlock` 模块，但只有 crate feature `deadlock` 开启时，才在 crate 根通过 `pub use mutex_deadlock::*` 把本文件的 `Mutex`、`RWMutex` 和 `EnableDeadlock` 选为默认锁表面；未开启时，crate 根改为再导出 `mutex_sync.rs`。因此常规上游应使用 crate 根导出，直接访问 `mutex_deadlock` 模块则会绕过 feature 选择。

## 核心职责

- `Mutex<T>` 和 `RWMutex<T>` 透明包装 `parking_lot::Mutex<T>` / `parking_lot::RwLock<T>`，保留其加锁 API，同时保证首次正常构造锁时启动一次进程级死锁检测器。
- `init()` 每隔 `DEADLOCK_TIMEOUT`（20 秒）调用 `parking_lot::deadlock::check_deadlock()`；没有死锁时继续轮询，发现死锁时打印参与线程及回溯后终止进程。
- `EnableDeadlock = true` 向上层暴露与 Go 构建标签相同的能力标志；`DEADLOCK_TIMEOUT` 把 Go 中的 20 秒配置显式化。
- `detector_started()` 仅在测试编译中暴露，用于验证惰性初始化是否发生，不属于生产 API。

## 主要符号

- `static START_DETECTOR: Once`：保护检测线程的进程级一次性启动。所有 `Mutex<T>` 与 `RWMutex<T>` 构造共享同一个实例。
- `static DETECTOR_STARTED: AtomicBool`：在启动闭包进入后以 `Release` 写入；只供测试辅助函数以 `Acquire` 读取。
- `pub struct Mutex<T: ?Sized>(parking_lot::Mutex<T>)`：`#[repr(transparent)]` 的互斥锁新类型。`Mutex::new(T)` 先调用 `init()`，`Mutex::into_inner(self)` 消耗包装并返回数据；`Default` 委托给 `new`；`Deref`/`DerefMut` 将其余锁操作交给内部 `parking_lot` 锁。
- `pub struct RWMutex<T: ?Sized>(parking_lot::RwLock<T>)`：结构与 `Mutex` 对称，提供读写锁语义。公开名称保留 Go 的 `RWMutex` 拼写，底层 Rust 类型是 `RwLock`。
- `pub const EnableDeadlock: bool = true`：deadlock 变体的兼容标志。
- `pub const DEADLOCK_TIMEOUT: Duration = Duration::from_secs(20)`：检测轮询周期，也是与 Go 配置对齐的时间常量。
- `pub fn init()`：启动名为 `syncutil-deadlock-detector` 的后台线程；由两个包装类型的 `new` 间接调用，也可显式调用。
- `pub(crate) fn detector_started() -> bool`：受 `#[cfg(test)]` 约束的状态探针。

## 执行流程

1. 调用者通过 `Mutex::new(value)`、`RWMutex::new(value)` 或相应的 `Default::default()` 构造包装锁。
2. 构造器先进入 `init()`；`START_DETECTOR.call_once` 保证并发构造多个锁时只有一个调用者执行启动闭包，其余调用者等待或观察已完成状态。
3. 启动闭包设置 `DETECTOR_STARTED`，然后用 `std::thread::Builder` 创建具名后台线程。创建失败由 `expect` 转为 panic，锁构造不会继续。
4. 后台线程无限循环：先休眠 20 秒，再执行 `parking_lot::deadlock::check_deadlock()`。
5. 返回集合为空时进入下一轮；非空时先打印死锁数量，再逐组打印涉及线程数、线程 ID 和回溯，最后调用 `std::process::abort()`，不再尝试恢复或继续服务。
6. `init()` 成功返回后，构造器创建底层 `parking_lot` 锁。后续 `lock`、`try_lock`、`read`、`write` 等调用通过 `Deref` 直接使用底层实现；本文件不在每次加锁时增加一层业务逻辑。

`into_inner` 不调用 `init()`，因为它只能消费已经通过私有字段构造出的包装值；正常安全 Rust 调用路径在更早的 `new`/`Default` 阶段已经初始化检测器。

## 数据与状态

包装类型只持有一个底层锁，没有额外的逐锁元数据；`#[repr(transparent)]` 保持单字段透明布局，但字段是私有的，外部不能直接绕过构造器组装实例。`T: ?Sized` 允许类型本身表达未定长目标，而 `new`/`into_inner` 仅在有大小的 `T` 实现块中提供。

检测状态是进程级而不是 crate 调用者级：`START_DETECTOR` 决定最多启动一次，`DETECTOR_STARTED` 只表示启动闭包已经执行，并不表示线程已完成一次扫描。实际等待图与回溯由 `parking_lot` 的 deadlock 子系统维护，本文件只定期读取检测结果。

锁的受保护值、锁守卫与读写互斥规则都由 `parking_lot` 管理。本文件没有错误值、通道、异步任务、事务状态或可配置的停止标志。

## 依赖与调用关系

上游入口是 `pkg/util/syncutil/lib.rs`：feature `deadlock` 开启时再导出本文件全部公开符号，默认构建则选用 `mutex_sync.rs`。仓库中 `pkg/session`、`pkg/server`、`pkg/domain`、`pkg/executor` 等多个 crate 的 Cargo manifest 直接依赖 `astersql-util-syncutil`；具体业务调用通常只看到 crate 根的 `Mutex`/`RWMutex`，例如 `pkg/executor/test/oomtest/oom_test.rs` 使用 `astersql_util_syncutil::Mutex`。这意味着 feature 选择可以在不改业务锁调用点的情况下切换实现。

本文件内部的关键调用边为：`Mutex::new` / `RWMutex::new` → `init` → `Once::call_once` → `thread::Builder::spawn` → 循环中的 `sleep` 和 `parking_lot::deadlock::check_deadlock`；发现死锁后进入 `eprintln!` 报告链并最终调用 `process::abort`。两个 `Default` 实现分别调用各自的 `new`，两个 `Deref` 实现则把操作下沉到 `parking_lot`。

RustCodeGraph 将目标文件识别为 26 个符号，并显示它经 crate 导出被大量 Rust 文件使用；对精确的 `Mutex`/`init` callers/callees 查询会受到仓库内同名符号影响，未将宽查询中的无关 `lock` 结果当作本文件调用证据。直接依赖和 feature 边界以 `lib.rs`、Cargo manifest 与精确导入检索交叉核对。

## 错误处理与边界

- 检测线程创建失败时，`expect("failed to start syncutil deadlock detector")` 触发 panic；因为失败发生在 `Once::call_once` 闭包内，`Once` 会被毒化，后续再次调用 `init()` 也不会静默恢复。此时原子标志已经被写成 true，所以它只能作为“启动闭包已进入”的测试信号，不能证明线程创建成功。
- 检测到死锁不是返回 `Result` 的可恢复错误：实现先尽力向标准错误输出诊断信息，然后无条件 `abort`。析构函数、panic 展开和应用级清理逻辑都不会运行。
- 报告输出使用 `eprintln!`，没有接入结构化日志、指标或可替换回调；输出失败也没有独立处理路径。
- 第一次扫描发生在检测线程启动约 20 秒后，因此 `DETECTOR_STARTED == true` 不代表已经检测过死锁；短于该窗口的等待可能尚未被报告。
- `parking_lot` 锁不采用标准库的 poisoning 语义。相关测试对普通变体验证了持锁线程 panic 后仍可使用；deadlock 变体共享相同底层锁类别，但当前独立测试没有专门对该变体重复 panic 恢复场景。
- 本实现只检测 `parking_lot` deadlock 子系统能观察到的锁关系，不应把它解释为能检测任意阻塞、通道等待、异步任务停滞或外部系统死锁。

## 并发与资源生命周期

`Once` 是并发初始化的核心不变量：无论多少线程同时构造 `Mutex` 和 `RWMutex`，成功路径只产生一个检测线程。`DETECTOR_STARTED` 的 Release/Acquire 配对让测试线程可靠观察启动闭包已执行；它不参与业务同步。

检测线程没有保存 `JoinHandle`，也没有退出信号。正常情况下它与进程同寿命，每轮休眠会占用一个原生线程但不持续占用 CPU；发生死锁时它终止整个进程。包装锁及其守卫仍按 `parking_lot` 的 RAII 规则释放，`into_inner` 通过取得所有权取回受保护数据。

扩展时必须保持“先成功初始化检测器，再返回首个包装锁”的顺序，避免上层以为检测已启用而后台线程尚未成功创建。若引入可停止检测器、可调周期或测试重置能力，需要同时处理全局 `Once`、线程句柄、跨测试并发以及进程退出阶段的资源归属。

## 与 Go 版本的对应关系

Go 文件 `pkg/util/syncutil/mutex_deadlock.go` 仅在 `//go:build deadlock` 下编译，设置 `EnableDeadlock = true`，在包级 `init()` 中把 `github.com/sasha-s/go-deadlock` 的全局 `DeadlockTimeout` 配为 20 秒，并嵌入其 `Mutex`/`RWMutex`。默认 Go 文件 `mutex_sync.go` 则嵌入标准库 `sync` 锁并把标志设为 false。

Rust 用 Cargo feature 和 `lib.rs` 再导出模拟构建标签，用 `parking_lot` 替代 `go-deadlock`。API 意图保持一致：上层获得同名互斥锁/读写锁和能力标志，超时值保持 20 秒；但运行机制并非逐行等价。Rust 没有包级初始化，故在包装锁构造器中惰性调用 `init()`，并显式创建扫描线程；Go 依赖库自身在锁操作中完成检测。本 Rust 实现发现死锁后明确 `abort`，而这一终止策略不是 Go 文件本身直接表达的逻辑。

Go 同目录没有独立 `*_test.go`。Rust 的直接测试位于 `mutex_deadlock_test.rs`，迁移对齐测试位于 `migration_aster_unit_test.rs`；后者还比较 deadlock/sync 两个变体的常量、公共锁表面和基本互斥行为。

## 扩展指南

- 修改检测启动、周期、报告或终止策略时，以 `init()`、`START_DETECTOR` 和 `DEADLOCK_TIMEOUT` 为主要接入点，并在 `mutex_deadlock_test.rs` 增加独立测试；不要把测试嵌入生产源文件。
- 修改锁构造或包装表面时，同时维护 `Mutex` 与 `RWMutex` 的 `new`、`Default`、`into_inner`、`Deref`/`DerefMut` 对称性，并在 `migration_aster_unit_test.rs` 验证 deadlock 与 sync 变体仍可由相同上层调用方式使用。
- 修改 feature 或公开导出时同步检查 `lib.rs` 和 `Cargo.toml`。尤其要确认默认无 deadlock 构建仍导出唯一的锁表面，并评估直接引用公开 `mutex_deadlock` 模块的调用者。
- 改动 20 秒语义或 `EnableDeadlock` 时必须对照 `mutex_deadlock.go`；若有意产生差异，应明确记录兼容性原因，而不是把 Rust 特有机制描述成 Go 现状。
- 性能风险主要来自全局检测扫描、回溯收集和额外原生线程；正确性风险主要来自一次性初始化失败、报告后立即中止以及更改包装布局/API 后破坏已有调用。新增可配置状态时还需定义何时读取配置及并发更新规则。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件，`files --filter pkg/util/syncutil` 确认目标模块、Go 对照和两份 Rust 测试均已索引；`node --file pkg/util/syncutil/mutex_deadlock.rs` 读取完整 155 行及 26 个符号；精确 `query` 定位 `Mutex`、`RWMutex`、`EnableDeadlock`、`DEADLOCK_TIMEOUT`、`init`、`detector_started`。同名调用图查询没有稳定返回文件限定边，因此内部调用边以已索引源码节点为准，没有采用宽 `explore` 的跨模块同名噪声。
- crate 与模块边界：`pkg/util/syncutil/Cargo.toml`、`pkg/util/syncutil/lib.rs`、`pkg/util/syncutil/BUILD.bazel`。
- Go 对照：`pkg/util/syncutil/mutex_deadlock.go`、`pkg/util/syncutil/mutex_sync.go`。
- Rust 对照与测试：`pkg/util/syncutil/mutex_sync.rs`、`pkg/util/syncutil/mutex_deadlock_test.rs`、`pkg/util/syncutil/migration_aster_unit_test.rs`。
- 上游依赖抽查：根 `Cargo.toml` 及 `pkg/session/Cargo.toml`、`pkg/server/Cargo.toml`、`pkg/domain/Cargo.toml`、`pkg/executor/Cargo.toml` 等 manifest；精确导入示例为 `pkg/executor/test/oomtest/oom_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文件存在且固定二级标题恰好为 11 个，并人工复核所有行为结论均可回溯到上述符号或文件。

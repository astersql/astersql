# `pkg/store/driver/test_state.rs`

## 文件定位

[`test_state.rs`](test_state.rs) 是 `astersql-store-driver` crate 的测试专用状态协调模块。crate 入口在 [`lib.rs`](lib.rs) 中以 `#[cfg(test)]` 和 `#[path = "test_state.rs"] mod test_state;` 挂载它，因此普通库构建和线上 SQL/存储请求链不会编译或执行本文件；它只存在于该 crate 的单元测试二进制中。

该文件虽然位于生产目录并由计划纳入逐文件说明，但其真实角色不是 TiKV driver 的运行时实现，而是让会修改进程级配置或 failpoint 的 Rust 移植测试串行执行。crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[package] name = "astersql-store-driver"` 与 `[lib] path = "lib.rs"` 确定；本模块只使用 Rust 标准库，不直接依赖清单中的 TiKV、Tokio 或其他 workspace crate。

## 核心职责

文件只提供 `pub(crate) fn global_state_guard() -> MutexGuard<'static, ()>`。调用测试在修改共享状态前取得返回的 guard，并让 guard 在测试作用域结束时自动释放，从而避免同一测试进程内多个相关用例并行覆盖全局配置、常量标签或 failpoint。

其第二项职责是让互斥锁在持锁线程 panic 后仍可继续使用：标准库 `Mutex` 会因 panic 被标记为 poisoned，函数通过 `PoisonError::into_inner()` 取回 guard，而不是再次 panic。这个选择由 [`test_state_test.rs`](test_state_test.rs) 的 `global_state_guard_remains_usable_after_a_panicking_holder` 直接验证。

## 主要符号

- `global_state_guard() -> MutexGuard<'static, ()>`：唯一公开符号，可见性为 `pub(crate)`，只允许当前 crate 内的测试模块调用。返回 guard 而不是布尔值或显式解锁句柄，使释放动作绑定到 Rust 的 RAII 生命周期。
- `LOCK: OnceLock<Mutex<()>>`：函数内静态变量，首次调用时惰性创建一个无数据载荷的 `Mutex<()>`；后续调用复用同一把进程内锁。函数内定义避免把锁本身暴露为模块 API。
- `Mutex<()>`：载荷 `()` 表明锁不保存业务数据，只表达“进入全局测试状态临界区”的排他权。
- `MutexGuard<'static, ()>`：guard 借用静态锁，因此类型带 `'static`；这表示锁对象的存活期覆盖测试进程，并不表示调用方必须永久持锁。

文件没有常量、结构体、枚举、trait、`impl` 或额外条件编译项。条件编译发生在上游 [`lib.rs`](lib.rs) 的模块声明处。

## 执行流程

1. 测试在接触共享状态前调用 `crate::test_state::global_state_guard()`，例如 [`config_test.rs`](config_test.rs) 的 `TestSetDefaultAndOptions`。
2. `OnceLock::get_or_init` 检查静态 `LOCK` 是否已经初始化；第一次调用以 `Mutex::new(())` 创建它，之后直接返回同一个 `Mutex` 引用。
3. `Mutex::lock` 阻塞当前线程，直到此前 guard 被释放。成功时函数直接返回 `MutexGuard`。
4. 如果此前的持锁线程在临界区 panic，`lock` 返回 `PoisonError`；`unwrap_or_else(|poisoned| poisoned.into_inner())` 忽略 poisoned 标志并取回 guard，使后续测试仍能进入临界区。
5. 调用方通常把结果绑定为 `_guard`、`_serial` 等局部变量。变量离开作用域或线程 panic 展开栈时，`MutexGuard::drop` 释放锁；源码中没有手动 unlock。

当前已核实的临界区包括：[`config_test.rs`](config_test.rs) 修改 `set_global_config` 与 `set_const_labels`；[`driver_lifecycle_test.rs`](driver_lifecycle_test.rs) 的两个 driver 生命周期用例读取或依赖进程级配置；[`sql_fail_test.rs`](sql_fail_test.rs) 的 `TestFailBusyServerCop` 启用 `tikvclient/rpcServerBusy` failpoint。它们都必须在所有共享状态访问之前取得 guard，才能形成完整串行边界。

## 数据与状态

模块自身唯一的持久状态是静态 `OnceLock<Mutex<()>>`。`OnceLock` 保证并发首次初始化只产生一把锁；`Mutex` 保证任一时刻最多一个协作调用者持有 guard。锁内没有共享值，真正受保护的数据位于其他模块，例如 driver 的 `GlobalConfig`、metrics 常量标签以及测试 failpoint 注册表。

不变量是：所有需要隔离这些进程级状态的测试必须自愿使用同一 `global_state_guard`，并在整个读取、修改、断言和清理区间持续持有 guard。锁不会自动识别或保护绕过 helper 的状态访问。当前 `TestSetDefaultAndOptions` 在释放前显式恢复空标签和默认配置；guard 提供并发隔离，但不替代状态清理。

锁的作用域是单个 Rust 测试进程。不同进程各有自己的静态变量，不能用它协调跨进程资源；同一 crate 的一次测试执行中，各调用者共享该实例。

## 依赖与调用关系

上游装配边为 [`lib.rs`](lib.rs) `#[cfg(test)] -> mod test_state`。该模块不是 crate 的 `pub use` 项，函数也仅为 `pub(crate)`，因此外部 crate 无法把它当成公共测试工具。

已通过 RustCodeGraph 与源码检索核实的直接调用者为：

- [`config_test.rs`](config_test.rs) `TestSetDefaultAndOptions -> global_state_guard`；保护全局 driver 配置和常量 labels 的修改、断言及恢复。
- [`driver_lifecycle_test.rs`](driver_lifecycle_test.rs) `open_cache_gc_lock_wait_and_idempotent_close_match_go_lifecycle -> global_state_guard`；保护默认全局配置以及依赖它创建 driver/store 的完整生命周期。
- [`driver_lifecycle_test.rs`](driver_lifecycle_test.rs) `disabled_gc_does_not_start_worker_and_options_delete_on_none -> global_state_guard`；与同文件前一用例串行，避免 driver 创建期间观察到别的用例写入的全局配置。
- [`sql_fail_test.rs`](sql_fail_test.rs) `TestFailBusyServerCop -> global_state_guard`；覆盖启用 failpoint、后台线程撤销 failpoint、请求完成和资源关闭的整个区间。
- [`test_state_test.rs`](test_state_test.rs) `global_state_guard_remains_usable_after_a_panicking_holder -> global_state_guard`；先在线程内持锁 panic，再从当前线程重新获取。

下游只调用 `std::sync::{OnceLock, Mutex, MutexGuard}` 的初始化、加锁和中毒恢复能力，没有 I/O、网络或 storage 调用。RustCodeGraph 的文件关系确认 [`test_state.rs`](test_state.rs) 被 `sql_fail_test.rs` 和 `test_state_test.rs` 直接识别；由于图的 caller 摘要未列全所有 `use`/调用，最终调用者清单又以 `rg 'global_state_guard\(' --glob '*.rs'` 交叉核对。

## 错误处理与边界

函数没有 `Result` 返回值。正常锁竞争通过阻塞等待解决；锁中毒是唯一显式处理的异常分支，处理策略是取回内部 guard 并继续，而不是传播或 panic。这适合测试协调锁：它不承载需要修复的一致性数据，受保护的实际全局状态应由各测试自行清理或重新设置。

边界与风险如下：

- helper 不会恢复全局配置、labels 或 failpoint；调用者仍需显式清理，最好使用作用域 guard/cleanup 机制覆盖提前返回和 panic。
- `std::sync::Mutex` 不可重入；同一线程持有 guard 时再次调用本函数会等待自己释放锁并造成死锁。
- 只有使用该 helper 的测试会被串行化；新增的共享状态测试若遗漏调用，仍可能产生竞态或偶发失败。
- poisoned 锁恢复只保证锁可再次取得，不保证 panic 前修改的外部全局状态已恢复。因此后续测试应先建立自己所需的基线，而不能依赖前一用例留下的状态。
- 该锁不跨 crate 测试二进制或操作系统进程，不适合保护端口、文件等跨进程共享资源。

## 并发与资源生命周期

`OnceLock` 的生命周期覆盖整个测试进程且没有显式销毁步骤；`MutexGuard` 的生命周期由每个调用者的词法作用域控制。等待锁的线程由标准库互斥原语调度，本文件不创建线程、任务、通道或异步 runtime，也不保证等待公平性。

[`sql_fail_test.rs`](sql_fail_test.rs) 在持有 `_serial` 时启动恢复线程，恢复线程只撤销 failpoint，不会再次获取全局锁，因此当前实现没有嵌套加锁。父测试在线程 `join`、response/store 关闭之后才离开作用域，保证 failpoint 生命周期被包含在串行临界区中。

[`test_state_test.rs`](test_state_test.rs) 刻意让子线程持锁后 panic：栈展开先 drop guard 并释放 mutex，同时把它标为 poisoned；`join` 返回错误后，主线程的第二次调用经 `into_inner` 成功获得 guard。这同时证明“panic 不会永久占锁”和“中毒不会让 helper 自身 panic”。若进程以 abort 方式终止，则没有后续测试可恢复，不属于本 helper 的处理范围。

## 与 Go 版本的对应关系

Go 的 [`main_test.go`](main_test.go) 通过 `TestMain` 为 driver 测试统一执行 setup、启用 TiKV failpoints 并做 goroutine 泄漏检查；Go 目录中没有 `test_state.go` 或与 `global_state_guard` 一一对应的 helper。因此本文件是 Rust 测试运行模型所需的局部接线，不是 Go 生产逻辑的直接翻译。

语义对应体现在被保护的移植测试：Go [`config_test.go`](config_test.go) 的 `TestSetDefaultAndOptions` 读取全局 TiKV 配置并临时设置 metrics const labels，通过 `t.Cleanup` 恢复 labels；Rust [`config_test.rs`](config_test.rs) 还会设置可变的 Rust 全局配置并在末尾恢复，所以用本锁避免与其他 Rust 用例并行冲突。Go [`sql_fail_test.go`](sql_fail_test.go) 的 `TestFailBusyServerCop` 启用同名 `tikvclient/rpcServerBusy` failpoint并由 goroutine 延迟关闭；Rust 对照测试用线程和 RAII failpoint handle 实现同一生命周期，并额外用本锁隔离进程级 failpoint 状态。

Rust helper 没有改变对应测试的业务断言：它只约束测试调度。不能据此推断 Go 测试也被同一全局 mutex 串行化，也不能把它视为 driver 线上并发控制的一部分。

## 扩展指南

新增或移植会修改 driver 进程级状态的测试时，应在任何共享状态读取或写入之前调用 `global_state_guard`，并让 guard 覆盖状态建立、被测调用、断言、后台线程回收和清理全过程。优先复用本函数，不要为相同状态域再建第二把锁，否则两个测试仍可能并行。

若新场景需要在辅助函数内部再次进入临界区，应把现有 guard 作为参数向下传递或重构调用边界，不要递归调用 `global_state_guard`。若要保护跨进程资源，应另选文件锁、动态端口或独立临时目录等机制，而不是扩展此进程内 mutex 的职责。

修改中毒处理或 guard 生命周期时，应同步更新独立测试 [`test_state_test.rs`](test_state_test.rs)，并至少保留“持锁线程 panic 后再次获取成功”的回归场景。新增共享状态调用者的业务断言仍应放在各自独立的 `*_test.rs` 文件中，不要把测试逻辑内嵌进 `test_state.rs`。兼容性风险主要是遗漏调用导致测试不稳定，性能风险是临界区过宽降低测试并行度；由于模块仅在 `cfg(test)` 下挂载，不存在生产运行时性能影响。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/store/driver` 确认目标、入口和相关测试均已索引。
- RustCodeGraph `node --file pkg/store/driver/test_state.rs`：核实文件共 11 行、唯一函数签名、`OnceLock<Mutex<()>>` 初始化以及 poison 恢复分支。
- RustCodeGraph 对 `global_state_guard` 的 `query`/`explore`：核实符号位置及 `sql_fail_test.rs`、`test_state_test.rs` 调用关系；精确 `callers` 查询在本次环境中长时间无输出，已中止，并用限定 Rust 源文件的源码检索补齐调用者。
- RustCodeGraph `node`：读取 [`test_state_test.rs`](test_state_test.rs)、[`sql_fail_test.rs`](sql_fail_test.rs)、[`config_test.rs`](config_test.rs)、[`driver_lifecycle_test.rs`](driver_lifecycle_test.rs) 和 [`lib.rs`](lib.rs)，核实 panic、中毒恢复、failpoint、全局配置及模块挂载语义。
- 配置与 Go 对照：读取 [`Cargo.toml`](Cargo.toml)、[`main_test.go`](main_test.go)、[`config_test.go`](config_test.go) 和 [`sql_fail_test.go`](sql_fail_test.go)，核实 crate 边界、依赖范围及移植测试的原始意图。目标目录及其上级 `pkg/store` 下没有 `doc.go` 可供补充包契约。
- 这是纯文档分析，按计划不运行 Cargo。交付结构验证要求文档存在且固定二级标题恰好为 11 个。

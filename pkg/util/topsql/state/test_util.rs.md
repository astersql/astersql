# `pkg/util/topsql/state/test_util.rs`

## 文件定位

该文件是 `astersql-util-topsql-state` crate 的测试专用共享工具。crate 入口 `pkg/util/topsql/state/lib.rs` 通过 `#[cfg(test)]` 和 `#[path = "test_util.rs"]` 挂载它，因此它只参与本 crate 的测试构建，不进入普通库构建或 TopSQL 的生产请求链。`pkg/util/topsql/state/Cargo.toml` 将 `lib.rs` 声明为库入口，并以 `package.metadata.porting.go-package = "pkg/util/topsql/state"` 标明对应 Go 包。

尽管文件名为 `test_util.rs`，它不是独立测试集合：文件本身没有 `#[test]`，而是服务于 `state_test.rs` 和 `migration_aster_unit_test.rs` 两个独立测试模块。其存在原因是这两个模块都会修改 `state.rs` 中的进程级 `GlobalState`，需要共享一种串行化和复位协议，避免并行测试彼此污染。

## 核心职责

文件只承担两项职责，且二者应配套使用：

1. `lock_global_state` 获取 crate 内统一的测试互斥锁，使所有会修改 TopSQL/TopRU 全局状态的测试串行执行。
2. `reset_global_state` 把测试关心的可变状态恢复到确定基线：关闭 TopSQL、把 TopRU 消费者引用计数降到零，并把 TopRU item interval 恢复为默认值。

它不实现 TopSQL/TopRU 的生产状态机，也不绕过公开状态 API；实际原子状态、引用计数 CAS 循环和 interval 校验均位于 `pkg/util/topsql/state/state.rs`。

## 主要符号

- `TEST_LOCK: Mutex<()>`：模块私有的进程内互斥锁。零大小的 `()` 表明锁只表达排他关系，不承载业务数据；静态生命周期保证所有测试调用者竞争同一把锁。
- `pub(crate) fn lock_global_state() -> MutexGuard<'static, ()>`：crate 内可见的锁入口。返回 guard 而不是在函数内立即释放，调用者通过局部变量（现有测试均命名为 `_guard`）把临界区延长到整个测试函数结束。
- `pub(crate) fn reset_global_state()`：crate 内可见的复位入口。它依次调用 `DisableTopSQL`，在 `TopRUEnabled()` 为真时反复调用 `DisableTopRU`，最后调用 `ResetTopRUItemInterval`。

文件没有类型、trait、impl、feature 分支或公开到 crate 外的 API。唯一条件编译边界位于其模块入口 `lib.rs` 的 `#[cfg(test)]`。

## 执行流程

标准调用流程可由 `state_test.rs` 和 `migration_aster_unit_test.rs` 的每个相关测试复核：

1. 测试调用 `lock_global_state()`，`TEST_LOCK.lock()` 阻塞直至取得唯一 guard。
2. 若此前持锁测试发生 panic 使锁中毒，`unwrap_or_else(|poisoned| poisoned.into_inner())` 仍接管 guard，让后续测试有机会清理状态并继续执行。
3. 测试在仍持有 guard 时调用 `reset_global_state()`。
4. `DisableTopSQL()` 将生产单例的 TopSQL 原子开关写为 `false`。
5. `while TopRUEnabled()` 循环逐次调用 `DisableTopRU()`。生产实现每次至多减少一个消费者，并在最后一个消费者退出时复位 interval；循环因此把未知的正引用计数耗尽到零。
6. 函数再次显式调用 `ResetTopRUItemInterval()`，覆盖“计数原本已为零”或其他遗留情形，确保 interval 一定回到 `DefTiDBTopRUItemIntervalSeconds`。
7. 测试执行自己的状态变换和断言；函数返回或 panic 展开时 `_guard` 被丢弃，下一项测试方可进入。

调用顺序很重要：现有用法是“先加锁、再复位、再测试”。若先复位后加锁，复位和断言之间仍可被另一个并行测试改写。

## 数据与状态

`TEST_LOCK` 与生产状态是两套相互独立的数据：前者只协调 Rust 测试，后者是 `state.rs::GlobalState` 中由顺序一致性原子量维护的进程级单例。

`reset_global_state` 覆盖的状态包括：

- `GlobalState.enable`：经 `DisableTopSQL()` 变为 `false`。
- `GlobalState.ruConsumerCount`：没有直接访问私有字段，而是经 `TopRUEnabled()`/`DisableTopRU()` 从任意正值逐步降为零；`DisableTopRU()` 自身防止下溢。
- `GlobalState.TopRUItemIntervalSeconds`：最终经 `ResetTopRUItemInterval()` 写回默认的 60 秒。

它不复位 `PrecisionSeconds`、`MaxStatementCount` 或 `MaxCollect`。当前测试只读取这些默认常量/原子值而不修改它们；若将来测试开始改写这些字段，现有复位覆盖面将不再充分。

## 依赖与调用关系

上游模块关系为 `lib.rs --#[cfg(test)]--> test_util.rs`。精确引用搜索显示只有：

- `pkg/util/topsql/state/state_test.rs`：3 个测试同时调用 `lock_global_state` 和 `reset_global_state`。
- `pkg/util/topsql/state/migration_aster_unit_test.rs`：前 2 个迁移测试同时调用两者；默认值测试只加锁，因为它只读状态。

下游调用边为：

- `lock_global_state -> std::sync::Mutex::lock`，并在毒锁分支调用 `PoisonError::into_inner`。
- `reset_global_state -> DisableTopSQL`。
- `reset_global_state -> TopRUEnabled -> DisableTopRU`（循环边）。
- `reset_global_state -> ResetTopRUItemInterval`。

后三个状态 API 由 crate 根经 `pub use state::*` 再导出。Cargo 清单中的 `log` 与 `thiserror` 属于 `state.rs` 的生产实现；本文件自身除标准库同步原语和同 crate API 外没有额外依赖。

## 错误处理与边界

`lock_global_state` 不返回 `Result`。普通锁竞争通过阻塞解决；毒锁不是永久失败，而是取回内部 guard。这样可避免某个测试 panic 后整组状态测试因 `unwrap()` 连锁 panic，但接管者必须紧接着调用 `reset_global_state` 才能恢复逻辑基线。现有两个测试模块遵循这一约定。

`reset_global_state` 也不返回错误，因为它调用的四个状态操作均为无错误返回 API。循环只在 `TopRUEnabled()` 为真时执行；生产 `DisableTopRU()` 对零或负边界直接返回，并以 CAS 防止并发下溢。测试锁只能约束采用该工具的本 crate 测试，无法阻止不持锁的代码或其他线程直接修改全局状态，因此调用纪律是其边界，不是类型系统强制的不变量。

若有代码持续并发增加 TopRU 消费者，复位循环可能无法及时结束；当前测试设计通过统一互斥锁和不启动此类后台写入者来避免该情形。该工具也不负责验证 `SetTopRUItemInterval` 的错误，相关非法值行为由独立测试断言。

## 并发与资源生命周期

`TEST_LOCK` 的生命周期覆盖整个测试进程；`MutexGuard<'static, ()>` 的借用来源是静态锁，但 guard 自身仍按 RAII 在局部作用域结束时释放。现有测试把 guard 保留至测试函数末尾，所以复位、状态变换和断言构成一个不可与其他合规测试交错的临界区。

生产状态字段仍使用 `AtomicBool`/`AtomicI64` 和 `Ordering::SeqCst`；测试锁没有替代这些原子同步，只为一串多步骤操作提供测试层面的事务式隔离。`reset_global_state` 不创建线程、任务、通道、文件句柄或堆资源；唯一资源是互斥 guard。panic 展开会自动释放 guard，但会把锁标记为 poisoned，下一调用通过接管并复位恢复。

## 与 Go 版本的对应关系

Go 同路径没有对应的 `test_util.go`。`pkg/util/topsql/state/state_test.go` 在每个测试开头直接执行 `GlobalState.ruConsumerCount.Store(0)` 与 `ResetTopRUItemInterval()`；它能访问同 Go package 的私有字段。Rust 版本保持测试逻辑与 Go 用例一致，但因测试拆成独立 Rust 模块、生产字段保持私有，改用公开 API 循环耗尽引用计数，并增加 `TEST_LOCK` 处理 Rust 测试默认可并行执行造成的共享状态竞争。

复位语义与 Go 生产实现一致：`DisableTopRU` 每次减少一个活跃消费者、禁止下溢，并在最后一个消费者离开时重置 interval。Rust 工具末尾额外无条件复位 interval，是测试隔离措施，不改变生产函数语义。对应测试继续覆盖 Go 的三项意图：引用计数与最后退出复位、合法 interval 后写覆盖、非法 interval 不覆盖已有合法值；迁移测试另覆盖 TopSQL/TopRU 组合开关和默认常量。

## 扩展指南

新增会读写 `GlobalState` 的 Rust 单元测试时，应放在独立测试文件中，并在任何状态访问前取得 `lock_global_state`；若测试依赖默认开关、RU 计数或 interval，还应紧接着调用 `reset_global_state`。不要把新测试内嵌进本工具文件，也不要新建另一把局部锁，否则不同测试组仍可能并行修改同一单例。

若生产 `State` 新增可变全局字段或测试开始修改目前未复位的三个配置原子，应同步评估并扩展 `reset_global_state`，同时在 `state_test.rs` 或专门的同目录独立测试文件中加入“前一测试写入、复位后恢复默认”的回归证据。若新增状态只能通过可能失败的 setter 复位，则应重新设计本函数的返回类型或提供不会掩盖失败的清理 guard，而不是静默忽略错误。

兼容性风险主要是遗漏新状态导致测试顺序依赖；正确性风险是调用者未持有统一 guard；性能风险仅限测试阶段，串行化会降低这些小型状态测试的并发度，但不影响发布产物。改变毒锁策略时还需考虑 panic 后是否能可靠复位，改变循环策略时必须保留任意正引用计数归零以及不下溢的 Go 对齐语义。

## 验证依据

- RustCodeGraph `status`：索引包含 `pkg/util/topsql/state` 的 7 个 Go/Rust 文件；文件查询确认 `test_util.rs` 共 26 行、3 个符号，并被 `state_test.rs` 与 `migration_aster_unit_test.rs` 使用。
- RustCodeGraph `node --file pkg/util/topsql/state/test_util.rs`：确认静态锁、毒锁接管和复位调用顺序。
- RustCodeGraph `node --file` 对 `lib.rs`、`state.rs`、`state_test.rs`、`migration_aster_unit_test.rs` 的查询：确认测试条件编译边界、生产原子状态语义、两个真实调用者及测试覆盖。
- `rg -n --glob '*.rs' '\\b(lock_global_state|reset_global_state)\\b' pkg/util/topsql/state`：精确核对所有 Rust 定义和引用；RustCodeGraph 的精确 `callers` 查询在本地未及时返回，未把其结果作为调用边证据。
- `pkg/util/topsql/state/Cargo.toml`：确认 crate 名称、`lib.rs` 入口、Go 包映射及依赖边界。
- RustCodeGraph 对 `state.go` 与 `state_test.go` 的文件查询：确认 Go 的引用计数、CAS/复位行为和测试直接清零方式。
- 未运行 Cargo：本任务仅新增分析文档，任务计划明确禁止运行 Cargo；文档结构由任务指定的 11 标题命令验证。

# `pkg/util/topsql/stmtstats/test_guard.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-topsql-stmtstats`，其 crate 根是同目录的 `lib.rs`。`lib.rs` 通过 `#[path = "test_guard.rs"] pub mod test_support;` 将本文件装配成模块 `test_support`；因此调用路径是 `super::test_support::stmtstats_guard()`，而不是按文件名推导的 `test_guard::...`。

虽然模块没有受 `#[cfg(test)]` 限制并被标记为 `#[doc(hidden)] pub`，当前索引和文本引用都只显示它被本 crate 的 Rust 测试模块使用。它不参与 TopSQL/TopRU 统计的生产请求链，也不实现统计、聚合或上报行为。

## 核心职责

文件只提供一个进程级测试互斥入口 `stmtstats_guard`。Rust 测试默认可以并行执行，而本 crate 的若干测试会共同修改全局聚合器、TopSQL/TopRU 开关、collector 注册状态或 reporter 指标。测试在操作这些共享状态前持有该函数返回的守卫，从而把相关测试的临界区串行化，避免相互污染。

守卫还显式容忍锁中毒：若先前持锁测试发生 panic，后续测试仍取得互斥锁内部的 guard 并继续运行，而不是因 `PoisonError` 再次 panic。该选择改善整个测试集合在单个失败后的可诊断性，但不清理前序测试遗留的全局状态。

## 主要符号

- `stmtstats_guard() -> MutexGuard<'static, ()>`：唯一函数，声明为 `pub(super)`。相对于实际模块 `test_support`，它只对父模块（crate 根）及其可见后代开放，供同 crate 的测试模块调用，不构成 crate 的外部公共 API。
- `LOCK: Mutex<()>`：函数体内的局部静态变量。它不承载业务数据，单位值 `()` 仅作为互斥令牌；函数局部静态仍在整个进程生命周期内唯一存在。
- `MutexGuard<'static, ()>`：返回值同时代表已成功取得锁和解锁责任。调用者通常绑定为 `_guard` 或 `_serial`，依靠离开作用域时的 `Drop` 自动解锁。

本文件没有常量、结构体、枚举、trait、impl、异步任务或条件编译项。

## 执行流程

1. 测试调用 `super::test_support::stmtstats_guard()`；部分 `aggregator_test.rs` 用本地包装函数 `test_guard()` 转调。
2. 首次触达时，Rust 初始化函数局部静态 `LOCK`；之后所有调用复用同一把 `Mutex<()>`。
3. `LOCK.lock()` 阻塞等待现有持有者释放锁。
4. 正常加锁时直接返回 `MutexGuard`；若锁已中毒，`unwrap_or_else` 调用 `PoisonError::into_inner()`，取回其中的 guard。
5. 测试继续修改或检查共享状态。绑定守卫的变量存活期间，其他同样调用本函数的测试不能进入其临界区。
6. 守卫离开作用域或被显式丢弃时自动解锁。

因此，正确性取决于调用者在接触共享全局状态之前获取守卫，并让守卫覆盖完整的状态设置、断言和清理区间。

## 数据与状态

本文件唯一持久状态是 `LOCK` 的互斥状态及其中毒标记；锁内数据为 `()`，没有可读写的业务字段。`'static` 返回生命周期来自静态锁，并不要求调用者永久持锁，只表示 guard 借用的 mutex 在进程结束前一直有效。

它间接保护的状态可从使用方看到，包括 `aggregator.rs` 的 `GLOBAL_AGGREGATOR`，TopSQL/TopRU 全局开关，以及测试注册的 statement/RU collectors。`aggregator_1_aster_unit_test.rs`、`aggregator_test.rs`、`aggregator_bench_test.rs` 和 `kv_exec_count_test.rs` 在相关用例入口持锁；不触碰这些全局状态的纯局部并发测试不一定持锁。

这把锁不会保存状态快照，也不会调用 `reset_top_state()`、`CloseAggregator()` 或注销 collector。共享状态的初始化和恢复仍由每个测试负责。

## 依赖与调用关系

直接依赖仅是 Rust 标准库的 `std::sync::{Mutex, MutexGuard}`，不引入 Cargo 第三方依赖。`Cargo.toml` 将本目录定义为独立 crate，并声明 execdetails、reporter metrics 和 topsql state 三个路径依赖；这些依赖属于整个 stmtstats crate，本文件自身没有直接调用它们。

RustCodeGraph 将本文件列为被以下四个文件使用：

- `aggregator_1_aster_unit_test.rs`：直接获取守卫，覆盖 KV 去重、RU 增量、版本切换、聚合及容量等涉及全局开关或指标的测试。
- `aggregator_test.rs`：本地 `test_guard()` 包装本函数，供全局聚合器生命周期、collector 注册、开关门控和并发收尾等测试使用。
- `aggregator_bench_test.rs`：四个容量/规模测试在调用会重置并切换 TopRU 状态的 `drainShape` 前持锁。
- `kv_exec_count_test.rs`：在启用/恢复 TopSQL 状态并检查 KV target 去重期间持锁。

下游调用只有 `Mutex::lock`、`Result::unwrap_or_else` 和 `PoisonError::into_inner`。没有网络、存储、计时器、通道或数据库调用。

## 错误处理与边界

`Mutex::lock` 的唯一显式错误分支是锁中毒。本实现不把中毒当成当前测试的失败，而是用 `into_inner()` 恢复 guard。这样可以避免一次 panic 触发后续所有依赖守卫的测试级联失败，但调用者必须意识到：中毒意味着前一临界区可能未完成清理，恢复锁不等于恢复全局状态一致性。

标准 `Mutex` 没有超时参数；若持锁测试永久阻塞、遗忘释放 guard，或在持锁期间等待另一个需要同一守卫的执行路径，就可能导致测试挂起。当前函数不可重入，同一线程在仍持有 guard 时再次调用也会阻塞。

该锁只约束主动调用 `stmtstats_guard` 的代码。任何绕过守卫却修改相同全局状态的测试都可能继续产生竞态；因此它不是对所有 crate 状态的自动保护。

## 并发与资源生命周期

`LOCK` 在首次调用时初始化，生命周期覆盖测试进程；每次调用产生一个独占 guard。等待者由标准库 mutex 调度，本文件不保证公平性或获取顺序。

资源释放完全依赖 RAII：guard 的 `Drop` 解锁，即使测试通过 panic 展开离开作用域也会释放底层锁，但 mutex 会被标为中毒。下一调用通过恢复分支继续获得互斥访问。函数不创建线程，也不管理 `aggregator.rs` 中的后台 worker；相应测试仍需自行执行 `SetupAggregator`/`CloseAggregator` 和状态复位。

为了使临界区明确，调用者应在测试开头获取 guard，并避免把它提前 `drop`。反之，纯局部数据的并发测试不应无理由持锁，以免不必要地降低测试并行度。

## 与 Go 版本的对应关系

同路径 Go 包没有 `test_guard.go` 或同名函数。Go 的 `main_test.go` 只进行公共测试环境初始化和 goroutine 泄漏检查；当前同目录 Go 测试中也没有 `t.Parallel()`，所以其共享全局状态测试按默认方式串行运行。

Rust 移植后，测试 harness 默认并行运行多个 `#[test]`，因此该文件提供了 Go 测试原本隐含的串行执行条件。它对齐的是测试隔离语义，而不是 Go 生产代码中的某个类型或函数。Go 测试通过 `defer state.DisableTopSQL()`、`defer state.DisableTopRU()`、`defer CloseAggregator()` 等方式恢复状态；Rust 测试通常显式调用 `reset_top_state()` 或 `CloseAggregator()`，守卫只负责避免恢复前发生交叠，不能替代这些清理。

## 扩展指南

新增或修改会操作 stmtstats 共享全局状态的 Rust 测试时，应在任何状态变更之前调用 `super::test_support::stmtstats_guard()`，并让返回 guard 覆盖设置、执行、断言和清理全过程。测试逻辑应放在同目录独立的 `*_test.rs` 文件中，不要嵌入本源文件。

若新增另一套必须独立串行化的资源，先判断它是否与现有 TopSQL/TopRU 状态共享临界区：共享时复用此锁，互不相关且执行成本较高时可建立单独的、命名清晰的 guard，避免把所有测试无差别串行化。不要在持锁区中再次获取同一 guard。

若改变中毒策略，需要同步增加独立回归测试，分别覆盖正常互斥、持锁 panic 后的恢复，以及恢复后调用者仍须重置共享状态的约束。若调整 `lib.rs` 的 `#[path]` 或模块名，还需同步全部 `super::test_support::stmtstats_guard()` 调用点，尤其注意当前 `test_support.rs` 实际装配为 `stmtstats_tests`、`test_guard.rs` 实际装配为 `test_support` 的重命名关系。

兼容风险主要是模块可见性和测试调用路径；正确性风险主要是漏加守卫、临界区过短及误认为锁中毒恢复会清理状态；性能风险限于测试并行度，生产运行时不经过此函数。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标工程已建立索引。
- RustCodeGraph `files --filter pkg/util/topsql/stmtstats`：确认本 crate 的 Rust/Go 源文件及独立测试分布。
- RustCodeGraph `node --file pkg/util/topsql/stmtstats/test_guard.rs`：确认文件只有 `stmtstats_guard` 与局部静态锁，并列出四个使用文件。
- RustCodeGraph `query stmtstats_guard --kind function`：唯一结果为 `test_guard.rs:8`。`callers` 查询在本次执行中未在 60 秒内返回，调用点改由文件级索引结果和 `rg` 逐项核对；未据此推断额外调用边。
- RustCodeGraph `node`：读取 `lib.rs` 及 `aggregator_1_aster_unit_test.rs`、`aggregator_test.rs`、`aggregator_bench_test.rs`、`kv_exec_count_test.rs`，核对模块装配、guard 持有位置和受保护的测试行为。
- 直接读取 `Cargo.toml`，确认 crate 名、crate 根、路径依赖和 `go-package = "pkg/util/topsql/stmtstats"` 移植元数据。
- 直接读取 Go `main_test.go`，并检索同目录 `*_test.go` 的 TopSQL/TopRU、聚合器生命周期及 `t.Parallel()` 使用，确认没有直接 Go 对应守卫且当前测试未显式并行。
- 最终结构检查应确认本文恰好包含计划要求的十一个二级标题；本任务是纯文档分析，按计划不运行 Cargo。

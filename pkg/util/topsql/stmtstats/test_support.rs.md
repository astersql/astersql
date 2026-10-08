# `pkg/util/topsql/stmtstats/test_support.rs`

## 文件定位

该文件是 `astersql-util-topsql-stmtstats` crate 的测试辅助实现，服务于语句统计聚合器、TopSQL 和 TopRU 的 Rust 单元测试。crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，直接依赖执行明细、reporter 指标和 TopSQL 状态三个本地 crate。

[`lib.rs`](lib.rs) 通过 `#[path = "test_support.rs"]` 将本文件装配为隐藏的公共模块 `stmtstats_tests`；同目录测试使用 `use super::stmtstats_tests::*` 同时取得本文件的辅助符号以及 `pub use super::*` 重导出的 crate API。不要把名称相近的 `test_support` 模块混为一谈：`lib.rs` 将 [`test_guard.rs`](test_guard.rs) 装配成 `test_support`，后者只提供串行化全局状态测试的 `stmtstats_guard`。

本文件没有 `#[cfg(test)]` 门控，因而辅助类型会进入 crate 的普通编译表面，但模块被 `#[doc(hidden)]` 隐藏；它们的实际引用均位于同目录 Rust 测试，而不是数据库运行时主链。

## 核心职责

- `StatementCollector` 实现生产 trait `Collector`，把聚合器推送的每一份 `StatementStatsMap` 按到达顺序完整保存在内存中，供测试检查批次数和批次内容。
- `TestRUCollector` 实现 `RUCollector`，分别记录 `(RUIncrementMap, RUVersion)` 数据批次与版本切换通知，支持验证 RU 推送、版本交接和容量裁剪。
- `TestRUVersionProvider` 实现 `RUVersionProvider`，允许测试在运行期间原子切换 RU 协议版本。
- `ru_details` 和 `add_ru` 构造、修改生产类型 `SharedRUDetails`，让测试模拟语句执行期间持续增长的资源消耗。
- `reset_top_state` 把进程级 TopSQL/TopRU 开关恢复到关闭状态，避免使用全局状态的测试互相污染。

这些工具只负责制造输入、观测输出和清理全局状态；统计合并、版本归一化、容量上限和后台 worker 等生产逻辑仍位于 [`stmtstats.rs`](stmtstats.rs)、[`rustats.rs`](rustats.rs) 与 [`aggregator.rs`](aggregator.rs)。

## 主要符号

- `pub struct StatementCollector { pub batches: Arc<Mutex<Vec<StatementStatsMap>>> }`：可克隆的语句批次记录器。克隆共享同一 `Vec`，不是复制已有批次。`Default` 创建空记录。
- `impl Collector for StatementCollector::CollectStmtStatsMap`：取得 `batches` 互斥锁并将整个映射追加到队尾，不合并相邻批次。
- `pub struct TestRUCollector`：`batches` 保存数据与当时版本，`changes` 保存 `OnRUVersionChange` 的通知序列；两个容器使用彼此独立的锁。
- `CollectRUIncrements`：原样追加 `(increments, version)`；`OnRUVersionChange`：原样追加版本。两者都不解释或归一化版本。
- `pub struct TestRUVersionProvider(pub AtomicI32)`：tuple 字段公开，正常测试通过 `new` 与 `set` 操作。`GetRUVersion` 直接返回原子当前值。
- `pub fn ru_details(read_ru, write_ru, tikv_ru_v2, tiflash_ru) -> SharedRUDetails`：构造 `execdetails::RUDetails`，只显式填写四个 RU 分量，其余字段取默认值，再包入 `Arc<RwLock<_>>`。
- `pub fn add_ru(details, ...)`：在写锁内分别对上述四个分量做浮点加法。
- `pub fn reset_top_state()`：先无条件调用 `DisableTopSQL`，再在 `TopRUEnabled()` 为真期间重复调用 `DisableTopRU`。
- 模块属性 `#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]` 与父 crate 的 Go 风格 API、全局状态兼容；本文件自身没有常量、枚举或条件编译项。

## 执行流程

典型语句统计测试流程如下：测试先持有 [`test_guard.rs`](test_guard.rs) 的进程级串行锁并调用 `reset_top_state`，启用目标开关，创建 `Aggregator` 与 `StatementStats`，注册 `StatementCollector`，触发语句开始/结束并执行 drain，最后从 `batches` 读取生产代码发出的映射。`aggregator_test.rs::TestRegisterUnregisterCollector` 与 `TestAggregatorRegisterCollect` 覆盖了注册期间接收、注销后不再接收以及统计字段正确性。

典型 RU 测试先用 `ru_details` 创建共享明细，把句柄传给 `ExecBeginInfo`/`ExecFinishInfo`，必要时用 `add_ru` 模拟执行中增量；聚合器通过 `StatementStats::MergeRUInto` 取得增量并调用 `TestRUCollector::CollectRUIncrements`。测试随后检查 `batches` 中的 key、总 RU、执行次数和版本；`stmtstats_test.rs` 还用同一辅助函数验证 V1 执行中采样与 V2 结束时最终值的区别。

RU 版本交接由 `aggregator_test.rs::TestAggregatorDetectsRUVersionHandover` 给出完整路径：`TestRUVersionProvider::new(V1)` 绑定到聚合器，首次 drain 产生 V1 批次；`set(V2)` 后下一次 drain 触发 `OnRUVersionChange(V2)`、清理旧版本状态，之后才接受 V2 数据。

测试收尾时再次调用 `reset_top_state`。循环关闭 TopRU 是必要步骤，因为状态实现允许多次启用形成计数；只调用一次 `DisableTopRU` 不能保证全局状态归零。

## 数据与状态

`StatementCollector::batches` 的外层 `Vec` 表示推送时间序列，每个元素是一轮聚合后的 `StatementStatsMap`。辅助类型不修改映射，也不对空映射做过滤，因此“是否推送”和“推送内容”都由生产聚合器决定。

`TestRUCollector` 将普通数据流与控制流分离：`batches` 记录 RU 增量及其协议版本，`changes` 只记录版本切换。测试可独立断言版本通知发生次数和切换前后数据批次数量。它不强制两个序列之间的原子快照；顺序保证仅限各自互斥锁保护的 `Vec`。

`SharedRUDetails` 在 [`rustats.rs`](rustats.rs) 中定义为 `Arc<RwLock<RUDetails>>`。`ru_details` 返回的句柄可被测试和生产统计对象共享；`add_ru` 修改的是同一底层对象。四个参数分别对应 V1 使用的 `read_ru`、`write_ru` 以及 V2/存储侧的 `tikv_ru_v2`、`tiflash_ru` 原始字段，具体哪些字段计入某一版本由 `stmtstats.rs` 决定，而不是由辅助函数决定。

`reset_top_state` 修改的是进程级全局开关，不是某个 `Aggregator` 实例的局部状态。因此相关测试必须配合 `stmtstats_guard` 串行运行；本文件不提供 RAII 清理，测试 panic 时仍可能在当前进程留下开关状态。

## 依赖与调用关系

上游引用由 `rg` 核对：[`aggregator_test.rs`](aggregator_test.rs) 使用全部三种 mock/提供者及 RU 辅助函数，覆盖 Collector 注册、TopSQL/TopRU 门控、RU 版本交接、并发注销和 key 上限；[`stmtstats_test.rs`](stmtstats_test.rs) 大量使用 `ru_details`/`add_ru` 构造 V1/V2 采样场景；[`aggregator_bench_test.rs`](aggregator_bench_test.rs) 使用 `ru_details`、`TestRUCollector` 与 `reset_top_state` 验证 10k key 裁剪；[`kv_exec_count_test.rs`](kv_exec_count_test.rs) 使用 `reset_top_state` 隔离全局开关。

下游依赖来自父模块重导出：`Collector`、`RUCollector` 与 `Aggregator` 定义在 [`aggregator.rs`](aggregator.rs)；`RUVersionProvider`、`RUVersion`、`RUIncrementMap`、`SharedRUDetails` 定义在 [`rustats.rs`](rustats.rs)；`StatementStatsMap` 定义在 [`stmtstats.rs`](stmtstats.rs)；`execdetails::RUDetails` 和 `topsql_state` 分别由 [`lib.rs`](lib.rs) 重导出 Cargo 中的 `execdetails-dependency` 与 `topsql-state-dependency`。

关键调用边为：`Aggregator::drain_and_push_stmt_stats` → `Collector::CollectStmtStatsMap` → `StatementCollector::batches.push`；`Aggregator::drain_and_push_ru` → `RUCollector::{CollectRUIncrements, OnRUVersionChange}` → `TestRUCollector` 的两个记录容器；`Aggregator::current_ru_version` → `RUVersionProvider::GetRUVersion` → `TestRUVersionProvider` 的原子读取。RustCodeGraph 精确查询确认了这些 trait 方法和辅助符号的位置；图的 `callers/callees` 命令未返回可用边，所以上游测试引用由 `rg` 补证。

## 错误处理与边界

本文件没有可恢复错误返回。三个记录路径和 `add_ru` 都在锁中毒时使用带上下文的 `expect(...)` 直接 panic，符合测试辅助代码“立即暴露先前 panic/并发破坏”的策略。`TestRUVersionProvider` 的读取和写入不会返回错误。

`ru_details` 与 `add_ru` 不校验 `f64`：负数、`NaN`、无穷大以及精度损失都会原样进入共享明细。生产统计是否接受这些值不属于本辅助层职责，新增边界测试应明确预期，而不是在 helper 中静默规范化。

`reset_top_state` 假设每次 `DisableTopRU` 都使启用计数向关闭推进；若状态依赖违反此前提，循环可能不终止。它只复位 TopSQL/TopRU 开关，不会关闭全局聚合器、注销 collector、清空已有批次或重置 RU 版本提供者，相关资源必须由测试显式处理。

## 并发与资源生命周期

`StatementCollector` 和 `TestRUCollector` 通过 `Arc<Mutex<...>>` 满足 `Collector: Send + Sync` 与 `RUCollector: Send + Sync`，可以从聚合器后台线程或测试线程共享。每次回调在持锁期间只执行一次 `push`，临界区短；测试读取时也必须加锁。持有测试侧锁后不得同步触发可能回调同一 collector 的 drain，否则可能自锁。

`TestRUVersionProvider` 使用 `AtomicI32` 和 `Ordering::SeqCst`。这给所有版本读写提供单一全序，适合版本切换测试；它不把版本变化与 RU 明细更新组合成事务，聚合器负责在观察到版本变化后执行自己的清理和通知逻辑。

`SharedRUDetails` 的 `Arc` 决定所有权生命周期，`RwLock` 允许生产读取与测试写入互斥。`add_ru` 一次写锁内更新四个字段，因此同一把锁的读者不会看到本次调用的部分字段更新；跨多次 `add_ru` 调用则没有整体原子性。

mock 的批次会一直保留到所有 `Arc` 和 collector 自身被释放，没有容量限制。大规模测试（例如 10k key 上限测试）应在断言后及时释放 collector，避免不必要的峰值内存。全局开关生命周期不由类型自动管理，必须依赖串行守卫和显式复位。

## 与 Go 版本的对应关系

Go 没有同名 `test_support.go`；对应语义分散在测试文件中。[`aggregator_test.go`](aggregator_test.go) 的 `mockCollector`、`mockRUCollector` 与 `mockRUVersionProvider` 分别对应 Rust 的 `StatementCollector`、`TestRUCollector` 与 `TestRUVersionProvider`。Go mock 通过可选闭包即时执行断言或收集数据，Rust helper 则固定把事件写入线程安全 `Vec`，便于测试结束后断言；这是测试支撑方式的差异，不改变生产 trait 合约。

Go 的 `mockRUVersionProvider` 直接读取普通字段，相关测试在同一 goroutine 中赋值；Rust 版本用 `AtomicI32` 支持跨线程可见的动态切换。Go 的 RU collector 可通过 `fWithVersion`/`onChange` 分别观察数据和版本通知，Rust 用 `batches`/`changes` 保存相同两类证据。

Go 测试直接构造或修改资源控制器的 `RUDetails`，Rust 因执行明细以 `Arc<RwLock<_>>` 共享而集中提供 `ru_details`/`add_ru`。Go 的多个测试也以 `for state.TopRUEnabled() { state.DisableTopRU() }` 清理计数式开关；Rust `reset_top_state` 抽取了同一语义，并额外无条件关闭 TopSQL。Go `main_test.go` 的公共环境初始化与 goroutine 泄漏检查在本文件中没有对应实现。

## 扩展指南

新增 collector 回调或扩展 trait 时，应同步修改本文件的对应实现，使测试 double 记录完整输入；同时更新 [`aggregator_test.rs`](aggregator_test.rs) 中的注册、推送和版本交接断言，并核对 [`aggregator_test.go`](aggregator_test.go) 的 Go 合约。不要为了方便在 mock 中复制生产聚合逻辑，否则测试可能同时复制同一个缺陷。

给 `RUDetails` 增加会影响 TopRU 的字段时，应评估 `ru_details` 与 `add_ru` 是否需要新参数，并同步独立的 [`stmtstats_test.rs`](stmtstats_test.rs) 版本/增量测试。参数扩展会影响大量调用点；优先保持字段含义明确，避免以位置相近但语义不同的分量代替。若字段仅供其他子系统且不参与这里的采样，则保留 `Default` 即可。

修改全局状态清理时，应先检查 TopSQL/TopRU 状态 crate 的计数语义，并保持 [`test_guard.rs`](test_guard.rs) 的串行隔离。若引入 RAII 清理，需覆盖正常返回和 panic 展开两条路径，且不能在析构中意外关闭其他并发测试刚启用的状态。

测试逻辑继续放在独立 `*_test.rs` 文件中，不应内嵌到本文件。需要模拟阻塞或回调协调的专用 double（如 `aggregator_test.rs::BlockingRUCollector`）可留在使用它的测试文件；只有多个测试文件共同复用、且行为足够通用的 helper 才适合加入本文件。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库索引包含 11,467 个文件；`files --filter pkg/util/topsql/stmtstats` 确认本目录 Rust/Go 文件；`node --file .../test_support.rs` 读取了完整 115 行；`query` 精确定位了 `StatementCollector`、`TestRUCollector`、`TestRUVersionProvider`、`ru_details`、`add_ru`、`reset_top_state` 及各 trait 方法。`callers/callees` 对这些 helper 未返回可用调用边，因此没有据此臆造上游。
- 模块与依赖：读取 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)，确认 crate 名、三个直接依赖、隐藏模块装配、父模块重导出及测试文件列表；读取 [`test_guard.rs`](test_guard.rs) 区分串行守卫模块。
- 生产定义：通过 RustCodeGraph 读取 [`aggregator.rs`](aggregator.rs) 的 collector traits/聚合器、[`rustats.rs`](rustats.rs) 的版本与共享 RU 类型、[`stmtstats.rs`](stmtstats.rs) 的执行上下文和统计容器。
- Rust 测试：读取并检索 [`aggregator_test.rs`](aggregator_test.rs)、[`stmtstats_test.rs`](stmtstats_test.rs)、[`aggregator_bench_test.rs`](aggregator_bench_test.rs) 与 [`kv_exec_count_test.rs`](kv_exec_count_test.rs)，确认每个 helper 的真实使用场景、边界和全局状态清理方式。
- Go 对照：读取 [`aggregator_test.go`](aggregator_test.go) 的三个 mock 与 TopRU 清理循环、[`stmtstats_test.go`](stmtstats_test.go) 的 RU 场景及 [`main_test.go`](main_test.go) 的测试生命周期设置。仓库中没有与本文件一一对应的 Go 生产文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰含 11 个固定二级章节，并人工复核只新增本说明文档、不修改 Rust、Go、Cargo 或只读的 `plan.md`。

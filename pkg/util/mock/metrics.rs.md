# `pkg/util/mock/metrics.rs` 逻辑说明

## 文件定位

[`pkg/util/mock/metrics.rs`](./metrics.rs) 属于 `astersql-util-mock` crate；crate 由 `pkg/util/mock/Cargo.toml` 定义，以 `pkg/util/mock/lib.rs` 为入口。入口通过私有 `mod metrics` 装载本文件，再用 `pub use metrics::*` 将 `MetricsCounter` 从 crate 根导出。它是测试辅助计数器：不向 Prometheus registry 注册新指标，而是在内存中累计 `f64` 值，便于测试读取调用次数。

`pkg/util/mock/Cargo.toml` 直接声明 `prometheus = "0.14"`，所以公开字段可以保留 `prometheus::Counter` 类型。本文件无模块级常量、trait、条件编译项或内嵌测试；独立 Rust 回归测试位于 `pkg/util/mock/migration_aster_unit_test.rs`。

## 核心职责

- `MetricsCounter` 保留一个可选的 Prometheus counter 字段，同时用私有原子位模式保存可直接断言的浮点累计值。依据：`MetricsCounter::{Counter,val}`。
- `Add` 以 compare-and-swap（CAS）循环实现并发安全的 `f64` 加法；`Inc` 是加 `1.0` 的便捷入口；`Val` 返回当前值。
- 该类型对齐 `pkg/util/mock/metrics.go::MetricsCounter`，用于测试中替代只需 `Add`/`Inc` 行为的真实指标，但当前 Rust 仓库内确认的直接行为消费者是本 crate 的迁移回归测试，不能将 Go timer/TTL 测试当作已接线的 Rust 调用链。

## 主要符号

- `pub struct MetricsCounter` 是唯一公开类型。它没有实现 `Clone`；需要跨线程共享时，现有测试使用 `Arc<MetricsCounter>`。
- `pub Counter: Option<prometheus::Counter>` 保留 Go 版本匿名嵌入 `prometheus.Counter` 可为 `nil` 的形状。本文件的 `Add`、`Inc` 和 `Val` 都不读写该字段，因此它不是内存累计值的权威存储。
- `val: AtomicU64` 是私有状态，保存 `f64::to_bits()` 产生的 64 位模式。Rust 标准库没有在此处直接使用的原子 `f64` 类型，因而用 `AtomicU64` 完成原子更新。
- `impl Default for MetricsCounter` 将 `Counter` 设为 `None`，并将 `val` 初始化为 `0_f64.to_bits()`。这是 Rust 中对齐 Go 零值构造的入口。
- `pub fn Add(&self, value: f64)` 以共享引用接受任意 `f64`，通过 CAS 循环将它累加到当前值。
- `pub fn Inc(&self)` 调用 `self.Add(1.0)`，没有第二套计数逻辑。
- `pub fn Val(&self) -> f64` 以 `SeqCst` 读取位模式，再用 `f64::from_bits` 还原当前值。

## 执行流程

1. 调用方通常以 `MetricsCounter::default()` 建立计数器；初始内存值为 `0.0`，可选 Prometheus 字段为 `None`。
2. `Add(value)` 先用 `val.load(Ordering::SeqCst)` 获得当前 64 位模式。
3. 它将位模式还原为 `f64`，执行浮点加法，再把结果转回新位模式。
4. `compare_exchange_weak(current, next, SeqCst, SeqCst)` 成功则返回；失败则使用返回的 `observed` 作为新基准重算。失败可来自其他线程已更新该值，也可来自 weak CAS 允许的伪失败。
5. `Inc()` 复用上述流程，增量固定为 `1.0`。`Val()` 只做一次顺序一致读取，不会重置或消耗计数。

RustCodeGraph 的文件节点确认 `Inc -> Add`；索引还把 `pkg/util/mock/migration_aster_unit_test.rs::metrics_counter_is_atomic_across_threads` 标识为 `Val` 的使用者。精确 `callers/callees` 子命令对这组 impl 方法未返回额外边，因此其他关系由索引的文件使用列表和精确引用搜索交叉核对。

## 数据与状态

权威计数状态只有 `val`。它始终以 IEEE 754 `f64` 位模式存在 `AtomicU64` 中，每次更新在某一个已观测值上完成一次浮点加法。`Counter` 与 `val` 没有自动同步关系：即使外部将它设为 `Some`，本文件的计数方法也只更新 `val`。

`Add` 不限制输入范围，因而负数、无穷大和 NaN 都会按 Rust `f64` 加法规则进入状态。一旦累加结果成为 NaN，后续数值加法通常仍保持 NaN；源码没有对这些值做规范化或拒绝。计数器也没有 reset、上限或单调性保护，所以“counter 只能增长”不是本实现强制的不变量。

## 依赖与调用关系

- crate 接线：`pkg/util/mock/lib.rs` 声明 `mod metrics` 并对外再导出；`pkg/util/mock/Cargo.toml` 将 crate 命名为 `astersql-util-mock`，且没有定义 feature 条件。
- 标准库依赖：`std::sync::atomic::{AtomicU64, Ordering}` 提供状态容器及顺序一致读改写。
- 外部依赖：`prometheus::Counter` 只出现在公开可选字段的类型中；本文件不调用 Prometheus API。
- 内部调用：`Inc` 调用 `Add(1.0)`；`Add` 调用原子 load/CAS 以及 `f64::{from_bits,to_bits}`；`Val` 调用原子 load 和 `f64::from_bits`。
- Rust 直接测试调用：`pkg/util/mock/migration_aster_unit_test.rs::metrics_counter_is_atomic_across_threads` 通过 `Arc` 在 8 个线程中同时调用 `Inc` 和 `Add(0.5)`，线程结束后用 `Val` 验证 `12_000.0`。全仓 Rust 精确搜索未发现其他 `MetricsCounter` 构造或使用。
- Go 行为消费者：`pkg/timer/runtime/runtime_test.go`、`pkg/timer/runtime/worker_test.go` 和 `pkg/ttl/ttlworker/integrationtest/timer_sync_test.go` 将 Go `MetricsCounter` 注入指标接口并用 `Val` 断言调用次数。它们证明该 mock 在 Go 测试体系中的用途，不是 Rust 应用主链的调用证据。

## 错误处理与边界

所有方法都不返回 `Result` 或 `Option`，也没有 I/O、注册或分配失败分支。CAS 冲突不是业务错误：`Add` 在循环中重新基于已观测值计算，直到写入成功。

边界来自浮点和 API 设计：源码不拒绝 NaN、正负无穷或负增量，也不检测精度损失。大数上累加小增量可能因 `f64` 舍入而不改变可观测值。`Counter` 为 `None` 不影响 `Add`/`Inc`/`Val`；反之，外部使用 `Counter` 字段也不会自动改变 `Val()`。

## 并发与资源生命周期

`AtomicU64` 使 `Add`、`Inc` 和 `Val` 都能以 `&self` 并发调用，不需外部互斥锁。每次 `Add` 的 CAS 成功点是该次累加的线性化点；竞争线程不会用过期基值直接覆盖更新。全部原子操作使用 `Ordering::SeqCst`，提供单一全局顺序下的最强标准内存序语义；相对较弱内存序，这也可能带来更高同步成本。

本类型不创建线程、任务、通道、锁或事务，也没有显式 `Drop`。状态生命周期与 `MetricsCounter` 值一致；如现有回归测试那样放入 `Arc`，则在最后一个 `Arc` 释放时销毁。没有持久化、自动清零或 registry 解注册流程。CAS 循环是 lock-free 形式，但源码不保证单个高竞争线程的有界完成时间。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/mock/metrics.go`。两端都定义 `MetricsCounter`，对外提供 `Add`、`Inc` 和 `Val`，且使用原子浮点累计以支持并发测试。Go 使用 `go.uber.org/atomic.Float64`；Rust 则将 `f64` 位模式放入 `AtomicU64` 并显式实现 CAS 循环。Rust `Inc` 调用 `Add(1.0)`，Go `Inc` 直接调用原子值的 `Add(1)`；两者可观测的计数意图一致。

Go 通过匿名嵌入 `prometheus.Counter` 获得接口方法集，其零值可为 `nil`。Rust 不能用字段嵌入获得同样的方法提升，因此保留公开 `Option<prometheus::Counter>` 字段，但没有实现 Prometheus counter trait 转发。这意味着 Rust 类型的当前可替换性只由它自身的方法和使用点证明，不应假设它已能代替所有需要 `prometheus::Counter` 的 Rust API。

Go timer/TTL 测试证明原类型的典型用法是：把 mock 注入指标字段，执行被测流程，然后用 `Val` 断言增量。Rust 独立测试已验证原子累加，但全仓搜索未发现对应 timer/TTL Rust 路径使用本类型，因而该业务接线仍不能标记为已验证。

## 扩展指南

- 若增加 reset、减法、范围检查或 NaN/无穷处理，最直接的修改点是 `MetricsCounter` 的 impl；必须先核对 Go 增量，避免破坏“允许任意 `f64`”的现有行为。
- 若要让 `Counter: Some(_)` 同步获得增量，需明确 `val` 与 Prometheus counter 哪个是权威值，以及部分更新失败时的一致性策略。当前两者故意无联动，不应在无测试时暗中转发。
- 若改变原子内存序或 CAS 逻辑，应保留并发累加的线性化性，并在 `pkg/util/mock/migration_aster_unit_test.rs` 扩展高竞争、混合增量和重复读取用例。正确性风险是丢失更新；性能风险是 `SeqCst` 和 CAS 重试在高竞争下的成本。
- 若需要完整模拟 Prometheus 接口，应先查明 Rust 消费方要求的具体 trait/API，再在本类型或新适配器上实现；不能仅凭公开 `Counter` 字段宣称已完整对齐 Go 匿名嵌入。
- 测试逻辑应继续放在独立 `*_test.rs` 文件，不应内嵌到 `metrics.rs`。现有最近落点是 `pkg/util/mock/migration_aster_unit_test.rs`，由 `pkg/util/mock/lib.rs` 的 `#[cfg(test)]` 模块声明接入。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`explore "pkg/util/mock/metrics.rs symbols callers callees"`；`node --file pkg/util/mock/metrics.rs --offset 1 --limit 240`；`query MetricsCounter --limit 20 --json`；以及对 `MetricsCounter`/`Add`/`Inc`/`Val` 的 `callers`/`callees` 尝试。文件节点确认源文件全部 64 行、`Inc -> Add` 和测试使用；精确 impl 方法图查询未返回额外边，本文未据此推测调用者。
- Rust 源码与配置：`pkg/util/mock/metrics.rs`、`pkg/util/mock/lib.rs`、`pkg/util/mock/Cargo.toml`。它们分别证明实现、模块再导出、crate 边界与 `prometheus` 依赖。
- Go 对照与消费测试：`pkg/util/mock/metrics.go`、`pkg/timer/runtime/runtime_test.go`、`pkg/timer/runtime/worker_test.go`、`pkg/ttl/ttlworker/integrationtest/timer_sync_test.go`。
- 独立 Rust 测试：`pkg/util/mock/migration_aster_unit_test.rs::metrics_counter_is_atomic_across_threads`。该测试证明 8 线程混合调用 `Inc` 与 `Add(0.5)` 后的结果为 `12_000.0`，但没有覆盖 NaN、无穷、负数、精度上限或 `Counter: Some(_)` 的行为。
- 引用复核：用 `rg` 对 `MetricsCounter`、`Add`、`Inc` 和 `Val` 执行 Rust/Go 精确搜索，确认 Rust 直接使用范围与 Go 侧典型测试用法。本任务为纯文档分析，按计划未运行 Cargo。

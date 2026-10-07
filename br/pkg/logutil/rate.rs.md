# `br/pkg/logutil/rate.rs`

## 文件定位

本文件属于 `astersql-br-pkg-logutil` library crate；crate 根是 [`br/pkg/logutil/lib.rs`](lib.rs)，其以 `pub mod rate` 装配本模块，并再次导出 `RateTracer` 与 `TraceRateOver`。[`br/pkg/logutil/Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 指向 Go 包 `br/pkg/logutil`，因此本文件的直接语义基准是同目录的 [`rate.go`](rate.go)，而不是一个独立的 Rust 限流子系统。

它位于 BR 日志辅助层：接收 Prometheus `Counter`，记录构造时的基线与单调时钟起点，提供从构造以来的平均 ops/s，并把该速率包装为日志字段。RustCodeGraph 对本文件列出 8 个符号；仓库检索只发现 crate 导出和测试引用，未发现 Rust 生产调用点。因此当前事实是“Rust API 已实现并测试，但尚无可确认的 Rust 业务接线”；Go 版本则已用于恢复建表进度日志。

## 核心职责

- `TraceRateOver` 在创建追踪器时读取一次计数器，把已有累计值保存为 `base`，从而只统计创建后的增量。
- `RateTracer::Inc` 和 `RateTracer::Add` 直接更新同一个 Prometheus 计数器；追踪器不是额外计数层。
- `RateTracer::RateAt` 用 `(当前计数 - base) / (采样时刻 - start 的秒数)` 计算全窗口平均速率；它不是滑动窗口、瞬时速率或限流器。
- `RateTracer::Rate` 以 `Instant::now()` 调用 `RateAt`，`RateTracer::L` 再把结果格式化成两位小数的 `speed="... ops/s"` 字段附加到默认日志器。
- `counter: None` 是 Rust 为表达 Go `nil Counter` 保留的状态；该状态允许构造和调用，速率返回 `NaN`，增量操作静默跳过。

## 主要符号

- `pub struct RateTracer { pub start: Instant, pub base: f64, pub counter: Option<Counter> }`：三项状态均公开，便于当前独立测试固定时间原点及构造空计数器场景。`#[derive(Clone)]` 会复制时间和基线，并克隆 Prometheus 计数器句柄。
- `pub fn TraceRateOver(counter: Counter) -> RateTracer`：公开构造入口。先以 `Instant::now()` 保存起点，再由 `astersql_lightning_metric::read_counter` 读取基线，最后将计数器置为 `Some`。
- `pub fn Inc(&self)`：当 `counter` 存在时调用 `Counter::inc()`；为空时无动作。
- `pub fn Add(&self, value: f64)`：当 `counter` 存在时调用 `Counter::inc_by(value)`；本层不校验参数，具体可接受值由 `prometheus::Counter` 约束。
- `pub fn Rate(&self) -> f64`：以当前单调时刻计算平均速率。
- `pub fn RateAt(&self, instant: Instant) -> f64`：核心计算函数，也是可注入采样时刻的测试缝。过去时刻通过显式取负得到负的 elapsed；空计数器返回 `f64::NAN`。
- `pub fn L(&self) -> Logger`：通过 `log::L().With(...)` 创建带 `speed` 字段的新 `Logger`。字段值来自 `format!("{:.2} ops/s", self.Rate())`。

文件没有 trait、宏、模块级常量或条件编译项。导入的 `default_logger` 在本文件当前实现中未使用；真实日志入口是 `logging::log::L`。

## 执行流程

典型流程如下：

1. 调用方把一个已有的 Prometheus `Counter` 传给 `TraceRateOver`。
2. 构造函数记录单调时钟 `start`，读取计数器当前值为 `base`；此前的累计量被排除在追踪窗口外。
3. 工作过程中调用 `Inc` 或 `Add`，实际增量进入底层共享计数器。
4. `RateAt(t)` 再次读取计数器的**当前值**，计算相对 `base` 的增量，并除以 `t` 相对 `start` 的秒数。传入的 `t` 只控制分母，不冻结计数器快照。
5. `Rate()` 把第 4 步的 `t` 设为当前时刻；`L()` 读取该速率、格式化为两位小数，并派生一个附带 `speed` 字段的日志器。

Go 业务链的直接证据位于 `br/pkg/restore/snap_client/client.go`：`createTablesBatch` 和 `createTables` 分别创建 rater，在并发建表完成后调用 `Add`/`Inc`，随后以 `rater.L().Info(...)` 输出进度。当前仓库没有对应的 Rust 生产调用边，不能把这条 Go 接线描述为已经迁移到 Rust。

## 数据与状态

`start` 使用 `std::time::Instant`，只表达同一进程内的单调时间差，不携带墙钟日期或可序列化时间戳。`base` 是构造瞬间的浮点计数快照；后续不会更新，所以 `RateTracer` 给出的是自创建以来的累计平均值。`counter` 持有 `prometheus::Counter` 句柄；`Clone` 出的追踪器仍指向同一个底层计数器，但每个克隆保留复制出的相同 `start` 与 `base`。

计算不缓存速率，每次 `Rate`、`RateAt` 或 `L` 都重新读取 Counter。若其他持有者也更新同一 Counter，这些更新同样进入分子；该类型不区分增量来源。测试直接构造公开字段来固定 `start`，说明这些字段目前兼具 API 状态与测试注入面，收紧可见性会影响现有测试。

## 依赖与调用关系

上游关系：

- [`lib.rs`](lib.rs) 声明 `pub mod rate` 并 `pub use rate::{RateTracer, TraceRateOver}`，是 crate 对外门面。
- [`logging_test.rs`](logging_test.rs) 的 `test_rater`、`test_rater_go_time_boundaries` 直接构造或调用本模块；[`parity_test.rs`](parity_test.rs) 复核与 Go 样例相同的 10、约 13.33、100 ops/s 序列及构造入口。
- RustCodeGraph 的文件视图报告 `rate.rs` “used by 0 files”；结合文本检索，只能确认模块声明/re-export 与测试引用，不能确认 Rust 应用主链调用者。
- Go 侧 [`client.go`](../restore/snap_client/client.go) 的恢复建表流程是实际生产调用者；[`logging_test.go`](logging_test.go) 的 `TestRater` 是原始行为测试。

下游关系：

- `astersql_lightning_metric::read_counter` 在 `pkg/lightning/metric/metric.rs` 中直接调用 `Counter::get()`，负责读取当前累计值。
- `prometheus::Counter` 提供 `inc`、`inc_by`、`get` 以及可克隆共享句柄；Cargo manifest 直接依赖 `prometheus = "0.14"`。
- `crate::logging::{Field, Logger, log}` 提供日志字段、返回类型和默认日志入口；`log::L()` 新建默认 tracing 后端，`Logger::With` 返回附加字段的新实例。
- 标准库 `Instant` 提供单调计时与正、负时间差分支所需的 duration 运算。

## 错误处理与边界

本模块没有 `Result` 或显式错误传播。`counter: None` 时，`Inc`/`Add` 静默忽略，`RateAt` 返回 `NaN`；这对应 Go 的 nil Counter 边界，但 Rust 的公开构造函数始终产生 `Some(counter)`，只有直接结构体构造才能创建空状态。

当采样时刻等于 `start` 时，elapsed 为 `0.0`：有正增量时 IEEE 浮点除法得到正无穷。采样时刻早于 `start` 时，本实现计算负 elapsed，因此速率可以为负；`logging_test.rs::test_rater_go_time_boundaries` 明确断言 `+1 / 0s == +∞` 和 `+1 / -0.1s == -10`，不能把这两个分支改为报错、饱和或零值而不改变 Go 对齐语义。

`RateAt` 的 Counter 读数发生在调用时，而不是参数 `instant` 所代表的时刻；测试时间注入只固定分母。并发更新会使同一个 `instant` 的多次调用得到不同分子。`Add` 不在本层拒绝非有限值或不合规增量；扩展时应先核对 `prometheus 0.14` 的 Counter 合约，不能擅自在此层增加与 Go 不一致的归一化。

## 并发与资源生命周期

所有变更方法都只要求 `&self`，并把同步责任交给可共享的 Prometheus Counter；本文件没有自建锁、任务、通道、异步运行时或后台资源。`RateTracer::clone` 不创建新的指标序列，而是共享底层 Counter，因而任一克隆或外部 Counter 持有者的增量都会被所有追踪器观察到。

`L()` 每次创建一个新的默认 `Logger` 并附加当次速率字符串，不持有 `RateTracer` 引用，也不会持续刷新字段；日志器中的 `speed` 是调用 `L()` 时的快照。追踪器析构无需显式清理，计数器底层资源随所有句柄的所有权生命周期释放。若在多线程业务中追踪局部工作量，必须给不同工作范围使用独立 Counter，或接受共享 Counter 带来的全局增量语义。

## 与 Go 版本的对应关系

Rust `RateTracer` 对齐 [`rate.go`](rate.go) 的 `start`、`base` 和嵌入式 `prometheus.Counter`；Rust 用 `Option<Counter>` 显式表达 Go 接口可能为 nil。`TraceRateOver`、`Rate`、`RateAt` 与 `L` 保留 Go 命名，crate 级 `allow(non_snake_case)` 支持这种迁移 API。

核心公式一致：两端都忽略构造前的计数值，读取调用时的当前计数，再除以从创建到指定时刻的秒数。Go 通过 `time.Time.Sub` 自然得到负 duration；Rust 因 `Instant::duration_since` 不接受反向差值，使用比较加显式负号恢复该语义。两端在零分母时都保留 IEEE 浮点结果，在空 Counter 时都返回 `NaN`。

差异有三点：第一，Go 通过嵌入 Counter 自动暴露 `Inc`/`Add`，Rust 显式写了同名包装方法；第二，Go 的字段不导出，Rust 字段公开以支持当前独立测试构造；第三，Go `L` 返回 zap logger，Rust 返回本 crate 的 tracing/capture 适配 `Logger`。Go `logging_test.go::TestRater` 验证三个正常采样点；Rust 在此基础上增加了零时长和负时长边界测试。Rust 生产接线尚未由仓库证据确认，因此迁移状态不能表述为完整替代 Go 调用链。

## 扩展指南

- 若改变速率算法，应优先修改 `RateAt`，并同步 [`logging_test.rs`](logging_test.rs) 与 [`parity_test.rs`](parity_test.rs)；至少保留基线 42 后得到 10、约 13.33、100 ops/s 的 Go 对齐样例，以及零/负时长行为。
- 若增加滑动窗口、瞬时速率或重置能力，应新增独立类型或清晰的新状态，而不要悄悄改变 `RateTracer` 的“自创建以来平均值”合约；同时评估克隆后状态是否共享。
- 若把该 API 接入 Rust 恢复建表链，应对照 Go `br/pkg/restore/snap_client/client.go` 的两个调用位置，确认增量发生在建表成功后，并验证并发 worker 共用 Counter 时的统计范围。
- 若调整 `L` 的字段名或格式，需保持 `speed`、两位小数和 `ops/s` 的兼容性，或同步日志消费者与独立测试。日志后端的扩展位置在 [`logging.rs`](logging.rs) 的 `Logger::With`/`log::L`，不应把后端逻辑塞进本文件。
- 若收紧三个字段的可见性，应先为测试提供独立构造辅助或可控时钟；Rust 测试必须继续放在 `logging_test.rs`/`parity_test.rs` 等独立文件中，不能内嵌回 `rate.rs`。
- 性能上，`Rate` 每次读取指标，`L` 还会分配格式化字符串和派生 Logger；高频热路径如需优化，应先测量，并保持可观察输出语义。

## 验证依据

- Rust 源码：[`rate.rs`](rate.rs)；确认 `RateTracer`、`TraceRateOver`、`Inc`、`Add`、`Rate`、`RateAt`、`L` 的签名与分支。
- crate 边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)；确认 library 根、Go 包映射、`prometheus 0.14`/metric 依赖、模块声明及 re-export。
- 下游实现：`pkg/lightning/metric/metric.rs::read_counter` 与 [`logging.rs`](logging.rs) 的 `Logger::With`、`default_logger`、`log::L`。
- Go 对照与生产接线：[`rate.go`](rate.go)、[`logging_test.go`](logging_test.go) 的 `TestRater`，以及 `br/pkg/restore/snap_client/client.go` 的 `createTablesBatch`/`createTables`。
- Rust 独立测试：[`logging_test.rs`](logging_test.rs) 的 `test_rater`、`test_rater_go_time_boundaries`；[`parity_test.rs`](parity_test.rs) 的速率对齐段落。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/logutil` 找到 `rate.rs`/`rate.go` 及测试；`query RateTracer --kind struct`、`query TraceRateOver --kind function` 同时定位 Go/Rust 定义；`node --file br/pkg/logutil/rate.rs --offset 1 --limit 220` 列出完整 85 行与“used by 0 files”。通用名调用图查询未返回可区分的直接调用边，因此调用关系另以精确文本检索和上述源文件核验，未据此推断 Rust 生产调用。
- 任务为纯文档分析，未运行 Cargo；交付时运行任务指定的 11 章节结构检查，并人工复核链接、事实边界和 Rust/Go 接线差异。

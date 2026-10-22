# `pkg/util/topsql/reporter/report_ticker.rs`

## 文件定位

用 `ReportTicker` 同时保存创建时的间隔快照和 `crossbeam_channel::Receiver<Instant>`，隐藏 ticker 通道的构造细节。

## 核心职责

1. 用 `DEFAULT_REPORT_TICKER_INTERVAL` 把 TopSQL 状态 crate 的默认上报秒数转换为 `Duration`；当前对应常量在 `pkg/util/topsql/state/state.rs` 中为 60。
2. 用 `REPORT_TICKER_INTERVAL: LazyLock<Mutex<Duration>>` 保存“后续新建 ticker”采用的进程级间隔。
3. 用 `ReportTicker` 同时保存创建时的间隔快照和 `crossbeam_channel::Receiver<Instant>`，隐藏 ticker 通道的构造细节。
4. 用 `set_report_ticker_interval_seconds_for_test` 临时替换全局间隔并返回一次性恢复闭包；用 `SetReportTickerIntervalSecondsForTest` 保留与 Go 测试辅助 API 一致的导出名。

它不负责启动线程、执行上报、停止 reporter 或动态重配已创建的 ticker；这些生命周期必须由调用方管理。

## 主要符号

- `DEFAULT_REPORT_TICKER_INTERVAL: Duration`：由 `crate::topsql_state::DefTiDBTopSQLReportIntervalSeconds` 在编译期换算而来，是非正测试输入的回退值。
- `REPORT_TICKER_INTERVAL: LazyLock<Mutex<Duration>>`：首次访问时初始化的进程级可变状态。互斥锁串行化创建时读取、测试覆盖和恢复。
- `ReportTicker { interval, receiver }`：公开类型，但字段私有，调用方不能替换接收端或伪造与接收端不一致的间隔。
- `ReportTicker::interval(&self) -> Duration`：返回创建时记录的间隔快照，主要用于断言和观测。
- `ReportTicker::recv(&self) -> Result<Instant, RecvError>`：委托到底层接收端，无期限等待下一次 tick。
- `ReportTicker::recv_timeout(&self, timeout) -> Result<Instant, RecvTimeoutError>`：委托到底层接收端，允许调用方为等待设置上限。
- `new_report_ticker() -> ReportTicker`：读取当前全局间隔，在释放互斥锁后用该快照调用 `crossbeam_channel::tick`。
- `set_report_ticker_interval_seconds_for_test(seconds) -> Box<dyn FnOnce() + Send + 'static>`：Rust 风格测试入口；正数解释为秒，零或负数恢复默认值，返回捕获旧值的恢复闭包。
- `SetReportTickerIntervalSecondsForTest(...)`：仅委托给上一函数；`#[allow(non_snake_case)]` 明确容纳 Go 风格名称。

## 执行流程

创建流程从 `new_report_ticker` 开始：锁住 `REPORT_TICKER_INTERVAL`，复制当前 `Duration`，若锁曾中毒则通过 `PoisonError::into_inner` 继续读取；锁守卫随后离开作用域。函数用该快照创建 `crossbeam_channel::tick(interval)`，并把同一快照与接收端一起放入 `ReportTicker`。调用方之后通过 `recv` 或 `recv_timeout` 等待由 crossbeam 产生的 `Instant`。

测试覆盖流程从 `set_report_ticker_interval_seconds_for_test` 开始：`seconds > 0` 时构造对应秒数的 `Duration`，否则选择默认间隔；函数在锁内用 `std::mem::replace` 原子地取得旧值并安装新值，然后返回一个 `FnOnce` 闭包。执行该闭包会重新加锁并写回旧值。`datamodel_1_aster_unit_test.rs::subscription_parsing_send_order_channel_full_and_ticker_match_go` 以 1 秒覆盖创建 ticker，断言 `interval()` 为 1 秒，并在 1200 毫秒超时内收到 tick，最后调用恢复闭包。

覆盖只影响覆盖之后调用 `new_report_ticker` 的实例。已经创建的 `ReportTicker` 持有自己的接收端和间隔快照，不会因全局值后续改变或恢复而重配。

## 数据与状态

全局状态只有一个 `Duration`，由 `LazyLock` 延迟初始化、由 `Mutex` 保护。`Duration` 是按值复制的，因此创建函数不把锁守卫或全局引用带入 ticker 生命周期。`ReportTicker` 的 `interval` 与创建 `receiver` 时传入的值来自同一次快照，两者在构造后保持一致。

恢复闭包捕获覆盖前的值而不是固定默认值，所以顺序嵌套的覆盖可以按后进先出方式逐层恢复。该接口没有生成令牌或所有权检查；若多个线程并发覆盖，或者恢复闭包不按覆盖的逆序执行，较早捕获的旧值可能覆盖较新的设置。相关测试用 `#[serial]` 串行化全局状态用例，调用方也应把覆盖限定在受控测试生命周期内。

## 依赖与调用关系

- 上游模块：`lib.rs` 用 `#[path = "report_ticker.rs"] pub mod report_ticker;` 公开本模块，但没有把其中符号再导出到 crate 根。
- Rust 直接使用者：`datamodel_1_aster_unit_test.rs` 调用 Rust 风格的设置函数、构造函数、`interval` 和 `recv_timeout`；`pkg/server/tests/servertestkit/testkit.rs` 导入 Go 风格设置函数，并把恢复闭包保存到测试 suite，在 `Drop` 时执行。
- 生产对照：Go `reporter.go::collectWorker` 调用 `report_ticker.go::newReportTicker`；Rust `reporter.rs::collectWorker` 当前直接依赖 `crossbeam_channel::tick`，所以测试覆盖入口不会改变它的周期。
- 下游标准库：`std::sync::{LazyLock, Mutex}` 管理初始化与互斥状态，`std::time::{Duration, Instant}` 表示间隔和触发时刻，`std::mem::replace` 完成可恢复替换。
- 下游外部依赖：`crossbeam-channel = "0.5"` 由本 crate 的 `Cargo.toml` 声明，提供 `tick`、`Receiver`、`RecvError` 和 `RecvTimeoutError`。
- 默认值依赖：`topsql_state` 在 `Cargo.toml` 中指向相邻 `../state` crate，本文件通过 crate 根的 `pub use topsql_state` 读取默认上报间隔。

RustCodeGraph 的文件节点确认了本文件 95 行源码及主要符号；精确 `query` 找到 `ReportTicker` 和 `new_report_ticker`。`callers` 查询在本地索引上未在 60 秒内返回，因此真实直接调用者以限定 `*.rs` 的文本引用复核，没有采用文件节点宽泛的 “used by” 列表作为调用证据。

## 错误处理与边界

`new_report_ticker` 和恢复闭包对互斥锁中毒都使用 `unwrap_or_else(|error| error.into_inner())`，选择保留并继续使用锁内状态，而不是 panic。代价是调用方不会收到“曾有持锁线程 panic”的显式错误信号。

设置函数不返回错误：正数安全转换为 `u64` 秒；零和负数统一表示恢复默认值，因此不会把零间隔传给 `crossbeam_channel::tick`。默认值从当前 `topsql_state` 常量得到，当前为 60 秒。等待错误不被包装：`recv` 原样返回 `crossbeam_channel::RecvError`，`recv_timeout` 原样返回 `RecvTimeoutError`，调用方负责区分超时和断开等下游语义。

本文件不提供显式停止方法。释放 `ReportTicker` 会释放其接收端；除此之外的调度、丢弃或断开行为由 `crossbeam_channel::tick` 实现决定，本文件没有增加额外保证。

## 并发与资源生命周期

全局间隔的每次读、写和恢复都在同一个互斥锁内完成，单次操作不会观察到部分写入。创建函数仅在复制 `Duration` 时持锁，不会在等待 tick 时持锁，因此慢速或阻塞接收不会阻塞其他测试覆盖操作。

`ReportTicker` 拥有 `Receiver<Instant>`；它的生命周期由拥有者控制，没有后台任务句柄、显式取消通道或 `Drop` 实现。`recv` 可能长期阻塞，需可取消等待时应由上层使用 `recv_timeout` 或把接收端纳入更高层选择机制。恢复回调是 `FnOnce + Send + 'static`，可跨线程转移且只能消费执行一次；如果调用方忘记执行，覆盖值会继续影响进程内以后创建的 ticker。服务器测试辅助结构通过 `Option<Box<dyn FnOnce() + Send>>` 在 `Drop` 中调用，体现了推荐的作用域清理模式。

## 与 Go 版本的对应关系

主要表示差异如下：Go 的 `newReportTicker` 直接返回 `*time.Ticker`，使用方从 `Ticker.C` 接收并可调用 `Stop`；Rust 返回自定义 `ReportTicker`，用 `recv`/`recv_timeout` 隐藏 `Receiver<Instant>`，没有对应的显式 `Stop`。Go 的设置入口接收 `int` 并返回 `func()`；Rust 核心入口接收 `i64` 并返回 `Box<dyn FnOnce() + Send + 'static>`，另有 Go 风格名称的薄委托。

接线状态尚不等价：Go `reporter.go::collectWorker` 确实通过 `newReportTicker` 取得周期并在退出时 `Stop`；Rust `reporter.rs::collectWorker` 仍绕过本模块直接创建固定默认周期。

## 扩展指南

若要让测试覆盖真正控制 Rust reporter 的生产收集循环，最小接入点是 `reporter.rs::collectWorker`：改为调用 `report_ticker::new_report_ticker`，并在 `crossbeam_channel::select!` 中接收其底层事件。由于 `receiver` 当前私有，接线前需选择稳定接口（例如提供受控接收端访问或封装选择行为），不能为绕过私有字段而复制间隔状态。修改时应同步独立的 `reporter_test.rs`，验证覆盖周期确实驱动上报，并保留取消路径不会被阻塞的测试。

若扩展测试覆盖 API，应维持以下不变量：非正数回到默认值；新 ticker 的 `interval()` 与实际构造间隔一致；覆盖仅影响之后创建的实例；恢复操作不在持锁期间执行外部代码。涉及并发覆盖时，建议新增专门的独立测试文件或在现有独立测试模块中加入序列化用例，明确规定嵌套恢复顺序，而不要把测试嵌入生产源文件。

若需要可动态重配的生产 ticker，本文件现有“创建时快照”模型并不满足要求；应先定义重配、取消、积压 tick 和关闭时等待的兼容语义，再选择通道或任务模型。该改动会影响 `collectWorker` 的性能和关闭行为，不能只改全局 `Duration`。

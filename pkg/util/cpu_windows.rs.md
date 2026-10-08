# `pkg/util/cpu_windows.rs`

## 文件定位

本文件是 `astersql-util` crate 的 Windows 进程 CPU 占用采样实现。模块由 [`pkg/util/lib.rs`](lib.rs) 中的 `pub mod cpu_windows` 公开挂载；核心采样函数 `GetCPUPercentage` 受 `#[cfg(windows)]` 约束，非 Windows 构建只会保留不依赖系统 API 的 FILETIME 换算辅助函数。crate 边界由 [`pkg/util/Cargo.toml`](Cargo.toml) 定义，Windows 目标通过 `windows-sys` 的 `Win32_Foundation` 与 `Win32_System_Threading` feature 取得所需 FFI。

RustCodeGraph 对 `GetCPUPercentage` 的精确查询定位到本文件第 41 行，但未报告仓库内 Rust 生产调用者。因此当前可确认的位置是“通用工具 crate 中公开的 Windows 平台能力”，不能据此声称它已经接入某条 SQL 请求或监控主链。

## 核心职责

- `GetCPUPercentage`：读取当前进程累计的用户态和内核态 CPU 时间，以本次与上次采样的 CPU 时间差除以墙钟时间差，再乘以 100，返回百分比。例如一个墙钟区间内消耗 2.5% 的单核时间时返回 `2.5`。
- `filetime_ticks_to_unix_nanos`：把 Windows `FILETIME` 的 100 纳秒 tick 从 Windows epoch 转换为 Unix epoch 纳秒，并刻意使用 `i64` 环绕运算对齐 Go `syscall.Filetime.Nanoseconds`。
- 保存跨调用采样基线：`LAST_INSPECT_UNIX_NANO` 和 `LAST_CPU_USAGE_TIME` 分别记录上次墙钟时间与上次累计 CPU 时间。

本文件不负责周期调度、指标上报、平滑处理或按 CPU 核数归一化；调用频率和结果消费由上层决定。

## 主要符号

- `WINDOWS_TO_UNIX_EPOCH_TICKS: u64`：Windows epoch（1601-01-01）到 Unix epoch（1970-01-01）的 FILETIME tick 差，值为 `116_444_736_000_000_000`。
- `LAST_INSPECT_UNIX_NANO: AtomicI64`：仅在 Windows 编译，初值为 0，保存上一次调用取得的 Unix 纳秒墙钟时间。
- `LAST_CPU_USAGE_TIME: AtomicI64`：仅在 Windows 编译，初值为 0，保存上一次调用取得的进程累计 CPU 纳秒数。
- `pub fn GetCPUPercentage() -> f64`：Windows 下的公开入口。它取得四个 `FILETIME` 输出，实际计算只使用 `KernelTime` 与 `UserTime`；`CreationTime`、`ExitTime` 是 `GetProcessTimes` 接口要求的输出参数。
- `pub(crate) fn filetime_ticks_to_unix_nanos(ticks: u64) -> i64`：crate 内可见的纯换算函数；不受 `cfg(windows)` 限制，使独立 Rust 测试可在非 Windows 主机验证 epoch 与单位换算。
- `fn now_unix_nano() -> i64`：Windows 私有辅助函数，通过 `SystemTime::now()` 获取 Unix 纳秒；若系统时间早于 Unix epoch，则以零时长代替错误。

## 执行流程

1. `GetCPUPercentage` 为 creation、exit、kernel、user 四个 `FILETIME` 分配零初始化存储。
2. 通过 `GetCurrentProcess()` 取得当前进程伪句柄，再调用 `GetProcessTimes` 填充时间；返回值为 0 时立即返回 `0.0`，且不会更新上次采样状态。
3. 局部闭包把 `FILETIME` 的高、低 32 位拼成 `u64` tick，交给 `filetime_ticks_to_unix_nanos` 转成 `i64` 纳秒。
4. 用环绕加法合并 user 与 kernel 累计时间，并通过 `now_unix_nano` 获取当前墙钟时间。
5. 以 `SeqCst` 顺序分别加载两个旧基线，计算 `((usageTime - lastUsage) / (nowTime - lastInspect)) * 100`；两个差值都采用 `i64::wrapping_sub`。
6. 以 `SeqCst` 顺序先保存新墙钟时间，再保存新 CPU 时间，最后返回刚计算的 `f64`。

首次调用的旧基线均为零，因此返回的是“进程自创建以来的累计 CPU 时间 / Unix epoch 至今的墙钟时间”，通常接近零；从第二次调用开始才代表相邻采样区间。

## 数据与状态

全部持久状态是两个进程级原子整数，没有实例、堆分配、缓存容器或外部句柄所有权。`FILETIME` 每个 tick 为 100 纳秒；转换先减 epoch tick，再乘 100。对于进程 CPU 时长，Windows 返回的 kernel/user FILETIME 本质上是持续时间，但实现仍完全复刻 Go `Filetime.Nanoseconds` 的 epoch 换算；两个分量相加时共同的 epoch 偏移会保留在累计值中，不过相邻采样相减会消掉这个常量。

状态对整个进程共享，所有调用者共同推进同一采样基线。因此返回值描述“当前调用与全局上一次调用之间”的区间，而不是某个调用者独享的区间。

## 依赖与调用关系

- 上游模块：[`pkg/util/lib.rs`](lib.rs) 公开声明 `cpu_windows`；同一文件还在 `cfg(test)` 下挂载 [`pkg/util/cpu_windows_test.rs`](cpu_windows_test.rs)。
- 生产调用：RustCodeGraph `query` 找到本文件的 `GetCPUPercentage`，但 `callers` 未返回仓库内 Rust 生产调用边；代码搜索同样只发现函数定义。当前文档因此不推断未证实的业务调用链。
- 下游标准库：`AtomicI64`/`Ordering::SeqCst` 管理共享采样基线，`SystemTime`/`UNIX_EPOCH` 提供墙钟时间，`std::mem::zeroed` 初始化 FFI 输出结构。
- 下游系统 API：`windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes}` 和 `windows_sys::Win32::Foundation::FILETIME`。
- Cargo 条件依赖：[`pkg/util/Cargo.toml`](Cargo.toml) 仅在 `cfg(windows)` 下启用 `windows-sys = 0.61` 及两个 Win32 feature，和源码条件编译边界一致。
- 测试调用：[`pkg/util/cpu_windows_test.rs`](cpu_windows_test.rs) 直接调用 `filetime_ticks_to_unix_nanos`；RustCodeGraph 的文件节点也显示本文件被该测试文件使用。

## 错误处理与边界

- `GetProcessTimes` 失败：返回 `0.0`，不暴露 Windows 错误码，也不改变采样基线。下一次成功调用仍与最近一次成功采样（或初始零值）比较。
- `GetCurrentProcess`：`windows-sys` 接口直接返回当前进程伪句柄，本实现没有 Go 版本中单独的句柄获取错误分支。
- 墙钟异常：`SystemTime` 早于 Unix epoch 时 `unwrap_or_default` 返回零时长；系统时钟回拨则可能使墙钟差为负。
- 零时间差：代码没有显式保护分母为零；IEEE-754 浮点除法可能产生 `NaN` 或正负无穷，而不会 panic。
- 整数范围：FILETIME 拼接使用 `u64`，随后转换到 `i64`；epoch 减法、乘 100、CPU 分量相加与采样差均使用环绕语义，不会因 debug overflow 检查而 panic。
- 百分比范围：结果不夹在 `0..=100`。多核进程在一个墙钟区间内可累计多个核的 CPU 时间，因此结果可超过 100；时钟回拨、极短区间或交错并发也可能产生负数或非有限值。
- 平台边界：非 Windows 目标不存在 `GetCPUPercentage` 符号；跨平台调用方必须用与平台匹配的条件编译或平台门面。

## 并发与资源生命周期

两个原子变量避免数据竞争和未定义行为，`SeqCst` 为各次单变量读写提供最强原子顺序。然而一次采样需要成对读取、成对写入两个独立原子，整体并不是原子事务：并发调用可能分别观察到来自不同调用的墙钟与 CPU 基线，也可能相互覆盖更新。因此它是线程安全的共享状态实现，但并不保证并发采样区间在语义上彼此一致；若上层要求稳定区间，应串行调用，或未来将两项状态置于同一个锁/一致快照结构中。

`GetCurrentProcess` 返回 Windows 伪句柄，本函数不创建需要关闭的真实句柄；四个 `FILETIME` 都是栈上临时值，调用结束即释放。没有后台线程、任务、通道、锁守卫或显式清理流程。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/cpu_windows.go`](cpu_windows.go)：

- 两者都保存 `lastInspectUnixNano` 与 `lastCPUUsageTime` 两个全局基线，计算公式和更新顺序一致。
- Go 用 `syscall.Rusage` 承载四个 `FILETIME`，Rust 直接声明四个 `windows_sys::FILETIME`；两者都只把 user 与 kernel 用于 CPU 累计值。
- Go 的 `syscall.Filetime.Nanoseconds()` 隐含 Windows-to-Unix epoch 换算；Rust 将该语义显式抽到 `filetime_ticks_to_unix_nanos`，并用 wrapping 运算复刻 `int64` 溢出行为。
- Go 对 `syscall.GetCurrentProcess()` 的错误单独返回 0；Rust 使用的 `windows-sys::GetCurrentProcess()` 签名不返回 `Result`，只有 `GetProcessTimes == 0` 的失败分支。
- Go 的两个全局 `int64` 没有同步保护；Rust 改为 `AtomicI64` 消除了原始数据竞争，但两个字段仍不能作为一对原子快照。
- Go 文件用 `//go:build windows` 排除其他平台；Rust 对 Windows API import、状态、入口和墙钟辅助函数分别使用 `#[cfg(windows)]`，同时特意保留纯换算函数供跨平台单元测试。

Go 同目录没有 `cpu_windows_test.go`；当前直接回归证据来自独立 Rust 测试 [`pkg/util/cpu_windows_test.rs`](cpu_windows_test.rs)，验证 epoch 恰好映射为 0，以及增加 `12_345_678` tick 映射为 `1_234_567_800` 纳秒。

## 扩展指南

- 修改采样公式、首次采样语义或失败策略时，优先修改 `GetCPUPercentage`，并同步核对 [`pkg/util/cpu_windows.go`](cpu_windows.go)，避免 Windows Rust/Go 行为漂移。
- 修改 FILETIME 单位、epoch 或溢出策略时，集中修改 `filetime_ticks_to_unix_nanos`，并在独立的 [`pkg/util/cpu_windows_test.rs`](cpu_windows_test.rs) 增加 epoch 前后、边界 tick 与环绕用例；不要把测试内嵌到生产源文件。
- 若要保证并发调用获得一致基线，不能只调整 `Ordering`；需要让墙钟与 CPU 时间成为一次不可分割的状态转换，并补充多线程独立测试。该变更还需评估锁竞争或原子打包的性能成本。
- 若要增加可诊断错误，需决定是否改变当前返回 `0.0` 的公开兼容契约，并评估调用方能否接受 `Result` 或额外指标；当前仓库内没有可证明的 Rust 生产调用边，接线前仍应重新查询。
- 若要加入调度或指标上报，应在消费层实现，本文件维持“读取当前进程累计值并计算相邻差”的平台适配职责。
- Windows API、feature 或版本变化必须同步 [`pkg/util/Cargo.toml`](Cargo.toml) 的 target-specific 依赖；公开模块边界变化则同步 [`pkg/util/lib.rs`](lib.rs)。

## 验证依据

- 源码：[`pkg/util/cpu_windows.rs`](cpu_windows.rs)，核对常量、两个原子状态、`GetCPUPercentage`、`filetime_ticks_to_unix_nanos`、`now_unix_nano` 及全部 `cfg(windows)` 边界。
- 模块与测试入口：[`pkg/util/lib.rs`](lib.rs)，核对公开模块和独立测试模块挂载。
- crate 配置：[`pkg/util/Cargo.toml`](Cargo.toml)，核对 `astersql-util`、`autotests = false` 以及 Windows 专用 `windows-sys` feature。
- Go 对照：[`pkg/util/cpu_windows.go`](cpu_windows.go)，核对采样公式、状态更新、系统调用和错误返回；同时参考 [`pkg/util/cpu_posix.go`](cpu_posix.go) 与 [`pkg/util/cpu_posix.rs`](cpu_posix.rs) 确认同名 API 的平台分工。
- 独立测试：[`pkg/util/cpu_windows_test.rs`](cpu_windows_test.rs)，核对 FILETIME epoch 和 100 纳秒单位转换断言。
- RustCodeGraph：`status` 显示索引含本仓库 Rust/Go 文件；`query GetCPUPercentage --kind function` 定位 Windows Rust、Windows Go 及 POSIX 同名实现；`query filetime_ticks_to_unix_nanos --kind function` 唯一定位本文件；文件节点显示 `pkg/util/cpu_windows_test.rs` 使用本文件；调用边查询未给出 Rust 生产调用者。
- 文本搜索：对 `pkg/util` 搜索 `mod cpu_windows`、`cpu_windows_test` 与 `GetCPUPercentage(`，确认模块/测试挂载和仓库内可见定义，不将 TopSQL 等其他 CPU 统计概念误认为本函数调用。
- 本任务是纯文档分析，按任务约束不运行 Cargo；最终以 11 个固定二级标题的结构检查和人工事实复核作为交付验证。

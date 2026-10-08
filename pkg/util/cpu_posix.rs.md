# `pkg/util/cpu_posix.rs`

## 文件定位

[`cpu_posix.rs`](cpu_posix.rs) 是 `astersql-util` crate 的 POSIX 进程 CPU 采样实现。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod cpu_posix` 暴露该模块；[`Cargo.toml`](Cargo.toml) 将 crate 根指定为 `lib.rs`，并通过 `libc = "0.2"` 提供 `getrusage`、`rusage`、`timeval` 和 `RUSAGE_SELF` 的平台绑定。

文件中的两个静态量和公开采样函数都带有 `cfg(any(unix, target_os = "linux", target_os = "macos", target_os = "freebsd"))`。其中 `unix` 已覆盖通常的 Linux、macOS 和 FreeBSD 目标，后面的三个目标条件是显式重复说明；非 Unix 构建不会得到这些静态量或 `GetCPUPercentage`。同 crate 另有 [`cpu_windows.rs`](cpu_windows.rs) 承担 Windows 实现，但当前 `lib.rs` 没有用条件编译互斥两个模块，因此调用方必须选择与目标平台相符的模块。

仓库全文引用检查显示，目前生产 Rust 代码没有调用 `cpu_posix::GetCPUPercentage`；直接使用者只有独立测试 [`cpu_posix_1_aster_unit_test.rs`](cpu_posix_1_aster_unit_test.rs)。因此它当前是已公开、已测试但尚未接入应用主链的底层工具能力，不能据此声称服务运行时已经消费该指标。

## 核心职责

该文件负责把两类累计量转换成“自上次采样以来”的进程 CPU 百分比：

1. 通过 `libc::getrusage(libc::RUSAGE_SELF, ...)` 读取本进程累计用户态时间 `ru_utime` 和系统态时间 `ru_stime`。
2. 用 `timeval_nano` 将两段 `timeval` 转成纳秒并相加，得到本次累计 CPU 时间。
3. 用 `SystemTime` 取得本次 Unix 墙钟纳秒。
4. 读取上次累计 CPU 时间和上次墙钟时间，按 `(本次 CPU - 上次 CPU) / (本次墙钟 - 上次墙钟) * 100` 计算比例。
5. 保存本次样本，供下一次调用使用。

返回值表达百分数而不是 0 到 1 的比例，例如 `2.5` 表示 `2.5%`。由于测量的是整个进程的累计 CPU 时间，多线程进程在多个核心上并行工作时结果可以超过 `100%`；源码没有按逻辑 CPU 数归一化或截断。

## 主要符号

- `LAST_INSPECT_UNIX_NANO: AtomicI64`：最近一次采样的 Unix 墙钟纳秒，初始值为 `0`，只在支持的 Unix 目标上存在。
- `LAST_CPU_USAGE_TIME: AtomicI64`：最近一次采样的进程累计 CPU 纳秒，初始值为 `0`，只在支持的 Unix 目标上存在。
- `pub fn GetCPUPercentage() -> f64`：模块公开入口。采集真实进程样本、计算百分比、更新两个全局状态并返回结果。名称保留 Go 风格，因此文件级允许 `non_snake_case`。
- `pub(crate) fn cpu_percentage_from_samples(...) -> f64`：crate 内可见的纯计算函数。四个参数依次是上次墙钟、上次 CPU、本次墙钟、本次 CPU，方便独立测试公式和浮点边界。
- `fn timeval_nano(tv: libc::timeval) -> i64`：私有转换函数，将秒乘 `1_000_000_000`、微秒乘 `1_000` 后相加；显式转换到 `i64` 兼容 `c_long` 宽度不同的 Unix 目标。
- `fn now_unix_nano() -> i64`：私有时钟函数，取得 `SystemTime::now()` 相对 `UNIX_EPOCH` 的时长并转换为纳秒。

文件没有自定义类型、trait 或 `impl`。`#![allow(dead_code, non_snake_case)]` 与当前“公开但尚无生产调用”的迁移状态一致。

## 执行流程

`GetCPUPercentage` 的执行顺序是：

1. 将 `libc::rusage` 零初始化，以便系统调用失败时结构仍有确定值。
2. 对当前进程调用 `getrusage(RUSAGE_SELF)`；返回码被有意忽略。
3. 分别把 `ru_utime`、`ru_stime` 转成纳秒并求和。
4. 读取当前墙钟时间；若系统时间早于 Unix epoch，`duration_since` 的错误被 `unwrap_or_default` 转为零时长。
5. 以 `SeqCst` 顺序分别载入两个上次样本，调用 `cpu_percentage_from_samples`。
6. 以 `SeqCst` 顺序先保存本次墙钟、再保存本次 CPU 累计时间，最后返回已计算的值。

第一次调用时两个旧样本均为零，因此分母近似为从 Unix epoch 至今的纳秒数、分子为进程自启动后的累计 CPU 纳秒，结果通常接近零；第一次调用主要起到建立基线的作用。独立测试也先丢弃一次真实调用的结果，再短暂休眠并检查第二次结果。

## 数据与状态

状态只存在于两个进程级静态 `AtomicI64` 中，不按线程、请求或会话隔离。CPU 样本是 `ru_utime + ru_stime` 的单调累计值在正常系统行为下的差，墙钟样本来自 `SystemTime` 而不是单调时钟。

`cpu_percentage_from_samples` 本身不修改状态，也不校验输入：正的 CPU 增量和正的墙钟增量产生通常意义上的百分比；零墙钟增量遵循 IEEE 754 除法，可产生正/负无穷或 `NaN`；墙钟回拨、系统调用失败后归零、手工传入递减样本时可得到负值。结果也不限制上界。

时间最终都存入 `i64`。`SystemTime::as_nanos()` 原本返回 `u128`，源码使用 `as i64` 直接转换，极远未来超出 `i64` 范围时会截断；当前实现没有溢出检测。`timeval_nano` 的整数乘加同样没有显式溢出处理。

## 依赖与调用关系

上游装配关系为 `pkg/util/Cargo.toml` → `pkg/util/lib.rs` → `pub mod cpu_posix`。RustCodeGraph 将目标文件识别为含 5 个符号的已索引文件，并记录 [`cpu_posix_1_aster_unit_test.rs`](cpu_posix_1_aster_unit_test.rs) 使用它。仓库 `rg` 复核没有发现其他 Rust 直接引用，因此当前不存在可证实的生产上游调用边。

`GetCPUPercentage` 的下游关系为：

- `libc::getrusage` 和 `libc::RUSAGE_SELF`：取得当前进程资源使用量；
- `timeval_nano`：转换用户态和系统态累计时间；
- `now_unix_nano`：取得墙钟采样点；
- `AtomicI64::{load, store}` 与 `Ordering::SeqCst`：读写跨调用基线；
- `cpu_percentage_from_samples`：完成纯数学计算。

这段代码不进行 I/O、网络访问、任务调度或跨 crate 回调。`libc` 是本文件唯一直接使用的外部 crate 依赖。

## 错误处理与边界

实现刻意保持 Go 版本的宽松错误策略：`getrusage` 返回码被忽略。由于 `rusage` 预先清零，失败时本次 CPU 累计时间会变成零；如果已有非零旧样本，差值可能为负，同时失败样本仍会覆盖全局基线。函数没有返回 `Result`，调用者无法区分真实的低/负采样和系统调用失败。

`now_unix_nano` 对 Unix epoch 之前的系统时间使用零值兜底，不会 panic。纯计算函数对零间隔不做保护；测试 `cpu_percentage_preserves_go_floating_point_zero_interval_behavior` 明确要求 CPU 差为正且墙钟差为零时返回无穷大。真实采样测试只约束连续两次正常采样的第二次结果为有限且非负，并不证明时钟回拨、系统调用失败或并发调用场景。

扩展时不得把“通常非负”误写成函数不变量，也不应擅自钳制到 `0..=100`，因为多核进程可能合法超过 `100%`，且当前 Go 对照保留原始浮点除法语义。

## 并发与资源生命周期

函数不分配长期资源，也不创建线程、锁、文件描述符或需要显式释放的对象；`rusage` 是栈上值，系统调用返回后即结束其生命周期。两个原子静态量的生命周期覆盖整个进程，无法重置，也没有按实例销毁过程。

所有原子读写使用 `SeqCst`，所以单个载入和存储不存在数据竞争并参与全局顺序；但两个值不是一个原子快照，完整的“读取旧墙钟/旧 CPU—计算—写入新墙钟/新 CPU”也没有锁或 compare-and-swap 保护。多个线程同时调用时可能交错读取或写入来自不同采样的墙钟和 CPU 值，导致区间配对不一致。`SeqCst` 不等价于对整个函数串行化，现有测试未覆盖并发调用。

若未来生产调用可能并发，安全扩展需要先明确所需语义：可以由上游保证单一采样者，或把成对状态和完整更新放入同一互斥临界区；仅把现有原子顺序换成另一种 `Ordering` 不能解决成对一致性。

## 与 Go 版本的对应关系

直接对照文件是 [`cpu_posix.go`](cpu_posix.go)。两版都：

- 只面向 Linux、Darwin、FreeBSD 或一般 Unix；
- 保存 `lastInspectUnixNano` 和 `lastCPUUsageTime` 两个进程级基线；
- 查询 `RUSAGE_SELF`，相加用户态与系统态 CPU 时间；
- 忽略资源查询错误；
- 使用同一差分公式并在返回前更新基线。

Rust 的 `LAST_INSPECT_UNIX_NANO`、`LAST_CPU_USAGE_TIME` 分别对应 Go 的两个包级 `int64`。`libc::timeval` 的秒/微秒换算对应 Go `syscall.Timeval.Nano()`；`now_unix_nano` 对应 `time.Now().UnixNano()`。

可见差异包括：Rust 用 `AtomicI64` 避免对单个状态字段的未定义数据竞争，而 Go 原文件是普通包级变量；Rust 抽出 `cpu_percentage_from_samples` 以便独立测试；Rust 在系统时间早于 epoch 时返回零，而 Go 的 `UnixNano()` 可返回负值；Rust 具有 `u128` 到 `i64` 的直接转换边界。上述差异没有改变正常单调用路径的核心公式，但原子化也没有让两个字段成为不可分割的样本对。

## 扩展指南

- 修改百分比公式、零间隔行为或样本边界时，优先修改 `cpu_percentage_from_samples`，并在独立文件 [`cpu_posix_1_aster_unit_test.rs`](cpu_posix_1_aster_unit_test.rs) 增加正常、零间隔、负增量和多核大于 `100%` 等用例；不要把测试内嵌回生产文件。
- 修改系统采样方式时，入口是 `GetCPUPercentage`，转换行为位于 `timeval_nano`，时钟行为位于 `now_unix_nano`。需同步核对 [`cpu_posix.go`](cpu_posix.go)；若有意偏离 Go，必须记录兼容性理由。
- 若要暴露失败，需评估把 `f64` 改成 `Result<f64, _>` 对所有未来调用方的 API 影响，并决定失败时是否保留旧基线；当前静默失败行为是明确的 Go 兼容选择。
- 若要支持并发采样，必须保证墙钟与 CPU 基线的成对更新，并新增多线程独立测试；单独调整原子内存序不足以修复区间交错。
- 若要接入应用监控，应在真实消费模块显式调用并定义采样频率、单一采样者和首样本丢弃策略。目前没有生产调用边，不能仅凭 `pub mod` 认为已经接线。
- 修改平台范围时，需要同时检查本文件的 `cfg`、`lib.rs` 的模块装配、`Cargo.toml` 的目标依赖和 Windows 对应实现，避免同一目标同时暴露语义不一致的入口。

## 验证依据

- Rust 源码：[`pkg/util/cpu_posix.rs`](cpu_posix.rs)，核对两个条件编译静态量、`GetCPUPercentage`、`cpu_percentage_from_samples`、`timeval_nano` 和 `now_unix_nano`。
- crate 边界：[`pkg/util/Cargo.toml`](Cargo.toml) 与 [`pkg/util/lib.rs`](lib.rs)，核对 `astersql-util`、`lib.rs` crate 根、`libc` 依赖、公开模块和测试模块装配。
- Go 对照：[`pkg/util/cpu_posix.go`](cpu_posix.go)，核对构建约束、状态字段、系统调用、差分公式、错误忽略与更新顺序。
- 独立测试：[`pkg/util/cpu_posix_1_aster_unit_test.rs`](cpu_posix_1_aster_unit_test.rs) 中的 `cpu_percentage_uses_process_time_delta_over_wall_time_delta`、`cpu_percentage_preserves_go_floating_point_zero_interval_behavior` 和 `cpu_percentage_reads_real_process_usage`。仓库未发现直接覆盖该函数的 Go 测试。
- RustCodeGraph：`status` 显示索引可用；`files --filter pkg/util/cpu_posix.rs` 报告目标文件含 5 个符号；`query` 分别定位四个函数；`node --file pkg/util/cpu_posix.rs --offset 1 --limit 140` 返回完整目标源码并报告唯一使用文件为 `pkg/util/cpu_posix_1_aster_unit_test.rs`。`callers`/`callees` 使用路径限定与稳定 ID 的查询未产生可用输出，因此调用关系另以索引的 used-by 结果和仓库 `rg` 直接复核，未虚构图边。
- 人工边界复核：确认首样本语义、多核可超过 `100%`、零分母浮点行为、忽略系统调用错误、非单调墙钟以及两个原子字段不能组成事务性快照。

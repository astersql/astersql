# `pkg/util/cpu/cpu.rs`

## 文件定位

[`cpu.rs`](./cpu.rs) 是 `astersql-util-cpu` crate 的实现文件；[`lib.rs`](./lib.rs) 以 `mod cpu; pub use cpu::*;` 将这里的公开 API 提升到 crate 根。它提供两类进程级能力：后台观测当前进程相对于 cgroup CPU 份额的平滑使用率，以及查询当前进程可用的逻辑 CPU 数。

在服务主链上，[`cmd/tidb-server/main.rs`](../../../cmd/tidb-server/main.rs) 启动 `resourcemanager::InstanceResourceManager`，而 [`pkg/resourcemanager/rm.rs`](../../resourcemanager/rm.rs) 的 `ResourceManager` 构造、启动和停止本文件的 `Observer`。采样快照由 [`pkg/resourcemanager/scheduler/cpu_scheduler.rs`](../../resourcemanager/scheduler/cpu_scheduler.rs) 的 `CPUScheduler::Tune` 读取，并映射为线程池扩容、缩容或保持命令。因此本文件是“进程 CPU/cgroup 观测”到“进程内资源调度”之间的底层数据源，不负责调度策略本身。

[`Cargo.toml`](./Cargo.toml) 指定库入口为 `lib.rs`，默认启用 `failpoints` feature，并依赖 `astersql-util-cgroup`、`astersql-util-mathutil`、`fail`、`libc`、`log` 和 `prometheus`。`test-support` feature 只开放跨 crate 测试所需的 failpoint 辅助入口。

## 核心职责

- `Observer` 每约 100 ms 读取本进程累计用户态/系统态 CPU 时间，并用相邻采样差除以墙钟差和 cgroup CPU 份额，得到瞬时 CPU 使用率（`Observer::Start`、`observe`）。
- `ObserverState::cpu` 用参数 `(factor = 0.95, warmup_window = 10)` 的 `mathutil::ExponentialMovingAverage` 平滑瞬时值；前 10 个样本是算术平均，之后采用 EMA（`NewCPUObserver`，以及 `pkg/util/mathutil/exponential_average.rs::Add`）。
- 将最新平滑值发布到进程级原子快照 `CPU_USAGE`，并在可选指标已安装时更新 `metrics::EMACPUUsageGauge`（`Observer::Start`）。
- 在初始 cgroup 探测失败时发布 `UNSUPPORTED = true`，使上层调度器能够降级为 `Hold`，避免把“无数据”误判为“低负载”（`Observer::Start`、`GetCPUUsage`）。
- 提供跨平台的逻辑 CPU 数查询和 Unix 进程 CPU 时间读取（`GetCPUCount`、`getCPUTime`）。

## 主要符号

- `static CPU_USAGE: AtomicU64`：把 `f64` 的位模式存入原子整数；`GetCPUUsage` 用 `f64::from_bits` 还原。与 `UNSUPPORTED` 一样使用 `Ordering::SeqCst`，形成进程级共享快照。
- `static UNSUPPORTED: AtomicBool`：记录启动时是否无法取得 cgroup CPU 信息。它只在 failpoint 或初始 `GetCgroupCPU` 失败时置为 `true`；成功启动不会主动清回 `false`。
- `pub fn GetCPUUsage() -> (f64, bool)`：无锁读取最近一次平滑值和“不支持”标志。当前直接 Rust 生产调用者是 `CPUScheduler::Tune`；独立测试也读取它验证观测和降级行为。
- `struct ObserverState`：受互斥锁保护的可变采样状态，包含上次用户态时间 `utime`、系统态时间 `stime`、墙钟纳秒 `now` 和非线程安全 EMA `cpu`。
- `pub struct Observer`：拥有共享 `state`、退出通道发送端 `exit` 和后台线程句柄 `worker`。字段私有，生命周期只能通过 `NewCPUObserver`、`Start`、`Stop` 控制。
- `pub fn NewCPUObserver() -> Observer`：初始化时间基线和 EMA，但不创建线程。CPU 时间基线从零开始，与 Go 版本一致。
- `Observer::Start(&mut self)`：先探测 cgroup，再建立通道和采样线程；失败时只设置降级状态并返回。
- `Observer::Stop(&mut self)`：丢弃退出发送端使接收端断开，再 `join` 当前 worker。没有 worker 时是空操作。
- `fn observe(&mut ObserverState) -> f64`：计算单次归一化 CPU 使用率并推进三项时间基线。
- `pub(crate) fn cpu_share_from_result(...) -> f64`：保留 Go 忽略采样期 cgroup 错误的行为；错误被替换成 `CPUUsage::default()` 后调用 `CPUShares`。
- `setup_cgroup_cpu_error_failpoint_for_test()`：仅在 `test` 或 `test-support` 下公开，启用 `GetCgroupCPUErr` 并返回负责清理 failpoint 的 `FailScenario`。
- `getCPUTime()`：Unix 上调用 `libc::getrusage(RUSAGE_SELF)` 并将 `timeval` 截断为毫秒；非 Unix 返回 `io::ErrorKind::Unsupported`。
- `pub fn GetCPUCount() -> i32`：先允许 `mockNumCpu` failpoint 覆盖，否则读取 `std::thread::available_parallelism()`，失败回退为 `1`。
- `fn now_unix_nano() -> i64`：取得 Unix epoch 后的系统时间纳秒；epoch 之前的时间被 `unwrap_or_default` 折为零。

## 执行流程

1. `ResourceManager::new_with_schedulers` 调用 `NewCPUObserver`，此时记录构造时墙钟，CPU 使用率快照仍为进程全局旧值或初始零值。
2. `tidb-server` 的启动路径调用 `InstanceResourceManager.Start()`；资源管理器加锁后调用 `Observer::Start`。
3. `Start` 先执行 `GetCgroupCPUErr` failpoint，再调用 `cgroup::GetCgroupCPU()`。任一失败路径都把 `UNSUPPORTED` 设为 `true`，不创建通道或 worker；真实探测错误还会记录日志。
4. 探测成功后创建 MPSC 通道并启动线程。线程以 `recv_timeout(100 ms)` 兼作定时器和退出等待：只有超时才采样，收到消息或发现发送端断开都会结束循环。
5. 每次采样在 `state` 互斥锁内调用 `observe`。`getCPUTime` 给出累计毫秒值；代码乘以 `1_000_000` 转成纳秒，再用相邻 CPU 时间差除以相邻墙钟差，合并用户态和系统态速率，最后除以 `CPUUsage::CPUShares()`。
6. `CPUShares` 在 period/quota 均为正时返回 `quota / period`，否则返回 `NumCPU`；这样使用率表达为当前进程消耗的 CPU 相对于容器或可见 CPU 容量的比例。
7. 瞬时值进入 EMA，平滑值写入 `CPU_USAGE`，并在 `EMACPUUsageGauge` 为 `Some` 时同步写入 Prometheus Gauge。
8. 资源调度循环每 100 ms 调用调度器；超过调容防抖间隔后，`CPUScheduler::Tune` 读取 `GetCPUUsage`，在不支持时保持，低于 `0.5` 时扩容，高于 `0.7` 时缩容。
9. 停机路径先调用 `Observer::Stop`。`self.exit.take()` 丢弃唯一发送端，worker 的 `recv_timeout` 返回 `Disconnected` 并退出；`join` 保证采样线程完成后再继续资源管理器收尾。

`GetCPUCount` 不经过上述后台流程。当前 Rust 直接用途包括 [`pkg/lightning/config/config.rs`](../../lightning/config/config.rs) 的默认并发核数，以及 Windows 下 [`pkg/dxf/framework/scheduler/autoscaler.rs`](../../dxf/framework/scheduler/autoscaler.rs) 在测试环境无法取得 DXF task manager 时的本机回退值。

## 数据与状态

`CPU_USAGE` 和 `UNSUPPORTED` 是 crate/进程级全局状态，多个 `Observer` 实例会写同一份快照。`reset_test_state` 仅在单元测试编译时存在；[`lib.rs`](./lib.rs) 中的 `TEST_LOCK` 用于串行化会修改这些全局量的 crate 内测试。

每个 `Observer` 自有一个 `Arc<Mutex<ObserverState>>`。当前只有该实例的 worker 修改状态，但 `Arc` 让状态可以安全移入线程，`Mutex` 弥补 EMA 本身非线程安全。三个时间字段保存“上一采样点”，不是累计业务指标；每轮 `observe` 在返回前更新它们。

首次样本的 `utime`/`stime` 基线为零，因此分子包含进程启动以来的累计 CPU 时间，而分母只覆盖观察器构造后的墙钟时间；这与 Go 实现逐字段零值初始化的行为一致，可能令首个瞬时样本偏高，随后由 10 样本预热平均缓和。源码没有把使用率钳制到 `[0, 1]`；配额、并行执行、计时误差或异常分母均可能产生范围外值。

`UNSUPPORTED` 表示初始 cgroup 能力探测失败，不代表任何一次采样错误。采样阶段 cgroup 查询失败不会修改它；`cpu_share_from_result` 会用全零 `CPUUsage` 继续计算。

## 依赖与调用关系

上游链路为：

`cmd/tidb-server/main.rs::runMain` → `InstanceResourceManager.Start` → `ResourceManager::Start` → `Observer::Start` → worker → `observe`。

消费链路为：

`ResourceManager::schedule` → `CPUScheduler::Tune` → `GetCPUUsage` → `Overclock / Downclock / Hold`。

停机链路为：

信号处理或 keyspace 激活退出 → `InstanceResourceManager.Stop` → `ResourceManager::Stop` → `Observer::Stop`。

直接下游依赖包括：

- `cgroup::GetCgroupCPU` 和 `CPUUsage::CPUShares`：探测 cgroup v1/v2 配额并计算有效 CPU 份额。
- `mathutil::NewExponentialMovingAverage`、`Add`、`Get`：保存预热平均和后续 EMA 状态。
- `libc::getrusage`：Unix 进程累计 CPU 时间。
- `std::sync::mpsc`、`Mutex`、`JoinHandle`：定时/退出协调与线程生命周期。
- `fail`：`GetCgroupCPUErr`、`mockNumCpu` 两个故障注入点。
- `log` 与可选 `prometheus::Gauge`：错误记录和观测指标发布。

RustCodeGraph 的精确查询确认 `GetCPUUsage`/`NewCPUObserver` 被 `cpu_test.rs::TestCPUValue` 调用，并把本文件标记为被资源管理、Lightning、DXF 等路径使用；对通用名称 `Start`、`observe` 的图查询混入其他语言同名符号。因此生产调用边以上述文件限定搜索和相邻源码为最终依据，而不是采用未消歧的图结果。

## 错误处理与边界

- 初始 `GetCgroupCPU` 失败是显式降级：设置 `UNSUPPORTED`、记录错误并且不启动线程；上层 CPU 调度器据此返回 `Hold`。
- `getCPUTime` 失败时，`observe` 记录错误并把本轮累计用户/系统时间视为 `(0, 0)`，仍然推进状态。这可能产生负速率；下一次成功采样又可能产生补偿性正跃迁，源码不做过滤。
- 采样期 `GetCgroupCPU` 错误被有意忽略。默认 `CPUUsage` 的 `NumCPU` 为零，因此 `CPUShares()` 为 `0.0`；浮点除零不会 panic，但可能发布 `inf` 或 `NaN`。`migration_aster_unit_test.rs::sampling_cgroup_error_uses_go_zero_value_cpu_share` 只验证零 share 的 Go 对齐行为，没有断言最终采样值。
- 墙钟来自 `SystemTime` 而非单调时钟；时钟回拨、相同纳秒采样或 `i64` 转换溢出没有专项保护，可能导致非正 duration 或异常比率。
- worker 对中毒的状态互斥锁使用 `expect`，锁中毒会令后台线程 panic；`Stop` 忽略 `join` 返回的 panic 结果。
- `EMACPUUsageGauge` 是 `static mut Option<Gauge>`。worker 通过裸指针读取以避免创建普通共享引用；指标未安装时跳过。其初始化/替换必须由外部保证不与采样线程并发，否则该文件本身没有同步保护。
- 非 Unix 平台 `getCPUTime` 恒返回 `Unsupported`，但 `Start` 并不会据此拒绝启动；worker 会按上述 `(0, 0)` 降级路径继续运行。
- `Stop` 对未启动或已经停止的本 `Observer` 是空操作。源码没有 `Drop` 实现；直接丢弃正在运行的观察器会丢弃 sender 和线程句柄，通道断开后 worker 可自行退出，但调用方无法再等待它完成。
- API 没有声明支持重复 `Start`。再次启动会替换现有 sender/handle；旧 sender 的丢弃会促使旧线程退出，但旧 `JoinHandle` 被丢弃而不等待。安全扩展时不应依赖重复启动行为。

## 并发与资源生命周期

正常生命周期是“构造一次、启动一次、停止一次”。`Observer` 的 sender 与 worker 都用 `Option` 表示未启动/已停止状态。停止不发送数据，而是通过 `take()` 丢弃 sender；接收端把 `Disconnected` 与显式收到消息同样视作退出。由于采样线程持有 `Arc<Mutex<ObserverState>>`，状态至少存活到 worker 退出。

全局快照使用 `SeqCst` 原子操作，读取不需要获取 `ObserverState` 的锁。`CPU_USAGE` 将完整 64 位浮点位模式作为一个原子值发布，不会出现拆分读取；数值有效性仍取决于采样计算。`UNSUPPORTED` 与 `CPU_USAGE` 是两个独立原子量，`GetCPUUsage` 不提供二者作为单一事务快照的一致性承诺。

EMA 的全部读写发生在持有 `state` 锁期间；Prometheus 更新也发生在该临界区内。锁因此覆盖采样、平滑和指标写入的完整一轮，但慢指标写入会延长临界区。当前没有其他生产路径获取该锁，所以主要作用是满足跨线程所有权与未来访问安全。

测试中 `TEST_LOCK` 只串行化同 crate 用例，跨 crate 的 failpoint 集成测试依赖 `FailScenario` 的作用域清理；并行运行其他会读写相同进程级 failpoint/原子状态的测试时仍需由测试编排保证隔离。

## 与 Go 版本的对应关系

[`cpu.go`](./cpu.go) 是主要语义基准。两版都使用全局 CPU 快照和 unsupported 标志、100 ms 采样周期、`0.95/10` EMA、`(用户态速率 + 系统态速率) / CPU share` 公式，以及“初始 cgroup 失败则不启动，采样期 cgroup 错误则忽略”的行为。

关键实现差异如下：

- Go 用 goroutine、ticker、channel 和 `WaitGroup`；Rust 用 OS 线程、`recv_timeout`、MPSC 断连和 `JoinHandle`。Rust 的超时时间从每次 `recv_timeout` 调用重新计算，不是独立 ticker 的固定节拍。
- Go 的采样状态直接位于 `Observer`；Rust 将其放入 `Arc<Mutex<ObserverState>>` 以跨线程共享所有权。
- Go 用 `gosigar.ProcTime.Get(pid)`；Rust 在 Unix 上直接用 `getrusage(RUSAGE_SELF)`，并为非 Unix 提供错误桩。两者都把累计时间表达为毫秒。
- Go 的 `GetCPUCount` 返回 `runtime.GOMAXPROCS(0)`；Rust 返回 `available_parallelism()`。二者都面向当前进程可用的并行度，但受运行时设置、CPU affinity 和容器实现影响时不保证数值完全相同。
- Go 直接写全局 `metrics.EMACPUUsageGauge`；本 crate 的 `lib.rs` 当前只提供可选 Gauge，未设置时静默跳过。
- Go `Stop` 关闭已关闭 channel 会 panic；Rust `Stop` 因 `Option::take` 可重复调用。Go 的构造函数预先创建退出 channel，Rust 在 `Start` 成功探测后才创建。
- Rust 增加 `test-support` helper 与 crate 内复位函数，便于独立 Rust 测试和跨 crate 集成测试复现 Go failpoint 行为，不属于运行时业务接口。

测试对应关系：[`cpu_test.rs`](./cpu_test.rs) 的 `TestCPUValue` 移植 Go 容器采样测试；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 补充 CPU 数、进程时间单调性、干净停机、初始 cgroup 失败和采样期 cgroup 错误语义；[`pkg/resourcemanager/scheduler/tests/cpu_failpoint_integration.rs`](../../resourcemanager/scheduler/tests/cpu_failpoint_integration.rs) 验证 unsupported 最终令调度器 `Hold`。Go 的 [`cpu_test.go`](./cpu_test.go) 还直接覆盖 failpoint 场景及其调度结果。

## 扩展指南

- 修改采样公式、周期或 EMA 参数时，主要接入点是 `observe`、`Observer::Start` 和 `NewCPUObserver`。必须同时核对 `cpu.go`，并在独立的 [`cpu_test.rs`](./cpu_test.rs) 或 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 增加确定性测试；不要把测试内嵌进生产源文件。
- 增加新的错误状态时，不要复用 `UNSUPPORTED` 表达瞬时采样失败，除非同步修改 `CPUScheduler::Tune` 的降级契约和跨 crate 集成测试。需要一致读取多个字段时，应考虑用单个快照结构或版本机制，而不是继续增加互相独立的原子量。
- 改动 cgroup 归一化时应同步检查 `pkg/util/cgroup/cgroup_cpu.rs::CPUUsage::CPUShares`，覆盖无限额、零/负 quota、NumCPU 为零、v1/v2 和读取错误。特别要决定是否继续保留 Go 的浮点除零行为。
- 改动线程生命周期时应明确重复 `Start`、重复 `Stop`、`Drop`、worker panic 和启动失败后的状态机；至少增加“无泄漏退出”和“不会同时存在多个采样 worker”的独立测试。
- 替换计时时钟或 CPU 时间来源时要保持单位换算清晰，并测试首样本、时钟不前进/回拨、累计计数异常下降和长时间运行溢出。若有意修正首样本偏高，属于与 Go 行为不同的变更，应单独记录兼容性依据。
- 修改指标发布时应消除或封装 `static mut` 的并发初始化约束，并验证 Gauge 未安装、已安装和并发停机三种情况；当前可选指标不是本文件 CPU 快照正确性的前提。
- 修改 `GetCPUCount` 时需同步检查 Lightning 配置与 DXF 回退调用者，并明确要对齐 Go `GOMAXPROCS` 还是 Rust/操作系统可用并行度。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/util/cpu` 找到 `cpu.rs`、`cpu_test.rs`、`migration_aster_unit_test.rs`、`lib.rs` 及 Go 对照文件。
- 源码与符号：[`cpu.rs`](./cpu.rs) 的 `CPU_USAGE`、`UNSUPPORTED`、`GetCPUUsage`、`ObserverState`、`Observer`、`NewCPUObserver`、`Observer::{Start,Stop}`、`observe`、`cpu_share_from_result`、`getCPUTime`、`GetCPUCount`、`now_unix_nano`。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)；前者确认依赖/features，后者确认重导出、可选 Gauge 和测试模块装配。
- 下游实现：`pkg/util/cgroup/cgroup_cpu.rs::CPUUsage::CPUShares` 和 `pkg/util/mathutil/exponential_average.rs::{NewExponentialMovingAverage, Add, Get}`。
- 上游/消费方：`cmd/tidb-server/main.rs::runMain`、`pkg/resourcemanager/rm.rs::ResourceManager::{new_with_schedulers, Start, Stop}`、`pkg/resourcemanager/scheduler/cpu_scheduler.rs::CPUScheduler::Tune`、`pkg/lightning/config/config.rs::cpu_count`、`pkg/dxf/framework/scheduler/autoscaler.rs::GetExecCPUNode`。
- Go 对照：[`cpu.go`](./cpu.go) 和 [`cpu_test.go`](./cpu_test.go)；Rust 测试：[`cpu_test.rs`](./cpu_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 与资源调度器跨 crate failpoint 集成测试。
- RustCodeGraph 精确 `query/node/callers/callees` 与路径限定 `rg` 共同核对调用边；图对 `Start`/`observe` 等通用名称存在跨语言重名噪声，相关结论已回到具体调用点源码验证。

本任务是纯文档分析，未运行 Cargo。结构验收以任务指定的 11 个固定二级标题检查为准；人工复核重点是定位、公式、降级行为、线程退出、Go 差异和扩展风险是否都能反向对应到上述源码与测试。

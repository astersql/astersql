# `pkg/util/cgmon/cgmon.rs`

## 文件定位

本文件是 `astersql-util-cgmon` crate 的核心实现，负责把主机与 Linux cgroup 的 CPU、内存资源信息折算成进程可用上限，并维护对应的 Prometheus Gauge。crate 入口 `pkg/util/cgmon/lib.rs` 将本模块的公开项全部重新导出；`pkg/util/cgmon/Cargo.toml` 则声明它依赖相邻的 `astersql-util-cgroup`、`prometheus`、`sysinfo`、`anyhow` 和 `log`。

在完整服务生命周期中，`cmd/tidb-server/main.rs::runServer` 于全局配置和 CPU affinity 设置完成后调用 `cgmon::StartCgroupMonitor`，`cmd/tidb-server/main.rs::cleanup` 在 Domain、存储等组件关闭后调用 `cgmon::StopCgroupMonitor`。因此它是服务级资源观测后台任务，不参与 SQL 请求执行本身。

## 核心职责

- `CgroupMonitor` 封装四类可注入探针、两项 Gauge、上次发布值以及后台 worker 生命周期，既支持进程级默认实例，也保留独立测试缝。
- `refresh_cgroup_cpu` 以主机逻辑 CPU 数为上限；仅当 cgroup period 和 quota 都为正且计算结果更小时，采用 `ceil(quota / period)`。
- `refresh_cgroup_memory` 以主机总内存为默认值，在 cgroup 内存上限更小时采用 cgroup 值。
- cgroup 探针失败时仍先发布主机默认值，随后把探针错误返回给调用方；主机 CPU/内存探针失败则直接返回，不能形成新的有效值。
- `start`/`stop` 管理单一后台线程，首次启动后立即刷新，之后默认每 10 秒刷新，停止时发送取消信号并等待线程退出。

## 主要符号

- `REFRESH_INTERVAL: Duration`：全局监控器的 10 秒刷新周期。
- `CPUCountProbe`、`CPUQuotaProbe`、`MemoryProbe`：带 `Send + Sync` 的动态闭包类型，分别返回主机核数、cgroup period/quota 和内存字节数。
- `Worker { cancel, handle }`：保存取消通道发送端和后台线程 `JoinHandle`。
- `MonitorState { worker }`：用 `Option<Worker>` 表示停止或运行状态。
- `MonitorInner`：共享不可变配置和探针，并通过三个 `Mutex` 保护上次 CPU 值、上次内存值和 worker 状态；两项 Gauge 分别名为 `tidb_server_maxprocs` 与 `tidb_server_memory_quota_bytes`。
- `CgroupMonitor { inner: Arc<MonitorInner> }`：公开、可克隆的监控器句柄。克隆只增加同一内部状态的强引用，不创建新 worker 或新指标。
- `CgroupMonitor::with_probes`：构造自定义监控器，创建 Gauge，并把探针转为 `Arc<dyn Fn...>`。这里调用的是 `Gauge::with_opts`；本文件没有向 Prometheus registry 注册 Gauge 的调用。
- `start`、`stop`：实例级幂等启停接口，分别以 `bool` 表示本次是否改变了运行状态。
- `refresh_cgroup_cpu`、`refresh_cgroup_memory`：可直接调用的单次刷新接口；返回 cgroup/主机探针错误，同时按可用信息更新缓存和 Gauge。
- `last_cpu`、`last_memory_limit`、`max_procs_metric`、`memory_limit_metric`：读取最近生效值和 Gauge 读数，主要为观测与测试提供入口。
- `lock`：统一处理 `Mutex` 加锁；锁中毒时取回内部值，使非关键监控逻辑可继续运行。
- `refresh_cgroup_loop`、`refresh_once`：后台循环及一次 CPU/内存联合刷新；前者处理首次刷新、超时和取消，后者负责日志级别及 `Weak` 升级。
- `system_memory_total`：通过 `sysinfo::System` 刷新并读取主机总内存。
- `GLOBAL_MONITOR`：按需初始化的进程级监控器，连接真实 cgroup 探针与主机探针。
- `StartCgroupMonitor`、`StopCgroupMonitor`：公开兼容入口，只在 `cfg!(target_os = "linux")` 为真时操作全局实例。

## 执行流程

1. 服务调用 `StartCgroupMonitor`；Linux 上首次访问会初始化 `GLOBAL_MONITOR`，并进入 `CgroupMonitor::start`。
2. `start` 持有状态锁检查 `worker`。已有 worker 时返回 `false`；否则建立 MPSC 取消通道，以 `Weak<MonitorInner>` 启动名为 `cgroup-monitor` 的线程，并保存 `Worker` 后返回 `true`。
3. 线程在 `catch_unwind` 边界内执行 `refresh_cgroup_loop`。循环先调用一次 `refresh_once(initial = true)`，所以启动不必等待第一个 10 秒周期。
4. `refresh_once` 先升级 `Weak`；内部状态已释放时返回 `false` 令循环退出。升级成功后依次刷新 CPU 和内存。首次探针错误记 `warn`，周期性错误记 `debug`，一项失败不会阻止另一项刷新。
5. CPU 刷新先取得至少为 1 的主机逻辑核数并转换为 `i32`。有效的正 period/quota 只有在比主机容量小时才用向上取整结果；结果变化时才更新 Gauge 和 `last_cpu`。即使 cgroup 探针失败，主机默认值仍会更新，最后再返回原错误。
6. 内存刷新先取得主机总内存；成功后再探测 cgroup 上限并取较小值。结果变化时才更新 Gauge 和 `last_memory_limit`。cgroup 探针失败时发布主机值后返回原错误；主机探针失败时立即返回，保持旧值。
7. 循环通过 `recv_timeout(refresh_interval)` 等待。收到取消、发送端断开，或超时刷新时 `Weak` 无法升级，都会退出；普通超时则继续下一轮。
8. 服务清理调用 `StopCgroupMonitor`。`stop` 从状态中取出 worker，发送取消信号并 `join`；无 worker 时返回 `false`。

## 数据与状态

`MonitorInner` 是所有 `CgroupMonitor` 克隆共享的唯一状态中心。探针、刷新周期和 Gauge 构造后不再替换；`last_cpu`、`last_memory_limit` 只在计算值发生变化时写入，以避免重复日志和 Gauge 写入。初始缓存均为 0，因此第一次成功或采用主机回退值时通常会发布。

CPU 的关键不变量是最终值至少为 1 且不超过可转换为 `i32` 的主机逻辑核数；无效或无限制的 cgroup 值（period/quota 非正）不收紧主机值。内存值是不大于主机总内存的 `u64`；转为 Gauge 的 `f64` 时可能对极大整数失去逐字节精度，这是 Prometheus 浮点指标表示的固有限制。

`GLOBAL_MONITOR` 使用 `LazyLock` 延迟构造。非 Linux 的公开 Start/Stop 不访问它，因而不会启动 worker。文件没有持久化状态，也不会修改运行时线程数或内存限制；`maxprocs` 是观测值，不是 Rust 等价的 `GOMAXPROCS` 设置器。

## 依赖与调用关系

上游生产调用由直接搜索确认：`cmd/tidb-server/main.rs::runServer` 调用 `StartCgroupMonitor`，`cmd/tidb-server/main.rs::cleanup` 调用 `StopCgroupMonitor`。`pkg/util/cgmon/lib.rs` 通过 `pub use cgmon::*` 暴露这些入口，并把 `cgroup_dependency` 映射为实现使用的 `crate::util::cgroup`。

内部调用主链为 `StartCgroupMonitor -> GLOBAL_MONITOR.start -> refresh_cgroup_loop -> refresh_once -> refresh_cgroup_cpu / refresh_cgroup_memory`；停止链为 `StopCgroupMonitor -> GLOBAL_MONITOR.stop -> Sender::send + JoinHandle::join`。

下游依赖包括：`std::thread::available_parallelism` 提供主机 CPU 数；`cgroup::GetCPUPeriodAndQuota` 与 `cgroup::GetMemoryLimit` 提供真实 cgroup 数据；`sysinfo::System` 提供主机总内存；`prometheus::{Gauge, Opts}` 保存指标读数；`log` 记录生命周期、值变化和探针错误；`anyhow` 统一探针错误，并报告 CPU 数无法转换为 `i32` 的边界。

RustCodeGraph 已索引 `cgmon.rs` 的 20 个符号，并能定位上述函数及服务文件节点；其 `callers/callees` 精确查询在本次环境中超时，所以跨文件调用点另外由 `rg` 与 `cmd/tidb-server/main.rs` 源码节点核验，不能把图查询超时解释为无调用关系。

## 错误处理与边界

- `with_probes` 创建 Gauge 失败会 `expect` 并 panic；指标名和帮助文本是静态常量，正常情况下属于编程期不变量。
- `start` 创建线程失败会 `expect` 并 panic。worker 内部 panic 被 `catch_unwind` 捕获并记录，避免沿线程边界传播，但线程会就此结束。
- `lock` 对 poisoned mutex 选择恢复内部值而非继续 panic，符合监控辅助组件尽量不中断主进程的定位。
- 主机 CPU 探针失败，或逻辑核数超出 `i32`，CPU 刷新直接失败且不更新值；`max(1)` 防止成功探针给出零核。
- cgroup CPU 探针错误或非正限制都会保留主机 CPU 值；只有探针错误会作为 `Err` 返回，非正限制本身在探针成功时是正常的“不限额”结果。
- 主机内存探针失败时不调用 cgroup 内存探针，也不更新旧值；cgroup 内存探针失败时则仍发布主机总内存，然后返回错误。
- `start`/`stop` 的公开全局包装忽略实例方法的布尔返回值，重复调用对外表现为无操作。尽管注释沿用 Go 的“调用方串行化”警告，Rust 实例状态实际由互斥锁保护。
- `stop` 忽略取消发送错误和 worker 的 join 结果；若 worker 已 panic，panic 已在线程入口捕获，若发送端对应接收者已退出也仍可完成状态清理。

## 并发与资源生命周期

同一监控器的所有克隆共享 `Arc<MonitorInner>`。`state` 锁确保并发 `start` 最多安装一个 worker，并发 `stop` 最多取走一个 worker；CPU 与内存缓存各有独立锁，允许对应读接口安全访问。单一 worker 顺序刷新 CPU 和内存，不会在文件内部并行调用探针。

worker 只持有 `Weak<MonitorInner>`，不会反向延长监控器生命周期。每次刷新临时升级为强引用；若所有外部 `CgroupMonitor` 已释放，升级失败后线程退出。取消使用零容量语义无要求的标准 MPSC 通道：停止信号会唤醒 `recv_timeout`，`stop` 随后 join，保证返回时该 worker 已结束。若整个内部状态被丢弃而未显式停止，`Worker.cancel` 的发送端随之丢弃，接收端得到 `Disconnected` 后退出。

`stop` 在从 `state` 中取走 worker后即释放状态锁，再发送和 join，因此 worker 退出不会等待同一状态锁。不过，在旧线程退出完成前，另一个线程可以再次 `start` 并安装新 worker；调用方若要求严格的“旧 worker 完全停止后才重新启动”顺序，仍应遵守公开接口注释并串行组织 Start/Stop 生命周期。

## 与 Go 版本的对应关系

Rust 版本对应 `pkg/util/cgmon/cgmon.go`：默认周期同为 10 秒；CPU 都以主机核数为默认值，并对较小的正 cgroup 比率向上取整；内存都取主机总量与有效 cgroup 上限的较小者；首次错误用 warn、后续错误用 debug；启动时立即刷新，停止时等待后台执行单元结束。

Go 通过包级 `started`、context/cancel、WaitGroup、`lastCPU` 和 `lastMemoryLimit` 保存状态，并以可替换包变量注入两个 cgroup 探针。Rust 将这些状态收拢进 `CgroupMonitor`，用 Mutex、MPSC、JoinHandle 和四个构造时探针实现更完整的测试隔离；Go 注释明确 Start/Stop 非线程安全，Rust 虽保留调用方串行化警告，但内部已避免数据竞争。

指标实现也有结构差异：Go 直接写 `pkg/metrics` 的全局 `MaxProcs` 和 `MemoryLimit`，Rust 在每个监控器内创建同名语义的 Gauge。当前文件未调用 registry 注册，因此“更新 Gauge 对象”有源码证据，“已由默认 registry 对外抓取”没有本文件证据，扩展或接线时需单独核验。

`pkg/util/cgmon/cgmon_test.go::TestUploadDefaultValueWithoutCgroup` 验证 cgroup 探针报错后仍保存主机默认值。Rust 的 `pkg/util/cgmon/cgmon_test.rs` 保留该意图；`migration_aster_unit_test.rs` 进一步验证错误返回、Gauge 值、CPU ceil、无/超额限制、内存较小值以及启动立即刷新和幂等启停。

## 扩展指南

- 新增资源探针时，优先沿用 `with_probes` 的依赖注入方式，把不可变探针放入 `MonitorInner`，在 `refresh_once` 中明确单项失败是否应阻止其他资源刷新。
- 修改 CPU 或内存折算规则时，应同步扩展 `pkg/util/cgmon/migration_aster_unit_test.rs` 的边界表，并保留 `pkg/util/cgmon/cgmon_test.rs` 所代表的 Go 回退语义；测试逻辑继续放在独立测试文件，不嵌入 `cgmon.rs`。
- 修改全局启停或服务生命周期接线时，同时检查 `cmd/tidb-server/main.rs` 的启动/清理顺序，以及 `cmd/tidb-server/parity_test.rs` 对关闭事件的约束。
- 若目标是让指标被 Prometheus 默认采集，需要先确认全仓库 registry 约定，再决定复用全局指标还是注册本地 Gauge；应避免重复注册同名 descriptor，并为重复构造/注册增加独立测试。
- 缩短刷新周期或增加耗时探针时，应评估单线程串行刷新延迟、日志频率和 `stop` 等待时间。当前 `stop` 没有超时，阻塞探针会延迟服务清理。
- 若强化 start/stop 的严格线性化，需要处理“stop 取走旧 worker并 join 时允许新 start”的窗口，并增加并发回归测试；不要仅依靠现有 bool 幂等断言。
- 保持 Go/Rust 语义对齐时，重点比较错误发生后是否仍发布默认值、首次与周期日志级别、无 cgroup 限制的判定以及停止等待保证。

## 验证依据

- 实现与符号：`pkg/util/cgmon/cgmon.rs`；RustCodeGraph 文件节点覆盖 312 行并识别 20 个符号，精确查询确认 `CgroupMonitor`、`with_probes`、`refresh_cgroup_cpu`、`refresh_cgroup_memory`、`refresh_cgroup_loop`、`refresh_once`、`StartCgroupMonitor`、`StopCgroupMonitor`。
- crate 边界：`pkg/util/cgmon/lib.rs` 与 `pkg/util/cgmon/Cargo.toml`；确认公开重导出、测试模块挂载、依赖和 `go-package = "pkg/util/cgmon"` 移植元数据。
- 生产调用：`cmd/tidb-server/main.rs::runServer` 第 762 行附近和 `cleanup` 第 2090 行附近；另以 `rg` 核对 Rust 调用点。
- Go 对照：`pkg/util/cgmon/cgmon.go` 与 `pkg/util/cgmon/cgmon_test.go`。
- Rust 测试：`pkg/util/cgmon/cgmon_test.rs` 与 `pkg/util/cgmon/migration_aster_unit_test.rs`。这些是独立测试文件，本任务未运行 Cargo，符合纯文档任务约束。
- RustCodeGraph 状态：索引包含 11,467 个文件，目标目录的 Go/Rust 实现与测试均在索引中；`callers/callees` 查询曾超时，相关调用边已由源码节点和文本搜索交叉验证。
- 人工复核范围：已核对文件存在理由、CPU/内存刷新算法、错误后的回退行为、worker 生命周期、Go 差异和安全扩展位置；未声称本文件已完成 Prometheus registry 接线。

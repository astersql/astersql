# `br/pkg/utils/memory_monitor.rs`

## 文件定位

本文件属于 `astersql-br-pkg-utils` crate；crate 入口 `br/pkg/utils/lib.rs` 通过 `#[path = "memory_monitor.rs"] pub mod memory_monitor` 挂载它，但没有把 `RunMemoryMonitor` 再导出到 crate 根。其职责是把 BR 启动阶段得到的进程内存上限和 profile 输出目录适配到通用的 `astersql-util-memoryusagealarm` 告警框架。

应用侧直接入口位于 `br/cmd/br/cmd.rs`：该文件在计算 BR 可用内存、保证至少 256 MiB、排除无法表示为 `i64` 的上限并调用 `debug_runtime::SetMemoryLimit` 后，以 `utils::RunMemoryMonitor(ctx, &dumpDir, memlimit)` 启动监控。`br/pkg/utils/Cargo.toml` 明确声明了本文件直接使用的 `astersql-util-memory`、`astersql-util-memoryusagealarm`、`astersql-br-pkg-logutil`、`astersql-errors` 和 `crossbeam-channel` 依赖。

## 核心职责

1. `RunMemoryMonitor` 规范化 profile dump 目录；调用者传入空字符串时回退到 `DefaultProfilesDir`（`/tmp/profiles`）。
2. 当 `memory_limit > 0` 时，把字节数写入进程级原子变量 `ServerMemoryLimit`，使通用告警逻辑以该上限而非系统总内存为基准。
3. 用固定阈值 `0.8`、固定保留数 `3` 和实际 dump 目录构造 `BRConfigProvider`，向通用告警组件提供 BR 配置。
4. 记录启动参数，然后由 `spawn_memory_alarm` 启动告警循环；收到 `Context` 取消后通知告警句柄退出并等待其线程结束。

本文件只负责配置和生命周期接线。内存采样、阈值判断、profile 目录初始化、记录淘汰和实际 dump 均在 `pkg/util/memoryusagealarm/memoryusagealarm.rs` 的 `Handle` / `memoryUsageAlarm` 中实现。

## 主要符号

- `DefaultProfilesDir: &str`：公开默认 profile 根目录，值为 `/tmp/profiles`；入口和 `BRConfigProvider::GetLogDir` 都使用它进行空值回退。
- `defaultMemoryUsageAlarmRatio: f64`：私有默认触发比例 `0.8`。
- `defaultMemoryUsageAlarmKeepRecordNum: i64`：私有默认历史记录数 `3`。
- `BRConfigProvider`：公开配置提供者，字段本身私有。`ratio: AtomicU64` 保存 `f64::to_bits()`，`keepNum: AtomicI64` 保存记录数，`logDir: String` 保存输出目录。
- `BRConfigProvider::new(ratio, keep_num, log_dir)`：公开构造器；浮点数按 IEEE-754 位模式无损写入原子整数。
- `BRConfigProvider::set_log_dir`：需要 `&mut self` 的公开目录更新方法；主要由测试验证空目录回退，当前生产流程构造后不再调用。
- `BRConfigProvider::ratio_f64`：私有读取辅助，以 `Relaxed` 顺序加载位模式并还原为 `f64`。
- `impl ConfigProvider for BRConfigProvider`：实现 `GetMemoryUsageAlarmRatio`、`GetMemoryUsageAlarmKeepRecordNum`、`GetLogDir`、`GetComponentName` 四个配置读取接口；组件名固定为 `br`。
- `RunMemoryMonitor(ctx, dump_dir, memory_limit) -> Result<(), SharedError>`：公开启动入口。当前同步路径没有可失败操作，成功构造后台任务后总是返回 `Ok(())`。
- `spawn_memory_alarm(ctx, provider) -> JoinHandle<()>`：crate 内可见的线程启动辅助；返回 watcher 线程句柄供同 crate 独立测试等待收尾，生产入口有意丢弃该句柄。
- `MemoryUsageAlarmHandle`：对 `astersql_util_memoryusagealarm::Handle` 的公开别名；本仓库搜索未发现本 crate 外的 Rust 使用点。

## 执行流程

`RunMemoryMonitor` 的顺序如下：

1. 把 `dump_dir` 转为 `String`；为空则替换为 `/tmp/profiles`。
2. 仅在 `memory_limit > 0` 时执行 `ServerMemoryLimit.Store(memory_limit)`。传入零不会清空此前的全局值。
3. 取得 `std::env::temp_dir()`，以固定比例、固定保留数和目录构造 `Arc<BRConfigProvider>`。
4. 记录 `dump_dir`、目录是否等于系统临时目录、告警比例和按整数除法换算的 MiB 上限。这个日志仅描述配置，不代表告警线程已经完成首次采样。
5. 把上下文和配置传给 `spawn_memory_alarm`，随后立即返回 `Ok(())`。

`spawn_memory_alarm` 再执行以下生命周期：

1. 建立容量为 1 的 `crossbeam_channel`，把接收端和 provider 交给 `NewMemoryUsageAlarmHandle`。
2. 启动 watcher 线程；watcher 内再启动 alarm 线程执行 `Handle::Run()`。
3. watcher 通过 `Context::wait_cancelled_timeout(60s)` 循环等待取消。`Context::cancel` 会通知条件变量，因此正常取消无需等待完整 60 秒；60 秒超时只是周期性复查兜底。
4. 取消后发送一个退出值，并 `join` alarm 线程。底层 `Handle::Run` 每 100ms 使用 `recv_timeout`：收到值或发现通道断开时返回，超时时才执行一次告警检查。

底层 `memoryUsageAlarm::updateVariable` 至少间隔 60 秒读取一次 provider 和 `ServerMemoryLimit`；若全局上限为零，才回退到系统总内存。这解释了本文件为什么必须先写全局上限再启动句柄。

## 数据与状态

`BRConfigProvider` 的比例和保留数通过原子读取共享给告警线程。这里使用 `Ordering::Relaxed`，因为两者是独立配置值，不与其他内存状态组成发布/获取不变量；比例通过 `to_bits` / `from_bits` 保留精确位模式。`logDir` 是普通 `String`：构造完成并放入 `Arc<dyn ConfigProvider>` 后只读；若要更新，必须在共享前持有唯一的 `&mut BRConfigProvider`。

`ServerMemoryLimit` 是 `pkg/util/memory/tracker.rs` 定义的进程级 `atomicutil::Uint64`。它不归 monitor 实例所有，多次调用 `RunMemoryMonitor` 会共享并可能覆盖这一全局值；零值调用不会复位它。

每次 `spawn_memory_alarm` 都独立拥有一个有界退出通道、一个 watcher 线程和一个 alarm 线程。provider 由 `Arc` 同时被句柄及告警状态持有；线程退出后引用随句柄和状态析构释放。实际告警状态（上次检查时间、目录列表、上次内存用量、缓存阈值等）位于下游 `memoryUsageAlarm`，不存放在本文件。

## 依赖与调用关系

上游调用边由 RustCodeGraph 和源码共同确认：`br/cmd/br/cmd.rs` 的 BR 内存初始化流程调用 `utils::RunMemoryMonitor`。索引对本文件给出的内部调用边是 `RunMemoryMonitor -> spawn_memory_alarm`；独立测试 `run_memory_monitor_starts_alarm_and_stops_on_cancel` 也直接调用后者。仓库 Rust 文本搜索未发现其他 `RunMemoryMonitor` 生产调用点。

主要下游关系如下：

- `crate::stubs::context::Context`：提供可克隆取消令牌和带超时的条件变量等待。
- `astersql_util_memory::tracker::ServerMemoryLimit`：保存通用告警器读取的进程内存上限。
- `astersql_util_memoryusagealarm::{ConfigProvider, NewMemoryUsageAlarmHandle}`：定义配置协议并创建实际周期检查句柄。
- `crossbeam_channel::bounded(1)`：连接 watcher 与 alarm 线程的单次退出信号。
- `astersql_br_pkg_logutil::{log, Field}`：输出启动配置日志。
- `astersql_errors::SharedError`：保持 BR 公共入口的错误返回类型，尽管当前实现不生成错误。

`br/pkg/utils/lib.rs` 只公开 `memory_monitor` 模块，没有在根级 `pub use` 这些符号；调用者必须经过模块路径。`MemoryUsageAlarmHandle` 别名只是暴露下游句柄类型，本文件自身构造时直接使用 `NewMemoryUsageAlarmHandle`。

## 错误处理与边界

- 空 `dump_dir` 在入口和 provider getter 两层回退；构造时已规范化，getter 的回退还保护手工构造或 `set_log_dir("")` 的情况。
- `memory_limit == 0` 表示“不覆盖全局上限”，不是“清零”。扩展调用场景时必须考虑已有全局状态。
- `RunMemoryMonitor` 当前没有把线程创建、告警初始化或后台运行错误同步给调用者，因此其 `SharedError` 返回主要是接口兼容预留。`thread::spawn` 的系统级失败会 panic，而不是返回该错误。
- watcher 对 `exit_tx.send(())` 和 `alarm_thread.join()` 的结果都显式忽略：接收端已关闭、alarm 线程 panic 等情况不会反馈到生产调用者。
- `Context::wait_cancelled_timeout` 内部锁中毒会 panic；告警句柄的会话管理器锁中毒也会 panic。这些都不经过 `RunMemoryMonitor` 的 `Result`。
- 目录是否为临时目录使用路径值严格比较；默认 `/tmp/profiles` 通常并不等于 `std::env::temp_dir()` 本身，因此 `using_temp_dir` 描述“目录恰好是临时目录”，不是“位于临时目录树下”。
- `memory_limit_mb` 使用整除并转换为 `i64`。正常上游已排除 `>= i64::MAX` 的值，但该公开函数本身不执行这一保护，其他调用者传入极大 `u64` 时日志字段可能发生截断式转换；全局保存仍是原始 `u64`。
- 多次启动会产生多组后台线程，本文件没有幂等保护或集中关闭句柄。

## 并发与资源生命周期

同步入口是 fire-and-forget：生产调用者拿不到 `JoinHandle`，其所有权立即转入后台。watcher 持有 `Context`、发送端和 alarm 线程句柄；alarm 线程持有告警 `Handle`，后者持有接收端与 provider。取消顺序是“上下文置位并唤醒 watcher → watcher 发送退出值 → alarm 在最多约一个 100ms 检查周期内观察通道 → watcher join alarm → watcher 返回”。

配置读使用原子量，`Arc<dyn ConfigProvider>` 要求下游 trait 满足跨线程共享约束。`logDir` 没有锁仍是安全的，因为共享后的 trait 只暴露克隆读取，修改方法要求独占可变引用。退出通道容量为 1 且只发送一次；发送失败只意味着 alarm 已经退出或接收端已释放。

当前 watcher 在取消前一直存活；若调用方永不取消 `Context`，两条线程与其 provider 将持续到进程退出。若 alarm 线程提前 panic，watcher仍要等到上下文取消才会尝试 join，且 join 错误被忽略。

## 与 Go 版本的对应关系

Rust 文件明确移植自 `br/pkg/utils/memory_monitor.go`，常量、provider 四个 getter、空目录回退、仅正数写入全局上限、启动日志和“上下文取消后关闭退出通道”的主语义一致。Go 使用 `atomic.Float64` / `atomic.Int64` 指针；Rust 用内嵌的 `AtomicU64` 位模式和 `AtomicI64`，并由独立测试验证非简单小数的位精度。

Go 在构造 handle 后显式调用 `handle.SetSessionManager(nil)`，表示 BR 不提供 SQL 会话快照。Rust 的 `Handle` 构造器已经把内部 `Option<Arc<dyn SessionManager>>` 初始化为 `None`，达到同样效果；Rust 的 `SetSessionManager` 接口只接受实际 `Arc`，因此无需也不能传入空值。

Go 启动一个外层 goroutine，并在其中再启动 `handle.Run` goroutine；Rust 对应为 watcher 线程加 alarm 线程。Rust 额外保留并 join alarm 线程，使测试能够证明取消后的资源收尾；Go 外层 goroutine 关闭通道后直接返回。Rust 以 60 秒超时等待可取消条件变量，Go 直接阻塞在 `ctx.Done()`；由于 Rust `cancel()` 会唤醒条件变量，正常取消语义相同。

`br/pkg/utils/memory_monitor_test.go` 只验证 provider 的四个 getter和目录回退。`br/pkg/utils/memory_monitor_test.rs` 保留这些断言，并额外验证 ratio 位精度以及告警循环确实轮询配置、取消后 watcher 可成功 join。

## 扩展指南

- 新增动态阈值或记录数更新时，应在 `BRConfigProvider` 增加明确的原子 setter，并保持与 `ConfigProvider` 的 60 秒刷新语义一致；测试放在独立文件 `br/pkg/utils/memory_monitor_test.rs`，不要嵌入生产源文件。
- 改变默认比例、保留数或目录时，同时核对 `br/pkg/utils/memory_monitor.go`、Go/Rust 两份独立测试以及下游 `memoryUsageAlarm::updateVariable` 的合法值处理，避免两种实现漂移。
- 若要传播后台失败，需重新设计 `RunMemoryMonitor` 的生命周期返回值或错误通道；仅保留现有 `Result` 签名不能捕获线程 panic。还要同步调整 `br/cmd/br/cmd.rs` 的启动错误处理。
- 若允许多次启动，应增加实例所有权、幂等或集中关闭策略，并明确多个实例写同一 `ServerMemoryLimit` 的覆盖规则。
- 若要提供 session manager，不应在本文件复制告警逻辑；应扩展构造接线，在 alarm 线程启动前调用下游 `Handle::SetSessionManager`。
- 优化取消延迟或线程数量时，必须维持“先通知句柄、再等待其退出”的顺序，并扩展 `run_memory_monitor_starts_alarm_and_stops_on_cancel` 覆盖提前退出、panic 或重复取消等边界。
- 性能风险主要在告警器 100ms 轮询及 profile 生成，下游实现是评估入口；本文件的原子 getter 和每次目录字符串克隆通常不是主成本，但高频读取路径不应引入互斥锁或 I/O。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，其中 Rust 文件 7,032 个。
- RustCodeGraph `explore`：确认 `br/pkg/utils/memory_monitor.rs` 的 `RunMemoryMonitor -> spawn_memory_alarm` 调用边；blast radius 显示 `spawn_memory_alarm` 同时由生产入口和 Rust 取消测试调用。
- RustCodeGraph `query`：定位 `BRConfigProvider`、Rust/Go 两份 `RunMemoryMonitor`、`spawn_memory_alarm`、`NewMemoryUsageAlarmHandle` 和 `ServerMemoryLimit` 的真实定义。
- RustCodeGraph `node --file`：读取 `br/cmd/br/cmd.rs` 的调用点、`br/pkg/utils/stubs.rs` 的 `Context` 等待实现、`pkg/util/memoryusagealarm/memoryusagealarm.rs` 的句柄循环与配置刷新、`pkg/util/memory/tracker.rs` 的全局上限定义。
- 已阅读源码与配置：`br/pkg/utils/memory_monitor.rs`、`br/pkg/utils/lib.rs`、`br/pkg/utils/Cargo.toml`。
- 已阅读语义对照和独立测试：`br/pkg/utils/memory_monitor.go`、`br/pkg/utils/memory_monitor_test.go`、`br/pkg/utils/memory_monitor_test.rs`。
- 结构验证使用任务指定命令，要求目标文件存在且恰有上述 11 个固定二级标题。本任务只新增文档，按计划不运行 Cargo。

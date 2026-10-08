# `pkg/resourcemanager/scheduler/cpu_scheduler.rs`

## 文件定位

本文件实现 `astersql-resourcemanager-scheduler` crate 的 CPU 决策器。crate 入口
[`lib.rs`](lib.rs) 将它声明为 `cpu_scheduler` 模块并公开再导出其符号；
[`Cargo.toml`](Cargo.toml) 则把该 crate 对应到 Go 包
`pkg/resourcemanager/scheduler`，并以路径依赖 `astersql-util-cpu` 取得 CPU 采样值。

它处在“观测”与“执行”之间：上游资源管理器在
[`rm.rs`](../rm.rs) 构造默认调度器并周期调用它；本文件只把最近 CPU 快照映射为
`Command`，实际调整池容量的是 [`schedule.rs`](../schedule.rs) 中的
`ResourceManager::Exec`。因此这里不是采样线程、池管理器或调容执行器。

## 核心职责

- `CPUScheduler::Tune` 先用池的最近调容时间执行 200ms 防抖，再读取
  `cpu::GetCPUUsage()` 的全局快照。
- `command_for_cpu_usage` 实现纯决策规则：不支持采样时保持；CPU 使用率严格低于
  `0.5` 时升容，严格高于 `0.7` 时降容，闭区间 `[0.5, 0.7]` 内保持。
- `impl Scheduler for CPUScheduler` 让资源管理器可以通过 `dyn Scheduler` 统一调用该
  策略；自由函数 `Tune` 则保留机械迁移调用方需要的 Go 风格入口。

本文件只返回意图，不调用 `GoroutinePool::Tune`，也不检查池容量、运行 worker 数或
最大升容量；这些约束位于 `ResourceManager::schedulePool` 和 `ResourceManager::Exec`。

## 主要符号

- `pub struct CPUScheduler;`：无字段的零大小策略类型；全部输入来自本次调用的池和
  全局 CPU 快照，没有实例级配置。
- `pub fn NewCPUScheduler() -> CPUScheduler`：构造零大小实例。Rust 返回值而非堆指针，
  调用者可按需装入 `Box<dyn Scheduler + Send + Sync>`。
- `pub fn command_for_cpu_usage(value: f64, unsupported: bool) -> Command`：可独立测试的
  纯映射函数。`unsupported` 优先级最高；阈值比较都为严格比较。
- `pub fn CPUScheduler::Tune(&self, _component: util::Component,
  pool: &dyn util::GoroutinePool) -> Command`：实际策略入口。`component` 为兼容
  `Scheduler` 接口而保留，当前实现不参与决策。
- `impl Scheduler for CPUScheduler`：trait 方法显式委托到固有方法，避免复制策略。
- `pub fn Tune(scheduler, component, pool) -> Command`：兼容包装函数，同样委托给固有
  方法；它不增加状态或分支。

文件没有模块常量、枚举、条件编译项或内部可变静态量。命令枚举与 trait 定义在
[`scheduler.rs`](scheduler.rs)，最小调度间隔及池接口定义在
[`util.rs`](../util/util.rs)。

## 执行流程

完整运行链如下：

1. `ResourceManager::NewResourceManger` 把 `NewCPUScheduler()` 装入调度器列表；
   `ResourceManager::Start` 同时启动 CPU observer 和每 100ms 一次的调度循环。
2. `ResourceManager::schedule` 遍历池；`DistTask` 在进入调度器前被跳过，无运行中
   worker 的池也会在 `schedulePool` 中直接返回 `Hold`。
3. `schedulePool` 通过 `dyn Scheduler::Tune` 调用本文件的 trait 实现，后者委托给
   `CPUScheduler::Tune`。
4. `CPUScheduler::Tune` 计算 `pool.LastTunerTs().elapsed()`；发生系统时钟倒退时，
   `unwrap_or_default()` 把间隔视为零。若间隔严格小于
   `MinSchedulerInterval.Load()`，立即返回 `Hold`，不会读取 CPU 快照。
5. 间隔满足时调用 `cpu::GetCPUUsage()`，再交给 `command_for_cpu_usage`。采样不支持、
   中间负载或不可比较的 `NaN` 都落到 `Hold`；低负载返回 `Overclock`，高负载返回
   `Downclock`。
6. 返回的非 `Hold` 命令由 `schedulePool` 做降容安全检查，再由 `Exec` 再次校验间隔，
   最终把池容量增减 1；升容还受 `MaxOverclockCount` 限制。

## 数据与状态

`CPUScheduler` 自身无状态。一次决策只读取三项数据：调用参数中的
`Component`（当前忽略）、`GoroutinePool::LastTunerTs()`，以及 CPU crate 维护的
`(f64, bool)` 全局快照。`MinSchedulerInterval` 是 `AtomicDuration`，当前默认 200ms，
每次调用通过顺序一致性原子读取得到，因此运行时更新会影响后续决策。

`Command` 是值类型枚举，分别表示减并发、保持和增并发。这里不记录上次命令，不更新
池的时间戳，也不拥有 observer；上次调容时间应由具体池在真正调容时维护。

阈值的边界不变量是 `0.5 → Hold`、`0.7 → Hold`。负数会按“小于 0.5”返回
`Overclock`，正无穷会返回 `Downclock`，`NaN` 因两次比较均为假而返回 `Hold`；本文件
不校验输入范围，正常范围由 CPU observer 保证。

## 依赖与调用关系

上游调用关系：

- [`rm.rs`](../rm.rs) 的 `ResourceManager::NewResourceManger` 创建默认
  `Box<dyn Scheduler + Send + Sync>`；这是生产主链的装配入口。
- [`schedule.rs`](../schedule.rs) 的 `ResourceManager::schedulePool` 调用
  `scheduler.Tune(pool.Component, pool.Pool.as_ref())`，并消费返回命令。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 直接调用构造函数、固有
  `Tune`、纯映射函数和 trait 分派；
  [`tests/cpu_failpoint_integration.rs`](tests/cpu_failpoint_integration.rs) 验证跨 crate
  unsupported 传播。

下游依赖关系：

- `super::scheduler::{Command, Scheduler}` 提供输出协议和统一调度接口。
- `crate::util::{Component, GoroutinePool, MinSchedulerInterval}` 提供兼容参数、时间来源
  和防抖配置。
- `crate::cpu::GetCPUUsage` 由 [`lib.rs`](lib.rs) 从依赖 crate
  `astersql-util-cpu` 再导出；真实快照和 observer 位于
  [`pkg/util/cpu/cpu.rs`](../../util/cpu/cpu.rs)。

RustCodeGraph 将本文件识别为 7 个符号，并报告文件级直接使用者为
`migration_aster_unit_test.rs`；对关键符号执行精确 `callers`/`callees` 查询未返回边，
因此上述跨文件主链以模块源码中的构造、trait 调用和文本引用为直接证据，而不是把空图
误写成“没有调用者”。

## 错误处理与边界

公开函数均不返回 `Result`，策略用保守的 `Hold` 表示无法安全决策的情形：CPU 采样
不支持、调容间隔未到、系统时间早于记录的时间戳，以及处于阈值中间区间。CPU observer
的 cgroup 探测失败会把 `unsupported` 置真，本文件不会把缺失数据当作零负载扩容。

本文件不捕获 `GoroutinePool::LastTunerTs` 内部实现可能产生的 panic，也不验证 CPU 值
是否有限或位于预期范围。`SystemTime::elapsed` 自身的错误被降级为零时长。值得注意的是，
本文件提前返回条件为 `< MinSchedulerInterval`，而执行器的二次门禁为
`<= MinSchedulerInterval`；恰好等于阈值时策略可产生命令，但 `Exec` 仍会拒绝执行。

## 并发与资源生命周期

`CPUScheduler` 没有锁、线程、通道和析构逻辑；零大小实例可安全放入资源管理器要求的
`Box<dyn Scheduler + Send + Sync>`。它只做并发安全读取：最小间隔由原子量承载，CPU
快照由 CPU crate 的原子状态承载，池通过 `GoroutinePool: Send + Sync` 借用传入。

CPU observer 及其采样线程由 `ResourceManager::Start/Stop` 拥有；资源管理器的调度线程
每 100ms 调用本策略。策略不保证 observer 已启动：启动前读取到的初始快照仍按同一规则
处理，因此生产装配必须保持“先启动 observer，再进入周期调度”的顺序。策略返回后不保存
`pool` 引用，实际池调容和时间戳更新属于调用方生命周期。

## 与 Go 版本的对应关系

直接对照 [`cpu_scheduler.go`](cpu_scheduler.go)：Rust 保留 `CPUScheduler`、
`NewCPUScheduler` 和 `Tune` 名称，防抖顺序、unsupported 优先级以及 `0.5/0.7` 严格
阈值与 Go 一致。`Component` 在两边都未使用。Go 构造函数返回 `*CPUScheduler`，Rust
返回零大小值并由资源管理器装箱，这是所有权表达差异，不改变策略语义。

Rust 为可测试性额外抽出了 `command_for_cpu_usage`，并提供自由函数 `Tune` 作为迁移兼容
包装；Go 没有这两个独立入口。Rust 使用 `SystemTime::elapsed().unwrap_or_default()` 模拟
`time.Since` 的防抖判断，在未来时间戳场景下保守保持。命令的数值表示不应被依赖：Go
使用 `iota`，Rust 使用未指定判别值的 enum，双方契约是语义枚举而非整数 ABI。

现有 Rust 独立测试覆盖 Go 语义的主要边界，但 Go 同目录没有
`cpu_scheduler_test.go`；对照依据主要来自生产 Go 文件及 Rust 迁移/集成测试。

## 扩展指南

- 调整 CPU 阈值或 unsupported 策略时，集中修改 `command_for_cpu_usage`，并同步扩展
  `migration_cpu_thresholds_match_go` 的等于、略低、略高和异常浮点输入用例；若 Go 行为
  仍是兼容基准，也必须同步说明或修改 Go 对照，避免静默分叉。
- 引入按 `Component` 区分的策略时，在 `CPUScheduler::Tune` 接入，并检查
  `ResourceManager::schedule` 已在上游跳过 `DistTask` 的事实，避免重复或冲突过滤。
- 增加实例配置或历史状态会使当前零大小、天然 `Send + Sync` 的性质改变；应明确同步
  访问方案，并保持测试逻辑位于独立测试文件，不能嵌入本生产源文件。
- 改变防抖逻辑时，要同时审查 `CPUScheduler::Tune` 和 `ResourceManager::Exec` 的双重
  时间门禁及 `<`/`<=` 差异，并在独立测试中覆盖恰好等于边界的情况。
- 改变采样契约时，应同时验证 `pkg/util/cpu/cpu.rs`、本 crate 的依赖 feature，以及
  `cpu_failpoint_integration.rs`，尤其防止 unsupported 被误判为低 CPU 而扩容。

性能风险主要是把 CPU 读取或策略改成阻塞操作，因为它运行在遍历所有池的单次调度链中；
兼容风险集中在阈值、命令方向和 Go 风格公开符号；正确性风险集中在时间边界、异常采样值
以及与执行器二次门禁不一致。

## 验证依据

- 目标源码：[`cpu_scheduler.rs`](cpu_scheduler.rs)；符号清单为 `CPUScheduler`、
  `NewCPUScheduler`、`command_for_cpu_usage`、两个 `Tune` 入口及 `Scheduler` 实现。
- crate 与模块：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、
  [`scheduler.rs`](scheduler.rs)、[`util.rs`](../util/util.rs)。
- 生产调用链：[`rm.rs`](../rm.rs) 的默认装配和 observer 生命周期，
  [`schedule.rs`](../schedule.rs) 的池遍历、trait 调用、安全过滤和命令执行。
- Go 对照：[`cpu_scheduler.go`](cpu_scheduler.go) 与 [`scheduler.go`](scheduler.go)。
- 独立测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖阈值、防抖与
  trait 分派；[`tests/cpu_failpoint_integration.rs`](tests/cpu_failpoint_integration.rs)
  覆盖 cgroup 探测失败到 `Hold` 的跨 crate 传播。
- CPU 数据来源：[`pkg/util/cpu/cpu.rs`](../../util/cpu/cpu.rs)，用于确认全局快照、
  unsupported 标记和 observer 生命周期。
- RustCodeGraph：`status` 显示索引含本文件；`files --filter
  pkg/resourcemanager/scheduler`、目标文件 `node`、关键符号 `query` 以及对应
  `callers`/`callees` 均已执行。图没有为关键函数返回调用边，跨文件结论改由上述源码
  位置核实。

本任务是纯文档分析，没有运行 Cargo 或代码测试；验收以固定章节结构、链接存在性和上述
源码/图查询事实复核为准。

# `pkg/util/gctuner/memory_limit_tuner.rs`

## 文件定位

本文件属于 `astersql-util-gctuner` crate，由 `pkg/util/gctuner/lib.rs` 公开为 `memory_limit_tuner` 子模块。它移植 Go `pkg/util/gctuner/memory_limit_tuner.go` 的 memory-limit 调谐算法：以 `task_memory::tracker::ServerMemoryLimit` 为基数计算阈值，在预计下一次 GC 会触达阈值时短暂提高阈值，降低内存用量在阈值附近波动造成的频繁回收。

当前 Rust 实现必须与“生产接线”区分看待。`setMemoryLimit` 只读写本文件的 `RUNTIME_MEMORY_LIMIT: AtomicI64`，没有调用 Rust 分配器、操作系统或 Go `runtime/debug.SetMemoryLimit` 的等价接口；因此它目前记录并驱动移植后的状态机，但不能独自约束进程实际内存。RustCodeGraph 将本文件的直接使用者列为 `pkg/util/gctuner/memory_limit_tuner_test.rs`；工作区 Rust 搜索也未发现生产代码调用本文件的 `GlobalMemoryLimitTuner` 或 `init()`。`br/cmd/br` 中同名对象来自 `br/cmd/br/stubs.rs`，不是这里的类型。

## 核心职责

- `calcMemoryLimit` 计算 `ServerMemoryLimit * percentage`，全局内存仲裁启用时把比例上限钳制为 `1.0`，禁用调整或零结果则回退到初始值/无穷上限语义。
- `UpdateMemoryLimit` 把配置比例转换为当前阈值，并维护 `isValidValueSet`；调整窗口内配置未变时不覆盖临时 fallback。
- `tuning` 依据 `heap_inuse * (100 + GOGC) / 100` 估算下一次 GC 阈值，连续两次命中才启动一次 fallback→复位窗口。
- `startResetWorker` 更新 memory-limit GC 的时间与次数指标，立即应用 `fallbackPercentage`（110%），等待后恢复当前配置比例。
- `Start`/`Stop` 管理周期 finalizer，`DisableAdjustMemoryLimit`/`EnableAdjustMemoryLimit` 为需要临时关闭调谐的调用者提供可嵌套计数。

这些职责的直接依据分别是 `MemoryLimitTuner::{calcMemoryLimit, UpdateMemoryLimit, tuning, startResetWorker, Start, Stop, DisableAdjustMemoryLimit, EnableAdjustMemoryLimit}`。文件不负责解析系统变量，也不拥有 `ServerMemoryLimit`、GOGC 或内存统计数据。

## 主要符号

- `fallbackPercentage: f64 = 1.1`：调整窗口的临时比例；`DEFAULT_RESET_INTERVAL` 为生产默认的一分钟。
- `initGOMemoryLimitValue`：禁用调整或计算结果无效时的回退值。Rust 初值固定为 `i64::MAX`；不同于 Go 文件在 `init()` 中通过 `debug.SetMemoryLimit(-1)` 捕获运行时原值。
- `RUNTIME_MEMORY_LIMIT`：`setMemoryLimit`/`currentMemoryLimit` 操作的原子镜像；负参数表示查询，非负参数交换并返回旧值。
- `MEMORY_GOROUTINE_COUNT`：已启动且尚未退出的复位线程数，由线程内 `CountGuard::drop` 保证递减。
- `AtomicF64`：把 `f64::to_bits` 存入 `AtomicU64`，用 `SeqCst` 提供原子 load/store；不提供算术或 compare-and-swap。
- `MemoryLimitTuner`：状态机主体。`memoryLimitTuner` 只是保持 Go 命名的类型别名。
- `MemoryLimitTuner::new` 与 `withResetInterval`：以 `Arc::new_cyclic` 建立 self weak reference，并创建带 `tuning` 回调的 `Finalizer`。后者暴露自定义间隔，主要用于独立测试。
- `GlobalMemoryLimitTuner`：`LazyLock<Arc<MemoryLimitTuner>>` 进程级懒加载实例；`init()` 只强制构造它。
- `WaitMemoryLimitTunerExitInTest`：轮询计数直到所有复位线程退出，供异步测试收尾。

公开 API 包括上述常量/静态量中标记为 `pub` 的项目、构造函数、生命周期方法、比例读写、更新/计算方法和测试可观察状态方法。`setMemoryLimit`、`AtomicF64`、`startResetWorker` 及三个内部静态量是文件私有实现。

## 执行流程

1. `new` 调用 `withResetInterval(60s)`。构造过程创建 `Finalizer`，其 weak 回调在对象仍存活时调用 `tuning`；相邻 `finalizer.rs::newFinalizer` 会立即启动每 300ms 一轮的运行时线程，所以新实例无需先调用 `Start` 才能被周期驱动。
2. 配置方先 `SetPercentage`，再调用 `UpdateMemoryLimit`。后者持有 `tuningLock`，调用 `calcMemoryLimit`；有效结果写入阈值并把 `isValidValueSet` 设为真，`i64::MAX` 结果则标记无效并写回 `initGOMemoryLimitValue`。
3. `tuning` 同样持有 `tuningLock`。无有效阈值时立即返回；否则从 `mem::readMemoryInuse` 取得 `heap_inuse`，从 `task_util::gogc::GetGOGC` 取得比例，计算预计的下一次 GC 堆大小。
4. 预计值未超过当前阈值时，清除 `nextGCTriggeredByMemoryLimit` 与全局 `TriggerMemoryLimitGC`。超过时先设置这两个标志；只有此前已经命中过，并且 `adjustPercentageInProgress` 能从 false 原子切换为 true，才保存服务器上限与比例快照并启动复位线程。
5. 复位线程通过 weak reference 获取 tuner，记录 `MemoryLimitGCLast`、累加 `MemoryLimitGCTotal`，应用 110% fallback，等待 `resetInterval`，再按执行当时的 `GetPercentage()` 与 `ServerMemoryLimit` 重算阈值，最后清除调整中标志。
6. 调整窗口内调用 `UpdateMemoryLimit` 时，如果服务器上限和比例仍与快照完全相同，则直接返回以保留 fallback；任一值变化都会立即按新配置重算。该分支对应 Go issue 48741 回归意图。

## 数据与状态

`percentage` 是用户配置，`isValidValueSet` 表示该配置结合服务器上限后是否产生非 `i64::MAX` 的有效阈值。`nextGCTriggeredByMemoryLimit` 是两阶段检测的记忆位；`adjustPercentageInProgress` 是单个 fallback worker 的准入门。`serverMemLimitBeforeAdjust` 与 `percentageBeforeAdjust` 仅用于判断调整期间配置是否变化。

`adjustDisabled` 是可嵌套的有符号计数：大于零时 `calcMemoryLimit` 返回初始值。调用方必须配对 Disable/Enable；实现没有防止多调用 `EnableAdjustMemoryLimit` 令计数变为负数，负数会被视为“已启用”。`percentage` 未校验 NaN、负数或超大值；浮点到 `i64` 使用 Rust `as` 转换，全局仲裁仅对上界执行 `min(1.0)`。扩展配置入口时需要在上游或本函数明确这些输入约束。

所有原子访问均为 `SeqCst`。真正需要跨多个字段保持顺序的 `tuning` 与 `UpdateMemoryLimit` 还共享 `tuningLock`；调整快照和进行中标志并非一个事务，沿用 Go 的最终一致语义。

## 依赖与调用关系

上游方面，`Finalizer` 回调调用 `MemoryLimitTuner::tuning`，测试还会通过 `runFinalizer` 或直接 `tuning` 驱动状态机。RustCodeGraph 文件关系显示直接使用文件为 `memory_limit_tuner_test.rs`；局部搜索另发现 `migration_aster_unit_test.rs` 覆盖迁移回归。没有找到工作区生产调用 `GlobalMemoryLimitTuner`/`init()` 的 Rust 接线，因此不能把 Go 中系统变量更新、服务启动或 BR 暂停调谐的调用链描述为当前 Rust 已接通。

下游方面：

- `crate::finalizer::{newFinalizer, Finalizer}` 提供周期回调、停止和手动运行；其运行线程在回调前调用 `mem::releaseUnusedMemory`。
- `crate::mem::readMemoryInuse` 读取 `task_memory::memstats::ForceReadMemStats().heap_inuse`。
- `task_memory::tracker` 提供 `ServerMemoryLimit`、`TriggerMemoryLimitGC`、`MemoryLimitGCLast` 和 `MemoryLimitGCTotal`。
- `task_memory::global_arbitrator::UsingGlobalMemArbitration` 决定是否把比例钳制到 100%。
- `task_util::gogc::GetGOGC` 提供下一次 GC 估算所用的 GOGC 值。

`pkg/util/gctuner/Cargo.toml` 证实生产依赖仅为路径依赖 `astersql-util-memory`（别名 `task-memory`）和 `astersql-util`（别名 `task-util`）；`intest`、`serial_test` 仅为开发依赖。

## 错误处理与边界

本 API 不返回 `Result`。锁中毒会由 `expect("memory tuner ... lock poisoned")` 触发 panic；后台线程 panic 不会传播给调用线程，但 `CountGuard` 仍会在栈展开时递减计数。weak upgrade 失败表示 tuner 已释放，finalizer 回调或复位线程直接退出。

关键边界如下：服务器上限与比例乘积转换后为 0 时返回 `i64::MAX`；`UpdateMemoryLimit` 随后恢复初始阈值并禁用调谐判定。比较条件严格使用 `>`，相等不会被视为触达。调整窗口配置未变的判断对浮点位值读取后使用 `==`；NaN 永不相等，会强制更新。`SystemTime::now` 可能受系统时钟调整影响，但这里只作为观测指标，不参与等待期限。

当前最大能力边界是 `RUNTIME_MEMORY_LIMIT` 仅为模型状态：相关单测验证的是该原子值和状态转换，而非真实分配器/运行时是否接受内存限制。文档或调用者不得据此声称 Rust 进程已经具备 Go runtime memory-limit 的硬/软约束效果。

## 并发与资源生命周期

`MemoryLimitTuner` 由 `Arc` 持有；其内部只保存 `Weak<Self>`，避免 self-cycle。`Finalizer` 也由独立线程通过 weak reference 访问，`Stop` 设置停止位并唤醒等待线程。`Start` 会先创建替代 finalizer，再锁住字段、停止旧实例并换入新实例；短暂期间两个 driver 都存在，但旧实例收到停止信号后退出。

连续两次命中时，`compare_exchange(false, true)` 保证同一时刻只启动一个复位 worker。worker 成功 upgrade 后会在整个 sleep 期间持有强 `Arc`，所以即使外部释放 tuner，它仍完成复位；`Stop` 只停止 finalizer，不取消已经启动的复位 worker。`WaitMemoryLimitTunerExitInTest` 无超时地每 100ms 轮询全局计数，测试必须避免 worker 永久阻塞。

`tuningLock` 防止 `tuning` 与 `UpdateMemoryLimit` 同时修改状态，但 `SetPercentage`、Disable/Enable 以及 worker 的最终复位不持有该锁；原子字段保证无数据竞争，组合状态只保证最终一致。worker 恢复时使用最新比例和最新服务器上限，而不是保存的快照，这正是配置在窗口中变化后最终落到新值的机制。

## 与 Go 版本的对应关系

结构与算法基本逐项对应 `memory_limit_tuner.go`：同名状态字段、110% fallback、两次命中判定、服务器上限/比例快照、禁用计数、issue 48741 的窗口内更新规则，以及 `MemoryLimitGCLast`/`MemoryLimitGCTotal`/`TriggerMemoryLimitGC` 指标均保留。

重要差异包括：

- Go 用 `debug.SetMemoryLimit` 查询和设置真实 Go runtime 限制；Rust `setMemoryLimit` 只操作本地原子镜像。
- Go `init()` 捕获真实初始限制并启动全局 tuner；Rust 初始值静态设为 `i64::MAX`，`init()` 只 force `LazyLock`，且未发现生产调用。Rust `newFinalizer` 构造时已经启动周期线程。
- Go 从 runtime `MemStats.HeapInuse` 取值；Rust 经 `mem::readMemoryInuse` 使用跨平台分配器统计。
- Go 测试模式通过 `intest`/failpoint 把一分钟缩短；Rust 用 `withResetInterval` 直接注入时长，没有本文件内 failpoint。
- Go 测试等待仅在 `intest.InTest` 时计数；Rust 对所有复位 worker 计数，等待函数也不检查测试模式。
- Rust 使用 `Mutex` poison panic、`Weak` 生命周期和 RAII 计数；Go 使用 goroutine、defer 与原子包。

Go 测试 `memory_limit_tuner_test.go` 用真实分配与 `runtime.GC` 验证 runtime 行为；Rust 独立测试更确定性地驱动 finalizer并验证移植状态机。因此两套测试意图相同，但验证层级并不完全等价。

## 扩展指南

- 若要让功能真正约束 Rust 进程内存，接入点应是 `setMemoryLimit`/`currentMemoryLimit`，并新增能证明分配器或仲裁器实际响应限制的独立测试；不要只让原子镜像变化。
- 若接入服务配置，生产代码需要在服务器内存上限或 GC 触发比例变化后调用 `SetPercentage` 与 `UpdateMemoryLimit`，并在进程初始化时明确 force/start 全局实例。接线前应先确认与现有 `task_memory` 全局仲裁的所有权，避免两套控制器互相覆盖。
- 修改命中判定应集中在 `tuning`，同步检查首次命中的 `TriggerMemoryLimitGC`、第二次命中的 worker 准入和低于阈值时的清除逻辑。
- 修改窗口行为应集中在 `startResetWorker` 与 `UpdateMemoryLimit`，保留 issue 48741 的“不变配置保留 fallback、变化配置立即应用”不变量。
- 为 Disable/Enable 增加防错时，应定义嵌套调用和计数下溢语义，并扩充 `test_set_memory_limit`；为 percentage 增加校验时，应覆盖零、负数、NaN、无穷及全局仲裁模式。
- 测试逻辑必须继续放在独立的 `pkg/util/gctuner/memory_limit_tuner_test.rs`（迁移级契约可放在既有 `migration_aster_unit_test.rs`），不要内嵌回生产源文件。并发测试使用 `serial_test`，恢复 `ServerMemoryLimit`、触发标志与仲裁器状态，并等待 worker 退出。

## 验证依据

- RustCodeGraph：`status` 显示索引含本仓库 7,032 个 Rust 文件；`files --filter pkg/util/gctuner` 列出目标、Go 对照和独立测试；`node --file pkg/util/gctuner/memory_limit_tuner.rs --offset 1 --limit 360` 返回完整 322 行和 45 个符号，并标记直接使用者 `memory_limit_tuner_test.rs`。
- RustCodeGraph 精确符号查询：`query MemoryLimitTuner`、`query UpdateMemoryLimit --json` 定位 Rust `MemoryLimitTuner` 与 `memory_limit_tuner.rs::UpdateMemoryLimit`；对该 Rust 节点执行 callers/callees 未返回生产调用边。
- 已读生产证据：`pkg/util/gctuner/memory_limit_tuner.rs`、`pkg/util/gctuner/finalizer.rs`、`pkg/util/gctuner/mem.rs`、`pkg/util/gctuner/lib.rs`、`pkg/util/gctuner/Cargo.toml`。
- 已读对照与测试：`pkg/util/gctuner/memory_limit_tuner.go`、`pkg/util/gctuner/memory_limit_tuner_test.go`、`pkg/util/gctuner/memory_limit_tuner_test.rs`，以及 `migration_aster_unit_test.rs` 中 memory-limit 回归段落。
- 局部 `rg` 核验了 `GlobalMemoryLimitTuner`、`MemoryLimitTuner`、`UpdateMemoryLimit` 与 crate 依赖；结果仅显示本 crate 测试/迁移测试使用此 Rust 实现，生产侧同名 BR 调用解析到独立 stub。
- 独立 Rust 测试覆盖：首次命中发布触发位、两阶段 fallback 与复位、issue 48741、禁用/启用、全局仲裁比例上限、自动 finalizer 驱动以及 GC 时间指标等待。按任务约束，本次纯文档分析未运行 Cargo。

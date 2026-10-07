# `pkg/lightning/common/pause.rs`

## 文件定位

本文件属于 `astersql-lightning-common` crate；crate 边界由 `pkg/lightning/common/Cargo.toml` 定义，`pkg/lightning/common/lib.rs` 以 `mod pause` 装入模块，并通过 `pub use pause::*` 将其 API 暴露给依赖者。它提供一个进程内、线程安全的暂停门闩，以及供门闩等待使用的轻量取消/超时上下文。

当前接线状态需要与设计用途区分：仓库搜索没有发现目标 Rust `Pauser` 的生产调用点。Rust `lightning/pkg/importer` 使用其 `stubs.rs::common_ext::Pauser`，`lightning/pkg/server` 也沿该 importer/common 路径建立暂停控制；因此本文件目前是已导出的公共实现和 Go 移植候选，而不是 Rust Lightning 导入主链已经采用的暂停器。Go 主链的真实对应接线位于 `lightning/pkg/importer/import.go` 和 `lightning/pkg/importer/chunk_process.go`。

## 核心职责

- `Pauser` 把“是否允许工作线程继续”封装成可并发切换的门闩：`Pause` 关闭门闩，`Resume` 打开门闩并广播唤醒，`Wait` 在门闩打开后返回。
- `Context` 为 `Wait` 提供共享取消标志和可选截止时间，使永久暂停不会让调用者失去退出路径。
- `generation` 将一次有效 `Resume` 定义为一个暂停周期的边界：已属于旧周期的等待者在 `Resume` 后即使马上再次 `Pause`，仍可完成旧等待。这一语义由 `pause_test.rs::resume_releases_waiters_from_the_current_pause_generation` 专门覆盖。
- `pauseStateRunning`、`pauseStatePaused`、`pauseStateLocked` 保留 Go 枚举的公开名称和值，但 Rust 的 `Mutex<PauseData>` 实现不使用这三个常量驱动内部状态机。

## 主要符号

- `Context { cancelled: Arc<AtomicBool>, deadline: Option<Instant> }`：可克隆上下文。克隆共享取消位，但复制同一个绝对截止时间。
- `Context::Background() -> Context`：创建未取消且无截止时间的上下文。
- `Context::WithTimeout(Duration) -> Context`：以当前时刻加时长计算绝对截止时间。
- `Context::Cancel(&self)`、`Done(&self) -> bool`、`Err(&self) -> CommonError`：分别发布取消、检查取消或超时、构造结束原因。
- `PauseData { paused, generation, waiters }`：受同一互斥锁保护的私有状态。`waiters` 只计数，不保存线程句柄。
- `Pauser { state: Mutex<PauseData>, resumed: Condvar }`：暂停门闩；公开方法都以共享引用 `&self` 工作。
- `NewPauser() -> Pauser`：创建 `paused = false`、`generation = 0`、`waiters = 0` 的门闩。
- `Pauser::Pause`、`Resume`、`IsPaused`、`Wait`：分别关闭门闩、恢复并广播、查询状态、等待恢复或上下文结束。
- `Pauser::cancel`：只在 `Wait` 持锁时递减等待者计数；它不取消 `Context`，也不改变 `paused`。

## 执行流程

1. 调用者用 `NewPauser` 获得初始运行态门闩；此时 `Wait` 在取得锁并发现 `paused == false` 后立即返回 `Ok(())`。
2. `Pause` 取得互斥锁并把 `paused` 设为 `true`。重复暂停是幂等赋值，不推进代际，也不唤醒任何人。
3. 暂停期间调用 `Wait` 时，线程记录当前 `generation`，递增 `waiters`，然后进入条件变量循环。
4. 每轮先检查 `ctx.Done()`；若上下文已取消或过期，则经 `cancel` 递减计数，并返回 `ctx.Err()`。
5. 若上下文仍有效，线程通过 `Condvar::wait_timeout` 临时释放互斥锁。单次睡眠不超过 20ms；有 deadline 时还会被剩余时间进一步缩短。
6. `Resume` 仅在确实暂停时工作：清除 `paused`、以 wrapping 加法推进 `generation`，随后 `notify_all`。
7. 等待循环以“仍暂停且代际未变”为继续条件。因而一次 `Resume` 足以释放旧代际等待者；随后的快速 `Pause` 只约束新代际调用者。退出时 `waiters` 减一并返回成功。

## 数据与状态

`paused`、`generation` 和 `waiters` 始终由 `state` 互斥锁一起保护。核心不变量是：每个在暂停路径上成功递增 `waiters` 的 `Wait`，恰在成功退出或上下文错误退出时递减一次。`generation` 只由有效的 `Resume` 修改；对运行态调用 `Resume` 是无操作。

`Context.cancelled` 使用 `Release` 写和 `Acquire` 读，使克隆之间能跨线程观察取消。`deadline` 是本地单调时钟的 `Instant`，不受系统墙钟调整影响。`Context::Err` 本身不验证 `Done`；调用者若在未结束时直接调用它，会得到 `context deadline exceeded`，所以该方法的有效前置条件是先确认 `Done()`，本文件的 `Wait` 遵守此前置条件。

三个公开 `pauseState*` 常量分别为 0、1、2，仅用于保持与 `pause.go` 的枚举表面对齐；Rust 的真实状态不是一个可观察的三态原子值，“locked” 由 `Mutex` 的持有状态表达。

## 依赖与调用关系

下游依赖均来自标准库，另有 crate 内错误类型：`Arc<AtomicBool>` 承载共享取消，`Mutex` 串行化门闩状态，`Condvar` 执行阻塞/广播，`Instant`/`Duration` 处理截止时间；错误通过 `crate::CommonError::new` 构造。`pkg/lightning/common/Cargo.toml` 没有为本文件引入专用第三方依赖。

模块入口边为 `pkg/lightning/common/lib.rs -> mod pause -> pub use pause::*`。workspace 根 `Cargo.toml` 以 `facade_lightning_common` 指向此 crate，`pkg/lib.rs` 又将该 facade 暴露在兼容命名空间中；多个 crate 的 Cargo manifest 依赖 `astersql-lightning-common`，但仓库级精确符号搜索没有找到它们调用这里的 `NewPauser`/`Pauser::Wait`。

Go 侧调用链是 `lightning/pkg/importer/import.go::Controller.Pause/Resume -> common.Pauser.Pause/Resume`，工作循环在 `lightning/pkg/importer/chunk_process.go` 调用 `pauser.Wait(ctx)`。Rust importer 的同名实现当前由 `lightning/pkg/importer/stubs.rs::common_ext::Pauser` 提供，且该桩没有 `Wait`；因此不能把 Rust `lightning/pkg/importer/import.rs` 的同名 `Pause`/`Resume` 记作本文件的调用者。

## 错误处理与边界

`Wait` 只有上下文结束这一条业务错误路径：取消优先于截止时间，分别生成 kind 为 `context`、消息为 `context canceled` 或 `context deadline exceeded` 的 `CommonError`。无 deadline 的后台上下文在没有 `Resume` 时可无限等待。

锁中毒通过 `expect("Pauser mutex poisoned")` 触发 panic，而不是返回 `CommonError`；这是 API 的进程内一致性边界。`generation.wrapping_add(1)` 明确定义溢出为回绕。理论上只有在同一仍存活等待者跨越 `u64` 个有效恢复周期时才会发生代际别名，正常生命周期中不可达，但扩展为长期高频控制器时不应移除代际判断。

`Wait` 对在进入函数前已经结束的上下文仍先检查门闩：若当前未暂停，它返回成功，不返回上下文错误。这与它“只在暂停时等待”的职责一致。取消等待者不会恢复整个 `Pauser`；后来的等待者仍被同一个暂停周期阻塞。

## 并发与资源生命周期

`Pauser` 本身未实现 `Clone`；并发共享应放入 `Arc<Pauser>`，测试也采用这一模式。条件变量等待会释放互斥锁，允许 `Resume` 或其他线程取得状态锁。虚假唤醒由 while 条件重新检查吸收。

20ms 超时轮询是取消响应延迟与唤醒开销之间的折衷：`Context::Cancel` 不直接通知 `Condvar`，所以被取消线程最迟通常在下一次轮询醒来后退出；deadline 同样依靠超时醒来。`Resume::notify_all` 在持有状态锁期间调用，等待者醒来后仍需重新竞争该锁。

等待者没有独立堆资源或 channel；其生命周期由 `waiters` 计数和当前栈帧表示。正常恢复、取消和超时都会平衡计数。`Context` 克隆共享取消状态，因此任一克隆调用 `Cancel` 会影响全部克隆；截止时间则随值复制且不可续期。

## 与 Go 版本的对应关系

`pkg/lightning/common/pause.go` 是直接语义对照。两版都提供 `NewPauser`、`Pause`、`Resume`、`IsPaused`、`Wait`，都要求运行态快速放行、暂停态登记等待者、恢复时释放当期全部等待者、取消时只移除当前等待者而不改变全局暂停状态。

实现机制不同。Go 版用原子三态 `Running/Paused/Locked` 作为自旋锁，并用 `map[chan<- struct{}]struct{}` 保存每个等待者；`Resume` 交换整张 map 后关闭旧 channel。Rust 版用 `Mutex + Condvar + generation`，其中代际承担“恢复当期等待者”的 channel 批次语义。Rust 的三个状态常量没有参与算法。

上下文也不是完整等价：Go 接受标准 `context.Context`，可由父上下文、显式取消或多种 deadline 机制结束，并由 channel 即时通知；Rust `Context` 仅支持一个共享布尔取消位和可选 deadline，且以至多 20ms 的轮询观察结束。Go `NewPauser` 返回指针，Rust 返回值；共享 Rust 值需要调用方显式使用 `Arc`。

`pkg/lightning/common/pause_test.go::TestPause` 与 Rust `pause_test.rs::test_pause` 都验证初始放行、暂停阻塞、恢复广播、取消报错和取消不恢复门闩。Rust 另有代际回归测试，覆盖 `Resume` 后立即 `Pause` 时旧等待者仍必须获释。Go 文件还有三类 benchmark，Rust 独立测试当前没有对应性能基准。

## 扩展指南

- 若把本实现接入 Rust Lightning 主链，首先替换/收敛 `lightning/pkg/importer/stubs.rs::common_ext::Pauser`，并在实际 chunk 工作循环接入 `Wait`；不能只把控制面 `Pause`/`Resume` 指向本类型，否则数据面仍不会停下。
- 新增状态或观测字段时，应与 `paused/generation/waiters` 放在同一锁域内，保持登记与退出计数成对，并在 `pkg/lightning/common/pause_test.rs` 增加独立并发测试，不把测试写入生产文件。
- 若要降低取消延迟，需让取消动作能够唤醒条件变量，或采用可选择的通知原语；同时验证无 deadline 后台等待、并发 `Resume/Pause` 和虚假唤醒，避免以忙等换取响应速度。
- 若扩充 `Context`（父子取消、原因传播或 deadline 查询），应先决定是否直接复用仓库已有上下文抽象，并同步核对 Go `context.Context` 契约。保持“门闩运行时不因已取消上下文报错”的现有 `Wait` 边界，除非调用方迁移计划明确改变它。
- 性能改动应补齐 Rust 基准，重点测运行态热路径、已取消暂停路径和 Pause/Resume 竞争；Go benchmark 可作为场景清单，但不能充当 Rust 性能结论。

## 验证依据

- 生产实现：`pkg/lightning/common/pause.rs`，核对了全部常量、结构体、构造函数、方法、锁域和等待循环。
- crate 边界：`pkg/lightning/common/Cargo.toml`、`pkg/lightning/common/lib.rs`、workspace 根 `Cargo.toml` 与 `pkg/lib.rs`。
- Rust 独立测试：`pkg/lightning/common/pause_test.rs`，包括 `test_pause` 和 `resume_releases_waiters_from_the_current_pause_generation`。
- Go 对照与测试：`pkg/lightning/common/pause.go`、`pkg/lightning/common/pause_test.go`。
- 真实 Go 接线：`lightning/pkg/importer/import.go`、`lightning/pkg/importer/chunk_process.go`、`lightning/pkg/server/lightning.go`。
- Rust 接线边界：`lightning/pkg/importer/import.rs`、`lightning/pkg/importer/stubs.rs`、`lightning/pkg/server/lightning.rs`；它们证明同名 API 当前来自另一实现，未形成到本文件的调用边。
- RustCodeGraph：`status` 显示索引可用（11,467 个文件），但 `files --filter pkg/lightning/common/pause` 与 `files --filter lightning/common` 均返回无匹配；依照技能规则，对该未索引目录使用 `rg` 和直接文件读取。宽泛 `explore` 只用于发现 Lightning 同名路径，最终均以精确源码消歧。
- 本任务仅新增说明文档，不修改运行时代码，也不运行 Cargo；结构完整性由任务指定的 11 章节命令验证。

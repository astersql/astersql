# `pkg/util/gctuner/tuner.rs`

## 文件定位

该文件是 `astersql-util-gctuner` crate 中的 GOGC 百分比调谐实现。crate 入口 `pkg/util/gctuner/lib.rs` 以 `pub mod tuner` 暴露本模块；仓库门面 `pkg/lib.rs` 又通过 `facade_util_gctuner` 暴露整个 gctuner crate。它和同 crate 的 `finalizer.rs`、`mem.rs` 配合：周期驱动到来时读取进程堆占用，根据一个字节阈值计算目标 GOGC，再交给 `task_util::gogc` 保存运行时兼容值。

这里的 “GOGC” 是从 Go 版本迁移而来的 GC 触发比例语义。当前 Rust 后端 `pkg/util/gogc.rs` 只维护原子数值，不控制 Rust 分配器或 Rust GC（Rust 本身没有 Go 式 tracing GC）；实际周期驱动和分配器空闲页回收由 `pkg/util/gctuner/finalizer.rs` 实现。因此本文件是兼容层中的策略计算与生命周期控制，不应被理解为完整的 Rust 垃圾收集器。

`pkg/util/gctuner/Cargo.toml` 声明 crate 名为 `astersql-util-gctuner`，库入口为 `lib.rs`，运行时依赖为 `task-memory`（经同 crate 的内存探测间接使用）和 `task-util`（本文件直接调用 `task_util::gogc`）。Cargo 元数据将其 Go 对照包指定为 `pkg/util/gctuner`。

## 核心职责

本文件承担四项职责：

1. 管理进程级调谐配置：`minGCPercent`、`maxGCPercent`、`EnableGOGCTuner` 和从 `GOGC` 环境变量惰性读取的 `DEFAULT_GC_PERCENT`。
2. 通过 `Tuning` 管理一个进程级 `GLOBAL_TUNER`，创建实例、更新高水位阈值，或在已有实例时用零阈值停止并清空实例。
3. 由 `Tuner::tuning` 把 `readMemoryInuse()` 的当前占用和阈值送入 `calcGCPercent`，再调用 `task_util::gogc::SetGOGC`。
4. 用 `calcGCPercent` 实现 Go 公式 `(threshold - inuse) / inuse * 100`，并按可配置的最小值和最大值限幅。

职责边界也很明确：本文件不采集平台内存、不创建定时线程、不直接回收内存；这些分别属于 `mem.rs` 和 `finalizer.rs`。它也不修改真实 Go runtime；`pkg/util/gogc.rs` 明确说明迁移阶段只保存配置/观测值。

## 主要符号

- `defaultMaxGCPercent: u32 = 500`、`defaultMinGCPercent: u32 = 100`：默认限幅区间。公开原子 `maxGCPercent`、`minGCPercent` 保存当前配置，`SetMaxGCPercent`/`MaxGCPercent` 与 `SetMinGCPercent`/`MinGCPercent` 是读写 API，均使用 `Ordering::SeqCst`。
- `EnableGOGCTuner: AtomicBool`：调谐开关，初始为 `false`。它只阻止周期计算，不阻止 `Tuning` 创建实例或更新阈值。
- `DEFAULT_GC_PERCENT: LazyLock<AtomicU32>` 与 `defaultGCPercent()`：首次访问时读取环境变量 `GOGC`，先按 `i32` 解析再转换为 `u32`；未设置或解析失败时使用 100。初始化后环境变量变化不会被重新读取。
- `init()`：显式触发默认值初始化，并把上下限恢复成 100/500。Rust 没有 Go 包级 `init` 钩子；仓库检索未发现生产调用者，因此不能假定它会自动执行。静态上下限自身已用 100/500 初始化。
- `SetDefaultGOGC()`：将默认百分比传给 `gogc::SetGOGC`，忽略其旧值返回结果。
- `GLOBAL_TUNER: LazyLock<Mutex<Option<Arc<Tuner>>>>`：进程级单例槽位。`Tuning(threshold)` 是管理入口，`GetGOGC()` 在无实例时返回默认值，有实例时返回实例记录值。
- `Tuner`（以及 Go 风格别名 `tuner`）：包含 `Arc<Finalizer>`、记录值 `gcPercent: AtomicU32` 和字节阈值 `threshold: AtomicU64`。
- `newTuner(threshold) -> Arc<Tuner>`：用 `Arc::new_cyclic` 创建实例；finalizer 回调仅捕获 `Weak<Tuner>`，避免 `Tuner -> Finalizer -> callback -> Tuner` 的强引用环。
- `Tuner::{stop, runFinalizer, setThreshold, getThreshold, setGCPercent, getGCPercent, tuning}`：分别控制周期驱动、提供显式测试触发、管理阈值与观测值，以及执行一轮调谐。
- `calcGCPercent(inuse, threshold) -> u32`：纯计算入口，也是固定表测试的主要对象。

## 执行流程

全局入口的流程如下：

1. 调用者执行 `Tuning(threshold)`，函数先锁住 `GLOBAL_TUNER`。
2. 若 `threshold == 0` 且已有实例，函数从 `Option` 中取出实例，调用 `Tuner::stop()` 唤醒并终止 finalizer 驱动，然后返回。
3. 若当前无实例，则调用 `newTuner(threshold)`。这也意味着首次调用 `Tuning(0)` 会创建一个阈值为零的实例，而不是进入第 2 步；这是与 Go `tuner.go` 相同的实际分支顺序。
4. 若已有实例且阈值非零，则只用 `setThreshold` 原子更新阈值，不重建周期驱动。

实例的一轮调谐由 `newTuner` 注册的 finalizer 回调进入 `Tuner::tuning`：

1. `EnableGOGCTuner` 为 `false` 时立即返回。
2. 原子读取阈值；阈值为零时立即返回。
3. 调用 `readMemoryInuse()` 获取 `heap_inuse`，执行 `calcGCPercent(inuse, threshold)`。
4. `setGCPercent` 调用 `gogc::SetGOGC(target)`。后者以原子 swap 写入目标并返回旧值；本实例把这个旧值写入 `gcPercent`。因此 `getGCPercent()`/`GetGOGC()` 观测的是最近一次设置前的值，会相对目标更新滞后一轮。这与 Go `util.SetGOGC` 返回旧值并保存返回值的逻辑一致。

`calcGCPercent` 的分支顺序是：`inuse == 0` 或 `threshold == 0` 返回默认值；`threshold <= inuse` 返回最小值；否则以浮点数计算并向下取整，再先检查最小值、后检查最大值。正常默认区间下，较低占用得到较大百分比（降低调谐频率），接近或超过阈值得到较小百分比（提高调谐积极性）。

## 数据与状态

配置和运行状态均为进程级：上下限、开关和默认值是静态原子；唯一活动实例存放于静态互斥槽位。`Tuner` 内部的阈值和记录百分比也都是原子，因此 finalizer 驱动线程、配置线程和观测线程之间无需持有实例内部互斥锁。

`threshold` 的单位是字节，代表目标堆高水位；`inuse` 来自 `ForceReadMemStats().heap_inuse`。公式隐含关系为 `threshold = inuse + inuse * gcPercent / 100`。计算使用 `f64` 后向下取整，再转为 `u32`；最终值受当前上下限约束。

`gcPercent` 的名字容易造成误读：按当前 `setGCPercent` 实现，它存的是 `gogc::SetGOGC` 返回的旧值，不一定等于刚写入后端的当前值。`pkg/util/gctuner/tuner_test.rs` 因而通过最多八轮 finalizer 触发等待观测值收敛，而非假设一次调用立即可见目标值。

环境变量默认值由 `LazyLock` 固化。负数若能按 `i32` 解析，会按 Rust `as u32` 转成补码对应的大正数；Go 对照同样先 `Atoi` 再转换为 `uint32`。`gogc::SetGOGC` 接受 `i32`，本文件从 `u32` 转回 `i32`，所以非标准极端环境值或配置值会发生整数转换；当前测试覆盖的是正常百分比范围。

## 依赖与调用关系

下游直接依赖如下：

- `newTuner -> newFinalizer`：创建周期驱动。`finalizer.rs` 每 300 ms 尝试让系统分配器归还空闲页，再调用回调；`stop` 设置停止标志并通过条件变量唤醒线程。
- finalizer 回调 `-> Weak::upgrade -> Tuner::tuning`：实例仍存活才调谐，销毁后回调不会维持实例生命周期。
- `Tuner::tuning -> readMemoryInuse`：读取 `task_memory::memstats::ForceReadMemStats().heap_inuse`。
- `Tuner::tuning -> calcGCPercent -> {defaultGCPercent, MinGCPercent, MaxGCPercent}`：计算与限幅。
- `Tuner::setGCPercent`、`SetDefaultGOGC -> task_util::gogc::SetGOGC`：更新兼容 GOGC 原子后端。

RustCodeGraph 已索引 `tuner.rs` 的 35 个符号，并将该文件识别为被 13 个文件使用；精确 `query` 确认了本文件的 `Tuning` 与 `tuning` 节点。仓库文本检索在 Rust 侧找到的直接 `Tuning` 调用位于 `pkg/util/gctuner/migration_aster_unit_test.rs`，`newTuner`/`calcGCPercent` 的调用主要位于独立的 `tuner_test.rs` 和迁移聚合测试。未找到非测试 Rust 生产代码调用 `Tuning`，所以当前可证事实是 API 已经由模块/门面暴露，但生产主链接线尚未在仓库中发现；不能沿用 Go 调用链宣称 Rust 已实际启用它。

Go 侧上游可在 `pkg/sessionctx/variable/sysvar.go` 中看到 `gctuner.Tuning` 调用，但这只能作为 Go 版本应用位置的对照，不能当作 Rust 调用证据。

## 错误处理与边界

本模块没有返回 `Result`。可恢复边界通过默认值或提前返回处理：无效环境变量回退到 100；占用或阈值为零时计算返回默认值；禁用开关或实例阈值为零时一轮调谐直接退出；堆占用达到/超过阈值时返回最小值。

不可恢复的同步错误使用 `expect`：`GLOBAL_TUNER` 互斥锁中毒会在 `Tuning`/`GetGOGC` 处 panic；`global.is_none()` 分支之后理论上必有实例，若不变量被破坏，`expect("tuner was initialized")` 也会 panic。finalizer 内部的回调锁和条件变量锁中毒同样会 panic，详见 `finalizer.rs`。

上下限设置函数不验证 `min <= max`。`calcGCPercent` 固定先比较最小值再比较最大值，因此上下限倒置时可能返回配置的最小值；独立回归 `calc_gc_percent_preserves_go_order_for_inverted_bounds` 明确锁定了这一 Go 兼容行为，扩展时不应擅自改成会排序或报错的通用 clamp。

`calcGCPercent` 先处理 `threshold <= inuse`，避免无符号减法下溢。正常非零且阈值更大的路径使用浮点计算；默认上限很小，不会暴露转换饱和问题，但任意公开配置值仍属于调用者需要约束的输入。

## 并发与资源生命周期

Rust 版本用 `Mutex<Option<Arc<Tuner>>>` 串行化全局实例创建、更新、停止和读取，修复了 Go 源码注释中“全局单例非线程安全”的限制。实例字段和公共配置采用 `SeqCst` 原子操作，给并发配置与周期读取提供最强的原子顺序保证。

`newTuner` 创建 `Finalizer` 时会启动独立线程。线程仅持有 `Weak<Finalizer>`；回调又仅持有 `Weak<Tuner>`，两层弱引用都避免后台线程或回调延长所有者生命周期。`Tuning(0)` 对已有实例调用 `stop`：停止位变为真，条件变量唤醒等待线程，之后 `Finalizer::run` 返回 `false` 且不再执行回调。`runFinalizer()` 是显式触发边界，源码标注为测试用途。

`Tuning` 在持有全局互斥锁期间调用 `tuner.stop()`；当前 `stop` 只写原子并通知条件变量，不等待线程 join，也不回调全局入口，因此没有已证实的锁递归。停止后的 `Arc<Tuner>` 在局部变量离开作用域后释放，后台线程下一次无法升级弱引用或观察停止位后退出。

由于 GOGC 后端、上下限、开关和全局实例均是进程级状态，测试必须串行化并恢复修改过的值。`tuner_test.rs` 使用 `serial_test::serial`，并在相关用例中恢复默认上下限及关闭开关。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/gctuner/tuner.go`，Rust 基本保留了公开命名、默认值、计算公式、分支顺序和 `SetGOGC` 旧值记录语义：

- Go 的 `atomic.Uint32/Uint64/Bool` 对应 Rust 原子类型；`tuner` 对应 `Tuner` 加类型别名。
- Go 的 `newFinalizer(t.tuning)` 对应 Rust 闭包升级 `Weak<Tuner>` 后调用 `tuning`。
- `calcGCPercent` 的无效输入、超过阈值、向下取整和先 min 后 max 的顺序一致；Go/Rust 固定表都期望 4 GiB 阈值下得到 300、166、100 等结果。
- Go 的包级 `init()` 自动执行；Rust 的 `init()` 只是普通公开函数，当前未发现生产调用。Rust 静态原子已直接带默认上下限，默认 GOGC 则按首次访问惰性解析。
- Go 明确说 `globalTuner` 非线程安全；Rust 为单例槽位增加 `Mutex`，允许并发管理入口。
- Go 由 tracing-GC finalizer 触发；Rust 无对应运行时钩子，改为 `finalizer.rs` 的 300 ms 后台周期、分配器空闲页回收和显式 `run`。因此触发时机与内存统计源不可能完全等同。
- Go 测试通过分配 `testHeap` 改变 `HeapInuse` 并调用 `runtime.GC()`；Rust 测试根据每次实测进程占用反推阈值并显式运行 finalizer，以保持占用/阈值比例断言。

`pkg/util/gctuner/tuner_test.rs` 覆盖调谐从最大值、中间区间到最小值的收敛、超过阈值、停止后不能运行、固定计算表及倒置上下限。`migration_aster_unit_test.rs` 另外覆盖全局 `Tuning` 的创建、阈值更新和 `Tuning(0)` 清理后 `GetGOGC` 回到默认值。

## 扩展指南

新增或修改调谐策略时，优先保持职责分层：纯比例算法修改 `calcGCPercent`；配置 API 修改公共原子及访问函数；实例生命周期修改 `Tuning`/`newTuner`/`Tuner::stop`；触发频率、线程退出和分配器回收应修改 `finalizer.rs`；内存口径应修改 `mem.rs`；GOGC 后端语义应修改 `pkg/util/gogc.rs`。

任何策略变化都应同步独立测试 `pkg/util/gctuner/tuner_test.rs`，不要把 Rust 测试嵌入生产源文件。涉及全局生命周期时还应同步 `migration_aster_unit_test.rs`；涉及 Go 语义对齐时核对 `tuner.go` 与 `tuner_test.go` 的分支和期望。测试要继续使用串行保护，并在结束前恢复进程级开关和上下限，避免污染其他用例。

兼容风险包括：改变 `setGCPercent` 保存旧值的行为会改变 `GetGOGC` 的观测时序；调整首次 `Tuning(0)` 的分支会偏离现有 Go 行为；将倒置边界改为排序或 panic 会破坏已有回归；自动调用 `init()` 或改变环境变量解析时机会影响进程启动配置。性能风险主要来自缩短 finalizer 周期、增加内存探测频率、扩大锁持有范围或采用更激进的低 GOGC 值。若要把 API 接入 Rust 生产主链，必须先找到对应配置生命周期，并新增独立集成/单元验证，不能仅依据 Go 的 `sysvar.go` 调用位置推定接线。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/gctuner` 列出 `tuner.rs` 及相邻实现/测试；`node --file pkg/util/gctuner/tuner.rs --offset 1 --limit 260` 返回完整 210 行源码、35 个符号的文件视图及 13 个使用文件；`query Tuning --kind function --json` 定位 `tuner.rs::Tuning` 和 `tuner.rs::tuning`。精确 `callers` 查询在本次环境中未在限定时间内返回，因此调用关系又以仓库文本检索核对，没有把超时解释成“无调用者”。
- 生产源码：`pkg/util/gctuner/tuner.rs`、直接下游 `finalizer.rs`、`mem.rs`、`pkg/util/gogc.rs`，以及模块入口 `pkg/util/gctuner/lib.rs` 和门面 `pkg/lib.rs`。
- crate 配置：`pkg/util/gctuner/Cargo.toml`，用于确认 crate 名、入口、依赖和 Go 包映射。
- Go 对照：`pkg/util/gctuner/tuner.go` 与 `tuner_test.go`；Go 上游位置通过 `pkg/sessionctx/variable/sysvar.go` 的文本/图索引结果核对。
- Rust 测试：`pkg/util/gctuner/tuner_test.rs` 和 `pkg/util/gctuner/migration_aster_unit_test.rs`。它们分别验证比例计算、限幅顺序、周期收敛/停止，以及全局实例生命周期。

本任务是纯文档分析，按计划未运行 Cargo，也未修改 Rust、Go、Cargo 或总计划。人工复核重点为：文件存在的兼容目的、全局入口与周期执行流程、旧值观测语义、弱引用/线程生命周期、Go 差异、当前生产接线证据边界，以及安全扩展时需要同步的独立测试文件。

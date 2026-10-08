# `pkg/util/gogc.rs`

## 文件定位

本文件属于 `astersql-util` crate（见 `pkg/util/Cargo.toml`），并由 crate 根 `pkg/util/lib.rs` 通过 `pub mod gogc` 公开。它是 Go `pkg/util/gogc.go` 的迁移期对应实现，为其他 Rust crate 提供进程级 GOGC 数值的初始化、设置和读取接口。

这里的“GOGC”只是一个配置/观测缓存。当前实现没有连接 Rust 分配器或垃圾回收器，也没有 Go `runtime/debug` 的等价运行时边界。因此它在完整应用中的实际位置是 `pkg/util/gctuner` 的共享状态底座，而不是能够直接改变 Rust 进程内存回收策略的 GC 控制器。`pkg/util/gctuner/Cargo.toml` 以 `task-util = { package = "astersql-util", path = ".." }` 引入本 crate，`tuner.rs` 和 `memory_limit_tuner.rs` 再以 `task_util::gogc` 使用这些 API。

## 核心职责

- 在第一次访问模块状态时读取环境变量 `GOGC`，将合法的 `i32` 十进制文本作为初值；变量缺失、非 UTF-8、解析失败或超出 `i32` 范围时使用 `100`（`gogcValue`）。
- 对外提供 `SetGOGC`，把非正输入归一为 `100`，原子地写入新值并返回被替换的旧缓存值。
- 对外提供 `GetGOGC`，原子读取最近一次初始化或设置后的缓存值。
- 提供普通函数 `init`，显式触发懒初始化；Rust 不会像 Go 包那样自动调用该函数，且当前检索没有发现生产调用者。

本文件刻意只保存数值语义。源码注释中“调用 `runtime/debug.SetGCPercent` 并更新指标”描述的是 Go 版本意图，不是当前 Rust 函数已经实现的副作用；实际函数体只有 `AtomicI32` 的 `swap`/`load`。

## 主要符号

- `static gogcValue: LazyLock<AtomicI32>`：模块唯一状态。`LazyLock` 保证初始化闭包至多成功执行一次；闭包通过 `std::env::var("GOGC")`、`parse::<i32>()` 和 `unwrap_or(100)` 得到初值。它是私有符号，外部只能通过三个公开函数观察或改变状态。
- `pub fn init()`：执行一次顺序一致读，从而强制 `gogcValue` 初始化。无参数、无返回值，也不重置已经初始化的值；首次访问后再修改进程环境不会影响缓存。
- `pub fn SetGOGC(mut val: i32) -> i32`：公开写入口。`val <= 0` 时先改为 `100`，随后 `swap(val, Ordering::SeqCst)`；返回值是交换前的缓存值。
- `pub fn GetGOGC() -> i32`：公开读入口，以 `Ordering::SeqCst` 返回当前缓存。

文件没有类型、trait、`impl`、feature gate 或条件编译项；三个函数均为公开 API，初始化状态为内部实现。

## 执行流程

首次调用 `init`、`SetGOGC` 或 `GetGOGC` 时，解引用 `gogcValue` 会触发以下流程：

1. 读取当前进程的 `GOGC` 环境变量。
2. 若读取成功，尝试按 Rust `i32` 解析完整字符串；成功即保留原值，包括 `0` 和负数。
3. 若读取或解析失败，以 `100` 建立 `AtomicI32`。
4. 后续所有访问复用同一个原子对象，不再读取环境。

`SetGOGC` 的调用流程是先检查输入；输入小于等于零时统一改为 `100`，正数保持不变；随后一次原子交换同时发布新值和取得旧值。因此连续执行 `SetGOGC(250)`、`SetGOGC(0)` 会依次返回调用前的值和 `250`，最终 `GetGOGC()` 为 `100`。这一状态转换由 `pkg/util/cpu_posix_1_aster_unit_test.rs::gogc_and_id_generator_preserve_go_state_transitions` 直接断言。

`pkg/util/gctuner/tuner.rs::SetDefaultGOGC` 用默认百分比调用 `gogc::SetGOGC`；`Tuner::setGCPercent` 在动态调谐时写入目标值，并把 `SetGOGC` 返回的旧值记录进自己的 `gcPercent`。`pkg/util/gctuner/memory_limit_tuner.rs::MemoryLimitTuner::tuning` 则读取 `gogc::GetGOGC`，按 `(100 + GOGC) / 100` 估算下一次 GC 触发时的堆比例。因而缓存值会参与调谐决策，即使它目前不会直接控制真实运行时 GC。

## 数据与状态

全局状态只有一个 `i32`。默认值是 `100`，但环境变量中任何能解析为 `i32` 的值均可成为初值；初始化路径不会应用 `SetGOGC` 的“非正数归一为 100”规则。这意味着 `GOGC=0` 或负值在第一次读取时会原样暴露，直到一次 `SetGOGC` 调用将其改写。环境变量 `GOGC=off` 不能解析为整数，因此落到 `100`。

`SetGOGC` 不保留设置历史，只返回紧邻本次交换之前的单个值。没有独立的“默认值”“运行时值”或指标状态，也没有持久化；进程退出后状态消失。测试会在断言后用 `SetGOGC(old)` 恢复进入测试前的值，说明该状态跨调用、跨测试共享。

类型范围是 Rust `i32`，而 Go 对照实现的缓存是 `int64`、API 使用平台宽度的 `int`。超出 `i32` 范围的环境文本在 Rust 中会被当作解析失败并回退为 `100`，这是移植边界差异。

## 依赖与调用关系

直接下游依赖全部来自标准库：`std::env` 负责读取环境，`std::sync::LazyLock` 负责一次性初始化，`AtomicI32` 和 `Ordering::SeqCst` 负责共享状态。`pkg/util/Cargo.toml` 没有为本文件引入专用第三方依赖或 feature。

RustCodeGraph 对 `pkg/util/gogc.rs` 的文件节点记录了两个直接使用文件：`pkg/util/gctuner/tuner.rs` 与 `pkg/util/cpu_posix_1_aster_unit_test.rs`；精确符号探索还给出 `SetGOGC` 的三个实际调用点：`SetDefaultGOGC`、`Tuner::setGCPercent` 和聚合单测，以及 `GetGOGC` 在该单测中的调用。原始引用检索补充确认 `pkg/util/gctuner/memory_limit_tuner.rs::tuning` 也读取 `GetGOGC`。RustCodeGraph 的精确 `callers --file` 对这些函数未返回边，因此以上关系同时以调用点源码核验，而不是把空图结果解释为“无调用者”。

`init` 没有检索到调用点；模块仍会因 `LazyLock` 在首次 `SetGOGC`/`GetGOGC` 时正确初始化。`pkg/util/gctuner` 通过自己的 Cargo 路径依赖跨 crate 调用本模块，测试则通过 `pkg/util/lib.rs` 的同 crate 测试模块直接访问 `super::gogc`。

## 错误处理与边界

所有公开函数都是不可失败接口：没有 `Result`、错误返回、日志或 panic 分支。环境读取/解析错误被 `.ok()` 和 `unwrap_or(100)` 吞并为默认值，因此调用方无法区分“未设置”“格式非法”“非 UTF-8”与“数值越界”。

关键输入边界如下：

- `SetGOGC(0)` 和任意负数都写入 `100`；测试覆盖了零值，负值由同一 `val <= 0` 分支处理。
- 正 `i32`（包括 `i32::MAX`）原样写入；API 类型阻止更大整数进入函数。
- 初始化环境值不执行正值校验，因此非正但可解析的文本与 `SetGOGC` 的规则不一致。
- `GOGC` 环境值只在第一次访问时采样，后续环境变更不生效，也没有重新初始化入口。
- 当前实现不会因 GC runtime 或指标系统失败，因为它根本没有调用这些系统；相应地，也不能声称设置已作用于真实 GC 或指标。

## 并发与资源生命周期

`LazyLock` 管理进程生命周期的静态对象，无显式分配、关闭或销毁流程。初始化只发生一次；多个线程并发首次访问时由 `LazyLock` 串行化初始化并发布同一个 `AtomicI32`。

所有读、写均使用 `Ordering::SeqCst`。每次 `SetGOGC` 是不可分割的交换，任一调用得到的旧值对应全局原子修改顺序中的直接前驱；`GetGOGC` 看到的是该顺序中某个已发布值，不会发生撕裂。此处没有锁、任务、线程、通道、事务或 I/O 资源生命周期。原子性不代表多步骤业务操作具有事务性：例如 gctuner 在写本缓存后再写自身 `gcPercent`，其他线程可能在两次写之间观察到两个模块暂时不一致。

环境变量通常不应在多线程运行期并发修改；本文件通过只在 `LazyLock` 初始化时读取一次来缩小读取窗口，但不提供环境更新同步协议。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/gogc.go`。两边共同保留了三个接口概念：初始化时读取 `GOGC` 且默认 `100`，`SetGOGC` 将非正输入归一为 `100`，`GetGOGC` 原子读取缓存。Rust 的 `swap` 返回旧缓存值，在仅由该模块控制状态时与 Go `debug.SetGCPercent` 返回旧运行时百分比的常见调用效果相近，现有 Rust 单测也按这一状态机验证。

但当前并非完整语义对齐：

- Go `init` 在包加载时自动执行，并立即调用 `metrics.GOGC.Set`；Rust `init` 只是未接线的普通函数，懒初始化也不更新指标。
- Go `SetGOGC` 调用 `runtime/debug.SetGCPercent`，更新 `metrics.GOGC`，再写 `gogcValue`；Rust 只交换缓存，不改变任何运行时行为或指标。
- Go 返回真实 runtime 先前值，Rust 返回本地缓存先前值；若未来存在其他运行时修改入口，两者可能分离。
- Go 缓存为 `int64`、函数参数/结果为 `int`，Rust 统一为 `i32`，环境解析范围更窄。
- 两边初始化都接受可解析的非正整数且不归一化，也都把 `GOGC=off` 当成整数解析失败而采用本文件默认值；这对应这里的缓存逻辑，不等同于 Go runtime 对环境变量的全部原生解释。

Go 树中没有检索到直接覆盖 `SetGOGC`/`GetGOGC` 的 `*_test.go`。Rust 的直接回归证据位于独立文件 `pkg/util/cpu_posix_1_aster_unit_test.rs`，符合测试不内嵌到生产源文件的仓库规则。

## 扩展指南

若只扩展缓存规则，应集中修改 `gogcValue` 初始化闭包或 `SetGOGC`，并在独立测试文件中补齐环境初始化、负值、最大值及并发交换行为。测试环境变量初始化时需要隔离进程或保证该 `LazyLock` 尚未被访问；不能依赖在同一进程中修改环境后重置静态状态。

若目标是完成 Go 语义移植，应在 `SetGOGC` 接入明确的运行时 GC 控制抽象和 GOGC 指标更新，并决定 `init` 的可靠启动接线位置；不要仅修改注释或让本地缓存看似成功。此变更会影响 `pkg/util/gctuner/tuner.rs::Tuner::setGCPercent` 的返回值语义和 `memory_limit_tuner.rs::tuning` 的决策输入，必须同步它们的独立测试。还应明确外部 runtime 修改是否可能发生，以及缓存与 runtime/指标更新失败时的提交顺序、回滚策略和可观测错误。

若扩大数值类型，需要同时检查环境解析、原子类型、gctuner 中 `u32` 转换以及 `(100 + GOGC)` 运算。兼容风险主要是返回旧值语义、非正初始化值和公开签名变化；正确性风险是缓存与真实 runtime 脱节；性能风险较低，当前热路径只有顺序一致原子操作，但新增指标或 runtime 桥接后应重新评估调用频率和同步成本。

新增测试应继续放在独立文件，优先扩展 `pkg/util/cpu_posix_1_aster_unit_test.rs`，或为职责清晰新建 `pkg/util/gogc_test.rs` 并在 `pkg/util/lib.rs` 的 `cfg(test)` 区域挂载，不能把测试写入 `gogc.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/util/gogc.rs` 核对了完整 58 行源文件及两个使用文件；`query SetGOGC/GetGOGC` 核对了 Rust/Go 同名定义；`callers`、`callees` 和精确 `explore` 核对调用点，并记录了 `callers --file` 对跨模块引用未产出边的限制。
- Rust 源码：`pkg/util/gogc.rs`（目标实现）、`pkg/util/lib.rs`（模块公开与测试挂载）、`pkg/util/gctuner/tuner.rs`（写调用者）、`pkg/util/gctuner/memory_limit_tuner.rs`（读调用者）、`pkg/util/cpu_posix_1_aster_unit_test.rs`（直接状态转换回归测试）。
- crate 配置：`pkg/util/Cargo.toml` 确认目标属于 `astersql-util` 且无专用外部依赖；`pkg/util/gctuner/Cargo.toml` 确认 `astersql-util-gctuner` 通过 `task-util` 路径依赖调用本模块。
- Go 对照：`pkg/util/gogc.go` 核对自动初始化、`runtime/debug.SetGCPercent`、指标更新和原子缓存；`pkg/util/gctuner/tuner.go` 核对调谐器对返回旧值的使用方式。对 `pkg/**/*_test.go` 的检索没有找到直接调用 `SetGOGC` 或 `GetGOGC` 的 Go 测试。
- 本任务只生成说明文档，按计划不运行 Cargo；最终以固定十一个二级标题的结构命令验证，并人工复核文档区分了当前缓存实现与尚未接线的 runtime/metrics 语义。

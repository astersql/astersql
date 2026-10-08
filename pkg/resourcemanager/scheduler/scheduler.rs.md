# `pkg/resourcemanager/scheduler/scheduler.rs` 逻辑说明

## 文件定位

该文件是 `astersql-resourcemanager-scheduler` crate 的调度协议定义层：它只定义调度决策 `Command` 和调度器抽象 `Scheduler`，不采样资源、不计算具体策略，也不直接调整池容量。`pkg/resourcemanager/scheduler/lib.rs` 将 `scheduler` 声明为公开模块并用 `pub use scheduler::*` 在 crate 根重导出这两个 API。

`pkg/resourcemanager/scheduler/Cargo.toml` 声明的 crate 名为 `astersql-resourcemanager-scheduler`，库入口是 `lib.rs`，移植元数据指向 Go 包 `pkg/resourcemanager/scheduler`。上层 `pkg/resourcemanager/Cargo.toml` 以本地路径依赖 `scheduler_dependency`引入该 crate。

## 核心职责

- 用 `Command::{Downclock, Hold, Overclock}` 表达“减少并发、保持不变、增加并发”三种纯决策结果。
- 用 `Scheduler::Tune` 统一不同调度策略的输入和输出：输入是组件类别 `util::Component` 与只读借用的 `dyn util::GoroutinePool`，输出是 `Command`。
- 在决策层和执行层之间建立窄边界：实现者只返回命令，上层资源管理器负责过滤命令并调用池的 `Tune`。

## 主要符号

- `pub enum Command`：公开、无负载的枚举。`Clone + Copy` 允许按值复制，`Debug` 支持调试输出，`PartialEq + Eq` 支持分支比较和测试断言。
  - `Command::Downclock`：建议将池并发度减少一级；实际下界由 `ResourceManager::schedulePool` 和 `ResourceManager::Exec` 处理。
  - `Command::Hold`：不改变并发度，也是无可执行决策或边界条件不允许调容时的中性结果。
  - `Command::Overclock`：建议将池并发度增加一级；执行层另行限制最大超频幅度。
- `pub trait Scheduler`：公开的策略抽象。该 trait 本身没有 `Send`/`Sync` 超 trait 约束；生产持有者 `ResourceManagerInner::scheduler` 才将对象限制为 `Box<dyn Scheduler + Send + Sync>`。
- `Scheduler::Tune(&self, component, pool) -> Command`：唯一的 trait 方法。它允许按 `Component` 区分调度策略，并通过 `GoroutinePool` 查询池状态；接口本身不要求实现者必须使用所有输入。

文件没有模块级常量、结构体、`impl`、自由函数或条件编译项；`#![allow(non_snake_case)]` 为保留 Go 风格的 `Tune` 命名放宽 lint。

## 执行流程

1. `ResourceManager::NewResourceManger` 在 `pkg/resourcemanager/rm.rs` 中构造默认 `CPUScheduler`，并以 `Box<dyn Scheduler + Send + Sync>` 放入调度器列表。
2. 周期调度循环进入 `ResourceManager::schedule`，遍历池容器；`DistTask` 在进入调度器之前被跳过。
3. `ResourceManager::schedulePool` 对每个调度器动态分派 `scheduler.Tune(pool.Component, pool.Pool.as_ref())`。`Hold` 会让链继续查询下一调度器，首个可执行的非 `Hold` 命令被返回。
4. 当前生产实现 `CPUScheduler::Tune` 位于 `pkg/resourcemanager/scheduler/cpu_scheduler.rs`：它先检查距上次调容的时间，然后将 CPU 采样映射为三种 `Command`。它当前忽略 `component` 参数。
5. `ResourceManager::Exec` 消费命令：`Hold` 立即返回；`Downclock`/`Overclock` 在冷却时间、容量下界和超频上界检查通过后，才调用 `GoroutinePool::Tune`。

因此，本文件定义的 `Tune` 是“决策”而不是“执行”；只调用 trait 方法不会直接修改池。

## 数据与状态

`Command` 只携带三值决策，没有目标并发数、原因码或采样数据。其变体声明顺序与 Go 的 `iota` 顺序一致，但 Rust 枚举未声明 `#[repr(...)]`，不应将其内存布局或整数转换视为稳定的序列化协议。

`Scheduler` 不规定实现者的内部状态。`&self` 表示一次决策不需要对调度器本体的独占可变借用；若新策略要维护历史样本，需在实现内自行使用适合的内部可变性和同步。池状态由 `GoroutinePool` 对象持有，组件归属由按值传入的 `Component` 表示。

## 依赖与调用关系

- 直接源码依赖：`use crate::util`；`Command` 无外部类型依赖，`Scheduler::Tune` 依赖 `util::Component` 和 `util::GoroutinePool`。`lib.rs` 通过 `#[path = "../util/util.rs"]` 将这些工具类型编入调度器 crate。
- 实现边：`pkg/resourcemanager/scheduler/cpu_scheduler.rs` 中的 `impl Scheduler for CPUScheduler` 将 trait 方法委托给固有方法 `CPUScheduler::Tune`，这是目前查到的唯一生产实现。
- 上游调用边：`pkg/resourcemanager/schedule.rs` 的 `ResourceManager::schedulePool -> Scheduler::Tune`。`pkg/resourcemanager/rm.rs` 存储 `Vec<Box<dyn Scheduler + Send + Sync>>` 并安装默认 CPU 调度器。
- 下游消费边：`ResourceManager::schedulePool -> Command`，然后 `ResourceManager::Exec -> GoroutinePool::Tune`。命令枚举也在调度链的单元测试中用来注入固定策略。
- crate 依赖：`pkg/resourcemanager/scheduler/Cargo.toml` 唯一生产依赖是路径 crate `astersql-util-cpu`（别名 `cpu_crate`），由相邻 CPU 实现使用；本文件本身不直接调用它。

RustCodeGraph 能定位本文件的 `Command` 和 `Scheduler`，但未返回 trait 动态分派的完整 callers/callees 边；上述调用边因此使用已索引源码与 `rg` 引用搜索交叉核验。

## 错误处理与边界

`Scheduler::Tune` 返回 `Command` 而不是 `Result`，协议中没有可传播的错误。当实现无法得出可执行决策时，现有 CPU 实现使用 `Hold` 表示安全降级，例如 CPU 采样不受支持或仍在最小调度间隔内。

协议本身不约束返回命令的合法性，边界由上层防护：无运行 worker 的池直接 `Hold`；容量为 1 或运行数超容量时不执行 `Downclock`；`Overclock` 不能超过原始并发度加 `MaxOverclockCount`。新实现不应假定自己的命令一定会被执行。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或资源，也没有自身的启停生命周期。`Tune` 对池使用共享借用；`GoroutinePool: Send + Sync` 保证池对象可被跨线程共享。调度器是由 `ResourceManager` 长期持有，并从其 100ms 后台循环调用，所以被安装的生产实现还必须满足持有点附加的 `Send + Sync`。

如果新调度器使用内部锁或原子状态，应保证 `Tune` 快速且不长时间阻塞，因为 `schedulePool` 按顺序调用调度器，一个慢实现会延迟同一调度循环中的后续池。这是由当前调用结构得出的性能约束，不是 trait 在类型系统中强制的保证。

## 与 Go 版本的对应关系

`pkg/resourcemanager/scheduler/scheduler.go` 是直接对照文件：Go 的 `type Command int` 及三个 `iota` 常量对应 Rust 的三变体枚举；Go 的 `Scheduler` interface 及 `Tune(component util.Component, p util.GoroutinePool) Command` 对应 Rust trait 的同名方法。

关键差异是：

- Go `Command` 是整数别名并有明确 `iota` 数值；Rust 使用类型安全的枚举，当前未声明 C 表示或序列化形式。
- Go interface 的池参数按 interface value 传递；Rust 显式借用 `&dyn GoroutinePool`，不在调用时转移池所有权。
- Go interface 本身不写并发 marker；Rust trait 本身同样没有 `Send + Sync` 超 trait，但生产持有点为后台调度显式追加这两个约束。
- `pkg/resourcemanager/scheduler/cpu_scheduler.go` 和 `.rs` 中的当前 CPU 实现都忽略 `component`，并使用相同的冷却、不支持状态及 0.5/0.7 阈值语义。

Go 目录下没有 scheduler 专属 `*_test.go`；`pkg/resourcemanager/schedule_test.go` 只通过 `scheduler.Overclock` 覆盖命令执行上限。Rust 已在独立的 `pkg/resourcemanager/scheduler/migration_aster_unit_test.rs` 中增加调度器协议和 CPU 决策覆盖，并在 `pkg/resourcemanager/migration_aster_unit_test.rs` 覆盖命令过滤与执行。

## 扩展指南

- 新增一种调度策略时，在独立生产文件实现 `Scheduler`，并在 `ResourceManager` 构造或注入处安装它；实现类型需满足生产持有点的 `Send + Sync`。
- 若只需新策略且三种命令足够，不应修改本协议；应仿照 `CPUScheduler` 单独测试边界、降级语义和 trait-object 分派。
- 新增 `Command` 变体时，必须同步处理 `pkg/resourcemanager/schedule.rs` 中 `schedulePool`/`Exec` 的过滤与穷尽匹配，同步更新 `pkg/resourcemanager/migration_aster_unit_test.rs`，并评估 Go 对照协议。
- 若要让命令携带目标容量或原因，需同时重设调度器对象、链式 `Hold` 语义、执行层和所有模式匹配；这是协议变更，不只是枚举增项。
- 若调度可失败，需明确是继续以 `Hold` 表示降级，还是将 `Tune` 改为 `Result<Command, E>`；后者会改变所有实现者和 `schedulePool` 调用者。
- 保持 Rust 生产逻辑和 Go 语义对齐，且将 Rust 测试逻辑放在独立测试文件；不要把 `#[cfg(test)] mod tests` 内嵌回该生产文件。

兼容风险主要是命令含义或枚举表示变更，正确性风险是新策略绕过冷却/容量边界或错误地将不可用数据当作扩容信号，性能风险是 `Tune` 在周期串行调用路径中阻塞。

## 验证依据

- 目标与模块：`pkg/resourcemanager/scheduler/scheduler.rs`、`pkg/resourcemanager/scheduler/lib.rs`、`pkg/resourcemanager/scheduler/Cargo.toml`、`pkg/resourcemanager/Cargo.toml`。
- 实现与主链：`pkg/resourcemanager/scheduler/cpu_scheduler.rs` 的 `impl Scheduler for CPUScheduler`，`pkg/resourcemanager/rm.rs` 的 `ResourceManagerInner::scheduler`/`NewResourceManger`，`pkg/resourcemanager/schedule.rs` 的 `schedule`/`schedulePool`/`Exec`，`pkg/resourcemanager/util/util.rs` 的 `Component`/`GoroutinePool`。
- Go 对照：`pkg/resourcemanager/scheduler/scheduler.go`、`pkg/resourcemanager/scheduler/cpu_scheduler.go`、`pkg/resourcemanager/schedule_test.go`。
- Rust 测试：`pkg/resourcemanager/scheduler/migration_aster_unit_test.rs` 覆盖命令阈值、调度冷却和 `dyn Scheduler` 分派；`pkg/resourcemanager/scheduler/tests/cpu_failpoint_integration.rs` 覆盖 CPU 不受支持时返回 `Hold`；`pkg/resourcemanager/migration_aster_unit_test.rs` 覆盖多调度器顺序、缩容防护、命令执行和 `DistTask` 跳过。
- RustCodeGraph：`status` 显示本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/resourcemanager/scheduler` 列出本 crate 的 7 个已索引源/测试文件；`query`/`node` 定位 `scheduler.rs::Command` 于第 28 行、`scheduler.rs::Scheduler` 于第 42 行；由于 trait 动态调用边未被图输出，又用 `rg` 核对了实现、持有点、调用点和测试引用。
- 结构验证使用任务指定命令，确认本文件存在且恰好包含 11 个固定二级标题。本任务为纯文档分析，按计划不运行 Cargo。

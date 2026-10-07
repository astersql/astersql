# `pkg/executor/sortexec/sort_spill.rs`

## 文件定位

本文件属于 `astersql-executor-sortexec` crate；crate 入口 `pkg/executor/sortexec/lib.rs` 以 `pub mod sort_spill` 暴露它。它位于排序执行器的内存配额响应层：把“内存 tracker 超限”转换为串行分区或并行排序 helper 的 spill 动作，但不实现行排序、磁盘 run 格式或最终多路归并，这些工作分别下沉到 `SortPartition`、`parallelSortSpillHelper` 和 `sort_util::DiskRun`。

当前 Rust 接线范围有限。仓库中的 Rust 生产文件没有构造 `sortPartitionSpillDiskAction` 或 `parallelSortSpillAction`；直接使用者是独立测试 `pkg/executor/sortexec/sort_spill_test.rs`。因此，本文件提供了公开动作类型和可执行语义，但尚不能据此断言 Rust `SortExec` 主链已经安装这些 OOM action。对应的 Go 文件 `pkg/executor/sortexec/sort_spill.go` 已接入 TiDB 的 `memory.ActionOnExceed` 模型。

## 核心职责

- 用 `SpillAction: Send + Sync` 统一表示可在线程间共享的超限动作；`Action(&self) -> Result<()>` 同时承载 spill、fallback 与错误传播。
- `sortPartitionSpillDiskAction` 对一个 `Arc<Mutex<SortPartition>>` 串行化访问：分区尚未达到 `spillTriggered` 时调用 `SortPartition::spillToDisk`，已经触发时才转交 fallback。
- `parallelSortSpillAction` 先检查传入的 `MemoryTracker` 是否超限，再以 helper 自身 tracker 的用量是否达到外部限制的十分之一作为“值得 spill”的门槛；达到门槛时尝试设置 spill 标志并执行 spill，否则调用 fallback。
- 将锁中毒转换为 `SortError`，避免 panic 穿透动作边界。

这里的“十分之一”判断来自 `parallelSortSpillAction::executeAction`：`helper.memoryTracker().bytes_consumed() >= tracker.bytes_limit() / 10`。它比较的是排序器自身用量与传入 tracker 的限制，而不是传入 tracker 的当前用量；后者仅由 `tracker.exceeded()` 决定动作是否启动。

## 主要符号

- `pub trait SpillAction: Send + Sync`：动作抽象。唯一方法 `fn Action(&self) -> Result<()>`；两个本地 action 和测试中的 `CountingFallback` 都实现它。
- `pub struct sortPartitionSpillDiskAction`：持有 `partition: Arc<Mutex<SortPartition>>` 与可选 `fallback: Option<Arc<dyn SpillAction>>`。字段私有，只能通过 `new` 创建。
- `sortPartitionSpillDiskAction::new(...) -> Self`：绑定分区和 fallback，不执行 I/O。
- `sortPartitionSpillDiskAction::executeAction(&self) -> Result<()>`：锁定分区，读取 `spillStatus`，选择 `spillToDisk`、fallback 或无操作成功。
- `impl SpillAction for sortPartitionSpillDiskAction`：`Action` 直接委托 `executeAction`。
- `pub struct parallelSortSpillAction`：持有 `helper: Arc<Mutex<parallelSortSpillHelper>>`、触发判断所用的 `tracker: Arc<MemoryTracker>` 与可选 fallback。
- `parallelSortSpillAction::new(...) -> Self`：绑定 helper、外部 tracker 和 fallback。
- `parallelSortSpillAction::executeAction(&self) -> Result<()>`：实现“外部 tracker 超限 → 锁 helper → 十分之一门槛 → 设置状态并 spill / fallback”的分支。
- `impl SpillAction for parallelSortSpillAction`：同样以委托方式提供统一入口。

文件没有模块级常量、枚举、条件编译项或后台任务定义。命名保留 Go 风格；`lib.rs` 的 crate 级 `allow(non_snake_case, non_camel_case_types)` 允许这些名称通过 Rust 风格检查。

## 执行流程

串行分区路径：

1. 调用者执行 `SpillAction::Action`，进入 `sortPartitionSpillDiskAction::executeAction`。
2. 对共享 `SortPartition` 获取互斥锁；锁中毒立即返回 `SortError("sort partition lock poisoned")`。
3. 若 `SortPartition::spillStatus()` 不等于 `spillTriggered`，在持锁期间调用 `spillToDisk()`。该下游方法排序内存行、写入 `DiskRun`、切换内存/磁盘 tracker，并最终把状态写为 `spillTriggered`。
4. 若状态已经是 `spillTriggered` 且存在 fallback，则在仍持有 partition 锁时调用 fallback；否则返回成功。

并行路径：

1. `parallelSortSpillAction::executeAction` 先调用外部 `tracker.exceeded()`；未超限直接返回，既不锁 helper，也不触发 fallback。
2. 读取 `tracker.bytes_limit()`，随后锁定 `parallelSortSpillHelper`；锁中毒返回 `SortError("parallel spill helper lock poisoned")`。
3. 用 helper 的 `memoryTracker().bytes_consumed()` 与 `limit / 10` 比较。达到门槛时调用 `setNeedSpill()`；只有原子状态成功从 `notSpilled` 迁移到 `needSpill` 时才立即调用 `helper.spill()`。
4. 数据达到门槛后，无论 `setNeedSpill()` 是否成功，都直接返回成功，不触发 fallback。数据不足门槛时，若存在 fallback 则调用它，并传播其错误；没有 fallback 则成功返回。
5. `parallelSortSpillHelper::spill` 会进入 `inSpilling`，归并 worker 数据并写 `DiskRun`，最后将状态恢复为 `notSpilled`；本文件不保留后台 spill 任务。

## 数据与状态

两个 action 自身都没有可变标量状态，所有共享可变状态位于 `Mutex` 后的目标对象中；action 可借助 `Arc` 被多处持有。fallback 也是 `Arc<dyn SpillAction>`，可形成动作链。

串行状态由 `SortPartition::spillStatus` 管理，相关常量定义在 `sort_util.rs`。本文件只区分“等于 `spillTriggered`”与“其他所有状态”；因此 `notSpilled`、`needSpill` 或 `inSpilling` 都会走 `spillToDisk()`。实际 `SortPartition::spillToDisk` 对已完成 spill 做幂等返回，并在正常、空数据错误或关闭分支后写入 `spillTriggered`。

并行状态由 `parallelSortSpillHelper::status: AtomicI32` 管理。`setNeedSpill` 使用 `compare_exchange(notSpilled, needSpill, AcqRel, Acquire)`，保证只有一个竞争者取得发起 spill 的资格。本文件在持有 helper 互斥锁时完成状态切换和同步 spill，进一步串行化 helper 的复合状态。

门槛计算存在边界语义：`bytes_limit()` 为 `i64`，直接使用整数除法 `limit / 10`。若限制小于 10，门槛可能为 0；若限制为负数，门槛也为负数，通常会使非负用量满足条件。不过动作仍必须先通过 `tracker.exceeded()`，实际调用方应避免把“无限制”的负限制 tracker 当作超限触发器。

## 依赖与调用关系

上游边界：`lib.rs` 公开 `sort_spill` 模块；RustCodeGraph 将 `SpillAction::Action` 指向两个 `executeAction` 委托，并把 `sort_spill_test.rs` 标为直接使用文件。全仓 Rust 搜索未发现测试之外对两个 action 构造函数的调用，因此生产接线当前未验证、实际上也未在 Rust 源码中出现。

下游关系：

- `sortPartitionSpillDiskAction::executeAction` → `SortPartition::spillStatus` → `SortPartition::spillToDisk`；后者再依赖比较器、`DiskRun`、内存/磁盘 tracker 和查询中断标志。
- `parallelSortSpillAction::executeAction` → `MemoryTracker::{exceeded, bytes_limit}` → `parallelSortSpillHelper::{memoryTracker, setNeedSpill, spill}`。
- 两条路径均可调用动态分派的 `SpillAction::Action` 作为 fallback。
- 错误类型与结果别名来自 `sort_util::{Result, SortError}`。

`pkg/executor/sortexec/Cargo.toml` 声明 crate 名为 `astersql-executor-sortexec`，`lib.rs` 为库入口，并以 `package.metadata.porting.go-package = "pkg/executor/sortexec"` 标注 Go 来源。其显式依赖目前都置于 `cfg(windows)` 表中；本文件的直接 Rust 依赖均来自本 crate 与标准库，没有新增外部 crate。

## 错误处理与边界

- 两个 `Mutex::lock` 失败分别映射为明确的 `SortError`；不会用 `unwrap` 造成二次 panic。
- 串行路径直接传播 `SortPartition::spillToDisk` 的排序、查询中断、空分区或 `DiskRun` 写入错误；fallback 错误也原样传播。
- 并行路径直接传播 `parallelSortSpillHelper::spill` 的 worker 锁、归并或磁盘 run 错误，并传播 fallback 错误。
- 外部 tracker 未超限时，并行动作是严格的成功空操作，即使 helper 中已有数据也不会 spill或 fallback。`parallel_action_does_not_fallback_below_limit` 固定了这一行为。
- tracker 已超限但排序数据不足 `limit / 10` 时，不尝试 spill而调用 fallback；`parallel_action_falls_back_when_sort_data_is_below_ten_percent` 固定了这一行为。
- 数据达到门槛但 `setNeedSpill()` 因状态不是 `notSpilled` 而失败时，当前实现既不执行 spill，也不 fallback；调用者只能得到成功。扩展状态机时必须保留或有意识地改变这一语义。
- 串行 fallback 在 partition 锁内执行；若 fallback 反向获取同一锁，会造成死锁。当前类型系统不阻止这种 fallback 组合。

## 并发与资源生命周期

`SpillAction` 要求 `Send + Sync`，而 action 内部通过 `Arc<Mutex<_>>` 共享目标。串行路径把状态检查、spill 调用或 fallback 调用包含在同一临界区；并行路径先无锁检查 tracker，再持 helper 锁进行用量检查、状态迁移、spill 或 fallback。tracker 自身负责并发安全，helper 的状态还使用原子内存序保证状态可见性。

Rust 实现是同步的：`SortPartition::spillToDisk` 和 `parallelSortSpillHelper::spill` 都在调用 `Action` 的线程上运行，且运行期间持有对应 mutex。文件不会启动线程、任务或通道。磁盘 run 的创建、关闭以及 tracker 用量释放由下游对象负责；action 不拥有显式 `close`。

这与 Go 的资源生命周期不同。Go 串行 action 用条件变量等待已有 spill、以 `sync.Once` 和 goroutine 异步发起一次 spill，并把错误存回 partition；Go 并行 action 只设置 `needSpill` 标志，实际 spill 由 executor 后续执行。Rust 当前同步实现可能增加 OOM 回调延迟和锁持有时间，扩展时应把它视为兼容性与性能风险，而不是假定两端并发行为等价。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/sortexec/sort_spill.go`。

- 两端都有 `sortPartitionSpillDiskAction` 和 `parallelSortSpillAction`，并保留“排序数据至少达到外部限制的十分之一才值得 spill”的判断。
- Go 类型嵌入 `memory.BaseOOMAction`、实现 `memory.ActionOnExceed`、提供 `GetPriority`，并由 tracker 传参给 `Action(t)`；Rust 改为本地 `SpillAction` trait，将 tracker 保存在并行 action 字段中，没有优先级 API。
- Go 串行 action 会等待正在进行的 spill，检查 `hasEnoughDataToSpill`，以 `sync.Once` 异步启动 spill，并在 tracker 不再超限时停止 fallback；Rust 不等待、不检查分区数据量和 tracker 是否仍超限，未完成 spill 时同步调用 `spillToDisk`，已完成后才 fallback。
- Go 并行 action 只原子地请求 spill并记录触发时的 bytes consumed/limit；Rust 在成功设置 `needSpill` 后立即同步执行 `helper.spill()`，且 helper 当前不保存这两个观测值。
- Go `Action` 在 `actionImpl` 未设置 spill 时，仅当 tracker 仍超限且数据不足门槛才 fallback；Rust 入口先检查超限，然后在数据不足时采用同一 fallback 条件。独立 Rust 测试覆盖了“未超限不 fallback”和“低于十分之一时 fallback”。
- Go 测试 `sort_spill_test.go` 还覆盖完整 executor 的内存排序、单/多分区 spill、手动触发、fallback 与临时文件泄漏；Rust 同名测试的大段 Go 草稿存于字符串中，真正编译执行的测试目前只直接覆盖并行 fallback 两个分支和 `DiskRun` 边界，未证明 action 已接入完整 Rust executor。

因此，Rust 文件是 Go 概念的局部移植，并非并发时序与接线层面的完整等价实现。

## 扩展指南

- 接入 Rust `SortExec` 时，应在 executor 初始化/关闭生命周期中明确安装和释放 action，并新增独立测试验证 tracker 超限确实通过生产入口到达 action；不要把测试写进 `sort_spill.rs`。
- 修改串行 spill 判定时，主要入口是 `sortPartitionSpillDiskAction::executeAction`，需要同步检查 `SortPartition::{spillStatus, spillToDisk}` 的状态机，并在 `sort_spill_test.rs` 增加未 spill、已 spill、空分区、锁中毒及 fallback 错误用例。
- 修改并行门槛或调度方式时，主要入口是 `parallelSortSpillAction::executeAction`，同时核对 `parallelSortSpillHelper::{setNeedSpill, spill, mergeAll}`。至少覆盖临界值（低于、等于、高于 `limit / 10`）、重复 action、已在 spilling 状态以及 spill 错误。
- 若追求 Go 行为等价，需要设计条件等待、单次异步触发、错误回存、priority 和触发时用量快照；这些不是本文件当前支持的能力。引入异步执行前应评估锁顺序、executor 谁负责执行/等待 spill、关闭时如何 join，以及 fallback 是否允许在锁外调用。
- fallback 的新增实现必须避免反向锁定当前 partition/helper；更安全的改动方向是先确定状态与动作，再释放锁后调用不需要受保护状态的 fallback，但这会改变当前原子性，需要并发回归测试。
- 性能方面重点观察 mutex 持有时长、同步磁盘 I/O 延迟、重复状态竞争及小 limit 导致的零门槛；兼容性方面重点保持 Go 的触发条件、错误可见性和完整排序结果。

## 验证依据

- 目标源码：`pkg/executor/sortexec/sort_spill.rs`，完整读取 105 行并核对 trait、两个结构体、构造函数、`executeAction` 与 trait impl。
- crate 边界：`pkg/executor/sortexec/Cargo.toml`、`pkg/executor/sortexec/lib.rs`；确认 crate 名、Go 包映射、公开模块与独立测试模块。
- RustCodeGraph：`status` 显示索引含目标目录；`files --filter pkg/executor/sortexec` 确认目标、Go 对照和测试；`node --file .../sort_spill.rs` 确认完整源码；`node executeAction --file .../sort_spill.rs` 给出两处同名定义及 `spillStatus`、`spillToDisk`、fallback 调用边；对 `spillToDisk`、`setNeedSpill`、`spill` 的 callers 查询用于核对下游关系。图对两个同名 `executeAction` 的 callers/callees 存在合并歧义，因此最终结论再由源码逐分支核验。
- 下游源码：通过 RustCodeGraph 读取 `pkg/executor/sortexec/sort_partition.rs` 与 `pkg/executor/sortexec/parallel_sort_spill_helper.rs`，核对状态机、锁、原子迁移、磁盘 run、tracker 与错误传播。
- Go 对照：`pkg/executor/sortexec/sort_spill.go`、`pkg/executor/sortexec/sort_spill_test.go`；核对 ActionOnExceed、条件变量、goroutine、十分之一门槛、fallback 和集成测试意图。
- Rust 独立测试：`pkg/executor/sortexec/sort_spill_test.rs`；有效 Rust 测试覆盖未超限不 fallback、低于门槛 fallback 和 `DiskRun` 关闭边界。全仓 Rust 搜索未发现测试之外构造本文件两个 action，支持“当前未接入生产 Rust 主链”的结论。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前使用任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工检查没有把 Go 草稿或预期架构写成 Rust 已实现事实。

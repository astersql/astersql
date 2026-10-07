# [`pkg/dxf/framework/proto/task.rs`](task.rs)

## 文件定位

`task.rs` 是 `astersql-dxf-framework-proto` crate 的任务协议定义文件。crate 入口 `pkg/dxf/framework/proto/lib.rs` 通过 `pub mod task` 声明模块并以 `pub use task::*` 重导出其公开项，因此调度器、任务执行器、存储转换层和 HTTP 管理入口可以共享同一组任务状态、任务数据结构及进程内调节参数。该 crate 的边界由 `pkg/dxf/framework/proto/Cargo.toml` 确认：库入口为 `lib.rs`，本文件直接使用的外部能力来自 `chrono` 与 `serde`；`serde_json` 主要供同 crate 测试验证协议形状。

从完整 DXF 看，`pkg/dxf/framework/doc.go` 将 task 定义为按顺序推进的多个 step，每个 step 内含并行 subtask。owner 节点负责调度，所有节点可运行 task executor。本文件只承载这条主链共享的数据和局部纯逻辑，不负责持久化、状态迁移执行、subtask 创建或具体业务 step 的运行。

## 核心职责

- 定义任务状态与模式协议：`TaskState`、`TaskType`、`PrepareMode` 及其常量和字符串转换扩展 trait。
- 定义轻量任务视图 `TaskBase` 和完整任务 `Task`。前者刻意不包含可能很大的 `Meta`，适合排序、调度和资源判断；后者补充调度标识、时间、元数据、错误和修改参数。
- 提供三组纯判断/派生逻辑：`TaskStateExt::CanMoveToModifying`、`TaskBase::IsDone`、`TaskBase::{Compare, CompareTask, GetRuntimeSlots, String}`。
- 管理两个 owner 本地、仅内存的原子旋钮：最大并发任务数 `maxConcurrentTask`，以及清理查询批量 `taskCleanupBatchSize`。
- 定义向后兼容的 `ExtraParams` JSON：默认值字段不序列化，未知字段的兼容行为由 Serde 默认处理；本文件明确保证空默认参数编码为 `{}`。

本文件不是完整状态机实现。`TaskState*` 常量描述可被其他模块持久化和判断的状态值，而实际状态转换发生在 scheduler/storage 等上层；`pkg/dxf/framework/doc.go` 是状态机意图的最近包级说明。

## 主要符号

- `TaskState = &'static str`、`TaskType = &'static str`：使用静态字符串别名与 Go 字符串类型保持值语义。`TaskStatePending` 至 `TaskStateModifying` 覆盖等待、运行、成功、失败、回滚、取消、暂停/恢复、人工处理和修改中状态。
- `PrepareMode = i32`：`PrepareModeDisabled = 0` 保持旧数据默认行为，`PrepareModeRequired = 1` 要求先进入 prepare-mode；`PrepareModeExt::String` 对未知整数保留 `unknown(n)`，避免把未来值误报成已知模式。
- `TaskTypeExt`、`TaskStateExt`、`PrepareModeExt`：模拟 Go 接收者方法。`TaskStateExt::CanMoveToModifying` 只允许 `pending`、`running`、`paused`。
- `TaskIDLabelName`、`NormalPriority`：分别固定标签键 `task_id` 和默认优先级 512；`TaskBase::Compare` 采用“数值越小排名越高”的约定。
- `GetMaxConcurrentTask` / `SetMaxConcurrentTask` / `SetMaxConcurrentTaskForTest`：读取、校验更新及测试期无校验替换/恢复最大任务并发。生产 setter 的合法区间是 `[16, 1000]`。
- `GetTaskCleanupBatchSize` / `SetTaskCleanupBatchSize` / `SetTaskCleanupBatchSizeForTest`：对应清理批量，生产 setter 的合法区间是 `[1, 1000]`，默认值 20。
- `ExtraParams`：包含 `ManualRecovery`、`PauseOnKVDiskFull`、`MaxRuntimeSlots`、`TargetSteps` 和 `PrepareMode`。所有零值/空值字段均通过 `skip_serializing_if` 省略。
- `TaskBase`：包含 ID、业务键、任务类型、状态、step、优先级、申请槽位、目标 scope、创建时间、节点数上限、扩展参数和 keyspace。源码中的真实字段顺序与名称应作为协议事实，不应仅依赖相邻注释的位置。
- `Task`：组合 `TaskBase` 与 `SchedulerID`、`StartTime`、`StateUpdateTime`、`Meta`、`Error`、`ModifyParam`。`Deref`/`DerefMut` 让 Rust 调用方像 Go 匿名嵌入字段一样访问 `TaskBase` 方法和字段。
- `EmptyMeta`：共享的空 JSON 字节串 `b"{}"`，代表 task/subtask 的空元数据。

## 执行流程

1. 调用方从 `Task` 或 `TaskBase` 读取持久化/构造出的任务协议字段。`Task` 的 `Deref` 投影允许直接调用 `IsDone`、`GetRuntimeSlots` 等基础方法。
2. 调度排序时，`CompareTask` 取出对方的 `TaskBase` 并委托 `Compare`。`Compare` 依次比较 `Priority`、`CreateTime`、`ID`；第一个非相等结果立即转换为 `-1` 或 `1`，完全相等才返回 `0`。这对应 `pkg/dxf/framework/doc.go` 的 `priority asc, create_time asc, id asc`。
3. 创建或运行 step executor 前，调用方应使用 `GetRuntimeSlots` 而不是直接把 `RequiredSlots` 当成有效并发。如果 `MaxRuntimeSlots <= 0`，返回申请值；若上限为正且 `TargetSteps` 为空，则所有 step 返回两者最小值；若列表非空，仅当前 `Step` 命中时应用最小值。
4. 管理入口调整进程内旋钮时，生产 setter 先做闭区间校验，失败则返回字符串错误且不写原子值，成功才以 `SeqCst` 存储。测试 setter 刻意跳过范围校验，返回一次性恢复闭包以隔离全局状态。
5. `ExtraParams` 序列化时省略 false、0 和空列表；反序列化缺失字段时依赖 `#[serde(default)]` 回到 Rust 默认值。因此新增字段必须保持默认值对旧 JSON 无行为变化。

## 数据与状态

任务状态是静态字符串而非封闭 enum：优点是与 Go/数据库字符串直接对应，代价是类型系统不能阻止未知状态进入。`IsDone` 只把 `succeed`、`reverted`、`failed` 视为终态；`awaiting-resolution`、`paused`、`cancelling` 等均不是终态。`failed` 表示框架无法运行任务，而正常运行中失败通常进入 `reverting` 后以 `reverted` 结束，这一语义来自 `pkg/dxf/framework/doc.go`。

`ExtraParams::ManualRecovery` 使运行失败后的任务可停在 `awaiting-resolution` 供人工处置；`PauseOnKVDiskFull` 表达磁盘满时暂停而非回滚的策略。二者在本文件中只保存协议值，不直接驱动状态迁移。`MaxRuntimeSlots` 是运行期降载上限，可能小于或大于后来被修改的 `RequiredSlots`；`GetRuntimeSlots` 始终取最小值，因此不会把有效槽位提高到申请值以上。`PrepareMode` 的零值必须继续表示 disabled，保证旧 JSON 和旧存量任务兼容。

`Task::Meta` 是可变字节载荷：通常只读，但源码明确列出切换 step、cleanup 脱敏和 modifying 三种可能更新场景。`Task::Error` 在 Rust 中简化为 `Option<String>`，而 Go 对照为 `error`；它保留错误文本而不保留 Go 动态错误类型、包装链或类型判断能力。`ModifyParam` 来自相邻 `modify` 模块，记录修改前状态和待应用修改。

## 依赖与调用关系

下游依赖均可在本文件符号处直接核验：`Step`/`Step2Str` 来自 `step.rs`，其中 `Step2Str` 被 `TaskBase::String` 用于展示业务 step；`ModifyParam` 来自 `modify.rs`；`chrono::DateTime<Utc>` 将 `SystemTime` 格式化为 RFC 3339；Serde 派生与字段属性定义 `ExtraParams` 的 JSON；标准库原子类型承载进程内旋钮。

RustCodeGraph 对当前索引给出的直接上游证据包括：

- `GetTaskCleanupBatchSize` 被 `pkg/dxf/framework/storage/task_table.rs::GetCleanupTasks`、`pkg/dxf/framework/scheduler/interface.rs::cleanup_tasks` 和 `pkg/server/handler/tikvhandler/dxf.rs::writeTaskCleanupBatchSize` 消费；`SetTaskCleanupBatchSize` 被同一 HTTP 处理模块的 `ServeHTTP` 调用。
- `SetTaskCleanupBatchSizeForTest` 被 scheduler、importinto cleanup 与 HTTP 集成测试用于临时修改全局值并恢复。
- `GetRuntimeSlots` 在本 proto 层的直接 Rust 调用主要由 `task_test.rs` 以及 importinto 调度桥接测试覆盖；另一个同名运行资源入口位于 `pkg/dxf/framework/taskexecutor/interface.rs`，其 `GetStepResource` 使用有效槽位。这说明调用时必须按具体 `TaskBase` 类型消歧，不能把所有同名图边混为同一实现。
- `GetMaxConcurrentTask`、`SetMaxConcurrentTask` 在当前 Rust 图中的直接调用以本文件和 `task_test.rs` 为主；Go 图则显示生产 HTTP 入口 `pkg/server/handler/tikvhandler/dxf.go::ServeHTTP`。因此不能仅据 Go 接线宣称 Rust setter 已被同等生产路径调用。

`pkg/dxf/framework/proto/lib.rs` 是公开出口，`pkg/dxf/framework/proto/Cargo.toml` 的 `package.metadata.porting.go-package` 明确指向 `pkg/dxf/framework/proto`，构成 Rust/Go 对照边界。

## 错误处理与边界

- 两个生产 setter 都遵循“先校验、后写入”：最大并发拒绝 `<16` 或 `>1000`，清理批量拒绝不在 `1..=1000` 的值。错误采用可读 `String`，不携带结构化错误种类。
- `SetMaxConcurrentTaskForTest` 与 `SetTaskCleanupBatchSizeForTest` 不校验输入，这是测试工具的刻意能力，不应暴露为生产配置入口。恢复闭包是 `FnOnce`，只能执行一次；嵌套调用的恢复顺序必须后进先出，否则会恢复到中间快照。
- `GetRuntimeSlots` 不验证负数 `RequiredSlots`，也不规范化 `TargetSteps` 的重复值；其职责仅是按现有字段计算。字段合法性应在任务创建/修改边界保证。
- `TaskBase::Compare` 对 `SystemTime` 使用全序比较，不发生时间差计算；相同优先级和时间最终由 ID 稳定破同序。
- `TaskBase::String` 是诊断摘要，不包含 `Meta`、`Error`、`ExtraParams`、keyspace 等字段，不可作为无损序列化格式。
- `TaskState`/`TaskType` 是字符串别名，扩展 trait 的 `String` 不做合法性检查。未知 `PrepareMode` 可安全显示，但不代表上层调度支持该值。

## 并发与资源生命周期

`maxConcurrentTask` 与 `taskCleanupBatchSize` 均为进程静态 `AtomicI64`，使用 `Ordering::SeqCst` 读写：并发线程会看到单一全序的更新，不需要锁，也不存在部分写入。它们只属于收到更新的进程/owner 节点，不写 TiKV，进程重启分别恢复默认值 16 和 20；owner 切换或多节点配置传播不由本文件解决。

提高最大并发会增加 owner 上同时运行 scheduler 的 CPU/内存开销，1000 上限是 scheduler 尚集中运行于 owner 时的保护。清理批量控制一次查询/搬移处理的任务数，过大可能提高单次数据库和内存压力。测试恢复闭包捕获设置前的原子快照；由于这是进程级共享状态，并行测试若同时修改同一旋钮仍可能互相观察到临时值，调用方应串行化或采用明确的隔离策略。

`Task`/`TaskBase` 自身没有内部锁、引用计数、异步任务或资源释放逻辑；`Meta`、字符串和向量遵循普通 Rust 所有权，在值 drop 时释放。`DerefMut` 允许持有 `&mut Task` 的调用方直接修改基础字段，但线程间共享与持久化一致性必须由上层容器/事务负责。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dxf/framework/proto/task.go`，Rust 基本逐项保留状态字符串、prepare mode、优先级、两个原子旋钮、`ExtraParams` JSON 名称、`TaskBase` 排名/槽位逻辑和 `Task` 字段。Rust static 初始化替代 Go `init()`；原子类型统一存储为 `i64`，公开并发接口转换为 `i32`，清理批量公开为 `i64`。

需要注意的语义/类型差异：

- Go 的 `TaskState`/`TaskType` 是自有字符串类型，Rust 是 `&'static str` 别名，只能直接表示静态字符串；方法由扩展 trait 提供。
- Go 的嵌入 `TaskBase` 由 Rust 命名字段加 `Deref`/`DerefMut` 模拟，结构布局和构造语法并不相同。
- Go `time.Time` 对应 Rust `SystemTime`；`String` 输出都采用 RFC 3339，但 Rust 使用 UTC 转换与 `SecondsFormat::AutoSi`。测试目前验证排序而未逐字验证跨语言时间格式。
- Go `Task.Error` 是 `error`，Rust 是 `Option<String>`，只对齐“可选错误信息”而非错误类型系统。
- Go 测试 `task_test.go` 与 Rust 独立测试 `task_test.rs` 共同覆盖 prepare mode 与 JSON、终态、并发边界、清理批量、排名和运行槽位。Rust 测试还使用极小/极大 `i64` 检查清理批量拒绝路径。

## 扩展指南

- 新增状态时：在本文件与 `task.go` 同步常量；判断它是否应进入 `IsDone` 或 `CanMoveToModifying`；检查 scheduler/storage 的状态集合与状态机；在独立的 `task_test.rs` 和 Go `task_test.go` 增加对应断言。不要把测试内嵌回生产文件。
- 新增 `ExtraParams` 字段时：提供能兼容旧载荷的默认值，明确 JSON 名称和 `skip_serializing_if`；检查持久化编码/解码、修改检测以及 Go struct tag；至少验证空对象仍为 `{}`、旧 JSON 可反序列化、新非默认值可往返。参与过滤或排序的字段不应放入 `ExtraParams`。
- 修改槽位策略时：以 `TaskBase::GetRuntimeSlots` 为单一协议入口，同步 `task_test.rs::test_task_base_get_runtime_slots`、Go 对照测试及 taskexecutor 的资源桥接测试；特别验证 `TargetSteps` 为空、命中、不命中，以及上限高于/低于 `RequiredSlots`。
- 修改排名时：同步 `Compare`、`CompareTask`、`pkg/dxf/framework/doc.go` 的 task rank 定义及两语言排序测试。排序是调度公平性/抢占顺序协议，改变字段顺序属于兼容性风险。
- 新增进程内旋钮时：同时定义默认值、上下界、生产校验 setter、测试恢复函数和生产消费入口；明确是否持久化、owner 切换行为及资源风险。若需要跨节点一致性，不应复用当前仅本地 atomic 的模式。
- 扩充 `Task` 字段时：核查 storage converter、数据库 schema、scheduler/taskexecutor 桥接和 Go 对照；大载荷仍应留在完整 `Task`，避免破坏 `TaskBase` 的轻量用途。

## 验证依据

- RustCodeGraph：`status` 显示本地索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/framework/proto` 确认 Rust/Go 源与独立测试均被索引。
- RustCodeGraph 源节点：`node --file pkg/dxf/framework/proto/task.rs --offset 1 --limit 500` 完整覆盖目标文件 1–439 行；另读取了索引中的 `task.go`、`task_test.rs`、`task_test.go` 和 `lib.rs`。
- RustCodeGraph 查询：`query TaskBase --kind struct`、`query SetMaxConcurrentTask --kind function`、`query TaskStateExt --kind trait` 用文件路径消歧目标符号；精确 explore 查询核对了 `GetTaskCleanupBatchSize`、`SetTaskCleanupBatchSize`、`GetRuntimeSlots`、`Compare` 等调用边。图中同名符号较多，因此本说明只采纳路径明确的边，并明确标识 Go 与 Rust 接线差异。
- 配置与包级证据：`pkg/dxf/framework/proto/Cargo.toml`、`pkg/dxf/framework/proto/lib.rs`、`pkg/dxf/framework/doc.go`。
- 对照与测试证据：`pkg/dxf/framework/proto/task.go`、`pkg/dxf/framework/proto/task_test.go`、`pkg/dxf/framework/proto/task_test.rs`。
- 人工复核结论：该文件存在是为了给 DXF 调度、执行、存储和管理入口提供共享任务协议与无副作用的派生逻辑；安全扩展的关键是同时维护状态机/排序契约、JSON 默认兼容、Go 对照、独立测试，以及 owner 本地原子配置的边界和生命周期说明。

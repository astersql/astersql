# `pkg/dxf/framework/taskexecutor/execute/interface.rs`

## 文件定位

本文件是 `astersql-dxf-framework-taskexecutor-execute` crate 的主体实现，经同目录 `lib.rs` 的 `pub use interface::*` 对外导出。它对齐 Go 包 `pkg/dxf/framework/taskexecutor/execute`，定义业务步骤执行器契约、运行时摘要、指标收集接口，以及由框架注入的 step、资源、计量和 checkpoint 信息。

这里的 `execute::StepExecutor` 是 IMPORT INTO 等业务步骤使用的应用侧接口；仓库另有 `pkg/dxf/framework/taskexecutor/interface.rs` 中供节点任务循环使用的同名 trait。`pkg/dxf/importinto/task_executor.rs` 等适配代码把前者包装到后者的生命周期中，不能仅凭同名认为两者是同一个接口。

crate 边界由 `pkg/dxf/framework/taskexecutor/execute/Cargo.toml` 给出：运行时依赖 `anyhow`、`metering`、`proto`、`recording` 和 `tokio-util`，`http` 仅用于测试；`package.metadata.porting.go-package` 明确指向同路径 Go 包。

## 核心职责

1. `StepExecutor` 规定一个 task step 的 `Init`、逐 subtask 的 `RunSubtask`、摘要读取/重置、`Cleanup`、任务元数据变更和资源变更回调，并要求实现者同时提供 `StepExecFrameworkInfo`。
2. `SubtaskSummary` 原子累计行数、处理量、读取字节和对象存储请求数，通过最多五个 `Progress` 点计算平滑速度。
3. `Collector` 把“已接收字节”和“已处理单位/行数”抽象成统一上报入口；`NoopCollector` 丢弃指标，`TestCollector` 用于原子计数验证。
4. `FrameworkInfo` 保存框架拥有的 step、资源、meter recorder 和 checkpoint 回调；`SetFrameworkInfo` 在业务执行器启动前显式注入这些状态。

本文件只定义契约和轻量状态逻辑，不负责领取 subtask、更新任务表或决定重试。节点级主循环与失败状态转换位于 `pkg/dxf/framework/taskexecutor/task_executor.rs`，具体业务执行位于 `pkg/dxf/importinto/*` 等调用方。

## 主要符号

- `Context = CancellationToken`：跨回调传播取消信号。它对应 Go `context.Context` 的取消用途，但不携带 Go context 的任意值。
- `StepExecutor: StepExecFrameworkInfo`：公开 trait。`Init`/`RunSubtask`/`Cleanup` 等返回 `anyhow::Result<()>`；`RealtimeSummary` 返回可选借用；`SetFrameworkInfo` 是 Rust 为替代 Go embedding/反射而增加的显式注入点。
- `UpdateSubtaskSummaryInterval`：3 秒；`maxProgressInSummary`：5；`SubtaskSpeedUpdateInterval`：15 秒。后两者共同定义摘要窗口上限和展示速度的平滑周期。
- `Progress`：一次采样的 `RowCnt`、`Processed` 与 `UpdateTime`。`Processed` 是通用进度单位；Go 持久化兼容字段名仍为 `bytes`，Rust 结构本身未在这里实现 JSON 序列化。
- `SubtaskSummary`：五个原子计数器和一个 `Vec<Progress>`；实现 `MergeObjStoreRequests`、`Update`、`GetSpeedInTimeRange`、`UpdateTime`、`Reset`。
- `Collector`、`NoopCollector`、`TestCollector`：指标入口及两种实现。`TestCollector.NoopCollector` 字段保留 Go 嵌入结构的形状，但 trait 方法由 `TestCollector` 自己实现。
- `CheckpointUpdateFunc`：共享的线程安全回调，参数为 context、subtask ID 和 `Box<dyn Any + Send + Sync>`；`CheckpointGetFunc` 返回字符串 checkpoint。
- `StepExecFrameworkInfo`：框架状态访问 trait；私有风格的 `restricted` 方法体现 Go 防止外包误实现的意图，但 Rust trait 本身是公开的，实现者仍须显式实现该方法。
- `stepExecFrameworkInfoName`：保留 Go 反射类型名 `"StepExecFrameworkInfo"` 的对照常量；Rust 注入不依赖反射。
- `FrameworkInfo`：保存不可变 `step`、共享 `meter_recorder`、`RwLock<Option<Arc<StepResource>>>` 和两个可选回调。`new` 为私有构造函数。
- `SetFrameworkInfo`：若 executor 为 `None` 直接返回；否则注册 recorder、构造 `FrameworkInfo` 并调用 trait 方法注入。

## 执行流程

框架信息注入流程如下：调用方取得 `Task` 和当前 `StepResource`，调用 `SetFrameworkInfo`；函数先处理 `None` no-op，再由 `FrameworkInfo::new` 从 `task.TaskBase.Step` 取 step、调用 `metering::RegisterRecorder(&task.TaskBase)`，保存资源和 checkpoint 回调，最后分派给具体执行器的 `StepExecutor::SetFrameworkInfo`。RustCodeGraph 显示真实调用位于 `pkg/dxf/importinto/task_executor.rs`、`subtask_executor.rs`、`conflict_resolution.rs` 及其测试中。

业务生命周期由 trait 契约表达：先 `Init`，再对 step 的每个 subtask 调用 `RunSubtask`，结束时 `Cleanup`。节点级适配器会在运行 subtask 前检查是否存在 `RealtimeSummary`、调用 `ResetSummary`，并启动周期摘要线程；任务 `Meta` 变化时调用 `TaskMetaModified`，运行中资源变化时调用 `ResourceModified`。这些调用事实可在 `pkg/dxf/framework/taskexecutor/task_executor.rs` 的 `runSubtask`、`detectAndHandleParamModify` 和 `tryModifyTaskRequiredSlots` 中核对。

摘要流程是：业务 collector 原子增加计数；持久化/读取实时摘要前调用 `Update` 写入当前快照；超过五点时删除最旧点。`GetSpeedInTimeRange(end, duration)` 把每对相邻采样视为一个线性处理区间，只按其与查询窗口的时间重叠比例累计 processed 差值，最后除以完整查询时长并截断为 `i64`。`pkg/dxf/importinto/job.rs` 使用该函数和 15 秒窗口生成作业速度。

`Reset` 先把所有计数器清零、清空历史，再立即 `Update`，因此重置后不是空历史，而是含一个零值基线点。

## 数据与状态

`SubtaskSummary` 的计数使用 `AtomicI64`/`AtomicU64` 且统一采用 `Ordering::SeqCst`，适合 collector 与读取方跨线程更新。`Progresses` 是普通 `Vec`，只有持有 `&mut SubtaskSummary` 才能调用 `Update`/`Reset`；它没有内部锁，调用方必须通过执行器所有权或外部同步保证独占修改。

速度计算假设采样按时间顺序排列。相邻 `Processed` 差值使用 `wrapping_sub`，明确复刻 Go `int64` 二补码回绕行为。若样本不足两个、duration 为零、`end_time - duration` 下溢，或查询窗口与样本范围不相交，返回 0。重复/逆序时间点不会 panic：`duration_since` 失败转为零，零长度分段被忽略；但这类输入不代表正常采样顺序。

`FrameworkInfo.resource` 以 `RwLock<Option<Arc<StepResource>>>` 保存，读者取得共享快照，写者原子地替换整个 `Arc`。锁中毒时通过 `into_inner()` 恢复数据而不是传播 panic。`step`、recorder 和 checkpoint 回调在构造后不变；回调用 `Arc` 克隆共享。

## 依赖与调用关系

上游关系：

- `pkg/dxf/importinto/task_executor.rs` 的编码排序、合并排序、写入摄取等执行器实现本 trait，使用资源、meter 和 checkpoint 信息，并在 `RealtimeSummary` 中采样。
- `pkg/dxf/importinto/subtask_executor.rs` 与 `pkg/dxf/importinto/conflict_resolution.rs` 也实现/转发框架信息；冲突处理会调用 `MergeObjStoreRequests` 合并真实访问统计。
- `pkg/dxf/framework/mock/execute/execute_mock.rs` 提供同一契约的 mock；相应 migration 测试验证框架信息可被 mock。
- `pkg/dxf/importinto/job.rs` 消费已持久化摘要，以 `GetSpeedInTimeRange` 计算运行速度。

下游依赖：

- `proto::task::Task` 提供 step 和 recorder 注册所需 `TaskBase`；`proto::subtask::{Subtask, StepResource}` 定义运行对象和资源预算。
- `metering::RegisterRecorder` 在注入时建立任务级计量 recorder。
- `recording::Requests::snapshot` 提供对象存储 GET/PUT 请求快照；合并后使用原子加法累计。
- `tokio_util::sync::CancellationToken` 提供 `Context` 的取消传播。
- `anyhow::Result` 统一 trait 生命周期和 checkpoint 回调的错误返回。

RustCodeGraph 对 `SetFrameworkInfo` 的调用分析确认直接生产调用集中在 IMPORT INTO 适配路径；对 `GetSpeedInTimeRange` 的分析确认除函数自身外由独立单元测试覆盖，并由作业展示路径经摘要对象消费。通用名称的图查询会混入仓库其他 `Update`/`Reset`，因此本说明只采用文件限定结果和同目录/直接调用文件证据。

## 错误处理与边界

`StepExecutor` 不规定具体错误类型分类，所有业务回调通过 `anyhow::Result` 向框架返回；是否重试、是否标记 subtask 失败由外层 task executor 决定。Go 注释强调 `Cleanup` 错误只记录日志、不改变 task/subtask 状态，以及 `TaskMetaModified` 失败可能导致重建执行器；Rust trait 签名保留这些错误通道，但策略在外层实现。

`SetFrameworkInfo(None, ...)` 是有意的 no-op。传入实际 executor 时，`FrameworkInfo::new` 总会创建 recorder，资源初始为 `Some`；只有 trait 类型允许 `GetResource`/recorder/回调返回 `None`，以便未注入状态或 mock 表达缺失值。具体执行器若在注入前对内部 `Option<FrameworkInfo>` 做 `unwrap`，会 panic，因此必须保持“先注入、后 Init/运行”的框架顺序。

`GetSpeedInTimeRange` 对无效或不可表示的时间窗返回 0，不报告错误；浮点比例最后用 Rust `as i64` 截断。`UpdateTime` 无样本时返回 `SystemTime::UNIX_EPOCH`，对应 Go 的零时间哨兵语义，但表示值并非 Go `time.Time{}` 的年份。

checkpoint 更新值以 `Any` 传递，类型契约由生产者与消费者共同约定；错误的 downcast 是回调实现自己的失败/panic 风险，本文件不做运行时 schema 校验。

## 并发与资源生命周期

计数器可被多个工作线程并发增加；`SeqCst` 提供最强顺序保证。历史采样需要 `&mut self`，避免本类型内部出现并发修改 `Vec`。对象存储请求先由 `recording::Requests::snapshot` 读取，再累计进摘要，因此是调用时刻的快照而非持续绑定。

`FrameworkInfo` 通过 `Arc` 分享 resource、recorder 和回调。资源更新替换 `RwLock` 内的 `Arc`，已取得旧 `Arc` 的执行逻辑仍可短暂持有旧快照；实现 `ResourceModified` 时必须在返回前让实际资源使用符合新预算，尤其是缩容/OOM 边界。外层 `tryModifyTaskRequiredSlots` 明确区分缩容和扩容顺序。

`Context` 可克隆并派生取消传播；`StepExecutor` 实现应在长循环或阻塞操作中观察取消。`Cleanup` 是释放 step 级资源的最终回调，但外层可能忽略其错误，因此关键一致性操作不应只依赖 Cleanup 成功。

`meter_recorder` 与 checkpoint 回调随 `FrameworkInfo` 及其克隆存活；本文件没有后台线程或显式 `Drop`。实际摘要轮询线程、subtask monitor 的创建、取消与 join 由外层 task executor 管理。

## 与 Go 版本的对应关系

Rust 的 `StepExecutor`、常量、`Progress`、`SubtaskSummary`、collector 和框架信息访问器逐项对齐 `interface.go`。主要有以下语言适配：

- Go 用嵌入 `StepExecFrameworkInfo` 和反射字段名完成注入；Rust 用 supertrait 加 `SetFrameworkInfo(FrameworkInfo)` 显式注入，因此 `stepExecFrameworkInfoName` 只保留命名对照，不参与运行时反射。
- Go `context.Context` 映射为 `CancellationToken`；当前覆盖取消传播，不承诺 deadline/value 全语义。
- Go `atomic.Pointer[StepResource]` 映射为 `RwLock<Option<Arc<StepResource>>>`；Go accessor 返回指针，Rust accessor 返回可选共享所有权。
- Go 原子字段映射为标准库原子类型，Rust 使用 `SeqCst`。Go `Progresses` 切片和 Rust `Vec` 都只保留最近五点。
- Rust 额外处理零 duration、`SystemTime::checked_sub` 下溢、逆序时长和调试构建整数溢出；正常输入下的速度结果由两边相同表驱动用例验证。
- Go `time.Time{}` 的无样本返回在 Rust 中用 `UNIX_EPOCH` 作为可用哨兵。
- Go 结构上的 JSON tag（特别是历史 `bytes` 字段名）没有在本文件的 Rust derive 中直接表达；序列化兼容由使用方/适配层负责，不能从本文件推断原生 serde 支持。

对照测试包括 Go `interface_test.go` 和 Rust `interface_test.rs` 的七类速度窗口；Rust `migration_aster_unit_test.rs` 进一步覆盖整数回绕、五点窗口、Reset 基线、GET/HEAD/PUT 合并、collector 计数、完整框架信息注入和 `None` no-op。

## 扩展指南

新增生命周期能力时，应同时修改 `StepExecutor`、所有生产实现和 `pkg/dxf/framework/mock/execute/execute_mock.rs`，并确认节点级适配 trait 是否也需要桥接；不要只给 trait 添加默认桩而掩盖 Go 行为。对应独立测试应放在 `interface_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产文件。

新增摘要字段时，需要同步：原子更新入口、`Update`/`Reset`、持久化/JSON 适配、Go `SubtaskSummary` 字段及兼容标签、任务表摘要转换和展示消费者。变更 `Processed` 单位必须保持每个 step 的含义明确，避免把字节/行/KV 数直接混合计算。

修改速度算法时，应保留分段重叠、查询窗口分母、最多五点和 Go `int64` 回绕语义，并在 Rust/Go 表驱动测试中加入相同边界：零时长、无重叠、边界恰好相等、重复时间戳、计数回绕或回退。

新增框架状态时，优先扩展 `FrameworkInfo` 与 `StepExecFrameworkInfo` accessor，再由 `SetFrameworkInfo` 一次性注入；评估共享所有权、锁粒度、旧资源快照存活和回调 `Send + Sync + 'static` 约束。checkpoint 值若增加新类型，应在调用层给出可验证的类型约定，避免裸 `Any` 失配。

兼容风险主要是 Go/Rust 接口漂移和持久化摘要字段漂移；性能风险主要是热路径使用 `SeqCst`、频繁克隆 `Arc`，或扩大 `Progresses` 导致每次速度计算线性增长。当前窗口固定为 5，计算成本有界。

## 验证依据

- 源码全貌：`pkg/dxf/framework/taskexecutor/execute/interface.rs`（328 行），核对全部 trait、类型、常量、impl、公开函数及无条件编译项。
- crate/模块：`pkg/dxf/framework/taskexecutor/execute/Cargo.toml`、`pkg/dxf/framework/taskexecutor/execute/lib.rs`。
- Go 对照：`pkg/dxf/framework/taskexecutor/execute/interface.go`、`pkg/dxf/framework/taskexecutor/execute/interface_test.go`。
- Rust 独立测试：`pkg/dxf/framework/taskexecutor/execute/interface_test.rs`、`pkg/dxf/framework/taskexecutor/execute/migration_aster_unit_test.rs`。
- 直接生产证据：`pkg/dxf/framework/taskexecutor/task_executor.rs` 的 executor 生命周期、摘要循环与参数变更回调；`pkg/dxf/importinto/task_executor.rs`、`subtask_executor.rs`、`conflict_resolution.rs` 的 trait 实现和注入；`pkg/dxf/importinto/job.rs` 的速度消费。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点和 1,848,419 条边；`files --filter pkg/dxf/framework/taskexecutor/execute` 确认六个 Go/Rust 源与测试文件；文件限定 `node` 读取目标 1–328 行；`explore` 确认 `SetFrameworkInfo`、`RealtimeSummary`、`GetSpeedInTimeRange`、`MergeObjStoreRequests` 的直接调用/测试边。
- 人工复核结论：本文件存在是为了稳定业务 step 执行契约并承载可持久化实时摘要；实际运行由外层节点 executor 编排，安全扩展必须同步 Go 对照、所有实现、mock、独立测试和序列化消费者。

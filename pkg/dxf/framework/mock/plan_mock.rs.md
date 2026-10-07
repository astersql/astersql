# `pkg/dxf/framework/mock/plan_mock.rs`

## 文件定位

本文对应源码为 [`plan_mock.rs`](./plan_mock.rs)。该文件属于 `astersql-dxf-framework-mock` crate。crate 根 `pkg/dxf/framework/mock/lib.rs` 以私有模块 `mod plan_mock` 装载它，再通过 `pub use plan_mock::*` 向测试调用方公开其中的 mock 类型和构造函数。`pkg/dxf/framework/mock/Cargo.toml` 表明该 crate 直接依赖 planner、proto、storage、taskexecutor/execute 和 sessionctx；本文件实际只使用 planner、proto 以及 crate 根定义的 `Handler`。

它是 `pkg/dxf/framework/planner/plan.rs` 中 `LogicalPlan` 与 `PipelineSpec` 两个 trait 的测试替身边界，不参与生产规划算法本身。RustCodeGraph 的文件查询显示，目标文件的直接 Rust 使用者是同 crate 的 `pkg/dxf/framework/mock/migration_aster_unit_test.rs`；仓库搜索还表明 planner 的 Rust 测试目前主要使用手写 trait 实现，而 Go 测试直接使用同路径生成的 GoMock。

## 核心职责

文件提供两组可配置 mock：

- `MockLogicalPlan` 覆盖逻辑计划的四个生命周期操作：读取额外任务参数、任务 meta 编码、任务 meta 解码，以及由 `PlanCtx` 构造 `PhysicalPlan`。
- `MockPipelineSpec` 覆盖流水线规格的唯一操作：由 `PlanCtx` 生成单个 subtask meta。

每个 mock 方法都把参数原样交给对应 `Handler` 中安装的闭包，并把闭包结果原样返回。这样测试可以在不构造真实计划实现的前提下观察 planner 与计划接口的交互。`EXPECT`、`ISGOMOCK`、大写方法名和 `NewMock*` 构造器保留 GoMock 风格的外观，但实际期望注册与调用计数由 Rust 的 `Handler` 完成，并非完整复刻 `gomock.Controller`。

## 主要符号

- `type PlannerResult<T> = Result<T, planner::PlannerError>`：统一四类可失败回调的返回类型；`PlannerError` 在 `pkg/dxf/framework/planner/plan.rs` 中是 `storage::Error` 的别名。
- `MockLogicalPlan`：公开结构体，含 `FromTaskMeta`、`GetTaskExtraParams`、`ToPhysicalPlan`、`ToTaskMeta` 四个公开 `Handler` 字段，调用方通过字段的 `set` 安装闭包并通过 `call_count` 观察调用次数。
- `MockLogicalPlanMockRecorder`：`MockLogicalPlan` 的类型别名。它使 `EXPECT()` 的返回类型沿用 GoMock 命名，但没有单独 recorder 状态。
- `MockLogicalPlan::{EXPECT, ISGOMOCK, FromTaskMeta, GetTaskExtraParams, ToPhysicalPlan, ToTaskMeta}`：兼容外观与实际派发入口。四个业务方法分别传入稳定的方法名字符串，供缺失期望时的 panic 消息使用。
- `impl planner::LogicalPlan for MockLogicalPlan`：把 snake_case trait 方法转发到上述 Go 风格方法。其中 `from_task_meta(&[u8])` 会复制成 `Vec<u8>` 后交给回调，其余返回值不转换。
- `NewMockLogicalPlan<C: ?Sized>`：接受任意 controller 引用以保持构造形状，忽略参数并返回全部 Handler 均未设置的默认实例。
- `MockPipelineSpec`：公开结构体，仅含 `ToSubtaskMeta: Handler<dyn FnMut(planner::PlanCtx) -> PlannerResult<Vec<u8>> + Send>`。
- `MockPipelineSpecMockRecorder`、`MockPipelineSpec::{EXPECT, ISGOMOCK, ToSubtaskMeta}`：分别提供 recorder 名称兼容、标记占位与回调派发。
- `impl planner::PipelineSpec for MockPipelineSpec`：将 trait 的 `to_subtask_meta` 转发到 `ToSubtaskMeta`。
- `NewMockPipelineSpec<C: ?Sized>`：忽略 controller，创建空期望实例。

文件没有模块级常量、枚举、条件编译分支或后台任务。

## 执行流程

以逻辑计划调用为例，流程如下：

1. 测试调用 `NewMockLogicalPlan(&controller_like_value)` 得到默认实例；传入值不被保存。
2. 测试在目标字段上调用 `set(Box::new(...))`，例如为 `ToTaskMeta` 安装返回 meta 的闭包。
3. planner 通过 `&dyn LogicalPlan` 调用 snake_case trait 方法。`Planner::run`/`run_with_target_scope` 会先调用 `to_task_meta`，随后 `create` 调用 `get_task_extra_params` 并把结果交给 `TaskCreator`（`pkg/dxf/framework/planner/planner.rs`）。
4. trait 实现转到同名大写方法，大写方法调用相应 `Handler::invoke`；`invoke` 暂时取出闭包、增加调用次数、执行闭包，再在未被回调替换时放回闭包。
5. 回调返回的 `Ok`/`Err`、`ExtraParams`、`PhysicalPlan` 或字节向量直接返回上游。

物理计划路径中，`PhysicalPlan::to_subtask_metas` 按 step 过滤 processor，按插入顺序对匹配 processor 的 `PipelineSpec::to_subtask_meta` 传入克隆的 `PlanCtx`。`MockPipelineSpec` 再将该上下文原样交给 `ToSubtaskMeta` Handler。`plan_mock_forwards_plan_context_to_pipeline_handler` 通过把 `task_key` 编成字节验证了这条转发链。

## 数据与状态

mock 自身不维护业务计划状态；可变状态集中在每个 `Handler` 内：一个受 `Mutex` 保护的可选 `FnMut` 闭包，以及一个 `AtomicUsize` 调用计数。`#[derive(Default)]` 递归创建“无闭包、计数为零”的 Handler，因此构造后的任何业务方法都必须先安装期望。

`MockLogicalPlan::from_task_meta` 的 trait 输入是借用切片，但公开兼容方法及 Handler 使用拥有所有权的 `Vec<u8>`；这会产生一次完整复制，使回调可安全持有或修改自己的副本。`PlanCtx` 和 `PhysicalPlan` 则按值进入/离开 Handler；`ExtraParams` 与 meta 字节同样按值返回。类型中没有缓存、全局变量、文件句柄或网络资源。

## 依赖与调用关系

上游边界包括：

- `pkg/dxf/framework/mock/lib.rs` 装载并重导出本文件符号，同时定义 `Handler`。
- `pkg/dxf/framework/mock/migration_aster_unit_test.rs::plan_mock_forwards_plan_context_to_pipeline_handler` 构造 `MockPipelineSpec`、安装回调并直接调用兼容方法。
- 真实接口消费者位于 `pkg/dxf/framework/planner/planner.rs` 和 `pkg/dxf/framework/planner/plan.rs`：前者通过 `LogicalPlan` 获取任务 meta 与 `ExtraParams`，后者通过 `PipelineSpec` 生成 subtask meta。
- Go 侧 `pkg/dxf/framework/planner/planner_test.go` 使用 `MockLogicalPlan` 验证任务创建参数，`pkg/dxf/framework/planner/plan_test.go` 使用 `MockPipelineSpec` 验证物理计划生成 meta。

下游依赖包括：

- `astersql_dxf_framework_planner`：提供 `LogicalPlan`、`PipelineSpec`、`PlanCtx`、`PhysicalPlan` 和 `PlannerError`。
- `astersql_dxf_framework_proto`：提供 `ExtraParams`。
- crate 根 `Handler`：负责闭包安装、同步、调用计数、缺失期望检测和实际派发。

RustCodeGraph 对目标文件的索引列出 `EXPECT`、`ISGOMOCK`、四个逻辑计划方法、`ToSubtaskMeta` 和两个构造器，并把 `migration_aster_unit_test.rs` 标为文件级使用者。由于常见方法名会产生跨仓库同名候选，本文的调用关系仅采用目标文件节点、明确的模块引用和上述直接源码证据。

## 错误处理与边界

四个返回 `PlannerResult` 的方法不解释或包装错误，Handler 返回的 `planner::PlannerError` 原样传播；`GetTaskExtraParams` 是不可失败接口。`LogicalPlan::from_task_meta` 只负责复制字节，不负责格式校验，格式错误是否发生完全取决于测试安装的闭包。

若某方法尚未 `set` 就被调用，`Handler::invoke` 会 panic，消息形如 `mock method MockLogicalPlan.ToTaskMeta called without an expectation`。若 Handler 的 mutex 中毒，锁操作也会 panic。这里没有 GoMock 的参数 matcher、调用顺序、次数上下限、测试结束自动校验或 controller 报错机制；调用者只能通过闭包逻辑与 `call_count()` 自行断言。因此 `EXPECT()` 仅返回自身，不代表已注册或会在析构时校验期望；`ISGOMOCK()` 也只是无返回值标记。

`NewMock*` 的泛型 controller 参数可以是动态大小类型引用，但完全忽略该对象；依赖 controller 生命周期、`Finish()` 或测试框架 helper 标记的 Go 代码不能假定这些能力已迁移。

## 并发与资源生命周期

两个 mock 没有线程、异步任务、通道、事务或显式清理动作。Handler 的闭包要求 `Send`，闭包槽由 `Mutex` 保护，调用计数使用 `SeqCst` 原子操作。`Handler::invoke` 在运行用户闭包时不持有 mutex：它先 `take` 闭包，执行完成后再加锁恢复；如果回调期间通过 `set` 安装了新闭包，则保留新闭包。这一重入性质由 `handler_does_not_hold_callback_lock_during_callback` 独立测试覆盖。

同一 Handler 的并发调用存在明确边界：首个调用执行期间闭包已从槽中取出，另一并发调用会看到 `None` 并按“未设置期望”路径 panic，而不是并行执行同一个 `FnMut`。因此同步原语保证内部状态访问受保护，但不表示单个 mock 方法支持同时并发调用。mock 的资源生命周期就是拥有它的测试作用域；析构时没有 GoMock 式的未满足期望检查。

## 与 Go 版本的对应关系

直接对照文件是生成代码 `pkg/dxf/framework/mock/plan_mock.go`。类型与方法集合保持对应：Go 的四方法 `LogicalPlan`、单方法 `PipelineSpec`、两个 `NewMock*`、`EXPECT`、`ISGOMOCK` 均在 Rust 中有同名外观，Rust trait 实现另提供 idiomatic snake_case 入口。

关键差异如下：

- Go mock 持有 `*gomock.Controller` 和独立 recorder，方法通过 `ctrl.Call` 取返回槽；Rust recorder 只是 mock 自身的类型别名，controller 参数不保存，返回值由类型化闭包产生。
- Go recorder 支持 matcher、期望次数和 `Finish()` 校验；Rust `Handler` 只支持每方法一个当前闭包和成功派发计数。
- Go `FromTaskMeta([]byte)` 的切片可共享底层数据；Rust trait 的借用切片在进入兼容方法前复制为 `Vec<u8>`。
- Go `ToPhysicalPlan` 返回 `*PhysicalPlan`，可为 nil；Rust 返回拥有所有权的 `PhysicalPlan`，成功结果没有 nil 状态。
- Go `ISGOMOCK` 返回空结构体，Rust 返回单元值 `()`；二者都不承载业务数据。

Go 测试证明原始使用意图：`planner_test.go` 配置 `ToTaskMeta` 和 `GetTaskExtraParams` 后运行 Planner；`plan_test.go` 配置 `ToSubtaskMeta` 后由 `PhysicalPlan` 调用。Rust 独立测试当前只直接覆盖 PipelineSpec 的上下文转发和 Handler 的重入/计数基础设施，未直接覆盖 `MockLogicalPlan` 四方法的全部转发及错误路径。

## 扩展指南

若 planner trait 新增或修改方法，应同步更新 `MockLogicalPlan`/`MockPipelineSpec` 的 Handler 字段、公开 Go 风格方法、trait 转发实现以及同路径 Go 生成文件对应关系；方法名字符串必须保持稳定且唯一，以便缺失期望 panic 可定位。新增返回类型优先保持类型化 `Result`，不要把 Go 的动态返回槽机械搬入 Rust。

若需要更接近 GoMock 的 matcher、调用顺序或自动完成校验，应扩展 crate 根 `Handler` 或引入独立 recorder 抽象，而不是让 `EXPECT()` 暗示不存在的能力。改变 Handler 并发策略时必须同步评估所有 mock 文件，因为它是 crate 级共享基础设施。

测试应继续放在独立文件中：本 mock 的直接回归放入 `pkg/dxf/framework/mock/migration_aster_unit_test.rs`（或新增独立 `*_test.rs` 并由模块入口挂载），planner 主链行为放入 `pkg/dxf/framework/planner/planner_test.rs` 和 `plan_test.rs`。至少覆盖新增方法的参数原样转发、成功与错误返回、调用计数、未设置期望边界；涉及 `PlanCtx` 时覆盖新增字段不会在 mock 边界丢失。兼容风险主要是 GoMock 外观与实际能力差异，性能风险主要是 `from_task_meta` 的字节复制和 Handler 的串行 mutex 临界区。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/dxf/framework/mock/plan_mock.rs --offset 1 --limit 500` 读取目标文件全部 146 行，并报告文件被 `pkg/dxf/framework/mock/migration_aster_unit_test.rs` 使用；`query` 确认 Rust/Go 两侧 `MockLogicalPlan`、`MockPipelineSpec`、`NewMockLogicalPlan` 与 `NewMockPipelineSpec` 节点。
- 目标实现：`pkg/dxf/framework/mock/plan_mock.rs`，核对全部类型别名、结构体、固有方法、trait impl 与构造器。
- crate 边界：`pkg/dxf/framework/mock/Cargo.toml` 与 `pkg/dxf/framework/mock/lib.rs`，核对依赖、模块装载、公开重导出和 Handler 语义。
- 真实 trait 与调用链：`pkg/dxf/framework/planner/plan.rs`、`pkg/dxf/framework/planner/planner.rs`，核对 `PlanCtx`、`LogicalPlan`、`PhysicalPlan::to_subtask_metas`、`PipelineSpec` 和 Planner 创建任务流程。
- Go 对照：`pkg/dxf/framework/mock/plan_mock.go`，核对生成来源、方法集合、controller/recorder 调度与返回槽语义。
- 独立测试：`pkg/dxf/framework/mock/migration_aster_unit_test.rs`，核对 `PlanCtx.task_key` 转发、调用计数和 Handler 回调期间不持锁；`pkg/dxf/framework/planner/planner_test.rs` 与 `plan_test.rs` 核对真实 Rust planner/physical-plan 契约；`pkg/dxf/framework/planner/planner_test.go` 与 `plan_test.go` 核对 GoMock 原始使用意图。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的 `test -f` 加 11 个固定标题计数命令做结构验证，并人工检查文档未把 GoMock 的未迁移能力描述为已支持。

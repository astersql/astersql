# `pkg/dxf/framework/scheduler/mock/scheduler_mock.rs`

源码：[scheduler_mock.rs](scheduler_mock.rs)

## 文件定位

本文件属于独立 crate `astersql-dxf-framework-scheduler-mock`，由同目录 `lib.rs` 以 `pub mod scheduler_mock` 装配并通过 `pub use scheduler_mock::*` 导出。crate 在根 `Cargo.toml` 中注册为 `facade_dxf_framework_scheduler_mock`，又由 `pkg/lib.rs` 暴露到 `dxf::framework::scheduler::mock` 路径；直接依赖它的 Cargo 清单包括调度器的 Windows dev-dependency、`framework/integrationtests` 和 `framework/testutil`。

它是测试基础设施，不是调度算法本身：文件头保留 Go MockGen 的来源与生成命令，Rust 侧用 `mockall` 复刻 Go `pkg/dxf/framework/scheduler/mock/scheduler_mock.go` 的 `Extension` mock 方法面。当前文件已不是可由该 Go 命令重新生成的 Rust 产物，修改时必须同时理解 `mockall::mock!` 的展开规则。

需要特别区分“方法面迁移”和“生产 trait 接线”。本文件使用本 mock crate 在 `lib.rs`、`storage_adapter.rs` 中定义或再导出的 `Context`、proto 类型和 `storage::TaskHandle`，公开方法保留 Go 风格名称；源码中没有 `impl astersql_dxf_framework_scheduler::Extension for MockExtension<_>`。因此它目前是可独立配置和验证的兼容 mock，并不能仅凭类型名断言已作为生产 Rust 调度器的 `Extension` trait object 使用。`pkg/dxf/framework/integrationtests/framework_test.rs` 中两处看似使用它的调用位于归档 Go 草稿字符串，不是编译后的 Rust 调用证据。

## 核心职责

1. 通过 `mockall::mock!` 生成泛型 `MockExtension<H>`，为 Go `scheduler.Extension` 的八个回调提供 `expect_*` 配置入口、参数匹配、返回值注入、次数约束和 `checkpoint()` 校验。
2. 以泛型 `H: storage::TaskHandle + Send + Sync + 'static` 表示回调中的任务句柄。选择泛型而非 trait object 的原因写在源码注释中：`storage::TaskHandle` 继承的接口含泛型回调方法，因而不具备 dyn compatibility。
3. 保留 GoMock 的外形兼容层：`EXPECT` 返回 mock 自身以便继续调用 mockall 生成的 `expect_*`；`ISGOMOCK` 是空标记；`NewMockExtension` 接收但不使用 controller 参数。
4. 只负责分发预先配置的测试行为，不保存或实现真实的任务调度、步骤状态机、节点选择、元数据持久化或 session/事务访问逻辑。

## 主要符号

- `mockall::mock! { pub Extension<H> { ... } }`：生成 `MockExtension<H>` 及每个方法对应的 `expect_<Method>` 配置器。源码直接声明八个公开方法：
  - `GetEligibleInstances(Context, &Task) -> anyhow::Result<Vec<String>>`：返回测试设定的候选执行节点；空列表的业务含义由真实调度器解释，mock 本身不解释。
  - `GetNextStep(&TaskBase) -> Step`：按调用者配置返回下一步骤。
  - `IsRetryableErr(anyhow::Error) -> bool`：对注入错误给出可重试判定。
  - `ModifyMeta(Vec<u8>, Vec<Modification>) -> anyhow::Result<Vec<u8>>`：接收所有权并返回变换后的 meta 或错误。
  - `OnDone(Context, &H, &Task) -> anyhow::Result<()>`：模拟任务终结回调。
  - `OnNextSubtasksBatch(Context, &H, &Task, Vec<String>, Step) -> anyhow::Result<Vec<Vec<u8>>>`：按候选节点和目标步骤生成下一批 subtask meta。
  - `OnPrepare(Context, &H, &mut Task) -> anyhow::Result<()>`：唯一取得可变 `Task` 的方法，可就地修改准备阶段字段。
  - `OnTick(Context, &Task)`：无返回值的周期回调。
- `impl<H> MockExtension<H>`：只增加两个 GoMock 兼容方法，重复施加与生成类型一致的 `TaskHandle + Send + Sync + 'static` 约束。
- `MockExtension::ISGOMOCK(&self)`：空操作标记。Rust 返回单元值 `()`，对应 Go 版本返回空结构体，仅用于兼容性识别而不参与行为。
- `MockExtension::EXPECT(&mut self) -> &mut Self`：返回同一对象的可变引用；没有 Go 版本中独立的 `MockExtensionMockRecorder` 状态对象。
- `NewMockExtension<H, C>(&C) -> MockExtension<H>`：调用 mockall 生成的 `MockExtension::new()` 创建空期望集。`C: ?Sized` 让任意借用都可占据 Go controller 参数位置，但 `_controller` 不被保存，也不控制生命周期或校验。

本文件没有模块级常量、枚举、显式字段、条件编译项或手写错误类型。`MockExtension` 的期望存储、调用计数及析构检查均由 `mockall` 宏生成。

## 执行流程

典型可编译流程由 `migration_aster_unit_test.rs` 给出：

1. 测试定义满足 `storage_adapter.rs` 中 `SessionExecutor` 和 `TaskHandle` 的具体句柄，例如 `TestHandle`。
2. `NewMockExtension::<TestHandle, _>(&())` 忽略兼容 controller，返回一个尚未配置期望的 `MockExtension<TestHandle>`。
3. 调用 `EXPECT()` 可确认返回的仍是同一实例；实际配置通过宏生成的 `expect_GetNextStep()`、`expect_OnPrepare()` 等方法完成。测试可用 `withf` 匹配参数、`times` 约束次数、`return_const` 或 `returning` 注入行为。
4. 被测代码或测试直接调用 Go 风格方法。mockall 查找匹配期望，执行闭包，并原样返回闭包结果；例如 `ModifyMeta` 闭包取得 `Vec<u8>` 后追加内容，`OnPrepare` 闭包通过 `&mut Task` 改写 `Meta`、`RequiredSlots` 和 `MaxNodeCount`。
5. 测试显式调用 `checkpoint()`，或等待 mockall 在对象析构时检查尚未满足的期望。当前独立迁移测试选择显式检查，使失败位置更靠近行为断言。

在真实 Go 主链中，同名 mock 被注入 `scheduler.Extension`，其方法会由 scheduler 的 tick、步骤推进、节点筛选和结束路径调用；`scheduler_mock.go` 及 `scheduler_nokit_test.go` 展示了这些用法。但 Rust 文件本身没有将生成类型接入生产 Rust trait，所以这些 Go 调用只能作为迁移意图和方法语义证据，不能作为当前 Rust 静态调用边。

## 数据与状态

- 业务输入数据来自 proto crate：`Task`/`TaskBase` 表示完整任务与基础字段，`Step` 表示步骤，`Modification` 描述 meta 修改。`lib.rs` 只再导出 mock 所需的子集。
- `old_meta`、`modifications`、`exec_ids` 和返回的 subtask metas 都按值传递；期望闭包可以消费、修改或重新构造这些容器，不涉及本文件内共享缓存。
- `Task` 在大多数回调中只读，只有 `OnPrepare` 使用 `&mut Task`。独立测试证实其修改在调用返回后对调用者可见。
- `H` 只以共享引用 `&H` 传给生命周期回调。句柄实现可提供 session/事务回调和历史 subtask 查询，但本 mock 不主动调用这些能力，是否调用完全由配置的返回闭包决定。
- 隐式状态位于 mockall 生成对象内，包括各方法的期望队列、匹配器、剩余调用次数和返回闭包。`EXPECT()` 不创建第二份状态，`std::ptr::eq` 测试证明它返回同一实例。
- `Context` 在此 crate 中是 `()`；它只保留参数位置，不携带生产 `scheduler::Context` 的取消标志、条件变量或等待语义。

## 依赖与调用关系

上游装配与调用证据：

- `pkg/dxf/framework/scheduler/mock/lib.rs` 声明模块并通配再导出；同文件在 `#[cfg(test)]` 下挂载 `migration_aster_unit_test.rs` 和 `storage_adapter_test.rs`。
- `pkg/dxf/framework/scheduler/mock/migration_aster_unit_test.rs` 是当前最直接的可编译调用者，三次构造 mock，覆盖八个方法、兼容标记和显式期望检查。
- 根 `Cargo.toml` 与 `pkg/lib.rs` 提供 workspace facade；`scheduler/Cargo.toml`、`integrationtests/Cargo.toml`、`testutil/Cargo.toml` 声明依赖。不过 Cargo 依赖只证明 crate 可见性，不等于每个依赖方已经在可编译 Rust 路径中使用该 mock。
- RustCodeGraph 能定位 `NewMockExtension`，但对目标文件报告无静态 callers；这是宏生成方法和现有接线状态的实际索引结果。仓库文本搜索只找到独立迁移测试中的真实 Rust 调用，另两处 integrationtests 调用属于归档字符串。

下游依赖：

- `anyhow::{Error, Result}` 是八个方法的统一错误/结果边界。
- `mockall` 生成 mock 类型、`expect_*` 方法、匹配与调用计数状态，以及 `checkpoint()`/析构验证。
- `astersql-dxf-framework-proto` 经 `lib.rs::proto` 提供 `Modification`、`Step`、`Task`、`TaskBase`。
- `storage_adapter.rs` 提供 `SessionExecutor`、`TaskHandle` 及其相关测试占位类型；本文件只依赖 `TaskHandle` 约束。

Go 对照中的主调用边为 scheduler/testutil → `NewMockExtension` → `EXPECT`/各录制器方法 → scheduler 调用八个 `Extension` 方法。Rust 当前已验证的是测试 → `NewMockExtension` → mockall `expect_*` →直接调用生成方法，尚无到生产 `scheduler::Extension` trait 的实现边。

## 错误处理与边界

- `GetEligibleInstances`、`ModifyMeta`、`OnDone`、`OnNextSubtasksBatch`、`OnPrepare` 使用 `anyhow::Result`，配置闭包返回的错误不被包装或吞掉。`metadata_and_batch_callbacks_forward_arguments_and_errors` 验证 `OnDone` 的 `cleanup failed` 原样可见。
- `IsRetryableErr` 接收拥有所有权的 `anyhow::Error`。匹配闭包若需要判断类型或消息，应在消费边界内完成；同一错误不能在调用后继续使用。
- `GetNextStep`、`IsRetryableErr`、`OnTick` 无错误返回通道；其异常测试行为主要表现为未匹配期望、超出次数或配置闭包 panic。
- 空构造器不会提供默认业务返回值。调用未配置的方法是否 panic、默认次数以及析构检查细节由当前 `mockall` 版本决定，扩展者不应把它当成稳定业务默认值；应为预期路径显式配置返回和次数。
- `OnPrepare` 可修改 task，但 Go 接口注释限制准备阶段允许修改的字段。mock 不强制该业务约束，测试作者有责任只模拟真实实现允许的变化。
- `Context = ()` 意味着此 mock 无法验证取消、deadline 或唤醒传播。需要这些行为时应使用生产 scheduler 的 context 或专门适配层，不能从本 mock 的成功调用推导取消语义已覆盖。
- `NewMockExtension` 的 controller 参数被忽略；Go 中 controller 负责集中记录和生命周期验证，Rust 中由每个 mockall 实例自行持有并检查期望。

## 并发与资源生命周期

`H` 被要求同时满足 `Send + Sync + 'static`，使生成 mock 的回调签名能够安全持有适合跨线程测试的句柄类型。该约束不表示 `MockExtension<H>` 的每种配置闭包都适合任意并发模式；闭包捕获值仍需满足 mockall 生成 API 的线程安全约束。

文件自身不创建线程、异步任务、锁、通道、事务或外部资源。并发状态需要由期望闭包显式捕获线程安全对象；`prepare_can_mutate_task_and_tick_is_dispatched` 使用 `Arc<AtomicUsize>` 和 `Ordering::SeqCst` 统计两次 `OnTick`，证明回调可以操作共享原子状态，但没有证明并发调用顺序。

资源生命周期以 `MockExtension` 实例为界：构造时创建空期望状态，配置闭包及其捕获值随实例保存，调用时更新次数，`checkpoint()` 可提前验证并清空已满足期望，析构时 mockall 还会执行其默认校验。传入 `NewMockExtension` 的 controller 引用不会被保存，因此 controller 的生命周期与返回 mock 无关；传给回调的 `&H`、`&Task` 或 `&mut Task` 只在该次调用期间借用。

## 与 Go 版本的对应关系

对应源文件是 `pkg/dxf/framework/scheduler/mock/scheduler_mock.go`，其声明来自 `scheduler.Extension`。八个方法名、参数顺序和返回形状在 Rust 文件中逐项保留，因而迁移测试可以沿用 GoMock 的阅读习惯。

主要差异如下：

- Go 使用 `*gomock.Controller`、`MockExtensionMockRecorder` 和反射式 `RecordCallWithMethodType`；Rust 使用 mockall 的静态宏生成 API，没有独立 recorder 类型，`EXPECT()` 返回 `&mut Self`。
- Go controller 被构造器保存并统一驱动调用与校验；Rust 构造器的泛型 controller 参数只是形状兼容占位，实际状态由 `MockExtension::new()` 创建。
- Go `ISGOMOCK() struct{}` 与 Rust `ISGOMOCK() -> ()` 都是无业务状态的标记。
- Go `context.Context` 在 Rust mock crate 中退化为 `()`；Go `storage.TaskHandle` 是接口值，Rust 使用具体泛型 `H`，以绕开含泛型回调方法的 trait object 限制。
- Rust `OnPrepare` 明确接受 `&mut Task`，使测试闭包可在借用规则下就地修改任务；其他 task 参数保持只读。
- Go 的 `[]byte`/`[][]byte`/`[]string` 对应 Rust 的拥有型 `Vec<u8>`/`Vec<Vec<u8>>`/`Vec<String>`；Rust 方法会发生所有权移动。
- 生产 Rust `pkg/dxf/framework/scheduler/interface.rs::Extension` 使用蛇形方法、生产自有 `Context`/`Task`/`SchedulerError`，且部分回调接收 `&mut Task` 或 `&dyn TaskHandle`。目标 mock 没有实现它，因此两者不能视为当前可直接互换。

语义证据来自两侧测试：Go 的 `scheduler_nokit_test.go` 使用该 mock 注入候选节点错误、规划错误和不同 subtask 批次；Rust 的 `migration_aster_unit_test.rs` 聚焦验证 mock API 自身的参数转发、返回/错误、可变任务与次数约束。二者覆盖层级不同。

## 扩展指南

若 Go `scheduler.Extension` 新增或修改方法，安全迁移顺序是：

1. 先核对 `interface.go` 的签名、业务注释和真实 scheduler 调用点，再更新本文件 `mockall::mock!` 中的方法，不能只依据生成的 Go mock 猜测语义。
2. 同步检查 `lib.rs` 的 proto 再导出与 `storage_adapter.rs` 的最小句柄边界；新增类型应来自真实依赖或明确的适配层，避免在 mock 文件内复制生产模型。
3. 在独立的 `migration_aster_unit_test.rs` 增加期望匹配、成功返回、错误传播、调用次数和必要的可变性测试。测试逻辑不得放进本源文件。
4. 若目标是让 mock 注入生产 Rust scheduler，应单独设计 trait 实现或共享类型边界，并解决 Go 风格方法与生产蛇形 trait、`anyhow::Error` 与 `SchedulerError`、空 Context 与可取消 Context、泛型 `H` 与 `dyn TaskHandle` 的转换；不要仅增加类型别名掩盖不兼容。
5. 保持 Go 兼容入口时，不要删除 `EXPECT`、`ISGOMOCK`、`NewMockExtension` 或改变参数顺序；若决定改用纯 Rust 命名，应先迁移所有调用者并明确是否仍承诺 Go 形状兼容。

性能风险很低，因为该文件仅用于测试；主要风险是 mockall 匹配闭包或复制大型 `Vec` 带来的测试开销。更重要的是正确性风险：宽泛匹配器可能漏掉参数回归，遗漏 `times` 可能弱化生命周期断言，错误地把归档字符串当作编译接线则会高估迁移完成度。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件，目标目录六个已索引文件；`node --file pkg/dxf/framework/scheduler/mock/scheduler_mock.rs` 读取了完整 115 行。
- RustCodeGraph 符号查询：`query/node NewMockExtension` 定位目标构造器第 109 行；`callees NewMockExtension` 对目标 Rust 定义未发现下游静态边；文件索引显示 `used by 0 files`。宏生成的 `MockExtension`/`expect_*` 没有形成可依赖的完整静态图，因此以源码与独立测试补证。
- 读取的 Rust 事实文件：`scheduler_mock.rs`、同 crate 的 `lib.rs`、`Cargo.toml`、`storage_adapter.rs`、`migration_aster_unit_test.rs`，以及生产边界 `pkg/dxf/framework/scheduler/interface.rs`。
- 读取的 Go 对照与测试证据：`scheduler_mock.go`、`interface.go`、`scheduler_nokit_test.go` 的引用，以及 `pkg/dxf/framework/testutil/scheduler_util.go` 的通用期望配置。
- 调用与装配核验：根 `Cargo.toml`、`pkg/lib.rs`、`scheduler/Cargo.toml`、`integrationtests/Cargo.toml`、`testutil/Cargo.toml`；仓库 `rg` 搜索区分了可编译 Rust 调用与归档 Go 草稿字符串。
- 独立迁移测试覆盖：`constructor_marker_and_scalar_results_follow_gomock_contract`、`metadata_and_batch_callbacks_forward_arguments_and_errors`、`prepare_can_mutate_task_and_tick_is_dispatched`，合计覆盖八个 mock 方法、构造/标记、参数转发、错误传播、任务修改和期望次数。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另运行固定十一章节结构检查，并人工确认本文明确回答文件为何存在、如何运行、当前未接线边界及安全扩展位置。

# `pkg/dxf/framework/testutil/scheduler_util.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-testutil` crate，由 [`lib.rs`](lib.rs) 以 `pub mod scheduler_util` 暴露并通过 `pub use scheduler_util::*` 再导出。它提供 DXF（Distributed eXecution Framework）测试使用的脚本化调度器对象，用来表达“任务从哪个步骤走到哪个步骤、每一步生成多少子任务、何时注入规划错误、完成回调何时失败”等场景。源码位置是 [`scheduler_util.rs`](scheduler_util.rs)，crate 边界与 Go 包迁移来源记录在 [`Cargo.toml`](Cargo.toml)。

它位于生产目录中的 `testutil` crate，但职责是测试夹具，不是生产调度算法。生产调度扩展协议定义在 [`../scheduler/interface.rs`](../scheduler/interface.rs) 的 `Extension` trait；本文件的 `SchedulerExtension` 是一个简化的独立模型，没有直接实现该 trait。测试侧通过 [`disttest_util.rs`](disttest_util.rs) 的 `TaskTypeRegistry::register_scheduler`、`RegisterTaskType` 等抽象接收它，或直接调用其方法验证迁移语义。因此阅读调用链时应区分“复现扩展回调行为”和“已接入生产 scheduler trait”这两个层次。

## 核心职责

1. `SchedulerInfo` 与 `StepInfo` 把调度过程描述为有序脚本：从 `STEP_INIT` 开始，按给定顺序进入业务步骤，最终到 `STEP_DONE`。
2. `GetMockSchedulerExt` 把顺序脚本编译成两个按 `Step` 索引的表：步骤转移表 `transitions` 和规划行为表 `step_infos`。
3. `SchedulerExtension` 暴露与 Go `scheduler.Extension` mock 对应的核心行为：步骤推进、子任务 meta 生成、错误可重试判定、任务 meta 修改以及完成回调。
4. 若干便捷构造函数固定常见场景：基础两步、HA 大批量、第一步永久错误、第二步永久错误、首次规划失败并在首次完成回调失败、单步回滚。
5. 内部共享状态保证克隆后的 `SchedulerExtension` 仍观察同一份调用计数和一次性失败标志，适合并发或多持有者测试。

本文件不会选择真实节点、访问存储、启动调度线程或执行子任务。`eligible_instances` 固定返回空列表，`on_tick` 为空；实际节点过滤、任务存储和调度循环属于其他模块。

## 主要符号

- `SchedulerInfo { all_error_retryable, step_infos }`：公开构造配置。`all_error_retryable` 决定 `is_retryable_error` 的固定返回值；`step_infos` 的顺序同时决定状态迁移顺序。
- `StepInfo { step, error, error_repeat_count, subtask_count }`：公开的单步骤脚本。前 `error_repeat_count` 次规划返回 `error`；之后成功生成 `subtask_count` 条 meta。计数不放在 `StepInfo` 内，而由扩展的 `call_counts` 统一维护。
- `Modification { modification_type, to }`：测试用修改项。`modify_meta` 将其编码为逗号分隔的 `type=to` 字节串。
- `SchedulerMode`：私有模式枚举。`Normal` 执行通用步骤脚本；`RetryOnceThenOnDoneError` 专用于 `GetPlanErrSchedulerExt` 的顺序敏感错误场景。
- `SchedulerExtension`：可克隆的测试调度器。不可变配置放在 `Arc<HashMap<...>>`，可变计数与一次性标志分别放在 `Arc<Mutex<...>>` 和 `Arc<AtomicBool>`。
- `SchedulerExtension::next_step(current)`：查询当前步骤的后继；不存在映射时返回 `Step(0)`，复现 Go map 读取缺失键时的零值。
- `SchedulerExtension::next_subtasks_batch(task, next_step)`：规划入口。普通模式按目标 `next_step` 读取脚本；特殊模式则按 `task.base.step` 判断当前阶段。
- `SchedulerExtension::is_retryable_error`、`eligible_instances`、`on_tick`：分别返回构造时设定的布尔值、空实例列表和空操作。
- `SchedulerExtension::modify_meta`、`on_done`：前者做确定性文本编码；后者在特殊模式首次返回 `DxfError("not retryable err")`，以后成功。
- `GetMockSchedulerExt`：通用公开构造器；空脚本会以 `stepInfos should not be empty` panic，非空脚本返回 `Result<SchedulerExtension, DxfError>`。
- `GetMockBasicSchedulerExt`、`GetMockHATestSchedulerExt`、`GetPlanNotRetryableErrSchedulerExt`、`GetStepTwoPlanNotRetryableErrSchedulerExt`、`GetPlanErrSchedulerExt`、`GetMockRollbackSchedulerExt`：公开场景构造器。
- `step`、`permanent_error`：私有 `StepInfo` 工厂，分别生成无错误脚本和以 `i64::MAX` 次重复模拟“永久错误”的脚本。

## 执行流程

通用构造流程由 `GetMockSchedulerExt` 驱动：

1. 检查 `scheduler_info.step_infos` 非空；空值立即 panic，与 Go 实现保持一致。
2. 令 `current = STEP_INIT`，依次遍历每个 `StepInfo`。
3. 为每一项写入 `transitions[current] = step_info.step`，再把 `current` 更新为该步骤；同时用步骤值把脚本写入 `step_infos`。
4. 遍历结束后补上 `transitions[last_step] = STEP_DONE`。
5. 把两个表放入 `Arc`，创建空调用计数、`Normal` 模式和未触发的 `on_done_failed` 标志。

普通规划流程由 `next_subtasks_batch` 执行：若 `next_step` 没有脚本，直接返回空批次；否则锁定 `call_counts`，比较该步骤已调用次数与 `error_repeat_count`。仍在错误窗口内时先递增计数，再返回脚本错误；若脚本没有提供错误对象，则使用 `planned scheduler error` 作为兜底。错误窗口结束后生成 `subtask-0` 到 `subtask-(n-1)` 的字节 meta。负的 `error_repeat_count` 使初始计数 `0 < negative` 为假，因此立即成功；[`scheduler_util_test.rs`](scheduler_util_test.rs) 固定了这一 Go 兼容行为。

`GetPlanErrSchedulerExt` 不走通用脚本，而是建立固定转移 `INIT → ONE → TWO → DONE`。特殊规划依据当前 `task.base.step`：`STEP_INIT` 首次通过 `TestContext::next_call_time()` 返回 `retryable err`，后续返回 `task1`、`task2`、`task3`；`STEP_ONE` 返回 `task4`；其他当前步骤返回空批次。它的 `on_done` 使用原子交换保证第一次报 `not retryable err`，以后返回成功。

便捷构造器只是向上述流程填入不同脚本：基础场景为 `ONE:3, TWO:1`，HA 为 `ONE:10, TWO:5`，第一步/第二步错误场景将对应步骤设为 `i64::MAX` 次错误，回滚场景仅包含 `ONE:3`。

## 数据与状态

- `transitions: Arc<HashMap<Step, Step>>` 在构造后只读，保存完整的链式后继关系。重复的 `StepInfo.step` 会覆盖 `step_infos` 中先前脚本，也可能改写转移图的同名键；构造器没有单独拒绝重复或环路配置，因此调用方应提供唯一且有序的步骤。
- `step_infos: Arc<HashMap<Step, StepInfo>>` 按目标步骤查找普通规划脚本。调用 `next_subtasks_batch` 时传入的 `task` 在普通模式不参与选择，只有 `next_step` 是键。
- `call_counts: Arc<Mutex<HashMap<Step, i64>>>` 记录每个目标步骤的失败次数。不同步骤独立计数；成功后计数不再变化。所有克隆共享同一张表。
- `test_context: Option<Arc<TestContext>>` 仅特殊模式存在。`TestContext::next_call_time` 以 `AtomicU64::fetch_add(SeqCst)` 返回递增前的值，所以恰有一个调用能观察到 `0`。
- `on_done_failed: Arc<AtomicBool>` 在特殊模式中记录一次性失败；普通模式虽也持有它，但不会读取它。
- 子任务 meta 与修改后 meta 都是拥有所有权的 `Vec<u8>`，不借用输入，离开锁后可以独立保存。

重要不变量是：非空的普通脚本总能从 `STEP_INIT` 沿输入顺序到达 `STEP_DONE`；同一个 `SchedulerExtension` 的克隆共享错误预算；`GetPlanErrSchedulerExt` 的规划首次失败状态和完成首次失败状态彼此独立。

## 依赖与调用关系

直接源码依赖很小：标准库提供 `HashMap`、`Arc`、`Mutex`、`AtomicBool`；同 crate 的 [`context.rs`](context.rs) 提供 `DxfError`、`Step`、`Task`、`TestContext` 及四个步骤常量。`Cargo.toml` 声明的是整个 testutil crate 的依赖集合，本文件本身没有直接引用那些外部 crate。

RustCodeGraph 将本文件识别为被 DXF 示例、框架集成测试、scheduler 接口等多个文件使用，并确认主要下游边包括 `GetMockSchedulerExt → SchedulerExtension` 以及 `next_subtasks_batch → TestContext::next_call_time / DxfError`。精确调用方文本检索补充出以下主要上游：

- [`disttest_util.rs`](disttest_util.rs) 的 `RegisterExampleTask`、`RegisterTaskType` 和 `RegisterTaskTypeForRollback` 接收 `SchedulerExtension`，再通过 `TaskTypeRegistry::register_scheduler` 注册测试任务类型。
- [`../integrationtests/framework_scope_test.rs`](../integrationtests/framework_scope_test.rs) 用 `GetMockSchedulerExt` 自定义每步子任务数，并直接验证 `INIT → ONE → TWO → DONE`。
- [`../integrationtests/framework_ha_test.rs`](../integrationtests/framework_ha_test.rs) 用 `GetMockHATestSchedulerExt` 验证两步分别产生 10 和 5 条 meta。
- [`../integrationtests/framework_err_handling_test.rs`](../integrationtests/framework_err_handling_test.rs) 覆盖可重试的一次性规划错误、第一步永久错误、第二步永久错误和一次性 `on_done` 错误。
- [`../integrationtests/framework_rollback_test.rs`](../integrationtests/framework_rollback_test.rs) 使用单步回滚构造器，保证回滚前没有跳过业务步骤。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 集中验证步骤迁移、meta 编码和特殊模式的调用顺序。

生产 [`../scheduler/interface.rs`](../scheduler/interface.rs) 的 `Extension` 方法形状与这些方法概念对应，但参数更完整（如 `TaskHandle`、可变 `Task`、执行节点列表和生产错误类型）。当前文件不存在把 `SchedulerExtension` 转换或实现为该 trait 的代码；因此它目前主要支撑 Rust 迁移测试模型和 testutil 注册抽象。

## 错误处理与边界

- 空 `step_infos` 是配置错误，`GetMockSchedulerExt` 选择 panic 而非返回 `Err`；其 `Result` 返回类型在当前实现的非空路径中始终是 `Ok`。便捷构造器因此用 `expect` 表达内置脚本必然合法。
- 普通模式遇到未知 `next_step` 返回空批次，不报错；未知当前步骤经 `next_step` 返回 `Step(0)`。这两个行为都模拟 Go 的“缺失 map 项”语义，但也可能掩盖拼错步骤，新增调用方需要主动断言合法转移。
- 错误重复条件是 `call_count < error_repeat_count`。零或负数表示不注入错误；`i64::MAX` 被用作实际测试时不会耗尽的永久错误预算。
- `StepInfo.error` 在错误窗口内可以是 `None`；此时返回合成的 `DxfError("planned scheduler error")`，不会 panic。
- `call_counts.lock().unwrap()` 会在互斥锁中毒时 panic；本测试工具没有把锁中毒转换为 `DxfError`。
- `modify_meta` 不转义逗号或等号，也不读取旧 meta；类型名若包含分隔符会产生歧义。这是对当前 Go 测试编码的窄复现，不是通用序列化格式。
- 特殊模式内部用 `expect("plan-error context")` 取得上下文；公开构造器始终填入该值，但未来若新增内部构造路径必须维持“特殊模式必有上下文”的不变量。

## 并发与资源生命周期

`SchedulerExtension` 的所有共享字段都满足克隆后共享语义：静态表由 `Arc` 延长生命周期，错误计数由 `Mutex` 串行更新，完成失败标志和 `TestContext` 调用序号使用顺序一致性原子操作。由此，多线程同时规划同一步骤时，最多只有配置数量的调用获得脚本错误；特殊模式首次 INIT 规划错误与首次 `on_done` 错误也各自最多发生一次。

普通模式持有 `call_counts` 锁直到完成计数判断或构造成功结果。当前结果生成很轻量，但若未来把 meta 生成扩展为昂贵逻辑，应在复制必要配置后尽早释放锁，避免无关调用串行化。`Ordering::SeqCst` 强于这里只为一次性门闩通常所需的排序，但与 `TestContext` 的现有实现一致，修改时要同时检查并发测试语义。

本文件不创建线程、异步任务、通道、文件、网络连接或数据库事务，因此没有显式关闭流程。所有资源依靠 `Arc`/容器析构释放；它也不拥有 [`disttest_util.rs`](disttest_util.rs) 的 `RegistrationGuard`，注册清理由上层 RAII 守卫负责。

## 与 Go 版本的对应关系

直接对照文件是 [`scheduler_util.go`](scheduler_util.go)。主要一致点如下：

- `SchedulerInfo`、`StepInfo` 及六类构造器保持同一场景含义和 Go 风格公开名称。
- 通用转移均从 `StepInit` 串联输入步骤并以 `StepDone` 结束；缺失转移返回步骤零值。
- 普通规划前若干次返回错误，之后生成 `subtask-%d`；基础、HA、不可重试错误和回滚的步骤数与子任务数相同。
- `ModifyMeta`/`modify_meta` 都按输入顺序产生 `type=to` 并用逗号连接。
- 特殊构造器都在第一次 INIT 规划时报 `retryable err`，之后生成三条 meta；ONE 阶段生成 `task4`；首次完成回调报 `not retryable err`，以后成功。

主要差异也必须保留在迁移判断中：Go 版本用 gomock 构造真正的 `scheduler.Extension` mock，方法接收 context、storage handle、节点列表等完整参数；Rust 版本是具体 `SchedulerExtension`，暴露删去未使用参数的简化同步方法，且尚未实现生产 `Extension` trait。Go 的 `StepInfo.callCount` 位于复制进 map 的结构体中，Rust 用共享 `Mutex<HashMap<Step, i64>>` 单独计数。Go `ModifyMeta` 返回 `([]byte, error)`，Rust 直接返回 `Vec<u8>`。Go 的空 slice/nil 在 Rust 中都表现为 `Vec::new()`，无法保留 nil 与空切片的区别。

[`scheduler_util_test.rs`](scheduler_util_test.rs) 专门固定了空配置 panic、负错误次数立即成功、未知步骤返回 `Step(0)` 三个边界；Rust 集成测试及同目录 Go 集成测试则证明预设构造器仍服务于相同的测试场景。没有发现同路径独立的 `scheduler_util_test.go`，Go 侧证据来自 `framework_*_test.go` 对这些构造器的使用。

## 扩展指南

- 新增通用步骤行为时，优先扩展 `StepInfo` 和 `next_subtasks_batch`，并在独立的 [`scheduler_util_test.rs`](scheduler_util_test.rs) 增加最小边界测试；不要把 `#[cfg(test)]` 测试嵌入本源文件。
- 新增固定场景时，用私有工厂组合 `GetMockSchedulerExt`，避免复制转移表和共享状态初始化；同时在对应 `framework_*_test.rs` 验证该场景的业务意图。
- 调整步骤转移时要同步核对 Go [`scheduler_util.go`](scheduler_util.go)，尤其是输入顺序、最终 `STEP_DONE`、未知键零值和重复步骤行为。若选择偏离 Go，应在测试与迁移说明中明确原因。
- 修改错误脚本时要分别验证“错误次数”“错误是否可重试”和“错误发生在哪个当前/目标步骤”。普通模式按 `next_step` 索引，特殊模式按 `task.base.step` 分支，两者不可混淆。
- 若要让该类型直接接入生产 scheduler，应该新增明确的适配器或实现 [`../scheduler/interface.rs`](../scheduler/interface.rs) 的 `Extension`，补齐 `TaskHandle`、生产 `Task`/`SchedulerError`、`on_prepare` 等契约；不能仅靠同名方法假定类型兼容。这会扩大到跨 crate 行为修改，不属于当前文档任务。
- 若 meta 格式需要支持任意字符串或兼容持久化，必须替换当前无转义拼接，并同时更新 Go 对照与消费者；当前格式只适合受控测试输入。
- 并发相关修改需验证克隆实例共享计数、同时调用时的一次性语义，以及锁中毒/原子排序策略；性能风险主要是扩大 `call_counts` 临界区。

## 验证依据

- RustCodeGraph `status`：索引包含目标目录；`files --filter pkg/dxf/framework/testutil` 显示 `scheduler_util.rs`、Go 对照与独立 Rust 测试均已索引。
- RustCodeGraph 文件节点：完整读取 [`scheduler_util.rs`](scheduler_util.rs) 的 281 行、[`scheduler_util.go`](scheduler_util.go) 的 211 行、[`scheduler_util_test.rs`](scheduler_util_test.rs) 的 58 行和 [`lib.rs`](lib.rs) 的模块装配。
- RustCodeGraph 符号/调用查询：`query` 定位 `GetMockSchedulerExt`、`SchedulerExtension`、`next_subtasks_batch`、`next_step`、`modify_meta`、`on_done`；`callees` 确认构造和错误路径的直接依赖。限定文件的 `callers` 查询未在时限内返回，因此上游调用者改由精确 `rg` 引用检索并通过对应文件节点复核，没有把超时结果当作事实。
- crate 与架构证据：[`Cargo.toml`](Cargo.toml) 确认 crate 名、`lib.rs` 入口及 Go 包迁移来源；[`../doc.go`](../doc.go) 说明 DXF 的 owner 调度器、全节点执行器和“任务由顺序步骤、每步由并行子任务组成”的总体模型；[`../scheduler/interface.rs`](../scheduler/interface.rs) 给出生产 `Extension` 契约。
- 测试证据：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`scheduler_util_test.rs`](scheduler_util_test.rs)、[`../integrationtests/framework_scope_test.rs`](../integrationtests/framework_scope_test.rs)、[`../integrationtests/framework_ha_test.rs`](../integrationtests/framework_ha_test.rs)、[`../integrationtests/framework_err_handling_test.rs`](../integrationtests/framework_err_handling_test.rs) 和 [`../integrationtests/framework_rollback_test.rs`](../integrationtests/framework_rollback_test.rs)。Go 侧用 `rg` 核对 `framework_test.go`、`framework_ha_test.go`、`framework_scope_test.go`、`framework_err_handling_test.go`、`framework_rollback_test.go` 与 `modify_test.go` 的构造器调用。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在且恰有十一个规定的二级标题；最终交付前另行执行该命令并记录退出码。

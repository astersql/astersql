# `pkg/dxf/framework/mock/execute/execute_mock.rs`

## 文件定位

本文件属于独立 crate `astersql-dxf-framework-mock-execute`，crate 入口 `pkg/dxf/framework/mock/execute/lib.rs` 将 `execute_mock` 模块的公开项全部再导出。它位于 DXF（Distributed eXecution Framework）测试支撑层，不执行真实子任务，而是为 `pkg/dxf/framework/taskexecutor/execute/interface.rs` 中的 `StepExecutor` 和 `StepExecFrameworkInfo` trait 生成可编程替身。

`pkg/dxf/framework/mock/execute/Cargo.toml` 表明该 crate 直接依赖 `execute`（trait 与上下文/摘要类型）、`proto`（步骤、资源、子任务模型）、`metering`（计量记录器）、`anyhow`（统一结果）和 `mockall`（期望引擎）。当前直接消费方包括同 crate 的迁移回归测试 `migration_aster_unit_test.rs`，以及把该 crate 声明为依赖或开发依赖的 `framework/integrationtests`、`framework/testutil` 和 `framework/taskexecutor`。

## 核心职责

1. `mockall::mock!` 生成公开类型 `MockStepExecutor`，使测试可以为 DXF 单步执行器的完整接口逐方法登记参数匹配、调用次数和返回行为。
2. 同一个替身同时实现 `StepExecFrameworkInfo` 与 `StepExecutor`，因此既能模拟 `Init -> RunSubtask -> Cleanup` 等执行生命周期，也能模拟框架注入的 step、资源、计量和 checkpoint 访问面。
3. `NewMockStepExecutor`、`EXPECT`、`ISGOMOCK` 保留 GoMock 生成物的命名与使用习惯，降低 Go 测试迁移到 Rust 时的接口差异；真正的记录、匹配、派发和未满足期望检查由 `mockall` 生成代码完成。

该文件是行为可配置的测试替身，而不是生产执行器实现；其“业务行为”完全取决于调用方安装的 `expect_*` 规则。

## 主要符号

- `mockall::mock! { pub StepExecutor {} ... }`：宏声明的主体。它生成 `MockStepExecutor`、构造器以及每个 trait 方法对应的 `expect_<方法名>` 配置入口。
- `impl StepExecFrameworkInfo for StepExecutor`：声明七个框架信息方法：`restricted`、`GetStep`、`GetResource`、`SetResource`、`GetMeterRecorder`、`GetCheckpointUpdateFunc`、`GetCheckpointFunc`。其中资源、计量器及 checkpoint 回调使用 `Option` 表达当前未配置/不存在。
- `impl StepExecutorTrait for StepExecutor`：声明八个执行方法：`Init`、`RunSubtask`、`RealtimeSummary`、`ResetSummary`、`Cleanup`、`TaskMetaModified`、`ResourceModified`、`SetFrameworkInfo`。`StepExecutorTrait` 别名避免 trait 名与宏内 mock 名冲突。
- `MockStepExecutor::ISGOMOCK(&self)`：无状态标记方法，返回单元值；迁移测试用它验证 GoMock 兼容表面存在。
- `MockStepExecutor::EXPECT(&mut self) -> &mut Self`：返回自身，便于从 GoMock 风格的 `EXPECT()` 过渡到 `mockall` 生成的 `expect_Init()`、`expect_RunSubtask()` 等方法。它本身不创建独立 recorder。
- `NewMockStepExecutor() -> MockStepExecutor`：调用 `MockStepExecutor::new()` 创建尚未登记期望的实例。与 Go 构造器不同，它不接收外部 controller。
- `#![allow(non_snake_case)]`：允许公开 API 继续采用 Go 接口的首字母大写命名。

## 执行流程

典型测试流程如下：

1. 调用 `NewMockStepExecutor`，获得由 `mockall` 管理期望集合的空 mock。
2. 测试对实例调用 `EXPECT()`（兼容入口，可选），随后通过相应 `expect_*` 方法指定参数谓词、期望次数和返回闭包。例如 `migration_aster_unit_test.rs::lifecycle_expectations_dispatch_arguments_and_errors` 为 `Init` 返回 `Ok(())`，为 `Cleanup` 返回指定错误。
3. 被测框架把 mock 当作 `StepExecutor` 使用。真实主链在 `pkg/dxf/framework/taskexecutor/task_executor.rs::createStepExecutor` 中取得执行器并调用 `Init`；`runSubtask` 路径调用 `RunSubtask`；`cleanStepExecutor` 取消步骤上下文后调用 `Cleanup`。
4. 宏生成的 trait 实现按方法名和参数选择已登记期望，执行对应返回闭包，并记录调用次数。闭包可以修改借入的 `Subtask`，因此能模拟成功运行后原地更新 `Meta`。
5. 测试断言返回值、副作用和状态转换；mock 离开作用域时，`mockall` 还会检查强制调用次数。`framework/integrationtests/framework_test.rs::test_framework_sub_task_init_env_failed` 展示了上层效果：模拟 `Init` 持续报错后，任务最终进入 `Reverted`。

## 数据与状态

源文件没有手写字段或全局可变状态。`MockStepExecutor` 的期望队列、匹配器、调用计数和返回动作均由 `mockall` 展开后持有。

接口上传递的关键数据包括：

- `Context` 是 `execute` crate 定义的取消令牌，按值传给生命周期和变更回调；mock 不解释取消状态，但匹配闭包可检查它。
- `&mut Subtask` 允许 `RunSubtask` 期望闭包原地修改子任务，例如迁移测试把 `Meta` 从 `before` 改为 `after`。
- `Arc<StepResource>`、`Arc<metering::Recorder>` 和两个 `Arc` 回调类型表达共享所有权；getter 返回克隆/预设的共享对象，`SetResource` 与 `ResourceModified` 则分别模拟直接替换和运行中资源调整通知。
- `RealtimeSummary` 返回 `Option<&'static SubtaskSummary>`。目标 trait 原始签名是与 `self` 借用关联的 `Option<&SubtaskSummary>`，这里为了让 mockall 返回预置引用而收紧为静态引用；测试使用 `Box::leak` 构造该值，因此被泄漏对象不会自动回收。
- `FrameworkInfo` 按值交给 `SetFrameworkInfo`，用于验证框架注入的 step、资源、计量与 checkpoint 状态是否完整。

## 依赖与调用关系

上游关系：

- `pkg/dxf/framework/mock/execute/lib.rs` 私有声明模块并公开再导出其符号；外部 crate 因此通过 crate 根使用 `NewMockStepExecutor`/`MockStepExecutor`。
- `pkg/dxf/framework/mock/execute/migration_aster_unit_test.rs` 是最直接且完整的 Rust 使用者，覆盖所有两组 trait 的公开能力。
- `pkg/dxf/framework/integrationtests/framework_test.rs::test_framework_sub_task_init_env_failed` 创建 mock、配置 `Init` 错误，并把它交给公共 task executor extension，以检查任务状态机的回滚行为。
- RustCodeGraph 对目标文件报告直接文件使用边 `pkg/dxf/importinto/mock/import_mock_test.rs`，但该文件源码没有引用 `MockStepExecutor`；这是基于 crate/模块分析产生的粗粒度文件边，不能据此声称它直接调用本 mock。

下游关系：

- `execute::StepExecutor` 与 `execute::StepExecFrameworkInfo` 决定必须同步的接口集合；mock 的方法签名必须与 `pkg/dxf/framework/taskexecutor/execute/interface.rs` 保持一致。
- `proto::step::Step`、`proto::subtask::{Subtask, StepResource}` 构成步骤执行参数和资源数据。
- `execute::{FrameworkInfo, SubtaskSummary, CheckpointGetFunc, CheckpointUpdateFunc, Context}` 构成框架注入、进度和 checkpoint 边界。
- `mockall` 负责实际调用派发；RustCodeGraph 能索引手写的 `NewMockStepExecutor`、`EXPECT`、`ISGOMOCK`，但不能为宏展开后的 `expect_*` 方法提供可靠的逐方法 callers/callees 边。

## 错误处理与边界

- `Init`、`RunSubtask`、`Cleanup`、`TaskMetaModified`、`ResourceModified` 返回 `anyhow::Result<()>`。mock 不包装或转换错误，配置闭包返回的错误直接交给调用方；迁移测试验证 `Cleanup` 的 `"cleanup failed"` 文本原样可见。
- `GetResource`、`GetMeterRecorder` 和 checkpoint getter 可返回 `None`。调用者必须把“没有资源/计量器/回调”作为合法分支，不能无条件解包。
- 未登记调用、参数不匹配、调用次数超限或实例销毁时次数不足，遵循 `mockall` 的测试失败语义；本文件没有降级默认值或容错分支。
- `restricted` 虽然意在限制框架信息 trait 的外部误实现，但 mock 必须实现它才能满足 `StepExecutor: StepExecFrameworkInfo` 的父 trait 约束。
- `EXPECT()` 只是返回 `&mut Self`，与 Go 版返回独立 `MockStepExecutorMockRecorder` 不同；移植代码必须继续调用 Rust 的 `expect_*` API，而不能照搬 Go recorder 的方法签名。
- 本文件不决定哪些错误可重试，也不决定 Cleanup 错误如何影响任务状态；这些策略位于 task executor。Go 接口注释说明 Cleanup 错误仅记录，而 Rust 主链 `cleanStepExecutor` 当前也显式忽略其返回值。

## 并发与资源生命周期

`MockStepExecutor` 的执行方法多数接收 `&mut self`，配置期望也需要可变借用，因此一个实例默认按串行、独占方式使用；本文件未用 `Arc<Mutex<MockStepExecutor>>` 或异步任务共享它，也未声明额外的 `Send`/`Sync` 保证。

共享资源本身使用 `Arc`：资源、计量器、checkpoint 闭包可以在 mock 返回后继续由测试和框架共同持有。checkpoint 类型还要求回调为 `Send + Sync + 'static`。`Context` 的克隆共享取消状态，测试可用参数谓词确认取消信号是否按预期传递。

`NewMockStepExecutor` 创建期望状态；测试配置和实际调用消耗/更新该状态；实例析构是调用次数验证的重要生命周期终点。`RealtimeSummary` 的静态引用是特殊资源约束：迁移测试通过 `Box::leak` 满足它，代价是该摘要在进程余下生命周期中不释放，适合短生命周期测试但不应复制到生产路径。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/dxf/framework/mock/execute/execute_mock.go`，由 MockGen 针对 `execute.StepExecutor` 生成。两版均提供 `MockStepExecutor`、`NewMockStepExecutor`、`EXPECT`、`ISGOMOCK`，并覆盖 `StepExecutor` 连同其嵌入的 `StepExecFrameworkInfo` 方法。

关键对应与差异：

- Go 版持有 `*gomock.Controller` 和独立 recorder；Rust 版把这些职责交给 `mockall`，构造器无 controller 参数，`EXPECT()` 返回 mock 自身。
- Go 通过接口 embedding 获得框架信息方法，并在框架中用反射注入私有 `frameworkInfo`；Rust trait 显式增加 `SetFrameworkInfo(FrameworkInfo)`，所以 mock 也必须覆盖这一 Rust 专属接线。迁移测试 `framework_info_injection_dispatches_complete_go_equivalent_state` 验证了等价状态内容。
- Go 的指针可为空；Rust 使用 `Option<Arc<_>>` 表达资源、计量器和回调缺失。Go `[]byte` 对应 Rust `Vec<u8>`，`context.Context` 对应 `CancellationToken` 类型别名。
- Go `RealtimeSummary` 返回可空指针，Rust 使用 `Option<&SubtaskSummary>`；本 mock 为配置固定返回值采用 `Option<&'static SubtaskSummary>`。
- 方法名保留 Go 风格，因此文件级允许 `non_snake_case`。这属于迁移兼容选择，不代表仓库普通 Rust API 的命名惯例。

## 扩展指南

- 当 `execute::StepExecutor` 或 `StepExecFrameworkInfo` 增删/修改方法时，首先同步 `mockall::mock!` 中对应 impl；参数所有权、可变性、`Arc`/`Option` 和错误类型必须完全匹配 trait，不能仅按 Go 签名推断。
- 新方法应在独立测试文件 `pkg/dxf/framework/mock/execute/migration_aster_unit_test.rs` 中增加期望登记、实际 trait 调用、参数/返回值和调用次数验证，不要把测试写进 `execute_mock.rs`。若新方法影响任务状态，再扩充 `pkg/dxf/framework/integrationtests/framework_test.rs` 的相应场景。
- 若只是新增 GoMock 兼容门面，放在 `impl MockStepExecutor`，并明确它与 `mockall` 原生 API 的映射；不要重新实现期望引擎或复制 controller 状态。
- 对资源或 checkpoint 扩展，优先延续 `Arc` 与 `Send + Sync + 'static` 边界，并测试 `None`、共享身份和回调错误。对 `RunSubtask` 扩展，要同时验证 `&mut Subtask` 的原地变更是否保留。
- 修改 `RealtimeSummary` 返回策略时，要特别评估 `'static` 限制及测试内存泄漏；如果改用拥有型返回值，需先同步底层 trait 和 Go 语义，不能只修改 mock。
- 兼容风险主要是接口漂移和 Go/Rust 使用方式差异；性能风险较低，因为该 crate 用于测试，但过多 `AnyTimes`、昂贵匹配闭包或长期泄漏摘要仍会降低测试质量。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/framework/mock/execute` 找到 Go/Rust mock、crate 入口和迁移测试。
- RustCodeGraph `node --file pkg/dxf/framework/mock/execute/execute_mock.rs --offset 1 --limit 260`：核对目标文件全部 87 行、9 个索引符号及宏声明；`query StepExecutor --kind trait`、`query StepExecFrameworkInfo --kind trait` 定位两个真实 Rust trait。
- RustCodeGraph `node --file pkg/dxf/framework/mock/execute/migration_aster_unit_test.rs`：核对四个独立测试对生命周期、子任务变更、框架访问器/checkpoint/摘要和框架信息注入的覆盖。
- RustCodeGraph `node --file pkg/dxf/framework/taskexecutor/task_executor.rs --offset 330 --limit 90`：核对 `createStepExecutor` 调用 `Init`、`cleanStepExecutor` 调用并忽略 `Cleanup` 错误，以及 `runSubtask` 所在主链。
- RustCodeGraph `node --file pkg/dxf/framework/integrationtests/framework_test.rs --offset 490 --limit 85`：核对 Init 失败导致任务回滚的上层场景。该机械迁移测试片段仍使用 Go 风格调用表面，本文只把它作为意图与调用位置证据，不声称其当前已经通过编译。
- 直接读取 `pkg/dxf/framework/mock/execute/Cargo.toml`、`lib.rs`、`execute_mock.go`、`pkg/dxf/framework/taskexecutor/execute/interface.rs` 和 `interface.go`：核对 crate 边界、再导出、依赖、GoMock API 以及 Go/Rust trait 语义。
- `rg` 检索 `NewMockStepExecutor`、`MockStepExecutor`、`expect_*` 和 crate 名：确认直接 Rust 测试、集成测试使用点及 Cargo 依赖声明。未发现本目录 `doc.go`；最近的权威契约是上述 trait/interface 文件。
- 本任务只新增说明文档，不修改运行时代码，也不运行 Cargo；最终结构检查应确认本文恰有规定的 11 个二级标题。

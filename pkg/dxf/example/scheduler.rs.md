# `pkg/dxf/example/scheduler.rs`

## 文件定位

本文件是 `astersql-dxf-example` crate 的示例调度端实现，源码由 [`lib.rs`](lib.rs) 的 `mod scheduler` 纳入并通过 `pub use scheduler::*` 再导出。它演示一个固定的两阶段 DXF（Distributed eXecution Framework）任务：`StepInit(-1) -> StepOne(1) -> StepTwo(2) -> StepDone(-2)`，每个业务阶段生成相同数量的子任务元数据。包级说明 [`doc.go`](doc.go) 也把该示例定义为帮助开发者理解 DXF 集成的最小应用。

当前 Rust 移植有明确边界：`schedulerImpl` 只提供与 Go scheduler 扩展同名的固有方法，尚未实现一个 Rust scheduler trait，也没有接入完整 scheduler/handle/storage 后台循环。`Cargo.toml` 将这些尚未接线的框架 crate 放在永不成立的 `target.'cfg(any())'` 下；现有 [`app_test.rs`](app_test.rs) 手工驱动这些真实调度方法，再把产生的元数据交给已移植的 task executor。因而本文件是可执行的示例调度逻辑，但不是完整 Rust DXF 调度服务入口。

## 核心职责

- 用 `newScheduler` 保存一份 `Task` 快照，并由 `schedulerImpl::Init` 从 `Task.Meta` 解出每步子任务数。
- 用 `schedulerImpl::GetNextStep` 定义两步状态机。
- 用 `schedulerImpl::OnNextSubtasksBatch` 为 `StepOne`、`StepTwo` 批量生成 `subtaskMeta` JSON 字节，消息包含子任务序号和可读阶段名。
- 提供 scheduler 生命周期兼容钩子：`OnTick`、`OnPrepare`、`OnDone`、`GetEligibleInstances`、`IsRetryableErr`。
- 用 `Step2Str` 提供阶段名格式化，并用 `postCleanImpl::Clean` 提供任务结束后的清理钩子。

它不负责持久化、实例分配、并行执行或重试循环；这些在完整 Go 实现中由 DXF 框架完成，在当前 Rust 测试中则由 `FakeTaskTable` 和 `taskExecutor` 共同模拟/执行。

## 主要符号

- `StepInit = -1`、`StepOne = 1`、`StepTwo = 2`、`StepDone = -2`、`StepPrepared = -3`：阶段常量。`StepPrepared` 仅被 `Step2Str` 识别，当前 `GetNextStep` 不产生它。
- `schedulerImpl { task: Task, pub subtaskCount: i64 }`：保存构造时任务快照，以及从任务 meta 解出的每阶段子任务数量。`task` 为私有字段，`subtaskCount` 公开以支持当前 crate 的验证。
- `newScheduler(_ctx: Context, task: Task) -> schedulerImpl`：构造实例；忽略 context，并把 `subtaskCount` 初始化为 `0`。
- `Init(&mut self) -> Result<()>`：调用 `taskMeta::Unmarshal(&self.task.Meta)`；成功后写入 `subtaskCount`，失败时包装成 `ExecutorError("unmarshal task meta failed: ...")`。
- `OnNextSubtasksBatch(..., task: &Task, ..., next_step: i64) -> Result<Vec<Vec<u8>>>`：仅接受 `StepOne` 或 `StepTwo`，按 `0..subtaskCount` 生成序列化后的 `subtaskMeta`。
- `GetNextStep(&self, task: &TaskBase) -> i64`：`Init` 后进入 `One`，`One` 后进入 `Two`，其余任何阶段都返回 `Done`。
- `Step2Str(task_type: &str, step: i64) -> String`：特殊阶段与任务类型无关；`example` 类型还识别 `one`、`two` 和未被状态机使用的 `three`。
- `postCleanImpl::Clean`：无状态、无操作并直接成功的清理实现。

所有方法均为固有方法；文件内没有 `trait impl`、条件编译项或异步函数。

## 执行流程

1. 上游用任务快照调用 `newScheduler`。此时调度器尚未解析 meta，`subtaskCount == 0`。
2. 调用 `Init`。`taskMeta::Unmarshal` 按 Go `encoding/json` 兼容规则读取 `subtask_count`；缺字段或 `null` 得到 `0`，格式/类型错误返回错误。
3. 框架或当前 Rust 测试把任务的 `TaskBase` 传给 `GetNextStep`：初始阶段得到 `StepOne`。
4. `OnNextSubtasksBatch` 校验 `subtaskCount >= 0`，为每个 `i` 构造消息 `subtask {i} of step one`，调用 `subtaskMeta::Marshal` 得到一条 JSON 字节数组，并收集成批次。
5. 下游 [`task_executor.rs`](task_executor.rs) 的 `stepExecutor::RunSubtask` 反序列化这些字节并记录消息；`app_test.rs` 验证这一批全部进入 `Succeed`。
6. 当前阶段变为 `StepOne` 后，`GetNextStep` 返回 `StepTwo`；再次生成同样数量、消息阶段名为 `two` 的子任务并执行。
7. 当前阶段为 `StepTwo`（或任何未单独匹配的值）时，`GetNextStep` 返回 `StepDone`。随后 `OnDone` 与 `postCleanImpl::Clean` 均直接返回成功。

`OnTick` 和 `OnPrepare` 不改变上述流程；`GetEligibleInstances` 返回空列表，示例自身不选择节点。

## 数据与状态

调度器拥有构造时 `Task` 的值拷贝/所有权，而批次生成方法另外接收一个 `&Task`：前者只供 `Init` 读取 `Meta`，后者供 `OnNextSubtasksBatch` 读取 `TaskBase.Type` 来格式化消息。调用方若传入不同任务，数量仍来自构造时任务，类型名则来自方法参数；完整接线时应保持二者代表同一逻辑任务。

唯一可变运行状态是 `subtaskCount`。它在构造时为零，在 `Init` 成功后一次性取自 `taskMeta.SubtaskCount`。批次方法只读该值，不记录已生成位置，因此重复调用会重新生成完全相同的批次；这是示例的幂等形态，但持久化和去重责任不在本文件。

每条子任务 meta 是独立的 `Vec<u8>`，由 `subtaskMeta::Marshal` 产生；序号从 `0` 到 `subtaskCount - 1`。数量为零时返回空批次。`eligible_instances: &[String]` 参数当前被忽略，节点数量不会影响分片数。

## 依赖与调用关系

直接依赖如下：

- crate 内 [`proto.rs`](proto.rs)：`taskMeta::Unmarshal` 解析任务参数；`subtaskMeta::Marshal` 编码下游子任务消息。
- `astersql-dxf-framework-taskexecutor`：提供 `Context`、`Task`、`TaskBase`、`ExecutorError` 和统一 `Result`。这是 `Cargo.toml` 中当前启用的路径依赖。
- [`lib.rs`](lib.rs)：装配并公开本文件符号；测试模块也从 crate 根导入它们。

RustCodeGraph 的文件关系显示 `scheduler.rs` 被 [`app_test.rs`](app_test.rs) 使用。该测试的实际调用链是 `newScheduler -> Init -> GetNextStep -> OnNextSubtasksBatch -> taskExecutor::Run -> OnDone -> postCleanImpl::Clean`。生产式的上游注册调用目前只存在于 Go 的 [`app_test.go`](app_test.go)：它注册 scheduler factory、cleaner factory 和 task executor factory，再通过 `handle.SubmitTask`/`WaitTaskDoneByKey` 驱动完整框架。

`Cargo.toml` 声明了 `astersql-util-logutil`，但本文件没有使用它；Go 对照实现会在 tick、done、clean 时记录日志，Rust 本文件当前不会。

## 错误处理与边界

- `Init` 是主要的可恢复错误边界。无效 JSON、错误字段类型或越界整数由 `taskMeta::Unmarshal` 返回字符串错误，再转为带上下文的 `ExecutorError`；失败时不会更新 `subtaskCount`。
- `OnNextSubtasksBatch` 的返回类型允许错误，但当前 `subtaskMeta::Marshal` 不失败，所以合法阶段只返回 `Ok`。
- 负的 `subtaskCount` 会触发断言 panic，消息为 `makeslice len out of range`，模拟 Go `make([][]byte, negative)` 的运行时失败。`app_test.rs::scheduler_negative_subtask_count_matches_go_make` 固化了该边界。
- `StepOne`/`StepTwo` 之外的 `next_step` 会 panic；`scheduler_unknown_step_error_matches_go` 验证 `99` 被格式化为 `unknown step 99`。
- `GetNextStep` 对 `StepInit`、`StepOne` 之外的任何值均返回 `StepDone`，包括 `StepPrepared` 和未知值。调用方必须确保当前状态合法；该函数本身不拒绝非法状态。
- `Step2Str` 对特殊阶段优先返回固定名称；非 `example` 业务阶段返回 `unknown type ...`，`example` 未知阶段返回 `unknown step ...`。
- `IsRetryableErr` 对所有 `ExecutorError` 返回 `true`，没有错误分类；扩展为真实业务时必须重新评估永久错误，避免无限重试。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。方法接收共享借用或可变独占借用，Rust 类型系统保证 `Init` 修改 `subtaskCount` 时不能并发读取同一实例；但 `schedulerImpl` 没有声明或实现额外的跨线程同步协议。

`Context` 在构造、tick、批次、prepare、done 和 clean 接口中均未使用，因此本文件不响应取消或超时。任务快照随 `schedulerImpl` 生命周期持有并在实例析构时自然释放；批次中的 `Vec<u8>` 所有权交给调用方。`postCleanImpl` 不持有资源且 `Clean` 无副作用，所以可重复调用。

当前端到端测试用 `Mutex` 保护内存任务表和全局注册表，但这些锁位于 `app_test.rs`，不是调度器实现的一部分。完整 Go 框架负责后台循环、存储事务和节点调度；不能从本 Rust 文件推断这些能力已接线。

## 与 Go 版本的对应关系

直接对照文件是 [`scheduler.go`](scheduler.go)。两端保留了相同的业务状态机、每阶段固定 `SubtaskCount`、`subtask {i} of step {name}` 消息、空 eligible instances、恒可重试以及无业务准备/完成/清理逻辑。

主要差异是：

- Go `schedulerImpl` 嵌入 `BaseScheduler`，在 `Init` 中设置 `Extension = s` 并调用 `BaseScheduler.Init()`；Rust 结构只保存 `Task`，没有基类初始化或 scheduler trait 接线。
- Go 构造器接收 `scheduler.Param` 并创建带 task ID 的 logger；Rust 构造器只有 `Context` 与 `Task`，忽略 context，也没有 logger。
- Go `OnTick`、`OnDone`、`Clean` 会写日志；Rust 对应方法是无操作成功。
- Go `OnNextSubtasksBatch` 还接收 `storage.TaskHandle`；Rust 当前签名省略该句柄，且 Cargo 中 scheduler/storage 依赖被 `cfg(any())` 禁用。
- Go 使用框架 `proto.Step2Str`；Rust 本文件局部复制了阶段常量与 `Step2Str` 逻辑。框架 crate 另有 `pkg/dxf/framework/proto/step.rs`，未来接线时要避免两份定义漂移。
- Go 的 `TestExampleApplication` 通过完整 DXF 注册、提交和等待任务；Rust `test_example_application` 明确使用内存 `FakeTaskTable` 手工推进 scheduler，再调用真实 task executor。这验证了本 crate 内部链路，但不等价于完整 scheduler/handle/storage 集成验证。

## 扩展指南

- 新增业务阶段时，应同步修改阶段常量、`GetNextStep`、`OnNextSubtasksBatch` 的允许分支和 `Step2Str`，并在独立测试文件 [`app_test.rs`](app_test.rs) 增加阶段推进、批次数量和消息内容验证；不要把测试内嵌进 `scheduler.rs`。
- 改变分片策略时，主要接入点是 `OnNextSubtasksBatch`。若开始使用 `eligible_instances`，需明确空列表语义、最大分片数和稳定排序，避免节点变化导致不可重复的 meta。
- 扩展 `taskMeta` 或 `subtaskMeta` 时应修改 [`proto.rs`](proto.rs)，并同步 [`proto_test.rs`](proto_test.rs) 的 Go JSON 兼容测试及 `app_test.rs` 的端到端往返测试。
- 接入完整 Rust DXF 时，应优先复用框架的 step/trait/base scheduler 定义，恢复 `Cargo.toml` 中当前禁用的 scheduler、storage、handle 依赖，并对照 Go `scheduler.go` 保留 base initialization、task handle、日志和注册语义。该工作超出本文件当前事实，不能只靠增加固有方法宣称完成。
- 若错误不再全部可重试，应在 `IsRetryableErr` 中按错误类别区分，并补充永久错误不会进入重试循环的独立回归测试。
- 性能风险集中在一次性分配 `subtaskCount` 个 `Vec<u8>`：真实业务应约束数量或做分页生成；兼容风险集中在局部 step 数值/字符串与框架 proto 的同步，以及消息 JSON 与 Go `encoding/json` 的一致性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/dxf/example/scheduler.rs`（21 个符号），`files --filter pkg/dxf/example` 显示本文件由 `pkg/dxf/example/app_test.rs` 使用；`query` 区分了 Rust/Go 的 `schedulerImpl`、`newScheduler` 和 `Step2Str`。对 callers/callees 的精确查询在 30 秒内无结果，调用关系因此以直接引用搜索和测试源码核验。
- 目标源码：[`scheduler.rs`](scheduler.rs)，核对 5 个 step 常量、`schedulerImpl`、9 个调度/格式化方法和 `postCleanImpl`，以及 panic/错误分支。
- crate 边界：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`doc.go`](doc.go)，核对路径依赖、禁用的完整 DXF 依赖、模块再导出和示例目的。
- 数据协议与下游：[`proto.rs`](proto.rs)、[`task_executor.rs`](task_executor.rs)，核对 meta 编解码以及生成字节的消费方式。
- Rust 测试：[`app_test.rs`](app_test.rs) 覆盖两阶段各 3 个子任务、状态推进、meta 往返、done/clean、未知阶段 panic 和负数量 panic；[`task_executor_test.rs`](task_executor_test.rs) 覆盖下游 meta 解析成功/失败。未发现独立的同名 `scheduler_test.rs`。
- Go 对照：[`scheduler.go`](scheduler.go)、[`app_test.go`](app_test.go)，核对 BaseScheduler、注册、完整框架驱动、日志及清理差异。
- 本任务按计划仅做文档分析，未运行 Cargo；交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节。

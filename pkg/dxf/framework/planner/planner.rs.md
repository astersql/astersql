# [`pkg/dxf/framework/planner/planner.rs`](planner.rs)

## 文件定位

本文件是 `astersql-dxf-framework-planner` crate 的任务创建入口，位于 DXF（Distributed eXecution Framework）逻辑计划与任务存储之间。crate 入口 `pkg/dxf/framework/planner/lib.rs` 以 `pub mod planner` 声明该模块，并通过 `pub use planner::*` 导出这里的 `Planner`、`TaskCreator` 和 `new_planner`。

它不负责生成处理器 DAG 或子任务；这些能力属于相邻的 `plan.rs` 中的 `LogicalPlan::to_physical_plan`、`PhysicalPlan` 和 `PipelineSpec`。本文件只消费 `LogicalPlan` 的任务级 meta 与额外参数，再把 `PlanCtx` 中的任务创建字段转交给存储层。

`pkg/dxf/framework/planner/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/dxf/framework/planner`，直接依赖 `framework/handle`、`framework/proto` 和 `framework/storage`。可选的 `kv-runtime` feature 只改变 `plan.rs` 的 `PlanStore` 类型，本文件没有条件编译分支。

## 核心职责

1. 通过 `Planner::run` 按固定顺序序列化逻辑计划、解析当前 target scope，然后创建分布式任务。
2. 通过 `Planner::run_with_target_scope` 接受调用方已经解析的 scope，为测试或无需读取全局 runtime 的场景提供确定性入口。
3. 通过 `TaskCreator` trait 把任务创建行为抽象为可注入边界，同时为真实的 `storage::TaskManager` 提供适配实现。
4. 通过 `to_storage_extra_params` 在 planner proto 与 storage proto 之间逐字段转换 `ExtraParams`，避免跨 crate 同形类型被隐式混用。

本文件不执行任务、不调度 worker，也不构建物理计划；其成功结果只是存储层返回的新任务 ID。

## 主要符号

- `pub trait TaskCreator`：任务创建端口。`create_task_with_session` 接收请求/会话上下文、任务键和类型、keyspace、slot 数、target scope、最大节点数、额外参数与 meta，返回 `Result<i64, PlannerError>`。参数顺序刻意对齐 `storage::TaskManager::CreateTaskWithSession`。
- `pub(crate) fn to_storage_extra_params(...)`：将 `proto::ExtraParams` 的 `ManualRecovery`、`PauseOnKVDiskFull`、`MaxRuntimeSlots`、`TargetSteps`、`PrepareMode` 全部复制到 `storage::proto::ExtraParams`。它是 crate 内部函数，不构成外部 API。
- `impl TaskCreator for storage::TaskManager`：真实存储适配器；完成参数类型转换后直接调用 `CreateTaskWithSession`，不吞掉或改写错误。
- `pub struct Planner`：无字段、可复制的无状态规划器，派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq` 和 `PartialEq`。
- `pub fn new_planner() -> Planner`：返回零大小的 `Planner` 值；RustCodeGraph 中该构造器的直接调用者是本目录的两个 planner 单元测试。
- `Planner::run(...)`：标准入口，内部获取 target scope。
- `Planner::run_with_target_scope(...)`：由调用方传入 target scope 的入口。
- `Planner::create(...)`：私有汇聚点，负责从 `PlanCtx` 与 `LogicalPlan` 组装 `TaskCreator` 调用参数。

## 执行流程

标准路径 `Planner::run` 的顺序如下：

1. 调用 `LogicalPlan::to_task_meta()` 生成任务级 meta；失败时通过 `?` 立即返回。
2. 调用 `handle::GetTargetScope()`。Rust 实现从已安装的 handle runtime 读取 scope；next-gen runtime 固定返回 `NEXT_GEN_TARGET_SCOPE`，其他 runtime 返回 service scope。runtime 不可用时返回错误。
3. 调用私有 `Planner::create`。
4. `create` 从 `PlanCtx` 移出 `ctx`、`session_ctx`、`task_key`、`task_type`、`keyspace`、`thread_count` 和 `max_node_count`，并在此时调用 `plan.get_task_extra_params()`。
5. 调用 `TaskCreator::create_task_with_session`，原样传递 target scope 和已序列化的 meta，向上返回任务 ID 或错误。

`run_with_target_scope` 与上述流程共享第 1、3—5 步，但跳过第 2 步。无论使用哪个入口，meta 都先于额外参数生成；`planner_returns_serialization_error_without_creating_task` 用会在读取额外参数时 panic 的 `FailingPlan` 证明序列化失败会短路后续创建。

当 `TaskCreator` 是真实 `storage::TaskManager` 时，适配实现先逐字段转换额外参数，再进入 `pkg/dxf/framework/storage/task_table.rs::CreateTaskWithSession`。后者负责检查 required slots 不超过节点 CPU 数、序列化额外参数、插入 `mysql.tidb_global_task`，最后查询 `@@last_insert_id`；这些持久化细节不在本文件实现。

## 数据与状态

`Planner` 自身没有字段或可变状态。同一个实例可重复调用，调用之间不会在本文件中保存任务信息。

输入状态主要来自 `PlanCtx`（定义于 `plan.rs`）：

- 实际用于创建的字段为 `ctx`、`session_ctx`、`task_key`、`task_type`、`keyspace`、`thread_count` 和 `max_node_count`。
- `task_id`、`previous_subtask_metas`、`global_sort`、`next_task_step`、`execute_nodes_count` 与 `store` 不由本文件读取，它们服务于其他规划阶段。
- `thread_count` 被不经计算地映射为存储层 `required_slots`；本文件不检查正数、CPU 上限或节点容量。

计划相关数据来自动态分派的 `&dyn LogicalPlan`：`to_task_meta` 提供持久化字节，`get_task_extra_params` 提供任务控制参数。所有权在 `create` 中被转移给 `TaskCreator`，本文件不缓存 meta、scope 或上下文。

## 依赖与调用关系

上游 API 关系为：`lib.rs` 公开再导出本模块，调用方构造 `Planner` 后传入 `PlanCtx`、`LogicalPlan` 与任务创建器。当前 RustCodeGraph 对 `new_planner` 查到的直接调用者仅为 `pkg/dxf/framework/planner/planner_test.rs` 中的两个单元测试；没有从该证据确认生产代码通过该构造器接线，因此不能据此声称 Rust 主应用已使用此入口。

本文件的直接下游为：

- `crate::plan::{LogicalPlan, PlanCtx, PlannerError}`：定义计划协议、输入上下文，以及别名到 `storage::Error` 的错误类型。
- `astersql_dxf_framework_handle::GetTargetScope`：为标准入口解析执行范围。
- `astersql_dxf_framework_proto`：提供任务类型与 planner 侧 `ExtraParams`。
- `astersql_dxf_framework_storage`：提供请求上下文、会话上下文、错误、存储侧 `ExtraParams` 和真实 `TaskManager`。

关键调用边为 `Planner::run → LogicalPlan::to_task_meta → handle::GetTargetScope → Planner::create → TaskCreator::create_task_with_session`；确定性路径为 `Planner::run_with_target_scope → LogicalPlan::to_task_meta → Planner::create`。真实适配边继续到 `storage::TaskManager::CreateTaskWithSession`。

## 错误处理与边界

`PlannerError` 是 `storage::Error` 的类型别名，所以序列化、handle runtime 与存储创建错误沿同一 `Result<i64, PlannerError>` 通道传播。本文件所有可失败步骤均使用 `?` 或直接返回下游结果，不包装错误，也没有重试或补偿逻辑。

边界行为包括：

- `to_task_meta` 失败时，不读取 target scope（标准入口）、不读取额外参数，也不调用任务创建器。
- `GetTargetScope` 失败时，meta 已经生成，但尚未读取额外参数或创建任务。这是 Rust 相比 Go 的新增错误边界，因为 Go 的 `GetTargetScope()` 直接返回字符串。
- `get_task_extra_params` 在私有 `create` 内同步调用；trait 签名不允许返回错误。如果实现 panic，本文件不捕获。
- `TaskCreator` 返回的 CPU 容量错误、序列化错误、SQL 错误或任务 ID 查询错误均原样向上传播。
- 本文件不验证空任务键、空 keyspace、slot 数或最大节点数；真实有效性由调用者与存储层契约保证。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或资源池。`Planner` 无状态，接口只借用 `LogicalPlan` 与 `TaskCreator`；一次调用结束后不保留这些引用。

`PlanCtx` 中的请求上下文和会话上下文按值传入并移动到任务创建边界。真实存储实现会克隆上下文以执行 CPU 查询和 SQL，但其会话、SQL 执行及事务生命周期属于 `framework/storage`，不由 `Planner` 管理。

测试中的 `Creator(Mutex<Option<Call>>)` 仅用于安全记录一次调用参数，不代表生产 planner 内部有互斥状态。Go 测试创建并关闭 session resource pool；Rust 单元测试改用注入的 `TaskCreator`，因此不拥有资源池清理职责。

## 与 Go 版本的对应关系

Go 对照为 `pkg/dxf/framework/planner/planner.go`。两版均使用无状态 `Planner`，并保持以下核心顺序：先 `ToTaskMeta`，再确定 target scope，最后将 `PlanCtx` 字段、额外参数与 meta 传给 `CreateTaskWithSession`。字段映射保持 `ThreadCnt/thread_count → requiredSlots/required_slots`，并保留 keyspace 与最大节点数。

Rust 版为了 crate 与测试边界作了三项显式适配：

1. `TaskCreator` trait 替代 Go 中固定的 `*storage.TaskManager` 参数，使测试不需要真实 session 或全局 TaskManager。
2. `run_with_target_scope` 允许跳过全局 handle runtime；Go 文件没有此公开入口。
3. Rust `handle::GetTargetScope` 返回 `Result<String>`，而 Go 版直接返回 `string`，因此 Rust 标准入口多一个 runtime 获取失败分支。

Rust 还需要用 `to_storage_extra_params` 连接 planner proto 与 storage proto 两个 crate 的同形类型。`planner_test.rs` 验证所有五个字段均保留；Go 测试 `planner_test.go::TestPlanner` 通过真实 mock store 验证 required slots、任务类型和 `ManualRecovery` 已落库。Rust 测试验证参数转发与序列化短路，但没有在此文件的独立测试中复现 Go 测试的真实存储查询。

## 扩展指南

- 新增任务创建字段时，应同步修改 `TaskCreator::create_task_with_session`、`impl TaskCreator for storage::TaskManager`、`Planner::create` 的映射，以及 `planner_test.rs` 的 `Call` 捕获与断言；同时核对 Go `Planner.Run` 和 storage 的 `CreateTaskWithSession` 签名。
- `proto::ExtraParams` 新增字段时，必须更新 `to_storage_extra_params` 和 `storage_extra_params_preserve_all_fields`，否则字段会在跨 crate 边界静默丢失。还需检查 storage proto 的序列化兼容性。
- 若调整调用顺序，应保留或明确变更“meta 序列化失败不读取额外参数、不创建任务”的契约，并在独立的 `planner_test.rs` 中增加顺序或短路回归测试；不要把测试写入生产源文件。
- 若需要异步创建、重试或补偿，应优先放在清晰的上层编排或存储边界，而不是给无状态 `Planner` 隐式增加全局状态。重试必须评估任务键幂等性，避免重复插入全局任务。
- 若生产代码要使用 `run_with_target_scope`，调用方必须证明 scope 与 handle runtime 的规则一致；否则可能改变 next-gen/service-scope 路由。
- 性能风险集中在 meta 序列化和存储调用。本文件对 `Vec<u8>`、`String` 与额外参数采用所有权转移，没有额外复制；新增日志或校验时应避免复制大型 meta。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 `planner.rs`、`plan.rs`、`planner_test.rs`、Go 对照与测试均已索引。
- RustCodeGraph `node --file pkg/dxf/framework/planner/planner.rs`：核对了本文件 153 行全貌、全部 trait/函数/impl、调用顺序和无条件编译事实。
- RustCodeGraph `query/node new_planner`：确认符号位置及两个 Rust 单元测试调用者。
- RustCodeGraph `query/node GetTargetScope`：确认 Go 返回字符串，Rust 返回 `Result<String>`，以及 Rust runtime 的 next-gen/service-scope 分支。
- RustCodeGraph `query/node CreateTaskWithSession`：确认真实 Rust/Go 存储边界的 CPU slot 校验、额外参数序列化、任务表插入和任务 ID 获取流程。
- `pkg/dxf/framework/planner/plan.rs`：核对 `PlanCtx`、`LogicalPlan` 与 `PlannerError` 的定义和字段语义。
- `pkg/dxf/framework/planner/lib.rs`：核对模块声明、公开再导出及测试文件以独立模块接入。
- `pkg/dxf/framework/planner/Cargo.toml`：核对 crate 名称、Go 包映射、直接依赖和 `kv-runtime` feature。
- `pkg/dxf/framework/planner/planner.go` 与 `planner_test.go`：核对 Go 的调用顺序、字段映射、真实存储测试及资源池清理。
- `pkg/dxf/framework/planner/planner_test.rs`：核对完整参数转发、固定任务 ID、序列化错误短路和全部额外参数字段转换。
- 本任务为纯文档分析，未运行 Cargo；最终使用任务文件指定的 11 章节命令验证结构，并人工检查本文只描述有上述直接证据支持的行为。

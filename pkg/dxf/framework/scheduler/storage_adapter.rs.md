# `pkg/dxf/framework/scheduler/storage_adapter.rs`

## 文件定位

本文件位于 `astersql-dxf-framework-scheduler` crate 内，由 `lib.rs` 以 `pub mod storage_adapter` 声明并通过 `pub use storage_adapter::*` 再导出。它是调度状态机与持久化任务表之间的适配层：对上实现 `interface.rs` 定义的 `TaskManager: Send + Sync`，对下持有 `astersql_dxf_framework_storage::TaskManager`，把 scheduler 自有的 `Task`、`TaskBase`、`Subtask` 等值类型转换为 storage/proto 类型后调用持久化 API。

该适配器是 Rust 迁移特有的接线。Go 版本的 `pkg/dxf/framework/scheduler/interface.go` 直接让 storage `TaskManager` 满足同一包内的 Go 接口，没有同路径 `storage_adapter.go`。Rust 的直接生产入口之一是 `pkg/session/runtime/modify_column_dist_backfill.rs`：它用 `StorageTaskManagerAdapter::new(manager.clone())` 构造 `Arc<dyn scheduler::TaskManager>`，随后交给节点、槽位和单任务调度流程。

## 核心职责

1. `StorageTaskManagerAdapter` 将 scheduler trait 的 33 个查询、状态迁移、清理、槽位和子任务方法逐一委托给 storage manager；事务性和 SQL 语义仍由 `pkg/dxf/framework/storage/task_table.rs` 负责，本文件不自行开启事务。
2. `from_base`、`to_base`、`from_task`、`to_task`、`from_subtask_base`、`to_subtask_base`、`to_subtask` 负责跨 crate 数据模型转换，保留调度真正使用的状态、步骤、优先级、资源、meta、错误和修改参数。
3. `error` 把 storage 错误压平为 scheduler 的字符串错误；`context` 为每次下游调用创建默认 storage context。
4. `task_type` 把动态 `String` 驻留为 `&'static str`，满足 proto 中任务类型/修改类型的静态字符串表示。

因此本文件存在的主要原因是隔离两个 Rust crate 的类型和错误边界，而不是重新实现任务表算法。

## 主要符号

- `static TYPES: LazyLock<Mutex<HashMap<String, &'static str>>>`：进程级字符串驻留表。相同文本复用同一个静态引用；新文本经 `Box::leak` 永久保留。
- `task_type(&str) -> &'static str`：在互斥锁内查询或插入驻留值。锁中毒时通过 `expect("task type interner poisoned")` 直接 panic。
- `error(storage::Error) -> SchedulerError`：仅保留 `Display` 字符串，不保留 storage 错误的结构化类型或来源链。
- `context() -> storage::Context`：返回 `storage::Context::default()`；调用者传入的 scheduler `Context` 不会经过此适配层传到 storage。
- `from_base` / `to_base`：双向转换任务基础字段，包括 `ExtraParams` 和 `keyspace`。
- `from_task` / `to_task`：转换完整任务、meta、错误、前一状态和修改列表。`to_task` 还把 `SchedulerID` 置空，并把 `StartTime`、`StateUpdateTime` 置为 Unix epoch。
- `from_subtask_base` / `to_subtask_base` / `to_subtask`：转换子任务标识、步骤、状态、执行节点、并发度、序号和 meta；写方向的创建/更新时间用 Unix epoch，summary 置空。
- `pub struct StorageTaskManagerAdapter { pub manager: storage::TaskManager }`：可克隆的公开适配器，`new` 仅保存 manager。
- `impl TaskManager for StorageTaskManagerAdapter`：实现查询、节点维护、历史清理、任务终态/暂停/恢复、切步、槽位统计、子任务恢复/重分配，以及前序 meta/summary 读取。

## 执行流程

典型流程是：业务代码取得 storage manager，调用 `StorageTaskManagerAdapter::new`，再将其包成 `Arc<dyn TaskManager>` 交给 `scheduler::Manager`、`NodeManager`、`SlotManager` 或 `Param`。调度器通过 trait 方法发起操作，适配器即时创建默认 storage context，必要时转换入参，调用同名或语义对应的 storage 方法，再转换返回值或错误。

查询路径例如 `top_unfinished_tasks` 调用 `GetTopUnfinishedTasks`，将每个 `proto::TaskBase` 经 `from_base` 转成 scheduler `TaskBase`；`all_tasks`、`all_subtasks` 和两类 previous 查询把 storage 的 `Option<Vec<_>>` 中 `None` 规范化为空向量。`tasks_in_states` 先把静态状态字符串转换成 `storage::Value::String`，再将完整任务逐项转换。

写路径例如 `switch_task_step` 和 `switch_task_step_in_batch`：先由 `to_task` 构造 storage 任务，再根据父任务的 `task_type` 逐个构造 storage 子任务，最后把任务状态、步骤和子任务一起交给对应 storage API。`switch_task_step_after_prepare` 原样返回 storage 层 CAS 的布尔结果，`false` 表示状态/owner 竞争导致零行命中，而非错误。`fail_task`、`revert_task`、`awaiting_resolve_task` 和 `pause_task_on_error` 把 `SchedulerError` 文本重新包装成 `storage::Error`。

读取上一阶段结果时，`previous_subtask_metas` 固定查询 `SubtaskStateSucceed` 的 meta；`previous_subtask_summaries` 只将 storage summary 的 `RowCount` 映射为 scheduler `SubtaskSummary::row_count`。

## 数据与状态

任务基础数据在两侧基本一一对应：`id/key/type/state/step/priority/required_slots/target_scope/create_time/max_node_count/extra_params/keyspace` 均被复制。完整任务另外保留 `meta`、可选错误文本、`ModifyParam.PrevState` 与修改列表。子任务基础数据保留 `id/task_id/step/state/exec_id/concurrency/ordinal`。

写回 storage 时有意构造的是调度操作所需快照，而非无损序列化：`to_task` 不保留 scheduler ID 和两个时间字段，`to_subtask_base` 不保留创建/开始时间，`to_subtask` 不保留更新时间和 summary。这些字段应由 storage 事务/数据库侧规则维护；新增调用若依赖它们，不能直接假设当前转换已覆盖。

全局 `TYPES` 是本文件唯一自有的长期可变状态。它以任务类型或 modification kind 文本为键，持有泄漏后的静态字符串；条目只增不减。storage manager 自身的连接/session、表数据和事务状态属于下游 crate，不由适配器复制。

## 依赖与调用关系

crate 边界由 `pkg/dxf/framework/scheduler/Cargo.toml` 确认：本 crate 直接依赖 `astersql-dxf-framework-proto`、`schstatus`、`storage` 和 `dxfmetric`；本文件实际使用 scheduler `interface`、storage crate 及其 `proto` 再导出、标准库集合/同步/时间类型。Cargo 没有为本适配器单设 feature。

上游包括 `pkg/session/runtime/modify_column_dist_backfill.rs` 的真实分布式改列调度接线，以及 `pkg/session/runtime_test/ddl.rs`、`pkg/dxf/framework/handle/handle_test.rs`、`pkg/dxf/importinto/clean_up_test.rs`、`tests/realtikvtest/importintotest4/recorded_summary_harness.rs` 中构造真实 scheduler Manager 或直接读取任务快照的路径。`lib.rs` 的公开再导出使这些调用均可从 scheduler crate 根访问该类型。

下游是一组 `storage::TaskManager` 方法，主要落在 `pkg/dxf/framework/storage/task_table.rs`：任务筛选与排序、节点和槽位查询、任务转历史表、状态转换、原子切步、批量插入子任务、子任务状态统计及 summary/meta 查询。调度器侧消费这些方法的位置分布在 `scheduler_manager.rs`、`scheduler.rs`、`nodes.rs`、`slots.rs` 和 `balancer.rs`；适配器只维持边界，不改变其控制流。

## 错误处理与边界

所有 storage `Result` 都通过 `map_err(error)` 转为 `SchedulerError(error.to_string())`，所以成功/失败边界得到保留，但下游错误类型、可供匹配的变体和来源链会丢失。四个接收 scheduler error 的状态迁移则执行反向字符串包装；这同样不是结构化错误的往返转换。

`all_tasks`、`all_subtasks`、`previous_subtask_metas`、`previous_subtask_summaries` 将 storage 返回的 `None` 当作空集合；调用者不能据此区分“查询无行”和 storage API 用 `None` 表达的其他空结果。`subtask_errors` 还把单个空错误项转换为空字符串 `SchedulerError`。

适配器不会校验状态迁移合法性、批次子任务稳定性、CAS 前置状态、排序或清理批次上限；这些约束在 storage 实现中执行。例如 Go `interface.go` 明确要求批量切步重试时子任务数量、顺序和内容稳定，prepare 切步的零行 CAS 是良性竞争。`task_type` 的锁中毒会 panic，且任意数量的不同动态类型会永久增加驻留内存，这是调用方应限制输入基数的边界。

## 并发与资源生命周期

`TaskManager` trait 要求 `Send + Sync`，适配器可 `Clone`；克隆只克隆底层 `storage::TaskManager`，不会复制持久化数据。实际共享语义取决于 storage manager 的内部实现，scheduler 通常将适配器放进 `Arc<dyn TaskManager>` 供 manager 循环、节点刷新、槽位更新和单任务 scheduler 共同访问。

`TYPES` 使用进程级 `Mutex` 序列化驻留表访问，静态初始化由 `LazyLock` 保证只执行一次。锁只覆盖哈希表查询/插入，但每个新字符串的堆内存因 `Box::leak` 持续到进程结束；没有回收阶段。

每次 storage 调用都使用新建的默认 context。scheduler `Context` 的取消标志和生命周期不会穿过此接口，因此持久化调用本身不能借此适配器响应上层取消；超时、事务释放、session 归还和 SQL 连接资源均由 storage 方法负责。适配器不生成线程、任务或通道，也不持有显式事务守卫。

## 与 Go 版本的对应关系

Go 对照入口是 `pkg/dxf/framework/scheduler/interface.go` 的 `TaskManager` 接口和 `pkg/dxf/framework/storage/task_table.go` 的实现。Rust trait 使用 snake_case 和 scheduler 自有值类型；本适配器把它们映射到 Go 风格命名的 Rust storage API，例如 `top_unfinished_tasks -> GetTopUnfinishedTasks`、`switch_task_step_in_batch -> SwitchTaskStepInBatch`、`previous_subtask_summaries -> GetAllSubtaskSummaryByStep`。

主要语义保持一致：unfinished/no-resource 查询的筛选排序由 storage 决定；普通切步把任务更新和子任务写入视作一个存储操作；批量切步保留稳定重试约束；prepare 切步暴露零行 CAS；previous meta 只取成功子任务；节点已用槽位和子任务状态计数直接透传。

并非所有 Go 接口方法都在 Rust trait 中出现：例如 Go 的 session/transaction 回调以及面向外部请求的 cancel/pause 方法不由这个 scheduler trait 暴露。反之，Rust 将类型转换、默认 context 和错误字符串化显式放在适配器内。summary 模型也较窄：当前 scheduler 侧只保留 `RowCount`，Go storage 测试覆盖的 `Processed` 等字段不会由 `previous_subtask_summaries` 返回。上述差异应视为当前代码事实，而不是推断为完整等价。

## 扩展指南

新增 scheduler 存储能力时，应先在 `interface.rs::TaskManager` 定义最小 trait 方法，再在本文件实现与 storage API 的转换；若底层尚无能力，应在 storage crate 独立实现并测试，不能在适配器中复刻事务逻辑。新增字段时需要同时审查 `from_*` 与 `to_*` 两个方向，并确认默认 epoch、空字符串或空 summary 是否仍符合该 storage 方法的更新列集合。

涉及新任务状态或修改类型时，确认其字符串生命周期是否必须为 `&'static str`；若输入可能高基数或来自用户数据，不应未经评估继续使用永久驻留。需要保留错误类别时，应先调整跨 crate 错误契约，不能只依赖 `to_string()`。需要取消传播时，应明确设计 scheduler context 到 storage context 的桥接，而不是继续调用无参数的 `context()`。

测试应保持与源码分离。转换层的新增回归优先放在独立 `storage_adapter_test.rs`（当前 scheduler 目录尚无该文件）并从 `lib.rs` 的 `#[cfg(test)]` 模块声明接入；底层 SQL、事务/CAS、排序和 summary JSON 语义继续同步 `pkg/dxf/framework/storage/table_test.rs` 与 Go 的 `table_test.go`。完整接线可扩展现有 `pkg/session/runtime_test/ddl.rs`、`pkg/dxf/importinto/clean_up_test.rs` 或相应 scheduler 独立测试，避免把测试嵌入本生产文件。

兼容风险主要是遗漏字段或改变空结果/错误映射；正确性风险集中在状态与步骤转换、批次稳定性和 CAS 结果；性能风险集中在批量转换的复制、逐次分配，以及 `TYPES` 的全局锁和不可回收字符串。改动后应分别验证转换单元、storage 事务语义和一条真实 Manager 调度链。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引，并显示由 scheduler/interface、scheduler_manager、handle/status 等 16 个文件关联使用。
- RustCodeGraph `node --file pkg/dxf/framework/scheduler/storage_adapter.rs`（1–443 行）：核对全部模块状态、转换函数、公开结构和 33 个 trait 方法实现；`query StorageTaskManagerAdapter --kind struct` 定位结构体于第 144 行。对 `StorageTaskManagerAdapter::new` 的静态 callers/callees 查询未返回可靠边，故直接调用点用仓库文本搜索补充，未据此臆造调用图。
- 源码与边界：`pkg/dxf/framework/scheduler/storage_adapter.rs`、`interface.rs`、`lib.rs`、`Cargo.toml`；底层实现 `pkg/dxf/framework/storage/task_table.rs`；Go 对照 `pkg/dxf/framework/scheduler/interface.go` 和 `pkg/dxf/framework/storage/task_table.go`。
- 上游调用：`pkg/session/runtime/modify_column_dist_backfill.rs`；独立 Rust 覆盖路径 `pkg/session/runtime_test/ddl.rs`、`pkg/dxf/framework/handle/handle_test.rs`、`pkg/dxf/importinto/clean_up_test.rs`、`tests/realtikvtest/importintotest4/recorded_summary_harness.rs`。
- 行为测试证据：`pkg/dxf/framework/storage/table_test.rs` 覆盖 summary 的空结果、row count 映射和坏 JSON 错误；Go 的 `pkg/dxf/framework/storage/table_test.go` 覆盖 unfinished/no-resource 排序与上限、prepare CAS、批量切步稳定性以及 summary 行为。当前没有同名独立 Rust adapter 测试，因此“所有转换字段均被逐项回归覆盖”未验证。
- 本任务是只读行为分析加文档，不运行 Cargo。交付前使用任务指定的 `rg` 结构命令确认恰有 11 个固定二级标题，并人工检查本文能回答文件位置、运行链路和安全扩展点。

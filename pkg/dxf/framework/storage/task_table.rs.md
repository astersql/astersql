# `pkg/dxf/framework/storage/task_table.rs`

## 文件定位

`task_table.rs` 是 `astersql-dxf-framework-storage` crate 中任务表与子任务表的核心持久化实现。crate 根 `pkg/dxf/framework/storage/lib.rs` 通过 `include!("task_table.rs")` 将它直接纳入同一模块，因此本文件可使用 crate 根定义的 `Error`、`Value`、`sessionctx`、`sqlexec`、`proto`、行转换函数以及测试 SQL 执行器，而不是一个拥有独立命名空间的子模块。`pkg/dxf/framework/storage/Cargo.toml` 声明该 crate，并提供 `nextgen` feature；该 feature 经 `astersql-config-kerneltype/nextgen` 影响 `GetDXFSvcTaskMgr` 的路由判断。

它处在 DXF 控制面的持久化边界：上游 Planner 通过 `pkg/dxf/framework/planner/planner.rs` 的 `TaskCreator for storage::TaskManager` 调用 `CreateTaskWithSession`；Scheduler 通过 `pkg/dxf/framework/scheduler/storage_adapter.rs` 调用任务查询、切步与清理接口；TaskExecutor 的轮询循环在 `pkg/dxf/framework/taskexecutor/manager.rs::handleTasks` 使用 `GetTaskExecInfoByExecID` 决定启动、暂停或回滚执行器。下游是 Session 池以及 `mysql.tidb_global_task`、`mysql.tidb_background_subtask` 和对应 history 表。

## 核心职责

1. 用 `TaskManager` 封装 Session 池，并通过 `WithNewSession`、`WithNewTxn`、`ExecuteSQLWithNewSession` 统一 Session 获取、内部请求标记、事务提交/回滚和资源归还。
2. 创建和读取全局任务：`CreateTask`/`CreateTaskWithSession` 写入 pending/init 任务；按 ID、key、状态以及是否包含 history 的多组查询将数据库行转换为 `proto::Task` 或 `proto::TaskBase`。
3. 驱动任务切步：`SwitchTaskStep` 在同一事务中以旧 state/step 为条件更新任务，并在 CAS 成功后插入新 subtask；`SwitchTaskStepAfterPrepare` 专门持久化 prepare 阶段；`SwitchTaskStepInBatch` 支持大批 subtask 的可恢复续插。
4. 提供子任务读取与统计：按执行节点、step、state 查询，汇总状态、错误、summary、row count，并在用户可见查询需要时合并活跃表和 history 表。
5. 维护辅助状态：checkpoint、summary、任务 `extra_params`、升级遗留的过大 concurrency，以及按 keyspace 的活跃任务计数。
6. 保留 Go 版的哨兵错误、故障注入和测试同步点，使上层可以区分“不存在”“状态不允许”“并发改变”等控制面结果。

## 主要符号

- 列常量 `basicTaskColumns`、`TaskColumns`、`InsertTaskColumns`、`basicSubtaskColumns`、`SubtaskColumns`、`InsertSubtaskColumns` 固定 SQL 列顺序；它们必须与 `row2TaskBasic`、`Row2Task`、`row2BasicSubTask`、`Row2SubTask` 的解码顺序同步。
- `maxSubtaskBatchSize: AtomicUsize` 默认是 16 MiB；`splitSubtasks` 同时受它与 `kv::TxnTotalSizeLimit` 的较小值约束。`setMaxSubtaskBatchSizeForTest` 只为独立测试改变该全局上限。
- `ErrUnstableSubtasks`、`ErrTaskNotFound`、`ErrTaskAlreadyExists`、`ErrTaskStateNotAllow`、`ErrTaskChanged`、`ErrSubtaskNotFound` 是 Go 风格哨兵错误；本文件直接产生 `ErrUnstableSubtasks` 与 `ErrTaskNotFound`，其余供同 crate 的任务/子任务状态实现共用。
- `Manager` 是应用侧最小接口；`SessionExecutor` 抽象 Session/事务闭包；`TaskHandle: SessionExecutor` 是 Scheduler 读取前一步 meta/summary 的边界。
- `TaskExecInfo` 将 `proto::TaskBase` 与当前执行节点上该任务的最大 subtask concurrency 配对。`ActiveTaskSummary` 保存任务总数及按 keyspace 分组结果。
- `TaskManager { sePool }` 是主要实现类型。`taskManagerInstance` 保存本地 TiDB 存储管理器；`dxfSvcTaskMgr` 保存 nextgen 用户 keyspace 指向 SYSTEM keyspace DXF 服务的管理器；二者通过 `AtomicGoPointer` 发布。
- `NewTaskManager`、`GetTaskManager`、`SetTaskManager`、`GetDXFSvcTaskMgr`、`SetDXFSvcTaskMgr` 管理上述实例。`GetDXFSvcTaskMgr` 仅在 nextgen 且当前不是 SYSTEM keyspace 时走服务管理器，否则回退本地实例。
- 创建入口是 `CreateTask` 和 `CreateTaskWithSession`；关键切步入口是 `SwitchTaskStep`、`SwitchTaskStepAfterPrepare`、`SwitchTaskStepInBatch`，内部辅助为 `updateTaskStateStep`、`insertSubtasks`、`splitSubtasks`。
- 查询入口包括 `GetTopUnfinishedTasks`、`GetTopNoNeedResourceTasks`、`GetTaskExecInfoByExecID`、`GetCleanupTasks`、任务 ID/key/history 系列方法，以及子任务 state/step/exec/history 系列方法。
- `serializeErr`、`serializeErrOption`、`unmarshalSubtaskError` 维护错误列 JSON 形状；`UpdateSubtaskCheckpoint`/`GetSubtaskCheckpoint` 和 `UpdateSubtaskSummary`/summary 查询维护恢复进度。

## 执行流程

创建任务时，`CreateTask` 先通过 `WithNewSession` 获取内部 Session，再转入 `CreateTaskWithSession`。后者先用 `getCPUCountOfNodeByRole` 检查 `requiredSlots` 不超过受管节点 CPU，序列化 `ExtraParams`，向 `mysql.tidb_global_task` 插入 pending/init 行，随后查询 `@@last_insert_id`。Planner 已持有 Session 时可直接调用 `CreateTaskWithSession`，避免重复取得 Session。

普通切步由 `SwitchTaskStep` 开启事务。如果 Session 的 `MemQuotaQuery` 低于默认值，`runWithRestoredSystemVar` 临时抬高配额，并保证动作成功或失败后都恢复。`updateTaskStateStep` 用 `id + 旧 state + 旧 step` 做条件更新；从 pending 首次启动时还设置 `start_time`。受影响行数为零表示别的 Scheduler 已切步或任务不存在，此时按 Go 语义成功返回且不插入 subtask；否则 `insertSubtasks` 将新 step 的 subtask 参数化批量写入，最后由 `WithNewTxn` 提交，任一步失败则回滚。

prepare 切步由 `SwitchTaskStepAfterPrepare` 将 pending/init 原子改为 pending/prepared，同时更新 meta、concurrency 和 max node count，并用 affected rows 告知调用者本次是否真正完成切换；它不创建 subtask，也不设置 `start_time`。

大批量切步由 `SwitchTaskStepInBatch` 先查询目标 step 已存在的 subtask 数。数量超过调用方提供列表时返回 `ErrUnstableSubtasks`；否则跳过已存在前缀，通过 `splitSubtasks` 按 meta 字节数分批续插，最后才更新任务 state/step。这条路径使用一个新 Session，但每批 INSERT 不是被一个外层显式事务整体包裹，设计重点是中断后可按已有数量继续，而不是全批原子性。

TaskExecutor 路径中，`GetTaskExecInfoByExecID` 先按 `exec_id` 聚合 pending/running subtask 的 `max(concurrency)` 和任务 ID，再查询处于 running/reverting/pausing 的任务基行并排序，组装 `TaskExecInfo`。`pkg/dxf/framework/taskexecutor/manager.rs::handleTasks` 随后据状态启动执行器或执行暂停、回滚处理。

历史查询中，`GetSubtasksWithHistory` 在同一事务快照内依次读活跃子任务表和 history 表并合并，以覆盖任务恰在查询期间被归档的情况；任务的 `Get*WithHistory` 方法使用 `UNION` 读取活跃与历史任务表。`GetSubtaskRowCount` 则对活跃和历史 summary 做 `UNION ALL` 后汇总 `$.row_count`。

## 数据与状态

任务主键是数值 `id`，但子任务表中的 `task_key` 按 `TaskIDToKey(taskID)` 绑定为十进制字符串；`pkg/dxf/framework/storage/table_test.rs::subtask_sql_binds_large_task_ids_as_decimal_strings` 覆盖了超过 JavaScript 安全整数范围的 ID，防止转换精度或参数类型漂移。任务基础列包含 key、类型、状态、step、优先级、concurrency、创建时间、scope、节点上限、extra params 和 keyspace；完整列另含开始/状态更新时间、meta、dispatcher、error 和 modify params。

新任务固定从 `TaskStatePending + StepInit + NormalPriority` 开始。首次从 pending 切步写 `start_time`，后续切步只更新 `state_update_time`。普通切步的旧 state/step 条件构成乐观并发控制不变量；调用者传入的 `task.Meta` 与状态变更一同持久化。

新 subtask 固定写为 pending，保存 step、字符串 task key、exec ID、meta、任务类型整数、concurrency、ordinal 和创建时间；checkpoint 与 summary 初始为 `{}`。`splitSubtasks` 只累计 `Meta.len()`，不估算 SQL、其他列或编码开销；单条 meta 超限时仍单独成批，且不会产生空批。

空查询结果的返回形状并不完全统一：`GetAllTasks`、`GetAllSubtasks`、`GetSubtasksWithHistory`、`GetAllSubtasksByStepAndState` 和 `GetAllSubtaskSummaryByStep` 用 `Option` 区分无行；多数筛选查询返回空 `Vec`；按唯一 ID/key 查询则返回 `ErrTaskNotFound`。扩展调用方时必须保留对应契约。

## 依赖与调用关系

RustCodeGraph 的文件节点显示 `task_table.rs` 被 34 个文件使用。主要上游链如下：

- `planner/planner.rs::TaskCreator::create_task_with_session` → `TaskManager::CreateTaskWithSession` → CPU 校验、JSON 序列化与 `tidb_global_task` INSERT。
- `scheduler/storage_adapter.rs::switch_task_step*` → `SwitchTaskStep` / `SwitchTaskStepInBatch` / `SwitchTaskStepAfterPrepare` → CAS 更新与 subtask 写入。
- `scheduler/storage_adapter.rs::cleanup_tasks` → `GetCleanupTasks` → 选取 failed/reverted/succeed 的有限批任务，供后续归档/清理。
- `taskexecutor/manager.rs::handleTasks` → TaskTable 边界的 `GetTaskExecInfoByExecID` → 按节点分配和驱动执行器。
- 同 crate 的 `task_state.rs` 与 `subtask_state.rs` → `serializeErrOption` / `serializeErr`，共享数据库 error 列格式。

主要下游依赖是 `util::SessionPool`、`sessionctx::Context`、`sqlexec::ExecSQL`、`proto` 状态与数据结构、`json` 编解码、`converter.rs` 的行转换，以及 `injectfailpoint`/`failpoint`。Cargo manifest 的直接依赖包括配置与 kerneltype、proto/scheduler-status crate、`serde`/`serde_json`；大量 Go 兼容基础类型由 crate 根 `lib.rs` 提供。该文件没有网络客户端或后台线程，所有外部副作用都通过 Session SQL 边界发生。

## 错误处理与边界

Session 池获取、SQL、事务提交、JSON 编解码和 CPU 查询错误均使用 `Result<_, Error>` 原样或经 `errors::Trace` 传播。`WithNewTxn` 在闭包成功时提交；闭包失败时用带 `kv::InternalDistTask` 来源的 background context 回滚，并返回原闭包错误。需要注意：提交本身失败会直接返回提交错误；回滚返回值被忽略，符合 Go 版清理语义。

`CreateTaskWithSession` 在请求 slots 大于节点 CPU 时、执行 INSERT 前返回带具体数量的错误。唯一键冲突如何映射为 `ErrTaskAlreadyExists` 不在本文件中完成，依赖更下层错误转换或其他状态实现，不能仅凭哨兵声明推断此处已分类。

`SwitchTaskStep` 的 affected rows 为零不是错误，而是并发 Scheduler 的幂等成功路径。`SwitchTaskStepInBatch` 只验证“已有数量不超过期望数量”，并按列表前缀续插；它不在本文件核对已存在行的顺序与内容，因此 `ErrUnstableSubtasks` 的检测范围仅是数量上溢。states 参数为空时，`GetTasksInStates`/`GetTaskBasesInStates` 显式返回空列表；若给 `GetSubtasksByExecIDAndStepAndStates` 或 `GetFirstSubtaskInStates` 传空 states，SQL 仍会生成一个占位符但没有对应参数，调用者应保证集合非空。

错误列的 NULL 或空字节由 `unmarshalSubtaskError` 解释为无错误；非空 JSON 解码失败会传播。`serializeErr` 对 PingCAP Error 保留堆栈消息、RFC code 与 MySQL code，对普通错误规范化为消息；序列化失败则按 Go 语义退化为空字节。checkpoint JSON 转 UTF-8 时使用 `unwrap_or_default`，当前 JSON 序列化正常只产生 UTF-8；若未来更换编码器，应重新审视静默空字符串边界。

## 并发与资源生命周期

`taskManagerInstance`、`dxfSvcTaskMgr`、`TestLastTaskID` 和 `maxSubtaskBatchSize` 使用原子存储；后两者的测试修改是进程全局状态，相关测试用互斥锁串行化。管理器本身可 `Clone`，克隆的是 SessionPool 句柄语义。

`WithNewSession` 的生命周期顺序是：故障注入 → 从池取值 → downcast 为 Session → 保存并提高 `TxnEntrySizeLimit` → 执行闭包 → 恢复限制 → 归还池 → 返回闭包结果。显式收尾对应 Go `defer`；扩展该函数时必须保证闭包错误也不能跳过恢复与归还。`runWithRestoredSystemVar` 同样保证切步动作失败后恢复 `MemQuotaQuery`，但恢复错误有意忽略。

`WithNewTxn` 用同一 Session 执行 `BEGIN`、业务闭包和 commit/rollback。普通 `SwitchTaskStep` 因此保证任务 CAS 与 subtask 插入共同原子；`UpdateSubtasksExecIDs` 的多行更新也共同原子。`SwitchTaskStepInBatch` 则刻意让各批次持久化后可恢复，不能向调用者承诺所有批次与最终切步原子提交。

`TestChannel` 是 failpoint 下的双次接收同步点，用于模拟另一个 Scheduler 在批量插入期间写入。生产路径没有常驻 goroutine、锁持有或 channel 通信；主要并发冲突靠 SQL 条件更新、数据库事务和续插计数处理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/storage/task_table.go`。Rust 基本保持相同的列清单、哨兵错误文本、Session/事务顺序、CPU 校验、任务排序、CAS 条件、history 合并、SQL 表名和 JSON 错误形状。Rust 使用 `Vec`/`Option`/`Result` 表达 Go slice/nil/error，用 `AtomicGoPointer` 与原子整数表达 Go `atomic.Pointer`/`atomic.Int64`，并以 `runWithRestoredSystemVar` 显式模拟 Go `defer`。

值得记录的当前差异：Rust `splitSubtasks` 仅在当前批非空时才因超限封批，因此首条 meta 已超限时产生一个单元素批；同路径 Go 当前代码无此非空判断，可能先产生空批，但 Rust 的 `task_table_test.rs::TestSplitSubtasks` 和独立 `task_table_2_aster_unit_test.rs::oversized_first_subtask_never_creates_an_empty_batch` 明确把“不产生空批”作为 Rust 边界契约。Rust 还增加 `serializeErrOption` 来映射 `Option<Error>`，Go 直接接受 nil error。Rust `TaskExecInfo` 内嵌语义改为具名 `TaskBase` 字段，但上层适配器维持相同信息。

Go 行为证据主要来自 `task_table_test.go::TestSerializeErr`、`TestSplitSubtasks` 以及 `table_test.go::TestSwitchTaskStep`、`TestSwitchTaskStepInBatch`、`TestGetSubtaskSummaries`。Rust 的轻量 SQL 记录测试位于 `table_test.rs`，基础错误/分批/系统变量恢复测试位于 `task_table_test.rs`，可独立运行的公开 API 测试位于 `task_table_2_aster_unit_test.rs`。测试逻辑没有嵌入生产文件。

## 扩展指南

- 增删任务或子任务列时，必须同时修改列常量、INSERT 参数、`converter.rs` 的行转换及 Go 对照实现；列顺序错位会静默把字段解码到错误属性。同步扩展 `task_table_test.rs`/`table_test.rs`，需要真实 SQL 行为时再对照 `table_test.go`。
- 新增任务状态或资源调度语义时，检查 `GetTopUnfinishedTasks`、`GetTopNoNeedResourceTasks`、`GetTaskExecInfoByExecID` 与 `GetCleanupTasks` 的状态集合，并同步 Scheduler adapter 与 TaskExecutor 状态分支；遗漏会让任务无法被扫描、占用错误 slots 或无法清理。
- 修改切步必须保留 `id + state + step` CAS、不插入重复 subtask 的 affected-rows 分支、首次开始时间规则，以及普通切步的事务原子性。批量切步若改变断点续插策略，需要增加“部分已插入”“已有数量过大”“超大首条”“中途失败重试”的独立回归测试。
- 新增按 task ID 的子任务 SQL 时使用 `TaskIDToKey` 绑定十进制字符串，并扩展 `table_test.rs::subtask_sql_binds_large_task_ids_as_decimal_strings`，避免大整数类型回退。
- 扩展错误字段时保留 PingCAP RFC/MySQL code 与普通错误的兼容 JSON；同步 `task_table_test.rs::TestSerializeErr`、`task_table_2_aster_unit_test.rs::serialize_error_matches_pingcap_json_shape` 和 Go `task_table_test.go::TestSerializeErr`。
- 引入新的全局管理器或 keyspace 路由时，应在 `GetDXFSvcTaskMgr` 附近实现，检查 Cargo `nextgen` feature，并增加不同 kernel/keyspace 组合的测试。不要绕过 Session 池直接长期持有 Session。
- 性能风险集中在动态 `IN` 列表、大批量 meta 克隆、逐条 exec ID 更新和活跃/history 双表扫描；优化前须保持参数绑定、排序和事务/快照语义，并用真实规模数据证明收益。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/framework/storage` 确认目标及 Go/Rust 测试文件；`node --file pkg/dxf/framework/storage/task_table.rs` 读取完整 1,311 行并报告 34 个使用文件；`query` 定位了 Rust `SwitchTaskStep`、`GetTaskExecInfoByExecID`、`CreateTaskWithSession` 以及同名 Go/测试符号。对精确符号执行 `callers`/`callees` 未在 30 秒内返回结果，因此调用边又由下列直接源码位置核实，而未臆测图结果。
- 目标实现：`pkg/dxf/framework/storage/task_table.rs`；crate 边界和 include 入口：`pkg/dxf/framework/storage/Cargo.toml`、`pkg/dxf/framework/storage/lib.rs`。
- 直接上游：`pkg/dxf/framework/planner/planner.rs`、`pkg/dxf/framework/scheduler/storage_adapter.rs`、`pkg/dxf/framework/taskexecutor/manager.rs`；共享错误调用：`pkg/dxf/framework/storage/task_state.rs`、`pkg/dxf/framework/storage/subtask_state.rs`。
- Go 对照：`pkg/dxf/framework/storage/task_table.go`；Go 测试：`pkg/dxf/framework/storage/task_table_test.go`、`pkg/dxf/framework/storage/table_test.go`。
- Rust 测试：`pkg/dxf/framework/storage/task_table_test.rs`、`pkg/dxf/framework/storage/task_table_2_aster_unit_test.rs`、`pkg/dxf/framework/storage/table_test.rs`。这些测试覆盖错误 JSON、分批边界、系统变量恢复、CAS 切步、summary 与大 task ID 字符串绑定。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务指定命令验证目标存在且恰有 11 个固定二级标题，并人工复查本文能回答文件为何存在、主流程如何运行、并发/错误边界以及安全扩展位置。

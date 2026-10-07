# `pkg/dxf/framework/storage/subtask_state.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-storage` crate，源码不是独立模块声明，而是由 `pkg/dxf/framework/storage/lib.rs` 的 `include!("subtask_state.rs")` 直接挂入 crate 根作用域。因此，文件中的 `impl TaskManager` 可直接使用 crate 根已经定义或引入的 `Context`、`Error`、`proto`、`sqlexec`、`injectfailpoint`、`TaskIDToKey`、`serializeErr`、`serializeErrOption` 与 `ErrSubtaskNotFound`。

它位于 DXF 分布式任务执行链的持久化边界：上层 `pkg/dxf/framework/taskexecutor/task_executor.rs` 与 `manager.rs` 驱动子任务执行、取消和暂停，`pkg/dxf/framework/scheduler/storage_adapter.rs` 驱动恢复；本文件把这些动作转换成对 `mysql.tidb_background_subtask` 的带条件 `UPDATE`。文件不定义新的结构体、枚举、trait、常量或条件编译项，只为 `TaskManager` 增加八个公开方法。

`pkg/dxf/framework/storage/Cargo.toml` 将库入口设为 `lib.rs`，关闭自动测试发现（`autotests = false`）和 doctest，并声明 `nextgen` feature；本文件自身没有 feature 分支。crate 直接依赖 `astersql-config`、`astersql-config-kerneltype`、`parking_lot`、`proto-crate`、`schstatus-crate`、`serde` 和 `serde_json`，其中本文件显式使用的 DXF 状态类型由 crate 根的 `proto` 兼容层提供。

## 核心职责

该文件集中维护子任务状态写路径，并通过 SQL 条件表达状态机边界和执行节点所有权：

- `StartSubtask` 仅允许持有匹配 `id + exec_id` 的执行节点把记录标为 `running`，并用受影响行数检测所有权已变化或记录不存在。
- `FinishSubtask` 写回执行产物 `meta`，将记录标为 `succeed`，同时结束计时。
- `FailSubtask`、`CancelSubtask` 只处理 `pending/running` 子任务，避免覆盖已经进入终态或暂停态的记录。
- `PauseSubtasks` 与 `ResumeSubtasks` 批量实现任务级暂停/恢复；恢复时清空旧错误。
- `RunningSubtasksBack2Pending` 在节点故障恢复时，将仍由指定节点持有且仍为 `running` 的子任务回退为 `pending`。
- `UpdateSubtaskStateAndError` 提供按 `id + exec_id` 的通用状态/错误写入口，供执行器根据取消、不可重试失败等结果落库。

这些函数只负责持久化，不在内存中维护状态机，也不负责挑选下一条子任务；筛选、重试和调度决策位于调用方。

## 主要符号

- `TaskManager::StartSubtask(ctx, subtaskID, execID) -> Result<(), Error>`：注入 1% 随机失败点后，在新事务中更新状态、`start_time` 和 `state_update_time`。执行后读取同一 Session 的 `StmtCtx().AffectedRows()`；为零时返回 `ErrSubtaskNotFound`。
- `TaskManager::FinishSubtask(ctx, execID, id, meta) -> Result<(), Error>`：注入失败点，经新 Session 写入 `meta`、`succeed`、更新时间和 `end_time`。它没有受影响行数校验。
- `TaskManager::FailSubtask(ctx, execID, taskID, err) -> Result<(), Error>`：`err == None` 时无副作用成功返回；否则序列化错误，只把匹配节点和任务的一条 `pending/running` 记录标为 `failed`（SQL 含 `limit 1`），并同时设置开始、更新、结束时间。
- `TaskManager::CancelSubtask(ctx, execID, taskID) -> Result<(), Error>`：把匹配节点和任务的全部 `pending/running` 记录标为 `canceled`，并更新三个时间字段。
- `TaskManager::PauseSubtasks(ctx, execID, taskID) -> Result<(), Error>`：把匹配节点和任务的 `running/pending` 记录批量标为 `paused`；不修改时间或错误字段。
- `TaskManager::ResumeSubtasks(ctx, taskID) -> Result<(), Error>`：不限定 `exec_id`，把任务的全部 `paused` 记录恢复为 `pending` 并将 `error` 清空。
- `TaskManager::RunningSubtasksBack2Pending(ctx, subtasks) -> Result<(), Error>`：空输入立即返回；非空输入在一个事务内逐条以 `id + exec_id + state=running` 为条件回退到 `pending` 并更新时间。
- `TaskManager::UpdateSubtaskStateAndError(ctx, execID, id, state, subTaskErr) -> Result<(), Error>`：把指定状态和经 `serializeErrOption` 转换的可选错误写到匹配 `id + exec_id` 的记录，并更新状态时间；不注入本文件的 1% 失败点，也不校验更新行数。

## 执行流程

正常执行主链由 `pkg/dxf/framework/taskexecutor/task_executor.rs` 展示：

1. `startSubtask` 在执行实际工作前通过重试包装调用 `StartSubtask`，以 `id + 当前 execID` 抢占/确认记录并进入 `running`。
2. 工作成功后，`finishSubtask` 可先写实时 summary，再重试调用 `FinishSubtask`，持久化 `meta` 并进入 `succeed`。
3. 工作取消或失败时，`markSubTaskCanceledOrFailed` 根据上下文取消原因和错误可重试性决定保持 `running`、写 `canceled`，或通过 `UpdateSubtaskStateAndError` 写 `failed` 及错误。
4. 执行器初始化或某个任务级错误需要标记一条子任务时，`failOneSubtask` 调用 `FailSubtask`；其 `limit 1` 让一次调用只消费一条活跃记录。

任务管理主链在 `pkg/dxf/framework/taskexecutor/manager.rs`：`handlePausingTask` 先取消本地执行器，再调用 `PauseSubtasks`；`handleRevertingTask` 先取消运行中的执行，再调用 `CancelSubtask`。调度器恢复任务时，`pkg/dxf/framework/scheduler/storage_adapter.rs::resume_subtasks` 调用 `ResumeSubtasks`。

故障恢复主链中，`task_executor.rs` 调用 `RunningSubtasksBack2Pending`。方法在同一 `WithNewTxn` 闭包内逐条执行 CAS 风格更新：只有数据库中仍为 `running` 且所有者 `exec_id` 未变的记录会被回退；任一 SQL 出错会使闭包返回错误，并由事务封装回滚整批操作。

## 数据与状态

所有方法操作 `mysql.tidb_background_subtask`。关键列为 `id`、`task_key`、`exec_id`、`state`、`meta`、`error`、`start_time`、`state_update_time` 和 `end_time`。`taskID` 在参与 SQL 前通过 `TaskIDToKey` 转换，避免把业务任务标识的存储编码散落到状态方法中。

状态取自 `proto::SubtaskState*`：本文件实际写入 `running`、`succeed`、`failed`、`canceled`、`paused` 与 `pending`。状态转换具有以下约束：

- 开始、完成、通用更新和单条恢复以 `id + exec_id` 限定所有者；`StartSubtask` 额外以受影响行数把“没有匹配记录”提升为显式错误。
- 失败和取消只从 `pending/running` 转出；暂停也只从这两个活跃状态转出。
- 恢复只从 `paused` 转到 `pending`，且作用于任务下所有节点，同时清理旧错误。
- 故障恢复只从 `running` 回退，输入中即使包含其他状态的快照，SQL 条件也不会覆盖数据库中的非运行态。
- 除 `StartSubtask` 外，零行更新被视为成功；调用方不能仅凭 `Ok(())` 推断记录确实发生变化。

## 依赖与调用关系

上游直接证据包括：

- `pkg/dxf/framework/taskexecutor/task_executor.rs` 调用 `StartSubtask`、`FinishSubtask`、`FailSubtask`、`UpdateSubtaskStateAndError` 和 `RunningSubtasksBack2Pending`，并在若干写入外层提供最多三次重试。
- `pkg/dxf/framework/taskexecutor/manager.rs` 调用 `PauseSubtasks`、`CancelSubtask` 和 `FailSubtask`，把管理状态转换为存储动作。
- `pkg/dxf/framework/scheduler/storage_adapter.rs` 调用 `ResumeSubtasks`，为调度器提供存储适配。
- `pkg/dxf/importinto/scheduler_testkit_test.rs` 直接调用 `FinishSubtask` 构造导入流程的已完成状态，证明此 API 也用于跨模块测试夹具。

下游依赖分两类：`ExecuteSQLWithNewSession` 用于单条 SQL，负责从池借 Session、执行并恢复 `TxnEntrySizeLimit` 后归还；`WithNewTxn` 用于 `StartSubtask` 和批量回退，执行 `BEGIN`，闭包成功则提交、失败则用后台内部上下文回滚。两者定义于 `pkg/dxf/framework/storage/task_table.rs`。状态、子任务基类和错误序列化分别依赖 `proto`、`serializeErr`/`serializeErrOption`；SQL 执行依赖 `sqlexec::ExecSQL`。

RustCodeGraph 能解析八个方法到 `ExecuteSQLWithNewSession` 或 `WithNewTxn` 的下游调用边；当前索引没有为这些同名跨 crate 方法解析出可靠 callers，因此上游关系以精确的仓库引用搜索和调用方源码复核为准。

## 错误处理与边界

错误通过 `Result<(), Error>` 原样向上传播。`StartSubtask`、`FinishSubtask`、`CancelSubtask`、`PauseSubtasks`、`ResumeSubtasks` 和非空的 `RunningSubtasksBack2Pending` 在数据库操作前调用 `DXFRandomErrorWithOnePercent()`，用于验证调用方重试/容错路径；`FailSubtask` 和 `UpdateSubtaskStateAndError` 不调用该失败点。

`StartSubtask` 是唯一主动解释零行更新的方法：它把所有权不匹配或缺失记录统一映射为 `ErrSubtaskNotFound`。`FinishSubtask`、暂停、恢复、取消、通用更新和故障回退都允许零行更新成功，因此扩展调用方时必须判断是否需要额外 CAS 结果语义，不能假定现有接口会报告陈旧 `exec_id`。

`FailSubtask(None)` 与 `RunningSubtasksBack2Pending([])` 都在失败点和 Session 获取之前快速返回，不产生 SQL。错误写入经过序列化；恢复明确清空错误。上下文取消等 SQL 层错误由 Session/SQL 执行层返回，Go 测试 `table_test.go` 的取消上下文用例验证通用更新会返回 `context.Canceled`，而陈旧 `exec_id` 导致零行更新时仍成功且状态不变。

## 并发与资源生命周期

文件没有自身线程、锁、异步任务或通道；并发仲裁全部落在 SQL 条件和事务上。`exec_id` 是节点所有权令牌，`state in (...)` 或 `state = running` 是状态 CAS 条件，可防止迟到写覆盖不兼容状态。

`StartSubtask` 与 `RunningSubtasksBack2Pending` 使用显式事务。前者保证状态写入和 `AffectedRows` 检查使用同一 Session；后者保证整批回退全成或全退。其他方法每次借用独立 Session 执行一条 SQL，完成后由 `WithNewSession` 恢复 Session 配置并放回池。

批量回退当前逐条执行 SQL，复杂度和事务持有时间随 `subtasks.len()` 线性增长；增加大批量恢复能力时应评估事务大小、锁持有时间与失败重试成本。`PauseSubtasks`/`CancelSubtask` 是集合更新，而 `FailSubtask` 的 `limit 1` 在没有显式排序时不承诺选择哪一条匹配记录。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dxf/framework/storage/subtask_state.go`。八个 Rust 方法保留了 Go 版本的 SQL 条件、时间字段、失败点位置、事务选择与快速返回语义：尤其是 `StartSubtask` 的 `AffectedRows == 0 -> ErrSubtaskNotFound`、`FailSubtask` 的空错误短路和 `limit 1`、故障恢复的空切片短路及逐条事务更新。

主要语言映射是：Go `error` 对应 Rust `Result<(), Error>`，可空 Go 错误对应 `Option<Error>`，`[]byte` 对应 `Vec<u8>`，`[]*proto.SubtaskBase` 对应拥有所有权的 `Vec<proto::SubtaskBase>`。Go 的可变参数 SQL 参数在 Rust 中显式组成 `Vec<Value>`；Go `defer` 式 Session/事务清理由 `task_table.rs` 的封装实现。

当前可观察差异不改变核心状态语义：Rust 的 `UpdateSubtaskStateAndError` 使用 `serializeErrOption` 明确处理 `None`，而 Go 调用 `serializeErr(nil)`；Rust 的 `FailSubtask` 接收拥有所有权的 `Option<Error>`。调用方 trait `pkg/dxf/framework/taskexecutor/interface.rs` 使用引用参数，实际适配层需完成引用到本文件拥有值参数的转换，扩展时应同时检查接口和存储实现，避免只改单侧签名。

## 扩展指南

新增状态或改变转换规则时，首先修改对应方法的 SQL 状态集合，并同步检查 `pkg/dxf/framework/proto` 的状态定义、`pkg/dxf/framework/taskexecutor/interface.rs` 的 `TaskTable` 契约及调度/执行调用方。若新增“必须命中一行”的所有权敏感更新，应仿照 `StartSubtask` 在同一 Session 检查受影响行数；不要沿用其他方法“零行即成功”的语义而遗漏陈旧所有权检测。

若新增批量动作，需要明确是单 SQL 集合更新还是 `WithNewTxn` 内逐条 CAS，并记录原子性、失败回滚和大事务风险。错误字段变更应复用 `serializeErr`/`serializeErrOption`，恢复类操作要明确是否清理旧错误；时间字段变更需分别说明 `start_time`、`state_update_time`、`end_time` 的含义，避免把暂停/恢复误当作终态。

测试不要内嵌到生产源文件。优先扩展同目录独立 Rust 测试：SQL 形状与 `AffectedRows` 可放在 `converter_1_aster_unit_test.rs`，事务、批量回退和状态入口可放在 `table_test.rs`；上层重试与分支行为放在 `pkg/dxf/framework/taskexecutor/*_test.rs`。还应同步 Go 对照测试 `pkg/dxf/framework/storage/table_test.go`，确保移植语义没有分叉。

## 验证依据

- 生产源码：`pkg/dxf/framework/storage/subtask_state.rs`；确认唯一顶层项为 `impl TaskManager`，内部包含八个公开方法，无条件编译项。
- crate 与装配：`pkg/dxf/framework/storage/Cargo.toml`、`pkg/dxf/framework/storage/lib.rs`；确认 crate 名、入口、feature、依赖以及 `include!("subtask_state.rs")`。
- 事务与 Session：`pkg/dxf/framework/storage/task_table.rs` 中 `WithNewSession`、`WithNewTxn`、`ExecuteSQLWithNewSession` 和 `ErrSubtaskNotFound`。
- RustCodeGraph：`files --filter pkg/dxf/framework/storage/subtask_state.rs` 返回该文件及 10 个索引符号；`query SubtaskState`/`query subtask_state` 定位 Rust/Go 状态与文件；对八个 `subtask_state.rs::<方法>` 执行 `node`，确认源码与到 `WithNewTxn`/`ExecuteSQLWithNewSession` 的调用边。callers 查询未解析出结果，故调用方另以精确引用搜索核验。
- Rust 调用方：`pkg/dxf/framework/taskexecutor/task_executor.rs`、`pkg/dxf/framework/taskexecutor/manager.rs`、`pkg/dxf/framework/scheduler/storage_adapter.rs`、`pkg/dxf/importinto/scheduler_testkit_test.rs`。
- Rust 独立测试：`pkg/dxf/framework/storage/converter_1_aster_unit_test.rs::start_subtask_checks_affected_rows_like_go` 验证零/一受影响行；同文件 `history_validation_nodes_and_state_sql_match_go` 验证暂停 SQL；`pkg/dxf/framework/storage/table_test.rs::TestRunningSubtasksBack2Pending` 验证空输入、逐条参数和状态条件。`table_test.rs` 还调用失败、取消、暂停和恢复入口。
- Go 对照与测试：`pkg/dxf/framework/storage/subtask_state.go`；`pkg/dxf/framework/storage/table_test.go` 的 `TestRunningSubtasksBack2Pending`、`TestSubtasksState` 及暂停/恢复、上下文取消、陈旧 `exec_id` 等用例，覆盖状态、时间、错误和条件更新边界。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文档存在且恰含 11 个固定二级章节，并人工复核所有“已支持”陈述均有上述源码、调用边或测试依据。

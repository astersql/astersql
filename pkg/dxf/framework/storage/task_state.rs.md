# `pkg/dxf/framework/storage/task_state.rs`

## 文件定位

本文件是 DXF（Distributed eXecution Framework）持久化层的“全局任务状态写路径”，对应源码为 [`task_state.rs`](task_state.rs)。它不定义状态枚举，而是在 `TaskManager` 上实现取消、失败、回滚、等待决议、暂停/恢复、运行参数修改和成功终结等写操作。所有状态最终落入 `mysql.tidb_global_task`；其中错误暂停和修改完成还会同步 `mysql.tidb_background_subtask`（`task_state.rs:32-337`）。

crate 根通过 `include!("task_state.rs")` 把这些符号直接并入 `astersql-dxf-framework-storage` 根模块，并在测试构建中以独立文件模块加载 `task_state_test.rs`（`lib.rs:1247-1262`）。`Cargo.toml` 指定 `lib.rs` 为库入口、关闭自动测试发现，并声明 `nextgen` feature；本文件自身没有 `cfg` 分支，feature 不改变这里的状态转换。清单中的 `proto-crate`、`serde`/`serde_json` 等依赖支撑协议类型和 JSON，但当前实现通过 crate 根提供的 `proto`、`json`、session/SQL 兼容层工作。

## 核心职责

1. 用带当前状态谓词的条件 `UPDATE` 推进任务状态，避免旧 owner 或并发操作者覆盖已经变化的状态（例如 `FailTask`、`transitTaskStateOnErr`、`PausedTask`、`ResumedTask`、`SucceedTask`）。
2. 提供用户控制入口：按 ID 或 key 取消，按 key 暂停与恢复；`PauseTask`/`ResumeTask` 通过受影响行数返回“是否命中可转换任务”（`task_state.rs:32-72,195-255`）。
3. 为调度器记录失败、回滚原因和等待人工决议的错误，并在终态写入 `end_time`（`task_state.rs:74-193`）。
4. 在可恢复错误场景中，以同一事务把全局任务置为 `pausing`，再把当前 step 的失败子任务恢复成可继续的 `paused` 状态（`PauseTaskOnError`）。
5. 以事务化的两阶段协议修改任务参数：`ModifyTaskByID` 把允许的前态保存到 `modify_params` 并进入 `modifying`，`ModifiedTask` 应用槽位、节点数和 meta 后回到原状态，同时更新非终态子任务的并发度（`task_state.rs:258-322`）。
6. 用固定取消标记识别被包装或附加上下文的“用户取消”错误，供 scheduler 和历史错误分类复用（`TaskCancelMessage`、`IsCancelledErr`）。

## 主要符号

- `TaskCancelMessage: &str`：公开取消标记 `"cancelled by user"`。DXF 没有单独的 cancelled 终态；用户取消最终表现为带此错误的 `reverted`，所以错误文本承担分类标记（`task_state.rs:24-25`；Go 对照 `task_state.go:30-37`）。
- `IsCancelledErr(Option<&dyn Display>) -> bool`：对非空错误的展示文本做子串匹配，允许标记被包装；`None`、普通错误和 `context canceled` 不视为用户取消（`task_state.rs:27-30`，`task_state_test.rs:207-218`）。
- `TaskManager::CancelTask`：把 ID 对应的 `pending`、`running` 或 `awaiting-resolution` 任务置为 `cancelling`；会先执行百分之一随机错误注入钩子（`task_state.rs:32-49`）。
- `TaskManager::CancelTaskByKeySession`：使用调用方给定的 session 按 `task_key` 做同样转换，不创建 session，也不执行本文件的百分之一错误注入（`task_state.rs:51-72`）。
- `TaskManager::FailTask`：仅当状态等于调用方提供的 `currentState` 时转为 `failed`，序列化错误，并写 `state_update_time`、`end_time`（`task_state.rs:74-91`）。
- `TaskManager::RevertTask`、`AwaitingResolveTask`：分别委托私有 helper，把指定前态转为 `reverting` 或 `awaiting-resolution` 并记录错误（`task_state.rs:93-110,164-181`）。
- `TaskManager::transitTaskStateOnErr`：私有公共路径，执行 `currState -> targetState` 条件更新；它不检查 `AffectedRows`（`task_state.rs:112-128`）。
- `TaskManager::PauseTaskOnError`：事务内完成全局任务与失败子任务的联动；第一条更新未命中时返回 `ErrTaskChanged`，不执行第二条更新（`task_state.rs:130-162`）。
- `TaskManager::RevertedTask`：仅把 `reverting` 转成终态 `reverted` 并写 `end_time`（`task_state.rs:183-193`）。
- `TaskManager::PauseTask`、`PausedTask`：外部请求先令 `pending|running -> pausing`，调度收敛后再令 `pausing -> paused`（`task_state.rs:195-225`）。
- `TaskManager::ResumeTask`、`ResumedTask`：外部请求令 `paused -> resuming` 并清空任务错误，调度恢复完成后令 `resuming -> running`（`task_state.rs:227-256`）。
- `TaskManager::ModifyTaskByID`：验证 `PrevState` 只能是 `pending`、`running` 或 `paused`，序列化 `ModifyParam`，在事务中读取当前任务并以 compare-and-set 进入 `modifying`（`task_state.rs:258-288`；`proto/task.rs:83-101`）。
- `TaskManager::ModifiedTask`：仅在任务仍为 `modifying` 时应用 `RequiredSlots`、`MaxNodeCount`、`Meta`，清空 `modify_params` 并恢复 `PrevState`；随后更新 `pending|running|paused` 子任务的 concurrency（`task_state.rs:290-322`）。
- `TaskManager::SucceedTask`：仅把 `running` 转为 `succeed`，同步把 step 设为 `StepDone` 并写结束时间（`task_state.rs:324-337`）。

## 执行流程

普通单表转换的流程是：调用者给出任务标识与期望前态；方法执行可选的 `DXFRandomErrorWithOnePercent`；再通过 `ExecuteSQLWithNewSession` 或 `WithNewSession` 取得 session，提交带 `WHERE state ...` 的参数化更新；SQL/session 错误沿 `Result` 返回。取消、失败、回滚、等待决议、回滚完成、暂停落定、恢复落定和成功终结都属于这一类。条件未命中时，多数方法按 Go 语义静默成功；只有明确需要竞争反馈的方法读取 `AffectedRows`。

暂停链分为两个入口层级。Handle 层的 `PauseTask` 调用 runtime 的 `pause_task`，最终按 key 执行 `pending|running -> pausing`；scheduler 观察并停止执行器后，经 `storage_adapter.rs::paused_task` 调用 `PausedTask` 完成 `pausing -> paused`。恢复链对称：`ResumeTask` 先清错误并写 `resuming`，scheduler 完成恢复后由 `ResumedTask` 写回 `running`（`handle/handle.rs:472-481`；`scheduler/storage_adapter.rs:291-315`）。

错误暂停具有更强原子性：`PauseTaskOnError` 开事务，先以 `taskID + taskState` 条件写任务为 `pausing` 和序列化错误；若 `AffectedRows == 0`，返回 `ErrTaskChanged`，`WithNewTxn` 回滚。命中后，它只将同一任务、同一步骤、当前为 `failed` 的子任务改为 `paused`，清空 `end_time`，最后统一提交，确保任务状态与可恢复子任务不会只更新一半（`task_state.rs:130-162`；`task_table.rs:208-228`）。

参数修改也分两阶段。`ModifyTaskByID` 先验证前态并序列化参数，再在事务中读取任务；读取值和 `PrevState` 不同立即返回 `ErrTaskChanged`。随后条件更新为 `modifying`，并再次用 `AffectedRows` 捕获读取与写入之间的竞争。执行器应用修改后，`ModifiedTask` 在新事务中条件恢复原状态；若另一个 owner 已完成更新，零受影响行被视为幂等跳过。只有任务行由本次调用成功更新时，才更新三种活跃/可恢复子任务的并发度（`task_state.rs:258-322`）。

## 数据与状态

任务状态由 `proto::TaskState` 字符串常量表示。与本文件相关的主链包括：`pending|running|awaiting-resolution -> cancelling`，`任意调用方声明的 current -> failed|reverting|awaiting-resolution`，`reverting -> reverted`，`pending|running -> pausing -> paused -> resuming -> running`，`pending|running|paused -> modifying -> PrevState`，以及 `running -> succeed`。允许进入 `modifying` 的集合由 `TaskStateExt::CanMoveToModifying` 集中定义，不能仅在本文件扩展（`proto/task.rs:38-61,83-101`）。

持久化字段除 `state` 外还包括 `error`、`step`、`modify_params`、`concurrency`、`max_node_count`、`meta`、`state_update_time` 和终态的 `end_time`。`serializeErrOption(None)` 返回空字节以模拟 Go 的 nil error；`ResumeTask` 则显式把 error 置 SQL `NULL`。子任务通过 `TaskIDToKey(taskID)` 的十进制字符串关联任务；`PauseTaskOnError` 清除失败子任务 `end_time`，`ModifiedTask` 不触碰 finished/canceled/failed 等终态子任务（`task_table.rs:842-845`；`converter.rs:286-290`）。

`TaskManager` 自身只持有可克隆的 `SessionPool`，本文件没有额外缓存或进程级可变状态。`found` 是 `PauseTask`/`ResumeTask` 单次调用的局部反馈；状态真值始终在系统表，不能把返回 `Ok(())` 等同于一定更新了行。

## 依赖与调用关系

上游主链是 scheduler 的存储接口。`scheduler::TaskManager` trait 声明 fail/revert/awaiting-resolution/reverted/paused/pause-on-error/resumed/modified/succeed 等操作；`scheduler/storage_adapter.rs:256-323` 将 scheduler 的 `Task`、`TaskState` 和 `SchedulerError` 转成 storage 类型后调用本文件的方法。Handle 的公开取消/暂停/恢复 API 位于 `handle/handle.rs:460-481`，其中取消会先按 key 查任务再按 ID 下发；直接导入执行路径也在成功/失败后调用 `SucceedTask` 或 `FailTask`，再搬迁历史记录（`pkg/session/runtime/import_file.rs:423-459`）。

取消标记的读取链独立于状态写链：scheduler crate 将 `TaskCancelMessage` 重新导出并用 storage 的 `IsCancelledErr` 判断取消；历史摘要在 `history.rs::ClassifyTaskErrorMessage` 中把 `reverted + 取消标记` 分类为 `cancelled`，其他 reverted 错误分类为数据错误或失败（`scheduler/scheduler.rs:34,543-544`；`history.rs:318-338`）。

下游依赖包括：`TaskManager::{WithNewSession,WithNewTxn,ExecuteSQLWithNewSession}` 管理 session/事务；`sqlexec::ExecSQL` 执行参数化 SQL；`json::Marshal` 编码修改参数；`serializeErrOption` 编码可空错误；`TaskIDToKey` 生成子任务外键；`proto` 提供状态、step、任务及修改参数；`failpoint`/`injectfailpoint` 提供竞争与随机错误注入点。crate 清单的 storage、config、proto、schstatus、serde 依赖构成更宽的 crate 边界，但本文件没有网络、文件或异步运行时依赖。

RustCodeGraph 对目标文件的文件级关系只直接列出 `history.rs`，而精确 `callers/callees` 子命令未输出方法边；因此上述 Rust 上游方法调用同时用索引中的 `node/explore` 和仓库符号搜索核对，不能把文件级 “used by 1 file” 误解为完整运行时调用面。

## 错误处理与边界

- 所有 SQL、session、事务提交和 JSON 编码错误都通过 `Result<_, Error>` 原样或经 `errors::Trace` 返回。`WithNewTxn` 在闭包返回错误时用后台 internal-dist-task context 回滚；提交错误也向上传播（`task_table.rs:208-228`）。
- `CancelTask`、`FailTask`、`RevertTask`、`PauseTaskOnError`、`AwaitingResolveTask`、`RevertedTask`、`PauseTask`、`PausedTask`、`ResumedTask`、`ModifiedTask`、`SucceedTask` 包含百分之一错误注入；`ResumeTask`、`ModifyTaskByID` 和使用外部 session 的 `CancelTaskByKeySession` 没有这一层钩子。session 获取本身另有千分之一注入（`task_table.rs:189-205`）。
- 条件更新的零行语义并不统一：`PauseTask`/`ResumeTask` 返回 `false`；`PauseTaskOnError` 和 `ModifyTaskByID` 返回 `ErrTaskChanged`；`ModifiedTask` 幂等跳过；其余方法通常返回成功。扩展时必须保留调用方依赖的差异，不能统一成一种行为。
- `ModifyTaskByID` 在接触数据库前拒绝不允许的 `PrevState`，返回 `ErrTaskStateNotAllow`；任务不存在或读取失败由 `getTaskBaseByID` 的错误语义决定（`task_table.rs:74-81`）。
- `IsCancelledErr` 是文本子串判断，不是结构化错误码；它有意接受包装文本，也可能把包含固定短语的其他错误分类为取消。更改文案会影响 scheduler 与历史分类兼容性。
- 参数化 SQL 防止值直接拼接，但状态合法性主要由调用方和 `WHERE` 谓词保证；本文件没有数据库行级锁的显式 API，也不验证任意 `currentState` 是否属于业务允许的前态。

## 并发与资源生命周期

本文件全部为同步方法，没有线程、异步任务或通道。并发安全依赖数据库条件更新、事务和 `AffectedRows`：每次状态写都携带期望状态，避免旧 owner 无条件覆盖新 owner 的结果；对需要调用者重试/放弃的竞争，显式返回 `ErrTaskChanged`。Go 测试在 `beforeMoveToModifying` 与 `beforeModifiedTask` failpoint 上制造竞态，分别验证进入 modifying 时发现变化，以及两个完成者中后到者静默跳过（`task_state_test.go:253-381`）。

`WithNewSession` 从池中取 session，临时调整 `TxnEntrySizeLimit`，闭包结束后恢复限制并归还；`ExecuteSQLWithNewSession` 包装单条写操作。`WithNewTxn` 在同一 session 上执行 `BEGIN`，成功则 commit，失败则 rollback，因而 `PauseTaskOnError` 和修改两阶段中每一个阶段内部具备原子性，但 `ModifyTaskByID` 与稍后的 `ModifiedTask` 之间不是一个长事务，靠 `modifying` 状态和条件更新协调（`task_table.rs:189-245`）。

`CancelTaskByKeySession` 的 session 生命周期完全归调用方所有，适合嵌入调用方事务；其他方法取得的 session 在方法返回前归还池。`TaskManager` 可 `Clone`，共享 session pool 的并发细节由 `SessionPool` 管理。没有持有跨调用锁，也没有自动重试；随机注入或数据库瞬时错误是否重试由上层决定。

## 与 Go 版本的对应关系

直接对照文件是 [`task_state.go`](task_state.go)。Rust 按相同顺序保留了常量/取消判断、14 个公开 `TaskManager` 方法和 `transitTaskStateOnErr` 私有 helper，SQL 的目标表、状态谓词、更新时间/结束时间、错误列、修改参数及 subtask 联动均与 Go 对齐（Go `task_state.go:30-339`；Rust `task_state.rs:24-338`）。

表示层差异主要来自语言：Go 的 `error` 可为 nil，Rust 用 `impl Into<Option<Error>>` 兼容 `Error` 与 `None`；对应 Rust 测试明确验证四条错误写路径对 `None` 写空字节（`task_state_test.rs:54-80`）。Go 的 `*proto.ModifyParam`、`*proto.Task` 在 Rust 中按值传递；Go `json.RawMessage(bytes)` 的 SQL 参数在 Rust 由 `Vec<u8>`/crate 根值转换层承接。Rust 的 `Context` 当前是 crate 兼容类型，方法仍保留 Go 风格命名以便逐函数比对。

Go 测试连接真实 mock store/system tables，验证读回状态、错误归类、事务回滚、子任务恢复以及真实并发竞争（`task_state_test.go:40-403`）。Rust 的同路径测试使用内存 SQL executor，重点验证生成的 SQL、参数顺序、受影响行分支和 nil-error 契约（`task_state_test.rs:23-218`）。因此 Rust 测试证明写路径形状和局部控制流，但不能单独替代 Go 测试对真实 SQL 执行、数据库锁与提交行为的覆盖。

## 扩展指南

- 新增任务状态转换时，先在 `pkg/dxf/framework/proto/task.rs` 定义/核对状态及允许转换，再在本文件使用带期望前态的条件更新；若 scheduler 要调用，还需同步 `scheduler/interface.rs` trait 和 `scheduler/storage_adapter.rs`。不要只添加无谓词的状态写。
- 若转换同时影响 task 与 subtask，使用 `WithNewTxn` 保证原子性，并为首条条件更新规定清晰的零行语义。新增子任务关联条件时继续使用 `TaskIDToKey`，同时核对当前表与历史表 schema。
- 修改错误或取消语义时同步 `serializeErrOption`、scheduler 的 re-export/判断、`history.rs::ClassifyTaskErrorMessage`，并保留包装错误可识别性。若迁移到结构化错误码，需要兼容已经持久化的旧文本。
- 扩展可修改参数时同步 `proto::ModifyParam`/serde、`CanMoveToModifying`、`ModifyTaskByID` 的写入和 `ModifiedTask` 的应用字段；明确哪些 subtask 状态应更新，避免误改终态记录。修改 concurrency 还要关注源码注释指出的未来“不同 subtask 并发度”需求（`task_state.rs:307-318`）。
- 回归测试应放在独立文件 `pkg/dxf/framework/storage/task_state_test.rs`；需要更真实 SQL/并发验证时同步 Go `task_state_test.go`，或按 Cargo 清单注册新的独立 Rust 测试目标。不要把 Rust 测试内嵌到生产源文件。
- 性能风险主要是额外 session/事务往返和大范围 subtask 更新。新增写操作应保持精确的 task key、step、state 谓词，并评估索引命中与受影响行规模；兼容风险主要是状态字符串、错误序列化和零行返回语义。

## 验证依据

- Rust 源与装配：`pkg/dxf/framework/storage/task_state.rs:24-338`、`pkg/dxf/framework/storage/lib.rs:1247-1262`。
- session、事务与公共 helper：`pkg/dxf/framework/storage/task_table.rs:74-81,134-154,189-245,842-845`；`pkg/dxf/framework/storage/converter.rs:286-290`。
- 状态定义与修改前态约束：`pkg/dxf/framework/proto/task.rs:30-61,83-101`。
- crate 边界：`pkg/dxf/framework/storage/Cargo.toml` 的 `[lib]`、`[[test]]`、`[features]`、`[dependencies]` 与 `package.metadata.porting`。
- Rust 上游：`pkg/dxf/framework/scheduler/interface.rs:349-430`、`pkg/dxf/framework/scheduler/storage_adapter.rs:256-323`、`pkg/dxf/framework/handle/handle.rs:460-481`、`pkg/session/runtime/import_file.rs:423-459`、`pkg/dxf/framework/storage/history.rs:318-338`。
- Go 对照与测试：`pkg/dxf/framework/storage/task_state.go:30-339`；`pkg/dxf/framework/storage/task_state_test.go:40-403`。
- Rust 独立测试：`pkg/dxf/framework/storage/task_state_test.rs:23-218` 覆盖主状态写、nil error、错误暂停竞争、修改前态、暂停恢复序列和取消标记；仓库符号搜索还确认 scheduler adapter、Handle 测试与若干导入路径存在直接调用。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter`、`node --file`、`query` 和 `explore` 用于核对目标源码、状态定义、Go 对照、测试与调用链。精确 `callers/callees --file` 没有返回文本，故调用边又用仓库 `rg` 搜索及调用者源码交叉验证。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前执行任务指定的 11 标题结构验证，并人工复核文档只描述已由上述源码、调用点和测试证实的当前行为。

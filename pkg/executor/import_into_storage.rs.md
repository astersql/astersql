# `pkg/executor/import_into_storage.rs`

## 文件定位

本文件属于 `astersql-executor` crate；模块由 `pkg/executor/lib.rs` 以 `pub mod import_into_storage` 无条件导出，crate 依赖在 `pkg/executor/Cargo.toml` 中声明。它位于 `IMPORT INTO` 执行器与 DXF（Distributed eXecution Framework）存储层之间，专门处理“取消导入作业”时 DXF 任务和用户 keyspace 中导入作业记录之间的衔接。

当前 Rust 代码没有发现生产调用者：RustCodeGraph 和仓库引用搜索得到的直接调用均位于独立测试 `pkg/executor/import_into_test.rs`。因此它已经实现并公开了存储侧取消语义，但在当前 Rust 主执行链中的生产接线未由本文件或现有引用证明。Go 版本的生产入口位于 `pkg/executor/import_into.go`，由 `cancelAndWaitImportJob` 承接 `CANCEL IMPORT JOB` 和终止后台导入的路径。

## 核心职责

- `cancelAndWaitImportJobInStorage` 是公开门面：接收调用方准备好的 DXF 上下文、DXF 任务管理器和导入作业管理器，将实际分支交给内部函数。
- `cancelImportJobWithFallbackHook` 先以 `TaskKey(job_id)` 查询活跃表和历史表。如果任务存在，则在新事务内按 key 请求取消，并等待该 key 对应任务结束；如果且仅如果查询返回 `ErrTaskNotFound`，才转为直接取消尚处于 pending 的导入作业。
- `cancelDanglingImportJob` 在导入作业所属 keyspace 中开启新 session，调用 importer 的条件更新，并用 affected rows 验证 pending 状态确实被本次调用改为 cancelled。

这一设计解决 next-gen 提交中的两事务窗口：用户 keyspace 的 job 行可能已提交，而 SYSTEM keyspace 的 DXF task 尚未提交或提交失败。依据 `pkg/dxf/importinto/job_doc.go` 的 C1–C3 时序，缺少 task 时不能无限等待一个当时不可见的任务，也不能把已经开始运行的 job 当作悬空 job 取消。

## 主要符号

- `pub fn cancelAndWaitImportJobInStorage(context, job_id, task_manager, job_manager) -> Result<(), storage::Error>`：稳定的公开入口。`task_manager` 用于 DXF task 探测、取消和等待；`job_manager` 用于 fallback 中访问导入 job，两者可能分别属于 SYSTEM 和用户 keyspace。
- `pub(crate) fn cancelImportJobWithFallbackHook(..., before_fallback: impl FnOnce())`：包内入口和测试缝。生产门面传入空闭包；测试在 task probe miss 与 fallback 之间注入竞态，以验证一次探测决定后续路径。
- `pub fn cancelDanglingImportJob(manager, job_id) -> Result<(), storage::Error>`：只取消 pending job 的公开底层操作。它不会取消 running job，也不等待 DXF task。
- 文件没有模块级常量、结构体、trait、`impl` 或条件编译项；`#![allow(non_snake_case)]` 保留 Go 风格符号名，便于逐项对照移植。

## 执行流程

1. `cancelAndWaitImportJobInStorage` 调用 `cancelImportJobWithFallbackHook`，默认 hook 不执行任何动作。
2. 内部函数通过 `astersql_dxf_importinto::TaskKey(job_id)` 生成 DXF task key，并调用 `task_manager.GetTaskBaseByKeyWithHistory((), key)`；这里同时考虑活跃任务与历史任务。
3. 若查询成功，函数调用 `task_manager.WithNewTxn`，在同一事务 session 中执行 `CancelTaskByKeySession`。事务成功后调用 `WaitTaskDoneByKeyWithManager(context, &key, task_manager)`，直到任务进入该等待器认可的终态或返回错误。
4. 若查询错误恰为 `storage::ErrTaskNotFound`，先执行一次 `before_fallback`，随后调用 `cancelDanglingImportJob(job_manager, job_id)`。探测结论不会因为 hook 期间出现了一个迟到 task 而重新改走等待路径。
5. 若查询是其他错误，原样返回，不接触 job manager，避免把存储故障误判成 task 不存在。
6. fallback 通过 `manager.WithNewSession` 取得 session，将其 SQL executor 包装为 `ImportJobStorageSession`，然后调用 `astersql_executor_importer::CancelPendingJob`。该函数的 SQL 只匹配指定 ID 且状态为 pending 的行，并写入 cancelled 状态和固定错误文案 `cancelled by user`。
7. SQL 成功后读取 `session.GetSessionVars().StmtCtx.AffectedRows()`；零行表示 job 已不存在、已取消或已离开 pending，函数返回 `job state changed during cancel, please try again later`，非零才成功。

## 数据与状态

关键标识是 `job_id: i64` 及其派生的 `TaskKey(job_id)`。DXF task 与 import job 是两个不同存储对象，也可能位于不同 keyspace，因此接口明确接收两个 `TaskManager`，而不是在函数内假定单一管理器。

状态不变量如下：

- task 在活跃表或历史表中存在，就进入 task 取消/等待分支；task 是否仍满足取消更新的状态谓词，不决定它是否“悬空”。`archived_reverted_task_does_not_cancel_pending_user_job` 验证历史中的 reverted task 仍阻止 fallback。
- 只有 task 查询明确返回 `ErrTaskNotFound` 才可直接修改 job；普通查询错误必须传播。
- fallback 的 job 更新仅允许 `pending -> cancelled`。`CancelPendingJob` 不写 `start_time` 或 `end_time`，并把 `error_message` 设为 `cancelled by user`；对应 SQL 和参数顺序由 `pkg/executor/importer/job.rs` 及其独立测试验证。
- affected rows 是竞态仲裁结果：零行不是幂等成功，而是向上报告状态已经变化，让调用方重试或重新观察状态。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 公开本模块；当前 Rust 引用仅来自 `pkg/executor/import_into_test.rs`。RustCodeGraph 记录 `cancelAndWaitImportJobInStorage` 被三个测试直接调用，另有两个测试通过 `cancelImportJobWithFallbackHook` 注入竞态；仓库搜索还找到同一测试文件内对公开函数的更多调用。没有证据表明 `pkg/executor/import_into.rs` 或其他 Rust 生产模块已经调用这些函数。

下游调用边为：

- `cancelAndWaitImportJobInStorage -> cancelImportJobWithFallbackHook`；
- `cancelImportJobWithFallbackHook -> TaskManager::GetTaskBaseByKeyWithHistory / TaskManager::WithNewTxn / TaskManager::CancelTaskByKeySession / WaitTaskDoneByKeyWithManager / cancelDanglingImportJob`；
- `cancelDanglingImportJob -> TaskManager::WithNewSession / session.GetSQLExecutor / importer::CancelPendingJob / session.GetSessionVars().StmtCtx.AffectedRows`。

`pkg/executor/Cargo.toml` 为这些边声明了 `astersql-dxf-framework-handle`、`astersql-dxf-framework-storage`、`astersql-dxf-importinto` 和 `astersql-executor-importer` 路径依赖；`nextgen` feature 只向 `astersql-dxf-importinto/nextgen` 透传。本文件自身不受 feature gate 控制，但涉及真实存储与竞态的 Rust 测试均以 `#[cfg(feature = "nextgen")]` 保护。

## 错误处理与边界

- task 探测成功后，事务创建、取消请求或等待中的任一错误都通过 `?` 返回；不会回退到直接取消 job。
- task 探测失败时只对与 `storage::ErrTaskNotFound` 相等的错误启用 fallback。`failed_task_probe_does_not_cancel_import_job` 用失败 SQL backend 验证其他错误原样返回，且 job manager 不会被访问。
- `CancelPendingJob` 的字符串错误经 `storage::Error::new` 转换；随后 affected rows 为零会产生本文件定义的确定性错误。
- 同一个 pending job 被再次取消、job 已被删除，或调度器已把它推进到 running，都会表现为 affected rows 为零；本函数不进一步区分这些原因。
- 本文件不做权限、job 可取消性或 job 是否可见的前置检查。Go 生产链在进入 `cancelAndWaitImportJob` 前由执行器处理这些策略；未来 Rust 接线也不应把缺失的策略检查误认为本存储辅助函数的职责。
- task 已存在但处于不可取消终态时，代码仍走等待器。依据 `job_doc.go`，这可能看起来像取消成功，但不代表本次调用实际改变了 task 状态。

## 并发与资源生命周期

task 存在路径把 `CancelTaskByKeySession` 包在 `WithNewTxn` 中，事务 session 只在闭包期间有效；提交成功后才开始等待。等待使用调用方提供的 `Context`，其取消语义由 `WaitTaskDoneByKeyWithManager` 处理，本文件不创建线程或后台任务。

fallback 的 `WithNewSession` 管理 session 借用与归还，`ImportJobStorageSession` 仅在闭包内借用 SQL executor。条件 SQL 与 affected rows 检查共同形成原子状态守卫：如果调度器先执行 `pending -> running`，fallback 更新零行并报错；如果 fallback 先执行 `pending -> cancelled`，调度器后续应观察取消状态并阻止导入准入。

`before_fallback: FnOnce` 精确位于“task probe 已判定不存在”与“job 条件更新”之间，只执行一次。`task_committed_after_lookup_miss_does_not_delay_pending_job_cancel` 证明此窗口中新提交的 task 不会令当前取消调用转而等待；`task_started_after_probe_miss_cannot_cancel_a_running_job` 证明调度器抢先启动后，pending 谓词保护 running job。

## 与 Go 版本的对应关系

Rust 主分支逐项对应 `pkg/executor/import_into.go`：Go 的 `cancelAndWaitImportJob` 获取 SYSTEM DXF manager、构造 `TaskKey`、查活跃/历史 task、在事务内 `CancelTaskByKeySession`、等待任务；明确未找到 task 时再调用 `cancelDanglingImportJob`。Rust 把 manager 和 context 作为参数注入，便于 keyspace 分离和独立测试，但不负责 Go 中的 `WithInternalSourceType`、manager 获取、日志与 failpoint。

Rust 的 `before_fallback` 是 Go 两个 failpoint 所覆盖竞态窗口的可测试替代缝，而非业务 hook。Rust `cancelDanglingImportJob` 与 Go 同名函数一样：用 job-side manager 新建 session、调用 `CancelPendingJob`、要求 affected rows 非零，并使用相同错误文案。

Go 测试 `pkg/executor/import_into_test.go` 验证无 DXF task 时 pending job 被取消、running job 保持运行且报状态变化；`tests/realtikvtest/importintotest3/cross_ks_test.go` 验证跨 keyspace 与真实竞态。Rust 的 `pkg/executor/import_into_test.rs` 重现了核心语义，但当前 Rust 测试入口是 crate 内独立文件，并且 nextgen 案例受 feature gate 控制。

## 扩展指南

- 接入 Rust 生产执行链时，优先调用 `cancelAndWaitImportJobInStorage`，并分别传入 SYSTEM task manager 与用户 keyspace job manager；不要用单一 manager 隐含替代跨 keyspace 边界。接线前应补充与 Go `ImportIntoActionExec` 和后台终止路径对应的独立测试。
- 修改 task 是否存在的判定时，必须继续查询活跃与历史表。将“取消更新零行”等同于“不存在”会误取消仍有终态/历史 task 的 pending job。
- 修改 fallback 时必须保持只匹配 pending 的 SQL 状态守卫与 affected rows 检查；放宽到 running 会破坏 scheduler 所拥有的任务生命周期。
- 若增加日志、内部 source type、重试或可观测性，应明确放在调用层还是存储辅助层，并与 Go 行为对齐；不要让重试重新探测后改变“probe miss 后不等待迟到 task”的既有语义。
- 回归测试应继续放在独立文件。直接分支和竞态测试扩展 `pkg/executor/import_into_test.rs`；底层条件 SQL扩展 `pkg/executor/importer/job_test.rs`；跨 keyspace/真实 TiKV 行为与 Go 对照则参考现有 Go 单元测试和 `tests/realtikvtest/importintotest3/cross_ks_test.go`，不应把测试嵌入生产源文件。
- 性能风险主要来自 task 等待与存储往返；正确性风险集中在跨 keyspace 管理器混用、错误分类、历史任务可见性和 affected rows 语义；兼容性风险是错误文案、取消状态及 `cancelled by user` 对外表现变化。

## 验证依据

- RustCodeGraph：`status` 确认索引覆盖 Rust/Go；`query import_into_storage` 定位文件与三个函数；`node --file pkg/executor/import_into_storage.rs` 读取完整实现；`callers`/`callees` 核对公开门面、内部 hook、fallback 和测试调用边。图查询对部分跨语言常见符号产生噪声，因此具体 manager 方法边同时以目标源码复核。
- Rust 源与装配：`pkg/executor/import_into_storage.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`。
- Rust 独立测试：`pkg/executor/import_into_test.rs` 的 `dangling_import_job_cancellation_preserves_state_guards_on_real_sql`、`user_keyspace_job_cancelled_before_task_commit_stops_admission`、`task_started_after_probe_miss_cannot_cancel_a_running_job`、`failed_task_probe_does_not_cancel_import_job`、`archived_reverted_task_does_not_cancel_pending_user_job`、`task_committed_after_lookup_miss_does_not_delay_pending_job_cancel`；底层 SQL 由 `pkg/executor/importer/job_test.rs::pending_cancellation_uses_only_pending_predicate_and_propagates_sql_errors` 验证。
- Go 对照与设计证据：`pkg/executor/import_into.go`、`pkg/executor/import_into_test.go`、`pkg/executor/importer/job.go`、`pkg/executor/importer/job_test.go`、`pkg/dxf/importinto/job_doc.go`、`pkg/dxf/importinto/job_testkit_test.go`、`tests/realtikvtest/importintotest3/cross_ks_test.go`。
- 本任务只新增说明文档，未运行 Cargo 或代码测试；最终以任务规定的 11 标题结构命令检查文件存在性和章节数，并人工复核当前接线状态、竞态分支、错误传播及扩展约束没有超出上述证据。

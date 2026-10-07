# `pkg/dxf/importinto/job.rs`

## 文件定位

`job.rs` 是 `astersql-dxf-importinto` crate 的 IMPORT INTO 作业边界层。模块由 `pkg/dxf/importinto/lib.rs` 的 `pub mod job; pub use job::*;` 对外暴露，处在 SQL 导入执行器与 DXF（Distributed eXecution Framework）存储之间：上游决定提交普通分布式任务还是仅在指定实例运行的任务，本文件创建用户可见的 `mysql.tidb_import_jobs` 记录、持久化 DXF 全局任务，并把 DXF 的任务/子任务状态聚合为 SHOW IMPORT JOB 使用的运行时信息。

本文件同时承担两组相互关联但生命周期不同的职责：`SubmitTask`/`SubmitStandaloneTask` 到 `StorageTaskSubmissionService::CreateJobAndTask` 是提交路径；`StorageRuntimeInfoProvider` 到 `GetRuntimeInfoForJob`/`GetJobLastUpdateTime` 是只读展示路径。crate 的 `nextgen` feature 由 `pkg/dxf/importinto/Cargo.toml` 转发给 `astersql-config-kerneltype/nextgen`；实际分流还依赖运行时 kernel、deploy mode 和当前 keyspace，而不是只看编译 feature。

## 核心职责

1. `SubmitTask` 与 `SubmitStandaloneTask` 将 `importer::Plan`、原始 SQL、可选执行实例和预切分 chunk 组装为 `planner::LogicalPlan`，再通过 `TaskSubmissionService` 提交。
2. `ShouldUseAsyncPrepare` 在 NextGen、非 Starter、全局排序三个条件同时成立时选择异步 prepare；`doSubmitTask` 将 `PrepareMode` 标成 required，并把调用者计划的线程数和最大节点数临时改为 1。
3. `StorageTaskSubmissionService::CreateJobAndTask` 负责真实持久化顺序：创建 import job，Classic 模式下切换表模式，再按 keyspace 决定同事务创建 DXF task，或在用户 keyspace job 提交后到 SYSTEM/DXF 服务 keyspace 开第二个事务。
4. `StorageRuntimeInfoProvider` 从活跃或历史任务表取得任务，从当前 step 的子任务 JSON summary 重建处理量、行数、滑动窗口速度和更新时间，并查询活跃/历史子任务的最后状态更新时间。
5. `GetRuntimeInfoForJob` 按任务步骤选择正确的总量单位和导入行数语义；`RuntimeInfo` 再提供百分比、ETA、大小和速度的展示字符串。
6. `TaskKey` 集中把 job ID 映射为稳定的 IMPORT INTO DXF key，供提交、查询、取消和调度路径共同使用。

## 主要符号

- `SubmittedTask { JobID, TaskID, TaskKey }`：成功提交的跨表标识，避免调用者再次查找 job 与 DXF task 的对应关系。
- `TaskSubmissionService`：提交抽象，入口是 `CreateJobAndTask(&LogicalPlan, task_key, thread_count, max_node_count)`；`SharedTaskSubmissionService` 是其 `Arc<dyn ...>` 句柄。当前存储实现会自行用真实 job ID 构造 key，因此参数 `task_key` 在该实现中未使用，但保留在可替换服务契约中。
- `ClassicTableModeChanger` 与 `StorageSessionTableModeChanger`：把 Classic 内核下“在同一 session/事务内将表切到 import mode”的动作隔离为可测试依赖。
- `StorageTaskSubmissionService`：生产提交实现。`NewWithStorageBackend`/`New` 获取本地 task manager；`WithManagers`、`WithAfterUserJobCreated` 提供精确注入点供事务路由和竞态测试使用；私有 `create_import_job` 与 `submit_dxf_task` 分别写 job 表和 DXF task 表。
- `SubmitStandaloneTask`：通过 `infosync::GetServerInfo` 固定 eligible instance，并携带 `HashMap<i32, Vec<importer::Chunk>>`；`SubmitTask` 则不给预分配实例和 chunk。
- `ShouldUseAsyncPrepare`、`doSubmitTask`：提交前策略与公共组装核心；后者还拒绝缺失表元数据、零 table ID，以及无法转换为 `i32` 的线程数。
- `SubtaskRuntimeSummary`、`TaskRuntimeSnapshot`、`RuntimeInfoProvider`：将存储读取与聚合算法解耦的中间模型；`SharedRuntimeInfoProvider` 是共享 trait object。
- `StorageRuntimeInfoProvider`：生产读取实现；`WireProgress`/`WireSubtaskSummary` 是仅用于反序列化持久化 summary JSON 的内部 wire 类型。
- `RuntimeInfo`：SHOW 层可直接消费的任务状态、当前 step、错误、行数、速度、处理量、总量与更新时间。`Percent`、`ETA`、`TotalSize`、`ProcessedSize`、`SpeedStr` 负责 Go 兼容展示。
- `GetRuntimeInfoForJob`、`GetJobLastUpdateTime`：按 job ID 查询和聚合的公共入口；`convertToMySQLTime` 先截断小数秒，再转目标时区的 MySQL DATETIME。
- `FormatSecondAsTime`、`format_bytes`、`speed_window`：分别保持 Go `time.Duration`、`docker/go-units.BytesSize` 和 DXF 子任务速度窗口语义。

## 执行流程

提交普通任务时，`SubmitTask` 进入 `doSubmitTask`。后者先验证 `Plan.TableInfo` 存在且 `TableInfo.ID != 0`，随后计算 async prepare。若启用，它在复制到 `LogicalPlan` 之后把原始 `plan.ThreadCnt` 与 `plan.MaxNodeCnt` 改为 1，并以 `PrepareModeRequired`、并发 1、节点 1 调用服务；否则保留计划并把线程数安全转换为 `i32`。Standalone 路径只多一步：先读取本机 server info，写入唯一 eligible instance，并传入 chunk map。

生产服务 `StorageTaskSubmissionService::CreateJobAndTask` 先复制提交所需的逻辑计划字段，然后在 `local_manager.WithNewTxn` 内调用 `create_import_job`。该函数序列化 import 参数，插入 `mysql.tidb_import_jobs` 的 pending/none 初始记录，再用 `SELECT LAST_INSERT_ID()` 取得 job ID，并要求查询恰好返回一行。Classic 内核随后通过 `ClassicTableModeChanger` 切换表模式。若当前不是用户 keyspace，`submit_dxf_task` 直接在这个事务中将真实 job ID 写入 meta，通过 `LogicalPlan::ToTaskMeta`、`GetTargetScope` 和 `CreateTaskWithSession` 创建任务。

若运行于 NextGen 用户 keyspace，第一个事务只提交 job；可选 `after_user_job_created` hook 在两次提交之间触发，然后代码取得 `GetDXFSvcTaskMgr` 并在第二个事务中创建 SYSTEM keyspace DXF task。无论哪条路，任务持久化成功后都会 `NotifyTaskChange`，再按 task ID 回读 `TaskBase`，返回 `SubmittedTask`。

运行时查询从 `GetRuntimeInfoForJob` 生成 `TaskKey(job_id)` 并请求 provider。存储 provider 先用 `GetTaskByKeyWithHistory` 兼容活跃/历史任务，再在读取 summary 前反序列化 `TaskMeta`。无任务错误时，它只查询当前 step 的活跃子任务 summary，将每条 progress 的 RFC3339 时间转为 `SystemTime`，调用 DXF `SubtaskSummary::GetSpeedInTimeRange` 和 `UpdateTime` 得到子任务摘要。聚合函数对子任务的 processed、row count、speed 做 wrapping 加法并取最新更新时间；随后按 step 从 `TaskMeta.Summary` 选择总字节数或冲突行数，并只在 import/write-and-ingest 步骤保留实时行数，在 post-process 使用最终 `ImportedRows`。

## 数据与状态

提交侧的持久状态分布在两张表：`mysql.tidb_import_jobs` 保存用户可见 job，`mysql.tidb_global_task` 保存 DXF task；二者通过 `TaskKey(job_id)` 及写入 task meta 的 `JobID` 关联。job 初始状态为 `importer::jobStatusPending`、step 为 `importer::jobStepNone`。DXF task 还携带 keyspace、线程数、target scope、最大节点数，以及 `ManualRecovery`、`PauseOnKVDiskFull`、`MaxRuntimeSlots`、`TargetSteps`、`PrepareMode` 等 extra params。

展示侧有三层数据形态：数据库 JSON 对应 `WireSubtaskSummary`/`WireProgress`；provider 归一化为 `SubtaskRuntimeSummary`/`TaskRuntimeSnapshot`；公共聚合结果为 `RuntimeInfo`。当前 step 决定 `Total` 的来源和单位：import/write-and-ingest 取 ingest bytes，encode-and-sort 取 encode bytes，merge-sort 取 merge bytes，collect-conflicts 与 conflict-resolution 分别取对应 summary 的 row count。冲突步骤的 `Processed`、`Total`、`Speed` 都按 conflicts 展示，其余步骤按字节展示。

重要不变量包括：百分比上限为 100，但为兼容 Go 不限制负 processed，因此可以显示负百分比；StepInit 与 post-process 的进度显示 `N/A`；速度或总量非正时 ETA 为 `N/A`；来自 summary 的 UNIX_EPOCH 表示“无有效更新时间”，不会写入 `RuntimeInfo.UpdateTime`；无子任务更新时间时 `GetJobLastUpdateTime` 返回 MySQL `ZeroTime`。

## 依赖与调用关系

上游主链在 `pkg/executor/import_into.rs` 的 `ImportIntoExec::submitTask`：本地路径先切 chunk 后选择 standalone；远程路径按 distributed task 开关选择普通提交或 standalone。仓库直接引用还包括 `pkg/session/runtime_test/ddl.rs` 和 `pkg/executor/import_into_test.rs` 的真实存储接线，以及取消路径 `pkg/executor/import_into_storage.rs` 对 `TaskKey` 的复用。`pkg/dxf/importinto/lib.rs` 将本文件 API 从 crate 根重新导出。

下游提交依赖包括：`planner::LogicalPlan::{ToTaskMeta, GetTaskExtraParams}`、`storage::TaskManager::{WithNewTxn, CreateTaskWithSession, GetTaskBaseByID}`、`storage::sqlexec::ExecSQL`、`dxfhandle::{GetTargetScope, NotifyTaskChange}`、`infosync::GetServerInfo`、kernel/deploy mode 配置和 `taskkey::ForJob`。运行时路径依赖 `TaskManager` 的活跃/历史任务查询与 SQL session、`proto::TaskMeta::Unmarshal`、`execute::SubtaskSummary` 的速度计算，以及 `types::time` 的 DATETIME 构造。

RustCodeGraph 的文件查询确认 `job.rs` 含 60 个符号并被 12 个文件使用；精确 `callers/callees` 命令没有返回静态边，因此调用关系由仓库直接引用搜索补齐。Cargo 声明表明这些依赖均是 workspace 内的独立 crate；本文件没有自行创建异步任务，也没有网络 I/O，分布式唤醒由 DXF handle 完成。

## 错误处理与边界

所有公共提交和查询入口以 `errors::SharedError` 向上传播错误；storage error、JSON/RFC3339 解析错误和时间转换错误均转换为共享错误而不吞掉。提交前显式检查缺失 table metadata、零 table ID、线程数超出 `i32`；`create_import_job` 还检查 LAST_INSERT_ID 查询行数。Classic 路径中 job、表模式切换和 task 处于同一事务，任一步失败会由 `WithNewTxn` 回滚；`pkg/dxf/importinto/job_test.rs::classic_ddl_failure_rolls_back_job_without_creating_task` 验证 DDL 失败不会留下任务。

NextGen 用户 keyspace 是有意的非原子边界：job 事务已经提交后，获取 DXF manager 或第二个 task 事务仍可能失败，从而留下 pending job 而没有 task。`pkg/dxf/importinto/job_doc.go` 对该一致性窗口、取消竞态和 dangling job fallback 有详细说明；本文件当前不会在第二阶段失败时清理 job，也不会在 task 提交前重新检查 job 是否已被取消，扩展时不能误写成跨 keyspace 原子提交。

运行时 provider 保持 Go 的错误顺序：先解码 task meta，再检查 task error 或读取 summary，因此非法 meta 会优先于 summary 查询错误。只要 `ErrorMessage` 是 `Some`，即使字符串为空，`GetRuntimeInfoForJob` 也立即返回且不聚合进度。summary JSON 或其时间字段非法会使整个查询失败；最后更新时间 SQL 要求至少返回一行，NULL 则解释为无更新时间。

## 并发与资源生命周期

两个 service/provider trait 都要求 `Send + Sync`，共享别名使用 `Arc`，允许执行器或会话在并发上下文中复用。`StorageTaskSubmissionService` 持有可克隆的 `TaskManager`；每次提交通过 `WithNewTxn` 获取有界 session/事务，闭包成功后提交、错误时回滚。Classic/SYSTEM 路径只有一个事务；NextGen 用户 keyspace 顺序使用 local manager 与 DXF manager 的两个事务，中间 hook 专门暴露一致性窗口给测试。

运行时读取没有长期持锁：task snapshot 和 summary 在调用内构造并拥有数据；每个进度点的速度按 `execute::SubtaskSpeedUpdateInterval`（当前由测试验证为 15 秒）计算。`SystemTime::now()` 在一次 `GetTaskRuntime` 中只取一次，保证同批子任务使用相同窗口终点。`GetJobLastUpdateTime` 在短事务内 UNION 活跃表和历史表并取最大时间，事务结束后才转换为公共返回值。

本文件不启动线程、future、channel 或后台 worker；任务实际调度和执行生命周期由 DXF scheduler/task executor 管理。`NotifyTaskChange` 发生在任务事务提交之后，是唤醒调度观察者的边界，而不是持久化成功的替代品。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/importinto/job.go`。Rust 保留了 Go 的两类提交入口、async prepare 判定、Classic 同事务与 NextGen 用户/SYSTEM keyspace 双事务路由、任务变更通知、按 step 聚合进度、冲突计数展示、时间格式和 job task key 生成语义。

实现结构上的主要差异是依赖注入：Go 直接取得 task manager、domain DDL executor 和全局 failpoint；Rust 用 `TaskSubmissionService`、`ClassicTableModeChanger`、`RuntimeInfoProvider` 隔离可替换边界，并由调用者提供已判定的 `running_on_user_keyspace`。Go 通过 planner `Run` 创建 task；Rust 的 `submit_dxf_task` 直接把 `LogicalPlan` 转成 meta 后调用 `CreateTaskWithSession`。Rust 还显式验证缺失/非法 table ID 和 `usize` 到 `i32` 的转换。

兼容细节不可随意“修正”：`FormatSecondAsTime` 模拟 Go 有符号 `time.Duration` 的乘法溢出与负数分量；`format_bytes` 模拟 `docker/go-units` 的四位有效数字和负数不缩放行为；聚合使用 wrapping addition 来对应 Go `int64`；async prepare 先复制 logical plan 再修改调用者 plan，因此提交 meta 中保留复制前的计划字段，而实际并发参数为 1。Go 的 `mockDisableAsyncPrepare` 和 `mockSpeedDuration` failpoint 没有成为 Rust 生产 API，Rust 测试改用配置锁、注入 service/provider 与确定性 summary。

## 扩展指南

- 新增提交字段时，优先在 `planner::LogicalPlan`/`TaskMeta` 定义语义，再同步 `doSubmitTask` 的复制、`CreateJobAndTask` 中的 `submitted_plan` 复制和 `submit_dxf_task` 的 extra params；同时更新 `pkg/dxf/importinto/job_test.rs` 的捕获 service 或事务顺序测试，避免字段在两次复制中丢失。
- 改动 kernel/keyspace 路由时，必须同时核对 `pkg/dxf/importinto/job_doc.go` 的事务竞态、Classic 回滚测试、用户 keyspace 双事务测试，以及 `job_testkit_test.rs` 的 `nextgen_submission_tests`；不要把两个 keyspace 强行包装成并不存在的单事务。
- 新增导入 step 或 summary 指标时，需要同时更新 `GetRuntimeInfoForJob` 的 Total/ImportRows 选择、`RuntimeInfo::isConflictStep` 的单位、所有展示方法，以及独立测试 `show_import_progress_fields_match_go_step_matrix`。若 wire JSON 字段变化，还要同步 `WireProgress`/`WireSubtaskSummary`。
- 修改 task key 格式应从独立 crate `pkg/dxf/importinto/taskkey/task_key.rs` 入手，并检查提交、查询、取消、调度和 realtikv 测试的兼容性；本文件 `TaskKey` 应继续只是统一门面。
- 修改时间、速度或字节格式前应以 Go 行为为规范，扩展 `job_test.rs` 和 `job_testkit_test.rs` 的边界用例，尤其覆盖负值、零值、跨天、时区、小数秒、超大 task ID、活跃/历史表隔离。
- 测试逻辑保持在独立的 `pkg/dxf/importinto/job_test.rs` 与 `pkg/dxf/importinto/job_testkit_test.rs`，不要内嵌到生产文件；本任务是文档分析，不建议借机调整生产实现。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；`files --filter pkg/dxf/importinto/job.rs` 确认目标文件、60 个符号和 12 个使用文件；两次 `node --file ...` 覆盖源码 1–844 行；`query` 定位 Rust/Go 的 `SubmitTask`、`GetRuntimeInfoForJob`、`CreateJobAndTask`、`TaskKey`。精确 `callers/callees` 无返回，故没有把缺失的图边当成已验证事实。
- 生产源码：`pkg/dxf/importinto/job.rs`；模块与 crate 边界：`pkg/dxf/importinto/lib.rs`、`pkg/dxf/importinto/Cargo.toml`；上游调用与取消 key 复用：`pkg/executor/import_into.rs`、`pkg/executor/import_into_storage.rs`、`pkg/session/runtime/import_file.rs`。
- Go 对照：`pkg/dxf/importinto/job.go`；跨 keyspace 事务与取消竞态说明：`pkg/dxf/importinto/job_doc.go`。
- Rust 独立测试：`pkg/dxf/importinto/job_test.rs` 覆盖格式、事务顺序/回滚、双 keyspace、meta 错误优先级、async prepare、时区和错误短路；`pkg/dxf/importinto/job_testkit_test.rs` 覆盖 step 矩阵、NextGen 路由、时间边界和大 task ID 隔离。Go 对照测试入口为 `pkg/dxf/importinto/job_testkit_test.go`。
- 本说明仅做静态代码、调用引用和测试意图核验；按任务约束未运行 Cargo，也未声称执行测试。交付结构通过任务指定命令检查，固定二级章节应恰好为 11 个。

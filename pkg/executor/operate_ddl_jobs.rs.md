# `pkg/executor/operate_ddl_jobs.rs`

## 文件定位

该文件属于 `astersql-executor` crate，由 [`pkg/executor/lib.rs`](lib.rs) 以公开模块 `operate_ddl_jobs` 导出，直接依赖只有 `astersql_util_chunk::Chunk`（[`pkg/executor/Cargo.toml`](Cargo.toml)）。它把 DDL 作业管理分为两类 Open/Next 风格的执行逻辑：

- `CommandDDLJobsExec` 编排 `ADMIN CANCEL/PAUSE/RESUME DDL JOBS`，`Open` 执行命令，`Next` 分批输出每个 job ID 的结果。
- `AlterDDLJobExec` 编排 `ADMIN ALTER DDL JOBS` 的系统会话和事务重试，改写运行中作业的 reorg 参数。

当前 Rust 接线必须如实理解：[`pkg/executor/builder.rs`](builder.rs) 已将 `CancelDdlJobs` / `PauseDdlJobs` / `ResumeDdlJobs` / `AlterDdlJob` 计划分派到 `build_leaf(ExecutorKind::...)`，但仓库内没有生产代码为本文件的两个 backend trait 提供实现，也没有生产侧构造这些泛型执行器。因此该文件是已公开、可单测的迁移逻辑，不能仅凭此认定 Rust 主链已使用它完成真实 DDL 管理。

## 核心职责

1. 用 `DDLJobCommandBackend` 隔离系统会话、DDL owner 命令与错误格式化，使 `CommandDDLJobsExec::Open` 只负责“获取会话—执行—缓存逐作业结果—归还会话”。
2. 用 `CommandDDLJobsExec::Next` 将 `job_ids` 与 `errors` 按位置映射为两列 Chunk：job ID 字符串和 `successful` / `error: ...`。
3. 用 `AlterDDLJobBackend` 抽象作业读改写、可改性判断、NextGen 限制、事务与故障注入；`processAlterDDLJobConfig` 在最多三次尝试内更新作业。
4. 用 `updateReorgMeta` 把 `Thread` / `BatchSize` / `MaxWriteSpeed` 选项写入 job，并对每个实际应用的选项标记终端用户管理操作者。

该层管理的是已持久化 DDL job 的控制与 reorg 参数，不执行 DDL schema state machine 或 backfill 本身；真实 Go 实现通过 `mysql.tidb_ddl_job` 作业元数据与 DDL 后台系统衔接。

## 主要符号

- `DDLJobCommandBackend`：取得/归还系统会话、执行一组 job ID、提供 Chunk 最大容量和错误文本的能力边界。`execute` 同时返回逐作业 `Vec<Option<Error>>` 和整体 `Result`。
- `CommandDDLJobsExec<B>`：持有 `backend`、分页 `cursor`、`job_ids` 及与之对齐的 `errors`。公开方法为 `Open` 和 `Next`。
- `CancelDDLJobsExec<B>`、`PauseDDLJobsExec<B>`、`ResumeDDLJobsExec<B>`：包装同一 `CommandDDLJobsExec`的 tuple struct，本文件未为它们增加差异行为；命令差异应由构造时选择的 backend 实现表达。
- `AlterDDLJobOptionName`：闭集枚举 `Thread`、`BatchSize`、`MaxWriteSpeed`，避免在本层出现未知选项分支。
- `AlterDDLJobOption`：选项名与 `Option<i64>` 值；`None` 在更新时被忽略。
- `alterDDLJobMaxRetryCnt`：事务尝试上限，值为 `3`。
- `AlterDDLJobBackend`：系统会话、begin/get/update/commit/rollback、作业可改性、NextGen 限制、参数 setter、操作者标记及提交故障注入的完整能力边界。
- `AlterDDLJobExec<B>`：持有 backend、单个 `job_id` 和选项列表；`Open` 管理会话，`processAlterDDLJobConfig` 管理事务尝试，`updateReorgMeta` 执行内存中 job 改写。

## 执行流程

`CommandDDLJobsExec` 的流程如下：

1. `Open` 调用 `backend.system_session()`；取会话失败时立即返回，尚无需归还的句柄。
2. 调用 `backend.execute(ctx, session, &job_ids)`，先把逐作业错误保存到 `self.errors`。
3. 无论 `execute` 的整体 `Result` 是成功还是失败，正常返回路径都会调用 `release_system_session`，然后传递整体结果。
4. 调用者随后可反复调用 `Next`。每次先以 `max_chunk_size` 重置 Chunk；已到尾部则返回空批。
5. 当批行数为 `min(request.Capacity(), job_ids.len() - cursor)`。每行第 0 列是十进制 job ID，第 1 列是错误文本或 `successful`；最后推进 `cursor`。

`AlterDDLJobExec` 的流程如下：

1. `Open` 获取系统会话，调用 `processAlterDDLJobConfig`，在正常 `Result` 返回路径归还会话，再向上传递结果。
2. `processAlterDDLJobConfig` 最多循环三次。每次依次 `begin` 和 `get_job`；这两步失败时记录 `last_error` 并进入下次尝试。
3. 读到 job 后，`is_alterable == false` 直接返回 `unsupported_operation`；`is_next_generation_add_index == true` 直接返回 NextGen 专用错误。这两类语义错误不重试。
4. `updateReorgMeta` 忽略值为 `None` 的选项，对其余选项调用对应 setter，并标记管理操作者。
5. `update_job` 失败则记录错误并重试。若 `inject_commit_failure` 返回错误，立即 rollback 并返回，不重试。
6. `commit` 失败时 rollback、记录错误并重试；成功则结束。三次都失败后返回最后一个错误。

## 数据与状态

- `CommandDDLJobsExec.cursor` 是跨 `Next` 调用保留的唯一分页状态；它只增不减。`Open` 不会重置它，因而执行器若被非标准地重复 Open，调用方必须自行确保游标语义。
- `job_ids[index]` 和 `errors[index]` 在协议上应按位对齐。Rust 代码用 `errors.get(index)` 访问：缺失的错误项会被当作成功，而不会越界 panic。这是 backend 需维持的重要对齐不变式，也是新测试应覆盖的边界。
- `AlterDDLJobExec` 不缓存读到的 job；每次事务尝试都重新读取，并在内存副本上更新后写回。
- `AlterDDLJobOption.value` 已是 `i64`。线程数、批大小、速度字面量的类型/范围/单位校验不在本文件实现；应由上游计划构建或具体 backend 保证。
- `last_error` 仅保留最后一次可重试失败。三次循环的每个继续分支都先设置它，因此末尾 `expect` 表达内部控制流不变式。

## 依赖与调用关系

- 模块入口：[`pkg/executor/lib.rs`](lib.rs) 的 `pub mod operate_ddl_jobs`；测试模块通过 `#[cfg(test)] mod operate_ddl_jobs_test` 独立装配。
- Rust 上游计划分派：[`pkg/executor/builder.rs`](builder.rs) 的 `ExecutorBuilder::build` 识别四种 `Plan` 变体，`buildCancelDDLJobs`、`buildPauseDDLJobs`、`buildResumeDDLJobs`、`buildAlterDDLJob` 将它们降为对应 `ExecutorKind` 的 leaf executor。搜索未发现这些 builder 方法构造本文件的泛型类型。
- Rust 已证实调用边：`AlterDDLJobExec::Open -> processAlterDDLJobConfig -> updateReorgMeta`，并通过 `AlterDDLJobBackend` 调用事务、作业存储和参数 setter；`CommandDDLJobsExec::Open/Next` 通过 `DDLJobCommandBackend` 下沉具体命令与会话操作。
- 直接数据依赖：`Chunk::GrowAndReset`、`Capacity`、`AppendString`，由 Cargo 中路径依赖 `astersql-util-chunk = ../util/chunk` 提供。本模块没有 feature gate，`nextgen` 判断被抽象为 backend 方法。
- Go 主链：[`pkg/executor/builder.go`](builder.go) 直接构造 Go 执行器，并为通用命令分别注入 `ddl.CancelJobs`、`ddl.PauseJobs`、`ddl.ResumeJobs`；Go `AlterDDLJobExec` 通过 DDL session 读写 `mysql.tidb_ddl_job.job_meta`。

## 错误处理与边界

- 系统会话获取失败直接向上传播。会话获取成功后，两个 `Open` 都在被调用逻辑返回 `Result` 后显式归还会话；本文件没有 RAII guard，所以 backend 若 panic，该显式归还不会执行。
- 通用命令区分“整体执行错误”与“逐 job 错误”：前者由 `Open` 返回，后者缓存后由 `Next` 以行结果呈现。`Next` 自身没有当前可产生的错误分支。
- `errors` 比 `job_ids` 短时会静默显示 `successful`；比它长的尾项不会输出。安全的 backend 应返回等长向量或在新接线时增加显式协议检查。
- ALTER 中 `begin`、`get_job`、`update_job`、`commit` 错误可重试；作业不可改、NextGen add-index 不支持与注入的 commit 失败会立即返回。
- 只有 commit 错误和注入失败分支显式调用 `rollback`。`begin` 之后的 `get_job` / `update_job` 失败、不支持分支不在本层显式 rollback；具体 backend 必须与重新 begin/会话归还语义协调，不应由本文档推断隐式行为。
- `updateReorgMeta` 对 Rust 枚举所有变体完全匹配且不返回 `Result`；因此 Go 版的未知选项错误、`max_write_speed` 表达式解析错误不可能在这个 Rust 函数中产生。

## 并发与资源生命周期

本文件不创建线程、异步任务或通道；方法以 `&mut self` 和 `&mut Context/Session` 串行编排。真实并发来自 DDL owner/作业调度器与对同一作业元数据的并发更新；本层通过“每次尝试重新 begin + 重读 job”及最多三次重试应对冲突，但没有退避或睡眠。

系统会话由 backend 创建，两类 `Open` 都在处理结束后归还；ALTER 事务也由 backend 管理。在返回可重试错误的某些分支上本层不显式 rollback，因此 backend 实现的 begin 重入、失败事务清理和 session pool 归还协议是重要的资源不变式。`Chunk` 由调用者持有，`Next` 只重置并填充它，不保留其引用。

## 与 Go 版本的对应关系

Rust 以 [`pkg/executor/operate_ddl_jobs.go`](operate_ddl_jobs.go) 为直接语义对照，保留了主要结构：通用 cancel/pause/resume 执行器、Open/Next 分工、逐 job 结果文本、ALTER 的三次事务尝试、可改性检查、NextGen add-index 限制、三种 reorg 参数和 commit-failure rollback。

已核对的差异包括：

- Go `CommandDDLJobsExec` 嵌入 `BaseExecutor` 并持有具体函数字段，builder 会注入 `ddl.CancelJobs/PauseJobs/ResumeJobs`；Rust 用 trait 隔离，当前未找到生产 backend 实现。
- Go ALTER 直接构造 DDL session，SQL 读取/更新 `mysql.tidb_ddl_job.job_meta`；Rust 只定义 `get_job` / `update_job` 能力，没有 SQL、编解码或存储类型。
- Go 选项是 planner 表达式，`max_write_speed` 需解析，且 default 分支可返回不支持错误；Rust 选项已收敛为枚举 + `i64`，`updateReorgMeta` 无错误返回。
- Go `Next` 在 `errs != nil` 时直接索引 `errs[i]`；Rust 用 `get` 避免越界，但会把缺项当成成功。
- Go `Open` 用 `defer` 归还 ALTER 系统会话；Rust 是处理函数返回后显式释放。Go 通用命令也是显式释放。
- Go failpoint 分支 rollback 并立即返回；Rust `inject_commit_failure` 保留了同样语义，且独立 Rust 测试证实只 begin 一次、rollback 一次、归还会话一次。

Go 回归证据位于 [`pkg/ddl/db_test.go`](../ddl/db_test.go)：`TestAdminAlterDDLJobUpdateSysTable` 覆盖 thread/batch size 更新，`TestAdminAlterDDLJobUnsupportedCases` 覆盖参数范围、不存在 job、不支持作业及 NextGen，`TestAdminAlterDDLJobCommitFailed` 验证提交故障后元数据不变。这些 Go 测试证明参考实现的行为，不等于 Rust 已全部覆盖或已接入。

## 扩展指南

- 接入 Rust 生产主链时，应先为两个 backend trait 提供真实实现，并明确 `builder.rs` 的 `ExecutorKind` leaf 路径是否要构造本文件类型；不要把模块公开误当为已接线。
- 新增 cancel/pause/resume 类命令时，优先复用 `CommandDDLJobsExec`，在 backend/构造点表达命令差异；同时增加独立测试，覆盖多批 `Next`、混合成败结果、空 ID 列表和 errors 对齐协议。
- 新增 ALTER 选项时，需同步 `AlterDDLJobOptionName`、backend setter 和 `updateReorgMeta`，并在上游 planner/转换层保留 Go 的类型、范围、单位和未知选项错误语义。
- 改变重试逻辑时，必须明确哪些错误可重试、每个已 begin 分支如何清理事务、是否需要退避，并为 begin/get/update/commit 分别失败增加计数与最终错误测试。
- 资源管理若需覆盖 panic/unwind，应将系统会话改为 guard/RAII 协议，而不是仅在更多返回点手写 release。
- Rust 测试应继续放在独立的 [`pkg/executor/operate_ddl_jobs_test.rs`](operate_ddl_jobs_test.rs)，不嵌入生产源文件。如果完成真实后端移植，还应与 `pkg/ddl/db_test.go` 的 Go 用例对齐，并根据用户可见 SQL 行为增加 Rust 集成覆盖。

## 验证依据

- RustCodeGraph 索引状态：工程已索引（11,467 文件，其中 7,032 个 Rust 文件）；`node --file pkg/executor/operate_ddl_jobs.rs` 返回完整 282 行源码和符号上下文。
- RustCodeGraph 符号查询：`query CommandDDLJobsExec`、`query AlterDDLJobExec`、`query DDLJobCommandBackend`、`query AlterDDLJobBackend`、`query processAlterDDLJobConfig`、`query updateReorgMeta`。索引识别了 Rust trait/结构与 Go 同名实现；泛型 Rust impl 方法的 callers/callees 未完整建边，因而用直接引用搜索补齐。
- 已读生产与装配文件：[`pkg/executor/operate_ddl_jobs.rs`](operate_ddl_jobs.rs)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/executor/builder.rs`](builder.rs)、[`pkg/executor/Cargo.toml`](Cargo.toml)。`pkg/executor` 当前没有 `doc.go`，无额外包级契约可读。
- 已读对照与测试：[`pkg/executor/operate_ddl_jobs.go`](operate_ddl_jobs.go)、[`pkg/executor/builder.go`](builder.go)、[`pkg/executor/operate_ddl_jobs_test.rs`](operate_ddl_jobs_test.rs)、[`pkg/ddl/db_test.go`](../ddl/db_test.go) 的 ALTER DDL JOB 回归用例，以及 [`docs/agents/ddl/README.md`](../../docs/agents/ddl/README.md) 的 DDL 执行边界。
- Rust 测试 `commit_failpoint_rolls_back_and_returns_without_retrying` 的静态证据：注入失败后返回指定错误，`begin_calls == 1`、故障检查一次、rollback 一次、session release 一次。本任务按计划为纯文档分析，未运行 Cargo。
- 结构验证使用任务指定的 `test -f` + `rg -c` 命令，要求本文档恰有 11 个固定二级标题。

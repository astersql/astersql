# `pkg/executor/adapter.rs`

## 文件定位

`adapter.rs` 是 `astersql-executor` crate 的语句执行适配层。crate 根 `pkg/executor/lib.rs` 以公开模块 `pub mod adapter` 暴露它；`pkg/executor/Cargo.toml` 则声明该 crate 的根为 `lib.rs`，并把 planner、parser、session context、resource group、executor metrics、chunk、execdetails、sqlkiller 等本地 crate 接入同一执行边界。

它位于“物理计划已经产生、具体算子即将运行”与“会话/协议层消费结果”之间：上游会话桥接在 `pkg/session/runtime/typed_adapter_bridge.rs` 构造 `ExecStmt` 并调用 `ExecStmt::Exec`，下游通过 `AdapterRuntime` 构建具体 `ExecExecutor`。有结果的语句返回 `RecordSet` 给 server/session 逐批拉取；无结果 DML、悲观锁语句和 PointGet 则走各自的专用分支。

这个文件不是具体扫描、连接或 DML 算子的实现。它负责统一调度、事务/锁协议、结果集生命周期、外键级联、RU 结算和结束时的可观测性；实际算子树由 `AdapterRuntime::BuildExecutor*` 提供，具体会话实现位于 `pkg/session/runtime/typed_adapter_bridge.rs` 等文件。

## 核心职责

- 用 `ExecStmt::Exec` 把 `StatementNode`、`PlanInfo`、typed planner tree 和会话运行时接到执行器的 `Open`/`Next`/`Close` 生命周期。
- 把执行结果分成三类：`recordSet` 流式拉取、`chunkRowRecordSet` 内存物化结果、`detachedRecordSet` 脱离会话后的独立拉取。
- 为 PointGet、普通查询、无返回结果计划、悲观 DML、悲观 `SELECT FOR UPDATE` 选择不同执行路径，并维护 build/open/next/lock 阶段耗时。
- 在 DML 周围执行快照限制、悲观锁收集与重试、外键检查/级联、CTE 存储清理和 prepared execution 收尾。
- 在语句终止时统一生成慢日志、statement summary、TopSQL、计划 digest、网络流量、计划缓存、锁和 RU 指标，并更新上一条语句信息。
- 将 panic 转为 `SharedError`，避免执行器 panic 穿过适配层；同时保留普通错误供会话层决定事务语义。

## 主要符号

- `PlanKind`、`PlanInfo`、`StatementKind`、`StatementNode`：适配层使用的计划和语句摘要。`PlanInfo::IsDML`、`IsFastPlan`、`isNoResultPlan` 决定路径分类；`RebuiltPlan` 额外携带 typed plan、输出名和 schema version。
- `StatementContext`：执行期间的可变账本，保存行数、SQL/计划 digest、计划缓存状态、RU owner/evidence/final snapshot、commit details 和网络流量。其 `Default` 明确把所有计数和可选状态初始化为空。
- `ExecutionContext`：传给懒执行器的 trace、继承 RU details 与共享 `SQLKiller`。`inheritStmtRUV2Context` 只复制父语句已有的 RU 上下文。
- `ExecExecutor`：算子树的最小适配接口，定义 `Open`、`Next`/`NextWithContext`、`Close`、schema/chunk 配置、外键钩子、锁键提取、扫描行数和 `Detach`。
- `AdapterRuntime`：会话侧依赖倒置接口，集中提供执行器构建、时间戳、事务/锁、prepared statement、进程信息、RU/TopSQL/慢日志/summary 和清理钩子。它不要求 `Send + Sync`，因为注释明确会话表达式和事务可绑定当前线程。
- `ExecStmt`：文件的主状态机。它持有 `Plan`/`TypedPlan`、`StmtNode`、`Ctx`、输出列、重试计数、阶段耗时、遥测和 `StatementCtx`。
- `RecordSet`：协议层可消费的结果集接口。`Finish` 与 `CloseWithError` 允许会话先结束执行器再发布最终语句结果；`TryDetach` 和 `OnFetchReturned` 是可选能力。
- `recordSet`：普通流式结果集，持有 `ExecStmt` 克隆和仍打开的 executor；`Next` 按 chunk 拉取，`Finish` 关闭 executor/CTE，`Close` 再触发语句级收尾。
- `chunkRowRecordSet`：悲观锁查询先完整读取并加锁后形成的内存结果；客户端读取时不再访问原 executor。
- `detachedRecordSet`：只保留字段、独立 executor、SQL 文本和来源上下文，不持有会话/`ExecStmt`；适合 server 的 detach 路径。
- `StatementRUFailureGuard`：RAII 失败守卫。若会话清理提前返回或展开，`Drop` 尝试只记录一次失败终态，并在 full-report 模式发布失败原因。
- `MAX_FOREIGN_KEY_CASCADE_DEPTH`（15）限制递归外键级联；`MAX_ALIAS_IDENTIFIER_LEN`（256）限制返回列别名；`REDACT_LOG_ENABLED` 是使用 Acquire 读取的进程级原子脱敏开关。
- `FormatSQL`/`formatSQL`、`GetPlanDigest`、`getEncodedPlan`、`SelectRUDetailsForStatementLog`：日志、摘要和 RU 展示的纯辅助边界。

## 执行流程

1. 上游创建 `ExecStmt` 后调用 `ExecStmt::Exec`。外层用 `catch_unwind` 把 panic 变成 `panicError`，无论结果如何都记录执行锁指标并调用 `AdapterRuntime::OnExecComplete`；失败还会记录 RU 失败终态并取消最大执行时间。
2. `exec_inner` 先拒绝运行时不支持的 prepared execution 或 locking select，再为 `EXECUTE` 继承预处理语句内容。`PlanKind::PointGet` 直接转到 `PointGet`；其他计划先生成 digest、执行 runaway 前置检查，并安装 SQL killer/TopSQL 上下文。
3. `buildExecutor` 构造普通或 select-lock executor，随后设置 process info 和超时。`statementMaximumExecutionTime` 对 `SELECT` 使用查询超时，对 Insert/Update/Delete 使用 DML 超时，其他计划返回 0。未明确优先级时根据 `LowerPriority` 选择 Low 或 Normal。
4. `openExecutor` 成功后，locking select 在悲观事务中进入 `handlePessimisticSelectForUpdate`；低精度 TSO 会被立即拒绝。其余路径先准备外键级联环境，再由 `handleNoDelay` 判断是否应立即执行。
5. 有非空 schema 且未设置 `CalculateNoDelay` 的普通查询被包装为 `recordSet`。客户端每次调用 `Next` 时先检查 kill signal，再携带 trace/RU/SQLKiller 上下文调用 `NextWithContext`；零行表示根 executor 到达 EOF，否则累计 found rows。
6. 空 schema 或 calculate-no-delay 计划立即执行。悲观事务内的空 schema 进入 `handlePessimisticDML`；其他计划进入 `handleNoDelayExecutor`。后者拒绝 snapshot/low-resolution-TSO 下的写入，拉取一次、标记适用计划 EOF、执行外键触发器，然后关闭 executor 并记录审计。
7. `handlePessimisticDML` 在 statement start/end 钩子之间运行 `runPessimisticDML`。每轮先清理 unchanged-key 集合，执行 DML，收集待加锁键并划分 X/S lock；已写入的共享键由 `moveWrittenSharedLockKeysToExclusive` 提升为排他锁。锁冲突经 `handlePessimisticLockError` 决定返回还是重建 executor、回滚 statement 并重试。
8. 悲观 `SELECT FOR UPDATE` 在 `runPessimisticSelectForUpdate` 中拉完所有 chunk，每批先取 `TakeLockKeys` 并加排他锁，再将结果缓存到 `chunkRowRecordSet`。只有全部读取、加锁并关闭 executor 成功后，缓存结果才交给客户端。
9. `handleStmtForeignKeyTrigger` 必要时先 statement commit，再递归执行 `CheckForeignKeys` 与 cascade batch。每个 batch 构建/打开子 executor、执行一次、关闭、commit 并继续下一层；深度超过 15 返回错误。`ForeignKeyTriggerGuard` 保证退出时清除“正在处理 FK trigger”状态。
10. 流式 `recordSet::Finish` 幂等关闭 executor、结算 prepared execution 并清理 CTE；`Close`/`CloseWithError` 将 fetch/close 错误按原顺序合并后调用 `CloseRecordSet`。后者进入 `FinishExecuteStmt`，依次处理审计、Plan Replayer、RU 证据与结算、网络/慢日志/summary/TopSQL、缓存与锁指标、阶段耗时、tracker 和最终清理。

## 数据与状态

`ExecStmt` 的状态分成三层。计划层由 `Plan`、`TypedPlan`、`OutputNames` 和 `InfoSchema` 构成；`RebuildPlan` 会原子式替换这些关联值并同步 `StatementCtx.plan`。执行层由 `retryCount`、`retryStartTime` 和四组 `[当前轮, 历史锁重试轮]` 阶段耗时构成；`resetPhaseDurations` 把当前槽累加进历史槽。观测层集中在 `StatementCtx`，避免每个结果集各自维护 digest、RU 和流量。

`recordSet` 通过 `Option<Box<dyn ExecExecutor>>` 表达所有权：`Some` 表示仍可拉取，`Finish`/`Close` 取走或清空后再次关闭是幂等的；关闭后的 `Next` 返回 interrupted/closed 错误。`lastErrs` 保存 fetch 和 cleanup 错误，`joinRecordSetErrors` 使用 `errors::Join` 保持成员次序。字段元数据由 `colNames2ResultFields` 懒生成：缺少 db name 但有 table name 时补当前库，原列名为空时回退到别名，并按 Unicode 字符而非字节截断别名。

RU V2 采用“一次终态”模型。`StatementRUOwner::take_terminal_setup` 防止 panic、错误和重入重复结算；只有成功终态、无 terminal error、上下文合格、无 cursor/restricted SQL 冲突且根 EOF 已观察到时，`finishStatementRU` 才从 typed physical-plan forest 和运行时证据计算结果。缺失 typed plan 时不会用 `PlanInfo` 猜测算子成本。`SnapshotStatementRUEvidence` 在清理前冻结 plan IDs、TiKV response bytes、write keys/bytes 和 frontend compile bytes。

`REDACT_LOG_ENABLED` 是全局 `AtomicBool`；会话级 `AdapterRuntime::RedactLog` 也能触发 `GetTextToLog` 返回 `secure_text`。普通路径会去掉 `/*+ ... */` hint、按 `QueryLogMaxLen` 的字节上限在 UTF-8 边界截断，并将换行/回车/制表符折叠为空格。

## 依赖与调用关系

上游主链由源码搜索确认：

- `pkg/session/runtime/planning.rs` 和 `pkg/session/runtime/typed_adapter_bridge.rs` 调用 `ExecStmt::Exec`；后者还在同步/异步结果包装关闭时调用 `FinishExecuteStmt`。
- `pkg/server/internal/resultset/resultset.rs` 把 server `ResultSet::TryDetach` 转发到本文件的 `RecordSet::TryDetach`；`pkg/server/internal/resultset/cursor.rs` 也沿 cursor 包装转发该能力。
- `pkg/session/runtime/explain_analyze.rs` 通过同一 `Exec` 入口执行 explain-analyze 内部语句。

本文件内部的关键调用边是：

```text
ExecStmt::Exec
  -> exec_inner
     -> PointGet
     |  buildExecutor -> openExecutor
     |  handlePessimisticSelectForUpdate -> runPessimisticSelectForUpdate
     `- handleNoDelay
        -> handlePessimisticDML -> runPessimisticDML
        `- handleNoDelayExecutor -> handleStmtForeignKeyTrigger

recordSet::Close / chunkRowRecordSet::Close
  -> ExecStmt::CloseRecordSet
  -> ExecStmt::FinishExecuteStmt
```

直接 crate 依赖可由 `pkg/executor/Cargo.toml` 复核：错误统一使用 `astersql-errors`；计划 trait 来自 `astersql-planner-core-base`/`astersql-planner-core`；字段类型来自 `astersql-parser-types`；chunk 来自 `astersql-util-chunk`；RU details 与 commit details 来自 `astersql-util-execdetails`；kill handle 来自 `astersql-util-sqlkiller`；RU 模型来自 `astersql-resourcegroup`；执行阶段指标由 `astersql-executor-metrics` 和 `astersql-metrics` 记录。

RustCodeGraph 的精确查询把 `ExecStmt` 同时定位到 `pkg/executor/adapter.rs:925` 和 Go 对照 `pkg/executor/adapter.go:391`，并把 Rust `FinishExecuteStmt` 定位到 `adapter.rs:1999`、`handlePessimisticDML` 定位到 `adapter.rs:1709`、`TryDetach` 的具体实现定位到 `adapter.rs:768`。图的宽泛 `explore`/`callees` 输出也确认了 Go 主链的 builder、session transaction manager、resource-group runaway、foreign-key 和 statement-RU 下游；Rust caller 查询在当前 CLI 中未返回结果，因此 Rust 上游采用 `rg` 复核，没有把空图结果解释成“无调用者”。

## 错误处理与边界

- `ExecStmt::Exec`、`ExecStmt::PointGet`、`recordSet::Next` 和 `detachedRecordSet::Next` 都捕获 panic；`panicError` 保留操作名和字符串 panic payload，未知 payload 记为 `unknown panic`。
- executor `Open` 失败时会尽力调用 `Close`；外键 cascade 子 executor 同样遵守 open/next/close 配对。`recordSet::Finish` 同时检查 executor close 和 CTE reset，优先返回首次清理错误并把它加入最终错误集合。
- snapshot 下禁止 write executor 和悲观 locking select；low-resolution TSO 下禁止 write executor/locking select。这些检查在真正执行或返回结果前发生。
- prepared statement 与 select-for-update 需要运行时显式声明支持，否则返回“session-bound typed table scan”错误，不能把当前 Rust 适配层视为无条件支持所有会话绑定计划。
- 悲观重试受 `MaximumPessimisticRetries` 限制；客户端 `LOAD DATA` 是一次性输入流，`canRetryPessimisticLoadData(FileLocRef::Client)` 返回 false，避免重开已经消费的输入。
- 外键 cascade 深度严格限制为 15；构建器返回 `None` 会结束当前 batch，只有成功执行、关闭和 commit 后才 `MarkBatchComplete`。
- SQL hint 移除器只识别 `/*+ ... */`；遇到未闭合 hint 时保留此前内容并丢弃未闭合段之后的内容。`redactSQL` 是内部简单引号替换函数，但当前日志路径使用 `StatementNode.secure_text`，不能将它描述为完整 SQL 解析器。
- `SelectRUDetailsForStatementLog` 仅在 RU version 2 且已有 total RU 时重写展示值；写语句把总量放到 write RU，其他语句放到 read RU，并保留 wait duration。缺失总量时原样返回 raw details。

## 并发与资源生命周期

适配层本身不创建线程或异步任务。`AdapterRuntime`/`ExecExecutor` 没有被强制为 `Send + Sync`，这是有意保留的会话线程亲和性；只有具体 detach executor 在拥有完整数据源时才可能自行实现跨线程能力。`detachedRecordSet` 不保留 `ExecStmt` 或 session，因此它的正确性依赖 `ExecExecutor::Detach` 返回真正独立拥有快照/资源的 executor。

共享状态主要通过 `Arc` 延长生命周期：运行时 `Ctx`、SQL killer、RU metrics/details、commit details 和 statement-RU owner/evidence/final snapshot 都可跨结果集克隆，但最终发布由 owner 的一次性 terminal setup 去重。`StatementRUFailureGuard` 与 `ForeignKeyTriggerGuard` 使用 `Drop` 保证错误展开时也执行状态恢复；它们不是后台任务。

`recordSet` 的 executor 从 `Open` 持续存活到客户端 EOF/Close，因而 tracker 和 CTE 也必须保留到 `Finish`。`chunkRowRecordSet` 在返回前已经关闭原 executor，只持有内存 chunks；`detachedRecordSet::Close` 使用 `Option::take` 确保底层 executor 只关闭一次。`OnFetchReturned` 只报告“客户端取回、仍可能有更多结果”的慢日志，不替代最终 `Close`。

锁资源由运行时拥有。本文件只确定 X/S lock 键集合、调用顺序和耗时：若 FK shared-lock 模式关闭，所有共享键并入排他集合；开启时，已写共享键升级为排他键，未写键保留共享锁。任何本地写状态查询错误都会向上传播，不会静默降级锁级别。

## 与 Go 版本的对应关系

主对照文件是 `pkg/executor/adapter.go`。Rust 保留了 Go 的核心形状和顺序：`ExecStmt`、`recordSet`、PointGet/Exec/RebuildPlan、外键 trigger/cascade、no-delay、悲观 SELECT/DML、锁错误重试、阶段耗时、FinishExecuteStmt、慢日志/summary/plan digest/TopSQL，以及 `moveWrittenSharedLockKeysToExclusive` 均有明确同名或等价符号。

主要表达差异如下：

- Go 直接依赖 `sessionctx.Context`、`exec.Executor`、`sqlexec.RecordSet`；Rust 用 `AdapterRuntime`、`ExecExecutor`、`RecordSet` trait 隔离会话和具体算子，使测试可注入最小实现。
- Go 使用 `context.Context` 和 `defer/recover`；Rust 用结构化 `ExecutionContext`、`catch_unwind` 与 RAII guard。Rust 的 `ExecutionContext` 显式携带 trace、RU details 和 canonical SQL killer。
- Go 的悲观 SELECT 返回物化 `chunkRowRecordSet`，Rust保持相同行为；Rust 额外用所有权和 `Option<Box<_>>` 明确 executor 关闭/转移状态。
- Rust 当前在 `exec_inner` 中显式检查 `SupportsPreparedExecution` 与 `SupportsSelectForUpdate`，反映 typed table scan 尚需 session runtime 绑定；这是一项迁移边界，不应从 Go 已支持推断 Rust 任意 runtime 都支持。
- Rust 增加 typed physical plan 与 statement-RU owner/evidence/finalized snapshot，用于按原始算子树做 RU V2 结算；注释明确缺失 typed plan 时不以摘要估算替代。
- Go 的 `adapter_test.go` 包含更宽的真实 session/failpoint 集成覆盖（如 TSO 等待计入超时、DML build cancellation、slow log）；Rust 的就近 `adapter_test.rs`/`adapter_internal_test.rs` 更聚焦纯适配契约，端到端 typed runtime 行为分布在 `pkg/session/runtime/*_test.rs`。

Go/Rust 已有对齐证据包括：`FormatSQL` 的 `QueryLogMaxLen` 字节限制；fast/no-result 计划分类；SELECT 与 DML 超时选择；written shared-lock key 的去重、升级和错误传播；结果集 close 的幂等性与错误保留。Go 版本仍包含 Rust trait 表面没有逐项暴露的具体 TiDB session 行为，因此扩展时应按行为逐项核对，而不是仅依赖同名函数。

## 扩展指南

- 新增计划分支时，先扩展 `PlanKind`/`PlanInfo`，再检查 `ExecStmt::exec_inner`、`IsFastPlan`、`isNoResultPlan`、`statementMaximumExecutionTime`、`recordAffectedRows2Metrics`、`SummaryStmt` 和 statement-RU 分类是否需要同步；不要只在 builder 中接线。
- 新增 executor 能力时优先在 `ExecExecutor` 提供最小语义，并在 `AdapterRuntime` 中放置会话/事务相关操作。保持 `Open` 失败后的 Close、正常 Close 幂等和 panic 转换契约。
- 改动悲观锁逻辑时必须维持顺序：执行/收集键、shared-key 升级、X lock、S lock、FK lock duration；同步更新 `pkg/executor/adapter_internal_test.rs`，并对照 Go 的 `TestMoveWrittenSharedLockKeysToExclusive`。
- 改动外键级联时检查 `MAX_FOREIGN_KEY_CASCADE_DEPTH`、savepoint、每批 commit、`ForeignKeyTriggerGuard` 和 error rollback。测试应放在独立 `*_test.rs`，不要内嵌进生产源文件。
- 改动结果集生命周期时同时覆盖 `recordSet`、`chunkRowRecordSet`、`detachedRecordSet` 以及 server/session 包装层，特别是 EOF、fetch error、close error、重复 Close、CloseWithError 和 detach 后不再持有 session。
- 改动 RU 结算时维持“first outcome wins”和“无 typed evidence 不猜测”两个不变量，并同步 `pkg/executor/statement_ru_plan_walk_test.rs`、`statement_ru_result_test.rs` 及 session runtime 测试。
- 改动日志文本时同步 `FormatSQL`、hint 移除、`GetTextToLog` 的会话/全局脱敏选择和 UTF-8 截断测试；避免把简单字符串处理扩展成不完整 SQL 解析器。
- 与 Go 移植对齐时以 `pkg/executor/adapter.go` 的对应增量和测试意图为准，只引入必要局部接线；当前文档不是补建整个 session/runtime 子系统的授权。

## 验证依据

本说明基于以下直接材料：

- 目标源码：`pkg/executor/adapter.rs`（完整阅读；主要锚点为 `ExecStmt::Exec`、`exec_inner`、三个 RecordSet、悲观锁/外键路径、`FinishExecuteStmt` 和日志/计划辅助函数）。
- crate 边界：`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`。
- Rust 上游/桥接：`pkg/session/runtime/planning.rs`、`pkg/session/runtime/typed_adapter_bridge.rs`、`pkg/session/runtime/explain_analyze.rs`、`pkg/server/internal/resultset/resultset.rs`、`pkg/server/internal/resultset/cursor.rs`。
- Rust 独立测试：`pkg/executor/adapter_test.rs`、`pkg/executor/adapter_internal_test.rs`；补充行为证据来自 `pkg/session/runtime/scan_adapter_runtime_test.rs`、`pkg/session/runtime_test/typed_adapter_bridge.rs`、`pkg/executor/statement_ru_plan_walk_test.rs` 和 `pkg/server/internal/resultset/resultset_aster_unit_test.rs`。
- Go 对照：`pkg/executor/adapter.go`、`pkg/executor/adapter_test.go`、`pkg/executor/adapter_internal_test.go`。
- RustCodeGraph：`status` 显示本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query ExecStmt --kind struct`、`query FinishExecuteStmt --json`、`query handlePessimisticDML --json`、`query TryDetach --json` 定位 Rust/Go 对应符号；`explore`/`callees` 提供 Go adapter 到 builder、session transaction manager、resource group、foreign key、statement RU 的调用证据。当前 CLI 对精确 Rust caller ID 的调用未返回结果，所以调用者结论另由限定目录的 `rg` 搜索复核。
- 结构验收使用任务指定命令，要求目标文档存在且恰好包含本文件列出的 11 个固定二级标题。本任务只写文档，按计划不运行 Cargo 或代码测试。

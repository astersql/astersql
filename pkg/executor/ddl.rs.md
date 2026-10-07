# `pkg/executor/ddl.rs`

## 文件定位

[`ddl.rs`](./ddl.rs) 是 `astersql-executor` crate 暴露的 DDL（Data Definition Language，数据定义语言）执行骨架；模块由 [`lib.rs`](./lib.rs) 的 `pub mod ddl` 纳入 crate。它位于 SQL 执行层与 Domain/DDL、InfoSchema、会话事务及临时表实现之间，但不直接依赖这些组件的具体类型，而是把生产边界收敛为 `DdlRuntime` trait。

当前文件应理解为“可注入运行时的行为模型”，而不是已经完整接入服务器的具体执行器：仓库级 Rust 搜索未发现 `DdlRuntime` 的实现，也未发现除本文件之外对 `DDLExec` 或 `GetDropOrTruncateTableInfoFromJobs` 的直接引用。RustCodeGraph 只报告了若干文件对 `ddl.rs` 中通用符号（例如 `statement_kind`、`execute`）的名称级使用关系；这些不是 `DDLExec<R>` 的生产构造或调用证据。因此，文档不能把 Go 主链的接线状态等同于 Rust 当前状态。

[`Cargo.toml`](./Cargo.toml) 声明 crate 名为 `astersql-executor`、库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/executor"` 标明 Go 移植来源。虽然该 crate 整体依赖 `astersql-ddl`、`astersql-domain`、`astersql-infoschema`、`astersql-sessiontxn`、`astersql-table-temptable` 等包，本文件本身没有 `use` 语句；所有具体依赖均通过 `DdlRuntime` 的关联类型和方法间接注入。

## 核心职责

该文件承担四组职责：

1. `DDLExec::Next` 定义 DDL 算子的一次性生命周期：清空输出块、以 `done` 防止重复执行、预处理本地临时表、进入语句级新事务、分发操作，并在成功后刷新事务 InfoSchema 和离开事务状态。
2. `dispatchDDL` 将 `DdlStatementKind` 映射为具体的 `DdlOperation` 或恢复/临时表专用路径；绝大多数普通 DDL 最终经 `executeOperation` 调用 `DdlRuntime::execute`。
3. `executeTruncateTable`、`executeCreateIndex`、`executeDropIndex`、`executeAlterTable` 等方法维护本地临时表的特殊规则；恢复与 flashback 方法把查找历史对象和真正恢复动作分开。
4. `GetDropOrTruncateTableInfoFromJobs` 按 job 类型、GC 安全点与历史快照逐项筛选，允许调用方回调决定是否提前结束。

文件不解析 SQL、不构造物理计划、不实现 Domain/DDL job 队列，也不自行访问存储。上述行为均由上游提供的 `Statement` 和 `DdlRuntime` 决定。

## 主要符号

- `DdlStatementKind`：解析后语句的分类枚举，共覆盖数据库、表、索引、视图、序列、表锁、恢复/Flashback、放置策略、脱敏策略和资源组等种类。`UnsupportedFlashbackTableToTimestamp` 与 `UnsupportedFlashbackDatabaseToTimestamp` 被显式拒绝；`Other` 当前直接成功返回。
- `DdlOperation`：可交给 `DdlRuntime::execute` 的操作枚举。它与 `DdlStatementKind` 大体对应，但不包含两个显式不支持项和 `Other`；恢复表/库走专用 runtime 方法，而非通用 `execute`。
- `LocalTemporaryPreprocess<TableName>`：临时表预处理的控制结果。`Continue` 进入普通 DDL，`Completed` 表示预处理已完成整个语句，`DropOnly(Vec<TableName>)` 表示只逐个删除本地临时表。
- `DdlRuntime`：无默认成功实现的强制运行时边界。关联类型描述上下文、输出块、语句、表名、表、DDL job、表信息、恢复库信息和错误；方法覆盖语句分类、事务、通用执行、DDL job 状态、InfoSchema 刷新、临时表、恢复对象及历史快照读取。
- `DDLExec<R>`：持有 `runtime`、当前 `stmt` 和一次性标志 `done`。字段均公开，当前没有构造函数；调用方必须自行保证初始状态通常为 `done = false`。
- `DDLExec::Next`：主入口。它不产出行，只通过 `reset_chunk` 清理请求块并执行一次副作用。
- `DDLExec::dispatchDDL` / `executeOperation`：前者完成语句分类路由，后者将 `DdlOperation` 与当前语句传给 runtime。
- `getLocalTemporaryTable` / `rejectLocalTemporaryTable`：查询临时表存在性并复用“该 DDL 不支持本地临时表”的错误路径。
- `executeRecoverTable`、`getRecoverTableByJobID`、`getRecoverTableByTableName`、`executeFlashbackTable`、`executeFlashbackDatabase`、`getRecoverDBByName`：恢复和 Flashback 的薄编排层；具体历史查找、重复对象检查、元数据构造等均由 runtime 负责。
- `GetDropOrTruncateTableInfoFromJobs`：独立的泛型筛选函数。回调签名为 `FnMut(Job, TableInfo) -> Result<bool, Error>`，返回 `true` 表示找到目标并终止遍历。

## 执行流程

`Next` 的主流程如下：

1. 无条件调用 `runtime.reset_chunk(request)`；即使 `done` 已为真，输出块也先被清理。
2. 若 `done` 已为真，立即返回 `Ok(())`；否则先把 `done` 设为真。后续任一步失败也不会让同一实例重试。
3. 调用 `preprocess_local_temporary_tables(&mut stmt)`。`Completed` 直接成功；`DropOnly` 顺序执行 `dropLocalTemporaryTables` 后返回；只有 `Continue` 会建立语句事务。
4. 调用 `new_transaction_in_statement(context)`，再由 `dispatchDDL` 对 `statement_kind(&stmt)` 分支。
5. 无论分发结果成功与否，都读取 `ddl_job_was_queued()` 并调用 `reset_ddl_job_state()`。如果分发失败，只有 `should_convert_to_schema_changed(was_queued, &error)` 为真时才经 `toErr` 转换，否则保留原错误。
6. 分发成功后依次调用 `refresh_transaction_infoschema()` 和 `leave_transaction()`。

`dispatchDDL` 的普通分支经名称对应的 `execute*` 方法进入 `executeOperation`。三个例外族需要注意：

- `TRUNCATE TABLE` 若 `statement_table_name` 能取到目标且该目标是本地临时表，则调用 `truncate_local_temporary_table`，不提交普通 DDL。
- `CREATE INDEX`、`DROP INDEX`、`ALTER TABLE` 先调用 `rejectLocalTemporaryTable`；目标是本地临时表时返回 runtime 生成的专用错误。
- `RECOVER TABLE` 根据 `recover_table_uses_name` 选择按名或 job id 查找；`FLASHBACK TABLE` 固定按名查找；`FLASHBACK DATABASE` 按库名取得恢复信息。这三条路径随后调用专用恢复方法。

`GetDropOrTruncateTableInfoFromJobs` 对每个 job 依次检查：是否为 DROP/TRUNCATE、快照是否晚于 GC 安全点、快照中是否还能取得表信息。只有三项均满足才调用回调；回调返回 `true` 时整体返回 `Ok(true)`，遍历耗尽则返回 `Ok(false)`。

## 数据与状态

`DDLExec` 自身只保存三个字段。`stmt` 可在临时表预处理中被修改；`runtime` 聚合所有外部状态；`done` 是实例级一次性闩锁。`Next` 在外部工作开始前就写入 `done = true`，因此它表达的是“已经尝试执行”，不是“成功完成”。

DDL job 的排队状态不存放在 `DDLExec` 字段中，而通过 `ddl_job_was_queued`、`reset_ddl_job_state` 从 runtime 读取和清理。该状态影响失败是否转换成 schema changed 错误。事务的 InfoSchema 与 in-transaction 状态同样由 runtime 维护，本文件只规定成功后的更新顺序。

本地临时表有两层状态表示：预处理阶段用 `LocalTemporaryPreprocess` 控制整个语句是否短路；单表操作阶段用 `Option<Table>` 表示目标是否存在。`getLocalTemporaryTable` 同时返回 `Option<Table>` 和派生的 `bool`，调用者目前主要使用存在性。

历史恢复筛选函数消费传入的 `Vec<Job>`，并把匹配的 `Job` 与 `TableInfo` 所有权交给回调；它不缓存结果。GC 安全点作为 `u64` 透传给 runtime 判断，文件本身不解释时间戳编码。

## 依赖与调用关系

模块装配关系为 `pkg/executor/lib.rs -> pub mod ddl -> pkg/executor/ddl.rs`。文件内的主要调用链是：

`DDLExec::Next -> DdlRuntime::preprocess_local_temporary_tables -> DdlRuntime::new_transaction_in_statement -> DDLExec::dispatchDDL -> execute* -> DdlRuntime::{execute|execute_recover_table|execute_recover_database} -> DdlRuntime::{refresh_transaction_infoschema, leave_transaction}`。

临时表支链为 `Next -> dropLocalTemporaryTables -> drop_local_temporary_table`，以及 `execute{Truncate,CreateIndex,DropIndex,AlterTable} -> getLocalTemporaryTable/rejectLocalTemporaryTable -> local_temporary_table`。历史恢复支链为 `GetDropOrTruncateTableInfoFromJobs -> job_is_drop_or_truncate_table -> job_snapshot_is_after_gc -> table_at_job_snapshot -> handle`。

RustCodeGraph 对 `pkg/executor/ddl.rs` 报告 75 个符号，并能解析上述文件内调用；但对 `Next`、`DDLExec` 等跨 Go/Rust 重名符号的 callers/callees 查询产生歧义。用仓库级精确搜索补充后，只确认 `pkg/executor/lib.rs` 的模块导出，未找到 Rust 生产侧 `impl DdlRuntime` 或 `DDLExec` 构造调用。因此当前可确认的是 crate API 可见性和文件内编排关系，不能确认端到端服务器调用链。

## 错误处理与边界

所有可失败边界统一使用 `R::Error`，并以 `?` 原样传播。文件只主动构造两类错误：不支持的 Flashback-to-timestamp 通过 `unsupported_ddl` 生成；本地临时表不支持的索引/ALTER 操作通过 `local_temporary_ddl_error` 生成。

`Next` 的 schema changed 转换有明确条件：先保存 job 是否入队，再清理 runtime 中的 job 状态；仅当分发失败且 runtime 判定应转换时调用 `convert_to_schema_changed`。事务创建失败发生在 job 状态读取/重置之前；临时表预处理与 `DropOnly` 也在该清理区段之外。与 Go 的 `defer` 清理不同，这意味着 runtime 若在这些早期阶段已经写入 job 状态，本文件不会兜底清除，具体实现必须避免这种状态泄漏。

成功收尾也不是不可失败的事务：`refresh_transaction_infoschema` 和 `leave_transaction` 都没有返回值，所以其实现不能把失败反馈给调用者。相反，`dropLocalTemporaryTables` 按输入顺序执行，遇到首个错误即停止，之前已删除的表不会由本文件回滚。

`DdlStatementKind::Other` 返回成功但不执行任何操作。新增 AST 类型若错误地分类为 `Other`，会造成静默无动作；因此语句分类器是安全边界。两个 to-timestamp 枚举则明确失败，避免伪装成已支持。

## 并发与资源生命周期

该文件没有线程、异步任务、锁、通道或显式引用计数。`Next`、runtime 方法和回调均要求可变借用，单个 `DDLExec` 实例按设计串行使用；是否可跨线程取决于 `R` 及其关联类型，代码没有声明 `Send`/`Sync` 约束。

资源生命周期以调用顺序表达：普通 DDL 在 `new_transaction_in_statement` 后运行，成功后刷新 InfoSchema 并离开事务。错误路径不调用 `refresh_transaction_infoschema` 或 `leave_transaction`，所需回滚/清理由 runtime 或更外层负责。job 状态在分发后立即清理，且先保存 `was_queued` 供错误分类使用。

一次性 `done` 防止重复提交同一 DDL，但它不是并发原语；并发调用 `Next` 既不受类型层保护，也不应由外部通过不安全共享实现。历史 job 扫描逐项串行调用 runtime 和 `FnMut`，回调可维护局部可变状态，首个匹配可提前结束以避免继续读取快照。

## 与 Go 版本的对应关系

直接对照文件是 [`ddl.go`](./ddl.go)。两版均具有 `DDLExec`、`Next`、`toErr`、本地临时表分流、按语句类型分发、DDL 后刷新 InfoSchema/退出事务，以及 `GetDropOrTruncateTableInfoFromJobs` 的恢复筛选语义。Rust 的 `DdlStatementKind`/`DdlOperation` 将 Go 类型 switch 和各 `ddl.Executor` 方法压缩为枚举分发；`DdlRuntime` 则把 Go 中的 `Domain`、session context、InfoSchema、temporary-table DDL 与 DDL executor 访问封装为注入边界。

当前 Rust 逻辑不是 Go 文件的完整等价实现，关键差异包括：

- Go `Next` 用 `defer` 无条件清除 `IsDDLJobInQueue` 和 `DDLJobID`；Rust 只在 `dispatchDDL` 返回后调用 `reset_ddl_job_state`。
- Go 对 `CreateMaterializedView*`、`AlterMaterializedView*`、`DropMaterializedView*` 等有显式分支；Rust 枚举尚无这些变体。
- Go 的具体方法包含禁止删除 `mysql` 库、创建视图前预处理、恢复表重复 ID 检查、AutoID 读取、重命名恢复对象等细节；Rust 文件把恢复和普通操作交给 runtime，无法从本文件证明这些约束已经实现。
- Go 的 `executeRenameTable` 会拒绝本地临时表；Rust 的 `executeRenameTable` 直接下发 `RenameTable`，没有调用 `rejectLocalTemporaryTable`。
- Go `GetDropOrTruncateTableInfoFromJobs` 委托 `ddl.GetDropOrTruncateTableInfoFromJobsByStore` 并从 Domain snapshot meta 取表；Rust 在本文件展开筛选循环，但所有判定和快照读取仍由 runtime 提供。
- Go 文件已由实际 executor builder/session 主链使用；Rust 仓库搜索未找到 `DdlRuntime` 实现，因此 Rust 版本目前更接近可测试的移植边界，而非已验证的端到端替代。

相关 Rust SQL 测试位于 [`test/ddl/ddl_test.rs`](./test/ddl/ddl_test.rs)，对应 Go 的 [`test/ddl/ddl_test.go`](./test/ddl/ddl_test.go)，覆盖事务内失败后执行 DDL、建表、临时表等用户可见场景；[`temporary_table_test.rs`](./temporary_table_test.rs) 还覆盖全局/本地临时表查询行为。但这些测试未直接引用 `DDLExec<R>` 或实现 `DdlRuntime`，不能作为本文件每个分支的单元覆盖证据。

## 扩展指南

新增 DDL 种类时至少同步检查四处：为 `DdlStatementKind` 增加分类、必要时为 `DdlOperation` 增加下发值、在 `dispatchDDL` 增加穷尽分支，并让具体 runtime 的 `statement_kind`/`execute` 实现保持对应。不能暂时落入 `Other`，因为该分支会静默成功。

若新增操作涉及本地临时表，应先决定它属于预处理短路、专用本地实现、显式拒绝还是普通 DDL。可复用 `getLocalTemporaryTable`/`rejectLocalTemporaryTable`，但多表语句（例如 RENAME）不能只检查单个 `statement_table_name`；应保持 Go 版本逐目标验证的语义。

扩展恢复功能时，历史扫描条件必须继续同时约束 job 类型与 GC 安全点，并避免把 snapshot 中的共享元数据直接交给可修改路径。新增回调行为需测试“跳过非目标 job”“GC 前快照”“快照无表”“回调 false 继续”“回调 true 提前结束”和各层错误传播。

测试逻辑应继续放在独立 Rust 测试文件，不嵌入 `ddl.rs`。最贴近本文件的缺口是新增 `pkg/executor/ddl_test.rs`，用 mock `DdlRuntime` 验证一次性 `Next`、三种临时表预处理结果、job 状态清理与错误转换、成功收尾顺序、每个 dispatch 映射及历史 job 筛选。用户可见 SQL 行为仍应同步扩充 `pkg/executor/test/ddl/ddl_test.rs`，并与 Go `pkg/executor/test/ddl/ddl_test.go` 的意图保持一致。

兼容性风险集中在分类映射、错误转换条件、临时表例外和 Go/Rust 行为漂移；性能风险主要是历史 job 扫描中的逐 job 快照读取。若 runtime 的 `table_at_job_snapshot` 代价较高，应由 runtime 缓存或批量化，但不能放松 GC 与 job 类型过滤。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/ddl.rs` 报告该文件含 75 个符号；`node --file ... --offset 1/421` 完整读取 518 行并核对文件内调用。`explore` 确认 `Next -> reset_chunk/preprocess_local_temporary_tables/new_transaction_in_statement/dispatch/状态收尾` 等边。精确 callers/callees 因 Go/Rust 重名未产生可用输出，故未据此声称生产接线。
- Rust 源与装配：`pkg/executor/ddl.rs`、`pkg/executor/lib.rs`（`pub mod ddl`）、`pkg/executor/Cargo.toml`（crate、Go package 元数据和相关依赖）。该目录未发现 `doc.go`，所以没有额外包级 Go 契约可读。
- Go 对照：`pkg/executor/ddl.go`，重点核对 `DDLExec`、`toErr`、`Next`、临时表操作、恢复/Flashback 和 `GetDropOrTruncateTableInfoFromJobs`。
- 测试证据：`pkg/executor/test/ddl/ddl_test.rs`、`pkg/executor/test/ddl/main_test.rs`、对应 Go 测试 `pkg/executor/test/ddl/ddl_test.go` 与临时表测试 `pkg/executor/temporary_table_test.rs`。仓库精确搜索未发现直接实现 `DdlRuntime` 或直接实例化 `DDLExec<R>` 的 Rust 测试。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前以固定标题计数验证恰有 11 个二级章节，并人工复核无“已接入完整主链”等超出证据的结论。

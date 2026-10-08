# `pkg/session/runtime/dxf_session.rs`

源文件：[`dxf_session.rs`](./dxf_session.rs)

## 文件定位

本文件位于 `astersql-session` crate 的具体会话运行时中，由 [`pkg/session/runtime.rs`](../runtime.rs) 通过私有模块 `mod dxf_session` 接入。它把 `ConcreteSession` 适配为 `astersql_dxf_framework_storage`（下文简称 `dxf`）所需的 `SQLBackend`，并通过 `ConcreteSession::ImportTaskManager` 创建可访问 DXF 任务、子任务和导入作业系统表的 `dxf::TaskManager`。

其位置处于两层之间：上层是 `pkg/dxf/framework/storage`、`pkg/dxf/importinto` 以及 session 内的导入/分布式 DDL 流程；下层是 `ConcreteSession::execute`、`Domain` 的 InfoSchema/DDL 能力和规范 KV 事务。文件注释明确规定每个池租约拥有一个工作线程和一个规范会话，因此同一租约中的 `BEGIN`、业务 SQL、`COMMIT`/`ROLLBACK` 会落到同一个事务上下文。

[`pkg/session/Cargo.toml`](../Cargo.toml) 将本模块归入 `astersql-session`，并直接声明了本文件使用的 `astersql-dxf-framework-storage`、`astersql-meta-metadef`、`astersql-meta-model` 与 `chrono`；本文件没有条件编译项，也没有单独 feature 门控。

## 核心职责

1. `Backend` 为一个 DXF session lease 启动专属线程，在该线程内创建并持续持有唯一的 `ConcreteSession`，串行执行命令。
2. `impl dxf::SQLBackend for Backend` 将 DXF 的带 `%?` 占位符 SQL、事务时间戳读取和表模式切换请求转接到规范 session/Domain。
3. `bind` 与 `literal` 将 `dxf::Value` 安全地编码进当前 session 只接受 SQL 文本的执行入口；`cell` 将文本结果恢复成 DXF 所需的空值、二进制、时间或字符串值。
4. `ConcreteSession::ImportTaskManager` 幂等创建六张 DXF/IMPORT INTO 元数据表，并向 `dxf::TaskManager` 提供按租约新建真实 session 的工厂。
5. `Drop for Backend` 关闭命令发送端、等待工作线程退出；线程退出前尽力执行 `rollback`，避免把活动事务遗留在 session 中。

## 主要符号

- `Command`（私有结构体）：线程边界上的命令封装。`sql` 保存已绑定 SQL；`alter_table_mode` 选择按 schema/table ID 切换 `TableMode`；`read_txn_start_ts` 选择读取当前事务 `StartTS()`；`reply` 是一次性结果通道。后两种专用请求优先于普通 SQL 路径。
- `Backend`（私有结构体）：`sender: Mutex<Option<mpsc::Sender<Command>>>` 控制命令入口及关闭状态；`worker: Mutex<Option<JoinHandle<()>>>` 保存唯一工作线程。两个字段放入 `Option`，使 `Drop` 能且只能取走一次。
- `sql_error(error) -> dxf::Error`：统一把显示文本转换为 DXF 存储层错误，不保留原始错误类型。
- `literal(dxf::Value) -> String`：编码 `NULL`、有符号整数/十进制、无符号整数、字符串/JSON、字节串和 UTC 时间。字符串与 JSON 会转义反斜杠并将单引号加倍；字节使用十六进制字面量；时间格式化到微秒。
- `bind(sql, args) -> Result<String, dxf::Error>`：按 `%?` 拆分 SQL并逐个插入 `literal`。参数不足返回 `missing DXF SQL argument`，多余参数返回 `extra DXF SQL arguments`。
- `cell(column, value) -> dxf::Value`：先识别 `SHOW_NULL_CELL`，再调用 `row_codec::binary_runtime_bytes` 恢复二进制；对列名末段为 `create_time`、`start_time`、`state_update_time`、`end_time` 的值尝试解析 `NaiveDateTime` 并转为 UTC 时间；其余保留为字符串。
- `Backend::start(domain) -> Arc<Backend>`：创建 MPSC 通道和线程；线程内只创建一次 `ConcreteSession`，循环接收并回复命令。
- `dxf::SQLBackend::execute`：先绑定参数，再投递普通 SQL 命令并同步等待 `SQLResult`。结果行按列名经 `cell` 转换；`affected_rows` 取自 session 的 `last_dml_report`，无报告时为 0。
- `alter_table_mode_for_import` / `alter_table_mode_for_normal`：投递专用命令，目标分别为 `TableModeImport` 与 `TableModeNormal`。
- `txn_start_ts`：投递时间戳请求，从返回的首行首列读取 `u64`；事务未开启或结果缺失均报错。
- `Drop for Backend`：先移除 sender 令接收循环结束，再 `join` 工作线程。
- `ConcreteSession::ImportTaskManager`（本文件唯一公开新增方法）：准备系统表，并以 `SessionPool::with_factory` 创建 TaskManager。静态 `SCHEMA: Mutex<()>` 将同进程内的表初始化串行化。

## 执行流程

创建流程从 `ImportTaskManager` 开始：取得全局 `SCHEMA` 锁；用同一 `Domain` 新建 setup session；依次执行 `CreateTiDBGlobalTaskTable`、任务历史表、DXF meta 表、import jobs 表、subtask 表与 subtask history 表的建表语句。对以小写 `create table mysql.` 开头的常量补入 `if not exists`，然后构造带 factory 的 session pool。每次 pool `Get` 调用 factory，都会通过 `Backend::start(domain.clone())` 得到新的后端与新的规范 session；持久历史位于 mysql 系统表，不依赖这些 session 的寿命。

普通 SQL 流程为：DXF 存储层调用 `SQLBackend::execute` → `bind` 校验 `%?` 与参数数量并生成 SQL 文本 → 创建 reply 通道 → 在 sender 锁内确认后端未关闭并发送 `Command` → 调用线程同步等待 → 工作线程执行 `ConcreteSession::execute` → 每个结果集的每行按列转换为 `dxf::chunk::Row` → 从 `last_dml_report` 取得影响行数 → 回送结果。多结果集会顺序展平到一个 `rows` 向量。

事务由 `dxf::TaskManager::WithNewTxn` 驱动：DXF storage 在同一个 pool lease 上执行 `begin`、回调和结束操作。因为该 lease 的所有 `SQLBackend` 调用都进入同一个 `Backend` 工作线程和同一个 `ConcreteSession`，事务状态不会跨 session。`txn_start_ts` 的专用命令直接读取该 session 当前 transaction 的 `StartTS()`，所以同一事务内重复读取保持一致。

表模式切换不拼接 `ALTER TABLE` 文本，而是按 `(schema_id, table_id, mode)` 走 Domain DDL。工作线程先从 InfoSchema 查 schema/table，并用 stats table 判断模式是否实际改变；仅在需要改变时创建 `RuntimeDdlJobGuard`，调用 `ddl_set_table_mode_by_ids`，再以成功或经 `session_error("alter table mode", ...)` 包装后的失败结果结束 job。无变化仍调用 Domain 方法，但不创建 runtime DDL job；相关测试验证 schema version 不变。

关闭流程为：最后一个后端引用释放 → `Drop` 取走 sender → receiver 因所有 sender 消失而退出循环 → 尽力执行 `rollback` → `join` 等待线程结束。

## 数据与状态

- 会话状态只存在工作线程内的 `ConcreteSession` 中。普通 SQL、活动 transaction、`last_dml_report` 和读取到的 `StartTS` 都属于该唯一 session。
- `Command` 在调用线程与工作线程间转移 SQL 文本、控制标志和 reply sender；每次公开方法都创建独立 reply 通道，因此响应不会串线。
- `Backend` 的两个 `Mutex<Option<_>>` 分别保护并发发送/关闭和 worker 所有权。发送期间只短暂持有 sender 锁，等待 reply 时锁已释放。
- `SCHEMA` 只保护当前进程中 `ImportTaskManager` 的建表阶段，不保护跨进程竞争；跨进程安全依靠改写后的 `CREATE TABLE IF NOT EXISTS` 语义。
- `cell` 的类型恢复是有意受限的：空值依赖 session 的 `SHOW_NULL_CELL` 哨兵，二进制依赖 runtime 编码，时间只按四个约定列名识别；其他数字等查询结果也保持 `String`，除非由专用路径（如事务时间戳）构造为 `U64`。
- TaskManager 的持久状态在 mysql 系统表中。`ImportTaskManager` 每次可返回全新的 manager/session 工厂，销毁 manager 不会删除任务历史。

## 依赖与调用关系

上游直接入口包括：

- [`pkg/session/runtime/import_file.rs`](./import_file.rs) 的导入文件任务创建/查询流程调用 `ImportTaskManager`。
- [`pkg/session/runtime/modify_column_dist_backfill.rs`](./modify_column_dist_backfill.rs) 的分布式回填路径多次创建 manager。
- session 的独立测试入口 [`pkg/session/runtime_test/ddl.rs`](../runtime_test/ddl.rs) 直接验证表模式、事务时间戳、导入表空检查、统计增量与持久调度接线。
- DXF 测试如 [`pkg/dxf/importinto/scheduler_testkit_test.rs`](../../dxf/importinto/scheduler_testkit_test.rs)、[`pkg/dxf/importinto/clean_up_test.rs`](../../dxf/importinto/clean_up_test.rs)、[`pkg/dxf/importinto/job_testkit_test.rs`](../../dxf/importinto/job_testkit_test.rs) 与 [`pkg/dxf/framework/handle/handle_test.rs`](../../dxf/framework/handle/handle_test.rs) 使用真实 `ImportTaskManager` 覆盖任务表操作。

主要下游包括：

- `ConcreteSession::new`、`ConcreteSession::execute`：创建规范会话并执行建表、事务和任务表 SQL。
- `dxf::SQLBackend`、`dxf::sessionctx::Context::with_backend`、`dxf::util::SessionPool::with_factory`、`dxf::NewTaskManager`：定义适配契约并消费本后端。
- `Domain::info_schema`、`stats_table`、`ddl_set_table_mode_by_ids`：解析 ID、判断现态并执行模式 DDL。
- `begin_runtime_ddl_job`、`RuntimeDdlJobGuard`、`session_error`：将真实模式变化纳入 session runtime 的 DDL job 生命周期。
- `row_codec::binary_runtime_bytes`、`SHOW_NULL_CELL`：把规范 session 的文本行恢复为 DXF 值。
- `astersql_meta_metadef` 六个建表常量和 `astersql_meta_model::TableMode`：定义持久表及模式枚举。

RustCodeGraph 将 `ImportTaskManager` 定位在本文件第 273 行，并识别其下游 `execute`；图对跨 crate trait 动态分派和多数方法调用的调用边不完整，因此上述上游调用点又用 `rg` 的精确符号引用核对。

## 错误处理与边界

- SQL 参数数量必须与 `%?` 完全匹配；不足和多余参数均在发送前失败。该绑定器只识别字面量 `%?`，不解析 SQL 词法环境，因此调用方必须遵守 DXF storage 生成 SQL 的占位符约定。
- 字符串转义覆盖反斜杠和单引号；字节使用十六进制，避免按文本解释。`Decimal` 当前由 `dxf::Value` 持有的数值直接 `to_string`。
- sender/receiver/锁中毒/线程退出错误都转为只含文本的 `dxf::Error`。普通 SQL错误会添加 `DXF` 与 SQL 前三个空白分隔 token，提供操作上下文但避免把整条 SQL写进错误前缀。
- `txn_start_ts` 要求当前 lease 已执行 `begin`；无活动事务返回 `DXF transaction is not active`，返回行缺失则报 `DXF transaction timestamp missing`。
- 表模式切换允许 schema 或 table 查询不到时继续调用按 ID 的 Domain DDL，让权威 DDL 层返回错误；用于 job 描述的缺失名称退化为空字符串。DDL job guard 只包围实际模式变化。
- 结果转换对时间解析失败采取字符串回退，而不是报错；列名比较只看最后一个 `.` 后的精确小写名称。
- `command.reply.send`、退出时的 `rollback` 和 `worker.join` 都是尽力而为，返回值被忽略。调用方若已放弃 reply，不会使工作线程崩溃；worker panic 也不会在 `Drop` 中继续传播。
- `Drop::get_mut().unwrap()` 依赖析构时 mutex 未中毒；正常工作路径中发送侧的锁错误会返回 `dxf::Error`。

## 并发与资源生命周期

`Backend` 实现的并发模型是“外部可并发、内部严格串行”。`SQLBackend: Send + Sync` 允许 TaskManager 从多个调用点持有 `Arc<Backend>`；所有请求经 MPSC 排队，由唯一 worker 顺序访问非线程安全的 session 内部状态。`sender` mutex 还使关闭检查与发送成为一个临界区，避免拿到已移除 sender 后继续发送。

pool factory 并不复用一个全局 session：每次 lease 都创建新的 `Backend`、worker 和 `ConcreteSession`。同一 lease 内，`dxf::sessionctx::Context` 的克隆共享绑定后端，所以事务连续；不同 lease 则隔离事务状态。`SessionPool::Put` 本身为空操作，lease 的克隆引用全部释放后才触发 `Backend::drop`。

析构顺序是关键不变量：必须先关闭 sender，receiver 才会结束；之后才能 join，否则会死锁。worker 在循环结束后回滚，为异常中止或未完整结束的事务提供最后清理。`SCHEMA` 锁仅覆盖 setup 建表并在 `ImportTaskManager` 返回前释放，不进入长期 SQL执行路径。

当前实现每个同时存活的 lease 都占用一个 OS 线程，并且每个请求使用一个 reply channel；扩展高并发路径时需要评估线程数量和同步等待成本，不能直接把 session 移到多个线程并行访问。

## 与 Go 版本的对应关系

仓库中没有与 `pkg/session/runtime/dxf_session.rs` 同路径的一对一 Go 文件；对应语义分散在以下 Go 实现中：

- [`pkg/domain/domain.go`](../../domain/domain.go) 的 `Domain.InitDistTaskLoop` 用 `storage.NewTaskManager(do.dxfSessionPool)` 建立全局 manager，对应 Rust 将 session pool 接到 TaskManager 的总体位置。
- [`pkg/dxf/framework/storage/task_table.go`](../../dxf/framework/storage/task_table.go) 的 `NewTaskManager`、`WithNewSession`、`WithNewTxn` 和 `ExecuteSQLWithNewSession` 定义 session 借用与事务契约：同一个借用 session 上 BEGIN，回调成功则提交，失败则回滚。Rust 本文件用专属 worker/session 保住这一语义，而事务编排仍由 Rust DXF storage crate 完成。
- [`pkg/dxf/importinto/scheduler.go`](../../dxf/importinto/scheduler.go) 的 `getTaskMgrForAccessingImportJob` 在 user keyspace 场景使用 `SysSessionPool` 创建 manager，说明 import job 可能需要与任务执行 keyspace 不同的 session pool；Rust 各调用点当前通过相应 `ConcreteSession` 的 Domain 建 manager。
- [`pkg/dxf/framework/dxfutil/util.go`](../../dxf/framework/dxfutil/util.go) 的 `CheckTaskRuntime` 体现 Go 版本对 store/session keyspace 一致性的显式检查；本文件不自行执行同等 keyspace 校验，边界由传入 `Domain` 与上游 runtime 选择负责。
- Go session pool 直接返回实现 `sessionctx.Context` 的内部 session；Rust 的 `ConcreteSession` 不是 DXF crate 的同一类型，因此新增了 `SQLBackend`、值绑定/行转换、专用事务时间戳与表模式方法作为移植桥梁。
- Go 系统表通常在 bootstrap/upgrade 以及 Domain 初始化阶段准备；Rust `ImportTaskManager` 为当前可执行 runtime 额外做六张表的幂等初始化。这是 Rust 当前实现的显式自包含行为，不应误写成 Go 同函数的逐行翻译。

## 扩展指南

- 新增通用 DXF SQL 值类型时，同时修改 `literal` 与 `cell` 的往返规则，并在独立测试文件中覆盖引号、反斜杠、空值、二进制、时间精度及参数数目错误；不要把测试内嵌进本生产文件。
- 新增必须读取 session 内部状态或调用 Domain API 的后端操作时，应扩展 `dxf::SQLBackend` 契约和 `Command` 的明确操作类型，并继续在 worker 线程执行；不要从调用线程直接借用 `ConcreteSession` 内部状态。
- 新增命令分支时必须定义优先级或改为显式枚举，避免目前 `read_txn_start_ts`、`alter_table_mode`、普通 SQL 三路标志形成歧义；同时保证每个分支恰好发送一次 reply。
- 修改事务生命周期时应同步检查 Rust DXF storage 的 `WithNewTxn`、`SessionPool` lease 语义和 `Drop` 关闭顺序，并保留“同一 lease 同一 session”不变量。
- 扩展表模式 DDL 时优先复用 Domain 的按 ID 权威入口，并决定是否需要 `RuntimeDdlJobGuard`、no-op 检测和 schema version 不变测试。
- 新增系统表时修改 `ImportTaskManager` 的常量列表，并确认常量是否已经含 `IF NOT EXISTS` 或是否符合当前小写前缀改写规则；还应核对 Go bootstrap/upgrade 定义，避免表结构漂移。
- 最接近的现有独立回归位置是 [`pkg/session/runtime_test/ddl.rs`](../runtime_test/ddl.rs)。DXF 存储契约变化还应同步 `pkg/dxf/framework/storage/*_test.rs`，导入调度语义则同步 `pkg/dxf/importinto/*_test.rs`。按照仓库约定，Rust 测试继续放在独立测试文件中。
- 兼容风险集中在 SQL 字面量格式、系统表 schema、Go/Rust transaction 语义与 keyspace 选择；性能风险集中在每 lease 一个线程和逐请求同步通道。任何优化都必须先证明不会让同一事务跨 session。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/session/runtime/dxf_session.rs --offset 1 --limit 500` 返回目标文件完整 301 行；`query ImportTaskManager` 定位公开入口；`callees ImportTaskManager` 识别 `execute` 下游。图未完整恢复跨 crate 动态分派，调用点以精确文本引用补充。
- Rust 源与模块边界：[`pkg/session/runtime/dxf_session.rs`](./dxf_session.rs)、[`pkg/session/runtime.rs`](../runtime.rs)、[`pkg/dxf/framework/storage/lib.rs`](../../dxf/framework/storage/lib.rs)、[`pkg/session/Cargo.toml`](../Cargo.toml)。
- 上游 Rust 调用：[`pkg/session/runtime/import_file.rs`](./import_file.rs)、[`pkg/session/runtime/modify_column_dist_backfill.rs`](./modify_column_dist_backfill.rs)，以及 `pkg/dxf/importinto` 和 `pkg/dxf/framework/handle` 中对 `ImportTaskManager` 的精确引用。
- Rust 独立测试：[`pkg/session/runtime_test/ddl.rs`](../runtime_test/ddl.rs) 的 `dxf_backend_alters_classic_table_mode_with_canonical_session`、`dxf_backend_exposes_current_transaction_start_ts_for_import_stats`、`import_scheduler_classic_table_empty_check_runs_on_canonical_session`、`import_scheduler_stats_delta_runs_on_canonical_transaction` 与后续 durable scheduler 用例；[`pkg/dxf/importinto/scheduler_test.rs`](../../dxf/importinto/scheduler_test.rs) 的 mock `txn_start_ts` 测试；[`pkg/dxf/importinto/scheduler_testkit_test.rs`](../../dxf/importinto/scheduler_testkit_test.rs) 的真实 manager 场景。
- Go 对照：[`pkg/domain/domain.go`](../../domain/domain.go) 的 `InitDistTaskLoop`；[`pkg/dxf/framework/storage/task_table.go`](../../dxf/framework/storage/task_table.go) 的 `TaskManager` session/transaction 方法；[`pkg/dxf/importinto/scheduler.go`](../../dxf/importinto/scheduler.go) 的 import job manager 选择；[`pkg/dxf/framework/dxfutil/util.go`](../../dxf/framework/dxfutil/util.go) 的 runtime/session keyspace 检查。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并额外检查文档内相对链接目标存在；人工复核重点为文件存在原因、命令/事务流程、错误边界、资源关闭顺序和安全扩展入口。

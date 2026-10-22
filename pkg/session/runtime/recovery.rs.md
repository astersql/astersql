# `pkg/session/runtime/recovery.rs`

## 文件定位

从 `mysql.tidb` 读取并解析 `tikv_gc_safe_point`，转换为 TiDB TSO 的 `physical << 18` 形式（`runtime_gc_safe_point`）。

## 核心职责

- 从 `mysql.tidb` 读取并解析 `tikv_gc_safe_point`，转换为 TiDB TSO 的 `physical << 18` 形式（`runtime_gc_safe_point`）。
- 从当前 `Domain` 隔离的历史作业中反向选取最新可恢复的 drop/truncate table 或 drop schema 快照（`runtime_recovery_job`）。
- 在恢复前强制 `safe_point <= job.real_start_ts`，防止使用已超过 GC 窗口的快照（`validate_recovery_safe_point`）。
- 恢复表元数据，可选改名，并强制关闭恢复表的 TTL 调度（`restore_runtime_table`）。
- 为表恢复建立可观测的 runtime DDL job，执行 failpoint 钩子，挂接恢复后表信息，并刷新当前节点 schema version（`execute_runtime_table_recovery`）。
- 实现 `RECOVER TABLE [BY JOB]`、`FLASHBACK TABLE [TO ...]` 和 `FLASHBACK DATABASE [TO ...]` 的会话级调度。

## 主要符号

- `ConcreteSession::runtime_gc_safe_point(&self) -> SessionResult<u64>`：要求查询结果严格为一行一列；支持秒精度和小数秒精度时间，并先剥离最后的时区名。
- `ConcreteSession::runtime_recovery_job(...) -> SessionResult<RuntimeDdlJob>`：按 `domain_id`、可选 job ID、作业类型、库名和可选表名匹配 `history.iter().rev()`，表/库名比较忽略 ASCII 大小写。
- `ConcreteSession::validate_recovery_safe_point(...) -> SessionResult<()>`：比较 GC safe point 与作业真实开始 TSO；大于时拒绝恢复。
- `ConcreteSession::restore_runtime_table(...) -> SessionResult<TableInfo>`：处理改名、TTL 关闭、同名表检查和 `Domain::ddl_create_table`。
- `ConcreteSession::execute_runtime_table_recovery(...) -> SessionResult<()>`：`RECOVER TABLE` 与 `FLASHBACK TABLE` 共享的恢复主流程。
- `ConcreteSession::execute_recover_table(...)`：支持按表名或正数 job ID 选择 drop/truncate 历史。
- `ConcreteSession::execute_flashback_table(...)`：按表名选取最新历史，并将非空 `NewName` 传入共享流程。
- `ConcreteSession::execute_flashback_database(...)`：选取 drop schema 历史，新建目标库后逐表恢复，最后更新 runtime 库集合、会话状态和 schema version。

本文件不定义新类型、trait、常量或条件编译项；所有方法均是 `ConcreteSession` 的 inherent impl，且除内部 helper 外只对父模块暴露三个执行入口。

## 执行流程

1. `dispatch.rs` 通过 AST downcast 识别语句，调用对应的 `execute_*` 入口；成功时返回无结果集。
2. 表恢复先确定数据库。`RECOVER TABLE` 允许 AST 没有表名（BY JOB），正数 `JobID` 直接按 ID 找 drop/truncate job；否则由 `runtime_recovery_job` 按库/表名找最新历史。`FLASHBACK TABLE` 总是按库/表名找历史。
3. `execute_runtime_table_recovery` 先校验 GC safe point，再从 job 的 `old_tables` 中选出目标表，并通过 `begin_runtime_ddl_job` 建立 `recover table` 作业。
4. 它安装 `RuntimeDdlJobGuard`，触发 `beforeRunOneJobStep` 值型 failpoint。当 `mockRecoverTableCommitErr` 与 `tikvclient/mockCommitError` 同时为真时，仅把作业 detail 标成重试；随后仍执行一次元数据恢复。
5. `restore_runtime_table` 应用新表名，关闭 `TTLInfo.Enable`，拒绝已存在的同名表，再调用 `Domain::ddl_create_table`。成功后将 `TableInfo` 挂接到恢复 job，并调用 `update_self_version_with_retry`。
6. `RuntimeDdlJobGuard::finish` 把成功或错误写回 runtime DDL 状态；若中途 panic/unwind，`Drop` 会以 owner 退出错误收尾活跃作业。
7. 库恢复使用类似前置检查，但建立 `flashback database` job，先 `ddl_create_database`，再串行遍历 drop-schema 快照中的所有表。成功后仅把第一张表挂到 job 的 `table_info`，然后更新全局/会话库集合和 schema version。

## 数据与状态

`RuntimeDdlJob` 定义在 [`pkg/session/runtime.rs`](../runtime.rs)，恢复逻辑主要消费 `id`、`domain_id`、`database`、`kind`、`real_start_ts` 和 `old_tables`，并为新 job 填充 `detail` 和 `table_info`。`old_tables` 保留被 drop/truncate 的 `TableInfo` 快照；恢复前会 clone，避免直接修改历史记录。

`domain_id = Arc::as_ptr(&self.domain) as usize` 是进程内隔离键，防止不同 MockStore/Domain 共享同一历史。这个键不是持久化标识，不能跨进程或重启使用。`RUNTIME_DATABASES` 也以同样的 domain 指针为键；库恢复后还会将库名写入 `self.state.borrow_mut().databases`。

GC safe point 是从系统表中读取的文本时间，解析成 UTC offset-aware `chrono::DateTime` 后取毫秒，然后左移 18 位与 `runtime_ddl_tso` 生成的 TSO 物理部分对齐。

## 依赖与调用关系

上游唯一直接调用文件是 [`pkg/session/runtime/dispatch.rs`](dispatch.rs)；RustCodeGraph 也将目标文件标记为“used by 1 file”。模块声明位于 [`pkg/session/runtime.rs`](../runtime.rs)。

主要下游依赖是：

- `Domain::restricted_stats_query`：读取 `mysql.tidb` 中的 GC safe point。
- `Domain::stats_table`、`Domain::ddl_create_table`、`Domain::ddl_database_names`、`Domain::ddl_create_database`：查询和写入 runtime 元数据，定义在 [`pkg/domain/domain.rs`](../../domain/domain.rs)。
- `begin_runtime_ddl_job`、`attach_runtime_ddl_table_info`、`update_runtime_ddl_detail`、`RuntimeDdlJobGuard`：定义在 `runtime.rs`，维护活跃/历史作业及异常收尾。
- `ConcreteSession::update_self_version_with_retry`：定义在 [`pkg/session/runtime/ddl.rs`](ddl.rs)，最多尝试十次 schema-version 更新 failpoint 路径。
- `astersql_meta_model::TableInfo` 与 `astersql_parser_ast`：承载表快照和语句 AST。

## 错误处理与边界

`runtime_gc_safe_point` 对缺行/多行、列数不符、时间格式无效、Unix epoch 之前时间分别返回 `SessionError`；底层查询错误以 `read GC safe point` 添加语境。时间仅支持源码中的两种 TiDB 格式，并假定字符串末尾可能有一个空格分隔的时区名。

历史选取只接受 `drop table`/`truncate table` 或 `drop schema`；按 job ID 恢复时不使用 AST 中的数据库过滤，因为 job ID 被当作唯一键。按名称恢复取反向遍历中首个匹配项，即最新历史。若快照没有目标表、快照早于 GC safe point、表/库已存在，则在元数据写入前拒绝。

表恢复是单表操作；库恢复则先建库再逐表建表，本文件没有显式回滚已建的库或前缀表。因此中途某表失败可能留下部分恢复状态；这是当前 runtime 路径的可见边界，不应描述为 Go DDL 事务语义。

## 并发与资源生命周期

`RUNTIME_DDL_JOBS` 和 `RUNTIME_DATABASES` 是全局 `Mutex` 保护状态。历史查找持锁期间只做过滤和 clone，不在锁内执行 Domain I/O。`begin_runtime_ddl_job` 使用全局递增 ID 和单调 TSO，并依靠 `Condvar` 保证同 domain、同 schema 下的冲突表/库作业按提交顺序运行。

`RuntimeDdlJobGuard` 是 RAII 生命周期边界：正常路径显式 `finish`，非正常退出由 `Drop` 将作业收尾为错误，避免活跃队列永久占位。恢复本身是同步、串行调用，本文件不启动 async task、线程或 channel。`RefCell` 库集合只在当前会话中可变借用。

## 与 Go 版本的对应关系

Go 主入口在 [`pkg/executor/ddl.go`](../../executor/ddl.go)：`executeRecoverTable`/`getRecoverTableByJobID`/`getRecoverTableByTableName`、`executeFlashbackTable`、`executeFlashbackDatabase`/`getRecoverDBByName` 对应 Rust 的三个公开执行分支与历史选取 helper。两者共同保留了 drop/truncate 类型限制、GC safe point 检查、同名对象冲突、改名恢复以及 TTL 强制关闭等意图。

Rust 文件目前只调用 runtime `Domain` 元数据接口并刷新本节点版本，未在本文件中实现上述持久化状态机。

Go `mockRecoverTableCommitErr` 通过首次 commit error 验证作业重试；Rust 组合同名 failpoint 后只更新 job detail，然后继续一次元数据尝试。

## 扩展指南

- 新增可恢复 DDL 类型时，先更新产生 `RuntimeDdlJob.old_tables` 的 drop/truncate 侧，再扩展 `runtime_recovery_job` 的 `kinds` 参数；不要仅放宽字符串而没有快照证据。
- 修改 GC 判定时，同步核对 `runtime_ddl_tso`、safe-point 文本格式和 Go `ddl.GetRecoverSnapshotTS`/`gcutil.ValidateSnapshot*`；边界应覆盖缺值、小数秒、时区、epoch 前时间和恰好等于 safe point。
- 扩展表元数据恢复时，集中修改 `restore_runtime_table`，并保留 TTL 安全不变量；涉及 auto ID、分区、placement、label rule 或 delete-range 时，需明确下沉到 Domain/DDL 持久化边界，不宜只在 session 层增加进程内状态。
- 扩展库恢复时，重点处理中途失败的原子性/补偿、所有表的 job 可观测性，以及同名对象并发创建竞态。
- Rust 测试必须保持为独立文件。当前直接回归位于 [`pkg/ddl/tests/serial/serial_test.rs`](../../ddl/tests/serial/serial_test.rs)，已覆盖 TTL、BY JOB、GC safe point、drop/truncate、flashback database 和 commit-failpoint 场景；新的 session-runtime 窄单元测试应放在同目录独立 `recovery_test.rs` 并由 `runtime.rs` 在 `cfg(test)` 下接线，不得内嵌到 `recovery.rs`。
- 兼容性风险集中在错误文本、大小写/改名、历史选择顺序和 Go DDL 状态机语义；性能风险集中在反向线性扫描全部历史和 flashback database 串行建表。

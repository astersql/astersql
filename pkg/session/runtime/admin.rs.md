# `pkg/session/runtime/admin.rs`

## 文件定位

校验管理对象：`execute_admin_check_table` 解析当前数据库、表和索引元数据，拒绝不存在的对象，并对带条件的 MV/列式索引实施当前运行时的快速检查限制。

## 核心职责

1. 校验管理对象：`execute_admin_check_table` 解析当前数据库、表和索引元数据，拒绝不存在的对象，并对带条件的 MV/列式索引实施当前运行时的快速检查限制。
2. 修复或清理二级索引：`execute_admin_index_maintenance` 复用关系型 DML 的行扫描、索引值生成和 mutation 提交路径，使 `ADMIN RECOVER INDEX`/`ADMIN CLEANUP INDEX` 产生真实 KV 变更与计数结果。
3. 暴露紧凑 DDL 运行时状态：`execute_admin_ddl` 支持显示、调整和取消当前 `Domain` 的运行中 DDL job；`execute_runtime_ddl_system_select` 为少数 DDL/全局任务系统表查询提供记录集。
4. 模拟可验证的区域拓扑：`execute_show_regions` 从表、分区、索引元数据和运行时拆分计数构造 `SHOW ... REGIONS` 结果；`execute_split_region` 记录拆分数量并返回 TestKit 兼容的两列结果。
5. 接入统计锁与 warning：`stats_lock_table`、`execute_lock_stats`、`execute_unlock_stats` 把 AST 目标转换为 statistics handle 所需的表/分区映射，并把跳过信息写回语句 warning。
6. 维护两个特殊系统表边界：表达式下推黑名单保存在会话状态中；`mysql.tidb` 的 INSERT/UPDATE/DELETE 则转交 Domain 的受限 SQL 存储路径，并保留事务可见性。

## 主要符号

- `impl ConcreteSession`：文件唯一的顶层实现块；没有模块级常量、类型、trait、宏或条件编译项。
- `execute_admin_check_table(&self, &ast::AdminStmt) -> SessionResult<()>`：仅接受 `CheckTable`/`CheckIndex`，解析每个目标并执行元数据级前置检查。当前实现没有逐行一致性扫描；成功仅表示这些检查通过。
- `execute_admin_index_maintenance(&self, &ast::AdminStmt) -> SessionResult<Option<ConcreteRecordSet>>`：实现 `RecoverIndex` 和 `CleanupIndex`。聚簇主键没有独立二级索引 KV 时直接返回零计数。
- `execute_admin_ddl(&self, &ast::AdminStmt, &str) -> SessionResult<Option<ConcreteRecordSet>>`：在 `RUNTIME_DDL_JOBS` 中显示、修改或标记取消 job；按当前 `Domain` 指针及 `Weak<Domain>` 双重隔离。
- `execute_runtime_ddl_system_select(&self, &str) -> Option<ConcreteRecordSet>`：只识别三个规范化后的精确 SQL 形态：查询 `mysql.tidb_ddl_job.job_id`，以及查询当前/历史 global task 的 `state`。
- `execute_show_regions(&self, &ast::ShowStmt) -> SessionResult<ConcreteRecordSet>` 与 `execute_split_region(&self, &ast::SplitRegionStmt) -> SessionResult<ConcreteRecordSet>`：读写 `RUNTIME_REGION_COUNTS`，按物理表/分区和可选索引生成稳定的区域结果。
- `set_warning`、`set_warning_with_code`、`set_note`：维护 `StmtCtx` 与 `SessionState.current_warnings`。warning 同时进入两处；note 只进入会话当前 warning 列表。
- `stats_lock_table`：返回 `(table_id, database.table, partition_id -> partition_name)`，是锁/解锁统计的共同目标解析器。
- `execute_lock_stats` 与 `execute_unlock_stats`：分别调用 statistics handle 的 `LockPartitions`/`LockTables` 和 `RemoveLockedPartitions`/`RemoveLockedTables`。
- `execute_expr_pushdown_blacklist_insert` 与 `execute_expr_pushdown_blacklist_delete`：仅拦截 `mysql.expr_pushdown_blacklist`，返回 `bool` 告知分派器是否已经消费语句。
- `execute_mysql_tidb_sql`：本文件唯一私有方法；有活动事务时调用 `restricted_system_sql_in_transaction`，否则调用 `restricted_stats_execute`。
- `execute_mysql_tidb_insert`、`execute_mysql_tidb_update`、`execute_mysql_tidb_delete`：仅识别 `mysql.tidb` 后调用上述私有方法，同样以 `bool` 表示是否消费。

## 执行流程

管理语句首先由 [`dispatch.rs`](dispatch.rs) 将 parser AST 向下转型并分流：`CheckTable`/`CheckIndex` 调用 `execute_admin_check_table`；`RecoverIndex`/`CleanupIndex` 调用 `execute_admin_index_maintenance`；`ShowDdlJobs`/`AlterDdlJob`/`CancelDdlJobs` 调用 `execute_admin_ddl`。`ReloadExprPushdownBlacklist` 和 `FlushPlanCache` 在分派文件自身处理，不属于本文件的方法。

索引恢复流程为：解析表与索引元数据 → `scan_registered_table` 扫描记录 → `relational_index_value_rows` 展开索引值（含多值索引情形）→ `encode_relational_index_value_row` 编码 KV → `read_raw_kv` 找出缺项 → 一次 `apply_relational_mutations` 提交 → 返回 `added_count` 与 `scan_count`。清理流程反向扫描 `GetTableIndexKeyRange` 对应的索引快照，用 `DecodeIndexHandle` 构造记录键；仅当记录键不存在时产生删除 mutation，显式 `Close` iterator 后提交并返回 `remove_count`。

区域流程由 [`dispatch.rs`](dispatch.rs) 的 `SplitRegionStmt` 和带 `ShowStmt` 的 `SHOW ... REGIONS` 分支进入。拆分按值列表长度或 `Num - 1` 计算每个物理表的 split 数，乘选中分区数形成返回值，并把“拆分后区域数”写入 `(domain_id, database, table, optional_index)` 键。展示时先解析表、索引与分区，再结合公开索引的已记录拆分数；未拆分索引与记录范围去重，最后按 `runtime_topology()` 轮转 leader，构造 11 列记录。

统计锁流程先决定语句是否是“首个表带显式分区”的分区锁模式；该模式只处理该表的所选分区。否则为所有表构造 `StatsLockTable` 后调用批量表接口。statistics handle 返回的 skipped 文本通过 `set_warning` 写入语句上下文。

特殊 DML 在通用关系型 DML 之前被 [`dispatch.rs`](dispatch.rs) 拦截。黑名单 INSERT 将 name、store type、可选 reason 规范化为小写三元组；DELETE 无 WHERE 时清空，有 WHERE 时借助 `insert_select_predicate` 对每行求值并保留不匹配项。`mysql.tidb` 三类 DML 不自行解释赋值或谓词，而把原 SQL 交给 Domain 的受限 SQL 实现。

## 数据与状态

- 表、索引、分区元数据来自 `self.domain.stats_table(...)`；未写入本文件的本地缓存。
- 索引维护读取 Domain storage 的版本化 snapshot，并通过 `apply_relational_mutations` 写入关系型 KV。恢复操作先完整收集缺项；清理操作先完整收集孤儿索引键，再各自批量提交。
- DDL 状态位于 `RUNTIME_DDL_JOBS` 全局 `Mutex`。查询合并 `active` 与倒序 `history`；修改只允许命中当前 Domain 的 active job。`CancelDdlJobs` 仅设置 `cancelled = true` 和 `detail = "cancelling"`，实际停止由 job 执行路径观察该状态。
- 区域拆分状态位于 [`runtime.rs`](../runtime.rs) 的 `RUNTIME_REGION_COUNTS: Mutex<HashMap<(u64, String, String, Option<String>), usize>>`。`runtime_domain_id` 使用弱引用清理已销毁 Domain，避免裸 `Arc` 地址复用导致旧计数泄漏到新 Domain。
- 黑名单的待加载集合是 `SessionState.expr_pushdown_blacklist`；`ADMIN RELOAD` 在 [`dispatch.rs`](dispatch.rs) 中复制到 `loaded_expr_pushdown_blacklist`。因此 INSERT/DELETE 只改变待加载内容，不直接改变当前已加载快照。
- warning/note 写入 `SessionState.current_warnings`；带 code 的 warning 还写入 `session_vars.StmtCtx`。空字符串被忽略。
- `mysql.tidb` 变更由 Domain 后端持久化。存在 `state.transaction` 时使用该事务，从而具备本会话可见、其他会话不可见以及 rollback 撤销的边界。

## 依赖与调用关系

上游主链是 `ConcreteSession::execute`/内部 AST 分派 → [`dispatch.rs`](dispatch.rs) → 本文件的方法。RustCodeGraph 把 `admin.rs` 标为被 `dispatch.rs` 和 [`statistics.rs`](statistics.rs) 使用；其中显式入口引用由补充文本搜索确认在 `dispatch.rs`，而 warning 辅助方法也供同一 runtime 的统计逻辑复用。测试模块由 [`lib.rs`](../lib.rs) 的 `#[path = "runtime/admin_test.rs"] mod runtime_admin_test;` 独立装配，测试逻辑没有嵌入生产源文件。

关键下游包括：

- `astersql-parser-ast`：`AdminStmt`、`ShowStmt`、`SplitRegionStmt`、统计锁 AST 与特殊 DML AST。
- `astersql-domain` 及本地 Domain 运行时：元数据查询、storage snapshot、受限系统 SQL、statistics lock handle。
- `astersql-kv`、`astersql-tablecodec`：索引范围、iterator、记录键与 index handle 解码。
- [`dml.rs`](dml.rs) 的 `relational_index_value_rows` 和 [`row_codec.rs`](row_codec.rs) 的 `encode_relational_index_value_row`：确保 ADMIN 恢复与正常 DML 使用一致的索引展开/编码规则。
- `astersql-statistics-handle`：`StatsLockTable` 及锁/解锁接口。
- [`runtime.rs`](../runtime.rs) 的 `RUNTIME_DDL_JOBS`、`RUNTIME_REGION_COUNTS`、`runtime_domain_id`、`ConcreteRecordSet`、拓扑与会话状态。多数名称经 `use super::*` 引入；这也意味着拆分模块时必须显式重建依赖列表。

[`pkg/session/Cargo.toml`](../Cargo.toml) 明确声明上述 parser、domain、KV、tablecodec、sessionctx 和 statistics crates；`nextgen` feature 只转发到配置 crate，本文件没有 `#[cfg]` 分支。

## 错误处理与边界

所有可失败入口以 `SessionResult` 传播错误，并用 `session_error(context, source)` 为存储、iterator、统计锁和数值解析错误添加动作上下文。对象解析失败会报告数据库/表/索引名；不属于各入口支持集合的 ADMIN 类型会返回明确的 `unsupported ...` 错误。

重要边界如下：

- `execute_admin_check_table` 对空表列表返回成功；对 `CheckIndex` 必须找到索引。MV/列式且带条件的目标会返回 executor 8273 限制错误。该函数目前只做元数据与条件限制检查，不能据此声称完成 Go 的全量表/索引一致性校验。
- 索引维护要求至少一个表，只使用 `statement.tables.first()`。PK-is-handle、common handle 或 `IndexInfo.Primary` 的主键没有待维护的二级 KV，返回零计数；其他不存在索引则报错。
- 清理扫描区分 KV not-found 与其他读取错误，且无论扫描成功与否都在闭包结果返回前显式关闭 iterator；若解码结果无 handle，也会失败而不是猜测删除。
- DDL job 的 `SHOW` 过滤只从 SQL 文本中识别简单的 `job_id = <i64>`；系统表查询更是精确字符串匹配，不是通用 SQL 执行器。poisoned 全局锁使用 `expect`，会 panic。
- `SHOW REGIONS` 会拒绝缺表/缺索引；不存在或未显式选择的分区会回退到表 ID。其拓扑、region ID、统计列多为确定性运行时模型，不应等同真实 PD/TiKV 指标。
- 统计分区模式由第一张表是否带 `PartitionNames` 决定；该分支只处理第一张表。对象与 statistics handle 错误均中止操作，非空 skipped 信息降级为 warning。
- 特殊表 DML 的 AST 形状、schema 或表名不匹配时返回 `Ok(false)`，让通用路径继续。黑名单 INSERT 少于两列会失败；DELETE 谓词求值错误会传播且不会替换集合。

## 并发与资源生命周期

`RUNTIME_DDL_JOBS` 与 `RUNTIME_REGION_COUNTS` 通过进程级 `Mutex` 串行访问。DDL 的 SHOW 在持锁期间完成过滤、格式化和排序；ALTER/CANCEL 在持锁期间原地更新 job。扩展这些分支时应避免在锁内执行阻塞 I/O 或回调，且必须保留 Domain 隔离，否则不同测试/实例会串扰。

索引清理 snapshot 和 iterator 的生命周期局限在 `with_storage` 闭包内，扫描结果先物化为 mutation；`iterator.Close()` 在返回闭包结果前执行。mutation 提交发生在释放 snapshot/iterator 之后。索引恢复也先收集 mutation 再提交，因此内存占用随待修复或待清理条目线性增长；当前没有 Go executor 的分批事务/背压机制。

`SessionState` 使用 `RefCell`，黑名单、事务和 warning 更新依赖单线程会话借用规则，不提供跨线程共享。`mysql.tidb` 若处于事务中复用现有 transaction；否则走 Domain 的独立受限执行路径。`runtime_domain_id` 的 `Weak<Domain>` 清理为区域状态提供生命周期隔离，但区域计数 map 本身不会在每次 Domain close 时主动逐键删除，而是在获取 ID 时回收身份映射。

## 与 Go 版本的对应关系

- [`pkg/executor/admin.go`](../../executor/admin.go) 的 `RecoverIndexExec`/`CleanupIndexExec` 是索引维护的主要对照。Go 版本使用 executor、分批事务和 DistSQL 扫描；Rust 紧凑运行时复用本地关系 KV 编码并一次收集 mutation，结果列意图一致但执行规模与容错机制并不等价。
- [`pkg/executor/operate_ddl_jobs.go`](../../executor/operate_ddl_jobs.go) 的 DDL job cancel/alter executor，以及 planner 构造出的 show jobs executor，是 `execute_admin_ddl` 的对照。Rust 只操作当前进程的 `RUNTIME_DDL_JOBS` 模型和有限配置字段。
- [`pkg/executor/split.go`](../../executor/split.go) 的 split executor、`getPhysicalTableRegions`，以及 [`pkg/executor/show.go`](../../executor/show.go) 的 SHOW 区域入口是 region 功能对照。Go 查询真实 split store/PD/TiKV；Rust 构造确定性拓扑，并保留 Go MockStore 的物理 region 去重语义。
- Go 中黑名单和 `mysql.tidb` 通过常规系统表/executor/受限 SQL 基础设施运行；Rust 在 session runtime 入口显式抢先路由，以避免通用关系行存储对这些特殊后端产生“表面成功但状态未变”的结果。

## 扩展指南

- 新增 ADMIN 类型时，先在 [`dispatch.rs`](dispatch.rs) 增加精确 AST 分派，再在本文件新增/扩展方法；不要把未知类型静默当成功。若属于完整 planner/executor 能力，应评估是否应在 `astersql-executor` 实现，而不是继续扩大紧凑 runtime。
- 扩展索引维护必须继续复用 [`dml.rs`](dml.rs) 与 [`row_codec.rs`](row_codec.rs) 的编码逻辑，并覆盖普通索引、唯一索引、多值索引、分区、PK-is-handle/common handle、坏 index value、部分失败与大数据量分批策略。性能风险集中在全量扫描与 mutation 一次性物化。
- 扩展 DDL job 字段或 SQL 过滤时，应把字符串解析替换为 AST/系统表查询能力，保留 Domain 隔离，并同步 job 生产者/消费者；锁内不要加入阻塞操作。
- 扩展区域模型时，保持 key 为 domain/database/table/index 的隔离、不拆分索引去重、分区物理 ID 和 auto-random/shard-row-id 边界算法，并明确哪些列是模拟值。真实 TiKV 行为应落到 RealTiKV 接口而非伪造更多指标。
- 扩展统计锁时，核对 Go executor 对多表+分区组合的约束；保持 warning code/`StmtCtx` 可见性，并同步 statistics handle 的独立测试。
- 扩展特殊系统表路由时，必须让 `bool` 的“已消费”语义保持明确，并测试事务内、事务外、回滚、重复键、谓词错误及非目标表回退。
- Rust 测试应继续放在独立文件，不嵌入 `admin.rs`。最直接的同步位置是 [`admin_test.rs`](admin_test.rs)；跨会话事务与区域拓扑可沿用 [`runtime_pessimistic_test.rs`](../runtime_pessimistic_test.rs)，索引/DDL 组合行为可在现有同目录独立测试中扩展。Go 对照变化时同步检查上述 executor 文件及其测试。

# `pkg/ttl/ttlworker/job.rs`

## 文件定位

本文件属于 `astersql-ttl-ttlworker` crate 的 `job` 模块，由同目录 `lib.rs` 以 `pub mod job` 暴露。crate 的包边界和迁移来源由 `pkg/ttl/ttlworker/Cargo.toml` 声明：包名为 `astersql-ttl-ttlworker`，`package.metadata.porting.go-package` 指向 Go 包 `pkg/ttl/ttlworker`。文件直接依赖同 crate 的 `job_manager::TtlSummary` 和 `session::PhysicalTable`，没有条件编译项。

它位于 TTL 表级作业生命周期的中间层：`job_manager.rs::JobManager::lock_new_job` 创建 `TtlJob` 并登记历史，`JobManager::finish_completed_jobs` 汇总扫描子任务后调用 `TtlJob::finish`。跨节点可靠状态并不由这里的 `JobStore` 提供；系统表事务由 `persistent.rs::PersistentJobStore` 负责，后者明确把系统表视为权威状态、内存 map 视为 worker 缓存。

## 核心职责

1. 定义四条 TTL 系统表 SQL 模板及其类型化参数构造器：完成 table status、删除 job 的 task、创建 job history、完成 job history（`FINISH_JOB_SQL`、`REMOVE_TASK_FOR_JOB_SQL`、`CREATE_JOB_HISTORY_SQL`、`FINISH_JOB_HISTORY_SQL` 及同名 snake_case 构造函数）。
2. 用 `JobSqlValue` 保存 Go `[]any` 所需的 SQL `NULL`、有符号整数、无符号整数和字符串语义，尤其保持分区名缺失时使用 SQL `NULL`。
3. 定义一次 TTL 作业的内存快照 `TtlJob`、完成后的历史快照 `JobHistory`，以及按物理表/作业索引这些对象的 `JobStore`。
4. 提供本地生命周期操作：`TtlJob::create_history` 创建未完成历史，`TtlJob::finish` 在仍持有当前作业时原子地更新几个内存集合。

本文件不执行 SQL、不持有数据库会话、不启动扫描或删除任务，也不负责事务提交/回滚。持久化完成流程见 `persistent.rs::PersistentJobStore::finish_job`；子任务汇总和调用本地完成逻辑见 `job_manager.rs::JobManager::finish_completed_jobs`。

## 主要符号

- `FINISH_JOB_SQL`：把 `mysql.tidb_ttl_table_status` 的 `current_job_*` 快照转入 `last_job_*`，随后清空当前作业字段；`WHERE table_id=%? AND current_job_id=%?` 是防止完成错误作业的条件。
- `REMOVE_TASK_FOR_JOB_SQL`：按 `job_id` 删除 `mysql.tidb_ttl_task` 的全部扫描任务。`PersistentJobStore::finish_job` 直接复用此常量。
- `CREATE_JOB_HISTORY_SQL`：插入 `mysql.tidb_ttl_job_history` 的运行中记录；`finish_time` 固定为 `FROM_UNIXTIME(1)`，状态参数由构造器写为 `running`。
- `FINISH_JOB_HISTORY_SQL`：写完成时间、文本摘要、过期/成功删除/错误删除行数，并把状态置为 `finished`。
- `JobSqlValue::{Null,Integer,Unsigned,String}`：SQL 参数的封闭值集合。构造器返回 `(&'static str, Vec<JobSqlValue>)`，模板借用静态字符串，参数由调用者拥有。
- `finish_job_sql(table_id, finish_time, summary, job_id)`：参数顺序严格对应完成 status SQL 的四个占位符。
- `remove_task_for_job(job_id)`：生成删除该作业全部 task 的参数。
- `create_job_history_sql(job_id, table, partition_name, expire_time, create_time)`：从 `PhysicalTable` 取得物理表 ID、父表 ID、schema 和表名，并把 `None` 分区名映射为 `JobSqlValue::Null`。注意函数签名先接收 `expire_time`、后接收 `create_time`，但 SQL 参数按 `create_time`、`expire_time` 排列。
- `finish_job_history_sql(job_id, finish_time, summary_text, summary)`：把 `TtlSummary::{total_rows,success_rows,error_rows}` 映射为历史表的 `expired_rows/deleted_rows/error_delete_rows`。
- `TtlJob`：公开字段包含作业 ID、owner ID、完整 `PhysicalTable` 快照、创建时间、过期水位和本地完成标记。
- `JobHistory`：保留作业与物理/逻辑表标识、名称、可选分区、时间和可选汇总。当前 `create_history` 始终把 `partition_name` 写成 `None`，并未读取 `PhysicalTable::partition_name`。
- `JobStore`：`active_jobs` 按 `physical_id` 索引，`tasks_by_job` 按 job ID 记录预期扫描任务总数，`history` 按 job ID 索引；三个容器均为 `BTreeMap`。
- `TtlJob::create_history` 与 `TtlJob::finish`：本文件仅有的两个状态变更入口，均要求调用者提供 `&mut JobStore`，由 Rust 可变借用保证同一调用期间的独占修改。

## 执行流程

正常的本地生命周期可由调用边还原为：

1. `JobManager::lock_new_job` 要求当前实例是 leader，取得 `PhysicalTable`，检查该物理表没有当前作业且满足调度间隔，并以 `PhysicalTable::expire_time(now)` 计算过期水位。
2. 它构造 `TtlJob`，以 `physical_id` 写入 `JobStore::active_jobs`，随后调用 `TtlJob::create_history`。该方法以 job ID 覆盖式写入 `history`，初始 `finish_time` 和 `summary` 均为 `None`。
3. 扫描/删除任务由其他模块执行；`JobStore::tasks_by_job` 只保存预期任务数，本文件不推进任务状态。
4. `JobManager::finish_completed_jobs` 从 `TaskManager::finished()` 选择当前 job 的已完成任务，若已完成数尚小于非零的预期任务数则跳过；否则经 `summarize_task_results` 生成 `TtlSummary`。
5. `TtlJob::finish` 先检查 `active_jobs[physical_id].id == self.id`。检查失败时返回 `false`，不改变 store 和自身状态；成功时移除活跃作业和任务计数，若存在对应历史则写入完成时间与汇总，最后设置 `self.finished = true` 并返回 `true`。
6. 返回成功后，`JobManager::finish_completed_jobs` 再清空 `TableStatus` 的 current job/owner，并返回完成的 job ID。持久化路径则由 `PersistentJobStore::finish_job` 在悲观事务中依序更新 status、删除 task、完成 history。

SQL 构造器自身只组装模板和参数，不包含上述调度流程，也不会调用 `WorkerSession::execute`。

## 数据与状态

- 时间在本文件的内存结构中统一为 Unix 秒 `u64`；SQL 构造器则接收已格式化的 `String`。这两种表示不会在本文件中互相转换。
- `TtlJob::table` 是锁定时的 `PhysicalTable` 克隆，因此包含 schema/table、逻辑与物理 ID、TTL 开关、定义版本和过期间隔等快照；后续元数据变化不会自动更新该对象。
- `active_jobs` 的键是不重复的物理表 ID，表达“每个物理表至多一个本地活跃作业”；`finish` 还以 job ID 二次校验，防止旧对象结束后来替换的新作业。
- `tasks_by_job` 的值是任务总数而非完成数；完成数来自 `TaskManager`。成功完成时整项删除。
- `history` 在创建和完成之间保存同一个条目。完成时即使历史条目缺失，`finish` 仍会删除 active/task 状态并报告成功；因此“存在历史”不是本地完成的硬前置条件。
- `JobSqlValue::Unsigned` 用于汇总计数，避免把 `u64` 计数强制转为有符号值；表 ID 使用 `Integer(i64)`，文本时间和状态使用 `String`。

## 依赖与调用关系

上游直接关系：

- `job_manager.rs::JobManager` 持有公开的 `store: JobStore`。
- `JobManager::lock_new_job` 构造 `TtlJob`、写 `active_jobs` 并调用 `TtlJob::create_history`。
- `JobManager::finish_completed_jobs` 调用 `TtlJob::finish`；它的摘要来自同文件的 `summarize_task_results`。
- `persistent.rs::PersistentJobStore::finish_job` 直接引用 `job::REMOVE_TASK_FOR_JOB_SQL`。仓库 Rust 引用检索未发现其他生产代码调用四个 SQL 构造函数；它们目前由 `job_test.rs` 直接验证。

下游直接关系：

- `session.rs::PhysicalTable` 提供表/分区标识和过期配置；`TtlJob` 保存它，history SQL 构造器读取其标识与名称字段。
- `job_manager.rs::TtlSummary` 提供三类计数及扫描错误；本文件持久化的数值字段只读取前三类，`scan_task_err` 需要由调用方整理到 `summary_text`，本文件不生成摘要文本。
- 标准库 `BTreeMap` 提供确定顺序的本地索引，但文件没有依赖异步运行时、锁、通道或线程 API。

crate 层面，`Cargo.toml` 的无条件依赖只有 `astersql-ttl-cache`；大量完整 worker 依赖被放在 `cfg(windows)` 下。本文件本身仅使用 crate 内模块和标准库，不能据此推断所有平台都已接入完整 Go 等价运行链。

## 错误处理与边界

- 四个 SQL 构造器是纯函数，不返回 `Result`；它们不解析时间、不验证空 job ID、不校验 SQL 执行结果，错误处理属于数据库执行层。
- `TtlJob::finish` 用 `bool` 区分“仍拥有并完成”与“不是当前作业”。后者是正常竞争/陈旧对象边界，不携带错误原因。
- ownership 校验只比较物理表槽位中的 job ID，不比较 `owner_id`。数据库权威路径 `PersistentJobStore::finish_job` 额外在 SQL 条件中校验 owner ID，并在影响行数不是 1 时返回 `SessionError::Execute`；两条路径的强度不同。
- `finish` 先验证再连续修改普通 map；单线程可变借用内不会出现部分失败，因为 map 操作不返回业务错误。但它不是数据库事务，也不能提供崩溃恢复或跨节点原子性。
- `create_history` 对相同 job ID 使用 `insert`，会静默覆盖旧记录；调用者必须保证 job ID 唯一。
- `create_history` 当前丢弃 `PhysicalTable::partition_name`，而 `create_job_history_sql` 可由显式参数正确生成 `NULL` 或分区名。扩展持久化接线时不能假定内存历史已经保存分区名。
- SQL 模板使用 `%?` 方言占位符并依赖参数顺序。调整列、模板或函数签名时必须同步修改构造顺序和测试。

## 并发与资源生命周期

本文件没有内部互斥锁、原子变量、任务、通道、文件句柄或网络/数据库连接。并发控制依赖两个外部边界：Rust 侧由 `&mut JobStore`/`&mut self` 阻止同一对象的并发可变访问，分布式侧由 `PersistentJobStore` 的悲观事务、行锁和 owner/job 条件负责。

资源生命周期为：`TtlJob` 创建并放入 `active_jobs` → history 以未完成状态登记 → 外部任务管理器执行任务并在 `tasks_by_job` 记录规模 → 完成时移除 active/task 两项、保留并补全 history。`JobManager::gc` 只在 leader 上按 `JobHistory::create_time` 与保留期清理历史；本文件不负责 GC。

与 Go 的 `ttlJob` 不同，Rust `TtlJob` 没有 `status` 互斥保护，也没有 `Cancel` 方法。若未来从当前同步内存模型演进到可异步取消的共享作业，不能只在此结构上增加普通字段；需要同时设计锁/原子状态、取消传播以及与持久化 owner 校验的顺序。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ttl/ttlworker/job.go`：

- 四个 Rust SQL 常量分别对应 Go 的 `finishJobTemplate`、`removeTaskForJobTemplate`、`createJobHistoryRowTemplate`、`finishJobHistoryTemplate`；四个 Rust 构造器保持 Go 参数的列顺序和 `running`/`finished` 状态值。
- Go `createJobHistorySQL` 从 `tbl.Partition.O` 决定 SQL `NULL` 或分区名；Rust 把这一选择显式化为 `partition_name: Option<&str>`。`job_test.rs::create_history_preserves_go_partition_null_semantics` 覆盖两种分支。
- Go 使用 `time.Time` 并在构造器内按 `timeFormat` 格式化；Rust SQL 构造器接受已格式化文本，内存模型使用 Unix 秒。因此时区和格式正确性已移到调用方，当前测试只验证既成字符串的排列。
- Go `ttlJob` 保存 `assignTime`、`tableID` 和受 mutex 保护意图的 `status`；Rust `TtlJob` 保存完整 `PhysicalTable` 和 `finished: bool`，没有 reassignment 时间、取消状态或 mutex。两者不是字段级完整移植。
- Go `ttlJob.finish` 使用新的 `context.TODO()` 在悲观事务中依序执行三条 SQL，即使原 job context 已取消也继续收尾；Rust `TtlJob::finish` 只操作内存。更接近 Go 持久化语义的是 `persistent.rs::PersistentJobStore::finish_job`，它执行 begin/commit/rollback、校验 job 与 owner、删除任务并完成历史，但 SQL 采用 Unix 秒参数而非本文件所有字符串构造器。
- Go finish 会把 SQL 执行错误包装后返回，并留有 `ttl-finish` failpoint；本文件没有错误/failpoint 路径。不能用本地 `bool` 完成结果替代 Go 数据库事务成功的证据。

Go 的相关回归主要位于 `job_manager_test.go`，其中锁定新作业的预期执行序列调用 `createJobHistorySQL`，并通过测试导出方法调用 `ttlJob.finish`。Rust 的直接单元测试独立放在 `job_test.rs`，符合源文件与测试文件分离要求。

## 扩展指南

- 修改 SQL 模板或参数：同时更新对应构造器和 `job_test.rs::sql_builders_match_go_templates_and_argument_order` / `create_history_preserves_go_partition_null_semantics`；再核对 `PersistentJobStore` 是否复制了相同 SQL，避免两个实现漂移。
- 接入构造器到真实会话：新增显式的 `JobSqlValue` → `session::Datum` 转换或统一参数类型，并在 `persistent_test.rs` 验证 begin/commit/rollback、影响行数、owner 变化和失败后的 `avoid_reuse`。不能只验证 SQL 字符串。
- 增加本地生命周期字段：从 `JobManager::lock_new_job` 的构造点、`TtlJob::create_history`、`TtlJob::finish`、`JobManager::gc` 全链检查，并在独立的 `job_test.rs` 或 `job_manager_test.rs` 增加回归；不要把测试内嵌进 `job.rs`。
- 修复分区历史快照：最小接入点是让 `create_history` 从 `self.table.partition_name` 克隆；同步验证分区与非分区两条路径，并与 `create_job_history_sql`、Go `tbl.Partition.O` 的语义对齐。
- 增加取消/抢占：先明确本地状态与 `PersistentJobStore::{heartbeat,takeover_timeout_for_job,finish_job}` 的一致性规则。需要保留旧 job 无法误完成新 job 的不变量，并把 owner ID 校验带到所有权威写入。
- 修改完成条件或任务计数：主要入口在 `JobManager::finish_completed_jobs`，本文件只负责通过 ownership 后清理。测试应覆盖零任务、部分完成、陈旧 job、历史缺失和同物理表替换作业。
- 性能方面，完成路径的三个 `BTreeMap` 操作为对数复杂度；只有在 profiling 证明大量本地作业造成瓶颈时才考虑更换容器，并先确认是否依赖确定迭代顺序。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ttl/ttlworker/job.rs` 确认目标文件被索引并有 21 个符号。
- RustCodeGraph `query` 确认主要定义及位置：`TtlJob`（`job.rs:114`）、`JobStore`（`:156`）、`finish_job_sql`（`:45`）、`remove_task_for_job`（`:62`）、`create_job_history_sql`（`:69`）、`finish_job_history_sql`（`:92`）、`create_history`（`:167`）。`node job.rs::finish_job_sql` 显示它引用 `FINISH_JOB_SQL`，并由 `job_test.rs` 导入。
- RustCodeGraph 对若干哈希 symbol ID 的 `node/callers/callees` 返回空结果，且精确 `callers` 查询未稳定完成；因此调用关系又用仓库引用检索交叉验证，没有把空图结果解释为“无调用者”。
- 已完整阅读：`pkg/ttl/ttlworker/job.rs`、`job.go`、`job_test.rs`、`Cargo.toml`、`lib.rs`；直接边界还核对了 `job_manager.rs`、`persistent.rs`、`session.rs`。目标包不存在 `doc.go`。
- Rust 引用检索确认：生产侧 `JobManager::lock_new_job` 调用 `create_history`，`JobManager::finish_completed_jobs` 调用 `finish`，`PersistentJobStore::finish_job` 引用 `REMOVE_TASK_FOR_JOB_SQL`；其余 SQL 构造器只在 `job_test.rs` 出现。
- 测试证据：`job_test.rs::sql_builders_match_go_templates_and_argument_order` 验证三组模板/参数；`create_history_preserves_go_partition_null_semantics` 验证 history 插入顺序和分区 `NULL`；`local_finish_updates_only_the_current_job_and_cleans_tasks` 验证陈旧作业无副作用、当前作业清理和历史回填。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定的结构命令验证文件存在且恰有 11 个固定二级章节，并人工复核所有“当前已接入/未接入”陈述都有上述源码或调用证据。

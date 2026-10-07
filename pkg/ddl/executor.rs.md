# `pkg/ddl/executor.rs`

## 文件定位

`pkg/ddl/executor.rs` 属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 以 `lib.rs` 为库入口，`pkg/ddl/lib.rs` 通过 `pub mod executor` 无条件公开本模块，并仅在 `cfg(test)` 下装配独立的 `executor_test.rs`。文件本身不是模块装配壳：它定义了一套可执行、可测试的 DDL 前端模型，包括简化的 schema 元数据、DDL job、会话状态、作业后端抽象以及大量 DDL 校验和变更入口。

在完整应用架构中，它对应 Go `pkg/ddl/executor.go` 的“SQL DDL 请求转换为 job，并等待 job 完成”职责。不过当前 Rust `Executor<B>` 并不是 Go 生产 DDL 主链的等价接线：仓库中它的直接实例化集中在 `pkg/ddl/*_test.rs`，默认测试后端 `MemoryJobBackend` 也会同步完成 job；生产侧 Go 仍由 `pkg/executor/ddl.go` 调用 `pkg/ddl/executor.go`，再交给 `JobSubmitter`、owner scheduler 和 worker。因而应把本文件理解为正在移植中的、对 Go 行为进行独立验证的内存实现，而不能据其公开模块就断言 Rust 服务主链已经使用它。

DDL 框架问题的答案也应据此分层：Go 生产实现是持久 job、owner failover、schema state/version 同步模型；本 Rust 文件表达了 job/state/取消/回滚等合同，但不提供持久系统表、owner 选举、真实 backfill 或跨节点 schema 同步。

## 核心职责

1. 用 `Ident`、`SchemaInfo`、`TableInfo`、`ColumnInfo`、`IndexInfo`、`ForeignKeyInfo`、`PartitionDefinition` 表达大小写不敏感的库表元数据；`Executor.schemas` 是以小写 schema 名为键的内存目录（`executor.rs:151-510, 914-930`）。
2. 用 `DdlAction`、`DdlJob`、`JobState`、`ObjectState` 表达 DDL 动作、job 生命周期和 Online DDL 对象状态；`DdlJob::is_rollbackable` 按动作和 schema state 判定取消边界（`executor.rs:190-234, 512-666`）。
3. 对建删库表、列、索引、外键、分区、字符集、自增/分片位、TiFlash、TTL/放置/缓存、表锁等操作执行前置校验，先修改内存目录，再构造 job（`Executor` 的公开方法，`executor.rs:970-2167`）。
4. 通过 `JobBackend` 隔离提交、查询历史/当前 job 和取消操作；`do_ddl_job_wrapper` 负责提交后的等待、kill 取消、自动暂停、警告和 schema version 收尾（`executor.rs:770-856, 2169-2328`）。
5. 提供可独立复用的兼容性辅助函数：字符集归并、表定义/交换分区校验、匿名索引命名、UTF-8 安全的注释截断、禁止删除系统表、锁 ID 迁移、轮询策略、reorg 环境快照、多 schema change 判定、repair table ID 保留等（`executor.rs:2330-3046`）。

## 主要符号

- 常量：`EXPRESSION_INDEX_PREFIX`、`TABLE_NOT_EXIST`、`DEFAULT_PLACEMENT_POLICY_NAME`、`TIFLASH_PENDING_TABLE_LIMIT`、`TIFLASH_PENDING_TABLE_RETRY` 是与 Go 命名/默认值对齐的公共合同；`COLUMNAR_STORE_TYPE_OVERRIDE` 是列存类型的会话覆盖键（`executor.rs:38-50`）。
- 状态与元数据：`ObjectState` 覆盖 `None` 到 `Public` 以及 `ReplicaOnly`；`JobState` 覆盖排队、运行、暂停、回滚、取消和同步终态。`TableInfo` 聚合列、索引、外键、分区、TiFlash、TTL、放置、缓存与锁状态（`executor.rs:190-510`）。
- `DdlAction` 与 `DdlJob`：动作枚举覆盖 schema/table/column/index/partition/策略/资源组/约束等类别；job 保存目标 ID、状态、SQL、错误、警告、schema version、冲突对象和字符串参数（`executor.rs:512-618`）。
- `DdlJobQueue`：以 `BTreeMap<i64, DdlJob>` 保证按 job ID 有序，拒绝重复 ID，并支持全量读取或访问器提前停止（`executor.rs:668-704`）。这是队列语义模型，不是持久化 job 表。
- `JobBackend`：四个同步方法 `submit`、`history_job`、`current_job`、`cancel` 定义等待循环所需的最小后端合同。`MemoryJobBackend::submit` 分配 ID 后立即将 job 标成 `Synced` 并移入 history，主要服务测试（`executor.rs:770-856`）。
- `SessionContext`：保存 query、connection ID、kill/shutdown 标志、当前 job ID、警告/提示、系统变量、会话表锁、多 schema change 收集区和最近 DDL 信息（`executor.rs:859-909`）。
- `Executor<B>`：持有后端、内存 schema、全局 ID、lease、待同步 TiFlash 计数、本地 schema version 和完成通知队列；`new`、`backend` 是公开构造/观测入口（`executor.rs:911-955`）。
- 核心提交方法：`submit_simple_job`、私有 `submit_simple_job_with_schema_state`、`do_ddl_job_wrapper` 形成所有简单 DDL 的汇合点（`executor.rs:2169-2312`）。
- 校验与恢复工具：`validate_table_definition`、`check_create_global_index`、`check_table_def_compatible`、`repair_table_definition` 和 `RepairTableRegistry` 维护对象结构及物理 ID 不变量（`executor.rs:2412-2511, 2878-3046`）。
- `TableAccessController`：独立表达全局只读与连接级读/写锁兼容矩阵；它与 `SessionContext.locked_tables`、`TableInfo.table_lock` 是不同层次的内存状态（`executor.rs:2803-2876`）。

## 执行流程

普通操作的主流程如下：

1. 公开方法先解析目标并校验语义。例如 `create_table` 校验标识符与完整表定义，列存索引会隐式要求一个 TiFlash 副本；`create_index` 检查列存在、主键可见、全局索引只能用于分区表且不能覆盖生成列（`executor.rs:1115-1168, 1471-1539`）。
2. 方法直接更新 `Executor.schemas` 中的内存元数据并取得 `schema_id/table_id`。部分操作具有额外原子性保护：`rename_tables` 先验证全部目标，再整体摘除源表；`truncate_table` 先分配新表 ID 并预迁移锁（`executor.rs:1235-1316`）。注意，这种“先改内存、后提交”的模型并未为所有提交错误实现目录回滚，是简化实现的重要边界。
3. 方法调用 `submit_simple_job`。后者转给 `submit_simple_job_with_schema_state`，先通过 `check_columnar_storage_for_job` 做 job 侧二次列存开关校验；若 `session.multi_schema_actions` 已启用，只收集动作而不提交，否则构造 `DdlJob`（`executor.rs:109-149, 2169-2222`）。
4. `do_ddl_job_wrapper` 校验 `involving_schema` 无重复，按动作写入或清空原始 SQL，调用 `JobBackend::submit`，记录返回的真实 job ID 到 `session.ddl_job_id`（`executor.rs:2224-2243, 2608-2646`）。
5. 等待循环先处理 kill：关机直接返回 `Cancelled`，否则调用 backend 取消；`Finished/CannotCancel/NotFound` 是取消命令终态，`Temporary` 等错误继续重试。随后优先读 history；`Synced` 合并警告、推进版本、清空 job ID 并记录通知，带 error 的终态转成 `JobFailed`（`executor.rs:2244-2284`）。
6. history 尚不可见时读取 current job；若为带磁盘原因的 `Paused`，返回 `JobAutoPaused`。循环超过 10,000 次返回 `Timeout`。代码会按动作计算快/普通/慢退避并限制为十倍 lease，但只把结果存入 `_interval`，没有 sleep/ticker，所以当前 Rust 后端若长期不完成会忙轮询（`executor.rs:2285-2311, 2648-2687`）。

特定分支还包括：

- `set_schema_tiflash_replica` 遍历普通表，kill 返回 `QueryInterrupted`，测试 failpoint 风格的 `batch_tiflash_abort` 则成功提前退出；达到 pending 阈值时当前内存实现只能再次检查退出信号，不能等待异步 schema cache（`executor.rs:1988-2042`）。
- `create_index` 在 job 成功后把新索引从 `None` 推进到 `Public`；真实多阶段 backfill 不在本文件执行（`executor.rs:1511-1538`）。
- `repair_table_definition` 通过名称、列类型、索引列序/类型、分区名称/边界匹配旧对象并复用物理 ID，最后重新运行表定义校验（`executor.rs:2878-2967`）。

## 数据与状态

- 名称键统一用 `to_ascii_lowercase`，原始展示名仍保存在结构体字段中；`Ident::key`、`schema/schema_mut`、`table/table_mut` 都遵循这一规则。它提供 ASCII 大小写不敏感语义，并不等同于完整 Unicode case folding（`executor.rs:160-177, 957-968, 2314-2327`）。
- `next_id` 是 schema、table、index、partition 共用的进程内分配器，`alloc_id` 饱和递增并确保至少为 1；它不是持久全局 ID 服务（`executor.rs:919-955`）。
- `DdlJob.state` 描述 job 生命周期，`schema_state` 描述 DDL 对象状态，两者不能混用。`is_rollbackable` 对 drop index、modify column、partition、multi-schema、flashback 等动作分别编码 Go 的阶段边界（`executor.rs:589-666`）。
- `schema_version` 同时存在于完成 job、`Executor` 和 `SessionContext.last_ddl_sequence`。成功 history job 使执行器版本单调取最大值，并把 job 版本记录到会话（`executor.rs:612-613, 886-888, 2262-2271`）。
- `warnings` 在 job 中按错误码聚合消息与次数；成功后 `append_job_warnings` 将一次警告直接加入会话，多次则形成汇总字符串（`executor.rs:610-611, 2622-2633`）。
- 表锁跨三处保存：表元数据的 `table_lock`、会话的 `locked_tables` 以及独立 `TableAccessController.locks`。truncate 的 `handle_lock_on_submit/finish` 只处理会话 ID 映射；调用者必须维持这些层次的一致性（`executor.rs:448-449, 880-881, 2584-2606, 2803-2876`）。
- `DdlReorgMeta` 固化 sql mode、警告、时区、资源组、版本与新 collation 开关，但实际 reorg/checkpoint/backfill 不在本文件（`executor.rs:2689-2745`）。

## 依赖与调用关系

crate 边界由 `pkg/ddl/Cargo.toml` 确认：本文件直接使用标准库集合/时间，并通过 `astersql-config` 读取全局列存类型，通过 `astersql-sessionctx-vardef` 和 `astersql-sessionctx-variable` 解释 `tidb_columnar_storage_enabled`。这些依赖在 manifest 的常规 `[dependencies]` 中，不依赖 Windows 专属依赖段。

Rust 内部调用主干为：

`Executor::{create_schema,create_table,...}` → `submit_simple_job` → `submit_simple_job_with_schema_state` → `check_columnar_storage_for_job` / `do_ddl_job_wrapper` → `JobBackend::{submit,history_job,current_job,cancel}`。

`pkg/ddl/lib.rs` 公开模块后，同 crate 多个独立测试直接使用其 API：`db_test.rs`、`db_table_test.rs`、`index_change_test.rs`、`index_modify_test.rs`、`foreign_key_test.rs`、`partition_test.rs`、`tiflash_replica_test.rs`、`repair_table_test.rs`、`table_modify_test.rs`、`table_split_test.rs`、`placement_sql_test.rs`、`masking_policy_test.rs`、`fail_test.rs` 等。代码搜索没有找到非测试 Rust 模块实例化本文件的 `Executor<...>`；因此其主要可观测上游目前是这些测试，而非 Rust SQL session 主链。

Go 对应调用关系不同：`pkg/ddl/executor.go` 的 `Executor` 接口主要由 SQL executor 调用，内部 `executor.DoDDLJobWrapper` 将 `JobWrapper` 送入 `limitJobCh`，由 job submitter 持久化，再由 owner scheduler/worker 执行；完成通知通道、ticker、session pool 查询 history/current job 构成真实异步等待路径。Rust 的 `JobBackend` 把这些设施压缩成可替换接口，未直接依赖 `job_submitter.rs`、`job_scheduler.rs` 或 `job_worker.rs`。

## 错误处理与边界

- `ExecutorError` 区分对象存在性、标识符/字符集/表定义/分区/ID/分片位错误，依赖与锁冲突，unsupported，以及 job 提交、失败、自动暂停、取消、查询中断和超时（`executor.rs:715-768`）。`Display` 当前只输出 `Debug` 表示，不携带 TiDB/MySQL 错误码映射。
- `check_identifier` 只检查非空和不超过 64 个 Unicode scalar values；保留字、字符合法性等解析器职责不在此处（`executor.rs:2330-2337`）。
- 字符集支持集合仅为 `utf8mb4/utf8/latin1/ascii/binary`，排序规则匹配使用前缀规则；这比 Go 的完整 charset registry 简化（`executor.rs:2339-2410`）。
- `validate_table_definition` 限制 1017 列、名称唯一、主键不可见性和全局索引条件，但没有覆盖 Go 全部 AST、类型、表达式与存储约束（`executor.rs:2412-2474`）。
- `drop_column` 拒绝删除最后一个可见列或被索引、生成列、TTL 依赖的列；`drop_index` 在没有替代前缀索引时保护外键；`modify_column` 保护绑定 masking policy 的列类型（`executor.rs:1374-1469, 1541-1603`）。
- `check_table_def_compatible` 校验交换分区的临时/对象类型、列数、索引数、列属性和 TiFlash 副本数，但没有逐项比较索引定义；扩展此逻辑时必须先与 Go 同路径实际检查项对齐（`executor.rs:2476-2511`）。
- `RepairTableRegistry::repair` 禁止系统库、要求 repair mode 和已拉取清单；失败时把旧定义放回注册表，保证可重试（`executor.rs:2969-3046`）。
- 多数公开 DDL 方法在后端提交前已修改内存目录，提交失败时未统一撤销；只有 truncate 锁和 repair 清单等个别路径显式回退。不能把它当作具备事务原子性的生产 catalog。

## 并发与资源生命周期

本文件没有 `async`、线程、锁、channel 或真实定时器。所有集合均要求 `&mut self` 串行修改，`JobBackend` 也是同步可变借用；并发安全应由外层所有权或同步设施保证，`Executor<B>` 本身没有声明跨线程合同。

job 生命周期由 backend 决定。`MemoryJobBackend` 的 current 插入和 history 迁移发生在一次 `submit` 调用内，因而不会模拟 owner failover、长时间 backfill 或通知竞争。`done_notifications` 仅在成功时压入 job ID，本文件没有公开消费方法；它是完成通知的简化记录，不等同于 Go `ddlJobDoneChMap` 的 channel 生命周期。

等待循环的资源清理要点是 `session.ddl_job_id`：成功、成功取消以及非重试取消错误会清空；shutdown、自动暂停、timeout 和“终态无错误”分支不都显式清空，调用方不能假定任意返回后均为 `None`。kill 时临时取消错误会保留 ID 并重试，这由 `executor_test.rs` 的 `RetryCancelBackend` 验证。

表锁生命周期在 truncate 时为“提交前复制旧锁到新 ID；成功删旧锁，失败删新锁”。`TableAccessController` 则允许不同连接共享读锁，写锁/本地写锁要求没有其他连接持锁；同一连接重新 `lock` 会替换自己的旧锁。开启全局只读前必须没有活跃锁，`cleanup` 同时退出只读并清空锁（`executor.rs:2584-2606, 2803-2876`）。

真实 Go 版本使用 `limitJobCh`、job 完成 channel、ticker、session pool、持久 job 表和 owner/worker；这些资源及其关闭/故障转移语义不能从本 Rust 文件推导。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/executor.go`。Rust 的 `Executor<B>` 对应 Go `Executor` 接口加私有 `executor` 实现的一部分，但将 TiDB 的 AST、infoschema、kv/meta、sessionctx 和持久 job 类型替换成自有简化结构。

保留得较明确的语义包括：

- Go `DoDDLJobWrapper` 的多 schema change 收集、involving schema 校验、job query 设置、提交后记录 job ID、kill 后系统取消、history/current job 检查、磁盘满自动暂停、成功警告合并和 last DDL 信息，在 Rust `submit_simple_job_with_schema_state` / `do_ddl_job_wrapper` 中都有对应分支（Go `executor.go:7916-8158`；Rust `executor.rs:2188-2312`）。
- Go `isRetryableDDLCancelErr` 将 finished/cannot-cancel/not-found 视为终态；Rust `CancelJobError` 和 `is_retryable_ddl_cancel_err` 保持同一分类（Go `executor.go:7927-7934`；Rust `executor.rs:785-804`）。
- Go `setDDLJobQuery` 对 TiFlash replica status 和 unlock table 清空 query；Rust `set_ddl_job_query` 相同（Go `executor.go:8236-8245`；Rust `executor.rs:2608-2620`）。
- Go 快/普通/慢 job 检查策略分别为 500ms、500/500/1000ms、500/500/1000/1000/3000ms，动作分类在 Rust `get_job_check_interval` 中基本对齐（Go `executor.go:8247-8305`；Rust `executor.rs:2648-2687`）。
- Go truncate 表锁的成功提交/结束处理对应 Rust `handle_lock_on_submit` / `handle_lock_on_finish`（Go `executor.go:8177-8203`；Rust `executor.rs:2584-2606`）。

关键差异与迁移状态：

- Go 提交走 `JobWrapper`、`limitJobCh` 和持久化 job submitter，并靠 channel/ticker 等待 owner worker；Rust 仅抽象 `JobBackend`，默认内存后端同步完成且计算出的轮询间隔未真正等待。
- Go 使用 `model.Job`、`model.TableInfo`、真实 infoschema/meta 和 session variables；Rust 使用本文件自定义的简化结构及字符串 `args`，无法表达完整参数版本化和兼容性合同。
- Go `getJobCheckInterval` 还包含 materialized view 动作；Rust `DdlAction` 没有相同细分，说明动作覆盖并非当前 Go 文件的完整一一映射。
- Go `NewDDLReorgMeta` 使用真实 SQL mode、时区、资源组和当前 meta version；Rust 从字符串系统变量解析并固定 `version: 1`。
- Rust 公开方法覆盖广泛语义，但许多动作只存在于枚举或辅助模型，并不意味着 Go 生产执行路径、worker 状态机和持久化实现已移植完成。

## 扩展指南

- 新增 DDL 动作时，至少同步检查 `DdlAction`、`DdlJob::is_rollbackable`、`get_job_check_interval`、`submit_simple_job_with_schema_state` 的前置 gate，以及 `ExecutorError` 映射；必须以 Go 对应 commit 的增量为边界，不借机补建完整 DDL 子系统。
- 新增/修改表结构校验时，优先接入 `validate_table_definition` 或目标操作的前置检查；若涉及交换分区、全局索引、repair table，则同步审查 `check_table_def_compatible`、`check_create_global_index`、`repair_table_definition` 的物理 ID 和数据兼容不变量。
- 改动等待/取消语义时，应通过新的自定义 `JobBackend` 构造 current/history/temporary-error 序列；不要把测试写入 `executor.rs`，应扩展同目录 `executor_test.rs` 或更聚焦的现有独立测试文件。特别要覆盖 job ID 清理、取消重试、自动暂停、终态 error 和 timeout。
- 改动具体 DDL 行为时按现有测试归属选择文件：库与表用 `db_test.rs`/`db_table_test.rs`，索引用 `index_change_test.rs`/`index_modify_test.rs`，外键用 `foreign_key_test.rs`，分区用 `partition_test.rs`，TiFlash 用 `tiflash_replica_test.rs`，repair 用 `repair_table_test.rs`，表锁/修改用 `table_modify_test.rs`，切分策略用 `table_split_test.rs`。
- 如果要把本执行器接入 Rust 生产主链，不能只新增调用点：需要先明确持久 job 表、owner 单写、schema state/version 同步、通知、取消、恢复和 backfill checkpoint 的实现边界，并将 `JobBackend` 的同步合同扩展为不会忙轮询的真实等待机制。这属于超出当前单文件说明任务的独立架构工作。
- 性能风险主要来自 `rename_tables` 和 `update_replica_status` 的全库扫描、`lock_tables` 的线性检查、忙轮询等待以及大量 clone/string args；兼容风险主要来自简化 charset/type/action/error 模型与 Go 演进漂移。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、目标 `pkg/ddl/executor.rs` 已索引且有 275 个符号；`files --filter pkg/ddl/executor.rs` 确认唯一目标；`node --file pkg/ddl/executor.rs --offset ... --limit ...` 分段读取了全部 3,046 行。自然语言 `explore "pkg/ddl/executor.rs Executor DoDDLJobWrapper key symbols call graph"` 返回了 `Executor` 的索引位置与仓库 blast radius，但精确 snake_case method 查询没有结果，因此调用关系再由源码和限定 `rg` 核验。
- Rust 源与装配：完整读取 `pkg/ddl/executor.rs`；读取 `pkg/ddl/Cargo.toml` 确认 crate、常规依赖、lib 入口与 Go package 移植元数据；读取 `pkg/ddl/lib.rs` 的 `pub mod executor` 和 `#[cfg(test)] mod executor_test`。
- Rust 测试：完整读取 `pkg/ddl/executor_test.rs`。其中 7 个测试覆盖队列可见性/排序、动作与 schema state 的回滚性、truncate 锁交接、取消错误分类，以及临时取消错误重试后清理 job ID。限定代码搜索还确认了按领域拆分的相邻独立测试文件。
- Go 对照：读取 `pkg/ddl/executor.go` 的 `Executor`/`executor` 定义、`DoDDLJobWrapper`、current/history 查询、锁迁移、完成 channel、query 设置、轮询策略和 `NewDDLReorgMeta`；同时用 `docs/agents/ddl/README.md` 定位完整生产链，但所有结论均以代码/测试复核，未把概览文档本身当实现证据。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构以任务指定命令检查 11 个固定二级标题。人工复核重点为：明确当前 Rust 接线状态，未把同步内存后端描述成生产持久 DDL，未宣称缺失的 owner/backfill/schema sync 已支持，并将测试建议保留在独立测试文件中。

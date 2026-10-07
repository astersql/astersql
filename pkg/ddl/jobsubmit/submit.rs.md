# `pkg/ddl/jobsubmit/submit.rs`

## 文件定位

本文件是 `astersql-ddl-jobsubmit` crate 的持久化提交核心：接收已经构造好的 `JobSpec`，在提交前校验和补齐 `Job`，为 job 及其新建元数据对象分配全局 ID，并把编码后的记录批量写入 `mysql.tidb_ddl_job`。crate 入口 `pkg/ddl/jobsubmit/lib.rs` 将本模块全部公开符号再导出；`pkg/ddl/jobsubmit/Cargo.toml` 指定 `lib.rs` 为库入口，并以 `package.metadata.porting.go-package = "pkg/ddl/jobsubmit"` 标明 Go 对照包。

Rust 应用中已有三类直接入口：普通 DDL 的 `pkg/session/runtime/normal_ddl_submit.rs::submit_and_wait`、跨 keyspace 的 `pkg/session/runtime/crossks_job_submit.rs::CrossKSJobSubmitter::submit_table_mode`，以及 `pkg/domain/crossks/ddl_submit.rs::SubmitOnlyBackend::submit`。前两者将会话池、系统表管理器、BDR 策略和重试策略装配为 `SubmitOptions`；后者还明确说明提交器本身不启动 owner、scheduler 或 worker。因此本文件位于“构造 DDL job”与“owner 调度并执行 job”之间，只负责可靠入队，不负责执行 schema 状态机、等待历史结果或启动后台服务。

Go 生产主链的对应入口是 `pkg/ddl/job_submitter.go::JobSubmitter.addBatchDDLJobs2Table` 调用 `pkg/ddl/jobsubmit/submit.go::SubmitBatch`。RustCodeGraph 对 Go 文件给出了 `addBatchDDLJobs -> addBatchDDLJobs2Table -> SubmitBatch` 调用边；Rust 调用点则由上述三个文件的实际 `submit_batch` 调用核实。

## 核心职责

1. `submit_batch` 执行整批提交前置工作：空批次快速成功、借用/归还系统会话、阻止 flashback cluster 期间的新 DDL、读取 BDR 角色与事务 `start_ts`、规范化并校验 involving schema、补齐 trace 标记、应用 BDR 限制和升级期暂停策略。
2. `generate_ids_and_insert_jobs_with_retry` 把“分配全局 ID”和“插入 job 表”放在同一个悲观事务中，使持久化顺序与 job ID 顺序一致；失败时清理尝试级副作用、回滚，并只对 `Retryable` 错误重试。
3. `required_global_id_count` 与 `assign_global_ids_for_jobs` 共同维护 ID 数量和消费顺序，覆盖建表/批量建表、建库、资源组、分区变更以及 truncate 等当前 Rust `JobArgs` 分支。
4. `insert_ddl_jobs_to_table` 生成 `mysql.tidb_ddl_job` 的批量 `INSERT`，写入调度所需的 `reorg`、涉及 schema/table ID、job 元数据、类型码和 processing 状态。
5. `job_schema_ids`、`job_table_ids`、`fill_args_with_sub_jobs`、`set_job_state_to_queueing` 等辅助函数把复合 DDL 的索引字段、子 job 参数和排队状态处理成 scheduler 可消费的形式。

本文件不拥有业务对象的长期状态，也不自行通知 owner。`notify_ddl_owner` 只是可选 notifier 的容错辅助，当前仓库没有它的 Rust 调用点；跨 keyspace domain 路径在 `SubmitOnlyBackend::submit` 成功后，通过编排层的 `notify_owner` 单独通知，并把通知失败视为不回滚已提交 job 的建议性失败（`pkg/domain/crossks/ddl_submit.rs`）。

## 主要符号

- `pub fn submit_batch(options: &SubmitOptions, specs: &mut [JobSpec]) -> Result<(), Error>`：公开批量入口。它会原地修改 `specs`，包括 job ID、名称规范化结果、`start_ts`、BDR role、trace 标记和状态；调用方不能把输入视为只读。
- `pub fn generate_ids_and_insert_jobs_with_retry(session, specs, options)`：事务/重试边界。至少执行一次，最多执行 `max_retry_count.max(1)` 次；成功提交后立即返回。
- `GlobalIdAllocator<'a>`：顺序消费预生成 ID 切片的内部游标；`next` 在数量不足时返回 `Invalid` 错误，`assign_table` 依次写表 ID 和分区 definition ID。
- `pub fn id_count_for_table`、`required_global_id_count`：计算对象 ID 加每个 job 自身 ID 的精确总量；`id_allocated` 为真时只保留 job ID 配额。
- `pub fn assign_global_ids_for_jobs`：先要求 `ids.len()` 与需求完全相等，再按与计数函数相同的分支顺序赋值，最后给每个 `job.id` 分配一个 ID。
- `pub fn lock_global_id_key` 与内部 `lock_global_id_key_with_backoff`：用事务 `start_ts` 尝试悲观锁；仅遇到 `WriteConflict` 时刷新当前版本作为新的 `for_update_ts` 并无限继续，其余错误立即返回。
- `pub fn insert_ddl_jobs_to_table`：为所有 spec 构造一条多 values INSERT，并以标签 `insert_job` 交给 `Session::execute`。
- `pub fn fill_args_with_sub_jobs`：当前只对 `MultiSchemaChange` 的每个子 job 写入 `v<version>:<Debug args>` 字节串。
- `pub fn make_string_for_ids`、`job_schema_ids`、`job_table_ids`：构造系统表的可检索 ID 列；通用集合会转换为十进制字符串后按字典序排序并去重。
- `pub fn set_job_state_to_queueing`：把主 job 以及 multi-schema 子 job 置为 `Queueing`。
- `pub fn notify_ddl_owner`：若提供 `OwnerNotifier` 则调用 `notify`，但故意丢弃通知错误；无 notifier 时无操作。
- `is_system_schema`、`job_has_system_schema`、`hex`：内部策略与编码辅助。系统 schema 集合包括 `mysql`、`information_schema`、`performance_schema`、`metrics_schema` 和 `sys`。

## 执行流程

`submit_batch` 的主流程如下：

1. 空 `specs` 直接返回，且不会借会话。
2. 从 `SubmitOptions.session_pool` 取出一个 `Session`。业务闭包结束后，无论成功或失败，都调用 `session_pool.put` 归还会话；Rust 测试 `submit_batch_enqueues_job_and_returns_session_to_pool` 验证成功路径归还一次。
3. 读取当前最小 job ID，并让 `SystemTableManager::has_flashback_cluster_job` 检查队列表；若存在 cluster flashback job，整批拒绝，不进入事务插入。
4. 通过会话读取 BDR role 与 `start_ts`。对每个 spec：规范化 involving schema 名称、检查其合法性，拒绝 version 为 0 的 job，设置 trace 存在标记、`start_ts` 和 BDR role。
5. 对非 CDC 来源、非 `none` BDR role、非系统 schema 的 job 执行策略检查。`MultiSchemaChange` 逐个子 job 检查类型和参数；普通 job 检查自身类型和 `spec.args`。任一被拒即终止整批。
6. 主 job（以及 multi-schema 子 job）先置为 `Queueing`。服务器处于升级状态且 job 未涉及任何系统 schema 时，再把 `admin_operator_system` 置真并把状态改为 `Pausing`。
7. 进入 `generate_ids_and_insert_jobs_with_retry`。每次尝试依次执行：`begin`、设置悲观事务、锁全局 ID key、把成功加锁所用的 `for_update_ts` 设置为 snapshot TS、一次生成整批所需 ID、原地分配 ID、运行可选的 pre-insert hook、构造并执行 INSERT、`commit`。
8. 尝试失败时，先执行该次 hook 返回的 cleanup，再在事务确实已经开始时 rollback。只有错误类型为 `Retryable` 才调用 backoff 并开始新尝试；其他错误直接结束。成功 commit 后 cleanup 被取消，防止撤销已提交副作用。

ID 分配的稳定顺序是“每个 spec 的对象 ID（若需要且未预分配）在前，job ID 在后”。例如含两个分区的 `CreateTable` 依次消费表 ID、两个分区 ID、job ID；`canonical_submit_assigns_job_table_and_partition_global_ids` 用 `[100, 101, 102, 103]` 固定了这一不变量。`RemovePartitioning` 复用第一个特殊分区 ID 作为 `new_table_id`；空 definition 会返回显式错误。`TruncateTable` 分配新表 ID和与旧分区等长的新分区 ID，`TruncateTablePartition` 只分配新分区 ID。

## 数据与状态

输入状态由 `pkg/ddl/jobsubmit/types.rs` 定义。`JobSpec` 包含可变 `Job`、类型化 `JobArgs` 和 `id_allocated`；`SubmitOptions` 持有会话池、系统表检查器、最小 job ID 提供器、可选升级状态、BDR 策略、尝试级 hook、重试上限与 backoff。`Session` trait 将真实存储操作抽象为事务、锁、全局 ID、snapshot 和 SQL 执行原语，使本模块既可接系统会话，也可被独立测试。

主要状态变化均发生在调用方传入的 `specs` 上：

- 名称和 `involving_schemas` 被小写规范化；合法性规则由 `Job::check_involving_schema_info` 提供。
- `job.trace_info_present`、`job.start_ts`、`job.bdr_role` 在提交前写入。
- `job.state` 通常为 `Queueing`，升级期的非系统 DDL 为 `Pausing`；multi-schema 子 job 保持 `Queueing`。
- 对象 ID 和 job ID 在每次事务尝试中重新分配。可重试插入失败后，下一次尝试覆盖这些字段，因此 hook 的 cleanup 必须清理由前一次 ID 建立的外部映射。
- 持久化行包含 `job_id`、`reorg`、`schema_ids`、`table_ids`、`job_meta`、数值类型码及 `processing`。`job_meta` 由 `Job::encode(&spec.args)` 生成，再编码成 MySQL `x'...'` 字面量。

`schema_ids` 对 rename tables、rename table、exchange partition 收集所有涉及 schema；`table_ids` 对 rename tables、exchange partition 收集所有涉及表，truncate table 保留旧/新表 ID 对。集合型 ID 先转十进制文本再按字典序排序，因此 `2` 与 `10` 的顺序是 `10,2`，不是数值排序；Rust 测试 `persisted_id_lists_use_go_lexicographic_order` 明确锁定了这一兼容行为。

## 依赖与调用关系

上游 Rust 调用关系：

- `pkg/session/runtime/normal_ddl_submit.rs::submit_and_wait -> submit_batch`：普通 DDL 持久化后轮询 history，直到 synced 或失败；truncate table 还通过 pre-insert hook 把新 ID写回原始 Go 兼容 JSON 参数。
- `pkg/session/runtime/crossks_job_submit.rs::CrossKSJobSubmitter::submit_table_mode -> submit_batch`：跨 keyspace 的 table-mode job 使用真实 target SQL 会话池提交。
- `pkg/domain/crossks/ddl_submit.rs::SubmitOnlyBackend::submit -> submit_batch`：提交成功后复制 job ID；通知与历史等待由更外层 `DdlBackend` 编排完成。

模块内下游关系由 RustCodeGraph 核对：`submit_batch -> set_job_state_to_queueing / is_system_schema / job_has_system_schema / generate_ids_and_insert_jobs_with_retry`；后者再调用 `required_global_id_count -> assign_global_ids_for_jobs -> insert_ddl_jobs_to_table`；插入函数调用 `fill_args_with_sub_jobs` 和 `Job::encode`。图查询没有完整识别 trait 动态派发，因此会话池、系统表、BDR 与存储边界还需以 `types.rs` trait 和实际调用点为准。

Cargo 的正常依赖只有 `astersql-meta-model` 与 `serde_json`。大量 DDL、KV、parser、sessionctx 等迁移依赖位于 `[target.'cfg(any())'.dependencies]`，该 cfg 恒为假，不能据此声称本文件在运行时直接链接这些 crate；真实集成通过 `types.rs` 的本地模型和 trait 适配器完成。workspace 根清单提供 `facade_ddl_jobsubmit`，而 `pkg/session/Cargo.toml`、`pkg/ddl/Cargo.toml` 与 `pkg/domain/crossks/Cargo.toml` 均声明了该 crate。

Go 主链的直接关系是 `JobSubmitter.addBatchDDLJobs2Table -> jobsubmit.SubmitBatch -> GenGIDAndInsertJobsWithRetry -> insertDDLJobs2Table`。Go 的 owner 通知为独立的 `NotifyDDLOwnerByEtcd`，同样不属于 `SubmitBatch` 的原子事务。

## 错误处理与边界

- 空批次是成功 no-op；空批次不会触碰会话或数据库。
- 获取会话、flashback 检查、BDR/start-ts 读取、schema 校验、事务原语、ID 生成、编码/SQL执行和 commit 的错误均向上传播。
- version 0 在 Rust 中返回 `Error::invalid("Job version should not be zero")`；Go 对照实现用 `intest.Assert`，因此生产错误形态并不完全相同。
- 生成的 ID 数量必须与 `required_global_id_count` 完全相等，过多和过少都返回 invalid；Go 的 allocator 依赖调用方严格配对且会直接索引，Rust 在这里增加了可恢复边界检查。
- `RemovePartitioning` 必须至少有一个特殊分区，否则 Rust 返回 invalid；Go 直接索引 `Definitions[0]`。
- `lock_global_id_key_with_backoff` 对 write conflict 没有固定次数上限，这与 Go 注释所述的锁冲突策略一致；刷新版本或其他锁错误会终止。外层事务重试则受 `max_retry_count` 限制。
- `begin` 失败时不 rollback 未开始的事务；测试 `begin_failure_does_not_rollback_an_unstarted_transaction` 明确验证了这一点。
- hook cleanup 只在失败尝试执行，成功尝试不执行；测试 `submit_batch_retry_runs_cleanup_only_for_failed_attempt` 验证第一次 ID 被清理、第二次 ID 成为最终 job ID。
- `notify_ddl_owner` 忽略通知失败，保持“job 已经持久化后通知只是提示”的语义，但当前没有生产调用点直接使用这个辅助函数。
- SQL 文本中的数字和布尔值来自内部类型，job bytes 以 hex literal 编码，ID 列以单引号包裹。本函数没有外部字符串绑定接口；扩展字段时仍应维持不可注入的编码方式。

## 并发与资源生命周期

会话的生命周期由 `submit_batch` 显式管理：先 `get`，业务闭包返回后统一 `put`。它没有异步任务、线程、channel 或长期持锁对象。函数本身要求 `&mut [JobSpec]`，同一批 spec 不能被 Rust 安全地并发修改；共享依赖通过 `Arc<dyn ...>` 注入，其线程安全约束定义在对应 trait 上。

跨提交者的全局顺序由持久化层保证，而不是进程内 mutex：事务先将会话设为 pessimistic，再锁 meta 全局 ID key，随后在同一事务内生成 ID 与插入 job。这样并发提交不会先分配较小 job ID却后插入，scheduler 可以从 min job ID 顺序查询。write conflict 时更新 `for_update_ts` 并退避，防止使用过期悲观锁时间戳。

一次事务尝试的资源顺序为 `begin -> lock -> generate/assign -> hook -> insert -> commit`。失败路径为 `cleanup -> rollback`；成功 commit 后清空 cleanup。需要注意，`specs` 是事务外内存：回滚不会自动恢复已经写入的 ID，重试会覆盖当前分支所涉及的 ID 字段。任何新增 hook 都必须把外部资源限制在单次尝试，并提供幂等 cleanup。

本文件只把 job 放入 durable queue。owner election、worker 生命周期、schema version 同步和 history 迁移属于下游 DDL 服务；普通 Rust 调用方在 `submit_and_wait` 中轮询 history，跨 keyspace domain 在提交后建议性通知 owner 并等待 history。

## 与 Go 版本的对应关系

Rust 主要符号与 `pkg/ddl/jobsubmit/submit.go` 一一对应：`submit_batch`/`SubmitBatch`、`generate_ids_and_insert_jobs_with_retry`/`GenGIDAndInsertJobsWithRetry`、`GlobalIdAllocator`/`gidAllocator`、ID 计数与赋值函数、全局 ID 锁、job 表 INSERT、ID 列生成、queueing 状态以及 owner 通知辅助。事务顺序、失败 cleanup、只重试 retryable 错误、write-conflict 锁循环、字符串字典序排序等关键语义保持一致。

已确认的差异和迁移边界：

- Go 通过真实 `context.Context`、KV transaction、meta mutator、session pool 和 etcd client 实现；Rust 将这些边界抽象进 `Session`、`SessionPool`、`SystemTableManager`、`BdrPolicy`、`OwnerNotifier` 等 trait，由 session/domain 适配器接入。
- Go 的 `SubmitBatch` 设置内部事务 source type，Rust trait 接口不表达 context/source-type；是否设置由具体 session 适配器负责，本文件内无法验证。
- Go 仅在 `TraceInfo == nil` 时初始化空结构并保留既有内容；Rust 模型只有 `trace_info_present: bool`，提交时总置真，不能承载 Go trace 详情。Rust 测试只证明存在标记。
- Go 的升级暂停调用 `ddlutil.PauseRunningJob`，可能失败并记录日志；Rust 直接写 `admin_operator_system = true` 和 `state = Pausing`，没有同等级 helper 错误路径。
- Go 当前 ID 分支还覆盖 `CreateMaterializedViewLog`、`CreateMaterializedView`、`CreateMaterializedViewShadow`，且 `job2TableIDs` 覆盖物化视图日志和 out-of-place cutover 的关联表；Rust `JobType`/`JobArgs` 分支在本文件中没有这些路径。因此不能把 Go 的全部 action 支持视为 Rust 已支持，新增相关 DDL 前必须补齐计数、分配、持久化 ID 列和独立测试。
- Go 的非 multi-schema `fillArgsWithSubJobs` 会调用 `Job.FillArgs(spec.Args)`；Rust 在 `insert_ddl_jobs_to_table` 中直接执行 `job.encode(&spec.args)`，只有 multi-schema 子 job需要预填 `encoded_args`。Rust 采用 `v<version>:<Debug args>`，是否与所有 Go 子参数编码完全兼容只由当前模型/测试部分覆盖，不能外推到未测试参数。
- Go 通知函数直接向 etcd key 写值并记录错误；Rust `notify_ddl_owner` 仅调用抽象 notifier，且当前生产编排直接调用 notifier 而非此 helper。

Rust 测试 `pkg/ddl/jobsubmit/submit_test.rs` 使用 mock trait 验证算法和事务顺序；Go 测试 `pkg/ddl/jobsubmit/submit_test.go` 还用嵌入式 unistore 验证真实系统表行、job 解码、flashback/BDR/升级行为与 failpoint 重试。二者测试层级不同，Rust 单元通过不等于完整存储集成等价。

## 扩展指南

新增一种需要全局 ID 的 job 时，必须成对修改 `required_global_id_count` 和 `assign_global_ids_for_jobs`，并保持每个分支的消费顺序完全一致；同时检查 `job_schema_ids`、`job_table_ids` 是否需要暴露多个关联对象。建议在独立的 `pkg/ddl/jobsubmit/submit_test.rs` 增加“精确 ID 数量、精确赋值顺序、预分配 ID 分支、空集合/缺失对象”测试，不要把测试嵌入生产文件。

新增持久化字段或参数编码时，应从 `insert_ddl_jobs_to_table`、`Job::encode` 和 `fill_args_with_sub_jobs` 三处审查，并与 `pkg/ddl/jobsubmit/submit.go` 的 `insertDDLJobs2Table`/`fillArgsWithSubJobs` 及 scheduler 解码端共同核对。系统表字段和 job type code 是跨版本持久化协议；不能只让 INSERT 编译通过而忽略旧节点、history 解码或调度查询兼容性。

修改事务/重试逻辑时，应维持四个不变量：ID 生成与 INSERT 同一事务；write conflict 锁循环与事务 retryable 循环分层；未成功 begin 不 rollback；每次失败尝试执行 cleanup 而成功尝试取消 cleanup。对应测试入口是 `submit_batch_retry_runs_cleanup_only_for_failed_attempt`、`lock_global_id_retries_write_conflicts_with_backoff` 和 `begin_failure_does_not_rollback_an_unstarted_transaction`。

修改 BDR、系统 schema 或升级逻辑时，应同步审查 `submit_batch_checks_flashback_bdr_and_upgrade_state`、Go 的 `TestSubmitBatchChecksAndPauseState` 以及真实适配器中的策略实现。特别是 multi-schema 必须逐子 job 检查，系统 schema 判断必须包含 `involving_schemas`，否则升级期间可能错误暂停系统 DDL。

若要补齐 Go 物化视图分支，应先扩展本 crate 的 `JobType`/`JobArgs` 模型，再同时补齐 ID 计数、ID 分配、table ID 索引列和编码测试；这属于明确的现有差距，不应通过通用 `_ => 0`/`_ => {}` 分支静默处理。

性能上，整个批次构造单条 INSERT，可减少往返但 SQL 大小随批次线性增长；`hex` 使用逐字节 `format!`，大 job metadata 会产生额外分配。若优化，必须保持 MySQL hex literal 与 Go `WrapKey2String` 的字节等价，并用相同批量边界验证。

## 验证依据

本说明基于以下直接证据完成，未运行 Cargo（该计划明确为纯文档分析）：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标 `pkg/ddl/jobsubmit/submit.rs` 已索引为 493 行、27 个符号。
- RustCodeGraph `node --file pkg/ddl/jobsubmit/submit.rs`：完整读取第 1–493 行，核对所有函数、内部 allocator、事务重试、ID 分支、SQL 编码和辅助函数。
- RustCodeGraph `query/callers/callees`：核对 `submit_batch`、`generate_ids_and_insert_jobs_with_retry`、`insert_ddl_jobs_to_table`、`notify_ddl_owner`；确认主要模块内调用边。图未返回 Rust 跨 crate caller，随后用仓库搜索核实实际调用点。
- RustCodeGraph `node addBatchDDLJobs2Table`：确认 Go 生产边 `JobSubmitter.addBatchDDLJobs2Table -> jobsubmit.SubmitBatch`，其上游为 `addBatchDDLJobs`。
- 已读 Rust 源/边界：`pkg/ddl/jobsubmit/Cargo.toml`、`pkg/ddl/jobsubmit/lib.rs`、`pkg/ddl/jobsubmit/types.rs` 中的 `JobSpec`/`Session`/`SubmitOptions`、`pkg/session/runtime/system_session.rs::table_mode_submit_options`、`pkg/session/runtime/normal_ddl_submit.rs`、`pkg/session/runtime/crossks_job_submit.rs`、`pkg/domain/crossks/ddl_submit.rs`。
- 已读 Go 对照：`pkg/ddl/jobsubmit/submit.go` 全部 546 行、`pkg/ddl/job_submitter.go::addBatchDDLJobs2Table`。
- 已读独立测试：`pkg/ddl/jobsubmit/submit_test.rs` 全部 565 行、`pkg/ddl/jobsubmit/submit_test.go` 全部 344 行。Rust 测试证明 ID 顺序、持久化类型码、字符串排序、规范化、重试 cleanup、write-conflict backoff、begin 失败边界和 reorg metadata 编码；Go 测试提供真实系统表与 failpoint 证据。
- 已读 DDL 权威入口：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`；其中关于 job-based/owner-driven 的概览仅用于定位，具体结论均以上述代码和测试复核。

人工复核结论：本文分别回答了该文件为何存在（可靠持久化 DDL job）、如何运行（校验、状态设置、事务内 ID 与 INSERT、重试/清理）、当前由谁调用、哪些职责在边界外，以及新增 action/字段/策略时必须同步的符号和测试；已明确记录 Rust 与 Go 的未对齐处，没有把未接线或仅 Go 支持的能力写成 Rust 现状。

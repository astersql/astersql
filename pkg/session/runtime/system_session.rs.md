# `pkg/session/runtime/system_session.rs`

## 文件定位

本文件位于 `astersql-session` crate 的 `runtime` 模块，由 `pkg/session/runtime.rs` 以公开模块 `pub mod system_session` 导出。它不是一个单纯的连接池实现，而是把会话运行时的 `ConcreteSession` 接到多个 DDL 侧抽象上的适配层：底层使用 `astersql_session_syssession::AdvancedSessionPool` 保存线程绑定的系统会话，上层暴露 `astersql_ddl_session::Pool`、job submit、system table、MDL、delete-range 和 durable job worker 所需接口。

直接装配入口是 `pkg/session/runtime/session_factory.rs`：`KeyspaceSessionFactory::prepare` 和 `prepare_normal_schema_runtime` 都通过 `SystemSessionPool::new_with_validator` 创建池，并用 `SystemSessionCallbacks` 将借出的会话注册到 schema coordinator。普通 DDL 服务由 `pkg/session/runtime/normal_ddl_service.rs` 持有同一个池；提交路径 `normal_ddl_submit.rs`、跨 keyspace 运行时和多项 DDL 测试也从这里借用会话。

`pkg/session/Cargo.toml` 将本 crate 声明为 `astersql-session`，并直接依赖 `astersql-session-syssession`、`astersql-ddl-session`、`astersql-ddl-jobsubmit`、`astersql-ddl-systable`、`astersql-infoschema-issyncer`、`astersql-domain-crossks`、`astersql-kv`、`astersql-meta`、`astersql-tablecodec` 等。本文件因此处在 session、DDL、元数据和 KV 四个子系统的边界上。

## 核心职责

1. 以 `ConcreteSystemContext` 把线程绑定的 `ConcreteSession` 实现为 syssession 的 `SessionContext`，统一内部 SQL执行、参数绑定、事务回滚和连接状态重置。
2. 以 `ConcreteDdlContext` 和 `SystemResources` 把上述系统会话包装成 DDL 的可销毁资源池；借出、归还、销毁、关闭均保留稳定 session ID，并触发协调器回调。
3. 以 `SystemSessionPool`/`SystemSessionLease` 提供 RAII 借用接口。lease 离开作用域时无条件回滚；资源不可复用或归还失败时销毁，而不是把脏事务放回池中。
4. 为 DDL job submit、system table、MDL、delete range 和 durable job worker 实现同一真实会话上的事务及查询能力，避免各子系统建立语义不同的伪会话。
5. 在 `ConcreteJobExecutionContext` 中承接需要 session/domain/KV 共同参与的 DDL 副作用，包括物化视图构建与调度、表资源配置、列/索引回填、分布式重组、分析任务和 RU 上报。
6. 在 `generate_index_backfill_records` 与 `write_index_backfill_records` 中实现事务型/ingest 型索引回填的数据扫描、表达式求值、索引编码、唯一性检查、源行加锁和最终写入。

## 主要符号

- `SystemSessionCallbacks`：拥有 `borrowed`、`returned`、`destroyed` 三个线程安全回调；默认实现为空操作。`session_factory.rs` 用它维护内部会话与 MDL 注册表。
- `SystemSessionPool`：公开池句柄。`new`/`new_with_callbacks` 最终进入 crate 内的 `new_with_validator`；内部以容量 5 创建 `AdvancedSessionPool`，该容量只限制空闲缓存，不限制并发借用。`acquire_with_cancellation` 在借用前后各检查一次取消，避免竞态下泄漏 lease；`close` 先停止 DXF worker，再关闭 DDL 池。
- `SystemSessionLease`：公开借用句柄，保存 DDL pool、trait object context 和延迟的 `metadata_error`。`query` 通过 `ddl::Session::execute` 执行；`Drop` 回滚并归还，归还失败则关闭并销毁。
- `ConcreteSystemContext`：`id` 加 `ThreadBoundSession<ConcreteSession>`。成为 owner 时设置 connection ID、restricted SQL 和 autocommit；所有实际 session 操作通过 `worker.call` 回到拥有线程。
- `ConcreteDdlContext`：DDL 的 `SessionContext` 实现，包含共享物化视图/DXF 状态、storage-class transition manager、关闭标志、稳定 ID、syssession 句柄与 DDL session variables。
- `SystemResources`/`ResourceState`：实现 `ddl::ResourcePool`。`borrowed: HashMap<u64, Weak<ConcreteDdlContext>>` 追踪尚未归还的资源，使池关闭时能逐个通知并关闭活跃 context，同时不制造强引用环。
- `job_error`/`job_kv_error`：跨 ABI 映射错误；前者从文本中的 `TxnRetryableMark` 恢复 retryable 分类，后者优先识别 write conflict，再识别一般事务重试错误。
- `query`/`query_reorg`/`bound_sql`：收集并关闭所有 record set；为 `mysql.tidb_ddl_reorg` 的二进制列生成十六进制字符串；只接受 String/i64/u64 的系统 SQL 参数，并经统一 marker 绑定器替换。
- `ConcreteJobExecutionContext`：实现 `ReorgIndexEnvironment` 和 `JobExecutionContext`，是 DDL handler 访问真实 session、Domain 服务、独立池和共享作业状态的桥梁。
- `JobSubmitServerState`：把 DDL server-state syncer 的同步缓存适配为 job submit 的升级状态查询。
- `DdlOwnerLease`：把正常 DDL owner 与 cancellation token 适配为 job worker 的 owner epoch、owner 状态和取消状态；它明确不发起新的跨 keyspace 竞选。
- `transaction_mdl`：从 DDL trait object 向下转型到 `ConcreteDdlContext`，并在线程绑定 session 上取得共享 `TransactionMDL`。
- `IndexBackfillRecords`、`generate_index_backfill_records`、`write_index_backfill_records`：将回填拆为“扫描/编码”和“检查/写入”两个阶段；`backfill_index_batch_with_ingest_options` 串联两者。
- `derive_create_mlog_schedule`：按创建物化视图日志时保存的 SQL mode、时区和错误级别求值 `START WITH`/`NEXT` 表达式，并返回下一次调度 Unix 秒数。

## 执行流程

池创建与借用流程如下：

1. `SystemSessionPool::new_with_validator` 建立 `SystemResources`。其 factory 为每个新资源分配 `NEXT_ID`，创建 `ConcreteSession`，安装可选 schema validator，并交给 `ThreadBoundSession`；线程结束清理函数是 `cleanup`，即尽力执行 `ROLLBACK`。
2. `ddl::Pool::get` 调用 `SystemResources::get`，后者从 advanced pool 取出 syssession，读取稳定 ID，建立 `ConcreteDdlContext`，写入 `borrowed` 弱引用表，最后调用 `callbacks.borrowed`。
3. `acquire_with_cancellation` 将 context 包装为 `SystemSessionLease`。调用者通过 `query` 或各 trait 实现工作；实际闭包经 `ConcreteDdlContext::call`、`Session::WithSessionContext` 和 `ThreadBoundSession::call` 回到 session 所属线程。
4. lease drop 时先执行事务回滚，再调用 DDL pool `put`。`SystemResources::put` 触发 returned 回调、移除弱引用并放回 advanced pool；底层 pool 会检查 pending transaction、reset state 和 avoid-reuse 标记。
5. `close` 是幂等的：标记资源池关闭、关闭底层 pool，并对仍借出的 context 发 destroyed 回调和 close；后续 acquire 返回 pool-closed 错误。

DDL 持久作业流程如下：

1. `DurableJobSession::begin` 创建真实事务，随后用 `with_transaction` 标记 `RequestSourceInternal` 和 `InternalTxnDDL`。
2. job worker 通过 `with_execution_context` 要求事务已激活，并临时构造 `ConcreteJobExecutionContext`。该上下文允许 handler 查询/修改系统表，但其 `query` 明确只允许一条 SELECT、集合查询或 DML；事务边界和隐式提交 DDL 仍归外层 worker 所有。
3. handler 可调用物化视图构建、refresh 信息迁移、表资源配置、modify-column/index backfill、analyze 等操作。owner epoch 改变时，`bind_owner_epoch` 清空仅对旧 owner 有效的 completed 和 cloud URI 缓存。
4. `commit` 使用 `ConcreteSession::commit_ddl_transaction`；失败或取消由调用者 rollback，lease drop 仍提供最后一道回滚保证。

索引回填流程如下：

1. `generate_index_backfill_records` 要求 active transaction，读取 catalog 中的表/索引，验证索引处于 `WriteReorganization`，并拒绝非 ingest 路径上的 partial index。
2. 它设置优先级、资源组和 DDL request source，校验扫描区间属于目标物理表，然后从 transaction snapshot 迭代 record key。每条源记录先加锁并重新读取，以处理已删除行。
3. 行值和 handle 被解码为 datum/runtime value；虚拟生成列重新求值；partial/MV/changing-type/global index 分支分别生成正确的值行、有效字段类型和 partition handle，然后编码索引键值。
4. 生成阶段关闭 iterator；写入阶段检查 unique index 的旧 handle，锁住源行，再写入事务或收集 ingest KV。ingest 路径还验证重复键的值一致性，最后调用 `modify_column_backfill::ingest_with_options`。

## 数据与状态

- `NEXT_ID: AtomicU64` 以 relaxed 顺序生成进程内稳定系统会话 ID；ID 用于 DDL internal-session 注册和回调，不承担跨进程一致性。
- `ResourceState.closed` 与 `borrowed` 在同一 `Mutex` 下维护，保证 get/put/destroy/close 对池生命周期的观察一致。弱引用只用于关闭尚存 lease，不延长其生命。
- `ConcreteDdlContext.closed: AtomicBool` 让 `transaction()` 在 context 已关闭时返回 `None`；Acquire/Release 顺序用于与 close 的跨线程可见性。
- `MViewBuildContexts` 由 `Arc<Mutex<_>>` 在同一 pool 的作业上下文间共享：`owner_epoch` 界定缓存代次，`completed` 缓存 materialized-view 构建结果，`index_cloud_uris` 缓存重组 URI，`analyzes` 保存异步分析的计时器/receiver，`dxf_worker` 保存可停止的节点服务。
- `SystemSessionLease.metadata_error` 保存无法通过 trait 签名立即返回的错误。例如 `set_snapshot_ts` 的 ABI 无返回值；失败时写入该字段并调用 `AvoidReuse`，之后 query/commit/generate IDs 会返回错误。
- `ddl::SessionVariables` 单独保存在 `ConcreteDdlContext`，事务提交/回滚时同步清除 `in_transaction`；真实 SQL mode、hint variable 和 transaction 则从线程绑定 `ConcreteSession` 读取。
- `AnalyzeProgress` 允许两种观察方式：本进程启动的 ANALYZE 使用 channel；恢复旧作业时 `result == None`，周期性查询 `mysql.analyze_jobs`。超时至少 60 秒，并可按已耗时扩大。
- `IndexBackfillRecords` 只持有有界批次的 owned key/value 和索引元数据；transaction、snapshot iterator 与 handle 不跨线程或跨阶段保存。

## 依赖与调用关系

上游调用关系：

- `pkg/session/runtime/session_factory.rs::{KeyspaceSessionFactory::prepare, prepare_normal_schema_runtime}` 创建池、注入 validator 和 MDL 注册回调。
- `pkg/session/runtime/normal_ddl_service.rs` 持有 pool，用于正常 DDL scheduler、storage-class transition 轮询与清理；`normal_ddl_submit.rs` 用同一池提交作业。
- `pkg/domain/crossks/cross_ks.rs`、`pkg/executor/operate_ddl_jobs.rs` 通过相应 trait 获取 system session；RustCodeGraph 的 file usage 还直接识别到 `pkg/session/tests/system_session.rs` 和 `pkg/session/test/variable/variable_test.rs`。
- DDL 侧 `job_worker`、`jobsubmit`、`systable`、`delete_range` 和 infoschema MDL syncer 通过本文件实现的 trait 间接进入 lease。

下游依赖关系：

- 会话与线程边界：`ConcreteSession`、`astersql_session_syssession::{AdvancedSessionPool, ThreadBoundSession}`。
- DDL ABI：`astersql_ddl_session`、`astersql_ddl_jobsubmit`、`astersql_ddl_systable`、`astersql_ddl::job_worker`、`delete_range`、`backfilling` 与 `index`。
- 存储与编码：`kv::Transaction`、snapshot/iterator、`astersql_meta::TransactionMutator`、`astersql_tablecodec` 和 `row_codec`。
- Domain 副作用：infosync、external workload/TTL、placement/affinity、storage-class transition、RU reporter，以及同目录 `create_table_resources.rs`、`modify_column_backfill.rs`、`modify_column_dist_backfill.rs`。
- 表达式与调度：parser、planner simple-expression builder、expression eval context、chrono/chrono-tz。

RustCodeGraph 对 `acquire_with_cancellation` 确认其构造 `SystemSessionLease`、调用 pool get、检查 cancellation 并在后置取消时 drop；对 `generate_index_backfill_records` 确认其构造 `BackfillTaskContext`/`IndexBackfillRecords` 并调用 transaction `StartTS`、`LockKeys` 及 iterator `Valid/Key/Close`。同名 `get/query/transaction_mdl` 在全仓库中存在歧义，因此具体上游使用模块路径和 `rg` 结果补证。

## 错误处理与边界

- `sys_error`、`ddl_error`、`job_error` 是有损 ABI 边界，主要携带字符串；只有 `job_kv_error` 能直接保留 write-conflict/retryable/storage 分类。新增错误路径若影响 jobsubmit 重试，必须经过后者或保留标准 retry marker。
- `bound_sql` 拒绝未支持的动态类型，并委托 `bind_parameter_markers` 处理占位符；字符串同时转义反斜线与单引号。DDL `execute_internal` 另支持 NULL、bool 和 bytes，bytes 编为十六进制字面量。
- `query` 必须遍历并关闭每个 result set；读取或 close 失败均向上传播。`query_reorg` 只对精确的 reorg 查询前缀启用二进制转十六进制兼容逻辑。
- 池关闭、取消、context 向下转型失败、active transaction 缺失都会显式报错。仅 cleanup、RU 上报、部分 drop-table alert 清理等“尽力而为”路径有意忽略错误，源码中均能看到局部范围。
- `ConcreteJobExecutionContext::query` 拒绝多语句和事务控制/DDL，防止 handler 绕过外层 job worker 的提交、回滚和重试协议。
- 回填严格检查表/索引存在、schema state、物理表 key range、列 offset、唯一键冲突和 missing handle；iterator 在生成闭包结束后无条件 `Close`，生成错误再向上传播。
- materialized-view 构建会保存并恢复 current database、SQL mode、timezone 和 hint vars；出错时尝试 rollback。系统表缺失、陈旧 refresh TSO、NULL/错误类型调度表达式都有专门错误分支。
- 多处 `Mutex::lock().unwrap()` 表示 poisoning 被视为不可恢复的内部错误；用户输入/存储错误通常转换为 `Result`。扩展共享状态时应沿用既有策略，避免把可预期外部失败变成 panic。

## 并发与资源生命周期

`ConcreteSession` 不是任意线程可直接共享的对象。本文件用 `ThreadBoundSession` 将其固定到 worker 线程，所有跨线程调用以 `Send + 'static` 闭包排队执行；这也是 `ConcreteDdlContext::call` 的核心不变量。

底层 advanced pool 的容量 5 是空闲资源缓存上限：空池时会创建新 session，因此 6 个并发 borrower 不会等待。归还时底层负责 owner 转移、pending transaction 检查、reset 和 avoid-reuse；本层又在 lease drop 前 rollback，形成双重清理。池 close 可与活跃 lease 共存：`SystemResources::close` 关闭底池并主动关闭弱引用仍可升级的 context，后续 lease drop 仍执行 returned/put 路径而不会复活池。

回调顺序与 Go 的 Get/Put/Destroy 对齐：成功借出后 `borrowed`，正常归还时 `returned`，显式销毁或 pool close 活跃资源时 `destroyed`。`session_factory.rs` 利用该顺序安装/移除 MDL internal-session；改变顺序可能让 schema barrier 漏看或残留会话。

`MViewBuildContexts` 的 owner-epoch 清理防止新 owner 使用旧 owner 的完成缓存；DXF worker 由 pool 启动并由 `stop_dxf_worker`/`close` 终止。ANALYZE 使用独立 OS 线程和一次性 channel，poll 路径不阻塞 job worker。索引回填仅在拥有 transaction 的 session 线程上持有 iterator/transaction，跨阶段传递的是 owned bytes。

## 与 Go 版本的对应关系

- `pkg/session/syssession/pool.go` 的 `AdvancedSessionPool` 是底层池语义来源：空池即新建、容量只限制 idle cache、Put 时检查 avoid-reuse/pending transaction/reset、Close 后拒绝 Get。Rust `SystemSessionPool::new_with_validator` 固定容量 5，并复用已移植的 syssession crate 实现这些规则。
- `pkg/ddl/session/session_pool.go` 的 `Pool.Get/Put/Destroy/Close` 是 DDL 包装语义来源：Get 设置 autocommit/restricted SQL 并注册 internal session；Put 要求无有效事务、rollback 后归还并注销；Destroy 丢弃不可复用资源；Close 幂等。Rust 分别由 `ConcreteSystemContext::on_became_owner`、`SystemResources`、lease `Drop` 和 callbacks 落实。
- Go 使用 session context 和 `infosync.StoreInternalSession/DeleteInternalSession`；Rust 不直接写全局 infosync，而是把 stable ID 与 `transaction_mdl` 交给 `session_factory.rs` 的 coordinator callbacks。这是实现形态差异，但借用期间可见、归还/销毁后删除的生命周期一致。
- `jobsubmit::Session` 的 pessimistic transaction、BDR role、global ID key lock/allocate 与 Go DDL submit 路径对齐；`meta_key` 使用 `m` prefix、名字和 `s` field 编码构造 metadata key。`read_bdr_role_and_start_ts` 若已有事务则保持 affinity，否则使用可重试的新 metadata transaction。
- `ConcreteJobExecutionContext` 后半部分不是 Go 单一文件的逐行翻译，而是将 Go DDL worker 分散在 add-index、materialized view、table resource、analyze 和 delete-range 路径中的 session/domain side effects 汇聚到 Rust trait。源码注释明确标出的 Go 对照包括 `addIndexTxnWorker.BackfillData/fetchRowColVals` 和 `deriveMaterializedScheduleNextUnixSecondsForDDL`。
- 已确认的 Rust 对齐测试是 `pkg/session/tests/system_session.rs`：覆盖归还回滚、metadata/global ID 事务、容量/取消/关闭、jobsubmit 与 systable 共池、callbacks/不可复用错误和 MDL SQL barrier。Go 基准测试包括 `pkg/session/syssession/pool_test.go` 与 `pkg/ddl/session/session_pool_test.go`。

## 扩展指南

- 新增系统会话能力时，先判断它属于底层 `sys::SessionContext`、DDL `SessionContext`，还是某个专用 trait；不要绕过 `ConcreteDdlContext::call` 直接跨线程访问 `ConcreteSession`。
- 为 `SystemSessionLease` 新增无错误返回值的 ABI 操作时，若内部仍可能失败，应仿照 `set_snapshot_ts`：保存 deferred error、标记 `AvoidReuse`，并保证后续可返回错误的边界会检查它。
- 新增 SQL 参数类型必须同时审查 `bound_sql` 与 DDL `execute_internal` 的字面量编码，尤其是 bytes、NULL、bool、反斜线和单引号；优先复用统一 marker binder，不拼接未转义输入。
- 新增 DDL handler 查询必须保持 `ConcreteJobExecutionContext::query` 的“一条事务型查询/DML”约束。若操作需要独立提交，显式借用独立 lease，并在失败路径 rollback，不能在 handler 内偷偷执行隐式提交 DDL。
- 扩展回填时应保持扫描/编码与写入分离、snapshot iterator 及时关闭、源行锁、unique handle 校验、global partition handle、changing type、generated column、partial/MV index 等既有分支；不要以仅支持普通二级索引的简化实现替代。
- 新增 pool 共享缓存必须明确 owner epoch 是否使其失效、由谁关闭、是否允许跨 session 复用，并放入 `MViewBuildContexts` 或独立受控资源，而不是静态可变状态。
- 测试应放在独立文件。池/事务/回调行为优先扩展 `pkg/session/tests/system_session.rs`；普通 DDL 作业扩展同目录 `normal_ddl_*_test.rs`、`lifecycle_test.rs` 或 `durable_scheduler_test.rs`；回填还应同步 `pkg/ddl/tests/partition/reorg_partition_test.rs` 等最接近行为的测试。不要把测试内嵌进本生产文件；现有末尾 `#[cfg(test)]` 仅通过 `#[path]` 引用独立测试文件。
- 兼容风险主要是 Go 池生命周期、job retry 分类和 DDL transaction ownership；正确性风险集中在脏事务复用、MDL 注册泄漏、唯一索引冲突与物化视图状态恢复；性能风险集中在 pool lock 范围、每行编码/表达式求值、批次内存和不必要的新 session/线程。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件在索引中，报告 2 个直接 file usage：`pkg/session/test/variable/variable_test.rs`、`pkg/session/tests/system_session.rs`。
- RustCodeGraph 源码与符号查询：完整分段读取 `pkg/session/runtime/system_session.rs`；查询 `SystemSessionPool`、`SystemSessionLease`、`transaction_mdl`、`generate_index_backfill_records`、`derive_create_mlog_schedule`；对 `acquire_with_cancellation`、`generate_index_backfill_records` 执行 callers/callees。图中同名通用方法存在歧义，故没有把明显来自其他模块的候选当成本文件调用边。
- Rust 源与装配文件：`pkg/session/runtime/system_session.rs`、`pkg/session/runtime.rs`、`pkg/session/lib.rs`、`pkg/session/runtime/session_factory.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/session/runtime/normal_ddl_submit.rs`。
- crate 边界：`pkg/session/Cargo.toml` 的 package、feature、porting metadata、直接依赖与 dev-dependencies。
- Rust 测试：`pkg/session/tests/system_session.rs`；另通过引用搜索确认 `runtime/lifecycle_test.rs`、`runtime/durable_scheduler_test.rs`、`runtime/normal_ddl_test.rs`、物化视图专项测试、`pkg/ddl/tests/partition/reorg_partition_test.rs` 等覆盖实际消费者和后半部作业逻辑。
- Go 对照：`pkg/session/syssession/pool.go`、`pkg/session/syssession/pool_test.go`、`pkg/ddl/session/session_pool.go`、`pkg/ddl/session/session_pool_test.go`，以及 `pkg/domain/domain.go` 中 system session pool 的创建/关闭/访问位置。
- 本任务是纯文档分析，按计划不运行 Cargo。结构校验要求本文恰好具有任务规定的 11 个二级标题；同时人工复核了文件存在原因、借用/执行/归还流程、安全扩展点和独立测试位置。

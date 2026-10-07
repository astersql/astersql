# `pkg/ddl/persistent_actions.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml`，由 `pkg/ddl/lib.rs:289` 公开为 `persistent_actions` 模块），是 Rust 普通 DDL worker 在持久事务中执行具体 action 的分派层。直接主链是 `table_mode.rs::NormalDdlExecutor::step` 在复制并补齐持久化 `Job` 后调用 `persistent_actions::step`；更外层的 `job_worker.rs::JobWorker::transit_persisted_job_step` 负责 owner 租约检查、`mysql.tidb_ddl_job` 原始字节复核、事务提交/回滚与作业重新编码。

因此它不是 SQL AST 入口、任务提交器或调度器，也不独立完成所有 DDL。它只接收已经持久化并被 owner worker 选中的 `astersql_meta_model::group_3::Job`，在 `JobExecutionContext` 提供的 SQL/KV 能力内执行一步。复杂动作会转发给同目录的 `persistent_*` 模块；简单元数据动作则在本文件内完成。

按 `docs/agents/ddl/README.md` 的问题框架，本文件处理的是 **job-based** 路径：元数据型动作通常一次完成并保持/结束于 `Public`，无需 reorg；ADD INDEX/主键、修改列、物化视图等动作可能跨状态或 reorg，但本文件只负责分派或初始化阶段。持久点包括 KV 元数据、schema version/diff、DDL job 行，以及特定动作的通知或 storage-class transition 历史行。

## 核心职责

1. `handler_available(action)` 声明普通持久 worker 当前允许接管的 action 协议号，阻止未实现 action 被当作成功处理。
2. `step(context, job)` 按 `job.tp` 路由：批量建表、TTL、修改/删除列、截断/重命名/删除表、索引初始化、物化视图、建表和 engine attribute 等进入专用路径；剩余轻量动作进入 `step_metadata`。
3. `step_metadata` 在单个 `with_transaction` 回调中处理建 schema、删外键、表注释/AutoID cache、schema charset/placement、table mode 和 refresh-meta。
4. `public_table`、`update_version_and_table[_with_check]`、`async_notify_event` 和 `initialize_reorg_indexes` 为其他持久 action 模块提供共享的表校验、版本/元数据写入、事件发布和 reorg 初始化能力。
5. `modify_engine_attribute` 除更新表元数据外，还在 NextGen 内核上对 storage class 变化进行历史记录的 supersede/restart；`alter_ttl` 在元数据提交后同步外部 TTL 注册。

这些职责的共同约束是保留 Go DDL 的持久化 wire action 编号和作业状态语义，而不是提供一个通用、可回退到其他 backend 的执行框架（见 `pkg/meta/model/job.rs` 的 action 常量和本文件 `initialize_reorg_indexes` 的显式 action 白名单）。

## 主要符号

- `pub fn handler_available(action: u8) -> bool`：普通 worker 的能力门。接受协议号 1、3、4、6、7、10、11、12、14、17、26、32、39、47、55、60、65、67、74、75、76、85--94 中本文件列出的有效项；89--91 使用命名常量。`NormalDdlExecutor::step` 对非终态且不在此集合的作业直接报 unavailable。
- `pub fn step(&mut dyn JobExecutionContext, &mut Job) -> Result<i64, String>`：文件主入口，返回本步产生的 schema version；复杂动作转发到对应 `persistent_*::step`，元数据动作通过 `step_metadata` 执行。
- `fn step_metadata(&mut dyn Transaction, &mut Job)`：仅接受 1/10/17/26/39/55/75/76；其他 action 返回 `normal DDL persistent handler unavailable`，不存在默认成功分支。
- `create_schema`：兼容 V1 数组参数和 V2 `db_info` 参数；以 `job.schema_id` 覆盖 DB ID，检查 ID/名称冲突，生成 schema version/diff，写入 `Public` DBInfo 并完成 DB job。
- `modify_schema_charset`、`modify_schema_placement`：读取现存 DB，分别更新 charset/collation 或 placement policy。charset/collation 未变化时版本为 0；相同非空 policy 引用也直接完成。placement 会复核 policy ID 存在。
- `pub(crate) fn public_table`：验证 schema 和 table 存在、可选 `job.table_name` 匹配（repair table 例外）、表状态为 `Public`；失败会把 job 置为 `Cancelled`。
- `modify_table_metadata`：动作 17 修改 Comment，39 修改 AutoIDCache；multi-schema 仍可回滚时先 `mark_non_revertible` 并返回 0。
- `pub fn update_version_and_table_with_check` 与 `pub(crate) fn update_version_and_table`：前者先调用 `create_table::check_table_info_valid`；后者在非 `skip_version` 时生成版本和 diff，然后更新表。它只覆盖当前 Rust 单表元数据路径，不等价于 Go helper 的所有多表参数和 UpdateTS 行为。
- `drop_foreign_key`、`refresh_meta`：前者按不区分大小写名称删除 FK 并区分 `Done`/`RollbackDone`；后者只校验参数、生成 diff，并把作业置为 `Done/Public`，不重写表。
- `pub fn async_notify_event`：跳过内存/系统 schema；将 `sub_job_id == -1` 映射到 multi-schema `seq`；通过 notifier 生成插入 SQL，仅绑定整数和 JSON bytes，最终借 `context.query` 写入。
- `pub fn initialize_reorg_indexes` 与 `initialize_prepared_index_action`：只允许 ADD INDEX、ADD PRIMARY KEY、MODIFY COLUMN 初始化。prepared 路径只接受 `SchemaState::None` 且 `ReorgTypeNone`，从现有表中取 canonical index，初始化并回写；明确不负责后续 backfill、rollback 或 publish。
- `modify_engine_attribute`、`stage_storage_class_transitions`、`sql_string`：解析并保留 engine attribute 原始 JSON，重建表/分区 storage class；NextGen 下将受影响的 RUNNING 历史置为 SUPERSEDED，再为当前物理目标插入新 RUNNING 记录。`sql_string` 仅用于该内部 SQL 构造的字符串转义。
- `alter_ttl`：动作 65 应用 `GetAlterTTLInfoArgs`，67 清除 TTLInfo；写入 schema diff 后，根据最终 Enable 状态注册或删除外部 TTL workload，成功后才完成 table job。

文件没有自定义 struct、trait、模块级可变状态或条件编译项；主要状态都在传入的 `Job`、表/库元数据及 context 背后的持久资源中。

## 执行流程

1. `JobWorker::transit_persisted_job_step` 先验证 worker 未关闭、当前仍是 owner 且调度未取消，绑定 owner epoch，恢复前任 owner 未同步状态，然后开启 SQL 事务并复核 `mysql.tidb_ddl_job.job_meta` 与调度时读取的字节一致。
2. `NormalDdlExecutor::step` 先用 `handler_available` 做能力检查；处理 Paused/Pausing/Cancelling/终态；随后复制 wire `Job`，补回未编码的 multi-schema、reorg 和 args 字段，并对 KV transaction 创建 statement stage。
3. `persistent_actions::step` 依据 action 分派。需要自有多步状态机或额外 I/O 的动作调用专用模块；轻量动作在 `context.with_transaction` 中执行 `step_metadata`。
4. action 成功时外层释放 statement stage，并把返回版本写入 `last_schema_version`；失败时清理 stage、累加 `error_count`，达到策略上限且可回滚时转为 `Cancelling`。因此本文件返回 `Err` 不等于外层 SQL 事务必然直接报废，action 级错误由 normal executor 持久化到 `Job`。
5. `transit_persisted_job_step` 再检查租约，计算适用的 RU，重新编码 job 行，提交前再次检查租约；任一步失败则 rollback 并恢复事务前 RU。产生正 schema version 的步骤随后由 executor 的 barrier/MDL 流程完成集群 schema 同步（该部分在 `table_mode.rs`，不在本文件）。

典型轻量路径是：解码参数 → 读取并校验 DB/Table → 必要时把 multi-schema 标为不可回滚 → 修改内存元数据 → `gen_schema_version`/`set_table_schema_diff` → 写 DB/Table → `finish_*_job`。ADD INDEX 的当前路径例外：它只初始化 `ReorgMeta` 与 canonical index，再返回 0；下一阶段会明确报未实现，而不会重复初始化或虚假发布索引。

## 数据与状态

- `Job.tp` 是持久协议的 `u8` action number，不能重排；数值来源于 `pkg/meta/model/job.rs`。本文件同时依赖 `state`、`schema_state`、`raw_args`、`version`、`schema_id`、`table_id`、名称、`multi_schema_info`、`reorg_meta`、`real_start_ts/start_ts` 等字段。
- 作业状态主要为 `Running`、`Cancelled`、`Done`、`Rollingback`、`RollbackDone`；schema 状态主要为 `None` 与 `Public`。`drop_foreign_key` 在 rollback 时完成为 `RollbackDone/None`，其他本地元数据动作多数完成为 `Done/Public`。
- schema version 与 `SchemaDiff` 必须和元数据写入同处 worker transaction。`update_version_and_table` 在 multi-schema `skip_version` 时返回 0；调用者不能把 0 误认为没有元数据变化。
- `public_table` 的读取对象是 `TransactionMutator` 视图，且要求表处于 Public；这让调用方不能把尚在其他 DDL 状态的表当成普通元数据动作处理。
- storage-class 逻辑以 `BTreeMap<physical_id, PhysicalStorageClass>` 快照计算变化集合；历史表记录 direction、targets、schema_version、start_ts 和进度。过期 RUNNING 记录先被标为 SUPERSEDED，仍存在的 targets 可加入新一轮操作。
- 文件自身不缓存长期状态。`async_notify_event`、TTL 注册和 storage-class observation 的持久/缓存状态由 notifier、外部 workload 和 `JobExecutionContext` 实现持有。

## 依赖与调用关系

上游主链为：`job_worker.rs::JobWorker::transit_persisted_job_step` → `table_mode.rs::NormalDdlExecutor::step` → `handler_available`/`step`。模块由 `pkg/ddl/lib.rs` 装配；`pkg/ddl/Cargo.toml` 将其置于 `astersql-ddl` crate，并声明这里直接使用的 `astersql-meta-model`、`astersql-meta`、`astersql-kv`、`astersql-ddl-notifier`、`astersql-meta-metadef`、`astersql-config-kerneltype`、`serde/serde_json` 和 `chrono` 等依赖。

`step` 的主要下游包括 `persistent_create_table::{batch_step,step}`、`persistent_modify_column::step`、`persistent_drop_column::step`、`persistent_masking_actions::{truncate_table,rename_tables,drop_table}`、三个物化视图创建模块、out-of-place cutover 及三个 ALTER MATERIALIZED VIEW 模块。`table_mode::on_persistent_alter_table_mode` 是 metadata 分派的一支。

反向共享调用者包括：建表、修改列、物化视图和 masking 模块调用 `async_notify_event`；物化视图 ALTER、table mode、建表等调用 `public_table` 或 `update_version_and_table`；`job_worker.rs::IndexReorgInitialization::step` 和 `persistent_modify_column.rs` 调用 `initialize_reorg_indexes`。RustCodeGraph 对目标文件报告 39 个符号，并确认文件被 `persistent_masking_actions.rs` 及相关测试引用；图谱对常见名 `step` 存在歧义，具体跨文件调用以精确符号查询和上述 `rg` 引用结果共同核验。

## 错误处理与边界

- 参数解码错误通常将 `job.state` 置为 `Cancelled`；schema/table/policy/FK 不存在、表名不匹配或非 Public 也取消作业，并尽量保留 Go 风格错误类/错误码文本。
- 未支持 action、ADD INDEX rollback/后续阶段、缺失 prepared index、错误 reorg 类型会明确返回错误；没有“空操作即成功”的兜底。特别是当前 ADD INDEX/ADD PRIMARY KEY 仅有初始化阶段，不能宣称完整在线建索引已由本文件实现。
- `create_schema` 的冲突检查来自当前 metadata 列表，而 Go `onCreateSchema` 使用最新 InfoSchema cache；文档只能确认目标行为对齐，不能宣称两者并发可见性实现完全相同。
- Rust `update_version_and_table` 是单表实现，未复刻 Go helper 的 `multiInfos`、`needUpdateTs` 和 failpoint；新增调用前必须确认目标 Go handler 是否需要 checked/unchecked helper 以及额外表更新。
- `async_notify_event` 只接受 notifier SQL 参数中的 integer 和 bytes；其他 `SqlValue` 会失败。它直接拼装经约束的 SQL，修改 notifier 参数类型时必须同步这里，避免转义或占位替换错误。
- storage-class 历史行少于四列、targets JSON 非法/重复、数值解析失败、方向无效或 SQL 查询失败都会中止本步。元数据已由 statement stage 管理，外层失败路径负责 Cleanup/Rollback。
- TTL 外部 workload 同步发生在表元数据 transaction 回调之后但仍处于外层 staged action；失败会取消 job。该顺序是兼容性关键点。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或 channel。并发安全来自外层 owner worker：执行前后检查 `JobLease`，用 owner epoch 防止旧任期恢复；在同一事务中重读 job bytes，避免 ADMIN pause/cancel 或重叠 owner 覆盖更新；statement stage 在 action 成功时 Release、失败时 Cleanup，最终 transaction 再 commit/rollback。

schema version、diff、DB/Table 元数据和重新编码的 job 行属于一次持久 step 的原子边界。`table_mode_test.rs::crossks_align_normal_ddl_real_meta_go_diff_and_stage_cleanup` 证明 Cleanup 后不会留下 phantom version/diff；`job_worker_test.rs` 证明 commit 错误会 rollback 并恢复 RU，非 owner/cancelled scheduler 在开启事务前即被拒绝。

外部资源有两类额外生命周期：TTL 注册由最终 TTL Enable 状态决定 create/delete；storage-class transition 先终结与变化物理 ID 相交的旧 RUNNING 操作，再按当前拓扑创建新 RUNNING 操作。后者使用 context 的缓存 observation 补齐 superseded 记录进度，但缓存所有权不在本文件。

## 与 Go 版本的对应关系

- `create_schema`、`modify_schema_charset`、`modify_schema_placement` 分别对照 `pkg/ddl/schema.go` 的 `onCreateSchema`、`onModifySchemaCharsetAndCollate`、`onModifySchemaDefaultPlacement`。
- `modify_table_metadata`、`refresh_meta` 与版本更新 helper 对照 `pkg/ddl/table.go` 的 `onModifyTableComment`、`onModifyTableAutoIDCache`、`onRefreshMeta`、`updateVersionAndTableInfo[WithCheck]`。
- `drop_foreign_key` 对照 `pkg/ddl/foreign_key.go::onDropForeignKey/dropForeignKey`，包括找不到 FK 时取消和 rollback 完成状态。
- `alter_ttl` 对照 `pkg/ddl/ttl.go::onTTLInfoChange/onTTLInfoRemove`；Rust 把 model TTLInfo 经 JSON 转为本地结构，并用 `apply_model_ttl_change` 保留未显式指定字段，再同步外部 workload。
- `modify_engine_attribute` 对照 `pkg/ddl/engine_attribute.go::onModifyTableEngineAttribute` 及 storage-class transition staging；Rust 当前直接借 context SQL 管理 history 表。
- `async_notify_event` 对照 `pkg/ddl/ddl.go::asyncNotifyEvent`：都跳过系统 schema，并将 multi-schema 的隐式 sub-job ID 替换为 Seq；Rust 路径没有 Go 单测环境的 channel/backoff 分支，而是直接发布事务内 notifier 记录。

总体语义目标是 Go worker 的“一个持久 job step”，但 Rust 文件是按 action 拆分后的路由与共享 helper，不是同路径 Go 单文件翻译。已知差异必须按上节边界看待，尤其是 InfoSchema 检查、通用 update helper、测试 failpoint 和尚未实现的索引后续阶段。

## 扩展指南

新增普通持久 action 时，应先在 `handler_available` 增加对应 **命名常量**，再在 `step` 或 `step_metadata` 添加唯一分派；多步/reorg/外部 I/O 动作应放入独立 `persistent_*.rs`，不要把完整状态机塞入 metadata match。必须同步核对 `pkg/meta/model/job.rs` 的固定协议号、Go handler、取消/回滚语义、schema state、是否生成 diff、是否需通知和外部资源清理。

复用表更新 helper 前要判断 Go 使用 checked 还是 unchecked 路径、是否更新多个表/UpdateTS、以及 multi-schema `skip_version`。增加事件类型时同步 notifier 参数约束和独立测试；扩展 storage-class SQL 时保持 targets 校验、旧操作 supersede 与当前拓扑重建顺序。

测试必须保持在独立 Rust 测试文件中。最接近的接入点是：普通事务/statement stage 与元数据版本用 `pkg/ddl/table_mode_test.rs`；owner/提交回滚用 `pkg/ddl/job_worker_test.rs`；TTL 用 `pkg/ddl/ttl_test.rs` 和 `pkg/ddl/table_test.rs`；FK 用 `pkg/ddl/foreign_key_test.rs`；storage-class 用 `pkg/ddl/storage_class_transition_test.rs`；物化视图 dispatch 用 `pkg/ddl/persistent_mview_out_of_place_cutover_test.rs`。新增 bug fix 应添加先失败后通过的回归测试，并保留 Rust/Go 行为对照。

兼容性风险最高的是 action 数值、job raw args V1/V2 解码、错误码、schema diff 和 terminal state；正确性风险集中于 statement stage 外做副作用、误用返回版本 0、漏发事件或遗漏回滚清理；性能风险主要是 `list_databases` 冲突检查、storage-class history 查询/逐行更新，以及对大 metadata/targets 的 JSON 编解码。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点和 1,848,419 条边；`files --filter pkg/ddl/persistent_actions.rs` 报告目标文件 39 个符号；`node --file ... --offset 1/501` 完整读取 875 行；`query/node` 核验 `handler_available`、`step`、`async_notify_event`、`initialize_reorg_indexes` 和 `job_worker.rs::transit_persisted_job_step`，后者的调用轨迹确认事务、租约、job 行更新与 rollback 边界。
- Rust 源与装配：`pkg/ddl/persistent_actions.rs`、`pkg/ddl/table_mode.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/lib.rs`、`pkg/meta/model/job.rs`；crate/依赖边界由 `pkg/ddl/Cargo.toml` 核验。
- Go 对照：`pkg/ddl/schema.go`、`pkg/ddl/table.go`、`pkg/ddl/foreign_key.go`、`pkg/ddl/ttl.go`、`pkg/ddl/engine_attribute.go`、`pkg/ddl/ddl.go`。
- 独立 Rust 测试：`pkg/ddl/table_mode_test.rs`（真实 metadata/diff 与 statement cleanup）、`pkg/ddl/job_worker_test.rs`（owner、commit/rollback、RU）、`pkg/ddl/ttl_test.rs`、`pkg/ddl/table_test.rs`、`pkg/ddl/foreign_key_test.rs`、`pkg/ddl/storage_class_transition_test.rs`、`pkg/ddl/persistent_mview_out_of_place_cutover_test.rs`。这些测试是相关行为证据；本任务按计划为纯文档分析，未运行 Cargo，也未声称目标文件每个分支均有直接测试。
- DDL 背景入口：`docs/agents/ddl/README.md` 仅用于定位 job/owner/schema-sync 问题，具体结论均回到上述代码与测试复核。

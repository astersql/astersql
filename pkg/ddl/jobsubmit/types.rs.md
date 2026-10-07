# `pkg/ddl/jobsubmit/types.rs`

## 文件定位

本文件属于 `astersql-ddl-jobsubmit` crate，是 DDL job 直接提交路径的公共类型层。crate 入口 `pkg/ddl/jobsubmit/lib.rs` 将 `types`、`submit`、`table_mode` 三个模块公开再导出；其中本文件定义提交实现所操作的 job 数据模型、版本化持久化编码、错误分类，以及会话、系统表、BDR、升级状态和 owner 通知等依赖抽象。

在完整 DDL 链路中，本文件不执行 schema 变更，也不调度 worker。它承接已经构造好的 DDL job，供 `pkg/ddl/jobsubmit/submit.rs::submit_batch` 校验并补齐提交元数据，最终由 `insert_ddl_jobs_to_table` 编码后写入 `mysql.tidb_ddl_job`。因此它位于“DDL 语句构造 job”和“owner 调度持久化 job”之间的提交边界。

crate 边界由 `pkg/ddl/jobsubmit/Cargo.toml` 明确：当前实际依赖只有 `astersql-meta-model` 与 `serde_json`；一批 DDL 子系统依赖被放在 `cfg(any())` 下，不参与当前编译。本文件的持久化 wire 类型实际来自 `astersql_meta_model::group_3`。

## 核心职责

1. 用 `JobType`、`JobState`、`JobArgs`、`Job`、`SubJob` 和精简元信息类型表达提交侧所需的 DDL 数据。
2. 用 `Job::normalize_involving_schema_info` 与 `Job::check_involving_schema_info` 实施涉及对象名称的规范化和合法性约束。
3. 用 `Job::may_need_reorg`、`Job::started` 为系统表的 `reorg`、`processing` 列提供判定。
4. 用 `Job::encode` 将提交侧模型转换成 `astersql-meta-model` 的 `model::Job`，并生成与 Go `model.Job.Encode(true)` 对齐的 JSON wire 数据；`encode_job_args_v1`/`v2` 负责版本差异。
5. 用 `JobSpec` 将 job、类型化参数和“对象 ID 是否已分配”标志组合成一次提交单元。
6. 用 trait 和回调定义 `submit.rs` 的依赖注入边界，使事务、全局 ID、系统表检查、BDR 策略、升级状态、owner 通知及失败清理可替换、可测试。

本文件不负责 ID 数量计算、ID 分配、事务重试、SQL 拼装或通知调用；这些行为分别位于 `pkg/ddl/jobsubmit/submit.rs` 的 `required_global_id_count`、`assign_global_ids_for_jobs`、`generate_ids_and_insert_jobs_with_retry`、`insert_ddl_jobs_to_table` 和 `notify_ddl_owner`。

## 主要符号

- `ErrorKind`：提交错误分类。`Retryable` 控制外层事务重试；`WriteConflict` 控制全局 ID 悲观锁内部重试；`Invalid` 表示参数或状态错误；`Storage` 为存储错误预留类别。
- `Error`：携带 `kind` 和可展示的 `message`，实现 `Display` 与 `std::error::Error`。`invalid`、`retryable` 是当前公开构造器。
- `JobType` 与 `JobType::code`：Rust 枚举到持久化 action type 数值的映射；`Other(i64)` 保留尚未显式建模的类型码。
- `JobState`：提交侧使用的状态子集：`None`、`Queueing`、`Pausing`、`Paused`。`persistent_job_state` 将其转换为元模型状态。
- `PartitionDefinition`、`PartitionInfo`、`TableInfo`、`DatabaseInfo`、`ResourceGroupInfo`、`RenameTableInfo`：只保留提交和 ID 分配需要的字段，不是完整 infoschema 模型。
- `JobArgs`：按 DDL 类型区分参数载荷，包括建表、批量建表、建库、资源组、分区、截断表、重命名、交换分区和不透明字节。`Opaque` 允许未经本地解析的原始参数直通。
- `SubJob`：`MultiSchemaChange` 子任务，保存子类型、参数、状态、reorg 标志及提交前填充的 `encoded_args`。
- `Job`：提交侧 job 主体。除 ID、名称、类型、SQL、状态外，还保存 `start_ts`、BDR 角色、CDC 来源、SQL mode、reorg 元数据、session vars、trace/binlog 存在性、多 schema 子任务和升级期系统操作标志。
- `Job::normalize_involving_schema_info`：将普通 schema/table 名称转为小写，保留空串与 `*`；不排序、不去重。
- `Job::check_involving_schema_info`：校验涉及对象。显式列表为空时回退到 job 自身名称；只有 schema 名而无 table 名时使用 `*`；拒绝双空、单边空以及 `database == "*"` 但 table 不是 `*` 的组合。
- `Job::may_need_reorg`：为索引、主键、重组/移除/修改分区直接返回 true；`ModifyColumn` 还要求 `need_reorg`；`MultiSchemaChange` 递归检查子任务。
- `Job::started`：只有 `None`、`Queueing` 被视为尚未开始，`Pausing` 和 `Paused` 已进入处理阶段。
- `Job::encode`：组装 `model::Job` 并编码。版本值为 2 时选择 `JobVersion::V2`，其他值按 V1；多 schema 子任务、涉及对象、管理员来源、trace、BDR、CDC、reorg/session 元数据一并映射。
- `encode_job_args_v1`、`encode_job_args_v2`：V1 使用按 action 定制的位置数组，V2 使用具名 JSON 对象；`Opaque` 在分派前直接返回原始字节。
- `JobSpec`：提交单元；`id_allocated` 为 true 时，提交实现只再分配 job ID，不重新分配表、分区等对象 ID。
- `Session`、`SessionPool`：抽象事务生命周期、悲观锁、时间戳、全局 ID、SQL 执行及会话借还。
- `SystemTableManager`、`MinJobIdProvider`、`ServerState`、`BdrPolicy`、`OwnerNotifier`：分别抽象 flashback job 检查、最小 job ID、升级状态、BDR 拒绝规则和 owner 唤醒。
- `Cleanup`、`BeforeInsert`：ID 分配后、插入前的可选钩子及失败时只执行一次的清理回调。
- `SubmitOptions`：汇总提交依赖、重试上限和退避策略；`server_state` 与 `before_insert_with_assigned_ids` 是可选项。

## 执行流程

1. 上游构造 `JobSpec { job, args, id_allocated }`，并将一批 spec 交给 `pkg/ddl/jobsubmit/submit.rs::submit_batch`。
2. `submit_batch` 通过 `SubmitOptions.session_pool` 借出会话，使用 `min_job_id_provider` 和 `system_table_manager` 排除 flashback cluster job，并读取 BDR 角色与事务 `start_ts`。
3. 对每个 `Job`，提交逻辑调用 `normalize_involving_schema_info`、`check_involving_schema_info`，检查非零版本，填充 trace、`start_ts`、BDR 角色，并按 `BdrPolicy`、`ServerState` 决定拒绝或将状态置为 `Queueing`/`Pausing`。
4. `generate_ids_and_insert_jobs_with_retry` 通过 `Session` 开启悲观事务、锁全局 ID 键、生成并写回 ID，然后调用可选的 `BeforeInsert`。
5. `insert_ddl_jobs_to_table` 对每个 spec 调用 `Job::encode(&spec.args)`；同时调用 `may_need_reorg` 与 `started`，形成 `mysql.tidb_ddl_job` 的 `job_meta`、`reorg` 和 `processing` 列。
6. `Job::encode` 先把提交侧字段转换为 `model::Job`。普通参数按 job 版本进入 V1 或 V2 编码；`Opaque` 原样返回；多 schema 子任务逐个转换类型、状态、reorg 标志和参数。
7. 系统表插入与事务提交成功后，输入 `JobSpec` 中分配的 ID、时间戳、角色和状态保持可见；失败时提交实现利用 `Cleanup`、`rollback`、错误分类和 `backoff` 决定清理与重试。

## 数据与状态

`Job` 是可变的提交中间态，而不是只读 DTO。`submit_batch` 会修改名称、trace 标志、`start_ts`、`bdr_role`、状态和管理员来源；ID 分配逻辑还会修改 `Job.id`、`schema_id`、`table_id` 以及 `JobArgs` 中的表/分区 ID。调用方不能把传入的 `JobSpec` 当作不可变输入。

关键状态不变量如下：

- `Job.version` 在提交时不得为 0；`encode` 仅把恰好等于 2 的值作为 V2，其余值作为 V1，因此新增版本不能只修改调用方。
- `JobType::code` 是系统表兼容边界；改变现有数值会破坏已持久化 job 的解释。
- `involving_schemas` 为空时，校验使用 `schema_name`/`table_name` 回退，但 `encode` 只序列化显式列表；提交前规范化保证名称比较使用小写，空串和 `*` 保持语义。
- `reorg_meta` 用 `Arc<model::DDLReorgMeta>` 共享不可变快照，编码时经 serde value 转换复制进持久化模型。
- `session_vars`、`query`、BDR、CDC、SQL mode、trace/binlog 存在性均进入持久化 job，属于恢复和兼容所需状态。
- `SubJob.encoded_args` 由 `submit.rs::fill_args_with_sub_jobs` 填充，但当前 `Job::encode` 重新从 `SubJob.args` 生成 `model::SubJob.raw_args`；扩展时必须核对两条表示是否仍需并存。

## 依赖与调用关系

上游直接证据来自 RustCodeGraph：`submit_batch` 的调用者包括 `pkg/session/runtime/normal_ddl_submit.rs::submit_and_wait`、`pkg/session/runtime/crossks_job_submit.rs::submit_table_mode` 和 `pkg/domain/crossks/ddl_submit.rs::submit`，说明该类型层服务普通 DDL 与跨 keyspace/table-mode 提交入口。

本文件向下只直接依赖标准库、`serde_json` 和 `astersql-meta-model`：

- `std::sync::Arc` 用于共享 reorg 元数据、依赖对象和回调。
- `serde_json` 构造 V1/V2 参数 JSON，并桥接 reorg 元数据。
- `astersql_meta_model::group_3` 提供最终持久化的 `Job`、`SubJob`、状态、版本、trace 和管理员类型，并由其 `Job::encode(false)` 生成 wire 字节。

与相邻实现的已验证调用边为：

- `submit.rs::submit_batch` → `Job::normalize_involving_schema_info`、`Job::check_involving_schema_info`。
- `submit.rs::generate_ids_and_insert_jobs_with_retry` → `Session`、`SubmitOptions.before_insert_with_assigned_ids`、`SubmitOptions.backoff`。
- `submit.rs::insert_ddl_jobs_to_table` → `Job::encode`、`Job::may_need_reorg`、`Job::started`。
- `submit.rs::notify_ddl_owner` → `OwnerNotifier::notify`，且通知失败被有意忽略。

RustCodeGraph 报告 `types.rs` 被 24 个文件引用；但对同名方法的精确 `callers/callees` 命令未产生静态边，故上述局部调用关系以索引中的 `submit.rs` 源码上下文复核，而不是假称图已解析动态分派。

## 错误处理与边界

`check_involving_schema_info` 返回 `ErrorKind::Invalid`，错误消息与对象类型约束或名称缺失直接对应。`Session`、系统表和通知 trait 均使用统一 `Error`，但实际重试语义由 `kind` 决定：事务层仅重试 `Retryable`，悲观锁循环仅捕获 `WriteConflict`。

编码辅助函数使用 `expect`：参数 JSON 序列化、reorg 元数据转换和最终 `model::Job::encode(false)` 被视为内部不变量，失败会 panic，而不是返回 `Result`。因此引入不可序列化字段或扩展元模型时，应优先把失败纳入测试，并决定是否仍可维持“编码不失败”的契约。

边界情况包括：

- `JobArgs::Opaque` 绕过 V1/V2 JSON 重建并保留原字节，适合尚未建模的参数，但调用方负责确保其 wire 兼容。
- 未识别 action 可用 `JobType::Other(code)` 保留数值，但 `JobArgs` 仍可能需要 `Opaque` 才能无损持久化。
- `Job::default` 产生版本 0、空名称和 `Other(0)`，只是构造便利值，不能直接通过正常提交校验。
- `normalize_involving_name` 使用 Rust Unicode lowercase；它不是 ASCII-only 变换，且可能改变字符长度。当前代码没有额外排序或去重。
- `OwnerNotifier` 的错误在 `notify_ddl_owner` 中被吞掉；通知是唤醒优化而不是 job 持久化成功的判据。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。并发契约主要通过 trait bounds 和所有权表达：`Session: Send` 允许独占会话跨线程转移；池、系统表、最小 ID、升级状态、BDR 与 owner 通知接口均为 `Send + Sync`，可由多个提交者共享；共享依赖和回调存放在 `Arc` 中。

`SessionPool::get` 返回独占的 `Box<dyn Session>`，`submit_batch` 无论业务成功或失败都调用 `put` 归还。事务生命周期由 `Session::begin`/`commit`/`rollback` 显式管理；提交实现只在 begin 成功后回滚，避免回滚未启动事务。

`Cleanup` 是 `FnOnce + Send`，保证一次失败尝试的资源清理至多执行一次；成功 commit 后提交实现将其清空。`BeforeInsert` 是可共享的 `Fn`，每次重试都可能重新调用并产生新的 cleanup，因此实现者必须让“建立资源—失败清理”按单次 attempt 配对。`backoff` 同样可共享，既用于事务级 `Retryable` 错误，也用于全局 ID 锁写冲突。

## 与 Go 版本的对应关系

Go 同路径 `pkg/ddl/jobsubmit/types.go` 只定义 `JobSpec` 和 `SubmitOptions`，直接复用 `pkg/meta/model.Job`、`model.JobArgs`、`kv.Storage`、DDL session pool、system-table manager 和 server-state syncer。Rust 文件为了形成可编译、可注入的独立 crate，额外建立精简 `Job`/`JobArgs` 模型和多个 trait；这是实现形态差异，不应误认为 Go 也有这些同名本地类型。

语义对应关系：

- Rust `JobSpec` 对应 Go `JobSpec`，都绑定 job、typed args 与 `IDAllocated`。
- Rust `BeforeInsert`/`Cleanup` 对应 Go `BeforeInsertWithAssignedIDs func(specs []*JobSpec) (cleanup func())`，并由重试逻辑在失败 attempt 执行。
- Rust `SessionPool`、`SystemTableManager`、`MinJobIdProvider`、`ServerState` 分解了 Go `SubmitOptions` 中的具体依赖；Rust 另外显式注入 `BdrPolicy`、重试次数与退避回调。
- Go `SubmitBatch` 调用 `model.Job.NormalizeInvolvingSchemaInfo`、`CheckInvolvingSchemaInfo`、`MayNeedReorg`、`Started` 和 `Encode(true)`；Rust 在本文件中复刻这些提交所需语义，并由 `submit.rs` 以同样顺序调用。
- Go `insertDDLJobs2Table` 将 `Job.Encode(true)` 结果写入 `mysql.tidb_ddl_job`；Rust `Job::encode` 注释虽说明对齐该格式，内部调用元模型 `encode(false)` 是因为 typed args 已由本文件提前写入 `raw_args`。

已确认的迁移差异：Go 的 `job.Version != 0` 使用 `intest.Assert`，Rust 正常返回 `ErrorKind::Invalid`；Go 编码返回 error，Rust 编码 API 返回 `Vec<u8>` 并对内部失败 panic；Go 具体存储/context 能携带更多运行时语义，Rust trait 只暴露当前提交逻辑需要的方法。扩展时应保留 Go 行为，不应因为 Rust 接口较窄而删减分支。

## 扩展指南

新增 DDL 类型时，通常需要同步以下位置：

1. 在 `JobType` 增加变体并在 `code` 中使用 Go `model.ActionType` 的真实数值；若暂不建模，使用 `Other`，不要发明新码。
2. 若参数有结构化语义，在 `JobArgs` 增加变体，并同时实现 V1 位置数组与 V2 具名对象编码；参考 Go `model.JobArgs`/`FillArgs`/`Encode(true)` 验证字段名、顺序和默认值。
3. 若涉及新对象 ID，修改 `pkg/ddl/jobsubmit/submit.rs` 的 `required_global_id_count`、`assign_global_ids_for_jobs`、`job_schema_ids`、`job_table_ids`，保证计数、消费顺序和系统表索引列一致。
4. 若新类型需要回填，更新 `Job::may_need_reorg` 以及 `SubJob::may_need_reorg`，同时验证普通 job 与 `MultiSchemaChange`。
5. 若新增持久化字段，更新 `Job` 默认值和 `Job::encode` 到 `model::Job` 的映射，并评估旧版本 job、V1/V2 wire、panic 边界和恢复兼容性。
6. 若增加外部依赖，优先扩展细粒度 trait 与 `SubmitOptions`，维持所有权和 `Send + Sync` 契约；资源型钩子必须提供失败 cleanup。

测试不得内嵌到本文件。编码回归应放在同目录独立文件 `pkg/ddl/jobsubmit/types_test.rs`；提交状态、BDR、升级、事务重试和资源清理行为应扩展 `pkg/ddl/jobsubmit/submit_test.rs`，并与 Go 的 `pkg/ddl/jobsubmit/submit_test.go` 对照。兼容风险最高的是 action code、V1 参数位置、V2 JSON 字段名、状态码和默认值；性能风险主要来自批量 job 的 JSON 分配、reorg 元数据二次 serde 转换，以及过大的 `involving_schemas`/`sub_jobs`。

## 验证依据

- RustCodeGraph：`status` 显示索引有效（11,467 文件、307,296 节点、1,848,419 边）；`files --filter pkg/ddl/jobsubmit` 确认目标及相邻 Rust/Go 文件均被索引。
- RustCodeGraph：`node --file pkg/ddl/jobsubmit/types.rs` 分段读取 1–678 行，核对全部枚举、结构体、trait、别名、方法和编码辅助函数；索引报告该文件被 24 个文件使用。
- RustCodeGraph：查询 `normalize_involving_schema_info`、`check_involving_schema_info`、`may_need_reorg`、`encode_job_args`、`JobSpec`、`SubmitOptions`，并读取 `pkg/ddl/jobsubmit/submit.rs`，确认提交前校验及系统表编码调用链。精确 `callers/callees` 对这些方法未返回静态边，此限制已在本文明确披露。
- RustCodeGraph：读取 `pkg/ddl/jobsubmit/types_test.rs`，其 `encode_uses_go_job_json_and_versioned_raw_args` 验证 type/state/version/start_ts/BDR 与 V2 `raw_args`；读取 `pkg/ddl/jobsubmit/submit_test.rs` 的 `involving_schema_check_uses_job_name_fallback_and_wildcards`、`may_need_reorg_matches_job_type_and_sub_job_rules`，验证名称通配符和 reorg 分支。
- crate/模块证据：`pkg/ddl/jobsubmit/Cargo.toml`、`pkg/ddl/jobsubmit/lib.rs`。
- DDL 包契约：`pkg/ddl/doc.go`；DDL 阅读入口：`docs/agents/ddl/README.md`。后者仅用于定位，行为结论均由代码和测试复核。
- Go 对照：`pkg/ddl/jobsubmit/types.go`、`pkg/ddl/jobsubmit/submit.go`、`pkg/ddl/jobsubmit/submit_test.go`；元模型相邻实现还由 RustCodeGraph 查询到 `pkg/meta/model/job.rs` 的同名方法。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰好包含十一个固定二级章节，并人工检查没有把未解析的静态调用边、理想架构或 Go 的具体依赖误写成 Rust 当前事实。

# `pkg/ddl/table_mode.rs`

## 文件定位

[`table_mode.rs`](./table_mode.rs) 属于 `astersql-ddl` crate，并由 [`lib.rs`](./lib.rs) 的 `pub mod table_mode` 暴露。它并不只处理“表模式”：文件前段提供表模式的精简模型和权限判定，后段实现普通 DDL 持久化 worker 的执行器、schema 同步屏障接口，以及 action 75（AlterTableMode）的真实元数据处理函数。

生产主链是 [`pkg/session/runtime/normal_ddl_service.rs`](../session/runtime/normal_ddl_service.rs) 或 `session_factory.rs` 创建 `NormalDdlExecutor`，再由 [`job_worker.rs`](./job_worker.rs) 的 `DurableJobExecutor` 协议逐步执行持久化 job。表模式 action 最终经 [`persistent_actions.rs`](./persistent_actions.rs) 的 `step_metadata` 分派到 `on_persistent_alter_table_mode`。因此，真正的表模式修改是 job-based、owner-driven 的元数据变更，不是绕过 DDL 队列的快速路径；它保持 `SchemaState::Public`，不做 reorg/backfill。

## 核心职责

1. `TableMode`、精简 `TableInfo`、`alter_table_mode` 与 `on_alter_table_mode` 表达 Normal/Import/Restore 的基本迁移规则和幂等版本语义；它们目前直接用于 [`table_mode_test.rs`](./table_mode_test.rs) 的局部行为测试。
2. `TableOperation` 与 `table_mode_allows` 描述保护模式下的操作矩阵：Normal 全部允许，Import/Restore 只允许元数据读取和 checksum。当前仓库内该函数没有非测试调用者，生产 SQL 层的完整保护逻辑不能等同于这个辅助函数。
3. `DdlSchemaBarrier`、`DdlJobPolicy` 与 `NormalDdlExecutor` 把 schema 版本同步、升级期准入、持久化 action 执行、错误累计、暂停/取消、MDL 登记和 job 收尾组合到普通 DDL worker。
4. `on_persistent_alter_table_mode` 在调用者拥有的 KV statement stage 中解析 Go 兼容 job 参数，校验库表与迁移，更新完整表元数据和 schema diff，并完成 job。
5. `sql_text`、`is_system_related_schema`、`go_tso_datetime` 是该执行器写 MDL/history SQL 时使用的局部辅助函数；其中前两个也被同 crate 的 `persistent_modify_column.rs` 和 `schema_version.rs` 复用。

## 主要符号

- `enum TableMode { Normal, Import, Restore }`：精简模式枚举。Import 与 Restore 不能直接互转，必须经过 Normal。
- `struct TableInfo { schema_id, table_id, mode, version }`：仅保存迁移测试所需字段；不要与 `astersql_meta_model::TableInfo` 完整持久化模型混淆。
- `alter_table_mode(&mut TableInfo, TableMode) -> Result<bool, String>`：拒绝 `table_id == 0` 和 Import↔Restore；同模式返回 `Ok(false)`，真实迁移更新模式并将本地版本加一。
- `on_alter_table_mode(...) -> Result<i64, String>`：先核对 schema/table ID，再调用精简迁移；no-op 返回 0，变更返回新版本。
- `enum TableOperation` / `table_mode_allows`：六类操作及保护模式矩阵。
- `trait DdlSchemaBarrier`：`recover` 恢复前任 owner 未完成的版本同步，`wait` 等待当前版本完成 follower/MDL 同步；真实实现是 [`schema_version.rs`](./schema_version.rs) 的 `NormalDdlSchemaBarrier`。
- `trait DdlJobPolicy`：`runnable` 决定 job 是否可调度，`error_limit` 给出 action 错误上限，`mdl_owner` 决定是否登记 MDL owner；真实实现是 [`normal_policy.rs`](./normal_policy.rs) 的 `NormalDdlJobPolicy`。
- `struct NormalDdlExecutor<B, P>`：持有 barrier、policy 与全局 `AtomicI64` sequence，并实现 `job_worker::DurableJobExecutor`。
- `NormalDdlExecutor::step`：一次普通 DDL action 的核心事务步骤；区分暂停、取消、终态、action 成败与 MDL 登记。
- `NormalDdlExecutor::finish`：写 KV/SQL history、注册 delete ranges、分配序号并删除活动队列行。
- `on_persistent_alter_table_mode(&mut dyn Transaction, &mut Job) -> Result<i64, String>`：完整持久化 TableMode action handler。

## 执行流程

普通 DDL job 的一次执行按以下顺序进行：

1. worker 先通过 `DdlJobPolicy::runnable` 做升级期/暂停策略判断，并通过 `DdlSchemaBarrier::recover` 处理旧 owner 遗留同步；接口约束见 [`job_worker.rs`](./job_worker.rs) 的 `DurableJobExecutor`。
2. `NormalDdlExecutor::step` 首先要求 `persistent_actions::handler_available(job.tp)`；未知且非终态 action 立即报错，避免没有实现的 action 被当作成功。
3. `Paused` 保持不动；`Pausing` 转为 `Paused` 并记录 `[ddl:8262]`；`Cancelling` 通常转为 `Cancelled`，但处于 `WriteReorganization` 的 create-materialized-view 会进入 `Rollingback`。已有终态直接进入 `finish`。
4. 执行器通过 Go wire encode/decode 复制一个内存 job，再补回未进入持久化 JSON 的 `multi_schema_info`、`need_reorg` 和 decoded `args`。`with_execution_context` 打开真实 worker 上下文，先 `StageStatement`，再调用 `persistent_actions::step`。
5. action 成功时 `ReleaseStatement` 并记录 schema version；action 失败时 `CleanupStatement`，把错误和计数写回 job，超过 `error_limit` 且可回滚时置为 `Cancelling`。这类 action 错误是可持久化的 job 结果；stage/存储等基础设施错误才从 `step` 返回 `Err`。
6. action 75 在 `persistent_actions::step_metadata` 中进入 `on_persistent_alter_table_mode`：解析 V1 数组首元素或 V2 对象，读取数据库和完整表，验证模式值及 `CanTransitionTo`，no-op 直接完成；真实变更调用 `update_version_and_table` 写表、schema version/diff，再以 Public schema state 完成 job。
7. 若 action 产生正版本，启用 MDL 时从 `mysql.tidb_ddl_job.table_ids` 取得完整依赖表集合，并 `REPLACE` 到 `mysql.tidb_mdl_info`；系统库不写 owner ID。随后 worker 可调用 barrier 等待版本同步。
8. job 达到 `Done`、`Cancelled` 或 `RollbackDone` 后，`finish` 将 Done 转为 Synced，必要时注册 delete ranges，分配单调 `seq_num`，在 KV history 设置 `finished_ts`，容忍 SQL history 插入失败，最后删除 `mysql.tidb_ddl_job` 队列行并返回 `removed: true`。

## 数据与状态

- 精简状态：`TableInfo.version` 只在 `alter_table_mode` 的真实迁移中加一；同模式不变。它是局部测试模型，不是全局 schema version 的存储实现。
- 持久化状态：`on_persistent_alter_table_mode` 操作 `astersql_meta_model::TableInfo.Mode`，并由 `update_version_and_table` 生成全局 schema version 和 action 75 的 schema diff。成功后 job 为 `Done/Public`；同模式返回版本 0；非法输入通常将 job 置为 `Cancelled`。
- job 状态：`NormalDdlExecutor` 显式处理 `Paused`、`Pausing`、`Cancelling`、`Rollingback`、`Cancelled`、`RollbackDone`、`Done`、`Synced` 和运行态。action 前通常置 `Running`，但 rollback 流程保留 `Rollingback`。
- 兼容参数：V1 的 `raw_args` 是数组，handler 取首元素；V2 直接使用对象。缺失 `table_mode` 按 0（Normal）解释，非整数或超出 0..=2 的值报错。
- 历史/MDL：`last_schema_version` 驱动 MDL 登记；`seq_num` 来自共享原子计数器；history job 通过 Go 兼容编解码保留结构化错误与元数据。

## 依赖与调用关系

- 上游构造：[`pkg/session/runtime/normal_ddl_service.rs`](../session/runtime/normal_ddl_service.rs) 的 `with_upgrade_policy` 和 [`pkg/session/runtime/session_factory.rs`](../session/runtime/session_factory.rs) 构造 `NormalDdlExecutor`。
- 上游协议：[`job_worker.rs`](./job_worker.rs) 的 `DurableJobExecutor` 调用 `runnable`、`recover`、`step`、`wait_synced`；该 trait 明确区分可持久化 action 错误与应放弃事务的基础设施错误。
- action 分派：[`persistent_actions.rs`](./persistent_actions.rs) 对 `job.tp == 75` 调用 `on_persistent_alter_table_mode`；75 对应 `astersql_meta_model::group_3::ACTION_ALTER_TABLE_MODE`。
- 策略和屏障：[`normal_policy.rs`](./normal_policy.rs) 实现升级期 pause/resume 与错误上限；[`schema_version.rs`](./schema_version.rs) 实现 owner lease 检查、MDL 恢复和版本等待。
- 元数据依赖：`astersql-meta` 提供 `TransactionMutator`、table/database 读取、history 与 TSO 时间；`astersql-meta-model` 提供完整 `Job`、`JobState`、`SchemaState`、`TableMode`；`astersql-kv` 提供调用者拥有的事务和 statement stage。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 将 `astersql-meta`、`astersql-meta-model`、`astersql-kv`、`serde_json` 声明为直接依赖，`[lib] path = "lib.rs"`，porting 元数据指向 Go 包 `pkg/ddl`。
- 当前调用限制：RustCodeGraph 将本文件标为被多个 DDL/session 文件使用，但精确仓库搜索显示 `alter_table_mode`、`on_alter_table_mode`、`table_mode_allows` 的直接调用仅在独立 Rust 测试中；生产变更入口是 `on_persistent_alter_table_mode`，权限拦截的完整生产接线位于其他模块。

## 错误处理与边界

- 精简函数使用普通字符串错误：不存在的 `table_id == 0`、ID 不匹配和非法 Import↔Restore。它们不携带 TiDB terror class/code，不应作为生产错误 wire 的依据。
- 持久化 handler 用 `[ddl:1105]` 标识参数解码/类型错误，用 `[schema:1049]` 标识数据库不存在，用 `[schema:8259]` 标识未知模式或非法迁移。数据库、非法现存模式、非法目标与非法迁移会取消 job；底层 `public_table`/元数据更新失败则向外传播，由普通 worker 清理 stage、累计错误并决定重试或取消。
- 同模式是成功 no-op：handler 设置 `Done` 并返回 0，不发布 schema version。Import↔Restore 依赖完整模型的 `CanTransitionTo` 拒绝；安全路径是先回 Normal。
- `NormalDdlExecutor` 不允许缺少 persistent handler 的运行中 action 静默完成。暂停/取消分支具有专门状态和错误码，materialized-view 的特例不能推广到 TableMode。
- SQL history 插入失败仅输出诊断，KV history 仍是权威记录；相反，删除队列行、KV history 或事务/stage 失败会返回错误。
- `sql_text` 用十六进制 SQL literal 避免 SQLMode、引号与反斜杠差异；扩展 SQL 拼接时应继续使用安全编码，不应直接插入未转义文本。

## 并发与资源生命周期

- owner 生命周期由 `JobLease` 和 `DdlSchemaBarrier` 控制：恢复和等待必须观察 owner 身份与 scheduler cancellation，旧 owner 不能在失去所有权后继续提交。
- action 的元数据写入位于 `StageStatement`/`ReleaseStatement` 边界；失败路径调用 `CleanupStatement`，确保已经写入的 version/diff 不会形成“幽灵版本”。[`table_mode_test.rs`](./table_mode_test.rs) 用 KV 大小失败验证了该回滚性质。
- `NormalDdlExecutor::sequence` 是共享 `Arc<AtomicI64>`，以 `AcqRel` 为终态 job 分配序号；action 执行结果通过 `Arc<Mutex<Option<Job>>>` 从 `'static` execution closure 安全传回执行器。
- SQL queue、KV metadata 与 history 的提交边界由 `DurableJobSession`/`JobExecutionContext` 管理。delete-range 注册使用独立 autocommit 语义；TableMode 本身不需要 delete range 或 backfill。
- Go 测试 [`table_mode_test.go`](./table_mode_test.go) 覆盖并发提交同一目标模式的幂等成功，以及 Restore/Import 竞争时一个合法结果、一个非法迁移结果。Rust 本文件不自行创建线程或锁表；冲突串行化依赖普通 DDL 队列、事务重读和 owner worker。

## 与 Go 版本的对应关系

- [`table_mode.go`](./table_mode.go) 的 `onAlterTableMode` 对应 Rust `on_persistent_alter_table_mode`：两者都解码 `AlterTableModeArgs`、读取完整表、同模式 no-op、用 `CanTransitionTo` 校验、更新版本与表，并以 `Done/Public` 完成 job。
- Go `alterTableMode` 只做 `CanTransitionTo` 与赋值；Rust 持久化 handler 内联了该过程。Rust 前段的精简 `alter_table_mode`/`on_alter_table_mode` 是便于局部验证的附加模型，不是 Go 函数的一对一生产替代。
- Rust V1/V2 参数分支对应 Go job args 的兼容格式；action code 75、错误 display code、Go wire job 编解码和 schema diff 字段由 [`table_mode_test.rs`](./table_mode_test.rs) 验证。
- Go [`table_mode_test.go`](./table_mode_test.go) 的范围更广：它通过 SQL 覆盖 Import/Restore 的 metadata/checksum 例外、DML/DDL 拒绝、并发迁移和 RefreshMeta。Rust 独立测试当前覆盖精简权限矩阵、迁移/幂等/ID 校验、真实 KV stage 清理与错误 wire；不能据此声称 Rust 已在本文件中复刻 Go SQL 入口的所有保护检查。
- Go 注释指出 BR 通常通过 `(batch)CreateTableWithInfo` 设置 Restore，而不是普通 AlterTableMode；Rust handler仍按模型校验所有 0..=2 目标，实际调用约束应由 job 构造/上游入口共同保持。

## 扩展指南

- 新增模式或修改迁移矩阵时，先改 `astersql-meta-model::TableMode::CanTransitionTo` 和 Go 对应模型，再同步本文件的精简 `TableMode`、`alter_table_mode`、数值解码范围及错误文本；同时更新独立 [`table_mode_test.rs`](./table_mode_test.rs)、Go [`table_mode_test.go`](./table_mode_test.go) 和 jobsubmit 测试。不要把 Rust 测试内嵌回生产源文件。
- 修改持久化 TableMode 行为时，以 `on_persistent_alter_table_mode` 为 action handler 接入点，并保持调用者拥有 statement stage、no-op 不升版本、完整 `TableInfo` 字段不丢失、`Done/Public` 及 Go V1/V2 wire 兼容。
- 新增普通 DDL action 时，应在 `persistent_actions::handler_available`/`step` 建立明确分派，而不是让 `NormalDdlExecutor` 默认成功；涉及 schema 版本时还要验证 MDL table IDs、barrier wait 和 history 收尾。
- 修改暂停、取消、upgrade admission 或 schema sync 时分别进入 `NormalDdlExecutor::step`、[`normal_policy.rs`](./normal_policy.rs) 和 [`schema_version.rs`](./schema_version.rs)，避免把集群协议塞进 TableMode handler。
- 性能风险主要在每步 Go wire 深拷贝、SQL/MDL 查询与 history 编码，不在精简权限 match；兼容风险集中于 job raw args、错误码、action 75、schema diff 和 history wire；正确性风险集中于 stage 释放/清理、owner fence、no-op 版本语义和非法迁移。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ddl/table_mode.rs` 定位目标；`node --file ... --offset 1/261` 读取 482 行与 38 个符号；`query` 定位 `alter_table_mode`、`on_alter_table_mode`、`table_mode_allows`、`NormalDdlExecutor`、`on_persistent_alter_table_mode`。caller/callee 命令未返回可用边，因此调用关系按技能规则由精确源码搜索补证。
- 已读生产证据：[`table_mode.rs`](./table_mode.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`persistent_actions.rs`](./persistent_actions.rs)、[`job_worker.rs`](./job_worker.rs)、[`normal_policy.rs`](./normal_policy.rs)、[`schema_version.rs`](./schema_version.rs)、[`pkg/session/runtime/normal_ddl_service.rs`](../session/runtime/normal_ddl_service.rs)。
- 已读对照与测试：[`table_mode.go`](./table_mode.go)、[`table_mode_test.rs`](./table_mode_test.rs)、[`table_mode_test.go`](./table_mode_test.go)，并通过 `pkg/meta/model` 中 action 75、TableMode 和 job args 定义核对 wire/枚举来源。
- 结构校验使用任务指定命令，要求文件存在且恰有 11 个固定二级标题。本任务只新增说明文档，不修改运行时代码，按计划不运行 Cargo。

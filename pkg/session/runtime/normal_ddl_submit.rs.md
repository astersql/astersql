# `pkg/session/runtime/normal_ddl_submit.rs`

## 文件定位

[对应源文件](normal_ddl_submit.rs)属于 `astersql-session` crate（见 `pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以私有模块 `normal_ddl_submit` 装配。它位于普通 SQL DDL 的会话入口与 durable DDL owner 队列之间：`pkg/session/runtime/ddl.rs` 先解析、校验 SQL 并构造动作参数，本文件把参数转换为 `astersql_ddl_jobsubmit::JobSpec`、提交到系统表、等待历史任务终态，最后刷新当前会话看到的 schema 版本。

直接接线有两条：`ConcreteSession::submit_normal_action` 被 `ddl.rs` 的持久化动作分支调用；`submit_and_wait` 被 `NormalDdlService::submit_persistent_job`（`pkg/session/runtime/normal_ddl_service.rs`）调用。该模块不负责 owner 选举、后台调度或具体 schema 变更；这些分别由 normal DDL service、scheduler 和 `astersql_ddl::persistent_actions` 一侧承担。

## 核心职责

1. `Bdr` 把系统表中保存的字符串角色映射为 `astersql_ddl_bdr::ast::BDRRole`，并以 `submit::BdrPolicy` 接口复用 DDL BDR 拒绝规则。
2. `submit_and_wait` 组装提交依赖与 `JobSpec`，保留 Go job 所需的 schema/table、SQL mode、CDC source、reorg metadata 和 involving-schema 信息；提交成功后等待同一 job 出现在 durable history。
3. `ConcreteSession::persistent_actions_enabled` 只在 Domain 已安装且其 DDL service 明确支持 durable actions 时开启该路径，避免把默认 trait 实现误当成可用服务。
4. `ConcreteSession::submit_normal_action` 从当前 infoschema 和会话变量构造 Go 兼容的 `Job`。对 `ModifyColumn`（动作 12）、`TruncateTable`（动作 11）和 `RenameTable`（动作 14）补充各自必须的语义，再通过 DDL service 同步等待完成、reload Domain 并更新自身 schema version。

## 主要符号

- `struct Bdr`：文件私有、无状态的策略适配器。`Bdr::is_denied(role, tp, args)` 接受 jobsubmit 层的角色字符串和 `JobType`；未知字符串映射为 `BDRRole::Unknown`，最终调用 `astersql_ddl_bdr::IsDenied`。当前实现不使用 `JobArgs` 参数，而是以 job type code 判定。
- `submit_and_wait(pool, cancel, state, job) -> Result<(), String>`：模块内可见的提交与等待入口。`pool` 同时提供 jobsubmit 所需会话池和 durable-history 查询；`cancel` 是 normal DDL service 生命周期上下文；`state` 可传升级期 server-state；`job` 会在成功或已落历史的失败终态时被历史快照整体覆盖。
- `ConcreteSession::persistent_actions_enabled() -> bool`：检查 `Domain::ddl()` 并调用 `DdlService::supports_persistent_actions`。trait 默认返回 `false`，`NormalDdlService` 返回 `true`（`pkg/domain/domain.rs`、`pkg/session/runtime/normal_ddl_service.rs`）。
- `ConcreteSession::submit_normal_action(database, table, action, args) -> SessionResult<()>`：SQL runtime 的持久化普通 DDL 适配入口。它查表和库、编码 args、生成 reorg snapshot，调用 `submit_persistent_job`，然后依次执行 `domain.reload()` 与 `update_self_version_with_retry()`。

本文件没有公开 crate API、模块常量、trait 定义或条件编译项；三个业务入口均限制在父模块范围或 `ConcreteSession` 内部。

## 执行流程

`submit_normal_action` 的主流程如下。

1. 通过 `Domain::table_by_name` 与 `info_schema().AllSchemas()` 解析现有表、库及其物理 ID；库名比较不区分大小写，找不到库时报 `unknown database`。
2. 从会话状态解析 SQL mode，把 action、schema/table ID、规范化的小写名称和 JSON 编码的原始参数写入 `meta_model::Job`。
3. action 12（修改列）先从 `args["column"]` 反序列化新列，并按 `old_column_name.L/l` 找旧列；找到时调用 `persistent_modify_column::no_reorg_data_strict` 的反值设置 `job.need_reorg`。随后快照 analyze 变量、时区、资源组、collation、reorg 并发、batch size 和最大写速率。只有确实需要 reorg 时才设置 fast/dist reorg、target scope、最大节点数，并执行系统库禁用和“dist 必须依赖 fast”约束。
4. action 14（单表 rename）用 `new_schema_id` 覆盖 job 的 schema ID；多表 rename 使用 action 47，完整有序链由 `ddl.rs` 放在 args 中。
5. 从 Domain 取得 DDL service 并同步调用 `submit_persistent_job`；成功后 reload infoschema，再用重试更新当前 session 的 schema version。

`submit_and_wait` 的主流程如下。

1. 基于 `SystemSessionPool` 创建 system-table manager 和 min-job-id refresher，立即刷新一次最低 job ID，再生成 `SubmitOptions`；覆盖其中的 BDR policy，并从池中借一个真实会话读取 CDC source。
2. 把 `meta_model::Job` 转成 jobsubmit `JobSpec`。已知动作 12、14、47分别映射为 `ModifyColumn`、`RenameTable`、`RenameTables`，其余先保留为 `Other(code)`；复制 query、SQL mode、session vars、reorg meta、involving schemas 等字段。输入 job 未指定 CDC source 时才采用提交会话的值。
3. action 11 单独转换为 `TruncateTable`：读取旧分区 ID，设置 `id_allocated = false`，让 jobsubmit 在同一分配/插入事务中分配新表及新分区 ID。`before_insert_with_assigned_ids` 再把新 ID 写回 Go 兼容的 opaque JSON args。
4. `submit::submit_batch` 校验 flashback/BDR/involving-schema/升级状态，在悲观事务内分配 ID 并写入 `mysql.tidb_ddl_job`；成功后把分配得到的 job ID写回调用方。
5. 每 30 ms 从持久化 history 查询该 ID。取消上下文触发时返回取消错误；历史状态为 `Synced` 时成功；其他历史终态返回 job 自带错误，缺失错误时生成包含 ID 与状态的兜底错误。

## 数据与状态

- 输入的 `Job` 不是只读值：提交后至少更新 `id`；读到 history 后通过 `*job = history` 替换为最终 durable 快照。因此调用者可以取得最终状态、错误和 worker 写回的元数据。
- `JobSpec.id_allocated` 一般为 `true`，因为 SQL runtime 已持有对象 ID；truncate 是例外，必须在 jobsubmit 的全局 ID 事务中同时分配新表 ID、新分区 ID 和 job ID。
- `cdc_write_source == 0` 表示普通来源，才从 pooled session 继承 `tidb_cdc_write_source`；非零值被保留，也使 jobsubmit 的 BDR 限制不适用。
- `DDLReorgMeta` 是提交时的会话快照，而不是 worker 执行时重新读取的动态配置。它包含 SQL mode、时区、resource group、collation 版本、并发/batch/write-speed，以及需要 reorg 时的 fast/dist 和 target scope。
- `need_reorg` 由新旧列的严格数据兼容性决定。系统 schema 强制关闭 fast/dist reorg；非系统 schema 的 dist reorg 必须同时开启 fast reorg，否则提交前返回错误 8200。
- 本文件本身没有全局可变状态。全局配置读取（如 `EnableDistTask`、`DDLReorgMaxWriteSpeed`）只被固化进新 job；实际队列、history、schema 和 ID 均保存在共享存储中。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 文件关系与源码引用共同确认：

- `pkg/session/runtime/ddl.rs` 在 DROP TABLE、RENAME TABLE、MODIFY/CHANGE COLUMN、DROP COLUMN/INDEX、TRUNCATE TABLE 等已支持分支先检查 `persistent_actions_enabled`，再调用 `submit_normal_action`。例如 rename action 为 14/47，modify action 为 12，truncate action 为 11。
- `pkg/session/runtime/normal_ddl_service.rs::submit_persistent_job` 检查 service 未关闭，构造可选 `JobSubmitServerState`，再调用 `normal_ddl_submit::submit_and_wait`。

主要下游依赖为：

- `pkg/session/runtime/system_session.rs`：`table_mode_submit_options` 把池适配成 jobsubmit 的 `SessionPool`、`SystemTableManager`、`MinJobIdProvider`；`ddl_session_variables` 读取真实 session 的 CDC source；`persistent_history` 从一致快照读取历史 job。
- `pkg/ddl/jobsubmit/submit.rs`：`submit_batch` 完成 flashback 检查、BDR/升级期规则、状态设置、全局 ID 分配、事务重试和 job 表插入。
- `pkg/ddl/bdr`、`pkg/ddl/systable`、`pkg/owner`：分别提供 BDR 判定、系统表/min-ID 管理和可取消生命周期上下文。
- `pkg/meta/model`：提供 Go wire/storage 兼容的 `Job`、`JobState`、`DDLReorgMeta` 和 JSON 编码形态。

`pkg/session/Cargo.toml` 明确声明了本文件直接使用的 `astersql-ddl-jobsubmit`、`astersql-ddl-systable`、`astersql-ddl-bdr`、`astersql-meta-model`、`astersql-owner`、`astersql-sessionctx-vardef`、`astersql-sessionctx-variable`、`astersql-config`、`astersql-config-kerneltype`、`astersql-util-collate`、`serde` 与 `serde_json` 等 crate 边界；`nextgen` feature 会传递给 kernel/deploy mode，但本文件不含条件编译分支。

## 错误处理与边界

- 表、库、SQL mode、JSON 编解码、会话池、历史读取、service 提交、Domain reload 和 session-version 更新错误都会向调用者传播；跨 crate 的普通错误统一转为字符串或 `SessionError`。
- reorg metadata 的 serde 往返、truncate 新 ID 写回使用 `expect`。这里假定仓库内 `DDLReorgMeta` 与 job args 必须始终可 JSON 表示；若模型演进破坏该约束会 panic，扩展字段时必须同步验证序列化兼容性。
- 数值型 reorg session vars 解析失败时使用 0，而不是立即报错；其后是否合法由 metadata setter/下游语义决定。
- action 12 找不到匹配旧列时不会在本函数报错，只是不设置 `need_reorg`；上游 `ddl.rs` 已先验证列存在，worker 仍负责最终 durable 校验。调用本内部函数的新入口不得绕过上游校验。
- action 11 的 `raw_args` 必须是 JSON object；无效 JSON直接返回错误。缺少 `old_partition_ids` 时按空列表处理。
- 等待循环没有本地超时；唯一中断是 service cancellation 或底层读取错误。取消只停止调用者等待，不在这里发送 cancel-job 命令，已入队任务仍由 durable DDL 生命周期管理。
- 只有 `JobState::Synced` 被视为成功；任何其他已入 history 的状态均为错误。这与 SQL 调用需要等待 schema 对外同步完成的契约一致。

## 并发与资源生命周期

`SystemSessionPool::acquire` 返回的 lease 采用作用域归还：一次用于读取提交会话变量，等待阶段每次轮询再短暂借用一个 lease 查询 snapshot history。`submit_batch` 内部也通过 `SubmitOptions` 借还会话，并把 ID 分配与 job 插入放在一个可重试的悲观事务中，保证 job 按 ID 顺序持久化；本文件不持有跨 sleep 的数据库会话或事务。

等待采用同步线程 `sleep(30 ms)`，因此会占用调用 SQL 的线程，但不占用 pool lease。normal DDL service 的 cancellation context 在 `stop()` 时被取消，`submit_and_wait` 下一轮检查即可退出；service 关闭还会停止 scheduler、等待 worker join，最后关闭 pool（见 `normal_ddl_service.rs`）。

`Arc` 用于跨 service/submit 层共享 pool、manager、min-ID refresher、server state 与回调。truncate 的 `move` 回调持有原始 JSON 快照，并只在分配 ID 后、插入前改写当前 spec；jobsubmit 的重试流程会在每次尝试重新分配并调用该回调，因此不能在新增回调里依赖单次执行或持有未同步的外部可变状态。

## 与 Go 版本的对应关系

Go 的核心对应不是 `pkg/session` 下的同名文件，而是 DDL executor 与 jobsubmit 两层：

- `pkg/ddl/executor.go::DoDDLJobWrapper` 同样先提交 job，再等待 history/完成通知，并在成功或失败时返回最终结果。Go 版本还包含 trace、metrics、session killer 驱动的 cancel-job、table lock、动态 ticker 与通知 channel；本 Rust 文件目前采用 cancellation context、30 ms history 轮询的较窄适配，不能宣称覆盖这些旁路能力。
- `pkg/ddl/jobsubmit/submit.go::SubmitBatch` 与 Rust `pkg/ddl/jobsubmit/submit.rs::submit_batch` 都执行 flashback 检查、读取 BDR role/start TS、校验 involving schemas、应用升级期暂停、分配全局 ID并插入系统表。本文件只负责构造它所需的 options/spec。
- `pkg/ddl/executor.go::NewDDLReorgMeta` 与 action 12 分支都快照 SQL mode、时区、resource group、collation 版本；Rust 额外在本入口写入 concurrency、batch size、max write speed 以及 fast/dist 配置，这些对应 Go 在 reorg 初始化路径读取的会话/全局参数。
- Go `SubmitBatch` 的 truncate ID 计数与分配规则要求新 table ID 和每个旧 partition 对应的新 ID；本文件以 typed `TruncateTable` args 暂存旧 ID，再用回调恢复 Go opaque job args，保证 durable worker 读取的格式不变。

当前 Rust 路径是有真实系统表、owner worker 和 history 的已接线路径，不是桩；但它只覆盖 `ddl.rs` 明确路由到 `submit_normal_action` 的动作集合。其他 SQL DDL 仍可能走 runtime 的本地实现或专用持久化入口。

## 扩展指南

- 新增普通 durable action 时，先在 `pkg/session/runtime/ddl.rs` 完成与 Go 相同的语义校验和 args 形状，再在 `submit_and_wait` 的 job-type 映射中加入明确的 `submit::JobType`。若动作需要分配对象 ID，必须使用 typed `JobArgs`、正确设置 `id_allocated`，并让 `pkg/ddl/jobsubmit` 的 required-count/assign 逻辑原子处理；不能在提交前自行分配后拼接。
- 新增或改变 reorg 参数时，应同步 `submit_normal_action` 的 snapshot、`pkg/meta/model` 编码和 worker 消费逻辑，并在独立测试文件验证 history 中的快照。不要把 Rust 测试内嵌到本生产文件；本目录惯例是 `*_test.rs`。
- 扩展 BDR 行为时，应优先修改共享 `astersql_ddl_bdr` 规则；仅当角色编码边界变化时调整 `Bdr::is_denied` 的字符串映射，并补系统 schema、CDC source 与未知 role 的测试。
- 改变等待策略时必须保留“入队后用实际 job ID 查 history、仅 Synced 成功、service stop 可退出”的不变量。若引入通知 channel、超时或主动取消，需要核对 Go `DoDDLJobWrapper` 的 killer、retry 和终态处理，避免把调用者超时错误误当作 job 已取消。
- 重点风险：JSON args 与 Go 编码不兼容会让 worker 无法解码；错误的 ID 分配会破坏表/分区身份；漏快照 session vars 会造成 owner 节点执行语义漂移；错误放宽 fast/dist 组合会引入不受支持的回填路径；增加更短轮询会提高系统表读取压力。
- 应同步/扩展的独立测试首选 `pkg/session/runtime/normal_ddl_masking_policy_test.rs`（SQL 到 durable history 的端到端动作与 reorg 快照）、`pkg/session/runtime/normal_ddl_test.rs`（service/scheduler 生命周期），以及 `pkg/ddl/jobsubmit/submit_test.rs`（提交、ID 与策略边界）。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/session/runtime` 确认目标及独立测试布局；`node --file pkg/session/runtime/normal_ddl_submit.rs` 读取全部 274 行并确认该文件被 `ddl.rs`、`normal_ddl_service.rs` 使用；对 `submit_and_wait`、`submit_normal_action`、`persistent_actions_enabled` 执行了 `query`/`callers`/`callees`。图对 impl method 的 callers 信息不完整，因此用精确 `rg` 引用补齐上游边。
- 源码与模块边界：`pkg/session/runtime.rs`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/session/runtime/system_session.rs`、`pkg/domain/domain.rs`、`pkg/session/Cargo.toml`。
- 下游提交实现：Rust `pkg/ddl/jobsubmit/submit.rs`；Go 对照 `pkg/ddl/jobsubmit/submit.go` 与 `pkg/ddl/executor.go`。
- 独立 Rust 测试：`pkg/session/runtime/normal_ddl_masking_policy_test.rs` 验证 action 4/6/11/12/14/47 的 SQL durable history、truncate 表/分区新 ID、rename 链单 job、修改列真实行回填，以及 SQL mode/时区/concurrency/batch/write-speed reorg snapshot；`pkg/session/runtime/normal_ddl_test.rs` 验证 normal service 消费 durable queue、幂等 stop 和 pool 生命周期。Go 的 `pkg/ddl/jobsubmit/submit_test.go` 覆盖 SubmitBatch 入队、全局 ID 分配与相关错误分支。
- 本任务是只读代码分析与 Markdown 新增，按计划不运行 Cargo。交付验证以固定章节结构检查、链接/路径存在性检查和上述源码事实人工复核为准。

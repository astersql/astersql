# `pkg/ddl/persistent_modify_column.rs`

## 文件定位

本文件是 `astersql-ddl` crate 中普通表 `MODIFY/CHANGE COLUMN` 持久化执行器，模块由 `pkg/ddl/lib.rs` 公开。会话侧在 `pkg/session/runtime/normal_ddl_submit.rs` 构造动作号 12（`ACTION_MODIFY_COLUMN`）的 `Job`，并预置 `need_reorg`、SQL mode、时区及 analyze 会话变量；owner worker 的 `pkg/ddl/table_mode.rs::NormalJobPolicy::step` 在可回滚的 statement stage 内调用 `pkg/ddl/persistent_actions.rs::step`，后者把动作 12 分派到本文件的 `step`。

它不是 SQL 语法检查或建 job 的入口，而是 job 已持久化以后、可跨重试继续执行的运行时状态机。所属 crate 由 `pkg/ddl/Cargo.toml` 定义为 `astersql-ddl`；本文件直接依赖 metadata mutation、parser AST、collation/type、table codec、KV key、DDL notifier、session variable 等同 workspace crate。DDL 总体仍是 job-based/owner-driven 路径，不是 metadata-only 快捷旁路（`docs/agents/ddl/README.md`、`docs/agents/ddl/07-modify-column.md`）。

## 核心职责

- 兼容解码和重写 V1 数组参数、V2 结构参数，并持久化执行中产生的 changing column/index、待 GC ID 和分区 ID（`Args`、`decode`、`save_args`、`finished_range_ids`）。
- 根据旧/新列类型、NULL 属性、索引、分区、TiFlash、SQL strict mode 和 collation 选择执行策略 1–5（`no_reorg_data_strict`、`choose_type`）。数字与 Go 的 `ModifyTypeNoReorg`、`ModifyTypeNoReorgWithCheck`、`ModifyTypeIndexReorg`、`ModifyTypeReorg`、`ModifyTypePrecheck` 一一对应。
- 驱动 metadata-only、带数据检查、index-only reorg、row+index reorg 以及 VARCHAR→CHAR 预检降级路径（`step`、`advance_reorg`）。
- 在事务中生成 schema version、记录 schema diff、更新 `TableInfo`；在 reorg 中维护 `mysql.tidb_ddl_reorg` 的物理表范围游标（`write_table`、`initialize_cursor`）。
- 列改名/换 ID 时同步 masking policy 的列名、表达式、状态、类型和限制项，并发布 modify-column notifier 事件（`rewrite`、`sync_policy`、`advance_reorg`）。
- 在成功或回滚结束时产出 delete-range 所需的旧索引、临时索引和 partition ID（`temporary_gc_ids`、`finished_range_ids`；消费者为 `pkg/ddl/delete_range.rs`）。

## 主要符号

- `Args`：私有、带 `serde(default)` 的 durable payload。`column` 是目标定义；`old_column_id/name` 用于恢复定位；`position` 保存 FIRST/AFTER 的 JSON；`modify_column_type` 保存策略；`changing_column/changing_indexes` 保存中途对象；`removed_idxs/index_ids/new_index_ids/partition_ids` 服务切换和 GC。
- `decode(&Job) -> Result<Args, String>` / `save_args(&mut Job, &Args)`：V1 使用位置数组（短格式或包含 changing objects 的长格式），非 V1 使用结构 JSON。字段顺序是跨版本恢复协议，不能随意调整。
- `finished_range_ids`：从完成态参数提取旧索引和物理分区；V1 读取数组前两项，V2 读取命名字段。它是 `delete_range::build_tasks` 的直接下游接口。
- `no_reorg_data_strict`：判断“无论现存数据为何都无需改写”的强条件，包括 decimal 精度、enum/set 尾追加、整数符号、binary CHAR 长度、vector 长度、字符串族和整数族转换。
- `choose_type`：策略选择器。`compat == 6`、分区表、存在 TiFlash、非 strict mode、符号或不兼容 collation 变化、必须重写行时走 4；安全快速路径走 1/2；仅索引编码需变更时走 3。
- `destination`：把 FIRST/AFTER 描述解析为最终 offset；AFTER 只接受另一个 `Public` 列，排除旧列自身。
- `rewrite` / `sync_policy`：通过 parser 把 masking expression 解析为单字段 `SELECT` 表达式，用 AST visitor 精确改列引用，再恢复 SQL；随后更新 `mysql.tidb_masking_policy`。
- `check_data` / `data_error`：构造有界 `SELECT ... LIMIT 1` 检查整数范围、长度、CHAR 尾空格和 NULL，映射为 1138 或 1265 风格错误。
- `initialize_cursor`、`decode_hex`、`hex`：维护 `mysql.tidb_ddl_reorg` 的 record/index key 范围检查点；merge 阶段改用临时索引 keyspace。
- `advance_reorg`：changing objects、schema state、reorg stage、checkpoint、backfill/ingest/merge、可选 analyze、发布切换的核心状态机。
- `step`：唯一主执行入口，负责参数/表/旧列恢复、策略选择、回滚、非 reorg 完成，以及向 `advance_reorg` 分流。

## 执行流程

1. `step` 解码参数；解码失败将 job 置为 `Cancelled`。它用 `persistent_actions::public_table` 在事务内载入表，再优先按持久化的 `old_column_id`、否则按旧列名定位当前列。
2. 策略为 0 或兼容值 6 时重新调用 `choose_type`，并立即 `save_args`；VARCHAR→CHAR 且原选择为 2/3 时先改为预检策略 5。
3. 若 job 已在 rolling back，清除 `PreventNullInsertFlag` 和 `ChangingFieldType`，删除 changing column/index；为 merge reorg 补充临时索引 GC ID，写表后以 `RollbackDone/None` 完成。
4. 策略 2/5 首次先设置防 NULL 写标志和可能的 `ChangingFieldType`，下一步执行 `check_data`。发现坏数据时，策略 2 转 rolling back；策略 5 则清理临时标志并升级为完整 reorg（4）。预检通过时重新选择 1/2/3/4。
5. 策略 1 走无 reorg 完成：检查重名，计算位置，保留旧 ID、设 `Public`，更新普通索引列名、外键列名、TTL 列名和 masking policy；同一事务生成 schema version/schema diff 并更新表，最后 `Done/Public`。
6. 策略 3/4 进入 `advance_reorg`。策略 4 首次创建隐藏 changing column 和相关 changing indexes；策略 3 保留同一列 ID，只设置 `ChangingFieldType` 并重建相关索引。二者都持久化生成的 ID。
7. changing objects 依次经过 `None → DeleteOnly → WriteOnly → WriteReorganization`。进入 DeleteOnly 时初始化索引 reorg，NULL→NOT NULL 给旧列加 `PreventNullInsertFlag`；index-only 在 DeleteOnly 做数据检查。
8. `WriteReorganization` 由 `job.reorg_meta.Stage` 细分为更新列、重建索引、完成。每批从 `mysql.tidb_ddl_reorg` 读取 `start_key/end_key/physical_id`，按 batch size 调用 `backfill_modified_column`，或调用 prepared/ingest/merge index backfill；更新 row count 与 start key，逐 partition 推进。
9. 需要 merge process 时 changing indexes 先 `ReadyToMerge`、再 `Merging`、最终 `Inapplicable`。阶段完成后依据 `tidb_enable_ddl_analyze`、analyze version、partition/multi-schema 条件决定运行或跳过 analyze。
10. 发布时把新列/新索引改名并置 `Public`，旧列/旧索引改成 removing 名并置 `WriteOnly`；后续 `step` 将旧对象推进到 `DeleteOnly` 并移除，更新 offset、TTL、masking policy，发布 notifier，收集 GC ID，最终 `Done/Public`。

## 数据与状态

持久状态分三层：`Job.raw_args` 保存 `Args` 和中途对象 ID；`Job.schema_state`/`Job.reorg_meta` 保存在线状态、阶段、snapshot、backfill type、analyze state；`mysql.tidb_ddl_reorg` 保存当前 element、key range 和 `physical_id`。每次只推进一个可重放的小步骤，通常返回 schema version；只更新检查点或 args 的步骤可返回 0。

表内状态包括 `TableInfo.Columns/Indices/ForeignKeys/TTLInfo`、最大 column/index ID、`ChangingFieldType`、`ChangeStateInfo`、index `BackfillState` 与 `UseChangingType`。关键在线不变量是：公开切换前 DML 能通过 schema state 和 changing metadata 同时维护旧/新表示；NULL→NOT NULL 检查窗口由 `PreventNullInsertFlag` 阻止新 NULL；发布后旧对象先降到 write/delete only 再删除。

事务不变量由 `write_table` 统一实现：同一 meta transaction 内依次 `gen_schema_version`、`set_table_schema_diff`、`update_table`。外围 `NormalJobPolicy::step` 还以 `StageStatement` 包裹单步，成功 release、失败 cleanup，避免失败步骤把部分 statement mutation 留在 durable transaction。

## 依赖与调用关系

上游主链为：`normal_ddl_submit.rs` 建立动作 12 job → durable scheduler/`table_mode::NormalJobPolicy::step` → `persistent_actions::step` → 本文件 `step`。`pkg/session/runtime/normal_ddl_submit.rs` 还直接调用 `no_reorg_data_strict` 填写 `job.need_reorg`。

下游分为四组：

- 元数据：`astersql_meta::TransactionMutator`、`persistent_actions::public_table/initialize_reorg_indexes/async_notify_event`。
- 数据重组：`JobExecutionContext::{backfill_modified_column,backfill_prepared_indexes,ingest_modified_indexes,merge_modified_indexes,analyze_modified_table}` 与 `crate::backfilling` 请求结构。
- SQL/策略：`JobExecutionContext::query` 访问 `mysql.tidb_ddl_reorg`、`mysql.tidb_masking_policy` 和业务表；parser AST/collation/type utilities 决定表达式及转换安全性。
- 完成清理：`pkg/ddl/delete_range.rs` 调用 `finished_range_ids`，按 table/partition 为旧索引及 merge 临时索引生成 delete-range task。

`pkg/ddl/Cargo.toml` 明确列出这些 crate 依赖且 `[lib] path = "lib.rs"`；没有本文件专属 feature gate。`target.'cfg(windows)'` 的大依赖表是 crate 平台配置，不意味着此模块只在 Windows 编译。

## 错误处理与边界

所有接口把底层错误规整为 `String`，但状态副作用按错误类别不同：参数损坏或旧列不存在置 `Cancelled`；changing object 初始化、坏数据、重名或 policy 同步等可回滚问题置 `Rollingback`；批量 reorg/SQL/事务错误多数直接上抛，由 durable policy 计数、清理 statement stage，并按 job 可回滚性决定后续处理。

明确边界包括：AFTER 缺 relative column 返回 `invalid column position`，找不到公开 relative column 返回 1054 风格错误；奇数长度 checkpoint hex、非法 hex、非法 physical ID、缺 reorg meta/changing object/index 都直接失败；数据检查只取首个坏行并返回 NULL 1138 或截断 1265；masking policy 禁止 generated column 和不支持的列类型，解析必须得到单字段表达式。

实现使用字符串组装内部 SQL，但 identifier 以反引号并双写反引号，policy 值经 `table_mode::sql_text`；扩展查询时必须保持这两类转义。`check_data` 对非整数变化主要用 `LENGTH` 和 VARCHAR→CHAR 尾空格检测；其覆盖范围应始终与 `choose_type/no_reorg_data_strict` 同步，不能单独放宽策略。

当前 Rust 执行器与 Go 全量入口仍有边界差异：Go `onModifyColumn` 在分流前还有 index prefix、auto-random、partial condition、generated/columnar/primary-key、partition recheck 等防线；这些可能位于 Rust 提交/建 job 路径或仍是迁移缺口。本文件文档只确认实际可见的 durable handler，不宣称它独立覆盖 Go 的全部前置校验。

## 并发与资源生命周期

本文件本身不创建线程；并发所有权在 owner-side durable scheduler。单个调用推进一个 job step，schema version 返回给外层做 schema barrier/sync。owner 转移或重试时，恢复点来自 job args、schema/reorg state 和 `mysql.tidb_ddl_reorg`，因此新增 ID、阶段和 cursor 必须在返回前持久化，步骤必须可重复执行。

reorg cursor 生命周期为：首次删除同 job 旧记录并插入当前 physical table 范围；每批提交后更新 `start_key`；一个 partition 完成后重置到下一个；全部完成后删除记录。`snapshot_ver` 首次从 transaction `StartTS` 取得，在切换 row/index 阶段时清零以重新初始化。batch size 至少为 1，row count 累加扫描量。

changing column/index 从创建、在线双写、回填、发布到旧对象清理跨多个 schema-sync 边界。回滚会删除尚未发布的 changing objects并恢复临时 flags；成功后旧/临时索引通过 delete-range 异步 GC。masking policy 更新和表 metadata 写入并非同一函数调用，policy 失败会阻止最终表提交或使 durable step cleanup；直接回归测试验证了缺 policy 系统表时不应错误发布改名结果。

## 与 Go 版本的对应关系

主要对照是 `pkg/ddl/modify_column.go`：Rust `choose_type` 对应 `getModifyColumnType`，`no_reorg_data_strict` 对应 `noReorgDataStrict`，`step` 对应 `onModifyColumn` 加无 reorg/回滚/完成分支，`advance_reorg` 对应 `doModifyColumnIndexReorg` 与 `doModifyColumnTypeWithData`，批量行回填对应 `doReorgWorkForModifyColumn`，完成参数对应 Go `model.ModifyColumnArgs`。

已对齐的核心语义包括：五类策略及旧版本兼容值、VARCHAR→CHAR 预检、分区/TiFlash/非 strict 强制完整 reorg、changing column/index 状态机、`ReorgStageModifyColumn*` 三阶段、NULL 防写、可选 analyze、old object 延迟移除及 finished args 交给 delete-range。

需要谨慎看待的差异：Rust 使用通用 `JobExecutionContext` 的同步 batch 调用和显式 SQL checkpoint，而 Go 通过 session pool、reorg handler、`runReorgJob` 及更细的 retry/error 分类；Go handler 的前置约束更多。Rust `Args` 的字段名也不是 Go struct 的逐字段拼写（例如 `removed_idxs` 对应 redundant/removed index 语义）。扩展时应按行为、不按表面函数形状对齐，并同时核对 `pkg/meta/model/job_args.go`、`pkg/meta/model/reorg.go`。

测试证据分两类：Go 的 `pkg/ddl/modify_column_test.go`、`column_modify_test.go`、`column_change_test.go` 和 partition modify tests 定义广泛兼容矩阵；Rust `pkg/session/runtime/normal_ddl_masking_policy_test.rs` 是本持久执行器的直接运行时证据。`pkg/ddl/modify_column_test.rs` 大量内容是 Go 测试语义记录/迁移辅助，不能单独视为本 handler 的端到端执行证明。

## 扩展指南

- 新增转换规则时，同时修改 `no_reorg_data_strict`、`choose_type`、`check_data`，并确认提交侧 `job.need_reorg` 与执行侧重算一致；至少增加独立 Rust 测试覆盖安全值、越界值、NULL、indexed/unindexed、strict/non-strict、partition/TiFlash。
- 新增 durable 字段时，在 `Args` 保持 default/backward compatibility，分别更新 V1 `decode/save_args` 的位置协议、V2 serde、完成态序列化及 `finished_range_ids`。必须模拟旧 job 重启恢复，不能只测新格式。
- 改 reorg 阶段时，修改 `advance_reorg`，明确 schema state、`ReorgMeta.Stage`、snapshot/cursor 初始化、partition 遍历、merge backfill state、analyze 和 multi-schema revertible 边界；测试要覆盖每个中断点的再次执行。
- 改列发布内容时同步检查普通/临时索引、外键、TTL、masking policy、notifier、offset 和 GC ID。masking expression 变换应扩展 `rewrite/sync_policy` 的独立测试，禁止用字符串替换代替 AST visitor。
- 回滚或 GC 变化应同时检查 `step` rollingback 分支、成功后的 old-object 删除分支、`temporary_gc_ids`、`delete_range.rs`；测试文件必须保持独立，优先扩展 `pkg/session/runtime/normal_ddl_masking_policy_test.rs` 或新增同目录 `*_test.rs`，不要把测试嵌入生产文件。
- 与 Go 对齐时以 `modify_column.go` 当前行为为基准，但只移植对应增量；若前置校验由提交层承担，应以调用证据标注，避免在 durable handler 重复或遗漏。

## 验证依据

- RustCodeGraph：索引状态为 11,467 文件、307,296 节点；`files --filter pkg/ddl/persistent_modify_column.rs` 确认目标文件含 28 个符号；`node --file ...` 完整读取 1–1341 行；`query persistent_modify_column` 列出 `Args`、`step`、`advance_reorg` 等主要符号；`callees step` 确认本地 helper 调用，并显示 `persistent_actions::step → persistent_modify_column::step` 静态边。
- 入口/下游：`pkg/ddl/lib.rs`（模块公开）、`pkg/session/runtime/normal_ddl_submit.rs`（动作 12 job 与 `need_reorg`）、`pkg/ddl/table_mode.rs`（statement stage 和 durable 单步）、`pkg/ddl/persistent_actions.rs`（动作分派）、`pkg/ddl/delete_range.rs`（完成 ID → GC task）。
- crate/设计：`pkg/ddl/Cargo.toml`、`docs/agents/ddl/README.md`、`docs/agents/ddl/07-modify-column.md`。
- Go 对照：`pkg/ddl/modify_column.go` 的 `getModifyColumnType`、`onModifyColumn`、rollback helpers、`doModifyColumnTypeWithData`、`doReorgWorkForModifyColumn`、`noReorgDataStrict`；广泛回归入口为 `pkg/ddl/modify_column_test.go` 等。
- Rust 直接测试：`pkg/session/runtime/normal_ddl_masking_policy_test.rs::{modify_column_persistent_syncs_nonempty_masking_policy,modify_column_persistent_missing_policy_table_rolls_back,modify_column_persistent_reorg_final_syncs_new_column_id,modify_column_persistent_reorg_advances_real_rows,modify_column_check_rejects_null_and_clears_temporary_flags,modify_column_reorg_rollback_removes_changing_column}`。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；交付验证仅检查固定十一节、文件范围和差异。上述测试是读取到的行为证据，不代表本轮重新执行结果。

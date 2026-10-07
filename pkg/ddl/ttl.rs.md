# `pkg/ddl/ttl.rs`

## 文件定位

`pkg/ddl/ttl.rs` 属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml`），由 `pkg/ddl/lib.rs` 以 `pub mod ttl` 对外暴露。它位于 SQL 表选项与 DDL 元数据更新之间，负责 TTL（Time To Live）配置的聚合、局部合法性检查和变更合并；它不执行过期行扫描或删除，真正的 TTL 调度与工作器在其他模块中。

本文件同时存在两层 API。`TtlTable`、`TtlInfo`、`TtlOption` 等是便于独立校验的精简模型；`apply_model_ttl_change` 则直接操作 `astersql_meta_model::TableInfo`，已接入实际 ALTER TABLE TTL 路径。RustCodeGraph 显示生产调用包括 `pkg/ddl/persistent_actions.rs::alter_ttl`、`pkg/domain/domain.rs::ddl_alter_table_ttl`，建表和会话入口还调用 `check_ttl_job_interval_for_ddl`。因此不能把所有精简模型函数都视为已接入完整 DDL 主链。

从 DDL 生命周期看，TTL 配置变更是保持表为 `Public` 的元数据更新，不需要 delete-only/write-only/reorg 状态迁移，也不做 backfill。持久化、schema version 发布、作业完成及外部 TTL workload 同步由调用方完成，而非本文件完成（`pkg/ddl/persistent_actions.rs::alter_ttl`、`pkg/domain/domain.rs::ddl_alter_table_ttl`）。

## 核心职责

1. 定义精简 TTL 校验模型：列类型、列定义、表配置、TTL 选项及领域错误（`ColumnType`、`ColumnDefinition`、`TtlInfo`、`TtlTable`、`TtlOption`、`TtlError`）。
2. 在精简模型上设置、移除和校验 TTL：`remove_ttl_info`、`change_ttl_info`、`validate_ttl_info`、`check_drop_column_with_ttl`。
3. 校验 TTL 作业周期：`check_ttl_job_interval` 处理 Starter 部署约束，`validate_job_interval` 处理本地的“正整数 + s/m/h/d”格式，`check_ttl_job_interval_for_ddl` 将 Starter 错误映射为兼容的数据库错误文本。
4. 聚合建表/改表选项：`get_ttl_info_in_options` 将 `TTL`、`TTL_ENABLE`、`TTL_JOB_INTERVAL` 合并，并应用部署模式相关默认值。
5. 合并真实表元数据变更：`apply_model_ttl_change` 模拟 Go `onTTLInfoChange` 的保留/覆盖规则，并阻止在非 TTL 表上单独设置开关或周期。

它不负责 Go `checkTTLInfoValid` 的完整职责：例如被其他表外键引用的查询、实际 SQL 间隔表达式求值、完整 `TableInfo` 列类型查找均不在 `apply_model_ttl_change` 中完成。精简模型中的 `cached`、`foreign_key_columns` 字段也没有被 `validate_ttl_info` 读取。

## 主要符号

- `DEFAULT_TTL_JOB_INTERVAL: &str = "1h"`：精简模型在非 Starter 模式下创建 TTL 定义时使用的默认调度周期；真实模型另有 `astersql_meta_model::DefaultTTLJobInterval`。
- `ColumnType`：精简列类型枚举。TTL 时间列只接受 `Date`、`DateTime`、`Timestamp`；`Float`、`Double` 还用于 common-handle 主键限制。
- `ColumnDefinition`：精简列名和列类型。
- `TtlInfo`：精简 TTL 配置，包含列名、整数间隔、单位、启用状态和作业周期。
- `TtlTable`：精简表视图。`ttl` 是当前配置；`temporary`、`common_handle`、`primary_key_columns` 参与校验；`cached` 和 `foreign_key_columns` 当前仅保留建模信息。
- `TtlOption`：选项流，允许多个 Definition/Enable/JobInterval；遍历时后值覆盖前值。
- `TtlError`：精简错误集合。`CachedTable`、`ForeignKey` 当前无产生路径；`ColumnUsedByTtl` 仅由 `check_drop_column_with_ttl` 产生。
- `remove_ttl_info(&mut TtlTable)`：无校验地将精简表的 `ttl` 设为 `None`。
- `change_ttl_info(&mut TtlTable, Option<TtlInfo>)`：新值非空时先调用 `validate_ttl_info`，成功后才替换；传入 `None` 等价于清除。
- `validate_ttl_info(&TtlTable, &TtlInfo)`：按固定顺序检查临时表、列存在性/类型、正间隔、Starter 周期、周期格式和 common-handle 浮点主键。
- `check_ttl_job_interval(&str)`：只在 `astersql_config_deploymode::IsStarter()` 为真时限制值必须等于 `StarterDefaultTTLJobInterval`。
- `check_ttl_job_interval_for_ddl(&str)`：把上一个函数的错误转换为 `ErrUnsupportedTTLJobIntervalInStarter` 的字符串，供 DDL/会话边界保留 Go 错误模板。
- `validate_job_interval(&str)`：接受去除首尾空白后的正整数与单个单位 `s|m|h|d`；拒绝零、缺单位、未知单位、负数和非数字前缀。
- `check_drop_column_with_ttl(&TtlTable, &str)`：大小写不敏感地禁止删除当前 TTL 时间列。
- `get_ttl_info_in_options(&[TtlOption])`：返回三元组 `(聚合后的定义, 最后开关, 最后周期)`；只有存在 Definition 时才将开关/周期写入定义并执行 Starter 周期检查。
- `apply_model_ttl_change(&mut TableInfo, Option<TTLInfo>, Option<bool>, Option<String>)`：实际模型合并入口；新定义未显式给出开关或周期时继承旧配置，随后应用显式覆盖。

## 执行流程

精简配置变更流程由 `change_ttl_info` 驱动：若目标是删除则直接清空；若目标是新增/替换，则先调用 `validate_ttl_info`。校验依次拒绝临时表，大小写不敏感地查找时间列，限制列类型，要求过期间隔为正且单位非空，检查 Starter 固定周期，再检查简化周期语法，最后检查 common handle 的任一主键列是否为 Float/Double。任何一步失败都在写入前返回，原 `table.ttl` 不变。

选项聚合由 `get_ttl_info_in_options` 顺序扫描输入。Definition 建立一份启用的配置，并根据部署模式设置默认周期；后续或先前出现的 Enable/JobInterval 都以各自最后一次出现的值为准。扫描结束后，仅当 Definition 存在时把两个可选覆盖写回配置。若只有 JobInterval 而没有 Definition，函数有意保留字符串且不做格式/Starter 校验，供后续 ALTER 现有 TTL 表处理；`pkg/ddl/ttl_test.rs::option_aggregation_defers_job_interval_validation_like_go` 固定了这一行为。

实际 ALTER 流程中，`pkg/session/runtime/ddl.rs` 解析 TTL 表选项，并先用 `check_ttl_job_interval_for_ddl` 检查显式周期；持久化动作模式提交类型 65 的 DDL job，最终由 `pkg/ddl/persistent_actions.rs::alter_ttl` 解码参数并调用 `apply_model_ttl_change`。该函数先校验新定义及显式周期的 Starter 限制；如果给了新定义，则在未显式提供 Enable/Interval 时继承旧值，再替换 `TableInfo.TTLInfo`；如果只给开关或周期但表没有 TTL 定义，则返回 `ErrSetTTLOptionForNonTTLTable`；最后应用显式值。

`alter_ttl` 在本文件返回成功后才通过事务更新表和 schema version；随后根据最终 `TTLInfo.Enable` 注册或删除外部 TTL workload 项，完成 job 并保持 `SchemaState::Public`。非持久化兼容路径由 `pkg/domain/domain.rs::ddl_alter_table_ttl` 调用同一合并函数，在元数据存储闭包中同步外部 workload 并发布变更。

建表路径 `pkg/ddl/create_table.rs` 自行构造标准 `TTLInfo`，只复用 `check_ttl_job_interval_for_ddl`；它没有调用精简的 `get_ttl_info_in_options`。这说明后者当前主要由独立 Rust 测试覆盖，并非标准模型建表的唯一实现。

## 数据与状态

精简模型的数据完全由调用者持有，没有数据库句柄。`change_ttl_info` 和 `remove_ttl_info` 原地修改 `TtlTable.ttl`；`apply_model_ttl_change` 原地修改标准 `TableInfo.TTLInfo`。两类模型名称相似但字段表示不同：精简模型用 `i64 interval_expression + String interval_unit`，标准模型用 `IntervalExprStr + IntervalTimeUnit`，不能直接互换。

选项合并有三个独立状态：是否提供 Definition、最后一次 Enable、最后一次 JobInterval。返回三元组保留这种“是否显式给出”的信息，使调用者能区分默认值、继承旧值和用户覆盖。`apply_model_ttl_change` 同样用 `Option` 表达未指定；新定义存在且旧定义存在时，未指定字段继承旧值，这是 ALTER TTL 的关键不变量。

部署模式是进程级外部状态。`check_ttl_job_interval` 和 `get_ttl_info_in_options` 每次调用都会读取 `astersql_config_deploymode`；Starter 默认值来自 `astersql_meta_model::StarterDefaultTTLJobInterval`，当前测试期望为 `15m`。常规默认值为 `1h`。

本文件不持久化 schema version、job 状态或 TTL 清理进度。标准模型变更进入调用方事务后，`persistent_actions::alter_ttl` 才设置表元数据、更新版本、同步 workload，并把 job 从运行状态推进到 `Done/Public`。

## 依赖与调用关系

直接依赖很少：标准库 `BTreeSet`；`astersql-config-deploymode` 读取部署模式；`astersql-meta-model` 提供标准 `TableInfo`/`TTLInfo` 和 Starter 默认周期；`astersql-util-dbterror` 提供兼容错误。前三个 AsterSQL crate 均在 `pkg/ddl/Cargo.toml` 的常规依赖中声明。

RustCodeGraph 的关键边如下：

- `change_ttl_info → validate_ttl_info`。
- `validate_ttl_info → check_ttl_job_interval`、`validate_job_interval`。
- `check_ttl_job_interval_for_ddl → check_ttl_job_interval`。
- `get_ttl_info_in_options → check_ttl_job_interval`（仅 Definition 与显式周期同时存在时）。
- `pkg/ddl/persistent_actions.rs::alter_ttl → apply_model_ttl_change → check_ttl_job_interval_for_ddl`。
- `pkg/session/runtime/ddl.rs` 与 `pkg/ddl/create_table.rs` → `check_ttl_job_interval_for_ddl`。
- `pkg/domain/domain.rs::ddl_alter_table_ttl → apply_model_ttl_change`。

上游 DDL job 路径还形成 `persistent_actions::step → alter_ttl → update_version_and_table`，并在成功后调用 `register_create_table_ttl` 或 `delete_drop_table_ttl`。因此本文件是配置合并/校验层，而事务、owner/worker 调度、schema 同步和外部 workload 生命周期属于调用方。

## 错误处理与边界

精简 API 使用 `Result<_, TtlError>`，`Display` 直接输出 Debug 形式，适合内部断言但不是稳定 SQL 错误协议。实际 DDL 边界使用 `Result<_, String>`，通过 `astersql_util_dbterror` 生成与 Go 对齐的错误文本。

`validate_ttl_info` 的错误优先级由代码顺序决定：临时表先于列错误，列不存在先于类型错误，TTL 间隔先于作业周期，周期先于主键限制。列名和主键列匹配不区分 ASCII 大小写。TTL 间隔只验证整数大于零和单位非空，无法表达或验证 Go 中任意 SQL interval expression；这属于精简模型的能力边界。

`validate_job_interval` 是本地简化语法校验，而 Go `checkTTLJobInterval` 只处理 Starter 模式限制，Go 的完整 TTL 有效性还用 `cache.EvalExpireTime` 验证过期表达式。当前标准模型入口 `apply_model_ttl_change` 仅校验 Starter 限制，不调用 `validate_job_interval`，因此文档或扩展代码不能假定所有生产 ALTER 路径都受“正整数 + s/m/h/d”约束。

`TtlError::CachedTable`、`ForeignKey` 当前没有返回点；`TtlTable.cached`、`foreign_key_columns` 当前不影响校验。Go 的真实语义是检查“其他表是否引用当前 TTL 表”，而不是检查当前表自己声明的外键列。`pkg/ddl/ttl_test.rs::ttl_validation_does_not_reject_unrelated_table_flags` 明确防止误加这两类拒绝逻辑。

`apply_model_ttl_change` 在修改标准模型前检查所有传入周期的 Starter 限制，所以该错误不会留下部分周期更新。对非 TTL 表单独设置 Enable/JobInterval 会返回标准错误；若同时提供 Definition，则先建立 TTL 配置，两个选项合法地作用于新定义。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、文件或网络资源，也不自行开启事务。所有修改都要求调用者持有独占的 `&mut` 引用，因此 Rust 类型系统阻止同一对象的并发可变访问。

需要关注的共享状态只有部署模式。测试 `starter_uses_fifteen_minute_default_and_rejects_other_intervals` 临时调用 `Set(Starter)`，并用 `RestoreDeployMode` 的 `Drop` 恢复原值，以限制进程级状态泄漏；新增测试若并行修改部署模式，应沿用仓库已有的串行化/恢复模式，避免互相影响。

真正的资源生命周期在上游：持久化动作在 `JobExecutionContext::with_transaction` 内更新表元数据和 schema version，事务成功后同步外部 TTL workload，失败则把 job 标记为 Cancelled，成功则 `finish_table_job(Done, Public, ...)`。本文件不拥有这些资源，返回错误即把回滚/取消责任交还调用方。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/ttl.go`，测试对照是 `pkg/ddl/ttl_test.go`。

- Rust `apply_model_ttl_change` 对应 Go `onTTLInfoChange` 中 TTLInfo/Enable/JobInterval 的合并部分：新定义继承旧开关与周期、非 TTL 表拒绝独立选项、显式选项最终覆盖。Go 函数还负责取表、持久化版本、同步外部 workload 和结束 job；Rust 将这些职责放在 `persistent_actions::alter_ttl` 或 domain 路径。
- Rust `check_ttl_job_interval` / `check_ttl_job_interval_for_ddl` 对应 Go `checkTTLJobInterval`，Starter 仅允许 `StarterDefaultTTLJobInterval`。Rust 精简错误与 DDL 错误映射分成两层。
- Rust `get_ttl_info_in_options` 对应 Go 同名函数的聚合顺序、默认值和覆盖规则。Rust 精简 Definition 只能携带整数表达式，而 Go 会 Restore AST 表达式文本并保存枚举时间单位。
- Rust `validate_ttl_info` 汇集了 Go `checkTTLInfoValid`、`checkTTLInfoColumnType`、`checkPrimaryKeyForTTLTable` 的一部分意图，但不是完整移植：Go 会求值过期表达式、读取完整列元数据并可查询被引用外键；Rust 精简函数没有 InfoSchema，也不使用 cached/foreign-key 字段。
- Rust `check_drop_column_with_ttl` 对应 Go `checkDropColumnWithTTLConfig`，都阻止删除 TTL 列；Rust 使用 ASCII 大小写不敏感比较，Go 比较规范化小写名。
- Rust `remove_ttl_info` 表达 Go `onTTLInfoRemove` 的内存变更，但没有 Go 的表读取、版本更新、外部 workload 删除和 job 完成步骤。

Rust 测试 `get_ttl_info_in_options_matches_go_cases` 复现 Go `Test_getTTLInfoInOptions` 的主要用例；Starter 默认和拒绝行为对应 Go `TestGetTTLInfoInOptionsStarterDefault`、`TestCheckTTLJobIntervalInStarter`。Rust 还增加了 option-only 延迟校验与 common-handle 浮点主键边界，记录当前迁移语义。

## 扩展指南

新增 TTL 表选项时，应同时评估 `TtlOption`、`get_ttl_info_in_options`、标准模型解析路径 `pkg/ddl/create_table.rs` 和 `pkg/session/runtime/ddl.rs`，以及参数持久化/解码。只改精简聚合器不会自动改变真实 SQL 行为。

修改 ALTER 合并规则时，核心落点是 `apply_model_ttl_change`，并需同步 `pkg/ddl/persistent_actions.rs::alter_ttl` 与 `pkg/domain/domain.rs::ddl_alter_table_ttl` 两条调用路径的语义。必须维持未显式选项继承旧值、非 TTL 表拒绝独立选项、错误发生时不留下部分更新等不变量。

扩展完整合法性校验时，要先决定逻辑属于精简模型还是标准 `TableInfo` 路径。涉及 SQL 表达式、InfoSchema 外键反向引用或真实 MySQL 类型时，不应继续堆叠无法表达上下文的布尔字段，而应接入标准模型与调用方上下文。性能风险主要来自未来加入的 catalog/InfoSchema 查询；当前函数均为列数/主键列数线性扫描。

测试必须放在独立文件 `pkg/ddl/ttl_test.rs`，不要内嵌到 `ttl.rs`。涉及真实 SQL/DDL job 或外部 workload 的行为还应扩展现有 Go/Rust 集成测试入口，而不只验证精简模型。若修改 Starter 全局模式测试，务必恢复全局状态；若增加生产错误，应优先使用 `astersql-util-dbterror` 保持兼容错误码与消息。

## 验证依据

- 源文件：`pkg/ddl/ttl.rs`，逐段核对 336 行内全部常量、枚举、结构体、trait 实现和 9 个公开函数；文件无条件编译项。
- crate 与模块：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`，确认 crate 名为 `astersql-ddl`、`ttl` 是公开模块，相关依赖已声明，测试以独立 `mod ttl_test` 组织。
- RustCodeGraph：索引状态为 11,467 files / 307,296 nodes / 1,848,419 edges；查询了 `apply_model_ttl_change`、`get_ttl_info_in_options`、`validate_ttl_info`、`check_ttl_job_interval_for_ddl` 的 node/callers/callees，并读取了 `persistent_actions::alter_ttl` 及直接调用片段。
- 生产调用证据：`pkg/ddl/persistent_actions.rs::alter_ttl`、`pkg/ddl/create_table.rs`、`pkg/session/runtime/ddl.rs`、`pkg/domain/domain.rs::ddl_alter_table_ttl`。
- Rust 测试：`pkg/ddl/ttl_test.rs`，覆盖 Go 对齐的选项聚合、无 Definition 时延迟周期校验、Starter 默认/拒绝、common-handle 主键以及无关表标志不应被拒绝。
- Go 对照：`pkg/ddl/ttl.go` 的 `onTTLInfoRemove`、`onTTLInfoChange`、`checkTTLInfoValid`、`checkTTLJobInterval`、`checkDropColumnWithTTLConfig`、`checkPrimaryKeyForTTLTable`、`getTTLInfoInOptions`；`pkg/ddl/ttl_test.go` 的对应单元测试。
- 结构校验采用任务指定命令，验证目标文件存在且恰好包含 11 个固定二级标题。本任务是纯文档分析，按计划不运行 Cargo，也不据此声称运行时测试通过。

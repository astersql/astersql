# `pkg/ddl/index.rs`

## 文件定位

`pkg/ddl/index.rs` 属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod index` 公开。它位于 DDL 的索引元数据与回填初始化层：一端接受会话层传来的 FULLTEXT 约束或 owner worker 已持有的 DDL Job，另一端修改 `astersql_meta_model` 中的表、索引和 reorg 元数据。

该文件不是完整的 ADD/DROP INDEX job 状态机。持久化 job 的动作分派位于 `pkg/ddl/persistent_actions.rs`，分区 reorg 推进也由 `pkg/ddl/partition.rs` 驱动；Go 版本中大量 worker、回填扫描、事务写入和分布式任务执行逻辑仍在 `pkg/ddl/index.go`。本文件还保留一套小写字段的简化 `TableInfo`/`IndexInfo`，用于移植索引校验、元数据变换和独立 Rust 测试；它与真实的 `astersql_meta_model::{TableInfo, IndexInfo}` 是两套不同类型，不能混用。

按 DDL 框架分类：`BuildCanonicalFullTextIndex` 是当前 session runtime 使用的元数据快速路径，直接把新索引置为 `StatePublic`；`init_for_reorg_indexes` 则是 job-based ADD INDEX/ADD PRIMARY KEY/MODIFY COLUMN 的 owner 本地初始化步骤，会选择并持久化回填类型，但不执行回填、不推进 schema state，也不发布索引。

## 核心职责

1. 在真实元数据上实现兼容策略：`set_global_index_version` 根据集群能力、GLOBAL/UNIQUE、聚簇句柄和列可空性选择全局索引磁盘格式；`BuildCanonicalFullTextIndex` 为 Starter 部署校验并追加规范 FULLTEXT 元数据。
2. 在简化元数据上实现索引定义校验和变换：列数/重复列/前缀长度/键长、生成列主键、特殊列存索引、重命名、可见性、删除及隐藏列清理。
3. 提供回填策略的纯决策辅助：ANALYZE 状态决策、可重试错误分类、任务键、并发度、行大小与分区游标。
4. 初始化真实 reorg job：加载 cloud storage URI，保持已启动 job 的 `ReorgTp` 不变，在 ingest、txn-merge、txn 间选择，拒绝无 fast reorg 的 partial index，并设置 telemetry 与 `BackfillStateRunning`。
5. 处理 owner 切换后仅存在于 owner 内存中的 cloud URI 恢复，并再导出 ingest 环境与 DXF URI 解析入口。

## 主要符号

- `set_global_index_version(&model::TableInfo, &mut model::IndexInfo)`：先无条件清零旧版本；仅当 V1 能力开启、索引为 GLOBAL、表非 clustered，且键需要携带 partition ID 时设为 `GlobalIndexVersionV1`。非唯一索引总是需要；唯一索引仅在命中可空列或 `PreventNullInsertFlag` 过渡列时需要。
- `BuildCanonicalFullTextIndex(&mut model::TableInfo, &ast::Constraint)`：真实 AST/元数据入口。要求 FULLTEXT、Starter、恰好一个完整升序普通列、字符串求值类型、名称不重复和合法 parser；分配 `MaxIndexID`，构造 `FullTextInfo`，设置 `MultipleKeyFlag` 并追加至 `TableInfo.Indices`。
- 简化数据模型：`ColumnType`、`ColumnInfo`、`IndexColumn`、`ColumnarIndexType`、`IndexKind`、`IndexInfo`、`TableInfo`、`IndexOptions`、`IndexError`。字段只覆盖本文件算法所需信息，不等价于完整元数据结构。
- `build_index_columns`、`check_primary_key_on_generated_column`、`check_index_prefix_length`、`get_index_column_length`：实现最多 16 个键列、大小写不敏感查找/去重、前缀合法性、3072 字节键长计算等规则。列存索引返回最小非零长度 1，避免后续大小/并发计算被零值破坏。
- `build_index_info` / `build_index_info_for_deploy_mode`：简化索引构造总入口。后者接收 Starter 能力，校验特殊索引与条件，单调分配 ID，维护列引用计数并写入 `table.indices`；前者固定以非 Starter 调用。
- `validate_special_index`：Vector 必须单列且类型为 Vector；Inverted 至少一列且仅接受列举的标量/时间类型；FullText 必须单个 CHAR/VARCHAR/TEXT 列。
- `validate_rename_index`、`rename_index`、`set_index_visibility`、`remove_index_info`、`remove_dependent_hidden_columns`：完成大小写兼容的名称变换、主键可见性约束、引用计数维护，以及删隐藏列后的全部索引 offset 修复。
- `AnalyzeStatus` / `analyze_status_decision`、`ReorgType` / `pick_backfill_type`、`JobErrorKind` / `is_retryable_job_error`：简化的状态和策略函数；其中 job 错误阈值固定为“下一次错误使计数达到 5 时停止”。
- `TaskKeyBuilder` / `task_key`：生成 `ddl/backfill/<job-id>[/<multi-schema-seq>][/merge]`；只保留非负 multi-schema 序号。
- `find_next_partition_id`、`find_next_non_touched_partition_id`、`next_non_touched_partition_id`、`next_recreated_index_partition_id`：推进普通、跳过 dropping、或优先沿 adding definitions 推进的分区游标；真实元数据版本以 0 表示结束/未找到。
- `index_column_slice_equal`、`find_related_indexes_to_change`、`rename_expression_index_columns`、`check_and_build_index_condition_string`、`build_index_condition_checker`：修复/modify-column/表达式索引/partial index 的简化辅助。行级 checker 当前只把条件字符串当作列名并判断可空整数值 `> 0`，不是 SQL 表达式求值器。
- `ReorgIndexEnvironment`：owner 本地服务边界，抽象 cloud URI 加载、加载后 hook、ingest 初始化状态与磁盘预检。
- `pick_job_backfill_type`、`init_for_reorg_indexes`：操作真实 `group_3::Job` 和真实 `model::IndexInfo` 的 reorg 初始化入口。
- `resolve_cloud_storage_uri_after_owner_failover`：新 owner 在 cloud job 恢复时填充内存缓存；merge 阶段、本地模式或已有缓存直接旁路，cloud 已启用但配置为空时返回带 job ID 的错误。
- 再导出：`init_global_lightning_env`、`initialized_disk_root`、`replace_global_lightning_env_for_test` 来自 ingest 环境，`resolve_cloud_storage_uri` 来自 DXF handle。

## 执行流程

FULLTEXT 快速路径如下：`pkg/session/fts_runtime.rs::alter_table` 识别 `ALTER TABLE ... ADD FULLTEXT`，调用 `BuildCanonicalFullTextIndex`；函数依次检查约束种类与部署模式、键列形态、列存在性与字符串类型、索引名和 parser，随后递增 `MaxIndexID`、构造 Public `IndexInfo`、修改列 flag 并写入 `Indices`。任一校验失败都在写元数据前返回；ID 分配发生在全部校验之后。

简化索引构造路径为 `build_index_info → build_index_info_for_deploy_mode → build_index_columns → check_index_column/get_index_column_length → validate_special_index → allocate_index_id → add_index_column_flag → table.indices.push`。主键先检查非 STORED generated column 和 invisible 冲突；GLOBAL 仅在 `table.partitioned` 时生效。删除路径反向执行：定位并移除索引，递减列引用，然后删除该索引涉及的隐藏列，最后按列名重算剩余索引的 offset。

真实 reorg 初始化路径为 `persistent_actions::initialize_reorg_indexes → index::init_for_reorg_indexes`。空索引列表立即返回且不接触环境；否则先加载 URI，并以“URI 非空且 `IsDistReorg`”设置 `UseCloudStorage`，调用 hook 后进入 `pick_job_backfill_type`。已有非 None 类型绝不改变；非 fast reorg 选 Txn；fast 且 ingest 环境可用选 Ingest（仅非 cloud 时做磁盘预检）；否则选 TxnMerge。Txn/TxnMerge 遇到 partial index 返回错误；需要 merge process 的类型增加 telemetry 并把每个真实索引置为 `BackfillStateRunning`。

分区 reorg 的生产调用在 `pkg/ddl/partition.rs`：一个物理分区完成后调用 `next_recreated_index_partition_id`。若当前 ID 位于 `AddingDefinitions`，继续该数组；否则转到 canonical definitions 并跳过 `DroppingDefinitions`，返回 0 表示结束。

owner failover URI 恢复只在 `use_cloud_storage && !merge_temp_index && cached_uri.is_empty()` 时读取配置。读取到空值会失败而不是静默降级为本地排序；成功后同时返回并更新 owner 本地缓存。

## 数据与状态

真实持久状态包括 `TableInfo.MaxIndexID/Indices/Columns[*].Flag`、`IndexInfo.GlobalIndexVersion/BackfillState/ConditionExprString`，以及 `Job.reorg_meta` 中的 `UseCloudStorage`、`IsDistReorg`、`IsFastReorg` 和 `ReorgTp`。`pick_job_backfill_type` 的关键不变量是：一旦 `ReorgTp != ReorgTypeNone`，后续恢复不得重新选择后端。

`ReorgIndexEnvironment` 管理的是 owner 本地资源视图；cloud URI 缓存不随 job 持久化，因此 failover 时必须由 `resolve_cloud_storage_uri_after_owner_failover` 恢复。`init_for_reorg_indexes` 先确定 cloud 状态再决定是否探测本地磁盘，保证 cloud job 不触碰本地 ingest 目录。

简化 `TableInfo.max_index_id` 单调递增；`ColumnInfo.index_flags` 是引用计数并用饱和减法防止下溢；索引和列名比较普遍采用 ASCII 大小写不敏感规则。`BTreeSet` 用于确定性去重/隐藏列集合，`BTreeMap` 用于删除列后的 offset 重建。任务键的标签顺序是稳定协议，multi-schema 标签位于 job ID 与 `merge` 之间。

该文件没有锁、channel 或事务对象。它会修改调用方传入的 `&mut` 元数据；错误发生前后的原子性依具体函数而异。例如 `BuildCanonicalFullTextIndex` 在变更前完成校验，而 `init_for_reorg_indexes` 可能已写入 `UseCloudStorage`/`ReorgTp` 后才因 partial index 拒绝，这与 Go 的初始化顺序一致。

## 依赖与调用关系

上游生产调用：

- `pkg/session/fts_runtime.rs::alter_table` 调用 `BuildCanonicalFullTextIndex`。
- `pkg/ddl/persistent_actions.rs::initialize_reorg_indexes` 限定 action 为 ADD INDEX、ADD PRIMARY KEY 或 MODIFY COLUMN 后，取得 `JobExecutionContext::reorg_index_environment()` 并调用 `init_for_reorg_indexes`。
- `pkg/ddl/partition.rs` 调用 `next_recreated_index_partition_id` 推进 recreated-index reorg。

RustCodeGraph 的 `node init_for_reorg_indexes` 明确给出 caller `persistent_actions.rs::initialize_reorg_indexes`，并给出 callees `load_cloud_storage_uri`、`after_load_cloud_storage_uri`、`pick_job_backfill_type`；`node resolve_cloud_storage_uri_after_owner_failover` 当前只发现两个 `index_nokit_test.rs` 测试 caller。对 `BuildCanonicalFullTextIndex` 的图节点能定位定义及构造的真实元数据类型，但未列出 session caller，因此用仓库搜索核实了 `fts_runtime.rs` 的 import 与调用。

主要下游 crate 依赖均由 `pkg/ddl/Cargo.toml` 声明：`astersql-meta-model`、`astersql-parser`/`ast`/`mysql` 提供真实元数据、AST、错误和 flag；`astersql-config-deploymode` 提供 Starter gate；`astersql-util-logutil` 记录异常分区游标；`astersql-util-dbterror` 生成 partial-index 兼容错误；`astersql-metrics` 记录 ingest telemetry；`astersql-ddl-ingest` 与 `astersql-dxf-framework-handle` 提供再导出资源接口。

简化 API 的直接使用主要位于 `pkg/ddl/index_test.rs`、`db_change_test.rs` 和 `db_integration_test.rs`；它们目前没有被生产 ADD INDEX worker 用来替代完整 model/AST 流程。扩展时必须先确认目标属于哪套类型体系。

## 错误处理与边界

`BuildCanonicalFullTextIndex` 返回 parser `Error`，覆盖非 FULLTEXT、非 Starter、键列数量/方向/前缀、表达式键、列不存在、非字符串、重名和 parser 非法。它不会在该函数内创建持久 DDL job，也没有 rollback 状态机；当前调用路径把错误映射成 `SessionError`。

简化构造使用可比较的 `IndexError`：列不存在、键列超过 16、重复列、非法生成列主键、前缀错误、BLOB/TEXT 缺前缀、键长超过上限、重名、主键不可见、特殊索引/partial condition 非法等。`get_index_column_length` 使用饱和乘法；负数输入仅出现在行大小估算中并被钳为 0，正溢出转换则回退 `usize::MAX`。

`init_for_reorg_indexes` 使用 `Result<(), String>` 对接现有 job worker 边界：缺失 reorg meta、URI 加载、磁盘预检、telemetry 初始化和 unsupported partial index 均向上传播。空索引列表是严格 no-op。partial index 的拒绝发生在后端选择之后，因此 job 中选出的 `ReorgTp` 会保留，但不会递增 telemetry 或设置 BackfillState。

`resolve_cloud_storage_uri_after_owner_failover` 保证 cloud-enabled job 不因新 owner 缺少 URI 而错误降级；错误消息包含 job ID。merge temporary index 明确旁路 cloud。`next_non_touched_partition_id` 遇到未知 current ID 只告警并返回 0，与 Go 的 warning-only fallback 对齐。

简化 `build_index_condition_checker` 是明确边界：它不解析 `check_and_build_index_condition_string` 返回的一般 SQL 文本，只支持把文本当作 map key 的 `> 0` 判断。不能据此声称 Rust 已实现 Go `checkIndexCondition` 的完整 AST 类型检查和表达式执行。

## 并发与资源生命周期

文件自身为同步函数集合，不创建线程、异步任务、锁、channel、事务或 worker。并发资源由外层 owner/job worker、ingest 和 DXF 框架持有；`adjust_concurrency` 只计算 `min(worker_count, available_slots)`，不会创建 worker，也不会把 0 自动提升为 1。

reorg 生命周期边界由 `ReorgIndexEnvironment` 显式分离：URI 加载和磁盘探测由 owner execution context 实现；job 仅持久化 `UseCloudStorage` 与 `ReorgTp`，URI 本身留在 owner 本地缓存。owner failover 后，缓存为空才重载配置；已有缓存优先，避免运行中配置变化改写 job 的资源位置。

索引元数据的借用生命周期由 `&mut` 保证单次调用期间独占修改。该文件不负责跨节点 schema version 同步、MDL、lease、job checkpoint、delete-range GC 或最终 history 搬迁；这些均是外层 DDL 生命周期职责。telemetry 是进程级全局资源，测试以互斥锁串行观察计数差值，生产函数本身只调用线程安全 counter 的 `inc()`。

## 与 Go 版本的对应关系

主要一一对应关系位于 `pkg/ddl/index.go`：Rust `set_global_index_version` 对应 Go `setGlobalIndexVersion`；`calc_bytes_length_for_decimal` 对应 `calcBytesLengthForDecimal`；简化构造覆盖 `buildIndexColumns`、`CheckPKOnGeneratedColumn`、`checkIndexPrefixLength`、`getIndexColumnLength`、`BuildIndexInfo` 的核心规则；元数据变换对应 `AddIndexColumnFlag`、`DropIndexColumnFlag`、`ValidateRenameIndex`、`setIndexVisibility`、`removeIndexInfo`、`RemoveDependentHiddenColumns`。

真实 reorg 路径直接对齐 Go `initForReorgIndexes`、`pickBackfillType` 和 `loadCloudStorageURI`：空列表提前返回、先加载 cloud、已启动类型不可改变、非 fast 选 txn、可用 ingest 环境选 ingest、否则 txn-merge、partial index 限制、telemetry 与 BackfillState 的顺序均由 Rust 测试覆盖。`TaskKeyBuilder` 对齐标签顺序，但 Go NextGen 模式还会在最前面添加 keyspace 名；简化 Rust builder 没有该分支，不能当作所有部署形态的完整替代。

分区辅助对应 Go `findNextPartitionID` / `findNextNonTouchedPartitionID` / recreated-index 分支；索引比较和修改列辅助对应 `indexColumnSliceEqual`、`FindRelatedIndexesToChange`、`RenameExpressionIndexColumns`。Rust 的 `index_column_slice_equal` 与 Go repair-table 语义一样只比较规范化名称，不比较 offset 和 prefix length。

差异必须保留可见：Go `index.go` 还包含完整 worker schema-state 迁移、回填扫描与写入、唯一键冲突处理、分布式任务提交/暂停/调参、ANALYZE 执行和复杂 partial-index AST 校验；这些不在当前 Rust 文件中。简化 Rust `JobErrorKind` 用枚举和固定阈值近似 Go 的错误码/消息集合与可配置 `DDLErrorCountLimit`，`build_index_condition_checker` 也只是测试用最小行判断。`BuildCanonicalFullTextIndex` 则是 Rust session runtime 的规范快速路径，并非 Go `onCreateIndex` job 状态机的完整复刻。

## 扩展指南

若新增真实 SQL/DDL 能力，优先沿 `astersql_meta_model` 路径扩展，确认它是 metadata-only fast path 还是必须进入持久 job；涉及 schema state、backfill、owner failover 或 rollback 时，应在 `persistent_actions.rs`/worker 层接线，不能只向简化 `TableInfo` 写字段。新增 FULLTEXT 校验应同步 `BuildCanonicalFullTextIndex` 与 session caller，并在独立测试文件覆盖部署 gate、parser、列形态和无副作用失败。

若改变 reorg 后端选择，修改 `pick_job_backfill_type` 和 `init_for_reorg_indexes`，保持“已启动类型不可变”“cloud 不探测本地磁盘”“partial index 检查顺序”三个兼容不变量；同步 `pkg/ddl/backfilling_test.rs`。若改变 failover URI 行为，同步 `pkg/ddl/index_nokit_test.rs`，尤其验证缓存优先、空配置报错、本地与 merge 旁路。

若改变全局索引磁盘格式，必须同步 `pkg/ddl/tests/partition/global_index_version_test.rs`，验证能力开关、GLOBAL→LOCAL 清零、unique/nullability、clustered handle 和 metadata clone；该修改具有磁盘格式与跨版本兼容风险。

若扩展简化构造规则，修改最接近的 `build_index_columns`、`get_index_column_length`、`validate_special_index` 或 `build_index_info_for_deploy_mode`，测试应放在独立的 `pkg/ddl/index_test.rs`、`db_change_test.rs` 或 `db_integration_test.rs`，不要内嵌到生产文件。partial index 若要支持一般 SQL 条件，不能继续扩张当前字符串 checker；应接入 AST、类型/列可见性校验和真正的表达式执行上下文，并评估与 Go `checkIndexCondition` 的语义一致性。

性能风险集中在回填后端、并发度、键长和分区游标；兼容风险集中在任务键、`ReorgTp` 持久值、GlobalIndexVersion 和错误分类。任何更改都应先搜索 Go 对应函数与现有 Rust production caller，避免把测试模型误接到真实 job 主链。

## 验证依据

- 源码：`pkg/ddl/index.rs`（1,239 行），逐段核对全部公开/私有函数、类型、trait、常量与再导出；文件无条件编译项。
- crate 与模块：`pkg/ddl/Cargo.toml` 的 package/lib/dependencies/porting metadata；`pkg/ddl/lib.rs` 的 `pub mod index` 与独立 `mod index_test`。
- 上游/下游：`pkg/session/fts_runtime.rs::alter_table`、`pkg/ddl/persistent_actions.rs::initialize_reorg_indexes`、`pkg/ddl/partition.rs` 的 recreated-index 调用。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点和 1,848,419 边；`files --filter pkg/ddl/index.rs` 显示目标文件 127 个符号；`query` 定位 `BuildCanonicalFullTextIndex`、`build_index_info`、`init_for_reorg_indexes`、`resolve_cloud_storage_uri_after_owner_failover`、`remove_index_info`、`pick_job_backfill_type`；`node` 验证关键源码、`init_for_reorg_indexes ← initialize_reorg_indexes` 以及其内部调用边。通用 `callers build_index_info` 查询长时间无输出后中止，因此其使用点另以仓库搜索核验，未据此推断不存在调用者。
- Rust 测试：`pkg/ddl/index_test.rs` 覆盖 decimal/时间长度、列存最小长度、generated PK、ANALYZE、重试阈值、列比较、任务键、并发、大小写重命名、特殊索引、partial condition 和隐藏列 offset；`pkg/ddl/backfilling_test.rs` 覆盖 cloud/磁盘顺序、已启动类型、fallback、空列表与 partial-index 错误；`pkg/ddl/index_nokit_test.rs` 覆盖 owner failover URI；`pkg/ddl/tests/partition/global_index_version_test.rs` 覆盖真实全局索引版本；`pkg/ddl/ddl_test.rs` 覆盖分区推进。另以 `db_change_test.rs`、`db_integration_test.rs` 核对简化元数据集成用法。
- Go 对照：`pkg/ddl/index.go` 的 `buildIndexColumns`、`getIndexColumnLength`、`setGlobalIndexVersion`、`BuildIndexInfo`、`initForReorgIndexes`、`pickBackfillType`、`isRetryableJobError`、`TaskKeyBuilder`、`adjustConcurrency`、分区推进、repair/modify-column/partial-index 辅助；相关 Go 测试包括 `pkg/ddl/backfilling_test.go`、`ddl_test.go`、`db_integration_test.go`、`index_modify_test.go` 和 `index_nokit_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认目标文件存在且恰有十一个规定的二级标题；人工复核重点是两套元数据模型、真实生产调用与尚未移植的 Go worker 逻辑均被明确区分。

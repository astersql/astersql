# `pkg/ddl/modify_column.rs`

## 文件定位

`modify_column.rs` 属于 `astersql-ddl` crate，并由 [`pkg/ddl/lib.rs`](lib.rs) 以 `pub mod modify_column` 公开。它用仓库内的简化 `column::{ColumnInfo, TableInfo, SchemaState, ...}` 模型表达 MODIFY/CHANGE COLUMN 的路径选择、前置校验和一次一步的在线状态推进，便于 Rust 侧对 Go 语义做独立、确定性的单元验证。

当前接线边界必须明确：代码搜索未发现生产模块调用本文件的主入口 `advance_modify_column`；直接调用者均是 `column_modify_test.rs`、`column_type_change_test.rs`、`db_change_test.rs`、`db_integration_test.rs` 等测试。真正接收持久化 DDL Job 的 Rust 生产链是 [`persistent_actions.rs`](persistent_actions.rs) 分派到 [`persistent_modify_column.rs`](persistent_modify_column.rs) 的 `step`。因此，本文件不是完整 SQL→Job→存储事务执行器，也不直接访问 `mysql.tidb_ddl_job`、重组表或 KV；它是公开的、可测试的 MODIFY COLUMN 语义模型与移植辅助实现。

[`pkg/ddl/Cargo.toml`](Cargo.toml) 将 crate 命名为 `astersql-ddl`，入口为 `lib.rs`，并以 `[package.metadata.porting].go-package = "pkg/ddl"` 声明 Go 对照包。本文件自身只直接依赖标准库 `BTreeSet` 以及同 crate 的 `column`、`generated_column` 模块，没有条件编译项、异步函数或外部 crate API。

## 核心职责

1. 选择物理修改路径：`get_modify_column_type` 在 `NoReorg`、`NoReorgWithCheck`、`IndexReorg`、`Reorg` 之间决策；`Precheck` 由 `advance_modify_column` 为 VARCHAR→CHAR 的特殊检查插入。
2. 验证修改是否合法：列名冲突、生成列/函数索引依赖、字符集与索引排序规则、分区键兼容性、AUTO_RANDOM 参数都在进入状态推进前被拒绝或放行。
3. 生成存量数据探测条件：`build_check_sql_from_modify_column` 只构造 `SELECT ... WHERE ... LIMIT 1`，调用者负责执行 SQL，并把结果折叠成 `data_is_valid` 传给状态机。
4. 模拟在线 DDL 双对象过程：重组路径创建隐藏 changing 列及临时索引，逐次推进 `None → DeleteOnly → WriteOnly → WriteReorganization → Public`，完成时把目标定义落回旧列 ID，回滚时清理临时对象。
5. 保持表元数据关联：最终写回会同步索引列和外键列名、移动列位置、清除在线修改标记，并增加由调用者持有的 schema version。

本文件不承担实际扫描、行值 cast、索引 backfill、checkpoint 持久化、owner failover、schema sync 等生产职责；这些能力在 Go `modify_column.go` 以及 Rust `persistent_modify_column.rs`/worker 框架中实现。

## 主要符号

- `ModifyColumnType`：物理路径枚举。`None` 是未选择，`Precheck` 是 VARCHAR→CHAR 的数据预检，另四项对应纯元数据、带检查的元数据、索引重组和行重组。
- `PartitionType`、`PartitionExpressionUsage`、`PartitionInfo`：简化分区元数据。表达式用法只区分裸列、`TO_DAYS`、`EXTRACT` 和其它不支持的函数上下文。
- `ModifyColumnArgs`：一次修改的输入和可恢复中间标识，包括旧列名/ID、目标列、位置、已选路径、changing 列/索引 ID、ENUM/SET 元素及 AUTO_RANDOM 位数。它是内存结构，并非生产 Job args 的序列化实现。
- `ModifyColumnContext`：路径判定所需的严格 SQL mode、分区摘要、关闭有损优化开关和 AUTO_RANDOM 上限/默认值。
- `ModifyColumnError`：本地错误域，覆盖列不存在/重名、类型与字符集限制、生成列依赖、分区限制、数据截断、非法 NULL、状态/位置和 AUTO_RANDOM 错误。`GeneratedColumnError` 保留为 `Generated`；`ColumnError` 被统一映射成 `InvalidPosition`。
- `no_reorg_data_strict`、`need_row_reorganization`、`need_index_reorganization`、`get_modify_column_type`：类型与索引层面的路径选择函数。
- `build_check_range_for_integer`、`build_check_sql_from_modify_column`：构造存量数据范围、尾空格/长度、NULL 探测 SQL；不执行查询。
- `collect_partition_expression_usage`、`check_partition_column_modifiable`：扫描分区表达式并实施按分区类型划分的扩大变更白名单。
- `check_auto_random`：检查 shard bits 只能保持或增加、不得越界，类型/default/auto_increment 必须兼容，range bits 不得改变。
- `advance_modify_column`：本文件的聚合入口，执行查列、校验、选路、回滚或单步状态推进，返回 `ModifyColumnOutcome`。
- `apply_modified_column`、`initialize_changing_objects`、`rollback_changing_objects`：私有元数据变换函数，分别完成最终写回、临时对象初始化和回滚清理。
- `convert_between_char_and_varchar`、`set_default_for_modified_column`：小型公共辅助函数；后者同时更新 `default_value` 与 `origin_default_value`。

## 执行流程

`advance_modify_column` 的单次调用流程如下：

1. 优先按已持久化的 `old_column_id` 查找旧列；ID 尚未回填时按旧列名或 `_tidb_removing_<name>` 查找，并把真实 ID 写回 `args`。
2. 运行共同前置检查：`check_column_already_exists` 防止改名冲突；`has_dependent_generated_column` 区分普通生成列和隐藏函数索引依赖；`check_modify_types` 检查生成列、GBK 和索引字符列 collation 限制；有分区上下文时再运行 `check_partition_column_modifiable`。
3. `args.modify_type == None` 时调用 `get_modify_column_type`。纯扩大优先走无重组；分区表、TiFlash 副本、关闭优化或非严格模式保守走 `Reorg`；剩余情形再按行编码和索引编码是否变化决策。VARCHAR→CHAR 且初选为检查/索引重组时先改成 `Precheck`。
4. 若 `rolling_back`，立即调用 `rollback_changing_objects`，删除记录在 args 中的临时列/索引，清除所有列的中间标记，schema version 加一并返回完成。
5. `Precheck` 要求 `data_is_valid`；失败返回 `DataTruncated`，成功后依据是否有关联索引转成 `IndexReorg` 或 `NoReorgWithCheck`。
6. `NoReorg`/`NoReorgWithCheck` 直接调用 `apply_modified_column`；带检查路径会把无效数据区分为 `InvalidNull` 或 `DataTruncated`。成功后 schema version 加一并完成。
7. `IndexReorg`/`Reorg` 共享本文件的简化 changing-object 状态机。首次调用创建隐藏 changing 列、克隆关联索引并持久在 `args` 的 ID 列表中；NULL→NOT NULL 同时给旧列设置 `prevent_null_insert`。
8. 每次调用只前进一个 schema state，并同步临时索引状态、增加 schema version。到达 `Public` 前返回 `finished = false`；到达 `Public` 后把目标定义写回旧 ID、删除临时对象、重算 offset 并返回完成。

注意：这里的 `IndexReorg` 与 `Reorg` 都只推进元数据状态，没有在两次调用之间执行真实 backfill；生产代码必须通过持久化 worker 的 `step` 和重组设施完成该工作。

## 数据与状态

- 稳定身份不变量：完成修改后的业务列继续使用旧列 ID；changing 列使用递增后的 `table.max_column_id`，changing 索引 ID 从现有最大索引 ID 之后分配。
- 幂等初始化：`initialize_changing_objects` 在 `changing_column_id` 已存在时直接返回，避免重试重复创建；恢复所需的临时对象 ID 保存在 `ModifyColumnArgs`。
- 命名策略：隐藏对象用 `_col$_<old>_<n>`、`_idx$_<old>_<n>`，从 0 递增直到大小写不敏感地避开已有名称。
- 双写窗口标记：changing 列记录 `change_dependency_offset = Some(old.offset)`；NULL→NOT NULL 时旧列设置 `prevent_null_insert`。完成或回滚会清除这些标记。
- 元数据同步：`apply_modified_column` 把状态设为 `Public`，同步引用旧列名的索引列元数据和外键列名，再按 `ColumnPosition` 移动列。
- 版本所有权：schema version 由调用者以 `&mut i64` 注入。本文件只在成功状态步、无重组完成或回滚清理时自增，不做全局发布或跨节点等待。
- `redundant_index_ids`、`new_shard_bits`、`new_range_bits` 在参数结构中保留了 Go Job 的迁移语义，但 `advance_modify_column` 当前没有消费这些字段；不能据此宣称本文件已完成冗余索引 GC 或 AUTO_RANDOM Job 接线。

## 依赖与调用关系

上游与接线：

- `lib.rs → pub mod modify_column` 暴露本模块。
- RustCodeGraph 的文件节点报告该文件被 26 个文件使用；精确 `callers advance_modify_column` 未返回调用边。文本级调用核验显示 `advance_modify_column` 仅由独立 Rust 测试调用，包括 `column_modify_test.rs`、`column_type_change_test.rs`、`db_change_test.rs` 和 `db_integration_test.rs`。
- 生产 DDL 分派不进入本文件：`persistent_actions.rs` 调用 `persistent_modify_column::step`；删除范围处理也读取 `persistent_modify_column::finished_range_ids`。

下游依赖：

- `column` 提供简化的表、列、索引、字段类型、位置、默认值和 schema state，以及 `locate_offset_to_move`、`update_index_column`。
- `generated_column::has_dependent_generated_column` 提供生成列依赖检查。
- 标准库 `BTreeSet` 让分区表达式用法去重且有稳定顺序。

Go 完整主链则为 executor 创建 `model.ActionModifyColumn` Job，owner worker 在 `onModifyColumn` 解码 args、选择路径并分派到无重组、索引重组或全量重组函数。该关系是 Go 对照证据，不是本文件现有 Rust 调用边。

## 错误处理与边界

- 名称比较多处大小写不敏感，但 `related_indices` 与部分索引/外键替换使用字符串相等；扩展名称规范时必须验证简化模型与 canonical `CIStr` 语义是否一致。
- `collect_partition_expression_usage` 是轻量字节扫描器，会跳过引号、追踪括号前函数名，但不是 SQL AST 解析器；嵌套在非 `to_days`/`extract` 函数内一律标成 `Unsupported`。Go 实现还会重建并解析分区定义进行最终校验，本文件没有该安全网。
- `check_modify_types` 只覆盖本模型列出的限制；`ModifyColumnError` 中 `PrimaryKey`、`ColumnarIndex`、`IndexPrefixTooLong` 等变体在本文件当前流程未构造，属于尚未接线的错误表达能力。
- 数据检查是职责分离的：空 SQL 表示无需探测；非空 SQL 必须由外部安全执行并把结果转成 `data_is_valid`。本文件不处理执行器错误、具体坏值或 SQL 上下文。
- 非法 schema state 会返回 `InvalidState`，而不是跳过或强行完成。找不到旧列/changing 列时返回 `ColumnNotFound`。
- `check_auto_random` 不允许 shard bits 减少/移除或 range bits 变化，但相较 Go `checkAutoRandom`，它没有验证“首次启用必须由 AUTO_INCREMENT clustered PK 转换”等完整表级条件。
- `unique_changing_*_name` 理论上无限搜索并以 `expect` 收尾；其 panic 前提是无界 `usize` 后缀空间全部耗尽，实际不可达，但仍不是可恢复错误。

## 并发与资源生命周期

本文件没有线程、锁、channel、async task、事务或 I/O；所有状态通过独占 `&mut TableInfo`、`&mut ModifyColumnArgs` 和 `&mut i64` 同步修改，因此 Rust 借用规则阻止同一实例在一次调用内并发写。

生命周期跨多次调用：调用者必须保存更新后的 `args` 与表元数据，之后再次调用才能继续状态机。临时列/索引从初始化存活到达到 `Public` 或显式 `rolling_back`；完成/回滚负责清理，半途丢弃 args 会失去临时 ID 的恢复信息。

该内存生命周期不能等同生产容错。真实 DDL 必须把 Job args、schema state、reorg stage/checkpoint 写入持久存储，并在 owner 切换、重试、取消及 schema version 同步边界恢复；这些职责位于 `persistent_modify_column.rs` 与 worker/meta 层，而不在本文件。

## 与 Go 版本的对应关系

- `get_modify_column_type` 对应 Go `getModifyColumnType`：都先用强无重组判定，再处理分区/TiFlash、SQL mode、行重组和索引重组。Rust 简化版通过 context 显式注入优化开关，且没有 Go 对旧 Job `mysql.TypeNull` 的完整兼容分支。
- `no_reorg_data_strict`、`need_row_reorganization`、`need_index_reorganization` 分别对应 Go 同名逻辑；Rust 的 `ColumnKind`/`flen` 规则是压缩模型，Go 还处理 DECIMAL 编码、默认整型宽度、binary padding、vector、collation compatibility 和 restored-data 等细节。
- `build_check_sql_from_modify_column` 对应 Go `buildCheckSQLFromModifyColumn`，保留整数范围、字符串长度、VARCHAR→CHAR 尾空格以及 NULL→NOT NULL 条件；Go 随后通过 restricted SQL executor 执行并生成带坏值的具体错误，Rust 只生成 SQL。
- `check_partition_column_modifiable` 对应 Go 同名守卫及 `checkPartitionColumnTypeChangeAllowlist`。Rust 保留 KEY、RANGE/LIST COLUMNS、表达式分区的主要白名单思想，但没有 Go 的字段 EvalType/flag 全量比较、合法分区类型检查及“生成 SHOW CREATE 片段→重新解析→重建定义”的最终验证。
- `advance_modify_column` 概括 Go `onModifyColumn`、无重组完成、changing object 创建和回滚的状态语义，但没有 Go 的 worker transaction、行更新、索引 backfill、reorg stage、delete range、统计信息与通知流程。
- `check_auto_random` 对应 Go `checkAutoRandom` 的核心位数与列属性约束，但不是完整替代。

[`modify_column_test.rs`](modify_column_test.rs) 大部分测试是对 Go 测试步骤的 `CaseRecorder` 记录，返回成功并不执行对应 SQL；其中 `parity_tests` 的两个普通断言测试真实执行了分区表达式分类与白名单拒绝逻辑。更直接验证本文件状态机的测试位于 `column_modify_test.rs`、`column_type_change_test.rs`、`db_change_test.rs`、`db_integration_test.rs`。Go 的真实 SQL 回归位于 `modify_column_test.go`，覆盖 NULL/NOT NULL、字符集、时间类型、写冲突、skip reorg、路径选择、并行 ALTER、collation、统计信息和加载范围错误。

## 扩展指南

- 修改路径选择时，优先改 `no_reorg_data_strict`/`need_*_reorganization`/`get_modify_column_type`，并同步 `column_type_change_test.rs` 的直接断言和 Go `modify_column_test.go::TestGetModifyColumnType` 的矩阵。不要只调整 `CaseRecorder` 文本。
- 增加分区键允许变更时，同时修改 `collect_partition_expression_usage`、`check_partition_column_modifiable` 及 `modify_column_test.rs::parity_tests`；还应对照 Go partition 测试，特别验证 NULL flag、默认值、表达式函数和分区定义值兼容性。
- 扩展在线状态时，必须同时审查 `initialize_changing_objects`、`advance_modify_column`、`rollback_changing_objects` 的正向/回滚幂等性，确保新 ID 先写入 args，再执行依赖它的步骤，并补充独立测试文件中的中断/恢复场景。
- 接入真实生产链时，不应把本文件的内存模型直接替换持久化 worker；应在 `persistent_modify_column.rs` 和 Job 序列化兼容边界实现，再决定哪些纯判定函数可复用。尤其要保留旧 Job args 解码、checkpoint、delete range 和 schema sync 行为。
- 修改数据检查 SQL 时，要覆盖标识符引用、整数边界、NULL、字符长度和尾空格；生产执行仍应使用受限 SQL executor，并保持具体错误兼容。
- 修改 AUTO_RANDOM 时应同步 Go `checkAutoRandom` 的 clustered PK/AUTO_INCREMENT 转换规则；仅扩展本地 context 不足以证明生产兼容。
- 性能风险主要来自误选 `Reorg` 导致全表扫描，或误选快路径导致数据/索引编码不兼容；兼容风险主要在旧 Job args、分区定义、collation 和 rolling upgrade。任何路径放宽都应以 Go 行为和真实 Rust production worker 的双重证据为准。
- Rust 测试继续放在独立 `*_test.rs` 文件，不在本源文件内嵌测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/ddl/modify_column.rs` 读取了完整 1–1025 行并报告 26 个使用文件；`query advance_modify_column`、`query get_modify_column_type` 定位到源文件符号；`callers/callees advance_modify_column` 未返回边，因此调用接线改用文本搜索核验。
- Rust 源与装配：`pkg/ddl/modify_column.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/column.rs`、`pkg/ddl/generated_column.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/persistent_modify_column.rs`。
- crate 边界：`pkg/ddl/Cargo.toml` 的 package、lib、dependencies、dev-dependencies 和 porting metadata。
- Rust 测试：`pkg/ddl/modify_column_test.rs`、`pkg/ddl/column_modify_test.rs`、`pkg/ddl/column_type_change_test.rs`、`pkg/ddl/db_change_test.rs`、`pkg/ddl/db_integration_test.rs`。
- Go 对照：`pkg/ddl/modify_column.go`、`pkg/ddl/modify_column_test.go`；DDL 包契约与导航依据为 `pkg/ddl/doc.go`、`docs/agents/ddl/README.md`、`docs/agents/ddl/07-modify-column.md`，其中说明性文档只作为导航，结论已由源码/测试复核。
- 本任务是纯文档分析，按任务要求不运行 Cargo。结构验证要求本文恰有十一个固定二级标题；最终交付前另行运行该命令并记录退出码。

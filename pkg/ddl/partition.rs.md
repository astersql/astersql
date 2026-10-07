# `pkg/ddl/partition.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；crate 根 `pkg/ddl/lib.rs` 以 `pub mod partition` 暴露它，`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认其编译边界。它位于 DDL 层，但当前 Rust 实现并不是 Go `pkg/ddl/partition.go` 中完整 owner/worker 分区状态机的逐项复刻，而是两部分能力的组合：

1. `PartitionInfo`、`PartitionDefinition`、`PartitionValue` 等轻量模型，以及建表校验、定义变更、exchange 条件生成和 `SHOW CREATE` 格式化辅助逻辑（`partition.rs:28-1200`）。
2. 已接到真实 DDL 重组类型上的 `backfill_non_touched_partition_indexes`（`partition.rs:1213-1288`），负责 Go `reorgPartitionDataAndIndex` 最后“为未触碰分区补全新全局索引”的一个可恢复批次。

包级契约见 `pkg/ddl/doc.go`：DDL 必须在全局 schema 版本 N/N+1 并存期间等待所有节点同步。本文件的轻量变更函数本身不提交持久化 job；真正涉及持久化恢复语义的是末尾回填函数，它更新 `mysql.tidb_ddl_reorg` 游标。

## 核心职责

- 建立分区元信息：`build_table_partition_info` 接收 `PartitionOptions`，处理 LINEAR/不支持类型警告、临时表限制、分区数量、KEY 隐式主键列、列类型、边界和唯一键约束，成功后一次性写入 `table.partition`。
- 校验定义不变量：`check_range_partition_values` 保证 RANGE 元组严格递增且 `MAXVALUE` 只在最后；`check_list_partition_values` 保证 LIST 元组唯一且 `DEFAULT` 唯一并位于最后；名称比较忽略大小写。
- 提供内存态分区变更：`add_table_partitions`、`drop_table_partitions`、`truncate_table_partitions`、`reorganize_partitions` 和 `exchange_table_partition` 操作 `PartitionedTableInfo`，同时维护物理 ID、adding/dropping 定义及 TiFlash 可用 ID。
- 生成 exchange 校验条件：`build_check_condition_for_range` 返回违规行谓词及参数；`build_check_condition_for_list` 使用 null-safe `<=>` 构造非成员条件。
- 输出 SQL 与运维标识：`append_partition_info`/`append_partition_definitions` 生成 `PARTITION BY` 片段，`partition_ids` 和 `partition_rule_ids` 提取物理 ID/placement 路径。
- 执行真实重组尾段：`backfill_non_touched_partition_indexes` 在现有 owner 事务中调用 `JobExecutionContext::backfill_prepared_indexes`，推进并持久化回填游标；它不负责提交 job、复制分区数据或切换分区定义。

## 主要符号

- 常量：`PARTITION_MAX_VALUE = "MAXVALUE"`；`PARTITION_COUNT_LIMIT = 8192`。
- 枚举：`PartitionType::{None,Range,List,Hash,Key}`；`PartitionValue::{MinValue,Null,Int,UInt,String,MaxValue,Default}`；`TemporaryTableType`；覆盖输入、边界、schema、TiFlash 和 ID 错误的 `PartitionError`。
- 数据结构：`PartitionDefinition` 保存物理 ID、名称、RANGE 上界、LIST 值、注释和 placement policy；`PartitionInfo` 保存策略、表达式/列、正式及 adding/dropping 定义；`PartitionedTableInfo` 聚合列、索引、临时表类型和 TiFlash 可用分区；`PartitionOptions` 是建表输入。
- 构建/校验入口：`build_table_partition_info`、`build_partition_definitions`、`check_partition_definition_constraints`、`check_partition_columns`、`check_partition_keys_constraints`、`check_partition_expression_allowed`。
- 变更入口：`add_table_partitions`、`drop_table_partitions`、`truncate_table_partitions`、`reorganize_partitions`、`exchange_table_partition`、`truncate_table_by_reassign_partition_ids`。
- 输出入口：`build_check_condition_for_range`、`build_check_condition_for_list`、`append_partition_info`、`partition_rule_ids`。
- 回填入口：`backfill_non_touched_partition_indexes(context, job, table, reorg, batch_size) -> Result<bool, String>`；返回 `true` 表示游标已归零、阶段完成。
- 内部辅助：`compare_partition_tuple`、`list_tuple_match`、`tuple_placeholders`、`partition_type_name`、`encode_hex`，均不构成 crate 外 API。

## 执行流程

建表路径以 `build_table_partition_info` 为中心：无 options 时直接成功；有 options 时先拒绝 HASH/KEY 子分区、记录 LINEAR 降级警告并处理不支持类型，再校验临时表和数量。随后构造候选 `PartitionInfo`；KEY 未指定列时从 primary index 推导列并设置 `empty_columns`。候选依次经过列合法性、定义构建、RANGE/LIST/HASH/KEY 约束、唯一索引包含分区列约束和 partial index 拒绝，全部成功后才赋给 `table.partition`（`partition.rs:165-219`）。

内存变更流程都先验证再写回。ADD 合并现有与新增定义验证边界；DROP 解析全部名称且禁止删光；TRUNCATE 要求旧/新 ID 数量相同并确保所有旧 ID 命中；REORGANIZE 在临时 candidate 上做完整定义校验后才替换；EXCHANGE 先比较列和索引 schema、placement policy，再调用传入的记录验证闭包，最后只交换目标分区与普通表的物理 ID（`partition.rs:393-659`）。这些函数不包含 Go owner worker 的 schema-state 推进、schema diff、通知或 delete-range。

真实回填流程如下：

1. `physical_table_id == 0` 视为幂等完成；否则校验游标 job ID、正 batch size、分区定义存在，以及所有 `reorg.element_ids()` 都对应真实 global index。
2. 以当前游标构造 `IndexBackfillBatch`，透传 schema/table/job、key range、priority、resource group 与 SQL mode，调用 `context.backfill_prepared_indexes`。
3. 先在克隆的 `next` 游标上保存 `result.next_key`；当前物理分区完成时，通过 `index::next_recreated_index_partition_id` 选下一个未触碰分区，并用 `tablecodec::GenTableRecordPrefix` 初始化完整记录 key range。
4. 使用同一执行上下文更新 `mysql.tidb_ddl_reorg`；只有 backfill 和 checkpoint 都成功后才以 `*reorg = next` 发布内存游标。最终 `physical_table_id == 0` 时返回完成。

## 数据与状态

轻量模型的核心不变量是 `PartitionInfo.number == definitions.len()`；建表及 ADD/DROP/REORGANIZE 成功路径都会重算它。`adding_definitions`/`dropping_definitions` 模拟 DDL 中间态，但轻量 ADD 和 REORGANIZE 在函数返回前会清空这些字段，DROP 则保留 `dropping_definitions`，因此不能把它们等同于 Go 状态机的完整持久化生命周期。

RANGE 上界以 `Vec<PartitionValue>` 做字典序比较；LIST 以 `BTreeSet<Vec<PartitionValue>>` 去重，因而结果确定且不依赖哈希随机性。名称一律转小写比较，但原始名称保留用于输出和错误。

TiFlash 状态由 `tiflash_available_partition_ids` 表示；DROP/TRUNCATE 会删除旧物理 ID。`check_partition_replica` 只依据调用方提供的 `available_replicas` 计数判断“完成/继续等待/无数据错误”，不访问 PD；这比 Go `checkPartitionReplica` 的 store/region 扫描更轻量。

真实回填状态是 `ReorgInfo` 中的 `job_id`、`physical_table_id`、`start_key`、`end_key` 和 index element 集合，以及 `mysql.tidb_ddl_reorg` 对应行。零 `physical_table_id` 是完成哨兵。checkpoint 与索引写入共享调用方事务，失败时旧 `ReorgInfo` 不被覆盖。

## 依赖与调用关系

轻量部分直接依赖 `crate::index::{ColumnInfo, IndexInfo}`，并使用标准库 `BTreeMap/BTreeSet`。`pkg/ddl/partition_test.rs` 直接验证 RANGE/LIST exchange 条件，另通过内存 `Executor` 验证 add/drop/exchange 的物理 ID 行为；`pkg/ddl/lib.rs:246` 在测试配置下加载该独立测试文件。

回填入口向下调用 `JobExecutionContext::backfill_prepared_indexes`、`JobExecutionContext::query`、`ReorgInfo::element_ids`、`index::next_recreated_index_partition_id` 和 `astersql_tablecodec::GenTableRecordPrefix`，并依赖 `astersql-meta-model` 的真实 `Job`/`TableInfo`。上游直接证据包括 `pkg/ddl/ddl_test.rs:375` 的失败原子性回归，以及 `pkg/ddl/tests/partition/reorg_partition_test.rs:482` 的真实重组场景循环调用。

RustCodeGraph 将本文件识别为被 15 个文件使用，但对所查公开辅助函数的精确 `callers` 未返回边；仓库文本检索显示，除测试之外，当前明确的跨文件直接调用集中在 `backfill_non_touched_partition_indexes`。因此不能据轻量 API 的 `pub` 可见性推断它们已进入生产 owner/job 主链。

## 错误处理与边界

轻量 API 使用细分 `PartitionError`，在修改正式状态前优先返回错误。主要边界包括：0 或超过 8192 个分区；大小写不敏感的重名；RANGE 非递增/提前 `MAXVALUE`/NULL；LIST 重值或非末尾 DEFAULT；分区列缺失、重复或类型不支持；非全局 unique index 缺少分区列；partial index；临时表；DROP 全部分区；TRUNCATE ID 数不等或 ID 不存在；exchange schema/placement/记录不匹配。

`extract_partition_columns` 只是把表达式按非字母数字/下划线切词后匹配列名，`check_partition_expression_allowed` 也是基于禁用 token 的保守字符串筛选；两者不等价于 Go AST、类型推导和常量折叠，扩展时不可把它们当完整 SQL 语义分析器。`PartitionValue` 的派生顺序参与 RANGE 元组比较，也不代表所有 MySQL 跨类型比较规则。

回填函数将错误压成 `String`，但在副作用顺序上有明确原子性：job 不匹配、batch size 为 0、无分区、无 global index element 都在回填前失败；索引写失败不会写 checkpoint；checkpoint 写失败不会发布克隆游标。SQL 字符串由内部数值和十六进制编码 key 构造，没有接收用户 SQL 文本。

## 并发与资源生命周期

轻量模型只接收 `&mut` 引用，无内部线程、锁、channel 或异步任务；Rust 借用保证单次调用期间对表快照的独占修改，但跨节点并发、owner failover 和 schema 同步不在这些函数内。

`backfill_non_touched_partition_indexes` 明确复用调用方已有的 worker 事务和恢复游标。每次只提交一个有界 batch，适合由上层循环重试；它不创建/提交事务，也不清理 reorg 行。先写索引、再写 checkpoint、最后发布内存游标的顺序保证失败重试仍从旧游标开始。物理分区完成后，下一分区的 start/end key 被重置为完整 record prefix 范围；最后一个分区完成后将 ID 置零。回归测试还验证“最后一个当前分区仍必须实际回填”，不能用是否存在 successor 来提前跳过。

## 与 Go 版本的对应关系

`pkg/ddl/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/ddl"` 给出 crate 级迁移来源；具体对照是 Go `pkg/ddl/partition.go`。

- Rust `build_table_partition_info` 对应 Go `buildTablePartitionInfo` 的一部分，但 Go 使用 AST restore、表达式上下文、类型系统和错误码；Rust 使用独立轻量结构与字符串表达式，因此是行为子集。
- Rust RANGE/LIST、分区键、TiFlash、interval 和 exchange 辅助函数分别对应 Go `checkRangePartitionValue`、`checkListPartitionValue`、`checkPartitionKeysConstraint`、`checkPartitionReplica`、`generatePartitionDefinitionsFromInterval` 及 exchange validation 逻辑；Go 版本还包含 PD region 扫描、常量折叠、collation/type 语义和 failpoint，Rust 不应被描述为完整替代。
- Go ADD/DROP/TRUNCATE/REORGANIZE/EXCHANGE 由 job handler 推进 `SchemaState`，处理 schema diff、global index、placement、通知和 delete-range；Rust 同名轻量函数主要变更内存模型，没有完整状态机。
- `backfill_non_touched_partition_indexes` 精确对应 Go `reorgPartitionDataAndIndex` 的末段：新增分区的数据和索引完成后，为未新增、未删除的分区补建处于 write-reorganization 状态的 global index。Rust 将这一末段拆成可恢复的单批函数，并通过真实 `mysql.tidb_ddl_reorg` checkpoint 测试接线。

## 扩展指南

新增分区类型或值语义时，应同步修改 `PartitionType`/`PartitionValue`、定义构建、约束校验、列类型 allowlist、exchange 条件和 SQL 格式化，避免只让解析通过而输出/校验不一致。对应测试应放在独立 `pkg/ddl/partition_test.rs`；涉及真实重组和 checkpoint 的场景应同步 `pkg/ddl/ddl_test.rs` 或 `pkg/ddl/tests/partition/reorg_partition_test.rs`，不要把测试内嵌进生产文件。

增强 SQL 表达式语义时，应优先接入 parser/AST/type context，而不是继续扩大字符串 token 启发式；需要核对 unsigned、collation、日期时间、生成列、常量折叠及 MySQL 错误兼容。增强 ADD/DROP/TRUNCATE/REORGANIZE 时，先判断需求属于轻量模型还是 owner job 主链；若是后者，必须同时考虑 `job.SchemaState`、schema diff、global index 清理/回填、delete-range、placement/TiFlash 和跨节点同步，不能仅修改本文件的内存向量。

修改回填阶段时必须保持三个不变量：当前最后一个分区也要执行；只允许 global index element；索引写和 checkpoint 均成功后才能发布新游标。新增 element 类型、batch 字段或 checkpoint 列时要同步真实集成测试，并评估重试幂等性、事务大小、每批延迟和 SQL mode/resource group 透传。

## 验证依据

- RustCodeGraph：`status` 显示 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ddl/partition.rs` 确认目标有 93 个符号；`node --file ...` 分段读取全部 1,289 行；`query backfill_non_touched_partition_indexes` 定位到 1216 行；`callees` 识别 `IndexBackfillBatch`/`ReorgBackfillTask`，而精确 `callers` 对所查入口为空，已按索引限制处理。
- 源码与边界：`pkg/ddl/partition.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`、`pkg/ddl/doc.go`。
- Go 对照：`pkg/ddl/partition.go`，重点核对 `buildTablePartitionInfo`、`checkAddPartitionValue`、`checkPartitionReplica`、`checkRangePartitionValue`、`checkListPartitionValue`、`checkPartitionKeysConstraint` 与 `reorgPartitionDataAndIndex`。
- Rust 测试：`pkg/ddl/partition_test.rs`（exchange 条件与 add/drop/exchange）；`pkg/ddl/ddl_test.rs`（最后分区、回填错误和 checkpoint 错误不发布游标）；`pkg/ddl/tests/partition/reorg_partition_test.rs`（真实 global index 重组与 reorg 表归零）。Go 边界参考 `pkg/ddl/partition_test.go`。
- 人工复核结论：该文件存在于分区元数据/校验辅助层，并承载一个已接线的重组回填尾段；安全扩展必须先辨明这两层边界。按任务约束未运行 Cargo；交付只做文档结构验证。

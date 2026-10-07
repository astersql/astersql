# `pkg/ddl/create_table.rs`

## 文件定位

`pkg/ddl/create_table.rs` 位于 `astersql-ddl` crate，负责把解析器生成的 `ast::CreateTableStmt`、`ast::ColumnDef`、分区和索引表达式转换成 `astersql_meta_model` 中的 `TableInfo`、`ColumnInfo`、`IndexInfo`、`PartitionInfo` 等内存元数据。crate 根模块 `pkg/ddl/lib.rs` 将 `BuildTableInfoFromAST`、`BuildTableInfoWithStmt`、`BuildColumnInfoFromAST`、`BuildPartitionInfo`、`MaterializeExpressionIndexColumns`、`BuildPartialIndexCondition` 和 `expression_text` 重新导出，因此调用者通常通过 `astersql_ddl::*` 使用它们，而不是直接访问私有模块。

它处在“SQL AST 已经解析完成、DDL Job 尚未持久化”这一段：例如 `pkg/session/runtime/ddl.rs` 用它构建建表、加列、加索引和重分区所需元数据，随后才由 DDL 执行层提交作业。真正把表写入 meta、设置 schema diff、发送 notifier、注册 TTL 并推进 Job 的代码在 `pkg/ddl/persistent_create_table.rs`；本文件本身不分配最终 TableID/PartitionID，不写系统表，也不推进 schema state。

从 DDL 生命周期看，这里是 Job 创建前的纯构建/拒绝关口，而不是 metadata-only 快路径：产出的 `TableInfo` 会被后续 owner/worker 驱动的持久化流程消费。文件内创建的列、索引和约束初始为 `StatePublic`，但该事实只是新表整体一次发布的元数据形态，不代表本文件执行了 schema version 同步。

## 核心职责

1. 将列定义转成 `ColumnInfo`：补齐长度/小数位、字符集/排序规则和 flags，处理默认值、注释、生成列表达式及依赖（`build_column`、`column_default`、`set_no_default_value_flag`）。
2. 将受限 AST 表达式稳定还原为元数据 SQL 文本，并为默认值、生成列、CHECK、分区和部分索引复用（`expression_text_with_column_qualifiers`）。
3. 物化表达式索引：把表达式键改写成 `_V$_<索引名>_<序号>` 隐藏生成列，保留类型、数组属性和依赖（`materialize_expression_index_columns`）。
4. 构造主键、普通/唯一、FULLTEXT、向量、倒排、部分及多值索引，并决定整数 handle 或 common handle（`make_index`、`should_cluster_primary_key`、`BuildPartialIndexCondition`）。
5. 构造分区、外键、CHECK、TTL、placement policy、affinity、storage class、AUTO_RANDOM、SHARD_ROW_ID_BITS 等表级元数据，并在发布前拒绝不兼容组合（`BuildTableInfoWithStmt`）。
6. 通过语句相关校验和通用数量/名称限制提供带校验入口（`build_table_info_with_check`、`check_table_info_valid_with_stmt`、`check_table_info_valid_extra`）；另有 `check_table_info_valid` 供持久化流程对现成 `TableInfo` 做 catalog 校验。

## 主要符号

- `type BuildResult<T> = Result<T, parser::errors::Error>`：本文件 AST 构建路径的统一返回类型；`build_error` 将消息包装为解析器错误。
- `BuildTableInfoFromAST(context, statement)`：最常用的公开入口，以 `utf8mb4` 默认值调用完整构建和两轮本地校验。注释和 Go 对照均说明最终表/分区 ID 仍由后续流程分配。
- `BuildTableInfoWithStmt(context, statement, db_charset, db_collate, placement_policy_ref)`：主构建器，只组装元数据并执行其内部必要检查；调用者若需要完整入口级校验应使用 `BuildTableInfoFromAST` 或自行补齐检查。
- `BuildColumnInfoFromAST(definition, offset, table_charset, table_collate)`：供 `ALTER TABLE ... ADD COLUMN` 等路径复用列构建逻辑；列 ID 的最终分配由调用者负责。
- `BuildPartitionInfo(options)`：供 `ALTER TABLE ... PARTITION BY` 复用分区元数据构建；它是私有 `build_partition_info` 的窄包装。
- `MaterializeExpressionIndexColumns(constraint, columns, offsets)`：供建表和 `ALTER TABLE ADD INDEX` 共用隐藏列命名、类型推断和依赖检查。
- `BuildPartialIndexCondition(expression, table)`：只允许“物理列与字面量的一次比较”或 `IS [NOT] NULL`，并返回规范化条件文本。
- `expression_text(expr)`：公开表达式还原器；支持值、列、函数、二元/一元、NULL 判断、括号、CASE、时间单位、CAST、MAXVALUE、DEFAULT 和 ROW，其他形态返回错误。
- `check_table_info_valid(table)`：crate 内可见；先调用 `astersql_table_tables::tables::table_from_meta_for_validation`，再拒绝不可见的非 handle 主键。`pkg/ddl/persistent_create_table.rs` 和 `pkg/ddl/persistent_actions.rs` 在持久化前调用它。
- 关键私有辅助包括 `build_column`、`resolve_charset_collation`、`check_constraint_names`、`set_table_auto_random_bits`、`validate_generated_column_qualifiers`、`validate_range_partition_boundaries` 和 `make_index`。

## 执行流程

`BuildTableInfoFromAST` 的主链如下：

1. `build_table_info_with_check` 调用 `BuildTableInfoWithStmt`；若语句含 `ReferTable`（CREATE TABLE LIKE）或 `Select`（CREATE TABLE AS SELECT），本 Rust 入口立即返回“需要额外元数据”的错误。
2. `resolve_charset_collation` 按“表选项、数据库默认、collation 反推 charset、utf8mb4 兜底”的顺序解析表级字符集，并检查 charset/collation 匹配。
3. 逐个调用 `build_column`，拒绝重复列名，设置类型默认长度、fsp 上限、YEAR unsigned、默认值、生成列等；字符类型继承表或列级 charset/collation，非字符类型固定为 `binary`，然后补齐 BINARY 默认值和时间默认值精度。
4. 克隆表级约束并经 `check_constraint_names` 处理显式重名及隐式命名；索引类约束再经 `materialize_expression_index_columns` 把表达式键转成隐藏列。
5. 创建 `TableInfo` 骨架并应用 TTL、临时表类型、注释、自增基数、auto-id cache、shard bits、预切分、压缩、placement、affinity 和 storage class。临时表禁止 placement、affinity、分区和预切分；全局临时表只允许 `ON COMMIT DELETE ROWS`。
6. 若存在分区，`build_partition_info` 生成定义，检查重复名称和可解析的 RANGE 严格递增关系；随后校验 `EXTRACT` 的时间单位/列类型，规范 storage class 并重建分区信息。affinity 的 table/partition level 必须与是否分区一致。
7. 收集列级 CHECK 和 REFERENCES；汇总列级或表级主键，拒绝多主键或缺失必需主键，通过上下文的 clustered-index 模式决定 `PKIsHandle`/`IsCommonHandle`，必要时生成 `PRIMARY` 索引。
8. 生成列级 UNIQUE 及表级索引/约束。向量索引验证向量列和距离函数；FULLTEXT 只在 starter 模式允许且要求单个完整升序字符串列；倒排索引要求单个 columnar 列；部分索引和多值索引分别设置条件文本与 `MVIndex`。
9. 汇总最大对象 ID，检查约束/索引/外键重名、外键列数和存在性、自增列唯一性/类型以及 AUTO_RANDOM 规则。聚簇表拒绝 `SHARD_ROW_ID_BITS`；非聚簇普通表可继承会话默认 shard/pre-split，且预切分数不会超过实际分片位数。
10. 返回主构建结果后，`check_table_info_valid_with_stmt` 检查主键必需性、生成列限定名/依赖顺序/embedding 依赖和分区名；`check_table_info_valid_extra` 再限制表名/列名 64 字符、最多 1017 列和 64 索引。

## 数据与状态

- 输入状态来自不可变的 `metabuild::Context` 和 AST；主构建器仅克隆、遍历和局部改写 AST 约束副本，不修改调用者的 `CreateTableStmt`。
- `offsets: HashMap<String, usize>` 以小写列名连接列、索引、外键和表达式依赖；`HashSet` 用于大小写不敏感的列名、约束名、索引名、外键名和分区名去重。
- 普通列 ID 以 `offset + 1` 初始化；表达式索引隐藏列追加到列尾。`MaxColumnID`、`MaxIndexID`、`MaxForeignKeyID`、`MaxConstraintID` 记录构建阶段最大值，但 TableID 和 PartitionID 仍未初始化。
- 生成列在 `GeneratedExprString`、`GeneratedStored`、`Dependences` 中保存表达式文本、存储方式和依赖；表达式索引隐藏列还设置 `Hidden = true`。数组 cast 的隐藏列 charset/collation 强制为 `binary`，以保持下游索引编码行为。
- 主键状态由 `PKIsHandle`（单整数聚簇主键）与 `IsCommonHandle`（非单整数聚簇主键）表达；非聚簇主键由显式 `IndexInfo` 表达。
- 所有新建列、索引、约束、外键都直接初始化为 `StatePublic`。本文件没有 delete-only/write-only/reorg 状态，也没有 checkpoint；新表的持久化、schema diff 和最终版本发布由后续 DDL Job 路径负责。
- TTL 聚合到 `TableInfo.TTLInfo`；`TTL_ENABLE` 和 `TTL_JOB_INTERVAL` 没有 `TTL` 主定义时会报错。starter 模式选择不同的默认 Job interval。

## 依赖与调用关系

上游调用关系可由源码直接核对：

- `pkg/session/runtime/ddl.rs` 调用 `BuildTableInfoFromAST` 构建 CREATE TABLE 元数据，并分别调用列、分区、表达式索引、部分索引和表达式文本辅助完成 ALTER DDL。
- `pkg/session/ddl_tables.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/mview_ddl.rs`、`pkg/session/fts_runtime.rs` 分别用于会话建表、会话运行时、物化视图日志表和全文运行时。
- `pkg/importsdk/file_scanner.rs`、`pkg/testkit/ddlhelper/helper.rs`、`pkg/util/dbutil/dbutiltest/utils.rs` 和 `pkg/ddl/mock.rs` 复用同一构建入口，避免另造 TableInfo 规则。
- `pkg/ddl/persistent_create_table.rs`、`pkg/ddl/persistent_actions.rs` 调用较窄的 `check_table_info_valid`，在写 meta 前再次检查现成表元数据。

下游依赖由 `pkg/ddl/Cargo.toml` 的 `astersql-ddl` crate 声明支撑：解析和 AST 来自 `astersql-parser`、`astersql-parser-ast`、`astersql-parser-charset`；上下文来自 `astersql-meta-metabuild`；目标元数据来自 `astersql-meta-model`；通用表验证来自 `astersql-table-tables`；会话默认值来自 `astersql-sessionctx-vardef`；embedding 识别依赖 `astersql-expression`；starter 能力门控依赖 `astersql-config-deploymode`。同 crate 的 `generated_column`、`storage_class`、`ttl` 模块承担专门规则。

RustCodeGraph 的文件节点报告本文件被 `pkg/ddl/lib.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/split_region.rs` 以及测试文件等 7 个文件使用；精确 `query` 同时定位了 Rust/Go 两侧的 `BuildTableInfoFromAST` 与 `BuildTableInfoWithStmt`。本次 `callers`/`callees` 查询超时无输出，因此以上具体跨文件边以源码引用搜索作为补充证据，不声称来自未返回的图边。

## 错误处理与边界

构建错误统一为 `parser::errors::Error`，通常通过 `?` 原样向上返回；其他 crate 的错误以 `to_string()` 或 `build_error` 映射。错误发生在返回 `TableInfo` 之前，局部 `Vec`/`HashMap` 会随栈释放，不会留下半持久化对象。

重要拒绝边界包括：重复列/索引/约束/外键/分区名，非法 charset/collation，时间 fsp 大于 6，非法数值默认值，不支持的表达式形态，生成列未知或后置生成列依赖，表达式索引未知列，RANGE 数字边界非严格递增，外键列数不匹配，多主键，多自增列，AUTO_RANDOM 位数或组合非法，以及临时表的 placement/partition/affinity/pre-split 限制。

需要特别注意的能力边界：

- `BuildTableInfoWithStmt` 的注释明确表示“without validity check”；虽然内部有大量必要检查，它不包含包装入口的两轮末端校验。
- Rust 的 `check_table_info_valid_extra` 当前只覆盖名称长度、列数和索引数；Go 同名路径还接收 storage、schema 和错误上下文并执行更多分区、TTL、列存/存储相关验证。不能把 Rust 当前实现描述为与 Go 全量等价。
- Rust 入口对 CREATE TABLE LIKE/AS SELECT 明确报错；这些场景需要源表或 planner 元数据，应由更高层专用流程处理。
- `expression_text_with_column_qualifiers` 只接受列出的元数据表达式形态，遇到未覆盖 AST variant 会失败，而不是猜测恢复文本。
- `validate_range_partition_boundaries` 只直接比较可解析整数 tuple；非整数表达式不做字符串序比较，留给其他表达式验证路径。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、async task、channel、事务或外部 I/O。每次调用只读取传入的 AST/上下文，创建局部集合并返回拥有所有权的元数据，因此可以由不同请求并发调用；是否可跨线程共享仍取决于调用者传入的 `metabuild::Context<C, E>` 及其内部对象约束，本文件没有额外同步保证。

它也不持有数据库 lease、MDL、region、allocator 或 transaction。最终 ID 分配、Job 持久化、owner failover、取消/回滚、schema version + diff、follower 同步以及 notifier/TTL 注册均属于后续 DDL 生命周期，主要证据在 `pkg/ddl/persistent_create_table.rs` 和 DDL worker/scheduler 模块。因此本文件失败可直接丢弃局部结果，不涉及 delete-range GC 或回滚状态机。

性能上，构建大致随列、约束、索引和表达式树规模线性增长；重复名集合与列偏移映射提供平均常数时间查询。表达式递归没有在本文件内设置独立深度限制，实际安全边界还依赖解析器产生的 AST 约束。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/create_table.go`。Rust `BuildTableInfoFromAST` 对应 Go 同名函数；二者均以默认 utf8mb4 进入 `buildTableInfoWithCheck`，并按 `BuildTableInfoWithStmt` → 语句相关校验 → 通用校验的顺序返回未分配最终 ID 的 `TableInfo`。Rust 的 `build_column` 对应 Go 建表/加列共用的 `columnDefToCol` 思路，`set_no_default_value_flag`、`set_table_auto_random_bits`、约束命名和 clustered-index 判定也按 Go 规则移植。

目前可确认的差异与迁移边界如下：

- Go 的 `BuildTableInfoWithStmt` 将工作进一步分派给 `buildColumnsAndConstraints`、`BuildTableInfo`、`handleTableOptions`、`buildTablePartitionInfo` 等成熟辅助；Rust 把大部分流程集中在本文件，但继续调用本 crate 的 generated-column、storage-class 和 TTL 辅助。
- Go `buildTableInfoWithCheck` 接收 `kv.Storage`，其 `checkTableInfoValidExtra` 还执行更完整的 storage/partition/TTL/columnar 相关验证；Rust 包装器没有 storage 参数，末端校验子集较窄。
- Go 的更高层 `BuildTableInfo`/执行器可处理 LIKE 等需要 infoschema/source table 的路径；Rust 本入口把 LIKE/AS SELECT 明确留给上层。
- Rust 已直接支持向量、倒排、FULLTEXT、部分索引和多值索引元数据，但每项是否覆盖 Go 的所有错误码、表达式求值和 feature gate 仍应以对应测试与调用层为准，不能仅凭字段存在认定完全移植。

相关 Go 回归证据分布在 `pkg/ddl/db_table_test.go`、`pkg/ddl/db_integration_test.go`、`pkg/ddl/index_modify_test.go`、`pkg/ddl/storage_class_test.go` 和 `pkg/ddl/split_region_test.go`，覆盖 TableInfo 构建、生成列/表达式索引、向量/倒排索引、storage class、分区与 split 行为。

## 扩展指南

- 新增列选项：修改 `build_column` 的 option 分支；若影响“是否有默认值”、charset 或默认值归一化，还要同步 `set_no_default_value_flag`、`pad_binary_default_value` 或 `normalize_temporal_default_value`。回归测试应放在独立的 `pkg/ddl/create_table_aster_unit_test.rs` 或 `pkg/ddl/create_table_validation_aster_unit_test.rs`，不要内嵌进生产文件。
- 新增表选项：在 `BuildTableInfoWithStmt` 的聚合/应用阶段选择正确顺序；需要先有主体定义的子选项应仿照 TTL 做成显式依赖检查。同步核对 `pkg/ddl/create_table.go` 的 `handleTableOptions` 与调用层是否另有验证。
- 新增表达式 AST 形态：同时审视 `expression_text_with_column_qualifiers`、`collect_expression_columns`、`collect_generated_column_dependencies` 和 `validate_generated_column_qualifiers`，避免“能恢复文本但漏依赖/限定名校验”。
- 新增索引类型：在约束分派、`make_index`、名称去重、列 flags、专用 metadata 和末端数量限制各处接线；ALTER ADD INDEX 若需相同隐藏列语义，应继续复用 `MaterializeExpressionIndexColumns`。
- 新增分区语义：修改 `build_partition_info` 及对应验证函数，并确认 storage class normalization/rebuild 的调用顺序；若依赖表达式求值或完整 Go partition checks，不应只在当前的整数边界检查中做简化。
- 修改主键/AUTO_RANDOM/shard 规则：同时检查 `primary_key_type`、`should_cluster_primary_key`、`set_table_auto_random_bits` 和最终 shard/pre-split 归一化；重点回归 `pkg/ddl/primary_key_handle_test.rs` 与 `pkg/ddl/create_table_test.rs`。
- 若扩展涉及真正持久化、回滚或 schema state，请在 `persistent_create_table.rs`/worker 路径实现，不要把事务或 owner 逻辑塞进本 AST 构建器。

兼容性风险主要来自 Go 错误优先级、错误码/消息、默认 charset/collation、隐藏列命名和元数据字段序义；性能风险主要是对每个约束重复遍历大表达式或大列集合。安全扩展应先对照 Go 增量，再新增独立 Rust 回归，最后验证 session/ALTER 调用者仍走统一辅助。

## 验证依据

- RustCodeGraph：`status` 确认本地索引包含 11,467 文件、307,296 节点和 1,848,419 边；`node --file pkg/ddl/create_table.rs` 返回完整 2,462 行文件节点及 7 个文件级使用者；`query BuildTableInfoFromAST`、`query BuildTableInfoWithStmt` 同时定位 Rust 与 Go 定义。`callers`/`callees` 在本次会话中超时且无结果，故跨文件调用边另由下列源码引用核对。
- 生产源码：`pkg/ddl/create_table.rs`（全部模块级符号和主流程）、`pkg/ddl/lib.rs`（模块声明及公开再导出）、`pkg/session/runtime/ddl.rs`、`pkg/session/ddl_tables.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/mview_ddl.rs`、`pkg/session/fts_runtime.rs`、`pkg/importsdk/file_scanner.rs`、`pkg/ddl/persistent_create_table.rs`、`pkg/ddl/persistent_actions.rs`。
- crate 边界：`pkg/ddl/Cargo.toml`，package 为 `astersql-ddl`、lib 入口为 `lib.rs`，并声明 parser/AST/charset、meta model/metabuild、expression、table validation、deploy mode 和 session vardef 等直接依赖。
- Go 对照：`pkg/ddl/create_table.go` 中 `BuildTableInfoFromAST`、`buildTableInfoWithCheck`、`checkTableInfoValidWithStmt`、`checkTableInfoValidExtra`、`BuildTableInfoWithStmt` 和 `setTableAutoRandomBits`；调用侧还核对 `pkg/ddl/executor.go`、`pkg/ddl/materialized_view.go` 与 `pkg/testkit/ddlhelper/helper.go`。
- Rust 独立测试：`pkg/ddl/create_table_test.rs`（clustered shard 错误、shard 上限、TTL 子选项依赖）；`pkg/ddl/create_table_aster_unit_test.rs`（列/索引/生成列/CHECK/FK、默认 flags、charset/collation、TTL、分区、表达式索引、AUTO_RANDOM、storage class）；`pkg/ddl/create_table_validation_aster_unit_test.rs`（默认值、fsp、限定生成列、CAST、embedding）；`pkg/ddl/primary_key_handle_test.rs`（聚簇主键）；`pkg/ddl/tests/multivaluedindex/multi_valued_index_test.rs`（多值索引）。
- Go 测试参考：`pkg/ddl/db_table_test.go`、`pkg/ddl/db_integration_test.go`、`pkg/ddl/index_modify_test.go`、`pkg/ddl/storage_class_test.go`、`pkg/ddl/storage_class_partition_test.go`、`pkg/ddl/split_region_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证只检查文件存在、固定章节数量、引用和 diff 范围。

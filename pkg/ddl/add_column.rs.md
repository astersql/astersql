# `pkg/ddl/add_column.rs`

## 文件定位

[目标源码](add_column.rs)属于 Cargo 包 `astersql-ddl`（`pkg/ddl/Cargo.toml`，库入口为 `pkg/ddl/lib.rs`），并由 `lib.rs` 以 `pub mod add_column` 暴露。它把 ADD COLUMN 的一部分 Go 语义移植为纯 Rust 内存模型：校验 `ColumnDefinition`、构造 `ColumnInfo`、把列追加到 `TableInfo`，以及逐次推进 `SchemaState`。

必须注意当前接线边界：仓库内对 `start_add_column`、`advance_add_column` 和 `create_new_column` 的直接引用都来自 Rust 测试；生产侧 `pkg/ddl/executor.rs::Executor::add_column` 直接插入其自身的列模型并提交 `DdlAction::AddColumn`，没有调用本文件。因此，本文件当前不是完整 SQL → 持久化 DDL job → owner worker → schema sync 主链的实现，而是可独立验证的校验与状态迁移模型。`adjust_blob_type_length` 在仓库内也没有调用者。

## 核心职责

1. 用 `ColumnConstraint`、`GeneratedDefinition`、`ColumnDefinition` 表达 ADD COLUMN 输入，并用 `AddColumnError` 汇总本地错误。
2. `create_new_column` 拒绝本路径不支持的约束、TiFlash 不支持的字符集、保留名/重名、stored generated column、非法生成表达式及不安全默认函数，然后构造初始 `ColumnInfo`。
3. `overwrite_collation_with_binary_flag`、`process_column_flags` 和 `adjust_blob_type_length` 提供 MySQL/TiDB 字段类型归一化的局部实现。
4. `start_add_column` 在列数上限检查后分配列 ID，并以 `SchemaState::None` 追加到表尾。
5. `advance_add_column` 每次只前进一步：`None → DeleteOnly → WriteOnly → WriteReorganization → Public`；进入 `Public` 时才按 `FIRST`/`AFTER` 调整列顺序，或在 `rolling_back` 时移除列。

这些职责仅修改传入的 Rust `TableInfo` 和 `schema_version`；本文件不持久化 job/reorg checkpoint，不访问系统表，不等待集群 schema 同步，也不执行存量行扫描或回填。

## 主要符号

- 四个 BLOB 上限常量：`TINY_BLOB_MAX_LENGTH`、`BLOB_MAX_LENGTH`、`MEDIUM_BLOB_MAX_LENGTH`、`LONG_BLOB_MAX_LENGTH`，供 `adjust_blob_type_length` 选择最小可容纳类型。
- `ColumnConstraint`：列级选项枚举。其中 `AutoIncrement`、`PrimaryKey`、`UniqueKey`、`AutoRandom` 会被 `check_unsupported_column_constraint` 拒绝；当前 `create_new_column` 只显式消费 `NotNull`，`Null`、`Binary`、`OnUpdateCurrentTimestamp` 不会直接改变产物，二进制属性来自 `FieldType::binary`。
- `GeneratedDefinition`：保存原始表达式文本、`ExpressionNode`、依赖列集合及 stored 标志。
- `ColumnDefinition`：新列名称、`FieldType`、约束、可选默认值、注释和可选生成列定义。当前 `comment` 字段没有写入 `ColumnInfo`。
- `AddColumnError`：本文件的闭合错误集合；`From<GeneratedColumnError>` 保留生成列错误的结构化来源。
- `check_unsupported_column_constraint`、`check_unsupported_charset_for_tiflash`：前置兼容性检查。后者只在 `TableInfo::tiflash_replica == true` 时允许 `binary/ascii/latin1/utf8/utf8mb4`。
- `overwrite_collation_with_binary_flag`：仅对 `Varchar/String/Enum/Set` 且字符集非空的 binary 字段写入 `<charset>_bin`。
- `process_column_flags`：字符串/BLOB 类字段的 binary 标志跟随字符集；BIT 强制 unsigned 且清 binary；YEAR 强制 zerofill 且清 binary；zerofill 最终隐含 unsigned。
- `adjust_blob_type_length`：只处理 `ColumnKind::Blob`，按 `flen * charset_max_length` 选择 Tiny/Blob/Medium/Long；使用 `checked_mul` 防止溢出。它不是 `create_new_column` 当前调用链的一部分。
- `create_new_column`：核心构造入口，返回尚未分配 ID、尚未加入表的 `ColumnInfo`。
- `start_add_column`：组合构造、列数检查与 `column::init_and_add_column_to_table`，返回新列 ID。
- `AddColumnOutcome`：报告一步迁移后的状态、版本和完成标志。
- `advance_add_column`：正常推进或回滚的唯一入口。

## 执行流程

`create_new_column` 的顺序是：检查不支持的约束 → 检查 TiFlash 字符集 → 拒绝大小写不敏感的 `_tidb_rowid` → 大小写不敏感地查重 → 若为生成列则拒绝 stored、检查非法函数、依赖存在性、依赖顺序以及自增列引用 → 克隆并归一化字段类型 → 检查默认值 → 填充 `ColumnInfo` 的 not-null、默认值及生成列元数据。默认表达式通过首个 `(` 前的 ASCII 小写函数名匹配黑名单；这不是完整 SQL 表达式解析。

`start_add_column` 先完整构造列，再以 `table.columns.len() + 1` 检查上限。只有检查通过才调用 `init_and_add_column_to_table`，后者递增 `max_column_id`，把状态设为 `None`、offset 设为当前尾部并追加；所以前置失败不会留下半加入的列。

正常的 `advance_add_column` 先按 ID 定位列，再计算唯一合法后继。前三步只改状态；第四步先用 `locate_offset_to_move` 解析目标位置，再把状态设为 `Public` 并由 `move_column_info` 移动、重排 offset。每个成功步骤将外部传入的 `schema_version` 加一，`finished` 只在 `Public` 为真。

回滚分支不按反向状态机逐级推进：只要能按 ID 找到列，就立即移除它、重排剩余 offset、版本加一，并返回 `{ state: None, finished: true }`。这与 Go `onAddColumn` 把 rolling-back job 委托给 drop-column 流程并持久化 job 状态不是同一完整机制。

## 数据与状态

主要可变状态均由调用者所有：`TableInfo.columns`、`TableInfo.max_column_id` 和 `&mut i64 schema_version`。新列先追加在尾部，以便非 Public 阶段保持稳定的内部 offset；用户指定的位置只在发布时应用。`move_column_info` 与回滚分支都保证 `column.offset == columns` 中的数组下标。

`default_value` 与 `origin_default_value` 在构造时取同一个克隆值，随后状态迁移不改默认值；`pkg/ddl/column_change_test.rs::test_column_add` 验证四个阶段中默认值保持不变，也验证未指定默认值保持 `None`。生成列会复制 SQL 文本和依赖集合；普通列保留 `generated == false`。

版本号只是调用者传入的局部计数器；文件没有全局原子量、schema diff 或 durable version。`AddColumnOutcome.schema_version` 是递增后的值，而不是从存储层分配的集群版本。

## 依赖与调用关系

RustCodeGraph 给出的核心下游边包括：`start_add_column → create_new_column`、`check_add_column_too_many_columns`、`init_and_add_column_to_table`；`advance_add_column → locate_offset_to_move`、`TableInfo::move_column_info`；`create_new_column` 调用本文件的约束、TiFlash、排序规则、标志和默认值检查，并在源码中调用 `generated_column.rs` 的 `check_illegal_function_for_generated`、`check_depended_columns_exist`、`verify_column_generation_single`、`check_auto_increment_reference`。

直接 Rust 调用者目前位于测试：`pkg/ddl/column_change_test.rs` 覆盖启动和完整迁移；`pkg/ddl/db_change_test.rs` 用它模拟 two-state 检查；`pkg/ddl/db_integration_test.rs` 与 `pkg/ddl/integration_validation_aster_unit_test.rs` 直接验证构造错误/后续改列组合；`pkg/ddl/add_column_test.rs` 直接验证字段标志。生产 `pkg/ddl/executor.rs::Executor::add_column` 是平行实现，不构成本文件的上游。

crate 内依赖来自 `crate::column` 与 `crate::generated_column`；本文件自身唯一标准库依赖是 `HashSet`。`pkg/ddl/Cargo.toml` 将 Go 对照声明为 `[package.metadata.porting] go-package = "pkg/ddl"`，但没有为本文件设置 feature gate；模块始终编译，独立测试通过 `lib.rs` 的 `#[cfg(test)] mod add_column_test` 挂载。

## 错误处理与边界

所有入口返回结构化 `Result`，不吞错。重要边界如下：

- 重名和 `_tidb_rowid` 均大小写不敏感；保留名目前复用 `InvalidDefault`，错误类别不够精确。
- `AFTER` 目标必须是已 `Public` 的列；解析或移动失败被压缩为 `UnknownColumn`，会丢失底层 `ColumnError` 的类别。
- NOT NULL + 显式 `DefaultValue::Null` 被拒绝；没有默认值的 NOT NULL 列在本地模型中可构造，真实 origin default/backfill 语义不由本文件补齐。
- 默认函数黑名单只按字符串首个括号前文本识别，且未 `trim`；复杂表达式、大小写以外的词法差异不应被认为已经完整覆盖。
- BLOB 长度乘法溢出及超过 LONGBLOB 上限都复用 `InvalidDefault`。Go `adjustBlobTypesFlen` 对超过 long 上限没有在该函数内显式报错，而 Rust 辅助函数更严格；同时 Rust 当前未把它接入构造流程。
- 对 `Public` 或其他非四个前驱状态再次推进返回 `InvalidState`，不会增加版本。列 ID 不存在也不会改表或版本。
- 回滚是直接元数据删除，没有索引清理、delete-range GC、reorg checkpoint 或不可逆阶段判断。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel、事务、文件或网络资源。借用签名确保一次调用独占修改 `TableInfo`/版本值，但不提供跨线程、跨节点或跨进程协调。每次调用结束时所有临时克隆和依赖集合按普通 Rust 所有权释放。

在线 DDL 注释描述的是状态兼容协议，而不是本文件已经实现 owner failover 或 schema barrier。真正系统应把每一步持久化并等待其他节点同步；Go `worker.onAddColumn` 会调用 `updateVersionAndTableInfo*`、在 WriteOnly 后标记不可回滚、发布 notifier 事件并完成 job。本 Rust 文件只返回 `AddColumnOutcome`，没有这些生命周期动作，也没有真实 `WriteReorganization` 扫描。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/ddl/add_column.go`：

- Rust 的约束与 TiFlash 检查对应 Go `checkUnsupportedColumnConstraint`、`checkUnsupportedCharsetForTiFlash`；允许字符集集合与 Go 的 `charset.TiFlashSupportedCharsets` 意图一致，但 Rust 使用硬编码白名单。
- `create_new_column` 对应 Go `CreateNewColumn` 的一小部分：二者都拒绝 stored generated column、验证依赖/顺序/自增引用并拒绝 `nextval/rand/uuid/uuid_to_bin/replace/upper` 默认函数；Go 还处理 AST、会话变量、schema/table 字符集解析、长度/标识符、origin default、EMBED_TEXT 和更完整的类型/default 校验。
- `process_column_flags` 对应 Go `processColumnFlags`，`pkg/ddl/add_column_test.rs::string_binary_flag_follows_charset_like_go` 明确锁定“字符串 binary 标志跟随 charset”语义。
- `adjust_blob_type_length` 对应 Go `adjustBlobTypesFlen`，但 Rust 把字符集最大字节数作为参数并增加溢出/超上限错误；当前没有接入 `create_new_column`。
- `advance_add_column` 的前进状态序列与 Go `worker.onAddColumn` 相同；差异是 Go 每阶段更新持久元数据/schema version，WriteOnly → WriteReorganization 后 `MarkNonRevertible`，Public 前后同步物化视图日志、校验表元数据、发 notifier 并结束 job。Rust 无这些行为。
- Go rolling back 路径调用 `onDropColumn`；Rust 直接删除内存列。Go 先初始化列再检查列数，Rust `start_add_column` 在追加前检查，因此失败时的内存中间态也不同。

因此该 Rust 文件应视为 Go 行为的聚焦模型，而不是 `add_column.go` 的等价完整移植。

## 扩展指南

扩展前先判断目标属于“局部模型”还是“完整 DDL 接线”。新增纯校验应优先放进 `create_new_column` 的前置阶段，并在 `pkg/ddl/add_column_test.rs` 或同目录独立测试文件增加最小边界用例；不要把测试内嵌到 `add_column.rs`。新增状态应同步修改 `SchemaState`、`advance_add_column`、`AddColumnOutcome` 断言和 `pkg/ddl/column_change_test.rs` 的完整状态序列。

若要让生产执行器复用本文件，不能只调用 `advance_add_column`：还必须设计 job 参数持久化、幂等恢复、owner worker 调度、schema version/diff、同步屏障、取消/不可逆边界、真实回填 checkpoint、notifier 与 GC，并解决 `executor.rs` 和 `column.rs` 两套列/表模型的转换。此类接线应对照 Go `worker.onAddColumn`，并按 DDL 测试流程增加独立的 job/集成测试。

修改类型规则时，应同时检查 create/add 与 modify/change 的一致性：Go 特意让 `processColumnFlags` 被两类语句共享。若接入 `adjust_blob_type_length`，需先决定 Rust 超长报错与 Go 当前行为的兼容策略，并为字符集倍数、边界值和乘法溢出补测试。还应决定当前未消费的 `comment`、`Null`、`Binary`、`OnUpdateCurrentTimestamp` 是实现缺口还是仅供上层预处理，避免静默接受却不生效。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`node --file pkg/ddl/add_column.rs` 读取完整 462 行；`query` 精确定位 `create_new_column`、`start_add_column`、`advance_add_column`、约束/字符集/标志/BLOB 辅助函数；`callees` 核对了上述核心下游边。批量 `callers` 查询超时，因此调用者集合另用精确 `rg` 核对。
- 源码与边界：`pkg/ddl/add_column.rs`、`pkg/ddl/column.rs`、`pkg/ddl/generated_column.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/executor.rs`、`pkg/ddl/Cargo.toml`。
- Go 对照：`pkg/ddl/add_column.go` 的 `worker.onAddColumn`、`checkUnsupportedCharsetForTiFlash`、`checkUnsupportedColumnConstraint`、`CreateNewColumn`、`processColumnFlags`、`adjustBlobTypesFlen`。
- Rust 测试：`pkg/ddl/add_column_test.rs`、`pkg/ddl/column_change_test.rs`、`pkg/ddl/db_change_test.rs`、`pkg/ddl/db_integration_test.rs`、`pkg/ddl/integration_validation_aster_unit_test.rs`。它们证明标志归一化、默认函数拒绝、默认值跨状态保持、位置移动和版本逐步递增；未证明生产 executor 已接线或分布式 job 生命周期。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅执行固定的 11 章节结构校验，并人工核对重要陈述均指向上述符号或路径。

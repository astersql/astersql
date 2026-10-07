# `pkg/ddl/foreign_key.rs`

源文件：[`foreign_key.rs`](foreign_key.rs)

## 文件定位

`foreign_key.rs` 属于 `astersql-ddl` crate；[`pkg/ddl/Cargo.toml`](Cargo.toml) 以 `lib.rs` 为库入口，而 [`pkg/ddl/lib.rs`](lib.rs) 通过 `pub mod foreign_key` 公开本模块。它依赖同 crate 的 [`column.rs`](column.rs) 中 `ColumnInfo`、`ColumnKind`、`IndexInfo`、`SchemaState` 和 `TableInfo`，用一个内存 `ForeignKeyCatalog` 表达外键相关的元数据与规则。

当前接线边界必须特别说明：RustCodeGraph 对本文件的公开函数未找到生产调用者，仓库直接引用集中在 [`foreign_key_test.rs`](foreign_key_test.rs) 和 [`tests/fk/foreign_key_test.rs`](tests/fk/foreign_key_test.rs)；生产 DDL 执行器中另有 [`executor.rs`](executor.rs) 的外键 API，以及 [`persistent_actions.rs`](persistent_actions.rs) 的持久化删除动作。因此，本文件是已实现并有独立测试覆盖的外键规则/状态机模型，不应被描述成已经接入 owner、持久化 job、InfoSchema 和真实 SQL 执行器的完整生产链路。

## 核心职责

- 用 `ForeignKeyInfo`、`ForeignKeyTable` 和 `ForeignKeyCatalog` 保存父子表关系、引用动作、Schema 状态、索引和表属性，并提供大小写不敏感的目录查找。
- 用 `check_foreign_key_definition` 和 `check_add_foreign_key_valid` 检查外键名称、父表存在性、表类型限制、列映射、列类型以及父子两侧覆盖索引。
- 用 `advance_create_foreign_key` 模拟创建外键的 `None -> WriteOnly -> WriteReorganization -> Public` 状态推进，用 `drop_foreign_key` 模拟单步删除或创建失败后的回滚清理。
- 在修改列、删除列、删除索引和删除被引用表前，检测已有外键依赖并返回结构化的 `ForeignKeyError`。
- 用 `build_foreign_key_check_sql` 生成探测存量孤儿行的 SQL 文本；本文件只生成文本，不执行 SQL。

## 主要符号

- `ReferentialAction`：`Restrict`、`Cascade`、`SetNull`、`NoAction` 四种 `ON DELETE`/`ON UPDATE` 动作。动作会随元数据保存；本文件除校验 `SetNull` 与子列 `NOT NULL` 冲突外，不执行级联数据修改。
- `ForeignKeyInfo`：单条外键的 ID、名称、子列、父库/表/列、引用动作、版本和 `SchemaState`。`columns` 与 `referenced_columns` 必须非空且等长。
- `ForeignKeyTable`：把 `TableInfo` 与库名、临时/分区/TTL 标记、`primary_key_is_handle`、最大外键 ID 和子表持有的外键列表组合起来。
- `ForeignKeyCatalog`：以小写 `(schema, table)` 为键的 `BTreeMap`，并带全局 `enabled` 开关。`add_table`、`table`、`table_mut` 和 `referred_foreign_keys` 都实现大小写不敏感的定位。
- `ForeignKeyError`：覆盖重复/缺失外键、父表打不开、非法定义、受限表类型、缺列、类型不兼容、缺索引、存量行违规、父表/索引/列仍被引用以及非法状态。
- `check_foreign_key_definition`：单条定义的核心校验器；检查临时表、TTL 父表、分区表、列数、自引用同列、虚拟生成列、`SET NULL`、类型兼容和父表覆盖索引。
- `check_add_foreign_key_valid`：新增外键入口校验；无论功能开关如何都先查重名，启用后再处理父表、定义和子表覆盖索引。
- `ForeignKeyOutcome`、`advance_create_foreign_key`、`drop_foreign_key`：分别表示单步结果、创建状态机和删除状态机。
- `is_acceptable_foreign_key_column_change`、`check_modify_column_with_foreign_key`：检查列修改是否继续满足子表方向和父表方向的外键约束。
- `check_table_has_foreign_key_referred`、`check_index_needed_in_foreign_key`、`check_drop_column_with_foreign_key`：表、索引和列删除前的依赖保护。
- `build_foreign_key_check_sql`：按复合列顺序构造 `IS NOT NULL` 与元组 `NOT IN` 探测语句。

## 执行流程

创建外键的模型流程如下：

1. 调用者构造 `ForeignKeyCatalog`、父子 `ForeignKeyTable` 和状态为 `None` 的 `ForeignKeyInfo`。
2. `advance_create_foreign_key` 在 `None` 阶段调用 `check_add_foreign_key_valid`。该函数先从目录找子表并检查名称冲突；`catalog.enabled == false` 时在查重后直接成功。
3. 功能启用时，父表不存在且 `foreign_key_checks == true` 返回 `CannotOpenParent`；检查关闭则允许延迟校验。父表存在时，继续执行定义检查和子表覆盖索引检查。
4. 初次推进成功后，子表的 `max_foreign_key_id` 自增并赋给外键，外键克隆进子表元数据，状态变为 `WriteOnly`，`schema_version` 自增一次。
5. 第二次推进从目录中按 ID 找到已存外键。若 `foreign_key_checks` 开启且 `existing_rows_valid == false`，返回 `ExistingRowsViolate`，状态和版本保持不变；否则进入 `WriteReorganization` 并递增版本。
6. 第三次推进进入 `Public`，再次递增版本，返回 `finished == true`。若已存状态不是 `WriteOnly` 或 `WriteReorganization`，返回 `InvalidState`。
7. 任意创建阶段由调用者以 `rolling_back == true` 重新调用时，本函数按名称移除子表中的外键，递增版本并返回 `None`、`finished == true`、`rollback_done == true`。

删除外键是单阶段的：`drop_foreign_key` 找到子表，按大小写不敏感的名称过滤外键列表；若长度未变化则返回 `ForeignKeyNotFound`，否则递增版本并返回完成结果。其 `rolling_back` 参数只决定结果中的 `rollback_done` 标志。

其他保护流程均是同步只读检查：列修改先扫描本表持有的外键，再通过 `referred_foreign_keys` 扫描把本表当父表的外键；索引删除先构造排除目标索引后的列表，再验证每组相关列是否仍有替代覆盖；表和列删除则在找到第一条未忽略依赖时立即报错。

## 数据与状态

- 目录键由 `to_ascii_lowercase` 生成，库名、表名、列名和外键名的比较使用 ASCII 大小写不敏感语义。`BTreeMap` 使目录遍历顺序确定，但业务逻辑不应依赖该顺序。
- `max_foreign_key_id` 是表级单调递增计数器，仅在创建的 `None` 阶段校验通过后更新；删除或回滚不会回收 ID。
- `schema_version: &mut i64` 由调用者持有。创建每个成功阶段、成功删除和成功回滚各加一；校验失败、找不到外键或非法状态不会加一。
- 创建时存在两份状态：调用者传入的 `foreign_key` 和目录中保存的克隆。后续推进同时更新二者；查找目录对象使用 ID，回滚删除使用名称。
- `table_has_covering_index` 要求索引的前 N 列按顺序覆盖外键列，短于列定义宽度的前缀索引不算覆盖。一般路径不检查索引的 `SchemaState`；`primary_key_is_handle` 的单列特殊路径会额外要求列为 `Public`，随后一般索引路径仍可提供覆盖。
- `foreign_key_column_types_match` 实际比较 `kind`、`unsigned`、`charset`、`collation`，不比较 `flen` 或 `decimal`。虽然源码注释把字符集/排序规则强调为字符串规则，当前实现对所有 `ColumnKind` 都比较这两个字段。
- `is_acceptable_foreign_key_column_change` 对 `ColumnKind::Integer` 直接返回 `true`；其他类型要求新长度不小于关联列和原列。函数后部再次判断 `Integer` 的分支因前面的提前返回而不可达；当前 `column.rs::ColumnKind` 也没有单独的 Decimal 变体。

## 依赖与调用关系

直接下游依赖全部来自 `crate::column` 和标准库：`ColumnInfo`/`TableInfo` 提供列与索引元数据，`IndexInfo` 提供索引列和前缀长度，`SchemaState` 驱动外键状态，`ColumnKind` 区分整数列；`BTreeMap` 保存目录。`pkg/ddl/Cargo.toml` 没有为本文件引入专属外部 crate。

文件内主要调用边经 RustCodeGraph 核对为：

- `advance_create_foreign_key -> check_add_foreign_key_valid -> check_foreign_key_definition`；后者继续调用 `find_column`、`is_virtual_generated`、`set_null_on_not_null_child`、`foreign_key_column_types_match` 和 `table_has_covering_index`。
- `check_modify_column_with_foreign_key -> referred_foreign_keys/find_column/foreign_key_column_types_match/is_acceptable_foreign_key_column_change`。
- `check_index_needed_in_foreign_key -> referred_foreign_keys/index_covers_columns/find_column`。
- 表删除和列删除保护都通过 `referred_foreign_keys` 查找反向关系。

上游方面，`lib.rs` 公开模块，两个 Rust 外键测试文件直接调用这些 API；RustCodeGraph `callers` 对九个主要公开函数均未报告生产调用边，`rg` 也只发现测试侧调用。因此扩展本文件不会自动改变当前 `executor.rs` 或持久化 job 的生产行为，若目标是用户可见 SQL 行为，必须先确认并修改实际执行链，而不能只修改本模型。

## 错误处理与边界

所有校验函数使用 `Result<_, ForeignKeyError>`，错误包含可供测试断言的名称或列名，但没有 Go `dbterror` 的错误码、堆栈包装或 job 状态副作用。主要边界如下：

- 子表缺失被映射为 `ColumnNotFound(child_table)`；父表缺失依据 `foreign_key_checks` 返回 `CannotOpenParent` 或放行。
- 临时父/子表、分区父/子表和 TTL 父表被拒绝；TTL 子表没有在此函数中被拒绝。
- 空列组、列数不等、同表同列组自引用、任一侧虚拟生成列，以及在 `NOT NULL` 子列上配置任一 `SetNull` 动作，返回定义错误。
- 不同列类型、unsigned、charset 或 collation 返回 `IncompatibleColumns`；显示宽度由列修改规则另行约束。
- `check_index_needed_in_foreign_key` 在索引 ID 不存在时返回 `IndexNeeded(index_id.to_string())`，与“索引存在但无替代项”共用错误变体。
- `build_foreign_key_check_sql` 直接把标识符插入反引号，不转义名称中的反引号，也不负责参数绑定；它适合当前受控元数据模型，不能直接视为通用 SQL 构造器。空列列表会生成无效/无意义 SQL，但合法外键应已由定义校验排除这种输入。
- `NoAction` 在本文件中只是元数据枚举值，没有单独执行逻辑；源码注释约定其语义等同 `Restrict`。

## 并发与资源生命周期

本文件没有锁、原子变量、异步任务、通道、事务、I/O 或会话池。`ForeignKeyCatalog` 由调用者以普通共享引用或独占可变引用传入，并发安全和持久化责任均在调用者之外；Rust 借用规则只保证单进程调用期间的别名安全。

资源生命周期完全是内存级：`add_table` 接管表值，创建外键时克隆元数据到子表，删除/回滚用 `Vec::retain` 立即移除，存量校验结果由布尔参数注入，SQL 构造函数只返回 `String`。这里没有 owner 选举、job 重试/恢复、事务提交、schema sync、会话归还或故障注入；这些不能从 `schema_version` 自增这一模型行为推断出来。

## 与 Go 版本的对应关系

主要语义映射到 [`foreign_key.go`](foreign_key.go)：

- Rust `advance_create_foreign_key` 对应 Go `(*worker).onCreateForeignKey` 的三阶段状态机；Rust `drop_foreign_key` 对应 Go `dropForeignKey`。
- Rust `check_foreign_key_definition` 对应 Go `checkTableForeignKey`，`check_add_foreign_key_valid` 对应 `checkAddForeignKeyValidInOwner` 的查重、父表和两侧索引检查。
- Rust 的列修改、父表引用、索引删除和列删除保护分别对应 Go `checkModifyColumnWithForeignKeyConstraint`、`checkTableHasForeignKeyReferred`、`checkIndexNeededInForeignKey`、`checkDropColumnWithForeignKeyConstraint`。
- Rust `build_foreign_key_check_sql` 对应 Go `checkForeignKeyConstrain` 中的 SQL 形状：过滤所有子列的 NULL，按列顺序进行元组 `NOT IN`，并以 `LIMIT 1` 探测违规行。

Rust 保留了关键规则和状态顺序，但并非 Go 生产实现的等价替换。Go 通过 `worker`、`jobContext`、`model.Job`、InfoSchema/Meta、`updateVersionAndTableInfo` 和 session pool 运行，写入持久元数据、更新 job 状态、执行受限 SQL、带有异步提交延迟和 failpoint，并用具体数据库错误码报告失败；Rust 本文件用内存目录、调用者提供的版本计数和 `existing_rows_valid` 布尔值模拟这些结果。Go 还会根据 `FKVersion1` 过滤旧元数据，并含数据库级引用检查和 owner 包装函数，这些不在本文件中。

细节差异也需保留：Go 的整数类型判断覆盖多种 MySQL 整数类型，并对 `TypeNewDecimal` 保持精度/小数位不变；Rust `ColumnKind` 只用一个 `Integer` 变体且没有 Decimal 变体。Go 使用参数化的 `%n` 标识符格式执行检查 SQL，Rust 返回直接反引号拼接的字符串。Go 的 PK-handle 特例基于主键 flag，Rust 模型依赖 `primary_key_is_handle`、索引元数据和列状态。

## 扩展指南

- 新增或调整外键定义规则时，优先修改 `check_foreign_key_definition` 及其小型辅助函数，并在独立的 [`tests/fk/foreign_key_test.rs`](tests/fk/foreign_key_test.rs) 增加正反例；不要把测试内嵌回生产源文件。
- 调整创建/回滚阶段时，修改 `advance_create_foreign_key`，同时断言传入外键、目录副本、`schema_version`、`finished` 和 `rollback_done`。必须保持失败时是否修改状态/版本的语义明确。
- 修改索引覆盖规则时，同时检查父表定义校验、子表新增校验和 `check_index_needed_in_foreign_key`，覆盖复合列顺序、前缀长度、冗余索引与 PK-handle 特例。
- 修改列兼容规则时，同时审查 `foreign_key_column_types_match` 与 `is_acceptable_foreign_key_column_change`；需特别处理当前不可达的第二个整数分支，以及 Rust 类型模型没有 Decimal 变体造成的 Go 对齐边界。
- 若要支持带反引号的标识符或执行真实存量校验，应把 `build_foreign_key_check_sql` 改为可靠的标识符转义/参数化接口，并把执行、事务和会话生命周期放在实际执行层。
- 若需求来自 SQL 用户路径，应先追踪 `executor.rs`、`persistent_actions.rs` 和 job 分发链，决定是接入本模型还是同步修改两套实现；只更改本文件目前主要影响直接调用它的测试/未来调用者。
- 与 Go 行为对齐时至少同步阅读 `foreign_key.go`、[`foreign_key_test.go`](foreign_key_test.go) 和 [`tests/fk/foreign_key_test.go`](tests/fk/foreign_key_test.go)，避免把 Go 的 owner、持久化、错误码或并发语义简化掉。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可通过 `node --file pkg/ddl/foreign_key.rs` 完整读取。
- RustCodeGraph 源码与符号查询：读取 `foreign_key.rs:1-789`；查询并核对 `check_foreign_key_definition`、`check_add_foreign_key_valid`、`advance_create_foreign_key`、`drop_foreign_key`、`check_modify_column_with_foreign_key`、`check_table_has_foreign_key_referred`、`check_index_needed_in_foreign_key`、`check_drop_column_with_foreign_key`、`build_foreign_key_check_sql` 的定义和调用边。
- crate/模块证据：读取 `pkg/ddl/Cargo.toml`，并核对 `pkg/ddl/lib.rs:63` 的 `pub mod foreign_key`。
- Go 对照证据：读取 `pkg/ddl/foreign_key.go`，核对创建/删除状态机、定义与列/索引/表保护、存量 SQL 执行；用 `rg` 定位 `pkg/ddl/foreign_key_test.go` 和 `pkg/ddl/tests/fk/foreign_key_test.go` 的外键回归测试集合。
- Rust 测试证据：读取 `pkg/ddl/foreign_key_test.rs:1-246` 和 `pkg/ddl/tests/fk/foreign_key_test.rs:1-1179`。测试覆盖状态推进、违规行回滚、定义矩阵、引用动作、大小写不敏感、前缀索引、PK-handle、列修改、删除保护和复合检查 SQL。
- 接线证据：RustCodeGraph 对主要公开函数的 `callers` 查询没有生产调用边；`rg` 的直接引用仅落在本文件和上述测试中，而 `executor.rs`/`persistent_actions.rs` 各自存在另一套外键行为。
- 本任务是纯文档分析，未运行 Cargo。交付前另以任务指定命令验证本文恰含十一个固定二级标题，并人工复核每个行为结论都能回指上述源码、调用图、Go 文件或测试。

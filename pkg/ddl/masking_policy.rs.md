# `pkg/ddl/masking_policy.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；crate 入口 `pkg/ddl/lib.rs` 以 `pub mod masking_policy` 对外公开该模块，测试则由同一入口中的 `mod masking_policy_test` 装配。`pkg/ddl/Cargo.toml` 指明 crate 根为 `pkg/ddl/lib.rs`，并用 `package.metadata.porting.go-package = "pkg/ddl"` 标记其 Go 对照包。

它提供列脱敏策略的 Rust 领域模型、目标与表达式校验、字符串编解码、列改名表达式重写，以及一个进程内 `MaskingPolicyStore`。需要区分：该 store 本身不是当前完整应用的持久化 DDL worker；当前 Rust 系统表接线位于 `pkg/ddl/persistent_masking_actions.rs`、`pkg/ddl/persistent_modify_column.rs` 和 `pkg/ddl/persistent_drop_column.rs`，操作 `mysql.tidb_masking_policy`。因此本文件既是可复用的策略语义实现，也是独立内存执行/测试表面，但不能单独证明生产 SQL DDL 已经过 `MaskingPolicyStore`。

## 核心职责

- 用 `MaskingPolicyInfo` 表达一条策略的身份、表列绑定、SQL 表达式、启用状态、类型、限制操作、审计时间和 `SchemaState`。
- 用 `MaskingTarget`、`check_masking_policy_column`、`validate_masking_policy_target` 和 `validate_masking_policy_expression` 阻止视图/序列、临时表、系统库、生成列、不支持类型及跨列引用。
- 用 `build_masking_policy_info` 规范化策略名和对象名，推断状态/类型，并建立初始 `SchemaState::None` 元数据。
- 用 `MaskingPolicyStore` 实现创建、替换、修改、删除、查询，以及 DROP/RENAME/TRUNCATE/MODIFY COLUMN 所需的级联维护。
- 用 `rewrite_masking_policy_expression_column_name` 在列改名时只替换标识符而保留字符串字面量；用四个字符串转换函数对系统表状态、类型和限制位进行编解码。

## 主要符号

- `MaskingPolicyStatus::{Enable, Disable}`：策略开关状态。
- `MaskingPolicyType::{Full, Partial, Null, Date, Custom}`：四种内置 `MASK_*` 顶层函数及自定义表达式分类。
- `MaskingPolicyRestrictOps(u8)`：私有位域；四个公开常量分别占用 `1/2/4/8`，`contains` 查询任一位，`insert` 以按位或合并。
- `MaskingExpression { sql, function_name, referenced_columns }`：调用方已解析的表达式摘要。本文件不构建 SQL AST，校验依赖调用方准确提供引用列和顶层函数名。
- `MaskingPolicyInfo`：完整策略记录，对应 `mysql.tidb_masking_policy` 的语义字段；`id == 0` 表示尚未由 store 分配，创建成功后状态成为 `Public`。
- `MaskingTableKind` 与 `MaskingTarget<'a>`：将数据库属性、对象种类和借用的 `TableInfo` 聚合成校验上下文。
- `MaskingPolicyError`：纯领域错误枚举，覆盖缺失/冲突、对象或列不存在、不合法目标/表达式、未知编码和非法行；其中 `MissingPolicy`、`InvalidRow` 在本文件内没有产生点，是供相邻接线使用的错误契约。
- `is_masking_policy_supported_type`：允许 Integer、Varchar/String、四类 Blob、Timestamp、DateTime、Year。
- `build_masking_policy_info`：构造入口；忽略大小写查找列，先验证列和表达式，再小写化策略名、填充时间与创建者并回填规范对象名。
- `MaskingPolicyStore`：以 `BTreeMap<i64, MaskingPolicyInfo>` 按 ID 保存记录，并维护 `next_id` 和公开的 `schema_version`。
- `rewrite_masking_policy_expression_column_name`：识别裸标识符、反引号标识符以及单双引号字符串的扫描器。
- `masking_policy_status_from_string`、`masking_policy_type_from_string`、`masking_policy_restrict_ops_to_string`、`masking_policy_restrict_ops_from_string`：系统表文本格式适配函数。

## 执行流程

构造策略时，调用方先形成 `MaskingTarget` 和 `MaskingExpression`，然后调用 `build_masking_policy_info`。函数按名称在 `target.table.columns` 中找列，执行列类型/生成列校验，再拒绝空表达式或任何非目标列引用。随后它建立 ID 为 0、状态为 `None` 的记录，默认启用（只有 `explicitly_disabled` 才禁用），按顶层函数名推断类型，最后通过 `validate_masking_policy_target` 再核验对象种类、临时/系统属性、表列 ID，并用表上的规范名称覆盖输入名称。

将记录交给 `MaskingPolicyStore::create` 后，冲突范围是“同一 `table_id` 内同名（忽略大小写）”。同名但列不同返回 `PolicyExistsOnAnotherColumn`；同列且未允许替换返回 `PolicyExists`；允许替换时保留原 ID、`created_at` 和 `created_by`，更新内容并变为 `Public`。新记录递增 `next_id` 后插入。两条成功路径各自把 `schema_version` 加一。

`alter` 只覆盖表达式、状态、类型、限制位和更新时间；`drop` 删除指定 ID；二者成功时递增版本。`by_id`、`by_name`、`by_table` 只读查询。`drop_on_table`、`drop_by_database_name`、`drop_on_column` 通过 `retain` 级联清理；`update_table_id_after_truncate` 保留策略但更新表 ID；`update_names_after_rename` 更新库表名；`sync_modified_column` 先验证新列，再更新匹配策略的列 ID、列名、表名和表达式。

列改名重写逐字符扫描：单双引号内容原样复制并处理反斜杠和成对引号；反引号或 ASCII 字母/下划线开头的 token 才参与忽略大小写匹配。只有完整标识符等于旧列名时才替换，且保留是否使用反引号。未闭合字符串、未闭合反引号或空表达式返回 `InvalidExpression`。

## 数据与状态

`MaskingPolicyInfo` 同时保存稳定 ID（`table_id`、`column_id`）和展示/持久化名称；校验与 rename/modify 流程负责保持两者一致。策略名在构造时转为 ASCII 小写，但 store 的冲突和名称查询仍采用忽略 ASCII 大小写比较。

`MaskingPolicyStore` 的 `BTreeMap` 使 ID 遍历确定，但 `by_name` 没有表 ID 参数：当多个表存在同名策略时，它只返回按 ID 顺序遇到的第一条；需要消歧时应使用 `by_table` 或 ID。独立测试 `duplicate_policy_names_are_scoped_to_a_table` 证明创建冲突确实按表隔离。

版本不变量只覆盖 `create`、`alter`、`drop` 的成功路径。当前 `drop_on_table`、`drop_by_database_name`、`drop_on_column`、`update_table_id_after_truncate`、`update_names_after_rename` 和 `sync_modified_column` 均不会更新 `schema_version`；调用方若把该字段当作所有元数据变化的通知源，必须在接线层补足版本推进或重新审视此契约。

限制操作的文本输出顺序固定为 INSERT、UPDATE、DELETE、CTAS；空位域输出 `NONE`。解析接受空串/`NONE`，未知 token 返回错误。类型字符串的未知值则有意降级为 `Custom`，与状态/限制的严格解析不同。

## 依赖与调用关系

直接 Rust 依赖只有标准库 `BTreeMap` 和同 crate 的 `crate::column::{ColumnInfo, ColumnKind, SchemaState, TableInfo}`；文件无条件编译项、无异步运行时或第三方库调用。`pkg/ddl/lib.rs` 将它作为公开模块导出，并把 `pkg/ddl/masking_policy_test.rs` 作为独立测试模块装配，符合测试与生产源分离约束。

RustCodeGraph 对 `build_masking_policy_info` 给出的下游边为 `check_masking_policy_column`、`validate_masking_policy_expression`、`masking_policy_type_from_expression` 和 `validate_masking_policy_target`；对 `rewrite_masking_policy_expression_column_name` 的明确上游包括 `MaskingPolicyStore::sync_modified_column` 与独立测试 `renaming_a_column_does_not_rewrite_string_literals`。图中没有发现生产模块调用 `MaskingPolicyStore::create`/`alter`/`drop` 的可靠边，现有明确调用来自 `pkg/ddl/masking_policy_test.rs`。

完整应用的持久化关联是并行实现而非此 store 的直接下游：`persistent_actions.rs` 将 truncate/rename/drop table 分发给 `persistent_masking_actions.rs`；`persistent_modify_column.rs` 同步修改列后的策略；`persistent_drop_column.rs` 清理列策略。它们执行针对 `mysql.tidb_masking_policy` 的 SQL，并由 job worker 上下文提供时间戳和错误处理。

## 错误处理与边界

所有可失败函数使用 `Result<_, MaskingPolicyError>`，不记录日志也不包装来源链。校验顺序会决定外显错误：构造时列名不存在返回 `ColumnNotFound(0)`，找到后先报生成列/类型错误，再报表达式错误，最后报目标表属性或 ID 错误。`validate_masking_policy_target` 先检查对象/临时/系统属性，再核验表 ID 和列 ID。

表达式校验只检查非空和 `referenced_columns` 是否均为目标列，不在本文件内解析 SQL、验证函数参数或验证表达式类型；其正确性依赖上游解析结果。重写器同样不是完整 SQL lexer/parser：它支持常见引号与 ASCII 标识符，但不处理反引号转义、注释、Unicode 裸标识符或所有 SQL mode 差异。独立测试只直接证明字符串字面量不会被误改。

级联清理和 rename/truncate 更新返回 `()`，无法报告“未命中”；且它们不推进 store 的 `schema_version`。`sync_modified_column` 在真正遍历策略前就验证 `new_column`，所以即使没有关联策略，不支持的新类型也会报错；生产接线是否只在确认策略存在后调用需由相邻模块保证。

从 DDL 生命周期看，本文件的内存 store 没有 job 持久化、owner failover、回滚、schema diff、MDL、reorg/backfill 或 follower schema 同步。其状态只有创建前 `None` 到创建后 `Public`，没有多阶段 schema state。生产 job/系统表行为必须以持久化接线为准，不能由此文件推断。

## 并发与资源生命周期

`MaskingPolicyStore` 的修改方法均要求 `&mut self`，本文件没有 `Mutex`、原子变量、通道、后台任务或 `unsafe`；跨线程共享时必须由外层同步。借用查询返回与 store 生命周期绑定的引用，`by_table` 只分配一个引用向量，不复制策略。

策略记录及字符串完全由 store 拥有；`MaskingTarget` 只在构造/校验期间借用 `TableInfo`。表达式重写按字符数分配 `Vec<char>` 和容量近似输入长度的 `String`，时间复杂度线性；批量级联/查找扫描全部策略，复杂度为 O(n)。`BTreeMap` 的 ID 插入/删除为 O(log n)，但同名检测和按表/名称查询仍为 O(n)。

本文件没有事务边界：一次内存方法调用要么在错误前未修改，要么同步完成修改；但多个方法的组合不具备原子性，也没有崩溃恢复。生产系统表路径的事务、job 重试和版本同步属于相邻 worker 接线。

## 与 Go 版本的对应关系

Go 对照为 `pkg/ddl/masking_policy.go`，回归测试为 `pkg/ddl/masking_policy_test.go`。字段模型、目标限制、同表同名冲突、默认启用、内置类型推断、限制位文本格式、rename/truncate/drop/modify-column 联动及系统表 `mysql.tidb_masking_policy` 均有直接对应。

主要差异如下：

- Go 的 `onCreateMaskingPolicy`、`onAlterMaskingPolicy`、`onDropMaskingPolicy` 是 job worker 入口，会取消失败 job、写系统表、更新 schema version 并完成 job；Rust 本文件的 `MaskingPolicyStore` 是进程内模型，不提供这些持久化/恢复语义。Rust 的实际系统表逻辑分散在 `persistent_masking_actions.rs` 等文件。
- Go `validateMaskingPolicyTarget` 从 `InfoCache` 按 ID 取最新 schema/table/column；Rust 接收调用方提供的 `MaskingTarget`，因此新鲜度由调用方负责。
- Go `validateMaskingPolicyExpression` 用表达式解析器和 `ExtractColumns` 得到真实列 ID；Rust 只消费 `MaskingExpression.referenced_columns`。Rust 调用方若遗漏引用，会弱化保护。
- Go 的支持类型包含所有 numeric、time 以及 duration；Rust 枚举列表明确但较窄，当前未列 Float/Decimal/Duration 等类别。新增类型必须先核对 `ColumnKind` 定义及 Go 的 `types.IsTypeNumeric/IsTypeTime`，不能仅按注释扩展。
- Go 的列名重写解析 `SELECT <expr>` 并用 AST visitor 只改 `ColumnNameExpr`，再标准化输出；Rust 使用轻量词法扫描，保留更多原始格式但 SQL 语法覆盖更窄。
- Go 从系统表读取时把 `State` 设为 `Public` 并解析时间；Rust 本文件没有系统表行到 `MaskingPolicyInfo` 的完整转换，`InvalidRow` 也未在本文件使用。

Go 测试覆盖基本 create/alter/drop、CASE、IF NOT EXISTS、表和跨库 rename、列 rename、不支持类型、跨列表达式、truncate 保留及 drop database 清理。Rust 独立测试当前只直接覆盖同名跨表、字符串字面量改名和一个 Executor 层 rename/不兼容类型场景，覆盖面明显小于 Go。

## 扩展指南

新增策略状态、类型或限制操作时，应同步修改对应枚举/位常量、推断函数、字符串双向转换、系统表持久化接线和 `pkg/ddl/masking_policy_test.rs`；同时对照 `pkg/ddl/masking_policy.go` 与 `pkg/ddl/masking_policy_test.go`，保证文本值、默认值和错误分支兼容。位域当前只有 `u8`，增加超过 8 个标志前需要扩容并评估存储格式。

新增可支持列类型时，入口是 `is_masking_policy_supported_type`，必须同时验证构造与 modify-column 路径，并补独立 Rust 测试；特别要核对 Go 的 numeric/time 范围。改变目标规则时应同步 `validate_masking_policy_target` 和实际持久化 worker，不能只改内存 store。

增强表达式安全性时，优先让 `MaskingExpression` 来自可靠 parser/AST 并按列 ID 校验；若扩展改名语法，应补字符串、转义、反引号、注释、Unicode、函数名与相似前缀测试。不要把测试内嵌到本文件，应继续放在 `pkg/ddl/masking_policy_test.rs`。

若将 `MaskingPolicyStore` 接入并发或生产持久化路径，需要先定义版本推进不变量、事务/重试、崩溃恢复和 owner failover；并明确所有级联方法是否递增 `schema_version`。若只扩展当前生产 SQL 路径，应优先修改 `persistent_masking_actions.rs`、`persistent_modify_column.rs` 或 `persistent_drop_column.rs`，同时维持本文件领域语义与 Go 行为一致。

性能风险集中在全表扫描：策略量增大时可增加 `(table_id, lower(name))`、`table_id`、`(table_id, column_id)` 等辅助索引，但必须保证替换、rename、truncate 和 drop 同步更新，避免多索引漂移。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标源文件共 668 行。查询了 `build_masking_policy_info` 的定义/下游、目标文件全貌、独立测试文件和 Go 对照文件，并核对表达式重写的调用边。
- 生产源：`pkg/ddl/masking_policy.rs`；crate 边界：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`；同包契约：`pkg/ddl/doc.go`。
- Rust 独立测试：`pkg/ddl/masking_policy_test.rs`，覆盖同名策略按表隔离、列改名不改字符串字面量，以及跨库 rename 后不兼容类型变更被拒绝。
- Rust 生产接线证据：`pkg/ddl/persistent_masking_actions.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/ddl/persistent_drop_column.rs`、`pkg/ddl/job_worker.rs`。
- Go 对照与测试：`pkg/ddl/masking_policy.go`、`pkg/ddl/masking_policy_test.go`。Go 源码直接证明 job worker、InfoCache、表达式 AST、系统表 SQL、级联维护和文本解析语义。
- DDL 模块背景只以 `pkg/ddl/doc.go` 和 `docs/agents/ddl/README.md` 作为导航；本文关于当前实现的结论均以上述源码/测试为准。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核唯一新增生产物、无源码修改、无无依据的“已支持”结论。

# `pkg/table/column.rs`

源文件：[`column.rs`](./column.rs)；同包独立测试：[`column_test.rs`](./column_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；Go 对照：[`column.go`](./column.go)、[`column_test.go`](./column_test.go)。

## 文件定位

本文件属于 `astersql-table` crate（见 [`Cargo.toml`](./Cargo.toml)），由 [`lib.rs`](./lib.rs) 以 `pub mod column` 声明并通过 `pub use column::*` 在 crate 根重导出。它位于规范列元数据 `model::ColumnInfo` 与表读写执行之间：不重新拥有名称、类型、标志、默认值等元数据，而用 `Column` 附加可执行的生成列表达式和默认表达式状态，并集中实现列查找、Datum 类型转换、默认值解析、约束检查、`DESC` 描述和虚拟生成列物化。

这不是纯数据结构文件。其转换与默认值逻辑参与行解码、在线 DDL 回填、规划器默认值重写和导入编码。例如 [`raw_row.rs`](./raw_row.rs) 的 `DecodeRawRowDataWithMeta` 调用 `GetChangingColVal`/`GetColDefaultValue` 恢复旧行缺失列；`pkg/session/runtime/modify_column_backfill.rs` 调用 `GetColOriginDefaultValue` 与 `CastValue`；`pkg/planner/core/expression_rewriter.rs` 调用 crate 根重导出的 `GetColDefaultValue`。

## 核心职责

- 用 `Column` 包装 `Box<model::ColumnInfo>`，保持 model 元数据为唯一权威，同时承载 `GeneratedExpr`、`DefaultExpr` 两类可执行 AST 状态。
- 提供按原名或小写名查列、批量查列、`_tidb_rowid` 隐式 handle 合成，以及 ON UPDATE 列筛选。
- 在 table 边界执行 Datum 到列类型的转换，并保持 Go `(casted, err)` 语义：失败时 `CastError` 仍保存可用于诊断或兼容处理的部分转换值。
- 根据 SQL Mode、错误组级别、字符集、排序规则和列标志处理截断、非法字符、零日期、坏 NULL 与无默认值。
- 生成 `DESC`/`SHOW COLUMNS` 所需的 `ColDesc`，以及各 MySQL 类型的零值。
- 求取普通、origin 和表达式默认值，并为在线 DDL changing column 从旧列值或缓存默认值恢复数据。
- 逐行求值并写回虚拟生成列；该能力在本文件和测试中存在，但本次 RustCodeGraph/源码搜索未找到 Rust 跨文件生产调用者，不能据此宣称已接入所有 Go 执行器路径。

## 主要符号

- `ExprNodeCtor`、`ClonableExprNode`、`NewClonableExprNode`：线程安全的 AST 重建器。`Clone()` 优先调用构造闭包以生成新 AST，无构造器时克隆 `internal`；`Internal()` 始终返回内部节点副本。
- `Column`：包含 `ColumnInfo`、`GeneratedExpr`、`DefaultExpr`。`New` 返回 `Arc<Column>`；`GetFlag`、`String`、`ToInfo` 分别暴露标志、紧凑描述和只读规范元数据。
- `FindCol`/`FindColLowerCase`/`FindCols`/`FindColumns`/`FindOnUpdateCols`：保留 `Arc` 身份的查找辅助。两种批量 API 的失败信息不同：`FindCols` 返回缺失名称，`FindColumns` 返回缺失下标。
- `CastContext`、`CastErrorKind`、`CastError`、`CastResult`：把求值上下文收窄为转换所需能力，并显式表达 `Truncated`、`InvalidCharacter`、`WrongDatetime`、`Other` 四类失败。所有 `EvalContext` 自动实现 `CastContext`。
- `CastValue`、`CastColumnValueWithStrictMode`、`CastColumnValue`、`castColumnValue`：三种入口汇聚到核心转换。`CastColumnValue` 额外处理旧/新 collation 模式下 ENUM/SET 的 binary 匹配兼容。
- `ColDesc`、`NewColDesc`、`ColDescFieldNames`：构造 MySQL 风格列描述；`defaultPrivileges` 固定为 `select,insert,update,references`。
- `CheckOnce`、`Column::CheckNotNull`、`Column::HandleBadNull`：重复名与非空约束。VECTOR 非空列没有可用零值，始终拒绝 NULL；其他类型可由错误上下文降级为警告并填零值。
- `HandleTableInfo`：只暴露 `PKIsHandle`/`IsCommonHandle`，并为 `model::TableInfo` 实现，以隔离列逻辑所需的最小表元数据视图。
- `GetColOriginDefaultValue`、`GetColOriginDefaultValueWithoutStrictSQLMode`、`CheckNoDefaultValueForInsert`、`GetColDefaultValue`、`EvalColDefaultExpr`：默认值与表达式默认值入口。
- `GetChangingColVal`：在线 DDL changing column 恢复逻辑；优先转换依赖旧列值，否则按目标列 offset 缓存并复用默认值。
- `GetZeroValue`、`OptionalFsp`、`FillVirtualColumnValue`：类型零值、时间小数秒显示和虚拟生成列物化。

## 执行流程

1. 列元数据进入 table 层时，`ToColumn`/`Column::New` 将 `model::ColumnInfo` 放入 `Arc<Column>`；调用者通过 crate 根重导出或 `column` 模块使用它。
2. 查找路径先比较 `ColumnInfo.Name.O`（ASCII 大小写不敏感）或 `Name.L`（精确小写）。批量查找遇到 `_tidb_rowid` 且 `pk_is_handle == false` 时调用 `extra_handle_column`，以 `cols.len()` 为 offset 合成额外 handle 列；否则立即返回首个缺失项。
3. 转换路径由 `CastValue` 或 `CastColumnValue` 提取 type/error/SQL-mode 上下文后进入 `castColumnValue`。`convert_with_partial` 先调用 `Datum::ConvertTo`；失败则按目标类型分类，并由 `recover_partial` 恢复有界整数、合法字符串前缀、零时间或默认 Datum，装入 `CastError`。
4. `castColumnValue` 依次处理 `return_error` 早退、截断消息改写、日期/时间 SQL Mode、非法字符错误格式化、`Context::HandleTruncate` 和 `force_ignore_truncate`。成功落地到非二进制 CHAR 时，`truncateTrailingSpaces` 去掉尾部空格。
5. 默认值路径由 `GetColDefaultValue` 判断 `DefaultIsExpr`：表达式字符串经 `ParseSimpleExpr`、`Eval`、`CastColumnValue`；普通值进入 `getColDefaultValue`。时间类型直接按字段类型转换；版本 1 及以上的非零、非 `CURRENT_TIMESTAMP` TIMESTAMP 默认值先按 UTC 解码，再转为会话时区。
6. 缺省值为 `None` 时，`getColDefaultValueFromNil` 依序处理可空列、NOT NULL ENUM 首元素、自增列零值、非严格模式警告加零值、错误上下文降级，最终才报无默认值错误。
7. 旧行解码时，`DecodeRawRowDataWithMeta` 对缺失 changing column 调用 `GetChangingColVal`：若 row map 含依赖列 ID，则转换旧值；否则按目标 offset 从 `default_values` 取值或只计算一次默认值。
8. `FillVirtualColumnValue` 为每个虚拟列和输入行调用 `EvalVirtualColumn`，再以忽略截断方式转换；允许负数转无符号时把负输入回退为列零值，NOT NULL/PREVENT NULL 列的 NULL 也回退零值，最后用 `Chunk::SetCol` 替换目标列。

## 数据与状态

`ColumnInfo` 是名称、ID、offset、字段类型、标志、默认值、版本和 change-state 的规范来源；`Column` 不复制这些字段。`Arc<Column>` 让表结构中的列对象可廉价共享，查找函数克隆的是 `Arc` 而非列内容。`GeneratedExpr` 使用 `Arc<ClonableExprNode>`，构造器要求 `Send + Sync`；`DefaultExpr` 则直接持有一个 AST 节点。

转换状态是调用局部的：`CastError` 同时拥有部分转换后的 `Datum`、底层错误和稳定分类；`castColumnValue` 会根据错误上下文替换其中的值或错误文本。默认值恢复中的 `default_values: &mut [Option<Datum>]` 是由调用者按完整 schema 分配的行级缓存，索引取自 `ColumnInfo.Offset`，用于避免一行内重复计算默认值。

`FillVirtualColumnValue` 额外分配一个只容纳虚拟返回类型的临时 `Chunk`，完成所有行求值后把列向量写回原 `request`。它不保存跨调用状态；警告通过 `EvalContext`/`ErrorContext` 的内部处理器累积。

## 依赖与调用关系

`Cargo.toml` 表明本文件直接跨越多个 crate 边界：`astersql-meta-model` 提供 `ColumnInfo`/`TableInfo`，`astersql-types` 提供 `Datum`/`FieldType`/转换上下文，`astersql-expression` 提供 `BuildContext`、表达式构建与求值，`astersql-errctx`/`astersql-errors` 提供错误分组和警告处理，`astersql-util-chunk` 提供行与列式 Chunk，parser AST/mysql crate 提供 AST 与 SQL Mode/类型常量。

RustCodeGraph 索引显示目标文件有 99 个符号，并确认包内主边：`CastValue`、`CastColumnValueWithStrictMode`、`CastColumnValue` → `castColumnValue`；`GetColDefaultValue` → `getColDefaultValue` 或 `getColDefaultExprValue`；`EvalColDefaultExpr`/默认表达式路径 → `CastColumnValue`；`DecodeRawRowDataWithMeta` → `GetChangingColVal`/`GetColDefaultValue`。源码搜索还确认 `modify_column_backfill.rs`、`expression_rewriter.rs`、`physical_batch_point_get.rs`、`persistent_drop_column.rs` 和 importer 代码使用列查找、转换或默认值能力。

Go 图中 `FillVirtualColumnValue` 被 table reader、point get、MPP gather、DDL index cop 和 analyze sampling 等路径调用；Rust 图只显示其自身定义，没有跨文件调用者。因此文档把它描述为已实现 API，而不是已完整接线的执行主链。

## 错误处理与边界

- `CastError` 保留部分值是公共契约；新增调用者不能只把它降成字符串而丢失 `casted()`/`kind()`，除非调用边界只接受 `types::errors::Error`。
- `return_error` 在首次转换失败后立即返回；`force_ignore_truncate` 仅在错误上下文处理之后决定是否忽略剩余错误。两者含义不同，不能互换。
- 字符集检查只显式识别 ASCII、UTF-8/UTF8MB3/UTF8MB4；其他字符集依赖底层 `ConvertTo`，`first_invalid_character` 不自行判错。错误文本把首个非法字节编码为大写 `\xNN`。
- 零日期逻辑同时依赖目标类型、`STRICT`、`NO_ZERO_DATE`、`NO_ZERO_IN_DATE` 和错误组级别。TIMESTAMP 的非法/零值有专门分支，不能用普通截断逻辑替代。
- `FindCols`/`FindColumns` 只在非 PK handle 表中合成 `_tidb_rowid`。二者一个按原名大小写不敏感匹配，一个要求调用方传入规范小写名。
- `GetChangingColVal` 对缺失 change-state、负 offset、越界依赖/目标 offset 都返回显式错误。默认值缓存长度必须覆盖完整 schema，而不只是当前投影列；[`column_test.rs`](./column_test.rs) 的 hidden-column 用例验证了这一点。
- `FillVirtualColumnValue` 将 `i32` 列下标直接转为 `usize` 后索引多个切片；当前实现依赖调用者提供非负且对齐的下标和返回类型数量，函数自身没有完整边界校验。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、事务或通道。共享主要由不可变 `Arc` 完成：`Column::New` 和 `NewClonableExprNode` 返回 `Arc`，表达式构造闭包必须 `Send + Sync`，但 `ColumnInfo` 仅通过 `ToInfo` 暴露共享只读借用。测试若需修改元数据，必须先保证 `Arc` 唯一再用 `Arc::get_mut`，这反映了生产侧“建表后共享读取”的预期。

警告收集器和求值上下文由调用者拥有；本文件只在一次转换/默认值求值期间借用。`GetChangingColVal` 借用调用者的可变缓存，`FillVirtualColumnValue` 借用并原地更新调用者的 `Chunk`，临时 chunk 与所有中间 Datum 在函数返回时释放。表达式 AST 的 `Clone()` 每次重建或深克隆节点，避免多个消费者意外共享可变 AST 内部状态。

## 与 Go 版本的对应关系

Rust 文件总体按 [`column.go`](./column.go) 的同名 API 移植：`Column`/`ClonableExprNode`、查找函数、`CastValue` 系列、`ColDesc`、非空检查、默认值链、`GetZeroValue`、`OptionalFsp` 和 `FillVirtualColumnValue` 均有直接对应。Rust 用 `Arc`/`Box` 表达 Go 指针所有权，用 `Option` 代替 nil，用 `Result<Datum, CastError>` 保留 Go 的 `(casted, err)` 双返回语义，并用 `CastContext`/`HandleTableInfo` 收窄依赖面。

重要差异包括：Rust 的 `Column` 组合 `ColumnInfo`，Go 的 `Column` 直接嵌入它；Rust `ColDesc.DefaultValue` 是 `Option<model::DefaultValue>`，而 Go 使用 `any`；Rust 的表达式默认值通过 expression crate 的 `ParseSimpleExpr`/`BuildSimpleExpr` 接口实现，Go 通过 parser 与 `EvalSimpleAst`；Rust 只对 version 1+ TIMESTAMP 默认值显式按 UTC 解码，Go 还对旧版本使用系统时区。该旧版本时区差异在当前 Rust 测试中没有对应用例，应视为兼容性关注点而非已验证等价。

[`column_test.rs`](./column_test.rs) 对齐 Go 的 `TestString`、`TestFind`、`TestCheck`、`TestHandleBadNull`、`TestDesc`、`TestGetZeroValue`、`TestCastValue`、`TestGetDefaultValue`、`TestCastValueStrict`，并增加 changing-column/row decode、legacy ENUM/SET collation 等 Rust 回归。迁移测试还验证 `_tidb_rowid`、部分转换值与错误分类、零日期模式及非法字符前缀。

## 扩展指南

- 新增列类型转换行为时，从 `cast_error_kind`、`recover_partial`、`castColumnValue` 和 `GetZeroValue` 四处检查完整性；同时在独立的 [`column_test.rs`](./column_test.rs) 或 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 增加成功、严格失败、警告降级及部分值断言，不要把测试嵌入生产文件。
- 新增字符集支持时，同步检查 `valid_character_prefix_len`、`first_invalid_character`、错误转义格式和 collation 保存；风险是多字节边界截断或错误位置与 Go 不一致。
- 调整默认值时，同步检查普通/origin/表达式默认值、TIMESTAMP 版本与时区、ENUM 首元素、自增列、严格模式和 `DecodeRawRowDataWithMeta` 的缓存调用；性能上避免每行重复解析固定表达式或重复计算默认值。
- 调整虚拟列填充时，应先确认 Rust 执行器接线现状，验证 indexes、return types、expression columns、table columns 四组输入对齐，并补充独立测试覆盖负下标/越界、负转无符号、NULL 非空回退及多行多列写回。
- 扩展 `Column` 字段时保持 `ColumnInfo` 为规范元数据来源；若引入可变共享状态，需要重新评估 `Arc<Column>` 的线程安全与克隆语义，而不是依赖当前无锁只读假设。
- 所有行为变化应与 [`column.go`](./column.go) 和 [`column_test.go`](./column_test.go) 对照；若有意产生差异，文档和测试必须明确说明兼容边界。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/table/column.rs` 确认目标文件已索引且含 99 个符号；`query`/`explore` 核对 `CastColumnValue`、`GetColDefaultValue`、`FillVirtualColumnValue`、`NewColDesc`、`FindCols` 的定义和上述调用边；`node --file pkg/table/column.rs --offset 780 --limit 700` 核对文件后半段源码。
- 直接读取：[`column.rs`](./column.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`column_test.rs`](./column_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`raw_row.rs`](./raw_row.rs)、[`column.go`](./column.go)、[`column_test.go`](./column_test.go)。目标包不存在 `doc.go`，因此没有可额外读取的包契约文件。
- 结构校验要求：本文恰含“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”十一个二级章节。
- 本任务是只读源码分析与文档新增，按计划不运行 Cargo；行为结论来自源码、调用图和既有独立测试，不声称本次重新执行了这些测试。

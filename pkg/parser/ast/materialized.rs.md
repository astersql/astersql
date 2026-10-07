# `pkg/parser/ast/materialized.rs`

## 文件定位

本文件属于 `astersql-parser-ast` crate。`pkg/parser/ast/Cargo.toml` 将该 crate 的入口设为 `lib.rs`，而 `pkg/parser/ast/lib.rs:35` 以 `pub mod materialized;` 公开本模块。AST 数据结构本身集中定义在 `lib.rs:1939-2120`；本文件不负责词法解析、语法归约或执行物化视图任务，而是为这些既有类型补充两类行为：将 AST 恢复成 SQL 文本，以及把刷新语法组合归一化为具体的 `RefreshMaterializedViewMode`。

因此它处在“解析后的 AST 表示”与“可读、可重放的 SQL 文本/执行模式选择”之间。当前可直接验证的使用面是同 crate 的独立测试 `pkg/parser/ast/materialized_test.rs`；RustCodeGraph 能确认目标文件被索引并由其他文件引用，但未为这些同名 inherent `restore` 方法解析出可归属的应用运行时调用边，不能据此宣称它已经接入某条具体执行主链。

## 核心职责

- 为 `RefreshMaterializedViewType`、`RefreshMaterializedViewCompleteType` 和 `RefreshMaterializedViewMode` 实现稳定的大写文本表示；未知枚举值在展示时统一写为 `UNKNOWN`（`materialized.rs:17-47`）。
- 为 PURGE、CANCEL、REFRESH、IMPLEMENT、CREATE、ALTER、DROP 等物化视图及物化视图日志节点生成 SQL（`materialized.rs:49-142, 188-428`）。
- 为标识符和字符串字面量做最小必要转义：反引号标识符中的反引号翻倍，字符串中的反斜杠和单引号转义（`quote_name`、`quote_string`）。
- 校验恢复 SQL 所需的结构性前置条件，例如必需表名、`CREATE ... AS` 的查询体、内部 IMPLEMENT 语句的刷新节点，以及 COMPLETE 刷新的具体实现方式。
- 递归委托 `crate::sql_restore::restore_expr` 与 `restore_node` 恢复表达式和查询节点，而不是在本文件重复实现通用 SQL AST 恢复器。
- 为共享的 `TableOption` 增加本文件所需的有限恢复分支；未覆盖的 option 明确返回错误，而不是静默输出不完整 SQL（`TableOption::restore`）。

## 主要符号

- `Display for RefreshMaterializedViewType`、`RefreshMaterializedViewCompleteType`、`RefreshMaterializedViewMode`：分别输出刷新策略、COMPLETE 子类型和归一化执行模式。
- `RefreshMaterializedViewStmt::mode() -> Result<RefreshMaterializedViewMode, String>`：把 `FAST` 或 `COMPLETE + {IN PLACE, OUT OF PLACE, DELTA APPLY}` 映射为四个具体模式；拒绝未知类型和缺少 COMPLETE 子类型的组合。
- `restore() -> Result<String, String>` 公共方法族：定义在 `PurgeMaterializedViewLogStmt`、`CancelMaterializedViewJobStmt`、`RefreshMaterializedViewStmt`、`RefreshMaterializedViewImplementStmt`、`TableOption`、`MViewRefreshClause`、`MLogPurgeClause`、`CreateMaterializedViewStmt`、`CreateMaterializedViewLogStmt`、`AlterMaterializedViewAction`、`AlterMaterializedViewStmt`、`AlterMaterializedViewLogAction`、`AlterMaterializedViewLogStmt`、`DropMaterializedViewStmt` 和 `DropMaterializedViewLogStmt` 上。
- `MLogAccumulationAlertClause::restore() -> String`：该节点只有一个无失败分支，生成 `ALERT ROWS <Rows>`。
- `quote_name`、`quote_string`、`table_name`：内部格式化辅助函数。`table_name` 省略空 schema，否则生成带反引号的 `schema.table`。
- `required_name(&Option<TableName>, context)`：统一把缺失必需名称转换成包含字段上下文的错误。
- 相关数据类型位于 `pkg/parser/ast/lib.rs:1939-2120`，包括各 statement/clause/action、刷新枚举和可选子节点；本文件没有自行声明 struct、enum、trait、常量或条件编译项。

## 执行流程

1. 调用者先持有 `lib.rs` 中构造好的 AST 节点，再调用相应的 `restore()`；刷新执行方也可调用 `RefreshMaterializedViewStmt::mode()` 获取归一化模式。
2. 顶层 statement 先写固定 SQL 前缀，并通过 `required_name` 恢复必需的视图名或表名。名称恢复经 `table_name`/`quote_name` 处理 schema 和反引号。
3. 可选字段按语法顺序追加：例如 `RefreshMaterializedViewStmt::restore` 依次处理异步模式、刷新类型、COMPLETE 子类型、`AS OF TIMESTAMP` 和观测模式；`CreateMaterializedViewStmt::restore` 依次处理列、注释、表选项、刷新子句、属性与查询体。
4. 嵌套表达式交给 `restore_expr`。创建物化视图的 `Select: Box<dyn Node>` 交给 `restore_node`，所以查询恢复能力及其错误边界来自通用恢复器。
5. action 列表通过 `enumerate` 逐项恢复，成功后以 `, ` 连接；任一子项失败即短路并在错误中加入数组下标。
6. `MLogPurgeClause::restore` 遇到 `Immediate` 时立即返回 `PURGE IMMEDIATE`，不会再追加 `StartWith`/`Next`；IMPLEMENT 语句只在两个可选 TSO 大于零时输出相应时间戳子句。
7. 最终返回完整 `String`；本文件不解析生成的 SQL、不调度 DDL，也不修改 AST。

## 数据与状态

所有行为都只读取 `&self`，并在局部 `String` 中累积结果。长期状态保存在 `lib.rs` 的 AST 字段中：名称为 `Option<TableName>`，列和 action 为 `Vec`，可选表达式/子句为 `Option`，TSO 为 `u64`，任务 ID 与告警行数为 `i64`。

关键不变量是“语法必需字段在恢复前必须存在”：视图/表名、`CreateMaterializedViewStmt.Select`、`RefreshMaterializedViewImplementStmt.RefreshStmt` 不能为空；COMPLETE 刷新必须给出具体 complete type。相反，空列/action 列表目前并不会由本文件拒绝，而会生成空括号、空 action 尾部等文本，语法有效性需由构造/解析层保证。

`Unknown(value)` 保留无法识别的原始数值，便于类型表示保持前向兼容；但具体处理并不一致：展示实现输出 `UNKNOWN`，`RefreshMaterializedViewStmt::restore` 对未知顶层类型也输出 `UNKNOWN`，而 `mode()` 会报错，CANCEL 的未知类型同样报错。

## 依赖与调用关系

- crate 内部定义依赖：所有 receiver 类型、`TableName`、`TableOption` 和枚举来自 crate 根，即 `pkg/parser/ast/lib.rs`。
- 直接下游调用：`MViewRefreshClause`、`MLogPurgeClause`、刷新语句和 ALTER refresh action 调用 `crate::sql_restore::restore_expr`；`CreateMaterializedViewStmt` 调用 `crate::sql_restore::restore_node`；顶层 restore 还组合调用 `TableOption::restore`、子 clause/action 的 `restore`。
- 格式化依赖只有标准库 `std::fmt` 和 `String` 操作；本文件未直接使用 `Cargo.toml` 中的外部 crate。通用 AST 类型则由整个 `astersql-parser-ast` crate 依赖 `parser-auth`、`parser-charset`、`parser-mysql`、`parser-types`、`serde`、`serde_json` 和 `url`。
- 已验证上游：`pkg/parser/ast/materialized_test.rs` 直接调用本文件的 restore 方法，并验证结构化 SELECT、表达式、CTE、UNION、子查询和错误分支。模块在 `lib.rs:35` 公开，测试在 `lib.rs:5109` 作为独立测试模块接入。
- RustCodeGraph `node --file` 报告目标文件被 18 个文件使用，但其概览只展示部分文件，且对 inherent `restore`/`mode` 未返回可可靠归属的 callers/callees；故具体生产调用者在本次证据范围内记为“未验证”。

## 错误处理与边界

公开的可失败方法统一使用 `Result<_, String>`。`required_name`、缺失查询体和缺失 refresh statement 产生直接、带字段路径的错误；嵌套恢复错误用 `map_err` 增加 statement、字段或 action/option 下标，保留错误发生位置。

明确拒绝的输入包括：未知 CANCEL 类型、无法映射的刷新模式、COMPLETE 未指定实现方式、`TableOption` 不属于 `ShardRowID`/`PreSplitRegion`/`EngineAttribute`/`StorageClass`/`StartTransaction`，以及下游表达式/查询恢复失败。`materialized_test.rs` 验证了缺失 SELECT 的错误，也通过通用恢复器验证不支持的 MATCH modifier 会传播错误。

需要注意的边界是 `AlterMaterializedViewLogActionType::Purge` 在 `Purge == None` 时返回空字符串，未知刷新类型在 SQL 恢复时输出 `UNKNOWN`，空 action 列表也不报错。这些行为与当前源码一致，扩展时不应未经兼容性评估自行收紧。名称转义只使用 `TableName` 的原始 `O` 字段；字符串转义规则是本地实现，应与 Go `format.RestoreCtx` 的行为持续对照。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务、文件句柄或网络资源。所有方法借用不可变 AST、分配并返回独立字符串；除内存分配外没有需要显式清理的资源，也没有跨调用共享的可变状态。

生命周期边界仅体现在递归借用：子表达式和查询节点在恢复期间被只读访问，错误会立即沿调用栈返回。时间复杂度主要随列数、option/action 数和嵌套 AST 大小线性增长；多次 `push_str` 与中间 `Vec<String>` 会产生分配，但当前没有缓存或并行化语义。

## 与 Go 版本的对应关系

`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/parser/ast`。主要 Go 对照分布在 `pkg/parser/ast/ddl.go:1780-2330` 与 `pkg/parser/ast/misc.go:660-929`：前者包含 CREATE/ALTER/DROP 物化视图及日志，后者包含 PURGE、CANCEL、REFRESH、IMPLEMENT、刷新枚举和 `Mode`。

Rust 基本保持 Go 的子句顺序、逗号连接、`PURGE IMMEDIATE` 短路、可选 TSO 仅在大于零时输出、COMPLETE 模式校验以及嵌套错误上下文。`RefreshMaterializedViewStmt::mode` 不需要处理 Go 的 nil receiver，因为 Rust 方法只能作用于现存值；缺失的可选内部节点仍通过 `Option` 显式报错。

实现机制存在必要差异：Go 写入 `format.RestoreCtx` 并通过 `Node.Restore` 动态分派；Rust 返回拥有所有权的 `String`，名称/字符串转义由本文件辅助函数完成，表达式和查询则交给 `sql_restore`。Go 的未知枚举通常由整数默认分支产生，Rust 用 `Unknown(i32/u8)` 显式携带值。Rust 的 `TableOption::restore` 仅覆盖本文件当前需要的五类 option，不能视为 Go `TableOption.Restore` 的完整移植。

## 扩展指南

- 新增刷新策略或 COMPLETE 实现时，必须同步修改 `lib.rs` 的枚举、本文件三个 `Display`、`RefreshMaterializedViewStmt::mode` 和 `restore`，并在独立的 `materialized_test.rs` 覆盖成功、未知值和非法组合；同时对照 `misc.go`，避免 Rust/Go 模式编号与文本漂移。
- 新增 CREATE/ALTER 子句时，应在对应 AST 数据结构和本文件顶层 restore 的正确语法位置接入；若含表达式或查询，复用 `sql_restore` 并追加字段上下文，不要在本文件另写一套表达式恢复逻辑。
- 扩展 `TableOption::restore` 前先核对共享类型的所有使用者和 Go `TableOption.Restore`，因为这是对全 crate 类型增加的公共方法；需要明确区分“物化视图允许的 option”与“通用 TableOption 能恢复的 option”。
- 保持 Rust 源码与测试分文件：生产修改放在 `materialized.rs`/`lib.rs`，回归测试放在同目录 `materialized_test.rs`。重点补充缺失名称、空 action、无 Purge 节点、引号/反斜杠转义、schema 限定名和下游恢复错误。
- 兼容风险主要是输出空格、关键字、引号及错误文本改变；性能风险主要来自大型列/action 列表与深层查询的字符串分配。若优化为直接写 formatter，应先用现有精确字符串测试锁定行为。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`node --file pkg/parser/ast/materialized.rs --offset 1 --limit 400` 与 `--offset 401 --limit 80` 覆盖目标文件 428 行；`query RefreshMaterializedViewStmt --kind struct`、`query CreateMaterializedViewStmt --kind struct` 将 Rust 类型定位到 `lib.rs`，并定位 Go 对照方法。对 inherent 方法的 callers/callees 查询未形成可用静态调用边，文中已将生产调用者标为未验证。
- Rust 源码：`pkg/parser/ast/materialized.rs`（全部行为）、`pkg/parser/ast/lib.rs:35,1939-2120,5109`（模块公开、数据类型、独立测试模块）、`pkg/parser/ast/sql_restore.rs`（本文件直接委托的通用恢复边界）。
- crate 配置：`pkg/parser/ast/Cargo.toml`（crate 名、`lib.rs` 入口、依赖与 Go package 元数据）。
- Go 对照：`pkg/parser/ast/ddl.go:1780-2330`、`pkg/parser/ast/misc.go:660-929`。
- 测试证据：`pkg/parser/ast/materialized_test.rs` 覆盖 CREATE、日志及 ALTER/DROP、缺失 SELECT、结构化 SELECT/表达式、CTE/UNION、子查询和通用恢复错误。按任务约束本次不运行 Cargo，测试文件仅作为现有行为证据读取。
- 交付结构以任务指定命令验证，且人工复核本文只描述现有源码可证实的职责、流程、边界和扩展点。

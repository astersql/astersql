# `pkg/session/diagnostic_sql.rs`

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准。

## 文件定位

只接受系统表 SELECT/集合查询、USE、明确列举的元数据 SHOW，以及不执行查询的 EXPLAIN SELECT/集合查询。

## 核心职责

只接受系统表 SELECT/集合查询、USE、明确列举的元数据 SHOW，以及不执行查询的 EXPLAIN SELECT/集合查询。递归遍历 AST，排除写入、锁、文件输出、变量赋值和有副作用函数；以当前库和视图查询回调验证直接 SELECT 表来源。

## 主要符号

- `is_diagnostic_sql_allowed(statement, current_database, is_view) -> bool`：crate 内入口；回调 `Fn(&str, &str) -> bool` 判断表是否视图。
- `DiagnosticSqlChecker<F>`：保存 denied、only_allow_system_tables、has_table、当前库、CTE scope 栈及回调。
- `ast::Visitor` 实现：普通节点负责 SELECT 限制和 scope，embedded 节点负责 CTE/hint/表达式，table-name 钩子负责库与视图限制。
- `diagnostic_side_effect_function` 与 `diagnostic_read_only_show_type`：显式匹配拒绝函数和允许 SHOW 种类；未知 SHOW 默认拒绝。

## 执行流程

直接 SELECT/SetOprStmt 使用 system-table 模式遍历，必须 `!denied && has_table`。USE 直接通过。SHOW 先检查类型 allowlist，再以非 system-table 模式遍历。EXPLAIN 拒绝 analyze、无子语句、非 Select kind 和非集合查询子语句，再以非 system-table 模式遍历。其他顶层语句全部拒绝。

所有 SELECT 子块必须 Kind 为 Select，没有 lock_info、SelectIntoOpt 或 set_var hint。embedded 检查还拒绝带 Value 的变量赋值、有副作用函数；非 system-table 模式拒绝表达式子查询，防止 EXPLAIN 编译期间求值。CTE 定义通过专用 embedded 遍历，而非当作普通表达式子查询。

## 数据与状态

当前数据库按 ASCII 小写保存。直接查询的表名若不带 schema，先从内到外匹配 CTE 名；命中则不视为实体表。否则采用显式 schema 或当前数据库，必须满足 metadef 的 IsMemDB 或 IsSystemDB，置 has_table，并通过 is_view 回调拒绝视图。系统库视图可能访问普通表，因此不能凭 schema 放行。

SELECT、集合语句和集合列表进入/离开时 push/pop scope。递归 CTE 在进入定义时可见自身，非递归 CTE 在定义离开后才可见。CTE 引用本身不置 has_table，因此纯常量 CTE 不能绕过实际系统表要求。EXPLAIN/SHOW 不检查表库和视图，也不要求 has_table；所以 `EXPLAIN SELECT 1` 和普通表 join 可通过。

## 依赖与调用关系

入口依赖 `astersql_parser_ast` 的 Node/Visitor/ExprKind 和 `astersql_meta_metadef` 的库分类，标准库只用 Any/HashSet。`runtime/dispatch.rs` 解析语句后、进入每条语句执行工作前，在 diagnosticmode 开启且非 in_restricted_sql 时调用入口；当前数据库来自会话，视图回调读取 Domain InfoSchema。拒绝则返回 runtime_read_only_mode_error。

内部 restricted SQL 在调用边界绕过；本函数不自行检查模式。prepared statement 的诊断拒绝是另一条会话边界，由测试覆盖，不能归功于本文件。多语句逐条检查，并非先验证全部语句的原子批次。

## 错误处理与边界

本文件返回 bool，不生成或传播 SessionError；解析失败及实际执行错误由会话处理。放行只表示允许进入执行链，不能保证 Rust executor 支持该查询形状。

拒绝函数精确为 `get_lock`、`release_lock`、`release_all_locks`、`last_insert_id`、`nextval`、`lastval`、`setval`、`sleep`、`setvar`；不按参数区分。SHOW 类型显式包括数据库/表/列/状态/变量、各 Create 元数据、Grants、MaskingPolicies、Triggers、过程/函数状态、索引、ProcessList、OpenTables、Events、Plugins、Privileges、Errors、Warnings、Charsets/Collations、Engines、Builtins 和复制状态等，完整集合以源码 matches 为准；Profile 和 TableRegions 不在允许面。

## 并发与资源生命周期

每次调用独占一个 checker，scope 与集合随调用销毁，没有全局缓存、锁或后台任务；回调的元数据一致性由调用方负责。本文件没有取消参数或事务，不负责阻止所有潜在 I/O；递归 AST 检查与实际执行能力、安全性必须分别验证。

## 与 Go 版本的对应关系

Go 用 ast.Walk、MetaOnlyInfoSchema 与 map；Rust 用 accept/Visitor、embedded hooks 和注入 is_view 闭包。Go 显式直接 walk CTE query，Rust 依赖 AST 对 embedded CTE 的遍历行为，不能只检查顶层类型推断安全。

## 扩展指南

新增 SQL、SHOW 或函数时同时核对 Go allowlist 和 AST 遍历，增加独立允许/拒绝用例，尤其覆盖嵌套子查询、CTE shadowing、scope 逃逸、set_var hint、视图和当前库。不能为执行器暂不支持的 EXPLAIN 扩大允许面；应区分 admitted 与成功执行。新增内部绕过路径必须在会话入口测试，不能仅改此 predicate。

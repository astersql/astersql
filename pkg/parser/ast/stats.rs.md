# `pkg/parser/ast/stats.rs`

## 文件定位

本文件位于 `astersql-parser-ast` crate（边界由 `pkg/parser/ast/Cargo.toml` 的 `[lib] path = "lib.rs"` 确定），负责统计类 SQL AST 的文本还原，以及 `REFRESH STATS` / `FLUSH STATS_DELTA` 目标对象的作用域去重。`pkg/parser/ast/lib.rs:5003-5005` 以公开模块 `stats` 装入本文件。

当前实现同时存在两层表示，扩展时必须先分清：

- `pkg/parser/ast/lib.rs` 定义了解析器使用的规范 AST，如 crate 根的 `AnalyzeTableStmt`、`RefreshStatsStmt`、`FlushStmt`；`pkg/parser/parser_actions/admin.rs:5641-5642` 会构造其中的规范 `RefreshStatsStmt`。
- 本文件另定义 snake_case 字段的轻量结构，如 `stats::AnalyzeTableStmt`、`stats::RefreshStatsStmt`、`stats::StatsObject`。代码搜索未发现这些轻量统计结构被非测试生产代码直接构造；它们当前主要由 `pkg/parser/ast/stats_test.rs` 验证。
- 本文件对 crate 根的规范 `crate::AnalyzeTableStmt` 实现了 `restore`，并由 `pkg/parser/ast/sql_restore.rs:678-679` 的 `restore_node` 分派调用。这是本文件目前明确接入规范 AST 主链的部分。

因此，本文件是“统计 SQL 文本/去重语义的移植实现与局部接线”，不是完整的统计执行器；它不读取或更新真实统计数据。

## 核心职责

1. 将 ANALYZE 选项和直方图操作编码映射为固定 SQL 关键字：`analyze_option_string`、`histogram_operation_string`。
2. 为轻量 `AnalyzeTableStmt`，以及 crate 根规范 `crate::AnalyzeTableStmt`，按固定子句顺序生成规范 SQL；规范类型的表达式值通过 `crate::sql_restore::restore_expr` 还原。
3. 为轻量 `DropStatsStmt`、`LoadStatsStmt`、`LockStatsStmt`、`UnlockStatsStmt`、`RefreshStatsStmt` 和 `StatsObject` 生成 SQL 文本。
4. 通过 `dedup_stats_objects` 实现统计对象的覆盖规则：全局 `*.*` 覆盖所有对象，数据库 `db.*` 覆盖同库表，表对象按大小写不敏感的 `(db, table)` 键去重。
5. 为轻量 `RefreshStatsStmt::dedup` 和 `FlushStmt::dedup_flush_objects` 提供同一去重入口，保证两种命令共享规则。

本文件不负责语法解析、Visitor 遍历、权限校验、统计刷新/持久化或 FLUSH 执行；规范 AST 的类型与节点枚举位于 `lib.rs`，解析构造位于 `parser_actions`，执行语义属于下游模块。

## 主要符号

- `quote_name(&str) -> String`：用反引号包裹标识符，并把内部反引号加倍；供表、库、分区、索引和列名还原使用。
- `quote_string(&str) -> String`：用单引号包裹字符串，并把内部单引号加倍；当前用于 `LoadStatsStmt::restore` 的路径。
- `AnalyzeOptionType` 及 `AnalyzeOpt*` 常量：用 `i32` 表示 BUCKETS、TOPN、CMSKETCH DEPTH/WIDTH、SAMPLES、SAMPLERATE、NDVRATE。未知值在 `analyze_option_string` 中映射为空串，而不是返回错误。
- `HistogramOperationType` 及 `HistogramOperation*` 常量：表示 NOP、UPDATE HISTOGRAM、DROP HISTOGRAM；未知值同样映射为空串。
- `AnalyzeOpt`：轻量选项；`value: None` 表示 `DEFAULT`，与 Go 中 `AnalyzeOpt.Value == nil` 的“清除持久化值”语义对应。
- `stats::AnalyzeTableStmt::restore`：还原轻量 ANALYZE 结构，返回 `Result<String, String>`，但当前路径本身不产生错误。
- `impl crate::AnalyzeTableStmt::restore`：还原规范 AST；表名支持 schema 限定，选项值是 `ExprNode`，会调用 `restore_expr`，因而可能传播表达式还原错误。
- `DropStatsStmt`、`LoadStatsStmt`、`LockStatsStmt`、`UnlockStatsStmt`：轻量语句模型及各自的 `restore`。
- `RefreshStatsStmt`：保存 `refresh_objects`、可选 LITE/FULL 模式和 CLUSTER 标志；`new` 只设置对象列表，`restore` 生成 SQL，`dedup` 就地规范化对象列表。
- `FlushStmt`：这里只保存 `flush_objects` 并提供去重；完整规范 `FlushStmt` 的类型、CLUSTER 等字段在 `pkg/parser/ast/lib.rs:3215-3224`。
- `StatsObjectScopeType` 与三个常量：表为 1、数据库为 2、全局为 3，与 Go 的 `iota + 1` 顺序一致。
- `StatsObject::{table,database,global}`：构造对应作用域对象，名称通过 `NewCIStr` 同时保存原始形式 `O` 和小写形式 `L`。
- `StatsObject::restore`：按作用域输出 `` `db`.`table` ``、`` `db`.* `` 或 `*.*`；非法作用域返回错误。
- `dedup_stats_objects`：本文件的核心状态变换；内部 `TableKey` 使用小写库名和表名进行哈希去重。

## 执行流程

ANALYZE 的轻量还原流程是：先输出 `ANALYZE`，按标志追加 `NO_WRITE_TO_BINLOG` 和 `INCREMENTAL TABLE`/`TABLE`，再依次写表列表与分区；随后处理可选直方图操作及列、列选择、索引选择，最后以 `WITH <值或 DEFAULT> <选项名>` 输出分析选项。规范 `crate::AnalyzeTableStmt::restore` 的流程相同，但表名由 `TableName.Schema/Name` 组合，选项表达式委托 `restore_expr`。

`RefreshStatsStmt::restore` 先逐个调用 `StatsObject::restore`，以 `, ` 连接；再追加可选 ` LITE` 或 ` FULL`，最后追加可选 ` CLUSTER`。模式值既非 0 也非 1 时立即返回 `invalid refresh stats mode`。

去重流程由 `dedup_stats_objects` 顺序扫描：

1. 空输入原样返回。
2. 遇到全局对象时立即只返回该对象；输入中其前后所有目标都被全局作用域吸收。
3. 遇到数据库对象时，若该数据库已出现则跳过；否则记录数据库，并从已收集结果中删除同库、且显式带库名的表对象，同时清理对应 `table_seen` 键。
4. 遇到表对象时，若其非空库名已被数据库对象覆盖则跳过；否则用 `(db_name.L, table_name.L)` 去重，首次出现才保留。
5. 未知作用域不会进入结果。没有被覆盖的对象保持输入相对顺序。

`RefreshStatsStmt::dedup` 和 `FlushStmt::dedup_flush_objects` 都用 `std::mem::take` 暂时取走原向量，再把结果写回；这避免克隆整个对象列表。

## 数据与状态

所有状态都由调用者拥有，文件中没有全局可变状态。语句结构保存名称列表、选项和值；还原方法只读取 `&self` 并分配新的 `String`。

`CIStr` 的 `O` 字段用于输出，保留用户输入的原始大小写；`L` 字段用于去重，提供大小写不敏感比较。因此 `db1.t1` 与 `db1.T1` 被视为同一对象，但 `` `a.b`.`c` `` 与 `` `a`.`b.c` `` 形成不同的二元键，不会因简单字符串拼接而冲突。无数据库限定的表使用空 `db_name.L`；数据库级覆盖只吸收明确属于该库的表，不吸收这些未限定表。

轻量的选项/模式/作用域使用 `i32` 类型别名与常量，默认派生值可能是 0。需要注意：表作用域从 1 开始，而 `StatsObject::default()` 的作用域是 0，直接还原会报非法作用域；只有 `StatsObject::global()` 才会显式设为 3。

去重的额外状态是两个局部 `HashSet`：`db_seen` 记录小写数据库名，`table_seen` 记录 `TableKey`。结果向量预分配为输入长度；数据库对象可能触发一次 `retain` 扫描，因而最坏情况下并非严格线性。

## 依赖与调用关系

直接 Rust 依赖很小：`crate::model::{CIStr, NewCIStr, ColumnChoice 及其变体}` 提供大小写不敏感名称和列选择；`std::collections::HashSet` 支持去重；规范 ANALYZE 还原依赖 `crate::sql_restore::restore_expr`。`Cargo.toml` 没有为本文件增加独有外部 crate，说明这些能力都在 crate 内或标准库中完成。

已验证的上游关系：

- `pkg/parser/ast/sql_restore.rs:678-679` 在动态节点为规范 `parser_ast::AnalyzeTableStmt` 时调用本文件提供的 `restore`。
- `pkg/parser/parser_actions/admin.rs:5641-5642` 构造规范 `parser_ast::RefreshStatsStmt`；`pkg/parser/ast/walk.rs:134,533` 和 `pkg/parser/ast/sem.rs:522` 分别登记其遍历与语义分类。这些证据说明规范 REFRESH 节点属于解析主链，但本文件轻量 REFRESH 类型尚未等同接入该链。
- `pkg/parser/ast/stats_test.rs` 直接调用轻量 `StatsObject` 构造器、`RefreshStatsStmt::restore/dedup`、`FlushStmt::dedup_flush_objects`，是这些符号当前最直接的 Rust 调用者。

RustCodeGraph 的文件节点显示 `stats.rs` 被 `sql_restore.rs` 和若干测试文件引用；精确生产代码搜索没有发现轻量 `dedup_stats_objects`、轻量 `RefreshStatsStmt` 或轻量 `FlushStmt` 的其他直接调用者。不要据此宣称规范 REFRESH/FLUSH 已通过本文件完成还原或去重接线。

## 错误处理与边界

- `StatsObject::restore` 拒绝 1/2/3 之外的作用域，并返回包含数值的 `String` 错误。
- `RefreshStatsStmt::restore` 拒绝 LITE/FULL 之外的显式模式；对象还原错误通过 `collect::<Result<...>>()?` 原样传播。
- 规范 `crate::AnalyzeTableStmt::restore` 会传播 `restore_expr` 的错误；轻量 ANALYZE 的 `Result` 当前没有实际失败分支。
- `analyze_option_string` 和 `histogram_operation_string` 对未知值返回空字符串。调用者若构造非法值，可能得到缺少关键字而非显式失败的 SQL；这是兼容现状，不应在无测试和调用方评估时单独改成报错。
- `quote_name` 与 `quote_string` 分别正确加倍反引号和单引号，但它们只是本文件的局部文本器，不代替完整 SQL formatter。
- `dedup_stats_objects` 对未知作用域静默丢弃；对全局对象保留扫描中遇到的第一个全局实例。空对象列表会产生 `REFRESH STATS ` 这样的文本，文件本身不做语法完整性校验，正常输入应由解析器/构造方保证。
- `DropStatsStmt::restore` 在 `is_global_stats` 为真时优先输出 `GLOBAL` 并忽略分区；这对应 Go 注释要求 GLOBAL 或分区形式只含一张表，但 Rust 轻量结构本身不强制该不变量。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务、文件句柄或网络资源。所有还原方法都是同步的纯内存操作；共享引用只读，去重方法要求 `&mut self`，并由 Rust 借用规则保证同一语句对象的独占修改。

`std::mem::take` 在去重期间把字段替换为空向量，旧向量的所有权交给 `dedup_stats_objects`；函数正常返回后新向量写回字段，中间没有外部可观察的悬空引用。`HashSet` 和临时字符串随函数返回自动释放。性能上，普通表去重接近 O(n)，但每个首次出现的数据库对象会对当前结果执行 `retain`，最坏可到 O(n²)；扩展大对象列表时应保留顺序与覆盖语义，再评估是否需要索引化优化。

## 与 Go 版本的对应关系

Go 权威对照是 `pkg/parser/ast/stats.go`，回归用例是 `pkg/parser/ast/stats_test.go`。Rust 在以下方面直接保持 Go 语义：ANALYZE 子句顺序、`DEFAULT` 的 nil/None 表示、选项关键字、直方图 UPDATE/DROP、REFRESH 的 LITE/FULL/CLUSTER 输出、StatsObject 三种作用域，以及全局/数据库/表的去重优先级与大小写不敏感键。

`pkg/parser/ast/stats_test.rs` 复刻了 Go 测试的核心矩阵，包括：全局覆盖全部、数据库删除之前的同库表、数据库覆盖之后的同库表、大小写重复、无数据库限定表重复，以及带点的引用标识符仍按 `(db, table)` 分量区分。

仍有明确的结构差异：

- Go 统计节点直接实现统一 `Node` 的 `Restore`/`Accept`，并被 Go parser 构造；Rust 轻量节点没有统一 Node/Visitor 实现。
- Rust crate 根另有一套规范 AST。当前本文件仅对规范 `AnalyzeTableStmt` 增加还原；轻量 REFRESH/FLUSH/StatsObject 的逻辑尚未证明被规范节点或解析器主链调用。
- Go 的表名是 `*TableName`，会走统一 formatter 并带上下文错误；若干 Rust 轻量类型只用 `String`，信息量和错误上下文更少。
- Go `AnalyzeOpt.Value` 是 `ValueExpr`；规范 Rust 对应 `Option<ExprNode>` 并使用 `restore_expr`，而轻量 Rust 对应 `Option<String>`，只按原字符串输出。

因此新增功能时应以 Go 行为和 crate 根规范 AST 为目标，不能只修改轻量模型或只让 `stats_test.rs` 通过就视为解析主链已完成。

## 扩展指南

新增 ANALYZE 选项时，应同时更新规范 `lib.rs::AnalyzeOptionType`、本文件轻量常量/`analyze_option_string`、规范 `crate::AnalyzeTableStmt::restore` 的 match、Go 对照映射及独立测试；若语法可解析，还要检查 parser action。避免只在一个枚举体系中增加分支。

新增统计对象作用域或改变覆盖规则时，至少同步 `StatsObjectScope*`、构造器、`StatsObject::restore`、`dedup_stats_objects` 与 `pkg/parser/ast/stats_test.rs`，并对照更新 `stats.go`/`stats_test.go` 的预期。必须明确新作用域与 global/database/table 的包含关系、输出顺序、大小写规则和非法值行为。

若要把 REFRESH/FLUSH 逻辑接入规范 AST，优先为 `lib.rs` 中现有规范类型补充实现或建立无歧义的共享辅助函数，不要继续复制第三套结构。接线后应从 parser action 构造的节点开始验证解析、去重、统一 `restore_node` 和 Visitor/语义分类；测试仍放在独立 `*_test.rs` 文件，不能嵌入生产源文件。

任何优化 `dedup_stats_objects` 的改动都要保留：全局早返回、首次数据库对象的位置、删除早先同库表、保留其他对象相对顺序、空数据库名不被库级对象吸收，以及 `CIStr.L` 大小写折叠。对还原文本的修改还要覆盖反引号/单引号转义和非法枚举值，避免生成静默残缺 SQL。

## 验证依据

- RustCodeGraph：`status` 显示当前仓库索引包含 11,467 个文件；`node --file pkg/parser/ast/stats.rs --offset 1 --limit 500` 与 `--offset 498 --limit 120` 核对了本文件全部 558 行、主要符号和文件引用；`query dedup_stats_objects --kind function` 唯一定位到本文件第 450 行。`callers/callees` 查询在限定时间内未返回结果，故调用关系以索引文件引用和精确代码搜索补证。
- Rust 源与 crate 边界：`pkg/parser/ast/stats.rs`、`pkg/parser/ast/lib.rs`、`pkg/parser/ast/Cargo.toml`、`pkg/parser/ast/sql_restore.rs`、`pkg/parser/parser_actions/admin.rs`、`pkg/parser/ast/walk.rs`、`pkg/parser/ast/sem.rs`。
- 独立 Rust 测试：`pkg/parser/ast/stats_test.rs`，覆盖 REFRESH 输出、FLUSH 对象输出及两者共享的作用域去重边界。
- Go 对照：`pkg/parser/ast/stats.go` 与 `pkg/parser/ast/stats_test.go`，核对结构、还原顺序、错误分支、去重算法和测试矩阵。
- 生产调用搜索：对非测试 `*.rs` 精确搜索 `dedup_stats_objects`、`dedup_flush_objects`、`StatsObject::{table,database,global}`、`RefreshStatsStmt` 及 `restore_node`，确认规范 ANALYZE 的还原接线，并记录轻量 REFRESH/FLUSH 当前未检出生产调用这一限制。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档存在且恰有十一个固定二级标题，并人工复核本文件存在原因、执行路径与安全扩展点均有源码依据。

# `pkg/util/parser/ast.rs`

## 文件定位

本文件属于 `astersql-util-parser` crate，是解析器 AST 与 SQL binding/会话层之间的规范化适配层。crate 根 `pkg/util/parser/lib.rs` 通过 `#[path = "ast.rs"] pub mod ast` 挂载本模块，并用 `pub use ast::*` 导出其公开函数和常量；`pkg/util/parser/Cargo.toml` 表明该 crate 只直接依赖本仓库的 `astersql-parser`，本文件再经 `crate::parser_core::ast` 使用其 AST 类型。

生产侧已确认的直接 Rust 调用者包括 `pkg/bindinfo/binding.rs` 的 `RestoreDBForBinding`、`NormalizeStmtForBinding`/`normalize_parsed_statement`，以及 `pkg/session/hint_runtime.rs` 的 `binding_sql_for_warning`。因此它处在“已解析 AST -> 带库名或去库名的稳定 SQL -> binding 归一化、摘要与提示信息”的链路中，而不是 SQL 解析入口本身。RustCodeGraph 的文件节点还报告该文件被 33 个文件引用；精确函数调用以以上源码调用点为准，因为同仓库存在同名 Go API。

## 核心职责

文件有三组职责：

1. `GetDefaultDB` 用 `ImplicitDatabase` 遍历语句，判断是否存在未显式写 schema 的真实表；只要发现一个，就返回调用者提供的默认库名，否则返回空串。
2. `SimpleCases` 为结构简单的单表 `INSERT` 提供字符串级快路径：保留原 SQL 的大小写和布局，只在目标表前插入 schema，避免完整规范化的额外成本和文本变化。
3. `RestoreWithDefaultDB`、`RestoreWithoutDB` 通过 `Restorer` 把当前支持的查询/DML AST 恢复为规范 SQL，分别注入默认 schema 或抑制 schema。恢复器统一处理反引号、字符串转义、表达式优先级、CTE 名称、连接、集合操作和常用 DML 子句。

本模块不是完整的通用 AST formatter。`Restorer::node` 当前只接受 `SelectStmt`、`SetOprStmt`、`InsertStmt`、`UpdateStmt`、`DeleteStmt`；其他节点或缺少必需子节点时产生内部错误，两个公开恢复入口随后用 `unwrap_or_default` 将其转换为空串。调用者必须把空串视为“不支持/恢复失败”，不能视为一条有效 SQL。

## 主要符号

- `ImplicitDatabase { has_implicit }`：私有访问器状态。`ast::Visitor::enter` 对 `SelectStmt`、`InsertStmt`、`UpdateStmt`、`DeleteStmt` 的表引用做结构化检查，并识别 `ExprKind::TableName`；`visit_join`/`visit_result_set`/`visit_table` 递归到真实表。派生表由 AST 子树继续遍历，`QuerySource.is_none()` 才把 `TableSource.Source` 当作普通表直接检查。
- `GetDefaultDB(statement, db_name) -> String`：公开兼容 API；调用 `Node::accept` 后根据 `has_implicit` 返回 `db_name` 或空串。解析器的 `Visitor::enter` 返回 `true` 表示跳过当前节点的子树，所以发现隐式表后会短路剩余遍历。
- `find_table_pos(sql_prefix, table_name) -> Option<usize>`：仅在空格或逗号分隔的完整 token 上匹配小写表名，供快路径定位目标表；它不是 SQL lexer。
- `SimpleCases(statement, default_db, origin) -> (String, bool)`：仅接受具有单个、非派生目标表的普通 `INSERT`/`REPLACE` 形态，并排除查询源、`SET`、`ON DUPLICATE KEY UPDATE`、table hint、非 `CrossJoin` 或有右表的结构。返回值中的布尔值表示是否命中快路径。
- `defaultRestoreFlag`、`bindingRestoreFlag`：与 Go formatter 位掩码对齐的公开常量，分别表达单引号字符串、二元运算符空格、无 charset 前缀、反引号名称，以及 binding 场景额外跳过冗余括号。当前 Rust `Restorer` 直接实现这些格式规则，并未把位掩码传给另一个 formatter。
- `Restorer<'a>`：私有、每次调用新建的有状态恢复器。`default_db` 保存待注入的 schema，`without_schema` 控制全局去 schema，`cte_names` 保存当前恢复过程中登记的 CTE 小写名。
- `Restorer::name`、`table_name`、`column_name`、`value`：恢复标识符、表/列限定名和字面量。反引号通过双写转义，字符串中的单引号也通过双写转义；浮点值从 AST 保存的位模式还原。
- `Restorer::expr`/`expr_with_parent`：覆盖 `Value`、列/变量、标量/聚合/窗口函数、二元/一元运算、真假与空值判断、列表/子查询、`BETWEEN`、`LIKE`、正则、行、`CASE`、时间单位、类型转换等当前 `ExprKind`。`binary_precedence` 与 `parentheses_are_redundant` 决定 binding 规范化时哪些括号可以去除。
- `Restorer::result_set`/`join`/`select`/`set_operation`：恢复表源、派生表、别名、各种 join、CTE、查询子句和集合运算。
- `Restorer::insert`/`update`/`delete` 与 `assignments`：恢复常见写入语句、赋值、过滤、排序、限制和冲突更新。
- `Restorer::node`：动态类型分派总入口，也是当前受支持语句种类的明确边界。
- `RestoreWithDefaultDB`：先尝试 `SimpleCases`，未命中才构造 `Restorer::new(default_db, false)`。
- `RestoreWithoutDB`：直接构造 `Restorer::new("", true)`，从表名、列名和通配符输出中抑制 schema。

文件没有条件编译项；公开面只有四个函数和两个兼容常量，其余符号均为模块内部实现。

## 执行流程

默认库检测流程如下：调用者把解析完成的 `dyn ast::Node` 交给 `GetDefaultDB`；函数创建空状态的 `ImplicitDatabase` 并调用 `statement.accept`；访问器遇到语句表结构时沿 `Join.Left`/`Right` 下钻，遇到无 schema 的 `TableName` 就置位；外层包装节点（例如 `ExplainStmt`）由解析器 AST 自身的 `accept` 子节点遍历传递到内部语句。任一无 schema 表足以决定返回默认库，因此 `enter` 在置位后返回 `true` 以停止继续展开当前/后续子树。

带默认库恢复先执行保守快路径。`SimpleCases` 依次验证原文非空、节点确为 `InsertStmt`、没有复杂来源或附加子句、目标是单一普通表、原文在第一个 `(` 之前可定位表名。如果原文前缀已经有点号，直接原样成功返回；否则选择 AST 显式 schema 或 `default_db`，在目标 token 前插入 `schema.`。任一守卫不满足都返回 `(空串, false)`，由 `RestoreWithDefaultDB` 转入结构化恢复，而不是报告错误。

结构化恢复由 `Restorer::node` 按动态 AST 类型分派。查询路径先登记 `WITH` 中的全部 CTE 名称，再恢复各 CTE、字段、表源、过滤、分组、聚合后过滤、排序与限制；提前登记全部 CTE 可避免同名 CTE 引用被误加默认库。表达式递归过程中根据父运算符和左右位置保留必要括号；子查询通过 `NodeRef::with_node` 回到 `node`。DML 路径先恢复目标表和赋值/值列表，再追加各语句支持的过滤、排序、限制或冲突更新。任一深层步骤的 `Err(String)` 经 `?` 传播到公开入口，最终折叠为空串。

去库名恢复走相同递归路径，但 `without_schema = true`：`table_name` 不输出显式或默认 schema，`column_name` 忽略列的 schema 部分，SELECT 通配符也不输出 schema；表名和表限定列名仍保留。

## 数据与状态

`ImplicitDatabase` 只保存一个单调从 `false` 变为 `true` 的布尔值，没有回滚或跨调用共享。`Restorer` 的两个配置字段在一次恢复中不变，唯一可变集合 `cte_names: HashSet<String>` 随 SELECT 恢复登记 CTE 的 `CIStr.L`（小写规范形式），用于区分普通无 schema 表与 CTE 引用。

输出完全由调用栈上的 `String`/`Vec<String>` 逐层组合。该文件不修改输入 AST，不写全局状态，不缓存结果，也不访问存储或网络。字面量与名称输出使用 AST 同时保存的原始形式 `O` 和规范小写形式 `L`：展示通常使用 `O`，不区分大小写的匹配（表快路径、CTE 集合）使用 `L`。

需要维持的关键不变量是：显式 schema 优先于默认 schema；`without_schema` 优先禁止所有 schema；CTE 名不能被默认 schema 限定；快路径只有在能证明目标是单一普通表时才可报告成功；结构化恢复遇到未知语句不能生成貌似成功的部分 SQL。

## 依赖与调用关系

下游唯一 crate 依赖是 `astersql-parser`。具体依赖面包括 `ast::Node`/`Visitor` 的类型擦除与遍历、语句和表达式节点、`NodeRef::with_node`、`CIStr` 的 `O`/`L` 双表示以及 `FieldType::String()`。标准库只使用 `HashSet` 和字符串/集合构造。

已核实的 Rust 上游关系：

- `pkg/bindinfo/binding.rs::RestoreDBForBinding` 解析 `Statement.SQL` 后调用 `RestoreWithDefaultDB`。
- `pkg/bindinfo/binding.rs::normalize_parsed_statement` 在 `no_db` 分支调用 `RestoreWithoutDB`，否则调用 `RestoreWithDefaultDB`；空输出会使归一化与摘要也返回空，成功输出再交给 `parser::NormalizeDigestForBinding`。
- `pkg/session/hint_runtime.rs::binding_sql_for_warning` 用 `RestoreWithDefaultDB` 生成 warning 展示文本；恢复为空时回退到原始 binding SQL，SELECT hint 另由 hint 模块恢复。
- crate 根 `pkg/util/parser/lib.rs` 将本模块所有公开项再导出，因此调用方通常使用 `astersql_util_parser::RestoreWithDefaultDB` 或别名 `utilparser::*`。

RustCodeGraph 对文件的结构查询确认 46 个符号和 33 个引用文件；其 `callers` 命令在本次检查中超时且未返回边，因此上述函数级调用关系由 `rg` 后逐段读取调用源码确认。Go 调用链单独列在“与 Go 版本的对应关系”，不作为 Rust 已接线证据。

## 错误处理与边界

公开 API 没有返回 `Result`。`GetDefaultDB` 和 `SimpleCases` 用空串/布尔值表达结果；恢复器内部使用 `Result<String, String>` 提供上下文，例如缺失 join 左源、缺失子查询节点、缺失 DML 目标表、`INSERT SET` 无值或语句类型未支持。`RestoreWithDefaultDB`/`RestoreWithoutDB` 最终丢弃错误文本并返回空串，这与当前调用者的回退约定相容，但会隐藏具体失败原因。

`SimpleCases` 是刻意保守的文本算法：它只检查第一个左括号之前的点号，并只以空格/逗号切 token；引号、注释或更复杂布局不由它解析。守卫失败必须回退结构化恢复。若 AST 表已有 schema，它使用该 schema；否则即使 `default_db` 为空也仍会拼接一个点号，因此调用方应只在有可用默认库时依赖快路径补全效果。

结构化恢复覆盖的是 binding 所需子集而非 AST 全能力。以当前源码为准，`Restorer::node` 不支持 `ExplainStmt`、`DoStmt`、DDL 等顶层节点；调用方 `normalize_parsed_statement` 会在进入本模块前解包可绑定的 EXPLAIN。SELECT 的 hint、窗口声明、锁/INTO、ROLLUP 等字段，以及部分 DML 修饰项没有通用输出路径。扩展时不能仅新增 AST 类型分支，还要核对对应节点的所有语义字段，避免静默丢失语义。

括号去除依赖有限的运算符优先级表；未知运算符优先级为 0。相同优先级的右子树只对同名 `AND`、`OR`、`+`、`*` 视为可安全去括号。任何新增运算符都应先验证结合性和混合优先级。当前 `ExprKind::Case` 分支在每个子句前连续追加两次 `" WHEN "`；这是源码现状而非预期 SQL 语法，文档不把该路径声明为已正确支持，后续行为修复应在独立代码任务中增加回归测试。

## 并发与资源生命周期

所有状态均为函数局部或单次 `Restorer` 实例所有，没有 `static` 可变数据、锁、原子变量、线程、异步任务、通道或事务。只要输入 AST 可安全共享，公开函数本身不会造成跨线程状态竞争。

`Restorer` 持有的 `default_db: &str` 生命周期受一次函数调用约束；`NodeRef::with_node` 仅在闭包期间借用子查询节点。构造的 `String`、临时 `Vec` 与 `HashSet` 在调用返回后按 Rust 所有权自动释放。递归深度取决于 AST 嵌套深度，文件没有显式深度限制；极深表达式或子查询会增加栈与临时字符串分配压力。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/parser/ast.go`，测试对照是 `pkg/util/parser/ast_test.go`。

`GetDefaultDB`、`implicitDatabase`、`findTablePos` 和 `SimpleCases` 保留了 Go API 名称与主要守卫顺序。Rust 因 `TableName` 在当前 AST 中是值类型且结果集有显式枚举，访问器不仅依赖通用节点遍历，还在 `SelectStmt`/DML 的 join 结构及 `ExprKind::TableName` 上显式检查；测试 `default_db_walks_through_statement_wrappers_like_go_visitor` 验证 EXPLAIN 包装仍能下钻。

最大的迁移差异在恢复路径。Go 的 `RestoreWithDefaultDB`/`RestoreWithoutDB` 创建 `format.RestoreCtx`，把 `bindingRestoreFlag`、`DefaultDB` 或 `RestoreWithoutSchemaName` 交给每个 AST 节点自身的 `Restore`，失败时记录 debug 日志并返回空串。Rust 当前没有调用统一节点 formatter，而在本文件的 `Restorer` 中实现支持子集，错误同样折叠为空串但不记录日志。由此 Go 新增节点恢复能力不会自动出现在 Rust；两边演进时必须同步检查格式标志、节点覆盖和错误可观测性。

`pkg/util/parser/ast_test.go::TestSimpleCases` 的四组 INSERT 用例已移植到 `pkg/util/parser/ast_test.rs::test_simple_cases`。Rust 还在 `ast_test.rs` 覆盖 full outer join、真实 SELECT 默认库/去库名、EXPLAIN 包装和 INSERT/UPDATE/DELETE；`pkg/util/parser/migration_aster_unit_test.rs` 补充快路径拒绝 `INSERT ... SELECT` 与不支持 `DoStmt` 返回空串的契约。

## 扩展指南

新增顶层语句支持时，先在 `Restorer::node` 添加精确动态类型分支，再用独立测试文件 `pkg/util/parser/ast_test.rs` 或既有迁移测试覆盖成功格式和缺失必需节点的失败路径；不要把 Rust 测试嵌入本生产文件。若调用者会传包装节点，还要决定应由调用者解包还是由恢复器保留包装语义。

新增表达式或 SQL 子句时，优先扩展相应的 `expr_with_parent`、`select`、`insert`、`update` 或 `delete`，逐字段对照 `pkg/parser/ast/lib.rs` 的节点定义和 Go 节点 `Restore` 行为。涉及运算符时同步更新 `binary_precedence` 与右结合/非结合规则；涉及名称时同时验证 `without_schema`、显式 schema、默认 schema、CTE 同名和反引号转义；涉及 CTE 作用域时需注意当前 `cte_names` 在同一 `Restorer` 中累积，嵌套/重名场景应有专门测试。

修改 `SimpleCases` 必须保持“不能证明安全就返回 false”的原则，并至少同步 Go 的四个基准用例、`INSERT ... SELECT` 拒绝用例，以及注释、引号、大小写、显式 schema、空默认库等边界。扩大快路径前应评估原文保真收益与误定位风险。

兼容性风险主要是规范 SQL 文本变化会改变 binding 归一化与 digest；正确性风险主要是遗漏节点字段、错误去括号或错误注入 schema；性能风险来自递归中频繁构造临时 `String`/`Vec`。因此扩展后应检查 `pkg/bindinfo/binding.rs` 和 `pkg/session/hint_runtime.rs` 的调用契约，并用解析后的真实 AST 做往返/语义等价验证，而不能只手工构造局部节点。

## 验证依据

本说明依据以下直接材料：

- 生产实现：`pkg/util/parser/ast.rs` 全部 1038 行；主要入口为 `GetDefaultDB`、`SimpleCases`、`RestoreWithDefaultDB`、`RestoreWithoutDB`，核心分派为 `Restorer::node`。
- crate 边界：`pkg/util/parser/Cargo.toml`、`pkg/util/parser/lib.rs`；确认 crate 名、唯一直接依赖、模块挂载和再导出。
- AST 契约：`pkg/parser/ast/lib.rs` 的 `Visitor`、`Node`、`ExprKind`、`SelectStmt::accept`，确认 visitor 的跳过语义、动态类型分派和子树遍历。
- Rust 调用点：`pkg/bindinfo/binding.rs` 的 `RestoreDBForBinding`、`normalize_parsed_statement`，`pkg/session/hint_runtime.rs` 的 `binding_sql_for_warning`。
- Rust 独立测试：`pkg/util/parser/ast_test.rs`、`pkg/util/parser/migration_aster_unit_test.rs`；确认简单 INSERT、默认库检测、EXPLAIN 下钻、SELECT/DML 恢复、full outer join、去库名和不支持节点空串行为。
- Go 对照：`pkg/util/parser/ast.go`、`pkg/util/parser/ast_test.go`；确认 API 起源、快路径用例、formatter flag 与错误处理差异。
- RustCodeGraph：`status` 显示索引含 11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/util/parser` 显示本目录 12 个相关文件；`node --file pkg/util/parser/ast.rs --offset 1 --limit 120` 确认文件共 1038 行、被 33 个文件使用；`query` 分别定位到 Rust/Go 同名入口。函数级 `callers` 查询超时且无输出，故调用边另由仓库文本搜索和调用点源码复核。

人工复核结论：本文能说明文件存在目的、三条运行路径、支持边界、失败约定、上/下游关系及安全扩展位置；没有把未支持节点描述为已支持，也没有建议把测试写入生产源文件。任务是纯文档分析，未运行 Cargo 或代码测试；最终以任务指定的 11 章节结构命令验证文档形状。

# [`pkg/planner/core/fts_plan_builder.rs`](fts_plan_builder.rs)

## 文件定位

本文件属于 `astersql-planner-core` crate（见 `pkg/planner/core/Cargo.toml`），负责把解析器的 `ast::SelectStmt` 转成一棵轻量 FTS 逻辑计划树。`pkg/planner/core/lib.rs` 以私有模块 `mod fts_plan_builder` 装入它，再通过 `pub use fts_plan_builder::*` 将公开类型和函数提升到 crate 根。

它位于“规范 SQL AST”与本 crate 的轻量 `PlanNode` 表示之间：构造 `DataSource`，按语句内容向外包装 `Selection`、`TopN`/`Sort`/`Limit` 和 `Projection`。它不负责把 `FTS_MATCH_WORD()` 绑定到具体全文索引，也不生成 TiPB 下推载荷；这些后续语义由相邻的 `fts_resolve_index.rs::ResolveFullTextPlan` 及各 resolver 承担。

全仓 Rust 精确搜索只发现 `lib.rs` 的模块声明/重导出和本文件内部引用，没有发现 `BuildFullTextPlan`、`RestoreFtsExpression`、`FullTextIndexes` 或 `FtsTableMetadata` 的生产调用者，也没有 `fts_plan_builder_test.rs`。因此当前可确认的是一组已编译、已导出的构造能力，不能据此声称它已接入 Rust SQL 规划主链。

## 核心职责

- `FtsTableMetadata` 提供构造 FTS 计划所需的轻量表信息：表标识、表名、列名、全文索引列表和 TiFlash 可用性。
- `BuildFullTextPlan` 验证查询是直接访问单个物理表、表名匹配且 TiFlash 可用，然后按 SELECT AST 组装轻量计划节点。
- `RestoreFtsExpression` 把本构造器支持的表达式子集递归还原为字符串，写入计划节点的 `conditions`、`by_items` 或 `operator_info`，供后续 resolver 识别。
- `FullTextIndexes` 以借用切片暴露元数据中的全文索引，便于后续调用 `ResolveFullTextPlan` 时复用，但本文件没有自动串联这两个阶段。

本文件不做全文索引匹配、不验证 `FTS_MATCH_WORD` 的参数形状或出现位置、不处理脏事务、不改写 `_FTS_SCORE`，也不进行代价估算或物理计划选择。`FtsTableMetadata.id`、`columns` 和 `full_text_indexes` 不参与 `BuildFullTextPlan` 的构造判断，其中索引列表仅由独立访问器返回。

## 主要符号

- `pub struct FtsTableMetadata`：公开、可克隆的元数据值。`id: i64` 和 `columns: Vec<String>` 当前仅保存信息；`name` 用于表名验证及扫描节点字段；`tiflash_available` 是构造前置条件；`full_text_indexes` 由 `FullTextIndexes` 借出。
- `fn one_table(select: &ast::SelectStmt) -> Result<&ast::TableSource, String>`：私有形状检查。要求存在 `FROM`、`TableRefs.Right` 为空、左侧是 `TableSource`，且 `QuerySource` 为空，从而排除 join、子查询和其它结果集节点。
- `fn literal(value: &ast::ValueExpr) -> String`：私有字面量渲染器。字符串和 decimal 都加单引号并将单引号写成 `''`；字节值经 `String::from_utf8_lossy` 转成文本后采用相同转义；其它 datum 使用 `value.text()`。
- `pub fn RestoreFtsExpression(expression: &ast::ExprNode) -> Result<String, String>`：公开递归渲染入口。支持值、列、带可选 schema 的函数、二元/一元运算、括号和参数标记；其它 AST 变体返回错误。
- `fn read_limit_value(value: Option<&ast::ExprNode>, default: u64) -> Result<u64, String>`：私有 LIMIT/OFFSET 读取器。缺省采用调用方默认值，只接受 `Uint64` 或非负 `Int64` 字面量。
- `pub fn BuildFullTextPlan(select: &ast::SelectStmt, table: &FtsTableMetadata) -> Result<PlanNode, String>`：公开主入口，生成轻量逻辑树并传播上述辅助函数的错误。
- `pub fn FullTextIndexes(table: &FtsTableMetadata) -> &[FullTextIndex]`：返回与 `table` 生命周期绑定的索引切片，不克隆索引。

文件没有常量、trait、`impl` 或条件编译项；`#![allow(non_snake_case)]` 用于保留与 Go 迁移接口一致的大写函数名。

## 执行流程

`BuildFullTextPlan` 的顺序固定如下：

1. 调用 `one_table` 取得唯一直接物理表。用 AST 表名的规范化小写字段 `Source.Name.L` 与元数据表名做 ASCII 不区分大小写比较；不匹配时以 AST 原始名 `Source.Name.O` 形成错误。
2. 检查 `table.tiflash_available`。不可用时立即拒绝，因为该构造器要求 FTS 扫描落到 TiFlash。
3. 创建 ID 为 `1` 的 `PlanKind::DataSource`。表名来自元数据；非空别名被保存；`partition_id` 固定为 `None`。随后把 `store_type` 改为 `StoreType::TiFlash`，并把 `access_object` 设为表名。
4. 若存在 `WHERE`，将整个条件通过 `RestoreFtsExpression` 渲染成一个字符串，创建 ID 为 `2` 的 `Selection` 包住当前计划。这里不会拆分 `AND`，也不会确认条件包含 FTS。
5. 若 `ORDER BY` 非空，逐项渲染表达式并为降序项追加 ` DESC`。同时有 `LIMIT` 时，读取 offset/count 并创建 ID 为 `3` 的 `TopN`；没有 `LIMIT` 时创建 `Sort`，把逗号连接后的排序项写入 `operator_info`。
6. 若没有 `ORDER BY` 但有 `LIMIT`，创建 ID 为 `3` 的 `Limit`。offset 缺省为 `0`，count 缺省为 `u64::MAX`。
7. 过滤 SELECT 字段中的通配符。只要存在至少一个非通配字段，就创建 ID 为 `4` 的 `Projection`，要求每个保留字段都带 `Expr`，逐项渲染后以逗号连接写入 `operator_info`。纯 `*` 不增加 Projection；混合通配符与显式字段时，字符串元数据只记录显式字段。
8. 返回最外层节点。典型完整形状为 `Projection → TopN/Sort/Limit → Selection → DataSource`，不存在的子句对应层被省略。

节点 ID 是本函数内按层级固定的 `1..=4`，而不是从规划上下文分配；同类分支共享 ID。调用者若把多棵结果合并到更大计划中，不能假设这些 ID 全局唯一。

## 数据与状态

构造过程不修改 `SelectStmt` 或 `FtsTableMetadata`。表名、别名和表达式字符串被复制进拥有所有权的 `PlanNode`；子节点则在逐层包装时移动到新的 `children` 向量。最终返回值拥有整棵树，与输入 AST 的借用生命周期无关。

`PlanNode::New` 定义于 `common_plans.rs`，为节点初始化 `StoreType::Root`、空展示字段和零估算值；本文件只把最底层 DataSource 改为 `TiFlash`。外围 Selection、TopN、Sort、Limit、Projection 仍保留 `Root` 默认值。本文件不填写基数、代价、函数依赖、运行时统计或内存/磁盘数据。

表达式在计划中被降格为字符串，而不是继续保存类型化 AST。字符串格式是后续 `fts_resolve_index.rs` 的输入契约：例如 WHERE 条件位于 `Selection.conditions`，排序项位于 `TopN.by_items` 或 `Sort.operator_info`，投影位于 `Projection.operator_info`。这种表示便于当前轻量 resolver 工作，但会丢失类型、列 ID、collation、别名和完整 SQL restore 语义。

`FtsTableMetadata.full_text_indexes` 保持调用者给定的顺序；`FullTextIndexes` 只借出该向量，没有缓存或派生状态。`id` 与 `columns` 当前未读，修改它们不会改变 `BuildFullTextPlan` 的结果。

## 依赖与调用关系

直接 AST 依赖是 `parser_ast_dependency`，在 `pkg/planner/core/Cargo.toml` 中映射到本地 `astersql-parser-ast`。本文件使用其 `SelectStmt`、`TableSource`、`ResultSetNode`、`ExprNode`、`ExprKind`、`ValueExpr` 和 `ValueDatum`。计划数据类型 `PlanNode`、`PlanKind`、`StoreType` 来自 crate 根重导出的 `common_plans.rs`；`FullTextIndex` 来自 `fts_resolve_index.rs`。文件不受 `nextgen` feature 条件编译控制。

下游理想局部链是 `BuildFullTextPlan(select, metadata)` 生成树，再将 `FullTextIndexes(metadata)` 交给 `ResolveFullTextPlan(plan, indexes, dirty_txn)`。后者依次运行 WHERE、TopN、Projection 和残留用法拒绝规则，把匹配到的索引及查询信息编码到 DataSource。不过仓库搜索没有发现实际串联这两个公开入口的 Rust 生产调用，因此该链只能描述 API 之间由类型和数据格式证明的可组合关系，不能当作已接入应用入口。

RustCodeGraph 能精确定位 `BuildFullTextPlan` 和 `RestoreFtsExpression`，但本次 `callers BuildFullTextPlan` 查询在本地索引上持续无输出并被中止。为避免把工具超时解释成“无调用者”，又使用全仓 `rg` 精确搜索四个公开符号；结果仅有定义、递归自调用和 `lib.rs` 重导出，没有外部 Rust 调用点。

## 错误处理与边界

所有业务失败均用 `Result<_, String>` 返回，`BuildFullTextPlan` 通过 `?` 保留辅助函数的错误文本。显式边界包括：缺少 FROM、join/子查询/非直接表、表名不匹配、TiFlash 不可用、表达式变体不支持、LIMIT/OFFSET 不是非负整数字面量，以及显式 SELECT 字段缺少表达式。

表形状检查有意狭窄：即使 SQL 在 Go 规划器中合法，多表、派生表、CTE 引用或其它 `ResultSetNode` 也会在这里统一报“currently requires one physical table”。LIMIT 参数标记和算术表达式不被接受；缺省 count 则被表示为 `u64::MAX`。

表达式恢复不是通用 SQL pretty-printer。它不输出标识符引号，不保留所有语法细节或运算符优先级所需的额外括号，未知表达式直接失败；bytes 的无效 UTF-8 会替换为 U+FFFD；decimal 被字符串分支加引号。函数 schema 仅在非空时输出，参数标记固定输出 `?`。扩展前必须确认生成字符串仍能被 `fts_resolve_index.rs` 的文本解析器识别。

本构造器只验证“有 TiFlash 副本”布尔值，不验证索引是否存在、是否 public、是否单列、列是否匹配，也不检查 FTS 查询必须是常量。相关错误在后续 resolver 中处理；如果调用者只运行本构造器而跳过 resolver，这些约束不会生效。

## 并发与资源生命周期

本文件全部逻辑同步执行，不创建线程、异步任务、锁、通道、事务、文件句柄、网络连接或后台资源。公开构造函数只借用不可变输入，局部状态由当前调用独占；并发调用之间没有共享可变状态。

返回的 `PlanNode` 拥有所有字符串和子树，输入借用在函数返回后结束。`FullTextIndexes` 是例外：它返回借用切片，不能超过 `FtsTableMetadata` 的生命周期；切片存在期间，Rust 借用规则阻止对索引向量进行冲突的可变访问。

主要资源成本来自递归表达式渲染、字符串格式化/连接、字段及排序项收集、表名/别名克隆和逐层分配计划节点。递归深度随 AST 表达式嵌套增长；文件没有显式深度限制或迭代化保护。大表达式/字段列表会产生相应的线性字符串与向量分配。

## 与 Go 版本的对应关系

同目录没有 `fts_plan_builder.go`，因此本文件不是某个 Go 同名文件的逐函数翻译。Go 的真实 FTS 生产链分散在通用 plan builder、表达式重写和 `pkg/planner/core/fts_resolve_index.go`：Go builder 直接创建 `logicalop.DataSource`、`LogicalSelection`、`LogicalTopN`、`LogicalProjection` 等完整逻辑计划，resolver 再基于列 ID、表 `IndexInfo`、schema 和 session context 绑定全文索引。

Rust 本文件保留了这条链的前半段形状，但使用 `common_plans.rs` 的轻量 `PlanNode` 和字符串表达式。相邻 Rust `fts_resolve_index.rs` 对应 Go `fts_resolve_index.go` 的核心规则顺序，并由 `fts_resolve_index_test.rs` 覆盖索引匹配、字符串转义、TopN/Projection 对齐、脏事务和错误文本；这些测试验证的是构造后计划格式的消费者，不是对 `BuildFullTextPlan` 的直接调用测试。

与 Go 完整实现相比，当前构造器没有 session/deployment-mode 检查、真实表/列 ID 解析、public 单列全文索引验证、`_FTS_SCORE` 虚拟列 schema 追加、类型信息、计划列 ID 分配、别名解析、谓词拆分或父子节点替换。`fts_resolve_index_test.go` 和 JSON testdata 证明 Go 侧已覆盖 TiFlash FTS EXPLAIN、错误用法、prepared plan cache 与脏事务；不能用这些 Go 集成测试替代 Rust 构造器自身尚缺的直接测试。

## 扩展指南

- 新增 AST 表达式支持时，修改 `RestoreFtsExpression`，并在新的独立 `pkg/planner/core/fts_plan_builder_test.rs` 中覆盖转义、括号/优先级、schema 函数名、参数标记和不支持变体；不要把测试放回生产源文件。
- 扩展 FROM 支持应从 `one_table` 入手，但不能仅放宽 AST 形状：还需定义 alias、schema、join/派生表下的元数据解析和 DataSource 数量语义，并核对 Go builder 行为。
- 若要接入 Rust 生产链，应在规范 parser AST 已解析且表元数据可用的位置调用 `BuildFullTextPlan`，随后显式调用 `ResolveFullTextPlan`，并把 dirty transaction 状态传入；应增加覆盖整条链的独立回归测试。
- 若计划节点进入需要全局唯一 ID 的框架，应替换固定 `1..=4` 为调用方提供的分配器，且同步检查 EXPLAIN、缓存键和树合并行为。
- 增加 LIMIT 表达式或 prepared parameter 支持时，不应只让 `read_limit_value` 接受更多形状；必须定义执行期绑定、溢出和计划缓存语义。当前 FTS prepared cache 在 Go 测试中被明确禁用复用。
- 修改表达式文本格式必须同步检查 `fts_resolve_index.rs` 的 `interpret_fts_expression`、`split_function_arguments`、`contains_fts_text` 和相关独立测试，否则可能导致 FTS 未识别或错误绕过。
- 兼容风险集中在字符串化 AST、大小写/标识符引用、固定错误文案和 Go 完整逻辑计划的语义差距；性能风险主要是重复分配及深层递归。迁移时应复用现有 parser restore/type infrastructure，而不是继续扩大脆弱的手写字符串协议。

## 验证依据

- 目标源码：`pkg/planner/core/fts_plan_builder.rs`，人工核对全部 230 行、所有模块级符号、分支、公开性和固定节点 ID。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query BuildFullTextPlan --kind function` 与 `query RestoreFtsExpression --kind function` 精确定位本文件符号。`explore`/`node` 本次未返回内容，`callers BuildFullTextPlan` 持续无输出后中止，因此调用关系以全仓精确源码搜索补证，没有把超时结果当作无调用证据。
- 模块与 crate：`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`、`pkg/planner/core/common_plans.rs`；确认私有模块公开重导出、parser AST 依赖和 `PlanNode` 默认字段语义。
- Rust 下游实现与独立测试：`pkg/planner/core/fts_resolve_index.rs`、`pkg/planner/core/fts_resolve_index_test.rs`；确认字符串计划的消费方式、resolver 顺序和已有边界覆盖。仓库中未发现直接调用本文件公开 API 的 Rust 测试。
- Go 对照：`pkg/planner/core/fts_resolve_index.go`、`pkg/planner/core/fts_resolve_index_test.go`、`pkg/planner/core/testdata/fts_resolve_index_suite_{in,xut,out}.json`；确认 Go 侧使用完整逻辑计划与真实索引/schema/session 状态，而非本文件的轻量构造器。
- 搜索证据：`rg` 对 `BuildFullTextPlan|RestoreFtsExpression|FullTextIndexes|FtsTableMetadata|fts_plan_builder` 的全仓/目录精确搜索只发现本文件定义、内部递归调用和 `lib.rs` 装配；对 `FTS_MATCH_WORD` 的测试搜索定位到上述 Rust/Go resolver 测试及 testdata。
- 验证范围：本任务为纯文档分析，按计划不运行 Cargo 或代码测试。交付前仅运行指定的十一章节结构检查和文档差异检查；运行时行为仍以现有源码与测试内容为证据，未在本任务中重新执行。

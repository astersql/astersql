# `pkg/lightning/mydump/view_import.rs`

## 文件定位

本文件属于 `astersql-lightning-mydump` 子 crate。`pkg/lightning/mydump/Cargo.toml` 将 `lib.rs` 作为库入口，`lib.rs` 以私有模块 `view_import` 装载本文件，再通过 `pub use view_import::*` 导出其中的公开类型和函数。该 crate 的 Go 对照包由 `[package.metadata.porting].go-package = "pkg/lightning/mydump"` 指定。

它位于 Lightning schema 导入链的“视图规划”阶段，而不是执行 SQL 的阶段。真实上游是 `pkg/lightning/mydump/schema_import.rs` 中的 `SchemaImporter::Run`：先调用 `NewSchemaImportPlan`，再创建数据库和表，最后由 `SchemaImporter::importViews` 校验并按计划创建视图。目标文件负责读取视图 schema、提取依赖、构图和排序；数据库查询、SQL 改写与执行仍在 `schema_import.rs`。

## 核心职责

1. 用 `TableName`/`TableNameSet` 表示大小写不敏感的限定对象名，并通过 `normalizeTableName` 统一转为 ASCII 小写。
2. `parseViewSchemaSQL` 从一个视图 schema 文件中找出唯一的 `CREATE VIEW ... AS ...`，提取 `FROM`/`JOIN` 依赖，去掉 Dumpling 生成的 `DROP TABLE`/`DROP VIEW` 清理语句，并规范化简单的 `SET NAMES`。
3. `buildViewImportPlan` 把依赖分成 dump 内基础表、dump 内视图和外部对象；只为视图间依赖建边，并用稳定的 Kahn 拓扑排序生成创建顺序。
4. `validateViewImportPlan` 在真正创建视图前检查外部对象是否已存在。
5. `NewSchemaImportPlan` 汇总数据库元数据和可选视图计划，作为 `SchemaImporter` 三阶段导入的输入。

本文件不负责执行 DDL，也不查询下游数据库。外部依赖集合由此处计算，但实际已有对象集合由 `SchemaImporter::loadExistingViewDependencies` 查询后传入校验。

## 主要符号

- `TableName { schema, name }`：可哈希、可排序的限定名。`Ord` 派生使排序顺序为先 `schema`、后 `name`；`lessTableName` 和 `sortViewNodes` 是这一顺序的公开包装。
- `TableNameSet = HashSet<TableName>`：对象名集合。`add` 和 `has` 会先归一化参数；需要注意，`validateViewImportPlan` 本身直接调用 `existing.contains(dep)`，依赖调用方传入已归一化集合。
- `ParsedViewSchema { key, deps, create_sql }`：单个视图的解析产物。`key` 是归一化后的视图名，`deps` 已去重但其向量顺序不构成接口保证，`create_sql` 是后续创建阶段使用的语句文本。
- `ViewNode`：图节点。`deps` 仅保留 dump 内其他视图，`external_deps` 保存既非 dump 表也非 dump 视图的对象，`dependents` 是反向邻接表，`indegree` 是排序中的可变工作状态。
- `ViewImportPlan { nodes, ordered }`：节点表与确定性的拓扑序。`ordered` 存 `TableName`，执行阶段再用它索引 `nodes`。
- `SchemaImportPlan { db_metas, view_plan }`：完整 schema 导入计划；没有视图时 `view_plan` 为 `None`。
- `ViewDependencyCollector`：提供 CTE 作用域栈、CTE 名登记及表名收集操作的公开结构。当前 Rust 的 `parseViewSchemaSQL` 没有调用它，也没有遍历 SQL AST；它反映 Go 实现的访问器结构，但不是当前 Rust 主链的一部分。
- `hasWithClause`：仅按文本是否以 `with ` 开头判断，当前没有生产调用者；它不同于 Go 版针对多种 AST 节点判断 `With != nil` 的函数。
- `parseViewSchemaSQL`：视图文本解析入口；下游调用 `split_view_statements`、`mask_view_query_literals_and_comments`、`parse_name` 和 `normalizeTableName`。
- `buildViewImportPlan`：图构建与排序入口。
- `validateViewImportPlan`：外部依赖存在性校验入口。
- `NewSchemaImportPlan`：本文件面向导入器的聚合入口。

## 执行流程

完整主链如下：

1. `SchemaImporter::Run` 调用 `NewSchemaImportPlan(store, dbs)`。
2. `NewSchemaImportPlan` 遍历每个 `MDDatabaseMeta`：把 `db.tables` 加入归一化的基础表集合；对每个 `db.views` 调用 `MDTableMeta::GetSchema` 从 `Storage` 读取文本，再调用 `parseViewSchemaSQL`。
3. `parseViewSchemaSQL` 先用 `split_view_statements` 按分号拆分文本。拆分器识别单引号、双引号和反引号，避免在这些引号内切分；发现未闭合引号则返回 `MydumpError::Syntax`。
4. 它用正则定位 `CREATE [OR REPLACE] [ALGORITHM] [DEFINER] [SQL SECURITY] VIEW ... AS ...`。零条或多条匹配都会返回 `MydumpError::Schema`。
5. 对 `AS` 后查询，`mask_view_query_literals_and_comments` 以同长度空格屏蔽单双引号字符串、`#`/合规 `-- ` 行注释和 `/* ... */` 块注释，同时保留换行与反引号标识符。随后正则收集 CTE 名和 `FROM`/`JOIN` 后的对象名；CTE 名被排除，裸表名用当前视图 schema 补全，集合负责去重。
6. 语句输出阶段丢弃以 `DROP TABLE` 或 `DROP VIEW` 开头的语句，把简单的 `SET NAMES charset` 改为 `SET NAMES 'charset';`，其余语句原样保留并以换行连接。
7. 全部视图解析后，`buildViewImportPlan` 归一化基础表和视图集合，拒绝大小写归一后重复的视图定义。每个依赖若属于 dump 基础表则忽略；若属于待导入视图则进入 `deps` 并增加入度；否则进入 `external_deps`。
8. 构建 `dependents` 反向边后，从所有零入度节点开始 Kahn 排序。每次都按 `TableName` 排序 ready 集合并取最小项，因此同一拓扑层的结果可复现。排序结束仍有正入度节点时，返回列出环成员的错误。
9. `SchemaImporter::importViews` 查询涉及 schema 中已有的基础表和视图，调用 `validateViewImportPlan` 检查所有 `external_deps`，再按 `ordered` 创建尚不存在的视图；同名非视图对象会由执行器拒绝。

## 数据与状态

对象身份不变量是 `schema` 与 `name` 的 ASCII 小写组合。`normalizeTableName`、`add`、`has`、`NewSchemaImportPlan` 和 `buildViewImportPlan` 都维持这一规则。解析得到的依赖先放入 `HashSet`，所以 `ParsedViewSchema.deps` 的迭代顺序不稳定；规划正确性不能依赖该向量顺序。

`ViewNode.indegree` 在 `buildViewImportPlan` 的排序过程中原地递减。因此成功返回的计划中，能被处理的节点通常已降为零；它是算法工作字段，不应被当作原始依赖数。原始视图间依赖仍保存在 `deps`。

`create_sql` 拥有自己的 `String`，建图时从 `ParsedViewSchema` 克隆到 `ViewNode`。`SchemaImportPlan.db_metas` 同样克隆输入切片，因此计划不借用调用者数据。`Storage` 只在规划阶段按共享引用读取，目标文件不缓存句柄、不持有数据库连接。

`mask_view_query_literals_and_comments` 只把原 UTF-8 字节中的 ASCII 区域替换为空格或换行，不改变长度，也不会破坏非 ASCII 字节，最后以“不改变 UTF-8 有效性”为不变量恢复 `String`。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 与精确源码搜索共同确认）：

- `SchemaImporter::Run`（`schema_import.rs`）→ `NewSchemaImportPlan`。
- `SchemaImporter::importViews`（`schema_import.rs`）→ `validateViewImportPlan`，并消费 `ViewImportPlan::ordered`、`nodes` 和每个节点的 `create_sql`。
- `schema_import_test.rs` 直接调用 `NewSchemaImportPlan` 验证拓扑计划和实际导入顺序；`view_import_test.rs` 直接覆盖解析与建图入口。

目标文件内部下游边：

- `NewSchemaImportPlan` → `MDTableMeta::GetSchema`（`loader.rs`）→ `Storage`/`ExportStatement`（`reader.rs`）；随后调用 `parseViewSchemaSQL` 和 `buildViewImportPlan`。
- `parseViewSchemaSQL` → `split_view_statements`、`mask_view_query_literals_and_comments`、`parse_name`、`normalizeTableName`，并构造 `ParsedViewSchema`。
- `buildViewImportPlan` → `normalizeTableName`，并构造 `ViewNode`、`ViewImportPlan`。

直接外部 crate 依赖只有 `regex::Regex`；图和集合使用标准库 `HashMap`/`HashSet`。`use crate::*` 引入同 crate 的 `Storage`、`MDDatabaseMeta`、`MydumpError` 等。模块由 `lib.rs` 统一再导出，所以 `schema_import.rs` 可经 crate 根直接使用这些符号。

## 错误处理与边界

- `split_view_statements` 只显式报告未闭合引号，错误为 `MydumpError::Syntax`。它不实现完整 MySQL 词法器；例如注释、复杂转义和 delimiter 语法不由拆分器完整建模。
- `parseViewSchemaSQL` 对缺少或包含多个匹配的 `CREATE VIEW` 返回 `MydumpError::Schema`，错误文本包含当前视图名或原因。内部正则是静态合法常量但每次调用重新编译，使用 `unwrap` 的前提是开发期固定表达式不会失效。
- 当前解析器并非 Go 版的 SQL AST 解析器。它先屏蔽字面量和注释以避免明显误报，再用正则识别 CTE、`FROM` 和 `JOIN`；复杂 MySQL 语法、嵌套/同名 CTE 作用域、带特殊字符的标识符或非标准语句恢复仍可能与 Go 版不同。文档不能据此声称完整 SQL 语法等价。
- `buildViewImportPlan` 拒绝大小写归一后重复的视图；自环和多节点环统一在拓扑排序未覆盖全部节点时报告。错误列出所有剩余正入度节点，并先排序以保证稳定文本。
- 外部依赖不会在建图时立即报错，因为它可能已存在于下游。`validateViewImportPlan` 只检查调用方提供的已有对象集合；第一个缺失对象即返回 `MydumpError::Schema`。
- `NewSchemaImportPlan` 遇到视图 schema 读取、解析或建图错误立即用 `?` 传播；没有视图时返回 `view_plan: None`，不构造空图。

## 并发与资源生命周期

本文件全部逻辑同步执行，没有线程、异步任务、锁、通道或事务。`NewSchemaImportPlan` 顺序读取每个视图文件，`buildViewImportPlan` 在局部所有权的数据结构上完成计算，返回后局部解析向量和临时集合释放。

导入器虽保存可跨线程共享的 `Arc<dyn Storage>`，但传入本文件的是 `&dyn Storage`；这里只在 `MDTableMeta::GetSchema` 调用期间借用它。数据库连接及 SQL 执行生命周期属于 `SchemaImporter`。`concurrency` 字段也不参与本文件的规划算法。

性能上，设视图数为 V、视图依赖数为 E：图构建近似 O(V+E)，但 ready 使用 `Vec`，每次取首项会移动元素，并在新增零入度节点时重新排序；宽图最坏会引入额外的 O(V²) 移动/排序成本。每个视图解析还会重新编译多条正则。修改这些实现时必须保持确定性顺序和错误语义。

## 与 Go 版本的对应关系

直接对照为 `pkg/lightning/mydump/view_import.go`，测试对照为 `view_import_test.go`；完整接线对照在 `schema_import.go`/`schema_import_test.go`。

共同语义包括：对象名大小写归一；保留创建视图所需语句并移除占位 DROP；收集视图引用；把依赖分成 dump 表、dump 视图和外部对象；稳定拓扑排序；拒绝重复定义和环；创建前校验外部依赖。

重要差异如下：

- Go 的 `NewSchemaImportPlan` 接收 context、SQL mode 和对象存储，创建 `parser.Parser` 并设置 SQL mode；Rust 签名只有 `Storage` 与数据库元数据，不接收 context 或 SQL mode。
- Go 的 `parseViewSchemaSQL` 使用 TiDB parser 解析成 AST，通过 `viewDependencyCollector`/`ast.Walk` 处理 CTE 作用域和表节点，再用 AST `Restore` 规范化所有保留语句。Rust 当前使用手写分号拆分、屏蔽器和正则，仅对简单 `SET NAMES` 做文本规范化。
- Rust 定义了同名概念 `ViewDependencyCollector`，但 `parseViewSchemaSQL` 未使用它；其 `Enter`/`Leave` 接口接收字符串/布尔量而非 AST 节点。因此它不能作为“Rust 已采用 AST 遍历”的证据。
- Go 计划的 `ordered` 保存节点指针；Rust 保存 `TableName` 再索引 `nodes`。Go 保留节点原始 key 的大小写而以归一化 key 建表；Rust 在建节点时把 `key` 本身也归一化。
- Go `validateViewImportPlan` 按 `ordered` 遍历并通过会归一化的 `has` 检查；Rust 遍历 `HashMap::values()`，直接 `contains` 已归一化依赖。因此若多个外部依赖同时缺失，Rust 首个错误对象的选择不保证稳定，且调用方必须先归一化已有集合。

## 扩展指南

- 扩展 SQL 语法或依赖抽取时，首要修改点是 `parseViewSchemaSQL`、`mask_view_query_literals_and_comments` 和 `split_view_statements`。应优先评估接入真正的 SQL parser，而不是继续扩大正则；至少要在独立的 `view_import_test.rs` 增加复杂标识符、嵌套 CTE、注释/转义和 SQL mode 回归，并与 `view_import_test.go` 的意图逐项对照。
- 修改对象归一化规则时，要同步审查 `normalizeTableName`、`add`/`has`、`buildViewImportPlan`、`validateViewImportPlan`，以及 `schema_import.rs` 的 `tableKey`、`collectDumpTables`、`unionTableNames`、`loadExistingViewDependencies`，避免集合一侧归一化而另一侧未归一化。
- 修改拓扑策略时，应保留“依赖先于使用者”“同层顺序确定”“重复定义和环均失败”三个契约，并在 `view_import_test.rs` 扩展宽层、跨库、自环和多环测试。测试逻辑继续放在独立测试文件，不应内嵌回生产源文件。
- 新增外部依赖策略时，规划分类修改 `buildViewImportPlan`，下游存在性与对象类型检查修改 `validateViewImportPlan`/`SchemaImporter::loadExistingViewDependencies`，端到端行为放入 `schema_import_test.rs`。
- 优化性能可缓存正则，并把 ready 集合改为有序集合或最小堆；必须维持 `TableName` 字典序。若改变 `ViewNode.indegree` 的消费方式，还要确认执行阶段没有依赖其返回值。
- 对齐 Go 时不能仅复刻类型名；需要特别验证 context 取消、SQL mode、AST Restore、CTE 可见性和错误类别。这些是当前 Rust 实现与 Go 实现之间明确存在的能力边界。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/lightning/mydump` 显示 45 个 Go/Rust 文件，`view_import.rs` 含 35 个符号。
- RustCodeGraph `node --file pkg/lightning/mydump/view_import.rs --offset 1 --limit 520`：读取目标文件完整 474 行，并确认主要类型、函数和内部流程。
- RustCodeGraph `query`/`callees`：核对 `parseViewSchemaSQL` 调用 `mask_view_query_literals_and_comments`、`split_view_statements`、`parse_name`、`normalizeTableName`；`NewSchemaImportPlan` 调用解析与建图入口；`buildViewImportPlan` 构造 `ViewNode`/`ViewImportPlan`。由于 Go/Rust 同名符号使 `callers` 未返回可用结果，上游边由精确 `rg` 复核。
- 已读生产路径：`pkg/lightning/mydump/view_import.rs`、`view_import.go`、`schema_import.rs`、`schema_import.go`、`loader.rs`、`reader.rs`、`common.rs`、`lib.rs`、`Cargo.toml`。
- 已读测试路径：`pkg/lightning/mydump/view_import_test.rs`、`view_import_test.go`、`schema_import_test.rs`、`schema_import_test.go`。Rust 测试直接证明依赖去重、当前 schema 补全、跨库引用、字面量/注释屏蔽、CTE 排除、额外语句保留、缺失/多条 CREATE 错误、确定性拓扑、大小写归一、重复定义和环检测；schema 测试证明计划接入与导入顺序、已有视图跳过和同名非视图冲突。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定命令确认目标文档存在且恰有 11 个固定二级标题，并人工复核唯一生产物及未修改 `plan.md`。

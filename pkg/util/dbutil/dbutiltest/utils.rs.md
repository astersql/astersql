# [`pkg/util/dbutil/dbutiltest/utils.rs`](./utils.rs)

## 文件定位

该文件属于独立 crate `astersql-util-dbutil-dbutiltest`，crate 边界由同目录 `Cargo.toml` 定义，入口 `lib.rs` 通过私有 `mod utils` 装载本文件，再用 `pub use utils::*` 将其公开 API 暴露到 crate 根。仓库总门面还在 `pkg/lib.rs` 的 `util::dbutil::dbutiltest` 下再导出这个 crate。

它是测试辅助代码而非数据库请求主链：输入一条 `CREATE TABLE` SQL，构造内存中的 `model::TableInfo`，供不希望启动真实数据库或执行 DDL 的测试使用。文件没有连接数据库、提交事务或写入元数据存储。

## 核心职责

本文件只有一个职责：`GetTableInfoBySQL` 将单条建表 SQL 依次转换为 AST 和 `TableInfo`，并补齐整数聚簇主键在测试视角下需要的合成 `PRIMARY` 索引。

补索引是必要的兼容处理：`BuildTableInfoFromAST` 生成的 `TableInfo` 在 `PKIsHandle == true` 时用行句柄表达整数聚簇主键，而部分测试按 `Indices` 读取主键。函数因此只在该标志为真时追加一个公开、唯一、B-tree 类型的主索引；非句柄主键沿用 AST 构建阶段已有的索引，避免重复添加。

## 主要符号

- `pub fn GetTableInfoBySQL(create_table_sql: &str, parser: &mut parser::Parser) -> Result<model::TableInfo, parser::errors::Error>`：文件唯一的公开函数。调用者负责创建并可复用可变 `Parser`；成功时按值返回完整 `TableInfo`，失败时返回解析、语句类型检查或元数据构建错误。
- `parser.ParseOneStmt(create_table_sql, "", "")`：只解析一条语句，两个空字符串参数表示不额外指定字符集和排序规则。
- `statement.as_any().downcast_ref::<ast::CreateTableStmt>()`：运行时确认 AST 确实是建表语句；失败时通过 `parser::errors::New` 构造包含原始 SQL 的错误。
- `metabuild::NewContext::<(), std::convert::Infallible>(Vec::new())`：创建没有外部依赖项的构建上下文。这里不注入 schema 查询或其他可失败依赖。
- `astersql_ddl::BuildTableInfoFromAST(&context, create_table)`：把已确认的建表 AST 转成 `model::TableInfo`。
- 合成的 `model::IndexInfo`：名称为 `ast::NewCIStr("PRIMARY")`，`Primary`、`Unique` 均为真，状态为 `StatePublic`，类型为 `Btree`；其唯一 `IndexColumn` 使用 `table.GetPkName()` 和 `UnspecifiedLength`。

本文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项。

## 执行流程

1. 调用传入解析器的 `ParseOneStmt`。语法错误立即由 `?` 返回，不进入元数据构建。
2. 将通用语句 AST 下转为 `ast::CreateTableStmt`。如果输入是可解析但并非 `CREATE TABLE` 的语句，则返回 `get table info from sql <原始 SQL> failed`。
3. 创建空依赖的 metabuild 上下文，并调用 `BuildTableInfoFromAST`。构建错误由 `?` 原样沿错误类型链向上传播。
4. 检查生成结果的 `table.PKIsHandle`。为假时保持 `Indices` 不变；为真时构造并追加一条单列合成主索引，列名来自 `GetPkName()`。
5. 返回最终 `TableInfo`。

同目录 `utils_aster_unit_test.rs` 对三个关键分支给出可执行证据：整数聚簇主键会生成合成索引，`varchar` 非句柄主键不会重复添加索引，`SELECT` 会在语句类型检查阶段被拒绝。

## 数据与状态

函数的输入状态只有 SQL 字符串借用和调用者独占借用的 `Parser`。解析器可能保存 SQL mode 等调用者配置；例如 Go 对照测试会设置 ANSI Quotes 后复用解析器，因此函数没有自行重置解析器配置。

输出 `TableInfo` 是新构建并按值返回的内存对象。函数对它唯一的后处理是条件性修改 `Indices`：`push` 一条新 `IndexInfo`。合成索引的列名动态取自 `GetPkName()`，不会另行猜测列序号；索引列长度使用模型层约定的 `UnspecifiedLength`。除传入解析器和局部返回值外，没有全局变量、缓存、持久化状态或环境变量。

## 依赖与调用关系

下游依赖均可由源码与 `pkg/util/dbutil/dbutiltest/Cargo.toml` 核对：

- `astersql-parser` 提供 `Parser`、单语句解析和统一错误类型。
- `astersql-parser-ast` 提供 `CreateTableStmt`、大小写不敏感名称构造和索引类型。
- `astersql-meta-metabuild` 提供构建上下文。
- `astersql-ddl::BuildTableInfoFromAST` 承担 AST 到表元数据的核心转换。
- `astersql-meta-model` 提供 `TableInfo`、`IndexInfo`、`IndexColumn`、schema 状态及未指定索引长度常量。

直接 Rust 接线来自 `lib.rs` 的再导出，直接可执行调用证据来自 `utils_aster_unit_test.rs`。`rg` 还能在 `pkg/util/dbutil/index_test.rs` 和 `table_test.rs` 找到同名调用，但这些调用位于 `GO_REFERENCE` 原始字符串中，不参与 Rust 编译或执行，不能算作 Rust 调用者。Go 侧真实消费者包括 `pkg/util/dbutil/index_test.go`、`pkg/util/dbutil/table_test.go` 和 `pkg/lightning/common/util_test.go`，它们说明该工具主要为索引发现、表结构比较、schema 编码、自动随机列识别和加索引 SQL 构造准备 `TableInfo`。

RustCodeGraph 的文件节点确认 `utils.rs` 含两个索引节点（文件与函数），但其文件级 “used by” 结果混入了同名 Go 符号的边；精确到 Rust 符号 ID 的 callers/callees 查询没有产出并超时。因此这里不把该混合结果当作调用事实，调用关系以模块接线、源码调用和精确文本位置交叉核验。

## 错误处理与边界

- SQL 语法或解析失败：`ParseOneStmt` 的错误经第一个 `?` 直接传播。
- SQL 合法但不是 `CREATE TABLE`：下转失败，返回包含完整输入 SQL 的新 parser 错误；同目录测试对 `select 1` 的精确错误文本有断言。
- AST 合法但无法构建表元数据：`BuildTableInfoFromAST` 的错误经第二个 `?` 传播。
- 只有 `PKIsHandle` 为真才追加合成索引。函数依赖构建器保证 `GetPkName()` 对这种表能返回正确主键名，没有在本层重新验证该不变量。
- 函数不负责多语句批处理、执行 DDL、检查数据库中是否已有同名表，也不接收 schema 依赖；需要这些能力时不应在这里模拟数据库行为。
- 错误消息会包含原始 SQL，测试输入通常不含秘密；若扩展到可能承载敏感文本的调用场景，应先评估日志或错误外泄风险。

## 并发与资源生命周期

函数不创建线程、异步任务、锁、通道、事务、连接或文件句柄。`&mut parser::Parser` 在一次调用期间提供 Rust 级独占访问，所以同一个解析器不能被多个并发调用直接共享；需要并行测试时，每个执行流应持有自己的解析器，或由上层显式同步。

构建上下文、AST 借用和临时索引都局限于函数调用。AST 只在构建 `TableInfo` 期间被借用，返回值不借用 SQL、解析器或 AST；局部对象在返回或错误退出时按 Rust 所有权规则释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/dbutil/dbutiltest/utils.go`，两版保持相同主流程：解析单条 SQL、确认 `*ast.CreateTableStmt`、调用 `ddl.BuildTableInfoFromAST`、在 `PKIsHandle` 时追加合成主索引，然后返回表元数据。

语义上的对应点包括合成索引的名称、主键/唯一标志、公开状态、B-tree 类型、`GetPkName()` 列名和 `UnspecifiedLength`。Go 版通过 `errors.Trace` 包装解析与构建错误；Rust 版用 `?` 传播 `parser::errors::Error`，没有额外堆叠一层显式 trace。Go 版返回 `*model.TableInfo` 和独立的 `error`，Rust 版返回拥有所有权的 `TableInfo` 包在 `Result` 中。

Go 文件以空白导入 `pkg/planner/core` 初始化表达式求值接线；Rust `Cargo.toml` 仍声明 `astersql-planner-core`，但 `utils.rs` 没有直接导入或调用它。仅凭本文件无法证明 Rust 依赖是否承担同等初始化副作用，因此该点保持“未验证”，不宣称已完全对齐。Go/Bazel 边界由同目录 `BUILD.bazel` 的 `go_library` 定义，Rust crate 边界则由 `Cargo.toml` 和 `lib.rs` 定义。

## 扩展指南

- 若扩展 SQL 到元数据的转换规则，优先确认逻辑属于通用 `BuildTableInfoFromAST` 还是仅属于测试兼容层；只有后者才应修改 `GetTableInfoBySQL`，避免在测试工具中复制 DDL 核心逻辑。
- 若调整聚簇主键表现，必须同步检查 `PKIsHandle` 分支的索引名称、状态、唯一性、类型、列名和长度，并扩展独立测试文件 `utils_aster_unit_test.rs`；不要把测试内嵌回 `utils.rs`。
- 若新增错误分支，应分别覆盖解析错误、非建表 AST 与构建错误，并确认错误上下文与 Go 行为的兼容要求。
- 若希望让调用者传入 schema 依赖，应先修改 metabuild 上下文设计并评估泛型错误类型；当前 `()`/`Infallible` 明确表达“无外部依赖”。
- 若改变公开签名或 crate 导出，应同步检查 `lib.rs`、根 `Cargo.toml` 中的 `facade_util_dbutil_dbutiltest`、`pkg/lib.rs` 门面以及潜在消费者。性能风险主要来自解析和 DDL 元数据构建；追加单元素索引本身为常数规模，但不要在本层引入数据库 I/O 或全局锁。
- Go 的下游测试展示了更广的预期用法，但当前 Rust 对应文件中的旧调用仅是 `GO_REFERENCE` 存档。迁移这些测试时应使用当前 `&mut Parser -> Result<TableInfo, Error>` 签名，而不是照抄 Go 风格的 `(table, err)`。

## 验证依据

- 源码：`pkg/util/dbutil/dbutiltest/utils.rs`，核对唯一公开函数、完整分支、错误传播和合成索引字段。
- crate 接线：`pkg/util/dbutil/dbutiltest/Cargo.toml`、`pkg/util/dbutil/dbutiltest/lib.rs`、根 `Cargo.toml` 与 `pkg/lib.rs`，核对依赖、再导出和 facade 路径。
- Rust 独立测试：`pkg/util/dbutil/dbutiltest/utils_aster_unit_test.rs`，核对聚簇整数主键、非句柄主键和非建表语句三类行为。
- Go 对照与下游意图：`pkg/util/dbutil/dbutiltest/utils.go`、同目录 `BUILD.bazel`、`pkg/util/dbutil/index_test.go`、`pkg/util/dbutil/table_test.go`、`pkg/lightning/common/util_test.go`。
- Rust 引用排查：`pkg/util/dbutil/index_test.rs` 与 `table_test.rs` 的命中均位于 `GO_REFERENCE` 字符串，已人工确认不属于可执行调用。
- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/util/dbutil/dbutiltest` 定位到 Rust/Go 实现、入口和独立测试；`node --file .../utils.rs` 返回完整 66 行源码；`query GetTableInfoBySQL --kind function --json` 区分了 Go 与 Rust 两个同名符号。精确 Rust 符号的 callers/callees 查询超时且无输出，未将模糊边作为结论。
- 本任务是纯文档分析，按计划不运行 Cargo。最终以固定十一个二级标题的结构命令、链接/路径检查、差异复核和人工事实复核完成 Ready 文档验证。

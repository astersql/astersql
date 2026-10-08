# `pkg/util/ddl-checker/executable_checker.rs`

## 文件定位

该文件属于 `astersql-util-ddl-checker` crate；crate 入口 `pkg/util/ddl-checker/lib.rs` 将本文件模块化并重新导出全部公开检查 API。它把“能否执行一段 SQL”拆成三个边界：`CheckerSession` 负责在隔离会话中执行 SQL，`CheckerParser` 负责把单条 SQL 解析为本文件的 `Statement`，`ExecutableChecker` 负责编排两者并管理关闭状态。相邻的 `ddl_syncer.rs` 是当前仓库中可确认的直接生产消费者：`DDLSyncer::SyncTable` 通过检查器先删本地表，再执行从上游取得的建表 SQL。

这不是 TiDB 主请求链上的 SQL 执行器，而是同步或迁移前的 DDL（数据定义语言）预检组件。`Cargo.toml` 将其声明为独立库，并通过根 `Cargo.toml` 的 `facade_util_ddl_checker` 和 `pkg/lib.rs::util::ddl_checker` 暴露为仓库门面。

## 核心职责

- `NewExecutableChecker` 按“初始化错误级日志 → 创建已 bootstrap 的 session → 创建 parser”的顺序构造检查器，任一步失败即短路返回。
- `ExecutableChecker::{Execute,CreateTable,DropTable,IsTableExist}` 将执行能力统一收口到 `CheckerSession::execute`；其中 `CreateTable` 是直接委托，`DropTable` 和 `IsTableExist` 生成探测 SQL。
- `ExecutableChecker::Parse` 从 session 读取字符集与排序规则，再调用 `CheckerParser::parse_one_statement`，保证解析配置来自执行会话。
- `StatementFromAst` 将 parser AST 压缩成本文件关心的 `Statement` 分类；`GetTablesNeededExist` 与 `GetTablesNeededNonExist` 据此计算执行前必须存在或必须不存在的表。
- `ExecutableChecker::Close` 用原子比较交换保证底层 session 至多关闭一次；重复关闭是显式错误。

## 主要符号

- `ExecutionContext { request_id, cancelled }`：对齐 Go `context.Context` 的最小数据载体。`background()` 返回空请求 ID、未取消的默认值；本文件不自行解释取消标志，由 session 适配器决定如何处理。
- `CheckerError` / `CheckerResult<T>`：统一错误类型与结果别名。`new` 仅保存消息，`trace` 同时保留实现 `Error + Send + Sync` 的底层 cause，`Display` 输出消息，`Error::source` 暴露错误链。
- `RenameTablePair`：保存一次 rename 的旧名和新名。
- `Statement`：把 AST 分为 `TruncateTable`、`CreateIndex`、`DropTable`、`DropIndex`、`AlterTable`、`RenameTable`、`CreateTable`、`OtherDdl`、`NonDdl`。这是依赖分析的封闭输入，不是完整 AST。
- `CheckerSession: Send`：抽象 `execute`、`charset_info` 和 `close`；`CheckerParser: Send`：抽象单语句解析。两者使实现可由调用方注入。
- `ExecutableCheckerFactory`：构造期依赖注入边界，要求日志、已 bootstrap 会话和解析器三个步骤都真正执行。
- `ExecutableChecker`：持有 `Box<dyn CheckerSession>`、`Box<dyn CheckerParser>` 和 `AtomicBool isClosed`。字段私有，只能通过公开方法操作。
- `NewExecutableChecker` / `ExecutableChecker::from_parts`：分别用于完整工厂构造和已有部件注入；后者绕过初始化流程，适合测试或上层已经完成生命周期管理的场景。
- `StatementFromAst` / 私有 `is_ddl_ast`：前者处理需要提取表名的 AST 类型，后者枚举其它已知 DDL 类型并将其归为 `OtherDdl`。

## 执行流程

构造流程从 `NewExecutableChecker(factory)` 开始。它先调用 `initialize_error_logger()`；成功后创建 bootstrap 完成的 session，再创建 parser，最终将 `isClosed` 初始化为 `false`。由于使用 `?` 传播错误，后续步骤不会在前一步失败后运行。

执行流程中，`Execute(context, sql)` 原样把上下文和 SQL 交给 session。`CreateTable` 调用 `Execute`；`DropTable` 生成 ``drop table if exists `表名` `` 后调用 `Execute`；`IsTableExist` 生成 ``select 0 from `表名` limit 1``，仅用执行成功与否返回布尔值。相邻 `DDLSyncer::SyncTable` 的实际顺序是：以后台上下文读取上游建表 SQL，以调用者上下文 `DropTable`，再以同一上下文 `Execute` 重建。

解析与依赖分析流程是：`Parse(sql)` 先取得 session 的 `(charset, collation)`，parser 产出 `Statement`；或者调用者先取得 parser AST，再用 `StatementFromAst` 映射。表级 DDL 返回对应名称：truncate/create-index/drop-index/alter 需要一个既存表，drop-table 需要全部列出的表，create-table 需要目标表不存在。rename 只读取第一对，旧名进入“需存在”，新名进入“需不存在”。其它 DDL 返回空列表；`NonDdl` 返回错误。

关闭流程由 `Close` 执行 `false → true` 的原子 CAS。只有赢得 CAS 的调用会执行 `session.close()` 并返回成功，之后的调用收到 `ExecutableChecker is already closed`。`is_closed` 以 Acquire 顺序读取状态。

## 数据与状态

持久状态只有 session、parser 和关闭标志；本文件不缓存 SQL、AST、表清单或执行结果。`Statement` 和 `RenameTablePair` 拥有自己的 `String`/`Vec`，因此映射 AST 时会复制表名，后续不再借用 AST。

表依赖结果保留输入顺序：`DropTable` 通过迭代 `Tables` 收集所有名称；rename 则有意只读取 `pairs.first()`，与 Go 版本读取 `TableToTables[0]` 的既有语义一致。`OtherDdl` 的空列表表示“是 DDL，但本检查器不声明表存在性约束”；它不表示该语句没有其它对象依赖。

`isClosed` 只记录关闭动作是否已经被领取，不阻止 `Execute`、`Parse` 等方法在关闭后被调用；关闭后的具体行为取决于注入的 session/parser。`ExecutionContext.cancelled` 同样只被传递，不在本层强制短路。

## 依赖与调用关系

直接源码依赖是标准库的 `Error`、`fmt`、`AtomicBool`/`Ordering`，以及 `astersql_parser_ast` 的 `Node` 和各类具体 AST 节点。`pkg/util/ddl-checker/Cargo.toml` 还声明 parser、session、sessionapi、mockstore、dbutil、logutil，并以 testkit 为开发依赖；其中部分依赖供 crate 的相邻实现或适配层使用，并非都由本文件直接引用。

上游关系可由源码确认如下：`lib.rs` 重新导出本文件 API；`ddl_syncer.rs::DDLSyncer` 持有 `&mut ExecutableChecker`，其 `SyncTable` 调用 `DropTable` 和 `Execute`，`Close` 调用检查器 `Close`。全仓 Rust 文本检索未发现除本 crate、其独立测试和门面重导出之外对 `NewExecutableChecker`、`StatementFromAst`、`GetTablesNeeded*` 的直接调用，因此这些 API 当前主要提供 crate 边界与测试验证，不能据此声称已经接入其它生产主链。

RustCodeGraph 将目标文件识别为 49 个符号，并显示它被 `ddl_syncer.rs`、crate 入口及测试等文件关联；对关键函数执行精确 `callers/callees` 查询没有返回调用边，因此上述直接调用关系由索引源码视图与 `rg` 使用点补证，而非推断。

## 错误处理与边界

构造、执行和解析错误均使用 `CheckerResult` 返回。`CheckerError::trace` 可以保留底层 cause，但 `new` 只保留文本；具体适配器选择哪种构造方式会影响错误链。`IsTableExist` 是例外：它把所有执行错误折叠为 `false`，因此无法区分“表不存在”、权限/连接错误、取消或 SQL 构造错误。

`GetTablesNeededExist` 和 `GetTablesNeededNonExist` 对 `NonDdl` 返回 `stmt is not a DDLNode`；空 rename 列表返回 `rename table statement contains no table pair`。后者比 Go 直接索引 `[0]` 更安全，但正常 parser 生成的 rename AST 通常应至少有一对。

`StatementFromAst` 只会把 `is_ddl_ast` 明确列举的非表级 DDL 归为 `OtherDdl`。新增 AST DDL 类型若未加入具体分支或 `is_ddl_ast`，会被错误归为 `NonDdl`，这是扩展时最重要的兼容边界。

`DropTable` 和 `IsTableExist` 直接把 `tableName` 插入反引号标识符，未转义其中的反引号；调用方必须保证输入是可信且合法的单个表名。`Close` 在调用 `session.close()` 前已把状态设为关闭，而 `CheckerSession::close` 不返回错误，因此不存在回滚关闭状态的路径。

## 并发与资源生命周期

`CheckerSession` 与 `CheckerParser` 要求 `Send`，允许其所有权随 `ExecutableChecker` 在线程间移动；但主要操作都需要 `&mut self`，类型也没有声明 `Sync`，因此本 API 不提供多个线程并发执行/解析的共享访问模型。`AtomicBool` 的作用仅是为关闭状态提供 CAS 语义，不能单独使整个检查器并发安全。

`Close` 使用成功路径 `AcqRel`、失败读取 `Acquire`，确保关闭权只授予一次。类型没有实现 `Drop`，若调用方遗漏 `Close`，本文件不会自动调用 `CheckerSession::close`；底层 `Box` 虽会析构，但适配器所需的显式关闭语义不由本层保证。`from_parts` 和 factory 都把 session/parser 的所有权移入检查器；`DDLSyncer` 则通过生命周期参数只可变借用检查器，阻止 syncer 存活期间的其它可变访问。

## 与 Go 版本的对应关系

核心行为来自同目录 `executable_checker.go`：Go 版本同样持有 session、parser、原子关闭位，提供 Execute、表存在探测、建表、删表、关闭、解析以及两组表依赖函数。Rust 的 SQL 模板、单次关闭错误文本、drop-table 全表收集、rename 只取第一对和非 DDL 报错均与 Go 语义对齐。

Rust 为可测试和跨 crate 解耦增加了显式 trait/factory；Go 的 `NewExecutableChecker` 在函数内直接初始化 logger、mock store、bootstrap session 和 parser，Rust 构造函数把这些动作委托给调用方实现。当前独立 Rust 测试中的 `DefaultExecutableCheckerFactory` 提供了真实 parser 与轻量 session，但全仓未检索到生产版 factory 实现，因此不能把 Cargo 中声明的 session/mockstore 依赖等同于本构造器已完成生产接线。

Go 直接返回 parser 的 `ast.StmtNode` 并以类型 switch 判定，Rust 先映射为自有 `Statement`。这要求 `is_ddl_ast` 随 parser AST 类型扩展同步维护。Rust 对空 rename 返回结构化错误，而 Go 会因 `[0]` 越界；Rust 的 `ExecutionContext` 仅保留 request ID 和布尔取消位，并不等价于 Go context 的 deadline、值传播和取消通知能力。

`executable_checker_test.rs` 保留 Go 测试的 12 条 fixture 顺序和预期，覆盖 drop database、create/drop table、DML/查询、约束失败与特殊 comment；还额外验证 masking policy、alter database 和六种 fallback AST 被分类为 `OtherDdl`。执行测试使用 testkit mock store，而解析测试通过独立 adapter 走真实 parser。

## 扩展指南

- 新增需要提取表名的 DDL 时，先扩展 `Statement`，再在 `StatementFromAst` 添加 downcast 和字段映射，并同时更新两组 `GetTablesNeeded*`。测试应放在独立的 `pkg/util/ddl-checker/executable_checker_test.rs`，不要内嵌到生产文件。
- 新增只需识别为 DDL、但不贡献表依赖的 AST 类型时，将类型加入 `is_ddl_ast`，并增加至少一条 fallback 分类断言；否则它会退化为 `NonDdl`。
- 若要支持 rename 的全部 pair，必须先核对并有意改变 Go 当前“只取第一对”的兼容语义，同时更新 Go/Rust 对照测试；不能仅在 Rust 侧悄然扩展。
- 若要把检查器接入真实生产构造路径，应在适当模块实现 `ExecutableCheckerFactory`、`CheckerSession` 和 `CheckerParser`，验证 logger/mockstore/bootstrap/session/parser 每一步的错误传播，并补充独立集成测试。不要把测试中的空执行 session 当作生产实现。
- 若允许不可信表名进入 `DropTable`/`IsTableExist`，应复用 parser/标识符转义设施，而不是继续字符串插值；需要测试反引号、限定名与非法标识符。
- 若要求自动资源回收，可评估 `Drop` 或显式 guard，但必须处理 `Close` 的幂等契约及无法从析构返回错误的问题。若要求共享并发访问，应先定义 session/parser 的并发保证，而不是仅依赖原子关闭位。

## 验证依据

- 目标源码：`pkg/util/ddl-checker/executable_checker.rs`，RustCodeGraph `node --file` 完整读取 1–387 行，核对 49 个符号、所有分支和原子顺序。
- crate 边界：`pkg/util/ddl-checker/Cargo.toml`、`pkg/util/ddl-checker/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`。
- 直接生产关系：`pkg/util/ddl-checker/ddl_syncer.rs` 中 `DDLSyncer::SyncTable`、`DDLSyncer::Close` 及其对检查器的可变借用。
- Go 对照：`pkg/util/ddl-checker/executable_checker.go`；Go 测试：`pkg/util/ddl-checker/executable_checker_test.go`。
- Rust 独立测试：`pkg/util/ddl-checker/executable_checker_test.rs`，覆盖解析分类、表依赖清单、其它 DDL fallback 和 testkit 执行结果。
- 图查询：RustCodeGraph `status` 显示索引含目标文件；执行了目标目录 `files`、目标文件 `node`、`NewExecutableChecker`/`StatementFromAst`/`GetTablesNeededExist` 等 `query`，以及三者的 `callers`/`callees`。精确调用边无返回，故用限定为 Rust 文件的 `rg` 搜索补充使用点。
- 本任务为纯文档分析，按总计划不运行 Cargo。交付前另运行任务指定的 11 章节结构验证，并人工核对未修改 Rust、Go、Cargo 或 `plan.md`。

# `pkg/infoschema/perfschema/init.rs`

## 文件定位

本文件属于 `astersql-infoschema-perfschema` crate（入口为 `pkg/infoschema/perfschema/lib.rs`），负责把 `const.rs` 中的静态 `CREATE TABLE` 文本转换为 `tables.rs` 定义的轻量元数据，并提供一次性注册 `PERFORMANCE_SCHEMA` 虚拟库的接口。它处在“表定义清单”与“会话可查询的系统表目录”之间，不负责生成虚拟表数据行；行包装和数据源在 `tables.rs`。

当前存在两条使用方式。`init()`/`Init()` 在依赖就绪后把数据库放入本文件私有的进程内注册表；`pkg/session/runtime/system_query.rs` 则直接调用 `build_performance_schema()`，把返回的表元数据接入会话系统目录。后者是已检索到的生产调用点，因此不能把 `VIRTUAL_DATABASES` 说成所有查询路径的唯一注册中心。

## 核心职责

1. `parse_create_table()` 以受限语法解析静态建表文本，产生 `TableMeta`、`ColumnInfo` 和 `IndexInfo`。
2. `build_performance_schema()` 遍历 `perfSchemaTables`，从 `TABLE_ID_MAP` 分配稳定表 ID，设置库 ID、public 状态以及列 ID/偏移，最终组装 `VirtualDatabase`。
3. `init()` 用 `AtomicBool` 控制前置依赖，用 `Once` 保证最多注册一次，并用互斥量保护全局注册表。
4. `registered_databases()` 提供注册表的克隆快照；`set_eval_simple_ast_ready()` 暴露启动/测试接线所需的就绪开关。

本文件不执行 SQL、不保存表数据，也不实现 `PerfSchemaTable` 的行读取。

## 主要符号

- `VirtualDatabase`：轻量数据库元数据，包含固定库 ID、名称、字符集、排序规则和 `Vec<TableMeta>`。字段公开，便于会话目录消费。
- `InitError`：构建阶段的可比较错误枚举。`InvalidCreateSql(String)` 表示受限解析失败，`UnknownTable(String)` 表示 DDL 清单与 ID 映射不一致，`DuplicateDatabase` 描述重复注册；当前重复注册分支直接 panic，并未实际返回该枚举值。
- `set_eval_simple_ast_ready(bool)`：以 Release 顺序写入 `EVAL_SIMPLE_AST_READY`。
- `registered_databases() -> Vec<VirtualDatabase>`：持锁克隆整个注册表；返回值修改不会影响全局状态。
- `init()`：检查 Acquire 就绪标志，然后通过 `INIT_ONCE.call_once` 构建并注册数据库。构建错误、锁中毒或重复库均导致 panic。
- `Init()`：保留 Go 风格名称的公开别名，仅转调 `init()`。
- `build_performance_schema() -> Result<VirtualDatabase, InitError>`：纯构建入口，也是 `pkg/session/runtime/system_query.rs` 和元数据兼容测试使用的入口。
- `parse_create_table(&str) -> Result<TableMeta, InitError>`：公开的轻量解析入口；独立测试直接覆盖它。
- `split_definitions(&str) -> Vec<&str>`：私有顶层逗号切分器，跟踪括号深度和单/双/反引号状态。
- `INIT_ONCE`、`EVAL_SIMPLE_AST_READY`、`VIRTUAL_DATABASES`：分别承载一次性执行、前置就绪发布和进程内注册状态。

## 执行流程

启动式路径如下：调用方先在表达式简单 AST 求值能力就绪后调用 `set_eval_simple_ast_ready(true)`，再调用 `init()`。若标志仍为 false，`init()` 立即返回且不会消耗 `Once`，之后仍可重试。首次就绪调用进入 `call_once`，执行 `build_performance_schema()`；成功后持有注册表互斥锁，按 ID 相等或名称忽略 ASCII 大小写检查冲突，然后追加数据库。后续调用不再重建。

构建路径中，`build_performance_schema()` 按 `perfSchemaTables` 原顺序逐条调用 `parse_create_table()`。每张表的规范化小写名称必须出现在 `TABLE_ID_MAP`；随后写入 `PERFORMANCE_SCHEMA_DB_ID`，把表设为 public，并按 DDL 列序把列 ID 设为从 1 开始、offset 设为从 0 开始。数据库固定命名为 `PERFORMANCE_SCHEMA`，字符集/排序规则为 `utf8mb4`/`utf8mb4_bin`。

解析单条 DDL 时，函数去除外围空白和末尾分号，以首个 `(`、末个 `)` 划分头部和定义体；头部最后一个 token 经过去库名前缀、反引号和 ASCII 小写化得到表名。`split_definitions()` 只在括号深度为零且不在引号中时按逗号切分。定义以 `PRIMARY KEY`、`KEY ` 或 `UNIQUE KEY` 开头时解析为索引，并反查此前已出现列的偏移；其余非空定义取第一个 token 作为列名。最后先返回 ID/库 ID 为 0、public 为 false 的 `TableMeta`，由上层构建函数补齐注册属性。

## 数据与状态

稳定身份来自 `tables.rs`：`PERFORMANCE_SCHEMA_DB_ID = (1 << 62) | 10_000`，`TABLE_ID_MAP` 按固定表名序列分配 `DB_ID + offset + 1`。因此调整表名映射的顺序会改变表 ID，属于兼容性敏感变更；新增 DDL 必须同步映射，否则构建返回 `UnknownTable`。

`VIRTUAL_DATABASES` 是 `LazyLock<Mutex<Vec<VirtualDatabase>>>`，首次访问时创建空向量。`registered_databases()` 深度克隆数据库和表元数据，隔离调用方修改，但读取成本随元数据规模增长。`INIT_ONCE` 没有重置接口；测试与同进程调用共享其生命周期。`EVAL_SIMPLE_AST_READY` 可被再次写成 false，但这不会撤销已注册数据库或重置 `Once`。

解析器保留原始 `create_sql`，列名规范化为 ASCII 小写，索引显式名称保留输入大小写；匿名 `UNIQUE KEY` 以首个索引列名作为名称。索引 ID 和列 ID 都从 1 起，索引列保存的是零基列偏移。

## 依赖与调用关系

直接下游依赖只有同 crate 模块：`crate::consts::perfSchemaTables` 提供 DDL 数组；`crate::tables::{ColumnInfo, IndexInfo, TableMeta, PERFORMANCE_SCHEMA_DB_ID, TABLE_ID_MAP}` 提供元数据结构和稳定 ID。标准库提供 `Once`、`LazyLock`、`Mutex` 与原子内存顺序。虽然 `Cargo.toml` 声明了 `tracing`，本文件没有使用它；大量旧 Go 依赖位于 `target.'cfg(any())'.dependencies`，该永假条件说明它们不是当前 Rust 构建的活动依赖。

`lib.rs` 公开再导出 `Init`、`init`、`build_performance_schema`、`registered_databases` 和 `set_eval_simple_ast_ready`；`parse_create_table` 仍可通过公开的 `init` 模块访问，但未在 crate 根再导出。RustCodeGraph 的文件关系显示直接使用者包括 `pkg/infoschema/perfschema/tables_test.rs`、`pkg/session/mysql_metadata_compat_test.rs` 和 `pkg/session/runtime/system_query.rs`。源码核对表明生产路径 `system_query.rs` 调用 `build_performance_schema()` 后逐表写入 `performance_schema` 系统目录；另外两个是测试证据。

## 错误处理与边界

可恢复构建 API 通过 `Result` 返回错误：缺失/逆序括号、空表名、索引缺括号、索引引用尚未解析或不存在的列均形成 `InvalidCreateSql`；表名不在映射中形成 `UnknownTable`。轻量解析器并非通用 SQL parser：它依赖静态 DDL 的形态，只识别三类索引前缀，不验证 `CREATE TABLE` 关键字、列类型、约束完整性或引号转义，也把其他定义的首 token 当作列名。新增复杂 DDL 前必须扩展解析与回归测试，不能假定完整 MySQL 语法支持。

`split_definitions()` 用饱和减法处理多余右括号，并只在遇到相同引号字符时结束引号状态；它不处理反斜杠或成对引号转义。外层首/末括号检查也不会验证整体括号平衡。这些限制对当前静态清单可接受，但属于外部调用 `parse_create_table()` 的明确边界。

`init()` 把构建错误转为 panic；锁中毒也通过 `expect` panic。重复 ID/名称同样 panic，且 `InitError::DuplicateDatabase` 当前未接入该分支。由于 panic 发生在 `Once` 闭包内会使 `Once` 中毒，后续调用也不能作为恢复机制。

## 并发与资源生命周期

就绪标志采用 Release 写/Acquire 读，使调用者在设置就绪前完成的初始化写入对观察到 true 的线程可见。`Once` 保证成功的注册闭包只执行一次；多个并发调用者不会重复构建或重复追加。注册表访问由 `Mutex` 串行化，锁仅覆盖冲突检查与追加，耗时的 DDL 解析发生在锁外。

所有状态均为进程级静态对象，生命周期持续到进程退出，没有卸载、清空或测试重置能力。函数不创建线程、异步任务、通道、网络连接、文件或事务；唯一资源风险是互斥锁中毒以及克隆完整快照的内存/CPU 成本。

## 与 Go 版本的对应关系

Go 的 `pkg/infoschema/perfschema/init.go::Init` 同样先检查 `expression.EvalSimpleAst != nil`，再用 `sync.Once` 执行一次初始化；Rust 用显式原子布尔值替代函数指针是否为空，并保留 `Init()` 别名。两者都按静态 DDL 顺序分配映射中的表 ID、把列 ID 设为从 1 开始、设置固定库 ID/public 状态，并注册名为 `PERFORMANCE_SCHEMA` 的 utf8mb4 数据库。

关键差异是 Go 使用完整 `parser.ParseOneStmt` 与 `ddl.BuildTableInfoFromAST`，再调用 `infoschema.RegisterVirtualTable(dbInfo, tableFromMeta)`；Rust 使用本文件的受限字符串解析器和轻量 `TableMeta`，`init()` 只写入私有 `VIRTUAL_DATABASES`。当前会话目录通过 `system_query.rs` 直接消费 `build_performance_schema()`，而不是消费该注册表或 `table_from_meta` 回调。因此 Rust 已覆盖元数据构建和可查询目录接线，但注册抽象与 Go 并非一一等价。

Go 的 `tables_test.go::TestPerfSchemaTables` 通过 SQL 验证若干表可查询且返回空行；Rust 的 `tables_test.rs::test_perf_schema_tables` 验证数据库包含对应表且每张表可由 `table_from_meta` 包装，`test_init_registers_when_eval_ready` 验证就绪后的 Once 注册。`init_test.rs::unnamed_unique_index_uses_first_column_name_like_go` 专门固定匿名唯一索引命名语义。

## 扩展指南

新增或修改 performance schema 表时，应同步检查 `const.rs::perfSchemaTables`、`tables.rs::TABLE_ID_MAP` 与 Go 的 `const.go`/`tables.go`；保持旧表 ID 稳定，并通过 `build_performance_schema()` 验证每个名称可映射。涉及新 DDL 形态时，优先修改 `parse_create_table()`/`split_definitions()`，在独立的 `init_test.rs` 或 `tables_test.rs` 增加成功与失败边界测试，不要把测试内嵌回生产文件。

若要把一次性注册表接入更多生产入口，应先决定是否统一 `system_query.rs` 的直接构建路径，避免出现两份状态来源。修改 `init()` 时需保留“未就绪调用不消耗 Once”的性质，并评估 panic、Once 中毒和并发调用。若开放注册失败恢复，应同时调整错误 API，而不是仅使用当前未消费的 `DuplicateDatabase` 变体。

兼容风险主要是库/表/列 ID 与列顺序变化、名称大小写规则和默认字符集/排序规则偏离 Go；性能风险主要来自启动时解析全部静态 DDL以及每次读取注册表时的全量克隆。新增复杂索引还需验证索引列必须在其定义前已经解析这一约束。

## 验证依据

- 源码：`pkg/infoschema/perfschema/init.rs`（全部 259 行）、`const.rs`、`tables.rs`、`lib.rs`。
- crate 边界：`pkg/infoschema/perfschema/Cargo.toml`；确认当前活动依赖与 `cfg(any())` 下的非活动移植依赖。
- RustCodeGraph：`status` 显示索引含 11,467 文件、7,032 个 Rust 文件；`files --filter pkg/infoschema/perfschema` 找到目标及对照文件；`node --file .../init.rs` 列出 23 个符号并报告三个直接使用文件；`query` 精确定位 `build_performance_schema`、本文件的 `parse_create_table`、`split_definitions`、`set_eval_simple_ast_ready` 和 `registered_databases`。精确 `callers/callees` 批量查询超时，调用边因此又以直接源码引用核验，不据此推断额外调用者。
- 上游接线：`pkg/session/runtime/system_query.rs`；兼容行为测试：`pkg/session/mysql_metadata_compat_test.rs`。
- Rust 测试：`pkg/infoschema/perfschema/init_test.rs`、`tables_test.rs`，覆盖匿名唯一索引、元数据构建、表包装和就绪后注册。
- Go 对照：`pkg/infoschema/perfschema/init.go`、`const.go`、`tables.go`、`tables_test.go`、`main_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工事实复核验收。

# `pkg/meta/metadef/db.rs`

## 文件定位

`pkg/meta/metadef/db.rs` 是 `astersql-meta-metadef` crate 中的库名定义与分类模块。crate 根模块 `pkg/meta/metadef/lib.rs` 通过 `pub mod db` 声明它，再用 `pub use db::*` 将本文件的公开符号重导出到 crate 根。因此上层既可以按 `astersql_meta_metadef::IsMemOrSysDB` 调用，也可以在局部别名为 `metadef` 后调用。

本文件不读写元数据，也不管理 schema 对象；它提供的是纯字符串分类边界，供 DDL、规划器、锁、备份以及 schema 过滤等上层路径共享。直接源文件是 `pkg/meta/metadef/db.rs`，crate 声明在 `pkg/meta/metadef/Cargo.toml`。

## 核心职责

- 定义三个内存 schema 的 `ast::CIStr` 名称：`InformationSchemaName`、`PerformanceSchemaName` 和 `MetricSchemaName`。`CIStr` 同时保存原始形式 `O` 与小写形式 `L`。
- 定义集群表的实例列名 `ClusterTableInstanceColumnName` 和 BR 临时库的内部前缀 `temporaryDBNamePrefix`。
- 将库名区分为内存 schema、严格的 `mysql` 系统库、更广的系统相关库，以及 BR 临时库。这些分类并非同义：`IsSystemDB` 只识别 `mysql`，`IsSystemRelatedDB` 额外包含 `sys` 和 `workload_schema`，`IsMemOrSysDB` 再将三个内存 schema 纳入。
- 保留 Go 版本的输入契约：前四个分类函数对传入字符串做精确比较，调用方应传小写名；`IsBRRelatedDB` 使用原始名并做大小写敏感的前缀判定。

## 主要符号

- `pub static InformationSchemaName: LazyLock<ast::CIStr>`：首次解引用时通过 `ast::NewCIStr("INFORMATION_SCHEMA")` 构造，`O` 为大写原名，`L` 为 `information_schema`。
- `pub static PerformanceSchemaName: LazyLock<ast::CIStr>` 与 `pub static MetricSchemaName: LazyLock<ast::CIStr>`：以同样方式表示 `PERFORMANCE_SCHEMA` 和 `METRICS_SCHEMA`。
- `pub const ClusterTableInstanceColumnName: &str = "INSTANCE"`：对外公开的集群表列名。它是编译期字符串，不涉及惰性初始化。
- `const temporaryDBNamePrefix: &str = "__TiDB_BR_Temporary_"`：仅本模块可见，保存 BR 临时库原始名的固定前缀。
- `pub fn IsMemOrSysDB(dbLowerName: &str) -> bool`：组合入口，短路求值 `IsMemDB(...) || IsSystemRelatedDB(...)`。
- `pub fn IsMemDB(dbLowerName: &str) -> bool`：将输入与三个 `CIStr.L` 比较，仅识别三个内存 schema。
- `pub fn IsSystemRelatedDB(dbLowerName: &str) -> bool`：组合 `IsSystemDB` 与 `mysql::SysDB` / `mysql::WorkloadSchema` 比较。
- `pub fn IsSystemDB(dbLowerName: &str) -> bool`：仅与 `mysql::SystemDB` 比较，不包含 `sys`。
- `pub fn IsBRRelatedDB(dbOriginName: &str) -> bool`：调用 `str::starts_with(temporaryDBNamePrefix)`，前缀大小写敏感。

本文件没有 trait、struct、enum、`impl` 块或条件编译项。

## 执行流程

1. crate 首次访问任一内存 schema 静态量时，对应 `LazyLock` 调用 `ast::NewCIStr`，构造包含 `O`/`L` 两种形式的 `CIStr`；后续访问复用同一对象。
2. 调用方对一般 schema 分类时，应先取小写名（例如 `CIStr.L`），再调用 `IsMemDB`、`IsSystemDB`、`IsSystemRelatedDB` 或联合入口 `IsMemOrSysDB`。函数内部不做大小写归一化。
3. `IsMemOrSysDB` 先判断三个内存 schema；若未命中，再判断 `mysql`、`sys` 和 `workload_schema`。短路逻辑保证命中后不继续评估右侧。
4. BR 路径传入原始库名给 `IsBRRelatedDB`；只要字符串以完整的 `__TiDB_BR_Temporary_` 开头即命中，函数不校验后缀是否为空或是否对应真实数据库。
5. 上层根据布尔结果改变自己的策略。例如 `pkg/ddl/persistent_actions.rs::async_notify_event` 对内存/系统库跳过 schema-change 通知，`pkg/planner/util/misc.rs::FilterPathByIsolationRead` 对系统相关库不执行隔离读路径过滤，`pkg/lock/lock.rs::CheckTableLock` 对内存/系统库跳过表锁检查。

## 数据与状态

本模块的持久数据为零；所有判定函数都是无副作用的纯读逻辑。唯一的运行时状态是三个 `LazyLock<ast::CIStr>` 的一次性初始化状态；初始化后其 `CIStr` 在进程生命期内保持不变。

关键不变量如下：

- `InformationSchemaName.O == "INFORMATION_SCHEMA"` 且 `InformationSchemaName.L == "information_schema"`，这由 `pkg/meta/metadef/migration_aster_unit_test.rs::database_names_match_go_case_sensitive_classification` 直接断言。
- `IsMemDB` 只认小写的 `information_schema`、`performance_schema`、`metrics_schema`；大写 `INFORMATION_SCHEMA` 不会被函数内部自动转换。
- `IsSystemDB("sys") == false` 但 `IsSystemRelatedDB("sys") == true`，两个 API 的语义边界不可合并。
- `IsBRRelatedDB("__TiDB_BR_Temporary_orders") == true`，但小写前缀 `__tidb_br_temporary_orders` 为 `false`。

## 依赖与调用关系

下游依赖只有标准库与两个 workspace crate：

- `std::sync::LazyLock` 负责线程安全的惰性构造。
- `crate::ast` 在 `pkg/meta/metadef/lib.rs` 中重导出 `parser_ast::model::{CIStr, NewCIStr}`；对应 Cargo 依赖为 `astersql-parser-ast`。
- `crate::mysql` 重导出 `parser_mysql::r#const::{SystemDB, SysDB, WorkloadSchema}`；对应 Cargo 依赖为 `astersql-parser-mysql`。

RustCodeGraph 对 `pkg/meta/metadef/db.rs` 报告有 7 个使用文件。结合精确文本搜索，可核验的直接上游包括：

- `pkg/ddl/persistent_create_materialized_view_log.rs`：在创建 materialized-view log 前，使用 `IsMemOrSysDB` 拒绝内存/系统 schema 上的不合适基表。
- `pkg/ddl/persistent_actions.rs::async_notify_event`：对这些 schema 直接返回，不发布持久化 schema-change 通知。
- `pkg/ddl/normal_policy.rs`：使用 `IsSystemRelatedDB` 识别系统相关数据库。
- `pkg/planner/util/misc.rs::FilterPathByIsolationRead`：对系统相关库原样返回访问路径。
- `pkg/util/filter/schema.rs::IsSystemSchema` 和 `pkg/lock/lock.rs::CheckTableLock`：将 `IsMemOrSysDB` 作为更高层系统 schema/锁策略的基础判定。
- `pkg/store/helper/helper.rs` 与 `br/pkg/backup/client.rs`：通过 `IsMemDB` 过滤内存 schema。
- `pkg/infoschema/issyncer/loader_test.rs`：在相关测试辅助逻辑中组合 `IsSystemDB` 与 `IsBRRelatedDB`。

`pkg/domain/domain.rs` 和 `pkg/privilege/privileges/privileges.rs` 中还有注明“对齐 `metadef.IsMemOrSysDB`”的本地判定；它们不是本 Rust 函数的直接调用边，不应写成已接线调用者。

## 错误处理与边界

所有 API 都返回 `bool` 或暴露静态字符串，本文件没有 `Result`、`Option` 或 panic 分支，也不记录错误。错误语义由上层根据分类结果产生；例如 DDL 调用者会将不允许的系统库对象转换为 SQL/DDL 错误。

需特别注意的边界是：

- 函数名中的 `dbLowerName` 是输入契约，不是函数内会完成的操作。传入混合或大写会得到 `false`，不会报错。
- `IsBRRelatedDB` 只判定前缀，所以固定前缀本身也会命中；它不负责证明数据库确由 BR 创建。
- 新增或重命名系统 schema 如果只改上层一处，可能导致 DDL、锁、过滤和备份策略分歧；这是此集中分类模块要避免的兼容性风险。

## 并发与资源生命周期

`LazyLock` 保证每个 `CIStr` 在并发首次访问时只初始化一次，初始化后可在多线程间共享读取。初始化闭包只构造固定字符串，不持有锁、事务、文件句柄、通道、异步任务或网络连接。其余常量是 `'static` 字符串，函数只借用输入 `&str` 并在返回前结束借用，没有显式释放或清理阶段。

由于分类仅为少量等值比较或一次前缀检查，它们不会阻塞；`starts_with` 的工作量只与固定前缀长度有关。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/meta/metadef/db.go`，公开名称、分类集合、判定顺序和前缀字面量均保持一致：

- Go 的包级 `var` 使用 `ast.NewCIStr`；Rust 使用 `LazyLock<ast::CIStr>` 保留运行时构造，而不假设 `CIStr` 可在 `const` 上下文初始化。
- Go `switch` 的三个 case 被 Rust `matches!` 守卫表达，识别集合没有缩减。
- Go 的 `strings.HasPrefix` 对应 Rust 的 `str::starts_with`，两者在这里均为大小写敏感。
- Go 的 `ClusterTableInstanceColumnName` 位于 `var` 块，Rust 将不可变字面量表达为 `pub const &str`；对外值不变。

`pkg/meta/metadef/db_test.rs` 保留 `pkg/meta/metadef/db_test.go` 的三组主要断言，但两者都未直接覆盖 `IsMemOrSysDB`、`IsBRRelatedDB` 和所有公开常量；Rust 的 `pkg/meta/metadef/migration_aster_unit_test.rs::database_names_match_go_case_sensitive_classification` 补充了 `InformationSchemaName.O/L`、组合分类和 BR 大小写边界。

## 扩展指南

- 新增内存 schema 时，在本文件增加对应 `LazyLock<ast::CIStr>`，并扩展 `IsMemDB`。同时检查 Go 对照文件、parser/mysql 常量是否需要同步，以及所有依赖 `IsMemOrSysDB` 的策略是否应自动纳入新 schema。
- 新增持久化系统库时，优先扩展 `IsSystemRelatedDB`；只有它与 `mysql.SystemDB` 语义等价时才能改 `IsSystemDB`。不得为了通过单个调用点而模糊两者边界。
- 修改 BR 前缀时，同步检查 BR 创建/清理临时库的实际命名处。前缀与大小写都是兼容性契约，修改可能造成旧临时库无法被识别。
- 新增或改变分类行为时，应将 Rust 回归断言放在独立的 `pkg/meta/metadef/db_test.rs` 或已有的 `migration_aster_unit_test.rs`，不要内嵌到 `db.rs`。若该行为来自 Go 迁移，应同步核对 `pkg/meta/metadef/db_test.go`。
- 回归用例至少覆盖：每个命中值、一个普通业务库、大写/混合大小写输入、`sys` 在两个 system API 间的差异，以及 BR 前缀的大小写反例。
- 性能上应继续保持无分配的 `&str` 比较。不要在这些热路径函数内通过 `to_lowercase()` 隐式分配；大小写归一化仍由持有 `CIStr` 或输入名称的调用方完成。

## 验证依据

- RustCodeGraph 索引状态：当前项目索引包含 11,467 个文件，其中 7,032 个 Rust 文件；`rustcodegraph files --filter pkg/meta/metadef` 确认目标、Go 对照和测试均被索引。
- 源码全貌：`rustcodegraph node --file pkg/meta/metadef/db.rs --offset 1 --limit 240` 读取了全部 85 行，确认三个惰性静态量、两个字符串常量与五个公开判定函数。
- 符号查询：对 `IsMemDB`、`IsSystemRelatedDB`、`IsBRRelatedDB`、`IsMemOrSysDB` 和 `IsSystemDB` 执行了 RustCodeGraph `query`，确认 Rust/Go 同名实现与部分直接使用点。`node` 对目标文件报告 7 个使用文件。
- 调用边复核：RustCodeGraph 的 `callers/callees` 命令在当前环境中持续无输出，已中止；随后使用索引 `node` 读取 `pkg/ddl/persistent_create_materialized_view_log.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/planner/util/misc.rs`、`pkg/util/filter/schema.rs` 和 `pkg/lock/lock.rs` 的命中上下文，并用 `rg` 列出 Rust 直接引用，避免将本地复制逻辑误写成调用边。
- crate 边界：`pkg/meta/metadef/Cargo.toml` 声明 crate 名 `astersql-meta-metadef`、库入口 `lib.rs` 及 `astersql-parser-ast` / `astersql-parser-mysql` 两个依赖；无 feature 声明。`pkg/meta/metadef/lib.rs` 确认模块声明、重导出与独立测试接线。
- Go 对照：逐项阅读 `pkg/meta/metadef/db.go` 和 `pkg/meta/metadef/db_test.go`，确认常量、分类逻辑、大小写契约与主要断言一致。
- Rust 测试：阅读 `pkg/meta/metadef/db_test.rs` 和 `pkg/meta/metadef/migration_aster_unit_test.rs::database_names_match_go_case_sensitive_classification`，确认内存/system/BR 的正反例。本任务为纯文档分析，按计划不运行 Cargo。

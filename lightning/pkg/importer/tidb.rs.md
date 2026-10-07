# `lightning/pkg/importer/tidb.rs`

## 文件定位

[`tidb.rs`](tidb.rs) 是 `astersql-lightning-pkg-importer` crate 面向目标 TiDB 的 SQL 适配层。crate 根 [`lib.rs`](lib.rs) 通过 `mod tidb` 加载它并 `pub use tidb::*`，因此本文件的公开函数同时构成 importer crate 的公开导出面。它不负责整条导入流水线，而是集中承载四类边界操作：按 Lightning 配置创建目标库连接、包装连接和 SQL parser、把 dump 元数据与目标 schema 对齐、查询导入所需系统状态及执行少量收尾 DDL。

该 crate 在 [`Cargo.toml`](Cargo.toml) 中声明为 library，`package.metadata.porting.go-package` 指向 `lightning/pkg/importer`，且注释明确这是一个依赖本地 stubs 覆盖 SQL/PD/TiKV 等边界的 slim port。因此本文所说的 `DB`、`Parser`、日志、指标和重试器，均是 crate 内经 [`stubs.rs`](stubs.rs) 重导出的迁移边界类型，不应直接等同于完整 Go 运行时的 `database/sql`、真实 parser 或完整重试实现。

## 核心职责

1. `DBFromConfig` 把 `config::DBStore` 转换为 `common::MySQLConnectParam`，先探测哪些 session variables 可被目标 TiDB 接受，再用仅含成功变量的参数重连，向后续 importer 提供带稳定会话环境的 `DB`。
2. `TiDBManager`、`NewTiDBManager` 和 `NewTiDBManagerWithDB` 把数据库句柄与使用相同 SQL mode 的 parser 放在一起，并提供关闭连接和带重试语义的删表操作。
3. `LoadSchemaInfo` 以 dump 中的数据库/表清单为准，将目标 TiDB 返回的 `model::TableInfo` 映射为 importer 使用的 `importdef::DBInfo`；匹配时忽略大小写，写 checkpoint-facing 名称时保留 dump 原名。
4. `ObtainImportantVariables` 和 `ObtainNewCollationEnabled` 查询导入编码/兼容性所需的服务端变量，并分别采用“查询失败回退默认值”和“缺行视为旧版本关闭”的容错策略。
5. `AlterAutoIncrement`、`adjustIDBase` 与 `AlterAutoRandom` 生成导入完成后的 allocator rebase DDL，复现 Go 对溢出、最大值和无操作分支的处理。

## 主要符号

- `pub struct TiDBManager { pub db: DB, pub parser: Parser }`：持有可克隆的 SQL 边界句柄和已设置 SQL mode 的 parser。字段在 Rust 端是公开的，而 Go 对照结构体字段为包内私有；这是导出可见性的实现差异，不代表额外业务语义。
- `pub fn DBFromConfig(ctx, dsn) -> Result<DB>`：两阶段建连入口。默认 session variables 包含统计、DistSQL、索引串行扫描、checksum 并发度，以及显式 auto-random、`_tidb_rowid`、autocommit、optimistic transaction、关闭 foreign key checks 和 import request source。`dsn.Vars` 在默认值之后写入，所以同名自定义项覆盖默认值。
- `pub fn NewTiDBManager(...) -> Result<TiDBManager>`：调用 `DBFromConfig`，再以 `dsn.SQLMode` 构建 manager。第三个 TLS 参数当前命名为 `_tls` 且未使用；实际 TLS 字段已由 `DBFromConfig` 从 `dsn.Security` 复制。
- `pub fn NewTiDBManagerWithDB(db, sqlMode) -> TiDBManager`：适合已有连接或测试，创建 parser、设置 SQL mode 并组装 manager。
- `TiDBManager::Close(&self)`：关闭底层连接但丢弃关闭错误，与 Go `Close` 不返回错误一致。
- `TiDBManager::DropTable(ctx, tableName) -> Result<()>`：构造 `DROP TABLE {tableName}`，通过 `common::SQLWithRetry::Exec` 执行；调用者必须传入已经正确引用的表名。
- `pub fn LoadSchemaInfo(...) -> Result<HashMap<String, importdef::DBInfo>>`：通过注入的 `getTables` 回调逐库获取目标表，校验缺表和非 public 状态，生成 importer 元数据。
- `pub fn ObtainImportantVariables(ctx, db, needTiDBVars) -> HashMap<String, String>`：查询公共重要变量，并按开关追加 TiDB import 专属变量；结果始终用默认表补齐，因此接口不返回错误。
- `pub fn ObtainNewCollationEnabled(ctx, db) -> Result<bool>`：只有精确字符串 `"True"` 返回 `true`；`ErrNoRows`/`not_found` 返回 `false`，其他错误传播。
- `pub fn AlterAutoIncrement(...) -> Result<()>`：将 `u64` base 压到 `i64::MAX`，溢出时加入 `FORCE`，执行失败时附加完整 SQL 上下文。
- `pub fn adjustIDBase(incr) -> i64`：纯数值边界函数，为 auto-increment DDL 提供有符号上限。
- `pub fn AlterAutoRandom(...) -> Result<()>`：`randomBase == maxAutoRandom.wrapping_add(1)` 时压回最大值；更大值只告警并成功返回；其余情况执行 `AUTO_RANDOM_BASE` DDL。

文件没有模块级常量、trait、条件编译项或异步函数；所有生产符号均在普通构建中存在。

## 执行流程

### 建连与 manager 生命周期

`DBFromConfig` 先从 `DBStore` 复制地址、认证、SQL mode、包大小、TLS/fallback 和 UUID/net 参数，首次调用 `MySQLConnectParam::Connect`。随后它建立默认变量表，并让 `dsn.Vars` 覆盖同名值。每个变量通过临时连接执行 `SET SESSION k = 'v'`；失败只记录 warning，并在遍历完成后从变量表删除。函数关闭探测连接，把剩余变量写入 `param.Vars` 后第二次连接并返回。这一设计将“不被旧 TiDB 支持的变量”从最终 DSN 初始化中剔除，同时让真正的首次/二次连接错误终止流程。

`NewTiDBManager` 在该连接之上调用 `NewTiDBManagerWithDB`。后者保证 parser 的 SQL mode 与连接配置一致。RustCodeGraph 显示当前生产调用包括 [`precheck.rs`](precheck.rs) 的 `NewPrecheckItemBuilderFromConfig -> DBFromConfig`，以及 [`server/checkpoint_control.rs`](../server/checkpoint_control.rs) 的 checkpoint 错误销毁流程 `DestroyError -> NewTiDBManager -> DropTable -> Close`。

### schema 映射

`LoadSchemaInfo` 对每个 dump schema 调用 `getTables(ctx, schema.Name)`，把目标表按 `TableInfo.Name.L` 建立小写索引。之后仅遍历 dump 明确列出的表：用 dump 名的 ASCII 小写形式查目标表；缺失时产生 `ErrSchemaNotExists`；状态不是 `StatePublic` 时记录失败指标并返回；成功时记录 pending/success 指标并生成 `importdef::TableInfo`。其中 `Core` 与 `Desired` 保存目标表结构，而 map key 和 `TableInfo.Name` 保留 dump 的大小写敏感名称。

Go 生产链是 `GetAllTableStructures -> LoadSchemaInfo`。当前 Rust [`get_pre_info.rs`](get_pre_info.rs) 的 `PreImportInfoGetterImpl::GetAllTableStructures` 自行逐表构建 `DBInfo`，RustCodeGraph 未发现它调用本函数；因此 `LoadSchemaInfo` 当前是已实现且有独立测试的公共能力，不能描述成 Rust 生产主链上已经接线的步骤。

### 状态查询与导入收尾

`ObtainImportantVariables` 用默认变量表的 key 组装一条 `SHOW VARIABLES ... IN (...)` 查询，`needTiDBVars` 决定是否加入 import 专属变量。查询失败时记录 warning 并按空结果继续；成功结果中长度不足 2 的行被忽略。最后公共默认值一定补齐，TiDB 专属默认值仅在开关为真时补齐。当前 Rust 生产边 `Controller::setGlobalVariables -> ObtainImportantVariables` 位于 [`import.rs`](import.rs)，`TargetInfoGetterImpl` 相关路径也会提供这一变量视图。

`ObtainNewCollationEnabled` 查询 `mysql.tidb`；它保留 Go 的旧版本兼容分支，但当前 RustCodeGraph 仅找到 parity/独立测试调用，未找到 Go `setGlobalVariables` 对应的 Rust 生产调用。

两个 allocator 函数都先处理边界值再交给 `SQLWithRetry::Exec`。Go 侧它们由 `table_import.go::postProcess` 调用；当前 Rust [`table_import.rs`](table_import.rs) 的 `postProcess` 只更新 meta、校验 checksum、完成表并推进 checkpoint，未调用这两个函数，所以它们目前同样属于“实现与测试已存在、生产接线未验证”。

## 数据与状态

- `TiDBManager` 唯一持久状态是 `DB` 与 `Parser`。本文件不记录连接是否已关闭，也没有 `Drop` 自动清理；生命周期由调用方显式调用 `Close`。
- `DBFromConfig` 的 `vars` 是最终连接的 session 初始化集合。探测失败项会被删除；自定义 `dsn.Vars` 可以覆盖默认项，也可能因探测失败被删除。
- `LoadSchemaInfo` 的输出按 schema 原名索引，内部表 map 按 dump 表原名索引。目标 `CIStr.L` 只用于大小写不敏感查找，`CIStr.O`/dump 名承担展示和 checkpoint 身份语义。
- 指标状态使用 `metric::TableStatePending`：非 public 表以错误记录，成功映射以 `None` 记录。本文件只发出计数，不保存 metric 状态。
- `ObtainImportantVariables` 返回的 map 允许保留查询返回的额外变量，同时为所选默认集合补缺；测试证明 `needTiDBVars=true` 时包含 `tidb_placement_mode` 等 import 默认项。
- auto-increment 的 SQL 表示范围是 `i64`：超过上限的 `u64` 被压为 `i64::MAX`。auto-random 使用 `wrapping_add(1)` 明确模拟 Go `uint64` 的溢出比较语义，包括 `maxAutoRandom == u64::MAX` 时加一回绕到 0 的情况。

## 依赖与调用关系

上游已验证的 Rust 调用边：

- `lightning/pkg/importer/precheck.rs::NewPrecheckItemBuilderFromConfig -> DBFromConfig`，为预检构造目标 DB。
- `lightning/pkg/importer/import.rs::Controller::setGlobalVariables -> ObtainImportantVariables`，把目标变量写入 controller 的 `sysVars`。
- `lightning/pkg/server/checkpoint_control.rs::DestroyError -> NewTiDBManager -> DBFromConfig/NewTiDBManagerWithDB`，随后逐表调用 `TiDBManager::DropTable` 并显式 `Close`。
- `lightning/pkg/importer/tidb_test.rs` 与 `parity_test.rs` 调用其余接口以保护公开契约和 Go 对齐边界。

下游主要依赖：

- `common::MySQLConnectParam`、`sql::DB` 和 `common::SQLWithRetry` 负责连接、SQL 执行/查询及日志化重试边界。
- `config::DBStore`、`vardef::*`、`tikv_util::ExplicitTypeImport` 决定最终连接及 session 环境。
- `parser::Parser` 与 `mysql::SQLMode` 保证 manager 后续解析目标 DDL 时遵循配置模式。
- `mydump::MDDatabaseMeta`、`model::TableInfo`、`importdef::{DBInfo, TableInfo}` 构成 dump 元数据到 importer 运行时元数据的转换链。
- `metric::FromContext`、`logutil`、`log`、`zap` 提供可选指标和结构化日志；它们不改变核心返回值。
- `errors::{Trace, Annotatef, Errorf}` 与 `common_ext::ErrSchemaNotExists` 维护错误因果和用户可操作的 SQL/schema 上下文。

Cargo 直接依赖没有为本文件单列第三方库；这些符号主要来自 crate 内重导出的本地模块/stubs。`Cargo.toml` 中的本地 Lightning crates和 storage/meta 依赖定义了整个 importer crate 的更大边界，但不能据此推断本文件直接调用它们。

## 错误处理与边界

- 两次 `Connect` 的错误都经 `errors::Trace` 传播；探测 `SET SESSION` 的单项失败是非致命的，记录 query 和错误后跳过该变量。探测连接的 `Close` 错误被刻意忽略。
- session variable SQL 通过字符串格式化生成，key/value 未在本文件内转义。当前 key 来自 vardef/default 配置或 `dsn.Vars`，因此扩展自定义变量能力时必须审视标识符和值的可信边界，避免把任意输入直接拼入 SQL。
- `DropTable`、`AlterAutoIncrement`、`AlterAutoRandom` 同样信任调用方传入已引用的 `tableName`；现有测试使用 `` `db`.`table` ``。新增调用者应使用仓库统一的 identifier escaping/unique-table helper，而不是传入裸用户输入。
- `LoadSchemaInfo` 会原样传播 `getTables` 错误；dump 声明的表在目标端缺失、或目标表不为 public，都会 fail fast，不返回部分结果。目标端额外表被忽略。
- `ObtainImportantVariables` 的 API 选择可用性优先：查询错误不传播，缺失值由默认值代替。调用方不能通过返回值判断查询是否失败，只能依赖日志。
- `ObtainNewCollationEnabled` 仅把 `not_found` 或 class 为 `ErrNoRows` 的错误降级为 `false`；权限等其他错误保留。值比较大小写敏感，除 `"True"` 外均视为 false。
- `AlterAutoIncrement` 在越过 `i64::MAX` 时用 `FORCE` 和最大有符号值；执行失败会记录手工执行提示，并用 SQL 文本 annotate 原错误。
- `AlterAutoRandom` 对 `max+1` 做封顶，对更大值模拟 TiDB overflow no-op 并返回成功。这里使用 `wrapping_add` 是显式兼容点，不应替换成会在 debug 构建 panic 的普通加法。
- Rust 测试说明当前 `SQLWithRetry` stub 不实现 Go 测试覆盖的 TiKV-busy retry loop；所以文档不能声称 slim Rust 环境已经验证真实重试时序。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel 或共享可变全局状态。`Context` 通过 clone 传给日志、回调和 SQL wrapper；取消传播的实际能力取决于 slim `Context`/SQL stub，而非本文件自身。

资源生命周期的关键点是 `DBFromConfig` 的两阶段连接：第一条连接只用于能力探测并在重连前关闭，第二条连接交给调用方。`TiDBManager::Close` 是显式、幂等性由 `DB::Close` 实现决定且错误不可见。checkpoint 清理调用点在结束时显式关闭 manager；测试 `test_drop_table` 也验证执行后关闭。未来若在中途错误路径增加更多 manager 使用，应采用作用域 guard 或确保所有返回分支都关闭连接，不能依赖本类型的 `Drop`。

`LoadSchemaInfo` 和变量查询均为顺序执行：schema、table、session variable 逐项处理，没有并发化。若未来并发探测 session variables，需要保持最终 map 的确定规则、日志完整性以及临时连接上的会话隔离；若并发 schema 查询，需要避免指标计数和 fail-fast 返回发生语义漂移。

## 与 Go 版本的对应关系

整体算法逐项对应 [`tidb.go`](tidb.go)：两阶段连接及失败变量剔除、manager/parser 组装、大小写不敏感 schema 匹配且保留 dump 名、变量默认值回填、新 collation 的旧版本兼容、auto-increment 有符号封顶与 `FORCE`、auto-random 最大值封顶/no-op 均保持相同分支。

已确认的差异和迁移限制：

- Go 使用真实 `*sql.DB`、parser、指标和 `common.SQLWithRetry`；Rust crate 注释声明 SQL 等边界由本地 stubs 覆盖。Rust 单测使用内存 `DB`/脚本化错误，因此证明的是可观察契约，不是完整网络与重试行为。
- Rust `TiDBManager` 返回值是按值类型且字段公开；Go 返回指针且字段包内私有。Rust 的 `DB` 本身可 clone，相关共享语义取决于 stub 实现。
- Rust `NewTiDBManager` 与 Go 一样保留 TLS 参数，但当前两边该参数在函数体内都未使用；TLS 配置来自 `DBStore.Security`。
- Rust `ObtainImportantVariables` 防御性忽略少于两列的结果行；Go 直接索引两列。Rust 测试还注明当前 `DefaultImportantVariables` 的 row-format 默认值为 `"2"`，而所读 Go 失败场景断言为 `"1"`，且 Rust 默认集合可能不含 Go 的 `tidb_backoff_weight`。这是默认表边界的已知语义差异，应在默认值定义处单独核对，而不是在本文件硬编码修正。
- Rust `ObtainNewCollationEnabled` 可识别 `not_found` 或 `ErrNoRows` class；Go 用 `errors.ErrorEqual(err, sql.ErrNoRows)`。Rust 测试未覆盖 Go 的 TiKV busy 后重试成功场景，因为当前 stub 没有重试循环。
- Rust 使用 `maxAutoRandom.wrapping_add(1)` 明确保持 Go `uint64` 回绕；附加测试覆盖了 `u64::MAX`，比当前 Go 同名测试多一个边界案例。
- 最重要的接线差异是：Go 的 `LoadSchemaInfo`、`ObtainNewCollationEnabled`、`AlterAutoIncrement`、`AlterAutoRandom` 均有生产调用；当前索引中对应 Rust 生产调用未全部出现。它们不能仅凭实现存在就标记为完整迁移。

## 扩展指南

- 增加或修改连接 session variable：修改 `DBFromConfig` 的默认 map，并同步 [`tidb_test.rs`](tidb_test.rs) 中连接/变量契约测试；检查自定义 `dsn.Vars` 的覆盖优先级、旧版本拒绝变量后的删除行为，以及值的 SQL 转义风险。若涉及真实重试/TLS，需在独立测试文件补足非 stub 集成证据。
- 改 schema 映射：以 `LoadSchemaInfo` 为入口，保持 `CIStr.L` 查找和 dump 原名保存这两个不同不变量；同步 `test_load_schema_info`、缺库/缺表、非 public 状态与 metric 断言。不要把测试写进 `tidb.rs`。
- 改系统变量集合或默认值：实际默认表位于 `common_ext::DefaultImportantVariables` / `DefaultImportVariablesTiDB`，本文件只负责查询和回填。需要同时检查 `get_pre_info.rs` 和 `import.rs::setGlobalVariables` 的消费方，并解释 Rust/Go 默认值差异。
- 接通 new-collation 或 allocator 收尾链：先对照 Go `import.go::setGlobalVariables` 与 `table_import.go::postProcess`，在 Rust 对应入口增加最小必要接线；同步 `tidb_test.rs` 以及 `table_import_test.rs`/`import_test.rs`，保留错误传播、checkpoint 顺序和 no-op 分支。不要以本文件已有 helper 代替生产调用证据。
- 改 DDL 文本：保持调用方负责 identifier quoting 的契约，继续通过 `SQLWithRetry`，并在错误中保留完整可手工执行 SQL；评估兼容旧 TiDB 语法和日志敏感信息风险。
- 改资源管理：`TiDBManager` 当前没有 RAII close。若引入自动清理，要验证显式 `Close` 与 drop 双重关闭的行为，并同步 checkpoint control 的错误路径测试。
- 所有 Rust 逻辑变更都应按仓库规则同步同目录独立测试 [`tidb_test.rs`](tidb_test.rs)，并尽量保持 [`tidb.go`](tidb.go) 与 [`tidb_test.go`](tidb_test.go) 的分支、错误和资源语义；测试逻辑不要内嵌回生产源文件。

兼容性风险集中在 session variables 和 DDL 的旧版本支持；正确性风险集中在大小写映射、默认值静默回退和 allocator 溢出；性能风险较低，但 `DBFromConfig` 的逐变量串行探测和两次连接是有意成本，改成批量或并发前必须证明失败隔离仍然成立。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter lightning/pkg/importer/tidb.rs` 确认目标文件含 31 个符号；`node --file ... --offset 1 --limit 500` 读取了完整 410 行实现。
- RustCodeGraph 精确符号查询覆盖：`DBFromConfig`、`NewTiDBManager`、`NewTiDBManagerWithDB`、`LoadSchemaInfo`、`ObtainImportantVariables`、`ObtainNewCollationEnabled`、`AlterAutoIncrement`、`adjustIDBase`、`AlterAutoRandom`、`DropTable`。
- RustCodeGraph 调用证据：查询结果确认 `precheck.rs::NewPrecheckItemBuilderFromConfig -> DBFromConfig`、`import.rs::setGlobalVariables -> ObtainImportantVariables`、`server/checkpoint_control.rs::DestroyError -> NewTiDBManager/DropTable`；同时仅在测试/契约测试中发现若干尚未接线 helper 的 Rust 调用。宽泛 `callers DBFromConfig` 命令曾无输出超时，故调用边以收窄后的 `explore`、精确 `query` 和调用方 `node` 片段交叉确认。
- 已读实现与边界：[`tidb.rs`](tidb.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`precheck.rs`](precheck.rs)、[`get_pre_info.rs`](get_pre_info.rs)、[`import.rs`](import.rs)、[`table_import.rs`](table_import.rs)、[`server/checkpoint_control.rs`](../server/checkpoint_control.rs)。目标包没有 `doc.go`。
- 已读 Go 对照与独立测试：[`tidb.go`](tidb.go)、[`tidb_test.go`](tidb_test.go)、[`tidb_test.rs`](tidb_test.rs)。Rust 测试覆盖删表、schema 大小写匹配/回调错误、auto-increment 普通与溢出、auto-random 封顶/no-op/`u64::MAX` 回绕、变量结果和默认回填、new-collation 权限错误/缺行/布尔值。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的结构命令验证本文恰有 11 个固定二级章节，并人工复核所有“已接线”结论均有调用边证据，未把 Go 主链或 slim stub 的预期能力写成 Rust 当前事实。

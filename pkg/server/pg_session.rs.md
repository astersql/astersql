# `pkg/server/pg_session.rs`

## 文件定位

本文件属于 `astersql-server` crate：`pkg/server/Cargo.toml` 以 `lib.rs` 为库入口，`pkg/server/lib.rs` 通过私有模块 `mod pg_session;` 装配它。它位于 PostgreSQL 协议兼容入口与原生 TiDB 执行上下文之间，保存单条 PostgreSQL 连接的 `search_path`，并识别少量必须具有 PostgreSQL 会话语义的语句。模块注释明确限定：这里的路径是 connection-private 状态，不会切换或改写底层原生 SQL mode。

真实上游有两条。简单查询协议由 `pkg/server/pg_conn.rs` 在收到 `Q` 消息后先调用 `SessionQuery::parse`，命中时直接通过 `extended.session.execute` 执行；扩展查询协议由 `pkg/server/pg_extended.rs` 在 Parse 阶段保存 `SessionQuery` 及其元数据，在 Execute 阶段才用同一个 `PgSession` 执行。未命中的 SQL继续进入 `CatalogQuery`、`pg_name::adapt` 或原生引擎。

## 核心职责

- `SessionQuery::parse` 使用 `pg_catalog_query::lex` 的有界词法规则，从普通 SQL 中严格识别 `SHOW/SET/RESET search_path`、`current_schema()` 和 `current_database()`，并区分“不是本模块语句”与“形似支持语句但语法/能力不合法”。
- `PgSession` 保存连接私有的 schema 搜索顺序；默认值为 `public`，可设置为 `public`、`pg_catalog` 的无重复组合或空列表。
- `PgSession::execute` 实施状态变更或构造单行结果。`current_database()` 不缓存数据库名，而是通过 `TiDBContext` 执行 `SELECT DATABASE()`；`current_schema()` 读取当前路径首项。
- `SessionQuery::metadata` 和 `command` 为简单/扩展协议提供统一的列描述与 CommandComplete 命令标签，使这些本地会话操作无需注册原生 prepared statement。
- `has_public`、`public_precedes_catalog` 和 `schema` 将当前路径传给 `pg_name` 与 `pg_catalog`，控制未限定原生关系是否可见、`public` 表是否遮蔽隐式 `pg_catalog` 关系，以及目录查询中的当前 schema。

## 主要符号

- `SessionQuery`：crate 私有、可克隆的语句分类。`Show` 返回路径文本；`Set(Vec<String>)` 替换路径；`Reset` 恢复默认；`Schema(String)` 和 `Database(String)` 保存结果列标签，而不是 schema/database 值。
- `PgSession { path: Vec<String> }`：连接私有状态容器。`Default` 建立 `vec!["public"]`；字段不对模块外暴露，所有观察和修改都经过方法。
- `PgSession::public_precedes_catalog`：只有路径中同时显式出现 `public` 与 `pg_catalog` 且前者索引更小时才返回真。若省略 `pg_catalog`，注释说明它按 PostgreSQL 规则隐式先于用户 schema 搜索，因此不会把 `public` 当成先行项。
- `PgSession::has_public`：告诉 `pg_name::adapt/rewrite` 是否允许把未限定原生关系映射到当前 TiDB database；路径不含 `public` 时，未限定原生关系应保持隐藏。
- `PgSession::schema`：返回路径首项，空路径返回 `None`；该值同时支持 `current_schema()` 和目录查询的 `current_schema`。
- `PgSession::execute`：唯一状态执行入口，返回 `ConnResult<QueryResult>`；结果总是附带 `context.state()`。
- `SessionQuery::{command, metadata, parse}`：分别提供协议命令名、prepared/row description，以及严格分类和 SQLSTATE 错误。

## 执行流程

1. `SessionQuery::parse` 先调用共享 `lex`。词法失败被当成 `Ok(None)`，让正常 SQL 解析/执行链决定最终语法错误；仅删除一个末尾分号，因而不会接受拼接的第二条语句。
2. 对 `SHOW search_path`、`RESET search_path`，令牌必须恰好两个；多余令牌返回 `42601`。`SET search_path` 必须使用 `TO` 或 `=`；单个 `DEFAULT` 转为 `Reset`，单个空字符串转为 `Set([])`。
3. 普通 SET 列表必须以 schema、逗号、schema 交替。schema token 可来自未引号单词、双引号标识符或字符串，但值只允许精确的 `public`/`pg_catalog`，并拒绝重复项。
4. SELECT 分类允许可选的 `pg_catalog.` 前缀，只接受无参数的 `current_schema()` 或 `current_database()`；可选 `AS`，以及一个 word/quoted alias。函数之后仍有令牌（例如 `FROM`）时返回 `Ok(None)`，交由目录或原生查询路径处理。
5. 简单协议命中后，`pg_conn.rs` 立即调用 `PgSession::execute`、用 `command()` 写结果，并发送 ReadyForQuery。解析得到 SQLSTATE 错误时直接写 ErrorResponse，路径保持不变。
6. 扩展协议在 Parse 时用 `metadata()` 保存列描述且不调用原生 `prepare_statement`；Bind 把查询克隆进 portal；Execute 首次真正调用 `self.session.execute`。因此已 Parse 的 `current_schema()` 会观察到其后执行的 SET，而不会冻结旧值。
7. `execute` 对 Show/Schema/Database 产生一行一列，对 Set/Reset 产生零行零列；最后使用同一份 `metadata`、`context.state()` 和默认的其余 `QueryResult` 字段返回。

## 数据与状态

`PgSession.path` 是有序 `Vec<String>`，顺序同时影响 `current_schema()`、目录名遮蔽和名称改写。有效元素由解析器限制为 `public` 与 `pg_catalog`，且不可重复；空向量是受支持状态，对应 `SET search_path TO ''`，此时 `schema()` 为 `None`、`current_schema()` 返回 SQL NULL、`SHOW` 返回空文本。`Reset` 通过重新赋值 `Default` 恢复单元素 `public`。

每条 PostgreSQL 连接在 `pg_conn.rs` 中建立一个 `Extended`，后者内含一个默认 `PgSession`；测试用两条 socket 证明一条连接的 SET 不影响另一条。`SessionQuery` 本身只保存解析结果，可安全克隆到 statement/portal；真正可变状态只在 Execute 时由 `&mut PgSession` 修改。

返回元数据使用现有 MySQL/TiDB 内部表示：有结果的 Show/Schema/Database 建立一列 `ColumnInfo` 和一个 `NativeType`，内部类型码为 253、charset 45、长度 64；PG 线协议转换由后续 `pg_result` 完成。Set/Reset 的元数据列为空。`statement_id` 和参数数目均为 0，因为本地会话命令没有后端 prepared statement 资源。

## 依赖与调用关系

上游直接调用点包括：`pkg/server/pg_conn.rs` 的简单查询分流；`pkg/server/pg_extended.rs` 的 Parse、metadata、command、portal clone 与 Execute；`pkg/server/pg_catalog.rs::parse_session_with_types` 调用 `public_precedes_catalog`，并在执行 portal 前由 `pg_extended.rs` 用 `schema()` 注入当前 schema；`pkg/server/pg_name.rs::adapt` 调用 `has_public` 决定原生关系改写。

下游只依赖同 crate 的窄接口：`pg_catalog_query::{lex, Token, ParseResult}` 提供 PostgreSQL 兼容词法和 `(SQLSTATE, message)` 错误；`conn::{TiDBContext, QueryResult, PreparedMetadata, ColumnInfo, NativeType, Value}` 提供引擎查询、协议状态与结果模型。Database 分支还创建一次新的 `CancellationToken` 调用 `context.execute_query("SELECT DATABASE()", false, ...)`。

`pkg/server/Cargo.toml` 将该代码归入 `astersql-server`，并声明对 session、sessionctx、parser、infoschema 等 workspace crate 的服务器级依赖；本文件没有新增独立第三方 crate 或 feature gate。RustCodeGraph 能定位 `SessionQuery`、`PgSession`、`execute`、`command`、`metadata`、`parse` 等符号；精确 callers 查询未给出方法调用边，因此上述调用关系由调用点源码补证。

## 错误处理与边界

`ParseResult<Option<SessionQuery>>` 的三态是重要边界：`Ok(Some(_))` 表示本模块负责；`Ok(None)` 表示交给后续 SQL/目录链；`Err((state, message))` 表示已识别为会话语句但必须向 PG 客户端报错。SHOW/RESET 的尾随内容、SET 列表结构错误使用 `42601`；未知 schema、大小写不匹配的引号名和重复 schema 使用 `0A000`。测试表明错误不会改变已有路径。

实现刻意不是完整 PostgreSQL `search_path`：不支持任意 TiDB database/schema、`$user`、NULL、复杂 SET 表达式或重复项；只支持兼容层能可靠解释的 `public` 和 `pg_catalog`。`current_schema/current_database` 只接管最简单的独立投影；带 `FROM` 的调用必须由目录查询或后续改写处理，避免本地单行结果错误吞掉真实查询。

`lex` 失败返回 `Ok(None)`，并不等同于接受非法 SQL。Database 执行的引擎错误通过 `?` 原样进入 `ConnResult`；查询没有结果、首行或首列时有意退化为 `Value::Null`。本文件不验证 `public` 在 TiDB infoschema 中真实存在，因为它是 PG 兼容命名空间；`pg_query_test.rs` 明确验证 TiDB schema 列表中无需存在名为 public 的实际 schema。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或长生命周期资源。状态所有权落在单连接的 `Extended.session`，协议处理循环通过可变借用串行修改；不同连接拥有不同 `PgSession`，无需同步原语。

SessionQuery 在扩展协议中的生命周期跨 Parse、Bind、Describe、Execute：statement 和 portal 各持有 clone，但 clone 不包含会话快照。Execute 时才读取/修改共享的连接级 `PgSession`，这是 named prepared statement 能看到后续 SET 的关键不变量。会话查询的 `statement_id == 0`，`pg_extended.rs` 在 reset/close/replace 时通过 `session_query.is_none()` 保护，避免把本地语句当作原生句柄关闭。

唯一临时引擎资源来自 Database 分支的同步 `execute_query` 和局部 `CancellationToken`，调用返回后即释放；本文件没有 response lifecycle。`QueryResult.state` 每次从当前 context 获取，使本地命令的事务状态仍与连接引擎状态一致。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/server"` 给出包级对照，但 `pkg/server` 的 Go 文件中检索不到 `pg_session`、`search_path`、`current_schema` 或对应 PostgreSQL 会话实现，也不存在 `pkg/server/pg_session.go`。因此本文件不是同路径 Go 源的逐函数移植，而是 Rust 服务端新增的 PostgreSQL 协议兼容层；Go `pkg/server` 主要承载既有 MySQL 协议连接与执行服务。

它复用的 Go/TiDB 语义边界是 `TiDBContext` 背后的当前数据库和事务状态：`current_database()` 委托原生 `SELECT DATABASE()`，而不是在 PG 兼容层另建数据库状态。相反，`search_path` 明确保持为 PG 连接私有状态，不能写入 TiDB session SQL mode 或把 `public` 假装成真实 TiDB database。后续对齐应以现有 Rust PG 协议测试和原生 context 行为为依据，不能假定存在尚未找到的 Go 等价实现。

## 扩展指南

新增会话语句时，应同时更新 `SessionQuery` 变体、`parse` 的严格边界、`command`、`metadata` 和 `execute`，并审计简单协议与扩展协议两条路径。凡是返回结果的变体必须保证 Parse/Describe 元数据与 Execute 结果列数、名称和类型一致；凡是本地执行的变体必须保持 `statement_id == 0` 语义，避免触发原生 prepared statement 关闭。

扩展 search_path 支持时，必须同步审计 `public_precedes_catalog`、`has_public`、`schema`、`pg_catalog.rs` 的隐式关系遮蔽和 `pg_name.rs` 的关系改写。任意 schema、`$user` 或大小写/引号规则不能只放宽解析器；还需证明该名字如何映射到 TiDB database/infoschema，并保持显式 `pg_catalog`、隐式 catalog、CTE 和原生表的优先级。

测试逻辑必须继续放在独立文件。解析器边界优先扩展 `pkg/server/pg_session_test.rs`；连接隔离、错误恢复、名称可见性与简单/扩展协议行为扩展 `pkg/server/pg_query_test.rs`；目录遮蔽扩展 `pkg/server/pg_catalog_test.rs`；Parse/Bind/Describe/Execute 的元数据或 portal 生命周期可补在 `pkg/server/pg_extended_test.rs`。重点兼容风险是客户端依赖的 SQLSTATE、列标签/类型、CommandComplete 标签、路径优先级和 prepared statement 的执行时状态。

## 验证依据

- 生产源码：`pkg/server/pg_session.rs`；核对了 `SessionQuery` 五个变体、`PgSession` 默认值和三个观察方法，以及 parse/metadata/command/execute 全部分支。
- crate 与装配：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`；`pkg/server/doc.go` 不存在，故无额外 package contract 可读。
- 上游与下游接线：`pkg/server/pg_conn.rs`（简单查询分类和执行）、`pkg/server/pg_extended.rs`（Parse/Bind/Describe/Execute 与资源区分）、`pkg/server/pg_catalog.rs`（路径遮蔽和当前 schema）、`pkg/server/pg_name.rs`（public 可见性与关系改写）。
- 独立测试：`pkg/server/pg_session_test.rs` 覆盖分类与拒绝边界；`pkg/server/pg_query_test.rs` 覆盖默认值、空路径、连接隔离、SQLSTATE、RESET、关系可见性和 Execute 时读取最新 schema；`pkg/server/pg_catalog_test.rs` 覆盖 public/pg_catalog 遮蔽与 CTE 优先级。
- Go 对照：检索 `pkg/server/**/*.go` 未发现 search_path/current_schema/current_database 的 PG 会话实现；只把 `Cargo.toml` 的包级 porting 声明及 `TiDBContext` 原生数据库语义作为对照。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`query PgSession`、`query SessionQuery`、`query public_precedes_catalog`、`query has_public`、`query command/metadata/parse` 均定位到目标符号。`files --filter pg_session` 与精确 callers/callees 未产出有效边，已用上述直接调用点补证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级章节，并人工检查唯一生产物、事实可追溯性及无内嵌测试建议。

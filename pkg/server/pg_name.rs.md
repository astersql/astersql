# `pkg/server/pg_name.rs` 逻辑说明

## 文件定位

`pkg/server/pg_name.rs` 属于 `astersql-server` crate；crate 根在 `pkg/server/lib.rs`，通过私有的 `mod pg_name;` 挂载本模块，因此这里的 API 只服务于 server crate 内部。`pkg/server/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/server`，并声明本文件直接使用的 `astersql-parser` 与 `astersql-parser-ast` 路径依赖；该 manifest 没有为本模块设置独立 feature。

本文件位于 PostgreSQL 协议 SQL 与原生 MySQL 方言执行引擎之间，职责不是执行 SQL 或完成对象权限检查，而是在 SQL 进入原生解析/执行链之前处理 PostgreSQL 标识符和关系名：把双引号标识符转换为反引号，把 PostgreSQL 的 `public`/未限定关系映射到当前原生数据库，并拒绝当前适配层无法安全表达的命名形式。文件头注释和 `adapt`、`validate_relations`（`pkg/server/pg_name.rs:439,467`）共同限定了这个边界。

它有两条生产入口：普通/扩展 PostgreSQL 查询通过 `pkg/server/pg_conn.rs` 和 `pkg/server/pg_extended.rs::process` 调用 `adapt`；`pkg/server/pg_catalog.rs::evaluate` 在实现 `pg_get_viewdef` 时调用 `stored_view_select`，从原生 `ViewInfo` 保存的文本中取出 SELECT 定义。

## 核心职责

1. `source_tokens`/`tokens`（`pkg/server/pg_name.rs:31-135`）进行保持字节跨度的轻量词法扫描。它跳过空白和注释，但保留字符串、标识符、符号在原 SQL 中的起止位置，以便后续只替换必要片段而保留其余字面文本。
2. `Resolver::scope` 与 `Resolver::relation`（`pkg/server/pg_name.rs:218-407`）识别查询作用域、CTE 和关系出现位置，将可支持的 PostgreSQL 关系名规范化为原生 `` `database`.`table` ``。
3. `rewrite`（`pkg/server/pg_name.rs:410-437`）汇总双引号标识符替换与关系名替换，并按原始偏移一次性重建 SQL。
4. `adapt`（`pkg/server/pg_name.rs:439-463`）为协议主链提供按需适配：无关系查询不访问引擎；只有遇到“未选择当前数据库”信号时才执行 `SELECT DATABASE()`，随后重写并做 AST 级跨库校验。
5. `validate_relations`（`pkg/server/pg_name.rs:467-509`）使用原生 AST visitor 补查轻量 token 规则未覆盖的 `TableName` 节点，拒绝当前数据库之外的 schema。
6. `stored_view_select`（`pkg/server/pg_name.rs:140-189`）校验持久化视图源文本，并从 `CREATE VIEW ... AS ... [WITH ... CHECK OPTION]` 中提取 SELECT；已经是单条 SELECT/集合操作时原样去除首尾空白后返回。

## 主要符号

- `Token`（`pkg/server/pg_name.rs:9`）：内部词法单元，保存 `start`/`end` 字节偏移、可选规范化名称、是否为带引号标识符以及单字节符号。`Token::word` 仅对未加引号名称做 ASCII 大小写无关的关键字判断；因此带引号的 `"PUBLIC"` 不会被当成 PostgreSQL 的未引号 `public`。
- `error(code, message)`（`pkg/server/pg_name.rs:25`）：构造共享的 `ParseResult` 错误 `(SQLSTATE, String)`；`ParseResult<T>` 定义在 `pkg/server/pg_catalog_query.rs:5`。
- `quote(name)`（`pkg/server/pg_name.rs:28`）：输出原生反引号标识符，并将名称内反引号加倍，避免改写结果破坏标识符边界。
- `tokens` 与 `source_tokens`（`pkg/server/pg_name.rs:31,34`）：前者固定使用 PostgreSQL 输入模式，后者的 `native` 参数只供持久化视图文本使用，使原生反引号可以被扫描，但禁止 PostgreSQL 输入借此绕过限制。
- `stored_view_select`（`pkg/server/pg_name.rs:140`，`pub(crate)`）：系统目录路径可调用的视图 SELECT 提取器。
- `Resolver`（`pkg/server/pg_name.rs:190`）：一次重写的临时状态，持有 token、当前数据库、`public` 是否在 search path 中，以及按起始偏移排序的编辑集合 `BTreeMap<usize, (usize, String)>`。
- `Resolver::close`（`pkg/server/pg_name.rs:203`）：在指定 token 范围内配对圆括号；找不到闭合符时返回 `42601`。
- `Resolver::relation`（`pkg/server/pg_name.rs:218`）：消费一个关系引用，区分 CTE、子查询、函数式调用和一至三段限定名，并创建覆盖整个关系跨度的编辑。
- `Resolver::scope`（`pkg/server/pg_name.rs:297`）：递归遍历 CTE/括号子作用域，依据 `FROM`、`JOIN`、`INTO`、`UPDATE`、DDL 的 `TABLE`/`VIEW`、`REFERENCES`、`LIKE`、`RENAME TO` 等位置驱动 `relation`。
- `rewrite`（`pkg/server/pg_name.rs:410`，`pub(crate)`）：纯字符串适配入口，也是 `pkg/server/pg_name_test.rs` 的直接测试对象。
- `adapt`（`pkg/server/pg_name.rs:439`，`pub(crate)`）：依赖连接上下文与 `PgSession` 的生产入口。
- `validate_relations`（`pkg/server/pg_name.rs:467`）：私有的原生 AST 二次防线；局部 `Guard` 实现 `astersql_parser_ast::Visitor`。

本文件没有模块级常量、trait、条件编译项或长期存活的全局状态。

## 执行流程

协议主流程如下：

1. 简单查询路径在 `pkg/server/pg_conn.rs:430`、扩展协议 Parse 路径在 `pkg/server/pg_extended.rs:506-547` 进入 `adapt`。扩展路径先排除 session query 和内置 catalog query，避免对这些专用语法重复处理。
2. `adapt` 先调用 `rewrite(sql, "", session.has_public())`。若 SQL 没有需要数据库名的原生关系，这一步直接得到结果，故 `SELECT 1` 等查询不会额外访问引擎。
3. 若预检只因空数据库返回 `3D000`，`adapt` 通过 `TiDBContext::execute_query("SELECT DATABASE()", false, CancellationToken::new())` 获取当前数据库。其他词法/命名错误立即返回。
4. `rewrite` 调用 `tokens`。扫描时字符串只用于保护跨度，不产生名称；双引号标识符解码成名称；普通标识符转成小写；注释被跳过但原文不会从最终 SQL 中删除。
5. `rewrite` 先为每个双引号标识符登记反引号替换，再构造 `Resolver` 调用 `scope`。`scope` 克隆继承的 CTE 集合；每个 CTE 子查询在当前已可见 CTE 集合下递归解析，随后才把当前 CTE 名加入集合。
6. `relation` 跳过 `IF NOT EXISTS`、`ONLY` 等前缀。单段名称若命中 CTE 则不改写；否则仅接受单段名称、`public.table`，或数据库等于当前数据库的 `database.public.table`。成功时登记整个关系跨度替换，并删除被该跨度吞并的单个标识符编辑。
7. `rewrite` 按 `BTreeMap` 的递增偏移拼接未修改原文和替换文本，保证编辑顺序确定且不在原字符串上原地移动偏移。
8. 第二次 `rewrite` 后，`adapt` 调用 `validate_relations`。后者先通过 `pkg/server/pg_extended.rs::markers` 将 PostgreSQL 参数标记转换成原生解析器可接受形式；若原生解析成功，则访问所有 `TableName`，只要显式 schema 非空且不等于当前数据库（按小写比较）就返回 `0A000`。原生解析失败在这里不提前报错，语法错误仍留给后续协议 command gate/原生执行链，以维持既有错误优先级。

视图定义流程独立：`pkg/server/pg_catalog.rs:1786` 在 `pg_get_viewdef` 求值时把 `ViewInfo.SelectStmt` 传给 `stored_view_select`；该函数先要求原生解析器得到恰好一条 SELECT、集合操作或 CREATE VIEW。对 CREATE VIEW，它以 native token 模式寻找 VIEW 之后的 AS，去掉末尾分号和可选的 `WITH [CASCADED|LOCAL] CHECK OPTION`，最后返回中间原文本。

## 数据与状态

所有状态均限于一次函数调用：`Vec<Token>` 保存扫描结果，`HashSet<String>` 保存当前/继承 CTE 名，`BTreeMap` 保存互不重叠的文本编辑。`Resolver` 借用 `database: &str`，不拥有连接或会话，也不会把状态写回 `PgSession`。

token 的 `start`/`end` 是原 SQL 的字节偏移。扫描对 ASCII 语法字节逐个前进，对非 ASCII 字节把它们视为标识符组成部分；切片边界来源于扫描得到的位置。普通标识符调用 `to_lowercase()`，带双引号的名称保留大小写并解码成对双引号。最终替换中的数据库名和表名均经 `quote` 转成安全的原生标识符。

`public: bool` 是 `PgSession::has_public`（`pkg/server/pg_session.rs:34`）的快照，只控制未限定名称能否通过 search path 解析；显式 `public.table` 仍可改写。当前数据库来自 `SELECT DATABASE()` 的第一个结果集、第一行、第一列，且仅接受 `Value::Text`；其他形状统一视为空数据库，下一次 `rewrite` 将返回 `3D000`。

## 依赖与调用关系

上游调用边：

- RustCodeGraph 的 `pg_name.rs::adapt` 节点确认其调用者为 `pkg/server/pg_extended.rs::process`；源码还显示简单查询路径 `pkg/server/pg_conn.rs:430` 直接调用同一入口。
- RustCodeGraph 的 `pg_name.rs::stored_view_select` 节点确认其调用者为 `pkg/server/pg_catalog.rs::evaluate`，位于 `pg_get_viewdef` 分支。
- `pkg/server/lib.rs:36` 私有挂载模块，并在测试构建中于 `pkg/server/lib.rs:39` 挂载独立的 `pg_name_test.rs`。

下游调用边：

- `adapt` 调用本文件的 `rewrite`、`validate_relations`，调用 `pkg/server/pg_conn.rs::sqlstate` 映射引擎错误，并通过 `TiDBContext::execute_query` 查询当前数据库。
- `rewrite` 调用 `tokens`、`quote`、`Resolver::scope`；`scope` 调用 `close`、`relation` 并递归调用自身；`relation` 写入 `Resolver::edits`。
- `validate_relations` 调用 `pkg/server/pg_extended.rs::markers`，依赖 `astersql_parser::New().ParseSQL` 和 `astersql_parser_ast::{Visitor, TableName}`。
- `stored_view_select` 同样依赖原生 parser/AST 作语句种类校验，再调用 `source_tokens(native = true)` 按原始跨度提取文本。

标准库依赖仅为 `BTreeMap`（稳定应用编辑顺序）与 `HashSet`（CTE 可见性判断）。`pkg/server/Cargo.toml` 的 parser/AST 依赖是本文件的直接 crate 边界依据；连接抽象、会话和协议辅助均来自当前 `astersql-server` crate。

## 错误处理与边界

错误均使用 `ParseResult<T> = Result<T, (&'static str, String)>`，由调用协议层把 SQLSTATE 和消息编码为 PostgreSQL ErrorResponse。关键边界为：

- `42601`（语法错误）：未终止注释/引号、空的双引号标识符、未闭合括号、缺少关系标识符、CTE 名或 CTE 查询结构。
- `0A000`（不支持）：PostgreSQL 输入中的反引号、`#`、非参数形式 `$` 引用；MySQL 可执行注释；嵌套块注释；递归 CTE；关系括号组；原生表函数；跨库、非 `public` 或超过三段的关系名；AST 校验发现当前数据库外关系；不合法的持久化视图源。
- `42P01`：未限定关系出现时 `public` 不在当前 `search_path`。显式 `public.table` 不受该检查限制，这一行为由 `pkg/server/pg_name_test.rs` 和 `pkg/server/pg_query_test.rs::pg_introspection_names_live_tables` 共同固定。
- `3D000`：需要改写真实关系却没有当前数据库。`adapt` 将第一次的该错误用作惰性查询数据库的内部控制信号；若查询后仍无文本数据库名，第二次重写才把它暴露给调用者。

`validate_relations` 特意忽略原生 parser 的解析失败，而不是把它转换为成功执行：其注释说明 command gate/执行层仍负责语法错误，本函数只对成功得到的 AST 做额外关系校验。这避免关系检查改变扩展协议参数 OID 校验与语法错误的先后顺序。

当前词法器是范围受控的兼容层，不是完整 PostgreSQL parser。新增语法若会在 `FROM` 等关键字附近出现非关系表达式，必须先验证 `scope` 的状态机不会误判；不能仅让原生 parser 接受就宣称 PostgreSQL 语义已支持。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或静态可变状态。`Token`、`Resolver`、CTE 集合、编辑集合和 AST `Guard` 全部在同步调用栈内创建并在返回时释放，所以同一函数可被多个连接并发调用，彼此不共享本模块状态。

唯一外部资源交互发生在 `adapt`：它可能同步调用一次 `TiDBContext::execute_query`，并为该查询创建临时 `CancellationToken`。token 未存入结构或跨调用复用；本文件也不开始、提交或回滚事务。是否处于事务以及查询资源的真实生命周期由传入的 `TiDBContext` 和上游连接处理器管理。

性能上，通常需要一次词法扫描、一次作用域扫描以及按编辑数排序的 `BTreeMap` 插入/遍历；需要关系改写时还会查询当前数据库并尝试一次原生 parse/AST 遍历。首轮空数据库重写用于避免无关系 SQL 产生额外引擎查询，这是 `adapt` 注释明确的不变量。递归子查询会克隆 CTE `HashSet`；扩展复杂查询时应关注深嵌套与大量 CTE 的时间/内存成本。

## 与 Go 版本的对应关系

仓库不存在同路径 `pkg/server/pg_name.go`，在 `pkg/server` 的 Go 文件中也找不到本文件的同名函数或这些 SQLSTATE 错误文本。因此没有可声称逐函数对应的 Go 实现；`pkg/server/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/server"` 只能证明 crate 的包级迁移归属，不能证明该 Rust 文件是某个 Go 文件的机械翻译。

Go 侧 `pkg/server/server.go` 属于原有 TiDB server 主体，而这里的 `pg_*` 模块由 `pkg/server/lib.rs` 在 Rust server crate 内装配，服务新增的 PostgreSQL 协议兼容路径。`pkg/server/internal/testserverclient/server_client.go` 中的 CREATE VIEW 用例证明 Go server 测试覆盖原生视图行为，但不覆盖 PostgreSQL 名称重写、search path、双引号转义或 `pg_get_viewdef` 的文本提取规则。

因此迁移/兼容判断应以行为边界为单位：最终 SQL 必须仍由原生 parser、对象查找与授权链验证；本文件只弥合 PostgreSQL 命名语法。未来若 Go 侧加入对应适配，需逐项比较 SQLSTATE、search path、CTE 可见性、视图源文本保真和协议错误顺序，而不能只比较重写后的字符串。

## 扩展指南

- 扩展可识别关系位置（例如新的 DDL 子句）时，优先修改 `Resolver::scope`；若变化涉及限定名段数、CTE/子查询/表函数判断或 search path，则修改 `Resolver::relation`。必须同步扩展 `pkg/server/pg_name_test.rs` 的成功与拒绝表，并在 `pkg/server/pg_query_test.rs::pg_introspection_names_live_tables` 增加真实协议回归。
- 扩展引号、注释或标识符规则时修改 `source_tokens`，保持 `native = false` 的客户端输入与 `native = true` 的存储源文本模式隔离；同步覆盖未终止输入、转义、注释内关键字和非 ASCII 标识符。不要把测试嵌入生产源文件。
- 扩展 view source 格式时修改 `stored_view_select`，先用原生 AST 限定允许的语句类型，再以 token 跨度提取；同步 `pkg/server/pg_client_integration_test.rs::pg_introspection_clients_stored_view_source`，尤其验证引号/注释中的 `AS` 和尾部 CHECK OPTION。
- 新增原生 AST 可表达但 token 状态机看不到的关系节点时，扩展 `validate_relations::Guard`。需保持“解析失败不在这里抢先返回”的错误顺序约束，并在扩展协议测试中验证参数 OID 错误优先级不回归。
- 修改数据库获取策略时聚焦 `adapt`，保留无关系 SQL 零额外查询的不变量。若引入缓存，必须明确连接切库、事务和 session 生命周期中的失效规则；当前实现没有缓存，天然读取调用时的数据库。
- 兼容风险主要是错误 SQLSTATE/先后顺序、带引号大小写、search path 和跨库隔离；性能风险主要是对所有查询无条件访问引擎或增加重复解析。任何支持面扩大都应同时经过纯重写测试和简单/扩展协议真实链路测试。

## 验证依据

本说明基于以下直接证据：

- 生产源码：`pkg/server/pg_name.rs` 全部 509 行；模块与会话边界：`pkg/server/lib.rs`、`pkg/server/pg_session.rs`；调用入口：`pkg/server/pg_conn.rs`、`pkg/server/pg_extended.rs`、`pkg/server/pg_catalog.rs`；错误类型：`pkg/server/pg_catalog_query.rs`。
- crate 配置：`pkg/server/Cargo.toml`，确认 crate 名、`lib.rs` 根、Go 包迁移元数据、parser/AST 依赖及无本模块 feature。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/server/pg_name.rs` 读取全文件；`node pg_name.rs::adapt` 确认 `process → adapt → rewrite/validate_relations`；`node pg_name.rs::stored_view_select` 确认 `evaluate → stored_view_select`；`node pg_name.rs::rewrite`、`scope`、`validate_relations` 确认内部调用边。常见名的独立 `callers/callees` 查询存在歧义，故只采用带文件限定节点和源码可复核的边。
- 独立测试：`pkg/server/pg_name_test.rs` 验证字符串/注释保护、双引号、CTE 作用域、DML/DDL 重写和 SQLSTATE 拒绝边界；`pkg/server/pg_query_test.rs::pg_introspection_names_live_tables` 验证简单/扩展协议、真实多数据库隔离、search path 和 CRUD；`pkg/server/pg_extended_test.rs` 验证 prepared statement 中的 `public` 关系；`pkg/server/pg_client_integration_test.rs::pg_introspection_clients_stored_view_source` 验证视图源提取。
- Go 对照搜索：`pkg/server` 下无 `pg_name.go`，Go 文件中无对应符号/错误文本；仅 `pkg/server/internal/testserverclient/server_client.go` 有一般 CREATE VIEW 测试，因此文档明确标为 Rust PG 专用新增逻辑而非直接 Go 复刻。

本任务是只读行为分析与文档新增，按计划未运行 Cargo 或代码测试。交付结构检查要求目标文件存在且恰有上述 11 个固定二级标题；人工复核重点是每项行为均可回溯到点名的符号、调用边或测试，且没有把原生 parser/授权职责误写成本文件职责。

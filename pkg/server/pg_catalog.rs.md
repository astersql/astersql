# `pkg/server/pg_catalog.rs`

## 文件定位

本文件属于 `astersql-server` crate（见 `pkg/server/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/server/lib.rs` 以私有模块 `mod pg_catalog;` 装配。它不是通用 SQL 执行器，而是 PostgreSQL 协议入口中的一个有界目录查询适配层：识别 DataGrip 等客户端发出的 `pg_catalog` 探测 SQL，把 AsterSQL/TiDB 的 schema snapshot、表模型和少量原生系统查询投影成 PostgreSQL 风格的目录行，再返回协议层可直接编码的 `QueryResult`。

上游有两条真实入口。简单查询协议在 `pkg/server/pg_conn.rs` 中调用 `CatalogQuery::parse_session`，命中后直接调用 `CatalogQuery::execute`；扩展协议在 `pkg/server/pg_extended.rs` 的 Parse 阶段调用 `parse_session_with_types`，Bind 阶段调用 `bind_values`，Describe 使用 `metadata`，Execute 最终仍由 `pg_conn.rs` 调用 `execute`。未命中的 SQL继续走 `pg_name::adapt` 和原生引擎，不由本文件接管。

## 核心职责

- `CatalogQuery` 将 `pg_catalog_query` 产生的有限 AST 做绑定、列解析、类型推断和合法性检查；它只接受代码中列出的目录关系、表达式、函数及查询形态。
- `Execution` 为一次执行固定当前数据库、单个 `SchemaRef` snapshot、取消令牌、provider/CTE 缓存和资源计数，保证同一查询的对象身份与元数据来自同一视图。
- `provider_rows` 负责把 `pg_class`、`pg_namespace`、`pg_attribute`、`pg_type`、`pg_index`、`pg_constraint`、`pg_description`、`pg_sequence`、`pg_depend` 等 provider 映射为固定槽位行；没有可靠原生语义的 PG 对象返回有类型的空关系，而不是虚构对象。
- `evaluate`、`project`、`execute_rows` 和 `execute_select` 实现受限的 JOIN/LEFT JOIN、CTE、相关或非相关子查询、UNION、过滤、排序、DISTINCT、LIMIT、数组、`unnest`、`array_agg` 和三值逻辑。
- `DATABASES_SQL`、`TRANSACTIONS_SQL` 是协议/客户端测试使用的代表性目录探测语句；`namespace_oid` 与 `catalog_indexes` 是同 crate 内 OID 解析和目录投影共享的窄接口。

## 主要符号

- `CatalogQuery { select, current_schema, public_first, cte_columns, parameter_oids, parameter_values }`：已解析并绑定的目录语句。`parse` 用于无会话分类；`parse_session`/`parse_session_with_types` 结合 `search_path`、当前数据库和参数 OID；`classify` 是容错分类入口。
- `CatalogQuery::bind`：递归绑定 CTE/UNION/子查询，生成 NATURAL JOIN 条件，展开 CTE 的 `*`，执行 UNION 数值提升，最后调用 `validate`。
- `CatalogQuery::column` 与 `expr_type`：将列映射到 `relation_index * CATALOG_ROW_WIDTH + slot`，并给表达式赋 MySQL/PG 线协议需要的类型码与 flags。`CATALOG_ROW_WIDTH = 25` 包含 `pg_proc.proacl` 的第 24 槽。
- `CatalogQuery::metadata`：构造列描述和 native type；目录语句不分配后端 prepared statement，因此 `statement_id` 固定为 0。
- `CatalogQuery::execute`：取得 schema snapshot 和 `SELECT DATABASE()`，建立 `Execution`，执行 AST，返回没有后端 result-set 生命周期的 `QueryResult`。
- `Execution::materialize` 与 `comparison`：分别累计物化行数和连接/比较工作量，落实 `MAX_CATALOG_ROWS = 16_384`、`MAX_CATALOG_JOIN_WORK = 100_000`；`MAX_CATALOG_CTE_COLUMNS = 24` 单独限制公开 CTE 投影。
- provider 构造器：`class_rows`、`native_metadata_rows`、`sequence_dependency_rows`、`function_rows`、`column_catalog_rows`、`index_constraint_rows`。
- 类型/定义辅助：`native_column_type`、`format_column_type`、`column_default`、`catalog_parameter_type`、`numeric_array_values`、`array_text`、`pg_identifier`。
- 身份辅助：`namespace_oid` 委托 `pg_oid::namespace_oid`；`catalog_indexes` 补出 `PKIsHandle` 且缺少显式主索引时的本地虚拟主键索引（局部 ID `-1`），同时拒绝非正的既有索引 ID。

## 执行流程

1. 协议层先尝试会话命令，再把 SQL 交给 `CatalogQuery::parse_session` 或 `parse_session_with_types`。当 `public` 位于 `pg_catalog` 前时，后者查询 `SELECT DATABASE()` 并从 snapshot 找出会遮蔽隐式目录名的 public 表，交给 `pg_catalog_query::parse_shadowed`。
2. `parameters` 跨 CTE、UNION 和子查询统计参数；只有直接 `::oid` 可把未知 OID 推断为 26，其余未知或不支持的参数类型在 Parse 阶段报错。扩展协议 Bind 后由 `bind_values` 校验数量并保存字面表达式。
3. `bind` 递归处理作用域和类型：CTE 记录输出列；NATURAL JOIN 按共享列生成等值条件；`*` 仅对具有已知列集合的 CTE 展开；UNION 校验列数并把兼容数值臂提升为 bigint；`validate` 拒绝未知 provider、重复别名、非法函数、非法聚合和作用域越界。
4. `execute` 固定 schema snapshot 与当前数据库。`execute_select` 先物化 CTE 和 IN 子查询，再执行主查询与 UNION；`execute_rows` 对每个 provider 只构造并缓存一次行集，然后按顺序做连接。LATERAL 表函数逐左行求值，LEFT JOIN 未匹配时补一个 25 槽 NULL 行。
5. `project` 依次执行 WHERE、排序键、聚合或普通投影。投影中的多个 `unnest` 按 PostgreSQL SRF 语义拉链对齐，短数组补 NULL；之后执行 DISTINCT/LIMIT。最外层显示阶段才把 regclass OID 转成人类可读名称。
6. `QueryResult` 携带 `metadata` 的列与 native types、计算出的 rows 和 `context.state()` 返回 `pg_conn`，再由 `pg_result` 编码为 PostgreSQL 消息。

## 数据与状态

目录行使用 `Vec<Value>`，每个关系占 25 个固定槽，连接时直接拼接，因此 `column` 的槽位计算是整个执行器的重要不变量。provider 与 CTE 行以 `Arc<Vec<Vec<Value>>>` 缓存；缓存只存在于单次 `Execution`，不会跨查询共享。

`Execution.snapshot` 是一次执行唯一的 schema 视图；`database` 同样只获取一次。`pg_class`、`pg_attribute`、`pg_index`、regclass 显示和定义反解都应使用这组一致数据。当前用户、事务列表和当前 TSO 则通过 `TiDBContext` 的受控查询或身份接口获取：`mysql.user` 映射 `pg_user`，`information_schema.tidb_trx` 映射 `pg_locks.transactionid`，`@@tidb_current_ts` 支撑 `txid_current()`。

OID 身份由 `pg_oid` 统一生成；`namespace_oid`、`table_oid`、`index_oid` 与 `SYSTEM_RELATIONS` 保证 provider、regclass 和定义查询指向同一对象。`COLUMN_TYPES` 只登记当前可可靠映射的内建类型。未建立等价语义的 FDW、策略、触发器、rewrite、PG collation/operator family 等 provider 保留列类型但返回空行。

## 依赖与调用关系

上游直接调用关系以源码为准：`pg_conn.rs` 调用 `CatalogQuery::parse_session`/`execute`；`pg_extended.rs` 调用 `parse_session_with_types`、`parameter_oids`、`metadata`、`bind_values`；`pg_oid.rs` 调用 `catalog_indexes` 来解析/枚举索引身份；测试通过 `DATABASES_SQL`、`TRANSACTIONS_SQL`、`namespace_oid` 和 `CatalogQuery` 检验协议行为。

下游主要依赖为：`pg_catalog_query::{Select, Expr, CastType}` 提供受限 AST；`conn::{TiDBContext, CancellationToken, Value, QueryResult, PreparedMetadata}` 提供执行边界；`astersql-infoschema::InfoSchema/SchemaRef` 和 `astersql-meta-model` 提供 schema、表、列、索引、外键、序列与视图模型；`pg_oid` 提供稳定 PG OID；`pg_name::stored_view_select` 提取持久化视图定义；`pg_result::CatalogColumnType` 提供 PG 特有数组/vector/char/ACL 类型码。

`pkg/server/Cargo.toml` 表明这些都是 `astersql-server` 的 crate 内模块或 workspace path 依赖，尤其直接声明了 `astersql-infoschema`、`astersql-meta-model`、`astersql-parser-ast`、`astersql-parser-mysql`。RustCodeGraph 的目标文件节点存在并显示全文件 3689 行；对 `parse_session_with_types`、`CatalogQuery::execute`、`catalog_indexes` 的精确 callers/callees 查询未产出方法边，因此调用方向由上述直接调用点复核，不把图缺失当作“无调用者”。

## 错误处理与边界

解析/绑定错误使用 `ParseResult` 的 PostgreSQL SQLSTATE：例如未确定参数类型 `42P18`、参数协议错误 `08P01`、语法/UNION 列数 `42601`、重复别名 `42712`、歧义列 `42702`、类型不匹配 `42804`，大量明确不支持的目录形态使用 `0A000`。运行时错误转为 `ConnError::Session` 或 `UnsupportedCommand(0)`，由 `pg_conn::sqlstate` 映射到协议错误。

关键硬边界包括：总物化行不超过 16,384；比较/连接工作不超过 100,000；CTE 输出不超过 24 列；每次循环和关键阶段检查取消；算术使用 checked 运算；OID、时间戳、事务 ID、列位置和类型 modifier 均检查范围。缺失 snapshot、损坏/不完整模型、非法数组、无穷数值、零日期、无法忠实翻译的 generated/default/check 表达式都会显式失败，而不是静默伪造结果。

该实现不是完整 PostgreSQL：`age(transactionid)` 只用于原生 TSO 的“越小越旧”排序，不能投影为 PG XID 年龄；`array_agg` 目前只接受 bigint；通配符仅支持已知 CTE 列；表空间等无原生来源的关系为空；`pg_get_userbyid` 没有原生 owner 映射时返回 NULL；未知对象定义和不支持的索引形态不会猜测。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。`Execution` 借用 `TiDBContext` 和 `CancellationToken`，内部使用 `RefCell`/`Cell`，明确是单线程、单次同步执行状态；它的生命周期止于一次 `CatalogQuery::execute`。`Arc` 仅用于同次执行中廉价复用 provider/CTE 行，并不表示跨线程共享缓存。

取消令牌来自 `pg_conn` 的 `catalog_cancel(pid)`，在入口、provider 扫描、连接、投影和元数据循环中反复检查。目录 prepared statement 只保存在 `pg_extended::Statement/Portal` 中，`metadata.statement_id == 0`，因此关闭或替换它时不应调用原生后端 statement 关闭逻辑；`pg_extended.rs` 正是以 `catalog.is_none() && session_query.is_none()` 区分原生句柄。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 将 crate 对应到 Go 包 `pkg/server`，但在该 Go 目录中检索不到 `pg_catalog`、DataGrip 或 PostgreSQL 目录适配实现，也不存在 `pkg/server/pg_catalog.go`。因此本文件不是同路径 Go 文件的机械翻译，而是 Rust 服务器新增的 PostgreSQL 3.2 协议兼容层；Go `pkg/server` 仍主要是 MySQL 协议服务端实现。

可对齐的是底层数据语义而非文件结构：Rust 从与 Go/TiDB 共用的 infoschema/model 概念读取 public schema/table/column/index/foreign key/sequence，并坚持不把 MySQL/TiDB 特性伪装成不存在的 PG 对象。例如 auto-increment 不当作 PG sequence，native partition 不生成 inheritance 边，MySQL collation 不生成 PG collation，持久化 view source 通过专用转换读取。扩展该层时应继续以真实 model 字段和原生查询为证据，不能仅按 PostgreSQL 目录定义填充合成数据。

## 扩展指南

新增 provider 时，应同时修改 `validate` 的允许关系、`column`/`column_catalog_field` 的列槽和类型、`provider_rows` 的真实数据来源，并在 `CATALOG_ROW_WIDTH` 内保持所有 JOIN 行宽一致；若需要更多槽，必须审计全部 provider、LEFT JOIN NULL 扩展、测试中的 row width 以及 `pg_proc.proacl`。新增表达式/函数要同步更新 `expr_type` 与 `evaluate`，二者必须保持“可绑定即能执行”，并补 `visit_expr`、聚合/相关子查询限制和 NULL 三值语义。

新增原生类型映射应同步 `COLUMN_TYPES`、`native_column_type`、`format_column_type`、`column_default` 和线协议元数据；新增对象身份应先进入 `pg_oid`，再让 provider、regclass、定义函数共享同一映射。新增参数类型要同步 `catalog_parameter_type` 以及 `pg_extended.rs` 的文本/二进制参数解码。

测试必须放在独立文件而非 `pg_catalog.rs` 内。首选扩展 `pkg/server/pg_catalog_test.rs` 的对应主题；纯 DataGrip 模板放 `pg_datagrip_test.rs`，Parse/Bind/Execute 和参数放 `pg_extended_test.rs`，解析边界放 `pg_catalog_query_test.rs`，线类型放 `pg_types_test.rs`，真实客户端兼容放 `pg_client_integration_test.rs`。重点风险是 OID 稳定性、客户端依赖的列名/类型/顺序、snapshot 一致性、NULL 语义、资源上限及不受控笛卡尔积。

## 验证依据

- 生产源码：`pkg/server/pg_catalog.rs`；逐段核对了常量、`Execution`、`CatalogQuery` 的解析/绑定/执行链，以及 provider、OID、类型、索引/约束辅助函数。
- crate 与模块边界：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`；`pkg/server/doc.go` 不存在。
- 上游接线：`pkg/server/pg_conn.rs`（简单查询分类、取消与执行）、`pkg/server/pg_extended.rs`（Parse/Bind/Describe/Execute）、`pkg/server/pg_oid.rs`（`catalog_indexes` 调用）。
- 独立测试：`pkg/server/pg_catalog_test.rs` 覆盖投影、描述、关系、谓词、JOIN、CTE、列、约束、函数和依赖；`pg_datagrip_test.rs` 覆盖客户端模板；`pg_extended_test.rs` 覆盖参数和扩展协议；`pg_catalog_query_test.rs` 覆盖解析/拒绝路径；`pg_types_test.rs` 覆盖目录线类型；`pg_client_integration_test.rs` 覆盖客户端交互。
- Go 对照：检索 `pkg/server/*.go` 未发现对应 PG catalog/DataGrip 实现；只将 `Cargo.toml` 声明的 `pkg/server` 包归属和共享 TiDB 模型语义作为对照，不声称存在逐函数 Go 源。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/server/pg_catalog.rs` 成功读取 3,689 行目标源码。精确方法 callers/callees 未返回边，已由直接源码调用点补证。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的命令确认目标文档存在且恰有 11 个固定二级章节，并人工复核唯一新增产物与上述事实链。

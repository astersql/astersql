# `pkg/server/pg_catalog.rs`

## 文件定位

`evaluate`、`project`、`execute_rows` 和 `execute_select` 实现受限的 JOIN/LEFT JOIN、CTE、相关或非相关子查询、UNION、过滤、排序、DISTINCT、LIMIT、数组、`unnest`、`array_agg` 和三值逻辑。

## 核心职责

- `CatalogQuery` 将 `pg_catalog_query` 产生的有限 AST 做绑定、列解析、类型推断和合法性检查；它只接受代码中列出的目录关系、表达式、函数及查询形态。
- `Execution` 为一次执行固定当前数据库、单个 `SchemaRef` snapshot、取消令牌、provider/CTE 缓存和资源计数，保证同一查询的对象身份与元数据来自同一视图。
- `provider_rows` 负责把 `pg_class`、`pg_namespace`、`pg_attribute`、`pg_type`、`pg_index`、`pg_constraint`、`pg_description`、`pg_sequence`、`pg_depend` 等 provider 映射为固定槽位行；可证实不存在的 PG 对象返回有类型的空关系；存在对象却缺少可靠事实时拒绝读取，不虚构对象或伪空。
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

OID 身份由 `pg_oid` 统一生成；`namespace_oid`、`table_oid`、`index_oid` 与 `SYSTEM_RELATIONS` 保证 provider、regclass 和定义查询指向同一对象。`COLUMN_TYPES` 只登记当前可可靠映射的内建类型。FDW、策略等尚无安装能力的 provider 保留列类型并返回空行；触发器读取须先核实当前库无FK，否则因缺PG内部trigger/function身份返回0A000。rewrite、PG collation/operator family等其他缺口沿各自任务处理。

## 依赖与调用关系

上游直接调用关系以源码为准：`pg_conn.rs` 调用 `CatalogQuery::parse_session`/`execute`；`pg_extended.rs` 调用 `parse_session_with_types`、`parameter_oids`、`metadata`、`bind_values`；`pg_oid.rs` 调用 `catalog_indexes` 来解析/枚举索引身份；测试通过 `DATABASES_SQL`、`TRANSACTIONS_SQL`、`namespace_oid` 和 `CatalogQuery` 检验协议行为。

下游主要依赖为：`pg_catalog_query::{Select, Expr, CastType}` 提供受限 AST；`conn::{TiDBContext, CancellationToken, Value, QueryResult, PreparedMetadata}` 提供执行边界；`astersql-infoschema::InfoSchema/SchemaRef` 和 `astersql-meta-model` 提供 schema、表、列、索引、外键、序列与视图模型；`pg_oid` 提供稳定 PG OID；`pg_name::stored_view_select` 提取持久化视图定义；`pg_result::CatalogColumnType` 提供 PG 特有数组/vector/char/ACL 类型码。

`pkg/server/Cargo.toml` 表明这些都是 `astersql-server` 的 crate 内模块或 workspace path 依赖，尤其直接声明了 `astersql-infoschema`、`astersql-meta-model`、`astersql-parser-ast`、`astersql-parser-mysql`。RustCodeGraph 的目标文件节点存在并显示全文件 3689 行；对 `parse_session_with_types`、`CatalogQuery::execute`、`catalog_indexes` 的精确 callers/callees 查询未产出方法边，因此调用方向由上述直接调用点复核，不把图缺失当作“无调用者”。

## 错误处理与边界

解析/绑定错误使用 `ParseResult` 的 PostgreSQL SQLSTATE：例如未确定参数类型 `42P18`、参数协议错误 `08P01`、语法/UNION 列数 `42601`、重复别名 `42712`、歧义列 `42702`、类型不匹配 `42804`，大量明确不支持的目录形态使用 `0A000`。运行时错误转为 `ConnError::Session` 或 `UnsupportedCommand(0)`，由 `pg_conn::sqlstate` 映射到协议错误。

关键硬边界包括：总物化行不超过 16,384；比较/连接工作不超过 100,000；CTE 输出不超过 24 列；每次循环和关键阶段检查取消；算术使用 checked 运算；OID、时间戳、事务 ID、列位置和类型 modifier 均检查范围。缺失 snapshot、损坏/不完整模型、非法数组、无穷数值、零日期、无法忠实翻译的 generated/default/check 表达式都会显式失败，而不是静默伪造结果。

该实现不是完整 PostgreSQL：`age(transactionid)` 只用于原生 TSO 的“越小越旧”排序，不能投影为 PG XID 年龄；`array_agg` 目前只接受 bigint；通配符仅支持已知 CTE 列；表空间等无原生来源的关系为空；`pg_get_userbyid(oid)` 使用共享持久角色身份反查，NULL 参数传播，未知 OID 返回 `unknown (OID=<十进制OID>)`（name/19）；未知对象定义和不支持的索引形态不会猜测。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。`Execution` 借用 `TiDBContext` 和 `CancellationToken`，内部使用 `RefCell`/`Cell`，明确是单线程、单次同步执行状态；它的生命周期止于一次 `CatalogQuery::execute`。`Arc` 仅用于同次执行中廉价复用 provider/CTE 行，并不表示跨线程共享缓存。

取消令牌来自 `pg_conn` 的 `catalog_cancel(pid)`，在入口、provider 扫描、连接、投影和元数据循环中反复检查。目录 prepared statement 只保存在 `pg_extended::Statement/Portal` 中，`metadata.statement_id == 0`，因此关闭或替换它时不应调用原生后端 statement 关闭逻辑；`pg_extended.rs` 正是以 `catalog.is_none() && session_query.is_none()` 区分原生句柄。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 将 crate 对应到 Go 包 `pkg/server`，但在该 Go 目录中检索不到 `pg_catalog`、DataGrip 或 PostgreSQL 目录适配实现，也不存在 `pkg/server/pg_catalog.go`。因此本文件不是同路径 Go 文件的机械翻译，而是 Rust 服务器新增的 PostgreSQL 3.2 协议兼容层；Go `pkg/server` 仍主要是 MySQL 协议服务端实现。

可对齐的是底层数据语义而非文件结构：Rust 从与 Go/TiDB 共用的 infoschema/model 概念读取 public schema/table/column/index/foreign key/sequence，并坚持不把 MySQL/TiDB 特性伪装成不存在的 PG 对象。

## 扩展指南

新增 provider 时，应同时修改 `validate` 的允许关系、`column`/`column_catalog_field` 的列槽和类型、`provider_rows` 的真实数据来源，并在 `CATALOG_ROW_WIDTH` 内保持所有 JOIN 行宽一致；若需要更多槽，必须审计全部 provider、LEFT JOIN NULL 扩展、测试中的 row width 以及 `pg_proc.proacl`。新增表达式/函数要同步更新 `expr_type` 与 `evaluate`，二者必须保持“可绑定即能执行”，并补 `visit_expr`、聚合/相关子查询限制和 NULL 三值语义。

新增原生类型映射应同步 `COLUMN_TYPES`、`native_column_type`、`format_column_type`、`column_default` 和线协议元数据；新增对象身份应先进入 `pg_oid`，再让 provider、regclass、定义函数共享同一映射。新增参数类型要同步 `catalog_parameter_type` 以及 `pg_extended.rs` 的文本/二进制参数解码。

测试必须放在独立文件而非 `pg_catalog.rs` 内。首选扩展 `pkg/server/pg_catalog_test.rs` 的对应主题；纯 DataGrip 模板放 `pg_datagrip_test.rs`，Parse/Bind/Execute 和参数放 `pg_extended_test.rs`，解析边界放 `pg_catalog_query_test.rs`，线类型放 `pg_types_test.rs`，真实客户端兼容放 `pg_client_integration_test.rs`。重点风险是 OID 稳定性、客户端依赖的列名/类型/顺序、snapshot 一致性、NULL 语义、资源上限及不受控笛卡尔积。

## 整数参数元数据接线

目录 `metadata()` 的 `parameter_cast_oids` 为空；目录参数类型仍由本模块既有目录分析决定，不进入共享 engine 的直接整数约束折叠。

## 2026-10-26 文本族身份边界（任务21）

既有目录/会话类型的 NativeType 初始化明确设置 postgresql_text_oid=None，保持目录专用 wire 类型的原解释；仅为共享元数据新增字段所必需的局部接线。

## 索引身份与句柄主键（2026-11-05）

catalog_indexes 校验表内索引ID唯一、多主索引和整数句柄的唯一主键列；缺显式Primary时才投影局部ID -1。已保存的PG主键声明名用于关系命名，pg_class、pg_index及regclass共用该身份。PG表的独立唯一索引没有PostgreSQLConstraintName时不产生唯一约束行；原生遗留约束语义留待约束来源专项。IDX-BASE字段及conrelid补齐PG目录来源属性号，零行Describe也保持来源。

索引辅助身份仅登记已证明的有符号整数 btree 等价类 int2_ops/int4_ops/int8_ops（1979/1978/3124，方法403）。indclass 与有序规范整数键关联，整数 indcollation 为0；未知等价来源在相关投影前拒绝。indexprs/indpred 使用 pg_node_tree/194，普通无表达式/条件才为NULL。状态/EXTRA 依赖 PostgreSQLBuildComplete 与 State/BackfillState；过渡模型拒绝。EXCLUDE/延迟/CLUSTER/并发构建/复制身份不开放时遵循入口拒绝策略。

## 2026-11-06 成员目录与角色权限

`pg_auth_members`读取本次Execution缓存的`PgRoleFact.memberships`，其存在性来自共享native role_graph，OID/grantor/ADMIN/INHERIT/SET属性来自按稳定角色OID关联的持久声明。边OID使用已有NextGlobalID分配；重新GRANT更新选项保留OID，REVOKE后重新授予分配新OID。旧native边缺属性时拒绝成员目录与依赖未知选项的查询，不导出占位布尔值。

`pg_has_role`支持省略调用者和显式name/oid调用者、text/oid目标，以及MEMBER/USAGE/SET、六种WITH ADMIN/GRANT OPTION后缀和逗号任一匹配。MEMBER遍历真实存在性，USAGE/SET每一跳分别要求INHERIT/SET；ADMIN沿成员闭包检查最后一跳ADMIN。NULL严格传播，不存在的名字42704，不存在数值OID返回false（不是其他has_*函数的通用NULL规则），非法权限22023。目录提供器的RowDescription来源OID/属性号保持0，oid/26与bool/16使用既有codec。

ROLE-MEMBER的真实服务验证为独立`pg_catalog_test::cat14_10`。本功能是有界目录消费者；普通SQL全局函数注册、完整角色DDL/委派授权、任意客户端UI流程仍有独立范围。

## 角色 OID 反查（任务11）

`Execution::role_name` 消费单次执行缓存的 `TiDBContext::pg_role_facts`，与 `pg_roles.oid/rolname` 使用同一稳定身份来源，不依赖数据库列表、owner 或调用者角色。`database_role_name` 仍服务于既有 owner 存在性检查，未知 owner 不由函数 fallback 掩盖。`pg_catalog_query::identity_scalar` 接入无 FROM 的普通/pg_catalog 限定调用及 OID 参数；带原生 FROM 的语句仍保留共享执行器归属。

独立测试 `pg_catalog_test::cat14_11` 覆盖真实非root双连接、特殊名称、NULL、0/最大OID、删除/重建、文本/二进制参数及name结果描述、事务错误与恢复。未知值文字由固定PG18.4/18.6参考构建采证；对象owner写入与完整普通SQL函数工厂仍遵循其主责范围。

## 数据库 owner 按名关联（目录任务12）

`owner_object_name_filter` 只提取基表 `pg_database.datname` 与文本/NULL字面量的等值条件（允许AND中的合取项与交换操作数），`execute_rows` 在JOIN读取owner前对基表行应用它。完整WHERE仍在project执行，缓存provider不裁剪；OR、其他关系列和owner条件不提前求值。这样有持久owner的指定数据库可JOIN公开角色目录，不受无关legacy数据库缺owner影响。目标库缺owner仍0A000，外连接补NULL仍是SQL NULL。独立`pg_catalog_test::cat14_12`覆盖真实非root双连接、默认创建owner、两次ALTER OWNER、OID不变、同prepared新Bind、text/binary描述、缺源拒绝与恢复；未增加owner持久化或权限系统。

## 命名空间 owner 按名关联（目录任务13）

同一基表name等值合取项过滤也应用于`pg_namespace.nspname`；限定目标schema的角色JOIN不读取无关legacy schema的缺失owner。只对基表slot1的文本/NULL等值下推，OR不下推，provider缓存和最终WHERE完整保留。`nspowner`仍读取已有`PgNamespaces.owner_id`，缺源或身份不匹配仍0A000；不分配、推断或迁移owner。独立`pg_catalog_test::cat14_13`覆盖真实reader双连接、创建与管理员转移、持久OID、角色反查/JOIN、权限变化、text/binary新Bind、缺源和恢复。

内置public/pg_catalog/information_schema的历史root标签也不能替代持久owner_id；共享`PgNamespaceFact.owner`不再按标签推断OID，缺源nspowner投影0A000。已有管理员ALTER SCHEMA OWNER可显式补齐；默认namespace记录回收须保留任何显式owner_id，防止ALTER OWNER TO root及删除最后一个用户schema后丢失事实。共享独立测试`runtime::pg_schema_test::cat14_13_builtin_owner_requires_persistence_and_survives_cleanup`覆盖这一来源和回收边界，wire测试同时验证reader迁移前拒绝、迁移后真实root来源。

## 关系 owner 按名关联（目录任务14）

`pg_class`基表也在JOIN前应用已绑定的`relname`文本/NULL等值合取项过滤，限定关系与`pg_roles`关联时不会读取无关legacy对象的缺源owner。完整provider缓存和最终WHERE保留，OR不下推，未匹配LEFT JOIN保留SQL NULL。表/视图/序列读取`TableInfo.PostgreSQLOwner`，索引读取所属表的同一持久owner；没有来源或角色不存在仍对所需投影0A000，不用调用者、root、0或NULL替代。

`pg_catalog_test::cat14_14`经真实Domain/KV及两个非root reader验证四类关系、新建者事实、管理员两次ALTER TABLE OWNER、reload后OID不变、角色JOIN/反查、GRANT/REVOKE、同prepared新Bind的text/binary及NULL/不存在名称、legacy拒绝和显式迁移、Sync/ROLLBACK恢复。该局部接线不实现新的owner存储、非管理员自转移或完整DDL语法。


## 类型 owner（目录任务15）

pg_type.typowner使用oid/26、来源1247/4；四个已注册information_schema域从PgStandardSchema.type_owners按持久类型ID读取显式角色OID，内置类型使用独立builtin_type_owners映射以避免静态OID与域的内部ID碰撞，并与当前公开角色事实校验。缺失旧字段或角色来源时，只对所需owner读取返回0A000；oid/typname仍可读。未匹配外连接保持SQL NULL。类型JOIN沿已有基表名称等值合取项过滤，不提前读取其他缺源类型，OR不下推。标准视图owner不作为类型owner来源。

新标准域初始化在同一KV事务记录bootstrap初始化角色OID；历史记录不读时补齐。管理员ALTER TYPE/DOMAIN OWNER只更改已注册标准域的显式owner，同一类型ID保持不变。显式更新按当前数据库持久ID保存覆盖值，其他数据库不共享可变owner；初始逐类型事实保持独立，不改变标准视图owner。CREATE TYPE/DOMAIN仍由既有入口门禁。本任务新增真实reader双连接及text/binary新Bind、历史缺字段和显式迁移回归。新初始化按已批准策略将bootstrap稳定角色OID和现有内置类型声明在同一KV事务持久记录；旧记录启动只登记既有类型身份，不补owner。管理员ALTER TYPE pg_catalog.int4 OWNER同步更新数组owner，直接修改数组或ALTER DOMAIN非域为42809，更新按数据库ID隔离。


## 2026-11-06 数据库授权事实与查询

pg_database.datacl按需消费共享PgDatabaseFact::acl_text，默认ACL为NULL、显式空ACL为{}、缺源为0A000；aclitem[]描述1034/-1/-1，来源1262/18。has_database_privilege与普通函数复用pg_has_database_privilege，不从目录存在性推断权限；Execution缓存只限一次执行。name显式转换保留类型19并按UTF8边界裁剪63字节。

聚焦行为回归：`pg_catalog_test::cat14_17`。


## 函数 owner（目录任务16）

pg_proc.proowner按oid/26、来源1255/4投影；三条原语的name/OID与共享PG_INTROSPECTION_FUNCTION_IDENTITIES一致。目录从既有PgStandardSchema的function_owners读取逐函数初始owner，再应用逐数据库function_owner_overrides的显式事实。真实角色OID缺失时仅所需读取返回0A000；oid/proname仍可读。name等值合取项在角色JOIN前使用已有过滤，OR不下推，未匹配外连接保持SQL NULL。不从调用者、schema/type/view owner或root标签推断函数owner。

真正新建共享标准记录时，在同一KV事务逐函数登记bootstrap管理角色的稳定OID；旧记录缺字段不自动回填，管理员ALTER FUNCTION ... OWNER可显式迁移或转移。新执行/新Bind和另一连接消费持久事实，三原语OID不变，其他数据库保持各自owner。独立cat14_16覆盖三条初始持久事实、真实reader、数据库隔离、历史字段缺失与显式恢复、text/binary和事务恢复；共享pg_schema_test::cat14_16_historical_function_owner_is_not_backfilled直接核验真实KV写入、旧记录重初始化不回填及显式迁移持久性。不开放完整用户函数DDL或函数ACL。

## 命名空间授权（目录任务18）

目录has_schema_privilege支持省略用户、name/text/OID用户及text/OID对象，严格NULL、未知对象OID NULL、缺名称3F000、缺角色42704、完整逗号列表和22023校验。消费pg_namespace_facts的同一持久ACL/成员来源，nspacl既有1034与2615/4属性不变；不按root或展示常量构造授权答案。cat14_18通过真实Domain和非owner wire连接验证。


## 表授权（目录任务19）

relacl绑定为aclitem[]/1034、来源1259/32；按已筛选关系OID惰性取当前Execute模型与角色事实，真实默认NULL与显式{}区分，未知来源0A000，外连接空侧仍NULL。has_table_privilege省略用户及name/OID用户、text/OID对象沿共享表ACL判定，普通FROM与目录入口不构造静态权限答案。

表名重载显式schema解析检查调用者的真实namespace USAGE，缺schema为3F000、无USAGE为42501；OID重载仍只查询目标表授权，与PG18 convert_table_name/LookupExplicitNamespace及双参考端采样一致。该namespace门禁不隐藏公开relacl目录行。

Execution惰性缓存本次schema快照对应的表模型和namespace事实，relacl及权限函数复用同一个缓存；避免逐ACL单元重复深拷贝整个模型集合或重复扫描KV。缓存不跨Execute/portal，新查询仍可看到已提交GRANT/REVOKE。角色事实继续用现有单次执行缓存；跨来源并发同代际一致性仍归后续快照专项。

## 2026-11-06 列授权事实与查询（任务20）

attacl在实际选中pg_attribute行惰性读取本执行快照的TableInfo列ACL与角色事实；aclitem[] OID1034、来源1249/22、typlen=-1、typmod=-1。默认NULL、显式空数组、缺源0A000保持区别。新增两列权限函数共用正式typed签名和共享求值，int2属性号通过受检Smallint cast，保持NULL及SQLSTATE。

任务21：has_sequence_privilege的用户省略/name/oid、对象text/oid重载沿单次Execution缓存的共享角色、namespace和TableInfo ACL事实；严格NULL、缺OID NULL、缺名称42P01、非法权限22023、错误类别42809。relacl序列文本使用真实rwU与grantor/grantee，默认NULL与显式{}不同。cat14_21同时覆盖正式表达式FROM入口及Q/扩展协议，未按客户端构造授权值。

## 类型授权（任务22）

任务22：pg_type.typacl接持久类型ACL，描述为1034/来源1247/32，缺源在取值前0A000；has_type_privilege与共享层使用同一真实USAGE/角色继承/委派授权。普通SQL函数正式工厂和目录标量使用一致接口，NULL与不存在OID保留SQL NULL。

## 任务23：函数授权

proacl 按实际投影读取共享 pg_function_facts，并以 aclitem[]/1034、来源1255/30输出；默认 SQL NULL 与显式空数组分开。has_function_privilege 使用同源角色/函数快照，支持省略用户及 name/text/OID 组合与完整 oid 输入签名。三条内省原语执行前检查调用者的真实 EXECUTE；目录读取仍可在无 EXECUTE 时读取。扩展新 Bind 重新取得事实，非NULL ACL binary 沿 PG18 的42883边界。

## 任务24：关系可见性与有效路径

pg_table_is_visible(oid)使用同次Execution的有界pg_class provider缓存及有效显式路径。缺pg_catalog时先隐式搜索它，已显式列出时保留次序；按真实namespace USAGE与同名首个关系决定可见性，独立于表SELECT，不因path过滤显式目录行。NULL及未知OID返回NULL/bool16，错误签名42883；保留行/工作预算和取消检查。

任务25补充：pg_type_is_visible(oid)共用上述可见性求值，切换为同次Execution的真实pg_type provider。内建和持久information_schema域沿原OID/name/namespace身份，按有效路径同名首项遮蔽；NULL/未知OID返回NULL，结果bool16。可见性独立于类型USAGE；尚无用户类型来源，CREATE TYPE/DOMAIN保持输出前0A000门禁，不造夹具。

任务26局部实现：pg_function_is_visible(oid)沿同次Execution的真实pg_proc provider和有效路径，按proname及proargtypes输入OID向量匹配首项，跳过保留pg_temp/pg_temp_* namespace。结果bool16，NULL/未知OID返回NULL，与目标函数EXECUTE授权独立；仍受原有取消、行数和工作预算约束。当前仅三条真实内省原语有登记；用户函数DDL/跨schema重载的本端验收未开放，不能用门禁结果代替遮蔽验收。

## 2026-11-07 目录任务26：真实函数签名可见性

pg_proc由原三原语及PgStandardSchema/本连接临时声明枚举真实登记对象，投影OID/namespace/输入向量/结果/owner/implementation；可见性与普通调用同源，登记函数自身EXECUTE也走原ACL；继续保留行数和工作预算。

## 名称与 OID 往返（cat14_27）

regclass 强制转换区分输入函数和已声明 text 的名称转换：未知字符串字面量调用 `pg_oid::resolve_in_path`，包括数字 OID 与 `-`；text 参数或表达式调用 `resolve_name_in_path`，数字也必须作为关系名查找。两者继续共享同次 Execution 快照、有效路径和 namespace USAGE。regclass 到 text 使用同一身份的 `display_in_path`，必要时限定 schema 并正确引用名称。


## 2026-11-07 表空间空关系策略（任务28）

pg_tablespace保持真实零行，五列为oid/name/oid/aclitem[]/text[]，保留1213的真实PG属性号与text/binary描述；未知列42703。每次Execute复用共享角色图和按库持久的虚拟关系ACL检查SELECT，拒绝42501优先于空行/LIMIT；不把无权限伪装为空集。pg_class.reltablespace使用0表示原生存储没有显式PG空间分配，LEFT JOIN保左行；没有捏造pg_default/pg_global对象。独立回归为pg_catalog_test::cat14_28。


## 2026-11-07 任务29外部对象空关系

三类FDW目录按PG18完整7/8/3列绑定，保留2328/1417/3118来源属性与oid/name/text/aclitem[]/text[]类型；未知字段42703，合法未实现系统属性0A000。无对象是因安装入口拒绝，不是缺provider；普通表不投影为外表。每次Execute复用pg_empty_catalog_select检查真实有效角色与当前库目录SELECT；LEFT JOIN空右侧仍保留左行与带类型NULL。独立回归cat14_29覆盖Q、扩展Describe/二进制、撤权重执行、恢复与秘密不泄漏。


## 2026-11-07 任务30：行安全策略门禁

pg_policy的8个已知字段绑定到26/19/26/16/1028/18/194/194，字段来源3256，物理属性序1/2/3/5/6/4/7/8。策略表达式保留pg_node_tree，包括空关系和LEFT JOIN产生的NULL；合法但未实现系统属性拒绝0A000、未知字段42703。当前共享模型/原生入口不能安装PG RLS或policy，SessionQuery补齐所有策略/RLS状态变更前置拒绝，所以本目录可返回已知空集；不能把一般PG的零policy行推导成RLS关闭。每次Execute仍复用真实有效角色和pg_empty_catalog_select授权。

聚焦回归：`pg_catalog_test::cat14_30`及`pg_session_test::cat14_30_policy_parser`。


## 2026-11-07 目录任务31：触发器必需事实门禁

`pg_trigger`提供oid/tgrelid/tgname/tgfoid/tgisinternal五列描述，类型26/26/19/26/16，来源OID2620，物理属性号1/2/4/5/8。未知用户列42703，尚未支持的系统属性0A000；空集和LEFT JOIN NULL仍保留这些描述。

provider复用单次Execution的当前数据库schema快照与连接临时模型。无FK且未开放用户触发器安装能力时返回有类型的空集；任一模型有ForeignKeys则输出前0A000，因为现有FK执行没有PG内部trigger/function身份，不能以空集宣称完整FK内省。provider在过滤与JOIN前物化，故当前保守拒绝该库内所有pg_trigger读取（包括无关表过滤、父表、LIMIT0），不从SQL条件猜测不存在对象。每次Execute重新取事实，drop最后FK后恢复空集，另一数据库不受影响。未新增触发器执行器、对象或函数身份。

`SessionQuery`的最小TRIGGER DDL拒绝门禁维护用户触发器不可安装边界；未来引入共享触发器事实时须一起更新provider。聚焦真实服务回归`pg_catalog_test::cat14_31`，含非root owner/reader、字段描述、text/binary、LEFT JOIN、FK前后prepared重执行、I/E恢复及跨库隔离。

# PostgreSQL 协议第一阶段

Rust server 提供独立 PostgreSQL TCP listener。支持请求前后端协议 **3.0 或 3.2** 的客户端；保留编号 3.1 被明确拒绝，SQL 由现有 AsterSQL 执行引擎处理；协议支持不代表 PostgreSQL SQL 语义兼容。

## 启用与客户端

默认不启动 PG listener。Rust tidb-server 使用 `--postgres-port=<port>` 或 TOML 顶层 `postgres-port = <port>` 显式启用；命令行优先。PG 使用现有 host 配置，与 MySQL 使用不同端口；端口 0 分配临时端口，适合测试。关闭 server 时关闭两个 listener 及 PG 会话；PG 端口占用导致启动失败并清理 listener。

真实客户端回归使用系统 PostgreSQL libpq 18，经 Python 3 ctypes 调用；没有新增 Cargo 客户端依赖。测试不指定协议上下限时校验 PQfullProtocolVersion 为 30000；显式设置 `min_protocol_version=3.2 max_protocol_version=3.2` 时为 30002。启动报文版本分别为 196608/196610。默认 psql 可连接：

```bash
psql "host=127.0.0.1 port=<postgres-port> user=root dbname=test sslmode=disable gssencmode=disable"
```

握手的 `server_version` 为 `18.0 (AsterSQL)`，libpq 的 PQserverVersion 为 180000。这是带产品标识的 PG18 协议兼容基线，不代表完整 PostgreSQL 18 功能或 SQL 语义。旧版本 libpq/psql 未逐版本验收。

当前鉴权仅支持 canonical driver 的 `InsecureRootOnly` 开发模式：driver 验证 root 空 native credentials 成功后才返回 AuthenticationOk。其他身份与 `SecureUnsupported` 模式被拒绝。不提供生产密码鉴权、SCRAM、TLS 或 GSS 加密；SSL/GSS 协商返回 N，要求 secure transport 时拒绝连接。不要将此入口视为生产鉴权方案。

startup 接受 user、database、application_name、UTF8/UTF-8 client_encoding、ISO/ISO, MDY DateStyle 和正值 1/2/3 extra_float_digits。DateStyle 报告为 ISO, MDY；浮点文本使用现有最短可往返编码，非正值舍入模式不支持。TimeZone 在鉴权后设置到 canonical 会话 time_zone，并通过 ParameterStatus 回报；缺省为 UTC，无效时区返回 22023，其他不支持参数返回 0A000。database 在鉴权成功后通过既有会话接口选择。libpq 连接配置应显式设置 `sslmode=disable gssencmode=disable` 并指定存在的数据库。

## 已验证工作流

真实 TCP/libpq 默认 3.0 与显式 3.2 回归执行 SELECT、CREATE TABLE、INSERT、UPDATE、DELETE、DROP TABLE、显式 int4 OID 的 `$1` 参数及 BEGIN/COMMIT/ROLLBACK。同一 Server 上的 MySQL 连接在 PG 工作流前完成鉴权，之后 COM_PING 仍成功。

简单 Query 只接受一条现有引擎可执行的语句；空查询返回 EmptyQueryResponse，多语句返回 0A000。允许的 AST 命令还包括集合查询、CREATE/DROP DATABASE、ALTER/TRUNCATE TABLE、DROP VIEW、SET；这些命令的全部 SQL 变体没有逐一验收。REPLACE 与其他不支持命令被拒绝。DataGrip 的 `select round(extract(epoch from pg_postmaster_start_time() at time zone 'UTC')) as startup_time` 探测在 PG 适配层按 SQL token 识别，以 PG listener 本次启动时记录的微秒时间计算并四舍五入到 epoch 秒；同一 PG 服务的所有连接和预处理查询共用该值，结果 OID 为 numeric（1700），保留别名。此支持仅覆盖该 UTC 启动时间探测，不代表通用 EXTRACT、AT TIME ZONE 或 PostgreSQL 时间函数兼容。PG 适配层按 AST 投影和源码位置将未引用、未限定的直接 `current_catalog` 投影映射到 canonical 会话的数据库名，保留默认结果列名与显式别名；普通与扩展查询共用该适配。字符串、引用列名、限定列名不改写；完整 PostgreSQL 表达式及 pg_catalog 仿真不作兼容承诺。

扩展查询支持 Parse、Bind、Describe、Execute、Close、Sync、Flush，具备命名 statement/portal、重复和乱序 `$n` 参数映射、分段返回 PortalSuspended、错误后丢弃消息直到 Sync。参数传给既有预处理接口，不通过字符串拼接值。原生执行查询的参数必须提供明确类型 OID；目录查询可从显式 `$n::oid` 等转换取得参数类型，无转换时仍需客户端提供 OID。没有通用参数类型推断，参数化投影的结果元数据缺失或 prepare/execute 元数据不一致时明确报错。目录查询的二进制参数支持 OID、int2/int4/int8、boolean 和 text/varchar/bpchar；Bind 按零个、一个或逐参数格式代码解码，OID 保留完整无符号范围，非法定长值返回 22P03。原生执行查询的二进制参数与全部二进制结果格式仍被拒绝。JDBC 连接需设置 `binaryTransfer=false`，避免驱动在达到 prepareThreshold 后切换为二进制结果。美元引用、引擎可执行注释/提示注释及 `?` 参数标记不支持。

3.0 BackendKeyData 使用 4 字节随机取消密钥，3.2 使用 32 字节；CancelRequest 必须匹配当前后端和密钥。真实 libpq 验证 idle cancel 不影响下一条查询；相邻 TCP 测试验证正在执行的命令取消、错误密钥及旧/空闲取消不会污染下一命令。事务状态来自共享会话的 in_transaction；只报告 I/T，不承诺 PostgreSQL 出错事务的 E 状态及其后续语义。

## DataGrip 内省目录探测

PG 独立目录查询结构支持 `pg_database`/`pg_shdescription` 数据库列表、`pg_namespace`/`pg_description` namespace 列表、`pg_tablespace` 基础投影，以及 `pg_locks` 最老事务探测。支持这些查询所需的投影、别名、bigint/varchar 转换、受限 LEFT JOIN、WHERE、CASE 排序和 LIMIT；未知目录、不支持结构与语法错误明确报错。此范围不代表完整 PostgreSQL 语法或 pg_catalog 兼容；批量 SQL 不支持。

数据库名来自当前 canonical InfoSchema；目录中的数据库、namespace、表与索引身份使用带范围检查的 PG 32 位 OID，并由持久原生 ID 推导，目录引用之间一致。当前数据库排在首位。AsterSQL 数据库不是 PostgreSQL template，root 开发鉴权下允许连接；没有原生 PG owner 或 shared description 来源时返回 SQL NULL。OID 列报告 OID 类型（26），名称、布尔与可空列按实际目录定义报告。

事务探测读取当前 Domain 的真实 `information_schema.tidb_trx`，返回最小的原生 TSO start timestamp。没有活动事务时返回零行；事务结束后移除。这里的 transaction_id 是原生 64 位事务标识，不是 PG wraparound XID，也不代表完整 PostgreSQL 锁模式、持锁表或 `age(xid)` 语义。

PG startup 的 database 选择同名原生库；该连接的 `public` 虚拟映射当前库对象，不创建原生 public 库。`pg_namespace` 提供 `public` 与 `pg_catalog`，当前 schema 默认 public，连接私有 `SHOW/SET/RESET search_path` 支持 public 与 pg_catalog（含空路径）。未引用标识符按 PG 规则折叠，引用名称保留大小写；public 限定的表名映射当前库，跨库和非 public 用户 schema 明确拒绝。未显式列出的 pg_catalog 在搜索路径前隐式搜索；显式 public,pg_catalog 顺序允许当前库同名表优先。默认 MySQL 的数据库限定、USE、SQL mode 与 prepared statement 行为保持原有语义。

没有原生 PG XID、owner 或注释来源时，xmin/state_number、owner 和 description 为 SQL NULL，不用原生 TSO 伪造 xmin。没有 PG tablespace 来源，pg_tablespace 返回类型正确的零行；基础 spcacl/spcoptions 是可空 text，完整 tablespace ACL/数组模板仍明确拒绝。

真实元数据投影覆盖当前库关系、列、类型、默认值、索引及约束目录：pg_class、pg_attribute、pg_type、pg_attrdef、pg_index、pg_constraint。支持来源查询所需的 INNER/LEFT JOIN、SQL 三值谓词、CASE、排序/限制、非递归只读 CTE、受限子查询和目录函数；结构深度、行数及取消仍受目录预算约束。一次 Execute 共用一个元数据快照，Parse 不冻结行，后续 Execute 可见 DDL 更新。目录 regclass 名称解析复用同一对象 OID，不按结果顺序分配身份。

视图源从真实 model 元数据读取并提取 SELECT，pg_get_viewdef 支持来源模板及 NULL/未知 OID 边界。pg_proc/pg_language 仅注册有界内省 primitive；当前原生引擎不支持用户存储程序，原生 ROUTINES 与来源函数查询均返回真实类型正确的空集合，不能据此宣称完整 PG 函数系统。pg_sequence 投影真实序列对象；pg_depend 表示真实序列到当前 schema 的 n 依赖。原生元数据没有 owned-by 表/列或稳定默认值序列引用，因此原始拥有依赖查询为零行，不把 AUTO_INCREMENT 伪造为序列或猜测 a/i 关系。完整 RetrieveSequences10 描述模板尚未验收。

普通 Query 和扩展 Parse/Bind/Describe/Execute 都支持这些基础探测。按 [PG18 Close 规范](https://www.postgresql.org/docs/18/protocol-flow.html)，关闭不存在的 statement/portal 也返回 CloseComplete；这使 JDBC 在 Parse 报错后清理失败对象时不再阻断下一条查询。PG 目录 statement/portal 生命周期按 PG statement 名字管理，不分配或关闭 engine prepared handle；在 Execute 时重新读取元数据，继续使用统一 portal 分页和 Sync 错误恢复。共享会话接口仅新增协议无关的只读 schema snapshot，没有加入 PG SQL 解析或目录行为。

本机 DataGrip JDBC 42.7.13、42.7.3 对两条原始 SQL 的普通及 PreparedStatement 查询通过，并验证实际新建/删除数据库、boolean/NULL 类型、活动事务及回滚后的变化。真实 TCP 测试另外验证 Parse 后新建库可见、两个目录 statement 的 Close 相互隔离、最老事务跨连接排序。未验证完整 DataGrip 元数据树或 RealTiKV 分布式锁行为。

## 类型与错误边界

结果 OID 由引擎原始类型和完整标志得到，不凭列名或数据值猜测。支持整数（unsigned 按范围扩大）、numeric、float4/float8、text、bytea、布尔标志、date、time、无时区 timestamp 及 NULL。BOOL 表定义仍遵循现有引擎 TINYINT 语义；布尔表达式保留 bool 标志。DATETIME/TIMESTAMP 都映射无时区 timestamp，不推测绝对时区。结果只使用文本格式，表 OID/属性编号未知为 0，typmod 为 -1。

参数支持 OID 16、17、20、21、23、25、700、701、1042、1043、1082、1083、1114、1700 的文本值及 NULL；bytea 仅接受十六进制形式。日期时间参数使用明确格式，时间精度最多微秒。不支持类型、缺失元数据、零日期、负数或超出一天的 TIME duration、文本 NUL 等明确返回错误。原生 JSON、enum/set、数组、带时区类型及全部二进制格式不作兼容承诺。目录结果另支持 OID/regclass、内部 char、int2vector 和 int2/int4/OID/text 数组的文本编码；目录参数支持 OID 26 的十进制文本、四字节网络字节序二进制值及 NULL，检查 0..u32::MAX 范围，不代表原生 SQL 数组参数支持。

PG 边界映射语法错误 42601、已知唯一键错误 23505、未知列 42703、未知表 42P01、未知库 3D000、取消 57014，并对不支持功能使用 0A000、错误报文使用 08P01。部分执行错误仍是共享接口字符串，仅匹配已知引擎错误形式；未知错误保留 XX000，不保证全量 PostgreSQL 错误分类。

## 回归与未验证项

从仓库根目录执行：

```bash
cargo test -p astersql-server --lib pg_introspection_clients -- --nocapture
cargo test -p astersql-server --lib postgres_client_protocol_versions
cargo test -p astersql-server --lib pg_
cargo test -p astersql-server --lib postgres_listener_lifecycle
cargo test -p astersql-server --lib real_listener_serves_handshake_ping_select_and_drains_connection
cargo test -p astersql-server --lib mysql_protocol_connection_commands_match_mysql_80
cargo test -p astersql-server --lib mysql_type_packets_expose_correct_type_flags_charset_and_decimal
cargo test -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values
```

真实客户端测试缺少 Python 3 或 libpq >=18 时明确失败。macOS 默认库路径是 `/opt/homebrew/opt/libpq/lib/libpq.dylib`；其他路径可设置 `PG_LIBPQ_LIBRARY`。

此前 DataGrip PostgreSQL JDBC 在 startup 阶段被 DateStyle 拒绝；现已补齐 ISO DateStyle、TimeZone 和正值 extra_float_digits 启动参数兼容；DataGrip 已安装的 PostgreSQL JDBC 42.7.13 和 42.7.3 驱动均在临时真实 TCP listener 上完成连接和 SELECT 1；另通过 Statement 和 PreparedStatement 验证 SELECT current_catalog 的数据库值与结果列名，以及 DataGrip ServerStartupTime 原始查询的 numeric 类型、别名和跨连接固定的启动时间。默认系统 psql 18 的 SELECT 1 已实测返回 1。DataGrip UI 和元数据浏览未验证。没有验证 RealTiKV、生产鉴权、TLS、COPY、复制、通知、完整 pg_catalog、ORM/JDBC 全组合或完整 PostgreSQL SQL 语法/事务语义。结果和 portal 当前全量物化；增加原始类型向量，必要时进行只读 AST/catalog 元数据解析，没有大结果吞吐或内存基准。

PG 编解码、OID、SQLSTATE、鉴权协商和连接状态仅在 `pkg/server/pg_*.rs`；共享执行模块不引用 PG。现有入口仅接线配置与独立 listener。后续支持能力应继续沿该边界扩展，并同时验证 MySQL 行为。

2026-10-01 客户端统一回归：libpq 18 在临时双 listener 上的 3.0/3.2 测试通过，验证原始数据库/namespace SQL、tablespace 零行列元数据、NULL、排序及错误后的 SELECT 1，保留 MySQL COM_PING。真实 TCP 回归覆盖空结果 Statement/Portal Describe、namespace Parse 后创建可见及缺失 Close 后流水查询。DataGrip 安装的 JDBC 42.7.13/42.7.3 在 `prepareThreshold=1&binaryTransfer=false` 下通过 Statement/PreparedStatement 查询、类型/NULL/临时库行内容和错误恢复；默认二进制结果仍不支持。

本机 DataGrip `database-plugin.jar` 中 `com/intellij/database/dialects/postgres/introspector/PgIntroQueries.sql` 的 tablespace 原始模板已提取。`RetrieveExistentTablespaces` 为 `select oid::bigint from pg_catalog.pg_tablespace`，已验证零行 bigint 结果。`RetrieveTablespaces` 在 PG18、非增量模式下选择的 SQL 为：

```sql
select T.oid::bigint as id, T.spcname as name,
       T.xmin as state_number, pg_catalog.pg_get_userbyid(T.spcowner) as owner,
       pg_catalog.pg_tablespace_location(T.oid) as location,
       T.spcoptions as options, D.description as comment
from pg_catalog.pg_tablespace T
  left join pg_catalog.pg_shdescription D on D.objoid = T.oid
```

该完整查询需要尚不支持的 tablespace xmin/location，当前明确返回 0A000，随后连接仍能查询。原始模板还有按 `pg_catalog.age(T.xmin)` 过滤的增量分支，同样不属于基础范围。上述 SQL 证据来自安装资源及 JDBC 执行，未捕获 DataGrip UI 发出的流量；UI 元数据树和完整 tablespace 内省未验证。


## 2026-10-03 内省来源与验收清单

本节冻结来源，不承诺这些查询已可执行。DataGrip 2025.1.3（build 251.26094.87）的 DatabaseTools `database-plugin.jar` 包含 `com/intellij/database/dialects/postgres/introspector/PgIntroQueries.sql`；模板 SHA-256 为 `d5c90888f1ecd79e6badaf75ab5428998b01090c77067fd89975a49d5fa196d1`。本机安装 PostgreSQL JDBC 42.7.13、42.7.3；错误日志没有记录选用版本，不从安装列表推断实际驱动。

原始用户证据来自“分析 PostgreSQL 兼容问题”聊天（01a0ff3d-745f-7dc0-96fc-bfc103996367）。其首条仅有尾部；DataGrip 缓存 `database-log/database.0.log` 在 2026-10-03 08:45:58、会话 1533977248 中补齐同一 statement 1869279758，确认是 **RetrieveViewSources**。1869279759 是 **RetrieveFunctionSources**，1869279760 是 **RetrieveRelations**。三条日志全文（保留空白、注释和 JDBC 展示的问号）冻结为 `pkg/server/pg_client_integration_test.rs` 的三个 `DATAGRIP_*_SQL` 常量；原始错误分别是关系不存在、only catalog SELECT is supported、regclass 语法错误。

日志 `?::oid` 是 JDBC 展示形式，安装模板中是 `:schema_id::oid`。PG Parse 验收使用 `$1::oid` 和类型 OID 26。libpq 测试从真实当前目录读取 namespace ID，Query 用其整数文本替换唯一问号，PQexecParams 则用 $1 和显式 OID 26。未捕获原 DataGrip 连接的线包或参数值，不宣称此转换证明了原 JDBC 绑定值。

以下完整模板选择 PG18、非增量、非名称片段分支：选择全部 #V 当前版本分支，排除 #INC、#INCSRC、#FRAG 控制的增量或名称过滤片段。保留投影中的 xmin 不表示已经支持；没有 PG XID 不得伪造状态号，age(xmin) 增量内省仍为范围外。三条原始查询另有错误日志证据，其余模板仅冻结后续字段需求，不能声称全部实际执行过。

### 字段及验收边界

完整 SQL 的每一项投影和谓词是逐字段来源清单，以下说明对象语义、边界及重点验证。

| 对象 / 模板 | 必需字段、函数与验收 |
| --- | --- |
| 视图源码 | pg_class.relkind/oid/relnamespace、pg_namespace.oid；输出 view_kind/view_id/source_text；pg_get_viewdef(oid,true)，m/v 过滤，真实视图定义。 |
| 函数源码 | pg_language.oid/lanname、pg_proc.oid/pronamespace/prokind/prolang/prosrc；id/arguments_def/result_def/sqlbody_def/source_text；pg_get_function_arguments/result/sqlbody；WITH、IN/NOT IN 子查询、NOT、IS NOT NULL。只有确认原生函数集合不存在才可返回类型正确空目录。 |
| 序列依赖 | pg_depend.objid/refobjid/refobjsubid/classid/refclassid/deptype；pg_class.oid/relkind/relnamespace；dependent_id/owner_id/owner_subobject_id；两次 INNER JOIN、regclass→oid、<>、OR、排序；真实序列及依赖，不能把自增列伪装为序列。 |
| 表 | RetrieveTables 的 kind/name/id、owner、persistence、分区、访问方法、tablespace 等投影；真实对象 OID，创建/重命名/删除后可见，当前库 public 隔离。无法映射属性须明确报告，不能以空目录隐藏已有表。 |
| 列 / 类型 / 默认值 | pg_attribute.attrelid/attnum/attname/xmin/atttypmod/attndims/atttypid/attnotnull/attislocal/attfdwoptions/attisdropped/attidentity/attgenerated；pg_attrdef.adrelid/adnum/adbin；format_type、pg_get_expr；真实列编号、NULL、删除列、identity/generated、ALTER 后变化。pg_type 字段以 RetrieveDataTypes 完整 SQL 为准，读取完整 ModelMeta，不凭精简结构猜类型。 |
| 索引 | pg_index.indexrelid/indrelid/indnkeyatts/indisunique/indisprimary/indnullsnotdistinct/indkey/indoption/indcollation/indclass/indexprs/indpred；复合键、unique、include 列、表达式、谓词、排序、collation/opclass；pg_get_indexdef、pg_get_expr；索引 OID 与表内局部 ID 区别，数组保留元素类型。 |
| 约束 | pg_constraint.oid/xmin/conname/contype/condeferrable/condeferred/connoinherit/conbin/conkey/conindid/confkey/confrelid/confupdtype/confdeltype/connamespace/conexclop；表/引用表 OID 与列号一致；主键、唯一、检查、外键及删除后的清理。 |
| 函数 / 语言描述 | RetrieveRoutines 与 ListLanguages 的真实签名、参数数组、结果、kind、语言、owner、cost、volatility、security、strict、parallel、handler/inline/validator 与 namespace；全字段见模板投影，无法映射时明确说明。 |
| 序列描述 | PG18 使用 RetrieveSequences10：pg_sequence.seqrelid/seqtypid/seqstart/seqmin/seqmax/seqincrement/seqcache/seqcycle、关系及 owner；实际核查原生序列集合和 OID，与 RetrieveRelations 引用一致。 |

三条日志查询是本轮必须保留的验收入口。辅助模板不能自动扩大为完整 DataGrip 兼容：RetrieveIndexColumns 还要求数组下标、WITH ORDINALITY、unnest、CROSS JOIN、collation/opclass 和访问方法属性；RetrieveRoutines 涉及 NATURAL JOIN、星号投影和更多 CTE/函数，RetrieveConstraints 还使用元组连接、数组子查询和 regoper 转换，RetrieveIndices 使用 ANY 与继承聚合。未获相邻任务明确授权的部分须单列缺口，不能用只支持核心字段宣称整条辅助模板已通过。UI 元数据树、foreign/aggregate/operator/extension/trigger/policy、任意多 schema、增量 xmin 仍为范围外。

### 当前失败证据

libpq >=18 在临时真实双 listener 上以协议 3.0、3.2 复现三个来源夹具，每次错误后 SELECT 1 通过，原 MySQL COM_PING 继续通过。Query SQLSTATE 分别为 42P01、0A000、42601；显式 oid 参数的 Parse 分别为 42P01、0A000、0A000。第三条扩展查询因不支持 OID 26 提前失败，不证明 regclass、JOIN 或 Bind 已执行。后续实现需把相应错误断言升级为真实结果断言，不能长期把预期报错当成兼容验收。

### PG18 非增量完整模板（参数为 $1::oid）

#### RetrieveTables

```sql
select T.relkind as table_kind,
       T.relname as table_name,
       T.oid as table_id,
       T.xmin as table_state_number,
       false as table_with_oids,
       T.reltablespace as tablespace_id,
       T.reloptions as options,
       T.relpersistence as persistence,
       (select pg_catalog.array_agg(inhparent::bigint order by inhseqno)::varchar from pg_catalog.pg_inherits where T.oid = inhrelid) as ancestors,
       (select pg_catalog.array_agg(inhrelid::bigint order by inhrelid)::varchar from pg_catalog.pg_inherits where T.oid = inhparent) as successors,
       T.relispartition as is_partition,
       pg_catalog.pg_get_partkeydef(T.oid) as partition_key,
       pg_catalog.pg_get_expr(T.relpartbound, T.oid) as partition_expression,
       T.relam am_id,
       pg_catalog.pg_get_userbyid(T.relowner) as "owner"
from pg_catalog.pg_class T
where relnamespace = $1::oid
       and relkind in ('r', 'm', 'v', 'f', 'p')
order by table_kind, table_id
;
```

#### RetrieveDataTypes

```sql
select T.oid as type_id,
       T.xmin as type_state_number,
       T.typname as type_name,
       T.typtype as type_sub_kind,
       T.typcategory as type_category,
       T.typrelid as class_id,
       T.typbasetype as base_type_id,
       case when T.typtype in ('c','e') then null
            else pg_catalog.format_type(T.typbasetype, T.typtypmod) end as type_def,
       T.typndims as dimensions_number,
       T.typdefault as default_expression,
       T.typnotnull as mandatory,
       pg_catalog.pg_get_userbyid(T.typowner) as "owner"
from pg_catalog.pg_type T
         left outer join pg_catalog.pg_class C
             on T.typrelid = C.oid
where T.typnamespace = $1::oid
  and (T.typtype in ('d','e') or
       C.relkind = 'c'::"char" or
       (T.typtype = 'b' and (T.typelem = 0 OR T.typcategory <> 'A')) or
       T.typtype = 'p' and not T.typisdefined)
order by 1
;
```

#### RetrieveColumns

```sql
with T as ( select
                  T.oid as table_id, T.relname as table_name
            from pg_catalog.pg_class T
            where T.relnamespace = $1::oid
              and T.relkind in ('r', 'm', 'v', 'f', 'p')
            )
select T.table_id,
       C.attnum as column_position,
       C.attname as column_name,
       C.xmin as column_state_number,
       C.atttypmod as type_mod,
       C.attndims as dimensions_number,
       pg_catalog.format_type(C.atttypid, C.atttypmod) as type_spec,
       C.atttypid as type_id,
       C.attnotnull as mandatory,
       pg_catalog.pg_get_expr(D.adbin, T.table_id) as column_default_expression,
       not C.attislocal as column_is_inherited,
        C.attfdwoptions as options,
       C.attisdropped as column_is_dropped,
       C.attidentity as identity_kind,
       C.attgenerated as generated
from T
  join pg_catalog.pg_attribute C on T.table_id = C.attrelid
  left join pg_catalog.pg_attrdef D on (C.attrelid, C.attnum) = (D.adrelid, D.adnum)
where attnum > 0
order by table_id, attnum
;
```

#### RetrieveIndices

```sql
select tab.oid               table_id,
       tab.relkind           table_kind,
       ind_stor.relname      index_name,
       ind_head.indexrelid   index_id,
       ind_stor.xmin         state_number,
       ind_head.indisunique  is_unique,
       ind_head.indisprimary is_primary,
       ind_head.indnullsnotdistinct nulls_not_distinct,
       pg_catalog.pg_get_expr(ind_head.indpred, ind_head.indrelid) as condition,
       (select pg_catalog.array_agg(inhparent::bigint order by inhseqno)::varchar from pg_catalog.pg_inherits where ind_stor.oid = inhrelid) as ancestors,
       ind_stor.reltablespace tablespace_id,
       opcmethod as access_method_id
from pg_catalog.pg_class tab
         join pg_catalog.pg_index ind_head
              on ind_head.indrelid = tab.oid
         join pg_catalog.pg_class ind_stor
              on tab.relnamespace = ind_stor.relnamespace and ind_stor.oid = ind_head.indexrelid
         left join pg_catalog.pg_opclass on pg_opclass.oid = ANY(indclass)
where tab.relnamespace = $1::oid
        and tab.relkind in ('r', 'm', 'v', 'p')
        and ind_stor.relkind in ('i', 'I')
```

#### RetrieveIndexColumns

```sql
select ind_head.indexrelid index_id,
       k col_idx,
       k <= indnkeyatts in_key,
       ind_head.indkey[k-1] column_position,
       ind_head.indoption[k-1] column_options,
       ind_head.indcollation[k-1] as collation,
       colln.nspname as collation_schema,
       collname as collation_str,
       ind_head.indclass[k-1] as opclass,
       case when opcdefault then null else opcn.nspname end as opclass_schema,
       case when opcdefault then null else opcname end as opclass_str,
       case
           when indexprs is null then null
           when ind_head.indkey[k-1] = 0 then chr(27) || pg_catalog.pg_get_indexdef(ind_head.indexrelid, k::int, true)
           else pg_catalog.pg_get_indexdef(ind_head.indexrelid, k::int, true)
       end as expression,
       amcanorder can_order
from pg_catalog.pg_index ind_head
         join pg_catalog.pg_class ind_stor
              on ind_stor.oid = ind_head.indexrelid
    cross join unnest(ind_head.indkey) with ordinality u(u, k)
         left join pg_catalog.pg_collation
                   on pg_collation.oid = ind_head.indcollation[k-1]
         left join pg_catalog.pg_namespace colln on collnamespace = colln.oid
cross join pg_catalog.pg_indexam_has_property(ind_stor.relam, 'can_order') amcanorder
         left join pg_catalog.pg_opclass
                   on pg_opclass.oid = ind_head.indclass[k-1]
         left join pg_catalog.pg_namespace opcn on opcnamespace = opcn.oid
where ind_stor.relnamespace = $1::oid
  and ind_stor.relkind in ('i', 'I')
order by index_id, k
```

#### RetrieveConstraints

```sql
select T.oid table_id,
       relkind table_kind,
       C.oid::bigint con_id,
       C.xmin::varchar::bigint con_state_id,
       conname con_name,
       contype con_kind,
       conkey con_columns,
       conindid index_id,
       confrelid ref_table_id,
       condeferrable is_deferrable,
       condeferred is_init_deferred,
       confupdtype on_update,
       confdeltype on_delete,
       connoinherit no_inherit,
      pg_catalog.pg_get_expr(conbin, T.oid) con_expression,
       confkey ref_columns,
       conexclop::int[] excl_operators,
       array(select unnest::regoper::varchar from unnest(conexclop)) excl_operators_str
from pg_catalog.pg_constraint C
         join pg_catalog.pg_class T
              on C.conrelid = T.oid
   where relkind in ('r', 'v', 'f', 'p')
     and relnamespace = $1::oid
     and contype in ('p', 'u', 'f', 'c', 'x')
     and connamespace = $1::oid
;
```

#### ListLanguages

```sql
select l.oid as id, l.xmin state_number, lanname as name, lanpltrusted as trusted,
       h.proname as handler, hs.nspname as handlerSchema,
       i.proname as inline, isc.nspname as inlineSchema,
       v.proname as validator, vs.nspname as validatorSchema
from pg_catalog.pg_language l
    left join pg_catalog.pg_proc h on h.oid = lanplcallfoid
    left join pg_catalog.pg_namespace hs on hs.oid = h.pronamespace
    left join pg_catalog.pg_proc i on i.oid = laninline
    left join pg_catalog.pg_namespace isc on isc.oid = i.pronamespace
    left join pg_catalog.pg_proc v on v.oid = lanvalidator
    left join pg_catalog.pg_namespace vs on vs.oid = v.pronamespace
order by lanname
;
```

#### RetrieveRoutines

```sql
with languages as (select oid as lang_oid, lanname as lang
                   from pg_catalog.pg_language),
     routines as (select proname as r_name,
                         prolang as lang_oid,
                         oid as r_id,
                         xmin as r_state_number,
                         proargnames as arg_names,
                         proargmodes as arg_modes,
                         proargtypes::int[] as in_arg_types,
                         proallargtypes::int[] as all_arg_types,
                         pg_catalog.pg_get_expr(proargdefaults, 0) as arg_defaults,
                         provariadic as arg_variadic_id,
                         prorettype as ret_type_id,
                         proretset as ret_set,
                         prokind as kind,
                         provolatile as volatile_kind,
                         proisstrict as is_strict,
                         prosecdef as is_security_definer,
                         proconfig as configuration_parameters,
                         procost as cost,
                         pg_catalog.pg_get_userbyid(proowner) as "owner",
                         prorows as rows ,
                         proleakproof as is_leakproof ,
                         proparallel as concurrency_kind
                  from pg_catalog.pg_proc
                  where pronamespace = $1::oid
                    and not (prokind = 'a')
                    )
select *
from routines natural join languages
;
```

#### RetrieveSequences10

```sql
select cls.xmin as sequence_state_number,
       sq.seqrelid as sequence_id,
       cls.relname as sequence_name,
       pg_catalog.format_type(sq.seqtypid, null) as data_type,
       sq.seqstart as start_value,
       sq.seqincrement as inc_value,
       sq.seqmin as min_value,
       sq.seqmax as max_value,
       sq.seqcache as cache_size,
       sq.seqcycle as cycle_option,
       pg_catalog.pg_get_userbyid(cls.relowner) as "owner"
from pg_catalog.pg_sequence sq
    join pg_class cls on sq.seqrelid = cls.oid
    where cls.relnamespace = $1::oid
;
```

#### RetrieveViewSources

```sql
select
       T.relkind as view_kind,
       T.oid as view_id,
       pg_catalog.pg_get_viewdef(T.oid, true) as source_text
from pg_catalog.pg_class T
  join pg_catalog.pg_namespace N on T.relnamespace = N.oid
where N.oid = $1::oid
  and T.relkind in ('m','v')
;
```

#### RetrieveFunctionSources

```sql
with system_languages as ( select oid as lang
                           from pg_catalog.pg_language
                           where lanname in ('c','internal') )
select oid as id,
       pg_catalog.pg_get_function_arguments(oid) as arguments_def,
       pg_catalog.pg_get_function_result(oid) as result_def,
       pg_catalog.pg_get_function_sqlbody(oid) as sqlbody_def,
       prosrc as source_text
from pg_catalog.pg_proc
where pronamespace = $1::oid
  and not (prokind = 'a')
  and prolang not in (select lang from system_languages)
  and prosrc is not null
;
```

#### RetrieveRelations

```sql
select D.objid as dependent_id,
       D.refobjid as owner_id,
       D.refobjsubid as owner_subobject_id
from pg_depend D
  join pg_class C_SEQ on D.objid    = C_SEQ.oid and D.classid    = 'pg_class'::regclass::oid
  join pg_class C_TAB on D.refobjid = C_TAB.oid and D.refclassid = 'pg_class'::regclass::oid
where C_SEQ.relkind = 'S'
  and C_TAB.relkind = 'r'
  and D.refobjsubid <> 0
  and (D.deptype = 'a' or D.deptype = 'i')
  and C_TAB.relnamespace = $1::oid
order by owner_id
;
```


## PG 内省逐需求验收矩阵

以下测试使用 CreateAnalyzeSession 的真实元数据与临时 TCP listener；客户端测试使用本机 libpq 18 和 JDBC 42.7.13/42.7.3，冻结原始来源 SQL，未模拟执行结果或降低版本。

| 需求 | 证据入口（astersql-server --lib 测试过滤器） | 支持边界 |
| --- | --- | --- |
| public、连接私有搜索路径、真实关系映射 | pg_introspection_namespace、pg_introspection_names | 当前库对象，拒绝其他用户 schema/跨库 |
| 一致 OID、regclass、参数及 Describe | pg_introspection_oid、pg_introspection_parameters | 明确类型、文本格式，NULL/边界/错误恢复 |
| 真实关系及谓词/连接 | pg_introspection_relations、pg_introspection_predicates、pg_introspection_joins | SQL 三值逻辑及有界目录表达式 |
| CTE/子查询与 Execute 新快照 | pg_introspection_cte | 非递归只读，拒绝越界及写入 |
| 列/类型/默认值、索引/约束 | pg_introspection_columns、pg_introspection_constraints | 真实 model 元数据，未知字段/函数明确拒绝 |
| 函数/语言、序列依赖、视图来源 | pg_introspection_functions、pg_introspection_dependencies、pg_introspection_clients | 原生能力来源明确，不伪造用户函数或拥有关系 |
| 真实客户端和默认 MySQL 隔离 | pg_introspection_clients、postgres_client_protocol_versions、mysql_protocol_ | libpq 3.0/3.2、JDBC 文本结果；UI 未验收 |

原生类型与目录私有类型使用互不重叠的内部码，DECIMAL 结果仍为 numeric（1700）；没有借目录数组支持扩大原生 JSON/数组类型承诺。完整 DataGrip UI 元数据树、持久重启、RealTiKV、生产鉴权与大 schema 性能仍未验证。

2026-10-03 DataGrip 实际 Bind 回归：会话 1533977259 的函数源、序列依赖等查询被二进制参数拒绝。现补齐目录参数解码，TCP 覆盖 OID 无符号边界、NULL、混合格式及坏数据后的 Sync 恢复；本机 JDBC 42.7.13/42.7.3 强制二进制 int8 参数且保留文本结果，对三条原始来源查询执行回归。此证据不代表默认二进制结果或完整 UI 验收；同会话 RetrieveTables 的有序 array_agg/关联标量子查询仍报 expected )，其他目录存在独立缺口。

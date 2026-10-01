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

扩展查询支持 Parse、Bind、Describe、Execute、Close、Sync、Flush，具备命名 statement/portal、重复和乱序 `$n` 参数映射、分段返回 PortalSuspended、错误后丢弃消息直到 Sync。参数传给既有预处理接口，不通过字符串拼接值。参数必须提供明确类型 OID；没有参数类型推断，参数化投影的结果元数据缺失或 prepare/execute 元数据不一致时明确报错。二进制参数与结果格式被拒绝。JDBC 连接需设置 `binaryTransfer=false`，避免驱动在达到 prepareThreshold 后切换为二进制结果。美元引用、引擎可执行注释/提示注释及 `?` 参数标记不支持。

3.0 BackendKeyData 使用 4 字节随机取消密钥，3.2 使用 32 字节；CancelRequest 必须匹配当前后端和密钥。真实 libpq 验证 idle cancel 不影响下一条查询；相邻 TCP 测试验证正在执行的命令取消、错误密钥及旧/空闲取消不会污染下一命令。事务状态来自共享会话的 in_transaction；只报告 I/T，不承诺 PostgreSQL 出错事务的 E 状态及其后续语义。

## DataGrip 内省目录探测

PG 独立目录查询结构支持 `pg_database`/`pg_shdescription` 数据库列表、`pg_namespace`/`pg_description` namespace 列表、`pg_tablespace` 基础投影，以及 `pg_locks` 最老事务探测。支持这些查询所需的投影、别名、bigint/varchar 转换、受限 LEFT JOIN、WHERE、CASE 排序和 LIMIT；未知目录、不支持结构与语法错误明确报错。此范围不代表完整 PostgreSQL 语法或 pg_catalog 兼容；批量 SQL 不支持。

数据库名和 ID 来自当前 canonical InfoSchema，ID 是 AsterSQL 原生 schema ID，并非 PostgreSQL 32 位 OID；当前数据库排在首位，其余按 ID 排序。AsterSQL 数据库不是 PostgreSQL template，root 开发鉴权下允许连接。原生元数据没有 PostgreSQL database owner 或 shared description，因此两列返回 SQL NULL。结果明确报告 bigint、text、boolean 类型。

事务探测读取当前 Domain 的真实 `information_schema.tidb_trx`，返回最小的原生 TSO start timestamp。没有活动事务时返回零行；事务结束后移除。这里的 transaction_id 是原生 64 位事务标识，不是 PG wraparound XID，也不代表完整 PostgreSQL 锁模式、持锁表或 `age(xid)` 语义。

namespace 对应原生数据库，当前 schema 是所选数据库；不合成 public schema。namespace OID 对正 schema ID 编码为两倍、负 ID 编码为绝对值两倍减一，并检查 32 位范围。没有原生 PG XID、owner 或注释来源，xmin/state_number、owner 和 description 为 SQL NULL。原生存储没有 PG tablespace 映射，因此 pg_tablespace 返回零行，仍提供 bigint/text 列元数据；spcacl/spcoptions 在当前基础查询中定义为可空 text，不承诺 PG 数组语义。

普通 Query 和扩展 Parse/Bind/Describe/Execute 都支持这些基础探测。按 [PG18 Close 规范](https://www.postgresql.org/docs/18/protocol-flow.html)，关闭不存在的 statement/portal 也返回 CloseComplete；这使 JDBC 在 Parse 报错后清理失败对象时不再阻断下一条查询。PG 目录 statement/portal 生命周期按 PG statement 名字管理，不分配或关闭 engine prepared handle；在 Execute 时重新读取元数据，继续使用统一 portal 分页和 Sync 错误恢复。共享会话接口仅新增协议无关的只读 schema snapshot，没有加入 PG SQL 解析或目录行为。

本机 DataGrip JDBC 42.7.13、42.7.3 对两条原始 SQL 的普通及 PreparedStatement 查询通过，并验证实际新建/删除数据库、boolean/NULL 类型、活动事务及回滚后的变化。真实 TCP 测试另外验证 Parse 后新建库可见、两个目录 statement 的 Close 相互隔离、最老事务跨连接排序。未验证完整 DataGrip 元数据树或 RealTiKV 分布式锁行为。

## 类型与错误边界

结果 OID 由引擎原始类型和完整标志得到，不凭列名或数据值猜测。支持整数（unsigned 按范围扩大）、numeric、float4/float8、text、bytea、布尔标志、date、time、无时区 timestamp 及 NULL。BOOL 表定义仍遵循现有引擎 TINYINT 语义；布尔表达式保留 bool 标志。DATETIME/TIMESTAMP 都映射无时区 timestamp，不推测绝对时区。结果只使用文本格式，表 OID/属性编号未知为 0，typmod 为 -1。

参数支持 OID 16、17、20、21、23、25、700、701、1042、1043、1082、1083、1114、1700 的文本值及 NULL；bytea 仅接受十六进制形式。日期时间参数使用明确格式，时间精度最多微秒。不支持类型、缺失元数据、零日期、负数或超出一天的 TIME duration、文本 NUL 等明确返回错误。JSON、enum/set、数组、带时区类型及全部二进制格式不作兼容承诺。

PG 边界映射语法错误 42601、已知唯一键错误 23505、未知列 42703、未知表 42P01、未知库 3D000、取消 57014，并对不支持功能使用 0A000、错误报文使用 08P01。部分执行错误仍是共享接口字符串，仅匹配已知引擎错误形式；未知错误保留 XX000，不保证全量 PostgreSQL 错误分类。

## 回归与未验证项

从仓库根目录执行：

```bash
cargo test -p astersql-server postgres_client_protocol_versions --lib
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

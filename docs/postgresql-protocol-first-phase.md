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

startup 接受 user、database、application_name 和 UTF8/UTF-8 client_encoding；其他参数返回不支持。database 在鉴权成功后通过既有会话接口选择。libpq 连接配置应显式设置 `sslmode=disable gssencmode=disable` 并指定存在的数据库。

## 已验证工作流

真实 TCP/libpq 默认 3.0 与显式 3.2 回归执行 SELECT、CREATE TABLE、INSERT、UPDATE、DELETE、DROP TABLE、显式 int4 OID 的 `$1` 参数及 BEGIN/COMMIT/ROLLBACK。同一 Server 上的 MySQL 连接在 PG 工作流前完成鉴权，之后 COM_PING 仍成功。

简单 Query 只接受一条现有引擎可执行的语句；空查询返回 EmptyQueryResponse，多语句返回 0A000。允许的 AST 命令还包括集合查询、CREATE/DROP DATABASE、ALTER/TRUNCATE TABLE、DROP VIEW、SET；这些命令的全部 SQL 变体没有逐一验收。REPLACE 与其他不支持命令被拒绝。没有全局 SQL 改写或 pg_catalog 仿真。

扩展查询支持 Parse、Bind、Describe、Execute、Close、Sync、Flush，具备命名 statement/portal、重复和乱序 `$n` 参数映射、分段返回 PortalSuspended、错误后丢弃消息直到 Sync。参数传给既有预处理接口，不通过字符串拼接值。参数必须提供明确类型 OID；没有参数类型推断，参数化投影的结果元数据缺失或 prepare/execute 元数据不一致时明确报错。二进制参数与结果格式被拒绝。美元引用、引擎可执行注释/提示注释及 `?` 参数标记不支持。

3.0 BackendKeyData 使用 4 字节随机取消密钥，3.2 使用 32 字节；CancelRequest 必须匹配当前后端和密钥。真实 libpq 验证 idle cancel 不影响下一条查询；相邻 TCP 测试验证正在执行的命令取消、错误密钥及旧/空闲取消不会污染下一命令。事务状态来自共享会话的 in_transaction；只报告 I/T，不承诺 PostgreSQL 出错事务的 E 状态及其后续语义。

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

本机未找到 PG JDBC 驱动，DataGrip/JDBC UI 连接与 SELECT 1 未验证，不承诺 DataGrip 元数据浏览。没有验证 RealTiKV、生产鉴权、TLS、COPY、复制、通知、完整 pg_catalog、ORM/JDBC 全组合或完整 PostgreSQL SQL 语法/事务语义。结果和 portal 当前全量物化；增加原始类型向量，必要时进行只读 AST/catalog 元数据解析，没有大结果吞吐或内存基准。

PG 编解码、OID、SQLSTATE、鉴权协商和连接状态仅在 `pkg/server/pg_*.rs`；共享执行模块不引用 PG。现有入口仅接线配置与独立 listener。后续支持能力应继续沿该边界扩展，并同时验证 MySQL 行为。

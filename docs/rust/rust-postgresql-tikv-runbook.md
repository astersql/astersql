# AsterSQL Rust + TiKV：启动 PostgreSQL 协议入口

本文在 [Rust client-rust / TiKV 运行手册](rust-client-tikv-runbook.md) 的启动流程上增加 PG listener。一个 Rust AsterSQL 进程同时提供 MySQL 4000、PG 5432 和 status 10080；底层使用同一 PD/TiKV 集群。无需另行启动 PostgreSQL 数据库进程。

以下命令用于本机开发模式，root 空密码。PG 支持前后端协议 3.0 和 3.2，拒绝保留编号 3.1。SQL 仍由 AsterSQL 执行引擎处理，详细边界见 [PostgreSQL 协议支持说明](../postgresql-protocol-first-phase.md)。

## 1. 确认 PD/TiKV 和端口

在仓库根目录操作：

```bash
cd /Users/Shared/work/dir/data/codes/astersql-tidb
```

复用已有 Docker PD/TiKV 时，按原手册检查并启动已有容器：

```bash
./scripts/run-local-tikv-source.sh existing-status
./scripts/run-local-tikv-source.sh start-existing
```

如果使用脚本管理的源码 PD/TiKV，则使用以下命令，二选一即可：

```bash
./scripts/run-local-tikv-source.sh start
./scripts/run-local-tikv-source.sh status
```

确认 PD 健康、TiKV store 为 Up：

```bash
/usr/bin/curl --fail http://127.0.0.1:2379/pd/api/v1/health
/usr/bin/curl --fail http://127.0.0.1:2379/pd/api/v1/stores
```

AsterSQL 的 `--path` 填 PD 地址 `127.0.0.1:2379`，不要填 TiKV 的 20160 端口。检查 SQL/status 端口占用：

```bash
lsof -nP -iTCP:4000 -sTCP:LISTEN
lsof -nP -iTCP:5432 -sTCP:LISTEN
lsof -nP -iTCP:10080 -sTCP:LISTEN
```

PG 默认关闭，已经运行的进程需要以新增 `--postgres-port=5432` 的命令重启才能启用。重启前先结束该实例的业务工作，再通过原来的启动终端或服务管理方式停止它；不要删除 PD/TiKV 数据。若保留现有实例并另起开发实例，将下文 MySQL/status 端口分别换成 4001/10081；5432 被占用时也要更换 PG 端口，并同步修改 psql 命令。

## 2. 启动 Rust server 并启用 PG

在第一个终端前台运行，保持终端开启：

```bash
cd /Users/Shared/work/dir/data/codes/astersql-tidb

PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc \
cargo run -p astersql-cmd-tidb-server \
  --bin astersql-cmd-tidb-server --locked -- \
  --store=tikv \
  --path=127.0.0.1:2379 \
  --host=127.0.0.1 \
  -P=4000 \
  --postgres-port=5432 \
  --report-status=true \
  --status-host=127.0.0.1 \
  --status=10080 \
  --socket=
```

相对于原 TiKV 手册，关键新增项是 `--postgres-port=5432`。`-P=4000` 仍是 MySQL 端口；`--socket=` 关闭当前不支持的 Unix socket。不要启用 secure bootstrap：当前 PG 鉴权只支持开发模式 root 空凭据。

也可在已有 TOML 配置的顶层加入 `postgres-port = 5432`，启动时通过 `--config=<配置文件路径>` 加载；命令行 `--postgres-port` 优先于配置文件。

在第二个终端确认 server 就绪：

```bash
/usr/bin/curl --fail http://127.0.0.1:10080/status
lsof -nP -iTCP:5432 -sTCP:LISTEN
```

预期 status 请求成功，5432 的监听进程为 Rust AsterSQL。

## 3. 准备 test 数据库

PG startup 会选择 `dbname` 对应的已有数据库。为避免 `test` 不存在导致连接失败，通过 MySQL 入口确认创建：

```bash
/opt/homebrew/opt/mysql-client@8.4/bin/mysql \
  --protocol=TCP --host=127.0.0.1 --port=4000 \
  --user=root --skip-password \
  --execute='CREATE DATABASE IF NOT EXISTS test;'
```

若启动时使用其他 MySQL 端口，这里也要同步修改。

## 4. 使用 psql 显式连接协议 3.2

显式请求 3.2 需要 libpq/psql 18，先检查安装版本：

```bash
/opt/homebrew/opt/libpq/bin/psql --version
```

使用以下连接命令：

```bash
/opt/homebrew/opt/libpq/bin/psql 'host=127.0.0.1 port=5432 user=root dbname=test sslmode=disable gssencmode=disable min_protocol_version=3.2 max_protocol_version=3.2'
```

连接后执行：

```sql
SELECT 1;
```

预期返回一行数值 1。PG 的 public 映射 dbname 选择的当前原生库，默认 search_path 为 public；可用 `SHOW search_path` 和 `SELECT current_schema()` 检查。访问当前库表用 `public.<表名>`，不要把原生库名当成 PG 用户 schema；跨库与其他用户 schema 被拒绝。无需创建原生 public 库。输入 `\q` 退出。若希望跳过本机 psqlrc 并直接完成一次查询：

```bash
/opt/homebrew/opt/libpq/bin/psql \
  'host=127.0.0.1 port=5432 user=root dbname=test sslmode=disable gssencmode=disable min_protocol_version=3.2 max_protocol_version=3.2' \
  -X -A -t -v ON_ERROR_STOP=1 -c 'SELECT 1'
```

预期标准输出为 `1`，退出码为 0。PG 入口目前不提供 SQL TLS/GSS 加密，因此连接参数显式禁用这两项。

## 5. 默认协议 3.0 连接

不指定协议上下限时，本机 libpq 18 默认使用 3.0：

```bash
/opt/homebrew/opt/libpq/bin/psql \
  'host=127.0.0.1 port=5432 user=root dbname=test sslmode=disable gssencmode=disable' \
  -X -A -t -v ON_ERROR_STOP=1 -c 'SELECT 1'
```

握手返回 `server_version = 18.0 (AsterSQL)`。这是协议兼容基线声明，不表示实现完整 PostgreSQL 18 功能。不要用 `SELECT version()` 的执行引擎结果判断 PG 握手声明。

## 6. 常见问题与验证范围

- `Connection refused`：确认启动命令包含 `--postgres-port=5432`、进程启动成功且 5432 已监听。只启动 PD/TiKV 或只启动 MySQL listener 不会开放 PG。
- `Address already in use`：用 `lsof` 确认占用者，选择空闲端口并修改服务端及客户端配置。
- JDBC 报 `binary results are unsupported`：设置连接属性 `binaryTransfer=false`；当前只支持文本结果，JDBC 达到 prepareThreshold 后可能请求二进制格式。
- 客户端不识别 `min_protocol_version`：检查当前调用的 psql/libpq 版本；显式 3.2 使用 libpq 18。
- 数据库不存在：通过 MySQL 入口执行第 3 节命令，或将 `dbname` 改为已有数据库。
- TLS/GSS 或身份错误：使用文中的禁用加密参数和 root 空密码开发模式；当前不支持生产 PG 密码鉴权。
- `unsupported startup parameter DateStyle`：当前接受 ISO 和 ISO, MDY，并报告 ISO, MDY；此前 JDBC 启动失败说明已过期。其他 DateStyle 值仍不支持，先检查客户端实际发送值。此前两版本机 JDBC 驱动已验证连接；这不代表完整 DataGrip UI 内省通过。
- `\dt`、`\d`、DataGrip 表结构浏览：这些操作可能查询 PostgreSQL 系统目录，当前不承诺完整 pg_catalog。先用明确的 SQL `SELECT 1` 验证入口。

已实测临时双 listener 上的默认 psql 18 查询，以及 libpq 18 的 3.0/3.2 查询、文本参数、事务和取消。2026-10-09 又在 TiKV 8.5.1 RealTiKV 环境验证 libpq/psql 18 的 DDL 与 DML 提交/回滚，以及 JDBC 42.7.13/42.7.3 的 prepared CRUD、提交/回滚和失败事务恢复。完整 PostgreSQL SQL 语义和生产鉴权仍未验证。

### 2026-10-09 RealTiKV + DataGrip 复验

本次使用隔离 tag `astersql-task6-20261009` 和端口 PD/MySQL/PG/status `13379`/`14001`/`15432`/`20080`，不影响默认 2379/4000/5432 服务。TiUP 1.17.1 启动 TiKV 8.5.1，Rust server 通过 `--store=tikv --path=127.0.0.1:13379` 连接，专用库为 `pg_task6_delivery`。

DataGrip 2025.1.3 的 Test Connection 成功，显示 `18.0 (AsterSQL)`、JDBC 42.7.13 和 21 ms ping。UI 编辑器的 PostgreSQL DDL 生命周期已在 RealTiKV 生效，但 Database Explorer 刷新后留有 ALTER 前列缓存。删除旧任务数据源、创建并成功测试全新数据源后，IDE 仍未在不重启的情况下向 Explorer 动态注册新节点。为避免中断用户其他 DataGrip 会话，未强制重启；最终列树保留为待回归项。完整自动内省仍可报告已知范围外的 DateStyle、timezone、roles 和 server objects；当前可靠范围是连接、编辑器 DDL/DML、JDBC prepared CRUD 和 DML 事务。

日常退出前台 Rust server 可在其启动终端按 Ctrl-C。PD/TiKV 的停止与重启仍按原手册执行，保留数据目录。

基础内省边界与回归：参见 [PostgreSQL 首期协议说明](../postgresql-protocol-first-phase.md#datagrip-内省目录探测)。2026-10-01 libpq 18 的 3.0/3.2 首轮统一测试通过，覆盖数据库、namespace、空 tablespace 元数据及错误恢复。扩展协议与本机 JDBC 42.7.13/42.7.3 文本结果回归已通过；DataGrip 安装资源中的 tablespace SQL 已提取并通过 JDBC 验证支持边界。完整 tablespace 查询仍返回 0A000，错误后连接可恢复；UI 内省未验收。2026-10-03 的真实客户端回归进一步覆盖 public/搜索路径、关系/列/索引/约束、视图源及函数/序列依赖来源查询，含 OID/NULL/边界、Describe、Parse 后 DDL 与错误恢复；能力以协议说明的逐需求矩阵为准。原生无用户存储程序或序列 owned-by 元数据时，相应来源查询为类型正确的真实空集合，不能据此宣称完整 PG 内省。

# AsterSQL Rust client-rust / TiKV 运行手册

本文说明如何从本机源码启动一套单节点 PD/TiKV，并验收 AsterSQL Rust
存储链路。该集群只用于开发测试，不适合生产环境。

## 依赖与端口

- PD 源码默认位于 `/Users/Shared/work/dir/data/codes/pd-master`。
- TiKV 源码默认位于 `/Users/Shared/work/dir/data/codes/tikv-master`。
- `pkg/store/driver/Cargo.toml` 和 `Cargo.lock` 都固定
  `tikv-client = 0.4.0`。
- protobuf 代码生成使用
  `/opt/homebrew/opt/protobuf@21/bin/protoc`。
- PD client/peer 分别监听 `127.0.0.1:2379`、`127.0.0.1:2380`。
- TiKV KV/status 分别监听 `127.0.0.1:20160`、`127.0.0.1:20180`。

AsterSQL 只配置 PD 地址 `127.0.0.1:2379`。TiKV 地址和 Region leader
由 PD 返回，不应把 `20160` 配置给 AsterSQL。

需要覆盖源码或二进制位置时，设置 `PD_SOURCE`、`TIKV_SOURCE`、
`PD_BIN`、`TIKV_BIN`。需要并行运行多套集群时，还应设置
`ASTERSQL_TIKV_STATE_DIR` 和全部端口环境变量。

## 构建并启动源码集群

脚本会在二进制缺失时执行对应构建，也可以预先构建：

```bash
make -C /Users/Shared/work/dir/data/codes/pd-master pd-server
cd /Users/Shared/work/dir/data/codes/tikv-master
cargo build --bin tikv-server
```

从 AsterSQL 仓库根目录启动并检查：

```bash
./scripts/run-local-tikv-source.sh start
./scripts/run-local-tikv-source.sh status

curl --fail http://127.0.0.1:2379/pd/api/v1/health
curl --fail http://127.0.0.1:2379/pd/api/v1/cluster
curl --fail http://127.0.0.1:2379/pd/api/v1/stores
curl --fail http://127.0.0.1:20180/status
```

开始测试前，PD health 中的成员必须为 `health: true`，PD stores 中必须
至少有一个 `state_name: Up`。脚本把数据、日志和 PID 默认放在
`${TMPDIR:-/tmp}/astersql-local-tikv-source-${UID}`。

脚本只向 PID 文件记录且命令行包含该私有数据目录的进程发信号。端口已被
其他 PD/TiKV 占用时会直接失败，不会停止现有进程。

## 真实端到端验收

下面的 ignored 门禁不包含 PD、TiKV、TSO、MVCC、Region 或
Coprocessor 模拟。测试会提交和回滚事务、比较提交前后快照、发送标准
TiDB DAG，并调用脚本重启自管 TiKV 后复用同一客户端验证重连。

```bash
REAL_TIKV_PD=127.0.0.1:2379 \
PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc \
cargo test -p astersql-store-driver \
  real_tikv_client_rust_end_to_end \
  --locked -- --ignored --nocapture
```

成功输出包含非零 cluster identity、严格递增的两次 TSO、MVCC
commit/rollback/snapshot 结果、DAG packet/byte 计数、重连后的 TSO
和 DAG 结果，以及错误 PD 的 fail-fast 耗时。

tidb-server 的真实注册、bootstrap 存储传递、有效 PD TSO 和错误 PD
失败门禁：

```bash
REAL_TIKV_PD=127.0.0.1:2379 \
PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc \
cargo test -p astersql-cmd-tidb-server \
  tikv_storage_wiring --locked -- --nocapture
```

完整 Rust 回归：

```bash
PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc \
cargo test \
  -p astersql-store-driver \
  -p astersql-store-copr \
  -p astersql-store \
  -p astersql-cmd-tidb-server \
  --locked
```

## Rust tidb-server 黑盒验收

仓库提供标准 MySQL 客户端和 status HTTP 的黑盒脚本。默认复用现有
`astersql-task4-pd`、`astersql-task4-tikv` 容器；脚本只在容器已存在但
未运行时执行 `docker start`，不会执行 `docker rm`、`docker compose
down`、`--rm`，也不会停止或重建容器。

### Docker 持久化 PD/TiKV/TiFlash

仓库根目录的 `docker-compose.tikv.yml` 启动 PD、TiKV 和 TiFlash。bind mount
默认把数据分别保存在仓库的 `.local/tikv-docker/pd`、
`.local/tikv-docker/tikv` 和 `.local/tikv-docker/tiflash/data`；可用
`ASTERSQL_DOCKER_DATA_DIR` 覆盖根目录。容器使用 `restart: unless-stopped`，
Docker Desktop 或机器重启后会自动启动；删除并重建容器时，只要不删除这些
宿主机目录，数据也不会丢失。Rust AsterSQL 按下文的 `cargo run`
命令在宿主机启动。

TiFlash 和宿主机 Rust server 都需要访问 PD/TiKV 的通告地址。启动前将
`ASTERSQL_ADVERTISE_HOST` 设为两者都能访问的宿主机 IP，例如本机当前的
`192.168.10.226`。网络地址变化后，更新此值并重建 PD/TiKV 容器。
仅启动容器并不代表 Rust server 已能使用 TiFlash 向量索引；还需验证
`/schema` 元数据接口、TiFlash 副本状态和向量查询执行计划。

仅首次明确需要丢弃全部历史数据时执行：

```bash
cd /Users/Shared/work/dir/data/codes/astersql

docker compose -f docker-compose.tikv.yml down
docker volume rm astersql-task4-pd-data astersql-task4-tikv-data 2>/dev/null || true
rm -rf .local/tikv-docker/pd .local/tikv-docker/tikv
mkdir -p .local/tikv-docker/pd .local/tikv-docker/tikv

ASTERSQL_ADVERTISE_HOST=192.168.10.226 docker compose -f docker-compose.tikv.yml up -d
```

`rm -rf` 会不可恢复地删除现有集群数据，日常重启或重建不得执行。日常启动、
停止和重建容器使用：

```bash
ASTERSQL_ADVERTISE_HOST=192.168.10.226 docker compose -f docker-compose.tikv.yml up -d
docker compose -f docker-compose.tikv.yml stop
docker compose -f docker-compose.tikv.yml down
ASTERSQL_ADVERTISE_HOST=192.168.10.226 docker compose -f docker-compose.tikv.yml up -d
```

`docker compose down` 只删除容器和网络，不删除 bind mount 中的数据。确认
容器确实挂载了宿主机目录：

```bash
docker inspect astersql-task4-pd \
  --format '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{println}}{{end}}'
docker inspect astersql-task4-tikv \
  --format '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{println}}{{end}}'
```

先确认容器、PD health 和 TiKV store：

```bash
./scripts/run-local-tikv-source.sh existing-status
./scripts/run-local-tikv-source.sh start-existing

/usr/bin/curl --fail http://127.0.0.1:2379/health
/usr/bin/curl --fail http://127.0.0.1:2379/pd/api/v1/stores
```

执行完整验收：

```bash
PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc \
./scripts/test-rust-tidb-server.sh
```

脚本会构建并启动 Rust server，等待 `127.0.0.1:4000` 和
`127.0.0.1:10080`，再使用
`/opt/homebrew/opt/mysql-client@8.4/bin/mysql` 以 `root` 空密码完成
`SELECT VERSION()`、`SELECT 1`、建库建表、INSERT、UPDATE 和 SELECT。
随后它创建一个新连接读取唯一值，证明结果来自同一真实 TiKV；最后请求
`/status`、发送 SIGTERM，并确认两个 Rust listener 都已关闭。专用测试库会
在成功路径删除，PD/TiKV 容器始终保留。

可直接手工启动：

```bash
PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc \
cargo run -p astersql-cmd-tidb-server --locked -- \
  --store=tikv \
  --path=127.0.0.1:2379 \
  --host=127.0.0.1 \
  -P=4000 \
  --status-host=127.0.0.1 \
  --status=10080 \
  --socket=
```

```bash
cargo run -p astersql-cmd-tidb-server \
  --bin astersql-cmd-tidb-server -- \
  --store=tikv \
  --path=127.0.0.1:2379 \
  --host=127.0.0.1 \
  -P=4000 \
  --report-status=true \
  --status-host=127.0.0.1 \
  --status=10080 \
  --socket=
```

健康检查和连接：

```bash
/usr/bin/curl --fail http://127.0.0.1:10080/status
/opt/homebrew/opt/mysql-client@8.4/bin/mysql \
  --protocol=TCP --host=127.0.0.1 --port=4000 \
  --user=root --skip-password
```

当前 Rust listener 不支持 Unix socket、SQL TLS 和 PROXY Protocol。
相关配置会在启动时显式报错；集群侧 PD/TiKV TLS 与 SQL listener TLS 是
不同边界。

## 启用 PostgreSQL 协议入口

在上述 Rust server 命令中增加 `--postgres-port=5432`，即可同时提供
MySQL 和 PG listener。完整启动、test 数据库准备及 psql 3.0/3.2 连接步骤见
[TiKV + PostgreSQL 启动手册](rust-postgresql-tikv-runbook.md)。

## tidb-server 存储参数

tidb-server 的存储注册路径由 `--store tikv`、`--path` 和可选 keyspace
组成。例如：

```bash
cargo run -p astersql-cmd-tidb-server -- \
  --store tikv \
  --path 127.0.0.1:2379 \
  --socket= \
  --keyspace-name analytics
```

最终存储 URI 为
`tikv://127.0.0.1:2379?keyspaceName=analytics`。不使用 keyspace 时省略
`--keyspace-name`；classic API 不会附加 keyspace 前缀。

TLS 集群需要同时提供 CA、客户端证书和私钥：

```bash
cargo run -p astersql-cmd-tidb-server -- \
  --store tikv \
  --path pd.example.com:2379 \
  --cluster-ca /path/to/ca.pem \
  --cluster-cert /path/to/client.pem \
  --cluster-key /path/to/client-key.pem
```

证书与私钥必须成对配置。TLS 文件和 keyspace 必须与 PD/TiKV 集群配置
一致。

## 本机运行 WordPress

WordPress 源码位于 `/Users/Shared/work/wordpress-astersql`，其
`wp-config.php` 使用 `wordpress` 数据库、`root` 空密码和
`127.0.0.1:4000`。先按上文启动 Docker PD/TiKV 与本机 Rust AsterSQL，
再创建数据库：

```bash
mysql --protocol=TCP --host=127.0.0.1 --port=4000 \
  --user=root --skip-password \
  --execute='CREATE DATABASE IF NOT EXISTS wordpress;'
```

按 `docs/dev/dev.md` 中的命令启动 PHP 开发服务：

```bash
php -S 127.0.0.1:8081 -t /Users/Shared/work/wordpress-astersql
```

打开 `http://127.0.0.1:8081/` 完成 WordPress 安装。首次访问会跳转到
`/wp-admin/install.php`；出现安装页表示 PHP 已连接 AsterSQL 的数据库。
开发服务只监听本机地址。

## 生成 5 亿条订单测试数据

仓库脚本 `scripts/load-order-data.sh` 会创建订单结构的
`load_test.orders_500m` 表，并生成确定性测试数据。默认目标为 5 亿行，
每 2000 万行为一个逻辑批次；为避免单个事务占用过多内存和事务日志，每个
逻辑批次默认再拆成每 10 万行一次提交。

脚本需要 MySQL 命令行客户端。Homebrew 安装的客户端可加入 `PATH`：

```bash
export PATH="/opt/homebrew/opt/mysql-client@8.4/bin:${PATH}"
```

首次运行前配置连接。密码为空时可以直接把 `MYSQL_PASSWORD` 设为空：

```bash
cd /Users/Shared/work/dir/data/codes/astersql

export MYSQL_HOST=127.0.0.1
export MYSQL_PORT=4000
export MYSQL_USER=root
read -rsp "数据库密码: " MYSQL_PASSWORD
export MYSQL_PASSWORD
echo

mkdir -p logs
RESET_TABLE=1 nohup ./scripts/load-order-data.sh \
  > logs/orders-500m.log 2>&1 &
echo $! > /tmp/astersql-orders-500m.pid
```

`RESET_TABLE=1` 会先删除同名订单表和辅助数字表，只能在明确需要从零开始时
使用。已有数据需要续跑时不得再次设置该变量。

查看实时日志：

```bash
tail -f logs/orders-500m.log
```

也可以通过最大订单 ID 快速查看已提交进度，避免在装载期间反复执行全表
`COUNT(*)`：

```bash
mysql --protocol=TCP \
  --host="${MYSQL_HOST}" --port="${MYSQL_PORT}" --user="${MYSQL_USER}" \
  --database=load_test \
  --execute='SELECT order_id AS committed_order_id
             FROM orders_500m ORDER BY order_id DESC LIMIT 1;'
```

需要停止时向后台脚本发送 `TERM`。当前小事务可能完成后才退出：

```bash
kill "$(cat /tmp/astersql-orders-500m.pid)"
```

中断后使用相同连接配置重新运行，但不要设置 `RESET_TABLE=1`：

```bash
cd /Users/Shared/work/dir/data/codes/astersql

nohup ./scripts/load-order-data.sh \
  >> logs/orders-500m.log 2>&1 &
echo $! > /tmp/astersql-orders-500m.pid
```

脚本以当前最大 `order_id + 1` 作为恢复点。已经提交的 10 万行小事务不会
重复写入；连接中断时尚未提交的小事务会在续跑时重新执行。断点续跑假设该表
只由本脚本写入、`order_id` 连续且没有被手工删除或插入更大的 ID。

完成后验证总行数：

```sql
SELECT COUNT(*) FROM load_test.orders_500m;
```

常用覆盖参数如下：

- `MYSQL_DATABASE`、`MYSQL_TABLE`：修改目标库表。
- `TOTAL_ROWS`：修改总行数，默认 `500000000`。
- `LOGICAL_BATCH_ROWS`：修改日志中的逻辑批次，默认 `20000000`。
- `TRANSACTION_ROWS`：修改单次提交行数，默认 `100000`，最大 `1000000`。
- `KEEP_NUMBER_TABLE=1`：成功后保留辅助数字表，便于再次装载。

例如先用 100 万行做容量和速度验证：

```bash
RESET_TABLE=1 TOTAL_ROWS=1000000 LOGICAL_BATCH_ROWS=200000 \
TRANSACTION_ROWS=10000 ./scripts/load-order-data.sh
```

5 亿行会占用大量磁盘并产生大量 TiKV 写入、Raft 和压缩流量。正式装载前应
先用小规模参数估算磁盘空间、写入速度和压缩放大，并确认 PD/TiKV 状态正常。

## 重启、停止与排障

只重启本脚本管理的 TiKV并保留数据目录：

```bash
./scripts/run-local-tikv-source.sh restart-tikv
```

停止脚本自管集群：

```bash
./scripts/run-local-tikv-source.sh stop
```

常见问题：

- `port ... is already in use`：使用 `lsof -nP -iTCP:<port> -sTCP:LISTEN`
  查明占用者；不要用本脚本停止非自管进程。可停止冲突服务，或同时覆盖
  PD/TiKV 端口环境变量。
- `PD cluster failed to respond`：先运行 health、cluster 和 stores 检查，
  再查看状态目录中的 `pd.log`。
- TiKV status 正常但 store 不是 `Up`：查看 `tikv.log`，确认 PD endpoint
  与 advertised TiKV 地址可达，等待心跳完成后重试。
- protobuf 构建失败：确认 protobuf 21 的 `protoc` 路径存在，并显式设置
  `PROTOC`。
- keyspace 打开失败：确认集群启用了 API v2 且 keyspace 已创建；classic
  集群应省略 `--keyspace-name`。
- `Unix socket listener is not supported`：当前 Rust server 只支持 TCP，
  启动时传入 `--socket=`。
- `SQL TLS is not supported` 或 `PROXY Protocol is not supported`：当前
  Rust listener 不会静默降级；移除这些 SQL 监听配置，或改用已支持的部署。
- `MySQL port is already in use`：黑盒脚本不会终止占用者。先用
  `lsof -nP -iTCP:4000 -sTCP:LISTEN` 查明进程，再决定是否停止。

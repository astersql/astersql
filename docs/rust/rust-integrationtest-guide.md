# Rust tidb-server 集成测试执行指南

本文说明如何把 `tests/integrationtest` 中的 mysql-tester 脚本运行在
AsterSQL Rust tidb-server 上，并连接真实 PD/TiKV。当前已建立可持续通过的
CRUD 冒烟基线；这不代表所有从 Go TiDB 继承的兼容性用例均已通过。

## tests 统一 Rust 入口

`tests/` 下已有 Rust 实现的主要测试套件可从仓库根目录统一运行：

```bash
./tests/run-rust-tests.sh list
./tests/run-rust-tests.sh local
```

`local` 是默认快速集合，包含：

| 测试目录 | suite | 边界 |
| --- | --- | --- |
| `tests/globalkilltest` | `globalkill` | Rust 内存控制面/parity，不启动真实多 TiDB 集群 |
| `tests/graceshutdown` | `graceshutdown` | Rust 生命周期测试，当前使用进程/连接边界桩 |
| `tests/readonlytest` | `readonly` | Rust testkit 与只读状态合同，不要求手工启动两个 Go TiDB |
| `tests/llmtest` | `llm` | Rust CLI、生成器、fixture 和 parity；不会调用外部 LLM |
| `tests/realtikvtest` 根 crate | `realtikv root` | 共享 RealTiKV harness 公共合同 |
| `tests/clusterintegrationtest` | `cluster` | TiUP 真实集群上的 mysql、Python 向量和 v8.5.1 升级场景 |

可单独运行：

```bash
./tests/run-rust-tests.sh globalkill
./tests/run-rust-tests.sh graceshutdown
./tests/run-rust-tests.sh readonly
./tests/run-rust-tests.sh llm
./tests/run-rust-tests.sh realtikv root
./tests/run-rust-tests.sh cluster --help
```

这些目录也各自提供 `run-rust-tests.sh`，例如：

```bash
./tests/readonlytest/run-rust-tests.sh
./tests/realtikvtest/run-rust-tests.sh ddltest
```

本轮 `local` 实测全部通过：Global Kill 16 条、Grace Shutdown 4 条、LLM
5 条、Read-only 5 条、RealTiKV 根合同 1 条。

## RealTiKV Rust 子套件

列出并运行某个子套件：

```bash
./tests/run-rust-tests.sh list
./tests/run-rust-tests.sh realtikv ddltest
```

`ddltest` 本轮实测 5 条通过。运行全部 `tests/realtikvtest/*/Cargo.toml`：

```bash
./tests/run-rust-tests.sh realtikv all
```

完整集合会编译所有子 crate，耗时和依赖面都显著大于 `local`。本轮它已正确发现
20 个子 suite，但被工作区同时存在的 privilege API 未完成修改阻断：
`pkg/session/runtime/control.rs` 对当前 `MySQLPrivilege`/`PrivilegeType` 接口产生
19 个编译错误。该问题不属于测试 runner；相关代码恢复可编译后应重跑 `realtikv all`。

`tests/realtikvtest` 的 Rust 文件目前既包含 testkit/mock/parity 测试，也包含按目录
迁移的场景测试。只有测试本身明确使用真实客户端且环境指向 PD/TiKV 时，才能把结果
称为真实集群验收；不能仅凭目录名 `realtikvtest` 推断所有 case 都访问真实 TiKV。

## Cluster Integration Rust runner

Cluster suite 不属于默认 `local` 快速集合。它会构建
`astersql-cmd-tidb-server --locked`，并用绝对路径把 Rust server 传给既有场景；
TiUP 仍提供真实 PD、TiKV、TiFlash。需预先安装 Cargo、TiUP、MySQL 客户端、Go、
`uv` 和 `tests/clusterintegrationtest/requirements.txt` 中的 Python 依赖。

```bash
./tests/run-rust-tests.sh cluster mysql
./tests/run-rust-tests.sh cluster python
./tests/run-rust-tests.sh cluster upgrade
./tests/run-rust-tests.sh cluster all
```

也可直接运行 `./tests/clusterintegrationtest/run-rust-tests.sh <suite>`。
`upgrade` 首段运行完整 TiDB v8.5.1 集群并写入、验证向量数据；停止该 TiUP 进程后，
第二段在同一数据路径上用 Rust tidb-server 搭配 nightly PD/TiKV/TiFlash 再次验证。
因此它验证的是升级兼容性，不表示 PD、TiKV 或 TiFlash 已由 Rust 替换。

帮助和非法 suite 不会构建或启动集群。真实运行失败时检查 TiUP 输出、4000 端口、
数据集与 `uv` 环境；Rust 行为差异应作为兼容性问题保留，不能通过更新 `r/` 掩盖。

## 尚未纳入统一 Rust 默认集合的目录

- `tests/integrationtest2` 会自行编排 PD、TiKV、TiFlash、TiCDC、BR 和 Dumpling，
  现有脚本仍是外部组件集成路径；不能只替换 tidb-server 就称为完整 Rust 支持。
- `tests/cncheckcert` 仅包含证书 fixture，不是独立测试 runner。
- `tests/mysqlcompat` 是兼容性清单/参考采集工具，其 Rust 执行测试位于对应
  `pkg/session`、`pkg/server` crate，不是一个 Cargo package。

Rust tidb-server 的真实 SQL 黑盒继续使用：

```bash
./tests/run-rust-tests.sh integration
```

后续参数会透传到 `tests/integrationtest/run-rust-tests.sh`，例如：

```bash
./tests/run-rust-tests.sh integration -t topn_pushdown
```

## 测试链路

```text
tests/integrationtest/run-rust-tests.sh
  -> mysql_tester
  -> target/debug/astersql-cmd-tidb-server (MySQL 127.0.0.1:6999)
  -> PD 127.0.0.1:2379
  -> TiKV 127.0.0.1:20160
```

`run-tests.sh` 会为每个 `.test` 文件创建同名数据库，执行 SQL 并与
`r/<用例>.result` 比较，正常结束后删除测试期间新增的数据库。

## 前置依赖

- Rust/Cargo 和 Go；Go 仅用于首次构建 mysql-tester。
- `curl`、`lsof`、`unzip`。
- protobuf 21，默认路径为 `/opt/homebrew/opt/protobuf@21/bin/protoc`。
- 可访问的 PD/TiKV；默认 PD 地址为 `127.0.0.1:2379`。

本仓库已有持久化容器时，从仓库根目录执行：

```bash
./scripts/run-local-tikv-source.sh start-existing
./scripts/run-local-tikv-source.sh existing-status

curl --fail http://127.0.0.1:2379/health
curl --fail http://127.0.0.1:2379/pd/api/v1/stores
```

第二个请求中必须至少有一个 `state_name` 为 `Up` 的 TiKV store。

## 运行已通过的 Rust 冒烟基线

从仓库根目录运行：

```bash
./tests/integrationtest/run-rust-tests.sh
```

脚本会完成以下工作：

1. 检查 PD health 和 TiKV store 状态。
2. 以 `--locked` 构建 `astersql-cmd-tidb-server`。
3. 首次运行时构建仓库锁定版本的 mysql-tester。
4. 启动 Rust server，显式传入 `--socket=`，连接真实 TiKV。
5. 执行 `rust_integration_smoke_v1` 的建表、INSERT、SELECT、UPDATE、DELETE。
6. 终止本次 Rust server，不停止 PD/TiKV。

成功输出应包含：

```text
./t/rust_integration_smoke_v1.test: ok! 9 test cases passed
Great, All tests passed
integrationtest passed!
```

本轮实测上述用例退出码为 `0`。

## 运行其他既有用例

把原 `run-tests.sh` 参数直接附在专用脚本后：

```bash
./tests/integrationtest/run-rust-tests.sh -t topn_pushdown
./tests/integrationtest/run-rust-tests.sh -t session/txn
```

也可不使用包装脚本，直接执行底层命令：

```bash
cd tests/integrationtest

TIDB_TEST_STORE_NAME=tikv \
TIKV_PATH=127.0.0.1:2379 \
./run-tests.sh -b n \
  -s ../../target/debug/astersql-cmd-tidb-server \
  -t rust_integration_smoke_v1
```

只有 `mysql_tester` 已存在时才能使用 `-b n`。首次直接运行底层脚本时去掉
`-b n`，让脚本安装测试工具。

不要在确认 Rust 结果正确前使用 `-r` 覆盖既有 `.result`。失败很可能是 Rust
兼容性缺口，不能通过录制新结果掩盖。

## 当前兼容性边界

本轮还执行了：

```bash
./tests/integrationtest/run-rust-tests.sh -t topn_pushdown
```

mysql-tester 已成功连接 Rust 服务并发送 SQL，但 Rust 返回：

```text
Error 1105 (HY000): unknown EXPLAIN table topn_pushdown
```

因此该结果表示测试接入已工作，但 `EXPLAIN FORMAT='plan_tree'` 的相关语义尚未
达到既有 TiDB 期望。扩大测试范围时，应逐个记录此类产品兼容性缺口。

## 日志与排错

- 服务日志：`tests/integrationtest/integration-test.out`。
- SQL 差异：mysql-tester 直接输出预期值、实际值和 diff。
- 默认 MySQL 测试端口：`6999`；status 端口由 runner 从 `4000` 起选择。
- 服务启动即退出时，runner 会立刻打印日志，不再等待 mysql-tester 超时。

如果上次异常中断留下了与用例同名的数据库，mysql-tester 可能报
`Error 1007 ... database ... already exists`。连接 Rust 服务后，只删除确认属于
失败用例的专用数据库，例如：

```sql
DROP DATABASE IF EXISTS rust_integration_smoke_v1;
```

不要批量删除数据库。若要手工验证服务与清理，可参考
`docs/rust-client-tikv-runbook.md` 中的 Rust tidb-server 启动及 MySQL 连接命令。

使用非默认 PD 时，同时覆盖测试存储地址与健康检查 URL：

```bash
TIKV_PATH=127.0.0.1:12379 \
PD_HTTP_URL=http://127.0.0.1:12379 \
./tests/integrationtest/run-rust-tests.sh
```

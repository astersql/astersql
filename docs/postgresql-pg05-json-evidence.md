# JSON 真实客户端四方向差分证据

2026-10-27，批次10，状态为**已完成，待回归**。依赖实现提交为
`55dd1489bbe5eb7dc5e66d336a3c041dd325596a`；依赖任务9的真实TCP仍待回归，
依据主会话补充的实现调度授权继续。本批只新增测试、脚本和证据，没有生产改动。

## 当前证据与限制

新增 `pkg/server/pg_client_integration_test.rs::pg05_json_libpq_four_directions`，
调用 `testdata/pg05_json_libpq.py`。真实双端差分尚未运行：首次冷缓存和随后热缓存
server构建均因磁盘空间不足退出101，0项有效Rust行为测试；编译失败不算行为红灯。
没有人为制造生产失败，也没有把前批局部codec或旧18.4证据当成本批验收。

脚本的诊断模式在本轮真实PostgreSQL **18.6** 上完成 **284项**断言，退出0。
此模式只访问本任务独立参考集群，明确写 `mode=reference-only, passed=false`；
它验证参考行为和客户端断言，不能证明Aster四方向、共享执行或MySQL通过。
原始记录在忽略目录 `target/pg05-json-evidence/reference-final/`，包括
`identity.json`、`cases.jsonl`、`summary.json`。

## 矩阵与可观测字段

| 覆盖 | 项数 | 断言 |
| --- | ---: | --- |
| 20种合法原文及SQL NULL × 四方向 | 84 | OID114/-1/-1、格式、NULL标志及原始十六进制字节 |
| 推导OID、OID0、空集、首行NULL × 值/NULL × 四方向 | 32 | 参数114、稳定描述、全NULL、重绑定 |
| 非法输入 × 输入/输出格式 | 84 | 非法JSON为22P02；非法UTF8和binary原始NUL为22021；无结果列/行 |
| 每次拒绝后Sync恢复 | 84 | 同一prepared statement与连接重绑定新值成功 |

合法输入包含键序、重复键、空白、所有根值、嵌套值、精确大数、数字尾零、超numeric
域指数、直接Unicode、`\u0000`与孤立代理转义。JSON null为4字节，字符串null带引号
为6字节，SQL NULL独立；binary是原文，无jsonb版本前缀。成功SQLSTATE记为N/A。
输入文本不含DSN、密码或认证载荷。标量JSON不适用数组维数或下界。

脚本实调 `PQprepare`、`PQdescribePrepared`、`PQexecPrepared`、`PQclosePrepared`。
statement Describe实际记录每个端点的参数与结果描述，不把缺字段默认相等。
libpq会发送Sync；Bind错误阶段、原始portal Describe、混合格式、重复Execute、
text参数原始NUL与抑制Execute等由任务8/9既有TCP测试补证，当前这些TCP证据仍待回归，
不声称libpq暴露了这些字段。拒绝路径只执行SELECT，没有DML副作用。

Rust夹具启动同一个Server的两协议端口，唯一数据库中MySQL原协议保留JSON(245)、
普通VARCHAR(253)、JSON null、字符串null及SQL NULL，在PG正常请求后、拒绝请求前
以及全部请求后比较值、原始列描述和行载荷。这部分当前尚未运行。

## 精确参考构建与资源管理

复用前批独立安装 `/tmp/pg05-uuid-reference/install`，没有重启或修改任何既有服务。
本轮实际SQL `version()`：PostgreSQL 18.6 on aarch64-apple-darwin25.1.0,
compiled by Apple clang version 16.0.0 (clang-1600.0.26.6), 64-bit。
`server_version_num=180006`；libpq=180000；数据库/客户端UTF8；
standard_conforming_strings=on；DateStyle=ISO, MDY；TimeZone=Asia/Shanghai。
`pg_config --configure`完整结果写入identity.json，参考工具的来源和构建方法见
[UUID构建记录](postgresql-pg05-uuid-evidence.md)。客户端版本不冒充服务端版本。
脚本严格拒绝其他服务端版本，没有18.4成功降级。

每次运行只initdb自身临时目录、领取空闲本地端口。finally关闭libpq、停止本任务
pg_ctl服务并验证端口关闭；TemporaryDirectory移除集群，SIGTERM转异常走清理。
本轮参考模式确认clients_closed、reference_port_closed、reference_directory_removed
均true。Rust guard关闭两协议端口并删除唯一数据库，记录真实清理结果；子进程guard
先关闭stdin使检查点释放，再等待Python finally，异常时也不会提前释放Cargo槽位。
Rust清理尚待实际运行。未启动Playground、TiKV或其他服务。

## 验证命令与结果

在仓库根通过 `/tmp/pg05-task10-slot.sh` 原子mkdir动态领取共享槽位：fmt使用slot-1，
首次server构建使用slot-2，热缓存server重试使用slot-1。
`CARGO_TARGET_DIR`分别为仓库绝对路径下的 `target/rust-slot-1` 与 `target/rust-slot-2`。
各阶段结束后自有锁已释放，缓存全部保留。

| 确切命令 | 退出码/结果 |
| --- | --- |
| `/tmp/pg05-task10-slot.sh cargo fmt --all` | 0，Rust每次修改后先格式化 |
| `PG18_BIN=/tmp/pg05-uuid-reference/install/bin /tmp/pg05-task10-slot.sh cargo test -p astersql-server --lib pg05_json_libpq -- --nocapture --test-threads=1` | 两次101，0有效测试；slot-2依赖写入os28，slot-1 rustc-LLVM输出os28 |
| `PG18_BIN=/tmp/pg05-uuid-reference/install/bin python3 pkg/server/testdata/pg05_json_libpq.py --reference-only unused target/pg05-json-evidence/reference-only` | 0，284项参考端断言通过 |
| `make lint` | 首次2（Go临时目录os28）；空间自然回升后重试0 |
| `/tmp/pg05-task10-slot.sh cargo fmt --all -- --check` | 0 |
| `PG18_BIN=/tmp/pg05-uuid-reference/install/bin /tmp/pg05-task10-slot.sh cargo test --config 'profile.test.package.astersql-server.debug=0' -p astersql-server --lib pg05_json_libpq -- --nocapture --test-threads=1` | 130，发现配置改变大量依赖指纹后主动SIGINT；0有效测试 |
| `git diff --check` | 0 |

采用WIP聚焦及Ready适用检查；Ready的fmt与lint通过，真实客户端尚待回归。
没有Go、依赖manifest或Bazel修改，不触发bazel_prepare；没有Go单测或failpoint操作，
未运行bazel_lint_changed。Ready技能目录缺失，使用根AGENTS和testing-flow。
日志 `/tmp/pg05-task10-server-final.log`、`/tmp/pg05-task10-reference.log`、
`/tmp/pg05-task10-lint-final.log`、`/tmp/pg05-task10-server-lowdebug.log`。低磁盘时没有删除保留缓存或忽略链接符号。为缩减server调试产物尝试debug=0，发现Cargo实际重算大量依赖指纹后中断，确认子进程结束并释放锁；没有继续无边界冷重建。

## 恢复验证与风险

恢复可写空间与server可链接依赖后，先动态领取共享槽位并导出绝对CARGO_TARGET_DIR，
设置 `PG18_BIN=/tmp/pg05-uuid-reference/install/bin`，依次执行：

```sh
cargo fmt --all -- --check
cargo test -p astersql-server --lib pg05_json_libpq -- --nocapture --test-threads=1
cargo test -p astersql-server --lib pg05_json_input -- --nocapture --test-threads=1
cargo test -p astersql-server --lib pg05_json_output -- --nocapture --test-threads=1
make lint
git diff --check
```

本族客户端预期1项Rust测试、284项严格双端断言，并有三个MySQL检查点及清理证据。
新增用例在既有独立测试模块注册，无新feature。正确性/兼容性风险是Aster双协议路径
尚未实际运行；新增测试无生产性能开销，未做性能基准、全仓或持久化验证。

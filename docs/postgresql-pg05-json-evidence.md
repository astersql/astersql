# JSON 真实客户端四方向差分证据

2026-10-28 统一回归已完成。最新共享工作区上的1项Rust测试及284项PostgreSQL18.6
双端差分通过，MySQL三个检查点与全部清理断言通过；无生产修复。详见本文末尾的
“统一回归完成证据”。以下保留2026-10-27的历史实施记录，不代表当前仍然阻塞。

## 2026-10-27 实施阶段记录（历史）

2026-10-27，批次10，当时状态为**已完成，待回归**。依赖实现提交为
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


## 统一回归完成证据

2026-10-28，验证基线 `7dd3777a359f78bff51173d21277b943a94c4aa1`。
原实现提交 `bf786328108ac85c04153e53cc1a693ed48f9fdc` 保留，测试函数与Python脚本
相对任务41均未变化；本轮只更新本文证据，不修改代码或依赖。

先核对 `/tmp/pg05-json-array-ready.log` 原始输出及其
`target/pg05-json-evidence/35971-59671/`：284项双端观察一致，summary.passed=true，
三个MySQL快照相同，全部清理字段true。该组合命令
`cargo test -p astersql-server --lib pg05_json -- --nocapture --test-threads=1`
退出0、15项通过，含本任务客户端、3项输入测试、3项结果测试及其他JSONB测试。
任务42后续只在数组3807分支扩展，任务43只改record消费者；未改本任务标量114出口、
脚本或Rust函数。前批TCP/结果测试的该证据仍适用，不重复广泛构建。
此外已读取最新任务8原始 `/tmp/pg05-task8-regression.log` 与执行脚本：同一验证基线
的 `cargo fmt --all -- --check` 退出0，以及
`cargo test -p astersql-server --lib pg05_json_input -- --nocapture --test-threads=1`
退出0、3项通过。

本任务在最新工作区另补跑精确入口：

```sh
# /tmp/pg05-task10-regression-slot.py先原子mkdir动态领取槽位1，写owner/PID，
# 所有Cargo与脚本结束后释放自己的锁，保留缓存。
export CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1
export PG18_BIN=/tmp/pg05-task14-reference/install/bin
cargo test -p astersql-server --lib pg05_json_libpq -- --nocapture --test-threads=1
```

通过包装器执行的确切命令为
`python3 /tmp/pg05-task10-regression-slot.py cargo test -p astersql-server --lib pg05_json_libpq -- --nocapture --test-threads=1`。
Cargo与包装器均退出0，1 passed/0 failed/0 ignored，284项双端差分通过；
日志 `/tmp/pg05-task10-regression-final.log`，原始结果位于
`target/pg05-json-evidence/80301-63984/`。本轮先后两次动态领取slot-2时发现冷缓存依赖
广泛重编，主动向自身Cargo发送SIGINT、等待所有子进程结束，包装器退出254（Cargo被
信号2中断）、有效测试0；保留缓存及日志，不计成功。后续slot-1可用后上述精确入口
1.49秒完成构建、1.50秒完成测试，未用跳过或忽略未定义符号的方式验收。

实际参考工具 `postgres --version` 与SQL server_version_num分别为18.6和180006；
PG18_BIN来自主会话保留的独立安装，脚本启动并清理自己的临时参考集群。libpq180000，
UTF8/UTF8，standard_conforming_strings=on，ISO/MDY、Asia/Shanghai。
identity.json保留实际version()/配置，不以旧18.4或客户端版本替代。

再次解析本轮cases.jsonl确认294条记录中284组双端观察完全相同（另10条为每端5个
statement Describe）。四方向原文/NULL重绑定84、推导/空集/首行NULL32、拒绝84、
Sync恢复84；SQLSTATE为N/A 200项、22P02 72项、22021 12项，拒绝均无结果列/行。
MySQL before/during/after原始对象完全相同，保留JSON(245)、VARCHAR(253)及JSON null、
字符串null、SQL NULL。summary的客户端/参考端口/目录清理与rust-cleanup的数据库删除、
PG/MySQL端口关闭全部true。自身锁已释放，没有服务或数据库遗留，缓存保留。

Ready适用检查：复用同一基线任务8的fmt check退出0；`make lint`复用已核对原始末尾
输出的 `/tmp/pg05-json-array-lint.log`（退出0），此后无Go/dashboard变动；本轮只有
证据文档变更，无需重跑代码检查。`git diff --check -- docs/postgresql-pg05-json-evidence.md`
退出0。无Go/Bazel/依赖修改，不触发bazel_prepare，无failpoint或RealTiKV操作。

正确性及兼容性：本族指定四方向、NULL、描述、非法值与恢复未观察到差异；未承诺完整
隐式转换或其他JSON功能。本轮没有运行时改动，无新增生产性能开销；未做性能基准、
其他平台/32位、持久化、全仓或RealTiKV验证。本任务编号文件按完成契约删除，plan.md
保持只读，其他任务文件不纳入本次提交。

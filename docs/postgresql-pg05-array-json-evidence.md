# JSON 数组 PostgreSQL 18.6 差分证据

任务41承接UUID数组提交`6a9b1ed05883d54fd46afba7d6491d475b7e72b6`，仅开放元素114/数组199。标量沿既有PgJson及114参数codec；没有复制JSON校验器或扩展其他标量子系统。任务7–10的历史待回归记录不代表本次通过，本次独立记录实际证据。

## 文件与局部接线依据

11个生产Rust文件及各自.rs.md：types/pg_array.rs增加Json(PgJson)规范元素和原字节binary输出；parser/types/field_type.rs与parser_actions/expression.rs注册JSON[]身份和Restore；session/runtime/pg_array.rs接通元素推断、构造、同族cast与原文校验，control.rs/planning.rs保存声明/推断参数、typed NULL及绑定时JSON[] cast，query.rs/relational_value.rs/system_query.rs接通SELECT/UNION已有消费者；server/pg_extended.rs接入199的Parse及text/binary Bind并委托114标量codec，pg_result.rs复用规范数组两种结果格式。额外落点是本族单一闭环不可缺少的接线，已获主会话明确授权；实际解析证明还需在grammar/main.astergram加入PgPostfixJson数组后缀规则，并由release parsergen更新generated/main_tables.rs及其说明；同步动作分派，保留MySQL方言门禁。

对应6个独立测试文件：parser/parser_test.rs、parser/types/field_type_test.rs、types/pg_array_test.rs、session/runtime/pg_array_test.rs、server/pg_extended_test.rs与server/pg_client_integration_test.rs；新增server/testdata/pg05_array_json_libpq.py和本文。既有未开放族反例从199/114迁至3807/3802或未知OID9999，保留空值、非法头及拒绝意图。没有新增生产源文件，无需新增docs-manifest条目；保留PingCAP许可和AsterSQL版权标记。

## TDD与参考端

先运行当前标量前置：`cargo test -p astersql-server --lib pg05_json -- --nocapture --test-threads=1`退出0，15项含18.6真实libpq、输入/结果TCP及JSONB相关回归；日志`/tmp/pg05-json-array-prerequisites.log`。本次不修改其他任务的状态或删除其文件。

新增2项本族server测试，经`cargo fmt --all`后运行`cargo test -p astersql-server --lib pg05_array_json -- --nocapture --test-threads=1`：退出101、2项行为失败，Parse及parameter入口拒绝199为0A000，标量114前置断言通过。日志`/tmp/pg05-json-array-red.log`。首次实施构建读取到接线编辑前的types产物，session编译报缺Json变体；该轮不算行为验收，之后冻结源码并重新fmt/编译。随后codec通过、真实客户端在::json[] Parse返回42601；独立parser测试1项失败near []，据此补JSON独立后缀规则，保留原输入/断言继续验证。

PG18_BIN=/tmp/pg05-task14-reference/install/bin，postgres --version现场为18.6，SQL server_version_num=180006，libpq180000；UTF8、array_nulls=on、standard_conforming_strings=on、ISO/MDY、Asia/Shanghai。pg_type现场核对114/typelem0/typlen-1和199/typelem114/typlen-1。参考模式392项libpq、188项TCP、12标量构造、7 SQL场景通过；summary.passed=false，不能代替双端完成。参考端确认裸null是元素NULL；JSON null采用带引号的"null"，与外层SQL NULL及JSON字符串"null"分别断言。

## 覆盖与清理

四方向IT→OT、IT→OB、IB→OT、IB→OB独立比对原始载荷、参数114/199、结果199/-1/-1/格式和值/NULL。覆盖重复键、空白、巨大指数与精确小数、转义原拼写、Unicode/孤立代理词法、矩形/六维、默认/负下界、空数组/空集、首行NULL/全NULL及重绑定；保留两层NULL及JSON null区别。拒绝非法任一元素时不产生部分数组。

参考实际采集元素词法/编码及容器头/长度/OID/维数/乘积/下界/尾随字节诊断，不猜SQLSTATE。参考端libpq拒绝计数为{'22P02': 50, '22021': 8, '54000': 18, '22P03': 20, '42804': 4, '08P01': 50}。TCP补证statement/portal Describe、精确Bind拒绝、无DataRow、错误后CREATE被丢弃、Sync后SELECT1恢复及表查询42P01。libpq未暴露字段不默认相等；成功SQLSTATE为N/A。

每次脚本自建临时目录和独立参考服务，finally关闭客户端、停止并等待参考服务、删除自己的集群目录；SIGTERM同样清理。Rust guard关闭同Server的PG/MySQL端口并删除唯一数据库。MySQL原协议在差分前/中/后核对JSON245、VARCHAR253及JSON null/字符串null/SQL NULL的值、列与raw行载荷。不操作用户服务/数据、不启动Playground。

## Ready范围、风险及限制

按根AGENTS.md及testing-flow选Ready；.agents/skills/tidb-verify-profile不存在，不报告虚构技能通过。无Go源/import/顶层Go测试、Go依赖或Bazel改动，不触发bazel_prepare；未运行bazel_lint_changed。没有外部Rust依赖修改、本地patch或临时上游副本。

正确性/兼容性限于本族及精确18.6构建，合法但未开放跨族转换仍0A000。数组运算、完整赋值/隐式矩阵、持久化与RealTiKV不在本次范围。既有lexer普通SQL字符串仍按MySQL sqlMode解反斜杠（任务7已记录）；包含数组转义的普通SQL字面量会22P02，未递归修复通用词法。三项SQL字面量cast/UNION用无需反斜杠的1e100000，复杂对象/转义仍经真实Bind四方向和ARRAY标量构造保真验收，不能声称完整PG字符串语法兼容。沿现有容器O(输入与元素总大小)处理，未做吞吐/超大数组基准、32位或其他平台验证。


## 最终验证命令

所有Cargo命令从仓库根执行，经`/tmp/pg05-json-array-cargo.sh`按1–10顺序原子mkdir动态领取槽位、写task41/会话/PID，设置PG18_BIN及绝对CARGO_TARGET_DIR。实际均领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；trap只释放自有owner/锁，缓存保留。最终组合由`/tmp/pg05-json-array-ready.sh`顺序执行，日志`/tmp/pg05-json-array-ready.log`。

| 确切命令 | 退出码及有效测试 |
| --- | --- |
| `cargo fmt --all` | 0，每次Rust变更后先执行 |
| `cargo fmt --all -- --check` | 0 |
| `cargo run --release -p astersql-parsergen -- generate` | 0 |
| `cargo run --release -p astersql-parsergen -- check` | 0，解析表一致性；非行为测试 |
| `cargo test -p astersql-server --lib pg05_array_json -- --nocapture --test-threads=1` | 0，2项；初始红灯101、2项失败 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_json -- --nocapture --test-threads=1` | 0，6项：server2/session1/parser1/parser-types1/types1 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1` | 0，165项：server53/session34/parser17/parser-types18/types43 |
| `cargo test -p astersql-server --lib pg05_int4_array -- --nocapture --test-threads=1` | 0，2项，受影响门禁的独立前缀 |
| `cargo test -p astersql-server --lib pg05_json -- --nocapture --test-threads=1` | 0，15项含本族标量真实18.6差分 |
| `cargo test -p astersql-types --lib pg_json_test -- --nocapture --test-threads=1` | 0，8项 |
| `make lint` | 0，/tmp/pg05-json-array-lint.log；后续无Go/dashboard改动，复用适用检查 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_json_libpq.py` | 0 |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin python3 pkg/server/testdata/pg05_array_json_libpq.py --reference-only unused /tmp/pg05-json-array-reference-final` | 0，参考采样，passed=false，不计双端验收 |
| `git diff --check -- pkg docs` | 0 |

最终组合整体退出0，去重190项Rust测试通过（165共享数组、2 int4、15 server JSON、8 types JSON）；本族6项已包含在共享数组中，精确过滤器重复执行不重复累计。不存在零测试替代验收。

最终本族原始证据目录`target/pg05-array-json-evidence/35070-59444`：summary.passed=true、mismatches=[]，392 libpq、376双端TCP、12标量参数构造、7 SQL场景通过；场景数不是Rust测试数。Python/Rust全部清理字段true，MySQL三次原始快照完全相同。

最终所有Cargo/脚本/参考服务及槽位产物使用者结束后，自有slot-1锁已释放；缓存保留。33个本任务文件自审通过，plan.md只读未变，既有编号任务改动不提交；无外部依赖副本或本地覆盖。推荐继续独立jsonb数组任务，并将通用PG字符串反斜杠词法缺口留给原负责任务处理。

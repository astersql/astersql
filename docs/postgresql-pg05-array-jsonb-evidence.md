# JSONB 数组 PostgreSQL 18.6 差分证据

任务42承接任务41提交`c5308c1fc83b8b9a6e61768f05d8d622a76a180b`及其独立18.6证据，仅开放元素3802/数组3807。复用PgJsonb及3802标量codec，不复制JSONB规范化实现。历史标量任务待回归标签保留，本次运行结果独立记录。

## 文件与闭环依据

11个生产Rust文件及各自.rs.md：types/pg_array.rs增加Jsonb(PgJsonb)规范元素、身份验证和逐元素binary01；parser/types/field_type.rs与parser_actions/expression.rs注册JSONB[]身份和Restore；session/runtime/pg_array.rs接通元素推断、构造、同族cast与严格规范值校验；control.rs/planning.rs保留声明/推断参数、typed NULL及绑定时JSONB[] cast；query.rs/relational_value.rs/system_query.rs接通SELECT/UNION已有消费者；server/pg_extended.rs接入3807 Parse及text/binary Bind并委托3802标量codec，pg_result.rs复用规范数组两种结果格式。额外落点属于协调会话明确授权的本族必要接线，不扩展其他标量/子系统。

6个独立测试文件为parser/parser_test.rs、parser/types/field_type_test.rs、types/pg_array_test.rs、session/runtime/pg_array_test.rs、server/pg_extended_test.rs与server/pg_client_integration_test.rs。新增server/testdata/pg05_array_jsonb_libpq.py和本文。既有未开放族反例从3807/3802迁到未知OID9999或u32::MAX，保留空值、非法头与拒绝意图。所有测试与生产文件分离，保留PingCAP许可及AsterSQL版权标记；无新增生产Rust文件，无新增docs-manifest条目。

## TDD与参考端

新增2项server本族测试后先运行`cargo fmt --all`，然后`cargo test -p astersql-server --lib pg05_array_jsonb -- --nocapture --test-threads=1`退出101，2项实际行为失败：Parse及parameter入口对3807返回0A000；3802标量文本规范化及binary01前置断言通过。日志`/tmp/pg05-jsonb-array-red.log`。实施中两次测试克隆笔误编译失败（serde_jsonb、Json变体）均修正，不计行为验收。

固定PG18_BIN=/tmp/pg05-task14-reference/install/bin，现场version()/server_version_num=18.6/180006，libpq180000；UTF8、array_nulls=on、standard_conforming_strings=on、ISO/MDY、Asia/Shanghai。pg_type现场核对3802/typelem0/typlen-1和3807/typelem3802/typlen-1。最终reference-only采样442项libpq、222项TCP、12标量构造、7 SQL场景通过；其summary.passed=false，不能代替双端验收。

## 覆盖与清理

合法输入逐项覆盖IT→OT、IT→OB、IB→OT、IB→OB。记录原始载荷、参数3802/3807、结果3807/-1/-1/格式与值/NULL；TCP补statement/portal Describe原始字段。覆盖默认/负下界、矩形/六维、空数组/空集/首行NULL/全NULL、重绑定、SQL NULL/元素NULL/JSON null/JSON字符串null。元素01版本在输入及输出独立验证，不能用数组头代替。

规范值包含键长顺序、重复键最后值、转义与未转义同键、嵌套重复键、精确大整数/长小数、指数与显示scale、Unicode与代理对。脚本使用参考端独立3802标量规范值作为预期拼写oracle，另行断言容器shape/lower/NULL/版本字节与结果元数据；不调用生产codec构造预期，不用serde浮点树替代规范值。非法Unicode、数值超域、词法/编码、缺/错版本、容器头/长度/OID/维数/乘积/下界/尾随字节由实际18.6采集SQLSTATE；坏元素跟在合法元素后，拒绝整个值。

拒绝项记录libpq观察及精确TCP Bind阶段、无DataRow、后续CREATE被丢弃、Sync后SELECT1恢复、表查询42P01。缺失字段不默认相等；成功SQLSTATE为N/A。MySQL同Server原协议在差分前/中/后检查JSON245、VARCHAR253及JSON null/字符串null/SQL NULL，比较列和原始行载荷。

Python try/finally及SIGTERM处理关闭libpq和TCP客户端、prepared/portal，停止并等待专属参考服务并删除本次临时目录；Rust guard关闭同Server的PG/MySQL端口，删除唯一数据库。测试不操作用户服务/数据，不启动Playground。

## Ready范围、风险和限制

依据根AGENTS.md及testing-flow选择Ready；verify-profile技能不存在，不报告虚构技能通过。无Go源/import/顶层Go测试、Go依赖或Bazel改动，不触发bazel_prepare；未运行bazel_lint_changed。没有外部Rust依赖修改、本地patch或临时上游副本。

正确性与兼容性证据限定本族及精确18.6；合法但未开放跨族数组转换仍0A000。数组运算、完整赋值/隐式矩阵、持久化、RealTiKV、其他平台与32位均未验证。既有普通SQL字符串沿MySQL sqlMode解反斜杠的通用词法缺口仍由原任务负责；复杂对象/Unicode转义经真实Bind四方向及标量参数构造验收，SQL字面量场景使用无需反斜杠的载荷，不声称完整PG字符串语法兼容。沿既有容器及标量规范化处理，没有新增吞吐/超大数组基准；JSONB大指数的规范文本可扩展，与标量资源约束一致。

## 验证记录

所有Cargo命令从仓库根经`/tmp/pg05-jsonb-array-cargo.sh`按1–10顺序原子mkdir动态领取槽位，写task42/session/PID，设置绝对CARGO_TARGET_DIR并由子进程继承。正常/失败/中断trap仅清理自己的owner/锁；使用槽位产物的子进程结束后才释放，缓存保留。

实际各阶段均动态领取slot-1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。TDD日志/tmp/pg05-jsonb-array-red.log，跨crate本族日志/tmp/pg05-jsonb-array-green.log，Ready组合由/tmp/pg05-jsonb-array-ready.sh顺序执行，日志/tmp/pg05-jsonb-array-ready.log。make lint独立日志/tmp/pg05-jsonb-array-lint.log。

| 确切命令（仓库根） | 退出码及有效测试 |
| --- | --- |
| `cargo fmt --all` | 0，每次Rust修改后先运行 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server --lib pg05_array_jsonb -- --nocapture --test-threads=1` | 初始101、2项行为失败；实施后0、2项通过 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_jsonb -- --nocapture --test-threads=1` | 0、6项：server2，其余各1 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1` | 0、171项：server55/session35/parser18/parser-types19/types44 |
| `cargo test -p astersql-server --lib pg05_int4_array -- --nocapture --test-threads=1` | 0、2项 |
| `cargo test -p astersql-server --lib pg05_jsonb -- --nocapture --test-threads=1` | 0、7项含真实18.6标量差分 |
| `cargo test -p astersql-types --lib pg_jsonb_test -- --nocapture --test-threads=1` | 0、14项 |
| `make lint` | 0 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_jsonb_libpq.py` | 0 |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin python3 pkg/server/testdata/pg05_array_jsonb_libpq.py --reference-only unused /tmp/pg05-jsonb-array-reference-final` | 0；参考采样passed=false不计双端验收 |
| `git diff --check -- pkg docs` | 0 |

本族跨crate原始证据目录target/pg05-array-jsonb-evidence/45745-60885：summary.passed=true、mismatches=[]，442 libpq、444双端TCP、12标量参数构造、7 SQL场景通过（场景数不冒充Rust测试数）；Python/Rust所有清理字段true，MySQL三次原始快照完全相同。Ready单独前缀目录target/pg05-array-jsonb-evidence/45932-60945，共享数组回归中的最终目录target/pg05-array-jsonb-evidence/45994-61130；两次summary.passed=true、mismatches=[]，所有Python/Rust清理字段true，MySQL快照一致。


去重194项Rust测试通过（171共享数组、2 int4、7 server JSONB、14 types JSONB）；本族6项已包含在共享数组回归内。没有零测试替代验收。

Ready底层命令全部通过后，临时槽位包装脚本因执行期间更新了自身文件，尾部读取偏移发生变化，外层退出2；不把该包装退出码写成0。所有Cargo/脚本/参考服务已结束，EXIT trap释放了自有owner/slot-1锁，缓存保留。修正后`bash -n /tmp/pg05-jsonb-array-cargo.sh`与`/tmp/pg05-jsonb-array-cargo.sh bash -c 'printf "slot-wrapper-recheck=ok\n"'`均退出0，重新动态领取slot-1、成功释放，锁目录为空；日志/tmp/pg05-jsonb-array-wrapper-check.log。此处复用已完成的行为检查，不因包装尾部错误重复昂贵差分。

30个任务文件自审通过，plan.md只读未变，既有编号任务改动不提交。推荐进入后续行构造消费者任务；通用PG字符串反斜杠词法与完整跨族数组转换留给相应原任务。

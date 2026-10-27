# date数组PostgreSQL 18.6差分证据

任务35依赖任务34提交 `d330db4b332ae1aae250e45a497a399add701230` 及保留的bpchar数组18.6证据。仅开放date元素1082/数组1182；日期日序、范围、BC/扩展年份和infinity复用已有PgDate，未复制或扩建标量子系统。

## 必要修改与范围

parser/types/field_type.rs增加Date身份、OID及DATE[]恢复；parser_actions/expression.rs将已有PgPostfixDate标量声明注册到数组动作；grammar/main.astergram新增PgCastType : PgPostfixDate [ ]，generated/main_tables.rs由parsergen generate再生，没有手改自动机。

types/pg_array.rs新增PgArrayElement::Date(PgDate)，验证1082载体并使用标量四字节日序和文本显示。容器沿既有shape、lower、typmod及两层NULL模型。session/runtime/pg_array.rs注册本族分析、构造、同族转换；query.rs/relational_value.rs连接现有共享消费者；control.rs/planning.rs保持prepared参数、typed NULL及空集的1182身份。构造叶值调用PgDate，避免BC与扩展年份经过MySQL日期归一化。

session/runtime/pg_temporal.rs将既有Parse字面量冻结局部扩展至date[]。query.rs只在date数组求值采样日期上下文。server/pg_extended.rs注册文本和binary数组输入，Bind文本数组复用标量不可变日期上下文；pg_result.rs注册文本与binary输出。任一元素非法则整体拒绝；跨族数组转换仍明确0A000。

12个生产Rust文件及对应12个.rs.md同步说明；pg_temporal.rs.md补充了原本缺少的说明并登记docs/docs-manifest.md。6个独立Rust测试文件分别在parser、parser-types、types、session、server两文件，另新增testdata/pg05_array_date_libpq.py。此前8个server拒绝门禁样例改为未开放UUID数组，2个types门禁样例改为未开放time，保留原拒绝/恢复意图。未新增Cargo依赖、本地patch或上游临时副本。

## 测试驱动

`cargo test -p astersql-server --lib pg05_array_date -- --nocapture --test-threads=1`首次成功编译，实际2项测试，标量前置通过，数组Parse收到0A000而非ParseComplete，退出101；日志/tmp/pg05-date-red.log。后续相同断言通过。编译或零测试未用作行为完成证据。

## 当前差分

真实参考为PostgreSQL18.6/180006，aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位；libpq180000。配置UTF8、array_nulls=on、standard_conforming_strings=on、ISO/MDY、Asia/Shanghai；另切换ISO/DMY与ISO/MDY。pg_type实际核对1082/1182、typelem1082和typlen=-1。

四方向覆盖IT→OT、IT→OB、IB→OT、IB→OB。日期两端4714-11-24 BC和5874897-12-31、BC/AD相邻日期、epoch日序-10957、PG epoch日序0、±1日、闰日、10000年及±infinity均独立给出原始文本和日序字节；不从参考输出生成目标期望。包括声明、OID0推断、空集、首行NULL、全NULL、重绑定、二维/六维、负下界、空数组及两层NULL。记录参数OID、结果OID/typlen/typmod/格式、原始输入输出hex和NULL；成功SQLSTATE为N/A。

日期配置场景验证DMY/MDY的01/02/2000逐元素解释，Parse冻结的数组字面量在随后切换DateStyle时仍为原值。SQL构造与同族连续cast也保持1182身份和shape/lower。非法日期、非法UTF8、词法/形状、日期二进制长度与范围、容器头/元素OID/flags、维度/乘积/下界溢出及尾随字节均现场从18.6采集诊断，再比较目标值；不猜未知SQLSTATE。

双端TCP补statement/portal Describe与无DataRow拒绝，Bind错误后流水线CREATE被丢弃，Sync后SELECT1恢复，查询该表42P01证明无副作用。同Server MySQL JSON/VARCHAR/NULL的值、列类型和原始行在前/中/后三次一致。Python finally与Rust guard关闭客户端、prepared/portal及自有服务，等待参考pg_ctl结束并删除自有临时数据库目录；不操作用户服务/数据。

## 验证环境与限制

Cargo阶段均由/tmp/pg05-date-cargo.sh动态原子领取共享槽位，设置绝对CARGO_TARGET_DIR并继承PG18_BIN=/tmp/pg05-task14-reference/install/bin。Rust修改先cargo fmt --all。Ready按根AGENTS与testing-flow执行；verify-profile技能不存在，不宣称其通过。只改Rust/专属grammar与文档，没有Go/import/依赖/Bazel变更，不触发bazel_prepare；未运行bazel_lint_changed。

兼容性仅限本次18.6构建和受测配置；其他DateStyle输出样式沿标量现有0A000门禁。未验证RealTiKV/持久化、32位、其他平台与性能基准；未扩展数组运算、存储或跨族隐式转换矩阵。后续建议独立任务36的下一时间族数组。

## 最终Ready结果

证据目录`target/pg05-array-date-evidence/13502-61015`含identity.json、cases.jsonl、summary.json、MySQL三个快照及rust-cleanup.json。最终562个libpq数组场景、48个标量参数构造场景、8个SQL构造/cast场景和6个DateStyle/冻结观察通过；460个双端TCP场景包括160个成功Execute及300个Bind拒绝/恢复。summary.passed=true、mismatches=[]；clients_closed/raw_clients_closed/reference_port_closed/reference_directory_removed以及Rust database_dropped/postgres_port_closed/mysql_port_closed全部true。

非法libpq场景现场SQLSTATE分布：22P02(18)、22021(4)、54000(18)、22007(6)、22008(20)、22P03(22)、42804(4)、08P01(58)。每项均有原始输入、两端实际错误字段及Sync恢复；Bind失败流水线无DataRow且无CREATE副作用。

所有Cargo阶段实际动态领取槽位1：`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；阶段结束后owner与锁目录清除，保留编译缓存。下表命令从仓库根执行，经`bash /tmp/pg05-date-cargo.sh`继承上述环境；失败预检与WIP/Ready复用同一过滤器。独立Rust行为测试共127项，重复运行不重复计数，JSON场景也不作为Rust测试数。

| 确切命令 | 退出码及有效数量 | 日志 |
| --- | --- | --- |
| cargo fmt --all | 0 | /tmp/pg05-date-final-server.log |
| cargo fmt --all -- --check | 0 | /tmp/pg05-date-final-server.log |
| cargo test -p astersql-server --lib pg05_array_date -- --nocapture --test-threads=1 | 0，3项，含最终新增构造器场景 | /tmp/pg05-date-final-server.log |
| cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_date -- --nocapture --test-threads=1 | 0，7项：server3/session1/parser1/parser-types1/types1 | /tmp/pg05-date-family.log |
| cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --skip pg05_array_date --nocapture --test-threads=1 | 0，113项：server32/session26/parser10/parser-types11/types34 | /tmp/pg05-date-ready.log |
| cargo test -p astersql-server --lib pg_temporal_date -- --skip pg_temporal_date_diff --nocapture --test-threads=1 | 0，7项；未把旧18.4差分算作18.6验收 | /tmp/pg05-date-ready.log |
| cargo run --release -p astersql-parsergen --bin astersql-parsergen -- generate | 0 | /tmp/pg05-date-wip.log |
| cargo run --release -p astersql-parsergen --bin astersql-parsergen -- check | 0 | /tmp/pg05-date-ready.log |
| make lint | 0，适用Go/dashboard检查 | /tmp/pg05-date-lint.log |
| python3 -m py_compile pkg/server/testdata/pg05_array_date_libpq.py | 0 | 本轮命令 |
| git diff --check | 0 | 本轮命令 |

自审确认只修改34个本任务文件；保留PingCAP许可，12个生产Rust文件顶部均保留AsterSQL处理标记。没有Go/Bazel/Cargo依赖改动；未运行昂贵RealTiKV或全包广扫。参考-only运行用于诊断采集，summary.passed=false，不计为双端验收。最终18.6证据来自上述真实双端测试。

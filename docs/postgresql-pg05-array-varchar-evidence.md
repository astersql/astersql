# varchar数组的PostgreSQL 18.6差分证据

任务33依赖任务32提交`94806dc7caefac7b3064152868fd32b56ef12a82`及保留的float8数组证据。varchar标量复用`pg_text_cast`、`pg_varchar_length`、已有1043 text/binary参数codec和标量赋值约束；其长度实现提交为`dca465fcd5394b0468b5292795d038670c5d2c76`。本轮重新运行标量21/22/23测试，不以历史状态或旧18.4证据替代本族18.6差分。

## 修改与必要接线

生产文件共10个：`pkg/parser/types/field_type.rs`注册Varchar元素身份1043/数组1015和varchar(n)[]恢复；`pkg/parser/parser_actions/expression.rs`把已有标量1043声明接入数组并保留元素长度；`pkg/types/pg_array.rs`在规范容器注册1043，复用Text(String)载体，OID/typmod独立保留。

`pkg/session/runtime/pg_array.rs`完成本族分析、构造、显式逐元素长度转换，复用标量字符计数，不以字节长度截断；`control.rs`、`planning.rs`补prepared参数推断、typed NULL及已有参数绑定桥的1015身份；`query.rs`、`relational_value.rs`把现存表达式消费者接到同一数组实现。这些是本族单一闭环必需接线，不补建标量子系统。

`pkg/server/pg_extended.rs`在既有数组分派注册1015，text/binary元素输入委托1043标量codec；`pg_result.rs`接入1015数组输出及n+4 typmod。元素长度不限制数组维长。维度、负下界、行主序、空数组与两层NULL沿通用容器保留。已有10个.rs.md同步说明，均已在docs/docs-manifest.md登记，无新增生产文件或依赖。

独立测试修改6个：parser/parser_test.rs、parser/types/field_type_test.rs、types/pg_array_test.rs、session/runtime/pg_array_test.rs、server/pg_extended_test.rs、server/pg_client_integration_test.rs。新增`pkg/server/testdata/pg05_array_varchar_libpq.py`及本文，总计28个文件。既有7个server测试及2个types测试的未开放门禁样例改下一族1014/1042，保留拒绝与恢复意图。

## 当前行为证据

新增server过滤器最初编译成功，2项实际运行均失败，退出101；Parse以0A000拒绝1015，日志`/tmp/pg05-varchar-red.log`。最终相同入口与断言通过。中间测试脚手架编译/辅助函数错误不计行为红灯。

最终证据目录`target/pg05-array-varchar-evidence/81342-55470`包含identity.json、cases.jsonl、summary.json、MySQL前/中/后三次快照与rust-cleanup.json。参考端现场version()/server_version_num为PostgreSQL18.6/180006，aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位，libpq180000；配置UTF8、array_nulls=on、standard_conforming_strings=on、ISO/MDY、Asia/Shanghai。参考pg_type实查1043/1015、typelem1043、typlen=-1，未按OID算术猜测。

424个libpq场景含原364容器场景及60个带长度四方向场景：IT→OT、IT→OB、IB→OT、IB→OB；覆盖声明/推断/OID0、重绑定、空集、首行NULL、数组NULL、元素NULL/全NULL、六维、空数组、二维及负下界、Unicode/组合字符、空串、NULL字样、引号/反斜线/花括号。所有原始输入输出为十六进制；参数及结果身份1015，binary头元素1043，结果typlen=-1，typmod无修饰-1/长度2为6，格式独立选择。长度2把é😀x截成é😀，组合字符按字符计数，不按字素簇计数，尾空格保留到目标长度。

另9个SQL差分场景检查嵌套构造、同类型连续长度转换、空/全NULL、varchar标量构造及非法长度0/10485761；非法长度包括NULL及WHERE false均22023，恢复SELECT1成功。成功SQLSTATE记录N/A。

372个双端TCP场景含136个成功portal执行及236个Bind拒绝/恢复，补证statement/portal Describe与格式、typmod。拒绝项现场18.6诊断为22P02（18）、22021（8）、54000（18）、22P03（20）、42804（4）、08P01（50），括号内是libpq拒绝场景数。覆盖先合法后非法元素、UTF8/NUL、数组词法/矩形、头与长度截断、错误OID/flags、维数/下界/乘积溢出及尾随字节。每次Bind拒绝无DataRow，流水线CREATE被丢弃，Sync后SELECT1恢复并查询该表得42P01，无副作用。

summary.passed=true、mismatches=[]。同Server MySQL前/中/后三次JSON/VARCHAR/NULL的值、列类型和raw行字节相同。Python finally和Rust guards关闭prepared、portal、libpq/TCP客户端及本任务服务，等待参考进程结束、清理自有临时数据库；reference_port_closed/reference_directory_removed/clients_closed/raw_clients_closed及Rust database_dropped/postgres_port_closed/mysql_port_closed全部true。不操作用户服务/数据。

## Ready验证

所有Cargo命令经`bash /tmp/pg05-varchar-cargo.sh`动态原子领取槽位1，继承绝对路径`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；各阶段结束trap删除自己的owner与锁，保留缓存。Rust修改后先cargo fmt --all。采用Ready，因为交付类型、执行及协议修改；verify-profile技能不存在，直接遵守根AGENTS和testing-flow。未改Go/import/依赖/Bazel，非新工作区，不触发bazel_prepare；未运行bazel_lint_changed。

| 确切命令（仓库根目录，继承上述槽位） | 退出码与有效数量 | 日志 |
| --- | --- | --- |
| `cargo fmt --all` | 0 | `/tmp/pg05-varchar-fmt4.log` |
| `cargo fmt --all -- --check` | 0 | `/tmp/pg05-varchar-ready-fmt.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server --lib pg05_array_varchar -- --nocapture --test-threads=1` | 0，3项 | `/tmp/pg05-varchar-ready-server.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_varchar -- --nocapture --test-threads=1` | 0，7项：server3/session1/parser1/parser-types1/types1 | `/tmp/pg05-varchar-ready-family.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --skip pg05_array_varchar --nocapture --test-threads=1` | 0，99项：server26/session24/parser8/parser-types9/types32 | `/tmp/pg05-varchar-ready-regression.log` |
| `cargo test -p astersql-server --lib pg_extended_test::pg_scalar_21 -- --nocapture --test-threads=1` | 0，1项 | `/tmp/pg05-varchar-ready-scalar21.log` |
| `cargo test -p astersql-server --lib pg_extended_test::pg_scalar_22 -- --nocapture --test-threads=1` | 0，1项 | `/tmp/pg05-varchar-ready-scalar22.log` |
| `cargo test -p astersql-server --lib pg_extended_test::pg_scalar_23 -- --nocapture --test-threads=1` | 0，1项 | `/tmp/pg05-varchar-ready-scalar23.log` |
| `make lint` | 0，适用Go/dashboard lint | `/tmp/pg05-varchar-lint.log` |
| `python3 -m py_compile pkg/server/testdata/pg05_array_varchar_libpq.py` | 0 | 本轮命令输出 |
| `git diff --check` | 0 | 本轮差异自审 |

共109项不同Rust测试通过，不重复计数server重跑或JSON内差分场景。自审确认未修改plan.md、未删除许可证、无本地依赖覆盖及无关任务提交。

## 风险与后续

正确性限于本族当前覆盖，合法跨族转换仍明确0A000，未实现完整数组赋值/存储及隐式转换矩阵；这些不递归扩展到本任务。兼容性证据仅此18.6构建与UTF8配置，不推广其他族或版本。长度转换复用标量实现，未做吞吐、超大数组或性能基准；未验证RealTiKV/持久化、32位或其他平台。建议继续独立批次34的bpchar数组。

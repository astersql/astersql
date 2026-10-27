# int8 数组 PostgreSQL 18.6 差分证据

任务28接续批次27提交 `b5cbe13628ebadb14b0a1bbcf6bc7acbb245c3cd`，复用批次23 `ddafa21eee86cdcd1cc7d3c719b592aea7cb7d77` 的Int8载体、转换和共享执行消费者，仅开放int8数组的普通Bind与必要接线。精确参考目录实查元素/数组OID20/1016、typlen8/-1，目录观测不是SQL支持证据。

## 实现及边界

FieldType还原INT8[]，普通Parse、prepared字段、参数声明白名单和literal binder注册1016。现有PgArray二进制输入元素支持门禁注册20，不新建容器。Bind逐元素调用parameter(20)/binary_parameter(20)，输出复用同一codec；删除此前仅供显式转换结果使用的重复int8解析。既有Int8(i64)、shape/lower、元素typmod及两层NULL沿共享消费者保持，整体校验成功才返回值。

OID20原有binary入口将不足8字节归为22P03；独立18.6确认应为08P01，按协调授权补短载荷判断，9字节仍22P03。新增ARRAY边界测试发现一元负号仅分析正Uint64 token而误拒i64::MIN，pg_array::element_type复用既有query::pg_integer_literal_type分析完整带符号表达式。实际值沿既有i128一元运算、pg_cast_int8和i64网络序，不经f64。

未新增scalar子系统、numeric数组、隐式转换矩阵、数组运算、外部依赖或存储。本族与既有int4显式转换保持原行为，其他未开放路径仍0A000。七个生产文件同步.rs.md，均为已有清单条目；独立Rust测试与源码分离，PingCAP许可及AsterSQL版权保留。

## 测试驱动证据

前置 `cargo test -p astersql-server --lib pg05_array_int8 -- --nocapture --test-threads=1` 编译成功执行1项，先通过i64边界、±9007199254740993精确text/binary、NULL、超域和9字节，随后0–7字节8项诊断全部22P03而非08P01，退出101；日志 `/tmp/pg05-int8-prerequisite-rust.log`。

`python3 /tmp/pg05-int8-prerequisite-reference.py` 退出0，独立initdb启动精确18.6，以真实libpq验证20场景（标量和数组元素各0–9字节），短08P01、8字节成功、长22P03，并验证Sync恢复，记录 `/tmp/pg05-int8-prerequisite-reference.json`，参考进程/目录清理完成。

完整注册后同一前缀执行3项，普通数组Parse报0A000，scalar/真实libpq短载荷诊断错误，退出101，日志 `/tmp/pg05-int8-red-complete.log`。先前一次脚本尚未创建的运行不作为libpq行为红灯。修正后WIP2双端差分已通过，但新增ARRAY下界构造器仍0A000（2通过/1失败），日志 `/tmp/pg05-int8-wip2.log`；保留原边界输入、复用现有带符号字面量规则后重新验证。

scalar补充探针使用首批已开放的 `SELECT $1::int8 AS v`，不扩修裸标量 `SELECT $1 AS v` 的NULL结果被描述为text这一既有范围外缺口。数组的声明/推断/OID0及外层NULL均独立验证，无此缺口。reference-only完整预校验明确passed=false，不算AsterSQL验收。

## 当前双端行为

精确 `PG18_BIN=/tmp/pg05-task14-reference/install/bin`：PostgreSQL18.6、server_version_num180006、aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位，libpq180000。identity.json保存version、pg_config构建选项、UTF8、array_nulls=on、standard_conforming_strings=on、DateStyle=ISO, MDY和TimeZone=Asia/Shanghai。

真实CreateAnalyzeSession/ConcreteSessionDriver同Server双协议内存夹具；Python ctypes libpq为本任务独立创建、启动、停止参考集群。378数组场景含222成功、156拒绝；覆盖IT→OT、IT→OB、IB→OT、IB→OB、声明/推断/OID0、反复绑定、二维/六维、负下界、空数组/空集、首行NULL、全NULL、外层SQL NULL。i64上下界与±9007199254740993逐字节精确匹配；合法十六/八/二进制、下划线、符号、空白归规范输出。成功记录原始输入、参数OID、结果OID/typlen/typmod/format和值/NULL，成功SQLSTATE=N/A；独立容器测试验证元素typmod7保真。

34标量binary场景验证min/max、±9007199254740993、-1/0/1、NULL及所有0–9字节长度，分别text/binary输出。392双端TCP场景（80合法portal执行、312拒绝恢复）补证statement/portal Describe、Bind错误阶段和ErrorResponse→ReadyForQuery。拒绝无DataRow，错误后的CREATE被丢弃，Sync后SELECT 1恢复，查询被跳过的表得42P01证明无副作用。

非法输入包括超域、词法、quoted NULL、UTF8、非法后续元素、矩形/前缀、维数/flags/OID、shape乘积/下界溢出、所有头截断、长度头/载荷不足、length<-1、0–7和9字节元素、尾随字节。18.6与AsterSQL逐项SQLSTATE一致：08P01×66、22P02×34、22P03×22、54000×18、22003×8、22021×4、42804×4。

MySQL在同Server前/中/后三次检查JSON/VARCHAR/SQL NULL、列定义及原始行载荷完全相同。Python finally和Rust Drop guards关闭客户端、prepared、portal及服务，等待子进程并清理自有临时数据库；数据库删除和双端口关闭均核验。不操作用户服务/数据，保留共享参考工具安装。

## 验证与限制

使用Ready：Rust格式、本族跨crate行为、已开放数组和整数协议回归、make lint。verify-profile技能不存在，按根AGENTS和testing-flow执行。仅Rust/测试/说明变更，非新工作区，无Go/import/顶层Go测试/依赖/Bazel变化，不触发bazel_prepare；无Go单元测试，不涉及failpoint。未运行bazel_lint_changed、Playground或RealTiKV。

动态mkdir领取Cargo slot-1，绝对 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；make lint独立领取slot-2，绝对 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。所有子进程继承所属环境，无target-dir覆盖。Python finally负责退出清理，保留缓存。前置首次shell trap字符串转义失败，已核实原持有PID47039和相关进程结束，仅清理本任务owner和锁；后续均正常释放。

正确性风险限于未覆盖输入组合；兼容性仅承诺所记录18.6/UTF8构建与已开放路径。裸int8标量直接NULL投影缺口属范围外，未修复；未验证完整数组cast矩阵、其他PG版本、32位、持久化、RealTiKV或性能/超大载荷。无新增features、Cargo依赖、vendor、third_party、本地patch或临时上游副本。

## 最终验证记录

当前最终原始证据为 `target/pg05-array-int8-evidence/57870-58539`，summary.passed=true、mismatches为空、全部清理标志true，MySQL三次原始记录相等。Rust日志 `/tmp/pg05-int8-ready-final.log` 和 `/tmp/pg05-int8-ready-remaining.log`，lint日志 `/tmp/pg05-int8-lint.log`。下表从仓库根、默认features运行，退出码全部0；共74项有效Rust测试通过（不重复计数）。

| 精确命令 | 有效测试数 |
| --- | ---: |
| `cargo fmt --all` | 格式化 |
| `cargo fmt --all -- --check` | 格式检查 |
| `cargo test -p astersql-server --lib pg05_array_int8 -- --nocapture --test-threads=1` | 3 |
| `cargo test -p astersql-session -p astersql-parser-types -p astersql-types --lib pg05_array_int8 -- --nocapture --test-threads=1` | 4（session2/parser-types1/types1） |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array -- --skip pg05_array_int8 --nocapture --test-threads=1` | 53 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types --lib pg05_int4_array -- --nocapture --test-threads=1` | 8 |
| `cargo test -p astersql-server --lib pg_core_integer -- --skip pg_core_integer_cast_parse_boundary_libpq --nocapture --test-threads=1` | 3 |
| `cargo test -p astersql-server --lib pg_core_int8_wire_semantics -- --nocapture --test-threads=1` | 1 |
| `cargo test -p astersql-server --lib pg_introspection_parameters_array_metadata -- --nocapture --test-threads=1` | 1 |
| `cargo test -p astersql-server --lib pg_binary_native_results -- --nocapture --test-threads=1` | 1 |
| `make lint` | Go/dashboard检查 |
| `PYTHONPYCACHEPREFIX=/tmp/pg05-int8-pycache python3 -m py_compile pkg/server/testdata/pg05_array_int8_libpq.py` | Python语法检查 |
| `git diff --check`、`git diff --cached --check` | 差异检查 |

额外 `cargo test -p astersql-server --lib pg_core_integer -- --nocapture --test-threads=1` 实际3通过/1失败，退出101：历史 `pg_query_test::pg_core_integer_cast_parse_boundary_libpq` 的 `testdata/pg_core_cast_syntax.py` 仍要求 `SELECT 1::text`、`CAST(1 AS int2)` 报42601，当前第一条成功而触发旧断言。该脚本最后修改提交 `40f232b42095e92ec40a5e86c9738d5ac215bbe8`，本任务未改text/int2语法或脚本；与本族数组无关，记录范围外修正建议，不吞并旧语法测试。本任务所需四方向及Ready检查均已实际运行，未因该额外旧断言缺少本族验收。

批次23旧测试仍断言int8数组1016的Parse拒绝，因本任务开放而正确失效；已同步成真实Bind/portal Describe/8字节结果断言，并保留未开放numeric数组与未知类型拒绝。WIP/Ready复用未受后续Rust改动影响的make lint结果（该目标仅Go/dashboard检查），未为报告状态重复运行。

自审只提交本任务7个生产Rust文件、5个独立测试文件、7份.rs.md、libpq脚本及本证据，共21文件；排除全部计划文件和其他任务变更。最终锁释放、缓存保留，plan.md只读。

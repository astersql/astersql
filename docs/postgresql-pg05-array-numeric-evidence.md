# numeric 数组与共享 codec 的 PostgreSQL 18.6 证据

任务30依赖批次29提交 `ff509186a1a62b8dc10886f24f32b81641d8eb5f`，其bytea接口和18.6行为证据已核实。参考目录实际返回数组OID1231/typelem1700/typlen-1、标量OID1700/typelem0/typlen-1。

## 实现与授权范围

前置检查发现PgNumeric已有完整有限/NaN/±Infinity身份与typmod，但没有binary接收算法；server有限binary输出也拒绝特殊值。用户明确“允许补齐必要 codec，继续计划”，因此在 `pkg/types/pg_numeric.rs` 集中实现 `PgNumeric::from_binary/to_binary`，server标量和数组共用，未在数组层复制算法。

接收按18.6源码numeric_recv顺序读取uint16 ndigits、signed weight、sign、14-bit dscale和base-10000 digits；全部digit包括特殊值载荷均验证，隐藏小数位按wire dscale截断，不做舍入。截断08P01、非法sign/scale/digit或尾随数据22P03。完整位数、signed weight和超过32767组的uint16 digit count保留，不经过MySQL decimal或浮点。输出保留显示scale和特殊值；零weight归0；NaN/Infinity/-Infinity实际输出分别为 `00000000c0000000`、`00000000d0000020`、`00000000f0000020`，后两者dscale32来自PG18短特殊值头，与参考实测一致。

规范 `PgArrayElement::Numeric(PgNumeric)` 注册元素OID1700，容器输出调用同一codec。parser识别numeric(p,s)[]和多维声明，FieldType/Restore保留precision/signed scale；prepared声明/推断/OID0、typed NULL、参数绑定、两处共享cast消费者与结果编码注册OID1231。逐元素apply_modifier完成后才发布整个值；shape、负下界、元素NULL与外层SQL NULL保持独立，非法任一元素整体失败。RowDescription保留目标数组元素typmod。

每个额外生产落点都有本族闭环依据：parser expression和field_type负责声明/恢复；session planning负责声明和cast参数约束，control负责prepared元数据与NUMERIC/NUMERIC[]绑定，pg_array负责规范值/目标typmod，query/relational_value负责已有两个执行消费者；types pg_numeric是已授权codec前置扩展，pg_array负责新规范变体；server pg_extended/pg_result负责正常参数及结果。所有11个生产Rust文件同步同目录.rs.md，新pg_numeric.rs.md登记docs-manifest；保留已有PingCAP许可及AsterSQL版权。

没有扩展其他标量、隐式cast矩阵、数组运算或存储子系统。numeric与其他数组族的显式跨族cast仍明确0A000，任意未开放元素表达式亦不承诺。未添加features、外部依赖、本地patch或vendor副本。

## 测试驱动和发现

临时前置诊断实际证明4种合法binary输入均0A000、3种特殊值binary输出均失败；诊断探针随后移除，不当作数组验收。原始日志 `/tmp/pg05-numeric-recovery-1.log`，值与typmod前置5项日志 `/tmp/pg05-numeric-recovery-2.log`。

正式新增 `pg05_array_numeric_scalar_binary_prerequisite` 后，`cargo test -p astersql-server --lib pg05_array_numeric -- --nocapture --test-threads=1` 编译成功实际执行1项，在合法12.340的binary接收处因0A000失败、退出101，日志 `/tmp/pg05-numeric-red.log`。共享codec后同一测试通过。其后的真实libpq测试暴露OID1700 bare参数元数据误分派，修正scalar与array身份后通过；没有把编译错误或零测试当红灯。

规范codec最大域回归实际测试131072位整数、16383位小数及二者组合，曾触发Rust格式化width上限panic（`/tmp/pg05-numeric-cross-wip.log`）。补零改用显式字符串扩展，同一测试通过；使用uint16计数支持完整36864组。session常量执行不保证运行期字段，测试改从真实Prepare字段断言身份，再运行相同执行断言。

旧测试中“numeric尚未开放”的拒绝项换为仍未开放的float8族，保留未开放族拒绝意图；没有删断言来掩盖行为失败。首次聚焦回归types的两项旧假设失败见 `/tmp/pg05-numeric-array-regression.log`，更新后28项全部通过。

## 真实差分

最终原始证据目录：`target/pg05-array-numeric-evidence/11797-58152`。summary.passed=true、mode=differential、mismatches=[]，不是reference-only或历史18.4证据。

参考工具 `/tmp/pg05-task14-reference/install/bin` 先核实PostgreSQL18.6；实际server_version_num180006，aarch64-apple-darwin25.1.0、Apple clang16.0.0、64-bit，libpq180000、双端UTF8。identity.json保存version、pg_config构建配置、目录OID/typlen、array_nulls=on、standard_conforming_strings=on、DateStyle=ISO, MDY和TimeZone=Asia/Shanghai。每次脚本自建临时initdb及专属空闲端口，AsterSQL使用CreateAnalyzeSession/ConcreteSessionDriver真实同Server双协议内存夹具。

356数组场景为222合法、134拒绝，覆盖IT→OT、IT→OB、IB→OT、IB→OB，声明/推断/OID0、重复绑定、二维/六维、默认/负下界、空数组、零行、首行NULL、全NULL、外层SQL NULL、120位大整数和超过120位小数。每项保留原始载荷、输入/输出格式、参数OID、结果OID/typlen/typmod/format、原始值和NULL；成功SQLSTATE=N/A。

另有28标量及28标量参数ARRAY构造场景覆盖四方向和typed NULL、尾零、大精度、NaN/±Infinity；20非规范binary场景包括wire dscale截断、负零、前导零组、Infinity dscale0输入与特殊值携带digits，验证输入归规范后双端输出一致。最大131072/16383位数边界在独立Rust规范codec测试中验证，未声称都经真实SQL参考端。

64目标typmod场景覆盖numeric(3,1)舍入、numeric(2,-2)负scale、numeric(2,4)scale超过precision、NaN，分别含负下界/元素NULL、外层NULL、空数组、全NULL，四方向均保持modifier元数据。20拒绝包括16次Execute舍入溢出/Infinity目标约束22003及4次NULL/零行声明非法precision/scale的Parse 22023；拒绝无DataRow，随后SELECT1恢复。

134次libpq非法输入当前18.6实测SQLSTATE：22P02×22、22021×2、54000×18、22003×4、22P03×28、42804×4、08P01×56。包括数组词法/矩形/维度、非法第二元素、超大numeric、非法binary符号/scale/digit、长度/头截断、元素不足和尾随数据，均与AsterSQL一致。

356双端TCP场景（80成功、276拒绝/恢复）补证statement/portal Describe、Bind错误阶段、无DataRow、丢弃直到Sync和恢复。libpq文本遇NUL会截断，故另用raw TCP提交完整含NUL载荷，不伪称libpq验证完整NUL。错误后发送的CREATE被丢弃，Sync后SELECT1成功且查询该表得到42P01，证明无副作用。MySQL同Server前/中/后三次JSON/VARCHAR/NULL和原始元数据/行字节一致；numeric原MySQL DECIMAL路径另由pg_scalar_07–10聚焦回归覆盖。

Python finally关闭libpq/raw sockets、prepared/portal和参考服务，wait后删除自有临时数据库；Rust guard删除唯一测试数据库并验证PG/MySQL端口关闭。最终所有清理标志true，没有操作用户服务或数据。

## Ready 验证与限制

所有命令从仓库根执行，默认features。Cargo阶段动态原子mkdir领取slot-1，绝对 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；并行只读make lint阶段动态取得slot-2，绝对目录同根 `target/rust-slot-2`。每阶段owner记录计划/任务/会话/PID，子进程继承设置，正常/失败均finally释放自己的锁，保留缓存；最终无本任务锁或服务。实际领取/释放及路径保留在每份日志首尾。

以下是交付检查命令；除表中明确保留的旧假设回归退出101外，其余退出码均0。8项本族、70项既有数组、5项server numeric/binary、5项既有numeric载体，共88项不同Rust行为测试；重复执行不重复计数。JSON中的嵌入场景不充作额外Rust测试数。

| 确切命令 | 有效测试数 | 日志 |
| --- | ---: | --- |
| `cargo fmt --all` | 格式化 | `/tmp/pg05-numeric-fmt-final.log` |
| `cargo fmt --all -- --check` | 格式检查 | `/tmp/pg05-numeric-fmt-final-check.log` |
| `cargo test -p astersql-server --lib pg05_array_numeric -- --nocapture --test-threads=1` | 2 | `/tmp/pg05-numeric-server-final.log` |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array_numeric -- --nocapture --test-threads=1` | 8（parser1/parser-types2/server2/session1/types2） | `/tmp/pg05-numeric-wip5.log` |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array -- --skip pg05_array_numeric --nocapture --test-threads=1` | parser5/parser-types5/server16/session16通过；types旧假设2失败后按下一行修正验证 | `/tmp/pg05-numeric-array-regression.log` |
| `cargo test -p astersql-types --lib pg05_array -- --skip pg05_array_numeric --nocapture --test-threads=1` | 28 | `/tmp/pg05-numeric-types-regression-final.log` |
| `cargo test -p astersql-types --lib pg_numeric_test -- --nocapture --test-threads=1` | 6，其中1已计本族codec | `/tmp/pg05-numeric-numeric-final.log` |
| `cargo test -p astersql-server --lib pg_scalar_07 -- --nocapture --test-threads=1` | 1 | `/tmp/pg05-numeric-scalar-regression.log` |
| `cargo test -p astersql-server --lib pg_scalar_08 -- --nocapture --test-threads=1` | 1 | 同上 |
| `cargo test -p astersql-server --lib pg_scalar_09 -- --nocapture --test-threads=1` | 1 | 同上 |
| `cargo test -p astersql-server --lib pg_scalar_10 -- --nocapture --test-threads=1` | 1 | 同上 |
| `cargo test -p astersql-server --lib pg_binary_native_results -- --nocapture --test-threads=1` | 1 | 同上 |
| `make lint` | Go/dashboard适用检查 | `/tmp/pg05-numeric-lint.log` |
| `PYTHONPYCACHEPREFIX=/tmp/pg05-numeric-pycache python3 -m py_compile pkg/server/testdata/pg05_array_numeric_libpq.py` | Python语法 | 退出0 |
| `git diff --check -- pkg docs`、`git diff --cached --check` | 差异自审 | 退出0 |

上表完整跨crate数组回归整体退出101，唯独types两项旧假设失败；其他四crate有效通过证据复用，types修正后单独28项退出0。这里明确保留失败而不把整体写成通过。make lint在最后仅测试断言修改前完成；其Go/dashboard检查不受两项Rust测试修正影响，因此复用结果。Ready按AGENTS/testing-flow选择，verify-profile技能不存在，未报告虚构技能通过。

仅Rust、独立测试和说明变更，现有工作区，无Go文件/导入/顶层Go测试、Go模块或Bazel元数据变更，不触发bazel_prepare。没有运行Go单元测试、变更failpoint状态、默认bazel_lint_changed或Playground/RealTiKV。

交付32文件：11生产Rust及11对应.rs.md为 `pkg/parser/parser_actions/expression.rs`、`pkg/parser/types/field_type.rs`、`pkg/server/{pg_extended,pg_result}.rs`、`pkg/session/runtime/{planning,control,pg_array,query,relational_value}.rs`、`pkg/types/{pg_numeric,pg_array}.rs`；7独立测试为 `pkg/parser/parser_test.rs`、`pkg/parser/types/field_type_test.rs`、`pkg/server/{pg_extended_test,pg_client_integration_test}.rs`、`pkg/session/runtime/pg_array_test.rs`、`pkg/types/{pg_numeric_test,pg_array_test}.rs`；脚本 `pkg/server/testdata/pg05_array_numeric_libpq.py`、`docs/docs-manifest.md`、本文。

正确性风险限于未覆盖组合；兼容性承诺精确18.6/UTF8和记录路径，不承诺其他版本、32位、完整numeric算术/数组cast矩阵、任意表达式或持久化。没有性能基准；codec时间/空间随规范数字长度增长，最大域与容器恶意长度已有聚焦测试。未验证真实TiKV或超大数组实际分配，不把内存夹具称作RealTiKV。

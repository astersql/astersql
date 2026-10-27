# int2 数组 PostgreSQL 18.6 差分证据

任务27接续批次26提交 `d597fc0d650935fb623ada1f78b79286154f1897`，只开放int2数组（元素OID21、数组OID1005）。参考普通目录实查oid/typelem/typlen为21/0/2及1005/21/-1；目录结果不代替普通SQL支持。

## 实现与局部范围

`PostgreSQLArrayElement::Int2`和`PgArrayElement::Int2(i16)`在现有类型与规范容器注册本族。形状、行主序、负下界、元素typmod及元素NULL沿原容器保留；外层SQL NULL仍独立。parser从已有TypeShort识别int2[]/smallint[]，FieldType还原INT2[]。control/planning注册prepared声明和上下文推断1005，literal binder、query和relational_value沿既有共享消费者传递身份。

Bind文本逐元素委派`parameter(21)`，二进制逐元素委派`binary_parameter(21)`，构造和显式文本转换调用已存在`pg_cast_int2`。规范容器输出2字节网络序，不在数组层复制标量解码语义。前置探针发现既有标量21分支对0/1字节误报22P03；协调会话授权将该分支增加短载荷08P01判定，保留多余字节22P03，属于本族数组闭环不可缺少的局部纠错。其他标量不改。

仅开放本族，跨int2与其他数组族转换包括typed NULL仍明确0A000；标量组成的ARRAY显式转换复用原标量规则，混合int2/int4/int8组成ARRAY沿共同整数类型选择。没有新增存储、数组运算、隐式转换矩阵或外部依赖。十个生产Rust文件同步已有.rs.md；清单已有条目，不新建生产文件或修改docs-manifest。

## 红灯与参考构建

`pg05_array_int2_scalar_prerequisite`首次编译成功运行1项，边界text/binary与NULL断言先通过，短载荷得到22P03而预期08P01，退出101，日志`/tmp/pg05-int2-prerequisite-rust.log`。独立真实libpq参考探针运行6场景确认18.6对短载荷08P01、多字节22P03及边界网络序，证据`/tmp/pg05-int2-prerequisite-reference.json`。

本族TCP/libpq测试在Parse OID1005尚未注册时编译成功执行3项，1通过/2失败，实际0A000而非ParseComplete，退出101，日志`/tmp/pg05-int2-red-registered.log`。恢复该本族注册后相同断言通过。先前一次复制测试的i16拼写错误导致编译失败，已修正，不计行为红灯。参考脚本首次未显式指定PG18_BIN，被严格版本门禁以180004拒绝；后续固定18.6。一次参考探针残留bool的非法值2，实际18.6证明它是合法整数，修正为1x后重跑，不将这些失败计为通过。

固定`PG18_BIN=/tmp/pg05-task14-reference/install/bin`：PostgreSQL18.6、server_version_num180006、aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位；libpq180000。identity.json记录version、配置及目录实查；UTF8、array_nulls=on、standard_conforming_strings=on、DateStyle=ISO, MDY、TimeZone=Asia/Shanghai。

## 当前双端行为证据

最终Ready原始证据目录为`target/pg05-array-int2-evidence/43105-55478`，验证命令见下方最终记录。首轮WIP原始证据在`target/pg05-array-int2-evidence/39453-54374`，`passed=true`且mismatches为空；独立reference-only预校验为`passed=false`，不作为AsterSQL验收。

真实CreateAnalyzeSession/ConcreteSessionDriver同Server双协议内存夹具，Python ctypes libpq独立initdb并启动专属18.6参考服务。数组共346场景（202成功、144拒绝）；覆盖IT→OT、IT→OB、IB→OT、IB→OB，声明/推断/OID0、重绑定、默认与负下界、二维/六维、空数组、空集、首行NULL、元素NULL/全NULL和外层SQL NULL。-32768/32767保持身份和2字节网络序；合法十六/八/二进制、下划线、符号、空白输入归规范输出。每条成功记录包含原始输入、参数OID、结果OID/typlen/typmod/格式、值与NULL，成功SQLSTATE=N/A。

独立标量binary共18场景，覆盖min/-1/0/1/max、NULL、0/1/3字节，分别text/binary输出。双端TCP共360场景（72合法portal执行、288拒绝及恢复），补证statement/portal Describe。所有非法输入整体拒绝，无DataRow；错误后流水线CREATE被丢弃，Sync后SELECT 1恢复且被跳过的表查询42P01证明无副作用。

拒绝项包括整数越界、非法词法、quoted NULL、非法UTF8、非法后续元素、矩形/前缀、维数/flags/OID、shape乘积/下界溢出、所有头截断、长度头/载荷不足、长度<-1、元素短/长载荷及尾随字节。18.6和AsterSQL逐项SQLSTATE相同：08P01×54、22P02×34、22P03×22、54000×18、22003×8、22021×4、42804×4。TCP精确检查Bind拒绝阶段与ErrorResponse→ReadyForQuery。

MySQL在同Server前/中/后三次检查JSON/VARCHAR/SQL NULL、列定义与原始行载荷完全相同。Python finally和Rust guards关闭prepared、portal、libpq/TCP客户端、参考服务，等待子进程并清理自有临时数据库；Rust数据库删除和双端口关闭均true。共享参考安装保留，未触碰用户服务/数据。

## 验证与限制

使用Ready，包含Rust格式、跨crate聚焦行为、已开放数组回归及make lint。verify-profile技能不存在，按根AGENTS及testing-flow选择。仅Rust/测试/说明变动、非新工作区，无Go/import/顶层Go测试/依赖/Bazel变化，未触发bazel_prepare；无Go单元测试，不涉及failpoint启停。未运行bazel_lint_changed、Playground或RealTiKV。

Cargo动态原子mkdir领取slot-1，绝对`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。make lint独立阶段领取slot-2，绝对`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。各命令和子进程继承所属槽位环境，EXIT/INT/TERM trap清理自己的owner及锁，保留缓存。

正确性风险限于未覆盖输入组合；兼容性仅承诺该18.6/UTF8构建和本族已开放路径，不承诺完整数组cast矩阵、其他族及持久化。未做性能/超大载荷基准，未验证RealTiKV、存储、其他PG版本、32位平台。无新增Cargo依赖、features、local patch或临时依赖副本。

## 最终验证命令

下列命令从仓库根执行、默认features，全部退出0。最终Rust日志`/tmp/pg05-int2-ready-final.log`，共64项有效Rust测试，0失败；`make lint`日志`/tmp/pg05-int2-lint.log`。精确18.6差分包含在server本族测试中。

| 命令 | 有效测试数 |
| --- | ---: |
| `cargo fmt --all` | 格式化 |
| `cargo fmt --all -- --check` | 格式检查 |
| `cargo test -p astersql-server --lib pg05_array_int2 -- --nocapture --test-threads=1` | 3 |
| `cargo test -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array_int2 -- --nocapture --test-threads=1` | 7（session4/parser1/parser-types1/types1） |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array -- --skip pg05_array_int2 --nocapture --test-threads=1` | 43 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types --lib pg05_int4_array -- --nocapture --test-threads=1` | 8 |
| `cargo test -p astersql-server --lib pg_scalar_05 -- --nocapture --test-threads=1` | 1 |
| `cargo test -p astersql-server --lib pg_introspection_parameters_array_metadata -- --nocapture --test-threads=1` | 1 |
| `cargo test -p astersql-server --lib pg_binary_native_results -- --nocapture --test-threads=1` | 1 |
| `make lint` | Go/dashboard检查 |
| `PYTHONPYCACHEPREFIX=/tmp/pg05-int2-pycache python3 -m py_compile pkg/server/testdata/pg05_array_int2_libpq.py` | Python语法检查 |
| `git diff --check`、`git diff --cached --check` | 差异检查 |

最终自审：只提交本族10个生产文件、对应独立测试及说明、libpq脚本和本证据（共28文件），排除其他计划变动。PingCAP许可保留，生产Rust顶部AsterSQL版权保留；未引入外部依赖或本地覆盖。最终JSON再次核验mismatches为空、MySQL三次原始证据相等、全部清理标志true；slot-1/slot-2锁均已释放并保留缓存。

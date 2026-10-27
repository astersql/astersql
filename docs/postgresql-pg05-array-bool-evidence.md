# bool 数组 PostgreSQL 18.6 差分证据

任务26接续批次25完整提交 `a59b8128874a59e9145b00aee07745c8d9803266`，只开放bool数组，元素/数组OID16/1000。参考目录实查oid/typelem/typlen分别16/0/1与1000/16/-1；目录结果不代替普通SQL支持。

## 实现与范围

规范PgArray增加Bool(bool)及元素OID16，保留shape、lower、typmod、元素NULL与外层SQL NULL。parser的PgArrayType从已有标量Boolean flag确定数组族；FieldType还原BOOL[]；runtime既有allowlist、上下文推断、prepared元数据和literal binder保留1000；query/relational_value调用同一pg_array求值器。构造及显式文本转换委派既有pg_boolean_cast(TextToBool)，Bind文本元素委派parameter(16)，二进制元素将完整载荷委派binary_parameter(16)，成功后标记消费，不在数组层复制标量接收语义。结果沿规范容器输出t/f或00/01。

前置核对发现共享标量binary入口只接受00/01且空载荷误报22P03。没有其他合格入口：目录专用binary bool也只收00/01，SQL cast接受字符串，不能代替binary接收。主会话确认修正现有OID16单一分支属于数组闭环不可缺少的最小局部接线，未移植或扩建标量子系统。该分支现为00=false、01–ff=true并规范化为01；空载荷08P01，多字节22P03。目录专用入口未改，其他标量、外部依赖和MySQL转换规则未改。

未开放bool数组与整数/text数组的显式转换矩阵，明确0A000，包括typed NULL；其他族仍沿既有门禁。只增既有规范容器和消费者的本族注册，不补建存储、运算、隐式转换或目录子系统。十个修改的生产Rust文件均同步同目录.rs.md；docs/docs-manifest.md已有相应条目，本次没有新增生产Rust文件，不改清单。

## 红灯与参考核对

新增TCP测试首次编译并执行1项，在Parse OID1000得到0A000而非ParseComplete，退出101，日志 `/tmp/pg05-bool-red.log`。独立必需标量接口测试执行1项，在ff得到22P03，退出101，日志 `/tmp/pg05-bool-prerequisite.log`。完整标量诊断临时测试执行1项，收集空载荷、00、01、02、7f、ff、0100的实际结果，空载荷及02/7f/ff与契约不同，退出101，日志 `/tmp/pg05-bool-alternatives.log`。这些都是编译成功的运行期失败，非零测试。

独立参考探针以真实18.6普通prepared statement采集16条标量binary场景，确认非零单字节、空载荷和多字节SQLSTATE；参考数组探针通过870个libpq与170个TCP参考场景。reference-only明确passed=false，不作为AsterSQL差分通过。第一次参考探针误写标量bool typlen预期-1，实查为1后更正重跑；首次集成探针因Python局部名称遮蔽PQgetvalue而失败，更正后重跑，不把这些失败记为验收通过。

## 当前真实双端证据

固定PostgreSQL18.6（180006），aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位；libpq180000。PG18_BIN指向本机精确18.6安装；参考工具配置、version()、server_version_num及目录实查写入identity.json。服务端/客户端UTF8，array_nulls=on、standard_conforming_strings=on、DateStyle=ISO, MDY、TimeZone=Asia/Shanghai。

最终原始证据目录：`target/pg05-array-bool-evidence/27233-51550`。Rust测试启动CreateAnalyzeSession/ConcreteSessionDriver同Server双协议内存夹具；Python ctypes调用真实libpq，并独立initdb、启动及停止本任务参考服务。

数组libpq共870场景，其中736成功、134拒绝。合法输入覆盖IT→OT、IT→OB、IB→OT、IB→OB，声明/推断/OID0、空集、首行NULL和重复绑定；规范payload包含二维行主序、默认/负下界、六维、空数组、外层NULL、单个/全元素NULL。所有合法bool唯一前缀、大小写和两端空白均取证；全部256个binary单字节逐一验收规范化结果。flags非规范值及零维长输入规范化输出。每条记录包含原始载荷hex、参数/结果OID、typlen、typmod、格式、值及NULL，成功SQLSTATE为N/A。

独立普通标量binary差分共518场景（256个单字节、NULL、空载荷、多字节，分别text/binary结果），确保数组委派的入口本身真实可用。非法输入无DataRow，错误后SELECT 1::int4恢复。

双端TCP共340场景：72个合法portal执行与268个错误及恢复，补证libpq不暴露的statement/portal Describe和Bind阶段。数组拒绝项含非法布尔文本、空串、歧义o、数字2、quoted NULL、合法首元素后非法元素、UTF8、矩形/前缀、维数/flags/OID、shape乘积/下界溢出、所有头截断、长度头/载荷不足、长度<-1及尾随字节。18.6实际诊断与AsterSQL逐项相同：

| SQLSTATE | libpq数组拒绝数 |
| --- | ---: |
| 08P01 | 52 |
| 22P02 | 32 |
| 22P03 | 24 |
| 54000 | 18 |
| 22021 | 4 |
| 42804 | 4 |

每个TCP拒绝精确断言ErrorResponse→ReadyForQuery，无DataRow；错误后流水线CREATE被丢弃，Sync后SELECT 1::int4恢复，再查询被丢弃的表为42P01，证明无副作用。mismatches为空。MySQL前/中/后三次JSON/VARCHAR/SQL NULL值、列定义和原始行载荷一致。

Python finally与Rust guards清理prepared、portal、客户端、本任务服务和临时数据库并等待子进程结束，reference_directory_removed/reference_port_closed/clients_closed/raw_clients_closed及Rust数据库删除/双端口关闭全部true。共享参考工具安装保留；本测试不代表RealTiKV或持久化能力。

## 验证与风险

使用Ready：Rust格式、跨crate聚焦行为回归及make lint。verify-profile技能不存在，按根AGENTS和testing-flow执行。只改Rust、测试和说明，没有Go/import/依赖/Bazel变动且非新工作区，不触发bazel_prepare；未运行bazel_lint_changed、Playground或RealTiKV。

Cargo阶段动态依次原子mkdir领取slot-1，绝对CARGO_TARGET_DIR为 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；make lint并行独立阶段领取slot-2，绝对路径 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。所有Cargo及可能调用Cargo的脚本均继承所属槽位环境；EXIT/INT/TERM trap清理自己的owner和锁，保留缓存。

全部命令在仓库根执行，默认features，`PG18_BIN=/tmp/pg05-task14-reference/install/bin`。最终输出 `/tmp/pg05-bool-ready-final.log`，55项有效Rust测试、0失败；先前同输入红灯及绿灯另见上文日志。

| 最终命令 | 退出码 / 有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array_bool -- --nocapture --test-threads=1` | 0 / 9（server3、session3、parser1、parser-types1、types1） |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array -- --skip pg05_array_bool --nocapture --test-threads=1` | 0 / 34 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types --lib pg05_int4_array -- --nocapture --test-threads=1` | 0 / 8 |
| `cargo test -p astersql-server --lib pg_scalar_01 -- --nocapture --test-threads=1` | 0 / 1 |
| `cargo test -p astersql-server --lib pg_scalar_02 -- --nocapture --test-threads=1` | 0 / 1 |
| `cargo test -p astersql-server --lib pg_scalar_03 -- --nocapture --test-threads=1` | 0 / 1 |
| `cargo test -p astersql-server --lib pg_introspection_parameters_array_metadata -- --nocapture --test-threads=1` | 0 / 1 |
| `make lint` | 0（日志 `/tmp/pg05-bool-lint.log`；只检查Go/dashboard，后续Rust分支及测试更改不影响其覆盖，按根规则复用） |
| `python3 -m py_compile pkg/server/testdata/pg05_array_bool_libpq.py` | 0 |
| `git diff --check` | 0 |

自审覆盖共享scalar入口没有被数组回调复制、规范Bool身份/typed NULL、跨族未开放门禁、其他已开放数组回归、PingCAP许可和AsterSQL版权、各生产说明同步及资源释放。未增加Cargo依赖、features、local patch或临时依赖副本。

正确性风险限于未覆盖输入组合；兼容性仅承诺该18.6构建和UTF8配置，不承诺其他标量族、完整cast矩阵、目录专用bool binary入口或存储。性能仅小载荷行为验证，无吞吐/超大分配基准。未验证RealTiKV、持久化、32位平台及其他PostgreSQL版本。

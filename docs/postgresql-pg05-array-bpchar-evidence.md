# bpchar数组的PostgreSQL 18.6差分证据

任务34依赖任务33完整提交 `00aae9198666374db96b41fd5c2e24942e3f4c28` 与保留的varchar数组证据。标量1042复用已有text/binary参数codec及 `pg_text_cast`；按Unicode字符计数截断，按目标字符长度补U+0020，未指定长度保留原始尾空格。只开放本族数组1042/1014，不扩建标量子系统。

## 必要修改

parser/types/field_type.rs增加Bpchar元素身份与数组OID，长度恢复使用已有CHAR(n)语法，裸BPCHAR[]保留无长度。parser_actions/expression.rs复用已有1042标量声明并保留flen。grammar/main.astergram增加PgCastType : PgPostfixText [ ]，把既有CHAR/CHARACTER(n)标量声明接到数组规则，与先前REAL数组接法相同；generated/main_tables.rs用parsergen generate再生，未手工修改自动机。

types/pg_array.rs允许1042规范容器，复用Text(String)载体，元素OID/typmod独立保留。session/runtime/pg_array.rs在分析、构造和同族逐元素cast注册Bpchar，调用标量转换；control.rs/planning.rs保留prepared参数及typed NULL身份，query.rs/relational_value.rs把已有消费者接到共享数组内核。server/pg_extended.rs注册text/binary数组输入，逐元素委托1042标量codec；pg_result.rs接入数组输出和n+4结果typmod。维数、行主序、下界及两层NULL沿既有容器保存。

以上均是本族闭环必要局部接线。11个生产Rust文件、对应11个.rs.md、1个Rust专属grammar、6个独立测试文件、1个Python脚本及本文，共31个文件。既有.rs.md已登记docs/docs-manifest.md，无新增生产文件、Cargo依赖或本地覆盖。此前8个server拒绝门禁和2个types门禁的样例移到未开放族，保留拒绝、恢复和容器校验意图。

独立测试文件为pkg/parser/parser_test.rs、pkg/parser/types/field_type_test.rs、pkg/types/pg_array_test.rs、pkg/session/runtime/pg_array_test.rs、pkg/server/pg_extended_test.rs、pkg/server/pg_client_integration_test.rs；新增脚本pkg/server/testdata/pg05_array_bpchar_libpq.py。11个生产Rust文件的同名.rs.md均同步更新。文法为pkg/parser/grammar/main.astergram，生成表为pkg/parser/generated/main_tables.rs；其余生产落点均在上文逐项列明。

## 测试驱动证据

新增 `pg05_array_bpchar` server过滤器首次成功编译，2项实际运行，标量codec通过，数组测试在Parse收到0A000而非ParseComplete；退出101，日志 `/tmp/pg05-bpchar-red.log`。这是真实行为红灯，编译失败不计入。后续WIP暴露长度数组缺少[]文法接线，按上述局部规则修复。MySQL隔离fixture保持原VARCHAR行为。

## 当前行为证据

长度差分还发现ARRAY['a'::char(2),''::char(2)]的值正确而typmod从6丢到-1，现场断言为reference metadata=[[1014,-1,6,0]]、Aster=[[1014,-1,-1,0]]，两端rows同为7b226120222c222020227d。修复用constructor_field_type逐叶检查共同长度：同长度、嵌套及typed NULL保留；不同长度或未定型NULL为-1。普通推断与prepared描述共用该函数，没有删除原断言。

最终证据目录target/pg05-array-bpchar-evidence/231-58627包含identity.json、cases.jsonl、summary.json、三次MySQL快照和rust-cleanup.json。现场参考version()/server_version_num为PostgreSQL18.6/180006，aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位；libpq180000。配置UTF8、array_nulls=on、standard_conforming_strings=on、ISO/MDY、Asia/Shanghai。pg_type实查1042/1014、typelem1042、typlen=-1。

460个libpq场景包含96个长度场景，全部文本/二进制输入与文本/二进制输出四组合。覆盖声明/推断/OID0、空集/首行NULL、重绑定、数组NULL/元素NULL/全NULL、空数组、二维及六维、负下界、Unicode/组合字符、空串、NULL字样、引号/反斜线/花括号。原始输入输出十六进制、参数和结果OID1014、binary头元素1042、typlen=-1、无长度typmod=-1/长度2为6、格式及NULL逐项比对。é😀x截成é😀；空串补成两个U+0020；a、é、😀各补一个U+0020；尾空格保留或按长度截断。成功SQLSTATE为N/A。

19个SQL场景补构造、默认char[]/character[]长度1、CAST、同族连续长度转换、嵌套/typed NULL及共同长度，非法长度0/10485761在NULL与WHERE false也拒绝22023并恢复。另6个prepared构造器Describe/Execute场景检查同长度、typed NULL与混合长度的共同typmod及文本/二进制输出。

396个双端TCP场景补statement/portal Describe、格式和typmod，包括160个成功Execute及236个Bind拒绝/恢复。libpq拒绝场景现场SQLSTATE为22P02(18)、22021(8)、54000(18)、22P03(20)、42804(4)、08P01(50)。覆盖先合法后非法元素、编码/NUL、词法/矩形、头/元素长度截断、错误元素OID/flags、维数/下界/乘积溢出及尾随字节。每次Bind拒绝无DataRow，流水线CREATE被丢弃，Sync后SELECT1恢复，再查该表返回42P01，证明无副作用。

summary.passed=true、mismatches=[]；同Server MySQL JSON/VARCHAR/NULL的值、列类型和raw行字节在前/中/后三次相同。Python finally与Rust guard关闭prepared/portal、客户端、任务自有服务，等待参考进程结束并清除临时数据库目录。clients_closed/raw_clients_closed/reference_port_closed/reference_directory_removed及Rust database_dropped/postgres_port_closed/mysql_port_closed全部true；未操作用户服务或数据。最终阶段槽位1自己的owner/锁已释放，缓存保留。

## Ready验证

所有Cargo命令经bash /tmp/pg05-bpchar-cargo.sh动态原子领取槽位1，继承CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1及PG18_BIN=/tmp/pg05-task14-reference/install/bin；结束trap清除自己owner/锁，保留缓存。Rust修改后先cargo fmt --all。采用Ready交付行为与协议变更；verify-profile技能缺失，直接遵守根AGENTS与testing-flow。未改Go/import/依赖/Bazel，非新工作区，不触发bazel_prepare；Rust专属grammar由parsergen生成，不运行Go yacc生成。未运行bazel_lint_changed。

以下命令均从仓库根目录执行，Cargo命令继承上述环境。

| 确切命令 | 退出码与有效数量 | 日志 |
| --- | --- | --- |
| cargo fmt --all | 0 | /tmp/pg05-bpchar-fmt4.log |
| cargo fmt --all -- --check | 0 | /tmp/pg05-bpchar-ready-fmt.log |
| cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_bpchar -- --nocapture --test-threads=1 | 0，7项：server3/session1/parser1/parser-types1/types1 | /tmp/pg05-bpchar-ready-family.log |
| cargo test -p astersql-server --lib pg05_array_bpchar -- --nocapture --test-threads=1 | 0，3项 | /tmp/pg05-bpchar-ready-server.log |
| cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --skip pg05_array_bpchar --nocapture --test-threads=1 | 0，106项：server29/session25/parser9/parser-types10/types33 | /tmp/pg05-bpchar-ready-regression.log |
| cargo test -p astersql-server --lib pg_extended_test::pg_scalar_21 -- --nocapture --test-threads=1 | 0，1项 | /tmp/pg05-bpchar-ready-scalar21.log |
| cargo test -p astersql-server --lib pg_extended_test::pg_scalar_22 -- --nocapture --test-threads=1 | 0，1项 | /tmp/pg05-bpchar-ready-scalar22.log |
| cargo test -p astersql-server --lib pg_extended_test::pg_scalar_23 -- --nocapture --test-threads=1 | 0，1项 | /tmp/pg05-bpchar-ready-scalar23.log |
| cargo run --release -p astersql-parsergen --bin astersql-parsergen -- generate | 0 | /tmp/pg05-bpchar-generate.log |
| cargo run --release -p astersql-parsergen --bin astersql-parsergen -- check | 0 | /tmp/pg05-bpchar-parser-check.log |
| cargo test -p astersql-server --lib pg05_array_bpchar -- --nocapture --test-threads=1 | 0，3项，含最终新增prepared脚本场景 | /tmp/pg05-bpchar-final-server.log |
| make lint | 0，适用Go/dashboard lint | /tmp/pg05-bpchar-lint.log |
| python3 -m py_compile pkg/server/testdata/pg05_array_bpchar_libpq.py | 0 | 本轮命令 |
| git diff --check | 0 | 本轮命令 |

116项不同Rust行为测试通过，server重复运行及差分JSON场景不重复计数。lint检查对象为Go/dashboard，本轮后续仅Rust改动，复用该适用检查，不重复运行无关广扫。

## 风险与后续

正确性限于本族已覆盖闭环；跨族数组转换仍明确0A000，未实现完整数组赋值/存储及隐式转换矩阵。长度语法复用CHAR(n)/CHARACTER(n)，未扩展既有标量bpchar(n)别名解析。这些范围外缺口不递归扩写。兼容性证据限此18.6构建与UTF8配置，不推广其他平台、版本或族。未做吞吐、超大数组或性能基准，未验证RealTiKV/持久化及32位。建议继续独立批次35的date数组。

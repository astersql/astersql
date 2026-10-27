# float8数组与原始binary64位型的PostgreSQL18.6证据

任务32依赖批次31完整SHA `d5722b05f2fa3628f58770c1f27736f2f7046142`，已核对其18.6四方向、位型桥及Ready证据。用户授权补齐float8同类必要浮点位型接口；复用既有数组容器和批次31私有AST绑定桥，不扩展存储、算术、完整隐式转换矩阵或外部依赖。

## 行为与必要落点

精确18.6参考目录实测元素OID701/typlen8/typelem0，数组OID1022/typlen-1/typelem701。FieldType、Parse/Bind、参数推导、typed NULL、空集/首行NULL、Describe/Execute及重绑定均保留本族身份，元素typmod与结果typmod为-1。PgArrayElement::Float8(u64)保存原始IEEE64位型；binary输入输出不计算或规范化NaN，保留sign/payload/signalling、±0、次正规与有限边界。text出口调用共享float_text有限值格式算法，NaN/±Infinity按PG规范显示。shape/lower/元素NULL/外层SQL NULL分别保留，非法任一元素拒绝整个值。

生产修改共12个Rust文件，各同步现有.rs.md：

| 文件 | 本闭环不可缺少的接线依据 |
| --- | --- |
| `pkg/parser/types/field_type.rs` | Float8元素身份、OID与FLOAT8[]恢复 |
| `pkg/parser/parser_actions/expression.rs` | 已有double precision/float8/float(p>24)声明接入数组；无需改grammar或生成表 |
| `pkg/types/pg_array.rs` | 规范Float8(u64)、共享8字节标量接收与文本输出、数组编码 |
| `pkg/server/pg_extended.rs` | OID1022 Parse门禁、四方向参数codec、私有float8标签；标量与元素共用float8_binary_input |
| `pkg/server/pg_result.rs` | 已定型701/1022 Bytes结果直接输出binary，text才调用共享输出codec |
| `pkg/server/runtime.rs` | 私有BinaryParam标签恢复Float8Bits/Float8Array，MySQL原参数标签不变 |
| `pkg/session/runtime/session.rs` | 私有Float8Bits(u64)/Float8Array参数变体，不渲染成公开SQL字符串 |
| `pkg/session/runtime/control.rs` | 1022参数身份/约束/typed NULL，安全CAST(NULL AS FLOAT8/FLOAT8[])占位按offset恢复私有AST；复用原状态还原及失败清理 |
| `pkg/session/runtime/planning.rs` | 数组参数约束与1022声明 |
| `pkg/session/runtime/pg_array.rs` | 分析/定型/构造/同族cast；Float4/Float8共用矩形shape收集、分别调用标量codec，全部成功后发布 |
| `pkg/session/runtime/query.rs` | 真实共享查询消费者保持Float8[]与标量位型 |
| `pkg/session/runtime/relational_value.rs` | 另一执行消费者一致；同族保位、语义text cast才规范化NaN，普通公开文本不能伪造内部载体 |

独立测试6文件为parser/parser_test.rs、parser/types/field_type_test.rs、types/pg_array_test.rs、session/runtime/pg_array_test.rs、server/pg_extended_test.rs、server/pg_client_integration_test.rs。新增Python真实libpq脚本 `pkg/server/testdata/pg05_array_float8_libpq.py` 与本文；总计32文件，现存.rs.md已经在docs/docs-manifest.md登记，无新增生产文件或依赖。

## 红灯与差分覆盖

初始指定server命令运行3项，退出101：1通过2失败。短binary标量实际22P03期望08P01；真实TCP输入7ff812345678abcd实际返回7ff8000000000000。日志 `/tmp/pg05-float8-red.log`，不是编译失败或零测试。修复后相同断言通过；标量与数组元素统一不足8字节08P01、超过8字节22P03。

真实libpq矩阵固定PostgreSQL18.6/180006，工具 `/tmp/pg05-task14-reference/install/bin`；实际aarch64-apple-darwin25.1.0/Apple clang16/64-bit、libpq180000、UTF8，array_nulls=on、extra_float_digits=1、standard_conforming_strings=on。identity.json记录完整version、构建参数、目录与配置。所有成功SQLSTATE写N/A；拒绝保存实测阶段与SQLSTATE，不将缺失字段默认相等。

360数组/libpq场景包含200次四方向Bind/Execute/rebind（声明/推导/OID0/空集/首行NULL × 默认/多维/负下界/空数组/元素NULL/6维/浮点边界/数组NULL）、6次非规范头规范化、12次任意NaN sign/payload/signalling位型以及142次拒绝。另有68标量四方向和68标量参数构造ARRAY四方向。边界包含±0、±最小次正规5e-324、±最小正规2.2250738585072014e-308、±最大有限1.7976931348623157e+308、NaN/±Infinity；规范载体测试另覆盖最大次正规位型。

142次libpq拒绝：22P02×26、22021×2、54000×18、22003×8、22P03×24、42804×4、08P01×60。覆盖词法、参差形状、维数/乘积/下界溢出、错误元素OID、缺头/长度/元素、超长/短元素、尾随字节及合法首元素后的非法元素。非法NUL文本由raw TCP提交完整载荷，避免libpq按NUL截断影响测试。

372双端TCP场景包含80成功portal Describe/Execute及292拒绝/Sync恢复；记录statement/portal元数据、原始输入输出、Bind错误阶段与无DataRow。Bind错误后发送CREATE并确认其被丢弃，恢复后查询表42P01证明无副作用。MySQL同Server前/中/后三次JSON/VARCHAR/NULL的值、列类型和raw行字节一致。

每次脚本自建唯一临时initdb/空闲端口，try/finally关闭libpq/raw clients、prepared和参考服务并等待结束、删除临时目录。Rust guard关闭本任务双协议服务、DROP唯一临时库，rust-cleanup.json三个字段均true。只操作自有服务/数据，不启动Playground或RealTiKV。

## 验证命令与限制

所有Cargo阶段通过 `/tmp/pg05-float8-cargo.sh` 从仓库根动态领取共享槽位1，设置 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`，全部子进程继承；阶段结束释放自己的锁、保留缓存。Rust修改后先cargo fmt --all，再行为验证。采用Ready，因为修改类型身份、共享执行及协议codec；完整命令与最终结果记录如下。

只改Rust、独立Python和文档，不触发Go/Bazel prepare；未运行bazel_lint_changed、Go failpoint或昂贵全包测试。未修改plan.md、Cargo依赖、vendor/third_party或本地patch。Ready技能路径不存在，按AGENTS.md/testing-flow执行，不声称不存在的技能通过。

风险：正确性限于已记录的表达式组合；跨族数组cast明确0A000，未承诺完整数组操作、浮点算术、存储及隐式转换矩阵。兼容性证据仅精确18.6、UTF8、64-bit与记录配置；未验证其他版本或RealTiKV。性能未基准测试，按元素数和载荷大小线性处理，既有私有字节桥可能增加hex编码分配；恶意长度边界已测，不承诺超大实际分配。

既有数组回归发现8项过时门禁：server6项以1022、types2项以701断言“未开放”，实际已正确接受。保留拒绝与错误恢复断言，将未开放样例分别改为下一批尚未开放varchar数组1015/元素1043；没有删除测试或把合法float8值改为拒绝。原日志 `/tmp/pg05-float8-regression.log`。本族跨crate最终生产代码在 `/tmp/pg05-float8-wip4.log` 运行11项全部通过（parser1/parser-types1/server4/session4/types1），其18.6证据目录 `target/pg05-array-float8-evidence/69162-53044`。


## 最终Ready结果

最终server命令的当前证据目录 `target/pg05-array-float8-evidence/70794-53890`：summary.passed=true、reference_version_num180006、mismatches=[]，360数组/libpq、68标量、68构造器、372双端TCP、12任意NaN位型场景。rust-cleanup.json的database_dropped/postgres_port_closed/mysql_port_closed全部true，参考端目录/端口与全部客户端清理字段也全部true；自己的slot-1.lock已释放，缓存保留。

以下命令均在仓库根、前述绝对CARGO_TARGET_DIR及动态槽位下运行。含真实差分的命令设置 `PG18_BIN=/tmp/pg05-task14-reference/install/bin`；默认features，无新增开关。

| 精确命令 | 退出码/有效测试数 | 日志 |
| --- | --- | --- |
| `cargo fmt --all` | 0，Rust修改后先执行 | `/tmp/pg05-float8-fmt5.log` |
| `cargo fmt --all -- --check` | 0 | `/tmp/pg05-float8-ready-fmt.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_float8 -- --nocapture --test-threads=1` | 0，11项：server4/session4/parser1/parser-types1/types1 | `/tmp/pg05-float8-wip4.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server --lib pg05_array_float8 -- --nocapture --test-threads=1` | 0，4项 | `/tmp/pg05-float8-ready-server.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --skip pg05_array_float8 --nocapture --test-threads=1` | 0，88项：server22/session20/parser7/parser-types8/types31 | `/tmp/pg05-float8-ready-array-regression.log` |
| `cargo test -p astersql-server --lib pg_extended_test::pg_scalar_1 -- --nocapture --test-threads=1` | 0，8项 | `/tmp/pg05-float8-ready-scalar.log` |
| `make lint` | 0，适用Go/dashboard lint | `/tmp/pg05-float8-lint.log` |
| `PYTHONPYCACHEPREFIX=/tmp/pg05-float8-pycache python3 -m py_compile pkg/server/testdata/pg05_array_float8_libpq.py` | 0 | Ready脚本执行 |
| `git diff --check` | 0 | 自审通过 |

共107项不同Rust测试通过，不重复计数server重跑或JSON内差分场景。本族跨crate最终生产代码检查后只调整旧门禁测试样例；后续完整88项回归及server Ready均已覆盖。make lint检查内容未受随后Rust专属改动影响，复用有效结果。未运行两个需要外部DSN的旧标量libpq测试pg_scalar_14/15、RealTiKV/存储、其他版本或性能基准，不将其报告为通过。没有尚缺的本任务核心行为或适用Ready证据；后续建议继续独立任务33的varchar数组。

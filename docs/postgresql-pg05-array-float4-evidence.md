# float4数组与原始binary32位型的PostgreSQL18.6证据

任务31依赖批次30完整SHA `dffe8eba9695391056fd601ac6cf68d29e742a18`；已核对numeric数组当前证据与接口。用户明确授权补齐必要浮点位型接口，并允许本族规范载体/parser/session最小接线。只实现float4，不提前实现任务32的float8，不修改Datum布局、存储或外部依赖。

## 交付行为和必要接线

元素OID700、数组OID1021；精确18.6目录返回700/typelem0/typlen4及1021/typelem700/typlen-1。FieldType、参数约束、typed NULL、Prepare/Describe/重绑定和两个共享cast消费者保留本族身份。PgArrayElement::Float4(u32)保存原始IEEE位型，binary输入输出完全保留NaN sign/payload/signalling、±0、次正规和有限边界；text出口按PostgreSQL格式规范输出NaN/±Infinity及finite shortest text。shape/lower/元素typmod-1、元素NULL和外层SQL NULL分别保留，任意非法元素拒绝整值。

既有BinaryParam能保存4字节，但标量参数先提升到f64、渲染成CAST('NaN' AS FLOAT4)，结果又先to_text再parse，导致原始NaN位型丢失。内部参数标签PG_FLOAT4_PARAMETER/PG_FLOAT4_ARRAY_PARAMETER分别生成Float4Bits(u32)/Float4Array(Vec<u8>)，通过连接私有AST常量在绑定后的offset恢复。公开SQL仅包含安全CAST(NULL AS ...)占位符，无“特殊NaN文本”语法；私有状态执行后恢复，失败也清理。结果复用既有binary字节桥，只有binary出口直接发原始位型，text出口才格式化。不修改MySQL既有type4路径，不把已经丢失的payload猜回来。

共享float4_binary_input只复制4字节：不足4字节08P01，超过4字节22P03；server标量与数组元素均复用。既有finite float_text算法从server提取到types/pg_array供标量/数组共用，没有复制另一套解析或格式算法。构造器直接收集规范元素及矩形shape后发布，避免逐元素text中转。数组普通text输入继续调用既有pg_cast_float4；普通SQL字符串不能伪造内部binary载体。

生产落点及依据：parser field_type/expression负责本族声明/恢复；types pg_array负责规范位型与共享codec/输出formatter；server pg_extended/runtime/pg_result负责typed参数和原字节结果；session session/control/dispatch负责私有AST绑定生命周期，planning负责参数身份，pg_array负责规范值及构造器，query/relational_value同步两个实际执行消费者。全部生产Rust同步.rs.md、保留许可与AsterSQL版权。Rust grammar的REAL/FLOAT(p)数组后缀规则复用既有PgArrayType动作，并由parsergen生成main_tables；float8仍受未开放族门禁限制。

## 红灯与修复证据

初始 `cargo test -p astersql-server --lib pg05_array_float4 -- --nocapture --test-threads=1` 编译成功、运行2项、退出101：空binary标量载荷实际22P03/期望08P01；真实TCP OID700 binary输入7fc12345实际DataRow返回7fc00000。原始日志 `/tmp/pg05-float4-red.log`，不是编译错误或零测试。18.6参考探针同时证明7fc00000/7fc12345/ffc12345/7fa12345均原位返回，短载荷08P01、超长22P03，日志 `/tmp/pg05-float4-reference.log`。授权后同两个回归通过，并追加超长/NULL/-0与私有状态/公开字符串隔离测试。

补充SQL构造器测试在 `SELECT ARRAY[]::real[]` 处暴露本地语法缺口，wip5日志保留实际失败；没有删除断言。新增一条PgCastType : PgPostfixFloat [ ]语法并重新生成Rust表，补充real[]/float(24)[]解析回归。

一次WIP未设置PG18_BIN，测试实际发现本机18.4并明确失败/清理，未将其作为18.6证据。wip2/wip3脚本双端均零差异，但Rust包装仍保留复制模板的旧348场景数；修正计数为本族360/372，未改行为期望或减少断言。

## 真实差分矩阵

已通过的原始证据目录 `target/pg05-array-float4-evidence/45117-62399`；最终Ready重跑的目录和命令在下节补录。summary.passed=true、mode=differential、reference_version_num180006、mismatches=[]。参考工具 `/tmp/pg05-task14-reference/install/bin`，实际PostgreSQL18.6/aarch64-apple-darwin25.1.0/Apple clang16/64-bit，libpq180000、UTF8，array_nulls=on、extra_float_digits=1、standard_conforming_strings=on。version、构建参数、编码、目录和会话配置保存在identity.json；不是18.4或reference-only证据。

360数组/libpq场景由200次四方向Bind/Execute/rebind（declared/inferred/OID0/零行/首行NULL × 默认/多维/负下界/空数组/全NULL/6维/浮点边界/外层NULL）、6次非规范header规范化、12次非规范NaN sign/payload/signalling数组binary位型和142次拒绝构成。另有68标量四方向和68标量参数构造ARRAY四方向（包含4种非规范NaN位型的IB→OB原样及IT规范NaN），记录原始载荷、参数OID、结果OID/typlen/typmod/格式和值/NULL，成功SQLSTATE写N/A。

142次libpq拒绝实测SQLSTATE：22P02×26、22021×2、54000×18、22003×8、22P03×26、42804×4、08P01×58。覆盖词法/矩形/维度、非法第二个标量、溢出/下溢、错误元素OID、恶意长度/头截断/元素不足/尾随字节/短或超长float4元素。两端一致，无DataRow，随后SELECT1恢复。文本NUL由raw TCP提交完整载荷，未将libpq按NUL截断的请求冒充完整测试。

372双端TCP场景由80成功portal Describe/Execute与292拒绝/Sync恢复组成，补证statement/portal元数据、Bind错误阶段、丢弃直至Sync、无DataRow。错误后发送的CREATE被丢弃，恢复后查询对应表42P01，证明没有副作用。MySQL同Server前/中/后三次JSON/VARCHAR/NULL值、列类型和raw行字节相同；另有既有标量TCP float值与MySQL隔离回归。

每次脚本自建唯一临时initdb与空闲端口，finally关闭libpq/raw clients、prepared和参考服务并等待退出，删除临时目录。Rust guard关闭本任务双协议服务并DROP唯一临时库，rust-cleanup.json的database_dropped/postgres_port_closed/mysql_port_closed均true。未操作用户数据库或服务，未启动RealTiKV/Playground。

## 验证与边界

所有Cargo阶段动态领取仓库共享槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；owner保存任务/chat/PID，阶段结束释放自己的锁、保留缓存。使用 `/tmp/pg05-float4-cargo.sh` 动态领取后执行下列Cargo命令，所有子进程继承同一绝对路径。

已完成检查：

- `cargo fmt --all`：退出0，每次Rust修改后先格式化。
- `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_float4 -- --nocapture --test-threads=1`：wip4退出0，7项通过（新增边界/别名后最终数量待下文记录）；日志 `/tmp/pg05-float4-wip4.log`。
- 同上crate集合运行 `cargo test ... --lib --no-fail-fast pg05_array -- --skip pg05_array_float4 --nocapture --test-threads=1`：退出0，parser6/parser-types7/server18/session17/types30，共78项既有数组测试通过，日志 `/tmp/pg05-float4-array-regression.log`。
- `cargo test -p astersql-server --lib pg_scalar_1 -- --nocapture --test-threads=1`：退出101，8项TCP测试pg_scalar_10/11/12/13/16/17/18/19通过；旧外部客户端pg_scalar_14/15因缺少PG18_REFERENCE_DSN在启动前失败。后两项超出本任务float4数组验收，未配置外部DSN、未伪称通过，日志 `/tmp/pg05-float4-scalar-regression.log`。
- `make lint`：退出0，日志 `/tmp/pg05-float4-lint.log`。
- `PYTHONPYCACHEPREFIX=/tmp/pg05-float4-pycache python3 -m py_compile pkg/server/testdata/pg05_array_float4_libpq.py`：退出0。

Ready最终检查将追加于下文。只改Rust/其专属grammar、独立测试/Python和文档，无Go/Bazel触发项，不需要bazel_prepare。Rust专属解析表用parsergen generate/check验证，不改或生成Go parser。未运行bazel_lint_changed、Go failpoint、真实TiKV或性能基准。

正确性风险限于未覆盖表达式组合；原位bit承诺覆盖本族参数与数组共享消费者，不扩展其他浮点族、算术或存储。跨族数组cast、其他未开放族仍按既有范围门禁拒绝；不承诺完整隐式转换矩阵。性能未基准测试，规范数组及现有字节桥按元素数/载荷大小线性处理并可能额外编码hex；恶意尺寸/长度已测试，不承诺超大实际分配。兼容性证据限定精确18.6/UTF8/64-bit与记录的会话配置。


## 精确文件清单

14个生产Rust文件各同步同目录`.rs.md`（28文件）：

- `pkg/parser/generated/main_tables.rs` 与 `pkg/parser/generated/main_tables.rs.md`。
- `pkg/parser/parser_actions/expression.rs` 与 `pkg/parser/parser_actions/expression.rs.md`。
- `pkg/parser/types/field_type.rs` 与 `pkg/parser/types/field_type.rs.md`。
- `pkg/server/pg_extended.rs` 与 `pkg/server/pg_extended.rs.md`。
- `pkg/server/pg_result.rs` 与 `pkg/server/pg_result.rs.md`。
- `pkg/server/runtime.rs` 与 `pkg/server/runtime.rs.md`。
- `pkg/session/runtime/control.rs` 与 `pkg/session/runtime/control.rs.md`。
- `pkg/session/runtime/dispatch.rs` 与 `pkg/session/runtime/dispatch.rs.md`。
- `pkg/session/runtime/pg_array.rs` 与 `pkg/session/runtime/pg_array.rs.md`。
- `pkg/session/runtime/planning.rs` 与 `pkg/session/runtime/planning.rs.md`。
- `pkg/session/runtime/query.rs` 与 `pkg/session/runtime/query.rs.md`。
- `pkg/session/runtime/relational_value.rs` 与 `pkg/session/runtime/relational_value.rs.md`。
- `pkg/session/runtime/session.rs` 与 `pkg/session/runtime/session.rs.md`。
- `pkg/types/pg_array.rs` 与 `pkg/types/pg_array.rs.md`。

独立测试6文件：`pkg/parser/parser_test.rs`、`pkg/parser/types/field_type_test.rs`、`pkg/server/pg_extended_test.rs`、`pkg/server/pg_client_integration_test.rs`、`pkg/session/runtime/pg_array_test.rs`、`pkg/types/pg_array_test.rs`。另有 `pkg/parser/grammar/main.astergram`、`pkg/server/testdata/pg05_array_float4_libpq.py`、本文，共37文件。所有现存.rs.md已经在docs/docs-manifest.md登记，未新增生产文件，故清单无需改动。


新增位型桥的语义cast回归另获当前红灯：`cargo test -p astersql-session --lib pg05_array_float4_scalar_cast -- --nocapture --test-threads=1`运行1项退出101，typed OID700参数cast为text时暴露内部binary标记，日志 `/tmp/pg05-float4-cast-red.log`。修复仅在真实Float4/Float4[]源身份的既有cast边界调用共享输出codec转换为规范text，同族cast继续保位，未新增跨族转换规则；同一回归及数组text消费者随后通过。对应两个共享消费者query/relational_value一致，普通未定型文本不会按前缀解码。

最终本族跨crate命令（Rust修改后先cargo fmt --all）：

`PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_float4 -- --nocapture --test-threads=1`

退出0，共10项不同Rust行为测试通过：parser1/parser-types1/server4/session3/types1。日志 `/tmp/pg05-float4-wip7.log`。覆盖修复后真实NaN位型、短/超长binary、NULL/-0、私有绑定错误恢复、公开字符串隔离、别名/构造器和重复cast/text消费者。未将JSON内的场景当成额外Rust测试。

Rust专属parsergen调试构建在约5分钟计算后由本任务显式TERM终止（退出143，尚未写文件）；改用 `cargo run --release -p astersql-parsergen --bin astersql-parsergen -- generate` 退出0。优化构建只影响生成器工具性能，不改生成算法、features或server测试profile；main_tables约5897行机械差异全部由一条新增语法重排生成表，非手工改表。日志 `/tmp/pg05-float4-parser-generate-release.log`，之后再次cargo fmt --all退出0。


## 最终Ready结果

最终当前证据目录：`target/pg05-array-float4-evidence/58545-50474`。重新核对summary与rust-cleanup全部成功，参考180006、360数组/libpq、68标量、68构造器、12非规范NaN、372双端TCP、mismatches=[]。先前目录仅为WIP证据；本目录是最终server命令的实际产物。

采用Ready，因为本任务修改类型身份、共享执行和协议codec。所有最终命令退出0：

| 精确命令（仓库根、共享槽位环境） | 有效Rust测试数/检查 | 日志 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | 格式通过 | `/tmp/pg05-float4-ready-fmt.log` |
| `cargo run --release -p astersql-parsergen --bin astersql-parsergen -- check` | 当前语法与生成表一致 | `/tmp/pg05-float4-parser-check.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server --lib pg05_array_float4 -- --nocapture --test-threads=1` | 4通过 | `/tmp/pg05-float4-ready-server.log` |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --skip pg05_array_float4 --nocapture --test-threads=1` | 78通过，6/7/18/17/30 | `/tmp/pg05-float4-ready-array-regression.log` |
| `cargo test -p astersql-server --lib pg_extended_test::pg_scalar_1 -- --nocapture --test-threads=1` | 8通过 | `/tmp/pg05-float4-ready-scalar-regression.log` |
| `make lint` | 适用Go/dashboard lint通过 | `/tmp/pg05-float4-lint.log` |

结合最终跨crate本族10项，共96项不同Rust测试通过；不重复计数server重跑或JSON内场景。最终阶段退出0，自己的slot-1.lock已释放，缓存保留。自审`git diff --check`通过，37文件均为本任务闭环，其他任务状态/删除不纳入提交；plan.md未修改。无Cargo manifest/依赖变化、vendor、third_party或本地patch。

本机验证限制仍为上述两个旧外部DSN客户端测试、RealTiKV/存储/其他版本及性能基准，均不冒充通过。任务31核心四方向/位型/NULL/恢复、当前18.6行为与适用Ready检查已满足，下一步为独立任务32复用接口实现float8。

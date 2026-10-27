# UUID 数组 PostgreSQL 18.6 差分证据

任务40依赖任务39提交 `1cc59597e10e7b48c2ccb8fccf59f3f3811c8fcf` 及其保留的18.6/Ready报告。UUID标量复用PgUuid、既有2950参数双向codec和标量任务1–5的独立身份；本轮另运行当前标量回归。仅开放元素2950/数组2951，不修改其他标量算法。

## 修改及局部接线依据

11个生产Rust文件及对应11个.rs.md：pkg/types/pg_array.rs增加Uuid(PgUuid)规范元素并保留16字节原序；pkg/parser/types/field_type.rs增加2950/2951及UUID[]恢复；pkg/parser/parser_actions/expression.rs沿既有标量/数组语法注册本族，无语法规则或生成表变化。pkg/session/runtime/pg_array.rs复用PgUuid解析进行元素推断、ARRAY构造和同族cast；control.rs/planning.rs保存声明、推断参数、typed NULL及绑定时UUID[] cast；query.rs/relational_value.rs/system_query.rs接通SELECT和常量UNION的已有消费者。pkg/server/pg_extended.rs注册Parse/Bind本族分派，text元素委托PgUuid、binary元素委托2950 codec；pg_result.rs将2951接到规范数组结果codec及元数据。

6个独立测试文件：pkg/parser/parser_test.rs、pkg/parser/types/field_type_test.rs、pkg/types/pg_array_test.rs、pkg/session/runtime/pg_array_test.rs、pkg/server/pg_extended_test.rs、pkg/server/pg_client_integration_test.rs。新增testdata/pg05_array_uuid_libpq.py及本文，共30个文件。server既有11项未开放数组门禁从2951迁至仍未开放的199（json[]）；types既有未开放族反例从2950迁至仍未开放的114，保留空/NULL/非法头及拒绝意图。上述均为UUID数组单一闭环所需；没有新增生产源文件或docs-manifest条目，没有递归移植标量子系统。

## TDD和参考端

`cargo fmt --all`后首次运行`cargo test -p astersql-server --lib pg05_array_uuid -- --nocapture --test-threads=1`：编译成功，2项实际测试中标量前置通过，数组入口以0A000拒绝2951，退出101；日志`/tmp/pg05-uuid-array-red.log`。后续首次双端验证补证prepared绑定尚缺UUID[] cast，该值在Execute被0A000拒绝（`/tmp/pg05-uuid-array-green.log`），按现有各族绑定方式补接线；保留相同输入/断言，不放宽成功语义。

中间共享数组回归（/tmp/pg05-uuid-array-ready.log）有10项旧门禁失败，149项通过；运行产物mtime为02:13:04，迁移反例的源码mtime为02:13:13，产物早于该测试更新。此轮不计最终全绿，随后冻结源码、重新fmt/编译并完整复跑。

参考PG18_BIN=/tmp/pg05-task14-reference/install/bin；现场`postgres --version`及SQL version()/server_version_num核对18.6/180006，libpq180000，UTF8、ISO/MDY、Asia/Shanghai、array_nulls=on。pg_type实查2950/typelem0/typlen16和2951/typelem2950/typlen-1。参考单独采样`/tmp/pg05-uuid-array-reference`明确passed=false，仅辅助确认参考输入/诊断，不能作为双端完成证据。

每次脚本自建/tmp下专属initdb和空闲端口；finally关闭libpq/raw TCP客户端与prepared/portal，停止并等待本任务参考服务，删除自有临时数据库目录。Rust guard关闭同Server PG/MySQL端口、删除唯一数据库。异常/SIGTERM走相同finally，不操作用户服务或数据。

## 验证范围及风险

Ready按根AGENTS.md及testing-flow选择；.agents/skills/tidb-verify-profile不存在，不报告虚构技能通过。Rust修改先cargo fmt --all；无Go源/import/顶层Go测试、Go依赖或Bazel变化，不触发bazel_prepare，未运行bazel_lint_changed。无外部Rust依赖变更、vendor/third_party或本地patch；既有PingCAP许可和11个生产文件的AsterSQL标记均保留。

正确性/兼容性限于本族和本18.6构建；完整ARRAY赋值、持久化、数组运算及跨族隐式矩阵仍不在范围。未开放合法跨族转换保持0A000。性能沿原容器O(输入与元素总大小)路径，未作吞吐/超大数组基准；未验证RealTiKV/持久化、32位和其他平台，内存夹具不称RealTiKV通过。

## 当前行为覆盖

IT→OT、IT→OB、IB→OT、IB→OB逐项比对参数OID、原始hex、结果2951/typlen-1/typmod-1/独立格式、值与两层NULL。包含259个全零、全ff、顺序字节及每位置位/清位模式（任意version/variant无额外限制）、不同合法文本拼写、默认/负下界、矩形/六维、空数组/空集、首行NULL/全NULL、typed NULL及连续重绑定。构造参数OID2950保持标量身份，结果2951；非法任一元素拒绝完整值。

真实参考诊断覆盖数组词法/矩形/边界、元素UUID词法/UTF8、binary短/长元素、头/长度截断、flags、元素OID、维数/乘积/下界溢出和尾随字节。libpq拒绝SQLSTATE计数为22P02×30、22021×4、54000×18、22P03×24、42804×4、08P01×82；TCP另测文本NUL22021。每次Bind拒绝无DataRow、丢弃其后的CREATE直到Sync，恢复SELECT1且该表查询42P01，证明无副作用。成功项SQLSTATE为N/A；缺失字段不默认相等。

MySQL同Server原协议对JSON/VARCHAR/NULL的列类型、值及raw行载荷在前/中/后三次快照一致。Python与Rust清理记录检查客户端关闭、参考端口关闭/目录删除、本任务数据库删除、PG/MySQL端口关闭。

## Ready 命令

所有命令从仓库根运行。Cargo通过`bash /tmp/pg05-uuid-array-cargo.sh`动态原子mkdir领取槽位、写入任务/会话/PID，并继承`PG18_BIN=/tmp/pg05-task14-reference/install/bin`及绝对CARGO_TARGET_DIR。该脚本正常/失败退出后删除自有owner和锁，保留编译缓存。

| 确切命令 | 当前证据 |
| --- | --- |
| `cargo fmt --all` | 退出0 |
| `cargo fmt --all -- --check` | 退出0 |
| `cargo test -p astersql-server --lib pg05_array_uuid -- --nocapture --test-threads=1` | 退出0，3项；初始红灯为退出101、2项 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_uuid -- --nocapture --test-threads=1` | 退出0，7项：server3/session1/parser1/parser-types1/types1；/tmp/pg05-uuid-array-family.log |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1` | 退出0，159项：server51/session33/parser16/parser-types17/types42 |
| `cargo test -p astersql-server --lib pg05_int4_array -- --nocapture --test-threads=1` | 退出0，2项；补独立前缀的受影响门禁 |
| `cargo test -p astersql-server --lib pg05_uuid -- --nocapture --test-threads=1` | 退出0，7项含真实18.6标量差分 |
| `cargo test -p astersql-types --lib pg_uuid -- --nocapture --test-threads=1` | 退出0，5项 |
| `make lint` | 退出0；/tmp/pg05-uuid-array-lint.log，后续仅Rust/测试及说明改动，复用适用Go/dashboard检查 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_uuid_libpq.py` | 退出0 |
| `git diff --check -- pkg docs` | 退出0 |

最终统一日志为`/tmp/pg05-uuid-array-final.log`；此前`/tmp/pg05-uuid-array-ready.log`含上述旧门禁失败，不能作全绿证据。

最终阶段整体退出0，去重173项Rust测试通过（159共享数组、2 int4、7 server UUID标量、5 types UUID）；本族7项包含在共享数组中，精确过滤器重复运行不重复累计。没有零测试、编译或旧18.4证据替代验收。

最终本族原始证据目录`target/pg05-array-uuid-evidence/18946-57262`：summary.passed=true、mismatches=[]，1392 libpq、400双端TCP、12标量参数构造、7 SQL场景均通过；上述为场景数，不是Rust测试数。所有Python/Rust清理字段为true，MySQL三次原始快照完全相同。

所有Cargo阶段实际动态领取槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。最终Cargo、Python/参考服务及槽位产物使用者全部退出后，确认自有owner与slot-1.lock已释放，缓存保留。

差异自审通过：仅30个本任务文件，排除既有编号任务状态；plan.md只读且未变。无Go/Bazel/Cargo依赖改动，无外部依赖临时副本或本地覆盖。建议继续独立任务41的json数组，本族证据不推广为json/jsonb数组支持。

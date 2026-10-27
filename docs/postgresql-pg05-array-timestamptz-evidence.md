# timestamptz数组 PostgreSQL 18.6 差分证据

任务38依赖批次37提交 `3c7add08c3ca95a9c5e1bb15c8d854155640e171` 及其18.6四方向和Ready报告。仅开放timestamptz元素1184/数组1185，复用PgTimestampTz的UTC微秒身份和codec、ConcreteSession::pg_timestamptz_input及pg_timestamptz_relational_value的会话TimeZone/DST契约。不复制标量实现或移植其他子系统。

## 本族必要局部接线

12个生产Rust文件及其对应12个.rs.md：pkg/types/pg_array.rs、pkg/parser/types/field_type.rs、pkg/parser/parser_actions/expression.rs、pkg/session/runtime/{pg_array,pg_temporal,control,planning,query,relational_value,system_query}.rs、pkg/server/{pg_extended,pg_result}.rs。

规范PgArray增加Timestamptz(PgTimestampTz)元素；parser将既有identifier目标timestamptz接入数组动作并保留p，无语法规则变化或新增生产文件，故无需重生成解析表或新增docs-manifest条目。control/planning保留1185声明参数、typed NULL和重绑定；query及常量SELECT/UNION的数组cast消费者传递会话。pg_temporal同标量在Parse冻结数组字面量输入瞬时，输出仍按Execute时活动TimeZone显示。普通结果文本验证瞬时后保留显示偏移，binary仅使用UTC codec，不重复施加偏移。

6个独立测试文件：pkg/types/pg_array_test.rs、pkg/parser/types/field_type_test.rs、pkg/parser/parser_test.rs、pkg/session/runtime/pg_array_test.rs、pkg/server/pg_extended_test.rs、pkg/server/pg_client_integration_test.rs；新增pkg/server/testdata/pg05_array_timestamptz_libpq.py及本文。共32个文件。原types两项未开放族门禁从1184改为仍未开放的1186，保持拒绝测试意图。

这些修改均对应本族规范载体、身份、上下文、四方向交接或不可缺少消费者接线。没有扩大到其他标量、完整ARRAY上下文或跨族cast矩阵。非直接SELECT下的ARRAY构造（如UNION分支）仍由既有分析门禁0A000拒绝；合法数组cast UNION已测试。

## TDD和差分方法

最初运行cargo fmt --all后，`cargo test -p astersql-server --lib pg05_array_timestamptz -- --nocapture --test-threads=1`：编译成功，2项有效测试中标量前置通过，1185数组入口以0A000拒绝，退出101；日志/tmp/pg05-timestamptz-red.log。后续相同断言通过，并扩展为3项server测试。

真实差分首次暴露常量SELECT消费者未传递会话：无显式偏移的timestamptz数组构造被0A000拒绝（/tmp/pg05-timestamptz-diagnostic.log）。补system_query本族分派后，同一输入及断言通过。中间跨crate夹具修正：Restore期望误留TIMESTAMP；另一个新增ARRAY构造UNION场景超出既有直接SELECT分析范围，改为合法数组cast UNION；未放宽门禁。以上中间失败不作最终Ready通过证据。

参考PG18_BIN=/tmp/pg05-task14-reference/install/bin；现场postgres --version、version()/server_version_num核对18.6/180006，libpq180000、UTF8、ISO/MDY；pg_type实测1184/typlen8和1185/typelem1184/typlen-1。reference-only采样明确passed=false，仅作参考端证据，双端报告独立运行。

脚本沿现有标量契约设置上下文：参考端SET TimeZone TO，native端SET time_zone =。不新增设置语法。每次运行自建专属参考服务，在/tmp短目录避免macOS socket路径限制，使用独占空闲端口。Python finally关闭libpq/raw TCP/prepared/portal、等待并停止本任务参考服务、清理临时目录；Rust guard关闭同Server双协议端口、删除唯一数据库。失败和异常清理不操作用户服务或数据。

## 当前行为证据

四方向IT→OT、IT→OB、IB→OT、IB→OB均检查原始hex、参数OID、结果OID/typlen/typmod/格式、值与两层NULL；成功SQLSTATE写N/A。包含默认/负下界、矩形/六维、空集/首行NULL/全NULL/整值NULL、重绑定、p=0..6舍入与进位、BC/12000年、标量最小/最大值与正负无穷。相同瞬时的+00/+08输入必须得到相同UTC binary；UTC、Asia/Shanghai、America/New_York、Australia/Lord_Howe涵盖DST间隙、重叠和半小时切换，SQL构造与Parse冻结/Execute显示另有差分。

拒绝场景现场采集18.6 SQLSTATE并完整比较；TCP补statement/portal Describe、Bind拒绝无DataRow、discard-until-Sync、错误后CREATE无副作用及Sync恢复SELECT 1和42P01目录缺失。缺失字段不默认相等。输入及证据不含密码、DSN或认证载荷。MySQL JSON/VARCHAR/NULL在同Server前中后三次原协议快照一致。

最终Ready真实差分证据目录为 `target/pg05-array-timestamptz-evidence/82213-51706`（/tmp/pg05-timestamptz-ready.log）。summary.passed=true、mismatches=[]；570个libpq、222个TimeZone/DST、48个标量参数构造、8个SQL构造/cast、308个p=0..6及688个双端TCP场景通过。这些为JSON场景数，不冒充Rust测试数。拒绝SQLSTATE计数：22P02(18)、22021(4)、54000(18)、22007(6)、22008(14)、22P03(22)、42804(4)、08P01(66)。客户端关闭、参考端口关闭/目录删除、Rust数据库删除/双协议端口关闭字段全true；MySQL前中后原始快照一致。

## Ready命令和有效测试数

命令从仓库根执行；Cargo及子进程统一继承PG18_BIN和动态槽位的绝对CARGO_TARGET_DIR。最终主日志/tmp/pg05-timestamptz-ready.log，本族跨crate最终复核/tmp/pg05-timestamptz-final-prefix.log，lint日志/tmp/pg05-timestamptz-lint.log。

| 确切命令 | 退出码及有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server --lib pg05_array_timestamptz -- --nocapture --test-threads=1` | 0，3项；初始行为红灯101，2项 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_timestamptz -- --nocapture --test-threads=1` | 0，8项：server3/session2/parser1/parser-types1/types1 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1` | 0，144项：server45/session31/parser14/parser-types15/types39 |
| `cargo test -p astersql-types --lib pg_timestamp -- --nocapture --test-threads=1` | 0，6项timestamp/timestamptz载体回归 |
| `cargo test -p astersql-server --lib pg_temporal_tz -- --skip pg_temporal_tz_diff --nocapture --test-threads=1` | 0，7项现有标量协议回归 |
| `make lint` | 0，适用Go/dashboard检查 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_timestamptz_libpq.py` | 0 |
| `git diff --check -- pkg docs` | 0 |

去重157项Rust行为测试通过，其中本族8项；重复运行不重复累计。历史需外接PG18_REFERENCE_DSN的pg_temporal_tz_diff未运行，本任务自建精确18.6参考服务独立覆盖本族四方向与时区。没有编译/零测试或旧18.4证据替代验收。

所有Cargo阶段通过/tmp/pg05-timestamptz-cargo.sh依次原子mkdir领取共享槽位、记录任务/会话/PID，设置绝对目标目录并清理自有owner/锁。红灯及最终验证实际动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；一次中间冗余构建领取槽位2，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2，人工中断退出130，核实其编译子进程结束并清理自有锁，不作行为通过证据；两槽缓存保留。最终验证阶段所有槽位产物使用者结束后释放自有锁。

差异自审覆盖本族分派、上下文传递、两层NULL/下界/typmod和测试意图。只提交上述32个本任务文件，排除编号计划状态改动；plan.md始终只读。

## 验证范围与风险

Ready按根AGENTS.md和testing-flow选择；.agents/skills/tidb-verify-profile不存在，不宣称技能通过。Rust修改先cargo fmt --all。无Go增删/移动/import/顶层测试、Go依赖或Bazel修改，不触发bazel_prepare；未运行bazel_lint_changed。代码交付执行make lint。无外部依赖变化、vendor/third_party/本地patch；保留全部PingCAP许可及生产Rust的AsterSQL处理标记。

正确性和兼容性结论限于本18.6构建及实际场景，不能推断其他数组族或完整日期时间语法。TimeZone设置语法沿已交付标量契约；未开放上下文及跨族转换继续0A000。性能沿O(输入大小+元素codec)路径，增加有限次规范解析/显示，未作基准量化。未验证RealTiKV、持久化、其他平台/32位或完整隐式转换矩阵；本任务内存夹具不称RealTiKV通过。

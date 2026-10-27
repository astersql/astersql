# time数组PostgreSQL 18.6差分证据

任务36依赖任务35完整SHA `99d471df816bf76a1010548be209b0e9c45e3930` 及其保留的18.6报告。仅开放元素1083/数组1183，复用PgTime及round_precision；不扩建标量或其他数组族。

## 必要修改

parser/types/field_type.rs增加Time身份及TIME(p)[]恢复；parser_actions/expression.rs、grammar/main.astergram将已有PgTimeType声明接入数组动作，保留decimal；generated/main_tables.rs由release parsergen再生。types/pg_array.rs增加Time(PgTime)规范元素，沿共享shape、lower、typmod和两层NULL及文本/八字节网络序输出。

session/runtime/pg_array.rs开放本族分析、构造和同族转换并委派标量codec及精度舍入；control.rs/planning.rs保持声明、参数替换、typed NULL与空集身份；query.rs/relational_value.rs接入同一消费者。server/pg_extended.rs接入文本/二进制Bind，pg_result.rs接入结果与precision描述。非法任一元素拒绝整体；未支持跨族转换仍0A000。12个生产Rust文件的.rs.md同步更新。新建types/pg_temporal.rs.md并登记docs/docs-manifest.md。两项types旧未开放门禁由time改为尚未开放timestamp，保持原拒绝测试意图。

## TDD与参考

首次cargo fmt --all后cargo test -p astersql-server --lib pg05_array_time -- --nocapture --test-threads=1成功编译，实际2项，标量前置通过，1183参数实际返回0A000而非成功，退出101（/tmp/pg05-time-red.log）。同一断言用于后续通过验证。

参考工具PG18_BIN=/tmp/pg05-task14-reference/install/bin，实际PostgreSQL18.6/180006、libpq180000、UTF8、ISO/MDY、Asia/Shanghai。pg_type实测1083/typlen8、1183/typlen-1/typelem1083。参考-only用于未知诊断采集，summary.passed=false，不计双端完成证据。

真实差分覆盖IT→OT、IT→OB、IB→OT、IB→OB，包括00:00、24:00、微秒、p=0..6舍入进位、默认与负下界、多维/六维、空数组、首行NULL/全NULL/整值NULL及重绑定。记录原始输入输出hex、参数OID、结果OID/typlen/typmod/格式与SQLSTATE/阶段。libpq隐藏的portal Describe和discard-until-Sync由原始TCP补证，错误后CREATE无副作用且SELECT1恢复。同Server MySQL JSON/VARCHAR/NULL前中后快照保持一致。Python finally与Rust guard关闭客户端及自有服务、清理专属目录与数据库，不操作用户资源。

## 验证与限制

Cargo阶段由/tmp/pg05-time-cargo.sh依次原子领取共享槽位并设置绝对CARGO_TARGET_DIR，阶段结束只清理自己的owner/锁并保留缓存。Rust修改先fmt。Ready按根AGENTS及testing-flow执行，verify-profile技能不存在。无Go/import/依赖/Bazel改动，不触发bazel_prepare；未运行bazel_lint_changed。

未验证RealTiKV/持久化、其他平台/32位或性能基准；不扩展数组运算、存储与跨族转换矩阵。建议下一批次独立验证下一时间族数组。

## 标量诊断的必要局部纠错

首次双端真实红灯保存在target/pg05-array-time-evidence/20986-61966，`{00:00:00,not-a-time}`参考22023、目标22007。随后独立types回归实际1项在相同分类处失败（/tmp/pg05-time-classification-red.log）。恢复授权只允许最小错误分类纠错，不移植完整标量子系统。

`python3 /tmp/pg05-time-diagnose.py`现场采集18.6的SQLSTATE及消息：not-a-time/not-an-hour/bad-clock/Missing/Zone/missing.zone为22023，原因是time zone ... not recognized；garbage/not_a_time及无效numeric suffix为22007，offset越界22009，clock越界22008。依据同一18.6源码datetime.c的ParseDateTime/DecodeTimeOnly：字母后首个'-'/'/'/'.'的词先作为date-shaped名字进行时区查找。原PgTime直接把名字中的'-'送入数值偏移验证，从而误报Syntax。

修正只在原数值分支前增加27行非数值名字分类，沿已有UnknownZone错误与chrono_tz名单，排除已知IANA名字；只在独立名字或有效时钟后缀的上下文分类。前缀中另一个名字词禁止递归验证，限制调用深度。没有按某个字符串特判，没有统一改写所有非法输入，没有新增时间/时区解析或POSIX求值算法，没有新增依赖。其余合法拼写与标量原支持边界保持。独立标量回归覆盖同类、相邻、大小写IANA名字、错误先后顺序以及22007/22008/22009保留；同类输入也加入真实数组差分。

## 最终Ready证据

最终证据目录target/pg05-array-time-evidence/29808-63751（同阶段单包命令另有29700-63726）。summary.passed=true、mismatches=[]；494个libpq数组场景、28个标量参数构造场景、8个SQL构造/cast场景、168个p=0..6四方向舍入/进位/typed NULL/全NULL/empty场景及720个双端TCP场景通过。statement与portal Describe均逐p断言1183/-1/precision及输出格式，包含24:00、NULL与空集；原始数据按原样记录。成功SQLSTATE=N/A。

非法libpq场景从参考现场采集：22P02(18)、22021(4)、54000(18)、22023(14)、22007(26)、22009(4)、22008(12)、22P03(22)、42804(4)、08P01(66)。双端TCP证明Bind拒绝后无DataRow、流水线CREATE被丢弃、Sync后SELECT1恢复、目录查询42P01确认无副作用。MySQL三个快照一致。clients_closed/raw_clients_closed/reference_port_closed/reference_directory_removed和Rust database_dropped/postgres_port_closed/mysql_port_closed全部true。

所有Cargo阶段实际动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；由bash /tmp/pg05-time-cargo.sh继承PG18_BIN。正常及失败阶段退出后自有锁均清理，保留缓存。以下命令从仓库根运行；重复运行不重复统计，JSON场景不冒充Rust测试数。受影响Rust行为测试共140项：数组跨crate129项，加server本地time标量6项、types标量5项。

| 确切命令 | 退出码/有效数量 | 日志 |
| --- | --- | --- |
| cargo fmt --all | 0 | /tmp/pg05-time-final.log |
| cargo fmt --all -- --check | 0 | /tmp/pg05-time-final.log |
| cargo test -p astersql-server --lib pg05_array_time -- --nocapture --test-threads=1 | 0，4项；最初红灯同命令101，2项；后续差分红灯101，4项 | /tmp/pg05-time-final.log；红灯/tmp/pg05-time-red.log、/tmp/pg05-time-wip.log |
| cargo test -p astersql-types --lib pg05_array_time_scalar_named_zone_diagnostics -- --nocapture --test-threads=1 | 修正前101、修正后0，各1项 | /tmp/pg05-time-classification-red.log、/tmp/pg05-time-resume.log |
| cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_time -- --nocapture --test-threads=1 | 0，9项：server4/session1/parser1/parser-types1/types2 | /tmp/pg05-time-final.log |
| cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1 | 0，129项：server39/session28/parser12/parser-types13/types37 | /tmp/pg05-time-ready.log |
| cargo test -p astersql-server --lib pg_temporal_time -- --skip pg_temporal_time_diff --nocapture --test-threads=1 | 0，6项本地标量回归 | /tmp/pg05-time-final.log |
| cargo test -p astersql-types --lib pg_temporal_time -- --nocapture --test-threads=1 | 0，5项 | /tmp/pg05-time-final.log |
| cargo run --release -p astersql-parsergen --bin astersql-parsergen -- generate | 0 | /tmp/pg05-time-wip.log |
| cargo run --release -p astersql-parsergen --bin astersql-parsergen -- check | 0 | /tmp/pg05-time-final.log |
| make lint | 0，适用Go/dashboard检查 | /tmp/pg05-time-lint.log |
| python3 -m py_compile pkg/server/testdata/pg05_array_time_libpq.py | 0 | 本轮命令 |
| git diff --check | 0 | 本轮命令 |

初次server标量过滤器未排除历史pg_temporal_time_diff，7项中6通过，历史入口因PG18_REFERENCE_DSN未设置失败，退出101（/tmp/pg05-time-ready.log）；后改用适用的6项本地回归。历史外接DSN差分未验证，不将它当本任务当前18.6证据；本族18.6差分全量自建专属参考，未增加skip或降级路径。

自审：35个本任务文件，包括12个生产Rust及对应12个.rs.md、7个独立Rust测试文件、grammar/main.astergram、专属libpq脚本、docs-manifest及本证据。测试文件为pkg/parser/parser_test.rs、pkg/parser/types/field_type_test.rs、pkg/server/pg_extended_test.rs、pkg/server/pg_client_integration_test.rs、pkg/session/runtime/pg_array_test.rs、pkg/types/pg_array_test.rs、pkg/types/pg_temporal_test.rs。PingCAP许可保留，12个生产Rust文件顶部均有AsterSQL处理标记。未改plan.md、Go/Bazel/Cargo依赖；无外部Rust临时副本或本地patch。未运行持久化/RealTiKV、其他平台或性能基准。正确性及兼容性证据限于上述18.6构建、受测标量拼写和数组范围，不宣称完整时间/时区输入语法支持。

# interval数组 PostgreSQL 18.6 差分证据

任务39依赖批次38提交 `fd07ef2aec1c508f4554e8d7e15dd5c52b3b74c4` 及其18.6四方向与Ready证据。仅开放interval元素1186/数组1187，复用PgInterval的(months:i32,days:i32,time_microseconds:i64)、双向codec、apply_typmod及IntervalStyle。不折为总秒，不修改其他族或标量解析算法。

## 本族局部接线

13个生产Rust文件及对应13个.rs.md：pkg/types/{pg_array,pg_temporal}.rs；pkg/parser/types/field_type.rs；pkg/parser/parser_actions/expression.rs；pkg/session/runtime/{control,planning,pg_array,pg_temporal,query,relational_value,system_query}.rs；pkg/server/{pg_extended,pg_result}.rs。每项依据如下。

规范数组载体注册Interval变体并验证1186身份，binary元素直接输出16字节；FieldType映射1186/1187并恢复fields/p，parser沿既有数组语法识别TypePgInterval且继承修饰符，不新增语法或生成解析表。control/planning保存声明参数、typed NULL和重绑定；pg_array元素输入/构造复用标量codec和typmod；pg_temporal冻结Parse时数组字面量的样式解释；query/relational_value/system_query将本族接入真实SELECT及常量UNION消费者。pg_extended在Bind冻结参数样式且验证全部元素，对直接参数投影提前验证typmod溢出；pg_result保留1187/-1/编码fields-p，并在Execute按有效样式输出text，binary保留三分量。

8个独立测试文件：pkg/types/{pg_array,pg_temporal}_test.rs；pkg/parser/{parser_test.rs,types/field_type_test.rs}；pkg/session/runtime/pg_array_test.rs；pkg/server/{pg_extended_test,pg_client_integration_test,pg_temporal_test}.rs。新增pg05_array_interval_libpq.py及本文。规范容器两项未开放族门禁从1186改为仍未开放的2950，保持拒绝测试意图。没有新增生产源文件，docs-manifest无新增项。

## TDD与既有标量输出纠错

最初运行cargo fmt --all后，cargo test -p astersql-server --lib pg05_array_interval -- --nocapture --test-threads=1：2项有效测试，标量前置通过，数组入口1187以0A000拒绝；退出101，日志/tmp/pg05-interval-red.log。

18.6双端差分发现标量SqlStandard遗漏零年月组：分量(0,-1,7384000000)应输出+0-0 -1 +2:03:04，目标输出-1 +2:03:04。独立OID1186复现日志/tmp/pg05-interval-scalar-blocker.log，3项中2过1失败，退出101；原始证据target/pg05-array-interval-evidence/95788-53473/cases.jsonl。协调交接明确授权现有格式化分支的最小纠错及独立回归属于本族必要局部接线。

先在精确18.6采集月份[-14,0,14]×天[-2,0,2]×微秒[-7384000000,0,7384000000]共27种正负零组合，四种样式；/tmp/pg05-interval-reference-matrix为参考端样本，明确passed=false。新types独立回归包含108个实采输出与分量回读断言，先运行1项失败退出101（/tmp/pg05-interval-scalar-red.log）：(-14,2,7384000000)的postgres输出多一个time正号。

仅修正pg_temporal.rs两处既有格式化状态/条件：Display的negative只保存上一个已输出字段的符号，不累计早先全部负号；SqlStandard混合符号时即使months为0也保留零年月组。没有元组特判、新算法或标量解析器扩建。既有pg_temporal_iv_range_special的一项极值期望原样保留了同一多余正号；在精确18.6对(i32::MIN,i32::MAX,i64::MAX)实采确认后，仅纠正期望文本，保留全部极值、四方向、元数据与NULL断言。该回归首次7过1失败，不冒充通过；原始行为红灯保留，夹具曾修正ParameterStatus及portal生命周期处理，不把这些夹具失败当生产红灯。

## 差分方法与资源

PG18_BIN=/tmp/pg05-task14-reference/install/bin，现场postgres --version及server_version_num核对18.6/180006；libpq180000、UTF8、ISO/MDY；pg_type实测1186/typlen16和1187/typelem1186/typlen-1。脚本每次自建/tmp短目录下的专属initdb和空闲端口，try/finally关闭客户端及本任务参考服务、等待进程并删除数据；Rust guard关闭同Server双协议端口并删除唯一数据库，不操作用户服务/数据。

合法值逐项验证IT→OT、IT→OB、IB→OT、IB→OB的原始载荷、参数OID、结果OID/typlen/typmod/格式、值与两层NULL。覆盖月份/天/24小时不折算、小数月、独立符号、三分量极值/混合有限极值及全极值非有限、默认/负下界/矩形/六维/空集、首行NULL/全NULL/typed NULL、重绑定、p=0..6及全部fields掩码。四样式27种相邻符号组合同时验证标量和数组四方向。

TCP补statement/portal Describe、Parse/Bind冻结与Execute样式变化、直接数组参数typmod溢出在Bind拒绝、非法任一元素的完整SQLSTATE/阶段/无DataRow、discard-until-Sync、后续CREATE无副作用和Sync后SELECT 1/42P01目录缺失。参考端ParameterStatus在记录中保留并核对其IntervalStyle值。成功SQLSTATE为N/A，缺失字段不得默认相等。MySQL在同Server前/中/后三次用原协议比对JSON/VARCHAR/NULL的列定义与原始行载荷。

## 验证范围与风险

Ready按根AGENTS.md和testing-flow选择；.agents/skills/tidb-verify-profile不存在，不虚构技能通过。无Go源/import/顶层测试、依赖或Bazel变更，不触发bazel_prepare；不运行bazel_lint_changed。无外部Rust依赖修改或本地patch/vendor，全部已有PingCAP许可和AsterSQL标记保留。所有Cargo阶段原子mkdir动态领取共享槽位并记录owner/PID，继承绝对CARGO_TARGET_DIR，正常/失败清理自己的锁并保留缓存。

正确性与兼容性仅限本18.6构建及所列场景；不推广到完整ARRAY赋值/存储、其他族或隐式转换矩阵。未测RealTiKV/持久化、其他平台/32位及性能基准，内存夹具不称RealTiKV。性能沿原数组O(输入与元素总大小)路径，新增本族标量解析/显示与参数typmod验证，未量化吞吐。

## Ready命令与最终结果

最终本族原始证据为 `target/pg05-array-interval-evidence/5782-54926`，主日志 `/tmp/pg05-interval-final.log`；此前跨crate本族日志 `/tmp/pg05-interval-matrix-green.log`，受影响数组与session/types日志 `/tmp/pg05-interval-ready.log`，lint日志 `/tmp/pg05-interval-lint.log`。最终summary.passed=true，mismatches=[]。590个基础libpq场景（含20个极值/小数月）、48个标量参数构造、8个SQL构造/cast、560个标量样式与560个数组样式、2520个fields/p、796个双端TCP场景通过；这些是场景数，不是Rust测试数。

libpq拒绝SQLSTATE：22P02×18、22021×4、54000×18、22007×8、22015×8、22P03×22、42804×4、08P01×82。TCP另核对typmod舍入溢出22008及NUL22021的Bind阶段。客户端关闭、参考端口关闭/目录删除、Rust唯一数据库删除/双协议端口关闭全部true；MySQL before/during/after原始快照完全相同。

以下命令从仓库根执行，全部Cargo和使用其产物的子进程继承动态槽位绝对目录及PG18_BIN。所有阶段实际领取槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。自有owner和slot-1.lock在全部产物使用者退出后释放，缓存保留；未删除其他锁或共享参考工具。

| 确切命令 | 最终退出码/有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server --lib pg05_array_interval -- --nocapture --test-threads=1` | 0，3项；初始行为红灯101/2项 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_interval -- --nocapture --test-threads=1` | 0，8项：server3/session1/parser1/parser-types1/types2 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1` | 0，152项：server48/session32/parser15/parser-types16/types41 |
| `cargo test -p astersql-types --lib pg05_array -- --nocapture --test-threads=1` | 0，41项；自审调整门禁反例后重跑 |
| `cargo test -p astersql-types --lib pg_interval -- --nocapture --test-threads=1` | 0，5项 |
| `cargo test -p astersql-types --lib pg_temporal_iv_bridge -- --nocapture --test-threads=1` | 0，1项 |
| `cargo test -p astersql-session --lib interval_ -- --nocapture --test-threads=1` | 0，6项，其中本族1项重复计数不累加 |
| `cargo test -p astersql-server --lib pg_temporal_iv -- --skip pg_temporal_iv_diff --nocapture --test-threads=1` | 0，8项；旧期望修正前7过1失败 |
| `make lint` | 0，适用Go/dashboard检查；后续仅Rust改动继续复用此证据 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_interval_libpq.py` | 0 |
| `git diff --check -- pkg docs` | 0 |
| `git diff --cached --check` | 0 |

去重171项Rust测试通过，其中本族8项。新增108条标量正负零输出/回读断言位于一个独立types测试中，不能将断言数冒充测试数。历史依赖PG18_REFERENCE_DSN的pg_temporal_iv_diff未重跑；本任务自建精确18.6参考集群独立覆盖标量/数组四方向和全部四种样式。不用旧18.4、编译或零测试代替本任务验收。

自审保留13个生产文件及同名文档、8个测试文件、新脚本与本文，共36个文件；每项均有本族增量或不可缺少接线依据。只提交这些文件，排除其他编号任务状态，plan.md始终只读。建议继续独立任务40的uuid数组。

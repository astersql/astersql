# timestamp数组 PostgreSQL 18.6 差分证据

任务37依赖任务36提交 `a23b5a0a36d279133274fab40fb53dd1dc5cb693` 及其18.6行为报告。仅开放 timestamp 元素1114/数组1115，复用现有 PgTimestamp 标量身份、墙钟解析、八字节网络序codec和精度舍入，不移植其他标量子系统。

## 必要修改与范围

`pkg/types/pg_array.rs` 增加 Timestamp(PgTimestamp)规范元素；容器复用原shape/lower/typmod及两层NULL、文本转义与行主序二进制输出。`pkg/parser/types/field_type.rs` 增加1114/1115及TIMESTAMP(p)[]恢复。`pkg/parser/parser_actions/expression.rs` 与 `pkg/parser/grammar/main.astergram` 将已有PgTimestampType接入数组动作并保留decimal，generated/main_tables.rs由release parsergen再生。

`pkg/session/runtime/pg_array.rs` 仅增加本族分析、构造、同族cast分派，调用PgTimestamp及round_precision；control.rs/planning.rs保留声明参数、typed NULL与重绑定身份，query.rs/relational_value.rs连接现有共享消费者。server/pg_extended.rs接入文本/二进制Bind及冻结输入上下文，pg_result.rs接入结果编码与precision描述。非法任一元素拒绝整体，未开放跨族转换仍拒绝。这11个生产Rust文件均同步同目录.rs.md；新增parser、parser-types、types、session各1项及server3项独立测试，不将测试放入生产文件。原types的2项未开放门禁改用仍未开放的1184，保留原拒绝测试意图。

## TDD和参考身份

最初 `cargo fmt --all` 后，`cargo test -p astersql-server --lib pg05_array_timestamp -- --nocapture --test-threads=1` 编译成功，2项有效行为测试中标量前置通过，1115数组参数入口返回0A000导致注册测试失败，退出101；日志 `/tmp/pg05-timestamp-red.log`。实施后相同断言通过，扩展为包含真实差分的3项server测试；首次通过日志 `/tmp/pg05-timestamp-wip.log`。

参考 `PG18_BIN=/tmp/pg05-task14-reference/install/bin`，现场 `postgres --version` 为18.6，version()/server_version_num为180006、libpq180000、UTF8、ISO/MDY、Asia/Shanghai。pg_type实测1114/typlen8与1115/typelem1114/typlen-1。`--reference-only`只采集参考端，不报告双端通过。

真实差分脚本 `pkg/server/testdata/pg05_array_timestamp_libpq.py` 由Rust双协议Server夹具启动，脚本自建独占临时参考集群。IT→OT、IT→OB、IB→OT、IB→OB四方向检查原始hex、参数OID、结果OID/typlen/typmod/格式、值、NULL与错误阶段/SQLSTATE。覆盖墙钟与忽略输入偏移、24:00规范化、微秒及超出微秒精度、epoch前后与正负舍入中点、BC/12000年及标量最小/最大值、正负无穷、默认/负下界、矩形与六维、空集/首行NULL/全NULL/整值NULL、重绑定与p=0..6。

原始TCP补证libpq隐藏的statement/portal Describe、Bind拒绝无DataRow、discard-until-Sync、错误后CREATE无副作用、SELECT1恢复及目录42P01。非法分类从本任务18.6端现场采集，比较完整观测，不默认缺失字段相等。成功SQLSTATE记录N/A。MySQL JSON/VARCHAR/NULL原行为在同Server前中后三次快照一致。

Python finally关闭libpq和raw TCP、prepared/portal及参考服务，清理自己的临时目录；Rust guard关闭Server/客户端、删除唯一测试数据库并检查双协议端口关闭，不操作用户服务/数据。

## 验证执行与限制

所有Cargo阶段通过 `/tmp/pg05-timestamp-cargo.sh` 从槽位1–10依次原子mkdir动态领取，设置绝对CARGO_TARGET_DIR并记录任务/会话/PID；EXIT/INT/TERM清理自己的owner/锁，保留缓存。Rust修改先cargo fmt --all。Ready按根AGENTS和testing-flow选择，verify-profile技能不存在。只改Rust/语法/本族Python/文档，无Go、import、依赖或Bazel改动，不触发bazel_prepare；未运行bazel_lint_changed。

一次中间跨crate运行退出101：构建过程中扩展了最小/最大值夹具，旧server测试二进制仍期望40个constructor场景，而脚本实际完成48个；其双端summary.passed=true、mismatches=[]。已同步源码计数并重新构建最终版本。该次输出不作为最终Ready通过证据，日志 `/tmp/pg05-timestamp-ready.log`。

未验证RealTiKV/持久化、其他平台/32位及性能基准；数组运算、存储、完整隐式/跨族转换矩阵和完整日期时间输入语法均在范围外。正确性和兼容性结论限于当前18.6构建与所列实际场景，不能从本族推断其他数组族通过。性能沿现有O(输入大小+元素codec)路径，无新增外部依赖；未作性能量化。

## 当前真实差分结果

最终版本证据目录 `target/pg05-array-timestamp-evidence/40135-49176`（相同阶段专属命令的路径见/tmp/pg05-timestamp-final.log）。summary.passed=true、mismatches=[]；570个libpq数组场景、48个标量参数/ARRAY构造、8个SQL构造/cast、308个p=0..6四方向舍入/进位/typed NULL/全NULL/empty/非有限值场景、688个双端TCP场景通过。上面数量为JSON场景，不冒充Rust测试数。

拒绝场景的参考SQLSTATE计数：22P02(18)、22021(4)、54000(18)、22007(6)、22008(14)、22P03(22)、42804(4)、08P01(66)。TCP证明错误在Bind阶段拒绝，无DataRow，无副作用，Sync后可恢复。客户端/参考服务/临时目录及Rust数据库/双协议端口清理字段全部true，MySQL前中后原始快照一致。

## 文件清单

11个生产Rust及其对应11个.rs.md：parser/types/field_type.rs、parser/parser_actions/expression.rs、parser/generated/main_tables.rs、types/pg_array.rs、session/runtime/pg_array.rs、session/runtime/control.rs、session/runtime/planning.rs、session/runtime/query.rs、session/runtime/relational_value.rs、server/pg_extended.rs、server/pg_result.rs（均位于pkg/）。6个独立测试文件：pkg/parser/parser_test.rs、pkg/parser/types/field_type_test.rs、pkg/types/pg_array_test.rs、pkg/session/runtime/pg_array_test.rs、pkg/server/pg_extended_test.rs、pkg/server/pg_client_integration_test.rs。另有pkg/parser/grammar/main.astergram、本族libpq脚本和本文，共31个文件。没有新增生产Rust文件，因此无需新增docs-manifest条目。

## Ready命令与有效测试数量

从仓库根运行，Cargo命令统一继承PG18_BIN和动态槽位的绝对CARGO_TARGET_DIR；同一个测试的重复运行不重复累计。最终验证日志 `/tmp/pg05-timestamp-final.log`，make lint日志 `/tmp/pg05-timestamp-lint.log`。

| 精确命令 | 退出码/有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server --lib pg05_array_timestamp -- --nocapture --test-threads=1` | 0，3项；最初红灯101，2项 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array_timestamp -- --nocapture --test-threads=1` | 0，7项：server3/session1/parser1/parser-types1/types1 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib --no-fail-fast pg05_array -- --nocapture --test-threads=1` | 0，136项：server42/session29/parser13/parser-types14/types38 |
| `cargo test -p astersql-types --lib pg_timestamp -- --nocapture --test-threads=1` | 0，6项timestamp及相邻timestamptz标量回归 |
| `cargo test -p astersql-server --lib pg_temporal_ts -- --skip pg_temporal_ts_diff --nocapture --test-threads=1` | 0，7项本地标量回归 |
| `cargo run --release -p astersql-parsergen --bin astersql-parsergen -- generate` | 0，生成解析表 |
| `cargo run --release -p astersql-parsergen --bin astersql-parsergen -- check` | 0，生成表一致 |
| `make lint` | 0，适用Go/dashboard检查 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_timestamp_libpq.py` | 0 |
| `git diff --check -- pkg docs` | 0 |

去重共149项Rust行为测试通过，非零测试、非编译替代。历史需外接PG18_REFERENCE_DSN的pg_temporal_ts_diff未运行；本任务自建精确18.6参考已独立覆盖本族四方向。未改plan.md、Go/Bazel/Cargo依赖，无vendor/third_party/本地patch及外部Rust临时副本；保留PingCAP许可，11个生产Rust文件均保留AsterSQL处理标记。

所有Cargo阶段实际动态领取槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。最终阶段退出0，parsergen check成功；所有槽位产物使用者已结束，自有owner和slot-1.lock均已清理，编译缓存保留。Ready全部适用检查通过；差异自审通过，只提交本文所列31个本任务文件，排除所有既有计划状态变更。

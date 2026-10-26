# PostgreSQL18 带时区时间戳真实客户端差分

2026-10-26，任务31基于 `de5dd7aaec46382830791906649686315228a772` 执行，直接复用
批次24–30的独立瞬时载体、时区/DST、精度、桥接、NULL及非法载荷实现。
新增 `pg_client_integration_test::pg_temporal_tz_diff` 和 `pg_core_libpq.py` 的
`timestamptz-differential` 模式，真实 libpq 连接 PostgreSQL18参考端与目标双listener，
不模拟SQL结果、时间解析或客户端。

## 发现与最小修复

`target/tz31-fourth.log` 实际执行1项差分测试，退出101：`TIMESTAMPTZ 'now'` 在参考端
成功返回1184，目标返回42703。独立parser回归 `target/tz31-parser-red.log` 运行1项失败，
证实目标把它解析成Column及别名。修复后同族parser测试2项通过。

生产修改只涉及 `pkg/parser/lexer.rs` 和 `pkg/parser/parser_actions/expression.rs`：
PG未引用的timestamptz紧随字符串时接入既有timestamp字面量语法，语义动作保留
`TypePgTimestampTz` 并生成真实Cast AST；既有session消费者处理Parse时特殊词。
依据为TZ-SESSION相对词时机、本任务所需的局部语法接线及用户明确允许补最小接口的授权。
MySQL及引用标识符保留原路径，测试放在独立 `pkg/parser/parser_test.rs`。
没有SQL字符串替换，没有扩写通用类型推导、GUC、存储或运算系统。

初始夹具继承的timestamp假设已按真实参考端纠正：timestamptz(-1)为22023，-0合法，
+3为42601；耗尽portal重复Execute不重复返回行，跨Simple Query使用命名portal。
这些夹具修正不作为生产缺陷证据，也没有删除原要求的行为断言。

## 实测环境与身份

SQL `version()` 为 PostgreSQL **18.4 (Homebrew)**，server_version_num=180004，
aarch64-apple-darwin25.4.0，Apple clang 21.0.0，64-bit；libpq=180000。
没有运行PostgreSQL18.6，不将客户端版本当作服务器版本。
目标采用默认features、CreateAnalyzeSession、真实TCP PgService及共享session消费者，
使用测试专用认证和临时双listener。

DateStyle=ISO, MDY，TimeZone基线UTC，另测Asia/Shanghai、America/New_York和
Etc/GMT+12、Etc/GMT-14。目标通过已有 `SET time_zone` / `SELECT @@time_zone` 设置和
读取真实会话区域，参考通过 `SET TimeZone` / `SHOW TimeZone`。
参考IntervalStyle=postgres；目标SHOW IntervalStyle返回42601，在identity原样记录。
本项不将该独立会话配置缺口当作timestamptz编解码成功证据。
主机tzdata=2026d-rearguard，目标chrono-tz 0.10.4内置2025b；参考发行标签未暴露，
identity保存五个受测zone文件SHA256，不以主机tzdata标签冒充参考端版本。
identity还记录实际HEAD、相关源码diff、Cargo.lock、夹具及Rust测试二进制SHA256。

一个Rust测试承载 **517项配对观察**，七项能力全部通过；517不是Rust测试数量。
另有行字节、类型元数据、SQLSTATE三项比较器扰动自检。

| 能力 | 本次断言 |
| --- | --- |
| TZ-SAME-INSTANT | +00午夜与+08八点为同一epoch，二进制八字节零；同墙钟+08与+00相差八小时。独立整数微秒期望与双端观察共同检查。 |
| TZ-SESSION | New_York Bind后改区Execute，UTC、Shanghai、New_York文本不同、binary瞬时不变；命名portal与重复Execute、格式切换。now/today/yesterday/tomorrow在三时区四方向比较，参考事务时刻经目标现有SET timestamp注入。跨26小时时区切换证明相对常量Parse冻结、参数Bind冻结，无sleep。 |
| TZ-DST | 2018春季缺口02:30→07:30Z，秋季重复01:30→06:30Z，显式-04→05:30Z、-05→06:30Z；四方向及改区显示保留瞬时。 |
| TZ-PRECISION-RANGE | epoch±1微秒，p=0两侧半秒离零舍入，p=3/6、七位输入、跨年进位；有限两端与相邻域外、BC无年份0、扩展年份、infinity/-infinity；归一偏移导致域外22008；p=7警告22023归6，负精度与语法错误分开。 |
| TZ-CAST-BRIDGE | 1114↔1184显式cast，UTC/Shanghai午夜不同瞬时，New_York重复小时两瞬时转同墙钟；NULL和特殊值保留目标身份。原生DATETIME显式依UTC/Shanghai解释；TIMESTAMP缺来源前提拒绝0A000，零日期22008，拒绝后SELECT 1成功。 |
| TZ-NULL-METADATA | p=-1/0/3/6，typed NULL、常量NULL、首行/全NULL、多行、零行；1184/8/p或-1、ParameterDescription、statement/portal Describe、重复Execute与格式切换。NULL长度-1与epoch八字节零分开。 |
| TZ-INVALID | garbage/空串/allballs为22007；日历、零日期、归一溢出22008；未知区域22023；UTF-8/NUL22021；binary空/7字节08P01、9字节22P03、帧声明错误08P01。Sync后SELECT 1；事务错误E经ROLLBACK恢复I。 |

合法值及typed NULL覆盖IT→OT、IT→OB、IB→OT、IB→OB，即文本/二进制参数到文本/二进制结果。
libpq准备/执行配合经libpq认证的独立wire连接，补充portal Describe、嵌入NUL和错误阶段；
比较SQLSTATE与协议阶段，完整诊断另存，不要求构建路径或错误文案相同。
同一MySQL连接在PG前、中、后实际更新并读回已有值，保留sql_mode=''、DATETIME=12、
TIMESTAMP=7、Fsp=6及原始列定义/行字节；三时区读回保持当前runtime既有行为。
这些断言不证明MySQL TIMESTAMP存取转换已符合MySQL服务器。

规范载体复用 `pkg/types/pg_temporal.rs::PgTimestampTz(i64)`，通过from_micros/micros、
from_binary/to_binary、round_precision、parse_with_offset/format_with_offset承接。
有限域为[-211813488000000000,9223371331200000000)，i64两端为无限哨兵；不借MySQL位域、
总纳秒或text OID冒充身份。非法内部构造、反向MySQL桥接由前置独立测试及累计TZ回归补证。

## 验证命令和资源

WIP用于失败定位，Ready用于交付；验证profile技能缺失，按根协议执行适用检查。
已有工作区，无Go源/import/顶层Go测试/依赖/Bazel元数据变化，无Bazel错误，不触发
bazel_prepare，未运行bazel_lint_changed。
所有Cargo阶段均通过原子mkdir动态领取槽位 **1**，设置：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

阶段所有子进程结束后释放自己的锁，保留缓存。参考端在唯一临时目录initdb、临时回环端口
启动；PG18_REFERENCE_DSN缺失明确失败。Python finally关闭结果/连接，Rust Drop关闭
listener并删除唯一数据库；runner停止自己启动的集群，检查端口关闭并删除临时目录。
不操作用户服务或数据。

仓库根目录的确切验证命令（Cargo由runner继承槽位；差分同时继承隔离参考端环境）：

    cargo fmt --all
    python3 -m py_compile pkg/server/testdata/pg_core_libpq.py
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_tz_diff -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_tz -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_timestamptz -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_temporal_ts -- --nocapture
    make parser parser_yacc parser_fmt
    tools/check/failpoint-state.sh enable go
    gofmt -w pkg/parser/ast/binding__failpoint_binding__.go
    make parser_unit_test
    tools/check/failpoint-state.sh disable go
    make lint
    git diff --check
    git diff --cached --check

首次parser_unit_test在failpoint生成的binding文件格式检查处失败，退出2；预先gofmt该临时
生成文件后重跑通过，退出0，trap保证failpoints关闭。一次与failpoints开启窗口重叠的lint
因生成符号命名返回2，已在failpoints完全关闭后重跑；最终结果见下文。

未验证PostgreSQL18.6、持久TiKV、性能、其他时间族、完整DateStyle/GUC生命周期及完整catalog。
正确性/兼容性风险集中在未列出的输入语法和这些范围外接口；性能变化仅为PG词法命中的一次
已有lookahead及类型选择，没有新存储或时区算法。

## 最终结果

`target/tz31-ready.log` 退出0：fmt、py_compile、差分1 passed、pg_types_test 7 passed、
累计pg_temporal_tz 8 passed（含本差分）、parser同族2 passed。
`target/tz31-parser-ts.log` 退出0：已有timestamp精度/字面量parser回归2 passed。
`target/tz31-parser-unit.log` 退出0：Go parser及子包测试通过，failpoints最终refcount=0。
`target/tz31-lint-final.log` 的make lint退出0。

Go临时failpoint文件全部还原后重采最终差分 `target/tz31-final-diff.log`，退出0，1 passed，
517项配对通过。最终原始证据目录为：

    target/pg-tz-evidence/58056-1793016698026007000/

summary两端cleanup=true，child-exit code=0；rust-cleanup的database_dropped、
postgres_port_closed、mysql_port_closed全部true，runner参考端口关闭检查通过。
源码diff SHA256为 `f46cc81c73a2218b5488477c9b30cd327421822043472ec722bd55b7141da3a2`，
夹具SHA256为 `4ff20e9bdd69384d9b2e40df96032bd2313bf63510029b2c192b04bd39456a21`。
已重新计算并核对源码diff与identity一致；实际槽位1锁已释放，缓存保留。
`git diff --check` 和暂存后的 `git diff --cached --check` 均退出0。

# PostgreSQL18 TIME 真实客户端差分

2026-10-26，任务15在基线 `807c64a3c3d77296422926740e2c7e9617e51525` 上执行。
前置批次9–14已提供独立日内表示、精度、文本时区、duration隔离、NULL元数据及非法载荷恢复。
本项复用这些实现，新增真实客户端差分，并修正真实参考端揭示的带符号精度语法错误阶段。

## 实际环境和结论

参考端 SQL `version()` 为 PostgreSQL **18.4 (Homebrew)**，`server_version_num=180004`，
aarch64-apple-darwin25.4.0，Apple clang 21.0.0 (clang-2100.0.123.102)，64-bit。
客户端为 Python3 ctypes 调用 libpq **180000**；真实 TCP 的启动和认证均由libpq完成。
目标端为默认features的 `astersql-server`，由 `CreateAnalyzeSession`、临时双listener及
真实session执行路径启动，认证为测试专用 `InsecureRootOnly`。

最终差分为 **292项独立配对观察通过**，另有行字节、OID元数据、SQLSTATE三项比较器扰动检查。
这些观察由一个Rust测试承载，不能将292称为Rust测试数。六个TIME能力ID均有当前运行证据。
MySQL同一连接在PG流程前、中、后均完成写入、读取、sql_mode及原始列定义的类型/Fsp检查。
值 `-00:00:01.000000`、`25:00:00.000000`、`838:59:59.000000` 保持MySQL TIME(6)身份，
两种PG结果格式的桥接均返回0A000，错误后SELECT 1::int4成功。该拒绝是双方言桥接契约，
不作为普通合法PG time值的验收结果。

DateStyle基线为ISO, MDY，TimeZone为UTC；另验证Asia/Shanghai、America/New_York下
带数值偏移的普通time输入仍保留墙钟字段。目标沿用既有 `SET time_zone = '…'` /
`SELECT @@time_zone` 接口，参考端用 `SET TimeZone TO '…'` / `SHOW TimeZone`。
这只是显式设置环境，不替换受测SQL或结果。参考IntervalStyle=postgres；目标
`SHOW IntervalStyle` 实际返回42601，单独记录在identity，不据此声称完整GUC已支持。

主机tzdata为2026d-rearguard；目标chrono-tz 0.10.4内置IANA 2025b。参考安装包未暴露
编译时区数据的发行标签，identity保存其五个受测zone文件的SHA256；没有将主机标签当成
参考服务器tzdata版本。普通time忽略已验证的数值偏移，不涉及按日期解析命名时区或DST。

## 精度错误的阶段区分与局部修复

首次原始断言要求SQL负精度返回22023，真实PG18.4却在Parse返回42601。
依据固定官方 [REL_18_0 gram.y 的 ConstDatetime](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/parser/gram.y#L13910)，
TIME(p)的p使用Iconst，不使用SignedIconst，因此 `time(-1)`、`time(-0)`、`time(+3)`
均在语法阶段失败，尚未到达内部typmod校验。
官方 [REL_18_0 date.c 的 anytime_typmod_check / timetypmodin](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/date.c#L45)
另规定已到达内部校验的负精度为22023，大于6警告22023并归6。这两种行为不互相替代。

恢复执行获得用户授权后，本项分别保留真实阶段证据：

- CAST和postfix的三种带符号精度，两端Query和Extended Parse均为42601，Parse分别含有/没有多余参数OID，Sync后SELECT 1恢复。
- 真实参考端 `SELECT pg_catalog.timetypmodin(ARRAY['-1']::cstring[])` 为22023，单独保存于
  identity的reference_internal_negative_typmod。目标没有该catalog函数接口；没有伪造配对
  观察，也不在本项扩建函数/数组子系统。该内部接口与SQL TIME(p)语法的验收分开报告。
- SQL time(7)在两端实际成功并发出22023告警，结果metadata保留1083/8/6。

生产修改只有 `pkg/parser/parser_actions/expression.rs` 拒绝带符号TIME精度，以及
`pkg/server/pg_result.rs` 将该语法错误的早期诊断正确归类42601。相邻独立解析器/服务器测试
同步更正并增加-0/+3。依据为TIME-PRECISION的真实错误阶段；未改只读路线图或plan.md。
累计回归还证明需保留既有早期诊断路径：语法错误须优先于未使用参数OID的计数错误08P01，
因此只修正该早期诊断的分类，不改通用协议状态机。
原先接受+3/-0及负数改报22023是前置实现偏差，不将原测试期望当参考行为继续沿用。

## 能力与当前证据

| 能力ID | 本次实际断言 |
| --- | --- |
| TIME-DAY | 00:00:00、23:59:59.999999、24:00:00及普通微秒值；0、86399999999、86400000000的独立八字节网络序。24:00不取模；24:00:00.000001、25:00、12:60、12:00:61均22008。 |
| TIME-PRECISION | time(0)半秒进位，time(3) .1234/.1235两侧舍入，日末向24:00进位，time(6)保留；七位输入归微秒；time(7)告警22023并归6；负精度按上述真实阶段分开。 |
| TIME-TEXT-ZONE | 04:05、040506、04:05 PM、allballs及+08:00数值偏移；规范输出、三种会话时区、OID1083连续保留。allballs为静态零点，不是依赖事务时刻的相对日期词。 |
| TIME-DURATION | 同进程MySQL负/长duration前中后自身规则和身份保留；PG两结果格式均明确0A000桥接拒绝，原连接仍能写入与读回。反向24小时和微秒桥接由当前累计TIME族回归覆盖。 |
| TIME-NULL-METADATA | 无声明、p=0/3/6的typed NULL、NULL常量、首行NULL、全NULL、多行、零行；参数Describe、statement/portal Describe、Execute、重复Execute和结果格式切换；1083/8/p或-1连续，NULL长度-1与零点八字节全零分开。 |
| TIME-INVALID | 空串/garbage为22007；UTF-8/NUL为22021；binary0/7字节08P01、9字节22P03，-1/86400000001/i64两端22008；帧长度声明大于余长08P01；每例两格式、Sync后SELECT 1，事务失败E经ROLLBACK回I。 |

每个规范合法值及typed NULL均覆盖IT→OT、IT→OB、IB→OT、IB→OB；IT/IB为文本/二进制
输入，OT/OB为文本/二进制输出。非规范官方拼写使用明确规范文本和独立整数微秒作为期望，
不从目标输出推导参考值。libpq检查准备语句和结果metadata、参数OID、原始字节和NULL；
独立已由libpq认证的空闲连接补测portal Describe、嵌入NUL、声明长度、错误阶段和重复Execute。
两端完整ErrorResponse字段另归档，比较SQLSTATE和阶段，不要求源码行号或文案相同。

内部非法12:60构造不能由libpq直接注入，由前置 `pg_temporal_time_day` 的结果编码双格式
断言及当前累计回归补证。timetz未开放由当前 `pg_temporal_time_text_zone` 回归验证0A000。
规范载体继续复用 `pkg/types/pg_temporal.rs::PgTime`，域0..=86400000000微秒；公开
from_micros/micros、from_binary/to_binary、round_precision、FromStr/Display、
from_mysql_duration/to_mysql_duration，不用MySQL duration身份或text OID冒充PG time。

## 验证命令与资源

WIP用于TDD迭代；Ready按根协议执行交付检查。仓库缺少
`.agents/skills/tidb-verify-profile`，不宣称运行过该技能。
每个Cargo阶段动态原子mkdir领取共享槽位1，锁记录计划、任务、会话和PID：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

所有Cargo、脚本及测试二进制均继承该目录；进程结束后释放自己的锁，编译缓存保留。
精确命令、结果及最终证据路径见下文的最终运行记录。

参考端由本任务独立initdb数据目录和回环端口启动，进程环境设置PG18_REFERENCE_DSN，
缺少DSN时测试明确失败而不跳过；不记录DSN或密码。Python finally关闭libpq及wire连接，
Rust Drop guard关闭双listener并删除唯一临时数据库。runner仅停止自己的pg_ctl集群，
删除自己的临时目录并检查端口不可连接；不操作用户服务或数据。

## 验证边界与风险

未验证PostgreSQL18.6、持久TiKV、性能基准、其他时间族、完整GUC和catalog typmod函数。
本次使用真实执行路径和测试存储，不能等同持久TiKV证据。没有Go、Bazel元数据、manifest
或外部Rust依赖变更，不触发bazel_prepare；没有运行bazel_lint_changed。Rust语义动作局部
修复未修改Go parser.y，不需要重新生成Go解析器。

正确性和兼容性风险集中于更多未列出的官方输入拼写及未开放的catalog接口。
带符号TIME精度改为与PG18一致的语法拒绝，会改变依赖先前错误接受行为的PG调用方；
MySQL既有拒绝/非负精度行为不变。无时间载体、存储和执行算法改动，无性能优化结论。

## 最终运行记录

从仓库根运行以下命令，全部退出0：

    cargo fmt --all
    python3 -m py_compile pkg/server/testdata/pg_core_libpq.py
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_time_diff -- --nocapture
    # 1 passed；292项配对观察、3项比较器扰动。
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    # 7 passed。
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_time -- --nocapture
    # 7 passed，包含6项前置TIME测试与1项真实差分。
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_temporal_time_precision_grammar -- --nocapture
    # 1 passed。
    make lint
    # 含Go revive与dashboard linter。
    git diff --check
    git diff --cached --check

Ready日志为 `target/time15-ready-final.log`（退出0）；最后一次源码身份核对与差分日志为
`target/time15-provenance.log`（退出0）。最终原始证据位于忽略目录
`target/pg-time-evidence/25248-1793007987026459000/`，包含两端独立观察、identity、summary、
完整错误字段、MySQL三个checkpoint、duration桥接、child-exit及rust-cleanup。
identity绑定实际HEAD、相关源码diff、Cargo.lock、Python夹具和测试二进制SHA256。
最终源码diff SHA256为 `97de3b98d18eabd8a8540c2a93c8ad61e294f28a8f9e7262ee3f45d3f96a5ca8`；
夹具SHA256为 `81acdb9c45d6732b5613eaabf4c2b56ee54d0c0c8895041867a8ee3e895b903f`。
已重新计算并核对源码diff哈希一致；交付说明不改变受测源码哈希。

summary为cases=292、negative_controls=3、passed=true，两端cleanup=true；child exit=0。
rust-cleanup确认database_dropped、postgres_port_closed、mysql_port_closed均true；
外层runner确认reference_port_closed=true。每个阶段实际领取槽位1且锁已释放。

TDD失败证据为 `target/time15-red.log`（退出101，实际1项Rust测试失败）：相同负精度输入，
参考端42601、目标端22023。第一轮恢复遇到夹具把参数长度错误写为八字节，修正为协议规定
四字节后取得上述产品红灯。后续 `target/time15-ready.log` 的累计回归为6 passed/1 failed，
揭示多余参数OID先报08P01的问题；修复早期语法诊断后，原累计命令7 passed。
没有删除失败断言或把零测试、编译通过作为完成证据。

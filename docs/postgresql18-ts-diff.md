# PostgreSQL18 墙钟时间戳真实客户端差分

2026-10-26，任务23在基线 `2b4ab679470962da9dd3caa22ab789d9b9cc0167` 上执行。
依赖批次22已经提交，批次16–22提供的表示、消费者、精度、特殊词及非法载荷实现直接复用。
本项新增 `pg_client_integration_test::pg_temporal_ts_diff` 和 `pg_core_libpq.py` 的
`timestamp-differential` 模式；没有生产代码修改，没有替换受测SQL或模拟时间解析。

## 当前实测

参考服务器 SQL `version()` 为 PostgreSQL **18.4 (Homebrew)**，
`server_version_num=180004`，aarch64-apple-darwin25.4.0，Apple clang 21.0.0，64-bit。
客户端 Python3 ctypes 调用真实 libpq **180000**。目标默认features，由
`CreateAnalyzeSession`、临时双listener及实际session消费者承接，采用测试专用认证。

一个Rust测试承载 **423项配对观察**，七个TS能力ID全部通过；另有行字节、类型元数据、
SQLSTATE三项比较器扰动自检。423是观察数，不是Rust测试数。
DateStyle基线ISO, MDY，TimeZone基线UTC。另测Asia/Shanghai、America/New_York及
相对词时机所需Etc/GMT+12、Etc/GMT-14。目标使用既有 `SET time_zone` / `SELECT @@time_zone`，
参考使用 `SET TimeZone` / `SHOW TimeZone`，这些只设置真实连接环境。
参考IntervalStyle=postgres；目标 `SHOW IntervalStyle` 当前42601，原样记录于identity，
不将该独立会话配置缺口当成timestamp结果编解码通过证据。

主机tzdata为2026d-rearguard，目标chrono-tz 0.10.4内置2025b；参考安装包没有暴露发行标签，
保存其五个受测zone文件SHA256，不将主机tzdata标签当作参考服务器的版本。
identity同时保存实际HEAD、相关源码diff、Cargo.lock、Python夹具和Rust测试二进制SHA256。

## 覆盖和局限

| 能力 | 本次实际断言 |
| --- | --- |
| TS-EPOCH-FRACTION | epoch前后-1/0/1微秒；七位输入归微秒；p=3两侧舍入，p=0正负半秒均离零舍入。 |
| TS-CALENDAR-PRECISION | 闰日；非法日期/月/分钟22008；年末进位；p=0/3/6及p=7告警22023归6。SQL带符号精度在Parse返回42601，参考内部timestamptypmodin负精度另采22023。 |
| TS-RANGE-ZERO | Julian day 0与最后有限微秒、相邻域外、BC/AD、10000年；零日期22008；特殊哨兵与有限边界分开。 |
| TS-ZONE-IDENTITY | 已定型1114忽略合法偏移；DST缺口墙钟四方向保留，Bind后切区再Execute不移动值；列名timezone不改变OID。 |
| TS-SPECIAL | infinity/-infinity、epoch；now/today/yesterday/tomorrow在三个区域四方向对比；参考事务时刻经目标既有SET timestamp接口注入；Prepare常量与Bind参数在跨26小时时区切换后保留各自读入日期，无sleep。 |
| TS-NULL-METADATA | 无声明和p=0/3/6，typed NULL、常量NULL、首行/全NULL、多行、零行；1114/8/p或-1；statement/portal Describe、重复Execute、结果格式切换；NULL长度-1和epoch八字节全零分开。 |
| TS-INVALID-LENGTH | 空串/garbage/allballs为22007，UTF-8/NUL为22021，binary 0/7字节08P01、9字节22P03，域外22008，帧声明不符08P01；Sync后SELECT 1，事务失败E经ROLLBACK回I。 |

规范值和typed NULL覆盖IT→OT、IT→OB、IB→OT、IB→OB（文本/二进制参数到文本/二进制结果）。
检查独立整数微秒、规范文本、原始字节、参数OID、typlen、typmod与NULL。
libpq准备语句/执行结果配合已由libpq认证的独立wire连接，补充portal Describe、嵌入NUL、
声明长度与错误阶段。完整ErrorResponse另存diagnostics，只比较SQLSTATE和协议阶段，
不要求参考构建源码行号或诊断文案相等。

同一MySQL健康连接在PG前、中、后实际更新并读回原值，保留sql_mode=''、NativeType
DATETIME=12/TIMESTAMP=7、Fsp=6及原始列定义和行字节。UTC写入后在三种时区读回：
当前原生runtime两列均显示2024-03-10 02:30:00.000000；没有据此声称TIMESTAMP存取转换
已经符合MySQL服务器。DATETIME显式桥接保留1114和微秒；TIMESTAMP缺少转换来源前提，
两格式均0A000；零日期两格式22008。每次拒绝后SELECT 1成功。这些桥接拒绝与合法PG值成功分开。

非法内部字段构造和反向MySQL桥接由前置独立测试以及本次累计timestamp回归补证；libpq
只能注入wire输入，不能直接构造无效Rust内部对象。参考内部typmod函数接口仅记录参考证据，
目标未开放catalog数组/typmod函数，本项不扩建该子系统。

规范载体为 `pkg/types/pg_temporal.rs::PgTimestamp(i64)`，公开from_micros/micros、
from_binary/to_binary、round_precision、parse_with_context及FromStr/Display。
有限域[-211813488000000000,9223371331200000000)，i64两端为独立无限哨兵。
未使用MySQL位域、总纳秒或text OID冒充PG身份。

## 验证与资源

WIP用于首轮行为验证，Ready用于交付；缺失 `.agents/skills/tidb-verify-profile`，按根协议执行，
不宣称该技能通过。没有Go源、import、顶层Go测试、依赖或Bazel元数据变更；已有工作区，
未发生Bazel错误，不触发bazel_prepare，未运行bazel_lint_changed。
未运行Go包单测，未启用/切换Go failpoints；Rust测试使用计划要求的默认features。

每阶段原子mkdir动态领取共享槽位，实际均为 **1**：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

所有Cargo及子进程继承该路径，阶段退出后释放自己的锁并保留缓存。
参考端用唯一临时目录initdb及回环端口启动，PG18_REFERENCE_DSN缺失时测试明确失败。
Python finally关闭结果与连接；Rust Drop关闭listener并删除唯一数据库；runner停止自己
启动的集群、检查参考端口关闭、删除临时目录。没有操作用户服务或数据。

首轮 `target/ts23-first.log` 退出101，实际1项Rust测试失败：夹具使用未命名portal跨越
Simple Query，参考PG18依法清理portal并返回34000。夹具改用命名portal后，相同产品断言
在 `target/ts23-second.log` 退出0。未发现生产回归，也未制造产品红灯。

Ready确切命令（仓库根目录；由本任务runner设置槽位及隔离参考端环境）：

    cargo fmt --all
    python3 -m py_compile pkg/server/testdata/pg_core_libpq.py
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_ts_diff -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_ts -- --nocapture
    make lint
    git diff --check
    git diff --cached --check

最终运行结果与证据路径见下文。未验证PostgreSQL18.6、持久TiKV、性能、其他时间族、
完整GUC/DateStyle输出样式或完整catalog函数。正确性与兼容性风险仍在未列出的输入形式和
这些范围外接口；本次只有测试及文档变更，无生产算法性能变化。

## 最终运行记录

`target/ts23-ready.log` 完整runner退出0：fmt、py_compile、聚焦差分1 passed、
pg_types_test 7 passed、累计pg_temporal_ts 8 passed（含本差分）、make lint及diff检查均退出0。
最终差分原始证据为 `target/pg-ts-evidence/77757-1793012124766674000/`；前一聚焦
运行证据为 `target/pg-ts-evidence/77727-1793012121345232000/`，均423项配对通过。
summary两端cleanup=true、child-exit code=0；rust-cleanup的database_dropped、
postgres_port_closed、mysql_port_closed均true；runner参考端口关闭检查通过。

源码diff SHA256为 `05f2208b8263401d31fc0308736066e7191178c8873aa5f36b6dca68f262acd0`；夹具SHA256为
`777e7ee7388ca3734340a832bfce1a976332db46bf56278d054f623ff3e6643b`。
已在文档交付前重新计算源码diff并核对与identity一致。实际槽位1的占用记录已删除，缓存保留。
暂存后另执行 `git diff --cached --check`，退出0。

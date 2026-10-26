# PostgreSQL18 DATE 真实客户端差分

2026-10-26 在基线 `9840ec46b55b5932c7f1cdfdc280070d2698c897` 上执行。
批次 7 的提交记录包含日期非法载荷 1 项、累计日期 7 项、类型结果 7 项及
`make lint` 通过；本项另取得下述当前差分证据，未修改日期生产实现。

## 实际环境与结论

真实参考端通过 SQL 查询为 PostgreSQL **18.4 (Homebrew)**，
`server_version_num=180004`，aarch64-apple-darwin25.4.0，Apple clang 21.0.0
(clang-2100.0.123.102)，64-bit。独立客户端加载 libpq **180000**，使用 Python 3
的 ctypes 调用系统 libpq；实际 TCP 连接由 libpq 启动和认证。目标端由
`CreateAnalyzeSession`、默认 features 和临时双 listener 启动，共用真实 session
执行路径与测试存储。认证为测试专用 `InsecureRootOnly`。

结果为 **303 项配对观察通过**，每项分别从参考端、目标端采集后逐项比较；
另有 3 项比较器扰动检查（行字节、OID 元数据、SQLSTATE）通过。一个 Rust 测试
承载上述观察，不能将观察数当作 Rust 测试数。MySQL 同一连接在 PG 流程前、中、后
均完成写入/读回及原始列元数据断言。MySQL 零日期到 PG 两种结果格式的桥接均拒绝
`22008`，错误后 `SELECT 1::int4` 成功。原有日期生产行为满足本次断言，无需修复。

DateStyle 基线为 `ISO, MDY`，另验证 `ISO, DMY`；TimeZone 基线为 UTC，另验证
Asia/Shanghai、America/New_York 和用于确定性跨日的 Etc/GMT+12、Etc/GMT-14。
参考端 IntervalStyle 为 `postgres`。目标 `SHOW IntervalStyle` 实际拒绝 `42601`，
该观察单独归档，没有据此宣称间隔/session GUC 已支持。

时区配置使用前置 DATE-EPOCH 已提供的真实目标接口 `SET time_zone = '…'` 和
`SELECT @@time_zone`；参考端使用 `SET TimeZone TO '…'`、`SHOW TimeZone`。
这是客户端的显式环境设置，未替换日期 SQL、参数或执行结果。
完整 PG GUC 拼写与生命周期不在本项范围。

主机 tzdata 为 `2026d-rearguard`，目标依赖 chrono-tz 0.10.4 内置 IANA **2025b**。
参考 PostgreSQL 使用其安装包的编译时区数据，安装包未暴露发行标签；未把主机
2026d 当作 PostgreSQL 的 tzdata 版本。最终 `identity.json` 保存实际参考包中五个
受测 zone 文件的 SHA256，其中 UTC 为
`fddce1e648a1732ac29afd9a16151b2973cdf082e7ec0c690f7e42be6b598b93`。
跨日时刻来自参考端事务时间，目标通过既有 `SET timestamp` 接口固定到同一瞬时，
不用主机日期或 sleep 推算期望值。

## 能力与断言

| 能力 | 本次实际断言 |
| --- | --- |
| DATE-CALENDAR | 2024/2000 闰日成功；2023/1900 非闰日、4 月 31 日、月 00/13、日 00/32 为 22008；非法 literal 和 Bind 分开采集。 |
| DATE-EPOCH | -1/0/1 日及 1970 年 -10957 日的独立大端字节；切换三个会话时区不改变结果。 |
| DATE-RANGE | -2451545/2145031948 两端及外侧一天；4714 BC、1 BC/AD 邻日、9999/10000 年；有限越域为 22008。 |
| DATE-ZERO | 0000-00-00、0000-01-01、零月/日为 22008；MySQL 自身零日期保留并能读写，桥接两格式均拒绝。 |
| DATE-SPECIAL-STYLE | ±infinity 保留非 NULL 身份与 i32 哨兵；epoch 及 now/today/yesterday/tomorrow 四格式；MDY/DMY 歧义读入四格式与规范回读。相对 literal 在 Parse 固定，参数在 Bind 固定，改变 TimeZone 后不重解。 |
| DATE-NULL-METADATA | typed NULL 参数、NULL::date、首行 NULL、全 NULL、多行、零行；1082/4/-1、参数 Describe、statement/portal Describe、Execute 行字节；NULL 长度 -1 与 epoch 0 分开；重复 Execute、同一命名语句格式切换。 |
| DATE-INVALID-BINARY | 0/3 字节为 08P01，5 字节为 22P03；四字节越域为 22008；帧参数声明长度大于剩余实长为 08P01；非法 UTF-8/NUL 为 22021。逐个 Sync 后恢复 SELECT 1，事务内失败进入 E，ROLLBACK 后回到 I。 |

15 个规范非 NULL 日期和 typed NULL 分别通过 libpq prepared 调用与 wire 完整流程
覆盖 IT→OT、IT→OB、IB→OT、IB→OB。IT/IB 为文本/二进制输入，OT/OB 为文本/二进制
输出。libpq 验证结果 metadata 与实际值；在独立、已由 libpq 认证的空闲连接上，
显式长度消息补测 portal Describe、原始 NULL 长度、嵌入 NUL、错误阶段与重复 Execute。
错误完整字段归档于各端 diagnostics，差分要求阶段/SQLSTATE/恢复一致，不强求源码
文件名、行号和诊断文案逐字相同。成功项的警告列表为空。

内部非法日期构造不属于可通过 libpq 注入的值；前置
`pg_temporal_date_calendar` 对结果编码双格式的独立断言在累计日期回归中重新运行。

## 命令与证据

所有命令从仓库根执行。WIP 用于新增断言迭代；Ready 按根协议执行，仓库缺少
`.agents/skills/tidb-verify-profile`，不宣称运行过该技能。
本任务所有 Cargo 阶段都动态取得共享 **槽位 1**，使用绝对路径：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

每次通过原子 `mkdir target/rust-slot-locks/slot-N.lock` 从 1–10 依次尝试领取，
锁内记录计划、任务、会话和 PID；阶段子进程结束后释放自己的锁，保留缓存。
参考端 DSN 只在进程环境设置，未写入源码、提交或证据。

最终验证命令及结果：

    cargo fmt --all
    # 退出 0；Rust 修改后先格式化。
    python3 -m py_compile pkg/server/testdata/pg_core_libpq.py
    # 退出 0。
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_date_diff -- --nocapture
    # 退出 0；1 passed，303 项配对观察、3 项比较器扰动。
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    # 退出 0；7 passed。
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_date -- --nocapture
    # 退出 0；8 passed（包含差分测试）。
    make lint
    # 退出 0；含 Go revive 与 dashboard linter。
    git diff --check
    git diff --cached --check
    # 均退出 0。

最终差分原始证据位于本机忽略目录：
`target/pg-date-evidence/65491-1793001414822270000/`。
该目录含 reference.json、aster.json、identity.json、summary.json、两端完整错误字段、
MySQL 三个 checkpoint、零日期桥接、child-exit.json 及 rust-cleanup.json。
identity 绑定实际源码 HEAD、工作区相关源码 diff SHA256、客户端文件和 Cargo.lock
SHA256，以及实际运行的 Rust 测试二进制路径与 SHA256。

最终源码 diff SHA256 为
`d63c32be9206eafe63550b7a1f3d34d23f6bfbba47954464ca3a3dfa8b7262a9`；
客户端文件 SHA256 为
`d69f29c48923857645b6a92eb937cf542161676406a6f035851507bf46bb7e8a`。
本文件不影响受测源码哈希。运行日志为 `target/date-diff-provenance.log`、
`target/date-types.log`、`target/date-family.log`、`target/date-lint.log`。

最初缺少 DSN 的环境断言退出 1；后续本机检查发现已安装 PostgreSQL18.4，使用
独立 initdb 数据目录和动态回环端口恢复，无需操作既有服务。首次完整用例运行
退出 101：参考端揭示测试构造中“声明 5 字节”会吸收格式字段而成为合法五字节
date 载荷，实际返回 22P03；将 framing 反例改为明确大于帧余长的 100 字节声明后
验证 08P01。另修正了无参数 SQL 的测试消息参数个数，以及本项预估用例总数。
这些均为测试夹具修正，没有修改生产语义或删除契约断言。日期值实际差分一致。

## 资源、复现与限制

目标临时数据库和双 listener 由 Rust Drop guard 清理；Python finally 关闭参考/目标
libpq 连接及原始消息连接。最终记录确认 database_dropped、postgres_port_closed、
mysql_port_closed 全为 true；外部参考端由本任务自行 pg_ctl 停止并删除独立数据目录，
另检查端口不可连接。没有停止用户服务、读取凭据或删除用户数据。

复现时设置可访问真实 PostgreSQL18 的 `PG18_REFERENCE_DSN`；若没有现成参考端，
可用已安装的 PostgreSQL18 `initdb -D <unique_temp_dir> -U root -A trust --encoding=UTF8
--no-locale` 后，由 `pg_ctl` 在独立回环端口启动。只对该测试集群使用 trust，退出时
停止自己的集群、删除自己的临时目录并验证端口关闭。可选环境
`PG_DATE_REFERENCE_TZDIR` 指向参考安装包的 timezone 目录以记录其实际文件指纹。
领取共享 Cargo 槽位后运行上述测试；不要复用本次已清理的临时端口或数据目录。

未验证 PostgreSQL18.6、持久 TiKV、其他时间族、全 GUC 生命周期和性能基准。
本次没有持久 TiKV 依赖，不把测试存储的行为当作真实 TiKV 结论；没有 Go/Bazel、
manifest 或外部 Rust 依赖变更，不触发 bazel_prepare，未运行 bazel_lint_changed。
兼容性结论仅限表列 DATE 能力及已声明的环境设置接口；IntervalStyle/PG TimeZone
命令拼写缺口仍归对应上游任务。正确性风险主要是未覆盖的更多日期输入拼写；
本项只增加测试与证据，无生产性能改动。

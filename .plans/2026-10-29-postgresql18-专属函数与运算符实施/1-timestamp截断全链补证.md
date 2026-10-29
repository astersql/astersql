# 任务 1: timestamp截断全链补证

批次：【实施批次 1】；语义依赖：无本目录语义前置；互斥前序：无。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已阻塞

目的：date_trunc(text,timestamp)→timestamp，复用 PgTimestamp::truncate，不重写已有实现。

来源任务：大计划8“专属函数与运算符”；路线图第4章“时间截断与提取”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

既有 timestamp 身份、参数和正式 PgDateTrunc 已在；检查列投影/嵌套调用是否仍走同一工厂。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/types/pg_temporal.rs`、`pkg/session/runtime/pg_temporal.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_01_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_01_test;`，新增测试`pg08_01_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_01_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_01_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_01_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

day 跨午夜；2021-01-01 的 week→2020-12-28；40.123456 秒按 milliseconds→40.123；逐项覆盖 microseconds/second/minute/hour/month/quarter/year/decade/century/millennium、负纪元与 infinity；typed NULL；未知单位22023、timezone单位0A000。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

保留MySQL EXTRACT(SECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6)))的整数秒40及MICROSECOND=123456，duration HOUR 27不改为PG interval。date_trunc/date_part在MySQL用合法DATETIME参数检查名称拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_01_test::pg08_01_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_01_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_01_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_01_sql`的TCP断言补充，未知字段不能默认相等。

两个PG连接A/B与一个MySQL连接M在相同Server交错，在流程前/中/后验证M仍执行自身行为与读回真实健康表。参考PG和AsterSQL分别保存原始观测，再比较；MySQL分别与自身参考比较列定义code/flags/charset/length/decimals、值、警告、错误。PG错误记录S/V/C/M及适用D/H/P和实际分析/折叠/执行阶段，不强制所有错误发生于Parse。版本函数等动态身份字段按本文件关系断言，不能任意白名单屏蔽。

先在编号文件列出唯一case ID和预期场景总数；实际按成功/NULL/错误/恢复/零行/绑定/交错分别计数，报告预期、实际、通过、失败、跳过及去重总数。循环次数和字段assert数不冒充业务场景数。差分器至少三项负向自检（修改观测副本的值/OID/SQLSTATE各一次必须失败），与业务用例分开计数。本轮规划有效运行数0，不能预写任何未来通过数。

## 里程碑与恢复清理

里程碑一：取证依赖、定义本任务cases并新增独立测试，得到真实断言失败（或已正确行为当前绿色证据）。缺失类型/事实接口时不以编译错误冒充红灯，不递归扩范围；在本编号记录依赖拥有者、实际缺口和恢复条件后暂停该行为。

里程碑二：只修改令本行为通过的工厂/求值与局部上下文接线，不新建第二求值器；保留已通过签名和MySQL行为。时间stable属性消费执行上下文，immutable路径仅在已证明时折叠，不新增下推许可。每项生产修改记录它对应的来源契约/测试或不可缺少的局部接线依据。

里程碑三：同命令复验、真实客户端差分、Ready检查与交接。错误后扩展协议Sync恢复，再执行`SELECT 1::int4`；事务错误记录E→ROLLBACK→I，确认连接仍可用。错误不能返回假成功行，检查用户数据无副作用。

测试用唯一带PID/端口的库/表/角色名称；记录并在finally恢复TimeZone、locale、search_path、sql_mode及授予权限。停止并wait本任务Python进程，清理其libpq结果/连接、MySQL statement/result/连接，关闭测试Server及连接；确认两个监听端口不可连接、仅自己的测试对象被删除。借用参考服务不停止用户服务；如专门启动参考实例，记录拥有PID与数据目录并在正常/失败/中断三路径停止、删除自己的临时目录。只保留脱敏证据与构建缓存。涉及schema授权夹具先读DDL入口文档及目标包doc.go，不趁测试补建DDL系统。

这些函数不默认需要RealTiKV或全包广扫。若真实缺陷依赖TiKV，先按`docs/rust/rust-postgresql-tikv-runbook.md`和testing-flow限定场景，后台启动playground、记录自己的PID/tag/data、验证后正常/失败/中断清理，不能删除用户服务和数据。

## Cargo 共享槽位规则

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 验证与交付

实施WIP只跑上述聚焦命令；交付Ready按根规则执行`cargo fmt --all`、本任务三条命令、`make lint`、`git diff --check`，必要相邻回归依据实际修改选取并记录精确过滤器。当前`.agents/skills/tidb-verify-profile`缺失，不宣称运行该技能。Cargo fmt及调用Cargo的脚本同样先领槽位。禁止默认`make bazel_lint_changed`；Bazel准备完整触发清单见根AGENTS.md，本计划默认不改Go/依赖/Bazel，如实施触发须先`make bazel_prepare`并收录元数据。Go failpoint单测按testing-flow启用且最终关闭，Go既有失败可独立记录不阻塞本任务Rust交付。

保留PingCAP Apache License；真正修复可用的Rust文件顶部加/保留`// Copyright 2026 AsterSQL.`。完全自建非Go复刻文件不添加PingCAP版权。外部Rust依赖若需移植，只能独立上游提交打tag、所有manifest引用同一个发布tag；禁止vendor/third_party或本地patch，清理临时副本。本任务不默认需要新依赖。

最终按Agent Output Contract报告文件、Ready原因、正确性/兼容性/性能风险、全部确切命令/退出码/有效计数、实际槽位与绝对CARGO_TARGET_DIR、参考构建与清理证据及未验证项。不凭编译或历史证据标完成。缺少本行为必需接口/18.6/客户端证据仍保留任务；无关环境问题只能如实标`已完成，待回归`并保留待验收项，不能宣称已通过。

状态仅用未开始、已完成、已阻塞、已完成，待回归。完成所需验证后有实际代码/测试/配置/文档变更才按`$git-commit`提交本任务，排除其他任务与无关工作区变更；只有编号状态/删除不提交。已完成后按提示删除本编号文件，待回归/阻塞保留；父大计划8保持直到整个工作流实际验收，所有plan.md只读。

## Progress

- [ ] 局部接口取证与当前测试名注册（正式工厂已注册并运行；SQL/客户端已挂载但构建受阻）
- [ ] 失败验证或已有正确行为的当前补证
- [ ] 最小生产接线及通过验证
- [ ] 真实参考/双方言、清理与Ready交付

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。


## 2026-10-29 本轮前置检查与阻塞交接

当前 HEAD：`c3b68dc460ad55ecded44d4131263522fba82ba7`。开始时 `git status --short` 无输出。确认本任务为实施批次1、无本目录语义前置；尚未取得完整生产消费者与当前注册测试的行为证据，不能将历史通过记录作为本轮通过。

阻塞原因：仅检查环境变量存在性（未输出任何连接值）确认 `PG18_REFERENCE_DSN`、`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD` 全部 absent；可选 `PG_LIBPQ_LIBRARY` 也 absent。此结论仅描述本进程环境，不推断本机没有参考服务。本任务要求真实PG18.6与MySQL自身参考，当前无法按规定执行双方言客户端验收。接口/资源拥有者为参考数据库环境配置维护者（执行会话的启动环境提供者）；不是 expression 实现缺口。

已运行只读检查：`git status --short`、`git rev-parse HEAD`；Python `os.environ` 存在性检查；`~/.rustcodegraph/bin/rustcodegraph status`（退出0，11703个索引文件）；`~/.rustcodegraph/bin/rustcodegraph explore 'resolve_pg_signature PgDateTrunc PgTimestamp truncate'`（退出0）；`~/.rustcodegraph/bin/rustcodegraph node resolve_pg_signature`（退出0）。索引显示 `pkg/expression/builtin.rs:2610` 的 date_trunc 候选为 VarString+Datetime→Datetime，调用者包括 `pkg/session/runtime/query.rs::Enter`；这只是静态导航证据，未完成整链当前源码/运行核验。expression/server/parser下未发现doc.go；`.agents/skills/tidb-verify-profile`目录检查退出1，仍缺失。

按 do-task-plans 的阻塞停止规则，在前置阶段停止；未新增测试或客户端夹具，未注册case ID，未运行Cargo或调用Cargo的脚本，未领取共享槽位，实际槽位与CARGO_TARGET_DIR均不适用。WIP三条测试、Ready格式化/lint以及真实参考版本、原始观测、负向自检、协议恢复和双方言隔离均未验证。有效业务运行数0，不构成行为红灯或通过证据。

恢复条件：向后续执行进程提供可用PG18.6参考DSN及MySQL参考连接环境（密码允许按参考配置为空，但须显式配置），确认可用原生客户端库，并确认前序共享文件/参考资源持有者已退出；随后重新核对HEAD和接口，先登记case ID及测试挂载，再动态领取Cargo槽位执行原定WIP/Ready流程。无需重建任何上游子系统。

清理：本轮未启动数据库、Server、Python客户端后台进程，未创建测试对象或槽位锁，无运行资源待释放。只更新本编号文件，保留只读plan.md与父大计划8；依照只有编号状态更新不提交的规则，不创建Git提交。


## 2026-10-29 恢复执行与用例登记

继续调查发现 /tmp/pg05-task14-reference/install/bin/postgres 实际为18.6，MySQL8.0.46和libpq18已安装。已独立initdb启动18.6并查询version及13个截断单位后停止、删除自己的临时目录，退出0。环境变量缺失不再作为不可恢复阻塞。

新增正式工厂测试pg08_01_test::pg08_01_formal，通过lib.rs的cfg(test)挂载。注册唯一ID：F-microseconds、F-milliseconds、F-second、F-minute、F-hour、F-day、F-month、F-quarter、F-year、F-decade、F-century、F-millennium、F-week-boundary、F-negative-epoch、F-infinity、F-null。预期16个场景、1个Rust测试。SQL/客户端case尚未新增，后续先登记再运行；参考探查不算AsterSQL验收。

SQL测试登记：pg_query_test::pg08_01_sql。S-day、S-week、S-milliseconds、S-null、S-zero、S-nested共6个场景；S-invalid与S-timezone各含后续独立恢复查询S-recover-invalid、S-recover-timezone，共10个场景。预期1个Rust测试。普通Q消息校验OID1114/typlen8/typmod-1/列名v及数据或SQLSTATE；扩展协议将在后续登记。正式工厂本轮列表1、执行1通过，16场景通过；槽位1已释放，缓存保留。

客户端登记：pg_client_integration_test::pg08_01_clients启动pg08_01_libpq.py。C-Q-<unit>覆盖13单位，C-Q-week-boundary/negative-epoch/infinity/null/zero/nested共6（合计19）；C-E-nonsense/timezone以及各自C-recover-<unit>为4；C-bind-<empty:0/1>-<input:0/1>-<output:0/1>-<value:0/1/2>共24；M-Q/prepare-<second/microsecond/hour/date_trunc/date_part>共10；I-before/middle/after共3。预期唯一场景60；对端各自采集再比较，A/B重复连接和阶段断言不重复计数。差分负向自检3项单独计数。
构建异常：首次server --list退出101，活动槽位1的fingerprint和owner锁文件在构建中消失，非行为红灯；df -h显示191GiB空闲。正在重新领取槽位聚焦重试。


## 2026-10-29 本轮恢复后的结果与实际阻塞

状态仍为已阻塞，但原因已更正为共享构建缓存不稳定，不是参考环境缺失。无生产修改；测试准备不等于实际实现/验收完成，不标记已完成待回归，也不提交或删除编号文件。

本机可安全启动自有参考：精确PG18.6路径 /tmp/pg05-task14-reference/install/bin，version()为 PostgreSQL 18.6 on aarch64-apple-darwin25.1.0, compiled by Apple clang version 16.0.0 (clang-1600.0.26.6), 64-bit。MySQL /opt/homebrew/opt/mysql@8.0/bin/mysqld 实际8.0.46。分别使用 tempfile 独立目录初始化，PG用trust+loopback、MySQL initialize-insecure+loopback，查询版本后 finally 停止/wait并删除自己的临时数据目录，退出0。不改动用户既有服务。可重用本轮临时编排脚本 /tmp/pg08-01-references.py，其环境只向测试子进程注入，不输出DSN/凭据；脚本未执行完整客户端验收。

已新增独立 pkg/expression/pg08_01_test.rs、lib.rs挂载；pg_query_test.rs追加pg08_01_sql；pg_client_integration_test.rs追加pg08_01_clients并调用新增testdata/pg08_01_libpq.py。前者通过NewFunctionBase，不创建第二求值器。客户端复用既有Libpq/MySQL原生ABI与差分器，登记60场景；还需补充并运行扩展协议TCP逐阶段Describe/Sync、事务E→ROLLBACK→I、完整构建/tzdata身份与恢复证据，不能将当前夹具草稿当成契约全部完成。三层测试未得到完整通过，不运行Ready交付或提交。

实际命令及结果（全部Cargo均先动态mkdir领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，env移除LDFLAGS；未使用其它构建目录）：

- cargo fmt --all：前3轮均退出0；最终补入客户端测试后格式化亦退出0，自有槽位锁正常释放。
- env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_01_test::pg08_01_formal -- --exact --list：退出0，恰好1测试。
- env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_01_test::pg08_01_formal -- --exact --nocapture：退出0，1 passed / 0 failed / 0 ignored，16个有效场景通过；日志 /tmp/pg08-01-formal.log。此为缓存消失之前的本轮真实结果，不能宣称当前server通过。
- env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_01_sql -- --exact --list：两次退出101，均0个有效运行测试，不构成行为红灯。第一次活动槽位fingerprint目录与owner锁文件消失，报No such file or directory；第二次重领槽位重建时backtrace报告librustc_demangle-c03760ef0140cbb9.rmeta不存在。重试日志 /tmp/pg08-01-pg_query_test-pg08_01_sql-list.log；df -h显示191GiB空闲，未证实删除者身份，不能断言是某个其他会话。
- python3 pkg/server/testdata/pg08_01_libpq.py --self-test：退出0，值/OID/SQLSTATE三个篡改均被拒绝，属于差分器自检，不计业务场景。
- git diff --check：退出0；Python AST解析通过。

资源状态：第一轮工厂槽位正常释放；首次server构建结束时发现自己的owner及锁目录已消失，未删除其他锁；第二次server重试正常释放自己的锁。PG/MySQL探查进程已停止并wait，临时目录已删。未运行AsterSQL server测试，不存在本轮测试listener或健康表待清理。保留缓存，不执行cargo clean或删除共享target。

真实外部恢复条件：仓库共享构建目录维护者确认活动rust-slot-*及rust-slot-locks不再被并发清理/替换，然后使用上述临时参考方案继续原定聚焦测试。当前无法从编译日志识别该维护者；需要协调target清理活动而不是获取数据库凭据。恢复时先核对HEAD与共享文件变更，检查最新测试仍注册，再运行原命令，补齐尚缺协议与元数据证据及Ready。已有工作区其他计划改动保持原样。


## 再次恢复：并发与协议补证

只读ps/lsof确认在运行的cargo PID4920与rm PID85477的cwd均在独立pd仓库，不是本仓库；不能据此归因本仓库此前缓存丢失。当前锁目录为空；.cargo/config.toml的默认target-dir被绝对CARGO_TARGET_DIR覆盖，无build-dir覆盖。再次动态领取槽位1，采用CARGO_BUILD_JOBS=1，避免重复原并发配置。

SQL补充登记：S-bind-<empty:0/1>-<input:0/1>-<output:0/1>-<value:0/1/2>共24个，均检查statement/portal Describe、Execute无多余描述、NULL及原始binary；S-transaction-nonsense/timezone各1，检查真实扩展协议错误后Sync状态E、ROLLBACK后I、SELECT恢复。SQL预期总数从10增至36。

正式工厂追加F-invalid与F-timezone两项，预期从16增至18；只检查工厂错误不伪装完整SQLSTATE，SQL/真实参考层负责精确SQLSTATE。

本轮恢复验证：make lint退出0（/tmp/pg08-01-lint.log），CARGO_BUILD_JOBS=1下构建已越过此前失败依赖，仍在编译原生依赖；无新缓存缺失。已补入36场景SQL协议断言与60场景客户端夹具，尚待当前运行结果。参考tzdata源码版本2026c。

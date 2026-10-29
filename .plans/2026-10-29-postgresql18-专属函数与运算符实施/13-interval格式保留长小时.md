# 任务 13: interval格式保留长小时

批次：【实施批次 13】；语义依赖：11；互斥前序：批次12结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：to_char(interval,text) 保持长小时与独立月日字段。

来源任务：大计划8“专属函数与运算符”；路线图第4章“格式化函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

任务11时间token复用；PgInterval三分量已存在，不先转NaiveDateTime。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_time.rs`、`pkg/expression/builtin_string.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_13_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_13_test;`，新增测试`pg08_13_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_13_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_13_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_13_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

36hours HH24:MI:SS→36:00:00；负分量、微秒、月日字段；NULL/空模板按参考；非法或不适用模板以参考实际行为为准，普通字面量不能统一报错。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

保留MySQL DATE_FORMAT的百分号模板、空模板空串和FORMAT的locale/数值规则；to_char以合法MySQL CAST参数检查名称拒绝，PG不得自动开放DATE_FORMAT。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_13_test::pg08_13_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_13_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_13_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_13_sql`的TCP断言补充，未知字段不能默认相等。

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

- [x] 局部接口取证与当前测试名注册
- [x] 失败验证或已有正确行为的当前补证
- [x] 最小生产接线及通过验证
- [ ] 真实参考/双方言、清理与Ready交付（已完成PG61场景、双方言72有效场景、清理与适用Ready检查；待回归6项既有MySQL差分）

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。

## 2026-10-30 当前实施取证与场景注册

HEAD `27fc03a9fb7c9da0b1f76c1fc1fb6ff415e2ad43`；批次11 `912661e50bd3cca102f4a0c77eb90914626d75ba` 的 pg_timestamp_format 与批次12正式工厂/会话消费者可复用。两前序会话均 idle，锁目录空，资源已释放。RustCodeGraph status 索引11772文件；expression/server/session未发现doc.go。真实参考环境变量未配置，但已定位现有PG18.6与MySQL8二进制并以独占临时实例探测，lifecycle证据在target/pg08-13-evidence。未调用Cargo。

新增挂载 pg08_13_test::pg08_13_formal（15场景）、pg_query_test::pg08_13_sql（40场景）及pg_client_integration_test::pg08_13_clients（60去重业务场景），每个过滤器预计恰好1测试。客户端唯一case ID：

I-before, I-middle, I-after, C-Q-hours, C-Q-negative, C-Q-components, C-Q-negative-months, C-Q-fill, C-Q-literal, C-Q-quoted, C-Q-percent, C-Q-empty, C-Q-infinity, C-Q-negative-infinity, C-Q-null, C-Q-null-template, C-Q-zero, C-Q-nested, C-E-signature, C-recover-signature, C-Q-row, C-Q-row-nested, C-Q-row-zero, C-bind-0-0-0-0, C-bind-0-0-0-1, C-bind-0-0-0-2, C-bind-0-0-1-0, C-bind-0-0-1-1, C-bind-0-0-1-2, C-bind-0-1-0-0, C-bind-0-1-0-1, C-bind-0-1-0-2, C-bind-0-1-1-0, C-bind-0-1-1-1, C-bind-0-1-1-2, C-bind-1-0-0-0, C-bind-1-0-0-1, C-bind-1-0-0-2, C-bind-1-0-1-0, C-bind-1-0-1-1, C-bind-1-0-1-2, C-bind-1-1-0-0, C-bind-1-1-0-1, C-bind-1-1-0-2, C-bind-1-1-1-0, C-bind-1-1-1-1, C-bind-1-1-1-2, C-context-utc, C-context-ny, C-isolation, C-txn-signature, C-E-date-format, M-Q-date-format, M-prepare-date-format, M-Q-empty, M-prepare-empty, M-Q-format, M-prepare-format, M-Q-to-char, M-prepare-to-char

生产设计：在既有pg_timestamp_format.rs共享扫描器增加interval字段提供者（PgInterval::components直接分解，保留长小时，禁止NaiveDateTime归一化），builtin.rs只增加interval签名及求值分支。若非法时区模板诊断需补充typed error与协议hint，仅作此行为的局部接线。客户端复用既有Libpq/Mysql观测工具，不创建第二求值器或通用平台。

生成测试脚本首次因Python语法错误未注册，误启动的过滤器返回0测试，已明确作废（阶段88826，不计红灯或通过）。随后修正生成器并真实挂载三个测试；runner增加 --list 恰好1测试硬断言，防止再次执行无效过滤器。该次TERM/INT结束清理记录保留。

注册补充：客户端新增C-E-tzh、C-recover-tzh、C-E-of、C-recover-of，预期去重总数64（原60加4），非法模板检查22007与H，恢复执行SELECT 1。正式工厂已获得运行期红灯：恰好1测试，hours因缺interval签名42883失败，退出101；不是编译/环境错误。

阶段89338三个 --list 均恰好1/退出0，三条执行均恰好1并因hours缺interval签名42883运行期失败/101。客户端已连接真实18.6和MySQL，双方原始观测/身份与清理保存在92452-58761，参考服务临时目录与端口均已清理，slot1释放。

生产修改依据：builtin.rs新增interval,text候选及规范值求值，对应hours/NULL/类型工厂红灯；pg_timestamp_format.rs复用扫描器并直接提供独立月日时间字段，对应长小时/负分量/微秒契约；pkg/types/pg_temporal.rs新增IntervalFormat typed error/22007，pkg/server/pg_conn.rs::temporal_error_hint和pg_extended.rs的Parse/Execute诊断传播对应真实18.6非法模板H字段（不可缺少的局部协议接线）。不新增辅助模块、不改Cargo manifest/依赖/Go/Bazel。

复验发现夹具literal=xyz错误：y为年份token；修正为无token的hello!，参考端同样断言该结果，不用夹具错误充当生产缺陷。补注册C-bind-{0,1}-{0,1}-{0,1}-3共8个场景，客户端预期总数72、SQL协议48：非零行时TZH在Execute报22007/H，Sync后SELECT1；零行仍正确Describe且不求值。正式工厂15场景不变。

阶段96117正式工厂15场景已绿，相邻timestamp检查继续执行中。SQL/客户端发现二进制夹具微秒计算写错（131523123456对应36:32，目标36:02:03.123456应为129723123456），真实PG参考同样拒绝错误期望，已修正wire payload，生产逻辑无需变更；此前该二进制失败不计实现缺陷。

清理审计：首次88826误启动时Ctrl-C打断父参考runner的finally，88798生命周期记录为未清理，遗留自有postgres PID88813。已核对命令行与唯一datadir匹配后pg_ctl immediate stop，端口不可连接且临时目录删除，证据更新recovered_after_interrupted_cleanup。参考/Cargo runner已在finally忽略后续INT/TERM以避免清理重入；后续单独验证TERM。未触碰其他任务的锁、进程、服务。

## 2026-10-30 交付结果与恢复条件

实现完成，状态为已完成，待回归，编号文件保留、不删除。正式工厂15、SQL/协议48、真实PG18.6客户端61场景均通过；完整客户端72预期/72实际/66通过/6失败/0跳过/72去重，差分器值/OID/SQLSTATE三项负向自检通过，单独计数。全部C-*61通过；I-before/middle/after三项双方言交错通过；MySQL to_char名称拒绝Query与Prepare两项通过。分类成功11、NULL5、零行2、错误4、恢复3、绑定32（含四个Execute错误/Sync恢复及四个零行不求值）、上下文3、事务1、MySQL通过2、交错3，合计66通过。原始双方观测与字段/错误阶段在 `target/pg08-13-evidence/98660-59183/`，有效统计见summary.json；Rust数据库/双listener清理见同名.cleanup.json。

唯一剩余6失败为M-Q-date-format、M-prepare-date-format的flags/length差分，M-Q-empty、M-prepare-empty的空模板NULL/空串差分，M-Q-format、M-prepare-format的locale FORMAT既有HY000错误。当前与批次11/12一致，不属于interval增量。接口拥有者为MySQL DATE_FORMAT/FORMAT正式工厂与协议元数据消费方；恢复条件为该拥有者修复对应行为后，配置真实参考并原样重跑pg08_13_clients，不改测试期望或白名单，不将该失败递归扩入本任务。

验证采用WIP聚焦到Ready适用检查（tidb-verify-profile技能路径不存在，未宣称执行技能）。最终证据：阶段96117正式工厂1测试/15场景退出0，timestamp相邻1/11退出0、timestamptz相邻1/16退出0；阶段98386 SQL1/48退出0、client1测试退出101且上述72场景实际运行。所有过滤器--list均恰好1/退出0。cargo fmt --all阶段92638、96117、98386均退出0；make lint阶段96117退出0；git diff --check退出0。两次夹具修正均明确记录，不用它们制造生产缺陷；最初零测试阶段88826完全作废，不计有效数。生产修复后的有效输入未再改动。

所有Cargo阶段动态领取slot1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；独立TERM清理自检动态领取slot2（/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2），未执行Cargo构建，只验证持锁子进程中断清理。两锁均释放，缓存保留。参考版本postgres18.6/server_version_num180006、MySQL8.0.46、原生mysqlclient80046、tzdata2026c；libpq版本、二进制hash、Cargo.lock/fixture/源码diff hash、编码、locale、sql_mode在build/reference/mysql identity原始观测。每个参考实例拥有独立PID/端口/datadir，仅清理本任务对象；正常探测退出0、测试断言失败退出101、Cargo TERM退出143、参考TERM退出143均有清理证据。首次重复中断清理中断已恢复并留记录，所有临时目录不存在且端口不可连接，Python子进程已wait，参考用户服务未停止。

确切命令（仓库根，所有Cargo命令继承上述绝对CARGO_TARGET_DIR并移除LDFLAGS）：

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_13_test::pg08_13_formal -- --exact --list
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_13_test::pg08_13_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_13_sql -- --exact --list
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_13_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_13_clients -- --exact --list
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_13_clients -- --exact --nocapture
    cargo test --locked -p astersql-expression --lib pg08_11_test::pg08_11_formal -- --exact --list
    cargo test --locked -p astersql-expression --lib pg08_11_test::pg08_11_formal -- --exact --nocapture
    cargo test --locked -p astersql-expression --lib pg08_12_test::pg08_12_formal -- --exact --list
    cargo test --locked -p astersql-expression --lib pg08_12_test::pg08_12_formal -- --exact --nocapture
    python3 -m py_compile pkg/server/testdata/pg08_13_libpq.py
    make lint
    git diff --check

commands.jsonl保留每次实际argv/退出码/slot/target/log（先前runner通过环境移除LDFLAGS，与env -u等效；98386采用字面env -u）。delivery.json索引最终证据与文件hash。没有改Go/依赖/Bazel，不触发bazel_prepare；没有运行Go单测，无failpoint启停需求；未运行禁止默认的bazel_lint_changed。未验证完整to_char token族/其他签名、全包/RealTiKV或性能基准。正确性限定本选定模板族；兼容风险为六项既有MySQL差分；性能沿用共享扫描器，未做性能保证。所有plan.md、父大计划8及无关未跟踪计划目录保持不变。下一步由MySQL接口拥有者修复六项并重跑客户端。

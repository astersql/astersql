# 任务 18: 数值locale格式

批次：【实施批次 18】；语义依赖：14；互斥前序：批次17结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：to_char数字D/G/S/L从当前locale读取，固定标点保持不变。

来源任务：大计划8“专属函数与运算符”；路线图第4章“格式化函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

任务14；会话lc_numeric/lc_monetary与可用locale事实门槛。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_time.rs`、`pkg/expression/builtin_string.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_18_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_18_test;`，新增测试`pg08_18_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_18_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_18_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_18_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

C下FM9G999D0的1234.5→1,234.5；德语locale采精确分隔符；L货币和S正负号按真实lc_monetary/lc_numeric；固定.与,不跟随；NULL；缓存跨设置刷新。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

保留MySQL DATE_FORMAT的百分号模板、空模板空串和FORMAT的locale/数值规则；to_char以合法MySQL CAST参数检查名称拒绝，PG不得自动开放DATE_FORMAT。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_18_test::pg08_18_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_18_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_18_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_18_sql`的TCP断言补充，未知字段不能默认相等。

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
- [ ] 真实参考/双方言、清理与Ready交付（本任务PG、清理和Ready适用检查完成；仅保留6项既有MySQL差分待回归）

## Surprises & Discoveries

初始静态取证发现数值/货币locale上下文缺口；用户授权突破后复用既有时间locale机制补齐必要接线。新增测试发现numeric to_char仍会常量折叠，已由运行期红灯证明并修复。当前证据与边界见本文件最终交付记录。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

本任务实现完成，实际PG工厂/SQL/独立客户端证据通过；完整客户端仍有6项既有MySQL差分，标为已完成，待回归并保留本编号。下方按时间保留初始阻塞、授权恢复、红灯和最终验证证据；前述初始暂停记录不代表当前交付状态。所有plan.md仍只读。


## 2026-10-30 前置接口核对与阻塞交接

当前完整 HEAD 为 c4f7539ddb408a72c861e8617b1be17b868269bd。当前确认为实施批次18，语义依赖14；批次14实现提交257bb96f800212e0fa775dd4b2e279af664a1410已保留，numeric格式器最近修改为42009a2d1d6629bc2a6eadfb15505a1a9ccc0dc6。批次17会话01a18951-1ed5-7d81-89ea-fde39743943d为idle，前序14/15/16也为idle；target/rust-slot-locks为空，工作区无前序源码未提交修改（只有无关未跟踪Go对齐计划目录）。前序共享文件与参考资源已释放，互斥门槛满足；这不代表语义事实门槛满足。

已读根与server AGENTS.md、PLANS.md、testing-flow、执行技能及RustCodeGraph技能；目标expression/session/server/parser未发现doc.go。RustCodeGraph status退出0，11784文件；使用explore及node核对生产消费者。现有可复用接口为builtin.rs的numeric/float正式to_char分支、pg_numeric_format.rs::format_numeric/format_float；批次14独立测试通过lib.rs挂载，server已有pg08_14_sql/pg08_14_clients。以上为当前源码事实，不以历史通过记录充当本轮验证。

阻塞原因：本任务明确要求“会话lc_numeric/lc_monetary与可用locale事实门槛”，但全pkg Rust源码搜索这两个名字均无匹配。session/runtime/session.rs只持有pg_lc_time，session/runtime/pg_temporal.rs::pg_temporal_eval_context只注入WithPgLcTime；server/pg_session.rs仅有LcTimeShow/Set/Reset与pg_lc_time桥接。expression/pg_time_locale.rs::Locale::new只创建LC_TIME_MASK | LC_CTYPE_MASK，name只读取MON/ABMON/DAY/ABDAY名称，不提供数值分隔符、正负号或货币符号事实。pg_numeric_format::format_numeric(value, template)与format_float(value, single, template)也没有locale上下文输入。不能用lc_time代替lc_numeric/lc_monetary，不能硬编码德语/C符号冒充真实locale事实，不能以SQL探测特例绕过正式上下文。

接口拥有者：session/runtime的连接设置与事务设置维护方、server/pg_session和conn/runtime的PG设置桥接维护方，以及expression locale事实提供方。批次14只交付numeric模板内核，不拥有或证明上述数值/货币会话事实；批次17只交付时间locale，不能据其提交推导数值locale已存在。恢复条件：由所属接口交付可消费的独立lc_numeric/lc_monetary设置（含有效locale校验、连接隔离、事务/重置规则）与执行时数值/货币符号事实，并保留实际消费者与当前验证证据；随后本任务登记唯一case ID、挂载三个目标测试/客户端夹具，领取槽位后完成真实行为红灯、最小格式接线、同断言绿灯及双方言交付。此处不递归补建locale子系统。

环境只读检查：PG18_REFERENCE_DSN、PG_LIBPQ_LIBRARY、MYSQL8_REFERENCE_HOST/PORT/USER/PASSWORD均未继承；未输出值。前序target/pg08-17-evidence内存在临时参考启动脚本，可在恢复时重新核对复用；本次没有启动参考，也没有运行18.6/MySQL客户端，因此无版本或业务通过证据。

已运行核对命令：`~/.rustcodegraph/bin/rustcodegraph status`、两次explore、按文件node及query（退出0）；`rg -n 'lc_numeric|lc_monetary' pkg --glob '*.rs'`退出1（无匹配）；`rg -n 'fn pg_lc_time|struct TimeLocaleSettings|WithPgLcTime|format_numeric\(|format_float\(' pkg/session/runtime/pg_temporal.rs pkg/expression/exprstatic pkg/expression/builtin.rs pkg/expression/pg_numeric_format.rs`退出0；`rg -n 'pg08_18' pkg/expression/lib.rs pkg/server/pg_query_test.rs pkg/server/pg_client_integration_test.rs`退出1（尚未注册）；`git status --short`、`git log`与`ls -la target/rust-slot-locks`退出0。最终`git diff --check`退出0。

未新增测试/夹具或生产修改；本轮预期场景尚未注册，实际业务运行0、通过0、失败0、跳过0，差分器负向自检0。三个Cargo目标与--list、cargo fmt、make lint、Ready、真实PG18.6/MySQL双方言、缓存刷新/NULL/协议描述与清理实验均未执行；依赖门槛不满足，不能声称行为红灯或当前通过。没有执行任何Cargo或调用Cargo的脚本，因此实际槽位与CARGO_TARGET_DIR均为不适用；没有领取锁、启动子进程/listener/参考实例或创建测试对象，无需资源清理，缓存未改动。

决策：遵守本文件里程碑一及do-task-plans的缺依赖暂停要求，只将本编号标为已阻塞并保留；不修改任何plan.md、来源路线图、覆盖表或父大计划8，不运行git-commit（只有编号状态及阻塞记录）。正确性和兼容性尚未验证，性能未测；下一步由上述接口拥有者补齐事实入口后原样恢复本任务。


## 2026-10-30 授权恢复与TDD注册

用户明确授权突破阻塞，继续实施最小事实接线；该授权覆盖do-task-plans遇阻暂停默认规则。原接口缺口证据保留，但不再以缺少环境变量阻塞；复用前序独立临时PG18.6/MySQL8.0.46启动方式并重新运行，不沿用历史通过数。

计划生产局部接线：在既有TimeLocaleSettings旁增加两项连接私有数值/货币设置，复用begin/finish/savepoint机制；既有EvalContext/静态快照透传两项locale，server以与lc_time相同的正式会话桥接实现SET/SHOW/RESET。新增辅助模块pkg/expression/pg_numeric_locale.rs::symbols/validate，通过独立newlocale与localeconv_l读取真实LC_NUMERIC/LC_MONETARY数据，RAII释放，不改进程setlocale；时间名称模块不适合读取这些类别。pg_numeric_format扩展D/G/S/L及固定逗号，保持既有精确数值/浮点舍入，不新增求值器。正式to_char已有stable防折叠策略继续复用。

已挂载pg08_18_formal（18场景）、pg08_18_sql（52场景）和pg08_18_clients（三个过滤器均预期1 test）。初始复用批次14独立测试/客户端断言，以FM9G999D0和FM999D990在C下验证新增模板；客户端初始74唯一case ID与批次14一致，全部在新夹具CASE_IDS登记，后续locale增量在运行前追加登记。此时尚未修改生产逻辑；下一阶段先--list再真实运行红灯。参考probe使用全新自有临时实例采集C/de_DE.UTF-8下独立numeric/monetary的实际分隔符/正负号/货币输出，清理记录在target/pg08-18-evidence。

新增locale客户端ID（运行前登记），预期总数98：C-Q-locale-de-main, C-Q-locale-de-fixed, C-Q-locale-c-currency, C-Q-locale-de-currency, C-Q-locale-both, C-Q-locale-sign-positive, C-Q-locale-sign-negative, C-Q-locale-sign-trailing, C-Q-locale-padding, C-Q-locale-small, C-Q-locale-zero, C-Q-locale-quoted, C-Q-locale-null, C-Q-locale-template-null, C-context-cache-c, C-context-cache-numeric, C-context-cache-monetary, C-context-cache-both, C-context-txn-lc_numeric, C-context-txn-lc_monetary, C-E-lc_numeric, C-recover-lc_numeric, C-E-lc_monetary, C-recover-lc_monetary。其余74个ID见新夹具CASE_IDS显式列表；差分器3项负向自检另计。

红灯阶段41517：三个--list均为1 test退出0；正式工厂实际得到G123D5而预期1,234.5，SQL相同原始行值错误，真实18.6客户端C-Q-main差分断言失败，三个运行均退出101。不是编译/环境/零测试失败。该证据授权D/G及必要会话上下文接线；扩展14个locale正式工厂/SQL场景到32/66，客户端98场景已登记。参考和槽位已finally清理，red.log与reference-lifecycle记录保留。

阶段44384的首次--list发生freelocale返回类型编译错误，补分号后修正，不计行为红灯。阶段45381正式工厂在新增FoldConstant保持动态上下文断言处失败：既有core_support.rs只保护timestamp/timestamptz，numeric仍会折叠；该运行期证据授权把to_char的防折叠保护扩到数值签名。同时query.rs原来只为时间to_char构造执行上下文，数值to_char必须复用同一上下文以消费新事实；这是不可缺少的局部接线，不扩建其他函数。中断该轮后wait Cargo并释放slot1，参考服务finally清理，避免重复构建已知错误版本。

本轮98个客户端唯一ID完整登记（参考差分运行前）：

I-before, I-middle, I-after, C-Q-main, C-Q-negative, C-Q-spaces, C-Q-zero-fill, C-Q-fraction, C-Q-optional, C-Q-round, C-Q-negative-round, C-Q-overflow, C-Q-zero, C-Q-fill-zero, C-Q-literal-spaces, C-Q-quoted, C-Q-empty, C-Q-null, C-Q-null-template, C-Q-exact, C-Q-zero-row, C-Q-nested, C-E-vdecimal, C-recover-vdecimal, C-E-signature, C-recover-signature, C-Q-row, C-Q-row-nested, C-Q-row-zero, C-bind-0-0-0-0, C-bind-0-0-0-1, C-bind-0-0-0-2, C-bind-0-0-0-3, C-bind-0-0-1-0, C-bind-0-0-1-1, C-bind-0-0-1-2, C-bind-0-0-1-3, C-bind-0-1-0-0, C-bind-0-1-0-1, C-bind-0-1-0-2, C-bind-0-1-0-3, C-bind-0-1-1-0, C-bind-0-1-1-1, C-bind-0-1-1-2, C-bind-0-1-1-3, C-bind-1-0-0-0, C-bind-1-0-0-1, C-bind-1-0-0-2, C-bind-1-0-0-3, C-bind-1-0-1-0, C-bind-1-0-1-1, C-bind-1-0-1-2, C-bind-1-0-1-3, C-bind-1-1-0-0, C-bind-1-1-0-1, C-bind-1-1-0-2, C-bind-1-1-0-3, C-bind-1-1-1-0, C-bind-1-1-1-1, C-bind-1-1-1-2, C-bind-1-1-1-3, C-context-utc, C-context-ny, C-isolation, C-txn-signature, C-E-date-format, M-Q-date-format, M-prepare-date-format, M-Q-empty, M-prepare-empty, M-Q-format, M-prepare-format, M-Q-to-char, M-prepare-to-char, C-Q-locale-de-main, C-Q-locale-de-fixed, C-Q-locale-c-currency, C-Q-locale-de-currency, C-Q-locale-both, C-Q-locale-sign-positive, C-Q-locale-sign-negative, C-Q-locale-sign-trailing, C-Q-locale-padding, C-Q-locale-small, C-Q-locale-zero, C-Q-locale-quoted, C-Q-locale-null, C-Q-locale-template-null, C-context-cache-c, C-context-cache-numeric, C-context-cache-monetary, C-context-cache-both, C-context-txn-lc_numeric, C-context-txn-lc_monetary, C-E-lc_numeric, C-recover-lc_numeric, C-E-lc_monetary, C-recover-lc_monetary


## 2026-10-30 最终交付与待回归项

本任务已完成数值to_char的D/G/S/L执行期locale行为、独立事实入口及最小生产接线，状态为已完成，待回归。本编号保留，不删除，不修改或标完成父大计划8/整个工作流。用户授权突破后已解决本任务自身前置缺口；初始“无接口/未继承参考变量”不再是当前阻塞原因。

WIP阶段48161与Ready阶段50829均重新启动本任务自有PG18.6/MySQL8.0.46独立临时实例。正式工厂32场景、SQL/TCP协议66场景通过，各目标运行有效测试数恰好1，退出0。客户端有效唯一场景预期98、实际98、通过92、失败6、跳过0；非零且全部执行。通过分类为成功30、NULL4、零行2、错误5、恢复4、绑定32、上下文9、事务1、MySQL2、交错3，合计92；预期MySQL8，其余分类均达到预期。PG87项和双方言交错3项全部通过，剩余6项只属于MySQL。差分器值/OID/SQLSTATE三项负向自检独立通过，不混入业务场景计数。

本任务验证覆盖C/de_DE.UTF-8独立numeric/monetary、固定点/逗号、货币符号、前后正负号、填充、小数/零、引用字面量、NULL/模板NULL、零行、round嵌套、真实表投影、四种text/binary参数与结果组合、重复/NULL绑定、statement和portal Describe身份、Execute不重发RowDescription、缓存从C切换至numeric/monetary/二者的刷新、A/B隔离、SET/SET LOCAL/SAVEPOINT/ROLLBACK TO/COMMIT/RESET、非法locale错误后恢复、事务E→ROLLBACK→I。普通SQL实际消费正式工厂，不使用探测文本替换或第二求值器。

六项未通过的客户端ID：M-Q-date-format/M-prepare-date-format（列flags/length不同）、M-Q-empty/M-prepare-empty（DATE_FORMAT空模板仍返回NULL，参考为空串）、M-Q-format/M-prepare-format（既有FORMAT locale返回1105/HY000）。与前序14–17已有记录相同，当前未改动MySQL DATE_FORMAT/FORMAT工厂和MySQL结果元数据逻辑。接口拥有者为MySQL DATE_FORMAT/FORMAT正式工厂及MySQL协议元数据维护方；恢复条件是修复这些范围外既有行为后，以真实18.6/MySQL8原样执行pg08_18_clients，不得跳过或放宽比较器。本任务客户端Rust测试真实退出101，不宣称完整绿色或删除编号。

Ready额外聚焦相邻回归：批次14/15/16/17正式工厂分别18/19/26/13场景，批次17 SQL38场景均退出0；每个过滤器的--list均恰好1 test、0 benchmarks。cargo fmt --all、make lint及git diff --check退出0。verify-profile技能目录仍不存在，因此按任务规定的Ready适用命令执行，没有宣称运行缺失技能。没有Go/Go import/Go module/Bazel元数据变更，不触发bazel_prepare；没有运行bazel_lint_changed、Go单测/failpoint、RealTiKV或全包广扫。最终自审仅修正两处相邻接口注释，语义未变，重新在槽位执行cargo fmt --all与git diff --check，复用已有行为证据。

Ready真实证据目录target/pg08-18-evidence/53041-64410；WIP完整证据50537-64215。PG version18.6/server_version_num180006、libpq180000、MySQL8.0.46、mysqlclient80046、tzdata2026c；reference.jsonl保留编码、locale与TimeZone，mysql-reference.jsonl保留sql_mode/charset/collation；build.jsonl保留真实二进制版本/hash、HEAD、源码diff/Cargo.lock hash及客户端版本。验证基线HEAD c4f7539ddb408a72c861e8617b1be17b868269bd，最终fixture SHA256 205b8318053281b21c57e81010e4a3b22d67d0b8ab7079ed5b1717dea340413e。均为本次重新运行的实例和观测，没有借历史18.4或其他函数族记录充当通过，也未输出/保存DSN或密码。

全部Cargo/调用Cargo脚本阶段均动态领取slot1，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。每轮wait子进程后finally释放锁，保留缓存。正常、断言失败和TERM路径的自有参考均停止wait、临时数据目录删除、两个参考端口关闭；所有reference-lifecycle-*.json的cleanup_verified为true。interrupt-check.py退出0，确认被TERM子执行退出143、槽位释放。Ready Rust cleanup文件确认本任务数据库删除、PG/MySQL listener不可连接；客户端summary确认参考表/库删除、locale/TimeZone恢复、连接关闭。未停止用户服务，未留下自有测试对象、锁或进程；只保留脱敏证据及构建缓存。

变更文件共21项，均属本任务：expression的builtin.rs/context.rs/core_support.rs/exprctx/context.rs/exprstatic/evalctx.rs/lib.rs/pg_numeric_format.rs/pg_numeric_locale.rs/pg08_18_test.rs；session/runtime的session.rs/transaction.rs/control.rs/pg_temporal.rs/query.rs；server的conn.rs/runtime.rs/pg_session.rs/pg_query_test.rs/pg_client_integration_test.rs/testdata/pg08_18_libpq.py；以及本编号。必要性映射：D/G/S/L和固定标点内核对应新值断言；locale事实模块对应独立类别/真实德语符号；EvalContext及其wrapper/static clone对应工厂快照；session状态和事务钩子、server设置桥接对应缓存/隔离/事务与错误恢复；core_support/query对应新增FoldConstant运行期失败和正式SQL执行上下文缺口；其他修改仅独立测试挂载/夹具，无新增依赖或Go移植简化。保留已有PingCAP版权，所有修复生产Rust文件已有AsterSQL标记。

正确性/兼容性边界：当前主机C/de_DE.UTF-8选定模板经过真实验证，完整MySQL隔离回归仍缺上述6项通过证据。没有验证所有系统locale、所有数字模板（例如其他符号组合）、非macOS平台、TiKV持久化或性能基准。locale可用性依赖部署主机；每次求值使用独立locale对象以避免跨连接污染，未做性能基准。非macOS POSIX分支通过线程局部locale和互斥保护localeconv缓冲区，但本机未跨平台构建验证。未扩建完整locale/collation子系统，也未改写外部Rust依赖。

下列Ready命令从仓库根目录执行，均继承上述CARGO_TARGET_DIR；reference-runner注入本任务自有参考连接环境。命令级退出码来自commands.jsonl，不能以外层runner退出0代替客户端退出101：

    cargo fmt --all  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_18_test::pg08_18_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_18_test::pg08_18_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_18_sql -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_18_sql -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_18_clients -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_18_clients -- --exact --nocapture  # exit 101
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_14_test::pg08_14_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_14_test::pg08_14_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_15_test::pg08_15_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_15_test::pg08_15_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_16_test::pg08_16_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_16_test::pg08_16_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_17_test::pg08_17_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_17_test::pg08_17_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_17_sql -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_17_sql -- --exact --nocapture  # exit 0
    make lint  # exit 0
    git diff --check  # exit 0

其他确切验证命令：python3 -m py_compile pkg/server/testdata/pg08_18_libpq.py（退出0）；python3 pkg/server/testdata/pg08_18_libpq.py --self-test（退出0，3项负向自检）；python3 target/pg08-18-evidence/interrupt-check.py（退出0，子执行143）；python3 target/pg08-18-evidence/cargo-runner.py --command cargo fmt --all（退出0，动态slot1）；git diff --check（退出0）。red.log/green1.log/green2.log/green3.log/ready.log、commands.jsonl与reference-lifecycle文件保留全部实际阶段命令/错误/退出证据；green1编译失败与green2主动中断不计行为红灯/通过。

按git-commit技能只提交上述21项任务变更，排除无关未跟踪的Go对齐计划目录，不push。下一步由MySQL接口拥有者修复六项后重跑保留的pg08_18_clients；当前无须继续扩入这些范围外实现。

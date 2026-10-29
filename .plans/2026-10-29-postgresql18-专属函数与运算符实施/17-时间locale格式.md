# 任务 17: 时间locale格式

批次：【实施批次 17】；语义依赖：11；互斥前序：批次16结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：to_char时间TM月份/星期读取当前lc_time。

来源任务：大计划8“专属函数与运算符”；路线图第4章“格式化函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

任务11；工作流会话侧必须供lc_time事实；环境没有德语locale记缺证，不安装通用locale平台。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_time.rs`、`pkg/expression/builtin_string.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_17_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_17_test;`，新增测试`pg08_17_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_17_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_17_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_17_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

C下TMMonth→January；真实可用德语locale采一月/星期字节；FM/TM与字面量、NULL；准备后改变lc_time再执行，另一PG及MySQL不受影响。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

保留MySQL DATE_FORMAT的百分号模板、空模板空串和FORMAT的locale/数值规则；to_char以合法MySQL CAST参数检查名称拒绝，PG不得自动开放DATE_FORMAT。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_17_test::pg08_17_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_17_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_17_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_17_sql`的TCP断言补充，未知字段不能默认相等。

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
- [x] 最小生产接线及通过验证（工厂13、SQL/协议38、PG客户端69当前通过）
- [ ] 真实参考/双方言、清理与Ready交付（Ready检查已执行；6项既有MySQL差分待回归）

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。

## 当前实施取证与阻塞交接（2026-10-30）

本轮为实施批次17，语义依赖11。最新完整HEAD为42009a2d1d6629bc2a6eadfb15505a1a9ccc0dc6；批次11保留提交912661e50bd3cca102f4a0c77eb90914626d75ba及真实注册的pg08_11_test::pg08_11_formal、pg_query_test::pg08_11_sql。批次16会话01a18949-33e9-7d73-8a44-01cb5d72b9e9已idle，其最终交接确认子进程、参考端口、目录及槽位锁已清理；本轮检查target/rust-slot-locks为空。没有并发修改共享文件。工作区另有未跟踪Go逐提交对齐计划目录，本轮不处理。plan.md及所有来源文档保持只读。

阻塞是本任务明确要求的上游lc_time事实接口缺失，而不是缺少TM token本身。RustCodeGraph status成功（11781文件），query lc_time仅找到MySQL TestLcTimeNamesReadOnly；原始全仓Rust检索确认lc_time_names只在MySQL变量注册、工具配置和既有测试中，pkg/sessionctx/variable/sysvar_builtins.rs:1548–1550将其定义为en_US、ScopeGlobal、ReadOnly。不能用此变量冒充PG连接私有lc_time。

实际生产消费者为pkg/expression/builtin.rs:5278–5302：timestamp仅传值及模板到pg_timestamp_format::format_timestamp，timestamptz额外消费Location/PgFixedTimeZone。pkg/expression/exprctx/context.rs::EvalContext具备SQLDialect、Location、PgFixedTimeZone等，但无lc_time事实；pkg/server/pg_session.rs::SessionQuery及PgSession没有lc_time设置/读取，pkg/server/conn.rs的既有pg_date_context只负责日期上下文。pkg/expression/pg_timestamp_format.rs目前复用统一扫描器，token清单为数字日期/时间和区域偏移，没有月份/星期名称和TM；待上游事实就绪后才新增本任务TM/FM消费，不创建另一求值器。

接口拥有者：工作流会话上下文维护者（PG会话设置、TiDBContext/SessionVars及expression EvalContext桥接）。恢复条件：提供连接私有lc_time的设置/读取、合法locale验证和执行阶段向正式工厂传递事实的可消费接口；准备后更改设置须在Execute生效，另一PG及MySQL连接保持隔离。该前置应由拥有者单独交付并提供当前源码/测试证据；本任务不递归补建通用locale平台。恢复后复用上述现有to_char工厂，先注册独立测试/客户端夹具及case ID、预期场景总数，再运行真实红绿验证。

环境只读检查：PG18_REFERENCE_DSN、PG_LIBPQ_LIBRARY、MYSQL8_REFERENCE_HOST、MYSQL8_REFERENCE_PORT、MYSQL8_REFERENCE_USER、MYSQL8_REFERENCE_PASSWORD均未配置；未打印值/DSN。locale -a确认de_DE.UTF-8、de_DE.ISO8859-15、de_DE.ISO8859-1及de_DE存在，因此不是德语locale缺失。真实参考恢复条件为配置PG18.6及MySQL自身参考，或复用已核实版本并带完整清理的专有参考启动方式；本轮没有启动服务或以历史18.6证据冒充当前通过。

已运行只读取证命令：git status --short、git log -6 --format='%H %s'、rg --files -g doc.go pkg/expression pkg/session pkg/server pkg/parser（未找到目标doc.go）、~/.rustcodegraph/bin/rustcodegraph status、explore 'lc_time PgSessionState EvalContext format_timestamp pg_timestamp_format'、query lc_time --limit 10、node --file pkg/expression/pg_timestamp_format.rs --offset 1 --limit 230、node --file pkg/server/pg_session.rs --offset 1 --limit 170、node --file pkg/expression/builtin.rs --offset 5260 --limit 53、node --file pkg/expression/exprctx/context.rs --offset 1 --limit 165；上述图命令退出0。补充rg -n -i 'lc_time|lctime|lc_time_names' pkg --glob '*.rs'与locale -a | rg '^de_DE'确认缺口及本机locale；环境Python仅输出configured/absent。本轮按根规则读取AGENTS.md、PLANS.md、pkg/server/AGENTS.md、testing-flow及两项技能，没有修改它们。

本轮未新增生产代码、测试、夹具或注册；pg08_17相关三个测试尚不存在，不能运行空过滤器制造证据。有效业务测试数0，成功0、行为失败0、跳过0（未启动验证）；case ID/预期场景总数尚未注册，不虚报未来计数。WIP/Ready构建验证未启动，三个cargo目标、cargo fmt --all、make lint及真实客户端均未运行。未运行任何Cargo命令或调用Cargo的脚本，因此未领取槽位，实际槽位及CARGO_TARGET_DIR均为不适用；本轮没有进程、端口、测试对象、锁或临时副本需要清理。

结果：只更新本编号文件为已阻塞，保留所有未完成验收项；不提交只有状态/阻塞记录的变更，不删除本编号。正确性和隔离尚未获得当前行为验证，未评估性能；不能宣称完成、红灯或绿色。恢复依赖后须按原有三层验收和Ready适用检查继续。

## 恢复实施与测试登记（2026-10-30）

用户授权突破阻塞，恢复本任务。只补必要lc_time事实桥接，不建设完整locale平台。新增生产辅助模块计划：pkg/expression/pg_time_locale.rs，lib.rs公开注册，使用系统newlocale/nl_langinfo_l（线程私有locale对象，不更改全局setlocale）读取真实月份/星期名称并校验locale；现有timestamp统一扫描器不具备该功能。需增加已锁定的libc crate直接依赖以安全使用平台常量和ABI，不移植外部仓库、不引入本地覆盖。最小接口接线涉及exprctx/context.rs、exprstatic/evalctx.rs、session/runtime/session.rs与pg_temporal.rs、server/conn.rs、runtime.rs、pg_session.rs；消费既有pg_temporal_eval_context，保守保持to_char稳定不折叠。生产依据均为本编号TM月份/星期、准备后改lc_time、连接隔离验收。

注册formal 7场景，SQL基线37加德语1为38场景，客户端63场景：复用pg08_12_libpq的62个已列CASE_IDS并新增C-context-german。CASE_IDS完整唯一清单存入本任务新夹具；未来扩充前更新清单。三项测试已在真实lib.rs/既有server测试模块注册，尚未运行。测试维持前序MySQL六项差分原断言，不降低验收。

扩充测试登记：formal 9（新增C/德语执行上下文），SQL38，客户端78（原63加C-context-cache-c, C-context-cache-de, C-Q-german-abbrev, C-Q-german-upper, C-Q-german-lower, C-Q-fm-english, C-Q-quoted, C-Q-locale-null, C-Q-locale-template-null, C-Q-locale-zero, C-Q-locale-nested, C-E-locale, C-recover-locale, C-Q-locale-row, C-Q-locale-row-zero），所有ID在夹具CASE_IDS中预先登记；绑定index2改为TM月份/星期，保持四种格式及重复/NULL/零行矩阵。基线期三项有效测试均运行1项/退出101：formal稳定性折叠、SQL德语值、client德语值真实失败。一次新增libc manifest但锁条目未更新造成--list失败，不计红灯；现已补齐同一libc已锁版本条目。

新增登记C-context-transaction（SET/LOCAL/保存点/ROLLBACK）及C-Q-unicode（真实德语三月字节），客户端预期80。formal增加三种德语三月大小写为12场景。新locale状态的事务/保存点接线是防止新增会话设置泄漏的必要局部保护，完全复用IntervalStyle现有生命周期；不扩展事务系统。

WIP formal12场景当前通过，SQL/client仍在德语场景返回January Tuesday。真实消费者定位query.rs:1380仅对timestamptz构建完整执行上下文，timestamp使用空静态上下文。最小修复为该既有分支对to_char(timestamp,text)也调用pg_temporal_eval_context；不修改分析身份或第二求值器。assertionEvalContext补转发PgLcTime，静态快照复制也补明确断言；正式测试加入FMTM/TMFM组合为13场景。协议绑定index2改为TMMonth TMDay以保持同一矩阵的真实locale模板覆盖。所有locale事务处理基于已有IntervalStyle钩子；已读取DDL入口与pkg/ddl/doc.go，只使用既有表夹具。

## 最终交付结果（2026-10-30，取代早期阻塞记录）

实施完成，状态为已完成，待回归；本编号保留，所有plan.md和父大计划8只读。本轮已突破环境与接口门槛，不再以未继承连接环境或缺少旧lc_time接口作为阻塞。仅完整客户端中的6项既有MySQL差分未通过，不能宣称完整Ready绿色。

生产修改对应本任务依据：pg_timestamp_format.rs复用统一扫描器实现MONTH/MON/DAY/DY及TM/FM、引用文字和C输入collation大小写；新pg_time_locale.rs从系统独立locale对象获取真实名称，编码转为UTF8，不调用全局setlocale；expression/Cargo.toml及Cargo.lock只增加已锁libc的直接使用，无版本变更/外部依赖移植/本地覆盖。builtin.rs是同一to_char求值分支传递lc_time；core_support.rs仅timestamp/timestamptz格式禁止准备阶段折叠，numeric/integer/float与interval路径保持原策略。exprctx/context.rs、exprstatic/evalctx.rs及context.rs分别为事实入口、快照复制和装饰器透传。session/runtime/session.rs与pg_temporal.rs保存私有事实并装入执行快照，query.rs使timestamp格式消费该快照；control.rs与transaction.rs按既有IntervalStyle钩子保存事务/保存点并回滚新设置，防止新增状态泄漏。server/conn.rs、runtime.rs、pg_session.rs为真实session worker提供SHOW/SET/LOCAL/RESET及locale合法性检查，未改目录查询解析器、OID猜测、参数协议或第二求值器。所有修改Rust文件保留AsterSQL及原有PingCAP Apache头。

最终工厂13场景、SQL/协议38场景通过，各exact过滤器有效1测试。真实客户端预期80、实际80、去重80、通过74、失败6、跳过0；PG客户端69全部通过，另MySQL名称拒绝2和双方言交错3通过。通过分类为success21、NULL7、zero4、error3、recovery2、binding24、context7、transaction1、mysql2、interleaved3，总数74；独立差分器负向自检3通过，不计业务总数。准备后切换C/de_DE.UTF-8、另一连接保持C、三月非ASCII字节、真实行源、typed NULL、零行、四种格式及重复/NULL绑定、事务LOCAL/保存点/回滚均有实际双方观测。statement/portal Describe的OID25、typlen/typmod -1、列名v及Execute不重发描述由SQL TCP测试验证，错误S/V/C/M与适用D/H/P、Sync恢复及E→ROLLBACK→I保留。

缺证为同前序的6项MySQL差分：M-Q-date-format/M-prepare-date-format值正确但metadata flags128/length124与Aster flags1/length440不符；M-Q-empty/M-prepare-empty参考实际返回NULL，任务保留契约要求空串；M-Q-format/M-prepare-format在Aster返回1105/HY000，参考locale成功。没有屏蔽字段、改断言或吞并MySQL子系统。拥有者为MySQL DATE_FORMAT/FORMAT正式工厂及元数据/兼容契约维护者。恢复条件是协调空模板契约并修复既有元数据/locale行为，然后原样重跑本编号客户端；因此保留本编号，不删除、不标全部通过。

Ready使用本任务明列检查（.agents/skills/tidb-verify-profile不存在）：fmt、lint、diff-check退出0，本任务formal/SQL退出0、client退出101（仅上述六项）；相邻timestamp工厂11场景与timestamptz工厂16场景通过，各有效1测试。WIP→Ready原命令及全部迭代退出码/日志在target/pg08-17-evidence/commands.jsonl；首次三层运行均有效1测试且运行期失败，不能以中间锁条目/参数修正的编译失败计红灯。未改变Go/import/Bazel/Go模块，不触发bazel_prepare；未运行bazel_lint_changed、Go单元/failpoint或RealTiKV。

最终真实参考证据为target/pg08-17-evidence/38124-63124：PG18.6/server_version_num180006、libpq180000、MySQL8.0.46/mysqlclient80046、tzdata2026c；reference.jsonl和mysql-reference.jsonl保留version、locale、编码、TimeZone、sql_mode，build.jsonl保留二进制版本/hash、源码diff和Cargo.lock/hash。测试基线HEAD42009a2d1d6629bc2a6eadfb15505a1a9ccc0dc6，fixture SHA256 f7fde236f5bc766d9066a040ffb7f257d0b8e6b2896ea17902e2551dab25baf4。参考为本任务独立临时实例，不是历史绿色；早期green6证据37861-63039同样保留真实失败。

所有Cargo及调用Cargo脚本均动态领取槽位1，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；每轮结束wait子进程后释放锁，缓存保留。正常、断言失败及TERM中断路径都验证自有PG/MySQL实例已停止wait、端口关闭、临时目录删除；interrupt-check.py退出0并验证子执行退出143、槽位释放。全部reference-lifecycle-*.json的cleanup_verified为true。最终Rust cleanup确认数据库删除、双listener不可连接；客户端summary确认参考表/库删除、locale/TimeZone恢复、连接关闭。没有停止用户服务，没有未清理的自有锁/进程/对象；仅保留脱敏证据和构建缓存。

风险与边界：C输入collation及C/de_DE.UTF-8的选定时间locale名称经过真实验证；其他collation的语言大小写、非Unix平台、全部系统locale、其他to_char模板、TiKV持久化和性能基准未验证。系统locale可用性依赖部署主机；实现使用独立newlocale对象保护并发隔离，但未做性能基准。不扩建通用locale/collation子系统。唯一未完成交付检查是上述范围外MySQL差分，不存在本任务PG行为缺证。

确切Ready命令均从仓库根目录在上述槽位、references-runner注入的自有参考环境执行，所有--list均1 test, 0 benchmarks且退出0：

    cargo fmt --all  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_17_test::pg08_17_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_17_test::pg08_17_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_17_sql -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_17_sql -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_17_clients -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_17_clients -- --exact --nocapture  # exit 101
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_11_test::pg08_11_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_11_test::pg08_11_formal -- --exact --nocapture  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_12_test::pg08_12_formal -- --exact --list  # exit 0
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_12_test::pg08_12_formal -- --exact --nocapture  # exit 0
    make lint  # exit 0
    git diff --check  # exit 0

其他验证命令：python3 -m py_compile pkg/server/testdata/pg08_17_libpq.py（退出0）；python3 pkg/server/testdata/pg08_17_libpq.py --self-test（退出0、3负向自检）；python3 target/pg08-17-evidence/interrupt-check.py（退出0，TERM子执行143）；git diff --check（退出0）。各WIP相同命令、外层参考启动命令及实际退出详情保留在red.log、green*.log、ready.log、commands.jsonl和reference-lifecycle文件，不混淆脚本退出0与其中client测试退出101。

本次交付包含本编号、Cargo.lock、expression manifest/locale/工厂/上下文/扫描器/测试挂载、session事实与事务钩子、server设置桥接与三层测试/客户端夹具。只按git-commit提交本任务明确文件，排除无关Go对齐计划目录。下一步由MySQL行为拥有者解决六项差分后回归本编号，不将父计划或全工作流标完成。

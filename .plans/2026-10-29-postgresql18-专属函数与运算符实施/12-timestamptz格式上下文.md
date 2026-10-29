# 任务 12: timestamptz格式上下文

批次：【实施批次 12】；语义依赖：2、11；互斥前序：批次11结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：to_char(timestamptz,text) 格式化执行时墙钟及偏移。

来源任务：大计划8“专属函数与运算符”；路线图第4章“格式化函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

任务11模板与任务2命名区域上下文；稳定上下文不得按immutable折叠。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_time.rs`、`pkg/expression/builtin_string.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_12_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_12_test;`，新增测试`pg08_12_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_12_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_12_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_12_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

UTC→2024-01-02 01:30 +00:00、New_York→2024-01-01 20:30 -05:00；春秋偏移、NULL、infinity；Prepare后SET再Execute；timestamp对照不随时区变化。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

保留MySQL DATE_FORMAT的百分号模板、空模板空串和FORMAT的locale/数值规则；to_char以合法MySQL CAST参数检查名称拒绝，PG不得自动开放DATE_FORMAT。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_12_test::pg08_12_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_12_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_12_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_12_sql`的TCP断言补充，未知字段不能默认相等。

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
- [ ] 真实参考/双方言、清理与Ready交付（本任务PG通过；既有6项MySQL差分待回归）

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

2026-10-30：本任务timestamptz执行时格式上下文已实现，正式工厂16场景、SQL协议42场景和真实PG18.6客户端51场景通过。完整客户端62项中56通过、6项既有MySQL失败、0跳过，状态为已完成，待回归。参考/客户端原始观测与清理证据保留在target/pg08-12-evidence；编号文件保留，所有plan.md只读。


## 2026-10-30 本轮前置检查与阻塞交接

本轮确认是实施批次12，语义依赖为批次2及11，整体plan.md仅作只读架构指南。当前完整HEAD为 `912661e50bd3cca102f4a0c77eb90914626d75ba`。批次11会话 `01a1890e-66ae-7e82-b962-b9fff053a6da` 在list_threads中为idle；进程检查未发现Cargo或批次11客户端进程，target/rust-slot-locks为空。前序共享文件及构建资源已释放。本轮未发消息至其他会话，不并行修改共享源码。

已读取do-task-plans、仓库skills/rustcodegraph/SKILL.md、根AGENTS.md、pkg/server/AGENTS.md、PLANS.md、testing-flow、本编号与本目录plan.md。expression、session、server、parser的doc.go均未发现；tidb-verify-profile/SKILL.md仍不存在。RustCodeGraph status退出0，索引11770文件；explore和精确node/query退出0。正式入口resolve_pg_signature位于builtin.rs:2610，图中消费者包含session/runtime/query.rs::Enter与sql_dialect。图已发现pg08_02_formal、pg08_02_sql及pg08_09_formal；当前源码保留批次11的to_char有界(timestamp,text)候选、正式factory及query.rs三个名称入口，最近提交就是批次11。批次11任务记录为已完成，待回归，其既有MySQL差分失败不扩入本任务。以上只是静态定位，尚未完整取证命名时区及模板上下文接口，也没有把历史通过记录视为当前通过。pg08_12的图查询没有结果；未新增或宣称注册本任务测试。

阻塞：使用Python只检查环境变量存在性，不打印任何值，PG18_REFERENCE_DSN、MYSQL8_REFERENCE_HOST、MYSQL8_REFERENCE_PORT、MYSQL8_REFERENCE_USER、MYSQL8_REFERENCE_PASSWORD均unset；可选PG_LIBPQ_LIBRARY也unset。进程列表虽有既存postgres实例，但尚无本任务必需的参考连接配置与版本验证，不能把既存进程或其他任务证据当成本轮真实18.6/MySQL验收。没有将环境缺失、未运行、零测试或编译错误当成行为红灯。

环境接口拥有者是工作区参考环境提供者；生产接口所有者分别为expression（正式签名与格式化求值）、types（规范时态及命名区域）、session（分析及执行上下文）、server（协议及独立客户端）。本轮没有断言这些生产接口缺失。恢复条件为向执行会话提供可用PG18_REFERENCE_DSN及MYSQL8_REFERENCE_*配置（密码允许显式空值），可用libpq及原生MySQL客户端库，并实际确认PG18.6及MySQL自身参考身份。恢复后重新核对HEAD/资源持有者，从里程碑一继续当前接口取证、登记唯一case ID与预期数、新增并挂载三项测试/夹具，再动态领取槽位执行原定TDD及Ready；不得凭本记录跳过测试。

遵守do-task-plans的“遇到阻碍（缺少依赖、测试失败、指令不明确）立即停止执行”，本轮仅更新本编号文件状态和证据。没有生产、测试、配置或其他文档修改；不修改任何plan.md、父大计划8、路线图或覆盖表；不提交、不删除本编号文件。原有无关未跟踪计划目录保持原样。

检查命令：git status --short、git log -8 --format='%H %s'、ls target/rust-slot-locks、ps -axo pid,ppid,command配合rg筛选、rustcodegraph status/explore/node resolve_pg_signature/query PgTimeZone/query pg08_12/query pg08_02/query pg08_09_formal，以及Python环境存在性检查；这些检查所在调用均退出0。最终执行git diff --check及本编号diff审阅。未运行WIP三项测试、Ready cargo fmt/三项复验/make lint、真实参考或客户端：本轮有效行为场景数0，无通过/失败/跳过计数，无fixture/hash、原始协议观测、负向自检或双方言清理证据。正确性、兼容性及性能尚未验证。

未执行Cargo或调用Cargo的脚本，实际槽位为未领取，CARGO_TARGET_DIR未设置。未启动Server、Python客户端或参考服务，未创建库表、监听或临时目录，无本任务资源需要清理；未停止既有参考服务。没有Go/依赖/Bazel变更，未触发bazel_prepare，未运行RealTiKV或广扫。完成证据不足，状态保持已阻塞而非已完成或已完成，待回归。


## 2026-10-30 授权恢复与测试登记

用户已授权遇阻突破，优先于技能默认暂停规则。变量未继承不再视为外部阻塞；参考二进制已定位为software/postgresql-18.6-reference/install/bin与/opt/homebrew/opt/mysql@8.0/bin，将启动独立临时实例并finally停止wait、检查端口及删除自己的目录。前序11参考生命周期已验证独占实例全部清理。当前仍未完成，状态待取得实际证据再更新。

复用接口：types::PgTimestampTz::extract_float8_in_zone("timezone",Location,PgFixedTimeZone)按实际瞬间取命名区/固定区偏移；micros与PgTimestamp::from_micros提供规范墙钟载体。expression::pg_timestamp_format::format_timestamp已有模板扫描器；builtin.rs::resolve_pg_signature及正式factory已有timestamp签名；core_support::foldConstant已有其他timestamptz稳定函数保护；session/query.rs三个to_char入口已接通，不重复修改。

先新增pg08_12_test.rs并在expression/lib.rs挂载pg08_12_formal（16场景），pg_query_test.rs追加pg08_12_sql（42场景：17普通SQL、24绑定、1事务错误/恢复），pg_client_integration_test.rs追加pg08_12_clients并启动pg08_12_libpq.py。客户端预期62去重场景，独立差分自检3项。唯一case ID为：I-before, I-middle, I-after, C-Q-utc, C-Q-ny, C-Q-spring-before, C-Q-spring-after, C-Q-fall-before, C-Q-fall-after, C-Q-kathmandu, C-Q-microseconds, C-Q-empty, C-Q-infinity, C-Q-negative-infinity, C-Q-null, C-Q-null-template, C-Q-zero, C-Q-nested, C-Q-timestamp-UTC, C-Q-timestamp-America/New_York, C-E-signature, C-recover-signature, C-Q-row, C-Q-row-nested, C-Q-row-zero, C-bind-0-0-0-0, C-bind-0-0-0-1, C-bind-0-0-0-2, C-bind-0-0-1-0, C-bind-0-0-1-1, C-bind-0-0-1-2, C-bind-0-1-0-0, C-bind-0-1-0-1, C-bind-0-1-0-2, C-bind-0-1-1-0, C-bind-0-1-1-1, C-bind-0-1-1-2, C-bind-1-0-0-0, C-bind-1-0-0-1, C-bind-1-0-0-2, C-bind-1-0-1-0, C-bind-1-0-1-1, C-bind-1-0-1-2, C-bind-1-1-0-0, C-bind-1-1-0-1, C-bind-1-1-0-2, C-bind-1-1-1-0, C-bind-1-1-1-1, C-bind-1-1-1-2, C-context-utc, C-context-ny, C-isolation, C-txn-signature, C-E-date-format, M-Q-date-format, M-prepare-date-format, M-Q-empty, M-prepare-empty, M-Q-format, M-prepare-format, M-Q-to-char, M-prepare-to-char。所有测试先注册后运行；本节写入时行为有效数仍为0。

生产修改设计（红灯后才实施）：builtin.rs仅增加(timestamptz,text)签名并在既有PgTimestampToChar分支区分源类型；pg_timestamp_format.rs沿同一扫描器加可选偏移及TZH/TZM/OF token，复用types现有瞬间偏移转墙钟，不新增辅助模块或第二求值器；core_support.rs仅保护本签名源参数0为timestamptz的stable折叠。以上依据本任务UTC/NY/DST、Prepare-SET-Execute与timestamp对照契约。不修改MySQL实现或其他签名。

WIP第一阶段动态领取slot1；cargo fmt退出0，三个过滤器--list各有效1项/退出0。formal与sql各执行1项/退出101，分别42883缺少timestamptz签名及普通SQL签名拒绝。客户端首轮在获取Aster SHOW TimeZone时夹具IndexError，不能算行为红灯；已改用批次9的PQparameterStatus读取真实会话时区并重跑客户端。TCP新增命名statement在一轮后Close，避免SET简单查询清理unnamed或重名Parse干扰业务。参考独占PG/MySQL均停止wait、端口关闭、目录删除；第一轮slot锁已释放。

客户端修正读取时区后再次list有效1项/退出0，执行1项/退出101：真实PG18.6首项UTC成功返回2024-01-02 01:30 +00:00，Aster返回42883，得到客户端行为红灯。三项均已取证，之后按已登记设计仅修改builtin.rs、pg_timestamp_format.rs、core_support.rs；不修改session/server生产代码。所有独占参考及slot1已finally清理。开始同命令复验。

复验：formal1项/16场景通过，timestamp相邻formal1项/11场景通过；SQL及真实客户端均在NY用例返回UTC值，出现真实上下文红灯。定位query.rs::relational_query_expression_value既有时间函数分支仅以返回类型或参数1是否timestamptz选择pg_temporal_eval_context，而to_char源在参数0。这一局部接线必需：保留返回类型判断，将候选参数判断改为contains(TypePgTimestampTz)，覆盖本签名执行时区，仍由正式工厂求值，不扩展新路径。make lint已退出0（本轮之后无Go变更，结果可复用）。

第二次NY仍失败，进一步核对批次9夹具，发现前两次NY测试使用SET TimeZone（Aster尚未接此PG拼写，命令成功但没有修改time_zone），故这些结果不能单独证明执行上下文修复有效。已改用既有上游接口：参考端SET TIME ZONE，Aster端SET time_zone；不扩修设置系统。query.rs的参数0上下文接线仍由实际类型路径证明必需，接下来用真实设置重验，并保持同一业务输入/期望。


## 2026-10-30 当前交付证据与剩余回归

本节取代早期环境阻塞状态。所选to_char(timestamptz,text)执行时墙钟/偏移已实现且有当前真实证据。生产修改精确范围为expression/builtin.rs有界签名及既有求值分支、expression/pg_timestamp_format.rs同一扫描器的时区偏移token及墙钟转换、expression/core_support.rs源参数0的stable常量折叠保护，以及session/runtime/query.rs时间函数分支按候选参数包含timestamptz注入既有执行上下文。每项均对应上文登记的本签名契约或不可缺少的局部接线。没有修改其他时间函数、MySQL实现、server生产协议、Cargo依赖或Bazel元数据。

最终有效证据为 `target/pg08-12-evidence/86356-58250/`：客户端预期62、实际62、通过56、失败6、跳过0、去重62。通过PG成功13、NULL5、零行2、错误2、恢复1、绑定24、上下文/连接隔离3、事务错误恢复1，共51场景；另通过MySQL to_char名称拒绝2、PG A/B与MySQL M交错及健康表读回3。值/OID/SQLSTATE差分负向自检独立3项通过，不计入62。SQL测试42场景覆盖statement/portal Describe的OID25、typlen=-1、typmod=-1、参数1184/25、四种参数/结果格式、重复/NULL绑定及零行、Execute不再发RowDescription、Sync恢复、事务E→ROLLBACK→I。正式工厂16场景及相邻timestamp正式工厂11场景通过。

UTC与NY日期翻转、春季前后-05/-04、秋季重复01:30的-04/-05、Kathmandu +05:45、微秒、NULL模板/源、空模板、正负infinity、嵌套date_trunc、真实行源及零行均获当前PG18.6对照；同一Parse参数及常量语句在SET后Execute读取当前时区，A/B连接隔离已验证，timestamp对照不随区变化。本轮没有将早期错误SET拼写或环境失败记为通过证据。

待回归6项为M-Q-date-format、M-prepare-date-format、M-Q-empty、M-prepare-empty、M-Q-format、M-prepare-format。直接DATE_FORMAT值正确而列flags/length不同（参考128/124，Aster1/440）；空模板参考8.0.46为NULL而任务/Go保留契约为空串；FORMAT(...,'de_DE')参考成功、Aster1105/HY000。这些与批次11一致，且本次未修改其生产路径。没有屏蔽metadata字段、删断言或改变空模板期望。接口拥有者为expression的MySQL DATE_FORMAT/FORMAT实现及session的MySQL投影描述；需相应任务解决既有差异后以原客户端命令复验至62通过。按用户允许因无关回归失败交付规则，标已完成，待回归，保留本编号文件，不宣称完整Ready通过。

Ready适用检查：`cargo fmt --all`退出0；下列三个原定过滤器的`-- --exact --list`各退出0且有效1项，执行命令分别退出0、0、101；`make lint > target/pg08-12-evidence/make-lint.log 2>&1`退出0；`git diff --check`退出0。未提供tidb-verify-profile技能，按本编号明确的Ready清单执行，未宣称调用缺失技能。

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_12_test::pg08_12_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_12_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_12_clients -- --exact --nocapture

相邻回归为`env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_11_test::pg08_11_formal -- --exact --nocapture`，先同过滤器--list有效1项/退出0，再执行1项/退出0。WIP所有红灯/夹具及上下文迭代、Ready确切命令/退出码/日志记录于target/pg08-12-evidence/commands.jsonl。所有Cargo调用及产物子进程阶段均动态领取slot1，绝对CARGO_TARGET_DIR为 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；均等子进程结束再释放自己owner/slot锁，保留构建缓存。

参考实际为PostgreSQL18.6（server_version_num180006）、MySQL8.0.46，libpq180006及原生MySQL客户端80046，tzdata2026c；build.jsonl记录当前HEAD、fixture/二进制/Cargo.lock/源码diff hash、configure及tzdata hash，reference/mysql-reference原始观测记录locale、编码及sql_mode，不含DSN/凭证。独占参考实例使用动态端口与临时数据目录；reference-lifecycle-*.json记录停止wait、端口关闭和目录删除。最终客户端summary确认自己的参考表/库删除、时区恢复和连接关闭，Rust同名cleanup.json确认自己的数据库删除及两个监听关闭。已覆盖断言失败路径，另对参考启动器执行TERM中断清理自检；未停止用户既有PG服务。临时启动脚本和本任务pyc交付前删除，保留脱敏证据和编译缓存。

正确性风险限于未覆盖的完整to_char模板和极端日期/历史偏移，不扩大至其他签名或locale主题；兼容性风险是上述6项既有MySQL差分仍失败；没有性能基准，执行时增加单次现有偏移查询，未增加下推许可。没有运行全包、RealTiKV或禁止默认的bazel_lint_changed；无Go/依赖/Bazel变化，不触发bazel_prepare，不涉及Go failpoint单测。本任务实际变更按git-commit技能单独提交，排除无关未跟踪计划；所有plan.md及父大计划8保持只读。

交付自审对照本地PG18.6 formatting.c:2772–2796发现新加时区token的FM细节：TZH/TZM始终两位，仅OF的小时受FM影响；OF仅有偏移秒也保留分钟段。为避免新代码引入差异，先扩充既有formal kathmandu场景模板为HH24:MI OF TZH:TZM FMTZH:FMTZM FMOF（仍16场景，不新增客户端场景），先取得FM断言红灯再修本次新增的三行格式化逻辑。当前最终证据需待该修正后原命令重验更新。

FM补充formal --list有效1项/退出0，运行1项退出101，kathmandu实际+5:45而期望+05:45，取得新增代码细节的行为红灯。现已按PG18.6源码令TZH/TZM固定两位、OF按FM处理小时且非整小时偏移保留分钟段；开始最后一次fmt及同命令复验。参考启动器正常关闭自检退出0，TERM中断自检退出143，均finally释放slot1并记录参考端口关闭、目录删除。

最终补充：FM回归已由红转绿，最终阶段83741的fmt退出0、四个list均有效1项/退出0，formal1项/16场景与SQL1项/42场景及timestamp相邻回归1项/11场景均退出0；client1项退出101，最新85334-58089仍为62实际、56通过、6既有MySQL失败、0跳过，PG51全通过。全部参考生命周期及Rust cleanup已再次只读确认；slot1锁不存在，临时目录已删除且端口不可连接。正常参考关闭自检退出0、TERM自检退出143，均无泄漏。交付汇总/命令/文件hash在target/pg08-12-evidence/delivery.json；临时runner与本任务pyc已清除，只保留脱敏证据和缓存。编号状态保留已完成，待回归，不删除，父计划与所有plan.md未修改。

暂存diff自审发现MySQL DATE_FORMAT期望字符串被夹具生成过程误替换为PG行源的NY格式，早前该两项失败先在值断言发生，不能声称已实际比较到metadata。已恢复原始期望2024-01-02 03:04:05.123456并修正夹具docstring任务号，仅重跑客户端（Rust生产/测试均未变化，formal/sql/相邻及fmt证据可复用）。历史观测保留，最新交付以修正后客户端为准，不用夹具误报充当既有失败。

客户端夹具修正后的最终复验：阶段86317的client --list有效1项/退出0，执行1项/退出101；最新target/pg08-12-evidence/86356-58250/为62实际、56通过、6既有MySQL失败、0跳过，PG51仍全通过。DATE_FORMAT直接查询明确失败于metadata差分，原生Prepare/Execute双方值相同而flags/length分别128/124与1/440，其余空模板及FORMAT失败与批次11相同。真实双方观测、身份/hash及cleanup均已更新核验，槽位锁已释放、参考临时目录已删除、双端口不可连接。没有Rust或SQL测试变更，最终83741阶段formal/sql/相邻及fmt证据仍适用。最终交付manifest已更新为此证据，仅本编号保留待回归，不宣称全Ready通过。

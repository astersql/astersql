# 任务 33: ILIKE匹配与转义

批次：【实施批次 33】；语义依赖：无本目录语义前置；互斥前序：批次32结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：PG ILIKE/NOT ILIKE按自身collation和ESCAPE执行，保留已定AST。

来源任务：大计划8“专属函数与运算符”；路线图第4章“运算符调用契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

大计划2 AST/ESCAPE存在标志与大计划3 collation；现有IlikeSig偏ASCII/Go语义，不能直接视为PG Unicode等价。MySQL既有ILIKE扩展与路线图拒绝目标冲突须先明确归属，不擅自删Go行为。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_ilike.rs`、`pkg/parser/ast/lib.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_33_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_33_test;`，新增测试`pg08_33_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_33_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_33_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_33_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

Ab ILIKE a%→true、NOT ILIKE→false；A_b配a!_b ESCAPE !→true；省略与ESCAPE空串分别检验；NULL；非法多字符escape/尾部escape采参考错误；NOT/AND/比较优先级；选C及可用非ASCII确定性locale。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

按本任务专属场景检查MySQL自身运算、sql_mode、collation、警告与元数据，不给MySQL发PG cast语法来冒充语义拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_33_test::pg08_33_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_33_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_33_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_33_sql`的TCP断言补充，未知字段不能默认相等。

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
- [x] 最小生产接线及PG聚焦通过验证
- [x] 真实参考/双方言、清理与Ready检查已执行（完整客户端仍有6项既有MySQL差异）
- [ ] 6项既有MySQL LIKE值/元数据差异消除后，重跑客户端完整通过再删除本文件

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。


## 2026-10-30 实施预检与阻塞交接

本轮为实施批次33；无本目录语义前置，按串行交接开始。当前完整HEAD为 `ea8e0d751328d829328bcff06a43ec5dd0f91678`（version方言隔离），上一backend身份提交为 `f57cdf013242fd21ecfdba2d58e667b98ae845ba`。不以这些提交的历史验证替代本任务验收。开始时存在无关编号文件 `.plans/2026-10-30-合并3ba36621-Go逐提交对齐Rust/4-提交-0766a90d52-严格对齐.md` 的修改，本轮未触碰。

已读取执行技能、仓库rustcodegraph技能、根/服务器AGENTS、PLANS、testing-flow及本目录只读plan.md。目标expression/server/session/parser范围 `rg --files ... -g doc.go` 未发现doc.go；`.agents/skills/tidb-verify-profile` 不存在。索引status报告11821文件、348530节点；使用explore及node定位以下接口，未将其视为当前行为通过：

- `pkg/expression/builtin_ilike.rs::IlikeSig::eval_int`（137行）具有三参NULL传播，`normalize_ilike`（210行）仅ASCII折叠，`compile_pattern`（196行）转换为MySQL二进制collator。现有Go兼容语义必须保留，不能直接全局替换为PG行为。
- `pkg/expression/builtin.rs` 已有 `CoreBuiltinKind::Ilike`、三参like/ilike工厂及CORE_BUILTIN_FACTORIES注册；尚未证明PG collation上下文和ESCAPE错误能经正式工厂消费。
- `pkg/session/runtime/query.rs` 5272–5279保留 `ExprKind::Like` 的PG布尔身份，可复用，不能按返回值猜OID。
- `pkg/session/runtime/relational_value.rs::pg_conditional_collation`（814行）只显示C/POSIX及条件表达式合并，不证明非ASCII确定性locale已存在可消费接口；恢复后须继续核对collation所属接口，不能扩大本任务补建完整locale系统。

阻塞：仅检查变量是否非空，未输出任何值或DSN。`PG18_REFERENCE_DSN`、`PG_LIBPQ_LIBRARY`、`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD` 全部missing/empty。其中PG_LIBPQ_LIBRARY可选，其余参考连接配置缺失使本任务强制真实PG18.6/原生MySQL8参考无法运行，不能取得version/server_version_num、locale或本行为真实错误证据。现有 `pkg/server/pg_client_integration_test.rs` 的客户端入口也明确要求PG18_REFERENCE_DSN，不允许skip。依赖拥有者为参考服务/运行环境配置提供者；locale接口属于上游PG collation实现，当前不宣称其缺失或可用。

恢复条件：在执行会话提供可用 `PG18_REFERENCE_DSN` 和既有MySQL8参考连接配置（通过环境注入，不在任务文件写密钥），确认真实PG版本18.6及MySQL自身参考可访问；必要时提供PG_LIBPQ_LIBRARY。继续核对PG collation/ESCAPE消费者，先定义唯一case ID与预期总数，再注册三项独立测试及客户端夹具，按本文件原命令获得运行期红灯后最小修复。若确认缺少上游locale接口，只登记精确接口及其拥有者，不递归补建。

已运行检查：`git status --short`、`git rev-parse HEAD`、`git log -3 --format='%H %s'` 均退出0；`~/.rustcodegraph/bin/rustcodegraph status`、三次explore和node（builtin_ilike.rs offset130 limit100、pg_conditional_collation、query.rs offset5240 limit90）均退出0。Python `os.environ.get` 非空性检查退出0，报告上述变量missing/empty；doc.go搜索无匹配退出1，缺失verify-profile目录的ls退出1。`git diff --check -- <本编号文件>` 退出0。

未新增测试/客户端夹具，未修改生产逻辑，未运行Cargo、WIP行为测试、真实客户端、cargo fmt或Ready make lint。有效测试与业务场景执行数均0；没有行为红灯/绿灯，没有差分自检通过证据，没有参考版本、二进制、fixture hash或清理端口证据。尚未进入用例注册阶段，预期场景数待恢复后定义，不能将未运行记为通过或测试skip。未领取Cargo槽位，实际槽位/CARGO_TARGET_DIR均不适用；未启动任何Server、参考实例或客户端子进程，也未创建数据库对象，因此无本任务锁、端口、进程或对象需清理。

决定：依do-task-plans的缺少依赖即停止规则及本任务强制参考门槛保持“已阻塞”，不以缺环境冒充红灯。只有本编号状态与交接记录变化，按授权不提交、不删除本文件；所有plan.md、父任务8及无关工作区内容保持原样。


## 2026-10-30 恢复、局部设计与用例注册

用户授权突破可逆环境阻碍，优先于技能默认即停规则。已复用本地 `/Users/Shared/work/dir/data/software/postgresql-18.6-reference/install/bin` 与 `/opt/homebrew/opt/mysql@8.0/bin` 创建独立临时参考：18.6及确定性libc `en_US.UTF-8` 已实测可用，探测实例已停止wait、删目录且两端口关闭（target/pg08-33-evidence/reference-lifecycle-*.json）。原环境阻塞解除；状态在执行中仍保留历史阻塞，不代表验收完成。

最小设计：在现有 `builtin_ilike.rs` 增加PG匹配入口及结构化诊断，使用独立libc locale句柄进行单字符小写化（C/POSIX仅ASCII），不改进程locale；现有Go IlikeSig不变。`builtin.rs` 现有ilike注册按PG上下文选字符串ESCAPE及原有三参工厂，保留分析collation。`query.rs::relational_query_expression_value` 的Like AST分支递归求值源/模式/ESCAPE后调用NewFunctionBase，消费IsLike/Not；同一分支支持PG LIKE以共享ESCAPE语义，绝不新增另一套SQL求值器。`parser_actions/expression.rs` 最小接收明确的en_US.UTF-8 collation，`relational_value.rs::pg_conditional_collation`保留它的分析身份；必要错误经已有PgFunctionError桥接到协议，不扩建目录/locale系统。每项修改均直接对应本任务ASCII/Unicode/ESCAPE/行源/参数验收。

新增测试前登记客户端唯一ID与预期：Q-{base,not,escape,default,empty,nullvalue,nullpattern,nullescape,C,unicode,utfescape,letterescape,percent,nested,row,rowzero,zero,priority}共18；bind-{ilike,not}-{0,1输入格式}-{0,1结果格式}-{0正常,1NULL,2重复}共24；bind-zero-{0,1}-{0,1}共4；error-{multi,tail}-{Q,P,txn}共6；M-{like,escape,null,health}-{Q,P}共8；I-{before,middle,after}共3，合计63个唯一业务case。A/B/参考同一case交错与字段断言不重复计数。差分负向自检值/OID/SQLSTATE共3项独立计数。所有实际数待运行记录。正式工厂测试与TCP协议测试各有效测试数1，先注册再list/run。

恢复后WIP取证：target/pg08-33-evidence/commands-red.json记录cargo fmt退出0，三个过滤器list均退出0且恰好1 test；三个run均退出101且各实际执行1 test。formal为字符串ESCAPE误入整数转换，SQL为typed ESCAPE返回0A000，客户端63实际/唯一、13通过/50失败/0跳过；其中6个MySQL LIKE值/元数据差异在生产修改前已存在，后续保留为范围外回归证据，不删断言。所有red参考实例均已停止、wait、目录删除、两端口关闭。实际动态领取槽位2，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2，各阶段自动释放锁保留缓存。

补充必要接线依据：Q-nested/Q-priority在红灯暴露OID20/8字节而参考为OID16/1字节，query.rs只对包含本任务Like AST的布尔运算/布尔COALESCE保留分析布尔身份。表扫描/通用行求值通过现有PgCastTable上下文复用同一个pg_pattern_value→NewFunctionBase入口，移除旧ESCAPE“暂不支持”闸门，MySQL行求值不改。PG18.6本地源 `src/backend/utils/adt/like.c` 与 `like_match.c` 用于确认locale和错误短路：'Ab'匹配'Ab\'为false而非报错，只有仍有输入可匹配时才触发尾部ESCAPE错误；原夹具错误预期已改为'Abc'对'Ab\'，另补正式工厂不可达尾部边界。第一次green尝试formal实际1 test通过，但session编译发现PgFunctionError.hint的Option<&str>接线类型错误（非行为红灯），已修正，继续同命令复验。


第二次WIP：正式工厂13场景/1 test通过；客户端实际63、53通过/10失败。PG剩余4项中：multi-P暴露扩展Execute遗漏PgFunctionError.hint，故必要局部修改 `pkg/server/pg_extended.rs` 的执行错误桥接，按Parse已有方式保留hint（对应真实参考H字段红灯）；另3项尾部夹具受PG SQL前置报价扫描缺口干扰。为聚焦本任务且不扩写扫描器，尾部模式采用等价显式 `ESCAPE '!'`/`'Ab!'` 触发同一22025；省略默认反斜杠仍有Q-default覆盖，正式工厂保留反斜杠尾部可达/不可达测试。参考预备语句名按multi/tail唯一化，防止前一错误Execute后已存在的statement影响后一个场景。非本任务的标准字符串扫描缺口列为范围外建议，不将42601当22025行为红灯。

新增 `pkg/parser/parser_test.rs::pg08_33_collation_predicate` 检查新locale名称、Not/IsLike/ESCAPE AST及MySQL隔离；Rust parser改动仅语义动作，不改Go grammar/generated files，采用Rust聚焦parser单测，Go parser生成/格式化目标不作为Rust语义动作验证。make lint本轮退出0。未触发Go/Bazel依赖或文件触发条件。


## 当前交付结论（替代历史预检阻塞）

本任务实现已完成，状态“已完成，待回归”。PG行为验收已取得当前证据；完整Ready未通过只因修改前已出现的6个MySQL自身LIKE回归，保持失败断言、不放宽比较，不扩修MySQL谓词和元数据子系统，因此保留本编号文件。所有来源plan.md及父任务8均未由本任务修改/清理。

本次文件：`pkg/expression/{builtin.rs,builtin_ilike.rs,lib.rs,pg08_33_test.rs}`；`pkg/parser/{parser_actions/expression.rs,parser_test.rs}`；`pkg/session/runtime/{query.rs,relational_value.rs}`；`pkg/server/{pg_extended.rs,pg_query_test.rs,pg_client_integration_test.rs,testdata/pg08_33_libpq.py}`；本编号文件。生产修改全部来自本任务真实红灯及上述最小绑定/错误hint接线；未增加依赖、Cargo manifest、Go或Bazel修改。

Ready适用检查（本轮先cargo fmt --all，后测试）：三个目标的list均恰好1 test，正式工厂1 test/13场景与SQL协议1 test/23场景通过；客户端1 test实际执行，63预期/实际/去重，57通过/6失败/0跳过。PG相关55场景全部通过，另MySQL健康Query/Prepare两场景通过；三个值/OID/SQLSTATE负向自检均通过。通过分类为success13、NULL3、zero2、binding28、error-recovery6、interleaved3、mysql2。未把字段断言、三端循环或恢复查询额外计数。

唯一失败ID保持 `M-like-Q`、`M-like-P`、`M-escape-Q`、`M-escape-P`、`M-null-Q`、`M-null-P`。reference的非NULL LIKE是BIGINT metadata `[code8,flags32897,charset63,length1,decimals0]`，NULL是flags32896；AsterSQL Query/Prepare/Execute长度/flags/decimals不一致、Execute又变为string metadata，且utf8mb4_bin大小写判断不同。这些同输入错误在生产修改前的red原始观测已存在。恢复条件是MySQL表达式/协议元数据拥有者修复后重跑本文件三个原始命令与本fixture；不得用白名单或清理用例解除缺证。

精确命令（均从仓库根运行，Cargo全程动态槽位2，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2，env继承临时参考，移除LDFLAGS）：

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_33_test::pg08_33_formal -- --exact --list
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_33_test::pg08_33_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_33_sql -- --exact --list
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_33_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_33_clients -- --exact --list
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_33_clients -- --exact --nocapture

上述Ready顺序退出码为0/0/0/0/0/0/101；逐次命令和输出见 `target/pg08-33-evidence/commands-*.json`、各阶段日志。WIP red/green1/green2与Ready各次真实结果均保留，编译失败没有冒充行为红灯。

相邻验证：下面三个parser过滤器均以 `env -u LDFLAGS cargo test --locked -p astersql-parser --lib <filter> -- --exact --list` 和同命令将 `--list` 换 `--nocapture` 执行，各list1/run1、退出0：`parser_test::pg08_33_collation_predicate`、`parser_test::pg_dialect_01_behavior`、`parser_test::pg_dialect_02_behavior`。原有MySQL/Go ILIKE回归以 `env -u LDFLAGS cargo test --locked -p astersql-expression --lib builtin_ilike_test:: -- --list` 及 `-- --nocapture`执行，list4/run4全部通过/退出0。准确命令见 `target/pg08-33-evidence/adjacent-commands.json`。`make lint`退出0（make-lint.log）；本任务路径 `git diff --check -- <delivery-paths.json所列路径>`退出0；`python3 pkg/server/testdata/pg08_33_libpq.py --self-test`及 `python3 -m py_compile pkg/server/testdata/pg08_33_libpq.py`均退出0。

真实参考为postgres18.6（binary SHA256 b1bb5425225d13f1b9265112da8662b2822fe643aee03fa415c1a37db8aabb2f）、MySQL8.0.46（SHA256 9b5f94e417e091a11432597365034e4398149cad410eed2ec130aa29387466bd）、libpq180006、tzdata2026c。server_version_num=180006，编码UTF8、locale C及明确确定性libc en_US.UTF-8，真实SHOW sql_mode/字符集/collation均保留原始观测。每次fixture保存HEAD/fixture hash/Rust binary hash、独立PG参考/A/B观测与MySQL参考/Aster观测，失败也写summary。客户端fixture未修改PG会话设置；MySQL只在自己的连接SET NAMES、连接关闭后丢弃该状态。

清理证据：所有本任务reference-lifecycle-*.json均cleanup_verified=true（包含中途编译失败）；Rust客户端同名*.cleanup.json记录数据库删除及PG/MySQL监听关闭；Python summary清理引用数据库/表、两端健康对象所属库删除、连接/结果/statement关闭。原始用户服务未停止，临时参考进程terminate/pg_ctl stop后wait，临时数据目录删除，参考端口均不可连接；所有Cargo阶段退出后仅释放本任务owner.json及slot-2.lock，保留编译缓存，其他槽位锁不动。

风险/未验证：本任务只交付C/POSIX与选定libc en_US.UTF-8的ILIKE；其他locale/provider、完整collation目录/隐式继承、标准字符串前置扫描缺口均范围外，不能把现有helper推导为完整locale实现。locale字符折叠使用独立句柄，不修改进程locale；显式栈避免匹配递归栈溢出，但每行locale创建及复杂百分号模式的性能未基准测试。仅macOS实际编译/验证，Linux未交叉编译；未运行RealTiKV、全包/全工作区测试或Go parser生成目标，未宣称完整Ready通过。最终只提交本任务文件，排除同时出现的其他计划/技能修改。


最终复验补记：`target/pg08-33-evidence/commands-final.json` 对应最终源码（含POSIX towlower_l显式ABI声明修订），fmt/list均退出0，formal13/SQL23通过，客户端仍仅6项既有MySQL失败（57/63、0跳过），不是历史结果。原始最终观测在 `target/pg08-33-evidence/81623-55212/`，Rust清理文件81623-55212.cleanup.json三项true，引用实例生命周期reference-lifecycle-*.json均true；最终槽位2锁已不存在。`target/pg08-33-evidence/delivery.json` 汇总精确命令、最终源码hash、计数及清理。自审已确认只有本任务明确路径进入提交，未放宽客户端断言、未回滚并行无关工作；交付后保持编号文件以便回归，不删除。

# 任务 38: NULL安全比较谓词

批次：【实施批次 38】；语义依赖：无本目录语义前置；互斥前序：批次37结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：int4 IS [NOT] DISTINCT FROM绑定底层等号但结果永不NULL。

来源任务：大计划8“专属函数与运算符”；路线图第4章“运算符调用契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

大计划2特殊谓词AST、大计划6 int4等号资格与bool结果；不得伪装成可查找的普通nulleq函数。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_arithmetic.rs`、`pkg/expression/builtin_op.rs`、`pkg/expression/builtin_compare.rs`、`pkg/parser/ast/lib.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_38_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_38_test;`，新增测试`pg08_38_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_38_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_38_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_38_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

两NULL/一NULL/相等/异值的NOT DISTINCT为true/false/true/false，DISTINCT逐项取反；普通=含NULL→NULL；PG <=>无内建签名42883；MySQL <=>保留1/0与自身元数据。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

按本任务专属场景检查MySQL自身运算、sql_mode、collation、警告与元数据，不给MySQL发PG cast语法来冒充语义拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_38_test::pg08_38_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_38_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_38_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_38_sql`的TCP断言补充，未知字段不能默认相等。

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

- [x] 局部接口取证与当前测试名注册（已注册工厂、TCP、客户端与独立parser测试）
- [x] 失败验证或已有正确行为的当前补证（三条精确测试均实际运行1个并退出101）
- [x] 最小生产接线及通过验证（Ready工厂13、TCP28、真实客户端69/69）
- [ ] 真实参考/双方言、清理与Ready交付（曾实测69/69；外部删除原始观测，恢复验证受无关原生构建失败影响，证据待重建）

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。

## 2026-10-30 实施前置核对与阻塞交接

- 当前批次确认为实施批次38；无本目录语义前置，仍依赖首批及上游适用局部接口。当前HEAD为`7e7d7488f81867a42bccf016caf187778860e23d`（批次37 text连接提交）；前序共享资源持有者已退出按本次调度交接，不以该提交推断38已通过。
- 已读执行技能、根/服务器AGENTS、根PLANS、testing-flow及本目录只读plan.md；目标expression/server/parser/session未找到doc.go。`.agents/skills/tidb-verify-profile`不存在，不宣称应用该技能。
- RustCodeGraph status成功：索引11832文件；explore定位`resolve_pg_signature`、`NewFunctionBase`及当前query消费者；`node pg08_37_formal`证实现有独立测试通过`NewFunctionBase`构造、类型断言及求值，可复用测试形式，不能以此证明NULL安全谓词。query `pg08_38`未找到节点；尚未创建或注册38测试。AST特殊谓词、int4等号与bool身份的完整局部链仍须恢复后逐项取证，接口拥有者为parser AST、expression比较/签名及session planning/query；本轮不断言这些接口缺失。
- 只读Python环境检查（只输出configured/unset，不输出连接值）退出0：`PG18_REFERENCE_DSN`、`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`均unset；可选`PG_LIBPQ_LIBRARY`也unset。真实PG18.6与MySQL8客户端差分环境尚不可直接消费。参考环境/启动配置由调用方或参考服务维护者提供；这不是行为红灯，也不是实现完成证据。
- 现有34号交接记录提到可独立启动的PG18.6/MySQL8参考脚本及二进制，但本轮没有启动、验证或借用这些实例。恢复时可由参考资源拥有者将可用连接注入本任务，或确认采用本任务自有临时实例的生命周期配置，再验证真实版本与客户端。不得复用历史绿色代替38实测。
- 按`do-task-plans`遇阻即停规则，本轮不新增测试/夹具或改生产。恢复后先完成局部接口取证、定义唯一case ID及总数，注册三个独立测试，再按同过滤器list/执行进行TDD、最小修复、真实客户端及Ready检查。全部plan.md与父大计划8未修改。
- 已运行静态命令：`git status --short`、`git log -5 --format='%H %s'`、`rg --files -g doc.go pkg/expression pkg/server pkg/parser pkg/session`、`rustcodegraph status`、`rustcodegraph explore 'DistinctFrom IsDistinctFrom NullEQ resolve_pg_signature NewFunctionBase pg08_37'`、`rustcodegraph query pg08_38 --limit 10`、`rustcodegraph node pg08_37_formal`及环境存在性检查；git/图/环境检查退出0，doc.go搜索退出1（无文件），verify-profile目录检查退出2（不存在）。
- WIP/Ready未执行：无cargo fmt、cargo test、make lint或Bazel命令；本轮业务有效运行0、通过0、行为失败0，未形成预期场景总数，不以零测试充当验证。PG18.6/双方言/协议/差分器负向自检与清理验收均未验证。
- Cargo槽位：未领取，CARGO_TARGET_DIR未设置；未启动Cargo、Python客户端、测试Server、参考实例，未创建监听器/数据库对象/锁，无本任务运行资源需清理。现有其他任务的工作区变更未修改。仅本编号文件状态与交接更新，依任务规则不提交，保留本文件以恢复。

## 2026-10-30 恢复与局部设计

用户明确授权突破可逆环境阻塞，覆盖执行技能遇阻停止规则。复用37号`references-runner.py`到本任务target证据目录，启动自有PG18.6/MySQL8.0.46实例，finally停止/wait/删除目录/核实端口关闭；未继承变量不再是阻塞。上次静态状态不能代表本次结论。

必要局部接线设计：现有AST Binary能保留谓词拼写及两操作数，并自动复用所有visitor/参数遍历，不新增普通nulleq Function节点。`grammar/main.astergram`追加BoolPri IS [NOT] DISTINCT FROM PredicateExpr产生专用Binary拼写，parser_actions做PG方言门控；重新生成现有parser tables。`builtin.rs`增加不可作为SQL标识符调用的谓词工厂名`is distinct from`/`is not distinct from`，绑定现有int4等号比较能力并专门处理NULL，结果bool非NULL、pb_code=0；不新增普通nulleq签名。session的query/relational_value消费同一工厂，分析时校验int4资格并固定bool身份，PG的<=>拒绝42883，MySQL不变。仅必要时补接类型/参数元数据。既有AST没有本谓词、属于上游最小前置，获排障授权，不扩建完整子系统。

测试case预注册：工厂 truth-{distinct,not-distinct}-{both-null,left-null,right-null,equal,different}共10及普通eq-null、PG-nulleq-rejected共2；TCP采用独立SELECT、四格式绑定/NULL及零行Describe、错误事务恢复。客户端唯一case ID由`pg08_38_libpq.py::CASE_IDS`登记，计划69：Q的15个（四组truth各2、eq-null、nested、boundary、row、rowzero、zero、typed-null），绑定36（ordinary/negated各16、zero4），错误Q/P/txn3，MySQL6种Q/P共12，交错3；差分器值/OID/SQLSTATE三项负向自检单列。实际运行数仍0，后续以真实summary为准。

生产辅助符号设计补充（应用前）：`builtin.rs::CoreBuiltin::compare_int`从既有int比较分支抽取返回比较结果和左右NULL标志，普通等号与新谓词共用，避免新增第二比较器或重复求值。`relational_value.rs::pg_distinct_value`仅把已有定型int4载荷交给正式工厂，query普通投影与共享行源均调用它。`query.rs`现有PG分析visitor负责拒绝<=>并校验谓词的int4等号资格；`relational_query_expression_type_from_sources`固定bool元数据并覆盖本任务普通int4等号NULL对照。AST采用保留拼写的Binary专用谓词，不把IS谓词降为Function；既有visitor会遍历两参数，无需补建AST子系统。独立parser测试`pg08_38_test::pg08_38_predicate_ast`注册于parser/lib.rs，验证两种谓词和MySQL语法隔离；作为必要语法局部接线的额外聚焦检查。

首轮已实测工厂：精确list=1（退出0），执行=1测试失败（退出101），原因为新谓词工厂缺失导致既有查找回退返回`No database selected`，日志`target/pg08-38-evidence/red-{0,1}.log`。SQL和客户端仍等待当前构建，不提前声称其结果。

红灯阶段完整证据：槽位2，绝对`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。三条任务命令各先`-- --exact --list`退出0、恰好1测试，再`-- --exact --nocapture`均退出101、有效1测试失败。工厂缺失、SQL错误42P01（DISTINCT FROM被旧语法错解为FROM关系）、客户端Parse/Describe断言不符，均为运行期断言失败，非环境/编译红灯。日志`red-0.log`至`red-5.log`及确切命令`red-commands.jsonl`。真实参考成功启动并验证版本后运行客户端；参考生命周期`reference-lifecycle-18494.json`显示停止/wait/端口关闭/临时数据目录删除全部true，槽位已释放、缓存保留。状态“未开始”在恢复期间表示尚未验收完成（状态集合没有进行中），旧阻塞已解除。

验证决策：调试构建parsergen连续计算约6分钟仍未输出，主动只终止本任务生成器PID41430（SIGTERM，退出-15）；外层退出241，未把这次中断当测试失败/成功。脚本已验证参考实例停止/wait/端口关闭/临时目录删除，释放自有槽位2。改用相同源代码的`cargo run --release --locked -p astersql-parsergen -- generate`重新生成，不修改生成算法或降低语法验收；重新原子领取槽位2。`make lint`已当前运行退出0，日志`target/pg08-38-evidence/make-lint.log`。仅Rust语法/测试改动，无Go/import/依赖/Bazel触发项，不运行bazel_prepare或bazel_lint_changed。

共享工作区说明：生成期间其他任务将其已暂存变更提交为`192afd6c32294df6b196345fbfd6951c630393ee`（NextGen add-index/TiKV disk-full），本任务未操作其暂存区。后续当前验证以最新HEAD和各fixture记录的HEAD为准；本任务不引用旧HEAD的历史绿色。提交继续使用明确路径以排除其他任务残留变更。

WIP发现与必要接线更新：工厂已构造到生成签名校验，但自造`builtinPgDistinctInt4Sig`不在既有thread-safety元数据中（有效1测试退出101），改复用底层等号的`builtinEQIntSig`，保留专有NULL语义与pb_code=0，不扩建生成签名系统。TCP仍42P01，经源码确认`pkg/server/pg_name.rs::Resolver::scope`将谓词的FROM当关系列表，属于普通SQL进入正式解析器前的名称绑定遗漏；不是目录事实接口缺失。新增独立`pg_name_test::pg08_38_relation_boundaries`，覆盖两种谓词、注释、真实FROM后的关系改写。设计最小修复`Resolver::relation_from`识别FROM前紧邻DISTINCT的谓词归属，应用到scope和既有递归CTE关系扫描，不在目录查询解析器伪造结果，不改变求值器。先list与执行此测试得到明确边界红灯，再应用修复。额外文件`pkg/server/pg_name.rs`及其现有独立测试文件在此登记；生产依据是当前TCP运行期42P01与上述token遍历代码。

首轮WIP实际结果：三个精确list均为1且退出0，三个执行均为1测试失败且退出101。客户端`70842-55700/summary.json`预期69、实际/去重17、通过1、失败16、未运行52，失败集中在名称绑定，未把未运行场景视为通过。工厂缺生成签名metadata与名称绑定FROM误识别均已定位，继续本范围最小修复。参考实例及客户端Server均已清理。新阶段自动领取槽位1（之前占用者已释放），`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`，不固定选择历史缓存槽位。

名称边界TDD证据：`cargo test --locked -p astersql-server --lib pg_name_test::pg08_38_relation_boundaries -- --exact --list`退出0且1测试；同命令`--nocapture`退出101且1测试失败。失败显示实际`SELECT NULL::int4 IS DISTINCT FROM `db`.NULL::int4 AS v`，期望原SQL，明确为生产名称绑定误改写。随后才新增`Resolver::relation_from`并复用到两处关系识别，运行cargo fmt --all退出0，继续WIP2。该文件已有AsterSQL版权，保留全部注释；不新增目录查询求值特例。

WIP2：工厂1测试通过（12语义case）、TCP1测试通过（28场景），客户端1测试失败。实际预期/实际/去重69，通过53、失败16、跳过0：PG <=>错误P为9而参考16（3场景），MySQL返回值正确但元数据被旧无FROM路径降为varchar（10个Q/P以及3个交错场景）。证据`92722-59178/{summary,reference,A,mysql-reference,mysql-aster}.jsonl`（summary为json）。未屏蔽任何字段。

继续最小接线设计：parser运行时`yySetOffset`会覆写ExprNode.OriginTextPosition，改把<=>操作符原始byte offset存入已有node_text.OriginTextPosition，并让PG诊断消费该0基偏移，协议层既有代码转换字符位置，无需改pg_extended。独立parser测试追加位置断言。MySQL在`query.rs::is_mysql_function_projection`仅补接<=>与普通=含显式NULL对照，复用已有`mysql_function_type_and_cache_reason`/`PlannerBuildSimpleExpr`和`mysql_conditional_projection_value`，获得原生比较的正式类型/求值，排除旧无FROMvarchar回退；不手写协议列值，不创建MySQL子系统。已有69客户端红灯授权这两处修复。名称绑定辅助进一步限定完整IS [NOT] DISTINCT FROM token序列，避免合法AS distinct列别名误判，扩展既有独立边界测试覆盖该邻接场景。

WIP3：fmt退出0、工厂1测试通过；server列表/执行因本次新增MySQL分类中的同名literal误引用导致E0599，server有效行为测试0，不能算红灯或当前回归成功。已改直接匹配ExprKind::Value(Null)，不再调用返回SessionResult<String>的source::literal。parser独立MySQL拒绝断言同时改为原生`SELECT 1 IS [NOT] DISTINCT FROM 1`，避免PG cast/参数语法造成伪拒绝。

共享槽位异常：WIP3所有Cargo与参考子进程结束、参考finally清理后，runner发现slot-1.lock/owner.json消失；随后只读发现整个target/rust-slot-locks目录已被外部移除，缓存及证据目录仍在。没有删除任何外部锁，也没有在发现后继续使用失去的租约。下一阶段重新创建锁根并原子领取；runner清理时核对记录PID，若锁已经消失只报告异常，不操作可能重建的外部锁。先前外层最终退出1包含此清理异常，命令实际server结果仍以wip3-commands.jsonl为准。完成验收须在新有效租约下重新取得当前server行为证据。

WIP4：fmt、工厂12语义case、TCP28场景均通过；客户端预期/实际/去重69，通过56、失败13、跳过0。PG所有54业务场景通过，剩余10个MySQL比较Q/P及3交错只缺BINARY_FLAG(128)：参考32897/32896，本端32769/32768；值、code、charset、length、decimals、警告完全一致。最新证据`38091-62102`；本轮槽位锁完整释放，参考进程/端口/目录清理通过。

最小修复设计（应用前）：`builtin.rs`既有Compare分支仅为MySQL eq/nulleq返回类型补BinaryFlag，nulleq补必然非NULL的NotNullFlag，沿用比较算子本身的类型契约，不在wire层造元数据。独立expression测试追加唯一case `mysql-formal-metadata`，正式工厂总数由12变13，先注册/执行断言失败后修复。同处源代码`query.rs::is_mysql_function_projection`的Binary局部资格保持既有排除聚合约束，避免让SUM等表达式进入简单标量binder，属于新增接线必需的局部边界保护。

MySQL标志TDD：flags-red --list退出0且1测试；--nocapture退出101且1测试失败，断言缺BinaryFlag。补齐现有eq/nulleq类型标志后，Ready工厂精确list与执行退出0、有效1测试、13语义case通过（含mysql-formal-metadata）。PG无下推授权，CoreBuiltinKind::PgDistinct pb_code仍0；nulleq的既有MySQLpb签名不变。Ready其余命令仍在执行，不预记成功。

Ready主验证已当前通过：三条任务命令各精确list=1、执行=1且退出0；正式工厂13语义case，TCP28场景，客户端预期/实际/去重/通过69、失败0、跳过0，差分器值/OID/SQLSTATE负向自检3单独通过。客户端category计数以最终summary.json为准，原始观测和构建身份保留于target/pg08-38-evidence的本次PID/端口目录。包括PG54、MySQL12、A/B/M交错3；错误恢复3是PG54的子集，不重复计入总数。全部参考/客户端连接、own数据库表、两个Server端口、参考进程/目录已清理；本阶段槽位1锁正常释放。cargo fmt --all退出0；make lint退出0；当前git diff --check退出0。正在执行Ready相邻六个精确测试及release parsergen check，尚不提前标最终完成。

外部清理复发（Ready相邻检查阶段）：六个相邻精确测试均当前通过且有效1（工具输出保存于会话），parsergen check运行时，整个target/pg08-38-evidence及release二进制和部分槽位缓存被外部删除。Cargo/代码没有主动清缓存；此次真实客户端69/69工具输出已取得，但原始观测文件不再存在，不能作为现存交付附件。已向用户异步请求停止删除在用目录，不等待授权也不停止安全恢复工作。

恢复设计：脱敏证据放到Git私有目录`.git/agent-evidence/pg08-38`，Cargo仍按原规则只用target/rust-slot-1..10，不绕过共享槽位。仅客户端测试增加可选`PG08_38_EVIDENCE_DIR`覆盖输出根目录，默认路径保持原规则；重新运行三条聚焦测试及必要检查生成当前原始观测，再交付。先前红灯/绿色历史在会话工具输出和本编号有据，原target日志被外部删除明确标注，不重新制造红灯或冒充文件仍在。代码功能已验证完成，验收证据恢复后再删除编号文件。

证据恢复环境排障：新租约下首次重建因E0463 idna_adapter失败，有效行为测试0；定向cargo clean -p idna_adapter只移除15个损坏缓存文件（406.9KiB），未改依赖，第二次编译继续发现libz-sys引用不存在的libpkg_config rlib，有效行为测试仍0。两轮参考服务和锁均正常清理。说明部分fingerprint残留指向被外部删除的产物，而不是本任务代码错误。采用安全替代：从仓库.cargo/config.toml完整继承原rustflags，仅追加Rustc help确认默认值的`-Cstrip=none`，经CARGO_ENCODED_RUSTFLAGS传递；仍用相同原子分配槽位及绝对CARGO_TARGET_DIR，不创建第二构建目录、不删除剩余缓存、不改features/优化/依赖。生成全新fingerprint避开损坏条目。确切flags保存在.git/agent-evidence/pg08-38/validation-flags.json。

## 最终交接（2026-10-30：已完成，待回归）

实现完成：int4 IS [NOT] DISTINCT FROM通过专用谓词AST拼写绑定共享整数等号比较，结果非NULL bool；正式工厂/普通与行源查询消费同一求值，PG <=>返回42883及精确诊断位置，MySQL eq/nulleq保持原生类型标志。parser/name binding与独立测试已完成，无新依赖、无Cargo/Bazel/Go文件修改。客户端证据目录可选PG08_38_EVIDENCE_DIR，默认仍原target路径。

实际已运行的行为证据（发生于外部清理之前，不能冒称当前有原始附件）：Ready工厂1测试13语义case、TCP1测试28场景、真实PG18.6/MySQL8.0.46客户端1测试69/69、3负向自检，均退出0，六个相邻精确测试各有效1且退出0。真实构建HEAD192afd6c32294df6b196345fbfd6951c630393ee、libpq180006、tzdata2026c、fixture SHA256 0d653a9039ffa165c9ee9d382c87febda565e913af124121a44000bbcff06b92。这些结果已在会话工具输出实测，原始target/pg08-38-evidence已被外部删除；历史事实单独存`.git/agent-evidence/pg08-38/historical-evidence.json`，不伪造已删除的raw logs。

当前持久证据：`.git/agent-evidence/pg08-38`，make lint退出0、git diff --check退出0、Python py_compile退出0、三轮cargo fmt --all退出0。三轮恢复命令首个expression测试的list编译均退出101、有效行为测试0，绝不当作红灯或通过：recovery为E0463 idna_adapter，recovery2为libz-sys引用不存在的pkg_config rlib，recovery3（等效默认strip=none新fingerprint）为静态z库不可定位、protobuf config.status失败、grpc构建失败、aws-lc对象临时重命名No such file or directory。磁盘仍有249GiB可用，未归因于磁盘不足。各轮已清理自有PG/MySQL参考实例、wait进程、删除临时数据目录、验证端口关闭；槽位1租约正常释放。较早阶段使用槽位2，最终阶段使用槽位1，绝对CARGO_TARGET_DIR分别为仓库target/rust-slot-2、仓库target/rust-slot-1；完整路径与命令见持久JSON清单。

范围外接口拥有者：共享target清理程序/构建环境维护者，及本机zlib/protobuf/grpc/aws-lc原生工具链环境；不是本谓词业务依赖缺口。未继续移植外部依赖、修改依赖tag、补建子系统、清空全部缓存或改Go/Bazel。用户异步协调问题仍未收到回答，已穷尽保留缓存的安全局部恢复。

恢复条件：停止删除在用槽位/证据；恢复完整可编译的原生依赖环境及一致缓存后，按本文件共享槽位规则领取，再运行持久runner/refs/checks恢复三条任务测试，真实客户端原始观测69/69、六项相邻与parsergen一致性检查，确认清理。默认flags优先；证据输出至PG08_38_EVIDENCE_DIR持久目录。所有来源plan.md及父大计划8保持只读。只有完成剩余原始证据及Ready验收后才删除本编号文件；当前必须保留。

未验证/缺证：外部清理后当前客户端测试新增输出目录配置未取得完整构建/运行；当前PG18.6双方言原始观测需重新生成；parsergen check旧结果丢失，现已在持久目录独立补跑退出0；未运行Go parser make目标、全包/RealTiKV及性能基准（本任务不需要广扫，Rust语法由parsergen和聚焦测试负责）。正确性/兼容性有此前选定场景通过证据，最终交付仍待恢复上述当前证据；性能没有测量。

最终补证：`cargo run --release --locked -p astersql-parsergen -- check`在新有效槽位1租约下独立运行退出0，日志`.git/agent-evidence/pg08-38/parsergen-check.log`及parsergen-result.json现存，锁正常释放。此前六项相邻通过事实保留于会话工具输出；当前原始客户端观测仍未补回，因此状态保持已完成，待回归，不删除本编号文件。已自审17个本任务文件的差分与暂存内容，排除其他工作区修改，按git-commit技能提交。

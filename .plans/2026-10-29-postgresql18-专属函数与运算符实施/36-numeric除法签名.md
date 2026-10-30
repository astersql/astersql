# 任务 36: numeric除法签名

批次：【实施批次 36】；语义依赖：34；互斥前序：批次35结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：PG numeric/numeric的/保留精确数值及错误。

来源任务：大计划8“专属函数与运算符”；路线图第4章“运算符调用契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

PgNumeric及大计划6 numeric结果身份；任务34运算符分流；不从float64转回decimal。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_arithmetic.rs`、`pkg/expression/builtin_op.rs`、`pkg/expression/builtin_compare.rs`、`pkg/parser/ast/lib.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_36_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_36_test;`，新增测试`pg08_36_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_36_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_36_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_36_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

5/2→精确2.5，不强制文本尾零；负值、非整除精度与大数；1/0→22012；NULL/0→NULL；限定运算符；MySQL /保留div_precision_increment=4与警告。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

按本任务专属场景检查MySQL自身运算、sql_mode、collation、警告与元数据，不给MySQL发PG cast语法来冒充语义拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_36_test::pg08_36_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_36_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_36_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_36_sql`的TCP断言补充，未知字段不能默认相等。

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
- [ ] 真实参考/双方言、清理与Ready交付（PG与清理已完成，MySQL既有差异待回归）

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。


## 2026-10-30 前置取证与阻塞交接

本轮使用do-task-plans及仓库rustcodegraph技能，确认当前为实施批次36，语义依赖34，互斥前序35。当前完整HEAD `12aa806b7e8c2f3a24bd6c91a1c8d50d94f9b8d1`；34实现提交为`814d22595be574d4c819d093f123a2595ded1c80`。35会话`01a18a8c-d09a-7c62-b9ad-82329fd751c5`已idle，最近执行turn已结束，最终报告已提交并清理参考实例、监听器和锁；本轮只读检查共享槽位锁目录为空。34/35均保留“已完成，待回归”，其MySQL既有差异不等于36号失败，也不能继承其PG通过数。

已阅读执行技能、本目录plan.md、本编号、根AGENTS.md、PLANS.md、pkg/server/AGENTS.md、docs/agents/testing-flow.md与skills/rustcodegraph/SKILL.md；expression/server/session/parser根目录没有doc.go。所有plan.md只读，其他任务已有生产、Cargo.lock及文档修改不动。

当前接口证据：rustcodegraph status退出0，索引11828文件；explore后用具体文件node核对。`pkg/types/pg_numeric.rs:47–55`的PgNumeric保留规范系数、符号和scale，构造/格式化明确不经浮点或MySQL decimal；`pkg/expression/builtin.rs:7355–7367`已有PG int4除法工厂，`:7369–7416`的其余div仍选择共享ETDecimal/ETReal并WrapWithCastAsDecimal。`pkg/session/runtime/query.rs:1389–1417`已有PgNumeric round消费者。以上仅定位可复用入口及待测区域，未证明numeric除法当前正确或失败，未授权无行为红灯的生产修复。正式工厂/签名拥有者为expression，规范数值载体拥有者为types/pg_numeric，会话类型和行源消费者拥有者为session runtime；恢复后需要继续核对当前接口而非补建完整子系统。计划的pg08_36测试/客户端注册尚不存在。

阻塞：只读检查显示`PG18_REFERENCE_DSN`、`PG_LIBPQ_LIBRARY`、`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`均absent，未输出值或DSN。PG_LIBPQ_LIBRARY本身可选，MySQL密码为空也可能合法；必需连接配置未注入才是此处阻塞。没有连接参考服务，未取得当前version/server_version_num/原生客户端版本。遵循do-task-plans“遇到阻碍（缺少依赖、测试失败、指令不明确）”立即停止，未将环境缺失或编译错误记为业务红灯。

恢复入口：前序`target/pg08-35-evidence/references-runner.py`存在；PG18.6二进制`/Users/Shared/work/dir/data/software/postgresql-18.6-reference/install/bin/postgres`和MySQL8二进制`/opt/homebrew/opt/mysql@8.0/bin/mysqld`存在，均仅为静态可恢复资源。参考连接配置与自有实例生命周期由执行环境/参考夹具维护者负责。恢复条件为注入可连接PG18.6及MySQL8配置，或在恢复阶段审阅并复用runner启动独立临时参考实例，确保正常/失败/中断均stop/wait、删除自有目录、验证端口关闭。随后先验证真实版本，登记36号唯一case ID及总数，新增并注册三个独立测试和客户端夹具，领取动态共享槽位后按原命令list恰好1、获取真实行为红灯或当前绿色、最小修复及Ready。不得因34/35的MySQL差异而扩大numeric任务范围或放宽断言。

本轮检查：`git status --short`、`git rev-parse HEAD`、`git log -3 --format='%H %s'`、`~/.rustcodegraph/bin/rustcodegraph status`、`~/.rustcodegraph/bin/rustcodegraph explore 'PgNumeric division checked_div CoreBuiltinKind DivideDecimal resolve_pg_signature'`均退出0；三次`rustcodegraph node --file`分别读取pg_numeric.rs offset45 limit24、builtin.rs offset7344 limit75、query.rs offset1380 limit62均退出0。`rg -n 'PgNumeric|DivideDecimal|PgInt4Divide|pg08_36'`针对候选生产及注册文件退出0；Python环境存在性核对退出0；参考脚本与两二进制`ls -l`退出0。`test -f .agents/skills/tidb-verify-profile/SKILL.md`退出1（技能仍缺失）。另通过Codex list_threads/read_thread核对35号最终完成turn。没有调用任何Cargo、构建或测试脚本。

验证：WIP与Ready行为验证均未开始；三个新增测试未创建/运行，有效测试0，实际业务场景0，通过0，行为失败0，不能以零测试宣称成功。PG18.6参考、双方言、四格式绑定、TCP元数据、差分器负向自检、恢复与清理验收尚未验证。未运行cargo fmt/test、make lint、Bazel准备或RealTiKV；本轮只有本编号状态/交接变更，按用户规则不提交、不删除。实际Cargo槽位不适用，CARGO_TARGET_DIR未设置；未创建本任务锁、参考进程、监听器、连接或数据库对象，无本任务运行资源需清理。未触发Go/依赖/Bazel修改。

正确性和兼容性缺少当前行为证据，性能未测；无生产修改引入运行时风险。下一步恢复参考环境后继续里程碑一。此次更新只记录真实前置证据及阻塞/恢复条件，所有来源plan.md、父大计划8、原路线图和覆盖表保持原状。


## 2026-10-30 恢复执行设计

用户明确授权突破可逆环境阻塞，覆盖技能默认遇阻停止；本轮将审阅并复用35号references-runner.py，自启独立PG18.6/MySQL8.0.46参考，传入原生客户端配置，finally停止wait、删除自有数据并检查端口。环境变量未继承不再作为外部阻塞。当前HEAD仍12aa806b7e8c2f3a24bd6c91a1c8d50d94f9b8d1，35号已退出且锁目录为空。

先登记客户端唯一case ID于新增夹具CASE_IDS，预期70项：22个Q场景（正常、负数、超过MySQL容量的大数、1/3、极小数、scale、special、NULL、嵌套、行源和零行），28绑定（普通/限定各4格式×值/NULL/重复负值，另零行4），3除零错误恢复（Q/P/事务），14个MySQL原生Q/Prepare，3个A/B/M交错。3个值/OID/SQLSTATE负向自检独立计数。正式工厂和TCP新增测试先挂载再运行；当前有效运行仍0。

拟最小实现入口（须运行期红灯确认后）：types/pg_numeric.rs::PgNumeric增加精确除法方法，复用其规范系数/scale及PG18源码numeric.c::select_div_scale（16显著位、以base10000首组估算、保留输入scale并clamp至1000）和ties-away舍入，不通过float64/MyDecimal。builtin.rs增加PG numeric/numeric专属工厂与EvalString以保存完整数值身份；同已有PG numeric工厂保留有限容量EvalDecimal兼容接口，实际会话须消费EvalString。query.rs与relational_value.rs新增同文件pg_numeric_divide_value适配器，经NewFunctionBase构造；普通、嵌套、行源复用此工厂，不另建求值器。若类型推导丢typmod则在原除法分支最小修正。没有新增生产模块或外部依赖。所有实际修改仍由本任务运行期断言缺口授权。


红灯证据：槽位1，fmt退出0，三个精确过滤器list各1；formal初轮检查发现共享下推签名212，调整断言顺序后同过滤器运行时在EvalString报“core builtin does not return STRING”（101），证明精确PG消费接口缺失。TCP退出101；客户端完整预期70/实际70/去重70/通过19/失败51/跳过0。真实差异包括9007199254740993/1被算为9007199254740992、1/3仅16位、除零成功NULL而非22012；不是仅尾零显示差异。原始观测和summary位于target/pg08-36-evidence红灯目录。参考server_version_num=180006已实测；两临时参考及监听器和对象清理true，锁已释放。正式工厂、SQL及客户端红灯支持前述最小设计；未改MySQL路径或依赖。

当前WIP：精确工厂14个语义场景list1/run1通过（退出0）；server重编译中。make lint退出0。types/pg_numeric.rs新增divide与DivisionByZero对应精确大数/非整除/特殊值/22012测试；builtin.rs的PgNumericDivide按PG方言与两个TypeNewDecimal绑定、EvalString保持精确载体、无下推，EvalDecimal仅复用已有有限容量兼容接口，正式会话消费EvalString；query.rs和relational_value.rs适配器及行源接线对应普通/嵌套/行源/NULL客户端红灯。附带pg_numeric.rs.md描述该新增接口。未修改Go、Cargo manifest、锁文件或Bazel；工作区已有他人Cargo.lock修改不纳入本任务。


WIP第二轮：formal list1/run1与TCP list1/run1已通过（22个TCP场景）。客户端被新增identity核对提前终止：AsterSQL查询@@div_precision_increment返回既有1105，同时夹具误将该观测加进三项设置恢复元组，finally解包报错；此轮不算客户端矩阵证据。修正夹具恢复元组，保留两边配置原始观测并要求参考值4，AsterSQL不支持变量查询明确作为未验证接口，不默认其等于参考；实际MySQL /的精度与警告仍由原严格M-slash/M-zero Q/P断言验证，不屏蔽差异、不扩建变量子系统。临时参考runner已stop/wait并删目录，server守卫关监听器删自有库，未形成外部残留。

补强验收在执行前注册：新增Q-round-up检查2/3最后一位向上舍入；普通与限定运算符每种四格式新增第四次绑定大数9007199254740993/2，预期4503599627370496.5000，共8个唯一case。最新CASE_IDS预期79（23 Q +36绑定+3错误恢复+14 MySQL+3交错），此前70项红灯证据保留原计数，不能追写为79。正式工厂新增2/3场景后共15个语义场景。无新增生产改动。


同目录类型回归注册（运行前）：在既有挂载的pkg/types/pg_numeric_test.rs追加pg08_36_numeric_domain，不改生产模块挂载；过滤器pg_numeric_test::pg08_36_numeric_domain预期有效测试1。针对独立数值载体新增7个有意义的域边界断言：整数131072位溢出、极小数下溢显示scale1000、正负边界tie远离零舍入、带尾零除数、Infinity/0错误与负零规范化。它不替代formal/SQL/client红灯证据。先在动态槽位cargo fmt --all，再以env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_numeric_test::pg08_36_numeric_domain -- --exact --list及--nocapture验证。


## 最终交付记录（2026-10-30）

当前实现与聚焦验证已完成；状态为“已完成，待回归”，保留本编号。生产变更仅为前述五处：PgNumeric精确除法与22012；PG numeric/numeric专属工厂；普通/嵌套会话适配；行源适配。类型分析与pg_result/pg_extended已保留numeric身份、元数据和编解码，经当前协议验证无需改动；限定运算符直接复用34号解析接线。没有新增依赖、第二求值器、下推许可、Go/Bazel或通用变量子系统。独立expression测试与同目录types测试、已挂载server测试及原生客户端夹具覆盖实际消费者。

Ready证据基于HEAD `12aa806b7e8c2f3a24bd6c91a1c8d50d94f9b8d1`及本任务工作区改动：正式工厂15个语义场景通过；TCP22场景（普通/限定、typed NULL、零行、statement/portal Describe、四格式Bind、Execute不重发RowDescription、22012、E→ROLLBACK→I/Sync恢复）通过，两个Rust过滤器均list1/run1、退出0。类型同目录pg_numeric_test::pg08_36_numeric_domain list1/run1通过，7个域边界场景，退出0。当前PG18.6客户端独立业务场景62/62通过（成功19、NULL2、零行2、绑定36、错误且恢复3），错误和恢复为同一case的两项检查，不相加冒充6个case。三项A/B/M交错的PG配对、健康表读取均通过，但组合case在原生MySQL元数据比较失败，不计整体通过。完整矩阵预期79/实际79/去重79/通过64/失败15/跳过0；MySQL健康Q/P另2通过，原生运算12失败、交错3失败。值/OID/SQLSTATE三项差分负向自检全部通过，独立计数，不作为业务场景。

未屏蔽失败：M-slash/negative/div/negdiv/null/zero各Q/P共12项仍有既有DECIMAL显示精度（2.5对2.5000）、列长度/decimals、原生Execute字符串列及1365除零警告缺失；I-before/middle/after严格比较自身DIV元数据也失败。修复前红灯的14项MySQL完整原始观测与Ready14项逐字完全相同，且Ready六份交错原生观测（reference/aster各三份）与修复前同侧M-div-Q完全一致，证明不是本numeric PG改动引入。对比证据`target/pg08-36-evidence/mysql-baseline-comparison.json`，原始观测分别在`2250-53100`与`15961-55574`目录。MySQL参考当前@@div_precision_increment读回4，AsterSQL此变量读回接口仍1105，原始配置观测已存，不将未知字段默认视作相等或宣称AsterSQL读回已验证；实际/精度与警告仍在严格M-slash/M-zero断言中失败。此缺口属于共享MySQL会话求值/结果元数据/原生预处理消费者（relational_value.rs未改MySQL分支、conn_stmt.rs及控制变量接口），不为本PG签名扩大修复范围。恢复条件为上述共享接口修复后原样复跑client达到79/79及0失败，方可删除本编号，不宣称完整双方言通过。

Ready适用命令（profile技能仍缺失，按任务明确清单执行）：

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_36_test::pg08_36_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_36_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_36_clients -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_numeric_test::pg08_36_numeric_domain -- --exact --nocapture
    make lint
    git diff --check -- <本任务12个路径>

每个Rust测试都先同过滤器-- --exact --list确认恰好1。Ready前三个run退出依次0、0、101；domain退出0，fmt、make lint与diff check退出0。相邻五个回归各list1/run1、退出0：expression的pg08_34_test::pg08_34_formal、pg08_35_test::pg08_35_formal、builtin_arithmetic_test::go_parity::divide_preserves_division_by_zero_policy_overflow_and_decimal_scale；types的pg_numeric_test::pg_function_04_round_exact、pg_numeric_test::pg05_array_numeric_binary_codec。完整argv/cwd/exit/有效计数/日志汇总于target/pg08-36-evidence/delivery.json与各commands-*.json；不是从历史绿色继承。没有运行全包、Go failpoint测试、RealTiKV或禁止的bazel_lint_changed；本任务未触发Bazel准备条件。没有改Cargo依赖/锁文件；他人已有Cargo.lock修改保持原状。

本任务各阶段均原子动态领取仓库共享槽位，WIP/主Ready/相邻验证实际槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；最后同目录类型边界验证因槽位1已被其他任务占用而领取槽位2，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。两个槽位均已释放自己的锁并保留缓存，未动其他持有者或创建独立Cargo缓存。

真实参考：server_version_num=180006，PG binary与libpq均18.6/180006，MySQL binary/server=8.0.46，tzdata2026c。原生MySQL客户端版本、locale/编码/TimeZone/search_path、sql_mode/collation、二进制及fixture hash见Ready目录的build/reference/mysql-reference/mysql-aster.jsonl。当前fixture SHA256 `7f97b7b892431ff411b3e53da72b68c0f22bc9ee9cdad5a4999bc640d854852f`；它与Ready归档hash一致。没有借用或停止用户服务。

清理：三轮自有PG/MySQL实例均停止并wait，临时目录删除，双参考端口关闭；三轮Rust server守卫均关双监听器、删除本任务数据库。Ready Python连接/libpq结果、MySQL statement/result/连接均释放，MySQL设置恢复和reference数据库删除全部true，健康数据未改变；PG设置未被修改，错误事务回滚后继续使用。中间WIP夹具解包错误轮不作为完整Python清理验收，通过停止其自有参考及server guard清理全部外部资源；最终Ready清理重新完整验证。`cleanup-audit.json`包含过程及最终检查，本任务锁剩余0。没有新建/遗留工作树、外部依赖临时副本或参考进程，仅保留脱敏证据和构建缓存。

风险/限制：本签名PG精度、NULL、特殊值、错误、类型和协议已验证；完整MySQL兼容性因上述既有差异待回归，AsterSQL div_precision_increment变量读回尚不支持。采用精确系数长除法，没有做性能基准；其他numeric运算符和完整ETDecimal载体扩展不在本任务范围，既有EvalDecimal容量兼容接口不作为精确PG输出消费者。本次提交仅12个本任务文件（11个代码/测试/说明文件及本编号记录），不包含其他任务DDL/导入/依赖/技能/其他计划变更，所有plan.md及大计划8保持原状。下一步修复共享MySQL结果契约后原样复验客户端，再清理36号编号。

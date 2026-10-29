# 任务 29: 当前schema有效路径

批次：【实施批次 29】；语义依赖：28；互斥前序：批次28结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：current_schema() 普通表达式只选存在且有USAGE的显式路径首项。

来源任务：大计划8“专属函数与运算符”；路线图第4章“必需系统函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

任务28 name桥接；现有effective_explicit_path只见存在性筛选，授权事实必须由目录/身份所有者提供；无权限接口不得固定public或实现完整角色系统。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_info.rs`、`pkg/server/pg_session.rs`、`pkg/server/pg_catalog.rs`、`pkg/server/runtime.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_29_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_29_test;`，新增测试`pg08_29_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_29_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_29_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_29_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

public→pg_catalog→空→NULL；missing,s_hidden,s_ok→s_ok；只剩missing/hidden→NULL；GRANT/REVOKE后下一语句刷新；Parse后SET再Execute；另一PG/MySQL状态不变。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

MySQL DATABASE()/VERSION()/CONNECTION_ID()保持其真实连接事实；current_database()/current_schema()/pg_typeof(1)/pg_backend_pid()对应PG专属签名检查合法语法下的拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_29_test::pg08_29_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_29_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_29_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_29_sql`的TCP断言补充，未知字段不能默认相等。

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
- [ ] 真实参考/双方言、清理与Ready交付（已实际执行并清理；MySQL六项既有回归待修复后原样复验）

## Surprises & Discoveries

2026-10-30 实施前置取证确认缺少本任务必需的 schema USAGE 事实及自定义 PG schema 目录接口；详细证据与恢复条件见下方。本轮未新增测试、未运行 Cargo 或行为验证。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

2026-10-30：新增排障授权后已补齐必要的局部schema/USAGE元数据接口，正式工厂6场景、SQL29场景及客户端57个PG/错误恢复/交错场景当前通过。完整客户端73场景仍有6项未修改MySQL路径回归，标记已完成，待回归并保留本文件；不宣称完整双方言通过。原阻塞记录为历史，所有来源plan.md及父大计划8保持不变。


## 2026-10-30 前置接口取证与阻塞交接

当前实施批次29，语义依赖28；实际完整HEAD为 `843c783d928239d85ce2a8460521ae8117257bd4`。批次28提交已保留正式 `current_database` 工厂、name身份及防折叠接线，可复用而不重做。通过应用 `list_threads` 核实前序会话 `01a189d4-a89b-77f3-afd7-158d386a5d1d` 为 idle；`read_thread` 的最终交接确认其已提交并清理参考实例、连接、监听器和槽位。批次28状态为已完成，待回归（六项未修改MySQL路径失败），不把此历史测试当成本轮证据。检查 `target/rust-slot-locks` 为空，本轮不占用共享资源。

依赖接口取证（RustCodeGraph已索引11812文件；status成功，实际源码文本仅用于图未覆盖的SQL字符串、注册及配置核对）：

1. `pkg/server/pg_session.rs:107 effective_explicit_path(&self, context: &dyn TiDBContext) -> ConnResult<Vec<String>>` 只调用 `namespace_names` 与 `user_identity`，展开 `$user`、存在性过滤和去重，无USAGE检查。真实消费者 `effective_schema`（127行）取首项，`PgSession::execute` 的 `SessionQuery::Schema`（206行）消费；扩展协议 `pg_extended.rs:1891` 也使用该事实。不能只修目录探测特例。
2. `pkg/server/pg_catalog.rs:3752 namespace_rows` 仅生成 `pg_catalog`，并在当前原生数据库存在时生成 `public`；注释明确原生数据库不是PG search_path schema。`namespace_names`（3762行）消费该列表，未提供 `s_hidden`/`s_ok` 等独立PG schema目录事实。通过创建其他MySQL数据库冒充PG schema会违背现有目录身份契约。
3. `pkg/server/conn.rs:908 TiDBContext` 提供 `user_identity`、`schema_snapshot`；全文schema/privilege/usage搜索未发现schema USAGE授权接口。`pkg/session/runtime/control.rs:5873 execute_grant` 是MySQL权限实现，5904行将 `mysql::UsagePriv` 映射成空集合；这不是PG schema USAGE许可事实。RustCodeGraph查询 `has_schema_privilege` 无结果。未将身份字符串或MySQL USAGE视为权限许可。
4. `pkg/expression/builtin.rs` 的正式PG签名（2615行）、求值分支（6725行）、工厂（8328/8427行）已支持 `current_database`，而 `current_schema` 只出现在MySQL拒绝集合（9120行）；规划门控 `pkg/session/runtime/planning.rs:3275` 已列其名称，不代表正式工厂已实现。`pkg/expression/lib.rs:1005` 已挂载 `pg08_28_test`；本任务三个 `pg08_29` 测试尚未注册，未伪造已存在测试数。

阻塞归属：PG目录/schema身份所有者（`pkg/server/pg_catalog.rs` 及其共享schema元数据），PG身份/授权事实所有者（`pkg/server/conn.rs::TiDBContext`、`pkg/server/runtime.rs` 到共享session权限状态的桥接）。本编号消费其接口，不拥有完整目录或角色/GRANT/REVOKE系统。

恢复条件：上游提供真实PG schema枚举和存在性，以及针对当前连接身份、可在GRANT/REVOKE后的下一语句更新的schema USAGE检查；明确生产消费者及注册测试，并释放共享文件和参考资源。之后先核对新HEAD与接口，再注册本编号formal/SQL/client测试及唯一case ID清单，领取动态Cargo槽位、运行list与行为红灯，按原计划最小接线。不得通过固定public、测试专用许可表或目录探测特例绕过本阻塞。

参考环境只读检查：`PG18_REFERENCE_DSN`、`PG_LIBPQ_LIBRARY`、`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD` 均unset（只报告配置状态，未输出值）。未连接服务，也未将此环境状态当行为红灯；接口恢复后可沿已有参考启动方式提供真实18.6/MySQL8服务，无需本轮提前启动。

本轮验证为前置只读取证，不适用WIP/Ready代码profile；未运行fmt、三条Cargo测试、make lint、真实客户端或负向自检，无行为通过声明。业务场景预期清单尚未注册，实际0、通过0、失败0、跳过0；此0不构成验收证据。Cargo实际槽位：未领取，CARGO_TARGET_DIR：未设置，无缓存或锁变动。未创建Server、Python客户端、数据库对象、参考实例和临时目录，无此类进程/端口/数据需清理；耗时图调用 `rustcodegraph callers effective_explicit_path --limit 15` 最终自行完成（退出0），返回实际消费者 `pg_extended::prepare`、`pg_extended::reanalyze`、`pg_name::adapt` 和 `effective_schema`；清理探测尝试kill时PID已不存在（退出1），未终止任何其他任务进程。

已运行检查：`git status --short`、`git log -5 --format='%H %s'`、`git rev-parse HEAD`；`rustcodegraph status`；`rustcodegraph explore 'PgSession effective_explicit_path current_schema schema USAGE privilege'`；`rustcodegraph query effective_explicit_path --limit 5`、`query current_schema --limit 10`、`query has_schema_privilege --limit 5`；`rustcodegraph node effective_explicit_path`、`node namespace_names` 及上述接口的按文件行号读取；`rg -n 'schema.*(usage|privilege)|has_schema_privilege|USAGE|UsagePriv|effective_schema|current_schema'` 对任务候选文件搜索；`rg -n 'pg08_29|pg08_28|PgCurrentDatabase'` 核对注册；Python只读环境配置检查；`git diff --check`（退出0）。上述图查询及取证命令成功；对已退出自有进程的kill探测退出1，不影响接口结论。expression/server/session目标包未发现doc.go；未创建schema授权夹具，因此未进入DDL修改流程。验证profile skill缺失，与编号预告一致。

风险/验证限度：未改变生产行为，无新增正确性、兼容性或性能风险；本任务要求的USAGE过滤、授权刷新、普通表达式/行源/嵌套、Parse后SET再Execute、双方言隔离及实际18.6差分仍全部待验证。不得宣称实现完成或删除编号文件。本次更新只记录阻塞、接口归属与恢复条件；所有来源plan.md保持只读。


## 2026-10-30 恢复实施设计（新增用户排障授权）

原“缺接口即停”记录为历史。用户明确授权突破阻塞并实现不可缺少的局部接口，本轮继续，非因参考环境变量未继承而停止。复用target/pg08-28-evidence的Cargo/临时参考启动器（同11参考二进制），复制到target/pg08-29-evidence并保留退出清理。

精确局部设计：expression新增 `EvalContext::PgCurrentSchema() -> Option<String>` 与exprstatic快照选项，正式 `current_schema` 工厂返回name并防折叠。server通过TiDBContext新请求将连接私有search_path同步到共享session；session执行时从目录/ACL事实计算有效首项，不能把Parse时值冻结。namespace与USAGE缺口用新增 `pkg/session/runtime/pg_schema.rs`（在runtime.rs注册）提供有界持久元数据接口：以当前原生database ID为键，在现有Domain KV事务保存独立PG schema名字、owner、USAGE授权；不把MySQL数据库当schema。公共schema/pg_catalog继承既有事实。PG utility命令由既有pg_session lexer解析CREATE/DROP SCHEMA及GRANT/REVOKE USAGE ON SCHEMA，转成类型化命令，经server worker交给session元数据所有者；不是current_schema探测实现或第二求值器。不新增角色系统：复用既有CREATE ROLE、GRANT角色、SET ROLE生产逻辑及其active_roles，ACL按当前有效角色检查，root bootstrap对应管理身份；未授权角色不能任意切换。作用域限空schema目录与USAGE元数据，不实现schema内表DDL/完整权限系统。新模块旧路径不适用原因：namespace_rows只有builtin映射，MySQL UsagePriv是无权限位，均不能持有独立PG schema ACL。所有生产修改限该契约、动态上下文、普通SQL和name元数据局部接线。

TDD先注册formal、SQL、client三个原定名字，工厂/普通SQL未实现时得到运行期断言红灯；新增上下文方法/选项只提供可构造测试接口，不预先实现函数逻辑。业务ID首批：formal A/B/A、empty、arity、dialect共6；SQL沿28的三表达式×零行/非零×Q/P0/P1及2错误恢复共20，追加path/Parse-SET/ACL独立ID。客户端基础沿28的49个ID，更名schema主题；新增路径及ACL刷新ID在运行前登记。原始预期数随新增测试同步记录，不以循环/字段assert充数。


新增测试运行前场景登记：formal6；SQL29（原20，加path-catalog/path-empty/path-missing/path-public、parse-set-execute、acl-mixed/acl-hidden/acl-grant/acl-revoke共9）；客户端72唯一ID（原49，加path-{catalog,empty,missing,mixed,hidden,grant,revoke}-{Q,P0,P1}共21与refresh-P0/P1共2）。无参数函数NULL输入不适用；空路径产生SQL NULL属于业务场景。实际运行计数待收集。测试名字list各1；初次接口测试编译import错误不算红灯，修正后formal、SQL、client均实际1测试运行失败（退出101），证据red-tests.log、red-final.log。真实临时参考生命周期已清理。

SQL授权回归产生新的真实红灯：`SET ROLE <owned-role>` 被pg_result.rs::command_for_phase拒绝0A000，已有session execute_set_role未被调用。只将SetRoleStmt归入SET command gate，不重做执行器或角色系统；此为必要局部接线。基础formal6已通过，SQL扩展场景未全绿前不宣称完成。

当前WIP：formal6/6、SQL29/29通过；实际客户端72/72，66通过、6失败、0跳过，失败仅M-{database,version,connection_id}-{Q,P}，与批次28原始观测同一未修改路径。增加第73个唯一ID I-path-isolation：A路径missing,s_hidden,s_ok，B保持public，M读健康表保持7；在运行前登记。最后DROP空schema时清除恢复为默认值的KV扩展记录，不删除修改过的builtin ACL。所有参考实例生命周期及监听器已关闭。


## 最终交接：2026-10-30，已完成，待回归

目的行为已实现：current_schema普通表达式按执行期显式path选择存在且具有USAGE的首项，返回独立name身份；空有效路径返回SQL NULL。formal6/6、SQL29/29、客户端57个PG/错误恢复/交错场景通过。当前完整客户端73/预期73/去重73，67通过、6失败、0跳过；独立value/OID/SQLSTATE负向自检3/3。成功分类success11、zero4、binding32（无参数载荷，这里指无参prepared执行）、error-recovery6、interleaved4、mysql10；12个空路径NULL观测包含在业务ID中，不追加总数。NULL输入不适用。libpq无参数时未伪造text/binary输入矩阵，结果text/binary均覆盖，TCP补充statement/portal Describe及Execute不得重发RowDescription、事务E→ROLLBACK→I与Sync恢复。

失败仍为M-{database,version,connection_id}-{Q,P}：MySQL系统函数metadata的code/flags/charset/length/decimals与自身MySQL8参考不一致，以及VERSION值与独立客户端握手身份不一致。DATABASE值、CONNECTION_ID与独立连接事实4项关系断言通过，VERSION两项关系失败未屏蔽。`pkg/session/runtime/system_query.rs`、`pkg/server/server.rs`与任务起始HEAD相同；批次28已有相同六项原始失败，本轮实际重现，未重编独立HEAD基线作对照。其修复归MySQL系统函数/协议拥有者，不吞并任务32或扩建MySQL系统函数。恢复条件：修复这些既有差异后原样运行本编号三条聚焦命令和适用Ready检查，完整成功后再删除本编号；当前保留严格失败断言。

生产修改及必要局部接线依据：

| 文件/符号 | 本任务契约依据 |
| --- | --- |
| expression/builtin.rs签名、PgCurrentSchema求值、正式工厂及有界注册 | formal A/B/A、NULL、参数个数、方言；SQL及真实客户端必须从同一工厂求值 |
| expression/core_support.rs、function_traits.rs、builtin_threadsafe_generated.rs | 禁止把会话schema冻结为常量、非法生成列或跨会话共享；core_support_test和formal实际折叠入口验证 |
| expression/exprctx/context.rs、exprstatic/evalctx.rs、expression/context.rs | 传递Option<String>执行期有效schema，装饰器与静态转换保留该事实；不固定public |
| session/runtime/query.rs、relational_value.rs | 行源/嵌套/普通表达式消费正式工厂，分析返回name，不按值猜OID |
| session/runtime/pg_schema.rs（runtime.rs注册）、runtime/session.rs | 按真实数据库ID持久化独立空PG schema及USAGE ACL；复用既有有效角色，不用MySQL数据库伪装schema；GRANT/REVOKE下一语句新读事实 |
| server/conn.rs、runtime.rs、pg_session.rs、pg_name.rs | 将连接私有path及有界schema utility交给共享worker；无FROM表达式也同步执行上下文；移除current_schema单独SELECT探测截获 |
| server/pg_result.rs | 允许已存在SetRoleStmt执行器经PG SET门控用于实际权限身份，无新角色系统 |
| 独立pg08_29_test、pg_query_test、pg_client_integration_test、pg08_29_libpq及挂载、pg_session_test | 独立factory/普通SQL/真实双方言与原parser边界回归，不将测试放生产文件 |

该局部前置接口仅实现空schema目录和USAGE管理，不实现schema内表DDL、完整pg_namespace扩展展示或完整角色/权限系统；unsupported schema utility形式拒绝向MySQL数据库DDL降级，事务内namespace utility延续有界DDL拒绝策略。root管理身份来自当前bootstrap root认证，SET ROLE资格复用已有共享角色校验；未新增登录或绕过鉴权。实现不改变MySQL连接sql_mode、数据库事实或活跃角色。正确性风险限这些明确有界接口；未验证完整DDL/授权语法或角色继承系统。兼容性为PG新增工厂/name及局部metadata key，不改Cargo依赖、Go或Bazel。性能风险为有效路径刷新需要读取小型namespace KV记录，未运行性能基准；按任务范围未进行RealTiKV或全包广扫。

验证profile为WIP聚焦及Ready适用检查（profile skill确实缺失，不宣称运行该技能）。Ready包含fmt、三条原命令、5个相邻过滤器、make lint及diff检查。所有8个过滤器list均恰好1测试；formal和SQL及5相邻均1测试通过，client恰好1测试失败但真实执行73业务场景。make lint退出0，未触发Go/依赖/Bazel变更，不需bazel_prepare，未运行bazel_lint_changed或Go failpoint测试。最终只改了上下文选项注释位置与NULL路径计数报告（12），又运行fmt和原客户端命令；行为断言未降低。

构建槽位：主要所有实际行为验证动态领取1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。一次早期formal预备编译动态领取2（/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2），发现冷缓存后主动TERM自有runner并回收其子进程，退出143，不计行为证据；随后在已释放的1重新运行得到真正红灯。最初接口测试的import编译错误退出101也未当红灯。所有槽位锁已释放，两个缓存保留。正常、失败和中断路径由cargo-runner和references-runner的finally清理。

临时参考复用现有二进制：PostgreSQL18.6、libpq180006，MySQL8.0.46原生客户端及参考；tzdata2026c。身份/编码/locale/TimeZone/sql_mode、真实二进制版本及hash、客户端版本、fixture hash、当前基线完整HEAD、Rust二进制hash和完整暂存pkg差异hash均存原始build/reference/mysql日志，未输出DSN。所有owned schema、角色、source/health表、数据库与连接、Server监听器、临时参考PID及数据目录均清理，进程已wait、端口拒绝连接；未停止用户服务。仅留下脱敏证据与编译缓存。

最终原始证据目录：`target/pg08-29-evidence/30048-62079`。summary.json、build.jsonl、reference-A/B.jsonl、aster-A/B.jsonl和mysql-reference/aster.jsonl保留独立观测；server清理`target/pg08-29-evidence/30048-62079.cleanup.json`，参考生命周期`target/pg08-29-evidence/reference-lifecycle-*.json`均cleanup_verified=true；检查锁目录为空。fixture SHA256 `b461f36ba6616025c67d6088f22035ef9abeecb7a25fb6604b89a904093c6f3c`，起始HEAD `843c783d928239d85ce2a8460521ae8117257bd4`；提交hash由最终交接报告。

确切交付命令、退出码与日志（工作目录为仓库根，均继承上述绝对CARGO_TARGET_DIR）：

    cargo fmt --all # exit=0; ready-fmt.log
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_29_test::pg08_29_formal -- --exact --list # exit=0; ready-pg08_29_test-pg08_29_formal--list.log
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_29_test::pg08_29_formal -- --exact --nocapture # exit=0; ready-pg08_29_test-pg08_29_formal--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_29_sql -- --exact --list # exit=0; ready-pg_query_test-pg08_29_sql--list.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_29_sql -- --exact --nocapture # exit=0; ready-pg_query_test-pg08_29_sql--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_29_clients -- --exact --list # exit=0; ready-pg_client_integration_test-pg08_29_clients--list.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_29_clients -- --exact --nocapture # exit=101; ready-pg_client_integration_test-pg08_29_clients--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib core_support_test::function_trait_sets_match_go_exactly -- --exact --list # exit=0; ready-core_support_test-function_trait_sets_match_go_exactly--list.log
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib core_support_test::function_trait_sets_match_go_exactly -- --exact --nocapture # exit=0; ready-core_support_test-function_trait_sets_match_go_exactly--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_28_test::pg08_28_formal -- --exact --list # exit=0; ready-pg08_28_test-pg08_28_formal--list.log
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_28_test::pg08_28_formal -- --exact --nocapture # exit=0; ready-pg08_28_test-pg08_28_formal--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_28_sql -- --exact --list # exit=0; ready-pg_query_test-pg08_28_sql--list.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_28_sql -- --exact --nocapture # exit=0; ready-pg_query_test-pg08_28_sql--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg_introspection_namespace_session_state -- --exact --list # exit=0; ready-pg_query_test-pg_introspection_namespace_session_state--list.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg_introspection_namespace_session_state -- --exact --nocapture # exit=0; ready-pg_query_test-pg_introspection_namespace_session_state--nocapture.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_session_test::pg_introspection_namespace_parser_boundaries -- --exact --list # exit=0; ready-pg_session_test-pg_introspection_namespace_parser_boundaries--list.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_session_test::pg_introspection_namespace_parser_boundaries -- --exact --nocapture # exit=0; ready-pg_session_test-pg_introspection_namespace_parser_boundaries--nocapture.log
    make lint # exit=0; ready-lint.log
    git diff --check # exit=0; ready-diff.log
    cargo fmt --all # exit=0; final-fmt.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_29_clients -- --exact --nocapture # exit=101; final-client.log
    git diff --check # exit=0; final-diff.log
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_29_clients -- --exact --nocapture # exit=101; delivery-clients.log

阶段启动命令：`python3 target/pg08-29-evidence/cargo-runner.py python3 target/pg08-29-evidence/references-runner.py python3 target/pg08-29-evidence/ready.py`（聚合退出1，因严格client失败）；最后暂存后客户端复验同包装器运行原client cargo命令，内层退出101、临时实例和锁正常清理。完整命令机器记录在ready-commands.json及commands.jsonl。额外静态检查`python3 -m py_compile pkg/server/testdata/pg08_29_libpq.py`、`git diff --cached --check`退出0。所有变更只提交本任务，外部未跟踪计划目录排除。

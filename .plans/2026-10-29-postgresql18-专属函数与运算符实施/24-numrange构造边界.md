# 任务 24: numrange构造边界

批次：【实施批次 24】；语义依赖：23；互斥前序：批次23结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：numrange(numeric,numeric[,text]) 保留连续边界。

来源任务：大计划8“专属函数与运算符”；路线图第4章“范围函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

独立numrange/subtype/codec依赖；任务23可复用构造接线，不能复用离散步长。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_compare.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_24_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_24_test;`，新增测试`pg08_24_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_24_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_24_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_24_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

1至3的[)/(]/[]/()分别保留；同端点[]非空，其余empty；缺端点无界；NULL flags/逆序/非法flags按22000/42601；(1,4,(])不得变[2,5)。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

MySQL保留lower('AbC')/upper('AbC')字符串语义；int4range(1,3)、numrange(1,3)、isempty(NULL)按本任务名检查拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_24_test::pg08_24_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_24_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_24_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_24_sql`的TCP断言补充，未知字段不能默认相等。

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

- [x] 局部接口取证、授权设计与当前测试名注册
- [x] 三项真实运行期红灯，原命令复验与当前证据
- [x] numrange连续边界及最小类型/codec/工厂/协议接线，formal及SQL通过
- [ ] 真实参考、清理及Ready适用检查已执行；剩余8项无关MySQL通用元数据差分待回归

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。


## 2026-10-30 当前依赖门槛取证与阻塞

本轮为实施批次24，语义依赖23。当前完整HEAD为`8aab0d67fe4aebd77c257e69d7607ebae748e9a0`。已读取整体plan.md（仅架构、未修改）、本文件、根及server AGENTS.md、PLANS.md、testing-flow.md及仓库rustcodegraph技能。通过`rg --files pkg/expression pkg/server pkg/session pkg/types`核对未发现这些目标包的doc.go。仓库索引status成功（11800文件，7075 Rust文件），随后用explore/query/node核对接口；图未命中numrange符号后用rg补证。

互斥门槛已满足：`list_threads`显示批次23会话`01a1899c-287c-77f0-a04c-3c049b9eb360`为idle，read_thread确认其最后一轮completed，已提交上述完整SHA且报告临时实例/监听/测试对象/槽位释放。当前`target/rust-slot-locks`为空。本轮未占用共享源码或参考服务。

可复用接口及实际消费者：`pkg/types/pg_int4range.rs::PgInt4Range`具有OID3904/subtype23、Option<i32>端点、empty、checked构造和text/binary codec；第71–84行对排除下界及包含上界递增，有限边界统一[lower,upper)，故不能复用于numrange连续语义。`pkg/parser/mysql/type.rs:234`仅提供TypePgInt4Range=0xdf。`pkg/expression/builtin.rs::resolve_pg_typed_signature`第2802行只接int4range；正式工厂/构造实现由批次23提交保留。`pkg/server/pg_result.rs:566`使用OID3904并调用PgInt4Range，`pkg/server/pg_extended.rs:1259`只列范围OID3904。批次23独立测试/客户端已注册，但其历史结果不计本轮证据。

缺失的必需接口：没有独立numrange身份（OID3906/subtype numeric1700）的类型名/FieldType映射，没有保留numeric精度及上下界开闭标志的规范载体，没有对应text/binary范围codec及typed NULL/零行/绑定/结果消费者。`rustcodegraph query PgNumRange --limit 10`及`query TypePgNumRange --limit 10`均返回No results。补充`rg -n -i 'numrange|pgnumrange|3906' pkg/types pkg/parser pkg/expression pkg/session pkg/server --glob '*.rs' --glob '*.py' --glob '!testdata/parser.rs'`只发现numrange拒绝名称/批次23 MySQL拒绝夹具；generated/main_tables.rs中的3906是解析器状态编号，不是类型身份。不能把text结果或int4range整数步长伪装成本类型。

接口拥有者：上游PG范围类型/codec交接（pkg/types、pkg/parser类型身份），及其session/server协议消费接线；批次23本身仅实现int4range，不提供numrange，未将批次23待回归的MySQL通用元数据问题当作本次语义阻塞。当前请求未授权补建缺失范围类型接口，本任务“独立numrange/subtype/codec依赖”和里程碑一要求缺接口时停止，不递归扩大范围。

环境门槛：仅检查环境变量是否存在，未输出值或DSN；PG18_REFERENCE_DSN、PG_LIBPQ_LIBRARY、MYSQL8_REFERENCE_HOST/PORT/USER/PASSWORD均未配置。未探测或启动用户服务；不宣称本轮真实18.6/MySQL参考验证。可选PG_LIBPQ_LIBRARY未配置本身不构成阻塞。

恢复条件：上游提供可消费的numrange独立身份、保留连续边界的numeric载体及codec/普通SQL桥接，或用户明确授权本批补齐这些最小前置接口并先记录设计；提供可用真实PG18.6/MySQL参考环境（也可在授权范围内建立本任务临时参考）。然后先登记唯一case ID/预期计数，新增并注册pg08_24_formal/sql/clients和夹具，动态领取槽位、--list确认各1项，取得真实行为红灯再最小修复及原命令复验。

本轮确切检查：`git status --short`、`git rev-parse HEAD`、`git show --stat --oneline HEAD`；`~/.rustcodegraph/bin/rustcodegraph status`、`explore 'numrange PgNumRange PgInt4Range resolve_pg_signature'`、上述两个query、node读取builtin.rs:2760/8830、pg_int4range.rs:1、parser/mysql/type.rs:1/215、pg_result.rs:565、pg_extended.rs:1230；上述rg搜索；`rg -n 'pg08_24' pkg/expression/lib.rs pkg/server/pg_query_test.rs pkg/server/pg_client_integration_test.rs`无匹配（退出1，尚未注册）。其他命令退出0。list_threads/read_thread完成前序状态只读核对。未运行任何Cargo命令、Cargo脚本、make lint或测试；WIP/Ready尚未进入，行为有效运行数0，不把环境缺失、未注册或编译错误当红灯。未领取Cargo槽位，实际槽位及CARGO_TARGET_DIR均不适用；无本任务进程/锁/临时服务/对象需清理。

Progress：已完成局部接口与前序资源取证；测试注册、行为红灯、生产修复、真实客户端和Ready均等待前置接口。Decision：按既定依赖门槛暂停，仅更新本编号，不修改生产/测试、来源plan.md或父任务8，不提交仅编号状态/取证更新。Outcomes：状态已阻塞，保留本文件用于恢复，不宣称实现完成或当前通过。风险：连续边界、numeric精度、OID与binary兼容性尚未验证；本轮无运行时行为或性能改动。工作区原有未跟踪计划目录保持不动。


## 2026-10-30 新授权与最小接口设计

用户通过父会话明确授权突破缺失前置，覆盖此前暂停决定。仅补numrange：parser/mysql/type.rs新增TypePgNumRange=0xde（OID3906/subtype1700），parser_actions/expression.rs和types/field_type.rs添加类型名/Restore；types/pg_numrange.rs使用已有PgNumeric保留精度、display scale、NaN/Infinity及边界开闭，不用i32或f64；实现checked构造、text/binary范围codec，挂types/lib.rs。numeric比较在本范围模块通过规范十进制字符串进行精确比较，不更改通用numeric系统。独立pg_numrange_test.rs挂载载体回归。

expression/builtin.rs新增两/三参签名、PgNumRange工厂/字符串求值分支，消费PgNumeric输入；builtin_threadsafe_generated.rs登记不可变签名。session/runtime/query.rs、relational_value.rs、planning.rs、control.rs按批次23相同正式消费点最小补numrange、typed NULL/零行、COALESCE、cast及错误；server/pg_result.rs、pg_extended.rs补独立OID/codec。无新依赖、Go/Bazel更改，不开放范围算子、列DDL或其他类型。上述生产变更均是本构造行为及参数/结果验收必需局部接线。

测试先注册pg08_24_formal、pg08_24_sql、pg08_24_clients和pg08_24_libpq.py，复用批次23结构但将离散断言替换为连续边界及numeric wire载荷。唯一case ID直接固定于夹具CASE_IDS，并在运行前同步本文件；工厂覆盖四种flags/相同端点/无界/精度/错误，客户端覆盖参数四组合、NULL重复、range codec和真实行投影。参考实例复用PG18.6 install与mysql@8.0本地二进制，为本任务建独立临时数据目录、随机端口，finally停止wait删除，不使用用户服务。

客户端唯一case IDs（预期97，当前未运行）：C-Q-empty, C-Q-equal, C-Q-bounds, C-Q-lower, C-Q-upper, C-Q-both, C-Q-null, C-Q-zero, C-Q-zero-three, C-Q-nested, C-Q-row, C-Q-row-zero, C-Q-flags-open, C-Q-flags-closed, C-Q-flags-left, C-Q-precision, C-Q-equal-open, C-Q-equal-right, C-E-reverse, C-recover-reverse, C-txn-reverse, C-E-null-flags, C-recover-null-flags, C-txn-null-flags, C-E-bad-flags, C-recover-bad-flags, C-txn-bad-flags, C-E-signature, C-recover-signature, C-txn-signature, C-bind-0-0-0-0-0, C-bind-0-0-0-0-1, C-bind-0-0-0-0-2, C-bind-0-0-0-1-0, C-bind-0-0-0-1-1, C-bind-0-0-0-1-2, C-bind-0-0-1-0-0, C-bind-0-0-1-0-1, C-bind-0-0-1-0-2, C-bind-0-0-1-1-0, C-bind-0-0-1-1-1, C-bind-0-0-1-1-2, C-bind-0-1-0-0-0, C-bind-0-1-0-0-1, C-bind-0-1-0-0-2, C-bind-0-1-0-1-0, C-bind-0-1-0-1-1, C-bind-0-1-0-1-2, C-bind-0-1-1-0-0, C-bind-0-1-1-0-1, C-bind-0-1-1-0-2, C-bind-0-1-1-1-0, C-bind-0-1-1-1-1, C-bind-0-1-1-1-2, C-bind-1-0-0-0-0, C-bind-1-0-0-0-1, C-bind-1-0-0-0-2, C-bind-1-0-0-1-0, C-bind-1-0-0-1-1, C-bind-1-0-0-1-2, C-bind-1-0-1-0-0, C-bind-1-0-1-0-1, C-bind-1-0-1-0-2, C-bind-1-0-1-1-0, C-bind-1-0-1-1-1, C-bind-1-0-1-1-2, C-bind-1-1-0-0-0, C-bind-1-1-0-0-1, C-bind-1-1-0-0-2, C-bind-1-1-0-1-0, C-bind-1-1-0-1-1, C-bind-1-1-0-1-2, C-bind-1-1-1-0-0, C-bind-1-1-1-0-1, C-bind-1-1-1-0-2, C-bind-1-1-1-1-0, C-bind-1-1-1-1-1, C-bind-1-1-1-1-2, M-Q-int4range, M-prepare-int4range, M-Q-numrange, M-prepare-numrange, M-Q-isempty, M-prepare-isempty, M-Q-lower, M-prepare-lower, M-Q-upper, M-prepare-upper, M-Q-health, M-prepared-health, I-before, I-middle, I-after, C-bind-range-0-0, C-bind-range-0-1, C-bind-range-1-0, C-bind-range-1-1。工厂预期16，SQL预期47。


## 当前WIP里程碑（2026-10-30）

red阶段三项--list各恰好1项/退出0；三项运行各退出101且均进入实际断言：formal缺工厂，SQL返回错误消息而非范围行，真实libpq首个C-Q-empty在PG18.6成功而AsterSQL为42883。原始输出保存在target/pg08-24-evidence/red-*.log；环境及编译错误未计作红灯。该阶段finally已停止wait两参考进程、关闭端口、删除独立数据目录并释放slot1（red-lifecycle.json）。

WIP最小实现已完成，增加pg_numrange规范载体，精确十进制比较（复用既有pg_extremum_numeric_order算法于本模块），保留显示scale及开闭标志，已接正式工厂和独立OID。范围共用相同诊断结构PgInt4RangeError，通过模块别名PgNumRangeError复用既有错误桥；仅将既有错误new构造可见性改为pub(crate)，不改变int4range行为。测试实际需求使注册表PG_EXTENSIONS计数增加numrange。SQL补六个连续/精度用例，预期由47变为53；客户端保持97唯一case，formal16。WIP formal已1项/16场景通过，尚待最终格式化及完整复验；make lint及git diff --check退出0。具体命令、阶段、日志、退出码和动态槽位均写入commands.jsonl/delivery-commands.jsonl，不引用历史通过为本轮通过。


## Ready证据与交付边界（2026-10-30，最终复验中）

本轮已解除numrange前置和参考环境门槛，早期“已阻塞”段落为执行历史。正式生产路径为builtin.rs签名→registered factory→NewFunctionBase→EvalString(PgNumeric/PgNumRange)，query/relational_value复用此正式工厂；parser类型动作及FieldType Restore、planning参数推断、control prepared上下文、pg_result及pg_extended分别保留独立3906身份，不按值猜OID，无SQL文本替换或第二套求值器。新增生产模块仅pkg/types/pg_numrange.rs；独立测试pg_numrange_test.rs，types/lib.rs已注册。全部原PingCAP版权保留，新自建模块仅AsterSQL版权。

ready阶段cargo fmt --all退出0，三项指定--list各1项/退出0，formal1项/16场景退出0，SQL1项/53场景退出0，客户端1项退出101。真实PG18.6/MySQL8.0.46客户端预期/实际/去重均97、通过89、失败8、跳过0；PG成功14、typed NULL1、零行3、绑定52、错误4、Sync后恢复4、事务E→ROLLBACK→I恢复4，共82全部通过。MySQL构造/函数拒绝6及真实健康表prepared1通过。三项独立值/OID/SQLSTATE污染自检通过，单独计数。A/B/M交错真实健康表值/列定义匹配，但额外SELECT1列定义差异使三个交错case整体仍失败，不宣称交错完全验收。

8项未通过为I-before/I-middle/I-after、M-Q-health的MySQL SELECT1通用列定义差异，及M-Q-lower/M-prepare-lower/M-Q-upper/M-prepare-upper的flags/charset/length/decimals差异；lower/upper值abc/ABC正确。原始query/prepare/execute列定义、值、警告/错误均保留，不屏蔽或白名单这些差分。WIP与Ready两轮同8项差异。与本批生产修改前red阶段I-before保留观察一致，也与批次23旧问题吻合；本轮证据独立重跑确认，不引用历史为当前通过。接口拥有者是MySQL表达式返回描述及server原生列定义；恢复条件为其修复SELECT1及lower/upper通用元数据，再原命令复验97/97。不将本构造任务扩展为MySQL元数据系统修复。

SQL首轮WIP在复制int4range测试时仍留“上界2147483647包含时22003”的错误预期；numrange没有离散递增，此输入合法，因此删去该整数专属错误预期（并非放宽本numrange行为断言），补充的连续四边界/精度/同端点场景保留。Ready SQL53全部通过。另一次相邻注册表命令误用不存在的过滤器，得到0测试；该输出不计通过，最终阶段使用真实builtin_registry_matches_complete_go_function_table精确名称重跑。没有将零测试当验收证据。

额外检查：make lint、git diff --check、make parser、make parser_yacc、make parser_fmt、make parser_unit_test均退出0。parser单测前./tools/check/failpoint-state.sh enable go退出0，gofmt仅处理本次生成binding__failpoint_binding__.go后目标一次通过，finally disable go退出0；最终Go/y/Bazel无差异。没有Go/import/dependency/Bazel更改，未触发bazel_prepare，未运行禁止默认的bazel_lint_changed。仅选定本函数/codec、相邻int4range及注册表，未做全包广扫、RealTiKV、性能或范围列DDL测试。

参考/客户端身份保留于每次独立目录reference.jsonl、mysql-reference.jsonl、build.jsonl；记录PG version/server_version_num180006、libpq版本、mysql8.0.46/client版本、真实二进制/fixture hash、HEAD/source diff hash、编码/locale/sql_mode及tzdata来源。PSQL额外实测同端点[1.00,1.0]保留scale、[-Infinity,Infinity]、[NaN,NaN]及相等不同scale半开empty，并据此补类型单测。

资源：每阶段原子mkdir动态领取slot1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；red/wip/ready各自finally已terminate/wait参考进程、端口关闭、独立数据目录删除及owner/锁释放，缓存保留，见对应*-lifecycle.json。Rust客户端cleanup.json确认本任务数据库删除、PG/MySQL监听关闭；summary.json确认参考表/库删除、TimeZone/locale恢复、连接关闭。未停止任何用户服务。最终阶段同样完成后复核，不在产物进程活跃时提前释放槽位。

精确命令、退出码、日志路径和Cargo槽位逐项记录在target/pg08-24-evidence/commands.jsonl；Go/Ready检查在delivery-commands.jsonl、parser-commands.jsonl。最终delivery.json将汇总文件清单与当前有效证据。范围风险：连续端点/精度/类型/codec聚焦证据已有；MySQL通用元数据兼容性仍欠8项，性能/TiKV/DDL/其他range函数未验证。只提交本批变更，排除工作区无关未跟踪计划，不修改任何plan.md或父编号8。


## 最终结果（2026-10-30）

最终状态为已完成，待回归，取代最初阻塞状态；本任务实现及聚焦PG验证已完成，只剩上节8项独立MySQL元数据回归，因此保留本编号文件且不宣称完整双方言97/97通过。所有来源plan.md/父编号8只读。

final阶段cargo fmt --all退出0；三项指定--list各1项/退出0，formal1项16场景退出0，SQL1项53场景退出0，client1项退出101。额外codec、相邻int4range formal1项10场景及SQL1项48场景、真实注册表exact测试均各--list1项且运行1项退出0；此前误过滤的零测试未计入证据。最终客户端仍97实际/89通过/8失败/0跳过，原始双方观测目录为`target/pg08-24-evidence/56842-56414`。错误阶段补证在final SQL输出：逆序/NULL flags/非法flags返回tags[1,2,E,Z]，在Parse/Bind完成后的Execute错误；非法cast/混合COALESCE为[E,Z]，在Parse分析错误；ReadyForQuery E、ROLLBACK恢复I和健康SELECT均通过。

最终只读复核red/wip/ready/final生命周期：参考进程全部结束、所有自有端口不可连接、数据目录已删除、slot1锁已释放；Rust cleanup.json/summary.json清理字段全部真，缓存保留。所有阶段实际CARGO_TARGET_DIR均为/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。Make lint/Parser证据覆盖未再改动的Go检查面；最终只改SQL测试日志以保留错误发生阶段，随后已格式化并重跑指定测试，不改变生产逻辑。

交付清单及确切命令/退出码/有效数/风险限制汇总于target/pg08-24-evidence/delivery.json。按git-commit只提交本批生产、测试和编号取证文件，排除原有无关未跟踪计划。下一步由MySQL返回列定义拥有者修复上述8项通用元数据后，原样复验97/97；本批不扩写该系统。

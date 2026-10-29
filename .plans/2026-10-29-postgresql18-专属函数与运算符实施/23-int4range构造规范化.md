# 任务 23: int4range构造规范化

批次：【实施批次 23】；语义依赖：无本目录语义前置；互斥前序：批次22结束或释放资源。不安排并行；来源为拆分批次8，不等于实施批次8。

状态：已完成，待回归

目的：int4range(int4,int4[,text]) 产生真实规范range值。

来源任务：大计划8“专属函数与运算符”；路线图第4章“范围函数契约”的模块、目标行为、范围外、验收、后续细分；原蓝图工作流4。

预计会话范围：只处理上述一个签名行为及必要生产/工厂接线、独立测试与客户端断言；不创建纯文档路线图任务，不实现接口所属完整子系统。保持此编号文件为遵循根`PLANS.md`的活动ExecPlan。

## 上下文与依赖门槛

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

必须先交付独立int4range/subtype身份、empty/无界/开闭载体及text/binary codec；当前扫描未见通用range模块。缺失时只登记上游接口，不在此创建range子系统。

实施前只读取本目录plan.md、本文件、根AGENTS.md、pkg/server/AGENTS.md、目标包doc.go（存在才读，规划时expression/server/parser未发现）、docs/agents/testing-flow.md。代码导航先读skills/rustcodegraph/SKILL.md并检查索引；图未覆盖再用rg。对每个前置核对保留代码、实际生产消费者、注册测试和最新完整SHA，将可用接口记录于本文件，不能以历史绿色记录代替当前验证。当前不确认的依赖只在本编号记录，规划交接阻塞集中于父编号8；不修改任何来源plan.md、覆盖表或原路线图。

## 文件与正式接线

候选生产修改（先证明必要性，不要求全部修改）：`pkg/expression/builtin.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/server/pg_result.rs`、`pkg/expression/builtin_compare.rs`。

沿`builtin.rs::resolve_pg_signature`/`registeredFunctionClass::getFunction`/`CORE_BUILTIN_FACTORIES`到`scalar_function.rs::newFunctionImpl`、`NewFunctionBase`及所属求值分支接线；运算符/特殊谓词保持自己的绑定模型。`pkg/session/runtime/planning.rs`负责分析身份，`query.rs`消费已定型表达式；普通投影、行源及嵌套表达式均须到正式工厂，不按SQL文本/值猜OID。返回身份交`pg_result.rs`，参数通过既有`pkg/server/pg_extended.rs`；只有确有局部遗漏才修改该文件。系统事实桥接不得在目录查询解析器加探测特例替代普通SQL。

待新增独立测试文件`pkg/expression/pg08_23_test.rs`，在`pkg/expression/lib.rs`只挂`#[cfg(test)] mod pg08_23_test;`，新增测试`pg08_23_formal`通过NewFunctionBase实际构造和求值；不把测试函数写入生产文件。在已挂载的`pkg/server/pg_query_test.rs`新增`pg08_23_sql`，在`pkg/server/pg_client_integration_test.rs`新增`pg08_23_clients`；测试前缀为本计划定义，当前均不存在。不得修改Cargo manifest以假装注册：已核对expression/server为lib.rs且autotests=false，测试挂载须真实有效。

待新增客户端夹具`pkg/server/testdata/pg08_23_libpq.py`，由上述Rust客户端测试启动。复用`pg_function_23_libpq.py`的独立libpq、`pg_function_24_libpq.py`/`pg_function_27_libpq.py`的原生MySQL预处理与观测方式，不另建通用平台、不调用参考端数据替代AsterSQL结果。若需要新增生产辅助模块，先在本文件写明精确路径/符号/注册和为何旧模块不适用；不得在未记录设计时扩大范围。

## 单一行为与验收用例

(4,4)→empty、(4,4,[])→[4,5)；(1,4,(])→[2,5)；缺端点→无界；双方NULL及[]→(,)；反向端点/NULL flags→22000、bad flags→42601、最大闭上界溢出22003。

每条SQL使用显式源类型、独立SELECT与别名v，保留上述所有正常/边界/非法/NULL用例；本主题的其他签名由映射任务负责。参数变体使用对应$1源及$2单位/模板/维号/元素等，EXTRACT字段是语法节点不作为Bind参数。每个签名追加零行与typed NULL描述、嵌套表达式或真实行源投影，排除只支持无FROM探测的假实现。无参数系统函数不虚构参数载荷；无NULL输入的函数明确记录不适用，仍检查零行描述。范围与数组必须断言规范shape/下界或empty/无界，不只比较显示字符串。

MySQL保留lower('AbC')/upper('AbC')字符串语义；int4range(1,3)、numrange(1,3)、isempty(NULL)按本任务名检查拒绝。

## 测试驱动计划与确切命令

先新增上述测试/夹具和必要测试挂载，不改生产逻辑。以下命令从仓库根目录执行，使用默认features（不加--all-features/nextgen），每条目标有效测试数恰好1；先用同过滤器`-- --list`核对名字再执行，零测试、编译错误、缺参考服务都不算行为红灯。

失败验证命令：

    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg08_23_test::pg08_23_formal -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg08_23_sql -- --exact --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_client_integration_test::pg08_23_clients -- --exact --nocapture

预期运行期失败：已选行为的缺失工厂/签名或错误值、NULL、类型、SQLSTATE/上下文断言与真实参考不一致。任务1及任何已正确的子用例可能先绿：记录当前通过并只补缺覆盖；不得改坏实现制造失败，不强制无意义生产修改。只有新增失败场景授权对应最小修复，原有Go语义与测试意图不得删减。

通过验证前先在槽位环境运行`cargo fmt --all`，再原样执行上面三条命令，必须同输入同断言由红转绿。正式工厂测试与真实SQL不能互相替代。客户端测试必须断言子进程成功且有效场景非零，未配置环境直接失败不skip。

## 真实参考、客户端与有效计数

客户端夹具继承`PG18_REFERENCE_DSN`、可选`PG_LIBPQ_LIBRARY`；MySQL参考继承既有`MYSQL8_REFERENCE_HOST`、`MYSQL8_REFERENCE_PORT`、`MYSQL8_REFERENCE_USER`、`MYSQL8_REFERENCE_PASSWORD`及既有原生库配置。执行前只读确认这些环境变量已配置，不输出值或DSN。本任务必须有真实PG18.6参考和MySQL自身参考；18.4可另列观察，不替代18.6目标。记录version()/server_version_num、真实二进制/客户端版本、HEAD、fixture hash、tzdata、编码、locale及sql_mode。已有18.6其他类型证据不等于本任务通过。

Rust测试复用`CreateAnalyzeSession`与`start_client_server`启动同一Server临时PG/MySQL listener；独立Python libpq采Q和Parse/Bind/Describe/Execute，原生MySQL采Query/Prepare/Execute。PG有参数时覆盖text/binary参数×text/binary结果四组合，重复值/NULL绑定；零行与全NULL仍按已分析签名检查OID、typlen、typmod，记录列名、NULL标志、原始载荷。statement与portal Describe分别断言，Execute不能重发RowDescription；libpq不暴露字段由`pg08_23_sql`的TCP断言补充，未知字段不能默认相等。

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

- [x] 局部接口取证、最小前置设计与三项测试注册
- [x] 三项运行期红灯、codec补充红灯与当前绿色证据
- [x] int4range最小类型/载体/工厂/协议接线；当前formal及SQL通过
- [ ] 真实参考与Ready适用检查已执行；保留MySQL通用元数据差分待回归

## Surprises & Discoveries

规划只进行了静态取证；当前实现状态与旧文档不同，必须复用本文件基线，不把历史缺口重新实现。本任务未运行测试。

## Decision Log

2026-10-29：选择正式共享工厂与已有规范值；拒绝SQL文本替换和独立PG求值器，因为它们绕过类型、NULL、会话及错误语义。批次保守串行以避免共享工厂/测试资源冲突。依赖未满足时只登记局部接口，不扩大范围。

## Outcomes & Retrospective

仅计划已生成，实施与验证尚未执行；未来在本节记录实际结果、证据路径和未决依赖，不写回只读plan.md。


## 2026-10-30 批次23实施取证与阻塞交接

本轮确认实施批次为23，语义依赖仍为“无本目录语义前置”，但本文件明确要求的上游范围身份、载体和codec接口尚未交付。未修改只读plan.md、父任务8、路线图或覆盖表。核对HEAD为 `51530f223e4452339a82bbfebaed2962448d7efe`，最近提交为批次22的int8数组追加提升；未发现本工作区已有批次23源码改动。未领取或移除其他会话资源。

### 当前可用接口与缺口

已读根AGENTS.md、pkg/server/AGENTS.md、PLANS.md和docs/agents/testing-flow.md；目标types/expression/session/server/parser目录检索未见doc.go。按skills/rustcodegraph/SKILL.md先运行索引status（退出0，11796文件），再explore/query/node。图对pg模块文件筛选未覆盖，按规则使用原始rg复核缺口。

类型接口归属上游类型/组合载体所有者（pkg/types与pkg/parser/types）：`pkg/types/lib.rs:119–133`注册uuid/json/jsonb/array模块，没有范围模块；`pkg/parser/types/field_type.rs:121–134`仅有text OID、array element、record fields等PG身份，没有范围/subtype身份。`pkg/types/datum.rs::DatumValue`没有范围变体；既有数组另有专用载体，不能仅据DatumValue判断，本轮同时复核了全目标目录。对pkg/types、pkg/parser/types、pkg/expression、pkg/session/runtime、pkg/server的Rust源检索`int4range|numrange|pg_range|pgrange|3904|3906`，只有其他数值测试的子串命中，没有int4range/numrange生产代码、载体或codec。`PgRange`/`int4range`图查询也只有不相关的存储/优化器范围。

协议接口归属上游server范围codec所有者：`pkg/server/pg_result.rs::pg_type`（450起）消费text/record/array身份及既有标量code，没有int4range OID 3904/范围编码分支。现有`CatalogColumnType`也不含range。不能借用text OID标记字符串替代规范范围值及binary codec。

本批正式工厂/分析接线归属expression/session：`pkg/expression/builtin.rs::resolve_pg_signature`（2610起）保留array_append/cardinality/array_length/to_char/date_part/extract/date_trunc等实现，默认返回42883，无int4range。消费者仍为`pkg/session/runtime/query.rs`；当前缺口首先是上游类型接口，不能在本批绕过类型身份创建第二求值器。最近相关完整提交：类型/身份 `f5ec57fec7cc3805cb8e42ec3037f3da7f93ee40`，server结果/参数 `f71759b093c1dff7ad5d19d3f5783e9ca2b53bdf`，工厂/查询 `51530f223e4452339a82bbfebaed2962448d7efe`。这些是源码定位信息，不是本任务当前绿色证据。

### 已运行检查与验证边界

所有检查均为静态只读：`git rev-parse HEAD`、`git log -6 --format='%H %s'`、按上述路径的`git log -1`、`rustcodegraph status/explore/query/node/files`及`rg`源码/注册检索。`rg -n 'pg08_23' pkg/expression/lib.rs pkg/server/lib.rs pkg/server/pg_query_test.rs pkg/server/pg_client_integration_test.rs`退出1（无注册）；未新增测试或夹具，未运行--list或测试，不以零测试或编译错误作为行为红灯。`rg --files ... | rg '/doc.go$|pg.*range|range.*pg'`退出1（无目标文件）。`.agents/skills/tidb-verify-profile/SKILL.md`不存在（test退出1），未宣称使用该技能。

用Python仅检查环境变量是否存在（退出0，未输出值）：PG18_REFERENCE_DSN、PG_LIBPQ_LIBRARY、MYSQL8_REFERENCE_HOST、MYSQL8_REFERENCE_PORT、MYSQL8_REFERENCE_USER、MYSQL8_REFERENCE_PASSWORD均unset。因尚未具备上游接口且参考连接未配置，未连接服务，不能确认实际PG版本为18.6或MySQL版本；未产生客户端、双方言、负向自检或清理观测。

行为场景预期集仍以本文件验收要求为准，尚未进入case注册阶段；本轮注册0、运行0、通过0、失败0、跳过0，业务去重总数0，负向自检0。没有成功验收证据。WIP/Ready构建测试、cargo fmt、make lint均未运行：本轮只更新阻塞交接，未改生产/测试代码；没有以历史记录代替当前验收。

Cargo槽位：未领取，CARGO_TARGET_DIR：未设置。未调用Cargo或调用Cargo的脚本，未启动Server/Python/参考实例，未创建测试对象、监听端口或临时目录，因此无本任务锁/进程/对象需要清理，构建缓存保持原状。

### 决策与恢复条件

2026-10-30 决策：遵守本文件“必须先交付独立int4range/subtype身份、empty/无界/开闭载体及text/binary codec；缺失时只登记上游接口”的门槛，停止于里程碑一取证，不补建范围子系统，不制造不可编译测试，不扩大到其他函数。接口拥有者是上述上游类型/组合载体及server codec子系统，当前文件未指定个人会话拥有者。

恢复条件：上游提供可被FieldType/正式工厂/会话结果消费的int4range及int4 subtype身份、规范empty/无界/开闭值载体和text/binary参数及结果codec，并给出当前注册测试；配置真实PG18.6及MySQL参考连接。随后重新核对最新HEAD及资源互斥、定义唯一case ID/预期计数、注册三项测试和客户端夹具、动态领Cargo槽位，再按原命令完成红绿、真实双方言和Ready检查。本批仍未实现，保留编号文件；只有编号状态和取证更新，按授权不提交。


## 2026-10-30 继续排障：参考环境可自行恢复

用户要求继续穷尽安全替代路径，覆盖技能的遇阻默认停止规则。最新HEAD仍为51530f223e4452339a82bbfebaed2962448d7efe，源码精确检索int4range/numrange/pg_range/pgrange仍无命中（rg退出1）。已读取编号11、其客户端夹具及target/pg08-11-evidence/reference-lifecycle-71216.json和71591-56494/build.jsonl，定位可用PG18.6安装与MySQL8.0.46二进制；因此撤销“未继承参考环境变量”作为外部阻塞的判断。正在仅使用自己的临时目录启动、采证和停止独立参考实例。

参考独立探测先登记唯一ID：R23-empty、R23-closed-equal、R23-open-closed、R23-lower-unbounded、R23-upper-unbounded、R23-both-unbounded、R23-reversed、R23-null-flags、R23-bad-flags、R23-overflow、R23-zero，共11项。它们只证明参考行为，不计为AsterSQL通过；正式三项测试尚未注册。

需要解决的范围决策：编号文件禁止本批创建range子系统，而缺口跨FieldType、types值载体、参数和结果codec，无法由已有数组/text/record接口安全代替。已向用户提出具体选择：授权仅int4range所需的最小前置类型/载体/codec，或保持原范围等待上游；等待该答案期间继续参考生命周期取证，不改受此决策影响的生产代码。


### 继续排障的实际结果

本轮已自行启动独立PG18.6与MySQL8.0.46参考实例，子进程继承本任务设置的连接环境，未依赖外部已有服务。执行命令为内联`python3 - <<'PY'`参考探测驱动（退出0），内部调用PG安装目录的`initdb -D <owned>/pg -U root -A trust --encoding=UTF8 --locale=C`、`postgres -D <owned>/pg -p <dynamic> -h 127.0.0.1 -k <owned>`，及MySQL的`mysqld --no-defaults --initialize-insecure --datadir=<owned>/mysql`和`mysqld --no-defaults --datadir=<owned>/mysql --port=<dynamic> --bind-address=127.0.0.1 --socket=<owned>/mysql.sock --pid-file=<owned>/mysql.pid --mysqlx=OFF`。使用既有Libpq/Mysql原生客户端绑定，libpq指定本地PG18.6安装的libpq.5.dylib；未写入凭证或连接串证据。

证据`target/pg08-23-evidence/reference-probe.json`保存HEAD、真实二进制版本、PQlibVersion、version()/server_version_num（180006）、PG编码/locale/TimeZone及MySQL版本/sql_mode。已登记参考场景预期11、实际11、通过11、失败0、跳过0，去重11。6项正常范围同时采isempty/lower_inf/upper_inf/lower_inc/upper_inc和range_send十六进制；4项错误验证22000/22000/42601/22003；零行描述仍为OID3904/typlen-1/typmod-1。所有这些均仅为参考观测，AsterSQL业务验证仍0；未声称完成，也未运行差分负向自检。

`target/pg08-23-evidence/reference-lifecycle.json`记录本任务自己的两个PID、动态端口与临时目录。两个进程均已terminate并wait，端口关闭，自己的临时目录删除（全部布尔为true）。驱动finally同时覆盖正常、异常和SIGTERM中断，未碰用户服务。源码替代路径补查`git ls-files '*range*.rs' '*range*.rs.md'`（退出0）仅有DDL删除范围、存储key range和优化器ranger；`pkg/parser/mysql/type.rs`的TypePg定义只有已交付的时态/UUID/JSON族，没有范围身份。不存在可直接复用以达到本任务语义的range接口。

剩余阻塞只涉及范围授权：需明确允许本批补交仅int4range必需的FieldType身份、规范范围载体与参数/结果codec，才能越过编号文件明确的“不在此创建range子系统”限制。参考环境不再列入恢复条件。该范围决定尚待用户答复；在答案前不改受其影响的生产文件。无Cargo调用，槽位和CARGO_TARGET_DIR仍不适用；本轮没有代码交付，不运行Ready或make lint。只有编号文件修改，不提交。最终`git diff --check`退出0。


## 最新授权后的最小接口设计（2026-10-30）

用户明确授权解除本批缺失range前置接口的范围门槛，仅实现int4range，不开放其他范围类型/运算。生产路径设计：pkg/parser/mysql/type.rs分配独立TypePgInt4Range内部code（0xdf，OID3904/subtype23）；既有FieldType.tp、序列化和NativeType.code直接承载身份，不增加重复字段。pkg/parser/parser_actions/expression.rs和pkg/parser/types/field_type.rs仅接既有PG类型名动作及Restore。pkg/types/pg_int4range.rs定义不可变规范PgInt4Range（Empty或Bounds{lower:Option<i32>,upper:Option<i32>}，非空有限边界统一[lower,upper)），提供checked构造/text输入输出/binary输入输出；注册于pkg/types/lib.rs，测试独立文件。复用现有文本Datum边界（同pg_array/jsonb）但按独立类型和规范载体验证，不把普通text当range，不按值猜OID。

pkg/expression/builtin.rs接两/三参数签名、PgInt4Range正式工厂、类型与EvalString分支；所有构造委托上述真实载体。pkg/session/runtime/query.rs及relational_value.rs复用已定型函数/行求值、分析及错误来源；planning.rs仅补typed参数OID映射所需分支。server pg_result.rs按独立code报告OID3904并委托text/binary codec；pg_extended.rs允许OID3904并解码至既有BinaryParam，control.rs仅在prepared类型桥中补映射。每项均是本构造函数、typed NULL/零行描述或绑定/结果验收不可缺少的局部接线，不增加range算子或DDL存储系统。

测试注册：expression/lib.rs挂pg08_23_test::pg08_23_formal，server既有模块挂pg_query_test::pg08_23_sql和pg_client_integration_test::pg08_23_clients，夹具pg08_23_libpq.py。正式工厂预期10正常/错误场景；SQL/客户端预期用例集合在各测试/CASE_IDS固定后同步记录，先注册再list和红灯。types载体单测另挂pg_int4range_test。后续逐项记录真实计数。

客户端唯一case ID固定为：C-Q-empty, C-Q-equal, C-Q-bounds, C-Q-lower, C-Q-upper, C-Q-both, C-Q-null, C-Q-zero, C-Q-zero-three, C-Q-nested, C-Q-row, C-Q-row-zero, C-E-reverse, C-recover-reverse, C-txn-reverse, C-E-null-flags, C-recover-null-flags, C-txn-null-flags, C-E-bad-flags, C-recover-bad-flags, C-txn-bad-flags, C-E-overflow, C-recover-overflow, C-txn-overflow, C-E-signature, C-recover-signature, C-txn-signature, C-bind-0-0-0-0-0, C-bind-0-0-0-0-1, C-bind-0-0-0-0-2, C-bind-0-0-0-1-0, C-bind-0-0-0-1-1, C-bind-0-0-0-1-2, C-bind-0-0-1-0-0, C-bind-0-0-1-0-1, C-bind-0-0-1-0-2, C-bind-0-0-1-1-0, C-bind-0-0-1-1-1, C-bind-0-0-1-1-2, C-bind-0-1-0-0-0, C-bind-0-1-0-0-1, C-bind-0-1-0-0-2, C-bind-0-1-0-1-0, C-bind-0-1-0-1-1, C-bind-0-1-0-1-2, C-bind-0-1-1-0-0, C-bind-0-1-1-0-1, C-bind-0-1-1-0-2, C-bind-0-1-1-1-0, C-bind-0-1-1-1-1, C-bind-0-1-1-1-2, C-bind-1-0-0-0-0, C-bind-1-0-0-0-1, C-bind-1-0-0-0-2, C-bind-1-0-0-1-0, C-bind-1-0-0-1-1, C-bind-1-0-0-1-2, C-bind-1-0-1-0-0, C-bind-1-0-1-0-1, C-bind-1-0-1-0-2, C-bind-1-0-1-1-0, C-bind-1-0-1-1-1, C-bind-1-0-1-1-2, C-bind-1-1-0-0-0, C-bind-1-1-0-0-1, C-bind-1-1-0-0-2, C-bind-1-1-0-1-0, C-bind-1-1-0-1-1, C-bind-1-1-0-1-2, C-bind-1-1-1-0-0, C-bind-1-1-1-0-1, C-bind-1-1-1-0-2, C-bind-1-1-1-1-0, C-bind-1-1-1-1-1, C-bind-1-1-1-1-2, M-Q-int4range, M-prepare-int4range, M-Q-numrange, M-prepare-numrange, M-Q-isempty, M-prepare-isempty, M-Q-lower, M-prepare-lower, M-Q-upper, M-prepare-upper, M-Q-health, M-prepared-health, I-before, I-middle, I-after, C-bind-range-0-0, C-bind-range-0-1, C-bind-range-1-0, C-bind-range-1-1。预期去重总数94；SQL固定46场景（10直接+32绑定+4错误恢复）；formal固定10场景。尚未运行。


## TDD当前证据

此前阻塞由最新授权解除，当前已进入实施（状态枚举无“进行中”，使用未开始直到取得交付结果）。red阶段动态领取slot1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。三条指定命令的--exact --list各退出0且有效1测试；三条--exact --nocapture各退出101且实际运行1项。formal工厂抛No database selected（未注册函数回退）；SQL首条构造返回42883；真实客户端在C-Q-empty看到Aster42883而参考成功，已保存双方观察。这是运行期行为失败，不是编译或环境红灯。确切命令与输出在target/pg08-23-evidence/commands.jsonl、red-0.log至red-5.log。red-lifecycle.json确认拥有的PG/MySQL进程wait、端口关闭、临时目录删除、slot释放均为true。

按设计新增规范载体及最小接线，生产改动都对应构造、typed身份或参数/结果codec，不包含其他范围类型/算子。green1阶段重新动态领取slot1，先cargo fmt --all退出0，正在原命令复验。尚未声明通过。

最小接线补充：builtin_threadsafe_generated.rs登记builtinPgInt4RangeSig为递归共享（规范化无会话依赖）；否则正式工厂拒绝未知签名。这由green1正式测试当前运行期失败证明必要。新增resolve_pg_typed_signature作为完整FieldType分派，原array解析接口保持原职责。范围cast资格在query分析时与既有UUID/JSON资格链相接，禁止typed NULL/零行掩盖不支持的转换。

当前嵌套COALESCE测试运行期失败表明既有fallback标量路径不认识新构造函数。仅为已分析为range的COALESCE补充递归到现有session正式工厂消费者的局部接线，并在分析阶段拒绝不兼容混型；不增第二求值器、不修改通用COALESCE。构造错误使用既有PgFunctionError诊断桥，保留PG18.6 bad flags的H字段。

green2正式工厂1测试/10场景与SQL1测试/46场景当前通过。客户端PG全部构造、错误/恢复、事务、绑定、range输入codec均通过后在既定M-Q-numrange失败：Aster1105/HY000，MySQL参考1305/42000。仅将numrange/isempty加入已有MySQL方言不可见名称集合（不实现函数/工厂），使本任务明确要求的这两个拒绝行为走已有1305诊断；int4range原有本轮名称拒绝已通过。客户端当次实际81通过，未宣称94全通过。

范围text输入使用PgInt4Range::from_text_with委托既有pg_cast_int4，保留PG18的十六/二/八进制与下划线输入，避免Rust十进制parse替代真实subtype语义。原四个range codec case中text输入改用[0x1,0b100]，与binary输入相同规范结果[1,5)；CASE_IDS与94计数不变，后续原命令复验。结果边界仅消费已规范十进制text，不猜身份。

green3中PG构造/错误/绑定及完整int4 subtype语法通过，三个MySQL名称拒绝Query/Prepare也通过。M-Q-lower遇既有元数据差异（Aster flags0/charset255/length1024/decimals0；MySQL8.0.46 flags128/charset46/length12/decimals31），值均为abc。未改lower/upper生产代码；客户端保留全部严格断言，改为聚合该组失败后继续所有94场景，不以提前失败遮蔽后续。新增两个SQL零行负例验证不可支持转换42846及range/text混型COALESCE42804，SQL预期变为48；客户端真实行源投影改为三参签名，二参已由嵌套覆盖，94唯一ID保持不变。

codec补充回归（pg_int4range_test::pg_int4range_codec）先注册并运行1项真实失败：输入[2147483648,2缺少闭符，错误返回22003，应先报container22P02。对照本地真实18.6源码rangetypes.c::range_in，先解析完整container后才调用subtype；据此改为先收集两端点、验证闭符，再按顺序委托int4输入。相邻pg08_22_formal当前1测试/14场景通过。现有注册表回归builtin_registry_matches_complete_go_function_table运行1项失败（329 vs323）：旧断言仅计date_trunc，漏计已有扩展及本次int4range。作为本次新增正式注册的必要测试维护，显式列出七个有界PG扩展，仍独立保留Go310和原运算符别名数，不改生产注册表来迎合旧计数。全部命令/退出码在codec-red日志与commands.jsonl。


## 2026-10-30 交付状态与回归边界（取代早期阻塞记录）

本批已实现所选int4range构造行为和最新授权的最小前置类型/codec，最终状态待最终命令完成后确认为“已完成，待回归”。已解除环境与上游接口阻塞，不需要额外用户权限或参考服务。此前未开始/阻塞记录仅为执行历史，不是当前接口现状。整体plan.md/父任务8保持只读。

实际文件分组：parser/mysql/type.rs的独立0xdf身份、parser_actions/expression.rs类型名动作、parser/types/field_type.rs Restore；types/pg_int4range.rs真实规范范围及codec、独立pg_int4range_test.rs、types/lib.rs挂载；expression/builtin.rs签名及正式工厂/求值、builtin_threadsafe_generated.rs不可变共享策略、pg08_23_test.rs与lib.rs注册、builtin_registry_aster_unit_test.rs扩展计数维护；session/runtime/query.rs分析/正式求值/范围COALESCE、relational_value.rs类型/cast/诊断、planning.rs参数OID推断、control.rs既有prepared上下文接线；server/pg_result.rs元数据与结果codec、pg_extended.rs参数codec、pg_query_test.rs协议回归、pg_client_integration_test.rs生命周期测试、testdata/pg08_23_libpq.py真实客户端。每项只对应本构造函数的类型、输入、结果或必要测试消费，不提供numrange/isempty的PG实现或完整范围子系统。保留全部既有版权，无依赖、Go、Bazel或Cargo manifest变更。

Ready阶段指定三项各--list有效1/退出0；formal执行1/10场景退出0，SQL执行1/48场景退出0，client执行1退出101。完整94场景已实际执行：通过86、失败8、跳过0、去重94；PG成功8、NULL1、零行3、绑定52、错误5、恢复5、事务5，共79全部通过；MySQL三个名称Query/Prepare拒绝6与真实健康表prepared1通过。独立值/OID/SQLSTATE差分器自检3通过，不计业务数。M与A/B交错的真实健康表值与读取metadata匹配，附加SELECT1元数据使I-before/I-middle/I-after标为失败，没有冒充交错完全验收通过。

8项剩余失败：I-before/I-middle/I-after、M-Q-health的SELECT1通用列定义差异，以及M-Q-lower/M-prepare-lower/M-Q-upper/M-prepare-upper的flags/charset/length/decimals差异。lower/upper值均正确abc/ABC，Query/Prepare/Execute原始metadata、值、警告和错误完整保留，不删断言、不加白名单。lower示例Aster Query元数据[v,253,0,255,1024,0]，参考[v,253,128,46,12,31]；Aster Prepare又给[v,253,0,46,5,31]。这些通用MySQL返回类型代码未由本批修改；早期red阶段本批尚未改生产代码的I-before观察也存在SELECT1差分。所属拥有者为session的MySQL表达式描述、server原生结果元数据及expression lower/upper工厂；恢复条件是由该接口拥有者修复当前直接SQL/原生预处理的通用列定义，然后原命令复验94/94。本批不递归改写MySQL返回元数据系统。任务因这些实际无关差分保留，不删除，不宣称双方言完整通过。

适用检查：cargo fmt --all退出0；make lint退出0；git diff --check退出0。Parser变更额外运行make parser、make parser_yacc、make parser_fmt、make parser_unit_test均最终退出0，后者14个成功包行；启用failpoint后执行并最终disable退出0、工作区Go文件恢复无差异。早期启用扫描与Cargo重命名造成lstat瞬时失败，串行重试已解决；初次parser_unit_test只因新生成failpoint绑定文件需格式化退出2，格式化自己的生成绑定后原目标重试通过。未用这些环境/格式失败代替行为红灯。相邻pg08_22_formal实际1/14场景退出0，注册表回归1项退出0，codec独立回归1项退出0（包括已红绿的container优先级）。确切命令/退出码见target/pg08-23-evidence/commands.jsonl与parser-commands.jsonl，各输出log保留。make lint和Go parser证据覆盖未再改变的Go检查面；最终Rust载体微调后正在原样复验三条指定命令及cargo fmt。

风险与未验证：int4range规范化、empty/无界、边界溢出、独立OID/typlen/typmod、零行/typed NULL、text/binary参数和结果、statement/portal Describe、Execute不重复RowDescription、错误Sync及事务ROLLBACK恢复已作聚焦验证；未做性能基准、TiKV实存储、范围列DDL、通用范围运算、其他范围类型或全包广扫。新类型不增加下推签名。MySQL通用metadata兼容性仍有上述缺口，所以完整Ready验收未全绿。没有修改Go/import/dependency/Bazel，不触发bazel_prepare；未跑禁止默认的bazel_lint_changed。


最终复验完成：final阶段cargo fmt --all退出0，三项--list各1项/退出0，formal1项/10场景退出0，SQL1项/48场景退出0，client1项退出101，原94场景仍86通过/8同一MySQL元数据失败/0跳过。当前行为与限制保持上述交付结论。最终原始双方观测目录为 `target/pg08-23-evidence/41060-55239`；delivery.json汇总全部确切命令、退出码、文件、fixture hash、计数和未验证项。最终无额外生产改动，Go parser/make lint证据仍适用，git diff --check复验通过。

所有Cargo阶段实际均动态领取slot1，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；每次阶段结束均wait所有子进程、删除自己的owner与slot锁，保留缓存。最终只读复核所有reference lifecycle的进程结束、端口关闭、临时目录删除及slot释放；Rust同名cleanup.json确认本任务数据库删除和两个监听关闭。客户端summary记录自己的参考temp表/库删除、locale/TimeZone恢复、连接关闭。参考构建/客户端版本、二进制及fixture hash、HEAD/source diff hash、tzdata2026c、编码/locale/sql_mode保存在build与reference/mysql-reference.jsonl。只保留脱敏证据与缓存；交付前删除自己的临时验证脚本。

状态确认为已完成，待回归：仅剩无关MySQL通用元数据差分，因此保留本编号文件。按最新用户授权的交付规则，提交本批生产、测试及编号取证变更，排除无关未跟踪计划；不删除编号，不声称完整94/94通过，不更改任何plan.md或父计划8。

交付前仅整理codec单测中非法空白端点用例的位置，未改生产逻辑；final-codec阶段cargo fmt --all与该exact单测均退出0。再次git diff --check及暂存区检查退出0，Ready与final的生产行为证据保持有效。

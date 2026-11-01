# 任务 28：多SRF协调与非法位置门禁

批次：【批次 28】；调度依赖：批次27（共享源码/注册与验证资源串行）；行为依赖：27。

状态：已完成

目的：同层多投影SRF同步取第n项、短者补NULL、行数取最长；CASE/COALESCE内SRF分析拒绝。尚未承诺的嵌套/多数组/复杂输出保持显式拒绝。

来源任务：大计划9；路线图第4章 `SRF多投影扩展边界`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：任务27投影展开，2号表达式位置合法性。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

聚合复用 `AggFuncDesc` / `TypedHashAgg` / `BuildTypedPhysicalPlan`，窗口复用 `PhysicalWindowPlan` 与缓冲/流水线入口，SRF沿现有物理行源/投影接线；只使用本任务相关链。协议复用 `PgService`、`ConcreteSessionDriver` 与既有真实会话；目录特例不能充当普通SQL执行。新生产类型/文件若非必要不新增，必须新增时先在本活动计划记录职责及实际注册点。

## 文件与修改依据


- 候选修改：`pkg/executor/physical_plan_runtime.rs`。
- 候选修改：`pkg/executor/builder.rs`。
- 候选修改：`pkg/session/runtime/query.rs`。
- 候选修改：`pkg/session/runtime/planning.rs`。
- 候选修改：`pkg/parser/ast/lib.rs`。
- 必要语法局部接线候选：`pkg/parser/parser_runtime.rs`、`pkg/parser/ast/lib.rs`；复用上游2号已提供载体，不重建解析器。
- 独立测试：`pkg/executor/physical_plan_runtime_test.rs`、`pkg/server/pg_query_test.rs`；描述断言按需扩展 `pkg/server/pg_types_test.rs`。

上述均为已存在候选文件（明确标为待新增者除外），只修改实现本项所需者。每项生产修改须在本文件记录“路径/符号→本任务行为或不可缺少的局部接线”。本计划是PG功能增量，不是Go逐提交映射；不得伪造Go SHA或删减既有Go回归语义。

## 行为与验收夹具


同层多投影SRF同步取第n项、短者补NULL、行数取最长；CASE/COALESCE内SRF分析拒绝。尚未承诺的嵌套/多数组/复杂输出保持显式拒绝。

代表SQL（其他边界使用同型最小夹具）：

    SELECT generate_series(1::int4,2::int4) AS a,generate_series(10::int4,12::int4) AS b

目标(1,10)/(2,11)/(NULL,12)，不能笛卡尔乘积或最短截断；两者均空零行、单者NULL补齐。原文已给出本扩展目标，此项独立取证，不扩展时间/numeric序列。

每项补正常、空输入、全NULL、边界、非法输入、恢复和清理。失败后在同连接执行 `SELECT 1::int4`，扩展协议先Sync；事务失败则ROLLBACK后再探测，记录ReadyForQuery状态。结果列显式别名，检查实际行数及值/NULL，不能仅断言没有错误。普通Q和扩展协议固定返回身份；不因首行值或零行而改变OID/typmod。不得由测试存储通过推断RealTiKV或性能。

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

## 测试计划


在 `pkg/executor/physical_plan_runtime_test.rs` 待新增 `pg09_28_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_28_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_28 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_28 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次28执行；行为依赖为27，调度依赖为批次27。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_28 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_28 -- --nocapture
    make lint
    git diff --check

新增/修改解析器时还须按根矩阵运行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 并审查生成差异；不修改解析器不无故执行这些命令。Bazel准备按根完整触发清单判定；只改Rust且无Bazel/Go/依赖触发时不运行 `make bazel_prepare`。禁止默认运行 `make bazel_lint_changed`。如需Go测试，先依testing-flow判定failpoint并在退出禁用；Go既有回归失败记录后按用户规则不阻塞本任务，新增回归不得忽略。

检查当前改动实际影响的原有相邻测试并记录精确命令；不运行无依据全包/昂贵广扫。涉及真实存储才依 `docs/rust/rust-postgresql-tikv-runbook.md` 与testing-flow启动专属后台playground、验证并清理。本计划不授权在拆分阶段运行上述命令。

风险：共享描述符/执行状态改动可能影响MySQL；排序/帧/行扩展可能增加内存或错误地改变行数；先保持单阶段正确并验证资源释放，不据小夹具给出吞吐承诺。恢复只撤销自己明确有问题的变更，不删除其他任务/用户文件，不移除未知锁或服务。

## 完成与交接


只使用状态：未开始、已完成、已阻塞、已完成，待回归。完整行为和所需适用验证通过后标记已完成；有实际代码/测试/配置/文档变更使用 `$git-commit` 技能只提交本任务。只有编号文件状态或删除时无需单独提交。最终报告路径/符号、生产修改依据、验证profile、实际槽位/绝对CARGO_TARGET_DIR、完整命令/退出码/有效计数、正确性/兼容性/性能风险、未验证项及完整提交SHA。

验证因无关环境缺失无法运行，可记录已完成，待回归并保留本文件；不能把仍缺失的目标行为或依赖接口标为完成。阻塞只写本文件及已做的检查，不扩大范围。完成证据已交接且提交妥当后按执行契约删除本编号文件，`plan.md`只读，父大计划和覆盖表保留。删除文件本身不是完成证据。

## Progress


- [x] 确认行为依赖与实际入口。
- [x] 记录运行期失败或已有正确行为的当前证据。
- [x] 完成最小实现及同夹具通过验证。
- [x] Ready检查、diff自审与提交交接（完整SHA由最终回复报告）。

## Surprises & Discoveries


当前仅有静态规划，没有运行结果。实施中在这里记录实际接口差异、缺失依赖和对应证据。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


本任务同层多SRF协调与位置门禁已实现并取得红绿灯、Ready验证；精确证据见下方。完整SHA在最终交接报告，提交后按契约删除本文件。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-11-01 实施记录

HEAD为4d3b5a41d9d33a2377d6c9d2c7de02c6e7ac5ee9，批次27已提交且单投影SRF入口、固定描述和独立pg09_27测试存在；2号提供Function/Case及表达式访问器，局部合法性检查可复用，不缺语法载体。目标包无doc.go，server AGENTS只增加HTTP规则。本次仅在query.rs分析直接投影SRF的位置合法性，并在source.rs把已有逐外行单SRF展开改为同层最长补NULL；复用现有函数行源与类型绑定，无新增公共生产类型或解析器改动。独立executor/server pg09_28测试先加入，覆盖1025边界、混合重载、空/NULL/数组、非法位置Parse期拒绝、Q与扩展四格式和重绑清理。

生产修改映射：query.rs::validate_pg_integer_select_inputs → 本任务CASE/COALESCE和嵌套SRF分析门禁；source.rs::execute_insert_select_query_body_with_outer → 本任务同层同步取第n项、最长行数与短者补NULL。query.rs::pg_projection_srf仅更新说明，同层多SRF使用同一识别器。拟同步对应既有.rs.md，避免文档仍宣称多SRF由后续负责。

RustCodeGraph status/explore/node已运行；复杂泛名explore无精确门禁结果，用rg复核，既有直接SRF未见位置门禁。未触发Go/Bazel/依赖/解析器修改，不运行bazel_prepare/parser目标。verify-profile技能仍缺失，按根Ready要求验证。

红灯阶段bash /tmp/pg0928-cargo.sh动态取得槽位1、PID13350、CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。cargo fmt --all退出0；executor/server pg09_28各1项实际运行并退出101，均因多个generate_series未展开而unsupported scalar function，非编译失败。日志/tmp/pg0928-{executor,server}-red.log。阶段结束包装器trap已释放锁。PG18.6独立参考bash /tmp/pg0928-reference.sh退出0，确认最长补NULL、全空、NULL元素及边界、CASE/COALESCE 0A000；参考进程与数据已清理，日志/tmp/pg0928-reference.log。

首轮实现编译发现streaming快路径仍使用旧变量projection_source，已改为projection_sources.is_empty；两命令编译退出101不计行为证据。第二轮executor实际1失败，是测试把会话结果集NULL文本误记为NULL，实际为<nil>（left第三行b为<nil>、行数1029已通过）；修正仅测试解码，协议测试继续用真实NULL长度字段断言。新增GROUP/ORDER中的CASE/COALESCE门禁夹具和事务ROLLBACK恢复，以及扩展协议两参数四格式正常/空/全NULL/恢复，共20绑定。make lint当前退出0。

第二轮server pg09_28退出0，有效2项（SQL、同Server双PG/MySQL隔离）通过。原始错误字段S/V=ERROR、C=0A000、CASE/COALESCE对应M已采集；嵌套调用0A000，本任务保留的多数组门禁由原数组分析以0A000拒绝；零步长Execute/Q运行期22023。D/H/P字段当前未发出，不能声称与PG18.6诊断字段完全等价。事务错误ReadyForQuery=E，ROLLBACK后I及SELECT1恢复通过。20次扩展绑定含text/binary参数×结果、正常→单NULL→全空→全NULL→恢复，statement/portal描述始终a=OID23/4/-1、b=OID20/8/-1，Execute未重发描述。

最终Ready使用bash /tmp/pg0928-cargo.sh bash /tmp/pg0928-ready.sh动态取得槽位1，持有PID77387，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；验证进行中，结束后包装器trap释放本人的owner与锁，保留缓存。所有Cargo默认features且--locked，未引入依赖。

局部生产类型说明：query.rs::validate_pg_integer_select_inputs内SetCalls仅实现现有ExprNodeVisitor，计数本表达式的SRF并记录CASE/COALESCE上下文；它由同函数在绑定前直接调用，无新增公共载体、独立注册或执行子系统。source.rs保持非SRF路径不克隆FieldList，避免扩大MySQL开销。

Ready executor pg09_28已退出0，有效1项，1029行跨1024边界、短序列补NULL以及下一外行重启均逐行核对；协议NULL断言由server测试独立验证。

Ready同过滤器executor1/server2通过；相邻executor pg09_2共14通过、server pg_query_test::pg09_2共10通过。session的MySQL兼容性及array分析定向测试仍在运行。源码审查发现其他并行任务在query.rs及query.rs.md加入压缩函数接线，保持原样；提交按hunk隔离，仅选SRF说明/SetCalls两个Rust hunk和SRF文档hunk，不提交其他工作。


## 最终Ready证据与交接

bash /tmp/pg0928-cargo.sh bash /tmp/pg0928-ready.sh退出0；实际动态领取槽位1，PID77387，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。阶段结束已移除本人的owner及slot-1.lock，保留缓存；参考临时数据与TCP listener已清理。

| 完整验证命令 | 退出码 / 有效计数 |
| --- | --- |
| cargo fmt --all | 0 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_28 -- --nocapture | 0 / 1通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_28 -- --nocapture | 0 / 2通过，含20次扩展绑定 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_2 -- --nocapture | 0 / 14通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg09_2 -- --nocapture | 0 / 10通过 |
| env -u LDFLAGS cargo test --locked -p astersql-session --lib mysql_relational_query_compat_test -- --nocapture | 0 / 6通过 |
| env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_array_test -- --nocapture | 0 / 45通过 |
| make lint | 0，日志/tmp/pg0928-lint.log |
| git diff --check | 0 |
| git diff --quiet -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/plan.md | 0 |
| bash /tmp/pg0928-reference.sh | 0 / 独立PG18.6同型夹具观测 |

上述过滤器重叠，测试数不可相加为唯一数量；初始同过滤器红灯各1项实际失败（退出101），中间编译错误及会话NULL文本测试期待纠正均已单独记录，不作为行为通过证据。最终日志/tmp/pg0928-ready-{fmt,executor,server,executor-neighbors,server-neighbors,mysql-neighbors,array-neighbors,diff}.log。参考PG18.6 aarch64-apple-darwin25.1.0、Apple clang16、64位，SQL_ASCII、Asia/Shanghai、psql18.6；不是任务30完整客户端差分。

Profile：WIP用于测试驱动红绿灯，Ready用于交付；verify-profile技能缺失，按根AGENTS和testing-flow完成适用检查，不声称该技能运行。无本任务Go/Bazel/依赖/解析器变更，故不触发bazel_prepare、parser Make目标或Go failpoints；不涉及真实存储，不运行RealTiKV。

本任务修改query.rs的SRF识别说明与局部SetCalls分析门禁、source.rs的同层展开（含快路径排除），同步两份既有.rs.md，追加physical_plan_runtime_test.rs和pg_query_test.rs独立回归及本记录，共7文件。保留PingCAP版权，两生产Rust文件已有AsterSQL版权。query.rs及其说明中的并行压缩函数修改通过hunk排除；不提交其他计划/解析器/planner等既有工作。

正确性限于已承诺直接投影整数序列/int4数组、最长补NULL与位置拒绝。D/H/P字段当前未发出，不声称原始诊断全部等价。兼容性证据为定向PG/MySQL回归；未做清洁checkout、全workspace、真实TiKV、吞吐/内存压力或任务29的portal暂停/取消资源验收。仍沿用既有物化关系入口，内存随结果量增长，无性能承诺。未新增上游依赖或本地覆盖。

下一步推荐任务29：SRF暂停恢复与资源释放。plan.md完全只读；完成状态与证据随本任务提交，完整SHA由最终回复交接；提交后删除本编号文件，不改父计划或覆盖表。

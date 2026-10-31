# 任务 23：INT4序列FROM行源

批次：【批次 23】；调度依赖：批次22（共享源码/注册与验证资源串行）；行为依赖：无计划内行为依赖。

状态：已完成

目的：generate_series(int4,int4[,int4])在FROM返回零至多行；正反向、反方向零行、相等端点、默认步长、任一NULL零行和零步长错误均覆盖。

来源任务：大计划9；路线图第4章 `SRF-DIRECTION/ZERO/NULL`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：2号函数行源AST，7号SRF分类，3/6号int4身份。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

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


generate_series(int4,int4[,int4])在FROM返回零至多行；正反向、反方向零行、相等端点、默认步长、任一NULL零行和零步长错误均覆盖。

代表SQL（其他边界使用同型最小夹具）：

    SELECT g.v FROM generate_series(1::int4,3::int4,1::int4) AS g(v) ORDER BY g.v

期望1/2/3及固定23/4/-1；边界到达int4上下限后正常终止不回绕。普通SQL执行器接线，不能走目录特例或标量工厂。

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


在 `pkg/executor/physical_plan_runtime_test.rs` 待新增 `pg09_23_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_23_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_23 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_23 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次23执行；行为依赖为无计划内行为依赖，调度依赖为批次22。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_23 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_23 -- --nocapture
    make lint
    git diff --check

新增/修改解析器时还须按根矩阵运行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 并审查生成差异；不修改解析器不无故执行这些命令。Bazel准备按根完整触发清单判定；只改Rust且无Bazel/Go/依赖触发时不运行 `make bazel_prepare`。禁止默认运行 `make bazel_lint_changed`。如需Go测试，先依testing-flow判定failpoint并在退出禁用；Go既有回归失败记录后按用户规则不阻塞本任务，新增回归不得忽略。

检查当前改动实际影响的原有相邻测试并记录精确命令；不运行无依据全包/昂贵广扫。涉及真实存储才依 `docs/rust/rust-postgresql-tikv-runbook.md` 与testing-flow启动专属后台playground、验证并清理。本计划不授权在拆分阶段运行上述命令。

风险：共享描述符/执行状态改动可能影响MySQL；排序/帧/行扩展可能增加内存或错误地改变行数；先保持单阶段正确并验证资源释放，不据小夹具给出吞吐承诺。恢复只撤销自己明确有问题的变更，不删除其他任务/用户文件，不移除未知锁或服务。

## 完成与交接


只使用状态：未开始、已完成、已阻塞、已完成，待回归。完整行为和所需适用验证通过后标记已完成；有实际代码/测试/配置/文档变更使用 `$git-commit` 技能只提交本任务。只有编号文件状态或删除时无需单独提交。最终报告路径/符号、生产修改依据、验证profile、实际槽位/绝对CARGO_TARGET_DIR、完整命令/退出码/有效计数、正确性/兼容性/性能风险、未验证项及完整提交SHA。

验证因无关环境缺失无法运行，可记录已完成，待回归并保留本文件；不能把仍缺失的目标行为或依赖接口标为完成。阻塞只写本文件及已做的检查，不扩大范围。完成证据已交接且提交妥当后按执行契约删除本编号文件，`plan.md`只读，父大计划和覆盖表保留。删除文件本身不是完成证据。

## Progress


- [x] 确认行为依赖与实际入口；获得有界补齐接口授权。
- [x] 记录运行期失败：executor会话及server TCP各1/1失败。
- [x] 完成最小实现及同夹具通过验证：executor2/2、server2/2、parser1/1。
- [x] Ready适用检查与diff自审通过；按git-commit技能交付，完整SHA在最终回复报告。

## Surprises & Discoveries


已有运行期红灯及最终通过证据。缺失的函数FROM接口按用户2026-11-01有界授权补齐；语义载体、目录所有权及验证准备的实际发现见后文记录。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


已实现并验证INT4 generate_series普通FROM行源。真实执行器跨Chunk/EOF/关闭重开、普通SQL及TCP四格式参数重绑/固定描述/NULL/边界/零步长/Sync与事务恢复均通过；最终证据与限制见末节，提交完整SHA由最终交接报告。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-11-01 前置接口核对与阻塞证据


本轮仅修改本编号任务文件，未修改只读plan.md、生产代码或测试，未删除其他会话变更。当前HEAD为8039f0acef400bf8941f63c95909b223395369cf。批次22的该提交及其已提交编号记录确认已完成：Rust客户端1/1、589/589唯一场景、Ready检查及清理证据均存在；这些是前序交接，未冒充本轮重跑。

任务23实际外部前置尚未满足，停止于第一里程碑之前：

- 2号函数行源AST缺失：pkg/parser/ast/lib.rs:553的TableSource只有Source: TableName、QuerySource: Option<NodeRef>、别名及Lateral等字段，无函数调用载体；ResultSetNode只有TableSource/Join。pkg/parser/grammar/main.astergram:2999–3002的TableFactor仅接受TableName、SubSelect、LATERAL SubSelect及括号TableRefs，无函数调用FROM语法。现有parser_contract_cases_test.rs::pg_function_23_function_source_spans测试的是SELECT标量函数的原始位置，并非FROM函数行源。
- 7号SRF分类接口缺失：pkg/expression/builtin.rs::PgFunctionCandidate只有arguments/result，仓库Rust源码搜索未发现SetReturning、set_returning、returns_set或普通generate_series实现。generate_series仅见pg_catalog_query_test的目录拒绝夹具；pg_catalog.rs的投影SRF注释属于目录特例，按本任务约定不能替代普通执行接口。尚未找到可复用的SRF分类/行源消费者。
- 3/6号int4已有pg_query_test.rs::pg_core_int4_wire_semantics等身份测试入口，本轮仅静态定位，未运行验证；不能用已有int4入口替代缺失的前两项前置。

不在本任务中新建函数行源AST、重建解析器或扩展通用SRF分类。建议由2号/7号责任任务交付上述真实接口及测试后恢复本任务，再新增pg09_23_execution/pg09_23_sql获取运行期红灯和同过滤器通过证据。此处为行为前置缺失，不能标记“已完成，待回归”。

检查命令及结果（从仓库根目录执行；均非Cargo验证）：

    rustcodegraph status
    # exit0，11849文件、350914节点，未报告新鲜度警告。
    rustcodegraph node --file pkg/parser/ast/lib.rs --offset 540 --limit 90
    # exit0，核对TableSource与ResultSetNode。
    rustcodegraph node --file pkg/expression/builtin.rs --offset 2370 --limit 65
    # exit0，核对PgFunctionCandidate。
    rustcodegraph node --file pkg/session/runtime/source.rs --offset 340 --limit 82
    # exit0，核对既有表来源消费者。
    rg -n 'TableFactor|TableRef|LATERAL|DerivedTable' pkg/parser/grammar/main.astergram
    # exit0，TableFactor无函数行源产生式。
    rg -n -i 'SetReturning|set_returning|returns_set|proretset|generate_series|srf' pkg --glob '*.rs' --glob '!generated/*' --max-count 4
    # exit0；命中目录专用路径及无关字符串，无普通行源/SRF分类实现。
    git show HEAD:.plans/2026-10-29-postgresql18-聚合窗口与集合返回/22-有序集合真实客户端差分.md
    # exit0，读取前序已提交完成证据。
    git diff --quiet -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/plan.md
    # exit0，只读文件未改。

另有两次候选路径搜索包含不存在的pg_numeric.rs/pg_exact_numeric.rs或专属函数计划目录，rg报告路径不存在；未据此判定接口缺失，结论以上述实际AST/grammar及全pkg搜索为依据。RustCodeGraph自然语言explore结果过宽，已改用精确文件node核对。

验证profile：只读前置分析，不适用代码WIP/Ready验收。无Cargo调用、未领取槽位、CARGO_TARGET_DIR未设置、有效运行测试数0；没有把零测试或静态检查当作行为完成证据。未运行cargo fmt/test、make lint或parser命令；未验证INT4序列值/行数/描述、NULL/边界/零步长、错误恢复或协议矩阵。正确性风险为目标行为仍未实施；无本轮生产修改引入的兼容性或性能变化。未新增生产文件、feature、依赖或Bazel变更。本轮仅状态/阻塞记录，无需git提交，完整新提交SHA不适用；保留任务文件供恢复。

按执行契约保留原Progress未完成项；本轮结果是明确前置阻塞，不是任务完成。恢复入口为上游2号函数行源AST与7号SRF分类接口的实际提交及聚焦测试证据。


## 2026-11-01 阻塞恢复与有界扩展授权


用户授权在现有解析器、分类、物理行源链补齐INT4 generate_series FROM所需局部接口，历史阻塞不再约束实施。只支持该重载及普通非关联FROM，不扩展INT8/UNNEST/LATERAL/投影。职责/实际注册点：ast/lib.rs::TableSource.FunctionSource存储调用，walk.rs使参数遍历/重绑沿原Visitor，sql_restore.rs保持语法还原；main.astergram与parser_actions/query.rs仅补FunctionCallGeneric的PG FROM产生式；query.rs的既有append_relational_source_fields完成SRF签名分类和固定int4描述；source.rs的既有表源入口消费该分类并调用executor builder；physical_plan_runtime.rs新增最小INT4序列ExecExecutor，builder.rs提供构造入口，按现有Chunk Open/Next/Close生命周期输出。新类型在现有文件内，不建第二套执行器，注册点仅是既有表源分支。必要分派门禁同步排除函数源进入普通物理表扫描；全部修改对应本任务函数行源、身份和生命周期。

先新增独立executor会话与server TCP回归，取得相同过滤器运行失败；再补生产实现与跨Chunk内核测试，最后执行Ready适用检查。原plan.md保持只读。使用do-task-plans与rustcodegraph技能，交付使用git-commit技能。


恢复阶段红灯：默认features、动态槽位1、CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。cargo fmt --all退出0；env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_23 -- --nocapture退出101，实际1项失败，收到0A000/native table functions are unsupported。executor初版测试访问私有rows编译失败不算行为证据；改用next_row/close后，同一env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_23 -- --nocapture退出101，实际1项失败，普通SQL FROM产生1064解析错误。日志/tmp/pg0923-{executor,server}-red.log。两个阶段锁均在进程结束后清理。

注册/修改依据补充：server/pg_name.rs仅让generate_series及pg_catalog.generate_series越过普通表名重写并交给session签名绑定，其他目录特例和函数门禁保留；dispatch.rs::simple_typed_select_shape及source.rs::collect_physical_table_sources/result_set_node_has_derived_source区分函数源和持久表源，避免扫描不存在的物理表。上述为本任务不可缺少的局部接线。main grammar增加1个TableFactor action，query动作清单244→245，正式RuleId tablefactor_functioncallgeneric_tableasnameopt_i--d6046ddcd17dfb81。parsergen在动态槽位1执行env -u LDFLAGS cargo run --locked --release -p astersql-parsergen -- generate退出0；只纳入main_tables生成差异，工作区原有hint修改及生成物不纳入本任务。INT4行源在Open检查零步长，任一NULL严格零行，checked_add溢出终止；Next保持跨Chunk状态，Close释放游标，重开重置。既有关系查询仍可能物化全部FROM结果，内存/取消/portal流式扩展属于后续29，不据小夹具声称常量查询内存。


实现中间检查：Int4Series内核跨块测试1/1通过，首次完整运行executor为1通过/1失败，server为0通过/1失败，均exit101，报Table test.不存在。定位为新增query动作误从item而非既有expr读取FunctionCallGeneric语义；已修正并增加独立parser FROM/参数遍历/还原/MySQL拒绝覆盖。该parser测试两次编译修正（crate::Parser限定名、Node::accept小写）不计行为证据。当前仍需同过滤器通过证据，不宣称完成。

Ready准备已执行make lint、make parser、make parser_yacc、make parser_fmt，各exit0；Go parser语法生成物无差异。第一次./tools/check/failpoint-state.sh enable go退出1，原因扫描target中同时由Cargo删除的rmeta临时目录，工具报告Rewrite/Restore lstat错误；refcount仍0、无Go源码差异/临时binding，未开始Go业务测试。待Cargo结束重试，不改共享failpoint脚本或其他进程。


后续运行executor2/2、parser1/1通过（exit0），server运行期失败exit101：代表简单FROM/全部13组方向NULL边界含2050行均已通过，pg_catalog.generate_series被目录所有权分类提前接管。追加本行为必要局部接线：pg_catalog_query.rs::parse_shadowed仅排除真实函数调用pg_catalog.generate_series(...)本身触发目录所有权，遇真实目录表仍按原门禁拒绝混合查询；独立pg09_23_catalog_ownership同时覆盖普通移交及混合目录拒绝，不在目录解释器内生成结果。这不是目录特例实现。pg_name.rs的范围名绑定补缺省函数范围名，供无表别名限定引用。


## 2026-11-01 最终完成证据


状态已完成，历史失败保留用于区分TDD/测试编译修正/准备失败与最后通过证据。使用Ready profile：先WIP聚焦红灯，最终默认features的同过滤器及实际受影响相邻回归、根规则适用lint/parser检查全部通过；缺失的tidb-verify-profile技能未宣称已调用。

最终行为证据：executor2/2（普通会话行值，以及10组Chunk输入、两次关闭重开、重复EOF、零步长及提前关闭），server2/2（TCP SQL/协议+目录所有权）。TCP的13组简单Q夹具含2050行、正反向、反方向零行、相等端点、任一NULL/全NULL、int4上下限及越界后终止；另4组默认列名/表别名变体及pg_catalog限定名过滤/排序/LIMIT、零步长错误和恢复。扩展协议Parse/statement Describe验证3个参数OID23及固定23/4/-1，text/binary参数×结果四组合各9次重绑（含重复NULL、方向反转、NULL与零步长、错误后恢复）；每轮Bind/portal Describe/Execute/Sync核对真实行数与字节/完成计数，Execute不重发描述。零步长C22023/SV ERROR/M step size cannot equal zero，D/H/P无字段；简单Q与扩展均恢复I，同事务错误后E、ROLLBACK后I及SELECT 1恢复已断言。listener由Drop关闭，连接通过Terminate退出；未创建外部参考服务或用户表。

所有本任务Cargo阶段均原子动态取得槽位1，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；包装脚本/tmp/pg0923-cargo.sh记录计划/任务/会话/PID并在Cargo及产物子进程结束后trap删除owner和rmdir。最后Ready持有PID14739，包装退出0，锁已不存在，编译缓存保留。未删除其他锁或进程。最终脚本bash /tmp/pg0923-cargo.sh bash /tmp/pg0923-ready-rust.sh退出0，日志/tmp/pg0923-<label>.log；以下每项最终退出码均0，数量是实际执行数，含不同过滤器的重复覆盖，不混算为唯一场景数。

| 完整命令 | 最终有效数量/结果 |
| --- | --- |
| cargo fmt --all | 0，最终Rust检查前执行 |
| env -u LDFLAGS cargo run --locked --release -p astersql-parsergen -- generate | 0，main生成物仅对应1条TableFactor语法增量 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_23 -- --nocapture | 2/2；executor-final |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_23 -- --nocapture | 2/2；server-final |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_23 -- --nocapture | 1/1；同一测试包含在最终15项邻近过滤器中 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_ -- --nocapture | 15/15；parser-neighbors |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib query_expression_rule_coverage -- --nocapture | 1/1；parser-actions，expression318/query245的完整分派/互斥清单 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib physical_plan_runtime_test -- --nocapture | 12/12；executor-neighbors |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_name_test -- --nocapture | 17/17；names，原范围/方言/描述边界 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib catalog_datagrip_scalar_and_lateral_bounds -- --nocapture | 1/1；catalog，混合目录generate_series仍拒绝 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_19_sql -- --nocapture | 1/1；percentile-disc |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_20_sql -- --nocapture | 1/1；percentile-cont |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_21_sql -- --nocapture | 1/1；percentile-diagnostics |
| make lint | 0 |
| make parser | 0 |
| make parser_yacc | 0；Go生成物无差异 |
| make parser_fmt | 0 |
| ./tools/check/failpoint-state.sh enable go | 最后0，refcount0→1；首次环境lstat失败1不算成功 |
| gofmt -w pkg/parser/ast/binding__failpoint_binding__.go | 0，仅临时注入文件 |
| make parser_unit_test | 0，14个ok包，多数缓存；不推断测试函数数 |
| ./tools/check/failpoint-state.sh disable go | 0，refcount1→0，临时注入与全部Go差异清理 |
| rustfmt --edition 2024 --config skip_children=true pkg/parser/parser_actions/query.rs | 0，补充include模块格式化 |
| rustfmt --edition 2024 --config skip_children=true pkg/server/pg_query_test.rs | 0，之后已执行最终cargo fmt --all |
| git diff --check | 0 |
| git diff --quiet -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/plan.md | 0，整体指南只读 |
| test ! -d target/rust-slot-locks/slot-1.lock | 0 |
| test ! -e pkg/parser/ast/binding__failpoint_binding__.go | 0 |

Go准备重试在本任务Cargo阶段结束后运行bash /tmp/pg0923-parser-go-tests.sh（0）；不修改共享failpoint脚本，日志/tmp/pg0923-{failpoint-enable,parser_unit_test,failpoint-disable}.log。Ready非Rust命令包装bash /tmp/pg0923-ready-go.sh退出0。未触发make bazel_prepare（无Go/依赖/Bazel修改、复用现有工作区），未运行禁止默认执行的bazel_lint_changed。最终git diff --name-only -- '*.go'输出为空。

变更30文件：executor builder/physical runtime及独立测试；parser AST字段/原Visitor/结构化还原、1条main grammar及生成表、query动作/独立parser测试/动作清单测试；server普通名称与目录所有权移交/独立TCP及所有权测试；session既有绑定/分派/行源入口；11份相应既有rs.md和本任务记录。全部生产路径/符号依据已逐项在授权恢复段及后续局部接线段记录；无新生产文件、feature、manifest或外部依赖。未暂存其他会话hint、expression、planner、runtime.rs、compression_port_test或计划变更。按git-commit技能提交本任务30文件；同一提交SHA无法自引用，完整SHA由最终回复给出。提交后按契约删除本任务文件，不改plan.md。

风险/限制：正确性覆盖本任务INT4普通非关联FROM支持域；未把INT8、UNNEST、LATERAL、投影SRF、WITH ORDINALITY或portal资源扩建纳入。兼容性由MySQL语法拒绝、原名称/目录/有序集合回归约束；不声称穷尽MySQL全套回归。序列执行器按Chunk逐步产生值，但原关系查询可能物化全部源行，时间O(n)、内存随结果量增长，未测吞吐/大内存/取消压力。未验证RealTiKV、全workspace、清洁独立checkout或真实PG18.6客户端差分；本轮未启动参考PG，不借用批次22的18.6结果充当序列证据。两PG连接和同Server MySQL交错的序列差分留任务30；暂停恢复与资源扩展留29。下一步按原计划继续24的INT8序列边界终止，不以这些范围外事项阻塞23。

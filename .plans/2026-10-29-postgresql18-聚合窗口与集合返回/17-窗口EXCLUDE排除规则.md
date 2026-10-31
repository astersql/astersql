# 任务 17：窗口EXCLUDE排除规则

批次：【批次 17】；调度依赖：批次16（共享源码/注册与验证资源串行）；行为依赖：16。

状态：已完成

目的：帧排除在基础帧上应用 CURRENT ROW/GROUP/TIES/NO OTHERS，不改变分区或 peer 身份。10/10/20 按唯一 id 的累计 ROWS EXCLUDE CURRENT ROW 得 NULL/10/10。

来源任务：大计划9；路线图第4章 `WIN-CALL-FRAME的EXCLUDE目标`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：2号 EXCLUDE AST，任务13–16的基础帧与peer。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

聚合复用 `AggFuncDesc` / `TypedHashAgg` / `BuildTypedPhysicalPlan`，窗口复用 `PhysicalWindowPlan` 与缓冲/流水线入口，SRF沿现有物理行源/投影接线；只使用本任务相关链。协议复用 `PgService`、`ConcreteSessionDriver` 与既有真实会话；目录特例不能充当普通SQL执行。新生产类型/文件若非必要不新增，必须新增时先在本活动计划记录职责及实际注册点。

## 文件与修改依据


- 候选修改：`pkg/expression/aggregation/window_func.rs`。
- 候选修改：`pkg/executor/windows/builder.rs`。
- 候选修改：`pkg/executor/windows/window.rs`。
- 候选修改：`pkg/executor/windows/pipelined_window.rs`。
- 候选修改：`pkg/executor/aggfuncs/builder.rs`。
- 候选修改：`pkg/session/runtime/query.rs`。
- 候选修改：`pkg/session/runtime/planning.rs`。
- 必要语法局部接线候选：`pkg/parser/parser_runtime.rs`、`pkg/parser/ast/lib.rs`；复用上游2号已提供载体，不重建解析器。
- 独立测试：`pkg/executor/windows/window_sql_test.rs`、`pkg/server/pg_query_test.rs`；描述断言按需扩展 `pkg/server/pg_types_test.rs`。

上述均为已存在候选文件（明确标为待新增者除外），只修改实现本项所需者。每项生产修改须在本文件记录“路径/符号→本任务行为或不可缺少的局部接线”。本计划是PG功能增量，不是Go逐提交映射；不得伪造Go SHA或删减既有Go回归语义。

## 行为与验收夹具


帧排除在基础帧上应用 CURRENT ROW/GROUP/TIES/NO OTHERS，不改变分区或 peer 身份。10/10/20 按唯一 id 的累计 ROWS EXCLUDE CURRENT ROW 得 NULL/10/10。

代表SQL（其他边界使用同型最小夹具）：

    SELECT last_value(x) OVER(ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW) FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)

其余排除形式用含同序行与 NULL 的小夹具取得参考结果；合法排除后空帧仍保留输入行，不能仅接受语法而忽略排除。

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


在 `pkg/executor/windows/window_sql_test.rs` 待新增 `pg09_17_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_17_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_17 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_17 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次17执行；行为依赖为16，调度依赖为批次16。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_17 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_17 -- --nocapture
    make lint
    git diff --check

新增/修改解析器时还须按根矩阵运行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 并审查生成差异；不修改解析器不无故执行这些命令。Bazel准备按根完整触发清单判定；只改Rust且无Bazel/Go/依赖触发时不运行 `make bazel_prepare`。禁止默认运行 `make bazel_lint_changed`。如需Go测试，先依testing-flow判定failpoint并在退出禁用；Go既有回归失败记录后按用户规则不阻塞本任务，新增回归不得忽略。

检查当前改动实际影响的原有相邻测试并记录精确命令；不运行无依据全包/昂贵广扫。涉及真实存储才依 `docs/rust/rust-postgresql-tikv-runbook.md` 与testing-flow启动专属后台playground、验证并清理。本计划不授权在拆分阶段运行上述命令。

风险：共享描述符/执行状态改动可能影响MySQL；排序/帧/行扩展可能增加内存或错误地改变行数；先保持单阶段正确并验证资源释放，不据小夹具给出吞吐承诺。恢复只撤销自己明确有问题的变更，不删除其他任务/用户文件，不移除未知锁或服务。

## 完成与交接


只使用状态：未开始、已完成、已阻塞、已完成，待回归。完整行为和所需适用验证通过后标记已完成；有实际代码/测试/配置/文档变更使用 `$git-commit` 技能只提交本任务。只有编号文件状态或删除时无需单独提交。最终报告路径/符号、生产修改依据、验证profile、实际槽位/绝对CARGO_TARGET_DIR、完整命令/退出码/有效计数、正确性/兼容性/性能风险、未验证项及完整提交SHA。

验证因无关环境缺失无法运行，可记录已完成，待回归并保留本文件；不能把仍缺失的目标行为或依赖接口标为完成。阻塞只写本文件及已做的检查，不扩大范围。完成证据已交接且提交妥当后按执行契约删除本编号文件，`plan.md`只读，父大计划和覆盖表保留。删除文件本身不是完成证据。

## Progress


- [x] 确认行为依赖与实际入口；获授权补齐有界EXCLUDE接口。
- [x] 记录运行期失败：parser、executor、server各实际运行1测试并失败。
- [x] 完成最小实现及同夹具通过验证。
- [x] Ready检查与diff自审完成；按git-commit技能提交本任务交接。

## Surprises & Discoveries


2026-10-31：批次16已有实现和交接证据，但外部2号EXCLUDE AST/语法接口缺失；具体静态证据和未运行检查见本文件末尾。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


2026-10-31：阻塞恢复后获用户明确授权，补齐有界EXCLUDE语法/AST并完成共享窗口实现及Ready验证，最终证据见末节。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-10-31 前置检查与阻塞交接


使用仓库 skills/do-task-plans/SKILL.md、skills/rustcodegraph/SKILL.md，已读取只读plan.md、本任务、PLANS.md、根AGENTS.md、pkg/server/AGENTS.md及docs/agents/testing-flow.md。目标目录未发现doc.go；.agents/skills/tidb-verify-profile/SKILL.md读取退出1（文件不存在），未宣称运行该技能。

批次16的实现提交为67c1016bed01f921af47723cd6624a0320e7deb8，已读取其编号文件内最终证据：pg09_16执行器及server各1/1通过，周边12–15回归及make lint通过；这些是上轮交接证据，本轮未重跑。当前图中pg09_16_execution、pg09_16_sql及relational_window_value均存在，基础帧/peer接口可追溯。检查开始时HEAD为67c1016bed01f921af47723cd6624a0320e7deb8，后续git rev-parse HEAD为15a82fc23d861123201147b70030116af46ce6c1（共享仓库并发提交），本任务不改动其他会话内容。

阻塞责任为本文件“实际外部前置”的外部2号语法/作用域接口：pkg/parser/ast/lib.rs:4453的FrameClause仅Type和Extent，WindowSpec也无排除字段；pkg/parser/grammar/main.astergram:2935仅WindowFrameUnits WindowFrameExtent；pkg/parser/parser_actions/query.rs:1105构造FrameClause也仅写入这两个字段。因此没有可供本任务接线的CURRENT ROW/GROUP/TIES/NO OTHERS载体。不能仅扩展执行器就保留SQL排除语义。本任务明确复用2号载体且缺失时记录阻塞，不在17号补建解析器。

静态检查命令及退出码（全部从仓库根目录运行）：

    ~/.rustcodegraph/bin/rustcodegraph status
    # 0；11847文件，未报告新鲜度警告；索引不是行为验证。
    ~/.rustcodegraph/bin/rustcodegraph query FrameClause --limit 5
    # 0；定位当前AST。
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/parser/ast/lib.rs --offset 4390 --limit 110
    # 0；FrameClause/WindowSpec无排除字段。
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/parser/parser_actions/query.rs --offset 1090 --limit 28
    # 0；构造载体无排除字段。
    rg -n 'EXCLUDE|EXCLUSION|Exclude|Exclusion' pkg/parser/grammar pkg/parser/parser_actions/query.rs pkg/parser/parser_runtime.rs pkg/parser/ast/lib.rs
    # 1；无匹配。原始文件搜索补充图未覆盖的语法文本与缺失符号证据。
    rg -n 'FrameClause|FrameExtent|FrameType::Groups|frameClause' pkg/parser/parser_actions/query.rs pkg/parser/grammar/main.astergram
    # 0；定位现有帧产生式。
    ~/.rustcodegraph/bin/rustcodegraph query pg09_16 --limit 5
    # 0；两个独立测试存在。
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/relational_value.rs --offset 3870 --limit 85
    # 0；现有真实窗口接线与peer排序入口。
    git show --stat 67c1016bed01f921af47723cd6624a0320e7deb8
    # 0；批次16提交存在。
    git show 67c1016bed01f921af47723cd6624a0320e7deb8 --format= -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/16-GROUPS同序组帧.md
    # 0；读取前序最终交接。
    git diff --quiet -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/plan.md
    # 0；只读plan.md无差异。

本轮仅修改本编号文件，未修改生产/测试/配置/其他文档，不提交此状态更新，不删除任务文件。Profile不适用（仅前置只读分析与阻塞状态记录）；未运行任何Cargo或调用Cargo的脚本，未领取槽位，CARGO_TARGET_DIR未设置，无本任务锁或产物进程待清理，有效测试数0。跳过cargo fmt --all、两个pg09_17测试、相邻回归、make lint及parser构建：外部行为前置不满足，尚未实施；不将零测试当作通过。

风险与下一步：EXCLUDE行为仍未实现/验证，正确性不能宣称满足；无生产变更，不新增兼容性或性能风险，也未测量性能。请先在外部2号提供EXCLUDE语法、AST载体及可追溯测试，再恢复17号的正常TDD与Ready交付。真实PG18.6、协议固定描述/错误恢复、四种排除形式、跨页清理均未在本轮验证；依赖缺口不能标为“已完成，待回归”。最终git diff --check结果在最终回复报告。


## 2026-10-31 阻塞恢复复核与所需范围决策


本轮按恢复指令重读任务证据，HEAD为15a82fc23d861123201147b70030116af46ce6c1。git status --short pkg/parser显示其他会话仅在hint/misc相关路径有未提交修改，没有新增窗口载体；不触碰这些改动。复用上一轮已核对的批次16行为证据，不重复构建。

已穷尽当前可识别的载体/入口替代方案：

1. 直接复用真实AST：lib.rs::FrameClause仍只有Type/Extent；不存在排除字段。备用ast/dml.rs::FrameClause也只有frame_type/start/end，且其try_restore对GROUPS仍拒绝，不能替换真实SQL AST。
2. 直接复用计划/内核载体：logicalop/logical_window.rs::WindowFrame只有Type/Start/End，executor/windows/window.rs::WindowFrame只有frame_type/start/end；均不能接收四种排除身份。改内核本身仍无法使未被语法接受的SQL到达执行阶段。
3. 借用现有FrameBound.Expr、Unit、WindowSpec.Name/Ref等字段：这些字段已有边界表达式、单位、命名窗口语义，排除与基础帧独立，借用会丢失真实含义或破坏合法组合及克隆/还原，不是安全载体。
4. session或parser_runtime预处理原SQL、剥离EXCLUDE后通过旁表/线程局部状态传递：yyParse实际是依赖生成表的LALR状态机，无现有EXCLUDE扩展钩子。安全处理引号/注释、多个窗口、命名窗口、嵌套查询及错误位置会另行承担语法/作用域职责；只按字符串匹配又不能满足行为契约，因此不实施旁路。
5. 在现有解析器中正式增加局部产生式是最小正确路径，无需完整解析器重建；但当前没有上游载体可复用，与任务明确的“实际外部前置：2号 EXCLUDE AST”“复用上游2号已提供载体”冲突。恢复指令仍要求不吞并外部2号任务，因此需要一次明确范围决策，而非技术上宣称不可实现。

需要用户作出的具体决定：是否授权17号在本次任务中补齐仅限窗口EXCLUDE的外部2号接口，随后继续原17号执行与验收？建议授权这个有界增量；另一选择是保持17号阻塞，由外部2号先交付下列接口。任一方式都不包含2号其他SQL语法、通用作用域、完整解析器或其他窗口函数扩建。

最小外部补齐范围为：pkg/parser/ast/lib.rs增加四种排除身份（NoOthers默认、CurrentRow、Group、Ties）及FrameClause字段；在pkg/parser/grammar/main.astergram现有WindowFrameExtent之后添加可选EXCLUDE子句，仅允许合法帧位置，保持MySQL方言拒绝；必要时在现有token/lexer路径添加或复用词元，避免不必要的保留字扩大；pkg/parser/parser_actions/query.rs构造该字段，pkg/parser/ast/sql_restore.rs保留排除子句；更新所有该AST构造点的默认字段，增加独立解析/还原/非法位置及MySQL隔离测试；用仓库现有生成器更新必要生成表与对应.rs.md说明。需先核对现有生成命令，禁止手改生成表或另建解析器。该最小范围不预先承诺零生成差异。

外部交付的可验收接口：代表SQL和CURRENT ROW/GROUP/TIES/NO OTHERS均通过真实parser产生可区分枚举；省略等价NO OTHERS；嵌套/多窗口/命名窗口不串用状态；非法排除位置及MySQL相同语法被拒绝；AST克隆及SQL还原不丢失身份。随后17号仍承担基础帧上排除、peer不变、空帧行保留和原SQL/协议测试，不把解析测试替代执行证据。

本轮新增静态检查（均仓库根执行，退出0）：

    git status --short pkg/parser
    git log -4 --oneline
    ~/.rustcodegraph/bin/rustcodegraph status
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/parser/parser_runtime.rs --offset 1 --limit 160
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/parser/ast/dml.rs --offset 325 --limit 45
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/parser/ast/sql_restore.rs --offset 465 --limit 65
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/executor/windows/window.rs --offset 660 --limit 60
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/planner/core/operator/logicalop/logical_window.rs --offset 155 --limit 40
    rg -n 'EXCLUDE|Exclusion|exclusion|FrameClause \{' pkg/parser pkg/session/runtime pkg/executor/windows pkg/planner/core/operator/logicalop/logical_window.rs -g '*.rs' -g '*.astergram' -g '!hint_tables.rs' -g '!main_tables.rs'
    rg -n '^parser:|^parser_yacc:|^parser_fmt:|^parser_unit_test:|parsergen' Makefile pkg/parser/Makefile

搜索命中仅FrameClause构造及ON CONFLICT约束注释，没有窗口排除；搜索不是运行证明。仍仅更新本任务文件，状态保持已阻塞，未实施生产/测试改动。Profile不适用；无Cargo、无槽位、未设置CARGO_TARGET_DIR、有效测试0，不提交此状态更新。未运行的验证与风险保持上一节所列。下一步仅等待上述有界接口授权或外部2号实际交付，不等待宽泛的完整2号完成；未标记待回归。


## 授权恢复与局部实施方案


2026-10-31：用户已明确授权17号补齐有界EXCLUDE接口，无需外部2号先交付。保持实施中原状态至验收成功（允许状态无进行中）；此前范围决策阻塞已解除。将增加既有AST的FrameExclusion枚举及字段，扩展现有生成语法/语义动作/还原和独立测试。执行器复用现有ROWS/RANGE边界处理，在基础帧后筛除当前行或peer，排除帧禁用连续滑动优化并使用既有缓冲路径，不改变分区/peer身份。生产枚举注册于FrameClause与WindowFrame；不新增生产文件/第二执行器。每处必要默认字段迁移仅保证原有NoOthers行为。


## 有界生产变更依据与WIP证据


2026-10-31：用户已明确授权以下必要局部接线，本轮不再等待范围批准。pkg/parser/ast/lib.rs::FrameExclusion/FrameClause增加四种排除身份；grammar/main.astergram仅在既有显式帧后添加可选EXCLUDE，三个新词元仍为非保留标识符；lexer.rs仅PG未引用名字映射；parser_actions/query.rs在原有帧动作注册排除枚举并保持MySQL拒绝；ast/sql_restore.rs还原语义后缀，NoOthers可省略为等价默认。ast/dml_test.rs构造点仅补默认字段。现有parsergen生成main_tables.rs/lexer_tokens.rs，保留其他会话hint输出不提交。

pkg/executor/windows/window.rs::WindowFrame增加同等独立枚举；calculate_frames在现有ROWS/RANGE基础边界后按原始当前行和peer排除，TIES不把帧外当前行加入；排除帧禁止连续slide而复用reset/update/result，frame-insensitive函数仍走原路径。builder.rs::build将非默认排除路由既有缓冲处理器，ROWS透传ORDER BY peer键，无新执行器。pkg/session/runtime/relational_value.rs::relational_window_value透传真实AST到int4 last_value物理帧，GROUPS仍复用dense peer/RANGE；未接线的帧敏感旧求值器签名明确0A000，避免忽略排除。其他现有WindowFrame构造点（windows独立测试及executor/benchmark_test.rs）只增加NoOthers默认值以维持原行为。已有对应.rs.md同步说明，没有新增生产文件或外部依赖。

WIP红阶段：槽位2，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2，PID74150；cargo fmt --all退出0；env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_17 -- --nocapture退出101，实际1失败1，实际[10,10,20]与预期[NULL,10,10]不符；server同过滤器命令退出101，实际1失败1，EXCLUDE在真实SQL入口拒绝。另动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，PID90220；env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_17 -- --nocapture退出101，实际1失败1，EXCLUDE处运行期语法错误。三项均非编译失败/零测试。锁均已清理，缓存保留。日志/tmp/pg09_17-{parser,executor,server}-red.log。

本机参考为PostgreSQL18.4(Homebrew)，不是18.6。python3 /tmp/pg09_17-reference.py退出0，通过任务专属initdb+Unix socket临时实例核对28组成功SQL的有序值/NULL/行数，finally停止进程并删除仅自建目录；日志/tmp/pg09_17-reference.log。参考比对脚本首轮因None解析自身错误退出1（数据库实际代表输出已是NULL/10/10），修正脚本后28组全部匹配。未把18.4作为18.6完整客户端差分证据。

Ready前置：make lint退出0；make parser、make parser_yacc、make parser_fmt均退出0，未产生Go文件差异。make parser_unit_test初次未启用failpoints命中缓存退出0，不作为符合failpoint规则的证据。发现ast/misc.go使用failpoint后，通过tools/check/failpoint-state.sh enable go启用，make parser_unit_test首次退出2：临时binding__failpoint_binding__.go被gofmt判格式不符，未进入测试；trap已disable成功。第二轮enable后仅gofmt此临时文件，再make parser_unit_test退出0，trap disable退出0/refcount0，无持久Go差异。对应日志/tmp/pg09_17-{parser,parser_yacc,parser_fmt,parser_unit_test-failpoint-2,failpoint-enable-2,failpoint-disable-2,lint}.log。根Bazel触发检查无新增/删除Go、import、依赖或Bazel元数据变更；无需bazel_prepare，未运行bazel_lint_changed。

绿阶段槽位1/PID96387先cargo fmt --all退出0。debug版parsergen generate计算耗时后暂停自己的外层shell、终止自己的生成器，不释放仍持有的槽位1锁，改为动态领取槽位2/PID11438运行env -u LDFLAGS cargo run --release --locked -p astersql-parsergen -- generate；待成功后再继续原测试阶段，避免并行写生成文件。生成过程及后续验证结果待补。


首轮绿阶段parser实际2/2通过；executor/server先遇到默认字段迁移的编译错误（不作为行为失败证据）。补齐windows测试和executor benchmark的shorthand字段，以及expression/aggregation/window_func.rs::useDefaultFrameWindowFuncs、planner/core/logical_plan_builder_runtime.rs默认AST帧的NoOthers；后两处只是新增字段必需构造接线，不改变其他规划逻辑。planner文件已有他人修改，提交时只暂存本任务新增默认字段，不能整文件暂存。


## 最终完成证据（2026-10-31）


任务已完成：真实PostgreSQL会话中的int4 last_value在ROWS/RANGE/GROUPS基础帧后应用CURRENT ROW/GROUP/TIES/NO OTHERS。代表SQL为NULL/10/10；空帧保留每个输入行，peer/分区身份不变，TIES不会引入帧外当前行。共享物理内核也支持既有帧函数的非连续输入，SQL本任务只为既有typed int4 last_value链接线；尚未接线的帧敏感签名返回0A000，不声称其他函数/类型完整支持。

测试覆盖28组成功SQL，Q、扩展text/binary结果均断言有序值/NULL/行数及固定别名r、OID23、长度4、typmod-1；零行和首行NULL不改变描述。statement/portal两次Describe且Execute不重发描述。text/binary参数×结果四组合，每种false/true/false三次NULL重绑，共12次。非法位置/未知排除42601；NULL/负帧偏移22004/22013；未接线text last_value排除0A000；输出原始S/V/C/M与适用P（缺省D/H不声称差分完成），错误Q后和扩展Sync后SELECT1成功，ReadyForQuery I。两PG连接和同Server MySQL交错合法查询均通过，Drop关闭自建listener并验证端口不可重连。

内核同一测试覆盖两种执行模式请求、ROWS/RANGE、四种排除、跨Chunk的peer、分区重置、ROW_NUMBER忽略帧及close后内存0、耗尽输入重开无残留。NO OTHERS保持原滑动/流水线行为，非连续排除复用既有缓冲处理器。新增默认字段迁移遗漏造成首轮编译失败，修正后新增ROW_NUMBER断言发现既有结果是UInt（测试误写Int），仅修正测试预期；这些中间失败不替代原有有效运行期红证据。最终同过滤器全部转绿。

Profile为WIP红/绿后Ready；verify-profile技能缺失，按根AGENTS/testing-flow选取适用检查。实际槽位仅1和2：CARGO_TARGET_DIR分别/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1与/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2。绿阶段/Ready的holder为96387、17264、19749、21631，release生成holder11438。所有本任务Cargo及产物进程已结束，自身owner/锁目录已清理、缓存保留；剩余slot-3属于其他会话未触碰。debug生成主动终止退出143，release生成及check均退出0，生成成功后才恢复测试，没有并行写生成文件。

以下命令均仓库根执行，Cargo继承动态领取槽位的上述绝对目录；最终成功命令及有效数量如下（原红证据见前节）：

| 完整命令 | 退出码/有效计数 |
| --- | --- |
| cargo fmt --all | 0（每次代码调整后先格式化） |
| env -u LDFLAGS cargo run --release --locked -p astersql-parsergen -- generate | 0 |
| env -u LDFLAGS cargo run --release --locked -p astersql-parsergen -- check | 0 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_17 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_17 -- --nocapture | 0，1/1 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_17 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib | 0，23/23 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_ -- --nocapture | 0，10/10 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib test_window -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib benchmark_window_frame -- --nocapture | 0，6/6 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_12 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_14 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_15 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_16 -- --nocapture | 0，1/1 |
| make parser | 0 |
| make parser_yacc | 0 |
| make parser_fmt | 0 |
| ./tools/check/failpoint-state.sh enable go | 0 |
| gofmt -w pkg/parser/ast/binding__failpoint_binding__.go | 0，仅临时failpoint文件 |
| make parser_unit_test | 0，Go包级测试含缓存，不计入Rust行为数 |
| ./tools/check/failpoint-state.sh disable go | 0，refcount0，trap清理 |
| make lint | 0 |
| python3 /tmp/pg09_17-reference.py | 0，28组PG18.4参考匹配并清理 |
| /opt/homebrew/opt/postgresql@18/bin/psql --version | 0，18.4(Homebrew) |
| git diff --check | 0 |
| git diff --quiet -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/plan.md | 0 |

参考构建为PostgreSQL18.4(Homebrew) on aarch64-apple-darwin25.4.0，Apple clang21.0.0(clang-2100.0.123.102)，64-bit；client psql18.4，UTF8，TimeZone Asia/Shanghai，--no-locale任务专属initdb、trust本机Unix socket且listen_addresses空。未使用用户数据库/表/进程。

风险：正确性以已验证的typed int4 last_value和现有peer比较域为界；未接线签名不忽略EXCLUDE，明确拒绝。兼容性由MySQL parser用例、原窗口23项、默认帧6项、PG12–16回归及双协议交错约束。性能沿既有逐行会话规划及缓冲内核，非连续帧逐行筛选/重算并短期克隆帧行，未压测、不作吞吐承诺。未验证真实PG18.6完整客户端/所有错误字段精确差分（任务18负责），未运行RealTiKV、workspace全量或昂贵exact-Go benchmark；无真实存储依赖。

自审仅本任务28整文件及planner共享文件一个NoOthers构造hunk（共29文件含编号任务）。不暂存其他hint/mview/compression等改动。git add初次退出128，发现约14分钟的空.git/index.lock；两次lsof无持有者、进程表无git/钩子进程，复核inode/mtime未变后清理失效锁。此为提交准备问题，不是行为阻塞。提交前核对cached清单及git diff --cached --check；完整SHA在最终回复报告，随后删除本编号文件而不另提交删除。plan.md保持只读。

日志集中/tmp/pg09_17-*.log；最终绿为parser-green、executor-ready、server-green-2；回归为windows-regression、parser-regression、parser-mysql、executor-defaults、server-pg09_12至16；生成检查为generate-release和parsergen-check。后续推荐执行18号窗口真实客户端差分，针对PG18.6扩充证据，不递归扩大本任务范围。

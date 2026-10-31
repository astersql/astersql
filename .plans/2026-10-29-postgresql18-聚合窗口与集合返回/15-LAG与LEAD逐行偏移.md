# 任务 15：LAG与LEAD逐行偏移

批次：【批次 15】；调度依赖：批次14（共享源码/注册与验证资源串行）；行为依赖：13。

状态：已完成

目的：offset/default 相对当前行求值；0 取自身，负值反方向，NULL offset 返回 NULL，越界才用 default，存在的 NULL 值不能替换。偏移访问整分区，不受窗口帧限制。

来源任务：大计划9；路线图第4章 `WIN-CALL-OFFSET/NULL/EMPTY-SINGLE`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：6号 offset/default 参数类型，任务12的分区和任务13的帧。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

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


offset/default 相对当前行求值；0 取自身，负值反方向，NULL offset 返回 NULL，越界才用 default，存在的 NULL 值不能替换。偏移访问整分区，不受窗口帧限制。

代表SQL（其他边界使用同型最小夹具）：

    SELECT lag(x,o,id::int4) OVER(ORDER BY id) FROM (VALUES(1,10::int4,1),(2,20::int4,2),(3,30::int4,0)) t(id,x,o)

期望 [1,2,30]；补 lead NULL offset、lag -1、跨 Chunk、两分区、空/单行，固定 int4 描述。只消费已有 anycompatible 在 int4 域的能力。

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


在 `pkg/executor/windows/window_sql_test.rs` 待新增 `pg09_15_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_15_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_15 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_15 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次15执行；行为依赖为13，调度依赖为批次14。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_15 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_15 -- --nocapture
    make lint
    git diff --check

新增/修改解析器时还须按根矩阵运行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 并审查生成差异；不修改解析器不无故执行这些命令。Bazel准备按根完整触发清单判定；只改Rust且无Bazel/Go/依赖触发时不运行 `make bazel_prepare`。禁止默认运行 `make bazel_lint_changed`。如需Go测试，先依testing-flow判定failpoint并在退出禁用；Go既有回归失败记录后按用户规则不阻塞本任务，新增回归不得忽略。

检查当前改动实际影响的原有相邻测试并记录精确命令；不运行无依据全包/昂贵广扫。涉及真实存储才依 `docs/rust/rust-postgresql-tikv-runbook.md` 与testing-flow启动专属后台playground、验证并清理。本计划不授权在拆分阶段运行上述命令。

风险：共享描述符/执行状态改动可能影响MySQL；排序/帧/行扩展可能增加内存或错误地改变行数；先保持单阶段正确并验证资源释放，不据小夹具给出吞吐承诺。恢复只撤销自己明确有问题的变更，不删除其他任务/用户文件，不移除未知锁或服务。

## 完成与交接


只使用状态：未开始、已完成、已阻塞、已完成，待回归。完整行为和所需适用验证通过后标记已完成；有实际代码/测试/配置/文档变更使用 `$git-commit` 技能只提交本任务。只有编号文件状态或删除时无需单独提交。最终报告路径/符号、生产修改依据、验证profile、实际槽位/绝对CARGO_TARGET_DIR、完整命令/退出码/有效计数、正确性/兼容性/性能风险、未验证项及完整提交SHA。

验证因无关环境缺失无法运行，可记录已完成，待回归并保留本文件；不能把仍缺失的目标行为或依赖接口标为完成。阻塞只写本文件及已做的检查，不扩大范围。完成证据已交接且提交妥当后按执行契约删除本编号文件，`plan.md`只读，父大计划和覆盖表保留。删除文件本身不是完成证据。

## Progress


- [x] 确认行为依赖与实际入口；按用户继续授权补最小局部类型接口。
- [x] 记录运行期失败或已有正确行为的当前证据。
- [x] 完成最小实现及同夹具通过验证。
- [x] Ready检查和diff自审完成；仅提交本任务20个文件，SHA在最终交接报告。

## Surprises & Discoveries


当前仅有静态规划，没有运行结果。实施中在这里记录实际接口差异、缺失依赖和对应证据。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


尚未实施。完成时填写实际输出、有效测试数量、参考版本、清理结果、剩余范围外依赖及完整SHA。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-10-31 前置接口核对与阻塞交接

仅更新本编号文件；未修改只读plan.md或任何生产/测试文件。未递归实施6号类型系统任务。

调度批次14已有完整提交 de5b853e46cdbbd922564d0d75246f5e08d04cd4，git show中的交付记录为窗口3/3、server2/2及Ready检查通过。行为依赖12为3515502d56599bac520ac11e38c1f1257009e800；13为ccecda92ec36bafb9bb9a6ee74115bc198070c7b，其保留文件记录窗口2/2、server2/2，状态待回归仅由既有parser admin库存断言导致。以上是既有交接证据，本会话没有重跑，未凭删除任务文件推定依赖完成。

实际阻塞是本文件明确要求的外部前置“6号 offset/default 参数类型”和“只消费已有 anycompatible 在 int4 域的能力”尚未在当前入口提供。planning.rs/ConcreteSession::protocol_parameter_types（2848起）逐项读取Constraints visitor：只有substr/substring/date_trunc、int8not、浮点abs及显式Cast约束，没有WindowFunction的lag/lead offset为int4、value/default同型绑定。全pkg/session搜索anycompatible无结果，lag/lead仅命中relational_value.rs的固定偏移分支。relational_expression_type（3725起）和query.rs/relational_query_expression_type_from_sources_uncollated（5910起）目前仅对last_value恢复参数返回身份；不能将显式cast参数可用声称为lag/lead签名已提供。

另确认目标行为尚缺：relational_value.rs/relational_window_value的lag/lead（4372–4384）只访问position±1，完全未读取Args[1]/Args[2]，故本任务不是已有正确行为可直接标记完成的情况。此次为前置检查阶段，不将静态检查冒充运行期红灯。

检查命令与结果（仓库根目录）：

- `~/.rustcodegraph/bin/rustcodegraph status`：0，11847索引文件，无新鲜度警告。
- `~/.rustcodegraph/bin/rustcodegraph explore 'lag lead PhysicalWindowPlan window offset'`：0，365结果；随后使用精确node避免名称歧义。
- `~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/relational_value.rs --offset 3797 --limit 115`、同命令`--offset 4270 --limit 150`及`--offset 3700 --limit 90`：均0，定位分区排序、固定偏移和类型分派。
- `~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/planning.rs --offset 2840 --limit 100`：0；`sed -n '2930,3118p' pkg/session/runtime/planning.rs`和`sed -n '3155,3205p' pkg/session/runtime/planning.rs`：均0，完整读取参数约束逻辑。
- `rg -n 'fn protocol_parameter_types|fn .*parameter.*type|anycompatible' pkg/session`：0，定位planning.rs与control.rs入口。
- `git show -s --format='%H %s' ccecda92ec 3515502d56`：0；`git show HEAD:'.plans/2026-10-29-postgresql18-聚合窗口与集合返回/14-数值RANGE偏移帧.md'`：0，读取批次14提交内交接。
- 查阅根AGENTS.md、PLANS.md、执行技能、RustCodeGraph技能、testing-flow和server/AGENTS.md。目标windows/session/server的doc.go及verify-profile技能不存在（ls退出1），未虚构读取或执行成功。
- 早期带未匹配`pg_parameter*`、`pg_type*`、`parameter*`、`protocol*`的搜索被zsh拒绝（退出1，0测试），已改为对pkg/session的无glob搜索；不视为缺接口的唯一依据。

没有执行Cargo或调用Cargo的脚本，没有领取槽位、没有CARGO_TARGET_DIR、没有锁或进程需要清理；有效行为测试数0。此处只有前置只读分析，无WIP/Ready代码验证profile，不运行fmt/lint/build/parser/RealTiKV。`git diff --check -- <本任务文件>`结果见最终交接。

正确性/兼容性/性能：无代码变更，因此没有引入运行行为变化；目标行为尚未实现和验证。未验证Q/扩展协议、零行描述、跨Chunk及当前参考PG18.6差分。下一步由6号接口责任提供上述窗口签名及int4同型能力，或用户明确授权本任务补齐最小接口后再继续TDD。按阻塞分支保留本任务文件，不提交仅状态/阻塞记录，不报告完成SHA。


## 继续实施（2026-10-31）

用户明确授权在现有AST/类型/窗口链内补齐offset/default与int4同型最小接线，前次依赖阻塞被此授权解除。仅限LAG/LEAD，禁止扩建通用类型系统。先新增pg09_15执行器/真实TCP会话回归取得运行期失败。生产方案：扩展既有Lag状态支持逐行列参数和方向，通过PhysicalWindowPlan复用分区访问；relational_value/query绑定int4返回身份；planning对WindowFunction提供局部参数约束。


生产修改依据：

- `pkg/executor/windows/window.rs` → Lag 复用既有状态，新增 with_row_arguments 绑定逐行 offset/default 与 lead 方向；signed 偏移用 i128 计算，NULL offset 返回 NULL，越界才选择当前行 default。requires_partition 标识整分区读取；窄帧处理器仅初始化一次该状态，保留目标 NULL。
- `pkg/executor/windows/builder.rs` → 存在 requires_partition 函数时复用缓冲式执行器；任意 signed LAG/LEAD 偏移不能依赖流水线当前帧已就绪条件。此回退不创建第二执行器。
- `pkg/session/runtime/relational_value.rs` → pg_offset_window_signature 仅检查本任务 int4 LAG/LEAD 签名；relational_window_value 将当前行 offset/default 与整分区输入绑定到 PhysicalWindowPlan/Lag；default 只在对应行越界时求值。保留现有 SQL 分区排序，结果身份继承首参数。
- `pkg/session/runtime/query.rs` → PG LAG/LEAD 与 last_value 一样从绑定首参数推导固定返回身份，零行 Describe 不从数据猜测类型。
- `pkg/session/runtime/planning.rs` → protocol_parameter_types 对 LAG/LEAD 的直接未知参数提供本任务 int4 域约束，显式不兼容参数在 Parse 返回42883；validate_function_visibility 接入局部窗口签名检查，覆盖空输入。
- `pkg/parser/parser_actions/query.rs` → OptLeadLagInfo 复用 Expression 载体接收逐行 offset，原 NumLiteral/paramMarker 两条归并为一条动作；MySQL 仍限制原有数字或参数形式。
- `pkg/parser/generated/main_tables.rs` → 由既有 astersql-parsergen generate 生成；OptLeadLagInfo 的 NumLiteral/paramMarker 合并为 Expression，以承载 PG 逐行、负数、NULL 与 cast 偏移。

WIP红灯：kernel pg09_15 退出101，1个有效失败（窄帧LAG source row越界）；server pg09_15退出101，1个有效失败（代表SQL offset表达式42601）。修改后kernel2/2通过，parser AST1/1通过；server扩充协议用例后仍在验证。动态槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。无外部依赖变更，无Go源码/import/Bazel变更，不触发bazel_prepare；修改Rust解析器后运行指定parser Make目标。不运行bazel_lint_changed。


最终审查补充：query.rs与relational_value.rs在本int4域复用pg_union_common_type合并value/default的身份，尤其未定型NULL value与int4 default以及零行Describe；不实现通用anycompatible函数系统。relational_window_value先求各行offset并计算实际目标，只有被引用的value表达式才求值，NULL offset不触发未访问值的除零错误；default仍只在各当前行越界时求值。新增真实SQL对此取证。Lag::reset释放完整rows容量；build对requires_partition回退既有缓冲路径，因此两种计划请求均正确，不宣称LAG可以提前流式输出。

Ready当前完成：cargo fmt --all退出0；kernel pg09_15 2/2、windows --lib 21/21（crate小且共享帧/状态接口变化影响全crate）、parser pg09_15 1/1（含PG表达式和MySQL原数字/参数形式及非法表达式拒绝）、query_expression_rule_coverage 1/1；parsergen generate/check均0；make parser/parser_yacc/parser_fmt均0，guarded make parser_unit_test退出0、14个成功Go包，make lint退出0，git diff --check退出0。Go测试按failpoint前置enable、EXIT/INT/TERM disable；disable末尾refcount0、无binding残留；工具递归遍历正在重建的target报一条rlib lstat不存在，非Go源恢复失败。尚在完成最终SQL补充与批次12–14相邻回归，不据此提前完成。


## 最终完成证据（2026-10-31）

已完成。用户继续授权已解除最初外部接口阻塞；实现只补本任务局部签名与执行链，未扩建通用类型系统、未新建生产文件、未改外部依赖和plan.md。独立测试保留完整目标行为，没有以固定偏移、编译成功或零测试替代验收。

最终15组SQL夹具验证Q与扩展协议Parse/statement Describe/Bind/portal Describe/Execute/Sync，固定别名r、OID23、长度4、typmod-1，零行不漂移。offset/default相对当前行，0自身、负数双向、NULL offset返回NULL、越界才用default、存在NULL不替换、窄帧不影响分区访问；两分区、空/单行/全NULL、i32两端、算术表达式、未知NULL value+int4 default、未引用值不求值均通过。offset与default的直接未知参数分别24次合计重绑（两个SQL各2种参数格式×2种结果格式×3次NULL/非NULL），声明参数身份23、结果格式四组合、Execute不重发描述均有断言。非法decimal/bool/text default与显式text参数在Parse拒绝42883，原始S/V/C/M输出，D/H/P未发送；Sync后ReadyForQuery I，SELECT1恢复。双PG与同Server MySQL原生last_value/UNION ALL交错通过且监听端口关闭后拒绝重连。kernel两个计划模式请求对任意偏移都使用既有缓冲路径；跨Chunk和close后内存归零通过。

Profile：WIP运行期红/绿后Ready按Rust/Parser影响选择；verify-profile技能不存在，按根AGENTS/testing-flow执行，不虚构技能结果。所有Cargo和parsergen产物命令继承动态槽位1，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。以下为实际命令，重复运行时列最终退出码/有效计数；编译错误0项。

| 完整命令 | 退出码/有效计数 |
| --- | --- |
| cargo fmt --all | 0，修改后、最终验证前运行 |
| rustfmt --edition 2024 pkg/parser/parser_actions/query.rs | 0 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_15 -- --nocapture | 最初101（1有效运行期失败），最终0、2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_15 -- --nocapture | 最初101（1有效运行期失败、代表SQL42601），最终0、2/2；最终格式化后再次2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib -- --nocapture | 0、21/21，覆盖共享帧接口变化与既有Go对齐/内存/窗口回归 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_15 -- --nocapture | 0、1/1 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib query_expression_rule_coverage -- --nocapture | 0、1/1；query245→244仅对应两offset产生式归并一条Expression |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_12 -- --nocapture | 0、2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture | 0、2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_14 -- --nocapture | 0、2/2 |
| "$CARGO_TARGET_DIR/release/astersql-parsergen" generate | 0 |
| "$CARGO_TARGET_DIR/release/astersql-parsergen" check | 0、无生成漂移 |
| make parser | 0 |
| make parser_yacc | 0、Go生成物无差异 |
| make parser_fmt | 0 |
| make failpoint-enable | 0 |
| gofmt -w pkg/parser/ast/binding__failpoint_binding__.go | 0，只格式化临时binding，最终自动恢复 |
| make parser_unit_test | 0、14个Go测试包成功；子测试总数未采集 |
| make failpoint-disable | 0、refcount0，EXIT/INT/TERM trap；无binding残留 |
| make lint | 0 |
| git diff --check | 0 |
| git diff --cached --check | 0 |

没有执行make bazel_prepare（无本任务Go/import/Bazel/外部依赖触发）、bazel_lint_changed、RealTiKV或workspace广扫。只复用适用且仍覆盖当前修改的验证。既有全局admin规则库存断言属任务13记录的范围外问题，本任务未改admin、不重跑无关全parser库存扫；本任务query规则覆盖及生成物check均通过。

正确性由上述值/行数/NULL/描述/错误恢复断言约束；兼容性由PG批次12–14、MySQL合法对照及解析器原MySQL offset形式约束。性能沿现有逐行重建分区的会话入口，LAG/LEAD任意偏移需要缓存分区，未压测和不承诺吞吐。未验证PostgreSQL18.6真实参考客户端及精确错误消息/阶段差分（任务18责任），未启动外部数据库、RealTiKV或用户服务，不将局部结果推断为完整窗口库支持。

提交准备遇到11:22遗留的.git/index.lock；git add退出128。通过ps无git进程、lsof无打开持有者核实后，保留原文件为.git/index.lock.task15-stale-20261031，不删除未知数据；重试git add成功。最终只暂存本任务19个代码/测试/说明文件及本任务记录；其余用户/并行任务修改保留。Cargo子进程全部结束后核验自己的owner标识，删除自己的owner并rmdir释放1号锁，保留构建缓存；测试服务资源已清理。提交完整SHA在最终回复报告，完成交接后删除本编号文件（不另提交删除）。

# 任务 29：SRF暂停恢复与资源释放

批次：【批次 29】；调度依赖：批次28（共享源码/注册与验证资源串行）；行为依赖：28。

状态：已完成，待回归

目的：同一portal分段Execute不丢不重，暂停恢复保持外行及各SRF游标；LIMIT早停、耗尽、错误、取消、Close、重扫均释放/重置状态。

来源任务：大计划9；路线图第4章 `SRF生命周期`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：16号现有portal生命周期、18号取消资源接口；缺失只记录对应依赖。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

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


同一portal分段Execute不丢不重，暂停恢复保持外行及各SRF游标；LIMIT早停、耗尽、错误、取消、Close、重扫均释放/重置状态。

代表SQL（其他边界使用同型最小夹具）：

    SELECT generate_series(1::int4,7::int4) AS v

每次Execute限2行直到7行全部返回；穿插两个portal与两连接，不重发RowDescription。用现有资源计数断言清理，不新增通用取消框架。

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


在 `pkg/executor/physical_plan_runtime_test.rs` 待新增 `pg09_29_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_29_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_29 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_29 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次29执行；行为依赖为28，调度依赖为批次28。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_29 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_29 -- --nocapture
    make lint
    git diff --check

新增/修改解析器时还须按根矩阵运行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 并审查生成差异；不修改解析器不无故执行这些命令。Bazel准备按根完整触发清单判定；只改Rust且无Bazel/Go/依赖触发时不运行 `make bazel_prepare`。禁止默认运行 `make bazel_lint_changed`。如需Go测试，先依testing-flow判定failpoint并在退出禁用；Go既有回归失败记录后按用户规则不阻塞本任务，新增回归不得忽略。

检查当前改动实际影响的原有相邻测试并记录精确命令；不运行无依据全包/昂贵广扫。涉及真实存储才依 `docs/rust/rust-postgresql-tikv-runbook.md` 与testing-flow启动专属后台playground、验证并清理。本计划不授权在拆分阶段运行上述命令。

风险：共享描述符/执行状态改动可能影响MySQL；排序/帧/行扩展可能增加内存或错误地改变行数；先保持单阶段正确并验证资源释放，不据小夹具给出吞吐承诺。恢复只撤销自己明确有问题的变更，不删除其他任务/用户文件，不移除未知锁或服务。

## 完成与交接


只使用状态：未开始、已完成、已阻塞、已完成，待回归。完整行为和所需适用验证通过后标记已完成；有实际代码/测试/配置/文档变更使用 `$git-commit` 技能只提交本任务。只有编号文件状态或删除时无需单独提交。最终报告路径/符号、生产修改依据、验证profile、实际槽位/绝对CARGO_TARGET_DIR、完整命令/退出码/有效计数、正确性/兼容性/性能风险、未验证项及完整提交SHA。

验证因无关环境缺失无法运行，可记录已完成，待回归并保留本文件；不能把仍缺失的目标行为或依赖接口标为完成。阻塞只写本文件及已做的检查，不扩大范围。完成证据已交接且提交妥当后按执行契约删除本编号文件，`plan.md`只读，父大计划和覆盖表保留。删除文件本身不是完成证据。

## Progress


- [x] 确认批次28及真实入口；用户授权突破后补齐必要结果所有权和恢复接线，原静态阻塞已转入实施。
- [x] 记录运行期失败：executor/server pg09_29各1项退出101，均提前执行后续外行。
- [x] 完成局部结果所有权、SRF按需推进和portal取消/恢复接线；隔离executor/server pg09_29各1项通过。
- [x] Ready适用检查在隔离环境全部通过，原工作区复验遇外部缓存/锁目录消失；diff自审完成，按已完成，待回归提交并保留本文件。

## Surprises & Discoveries


2026-11-01已完成两轮静态前置接口复核；通用ProtocolResultSet存在，但底层ConcreteRecordSet仅承载物化行，不能据此宣称SRF游标已可跨Execute存活。详细证据和替代方案见下方恢复复核记录；本任务尚无运行结果。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


2026-11-01：用户授权突破后已实现SRF活结果所有权、暂停恢复、早停和错误/取消/Close清理，独立executor与真实TCP测试通过，隔离Ready及相邻PG/MySQL回归通过。原工作区最终复验被外部构建缓存与锁目录消失中断，状态为已完成，待回归；本文件保留供共享工作区复验，具体命令与退出码见末节。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-11-01 前置接口复核（已阻塞）

当前完整HEAD：728cf6894292e3d7f18b482dea005a6b81ecf0bc。已阅读本任务、只读plan.md、skills/do-task-plans/SKILL.md、skills/rustcodegraph/SKILL.md、根AGENTS.md、pkg/server/AGENTS.md、PLANS.md与docs/agents/testing-flow.md。目标executor/session/server未发现doc.go；tidb-verify-profile仍缺失。未修改只读plan.md，也未修改其他任务/生产代码/测试。

批次28依赖：已确认提交728cf6894292e3d7f18b482dea005a6b81ecf0bc及其独立pg09_28_execution、pg09_28_sql和双监听测试存在。该提交内编号任务的最终交接记录executor 1/server 2有效通过、邻接验证与make lint退出0。此为上游交付证据，本轮没有重新运行，不冒充本轮验证。

阻塞为任务明确要求先具备的大计划16 portal生命周期及大计划18取消资源接口尚未形成适用于SRF的可恢复执行/资源验收闭环：

1. pkg/server/pg_extended.rs:968-985的Portal仅保存完整编码messages和offset；1869-1992的Execute先执行并完整encode_formats_interval，1951-1954已finish生命周期，随后才按limit切分消息。没有持有ProtocolResultSet或执行器游标。可以发PortalSuspended，但这不能证明外行/各SRF游标跨Execute存活。不能以7行结果缓存分页替代本任务验收。
2. 已明确区分现有通用接口与缺失接线：conn.rs:619-625的ProtocolResultSet以及1017-1024的execute_prepared_streaming存在，runtime.rs:1133-1153及2256-2275有真实工作线程接线。不能声称整个流式接口不存在；缺的是PG portal对其生命周期的所有权与分页消费契约。现有SRF source.rs:455-554在局部循环耗尽叶执行器后Close，2352-2384对每个外行收集所有SRF结果再展开，2386-2397才应用LIMIT。因此尚不能沿现有portal接口保留活的外行/多SRF游标；解决需要先交付可复用的portal执行生命周期接口，而非扩大本任务重做协议层。
3. pg_conn.rs:791-842有真实活动取消路由与命令结束重置；但56-61的resource_counts只统计active/socket/worker，不能证明SRF游标、源rows、portal缓存已释放。大计划18子任务16-物化峰值与首行测量.md和30-物化阶段取消与断连回收.md仍要求补计费/回收接口及独立证据；证据/4-验收.md、5-验收.md、16-验收.md、30-验收.md均未找到，对应pg_lifecycle_4/5/16/30及pg16_019/020/021/022/024/071测试搜索未命中。未用未开始状态单独推断缺口，已核对上述真实消费者。缺少适用资源计数与验收证据是实际前置缺口，不是无关环境不能编译。

遵从本任务“实际外部前置……缺失只记录对应依赖”及执行技能“遇到阻碍（缺少依赖、测试失败、指令不明确）”停止规则，本轮仅更新此编号文件为已阻塞。没有新增测试、生产修改或临时框架；没有运行Cargo/调用Cargo脚本，没有领取槽位，CARGO_TARGET_DIR不适用，无锁/缓存/进程需要清理。尚无本任务运行期红灯或通过证据，不能标记已完成或已完成，待回归。不提交仅状态/阻塞记录更新，不删除本任务。

核心只读检查命令（均从仓库根执行）：

    ~/.rustcodegraph/bin/rustcodegraph status
    ~/.rustcodegraph/bin/rustcodegraph explore 'PgService portal suspend resume cancel resource ProjectSet generate_series'
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_extended.rs --offset 950 --limit 100
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_extended.rs --offset 1770 --limit 250
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_conn.rs --offset 30 --limit 55
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_conn.rs --offset 785 --limit 58
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/conn.rs --offset 610 --limit 24
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/conn.rs --offset 1000 --limit 65
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/runtime.rs --offset 1105 --limit 80
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/runtime.rs --offset 2225 --limit 100
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/source.rs --offset 450 --limit 130
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/source.rs --offset 2335 --limit 100
    git show --stat 728cf68942
    git show 728cf68942:.plans/2026-10-29-postgresql18-聚合窗口与集合返回/28-多SRF协调与非法位置门禁.md
    git rev-parse HEAD

以上命令退出0；status报告11849个文件，explore泛名结果不精确，使用上述node精确复核。如下缺失检查退出1（预期未命中），有效行为测试数0：

    rg --files .plans/2026-10-29-postgresql18-活动取消关闭与资源预算 | rg '证据/(4|5|16|30)-验收.md'
    rg -n 'pg_lifecycle_(4|5|16|30)|pg16_0(19|20|21|22|24|71)' pkg/server/pg_client_integration_test.rs pkg/server/pg_extended_test.rs

验证profile：仅前置只读分析和任务状态记录，不适用代码WIP/Ready检查；未运行cargo fmt/test、make lint、Go/Bazel/parser/RealTiKV、PG18.6参考或客户端差分。正确性风险仍是未实现的游标恢复/早停与资源释放；兼容性及性能本轮未验证，没有生产行为变更。建议先由大计划16/18提供可供PG portal持有、恢复、关闭的真实结果流及源/缓存资源计数和取消回收证据，再恢复本任务TDD；不要求吞并其全部子计划。


## 2026-11-01 阻塞恢复复核

本轮依据明确恢复指令重新检查当前共享工作区，继续使用do-task-plans与rustcodegraph技能。HEAD仍为728cf6894292e3d7f18b482dea005a6b81ecf0bc，目标共享文件有其他任务既有改动，未覆盖或纳入本任务。本轮只更新本编号文件，不提交、不删除，plan.md继续只读。

对上一轮结论作两点限定：ProtocolResultSet、WorkerResults、SQLKiller以及叶执行器Open/Next/Close均已存在，不应描述为整个执行/取消系统缺失；缺少新编号测试或上游计划仍标未开始，也不独立构成阻塞。现有证据若适用即可复用，不要求完成大计划16/18整个子计划，更不要求引入扫描/锁/全系统内存计费框架。

### 继续沿真实消费者核对

ConcreteRecordSet（pkg/session/runtime/session.rs:1939-2039）只有columns、result_fields、VecDeque行缓冲、closed及延迟store_read；next_row只完成store_read后pop_front，close仅清缓冲，没有执行器或延迟行源所有权入口。query.rs::execute_relational_query_record_set在调用execute_insert_select_node并完成全部行后才new_with_fields。source.rs中的IntegerSeries/Int4Unnest在构造ConcreteRecordSet前已在局部循环耗尽并Close。故streaming名字并不表示活的SRF源。

server/runtime.rs::execute_prepared_on_session（1629-1666）在streaming分支前先调用execute_protocol_statement；streaming只选择注册这个已经返回的ConcreteRecordSet或将其转换为rows。protocol_result.rs::WorkerResults::register的source参数固定为ConcreteRecordSet，CanonicalResultSet::Next（133-151）逐行读取同一行缓冲，其Finish（168-174）注释明确说明没有executor owner且实现只返回Ok。ResultHandle::close/Drop确有关闭真实工作线程结果的能力，WorkerResults::results是HashMap，可以保存多个结果，不是“单句柄数量限制”；但它们保存的是现有物化结果，不能把已经关闭的SRF复活。

PG Execute（pg_conn.rs:406-417）只把首次execute_prepared_statement放进with_query；Extended::process的后续Execute只读取portal.messages（pg_extended.rs:1972-1992），未调用该闭包。改成流式后，恢复时的next_chunk/current_row/advance也须在每个活动Execute中纳入with_query的执行/取消代次，而不是只包首次打开。现有叶Next本身没有SQLKiller引用。仅将execute_prepared_statement改为execute_prepared_streaming会在PG encoder仍只读result.rows（pg_result.rs:1328）时遗漏所有返回行，不能直接替换。可以通过局部分页编码解决这一传输问题，但仍不能解决前述执行源已耗尽的根本边界。

另核对session现有执行收尾：dispatch.rs:4032-4040和4629-4648直接truncate record_set.rows并使用rows.len统计；query.rs::execute_full_relational_select在同步调用后恢复snapshot_read_ts（5362），PgCastTable作用域亦于同步返回时退出。把未完成的通用查询闭包塞进recordset不能仅改一处next_row：需明确活执行状态和固定绑定上下文、select limit及声明周期由哪个结果所有者负责。这里是对通用延迟查询替代的边界检查，不主张SRF整数叶本身需要新增存储子系统。

### 安全替代方案逐项判断

- 直接复用ProtocolResultSet并改PG分页编码：真实接口和多结果句柄存在，这部分技术上可局部接线；但源仍是物化VecDeque，无法满足“暂停恢复保持外行及各SRF游标”，不是充分方案。
- 复用既有叶执行器并只补Close/重扫断言：叶已具备跨Chunk状态与Open重置，可留作本任务后续TDD；SQL入口在局部耗尽后关闭，叶测试不替代同portal跨Execute和取消证据。单独改叶无法建立缺失的所有权边界，不提交无效部分实现。
- 在source.rs下推LIMIT、或只加SQLKiller检查：可能分别改善无排序简单路径的早停和取消响应，但不能保留跨Execute的多SRF/外行状态，不能据此交付本行为闭环；不以可做的孤立改动掩盖未满足的主契约。
- 保留所有展开行或编码消息，消费时清理前缀：可以减少部分缓存并保持值序，但仍在首个Execute前耗尽SRF；取消/早停不能中断已完成的源，不能作为游标恢复与资源释放实现。
- 每次Execute重跑SQL并追加LIMIT/OFFSET，或在server识别generate_series后直接生成结果：前者重新求值与重新读取外行，不能保持原游标、错误阶段和语句快照；后者违反共享执行器及禁止server拼接SQL结果的契约，均排除。
- 新增活SRF所有者作为ConcreteRecordSet延迟生产者，再把它贯通PG portal：这是技术上可实现的方向，但当前不是已有可挂接接口。它需新增结果源承载、跨Execute活动/收尾所有权及相关通用结果收尾适配。任务明确把既有portal生命周期和取消资源接口列为16/18前置并要求“缺失只记录对应依赖”；本轮不借恢复授权补建这一结果集生命周期边界，也不另建SRF专用协议/执行器绕过它。

因此结论维持已阻塞，原因为授权前置的活结果集所有权/恢复取消边界未提供，属于行为/接口缺口，而非无关回归失败。资源计数不足是验收缺口的补充，单凭计数或新测试缺失不作为独立技术阻塞。没有把本任务的多SRF协调、整数迭代和Open/Close逻辑推给上游，也不要求上游替本任务完成SRF执行状态机。

### 恢复所需最小外部交付

需要可复用、可由PG portal持有的真实结果源所有权边界：允许现有ExecExecutor/投影迭代器持续到下一次Execute；保留固定字段与绑定上下文，Close/错误/事务结束/会话断开只关闭属于该portal的源，支持两个结果交错；消费入口在每次Execute受既有with_query/SQLKiller保护。可以继续沿ProtocolResultSet和现有worker句柄实现，不要求新框架或新公共命名。由16/18明确并提供适用接口后，本任务负责挂接现有SRF叶、外行和多SRF游标、LIMIT及清理状态并完成pg09_29测试。

证据只需与本任务相关的真实结果流打开/暂停/恢复/关闭、活动取消后Sync恢复及资源收敛；使用已有边界事件或窄观测即可，不要求完整资源预算体系。应能区分“连接关闭”与“结果源已关闭/游标和暂存状态已释放”，并证明A的结束/取消不关闭B。缺少新的编号测试文件可以用有效既有测试补足；单纯提供文件名、编译成功、PING或缓存分页不充分。

### 本轮完整检查与退出情况

均从仓库根执行，未运行Cargo或调用Cargo的脚本。以下rustcodegraph单次工具命令输出成功；其中status本次11849文件，后续按文件node定位源码，泛名搜索不作为接口缺失证明：

    ~/.rustcodegraph/bin/rustcodegraph status
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/protocol_result.rs --offset 190 --limit 200
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_conn.rs --offset 365 --limit 85
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/runtime.rs --offset 1580 --limit 155
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/session.rs --offset 1880 --limit 230
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/query.rs --offset 7740 --limit 90
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/protocol_result.rs --offset 1 --limit 192
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/dispatch.rs --offset 3930 --limit 125
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/dispatch.rs --offset 4580 --limit 90
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/conn.rs --offset 470 --limit 145
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_result.rs --offset 1 --limit 130
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/query.rs --offset 5300 --limit 45
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_extended.rs --offset 1057 --limit 145
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/runtime.rs --offset 1804 --limit 40
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/control.rs --offset 1695 --limit 55
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/query.rs --offset 5340 --limit 62
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/executor/physical_plan_runtime.rs --offset 1100 --limit 115
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/executor/physical_plan_runtime.rs --offset 1208 --limit 70
    ~/.rustcodegraph/bin/rustcodegraph node --file pkg/server/pg_result.rs --offset 1300 --limit 95

源码导航批处理整体退出码：status/WorkerResults/pg_conn定位组0；runtime/recordset入口导航组0；ConcreteRecordSet/recordset构造/CanonicalResultSet定位组1（最后的streaming模式rg在session.rs/dispatch.rs/query.rs未命中，node读取成功）；dispatch/取消与观测检查组0；ResponseLifecycle/PG encoder/执行作用域组0；portal/响应收尾/snapshot组0；叶执行器/encoder组0。未把rg未命中当行为证据。git status --short -- pkg/server pkg/session/runtime pkg/executor及git log -4 --oneline退出0，确认仍有其他任务工作，未修改它们。

本轮仅状态文档变更，无代码验证profile；不适用WIP/Ready构建检查。有效测试数0，未取得新运行期失败/通过，不能标完成或已完成，待回归。无Cargo槽位、无CARGO_TARGET_DIR、无锁和测试进程。未验证项仍包括所有pg09_29行为、PG18.6/真实客户端、兼容性和性能。没有新增提交SHA，已有依赖SHA不冒充本任务提交。

## 2026-11-01 用户授权突破后的实施决策

用户明确要求“突破啊”，本轮继续实现必要局部接线，不再等待16/18新增接口。复用ProtocolResultSet/WorkerResults和SQLKiller，在ConcreteRecordSet增加仅承载SRF活状态的可选所有者；PG portal持有现有结果句柄，每次Execute在既有with_query下消费并按原codec编码。不会新增通用执行框架。source.rs新增局部PgSrfRows/PgSrfLeaf，职责分别为保存外行及同层各SRF游标、以已有ExecExecutor产生单行并确定性Close；注册点为query.rs::execute_relational_query_record_set，ConcreteRecordSet::next_row/close负责消费/释放。server pg_extended.rs/pg_conn.rs为不可缺少的portal恢复和取消接线，protocol_result.rs复用现有worker句柄。现有阻塞记录为历史复核，待本轮验证后更新结论。

先增加pg09_29_execution和pg09_29_sql，使用第一外行正常、第二外行零步长的夹具证明首次读取不提前求值后续外行；原实现应在返回首行之前报错，新实现应先返回首行并在推进至第二外行时报22023。后续覆盖分段Execute、LIMIT、取消与Close等。


实施进展：红灯包装器/tmp/pg0929-cargo.sh动态取得槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；cargo fmt --all退出0，executor/server pg09_29各运行1项失败（101），均为首外行返回前提前求值第二外行零步长。后续编译纠正发现TestRecordSet: Send约束，保留该约束和原有跨线程测试语义；ConcreteRecordSet仅在worker持有活状态，新增BufferedRecordSet为原缓冲结果的可移动适配，dispatch.rs和testkit/mockstore.rs转换点属于不可缺少的接口接线，未添加unsafe或新线程。首轮编译错误不计行为证据。新生产类型及注册点已据此补充，其他节点仍复用现有ProtocolResultSet和ExecExecutor。

生产依据补充：source.rs::open_pg_srf_executor复用原构造器，PgSrfRows保存当前外行/每个SRF迭代和LIMIT窗口；query.rs局部选择非阻塞SRF路径；session.rs结果所有权及close/error/耗尽清理，dispatch.rs让SelectLimit也作用于活结果并保持TestSession缓冲返回；testkit/mockstore.rs仅调整原Send转换点；server/runtime.rs为活SRF限制预取到一行防止Execute跨外行提前求值；protocol_result.rs在读取错误时保留既有SessionError到PG SQLSTATE转换；pg_extended.rs持有既有worker句柄并按每次Execute需求编码，Close/失败/耗尽释放；pg_conn.rs把恢复Execute纳入既有活动取消保护，新增窄测试观察器只读取已有active/probe状态。

共享解析器并行修改暂时造成token::compare/output/views未生成及parser_actions/mview.rs::Errorf参数缺失，Cargo退出101，非本任务行为证据。已确认其他任务的parsergen进程在target/rust-slot-3运行，未覆盖其文件或清理其锁，继续本任务静态审查和TCP覆盖后重试。中间验证动态领取槽位2和槽位1，包装器退出清理自己的锁并保留缓存；最终槽位以最终Ready记录为准。


补充边界决策：不可将活AST/ExecExecutor强制Send。新增BufferedRecordSet只承接原TestRecordSet缓冲表示，无新的执行逻辑；ConcreteRecordSet及PgSrfRows留在已有线程。非阻塞投影/单FROM/既有CROSS/LEFT LATERAL使用活状态；排序、聚合、窗口、DISTINCT、WITH及标量子查询等保持原阻塞路径，不为本任务重做算子系统，原SRF物化循环补既有SQLKiller检查。新观察器仅读取已有active状态/BoundaryProbe事件和recordset是否持有活状态，不引入资源预算框架。


隔离验证worktree位于/Users/xiangmin/.codex/worktrees/srf29-verify/astersql-tidb，基于当时共同HEAD f9aefb0070b6892e70353ad40212a6a921bae506，仅带本任务生产/测试差异。新workspace已尝试make bazel_prepare，退出2，原因本机无bazel；未产生Bazel元数据，Rust行为验证不以此伪装通过。Cargo仍动态领取原仓库共享槽位，未建新的构建缓存。隔离executor pg09_29已通过，有效1项，包含后续外行延迟错误、LIMIT提前释放、1029行两轮重启、多SRF补NULL、LEFT LATERAL、NULL数组关闭、取消与SELECT1恢复；对应日志/tmp/pg0929-isolated-executor.log。当前TCP验证尚在进行，不提前宣称完成。原工作区解析器token和Errorf并行修复现已出现，最终将尝试在原工作区运行Ready，隔离worktree验证后归档。


Ready进展：/tmp/pg0929-ready.sh在隔离worktree执行，动态槽位1，PID44445，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；fmt0、executor pg09_29 1通过、server pg09_29 1通过、executor pg09_2 15通过、server pg_query_test::pg09_2 11通过、server conn_stmt_test::port_592082dee7 5通过、server pg_extended_test::parse_bind_execute_sync 1通过。MySQL会话定向组尚在执行，不预填通过。过滤器重叠，不能将这些计数相加当唯一测试数。原与隔离环境11个Rust代码/测试文件已逐字节对照一致（退出0），仅外部并行解析器改动未带入隔离环境。

原工作区Ready首次重试fmt退出0，executor构建退出101、有效行为测试0，来自并行mview.rs引用CompareMaterializedViewStmt和六个ShowStmtType变体与AST中间状态不同步，7个编译错误；日志/tmp/pg0929-shared-executor-failure.log。本任务SRF行为不存在待实现的外部依赖；此失败不冒充本任务行为红灯，也不覆盖隔离验证结论。已观察这些AST定义随后出现，故以新证据启动原工作区最终定向复验（/tmp/pg0929-shared-check.sh），动态槽位3、PID50601，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3，仍在执行，结果稍后记录。未修改解析器文件。


## 最终验证证据（2026-11-01）

本任务原先记录的活结果所有权缺口已在本任务内通过必要局部接线解决，不再将16/18作为缺失行为的阻塞。实现范围为既有非阻塞直接SRF投影、单函数FROM以及已有CROSS/LEFT LATERAL形态的按需推进，保持同层最长补NULL、固定字段身份和外行重启；已有排序/分组/窗口/DISTINCT/WITH/标量子查询等路径继续物化，SRF物化循环补既有取消检查，不重建算子系统。没有Go/解析器/依赖/feature/Bazel生产修改。所有生产修改均已在上述路径/符号依据中映射；同步9份既有.rs.md，无新增生产源文件。

WIP红灯：/tmp/pg0929-cargo.sh在动态槽位1执行cargo fmt --all（0），同过滤器executor/server pg09_29各运行1项并退出101：首行前提前求值后续外行零步长。初版server测试为真实会话，后扩展为同SQL真实TCP场景；没有将编译错误当红灯。中间编译错误和外部解析器中间状态失败已单独记录。

Ready通过环境：/Users/xiangmin/.codex/worktrees/srf29-verify/astersql-tidb，基线与原仓库同为f9aefb0070b6892e70353ad40212a6a921bae506，加本任务差异；11个Rust生产/测试文件与交付原工作区逐字节一致（比较退出0）。bash /tmp/pg0929-cargo.sh bash /tmp/pg0929-ready.sh整体退出0；动态槽位1，PID44445，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。所有测试默认features、--locked，无--target-dir覆盖。该阶段结束已清理自己的owner/锁，保留缓存；后续共享目录消失不是本任务执行的清理操作。

| 完整命令（上述隔离仓库根执行） | 退出码 / 有效数 |
| --- | --- |
| cargo fmt --all | 0 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_29 -- --nocapture | 0 / 1通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_29 -- --nocapture | 0 / 1通过 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_2 -- --nocapture | 0 / 15通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg09_2 -- --nocapture | 0 / 11通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib conn_stmt_test::port_592082dee7 -- --nocapture | 0 / 5通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_extended_test::parse_bind_execute_sync -- --nocapture | 0 / 1通过 |
| env -u LDFLAGS cargo test --locked -p astersql-session --lib mysql_relational_query_compat_test -- --nocapture | 0 / 6通过 |
| git diff --check | 0 |

日志为/tmp/pg0929-ready-{fmt,executor,server,executor-neighbors,server-neighbors,mysql-cursors,portal-neighbor,mysql-session,diff}.log。过滤器重叠，不能相加为唯一测试总数。另在原仓库执行make lint，退出0，日志/tmp/pg0929-lint.log；交付文件git diff --check与只读plan.md未修改检查均退出0。verify-profile技能缺失，按根AGENTS/testing-flow选择Ready适用项，不虚称该技能运行。原仓库本任务未触发bazel_prepare；隔离新workspace按规则尝试make bazel_prepare，退出2（无bazel），无元数据变更，不将此称为Bazel验证通过。没有Go测试或failpoint启停、没有parser目标、没有RealTiKV。

实际行为证据：executor确认首行可返回、后续外行零步长才报错；LIMIT1不执行后续错误外行并立即释放活状态；两轮1029行多SRF跨页/NULL补齐与外行重启，LEFT LATERAL无匹配补NULL，NULL数组部分关闭、取消及SELECT1恢复。TCP每次Execute2行、两个portal交错读取7行、穿插第二连接；text/binary结果及参数×结果四组合，20次绑定包含正常/单NULL/全NULL/空范围/恢复；statement/portal描述OID23、typlen4、typmod-1保持固定，Execute只发数据和暂停/完成，不重发描述。后续外行错误为Execute期22023，显式事务ReadyForQuery E、ROLLBACK后I；真实CancelRequest命中已暂停再恢复的SRF为57014，Sync后I及同连接SELECT1成功；另一连接仍可查询。Close的真实worker结果事件增加一次，重绑从1开始。两连接Terminate与服务关闭后resource_counts=(0,0,0)。S/V=ERROR、C和M已采集；D/H/P本夹具未发出，不声称诊断全部等价。日志保留实际错误字段。

原工作区最终复验：bash /tmp/pg0929-cargo.sh bash /tmp/pg0929-shared-check.sh，动态槽位3，PID50601，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3。cargo fmt --all退出0；同executor pg09_29 Cargo命令退出101、行为有效数0，错误为无法写debug/.fingerprint/astersql-executor-598340d746d589db/dep-test-lib-astersql_executor（No such file or directory）。随后只读ls确认该fingerprint目录及target/rust-slot-locks整体不存在，PID50601已退出；包装器清理rmdir也报告自己的锁目录不存在。未推测谁移除了目录，未删除别人的锁、进程或缓存。server同过滤器因脚本遇首错停止，原工作区最终该条未运行；此前隔离同命令已实际通过1项。日志/tmp/pg0929-shared-final-{fmt,executor}.log。这是验证环境故障，不是SRF行为缺失，因此依据用户执行契约标记已完成，待回归并保留本文件。先前共享parser中间状态的101/0测试单独保留在/tmp/pg0929-shared-executor-failure.log，不替代此最终环境故障。

清理：本任务TCP listener与测试会话在测试内关闭，资源断言归零；没有自建数据库进程、用户表或上游依赖临时副本。Cargo包装器等待阶段子进程全部退出再释放自己的锁，SIGINT/TERM包装器传播到独立进程组并wait后再清理；SIGKILL保守留锁。隔离worktree归档调用被应用拒绝（The task's primary checkout cannot be removed），尝试重新attach原目录也被拒绝（不是managed worktree），因此不绕过主checkout保护删除，最终同步为任务提交并保留供审阅。原仓库代码提交不包括其他计划、解析器或sem并行改动。

风险与未验证：正确性限于已测试非阻塞SRF形态与原阻塞路径的定向回归；派生外行/阻塞算子仍可物化，不声称全查询流式。MySQL证据为6个会话、5个游标和既有PG邻接中的双入口用例，不代表全workspace兼容性。活SRF使用一行传输块，内存中保留外行输入，未测吞吐/大规模内存/传输背压或断连预算。不涉及真实存储，未验证RealTiKV；未进行PG18.6/libpq完整差分，留任务30。下一步先在共享构建目录稳定后按本任务两条pg09_29 Cargo命令回归，通过后可只更新状态为已完成并删除本编号文件，无需因仅状态更新另提交。完整本任务提交SHA由最终回复报告，不把基线SHA当任务提交。

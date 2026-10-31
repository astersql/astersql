# 任务 27：单SRF投影行扩展

批次：【批次 27】；调度依赖：批次26（共享源码/注册与验证资源串行）；行为依赖：24、25、26。

状态：已完成

目的：投影单SRF每外行展开至耗尽后进入下一外行；零行集合丢弃该外行，其他标量列按对应外行重复，保留固定输出描述。

来源任务：大计划9；路线图第4章 `SRF-DIRECTION/NULL/ARRAY/CORRELATED投影`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：任务23–26的行源和关联状态，2号投影调用位置。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

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


投影单SRF每外行展开至耗尽后进入下一外行；零行集合丢弃该外行，其他标量列按对应外行重复，保留固定输出描述。

代表SQL（其他边界使用同型最小夹具）：

    SELECT t.id,generate_series(1::int4,t.n,1::int4) AS v FROM (VALUES(1::int4,2::int4),(2,0),(3,NULL::int4)) t(id,n) ORDER BY t.id,v

期望仅(1,1)/(1,2)；重测unnest NULL元素、两整数重载、外层LIMIT。不允许project_row只取第一项。

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


在 `pkg/executor/physical_plan_runtime_test.rs` 待新增 `pg09_27_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_27_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_27 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_27 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次27执行；行为依赖为24、25、26，调度依赖为批次26。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_27 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_27 -- --nocapture
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
- [x] Ready检查、diff自审与提交交接。

## Surprises & Discoveries


实际接口、红绿灯及数组参数分析差异见下方2026-11-01实施记录。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


任务行为和适用Ready检查已完成；具体计数、命令和边界见最终交接。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-11-01 实施记录

已读执行技能、RustCodeGraph技能、PLANS.md、根和server AGENTS、testing-flow；目标包无doc.go。24/25/26前置提交分别为b20f646283dc911df0a26a9f2f29bada16d7b0cf、28697d5ebe09c813eee97dceb294cd837de07671、5635e094130846b3d381d8020098bdf76fc8f18c，实际有BuildInt8Series/BuildInt4Unnest和逐左行重启路径及pg09_24/25/26独立测试。2号已有普通投影Function AST，不缺语法载体。plan.md只读。

生产修改依据（拟实施）：session/runtime/source.rs::execute_insert_select_query_body_with_outer在WHERE后复用既有FROM SRF执行链逐外行展开，然后使用既有投影/排序/LIMIT；query.rs::relational_select_requires_full_query路由无FROM调用及物理表投影到同一关系入口，结果类型绑定和验证复用pg_integer_series_field，保持零行描述。无新增生产类型/文件或外部依赖。

首次动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，PID19201；/tmp/pg0927-cargo.sh使用mkdir原子领取、记录owner并trap释放。cargo fmt --all退出0；executor同过滤器红灯退出101，有效1测试在普通会话返回unsupported scalar function generate_series，是运行期行为失败。server红灯待完成。


server红灯同过滤器退出101，有效1项失败，SQLSTATE42883/unsupported scalar function generate_series。初版实现曾有SelectStmt无Clone和未初始化编译错误，已改为仅SRF查询克隆FieldList；不把编译错误算行为证据。中间扩展协议测试曾重复Parse同名statement出现42P05，已在重载切换时显式Close；不变更产品逻辑来适配测试。

同过滤器executor/server各1项已通过，中间日志/tmp/pg0927-{executor,server}.log。server正常夹具含代表相关投影、正反向、INT8超2^53及上界、零行/NULL严格、空外行、全NULL外行、UNNEST含NULL元素/重复元素、外层LIMIT/OFFSET和2052行跨Chunk重启。40次整数扩展绑定（两重载×四格式×5参数/步长情形）及16次数组绑定（四格式×4次正常/空/NULL/恢复）逐行检查字节、固定OID23或20/长度4或8/typmod-1、参数Describe、portal Describe唯一T和ReadyForQuery I。22023检查S,V,C,M，D/H/P无字段；扩展Sync后SELECT1恢复，事务失败E后ROLLBACK/I再SELECT1。

生产实际改动仅query.rs/source.rs及既有.rs.md：source在WHERE后、ORDER/LIMIT前复用原FunctionSource调用，每行完整消费现有执行器；内部NUL前缀列不会进入通配符输出，普通列名使用原始投影。仅SRF查询克隆FieldList；无新增AST/物理类型、解析器/Go/Bazel/manifest/依赖改动。LIMIT0在签名绑定后不打开SRF。

已执行bash /tmp/pg0927-reference.sh，退出0：PG/psql18.6、aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位、SQL_ASCII、Asia/Shanghai；独立mktemp数据目录、仅Unix socket、trap停止删除。代表相关序列仅(1,1)/(1,2)/(4,1)，数组NULL元素保留、空/NULL数组丢行，超2^53及上界精确，LIMIT/OFFSET同预期，WHEREfalse屏蔽零步长，非空零步长22023/int.c:1558。完整输出/tmp/pg0927-reference.log。不是任务30完整客户端差分。

Ready阶段动态领取槽位1，PID29508，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；bash /tmp/pg0927-cargo.sh bash /tmp/pg0927-ready.sh执行中。另一次make lint已退出0，最终仍按Ready重跑。无Go/Bazel/解析器/真实存储变化，不触发Bazel准备、parser Make目标、Go failpoints或RealTiKV。缺失verify-profile技能不宣称已运行。


最终补充夹具暴露直接UNNEST(ARRAY[NULL,NULL]::int4[])报0A000/ARRAY analysis requires a direct SELECT constructor；首次Ready server同过滤器实际1失败，不能算已完成。必要局部接线新增pg_array.rs::analyze：仅把直接投影UNNEST参数加入原FROM数组分析根，复用现有显式int4[]定型；独立pg_array_test.rs::pg09_27_array_projection_analysis覆盖空/全NULL/括号以及WHERE仍拒绝，并同步既有.rs.md。不扩大数组类型库。

首次Ready executor同过滤器1通过；executor pg09_2共13通过。server pg09_2选择过宽：8通过、2失败，除本次全NULL数组缺口外，误选任务22客户端测试缺少PG18_REFERENCE_DSN；后续精确过滤pg_query_test::pg09_2，不启动或修复任务22整套客户端平台。


## 2026-11-01 最终完成证据

第二次Ready包装命令bash /tmp/pg0927-cargo.sh bash /tmp/pg0927-ready.sh退出0；动态原子领取槽位1，持有PID36021，绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。本任务所有Cargo阶段均实际领取槽位1，所有子进程结束后trap删除自己的owner及锁目录，保留缓存；参考进程及临时数据目录已清理。默认features，未增加依赖或本地覆盖。

| 完整验证命令 | 退出码 / 有效测试数 |
| --- | --- |
| cargo fmt --all | 0 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_27 -- --nocapture | 0 / 1通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_27 -- --nocapture | 0 / 1通过，含56次扩展绑定 |
| env -u LDFLAGS cargo test --locked -p astersql-executor --lib pg09_2 -- --nocapture | 0 / 13通过 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_query_test::pg09_2 -- --nocapture | 0 / 8通过 |
| env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_array_test -- --nocapture | 0 / 45通过，含pg09_27_array_projection_analysis |
| env -u LDFLAGS cargo test --locked -p astersql-session --lib mysql_relational_query_compat_test -- --nocapture | 0 / 6通过 |
| make lint | 0 |
| git diff --check | 0 |
| bash /tmp/pg0927-reference.sh | 0 / PG18.6独立同型夹具观测 |
| git diff --quiet -- .plans/2026-10-29-postgresql18-聚合窗口与集合返回/plan.md | 0 |

不同过滤器有重复覆盖，不能相加为唯一测试数。日志/tmp/pg0927-ready-{fmt,executor,server,executor-neighbors,server-neighbors,array-neighbors,mysql-neighbors,lint,diff}.log；第一次Ready（含数组缺口和无关任务22 DSN失败）日志保留为/tmp/pg0927-first-ready-*.log，初始TDD红灯/tmp/pg0927-{executor,server}-red.log。补充全NULL数组后同过滤器已通过，LIMIT0不打开SRF、全NULL数组发出两行NULL、空数组零行、INT8负边界固定身份均实测。

Profile：WIP用于红绿灯与局部修复，Ready用于交付；verify-profile技能路径缺失，依根AGENTS与testing-flow执行适用检查，不宣称技能已运行。未改Go/Bazel/解析器/依赖，不触发bazel_prepare或parser生成；不涉及真实存储，不运行RealTiKV。

变更仅三个生产文件query.rs/source.rs/pg_array.rs、对应三份既有.rs.md、三个独立测试文件physical_plan_runtime_test.rs/pg_query_test.rs/pg_array_test.rs和本编号记录。各生产依据见实施记录，所有可用Rust文件已有AsterSQL版权，保留PingCAP原注释。diff自审通过，未暂存其他计划删除、expression/hint/planner/server handler/session runtime.rs/compression等既有改动，plan.md完全未改。

正确性证据限于单一直接投影SRF及既有重载，不将多SRF、嵌套SRF/聚合或窗口混用、portal暂停恢复归入完成范围。兼容性通过上述MySQL和既有PG邻测约束，未跑全套MySQL/全workspace/清洁checkout。现有关系入口仍物化全部输出，内存随结果量增长，未测吞吐、真实TiKV或大结果取消压力。PG18.6仅局部实际观测；任务22客户端差分曾因无PG18_REFERENCE_DSN失败，不将其伪装为通过或扩建本任务范围。完整SRF客户端矩阵由任务30承担。

下一步按原批次继续28多SRF协调与非法位置门禁。完成记录随本任务提交，完整SHA在最终回复交接；提交妥当后按执行契约删除本编号文件，保留只读plan.md。

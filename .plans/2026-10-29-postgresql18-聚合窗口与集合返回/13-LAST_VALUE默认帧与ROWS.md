# 任务 13：LAST_VALUE默认帧与ROWS

批次：【批次 13】；调度依赖：批次12（共享源码/注册与验证资源串行）；行为依赖：12。

状态：已完成，待回归

目的：last_value(int4) 返回 int4；默认 RANGE 截至最后 peer，10/10/20 得 10/10/20；显式全分区 ROWS 得 20/20/20；单行空帧得 NULL 但保留行。

来源任务：大计划9；路线图第4章 `WIN-CALL-FRAME/EMPTY-SINGLE/NULL`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：任务12的窗口计划和固定输出身份。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

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


last_value(int4) 返回 int4；默认 RANGE 截至最后 peer，10/10/20 得 10/10/20；显式全分区 ROWS 得 20/20/20；单行空帧得 NULL 但保留行。

代表SQL（其他边界使用同型最小夹具）：

    SELECT last_value(x) OVER(ORDER BY x) FROM (VALUES(10::int4),(10::int4),(20::int4)) t(x)

无 ORDER BY 是全分区但不冻结物理末值；覆盖末行 NULL、CURRENT ROW、自身合法空帧和非法起止/负/NULL offset。缓冲与流水线同夹具。

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


在 `pkg/executor/windows/window_sql_test.rs` 待新增 `pg09_13_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_13_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_13 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次13执行；行为依赖为12，调度依赖为批次12。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_13 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture
    make lint
    git diff --check

新增/修改解析器时还须按根矩阵运行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 并审查生成差异；不修改解析器不无故执行这些命令。Bazel准备按根完整触发清单判定；只改Rust且无Bazel/Go/依赖触发时不运行 `make bazel_prepare`。禁止默认运行 `make bazel_lint_changed`。如需Go测试，先依testing-flow判定failpoint并在退出禁用；Go既有回归失败记录后按用户规则不阻塞本任务，新增回归不得忽略。

检查当前改动实际影响的原有相邻测试并记录精确命令；不运行无依据全包/昂贵广扫。涉及真实存储才依 `docs/rust/rust-postgresql-tikv-runbook.md` 与testing-flow启动专属后台playground、验证并清理。本计划不授权在拆分阶段运行上述命令。

风险：共享描述符/执行状态改动可能影响MySQL；排序/帧/行扩展可能增加内存或错误地改变行数；先保持单阶段正确并验证资源释放，不据小夹具给出吞吐承诺。恢复只撤销自己明确有问题的变更，不删除其他任务/用户文件，不移除未知锁或服务。

## 完成与交接


只使用状态：已完成，待回归、已完成、已阻塞、已完成，待回归。完整行为和所需适用验证通过后标记已完成；有实际代码/测试/配置/文档变更使用 `$git-commit` 技能只提交本任务。只有编号文件状态或删除时无需单独提交。最终报告路径/符号、生产修改依据、验证profile、实际槽位/绝对CARGO_TARGET_DIR、完整命令/退出码/有效计数、正确性/兼容性/性能风险、未验证项及完整提交SHA。

验证因无关环境缺失无法运行，可记录已完成，待回归并保留本文件；不能把仍缺失的目标行为或依赖接口标为完成。阻塞只写本文件及已做的检查，不扩大范围。完成证据已交接且提交妥当后按执行契约删除本编号文件，`plan.md`只读，父大计划和覆盖表保留。删除文件本身不是完成证据。

## Progress


- [x] 确认行为依赖与实际入口。
- [x] 记录运行期失败或已有正确行为的当前证据。
- [x] 完成最小实现及同夹具通过验证。
- [x] Ready适用检查与diff自审；提交交接见下方。

## Surprises & Discoveries


当前仅有静态规划，没有运行结果。实施中在这里记录实际接口差异、缺失依赖和对应证据。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


尚未实施。完成时填写实际输出、有效测试数量、参考版本、清理结果、剩余范围外依赖及完整SHA。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。

2026-10-31：任务12提交3515502d56599bac520ac11e38c1f1257009e800提供PhysicalWindowPlan会话入口与固定int8身份；实际代码和独立测试已确认。RustCodeGraph status成功（11847文件）；verify-profile仍缺失。领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。初始工作区有其他任务修改，不纳入本任务。

生产修改依据：window.rs/LastValue → 将既有aggfuncs整数LAST_VALUE状态适配到WindowFunction；relational_value.rs/relational_expression_type、relational_window_value → int4固定描述和SQL到共享帧处理器的必要局部接线；pg_conn.rs/sqlstate → 帧offset和起止错误的PG诊断身份。对应.rs.md同步说明职责。
运行期失败：env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture，退出101，1个有效测试，OID实际25/预期23。初次修复后相同过滤器server 1/1通过；kernel pg09_13 1/1通过。继续补协议、同序组、空输入、重绑、恢复和分块清理覆盖。

新增取证：零行派生输入Describe仍为text，query.rs/relational_query_expression_type_from_sources_uncollated增加last_value参数身份的绑定递归（必要局部描述接线）。负数/NULL帧偏移被原语法拒绝为42601，grammar/main.astergram和parser_actions/query.rs新增四条PG方言限定规则，复用FrameBound载体；generated/main_tables.rs由既有release parsergen生成（共享槽位1），独立parser_test.rs增加pg09_13_frame_offset_ast。原MySQL语法保持拒绝。未修改Go源码/依赖/Bazel，不触发bazel_prepare。
Ready parser命令：make parser、make parser_yacc、make parser_fmt均退出0。首次make parser_unit_test启动后发现failpoint前置，主动终止（不计有效测试）；guarded重试因新增的failpoint binding未格式化而在fmt退出2，未运行测试；补格式化该临时binding后再次guarded执行，退出均自动make failpoint-disable。

## 2026-10-31 交付证据

本任务行为已实现并通过当前验证；状态“已完成，待回归”仅由无关的既有全局admin规则库存断言失败造成，保留任务供该断言修复后回归，不把行为缺失当验证缺证。不修改只读plan.md，不修改admin实现或其期望数。

新增窗口LastValue类型只负责把已有aggfuncs::func_value::LastValue<i64>状态适配到WindowFunction，注册点是会话PhysicalWindowPlan.window_functions；没有新执行器或依赖。LastValue的输出字段删除NotNullFlag以覆盖合法空帧。parser_actions/query.rs对非法起止在PG Parse阶段拒绝；窗口frame offset负数/NULL在Execute分别给22013/22004，语法非法给42601，原始S/V/C/M/P字段及缺失D/H记录在测试输出。query_expression_aster_unit_test.rs清单241→245只对应新增四条本任务语法。

验证profile：先WIP聚焦，再Ready按Rust/Parser影响选择检查；verify-profile技能不存在，不宣称运行该技能。所有Cargo/产物调用都使用槽位1和绝对CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。以下命令均在仓库根目录执行（Cargo环境前缀：CARGO_TARGET_DIR="$PWD/target/rust-slot-1"；Rust测试均保留默认features）。

| 完整命令 | 退出码与有效计数 |
| --- | --- |
| cargo fmt --all | 0，最终格式化；parser_actions另用下项格式化 |
| rustfmt --edition 2024 pkg/parser/parser_actions/query.rs | 0 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_13 -- --nocapture | 0，2/2测试；含缓冲/流水线同夹具、跨Chunk peers、NULL和close后内存归零 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture | 最终0，2/2；最初101（1有效失败，OID25≠23）；中间新增覆盖发现零行OID及负offset解析问题后均修复，最终覆盖Q、Describe、text/binary四组合、NULL重绑、错误恢复、双PG与同Server MySQL交错和关闭清理 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg09_13 -- --nocapture | 最终0，2/2；首次测试引用错误crate名编译101，0有效测试，不算行为证据，已修复 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib query_expression_rule_coverage -- --nocapture | 0，1/1 |
| env -u LDFLAGS cargo test --locked -p astersql-parser --lib all_action_rules_have_one_owner -- --nocapture | 101，1有效失败；逐规则唯一归属循环完成后admin库存断言553≠551，见下述基线证明 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib go_test -- --nocapture | 0，9/9既有相邻测试 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib window_memory_test -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_12 -- --nocapture | 0，2/2（窗口排名和VALUES接线） |
| target/rust-slot-1/release/astersql-parsergen generate | 0；仅本任务main表与语法/动作一并提交，工作区原有hint表改动不纳入 |
| target/rust-slot-1/release/astersql-parsergen check | 0，生成物无漂移 |
| make parser | 0 |
| make parser_yacc | 0；Go生成物无差异 |
| make parser_fmt | 0 |
| make failpoint-enable; gofmt -w pkg/parser/ast/binding__failpoint_binding__.go; make parser_unit_test; make failpoint-disable | 最终0，14个Go测试包成功（子测试数量未采计）；使用EXIT/INT/TERM trap保证disable，末尾refcount=0。早期未带failpoint运行已主动终止，首次guarded运行在临时binding的fmt阶段退出2，均不算有效测试通过 |
| make lint | 0 |
| git diff --check | 0 |

范围外基线证据：用Python读取git show HEAD:pkg/parser/parser_actions/admin.rs的稳定RuleId键，与git show HEAD:pkg/parser/generated/main_tables.rs及当前主表的RuleId求交，HEAD与工作区均为553；既有remaining_aster_unit_test.rs期望551。本任务只新增四条query规则，没有修改admin模块，因此不递归修复该既有库存测试。

正确性证据：默认RANGE 10/10/20→10/10/20；全分区ROWS→20/20/20；不同值同序组→2/2/3；空帧保留行并返回NULL，末行NULL、全NULL、空输入固定int4 OID23/长度4/typmod -1。MySQL使用自己的UNION ALL语法与同Server交错通过。无ORDER BY只验证全分区行为，不把物理末值顺序提升成通用SQL承诺。

未验证：未启动PostgreSQL18.6真实参考客户端、RealTiKV或性能压测；真实客户端差分属任务18，数值RANGE偏移属任务14，其他函数和复杂帧仍各归其任务。语法错误主消息/position只记录当前原始字段，未与18.6逐字差分。性能沿用现有逐行窗口会话接口，会重建分区帧，不作吞吐承诺。兼容风险以MySQL对照、既有窗口/内存及批次12回归约束。

清理：测试临时listener/socket均通过Drop及显式关闭释放，端口拒绝重连已验证；Cargo槽位阶段结束后仅删除自己的owner和锁目录，保留缓存。所有非本任务工作区修改保留。提交SHA在最终交接报告。

本提交精确文件清单（18个）：

- `.plans/2026-10-29-postgresql18-聚合窗口与集合返回/13-LAST_VALUE默认帧与ROWS.md`
- `pkg/executor/windows/window.rs`
- `pkg/executor/windows/window.rs.md`
- `pkg/executor/windows/window_sql_test.rs`
- `pkg/parser/generated/main_tables.rs`
- `pkg/parser/generated/main_tables.rs.md`
- `pkg/parser/grammar/main.astergram`
- `pkg/parser/parser_actions/query.rs`
- `pkg/parser/parser_actions/query.rs.md`
- `pkg/parser/parser_actions/query_expression_aster_unit_test.rs`
- `pkg/parser/parser_test.rs`
- `pkg/server/pg_conn.rs`
- `pkg/server/pg_conn.rs.md`
- `pkg/server/pg_query_test.rs`
- `pkg/session/runtime/query.rs`
- `pkg/session/runtime/query.rs.md`
- `pkg/session/runtime/relational_value.rs`
- `pkg/session/runtime/relational_value.rs.md`

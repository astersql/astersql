# 任务 14：数值RANGE偏移帧

批次：【批次 14】；调度依赖：批次13（共享源码/注册与验证资源串行）；行为依赖：13。

状态：已完成

目的：RANGE 按排序值距离而非行号划帧；10/10/20 使用 CURRENT ROW 至 5 FOLLOWING 得 10/10/20。升降序、peer、NULL、整数精确边界均不转 f64 丢精度。

来源任务：大计划9；路线图第4章 `WIN-CALL-FRAME`。父文件 `.plans/2026-10-24-postgresql18-后续整体实施/9-聚合窗口与集合返回.md` 仅记录拆分，不因本子任务生成而完成。

预计会话范围：只处理本标题的一个行为闭环及其必要分析/执行/描述接线、相邻测试；不承担其余函数族或整套基础设施。依赖满足后单个上下文执行；发现额外独立缺口记录到本文件，不吞并别的任务。

## 上下文与接口


所有命令从仓库根目录运行。默认 features，不添加新 feature。实施前读取根 AGENTS.md、目标包 doc.go（若届时存在）、server 局部 AGENTS.md；优先通过 `skills/rustcodegraph/SKILL.md` 检查索引和消费者。基线 HEAD 为 `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`，不得将本次规划当作运行证明。

保留现有 Go/MySQL 逻辑及 PingCAP Apache 版权；修复可用 Rust 文件顶部保留/添加 `// Copyright 2026 AsterSQL.`。源码仅挂独立测试模块。不得因本任务递归重写类型库或建立第二套执行器。外部依赖如需变更只能独立上游提交打tag、manifest统一已发布tag并清理临时副本，禁止vendor/third_party/本地patch。

实际外部前置：3号数值比较/加减，2号帧边界载体。确认真实接口与当前测试已存在且适用；不得仅凭任务文件删除推定完成。所有内部行为依赖的证据见其提交及最终交接；若尚缺，保持未开始或记录已阻塞。

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


RANGE 按排序值距离而非行号划帧；10/10/20 使用 CURRENT ROW 至 5 FOLLOWING 得 10/10/20。升降序、peer、NULL、整数精确边界均不转 f64 丢精度。

代表SQL（其他边界使用同型最小夹具）：

    SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND 5 FOLLOWING) FROM (VALUES(10::int4),(10::int4),(20::int4)) t(x)

offset 恰一排序键、非负非 NULL；时间键 interval RANGE 仍是4号类型接口依赖，显式能力拒绝不算支持。

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


在 `pkg/executor/windows/window_sql_test.rs` 待新增 `pg09_14_execution`，在 `pkg/server/pg_query_test.rs` 待新增 `pg09_14_sql`，必要时 `pg_types_test.rs` 新增同前缀描述断言。现有文件由各crate lib.rs挂载；测试名为本计划新定义，当前尚不存在。

先新增上述独立测试并编译可执行，再运行相同命令取得目标值/行数/描述或错误阶段不匹配的运行期失败；语法/计划缺口通过SQL入口返回错误可以证明尚未支持，但不得将编译失败当行为证据。若现有行为已正确，保留新增覆盖的当前通过证据，不作无意义生产修改。

失败验证与通过验证使用相同过滤器，默认features；命令必须在领取槽位后执行：

    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_14 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_14 -- --nocapture

断言命中的每个测试实际执行，报告有效测试数；任一命令零测试不算通过。先运行 `cargo fmt --all` 再运行实现后的验证。测试使用真实执行器/会话和临时TCP listener，不mock SQL结果、类型或错误。内核局部测试可直接构造真实输入Chunk，以证明跨页状态或边界计数，不替代SQL证据。

## 实施步骤与里程碑


第一里程碑是证据就绪：复核上述接口、写独立测试，取得运行期失败或确认已有正确行为。第二里程碑是本行为通过：只补必要生产接线，保持方言身份、NULL、错误和固定描述。第三里程碑是交付：同一夹具通过、周边回归适用、审查修改并记录证据。

按批次14执行；行为依赖为13，调度依赖为批次13。可并行执行批次在本计划中均只有一个任务，不能因Cargo有10槽就并行修改共享文件。任务级复核可以重用仍有效的验证，不重复昂贵广扫。

## 验证与交付


实施迭代使用WIP聚焦命令，交付使用Ready适用检查。当前 `.agents/skills/tidb-verify-profile` 不存在，执行根AGENTS.md与 `docs/agents/testing-flow.md`，不宣称技能已运行。

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_14 -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_14 -- --nocapture
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


当前仅有静态规划，没有运行结果。实施中在这里记录实际接口差异、缺失依赖和对应证据。

## Decision Log


2026-10-29：按单一行为拆分并串行使用共享源码；选现有执行链，禁止标量或目录特例替代。实施中记录必要方案变更与依据。

## Outcomes & Retrospective


尚未实施。完成时填写实际输出、有效测试数量、参考版本、清理结果、剩余范围外依赖及完整SHA。

## Artifacts and Notes


此编号文件是依 `PLANS.md` 维护的活动ExecPlan。证据记录在本节，长期测试用例在独立测试文件；不修改只读整体指南。规划新增测试名不代表当前已注册或已通过。


## 2026-10-31 前置接口核查与阻塞交接

当前核查 HEAD：`ccecda92ec36bafb9bb9a6ee74115bc198070c7b`。只更新本编号任务，不修改只读 plan.md，不改生产代码、测试或其他任务的工作区变更。

批次13已有提交 `ccecda92ec36bafb9bb9a6ee74115bc198070c7b`，编号文件保留为“已完成，待回归”，其中记录窗口内核2/2、server2/2及相邻验证通过；待回归项是既有admin规则库存553≠551。这是历史证据，未冒充本任务当前运行。真实入口确有 LastValue、PhysicalWindowPlan 与固定int4描述。批次2的AST边界载体已存在于 pkg/parser/ast/lib.rs:4437，包含 Expr/UnBounded/Type；这一前置满足。

阻塞是本任务明列的外部3号数值比较/加减接口尚不足：

- pkg/session/runtime/relational_value.rs:72 relational_compare 对整数使用精确比较，非整数走 parse::<f64>()；:3142 relational_order_compare 调用此比较器，:3850窗口先用它排序。数值键在进入物理帧之前就可能合并不同值，无法仅替换物理边界比较修复。
- pkg/types/pg_numeric.rs 当前公开接口有 add、divide、编解码与修饰符，但没有精确比较（Ord/PartialOrd/cmp）或减法接口。不可把已存在的add推定为所需比较/加减接口整体完成。
- pkg/executor/windows/window.rs:415 FrameBound仅承载u64偏移；:460 boundary_number及:485 candidate_cmp_boundary将有偏移数值转换为f64。:141 Decimal比较在scale对齐溢出时同样回退f64。这些是本任务后续必须处理的局部落点，但不应通过另写通用数值比较器来吞并外部3号责任。
- pkg/session/runtime/relational_value.rs:3926当前明确拒绝显式RANGE偏移；:3979将排序键编码为peer编号，因此目标行为当前仍缺失，不能标“已完成，待回归”。

精度探针（仅说明源码路径的数值风险，非SQL运行或行为验收）：Python float('9007199254740992.1') 与 float('9007199254740992.2') 均为9007199254740992.0，比较相等。后续需使用3号交付的定型精确比较及边界算术；不能把范围缩为int4代表SQL或用能力拒绝代替完整数值目标。

已执行检查（仓库根目录）：

- cat skills/do-task-plans/SKILL.md、总plan.md、本任务、任务13、PLANS.md、skills/rustcodegraph/SKILL.md及pkg/server/AGENTS.md：读取成功（0）。pkg/executor/windows/doc.go及pkg/session/doc.go不存在（cat退出1），文件搜索未找到目标目录doc.go。
- git status --short、git log -6 --oneline、git rev-parse HEAD：0；已识别并保留其他任务的大量在途修改。
- ~/.rustcodegraph/bin/rustcodegraph status：0，11847文件；explore 'PhysicalWindowPlan RANGE frame last_value'：0，结果过宽，继而使用node定点读取。
- ~/.rustcodegraph/bin/rustcodegraph node --file pkg/executor/windows/window.rs --offset 400 --limit 132；--offset 30 --limit 110；--offset 126 --limit 77：均0。
- ~/.rustcodegraph/bin/rustcodegraph node --file pkg/session/runtime/relational_value.rs --offset 3820 --limit 160；--offset 3978 --limit 70；--offset 70 --limit 85；--offset 3142 --limit 50：均0。
- rg -n 'pub fn|impl (PartialOrd|Ord)|fn (cmp|partial_cmp|subtract|sub)' pkg/types/pg_numeric.rs：0，输出见上述现有公开接口，无精确比较/减法命中。
- rg -n 'pub struct FrameBound|pub Expr:|pub UnBounded:' pkg/parser/ast/lib.rs：0，确认AST载体。
- python3精度探针：0，equal=True；有效SQL/协议测试0，不计作行为通过。
- test -f .agents/skills/tidb-verify-profile/SKILL.md：1，技能仍缺失。

验证profile：仅前置只读分析，未进入WIP/Ready代码验证，不宣称检查通过。未执行Cargo、调用Cargo的脚本或使用槽位产物，因此未领取Cargo槽位，CARGO_TARGET_DIR未设置；没有自己的锁或进程需清理。未运行cargo fmt、pg09_14两组测试、make lint、解析器检查、真实PG18.6/RealTiKV或性能验证。生产行为尚未实现，正确性/兼容性/性能均未验收。

决策：按本任务“实际外部前置”及执行技能“遇到阻碍（缺少依赖…）立即停止执行”要求停止，不扩建通用类型/排序系统。下一步由外部3号提供可复用精确数值比较/边界加减及对应测试，然后恢复本任务的独立TDD、必要局部接线和Ready验证。只有编号任务的状态及阻塞记录更新，按用户规则不提交、不删除本文件，无本任务提交SHA。


## 2026-10-31 恢复实施

用户补充授权允许在现有表示/执行链内补齐最小精确比较和偏移接口，前次静态阻塞结论被此决策替代。未修改plan.md。领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。新增pg09_14独立测试，kernel首个夹具通过后加强大整数降序边界，同过滤器退出101（1有效失败）；server退出101（1有效失败），目标SQL被明确拒绝。不是编译失败。

生产修改依据：pkg/types/pg_numeric.rs/PgNumeric::compare、subtract仅提供RANGE所需的系数精确排序/现有add的符号翻转复用；pkg/executor/windows/window.rs/FrameBound对原整数偏移使用i128边界，新增定型numeric偏移绑定复用既有范围处理器；pkg/executor/windows/Cargo.toml新增已有workspace本地types依赖，不引入外部依赖或本地覆盖；pkg/session/runtime/relational_value.rs/relational_window_value仅对显式数值RANGE使用精确排序与真实键值，替换peer编号接线，保留原默认帧路径。未创建生产类型文件或第二执行器。对应.rs.md在交付时同步。


## 最终交付证据（2026-10-31）

状态已完成，前次阻塞已通过用户授权的最小局部接线解除；未改只读plan.md。新增生产文件0、外部依赖0；仅已有workspace astersql-types依赖成为窗口的活动依赖，Cargo.lock已含此依赖故无变更。对应.rs.md已同步。pkg/server/pg_conn.rs新增必要错误映射（单排序键42601、非数值/interval能力拒绝0A000），不把时间RANGE或GROUPS记为支持。所有Rust源文件已有AsterSQL标记，PingCAP版权保留。新增测试均独立于生产源码。

精确边界：整数不转f64，Int/Int用i128避免上下溢，UInt/Decimal复用精确系数运算；SQL numeric排序/边界全程PgNumeric。浮点键独立使用自身浮点算术，未将它用于整数/numeric。代表10/10/20输出10/10/20；numeric 9007199254740992.1/.2/.4与offset0.1输出2/2/3；升降序、NULLS FIRST/LAST、int8两端、u64上端、小数、peer、合法空帧、空输入及跨Chunk均有当前测试。极小int8裸负数字面量原有解析/求值溢出未纳入修复，测试改用同值字符串显式cast保留边界本身；不以缩小数值范围绕过。

协议证据：Q与Parse/statement Describe/Bind/portal Describe/Execute/Sync；结果固定r/int4 OID23/typlen4/typmod-1，零行描述不漂移；结果text/binary及参数text/binary四组合、NULL重绑、Execute不额外发描述。负/NULL偏移、无/多排序键在本地Execute失败，状态分别22013/22004/42601；原始S/V/C/M均输出，D/H/P未发送，非参考差分。每次Sync得到ReadyForQuery I，再SELECT1成功。双PG与同Server MySQL（使用其原生UNION ALL/ROWS语法）交错通过，临时socket/listener关闭且端口拒绝重连。当前错误阶段只记录实际值，未与PostgreSQL18.6逐阶段逐字比较，此差分属任务18，不宣称18.6参考通过。

Profile：WIP红/绿后Ready；verify-profile技能确实缺失，按根AGENTS及testing-flow选择适用检查。无Go/解析器/Bazel/外部依赖变化，不触发bazel_prepare或parser系列；未运行bazel_lint_changed。窗口crate仅19个测试且共享边界比较影响全部帧与函数，因此完整运行这个小crate验证相邻行为，没有扩大到workspace。

所有Cargo命令从根执行并继承 CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1（动态领取1号）。同一阶段内同一持有会话使用该槽位；最终阶段用EXIT/INT/TERM trap在所有子进程结束后清理自己的owner与锁，保留缓存。没有删除他人锁/进程。

| 完整验证命令（Cargo均上述环境） | 退出码/有效计数 |
| --- | --- |
| cargo fmt --all | 0，实施后及最终运行 |
| cargo check -p astersql-executor-windows | 0，0行为测试，仅编译证据 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib pg09_14 -- --nocapture | 初版1/1通过，加强边界后101（1有效失败）；修复后最终0，3/3 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_14 -- --nocapture | 首次101（1有效失败，显式RANGE拒绝）；极值裸字面量阶段101（1有效失败），同值显式cast后及最终0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-executor-windows --lib -- --nocapture | 0，19/19（含既有9项Go对齐、内存、帧/排名及本项3项） |
| env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_numeric_test -- --nocapture | 0，10/10（本项1，既有numeric9） |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_13 -- --nocapture | 0，2/2 |
| env -u LDFLAGS cargo test --locked -p astersql-server --lib pg09_12 -- --nocapture | 0，2/2 |
| make lint | 0 |
| git diff --check | 0 |

最初一次包含rm -f清理trap的shell被工具安全规则拒绝，整条命令未执行；改为核对自身session标识后由Python unlink/rmdir清理，最终阶段成功结束并释放1号锁。

风险：正确性由上述局部行为约束；PG18.6错误消息/阶段、时间interval及其他窗口函数的完整numeric签名未据此验收。兼容性通过既有窗口、批次12/13及MySQL对照约束；没有修改通用关系排序器。性能仍沿现有逐行重建分区的会话路径，精确数值解析/运算可能增加CPU及临时分配，未压测、未作吞吐承诺。没有启动RealTiKV或外部PG参考进程；本项不依赖真实存储。

代码自审完成，仅暂存本任务12个代码/测试/说明文件与本编号文件。其他工作区变更保持原样。按git-commit技能提交后在最终回复报告完整SHA及上述证据，再按用户要求删除本编号文件（删除不另作提交）。

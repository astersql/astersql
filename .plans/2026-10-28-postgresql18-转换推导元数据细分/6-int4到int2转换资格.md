# 任务 6: int4到int2转换资格

批次：【批次 6】 依赖批次 5（共享源码与验证资源串行交接）

状态：已完成，待回归

目的：只处理int4→int2有向对：反向仅E/A，I拒绝；E为显式转换，A为目标列赋值，I为表达式隐式转换。数值舍入、溢出22003及NULL沿目标族，不能通过text串接。

来源任务：大计划6，`.plans/2026-10-24-postgresql18-后续整体实施/6-转换推导参数与元数据扩展.md`；路线图第3章显式隐式转换契约、准备描述执行元数据契约。

预计会话范围：一个有向类型对或一个推导/描述行为，复用相应类型族。只改该行为的分析、求值和必要协议接线及独立测试；不建立完整子系统。单次会话完成失败/通过证据及交接。

## 文件

候选修改：`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/expression/builtin_cast.rs`；协议必要接线在`pkg/server/runtime.rs`、`pkg/server/pg_extended.rs`、`pkg/server/conn.rs`、`pkg/server/pg_result.rs`。这些是候选落点，不要求全部修改。

测试：在已注册的`pkg/server/pg_extended_test.rs`追加待新增测试`pg06_task_006`，复用该文件read/send/query/parse/bind/execute和PgService、CreateAnalyzeSession真实入口。测试函数不得写入生产文件；不新增测试模块或Cargo feature。必要内核边界断言扩展`pkg/expression/builtin_cast_test.rs`，不得以其替代TCP行为验证。

## 上下文与接口依赖

大计划1、2及相应类型族的局部接口。执行前核对该最小接口存在且可消费；缺失时只在本任务记录精确签名、缺证及依赖，不递归扩建。`indexed_parameter_contract`负责参数映射，`prepare_statement_with_parameter_types`为准备入口；保留源类型，不靠Bind值或结果别名猜测。E/A/I表示显式/赋值/隐式资格，typmod表示声明长度或精度，OID表示PG类型或目录对象身份。

先读根AGENTS.md、pkg/server/AGENTS.md及存在的目标包doc.go（本轮三个包均无该文件），按rustcodegraph技能核查最新源码。大计划3–5拥有值模型，本任务只新增该有向差异或不可缺少的局部接线。每项生产修改在本文件记录其来源契约条目或接线依据。

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

行为：只处理int4→int2有向对：反向仅E/A，I拒绝；E为显式转换，A为目标列赋值，I为表达式隐式转换。数值舍入、溢出22003及NULL沿目标族，不能通过text串接。

真实输入：SELECT (1::int4)::int2；以int2目标列INSERT/UPDATE同源值；在仅接受int2的上下文验证I。覆盖目标边界两侧、舍入临界及typed NULL。

失败验证测试：待新增`pkg/server/pg_extended_test.rs::pg06_task_006`。失败验证命令（仓库根目录，默认features）：`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_006 -- --nocapture`。新增测试必须先可编译，预期运行期在上述值、资格、参数身份或描述断言失败；先确认实际缺口。若当前行为已正确，只补覆盖并保存当前通过证据，不人为造红灯。

通过验证使用完全相同命令及输入。至少覆盖正常、边界、非法输入、NULL；错误比较SQLSTATE、发生阶段、无部分结果/副作用，Sync后显式`SELECT 1::int4`恢复。成功比较值、NULL、OID、typlen、typmod和格式；无结果字段明确不适用，未采集字段不能认定相等。模拟策略：真实共享内核、测试存储、临时TCP端口；不mock转换结果或元数据。参考PostgreSQL18.6采集准确预期，18.4仅单独报告，不自动替代18.6；缺固定参考构建时保留待回归及缺证。DSN由环境读取且不得输出密钥。

## 步骤与里程碑

1. 确认局部依赖，阅读契约对应方向与现有实现；在本文件登记实际受影响符号及来源。
2. 编写上述独立测试，领取槽位后运行失败验证并保存有效测试数量、具体断言与退出码。编译失败、零测试不算行为失败。
3. 仅实现本行为所需最小改动，保持Go/MySQL语义，保留PingCAP头；修复可用Rust文件顶部增加或保留`// Copyright 2026 AsterSQL.`。
4. 在槽位内先运行`cargo fmt --all`，再运行同一聚焦测试，验收该输入闭环。按该任务输入扩展真实参考夹具前，先核对现有libpq测试注册及接口，不假定新脚本已存在。
5. Ready交付运行`make lint`，审查diff；若修改Go导入/新增Go测试等触发根清单则先`make bazel_prepare`。不运行`make bazel_lint_changed`。使用Go测试时按testing-flow启用并最终禁用failpoints；Rust默认features，无新增开关。

## 验证与完成

命令：`cargo fmt --all`；`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_006 -- --nocapture`；`make lint`；`git diff --check`。WIP用于聚焦迭代，Ready用于交付；verify-profile技能当前缺失，不宣称已使用该技能。新增有效测试至少1个，所有适用断言通过，保留失败/通过输出、真实参考版本与未验证项。同Server MySQL会话在PG流程前后验证原有整数值/CAST行为，避免方言泄漏。

涉及真实TiKV才按runbook后台启动专用playground、验证并清理自己服务和数据；不删除用户资源。外部Rust依赖不得vendor或本地patch，确有移植另走上游提交/tag流程。本任务不授权扩展依赖移植。

按Agent Output Contract报告文件、验证profile与理由、正确性/兼容性/性能风险、确切命令、退出码、测试数及未验证项。实际变更完成后使用$git-commit只提交本任务；仅状态更新/删除不单独提交。证据齐全才标记已完成并删除本编号文件；无关环境阻止验证可标记已完成，待回归并保留文件和缺证；依赖缺失标记已阻塞。不得修改plan.md。

## Progress

- [x] 核对依赖及生产差异依据。
- [x] 取得行为失败或既有正确行为的当前证据。
- [x] 完成实现、聚焦验证和Ready检查。
- [x] 完成交接、资源清理及适用提交。

## Surprises & Discoveries

规划时未运行测试；预期缺口须由实施前测试确认。

## Decision Log

2026-10-28：选择单行为最小接线与真实TCP验收，避免类型族重建；本编号文件按PLANS.md作为活动ExecPlan维护。

## Outcomes & Retrospective

本地TDD闭环及Ready检查通过；固定PostgreSQL18.6参考差分缺证，保留本文件待回归。早期阻塞已由下述最小局部接线解除。实施者在此记录新证据、实际修改和剩余范围外事项。

## 重试与恢复

测试使用唯一临时表及系统分配端口；成功、失败及中断均关闭连接/listener、删除自己夹具，待子进程结束才释放槽位。可重复运行；保留缓存，不重置其他任务变更。

## 执行记录（2026-10-28）

依赖批次5已提交 cf80601c38，本地TCP及Ready检查通过、固定18.6参考待回归；本轮读取其交接记录，未把参考缺证当作已通过。已读取根/server规则、PLANS.md、do-task-plans及仓库RustCodeGraph技能；目标三个包doc.go均不存在。plan.md保持只读。

阻塞在步骤1的最小接口核对：尚未定位可消费的“只接受int2参数”的表达式入口及其按源类型拒绝int4隐式收窄的分析接口。已有 `relational_query_expression_type_from_sources(&self, expression: &ast::ExprNode, source_fields: &[ConcreteResultField]) -> FieldType`（query.rs:2647）针对TypeShort/TypeLong混合加减乘执行整数提升，不能作为int4→int2的I拒绝证据。`indexed_parameter_contract`及`prepare_statement_with_parameter_types`已存在，但参数声明OID21会把输入定为int2，不能保留源int4身份来验收此项。显式CAST及目标列赋值分别属于E和A，也不能代替I。需要上游提供已有的int2专用表达式入口/签名及源int4分析路径，或明确授权一个最小局部入口；不扩建函数重载子系统，不用不存在函数的错误冒充类型拒绝。

已运行检查（只读）：`git status --short`（0，干净）；`git log -5 --oneline`（0）；RustCodeGraph `status`（0）、`explore 'pg06_task_005 pg06_task_004 indexed_parameter_contract'`（0）、`node pg06_task_005`（0）；rg检查planning.rs/query.rs及PG族模块的隐式转换和函数入口，仓库Rust文件搜索int2send/int2inc/int2_sum/int2_accum没有定位对应入口。读取query.rs:2647处实际类型提升分支核对，不将搜索缺失视为运行期拒绝。`ls pkg/session/doc.go pkg/server/doc.go pkg/expression/doc.go`退出1，确认三个文件不存在；verify-profile目录亦不存在。

未运行Cargo、未领取槽位；实际槽位及CARGO_TARGET_DIR均不适用。未新增pg06_task_006，行为测试有效数量0；fmt、聚焦测试、make lint、真实18.6参考、MySQL隔离均未执行，不宣称行为已完成。当前只修改本任务状态和阻塞交接记录，按用户规则不提交、不删除任务文件。恢复后先确认上述I入口，再完整执行本文件TDD与Ready流程。风险/限制：没有int4→int2行为验证结论；没有生产、测试、配置变更，兼容性和性能未评估。

### 恢复执行

收到继续执行指示后复核：原阻塞是尚需补足的局部实现，不是外部依赖。选用固定签名int2not(int2)->int2作为本方向唯一必要表达式上下文，直接接入query.rs既有类型/分析/求值入口，不建立通用重载系统。本机18.4的postgres.bki登记OID1895 int2not参数21/返回21，作为签名辅助核对，不替代18.6参考实测。

新增pg06_task_006首轮编译成功并实际运行1个测试，0 passed/1 failed/442 filtered out；失败于int2not(1::int2)尚不支持（XX000），其前E/A成功断言已通过。日志/tmp/pg06-006-initial.log。生产修改依据：query.rs固定签名分析拒绝源int4（含NULL）对应本任务I拒绝契约；固定int2求值及OID21类型推导为该拒绝上下文的最小正向接线；pg_conn.rs将该唯一固定诊断映射42883，保留Parse阶段语义。没有改E/A转换器，沿用已有范围检查。所有原版权头保留。

### 最终验证与交付

第二轮有效运行1个测试（0 passed/1 failed/442 filtered out，退出101），发现UPDATE int4→int2赋值越界返回XX000而非22003，日志/tmp/pg06-006-assignment-failure.log。据此在dml.rs两个写入入口（INSERT/UPDATE）增加仅PG、目标TypeShort且源TypeLong的pg_cast_int2范围检查，将既有typed错误交给协议；这是本任务A资格及22003契约必需局部接线。没有修改MySQL或其他类型对。中间一次编译因错误辅助函数名退出101，没有计为行为证据，已改为既有SessionError::with_source模式。

最终相同测试命令、相同输入：1 passed、0 failed、442 filtered out，退出0，日志/tmp/pg06-006-test.log。E显式值与边界、A的INSERT/UPDATE正常值及typed NULL、I的int2not正向及int4/NULL::int4于Parse拒绝42883均通过。参数OID23在Describe保留，目标结果OID21/typlen2/typmod-1及文本格式0、statement/portal描述一致；非法Bind22P02、目标上下越界22003、无部分行/无赋值副作用、Sync与SELECT 1::int4恢复、同Server MySQL CAST前后结果一致通过。int4值没有小数部分，本方向舍入临界不适用，不用text串接伪造输入。DROP成功，服务RAII关闭。

确切验证命令（仓库根目录）：
- `cargo fmt --all`：退出0（/tmp/pg06-006-fmt.log）。
- `env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_006 -- --nocapture`：初次101（有效1 failed），第二次101（有效1 failed），最终0（有效1 passed）。
- `make lint`：退出0（/tmp/pg06-006-lint.log）；其Go/dashboard检查在最终Rust范围检查新增前完成，内容仍覆盖最终交付，无Go/dashboard改动，无需重复。
- `git diff --check`：退出0；提交前另检查暂存diff。
- `/opt/homebrew/opt/postgresql@18/bin/postgres --version`：退出0，18.4 (Homebrew)。PG18_REFERENCE_DSN未配置，18.6实测缺证；未将18.4登记为参考通过。外部源码抓取尝试失败，与本地实现和验收无关。

WIP用于行为红/绿迭代，Ready用于fmt、聚焦TCP、lint及diff自审；verify-profile技能不存在，没有宣称调用该技能。未改Go/Bazel/依赖，不触发bazel_prepare，不运行bazel_lint_changed。所有Cargo阶段原子动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；owner记录本任务/会话/PID，所有子进程退出后EXIT trap清理owner和锁，保留缓存。

交付文件：query.rs（固定签名最小上下文）、dml.rs（本方向赋值越界）、pg_conn.rs（固定资格错误SQLSTATE）、pg_extended_test.rs（独立真实TCP测试）及本任务记录。仅提交本任务变更，不push，plan.md未修改。风险：固定签名接线不宣称通用函数重载支持；复杂表达式组合及binary结果未验证；基本MySQL隔离通过，完整双协议回归与18.6参考未验证；新增常数级类型/范围判断，未做性能基准。下一步在固定18.6参考端重放本测试SQL/参数并采集元数据、值、错误阶段与副作用。只因外部参考缺证标记已完成，待回归，保留本任务文件。

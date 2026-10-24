# 任务 10: libpq整数差分与首批回归

批次：【批次 10】依赖：批次 9；承接此前已完成的代码与验证

状态：已完成

目的：用真实libpq和PostgreSQL18参考端验收首批整数转换、参数与元数据闭环。

来源任务：蓝图1、8；首批四项用户需求的真实验收；用户2026-10-24首批实施要求。

预计会话范围：围绕一个可验收行为，仅修改下列生产链和独立测试；前序依赖满足后可独立阅读本文件执行，不重新展开八章。若调用链事实要求超出本行为，记录阻塞与证据，不扩建完整子系统。

## 文件


- 修改（由批次1创建）：`pkg/server/testdata/pg_core_libpq.py`。
- 修改：`pkg/server/pg_client_integration_test.rs`。
- 修改：`docs/postgresql-protocol-first-phase.md`。

## 上下文


任务1已提供独立客户端入口；本任务扩展同一小夹具，不能创建通用运行器。参考DSN和库只在测试进程读取，日志禁止输出密码及完整连接串。

本批线性顺序为1→2→3→4→5→6→7→8→9→10；每批一个任务，不安排并行。共享server/runtime、session、parser及验证资源有冲突；Cargo分槽不改变依赖顺序。

## 实施边界与执行前提


从仓库根目录执行。先读根 AGENTS.md、pkg/server/AGENTS.md、skills/rustcodegraph/SKILL.md、docs/agents/testing-flow.md；目标包有 doc.go 时必须读取。本轮已确认 server/session/parser 根包没有 doc.go；不要把模块说明当当前行为证据。优先用新鲜的RustCodeGraph定位生产消费者、工厂和相邻独立测试；未索引内容再用rg。来源为 docs/postgresql-18-core-compatibility-roadmap.md，原蓝图与路线图保持只读，不自动写回设计决策。

外部目标依据为[PostgreSQL18显式转换](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-TYPE-CASTS)、[整数类型](https://www.postgresql.org/docs/18/datatype-numeric.html)、[协议消息格式](https://www.postgresql.org/docs/18/protocol-message-formats.html)，2026-10-24已核对官方文档。执行期仍须采真实参考端；文档依据不是实测结果。

所有新测试名与新文件在本任务均为待新增，不是假定已存在。命令中的过滤器必须在新增测试注册后运行，核实实际执行数量大于0。测试位于独立 *_test.rs，不把测试函数放进生产源文件；新增Python文件是测试夹具。现有源文件中仅注册独立测试模块。保留PingCAP许可证，修复可用的Rust源文件顶部保留或增加 // Copyright 2026 AsterSQL.。

仅修本行为及不可缺少的局部接线，在本任务中记录实际生产修改与来源契约/调用依据；无关既有缺口登记范围外建议。外部Rust依赖若需移植，必须在独立上游提交打tag并统一引用已发布tag，不得vendor、third_party或本地patch；本批预期不需要外部依赖变更。

## 验证准备与交付检查


迭代使用WIP范围，交付使用Ready范围；当前仓库缺少 .agents/skills/tidb-verify-profile，不能虚构已执行该技能，按根AGENTS.md落实以下适用检查并记录缺失。所有Cargo命令使用默认features，不增加 --all-features；session现有fail依赖已启用failpoints，不要用Go failpoint工具改写Rust源码。若新增Go测试，按testing-flow启用并最终禁用Go failpoints。

Rust修改后先 cargo fmt --all，然后运行本任务精确命令；生成语法产物的任务在生成后再fmt并检查产物一致性。交付代码变更必须 make lint，若既有Go回归失败按根规则记录可不阻塞，不能把Rust或新增行为失败当Go既有失败。若lint本身无法执行须记录原因和未验证项，不标全部Ready通过。禁用 make bazel_lint_changed。本批仅Rust/Python测试及文档修改不自动触发bazel_prepare；若实际改动触发根AGENTS.md完整清单（含新Go测试/导入/依赖/Bazel文件/新工作区/工具链错误），先 make bazel_prepare 并纳入元数据。

真实客户端采用CreateAnalyzeSession提供的真实共享执行链和测试存储，是TCP/客户端真实执行，不宣称RealTiKV分布式验收。参考端与生产鉴权、性能和RealTiKV均须分别报告；不得借此安装或操作用户IDE。调用Cargo的任何脚本也须持有下节槽位。make lint等其他共享工具仍需避免与别的会话冲突。

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


行为：用真实libpq和PostgreSQL18参考端验收首批整数转换、参数与元数据闭环。

新增或修改的失败验证测试：pg_core_libpq_integer_differential；归属上述独立测试文件。类型/上下文测试可增加辅助断言，但不能替代真实SQL执行。

失败验证命令（先注册新测试，再执行）：

    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_integer_differential -- --nocapture

预期失败与例外（基于源码推断，实施时以实际运行核实）：验收测试新增覆盖省略OID准备、typed NULL/零行Describe和边界错误；历史代码预期不满足完整闭环，但前序已修复时可直接通过，不人为回退生产代码造失败。差分检测器须用临时修改内存观测值的负向自检证明能发现值/OID/SQLSTATE差异，不能改参考观测为期望值。

通过验证使用完全相同的命令与断言，不删除边界、不降级期待。模拟策略：使用真实canonical会话/共享内核，协议行为通过真实TCP；客户端用真实libpq。只允许测试构建下观察真实路径，不mock SQL结果、类型和错误，不用sleep猜测执行阶段。

## 步骤与里程碑


里程碑一：在实际消费者处注册上述测试，记录基线、失败阶段和有效测试数量；接口编译失败之后仍需取得适用的运行期失败证据。若已存在正确实现，保留当前通过记录，新增缺失回归，不做无意义生产改动。

里程碑二：完成下列本行为实现，并让同一组断言通过。

在独立Python夹具新增同输入运行AsterSQL和真实参考端的模式，测试配置名PG18_REFERENCE_DSN为本任务待新增的必要测试环境输入，缺失立即失败，不跳过；复用PG_LIBPQ_LIBRARY。运行前查询参考端version()/server_version_num，须为真实18.x，记录精确补丁和构建来源；来源18.6若不可获得，只能报告实际18.x对照，18.6目标列为未验证，不据此声称原蓝图完成。
用PQexec、PQprepare/PQdescribePrepared、PQexecPrepared及真实表回读覆盖本批两种cast语法、int4/int8边界、文本/NULL、参数OID省略与显式提供、重复/乱序参数、零行描述和22P02/22003。比较PQftype、PQfsize、PQfmod、PQfname、PQgetisnull、PQgetvalue、PQnparams/PQparamtype及SQLSTATE；portal原始字段由任务9真实TCP证据补齐，libpq未暴露字段不能默认相等。
普通值、typed NULL、零行、溢出、非法文本、重执行和错误恢复各有有效场景计数；两端观测独立存储，固定用例ID及输入顺序。显式事务错误核E→ROLLBACK→I，错误后下一请求成功；不把意料外错误改成预期拒绝。精确消息文本/错误位置不作全量相等，未覆盖字段列明限制。
同一Server保留健康MySQL连接，PG流程前中后核真实SELECT值，并执行MySQL自身CAST AS SIGNED与预处理回归。关闭所有结果和连接，删除两端本测试创建的唯一对象；不清空外部数据库，不停止参考服务。finally和Rust RAII覆盖成功、失败、中断清理。
执行本批pg_core_测试与指定相邻回归；更新已有协议说明，只记录当前已采集能力、精确版本、命令和限制，不重写路线图。生产差异若属于前序任务目标，只作最小修复并补同目录回归，禁止吞并其他PG能力。

里程碑三：只在测试通过后做必要局部整理，更新已改生产模块的现有 .rs.md 说明（存在且行为受影响时），再执行交付检查和差异自审。文档更新随代码走，不单独计作功能完成。

## 验证


具体crate与features：astersql-server，默认features；不得关闭现有运行路径来缩减测试。

阶段内先运行：

    cargo fmt --all

行为通过命令：

    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_integer_differential -- --nocapture

适用额外检查（新命令指向已存在Cargo包/入口）：

    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_ -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_introspection_namespace_session_state -- --nocapture

交付检查：

    make lint
    git diff --check

预期：真实两端差分有效场景数大于0且各必测族非零，全部值/NULL/适用描述/错误/恢复断言通过，双listener隔离及清理通过；缺参考端不能标已完成。

所需证据：当前HEAD/工作树标识、适用基线失败和修复后同断言通过记录、确切命令及退出码、有效测试数、真实输入/值/类型/错误/恢复与清理证据、实际修改文件及原因、Ready适用检查与未运行项目。不能用编译成功、0测试、历史记录或文档关键词检查替代本行为完成。

## 幂等与恢复


临时库/表/statement使用本测试唯一名称与端口；重跑只清理本任务创建的对象。测试finally/RAII清理SQL对象、客户端、listener和子进程，子进程退出后再释放自己的槽位；失败和中断也执行。不要删除其他会话锁、缓存或无关工作区修改。对生产改动的调整使用局部差异，不自动回滚他人改动。

## 进度（Progress）


- [x] 当前消费者、失败断言和真实运行入口已核对。
- [x] 本行为的代码与独立测试已完成。
- [x] 聚焦验证、清理、Ready检查和差异审阅已记录。

## 发现与证据（Surprises & Discoveries）


规划时只完成源码阅读，未运行本任务测试。实施者在此追加实际发现、命令、退出码和适用限制，不将预期写成观测。

## 决策记录（Decision Log）


2026-10-24，计划作者：采用现有共享内核、连接私有身份和独立测试；此任务只交付上文一个行为。实施中的必要局部决策及依据写在本节，涉及新增需求或范围变化须先向用户确认是否写回来源，不自动修改来源路线图。

## 结果与复盘（Outcomes & Retrospective）


已完成真实两端94场景差分和首批/相邻回归，无生产变更。完成时记录用户可观察结果、兼容性/正确性/性能风险、残留范围外缺口和实际交接路径；本文件是遵守PLANS.md的活动ExecPlan，plan.md始终只读。

## 完成


仅在适用行为验证通过且证据完整后改为已完成。若环境等无关原因阻止必要验证，允许状态已完成，待回归并明确缺失项，保留本文件；本行为自身失败不能借此宣布完成。无法突破的阻塞只在本文件标已阻塞，记录原因、检查与可独立继续的范围，不递归扩建依赖。

有代码、测试、配置或实际说明变更时，使用 $git-commit 仅提交本任务变更，排除工作区已有删除及其他任务文件；只改编号任务状态或删除则不调用提交。最终按Agent Output Contract报告文件、验证范围、风险、确切命令、未验证项和槽位。完成证据报告后删除本编号任务文件；待回归/阻塞时保留。删除前提交中保留本任务当前证据，最终回复报告可追溯提交SHA。禁止修改或删除plan.md，不把prompt.md作为进度记录。

2026-10-24：HEAD 899335615b01ae366796a7ae0550b72922900008；批次9提交内状态已完成，含1/11/1真实测试及18.4参考证据，依赖满足。已读执行/RustCodeGraph技能、PLANS、server约束与testing-flow；无doc.go及verify-profile技能。新增差分消费者和同一Python夹具模式，未改生产逻辑。既有1–9任务删除不纳入提交。

2026-10-24 实际基线：bash /tmp/pg-task10-slot.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_integer_differential -- --nocapture'，退出101，实际1测试因缺PG18_REFERENCE_DSN失败，证明不跳过。日志/tmp/pg-task10-baseline.log。首次带真实参考的同过滤命令退出101，裸SELECT 1恢复探针描述不同（AsterSQL int8、PG int4）；另暴露清理后listener_addr为None的夹具错误。未改变生产逻辑，恢复探针显式1::int4，使其属于本批显式cast契约；保存原差异为范围外建议。日志/tmp/pg-task10-differential.log。

WIP通过：bash /tmp/pg-task10-reference.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_integer_differential -- --nocapture' 退出0，1测试/90场景。随后新增int别名、嵌套收窄/溢出及恢复4场景，并改善失败清理：Rust guard关闭stdin让Python finally运行，Python一端清理失败仍尝试另一端。

最终Ready命令：bash /tmp/pg-task10-reference.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_integer_differential -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_ -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_introspection_namespace_session_state -- --nocapture' 退出0；分别实际1/10/1/1测试全部通过，无ignored。日志/tmp/pg-task10-ready.log。make lint 退出0（/tmp/pg-task10-lint.log）；git diff --check 退出0。独立Cargo阶段均动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；原子mkdir后写计划/任务/会话/PID，进程结束后释放，锁不存在，缓存保留。

真实参考：独立Homebrew initdb与pg_ctl临时TCP实例55470；version()=PostgreSQL 18.4 (Homebrew) on aarch64-apple-darwin25.4.0, compiled by Apple clang version 21.0.0 (clang-2100.0.123.102), 64-bit；server_version_num=180004；libpq=180000。临时实例已停止、目录删除，其他参考服务未动。最终每次差分94场景：value18/typed_null4/zero_rows2/overflow13/invalid8/prepare5/describe4/reexecute9/recovery22/transaction4/write1/readback1/negative_self_check3。值/OID/SQLSTATE负向自检仅改变AsterSQL内存观测副本，均检测到差异；两端原观测保留。表/连接/结果finally清理，双listener关闭端口拒连；MySQL前中后真实SELECT/CAST及prepared通过。

文件：pg_client_integration_test.rs注册真实差分测试、同Server MySQL检查和RAII；testdata/pg_core_libpq.py复用夹具新增两端同输入模式、全字段描述/值/SQLSTATE及计数；docs/postgresql-protocol-first-phase.md只记录本次真实能力/命令/版本/限制；本编号文件记录证据。无生产代码修改、无新增依赖，未触发bazel_prepare，未运行bazel_lint_changed。fmt后确认无其他Rust文件差异；未触碰只读plan.md、来源蓝图和路线图。自审无DSN/密码写入日志或提交。

风险与未验证：生产行为/性能不变；测试新增必要PG18_REFERENCE_DSN环境依赖。未验证18.6、生产鉴权、RealTiKV分布式存储、性能、完整错误消息/位置、表OID/列来源；portal原始字段由批次9实际TCP证据覆盖。本次只验证显式整数cast；默认裸整数定型差异范围外，建议另立任务。无强制生产修改；verify-profile技能缺失按根WIP/Ready范围执行。只提交本任务4文件（含本证据），排除既有1–9任务删除；交付后删除本任务文件，plan.md不变。

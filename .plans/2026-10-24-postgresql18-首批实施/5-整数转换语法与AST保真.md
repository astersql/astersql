# 任务 5: 整数转换语法与AST保真

批次：【批次 5】依赖：批次 4；承接此前已完成的代码与验证

状态：未开始

目的：在PG解析入口接入int4/int8的::与CAST语法，输出共享Cast并保持参数和原始位置。

来源任务：蓝图2、3；路线图类型转换语法及解析方案决策；用户2026-10-24首批实施要求。

预计会话范围：围绕一个可验收行为，仅修改下列生产链和独立测试；前序依赖满足后可独立阅读本文件执行，不重新展开八章。若调用链事实要求超出本行为，记录阻塞与证据，不扩建完整子系统。

## 文件


- 修改：`pkg/parser/grammar/main.astergram`。
- 修改：`pkg/parser/lexer.rs`。
- 修改：`pkg/parser/yy_parser.rs`。
- 修改：`pkg/parser/parser_actions/expression.rs`。
- 修改：`pkg/parser/ast/lib.rs`。
- 生成：`pkg/parser/generated/main_tables.rs`。
- 生成：`pkg/parser/generated/lexer_tokens.rs`。
- 测试：`pkg/parser/parser_test.rs`。
- 测试：`pkg/server/pg_query_test.rs`。
- 修改：`pkg/server/pg_name.rs`。
- 修改：`pkg/server/pg_sql.rs`。
- 修改：`pkg/server/pg_result.rs`。

## 上下文


正式Rust输入是grammar/main.astergram，动作在parser_actions/expression.rs；parser.y的Go动作不能证明Rust接线。ExprKind::Cast现有Tp/Expr可复用，新增源信息必须明确标为本任务实现。

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


行为：在PG解析入口接入int4/int8的::与CAST语法，输出共享Cast并保持参数和原始位置。

新增或修改的失败验证测试：pg_core_integer_cast_syntax; pg_core_integer_cast_parse_boundary；归属上述独立测试文件。类型/上下文测试可增加辅助断言，但不能替代真实SQL执行。

失败验证命令（先注册新测试，再执行）：

    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture

预期失败与例外（基于源码推断，实施时以实际运行核实）：SELECT 1::int4 及 CAST(1 AS int4) 当前没有完整PG语法/身份接线；树形、目标宽度、参数位置或前置PG门禁任一不保真即失败。本任务不以尚未实现的求值值/OID作为通过前提。

通过验证使用完全相同的命令与断言，不删除边界、不降级期待。模拟策略：使用真实canonical会话/共享内核，协议行为通过真实TCP；客户端用真实libpq。只允许测试构建下观察真实路径，不mock SQL结果、类型和错误，不用sleep猜测执行阶段。

## 步骤与里程碑


里程碑一：在实际消费者处注册上述测试，记录基线、失败阶段和有效测试数量；接口编译失败之后仍需取得适用的运行期失败证据。若已存在正确实现，保留当前通过记录，新增缺失回归，不做无意义生产改动。

里程碑二：完成下列本行为实现，并让同一组断言通过。

先用同一组真实解析测试对比共享Cast扩展与受控PG入口最小原型：要求嵌套树形、参数原始偏移、目标int4/int8身份和前置门禁可承接。选择共享Rust语法/AST扩展；若遇到可复现的载体障碍，记录证据后调整本任务内最小承载，禁止产出纯方案文档即结束。
实现仅PG启用的 :: 后缀转换及 CAST(expr AS int4/integer/int/int8/bigint)，两个语法落到同一共享规范转换。测试 SELECT 1::int4::int8、SELECT ((1)::int4)::int8、SELECT (1+2)::int4、SELECT 1+2::int4 的树形和优先级；不得删去内层转换。
保留原目标拼写、原SQL跨度和$n身份；字符串/注释中的::不变，($1)::int4原偏移可追踪。缺类型 SELECT 1:: 应为42601；错误位置若提供，按原查询一基字符位置，UTF8不能用字节冒充。未知目标不能静默当text/int8；不开放其他类型族。
PG前置名称/命令门禁必须使用已接通的PG解析入口；MySQL入口拒绝新增::，其 CAST(... AS SIGNED) 维持现状。不得用正则替换为SIGNED丢失宽度。
修改.astergram后用实际parsergen生成并检查提交产物，不手写生成表、不改Go语法来代替Rust；若触及Go文件则额外履行Bazel触发要求。

里程碑三：只在测试通过后做必要局部整理，更新已改生产模块的现有 .rs.md 说明（存在且行为受影响时），再执行交付检查和差异自审。文档更新随代码走，不单独计作功能完成。

## 验证


具体crate与features：astersql-parser, astersql-server，默认features；不得关闭现有运行路径来缩减测试。

阶段内先运行：

    cargo fmt --all

行为通过命令：

    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture

语法生成命令（修改语法后、行为测试前执行generate，再fmt；完成时执行check；包与CLI已核实存在）：

    env -u LDFLAGS cargo run --locked -p astersql-parsergen --bin astersql-parsergen -- generate
    env -u LDFLAGS cargo run --locked -p astersql-parsergen --bin astersql-parsergen -- check

交付检查：

    make lint
    git diff --check

预期：新增解析树和真实listener拒绝路径测试通过，参数/位置保真、MySQL隔离，生成产物检查一致。

所需证据：当前HEAD/工作树标识、适用基线失败和修复后同断言通过记录、确切命令及退出码、有效测试数、真实输入/值/类型/错误/恢复与清理证据、实际修改文件及原因、Ready适用检查与未运行项目。不能用编译成功、0测试、历史记录或文档关键词检查替代本行为完成。

## 幂等与恢复


临时库/表/statement使用本测试唯一名称与端口；重跑只清理本任务创建的对象。测试finally/RAII清理SQL对象、客户端、listener和子进程，子进程退出后再释放自己的槽位；失败和中断也执行。不要删除其他会话锁、缓存或无关工作区修改。对生产改动的调整使用局部差异，不自动回滚他人改动。

## 进度（Progress）


- [ ] 当前消费者、失败断言和真实运行入口已核对。
- [ ] 本行为的代码与独立测试已完成。
- [ ] 聚焦验证、清理、Ready检查和差异审阅已记录。

## 发现与证据（Surprises & Discoveries）


规划时只完成源码阅读，未运行本任务测试。实施者在此追加实际发现、命令、退出码和适用限制，不将预期写成观测。

## 决策记录（Decision Log）


2026-10-24，计划作者：采用现有共享内核、连接私有身份和独立测试；此任务只交付上文一个行为。实施中的必要局部决策及依据写在本节，涉及新增需求或范围变化须先向用户确认是否写回来源，不自动修改来源路线图。

## 结果与复盘（Outcomes & Retrospective）


尚未实施，无功能通过结论。完成时记录用户可观察结果、兼容性/正确性/性能风险、残留范围外缺口和实际交接路径；本文件是遵守PLANS.md的活动ExecPlan，plan.md始终只读。

## 完成


仅在适用行为验证通过且证据完整后改为已完成。若环境等无关原因阻止必要验证，允许状态已完成，待回归并明确缺失项，保留本文件；本行为自身失败不能借此宣布完成。无法突破的阻塞只在本文件标已阻塞，记录原因、检查与可独立继续的范围，不递归扩建依赖。

有代码、测试、配置或实际说明变更时，使用 $git-commit 仅提交本任务变更，排除工作区已有删除及其他任务文件；只改编号任务状态或删除则不调用提交。最终按Agent Output Contract报告文件、验证范围、风险、确切命令、未验证项和槽位。完成证据报告后删除本编号任务文件；待回归/阻塞时保留。删除前提交中保留本任务当前证据，最终回复报告可追溯提交SHA。禁止修改或删除plan.md，不把prompt.md作为进度记录。

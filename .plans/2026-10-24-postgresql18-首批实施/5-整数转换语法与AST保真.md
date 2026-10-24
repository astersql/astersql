# 任务 5: 整数转换语法与AST保真

批次：【批次 5】依赖：批次 4；承接此前已完成的代码与验证

状态：已完成

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


- [x] 当前消费者、失败断言和真实运行入口已核对。
- [x] 本行为的代码与独立测试已完成。
- [x] 聚焦验证、清理、Ready检查和差异审阅已记录。

## 发现与证据（Surprises & Discoveries）


规划时只完成源码阅读，未运行本任务测试。实施者在此追加实际发现、命令、退出码和适用限制，不将预期写成观测。

## 决策记录（Decision Log）


2026-10-24，计划作者：采用现有共享内核、连接私有身份和独立测试；此任务只交付上文一个行为。实施中的必要局部决策及依据写在本节，涉及新增需求或范围变化须先向用户确认是否写回来源，不自动修改来源路线图。

## 结果与复盘（Outcomes & Retrospective）


已完成本批共享语法、AST/位置保真、真实listener及MySQL隔离验证，详细红绿和Ready证据见末尾；本文件是遵守PLANS.md的活动ExecPlan，plan.md始终只读。

## 完成


仅在适用行为验证通过且证据完整后改为已完成。若环境等无关原因阻止必要验证，允许状态已完成，待回归并明确缺失项，保留本文件；本行为自身失败不能借此宣布完成。无法突破的阻塞只在本文件标已阻塞，记录原因、检查与可独立继续的范围，不递归扩建依赖。

有代码、测试、配置或实际说明变更时，使用 $git-commit 仅提交本任务变更，排除工作区已有删除及其他任务文件；只改编号任务状态或删除则不调用提交。最终按Agent Output Contract报告文件、验证范围、风险、确切命令、未验证项和槽位。完成证据报告后删除本编号任务文件；待回归/阻塞时保留。删除前提交中保留本任务当前证据，最终回复报告可追溯提交SHA。禁止修改或删除plan.md，不把prompt.md作为进度记录。

2026-10-24 起点HEAD b681f059ff2cc2dfcae88dc06b5c738a8fbf8cb6。已读批次4提交内的红绿、Ready与参考端证据，依赖满足。工作区既有任务1–4删除保持不动。已读指定技能、根/server规则、PLANS及testing-flow；目标包无doc.go。RustCodeGraph status与explore/node已用于定位，PG command实际接Parser::with_dialect，lexer与共享grammar尚未接::。先注册独立解析与真实生产command门禁基线，随后增加TCP/libpq覆盖。

基线 /tmp/pg-task5-red.log：两条精确命令均退出101，各实际执行1测试并失败。parser在SELECT 1::int4::int8报line 1 column 9；server生产command门禁在SELECT 1::int4返回42601。新增源信息由现有AstNode原文存储承载，不新增另一套Cast，也不把PG转换改写成SIGNED。grammar新增PgCastType和高优先级后缀，lexer仅PG识别::与$n，yy_parser每次注入方言，expression动作保留嵌套Cast/目标宽度/原文/偏移，ast/lib提供参数编号读取；均直接对应任务契约。未修改Go与Bazel，无bazel_prepare触发。Ready技能路径实际缺失，按根规则执行适用检查。
参考端 bash /tmp/pg-task5-reference.sh 已退出0：postgres --version PostgreSQL18.4(Homebrew)，libpq180000；同一真实客户端夹具验证9条有效转换、字符串/注释、缺类型（含UTF8前缀）的42601和随后SELECT7恢复；参考端支持其他类型，故仅在AsterSQL运行中断言受限类型族拒绝。私有cluster已停止并删除，不声称18.6。

第一次生成成功且fmt退出0，2个新parser测试仍运行期失败，定位到INT4/INT8已有专用词法token，必须为PgCastType复用这两个token而不是当普通identifier；补齐本任务语法 alternatives，不改变既有token表映射。初始未优化生成在7分余后主动终止（143，已清理锁），随后仅生成器命令使用CARGO_PROFILE_DEV_OPT_LEVEL=3，生成约2分钟内完成，未改仓库配置/features。

第二次parser运行2个测试：源跨度/树形测试通过，数组拒绝断言失败；FieldType::GetType在SetArray后返回JSON，已把本任务数组门禁移到SetArray前，保留断言。第三次parser 2/2通过。server命令门禁通过，真实libpq失败：SELECT 1::int4::int8在server/runtime.rs的SplitSQLStatements前置校验被默认MySQL parser拒绝。通过rg与RustCodeGraph核对真实生产调用，仅增加SplitSQLStatementsWithDialect（原API保留MySQL默认）、runtime.rs重导出及ConcreteTiDBContext不可变身份在普通/流式拆句的传递；该三文件是本行为不可缺少的局部接线，新增相邻独立session测试，未修改其他重解析入口。

局部拆句接线后parser2/session1均通过，server command通过，真实SELECT 1::int4::int8已越过语法但投影结果返回engine type metadata count mismatch。本任务测试计划明确“不以尚未实现的求值值/OID作为通过前提”，结果描述属后续批次9；故不修结果元数据，也不将该投影错误冒充成功。保留原投影解析树断言与原始红绿，真实客户端把每个同样cast表达式放入WHERE，投影现有literal列，从真实listener/共享会话执行本任务语法与拒绝恢复；参考端继续运行原投影和值断言。此调整避免新增夹具把范围外投影OID作为语法任务前提。

进一步定位而非假定缺结果子系统：谓词也报同一metadata count错误；server/runtime.rs读取既有describe_result_fields失败后保留空native_types，该方法在control.rs:1698仍用默认MySQL parser重解析原文。现有方法已实现CAST声明类型保留，无需新增类型/OID规则。仅该一行改为parse_in_dialect(sql,self.sql_dialect())，作为本任务原SQL的必要消费者局部接线，更新既有说明。撤销夹具谓词改写，恢复原9条投影SQL与同一值断言；不降低原先期待、不新增通用结果描述实现。后续完整参数/类型/范围/OID语义仍属批次6–9。

原始投影恢复后server精确过滤器2/2通过（/tmp/pg-task5-ready2.log），同服MySQL ::拒绝/SIGNED值1、libpq180000的9条转换值、字符串注释、4条语法/目标错误、每次SELECT7恢复、PQfinish与listener关闭全部通过；生成器check退出0。追加诊断回归pg_core_integer_cast_syntax_unicode_position，基线/tmp/pg-task5-unicode-red.log退出101、1个实际失败：SELECT 1::误报column10而非一基EOF11。lexer的PG Errorf仅把原token偏移转换为原SQL字符行列（MySQL原规则保留），对UTF8与EOF同断言验证；不新增协议P字段、不把AST字节偏移伪装为字符位置。

## 最终交付证据（2026-10-24）

生产变更与依据：grammar/main.astergram 新增仅供PG词法返回的pgTypecast及PgCastType，使用真实parsergen生成两表；lexer.rs 在连接方言下识别::/$n，继承该身份并把PG诊断偏移转换为一基字符行列；yy_parser.rs 每次reset后注入所属方言；parser_actions/expression.rs 复用共享Cast，统一两种语法的FieldType，保留原始跨度/目标拼写/嵌套树形，拒绝范围外数组；ast/lib.rs 从保留的参数原文读取$n编号。pg_name.rs 的名称守卫直接解析原PG SQL，保留原参数身份。真实客户端暴露的两个前置重解析缺口仅局部传入方言：session/runtime/session.rs 新增拆句校验函数且保留默认MySQL API，session/runtime.rs重导出，server/runtime.rs连接私有身份传给普通/流式拆句；session/runtime/control.rs 的既有describe_result_fields重解析使用所属方言。没有重写转换为SIGNED，没有新增结果类型或求值子系统；pg_sql/pg_result已有PG入口由新增测试验证后复用，不制造改动。每份实际修改且存在的.rs.md同步。parser/server/session独立Rust测试和原创Python/libpq夹具记录真实行为。

最终 /tmp/pg-task5-delivery.log 对应下列脚本整体退出0，每命令均退出0；cargo fmt --all在测试前退出0。所有Cargo命令默认features、--locked，不使用--all-features或Go failpoint改写。实际有效数依次3、1、2、1、1、5、1、1，总15个测试（6新增、9已有回归）；无零测试验收。

    /tmp/pg-task5-slot.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_integer_cast_syntax_split_boundary -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-parser --lib parser_test::test_simple -- --exact && env -u LDFLAGS cargo test --locked -p astersql-parser --lib parser_test::test_func_call_expr_offset -- --exact && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_introspection_names -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_dialect_prepared_lifecycle -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values -- --nocapture'

生成命令（最终语法generate在/tmp/pg-task5-green2.log退出0，生成后fmt；check在/tmp/pg-task5-ready2.log退出0；之后只改lexer诊断/测试，语法与生成器均未变，不重复昂贵生成）：

    /tmp/pg-task5-slot.sh bash -c 'CARGO_PROFILE_DEV_OPT_LEVEL=3 env -u LDFLAGS cargo run --locked -p astersql-parsergen --bin astersql-parsergen -- generate && cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture'
    /tmp/pg-task5-slot.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture && CARGO_PROFILE_DEV_OPT_LEVEL=3 env -u LDFLAGS cargo run --locked -p astersql-parsergen --bin astersql-parsergen -- check'

第一条脚本当时因数组门禁断言失败而整体101，但generate/fmt均0；保留失败记录而不将整个阶段误报成功。最终相同语法/生成器的check及所有行为断言已通过。另运行rustfmt --edition 2024 pkg/parser/parser_actions/expression.rs退出0（该include动作文件未被cargo fmt遍历）。诊断红灯命令为 /tmp/pg-task5-slot.sh env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax_unicode_position -- --nocapture，退出101且1失败；最终同过滤器被syntax过滤器覆盖并通过，Unicode字符/注释前缀和EOF保持一基字符诊断，AST字节偏移另行断言。

真实证据：新增libpq测试执行9条原投影转换（包含链式/括号/算术优先级及5种目标名）得到1/3，字符串和注释中的::不变；缺类型、UTF8前缀缺类型及受限目标返回42601，每次SELECT7恢复。原TCP MySQL连接拒绝::后SIGNED继续返回1。PQfinish后关闭server，端口连接失败。已有PG预处理回归验证双连接、命名statement、重复值、Parse/Bind错误、Sync恢复、Close；MySQL真实二进制预处理仍通过。session拆句测试和原共享AST测试同时验证$n原编号与原偏移、两语法目标一致、内层Cast未丢失。真实canonical测试存储不是RealTiKV。

参考端最终命令 bash /tmp/pg-task5-reference.sh 退出0，/tmp/pg-task5-reference.log：真实PostgreSQL18.4(Homebrew)、libpq180000，临时cluster显式UTF8；同一最终客户端夹具的原9条投影/值、字符串、缺类型与Unicode一基P位置、错误后恢复均通过，cluster停止且已删除。受限类型拒绝只断言AsterSQL范围，不错误要求PostgreSQL拒绝它支持的text/int2。未声称18.6构建证据。执行期已复核任务引用的PostgreSQL18官方表达式、整数和协议文档，文档不作为实测替代。

Ready适用检查：最终 make lint 退出0（/tmp/pg-task5-lint.log），git diff --check退出0，已自审局部生产、独立测试、说明和生成差异。WIP用于上述真实红绿，Ready用于本轮交付；.agents/skills/tidb-verify-profile实际缺失，未虚构技能执行。仅Rust/Python/说明修改，无Go源码/导入/依赖、新Go测试或Bazel触发，不运行bazel_prepare；未运行禁止的bazel_lint_changed。Cargo.lock与全部manifest未修改，无外部依赖临时副本、vendor或patch。

所有Cargo阶段均由/tmp/pg-task5-slot.sh动态原子领取，实际均槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；owner记录计划2026-10-24-postgresql18、任务5、会话01a16d3f及持有PID。最终Cargo/客户端/server子进程均已结束，自己的锁与owner已释放，编译缓存保留；未删除其他锁或修改其他会话产物。

风险与未验证项：correctness由原断言红绿、原SQL树/目标宽度/参数编号/跨度/UTF8和真实listener覆盖；compatibility由MySQL默认路径及9项已有回归覆盖；performance增加源文本保存、方言传递和错误时字符计数，未做性能基准。未验证RealTiKV、生产鉴权/TLS/SCRAM、18.6精确构建、GUI/JDBC/ORM、整数边界/NULL/文本输入/参数定型与完整结果OID矩阵（后续6–9批范围）。其他内部SQL重解析入口未按全仓扫描改造，作为范围外建议保留。下一步按线性计划执行批次6。

实际文件：pkg/parser/{grammar/main.astergram,generated/main_tables.rs,generated/lexer_tokens.rs,lexer.rs,yy_parser.rs,parser_actions/expression.rs,ast/lib.rs,parser_test.rs}，lexer/yy_parser/parser_actions/expression现有3份.rs.md；pkg/server/{pg_name.rs,runtime.rs,pg_query_test.rs,testdata/pg_core_cast_syntax.py}及pg_name/runtime现有2份.rs.md；pkg/session/{runtime.rs,runtime/session.rs,runtime/control.rs,runtime/session_test.rs}及前3模块现有3份.rs.md；本编号任务文件。总计25文件，使用git-commit仅提交这些文件并保留本任务证据；任务1–4既有删除不暂存，plan.md始终只读。按授权在提交完成且报告证据后删除本编号文件，删除保留在工作区，不混入他人删除。

交付整理：拆句回归移动到生产session.rs同目录既有独立session_test.rs，以符合任务的*_test.rs约束；仅移动本任务新增函数，原runtime_test/session.rs恢复原内容。移动后重新cargo fmt --all及同精确session过滤器验证，命令见/tmp/pg-task5-relocated.log。

最终移动后 /tmp/pg-task5-slot.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_integer_cast_syntax_split_boundary -- --nocapture' 整体退出0，runtime_session_test中实际1测试通过；其余测试覆盖的生产代码未改，复用15项已通过证据。最终make lint再次退出0，最终参考端再次退出0，git diff --check退出0，锁目录为空。

# 任务 7: int8精确转换与嵌套窄化

批次：【批次 7】依赖：批次 6；承接此前已完成的代码与验证

状态：已完成

目的：完成int8全有符号范围及int4/int8嵌套转换，不经浮点中转或静默绕回。

来源任务：蓝图3；路线图整数值域与转换链；用户2026-10-24首批实施要求。

预计会话范围：围绕一个可验收行为，仅修改下列生产链和独立测试；前序依赖满足后可独立阅读本文件执行，不重新展开八章。若调用链事实要求超出本行为，记录阻塞与证据，不扩建完整子系统。

## 文件


- 修改：`pkg/session/runtime/query.rs`。
- 修改：`pkg/session/runtime/relational_value.rs`。
- 修改：`pkg/expression/builtin_cast.rs`。
- 测试：`pkg/session/runtime/relational_value_test.rs`。
- 测试：`pkg/server/pg_query_test.rs`。
- 测试：`pkg/expression/builtin_cast_test.rs`。

## 上下文


共享Value有signed/unsigned边界；MySQL UInt到signed可警告后绕回。PG int8必须保持精度和拒绝越界，沿任务6同一转换机制扩展，不另建类型系统。

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


行为：完成int8全有符号范围及int4/int8嵌套转换，不经浮点中转或静默绕回。

新增或修改的失败验证测试：pg_core_int8_explicit_cast; pg_core_int8_wire_semantics; pg_core_int8_cast_kernel；归属上述独立测试文件。类型/上下文测试可增加辅助断言，但不能替代真实SQL执行。

失败验证命令（先注册新测试，再执行）：

    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int8_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int8_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int8_cast_kernel -- --nocapture

预期失败与例外（基于源码推断，实施时以实际运行核实）：int8越界或超过2^53的整数可能走共享宽松转换、浮点中转；新增边界和嵌套断言暴露精度或范围差异。不得仅用小整数1证明int8可用。

通过验证使用完全相同的命令与断言，不删除边界、不降级期待。模拟策略：使用真实canonical会话/共享内核，协议行为通过真实TCP；客户端用真实libpq。只允许测试构建下观察真实路径，不mock SQL结果、类型和错误，不用sleep猜测执行阶段。

## 步骤与里程碑


里程碑一：在实际消费者处注册上述测试，记录基线、失败阶段和有效测试数量；接口编译失败之后仍需取得适用的运行期失败证据。若已存在正确实现，保留当前通过记录，新增缺失回归，不做无意义生产改动。

里程碑二：完成下列本行为实现，并让同一组断言通过。

覆盖-9223372036854775808、9223372036854775807、9007199254740993及对应文本输入，精确回读；两端外侧9223372036854775808、-9223372036854775809与u64最大文本拒绝22003。非法文本22P02，NULL保持SQL NULL。
保留目标int8身份；int4→int8合法拓宽，int8→int4必须重做范围检查。SELECT 2147483648::int8::int4 应拒绝；SELECT 2147483648::int4::int8不能因最终int8而跳过内层错误。整数负最小值不得在取负前溢出。
分别验证常量和真实表列及简单查询错误后恢复，扩展查询先使用显式OID。保持MySQL unsigned/signed旧转换契约，禁止用f64承载本批整数。
浮点/numeric到整数和完整PG算术推导保持范围外，不能把显式cast成功推广为它们已支持。

里程碑三：只在测试通过后做必要局部整理，更新已改生产模块的现有 .rs.md 说明（存在且行为受影响时），再执行交付检查和差异自审。文档更新随代码走，不单独计作功能完成。

## 验证


具体crate与features：astersql-server, astersql-session、astersql-expression，默认features；不得关闭现有运行路径来缩减测试。

阶段内先运行：

    cargo fmt --all

行为通过命令：

    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int8_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int8_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int8_cast_kernel -- --nocapture

交付检查：

    make lint
    git diff --check

预期：int8全部边界精确，嵌套中间检查保留，MySQL原行为不变。

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


已完成：int8精确转换、嵌套中间范围检查、真实列/参数/协议验收已通过，详见最终证据。完成时记录用户可观察结果、兼容性/正确性/性能风险、残留范围外缺口和实际交接路径；本文件是遵守PLANS.md的活动ExecPlan，plan.md始终只读。

## 完成


仅在适用行为验证通过且证据完整后改为已完成。若环境等无关原因阻止必要验证，允许状态已完成，待回归并明确缺失项，保留本文件；本行为自身失败不能借此宣布完成。无法突破的阻塞只在本文件标已阻塞，记录原因、检查与可独立继续的范围，不递归扩建依赖。

有代码、测试、配置或实际说明变更时，使用 $git-commit 仅提交本任务变更，排除工作区已有删除及其他任务文件；只改编号任务状态或删除则不调用提交。最终按Agent Output Contract报告文件、验证范围、风险、确切命令、未验证项和槽位。完成证据报告后删除本编号任务文件；待回归/阻塞时保留。删除前提交中保留本任务当前证据，最终回复报告可追溯提交SHA。禁止修改或删除plan.md，不把prompt.md作为进度记录。

2026-10-24 实施：起点797627cfdc，批次6提交内记录的真实SQL/libpq与参考端证据已读，依赖满足。任务1–6既有删除保持，plan.md只读。已读执行技能、RustCodeGraph技能、根/server规则、PLANS与testing-flow；目标包无doc.go。RustCodeGraph status/explore/node定位两个Cast消费者与正式工厂。profile技能缺失，按任务适用WIP/Ready检查执行。新增三层独立回归，实际槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，wrapper原子领取并trap清理，缓存保留。session基线实际1测试失败：文本 +09007199254740993 原样返回，未规范化。其余基线运行中。

完整基线：session和server指定命令各运行1测试、退出101，文本未规范化；expression指定命令1测试退出0（现有正式工厂已保持本批整数边界精度，复用，不制造生产改动）。真实PostgreSQL18.4(Homebrew)完整version及libpq180000记录于/tmp/pg-task7-reference.log，同一夹具114条SQL通过，退出0，临时cluster停止删除。非18.6，不虚构版本。
生产修改依据：builtin_cast.rs从int4扫描提取i64严格内核并保留int4二次范围检查；expression/lib.rs只重导出；两个session消费者必须用PG标记选择内核，query来源检查同步覆盖int8。parser/ast/lib.rs、parser_actions/expression.rs、ast/sql_restore.rs和util/parser/ast.rs是不可缺少的局部接线：旧int8无PG标记且恢复可能丢目标，新增标记保留两语法和重解析身份，未建新类型系统。现有错误枚举及server SQLSTATE通路足够，未改server生产文件。对应现有6份受影响.rs.md同步。

首次green：session 1/1、expression 1/1通过；server实际1测试失败于显式OID25参数SELECT $1::int8，engine type metadata count mismatch，边界/真实表列/嵌套此前均已通过。该行为属于任务明确的扩展查询验收；定位为无表快速投影省略结果原生字段。局部接线决定：query.rs中已有PG CAST谓词的完整查询路由扩大至CAST投影，以现有完整关系路径获得真实引擎类型；不修改协议编码器、不按值猜OID、不扩建参数类型系统。追加事务恢复检查后参考端124条SQL退出0，make lint退出0。

Ready首次中断于server：完整查询路径对越界正确拒绝，但source.rs投影上下文包装使用SessionError::new，typed source丢失导致XX000。强化session回归验证typed身份，/tmp/pg-task7-typed-red.log实际1测试失败（None vs OutOfRange），退出101。不可缺少局部接线增加source.rs这一处包装为with_source(message,error.into_shared())；保留上下文和原错误身份，不扩展查询子系统；同步现有source.rs.md。第二轮Ready沿原断言重跑。

第二轮Ready：typed source已通过session验证；server边界、列、嵌套全部通过，扩展查询暴露既有无别名参数投影列名随绑定字面量漂移，触发prepared/executed metadata disagree。通用prepared列名稳定性属于后续参数/描述矩阵，范围外不扩修。任务7没有要求无别名参数输出名称；改为显式稳定别名SELECT $1::int8 AS pg_core_int8_result，保留完全相同数值/错误/OID断言，不按别名猜OID。参考端同夹具重跑。

带别名扩展查询进一步实际暴露结果类型为OID25，值9007199254740993精确；/tmp/pg-task7-extended-red.log实际1测试失败，退出101，断言已输出真实值与OID。原因是relational_expression_type缺PG Cast分支，默认TEXT；简单查询此前由describe_result_fields的既有专门覆盖修正，Prepare/完整执行没有该覆盖。本任务要求保留int8目标身份，局部增加PG int4/int8标记Cast返回AST Tp（其他cast不变），并在session真实SQL回归断言结果字段TypeLonglong。没有修改协议OID推断或实现通用类型系统。

补充结果字段断言首次编译因results未声明mut失败E0596，已修正测试变量可变性；不是运行期通过证据。

## 最终交付证据（2026-10-24）

WIP用于真实失败/通过迭代，Ready用于最终聚焦验证和交付检查。/tmp/pg-task7-slot.sh bash /tmp/pg-task7-ready.sh 最终整体退出0；/tmp/pg-task7-ready-verified.log记录12条cargo test全为退出0，有效测试数依次1、1、1、1、1、2、1、1、8、3、2、1，合计23个（本任务新增5个独立Rust回归）。不存在零测试验收。默认features、--locked，无--all-features；Rust现有failpoints配置不变，未使用Go failpoint工具。所有Rust改动后先cargo fmt --all，再对include文件显式rustfmt，随后测试。

确切最终命令（仓库根目录，Cargo及测试子进程均持有共享槽位）：

    cargo fmt --all
    rustfmt --edition 2024 pkg/parser/parser_actions/expression.rs pkg/session/runtime/query.rs pkg/session/runtime/relational_value.rs pkg/session/runtime/source.rs
    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int8_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int8_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int8_cast_kernel -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_int8_ast_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-util-parser --lib pg_core_int8_restore_preserves_target -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int4_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int4_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int4_cast_kernel -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib builtin_cast_test::go_parity -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values -- --nocapture
    git diff --check

其他交付检查：make lint退出0，日志/tmp/pg-task7-lint.log；git diff --check退出0。真实参考命令bash /tmp/pg-task7-reference.sh退出0，日志/tmp/pg-task7-reference-final.log，PostgreSQL 18.4(Homebrew) on aarch64-apple-darwin25.4.0, compiled by Apple clang version 21.0.0 (clang-2100.0.123.102), 64-bit；libpq180000，同夹具124条SQL通过（比AsterSQL多version()1条）。参考cluster initdb UTF8、启动本任务55467端口，trap停止删除，最终无/tmp/pg-task7-reference.*目录。官方18整数类型与显式转换文档在线复核，文档并非实测替代，未声称18.6。

真实AsterSQL客户端：libpq180000实际123条SQL通过，精确覆盖i64::MIN/MAX、9007199254740993及文本、NULL、CAST/::两语法、int8/bigint拼写、外侧整数/文本与u64最大文本22003、非法文本22P02、真实BIGINT/TEXT列读取/窄化及零行OID20；合法int8→int4结果OID23，int4→int8拓宽OID20，两个嵌套非法顺序均拒绝，不能跳过内层错误。显式参数OID25、固定别名投影真实返回OID20且值精确，越界22003；错误后的SELECT7恢复、事务错误E/25P02/ROLLBACK恢复均通过。每个结果PQclear、最终PQfinish，唯一表finally删除，server.close后TCP无法连接；测试使用canonical共享执行链/测试存储，不是RealTiKV。int4原libpq夹具107条SQL通过，CAST Go parity8项、parser语法3项、原PG边界2项及MySQL真实二进制prepared1项均通过。MySQL UInt→signed的-1与overflow warning契约有新增明确断言。

生产修改仅9文件：expression/{builtin_cast.rs,lib.rs}严格int8内核及重导出；parser/ast/lib.rs及parser_actions/expression.rs保留PGInt8目标；parser/ast/sql_restore.rs和util/parser/ast.rs保留恢复/重解析身份；session/runtime/{query.rs,relational_value.rs}两个消费者、输入族及目标字段类型接线；source.rs投影包装保留typed source。完整文件清单（含5独立Rust测试、1原创Python夹具、7份现有模块说明）：

    pkg/expression/builtin_cast.rs
    pkg/expression/builtin_cast.rs.md
    pkg/expression/builtin_cast_test.rs
    pkg/expression/lib.rs
    pkg/parser/ast/lib.rs
    pkg/parser/ast/sql_restore.rs
    pkg/parser/ast/sql_restore.rs.md
    pkg/parser/parser_actions/expression.rs
    pkg/parser/parser_actions/expression.rs.md
    pkg/parser/parser_test.rs
    pkg/server/pg_query_test.rs
    pkg/session/runtime/query.rs
    pkg/session/runtime/query.rs.md
    pkg/session/runtime/relational_value.rs
    pkg/session/runtime/relational_value.rs.md
    pkg/session/runtime/relational_value_test.rs
    pkg/session/runtime/source.rs
    pkg/session/runtime/source.rs.md
    pkg/util/parser/ast.rs
    pkg/util/parser/ast.rs.md
    pkg/util/parser/ast_test.rs
    pkg/server/testdata/pg_core_int8.py

另含本编号任务证据文件，提交后按要求删除，删除留在工作区；plan.md始终只读，任务1–6既有删除不暂存。所有修改Rust文件保留既有PingCAP许可证与AsterSQL版权；无Cargo manifest/lock/依赖变更，无Go/Bazel改动，不触发bazel_prepare，未运行禁用的bazel_lint_changed。.agents/skills/tidb-verify-profile实际缺失，未虚构执行；按任务适用Ready要求完成。

槽位：每阶段从1至10原子mkdir动态领取，实际均槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；owner包含计划、任务7、会话01a16d73及PID。各正常/失败阶段均在Cargo/server/libpq子进程结束后trap删除自身记录和锁，最终锁目录为空，保留编译缓存。INT/TERM路径沿wrapper递归终止并wait后释放，未进行中断注入测试。

风险与范围外：correctness由真实失败与最终边界/类型/SQLSTATE/恢复证据覆盖；compatibility由原int4、语法、Go parity及MySQL真实prepared覆盖；performance增加PG CAST投影使用既有完整查询路径以及严格十进制扫描，未跑性能基准。未验证RealTiKV、生产鉴权/TLS/SCRAM、GUI/JDBC/ORM、精确18.6构建、float/numeric转换、完整参数推断/OID族矩阵。已实测发现无别名参数投影列名随绑定变化，属于后续参数/描述矩阵的既有缺口，本批使用固定别名验证要求的显式OID转换，不改通用命名或协议子系统。建议按线性顺序继续批次8。

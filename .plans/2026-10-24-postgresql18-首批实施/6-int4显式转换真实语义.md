# 任务 6: int4显式转换真实语义

批次：【批次 6】依赖：批次 5；承接此前已完成的代码与验证

状态：已完成

目的：在真实执行链实现int4的整数/文本显式转换、NULL和越界错误。

来源任务：蓝图3、5；路线图整数与显式转换契约；用户2026-10-24首批实施要求。

预计会话范围：围绕一个可验收行为，仅修改下列生产链和独立测试；前序依赖满足后可独立阅读本文件执行，不重新展开八章。若调用链事实要求超出本行为，记录阻塞与证据，不扩建完整子系统。

## 文件


- 修改：`pkg/session/runtime/query.rs`。
- 修改：`pkg/session/runtime/relational_value.rs`。
- 修改：`pkg/session/runtime/planning.rs`。
- 修改：`pkg/planner/core/expression_rewriter.rs`。
- 修改：`pkg/expression/builtin_cast.rs`。
- 修改：`pkg/server/pg_conn.rs`。
- 测试：`pkg/session/runtime/relational_value_test.rs`。
- 测试：`pkg/server/pg_query_test.rs`。
- 测试：`pkg/expression/builtin_cast_test.rs`。

## 上下文


共享MySQL cast存在警告后截断/绕回行为，不能直接作为PG转换；query.rs和relational_value.rs都有Cast消费者。应复用共享算子并按连接身份选择错误/值域规则。

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


行为：在真实执行链实现int4的整数/文本显式转换、NULL和越界错误。

新增或修改的失败验证测试：pg_core_int4_explicit_cast; pg_core_int4_wire_semantics; pg_core_int4_cast_kernel；归属上述独立测试文件。类型/上下文测试可增加辅助断言，但不能替代真实SQL执行。

失败验证命令（先注册新测试，再执行）：

    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int4_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int4_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int4_cast_kernel -- --nocapture

预期失败与例外（基于源码推断，实施时以实际运行核实）：PG的int4窄化可能被共享signed i64转换接受，非法文本可能走MySQL宽松转换；先证明范围或文本错误与真实PG目标不符，再改生产逻辑。

通过验证使用完全相同的命令与断言，不删除边界、不降级期待。模拟策略：使用真实canonical会话/共享内核，协议行为通过真实TCP；客户端用真实libpq。只允许测试构建下观察真实路径，不mock SQL结果、类型和错误，不用sleep猜测执行阶段。

## 步骤与里程碑


里程碑一：在实际消费者处注册上述测试，记录基线、失败阶段和有效测试数量；接口编译失败之后仍需取得适用的运行期失败证据。若已存在正确实现，保留当前通过记录，新增缺失回归，不做无意义生产改动。

里程碑二：完成下列本行为实现，并让同一组断言通过。

只覆盖整数与十进制文本到int4：-2147483648、0、2147483647成功；2147483648、-2147483649及更大整数拒绝22003；文本空串、12x等非法输入拒绝22P02；NULL::int4仍是SQL NULL。两种CAST语法结果相同。
分别跑常量投影和真实表列投影，防止只修一个解释分支。typed NULL和零行不靠首行值决定目标类型。对显式text空白/符号输入先按参考端真实采集固定目标，不照搬Rust parse默认词法；本批不开放float/numeric到int4，其入口保持明确不支持。
在共享表达式构建/求值及实际session Cast消费者接入PG严格规则，必要时保留明确的目标类型标记。映射22003/22P02须从有类型的错误依据传递，不能对任意错误文本做宽泛匹配，也不能改变MySQL错误/告警语义。
真实PG显式事务中转换错误进入E，后续25P02，ROLLBACK后SELECT正常；无事务错误后下一查询正常。复用现有事务机制，不新增事务系统。

里程碑三：只在测试通过后做必要局部整理，更新已改生产模块的现有 .rs.md 说明（存在且行为受影响时），再执行交付检查和差异自审。文档更新随代码走，不单独计作功能完成。

## 验证


具体crate与features：astersql-server, astersql-session、astersql-expression，默认features；不得关闭现有运行路径来缩减测试。

阶段内先运行：

    cargo fmt --all

行为通过命令：

    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int4_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int4_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int4_cast_kernel -- --nocapture

交付检查：

    make lint
    git diff --check

预期：int4边界、非法输入、NULL、真实表读取和错误后恢复全部实际执行，MySQL CAST AS SIGNED回归不变。

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


本行为已实施并通过真实SQL、TCP/libpq及参考端验证，详见最终证据。完成时记录用户可观察结果、兼容性/正确性/性能风险、残留范围外缺口和实际交接路径；本文件是遵守PLANS.md的活动ExecPlan，plan.md始终只读。

## 完成


仅在适用行为验证通过且证据完整后改为已完成。若环境等无关原因阻止必要验证，允许状态已完成，待回归并明确缺失项，保留本文件；本行为自身失败不能借此宣布完成。无法突破的阻塞只在本文件标已阻塞，记录原因、检查与可独立继续的范围，不递归扩建依赖。

有代码、测试、配置或实际说明变更时，使用 $git-commit 仅提交本任务变更，排除工作区已有删除及其他任务文件；只改编号任务状态或删除则不调用提交。最终按Agent Output Contract报告文件、验证范围、风险、确切命令、未验证项和槽位。完成证据报告后删除本编号任务文件；待回归/阻塞时保留。删除前提交中保留本任务当前证据，最终回复报告可追溯提交SHA。禁止修改或删除plan.md，不把prompt.md作为进度记录。

2026-10-24：起点 HEAD 40f232b42095e92ec40a5e86c9738d5ac215bbe8，已读批次5提交内通过证据，依赖满足。任务1–5既有删除保持原样；plan.md只读。已读指定技能、根/server规则、PLANS、testing-flow；目标根包无doc.go；RustCodeGraph status/sync/explore已执行，索引同步0变更。两个session Cast消费者均直接返回值，准备独立session/kernel/TCP-libpq基线。验证profile技能实际缺失，按任务列出的适用WIP/Ready执行，无Go/Bazel修改，不触发bazel_prepare。

基线 /tmp/pg-task6-red.log：三条指定精确过滤命令各实际运行1测试，各101。session接受2147483648；wire返回未规范化的 +00042 文本；共享cast kernel返回Int(2147483648)。新增测试不是0测试通过。先完成三项基线，再改生产。

参考端 bash /tmp/pg-task6-reference.sh 退出0，/tmp/pg-task6-reference.log：真实PostgreSQL18.4(Homebrew)，libpq180000，同一夹具93条真实SQL通过。ASCII空白/正负号/前导零允许，Unicode全角空白、空串、12x、小数文本和符号后空白22P02；边界22003；NULL和零行OID23；真实列读与事务E/25P02/ROLLBACK恢复。观察到原始 SELECT -2147483648::int4 在PG因后缀优先而拒绝；将负整数输入写成(-2147483648)::int4，CAST写法无此歧义。初次参考该差异失败后修正SQL，未降低值域断言。临时cluster均RAII停止并删除，未声称18.6。

局部实施决定：共享AST的CastFunctionType新增PostgreSQLInt4标记，由已有PG parser两种cast语法设置；原因是无会话参数的relational_expression_value也是实际消费者，单靠query分支连接身份不能覆盖该路径。只标记int4，int8仍保留原实现。SQL恢复分支保留标记目标INT4，避免恢复为MySQL SIGNED或空目标。builtin_cast.rs新增严格十进制ASCII转换与PgInt4Error三种有类型错误，expression/lib.rs只重导出。formal_registry现有工厂通过已具备的SQLDialect选择动态strict cast，拒绝其他Datum类且不发布MySQL cast protobuf；未修改planner/context基础设施。两个session Cast消费者复用strict kernel；SessionError保留typed source，server普通/预处理/recordset局部桥接为ConnError::PgInt4，pg_conn直接按enum映射22003/22P02/0A000，无错误文本匹配。各项均对应本任务int4语义、双消费者、MySQL隔离和typed SQLSTATE传递契约，不扩建子系统。

首次green因ast/functions.rs另有同名独立enum误扩展恢复match而编译失败，已撤销该文件无效改动；第二次因util/parser恢复match缺ast限定编译失败，局部补齐。记录编译失败但仍以真实运行通过为验收。

追加范围内红灯 /tmp/pg-task6-numeric-red.log：2个session测试实际失败。新增真实表DECIMAL/DOUBLE输入测试发现原实现以文本词法给22P02而非本批明确不支持；同时真实SELECT7 WHERE '12x'::int4=12暴露无表快速投影忽略WHERE转换。局部解决：query.rs行求值前使用已有来源字段及relational_expression_type检查PG int4输入族，零行亦检查；仅PG含CAST谓词的查询选已有完整查询路径。不新增关系执行系统。kernel测试用实际dev-dependency exprstatic及Constant::with_type修正测试构造API；编译失败保留，未视作行为通过。

中间green3：session 1/1通过，server 1/1通过、libpq实际103条SQL通过；kernel新测试编译错误，尚不能交付。green4由于输入类型检查误用astersql_types常量命名空间未编译，按现有query/source消费者改用astersql_parser_types::eval_type，并继续同断言验证。参考端增强后102条SQL通过，/tmp/pg-task6-reference-final.log；make lint 已执行至末尾，待确认退出码。

## 最终交付证据（2026-10-24）

最终 /tmp/pg-task6-slot.sh bash /tmp/pg-task6-ready.sh 整体退出0，/tmp/pg-task6-ready.log 带精确命令。10条cargo test均退出0，有效测试数依次2、1、1、1、1、8、3、2、1、1，合计21测试（本任务6个新增，其余15个原回归），无零测试验收。cargo fmt --all先执行并退出0；parser_actions/expression.rs由rustfmt --edition 2024显式格式化（include动作文件不被cargo fmt遍历），随后再测试。默认features、--locked，无--all-features，未改Rust failpoint配置或使用Go failpoint改写。

确切交付命令（均在仓库根目录、持有槽位时运行）：

    cargo fmt --all
    rustfmt --edition 2024 pkg/parser/parser_actions/expression.rs
    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_core_int4_explicit_cast -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_int4_wire_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib pg_core_int4_cast_kernel -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_int4_ast_semantics -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-util-parser --lib pg_core_int4_restore_preserves_target -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-expression --lib builtin_cast_test::go_parity -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_core_integer_cast_syntax -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_integer_cast_parse_boundary -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_dialect_prepared_lifecycle -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values -- --nocapture
    git diff --check

其他适用验证：make lint 退出0（/tmp/pg-task6-lint.log）；python3 -m py_compile pkg/server/testdata/pg_core_int4.py 退出0，自己的pyc已清理；git diff --check 退出0；自审生产、测试、说明、版权和暂存范围。WIP用于红绿迭代，Ready用于上述交付检查；.agents/skills/tidb-verify-profile实际缺失，未虚构技能执行。仅Rust/Python和模块说明变更，未触发bazel_prepare，不运行被禁止的bazel_lint_changed。Cargo.lock/Cargo manifest/外部依赖均未改。

真实行为：libpq180000通过最终107条SQL，包含三种int4目标拼写、CAST/::两语法、上下界、正负号/ASCII空白/前导零、9223372036854775808和长十进制文本越界22003、空串/12x/小数文本/Unicode空白22P02；NULL投影和表列NULL保持SQL NULL。实际BIGINT与TEXT列投影规范化且零行仍OID23；REAL/DECIMAL列（包含零行）和float/numeric常量明确0A000。不在事务内错误后下一SELECT正常，显式事务中两类错误使PQtransactionStatus=3（E），后续25P02，ROLLBACK后状态0及SELECT7通过。每个结果PQclear、最终PQfinish，表在finally删除，server.close后TCP连接失败。测试canonical存储和共享执行链不是RealTiKV。

参考端最终 bash /tmp/pg-task6-reference.sh 退出0，/tmp/pg-task6-reference-ready.log记录真实PostgreSQL18.4(Homebrew)完整version()构建串与libpq180000；同一夹具102条SQL通过（参考支持float/numeric，AsterSQL不开放这些入口的6条检查仅对Aster执行，参考多1条version查询）。临时cluster UTF8，pg_ctl停止并删除，只清理本任务对象。不声称18.6版本证据；官方18整数/表达式文档已在线复核，文档不是实测替代。

生产文件与契约依据：pkg/expression/builtin_cast.rs新增严格int4十进制内核与typed错误，builtin.rs正式工厂根据已有连接SQLDialect构建strict动态签名，lib.rs重导出复用边界；pkg/parser/ast/lib.rs新增PG int4 Cast标记，parser_actions/expression.rs两语法设置标记，ast/sql_restore.rs及util/parser/ast.rs专门恢复INT4，保障现有无会话参数消费者与重解析路径不丢语义；pkg/session/runtime/{query.rs,relational_value.rs}接入两个真实消费者并在关系行读取前验证来源类型，query.rs仅PG含CAST谓词路由既有完整查询路径；pkg/session/lib.rs保留typed source供边界读取；pkg/server/{conn.rs,runtime.rs,pg_conn.rs}仅增加PgInt4错误变体、普通/预处理/recordset必要桥接及枚举SQLSTATE映射。规划上下文和expression_rewriter已正确携带连接身份/FieldType，复用其现有工厂调用，不制造生产改动。未新增完整类型系统、事务系统或外部依赖。

独立测试文件：pkg/expression/builtin_cast_test.rs，pkg/parser/parser_test.rs，pkg/server/pg_query_test.rs，pkg/session/runtime/relational_value_test.rs，pkg/util/parser/ast_test.rs；原创libpq夹具pkg/server/testdata/pg_core_int4.py。10份实际受影响且存在的.rs.md已同步（expression/builtin与builtin_cast、parser/ast/sql_restore与parser_actions/expression、server/conn/pg_conn/runtime、session/runtime/query与relational_value、util/parser/ast）。新增/修改Rust源文件均保留已有PingCAP许可证与AsterSQL版权。

槽位：每阶段均从1至10按原子mkdir动态领取，实际均槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。持有记录包含计划、任务6、会话01a16d61及PID。每阶段正常或编译/测试失败后均由trap清理自己的owner与锁；最终全部Cargo/测试客户端/server子进程已结束，锁目录为空，编译缓存保留。最终wrapper还为INT/TERM加入子进程树终止及wait后释放路径；本轮无中断，不声称中断注入已测试。

风险：correctness由范围/语法/类型/NULL/真实列/事务恢复红绿及双消费者验证覆盖；compatibility由原CAST Go parity8项、原parser3项、原PG command/libpq2项、PG prepared1项和MySQL真实二进制prepared1项覆盖；performance新增PG来源类型检查、AST遍历与严格十进制扫描，未运行性能基准。未验证RealTiKV分布式存储、生产鉴权/TLS/SCRAM、精确18.6构建、GUI/JDBC/ORM、float/numeric转换（明确范围外）、int8新语义、完整参数定型与结果OID族矩阵（后续批次7–9）。未递归扩展范围外子系统。

使用git-commit仅提交以上本任务30个文件（13份生产Rust、5份独立Rust测试、1份Python、10份模块说明、本编号证据文件），任务1–5既有删除不暂存，plan.md始终只读。提交中保留此证据，报告完成后删除本编号文件，删除留在工作区。建议下一步按线性计划执行批次7。

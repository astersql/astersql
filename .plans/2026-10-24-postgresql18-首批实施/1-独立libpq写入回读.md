# 任务 1: 独立libpq写入回读

批次：【批次 1】依赖：无

状态：已完成

目的：提供不启动 JDBC 的独立 libpq 参数化 INSERT 与真实回读测试入口。

来源任务：蓝图1、8；路线图第8章libpq交接；用户2026-10-24首批实施要求。

预计会话范围：围绕一个可验收行为，仅修改下列生产链和独立测试；前序依赖满足后可独立阅读本文件执行，不重新展开八章。若调用链事实要求超出本行为，记录阻塞与证据，不扩建完整子系统。

## 文件


- 修改：`pkg/server/pg_client_integration_test.rs`。
- 新建：`pkg/server/testdata/pg_core_libpq.py`。

## 上下文


现有 run_clients 同时运行 libpq/JDBC；保留原入口，复用真实 Server、ConcreteSessionDriver 和 ctypes，不重写 PG 协议。新 Python 文件只服务本批流程。

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


行为：提供不启动 JDBC 的独立 libpq 参数化 INSERT 与真实回读测试入口。

新增或修改的失败验证测试：pg_core_libpq_insert_readback；归属上述独立测试文件。类型/上下文测试可增加辅助断言，但不能替代真实SQL执行。

失败验证命令（先注册新测试，再执行）：

    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_insert_readback -- --nocapture

预期失败与例外（基于源码推断，实施时以实际运行核实）：本任务属于测试流程提取，既有 INSERT/回读可能已经通过，TDD 不适用部分是保持既有行为的提取；不得故意破坏旧代码制造红灯。新增断言若暴露真实行为失败，必须留失败记录并最小修复。缺失入口或零测试不是行为失败证据。

通过验证使用完全相同的命令与断言，不删除边界、不降级期待。模拟策略：使用真实canonical会话/共享内核，协议行为通过真实TCP；客户端用真实libpq。只允许测试构建下观察真实路径，不mock SQL结果、类型和错误，不用sleep猜测执行阶段。

## 步骤与里程碑


里程碑一：在实际消费者处注册上述测试，记录基线、失败阶段和有效测试数量；接口编译失败之后仍需取得适用的运行期失败证据。若已存在正确实现，保留当前通过记录，新增缺失回归，不做无意义生产改动。

里程碑二：完成下列本行为实现，并让同一组断言通过。

提取最小真实 Server 夹具和 libpq 调用入口，使新测试仅依赖 Python3 与 libpq18，不调用 run_jdbc，不移除旧混合回归。新增测试名为 pg_core_libpq_insert_readback。
使用 root 开发鉴权、独立临时库/表和临时端口；数据库对象由真实 SQL 创建，参数化 INSERT 显式提供 int4 OID23，随后用新 SELECT 按主键回读，比较完整值及整数列 OID，验证写入确有副作用。至少覆盖两行不同参数、NULL 与空字符串的区别，避免固定返回值假通过。
Python 使用现有 PG_LIBPQ_LIBRARY 选择库，打印实际 libpq 版本、协议版本、有效场景数量，PQclear/PQfinish 放入 finally；Rust 即使子进程失败也关闭 listener、会话并删除自建对象。错误不可跳过、缺依赖不可标通过。
从环境移除 JDBC jar/JRE 的影响后运行新入口，证明单独可用；不修改用户系统安装，不停止其他服务。

里程碑三：只在测试通过后做必要局部整理，更新已改生产模块的现有 .rs.md 说明（存在且行为受影响时），再执行交付检查和差异自审。文档更新随代码走，不单独计作功能完成。

## 验证


具体crate与features：astersql-server，默认features；不得关闭现有运行路径来缩减测试。

阶段内先运行：

    cargo fmt --all

行为通过命令：

    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_insert_readback -- --nocapture

适用额外检查（新命令指向已存在Cargo包/入口）：

    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_introspection_clients_mysql_only -- --nocapture

交付检查：

    make lint
    git diff --check

预期：独立测试至少1个实际执行成功，SQL回读断言实际运行；没有启动 JDBC；原混合入口仍保留。

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


已交付独立libpq参数化写入/真实回读入口，生产行为未改动，混合入口保留。最终验证证据见下文；本文件是遵守PLANS.md的活动ExecPlan，plan.md始终只读。

## 完成


仅在适用行为验证通过且证据完整后改为已完成。若环境等无关原因阻止必要验证，允许状态已完成，待回归并明确缺失项，保留本文件；本行为自身失败不能借此宣布完成。无法突破的阻塞只在本文件标已阻塞，记录原因、检查与可独立继续的范围，不递归扩建依赖。

有代码、测试、配置或实际说明变更时，使用 $git-commit 仅提交本任务变更，排除工作区已有删除及其他任务文件；只改编号任务状态或删除则不调用提交。最终按Agent Output Contract报告文件、验证范围、风险、确切命令、未验证项和槽位。完成证据报告后删除本编号任务文件；待回归/阻塞时保留。删除前提交中保留本任务当前证据，最终回复报告可追溯提交SHA。禁止修改或删除plan.md，不把prompt.md作为进度记录。

2026-10-24 实施记录：批次1无依赖，HEAD `9736ec73b8084132c187f541c4ffb32c1d7c4aa7`，初始工作树干净。已读执行技能、根及server约束、PLANS和testing-flow；RustCodeGraph sync=0变更，已定位真实夹具。验证profile技能确实缺失。本任务提取测试夹具，无生产行为修改，按计划允许既有行为基线通过而不制造红灯。参考构建已发现Homebrew PostgreSQL 18.4，不能报告为18.6。

2026-10-24 决策：仅从run_clients提取start_client_server和CloseServer供独立测试复用，原混合入口和MySQL流程保留；Python夹具移除PG_JAVA/PG_JDBC_LIBRARIES/JAVA_HOME/CLASSPATH环境，使用真实libpq，不调用JDBC。生产文件未修改，无需生产.rs.md更新；无Go/Bazel/依赖改动，不触发bazel_prepare。

参考端验证：`/tmp/pg-task1-reference.sh` 初始化独立临时cluster（initdb -U root --auth=trust --no-locale，随机TCP端口），执行 `python3 pkg/server/testdata/pg_core_libpq.py "$port" postgres`，最后pg_ctl stop并删除临时目录。首次参考运行发现VARCHAR实际OID1043，夹具改用两端通用的TEXT保持明确OID25断言（本任务目标为int4 OID23），未改变生产代码或削减整数断言。最终参考运行退出0，libpq=180000、protocol=30000、参考server build=18.4 (Homebrew)，3组写入/回读、完整值、NULL/空串、表清理与后续SELECT均通过。该证据不是18.6构建验收。

Ready交付检查：`make lint` 退出0；`git diff --check` 退出0。Rust fmt已退出0，聚焦行为测试编译进行中，槽位1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`，锁记录持有PID90466；阶段shell设置EXIT/INT/TERM清理自己的锁，缓存保留。

首次Rust编译退出101，E0308：提取函数返回类型应为Arc<Server>（Server::new_test实际返回），已局部修复，仅测试夹具签名；本次有效测试数量0，不当作行为失败或验收证据。槽位1的阶段进程已退出并自动释放锁，保留缓存；随后重新动态领取。

运行期基线：`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_insert_readback -- --nocapture` 退出101，有效测试1个（0通过/1失败），3组目标写入、主键回读、NULL/空串和表列OID23均已通过；失败是我额外加入的`SELECT 1`期望OID23，而现实现返回OID20。整数常量定型不属于本任务，未扩修生产类型系统；用真实表列的不存在主键零行查询替代额外的常量查询，继续严格验证[23,23,25]元数据并证明连接可用，不改变写入/回读任何断言。此范围外差异留给后续整数类型任务评估，不能把它报告成已修复。原计划允许测试入口提取沿用正确行为，无需制造生产改动。

2026-10-24 最终验收：最终工作树运行 `/tmp/pg-task1-slot.sh bash -c 'cargo fmt --all && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_insert_readback -- --nocapture && env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_introspection_clients_mysql_only -- --nocapture'` 整体退出0。每阶段动态领取实际均为槽位1，绝对CARGO_TARGET_DIR为`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。所有阶段子进程结束，锁目录已为空，编译缓存保留。

命令与证据：`cargo fmt --all`退出0；`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_core_libpq_insert_readback -- --nocapture`退出0，实际1通过/0失败，0.69s；`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_introspection_clients_mysql_only -- --nocapture`退出0，实际1通过/0失败，0.49s。libpq=180000，protocol=30000；AsterSQL握手版本仅记录18.0 (AsterSQL)，不当作真实PG构建证据。相同最终Python夹具在独立PostgreSQL18.4 (Homebrew)临时参考端再次运行 `/tmp/pg-task1-reference.sh` 退出0（实际调用 `python3 pkg/server/testdata/pg_core_libpq.py "$port" postgres`），3组INSERT/按主键回读通过，零行列描述[23,23,25]通过。数据库/表均由真实SQL创建，写入值为(17,-41,'first write')、(29,NULL,'')、(43,2147483647,NULL)。PQcmdTuples每次=1，逐行及全表值均相符。Python finally清表/PQclear/PQfinish；Rust即使前次Python断言失败也在报告失败前执行关闭listener、删除独立数据库和listener不可连接断言，正常运行同样完成清理。独立入口不调用run_jdbc，并移除四项Java/JDBC环境变量。

验证范围：WIP为聚焦迭代；Ready为上述两项聚焦运行、真实参考端、`make lint`退出0、最终`git diff --check`退出0及差异自审。因profile技能缺失，只按根AGENTS.md逐项落实，未虚构技能执行；未运行bazel_lint_changed，无需bazel_prepare。改动仅`pkg/server/pg_client_integration_test.rs`（提取真实夹具、注册独立测试）与`pkg/server/testdata/pg_core_libpq.py`（真实ctypes客户端），另本编号任务记录证据；没有生产模块、Go文件、依赖、总plan或路线图改动。

风险与未验证：生产正确性/兼容性/性能无新增行为改动；测试依赖Python3与真实libpq18，缺依赖必须失败。没有运行旧libpq/JDBC混合全套、PostgreSQL18.6参考构建、生产鉴权/TLS、性能压测或RealTiKV，不能宣称这些已通过。范围外发现为整数常量SELECT 1的OID20，不在本任务修复；建议后续整数类型任务评估。下一步按原计划进入批次2。本任务已完成，提交保留证据后按用户要求删除本文件，不修改plan.md。

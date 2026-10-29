# 任务 22: 真实TiKV事务持久证据


批次：【批次22】；依赖批次：批次21。线性位置：22/24；本批仅本任务，无并行任务。

硬依赖任务：1、2、6、8。前一批是串行资源顺序；硬依赖已满足而前批其他能力阻塞时，确认其已停止且释放文件/验证资源即可继续本任务；硬依赖未满足必须记录已阻塞。
状态：未开始

目的：在真实共享存储验证DML提交持久及回滚/关闭撤销。

来源任务：`.plans/2026-10-24-postgresql18-后续整体实施/12-事务恢复与事务型DDL门槛.md`；SESSION-TXN/pg；真实存储限定验收。

预计会话范围：只处理“真实TiKV事务持久证据”这一行为族及必要局部接线、独立测试和证据；不重做首批、不扩建其他子系统。若明确依赖缺失，记录具体符号/路径和证据后停止依赖工作，不递归扩范围。

## 文件


- 修改任务1创建的测试夹具：`pkg/server/testdata/pg12_transaction_libpq.py`。
- 待新增：`scripts/verify-pg12-tikv-transactions.sh`。

## 上下文


所有命令从仓库根目录运行。先读根AGENTS.md、pkg/server/AGENTS.md及PLANS.md。server/session/executor无包级doc.go；涉及DDL先读docs/agents/ddl/README.md及pkg/ddl/doc.go。先用skills/rustcodegraph/SKILL.md核对当前索引与实际调用链；缺图覆盖再rg。当前workspace resolver=3，server crate为astersql-server，autotests=false、lib.rs显式挂载pg_transaction_test；默认features，不添加feature。session crate为astersql-session、lib.rs，默认features；不用nextgen替代默认验证。

实际入口：docs/rust/rust-postgresql-tikv-runbook.md；astersql-cmd-tidb-server。任务1、2、6、8为行为硬依赖；前批19–21的原型结论不是DML验证依赖，原型已阻塞且资源释放时仍可执行本任务。

查阅`docs/postgresql-18-core-compatibility-roadmap.md`第五章对应同名契约的模块、目标、范围外、验收和后续细分；不是只读标题。源码基线为de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3，本计划未运行任何行为测试。

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


行为：在真实共享存储验证DML提交持久及回滚/关闭撤销。

本任务不创建Rust测试函数；待新增上述脚本入口，复用任务1新增Python脚本场景commit、rollback、disconnect、failed_commit。脚本必须检查每个场景有效数量非零。

具体输入与断言：在自有隔离TiKV playground和Rust双listener上重跑commit、rollback、disconnect、failed_commit四个已定义DML场景；确认提交后关闭并重新启动自有Rust server仍见已提交值，失败/回滚前缀无残留。重启server不等于TiKV崩溃恢复，不宣称WAL兼容。

两端必须记录完整SHA、version()/server_version_num、客户端版本、存储、隔离配置、输入和初始状态。PG18_REFERENCE_DSN仅从环境读取，不打印密码/连接串；缺参考明确失败而非skip。18.4与目标18.6分别列证据，缺18.6不得降级宣称目标已验收。参考实例不得由测试擅自停止。复用既有libpq加载方式PG_LIBPQ_LIBRARY及首批独立采集模式；新增Python脚本及场景均是本计划待新增产物。脚本以场景名和仅含本地端口/测试库名的参数接收输入，敏感DSN仅环境传递；Rust测试将当前任务场景名传给脚本，两端分别保存观测，再比较，不能共享一份预期响应冒充两端结果。

记录完整DataRow值、SQL NULL、零行、列名/OID/typlen/typmod/格式、适用可空性，控制命令无结果列须注明；CommandComplete、影响行数、I/T/E（空闲/正常事务/失败事务）以及首错S/V/C/M与适用D/H/P和阶段分开保存。PBX指Parse/Bind/Execute，Sync后才读取ReadyForQuery；Flush只刷新响应。未知诊断/字段标未验证，不填固定值。

默认独立测试库，A为写连接，B每次自动提交获得新快照；初始表tx_contract(id integer PRIMARY KEY,value integer)，S0=[(1,10),(2,20)]，S1=S0加(3,30)，U=[(1,11),(2,20)]，D=[(1,11)]；按任务目标重建夹具。使用SELECT id,value FROM public.tx_contract ORDER BY id读取完整值，不数行替代。恢复探针默认SELECT 1::int4；涉及裸字面量原始输入保持原样，类型差异单列。

失败验证命令：`bash scripts/verify-pg12-tikv-transactions.sh`。新用例先完成注册及可运行性，再验证行为失败；编译失败不是红灯。预期失败原因：测试存储通过但真实TiKV丢失提交或回滚/关闭不撤销。 如果现有实现已正确，只扩充覆盖取得当前绿灯，不制造缺陷。

通过验证命令：`bash scripts/verify-pg12-tikv-transactions.sh`（同输入、同断言）。crate为astersql-server，默认features，不添加--features。

模拟策略：使用真实TCP/ConcreteSession/共享测试存储和独立PG参考；不mock查询结果或提交/回滚。只有活动执行/持久故障边界可使用确定性测试屏障。CreateAnalyzeSession测试存储不等于RealTiKV。

## 脚本构建与资源契约


新增脚本先要求调用者已有槽位锁且CARGO_TARGET_DIR为仓库槽位绝对路径，缺少即失败。内部构建命令固定为`cargo build --locked -p astersql-cmd-tidb-server --bin astersql-cmd-tidb-server`，执行二进制取自`$CARGO_TARGET_DIR/debug/astersql-cmd-tidb-server`。沿rust-postgresql-tikv-runbook的--store=tikv、--path、--host、-P、--postgres-port、--status-host、--status及--socket=参数启动；端口先探测，已占用则重选，不能杀现有进程。

playground使用唯一自有tag及独立PD端口在后台启动，保存PID并有界等健康；Rust server也后台启动并记录PID。Python分别执行commit、rollback、disconnect、failed_commit；参数由脚本注入当前端口/唯一数据库。PG18_REFERENCE_DSN仅环境传递。停服重启后只验证已记录的提交行，不重跑setup覆盖原值。最终按子进程→server→playground顺序退出、wait并删除自有tag数据，任何退出路径执行同一清理；清理完成后才允许释放槽位。Rust二进制构建及所有使用它的进程均属于本验证阶段。

## 步骤与里程碑


1. 核对当前源码及前批证据，确认目标路径和依赖；记录下列活动ExecPlan栏目。
2. 在独立测试中完成本任务夹具和真实输入，运行失败验证，保存阶段、状态及副作用。
3. 新增 scripts/verify-pg12-tikv-transactions.sh，只负责本场景自有环境生命周期并调用已有Python场景；脚本先检查工具/端口，后台启动playground，trap正常/失败/中断清理自有子进程和唯一tag数据。不得复用后删除用户服务。
4. 在失败证明的最小范围修复（原型任务仅调查）；同命令重新取得行为证据和清理证据。
5. Rust改动先cargo fmt --all，再运行聚焦验证和Ready适用检查；测试保持通过才重构。

首个里程碑是能稳定重现的真实用例；第二个是目标行为及跨连接副作用一致；最后是资源清理、Ready检查和可交接证据。每个里程碑均需记录命令、退出码及观测，不能只记录文件完成。

## 验证与完成


运行：`cargo fmt --all`、`bash scripts/verify-pg12-tikv-transactions.sh`；代码/测试代码变化运行`make lint`，最后`git diff --check`。预期：过滤器有效测试非零、目标断言通过、清理成功。

WIP阶段只跑本任务过滤器；Ready按变更类型交付：Rust改动先cargo fmt --all，再聚焦行为测试，代码变更必须make lint。当前.agents/skills/tidb-verify-profile不存在，按根AGENTS.md与docs/agents/testing-flow.md执行，不声称调用了缺失技能。仅Rust/测试夹具变化通常不触发bazel_prepare；如新增/移除Go文件、Go imports/顶层Test、Go模块、Bazel元数据变化或新workspace/工具链错误，按根完整清单先make bazel_prepare。禁止默认make bazel_lint_changed。Go测试使用failpoints时按testing-flow启用并最终禁用；既有无关Go失败记录即可，不吞并修复。

保留PingCAP版权；真正修复可用的Rust文件顶部增加或保留 // Copyright 2026 AsterSQL.。Rust测试函数必须在独立测试文件中；如必须改共享runtime，补同目录已有独立测试并在本文件补实际过滤器。外部依赖只能独立上游提交打tag并统一Git tag引用，禁止vendor、third_party或本地patch；本任务不负责移植外部依赖，缺口列依赖。

完成需当前同输入红/绿证据（已有正确实现则当前绿灯及不改生产理由）、有效测试数量非零、退出码、命令、实际槽位、绝对CARGO_TARGET_DIR、修改文件/符号、正确性/兼容性/性能风险和未验证项。只编译、零测试或历史结果不足。提交前自审diff，仅实际代码/测试/配置/文档变更用$git-commit提交本任务相关文件；只有任务状态/删除不提交。通过后最终回复交接证据并删除本任务文件；确为无关环境问题阻止验证时可标已完成，待回归并保留文件；功能或必需依赖缺失标已阻塞，不冒充完成。plan.md只读。

## 幂等与恢复


每轮唯一测试库/对象与新连接。正常、失败、INT/TERM均先停止测试子进程，存活连接必要时Sync，再ROLLBACK，事务外清理自有对象、关闭连接/listener并验证端口释放，最后释放自己的Cargo锁。故障先采残留再清理，清理不算原子性证据。只结束本任务进程/删除自有对象，不删除他人锁/缓存；失败从新夹具重试，不沿用半完成事务。

## Progress


- [ ] 核对依赖、源码和夹具。
- [ ] 取得当前失败或已有正确行为证据。
- [ ] 完成本任务局部变更/原型取证。
- [ ] 完成通过验证、清理、自审及交接。

## Surprises & Discoveries


规划时仅静态阅读；没有本任务运行结论。执行时把新发现及短证据记录在此。

## Decision Log


2026-10-29：采用单一行为闭环和串行文件占用；状态、阻塞和设计变化仅维护本文件，整体plan.md不改。

## Outcomes & Retrospective


尚未执行。完成时记录与目的的差距、当前证据路径、相关完整提交SHA和范围外依赖；删除本文件前必须在最终回复交接这些内容。

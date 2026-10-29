# 任务 35: COPY输出读取编码中断

批次：【批次 35】；依赖批次：30、33。保守线性执行1→36；每批仅本任务，禁止并行写共享服务/测试文件或争用真实参考端。外部局部依赖不意味着必须完成其整个大计划。

状态：未开始

目的：COPY输出读取编码中断，以当前真实行为和资源证据验收。

来源任务：`.plans/2026-10-24-postgresql18-后续整体实施/18-活动取消关闭与资源预算.md`；对应路线图第7章WIRE-LIFECYCLE与第8章CLIENT-LARGE/活动取消，具体归属见下述行为。

预计会话范围：一个生命周期阶段或一个计费所有者，局部生产接线加同目录独立回归；不重建共享内核。依赖满足后在单个上下文内完成；发现越界需要另立任务，不扩大本任务。

## 文件

- 候选修改：`pkg/server/pg_conn.rs`（只有失败证明局部缺口才修改）。
- 候选修改：`pkg/server/pg_extended.rs`（只有失败证明局部缺口才修改）。
- 候选修改：`pkg/server/pg_result.rs`（只有失败证明局部缺口才修改）。
- 修改测试：`pkg/server/pg_client_integration_test.rs`，新增函数`pg_lifecycle_35`；这是待新增名称，当前不能当成已有测试证据。
- 注册：复用`pkg/server/lib.rs`中既有`pg_client_integration_test`独立测试模块；生产文件不放测试函数。新增辅助测试模块时在lib.rs用cfg(test)/path显式注册。
- 新建证据：`.plans/2026-10-29-postgresql18-活动取消关闭与资源预算/证据/35-验收.md`，记录实际观测，保留供下游读取，不预填通过。

## 上下文

依赖交付读取：`.plans/2026-10-29-postgresql18-活动取消关闭与资源预算/证据/30-验收.md`、`.plans/2026-10-29-postgresql18-活动取消关闭与资源预算/证据/33-验收.md`（由对应任务创建；文件不存在或缺有效行为证据则该依赖未满足，不能以编号删除推断完成）。

crate为astersql-server，Cargo.toml设置autotests=false、lib.rs入口，无本crate自定义features；验证使用默认features。当前PgService::with_query/Completion只覆盖执行，结果编码/发送需独立证据；resource_counts只提供active/socket/worker，不能证明全部资源已回收。

## 测试计划

- 行为：COPY-OUT READ/ENCODE：依17已实现OUT，H后实际读取/编码屏障分别取消/断连，核行内容/NULL/格式正常对照及无成功COPY n、快照/编码缓存释放；已提交数据保留，依Q/扩展恢复。不要重写COPY格式或增加类型。
- 失败验证测试：上述独立文件中的`pg_lifecycle_35`，先完成可编译的真实场景与断言，再运行。
- 失败验证命令：`cargo test -p astersql-server --lib pg_client_integration_test::pg_lifecycle_35 -- --exact --nocapture --test-threads=1`。
- 预期失败原因：若当前路径缺本任务契约，将在上述行为的结果/状态/计费或期限断言处运行期失败；不得将未定义函数、编译失败或零测试当红灯。若现有实现正确，记录当前通过并仅补覆盖，不制造生产修改。
- 通过验证命令：同一条`cargo test -p astersql-server --lib pg_client_integration_test::pg_lifecycle_35 -- --exact --nocapture --test-threads=1`；修改Rust后先`cargo fmt --all`。测试必须显示running 1 test、1 passed、0 failed；内部子场景数独立报告，全部适用断言通过才可完成。
- 模拟策略：真实canonical会话、真实TCP/必要的独立libpq；仅随机源、ID分配、确定活动屏障、内存计费观测与传输故障边界可受控，不能mock SQL结果、事务或取消传播。

## 步骤

1. 复核当前HEAD及本任务依赖的保留源码/完整SHA和当前证据，不以任务文件删除判完成。明确适用场景与资源口径。
2. 新增上述独立测试与最小屏障/观测，正常对照先证明SQL真实可执行；执行失败验证，保留运行期断言与有效数量。
3. 仅修复本任务失败所证明的生产路径；每项生产修改在本文件记录对应来源行为与不可缺少的局部接线依据，不吞并其他子系统。
4. 同输入同断言复测，核正常/边界/非法/NULL（适用时）/恢复/清理。重复资源轮按冻结次数执行；成功/失败/中断均不残留本轮资源。
5. Ready检查、审查git diff及证据，按完成规则交接。实际命令及失败限制记录在本任务，plan.md只读。

## 验证

运行：`cargo fmt --all`、`cargo test -p astersql-server --lib pg_client_integration_test::pg_lifecycle_35 -- --exact --nocapture --test-threads=1`、`make lint`（代码变更必需）、`git diff --check`。测量任务也使用同一实际测试入口；不默认运行全包或广泛pg_过滤器。按本任务受影响相邻测试可追加精确过滤器，实际执行前记录原因，不能未经需要全扫。

预期：真实行为契约全部满足、原失败断言通过或已正确行为获得当前证据、清理期限内资源按增量回基线；无secret泄漏、无其他入口串扰。编译/零测试/仅PING/仅取消发送成功均不够。当前本轮只创建计划，未执行上述验证。

## 执行契约

从仓库根目录执行。先读根AGENTS.md、pkg/server/AGENTS.md、PLANS.md与docs/agents/testing-flow.md；pkg/server/doc.go当前不存在，实施时如出现须读。导航使用skills/rustcodegraph/SKILL.md。WIP只跑本任务过滤器，Ready按变更类型执行；代码变更必须make lint，Rust改动先cargo fmt --all，最后git diff --check。当前tidb-verify-profile技能不存在，按根规则等效选择，不虚称技能通过；禁止默认make bazel_lint_changed。Bazel准备仅按根完整触发条件；若动Go测试须检查failpoint并最终禁用。保留PingCAP版权，可用Rust修复加/保留顶部AsterSQL版权。外部依赖只允许独立上游提交打tag及统一Git tag引用，禁止本地patch/vendor。

边界依大计划18及路线图第7章“取消资源与关闭边界/取消与资源关闭验收”、第8章“活动取消/大结果风险”；读取适用正文，不能只读标题。冻结预算必须有实测数值、单位、计费范围、策略依据/版本、拒绝阶段与回收期限；不得临时调大门槛抹掉失败。当前未冻结数值是实施前置产物，不是本轮已有配置。依赖缺失只记录本任务阻塞，不递归实现上游子系统。

真实libpq沿pkg/server/pg_client_integration_test.rs与testdata/pg_core_libpq.py现有Python ctypes模式；PG_LIBPQ_LIBRARY选择实际libpq18，缺客户端明确失败不可跳过。新窄客户端场景可在对应任务新增独立pkg/server/testdata/pg_lifecycle_35.py，由该Rust测试Command调用；不声称已有脚本/过滤器。只在需要时新增，不扩通用平台。产品/参考端完整构建SHA、实际版本、协议3.0/3.2及身份模式分别记录；18.4与18.6分别报告，参考端缺失记录未验证，不降级目标。参考端活动须有其自身可观测等待控制，不能冒称有产品屏障；生产安全场景依15，不把开发root探针当生产认证通过。

每轮正常、失败、中断均清理：先解除本轮屏障/唤醒阻塞，等执行与子进程退出；存活扩展连接按阶段Sync，T/E先ROLLBACK，Close对象及libpq结果/取消对象，再关连接；死连接由B观察回滚。只有观察到资源收敛后事务外DROP本轮唯一表/库，保留首错及清理错误。仅管理自建服务与数据；实存储按docs/rust/rust-postgresql-tikv-runbook.md及testing-flow后台启动/验证/清理三路径，不能用内存重启证明持久性。DDL仅作夹具时也先读docs/agents/ddl/README.md和相关doc.go，不改变DDL语义。

最终证据包含有效场景/断言数量（不能为0）、命令/退出码、SQLSTATE/错误阶段/I-T-E、真实值/NULL/适用类型元数据、另一连接真实查询、资源基线/峰值/结束值、预算与清理期限；secret只存长度与脱敏关联。资源分别计扫描/快照/锁、源rows/编码/待发/对象缓存、context/socket/worker/ID；RSS不要求立即还给OS，不替代逻辑计费。观察器仅最小且受控，不能mock执行结果或取消命中。

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

## Progress

- [ ] 复核依赖与活动/资源口径。
- [ ] 完成失败验证或测量对照。
- [ ] 完成局部修复/测量和通过验证。
- [ ] Ready检查、证据及交接。

## Surprises & Discoveries

规划时仅静态核对；实施发现与阻塞在这里记录，未知数据不得填为通过。

## Decision Log

2026-10-29：本任务按一个行为所有者拆分；共享pg_conn/pg_extended及客户端验证资源，采用单任务批次。预算缺实测则先取得证据，跨域缺接口则记录局部依赖。

## Outcomes & Retrospective

尚未实施。结束时写实际行为、失败/通过或测量证据、资源前后值及未验证范围，并复制必要证据到本任务独立验收文件。

## 完成

取得上述当前证据才标记`已完成`。有实际代码、测试、配置或说明变更时使用$git-commit，只提交本任务相关变更；只有编号文件状态更新/删除时不提交。完成后最终回复给Agent Output Contract五项、完整SHA、实际槽位与CARGO_TARGET_DIR，删除本编号任务文件，保留证据供下游消费。被阻塞只在本任务记录具体原因和检查；若仅无关环境导致验证不能运行，可标`已完成，待回归`并保留本文件，不声称验收通过。缺行为/依赖或未冻结预算不能按环境例外完成。

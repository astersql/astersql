# 任务 4: 优化 `astersql-planner-core-casetest-instanceplancache::concurrency_test::TestInstancePlanCacheConcurrencyComp`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 4、原始耗时 10.087 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 4 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`4` / `10.087s` / `TIMEOUT`；原日志为 TIMEOUT，同时超过性能门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`concurrency_test::TestInstancePlanCacheConcurrencyComp`；binary id：`astersql-planner-core-casetest-instanceplancache`。
- 初步分型：`combination-scale`。保留全部组合、并发 worker、数据规模与随机/种子契约；优化共享准备、批处理、数据结构和重复解析，不通过抽样或减少循环次数达标。 该系列与 Go 并发契约对齐；不得缩减 2,000 条语句、10 个 worker 或 SQL 类型分布，应优化 Rust harness/执行热路。
- 原始数字来自全 workspace 并发运行，只用于入选与排序；修改依据必须来自本任务五次预热隔离测量和剖析。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

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

## 性能测量独占规则

1. Cargo 编译预热可使用已领取的独立槽位；真正执行 `before`/`after` 五次计时前，还须用原子 `mkdir target/rust-performance/perf-audit.lock` 领取仓库级性能锁。
2. 锁目录中记录任务 4、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `concurrency_test::TestInstancePlanCacheConcurrencyComp` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/concurrency_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0004-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyComp)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0004-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyComp)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；以同一真实测试的五次 before/after 对比作为性能证据。默认在 after 的中位数和最大值均比 before 改善至少 20% 时认定为明显提升；若未达到，必须用剖析、已尝试方案和剩余下界证明继续优化不合理。原有功能断言始终不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/instanceplancache/Cargo.toml`、`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。失败、超时或零测试必须修复；若中位数和最大值均改善至少 20%，可按明显提升完成。未达到 20% 时继续剖析，或记录主要热点已消除、不可消除下界及继续优化会损害语义或投入产出失衡的证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；正确性通过，且取得明显提升或有充分停止优化证据后标记 `已完成`，使用 `-commit` 只提交本任务变更并删除本任务文件。只有缺少完成所需外部条件且无法取得上述证据时才标记 `已阻塞`。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0004-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyComp)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0004-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyComp)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，精确符号 `concurrency_test::TestInstancePlanCacheConcurrencyComp`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据；阶段耗时插桩因其他任务持有性能锁且同时有锁外高负载测试而撤回，未产生可信的阶段数据。
- [ ] 完成保持语义的最小优化。
- [ ] 保存 after、测试数量与耗时证据；有代码修改时同时保存 fmt、lint 证据。
- [ ] 自审、更新状态、提交并删除任务文件。

## 意外发现

- 初始事实：全量并发日志记录 `10.087s` / `TIMEOUT`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 2026-10-12 隔离基线在槽位 1（`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`）中预热后，五次均精确选中 1 个测试，均在 nextest 60 秒上限超时：`60.146075s`、`60.128792s`、`60.120425s`、`60.121947s`、`60.124185s`；报告为 `target/rust-performance/task-0004-before.tsv`。
- 调用链证据：`TestInstancePlanCacheConcurrencyComp -> mixed_read_workload -> test_with_workers -> run_statements`。工作负载保留 2,000 个生成槽位和 10 个 worker；DQL 广播到全部 worker，每项至少执行 normal query、prepare、set 和 execute，Rust 还为并发可见性保留 confirmed query/最多 32 次稳定窗口尝试。
- 基线后准备运行临时阶段插桩时，仓库性能锁已先后被任务 7 和任务 5 合法持有；同时还观察到其他会话在锁外直接运行同包 `ConcurrencySysbench` 和多个 `target/debug` Cargo/rustc 进程。为不污染他人计时或让他人编译临时插桩，插桩已完全撤回。
- 2026-10-13 重试时，共享工作区已由其他 instance-plan-cache 任务增加 `QueryStatementBatch` / `run_statements_batch`，它们在 canonical session owner 内保留真实 SQL 顺序和结果集，为可验证的必要局部扩围候选。
- 在槽位 5（`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-5`）完成当前组合预编译后，临时仅将 Comp 的调用点切换为 `run_statements_batch`。持性能锁运行时，脚本新增的未计时执行预热精确选中 1 项，但仍在 `60.197s` 超时，因此脚本在正式五次 sample 前以退出码 1 终止，`target/rust-performance/task-0004-after.tsv` 只有表头。
- 上述候选未产生加速，且它不保留 `run_statements` 的最多 32 次稳定窗口重试；Comp 调用点已撤回到 `test_with_workers`。其他任务的 helper/API 和相邻用例改动未被撤回或纳入本任务。
- 2026-10-13 在槽位 5 与性能锁下重跑原始路径，精确选中 1 项并在 `60.171s` 超时；运行日志为 `target/rust-performance/task-0004-sample-run.log`。macOS `sample` 对测试进程做了 5 秒、1ms 间隔的调用栈采样，报告为 `target/rust-performance/task-0004-sample.txt`。采样中 10 个 `run_statements` worker 都存活，分布在第 85–105 行的 normal query、prepare/set/execute 和 confirmed query，主要等待各自真实 session-owner 的 `SessionReply`；session 侧同时持续出现 planner/statistics 执行栈。这排除了 worker 互等或单纯 channel 往返死锁，确认主要下界来自必须保留的真实 SQL 数量。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：不移除 DQL 广播、不缩减 2,000 条语句或 10 个 worker、不缓存/跳过 SQL `prepare`，也不以直接 API/mock 替换 SQL normal/prepared 真实路径。理由：这些都是 Go 用例的核心并发契约，会违反本任务明确禁止项。日期/作者：2026-10-12 / Codex。
- 决策：状态标记为 `已阻塞`并保留任务文件，不提交伪优化。理由：五次实测与真实 SQL 调用数量表明 `<=0.499s` 与必须保留的负载相差两个数量级；当前机器还有锁外高负载，无法取得可信的进一步严格计时。日期/作者：2026-10-12 / Codex。
- 决策：穷尽当前仓库已有的真实 session-owner 批处理替代后仍保持 `已阻塞`。理由：批处理未计时预热仍 `60.197s` 超时，证明 channel 往返不是可弥合门槛的主导热点；继续达标需要任务所有者决定是否放弃 2,000 条、10 worker、逐项真实 SQL 或 0.499 秒中的至少一项硬约束。日期/作者：2026-10-13 / Codex。
- 决策：运行期采样后不实施删除查询/断言或替换真实 session 的优化。理由：Go `testWithWorkers` 与 Rust 均把 DQL/事务边界广播给 10 个 worker；采样显示 worker 正常推进且成本落在真实 session SQL 执行，继续压缩只能违反明确验收约束。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：已阻塞。按“失败验证”中的完整命令执行 `before`，退出码 `1`；五次均超时，最大值 `60.146075s`，每次有效测试数均为 1，未观察到功能 panic，但在 60 秒内无法完成正确性断言。未修改生产或测试逻辑，临时插桩已撤回；`cargo fmt --all`、`after`、`make lint` 未运行，因为没有可交付的代码修改，且性能资源持续被其他任务占用/污染。
- 解除阻塞所需的方向：先修复调度器令所有高负载测试都尊守仓库性能锁，再在空闲主机上重做阶段剖析；如果仍要求 0.499 秒门槛，需要任务所有者明确改变“保留全部 2,000 条、10 worker 和每条真实 normal/prepared SQL 路径”中的至少一项约束，否则局部 harness 微优化无法弥合当前超过 120 倍的差距。
- 2026-10-13 复查补充：已实际尝试当前仓库最强的局部批处理路径，仍在 60 秒超时，所以阻塞不再仅是机器瞬时负载或未尝试的优化假设；需要人工重定任务约束。
- 2026-10-13 采样补充：原始路径的 5 秒调用栈再次证实没有死锁，而是 10 个 worker 都在等待各自真实 SQL 请求完成；精确测试最终 `60.171s` 超时。证据文件保留在 `target/rust-performance/task-0004-sample.txt` 和 `target/rust-performance/task-0004-sample-run.log`，槽位 5 与本任务性能锁已释放。

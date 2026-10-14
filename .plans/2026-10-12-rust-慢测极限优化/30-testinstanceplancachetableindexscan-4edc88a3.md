# 任务 30: 优化 `astersql-planner-core-casetest-instanceplancache::concurrency_test::TestInstancePlanCacheTableIndexScan`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 30、原始耗时 6.203 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 30 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`30` / `6.203s` / `PASS`；原日志已通过且达到本轮入选门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`concurrency_test::TestInstancePlanCacheTableIndexScan`；binary id：`astersql-planner-core-casetest-instanceplancache`。
- 初步分型：`combination-scale`。保留全部组合、并发 worker、数据规模与随机/种子契约；优化共享准备、批处理、数据结构和重复解析，不通过抽样或减少循环次数达标。
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
2. 锁目录中记录任务 30、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `concurrency_test::TestInstancePlanCacheTableIndexScan` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/concurrency_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0030-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheTableIndexScan)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0030-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheTableIndexScan)'`
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

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0030-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheTableIndexScan)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0030-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheTableIndexScan)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，精确符号 `concurrency_test::TestInstancePlanCacheTableIndexScan`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 评估保持语义的优化边界；未保留无法把真实协议下界降至门槛内的代码改动。
- [x] 保存 5 次测试数量与耗时证据；临时插桩前运行 `cargo fmt --all`，插桩已完全撤回，Ready lint 对无代码交付不适用。
- [x] 自审并更新为已阻塞；按协议保留任务文件，不提交、不删除。

## 意外发现

- 初始事实：全量并发日志记录 `6.203s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- before 精确选中 1 项，5 次均功能通过且零超时，耗时为 `1.466218 / 1.418063 / 1.409808 / 1.579509 / 1.967473s`，最大 `1.967473s`，严格门槛失败。
- 2026-10-13 恢复后的 before 在已解锁主机上精确选中 1 项，5 次均功能通过且零超时，耗时为 `1.004583 / 1.004183 / 0.985863 / 1.185222 / 1.081418s`，最大 `1.185222s`；旧锁屏证据已被本组新数据替代。
- 持性能锁的阶段分解显示，每个 worker 的 100 次累计耗时范围为：`PREPARE 100–203ms`、`SET 120–229ms`、`EXECUTE + Sort 973–1142ms`、结果 `Check 1.4–2.2ms`；热点是完整 10 worker × 100 次真实 `EXECUTE` 请求，而非 fixture、期望值构造或断言。
- Rust 与 Go 源码均在每轮重新 `PREPARE`，随后设置两个变量并执行范围查询；保持 Go 对齐意味着不能把 3,000 个 text-protocol 请求合并、把语句移出循环、减少 worker/循环或绕过真实计划缓存执行路径。
- 临时计时插桩只用于定位并已完全撤回；目标 Rust 测试及生产代码没有留下本任务改动。
- 穷尽三种保持规模的局部方案：typed-adapter 首次错误实现用时 `0.205s` 但主键范围只返回下界且二级索引无路径，修复完整范围后功能通过但用时 `1.788s`；同一 session-owner 中逐对执行完整 PREPARE/SET/EXECUTE 功能通过但用时 `1.119s`；单个多语句批次保留全部 100 组语句与结果断言，功能通过但用时 `1.005s`。所有实验改动均已撤回。
- 最新证据表明跨线程通道、逐条 parser 调用和协议帧合并都不是剩余主因；完整执行 10×100 个真实范围查询并物化约五万行的墙钟下界仍约 1 秒。
- 2026-10-13 再次续跑时，共享 runtime/testkit 已新增等值索引 typed adapter 和 prepared batch 能力，但本测试的参数化范围谓词仍不在新门禁内；严格命令的 5 个测试执行样本为 `1.502757 / 1.484094 / 1.705031 / 1.665193 / 1.894881s`，全部功能通过且每次精确 1 项，仍远超 `0.499s`。共享工作区在第 4、5 次前触发了重编译，因此该组不作为同一源码快照的 after 证据；但 nextest 报告的单测试执行时间每次均为门槛约 3 倍以上，足以排除“新共享改动已自然达标”。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：不采用移出循环的 `PREPARE`、合并协议请求、减少范围结果、降低 10×100 并发规模或绕过 TestKit/计划缓存的直接 API。理由：这些方案会改变 Go 原测试明确覆盖的逐轮 PREPARE/SET/EXECUTE 与实例计划缓存并发语义。日期/作者：2026-10-12 / Codex。
- 决策：在真实 text-protocol 边界的功能正确耗时下界仍为 `1.409808s`、且剖析确认约 1 秒集中于不可删减的 EXECUTE 路径后标记已阻塞。理由：安全的测试侧分配/断言优化不足以跨越至少 0.910808 秒的门槛差距；进一步提速需要扩大为 session/planner typed range 执行能力，超出本 Go commit 增量的最小必要局部接线。日期/作者：2026-10-12 / Codex。
- 决策：2026-10-13 恢复后允许把范围扩大到 `pkg/session/runtime/dispatch.rs` 的既有 typed-adapter 入口。调用链证据为 `EXECUTE` → `execute_prepared` → `execute_simple_prepared_select_through_adapter`；当前门禁只接受主键等值谓词，而下游 `typed_adapter_bridge::encode_index_ranges` 与 `encode_table_record_ranges` 已支持参数化二级索引和主键范围。仅扩展门禁，保留原 text-protocol PREPARE/SET/EXECUTE、planner 和 KV 执行。日期/作者：2026-10-13 / Codex。
- 决策：同时修复 `pkg/session/runtime/planning.rs` 对 `USE INDEX(primary)` 的访问路径接线，以及 `pkg/planner/core/expression_rewriter.rs` 在分拆范围条件时按 SQL offset 恢复参数序号。理由：首次 WIP 已在 `0.205s` 内完成但暴露二级索引无路径和两个范围边界都绑定首参；两处均是 typed 范围执行正确性不可缺少的局部调用链修复，不改变测试规模或协议语义。日期/作者：2026-10-13 / Codex。
- 决策：撤回 typed-adapter、参数序号、访问路径和测试批处理实验，不将它们作为任务 30 交付。理由：正确实现仍为 `1.788s`，两种真实 SQL 批处理仍为 `1.119s` / `1.005s`；没有任何方案同时满足完整 Go 行为和 `<0.500s`。日期/作者：2026-10-13 / Codex。
- 决策：以已解锁主机的新五次数据重新标记已阻塞。理由：要跨越剩余约 `0.486s` 的最佳正确结果差距，只能减少查询/结果规模、复用查询结果或绕过逐轮真实 KV 范围执行，均直接违反本任务硬约束。日期/作者：2026-10-13 / Codex。
- 决策：共享 runtime/testkit 变更后只重跑原始精确测试，不改用新增的 `QueryPreparedStmtBatch`。理由：它需要把 Go 每轮的 `PREPARE` 移出 100 次循环，会削弱实例计划缓存的并发重复准备语义；新五次执行时间又已证明原始语义未自然达标。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 已阻塞。基线使用 Cargo 槽位 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；阶段剖析使用槽位 6，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-6`，两个阶段均已释放自己的槽位锁和性能锁并保留缓存。
- 2026-10-13 恢复基线实际使用槽位 8，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-8`；WIP 实验动态使用槽位 1、4、5、6、9，所有本任务锁均已释放并保留缓存。
- before 命令退出 1，报告 `target/rust-performance/task-0030-before.tsv`，5/5 项功能通过、有效测试数量均为 1、零失败、零超时，但最大 `1.967473s > 0.499s`。
- 剖析命令退出 0，精确运行 1 项并确认 10 个 worker 均完成完整 100 轮；临时插桩前 `cargo fmt --all` 退出 0。因最终无代码改动，未运行 `make lint`，也未生成 after 报告；before 的五次真实失败证据已满足阻塞记录要求。
- 自审：未删减 case、断言、100 行数据、10 worker、每 worker 100 轮、主键/二级索引随机覆盖或真实依赖；未修改 Cargo 依赖、license、`plan.md` 或任何生产代码。剩余建议是在独立任务中扩展 session/planner typed adapter 对参数化主键与二级索引范围谓词的等价支持，并单独补充行为回归后再复测本任务。
- 最新恢复验证命令退出 1，报告 `target/rust-performance/task-0030-resume-before.tsv`，5/5 功能通过、有效测试数量均为 1、零失败、零超时，最大 `1.185222s > 0.499s`。WIP 过程中多次执行 `cargo fmt --all` 均退出 0；最终没有保留任务 30 的 Rust 代码，因此 Ready `make lint` 不适用。
- 2026-10-13 续跑使用槽位 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；精确二进制预热退出 0，五次复测命令退出 1，报告 `target/rust-performance/task-0030-resume-recheck.tsv`，5/5 功能通过、每次精确 1 项、零失败、零超时，最大 `1.894881s > 0.499s`。测量期间共享源码变动导致两次重编译，已明确记为验证限制；本任务没有留下 Rust 代码改动，槽位与性能锁均由同一进程自动释放。

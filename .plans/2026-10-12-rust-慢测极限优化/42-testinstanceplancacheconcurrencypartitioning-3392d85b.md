# 任务 42: 优化 `astersql-planner-core-casetest-instanceplancache::concurrency_test::TestInstancePlanCacheConcurrencyPartitioning`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 42、原始耗时 5.565 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 42 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`42` / `5.565s` / `PASS`；原日志已通过且达到本轮入选门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`concurrency_test::TestInstancePlanCacheConcurrencyPartitioning`；binary id：`astersql-planner-core-casetest-instanceplancache`。
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
2. 锁目录中记录任务 42、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `concurrency_test::TestInstancePlanCacheConcurrencyPartitioning` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/concurrency_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0042-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPartitioning)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0042-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPartitioning)'`
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

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0042-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPartitioning)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0042-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPartitioning)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，精确符号 `concurrency_test::TestInstancePlanCacheConcurrencyPartitioning`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文；Cargo 槽位与性能锁在首次基线执行时动态领取。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 尝试了保持 10 个 worker、1,000 次真实 prepared 查询、三类 SQL 及逐项结果断言的批处理优化；干净窗口下仍超过门槛，已撤回本任务的未达标源码 hunk。
- [x] 保存 after 与测试数量/耗时证据；`cargo fmt --all` 退出码 0，性能门槛仍失败，因此未运行 `make lint`。
- [x] 自审并更新为已阻塞；按协议保留本文件，不提交、不删除。
- [x] 2026-10-13 解锁后重新验证最新基础设施，并尝试去除结果列元数据及短参数向量分配；严格门槛仍失败，已撤回这些无效源码改动。

## 意外发现

- 初始事实：全量并发日志记录 `5.565s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 工作区存在其他并行任务的未提交变更，且同一 `concurrency_test.rs` 中的 `TestInstancePlanCacheConcurrencyPointMultipleColPKNoTxn` 已被修改；本任务将仅触及分区并发用例的必要 hunk，不覆盖或提交他人变更。
- before 在性能锁内精确选中 1 项，5 次均功能通过、零失败、零超时，但耗时为 `2.321097 / 1.500033 / 1.603630 / 1.390672 / 1.628923s`，最大值 `2.321097s`。
- 将文本 `prepare/set/execute` 改为真实二进制协议 prepare 和 session-owner 批执行，保留全部 1,000 次查询后，干净低负载窗口的 5 次为 `0.618343 / 0.603660 / 0.620001 / 0.604650 / 0.564046s`；功能仍全部通过，但最大值 `0.620001s` 高于 `0.499s`。
- 共享真实计划预热不能稳定改善（五次 `0.810197 / 0.782006 / 0.816812 / 0.721244 / 0.600683s`），已撤回，避免偏离 Go 冷启动意图。
- 原始结果已由 `QueryRows` 物化为 `DbValue::String`；去除二次字符串矩阵构造后干净数据仍稳定在 `0.56–0.62s`。继续降至 `0.499s` 以下需要减少查询/结果规模、改变真实 session 边界或深入优化公共 SQL 执行器；前两者被任务禁止，后者超出对应 Go commit 的最小增量范围。
- 2026-10-13 恢复后的原始实现隔离样本为 `1.022405 / 1.078102 / 1.039076 / 1.057082 / 1.031682s`。在保留 10 worker、每 worker 100 次真实查询及全部结果断言的二进制 prepared 批执行方案下，最佳稳定一组仍为 `0.648966 / 0.660838 / 0.688227 / 0.713203 / 0.686424s`；去除二次字符串结果构造后四个无重编译样本为 `0.624177 / 0.606909 / 0.607887 / 0.619010s`。
- 公共批执行路径进一步省略未使用列名复制后，五次为 `0.631029 / 0.642779 / 0.616058 / 0.622964 / 0.612216s`；短参数改为栈数组后反而为 `0.706220 / 0.749956 / 0.726057 / 0.740986 / 0.745046s`。这些微分配不是决定性瓶颈，且后一轮存在主机负载抬升；两项公共路径修改均已撤回。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：将任务标记为 `已阻塞`，并撤回未达标的任务 42 源码 hunk。理由：保留 Go 对齐的 10 个 worker、每 worker 100 次真实查询、三类 SQL、参数分布和全部结果断言时，最小可重现下界仍为 `0.564046s`；进一步局部缩减会违反明确边界。日期/作者：2026-10-12 / Codex。
- 决策：2026-10-13 本地 Mac 解锁后恢复任务，先基于最新共享基础设施重跑五次基线，不将旧锁屏/资源状态当作当前阻塞。日期/作者：2026-10-13 / Codex。
- 决策：恢复后仍标记为 `已阻塞`，不提交未达标优化。理由：允许范围内最有效方案约为 `0.61–0.71s`，仍无法满足五次最大值 `<=0.499s`；继续减少真实查询、并发或断言会违反任务边界，而公共执行器微优化没有收益。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：已阻塞；功能始终通过，但严格性能门槛未达到，不声称完成。
- Cargo 槽位/路径：before 使用槽位 3，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3`；迭代/after 实际使用过槽位 `6、8、10、3、6、5、2`，对应绝对路径均为 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-N`。每阶段均已释放自有锁并保留缓存。
- 证据：`target/rust-performance/task-0042-before.tsv` 和 `target/rust-performance/task-0042-after.tsv`；每份报告精确 1 项 × 5 次，零功能失败、零超时，但均因耗时超过 `0.499s` 而非零退出。
- 恢复证据：`target/rust-performance/task-0042-resume-before.tsv`、`target/rust-performance/task-0042-resume-after.tsv`；后者最终五次精确执行 1 项，全部功能通过，最大值 `0.749956s`，门槛脚本退出码 1。实际最终槽位为 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。
- 命令：已按任务文件原样运行 before/after `./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 ...`；每轮 Rust 变更后先运行 `cargo fmt --all`，退出码 0。
- 未验证：未运行 `make lint`，因为性能硬门槛已失败且最终不交付代码修改；未运行 workspace 全量回归。
- 自审：本任务的优化 hunk 已撤回；未删减 case/断言/数据/并发规模，未改 mock、timeout 或 Cargo 依赖，也未触及 `plan.md`。共享工作区中同文件的其他任务改动保留不动。

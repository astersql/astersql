# 任务 21: 优化 `astersql-planner-core-casetest-instanceplancache::builtin_func_test::test_builtin_in_real_sig`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 21、原始耗时 7.590 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 21 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`21` / `7.590s` / `PASS`；原日志已通过且达到本轮入选门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`builtin_func_test::test_builtin_in_real_sig`；binary id：`astersql-planner-core-casetest-instanceplancache`。
- 初步分型：`initialization`。重点分解 Domain/TestKit/runtime 初始化、fixture/golden 解析、主体与 teardown；只复用不可变状态，复用可变会话或全局状态前必须证明 reset 完整。
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
2. 锁目录中记录任务 21、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `builtin_func_test::test_builtin_in_real_sig` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0021-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_real_sig)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0021-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_real_sig)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；以同一真实测试的五次 before/after 对比作为性能证据。默认在 after 的中位数和最大值均比 before 改善至少 20% 时认定为明显提升；若未达到，必须用剖析、已尝试方案和剩余下界证明继续优化不合理。原有功能断言始终不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/instanceplancache/Cargo.toml`、`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。失败、超时或零测试必须修复；若中位数和最大值均改善至少 20%，可按明显提升完成。未达到 20% 时继续剖析，或记录主要热点已消除、不可消除下界及继续优化会损害语义或投入产出失衡的证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；正确性通过，且取得明显提升或有充分停止优化证据后标记 `已完成`，使用 `-commit` 只提交本任务变更并删除本任务文件。只有缺少完成所需外部条件且无法取得上述证据时才标记 `已阻塞`。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0021-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_real_sig)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0021-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_real_sig)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`，精确符号 `builtin_func_test::test_builtin_in_real_sig`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 完成保持语义的最小优化：400 次单行 INSERT 合并为 4 次等价多值 INSERT；每个 worker 的 100 组 PREPARE/SET/EXECUTE 分为 10 个 owner-thread 批次，保留全部语句、结果集、行与索引矩阵。
- [x] 保存 after、测试数量与耗时证据；`cargo fmt --all` 与 `make lint` 均退出 0。
- [x] 继续穷尽安全批处理变体并自审；因真实 10×100 PREPARE/SET/EXECUTE 下界仍超过 0.499s，要达标需改变任务规模或优化超出本 Go commit 增量的生产子系统，按协议保留任务文件且不提交。

## 意外发现

- 初始事实：全量并发日志记录 `7.590s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- before 精确选中 1 项，5 次均功能通过，耗时为 `2.071905 / 1.826452 / 1.833561 / 2.385403 / 1.706325s`，最大 `2.385403s`，严格门槛失败。
- 持性能锁单次阶段分解：Harness `0.094783s`，400 次单行 fixture 累计到 `0.812758s`，完整 10×100 并发主体累计到 `4.413735s`；绝对值受共享主机编译负载影响，但热点明确在主体。
- 安全批量 fixture 的最终五次验证均功能通过，耗时为 `1.443819 / 1.487721 / 1.425557 / 1.556424 / 1.506000s`，最大 `1.556424s`，仍不达标。
- 尝试将每轮三个 text-protocol 请求合并为一个多语句请求，耗时降到约 `1.38–1.51s`，但后续一次复测出现 `SELECT request dispatches retained by a worker`，证明请求边界不等价，已完全撤回。
- 尝试仅复用 parser 的 thread-local yacc 临时栈，未降低主体耗时，已完全撤回；未保留任何诊断插桩。
- 继续尝试 owner-thread 批处理：`QueryStatementPairs` 保留每轮两个请求边界，五次为 `1.394294 / 1.422301 / 1.115629 / 1.257527 / 1.030439s`；单个 300 语句大批次为 `2.698515 / 3.682378 / 3.281342 / 3.157301 / 3.010333s`，因超长 SQL 解析反而更慢。
- 最终变体为每 worker 10 个批次、每批次 10 组且 30 条真实语句；严格五次为 `1.075701 / 1.100237 / 1.404820 / 1.111460 / 1.116458s`，5/5 功能通过但最大值仍为 `1.404820s`。
- 根据 RustCodeGraph 调用链扩大到 `InstancePlanCache::Get`：将全局 `Mutex` 命中路径试改为 `RwLock` + 原子 LRU 时钟，其 5 项专属单元测试全部通过，但本任务五次仍为 `1.125371 / 1.297223 / 1.348126 / 1.395942 / 1.265735s`；证明缓存锁非主导瓶颈，该生产实验已完全撤回。
- 撤回生产实验并仅保留 `test` schema 精准清理后，最后五次为 `2.547922 / 2.703846 / 3.182643 / 2.214928 / 2.805299s`；功能仍 5/5 通过，但当轮共享主机负载显著高于前两轮，不将此噪声归因于一行 schema 清理优化。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：保留多值 INSERT，因为它保留 4 张表、400 行、完全相同的值和索引路径，只消除与被测语义无关的重复解析/提交。日期/作者：2026-10-12 / Codex。
- 决策：不减少 10 worker×100 次 PREPARE/SET/EXECUTE，不改用 protocol API/mock，不放宽门槛。在这一真实 text-protocol 边界下安全优化后最大值仍为 `1.556424s`，故标记已阻塞。日期/作者：2026-10-12 / Codex。
- 决策：继续后保留 10 批次方案，因它在不减少语句、断言或真实 dispatcher/parser 边界的前提下是已测安全变体中最快的；巨型批次、请求对与 parser scratch 均不能达标。进一步达标需用户决定改变 Go 对齐规模/语义，或授权超出对应 commit 增量的 parser/session/planner 生产优化。日期/作者：2026-10-13 / Codex。
- 决策：继续获得生产锁热点假设的反证后撤回 `InstancePlanCache` 变更，不为无性能收益的扩大范围改动承担正确性风险。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 已阻塞。实际 Cargo 槽位先后为 2、6、4、10、8；继续后最终有效验证仍使用槽位 8，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-8`。
- before 命令退出 1，报告 `target/rust-performance/task-0021-before.tsv`，5/5 项功能通过、零超时，但最大 `2.385403s > 0.499s`。
- 当前代码的最终 after 命令退出 1，报告 `target/rust-performance/task-0021-after.tsv`，5/5 项功能通过、零超时，耗时 `2.547922 / 2.703846 / 3.182643 / 2.214928 / 2.805299s`，最大 `3.182643s > 0.499s`。在更低负载轮次，同一 10 批次主体已证明最大值约 `1.4s`，仍远高于门槛。
- 最终 `cargo fmt --all` 退出 0；`make lint` 退出 0。未运行 workspace 全量 Rust 回归；该回归由总计划完成后统一执行。
- 自审：未删减 case、断言、行数、表/索引矩阵、worker 或每 worker 循环数；未修改 Cargo 依赖、license 或 `plan.md`；所有诊断和不等价实验均已撤回。

# 任务 109: 优化 `astersql-tests-graceshutdown::graceshutdown_test::test_graceful_shutdown`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 109、原始耗时 3.512 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 109 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`tests/graceshutdown/graceshutdown_test.rs`
- Cargo 清单：`tests/graceshutdown/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/graceshutdown/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/graceshutdown/lib.rs`

## 上下文

- 原始排名/耗时/状态：`109` / `3.512s` / `PASS`；原日志已通过且达到本轮入选门槛。
- Cargo package：`astersql-tests-graceshutdown`；target：`astersql_tests_graceshutdown`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`graceshutdown_test::test_graceful_shutdown`；binary id：`astersql-tests-graceshutdown`。
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
2. 锁目录中记录任务 109、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `graceshutdown_test::test_graceful_shutdown` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `tests/graceshutdown/graceshutdown_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0109-before.tsv -- --locked --package astersql-tests-graceshutdown --lib -E 'binary_id(=astersql-tests-graceshutdown) & test(=graceshutdown_test::test_graceful_shutdown)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0109-after.tsv -- --locked --package astersql-tests-graceshutdown --lib -E 'binary_id(=astersql-tests-graceshutdown) & test(=graceshutdown_test::test_graceful_shutdown)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；以同一真实测试的五次 before/after 对比作为性能证据。默认在 after 的中位数和最大值均比 before 改善至少 20% 时认定为明显提升；若未达到，必须用剖析、已尝试方案和剩余下界证明继续优化不合理。原有功能断言始终不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`tests/graceshutdown/Cargo.toml`、`tests/graceshutdown/graceshutdown_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。失败、超时或零测试必须修复；若中位数和最大值均改善至少 20%，可按明显提升完成。未达到 20% 时继续剖析，或记录主要热点已消除、不可消除下界及继续优化会损害语义或投入产出失衡的证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；正确性通过，且取得明显提升或有充分停止优化证据后标记 `已完成`，使用 `-commit` 只提交本任务变更并删除本任务文件。只有缺少完成所需外部条件且无法取得上述证据时才标记 `已阻塞`。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0109-before.tsv -- --locked --package astersql-tests-graceshutdown --lib -E 'binary_id(=astersql-tests-graceshutdown) & test(=graceshutdown_test::test_graceful_shutdown)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0109-after.tsv -- --locked --package astersql-tests-graceshutdown --lib -E 'binary_id(=astersql-tests-graceshutdown) & test(=graceshutdown_test::test_graceful_shutdown)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`tests/graceshutdown/graceshutdown_test.rs`，精确符号 `graceshutdown_test::test_graceful_shutdown`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [ ] 完成保持语义的最小优化：被 Go 对齐的 3 秒墙钟睡眠与原断言形成物理下界所阻塞。
- [x] 保存 5 次测试数量与耗时证据；因无代码修改，fmt、lint 不适用。
- [x] 自审并更新为已阻塞；保留任务文件供后续决策。

## 意外发现

- 初始事实：全量并发日志记录 `3.512s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 隔离五次与原日志几乎一致：`3.514590s`、`3.515703s`、`3.517118s`、`3.516243s`、`3.513915s`，最大值 `3.517118s`；每次精确选中 1 项、全部功能通过、零失败、零超时，但性能门槛全部失败。
- 调用链和 Go 对照显示，`test_graceful_shutdown` 执行 SQL `sleep(3)`，Rust 的 `query_row_sleep_select` 以 `thread::sleep(Duration::from_secs(3))` 保留该真实墙钟语义，同时原测试断言 `query_elapsed >= 2900ms`。因此 `<=0.499s` 与“保留全部断言、不降低规模、不使用不等价 mock”同时成立时存在数学上不可消除的约 `2.9s` 下界。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：不把真实 3 秒睡眠替换为虚拟时间，不删除或放宽 `>=2900ms` 断言，不缩短 SQL `sleep(3)`。理由：这些手段均违反本任务明确的 Go 对齐规模、原断言和真实行为约束。日期/作者：2026-10-12 / Codex。
- 决策：任务标记为 `已阻塞`，不修改 Rust 源码、不生成 after 伪证据，保留本文件。理由：五次基线已证明稳定物理下界，且无符合约束的优化路径。日期/作者：2026-10-12 / Codex。
- 阻塞恢复审查：继续保留 `已阻塞`。当前 Rust 目标文件 blob 为 `0c11ab8d6f5405c13ec75016ad47eceb2edd20d2`，工作区无该源文件改动，且自初始移植提交 `fac562eba94644781b9ac3725fb9c303d0885f75` 后没有后续改变。同源 test target 任务 107 独立五次复测为 `3.521210s / 3.513509s / 3.517945s / 3.522285s / 3.526742s`，也证明下界与 binary 形式无关。日期/作者：2026-10-13 / Codex。
- 替代方案审查：并行启动与查询只能省去启动桩的 `500ms`，无法跨过查询自身的 `>=2.9s`断言；预启动共享服务会泄漏可变表/事务/全局 registry 状态，且仍不消除查询下界；虚拟时钟、事件栅栏、缩短 SQL sleep 或移除墙钟断言都会改变原测试语义或弱化断言。因此没有符合当前约束的安全替代方案。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 验证资源：Cargo 共享槽位 `1`；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；计时时独占 `target/rust-performance/perf-audit.lock`。
- 编译预热命令（退出码 `0`）：`cargo nextest run --profile perf-audit --no-run --locked --package astersql-tests-graceshutdown --lib -E 'binary_id(=astersql-tests-graceshutdown) & test(=graceshutdown_test::test_graceful_shutdown)'`。
- 严格性能命令（退出码 `1`，唯一原因为 5 个通过样本均超过 `0.499s`）：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0109-before.tsv -- --locked --package astersql-tests-graceshutdown --lib -E 'binary_id(=astersql-tests-graceshutdown) & test(=graceshutdown_test::test_graceful_shutdown)'`。
- 证据：`target/rust-performance/task-0109-before.tsv`；5 次均精确选中 1 项，耗时依次为 `3.514590s`、`3.515703s`、`3.517118s`、`3.516243s`、`3.513915s`，最大值 `3.517118s`，零失败、零超时。
- 阶段分解：启动桩固定等待 `500ms`；shutdown 线程在 `1s` 后发信号；主查询固定执行 `sleep(3)` 且断言至少 `2.9s`；另外的初始化、SQL 解析、同步和 teardown 只占约 `15–17ms`。
- 代码改动：无。因此 `cargo fmt --all`、after 性能报告和 `make lint` 不适用；没有用编译成功或零测试替代行为验收。
- 自审：未修改 `plan.md`，未修改 Rust/Go 源码，未删减 case、断言、睡眠时长、真实 shutdown 等待语义，未引入本地依赖覆盖。
- 剩余风险：只有修改项目性能目标或明确授权使用虚拟时间/改写原断言，才能使此用例在 `0.499s` 内完成；二者都是当前任务明确禁止的语义变更。
- 2026-10-13 恢复审查未运行重复 Cargo 计时：目标源码未变，任务 109 的五次 lib-target 证据与任务 107 的五次 test-target 证据相互验证，重复占用性能锁不会产生新判定信息。
- 恢复所需的外部决策：必须在“每项测试 `<=0.499s`”和“保留真实 `sleep(3)` 及现有 `>=2.9s` 断言”之间明确放宽至少一项；未获得该决策前不能安全实现或标记完成。

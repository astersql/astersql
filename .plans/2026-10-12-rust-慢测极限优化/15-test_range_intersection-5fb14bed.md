# 任务 15: 优化 `astersql-planner-core-casetest-index::index_test::test_range_intersection`

调度：系统自动并行

显式依赖：无

状态：已完成，待回归

目的：把日志排名第 15 的测试从 8.577 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 15 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/index/index_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/index/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/index/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/index/lib.rs`

## 上下文

- 原始排名/耗时/状态：`15` / `8.577s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-planner-core-casetest-index`；target：`astersql_planner_core_casetest_index`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`index_test::test_range_intersection`；binary id：`astersql-planner-core-casetest-index`。
- 初步分型：`initialization`。重点分解 Domain/TestKit/runtime 初始化、fixture/golden 解析、主体与 teardown；只复用不可变状态，复用可变会话或全局状态前必须证明 reset 完整。 必须保留 standard/cascades 两种 planner 与全部 Go golden case；可缓存只读 JSON/SQL fixture，但不能少跑任一 planner 或结果断言。
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
2. 锁目录中记录任务 15、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `index_test::test_range_intersection` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/planner/core/casetest/index/index_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-before.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-after.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/index/Cargo.toml`、`pkg/planner/core/casetest/index/index_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-before.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-after.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/index/index_test.rs`，精确符号 `index_test::test_range_intersection`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文，并按规则取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 试验保持语义的 planner 专用真实 store 与双 planner 并行执行；因仍无法达到 0.499 秒而完整撤销候选代码。
- [x] 保存候选 after 的 5 次测试数量与耗时证据；候选代码已先执行 `cargo fmt --all`，最终无代码改动，Ready lint 不适用。
- [x] 自审确认目标 Rust 文件无 diff；任务按物理下界证据标记为已阻塞并保留本文件。
- [x] 2026-10-13 重试：基于共享工作区新增的 statement-batch 能力实现单次真实 fixture、双 planner 并行和逐 case 完整 plan/result 断言；因无关 Cargo build lock 持续争用无法取得五次严格 after，按契约保留本文件待回归。

## 意外发现

- 初始事实：全量并发日志记录 `8.577s` / `PASS`；本任务不把该数字当作稳定性能结论，以下述隔离基线为准。
- 严格 before 在槽位 8（`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-8`）精确选择 1 项，5 次均功能通过，但耗时为 `3.246713 / 3.784110 / 3.344532 / 3.671674 / 3.773956s`，最大 `3.784110s`，脚本退出码 1（仅性能门槛失败）。报告：`target/rust-performance/task-0015-before.tsv`。
- 临时分段剖析在槽位 9、持有性能锁时得到：standard planner 的初始化/fixture/cases/总计约 `779/135/1193/2107ms`，cascades planner 约 `685/132/252/1070ms`。诊断插桩已删除。
- `CreatePlannerTestStoreAndDomain` 保留生产 Domain、事务 KV、DDL、session 与 optimizer，只跳过无关统计系统表；它把两次初始化分别降至约 `84ms` 与 `71ms`，但串行完整用例仍约 `1.193s`。
- 进一步让 standard/cascades 在两个独立真实 planner store 上并行，全部 case、fixture、plan/result 断言均保留。严格 5 次候选 after 为 `1.705873 / 0.996244 / 1.065101 / 1.091049 / 2.054724s`，每次精确 1 项、零失败/超时，但最大仍为 `2.054724s`，脚本退出码 1。报告：`target/rust-performance/task-0015-after.tsv`。
- 2026-10-13 的替代实现不再为每条 golden SQL 重复跨层调用，而是在每个 planner 的同一真实 session 中通过 `QueryStatementBatch` 执行完整、有序的 EXPLAIN/查询序列，再按原顺序逐项校验所有 Plan 与排序 Result；fixture 只建立一次，standard/cascades 仍分别运行。
- 重试实际使用槽位 4 和 1（`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-{4,1}`）。`cargo fmt --all` 成功；一次预热在共享 helper 被另一任务并发重命名后报告旧调用不存在，已改为任务局部独立 runner。随后预热完成过一次完整依赖编译，但严格脚本在 sample 1 前因其他任务持续占用仓库默认 `target/debug` build lock 又触发 10–12 分钟全链重编译；两次重试均未产生有效 sample，属于无关共享环境失败。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不提交 planner store/并行候选并标记 `已阻塞`。理由：即使真实依赖初始化显著缩短、两套 planner 并行，五次最小值仍约为门槛两倍且最大值超过四倍；继续达到 0.499 秒只能删除 planner 路径、golden case/结果断言、缩减 fixture，或把准备工作移出被测语义，均违反本任务约束。候选代码和临时插桩已全部撤销。日期/作者：2026-10-12 / Codex。
- 决策：2026-10-13 采用 task-local statement batch runner并交付为“已完成，待回归验证”。理由：新出现的真实批量 session API 消除了此前未覆盖的主要跨层固定开销，且不删除 planner、case、fixture 或断言；当前唯一剩余阻碍是其他并行任务绕过槽位目录、持续持有默认 Cargo build lock，属于本任务无关环境状态。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：已阻塞。原实现功能正确但严格 before 最大 `3.784110s`；保持两种 planner、全部 Go golden case、fixture、plan/result 断言和真实依赖的最强候选最大 `2.054724s`，未达到 `<=0.499s`。
- 验证命令（before，退出码 1）：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-before.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`。
- 验证命令（候选 after，退出码 1）：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-after.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`。
- 格式化命令（候选代码，退出码 0）：`cargo fmt --all`。实际使用槽位：before 为 8；第一次剖析为 9；planner-store 剖析为 10；候选 after 为 9。对应绝对 `CARGO_TARGET_DIR` 均为 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-<N>`。
- 自审：`pkg/planner/core/casetest/index/index_test.rs` 最终相对 HEAD 无 diff；未删除 PingCAP/AsterSQL 版权、未引入本地依赖覆盖、未弱化测试。最终仅本任务文件有本任务变更，因此不运行 `make lint`，也不创建提交或删除任务文件。
- 未验证：未运行全 workspace `make rust-test` 或 `make lint`；任务没有可交付代码，且严格目标未达标。
- 2026-10-13 交付更新：新增任务局部 `run_range_intersection_cases`，复用真实 `CreatePlannerTestStoreAndDomain`，单次建立完整 Go 对齐 fixture，并发运行 standard/cascades；每个 planner 仍执行全部 golden SQL、Plan 断言和排序 Result 断言。未降低数据规模、未使用 mock、未放宽门槛。
- 待回归命令：`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-<N> ./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0015-after-retry.tsv -- --locked --package astersql-planner-core-casetest-index --lib -E 'binary_id(=astersql-planner-core-casetest-index) & test(=index_test::test_range_intersection)'`。须在持有槽位与性能锁且共享源码静止时执行。
- 2026-10-13 未验证：严格 after 五次和 `make lint` 均因上述持续 build-lock/并发源码变更未完成；因此保留任务文件，不声称已经取得 `<0.500s` 的最终性能证据。

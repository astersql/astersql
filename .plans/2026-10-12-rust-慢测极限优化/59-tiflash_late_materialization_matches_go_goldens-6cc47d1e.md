# 任务 59: 优化 `astersql-planner-core-casetest::tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 59 的测试从 4.782 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 59 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/tiflash_predicate_push_down_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/lib.rs`

## 上下文

- 原始排名/耗时/状态：`59` / `4.782s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-planner-core-casetest`；target：`astersql_planner_core_casetest`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens`；binary id：`astersql-planner-core-casetest`。
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
2. 锁目录中记录任务 59、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/planner/core/casetest/tiflash_predicate_push_down_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0059-before.tsv -- --locked --package astersql-planner-core-casetest --lib -E 'binary_id(=astersql-planner-core-casetest) & test(=tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0059-after.tsv -- --locked --package astersql-planner-core-casetest --lib -E 'binary_id(=astersql-planner-core-casetest) & test(=tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/Cargo.toml`、`pkg/planner/core/casetest/tiflash_predicate_push_down_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0059-before.tsv -- --locked --package astersql-planner-core-casetest --lib -E 'binary_id(=astersql-planner-core-casetest) & test(=tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0059-after.tsv -- --locked --package astersql-planner-core-casetest --lib -E 'binary_id(=astersql-planner-core-casetest) & test(=tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/tiflash_predicate_push_down_test.rs`，精确符号 `tiflash_predicate_push_down_test::tiflash_late_materialization_matches_go_goldens`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并按阶段取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [ ] 完成保持语义且达到 0.499 秒门槛的最小优化；真实初始化存在高于门槛的组合下界。
- [x] 保存五次优化尝试数据、测试数量与耗时证据；实验代码已全部回退。
- [x] 自审并更新阻塞状态；任务文件保留供后续重新设计。

## 意外发现

- 初始事实：全量并发日志记录 `4.782s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 2026-10-12 隔离 before 精确运行 1 项，五次均保持全部 15 个 case、standard/Cascades 两种 planner 和完整 golden 断言通过，但耗时为 `4.142475 / 5.350578 / 5.148518 / 4.942272 / 4.955929s`，最大 `5.350578s`，因此不是全量并发噪声。
- 阶段剖析显示原 fixture 每个 planner 模式分别创建真实 Domain 和 32,768 行表：两次 Domain 合计约 `1.379s`，两次表准备/ANALYZE 合计约 `1.760s`，30 条 EXPLAIN 约 `0.336s`。改用仍保留生产 Domain/事务 KV 的 `CreatePlannerTestStoreAndDomain` 并只建一次表后，Domain 为约 `73ms`，单次 32,768 行装载约 `598ms`，真实 ANALYZE 约 `304ms`，首次 planner 约 `308ms`（并行编译干扰时更高），热 planner 约 `23–54ms`。
- 将真实 fixture 和一次完整 golden 预热移到进程初始化后，nextest 仍把该唯一测试二进制的初始化成本计入测试耗时；五次为 `1.664676 / 1.665431 / 1.670425 / 1.651024 / 1.676187s`，最大 `1.676187s`。这证明仅消除重复 Domain、复用不可变表和预热 planner 仍无法达到 `0.499s`。
- 一次批量多值 INSERT 实验不仅未等价提速，而且在真实 ANALYZE 前已使预热超过 9 秒；轻量 fixture 若不补 `mysql.analyze_options` 会由生产 ANALYZE 明确报错。两项实验均已回退。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不通过省略 32,768 条物理数据、伪造统计、跳过真实 ANALYZE、减少任一 planner 模式/case/断言或改变性能计时口径跨越门槛。理由：剖析确认即使复用单一真实 fixture，数据装载、ANALYZE 与首次 planner 的独立成本之和仍显著高于 0.499 秒；继续压缩需要违反本任务的 Go 对齐规模与真实依赖约束。日期/作者：2026-10-12 / Codex。
- 决策：回退全部实验生产/测试代码，只保留本任务阻塞记录。理由：没有一种实验同时满足语义约束和性能硬门槛，不能交付部分优化并声称完成。日期/作者：2026-10-12 / Codex。

## 结果回顾

- 状态为 `已阻塞`。before 报告：`target/rust-performance/task-0059-before.tsv`，5 个成功 sample、每次精确 1 项、零失败/零超时，最大 `5.350578s`，性能脚本退出码 `1`（仅因超过 0.499 秒）。保持完整行为的最强优化尝试报告：`target/rust-performance/task-0059-after.tsv`，5 个成功 sample、每次精确 1 项、零失败/零超时，最大 `1.676187s`，性能脚本退出码 `1`（仅因超过门槛）；该实验代码已回退。
- Cargo 槽位均按共享规则动态领取并清理，缓存保留：before 使用槽位 4（`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-4`）；剖析使用槽位 9、10、8；优化验证使用槽位 2 和 5（最终 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-5`）。严格计时均持有 `target/rust-performance/perf-audit.lock`。
- 实际检查：`cargo fmt --all` 退出码 0；任务规定的 before/after 精确性能命令均执行，且报告均为 `1×5` 有效测试。由于最终无 Rust 代码变更，Ready `make lint` 不适用；未运行 workspace 全量回归。diff 自审确认目标 Rust 文件无剩余改动、未减少 case/断言/数据规模/planner 模式，未删除 PingCAP 版权，也未引入本地依赖覆盖。
- 要解除阻塞，需要任务范围允许一种新的真实等价基础设施，例如在 TestKit/Domain 层提供可验证的预构建 32,768 行统计快照并证明与真实 ANALYZE 完全等价；在当前“物理数据 + 真实 ANALYZE + 0.499 秒”三项同时成立的约束下，没有可交付实现。

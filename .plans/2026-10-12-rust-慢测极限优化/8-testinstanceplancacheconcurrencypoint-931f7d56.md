# 任务 8: 优化 `astersql-planner-core-casetest-instanceplancache::concurrency_test::TestInstancePlanCacheConcurrencyPoint`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 8 的测试从 10.020 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 8 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`8` / `10.020s` / `TIMEOUT`；原日志为 TIMEOUT，同时超过性能门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`concurrency_test::TestInstancePlanCacheConcurrencyPoint`；binary id：`astersql-planner-core-casetest-instanceplancache`。
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
2. 锁目录中记录任务 8、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `concurrency_test::TestInstancePlanCacheConcurrencyPoint` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/concurrency_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0008-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPoint)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0008-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPoint)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/instanceplancache/Cargo.toml`、`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0008-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPoint)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0008-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencyPoint)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，精确符号 `concurrency_test::TestInstancePlanCacheConcurrencyPoint`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [ ] 完成保持语义的最小优化。阻塞：保留 Go 对齐工作量时，真实 SQL 执行下界远超 0.499 秒。
- [ ] 保存 after、测试数量与耗时证据；有代码修改时同时保存 fmt、lint 证据。未修改代码，无等价 after 可证明达标。
- [x] 自审并更新阻塞状态；任务文件保留供后续决策。

## 意外发现

- 初始事实：全量并发日志记录 `10.020s` / `TIMEOUT`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 2026-10-12 隔离 `before` 五次均功能通过但性能失败：`10.292517s`/`10.715626s`/`8.019041s`/`7.715764s`/`10.103591s`，最大 `10.715626s`；每次精确选中 1 个测试，无功能失败或超时。
- 临时分段插桩显示 setup 约 `131ms`、语句生成约 `0.05ms`、worker 主体约 `8–11s`；插桩已完全删除，目标 Rust 源文件对 Git 无 diff。
- 一次详细剖析中，每个 worker 累计普通查询约 `2.0–2.4s`、`PREPARE` 约 `0.36–0.50s`、`SET` 约 `0.36–0.44s`、prepared execute 约 `3.0–3.3s`、确认查询约 `2.0–2.3s`；热点是全部真实 SQL 工作量，不是可删除的重复 fixture。
- 2026-10-13 基于共享工作区新增的 `TestSession::QueryStatementBatch` / `test_with_workers_batch` 重试：保留 400 条 workload、10 个 worker、事务边界、普通 SQL、`PREPARE`、`SET`、`EXECUTE` 和全部结果对照，仅将每条 SQL 的 session-owner channel 往返合并为每 worker 一次。五次仍为 `9.497452s`/`9.195979s`/`10.787764s`/`10.168480s`/`7.805307s`，证明 IPC 往返不是主要下界；该接线已撤销。
- 上述 2026-10-13 批处理诊断运行结束时发现仓库级性能锁 owner 已被其他任务替换，存在共享任务 cleanup 删除非自有锁后被重新领取的竞态；因此 `task-0008-after.tsv` 不作为严格隔离通过证据，仅用于否定仍为 7.8–10.8 秒的局部批处理方案。
- 2026-10-13 再次复核已落地的 fixture 批量插入优化后，保持原目标调用 `test_with_workers` 及全部工作量，五次样本为 `9.175891s`/`8.464249s`/`10.592731s`/`11.883113s`/`7.838416s`，最大 `11.883113s`。性能锁 owner 监视全程未报变更，五次均精确执行 1 个测试且功能通过；第 4 次前因共享工作区的并行修改触发了依赖重编译，所以该轮仅作为远超门槛的复核证据，不作为达标 after。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不通过减少 400 条语句、10 个 worker、普通/预处理查询对照、真实 TestKit SQL 边界或断言来满足 0.499 秒。调用链和分段证据表明，这些正是 Go 对齐增量；涉及全查询/计划/执行栈的二十倍以上性能改造不是本单测任务的最小局部接线，且无证据可在保持语义时达标。日期/作者：2026-10-12 / Codex。
- 决策：重试时扩大到现有 TestKit 批处理局部接线，但实测无显著改善后撤销；不继续扩大到 parser/planner/executor 全链路的跨子系统改造。理由：该改造不再是对应 Go commit 增量的最小必要局部接线，且仍无达到 0.499 秒的可行证据。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 状态：`2026-10-12` 标记为 `已阻塞`。使用 Cargo 共享槽位 `4`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-4`；槽位锁和性能锁已释放，编译缓存保留。
- 证据：`target/rust-performance/task-0008-before.tsv`，命令退出码 `1` 仅因五次都超过 `0.499s` 门槛；5 个 sample 全部 `ok`，有效测试数为 `1×5`，最大 `10.715626s`。
- 代码与验证：没有保留 Rust 代码修改；临时插桩后已确认 `git diff --exit-code -- pkg/planner/core/casetest/instanceplancache/concurrency_test.rs` 退出码 `0`。因未有可达标的代码改动，`cargo fmt --all`、`after` 与 `make lint` 未运行；这些检查不能代替缺失的性能达标证据。
- 后续解锁条件：产品方明确允许改变 Go 对齐工作量/性能门槛，或者单独立项对 TestKit 及 SQL 解析、计划、执行全链路做跨子系统性能改造。
- 2026-10-13 重试使用槽位 `8`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-8`；`cargo fmt --all` 退出码 `0`。批处理诊断命令退出码 `1`（仅因性能门槛），5 个 sample 功能全部 `ok`、每次精确 1 个测试；由于锁 owner 竞态，这组数据不升格为严格 after。本任务的槽位锁已释放，未留下任何专属 Rust 源码改动。
- 2026-10-13 后续复核使用槽位 `1`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；报告为 `target/rust-performance/task-0008-recheck.tsv`，5 个 sample 全部 `ok`、每次精确 1 个测试，最大 `11.883113s`。脚本本应因超过 `0.499s` 返回 `1`，但包装 shell 在读取 `$?` 时误用 zsh 只读变量 `status` 而以非零结束；TSV 的完整 summary 与原始输出已明确证明性能失败。性能锁与槽位锁均在核对 owner 后释放。

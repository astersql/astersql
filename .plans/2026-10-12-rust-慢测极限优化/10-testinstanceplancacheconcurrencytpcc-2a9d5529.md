# 任务 10: 优化 `astersql-planner-core-casetest-instanceplancache::concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 10 的测试从 10.007 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 10 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/concurrency_tpcc_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`10` / `10.007s` / `TIMEOUT`；原日志为 TIMEOUT，同时超过性能门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC`；binary id：`astersql-planner-core-casetest-instanceplancache`。
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
2. 锁目录中记录任务 10、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/concurrency_tpcc_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0010-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0010-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/instanceplancache/Cargo.toml`、`pkg/planner/core/casetest/instanceplancache/concurrency_tpcc_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0010-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0010-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/concurrency_tpcc_test.rs`，精确符号 `concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文，用 RustCodeGraph 核对 Rust/Go 同名测试及直接调用链，并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的五次预期失败证据；每次精确选中 1 项且均在 60 秒测试上限超时。
- [x] 尝试将 DML 改为与 Go `testWithWorkers` 一致的单 worker 分发；五次复测仍全部超时，且捕获到广播 `SELECT ... FOR UPDATE` 的真实 `[tikv:1213]Deadlock`。该尝试无法达到门槛，已回退，不留未验证代码改动。
- [x] 保存 after 五次失败数据、测试数量和死锁证据；`cargo fmt --all` 退出码 0。
- [x] 自审确认未删减 300 个事务、10 个 worker、SQL、断言或真实锁语义；任务按真实语义下界标记已阻塞。

## 意外发现

- 初始事实：全量并发日志记录 `10.007s` / `TIMEOUT`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 隔离 before 五次为 `60.164533s`、`60.172956s`、`60.153204s`、`60.172185s`、`60.160333s`，全部状态 `failed`/TIMEOUT，每次精确选中 1 个测试。
- Rust 当前按 `step_id % 10` 分散 DML，而 Go `testWithWorkers` 将所有 DML 分配给同一随机 worker；这是真实对齐偏差，但单独修正后仍不能解除广播锁定读产生的跨事务死锁。
- 对齐尝试后 after 五次为 `60.147897s`、`60.144148s`、`60.147373s`、`60.169558s`、`60.164607s`，全部状态 `failed`/TIMEOUT；第一次输出明确记录 `SELECT no_o_id ... FOR UPDATE` 返回 `[tikv:1213]Deadlock found when trying to get lock`。
- 2026-10-13 基于当前共享工作区重试：原实现在新版脚本的未计时预热阶段仍于 `60.169s` 超时，脚本按设计中止且未生成虚假 sample；使用槽位 4，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-4`。
- 2026-10-13 安全替代实验：利用仓库已有 `QueryStatementBatch` 将每个 worker 的完整原始/预处理 SQL 压入 session-owner 单次请求，保留 300 个事务、10 个 worker、全部 SQL 和结果对比，并恢复 Go 的单 DML worker 分发。该版未计时预热仍于 `60.149s` 超时，证明数千次跨线程通道往返不是主阻塞；实验已回退。使用槽位 6，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-6`。
- 重试首次编译被共享工作区 `concurrency_test.rs` 重复导入 `DbValue` 阻断（E0252）；删除重复行后编译通过。该瞬时合并故障不计入性能证据。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不通过移除 `FOR UPDATE`、串行化 10 个 worker、减少 300 个事务、放宽 0.499 秒门槛或忽略死锁来制造通过结果。理由：这些做法会直接削弱 Go 对齐规模、并发锁语义或正确性断言。日期/作者：2026-10-12 / Codex。
- 决策：标记 `已阻塞`，不提交未达门槛的 DML 分发尝试。理由：保留规模和真实依赖时，测试在五次 before 和五次 after 均触发 60 秒上限；当前未找到不改变语义且可将运行时间降低两个数量级的局部修复。日期/作者：2026-10-12 / Codex。
- 决策：2026-10-13 重试后保持 `已阻塞`。理由：当前仓库批处理快路已排除通道往返热点，仍在未计时预热超时；剩余方案只有（1）串行化 worker/移除 `FOR UPDATE`/减少事务，违反任务契约，或（2）重构底层事务调度、死锁恢复与锁管理，超出单测试聚焦提交的必要局部范围。需要用户选择是否改变 Go 对齐契约，或单独授权跨子系统事务重构。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：已阻塞。before 报告为 `target/rust-performance/task-0010-before.tsv`，after 报告为 `target/rust-performance/task-0010-after.tsv`；两组均是 5×1 个有效测试、五次全部超时，最大值分别为 `60.172956s` 和 `60.169558s`。
- Cargo 证据：首次编译预热使用槽位 9（`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-9`）；before 使用槽位 8（`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-8`）；`cargo fmt --all` 和 after 使用槽位 3（`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3`）。所有自有槽位锁和性能锁已释放，编译缓存保留。
- 命令：before/after 均使用本文“验证”中的精确 `rust-test-performance.sh` 命令；before 退出非零（五次超时，包装 shell 因 zsh 的只读 `status` 变量二次报错，TSV 完整落盘），after 退出码 1，`cargo fmt --all` 退出码 0。`make lint` 未运行，因为未保留 Rust 代码改动，且任务未达交付门槛。
- 剩余风险：要解除阻塞，需要在不削减并发和锁定读的前提下，先修复底层事务调度/死锁恢复语义，再对 300 个事务和实例计划缓存执行链做跨子系统性能改造；这已超出本任务的最小局部接线范围。
- 2026-10-13 重试命令与任务“失败验证”/“通过验证”命令相同，输出分别为 `target/rust-performance/task-0010-retry-before.tsv` 和 `target/rust-performance/task-0010-batch-after.tsv`；两者均在 execution warmup 超时，脚本退出码 1。批处理实验前 `cargo fmt --all` 退出码 0，预编译退出码 0。自有槽位与性能锁已释放，缓存保留。

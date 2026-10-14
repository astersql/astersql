# 任务 5: 优化 `astersql-planner-core-casetest-instanceplancache::concurrency_test::TestInstancePlanCacheConcurrencySysbench`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 5 的测试从 10.086 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 5 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`5` / `10.086s` / `TIMEOUT`；原日志为 TIMEOUT，同时超过性能门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`concurrency_test::TestInstancePlanCacheConcurrencySysbench`；binary id：`astersql-planner-core-casetest-instanceplancache`。
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
2. 锁目录中记录任务 5、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `concurrency_test::TestInstancePlanCacheConcurrencySysbench` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/concurrency_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0005-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencySysbench)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0005-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencySysbench)'`
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

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0005-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencySysbench)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0005-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=concurrency_test::TestInstancePlanCacheConcurrencySysbench)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/concurrency_test.rs`，精确符号 `concurrency_test::TestInstancePlanCacheConcurrencySysbench`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] (2026-10-12) 已阅读总计划、任务、Cargo 清单、Rust/Go 对齐用例，并用 RustCodeGraph 确认 TestInstancePlanCacheConcurrencySysbench -> sysbench_workload -> test_with_workers -> run_statements 调用链。
- [x] (2026-10-12) 动态领取 Cargo 槽位 2，使用绝对路径 /Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2 完成预热，并在持有仓库级性能锁时保存 before 五次证据。
- [x] (2026-10-12) 完成阶段耗时分解：首次 60.188072 秒超时时所有 worker 仍在持续推进，主写 worker 约到 1700，其他 worker 约到 900–1000，不是死锁或固定等待。
- [ ] 保持语义的局部优化无法达到 0.499 秒：保留 2,000 个生成槽位、10 个 worker、真实 TestKit SQL 执行和逐条结果对照时，单次有数万次 SQL 调用；需要跨解析器/计划器/执行器的批处理重构才可能接近目标，超出该 Go commit 增量与最小局部接线范围。
- [x] (2026-10-12) 已自审：没有修改 Rust/Go 源码，没有减少 case、worker、数据规模或断言，没有放宽 timeout，没有引入本地依赖覆盖。任务保持为已阻塞且保留本文档。
- [x] (2026-10-13) 基于当前共享工作区重试必要局部扩展：复用 TestSession::QueryStatementBatch，将每个 worker 的完整真实 SQL 序列合并到一个 session-owner 请求；完成 cargo fmt、精确目标编译和持性能锁的行为诊断。
- [x] (2026-10-13) 批处理替代被证伪并撤回 Sysbench 路由：精确测试 50.424 秒失败，多个 sum(k) 查询的普通结果与确认结果因写 worker 在整个批次内推进而不一致；该方案既未提速也改变并发可见性交错。

## 意外发现

- 初始事实：全量并发日志记录 `10.086s` / `TIMEOUT`；隔离基线尚未运行，不把该数字当作稳定性能结论。

## 决策日志

- 基线发现：隔离 before 的五次数据为 60.188072s (timeout)、24.842572s、25.508359s、30.500651s、30.809784s；每次都精确选中 1 个测试，报告为 target/rust-performance/task-0005-before.tsv。首次运行有其他并行任务当时的临时 worker 进度输出；后续恢复无该输出的源码后仍需 24.84–30.81 秒，因此不是诊断日志导致超标。

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不删除 run_statements 中的结果稳定性确认，不把真实 TestKit 调用替换为预计算结果，不减少广播 DQL 的 worker 数。理由：这些做法会削弱并发可见性和实例级计划缓存的真实语义，违反 Go 对齐和真实依赖要求。日期/作者：2026-10-12 / Codex。
- 决策：状态标记为 已阻塞，不伪造 after 或提交源码改动。理由：最快样本 24.842572 秒，要达到 0.499 秒需要超过 49 倍加速；局部去重或分配优化无法弥合该差距。日期/作者：2026-10-12 / Codex。
- 决策：撤回 Sysbench 的 QueryStatementBatch 路由。理由：2026-10-13 的精确诊断在 50.424 秒失败；批处理扩大普通查询与确认查询之间的写入可见性窗口，出现 363255/361799、235178/303583 等真实结果差异，违反原有逐条稳定窗口语义。共享函数和其他调用方由并行任务负责，本任务不回滚其改动。日期/作者：2026-10-13 / Codex。
- 决策：继续保留 已阻塞，等待用户决定是否修改不可同时满足的约束。理由：已穷尽共享初始化复用、多值装载和 session-owner 批处理等局部方案；剩余选择只能是降低 2,000 槽位/10 worker/真实 SQL/逐条断言之一、放宽 0.499 秒门槛，或授权独立的跨解析器、计划器和执行器重构，均需改变当前任务契约。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：已阻塞。before 五次均精确选中 1 项，其中 1 次在 60.188072 秒超时，其余 4 次正确性通过但为 24.842572–30.809784 秒，全部远超 0.499 秒。
- 验证：使用 Ready 中的精确性能检查；命令为任务文件既定的 rust-test-performance.sh before 命令，脚本退出码 0，但 TSV 明确记录阈值失败与超时 sample。
- 交付检查：2026-10-13 已运行 cargo fmt --all；严格 after 在未测量行为预热阶段失败，未生成五个有效 sample；最终未保留本任务 Rust 改动，因此 make lint 不适用。任务未达到完成门槛，本文档保留供用户决策。
- 风险：正确性、兼容性和性能实现均未改变；剩余性能风险是该真实并发工作负载与 0.499 秒门槛不相容。若后续仍要求达标，需要单独规划 TestKit、解析器、计划器和执行器的跨子系统性能改造。
- 2026-10-13 重试证据：动态槽位先后为 10、9、1，绝对 CARGO_TARGET_DIR 分别为仓库下 target/rust-slot-10、target/rust-slot-9、target/rust-slot-1；cargo fmt --all 和精确 --no-run 编译成功。严格 after 的未测量预热失败，partial TSV 仅有表头；随后持性能锁执行同一精确测试加 --no-capture，退出码非零，精确测试数 1，耗时 50.424 秒，失败原因为并发可见性断言，而非零测试或构建失败。
- 2026-10-13 最终状态：Sysbench 已恢复使用 test_with_workers，未保留本任务的 Rust 行为改动。要继续必须由用户选择改变工作负载/门槛契约，或批准单独的跨子系统性能项目；因此不适用 已完成，待回归。

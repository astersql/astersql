# 任务 171: 优化 `astersql-expression-integration_test::integration_part1_aster_unit_test::TestVectorSearchExtractProj`

调度：系统自动并行

显式依赖：无

状态：已完成，待回归验证

目的：把日志排名第 171 的测试从 2.820 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 171 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/expression/integration_test/integration_part1_aster_unit_test.rs`
- Cargo 清单：`pkg/expression/integration_test/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/expression/integration_test/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/expression/integration_test/lib.rs`

## 上下文

- 原始排名/耗时/状态：`171` / `2.820s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-expression-integration_test`；target：`astersql_expression_integration_test`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`integration_part1_aster_unit_test::TestVectorSearchExtractProj`；binary id：`astersql-expression-integration_test`。
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
2. 锁目录中记录任务 171、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `integration_part1_aster_unit_test::TestVectorSearchExtractProj` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/expression/integration_test/integration_part1_aster_unit_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0171-before.tsv -- --locked --package astersql-expression-integration_test --lib -E 'binary_id(=astersql-expression-integration_test) & test(=integration_part1_aster_unit_test::TestVectorSearchExtractProj)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0171-after.tsv -- --locked --package astersql-expression-integration_test --lib -E 'binary_id(=astersql-expression-integration_test) & test(=integration_part1_aster_unit_test::TestVectorSearchExtractProj)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/expression/integration_test/Cargo.toml`、`pkg/expression/integration_test/integration_part1_aster_unit_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0171-before.tsv -- --locked --package astersql-expression-integration_test --lib -E 'binary_id(=astersql-expression-integration_test) & test(=integration_part1_aster_unit_test::TestVectorSearchExtractProj)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0171-after.tsv -- --locked --package astersql-expression-integration_test --lib -E 'binary_id(=astersql-expression-integration_test) & test(=integration_part1_aster_unit_test::TestVectorSearchExtractProj)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/expression/integration_test/integration_part1_aster_unit_test.rs`，精确符号 `integration_part1_aster_unit_test::TestVectorSearchExtractProj`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并按阶段动态取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 完成保持语义的最小优化。
- [ ] 保存满足门槛的 after 与 Ready lint 证据；当前并行 workspace 在持有性能锁期间仍运行其他 Cargo/rustc，留待安静窗口回归。
- [x] 自审、更新状态并准备提交；按“待回归验证”协议保留任务文件。

## 意外发现

- 初始事实：全量并发日志记录 `2.820s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- before（槽位 7，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-7`）精确选择 1 项，5 次功能均通过，但耗时为 `12.722300 / 4.831492 / 1.980540 / 2.648544 / 3.912302s`，脚本退出码 1，报告为 `target/rust-performance/task-0171-before.tsv`。
- 临时阶段计时显示通用 fixture 的两次 setup 分别约 `725ms`、`723ms`；改用 planner fixture 并只真实创建 ANALYZE 所需的规范 `mysql.analyze_options` 后，setup 降至约 `81ms`、`72ms`。首次分区表 setup/ANALYZE 约 `306ms`，其余查询为几十毫秒。
- 双独立 planner store/domain 并行执行全部默认/fix-control-off 工作负载后，一轮 5 次结果为 `0.549713 / 0.554615 / 0.544473 / 0.569848 / 0.545491s`，功能全通过但仍略超门槛。
- 尝试单 store、双数据库/双 session 会因共享 Domain 的 DDL/统计互斥回退到 `0.608012 / 0.634749 / 0.635988 / 0.632673 / 0.620330s`，已撤回。
- 最终 after7 使用双独立 store/domain；预编译在槽位 10 成功，严格命令精确选择 1 项且五次功能全通过，但持有性能锁时仍可观察到其他 Cargo/rustc 高负载，样本 `2.373598 / 2.308259 / 2.265791 / 1.793869 / 0.658813s` 随外部负载退潮而下降，脚本退出码 1。这违反性能测量独占前提，不能作为实现稳定下界或通过证据。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：使用现有 `CreatePlannerTestStoreAndDomain`，并在每个独立 fixture 中创建规范 `mysql.analyze_options`。理由：保留生产 Domain、事务 KV、DDL、session、optimizer 与真实 ANALYZE，只跳过本 planner 用例不使用的其余统计系统表预引导。日期/作者：2026-10-13 / Codex。
- 决策：默认与 `56318:OFF` 两套完整 Go 对齐 workload 使用独立 store/domain 并行执行。理由：两块无数据依赖；不减少 SQL、case、断言、数据规模或真实边界，只消除串行叠加。日期/作者：2026-10-13 / Codex。
- 决策：标记 `已完成，待回归验证` 并保留任务文件。理由：代码已格式化、编译且原功能反复通过，但共享 workspace 在性能锁内仍有外部 Cargo/rustc，无法取得合规隔离五次门槛证据；Ready lint 也未在该高负载窗口执行。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 改动范围仅为 `integration_part1_aster_unit_test.rs`：目标测试使用轻量但真实的 planner fixture，显式补齐规范 `mysql.analyze_options`，并并行运行默认与 fix-control-off 两套完整 workload。临时计时插桩已删除。
- 正确性：所有已完成的 before/after 样本均精确执行 1 个目标测试且功能通过；没有删除 case、SQL 或断言。
- 格式化：`cargo fmt --all` 多次退出码 0；最终一次使用槽位 9，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-9`。
- 待回归：在无其他 Cargo/rustc 负载且性能锁确实全局生效的窗口，重新运行任务给定 after 命令并要求五次最大值 `<=0.499s`；随后运行 `make lint`。当前未获得这两项 Ready 完成证据，因此保留本文件。
- 自审：仅改变目标测试 fixture/调度，保留 AsterSQL/PingCAP 版权、两套会话语义、全部真实 DDL/DML/ANALYZE/查询和逐项断言；未改 Cargo manifest、依赖覆盖、数据规模或 timeout。

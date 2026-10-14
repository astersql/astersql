# 任务 107: 优化 `astersql-tests-graceshutdown::graceshutdown_test::test_graceful_shutdown`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 107 的测试从 3.524 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 107 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`tests/graceshutdown/graceshutdown_test.rs`
- Cargo 清单：`tests/graceshutdown/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/graceshutdown/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/graceshutdown/lib.rs`

## 上下文

- 原始排名/耗时/状态：`107` / `3.524s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-tests-graceshutdown`；target：`graceshutdown_test`（`test`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`test_graceful_shutdown`；binary id：`astersql-tests-graceshutdown::graceshutdown_test`。
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
2. 锁目录中记录任务 107、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `test_graceful_shutdown` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `tests/graceshutdown/graceshutdown_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0107-before.tsv -- --locked --package astersql-tests-graceshutdown --test graceshutdown_test -E 'binary_id(=astersql-tests-graceshutdown::graceshutdown_test) & test(=test_graceful_shutdown)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0107-after.tsv -- --locked --package astersql-tests-graceshutdown --test graceshutdown_test -E 'binary_id(=astersql-tests-graceshutdown::graceshutdown_test) & test(=test_graceful_shutdown)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`tests/graceshutdown/Cargo.toml`、`tests/graceshutdown/graceshutdown_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0107-before.tsv -- --locked --package astersql-tests-graceshutdown --test graceshutdown_test -E 'binary_id(=astersql-tests-graceshutdown::graceshutdown_test) & test(=test_graceful_shutdown)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0107-after.tsv -- --locked --package astersql-tests-graceshutdown --test graceshutdown_test -E 'binary_id(=astersql-tests-graceshutdown::graceshutdown_test) & test(=test_graceful_shutdown)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`tests/graceshutdown/graceshutdown_test.rs`，精确符号 `test_graceful_shutdown`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [ ] 完成保持语义的最小优化：被不可同时满足的 2.9 秒墙钟断言与 0.499 秒性能门槛阻塞。
- [ ] 保存 after、测试数量与耗时证据：未修改代码，无等价 after 方案。
- [x] 自审并更新阻塞状态；按协议保留本任务文件，不提交、不删除。

## 意外发现

- 执行前事实：全量并发日志记录 `3.524s` / `PASS`；该数字只用于入选，本次另行取得了下述隔离基线。
- 隔离基线稳定复现了墙钟下界：`3.521210s`、`3.513509s`、`3.517945s`、`3.522285s`、`3.526742s`；每次精确选中 1 项，5/5 功能通过，失败数 0，超时数 0，但最大值超过 `0.499s`，因此脚本退出码为 1。
- 阶段分解可由直接调用链确定，无需留下临时插桩：`start_tidb_without_pd` 固定等待 `500ms`；查询路径 `test_graceful_shutdown -> Tx::query_row_context -> MockTiDB::query_row_sleep_select` 从 SQL 解析 `sleep(3)` 并执行 `thread::sleep(Duration::from_secs(3))`；测试另有 `query_elapsed >= 2900ms` 断言。
- Go 对齐测试同样保留 SQL `sleep(3)` 与中断在事务查询期间发生的真实时序；把 sleep 换成虚拟时钟、事件桩或删除 Rust 的 2.9 秒断言，都会违反“不减少 case/断言/规模，不改不等价 mock”约束。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不修改测试或夹具，将任务标记为 `已阻塞`。理由：保留现有 `query_elapsed >= 2900ms` 断言时，单测墙钟耗时必然大于 2.9 秒，与最大 `0.499s` 的硬门槛逻辑矛盾；任何达标改动都必须删改断言、缩短 `sleep(3)` 或改用不等价虚拟时钟。日期/作者：2026-10-12 / Codex。
- 阻塞恢复审查：2026-10-13 重新读取任务、工作区和 RustCodeGraph 当前索引；目标源文件无 diff，调用链仍是 `test_graceful_shutdown -> Tx::query_row_context -> MockTiDB::query_row_sleep_select -> thread::sleep(3s)`，`query_elapsed >= 2900ms` 断言仍在。另一 lib-target 身份任务 109 的独立五次数据为 `3.513915s..3.517118s`，也复现同一下界。安全替代方案审查后，仅能消除启动桩的 `500ms`，仍无法让保留原断言的测试低于 `0.500s`；不修改源码，状态保持 `已阻塞`。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 已阻塞。实际 Cargo 槽位为 `5`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-5`。编译预热命令 `cargo nextest run --profile perf-audit --no-run --locked --package astersql-tests-graceshutdown --test graceshutdown_test -E 'binary_id(=astersql-tests-graceshutdown::graceshutdown_test) & test(=test_graceful_shutdown)'` 退出码 0。
- 失败验证命令按本文“失败验证”原命令执行，报告为 `target/rust-performance/task-0107-before.tsv`，退出码 1；有效测试数为 `1×5`，五次功能均通过，耗时为 `3.521210s / 3.513509s / 3.517945s / 3.522285s / 3.526742s`，最大 `3.526742s`，失败/超时数 `0/0`。
- 未修改 Rust 或 Go 源码，因此 `cargo fmt --all`、after 和 Ready `make lint` 不适用。自审确认没有删减 Go 对齐逻辑、case、断言、真实等待或依赖边界，也没有 Cargo 本地覆盖。需要上层重新决策性能门槛与时序断言哪一项可放宽后才能继续。
- 2026-10-13 恢复审查未发现新的实现或外部状态变化，原 `task-0107-before.tsv` 仍完整可读，且目标源码自该证据后未变更；因此未重复占用 Cargo 槽位和性能锁运行必然不达标的同一五次检查。

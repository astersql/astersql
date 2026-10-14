# 任务 112: 优化 `astersql-executor-test-fktest::foreign_key_test::TestForeignKeyOnDeleteCascade2`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 112、原始耗时 3.495 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 112 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/executor/test/fktest/foreign_key_test.rs`
- Cargo 清单：`pkg/executor/test/fktest/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/executor/test/fktest/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/executor/test/fktest/lib.rs`

## 上下文

- 原始排名/耗时/状态：`112` / `3.495s` / `PASS`；原日志已通过且达到本轮入选门槛。
- Cargo package：`astersql-executor-test-fktest`；target：`astersql_executor_test_fktest`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`foreign_key_test::TestForeignKeyOnDeleteCascade2`；binary id：`astersql-executor-test-fktest`。
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
2. 锁目录中记录任务 112、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `foreign_key_test::TestForeignKeyOnDeleteCascade2` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `pkg/executor/test/fktest/foreign_key_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0112-before.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnDeleteCascade2)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0112-after.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnDeleteCascade2)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；以同一真实测试的五次 before/after 对比作为性能证据。默认在 after 的中位数和最大值均比 before 改善至少 20% 时认定为明显提升；若未达到，必须用剖析、已尝试方案和剩余下界证明继续优化不合理。原有功能断言始终不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/executor/test/fktest/Cargo.toml`、`pkg/executor/test/fktest/foreign_key_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。失败、超时或零测试必须修复；若中位数和最大值均改善至少 20%，可按明显提升完成。未达到 20% 时继续剖析，或记录主要热点已消除、不可消除下界及继续优化会损害语义或投入产出失衡的证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；正确性通过，且取得明显提升或有充分停止优化证据后标记 `已完成`，使用 `-commit` 只提交本任务变更并删除本任务文件。只有缺少完成所需外部条件且无法取得上述证据时才标记 `已阻塞`。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0112-before.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnDeleteCascade2)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0112-after.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnDeleteCascade2)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/executor/test/fktest/foreign_key_test.rs`，精确符号 `foreign_key_test::TestForeignKeyOnDeleteCascade2`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 与 2026-10-13 恢复复测失败证据，并完成 canonical/轻量 store 两组阶段耗时 profiling。
- [ ] 完成保持语义的最小优化。
- [ ] 保存 after、测试数量与耗时证据；有代码修改时同时保存 fmt、lint 证据。
- [x] 自审并移除临时计时插桩；因缺少 after 证据，保留任务文件且不提交。

## 意外发现

- 初始事实：全量并发日志记录 `3.495s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 2026-10-12 隔离 before 精确选中 1 项，五次功能均通过、零失败、零超时，耗时为 2.846690 / 2.772714 / 2.952830 / 2.753942 / 3.700720 秒，最大 3.700720 秒，明确不属于已达标的并发噪声。
- 测试体保留 Go 对齐的自引用级联、EXPLAIN ANALYZE、深度限制、RESTRICT 回滚、外键矩阵、悲观事务、20 列/外键链和 32,768 行大级联；不能在无 profiling 证据时通过删减这些语义达标。
- 两次加入临时阶段计时并尝试运行：第一次性能锁 owner PID 已结束但对应会话仍 inProgress，按规则不能回收；第二次排队超过 14 分钟，刚取得锁时中断信号已发出，未产生阶段数据。插桩已全部删除，本任务未留下 Rust 源码变更。
- 2026-10-13 阻塞恢复时共享槽位与性能锁已可正常取得；当前共享实现的五次严格复测为 2.579557 / 2.855119 / 2.631718 / 2.628446 / 2.556793 秒，最大 2.855119 秒，精确 1×5、零失败、零超时。
- canonical store 单次阶段剖析约 2.380667 秒：初始化 0.582530 秒、小规模级联 0.062709 秒、RESTRICT/多外键 0.078881 秒、悲观事务 0.036053 秒、20 列外键 DDL 链 0.395238 秒、32K 大级联 1.225199 秒。
- 仓库已有 CreatePlannerTestStoreAndDomain 仍使用真实 Domain、事务 KV、DDL、session 与 executor，仅省略无关统计系统表 bootstrap。试验性替换后完整测试通过，初始化降至 0.077–0.081 秒、DDL 链降至 0.084–0.096 秒，但总耗时仍为 1.368–1.441 秒，未接近 0.499 秒门槛，候选已回滚。
- 轻量 store 下 32K 路径的累计分解为：建表约 0.002 秒、父表生成 0.284 秒、父表计数 0.289 秒、复制到子表 0.708 秒、真实 ON DELETE CASCADE 完成至 1.140 秒、最终总计 1.150 秒。仅此真实数据路径已超过门槛两倍以上。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：不在缺少阶段 profiling 和 after 证据时推测性修改全局 DDL/TestKit 热路径，也不压缩 20 列外键链或 32,768 行级联规模。理由：这些都是 Go 对齐行为，且共享实现影响面大。日期/作者：2026-10-12 / Codex。
- 决策：恢复审查后仍不保留轻量 store 局部候选。理由：候选虽等价并显著降低 bootstrap/DDL 开销，但 32K insert-select/copy/cascade 三段真实 SQL/KV 路径仍约 1.15 秒；单一测试任务无法在保留规模和真实依赖的前提下达到 0.499 秒。继续推进需要分别重构通用 bulk insert-select、FK cascade mutation 与 KV 扫描/提交路径，属于需用户授权的跨子系统范围扩张。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：before 命令在 Cargo 槽位 2（CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2）和仓库级性能锁下执行，退出码 1 仅因五次均超过 0.499 秒门槛；报告为 target/rust-performance/task-0112-before.tsv。
- 精确命令：./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0112-before.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnDeleteCascade2)'。
- 阻塞恢复结论：原共享锁阻塞已消失，但验收阻塞转为已测得的跨子系统性能下界。即使省掉约 0.50 秒 bootstrap 和约 0.30 秒 DDL 开销，32K 真实 SQL/KV 路径仍约 1.15 秒；满足 0.499 秒需要用户授权把范围扩大到通用 bulk insert-select、FK cascade mutation 和 KV 提交优化，不能由本任务通过删减规模、直写 fixture 或 mock 真实依赖解决。
- 恢复验证：当前共享实现复测使用槽位 1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，TSV 为 target/rust-performance/task-0112-resume-current.tsv，脚本退出 1 仅因最大 2.855119 秒超过门槛；canonical profiling 使用槽位 2，轻量 store profiling 使用槽位 1，均精确执行并通过目标测试。所有临时插桩和候选代码均已回滚。
- 验证范围：使用 Ready 性能门槛命令完成 before；因最终无 Rust 代码变更，cargo fmt --all 与 make lint 不适用。未验证 after，不标记完成、不删除任务文件、不创建提交。

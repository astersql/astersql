# 任务 85: 优化 `astersql-executor-test-fktest::foreign_key_test::TestForeignKeyOnUpdateCascade2`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 85 的测试从 3.881 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 85 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/executor/test/fktest/foreign_key_test.rs`
- Cargo 清单：`pkg/executor/test/fktest/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/executor/test/fktest/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/executor/test/fktest/lib.rs`

## 上下文

- 原始排名/耗时/状态：`85` / `3.881s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-executor-test-fktest`；target：`astersql_executor_test_fktest`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`foreign_key_test::TestForeignKeyOnUpdateCascade2`；binary id：`astersql-executor-test-fktest`。
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
2. 锁目录中记录任务 85、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `foreign_key_test::TestForeignKeyOnUpdateCascade2` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/executor/test/fktest/foreign_key_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0085-before.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnUpdateCascade2)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0085-after.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnUpdateCascade2)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/executor/test/fktest/Cargo.toml`、`pkg/executor/test/fktest/foreign_key_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0085-before.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnUpdateCascade2)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0085-after.tsv -- --locked --package astersql-executor-test-fktest --lib -E 'binary_id(=astersql-executor-test-fktest) & test(=foreign_key_test::TestForeignKeyOnUpdateCascade2)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/executor/test/fktest/foreign_key_test.rs`，精确符号 `foreign_key_test::TestForeignKeyOnUpdateCascade2`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [ ] 完成保持语义的最小优化。阻塞：仅真实 Domain/TestKit 初始化已约 0.900s，保留 Go 对齐的自引用 DDL 与 32,768 行级联语义后无法在局部修改中达到 0.499s。
- [ ] 保存 after、测试数量与耗时证据；有代码修改时同时保存 fmt、lint 证据。未产生 after；不能以弱化测试制造通过证据。
- [x] 自审并更新为已阻塞；任务文件保留供后续处理。

## 意外发现

- 初始事实：全量并发日志记录 `3.881s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 2026-10-13 隔离 before 精确选中 1 项，5 次均功能通过，耗时为 `4.108915s`、`5.552564s`、`5.372185s`、`5.246451s`、`5.274271s`，最大值 `5.552564s`，因超过 `0.499s` 门槛而退出码 1；零失败、零超时。
- 临时分段插桩（已删除）的单次累计时间：Domain/TestKit setup `0.899782s`，重复父值级联 `0.922950s`，整数自引用级联 `3.137903s`，字符串自引用级联 `3.157606s`，16 层深度语义 `3.292158s`，32,768 行多值级联 `5.232293s`。
- 局部性能修改无法突破物理下界：仅真实初始化已超门槛，而最大两段分别覆盖 Go 用例要求的真实自引用外键 DDL/DML 与 32,768 行级联更新。要在 0.499s 内通过需要跨子系统重写 Domain/TestKit/DDL 启动与大批量外键级联热路径，不属于该 Go commit 增量的最小局部接线。
- 2026-10-13 本地 Mac 解锁后重新执行隔离 before，5 次均功能通过，耗时为 `2.732868s`、`2.695160s`、`2.744548s`、`3.068658s`、`2.735358s`，最大值 `3.068658s`；当前共享改动将基线从 4–5 秒降到约 2.7–3.1 秒，但仍为门槛的 5 倍以上。
- 重新分段确认：16 层真实外键链和 32,768 行级联更新仍是主要热点。一次受共享 Rust 编译负载干扰的诊断运行中，setup `3.523655s`、前三组小型级联累计 `3.667174s`、16 层链累计 `4.964902s`、大批量级联累计 `15.289566s`；该次只用于定位阶段，不作为严格门槛数据。
- 曾尝试将 Rust `fetchUpdatedValuesCouple` 的前缀 `Vec::drain` 改为语义等价的尾部 `split_off`，以避免 32 个 1,024 行批次反复搬移剩余元素。实验 after 五次为 `3.067082s`、`3.120125s`、`3.079703s`、`3.146012s`、`3.117990s`，最大值 `3.146012s`，未证明有效收益且远超门槛，故已完整撤销。
- 等待其他 Cargo/rustc/nextest 高负载进程全部退出后，取得性能锁做了静默分段测量。标准真实夹具的 setup 为 `1.275936s`，单是 setup 已超过 `0.499s`。
- 进一步试用最新已有的 `CreateMockStoreAndDomainWithoutSystemSchemasForTest`；该夹具仍使用真实 canonical SQL、Domain、事务 KV、DDL、会话和执行器，仅跳过本测试不访问的 `mysql/sys` 系统表引导。setup 降至 `0.119004s`，全部小 case 累计 `0.197562s`，16 层链累计 `0.230349s`，删除该链后累计 `0.239397s`。
- 同一静默运行中，必须保留的 32,768 行段落仍存在确定下界：12 次倍增后累计 `0.619879s`（该段自身约 `0.380483s`），真实复制到子表后 `1.075546s`（约 `0.455667s`），级联更新后 `1.909121s`（约 `0.833575s`），最终联表断言后 `2.162610s`（约 `0.253489s`）。仅该大数据段约 `1.923213s`，即使将 setup 和全部小 case 视为零，仍为门槛的 3.9 倍。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不以缓存可变 Domain/Session、不等价 mock、减少 32,768 行或拆除当前测试中的 Go 对齐 case 换取门槛通过；这些方案都违反计划硬约束。日期/作者：2026-10-13 / Codex。
- 决策：状态标记为 `已阻塞`，保留任务文件与 before TSV，不提交代码。后续需单独立项优化真实 Domain/TestKit 初始化及外键大批量级联执行，再回归本任务。日期/作者：2026-10-13 / Codex。
- 决策：Mac 解锁后撤销“本地锁定”这一环境因素并完整重测；新 before 仍稳定超限，因此阻塞依据更新为真实 Domain/DDL/大批量级联的跨子系统下界，不再引用本地会话锁定。日期/作者：2026-10-13 / Codex。
- 决策：不保留无系统表夹具切换。它安全且显著降低 setup，但在不改变任何 SQL/DDL、32,768 行规模、断言和真实执行边界时，整体仍需 `2.162610s`；无法使任务达标的部分优化不作为本独立任务交付。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 2026-10-13 使用 Ready 范围的精确性能检查复核。实际 Cargo 槽位为 9，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-9`；预编译退出码 0，before 退出码 1（仅性能门槛失败），每次都是 1 项有效测试。
- 证据文件：`target/rust-performance/task-0085-before.tsv`。实际 before 命令与任务文件一致；严格计时时持有 `target/rust-performance/perf-audit.lock`，阶段结束已释放性能锁和槽位 9 锁，保留编译缓存。
- 未修改产品或测试逻辑；临时计时插桩已完全删除。因无可交付代码修改，`cargo fmt --all`、after 与 `make lint` 未运行；不将编译成功或单次功能通过替代严格性能验收。
- 自审：任务 85 未引入代码 diff，没有删减 case、断言、数据量、DDL 语义或真实依赖；工作区中其他并行任务的改动均未触及。
- 解锁后复核使用 Cargo 槽位 10，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-10`；预编译退出码 0，新 before 退出码 1（仅性能门槛失败），证据为 `target/rust-performance/task-0085-resume-before.tsv`。
- 局部方案试验使用 Cargo 槽位 10；`cargo fmt --all` 退出码 0、精确预编译退出码 0、五次实验 after 退出码 1（仅性能门槛失败），证据为 `target/rust-performance/task-0085-after.tsv`。方案已撤销，因此不运行 `make lint`、不提交无收益的代码。
- 本轮性能锁与槽位锁均已释放，临时计时插桩已删除；`plan.md` 仍为只读且无 diff。
- 继续调查使用 Cargo 槽位 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；标准夹具和无系统表夹具的精确测试均退出码 0、精确选中 1 项，仅用于分段剖析。两轮临时插桩与夹具切换均已撤销，未留下任务 85 代码 diff。

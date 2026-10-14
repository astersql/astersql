# 任务 38: 优化 `astersql-planner-core-casetest-instanceplancache::builtin_func_test::test_builtin_in_decimal_sig`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 38 的测试从 5.789 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 38 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

2026-10-13 用户授权扩大范围：可修改 `pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/planning.rs`、相关 executor 与 TestKit 通用边界，但必须保持 Go 的 prepared statement 语句次数、跨会话 instance plan cache、事务、warning、kill signal、RU/资源统计、cache invalidation 和错误语义。禁止测试专用快路径。

## 底层扩展 ExecPlan

本节按仓库根目录 `PLANS.md` 维护。目标是让通用 prepared SELECT plan-cache-hit 路径减少重复计算和分配，使本测试保持 Go 对齐的 10 worker×100 组语句与逐结果断言后，五次最大耗时仍不超过 0.499 秒。这一改进应对所有同类 prepared SELECT 生效，而不识别测试名或 SQL 字面量。

里程碑一是在性能锁下对最新十批次实现采样 CPU/分配热点，接受结果是能将主要耗时归因到精确通用函数，而不是根据墙钟推测。里程碑二是在该函数上实施最小优化，并用新回归测试证明 cache miss、cache hit、参数变化、schema/cache generation 失效和错误清理仍与 Go 一致。里程碑三是先 `cargo fmt --all`，再跑精确功能测试和五次 after，最后跑 `make lint`。

底层调用链从 `pkg/testkit/mockstore.rs::execute_concrete_session` 进入 `ConcreteSession::Execute`，`EXECUTE st USING` 由 `pkg/session/runtime/dispatch.rs::execute_prepared` 完成缓存失效检查、参数绑定和执行，计划型 SELECT 最终经 `pkg/session/runtime/planning.rs` 的物理计划执行和 RecordSet/Chunk 排空。“plan-cache hit”是指 SQL 计划已存在于实例级缓存，本次只允许消除命中时不随参数变化的重复工作。

恢复策略：所有 profiling 插桩和原型必须是局部可撤回的；若某变体未提速或改变语义，立即只撤回本任务的 hunk，不回滚共享工作区中他人改动。Cargo 和性能锁按本文后续规则领取，中断后只能清理已确认 PID 退出且 owner 为 task 38 的锁。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`
- Cargo 清单：`pkg/planner/core/casetest/instanceplancache/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/planner/core/casetest/instanceplancache/lib.rs`

## 上下文

- 原始排名/耗时/状态：`38` / `5.789s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-planner-core-casetest-instanceplancache`；target：`astersql_planner_core_casetest_instanceplancache`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`builtin_func_test::test_builtin_in_decimal_sig`；binary id：`astersql-planner-core-casetest-instanceplancache`。
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
2. 锁目录中记录任务 38、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `builtin_func_test::test_builtin_in_decimal_sig` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0038-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_decimal_sig)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0038-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_decimal_sig)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/planner/core/casetest/instanceplancache/Cargo.toml`、`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0038-before.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_decimal_sig)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0038-after.tsv -- --locked --package astersql-planner-core-casetest-instanceplancache --lib -E 'binary_id(=astersql-planner-core-casetest-instanceplancache) & test(=builtin_func_test::test_builtin_in_decimal_sig)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/planner/core/casetest/instanceplancache/builtin_func_test.rs`，精确符号 `builtin_func_test::test_builtin_in_decimal_sig`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 复核保持语义的安全优化边界；现有批量 fixture 已消除无关单行提交，核心 10×100 text-protocol 负载仍构成不可突破下界。
- [x] 保存精确测试数量、五次耗时与临时阶段剖析；本任务未保留代码修改，after 与 Ready lint 不适用。
- [x] 自审并移除全部 task 38 诊断插桩；按协议更新为已阻塞并保留任务文件。
- [x] 2026-10-13 解锁后重新领取槽位和性能锁，在最新共享工作区上重跑五次严格基线，不沿用旧锁状态。
- [x] 复核同构用例的请求对、十批、单批及 binary-protocol 下界证据；均无法在保留 Go 对齐语义下达标。
- [x] 扩展到 prepared SELECT 底层路径，验证 typed physical adapter 的 `IN`/decimal 可行性；不安全原型已精确撤回。

## 意外发现

- 初始事实：全量并发日志记录 `5.789s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- before 精确选中 1 项，5 次均功能通过、零失败、零超时，耗时为 `1.456746 / 1.372771 / 1.276444 / 1.315508 / 1.187009s`，最大 `1.456746s > 0.499s`，严格门槛失败。
- 持性能锁的单次阶段分解：Harness `0.125395s`，含 4 表×100 行批量写入的 fixture 累计到 `0.213409s`，完整测试 `1.891685s`；各 worker 的 100 次累计 prepare `0.125–0.205s`、bind `0.158–0.218s`、query `1.085–1.348s`，热点是真实 `EXECUTE ... USING` 查询主体。
- 同构的 task 21 已独立验证：合并 text-protocol 请求会产生 `SELECT request dispatches retained by a worker`，不等价；复用 parser 临时栈无性能收益。两种路线均已撤回，不在本任务重复扩张。
- 诊断第一次编译因共享文件的并行任务同时清理 `Duration/Instant` 导入而退出 101；补回临时导入后重试成功，最终已移除所有 task 38 插桩。
- 2026-10-13 最新共享代码已将每 worker 的 100 组 `PREPARE/SET/EXECUTE` 分为 10 个 owner-thread text-protocol 批次，仍保留全部 1,000 组语句、100 个结果集/worker 和逐项断言。
- 新五次严格基线为 `1.421406 / 1.231191 / 2.360974 / 1.248824 / 1.226501s`，5/5 功能通过、每次精确 1 test、零失败/零超时，最大 `2.360974s > 0.499s`。
- 同构 task 21 的最新安全变体数据：请求对 `1.030–1.422s`，10 批次 `1.076–1.405s`，单个 300 语句批次 `2.699–3.682s`；巨型批次因超长 SQL 解析更慢。
- 同构 task 24 的下界诊断在违规把每会话 100 次 prepare 降为 4 次后，保留 10×100 次真实 binary-protocol query 仍为 `0.775708s`；该诊断不能交付，但证明查询执行本身已超过门槛。
- 底层原型一仅让 typed adapter 接受字符串 Datum，四个同构用例功能通过，但 decimal 单次仍为 `1.219229s`，未形成有效提速，报告为 `target/rust-performance/task-0038-typed-string-prototype.tsv`。
- 底层原型二将 `IN (?, ?)` 与无索引表放行到 typed physical adapter：int 用例通过（约 `0.841s`），但 decimal/real/string 分别约 `0.942/0.905/0.945s` 且均返回空结果，与期望两行不符。这证明当前 typed range/table source 没有实现 Go 的字符串到 DECIMAL/REAL/VARCHAR 比较强制转换；原型已撤回。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不减少 10 worker×100 次 `PREPARE/SET/EXECUTE`，不合并 text-protocol 请求，不改用 protocol batch/mock，不放宽门槛。理由：这些都会削弱本测试的实例级计划缓存跨会话复用压力和 Go 对齐请求边界。日期/作者：2026-10-12 / Codex。
- 决策：在安全批量 fixture 已存在的前提下，五次最大值仍为 `1.456746s`，且剖析证明剩余耗时为受保护的真实查询主体；因此按协议标记已阻塞。日期/作者：2026-10-12 / Codex。
- 决策：2026-10-13 以新槽位、新性能锁和最新十批次共享实现重新判定，不沿用锁屏或锁等待作为阻塞。新最大值 `2.360974s`，且已测安全局部变体和更弱下界均不达标，故保持已阻塞。日期/作者：2026-10-13 / Codex。
- 决策：不交付仅放宽 typed adapter 语法的快路径。理由：它改变 decimal/real/string 结果，偏离 Go；要继续底层优化，须先在通用 planner/executor 实现完整参数类型推导和 Go 兼容强转，或使通用 prepared path 持久化 AST/plan 以避免每次绑定后重解析。这是跨 planner/executor 的独立子系统工作，不应在本慢测中以不完整快路径替代。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 已阻塞。before 使用槽位 9，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-9`；预热编译退出 0，性能命令退出 1（仅因最大耗时超过 0.499s），报告 `target/rust-performance/task-0038-before.tsv`，有效测试数为 1×5，零失败、零超时。
- 剖析重试使用槽位 3，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3`；`cargo fmt --all` 退出 0，预热编译退出 0，持性能锁的精确单次测试退出 0 并输出上述阶段数据。另一次诊断编译因并行编辑短暂缺导入退出 101，未运行测试。
- 本任务未保留 Rust 代码改动；目标文件中现有多值 INSERT 优化属于其他并行任务，本任务不提交、不回滚。因无可交付代码改动，after 和 `make lint` 未运行。
- 自审：未删减 case、断言、行数、4 种表/索引路径、10 worker 或每 worker 100 次循环；未改动 Cargo 依赖、license 或只读 `plan.md`；所有 task 38 临时诊断已撤回。
- 2026-10-13 重试首次使用槽位 9，预热编译退出 0，但在等待其他存活任务的性能锁时被用户中断；确认 PID 已退出后仅清理了属于 task 38 的遗留 slot-9 锁。
- 最终重试使用槽位 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；预热编译退出 0，严格命令退出 1（仅因超过 0.499s），报告 `target/rust-performance/task-0038-resume-before.tsv`，有效测试数 1×5、零失败/零超时。
- 本次恢复未保留任何 task 38 源码改动；新的 `QueryStatementBatch` 与十批次 `run_in_case` 属于共享工作区的其他任务，本任务不提交、不回滚。因仍无可交付代码改动，`cargo fmt --all`、after 和 `make lint` 在本次恢复中不适用。
- 扩展底层试验使用动态领取的槽位 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；预热构建退出 0，`cargo fmt --all` 退出 0。四个同构精确测试共退出 100：int 通过，decimal/real/string 因结果错误失败；该语义变更已全部撤回，`git diff --check` 退出 0。
- 仍无 task 38 可交付的生产代码，因此不运行 after/Ready lint，不创建提交，保留本任务文件为已阻塞证据。

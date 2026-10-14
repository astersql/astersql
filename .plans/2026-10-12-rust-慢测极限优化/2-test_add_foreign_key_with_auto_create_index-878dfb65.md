# 任务 2: 优化 `astersql-tests-realtikvtest-addindextest::add_index_test::test_add_foreign_key_with_auto_create_index`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 2 的测试从 11.956 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 2 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`tests/realtikvtest/addindextest/add_index_test.rs`
- Cargo 清单：`tests/realtikvtest/addindextest/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/realtikvtest/addindextest/main_test.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/realtikvtest/addindextest/lib.rs`

## 上下文

- 原始排名/耗时/状态：`2` / `11.956s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-tests-realtikvtest-addindextest`；target：`add_index_test`（`test`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`test_add_foreign_key_with_auto_create_index`；binary id：`astersql-tests-realtikvtest-addindextest::add_index_test`。
- 初步分型：`external-service`。保持真实 TiKV/PD/etcd、真实网络与持久化语义；优先消除重复 bootstrap、轮询 sleep、重复建库/建表和可安全复用的不可变夹具，禁止换成 mock。 DDL 优化前逐项核对 job 持久化、schema state、reorg/checkpoint、取消回滚与 schema version sync，不能绕过 owner/worker 流程。
- 原始数字来自全 workspace 并发运行，只用于入选与排序；修改依据必须来自本任务五次预热隔离测量和剖析。
- DDL 前置：先读 `docs/agents/ddl/README.md`，再按涉及路径选择 lifecycle/reorg/add-index/partition 文档，并以代码和测试验证文档假设。

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
2. 锁目录中记录任务 2、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `test_add_foreign_key_with_auto_create_index` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `tests/realtikvtest/addindextest/add_index_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0002-before.tsv -- --locked --package astersql-tests-realtikvtest-addindextest --test add_index_test -E 'binary_id(=astersql-tests-realtikvtest-addindextest::add_index_test) & test(=test_add_foreign_key_with_auto_create_index)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0002-after.tsv -- --locked --package astersql-tests-realtikvtest-addindextest --test add_index_test -E 'binary_id(=astersql-tests-realtikvtest-addindextest::add_index_test) & test(=test_add_foreign_key_with_auto_create_index)'`
- 模拟策略：使用真实服务；仅允许模拟与本行为无关且慢/不安全/非确定的外围边界，且数据形状必须与真实响应一致。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`tests/realtikvtest/addindextest/Cargo.toml`、`tests/realtikvtest/addindextest/add_index_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0002-before.tsv -- --locked --package astersql-tests-realtikvtest-addindextest --test add_index_test -E 'binary_id(=astersql-tests-realtikvtest-addindextest::add_index_test) & test(=test_add_foreign_key_with_auto_create_index)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0002-after.tsv -- --locked --package astersql-tests-realtikvtest-addindextest --test add_index_test -E 'binary_id(=astersql-tests-realtikvtest-addindextest::add_index_test) & test(=test_add_foreign_key_with_auto_create_index)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`tests/realtikvtest/addindextest/add_index_test.rs`，精确符号 `test_add_foreign_key_with_auto_create_index`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 实施并复测一项保持语义的 AutoID 批量分配候选优化；收益不足后已回退，不留无效生产复杂度。
- [x] 保存候选优化后的五次复测、测试数量与耗时证据。
- [x] 自审并更新为已阻塞；任务文件按协议保留，不删除。

## 意外发现

- 初始事实：全量并发日志记录 `11.956s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 隔离 before 五次均功能通过，耗时为 `10.059371 / 9.719171 / 9.357015 / 9.332288 / 9.486396s`，最大 `10.059371s`；该热点不是全量并发噪声。
- 阶段剖析的累计耗时：setup `0.725s`，首轮 14 次扩容至 131,072 行 `1.811s`，首次外键自动索引及删除周期 `3.889s`，失败外键检查 `3.891s`，第二次扩容与 131,072 行修复 `7.297s`，最终外键自动索引 `8.562s`，nextest 总计 `9.724s`。无固定 sleep 热点。
- 一次候选优化将多行 `INSERT ... SELECT` 的 AutoID 改为按语句批量预留，保留 increment/offset、显式 ID、重试 ID、临时表和 AUTO_RANDOM 语义；五次为 `9.495419 / 9.568222 / 9.528651 / 9.597879 / 9.394576s`，最大 `9.597879s`，证明 allocator 不是主瓶颈，已回退该实现。
- 保持 Go 对齐规模时，每次必须实际处理 262,144 行、执行两轮外键全表校验/自动索引回填、一次失败回滚检查以及完整 KV mutation 提交。现有通用 `HashMap<String, Option<String>>` 行表示和逐行 SQL/KV 语义下，`0.499s` 需要重新设计成批量/列式执行器或等价的存储批接口，已超出本单一测试的最小必要局部接线范围。
- 2026-10-13 基于共享工作区的新 testkit/session 改动重跑，五次为 `10.984826 / 10.292863 / 10.319423 / 10.414821 / 10.393785s`，最大 `10.984826s`；并行任务的通用改动没有解除本热点。
- 第二个候选优化在可证明新分配的聚簇 AUTO_INCREMENT 主键不存在时，跳过逐行已提交主键读取；五次为 `10.410310 / 10.442292 / 10.448172 / 10.722571 / 10.941133s`，最大 `10.941133s`。该读取同样不是主瓶颈，实现已回退。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不通过减少 14 次扩容、262,144 行、两次成功外键、一次失败外键、索引元数据或数据一致性断言来达标。理由：这些是 Go 对齐的规模与本测试的真实行为闭包。日期/作者：2026-10-12 / Codex。
- 决策：回退收益不足的 AutoID 批量候选优化，任务标记为 `已阻塞`。理由：改动后最大值仍为 `9.597879s`，距离 `0.499s` 还有约 19.2 倍；剩余瓶颈需跨越 session DML、KV 批处理和 DDL 回填的子系统级重设计，不能作为本 Go commit 对齐任务的局部依赖补齐。日期/作者：2026-10-12 / Codex。
- 决策：2026-10-13 继续任务时，复用共享工作区新实现重跑，并额外验证聚簇 AUTO_INCREMENT 主键存在性读取快路；两者均无收益。不保留没有性能证据的复杂化，也不以专用 mock、符号化行计数或改写 Go SQL 序列来规避真实行为。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 结果：`已阻塞`。before 与候选优化后复测均精确选中 `1` 个测试，每轮功能断言通过、失败/timeout 均为 `0`，但最大耗时分别为 `10.059371s` 和 `9.597879s`，未达 `<=0.499s`。
- 实际 Cargo 槽位：`10`；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-10`。
- 证据文件：`target/rust-performance/task-0002-before.tsv`、`target/rust-performance/task-0002-after1.tsv`。
- 继续执行证据：`target/rust-performance/task-0002-retry.tsv`、`target/rust-performance/task-0002-after2.tsv`；两份均为 `5` 个成功 sample、每轮精确 `1` 个测试、失败/timeout `0`，仅因性能超线而退出 `1`。
- 继续执行使用 Cargo 槽位 `2`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。
- 执行检查：任务中精确 before 命令（退出 `1`，仅因五次均超过性能门槛）；同参数候选 after 命令输出到 `task-0002-after1.tsv`（退出 `1`，同样仅因耗时超线）；`cargo fmt --all` 已在槽位 10 执行。
- RealTiKV 生命周期：before、剖析和候选 after 均在持有仓库性能锁时启动独立 `tikv-slim` playground；测试后终止进程并清理明确 tag 数据。before 清理后已验证 `127.0.0.1:2379` 不可达；其他阶段释放后锁已被其他任务立即接管，因此用明确 tag 目录不存在作为本任务清理证据。
- diff 自审：临时计时插桩和收益不足的生产优化均已删除；未删减 Go 对齐逻辑、case、断言、数据/并发规模或真实服务边界；未改动 Cargo 依赖。
- 未执行：`make lint`，因最终无可交付源代码变更且任务未达性能硬门槛；Ready profile 不能标记通过。

## RealTiKV 生命周期

- 本任务运行性能命令前必须按 `docs/agents/testing-flow.md` 启动 `tiup playground --mode tikv-slim --tag rust-perf-0002`，等待 `http://127.0.0.1:2379/pd/api/v1/version` 可用。
- playground、测试和使用其数据的进程必须在同一清理作用域内；正常、失败、中断均停止并等待 playground 进程，再用 `tiup clean rust-perf-0002` 清理该明确 tag 的数据。
- 清理后用 `! curl -sf http://127.0.0.1:2379/pd/api/v1/version` 证明端点不可达。不得与其他 RealTiKV 任务并行，不得借用未知的常驻集群作为最终证据。

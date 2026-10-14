# 任务 94: 优化 `astersql-tests-realtikvtest-testutils::parity_test::contract_resource_cleanup`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：对日志排名第 94、原始耗时 3.645 秒的测试做证据驱动优化；保持全部原始行为与 Go 对齐规模，取得明显性能提升或证明已无合理继续优化空间。

来源任务：`target/rust-test.4uY80L` 第 94 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能目标和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能评估属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`tests/realtikvtest/testutils/parity_test.rs`
- Cargo 清单：`tests/realtikvtest/testutils/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`tests/realtikvtest/testutils/lib.rs`

## 上下文

- 原始排名/耗时/状态：`94` / `3.645s` / `PASS`；原日志已通过且达到本轮入选门槛。
- Cargo package：`astersql-tests-realtikvtest-testutils`；target：`astersql_tests_realtikvtest_testutils`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`parity_test::contract_resource_cleanup`；binary id：`astersql-tests-realtikvtest-testutils`。
- 初步分型：`external-service`。保持真实 TiKV/PD/etcd、真实网络与持久化语义；优先消除重复 bootstrap、轮询 sleep、重复建库/建表和可安全复用的不可变夹具，禁止换成 mock。
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
2. 锁目录中记录任务 94、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `parity_test::contract_resource_cleanup` 的现有语义，在保持语义的前提下取得可量化的明显提升，或证明已无合理继续优化空间。
- 失败验证测试：现有 `tests/realtikvtest/testutils/parity_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0094-before.tsv -- --locked --package astersql-tests-realtikvtest-testutils --lib -E 'binary_id(=astersql-tests-realtikvtest-testutils) & test(=parity_test::contract_resource_cleanup)'`
- 基线判定：脚本必须精确选中 1 个测试并产生 5 个成功 sample；`--max-seconds 300` 只是防失控护栏。before 用于建立中位数、最大值和抖动范围，不再以 0.5 秒判定成败。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0094-after.tsv -- --locked --package astersql-tests-realtikvtest-testutils --lib -E 'binary_id(=astersql-tests-realtikvtest-testutils) & test(=parity_test::contract_resource_cleanup)'`
- 模拟策略：使用真实服务；仅允许模拟与本行为无关且慢/不安全/非确定的外围边界，且数据形状必须与真实响应一致。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；以同一真实测试的五次 before/after 对比作为性能证据。默认在 after 的中位数和最大值均比 before 改善至少 20% 时认定为明显提升；若未达到，必须用剖析、已尝试方案和剩余下界证明继续优化不合理。原有功能断言始终不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`tests/realtikvtest/testutils/Cargo.toml`、`tests/realtikvtest/testutils/parity_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。失败、超时或零测试必须修复；若中位数和最大值均改善至少 20%，可按明显提升完成。未达到 20% 时继续剖析，或记录主要热点已消除、不可消除下界及继续优化会损害语义或投入产出失衡的证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；正确性通过，且取得明显提升或有充分停止优化证据后标记 `已完成`，使用 `-commit` 只提交本任务变更并删除本任务文件。只有缺少完成所需外部条件且无法取得上述证据时才标记 `已阻塞`。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0094-before.tsv -- --locked --package astersql-tests-realtikvtest-testutils --lib -E 'binary_id(=astersql-tests-realtikvtest-testutils) & test(=parity_test::contract_resource_cleanup)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 300 --runs 5 --output target/rust-performance/task-0094-after.tsv -- --locked --package astersql-tests-realtikvtest-testutils --lib -E 'binary_id(=astersql-tests-realtikvtest-testutils) & test(=parity_test::contract_resource_cleanup)'`
- Ready lint：`make lint`
- 预期：before/after 均精确选择 1 项并各提供 5 个成功 sample；默认 after 中位数和最大值均改善至少 20%，或提供充分的停止优化证据；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、各五次耗时、中位数与最大值改善比例、测试数量 1×5、失败/超时数 0；若按无法继续优化完成，附热点分解、已尝试方案、剩余下界和停止理由；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`tests/realtikvtest/testutils/parity_test.rs`，精确符号 `parity_test::contract_resource_cleanup`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过，并满足以下之一：after 中位数和最大值均改善至少 20%；或证据证明主要安全热点已消除且继续优化不合理。不能用编译成功、零测试、放宽 timeout、单次快结果或主观判断替代。

## 2026-10-14 验收规则更新

- 本节规则覆盖下方历史进度、决策和结果中所有以 `0.499s`/`0.500s` 为硬门槛的旧结论；旧数字仅作为当时测量记录，不再构成阻塞理由。
- 既有 before/after 若已证明中位数和最大值均改善至少 20%，或正确性通过且剖析、尝试方案和下界证据已证明无法合理继续优化，则本任务视为通过；无需为了跨越 0.5 秒再次修改或复测。
- 只有正确性失败、零测试、证据不足、仍有明确安全热点，或外部环境/依赖不可用时，才保留未完成或阻塞状态。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和调用链热点分解。
- [ ] 完成保持语义的最小优化：被不可删减的 3 张表×64 行逐条 SQL 夹具与三路 DDL/清理行为下界阻塞。
- [ ] 保存 after：未修改代码，after/fmt/lint 不适用；已保存 5 次失败门槛数据。
- [x] 自审并更新为已阻塞；保留本任务文件供后续决策。

## 意外发现

- 初始事实：全量并发日志记录 `3.645s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 2026-10-13 隔离基线精确选中 1 个测试，5 次功能全部通过，耗时为 `3.970813s`/`3.668533s`/`3.732565s`/`3.959973s`/`4.199269s`，最大值 `4.199269s`，严格门槛退出码 1，证明不是全 workspace 并发噪声。
- RustCodeGraph 与源码定位的调用链为 `run_contract` -> `CreateMockStoreAndSetup` -> `InitTestWithStore` -> 3 张表×64 行逐条 SQL -> workload 取消/join -> 三路 add-index/check/drop -> TestKit 回池 -> GCS 对象清理与状态重置；数据规模、worker 数和清理断言均属任务要求保留的行为。
- 相同夹具/DDL 路径的任务 83 已记录 5 次 `3.952495s`–`9.860745s` 的下界，并验证并行/批量夹具尝试仍为 `3.089897s`–`3.864814s`；这与本任务的数据一致。
- 主机 `127.0.0.1:2379` 被无关 Docker 进程长期占用，无法按任务文本在默认端口启动独立 playground；按 `docs/agents/testing-flow.md` 允许的备选端口在 `12379` 启动 `rust-perf-0094` 取得上述诊断数据，停服并 `tiup clean rust-perf-0094 --all` 后已确认 `12379` 不可达；该数据不冒充任务要求的 2379 端口交付证据。
- 2026-10-13 本地 Mac 解锁后恢复执行，重新预编译退出 0，再次严格选中 `1×5` 且功能全部通过；耗时为 `3.445648s`/`4.854956s`/`5.354906s`/`5.033766s`/`1.858119s`，最大值 `5.354906s`，报告为 `target/rust-performance/task-0094-resume-before.tsv`。运行期间共享源码变化触发了重编译，但 TSV 只记录测试执行时间；最快的最后一次已包含相邻任务 93 删除 placeholder TestKit 的优化，仍为门槛的约 3.7 倍。
- 恢复后复核任务 166 的分段数据：同一标准夹具的 `NewTestKit` 约 `771ms`，3 张表×64 行 fixture 约 `714ms`，任一必须保留的独立阶段已超过整项 `0.499s` 预算。任务 93 删除 3 个随后立即覆盖的 placeholder TestKit 是已找到的唯一安全局部优化，但相邻路径 after 仍为 `1.327938s`–`1.747303s`。
- 锁屏不再是当前阻塞。恢复后的阻塞是可重复的物理下界：继续突破需要跨测试进程复用已初始化 session/schema/data、将 192 次独立 SQL/事务边界批量化，或减少真实 DDL/清理行为；都会改变本任务要求保留的隔离或 Go 对齐语义。

## 决策日志

- 决策：使用五次预热隔离 before/after 的中位数与最大值比较；默认二者均改善至少 20% 即为明显提升，无法继续优化时以剖析和下界证据完成，不设 0.5 秒绝对线。理由：原日志存在并发争用，且不同测试具有不可等同的真实协议与时序下界。日期/作者：2026-10-14 / 计划更新者。
- 决策：不实施批量化 192 条 insert、不减少 3 张表×64 行、不跳过三路 DDL/清理、不替换真实夹具边界。理由：这些是唯一可能显著突破当前下界的方式，但违反 Go 对齐规模和用户明确禁止的弱化策略。日期/作者：2026-10-13 / Codex。
- 决策：标记 `已阻塞` 并保留任务文件，不制造不等价源码 diff，不执行 `$git-commit`。解锁需要上层调整 `0.499s` 目标或明确授权重构逐条 SQL 夹具语义；当前指令不允许两者。日期/作者：2026-10-13 / Codex。
- 决策：解锁后重新打开原阻塞结论，重读最新共享实现并实测相邻 placeholder 优化的影响；因最快仍为 `1.858119s` 且两个必须保留阶段各自已超整个预算，维持 `已阻塞`。这一结论不依赖锁屏或本机权限，而是门槛与不可弱化语义冲突。日期/作者：2026-10-13 / Codex。

## 结果回顾

- 状态：已阻塞。功能正确性 5 次均通过，精确测试数为 `1×5`，失败/超时数为 0；严格性能门槛 5 次均失败，最大耗时 `4.199269s`。
- 验证资源：实际 Cargo 槽位 `10`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-10`；预编译退出码 0；五次诊断报告为 `target/rust-performance/task-0094-before-alt-port.tsv`，性能脚本因超限退出 1；自有 playground 在 12379 启动、停止、清理后端点不可达。
- 实际命令：`cargo nextest run --profile perf-audit --no-run --locked --package astersql-tests-realtikvtest-testutils --lib -E 'binary_id(=astersql-tests-realtikvtest-testutils) & test(=parity_test::contract_resource_cleanup)'`；`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0094-before-alt-port.tsv -- --locked --package astersql-tests-realtikvtest-testutils --lib -E 'binary_id(=astersql-tests-realtikvtest-testutils) & test(=parity_test::contract_resource_cleanup)'`。
- 改动范围：未修改 Rust 源码或测试；因此 `cargo fmt --all`、after 和 `make lint` 不适用。自审确认未删减 Go 对齐逻辑、未更改依赖、未覆盖共享工作区的其他任务改动。
- 剩余风险：目标测试仍远超慢测目标；默认 2379 端口生命周期未能在本地复现。在不改变夹具语义的前提下未找到可达到 `<=0.499s` 的实现路径。
- 恢复验证资源：Cargo 槽位 `5`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-5`；预编译退出 0；严格性能命令退出 1，`1×5` 功能全部通过，最大 `5.354906s`；`rust-perf-0094` 在 12379 正常启停并定向清理，清理后端点不可达。

## RealTiKV 生命周期

- 本任务运行性能命令前必须按 `docs/agents/testing-flow.md` 启动 `tiup playground --mode tikv-slim --tag rust-perf-0094`，等待 `http://127.0.0.1:2379/pd/api/v1/version` 可用。
- playground、测试和使用其数据的进程必须在同一清理作用域内；正常、失败、中断均停止并等待 playground 进程，再用 `tiup clean rust-perf-0094` 清理该明确 tag 的数据。
- 清理后用 `! curl -sf http://127.0.0.1:2379/pd/api/v1/version` 证明端点不可达。不得与其他 RealTiKV 任务并行，不得借用未知的常驻集群作为最终证据。

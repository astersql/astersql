# 任务 186: 优化 `astersql-objstore::gcs_test::test_speed_read_many_files`

调度：系统自动并行

显式依赖：无

状态：已阻塞

目的：把日志排名第 186 的测试从 2.685 秒优化到五次预热隔离运行均小于 0.500 秒，保持全部原始行为与 Go 对齐规模。

来源任务：`target/rust-test.4uY80L` 第 186 个 `>=0.5s` 唯一测试身份

预计会话范围：单一 binary/test 身份、单一性能门槛和一次聚焦提交；先复核噪声，再只修改剖析证明的热路径。原始测试若已失败或超时，正确性恢复与性能达标属于同一任务。

本任务是依照仓库 `PLANS.md` 执行的独立活文档；执行中维护下方进度、意外发现、决策和结果，但不得修改只读 `plan.md`。

## 文件

- 测试：`pkg/objstore/gcs_test.rs`
- Cargo 清单：`pkg/objstore/Cargo.toml`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/objstore/gcs.rs`
- 候选共享夹具/实现（仅剖析确认后修改）：`pkg/objstore/lib.rs`

## 上下文

- 原始排名/耗时/状态：`186` / `2.685s` / `PASS`；原日志已通过但超过性能门槛。
- Cargo package：`astersql-objstore`；target：`astersql_objstore`（`lib`）；features：原始 `make rust-test` 的默认 features，不启用额外 feature。
- 完整测试名：`gcs_test::test_speed_read_many_files`；binary id：`astersql-objstore`。
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
2. 锁目录中记录任务 186、会话标识和 PID；锁被占用时等待，不得删除无法确认已失效的锁。
3. 持锁期间不得并行运行其他测试、lint、格式化、playground 或高负载命令；完成、失败、中断均清理自己的记录并用 `rmdir` 释放锁。
4. `before` 与 `after` 使用相同主机、profile、features、精确过滤器和五次运行；记录五个 sample，不用单次最好值替代最大值。

## 测试计划

- 行为：完整执行 `gcs_test::test_speed_read_many_files` 的现有语义并把五次隔离运行最大耗时降至 `<=0.499s`。
- 失败验证测试：现有 `pkg/objstore/gcs_test.rs` 中的精确测试，不新增同文件生产逻辑；性能门槛本身构成回归断言。
- 失败验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0186-before.tsv -- --locked --package astersql-objstore --lib -E 'binary_id(=astersql-objstore) & test(=gcs_test::test_speed_read_many_files)'`
- 基线判定：脚本必须精确选中 1 个测试；非零应归因为耗时越线、失败或超时。若五次已全部通过且最大值 `<=0.499s`，则记录全量并发噪声证据并停止改代码，不制造无意义 diff。
- 通过验证命令：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0186-after.tsv -- --locked --package astersql-objstore --lib -E 'binary_id(=astersql-objstore) & test(=gcs_test::test_speed_read_many_files)'`
- 模拟策略：优先真实依赖；仅在剖析证明外部、慢、不安全或非确定边界与被测语义无关时使用等价 mock，并补充真实边界回归。

性能优化不新增重复的功能测试，因此传统“先新增失败测试”不适用；若隔离基线越线，以同一真实测试在严格性能门槛下先失败、优化后通过作为证据；若隔离基线已达标，则不发生行为变更，以五次基线作为并发噪声复核证据。原有功能断言在两种路径下都不得删减。

## 步骤

1. 阅读 `plan.md`、本任务文件、`pkg/objstore/Cargo.toml`、`pkg/objstore/gcs_test.rs`，以及存在时最近的 `doc.go`；用 RustCodeGraph 查看精确测试节点、callees、callers 和 impact，并核对相邻 Rust/Go 测试意图。
2. 按 Cargo 槽位规则领取槽位并设置绝对 `CARGO_TARGET_DIR`；按性能锁规则独占计时资源，运行 `before`，确认恰好 1 个有效测试并记录五次结果；若已达标，停止改代码并直接整理复核证据。
3. 临时分解 setup、fixture/load、主体和 teardown 耗时；用证据确认瓶颈，提交前删除临时日志/计时插桩。
4. 在不减少 case、断言、数据规模、并发量、planner 模式或真实服务边界的前提下，实施最小优化；测试逻辑与生产逻辑分文件，生产 Rust 修复达到可用后补 AsterSQL 版权行。
5. 若修改了 Rust 代码，先运行 `cargo fmt --all`；随后在相同条件运行 `after`。任一次 `>=0.500s`、失败、超时或零测试都继续优化或记录真实阻塞；无代码修改时可把 before 报告同时作为 after 证据。
6. 若有代码修改，运行 `make lint`；无代码修改时记录 Ready lint 不适用。审查 diff 没有无关变更、Go 对齐逻辑删减、PingCAP license 删除、本地依赖覆盖或测试弱化。
7. 更新本文件的进度、发现、决策、结果和状态；获得全部证据后标记 `已完成`，使用 `$git-commit` 只提交本任务变更并删除本任务文件。确实无法跨越物理/协议下界时标记 `已阻塞` 并保留证据。

## 验证

- 失败验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0186-before.tsv -- --locked --package astersql-objstore --lib -E 'binary_id(=astersql-objstore) & test(=gcs_test::test_speed_read_many_files)'`
- 格式化：`cargo fmt --all`
- 通过验证：`./tools/check/rust-test-performance.sh --max-seconds 0.499 --runs 5 --output target/rust-performance/task-0186-after.tsv -- --locked --package astersql-objstore --lib -E 'binary_id(=astersql-objstore) & test(=gcs_test::test_speed_read_many_files)'`
- Ready lint：`make lint`
- 预期：before 精确选择 1 项；若越线或失败则优化后由 after 提供 5 个全部通过且最大值 `<=0.499s` 的 sample，若 before 已达标则不改代码并复用其 5 个 sample；有代码修改时 fmt 与 lint 退出码 0。
- 所需证据：实际槽位和绝对 `CARGO_TARGET_DIR`、实际执行命令及退出码、before/after TSV 路径、五次耗时、测试数量 1×5、失败/超时数 0；有代码修改时还要提供 fmt、lint 与保留原始行为的 diff 自审摘要。

## 完成

- 主要测试位置：`pkg/objstore/gcs_test.rs`，精确符号 `gcs_test::test_speed_read_many_files`。
- 只修改 profiling 证明必要的测试夹具或生产热路径；若需要超出上述文件，先在“决策日志”写明调用链依据和冲突检查。
- 标记 `已完成` 的硬门槛是正确性通过与五次最大耗时 `<=0.499s`，不能用编译成功、零测试、放宽 timeout 或单次快结果替代。

## 进度

- [x] 阅读上下文并取得 Cargo 槽位与性能锁。
- [x] 保存 before 的预期失败证据和阶段耗时分解。
- [x] 尝试保持语义的最小优化；等价内存后端和 Go 对齐并发尺度均不达标，实验改动已撤销。
- [ ] 保存 after、测试数量与耗时证据；有代码修改时同时保存 fmt、lint 证据。
- [x] 自审、更新状态并保留已阻塞任务文件。

## 意外发现

- 初始事实：全量并发日志记录 `2.685s` / `PASS`；隔离基线尚未运行，不把该数字当作稳定性能结论。
- 隔离 before 五次均正确通过且每次精确选中 1 项，耗时为 `2.814015s` / `2.717281s` / `2.212525s` / `2.546456s` / `2.816098s`，脚本因最大值超过 `0.499s` 以退出码 1 失败。
- 热点不是 Domain/TestKit/runtime 初始化：测试会完整写入 30 个 100 MiB 对象并再完整读回，共要求至少 3 GiB 写入与 3 GiB 读取/返回拷贝，另有 1000 个小文件和 1110 次小文件读取。
- 两个未提交实验均保留原文件数、字节数、读写调用和断言：把本地文件系统换为 `object_store::memory::InMemory` 仍约 `1.135s`；把 4 个 worker 换为对齐 Go `errgroup.Go` 的每对象独立任务反而约 `2.306s`。所有实验源码已撤销，`pkg/objstore/gcs_test.rs` 无 diff。

## 决策日志

- 决策：以五次预热隔离最大值而非原日志单次值作为完成判据。理由：原日志并行 18,166 项，存在 CPU、I/O 和共享服务争用。日期/作者：2026-10-12 / 计划创建者。
- 决策：不提交内存后端或扩大 OS 线程数实验，也不使用丢弃写入、合成读取、稀疏数据或放宽门槛。理由：前两者仍无法达到 `0.499s`，后四者会违反 Go 对齐规模、完整读回语义或任务明确禁令。日期/作者：2026-10-13 / Codex。

## 结果回顾

- `Ready` 中的精确性能检查已运行，报告为 `target/rust-performance/task-0186-before.tsv`，5 个 sample、失败 0、超时 0、每次测试数 1，最大值 `2.816098s`；因超过 `0.499s` 门槛而不能生成 after 通过证据。
- 实际使用 Cargo 共享槽位 `5`，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-5`。预热编译和单次功能运行退出码 0；正式 before 命令退出码 1，唯一失败原因是五次耗时均超过上限。
- 没有交付 Rust 源码改动，因此 after、`make lint` 和交付提交不适用；`cargo fmt --all` 已在实验改动后两次成功运行，实验改动均已撤销。
- 阻塞原因：在不降低 30×100 MiB 规模、不删除完整 `WriteFile`/`ReadFile` 路径、不使用不等价 mock 的约束下，当前主机上连续内存后端实验也高于门槛 2.27 倍，无可证明的局部优化可将其降至 `<=0.499s`。

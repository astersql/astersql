# 任务 12: 优化 TestAsOfTimestampCompatibility

批次：【批次 12】 依赖：批次 11

状态：未开始

目的：保持 Go 行为与覆盖完整，将 `stale_read_test::TestAsOfTimestampCompatibility` 的独立预热后三次耗时全部优化到严格小于 1 秒。

来源任务：`target/rust-test.NgV3pq` 中 1.174 秒的 PASS 记录（按耗时降序第 12 项）。

预计会话范围：聚焦一个测试、其 Go 对照和该测试实际命中的最小生产路径；先取得剖析证据，再消除固定等待、重复初始化或热点开销，不扩建无关子系统。

## 文件

- 修改：`tests/realtikvtest/txntest/stale_read_test.rs`
- 对照（除非行为确需同步，否则不修改）：`tests/realtikvtest/txntest/stale_read_test.go`
- 配置：`tests/realtikvtest/txntest/Cargo.toml`
- 工具：`tools/check/rust-test-performance.sh`
- 生产代码：仅限剖析证明由本测试命中的既有 Rust 路径；修改后在任务记录中写明对应 Go 差异或不可缺少的局部接线依据。

## 上下文

- 来源基线：1.174 秒，状态 PASS；阈值包含 1.000 秒，验收要求严格小于 1 秒。
- 精确 nextest 身份：`astersql-tests-realtikvtest-txntest::stale_read_test TestAsOfTimestampCompatibility`。
- 该基线功能通过，但耗时已达到优化阈值。
- 测试依赖 RealTiKV/共享全局状态，必须独占执行，不得与本计划其他任务并行；不得删除断言、组合、错误路径或真实事务行为。

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

## 测试计划

- 行为：`TestAsOfTimestampCompatibility` 保持同名 Go 测试的场景、断言顺序、错误语义和清理语义，并在优化后稳定通过。
- 失败验证测试：复用 `tests/realtikvtest/txntest/stale_read_test.rs` 中现有 `TestAsOfTimestampCompatibility`；性能缺陷的失败证据为独立预热后三次审计任一次达到或超过 1 秒，功能缺陷则为精确测试失败。
- 失败验证命令：`tools/check/rust-test-performance.sh --max-seconds 0.999 --runs 3 -- --locked --package astersql-tests-realtikvtest-txntest -E 'binary_id(=astersql-tests-realtikvtest-txntest::stale_read_test) & test(=TestAsOfTimestampCompatibility)'`
- 预期失败原因：来源基线为 1.174 秒（PASS），达到性能阈值。
- 通过验证命令：`make rust-test-diagnose-one PACKAGE=astersql-tests-realtikvtest-txntest BINARY_ID='astersql-tests-realtikvtest-txntest::stale_read_test' TEST_NAME='TestAsOfTimestampCompatibility' && tools/check/rust-test-performance.sh --max-seconds 0.999 --runs 3 -- --locked --package astersql-tests-realtikvtest-txntest -E 'binary_id(=astersql-tests-realtikvtest-txntest::stale_read_test) & test(=TestAsOfTimestampCompatibility)'`
- 模拟策略：沿用现有真实测试夹具；只允许模拟 Go 原测试已经模拟的边界，不得把真实事务、锁、时间戳或副本读取路径替换成 mock。

## 步骤

1. 对照 Go 与 Rust 同名用例，列出必须保留的场景、组合、断言和清理。
2. 领取 Cargo 槽位，独占 RealTiKV 验证资源，运行失败验证并保存三次样本或功能失败输出。
3. 对该唯一测试分段计时或剖析，确定主要耗时来自固定等待、重复初始化、算法还是 I/O；记录证据。
4. 实施由证据支持的最小优化；若改生产代码，补充独立测试文件中的回归覆盖并注明 Go 对齐/局部接线依据。
5. 运行 `cargo fmt --all`，再运行精确行为测试和三次性能审计；检查有效测试数均为 1。
6. 使用 Ready profile 执行适用检查，自审差异；完成后更新状态并使用 `$git-commit` 仅提交本任务变更。

## 验证

- 运行：`cargo fmt --all`
- 运行：`make rust-test-diagnose-one PACKAGE=astersql-tests-realtikvtest-txntest BINARY_ID='astersql-tests-realtikvtest-txntest::stale_read_test' TEST_NAME='TestAsOfTimestampCompatibility'`
- 运行：`tools/check/rust-test-performance.sh --max-seconds 0.999 --runs 3 -- --locked --package astersql-tests-realtikvtest-txntest -E 'binary_id(=astersql-tests-realtikvtest-txntest::stale_read_test) & test(=TestAsOfTimestampCompatibility)'`
- 预期：格式化成功；精确行为测试 1 个通过；性能审计三次各 1 个测试通过且最大耗时小于 1 秒。
- 所需证据：原始失败基线、剖析结论、优化后三次具体耗时、命令退出码、每轮有效测试数量、Go/Rust 行为对照、自审差异、实际槽位和绝对 `CARGO_TARGET_DIR`；若修改代码，还须报告 Ready profile 的 `make lint` 及其他适用检查。

## 完成

只有 `TestAsOfTimestampCompatibility` 的行为通过、三次独立预热耗时均严格小于 1 秒、未删减 Go 逻辑、资源与槽位已清理且适用 Ready 检查通过，才能标记为 `已完成`。若仅因与本修改无关的环境问题无法回归，可标记为 `已完成，待回归` 并保留本文件；真实行为或性能仍不达标时必须继续处理或记录为 `已阻塞`。


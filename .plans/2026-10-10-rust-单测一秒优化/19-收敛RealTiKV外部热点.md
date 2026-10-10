# 任务 19: 收敛RealTiKV外部热点

批次：【批次 6】 依赖批次 2、3、4、5

状态：未开始

目的：串行优化 8 个 RealTiKV 慢测，区分集群物理下界与可移除的重复 setup/轮询。

来源任务：用户提供的 `target/rust-test.Aw4Dhb` 慢测日志。

预计会话范围：围绕同一性能根因和共享 fixture 工作；若采样证明存在独立生产热点，只处理目标测试必需的局部接线。

## 文件

- 修改：`tests/realtikvtest/addindextest/add_index_test.rs`
- 修改：`tests/realtikvtest/addindextest/main_test.rs`
- 修改：`tests/realtikvtest/addindextest2/alter_job_test.rs`
- 修改：`tests/realtikvtest/addindextest3/temp_index_test.rs`
- 修改：`tests/realtikvtest/sessiontest/paging_test.rs`
- 修改：`tests/realtikvtest/testutils/parity_test.rs`
- 修改：`tests/realtikvtest/txntest/stale_read_test.rs`

## 上下文

- 目标含 FK auto-index、full-mode CLI、alter finish race、temp-index merge、paging、public contract、stale-read 两项。共享外部集群，禁止并行；split_file/TestSelectAsOf 功能 FAIL 属范围外。

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

- 行为：真实 TiKV 的 DDL传播、paging keys、公共契约与 stale-read 组合完整。
- 失败验证测试：上述来源目标及其一秒性能门槛。
- 失败验证命令：按 `docs/agents/testing-flow.md` 启动唯一 tag 的 tikv-slim，再运行 `tools/check/rust-test-performance.sh --max-seconds 1 --runs 1 -- --package astersql-tests-realtikvtest-addindextest --package astersql-tests-realtikvtest-addindextest2 --package astersql-tests-realtikvtest-addindextest3 --package astersql-tests-realtikvtest-sessiontest --package astersql-tests-realtikvtest-testutils --package astersql-tests-realtikvtest-txntest -E 'test(/(test_add_foreign_key_with_auto_create_index|test_full_mode_command_line_entrypoint|test_alter_thread_right_after_job_finish|test_merge_temp_index_stuck|test_paging_act_rows_and_process_keys|go_rust_public_contract_matches|TestStaleRead(AllCombinations|Compatibility))$/)'`
- 预期失败原因：当前 5.015–14.802 秒，含真实 RPC/DDL。
- 通过验证命令：同一 playground 中 `--runs 3`；退出后确认 PD 不可达并清理唯一 tag 数据。
- 模拟策略：禁止 mock TiKV；可复用同一干净 store/Domain或事件通知。

## 步骤

1. 运行失败验证并保存退出码、有效测试数、三阶段耗时或采样。
2. 用 trap 管理 playground、端口、PID 和数据。
3. 逐例测集群 setup、Domain bootstrap、DDL/RPC、teardown。
4. 复用安全 fixture、批量准备数据、用事件替代轮询；记录一秒不可达的物理下界。
5. 运行 `cargo fmt --all`、通过验证、适用周边测试、`make lint` 与 diff 自审。

## 验证

- 运行：同一 playground 中 `--runs 3`；退出后确认 PD 不可达并清理唯一 tag 数据。
- 预期：8 项无 FAIL/TIMEOUT、无残留进程/数据；报告每项最小/最大时长和例外依据。
- 所需证据：修复前后每个目标的耗时、退出码、有效测试数、保留的规模/矩阵/断言，以及未达一秒项的可复现下界。

## 完成

如修改 Rust 源文件，确认顶部保留 PingCAP Apache License 并增加 `// Copyright 2026 AsterSQL.`；记录确切修改文件和符号、目标测试的三次耗时及 Ready 验证。获得证据后状态改为 `已完成` 并使用技能 `$git-commit` 独立提交；无关环境阻断才可标记 `已完成，待回归`，真实未解决热点不得误标完成。


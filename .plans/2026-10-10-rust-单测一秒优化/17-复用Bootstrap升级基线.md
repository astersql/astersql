# 任务 17: 复用Bootstrap升级基线

批次：【批次 5】 依赖任务 13、14

状态：未开始

目的：优化 bootstraptest 的 4 个来源慢测，保持旧版本值和 dist-task 全状态升级。

来源任务：用户提供的 `target/rust-test.Aw4Dhb` 慢测日志。

预计会话范围：一个聚焦会话可完成该测试族的基线、热点修复与定向验证。

## 文件

- 修改：`pkg/session/test/bootstraptest/boot_test.rs`
- 修改：`pkg/session/test/bootstraptest2/boot_test.rs`
- 修改：`pkg/session/bootstrap.rs` 及对应独立测试

## 上下文

- 目标 analyze version、cost model、index merge、178 dist-task state；应复用同一已初始化 store snapshot，而非跳过升级或状态。

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

- 行为：每个旧值/缺失值与所有 dist-task state 的升级结果与 Go 一致。
- 失败验证测试：上述来源目标及其一秒性能门槛。
- 失败验证命令：`tools/check/rust-test-performance.sh --max-seconds 1 --runs 1 -- --package astersql-session-test-bootstraptest --package astersql-session-test-bootstraptest2 -E 'test(/(analyze_version_upgrades_missing_and_legacy_values_to_two|cost_model_upgrade_defaults_old_missing_to_one_and_preserves_existing_value|index_merge_upgrade_preserves_existing_choice_and_defaults_old_missing_to_off|upgrade_from_178_is_nonfatal_for_every_dist_task_state)$/)'`
- 预期失败原因：当前 5.285–6.166 秒，主要为重复 bootstrap。
- 通过验证命令：同一命令改为 `--runs 3`。
- 模拟策略：真实 mock store/bootstrap SQL，不 mock升级器。

## 步骤

1. 运行失败验证并保存退出码、有效测试数、三阶段耗时或采样。
2. 测初始化、回退版本、升级与关闭。
3. 建立可克隆干净基线并验证 case 隔离。
4. 优化重复扫描/写入，保留全部旧值矩阵。
5. 运行 `cargo fmt --all`、通过验证、适用周边测试、`make lint` 与 diff 自审。

## 验证

- 运行：同一命令改为 `--runs 3`。
- 预期：4 项通过，矩阵不减，最大耗时尽可能 ≤1 秒。
- 所需证据：修复前后每个目标的耗时、退出码、有效测试数、保留的规模/矩阵/断言，以及未达一秒项的可复现下界。

## 完成

如修改 Rust 源文件，确认顶部保留 PingCAP Apache License 并增加 `// Copyright 2026 AsterSQL.`；记录确切修改文件和符号、目标测试的三次耗时及 Ready 验证。获得证据后状态改为 `已完成` 并使用技能 `$git-commit` 独立提交；无关环境阻断才可标记 `已完成，待回归`，真实未解决热点不得误标完成。


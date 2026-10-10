# 任务 11: 优化Planner随机与golden

批次：【批次 2】 依赖批次 1

状态：未开始

目的：收敛 indexmerge、join、partition、join reorder 与 window 的 8 个慢测。

来源任务：用户提供的 `target/rust-test.Aw4Dhb` 慢测日志。

预计会话范围：围绕同一性能根因和共享 fixture 工作；若采样证明存在独立生产热点，只处理目标测试必需的局部接线。

## 文件

- 修改：`pkg/planner/core/casetest/indexmerge/indexmerge_path_test.rs`
- 修改：`pkg/planner/core/casetest/join/join_test.rs`
- 修改：`pkg/planner/core/casetest/partition/partition_pruner_test.rs`
- 修改：`pkg/planner/core/casetest/rule/rule_join_reorder_test.rs`
- 修改：`pkg/planner/core/casetest/windows/window_push_down_test.rs`

## 上下文

- 保留随机核心的类型/索引组合、date/time pruning、Go fixture 和 standard/cascades 双 planner；优先一次 Domain、批量数据和一次解析多次规划。

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

- 行为：8 个目标保持 golden 与计划断言，三次热运行显著下降。
- 失败验证测试：上述来源目标及其一秒性能门槛。
- 失败验证命令：`tools/check/rust-test-performance.sh --max-seconds 1 --runs 1 -- --package astersql-planner-core-casetest-indexmerge --package astersql-planner-core-casetest-join --package astersql-planner-core-casetest-partition --package astersql-planner-core-casetest-rule --package astersql-planner-core-casetest-windows -E 'test(/(test_(multi_)?mv_index_random|test_join_simplify_condition_runs_go_plan_contract|test_list_columns_partition_pruner|test_range_(date|time)_pruning_extract|join_reorder_go_fixture_cases|window_go_golden_replay_matches_standard_and_cascades_planners)$/)'`
- 预期失败原因：当前 5.688–10.023 秒。
- 通过验证命令：同一命令改为 `--runs 3`。
- 模拟策略：真实 planner/TestKit/testdata，不 mock 计划。

## 步骤

1. 运行失败验证并保存退出码、有效测试数、三阶段耗时或采样。
2. 分离 Domain、fixture parse、数据写入与 optimize 时间。
3. 复用不变 fixture并批量装载，生产热点需新增等价回归。
4. 审查所有 golden 不发生无关漂移。
5. 运行 `cargo fmt --all`、通过验证、适用周边测试、`make lint` 与 diff 自审。

## 验证

- 运行：同一命令改为 `--runs 3`。
- 预期：8 个目标通过且 testdata 无意外变化；记录最大耗时。
- 所需证据：修复前后每个目标的耗时、退出码、有效测试数、保留的规模/矩阵/断言，以及未达一秒项的可复现下界。

## 完成

如修改 Rust 源文件，确认顶部保留 PingCAP Apache License 并增加 `// Copyright 2026 AsterSQL.`；记录确切修改文件和符号、目标测试的三次耗时及 Ready 验证。获得证据后状态改为 `已完成` 并使用技能 `$git-commit` 独立提交；无关环境阻断才可标记 `已完成，待回归`，真实未解决热点不得误标完成。


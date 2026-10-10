# 任务 15: 优化Session统计大数据

批次：【批次 5】 依赖任务 13、14

状态：未开始

目的：把 100010 行七分区 combined statistics 用例的剩余热点压到一秒级或给出真实算法下界。

来源任务：用户提供的 `target/rust-test.Aw4Dhb` 慢测日志。

预计会话范围：一个聚焦会话可完成该测试族的基线、热点修复与定向验证。

## 文件

- 修改：`pkg/session/runtime/statistics_test.rs`
- 修改：`pkg/statistics/runtime_stats_builder.rs`
- 修改：`pkg/session/runtime/statistics.rs`
- 修改：`pkg/session/runtime/row_codec.rs`
- 修改：`pkg/tablecodec/tablecodec.rs`

## 上下文

- 上一轮已从 >60 秒降至约9秒，本次并发全量仍 10.022 秒超时。禁止减少 100010 行、7 分区和统计结果断言。

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

- 行为：大批 INSERT、分区统计构建与 combined merge 结果不变。
- 失败验证测试：上述来源目标及其一秒性能门槛。
- 失败验证命令：`tools/check/rust-test-performance.sh --max-seconds 1 --runs 1 -- --package astersql-session -E 'test(=runtime_statistics_test::combined_merge_sql_100010_rows_seven_partitions)'`
- 预期失败原因：当前并发全量 10.022 秒，仍是生产热路径。
- 通过验证命令：同一命令改为 `--runs 3`。
- 模拟策略：真实 SQL/statistics，不 mock；允许一次生成批量输入。

## 步骤

1. 运行失败验证并保存退出码、有效测试数、三阶段耗时或采样。
2. 对 insert/collect/merge/persist 分段并采样。
3. 新增批量/增量路径与现有结果逐字段等价测试。
4. 优化确认热点并运行 statistics 周边回归。
5. 运行 `cargo fmt --all`、通过验证、适用周边测试、`make lint` 与 diff 自审。

## 验证

- 运行：同一命令改为 `--runs 3`。
- 预期：原规模通过；优先 ≤1 秒，否则给出每阶段耗时与复杂度证据。
- 所需证据：修复前后每个目标的耗时、退出码、有效测试数、保留的规模/矩阵/断言，以及未达一秒项的可复现下界。

## 完成

如修改 Rust 源文件，确认顶部保留 PingCAP Apache License 并增加 `// Copyright 2026 AsterSQL.`；记录确切修改文件和符号、目标测试的三次耗时及 Ready 验证。获得证据后状态改为 `已完成` 并使用技能 `$git-commit` 独立提交；无关环境阻断才可标记 `已完成，待回归`，真实未解决热点不得误标完成。


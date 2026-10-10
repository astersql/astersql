# 任务 7: 收敛 profile 与 traceevent 超时

批次：【批次 2】 依赖批次 1

状态：未开始

目的：消除 profile/traceevent suite 的固定冷却、全局状态串扰或大循环成本，保留真实采样和 ring-buffer 行为。

来源任务：`profile_test::test_profiles`、`traceevent_test::test_suite`。

预计会话范围：两个相邻 util crate，均涉及全局 profiler/recorder 生命周期；按任务 1 分类决定是否共享串行锁或分别优化。

## 文件

- 修改：`pkg/util/profile/profile.rs`
- 测试：`pkg/util/profile/profile_test.rs`
- 修改：`pkg/util/traceevent/traceevent.rs`
- 测试：`pkg/util/traceevent/traceevent_test.rs`

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

- 行为：profile 数据有效；trace mode、category、trace ID、ring buffer 和 cooling-off 断言完整。
- 失败验证命令：`cargo nextest run --locked --package astersql-util-profile --package astersql-util-traceevent -E 'test(=profile_test::test_profiles) | test(=traceevent_test::test_suite)' --no-capture`
- 预期失败原因：任务 1 确认的固定等待、全局状态冲突或未优化循环。
- 通过验证命令：同上，随后各 crate 全测试。
- 模拟策略：真实 profiler/recorder；只将时间推进抽象为现有可注入 clock 时才使用模拟。

## 步骤

1. 分段测量 setup、采样、cooldown、drop。
2. 添加确定性状态清理/时间边界回归。
3. 修复生产生命周期或测试同步，不移除真实数据断言。
4. 格式化、精确与 crate 回归、lint。

## 验证

- 预期：2 个目标测试默认预算内通过，crate 全测无全局状态污染。
- 所需证据：前后耗时、重复运行、状态恢复证明。

## 完成

记录两个 crate 的热点和清理保证，使用 `$git-commit` 提交。

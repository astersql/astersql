# 任务 8: 修复 RealTiKV 调度器 API 调用

批次：【批次 1】 无

状态：已完成，待回归

目的：将 importintotest4 记录夹具对旧调度器方法名的调用对齐到当前正式 API。

来源任务：无

预计会话范围：单一诊断、单一模块和一组聚焦验证；不扩展到相邻功能重构。

## 文件

- 修改：`tests/realtikvtest/importintotest4/recorded_summary_harness.rs`
- 测试：`tests/realtikvtest/importintotest4/global_sort_test.rs`

## 上下文

- `Manager` 的正式方法为 `clean_finished_tasks`，且 scheduler 自身已有多个行为测试；记录夹具仍调用不存在的 `cleanup_finished_tasks`。

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

- 行为：记录汇总夹具继续清理已完成任务，并保留原有错误传播。
- 失败验证测试：先运行下述精确目标并保存当前诊断。
- 失败验证命令：`cargo check -p astersql-tests-realtikvtest-importintotest4 --all-targets --locked`
- 预期失败原因：方法名不存在，触发 E0599。
- 通过验证命令：`cargo test -p astersql-tests-realtikvtest-importintotest4 --test global_sort_test --locked -- --nocapture && cargo check -p astersql-tests-realtikvtest-importintotest4 --all-targets --locked`
- 模拟策略：使用现有真实类型和测试夹具，不新增行为 mock。

## 步骤

1. 记录 E0599 并核对 scheduler 正式方法签名/返回值。
2. 只替换调用名称，不改变清理时机或忽略返回错误。
3. 运行 fmt、global_sort_test 和全 targets check。

## 验证

- 运行：`cargo test -p astersql-tests-realtikvtest-importintotest4 --test global_sort_test --locked -- --nocapture && cargo check -p astersql-tests-realtikvtest-importintotest4 --all-targets --locked`
- 预期：E0599 消失，global_sort_test 非零通过。
- 所需证据：失败与通过输出、非零有效测试数、确切退出码、实际槽位和绝对 CARGO_TARGET_DIR、已审查差异。

## 完成

不得在 scheduler 新增兼容别名掩盖调用漂移。完成后使用 `$git-commit` 仅提交本任务变更。

## 执行记录

- 生产修改依据：本任务对应记录夹具的旧 API 名称漂移；仅将 `cleanup_finished_tasks` 替换为 scheduler 正式方法 `clean_finished_tasks`，保留调用时机和 `unwrap` 错误传播。
- 失败证据：`cargo check -p astersql-tests-realtikvtest-importintotest4 --all-targets --locked` 退出码 101，在 `recorded_summary_harness.rs:271` 报 E0599，并建议使用 `clean_finished_tasks`。
- 格式化：`cargo fmt --all` 退出码 0。
- 编译证据：修复后同一 `cargo check` 退出码 0。
- 聚焦行为证据：`cargo test -p astersql-tests-realtikvtest-importintotest4 --test global_sort_test --locked real_manager_failure_persists_after_explicit_node_initialization -- --nocapture` 退出码 0，1 个测试通过。
- 待回归原因：完整 `global_sort_test` 实际执行 13 个测试，12 通过、1 失败；失败为无关的 `test_global_sort_recorded_step_summary` 汇总断言（实际 `(16, 12)`，期望 `(12, 12)`），本任务覆盖的 manager 清理测试已通过。
- Cargo 共享槽位：槽位 1，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。

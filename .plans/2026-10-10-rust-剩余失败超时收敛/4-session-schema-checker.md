# 任务 4: 修复 session schema checker

批次：【批次 2】 依赖批次 1

状态：未开始

目的：修复 schema checker normal/partition 两条 SQL 路径与 Go 不一致的问题。

来源任务：`schema_checker_sql_matches_go_normal_and_partition_paths`。

预计会话范围：单 crate、单一 SQL 生成/参数契约，对照同目录 Go 路径修复。

## 文件

- 修改：`pkg/session/` 中 schema checker 拥有文件
- 测试：`pkg/session/test/session_test.rs`

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

- 行为：normal/partition schema checker SQL 与 Go 完全一致。
- 失败验证命令：`cargo nextest run --locked --package astersql-session-test -E 'test(=session_test::schema_checker_sql_matches_go_normal_and_partition_paths)' --no-capture`
- 预期失败原因：schema SQL 文本、参数或分区路径与 Go 不一致。
- 通过验证命令：同上。
- 模拟策略：使用现有 SQL runtime，不 mock SQL 生成。

## 步骤

1. 为 schema 差异保留精确字符串/参数断言；为性能热点记录采样。
2. 对照 Go 修正 normal 和 partition schema 路径。
3. 格式化并运行精确测试及相关 crate 窄回归。

## 验证

- 预期：1 个有效测试通过。
- 所需证据：Go 差异依据、失败前后断言和退出码。

## 完成

记录 schema checker 修改与证据，使用 `$git-commit` 提交。

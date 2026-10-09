# 任务 6: 修复 session 升级定义与回填

批次：【批次 3】 依赖批次 1

状态：已完成，待回归

目的：统一当前 bootstrap 版本、升级函数顺序和回填结果，修复版本 177/176 漂移。

来源任务：`upgrade_backfills_and_preserves_values`、`upgrade_functions_match_go_order_and_current_version`。

预计会话范围：仅处理 session bootstrap/upgrade 定义与同目录测试；不扩展到其它 session 运行时失败。

## 文件

- 修改：`pkg/session/` 中 upgrade definition/backfill 生产文件（按 RustCodeGraph 定位）
- 测试：`pkg/session/upgrade_backfill_test.rs`
- 测试：`pkg/session/upgrade_def_test.rs`

## 上下文

- 当前测试观察到升级函数数/版本为 177，而期望 176，且 current backfill 非空；必须核对 Go 当前版本与 Rust 注册顺序，不能简单修改常量或删除升级步骤。

## Cargo 共享槽位规则

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

- 行为：Rust 的 upgrade 列表顺序、current version 和新旧值回填与 Go 当前实现一致且保留已有用户值。
- 失败验证测试：上述两个现有测试，增加缺失值/已有值边界（若现有未覆盖）。
- 失败验证命令：`cargo nextest run --locked -p astersql-session -E 'test(upgrade_backfills_and_preserves_values) | test(upgrade_functions_match_go_order_and_current_version)'`
- 预期失败原因：177/176 不一致和 current backfill 非空。
- 通过验证命令：同失败命令，并运行 `cargo nextest run --locked -p astersql-session -E 'test(upgrade)'`。
- 模拟策略：使用真实内存 store/bootstrap SQL，不 mock upgrade registry。

## 步骤

1. 用 RustCodeGraph 定位 current version、upgrade registry 和 Go 对应定义。
2. 固化失败边界并确认失败。
3. 修复注册顺序、版本或回填逻辑的真实漂移，保留全部 Go 行为。
4. 格式化并运行聚焦升级测试。

## 验证

- 运行：`cargo fmt --all`
- 运行：上述两个通过验证命令。
- 预期：版本、顺序、缺失值和保留值断言通过。
- 所需证据：Go 差异依据、修复前后输出、退出码与有效测试数。

## 完成

报告版本映射和回填行为；完成后使用 `$git-commit` 提交。

## 执行记录

- Go 与 Rust 的 upgrade registry 均为 177 项，版本序列完全一致，current bootstrap version 均为 317。Go `upgradeToVer317` 会在缺失时回填 `tidb_enable_adaptive_limit_scan=OFF`，不覆盖已有值。
- 修复了测试中过期的 176 项和 current=287/283 假设；修复后目标两项 nextest 通过。
- 共享槽位：3；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3`。
- 修复前：`cargo nextest run --locked -p astersql-session -E 'test(upgrade_backfills_and_preserves_values) | test(upgrade_functions_match_go_order_and_current_version)'`，退出码 100，2 项均失败（`177 != 176`、current backfill 非空）。
- 修复后同一命令：退出码 0，2/2 通过。
- `cargo fmt --all`：退出码 0。
- `cargo nextest run --locked -p astersql-session -E 'test(upgrade)'`：退出码 100，27 项中 25 项通过、2 项被默认 10 秒预算终止；其中 masking-policy 生命周期属任务 7 范围，`sql_defaults_and_bootstrap_upgrade` 为既有慢测。
- `cargo test --locked -p astersql-session dml_runtime_test::sql_defaults_and_bootstrap_upgrade -- --exact`：退出码 0，1/1 通过，用时 13.42 秒，证明行为正确而 nextest 失败来自无关本次修改的统一 10 秒超时。
- `make lint`：退出码 0。
- 待回归：相关慢测超时修复后，重跑完整 `test(upgrade)` 过滤集。

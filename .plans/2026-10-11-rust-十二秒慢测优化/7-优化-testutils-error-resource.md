# 任务 7: 优化 testutils error 与 resource cleanup

批次：【批次 7】 依赖批次 6

状态：未开始

目的：减少 error/resource cleanup 契约的重复 fixture 开销，同时证明错误注入、worker 取消和池归还不串扰。

来源任务：任务 5 分离出的 error 与 resource cleanup 耗时。

预计会话范围：只处理两个 contract 与直接生命周期 helper，不改 normal/boundary。

## 文件

- 修改：`tests/realtikvtest/testutils/parity_test.rs`
- 可能修改：`tests/realtikvtest/testutils/common.rs`、`tests/realtikvtest/testutils/compatibility.rs`
- 测试：`tests/realtikvtest/testutils/parity_test.rs`

## 上下文

- error 路径安装 exec handler 并用 catch_unwind 验证 panic；resource 路径启动/停止 workload、归还 kit pool 并清理 GCS。
- 任何复用都必须先清除 handler，等待 worker 完全结束，并证明池与对象列表恢复。

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

- 行为：unique/PK 错误匹配、non-unique panic、对象清理、worker cancel、pool 归还和 flags reset 全部保留。
- 失败验证测试：错误 handler 后创建正常 context 并执行成功；Stop 后断言 worker 结束、kit 数量恢复且对象为空。
- 失败验证命令：`cargo nextest run --locked --package astersql-tests-realtikvtest-testutils -E 'test(=parity_test::error_and_resource_fixture_is_fully_reset)'`
- 预期失败原因：完整 reset/生命周期断言尚不存在。
- 通过验证命令：同上，加 error/resource 两项和总目标三轮审计。
- 模拟策略：沿用现有明确错误注入边界；不 mock worker 生命周期。

## 步骤

1. 分别测量 handler、panic、worker stop 和 GCS cleanup。
2. 添加失败的完整 reset 测试并实现最小生命周期清理/复用。
3. 格式化，运行四类契约与总目标三轮审计；若总目标仍 >12 秒，继续限于 testutils fixture 路径优化。

## 完成

四类契约、reset 回归和总目标 3/3 样本通过且总目标最大值 ≤12 秒后标记 `已完成`，使用 `$git-commit` 提交并删除任务文件。

# 任务 16: 保留-CTE-溢出错误

批次：【批次 2】依赖：批次 1

状态：未开始

目的：整数溢出先于 spill 注入错误返回。

来源任务：`target/rust-test.Uo3yeR` 对应失败。

预计会话范围：一个根因和一个聚焦验证故事；若复现证明存在多个独立根因，停止扩写并新增后续小任务。

## 文件

- 测试：`pkg/executor/test/cte/cte_test.rs`
- 修改：`pkg/executor/cte.rs`

## 上下文

- 失败测试：`cte_test::test_cte_exec_error_reports_integer_overflow`。
- 先用 RustCodeGraph 查看测试调用的生产符号、调用方和影响测试；DDL 行为遵守 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`。
- 不删测试、不忽略失败、不用桩替代 Go 已有逻辑。

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

- 行为：整数溢出先于 spill 注入错误返回。
- 失败验证测试：`cte_test::test_cte_exec_error_reports_integer_overflow`
- 失败验证命令：`make rust-test PACKAGE=astersql-executor-test-cte RUST_TEST_TARGETS='--lib' RUST_TEST_ARGS='cte_test::test_cte_exec_error_reports_integer_overflow -- --exact --nocapture'`
- 预期失败原因：重现原日志中的断言、错误或超时；不能复现时先确认并发、隔离和服务差异。
- 通过验证命令：`cargo fmt --all && make rust-test PACKAGE=astersql-executor-test-cte RUST_TEST_TARGETS='--lib' RUST_TEST_ARGS='cte_test::test_cte_exec_error_reports_integer_overflow -- --exact --nocapture'`
- 模拟策略：优先真实组件；只复用仓库现有 fake/mock 隔离外部慢速或非确定性边界。

## 步骤

1. 阅读目标 crate 的 `Cargo.toml`、测试和生产调用链，记录对应 Go 差异或必要局部接线依据。
2. 领取槽位，运行失败验证并保存证据。
3. 修改最小生产路径，同步修复独立测试文件。
4. 运行 `cargo fmt --all`、通过验证和 RustCodeGraph 选出的直接受影响测试。
5. 自审 diff、版权头、风险及未验证项。

## 验证

- 运行：`cargo fmt --all && make rust-test PACKAGE=astersql-executor-test-cte RUST_TEST_TARGETS='--lib' RUST_TEST_ARGS='cte_test::test_cte_exec_error_reports_integer_overflow -- --exact --nocapture'`
- 预期：退出码 0，目标测试实际执行且通过，不能是 0 tests。
- 所需证据：修复前失败、修复后退出码与测试数量、槽位、绝对 `CARGO_TARGET_DIR`、受影响测试结果。

## 完成

有当前证据后标记 `已完成`，使用技能 `$git-commit` 仅提交本任务变更并删除任务文件；仅因无关环境不能回归时标记 `已完成，待回归` 并保留文件。


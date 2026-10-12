# 任务 5: 迁移 RealTiKV 事务与会话 failpoint

批次：【批次 3】 依赖批次 2

状态：未开始

目的：迁移 RealTiKV 事务、stale read、shared lock 和 session failure 测试的 failpoint 消费者。

来源任务：全仓类似位置分析。

预计会话范围：限定事务与会话测试 crate；生产注入点由任务 1、2、3 完成后再验证。

## 文件

- 测试：`tests/realtikvtest/txntest/stale_read_test.rs`
- 测试：`tests/realtikvtest/txntest/shared_lock_test.rs`
- 测试：`tests/realtikvtest/txntest/txn_state_test.rs`
- 测试：`tests/realtikvtest/txntest/txn_test.rs`
- 测试：`tests/realtikvtest/sessiontest/session_fail_test.rs`
- 测试：`tests/realtikvtest/testutils/common.rs`
- 测试：`tests/realtikvtest/testutils/parity_test.rs`

## 上下文

- 不重复任务 1 已完成的 stale-read key；只迁移剩余活跃 key。RealTiKV 验证必须启动 playground、等待健康、运行聚焦测试并清理 playground 和数据。

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
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能代替行为验收证据。

## 测试计划

- 行为：真实 TiKV 环境下 AsterSQL key 仍能驱动事务/session 故障分支。
- 失败验证测试：生产者已由前置任务迁移，因此先保留一个旧测试 key 并证明它不能触发预期故障，再切换 AsterSQL key。
- 失败验证命令：按 `docs/agents/testing-flow.md` 启动 playground 后运行 `cargo test -p astersql-tests-realtikvtest-txntest --test stale_read_test TestSelectAsOf -- --exact`
- 预期失败原因：旧 PingCAP 消费者 key 与已迁移生产者不匹配。
- 通过验证命令：相同命令，测试 key 改为 AsterSQL 后通过。
- 模拟策略：使用真实 TiKV playground，不用 mock 替代存储行为。

## 步骤

1. 核对前置任务的生产 key 与本任务消费者。
2. 启动和健康检查 playground，获取 fail-before 证据。
3. 迁移剩余事务/session key，运行同一测试获得 pass-after。
4. 运行受影响测试二进制的最小补充过滤器。
5. 无论成功、失败或中断都清理 playground、数据和 Cargo 槽位锁。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-tests-realtikvtest-txntest --test stale_read_test TestSelectAsOf -- --exact`
- 运行：`cargo test -p astersql-tests-realtikvtest-sessiontest --test session_fail_test test_get_ts_fail_dirty_state -- --exact`
- 预期：真实 TiKV 用例通过，清理后 PD 端点不可访问。
- 所需证据：playground 健康与清理记录、失败/通过输出、有效测试数、退出码和槽位信息。

## 完成

事务和会话 RealTiKV 消费者使用 AsterSQL key，并由真实 TiKV 聚焦测试证明。


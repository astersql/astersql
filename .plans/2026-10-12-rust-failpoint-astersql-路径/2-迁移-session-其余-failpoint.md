# 任务 2: 迁移 session 其余 failpoint

批次：【批次 2】 依赖批次 1

状态：未开始

目的：迁移 session 运行时及相邻 Rust session 测试中的其余活跃 failpoint key，并保持每个生产者与消费者成对一致。

来源任务：全仓类似位置分析。

预计会话范围：限定在 `pkg/session/**`，排除任务 1 的 stale-read 三个 key；同一模块文件集中，适合一个聚焦会话。

## 文件

- 修改：`pkg/session/runtime.rs`
- 修改：`pkg/session/runtime/ddl.rs`
- 修改：`pkg/session/runtime/dispatch.rs`
- 修改：`pkg/session/runtime/dml.rs`
- 修改：`pkg/session/runtime/modify_column_cloud_planner.rs`
- 修改：`pkg/session/runtime/modify_column_dist_backfill.rs`
- 修改：`pkg/session/runtime/modify_column_pipeline.rs`
- 修改：`pkg/session/runtime/planning.rs`
- 修改：`pkg/session/runtime/recovery.rs`
- 修改：`pkg/session/runtime/source.rs`
- 修改：`pkg/session/runtime/statistics.rs`
- 修改：`pkg/session/runtime/system_query.rs`
- 修改：`pkg/session/runtime/system_session.rs`
- 测试：`pkg/session/runtime/normal_ddl_index_reorg_initialization_test.rs`
- 测试：`pkg/session/runtime/normal_ddl_masking_policy_test.rs`
- 测试：`pkg/session/runtime_test/ddl.rs`
- 测试：`pkg/session/test/txn/txn_test.rs`
- 测试：`pkg/session/test/variable/variable_test.rs`

## 上下文

- 只改传入 Rust failpoint API 的活跃 key；保留注释中的 Go 包路径和 Issue 链接。先用 RustCodeGraph 与局部读取配对生产注入点和测试注册点，不能盲目全局替换。

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

- 行为：AsterSQL session failpoint 能触发与迁移前相同的 DDL、DML、规划和事务故障路径。
- 失败验证测试：选择至少一个已有生产者/消费者配对测试，先只迁移测试 key。
- 失败验证命令：`cargo test -p astersql-session masking_policy_operations_require_system_table_with_failpoint -- --exact`
- 预期失败原因：测试注册 AsterSQL key，而生产注入点仍读取 PingCAP key。
- 通过验证命令：`cargo test -p astersql-session masking_policy_operations_require_system_table_with_failpoint -- --exact`
- 模拟策略：使用真实 failpoint registry 和现有 session fixture，不新增 mock。

## 步骤

1. 列出本任务文件中的活跃 key，并建立生产者/消费者配对清单。
2. 选一个可本地运行的现有测试完成 fail-before 证据。
3. 成对迁移 key，排除任务 1 和非运行时文本。
4. 格式化并运行所选测试及受影响的相邻测试过滤器。
5. 用限定于 `pkg/session/**` 的审计确认没有遗漏活跃旧 key。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-session masking_policy_operations_require_system_table_with_failpoint -- --exact`
- 运行：`rg -n 'github\.com/pingcap/tidb' pkg/session --glob '*.rs'` 并逐项分类剩余命中。
- 预期：迁移测试通过，剩余命中均有明确的非运行时保留理由。
- 所需证据：失败/通过输出、有效测试数、退出码、key 配对清单、槽位信息和剩余命中分类。

## 完成

`pkg/session/**` 中任务 1 之外的活跃 Rust failpoint key 使用 AsterSQL 前缀，相关生产者/消费者同步且聚焦行为测试通过。


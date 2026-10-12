# 任务 7: 迁移 RealTiKV BR 与 Import Into failpoint

批次：【批次 2】 依赖批次 1

状态：未开始

目的：迁移 BRIE、Import Into 及其 harness/job runtime 中的活跃 failpoint key。

来源任务：全仓类似位置分析。

预计会话范围：限定备份恢复和导入测试族；与任务 6 文件不重叠，可并行使用不同 Cargo 槽位，但 playground 端口必须隔离或串行。

## 文件

- 测试：`tests/realtikvtest/brietest/brie_test.rs`
- 修改：`tests/realtikvtest/brietest/harness.rs`
- 测试：`tests/realtikvtest/brietest/operator_test.rs`
- 测试：`tests/realtikvtest/brietest/pitr_test.rs`
- 测试：`tests/realtikvtest/brietest/registry_test.rs`
- 测试：`tests/realtikvtest/brietest/scheduler_test.rs`
- 修改：`tests/realtikvtest/importintotest/harness.rs`
- 测试：`tests/realtikvtest/importintotest/import_into_test.rs`
- 修改：`tests/realtikvtest/importintotest/job_runtime.rs`
- 测试：`tests/realtikvtest/importintotest/job_test.rs`
- 测试：`tests/realtikvtest/importintotest/parquet_test.rs`
- 测试：`tests/realtikvtest/importintotest/sdk_test.rs`
- 测试：`tests/realtikvtest/importintotest2/from_select_test.rs`
- 测试：`tests/realtikvtest/importintotest2/subtask_count_test.rs`
- 测试：`tests/realtikvtest/importintotest3/cleanup_on_error_test.rs`
- 测试：`tests/realtikvtest/importintotest3/cross_ks_test.rs`
- 测试：`tests/realtikvtest/importintotest3/extra_param_test.rs`
- 修改：`tests/realtikvtest/importintotest3/harness.rs`

## 上下文

- harness 中构造的 key 也属于运行时消费者。并行验证不得共享默认 playground 端口或同一数据目录；无法隔离时按顺序运行。

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

- 行为：AsterSQL key 在真实 TiKV 下继续触发 BR 与导入失败、暂停和清理路径。
- 失败验证测试：选择一个已有 BR 或 import 失败恢复用例，先保留旧消费者 key。
- 失败验证命令：启动 playground 后运行 `cargo test -p astersql-tests-realtikvtest-importintotest --test job_test test_job_fail_when_dispatch_subtask -- --exact`
- 预期失败原因：旧消费者 key 与已迁移生产点不匹配。
- 通过验证命令：同一用例迁移为 AsterSQL key 后运行。
- 模拟策略：使用真实 TiKV 和现有测试资源；不新增外部依赖副本。

## 步骤

1. 配对 BR/import 生产注入点和所有 harness/测试消费者。
2. 获取一个代表性用例 fail-before 证据。
3. 迁移所有已确认活跃 key，并保留 Go 来源说明。
4. 运行 BR 和 import 各至少一个受影响精确用例。
5. 清理 playground/data/槽位并审计剩余文本。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-tests-realtikvtest-brietest --test operator_test test_failure -- --exact`
- 运行：`cargo test -p astersql-tests-realtikvtest-importintotest --test job_test test_job_fail_when_dispatch_subtask -- --exact`
- 预期：BR 和 import 的代表性故障路径由新 key 触发，环境清理完整。
- 所需证据：fail-before/pass-after、有效测试数、退出码、playground 清理、槽位信息和剩余命中分类。

## 完成

RealTiKV BR/import 测试族的活跃 Rust failpoint key 使用 AsterSQL 前缀并保持行为。


# 任务 6: 迁移 RealTiKV DDL failpoint

批次：【批次 2】 依赖批次 1

状态：未开始

目的：迁移 add-index、DDL、flashback 测试中的活跃 failpoint 消费者，并与已迁移生产注入点匹配。

来源任务：全仓类似位置分析。

预计会话范围：仅 RealTiKV DDL 测试族；文件较多但语义一致，验证围绕代表性故障路径。

## 文件

- 测试：`tests/realtikvtest/addindextest/add_index_test.rs`
- 测试：`tests/realtikvtest/addindextest/concurrent_ddl_test.rs`
- 测试：`tests/realtikvtest/addindextest/failpoints_test.rs`
- 测试：`tests/realtikvtest/addindextest/multi_schema_change_test.rs`
- 测试：`tests/realtikvtest/addindextest/pitr_test.rs`
- 测试：`tests/realtikvtest/addindextest1/disttask_test.rs`
- 测试：`tests/realtikvtest/addindextest2/alter_job_test.rs`
- 测试：`tests/realtikvtest/addindextest2/global_sort_test.rs`
- 修改：`tests/realtikvtest/addindextest2/harness.rs`
- 测试：`tests/realtikvtest/addindextest3/functional_test.rs`
- 测试：`tests/realtikvtest/addindextest3/ingest_test.rs`
- 测试：`tests/realtikvtest/addindextest3/temp_index_test.rs`
- 测试：`tests/realtikvtest/addindextest4/failure_test.rs`
- 测试：`tests/realtikvtest/addindextest4/integration_test.rs`
- 测试：`tests/realtikvtest/ddltest/ddl_test.rs`
- 测试：`tests/realtikvtest/flashbacktest/flashback_test.rs`
- 修改：`tests/realtikvtest/flashbacktest/harness.rs`

## 上下文

- 执行前阅读 `docs/agents/ddl/README.md`。迁移测试消费者时不得顺带补建未实现 DDL 子系统；生产 key 缺失应作为明确阻塞或范围外发现记录。

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

- 行为：AsterSQL DDL failpoint 在真实 TiKV 下保持既有暂停、失败和恢复行为。
- 失败验证测试：选一个执行时间可控的 add-index 用例，先保留旧消费者 key。
- 失败验证命令：启动 playground 后运行 `cargo test -p astersql-tests-realtikvtest-addindextest2 --test global_sort_test test_global_sort_add_index_recover_from_retryable_error -- --exact`
- 预期失败原因：旧 key 无法触发已迁移生产点。
- 通过验证命令：同一用例改为 AsterSQL key 后运行。
- 模拟策略：真实 TiKV；不以编译成功代替 failpoint 行为验证。

## 步骤

1. 阅读 DDL 指南并按生产者配对所有待迁移 key。
2. 获取一个代表性用例的 fail-before 证据。
3. 迁移列出的测试和 harness 中活跃 key。
4. 运行代表性 add-index、DDL 或 flashback 聚焦用例。
5. 清理 playground/data/槽位并分类剩余 PingCAP 文本。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-tests-realtikvtest-addindextest2 --test global_sort_test test_global_sort_add_index_recover_from_retryable_error -- --exact`
- 运行：按实际改动再选一个 `ddltest` 或 `flashbacktest` 精确用例。
- 预期：至少一个真实 failpoint 故障路径被 AsterSQL key 触发并通过，环境完整清理。
- 所需证据：指南已读、fail-before/pass-after、有效测试数、退出码、playground 清理和槽位信息。

## 完成

RealTiKV DDL 测试族的活跃 failpoint 消费者使用 AsterSQL 路径，并保留真实故障行为。


# 任务 14: 优化Masking reorg矩阵

批次：【批次 4】 依赖任务 13

状态：未开始

目的：在共享 fixture 优化后收敛 masking policy 的 14 个慢测，保留 durable reorg、pause/resume、分区和索引语义。

来源任务：用户提供的 `target/rust-test.Aw4Dhb` 慢测日志。

预计会话范围：一个聚焦会话可完成该测试族的基线、热点修复与定向验证。

## 文件

- 修改：`pkg/session/runtime/normal_ddl_masking_policy_test.rs`
- 修改：`pkg/session/runtime/normal_ddl_fixture.rs`
- 修改：`pkg/ddl/persistent_actions.rs`
- 修改：`pkg/session/runtime/normal_ddl_service.rs`
- 审查：`docs/agents/ddl/03-reorg-backfill.md`
- 审查：`docs/agents/ddl/07-modify-column.md`

## 上下文

- 目标覆盖 cloud/dist/ingest reorg、persistent subtasks、named scope、secondary index、analyze 和 table actions。不得减少分区、索引、stage 或 pause/resume。

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

- 行为：14 个来源测试的 row/index IDs、计数、checkpoint、subtask 和回滚断言全部保留。
- 失败验证测试：上述来源目标及其一秒性能门槛。
- 失败验证命令：`tools/check/rust-test-performance.sh --max-seconds 1 --runs 1 -- --package astersql-session -E 'test(/runtime::normal_ddl_masking_policy_test::(masking_policy_serving_factory_backfills_named_keyspace|masking_policy_sql_modify_|masking_policy_sql_table_actions_)/)'`
- 预期失败原因：当前 5.346–7.475 秒。
- 通过验证命令：同一命令改为 `--runs 3`。
- 模拟策略：真实持久化 worker/metadata；外部对象存储只使用现有可控边界。

## 步骤

1. 运行失败验证并保存退出码、有效测试数、三阶段耗时或采样。
2. 在任务13 fixture上测 reorg scan、persist、schedule、analyze。
3. 为批量 row/index 处理添加结果等价回归。
4. 优化重复事务/编码/调度，不合并状态阶段。
5. 运行 `cargo fmt --all`、通过验证、适用周边测试、`make lint` 与 diff 自审。

## 验证

- 运行：同一命令改为 `--runs 3`。
- 预期：14 项通过，stage/partition/index/row 计数不减，记录最大耗时。
- 所需证据：修复前后每个目标的耗时、退出码、有效测试数、保留的规模/矩阵/断言，以及未达一秒项的可复现下界。

## 完成

如修改 Rust 源文件，确认顶部保留 PingCAP Apache License 并增加 `// Copyright 2026 AsterSQL.`；记录确切修改文件和符号、目标测试的三次耗时及 Ready 验证。获得证据后状态改为 `已完成` 并使用技能 `$git-commit` 独立提交；无关环境阻断才可标记 `已完成，待回归`，真实未解决热点不得误标完成。


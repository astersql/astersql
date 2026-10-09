# 任务 7: 修复 session DDL 生命周期

批次：【批次 3】 依赖批次 1

状态：已完成，待回归验证

回归记录（2026-10-09）：

- 聚焦失败验证先稳定复现：4 项中 1 通过、2 失败、1 超时，退出码 100；症状分别为 worker 关闭计数旧断言、已实现 create-table handler 被旧测试误当 unavailable、partial-index runtime 已支持后旧断言仍期待 deferred。
- 修复后聚焦验证：更新后的 4 项全部通过，退出码 0，772 项跳过；worker 生命周期项耗时 4.501 秒，无后台 panic 或 10 秒超时。
- `cargo fmt --all` 退出码 0。
- Ready 交付检查 `make lint` 退出码 0。
- `test(normal_ddl)` 广集启动 219 项，但前 10 项中的 5 个任务外 `normal_ddl_create_materialized_view_shadow_test` 在默认 10 秒预算超时；为遵守昂贵广扫范围规则主动中止，退出码 100，4 通过、1 SIGINT、5 超时、209 未运行。该任务外慢测阻止完整广集形成通过证据，需后续批次回归。
- Cargo 共享槽位：slot 1；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。

目的：修复 DDL worker 调优、handler unavailable、schema barrier 恢复和 partial-index validation 的实际生命周期错误。

来源任务：4 个 `pkg/session/runtime/normal_ddl*_test.rs` 真实失败及同组超时。

预计会话范围：围绕 normal DDL system-session 生命周期与关闭/恢复语义；若分诊证明多个无关根因，先处理共享生命周期根因并把独立项记录为阻塞/新增计划需求。

## 文件

- 修改：`pkg/session/runtime/` 中 normal DDL/system-session 生产文件
- 修改：`pkg/ddl/` 中不可缺少的局部接线（仅有直接证据时）
- 测试：`pkg/session/runtime/normal_ddl_masking_policy_test.rs`
- 测试：`pkg/session/runtime/normal_ddl_test.rs`

## 上下文

- 修改前必须阅读 `docs/agents/ddl/README.md`；测试表现包括 idle worker 未关闭、JSON 索引 panic、barrier 恢复失败与 worker panic。

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

- 行为：DDL worker tune(wait=true) 关闭空闲实例；handler 不可用返回稳定错误；barrier/validation 失败原子化且可重启恢复。
- 失败验证测试：现有四个失败测试，并对任务 1 确认的同根因超时加入最小回归。
- 失败验证命令：`cargo nextest run --locked -p astersql-session -E 'test(masking_policy_sql_modify_dist_reorg_tunes_actual_worker_lifetimes) | test(crossks_align_normal_ddl_unavailable_handler_preserves_legal_job_without_cancellation) | test(normal_ddl_plan_schema_barrier_unsynced_nodes_prevent_history_and_restart_recovers) | test(normal_ddl_plan_table_validation_partial_index_is_explicitly_deferred)' --test-threads 1`
- 预期失败原因：资源关闭计数不符、后台 panic 或恢复超时。
- 通过验证命令：同命令，并对 `test(normal_ddl)` 过滤集串行复跑。
- 模拟策略：复用现有 fake handler/barrier 边界，生命周期、队列和 worker 使用真实实现。

## 步骤

1. 阅读 DDL 指引、目标 package `doc.go`（若存在）和 Go 对应代码。
2. 用 RustCodeGraph 分别跟踪 worker close、system-session payload、barrier/validation 调用流。
3. 先复现失败，再修复共享生命周期/错误传播；不得删减 Go 分支。
4. 格式化并串行运行 DDL 聚焦集，检查线程和锁清理。

## 验证

- 运行：`cargo fmt --all`
- 运行：上述聚焦与 `test(normal_ddl)` 命令。
- 预期：有效测试均通过，无后台 panic、泄漏或 10 秒超时。
- 所需证据：失败前后、测试数、退出码、worker/锁清理断言。

## 完成

报告 DDL 生命周期根因、Go 对齐依据与未覆盖外部服务路径；完成后使用 `$git-commit` 提交。

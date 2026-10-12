# 任务 3: 迁移 executor、planner 与 statistics failpoint

批次：【批次 1】 无

状态：未开始

目的：迁移执行、规划和统计子系统中实际生效的 Rust failpoint key，并同步各自 Rust 测试。

来源任务：全仓类似位置分析。

预计会话范围：三个紧密相连的查询处理子系统；改动都是 key 对齐，验证按 crate 分组，无需改变业务算法。

## 文件

- 修改：`pkg/executor/analyze.rs`
- 修改：`pkg/executor/batch_point_get.rs`
- 修改：`pkg/executor/point_get.rs`
- 修改：`pkg/executor/shuffle.rs`
- 修改：`pkg/store/copr/batch_request_sender.rs`
- 测试：`pkg/executor/executor_failpoint_test.rs`
- 测试：`pkg/executor/importer/import_test.rs`
- 测试：`pkg/executor/importer/importer_testkit_test.rs`
- 测试：`pkg/executor/importer/table_import_testkit_test.rs`
- 测试：`pkg/executor/test/analyzetest/panictest/panic_test.rs`
- 测试：`pkg/executor/test/autoidtest/autoid_test.rs`
- 测试：`pkg/executor/test/cte/cte_test.rs`
- 测试：`pkg/executor/test/ddl/ddl_test.rs`
- 测试：`pkg/executor/test/fktest/foreign_key_test.rs`
- 测试：`pkg/executor/test/infoschema/infoschema_test.rs`
- 测试：`pkg/executor/test/jointest/join_test.rs`
- 测试：`pkg/executor/test/oomtest/oom_test.rs`
- 测试：`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_test.rs`
- 测试：`pkg/planner/core/casetest/correlated/correlated_test.rs`
- 测试：`pkg/planner/core/casetest/partition/integration_partition_test.rs`
- 测试：`pkg/planner/core/casetest/partition/partition_pruner_test.rs`
- 测试：`pkg/planner/core/casetest/planstats/plan_stats_test.rs`
- 测试：`pkg/statistics/handle/cache/statscache_test.rs`
- 测试：`pkg/statistics/handle/handletest/handle_test.rs`
- 测试：`pkg/statistics/handle/storage/gc_test.rs`
- 修改：`pkg/statistics/handle/util/util.rs`

## 上下文

- 文件中包含大量从 Go 移植时保留的原始文本；只有传给 Rust failpoint API 或与生产注入点配对的常量属于迁移范围。跨 crate key（如 store/copr）必须同时追踪实际生产者。

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

- 行为：迁移后的 AsterSQL key 继续触发 point-get、analyze、planner 和 statistics 的既有故障路径。
- 失败验证测试：在 `executor_failpoint_test.rs` 选择一个现有生产者/消费者用例，先迁移测试常量。
- 失败验证命令：`cargo test -p astersql-executor point_get_failpoints_are_consumed_at_the_canonical_second_read_boundary -- --exact`
- 预期失败原因：生产者仍监听旧 key。
- 通过验证命令：`cargo test -p astersql-executor point_get_failpoints_are_consumed_at_the_canonical_second_read_boundary -- --exact`
- 模拟策略：复用现有测试 fixture 和 failpoint，不扩大为真实外部服务测试。

## 步骤

1. 按 crate 建立活跃 key 配对清单并剔除 Go 原文。
2. 获取一个 executor 用例的 fail-before 证据。
3. 成对迁移 executor/store、planner 和 statistics key。
4. 分 crate 执行最小测试过滤器并审查差异。
5. 分类三个子树中的剩余 PingCAP 字符串。

## 验证

- 运行：`cargo fmt --all --check`
- 运行：`cargo test -p astersql-executor point_get_failpoints_are_consumed_at_the_canonical_second_read_boundary -- --exact`
- 运行：`cargo test -p astersql-planner-core test_xf_de_correlate_should_delete_intermediary_apply -- --exact`
- 运行：`cargo test -p astersql-statistics-handle test_extrem_case_of_gc -- --exact`
- 预期：每个受影响 crate 至少有一个有效行为测试通过，活跃 key 无旧前缀。
- 所需证据：fail-before/pass-after、每条命令退出码与有效测试数、槽位信息、剩余命中分类。

## 完成

指定查询处理子系统的 Rust failpoint 生产者和消费者统一使用 AsterSQL 路径，不修改上游溯源文本。


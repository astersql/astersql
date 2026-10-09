# 任务 2: 修复 planner 统计句柄接线

批次：【批次 2】 依赖批次 1

状态：未开始

目的：修复 planner 真实构建/优化路径在同步统计 pending 时缺失 `StatsHandle` 的共享根因，恢复相关 Go 对齐测试。

来源任务：原日志中 `astersql-planner-core` 约四十个共享失败。

预计会话范围：聚焦 `PlanContext`、planner 测试构造器和统计同步局部接线；不得逐个吞错或把同步统计路径改成伪统计。

## 文件

- 修改：`pkg/planner/core/` 中创建/使用 `PlanContext` 与统计加载的生产文件（以 RustCodeGraph 定位结果为准）
- 修改：`pkg/planner/planctx/` 中必要的局部接线（仅当生产契约要求）
- 测试：`pkg/planner/core/integration_test.rs`
- 测试：`pkg/planner/core/cbo_test.rs`
- 测试：`pkg/planner/core/enforce_mpp_test.rs`
- 测试：`pkg/planner/core/logical_plans_test.rs`
- 测试：`pkg/planner/core/main_test.rs`
- 测试：`pkg/planner/core/physical_plan_test.rs`
- 测试：`pkg/planner/core/planbuilder_test.rs`

## 上下文

- 失败文本一致为 `synchronous statistics are pending but this PlanContext has no StatsHandle`。
- 实现前用 RustCodeGraph 查找该错误的产生点、`StatsHandle` 调用方及已有成功测试的 context 构造模式；保持 Go 同步统计 fallback/等待语义。

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

- 行为：无显式测试句柄的标准 planner 构造仍能完成 Go 等价的统计加载/伪统计 fallback，而具备句柄时继续执行真实同步加载。
- 失败验证测试：扩展最小的 `pkg/planner/core/*_test.rs` 构造器回归，覆盖 pending statistics。
- 失败验证命令：`cargo nextest run --locked -p astersql-planner-core -E 'test(integration_test::common_handle_secondary_index_range_planning) | test(enforce_mpp_test::approx_count_distinct_without_group_by_builds_two_phase_stream_agg)'`
- 预期失败原因：当前返回无 `StatsHandle` 错误。
- 通过验证命令：`cargo nextest run --locked -p astersql-planner-core`
- 模拟策略：使用仓库真实测试 infoschema/statistics 构造；只模拟明确的 stats handle 边界。

## 步骤

1. 阅读目标 package 的 `doc.go`（若存在）并用 RustCodeGraph 定位错误产生点、调用方和受影响测试。
2. 增加最小失败回归，确认不是靠更新 golden 消除错误。
3. 按已有成功路径接入 stats handle 或正确 fallback，保持 Go 语义。
4. 运行 `cargo fmt --all`、聚焦测试和整个 planner-core crate。
5. 审查所有原共享失败不再出现该错误，再提交。

## 验证

- 运行：`cargo fmt --all`
- 运行：`cargo nextest run --locked -p astersql-planner-core`
- 预期：有效测试数大于零，所有共享 `StatsHandle` 失败通过；残余 golden 差异独立报告给任务 9。
- 所需证据：修复前错误、修复后退出码 0、测试数、RustCodeGraph 影响范围和差异审查。

## 完成

列出实际生产文件和测试文件、接线符号及 Go 行为依据。完成后使用 `$git-commit` 提交本任务变更。

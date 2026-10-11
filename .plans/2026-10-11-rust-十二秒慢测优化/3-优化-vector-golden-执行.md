# 任务 3: 优化 vector golden 执行

批次：【批次 3】 依赖批次 2

状态：未开始

目的：只处理 fixture 之外经采样确认的 golden 加载、解析或重复查询开销，保持所有 SQL、plan 和 warning 断言。

来源任务：任务 2 后三个 vectorsearch 目标的剩余耗时。

预计会话范围：`load_ann_suite`、`assert_ann_suite` 及相邻测试；不再改 fixture 数据生成。

## 文件

- 修改：`pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`
- 测试：`pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`
- 数据只读：`pkg/planner/core/casetest/vectorsearch/testdata/ann_index_suite_*.json`

## 上下文

- `assert_ann_suite` 必须执行每个 suite 的完整输入并逐项比较 plan/warning。
- 只有分段证据证明存在重复读取、解析或同一 SQL 双执行时才能改；不得缓存会受 SET/UPDATE 或 session warning 影响的结果。

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

- 行为：Auto 与 ForceTiKV 各 11 条、HeavyFunction 21 条 golden 输入在两种 Cascades 模式完整执行。
- 失败验证测试：添加调用计数/输入覆盖断言，固定 suite 名、每模式 case 数和 SET/UPDATE 状态影响；优化前先让针对新执行接口的测试失败。
- 失败验证命令：`cargo nextest run --locked --package astersql-planner-core-casetest-vectorsearch -E 'test(=vector_index_test::ann_suite_executes_every_go_case_once)'`
- 预期失败原因：覆盖计数接口或测试尚不存在。
- 通过验证命令：同上，并运行三个目标一次。
- 模拟策略：真实 planner/TestKit；只允许缓存不可变 testdata 解析结果。

## 步骤

1. 采样任务 2 后剩余阶段；若 fixture 已使三项稳定低于 12 秒且无显著剩余重复开销，记录无代码变更并结束。
2. 有证据时先添加覆盖计数失败测试，再实施最小去重或不可变加载复用。
3. 运行格式化、覆盖测试及三个目标一次，核对 warning/plan 无变化。

## 完成

所有 Go golden case 与双模式覆盖不减、功能测试通过且剩余开销下降后标记 `已完成`；有代码变更则用 `$git-commit` 提交并删除任务文件，无代码变更则记录证据后删除。

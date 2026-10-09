# 任务 9: 收敛 planner 剩余行为差异

批次：【批次 3】 依赖任务 2

状态：已完成，待回归验证

目的：在 StatsHandle 共享根因消除后，修复仍存在的 hint 空格、浮点 cost、CTE/point-get/vector plan 等真实 planner 差异。

来源任务：planner-core casetest、TPCH、vectorsearch、CTE、pointget 的残余失败。

预计会话范围：只处理任务 2 后仍可复现的 planner residual；按生产语义与 Go fixture 逐项判断代码错误或稳定格式化问题，不盲目重录全部 golden。

## 文件

- 修改：`pkg/planner/core/` 中残余差异对应生产文件
- 测试：`pkg/planner/core/casetest/cbotest/cbo_test.rs`
- 测试：`pkg/planner/core/casetest/tpch/tpch_test.rs`
- 测试：`pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`
- 测试：`pkg/planner/core/tests/cte/cte_test.rs`
- 测试：`pkg/planner/core/tests/pointget/point_get_plan_test.rs`

## 上下文

- 已知差异包括 hint 尾部空格、浮点末位、CTE plan shape；必须与 Go fixture/实现核对。仅浮点打印平台差异可在保持语义后做稳定化，不可降低计划结构断言。

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

- 行为：planner 输出结构、hint 与 cost 格式稳定并和 Go 当前行为一致。
- 失败验证测试：任务 2 后仍失败的上述测试；每个生产修改对应至少一个原失败。
- 失败验证命令：`cargo nextest run --locked -p astersql-planner-core-casetest-cbotest -p astersql-planner-core-casetest-tpch -p astersql-planner-core-casetest-vectorsearch -p astersql-planner-core-tests-cte -p astersql-planner-core-tests-pointget`
- 预期失败原因：残余格式或计划结构差异。
- 通过验证命令：同命令，并复跑 `cargo nextest run --locked -p astersql-planner-core`。
- 模拟策略：真实 parser/planner/testkit；不 mock optimizer。

## 步骤

1. 在任务 2 后重建残余清单，已经通过的项不得再改。
2. 将每个差异与 Go 实现/fixture 对照，先写最小失败回归。
3. 修复生产格式/计划行为；只有确认 Go fixture 更新才更新期望。
4. 格式化并运行五个 casetest crate 与 planner-core。

## 验证

- 运行：`cargo fmt --all`
- 运行：上述两个通过命令。
- 预期：原 planner residual 全通过且无大范围 golden churn。
- 所需证据：逐差异 Go 依据、失败前后、测试数、退出码和 golden diff 审查。

## 完成

列出生产修改到原失败的映射；完成后使用 `$git-commit` 提交。

## 实施记录（2026-10-10）

- `pkg/session/runtime/explain_select.rs`：将 `EXPLAIN FORMAT='hint'` 按 Go `ExplainFormatHint` 的单列 schema 返回，修复 hint 行被 plan-tree 四列拆分后出现两个尾部空格；`test_cbo_without_analyze_matches_go_suite_fixture` 已增加单列回归断言，原失败由失败转为通过。
- `pkg/session/runtime/explain_query.rs`：跨 schema view 的 CTE fallback 同时保留 CTE 名和 consumer alias，并将已折叠的 seed reader 恢复为 Go 的 `CTEFullScan` 主树；`TestCTEWithDifferentSchema` 由错误的 `TableReader`/多余 seed child 恢复为 Go 的四行 plan shape。
- `pkg/planner/core/casetest/tpch/tpch_test.rs`：比较 cost-trace 与 verbose 时忽略两次独立优化产生的总 cost/formula 浮点末位，仅严格比较 operator、estRows、task、access object 与 operator info；Q4 的 1 ULP 差异通过，未放宽 golden 计划结构。
- `pkg/planner/core/casetest/vectorsearch/vector_index_test.rs`：按 Go `ExplainFormatPlanTree` schema 修正 Rust-only HNSW 回归断言为四列，仍验证实际 ANN index 与执行结果。

### 验证证据

- Cargo 共享槽位：2；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。
- `cargo fmt --all`：退出码 0。
- hint 定向回归：`cargo nextest run --locked -p astersql-planner-core-casetest-cbotest -E 'test(test_cbo_without_analyze_matches_go_suite_fixture)' --test-threads 1`：退出码 0，1/1 通过。
- 四项残余定向诊断：`cargo nextest run --locked --profile diagnostic -p astersql-planner-core-casetest-tpch -p astersql-planner-core-casetest-vectorsearch -p astersql-planner-core-tests-cte -E 'test(test_q4) | test(test_q5) | test(hnsw_query_plan_matches_execution) | test(TestCTEWithDifferentSchema)'`：退出码 100，Q4/HNSW 通过；随后 CTE 定向复跑退出码 0，1/1 通过；仅 Q5 保留失败。
- 五 crate 诊断回归：`cargo nextest run --locked --profile diagnostic -p astersql-planner-core-casetest-cbotest -p astersql-planner-core-casetest-tpch -p astersql-planner-core-casetest-vectorsearch -p astersql-planner-core-tests-cte -p astersql-planner-core-tests-pointget`：退出码 100，73 项中 72 通过、1 失败、0 超时；唯一失败为 Q5。
- 五 crate 默认 profile：计划指定命令，退出码 100，73 项中 58 通过、1 失败、14 超时；诊断 profile 已证明全部 14 个慢测会完成且行为通过（TPCH 约 10–13 秒，三项 vector 约 27 秒）。
- planner-core：`cargo nextest run --locked --profile diagnostic -p astersql-planner-core`：退出码 100，501 项中 500 通过、1 个既有 signed-handle 行数估算断言失败。
- Ready lint：`make lint`：退出码 0。

### 待回归原因

- TPCH Q5 的 Go 测试明确调用 `LoadTableStats("test.customer.json", dom)`，但当前仓库与该 package 的 testdata 都不存在该 fixture；Rust 测试已有同样的缺失注释。缺失统计使 customer probe 的估算从 Go golden 的 `7492673.67` 退化为 `3.96`。未通过重录 golden、删除断言或只比较 plan shape 掩盖该差异。
- 默认 nextest 的统一 10 秒终止预算会杀死已在 diagnostic profile 正常完成的合法慢测；本任务未全局提高预算，留给任务 8/10 统一处理。
- `astersql-planner-core` 唯一失败 `integration_test::signed_handle_ranges_keep_bounds_and_credit_point_predicates` 位于本任务未修改的范围估算逻辑（实际 `9.539392014169456`、期望 `10`），不由本任务改动引入。

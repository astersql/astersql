# 任务 4: Go 物理计划桥接合同验收

批次：【批次 4】依赖批次：批次 3

状态：未开始

目的：逐分支证明 Rust 桥接满足 Go RU 输入合同，并把完成证据交给原 RU v3 计划任务 1。

来源任务：187、197；原 RU v3 计划任务 1

预计会话范围：审计 Go/Rust 计划字段、补缺失回归和必要修复；不实施 RU 计算或独立 SQL 功能。

## 文件

- 测试：`pkg/executor/statement_ru_plan_walk_test.rs`、`pkg/session/runtime_test/typed_adapter_bridge.rs`
- 修改：只有验收发现缺口时才修改 `pkg/planner/core/flat_plan.rs`、`pkg/executor/adapter.rs` 或实际归属的桥接文件
- 交接：`.plans/2026-09-29-ru-v3-直接移植剩余任务/1-真实物理计划桥接.md`

## 上下文

- Go `flat_plan.go` 主树、CTE、ScalarSubQueries 和包装计划分支是验收清单；Go RU `calculateStatementRUInternal` 只读这些结构与真实运行证据。
- 原 RU 计划任务 1 在本计划完成后做验收，不构成本计划前置。任务 2 的执行统计证据与任务 3–10 的 RU 逻辑不在此处实现。

## 验证计划

- 行为：Reader、Join、Shuffle、写语句、CTE、标量子查询、Execute 和 Explain 的真实树/森林及计划类型与 Go 一致。
- 验证测试：扩充 `go_merge_187_typed_plan_bridge`、`go_merge_187_typed_plan_forest`、`go_merge_197_wrapped_typed_exec_stmt_plan`；发现缺陷时先记录失败再修复。
- 验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187_typed_plan` 和 `cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_197_wrapped_typed_exec_stmt_plan`。
- 模拟策略：真实节点与生产构造器；不以摘要或估算证据充数。

## 步骤

1. 把 Go switch/分类各分支映射到 Rust 实现和测试断言，列出缺失项。
2. 对缺失项先运行失败测试，再补最小桥接并重跑；检查类型与子树索引。
3. 执行 Ready 检查，自审差异；将验收结果记录到原 RU 任务 1，供其按来源技能规则完成。

## 验证

- 运行：上述两条 Rust 测试、受影响 session 定向测试、`cargo fmt --all -- --check`、`make lint`、`git diff --check`。
- 预期：Go 计划分类与三类树逐分支有真实 Rust 证据，没有逆向依赖原 RU 任务 2–10。
- 所需证据：Go/Rust 分支矩阵、失败与通过结果、各命令退出码、未验证项和差异审查。

## 完成

证据齐全后按执行技能删除本任务文件；原 RU 计划任务 1 单独验收与关闭。阻塞只在本文件记录，不修改 `plan.md`。

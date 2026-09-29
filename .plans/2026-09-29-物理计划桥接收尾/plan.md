# Go RU v3 物理计划桥接收尾计划

目标：让 Rust `ExecStmt` 按 Go RU v3 的实际计划入口和树形合同，提供可遍历的真实物理算子与包装计划。

范围：对照 Go `pkg/planner/core/flat_plan.go` 的 `FlattenPhysicalPlan`、Go `pkg/executor/statement_ru_result.go` 的 `classifyStatementRUPlan` 和 `statement_ru_plan_walk.go` 的森林遍历输入，补齐 Rust 主树、CTE/标量子查询独立树、`Execute`/`Explain`/写语句包装计划，以及生产 `ExecStmt` 传递。只做计划桥接和验证，不实现 RU 公式。

范围外：`FULL OUTER JOIN` SQL 语法与执行、CTE 结果求值、任意 SELECT 的 SQL 行为扩展、RU 运行时证据与计费发布。这些不是 Go RU 桥接的前置条件；发现独立功能缺口时记录到对应来源任务，不把它们强加给本计划。

假设：已有 `ExecStmt.TypedPlan`、`FlattenTypedPhysicalPlan` 和窄范围的 prepared SELECT 适配器入口。它们只是部分桥接；Go RU 在 owner 安装时处理 `Execute`/`Explain` 包装，并使用 `FlatPhysicalPlan.Main`、`CTEs`、`ScalarSubQueries` 三类树。

## 设计决策

- Go 源码是合同。逐个对照包装计划解包、主树特殊子节点、CTE 去重和标量子查询独立树；Rust 独有适配仅用于接到现有会话所有权，不增删 Go RU 分支。
- 任务 1–4 线性执行且都早于 `.plans/2026-09-29-ru-v3-直接移植剩余任务/1-真实物理计划桥接.md` 的验收；本计划不依赖那个验收任务，避免跨计划环。
- 缺失真实物理节点或执行证据时按 Go 的不支持/失败合同处理，不能用 `PlanInfo`、估算行数或手工 SQL 结果冒充。

## 架构说明

- Go 树由 `pkg/planner/core/flat_plan.go` 建立，RU 在 `pkg/executor/statement_ru_plan_walk.go` 读取；Rust 对应在 `pkg/planner/core/flat_plan.rs`、`pkg/executor/adapter.rs`、`compiler.rs`、`pkg/session/runtime/typed_adapter_bridge.rs`。
- `pkg/planner/core/base/doc.go` 约束基础接口。Rust 计划/测试分文件，保留 PingCAP 版权并按仓库规则加 AsterSQL 版权。后续 RU 证据、资格、遍历及发布继续由原 RU v3 计划任务 2–10 实施。

## 开发策略

- 每项行为先写失败测试，再按 Go 对应分支实施并重跑；测试必须读取真实类型化节点与子树边界。
- 只在编号任务文件记录进度和阻塞；本 `plan.md` 是架构参考。交付使用 `AGENTS.md` 的 Ready 检查；仅在 Bazel 触发条件满足时运行 `make bazel_prepare`，不运行 `make bazel_lint_changed`。

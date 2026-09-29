# RU v3 直接移植剩余任务计划

目标：把 Go 的 statement RU v3 计算、生命周期与发布行为完整移植到 Rust 生产路径。

范围：承接来源任务 187、195、197 和依赖任务 20 的未完成部分；保留已写的 Rust 公式与测试。物理计划输入先由 `.plans/2026-09-29-物理计划桥接收尾/` 的四个任务按 Go `flat_plan.go` 完成，本计划任务 1 再验收桥接，任务 2–10 依次处理证据、遍历、终态、发布和旧调用迁移。

范围外：修改 Go 的 RU 算法、以估算值替代缺失证据、为使测试通过而简化算子分支；`FULL OUTER JOIN` 语法/执行和 CTE SQL 求值不是 Go RU 物理计划桥接的前置任务。

假设：现有 Rust `PlanInfo` 仅为摘要；Go 遍历依赖带类型的物理算子和运行时统计。Rust 已有部分 `TypedPlan` 桥接及通过的聚焦测试，但包装计划、独立树和完整生产入口仍需按 Go 合同验收。

## 设计决策

- 直接以 `pkg/executor/statement_ru_plan_walk.go`、`statement_ru_result.go`、`statement_ru_reporting.go` 为行为准绳，逐分支移植，不引入替代计算路径。
- 物理计划桥接有两种途径：扩充摘要或保留真实类型。选保留真实执行计划及证据，因为摘要无法恢复 Go 的算子字段和执行统计。桥接子计划不依赖本计划任务 1，任务 1 仅在桥接子计划完成后验收，避免循环依赖。
- 新路径完整可用之前保留旧 RUv2 API；只在终态语义和发布值经验证后迁移调用方。来源任务文件按原计划规则处理，本目录不改原 `plan.md`。

## 架构说明

- Rust 入口涉及 `pkg/executor/compiler.rs`、`adapter.rs`、`statement_ru_*.rs`；计划结构在 `pkg/planner/core/flat_plan.rs`、`common_plans.rs` 与 `physicalop`；共享公式在 `pkg/resourcegroup/ruv2/model.rs`。
- 先定位真实 `ExecStmt` 构造和执行链，再接线。缺失的运行时证据必须明确返回不支持或失败，不能发布成功 RU。
- Rust 源码与测试分文件；保留 PingCAP 版权。真正可用的 Rust 文件顶部加入 `// Copyright 2026 AsterSQL.`。遵守仓库的 Bazel、failpoint 与 Ready 验证门槛。

## 开发策略

- 每个行为任务先写失败测试，确认预期失败，再移植 Go 逻辑并通过同一测试；测试覆盖真实算子和真实证据形状。
- 先完成桥接子计划批次 1–4，再执行本计划批次 1–9；任务 10 还需 Go 合并同步计划的消费端任务 40、139 已完成。Go 合并同步计划的任务 20/187/195/197 排在本计划任务 10 之后作来源覆盖验收，不是本计划前置。这些依赖均指向已完成的上游工作，不形成环。
- 每个任务只在取得任务文件要求的当前证据后算完成。主工作区的无关编译阻塞应记录实际诊断及隔离验证，不得把隔离临时补丁视为交付代码。
- 最终按 `AGENTS.md` Ready 规则运行适用的测试、`cargo fmt --all -- --check`、`make lint` 和差异检查；任何触发 Bazel 准备的变更先执行 `make bazel_prepare`。

# RU v3 直接移植剩余任务计划

目标：把 Go 的 statement RU v3 计算、生命周期与发布行为完整移植到 Rust 生产路径。

范围：承接来源任务 187、195、197 和依赖任务 20 的未完成部分；保留已写的 Rust 公式与测试，补齐物理计划、运行时证据、算子遍历、所有终态及指标发布，最后迁移旧调用方。

范围外：修改 Go 的 RU 算法、以估算值替代缺失证据、为使测试通过而简化算子分支、无关的 planner/session 编译错误。

假设：现有 Rust `PlanInfo` 仅为摘要；Go 遍历依赖带类型的物理算子和运行时统计。当前 Rust 单测在主工作区受无关 `FullJoin` 编译错误阻断，隔离工作树的临时修复不能直接交付。

## 设计决策

- 直接以 `pkg/executor/statement_ru_plan_walk.go`、`statement_ru_result.go`、`statement_ru_reporting.go` 为行为准绳，逐分支移植，不引入替代计算路径。
- 物理计划桥接有两种途径：扩充摘要或保留真实类型。选保留真实执行计划及证据，因为摘要无法恢复 Go 的算子字段和执行统计。
- 新路径完整可用之前保留旧 RUv2 API；只在终态语义和发布值经验证后迁移调用方。来源任务文件按原计划规则处理，本目录不改原 `plan.md`。

## 架构说明

- Rust 入口涉及 `pkg/executor/compiler.rs`、`adapter.rs`、`statement_ru_*.rs`；计划结构在 `pkg/planner/core/flat_plan.rs`、`common_plans.rs` 与 `physicalop`；共享公式在 `pkg/resourcegroup/ruv2/model.rs`。
- 先定位真实 `ExecStmt` 构造和执行链，再接线。缺失的运行时证据必须明确返回不支持或失败，不能发布成功 RU。
- Rust 源码与测试分文件；保留 PingCAP 版权。真正可用的 Rust 文件顶部加入 `// Copyright 2026 AsterSQL.`。遵守仓库的 Bazel、failpoint 与 Ready 验证门槛。

## 开发策略

- 每个行为任务先写失败测试，确认预期失败，再移植 Go 逻辑并通过同一测试；测试覆盖真实算子和真实证据形状。
- 编号任务按批次线性执行，避免共享源码、Cargo 构建目录及终态集成争用。
- 每个任务只在取得任务文件要求的当前证据后算完成。主工作区的无关编译阻塞应记录实际诊断及隔离验证，不得把隔离临时补丁视为交付代码。
- 最终按 `AGENTS.md` Ready 规则运行适用的测试、`cargo fmt --all -- --check`、`make lint` 和差异检查；任何触发 Bazel 准备的变更先执行 `make bazel_prepare`。

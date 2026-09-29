# 物理计划桥接收尾计划

目标：让已支持的 SQL `EXECUTE` 与 `EXPLAIN ANALYZE` 保留并使用真实 Rust 物理算子树，补齐 Full Join、CTE 和一般查询的桥接覆盖。

范围：承接 `.plans/2026-09-29-ru-v3-直接移植剩余任务/1-真实物理计划桥接.md` 尚未完成的部分。处理 Rust SQL 解析、Join/CTE 物理计划、会话执行入口、类型化 EXPLAIN 和端到端验证。每个行为保持与现有 Go 路径一致。

范围外：RU v3 计费公式、运行时证据、发布与旧 RUv2 API 迁移；这些仍由原 RU v3 计划任务 2–10 跟踪。Go 源码和外部 Rust 依赖也不在本计划内。

假设：`ExecStmt.TypedPlan`、`FlattenTypedPhysicalPlan`、`SessionBoundAdapterOwner::BuildPreparedExecStmt` 已存在。当前生产 SQL 分发仅将单表、直接列投影、LIMIT 或主键等值查询接到这条路径；其他形态仍走既有执行路径。

## 设计决策

- 以真实 `dyn PhysicalPlan` 和执行器证据作为单一来源；`PlanInfo` 只保留摘要用途。先使每个新增形态有失败测试，再扩展桥接。
- Rust parser 以 `pkg/parser/grammar/main.astergram` 为语法来源；变更后由 `astersql-parsergen` 生成并检查提交的解析表，不手改生成表。
- 完整 SQL 执行、CTE、Full Join 分阶段接入；不支持的形态保持明确的旧路径或错误，不能用估算值冒充已执行计划。
- 所有任务按批次线性执行，因为后续任务依赖前一批次的计划形态，且 Cargo 构建及会话测试共用资源。

## 架构说明

- 解析器位于 `pkg/parser`，逻辑/物理计划位于 `pkg/planner/core`，实际执行和计划树读取位于 `pkg/executor`，SQL 分发与 EXPLAIN 位于 `pkg/session/runtime`。
- `pkg/planner/core/base/doc.go` 的接口约束适用于新增物理计划方法。Rust 源码和测试必须分文件，保留 PingCAP 版权；真正可用的 Rust 文件顶部增加 AsterSQL 版权行。
- 仓库 `AGENTS.md` 决定 Bazel 准备、Failpoint、Ready 验证和交接。若会话单测被缺失的 `tests/mysqlcompat/compatibility-cases.json` 阻断，只可记录并使用测试命令内的临时文件做隔离验证，不把假清单提交。

## 开发策略

- 对行为变化先运行新增失败回归，再实现、通过并审查差异；选择真实 parser、optimizer、会话和 KV 路径。
- 每项任务在自身文件记录当前证据和阻塞；本 `plan.md` 只作为架构指南，生成后不修改。
- 交付按 `AGENTS.md` 的 Ready 检查：聚焦 Rust 测试、适用的 parser Make 目标、`cargo fmt --all -- --check`、`make lint` 和 `git diff --check`。仅在 Bazel 触发条件成立时运行 `make bazel_prepare`；不运行 `make bazel_lint_changed`。

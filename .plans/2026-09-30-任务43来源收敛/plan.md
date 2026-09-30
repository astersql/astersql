# 任务 43 来源收敛与剩余实现计划

目标：以 Go 源码和原始差异为准，完成任务 43 尚未验证的行为，清楚处置扩展出来的 Rust 适配代码。

范围：`ad193e964b^1..ad193e964b` 中 `pkg/domain/` 的八个 Go 文件；当前任务 43 已引入、但仍需证明来源或生产可用性的 crossks owner、TTL timer、Inference/Jina、Extract 归档路径。先核对来源，再按单一行为实施和验证。共享工作区的其他任务改动不属于本计划。

范围外：重写整个 TiDB DDL/TTL/Inference 子系统，修复既有 Go 回归，真实多 Region TiKV 性能测试，以及修改原计划 `.plans/2026-09-29-go-合并-rust-同步/plan.md`。

假设：Rust 与 Go 不要求文件名一一对应，但每条新增行为必须能追溯到 Go 的函数、测试或 Rust 运行时必需的适配边界。现有工作区有大量未提交改动，实施者必须先确认文件归属，不得批量删除。

## 设计决策

- 先用原始 Go 差异建立行为清单，避免继续因“生产完整性”扩展无关功能。
- `pkg/inference/jina.rs` 对应 `pkg/inference/embedding/jina/jina.go`；目录不同不是无来源的证据，其请求和错误语义仍需核对。
- `pkg/session/runtime/ttl_runtime.rs`、`crossks_owner.rs` 没有同名 Go 文件。分别对照 `pkg/ttl/ttlworker/job_manager.go` 与 `timer.go`、`pkg/ddl/ddl.go` 的 owner/worker 链；无法证明需要的自定义行为应缩减或移除。
- TTL 完整 timer runtime 与 Extract 完整归档是当前任务文件额外承诺的生产验证，作为独立阶段完成，不倒推它们属于原始 Go 差异。

## 架构说明

- Domain 负责启停与依赖传递；Session runtime 仅作非 `Send` SQL 会话与 Rust 服务入口的适配；TTL 的 Go 调度语义以 `pkg/ttl/ttlworker` 和现有 `pkg/timer` 为准。
- Extract 的 Go 包结构、文件名、JSON/TOML 内容以 `pkg/domain/extract.go` 和 `plan_replayer_dump.go` 为准；HTTP 入口在 `pkg/server`。
- 先做来源核查和测试前置条件，再按批次顺序执行。每批次一个任务，避免共享 Cargo/测试资源及 `pkg/domain`、`pkg/session` 文件争用。

## 开发策略

- 行为变更先写失败回归，再作最小修复并验证通过；纯来源核查和最终审查使用证据清单。
- Rust 测试单独放在测试文件；保留 PingCAP 版权注释，修复后的 Rust 源文件顶部保留 AsterSQL 版权行。
- 选择最小有效测试；交付代码时按 `AGENTS.md` 的 Ready 要求运行 `make lint`、格式和差异检查。`make bazel_prepare` 只按 AGENTS 的触发条件执行，禁止 `make bazel_lint_changed`。
- 不把临时 `[]` 兼容性清单当成正式测试资产；若测试编译依赖缺失，先执行前置条件任务。

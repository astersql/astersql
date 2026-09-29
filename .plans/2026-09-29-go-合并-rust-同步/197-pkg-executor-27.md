# 任务 197: pkg/executor 第 27 组 Go 差异移植

批次：【批次 78】依赖：批次 77

状态：进行中

调度修订（2026-09-29）：本文件在 Go 同步计划中仅作来源覆盖验收；RU v3 代码由专门的桥接计划及 RU v3 计划按 Go 函数合同实现。前序普通批次不等待本文件；外部计划完成后在本批次核对该来源文件的逐分支证据并按原规则关闭。

实施记录（2026-09-29）：任务 20 的旧 RUv2 API 删除依赖本组 Go RU v3 结算代码。已核对 Go `statement_ru_result.go` 的 `currentStatementRUWeights`、`trimStatementRUExplainPrefix`、`classifyStatementRUScanEvidence`，以及 Rust `pkg/resourcegroup/ruv2/model.rs` 的 `StmtUnits`/`calculate`。已在独立 Rust 测试文件加入 `go_merge_197` 回归，并新建 `statement_ru_result.rs` 逐函数移植上述纯函数；覆盖零值、矛盾数据、缺失数据、正常比例、大整数、EXPLAIN 前缀。i64 三项计算的最大乘积仍小于 f64 最大值，因此大整数应为有效估计。曾尝试先造一个只返回 `StmtUnits` 的终端辅助函数；按用户纠正已删除，因为它跳过 Go `statementRUCalculator` 的 engine/report 语义。后续必须直接移植 Go 结算器与生产调用链，不能用简化的替代计算路径。整个 410 行 Go 文件尚未完成，不能删除任务文件。

继续移植 Go `statementRUCalculationSetup`、`newStatementRUCalculator`、`statementRUCalculator.finalize`：使用已移植的 ruv2 StmtUnits/Calculate、按 Go 公式修正 TiFlash 总量、检查所有 engine/result 非负且有限、复制 full report 并添加 statement-level units。隔离定向测试首轮 8/8 通过，包括 TiFlash 倍率、无效数值与冻结报告；为终端计算器配合调整 setup 入参后正在重跑。生产调用链、失败原因发布与 remainder 仍待移植。

最新验证：隔离定向 `go_merge_1` 14/14 通过；`cargo fmt --all -- --check`、`make lint`、`git diff --check` 通过。生产调用链、eligibility、publisher 仍待移植。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 197。

预计会话范围：1 个 Go 文件，合计 410 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/executor/statement_ru_result.go`（+410/-0）
- Rust 候选：`pkg/executor/statement_ru_result.rs（候选，先用索引确认）`
- Cargo 包线索：`pkg/executor/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/executor/statement_ru_result.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。


## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_197` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_197`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_197`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 格式：Rust 代码修改完成后，先运行 `cargo fmt --all` 自动格式化，再运行 `cargo fmt --all -- --check` 校验；自审格式化产生的差异。
- 运行：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_197`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

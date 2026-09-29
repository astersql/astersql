# 任务 20: pkg/util 第 4 组 Go 差异移植

批次：【批次 4】依赖：批次 3

状态：已阻塞

恢复记录（2026-09-29）：调度已确认同子系统前序任务 17 实现完成，其他子系统的任务 15 不阻塞本任务。现继续移植。经源码核对，本组 Go 差异删除了绝大多数 RUv2 计费 API，仅保留 coprocessor response bytes；Rust 的 `ruv2_metrics.rs` 仍有旧 API，`pkg/util/topsql/stmtstats`、`pkg/server/internal/resultset`、`pkg/sessionctx/variable`、`pkg/executor/adapter.rs` 等仍引用旧 API。需核对跨包任务归属，再做受控移植。

实施进度与再次阻塞（2026-09-29）：
- Go `UpdateRUV2MetricsFromRUV2`/`applyRawCounters` → Rust `ruv2_metrics.rs::applyRawCounters` → 独立 `go_merge_20_test.rs::go_merge_20_only_response_bytes_are_collected`：原始 RUv2 各字段中只累计 coprocessor response bytes，不再上报该字段的 Prometheus 计数。先失败证据：`cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib go_merge_20`，测试因 read RPC 结果为 3 而预期 0 失败；修复后 3/3 通过。
- Go `SyncRUV2MetricsFromRUDetails`、`AddTiKVCoprocessorResponseBytes`、`TiKVCoprocessorResponseBytes`、bypass/空值 → Rust 同名函数及 `go_merge_20` 另外两项测试：验证排空只转移一次、零值、bypass、负增量。现有相邻 raw-counter 测试已调整为当前 Go 语义。
- Go 删除的 `RUV2Weights`、commit details、executor/plan/resource/storage 指标、Clone/Merge、计费与格式化 API → Rust 仍保留旧逻辑，尚未覆盖。Rust 19 个文件仍引用这些旧 API，涉及 `topsql/stmtstats`、`server/internal/resultset`、`sessionctx/variable`、`executor`、`session/runtime` 及各自测试。逐项对照当前 Go 后确认 `topsql/stmtstats/stmtstats.go` 已改用版本判断和 RUDetails（清单任务 40），`server/internal/resultset/resultset.go` 已收缩为只同步 response bytes（任务 139）；新的 RU 计算位于 `pkg/executor/statement_ru_plan_walk.go`（任务 187）及 `statement_ru_result.go`（任务 197），当前尚无对应 Rust 文件，且这些任务文件状态为“未开始”。删除旧 API 会使跨包编译失败；必须在上述替代调用链可用后联动迁移。当前阻塞是具体尚未移植的下游实现，不是格式检查本身。不能将仅完成原始采集路径称为本文件的完整移植，也不能删除本任务文件。

恢复后已运行检查：`cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib go_merge_20`（先失败，修复后 3/3 通过）；`cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib`（调整相邻测试后 33/33 通过）；`cargo test --manifest-path pkg/util/execdetails/internal/ruv2/Cargo.toml --lib`（11/11 通过）；`git diff --check -- pkg/util/execdetails`（通过）；`rustfmt --edition 2021 --check pkg/util/execdetails/go_merge_20_test.rs`（通过）；`make lint`（通过）。`cargo fmt --all -- --check` 失败于并行修改的 `pkg/meta/model/engine_attribute.rs` 格式；本组新测试已单独格式检查通过。旧 `ruv2_metrics.rs` 自身还有大量既存格式差异，完整删减时应一起清理。

再次推进检查：用 `rustcodegraph explore 'RUV2Metrics TotalRU CursorRUV2Tracker'` 追踪调用链；Go 现有源码及覆盖清单确认任务 40、139、187、197 的替代路径。`go_merge_20` 测试已去除对待删除旧 API 的依赖，改以保留 API 与全局指标副作用检验；重跑同一聚焦命令 3/3 通过。`rustfmt --edition 2021 --check pkg/util/execdetails/go_merge_20_test.rs` 与 `git diff --check -- pkg/util/execdetails` 重跑通过。

已运行检查：`~/.rustcodegraph/bin/rustcodegraph status`（索引存在）；`git status --short`（工作树有其他任务的并行修改）；`git diff ad193e964b^1 ad193e964b -- pkg/util/execdetails/ruv2_metrics.go`（确认 +2/-962）；`rg` 搜索 Rust 调用方与批次 3 状态。未运行失败/通过 `cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib go_merge_20`、`cargo fmt --all -- --check`、`make lint`、`git diff --check`：依赖阻塞且尚无任务 20 代码改动，不能取得有效完成证据。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 20。

预计会话范围：1 个 Go 文件，合计 964 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/util/execdetails/ruv2_metrics.go`（+2/-962）
- Rust 候选：`pkg/util/execdetails/ruv2_metrics.rs`
- Cargo 包线索：`pkg/util/execdetails/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/util/execdetails/ruv2_metrics.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。

- 此组含超大单文件差异。先按 Go 函数和行为列出小段及其 Rust 对应测试，逐段完成；生成文件应追溯生成器与输入。不得只实现其中一段就删除任务文件。

## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_20` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib go_merge_20`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib go_merge_20`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 运行：`cargo test --manifest-path pkg/util/execdetails/Cargo.toml --lib go_merge_20`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

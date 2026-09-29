# 任务 5: pkg/meta 第 1 组 Go 差异移植

批次：【批次 1】依赖：无

状态：已完成，待回归

本任务行为和包内测试已完成；全工作区 `cargo fmt --all -- --check` 因本任务外已有 Rust 文件格式差异退出 1（例如 `pkg/ddl/job_submitter_test.rs`、`pkg/executor/aggfuncs/func_avg_test.rs`）。本任务三个 Rust 文件单独 `rustfmt --check` 通过。待其他任务处理全工作区格式差异后复跑该检查，再删除本任务文件。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 5。

预计会话范围：1 个 Go 文件，合计 321 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/meta/autoid/autoid_service.go`（+296/-25）
- Rust 候选：`pkg/meta/autoid/autoid_service.rs`
- Cargo 包线索：`pkg/meta/autoid/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/meta/autoid/autoid_service.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。


## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_5` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib go_merge_5`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib go_merge_5`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 运行：`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib go_merge_5`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

### 本次实施证据

- Go→Rust：`singlePointAlloc.Alloc`、`Rebase`、`ForceRebase`、`Transfer`、`Base`、`End` 对应 `SinglePointAllocator` 的同名 trait 方法及 `alloc_inner` / `rebase_inner`；重试策略、终止错误标记和日志状态分别对应 `RpcRetryPolicy` / `RpcRetryState`、`AutoIdError::RpcRetryLimit`、`RpcRetryLogState`。Go 的 keyspace oneof 在 Rust 本地 RPC 请求抽象中由 `AutoIdRequest.keyspace_id` 承载。Go 测试意图落在独立的 `autoid_service_test.rs` 中。
- 先失败：`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib go_merge_5`，两项初始回归测试分别因 base 为 `10` 而非 `20`、Transfer 后 base 为 `0` 而非 `42` 失败。修复后 7 项 `go_merge_5` 测试通过。
- 通过：`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib`（37/37）；`rustfmt --edition 2024 --check pkg/meta/autoid/autoid_service.rs pkg/meta/autoid/autoid_service_test.rs pkg/meta/autoid/errors.rs`；`git diff --check -- pkg/meta/autoid/autoid_service.rs pkg/meta/autoid/autoid_service_test.rs pkg/meta/autoid/errors.rs`；`make lint`，均通过。
- 待回归：`cargo fmt --all -- --check` 退出 1，输出指向本任务外多个文件的格式差异。本任务未修改 Go imports、Go 测试或 Bazel 元数据，故无需 `make bazel_prepare`。未做真实 AutoID 服务集成验证；Rust `Context` 通过 30 秒后取消表达 Go 写操作 deadline，当前 RPC 抽象没有暴露绝对 deadline 查询接口。
- 风险：本地状态以 `Mutex` 保护而非 Go 原子类型，但 RPC 不持有该锁，乱序返回测试通过；日志以标准错误输出字段记录，尚无仓库统一结构化 Rust 日志适配；真实服务对超时取消的响应需集成验证。
- 延后回归（当前共享工作区）：`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib go_merge_5` 7/7 通过；`cargo test --manifest-path pkg/meta/autoid/Cargo.toml --lib` 初次 36/37，通过前发现新增测试把重试调用次数固定要求为至少 3，但调度慢时第二次调用已同时满足错误次数和时长阈值。已改成至少 2 次，重跑包内 37/37 通过。`rustfmt --edition 2024 --check pkg/meta/autoid/autoid_service.rs pkg/meta/autoid/autoid_service_test.rs pkg/meta/autoid/errors.rs`、`git diff --check -- pkg/meta/autoid/autoid_service.rs pkg/meta/autoid/autoid_service_test.rs pkg/meta/autoid/errors.rs`、`make lint` 均通过。`cargo fmt --all -- --check` 仍因本任务外文件（如 `pkg/ddl/job_submitter_test.rs` 和 `pkg/session/runtime/dml.rs`）退出 1，故保持待回归状态，未删除任务文件。

# 任务 4: pkg/kv 第 1 组 Go 差异移植

批次：【批次 1】依赖：无

状态：已完成，待回归

## 执行记录（2026-09-29）

Go→Rust 覆盖：

| Go 文件 | Rust 落点与证据 |
| --- | --- |
| `checker.go` / `checker_test.go` | `checker.rs` 增加 `MinCount=3022`、`MaxCount=3023` 聚合白名单；`checker_test.rs` 的 `go_merge_4_max_min_count_are_supported` 及既有全枚举测试覆盖。 |
| `error.go` / `error_test.go` | `error.rs` 增加 TiKV 类 `ErrSharedLockLost`，复用已在 Rust `pkg/errno` 定义的 9015 错误码；`error_test.rs` 验证 SQL 错误码及完整错误原型列表。 |
| `kv.go` / `kv_test.go` | `kv.rs` 增加同步 `CoprRequestLimiter` 与查询范围的 `QueryCopStoreLimiter`，覆盖非正容量、阻塞/释放、取消、重复释放 panic、32 并发请求容量上限、store 0、同 store 身份复用及跨 store 独立性；`Request` 增加 Go 对应限流器和批处理字段；`kv_test.rs` 的 `go_merge_4_*` 验证。同步等待与现有 `pkg/store/copr` 同步发送模型一致。Rust `Request` 的构造点已同步更新。实际 cop RPC 的限流与批处理消费由清单任务 48 的 `pkg/store/copr/coprocessor.go` 差异负责，distsql 请求设置由任务 42 负责。 |
| `option.go` | `option.rs` 增加 `InternalTxnMViewMaintenance`；`option_test.rs` 验证常量值。 |

红灯：新增测试后，`cargo test --manifest-path pkg/kv/Cargo.toml --lib go_merge_4` 退出 101，编译器报告缺少 `ErrSharedLockLost`、`NewCoprRequestLimiter`、`NewQueryCopStoreLimiter`。修复后同一命令通过 8 项；补充选项测试后 `cargo test --manifest-path pkg/kv/Cargo.toml --lib` 通过 69 项，1 项既有测试忽略。`make lint`、`git diff --check`、仅针对本任务 Rust 文件的 `rustfmt --edition 2024 --check ...` 通过。`cargo check -p astersql-store-driver --message-format short` 和 `cargo test -p astersql-store-driver --lib --no-run --message-format short` 通过。

待回归原因：`cargo fmt --all -- --check` 退出 1，首个差异位于未改动的 `pkg/ddl/job_submitter_test.rs`，另有大量任务外文件格式差异；`cargo check -p astersql-session -p astersql-store-driver` 退出 101，`pkg/sessionctx/variable/session.rs` 引用当前 `RUV2Config` 不存在的字段（例如 `resource_manager_read_cnt`）。这两项不是本任务 Rust 改动产生的诊断，需在相关并行任务稳定后重跑。风险：`pkg/kv` API 已编译和测试，但端到端 cop RPC 限流与批处理行为须由任务 42/48 接线后验证；当前仅有局部 API 证据。正确性风险集中在跨包接线，兼容性风险集中在 `Request` 新字段的跨包构造点，性能风险为阻塞取消最多 10ms 的轮询延迟。

回归复查：在最新共享工作区重跑 `cargo fmt --all -- --check`，仍有 57 个格式差异，且无一位于本任务改动文件；`cargo check -p astersql-session -p astersql-store-driver --message-format short` 仍因 `pkg/sessionctx/variable/session.rs` 的 13 个 `RUV2Config` 未知字段错误失败。`cargo test --manifest-path pkg/kv/Cargo.toml --lib go_merge_4` 再次通过 8 项；`cargo test -p astersql-store-driver --lib --no-run --message-format short` 与 `git diff --check` 再次通过。状态继续保留为 `已完成，待回归`，等待这两项任务外全局门槛恢复后删除任务文件。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 4。

预计会话范围：7 个 Go 文件，合计 302 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/kv/checker.go`（+1/-1）
- Go 来源：`pkg/kv/checker_test.go`（+2/-0）
- Go 来源：`pkg/kv/error.go`（+2/-0）
- Go 来源：`pkg/kv/error_test.go`（+1/-0）
- Go 来源：`pkg/kv/kv.go`（+107/-4）
- Go 来源：`pkg/kv/kv_test.go`（+182/-0）
- Go 来源：`pkg/kv/option.go`（+2/-0）
- Rust 候选：`pkg/kv/checker.rs`
- Rust 候选：`pkg/kv/checker_test.rs`
- Rust 候选：`pkg/kv/error.rs`
- Rust 候选：`pkg/kv/error_test.rs`
- Rust 候选：`pkg/kv/kv.rs`
- Rust 候选：`pkg/kv/kv_test.rs`
- Rust 候选：`pkg/kv/option.rs`
- Cargo 包线索：`pkg/kv/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/kv/checker.go pkg/kv/checker_test.go pkg/kv/error.go pkg/kv/error_test.go pkg/kv/kv.go pkg/kv/kv_test.go pkg/kv/option.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。


## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_4` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/kv/Cargo.toml --lib go_merge_4`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/kv/Cargo.toml --lib go_merge_4`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 运行：`cargo test --manifest-path pkg/kv/Cargo.toml --lib go_merge_4`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

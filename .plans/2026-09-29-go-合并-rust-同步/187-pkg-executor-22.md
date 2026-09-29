# 任务 187: pkg/executor 第 22 组 Go 差异移植

批次：【批次 76】依赖：批次 75、`.plans/2026-09-29-ru-v3-直接移植剩余任务/10-旧调用迁移与回归.md` 完成

状态：进行中

调度修订（2026-09-29）：本文件在 Go 同步计划中仅作来源覆盖验收；RU v3 代码由专门的桥接计划及 RU v3 计划按 Go 函数合同实现。前序普通批次不等待本文件；外部计划完成后在本批次核对该来源文件的逐分支证据并按原规则关闭。

实施记录（2026-09-29）：按用户要求直接逐函数移植 Go `statement_ru_plan_walk.go`，不采用另一套 RU 计算路径。已新增 Rust `statement_ru_plan_walk.rs` 和独立测试，先实现 `statementRUWriteSnapshot` / `snapshotStatementRUWrites`，从现有 Rust `tikvutil::CommitDetails` 复制提交键数与字节数；空详情返回零值。该文件其余计划遍历、终端 finalization 和边界尚未移植，不得标记任务完成或删除。

验证进度：主工作区 `cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_197_scan_evidence_matches_go_validity_contract` 在无关 planner `FullJoin` 两处非穷尽匹配处失败。隔离工作树只为验证临时处理该匹配，首次编译到本组代码后发现 `WriteKeys`/`WriteSize` 实为 `u64`，`i64::from` 不适用。现改为与 Go `int64(uint64)` 同义的 `as i64`，并增加 `u64::MAX -> -1` 边界测试；正在重跑。

继续直接移植 Go `newStatementRUTerminalCalculator`：只有 root EOF 才创建 calculator；从 RUv2Metrics 读取语句级 TiKV coprocessor response bytes 一次，负值失败、bypass 忽略、full report 记入 TiKV/cop transport。相邻测试已加入。隔离工作树第一次 `cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_1` 8/8 通过（含本组写入快照），新终端计算器测试正在重跑。

继续直接移植 `statementRUOwner` 的首个结果 CAS、失败消耗、root EOF 与单次 terminal setup；并移植 `validateStatementRUFlatTree` 对连续深度优先子树的校验。隔离工作树定向 `cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_1` 11/11 通过。尚未移植完整算子遍历、生产 `ExecStmt` 挂载与发布，不能声称整个 Go 文件已覆盖。

继续移植 `statementRUSortWork`、四种单位累加及 `mergeStatementRUUnitDelta` 的整次提交、`mergeStatementRUOperatorState` 与终端失败原因分类。隔离定向测试最新为 14/14 通过。复杂跨文件后续步骤记录于 `ruv3-direct-port-execplan.md`。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 187。

预计会话范围：1 个 Go 文件，合计 1501 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/executor/statement_ru_plan_walk.go`（+1501/-0）
- Rust 候选：`pkg/executor/statement_ru_plan_walk.rs（候选，先用索引确认）`
- Cargo 包线索：`pkg/executor/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/executor/statement_ru_plan_walk.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。

- 此组含超大单文件差异。先按 Go 函数和行为列出小段及其 Rust 对应测试，逐段完成；生成文件应追溯生成器与输入。不得只实现其中一段就删除任务文件。

## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_187` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 运行：`cargo test --manifest-path pkg/executor/Cargo.toml --lib go_merge_187`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

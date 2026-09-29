# 任务 15: pkg/meta 第 3 组 Go 差异移植

批次：【批次 3】依赖：批次 2

状态：已完成，待回归

待回归原因：本任务的聚焦测试、包内测试、`pkg/ddl` 编译、格式检查、`make lint` 均通过；但 `cargo check --manifest-path pkg/session/Cargo.toml --quiet` 仍被合并中尚未完成的 FullJoin planner 移植挡住：`pkg/planner/core/logical_plan_builder_runtime.rs:4085,4242` 的 match 缺少 `JoinType::FullJoin` 分支（E0004），与本任务的 meta/model 改动无关。`pkg/util/parser` 相同枚举引起的编译缺口已按现有 SQL 恢复语义补齐并通过聚焦/包内测试；SEM 映射由任务 18 修复。planner 的完整 FullJoin 语义归后续 planner 任务，不以占位分支掩盖。待其落地后重跑 session 编译，再按本文件完成约定删除任务文件。

已运行检查：`~/.rustcodegraph/bin/rustcodegraph status` 显示索引存在；`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib go_merge_15` 初始为 0 项，新回归首次运行因 `may_need_reorg()` 返回 false 而按预期失败，修复后 6/6 通过；`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib --quiet` 为 129/129；`cargo check --manifest-path pkg/ddl/Cargo.toml --quiet`、`cargo fmt --all -- --check`、`make lint`、`git diff --check` 均通过。回归时新增 `pkg/util/parser/ast_test.rs` 的 FullJoin 测试，修复前因缺少分支编译失败；`cargo test --manifest-path pkg/util/parser/Cargo.toml --lib full_outer_join_restores_without_losing_join_kind` 修复后 1/1，`cargo test --manifest-path pkg/util/parser/Cargo.toml --lib --quiet` 为 16/16。`cargo check --manifest-path pkg/session/Cargo.toml --quiet` 先后暴露 util/parser、util/sem 和最终上述 planner E0004，尚未通过；未做真实 TiKV/集成测试。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 15。

预计会话范围：8 个 Go 文件，合计 719 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/meta/model/engine_attribute.go`（+93/-0）
- Go 来源：`pkg/meta/model/flags.go`（+2/-0）
- Go 来源：`pkg/meta/model/index.go`（+18/-0）
- Go 来源：`pkg/meta/model/job.go`（+103/-51）
- Go 来源：`pkg/meta/model/job_args.go`（+169/-8）
- Go 来源：`pkg/meta/model/job_args_test.go`（+219/-12）
- Go 来源：`pkg/meta/model/job_test.go`（+30/-2）
- Go 来源：`pkg/meta/model/reorg.go`（+6/-6）
- Rust 候选：`pkg/meta/model/engine_attribute.rs（候选，先用索引确认）`
- Rust 候选：`pkg/meta/model/flags.rs`
- Rust 候选：`pkg/meta/model/index.rs`
- Rust 候选：`pkg/meta/model/job.rs`
- Rust 候选：`pkg/meta/model/job_args.rs`
- Rust 候选：`pkg/meta/model/job_args_test.rs`
- Rust 候选：`pkg/meta/model/job_test.rs`
- Rust 候选：`pkg/meta/model/reorg.rs`
- Cargo 包线索：`pkg/meta/model/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/meta/model/engine_attribute.go pkg/meta/model/flags.go pkg/meta/model/index.go pkg/meta/model/job.go pkg/meta/model/job_args.go pkg/meta/model/job_args_test.go pkg/meta/model/job_test.go pkg/meta/model/reorg.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。


## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_15` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib go_merge_15`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib go_merge_15`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 运行：`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib go_merge_15`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

### 本次实施证据

- `engine_attribute.go` → 新增 `engine_attribute.rs`：空输入/非法 JSON/null 输入、RawMessage 原文、scope 判断、转换秒数与字符串；`flags.go` → `flags.rs` 的 TiKV 短路位；`index.go` → `index.rs` 的 HNSW kind 与 RegionSplitPolicy 时区。两个向量索引构造调用方同步写入 HNSW kind。
- `job.go` → `job.rs`：85–94 动作编号和动作名原已存在，经检查对齐；新增 RU JSON/克隆/proxy 传递、物化视图 reorg 与回滚边界、SubJob 涉及对象往返和 MultiSchemaInfo 执行期字段。`TimeZoneLocation` 原有 `Clone` 实现保留缓存并由新增测试验证。
- `job_args.go` → `job_args.rs`：新增物化视图创建/修改/切换与引擎属性参数，扩展影子建表和三类删除作业的 V1 解码，增加 TiFlash gate 与独立的 AutoPreSplit 字段。`internal/group2/lib.rs` 的参数表模型保留物化视图载荷及未知表字段，供任务 18 完善正式 `TableInfo` 前保持 V1/V2 无损往返；V1 null 值沿用 Go 零值语义。
- `job_args_test.go`、`job_test.go` → 独立的 `go_merge_15_test.rs`：覆盖上述 V1/V2 参数、旧版缺省字段、切换可选值、手动/自动拆分分离、RU 和作业状态。`reorg.go` 仅更新 `UseNewCollate` 注释的来源措辞，Rust `reorg.rs` 行为无需改动。
- 文件变更：`Cargo.lock`、`pkg/meta/model/{engine_attribute.rs,flags.rs,index.rs,job.rs,job_args.rs,go_merge_15_test.rs,lib.rs}`、`pkg/meta/model/internal/group1/{Cargo.toml,lib.rs}`、`pkg/meta/model/internal/group2/lib.rs`、`pkg/ddl/create_table.rs`、`pkg/session/runtime/ddl.rs`。未改 Go import、Go 测试、Bazel 或 Go module，故无需 `make bazel_prepare`。
- 延后回归时另修复 `pkg/util/parser/{ast.rs,ast_test.rs}` 的 FullJoin SQL 恢复分支，以解除本任务下游 session 检查遇到的第一个编译缺口；该修复与任务 18 的 SEM 映射以及后续 planner FullJoin 移植相互独立。
- 验证 profile：Ready 的适用检查已执行；session 下游编译受无关并行改动阻挡，保留待回归状态。正确性风险主要是正式 `TableInfo` 的物化视图类型仍由任务 18 补齐；本任务参数层已经保存其 JSON。兼容性风险由 V1/V2 回归覆盖；性能上仅新增 JSON 字段与参数编解码，无重型路径变化。

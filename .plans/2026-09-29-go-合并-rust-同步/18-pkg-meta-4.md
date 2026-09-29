# 任务 18: pkg/meta 第 4 组 Go 差异移植

批次：【批次 4】依赖：批次 3

状态：已完成，待回归

待回归原因：任务 18 的聚焦测试、包内测试、DDL 下游编译、格式检查、`make lint` 和差异检查已通过。共享 parser 的 `FullJoin` 编译缺口已由任务 15 修复；最新 `cargo check --manifest-path pkg/session/Cargo.toml --quiet` 继续编译到 planner，但 `pkg/planner/core/logical_plan_builder_runtime.rs:4085,4242` 的 match 缺少 `JoinType::FullJoin` 分支（E0004）。Go 的 FullJoin 涉及完整计划语义，不能用占位分支掩盖；该移植属于后续 planner 任务，与本任务的 meta 行为无关。待其落地后重跑 session 编译，再按本文件完成约定删除任务文件。

已运行检查：`~/.rustcodegraph/bin/rustcodegraph status`（索引存在）；`cargo test --manifest-path pkg/meta/Cargo.toml --lib go_merge_18` 初次为 0 项，故改用实际模型包验证；`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib go_merge_18` 首次因 TableInfo/PartitionDefinition 缺少 StorageClass 字段和方法而按预期编译失败，修复后 3/3 通过；meta 包对应 Reader 回归 1/1 通过；`cargo test --manifest-path pkg/meta/model/Cargo.toml --lib --quiet` 为 132/132，`cargo test --manifest-path pkg/meta/Cargo.toml --lib --quiet` 为 35/35；`cargo check --manifest-path pkg/ddl/Cargo.toml --quiet`、`cargo fmt --all -- --check`、`make lint`、`git diff --check` 均通过。回归阶段新增 SEM 对 `SHOW STORAGE_CLASS TRANSITIONS` 的映射，`cargo test --manifest-path pkg/util/sem/v2/Cargo.toml --lib go_merge_18_show_storage_class_transitions_command` 在缺少分支时编译失败，修复后 1/1 通过。任务 15 完成 parser 修复后重跑 `cargo check --manifest-path pkg/session/Cargo.toml --quiet`，现在因上述 planner E0004 失败；未运行真实 TiKV 或 SQL 集成测试。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 18。

预计会话范围：3 个 Go 文件，合计 304 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

## 文件

- Go 来源：`pkg/meta/model/table.go`（+230/-9）
- Go 来源：`pkg/meta/model/table_test.go`（+64/-0）
- Go 来源：`pkg/meta/reader.go`（+1/-0）
- Rust 候选：`pkg/meta/model/table.rs`
- Rust 候选：`pkg/meta/model/table_test.rs`
- Rust 候选：`pkg/meta/reader.rs`
- Cargo 包线索：`pkg/meta/Cargo.toml`、`pkg/meta/model/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/meta/model/table.go pkg/meta/model/table_test.go pkg/meta/reader.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。


## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_18` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/meta/Cargo.toml --lib go_merge_18`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/meta/Cargo.toml --lib go_merge_18`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 运行：`cargo test --manifest-path pkg/meta/Cargo.toml --lib go_merge_18`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。

### 本次实施证据

- `table.go` → `table.rs`：TableInfo 与 PartitionDefinition 加入存储层级、迁移规则及格式化方法；TableInfo 加入引擎属性和四类物化视图元数据；补齐构建状态消息、日志阈值、Unicode 表名截断、TTL starter 间隔。Rust `Clone` 对 Vec 和 Option 载荷深拷贝；把已有 TimeZoneLocation 从 Job 层统一为 group1 正式身份，供 Job 和 TableInfo 共用。`table_4_aster_unit_test.rs` 的显式分区构造同步新字段。
- `table_test.go` → 独立的 `go_merge_18_test.rs`：覆盖 after_seconds 为 0/17 时的 JSON、表和分区克隆、物化视图 JSON/克隆/时区缓存、构建状态各分支、日志阈值边界、Unicode 名字截断及 15 分钟 TTL 间隔。初次模型包回归在生产字段未补齐时按预期编译失败，修复后通过。
- `reader.go` → `reader.rs`：Reader trait 新增 starter bootstrap 读取并委托 Mutator 已存在实现；`meta_test.rs` 从 trait object 验证真实读取路径。任务文件给出的顶层 meta 包过滤命令只运行 0 个测试，已额外运行模型包过滤命令。
- 本任务修改：`pkg/meta/model/{table.rs,go_merge_18_test.rs,table_4_aster_unit_test.rs,lib.rs,job.rs,internal/group3/lib.rs}`、`pkg/meta/{reader.rs,meta_test.rs}`，以及回归阶段的 `pkg/util/sem/v2/{sql_rule.rs,sql_rule_test.rs}`。`job.rs` 与 `lib.rs` 原有未提交的任务 15 变更保留；`pkg/util/parser` 的共享修复归任务 15。未修改 Go import、Go 测试、Bazel 或 Go module，无需 `make bazel_prepare`。
- 验证 profile：Ready；`make lint` 与相关 Rust 测试、格式和差异检查完成。正确性与兼容性主要风险是 session 下游编译尚无通过证据；物化视图时区加载仍沿用该仓库此前的简化时区设施。性能上仅新增元数据字段与克隆，主要影响随元数据大小线性增长；分区 InValues 的 Rust 克隆策略沿用既有实现。

# v0.0.2 到 v0.0.3 Go 逐提交严格对齐 Rust 计划

目标：逐条核对并对齐 Git 差集 `v0.0.2..v0.0.3` 中 70 个来源提交，使当前 Rust 实现保留每个 Go 增量的真实行为、接线与测试意图。

范围：来源集合定义为“可从 `v0.0.3` 到达、但不可从 `v0.0.2` 到达”的提交，已由 `git rev-list --count v0.0.2..v0.0.3` 验证为 70 条。计划覆盖每条提交的完整 SHA、父 SHA、实际路径、关键 Go 差异、当前 Rust 候选入口、独立 Rust 测试、Cargo 所有者与最小验证；每条提交只补自身缺失或偏离及不可缺少的局部接线。

范围外：不递归补建完整子系统，不吞并区间外提交，不修复无关既有缺口，不改写已等价 Rust，不在本计划阶段修改代码或 Git 历史。

假设：`v0.0.2` 为 `b667d4d4f4091d8002d960bcdf13827373611d52`，`v0.0.3` 为 `dfda1fd3e83f73fcccb536fb3eeead54735de7b1`，共同祖先为 `4b2221cddfd81aae904573ab6a5c15e6c8351e6e`；因此这里使用精确 Git 可达差集，不声称两个 tag 是简单祖先关系。当前 RustCodeGraph 索引覆盖 Go/YAML，不覆盖 Rust，Rust 入口由同目录实现、Cargo manifest 与 `rg` 结果交叉确认。

## 设计决策

- 一条来源 commit 对应一个永久保留的编号任务；大提交在任务内部按行为分段，不拆成额外任务，也不把后续替代实现倒灌进早期任务。
- diagnostic mode、采样 NDV、add-index/global-sort、materialized view、paging 预扣费、server packet I/O 与 BR restore 是已确认的跨提交演进族；执行者仍只实现当前来源 SHA 的增量，并在覆盖记录中注明后续替代 SHA。
- 全部可能修改 Rust 的任务保守串行：同一工作树中的 crate 源码、workspace manifest/lock、`cargo fmt --all` 与 `make lint` 是共享写入或验证资源。此排序依据资源与真实候选重叠，不把 Git 父子关系本身当作业务依赖。
- 纯设计文档或所有者元数据提交先做 Rust 影响判定；没有行为影响时以 Git 证据和当前 Rust 搜索结果完成，不强造 Rust 代码或空测试。
- 外部 Rust 依赖若确为本提交不可缺少，必须在独立上游仓库移植、提交、打 tag，再由所有 Cargo manifest 统一引用该 tag；禁止 vendor、third_party、本地复制和 `[patch]` 本地覆盖。

## 架构约束

- Rust 源码与单元测试保持分文件；修复生产逻辑时同步修复同目录独立测试，条件顺序、错误、数据形状、状态、事务、并发、副作用与 Go 测试分支不得简化。
- DDL 对齐遵循 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`：区分 job/metadata fast path，保留重组、回滚、schema version、持久化和 owner/scheduler 语义。
- DXF 对齐遵循 `pkg/dxf/framework/doc.go` 的 task/subtask 状态机、owner 调度与清理责任；统计链保留 FM sketch 的持久化、抽样估算和分区合并顺序；BR 链保留 snapshot/PiTR 元数据与恢复目标命名的一致性。
- Rust 生产文件保留 PingCAP Apache License；真正可用后在顶部加入 `// Copyright 2026 AsterSQL.`。完全自建且非 Go 复刻的文件只遵循仓库对自建文件的版权规则。

## 对齐原则

- 先检查当前 Rust 是否已经等价：等价时不人为回退制造红灯，只运行当前绿灯并记录入口、输入、输出、退出码与有效测试数。
- 行为缺失或偏离时，先在独立 Rust 测试文件增加能真实失败的回归，再做最小生产修复，取得红→绿证据；编译成功、零测试、全部 ignored 或桩实现均不能作为完成证据。
- Go 既有回归失败可记录为来源基线限制，不阻塞已取得 Rust 行为证据的任务；无关缺口仅写入任务的“范围外建议”。
- 后续 commit 撤销或替代早期行为时，覆盖记录写完整后续 SHA、源码去向和仍保留行为，不恢复已撤销逻辑，也不吸收后续提交的全部实现。

## 总体验证策略

- 来源证据统一使用带真实完整 SHA 的 `git show --stat --oneline`、`git diff` 与 `git diff-tree --name-status`，先证明任务边界；各编号任务已经写入可直接执行的确切命令。
- Rust 验证使用任务列出的真实 `Cargo.toml`、测试过滤条件和 feature；领取仓库共享 Cargo 槽位后运行聚焦测试，要求至少一个有效测试执行。生产接线变化追加对应 crate 的 `cargo check` 或实际 NextGen 编译。
- Rust 修改后先运行 `cargo fmt --all`，再运行 `cargo fmt --all -- --check`；Ready 交付还运行 `make lint` 与 `git diff --check`。只有来源 Go 文件/导入、Go 依赖或 Bazel 元数据也被改动时才触发 `make bazel_prepare`，而这些通常不属于 Rust 对齐任务范围。
- 涉及 Go failpoint、integrationtest 或 RealTiKV 来源测试时，执行者必须按 `docs/agents/testing-flow.md` 选最小对应验证，并完整清理 failpoint/playground；不得运行 `make bazel_lint_changed`。


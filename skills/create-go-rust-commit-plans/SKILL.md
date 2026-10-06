---
name: create-go-rust-commit-plans
description: 当需要把 Go 提交历史逐提交规划为严格对齐 Rust 的任务，并且只生成 plan.md、prompt.md 和编号任务 Markdown 时使用。
---

# 创建 Go 逐提交严格对齐 Rust 计划

把用户指定的 Go commit 集合拆成“一条 commit 对应一个 Rust 对齐任务”的中文计划。只创建计划，不修改代码或 Git 历史。

开始时说明：`我正在使用 create-go-rust-commit-plans 技能创建 Go 逐提交严格对齐 Rust 计划。`

## 准备

完整阅读 `./skills/create-task-plans/SKILL.md`、根 `AGENTS.md` 和 `PLANS.md`。沿用通用技能的计划、批次、Cargo 共享槽位和自审要求；冲突时以本技能和 `AGENTS.md` 为准。

读取用户指定的提交范围，以及目标模块的 `doc.go`（存在时）、Go/Rust 实现、接线、独立 Rust 测试和 Cargo manifest。DDL 任务额外读取 `docs/agents/ddl/README.md`。代码导航优先使用 `./skills/rustcodegraph/SKILL.md`；Git 差异、文档、配置和未索引内容使用 `git`、`rg` 与文件读取。

## 唯一允许的输出

目标计划目录中只创建：

```text
plan.md
prompt.md
1-提交-<短SHA>-严格对齐.md
2-提交-<短SHA>-严格对齐.md
...
```

禁止生成其他文件，包括依赖图、冲突图、可视化、事实缓存、来源清单、调度数据、脚本、README，以及 `.json`、`.tsv`、`.mmd`、`.html`、`.py`、`.dot`、`.svg`、`.DS_Store`。目标目录已有其他文件时不删除、不覆盖；保留用户和其他任务的改动。

## 提交与范围

1. 用 Git 验证完整 SHA、父提交、标题、拓扑顺序和实际 diff。合并差集必须明确父提交、共同祖先或可达集合；含义不明确时先询问。
2. 一条来源 commit 对应一个编号任务。大提交只在原任务内按行为分段；非 Go commit 也核查 Rust、生成链、构建元数据和测试影响，无影响时不强造代码。
3. 每个任务记录完整来源 SHA、父 SHA、标题、全部变更路径和关键 Go 差异入口。

每个任务只负责该 commit 的增量、不可缺少的最小局部接线和原测试意图：

- 先核对当前 Rust，只补缺失或偏离，不重写等价实现。
- 每项生产修改必须映射到具体 Go diff，或证明是验证该增量不可缺少的局部接线。
- 禁止递归补建完整子系统、其他 commit 或通用基础设施。
- 严格保留 Go 的条件顺序、错误、数据形状、状态、事务、并发、副作用和测试分支；禁止桩、空样例、仅编译通过或删减版实现。
- 后续 commit 撤销或替代行为时，记录完整 SHA、源码去向和保留行为；不恢复已撤销逻辑，也不吞并替代 commit。
- 无关既有缺口只写入“范围外建议”，不规划实现，也不阻塞当前任务。
- 外部 Rust 依赖按 `AGENTS.md` 在独立上游移植、提交并打 tag；禁止 vendor、third_party、本地复制和 `[patch]` 本地覆盖。

## 三类文件

`plan.md` 只写共享目标、精确来源范围、范围外、已验证假设、设计决策、架构约束、对齐原则和总体验证策略。它是只读指南，不写任务清单、步骤、依赖图或进度。

编号任务是唯一进度来源，只使用 `未开始`、`进行中`、`已完成`、`已阻塞`。完成后保留任务作为覆盖证据；除非用户明确要求，不规划 Git 提交。

`prompt.md` 与编号任务一一对应，按安全批次提供可复制的中文提示。每条提示要求使用 `./skills/do-task-plans/SKILL.md`，先读只读 `plan.md` 和编号任务，只处理单 commit 范围，维护覆盖记录，按证据更新状态，并报告文件、验证 profile、风险、命令、退出码、测试数量和未验证项。

Rust/Cargo 任务按 `create-task-plans` 的格式，在编号任务内写完整“Cargo 共享槽位规则”，对应提示只写读取该规则的短句。

## 编号任务结构

每个 `N-提交-<短SHA>-严格对齐.md` 使用以下自包含结构：

```markdown
# 任务 N: 提交 <短SHA> 严格对齐 Rust

批次：【批次 N】；依赖批次：<批次或无>
状态：未开始
目的：<本提交增量的 Rust 对齐目标>
来源：Go commit `<完整SHA>`；父提交 `<完整SHA>`；标题：<原始标题>
预计会话范围：<单提交边界>

## 提交对齐范围硬约束
## 文件
## 上下文
## Go 变更定位清单
## Cargo 共享槽位规则（适用时）
## 测试计划
## 步骤
## 验证
## 完成
## Go 到 Rust 覆盖记录
## 范围外建议
## Progress（进度）
## Surprises & Discoveries（发现）
## Decision Log（决策）
## Outcomes & Retrospective（结果）
```

“文件”列出全部 Go 路径、构建/生成元数据，以及搜索确认的 Rust 候选、独立测试和 Cargo manifest；候选不等于必须修改。“Go 变更定位清单”来自真实 diff。“测试计划”覆盖全部生产变化和 Go 测试意图。“验证”包含确切来源命令、聚焦测试、Ready 命令和成功信号。“覆盖记录”逐项填写：Go 行为 → Rust 真实入口 → 已有/修复/后续替代 SHA → 测试输入输出、命令、退出码和数量。

Rust 单元测试必须规划在独立测试文件并尽量等价于 Go 测试。修改 Rust 生产代码时保留 PingCAP Apache License，真正可用后按 `AGENTS.md` 添加 `// Copyright 2026 AsterSQL.`；Rust 改动后先运行 `cargo fmt --all`。

## 批次与验证

- Git 父子顺序本身不是 Rust 业务依赖。共享候选写入文件按来源顺序串行；同批任务不得争用源码、manifest/lock、生成输出、failpoint、playground、格式化或 lint 窗口。
- 每批最多 10 个任务。必须核查真实接口、行为依赖和共享资源；不能证明可并行就提高批次，只在任务内写文字依据。
- Bug fix 或行为变化规划真实回归测试和红→绿证据；Rust 已等价时说明红灯不适用并取得当前绿灯，不人为回退。
- 只选该 commit 的最小测试面和适用交付检查。按仓库规则判断 failpoint、集成记录、RealTiKV 和 Bazel prepare；除非用户要求，禁止 `make bazel_lint_changed`。
- 代码 Ready 默认包含聚焦测试、`cargo fmt --all`、`cargo fmt --all -- --check`、`make lint`、`git diff --check`；生产接线变化再加实际 NextGen 编译。纯资料或无 Rust 影响任务只选适用检查。
- 命令必须对应实际 manifest、package、test target 和 feature。零测试、全部 ignored、编译失败或桩实现不算行为证据。

## 自我审查与交接

- 来源 commit 数量、编号任务数量、完整 SHA、父 SHA、标题和路径与当前 Git 证据一致。
- 每个 Go diff 分段和原测试入口都有对应任务，没有吞并其他 commit 或遗漏非 `.go` 元数据影响。
- `plan.md` 无任务进度；`prompt.md` 覆盖每个任务，路径、批次和依赖一致。
- 每个任务包含真实 Rust 候选、独立测试、Cargo 所有者、红绿或当前绿证据、Ready 检查和完成门槛。
- 同批任务无已知写入、能力或共享资源冲突；无占位符、虚构路径、符号或命令。
- 新生成或修改的文件只有 `plan.md`、`prompt.md` 和编号任务 `.md`。

最后简短报告计划目录、commit 数、任务数、批次数和实际创建/更新的允许文件；明确未生成依赖图、脚本或其他附加产物，并推荐按 `prompt.md` 执行。

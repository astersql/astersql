---
name: git-commit
description: 用户要求整理或提交当前 Git 变更时，分析 staged、unstaged 和 untracked 内容，制定必要的拆分计划，并在每次 commit 前征得明确确认。
---

# Git 提交助手

先看清变更，再按单一目的组织提交。默认使用 Conventional Commits；未经用户明确要求，不 push、amend、rebase 或修改代码。

## 流程

1. 检查 `git status --short --branch`、`git diff`、`git diff --cached`、未跟踪文件内容及 `git log --oneline -10`。工作区干净则报告；存在冲突或未完成的 Git 操作则先说明情况。
2. 概括各文件的改动、关联和风险。发现密钥、`.env`、凭证、个人路径、大文件或无关改动时，标出并默认排除，必要时请用户决定。
3. 按功能或目的分组；一句话难以概括的独立变更建议拆开。同一功能的代码、测试和文档可以合并。列出每条提交的完整信息、文件及简短理由，并说明剩余变更。
4. 每条提交执行前都等待用户明确确认（可修改、跳过或取消）。仅确认后按计划暂存对应内容；涉及同一文件的不同分组时按 hunk 暂存。用 `git diff --cached` 和 `git diff --cached --check` 核对实际提交内容，再执行 `git commit`。若暂存内容与已确认计划不符，重新确认。
5. 每次提交后报告短 hash、标题和 `git status --short --branch`；继续下一条时再次确认。

提交信息格式：`<type>(<scope>): <subject>`。使用恰当的 `feat`、`fix`、`docs`、`refactor`、`test`、`chore` 等类型；scope 可省略。标题简短、用祈使句、无句号；正文只补充原因、影响或迁移注意事项。不加 AI 署名。

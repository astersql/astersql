---
name: git-commit
description: 用户要求整理或提交当前 Git 变更时，分析 staged、unstaged 和 untracked 内容，必要时拆分，并直接完成提交。
---

# Git 提交助手

先看清变更，再按单一目的组织提交。默认使用 Conventional Commits；未经用户明确要求，不 push、amend、rebase 或修改代码。

## 流程

1. 检查 `git status --short --branch`、`git diff`、`git diff --cached`、未跟踪文件内容及 `git log --oneline -10`。工作区干净则报告；存在冲突或未完成的 Git 操作则先说明情况。
2. 概括各文件的改动、关联和风险。发现密钥、`.env`、凭证、个人路径、大文件或无关改动时，标出并默认排除，必要时请用户决定。
3. 按功能或目的分组；一句话难以概括的独立变更建议拆开。同一功能的代码、测试和文档可以合并。确定每条提交的信息、文件及简短理由。
4. 按分组暂存对应内容；涉及同一文件的不同分组时按 hunk 暂存。用 `git diff --cached` 和 `git diff --cached --check` 核对实际内容后直接执行 `git commit`，无需逐条确认。
5. 完成后报告各提交的短 hash、标题、文件和 `git status --short --branch`，说明剩余变更。

提交信息格式：`<type>(<scope>): <subject>`。使用恰当的 `feat`、`fix`、`docs`、`refactor`、`test`、`chore` 等类型；scope 可省略。标题简短、用祈使句、无句号；正文只补充原因、影响或迁移注意事项。不加 AI 署名。

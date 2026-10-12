# Rust 大块注释残留分析任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

# 批次 1

## 任务 1: 建立残留注释候选全集【批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/1-jian-li-hou-xuan-quan-ji.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 2

## 任务 2: 分析 Domain、Session 与 TTL 残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/2-domain-session-ttl.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 3: 分析 Planner 残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/3-planner.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 4: 分析 Executor 残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/4-executor.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 5: 分析 Expression 与 Statistics 残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/5-expression-statistics.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 6: 分析 DDL、Table、Store 与基础存储残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/6-ddl-table-store.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 7: 分析 BR、Lightning 与 Dumpling 残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/7-br-lightning-dumpling.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 8: 分析其余路径残留【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/8-qi-ta-lu-jing.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 3

## 任务 9: 汇总审计与处置建议【批次 3】依赖批次 2

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/9-hui-zong-yu-chu-zhi-jian-yi.md`。

开始前先阅读 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划执行，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

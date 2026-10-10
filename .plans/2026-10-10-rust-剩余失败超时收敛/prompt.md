# Rust 剩余失败与超时收敛任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md`、`execplan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不要修改。

# 批次 1

## 任务 1: 重建基线与分类【批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/1-重建基线与分类.md`。开始前阅读 plan.md、execplan.md 和任务文件，完成后更新任务状态与 execplan.md，使用 $git-commit 提交可提交记录。本任务涉及 Rust/Cargo：执行 Cargo 前遵守任务文件中的“Cargo 共享槽位规则”，动态领取槽位 1–10并报告实际路径。
```

# 批次 2（任务 2–7 与任务 10 可并行）

## 任务 2: Session Domain 与 Starter【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/2-session-domain与starter.md`。先阅读 plan.md、execplan.md 和任务文件，只处理本任务范围；完成后更新状态和 execplan.md 并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

## 任务 3: Bootstrap 升级回归【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/3-bootstrap升级回归.md`。先阅读 plan.md、execplan.md 和任务文件；按失败/通过证据实施，完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

## 任务 4: Session Schema Checker【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/4-session-schema-checker.md`。先阅读 plan.md、execplan.md 和任务文件；完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

## 任务 10: Global Stats 性能【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/10-global-stats性能.md`。先阅读 plan.md、execplan.md 和任务文件；完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

## 任务 5: SST transport 取消【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/5-sst-transport取消语义.md`。先阅读 plan.md、execplan.md 和任务文件；完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

## 任务 6: Timer panic 恢复【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/6-timer-panic恢复.md`。先阅读 plan.md、execplan.md 和任务文件；完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

## 任务 7: Profile 与 TraceEvent【依赖批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/7-profile与traceevent.md`。先阅读 plan.md、execplan.md 和任务文件；完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

# 批次 3

## 任务 8: RealTiKV DDL 与 Paging【依赖批次 1、2】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/8-realtikv-ddl与paging.md`。先阅读 plan.md、execplan.md 和任务文件；独占并清理 playground，完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

# 批次 4

## 任务 9: RealTiKV Import 与契约【依赖任务 8】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-10-rust-剩余失败超时收敛/9-realtikv-import与契约.md`。先阅读 plan.md、execplan.md 和任务文件；使用干净 playground 并完整清理，完成后更新状态并用 $git-commit 提交。本任务涉及 Rust/Cargo，必须遵守任务文件的 Cargo 共享槽位规则。
```

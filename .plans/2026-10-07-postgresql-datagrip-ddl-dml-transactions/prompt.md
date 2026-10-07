# PostgreSQL DataGrip DDL、DML 与事务完善任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

# 批次 1

## 任务 1: 冻结 DataGrip 字段生命周期回归【批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/1-冻结字段生命周期回归.md`。

开始前先阅读 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/1-冻结字段生命周期回归.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 2

## 任务 2: 适配 PostgreSQL 常用建表语法【批次 2】依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/2-适配postgresql建表语法.md`。

开始前先阅读 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/2-适配postgresql建表语法.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 3

任务 3 与任务 4 没有文件写入冲突，可在批次 2 完成后并行执行。

## 任务 3: 适配字段变更语法与目录刷新【批次 3】依赖批次 2

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/3-适配字段变更语法与目录刷新.md`。

开始前先阅读 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/3-适配字段变更语法与目录刷新.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 4: 完善 PostgreSQL 参数化 CRUD【批次 3】依赖批次 2

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/4-完善参数化crud.md`。

开始前先阅读 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/4-完善参数化crud.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 4

## 任务 5: 补齐事务失败状态与恢复【批次 4】依赖批次 3

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/5-补齐事务失败状态与恢复.md`。

开始前先阅读 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/5-补齐事务失败状态与恢复.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 5

## 任务 6: 真实 TiKV 与 DataGrip 交付验收【批次 5】依赖批次 4

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/6-真实tikv与datagrip交付验收.md`。

开始前先阅读 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/6-真实tikv与datagrip交付验收.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-07-postgresql-datagrip-ddl-dml-transactions/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

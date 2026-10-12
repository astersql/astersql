# Rust failpoint 切换 AsterSQL 路径任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

# 批次 1

## 任务 1: 迁移 stale-read 时间注入【批次 1】 无

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/1-迁移-stale-read-时间注入.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/1-迁移-stale-read-时间注入.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 3: 迁移 executor、planner 与 statistics failpoint【批次 1】 无

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/3-迁移-executor-planner-statistics-failpoint.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/3-迁移-executor-planner-statistics-failpoint.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 4: 迁移 DDL、DXF、BR 与服务 failpoint【批次 1】 无

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/4-迁移-ddl-dxf-br-服务-failpoint.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/4-迁移-ddl-dxf-br-服务-failpoint.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 2

## 任务 2: 迁移 session 其余 failpoint【批次 2】 依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/2-迁移-session-其余-failpoint.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/2-迁移-session-其余-failpoint.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 6: 迁移 RealTiKV DDL failpoint【批次 2】 依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/6-迁移-realtikv-ddl-failpoint.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/6-迁移-realtikv-ddl-failpoint.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

## 任务 7: 迁移 RealTiKV BR 与 Import Into failpoint【批次 2】 依赖批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/7-迁移-realtikv-br-import-failpoint.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/7-迁移-realtikv-br-import-failpoint.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 3

## 任务 5: 迁移 RealTiKV 事务与会话 failpoint【批次 3】 依赖批次 2

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/5-迁移-realtikv-事务与会话-failpoint.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/5-迁移-realtikv-事务与会话-failpoint.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 4

## 任务 8: 全仓审计与交付验证【批次 4】 依赖批次 2,3

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-rust-failpoint-astersql-路径/8-全仓审计与交付验证.md`。

开始前先阅读 `.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-rust-failpoint-astersql-路径/8-全仓审计与交付验证.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 $git-commit 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 $git-commit 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-rust-failpoint-astersql-路径/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```


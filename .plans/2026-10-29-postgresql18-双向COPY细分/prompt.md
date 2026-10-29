# PostgreSQL18 双向COPY任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

线性顺序为1至31，每批仅一个任务，无可并行批次；未满足接口依赖时保留阻塞，不递归扩大范围。

# 批次 1

## 任务 1：COPY语法与拒绝边界【批次 1】 依赖批次：无

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/1-COPY语法与拒绝边界.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次1及依赖批次无已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/1-COPY语法与拒绝边界.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 2

## 任务 2：text输入真实写入闭环【批次 2】 依赖批次：1

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/2-text输入真实写入闭环.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次2及依赖批次1已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/2-text输入真实写入闭环.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 3

## 任务 3：text输入跨帧与转义【批次 3】 依赖批次：2

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/3-text输入跨帧与转义.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次3及依赖批次2已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/3-text输入跨帧与转义.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 4

## 任务 4：text输入选项与编码【批次 4】 依赖批次：3

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/4-text输入选项与编码.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次4及依赖批次3已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/4-text输入选项与编码.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 5

## 任务 5：输入默认值约束与事务原子性【批次 5】 依赖批次：4

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/5-输入默认值约束与事务原子性.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次5及依赖批次4已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/5-输入默认值约束与事务原子性.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 6

## 任务 6：输入扩展协议与错误恢复【批次 6】 依赖批次：5

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/6-输入扩展协议与错误恢复.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次6及依赖批次5已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/6-输入扩展协议与错误恢复.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 7

## 任务 7：输入真实对象与权限门禁【批次 7】 依赖批次：6

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/7-输入真实对象与权限门禁.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次7及依赖批次6已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/7-输入真实对象与权限门禁.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 8

## 任务 8：CSV输入跨帧记录【批次 8】 依赖批次：7

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/8-CSV输入跨帧记录.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次8及依赖批次7已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/8-CSV输入跨帧记录.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 9

## 任务 9：CSV输入选项【批次 9】 依赖批次：8

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/9-CSV输入选项.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次9及依赖批次8已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/9-CSV输入选项.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 10

## 任务 10：binary输入合法流与类型【批次 10】 依赖批次：9

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/10-binary输入合法流与类型.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次10及依赖批次9已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/10-binary输入合法流与类型.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 11

## 任务 11：binary输入非法流与结束边界【批次 11】 依赖批次：10

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/11-binary输入非法流与结束边界.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次11及依赖批次10已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/11-binary输入非法流与结束边界.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 12

## 任务 12：text输出真实表读取【批次 12】 依赖批次：11

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/12-text输出真实表读取.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次12及依赖批次11已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/12-text输出真实表读取.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 13

## 任务 13：输出SELECT来源与列投影【批次 13】 依赖批次：12

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/13-输出SELECT来源与列投影.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次13及依赖批次12已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/13-输出SELECT来源与列投影.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 14

## 任务 14：text输出选项与呈现【批次 14】 依赖批次：13

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/14-text输出选项与呈现.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次14及依赖批次13已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/14-text输出选项与呈现.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 15

## 任务 15：输出扩展协议完成边界【批次 15】 依赖批次：14

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/15-输出扩展协议完成边界.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次15及依赖批次14已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/15-输出扩展协议完成边界.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 16

## 任务 16：输出真实SELECT授权【批次 16】 依赖批次：15

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/16-输出真实SELECT授权.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次16及依赖批次15已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/16-输出真实SELECT授权.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 17

## 任务 17：COPY简单多语句接续【批次 17】 依赖批次：16

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/17-COPY简单多语句接续.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次17及依赖批次16已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/17-COPY简单多语句接续.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 18

## 任务 18：CSV输出内容与选项【批次 18】 依赖批次：17

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/18-CSV输出内容与选项.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次18及依赖批次17已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/18-CSV输出内容与选项.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 19

## 任务 19：binary输出头尾与字段【批次 19】 依赖批次：18

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/19-binary输出头尾与字段.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次19及依赖批次18已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/19-binary输出头尾与字段.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 20

## 任务 20：输出事务与语句快照【批次 20】 依赖批次：19

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/20-输出事务与语句快照.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次20及依赖批次19已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/20-输出事务与语句快照.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 21

## 任务 21：输出读取编码失败恢复【批次 21】 依赖批次：20

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/21-输出读取编码失败恢复.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次21及依赖批次20已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/21-输出读取编码失败恢复.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 22

## 任务 22：输入活动取消与完成竞态【批次 22】 依赖批次：21

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/22-输入活动取消与完成竞态.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次22及依赖批次21已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/22-输入活动取消与完成竞态.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 23

## 任务 23：输出活动读取和发送取消【批次 23】 依赖批次：22

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/23-输出活动读取和发送取消.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次23及依赖批次22已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/23-输出活动读取和发送取消.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 24

## 任务 24：输入坏帧断连与停服清理【批次 24】 依赖批次：23

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/24-输入坏帧断连与停服清理.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次24及依赖批次23已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/24-输入坏帧断连与停服清理.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 25

## 任务 25：输出断连与写失败清理【批次 25】 依赖批次：24

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/25-输出断连与写失败清理.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次25及依赖批次24已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/25-输出断连与写失败清理.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 26

## 任务 26：输入资源预算准入【批次 26】 依赖批次：25

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/26-输入资源预算准入.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次26及依赖批次25已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/26-输入资源预算准入.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 27

## 任务 27：输出背压与资源上限【批次 27】 依赖批次：26

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/27-输出背压与资源上限.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次27及依赖批次26已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/27-输出背压与资源上限.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 28

## 任务 28：libpq双向COPY差分【批次 28】 依赖批次：27

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/28-libpq双向COPY差分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次28及依赖批次27已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/28-libpq双向COPY差分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 29

## 任务 29：psql脚本与客户端文件COPY【批次 29】 依赖批次：28

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/29-psql脚本与客户端文件COPY.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次29及依赖批次28已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/29-psql脚本与客户端文件COPY.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 30

## 任务 30：JDBC双向COPY客户端【批次 30】 依赖批次：29

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/30-JDBC双向COPY客户端.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次30及依赖批次29已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/30-JDBC双向COPY客户端.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 31

## 任务 31：RealTiKV持久化与双端回读【批次 31】 依赖批次：30

```text
请使用技能 /Users/xiangmin/.codex/skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/31-RealTiKV持久化与双端回读.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 和该任务文件，确认当前批次31及依赖批次30已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围，不得递归扩大范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/31-RealTiKV持久化与双端回读.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。不得跳过验证或阻塞记录，不得提交无关变更。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-双向COPY细分/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# PostgreSQL18函数注册重载与双方言隔离任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

线性顺序：1→2→3→4→5→6→7→8→9→10→11→12→13→14→15→16→17→18→19→20→21→22→23→24→25→26→27。每批一个任务；共享文件存在冲突，没有可并行实施批次。依赖按任务文件，前一批还承担文件互斥屏障。不得同时执行本目录任务。

# 批次 1

## 任务 1: 精确签名与正式工厂贯通【批次 1】依赖无

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/1-精确签名与正式工厂贯通.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次1的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/1-精确签名与正式工厂贯通.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 2

## 任务 2: 连接方言与限定名可见性【批次 2】依赖批次 1（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/2-连接方言与限定名可见性.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次2的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/2-连接方言与限定名可见性.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 3

## 任务 3: 缺失工厂与未知函数区分【批次 3】依赖批次 2（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/3-缺失工厂与未知函数区分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次3的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/3-缺失工厂与未知函数区分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 4

## 任务 4: 隐式转换候选顺序【批次 4】依赖批次 3（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/4-隐式转换候选顺序.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次4的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/4-隐式转换候选顺序.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 5

## 任务 5: 未知字符串与空值候选筛选【批次 5】依赖批次 4（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/5-未知字符串与空值候选筛选.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次5的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/5-未知字符串与空值候选筛选.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 6

## 任务 6: 歧义拒绝与隔离候选【批次 6】依赖批次 5（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/6-歧义拒绝与隔离候选.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次6的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/6-歧义拒绝与隔离候选.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 7

## 任务 7: 可变参数候选展开与优先级【批次 7】依赖批次 6（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/7-可变参数候选展开与优先级.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次7的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/7-可变参数候选展开与优先级.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 8

## 任务 8: 默认参数候选与歧义边界【批次 8】依赖批次 7（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/8-默认参数候选与歧义边界.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次8的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/8-默认参数候选与歧义边界.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 9

## 任务 9: 预处理签名与参数身份固定【批次 9】依赖批次 8（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/9-预处理签名与参数身份固定.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次9的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/9-预处理签名与参数身份固定.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 10

## 任务 10: 有符号绝对值域与空值【批次 10】依赖批次 9（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/10-有符号绝对值域与空值.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次10的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/10-有符号绝对值域与空值.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 11

## 任务 11: 合并空值的结果类型固定【批次 11】依赖批次 10（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/11-合并空值的结果类型固定.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次11的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/11-合并空值的结果类型固定.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 12

## 任务 12: 合并空值的运行期短路【批次 12】依赖批次 11（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/12-合并空值的运行期短路.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次12的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/12-合并空值的运行期短路.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 13

## 任务 13: 空值比较的提升与必需求值【批次 13】依赖批次 12（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/13-空值比较的提升与必需求值.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次13的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/13-空值比较的提升与必需求值.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 14

## 任务 14: 条件表达式字符规则【批次 14】依赖批次 13（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/14-条件表达式字符规则.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次14的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/14-条件表达式字符规则.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 15

## 任务 15: 拼接函数的双方言分流【批次 15】依赖批次 14（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/15-拼接函数的双方言分流.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次15的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/15-拼接函数的双方言分流.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 16

## 任务 16: 极值表达式的类型空值与参数【批次 16】依赖批次 15（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/16-极值表达式的类型空值与参数.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次16的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/16-极值表达式的类型空值与参数.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 17

## 任务 17: 极值比较的字符规则【批次 17】依赖批次 16（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/17-极值比较的字符规则.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次17的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/17-极值比较的字符规则.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 18

## 任务 18: 保留空值替代函数专属契约【批次 18】依赖批次 17（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/18-保留空值替代函数专属契约.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次18的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/18-保留空值替代函数专属契约.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 19

## 任务 19: 保留日期格式函数专属契约【批次 19】依赖批次 18（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/19-保留日期格式函数专属契约.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次19的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/19-保留日期格式函数专属契约.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 20

## 任务 20: 保留JSON提取函数专属契约【批次 20】依赖批次 19（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/20-保留JSON提取函数专属契约.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次20的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/20-保留JSON提取函数专属契约.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 21

## 任务 21: 构建信息函数身份与隔离【批次 21】依赖批次 20（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/21-构建信息函数身份与隔离.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次21的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/21-构建信息函数身份与隔离.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 22

## 任务 22: 交错连接与缓存签名隔离【批次 22】依赖批次 21（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/22-交错连接与缓存签名隔离.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次22的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/22-交错连接与缓存签名隔离.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 23

## 任务 23: 注册与重载真实客户端差分【批次 23】依赖批次 22（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/23-注册与重载真实客户端差分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次23的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/23-注册与重载真实客户端差分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 24

## 任务 24: 绝对值真实客户端差分【批次 24】依赖批次 23（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/24-绝对值真实客户端差分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次24的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/24-绝对值真实客户端差分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 25

## 任务 25: 条件表达式真实客户端差分【批次 25】依赖批次 24（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/25-条件表达式真实客户端差分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次25的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/25-条件表达式真实客户端差分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 26

## 任务 26: 同名异义真实客户端差分【批次 26】依赖批次 25（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/26-同名异义真实客户端差分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次26的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/26-同名异义真实客户端差分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 27

## 任务 27: 专属函数真实客户端差分【批次 27】依赖批次 26（共享文件串行屏障；语义依赖见任务文件）

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/27-专属函数真实客户端差分.md`。

开始前先阅读 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 和该任务文件，确认当前批次27的依赖批次及前一批共享文件屏障已满足。按任务文件中的测试驱动开发/验证计划实施，只处理该任务范围；缺少范围外依赖时记录具体接口，不递归补建子系统。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/27-专属函数真实客户端差分.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新或删除，则无需提交。在最终回复中报告完成证据并删除该任务文件；如果被阻塞，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，若有实际代码、测试、配置或文档变更，使用技能 `$git-commit` 提交该任务的变更；若只有编号任务文件本身的状态更新，则无需提交。保留任务文件供回归验证。
注意，`/Users/Shared/work/dir/data/codes/astersql-tidb/.plans/2026-10-29-postgresql18-函数注册重载与双方言隔离/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

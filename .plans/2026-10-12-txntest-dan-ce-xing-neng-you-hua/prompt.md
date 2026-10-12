# txntest 单测性能优化任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

# 批次 1

## 任务 1: 优化 TestStaleReadAllCombinations【批次 1】 依赖批次：无

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/1-test-stale-read-all-combinations.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/1-test-stale-read-all-combinations.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 2

## 任务 2: 优化 TestStaleReadCompatibility【批次 2】 依赖：批次 1

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/2-test-stale-read-compatibility.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/2-test-stale-read-compatibility.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 3

## 任务 3: 优化 TestStalePrepare【批次 3】 依赖：批次 2

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/3-test-stale-prepare.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/3-test-stale-prepare.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 4

## 任务 4: 优化 TestValidateReadOnlyInStalenessTransaction【批次 4】 依赖：批次 3

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/4-test-validate-read-only-in-staleness-transaction.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/4-test-validate-read-only-in-staleness-transaction.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 5

## 任务 5: 优化 TestStaleReadNoBackoff【批次 5】 依赖：批次 4

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/5-test-stale-read-no-backoff.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/5-test-stale-read-no-backoff.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 6

## 任务 6: 优化 TestStaleSessionQuery【批次 6】 依赖：批次 5

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/6-test-stale-session-query.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/6-test-stale-session-query.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 7

## 任务 7: 优化 test_mem_db_tracker【批次 7】 依赖：批次 6

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/7-test-mem-db-tracker.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/7-test-mem-db-tracker.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 8

## 任务 8: 优化 TestTxnScopeAndValidateReadTs【批次 8】 依赖：批次 7

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/8-test-txn-scope-and-validate-read-ts.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/8-test-txn-scope-and-validate-read-ts.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 9

## 任务 9: 优化 test_shared_lock_cascade_update_explicit_pessimistic_transaction【批次 9】 依赖：批次 8

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/9-test-shared-lock-cascade-update-explicit-pessimistic-transaction.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/9-test-shared-lock-cascade-update-explicit-pessimistic-transaction.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 10

## 任务 10: 优化 TestLockKeysInDML【批次 10】 依赖：批次 9

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/10-test-lock-keys-in-dml.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/10-test-lock-keys-in-dml.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 11

## 任务 11: 优化 TestSelectAsOf【批次 11】 依赖：批次 10

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/11-test-select-as-of.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/11-test-select-as-of.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 12

## 任务 12: 优化 TestAsOfTimestampCompatibility【批次 12】 依赖：批次 11

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/12-test-as-of-timestamp-compatibility.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/12-test-as-of-timestamp-compatibility.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 13

## 任务 13: 优化 test_replica_read_effect_scope【批次 13】 依赖：批次 12

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/13-test-replica-read-effect-scope.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/13-test-replica-read-effect-scope.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 14

## 任务 14: 优化 test_shared_lock_blocked_by_exclusive_lock【批次 14】 依赖：批次 13

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/14-test-shared-lock-blocked-by-exclusive-lock.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/14-test-shared-lock-blocked-by-exclusive-lock.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 15

## 任务 15: 优化 test_shared_lock_lock_view【批次 15】 依赖：批次 14

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/15-test-shared-lock-lock-view.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/15-test-shared-lock-lock-view.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 16

## 任务 16: 优化 test_shared_lock_child_table_conflict【批次 16】 依赖：批次 15

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/16-test-shared-lock-child-table-conflict.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/16-test-shared-lock-child-table-conflict.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 17

## 任务 17: 优化 test_shared_lock_data_lock_waits_from_storage_wait_table【批次 17】 依赖：批次 16

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/17-test-shared-lock-data-lock-waits-from-storage-wait-table.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/17-test-shared-lock-data-lock-waits-from-storage-wait-table.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```

# 批次 18

## 任务 18: 优化 test_shared_lock_blocks_exclusive_lock_until_every_holder_commits【批次 18】 依赖：批次 17

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/18-test-shared-lock-blocks-exclusive-lock-until-every-holder-commits.md`。

开始前先阅读 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的测试驱动开发/验证计划实施，尽可能只处理该任务范围，遇到阻塞可以扩大任务范围。
本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/18-test-shared-lock-blocks-exclusive-lock-until-every-holder-commits.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
任务获得所需验证证据后，标记为 `已完成`，使用技能 `$git-commit` 提交该任务的变更，在最终回复中报告完成证据并删除该任务文件；如果被阻塞，首先要尽可能突破阻塞，缺少依赖就修复依赖，也可以超出任务范围修改，如果实在无法突破，只在任务文件中更新为 `已阻塞` 并记录具体原因和已运行的检查；如果因为无关本次修改的原因导致无法运行验证，可以解除 `已阻塞`，修改状态为 `已完成，待回归`，使用技能 `$git-commit` 提交该任务的变更并保留任务文件供回归验证。
注意，`.plans/2026-10-12-txntest-dan-ce-xing-neng-you-hua/plan.md` 为只读，不要修改该文件的任何部分，这会增加上下文的大小，还会干扰其他任务的执行。
```


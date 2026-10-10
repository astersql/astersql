# Rust 单元测试超时修复任务提示词

按批次执行。执行前阅读 `plan.md` 和对应任务文件；`plan.md` 是只读架构指南。

# 批次 1

## 任务 1: 消除 lockstore 固定十秒测试【批次 1】

请执行 `.plans/2026-10-10-rust-test-timeout-fixes/1-lockstore-fixed-duration.md`。开始前阅读计划和任务文件，按失败/通过证据实施。执行 Cargo 前遵守任务文件中的“Cargo 共享槽位规则”。

## 任务 2: 优化 indexusage 会话增量热点【批次 1】

请执行 `.plans/2026-10-10-rust-test-timeout-fixes/2-indexusage-hot-path.md`。开始前阅读计划和任务文件，按失败/通过证据实施。执行 Cargo 前遵守任务文件中的“Cargo 共享槽位规则”。

# 批次 2

## 任务 3: 收敛 session runtime 典型超时【批次 2，依赖批次 1】

请执行 `.plans/2026-10-10-rust-test-timeout-fixes/3-session-runtime-cost.md`。开始前阅读计划和任务文件，先诊断后只修复有证据的共同根因。执行 Cargo 前遵守任务文件中的“Cargo 共享槽位规则”。

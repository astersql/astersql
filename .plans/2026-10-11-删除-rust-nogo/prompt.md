# 删除 Rust nogo 镜像任务提示词

按顺序复制以下提示到独立对话框执行。执行前先阅读 `plan.md` 和对应任务文件；`plan.md` 只是整体架构指南，不是进度来源，不要修改它的任何部分。

# 批次 1

## 任务 1: 删除 Rust nogo 镜像【批次 1】

```text
请使用技能 ./skills/do-task-plans/SKILL.md 执行 `.plans/2026-10-11-删除-rust-nogo/1-删除-rust-nogo.md`。

开始前先阅读 `.plans/2026-10-11-删除-rust-nogo/plan.md` 和该任务文件，确认依赖已满足。按任务文件中的验证计划实施，只删除 Rust nogo/linter 镜像及其专用接线，保留 Go/Bazel nogo 路径。获得所需验证证据后更新任务状态并在最终回复中报告；不要修改 `plan.md`。

本任务涉及 Rust/Cargo：执行任何 Cargo 构建、检查、测试或调用 Cargo 的脚本前，先阅读并遵守 `.plans/2026-10-11-删除-rust-nogo/1-删除-rust-nogo.md` 中的“Cargo 共享槽位规则”，动态领取仓库共享槽位 1–10，设置绝对路径 CARGO_TARGET_DIR；阶段结束后清理自己的锁并保留缓存，最终报告实际槽位和 CARGO_TARGET_DIR。
```

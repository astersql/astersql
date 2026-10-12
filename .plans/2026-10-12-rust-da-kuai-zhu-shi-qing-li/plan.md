# 清理已实现 Rust 大块注释残留计划

目标：删除审计确认已有可执行 Rust 实现和回归证据的 70 个重复档案，同时保留合法说明与尚未补齐行为的残留。

范围：来源为 `.plans/2026-10-12-rust-da-kuai-zhu-shi-can-liu-fen-xi/reports/final-audit.md` 的 P3：Domain/Session/TTL 14、Planner 12、Executor 18、DDL/Store 7、DXF/Errors/Meta 5、Parser 13、Privilege 1。

范围外：不删除 279 个部分实现缺口、36 个未实现占位、95 个非执行测试档案或 100 个合法说明/模板/夹具；不补建完整子系统。

假设：审计提交 `f63b32cec4` 已完成 580/580 核对；候选行号会漂移，执行时按内容、符号和 Git 差异定位。

## 设计决策

- 证据先行：先核对 Go 来源、当前 Rust 和真实测试，再删除档案；文本存在性测试不算行为证据。
- 七个模块任务写入互斥，可并行；最终复扫依赖全部模块任务。
- 清理不改变预期行为，因此使用“同一真实测试清理前后均通过”代替失败测试；证据不足即阻塞。

## 架构说明

- 根 `Cargo.toml` 是 workspace；每项使用对应 crate 和仓库级 Cargo 槽位。
- Rust 测试保持在独立测试文件；保留 PingCAP Apache License；处理过的 Rust 文件顶部保留或增加 `// Copyright 2026 AsterSQL.`。
- `plan.md` 是只读架构指南，任务状态只在编号文件中维护。

## 开发策略

- 清理前运行真实基线测试，删除重复注释/raw string 与纯文本 presence 断言，保留非显然不变量说明。
- 清理后先 `cargo fmt --all`，再运行同一测试和 `make lint`；Parser 运行专用 Make 目标。
- 不改 Go/Bazel，默认不触发 `make bazel_prepare`；若执行中触发 AGENTS.md 条件则必须补跑。

## Purpose / Big Picture

完成后，已被 Rust 实现和测试替代的旧档案不再误导维护者；行为测试保持通过，复扫显示 70 个目标归零。

## Progress

- [x] (2026-10-12) 提取并分组 70 个 P3 目标。
- [ ] 完成批次 1 七个模块清理提交。
- [ ] 完成批次 2 全局复扫与验收。

## Surprises & Discoveries

- Observation: 候选行号会因并行删除漂移。
  Evidence: 候选键是审计快照，因此最终按内容和 diff 核对。
- Observation: 当前工作树没有声明的 `.agents/skills/tidb-verify-profile`。
  Evidence: 读取路径返回不存在；计划直接落实 AGENTS.md 的 Ready 检查。

## Decision Log

- Decision: 只清理 70 个重复档案。
  Rationale: 其余 410 个真实残留仍承载缺失行为，直接删除不安全。
  Date/Author: 2026-10-12 / Codex

## Outcomes & Retrospective

创建阶段未改生产代码。完成时记录删除/保留数、提交、测试数、复扫和未验证边界。

## Context and Orientation

“重复档案”是注释或字符串中的旧实现/测试，其行为已有当前 Rust 与真实测试覆盖。执行者先读原分片报告对应记录；若只能找到同名函数或 archive contains 测试，不能删除。

## Plan of Work

批次 1 并行完成七个互斥模块。批次 2 从分片报告机械提取 70 个目标，重跑扫描并核对合法说明与 P0/P1/P2 残留仍在。

## Concrete Steps

按 `prompt.md` 在独立会话执行；每项完成后单独提交。所有 Cargo 命令先领取任务文件规定的共享槽位。

## Validation and Acceptance

每项真实测试清理前后退出码 0且测试数非零。最终目标总数 70、剩余 0、重复 0，合法内容未误删，`make lint` 通过。

## Idempotence and Recovery

搜索、格式化、测试可重跑。失败时只恢复本任务 diff，不重置其他任务。异常退出须确认进程结束后释放自己的 Cargo 锁。

## Artifacts and Notes

最终证据写入 `cleanup-result.md`，包含目标/删除/保留数、提交、命令、退出码和测试数量。

## Interfaces and Dependencies

不新增公共接口或外部依赖；若发现外部 Rust 依赖缺口，停止删除并另建上游 tag 移植计划。


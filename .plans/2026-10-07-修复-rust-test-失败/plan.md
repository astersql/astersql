# 修复 make rust-test 失败计划

目标：消除日志中 71 个失败 target，恢复全工作区 Rust 非文档测试稳定通过。

范围：依据 `target/rust-test.Uo3yeR` 修复真实行为偏差、测试隔离、外部服务前置条件、Go/Rust 清单漂移和超时计数；先聚焦验证，再执行全量回归。

范围外：不处理日志中已通过 target、Go 既有回归失败或无关重构；不以删测、忽略测试或桩实现换取通过。

假设：日志来自当前 `astersql` 分支；工作区当前干净；执行者须用当前源码和测试验证假设。

## 设计决策

- 按共同根因而非 71 个 target 机械拆分，每项任务只保留一个主要验证故事。
- 已有失败测试就是修复前证据，必须先复现再修改。
- RealTiKV/PD 环境失败独立处理，必须验证服务健康并清理。

## 架构说明

- 共享 testkit/session/planner 行为先修，依赖其输出的用例后修。
- DDL 遵循 owner/job/state-machine；Rust 逻辑和测试尽量与 Go 保持一致。

## 开发策略

- 使用失败验证—通过验证—重构，不删减逻辑、不刷新错误期望。
- Rust 代码可用后添加 `// Copyright 2026 AsterSQL.`，保留 PingCAP Apache License。
- 修改后先 `cargo fmt --all`，再运行聚焦与受影响验证。

## Progress

- [x] 2026-10-07：解析日志并确认 71 个失败 target。
- [x] 2026-10-07：按根因和验证资源拆为 56 个小任务。
- [ ] 执行批次 1–6。
- [ ] 执行批次 7 全量回归。

## Surprises & Discoveries

- 失败同时包含行为偏差、共享状态污染和未启动 PD/TiKV，不能统一按 golden 漂移处理。
- table already exists、指标累积、宿主机内存进入测试均表明隔离问题。

## Decision Log

- Decision: 共享基础行为前置，RealTiKV 与全量回归后置。
  Rationale: 降低重复修复与并行文件冲突。
  Date/Author: 2026-10-07 / Codex

## Outcomes & Retrospective

尚未实施；最终要求所有聚焦验证和 `make rust-test` 有当前通过证据。


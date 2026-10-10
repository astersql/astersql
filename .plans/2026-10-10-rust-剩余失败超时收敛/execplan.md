# 收敛 Rust 剩余失败与超时

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` current while executing it. Reference repository-root `PLANS.md`.

## Purpose / Big Picture

执行后，来源日志中的剩余 Rust FAIL/TIMEOUT 应能在各自真实运行环境中稳定通过；开发者可以从保存的基线清单看到每个用例的根因分类、修复前后耗时和验证命令。

## Progress

- [x] (2026-10-10) 从用户输出恢复目标测试清单并定位 crate、源文件和验证入口。
- [x] (2026-10-10) 执行任务 1，生成当前提交上 25/25 项的可复现基线。
- [ ] 并行完成本地 crate 任务 2–7 和任务 10。
- [ ] 串行完成共享集群任务 8–9。
- [ ] 汇总默认 profile 复验和 Ready 检查证据。

## Surprises & Discoveries

- Observation: `target/rust-test.Y9i1xa` 是已清理的临时产物，当前工作区和 `/tmp` 均不存在。
  Evidence: `find target /tmp -name 'rust-test.*'` 无输出，因此任务 1 不依赖旧文件内容。
- Observation: 当前提交已修复八个原超时用例。
  Evidence: 提交 `ac22fe7863` 包含 TTL、combined statistics、indexusage、lockstore 修复。
- Observation: 9 个既有修复复验项均在默认 profile 下通过，其中 `combined_merge_sql_100010_rows_seven_partitions` 为 8.888s，仍接近 10s 上限。
  Evidence: `target/rust-timeout-baseline/logs/fixed-*-default.log`。
- Observation: 当前稳定存在 3 个正确性失败和 3 个默认超时。
  Evidence: bootstrap 177/178 的存储版本断言为 `317 != 262`，schema checker 的 `unwrap_err` 收到 `Ok`；timer/profile/traceevent 分别在 10.006s/10.007s/10.007s 被终止。
- Observation: timer 是超出默认预算的固定重试等待，traceevent 包含显式 11 秒 cooloff，profile 则在 60s diagnostic 预算内仍未完成。
  Evidence: timer diagnostic 20.071s 通过，traceevent diagnostic 11.016s 通过，`pkg/util/traceevent/traceevent_test.rs:153` 执行 `sleep(Duration::from_secs(11))`，profile diagnostic 60.007s 超时。
- Observation: RealTiKV 基线无法在当前主机按仓库契约启动。
  Evidence: `command -v tiup` 退出码 1，同时 `127.0.0.1:2379` 已有外部 PD；本任务未动该集群。
- Observation: `tiup` 实际存在于 `/Users/xiangmin/.tiup/bin/tiup`，可用端口偏移隔离现有 2379 集群。
  Evidence: 唯一 tag `rust-baseline-01a12497` 在 12379 返回 PD v8.5.8；测试后 12379 不可达、tag 数据已清理，2379 仍返回 v8.5.1。
- Observation: 任务 8–9 的部分计划过滤器与 nextest 实际名称不符，原样执行会得到 0 tests。
  Evidence: `cargo nextest list` 显示 add-index、paging 与 split-file 是独立测试二进制名下的无模块前缀测试；更正过滤器后每组均运行 2 个有效测试。

## Decision Log

- Decision: 以精确测试重跑重建基线，不根据缺失日志猜测错误文本。
  Rationale: 测试名可靠，但失败原因和耗时可能已随当前提交变化。
  Date/Author: 2026-10-10 / Codex
- Decision: RealTiKV 两个任务置于独立串行批次。
  Rationale: 它们共享 playground、端口和数据目录，并行会制造非代码噪声。
  Date/Author: 2026-10-10 / Codex

## Outcomes & Retrospective

任务 1 已产出 25 行机器可读清单，25 项均有当前提交的有效测试证据。已确认任务 3、4、6、7 仍需实施，任务 2 和 5 当前为绿色；任务 8 的 add-index 已恢复而 paging 仍超时，任务 9 两项已恢复。隔离 playground、Cargo 槽位和 tag 数据均已清理，任务 1 完成。

## Context and Orientation

默认 nextest 在 5 秒标慢、10 秒终止普通测试。FAIL 表示断言或运行错误，TIMEOUT 只说明超过预算，必须采样确认是算法、固定等待、初始化还是资源竞争。`docs/agents/testing-flow.md` 定义 RealTiKV 生命周期；`.config/nextest.toml` 定义默认预算。

## Plan of Work

先执行任务 1 获取当前、可解析的基线。随后并行处理本地 crate：session/domain、bootstrap、schema checker、global statistics、SST transport、timer、profile/traceevent。最后启动一次受控 TiKV playground，串行处理 DDL/paging 与 import/testutils，并在每项后恢复干净集群状态。

## Concrete Steps

从仓库根目录按 `prompt.md` 的批次执行。每个 Rust 任务先领取共享 Cargo 槽位，运行精确 nextest；超时用 diagnostic profile 完成一次有界测量和采样。RealTiKV 任务严格使用任务文件中的启动、健康检查和 trap 清理命令。

## Validation and Acceptance

每个来源用例都必须有当前基线和修复后证据。普通测试以默认 nextest 单例小于 10 秒且退出码 0 为准；RealTiKV 以其已有 scoped override 内通过、无 playground 泄漏为准。最终运行 `cargo fmt --all`、适用的精确测试、`make lint` 和 `git diff --check`。

## Idempotence and Recovery

精确测试和采样可重复运行。Cargo 槽位只删除自己的 lock owner；RealTiKV 使用唯一 tag 和 trap，异常后先停止自己的 PID，再删除该 tag 数据，禁止清理其他任务资源。

## Artifacts and Notes

来源清单见任务 1；所有修复不得减少原 SQL 行数、分区数、状态枚举或并发语义。

## Interfaces and Dependencies

使用 `cargo nextest`、macOS `sample`（可用时）、RustCodeGraph、TiUP playground 和现有 crate 测试接口，不新增外部依赖。

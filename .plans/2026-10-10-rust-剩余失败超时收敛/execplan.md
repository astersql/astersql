# 收敛 Rust 剩余失败与超时

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` current while executing it. Reference repository-root `PLANS.md`.

## Purpose / Big Picture

执行后，来源日志中的剩余 Rust FAIL/TIMEOUT 应能在各自真实运行环境中稳定通过；开发者可以从保存的基线清单看到每个用例的根因分类、修复前后耗时和验证命令。

## Progress

- [x] (2026-10-10) 从用户输出恢复目标测试清单并定位 crate、源文件和验证入口。
- [x] (2026-10-10) 执行任务 1，生成当前提交上 25/25 项的可复现基线。
- [x] (2026-10-10) 复验任务 2 session Domain 与 Starter 目标及两个测试模块窄集合，确认当前实现已稳定收敛。
- [x] (2026-10-10) 完成任务 4，session schema checker 已正确校验分区表的物理写键。
- [x] (2026-10-10) 完成任务 3，bootstrap 177/178 回归改为断言 Rust 当前版本常量，5 个目标测试在默认 profile 下通过。
- [x] (2026-10-10) 完成任务 6，timer panic 恢复测试用显式阶段事件替代固定等待并连续 20 次通过。
- [x] (2026-10-10) 复验任务 5 SST transport 取消语义 20 次，确认当前实现已稳定收敛。
- [x] (2026-10-10) 完成任务 7，profile 与 traceevent 目标及两个 crate 全测在默认 profile 下通过。
- [x] (2026-10-10) 复验任务 10 global stats options 目标与窄回归，确认无需性能修改。
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
- Observation: 任务 10 的 `TestAnalyzeGlobalStatsWithOpts2` 已在默认 10 秒预算内稳定通过，未复现计划假设的 global merge 超时。
  Evidence: 单例复验 1/1 通过，nextest 耗时 3.439s；关联 `TestAnalyzeGlobalStatsWithOpts1/2` 窄回归 2/2 通过，分别耗时 3.115s 和 3.550s，测试中四次 ANALYZE 及 global/p0/p1 options 断言均保留。
- Observation: 任务 10 首次命令的 302.26s 总墙钟为冷编译和共享 Cargo build-dir 排队，不是测试执行热点。
  Evidence: nextest 报告测试本体 3.439s，而 Cargo 编译阶段为 4m57s；窄回归中两个测试总计 6.666s。
- Observation: profile 超时来自测试额外等待全局 profiler 首轮符号化输出；生产采集路径本身在保留真实 2 秒窗口后可于约 2.08s 完成。
  Evidence: 删除测试专用 warmup 后目标重复 3 次分别为 2.039s、2.037s、2.081s，仍启动全局 profiler、生成 CPU 负载并断言 profile 数据。
- Observation: traceevent 超时完全来自测试为跨过 10 秒冷却期而执行的 11 秒真实睡眠。
  Evidence: 将 dump 核心抽成接收秒时间戳的 crate 内边界后，原冷却断言保持不变，目标重复运行约 0.015s；reset 同时清除 `LAST_DUMP_TIME`。
- Observation: 任务 5 的 blocked-write 取消用例在当前提交上未复现原 2 秒失败。
  Evidence: 默认 nextest profile 首次 1/1 通过，随后连续 20 次均 1/1 通过；测试本体耗时 0.08–0.19s，取消后无 ingest 请求、无 batch frame，wire worker 由 harness `Drop` join。
- Observation: timer 用例的约 20 秒耗时来自两次真实 panic 后各执行一次生产 10 秒重试等待，不是恢复事件丢失。
  Evidence: Go 对齐测试将 `retryLoopWait` 设为 1ms；Rust 基线在默认 profile 10.006s 超时、diagnostic profile 20.071s 通过。对齐后单次测试 0.039s，连续 20 次全部通过。
- Observation: bootstrap 177/178 的两个正确性失败来自测试将当前版本硬编码为已过期的 `262`，升级执行器实际正确写入当前 `317`。
  Evidence: Go 对照测试断言 `session.CurrentBootstrapVersion`；Rust 改为读取 `upgrade_def::currentBootstrapVersion` 后两个失败项分别在 0.859s 和 0.788s 通过。
- Observation: Cargo 槽位隔离最终产物，但仓库 `.cargo/config.toml` 的共享 `build-dir = "target"` 仍会让并发构建排队并争用 CPU。
  Evidence: 首次完整复跑中 dist-task 状态矩阵在重负载下 10.015s 超时；负载下降后同一命令为 4.601s，5/5 通过。
- Observation: 任务 2 的 session 初始化目标在当前提交上未复现超时。
  Evidence: 精确默认 profile 2/2 通过，Starter 与 Domain 用例分别为 3.353s 和 2.190s；所属两个测试模块窄回归 28/28 通过，目标用例分别为 3.749s 和 2.067s。
- Observation: 仅设置 `CARGO_TARGET_DIR` 时，仓库 `build-dir = "target"` 配置会使 Cargo 仍打开根目录文件锁。
  Evidence: 槽位 5 首次命令打开 `target/debug/.cargo-build-lock`；同时设置 `CARGO_BUILD_BUILD_DIR="$CARGO_TARGET_DIR"` 后，冷编译和测试在 `target/rust-slot-5` 独立完成。

- Observation: 任务 4 补齐 Go 的 MDL/txn-mode 前置后 normal 路径恢复，partition 路径仍将应失败的提交返回 `Ok`。
  Evidence: 修复前精确 nextest 在 `pkg/session/test/session_test.rs:1662` 失败；补前置后失败点移到 partition 断言 `:1665`。
- Observation: Rust session 的相关表集合记录逻辑表 ID，但分区写键解码为物理分区 ID，导致 `transaction_schema_changed` 把真实分区写误判为无写入。
  Evidence: 按事务开始 schema 的 `PartitionInfo.Definitions` 映射物理 ID 后，目标用例 1/1 在 0.864s 通过，现有 normal/temporary-table 窄回归 1/1 在 0.740s 通过。

## Decision Log

- Decision: 以精确测试重跑重建基线，不根据缺失日志猜测错误文本。
  Rationale: 测试名可靠，但失败原因和耗时可能已随当前提交变化。
  Date/Author: 2026-10-10 / Codex
- Decision: RealTiKV 两个任务置于独立串行批次。
  Rationale: 它们共享 playground、端口和数据目录，并行会制造非代码噪声。
  Date/Author: 2026-10-10 / Codex
- Decision: 任务 10 不做无热点证据的生产代码优化。
  Rationale: 目标与关联回归均在默认预算内通过，且完整保留 Go 版测试意图；继续修改会超出“先复验、避免重复修改”范围。
  Date/Author: 2026-10-10 / Codex
- Decision: profile 只移除测试预热，不缩短真实采样窗口；traceevent 通过内部时间边界推进冷却时间。
  Rationale: 这样同时保留 Go 测试的真实 profiler 生命周期、profile 数据断言、ring-buffer 行为和完整 10 秒生产冷却语义。
  Date/Author: 2026-10-10 / Codex
- Decision: 任务 5 不修改 SST transport 生产或测试代码。
  Rationale: 当前用例已用受控 limiter 固定 blocked-write 交错，并在 20 次默认预算回归中持续证明取消先于 ingest；无失败证据时继续改动会违反“先复验已修复项”的范围限制。
  Date/Author: 2026-10-10 / Codex
- Decision: timer 生产重试间隔保持 10 秒，仅向 Rust 测试暴露与 Go 同类的 1ms 重试配置和 panic/retry/resumed 观测事件。
  Rationale: 这保留了真实 runtime loop 与取消语义，同时让测试通过通道确认阶段顺序，不再依赖轮询或任意 sleep。
  Date/Author: 2026-10-10 / Codex
- Decision: 任务 3 不修改 bootstrap 生产升级逻辑，只消除两个 Rust 回归中的版本号硬编码。
  Rationale: 同 store 的 DDL table version、global variable 容量和六种 dist-task 状态行为均已通过；根因是测试偏离 Go 的当前版本常量断言，扩大生产修改没有失败依据。
  Date/Author: 2026-10-10 / Codex
- Decision: 任务 2 不修改 session 生产或测试代码。
  Rationale: 两个目标用例在任务 1 基线和本次独立复验中都低于默认 10 秒预算，且 1024 用户、密码历史、claim/warning 和 Domain 复用/替换断言全部保留；无失败或热点证据时修改初始化路径会超出本任务范围。
  Date/Author: 2026-10-10 / Codex

## Outcomes & Retrospective

任务 2 在当前提交上无需代码修改：精确目标 2/2 与所属模块窄回归 28/28 均在默认 profile 下通过，最慢目标 3.749s；`cargo fmt --all`、`make lint` 与任务文档 diff 检查通过。

任务 5 在当前提交上无需代码修改：SST blocked-write 取消顺序已连续 20 次在默认 profile 下通过，且每次均无 ingest、batch frame 或未 join 的 wire worker。

任务 6 已保留生产 10 秒 panic 重试和立即取消语义，并使测试显式验证两次 panic、两次 retry 及 post-recover 刷新顺序。目标用例单次 0.039s，重复 20/20 通过，`make lint` 通过。

任务 3 已将 bootstrap 177/178 回归与 Go 的当前版本断言重新对齐，保留 DDL table version、16383 字符容量和完整六状态 dist-task 矩阵。默认 profile 目标 5/5 通过，`cargo fmt --all`、`make lint` 和 `git diff --check` 通过。

任务 1 已产出 25 行机器可读清单，25 项均有当前提交的有效测试证据。任务 3、4 已完成，任务 2 和 5 当前为绿色；任务 8 的 add-index 已恢复而 paging 仍超时，任务 9 两项已恢复。隔离 playground、Cargo 槽位和 tag 数据均已清理，任务 1 完成。任务 10 在当前提交上无需代码修改：target 和关联 global-stats options 窄回归均在默认 profile 下通过，最慢单例 3.550s。任务 7 已移除 profile 的冗余首次符号化预热，并以可控时间边界验证 traceevent 冷却；目标 2/2、重复 3/3 和 crate 全测 27/27 通过，`make lint` 通过。

任务 4 已补齐 Go `TestSchemaCheckerSQL` 的全局前置，并将分区写键的物理 ID 映射回事务开始 schema 中的逻辑表；目标 normal/partition 用例与相邻 schema-check 窄回归均在默认 nextest profile 通过。

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

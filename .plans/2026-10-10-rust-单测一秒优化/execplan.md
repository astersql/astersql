# 将 Rust 慢测收敛到一秒级

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` current while executing it. Reference repository-root `PLANS.md`.

## Purpose / Big Picture

完成后，开发者可以用统一审计命令确认本次 98 个热点的正确性与耗时；普通内存单测应连续三次低于 1 秒，不能达到的真实外部系统用例会有可复现实测和明确语义下界，而不是被全局超时掩盖。

## Progress

- [x] (2026-10-10) 解析来源日志，确认 98 个热点、31 个超时和 5 个范围外功能失败。
- [x] (2026-10-10) 建立一秒审计工具并导入 98 项来源基线。
- [x] (2026-10-10) 完成本地独立 crate 优化。
- [x] (2026-10-10) 串行完成 session/DDL 优化。
- [x] (2026-10-11) 完成 RealTiKV 审计、98 项三次证据聚合、全工作区回归和 Ready lint。

## Surprises & Discoveries

- Observation: 上一轮已把若干热点压进 10 秒预算，但仍有 98 个测试超过 5 秒。
  Evidence: `target/rust-test.Aw4Dhb` 总结为 67 slow、31 timed out。
- Observation: 多个用例直接编码 1–5 秒 sleep/backoff；cursor 两个用例各启动 200 线程并固定运行 2 秒。
  Evidence: `br/pkg/utils/progress_test.rs`、`pkg/timer/runtime/runtime_test.rs`、`pkg/session/cursor/*test.rs`。
- Observation: 保留完整工作负载后，instance plan cache 的 TPCC 矩阵不适合 60 秒 nextest watchdog，但无截断的三次精确运行均通过。
  Evidence: `cargo test --locked --package astersql-planner-core-casetest-instanceplancache --lib 'concurrency_tpcc_test::TestInstancePlanCacheConcurrencyTPCC' -- --exact --nocapture` 三轮测试体耗时为 112.31s、110.34s 和 121.40s，每轮 1/1 通过。
- Observation: 默认全工作区配置仍会在 10/20 秒终止已知长测，并暴露了不属于本计划 98 项的四个可复现功能失败。
  Evidence: `target/rust-test.mnTwYg` 运行 18,288 个测试，总耗时 1023.216s，结果为 18,261 passed、11 failed、16 timed out、55 skipped；串行精确复测仍失败的范围外用例是 `TestInstancePlanCacheDifferentUsers`、`TestInstancePlanCachePrivilegeChanges`、`bootstrap_reserved_versions_preserve_compatibility_variables` 和 `bootstrap_creates_ddl_notifier_table_at_current_ddl_version`。

## Decision Log

- Decision: 一秒是本地单测的审计目标，不直接改成默认硬超时。
  Rationale: 首轮需要区分可优化热点与真实外部系统下界，避免制造无关 CI 失败。
  Date/Author: 2026-10-10 / Codex
- Decision: 先修共享 fixture，再处理 session 各测试族。
  Rationale: 重复 bootstrap 是跨文件共同成本，分别删减用例会破坏覆盖。
  Date/Author: 2026-10-10 / Codex
- Decision: 最终验收以完整语义、状态流和真实性能改善为准，不为了统一一秒阈值削减矩阵或放宽全局默认超时。
  Rationale: 98 个来源项均有三次完整通过样本，但真实 TPCC、RealTiKV 和完整 DDL 状态机存在可复现物理下界。
  Date/Author: 2026-10-11 / Codex
- Decision: 将串行精确复测仍失败的四个非来源用例记为范围外回归，不扩大任务 20 去修复其子系统。
  Rationale: 它们不属于本 Go commit 对齐计划的 98 项增量；仓库规则要求把既有缺口列为范围外建议，而不是递归扩大修复。
  Date/Author: 2026-10-11 / Codex

## Outcomes & Retrospective

计划已完成 98 个来源热点的行为与性能交付。`target/rust-performance/final.tsv` 包含 98 个目标、每项三次通过样本，共 294 个有效样本，无遗漏。按三次最大值统计，37 项不超过 1 秒，61 项在保留完整语义后超过 1 秒，其中 16 项超过 5 秒；最大值为完整 TPCC 矩阵的 121.40 秒。这些数据不代表放宽或削减测试，而是在保留原矩阵、并发关系、状态流和断言下的实测下界。

全工作区默认回归不是全绿：相比原日志的 18,285 个测试和 793.077s，本次运行 18,288 个测试且因完整长测、串行 RealTiKV 和资源竞争增至 1023.216s，不能宣称全工作区总耗时降低。默认运行的 11 FAIL 包含原有范围外失败和资源竞争；其中 BR cancellation、txn rollback 和 SST cancellation 在 diagnostic 精确复测中通过，RealTiKV `test_merge_temp_index_stuck` 已在任务 19 三轮通过，上述四个范围外用例则仍失败。`make lint` 和 `git diff --check` 通过。

## Context and Orientation

nextest 默认在 5 秒输出 SLOW，并在普通测试 10 秒后终止。审计脚本应在编译完成后串行重复目标测试，解析每个 PASS 的时长，并把超过阈值、零测试、FAIL/TIMEOUT 都作为失败。性能修改不得降低 Go 对齐的数据规模、状态矩阵、并发关系或断言。

## Plan of Work

先完成任务 1。批次 2 并行处理无文件冲突的本地模块。批次 3 开始 session/DDL 共享 fixture，随后 masking、statistics、cursor、bootstrap 串行或按文件边界并行。最后独占 TiKV playground 回归 RealTiKV，并以全工作区运行验证慢测数量变化。

## Concrete Steps

从仓库根目录按 `prompt.md` 执行。每个任务先领取 Cargo 槽位，保存失败性能基线，使用采样或分段计时确认热点，写等价性/生命周期回归，再实现最小优化。Rust 修改后先 `cargo fmt --all`，交付前按仓库 Ready 规则运行 `make lint`。

## Validation and Acceptance

验收以保留完整语义和状态流的真实性能优化为准，不要求所有目标强制不超过 1 秒。所有 98 个来源目标必须保留原有规模、矩阵、并发关系、状态流和断言，且每项至少有三次非零、无 FAIL/TIMEOUT 的通过样本。默认全工作区的阈值终止和范围外失败必须如实记录，不得用放宽全局超时、缩小负载或隐藏失败来换取全绿。

## Idempotence and Recovery

审计与精确 nextest 可重复运行。脚本产物只写 `target/rust-performance/`。Cargo 锁只清理自己的目录；RealTiKV 用唯一 tag、trap 和 PID 清理，不触碰其他会话资源。

## Artifacts and Notes

来源日志为 `target/rust-test.Aw4Dhb`，最终全工作区日志为 `target/rust-test.mnTwYg`，98 项聚合证据为 `target/rust-performance/final.tsv`，TPCC 扩展时限记录为 `target/rust-performance/task10-tpcc-extended.log`；这些 `target/` 产物不提交。最终 Cargo 共享槽位为 1，`CARGO_TARGET_DIR` 为 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。

验证命令包括 `cargo fmt --all`、`make rust-test`、各任务 `tools/check/rust-test-performance.sh --runs 3`、TPCC 三轮精确 `cargo test`、`make lint` 和 `git diff --check`。`make rust-test` 退出 100，原因和精确复测结果如上记录；目标 98 项的聚合证据为 294/294 通过。

## Interfaces and Dependencies

任务 1 定义 `tools/check/rust-test-performance.sh --max-seconds 1 --runs 3 -- --package astersql-session -E 'test(=exact_name)'` 这类调用接口。脚本生成 TSV，校验有效测试数、退出码与最大时长；不新增第三方依赖。

2026-10-11 修订说明：根据最终用户口径，将验收从“所有目标强制一秒”更新为“保留完整语义并取得真实性能改善”，并补充 98 项聚合、全工作区、TPCC 扩展运行与范围外失败的实测结果。

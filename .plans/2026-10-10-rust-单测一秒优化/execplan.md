# 将 Rust 慢测收敛到一秒级

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` current while executing it. Reference repository-root `PLANS.md`.

## Purpose / Big Picture

完成后，开发者可以用统一审计命令确认本次 98 个热点的正确性与耗时；普通内存单测应连续三次低于 1 秒，不能达到的真实外部系统用例会有可复现实测和明确语义下界，而不是被全局超时掩盖。

## Progress

- [x] (2026-10-10) 解析来源日志，确认 98 个热点、31 个超时和 5 个范围外功能失败。
- [ ] 建立一秒审计工具并重建隔离基线。
- [ ] 完成本地独立 crate 优化。
- [ ] 串行完成 session/DDL 优化。
- [ ] 完成 RealTiKV 与全工作区回归。

## Surprises & Discoveries

- Observation: 上一轮已把若干热点压进 10 秒预算，但仍有 98 个测试超过 5 秒。
  Evidence: `target/rust-test.Aw4Dhb` 总结为 67 slow、31 timed out。
- Observation: 多个用例直接编码 1–5 秒 sleep/backoff；cursor 两个用例各启动 200 线程并固定运行 2 秒。
  Evidence: `br/pkg/utils/progress_test.rs`、`pkg/timer/runtime/runtime_test.rs`、`pkg/session/cursor/*test.rs`。

## Decision Log

- Decision: 一秒是本地单测的审计目标，不直接改成默认硬超时。
  Rationale: 首轮需要区分可优化热点与真实外部系统下界，避免制造无关 CI 失败。
  Date/Author: 2026-10-10 / Codex
- Decision: 先修共享 fixture，再处理 session 各测试族。
  Rationale: 重复 bootstrap 是跨文件共同成本，分别删减用例会破坏覆盖。
  Date/Author: 2026-10-10 / Codex

## Outcomes & Retrospective

计划阶段完成；执行者在每批次后记录每个目标的基线、最终最大耗时、未达一秒原因及全量回归结果。

## Context and Orientation

nextest 默认在 5 秒输出 SLOW，并在普通测试 10 秒后终止。审计脚本应在编译完成后串行重复目标测试，解析每个 PASS 的时长，并把超过阈值、零测试、FAIL/TIMEOUT 都作为失败。性能修改不得降低 Go 对齐的数据规模、状态矩阵、并发关系或断言。

## Plan of Work

先完成任务 1。批次 2 并行处理无文件冲突的本地模块。批次 3 开始 session/DDL 共享 fixture，随后 masking、statistics、cursor、bootstrap 串行或按文件边界并行。最后独占 TiKV playground 回归 RealTiKV，并以全工作区运行验证慢测数量变化。

## Concrete Steps

从仓库根目录按 `prompt.md` 执行。每个任务先领取 Cargo 槽位，保存失败性能基线，使用采样或分段计时确认热点，写等价性/生命周期回归，再实现最小优化。Rust 修改后先 `cargo fmt --all`，交付前按仓库 Ready 规则运行 `make lint`。

## Validation and Acceptance

普通目标三次串行热运行全部通过、有效测试数非零且最大耗时不超过 1.000 秒。外部协议或 RealTiKV 例外必须较基线显著下降、无固定无意义等待、通过原行为断言，并在任务文件中记录未达标的物理原因。最终全工作区不得新增 FAIL/TIMEOUT，目标 SLOW 数应为零或仅剩批准的外部例外。

## Idempotence and Recovery

审计与精确 nextest 可重复运行。脚本产物只写 `target/rust-performance/`。Cargo 锁只清理自己的目录；RealTiKV 用唯一 tag、trap 和 PID 清理，不触碰其他会话资源。

## Artifacts and Notes

来源日志为 `target/rust-test.Aw4Dhb`；任务文件列出目标测试族。5 个非性能 FAIL 不作为本计划完成阻塞，但最终报告必须列出并确认未因本计划增加。

## Interfaces and Dependencies

任务 1 定义 `tools/check/rust-test-performance.sh --max-seconds 1 --runs 3 -- --package astersql-session -E 'test(=exact_name)'` 这类调用接口。脚本生成 TSV，校验有效测试数、退出码与最大时长；不新增第三方依赖。

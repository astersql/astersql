# Rust 单测一秒优化计划

目标：消除本次日志中的 98 个 Rust 慢测/超时热点，使可隔离的本地单测尽可能稳定收敛到 1 秒内。

范围：覆盖日志中 67 个 SLOW 与 31 个 TIMEOUT，包含 BR、parser、planner、executor、session、server 及 RealTiKV；建立可重复的一秒审计，保留 Go 测试规模、并发、断言与真实行为。

范围外：日志中的 5 个非性能 FAIL、未进入 5 秒慢日志的普通用例、通过放宽超时或删减 Go 对齐语义换取通过；既有 Go 回归不阻塞 Rust 交付。

假设：`.config/nextest.toml` 默认 5 秒标慢、10 秒终止；当前日志 `target/rust-test.Aw4Dhb` 记录 18285 个测试。普通本地测试以三次串行热运行均不超过 1 秒为目标；真实 TiKV、真实 TLS/租约及生产周期若无法达到，必须给出不可替代语义和实测下界。

## 设计决策

- 方案一是缩小数据或并发，最快但会削弱 Go 测试意图，拒绝采用；方案二是提高超时，只隐藏问题，也拒绝采用。
- 采用“先一秒审计与采样，再消除固定等待、复用昂贵初始化、优化生产热点”的路线；只有真实外部时钟/集群语义可保留高于一秒的有证据例外。
- 性能验收与功能验收分离：目标测试必须保持原断言并通过，随后审计三次热运行；RealTiKV 独占 playground 串行执行。
- `pkg/session/runtime` 与 DDL fixture 任务串行，防止多个会话同时修改共享初始化；其他任务仅在文件和验证资源互不冲突时并行。

## 架构说明

- `tools/check/rust-test-performance.sh` 与 nextest 的 `perf-audit` profile 提供统一的一秒测量，不改变默认 CI 超时。
- planner/session 的主要成本来自重复 Domain、逐语句 fixture、组合矩阵和大数据路径；优化必须落在共享 fixture 或经采样确认的生产热点。
- DDL 仍遵循持久化 job、schema state、reorg/checkpoint 与 owner/barrier 语义；RealTiKV 仍使用真实 tikv-slim。
- 外部 Rust 依赖不在本计划新增；若执行中证明必须修改 `astersql/client-rust`，应另开上游移植计划并发布 tag。

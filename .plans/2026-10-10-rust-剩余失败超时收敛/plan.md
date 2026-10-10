# Rust 剩余失败与超时收敛计划

目标：在当前提交上复现并修复日志中尚未收敛的 Rust FAIL/TIMEOUT，使目标用例保持原规模并在默认 nextest 预算内稳定通过。

范围：session/bootstrap、global statistics、SST transport、timer/profile/traceevent，以及 RealTiKV add-index、paging、split-file、公共契约用例；先复验已修复项，避免重复修改。

范围外：未出现在来源清单中的全工作区失败、放宽全局超时、缩减数据规模、修复既有 Go 回归。

假设：原临时日志已被删除，来源测试名和原耗时取自用户贴出的输出；任务 1 必须在当前 `ac22fe7863` 或后续提交重新建立机器可读基线。

## 设计决策

- 先区分稳定代码缺陷、固定等待、资源争用和外部 TiKV 成本；不把提高超时当作首选修复。
- 本地 crate 可并行诊断；RealTiKV 共用 playground，必须串行并保证清理。
- 已修复的 TTL、combined statistics、indexusage、lockstore 仅复验，复验失败才回到对应已有实现。

## 架构说明

- session/bootstrap 共用 Domain 与升级存储路径；statistics analyze 独立处理，避免扩大 session 修改。
- store/timer/util 属于互不依赖 crate；RealTiKV 的 DDL、paging、import、testutils 共用外部集群但代码边界独立。

## 开发策略

- 对会改变行为的代码，使用失败验证-通过验证-重构。
- 在实施步骤前记录精确测试的退出码、墙钟时间和采样热点。
- 保留 Go 测试意图、SQL 数据规模、并发和故障注入语义。
- 每个任务完成后使用 `$git-commit` 单独提交，不夹带其他任务变更。

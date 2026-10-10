# ANALYZE 与大批 INSERT 性能修复计划

目标：修复运行时统计构建的二次内存扫描和索引编码的重复元数据克隆，使两个原规模回归测试进入默认 10 秒预算。

范围：`runtime_stats_builder` 增量内存记账、session row codec 借用式索引编码、test profile 轻量优化、对应独立回归测试和原 SQL 回归。

范围外：缩减测试数据、放宽 nextest 超时、RealTiKV 性能。

假设：macOS `sample` 已证明 53 分区热点位于 `collector_memory`，10 万行热点位于 `encode_relational_index_value_row` 的元数据 clone。

## 设计决策

- 保留现有拥有所有权的 tablecodec API，增加内部借用式编码入口，避免破坏调用者。
- 内存记账使用每次 Collect 前后 FM sketch 和新增 sample 载荷的差值，最终值继续与全量计算交叉验证。
- test profile 使用 `opt-level = 1`，保留调试断言和行号信息，同时避免未优化测试二进制把百万级编码工作放大到超时。

## 架构说明

- 原测试规模与 SQL 断言不变；默认 nextest 10 秒是验收门槛。

## 开发策略

- 先用现有超时作为失败证据，再增加等价性回归并实现优化。
- Rust 修改后先 `cargo fmt --all`，使用共享槽位 1。

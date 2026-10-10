# Session 升级测试性能计划

目标：保留 masking-policy 从 189、254、279 升级的完整行为验证，同时降低 nextest 墙钟时间和 10 秒超时风险。

范围：拆分 `pkg/session/runtime/normal_ddl_masking_policy_test.rs` 中的三版本循环，抽取共享断言，并去掉该场景不需要的 normal-DDL fixture 建表与元数据填充。

范围外：不改 bootstrap 生产语义，不删减列、索引、版本和表 ID 断言，不提高 nextest 超时。

## 设计决策

- 每个历史版本使用独立 `#[test]`，交由 nextest 调度。
- 专用 helper 只创建 bootstrap 所需 Domain/SystemSessionPool，不使用会额外创建 `test.normal_ddl_target` 的 `Fixture::new()`。

## 开发策略

- 先拆分且确认 3/3 行为通过，再移除无关 fixture 成本。
- 使用修改前 8.43–9.53 秒合并场景作为基线；最终同时验证三个独立测试和完整 `test(upgrade)` 过滤集。

## Progress

- [x] 将 189、254、279 拆成三个独立测试并共用完整断言 helper。
- [x] 用专用 Domain/SystemSessionPool 取代不必要的 normal-DDL fixture。
- [x] 完成聚焦测试、完整 upgrade 过滤集和 lint。

## Surprises & Discoveries

- 主要墙钟收益来自拆分后的 nextest 并行调度：合并场景 8.43–9.53 秒，拆分后 3 项总墙钟 2.82 秒。
- 移除无关 fixture 后三项墙钟从 2.82 秒变为 2.81 秒，耗时主体是真实 bootstrap，但该修改仍避免了无关表和 MVCC 元数据工作。

## Decision Log

- 选择三个显式 `#[test]` 而不在单测中创建线程：交给 nextest 做资源调度和单版本失败定位。
- 保留每个版本的独立 Domain/restart，不复用 store，避免跨版本状态污染。

## Outcomes & Retrospective

- 聚焦集 3/3 通过，耗时 2.81 秒；完整 upgrade 集 29/29 通过，耗时 5.80 秒，低于修改前约 9.53 秒。
- 生产 bootstrap 代码未改动，所有版本、表 ID、列和索引断言保留。

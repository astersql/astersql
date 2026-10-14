# Prepared IN 类型转换优化计划

目标：在不偏离 Go 语义的前提下，让 typed prepared SELECT 正确执行 `IN` 的 INT、VARCHAR、REAL 和 DECIMAL 参数，并使 decimal 慢测稳定达到 0.499 秒门槛。

范围：修复 session prepared-statement 到 typed planner/executor 的通用参数转换和 `IN` 接线；覆盖列类型推导、Go 兼容强转、无索引表扫描、索引范围、plan cache 命中/失效和原 decimal 性能回归。

范围外：不减少测试并发量、语句数、数据规模或断言；不增加测试专用快路径；不重写完整 SQL planner。

假设：现有 `PreparePlannedKVSelect` 和 `ExecutePreparedPlannedKVSelectThroughAdapter` 是通用 typed 执行边界；现有 INT 路径可作为正确基线；遇到 typed 路径不支持的 SQL 时必须保留原通用执行 fallback。

## 设计决策

- 选择列元数据驱动的 Datum 转换并扩展 typed `IN` 计划；不盲目将字符串 Datum 与数值列直接比较。
- 备选的“每次绑定后重解析”已证明语义正确但过慢；“持久化完整 AST/plan”影响更广，仅在 typed 通用方案经剖析仍无法达标时才扩展。

## 架构说明

- `ConcreteSession::execute_prepared` 负责缓存与失效，`execute_simple_prepared_select_through_adapter` 负责 typed 路由，planner/executor 根据 AST 和 Datum 构造范围或表扫描。
- 类型转换必须使用已有 TiDB/AsterSQL Datum 语义，并保留 warning、NULL、unsigned、collation、schema generation 失效和事务边界。

## 开发策略

- 对会改变行为的代码，使用失败验证-通过验证-重构。
- 在实施步骤前规划失败验证命令和预期失败。
- 将实施步骤限制在失败测试所证明的行为范围内。
- 测试真实行为，不验证模拟对象或实现细节。

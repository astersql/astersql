# PostgreSQL DataGrip DDL、DML 与事务完善计划

目标：让 DataGrip 通过 PostgreSQL 入口稳定识别并变更表字段，完成参数化 CRUD 与可观察的 DML 事务闭环。

范围：复用现有 PG 3.0/3.2 listener、`public` 到当前原生数据库的映射和 `pg_catalog` provider；补齐 DataGrip 常用 PostgreSQL 建表/字段变更语法的有界适配、字段生命周期后的目录一致性、PG JDBC 参数化 INSERT/SELECT/UPDATE/DELETE、事务提交/回滚/失败状态，并在 MockTiKV 与 RealTiKV 上验收。所有新行为均以真实引擎和目录元数据为准。

范围外：完整 PostgreSQL SQL 方言、事务型 DDL、多用户 schema、COPY、SCRAM/TLS、存储过程、复制协议、完整系统对象和 PostgreSQL MVCC/XID 语义。

假设：`pkg/server/pg_conn.rs` 与 `pg_extended.rs` 继续作为简单/扩展协议入口；`pg_catalog.rs` 从原生 `ModelMeta` 生成 DataGrip 目录结果；DataGrip 2025.1.3 的 27 条冻结查询和 JDBC 42.7.x 回归仍是结构识别基线。当前已有基础 CRUD 和 BEGIN/COMMIT/ROLLBACK，但字段变更闭环、常用 PG DDL 语法与失败事务状态尚无完整证据。

## 设计决策

- 选择“协议边界有界语法适配 → 现有 canonical engine”方案，不分叉执行引擎；无法无损映射的 PG 语法返回稳定 SQLSTATE。
- 不伪造 `pg_catalog` 行。字段、索引和约束必须来自真实原生元数据，DDL 后重新 Execute/刷新可见。
- PostgreSQL DDL 只承诺计划列出的类型和 ALTER COLUMN 形式；DML 事务承诺原子提交、回滚和失败态，不承诺事务型 DDL。
- 真实 DataGrip UI、JDBC 和 RealTiKV 是最终验收面；单元测试不能替代客户端可观察结果。

## 架构说明

- `pg_conn.rs` 处理简单 Query，`pg_extended.rs` 处理 Parse/Bind/Describe/Execute；两条路径必须共享同一 SQL 适配函数。
- `pg_result.rs` 负责命令标签、SQLSTATE 边界和结果编码；新增适配代码与测试分文件，Rust 测试不放入生产源文件。
- `pg_catalog.rs`/`pg_catalog_query.rs` 只负责目录投影和查询，不承担普通 DDL/DML 翻译。
- `pg_client_integration_test.rs` 已具备真实 listener、libpq/JDBC 与 DataGrip SQL 夹具，最终回归应扩展而不是另造模拟协议栈。

## 开发策略

- 对会改变行为的代码，使用失败验证-通过验证-重构。
- 在实施步骤前规划失败验证命令和预期失败。
- 将实施步骤限制在失败测试所证明的行为范围内。
- 测试真实行为，不验证模拟对象或实现细节。

## 方案权衡

- 方案一是在 PG 边界做受限 token/AST 适配，改动集中、可复用原生 DDL/DML，选用此方案。
- 方案二是扩展全局 parser 支持 PostgreSQL 方言，影响所有 MySQL 入口和大量 planner 代码，本阶段风险过大。
- 方案三是把 DataGrip SQL 特判成固定结果，不能保证 DDL 后元数据真实变化，明确拒绝。

## Progress

- [x] 2026-10-07：完成现状调研和任务拆分；实施进度仅记录在编号任务文件中。

## Surprises & Discoveries

- 现有仓库已经通过 DataGrip 2025.1.3 的表、列、键、索引 UI 验收及 27/27 JDBC 内省查询；本计划不重复首期目录实现。
- 当前文档明确只报告事务状态 I/T，不承诺 PostgreSQL 的失败事务 E 状态；这是“事务正常跑通”的关键缺口。

## Decision Log

- Decision: 以有界 PG DDL 适配和真实目录刷新为主线，而非宣称完整 PostgreSQL 兼容。
  Rationale: 复用既有执行引擎并保持 MySQL 行为隔离，范围可验证且不会伪造目录。
  Date/Author: 2026-10-07 / Codex
- Decision: PostgreSQL 事务仅覆盖 DML；DDL 的原生隐式提交行为不包装成事务型 DDL。
  Rationale: TiDB/AsterSQL 的 DDL 事务模型与 PostgreSQL 不同，伪装会造成数据一致性误导。
  Date/Author: 2026-10-07 / Codex

## Outcomes & Retrospective

计划完成时，DataGrip 应能创建表、刷新字段、增加/改名/修改/删除字段，使用 PreparedStatement 完成 CRUD，并观察提交、回滚和失败事务恢复；尚未实施，结果以各任务验证证据为准。


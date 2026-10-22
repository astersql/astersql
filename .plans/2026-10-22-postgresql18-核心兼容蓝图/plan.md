# PostgreSQL 18.6 核心兼容蓝图计划

目标：建立一个以 PostgreSQL 18.6 为基线、共享 AsterSQL 执行内核的核心兼容工程，并为后续小任务分解提供稳定边界。

范围：覆盖 PostgreSQL 线协议、认证与会话、SQL 方言与标识符、核心类型与转换、函数/运算符/聚合/窗口函数、DML/DDL/事务、`pg_catalog` 与真实客户端验收。MySQL 与 PostgreSQL 入口共享等价语义的函数实现，但保留方言特有的名称解析、重载、类型推导、错误和结果元数据。

范围外：不实现 `CREATE EXTENSION`、PostGIS/FDW/自定义过程语言、C 扩展、自定义类型/运算符类和第三方扩展的载入生态。

假设：现有 `pkg/server/pg_*.rs` 继续作为 PostgreSQL 边界，`pkg/expression` 和规范会话/执行链作为共享语义内核；不为 PostgreSQL 另建第二套存储、优化器或执行器。兼容结论必须来自官方 PostgreSQL 18 行为、真实客户端或差分测试，不以“能解析”代替语义验收。

## 设计决策

- 选择“方言感知解析/解析 + 共享规范表达式与执行 + 协议专属编解码”，拒绝长期依赖 SQL 文本替换或两套执行引擎。
- 函数按“语义等价共享、同名异义分流、PG 系统函数专属、MySQL/TiDB 管理函数专属”分类；只有签名、NULL、隐式转换、时区、collation 和错误契约均等价时才能共享别名。
- 不把“PostgreSQL 全量内部管理函数”作为首批终点；应先覆盖应用 SQL、驱动/ORM 必需系统函数和客户端内省契约，其余保持显式不支持。

## 架构说明

- `pkg/server/pg_conn.rs`、`pg_extended.rs`、`pg_result.rs` 负责 wire 与 OID；`pg_name.rs`、`pg_sql.rs`、parser/planner 负责方言语法；`pkg/expression` 负责可共享函数语义；`pg_catalog.rs` 只提供有真实元数据依据的 PG 系统视图。
- 当两种方言的同名函数行为不同时，优先共享底层算子，不共享方言签名、coercion 或返回类型规则。

## 开发策略

- 对会改变行为的代码，使用失败验证-通过验证-重构。
- 在实施步骤前规划失败验证命令和预期失败。
- 将实施步骤限制在失败测试所证明的行为范围内。
- 测试真实行为，不验证模拟对象或实现细节。


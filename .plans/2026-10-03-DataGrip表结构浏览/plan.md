# DataGrip 表结构浏览计划

目标：DataGrip PostgreSQL 数据源能显示真实表、列、索引和约束，并完成实际内省查询。

范围：2026-10-03 最新 DataGrip 日志完整 SQL，表列表关联聚合、数组与集合查询、真实目录元数据、所需辅助目录及客户端默认参数/结果格式，最终在真实客户端验收。

范围外：任意 PG SQL、生产鉴权、复制和存储重构、持久多 schema。

假设：复用 PG 独立目录执行器和共享 schema_snapshot，现有测试提供真实临时 listener 与两版安装 JDBC。用户已授权扩展原计划的 UI 验收边界。

## 设计决策

- 继续扩展受限 AST 和目录执行器；不用 SQL 字符串模板匹配或伪造空结果。
- 每条来源查询冻结，真实表与对象身份必须验证；不存在的原生 PG 特性以经确认的类型正确空集合表达。

## 架构说明

- PG 模块持有 AST、参数类型及单次执行快照，复用现有 planner/executor 接口；不改变 MySQL 全局语义。
- 查询深度、行数、工作量与取消检查覆盖新增子查询和集合查询。

## 开发策略

- 用户明确要求继续实现；创建计划后在当前会话串行实现。
- 失败回归先于修复；Rust 测试与生产分离，保留版权。
- 编号文件为活 ExecPlan，记录 Progress、Surprises & Discoveries、Decision Log、Outcomes & Retrospective；plan.md 生成后只读。
- 批次 1→2→3→4→5→6，全部串行，共享 PG 文件。
- 使用 target/rust-slot-1 至 rust-slot-10 原子领取锁，记录 PID 和 CARGO_TARGET_DIR；结束释放自有锁，缓存保留。
- 修改后 cargo fmt --all，Ready 要求 make lint；纯 Rust 不触发 bazel_prepare，不执行 bazel_lint_changed。verify-profile 技能路径缺失，不声称应用该技能。
- 不覆盖无关工作；完成任务按 git-commit 技能独立提交。

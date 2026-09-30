# 简单查询与 PostgreSQL 结果实施

本活文档遵循 PLANS.md；总计划 plan.md 只读。

## Purpose / Big Picture


3.2 TCP 客户端通过 Query 获取真实 SQL 列、行、完成标签与 ReadyForQuery，NULL 与空字符串区分。

## Progress


- [x] 阅读技能、总计划、任务、根协议、测试指南，确认批次 3 完成；doc.go 不存在。
- [x] 搜索共享执行接口并确认隔离边界。
- [x] 真实 TCP 失败验证：期望 TDCZ，实际 EZ，退出 101。
- [x] 实施编码、查询调用并验证：2 项 simple_query 测试通过。
- [x] Ready 检查与边界审查全部通过。

## Surprises & Discoveries


execute_on_session 收集全部结果后才返回，后续语句失败会丢弃前面结果。因此 PG 本阶段只接受一条语句，多语句明确拒绝且不执行。QueryResult 有 affected_rows、columns、rows 和 response_lifecycle，无 PG 类型或完成标签。

## Decision Log


- Decision: 按用户补充授权增加 lib.rs 模块注册。
  Rationale: 新结果与独立测试模块必须注册；替代为嵌套模块会影响维护，验证覆盖 server lib。不改共享接口或内核。
  Date/Author: 2026-09-30 / Codex
- Decision: 使用现有 Parser 识别单条语句及完成标签；多语句拒绝，结果字段暂以 text OID 25 返回。
  Rationale: 无字符串改写、不猜事务状态；精确类型映射属于任务 5。错误恢复与事务状态遵循现有执行引擎，不声称完整 PG 事务语义。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective


完成真实 TCP 查询与结果编码。覆盖 SELECT、NULL/空串、DDL、DML 影响行数、空查询、分隔符字面量、错误恢复、多语句拒绝无副作用以及真实 BEGIN/ROLLBACK 状态。单条 Query 限制为明确支持边界，类型精确映射、二进制值和执行错误 SQLSTATE 分类留给任务 5；事务语义仍由现有内核定义。

## Context and Orientation


pg_conn.rs 管独立 TCP 连接与 with_query 取消窗口；conn.rs::TiDBContext 是已有协议无关执行边界。pg_result.rs 编码 PG 专属结果，pg_query_test.rs 为分文件 TCP 测试。

## Plan of Work


先注册独立 pg_query_test.rs 并测试 SELECT 1 报文。pg_result.rs 使用现有 Parser 验证单语句、识别可支持命令；输出 RowDescription、DataRow、CommandComplete，确保每个值长度为 PG 大端 i32，NULL 为 -1。pg_conn.rs 调用 with_query 和 execute_query(false)，完成响应生命周期并从 in_transaction 读取 ReadyForQuery。

## Concrete Steps


根目录运行 cargo test -p astersql-server simple_query_roundtrip --lib，失败后实现，再 cargo fmt --all、相同测试、相邻 PG/MySQL/runtime 定向测试、make lint、git diff --check。

## Validation and Acceptance


真实会话及 TCP 上 SELECT 1 返回 T/D/C/Z，DML 返回准确行数；NULL 长度 -1、空字符串 0，空查询 I/Z，失败后下一查询可用，多语句在执行前拒绝。

## Idempotence and Recovery


测试可重复，仅修改 pg_conn.rs、pg_result.rs、pg_query_test.rs、lib.rs 与任务/本活文档。不回滚其他会话文件；无需新增依赖、Bazel 或 Go failpoint 步骤。

## Artifacts and Notes


验证日志存 /tmp/pg-task4-*.log。

## Interfaces and Dependencies


pg_result 调用既有 astersql_parser 与 astersql_parser_ast；pg_conn 调用 TiDBContext，不产生反向 PG 依赖。

## 最终验证证据


Ready 技能目录不存在，按 AGENTS.md 与 testing-flow.md 选择定向检查。所有命令从仓库根运行；Rust 源码与测试分文件，PG Rust/manifest 未使用 failpoint。未修改 Go、Bazel 或依赖，因此不触发 bazel_prepare。

    cargo test -p astersql-server simple_query_roundtrip --lib

实施前退出 101，失败断言实际标签 [69,90] (EZ)，期望 [84,68,67,90] (TDCZ)，日志 /tmp/pg-task4-red.log。第一次实施编译发现 Parser 构造应为 New() 及完成接口需要 Duration 参数，已修复并再格式化。最终同命令退出 0，1 项通过，日志 /tmp/pg-task4-roundtrip-final.log。

以下最终 Ready 命令均退出 0，日志分别为 /tmp/pg-task4-fmt.log、query-final.log、pg-final.log、mysql-final.log、runtime-final.log、lifecycle-final.log、lint.log、diff.log（均 pg-task4- 前缀）：

    cargo fmt --all
    cargo test -p astersql-server simple_query --lib
    cargo test -p astersql-server startup --lib
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    cargo test -p astersql-server concrete_session_driver_authenticates_and_returns_real_sql_results --lib
    cargo test -p astersql-server postgres_listener_lifecycle --lib
    make lint
    git diff --check

simple_query 2 项、startup 7 项，其余定向命令各 1 项通过。make lint 无 error/failed/illegal/pattern 告警。边界检查 `rg -n 'pg_' pkg/server/conn.rs pkg/server/runtime.rs pkg/server/conn_stmt.rs` 无匹配，`git diff --stat -- '.plans/2026-09-30-postgresql-协议第一阶段/plan.md'` 无输出。本任务只改 pg_conn.rs、pg_result.rs、pg_query_test.rs、lib.rs 的模块注册和本任务规划记录，保留其他会话变更。未新增依赖。查询调用使用 execute_query(sql, false, ...)，不改写 SQL；取消通过已有 with_query；结果生命周期记录写入耗时并完成，ReadyForQuery 读取现有 in_transaction。

正确性/兼容性限制：本阶段所有支持结果字段以 text OID 25 返回；二进制值、含 NUL 文本、不支持命令及多语句明确报错。影响行数采用既有内核结果，不承诺 PostgreSQL 完整 DML/事务语义。执行错误暂为 XX000，parser 错误为 42601。性能：继承已有结果全量物化，并在写入前编码完整结果以防部分结果泄露；未做容量基准。未验证全工作区、RealTiKV、libpq/psql 默认 3.0 或完整 PG 事务错误状态。建议任务 5 复用 pg_result::encode/command 接口补充类型与 SQLSTATE 映射。

更新说明（2026-09-30）：完成 Ready 验证与隔离审查，编号任务按要求删除，保留实施证据。

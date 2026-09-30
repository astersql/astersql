# PostgreSQL 第一阶段交付回归

本活文档遵循根目录 PLANS.md；总 plan.md 只读。

## Purpose / Big Picture

让使用 libpq 18 并显式请求协议 3.2 的客户端了解可用 SQL 工作流、启用方式和限制，取得当前完整工作树的 PG 与 MySQL 回归证据。

## Progress

- [x] 阅读任务、总计划、执行/导航技能、根协议与测试指南；批次 7 完成，批次 1/5 待回归按补充授权视为依赖满足。
- [x] 运行初始指定客户端命令：1 测试通过，未观察到需要修复的失败。
- [x] 编写并核对支持边界文档。
- [x] 格式化、PG 与 MySQL 定向回归、lint、差异边界审查。
- [x] 记录完成证据并删除编号任务 8；保留任务 1/5。

## Surprises & Discoveries

pkg/server/doc.go 和 .agents/skills/tidb-verify-profile 不存在。Ready 检查由 AGENTS.md 和 docs/agents/testing-flow.md 决定。已有多任务共享修改，不能把完整 git diff 归为本任务。

初始 cargo test -p astersql-server --lib pg_client_integration_test 退出 0，1 通过（6.99s），日志 /tmp/pg-task8-client-initial.log。任务 7 的历史失败为真实 libpq 拒绝空密码挑战，证据 /tmp/pg-task7-red.log；本交付任务复用其回归，不人为制造失败或重复修改已修复实现。

## Decision Log

- Decision: 本任务只新增支持边界文档与实施记录，不改已通过的协议实现。
  Rationale: 初始指定回归通过；补充授权允许最小必要范围扩大，但没有证据要求扩大。文档限制按实际代码与测试说明。
  Date/Author: 2026-09-30 Codex。

## Outcomes & Retrospective

已完成：当前工作树 Rust 定向回归全部通过，make lint 退出 0 且无错误/警告诊断。新增交付边界文档，无新增生产代码或依赖。总计划与其他任务文件保持原状；主会话可基于各自验证要求恢复任务 1/5。

## Context and Orientation

pkg/server/pg_protocol.rs 处理 startup；pg_conn.rs 管鉴权、取消及简单查询；pg_result.rs 从原始引擎类型编码文本结果；pg_extended.rs 管 Parse/Bind/Describe/Execute/Close/Sync。共享 conn.rs/runtime.rs 提供协议无关执行接口；server.rs 仅接线独立 listener。pg_client_integration_test.rs 使用系统 libpq 18，通过 Python ctypes 调用真实客户端。

## Plan of Work

新增 docs/postgresql-protocol-first-phase.md，说明 postgres-port 默认关闭、仅开发鉴权、显式 3.2、文本类型/参数、事务与不支持项。复用既有分文件测试，无生产代码修复。若出现范围外问题，记录路径和必要性，仅在既有授权覆盖最小直接依赖时处理。

## Concrete Steps

仓库根目录执行 cargo fmt --all；cargo test -p astersql-server --lib pg_；cargo test -p astersql-server --lib postgres_listener_lifecycle；cargo test -p astersql-server --lib real_listener_serves_handshake_ping_select_and_drains_connection；cargo test -p astersql-server --lib mysql_protocol_connection_commands_match_mysql_80；cargo test -p astersql-server --lib mysql_type_packets_expose_correct_type_flags_charset_and_decimal；cargo test -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values；make lint；git diff --check。最终复跑指定客户端命令。

## Validation and Acceptance

真实 libpq 3.2 完成 CRUD、显式类型参数、提交/回滚与 idle cancel，同一 Server 的 MySQL 鉴权连接继续 COM_PING。默认关闭时现有真实 MySQL handshake/ping/select 回归通过。独立 listener 生命周期测试检查启停、端口释放与启动失败清理。PG 相邻测试检查类型、错误、取消与扩展查询。命令失败如与本任务无关须保留编号任务为待回归，不能声称成功。

## Idempotence and Recovery

测试使用临时端口与测试域，可重复执行；没有安装软件或改变外部数据。不得回退共享工作树其他任务修改。格式化影响额外路径时先核对实际差异。

## Artifacts and Notes

日志统一保存 /tmp/pg-task8-*.log。历史失败来自 task-7-execplan.md；新的通过证据必须来自当前工作树。

## Interfaces and Dependencies

没有新增 Cargo 依赖；客户端需要 Python 3 和系统 libpq >=18，可通过 PG_LIBPQ_LIBRARY 指定路径。没有 Go 源码/Bazel 修改、没有新 workspace，不触发 bazel_prepare；Rust 使用动态 failpoint 实现，无 Go failpoint 生成操作。

## 当前验证证据

使用 Ready 档位，因为本任务是第一阶段最终交付。cargo fmt --all 执行两次均退出 0；git diff --check 退出 0。当前工作树没有测试模块排除或临时 harness。

以下命令全部退出 0：

    cargo test -p astersql-server --lib pg_client_integration_test
    cargo test -p astersql-server --lib pg_
    cargo test -p astersql-server --lib postgres_listener_lifecycle
    cargo test -p astersql-server --lib real_listener_serves_handshake_ping_select_and_drains_connection
    cargo test -p astersql-server --lib mysql_protocol_connection_commands_match_mysql_80
    cargo test -p astersql-server --lib mysql_type_packets_expose_correct_type_flags_charset_and_decimal
    cargo test -p astersql-server --lib mysql_prepared_statements_execute_real_sql_with_binary_values
    cargo test -p astersql-cmd-tidb-server --lib postgres_listener_config_projection

PG 全套 20 通过（9.41s）；每个其他作用域 1 通过；最终指定客户端 1 通过（6.95s）。日志 /tmp/pg-task8-client-final.log、/tmp/pg-task8-pg_.log、/tmp/pg-task8-<scope>.log、/tmp/pg-task8-config.log。真实 MySQL 默认关闭回归、PG/MySQL 共存及启动失败端口清理覆盖均通过。

边界证据：rg -n 'pg_|postgres|PostgreSQL|SQLSTATE' pkg/server/conn.rs pkg/server/runtime.rs pkg/session/runtime/control.rs 没有匹配；另查 session.rs 无匹配。审阅 /tmp/pg-task8-shared.diff：只新增协议无关 NativeType 元数据、只读原始类型描述、完成命令后取消复位，没有 PG 报文/OID/SQLSTATE 或 SQL 执行语义变化。审阅 /tmp/pg-task8-entry.diff：配置入口仅投影显式端口，server.rs 仅管理独立 listener/PgService，lib.rs 仅注册源码/分文件测试。client-rust 所有 manifest 使用 v0.4.2-aster.2 tag，无本地 patch；既有 crates.io patch 仅指向远程 git。Go/Bazel 路径 diff 和总 plan.md diff 均无输出。新增文档路径在原始白名单内，本任务未修改生产代码或依赖。

未验证 RealTiKV、生产鉴权/TLS、完整 PostgreSQL SQL/事务语义、默认旧 3.0 客户端、COPY/复制/通知、ORM/JDBC 全组合及性能基准。正确性受现有引擎及有限错误映射约束，兼容性仅覆盖显式 3.2 与文本格式，性能保留全量物化及元数据解析成本，未新增性能改动。

最终 make lint 退出 0，日志 /tmp/pg-task8-lint.log 无 error/warning/failed/find 诊断；最终 git diff --check 退出 0。失败证据已核对任务 7 原日志：postgres_client_workflow 在修复前 0 通过、1 失败，fe_sendauth: no password supplied。本任务初始与最终指定命令均通过，未编造新的 red 阶段。

更新说明（2026-09-30）：完成交付文档、当前工作树 Ready 证据与隔离边界审查；按执行技能删除编号任务 8，保留本活文档作为交接依据。

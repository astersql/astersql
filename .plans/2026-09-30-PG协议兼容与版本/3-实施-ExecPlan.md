# 双协议客户端交付回归实施

遵循根目录 PLANS.md；总计划 plan.md 只读。

## Purpose / Big Picture

系统 libpq 18 默认使用 3.0、显式使用 3.2 时都可完成文本 SQL 工作流、事务和取消，同时 MySQL 保持可用。

## Progress

- [x] 阅读技能与仓库规则，确认批次 2 完成证据。
- [x] 参数化真实客户端完整工作流并更新支持边界文档。
- [x] 完成 Ready 验证及差异审查，所有要求命令退出 0。

## Surprises & Discoveries

pkg/server/doc.go 与 Ready 技能不存在。客户端测试文件已有批次 1/2 修改；本次继续扩展该测试，不回退共享工作区。工作区其他修改属于并行任务。

## Decision Log

- Decision: 复用批次 1 实施 ExecPlan 保存的 startup_protocol_30_roundtrip 真实 TCP 失败证据（退出 101，FATAL 0A000），不回滚共享文件。
  Rationale: 任务允许复用前序网络失败；批次 1/2 已实现协议行为，本次只扩展交付回归。
  Date/Author: 2026-09-30 / Codex
- Decision: 每个协议使用同一真实 libpq CRUD、显式 int4 OID 文本参数、事务和空闲取消流程，保留 pg_conn_test.rs 的确定性执行中取消证据。
  Rationale: 不伪造客户端协议能力，不引入执行内核或客户端依赖变更。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective

双版本完整真实客户端工作流通过，PG 全套 22 测试、listener 生命周期和 MySQL 实监听通过。文档已明确双版本、版本声明和客户端边界。未提交、未创建 PR。

## Context and Orientation

pkg/server/pg_client_integration_test.rs 启动临时双 listener，Python ctypes 调用系统 libpq 18；先完成 MySQL 鉴权，PG 工作流后 MySQL COM_PING 验证连接仍可用。pg_conn_test.rs 保留按版本的活动命令取消窗口。协议兼容声明不是 SQL 语义兼容声明。

## Plan of Work

将 postgres_client_workflow 改名 postgres_client_protocol_versions；不带协议选项的连接断言 30000，带 min/max 3.2 的连接断言 30002。两者校验 server_version 原文及 180000，运行完整已有工作流。docs/postgresql-protocol-first-phase.md 更新双版本、psql 命令、取消密钥和未验证边界。

## Concrete Steps

仓库根执行 cargo fmt --all，cargo test -p astersql-server postgres_client_protocol_versions --lib，cargo test -p astersql-server pg_ --lib，cargo test -p astersql-server postgres_listener_lifecycle --lib，cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib，make lint，git diff --check。

## Validation and Acceptance

上述测试实际执行并退出 0；两个协议完整工作流和 MySQL 共存通过。Ready 用于交付回归，Rust 测试无需 Go failpoint。无 Go/Bazel/依赖变化，不触发 bazel_prepare。不运行 bazel_lint_changed。

## Idempotence and Recovery

测试可重复运行，用临时端口与测试会话，不停止用户实例。失败只记录本任务，不回退其他任务修改。

## Artifacts and Notes

前序失败/通过及依赖完成证据见本目录 1-实施-ExecPlan.md 与 2-实施-ExecPlan.md。本次精确命令与结果将在验证后记录。

## Interfaces and Dependencies

沿用系统 Python 3/libpq 18，没有新增或移植依赖。生产鉴权、TLS、全量 pg_catalog、DataGrip 元数据和 SQL 内核兼容不在范围。

## Final Validation Evidence

Ready 用于交付测试与支持文档。精确命令与退出码：

    cargo fmt --all
    退出 0。
    cargo test -p astersql-server postgres_client_protocol_versions --lib
    退出 0，1 passed，两个协议的完整工作流均执行。
    cargo test -p astersql-server pg_ --lib
    退出 0，22 passed，包含双版本确定性执行中取消测试 startup_cancel_is_scoped_and_consumed。
    cargo test -p astersql-server postgres_listener_lifecycle --lib
    退出 0，1 passed。
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    退出 0，1 passed。
    make lint
    退出 0。
    git diff --check
    退出 0。

差异自审：本次只修改 pkg/server/pg_client_integration_test.rs 和 docs/postgresql-protocol-first-phase.md，新增本实施 ExecPlan 并按交付要求删除编号任务。没有修改生产 Rust 逻辑、MySQL 或执行内核，没有新增依赖。共享 pkg/server/runtime.rs、pkg/session/runtime.rs 未搜索到 PG 引用；server.rs:453 默认 postgres_port 为 None，生命周期测试验证默认关闭与清理。

风险：本次为测试和文档变化，无生产性能影响或新增兼容行为；版本声明只代表协议基线。失败前证据复用前序任务记录，本次未重建旧源码。系统 JDBC 驱动在 /opt/homebrew/share 和 /Users/Shared/work/dir/data 的 jar 搜索中无匹配；DataGrip/JDBC、RealTiKV、全仓库回归、生产鉴权、TLS、完整 PG18/pg_catalog 和性能基准未验证。make lint 使用当前共享工作区的 Makefile 与工具修改，不归属本任务。

更新：已完成全部指定 Ready 检查，批次 3 可交付，总计划未修改。建议后续在实际 DataGrip/JDBC 环境开展限定连接与 SELECT 1 验证。

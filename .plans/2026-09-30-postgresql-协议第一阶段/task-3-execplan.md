# PostgreSQL 启动鉴权与取消实施

本活文档遵循根 PLANS.md，总计划 plan.md 保持只读。

## Purpose / Big Picture


显式 3.2 TCP 客户端通过独立 listener 接收认证、BackendKeyData 和 ReadyForQuery，错误身份被拒绝，取消密钥不能作用于其他会话。

## Progress


- [x] 阅读执行技能、导航技能、总计划、前序证据、测试指南；目标 doc.go 不存在。
- [x] 确认前序任务满足调度授权的依赖条件。
- [x] 新建独立真实 TCP 测试，失败重试出现 E0583 缺 pg_conn 模块。首次被其他任务正在修改的 TTL 编译错误阻挡。
- [x] 实施认证协商、取消、listener 接线及清理。
- [x] 作用域测试、fmt、Ready lint、MySQL 回归和边界审查。

## Surprises & Discoveries


ConcreteTiDBContext::authenticate 只支持 InsecureRootOnly 下 root 空密码；SecureUnsupported 明确拒绝。没有伪造用户校验或静默放行。runtime.rs::cancel 同时设置 cancel_requested 和 SQLKiller，正在执行命令取消后必须清理以免影响下一命令。

## Decision Log


- Decision: 使用现有身份校验，PG AuthenticationCleartextPassword 只接收空密码用于现有 native 空密码桥接；非空密码和其他插件明确拒绝。
  Rationale: 保持当前真实驱动能力，不新增或绕过身份机制。
  Date/Author: 2026-09-30 / Codex
- Decision: 根据用户“继续，允许适当扩大啊”授权加入 conn.rs、runtime.rs、runtime_test.rs 的协议无关 finish_query_cancellation。
  Rationale: cancel_requested 在执行中置位会使下一查询被拒绝；重置会话会改变事务，新增完成清理是最小必要接口。MySQL 不调用新方法，共享模块不引用 PG。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective


本任务完成，所有必要检查通过。SQL 报文编解码属于后续任务，本阶段取消验证通过真实会话边界执行查询与真实 TCP CancelRequest，使用确定性门控保证取消窗口，未声称已完成网络 SQL/长查询端到端验收。验证身份失败、SSL/GSS 的 N 响应、安全模式拒绝、空密码 root、3.0 拒绝、密钥错配/跨会话/闲置/过期无作用、正确取消及查询恢复。双 listener 同时运行及认证中/已认证 PG 连接关闭均已验证。

## Context and Orientation


pkg/server/pg_protocol.rs 提供 3.2 startup 解析；新 pg_conn.rs 管独立连接生命周期。server.rs 只安装服务、传入现有 driver/domain、关闭服务。TiDBContext 是共享会话边界。

## Plan of Work


在 pg_conn_test.rs 新建 startup_auth_roundtrip，先取得缺模块失败。pg_conn.rs 实现有界读取、SSL/GSS 拒绝 N、身份校验、随机 32 字节密钥和取消登记。server.rs 接入 PG listener，关闭时取消会话并关闭 sockets、join workers。增加取消后的完成清理及测试。

## Concrete Steps


从根目录运行 cargo test -p astersql-server startup_auth_roundtrip --lib、cargo fmt --all、PG 定向测试、MySQL 与真实 runtime 定向测试、make lint、git diff --check。

## Validation and Acceptance


Ready：真实 TCP 3.2 返回 R/S/K/Z；3.0、错误身份、非空凭证和安全模式拒绝。仅正确会话和密钥取消正在执行查询；退出/关闭清理密钥与 socket。默认 PG 关闭时 MySQL 定向回归通过。

## Idempotence and Recovery


验证可重复，不修改其他任务内容，不修改 plan.md。取消只作用于活动查询，关闭释放 TCP 和 ID。

## Artifacts and Notes


日志为 /tmp/pg-task3-*.log。失败重试缺模块 E0583；首轮 TTL 错误不归本任务修复。

## Interfaces and Dependencies


PgService::start/close 管服务，with_query 管命令取消窗口。finish_query_cancellation 是协议无关默认接口，真实适配重置 kill 标记。使用已有 rustls 的安全随机源，无新增依赖。

## 最终证据与交接


本任务源码路径：pkg/server/pg_conn.rs、pg_conn_test.rs、lib.rs、server.rs、conn.rs、runtime.rs、runtime_test.rs。无新增 manifest/锁文件依赖。总计划没有 diff；其他会话变更保留。PG 专属报文、密钥和状态仅在 pg_conn.rs；server.rs 仅服务生命周期接线；conn.rs/runtime.rs/conn_stmt.rs 的 rg pg_ 没有结果。

Ready 技能路径不存在，按根 AGENTS.md 选择以下命令（均退出 0）：

    cargo fmt --all
    cargo test -p astersql-server startup_auth_roundtrip --lib
    cargo test -p astersql-server startup --lib
    cargo test -p astersql-server query_cancellation_completion_preserves_session_transaction --lib
    cargo test -p astersql-server concrete_session_driver_authenticates_and_returns_real_sql_results --lib
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    cargo test -p astersql-server postgres_listener_lifecycle --lib
    make lint
    git diff --check

startup 过滤器通过 7 项（PG 连接 4 项及解析 3 项）；其余各通过 1 项。原始日志：/tmp/pg-task3-auth-final.log、startup-final.log、cleanup-final.log、runtime-final.log、mysql-final.log、lifecycle-final.log、fmt.log、lint.log，以上均有 pg-task3- 前缀。初始 cargo 失败首轮被并行 TTL 编译错误阻挡，重试在 /tmp/pg-task3-red-retry.log 包含 E0583 缺少 pg_conn，另有已修复的字段错误。取消清理回归在把新增 finish_query_cancellation 暂时置为空实现时退出 101，/tmp/pg-task3-cleanup-red.log 的真实 SELECT 断言失败；恢复实现后的 cleanup-final 为 0，事务保留。

Rust-only，不触发 Go/Bazel metadata 或 Go failpoint 步骤；未运行 make bazel_lint_changed。未运行全工作区、RealTiKV 或完整 SQL/extended query TCP 客户端回归。正确性风险限于后续查询接线需要调用 with_query；兼容性明确限制为 3.2、当前 bootstrap root 空密码和无 PG TLS，其他身份/能力明确拒绝。性能采用每连接 worker，未进行容量基准；有界 startup/message 长度和完成 worker 回收。

后续实现可调用 PgService::with_query 包围现有 TiDBContext 查询，保持取消只作用于活动命令。finish_query_cancellation 默认接口及真实适配清理，不更改 SQL 语义，不改变 MySQL 调用顺序。客户端基线是显式发送 196610 的真实回环 TCP 客户端，startup user=root，空 PasswordMessage，UTF8；可省略 database 或选择引擎已存在的数据库。SSL/GSS 请求返回 N，要求安全传输时拒绝。

更新说明（2026-09-30）：完成验证并保留唯一实施活文档，按授权删除编号任务文件。

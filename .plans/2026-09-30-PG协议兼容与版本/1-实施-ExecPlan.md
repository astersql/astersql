# PG 双版本握手与取消实施

遵循根目录 PLANS.md。总计划 plan.md 只读。

## Purpose / Big Picture

让 PostgreSQL 3.0 客户端通过真实 TCP 鉴权并接收 4 字节取消密钥，3.2 保持 32 字节。取消必须只影响目标连接的活动命令。

## Progress

- [x] 阅读技能、计划、包规则及测试流程；批次 1 无依赖。
- [x] 新增 3.0 TCP 回归并取得实现前失败（退出 101，收到 FATAL 0A000）。
- [x] 实现版本保存及按登记版本校验取消。
- [x] 扩展双版本确定性取消回归。
- [x] 格式化、聚焦测试、Ready lint 及 diff 审查。

## Surprises & Discoveries

Ready 技能路径不存在，使用根规则。工作区已有其他任务修改，不改动这些文件。pkg/server/doc.go 不存在。

## Decision Log

- Decision: 保留 PROTOCOL_VERSION 作为 3.2 默认值，新增 3.0 常量并在 StartupMessage、Active 保存版本。
  Rationale: 现有调用兼容，取消不根据请求长度猜测版本。
  Date/Author: 2026-09-30 / Codex

- Decision: 必要扩围到 pkg/server/pg_client_integration_test.rs，更新旧的拒绝 3.0 断言为系统 libpq 真实 3.0 握手及空闲取消成功。
  Rationale: cargo test -p astersql-server pg_ --lib 的唯一失败来自旧协议边界断言；保留原 3.2 工作流和无效身份拒绝，不提前实现批次 3 的完整双版本查询回归。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective

已完成双版本握手和按登记版本取消。真实 TCP 测试验证两个版本的长度、错误密钥、跨连接和跨版本、截断/超长、关闭后旧密钥、空闲取消与执行取消后恢复；系统 libpq 18 验证 3.0 握手/空闲取消及既有 3.2 工作流。MySQL 作用域回归通过。任务 1 文件按要求删除，总计划只读。后续批次处理 server_version 与完整双版本客户端查询/取消回归。

## Context and Orientation

pg_protocol.rs 解析 startup 长度、版本和参数；pg_conn.rs 的 PgService 实现真实鉴权、BackendKeyData（进程编号加取消密钥）、活动命令登记和取消。pg_conn_test.rs 已有 channel 同步的命令窗口，无需新增 sleep。

## Plan of Work

先在 pg_conn_test.rs 增加 startup_protocol_30_roundtrip，再让 pg_protocol.rs 接受 196608 和 196610 且保存版本。pg_conn.rs 根据认证连接版本生成随机密钥、登记版本并精确校验。两个独立测试文件补双版本边界和取消测试，保持 MySQL 与执行内核不变。

## Concrete Steps

从仓库根运行 cargo test -p astersql-server startup_protocol_30_roundtrip --lib，先观察 0A000 拒绝导致断言失败，修复后期望通过。运行 cargo fmt --all，cargo test -p astersql-server pg_ --lib，cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib，make lint，git diff --check。

## Validation and Acceptance

两个版本真实鉴权成功，密钥长度分别 4/32，错误、跨连接、长度不符、关闭后的密钥不影响查询；有效执行中取消返回 interrupted，后续查询正常。无 Go/Bazel/依赖变化，无 bazel_prepare 触发。Rust 测试无需 Go failpoint 开关。

## Idempotence and Recovery

测试和格式化可重复运行。只回退本任务四个 PG 文件，保留工作区其他改动。若无关失败阻止 Ready，记录任务状态已完成，待回归；若聚焦验证阻塞则记录已阻塞。

## Artifacts and Notes

实现前 cargo test -p astersql-server startup_protocol_30_roundtrip --lib：1 测试执行并失败，收到 E / 0A000 而非 R / AuthenticationOk，退出 101。

## Interfaces and Dependencies

StartupMessage 增加 protocol_version: u32；Active 增加 protocol_version: u32。复用现有 rustls 安全随机源，不新增依赖。

更新：实现前失败和实现后 3.0 单测通过已取得；首次 pg_ 20 通过、1 旧边界断言失败，make lint 退出 0。扩围修正直接相关 libpq 断言后重跑。

## Final Validation Evidence

Ready 档位：交付行为变化，按根规则执行聚焦 Rust 回归、make lint、diff 审查。

    cargo test -p astersql-server startup_protocol_30_roundtrip --lib
    实现前退出 101，1 failed，FATAL 0A000；实现后退出 0，1 passed。
    cargo fmt --all
    退出 0。
    cargo test -p astersql-server pg_ --lib
    首次退出 101，20 passed / 1 旧 libpq 拒绝 3.0 断言失败；修正后退出 0，21 passed。
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    退出 0，1 passed。
    make lint
    退出 0。
    git diff --check
    退出 0。

改动文件：pg_protocol.rs、pg_conn.rs、pg_protocol_test.rs、pg_conn_test.rs、pg_client_integration_test.rs，均位于 pkg/server。没有修改 MySQL 源码、执行内核、Go/Bazel 或依赖。工作区其他任务的 Makefile / lint-go.sh 等并发改动不属于本任务；make lint 使用运行时工作区规则。

风险：3.0 固定 4 字节取消密钥按官方协议要求，熵低于 3.2 的 32 字节；复用现有安全随机源和常量时间比较。无新增查询路径或可见性能开销。未验证完整仓库回归、TLS、生产鉴权、批次 2 的 server_version 与批次 3 的完整双版本客户端行为。

最终更新：已取得任务所有指定验证证据，未提交或创建 PR。

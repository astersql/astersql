# PG 服务器版本参数实施

遵循根目录 PLANS.md；总计划 plan.md 只读。

## Purpose / Big Picture

真实客户端在 3.0/3.2 握手后取得 18.0 (AsterSQL)，libpq 数值版本为 180000。此值是协议兼容基线，不承诺完整 PG18 功能。

## Progress

- [x] 读取规则并确认批次 1 已完成；新增双版本真实 TCP 测试。
- [x] 实现前失败（退出 101：收到 K 而非 server_version 的 S）、参数实现、libpq 验证。
- [x] Ready 验证与 diff 自审完成。

## Surprises & Discoveries

pkg/server/doc.go 和 Ready 技能不存在。工作区包含其他任务变更，保留这些改动。

## Decision Log

- Decision: 扩围现有 pg_client_integration_test.rs 的两个真实 libpq 握手断言。
  Rationale: 任务要求真实 PQparameterStatus/PQserverVersion 证据；不新增依赖，不改查询流程。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective

双版本握手返回版本参数，系统 libpq 验证原值与 180000。所有指定检查通过，任务 2 文件删除。下一步批次 3 双协议客户端交付回归；不提交、不创建 PR。

## Context and Orientation

pkg/server/pg_conn.rs 的 PgService 握手在认证后发送 ParameterStatus（零结尾名称和值），随后 BackendKeyData 和 ReadyForQuery。测试在 pg_conn_test.rs；系统 libpq 通过 pg_client_integration_test.rs 的 Python ctypes 调用。

## Plan of Work

先新增 startup_server_version_parameter，运行观察缺失参数失败。在既有编码参数后增加 server_version。现有 libpq 两个版本各断言原值与数值解析，MySQL/执行内核不改。

## Concrete Steps

仓库根运行 cargo test -p astersql-server startup_server_version_parameter --lib，先失败后通过。随后 cargo fmt --all；cargo test -p astersql-server pg_ --lib；cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib；make lint；git diff --check。

## Validation and Acceptance

两个协议按 R、三个 S、K、Z 顺序发送，编码仍 UTF8，错误身份仍拒绝；系统 libpq 取得原值及 180000。Ready 要求全部上述命令退出 0。无 Go/Bazel/依赖变更，不触发 bazel_prepare；Rust 测试不需 Go failpoint。

## Idempotence and Recovery

命令可重复运行；仅回退本任务 PG 参数与断言，不回退批次 1 或其他任务。

## Artifacts and Notes

Ready 档位用于交付协议行为变更。精确命令和证据：

    cargo test -p astersql-server startup_server_version_parameter --lib
    实现前退出 101，1 failed；实现后退出 0，1 passed。
    cargo fmt --all
    退出 0。
    cargo test -p astersql-server pg_ --lib
    退出 0，22 passed，包含系统 libpq workflow。
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    退出 0，1 passed。
    make lint
    退出 0。
    git diff --check
    退出 0。

自审确认本任务只增加 pg_conn.rs 的参数和两个独立测试文件的断言；保留版权。总计划只读，MySQL 与执行内核不变。风险：版本是协议兼容声明而非全量 PG18 能力，新增一个短握手消息，性能影响极小。未验证全仓库回归、TLS、生产鉴权和批次 3 完整双版本查询取消交付。

## Interfaces and Dependencies

沿用 ParameterStatus 编码与现有系统 libpq，不增加配置或依赖。

最终更新：完成失败/通过证据、Ready 检查和风险记录。

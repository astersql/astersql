# 独立 PostgreSQL listener 与配置实施

本活文档遵循 PLANS.md；总计划 plan.md 保持只读。

## Purpose / Big Picture


可通过 postgres-port TOML 配置或 --postgres-port CLI 开启独立 TCP 端口，与 MySQL 一起管理启停。默认 None 关闭，显式 0 为操作系统分配端口。本阶段只绑定，连接处理留后续任务。

## Progress


- [x] (2026-09-30) 阅读规则、总计划、执行技能与依赖证据；按调度状态机确认待回归前序可满足实现依赖。
- [x] 定位入口独立 Config 缺口，记录阻塞，获得用户扩大必要范围授权。
- [x] 新增真实 TCP 失败测试并取得缺字段/接口编译失败证据。
- [x] 配置、CLI/文件投影、默认关闭、listener 绑定/关闭/失败清理实现完成。
- [x] cargo fmt --all、PG 及入口定向测试、相邻入口/MySQL 回归、make lint、diff 审查完成。

## Surprises & Discoveries


入口 main.rs:46 导入 stubs::config，与核心 Config 独立。真实入口构建还发现 extract_runtime.rs:138/286 的两个本地 crate 仅在 dev-dependencies。共享 session 的 etcd_client 瞬时依赖失败由其他任务补齐，重试已通过。Ready 技能不存在，按根 AGENTS.md 执行。

## Decision Log


- Decision: 用户明确授权“继续，允许适当扩大啊”后加入 cmd/tidb-server/stubs.rs。
  Rationale: 增加 PostgresPort: Option<u16>、默认 None，维持 canonicalServerConfig 入参的单一配置投影，避免另建全局状态。
  Date/Author: 2026-09-30 / Codex
- Decision: 核心 config.rs 提供 load_postgres_port，只解析新端口字段，main.rs 将结果投影到入口配置，CLI 优先。
  Rationale: 不替换其他 MySQL 配置字段，不引入 PG 协议实现；配置文件读取/解析失败明确报错。
  Date/Author: 2026-09-30 / Codex
- Decision: 将 pkg/server/Cargo.toml 的 meta-model/plancodec 两个已有本地依赖移入正式 dependencies。
  Rationale: 真实入口验证构建 E0433；用户允许最小必要范围修复直接障碍。没有新增外部依赖和本地 patch，没有业务语义变更。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective


所有本任务必要检查通过，任务完成。默认关闭、双端口可连接、端口释放、PG bind 失败清理均由真实 TCP 测试验证；入口配置文件/CLI 优先级和端口越界已验证。下一阶段安装 PG 连接处理、启动鉴权与取消；本阶段不声称能完成 PG 握手。

## Context and Orientation


实际变更为 pkg/config/config.rs、pkg/server/server.rs、pkg/server/server_test.rs、cmd/tidb-server/main.rs、cmd/tidb-server/main_test.rs、cmd/tidb-server/stubs.rs、pkg/server/Cargo.toml。生产源码与测试分文件。server.rs 仅 listener 生命周期接线；conn.rs、conn_stmt.rs、runtime.rs 无新增 pg_ 引用。总计划无 diff，其他会话变更保留。

## Plan of Work


失败测试使用 ServerConfig.postgres_port 和 postgres_listener_addr 验证真实回环 TCP。实现独立 listener 存储、启动绑定及失败清理，关闭和 Drop 释放句柄。可选 u16 配置使缺省关闭并拒绝越界，操作系统 bind 负责端口冲突检查。

## Concrete Steps


从仓库根运行以下命令：

    cargo test -p astersql-server postgres_listener_lifecycle --lib
    cargo fmt --all
    cargo test -p astersql-cmd-tidb-server postgres_listener_config_projection --lib
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    cargo test -p astersql-cmd-tidb-server canonical_listener --lib
    make lint
    git diff --check

失败阶段首条退出 101，7 个 E0560/E0599；实施后首条退出 0（1 passed）。入口 PG 投影 1 passed，MySQL 1 passed，相邻入口 2 passed；fmt、lint、diff check 均退出 0。失败依赖修复后已重试相关检查。

## Validation and Acceptance


Ready 档位：证明 PG None 不绑定、Some(0) 独立绑定、MySQL 同时可连接、close 清空 PG 地址并释放端口，已有端口占用导致启动失败并释放 MySQL。配置测试验证文件设置、CLI 优先、65536 拒绝和未配置 None。MySQL 握手/PING/SELECT/关闭及既有配置投影回归通过。未运行全工作区测试、真实 PG 协议鉴权/SQL 和 real TiKV；不属当前生命周期阶段。Rust-only 无 Go/Bazel 元数据变更，不触发 bazel_prepare，无 Go failpoint 测试。

## Idempotence and Recovery


验证可重复。保持其他会话修改，不提交、不修改总计划。listener 失败和关闭释放句柄，避免端口残留。

## Artifacts and Notes


原始日志 /tmp/pg-task2-red.log、green-final.log、entry-retry.log、mysql-retry.log、adjacent.log、lint.log，均带 pg-task2- 前缀。入口链接器提示测试二进制 unwind section 大，退出码仍为 0。最终 git diff --check 通过，范围经 diff --name-only 复核。

## Interfaces and Dependencies


ServerConfig.postgres_port: Option<u16>，Server::postgres_listener_addr() -> Option<SocketAddr>；核心 Config.postgres_port 与入口 Config.PostgresPort 对应。没有修改 TiDBContext 或 MySQL ClientConn，没有 PG 专属编解码实现迁入共享模块。

# PostgreSQL 协议第一阶段计划

目标：让显式请求 PostgreSQL 前后端协议 3.2 的客户端通过独立端口连接 AsterSQL，并完成基本 SQL、参数查询与事务。

范围：PostgreSQL 前后端协议 3.2 的启动与版本检查、明确的鉴权方式、简单查询、常用类型文本格式、SQLSTATE、Parse/Bind/Describe/Execute/Close/Sync，以及真实 TCP 客户端回归。保持现有 MySQL 端口行为。最终验收以具体客户端执行结果为准。

范围外：协议 3.0 兼容、默认配置的 libpq/psql 客户端、完整 PostgreSQL 语法、`pg_catalog` 全覆盖、COPY、复制协议、通知、二进制格式全覆盖及通用 ORM 兼容承诺。

假设：`pkg/server/conn.rs::TiDBContext` 的查询和预处理接口可作为两种协议的共享执行边界；目前 `pkg/server/server.rs` 只管理 MySQL listener。用户要求仅支持协议 3.2；执行前须确认或制作能显式发送 3.2 startup 的真实客户端，不能以默认请求 3.0 的 libpq/psql 作为验收客户端。

## 设计决策

- 方案 A：独立 PostgreSQL listener 与连接状态机，共用 `TiDBContext`。选择此方案，隔离 MySQL 报文和握手。
- 方案 B：同端口探测协议，少占端口，但涉及现有握手、安全和代理协议路径，暂不采用。
- 方案 C：在 MySQL `ClientConn` 内穿插 PostgreSQL 分支，改动面大且难维护，暂不采用。
- 旧项目 `TiDB-for-PostgreSQL` 七次提交是行为参考，不直接移植 Go 实现；其中类型映射与 TLS 路径有未完善项。

## 架构说明

- `pkg/server/server.rs` 管 listener 生命周期；`pkg/server/runtime.rs` 连接真实会话；`pkg/server/conn.rs` 提供结果与预处理接口；`cmd/tidb-server/main.rs` 将配置传入 server。
- PostgreSQL 使用独立端口，禁止把 MySQL packet framing 用于 PostgreSQL 报文。鉴权先复用现有身份校验，不能静默放行。
- 协议状态、类型 OID、SQLSTATE 必须有明确的不支持响应；3.2 使用变长取消密钥和对应的 BackendKeyData/CancelRequest；不要伪造 PostgreSQL 语义。

## 开发策略

- 对行为变更先写真实 TCP 或纯编解码失败测试，确认失败后实现、格式化并复验。
- 从客户端可观察行为推进；用显式发送 3.2 startup 的客户端验证握手与取消请求，再扩展简单查询和预处理。对 3.0 startup 返回明确的不支持响应。
- 每阶段审查 MySQL 回归；最终使用仓库 Ready 验证要求，包括 `cargo fmt --all`、作用域测试和 `make lint`。若仓库提到的 `.agents/skills/tidb-verify-profile` 仍不存在，按 `AGENTS.md` 和 `docs/agents/testing-flow.md` 的对应要求执行并在交接中说明。
- 计划目录只保存规划；实施时按仓库 `PLANS.md` 创建并维护单独的活文档 ExecPlan，保留 Progress、Decision Log 与验证证据。此 `plan.md` 不在执行中修改。

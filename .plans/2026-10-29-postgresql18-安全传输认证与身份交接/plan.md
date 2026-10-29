# PostgreSQL18安全传输认证与身份交接计划

目标：提供可验证的 PG TLS、安全 SCRAM 交换及共享身份交接，拒绝降级、未授权执行和敏感材料泄漏。

范围：承接后续整体实施大计划15及路线图第7章 TLS/SCRAM 契约；覆盖流所有权、证书、协商门禁、proof、凭证来源、身份授权、期限、清理及独立客户端证据。仅生成小任务，本轮不实现或运行 Cargo、数据库、客户端。

范围外：新密码库、凭证迁移、完整权限体系、PLUS/GSS/OAuth/mTLS登录、直接TLS、复制及通用资源管理器。

假设：静态基线为 de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3；PG仍拒绝TLS且共享驱动只允许开发root。rustls及SQL证书加载已存在；真实SCRAM来源未确认，必须先核验，不能据接口名称认定已接通。

## 设计决策

复用rustls和共享会话；独立PG流封装优于复用MySQL包编解码或新执行内核。普通SCRAM先行，PLUS明确拒绝。凭证缺源只登记依赖，不造密码库。

## 架构说明

Server传递TLS配置给PgService；加密流承载全部协议消息。共享认证提供verifier和匹配身份，成功后交同一会话授权；协议层不造角色权限。

## 开发策略

对行为变更先取得运行期失败，再实施最小修复并用同断言验证通过；已有正确行为只补覆盖。测试代码放独立文件，真实TCP、共享内核和libpq各证明其边界。算法夹具不算生产凭证接入。

本文件生成后只读；活动编号文件按 PLANS.md 维护进度、发现、决策和结果。prompt.md 提供执行顺序与批次，不作为进度来源。所有编号任务均为大计划15的细分，不是新功能授权。

## 来源与证据边界

以 `docs/postgresql-18-core-compatibility-roadmap.md` 的“TLS 与安全传输”“SCRAM 身份与权限”“SCRAM 范围与开发边界”“SCRAM 身份、授权及失败验收”及“SCRAM 后续实施切口”为目标。`docs/postgresql-protocol-first-phase.md` 和来源目录的 `承接与覆盖.md` 提供历史证据与分责。大计划1/2仅消费适用局部接口，14提供CONNECT/USAGE/SELECT事实，19/21消费真实客户端和交付证据。

原引用的 `.plans/2026-10-22-postgresql18-核心兼容蓝图/7-线协议认证与数据通道.md` 以及 `.plans/2026-10-24-postgresql18-首批实施/plan.md`、`prompt.md` 当前不存在；不补造其内容，不重建已删除任务。保留来源任务和覆盖表，不能因生成本子计划而标记实施完成。

当前静态检查：`pkg/server/server.rs` 的 PG 工厂仅传安全开关；`pg_conn.rs` 的所有权和错误出口仍使用 TcpStream；`runtime.rs::ConcreteTiDBContext::authenticate` 仅支持 InsecureRootOnly；`privileges.rs::ConnectionVerification` 存在共享账户/锁定/SSL事实，但未证明有SCRAM来源。生产接线任务的凭证门槛必须由新的仓库证据解除。18.4历史记录不代表18.6实测。

## 验证约束

实施使用WIP聚焦回归、Ready交付检查；`.agents/skills/tidb-verify-profile` 当前缺失，按根AGENTS.md及testing-flow手工选择适用检查，不虚构技能执行。代码交付必须 make lint；不运行 make bazel_lint_changed。Cargo采用仓库10槽位，完整规则在各适用编号任务中。未改变Go/Bazel/依赖时不自动触发bazel_prepare；出现根规则任一触发项则必须先准备。

本轮是计划文档变更，仅检查需求覆盖、路径、依赖、提示和槽位规则；不产生功能、性能、兼容性通过证据。未确认的凭证来源、权限门禁和参考18.6构建均为显式验证门槛。

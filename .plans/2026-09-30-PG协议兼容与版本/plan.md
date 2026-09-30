# PG协议兼容与版本计划

目标：让请求 PostgreSQL 3.0 的客户端连接现有 PG listener，保留 3.2，并返回可解析的服务器版本。

范围：3.0/3.2 startup、按连接版本编码 BackendKeyData 和校验 CancelRequest、server_version 参数、双协议真实客户端查询与取消回归，以及支持边界文档。

范围外：DataGrip 全量系统目录探测、二进制格式、参数类型推断、生产鉴权、TLS、执行内核 PostgreSQL SQL 语义。

假设：现有 PG 独立 listener、真实会话鉴权及文本查询已可用；startup 严格拒绝 3.0，握手未发送 server_version；3.2 当前使用 32 字节取消密钥。

## 设计决策

- 选择按连接版本适配取消报文，共享查询流程；只放开版本检查不正确，全量重写无必要。
- 3.1 是保留号，明确拒绝。版本声明使用带 AsterSQL 标识的 18.0 协议兼容基线，文档限定能力。

## 架构说明

- PG 专属逻辑保持在 pg_*.rs；沿用真实鉴权、取消生命周期及执行接口。
- 不修改 MySQL 路径或 SQL 内核语义，不增加依赖或配置开关。

## 开发策略

- 对行为变更先取得失败证据，再实现、cargo fmt --all 和通过验证。
- 顺序执行批次 1、2、3；共享 pg_conn.rs 与客户端测试，无并行批次。
- 用户本次需求明确扩大原第一阶段的协议范围；不修改旧计划。必要直接依赖可适当扩围，记录实施 ExecPlan 决策并维持隔离。
- 每个实施会话读取根与包 AGENTS.md、PLANS.md、testing-flow；目标包 doc.go 当前不存在。维护单独实施 ExecPlan，本计划只读。
- Ready 运行作用域回归、make lint 与 diff 审查；当前 Ready 技能路径不存在时按根规则选检查。不运行 make bazel_lint_changed；无 Go/Bazel/依赖变更不触发 bazel_prepare。
- 实现完成且仅剩无关验证失败时使用“已完成，待回归”；该状态满足实现批次依赖，最终回归后删除任务文件。

## 规格依据

PostgreSQL 18 官方协议概述：https://www.postgresql.org/docs/18/protocol-overview.html 。3.0 的取消密钥固定 4 字节；3.2 为变长，BackendKeyData 和 CancelRequest 相应不同；3.1 未被实际采用。报文定义：https://www.postgresql.org/docs/18/protocol-message-formats.html 。psql 优先读取 ParameterStatus 的 server_version，否则回退数值版本：https://doxygen.postgresql.org/command_8c.html 。18.0 是本计划选定的协议兼容基线声明，不表示 AsterSQL 实现所有 PG18 功能。

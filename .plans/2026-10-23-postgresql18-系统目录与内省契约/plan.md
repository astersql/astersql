# PostgreSQL 18 系统目录与内省契约计划

目标：将来源蓝图任务 6 拆为单会话可完成的契约文档块，明确真实目录事实来源及客户端内省验收。

范围：只补充 `docs/postgresql-18-core-compatibility-roadmap.md` 第 6 章，覆盖 pg_catalog、information_schema、对象定义/权限/可见性函数、有界求值、元数据更新及客户端需求矩阵。

范围外：不实现生产代码、不运行 Cargo、差分平台或真实客户端、不另建执行器、不改其他章节或来源任务状态。

假设：来源为 `.plans/2026-10-22-postgresql18-核心兼容蓝图/6-系统目录与内省契约.md`；第 1–5 章已有正文，第 6 章为空。18.6 为来源指定目标，本轮未核实该补丁版本发布或运行行为。

## 设计决策

- 选择直接分块补同章；独立草稿增加整合成本，直接拆实现会改变来源范围。
- 每块一个对象/同源字段组/客户端流程，全部串行，保持目标、静态现状及实测支持分开。

## 架构说明

- CatalogQuery 读取共享 TiDBContext、infoschema/model；OID 由 pg_oid 统一，名称与 wire 分层。
- 引用第 2–5 章语法/类型/函数/会话契约，协议与真实客户端运行交第 7、8 章。

## 开发策略

- 纯文档拆分不改变行为，不适用失败测试、Cargo 槽位、构建或代码 lint。实施任务另建遵循 PLANS.md 的 ExecPlan；本计划不属于复杂功能实现。
- 已核对根 Cargo workspace、pkg/server/Cargo.toml 和 lib.rs 的独立测试注册；astersql-server 设置 autotests=false。本文不据此要求运行任何 Cargo 命令。
- 代码导航先读 skills/rustcodegraph/SKILL.md，再用本地索引核实实际符号、接线与独立测试；相关包 doc.go 存在则先读。pkg/server/doc.go 当前不存在。模块说明只能作入口，不能代替源码和运行证据。
- 外部目标行为在执行文档块时查 PostgreSQL 18 官方文档或实际客户端权威材料，记录版本、URL、访问日期；精确 18.6 结论须对应参考构建，不伪造 SQLSTATE、客户端查询或运行记录。
- 字段必须有真实来源或逐项说明空集、NULL、拒绝策略。目标覆盖不能缩水；资料不足保留具体证据缺口，不虚报已支持，也不将缺口扩大成代码修复。
- 使用 Ready 文档适用检查：章节结构、来源覆盖、路径存在性、前后契约、diff。当前 .agents/skills/tidb-verify-profile/SKILL.md 缺失，报告替代检查及限制。
- plan.md 生成后只读；进度、发现、决策、阻塞和完成证据只维护在编号文件。每块文档交付完成不代表功能实现完成。

# PostgreSQL18 模式序列与会话对象语义计划

目标：按真实对象、会话作用域和执行时点补齐大计划11，形成可独立验收的增量任务。

范围：路径设置与解析、精确引用名称、预处理重分析、序列和identity；各族独立客户端差分，序列/identity真实存储正常重连验收。完整场景来自 docs/postgresql-18-core-compatibility-roadmap.md 第5章五个同名契约的模块、目标、范围外、验收、细分正文。

范围外：完整schema/角色/目录/事务/缓存子系统、事务型DDL、serial完整语法、崩溃恢复和性能基准；缺失局部依赖单列，禁止递归扩建。

假设：规划HEAD为 de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3；server为astersql-server，lib.rs注册独立测试，默认features。当前已有有效路径过滤和Extended::reanalyze，不能按历史缺口重复实现。

## 设计决策

- 采用现有会话/共享执行链的局部接线；拒绝另建PG引擎及全局改名。底层精确身份不足先能力拒绝并留缺口。
- 静态整体指南只读；编号ExecPlan维护进度、发现与证据，遵循PLANS.md。

## 架构说明

- PgSession保存连接状态，pg_name绑定，Extended管理statement/portal；ConcreteSession与共享allocator执行。
- 13/14提供真实对象/权限，12提供事务事实；按局部接口交接，不互等整个大计划。

## 开发策略

- 行为变更先取得运行期失败再修复；已正确行为只增覆盖，不能制造红灯。
- 每项只有一个主要验证目标，必要接线与同目录测试一起交付。
- 所有批次串行，批次N对应任务N；这是文件/验证资源互斥，不意味着每项业务都依赖所有前项。具体业务前置见任务文件。
- 原始大计划、路线图及所有plan.md只读；本次仅在来源11登记拆分路径，不改变实施状态。
- 缺失的历史首批plan/prompt及蓝图任务文件不能作为执行必读依赖；使用现存路线图、承接与覆盖、协议首批证据及保留源码。历史94场景仅覆盖18.4显式整数，不代表本计划通过。
- 当前 .agents/skills/tidb-verify-profile 不存在，WIP/Ready按根AGENTS.md及docs/agents/testing-flow.md执行，不宣称调用缺失技能。
- 编号任务完成并删除后，以对应提交、最终交接和evidence记录作为下游证据；文件消失不等于通过。

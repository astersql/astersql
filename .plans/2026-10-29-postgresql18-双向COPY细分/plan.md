# PostgreSQL18 双向COPY细分计划

目标：在共享会话读写链逐项开放COPY IN/OUT三种格式，以真实数据、事务、恢复和资源证据验收。

范围：承接大计划17的语法、text/CSV/binary、Simple/Extended、权限、默认值与约束、原子性、快照、活动取消、关闭、预算、背压及真实客户端；只做对应局部接线。

范围外：完整类型/权限/事务/流式执行器建设、服务器文件/PROGRAM、复制、未取证高级COPY选项。

假设：2026-10-29静态基线为de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3；旧首批计划与蓝图编号文件已缺失，不据删除推断完成。当前运行证据为0。

## 设计决策

采用COPY专用增量编解码及共享读写句柄；拒绝SQL拼接导入和全量物化冒充有界输出。格式逐项开放；依赖缺口显式阻塞，不递归扩建。

## 架构说明

PgService::negotiate与Extended::handle负责协议，pg_sql负责语法，runtime连接共享session读写；新增pg_copy.rs放格式逻辑，独立测试。事务/身份/codec沿既有模块。

## 开发策略

对行为变更先失败验证，再最小实现与同输入通过验证；已有正确行为仅补覆盖，不制造红灯。plan.md生成后只读，编号文件是按PLANS.md维护的活动ExecPlan，包含进度、发现、决策和结果。

每项任务一批，严格按编号执行；无可并行批次。它们共享pg_conn、pg_extended、运行器及全仓fmt/lint，构建槽位不消除源码竞争。外部依赖只要求当前行为必需的接口：2/3类型与编码、6及身份工作流授权、12事务快照、16查询同步、15安全传输、18活动唤醒和冻结预算。18先交局部接口，COPY通道再反馈生命周期证据，不形成等待整个工作流的循环。

## 证据与契约入口

来源：`.plans/2026-10-24-postgresql18-后续整体实施/17-双向COPY数据通道.md`及同目录`承接与覆盖.md`。规范目标以`docs/postgresql-18-core-compatibility-roadmap.md`第7章COPY IN/OUT的证据、真实读写、范围、验收、细分小节为准；`docs/postgresql-protocol-first-phase.md`只提供精确历史证据。

已读RustCodeGraph索引及pg_conn、pg_extended、pg_result相关调用，现有输入落点为`pkg/session/runtime/dml.rs::execute_relational_insert`，工厂为`pkg/server/runtime.rs::ConcreteSessionDriver`。普通查询当前先取得QueryResult再发结果，不能推断流式COPY。`pkg/server`、`pkg/session`均无doc.go；遵守root及server AGENTS。workspace包含astersql-server与astersql-session，server autotests=false，测试通过lib.rs显式注册；本计划使用默认features。

历史蓝图7与首批plan.md/prompt.md缺失，仅使用保留的源码与现有路线图承接，不伪造内容。实施前重新核HEAD及依赖接口；精确PG18.6缺失即保留缺证，18.4不能替代18.6。路线图已有REL_18_0静态边界不冒充运行结果。

## 交付边界

WIP做聚焦红绿验证；Ready按变更类型执行，代码变更先cargo fmt --all并运行make lint。当前`.agents/skills/tidb-verify-profile`缺失，按AGENTS与testing-flow执行等效检查。禁止默认make bazel_lint_changed，Bazel准备按完整触发条件判断。保留PingCAP版权，可用Rust修改增加/保留AsterSQL版权。外部Rust依赖只允许独立上游提交打tag并统一Git tag，禁止本地patch/vendor。

权限/TLS/SCRAM未就绪时，开发root测试只证明数据路径；正式授权验收仍阻塞。不得删减失败、NULL、边界、清理要求。验证记录含完整SHA、版本、输入、SQLSTATE与错误阶段、I/T/E、真实副作用、退出码、有效测试数及资源前后值，敏感凭据与行不无界写日志。RealTiKV仅使用自有后台实例并保证退出清理。

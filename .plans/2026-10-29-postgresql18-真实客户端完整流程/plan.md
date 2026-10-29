# PostgreSQL18真实客户端完整流程计划

目标：用五类真实客户端证明限定业务流程的值、元数据、错误恢复及清理，并保留逐端可复验证据。

范围：来源大计划19；libpq参数化CRUD、NULL、事务、协议与格式；psql脚本及交互；JDBC预处理与阈值；DataGrip真实结构刷新；单一GORM PostgreSQL模型流程。每项只补缺失断言、客户端夹具与必要局部接线。

范围外：重建类型、目录、认证、事务或差分平台；全ORM生态、自动迁移、自动主键、事务DDL、性能基准；本轮实现与运行客户端。

假设：规划HEAD为de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3；已有独立libpq写入入口和真实双listener。历史18.4证据不能证明18.6。旧蓝图及首批计划目录当前缺失，以保留源码、路线图和协议说明承接。

## 设计决策

选择扩展独立测试而非混跑run_clients或新建平台。ORM选现有GORM框架配PG适配器，避免引入第二套语言生态；所有批次串行以避免共享源码、IDE与数据库资源冲突。

## 架构说明

真实客户端→PG listener→ConcreteSession共享内核；MySQL旁路校验同进程健康。测试存储、RealTiKV、参考端和真实UI分别计证。生产文件仅挂载独立测试模块。

## 开发策略

- 先失败验证，再作最小实现，再同输入通过验证；现有实现已正确则只新增覆盖，不制造红灯。
- 本plan.md只读；编号文件按PLANS.md维护进度、发现、决策、结果。prompt.md只存执行提示。
- 每个能力均包含版本冻结、真实结果、恢复与finally清理；缺依赖不降级、不递归补子系统。
- PostgreSQL18.6精确参考构建为目标；其他版本另列诊断结果。缺目标构建不得声称目标兼容通过。
- 开发认证与生产TLS/SCRAM分开列证；后者消费大计划15/16实际接口，缺失登记阻塞，不将开发通过推广到生产。

## 来源与边界

来源为`.plans/2026-10-24-postgresql18-后续整体实施/19-真实客户端完整流程.md`和`docs/postgresql-18-core-compatibility-roadmap.md`第8章五类客户端及各自窄实施切口。`docs/postgresql-protocol-first-phase.md`保留历史局部结果；`docs/rust/rust-postgresql-tikv-runbook.md`提供实存储启动入口，旧binaryTransfer限制须以当前源码和实测核对。

`pkg/server/Cargo.toml`定义astersql-server，lib.rs为库入口、autotests=false；采用默认features。已有pg_client_integration_test模块挂载，新增各客户端独立测试模块需在lib.rs以cfg(test)挂载。start_client_server使用真实ConcreteSessionDriver及InsecureRootOnly；它不是生产认证证明。

GORM固定组合为gorm.io/gorm v1.25.12、gorm.io/driver/postgres v1.5.9、github.com/jackc/pgx/v5 v5.5.5；Go工具链按仓库go.mod的1.25.14。适配器的[固定版本依赖声明](https://raw.githubusercontent.com/go-gorm/postgres/v1.5.9/go.mod)支持此候选组合的依赖核对；实际解析版本和功能仍须执行时采证。[官方连接说明](https://gorm.io/docs/connecting_to_the_database.html)说明PG适配器使用pgx及database/sql池，实施仍须核固定tag源码。

libpq/psql目标客户端固定18.0；JDBC固定42.7.13，Java固定使用实际选定JRE并记录完整build；DataGrip目标2025.1.3/build 251.26094.87及JDBC42.7.13。这些是限定验收配置，不表示本机当前已安装。缺产物记阻塞；更换版本必须形成独立配置证据，不能覆盖固定配置。

# PostgreSQL 协议内省基础完善计划

目标：让 DataGrip 的数据库、schema、tablespace 基础内省读取真实信息；错误可恢复，连接与服务不崩溃。

范围：本轮覆盖用户提供的数据库与 namespace SQL，以及 tablespace 基础查询；建立 PG 独立的目录查询结构，支持这些查询需要的投影、别名、转换、连接与排序，验证简单和扩展协议。补充错误恢复及真实客户端回归。

范围外：完整 PostgreSQL 语法、完整 DataGrip 表/索引内省、生产鉴权、TLS、COPY、外部 Rust 依赖移植。

假设：现有 PG listener 独立；pg_catalog.rs 只识别数据库和事务的固定 token 序列。数据库数据来自 schema_snapshot；日志只显示断连，尚无进程崩溃证据。tablespace 原始 SQL 未提供，不假定其完整语句。

## 设计决策

- 选择 PG 专用查询结构与目录提供器；固定 SQL 分支易漂移，共享 MySQL parser 会混合方言。只复用中立元数据与执行接口。
- 不伪造 owner、注释、tablespace 或事务号；无原生来源的信息保持明确缺失语义。

## 架构说明

- pg_conn/pg_extended 负责协议；pg_catalog 负责目录；schema_snapshot 提供真实数据。MySQL 编解码与共享 parser 不增加 PG 分支。
- 查询与元数据在 PG 边界使用同一结构，Parse 不冻结 Execute 数据。

## 开发策略

- 行为修改必须先失败验证，再通过验证，再重构；生产代码与 Rust 测试分文件。
- 各任务文件承载可更新的执行记录，plan.md 只读。重大实现按 PLANS.md 在对应任务文件维护进度、发现、决策与结果。
- 批次严格串行：1 → 2 → 3 → 4 → 5 → 6，共享文件和 Cargo 资源不能并行。
- Rust 修改先 cargo fmt --all；保留 PingCAP 版权，修复可用的文件顶部添加 AsterSQL 2026 版权。
- Ready 交付需要 make lint；禁止 make bazel_lint_changed。纯 Rust 修改不触发 bazel_prepare；如新增 Go 测试/修改 import 等则按 AGENTS.md 完整判断。
- 仓库没有 .agents/skills/tidb-verify-profile；实施时复查路径，仍缺失则依据 AGENTS.md 与 docs/agents/testing-flow.md 选 WIP/Ready 检查并报告限制。

## 仓库证据与规格

pg_catalog.rs 的 CatalogQuery::classify 比较 DATABASES_SQL/TRANSACTIONS_SQL 的完整 token 序列；execute 从 schema_snapshot 与原生事务数据构造结果。pg_extended.rs 的 Statement/Portal 存储 CatalogQuery，Parse 在通用 parser 前分流。pg_query_test.rs 与 pg_extended_test.rs 已有真实 TCP + canonical session 测试。pg_client_integration_test.rs 通过 Python ctypes 调用 libpq18，且检验 MySQL COM_PING。

参考 MySQL 的 pkg/server/runtime.rs 中 execute_on_session、protocol_result_column 和 pkg/executor/infoschema_reader.rs 的元数据读取方式，不复制协议代码或引入共享 PG 语法分支。pkg/server 与 pkg/session 未发现 doc.go；实施若扩大包范围，必须读取新增目标包的 doc.go，DDL 行为还要读取 docs/agents/ddl/README.md。

官方 PG18 规格：[namespace](https://www.postgresql.org/docs/18/catalog-pg-namespace.html)、[tablespace](https://www.postgresql.org/docs/18/catalog-pg-tablespace.html)、[description](https://www.postgresql.org/docs/18/catalog-pg-description.html)。namespace 是 schema；tablespace 是集群级存储空间；description 以 objoid/classoid/objsubid 定位注释。系统常量可以按规格定义，业务行与身份不得写死。xmin 不能直接冒充 TiKV TSO。

用户原始 namespace 验收语句：

    select N.oid::bigint as id,
           N.xmin as state_number,
           nspname as name,
           D.description,
           pg_catalog.pg_get_userbyid(N.nspowner) as "owner"
    from pg_catalog.pg_namespace N
      left join pg_catalog.pg_description D on N.oid = D.objoid
    order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end

数据库原始语句见 pg_catalog.rs 的 DATABASES_SQL。tablespace 使用结构化基础列查询作为自动回归；最终客户端验收需捕获真实 SQL，缺少该证据不能宣称完整 DataGrip 内省通过。

# `pkg/parser/ast/lib.rs`

## 文件定位

parser AST crate 根，定义真实解析器输出的数据模型、SQL 方言和 visitor 契约；它包含业务节点与遍历逻辑，并非纯装配文件。

## 核心职责

持有语句、表达式、表名、查询与 DDL 等结构；导出 helper 模块和 Node 行为。SqlDialect 区分 MySQL/PostgreSQL，名称与查询节点保留 PostgreSQL 元数据。

## 主要符号

`Node` 暴露文本、动态类型和遍历；`Visitor`/`InPlaceVisitor` 暴露只读、原地写回和替换 hook。`simple_node!` 为具体节点实现 Node。`ShowStmtType`/`ShowStmt` 保存 SHOW 类别、目标、数据库和过滤条件；`CompareMaterializedViewStmt` 保存 ViewName、AsOf、OutputTable。ExprNode、SelectStmt、TableName 等为共享 AST。

## 执行流程

Parser semantic action 构造节点，主解析器返回 Box<dyn Node>。`accept` 执行 enter，按 walk.rs 中的字段顺序访问子节点，再 leave；skip 跳过子节点。`accept_in_place` 支持同类型 enter/leave replacement，并在子节点 false 时停止。

## 数据与状态

COMPARE 的三个子节点以 Option 持有；语法要求 AsOf，程序构造 AST 可留空，Restore 只对必需 ViewName 报错。SHOW 新增 MaterializedViews、MaterializedViewLogs、MaterializedViewRemainLogs、MaterializedViewLogWaitPurge、CreateMaterializedView、CreateMaterializedViewLog。字段沿用现有 DBName/Table/Pattern/Where，不另建平行 AST。

## 依赖与调用关系

parser_actions/mview.rs 构造本提交节点；walk.rs 访问字段，materialized.rs 负责 Restore，sem.rs 负责命令分类，sql_restore.rs 分派。parser_auth、parser_types 等提供身份与类型。Node 的实际解析和访问者路径已接线；ast.rs 的 StatementKind 标签仍是显式分类接口，不能声称有自动运行时分类主链。

## 错误处理与边界

结构本身可表示缺失字段；解析器、Restore 各自验证契约。replacement 类型不匹配会 panic。SQL 文本和结构化 Restore 是不同入口，不以原文回放代替新节点 Restore。

## 并发与资源生命周期

节点遵循 Rust 所有权及各既有内部引用模型；新增 COMPARE 字段无后台任务、全局锁或外部资源。遍历借用节点，原地遍历通过可变借用写回。

## 与 Go 版本的对应关系

来源 af11710fe4550179e8d096cf517462ecf4c7be05 的 dml.go 六类 SHOW 枚举和 misc.go CompareMaterializedViewStmt 在本文件表示；其余逻辑分布到 materialized.rs、walk.rs、sem.rs。Go 生成 visitor inventory 的新增 receiver 对应 simple_node 与 walk 注册；不照抄 Go 225/226 数字。COMPARE 实现 ast::StmtNode 契约，测试放独立 misc_test.rs。保留 PostgreSQL 方言、标识符、参数及既有共享节点，不扩建后续执行器。

## 扩展指南

新增实际 AST 必须同步 simple_node、walk 字段、SEM/Restore 和独立测试；结构变化时检查所有 semantic action。新增 MySQL 特性不能覆盖 PostgreSQL 身份与表达式语义。不要把整个 crate 的历史缺口纳入单个 Go commit 对齐任务。

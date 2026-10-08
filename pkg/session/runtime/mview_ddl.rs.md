# `pkg/session/runtime/mview_ddl.rs`

## 文件定位

本文件属于 `astersql-session` crate 的运行时 DDL 路径，由 [`pkg/session/runtime.rs`](../runtime.rs) 以私有模块 `mview_ddl` 装配。它不是完整的物化视图实现，而是 `CREATE MATERIALIZED VIEW LOG` 在会话层的语义检查、日志表元数据构造和首次清理时间计算入口；实际的持久化与模式发布交给 `Domain`。

SQL 入口位于 `pkg/session/runtime/dispatch.rs::execute_statement`：AST 被识别为 `ast::CreateMaterializedViewLogStmt` 后调用 `ConcreteSession::execute_create_materialized_view_log` 并返回空结果集。文件还提供两个包内时间表达式求值函数，分别服务于建表语句和后续系统会话中的 MLog/MView 调度维护。

`pkg/session/Cargo.toml` 表明该文件编入 `astersql-session`；其直接使用的能力来自本 crate 已声明的 `astersql-parser-*`、`astersql-planner-core(-base)`、`astersql-expression(-exprstatic)`、`astersql-meta-model`、`astersql-meta-metabuild`、`astersql-ddl`、`chrono` 与 `chrono-tz` 等依赖。目标文件没有条件编译项，也没有独立公开 API；三个函数都是 `pub(super)`。

## 核心职责

1. `execute_create_materialized_view_log` 解析目标库表，确认对象存在且是未分区的普通基础表。
2. 校验 `PURGE` 子句：拒绝 `IMMEDIATE`，要求 `NEXT`，并要求 `START WITH`/`NEXT` 表达式静态类型为 DATE、DATETIME 或 TIMESTAMP；随后把表达式恢复成 SQL 文本保存到元数据。
3. 校验显式日志列：禁止重复列、保留元列名、未知列、JSON 和二进制 BLOB；复制基础列类型时清除主键、唯一键、普通索引、自增和 `ON UPDATE NOW` 标志。
4. 追加 `_MLOG$_DML_TYPE` 与 `_MLOG$_OLD_NEW` 两个非空内部列，构造 `CreateTableStmt`，再经 `BuildTableInfoFromAST` 生成日志表 `TableInfo`。
5. 填充 `MaterializedViewLogInfo`，计算首次清理 Unix 秒，并调用 `Domain::ddl_create_materialized_view_log` 原子持久化日志表及其与基础表的关联。
6. 为已持久化的调度表达式提供指定 SQL mode、固定 UTC location 的再次求值函数，供 MLog 清理与物化视图刷新路径复用。

## 主要符号

- `mlog_schedule_unix_seconds(session, expression) -> SessionResult<Option<i64>>`：使用当前 `ConcreteSession` 的 `session_vars` 创建规划上下文，构建无参数标量表达式并调用 `EvalTime`。SQL NULL 映射为 `None`；非 NULL 时间字符串经 `parse_runtime_datetime` 转为 `NaiveDateTime`，再按 UTC 解释并输出 Unix 秒。错误消息区分“构建”“求值”和“无效时间”。
- `mlog_schedule_unix_seconds_with_mode(expression, sql_mode) -> SessionResult<Option<i64>>`：创建独立 `EvalContext`，显式设置传入的 `SQLMode` 与 `chrono_tz::UTC`，不借用调用会话的时区或 SQL mode。该函数用于重放元数据中保存的调度表达式，避免后台维护行为受当前系统会话设置漂移影响。
- `ConcreteSession::execute_create_materialized_view_log(&CreateMaterializedViewLogStmt) -> SessionResult<()>`：本文件唯一的语句执行入口，串联对象检查、子句检查、列定义构造、元数据构造、初始调度计算和 Domain 提交。
- 本文件没有自定义类型、trait、模块常量或静态可变状态；核心输入是解析器 AST，核心中间值是 `ast::CreateTableStmt` 与 `astersql_meta_model::TableInfo`，核心输出通过 Domain 的副作用产生。

## 执行流程

1. 从 `statement.Table` 取目标；缺失时返回错误。未写 schema 时采用 `ConcreteSession::current_database()`，然后由 `resolve_runtime_table` 查找基础表。
2. 用 `MaterializedViewLogTableName(&base.Name)` 推导物理日志表名。若基础表已分区，或其元数据表明它是 view、sequence、MLog、materialized view、shadow 或临时表，则在会话层拒绝。
3. 若有 `statement.Purge`：
   - `Immediate` 直接失败；`Next` 缺失直接失败。
   - 闭包 `validate` 通过 `PlannerBuildSimpleExpr` 构造表达式，读取结果类型，只接受 DATE/DATETIME/TIMESTAMP。
   - 合法表达式通过 `ast::sql_restore::restore_expr` 规范化成 SQL 文本；`PurgeMethod` 记为 `DEFERRED`。没有 PURGE 时三个相关字符串均为空。
4. 从会话状态字符串解析并记录 `PurgeScheduleSQLMode`；`ALERT ROWS` 以 `u64::try_from` 拒绝负数。
5. 遍历用户指定的日志列，使用小写名 `HashSet` 去重并阻止与两个内部元列冲突；从基础表按 `Name.L` 查列。JSON 和二进制字符集 BLOB 不允许进入日志；其余列克隆 `FieldType` 并删除键、自增和自动更新时间标志。
6. 追加 VARCHAR(1) NOT NULL 的 `_MLOG$_DML_TYPE` 和 TINYINT(4) NOT NULL 的 `_MLOG$_OLD_NEW`，保留语句的表选项，构造日志表 AST。
7. `BuildTableInfoFromAST` 生成 `TableInfo`；随后写入包含基础表 ID、跟踪列、清理表达式/SQL mode 和告警阈值的 `MaterializedViewLogInfo`。
8. 若有 PURGE，分别立即求值 `NEXT` 与可选的 `START WITH`。`START WITH` 只有至少晚于当前时间 10 秒时优先；否则退回 `NEXT`。注意 `Option::or(next)` 的语义是：存在但过近/过期的 START 被过滤后采用 NEXT；若 START 或 NEXT 求值为 SQL NULL，则相应结果为 `None`。
9. 调用 `Domain::ddl_create_materialized_view_log(database, base_name, log, next_purge_unix_seconds)`。`pkg/domain/domain.rs` 显示该出口在存储闭包内调用 DDL metadata 层，再发布元数据变更；本文件只把错误包装为 session error。

## 数据与状态

- 读取的会话状态包括当前数据库、`session_vars`、字符串形式的 `state.sql_mode`；目标文件不修改这些状态。
- `purge_start_with` 与 `purge_next` 保存的是恢复后的表达式 SQL，而 `next_purge_unix_seconds` 是创建时求值所得的调度快照。这一区分允许后台以后用保存的表达式继续派生下一次时间。
- `PurgeScheduleSQLMode` 与表达式一起持久化；`mlog_schedule_unix_seconds_with_mode` 在后台重放时重建相同 SQL mode，并固定 UTC。
- 日志表列顺序为用户声明的跟踪列，随后是两个内部元列。列类型从基础列克隆，但不会把基础表上的键、自增或 `ON UPDATE NOW` 属性复制成日志表约束。
- `MaterializedViewLogInfo.BaseTableID` 建立日志到基础表的身份关联；反向关联与日志表持久化不在本文件直接修改，而由 Domain/DDL metadata 层完成。

## 依赖与调用关系

上游调用边：

- `pkg/session/runtime/dispatch.rs::execute_statement` → `ConcreteSession::execute_create_materialized_view_log`，承接用户 SQL 的同步执行。
- `pkg/session/runtime/mlog_purge.rs` 的清理调度解析路径 → `mlog_schedule_unix_seconds_with_mode`，从 `MaterializedViewLogInfo.PurgeNext` 重建表达式后求值。
- `pkg/session/runtime/system_session.rs` 的物化视图刷新信息写入路径 → `mlog_schedule_unix_seconds_with_mode`，复用同一时间表达式求值器处理 `MaterializedViewInfo.RefreshStartWith/RefreshNext`。
- `execute_create_materialized_view_log` 内部两次调用 `mlog_schedule_unix_seconds`，计算创建时的 START/NEXT 候选。

主要下游依赖：

- `plan_context_with_params`、`PlannerBuildSimpleExpr`、`EvalTime`：建立表达式上下文并执行时间表达式。
- `parse_runtime_datetime`（`pkg/session/runtime/relational_value.rs`）：把表达式结果字符串转换为无时区日期时间。
- `ast::sql_restore::restore_expr`：生成可持久化、可再次解析的表达式文本。
- `BuildTableInfoFromAST`：从合成的 `CreateTableStmt` 生成标准表元数据。
- `Domain::ddl_create_materialized_view_log`：进入实际的 DDL metadata 原子变更和发布路径。

RustCodeGraph 能定位三个函数及目标文件被 `dispatch.rs`、`mlog_purge.rs`、`system_session.rs` 等使用，但对这些符号执行 `callers/callees` 未返回边，因此上述精确调用边又以 `rg` 和对应源码位置补证。

## 错误处理与边界

所有可恢复失败都通过 `SessionResult` 返回，不在本文件吞错。来自规划器、表达式求值器、SQL mode 解析、元数据构造和 Domain 的错误经 `session_error` 增加操作上下文；本地业务检查直接构造 `SessionError`。

明确拒绝的边界包括：目标 AST 无表、基础表不存在、分区表、非普通基础对象、`PURGE IMMEDIATE`、PURGE 缺少 NEXT、非日期时间调度表达式、负数 ALERT ROWS、保留或重复日志列、未知列、JSON 列和二进制 BLOB 列。时间表达式返回 SQL NULL 是合法状态并映射为无调度时间；无法解析成运行时日期时间才报错。

本文件没有直接检查日志表重名、数据库存在性、系统库限制、名称长度、placement/charset/collation 继承、功能开关或权限。这些行为不能仅凭 Go 版本推定为此 Rust 函数已完成；部分冲突与原子性由下游 DDL metadata 层处理，完整兼容性须以 Rust 集成路径测试为准。

当前时间通过 `SystemTime::duration_since(UNIX_EPOCH)` 获取；若系统时钟早于 Unix epoch 会报 `read MLog purge clock`。转为 `i64` 使用 `as`，现实时间范围内安全，但没有为极端超范围系统时钟单独防护。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、显式锁或事务，也不持有跨调用资源。规划/求值上下文、AST、列集合和 `TableInfo` 都是栈上或局部拥有的数据；`Arc` 只用于共享现有 `session_vars` 或独立求值上下文，函数返回后释放局部引用。

DDL 的原子性边界在 `Domain::ddl_create_materialized_view_log` 下游：`pkg/domain/domain.rs` 将 `create_materialized_view_log` 放入 `store.with_storage` 闭包，并在成功后发布元数据变更。相关 Rust 独立测试验证日志表、基础表反向关联、清理信息和 notifier 的提交/回滚行为；这些保证不应被误写成本文件自身的锁或事务实现。

时间敏感分支以调用瞬间的 Unix 秒为基准，并用 10 秒阈值避免把即将到期的 START 作为首次任务。扩展此分支时应避免多次取时或隐式采用调用会话时区，防止边界抖动和后台重放不一致。

## 与 Go 版本的对应关系

最接近的 Go 对照是 `pkg/ddl/materialized_view.go` 中的 `BuildMaterializedViewLogTableInfo`、`isValidMaterializedViewLogBaseTable` 和 `executor.CreateMaterializedViewLog`，以及 `pkg/ddl/mview_schedule_expr.go::deriveCreateMaterializedViewLogNextUnixSeconds`。

Rust 与 Go 保持的主要语义包括：只接受普通非分区基础表；跟踪列去重并禁止内部元列冲突；拒绝 JSON/二进制 BLOB；复制列类型时移除键、自增和自动更新标志；追加两个内部列；PURGE 只支持 DEFERRED 且要求 NEXT；保存恢复后的调度表达式、SQL mode、基础表 ID、跟踪列和 ALERT ROWS。

实现分层存在差异：Go `executor.CreateMaterializedViewLog` 显式检查功能开关、当前数据库/schema、系统库、日志表重名，并创建异步 DDL job，之后执行建表后处理；Rust 会话入口构造 `TableInfo` 后直接调用 Domain 的 metadata API。Go 的下一次清理时间由 DDL worker 通过系统会话派生，Rust 创建入口先求值并把结果传给 Domain。Rust 的 `mlog_schedule_unix_seconds_with_mode` 同时被刷新调度复用，这属于当前 Rust 模块边界，不等于 Go 文件的函数归属。

Go 测试 `pkg/ddl/tests/materializedviewlog/materialized_view_basic_test.go` 和 `materialized_view_create_test.go` 覆盖重复/保留列、字段类型、PURGE 类型、非基础对象、UTC/DST、回滚、placement 与告警阈值，适合作为兼容性基线；只有 Rust 测试或实现证实的行为才应宣称已移植。

## 扩展指南

- 新增 `CREATE MATERIALIZED VIEW LOG` 语义检查时，优先放在 `execute_create_materialized_view_log` 构造 `CreateTableStmt` 之前；涉及字段可记录性时同时对照 Go 的 `CheckMaterializedViewLogColumnSupported`，避免 Rust/Go 接受集合漂移。
- 新增持久化字段时，应同时更新 `MaterializedViewLogInfo` 构造、下游 Domain/DDL metadata 序列化与独立 Rust 测试；不要只改会话层临时值。
- 修改调度语义时要同时审查两个求值函数、`pkg/session/runtime/mlog_purge.rs`、`pkg/session/runtime/system_session.rs` 以及创建时 10 秒选择规则，特别验证 SQL NULL、过期 START、SQL mode、UTC、`NOW()` 和日期字面量。
- 修改日志列生成时必须保持内部列顺序、类型/长度/非空约束和被清除的字段标志，并验证 text 与 generated column 不被误判为 BLOB/不支持列。
- 测试逻辑应继续放在独立文件。最近的 Rust 测试是 `pkg/session/runtime/normal_ddl_create_materialized_view_log_test.rs`；会话级新增回归可扩展该文件，后台系统会话行为则同步检查 `pkg/session/tests/system_session.rs`。不要把测试内嵌进本生产文件。
- 兼容风险集中在错误码/消息、对象合法性、时区与 SQL mode；正确性风险集中在基础表—日志表关联和失败回滚；性能风险主要是每次创建/调度重算都会构建并执行表达式，但此文件不在逐行 DML 热路径上。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/session/runtime` 确认目标文件已索引；`node --file pkg/session/runtime/mview_ddl.rs` 阅读全部 281 行；`query` 确认 `mlog_schedule_unix_seconds`、`mlog_schedule_unix_seconds_with_mode`、`execute_create_materialized_view_log`。`callers/callees` 对这些函数无输出，故用精确文本搜索补齐调用边。
- Rust 生产代码：`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/mlog_purge.rs`、`pkg/session/runtime/system_session.rs`、`pkg/session/runtime/relational_value.rs`、`pkg/session/runtime/planning.rs`、`pkg/domain/domain.rs`。
- crate 边界：`pkg/session/Cargo.toml` 的 package、lib、features 与 dependencies；目标逻辑未受 feature gate 包围。
- Rust 独立测试：`pkg/session/runtime/normal_ddl_create_materialized_view_log_test.rs`，覆盖 V1/V2 job、元数据与参数、非法元数据、调度矩阵、清理表缺失回滚、调度错误无孤儿、提交冲突恢复、UTC/SQL mode/会话变量保持和 `DATE_ADD(NOW())`。
- Go 对照与测试：`pkg/ddl/materialized_view.go`、`pkg/ddl/mview_schedule_expr.go`、`pkg/ddl/tests/materializedviewlog/materialized_view_basic_test.go`、`pkg/ddl/tests/materializedviewlog/materialized_view_create_test.go`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务给定命令确认目标文件存在且恰有 11 个固定二级章节，并人工复核文档明确回答文件为何存在、运行路径及安全扩展位置。

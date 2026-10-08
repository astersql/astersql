# `pkg/session/runtime/system_query.rs`

## 文件定位

`system_query.rs` 属于 `astersql-session` crate（见 `pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以 `pub(crate) mod system_query` 装配，并通过 `use system_query::*` 把实现加入 `ConcreteSession`。它位于解析后的 SQL AST 与会话/Domain 元数据之间，负责三类不走普通持久表扫描的行为：SHOW 与常量查询、虚拟系统目录查询，以及若干依赖会话状态的系统函数。

调用入口主要在 `pkg/session/runtime/dispatch.rs`：SHOW COLLATION/PROCESSLIST 分派到 `execute_show_collation`、`execute_show_process_list`，集合操作和 SELECT 分别尝试 `execute_constant_set_operation`、`execute_information_schema_select`、`execute_constant_select`。`pkg/session/runtime/source.rs::ConcreteSession::relational_source` 还会为单源虚拟系统表调用 `execute_information_schema_select`。MySQL `COM_FIELD_LIST` 则沿 `pkg/server/runtime.rs::field_list_on_session` 到达公开方法 `ConcreteSession::field_list`。

本文件不是完整 SQL 执行器。多表系统目录查询、普通用户表查询，以及无法由这里的专用快路径完整表达的语句会返回 `Ok(None)`，让调用方继续走通用关系执行路径（`execute_information_schema_select`、`execute_constant_select`）。

## 核心职责

1. 构造进程内虚拟目录：`build_virtual_system_catalog` 汇合 INFORMATION_SCHEMA 注册表、PERFORMANCE_SCHEMA 注册表与 METRICS_SCHEMA 定义；`virtual_system_catalog` 用 `OnceLock` 缓存结果。`ConcreteSession::metadata_catalog` 再把它叠加到当前或快照 Domain 元数据之上，持久对象同名时优先。
2. 物化系统表行：`execute_information_schema_select` 针对系统表名称读取 Domain、DDL、会话状态、会话管理器、锁/区域状态和 failpoint 注入数据，生成 `HashMap<String, Option<String>>` 行。
3. 执行虚拟行的有限关系操作：`project_virtual_rows` 支持简单 WHERE、ORDER BY（列、别名、序号或表达式）、`COUNT`、通配符、字面量及派生表达式，并补充 `ConcreteResultField` 元数据。
4. 实现协议和 SHOW 元数据：`field_list` 提供 COM_FIELD_LIST 所需的公开列；SHOW COLLATION/PROCESSLIST 生成兼容 MySQL 的列名、类型和标志。
5. 承载会话级系统函数：常量 SELECT 中处理时间、连接身份、受影响行数、LAST_INSERT_ID、用户锁、TiDB key 编解码/MVCC 信息、EMBED_TEXT 等，并把其余表达式交给 `relational_value`。

## 主要符号

- `performance_schema_connection_summary_rows`：把会话管理器的账号连接汇总转换成 `accounts` 行，或按 user/host 聚合为 `users`、`hosts` 行；未知表名返回空集。
- `show_result_fields`：按列名、MySQL 类型码和 flags 构造协议结果字段，补齐默认长度、小数位、字符集和排序规则。
- `project_virtual_rows`：虚拟表公共的筛选、排序、聚合和投影器；内部 `Projection` 区分列、字面量、派生表达式，`OrderExpression` 区分列与任意表达式。
- `information_schema_column_type`、`information_schema_column_size`、`metadata_default_value`、`metadata_column_key`：把 `ColumnInfo` 转换为 INFORMATION_SCHEMA/JDBC 所需的类型、宽度、默认值与键类别。
- `SessionMetadataCatalog`：以 `(小写 schema, 小写 table)` 为键、`TableInfo` 为值的有序目录。
- `build_virtual_system_catalog` / `virtual_system_catalog`：构造并缓存虚拟表定义；构造失败会被缓存并转换为 `SessionError`。
- `ConcreteSession::metadata_catalog`：根据 `snapshot_read_ts`、`snapshot_catalog_version` 或当前统计上下文选择持久目录，再以 `entry.or_insert_with` 补入虚拟目录。
- `ConcreteSession::information_schema_table_visible`：登录用户存在时以活动角色执行 SELECT 权限验证；未认证的内部会话默认可见。
- `ConcreteSession::field_list`：检查当前数据库目标表权限，解析 Domain 表并返回列、表、数据库的规范身份。
- `ConcreteSession::execute_information_schema_select`：本文件最大的分派入口，仅接受单个物理源；覆盖连接汇总、拓扑、锁等待、事务、资源组、计划缓存、TiFlash 副本、分析状态、索引使用、DDL jobs，以及标准 schema/table/view/column/index/constraint/partition/region/deadlock 元数据。
- `ConcreteSession::execute_deadlock_history_select`：从 `RUNTIME_DEADLOCK_HISTORY` 快照生成死锁历史结果。
- `ConcreteSession::{execute_tidb_decode_key,tidb_key_table,execute_tidb_encode_record_key,execute_tidb_encode_index_key,execute_tidb_mvcc_info}`：解析表/分区与索引，编码或解码 TiDB record/index key，并查询运行时提交纪元近似生成 MVCC 信息。
- `RuntimeDeadlockRecord`、`RUNTIME_DEADLOCK_HISTORY`：进程级死锁历史记录和受互斥锁保护的双端队列。
- `RUNTIME_USER_LOCKS`：以 `(domain_id, 小写锁名)` 为键，保存 `(connection_id, 引用计数)` 的进程级用户锁表。
- `ConcreteSession::{user_lock_name,execute_user_lock_function}`：校验 1..=64 字符的非空锁名并实现 GET_LOCK、RELEASE_LOCK、RELEASE_ALL_LOCKS 的所有权/重入计数。
- `ConcreteSession::execute_embed_text`：仅在 starter 部署模式调用 Domain embedding runtime，解析可选 JSON 参数、过滤 `@search` 选项、响应 SQL kill signal，并验证/格式化向量。
- `ConcreteSession::{execute_constant_select,execute_constant_set_operation}`：执行无 FROM 的表达式 SELECT，以及仅含常量 SELECT 分支的 UNION/UNION ALL。

## 执行流程

系统表 SELECT 的主流程如下：

1. `dispatch.rs` 或 `source.rs` 传入已解析的 `ast::SelectStmt`。
2. `execute_information_schema_select` 收集物理表源。源数不是 1 时返回 `None`，避免用单表快路径破坏 Go 版本由通用 Join/Selection/Projection/Sort 执行器提供的多表语义。
3. 对 PERFORMANCE_SCHEMA、METRICS_SCHEMA、SYS，先用 `virtual_system_catalog` 验证定义；当前只有 performance_schema 的 `accounts/users/hosts` 会从 session manager 产生连接汇总，其余已登记表保持正确列契约但可能为空。
4. 对 INFORMATION_SCHEMA，按表名选择数据源。会话/运行时表读取 `ConcreteSession.state`、session manager 或全局锁；拓扑和 DDL 表读取 Domain；标准元数据表读取 `metadata_catalog` 或 `stats_context().catalog()`，并通过 `information_schema_table_visible` 过滤权限。
5. 每个分支把字段统一为小写键和可空字符串值，再调用 `project_virtual_rows`。后者先 WHERE，再计算排序键并稳定排序，随后处理 COUNT 或普通投影，最后生成 `ConcreteRecordSet`。
6. 不属于专用表集合时，只有 `tikv_region_status` 和 `deadlocks` 有尾部处理；其他表返回 `None`，允许通用路径接管。

无表 SELECT 的主流程由 `execute_constant_select` 完成：有 FROM 立即返回 `None`；否则为每个字段匹配字面量、变量或专用函数。NOW 类函数共享一次 `statement_timestamp`，保证同一语句内时间一致；未专门匹配的表达式交给 `relational_expression_value_with_embed`。`execute_constant_set_operation` 逐分支调用它，UNION 通过已有行比较去重，UNION ALL 直接追加，INTERSECT/EXCEPT 明确报错并要求完整集合执行器。

SHOW 流程较短：`execute_show_collation` 从会话变量读取 utf8mb4 默认排序规则，再应用 LIKE/WHERE；`execute_show_process_list` 升级弱引用 session manager，并让 executor 的 `FetchShowProcessListRows` 根据登录用户、PROCESS 权限和 FULL 标志生成行。

## 数据与状态

- 虚拟行采用 `HashMap<String, Option<String>>`：键统一使用小写便于 AST 列名匹配，`None` 表示 SQL NULL，最终用 `CONCRETE_NULL_VALUE` 编码到记录集。
- `SessionMetadataCatalog` 使用 `BTreeMap`，使目录遍历顺序可预测；持久目录与虚拟目录合并时持久表获胜。快照读取优先使用 `snapshot_read_ts`，否则按 `snapshot_catalog_version` 或当前 catalog 取值。
- `VIRTUAL_SYSTEM_CATALOG` 是进程生命周期内只初始化一次的 `OnceLock<Result<...>>`。注册表变化不会在同一进程内自动刷新。
- `ConcreteSession.state` 内的 slow-query/statement-summary 计划、inspection cache、last insert id、found rows、row count、warning 和 timestamp override 会影响结果；访问使用 `RefCell` 借用，作用域被限制在局部块内。
- `cluster_info` 在 inspection cache 存在时使用 `entry(...).or_insert(generated_rows)` 固定本轮检查结果，避免同一检查中拓扑漂移。
- 用户锁和死锁历史是进程级 `LazyLock<Mutex<...>>`；用户锁额外包含 domain 身份，避免不同 Domain 间同名锁冲突。
- `RANDOM_BYTES_CALL` 是 `AtomicU64` 计数器。当前实现提供确定长度的递增字符串兼容路径，不是密码学随机源。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 文件索引与直接入口核验）：

- `pkg/session/runtime/dispatch.rs` → `execute_show_collation` / `execute_show_process_list` / `execute_constant_set_operation` / `execute_information_schema_select` / `execute_constant_select`。
- `pkg/session/runtime/source.rs::relational_source` → `execute_information_schema_select`，用于关系源物化。
- `pkg/server/runtime.rs::field_list_on_session` → `ConcreteSession::field_list`，服务 MySQL COM_FIELD_LIST；`pkg/server/driver_tidb.rs` 定义相应接口边界。
- `pkg/session/runtime/query.rs` → `virtual_system_catalog`，用于虚拟表解析/规划。
- `pkg/session/runtime/dispatch.rs` → `format_runtime_datetime`，复用会话时区格式化。

主要下游依赖：

- `astersql-infoschema`、`astersql-infoschema-perfschema` 提供 INFORMATION_SCHEMA、PERFORMANCE_SCHEMA 与 metrics 注册表。
- `astersql-domain` 及其 stats/info-sync/DDL 能力提供当前或快照元数据、服务器信息、拓扑和 DDL 状态。
- `astersql-privilege-privileges` 执行表可见性与 FIELD_LIST 权限判断。
- `astersql-executor::show` 生成 PROCESSLIST 行；`astersql-session-sessmgr` 提供会话和连接统计。
- `astersql-meta-model`、parser AST/types/mysql 与 `astersql-types` 提供表列模型、表达式、MySQL 元数据和向量类型。
- `astersql-tablecodec` 负责 key 编解码；`astersql-inference` 负责 EMBED_TEXT；`astersql-testkit-testfailpoint` 只为指定兼容测试注入集群/请求数据。

`pkg/session/Cargo.toml` 把这些 crate 作为 `astersql-session` 的直接路径依赖；`nextgen` feature 只转发到部署模式和 kernel type，本文件没有自身的条件编译项。

## 错误处理与边界

- 可继续由通用执行器处理的情况用 `Ok(None)` 表示，而不是报错：无 FROM/有 FROM 不匹配专用路径、多源系统表、非系统 schema、未登记或未专门处理的表。
- 真实失败以 `SessionResult` 传播，并用 `session_error` 增加动作上下文，例如加载快照目录、读取存储级状态、解析时间或 key 参数。
- `project_virtual_rows` 对无表达式投影、未知 ORDER BY 列/越界序号、表达式求值失败返回错误；排序把 NULL 放在升序最前，数字字符串优先按 `i128` 比较，否则词法比较。
- `injected_cluster_info_rows` 对测试注入格式使用 `assert_eq!`/`expect`，因此恶意或错误 failpoint payload 会 panic；它不是面向用户 SQL 的普通输入通道。
- `field_list` 在已认证用户无权限时返回 SELECT denied；表不存在或 Domain 查找失败继续返回带上下文错误。独立 Rust 测试专门覆盖拒绝与授权后成功两条路径。
- 用户锁拒绝 NULL、空名和超过 64 字符的名称；负 timeout 只记录 1292 warning，当前实现不阻塞等待，锁被其他连接持有时直接返回 `0`。
- EMBED_TEXT 限制 2..=3 个参数和 starter 部署模式；NULL 模型/文本传播 NULL；JSON 选项格式、未初始化 runtime、embedding 错误、非法向量维度均返回错误。
- key 编码要求足够参数、存在的表/分区/索引及可解析整数；无法解码的 key 原样返回并追加 warning。`execute_tidb_mvcc_info` 是基于进程内提交纪元的运行时视图，不等同于向 TiKV 发起完整 MVCC 调试请求。
- 集合快路径只支持 UNION/UNION ALL；其他集合算子明确拒绝。

## 并发与资源生命周期

- `VIRTUAL_SYSTEM_CATALOG` 的初始化由 `OnceLock` 串行保证，成功值和失败值都存活至进程退出；读取只返回静态共享引用。
- `RUNTIME_DEADLOCK_HISTORY` 与 `RUNTIME_USER_LOCKS` 通过标准库 `Mutex` 保护；锁中毒时使用 `PoisonError::into_inner` 继续恢复数据。代码只在复制、筛选或一次更新所需的短临界区持锁，没有在持锁期间执行网络 I/O。
- 用户锁所有权绑定 `connection_id`，重入增加引用计数，逐次 RELEASE 递减，RELEASE_ALL 汇总并移除当前连接持有的条目。生命周期清理由调用这些函数的会话行为负责；本文件没有后台清理任务。
- session manager 以 `Weak` 保存；PROCESSLIST 和连接汇总在使用时升级，manager 已释放则返回空结果而非延长其生命周期。
- Domain、embedding runtime 与目录快照均由外部拥有；本文件借用或克隆必要元数据，不创建后台线程。EMBED_TEXT 通过闭包轮询 `sql_killer`，使长调用能够感知取消。
- 原子计数器 `RANDOM_BYTES_CALL` 使用 `Ordering::AcqRel` 保证跨线程唯一递增调用号；这只解决竞争，不赋予随机性。

## 与 Go 版本的对应关系

`pkg/session/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/session"` 明确该 crate 对照 Go `pkg/session`。

最直接的一一对应是 `pkg/session/session.go::(*session).FieldList`：Go 版本读取当前 InfoSchema、以活动角色和 `AllPrivMask` 检查权限、按表名解析表并为每列填写 Column/Table/DB identity；Rust `ConcreteSession::field_list` 保留相同顺序与权限意图，并由 `pkg/session/runtime/system_query_test.rs::field_list_enforces_go_table_privilege_check` 验证拒绝后授权成功。

系统表在 Go 中主要由 `pkg/executor/infoschema_reader.go` 的 memtable retriever、InfoSchema 注册表及普通 Join/Selection/Projection/Sort executor 分工完成。Rust 当前把常用表的行物化与有限投影集中到 `execute_information_schema_select`/`project_virtual_rows`；源码明确在多源时返回 `None`，避免把 Go 的完整关系语义简化为“取第一张表”。这说明该文件是兼容快路径与运行时桥接，不应被描述为 Go 系统表执行框架的完整替代。

函数侧 Go 对照分散在 `pkg/expression` builtin 实现和 executor 中，例如用户锁、TiDB key 函数与 EMBED_TEXT。Rust 版本只在无表常量 SELECT 专用路径中直接匹配这些函数，并对其他表达式回落到统一表达式求值器。`routines`/`parameters` 分支明确承认 stored-program execution 尚未支持，只保留标准列契约；部分已登记 PERFORMANCE_SCHEMA/METRICS_SCHEMA 表也可能返回空行。因此“表已登记”不等于“数据源已完整移植”。

## 扩展指南

- 新增虚拟 schema/table 定义：优先扩展权威 infoschema/perfschema/metrics 注册表；仅对注册表缺失的真实绑定列在 `build_virtual_system_catalog` 做最小覆盖，避免复制整套 schema。
- 新增系统表数据源：在 `execute_information_schema_select` 增加精确表名分支，先调用 `information_schema_table_visible` 做对象级过滤，再用小写行键和 `Option<String>` 表达 NULL，最后统一进入 `project_virtual_rows`。不要让单源快路径接管无法保持语义的 join/CTE/derived table。
- 新增投影或排序能力：修改 `project_virtual_rows` 时同步检查列别名、位置 ORDER BY、NULL 排序、数字/字符串比较、通配符字段元数据和表达式错误传播。
- 新增常量系统函数：在 `execute_constant_select` 中仅处理确实依赖会话/Domain 状态的专用函数；通用纯表达式应留给 `relational_value`。需定义参数数量、NULL、warning/error、取消与状态副作用。
- 扩展共享状态：明确键的隔离维度、所有者和释放时机；临界区不得包围网络或 embedding 调用。若新增测试，应放在同目录独立 `*_test.rs` 文件，不能内嵌到生产源文件。
- 测试入口：FIELD_LIST 语义扩展 `pkg/session/runtime/system_query_test.rs`；系统表/常量函数可在同目录新增或扩展独立测试，并参考 Go 的 `pkg/session/session_test.go`、`pkg/executor/infoschema_reader.go` 邻近测试及 `pkg/infoschema/test`。修改 Rust 行为时应保持 Go 边界和错误意图，不以空结果或桩替代。
- 兼容风险集中在列顺序/类型/NULL 表示、权限过滤、快照一致性和 MySQL 错误文本；性能风险集中在每次查询克隆整个目录、为所有候选行构造字符串 HashMap、内存排序，以及持全局 mutex 扫描锁历史。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/session/runtime` 确认 `system_query.rs`（377 个符号）与 `system_query_test.rs` 均已索引；`query --kind function` 确认 `execute_information_schema_select`、`execute_constant_select`、`field_list`、`project_virtual_rows` 的定义位置。`node --file pkg/session/runtime/system_query.rs` 分段读取了源码及其“used by 23 files”关系。
- 源码：`pkg/session/runtime/system_query.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/source.rs`、`pkg/session/runtime/query.rs`、`pkg/server/runtime.rs`、`pkg/server/driver_tidb.rs`。
- crate 边界：`pkg/session/Cargo.toml` 的 package、feature、porting metadata 与直接依赖。
- Rust 测试：`pkg/session/runtime/system_query_test.rs`，覆盖 FIELD_LIST 权限拒绝、GRANT 后成功和列顺序。
- Go 对照：`pkg/session/session.go::FieldList`；系统表架构对照 `pkg/executor/infoschema_reader.go` 与 `pkg/infoschema` 注册/测试目录；系统函数对照 `pkg/expression` builtin 文件。
- 人工复核结论：该文件存在是为了让会话运行时在不访问普通持久表执行链时提供系统目录、协议元数据和会话函数；安全扩展必须保持单源快路径边界、权限/快照语义、列契约及独立测试。

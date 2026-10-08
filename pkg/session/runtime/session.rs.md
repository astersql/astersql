# `pkg/session/runtime/session.rs` 逻辑说明

## 文件定位

`pkg/session/runtime/session.rs` 是 `astersql-session` crate 内具体会话运行时的聚合文件。模块入口 `pkg/session/runtime.rs` 以私有 `mod session` 装入它，再公开导出 `ConcreteSession`、`CanonicalSessionFactory`、`ConcreteRecordSet`、`BootstrapCanonicalDomain`、`CreateAnalyzeSession` 等生产和测试入口。crate 边界由 `pkg/session/Cargo.toml` 定义，`nextgen` feature 会同步打开 deploy-mode 与 kernel-type 的 NextGen 分支。

生产主链从 `cmd/tidb-server/main.rs` 调用 `CanonicalSessionFactory::from_tikv_store`，再由 `pkg/server/runtime.rs::ConcreteSessionDriver` 持有工厂的共享 `Domain`；每条 MySQL 连接在独立 worker 线程中建立会话。SQL 的解析、规划、执行主体分散在同一 `runtime` 模块的 `dispatch.rs`、`planning.rs`、`query.rs`、`dml.rs`、`transaction.rs` 等文件，本文件负责把这些能力所依赖的会话状态、Domain 生命周期和结果集边界装配起来，而不是单独实现完整 SQL 引擎。

## 核心职责

- 定义会话拥有的可变状态：`SessionState` 汇集事务、临时表、MDL、预编译语句、计划缓存观测、告警、慢日志、统计增量、stale read、优化器开关及协议结果等状态；`ConcreteSessionInner` 汇集共享 `Domain`、`SessionVars`、内存 tracker、`SQLKiller`、会话 binding、连接身份和语句级 RU/CTE 状态。
- 定义生产会话和协议边界：`ConcreteSession`、`ConcreteProtocolState`、`ConcretePreparedArgument`、`ConcreteResultField` 与 `ConcreteRecordSet` 为服务器适配层提供连接状态、参数字面量化、列元数据和逐行消费接口。
- 建立并启动规范 `Domain`：`CanonicalSessionFactory` 从 TiKV store 初始化 Domain、跨 keyspace 工厂、server-info、DDL owner、TTL/MLog 后台任务和外部 workload manager，再为连接创建相互隔离的会话。
- 执行 bootstrap/upgrade：`BootstrapCanonicalDomain` 创建系统库表、回填系统变量和 root 权限、执行版本升级，并写入完成版本；NextGen 还通过 KV metadata 直接创建保留 schema/table。
- 提供真实 KV 支撑的测试入口：`CreateAnalyzeSession` 和 `ConcreteTestRuntime` 使用事务型 mock store，避免把测试退化为固定 SQL 响应桩。

## 主要符号

- `RuntimeTimeZone::parse`：接受 `SYSTEM`、IANA 时区和 `[-12:59,+14:00]` 的定长偏移；返回 `None` 表示非法输入。
- `SessionWarning`：保存 MySQL 告警级别、错误码和消息；`from_error` 沿 error source 链保留 AsterSQL 错误码及全局变量错误码，否则退回通用 1105。
- `SessionState` / `NamedPreparedStatement`：前者是执行路径的会话可变事实源，后者保存 text protocol PREPARE 的 SQL、数据库、计划版本、参数形状和事务上下文缓存信息。`SessionState::default` 给出兼容默认值，例如当前库 `test`、autocommit 开启、隔离级别 `REPEATABLE-READ`、leader read、动态分区裁剪及内建 schema 集合。
- `ConcreteSession` / `ConcreteSessionInner`：外层用 `Rc` 共享同一连接会话的内部状态，并通过 `Deref` 暴露内部能力；`DerefMut` 只允许唯一 `Rc` 所有者。`ConcreteSession::new` 初始化 `SessionVars`、状态、MDL、计划缓存、内存 tracker、killer 和唯一 row-lock owner，随后加载持久化全局变量。
- `CanonicalSessionFactory`：`from_tikv_store[_with_server_info_options]` 是生产构造器，`from_crossks_tikv_store` 是目标 keyspace 构造器，`from_storage_for_test` 仅在测试编译，`create_session` 在共享 Domain 上创建独立会话。
- `ConcreteRecordSet`：拥有列名、可选结果字段和缓冲行；`next_row` 延迟触发 store-read 边界并检查 kill 信号，`close` 清空行并使后续读取失败。
- `SplitSQLStatements`：先用规范 parser 校验完整 SQL，再以和会话执行相同的引号/注释感知逻辑切分多语句；其调用者包括 `pkg/server/runtime.rs::execute_query` 和 `execute_query_streaming`。
- `BootstrapCanonicalDomain` / `CreateAnalyzeSession`：分别负责现有 Domain 的系统元数据引导，以及创建 mock KV Domain 后复用同一引导流程。
- `ConcreteTestRuntime<S, F>`、`RuntimeStore<S>`、`RuntimeDomain`：实现 testkit 的 `TestRuntime`、`TestStore`、`TestDomain` 抽象，显式约束 store 只能 bootstrap 一次、session 必须在 Domain 就绪后创建。
- `AddRecordWithoutAutoIDRebaseForTest`、`CreateDanglingIndexForTest`：面向 Go 回归语义的窄测试桥，分别绕过 auto-ID rebase 写入完整行、仅写二级索引键。

## 执行流程

生产启动流程如下：

1. `cmd/tidb-server/main.rs` 从注册 store 取得 `TikvStore`，调用 `CanonicalSessionFactory::from_tikv_store`；本地 mock 模式则调用 `CreateAnalyzeSession`。
2. `from_tikv_store_with_server_info_options` 读取 keyspace、PD/etcd/TLS 信息并调用 `from_storage` 完成 `Domain::new` 与 `Domain::init`。若有真实 etcd，它注册 server-info、准备 DDL owner/运行时并获取需要的 bootstrap upgrade 分布式锁；NextGen 同时安装跨 keyspace session factory。
3. 工厂调用 `BootstrapCanonicalDomain`。该函数判断 bootstrap 状态和版本，创建 `mysql`/`sys`/`test` 及系统表，运行版本升级、写入 `mysql.global_variables` 与 `mysql.tidb`，创建 sys view，刷新关键 ANALYZE 全局变量，最后发布 store bootstrap version。
4. bootstrap 锁释放后，工厂安装 serving DDL runtime，启动 Domain，协调 Starter bootstrap，初始化 stats、TTL manager、MLog purge worker，并返回可服务的工厂。
5. `pkg/server/runtime.rs::ConcreteSessionDriver` 共享工厂的 `Arc<Domain>`；每条连接的 worker 在该 Domain 上创建 `ConcreteSession`。`ConcreteSession::new` 为连接建立独立 `SessionState`，并从系统表及 Domain 缓存加载持久化全局变量。
6. SQL 执行由同模块其他文件对 `ConcreteSession` 的 impl 扩展完成，结果以 `ConcreteRecordSet` 返回；协议层读取列元数据与行，结束时显式 `close`。

测试路径 `CreateAnalyzeSession` 使用 wall-clock TSO 的内存 KV，关闭 schema/stats lease，初始化 Domain 后调用同一个 `BootstrapCanonicalDomain`，再尽力初始化统计模块，因此测试覆盖真实事务和系统表，而非伪造结果映射。

## 数据与状态

`SessionState` 按生命周期可分为以下几组：

- 连接/语句可见状态：当前库、告警队列、last message/query、协议 capability、affected rows/last insert ID/found rows、慢日志和 statement summary。
- 事务状态：`transaction`、固定的 `transaction_info_schema`、隔离级别与模式、savepoint、锁集合、写键、冲突上下文、commit/start timestamp、临时表事务记录和待提交统计增量。
- 规划状态：prepared statement 映射、实例/会话计划缓存标记、catalog version、optimizer fix control、MPP/TiFlash/paging/动态裁剪等开关。
- 元数据状态：会话局部临时表优先遮蔽 Domain；`mdl_tables`/`mdl_databases` 在首次真实表访问时固定事务可见元数据；stale/snapshot read 会跳过 MDL。
- 资源状态：RU pending/delayed、`SQLKiller`、session/statement 内存 tracker、CTE scope、store-read 延迟错误、pessimistic lock TTL。

共享边界是 `Arc<Domain>`、`Arc<SessionVars>`、`Arc<TransactionMDL>`、`Arc<SQLKiller>` 和线程安全的实例计划缓存；会话局部可变部分主要通过 `RefCell`/`Cell` 存放。`ConcreteSession::clone` 共享同一 `Rc<ConcreteSessionInner>`，不是复制一份状态；`CanonicalSessionFactory::create_session` 才产生拥有独立 `SessionState` 的新会话。

## 依赖与调用关系

RustCodeGraph 对 `BootstrapCanonicalDomain` 给出的直接上游是 `CanonicalSessionFactory::from_tikv_store_with_server_info_options`、`from_storage_for_test` 和 `CreateAnalyzeSession`；其下游包括 `build_bootstrap_view_table`、`runtime_privilege_handle`、`finish_store_bootstrap_version`、`ConcreteRecordSet::next_row` 及 Domain storage/global-variable API。`CreateAnalyzeSession` 的调用者覆盖 session、executor 和独立集成测试；`SplitSQLStatements` 直接被服务器普通/流式查询路径调用。

主要 crate 依赖由 `pkg/session/Cargo.toml` 证明：

- 存储与元数据：`astersql-kv`、`astersql-store`、`astersql-meta*`、`astersql-infoschema*`。
- 生命周期与后台组件：`astersql-domain*`、`astersql-owner`、`astersql-ddl*`、`etcd-client`、`tokio`、TTL/MLog/extworkload 相关 crate。
- SQL 与协议数据：`astersql-parser*`、`astersql-planner-*`、`astersql-executor*`、`astersql-sessionctx-*`、`rust_decimal`、`chrono`/`chrono-tz`。
- 资源与观测：`astersql-util-memory`、`astersql-util-sqlkiller`、statistics、resource group 与 logging crate。

生产调用证据还包括：`cmd/tidb-server/main.rs` 构造工厂；`pkg/server/runtime.rs::ConcreteSessionDriver::new` 克隆其 Domain；`pkg/session/runtime/crossks_runtime.rs` 使用 `from_crossks_tikv_store`；`pkg/testkit/mockstore.rs` 复用工厂和 `create_session`。

## 错误处理与边界

本文件统一返回 `SessionResult<T>`。外部错误通常经 `session_error(context, error)` 或 `SessionError::with_source` 增加动作上下文并保留 source 链；这使 `SessionWarning::from_error` 和 AutoID 回归测试可以识别底层错误类型。

`CanonicalSessionFactory` 的关键失败路径会在返回前调用 `domain.close()`，包括 server-info 注册、跨 keyspace 工厂安装、bootstrap、serving DDL、Domain start、TTL/MLog 启动及 Starter 协调失败。stats 初始化与非 GCV2-worker 的 external workload manager 初始化被设计为记录日志并继续；GCV2 worker 配置错误则是致命错误。bootstrap SQL 大多用 `?` 立即传播，变量迁移事务在错误时尽力 `ROLLBACK`。

明确的输入边界包括：非法时区返回 `None`；非有限浮点 prepared 参数、非法 decimal、auto-ID 溢出、缺少/重复 bootstrap 元数据、非单条或非 DML Starter SQL均返回错误；`ConcreteRecordSet` 关闭后读取报错，store-read 错误会先关闭结果集。`mdl_stats_table` 遇到事务中 public column 身份变化时记录 metadata error，调用方 `register_statement_mdl` 再把它提升为语句错误。

## 并发与资源生命周期

`ConcreteSession` 使用 `Rc`、`RefCell` 和 `Cell`，说明一个具体会话设计为归属于单个连接 worker，而不是跨线程共享；生产服务器正是在每连接线程内驱动它。跨会话/跨线程资源通过 `Arc` 或锁保护：Domain、实例计划缓存、MDL、killer、etcd client 及后台运行时可共享，连接 ID 和 trace count 使用原子类型。

`CanonicalSessionFactory` 的生命周期先 init/bootstrap、后 start/background workers；任一步关键初始化失败都关闭 Domain。`BootstrapOwnerLock` 的 `Drop` 会用自有 Tokio runtime 释放分布式锁，释放失败仅记录告警。`ConcreteRecordSet::close` 取消未消费的延迟 store read 并清空缓冲。`RuntimeStore` 用 `Mutex<Option<S>>` 将 store 所有权一次性移交给 Domain，用 `RwLock<Option<Arc<Domain>>>` 让后续测试会话取得同一 Domain。

MDL 生命周期以事务为界：首次真实表访问调用 `TransactionMDL::begin_table`，核对最新 schema 后 `finish_table`；表消失、非 public 或身份冲突时移除锁。局部临时表、全局临时表、stale/snapshot read 和 restricted SQL有不同跳过规则，扩展这里时不能把所有表统一处理。

## 与 Go 版本的对应关系

主要 Go 对照是 `pkg/session/session.go`：Go `session` 同样持有 txn、Domain/store、`SessionVars`、schema validator、计划缓存、统计 collector、session manager 和异步提交资源；Rust 将大量散布在 Go session/context 中的兼容状态集中进 `SessionState` 与 `ConcreteSessionInner`。Go `bootstrapSessionImpl` 的“检查版本—bootstrap/upgrade—初始化全局变量—创建会话—启动 Domain/后台循环”被 Rust 的 `CanonicalSessionFactory::from_tikv_store_with_server_info_options` 与 `BootstrapCanonicalDomain` 共同承接，但不是逐字段一一复刻：Rust runtime 仍是独立的 concrete ABI，`pkg/session/runtime.rs` 明确说明完整 `sessionapi::Session` 是另一边界。

Go `createSessionWithOpt` 为每个 session 创建变量、表达式/计划/表上下文和事务状态；Rust `ConcreteSession::new` 对应地创建独立状态，同时通过 `Arc<Domain>` 共享存储和 catalog。`pkg/session/runtime_test/session.rs::canonical_factory_shares_domain_and_store_but_isolates_session_state` 验证两个 Rust 会话共享 Domain/数据但保持独立当前库。

时区语义以 `pkg/util/timeutil/time_zone.go::ParseTimeZone` 和 `time_zone_test.go::TestParseTimeZone` 为直接依据：`SYSTEM`、命名时区和 UTC 偏移均受支持，负偏移下限为 `-12:59`、正偏移上限为 `+14:00`。Rust 的 `pkg/session/runtime/session_test.rs::fixed_time_zone_range_matches_go_parse_time_zone` 额外验证边界及畸形符号。

Bootstrap 语义以 `pkg/session/session.go::BootstrapSession`、`bootstrapSessionImpl`、`createSessionsImpl`、`createSessionWithOpt` 为对照。Rust 当前实现聚焦 canonical Domain、系统元数据与已移植后台组件；没有证据表明 Go session 的所有 collector、extension、cursor 或异步 commit 等字段均由本文件完整实现，因此不能把两者描述为完全等价。

## 扩展指南

- 新增会话变量或执行期开关时，优先修改 `SessionState` 默认值、`ConcreteSession::apply_persisted_global_variable`（若为持久化全局变量）以及真正消费该值的同目录执行模块；同步增加独立 `*_test.rs`，不要把测试放入本文件。
- 新增生产启动组件时，接入 `CanonicalSessionFactory::from_tikv_store_with_server_info_options`，为每个失败分支明确决定“关闭 Domain 并失败”还是“记录告警降级”，并验证资源的逆序清理。
- 新增 bootstrap/upgrade 元数据时，接入 `BootstrapCanonicalDomain`、`ensure_canonical_ddl_system_tables`、`upgrade_canonical_domain` 或 `CanonicalBootstrapSchemaRuntime` 中语义最窄的位置，并与 `pkg/session/session.go`、`pkg/session/bootstrap*.go` 及 `pkg/meta/metadef` 的权威 DDL 对齐。
- 修改事务表解析或 MDL 时，从 `local_temporary_table`、`resolve_runtime_table`、`mdl_stats_table`、`register_statement_mdl` 和 `transaction_mdl_schema` 这一组整体评估；风险集中在 stale read、临时表遮蔽、RC 与 RR 的 schema 可见性差异。
- 修改协议结果或 prepared 参数时，同步检查 `pkg/server/runtime.rs` 的 metadata/execute 路径、`ConcreteRecordSet` 关闭语义及 `pkg/session/runtime_test/session.rs::production_record_set_and_connection_state_do_not_depend_on_test_traits`。
- 修改共享/局部状态边界时，保持 `ConcreteSession` 单线程归属和 `CanonicalSessionFactory` 跨会话共享边界；不要仅为可跨线程而把 `Rc<RefCell<_>>` 机械替换为锁，这会改变借用失败、性能与生命周期语义。

兼容风险主要是 Go session 行为漂移、系统表版本不一致和协议状态差异；正确性风险主要是 bootstrap 部分成功、MDL 固定错误或会话状态串扰；性能风险主要是扩大锁范围、重复扫描系统表、在热路径克隆大状态或错误地串行化共享 Domain 操作。

## 验证依据

已核对的生产与配置文件：`pkg/session/runtime/session.rs`（完整 2979 行及符号清单）、`pkg/session/runtime.rs`（模块声明/再导出）、`pkg/session/lib.rs`（crate 入口）、`pkg/session/Cargo.toml`（crate、feature 与依赖）、`cmd/tidb-server/main.rs`（生产工厂入口）、`pkg/server/runtime.rs`（连接驱动和 SQL split 调用者）、`pkg/session/runtime/crossks_runtime.rs`（跨 keyspace 工厂使用）。目标包没有 `pkg/session/doc.go`。

已核对的 Go 对照：`pkg/session/session.go` 的 `session`、`BootstrapSession`、`bootstrapSessionImpl`、`createSessionsImpl`、`createSessionWithOpt`；`pkg/util/timeutil/time_zone.go::ParseTimeZone` 与 `pkg/util/timeutil/time_zone_test.go::TestParseTimeZone`。

已核对的独立 Rust 测试：`pkg/session/runtime/session_test.rs` 的时区、AutoID 失败/成功测试；`pkg/session/runtime_test/session.rs` 的 clone 共享、factory 隔离、真实执行/事务和 production record-set 测试；`pkg/session/runtime/lifecycle_test.rs` 的 Domain/cross-keyspace 生命周期测试。RustCodeGraph 索引状态为 11,467 文件、307,296 节点、1,848,419 边；执行了目标文件列表、`query`（`CanonicalSessionFactory`、`BootstrapCanonicalDomain`、`ConcreteSession`、`ConcreteRecordSet`）和 `node`（`BootstrapCanonicalDomain`、`from_tikv_store_with_server_info_options`、`CreateAnalyzeSession`、`SplitSQLStatements`）查询。精确 `callers BootstrapCanonicalDomain` 查询长时间未返回而中止，其等价调用边由 `node` 的 Trail 和 `rg` 交叉确认。

本任务是纯文档分析，未运行 Cargo。结构验收使用任务文件指定的 11 章节命令；人工复核重点是：现状与理想架构分离、每个重要结论均可回到真实符号/路径、测试保持独立、未声称 Go 全功能已经完成移植。

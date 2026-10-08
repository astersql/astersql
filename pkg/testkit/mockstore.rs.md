# `pkg/testkit/mockstore.rs`

## 文件定位

`pkg/testkit/mockstore.rs` 属于 `astersql-testkit` crate；crate 根模块 `pkg/testkit/lib.rs` 以 `pub mod mockstore` 暴露它，`pkg/testkit/Cargo.toml` 则声明它直接依赖 `astersql-domain`、`astersql-session`、mock storage、TiKV driver 等运行时 crate。它不是生产服务器的存储实现，而是测试代码进入“存储 + Domain + SQL 会话”环境的统一入口。

文件同时提供两种不同层次的后端。`MockStore` 是按规范化 SQL 查表返回预注册结果的轻量 `Database`；`AnalyzeStatsStore` 与 `AnalyzeSessionDatabase` 则在真实 `Domain`、mock KV（或显式 TiKV driver）和 canonical `ConcreteSession` 上执行 SQL，用于需要 DDL、统计信息、事务和运行时观测的集成测试。调用者不应把二者混为一谈：前者没有 SQL 引擎，后者的 store 本身也不执行 SQL，必须先通过 `Database::create_session` 创建会话。

RustCodeGraph 对本文件的文件节点报告 42 个使用文件，示例包括 `pkg/ddl/tests/partition/reorg_partition_test.rs`、`pkg/domain/plan_replayer_test.rs`、`pkg/dxf/importinto/job_testkit_test.rs` 和 `pkg/executor/join/hash_join_v1.rs`，说明它位于多子系统测试的公共底座，而不是应用请求主链。

## 核心职责

1. `MockStore`、`MockStoreConfig` 和 `StoreState` 提供确定性的 SQL 期望注册、失败注入、调用历史与关闭状态，适合隔离测试 `TestKit`/database driver 行为。
2. `AnalyzeSessionDatabase` 把一个非线程自由迁移的 `ConcreteSession` 固定在专用线程，通过 `SessionRequest` 消息转发执行、预处理语句和大量测试观测操作。
3. `AnalyzeStatsStore` 共享一个 canonical `Domain`，为每个用户连接创建独立会话，跟踪活跃会话，并为自动分析惰性保留受限系统会话。
4. `CreateMockStoreAndDomain*`、`CreateTiKVStoreAndDomain` 与 `CreateCrossKeyspaceTestCluster` 组装不同 schema loader、schema lease、真实 TiKV 或跨 keyspace 场景，并把 `AutoAnalyzeExecutor` 注册进 `Domain`。
5. `WithCascades` 和三个 `RunTestUnderCascades*` 包装 planner 测试；当前与 Go 对照一致只运行 Cascades=`off` 的一轮。
6. `GO_STATS_SYSTEM_TABLES` 补建 canonical bootstrap 尚未挂载、但 Go bootstrap 已具备的四张 `mysql` 统计系统表，避免普通 SQL 会话与 restricted stats KV 视图不一致。

## 主要符号

- `MockStoreConfig { cluster_id, keyspace, path, cascades_planner }`：轻量 store 的配置载体，默认 cluster ID 为 1，其余为空或关闭。当前 `MockStore` 只保存并通过 `config()` 返回配置，不据此启动真实集群。
- `MockStore`：公开的轻量 `Database` 实现。`expect_query`/`fail_query` 与 `expect_execute`/`fail_execute` 写入结果表，`history`/`clear_history` 管理原始 SQL 和参数的调用记录。
- `normalize_sql`：把任意连续空白折叠成单空格并转小写，仅用于注册结果的 key；历史仍保存调用者传入的原始 SQL。
- `SessionRequest`：专用线程协议，除 `Execute`、`Prepare`、`ExecutePrepared`、`DropPrepared`、`Close` 外，还承载 MemTracker、事务、replica read、stale read、鉴权、inspection cache 等测试探针。每个需要结果的请求携带容量为 1 的同步回复通道。
- `AnalyzeSessionDatabase`：公开的会话级 `Database` 适配器，持有生命周期锁、共享 `Domain`、`SQLKiller` 和 connection ID；真正的 `ConcreteSession` 只存在于 worker 线程。
- `ActiveSessionGuard`：worker 退出时以 RAII 方式递减用户活跃会话计数。
- `AnalyzeStatsStore`：公开的会话工厂和 `Domain` 容器；`state` 保存关闭标志、用户会话弱引用、最近会话弱引用和惰性 auto-analyze 会话。
- `CrossKeyspaceTestCluster`：以 `BTreeMap` 保持 keyspace 顺序确定，多个 store/domain 共享一个 `CrossKeyspaceCoordinator`。
- 公开构造入口：`CreateMockStore`/`CreateMockStoreWithConfig` 创建轻量 store；`CreateAnalyzeStatsStore`、`CreateMockStoreAndDomain`、`CreateMockStoreAndDomainWithSchemaLease`、`CreateMockStoreAndDomainV2` 创建 canonical 环境；`CreateTiKVStoreAndDomain` 显式连接 TiKV；`CreateCrossKeyspaceTestCluster` 创建 SYSTEM 加自定义 keyspace 集群。
- `protocol_arguments`、`drain_record_set`、`execute_concrete_session`：分别完成 `DbValue` 到协议参数的无损类型映射、结果集排空/关闭、SQL 执行与 DML 报告收集。

## 执行流程

轻量路径从 `CreateMockStore*` 返回 `Arc<MockStore>`。测试先注册规范化 SQL 的结果；`Database::execute` 或 `query` 获得 `StoreState` 锁，检查 `closed`，记录原始 SQL/参数，再按 `normalize_sql` 查表。未注册的 execute 返回默认 `ExecutionResult`，未注册的 query 明确报错；`close` 只设置关闭标志。

canonical mock 路径从 `CreateMockStoreAndDomain` 开始。`AnalyzeStatsStore::new` 调用 `AnalyzeSessionDatabase::bootstrap`，后者在 worker 中执行 `CreateAnalyzeSession`；bootstrap 会话补建 `GO_STATS_SYSTEM_TABLES`。`from_bootstrap` 取出共享 `Domain`，登记三个测试用 runtime topology 节点，关闭并 join bootstrap worker，再初始化空会话表。构造函数随后把 store 的弱 trait 引用注册为 `Domain` 的 `AutoAnalyzeExecutor`。

用户侧 `Database::create_session` 调用 `open_session`：在 store 锁内拒绝已关闭实例、清理失效弱引用、分配递增 connection ID，然后 `AnalyzeSessionDatabase::from_domain` 启动新 worker。worker 创建 `ConcreteSession`、设置 connection ID、取得 `SQLKiller`、增加活跃计数，并进入 `receiver.recv()` 循环。调用线程通过 `request` 或 `send_test_request` 发出消息并同步等待结果，因此所有 session 状态都在所属线程内访问。

执行普通 SQL 时，`execute_concrete_session` 对无参数 SQL 直接调用 `ConcreteSession::Execute`；有参数 SQL 则临时 prepare、把 `DbValue` 映射成 `ConcretePreparedArgument`、执行并关闭 statement。随后 `drain_record_set` 逐行消费并关闭每个结果集，把兼容层的 `"<nil>"` 或 `CONCRETE_NULL_VALUE` 恢复成 `DbValue::Null`，最后读取 `LastDmlReport` 形成 `ExecutionResult`。`Database::execute` 丢弃结果集；`query` 只返回第一个结果集并在没有结果集时报错，这与 Go TestKit 语义对齐。

内部 SQL 通过 `execute_internal`/`query_internal` 暂时设置 `InRestrictedSQL=true`，执行后恢复为 false。自动分析则经 `AutoAnalyzeExecutor::execute_auto_analyze` 惰性创建 connection ID 0 的长期受限会话并执行 SQL；该会话使用独立计数器，不计入用户 `active_session_count`。

其他构造分支只替换 bootstrap：显式 lease 与 `KvInfoSchemaLoader` 进入 `new_with_schema_lease`；InfoSchema v2 使用 `KvInfoSchemaLoader::new_v2`；TiKV 入口用 `TiKVDriver::Open` 和 `CanonicalSessionFactory`，失败直接返回 `TestError`，不会退回内存 store；跨 keyspace 入口为各配置创建真实独立 store/domain，再绑定共享 coordinator。

## 数据与状态

轻量 `StoreState` 的所有字段都在同一 `Arc<Mutex<_>>` 下：`closed` 是单向关闭标志，两张 `HashMap` 保存克隆后可返回的 `TestResult`，`history` 按调用顺序保存 SQL 与参数。相同规范化 SQL 的后注册值覆盖旧值；query 与 execute 使用不同结果表。

canonical store 的持久业务状态并不在本文件中；目录、统计、事务和 KV 数据属于共享 `Domain`/storage，`AnalyzeSessionDatabase` 仅持有访问桥。`AnalyzeStoreState::sessions` 和 `latest_session` 使用 `Weak`，避免 store 与用户 session 相互持有；`auto_analyze_session` 使用强引用，以对齐 Go 内部系统会话池的 Domain 生命周期。

`next_connection_id` 从 1 开始以 `Relaxed` 原子递增，connection ID 0 留给 bootstrap/自动分析；`active_sessions` 在 worker 成功启动后以 `AcqRel` 增加，在 `ActiveSessionGuard::drop` 中减少，对外以 `Acquire` 读取。`runtime_topology` 是固定的三个 `(store_id, address)` 测试节点，只用于 runtime region 观测。`GO_STATS_SYSTEM_TABLES` 固定保存四张表的 Go canonical DDL。

## 依赖与调用关系

上游通常经 `pkg/testkit/lib.rs` 的 `mockstore` 模块或 `NewTestKit(store)` 使用本文件。RustCodeGraph 将文件节点关联到 42 个调用文件；精确符号查询还显示 `CreateMockStoreAndDomain` 被统计、DDL、executor、domain 等测试广泛引用。`pkg/testkit/mockstore_test.rs` 直接验证它与 `Database` 的 prepared-statement 接口组合。

下游主链为：`CreateMockStoreAndDomain*` → `AnalyzeStatsStore::{new,from_bootstrap}` → `AnalyzeSessionDatabase::{bootstrap,spawn}` → `CreateAnalyzeSession` 或 `Domain::new` → `ConcreteSession`。执行链为 `Database::{execute,query}` → `request` → `SessionRequest::Execute` → `execute_concrete_session` → `ConcreteSession::{Execute,prepare_protocol_statement,execute_protocol_statement}` → `drain_record_set`。RustCodeGraph 的 `callees execute_concrete_session` 明确给出 `drain_record_set`、`ExecutionResult` 与 `SessionReply` 边。

Cargo 边界由 `pkg/testkit/Cargo.toml` 证实：canonical 路径直接依赖 `astersql-domain`、`astersql-session`、`astersql-store-mockstore-mockstorage` 和 `astersql-store-driver`；`Database`、`DbValue`、`QueryRows`、`PreparedResultField` 则来自同 crate 的 `pkg/testkit/db_driver.rs`。没有条件编译项位于目标文件内；仅 crate 根在 `#[cfg(test)]` 下挂入独立测试模块。

## 错误处理与边界

可恢复的运行时错误统一转换成 `TestError`：SQL/prepare/record-set 错误保留字符串信息，关闭后的请求得到 `"canonical analyze session is closed"`，worker 丢失回复和 worker panic 分别有专用消息。`drain_record_set` 在 `Next` 失败时仍尝试 `Close`，优先保留读取错误；正常排空后，`Close` 错误会向上传播。

bootstrap 属于测试环境建立的硬前置条件，失败会在 `spawn` 中 join worker 后 panic；补建统计表失败也会 panic。生命周期锁中毒同样使用 `expect`，因为这表示测试基础设施内部不变量已破坏。轻量 store 的状态锁也采用相同策略。

重要边界包括：轻量 execute 未注册时默认成功，而 query 未注册时报错；canonical store 不能直接 execute/query；canonical query 只暴露首个结果集；内部执行只在单次请求期间切换 restricted 标志；`latest_session` 仅指仍有强引用的最近用户会话，因此依赖“最近会话”的探针在没有活跃会话时返回错误或由 `sql_killer` panic；TiKV 路径不做静默降级。

有参数的便捷执行在成功执行后关闭临时 statement；若协议执行本身返回错误，当前 `?` 路径在到达 `close_protocol_statement` 前返回，因此新增错误清理语义时必须结合 session protocol 层确认，而不能只修改文档或测试适配器。

## 并发与资源生命周期

每个 `AnalyzeSessionDatabase` 独占一个 OS worker 线程，调用侧可通过 `Arc` 共享，但请求在 mpsc 队列中串行执行，保证 `ConcreteSession` 不跨线程访问。请求通道无界，回复使用 `sync_channel(1)`；调用者发送后同步等待，因此单个 API 调用具有请求/响应边界。

`shutdown` 在生命周期锁内原子地 `take` worker 和 sender，首次调用发送 `Close` 后在锁外 join；后续调用因 worker 已为空而成功返回，故显式 `close` 和 `Drop` 幂等。worker 中的 `ActiveSessionGuard` 确保正常关闭、通道断开或提前退出均减少计数。`AnalyzeStatsStore::shutdown` 先在锁内设置 closed、清空最近会话并取出强/弱引用，再在锁外逐个关闭 worker，避免持有 store 锁等待线程。

store 只弱持有用户 session，所以用户释放最后一个 `Arc` 会触发 session `Drop` 并 join。相反，store 强持有 auto-analyze session，直到 store shutdown。`pkg/testkit/mockstore_domain_stats_test.rs::analyze_session_workers_join_on_explicit_close_and_drop` 验证显式 close、Drop、store close 的计数归零和幂等性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/testkit/mockstore.go`。两端都有 `WithCascades`、三个 `RunTestUnderCascades*`、`CreateMockStoreAndDomain` 与 schema-lease 变体；Go 当前的 Cascades helper 也只列出 `off`，Rust 注释追踪了移除第二轮的 Go commit。Rust `test_callback_caller` 用闭包类型名近似 Go 的 `runtime.Caller`。

Rust canonical 路径保留 Go 的核心测试意图：创建 mock KV、bootstrap Domain、让多个会话共享同一 Domain、为内部工作使用 restricted session，并在清理时关闭 Domain/store 相关资源。`GO_STATS_SYSTEM_TABLES` 明确以 Go `bootstrapTables` 为规范补齐统计表。Rust 的 `CreateMockStoreAndDomainWithSchemaLease` 与 Go 同名入口都传播 lease。

二者并非逐行同构。Go `CreateMockStore` 还处理 `WithTiKV` flag、bootstrap image、全局配置、owner manager、gctuner/view cleanup 和 mockstore options；Rust 把真实 TiKV 分成显式 `CreateTiKVStoreAndDomain(path)`，没有 Go 的 `testing.TB` cleanup 或可变 opts。Go 的 `DistExecutionContext`、bootstrap image 与 `SetTiFlashReplica` 不在此 Rust 文件中；Rust 额外提供线程固定 adapter、细粒度 runtime 探针、InfoSchema v2 和 `CrossKeyspaceTestCluster`。因此扩展时应对齐行为目标，而不是假设 API 一一对应。

## 扩展指南

新增轻量 SQL 模拟行为时，优先扩展 `MockStore` 的注册表与同目录独立测试，不应把 parser/catalog 逻辑塞进它。若需要新的 `Database` 测试探针，应在 `pkg/testkit/db_driver.rs` trait 增加接口后，于 `SessionRequest` 增加请求、在 worker match 中访问 `ConcreteSession`、在 `AnalyzeSessionDatabase` 增加发送包装，并评估是否需要 `AnalyzeStatsStore` 的“最近会话”转发；这四处必须保持类型和错误语义一致。

新增 bootstrap 模式应复用 `spawn`/`from_bootstrap`，只替换 storage、schema loader 或 config，继续保证 bootstrap worker 被关闭、runtime topology 被登记且 auto-analyze executor 被注册。新增系统表时先验证 Go `bootstrapTables` 与 canonical session bootstrap 的真实差距，再更新 `GO_STATS_SYSTEM_TABLES`，避免重复或漂移。

修改参数协议或结果转换时必须同步检查 `protocol_arguments`、`execute_concrete_session`、`drain_record_set` 和 `pkg/testkit/db_driver.rs` 的扫描语义，特别关注 SQL NULL、bytes/数值类型、多个结果集、statement 清理和错误优先级。修改生命周期时必须保持 `shutdown` 幂等、锁外 join、弱引用用户会话及 auto-analyze 不计入用户计数这些不变量。

测试逻辑应继续放在独立文件，而非目标源文件。至少同步 `pkg/testkit/mockstore_test.rs`（prepared statement 生命周期）、`pkg/testkit/mockstore_domain_stats_test.rs`（共享统计状态和 worker 生命周期）、`pkg/testkit/mockstore_domain_ddl_test.rs`（DDL/跨 keyspace）及 `pkg/testkit/mockstore_domain_infoschema_v2_test.rs`（v2 loader）；涉及 Go 对齐时同时核对 `pkg/testkit/mockstore.go`。兼容风险集中于 TestKit 公共 API、Go 行为差异和全局 Domain 状态；性能风险主要是每会话一线程、同步请求等待及惰性长期系统会话。

## 验证依据

- 源码全貌：`pkg/testkit/mockstore.rs`，核对了模块常量、公开构造函数、两个 `Database` 实现、`AutoAnalyzeExecutor`、跨 keyspace 类型、消息枚举及 Drop/关闭路径。
- crate 与模块边界：`pkg/testkit/Cargo.toml`、`pkg/testkit/lib.rs`；该目录没有 `doc.go`，因此以 crate 根注释和 Cargo 声明为最近权威边界说明。
- Go 对照：`pkg/testkit/mockstore.go`，重点核对 Cascades helpers、mock/domain bootstrap、schema lease、TiKV、资源清理和缺失/新增能力。
- 独立测试：`pkg/testkit/mockstore_test.rs`、`pkg/testkit/mockstore_domain_stats_test.rs`、`pkg/testkit/mockstore_domain_ddl_test.rs`、`pkg/testkit/mockstore_domain_infoschema_v2_test.rs`；分别提供参数化语句释放、共享 Domain/会话线程生命周期、DDL/跨 keyspace、InfoSchema v2 的行为证据。
- RustCodeGraph：`status` 显示索引含 11,467 文件与 307,296 节点；`files --filter pkg/testkit` 确认目标、Go 对照和测试均已索引；`query AnalyzeStatsStore`、`query AnalyzeSessionDatabase`、`query CreateMockStoreAndDomain` 定位主要符号；目标文件节点报告 42 个使用文件；`callees execute_concrete_session` 给出到 `drain_record_set`、`ExecutionResult`、`SessionReply` 的边。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文恰有“文件定位”至“验证依据”共 11 个固定二级标题；人工复核重点是两种 store 的区别、canonical 执行链、错误边界、线程/引用生命周期和 Go 差异均有上述源码或测试依据。

# `pkg/session/runtime/crossks_runtime.rs`

## 文件定位

该文件属于 `astersql-session` crate 的 `runtime` 模块，由 `pkg/session/runtime.rs` 以 `pub mod crossks_runtime` 装配。它不是跨 Keyspace 管理策略本身；管理器、句柄和通用 `SessionManager` 位于 `pkg/domain/crossks/cross_ks.rs`。本文件提供面向生产环境的具体工厂和 DDL 后端，把目标 Keyspace 的 Store、Domain、系统会话池、etcd 命名空间、Schema 同步器、DDL Owner 与最小 Job ID 刷新循环组装成通用 `astersql_domain_crossks::SessionManager`。

正常服务器启动时，`pkg/session/runtime/session.rs` 在同时取得 PD 与 etcd 地址后，通过 `KeyspaceSessionFactory::install_on_domain` 接入跨 Keyspace 能力；本文件的 `CrossKSProductionRuntimeFactory::install_on_domain` 是该生产装配链上的具体安装入口。测试还可直接构造该工厂，但 `with_clients` 仅在 `cfg(test)` 下存在。

## 核心职责

1. `CrossKSProductionRuntimeFactory` 保存 PD/etcd/TLS 配置，并按目标 Keyspace 延迟创建运行时。
2. `target_real_etcd` 先通过 PD 解析 Keyspace 数字 ID，再建立带 `/keyspaces/tidb/{id}` 前缀的 etcd 客户端，保证虚拟服务器注册、DDL 通知和选主落在目标 Keyspace 的命名空间。
3. `create_runtime` 依次创建目标 Store、规范会话工厂与 Domain、跨 Keyspace 会话池、Schema/Server State 同步设施、系统表管理器、最小 Job ID 刷新线程、DDL 提交器和 DDL Owner，最终交给通用 `SessionManager` 管理。
4. `CrossKSProductionDdlBackend` 将通用跨 Keyspace DDL 客户端所需的元数据解析、会话变量读取、Server State 刷新、Job 提交、Owner 通知与历史 Job 查询映射到 session/runtime 中的具体实现。
5. 所有中途失败路径都在返回错误前回收已经创建的组件，避免残留 Store、Domain、线程或虚拟服务器注册。

## 主要符号

- `type StoreOpener`：`Fn(&[String], &str) -> Result<Arc<CrossKSStore>, ManagerError>`，按 PD 地址和 Keyspace 打开目标 Store；用 trait object 便于测试注入失败。
- `type EtcdProvider`：按 Keyspace 提供 `Arc<dyn EtcdClient>`；生产默认不用该注入点，测试用内存客户端绕开真实 PD/etcd。
- `CrossKSProductionRuntimeFactory`：公开、可克隆的生产工厂。字段包括端点、可选 TLS 文件、Store/etcd 提供器，以及 `pending_etcd` 临时缓存。
- `new`：建立默认生产配置；默认 `store_opener` 调用 `open_target_store_with_tls`。
- `with_clients`：仅测试可见，替换 Store 和 etcd 创建逻辑。
- `target_real_etcd` / `target_etcd`：解析目标 Keyspace 并建立或提供目标 etcd 客户端。
- `install_on_domain`：创建带虚拟 Server Info 注册支持的通用 Manager，并安装到 serving `Domain`。
- `TargetInfoCache`：持有目标 `Domain` 并实现通用 `InfoCache` 标记接口，确保 Domain 生命周期随运行时保留。
- `TargetDomainLifecycle`：把 `Domain::close` 适配为通用 `Lifecycle::close`。
- `CrossKSMinIdLoop`：持有 `systable::Cancellation` 与一个可取走的 `JoinHandle`；`start` 创建命名线程，`close` 取消并 join。
- `CrossKSProductionDdlBackend`：`pub(crate)` 的生产 DDL 适配器，实现 `DdlBackend` 的七个操作。
- `create_runtime`：本文件的核心装配与失败回滚函数。
- `RuntimeFactory` 实现：`create` 自动生成 `crossks-<pid>-<keyspace>` ID；`create_with_server_info` 使用注册流程传入的虚拟 Server ID；`registration_failed` 清除尚未移交的 etcd 客户端。

## 执行流程

安装流程如下：

1. `install_on_domain` 调用 `new_manager_with_server_info_provider(false, current_keyspace, ...)`，声明不是 classic kernel，并把 `target_etcd` 作为目标命名空间客户端提供器。
2. 通用 Manager 第一次为某 Keyspace 创建运行时时，先借助该提供器注册虚拟 Server Info；本工厂把创建的真实客户端暂存到 `pending_etcd[keyspace]`。
3. 注册成功后，Manager 以同一个 `server_info_id` 调用 `create_with_server_info`，进而进入 `create_runtime`。若注册失败，`registration_failed` 删除暂存项，释放其最后一个 `Arc`。

`create_runtime` 的成功路径为：

1. 从 `pending_etcd` 取走预创建客户端，打开目标 `CrossKSStore`。
2. 用 `CanonicalSessionFactory::from_crossks_tikv_store` 建立目标 Domain，再建立 `CrossKSSessionPool`。
3. 生产路径复用预创建的真实 etcd 客户端，并以其 raw client、命名空间下的 `DDLOwnerKey` 和相同的虚拟 Server ID 创建 Owner election；测试注入路径没有 election。
4. 启动 `CrossKSSchemaSyncer`，创建并首次刷新 `CrossKSStateSyncer`。
5. 基于 `CrossKSSystemTablePool` 创建系统表 Manager、Flashback Guard 与 `MinJobIdRefresher`；先同步刷新一次，再启动 `crossks-min-ddl-job-id` 后台线程。
6. 创建 `CrossKSJobSubmitter`；创建有 election 或无 election 的 `CrossKSDdlOwner`，安装 Schema Syncer 后启动 Owner。
7. 构造 `CrossKSProductionDdlBackend` 和通用 `DdlClient`，连同 Store、InfoCache、SessionPool、SchemaCoordinator 及生命周期对象返回 `SessionManager`。

Alter Table Mode 请求由 `pkg/domain/crossks/ddl_submit.rs` 的 `DdlClient::alter_table_mode` 驱动：先通过后端解析目标并检查名称/模式，再读取会话变量、刷新 Server State、提交 Job、写 etcd 通知，最后轮询 `history_job` 直到同步、失败、非预期终态或取消。本文件只实现这些后端步骤，不实现通用的轮询状态机。

## 数据与状态

- `pd_endpoints`、`etcd_endpoints` 和 `tls_files` 在工厂克隆之间按值共享配置；TLS 三元组顺序为 CA、证书、私钥。
- `store_opener` 与可选 `etcd_provider` 使用 `Arc` 共享可注入闭包。
- `pending_etcd: Arc<Mutex<HashMap<...>>>` 是“Server Info 注册准备”与“运行时正式创建”之间的所有权桥。键为 Keyspace；成功时 `create_runtime` 用 `remove` 取走，注册失败时 `registration_failed` 删除。
- `CrossKSProductionDdlBackend` 持有目标 Domain、会话池、提交器、Owner、Server State Syncer 和 etcd 客户端，保证 DDL 请求期间依赖存活。
- `session_variables` 当前通过池内 SQL 查询 `@@sql_mode` 并解析为位图，但将 `cdc_write_source` 固定为 `0`；这是当前 Rust 生产实现的明确行为，不应误写成已完整读取 Go 的全部会话变量。
- `resolve_table` 将元数据模型中的 Normal/Import/Restore 三种模式逐一映射到跨 Keyspace API 的 `TableMode`，并用按名称重新查得对象的 ID 防止名称指向其他表。

## 依赖与调用关系

上游：

- `pkg/session/runtime.rs` 公开该模块并在测试构建中装配 `crossks_runtime_test.rs`。
- `pkg/session/runtime/session.rs` 的规范 Session 启动流程间接通过 `KeyspaceSessionFactory` 安装跨 Keyspace Manager。
- `astersql-domain-crossks` 的 Manager 按需调用 `RuntimeFactory::{create_with_server_info,registration_failed}`；运行时句柄的 `alter_table_mode` 再调用本文件提供的 DDL 后端。

下游：

- `crossks_store` 打开目标 TiKV Store；`CanonicalSessionFactory` 建立目标 Domain。
- `crossks_session_pool` 提供普通系统会话、DDL 系统表会话、Flashback Guard 与最小 Job ID 适配器。
- `crossks_schema`、`crossks_owner`、`crossks_job_submit` 分别承担 Schema/Server State 同步、DDL 消费/历史读取和 Job 提交。
- `astersql-domain-serverinfo` 提供真实或内存 etcd 抽象；`astersql-owner` 提供生产选主。
- `pkg/session/Cargo.toml` 明确声明本文件直接使用的 workspace crate：`astersql-domain-crossks`、`astersql-domain-serverinfo`、`astersql-ddl-jobsubmit`、`astersql-ddl-systable`、`astersql-store-copr`、`astersql-owner`、`astersql-ddl-util`、`astersql-meta-model` 与 `astersql-parser-mysql`。该 crate 唯一显式 feature `nextgen` 不在本文件形成条件编译分支。

## 错误处理与边界

- PD 连接、Keyspace 解析、etcd 连接和线程创建错误均包装成带阶段上下文的 `ManagerError`。
- `Mutex` 中毒通过 `expect` 触发 panic，说明这些内部锁没有恢复策略；扩展代码不应在持锁区域执行可 panic 的用户逻辑。
- Store 已打开后，若 SessionFactory 或 SessionPool 创建失败，会显式关闭已创建的 Store，后者还会关闭 Domain。
- 完整装配闭包失败时，按已成功初始化的范围关闭 Owner、最小 Job ID 线程、Schema Syncer、SessionPool、Domain 和 Store。Option 槽位用于避免关闭未成功启动的组件。
- `notify_owner` 先写 `/tidb/ddl/add_ddl_job_general`，写失败则不调用本地 `owner.notify()`；成功后两种通知都发出。
- `resolve_database` / `resolve_table` 以 `Ok(None)` 表示目标不存在；名称查找或 SQL/解析失败转换成跨 Keyspace `Error`。
- `session_variables` 对空查询结果把 `sql_mode` 当空串解析；是否接受由 `GetSQLMode` 决定。
- 生产工厂只在 PD 与 etcd 配置存在时由启动链安装；真实端到端测试也要求外部 PD/TiKV/etcd，默认被 `#[ignore]`。

## 并发与资源生命周期

- 工厂可跨线程共享；闭包要求 `Send + Sync`，可变共享状态仅为 `pending_etcd` 的互斥 HashMap。
- `CrossKSMinIdLoop::start` 创建独立 OS 线程运行 `MinJobIdRefresher::start`。关闭时先设置 Cancellation，再从 `Mutex<Option<JoinHandle>>` 取走并 join，因此重复 `close` 不会重复 join。
- 返回的 `SessionManager` 先关闭 SessionPool，再逆序关闭 lifecycles。由于本文件按 `[TargetDomainLifecycle, schema syncer, min-id loop, ddl owner]` 注册，正常关闭顺序为 Owner、最小 ID 线程、Schema Syncer、Domain，最后由通用 Manager 按 Store 策略关闭 Store；这避免后台组件在池或 Domain 已销毁后继续工作。
- 虚拟 Server Info 注册的生命周期由 `astersql-domain-crossks` 的注册包装器在成功时转交给 `SessionManager`；注册或创建失败时包装器与 `registration_failed` 分别清理注册信息和暂存客户端。
- `CrossKSProductionDdlBackend` 本身不创建异步任务；DDL 完成等待由通用 `DdlClient` 以 100ms 间隔轮询并响应 Cancellation。

## 与 Go 版本的对应关系

主要对照为 `pkg/domain/crossks/cross_ks.go` 的 `Manager.createSessionManager` 与 `SessionManager.close`，以及 `pkg/domain/crossks/ddl_submit.go` 的 DDL 提交流程。

保持的语义包括：按目标 Keyspace 打开 Store；建立隔离的会话池和 Schema 协调/同步设施；注册虚拟服务器；初始化 Schema 与 Server State；建立 DDL 系统表、最小 Job ID 刷新器和提交客户端；通知 DDL Owner；轮询目标历史表；失败时按已创建资源回滚；关闭时停止后台循环并回收注册、客户端、池与 Store。

实现形态并非逐行相同：Go 将大部分资源直接放在一个 `SessionManager` 并用 context、channel 与 wait group 管理 goroutine；Rust 将通用管理策略放入 `astersql-domain-crossks`，本文件用 `Lifecycle` 对象和显式线程拼装具体实现。Go 使用 UUID 作为虚拟 Server ID，Rust 默认 `create` 使用进程 ID 与 Keyspace 组合，但正常注册路径通过 `create_with_server_info` 复用注册器生成的 ID。Go 的提交选项携带真实会话上下文中的 CDC write source；当前本文件的 `session_variables` 固定返回 `0`，这是需保留关注的迁移差异。

## 扩展指南

- 新增生产组件时，应在 `create_runtime` 中明确其“构造成功”边界，同时加入成功态 `lifecycles` 和失败态回滚；两条关闭路径的顺序必须一致地满足依赖逆序。
- 修改目标 etcd 命名空间、TLS 或 PD Keyspace 解析时，集中调整 `target_real_etcd`，并覆盖注册失败和运行时创建失败两种清理测试。
- 扩展 DDL 行为时，优先在 `CrossKSProductionDdlBackend` 对应 trait 方法接线；通用解析/轮询规则属于 `pkg/domain/crossks/ddl_submit.rs`，不要在本文件复制状态机。
- 如需补齐 CDC write source，应修改 `session_variables`，并同步 `pkg/session/runtime/crossks_runtime_test.rs` 中 Job 字段断言，同时与 Go `jobsubmit.SubmitOptions` 的取值时机保持一致。
- 增加 TableMode 枚举值时，必须同步更新 `resolve_table` 的穷尽映射以及独立测试。
- 测试必须继续放在独立的 `pkg/session/runtime/crossks_runtime_test.rs`，不要内嵌到生产文件。真实网络路径使用现有 ignored 测试；失败回滚、内存 etcd、Job 元数据和轮询边界使用本地独立测试。
- 兼容风险集中在 Go 编码的 DDL Job/历史记录、etcd key/namespace、Server ID 一致性和关闭顺序；性能风险集中在每个目标 Keyspace 的独立 Domain/会话池/线程，以及历史状态的固定间隔轮询。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`query CrossKSProductionRuntimeFactory --kind struct` 与 `node CrossKSProductionRuntimeFactory` 定位到 `pkg/session/runtime/crossks_runtime.rs:35`。索引未暴露 `create_runtime`、`install_on_domain` 的独立 method 节点，因此其调用关系由精确源码搜索补证，而非推测。
- 生产源码：`pkg/session/runtime/crossks_runtime.rs`；模块入口 `pkg/session/runtime.rs`；服务器安装链 `pkg/session/runtime/session.rs`；通用 Manager/RuntimeFactory/SessionManager 生命周期 `pkg/domain/crossks/cross_ks.rs`；通用 DDL 状态机 `pkg/domain/crossks/ddl_submit.rs`。
- crate 边界：`pkg/session/Cargo.toml` 的 `[lib]`、`[features]`、`[dependencies]`。
- Go 对照：`pkg/domain/crossks/cross_ks.go` 与 `pkg/domain/crossks/ddl_submit.go`。
- 独立 Rust 测试：`pkg/session/runtime/crossks_runtime_test.rs` 覆盖真实 TiKV 路径（ignored）、生产后端提交/通知/历史等待、Store 错误后的虚拟注册清理、Go meta/history 编码、Server State/BDR/Flashback 限制、重试型 KV 冲突、noop/名称不匹配、不可变快照及结构化历史错误。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节结构、真实路径引用和上述事实证据。

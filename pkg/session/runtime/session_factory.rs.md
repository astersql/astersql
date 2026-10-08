# `pkg/session/runtime/session_factory.rs`

源文件：[`session_factory.rs`](./session_factory.rs)

## 文件定位

本文件属于 `astersql-session` crate 的 `runtime::session_factory` 模块；模块由 `pkg/session/runtime.rs` 的 `pub mod session_factory` 暴露，crate 边界与 `nextgen` feature 见 `pkg/session/Cargo.toml`。它位于 Session、Domain、InfoSchema 同步和 DDL 服务的装配边界，负责把已有组件组装成三类运行时：目标 keyspace 的跨 keyspace `SessionManager`、当前 serving Domain 的普通 schema runtime，以及真正拥有 DDL worker 的普通 DDL service。

文件头注释给出的关键边界是：目标 keyspace 工厂只负责 store 与公共系统会话组件，DDL 执行仍属于目标 keyspace 的普通 DDL service。这个边界在 `KeyspaceSessionFactory::create_with_server_info` 中体现为构造 `SubmitOnlyBackend` 和 `DdlClient`，但不启动目标 keyspace 的 DDL worker；真正的 worker 由 `install_serving_ddl_runtime` 安装。

生产入口位于 `pkg/session/runtime/session.rs`：会话/Domain 初始化时构造 `KeyspaceSessionFactory::new` 并调用 `install_on_domain`；当 serving DDL 的 etcd、owner 和运行时准备好后调用 `install_serving_ddl_runtime`。因此，本文件不是 SQL 请求处理入口，而是 Domain 启动阶段的基础设施装配点。

## 核心职责

1. `TargetSessionStore` 把 `StorageHandle` 适配为 `astersql_domain_crossks::Store`，记录 keyspace 名称和底层 store 是否归本运行时所有，并提供幂等关闭。
2. `SystemSchemaLoader` 从目标 store 的指定时间戳加载完整 cross-KS InfoSchema，并把结果包装为 Domain 使用的 `LoadedInfoSchema`。
3. `KeyspaceSessionFactory` 实现 `RuntimeFactory` 的 prepare/create/失败回收协议：先建立目标 store、Domain、系统会话池、schema coordinator 与 validator，再接上 etcd transport、schema syncer、server-state 检查、submit-only DDL 后端和后台循环。
4. `prepare_normal_schema_runtime` 针对当前 serving Domain 的共享 InfoSchema cache 创建普通 schema 同步运行时；它同时创建系统会话池、validator、最小 DDL job ID refresher，并完成协议初始化与首次 reload。
5. `install_serving_ddl_runtime` 在普通 schema runtime 之上创建 `NormalDdlService`，注入 owner、schema barrier、升级策略和内部 SQL 提交回调，将服务注册到 Domain 并启动。这是本文件中唯一安装 DDL 消费侧 worker 的路径。

## 主要符号

- `TargetSessionStore { storage, keyspace, owned, closed }`：跨 keyspace store 适配器。`new` 返回 `Arc<Self>`；`keyspace` 暴露目标名称；`close_on_runtime_shutdown` 返回所有权标志；`close` 只在 `owned == true` 且首次调用时关闭底层 storage。
- `SystemSchemaLoader { store, keyspace }`：内部 InfoSchema loader。私有 `load(ts)` 调用 `NewLoaderForCrossKS(...).LoadWithTS(ts, true)` 并取 `CompleteInfoSchema()`；`InfoSchemaLoader` 实现分别处理当前版本、快照版本和 keyspace 存在性判断。
- `TargetTransport { server, schema }`：分别保存 server-info 和 schema-version 所需的 etcd client 抽象。两个 client 可共享底层连接，但承担不同 trait。
- `StoreOpener` / `TransportOpener`：可注入闭包类型。生产构造器使用真实 TiKV/PD/etcd，`with_openers` 允许独立测试注入内存 store 和故障 transport。
- `PreparedTarget`：prepare 阶段的所有权容器，持有 store、Domain、系统会话池、coordinator、validator 和 `transferred` 标志。其 `Drop` 在所有权尚未移交时依次关闭 pool、Domain 和 owned store。
- `KeyspaceSessionFactory { store, transport, min_job_id_refresher_hook, prepared, transports }`：跨 keyspace runtime 工厂。两个 `Mutex<HashMap<...>>` 保存按 keyspace 暂存的 prepare 结果和提前为虚拟 server 注册取得的 transport。
- `KeyspaceSessionFactory::new`：生产构造器。SYSTEM keyspace 借用全局 system storage；其他 keyspace 通过 `TiKVDriver::OpenWithOptions` 打开带 `keyspaceName` 的 store。transport 路径通过 PD 查询 keyspace ID，再把 etcd client namespace 限定为 `/keyspaces/tidb/{id}`。
- `KeyspaceSessionFactory::install_on_domain`：创建 cross-KS manager，提供虚拟 server-info transport，并启动 idle GC，最后安装到 Domain。
- `RuntimeFactory::{prepare, create, create_with_server_info, registration_failed}`：工厂协议的核心实现。`create` 使用随机 UUID；`create_with_server_info` 使用注册侧给定的虚拟 server ID；`registration_failed` 清除尚未消费的 transport 和 prepared state。
- `RuntimeLifetime`：实现 `Lifecycle`，统一取消 schema context 和 min-job refresher，join 所有线程，再关闭 syncer 和 Domain；`Drop` 再次调用 `close`，依赖各组件的幂等关闭语义。
- `SharedSchemaCache`：把 `schema::InfoCache::latest()` 暴露为 cross-KS `InfoCache`。
- `prepare_normal_schema_runtime`：返回 `Arc<NormalSchemaRuntime>` 的公共构造函数。
- `install_serving_ddl_runtime`：crate 内可见的 serving DDL 安装函数；成功后 Domain 持有服务，失败则先 `stop` 再返回错误。

## 执行流程

跨 keyspace 路径按以下顺序执行：

1. `session.rs` 创建 `Arc<KeyspaceSessionFactory>` 并调用 `install_on_domain`。工厂传给 `new_manager_with_server_info_provider`；manager 的虚拟 server provider 打开目标 transport，把它暂存在 `transports[keyspace]`，返回 server-info client 完成注册。
2. manager 调用 `RuntimeFactory::prepare`。store opener 对 SYSTEM keyspace 借用全局 store，对其他 keyspace 打开独立 TiKV store；随后创建带目标 keyspace 配置的 Domain、`SystemSchemaLoader`、schema coordinator、validator 和 `SystemSessionPool`。
3. 系统会话借出时，若会话存在事务 MDL，则把 `RegisteredMDLSession` 写入 coordinator；归还或销毁时按 session ID 删除。完整 prepare 结果进入 `prepared[keyspace]`。
4. `create_with_server_info` 取走 prepared state，并优先复用虚拟 server 注册阶段暂存的 transport。它初始化 schema-version protocol，读取一次全局 server state，创建 cross-KS InfoCache/Syncer，完成首次 `Reload` 和 Domain `init`。
5. 初始化后显式检查 `mysql.tidb_ddl_job` 与 `mysql.tidb_ddl_history`。缺表表示目标 keyspace 未 bootstrap，创建立即失败，`PreparedTarget::drop` 回收 pool、Domain 和 owned store。
6. 工厂建立系统表 manager 与 min-job refresher，把 refresher 注入 InfoSchema syncer；再建立 `SubmitOnlyBackend`。后端回调分别提供 snapshot、DDL session variables、同步刷新 server state，以及基于目标 etcd transport 的 owner 通知。
7. 启动 `keyspace-schema-sync`、`keyspace-mdl-check`，通常还会启动 `keyspace-min-job-id` 三个线程。随后构造 cross-KS `SessionManager`，注册 distributed backfill runtime，设置 `transferred = true`，把资源所有权移交给 `SessionManager`/`RuntimeLifetime`。
8. 运行时关闭时，`RuntimeLifetime::close` 先发出取消，再 join 后台线程，最后关闭 syncer 与 Domain；store 则由 `SessionManager` 根据 `close_on_runtime_shutdown` 决定是否关闭。

普通 serving 路径中，`prepare_normal_schema_runtime` 先拒绝零 lease 和毫秒数溢出，同步发布 MDL/NextGen 全局开关，按需借用 Domain 已有的 server-info transport，然后针对 Domain 的共享 storage/cache 建立 pool、validator、coordinator、普通 InfoSchema syncer 和 refresher。构造完成后先 `protocol.Init`，再 `ReloadWithContext`；任一步失败都由 runtime 的 RAII 关闭协议与 pool。

`install_serving_ddl_runtime` 基于上述 runtime 建立 server-state syncer 和 schema barrier，构造 `NormalDdlService`。executor factory 创建真正的 `NormalDdlExecutor`；SQL 提交回调则升级 Domain 弱引用、创建 `ConcreteSession` 并执行 SQL。服务通过 `domain.set_ddl` 接管 Domain 的 DDL 接口，随后以 `StartMode::Normal` 启动；启动失败会立即调用 `stop`。

## 数据与状态

- `owned` 区分独占 store 与借用 store。非 SYSTEM target 通常为 owned；SYSTEM store 来自 `GetSystemStorage`，必须跨 runtime idle eviction 保持存活。
- `closed: AtomicBool` 用 `swap(true, Ordering::AcqRel)` 实现并发安全的一次性关闭。失败的第一次底层 `close` 仍会把标志置为 true，因此不会重复调用底层关闭。
- `prepared` 是 prepare 与 create 两阶段之间的所有权暂存区；`transports` 是 server-info 注册与 runtime 创建之间的 transport 暂存区。两者都按 keyspace 索引，并由 `Mutex` 串行化访问。
- `PreparedTarget::transferred` 是资源归属不变量：为 false 时 drop 必须回收；仅当 `SessionManager` 已完整构造并完成 backfill runtime 注册后才设为 true。
- `SystemSchemaLoader::keyspace_exists` 只认可构造时绑定的 keyspace，防止 loader 被当作任意 keyspace 探测器使用。
- `RuntimeLifetime::loops` 保存所有已成功 spawn 的线程句柄；即使后续某个 spawn 或装配步骤失败，已创建的 lifetime drop 也会取消并 join 先前线程。
- normal runtime 的 `SchemaLifecycle` 初始为 `{ started: false, closed: false, loops: [] }`；启动/关闭幂等性由 `NormalSchemaRuntime` 实现并由独立测试验证。

## 依赖与调用关系

上游调用关系：

- `pkg/session/runtime/session.rs` 在 Domain 初始化中调用 `KeyspaceSessionFactory::new(...).install_on_domain(...)`，将 cross-KS manager 接入 Domain。
- 同一文件在具备 owner 与 etcd transport 时调用 `install_serving_ddl_runtime`，包括正常初始化和测试/兼容初始化路径。
- `pkg/domain/crossks/cross_ks.rs` 的 manager 协议负责按 keyspace 调用 `RuntimeFactory::prepare`、虚拟 server 注册、`create_with_server_info`，并在注册/创建失败时调用 `registration_failed`。
- `pkg/session/runtime/normal_ddl_test.rs`、`lifecycle_test.rs`、`normal_ddl_masking_policy_test.rs` 直接调用可注入入口验证构造、生命周期与跨 keyspace 行为。

主要下游依赖：

- `astersql-domain`：`Domain`、`DomainConfig`、`StorageHandle`、`InfoSchemaLoader` 与 DDL service 注册。
- `astersql-domain-crossks`：`RuntimeFactory`、manager、`SessionManager`、`SchemaCoordinator`、`SubmitOnlyBackend` 和 `DdlClient`。
- `astersql-infoschema-issyncer` 与 `astersql-infoschema-isvalidator`：完整 schema 加载、普通/cross-KS schema 同步和事务 schema 校验。
- `astersql-ddl-schemaver`、`astersql-ddl-serverstate`、`astersql-ddl-systable`：schema version 协议、全局升级状态、DDL 系统表及最小 job ID 刷新。
- `astersql-store-driver`、`astersql-store-copr`、`astersql-domain-serverinfo`：目标 store、PD keyspace ID 和 namespaced etcd/server-info transport。
- `super::system_session`、`super::normal_ddl_service`、`super::modify_column_dist_backfill`：系统会话池、普通 DDL 消费服务和目标 runtime 的分布式回填绑定。

`pkg/session/Cargo.toml` 明确声明上述 crate、`tokio`、`uuid`、`url` 等依赖；`nextgen` feature 转发到 deploy mode/kernel type，但本文件本身没有条件编译分支，运行时通过 `IsNextGen()` 设置 schema-version 行为。

## 错误处理与边界

- store、PD、etcd、schema loader、Domain、server state、线程创建等底层错误统一转换为 `ManagerError(String)`；normal runtime 路径则转换为 `String`。转换会丢失具体错误类型，但保留展示文本。
- `new` 在 system storage 未初始化时明确返回 `SYSTEM Store is not initialized`；目标 Domain 初始化后若缺少 DDL job/history 表，会返回 `target is not bootstrapped`，避免在未 bootstrap keyspace 上提交作业。
- `create_with_server_info` 在没有 prepared state 时会自行调用 `prepare`；取出时随后使用 `unwrap` 的安全前提是同一 keyspace 的 prepare/create 由 manager 串行协调。若未来允许同一工厂并发创建同一 keyspace，必须先修改这一状态机。
- transport 打开失败发生在 prepared state 已从 map 取出之后，局部 `PreparedTarget` drop 会清理资源；虚拟注册失败则由 `registration_failed` 删除两个 map 中残留状态。
- `prepare` 对同一 keyspace 再次调用会替换旧 `PreparedTarget`，旧值在 map insertion 时立即 drop 并清理。这不是共享/引用计数的复用接口。
- normal schema lease 必须大于零且可表示为 `u64` 毫秒；Domain server-info mutex poisoned、protocol init、首次 reload 等失败都会向上传播。
- 后台 schema/MDL loop 的运行期错误只打印到 stderr，不反向使已返回的 runtime 失败；健康监控若需要感知此类错误，必须在这些 loop 或 lifecycle 上增加显式信号。
- `install_serving_ddl_runtime` 在 `domain.set_ddl` 后启动服务。启动失败会 `stop` 服务，但 Domain 已经持有该 service；调用方应以返回错误作为初始化失败依据，不可把“已 set”误判为成功。

## 并发与资源生命周期

`KeyspaceSessionFactory` 可通过 `Arc` 跨线程共享；opener 闭包要求 `Send + Sync`，暂存 map 各自由 `Mutex` 保护。对同一 keyspace 的多步操作不是由单个大锁包围，正确性依赖 cross-KS manager 的按 keyspace 注册协议；工厂内部只保证单次 map 操作互斥。

`TargetSessionStore::close` 的原子标志避免 Domain、prepared guard、manager shutdown 等多条清理路径重复关闭 owned store。借用的 SYSTEM store无论调用多少次都不关闭。`PreparedTarget` 是创建完成前的 RAII guard；移交之后，`RuntimeLifetime` 负责同步线程和 Domain，`SessionManager` 负责 pool/coordinator/store 等其余资源。

三个 cross-KS 后台线程共享可取消的 `version::Context` 或 cancellation token。关闭顺序为先 cancel、再 join、后关闭 syncer/Domain，避免线程继续访问已释放对象。min-job refresher 可通过仅测试 hook 禁止启动，用于验证会话/刷新器计数边界；生产 hook 默认保持启用。

normal schema runtime 不在构造函数中启动循环，只完成 Init 与首次 Reload；循环由 `NormalDdlService::start` 接管。这样 Domain/DDL service 是普通路径的唯一生命周期 owner，避免 legacy Domain loop 与新 runtime 重复刷新。测试证明重复 `start`/`close` 不产生重复循环，Domain close 后 pool、validator 和 refresher一并停止。

## 与 Go 版本的对应关系

Rust 跨 keyspace 主流程对应 `pkg/domain/crossks/cross_ks.go::Manager.createSessionManager`：两者都先打开目标 store，再创建 coordinator、validator 和系统会话池，注册虚拟 server，初始化 schema-version/server-state syncer，首次 reload InfoSchema，创建 system-table manager/min-job refresher，最后组装只负责提交的 DDL client，并启动 schema/MDL/min-job 循环。

`TargetSessionStore` 的 SYSTEM 特例对应 Go `getOrCreateStore`：SYSTEM 返回 `kvstore.GetSystemStorage()`，其他 keyspace 初始化独立 storage。Rust 用显式 `owned` 表达 Go failpoint/清理代码中的“共享 SYSTEM store 不应随 runtime 关闭”规则。

`SystemSchemaLoader` 与 cross-KS syncer 对应 `pkg/infoschema/issyncer/loader.go::NewLoaderForCrossKS` 和 `syncer.go::NewCrossKSSyncer`：使用 full load、设置 crossKS 标志，并把 coordinator、schema-version syncer、pool、validator 和 min-job refresher接到同一链路。

系统会话池回调对应 Go `util.NewSessionPool` 的借出、归还和销毁回调。Rust 通过 `transaction_mdl` 提取可注册的事务 MDL，而 Go 直接把 `sessionctx.Context` 存入 coordinator；两者共同保证借出的内部事务参与 schema barrier，归还/销毁后取消登记。

Rust 的系统会话初始化语义还应与 `pkg/session/session.go::getSessionFactoryInternal` 和 `createCrossKSSession` 一起理解：Go 工厂设置 autocommit、执行超时和 packet 限制，并创建 `dom == nil` 的 cross-KS session；Rust 的具体会话配置封装在 `SystemSessionPool`/`ConcreteSession`，本文件只负责共享 pool、validator、Domain 与目标 store 的接线。

Rust 相比当前 Go 实现把创建拆成 `prepare`、虚拟 server registration、`create_with_server_info` 三阶段，并以 `PreparedTarget`/`registration_failed` 显式实现失败回滚；这是 Rust cross-KS manager trait 的结构差异，不应误解为额外业务阶段。普通 serving DDL 的 Rust 组件化实现也分散在 `normal_ddl_service.rs` 等文件中，本文件只承担与 Go Domain DDL 初始化相当的装配。

## 扩展指南

- 增加目标 keyspace 连接参数、安全选项或 namespace 规则时，修改 `KeyspaceSessionFactory::new` 的两个 opener，并同步验证 SYSTEM 借用语义、非 SYSTEM owned 关闭和 transport 注册失败清理。相关测试首选 `pkg/session/runtime/lifecycle_test.rs`，不要把测试内嵌进本文件。
- 增加 prepare 阶段资源时，把资源放入 `PreparedTarget`，同时更新其 `Drop` 和成功移交后的 lifecycle owner；必须覆盖 transport、schema init、reload、bootstrap 检查等每个失败点，防止只在成功路径释放。
- 修改系统会话的 MDL/coordinator 行为时，同步更新 `prepare` 与 `prepare_normal_schema_runtime` 的三类 pool 回调，确保 cross-KS 与普通 DDL 对借出、归还、销毁保持一致。
- 增加 cross-KS 后台循环时，将 `JoinHandle` 纳入 `RuntimeLifetime::loops`，提供取消来源，并保持“cancel 后 join，再关闭依赖”的顺序。不得创建无法被 runtime close 回收的 detached thread。
- 修改 DDL 提交行为时区分两条路径：`create_with_server_info` 只能扩展 submit-only backend；消费、owner election 和 worker 逻辑属于 `install_serving_ddl_runtime`/`NormalDdlService`。不要在 target factory 中启动第二套 DDL worker。
- 修改 schema lease、cache 或 validator 时，同时检查 `SystemSchemaLoader`、cross-KS `NewCrossKSSyncer` 和普通 `schema::New` 的语义，并同步 `normal_ddl_test.rs` 中构造失败、刷新和 lifecycle 测试。
- 对运行期 loop 错误增加可观测性时，应设计明确的 channel/status API；当前仅 `eprintln!`，直接改成 panic 或提前 drop 会改变清理与服务可用性语义。
- 兼容性风险主要在 keyspace 隔离、SYSTEM store 所有权、etcd namespace、DDL 只提交/消费分工；性能风险主要在每个 target 的线程、连接、完整 schema load 和系统会话池数量。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/session_factory.rs` 确认目标文件在索引中，`node --file ...` 完整读取了 698 行源码及 49 个符号。精确 `query` 确认 `KeyspaceSessionFactory`、`prepare_normal_schema_runtime`、`install_serving_ddl_runtime`、`install_on_domain` 和 `registration_failed` 的定义；图的 callers/callees 对这些 impl/trait 方法未返回有效边，因此调用点改用下列直接源码引用核验，没有据此虚构图关系。
- 目标源码：`pkg/session/runtime/session_factory.rs`，重点为 `TargetSessionStore`、`SystemSchemaLoader`、`PreparedTarget::drop`、`KeyspaceSessionFactory::{new, install_on_domain}`、`RuntimeFactory` 实现、`RuntimeLifetime::close`、`prepare_normal_schema_runtime`、`install_serving_ddl_runtime`。
- crate 与模块：`pkg/session/Cargo.toml`、`pkg/session/runtime.rs`；前者确认 crate、feature 和直接依赖，后者确认公开模块声明。
- Rust 上游：`pkg/session/runtime/session.rs` 中的 `KeyspaceSessionFactory::new/install_on_domain` 与 `install_serving_ddl_runtime` 调用点。
- Rust 独立测试：`pkg/session/runtime/lifecycle_test.rs` 覆盖 store-before-registration、公共组件加载/单次关闭、跳过 refresher、共享 SYSTEM store、transport/schema/server-state/registration/reload 失败清理、submit-only 和自动 GC；`pkg/session/runtime/normal_ddl_test.rs` 覆盖 normal schema runtime 的用户 KV 刷新、循环归属、构造失败清理、取消和 barrier；`pkg/session/runtime/normal_ddl_masking_policy_test.rs` 覆盖真实 cross-KS runtime 接线与 nextgen serving DDL 安装。
- Go 对照：`pkg/domain/crossks/cross_ks.go` 的 `createSessionManager`、`getOrCreateStore` 与 manager lifecycle；`pkg/infoschema/issyncer/loader.go::NewLoaderForCrossKS`；`pkg/infoschema/issyncer/syncer.go::NewCrossKSSyncer/SetMinJobIDRefresher`；`pkg/session/session.go::getSessionFactoryInternal/createCrossKSSession`。Go 测试证据位于 `pkg/domain/crossks/cross_ks_test.go`、`cross_ks_internal_test.go` 和 `pkg/infoschema/issyncer/{loader_test.go,syncer_test.go}`。
- 本任务只生成文档，未修改 Rust、Go 或 Cargo，也按计划未运行 Cargo。结构验证应确认文件存在且恰好包含规定的十一个二级标题；人工复核重点是 submit-only/serving worker 边界、两阶段资源所有权和失败回收链均有源码或测试依据。

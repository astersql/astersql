# `pkg/domain/serverinfo/syncer.rs`

## 文件定位

本文件是 `astersql-domain-serverinfo` crate 的节点发现与存活信息同步实现。crate 入口 `pkg/domain/serverinfo/lib.rs` 将本模块和 `info.rs`、`real_etcd.rs`、`status_endpoint_claim.rs` 一并重新导出；`pkg/domain/serverinfo/Cargo.toml` 则声明其 Go 对照包为 `pkg/domain/serverinfo`，直接依赖配置、日志、etcd client 与 Tokio。

在运行链上，服务型 Domain 通过 `pkg/domain/domain.rs::Domain::install_server_info_syncer` 构造 `Syncer`，先登记 `/tidb/server/info/<id>`，再登记 `/topology/tidb/<addr>/{info,ttl}`。跨 keyspace 运行时由 `pkg/domain/crossks/cross_ks.rs::RegisteredRuntimeFactory::create` 构造 `NewCrossKSSyncer`，并由 `RegisteredServerInfo::start` 在线程中运行 `ServerInfoSyncLoop`。本文件因此位于 Domain 生命周期和 etcd 集群发现数据之间，而不是 SQL 请求执行主链上。

配套数据模型和时间常量定义在 `pkg/domain/serverinfo/info.rs`：server-info 前缀为 `/tidb/server/info`，topology 前缀为 `/topology/tidb`，默认读取重试 5 次、单次超时 1 秒，topology lease 为 45 秒，topology 刷新和 min-start-TS 上报间隔均为 30 秒。

## 核心职责

1. `NewSyncer`、`NewSyncerWithOptions`、`NewCrossKSSyncer` 根据全局配置生成本地 `ServerInfo` 快照，确定是否需要声明 status endpoint，并建立节点专属 etcd 路径。
2. `NewSessionAndStoreServerInfo` 在登记前清理同一 IP/SQL 端口的陈旧实例，申请 lease，尽力竞争 status endpoint，随后把 server-info 绑定到 lease；最后一步失败时限时清理声明并撤销 lease。
3. `GetServerInfoByID`、`GetAllServerInfo`、`UpdateServerLabel` 提供节点信息读取和动态标签更新；写入成功后才替换本地动态信息。
4. `NewTopologySessionAndStoreServerInfo`、`StoreTopologyInfo`、`updateTopologyAliveness` 管理 topology 的持久 `info` 键和带 lease 的 `ttl` 键。
5. `ServerInfoSyncLoop` 和 `TopologySyncLoop` 处理 session 失效后的重建、周期上报或刷新，以及退出信号。
6. `RemoveServerInfo`、`RemoveTopologyInfo`、`RevokeSession`、`RevokeTopologySession` 执行关闭清理；`pkg/domain/domain.rs::Domain::close` 和 cross-keyspace 包装器是实际调用者。
7. `EtcdClient` 隔离存储实现；`MemoryEtcdClient` 为独立 Rust 测试提供确定性的键值、lease、revision 和故障注入语义，生产适配器位于相邻 `real_etcd.rs`。

## 主要符号

- `Context`：同步代码使用的轻量取消/截止时间对象。`WithTimeout` 取已有截止时间和新截止时间的较早者，`Done` 同时检查显式取消、外部 `done_check` 和超时。方法名保留 Go 风格以便移植对照。
- `SyncError(String)`：本模块统一错误类型，可由 `ServerInfoError` 转换；底层操作和反序列化错误沿调用栈返回。
- `KeyValue`、`EtcdClient`：同步 etcd 抽象。必需方法覆盖读写、精确删除、前缀删除和撤销 lease；`CompareAndPut`、`TryCreateClaim`、`ReattachClaim`、`CompareAndDelete` 为 status endpoint 的原子事务能力，默认实现明确返回“不支持”，因此实际客户端必须覆盖需要的事务。
- `MemoryEtcdClient`：以 `Mutex<BTreeMap<...>>` 保存值与 revision，以原子量保存临时 Get 失败次数及下一 revision。它实现精确/前缀读取、lease 撤销、CAS 写删和 claim 创建/重挂。
- `Session`：保存 `lease_id`、TTL 与原子 `done` 标志。`Close` 只改变本地结束状态；删除 lease 下键由 `EtcdClient::RevokeLease` 完成。
- `Storage`、`MinStartTSReporter`、`NoopMinStartTSReporter`：隔离 min-start-TS 上报所需的存储和回调。默认 reporter 不做工作；服务 Domain 当前安装的也是 `NoopMinStartTSReporter`（`pkg/domain/domain.rs::install_server_info_syncer`）。
- `ServerConfig`、`SetGlobalServerConfig`、`GetGlobalServerConfig`：本文件自己的进程级配置快照，受 `OnceLock<RwLock<_>>` 保护，供 `getServerInfo` 构造地址、端口、lease、keyspace、标签和版本字段。是否启用 status endpoint claim 则读取 `astersql_config::get_global_config().status.report_status`。
- `Syncer`：核心状态持有者，包含可选 etcd 客户端、reporter、本地 `ServerInfo`、server-info 键、两个独立 session 以及可选 status endpoint claim 键。
- `SyncerOption::WithoutStatusEndpointClaim`：用于临时全局变量 Domain；`newSyncer` 还会对 assumed-keyspace 实例自动禁用 claim。
- `serverInfoKeyPath`：形成 `/tidb/server/info/<id>`。
- `getInfo`：有限次读取、每次派生超时上下文并逐项反序列化为 `ID -> ServerInfo`。
- `getServerInfo`：从全局配置创建初始静态/动态信息；`MockServerInfo` 时固定启动时间和标签，以支持可重复测试。
- `join_host_port`：为裸 IPv6 地址加方括号，再拼接端口，供 topology 键使用。

## 执行流程

### server-info 登记

1. `newSyncer` 调用 `getServerInfo`，计算 claim 是否启用，并初始化 `Syncer`；此时尚无 session，也未写 etcd。
2. `NewSessionAndStoreServerInfo` 在无 etcd 客户端时直接成功返回；否则先调用 `cleanupStaleServerAndOwnerInfo`。
3. 清理逻辑通过 `getInfo` 扫描全部 server-info，只选择“ID 不同、IP 相同、SQL 端口相同”的陈旧节点。它仅扫描 `/tidb/ddl/fg/owner/`，将 owner value 按首个 `_` 截断后与陈旧 ID 比较，只删除第一个匹配 owner 键，之后删除该陈旧 server-info 键。
4. 客户端申请 45 秒 lease，`Syncer` 保存新 `Session`，再调用 `tryClaimStatusEndpoint`。claim 是尽力操作：冲突或事务错误由 claim 层报告，不阻断 server-info 登记。
5. `StoreServerInfo` 在持有 `info` 写锁时执行 `Marshal`（序列化会刷新动态 server ID），然后将结果绑定当前 session lease 写入节点路径。若 session 尚未建立则返回明确错误。
6. 若写入失败，`cleanupFailedRegistration` 先关闭本地 session，再用 1 秒上下文执行 compare-and-delete claim 与 lease revoke；清理错误仅记录日志，最终仍返回原始存储错误。

### 读取和标签更新

- `GetServerInfoByID` 对本机 ID 或无 etcd 场景直接返回本地副本；远端 ID 使用精确键读取，响应中找不到目标 ID 时返回包含完整 key 的错误。
- `GetAllServerInfo` 有 etcd 时前缀读取；无 etcd 时根据当前本地 ID、getter 和最新全局配置重建单项，而不是原样克隆本地对象。
- `UpdateServerLabel` 合并传入标签。没有变化时不写；有变化时先序列化并写 etcd，写成功后才调用 `setDynamicServerInfo` 提交本地状态，避免远端失败却提前改变本地缓存。

### topology 与后台循环

1. `NewTopologySessionAndStoreServerInfo` 申请独立的 45 秒 lease，然后调用 `StoreTopologyInfo`。
2. `StoreTopologyInfo` 把 `ServerInfo::ToTopologyInfo()` 写入 `<topology>/<host:port>/info`，该键不绑定 lease；随后 `updateTopologyAliveness` 将当前纳秒时间写入同前缀的 `/ttl`，并绑定 topology lease。
3. `TopologySyncLoop` 最多每 100 毫秒检查一次退出和 session 状态。session 结束时重建 session 并重新写两类键；正常时每 30 秒刷新 topology。
4. `ServerInfoSyncLoop` 同样以最多 100 毫秒粒度响应退出。server-info session 结束时先再次检查退出，再尝试 `Restart`；每 30 秒在 session 存在时调用 reporter。
5. 两个循环都忽略重启或周期刷新错误并继续运行；调用方应通过外部可观测性判断持续故障。cross-keyspace 包装器会先发送退出信号并 `join` 工作线程，再删除键或撤销 lease。

## 数据与状态

- 本地节点信息存于 `Arc<RwLock<ServerInfo>>`。读取返回 clone；标签更新通过“克隆动态部分—远端写入—本地替换”的顺序保持单次调用内的一致性。
- server-info 与 topology 使用两个独立 `Session`/lease。server-info 键及 status endpoint claim 绑定前者；topology 的 `/ttl` 绑定后者；topology `/info` 明确不绑定 lease，因而关闭时还需要 `RemoveTopologyInfo` 删除整个节点前缀。
- `statusEndpointClaimKey` 仅在全局 `report_status` 开启、未指定 `WithoutStatusEndpointClaim` 且 `ServerInfo::IsAssumed()` 为假时存在。claim value 是节点 ID，lease 和 revision 用于防止失败者或旧 session 删除新持有者。
- `MemoryEtcdClient` 的 `values` 和 `revisions` 分别加锁，原子 claim 方法按固定次序同时持有这两把锁。普通 `Put` 会推进 revision；`CompareAndDelete` 比较 value 与 lease，但删除时不移除 revision 记录，这不影响当前测试所需的可见键值语义。
- `GLOBAL_CONFIG` 是进程全局可变状态；测试通过 `syncer_test.rs::with_config` 的全局互斥和 panic 后恢复来隔离它。
- 取消状态和 session done 状态采用 `SeqCst` 原子序；循环退出通过 `std::sync::mpsc::Receiver<()>`，断开通道也被视为退出。

## 依赖与调用关系

RustCodeGraph 对 `NewSessionAndStoreServerInfo` 给出的直接被调用边包括 `cleanupStaleServerAndOwnerInfo`、`EtcdClient::GrantLease`、`tryClaimStatusEndpoint`、`StoreServerInfo` 和 `cleanupFailedRegistration`；对 `ServerInfoSyncLoop` 给出的边包括 `Done`、`Restart` 和 `MinStartTSReporter::ReportMinStartTS`；对 `TopologySyncLoop` 给出的边包括 `TopologyDone`、`RestartTopology` 和 `StoreTopologyInfo`。

上游接线由源码进一步确认：

- `pkg/domain/domain.rs::Domain::install_server_info_syncer` 调用 `NewSyncerWithOptions`、`NewSessionAndStoreServerInfo` 和 `NewTopologySessionAndStoreServerInfo`，并在 topology 登记失败时清理已建立资源；替换旧 syncer 时也先删除旧键并撤销两个 session。
- `pkg/domain/domain.rs::Domain::close` 取出 syncer，依次执行 `RemoveServerInfo`、`RemoveTopologyInfo`、`RevokeSession`、`RevokeTopologySession`。
- `pkg/domain/crossks/cross_ks.rs::RegisteredRuntimeFactory::create` 建立 assumed-keyspace syncer；`RegisteredServerInfo::start` 启动 `ServerInfoSyncLoop`，`stop` 发退出信号并等待线程结束。

下游数据/功能依赖包括：`info.rs` 的 `ServerInfo`、`DynamicInfo`、`TopologyInfo`、路径及时间常量；`status_endpoint_claim.rs` 的 claim 构造和竞争协议；`real_etcd.rs` 的生产 `EtcdClient`；`astersql-config` 的 status 开关；`astersql-util-logutil` 的失败清理日志。`etcd-client` 和 Tokio 虽在 crate manifest 中声明，但本文件本身通过同步 trait 和标准库线程/通道运行，真实连接适配由相邻模块承担。

## 错误处理与边界

- 所有可观察的存储、反序列化和 session 初始化错误统一为 `SyncError`。锁中毒使用 `expect`，会 panic，而不是转为 `SyncError`。
- 缺少 etcd 客户端是受支持的退化模式：登记、更新、删除均为成功/no-op；本地和全量读取仍可返回节点信息，topology 全量读取返回空列表。
- `getInfo` 在父上下文结束时立即停止；每次 Get 使用不晚于父截止时间的子截止时间，失败之间固定睡眠 200 毫秒。`retry_count <= 0` 时不发请求并返回初始错误。任意成功响应中的一项无法反序列化都会使整次读取失败。
- 陈旧清理是 best-effort：扫描失败直接放弃；owner 读取、owner 删除和 server-info 删除错误均被忽略，不能阻断新节点继续申请 lease 和登记。
- status claim 竞争不决定 server-info 是否可登记：竞争失败者仍保留自己的 `/tidb/server/info/<id>`。删除 claim 使用 ID+lease 比较，避免失败者或旧 lease 清除赢家。
- `RemoveServerInfo`、`RemoveTopologyInfo` 和两个 revoke 方法刻意吞掉客户端错误（claim 删除失败会记日志）；调用方无法从返回值获知清理是否完成。
- `ServerInfoSyncLoop`/`TopologySyncLoop` 不传播后台错误，且自身不捕获 panic。与 Go 的 `Recover` 保护不同，Rust reporter、锁或客户端实现若 panic，会终止承载线程。
- `StoreTopologyInfo` 先写无 lease 的 info 再写 ttl；第二步失败会留下 info。反过来，正常 lease 到期只移除 ttl，所以最终关闭路径必须显式前缀删除。
- `SystemTime` 早于 Unix epoch 时使用 0；IPv6 裸地址经 `join_host_port` 加方括号。测试辅助函数仍按 IPv4 字符串拼接，不代表生产键生成限制。

## 并发与资源生命周期

`Syncer` 构造阶段只创建内存状态；正式资源生命周期从 `NewSessionAndStoreServerInfo` 和 `NewTopologySessionAndStoreServerInfo` 开始。两个 session 相互独立，重启其中一个不会自动重建另一个。`Restart` 会再次运行陈旧清理并申请新 lease；claim 层负责把同一节点的声明安全重挂到新 lease。

后台循环独占 `&mut self`，通常由外层 `Arc<Mutex<Syncer>>` 串行访问。cross-keyspace 的 `RegisteredServerInfo` 在线程中持锁运行整个 `ServerInfoSyncLoop`，因此停止流程先通过通道唤醒循环，等待 `join` 后才重新取得锁清理；若反向先等待锁，会造成无法发送/处理关闭的生命周期问题。

服务 Domain 当前只保存 `Arc<Mutex<Box<Syncer>>>` 并负责安装/关闭；在本次直接证据中没有看到它启动两个后台循环。文档不据此推断其他未检索入口。关闭的安全顺序是先停止相关循环，再删除显式键，最后撤销 lease；Go `RevokeSession` 的注释也明确要求先停止循环，避免循环看到 done 后立即重建 session。

`MemoryEtcdClient` 适合并发 claim 测试：测试用 barrier 同时登记两个不同端口节点，验证两份 server-info 都保留、claim 只有一个赢家，并且失败者删除自身时不能删除赢家 claim。它是测试双实现，不应被当作完整 etcd 一致性模型。

## 与 Go 版本的对应关系

主要结构和顺序直接对应 `pkg/domain/serverinfo/syncer.go`：`Syncer` 字段、三类构造器、登记前陈旧清理、标签“远端成功后本地提交”、两个 lease、topology info/ttl 键布局、session 重启、min-start-TS 上报、读取重试以及 `getServerInfo` 的字段来源均保持一致。`pkg/domain/serverinfo/syncer_test.rs` 还明确将 `test_topology`、`test_cleanup_stale_server_and_owner_info`、`test_assumed_server_info_syncer` 对应到 Go 的同名测试。

当前 Rust 版本不是逐行等价实现，扩展时需保留以下已证实差异：

- Go 使用 `context.Context`、etcd concurrency session、channel/ticker 和异步 lease keepalive；Rust 使用同步 `Context`、抽象 `EtcdClient`、本地 `Session` 状态以及标准库阻塞通道。真实 lease 行为由 `real_etcd.rs` 适配器承担。
- Go `SyncerOption` 是可变参数函数选项；Rust 是枚举切片。Rust 的 `newSyncer` 明确对 assumed-keyspace 禁用 claim。
- Go 的 server-info 写、标签写和删除辅助带默认重试；Rust 写删接口单次调用，只有读取 `getInfo` 在本文件内重试。
- Go 后台循环带 panic recovery 和成功/失败日志；Rust 循环吞掉重启/刷新错误且无 recovery。Go `getInfo` 也记录每次失败和坏值，Rust 不记录。
- Go 从正式全局配置直接生成全部字段、更新 server-info metric，并用 failpoint 注入测试值；Rust 使用本地 `ServerConfig` 快照和 `MockServerInfo`，且版本字符串是 `info.rs` 中的占位常量。
- Go 的本地信息使用原子指针替换；Rust 使用 `RwLock`。两者均在远端标签写成功后才发布新动态状态。
- Rust 额外提供 `MemoryEtcdClient`、原子 claim trait 方法和 `NewSyncerWithOptions`，用于将生产适配和独立测试从核心逻辑中解耦。

## 扩展指南

- 新增 server-info 字段时，先修改 `info.rs` 的模型和编解码，再检查 `getServerInfo`、`StoreServerInfo`、`GetAllServerInfo` 的无 etcd 分支以及 `ToTopologyInfo`；同步更新独立测试 `pkg/domain/serverinfo/info_test.rs` 与 `syncer_test.rs`，不要把测试内嵌回本文件。
- 新增 etcd 操作时优先扩展 `EtcdClient`，同时实现 `MemoryEtcdClient` 和 `real_etcd.rs`；若涉及竞争所有权，必须基于 value、lease、revision 的事务条件，不能退化成 Get 后 Put/Delete。
- 调整登记流程时保持“清陈旧项—申请 lease—尽力 claim—写 server-info—失败限时回滚”的顺序，并用 `go_merge_43_failed_server_info_store_revokes_new_session`、status claim 竞争/失败测试覆盖原错误保留和赢家不受影响。
- 调整 topology 时维持 `/info` 无 lease、`/ttl` 有 lease 的兼容布局；同步覆盖 `test_topology` 的首次写入、restart 重建和 ttl 重建断言，并检查 IPv6 `join_host_port`。
- 修改循环或关闭路径时，明确谁拥有退出 sender、线程 join 和 `Syncer` 锁；增加测试证明退出优先于 session 重启，且循环停止后才 revoke。若让错误可观察，应同时设计返回通道或指标，而不是只改变当前吞错行为。
- 修改陈旧节点判定时只触及同 IP+SQL Port、不同 ID 的目标，并维持 owner 前缀及 value 后缀解析约定；`test_cleanup_stale_server_and_owner_info` 已验证第二个匹配 owner 和无关 DDL 键不会被误删。
- 对齐 Go 新提交时应比较 `pkg/domain/serverinfo/syncer.go` 和 `syncer_test.go` 的增量，避免顺带补建无关子系统。任何行为变化还需检查 `Domain::install_server_info_syncer`、`Domain::close` 与 cross-keyspace `RegisteredServerInfo` 的调用契约。
- 本文件已有 AsterSQL 2026 与 PingCAP Apache License 头；修改 Rust 生产代码时必须保留两者，并按仓库要求先运行 `cargo fmt --all`。本次仅新增说明文档，不触发该格式化要求。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/domain/serverinfo` 确认 `syncer.rs`、`syncer_test.rs`、Go 对照及相邻模块均已索引。
- 目标源码：`pkg/domain/serverinfo/syncer.rs` 全部 1,155 行；主要调用边通过 `callees NewSessionAndStoreServerInfo`、`callees ServerInfoSyncLoop`、`callees TopologySyncLoop`、`callees getInfo` 核对。
- crate 与数据边界：`pkg/domain/serverinfo/Cargo.toml`、`pkg/domain/serverinfo/lib.rs`、`pkg/domain/serverinfo/info.rs`。
- 上游入口：`pkg/domain/domain.rs::Domain::install_server_info_syncer`、`pkg/domain/domain.rs::Domain::close`、`pkg/domain/crossks/cross_ks.rs::RegisteredRuntimeFactory::create`、`RegisteredServerInfo::{start,stop}`。RustCodeGraph 的通用名称 callers 查询未返回 Rust 上游，因此这些入口又通过精确源码引用搜索确认。
- Go 对照：`pkg/domain/serverinfo/syncer.go` 全部 612 行；相关 Go 测试为 `pkg/domain/serverinfo/syncer_test.go` 中的 `TestTopology`、`TestCleanupStaleServerAndOwnerInfo`、`TestAssumedServerInfoSyncer` 以及 status endpoint 用例。
- Rust 独立测试：`pkg/domain/serverinfo/syncer_test.rs`。直接阅读并核对了 topology、陈旧清理、assumed keyspace、配置隔离、claim 选项、登记失败回滚、退出优先、真实 etcd（ignored）和并发登记用例；测试由 `lib.rs` 的 `#[path = "syncer_test.rs"]` 独立挂载，未与生产源文件混放。
- 本任务是纯文档分析，按计划未运行 Cargo。交付结构验证要求本文恰有上述 11 个固定二级标题；验证命令及退出码在任务完成时记录。

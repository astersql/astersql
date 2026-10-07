# `pkg/domain/infosync/info.rs`

## 文件定位

[`info.rs`](./info.rs) 是 `astersql-domain-infosync` crate 的核心门面。crate 入口 [`lib.rs`](./lib.rs) 以 `mod info; pub use info::*;` 重新导出本文件 API，因此调用方通常通过 `astersql_domain_infosync::*` 使用它，而不是直接引用私有模块。[`Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package` 指向 `pkg/domain/infosync`，表明本文件以同目录 [`info.go`](./info.go) 为主要移植对照。

它位于 Domain 元数据协调层：用进程级 `InfoSyncer` 聚合 etcd、PD HTTP、标签规则、放置规则、调度配置、TiFlash 和资源组管理器，并向规划、会话、服务器状态页及测试暴露统一查询/更新 API。RustCodeGraph 对该文件记录 116 个符号，并识别出 `pkg/planner/core/preprocess.rs`、`pkg/server/http_status.rs`、`pkg/session/runtime/normal_ddl_service.rs` 等使用文件；仓库搜索还确认 `GetAllServerInfo` 被 `pkg/session/runtime/ttl_metadata.rs`、`pkg/session/runtime/modify_column_cloud_planner.rs` 和 `pkg/util/disttask/idservice.rs` 调用。

当前 Rust 文件只实现了 Go 版本的一部分运行时接线。特别是 `InfoSyncer::init` 及五个 `init*Manager` 方法仍是空实现，不能把 Go 侧 etcd session、拓扑注册、完整 PD/资源管理初始化视为 Rust 已具备的能力。

## 核心职责

1. **维护全局同步器。** `globalInfoSyncer`、`global_slot`、`getGlobalInfoSyncer`、`setGlobalInfoSyncer` 管理 `Arc<InfoSyncer>`；几乎所有自由函数都先取全局实例，未初始化时返回 `Error::NotInitialized`。
2. **组装集群服务适配器。** `GlobalInfoSyncerInit` 根据是否提供 `PdHttpClient` 选择真实的标签/放置/调度管理器或内存 mock，创建 TiFlash mock 与资源组 mock，构造本机 `ServerInfo`，然后注册全局实例。
3. **同步和查询基础元信息。** `ReportMinStartTS` 写入 GC 安全下界，`GetAllServerInfo`、`GetPrometheusAddr`、`GetTiProxyServerInfo`、`GetTiCDCServerInfo` 分别读取节点、监控和组件拓扑信息。
4. **提供 PD 相关门面。** Bundle、标签规则、调度配置、Keyspace 配置和资源组 API 将请求委托给 `InfoSyncer` 持有的 trait object。
5. **协调 TiFlash/列存状态。** 本文件负责进度缓存的读写/清理、TiFlash 与 TiKV store 分组、超时熔断、列存索引进度聚合、放置规则配置和管理器关闭。
6. **保存进程内状态。** `server_info`、Prometheus TTL 缓存、内部会话 ID 集合及测试注入槽位都通过锁保护。

## 主要符号

- 常量 `ServerMinStartTSPath`、`ServerInformationPath`、`TopologyPrometheus`、`TopologyTiProxy`、`TopologyTiCDC` 定义 etcd 键空间；`TablePrometheusCacheExpiry` 为 10 秒；`RequestPDMaxRetry` 与 `RequestRetryInterval` 定义默认 PD 写重试为最多 3 次重试、间隔 200 毫秒。
- `EtcdClient` 是最小化的同步 KV 抽象，只有 `get`、`get_prefix`、`put`、`delete`。它使 `InfoSyncer` 可在不依赖具体 etcd SDK 的情况下使用真实或测试实现。
- `infoschemaMinTS` 通过 `GetAndResetRecentInfoSchemaTS(now)` 向 `ReportMinStartTS` 提供最近 infoschema 时间戳。
- `InfoSyncer` 是中心状态对象。公开字段保存 UUID、etcd/PD 客户端、管理器和 codec；私有字段保存 Prometheus 缓存、TiFlash mock 上下文、本节点 `ServerInfo` 与 `HashSet<usize>` 内部会话集合。
- `ColumnarProgressCollector` 是超时熔断路径的可注入采集接口。`SetColumnarProgressCollectorForTest` 和 `SetColumnarCollectTimeoutForTest` 以恢复闭包保护测试前状态。
- `GlobalInfoSyncerInit` 是主要构造入口；`getGlobalInfoSyncer` 是其余全局 API 的共同前置条件。
- `MustGetTiFlashProgressWithCircuitBreaker`、`MustGetTiFlashProgress`、`CalculateColumnarIndexProgress` 是进度计算主入口；`partitionTiFlashProgressStores` 是 crate 内可见的 store 分类函数。
- `PutRuleBundlesWithRetry` 区分不可重试的 `Error::DomainService` 与其他错误；`PutRuleBundlesWithDefaultRetry` 应用默认策略。
- `TiProxyServerInfo`、`TiCDCInfo` 通过 serde 字段重命名匹配 Go/etcd JSON 协议。
- 其余公开自由函数大多是薄门面，分别委托 `LabelRuleManager`、`PlacementManager`、`ScheduleManager`、`TiFlashReplicaManager` 或 `ResourceManagerClient`。

## 执行流程

**初始化流程：** `GlobalInfoSyncerInit` 克隆可选 `pdHTTPCli`，选择 PD 或 mock 标签/放置/调度实现，创建 `mockTiFlashReplicaManagerCtx` 和 mock 资源组客户端，读取全局配置中的 labels 构造 `ServerInfo`，再组装 `InfoSyncer`。随后调用当前为空的 `InfoSyncer::init`，写入 `globalInfoSyncer`，并向 `MockGlobalServerInfoManagerEntry` 登记 UUID。RustCodeGraph 的 callee 查询确认了到 `setGlobalInfoSyncer`、`InfoSyncer::init`、三个 PD 管理器构造及 mock 管理器入口的调用边。

**minStartTS 流程：** `ReportMinStartTS(store_min_ts, now)` 从可选 `infoCache` 取并重置最近 infoschema TS；无缓存时使用 `now`，随后取 `store_min_ts.min(schema_ts)` 写入锁内状态，再由 `storeMinStartTS` 通过无 keyspace 前缀的 etcd 客户端写到 `/tidb/server/minstartts/<uuid>`。未配置该客户端时，上报和删除都安静成功。

**服务发现流程：** `GetAllServerInfo` 在有 etcd 时扫描 `/tidb/server/info` 并反序列化所有值；无 etcd 时只返回本节点快照。Prometheus 查询先检查 `(address, Instant)` 是否仍在 10 秒 TTL 内，过期后直接读取 `/topology/prometheus` JSON 并缓存 `http://ip:port`。TiProxy 查询只接收 `/topology/tiproxy/.../info` 键，并从键截取地址；TiCDC 查询反序列化记录、去掉版本前导 `v`，再从键路径补齐 `ClusterID`。

**TiFlash/列存进度流程：** `MustGetTiFlashProgress` 首先命中 TiFlash 管理器缓存；未命中时 TiFlash 与 TiKV 两边默认各为 `1.0`，仅对非空 store 集合计算，然后取两者最小值并回写缓存。若测试注入了 `ColumnarProgressCollector`，熔断版本会开后台线程采集，用容量 1 的同步通道限时等待；超时设置取消原子标志并返回 `(1.0, true)`，采集线程断开则报错。没有注入 collector 时直接走普通进度函数。

`CalculateColumnarIndexProgress` 对每个 store 调用 `store_helper::CollectColumnarStatus`：访问失败且 store 为 `Tombstone` 时跳过，其他状态则立即报错；全文索引还要求响应显式包含 `fts-index-ready`，否则提示检查 TiKV 版本。最终按对应 ready 计数除以 total，total 为 0 时返回 0。

**规则写入流程：** `PutRuleBundlesWithRetry` 总共最多调用 `maxRetry + 1` 次。成功立即返回；`DomainService` 立即返回且不重试；其他错误记录为最后错误，并在尚有机会时休眠。TiFlash 表/分区配置先用 `MakeNewRule` 生成规则，分区路径批量写入后可选调用 `PostAccelerateScheduleBatch`；当前 Rust 实现故意保留已有表级规则。

## 数据与状态

- `globalInfoSyncer` 使用 `OnceLock<RwLock<Option<Arc<InfoSyncer>>>>`：槽位只初始化一次，但其中的 `Option` 可被测试或重复初始化覆盖。读取者获得 `Arc` 克隆，不持有全局读锁执行远端操作。
- `etcdCli`、`pdHTTPCli`、`minStartTS`、`managerMu`、`server_info`、`prometheusAddr` 和 `internal_sessions` 各自独立加锁，避免无关状态共享一个粗粒度锁。锁中毒均通过 `unwrap()` 触发 panic，而不是转为领域错误。
- `unprefixedEtcdCli`、`infoCache`、`tikvCodec` 和各管理器在构造后不替换；例外是可通过 `SetEtcdClient` 替换带前缀的 etcd 客户端，或通过测试函数暂时替换 PD 客户端。
- `prometheusAddr` 的空字符串只有在带时间戳时才可能形成有效缓存；正常路径必须先从 etcd 成功得到地址才同时写入地址和 `Instant`。
- `internal_sessions` 当前只存 `usize` 标识，不操作 `managerMu` 中的会话管理器；插入返回是否为新值，删除不存在的值仍成功。
- `TiProxyServerInfo` 与 `TiCDCInfo` 是线协议快照；serde 名称中的连字符、下划线不可随 Rust 字段名一起任意修改。

## 依赖与调用关系

crate 直接依赖 `astersql-config`、`ddl-label`、`ddl-placement`、`meta-model`、`store-helper`、`serde`、`serde_json`、`thiserror` 与带 tag `v0.4.2-aster.10` 的 `tikv-client`。本文件通过 `use crate::*` 使用 `types.rs` 的 `PdHttpClient`/store 类型、各 manager 模块的 trait 与实现、`mock_info.rs` 的 `ServerInfo`/全局 mock 表，以及 `error.rs` 的 `Error`/`Result`。

RustCodeGraph 的 callee 证据包括：`GlobalInfoSyncerInit -> setGlobalInfoSyncer / InfoSyncer::init / MockGlobalServerInfoManagerEntry / NewMockResourceManagerClient`；`ReportMinStartTS -> infoschemaMinTS::GetAndResetRecentInfoSchemaTS / storeMinStartTS`；`MustGetTiFlashProgressWithCircuitBreaker -> MustGetTiFlashProgress`；`PutRuleBundlesWithRetry -> PutRuleBundles`；`GetPrometheusAddr -> InfoSyncer::getPrometheusAddr`；两个组件拓扑门面分别调用对应的 `InfoSyncer` 私有方法。

索引对这些自由函数的反向 callers 查询没有返回完整调用边，因此上游事实以文件级 use 边和仓库搜索补充。生产侧可见调用包括：TTL 元数据和云端 DDL 规划读取 `GetAllServerInfo`，`pkg/util/disttask/idservice.rs` 用它做节点 ID 服务；`pkg/server/http_status.rs` 与 planner/session 文件依赖本 crate 的信息门面。`pkg/infoschema/tables.rs` 定义了接收 `ServerDiscovery` 的同名 TiProxy/TiCDC 转换函数，并非直接调用这里的零参数函数，扩展时要避免仅凭名称混淆调用链。

## 错误处理与边界

- 所有依赖全局实例的 API 在初始化前返回 `Error::NotInitialized`；`SetKeyspaceConfig` 还在 PD 客户端缺失时返回 `Error::PdHttpClientMissing`，PD 错误原样传播。
- etcd JSON 无效会由 serde 错误直接中止整次查询。TiProxy 会跳过非 `/info` 后缀或无法提取地址的键；TiCDC 会跳过路径段不足的键。
- `getPrometheusAddr` 会忽略一次 etcd 读取/解析错误并最终折叠为 `Error::PrometheusAddressNotSet`，因此调用者不能从该 API 区分“未设置”和“etcd/JSON 失败”。这与其他拓扑 API 的错误传播方式不同。
- `pdResponseHandler` 仅把 200、404、412 当成功；其他状态以响应体的有损 UTF-8 字符串生成 `Error::DomainService`。它只处理状态与 body，不负责像 Go handler 那样解析 200 响应对象。
- `PutLabelRule(None)`、空 `UpdateLabelRules`、空 `GetLabelRules` 都是无操作成功，避免无意义远端请求。
- `calculateColumnarProgressWithCtx` 对空集合返回 0；`MustGetTiFlashProgress` 对 TiFlash/TiKV 两组都为空则得到并缓存 1.0。两者是不同层级的既定边界，不能合并解释。
- `ConfigureTiFlashPDForPartitions` 在批量规则或加速调度失败时立即返回；参数 `tableID` 当前未使用，且不会删除表级规则。
- 所有 `RwLock`/`Mutex` 的 poison 都会 panic；该文件没有恢复策略。浮点进度也没有在本层强制裁剪到 `[0, 1]`。

## 并发与资源生命周期

`InfoSyncer` 通过 `Arc` 在调用者间共享，trait 均要求 `Send + Sync`。全局槽位和对象内部可变字段使用标准库 `RwLock`；远端调用前通常先克隆客户端或管理器引用并释放锁，例如 `getPrometheusAddr` 显式 `drop(cached)`，避免在 I/O 期间占用缓存读锁。

熔断路径为每次注入式采集启动一个 detached `thread::spawn`。超时后主线程只设置 `AtomicBool`，不会 join；采集器必须协作检查取消标志，才能及时释放线程和资源。测试中的 `BlockingCollector` 正是循环读取该标志。未注入 collector 的正常路径是同步调用，不创建线程。

测试修改全局 singleton、collector 和 timeout，`pkg/domain/infosync/info_test.rs::serial` 用进程级 `Mutex` 串行化相关用例。两个 setter 返回 `FnOnce` 恢复闭包；提前 panic 或漏调用恢复闭包会污染后续测试。生产关闭只通过 `CloseTiFlashManager` 委托 TiFlash 管理器；本文件没有全局 `InfoSyncer` 销毁、etcd session 关闭或后台线程统一回收流程。

## 与 Go 版本的对应关系

Rust 与 `pkg/domain/infosync/info.go` 保持的关键语义包括：Bundle 的 service error 不重试、普通错误最多尝试 `maxRetry + 1` 次；TiFlash 进度优先读缓存并取 TiFlash/columnar 最小值；分区表清理分区缓存而保留表 ID 缓存；PD 状态 404/412 兼容成功；列存全文索引要求 `fts-index-ready`；Tombstone store 采集失败可跳过；TiProxy/TiCDC JSON 字段和 TiCDC 版本前导 `v` 处理一致。

已确认的差异和未迁移能力如下：

- Go `GlobalInfoSyncerInit` 包装 PD HTTP caller/response handler，创建 `serverinfo.Syncer`，建立 etcd server/topology session，初始化全部 manager 与 affinity；Rust 的 `init` 和五个 `init*Manager` 为空，且资源组固定使用 mock，TiFlash 也固定以 mock context 起步。
- Go `ReportMinStartTS` 结合 store 快照、下界和内部事务 TS 并记录错误；Rust 接收已算好的 `store_min_ts`，只与 infoschema TS 取最小值，etcd 写失败向调用者返回。
- Go Prometheus 发现先查询 PD config 的 metric storage，再回退 etcd，并带更多 PD 可用性判断；Rust 只读取单个 etcd 键。
- Go TiProxy/TiCDC 查询有 context、超时和重试；Rust trait 是同步接口，单次 `get_prefix`，没有取消或重试。
- Go 内部会话 API 委托注入的 `sessmgr.Manager` 并存任意会话对象；Rust 仅维护独立的 `HashSet<usize>`，`managerMu` 与这三个 API 尚未接线。
- Go circuit breaker 用 context 取消完整进度调用；Rust 只有在注入 `ColumnarProgressCollector` 时使用原子取消，否则直接同步执行 `MustGetTiFlashProgress`。
- Go `GetServerInfoByID` 从 server info syncer/etcd 读取；Rust 当前从 `MockGlobalServerInfoManagerEntry` 查找。Go `UpdateServerLabel` 通过 syncer 更新持久化状态，Rust只修改本地 `server_info`。

因此新增功能应以 Go 文件为行为基准，但必须逐项验证 Rust 已有 trait 和管理器能力，不能仅复制函数名便宣称完成迁移。

## 扩展指南

- 扩展初始化或服务注册时，优先补 `InfoSyncer::init` 和对应 `init*Manager`，并在独立的 `info_test.rs` 增加生命周期/错误回滚用例；不要把测试放回生产源文件。接入真实外部 Rust 依赖时必须遵守仓库规则，在上游仓库移植、打 tag，并由 Cargo 使用统一 tag。
- 扩展 etcd 查询时，在 `EtcdClient` 增加最小必要能力并同步 `MemoryEtcd`；明确是否需要 Go 的 context、超时和重试语义。不要在持有 `RwLock` guard 时进行阻塞 I/O。
- 修改拓扑 JSON 结构或键路径时，同步检查 `TiProxyServerInfo`、`TiCDCInfo`、常量、Go 对照和 `etcd_injection_and_topology_decoding_match_go`；兼容旧字段或旧节点版本应有显式测试。
- 修改 TiFlash/列存进度时，保持“缓存优先”“空集合的层级差异”“Tombstone 跳过”“NextGen `tiflash_compute` 不归入任一组”等不变量，并更新 `test_tiflash_manager`、`columnar_index_progress_uses_index_type_and_rejects_old_tikv_for_fulltext`、`tiflash_progress_cache_and_partition_cleanup_match_go` 和 `tiflash_progress_store_partition_excludes_compute_nodes`。
- 修改重试策略时，同步验证总尝试次数、不可重试错误分类和 sleep 边界；生产代码中若引入取消能力，应避免无条件 `thread::sleep`。
- 将内部会话接回真正 session manager 时，需要先定义类型安全的会话标识/trait，明确 `managerMu` 与 `internal_sessions` 谁是权威状态，并核对 `pkg/server/server.rs` 与 `pkg/session/sessmgr/processinfo.rs` 的接口语义。
- 所有可用 Rust 生产修复都应保留 PingCAP Apache License，并在顶部保留/增加仓库要求的 `// Copyright 2026 AsterSQL.`；格式化后运行适用验证，但本次纯文档任务不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、`info.rs` 为 956 行/116 个符号；执行了目标文件 `node`、目录 `files`，以及 `GlobalInfoSyncerInit`、`ReportMinStartTS`、`MustGetTiFlashProgressWithCircuitBreaker`、`MustGetTiFlashProgress`、`PutRuleBundlesWithRetry`、`CalculateColumnarIndexProgress`、`GetAllServerInfo`、`GetPrometheusAddr`、`GetTiProxyServerInfo`、`GetTiCDCServerInfo`、`StoreInternalSession` 的 `query`/`callers`/`callees` 查询。反向 callers 对这些自由函数未产出完整结果，已用索引的文件级 use 信息和 `rg` 补证，未据此虚构调用边。
- 生产源码：`pkg/domain/infosync/info.rs`；crate 边界与测试装配：`pkg/domain/infosync/lib.rs`、`pkg/domain/infosync/Cargo.toml`。
- Rust 独立测试：`pkg/domain/infosync/info_test.rs`。重点覆盖 `test_put_bundles_retry`、`test_tiflash_manager`、`columnar_index_progress_uses_index_type_and_rejects_old_tikv_for_fulltext`、三个 Keyspace 配置用例、`tiflash_progress_cache_and_partition_cleanup_match_go`、`pd_status_and_tiflash_rule_contract_match_go`、`configure_partitions_preserves_existing_table_rule`、`etcd_injection_and_topology_decoding_match_go`、`tiflash_progress_store_partition_excludes_compute_nodes`。
- Go 对照：`pkg/domain/infosync/info.go`；Go 回归意图：`pkg/domain/infosync/info_test.go`，尤其是 `TestPutBundlesRetry`、`TestTiFlashManager`、`TestInfoSyncerMarshal` 和 Keyspace 配置测试。
- 直接依赖实现：`pkg/domain/infosync/{types,label_manager,placement_manager,schedule_manager,resource_manager_client,tiflash_manager,mock_info,error}.rs`；上游用例由 RustCodeGraph 文件级 use 及仓库搜索核验。
- 本文只描述当前源码，不把 Go 独有逻辑、空初始化钩子或未出现的调用边写成 Rust 已支持。结构验收以恰好存在本文规定的 11 个二级标题为准。

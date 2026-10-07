# `pkg/domain/infosync/types.rs`

源文件：[`types.rs`](./types.rs)

## 文件定位

`types.rs` 是 `astersql-domain-infosync` crate 的公共契约层：它集中定义 infosync 与 PD HTTP、Resource Manager、TiKV store/region 元数据之间交换的数据形状，以及两个可注入的客户端 trait。crate 根 `pkg/domain/infosync/lib.rs` 通过 `mod types; pub use types::*;` 将这里的符号全部重新导出，因此同 crate 的 `info.rs`、`region.rs`、`label_manager.rs`、`placement_manager.rs`、`schedule_manager.rs`、`tiflash_manager.rs` 和 `resource_manager_client.rs` 都从 crate 根使用这些类型。

该文件不负责创建网络客户端、保存全局 `InfoSyncer`，也不直接执行 PD 请求。真实调用发生在上述 manager 或 `InfoSyncer` 门面中；当前仓库可见的 `PdHttpClient` 实现主要是测试替身，而 `ResourceManagerClient` 的直接实现是 `resource_manager_client.rs` 中的内存 mock。`pkg/domain/infosync/Cargo.toml` 将本目录定义为独立 crate，并声明 `tikv-client`（固定 tag `v0.4.2-aster.10`）、`serde`、DDL label/placement 等依赖；没有 feature 条件控制本文件。

## 核心职责

1. 固定 PD HTTP 的线协议形状：`UpdateKeyspaceConfigParams`、`StoreMeta`/`StoreInfo`/`StoresInfo`、`RegionDistributions`、`LabelRulePatch` 使用 `serde` 显式保持 Go/PD 所需字段名；`ConfigValue` 以无标签枚举表示任意 JSON 值。
2. 抽象 PD HTTP 能力：`PdHttpClient: Send + Sync` 覆盖 keyspace 配置、placement bundle、region label、调度配置、region 复制/分布和 scheduler job 操作，使 infosync 上层不绑定具体 HTTP 实现。
3. 定义 Rust 侧资源组简化模型：`TokenLimitSettings`、`ResourceGroup`、`EventType`、`ResourceGroupEvent` 和 `ResourceGroupWatchResponse` 是内存 mock 与 tikv-client provider adapter 之间的中间表示。
4. 封装可克隆的 watch 接收端：`ResourceGroupWatchReceiver` 用 `Arc<Mutex<Receiver<_>>>` 让多个订阅句柄共享并竞争消费同一个标准库 MPSC 队列。
5. 抽象 Resource Manager：`ResourceManagerClient: Send + Sync` 同时提供 metastorage `Get`/`Put`、资源组 CRUD、watch，以及尚未完整接线的令牌桶/按 revision watch/load 默认方法。
6. 生成资源组 watch 键：`group_settings_path_prefix` 根据 keyspace ID 选择全局或 keyspace 隔离路径。

## 主要符号

- `UpdateKeyspaceConfigParams { Config, Preconditions }`：两个 map 的值都是 `Option<String>`；`None` 可表达删除或“期望不存在”。`Preconditions` 为空时不序列化，支持 PD 的乐观并发更新格式（`types.rs:20-29`）。
- `StoreMeta`、`StoreInfo`、`StoresInfo`：分别描述节点元数据、PD 单 store 包装和列表响应。`Labels` 是字符串键值；`Count` 与 `Stores.len()` 没有在类型层强制相等（`types.rs:31-68`）。
- `RegionDistributions`：保存 region 总数和 `store ID -> peer 数`；JSON 对象的字符串键由 serde 转为 `i64`（`types.rs:70-79`，由 `types_test.rs:69-75` 验证）。
- `KeyRange`：字节形式半开区间 `[start_key, end_key)`；类型本身不校验顺序或非空（`types.rs:81-88`）。
- `ConfigValue`：`#[serde(untagged)]` 的 Bool、Number、String、Array、Object、Null 六分支，对应 Go 的 `any`/JSON 配置值（`types.rs:90-100`）。浮点数使用 `f64`，因此调用方需自行处理精度和非有限值的序列化限制。
- `PdHttpClient`：线程安全的动态分派边界。所有方法都有默认实现且均返回 `Error::External("... is unsupported")`，因此实现者可只覆盖所需能力，但未覆盖的方法不会静默成功（`types.rs:102-198`）。
- `LabelRulePatch { DeleteRules, SetRules }`：PD label patch 的删除 ID 与覆盖写规则集合；实际 mock 在 `label_manager.rs:90-100` 按“先删后写”执行（`types.rs:200-209`）。
- `TokenLimitSettings`、`ResourceGroup`：分别保存 RU 填充/突发限制与资源组名称、优先级。`BurstLimit == -1` 表示无限突发；Rust 模型只保留当前 mock/adapter 使用的 Go proto 字段（`types.rs:211-228`）。
- `EventType::{Put, Delete}`、`ResourceGroupEvent`、`ResourceGroupWatchResponse`：表达一次资源组变更及 watch envelope；mock 的 `CompactRevision` 固定为 0（`types.rs:229-251`、`resource_manager_client.rs:55-65`）。
- `ResourceGroupWatchReceiver`：可克隆句柄；`recv_timeout` 在互斥锁内等待，返回标准库 `RecvTimeoutError`，锁中毒则 panic（`types.rs:253-265`）。
- `ResourceManagerClient`：`Get`/`Put` 使用 `tikv_client` protobuf 和 `LookupError`；CRUD 使用 crate `Result`；`watch` 返回可选共享接收器。`AcquireTokenBuckets`、`WatchResourceGroup`、`LoadResourceGroups` 当前默认返回空结果/`None`/revision 0，属于明确占位（`types.rs:267-312`）。大写 `Watch` 仅转调小写 `watch`。
- `group_settings_path_prefix(u32) -> Vec<u8>`：`u32::MAX` 对应 `resource_group/settings`，其他 ID 对应 `resource_group/keyspace/settings/{id}`（`types.rs:314-321`）。

## 执行流程

PD HTTP 主链如下：`GlobalInfoSyncerInit`/测试注入将 `Arc<dyn PdHttpClient>` 放入 `InfoSyncer.pdHTTPCli`（`info.rs:64-99,156-160`）；上层门面取得全局 `InfoSyncer`，检查客户端是否存在，然后经动态分派调用本 trait。具体例子包括：

1. `SetKeyspaceConfig` 接收 `UpdateKeyspaceConfigParams`，从 `InfoSyncer` 取客户端并调用 `update_keyspace_config`（`info.rs:391-400`；RustCodeGraph 显示测试 `test_set_keyspace_config*` 为调用者）。
2. `PDLabelManager` 将 `PutLabelRule`、`UpdateLabelRules` 和查询转成 `set_region_label_rule`、`patch_region_label_rules` 等方法；内存 manager 则直接消费同一个 `LabelRulePatch`（`label_manager.rs:39-71,81-120`）。
3. `PDPlacementManager`、`PDScheduleManager` 分别消费 placement bundle 方法和 `ConfigValue` 调度配置方法（`placement_manager.rs`、`schedule_manager.rs`）。
4. `region.rs` 把调用者的 start/end 字节包装为 `KeyRange`，再调用复制状态、region 分布、scheduler 配置/创建/取消方法（`region.rs:35-114`）。
5. `tiflash_manager.rs` 和 `info.rs` 使用 `StoreInfo`/`StoresInfo`/`RegionDistributions` 计算或暴露 TiFlash、列存和 store 状态；这些结构只是传输容器，计算逻辑不在本文件。

资源组 mock 主链如下：`NewMockResourceManagerClient` 创建容量 100 的 `sync_channel` 和默认资源组；增、改、删操作构造 `ResourceGroupWatchResponse` 并同步发送；`watch(group_settings_path_prefix(keyspaceID))` 返回持有同一 receiver 的克隆句柄；消费者调用 `recv_timeout` 取得事件（`resource_manager_client.rs:25-143`）。`ResourceManagerProviderAdapter` 再把简化 `ResourceGroup` 转为 tikv-client proto，并拒绝无法转成 `u64` 的负 `FillRate`（`resource_manager_client.rs:145-205`）。

## 数据与状态

本文件的大多数类型是拥有所有权的值对象，没有内部可变状态。map/list 的顺序不构成契约：`HashMap` 和 mock 的 `list_resource_groups` 都不保证稳定遍历顺序。`Default` 会产生空字符串、0、空集合等 Rust 零值；这些零值可构造但不代表远端 PD 一定接受。

唯一有状态对象是 `ResourceGroupWatchReceiver`，其状态实际位于共享 `Receiver` 中。克隆 receiver 不会广播复制事件，而是共享队列：一个句柄消费后，其他句柄看不到同一项。发送端及容量由 `mockResourceManagerClient` 持有；队列在订阅前已创建，所以订阅前事件仍会保留。`resource_manager_client_test.rs:109-196` 分别验证预订阅保留、容量 100 导致第 101 次发送阻塞，以及两个 receiver 竞争消费。

序列化状态由字段属性决定：`ConfigValue` 无额外类型标签；`UpdateKeyspaceConfigParams.Preconditions` 为空时省略；其余显式 `rename` 保持 snake_case wire name。`types_test.rs:19-76` 校验 keyspace 参数、store 嵌套、label patch 和 region 分布反序列化。

## 依赖与调用关系

- 标准库：`HashMap` 保存动态键值；`Arc<Mutex<Receiver<_>>>` 提供共享 watch 接收；`Duration`/`RecvTimeoutError` 定义超时 API。
- `serde`：为 PD HTTP 数据结构提供 JSON 编解码；`serde_json` 的直接使用主要在测试和 manager 的规则持久化中。
- crate 内部：`Error`/`Result` 统一错误边界；`label::Rule` 与 `placement::Bundle` 分别来自 `astersql-ddl-label` 和 `astersql-ddl-placement` 的 crate 根再导出。
- 外部 `tikv-client`：只直接出现在 `ResourceManagerClient::Get`/`Put` 的 protobuf 与错误类型中；Cargo 将其锁定到 astersql/client-rust 的发布 tag，文档没有引入本地覆盖。
- 上游消费者：RustCodeGraph 的文件关系显示 `types.rs` 至少被 `info.rs`、`info_test.rs`、`resource_manager_client.rs`、`resource_manager_client_test.rs`、`resource_group_controller_options.rs` 等使用。包内直接搜索还确认了 `region.rs`、label/placement/schedule/TiFlash managers。
- 下游动态边界：trait 调用目标运行时决定，静态调用图不会解析到具体 HTTP 客户端；因此应把 manager 到 trait 方法视为经过 `Arc<dyn ...>` 的动态边。

## 错误处理与边界

`PdHttpClient` 默认方法全部返回带具体操作名的 `Error::External`，可快速暴露实现缺失。上层还可能在客户端未注入时先返回 `Error::PdHttpClientMissing`；`region.rs:38-56` 是例外：复制状态查询在没有客户端时返回 Pending，而 region 分布和 scheduler 操作返回缺客户端错误。

`ResourceManagerClient` 没有为 CRUD 提供默认实现，具体实现必须决定缺失/重复语义。当前 mock 对重复 add、缺失 get/delete 返回 `Error::External`，相关状态保持不变（`resource_manager_client.rs:98-135`；`resource_manager_client_test.rs:92-106`）。相比之下，trait 中三个占位默认方法会返回空值而非错误；调用方不能把空返回误判为真实远端已成功处理。

`ResourceGroupWatchReceiver::recv_timeout` 原样传播 `Timeout` 或 `Disconnected`，但对 mutex 中毒使用 `expect`，会 panic。mock `publish` 在 receiver 异常断开时也使用 `expect`；正常设计中 client 始终保留 receiver，所以测试路径不会断开。同步通道满时发送者会阻塞，没有丢弃或背压错误。`KeyRange`、`StoresInfo.Count`、资源组名称和优先级都不在此层校验；这些属于调用方/远端契约边界。

## 并发与资源生命周期

两个 client trait 都要求 `Send + Sync`，允许 `InfoSyncer` 通过 `Arc<dyn Trait>` 跨线程共享。`ResourceGroupWatchReceiver` 的 `Mutex` 保证标准库单消费者 receiver 不被并发调用，但由于锁覆盖整个 `recv_timeout`，一个等待中的消费者会阻塞同 receiver 的其他消费者进入等待。

watch receiver 和 channel 的实际生命周期绑定到 mock client：client 持有 sender 与一个 receiver 克隆，外部克隆只延长共享 wrapper 的生命周期。所有 watcher 竞争同一队列，不是每订阅者独立广播。容量 100 形成同步背压，第 101 个未消费事件会阻塞生产线程；这一行为与 Go mock 的缓冲 channel 对齐，并由独立 Rust 测试验证。

数据结构自身没有锁、后台任务、事务或显式资源释放。`PdHttpClient` 也没有 `close` 方法，连接池/HTTP 资源的生命周期必须由具体实现及持有它的 `Arc` 管理。本文件没有条件编译项。

## 与 Go 版本的对应关系

Go 的 infosync 包没有同路径 `types.go`；对应公共类型和客户端接口主要来自 `github.com/tikv/pd/client/http`、`github.com/tikv/pd/client`、kvproto，而 Go `pkg/domain/infosync/info.go`、`region.go`、`resource_manager_client.go` 直接消费它们。Rust 因 crate 边界和可测试性在本文件建立本地等价契约，因此不是对单一 Go 文件的逐行翻译。

- `UpdateKeyspaceConfigParams`、store/region 类型、`LabelRulePatch` 和 `PdHttpClient` 对齐 Go 的 `pdhttp`/`pd` 客户端形状；Rust 独立测试用精确 JSON 断言锁定 wire name。
- `ConfigValue` 对应 Go scheduler API 的 `any`/`map[string]any`；Rust 通过封闭 JSON 枚举获得可序列化、可比较的表示。
- Go `resource_manager_client.go:33-178` 的 mock 使用 `map`、`sync.RWMutex` 和容量 100 的 `chan *WatchResponse`；Rust 的 `ResourceGroup`/event envelope 与 `Mutex<HashMap<...>>`、`sync_channel(100)` 保留 CRUD、默认组、事件类型、预订阅缓冲和共享消费语义。
- Rust 对 Go proto 做了有意简化：`ResourceGroup` 只保留 Name、RU token limit、Priority；`ResourceGroupWatchResponse` 直接携带已解码的 `ResourceGroupEvent`，而 Go watch 传输 protobuf `KeyValue.Value`。adapter 在需要接入 tikv-client 时重建 proto。
- Rust 的缺失 delete 会报错，而当前 Go mock 直接删除后 marshal 查询结果；独立 Rust 测试明确把“缺失时报错且保持状态”作为当前 Rust 契约。扩展时不能只凭 Go 零值行为覆盖既有 Rust 测试意图。
- `group_settings_path_prefix` 对齐 Go 的 `pd.GroupSettingsPathPrefixBytes`：Null keyspace（Rust 使用 `u32::MAX`）走全局路径，其余走 keyspace 路径；`types_test.rs:7-17` 固定了两种结果。

## 扩展指南

新增 PD HTTP 能力时，应先在 `PdHttpClient` 添加语义明确的方法和默认 unsupported 错误，再在真正的注入实现/测试替身中覆盖，并从对应 manager 或 `info.rs`/`region.rs` 门面接线。若请求或响应进入 JSON，需在 `types_test.rs` 增加 wire-name、空值和 round-trip 测试；不要把测试内嵌回生产源文件。

调整 store/region/key range 类型时，要同步检查 `tiflash_manager.rs`、`info.rs`、`region.rs` 及其独立测试。尤其不要假设 `Count == Stores.len()`、map 有稳定顺序或 key range 已校验。修改 `ConfigValue` 需评估现有 schedule manager 的相等性、浮点精度和 JSON 兼容性。

扩展资源组 proto 字段时，至少同步修改 `ResourceGroup`、`ResourceManagerProviderAdapter::get_resource_group`、mock CRUD 事件和 `resource_manager_client_test.rs`。若要实现 `AcquireTokenBuckets`、`WatchResourceGroup`、`LoadResourceGroups`，应把现在的占位返回替换成可观察行为与错误，并新增独立测试，避免“空结果”继续伪装成成功。

改变 watch 语义风险最高：广播订阅、无界队列或非阻塞发送都会偏离当前 Go 对齐的共享容量 100 队列。若确需改变，应同时更新 `ResourceGroupWatchReceiver`、mock client 和容量/多消费者测试，并评估锁内等待造成的吞吐与饥饿。改变 keyspace 路径必须同时核对上游 `pd.GroupSettingsPathPrefixBytes` 和两类 keyspace 测试。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/domain/infosync` 确认目标及相邻文件已索引；`node --file pkg/domain/infosync/types.rs` 读取 321 行/59 个符号，并报告至少 9 个使用文件；`query PdHttpClient --kind trait`、`query ResourceManagerClient --kind trait` 消除跨仓库同名符号；针对 `SetKeyspaceConfig`、manager、resource watch、store 消费链运行了 `explore`。动态 trait 分派没有静态 callee 边，本文据 manager 源码只陈述可验证的动态边界。
- 生产源码：`pkg/domain/infosync/types.rs`、`lib.rs`、`info.rs`、`region.rs`、`label_manager.rs`、`placement_manager.rs`、`schedule_manager.rs`、`tiflash_manager.rs`、`resource_manager_client.rs`。
- crate/依赖：`pkg/domain/infosync/Cargo.toml`，包括 `package.metadata.porting.go-package = "pkg/domain/infosync"` 和固定 tag 的 `tikv-client`。
- Rust 独立测试：`pkg/domain/infosync/types_test.rs` 验证路径与 JSON；`resource_manager_client_test.rs` 验证默认组、CRUD、错误保持状态、watch envelope、100 容量共享消费和 provider 响应；`info_test.rs`、`region_test.rs` 验证 trait 注入后的 keyspace/region/scheduler 边界。
- Go 对照：`pkg/domain/infosync/info.go`、`region.go`、`label_manager.go`、`resource_manager_client.go` 及 `info_test.go`；PD 客户端公共类型不定义在 infosync 同路径文件中，本文没有将外部实现臆测为仓库内现状。
- 本任务是纯文档分析，按任务约束未运行 Cargo。最终结构验证要求本文恰有上述 11 个固定二级标题。

# `pkg/domain/infosync/resource_manager_client.rs`

## 文件定位

本文件属于 `astersql-domain-infosync` crate（见 `pkg/domain/infosync/Cargo.toml`），实现资源组客户端的内存 Mock，并通过 `pkg/domain/infosync/lib.rs` 重新导出。它使用 `types.rs` 中的 `ResourceManagerClient`、资源组数据结构和 watch 信封，不负责连接真实 PD；`GlobalInfoSyncerInit` 在 `pkg/domain/infosync/info.rs` 中始终用 `NewMockResourceManagerClient` 装配当前 Rust `InfoSyncer`。

文件末尾的 `ResourceManagerProviderAdapter` 还承担一层边界适配：把 infosync 自有的 `ResourceManagerClient` 转成 `tikv_client::resource_group_lookup::ResourceGroupProvider`，供 `pkg/domain/runaway.rs` 中的 `ResourceGroupLookupController` 使用。RustCodeGraph 将该文件列为被 `info.rs`、`runaway.rs`、`runaway_test.rs`、`cmd/tidb-server/main.rs` 和本文件独立测试引用的生产文件。

## 核心职责

1. `NewMockResourceManagerClient` 创建一个无需外部服务的资源组存储，预置与 Go Mock 一致的 `default` 资源组。
2. `mockResourceManagerClient` 实现资源组列举、查询、新增、覆盖修改和删除，并为成功的写操作生成 watch 事件。
3. `Get`、`Put` 提供最小 metastorage 接口形状：返回带非空 header 的空响应，但不保存或读取 key/value。
4. `watch` 只接受当前 keyspace 的资源组设置前缀，并返回同一个共享消费队列。
5. `ResourceManagerProviderAdapter` 将内部资源组转换成 `tikv-client` protobuf 资源组，`NewMockResourceGroupProvider` 提供可直接交给资源组查询控制器的 trait object。

因此它是可执行的 Mock/适配层，而不是真实 Resource Manager 客户端；不能把成功返回理解为已经向 PD 持久化。

## 主要符号

- `DefaultResourceGroupName: &str`：值为 `"default"`，是构造时预置条目的键和名称。
- `mockResourceManagerClient`：私有实现体。`keyspaceID` 决定合法 watch 前缀，`groups` 是受 `Mutex` 保护的 `HashMap<String, ResourceGroup>`，`event_sender`/`event_receiver` 是容量 100 的同步通道两端。
- `NewMockResourceManagerClient(u32) -> Box<dyn ResourceManagerClient>`：公开构造器。默认组的 `FillRate` 为 `i32::MAX as i64`、`BurstLimit` 为 `-1`、`Priority` 为 `8`。
- `publish(EventType, ResourceGroup)`：私有事件封装函数；每个响应只有一个事件，`CompactRevision` 固定为 `0`，发送失败会 panic。
- `ResourceManagerClient for mockResourceManagerClient`：实现 `Get`、`Put`、CRUD 和 `watch`。`AcquireTokenBuckets`、`WatchResourceGroup`、`LoadResourceGroups` 仍使用 `types.rs` 中 trait 的占位默认实现，不在本文件覆写。
- `ResourceManagerProviderAdapter(Arc<dyn ResourceManagerClient>)`：公开 newtype，转发 provider 的 `get`/`put`，并在 `get_resource_group` 中执行内部类型到 protobuf 的转换。
- `NewMockResourceGroupProvider(u32)`：组合 `NewMockResourceManagerClient` 与 adapter，返回 `Arc<dyn tikv_client::resource_group_lookup::ResourceGroupProvider>`。

本文件没有条件编译项；测试装配位于 `lib.rs` 的 `#[cfg(test)] #[path = "resource_manager_client_test.rs"]`，测试逻辑没有嵌入生产文件。

## 执行流程

构造流程从 `NewMockResourceManagerClient` 开始：建立容量为 100 的 `sync_channel`，构造默认资源组，把它放入只含一个条目的 `HashMap`，再将接收端包装成 `Arc<Mutex<Receiver<_>>>` 后返回 boxed trait object。`GlobalInfoSyncerInit` 将其转成 `Arc<dyn ResourceManagerClient>` 并存入 `InfoSyncer.resourceManagerClient`。

读取流程中，`list_resource_groups` 加锁后克隆所有 map value，顺序由 `HashMap` 决定；`get_resource_group` 按名称克隆快照，缺失时返回 `Error::External("the group ... does not exist")`。返回克隆值意味着调用者不能绕过接口修改内部状态。

写入流程如下：

1. `add_resource_group` 获取 map 锁，先拒绝重复名称，再插入克隆并发布 `Put` 事件。
2. `modify_resource_group` 不检查旧条目是否存在，直接插入或覆盖，并发布 `Put` 事件。
3. `delete_resource_group` 先移除并取得旧快照；名称缺失则立即报错且不发事件，成功时发布携带被删除快照的 `Delete` 事件。
4. 三者成功均返回固定字符串 `"Success!"`。

订阅流程由 `watch` 比较传入字节串与 `group_settings_path_prefix(keyspaceID)` 的完整结果；不相等返回 `None`，相等返回共享接收器的克隆。事件通道在构造客户端时已经存在，所以订阅前的事件仍会排队。它不是广播：两个接收器竞争同一队列中的不同事件。

provider 流程中，`NewMockResourceGroupProvider` 先构造客户端，再包入 `ResourceManagerProviderAdapter`。adapter 的 `get`/`put` 原样转发；`get_resource_group` 将内部错误转为 `LookupError::Other`，把 `FillRate` 以 `u64::try_from` 转换，随后构造 `RuMode` protobuf 资源组并返回 `Some`。

## 数据与状态

资源组以名称为唯一键；添加时名称重复不会覆盖，修改时同名覆盖且允许以“修改”创建新条目。`ResourceGroup` 只保留名称、`TokenLimitSettings` 和优先级，adapter 输出 protobuf 时显式填入 RU 模式、priority、fill rate 与 burst limit，其余 protobuf 字段使用默认值。

状态变更和事件均携带 `ResourceGroup` 克隆。事件信封的 revision 恒为零，因此消费者不能依靠本 Mock 恢复真实 PD 的 revision/compaction 语义。`Get`/`Put` 忽略输入参数，不影响 `groups`，也不产生 watch 事件。

默认组不是特殊保护对象：本实现允许调用普通修改和删除接口处理 `default`。当前独立测试只验证它创建时存在且字段正确，没有规定其不可删除。

## 依赖与调用关系

直接标准库依赖是 `HashMap`、`Arc`、`Mutex` 和 `std::sync::mpsc`。crate 内依赖来自 `types.rs` 与 `error.rs`：`ResourceManagerClient` 定义接口，`ResourceGroupWatchReceiver` 定义串行接收语义，`group_settings_path_prefix` 构造 keyspace 路径，`Error::External` 承载 CRUD 错误。

外部依赖只有 `tikv-client`，由 `pkg/domain/infosync/Cargo.toml` 固定到 `astersql/client-rust` 的 `v0.4.2-aster.10` tag；本文件使用其 metastorage protobuf、资源组 protobuf、`LookupError` 和 `ResourceGroupProvider`。Cargo metadata 把该 crate 对应到 Go 包 `pkg/domain/infosync`。

RustCodeGraph 的关键静态边包括：`GlobalInfoSyncerInit -> NewMockResourceManagerClient`；`NewMockResourceGroupProvider -> NewMockResourceManagerClient` 和 `ResourceManagerProviderAdapter`；adapter 方法再动态转发到内部 trait object。`pkg/domain/runaway_test.rs::transient_provider` 以 `NewMockResourceGroupProvider(0)` 作为 provider stub 的基础对象，`pkg/domain/runaway.rs::init_resource_groups_controller` 则把 provider 交给 `ResourceGroupLookupController::new`。

## 错误处理与边界

- 查询不存在、重复添加、删除不存在均返回 `Error::External`；失败路径不产生 watch 事件。修改不存在的名称不是错误，而是插入。
- adapter 将内部查询错误压平成字符串形式的 `LookupError::Other`，不会保留原错误类型层次。
- provider 转换要求 `FillRate >= 0`；负值会返回 `LookupError::Other("negative resource group fill rate")`。`BurstLimit` 保持有符号数，因而 `-1` 能表达无限突发。
- `Get`/`Put` 总是返回成功和非空默认 header，这仅满足 mock provider 的元数据接口形状；它们不验证 key，也不模拟存储故障。
- `watch` 要求字节完全匹配合法前缀，并非“starts_with”匹配；错误 keyspace 或其他 key 返回 `None`。
- 所有 map 锁都用 `unwrap()`；锁中发生 panic 后，后续访问会因 poisoned mutex 再次 panic。`publish` 也用 `expect` 处理接收端断开。不过客户端自身持有接收端，正常生命周期内不会因所有外部订阅者释放而断开。

## 并发与资源生命周期

`ResourceManagerClient: Send + Sync`，`groups` 的单个 `Mutex` 串行化 CRUD 和列举。写方法在持有该锁期间调用同步 `publish`；当 100 个缓冲槽已满时，下一次写会阻塞，同时继续占有 map 锁，直到某个 watcher 消费事件。`watch_capacity_and_shared_consumer_semantics` 明确验证第 101 次写先阻塞、消费一个事件后才完成。

`ResourceGroupWatchReceiver` 的克隆只克隆 `Arc`，内部仍是同一个被 `Mutex` 保护的 `Receiver`。因此任一时刻只有一个线程执行 receive，并且事件只被一个消费者取走；这与 Go Mock 返回同一 channel 的竞争消费语义一致，不具备 fan-out。发送端和客户端保留的接收端都与客户端同生命周期；没有关闭、后台任务或显式清理流程。

持锁同步发送是重要扩展约束：如果新增写路径也调用 `publish`，必须评估满队列导致 CRUD 读写整体停顿的风险；若要改成广播或异步发送，需要同时改变既有共享消费者契约和独立测试。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/domain/infosync/resource_manager_client.go`。两端都预置同名 default 组，使用容量 100 的事件通道，以名称 map 实现 CRUD，新增/修改发 PUT、删除发 DELETE，并仅为当前 keyspace 的资源组设置路径返回 watch 通道。Rust 测试 `default_group_matches_go_mock`、`crud_and_watch_events_match_go_mock`、`watch_envelope_retains_pre_subscription_events` 和 `watch_capacity_and_shared_consumer_semantics` 专门固定这些移植语义。

已确认的差异如下：

- Go 使用 `sync.RWMutex`，读取可并发；Rust 使用单个 `Mutex`，所有访问串行。
- Go map 和事件携带 protobuf 指针；Rust 使用自有精简结构并克隆值，直到 adapter 边界才构造 protobuf。
- Go 的删除直接读取 map 后删除，再尝试 marshal；Rust 明确把缺失删除定义为 `Error::External`，独立测试固定了该行为。
- Go 类型直接满足 PD 的复合客户端接口，并包含三个返回空值的令牌桶/资源组加载占位方法；Rust 将这些占位放在 `ResourceManagerClient` trait 默认方法中，另以显式 adapter 满足 `tikv-client` provider trait。
- Go watch 响应承载序列化 protobuf bytes；Rust 事件直接承载已解析的 `ResourceGroup`，且没有序列化失败路径。

仓库中未发现独立的 `resource_manager_client_test.go`；Go 对照证据来自生产实现，细粒度行为回归由 Rust 独立测试承担。

## 扩展指南

新增资源组字段时，应先扩展 `types.rs::ResourceGroup`/相关设置，再同步修改默认组构造、`ResourceManagerProviderAdapter::get_resource_group` 的 protobuf 映射和 `resource_manager_client_test.rs` 的构造辅助函数及断言。尤其要决定字段缺省值、数值转换失败和未知枚举的处理方式，避免静默丢失 provider 所需信息。

新增 CRUD 行为应接入对应 trait 方法，并保持“状态成功变更后发布恰好一个事件”的顺序；回归测试继续放在独立的 `pkg/domain/infosync/resource_manager_client_test.rs`，不要放回生产文件。边界测试至少覆盖重复/缺失、修改插入语义、事件类型和快照、错误 keyspace、订阅前排队以及满缓冲阻塞。

若实现真实 metastorage、revision 或 token bucket 功能，不应继续扩张本 Mock 的无条件成功桩；应在真实客户端实现或 `types.rs` 接口层明确契约，并同步检查 `info.rs` 的装配选择。若改变通道容量、持锁发送或共享接收器模型，需要评估兼容性和吞吐/死锁风险，并更新并发测试。

若 provider 要支持负 fill rate、更多 RU 模式或 runaway 字段，应修改 adapter 的转换逻辑并增加 provider 专项测试；当前测试只验证默认组成功转换，尚未覆盖负 fill rate 的错误分支。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/domain/infosync` 确认目标、Go 对照和独立测试；`node --file pkg/domain/infosync/resource_manager_client.rs` 核对全部 205 行及 21 个符号。
- RustCodeGraph 调用证据：`explore` 核对 `NewMockResourceManagerClient`、CRUD、watch、provider 的调用者；`callees NewMockResourceManagerClient` 核对默认常量、资源组、设置和接收器构造；`callees NewMockResourceGroupProvider` 核对构造器与 adapter 组合；`node` 核对 `info.rs::GlobalInfoSyncerInit` 和 `runaway.rs::init_resource_groups_controller`。
- 接口与装配：读取 `pkg/domain/infosync/types.rs` 中 `TokenLimitSettings`、`ResourceGroupWatchReceiver`、`ResourceManagerClient` 和 `group_settings_path_prefix`；读取 `pkg/domain/infosync/lib.rs` 的模块导出和独立测试声明；读取 `pkg/domain/infosync/Cargo.toml` 的 crate、porting metadata 与带 tag 的 `tikv-client` 依赖。
- Go 对照：RustCodeGraph 完整读取 `pkg/domain/infosync/resource_manager_client.go`，逐项核对默认组、锁/map、CRUD、容量 100 的 channel、事件 envelope、metastorage 空响应和占位方法。
- 测试证据：RustCodeGraph 完整读取 `pkg/domain/infosync/resource_manager_client_test.rs` 的六个测试，并读取 `pkg/domain/runaway_test.rs::transient_provider` 对 `NewMockResourceGroupProvider` 的实际使用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另运行任务给定的 11 章节结构检查，并人工复核本文没有把 Mock 行为描述成真实 PD 持久化能力。

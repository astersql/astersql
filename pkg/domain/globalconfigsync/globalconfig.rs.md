# [`pkg/domain/globalconfigsync/globalconfig.rs`](globalconfig.rs)

## 文件定位

本文件是 `astersql-domain-globalconfigsync` crate 的业务实现，提供“配置项数据模型 + PD 写入抽象 + 有界通知队列”这一组最小能力。crate 入口 `pkg/domain/globalconfigsync/lib.rs` 通过 `pub mod globalconfig` 和 `pub use globalconfig::*` 同时暴露模块与其中的公开 API；`pkg/domain/globalconfigsync/Cargo.toml` 指定 `lib.rs` 为库入口，并声明 `crossbeam-channel`、`log`、`thiserror` 三个直接依赖。

它对应 Go 包 `pkg/domain/globalconfigsync/globalconfig.go`，但当前 Rust 应用集成度与 Go 不同：全仓 Rust 生产源码中没有 `GlobalConfigSyncer::new`、`notify`、`recv_notification` 或实例方法 `store_global_config` 的调用者，已找到的直接调用均在本目录的独立 Rust 测试中。根 `Cargo.toml` 提供 `facade_domain_globalconfigsync` 别名，`pkg/domain/Cargo.toml` 仅在 `cfg(windows)` 依赖区声明该 crate；这只能证明依赖/门面接线，不能证明 Rust `Domain` 已启动消费循环。相反，Go 的 `pkg/domain/domain.go` 已完成构造、通知、后台消费与退出控制。

## 核心职责

- `GlobalConfigEventType` 与 `GlobalConfigItem` 表示一条要写入 PD 全局配置存储的事件，保留事件类型、名称、文本值和二进制 payload。
- `GlobalConfigClient` 把具体 PD 客户端收窄为本同步器所需的单一写接口，使实现不依赖某个具体客户端类型，也便于测试替身注入。
- `GlobalConfigSyncer` 持有可选客户端和容量为 8 的 MPMC 有界通道；生产者调用 `notify` 入队，消费者调用 `recv_notification` 出队，再显式调用 `store_global_config` 写入。
- 本文件不创建后台线程、不自动把队列内容刷入 PD，也不负责把 SQL 系统变量名转换成 PD 键名；这些工作必须由上层调用者完成。Go 版的对应上层是 `Domain.NotifyGlobalConfigChange` 与 `globalConfigSyncerKeeper`，Rust 当前未发现等价生产接线。

## 主要符号

- `NOTIFY_CHANNEL_CAPACITY: usize = 8`：私有常量，决定 `bounded` 通道深度；与 Go `make(chan pd.GlobalConfigItem, 8)` 对齐。
- `GlobalConfigEventType`：`#[repr(i32)]` 的公开枚举。`Put = 0` 是默认值，`Delete = 1`；数值与 Go/PD 的 `pdpb.EventType` 约定对齐。修改判别值会影响跨语言/协议兼容性。
- `GlobalConfigItem`：公开可克隆值类型，字段均公开：`event_type`、`name`、`value`、`payload`。`GlobalConfigItem::new(name, value)` 总是构造 `Put`，并把 payload 置空；删除事件或带 payload 的事件必须用结构体字段显式构造。
- `GlobalConfigError::Client(String)`：当前唯一客户端错误变体，由 `thiserror::Error` 生成展示文本。同步器本身不增加上下文或重试。
- `GlobalConfigClient: Send + Sync`：可在线程间共享的客户端 trait。`store_global_config(&self, prefix, items)` 接收前缀和切片；具体网络、超时与取消语义留给实现者。
- `GlobalConfigSyncer`：包含 `Option<Arc<dyn GlobalConfigClient>>`、`Sender<GlobalConfigItem>` 和 `Receiver<GlobalConfigItem>`。字段私有，调用者只能通过公开方法操作队列和客户端。
- `GlobalConfigSyncer::new`：创建容量 8 的收发端；允许 `None` 客户端，以保留 Go 中 nil PD client 时写入成功的兼容行为。
- `GlobalConfigSyncer::store_global_config`：若客户端缺失则返回 `Ok(())`；否则以空字符串 prefix、单元素切片调用客户端，成功后记录名称和值并返回 `Ok(())`。
- `GlobalConfigSyncer::notify`：同步、阻塞式发送；通道满时等待容量释放。发送错误被 `expect` 转为 panic，但当前结构体同时持有接收端，正常持有同步器时接收端不会先断开。
- `GlobalConfigSyncer::recv_notification`：同步、阻塞式接收并保留 FIFO 顺序，通道断开时返回 `crossbeam_channel::RecvError`。
- `GlobalConfigSyncer::notify_capacity`：通过接收端报告有界容量；对当前 `bounded(8)` 返回 8，`unwrap_or(0)` 也为理论上的无界通道保留了 0 回退值。

## 执行流程

1. 上层准备实现 `GlobalConfigClient` 的 `Arc`，或在无需实际写 PD 的环境传入 `None`，再调用 `GlobalConfigSyncer::new`。构造函数同时创建容量为 8 的发送端和接收端。
2. 配置变化被转换成 `GlobalConfigItem`。普通 PUT 可用 `GlobalConfigItem::new`；DELETE、非空 payload 或非默认事件类型需要显式填字段。
3. 生产者调用 `notify`。队列未满时条目按发送顺序进入通道；队列已满时调用线程阻塞，不丢弃、不合并，也没有超时分支。
4. 独立消费者调用 `recv_notification`，取得下一条 FIFO 条目。此动作只出队，不自动持久化。
5. 消费者把条目传给 `store_global_config`。有客户端时，同步器调用 `client.store_global_config("", std::slice::from_ref(&item))`，因此每次写入使用空 prefix 且恰好包含一项；成功才输出 info 日志。无客户端时直接成功，客户端报错时立即向上传播。

`pkg/domain/globalconfigsync/globalconfig_test.rs::test_global_config_syncer` 实际演示了 `notify -> recv_notification -> store_global_config -> load` 的完整 crate 内链路；`test_store_global_config` 验证两条通知按序写入。这里的“完整”仅指同步器 API 链，不代表 Rust 应用已经自动运行消费者。

## 数据与状态

`GlobalConfigItem` 是队列与客户端边界之间原样传递的所有权值。通道发送会移动条目，接收后再由 `store_global_config` 持有该值；写客户端时借用单元素切片，调用结束后值被释放。`global_config_item_metadata_survives_notify_and_store` 验证 `Delete` 判别值和任意二进制 payload 经入队、出队及客户端调用后不被改写。

同步器的持久状态只有可选 `Arc` 客户端和通道两端。它不缓存“最后一次成功值”、revision、重试次数或关闭标志，也不维护 name/value 映射。测试中的 `/global/config/{name}` 补全行为属于 `FakePdClient`，不是本文件逻辑；同步器始终把传入名称原样交给客户端，并使用空 prefix。

通道容量是固定不变量 8。`migration_aster_unit_test.rs::notify_uses_capacity_eight_and_preserves_fifo_order` 填满八项后逐项接收，验证容量和 FIFO。由于测试刻意不发送第九项，它不验证满队列阻塞的解除、跨线程唤醒或公平性。

## 依赖与调用关系

上游 API 使用者理论上包括任何能持有 `GlobalConfigSyncer` 的 Domain/变量层组件；当前 Rust 索引和文本检索只确认以下直接调用者：

- `pkg/domain/globalconfigsync/globalconfig_test.rs`：用 `FakePdClient` 覆盖基础链路、元数据透传和两条变量写入。
- `pkg/domain/globalconfigsync/migration_aster_unit_test.rs`：用 `RecordingClient` 覆盖容量、FIFO、无客户端空操作、空 prefix/单项转发及错误传播。

当前未发现 Rust 生产调用者。Go 对照链路则是：`pkg/domain/domain.go::NotifyGlobalConfigChange` 调用 `Notify`，`globalConfigSyncerKeeper` 从 `NotifyCh` 接收并调用 `StoreGlobalConfig`，`Init` 附近用 PD client 创建同步器，Domain 的工作组启动 keeper，`do.exit` 负责终止循环。这条 Go 链路是迁移参照，不应当被当成 Rust 已有行为。

下游关系如下：

- `crossbeam_channel::bounded/send/recv` 提供有界队列、阻塞和断开语义。
- `Arc<dyn GlobalConfigClient>` 提供共享客户端所有权；trait 的具体实现决定 PD 调用、网络和错误来源。
- `std::slice::from_ref` 在不分配 `Vec` 的情况下形成单元素借用切片。
- `log::info!` 仅在客户端写入成功后记录 `name` 与 `value`。
- `thiserror::Error` 为 `GlobalConfigError` 提供标准错误展示/实现。

## 错误处理与边界

- `client == None` 是明确的兼容空操作，返回 `Ok(())`；它无法区分“刻意禁用写入”和“客户端未接好”，上层若要求强一致必须自行禁止这种构造方式。
- 客户端错误通过 `?` 原样返回为 `GlobalConfigError`，没有重试、退避、批处理或死信队列；`store_propagates_client_errors_without_masking_them` 验证错误内容未被吞掉或替换。
- `notify` 没有返回值。满队列会无限期阻塞；发送端断开路径使用 `expect` panic，而不是可恢复错误。当前 `GlobalConfigSyncer` 自己同时拥有接收端，使“接收端已释放但仍能通过该实例发送”在安全 API 下不易发生，但未来若拆分通道所有权需重新设计。
- `recv_notification` 在空队列上阻塞，并把断开表示为 `RecvError`；没有非阻塞、超时或 shutdown-aware 接口。
- 写入一项时 prefix 固定为空字符串，不能由调用者覆盖；这是与 Go 调用 `StoreGlobalConfig(ctx, "", []item)` 对齐的契约。
- 日志直接包含配置名称和值。新增敏感配置时必须评估脱敏，否则成功写入会把 value 暴露到 info 日志。
- 本文件不验证空名称、重复名称、事件类型与 payload 的组合，也不执行键名映射；合法性由上层和客户端协议保证。

## 并发与资源生命周期

`GlobalConfigClient: Send + Sync` 与 `Arc` 允许多个线程共享客户端；`crossbeam-channel` 的 sender/receiver 也支持并发使用。因此 `&self` 方法可以被共享同步器的多个线程调用，但本文件不承诺多个生产者之间超出通道定义的公平性，也不串行化多个消费者对 PD 的写入。

队列提供背压而非丢弃策略：最多缓存 8 项，第 9 个未被消费的同步发送会阻塞。接收同样是阻塞式，必须由上层安排专用线程/任务或确保调用点允许等待。同步器没有 `close`/`shutdown` 方法、取消令牌或后台任务句柄；通道在整个结构体析构时随两个端点一起释放，客户端 `Arc` 引用也在析构时递减。

Go Domain keeper 用 `select` 同时监听通知和 `do.exit`，因此具备显式退出路径；Rust 本文件没有对应循环，`recv_notification` 也无法同时监听 shutdown。未来接入异步运行时时，不能直接在异步执行器工作线程上调用这些阻塞方法，除非放入阻塞线程池或更换通道抽象。

## 与 Go 版本的对应关系

逐项对应关系：

- Go `GlobalConfigSyncer.pd pd.Client` 对应 Rust `Option<Arc<dyn GlobalConfigClient>>`；Rust 用窄 trait 隔离庞大的 PD client API，并显式编码 nil/None。
- Go 公有 `NotifyCh chan pd.GlobalConfigItem` 对应 Rust 私有 `notify_tx`/`notify_rx` 加 `notify`、`recv_notification` 方法；容量同为 8，但 Rust 不允许调用者直接 select 或关闭通道。
- Go `pd.GlobalConfigItem` 对应 Rust 本地 `GlobalConfigItem`；Rust 明确复制 `EventType/Name/Value/Payload` 四类数据，并以 `#[repr(i32)]` 固定 PUT/DELETE 数值。
- Go `StoreGlobalConfig(ctx, item)` 与 Rust `store_global_config(item)` 都在客户端缺失时成功，并以空 prefix、单元素集合写入，成功后记录 name/value。Rust API 没有 `context.Context` 等价参数，取消、deadline 与 trace 上下文目前无法由调用者逐次传入。
- Go `Notify` 与 Rust `notify` 都是满队列阻塞发送。Rust 额外公开 `recv_notification` 和 `notify_capacity`，用于封装私有 receiver 和验证容量。

最大的迁移差异不在本文件内部，而在应用接线：Go `Domain` 已构造同步器、由 `NotifyGlobalConfigChange` 生产、由后台 keeper 消费，并在退出信号到来时终止；Rust 生产代码目前没有这些调用边。Rust 单测模拟的是预期的数据通路，不能替代应用集成证据。Go 测试 `TestStoreGlobalConfig` 还通过 `SET GLOBAL` 和轮询 PD 覆盖端到端变量同步，Rust 的 `test_store_global_config` 只覆盖手工构造通知后的同步器侧行为。

## 扩展指南

- 新增事件字段或 PD 事件类型时，先保持 `GlobalConfigItem`/`GlobalConfigEventType` 与 Go `pd.GlobalConfigItem`、protobuf 判别值兼容，再更新 `global_config_item_metadata_survives_notify_and_store`；不要只修改构造函数而遗漏显式结构体构造路径。
- 调整容量、发送策略或关闭语义时，修改 `NOTIFY_CHANNEL_CAPACITY`、`notify`/`recv_notification`，并在 `migration_aster_unit_test.rs` 增加独立的跨线程背压、解除阻塞和关闭测试。测试逻辑继续放在独立测试文件，不嵌入本源文件。
- 增加批量写、重试或超时时，应在 `GlobalConfigClient` 与 `store_global_config` 的契约中明确幂等性、部分成功和错误分类；同步更新 `RecordingClient` 测试。当前每项一次调用和原样错误传播是可观察行为，不能无意改变。
- 接入 Rust Domain 主链时，应在 Domain 的构造/启动/关闭位置分别完成客户端适配、消费者生命周期和退出控制，并在变量变更入口调用 `notify`。需要特别处理阻塞 channel 与异步运行时的边界；完成前不要宣称 Rust 已支持 SQL `SET GLOBAL -> PD` 端到端同步。
- 若引入真实 `tikv-client`/PD client 适配器，应把外部依赖移植放在其独立上游仓库并通过统一 tag 引用，遵守仓库对外部 Rust 依赖的可复现要求；本 crate 不应复制依赖实现到本地 vendor 或使用本地 `[patch]`。
- 修改日志时评估敏感值和高频写入的性能风险；修改 `None` 行为时评估无 PD 环境、测试环境及 Go 兼容性。

## 验证依据

- RustCodeGraph `status`：索引包含本目录的 `globalconfig.rs`、`globalconfig_test.rs`、`migration_aster_unit_test.rs`、`lib.rs` 及 Go 对照文件；目标文件识别出 13 个符号。
- RustCodeGraph `node --file pkg/domain/globalconfigsync/globalconfig.rs`：核对常量、枚举、数据结构、错误、trait、同步器字段及全部方法实现。
- RustCodeGraph 精确查询 `GlobalConfigSyncer`、`recv_notification`、`store_global_config`：确认符号位置；`callers/callees` 对本文件方法未返回调用边，因此又用生产 Rust 全仓文本检索补证，结果仅发现本目录独立测试调用同步器 API，没有生产调用者。
- `pkg/domain/globalconfigsync/Cargo.toml`、`lib.rs`、根 `Cargo.toml` 与 `pkg/domain/Cargo.toml`：核对 crate 入口、直接依赖、门面别名，以及 Domain crate 中仅位于 `cfg(windows)` 区的依赖声明。
- `pkg/domain/globalconfigsync/globalconfig_test.rs`：核对基本 notify/recv/store 链、事件/payload 原样透传和两项 FIFO 写入。
- `pkg/domain/globalconfigsync/migration_aster_unit_test.rs`：核对容量 8、FIFO、无客户端空操作、空 prefix 单项转发和客户端错误原样传播。
- `pkg/domain/globalconfigsync/globalconfig.go`、`globalconfig_test.go` 与 `pkg/domain/domain.go`：核对 Go API、端到端测试、Domain 初始化、通知、后台 keeper 和退出路径。
- 本任务是纯文档分析，依计划不运行 Cargo。结构验证要求本文恰好包含上述 11 个固定二级标题；应用主链接线状态由静态索引与文本检索验证，未通过运行中的 AsterSQL 实例验证。

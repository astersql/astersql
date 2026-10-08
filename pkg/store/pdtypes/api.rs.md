# `pkg/store/pdtypes/api.rs`

## 文件定位

目标源码是 [`api.rs`](./api.rs)。本文件属于 `astersql-store-pdtypes` crate（`pkg/store/pdtypes/Cargo.toml`），由 crate 根 `pkg/store/pdtypes/lib.rs` 以 `pub mod api` 暴露，并经工作区依赖别名 `facade_store_pdtypes`、`pkg/lib.rs` 的 `store::pdtypes` 门面再次导出。它不是 PD 客户端，也不发起网络请求；它是 TiDB/AsterSQL 消费 PD HTTP API 时使用的 Store、Region JSON 数据传输对象（DTO）及其兼容序列化层。

RustCodeGraph 将本文件识别为 46 个符号、758 行，并只找到 `pkg/store/pdtypes/migration_aster_unit_test.rs` 与 `pkg/store/pdtypes/statistics_test.rs` 两个文件级使用者；进一步用 `rg` 搜索 `astersql_store_pdtypes`、`pdtypes::api` 和 `crate::api` 时，仓库内只有迁移测试直接导入这些 API 类型。因而当前仓库内可确认的接线是“公开门面 + 契约测试”，没有证据表明某条 Rust 生产执行链已经实例化这些 DTO；对工作区外部使用者则未验证。

## 核心职责

文件承担三类职责。

1. 用 `StoresInfo`/`StoreInfo`/`MetaStore`/`StoreStatus` 表达 PD 返回的 Store 清单、protobuf 元数据和运行负载。
2. 用 `RegionsInfo`/`RegionInfo`/`MetaPeer`/`PDPeerStats`/`ReplicationStatus` 表达 Region 键范围、副本、Leader、流量、近似规模和复制状态。
3. 把 Rust/Protobuf 的表示转换成 Go `encoding/json` 兼容形状：空列表按 Go `nil` slice 编码为 `null`，零值字段按 Go 的 `omitempty` 规则省略，容量、时长和 UTC 时间沿用 Go 格式，并把嵌入的 protobuf Store/Peer/PeerStats 展平成 JSON 字段。

这里的“兼容”是 API JSON 兼容，而不是 protobuf 的无损通用转码。`StoreJson`、`PeerJson`、`RegionEpochJson` 和 `PDPeerStatsJson` 只保存本文件显式列出的字段；将来 kvproto 增加字段时，必须同步扩展适配器，否则这些新字段不会进入 JSON 往返结果。

## 主要符号

- `StoresInfo { Count: i32, Stores: Vec<StoreInfo> }`：Store 集合顶层响应。`Stores` 使用 `nil_if_empty_vec`，空向量序列化为 JSON `null`，输入 `null` 或缺失字段则反序列化为空向量。
- `StoreInfo { Store: Option<Box<MetaStore>>, Status: Option<Box<StoreStatus>> }`：把静态 Store 元数据与动态状态组合；两个成员允许 JSON `null`。
- `MetaStore { Store: Option<Box<metapb::Store>>, StateName: String }`：对 Go 匿名嵌入 `*metapb.Store` 的 Rust 表达。它手工实现 `Serialize`/`Deserialize`，借助展平的 `MetaStoreJson` 保持 `id`、`address` 等字段与 `state_name` 同级。
- `StoreStatus`：容量、可用量、已用量，Leader/Region 计数、权重、得分和大小，慢节点评分、快照计数、忙碌标记、启动/心跳时间与 uptime。`Capacity`、`Available`、`UsedSize` 通过 `byte_size_json` 编码；可选时间和时长分别通过 `datetime_option_json`、`duration_option_json` 编码。
- `RegionsInfo { Count: i32, Regions: Vec<RegionInfo> }`：Region 集合顶层响应，空列表与 `StoresInfo` 采用相同的 `null` 兼容规则。
- `RegionInfo`：Region ID、字符串形式的 `[StartKey, EndKey)` 边界、epoch、副本/Leader/down/pending 状态、读写字节与键计数、近似大小/键数、复制状态。此文件只承载这些值，不校验键范围顺序、Leader 是否属于 Peers 或计数是否与列表一致。
- `MetaPeer { Peer, RoleName, IsLearner }`：展平 `metapb::Peer`，并显式保留可读角色名和旧 API 兼容用 learner 标记。
- `PDPeerStats { PeerStats, Peer }`：展平 down peer 统计并增加 `peer` 对象；当前 JSON 适配器仅搬运 `pdpb::PeerStats.down_seconds`。
- `ReplicationStatus { State, StateID }`：复制模式状态名和状态 ID，直接派生 serde 实现。
- 内部适配符号：`nil_if_empty_vec`、`byte_size_json`、`duration_option_json`、`datetime_option_json`、`region_epoch_option_json`，以及 `RegionEpochJson`、`StoreLabelJson`、`StoreJson`、`MetaStoreJson`、`PeerJson`、`MetaPeerJson`、`PDPeerStatsJson`。`is_zero_u32` 和 `is_false` 是 serde 的零值过滤谓词。

## 执行流程

本文件没有主动执行入口；流程由 serde 在上游调用 `serde_json::{to_*,from_*}` 时驱动。

Store 序列化路径如下：`StoresInfo` 派生实现遍历 `StoreInfo`；`MetaStore::serialize` 将可选 `metapb::Store` 交给 `StoreJson::from`，只把非零/非空 protobuf 字段变成 `Some`，再由 `MetaStoreJson` 的 `flatten` 与 `StateName` 一起输出。`StoreStatus` 的三个容量值先调用 `ByteSize_MarshalJSON` 得到 Go 风格 JSON 字符串；可选时长类似地调用 `Duration_MarshalJSON`；UTC 时间由 `format_go_time` 生成 RFC 3339，删除无意义的九位零纳秒或末尾零。

Store 反序列化路径相反：`MetaStore::deserialize` 先读入展平的 `MetaStoreJson`，再由 `StoreJson::into_store` 恢复 protobuf。完全没有 Store 字段时返回 `None`；有任意字段时补齐其余 protobuf 零值，并严格把数值枚举映射为 `StoreState::{Up,Offline,Tombstone}` 和 `NodeState::{Preparing,Serving,Removing,Removed}`。容量和时长通过对应的 `*_UnmarshalJSON` 解析。

Region 序列化时，`RegionEpochJson::from` 省略零值 `conf_ver/version`；`MetaPeer::serialize` 经 `PeerJson` 展平 Peer 字段，并添加 `role_name`、按需添加 `is_learner`；`PDPeerStats::serialize` 只提取非零 `down_seconds`，再嵌入 `MetaPeer`。反序列化时 `PeerJson::into_peer` 检查角色数值，构造 `metapb::Peer`；`PDPeerStats` 仅在存在 `down_seconds` 时构造 protobuf `PeerStats`。

## 数据与状态

全部公开结构都是普通拥有型数据，没有全局变量、缓存或隐式状态。`Clone` 允许独立复制，`Default` 配合结构级 `#[serde(default)]` 使缺失 JSON 字段落到 Rust/Go 语义上的零值，`PartialEq` 支持测试和调用方比较；`ReplicationStatus` 额外实现 `Eq`。

protobuf 对象使用 `Option<Box<T>>` 表示 Go 指针可空性。列表使用 `Vec<T>`，其中只有两个顶层列表通过 `nil_if_empty_vec` 把空向量编码成 `null`；`RegionInfo` 内的 `Peers`、`DownPeers`、`PendingPeers` 则为空时直接省略。数值类型按字段用途固定为 `i32`、`i64`、`u32` 或 `u64`，本文件不做溢出转换或业务范围校验。

重要不变量来自转换器而非结构本身：Store state 仅接受 0..=2，node state 仅接受 0..=3，Peer role 仅接受 0..=3；空的 `StoreJson`/`PeerJson` 表示空 protobuf 指针，而“任意字段存在但值为零”会构造一个其余字段取默认值的 protobuf 对象。

## 依赖与调用关系

直接依赖由 `pkg/store/pdtypes/Cargo.toml` 确认：`serde`/`serde_json` 提供 JSON 映射，`chrono` 提供 `DateTime<Utc>` 与 RFC 3339 格式，带 `protobuf-codec` feature 的 `kvproto` 提供 `metapb`/`pdpb` 类型，`astersql-config-configtypes` 提供 `ByteSize`、`Duration` 及 Go 兼容的 marshal/unmarshal 函数。该 crate 没有为本文件声明 feature 开关或条件编译分支。

内部下游调用边可由源码和 RustCodeGraph 节点核对：`MetaStore::{serialize,deserialize}` 调用 `StoreJson::{from,into_store}`；`MetaPeer::{serialize,deserialize}` 调用 `PeerJson::{from,into_peer}`；`StoreStatus` 的派生 serde 实现通过字段属性进入容量、时间和时长适配模块；Region epoch 和 down peer 统计走各自的中间 JSON 结构。RustCodeGraph 对这些 DTO 的 `callers`/`callees` 查询没有返回生产函数调用边，这与它们由 serde 派生/trait 回调驱动、且仓库内尚无生产实例化点的搜索结果一致。

上游可见性链是 `pkg/store/pdtypes/lib.rs::api` → 工作区 `facade_store_pdtypes` → `pkg/lib.rs::store::pdtypes`。直接测试调用者是 `migration_api_types_keep_go_json_contract` 和 `migration_nested_api_types_keep_go_json_contract`（`pkg/store/pdtypes/migration_aster_unit_test.rs`）；`statistics_test.rs` 被图索引为文件级使用者，但其源码只测试 `statistics::RegionStats`，不直接覆盖本文件行为。

## 错误处理与边界

公开类型没有自定义错误类型，错误通过 serde 的 `S::Error`/`D::Error` 返回。`ByteSize_MarshalJSON`、`ByteSize_UnmarshalJSON`、`Duration_MarshalJSON`、`Duration_UnmarshalJSON` 以及中间 `serde_json` 转换产生的错误都经 `custom` 保留为序列化/反序列化失败；RFC 3339 解析错误同样传递给调用方。

`StoreJson::into_store` 对未知 `metapb::StoreState` 或 `metapb::NodeState` 数值返回包含非法值的错误字符串；`PeerJson::into_peer` 对未知 `metapb::PeerRole` 同样拒绝输入。相比之下，本文件不验证 `StateName` 与数值 state、`RoleName`/`IsLearner` 与 Peer role 是否相符，也不验证 Region 的结构一致性，因此调用者不能把成功反序列化等同于 PD 元数据已经通过业务校验。

兼容边界还包括：完全空的展平 Store/Peer 被还原为 `None`；未知 JSON 字段按 serde 默认行为被忽略；protobuf 中未被适配器列出的字段无法经本文件的 JSON 往返保留；`PDPeerStats` 除 `down_seconds` 外的未来字段亦会丢失。时间只接受 chrono 能解析的 RFC 3339 文本，容量/时长只接受共享 configtypes 解析器认可的格式。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、文件句柄、网络连接或事务。对象在反序列化时一次性分配并拥有其字符串、向量和 boxed protobuf；离开作用域后由 Rust 自动释放。序列化只借用输入，除构造中间 JSON 结构和字符串外不保留引用。

这些 DTO 本身未显式实现并发策略；能否跨线程传递取决于各字段类型的自动 `Send`/`Sync` 实现，本文件没有用 `unsafe impl` 扩大保证。若调用方需要共享可变状态，应在上层选择锁或消息传递，本文件不应承担该生命周期管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/pdtypes/api.go`。九个公开 Rust 结构分别对应同名 Go 类型，JSON tag 和字段含义保持一致；Go 文件说明该包主要从 PD 仓库复制，以避免直接依赖 PD，Rust crate 继续采用本地 DTO + `kvproto` 的边界。

关键表示差异是：Go 的匿名嵌入 `*metapb.Store`、`*metapb.Peer`、`*pdpb.PeerStats` 由 Rust 的可选 boxed protobuf 加手工 `flatten` 适配器实现；Go `nil` slice 与 Rust 空 `Vec` 由 `nil_if_empty_vec` 对齐；Go `*time.Time`/`*Duration` 由 Rust `Option` 对齐；Go `omitempty` 由 `skip_serializing_if` 对齐。`MetaPeer.RoleName` 与 `IsLearner` 保留 Go 注释中的兼容目的：让角色可读，并兼容 kvproto 5.0 从 learner 标志迁移到 role 枚举后的 API。

存在两项应明确的迁移边界。第一，Rust 的 `Count` 仍为 `i32`，而 Go `int` 在支持的 64 位平台范围更大；当前 API 测试没有覆盖超出 `i32` 的 count。第二，Go protobuf 的 JSON 展平行为随生成类型而定，Rust 版本是字段白名单式手工映射；新增 protobuf 字段不会自动同步。现有 `migration_aster_unit_test.rs` 已验证零值列表、容量/时长/时间、Store state、epoch、Peer role、down seconds 和复制状态的代表性契约，但不是全字段穷举。

## 扩展指南

新增普通 API 字段时，先在 `pkg/store/pdtypes/api.go` 或上游 PD API 确认 JSON 名称、可空性、`omitempty` 和零值语义，再修改对应公开结构，并在独立测试文件 `pkg/store/pdtypes/migration_aster_unit_test.rs` 增加序列化与反序列化断言；不要把测试内嵌进 `api.rs`。

新增或升级 protobuf 字段时，必须同时检查 `StoreJson`/`PeerJson`/`RegionEpochJson`/`StoreLabelJson`/`PDPeerStatsJson` 的字段定义、`From` 转换、`into_store`/`into_peer` 和自定义 serde 实现。枚举扩展必须显式增加合法值映射并测试未知值仍可产生清晰错误。新增容量、时长或时间字段应复用已有适配模块，避免产生与 Go 不一致的单位或格式。

若要把这些 DTO 接入生产 PD HTTP 客户端，应从 `pkg/lib.rs::store::pdtypes` 的公开门面导入，并增加调用方独立测试，覆盖真实响应、`null`/缺失字段、非法枚举、未知字段以及大数边界。兼容风险主要是 JSON 形状或零值改变；性能风险主要来自大 Store/Region 列表的完整拥有型分配和 clone。改动后还应核对 `pkg/store/pdtypes/Cargo.toml` 的依赖是否仍最小，并保持所有 Rust 测试逻辑在独立测试文件中。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件（其中 Rust 7,032 个）；`files --filter pkg/store/pdtypes` 定位本 crate 的 Rust/Go 对照与测试；两次 `node --file pkg/store/pdtypes/api.rs` 覆盖 1--758 行；对九个公开结构执行 `query`/`callers`，并对转换入口执行 `callees`，确认符号定义、测试实例化关系及未发现生产调用边。
- 源码与装配：`pkg/store/pdtypes/api.rs`、`pkg/store/pdtypes/lib.rs`、根 `Cargo.toml` 的 `facade_store_pdtypes` 声明、`pkg/lib.rs` 的门面再导出。
- crate 边界：`pkg/store/pdtypes/Cargo.toml`，确认 `serde`、`serde_json`、`chrono`、`kvproto`、`astersql-config-configtypes` 依赖及 `protobuf-codec` feature。
- Go 对照：`pkg/store/pdtypes/api.go`，逐项核对同名结构、JSON tag、匿名 protobuf 嵌入与兼容注释；同目录没有 `api_test.go`。
- Rust 测试：`pkg/store/pdtypes/migration_aster_unit_test.rs` 的 `migration_api_types_keep_go_json_contract`、`migration_nested_api_types_keep_go_json_contract`；另读 `pkg/store/pdtypes/statistics_test.rs`，确认它不直接覆盖本文件。
- 文本交叉检查：`rg` 搜索 `astersql_store_pdtypes|pdtypes::api|store_pdtypes`、`api::{...}` 及九个公开类型，确认当前工作区直接导入点和测试范围。按任务约束未运行 Cargo，也未修改 Rust、Go、Cargo 或总计划。

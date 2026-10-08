# `pkg/store/pdtypes/statistics.rs` 逻辑说明

## 文件定位

`statistics.rs` 属于 `astersql-store-pdtypes` crate；crate 根 `pkg/store/pdtypes/lib.rs` 以公开模块 `pub mod statistics` 暴露它。该文件不是统计计算器，也不访问 PD：它定义 PD Region 统计响应在 Rust 侧的传输对象 `RegionStats`，并定义六个按 Store 聚合字段的 JSON 空值兼容规则。

`pkg/store/pdtypes/Cargo.toml` 指定库入口为 `lib.rs`，且以 `[package.metadata.porting].go-package = "pkg/store/pdtypes"` 标明 Go 对照包。此文件直接使用该 crate 的 `serde` 依赖；`serde_json` 只由相关测试和潜在调用方用于具体 JSON 编解码。

当前 RustCodeGraph 文件关系显示，生产 Rust 文件尚未引用这里的 `RegionStats`，直接使用者是 `pkg/store/pdtypes/statistics_test.rs`，另有 crate 内迁移测试 `pkg/store/pdtypes/migration_aster_unit_test.rs`。因此它目前是已经公开、具备兼容性测试但尚未进入 Rust 生产请求链的 DTO 边界。Go 版本的生产链已接通，见“与 Go 版本的对应关系”。

## 核心职责

1. `RegionStats` 保存 PD 对某个 Region 集合返回的总量和分布统计：Region 总数、空 Region 数、存储大小、key 数量，以及各 Store 的 Leader/Peer 数量、大小和 key 数量。
2. `#[derive(Serialize, Deserialize)]` 与字段上的 `#[serde(rename = ...)]` 固定 PD JSON 字段名，避免 Rust 标识符命名方式影响线协议。
3. `#[serde(default)]` 让缺失字段采用 `Default` 零值，保持 Go `encoding/json` 解码到零值 struct 的行为。
4. 私有模块 `nil_if_empty_map` 把 Rust 空 `HashMap` 编码为 JSON `null`，并把 JSON `null` 解码为空 map，以对齐 Go 未初始化 `nil map` 的 JSON 表现。

文件不负责按表生成 key range、发起 HTTP 请求、聚合 Peer 总数或生成 information schema 行；这些职责在 Go 当前实现中分别位于 `pkg/store/helper/helper.go::GetPDRegionStats` 和 `pkg/executor/infoschema_reader.go::tableStorageStatsRetriever`。

## 主要符号

- `pub struct RegionStats`：唯一公开业务类型。派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`、`Serialize` 和 `Deserialize`，可复制、比较、构造零值并进行 JSON 等 Serde 格式转换。
- `RegionStats::Count: i64`：JSON 名 `count`，Region 总数。Rust 明确采用 `i64`，以覆盖受支持服务器平台上 Go `int` 的 64 位范围。
- `RegionStats::EmptyCount: i64`：JSON 名 `empty_count`，空 Region 数。
- `RegionStats::StorageSize: i64` 与 `StorageKeys: i64`：JSON 名分别为 `storage_size`、`storage_keys`，保存集合级数据大小与 key 数量。
- `StoreLeaderCount`、`StorePeerCount: HashMap<u64, i64>`：Store ID 到 Leader/Peer Region 数量的映射。
- `StoreLeaderSize`、`StoreLeaderKeys`、`StorePeerSize`、`StorePeerKeys: HashMap<u64, i64>`：Store ID 到相应大小或 key 数量的映射。
- `nil_if_empty_map::serialize`：泛型 Serde helper。空 map 调用 `Serializer::serialize_none`，非空 map 调用 `serialize_some(value)`；应用于全部六个 map 字段。
- `nil_if_empty_map::deserialize`：先解码 `Option<HashMap<K, V>>`，再以 `unwrap_or_default` 将 JSON `null` 变为空 map。类型约束要求键可反序列化、判等和哈希。

该文件没有模块级常量、enum、trait、手写 `impl` 或条件编译项。`nil_if_empty_map` 及其两个函数仅供本文件字段属性调用，不构成 crate 的公开 API。

## 执行流程

反序列化流程如下：

1. 调用方以 Serde 将 PD JSON 解码为 `RegionStats`。
2. `#[serde(default)]` 先保证 JSON 中缺失的字段能取 `RegionStats::default()` 对应值：数值为 `0`、map 为空。
3. 标量字段按各自 `rename` 后的 JSON 名解码为 `i64`。
4. 六个 map 字段经 `nil_if_empty_map::deserialize` 解码：JSON object 中的字符串属性名由 Serde 转成 `u64` Store ID；JSON `null` 转为空 `HashMap`。
5. 非法类型、无法转换为 `u64` 的 map 键或超出 `i64`/`u64` 范围的数值由 Serde 返回错误；本文件不捕获或改写错误。

序列化流程与之相反：标量按固定 JSON 名输出；非空 map 的 `u64` 键在 JSON object 中表现为十进制字符串；空 map 被 helper 输出为 `null`。`pkg/store/pdtypes/migration_aster_unit_test.rs::migration_region_statistics_use_stringified_store_ids` 验证了空 map 和键 `42` 的这两条行为。

当前 Rust 生产代码没有发起上述流程的调用边。Go 主链可作为迁移时的行为参照：`GetPDRegionStats` 生成表 key range并调用 PD HTTP client，返回 `pd.RegionStats`；`tableStorageStatsRetriever::setDataForTableStorageStats` 对表和分区逐个获取统计、汇总 `StorePeerCount`，再把 `Count`、`EmptyCount`、`StorageSize`、`StorageKeys` 写入 `TABLE_STORAGE_STATS` 行。

## 数据与状态

`RegionStats` 是纯值对象，不持有客户端、上下文、句柄或缓存。四个集合级指标与六张 Store 映射共同构成一次统计快照；类型本身不维护字段之间的一致性，也不验证这些总量是否等于 map 中的聚合值。

所有计数和大小值均为有符号 `i64`。这与 Go 对照中的 `int`/`int64` 在受支持的 64 位平台上的可表示正数范围一致，也允许忠实解码线协议中的负数；本文件没有业务层非负校验。Store ID 使用 `u64`，所以负数键不可表示。

`Default` 的语义是所有数值为 `0`、所有 map 为空。内存中的空 map 不保留输入究竟是“字段缺失”“显式 `null`”还是“空 object”这一差异；再次序列化时三者都会得到 `null`。这是一项刻意的 Go nil-map 兼容取舍，而不是无损 JSON 往返。

## 依赖与调用关系

直接依赖只有：

- `serde::{Serialize, Deserialize}`：派生结构编解码能力，并为私有 helper 提供 serializer/deserializer trait。
- `std::collections::HashMap`：保存六类 Store 维度聚合；helper 另依赖 `std::hash::{BuildHasher, Hash}` 以支持泛型 map。
- `pkg/store/pdtypes/lib.rs`：以 `pub mod statistics` 建立模块入口，并在测试配置下挂接 `statistics_test.rs` 与 `migration_aster_unit_test.rs`。
- `pkg/store/pdtypes/Cargo.toml`：声明 `serde`（含 `derive` feature）和 `serde_json`；本文件不使用该 crate 的 `kvproto`、`tikv-client`、`chrono`、`anyhow` 或配置类型依赖。

RustCodeGraph 的文件级结果为 `statistics.rs` 标出一个直接使用文件 `statistics_test.rs`，并为 Go 对照 `statistics.go` 标出生产使用文件 `pkg/store/helper/helper.go` 和 `pkg/domain/infosync/tiflash_manager.go`。仓库精确引用搜索还确认 `migration_aster_unit_test.rs` 构造和序列化该 Rust 类型。未发现 Rust 生产调用者，因此不能声称当前 Rust 应用已经通过本类型展示 PD 统计。

Go 侧下游链的直接证据是：`pkg/store/helper/helper.go::GetPDRegionStats` 返回 `*pd.RegionStats`；`pkg/executor/infoschema_reader.go::tableStorageStatsRetriever` 保存该值并读取 `StorePeerCount`、`Count`、`EmptyCount`、`StorageSize`、`StorageKeys`；`pkg/domain/infosync/tiflash_manager.go::MockTiFlash::HandleGetPDRegionRecordStats` 返回只设置 `Count` 的测试/模拟值。

## 错误处理与边界

本文件没有自定义错误类型。`serialize` 和 `deserialize` 原样返回 Serde 的 `S::Error`/`D::Error`，不记录日志、不降级，也不添加上下文。字段类型不匹配、非法 Store ID 字符串以及整数越界会在反序列化层失败。

已验证的兼容边界包括：

- 缺失全部字段的 `{}` 解码为 `RegionStats::default()`（`migration_json_missing_fields_use_go_zero_values`）。
- `count`、`empty_count`、`store_leader_count`、`store_peer_count` 能保存超过 `i32::MAX` 的值（`region_statistics_preserve_go_int_range`）。
- 空 map 序列化为 `null`，非空 `u64` Store ID 键序列化为字符串（`migration_region_statistics_use_stringified_store_ids`）。

尚无独立测试直接覆盖：显式 JSON `null` 反序列化为空 map、空 object 再序列化为 `null`、非法/越界 Store ID 键、负数统计值以及全部十个字段的完整 payload。当前实现可以从 helper 代码推导这些行为，但新增约束前应补测试，而不能把推导当成既有业务保证。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、channel、事务、网络连接或文件资源。`RegionStats` 拥有其六个 `HashMap`；值被移动或克隆时遵循普通 Rust 所有权规则，离开作用域后自动释放。

类型没有内部可变性，也没有跨快照合并逻辑。并发安全性只取决于调用方如何共享实例以及字段类型的标准 trait；本文件没有同步协议。Serde 编解码期间唯一的临时状态是反序列化得到的 `Option<HashMap<...>>`，函数返回后即被转换或释放。

Go 生产链中 PD HTTP 请求的取消、客户端连接和 retriever 分批生命周期属于 `GetPDRegionStats` 及其调用方，不属于 Rust DTO。迁移这些流程时应在对应客户端/执行器模块说明资源所有权，不能把生命周期责任放进 `RegionStats`。

## 与 Go 版本的对应关系

直接对照为 `pkg/store/pdtypes/statistics.go::RegionStats`。十个字段及 JSON 名逐一一致：`Count`、`EmptyCount`、`StorageSize`、`StorageKeys` 和六个 `Store*` map 均保留 Go 导出字段命名，以便迁移时核对。

类型映射如下：Go `int` 与 `int64` 在 Rust 中统一为 `i64`；Go `map[uint64]int` 和 `map[uint64]int64` 在 Rust 中统一为 `HashMap<u64, i64>`。统一后不再表达 Go 源码中 `int` 与 `int64` 的名义差异，但在项目支持的 64 位服务器平台上保留数值范围；`statistics_test.rs` 对超过 `i32` 范围的值进行了回归验证。

Go 零值 struct 的 map 是 `nil`，`encoding/json` 将其编码为 `null`；Rust `HashMap::default()` 是已分配语义上的空集合，通常会编码为 `{}`。`nil_if_empty_map` 专门消除这一线协议差异：空 map 输出 `null`，输入 `null` 变为空 map。代价是 Rust 内存模型无法区分 Go 的 nil map 与非 nil 空 map。

Go 当前已有真实生产接线：`pkg/store/helper/helper.go::GetPDRegionStats` 从 PD 获取数据，`pkg/executor/infoschema_reader.go` 将其用于表存储统计，TiFlash mock 也返回该类型。当前 Rust 版本只完成类型、序列化兼容和测试接线，未找到等价的 Rust 生产调用边；后续迁移不得把 Go HTTP/执行器逻辑误写进此 DTO 文件，也不能以 DTO 已存在推断整条功能已支持。

## 扩展指南

新增或修改 PD 统计字段时，应先核对 PD 响应与 `pkg/store/pdtypes/statistics.go::RegionStats`，再修改 `RegionStats`：保持确切 JSON 名，选择与 Go/PD 范围一致的 Rust 类型，并决定缺失字段是否仍可使用零值。新增 map 字段若需要 Go nil-map 兼容，应复用 `with = "nil_if_empty_map"`；若空 object 与 null 必须区分，则需设计新的表示，不能继续使用当前 helper。

同步测试至少包括：

- 在 `pkg/store/pdtypes/statistics_test.rs` 增加数值范围、错误输入和新字段解码边界；不要把测试内嵌回生产 `statistics.rs`。
- 在 `pkg/store/pdtypes/migration_aster_unit_test.rs` 更新 Go JSON 契约、默认值、空 map 与 Store ID 键序列化断言。
- 若新增字段进入生产展示或计算，在相应 Rust 调用模块的独立测试中验证消费语义，并参照 Go 的 `pkg/store/helper/helper_test.go` 与 executor 测试，而不是仅验证 DTO 能编译。

主要兼容风险是字段名漂移、整数宽度收窄、缺失字段不再可解码、空 map 从 `null` 变成 `{}`，以及 Store ID 键不再使用 JSON 字符串。性能风险集中在大集群的六张 map：完整 clone 和 JSON 编解码均随 Store 数量线性增长；新增功能应优先借用或移动快照，避免无意义的重复 clone。由于 DTO 不校验跨字段不变量，需要一致性校验时应放在拥有业务上下文的上层，而不是悄然改变通用解码行为。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件；目标目录文件清单包含 `statistics.rs`、Go 对照和两份相关 Rust 测试。
- RustCodeGraph `node --file pkg/store/pdtypes/statistics.rs`：核对了文件全部 90 行、`RegionStats` 十个字段、Serde 属性及 `nil_if_empty_map::{serialize, deserialize}`；文件级关系显示直接使用者为 `statistics_test.rs`。
- RustCodeGraph `query RegionStats --kind struct`：区分了本文件、Go 对照以及其他包中的同名结构，避免把 `pkg/store/helper/helper.rs` 或 `pkg/executor/internal/pdhelper/pd.rs` 的不同类型混为一谈。精确内部 ID 的后续 `node` 查询发生索引错配，因此未将该结果用于调用关系结论。
- RustCodeGraph 文件节点：读取了 `pkg/store/pdtypes/lib.rs`、`statistics_test.rs`、`migration_aster_unit_test.rs`、`statistics.go`、`pkg/store/helper/helper.go`、`pkg/executor/infoschema_reader.go` 和 `pkg/domain/infosync/tiflash_manager.go` 的相关定义与调用片段。
- Cargo 证据：`pkg/store/pdtypes/Cargo.toml` 确认 crate 名、`lib.rs` 入口、`serde` derive、`serde_json` 和 Go 包映射；无 feature 条件控制本模块。
- 测试证据：`region_statistics_preserve_go_int_range`、`migration_json_missing_fields_use_go_zero_values`、`migration_region_statistics_use_stringified_store_ids`；Go 的 `TestGetPDRegionStatsKeyspaceEncoding` 仅验证上游 key range/HTTP 请求路径，不直接增加 Rust DTO 的行为保证。
- 调用边补充搜索：精确 `rg` 引用确认 Rust 侧只有两份 crate 测试使用目标类型；Go 侧确认 `GetPDRegionStats -> pd.RegionStats -> tableStorageStatsRetriever` 的生产消费链。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的十一章节结构命令，并人工复查“为何存在、如何编解码、当前接线程度、如何安全扩展”均有源码或测试依据。

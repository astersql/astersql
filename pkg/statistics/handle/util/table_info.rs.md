# `pkg/statistics/handle/util/table_info.rs`

## 文件定位

[`table_info.rs`](table_info.rs) 属于 `astersql-statistics-handle-util` crate；该 crate 的入口 [`lib.rs`](lib.rs) 以 `pub mod table_info` 声明模块，并通过 `pub use table_info::*` 导出其公开类型、trait 和构造函数。它位于 statistics handle 与 InfoSchema 之间，负责把“物理 ID（普通表 ID 或分区 ID）”解析为统计模块需要的父表元数据或轻量表条目。

[`Cargo.toml`](Cargo.toml) 的 `[package.metadata.porting]` 将本 crate 对应到 Go 包 `pkg/statistics/handle/util`，并记录旧迁移任务 `task-877`、`task-1490`。本文件自身只使用标准库的 `HashMap`、`Arc` 和 `Mutex`；清单中真实 InfoSchema、meta、table 等依赖位于永假条件 `target.'cfg(any())'` 下，说明当前 Rust 实现通过本文件定义的轻量抽象隔离了完整 TiDB 类型，而不是直接调用这些真实 crate。

## 核心职责

本文件承担四项紧密相关的职责：

1. 用 `PartitionDefinition`、`TableMeta`、`TableItem` 描述查找所需的最小元数据。
2. 用 `InfoSchema` trait 抽象 schema 版本、V1/V2 类型、表/分区查找和分区表枚举能力。
3. 用 `TableInfoGetter` 统一普通查找与初始化统计专用查找。普通查找允许 InfoSchema 自己按分区 ID 反查；初始化统计在 InfoSchema V1 上改用缓存，避免每个分区都触发全表扫描。
4. `CachedTableInfoGetter` 维护“分区 ID → 父表 ID”映射，并在 schema 版本变化时整体重建。

该设计保留了同路径 Go 实现 [`table_info.go`](table_info.go) 的关键性能语义：V2 直接使用通用查找；V1 初始化统计时用按 schema 版本失效的映射把昂贵扫描摊销到一次重建。

## 主要符号

- `PartitionDefinition { id, name }`：最小分区描述。缓存算法只读取 `id`，`name` 用于保持元数据表达完整。
- `TableMeta { id, schema_name, table_name, partitions }`：可共享的父表元数据。API 以 `Arc<TableMeta>` 返回它，避免复制分区列表。
- `TableItem { id, schema_name, table_name }`：不携带分区列表的轻量条目，供只需要身份和名称的初始化统计路径使用。
- `InfoSchema: Send + Sync`：注入式查询边界。`schema_meta_version` 驱动缓存失效，`is_v2` 选择算法；`table_by_id`、`find_table_by_partition_id`、`table_item_by_id`、`table_item_by_partition_id` 提供查找；`partitioned_tables` 是 V1 缓存重建的数据源。
- `TableInfoGetter: Send + Sync`：对上游暴露四种查找。`table_info_by_id`/`table_item_by_id` 是通用入口；带 `for_init_stats` 后缀的两个入口为初始化统计提供 V1 优化。
- `PartitionCache`：私有缓存状态，包含 `partition_to_table: HashMap<i64, i64>` 和建图时的 `schema_version`。
- `CachedTableInfoGetter { init_stats_v1: Mutex<PartitionCache> }`：唯一内置实现。`new` 创建空缓存；`partition_id_to_table_id_for_init_stats` 完成加锁、版本检查、重建和查值。
- `build_partition_id_to_table_id`：遍历 `InfoSchema::partitioned_tables` 返回的每张表及其 `partitions`，构造分区到父表的映射。
- `new_table_info_getter`：返回 `Arc<dyn TableInfoGetter>`，隐藏具体实现并允许多线程共享。

本文件没有常量、枚举、条件编译项或错误类型；除 `PartitionCache` 和辅助查找方法外，其余上述类型/trait/函数均为公开 API。

## 执行流程

### 通用完整元数据查找

`CachedTableInfoGetter::table_info_by_id` 先调用 `InfoSchema::table_by_id(physical_id)`。命中表示输入是普通表 ID，立即返回；未命中才调用 `find_table_by_partition_id`，由 InfoSchema 解析分区并返回父表。两步都失败时返回 `None`。

### 初始化统计的完整元数据查找

`table_info_by_id_for_init_stats` 先判断 `InfoSchema::is_v2`：

- V2：委托 `table_info_by_id`，使用 InfoSchema 的高效表/分区索引。
- V1：先按表 ID 调用 `table_by_id`；未命中时调用 `partition_id_to_table_id_for_init_stats`。后者持锁读取 schema 版本，版本变化则调用 `build_partition_id_to_table_id` 重建映射，再按分区 ID 得到父表 ID；最后重新调用 `table_by_id(parent_id)` 取得父表。

### 初始化统计的轻量条目查找

`table_item_by_id_for_init_stats` 与上一个流程同构。V2 委托通用 `table_item_by_id`；V1 先尝试 `InfoSchema::table_item_by_id(physical_id)`，未命中才经同一缓存解析父表 ID，然后再次按表 ID取得 `TableItem`。它刻意不调用 `table_item_by_partition_id`，因为同路径 Go 实现说明 V1 的该操作会扫描所有表。

### 通用轻量条目查找

`table_item_by_id` 先执行 `InfoSchema::table_item_by_id`，失败后执行 `table_item_by_partition_id`。因此该入口适用于普通查询或 V2；初始化统计必须使用专用入口才能避开 V1 的逐分区扫描。

### 缓存建图

`build_partition_id_to_table_id` 遍历所有分区表，对每个 `PartitionDefinition` 写入 `mapping[partition.id] = table.id`。重复分区 ID 会由后遍历的值覆盖；代码依赖上游保证物理分区 ID 唯一，没有在本层额外校验。

## 数据与状态

持久状态只存在于 `CachedTableInfoGetter::init_stats_v1`。初始 `PartitionCache` 由 `Default` 生成：映射为空、版本为 `0`。第一次查找仅在 `info_schema.schema_meta_version() != 0` 时重建；因此接口隐含要求有效 InfoSchema 版本与初始哨兵可区分，或者版本为 `0` 时空映射就是可接受状态。Go 实现同样以零值版本初始化，语义一致。

缓存键和值都是 `i64`：键是物理分区 ID，值是逻辑父表 ID。缓存不保存 `Arc<TableMeta>` 或 `TableItem`，所以最终返回值始终从当前传入的 InfoSchema 再查一次，避免缓存直接持有可能过期的完整元数据。

`TableMeta` 通过 `Arc` 在调用者之间共享；`TableItem` 按值返回并拥有字符串。所有 DTO 派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`，便于构造 fake InfoSchema 和比较测试结果。

## 依赖与调用关系

下游调用关系均是显式 trait 调用：

- 通用查找调用 `InfoSchema::{table_by_id, find_table_by_partition_id, table_item_by_id, table_item_by_partition_id}`。
- 初始化统计先调用 `InfoSchema::{is_v2, schema_meta_version}`，V1 缓存重建还调用 `partitioned_tables`。
- `partition_id_to_table_id_for_init_stats` 调用本文件的 `build_partition_id_to_table_id`；两个 init-stats API 共享这一私有路径。

Rust 上游现状需要分两层理解：

- [`lib.rs`](lib.rs) 公开重导出本模块；[`../types/interfaces.rs`](../types/interfaces.rs) 又重导出 `TableInfoGetter`，并把它列为 `StatsHandle` 的 supertrait。因此该接口已经进入 Rust statistics handle 的类型契约。
- 仓库搜索只发现 [`util_test.rs`](util_test.rs) 构造 `new_table_info_getter` 并调用 init-stats 轻量查找；当前生产 Rust 代码没有构造 `CachedTableInfoGetter`，也没有调用两个 `*_for_init_stats` 方法。也就是说，本文件的实现和接口契约存在，但与 Rust `Handle` 的运行时组合仍未形成与 Go 相同的接线。

Go 上游已完整接线：[`../handle.go`](../handle.go) 的 `NewHandle` 注入 `util.NewTableInfoGetter()`；[`../bootstrap.go`](../bootstrap.go) 在加载 `stats_meta` 时调用 `TableItemByIDForInitStats`，并在初始化直方图时调用 `TableInfoByIDForInitStats`。其他 Go 统计路径通过嵌入在 Handle/StatsHandle 中的 `TableInfoGetter` 使用通用查找。

RustCodeGraph 的全局索引状态可用，但 `files --filter table_info.rs` 和目标完整路径查询没有收录此文件；因此本文件的直接调用关系以 `rg` 和源码读取核验，不能声称已由图数据库确认 callers/callees。

## 错误处理与边界

所有查找失败都以 `Option::None` 表示，没有日志或错误分类。以下情况对上游不可区分：ID 不存在、对象刚被删除、分区缓存中没有该 ID、缓存得到父表 ID后父表再次查找失败。调用者必须把 `None` 当作正常的“当前 schema 中不可解析”结果，而不能假设是内部错误。

`Mutex::lock().unwrap()` 在锁中毒时会 panic；本文件没有恢复策略。`InfoSchema` trait 方法也不返回 `Result`，无法表达存储/刷新错误。这符合当前纯内存查询抽象，但若未来实现可能失败，需要先扩展 trait 契约并同步所有实现和调用者。

缓存失效只比较 schema 版本，不比较 InfoSchema 实例身份。若两个不同实例使用相同版本号却描述不同 schema，并共享同一个 getter，旧映射会被复用；当前设计依赖 schema 版本在该共享范围内能唯一代表元数据快照。

建图时不验证重复分区 ID，也不校验父表 ID。`HashMap::insert` 对重复键后写覆盖；该行为依赖 InfoSchema 的全局物理 ID 唯一不变量。空分区列表、空分区表集合和未知 ID 都自然得到 `None`。

## 并发与资源生命周期

`InfoSchema` 和 `TableInfoGetter` 都要求 `Send + Sync`，构造函数又返回 `Arc<dyn TableInfoGetter>`，明确支持多个统计任务共享 getter。`CachedTableInfoGetter` 用一个 `Mutex<PartitionCache>` 串行化 V1 初始化统计的缓存检查、全量重建和查值；因此不会出现两个线程同时用不同 schema 版本重建并交叉写入映射。

锁在 `partition_id_to_table_id_for_init_stats` 返回前释放。完整表/轻量条目的最终 `table_by_id` 或 `table_item_by_id` 调用发生在锁外，缩短了持锁时间；但 `build_partition_id_to_table_id` 及其 `partitioned_tables` 全量遍历发生在锁内，版本切换后的首个查找会阻塞其他并发 V1 init-stats 查找。这是与 Go `sync.RWMutex` 实现相同的串行重建语义，尽管 Rust 这里使用独占 `Mutex`，没有并行读路径。

缓存没有后台任务、通道、显式关闭或容量淘汰。它随最后一个 `Arc<dyn TableInfoGetter>` 释放而销毁；映射随 schema 版本变化整体替换。返回的 `Arc<TableMeta>` 生命周期独立于缓存，因为缓存只持有整数 ID。

## 与 Go 版本的对应关系

[`table_info.go`](table_info.go) 是直接语义基准，主要对应关系如下：

| Go | Rust | 说明 |
| --- | --- | --- |
| `TableInfoGetter` | `TableInfoGetter` | 四个查找入口一一对应；Go 的 `(value, bool)` 在 Rust 中合并为 `Option<value>`。 |
| `tableInfoGetterImpl` | `CachedTableInfoGetter` | 都只持有 V1 init-stats 分区缓存。 |
| `pid2tid` / `schemaVersion` | `partition_to_table` / `schema_version` | 缓存数据和按版本失效规则一致。 |
| `partitionID2TableIDForInitStats` | `partition_id_to_table_id_for_init_stats` | 都在锁内检查版本、必要时全量重建并查值。 |
| `buildPartitionID2TableID` | `build_partition_id_to_table_id` | 都将所有分区映射到父表。 |
| `NewTableInfoGetter` | `new_table_info_getter` | 都以接口对象隐藏实现；Rust 额外用 `Arc` 提供共享所有权。 |

两者的数据模型不同。Go 直接接收真实 `infoschema.InfoSchema`，返回 `table.Table`/`infoschema.TableItem`，并通过 `ListTablesWithSpecialAttribute(PartitionAttribute)` 建图；Rust 定义本地 `InfoSchema`、`TableMeta`、`TableItem`，由 `partitioned_tables` 直接提供候选表。Go 建图使用 `intest.AssertNotNil` 断言候选表确有分区信息；Rust 的 `TableMeta.partitions` 总是一个向量，因此没有同等的空值断言。

Go 的 V2 判定由 `infoschema.IsV2` 完成，Rust 把它收敛为 `InfoSchema::is_v2`。算法分支相同。Go 实现已嵌入生产 `Handle` 并由 `bootstrap.go` 使用；Rust 当前只有接口组合与测试调用，尚不能据此宣称生产初始化统计已走这条缓存路径。

## 扩展指南

- 若新增查找形态，先判断它是通用路径还是 init-stats 性能敏感路径。V1 分区查找不得绕过 `partition_id_to_table_id_for_init_stats` 回退到逐分区全表扫描。
- 若修改缓存失效条件、锁粒度或建图数据源，应同步 [`table_info.go`](table_info.go) 的语义，并为“版本不变复用、版本变化重建、并发查找”增加独立 Rust 测试。Rust 测试必须继续放在 [`util_test.rs`](util_test.rs) 或新的独立 `*_test.rs` 文件中，不能内嵌到生产源文件。
- 若把本地 DTO/trait 换成真实 `astersql-infoschema` 和 meta 类型，需要同时调整 [`Cargo.toml`](Cargo.toml) 中目前位于 `cfg(any())` 的依赖、[`lib.rs`](lib.rs) 的公开导出以及 [`../types/interfaces.rs`](../types/interfaces.rs) 的 `StatsHandle` 契约，并核查对象安全性和 trait-object 转换。
- 若完成生产接线，应在 Rust `Handle` 构造与初始化统计调用点接入 `new_table_info_getter`/两个 init-stats 方法，并以同路径 Go [`../handle.go`](../handle.go)、[`../bootstrap.go`](../bootstrap.go) 为行为基准；不能只依靠本文件单测声称主链已经启用。
- 若让 `InfoSchema` 操作可失败，不能只在实现内吞掉错误；应把 `Option` 契约升级为可区分“未找到”和“查询失败”的结果，并逐一迁移上游。
- 性能风险集中在 schema 版本变化时的锁内全量建图，以及误用通用 V1 分区查找造成的重复扫描；兼容风险集中在物理 ID 唯一性、schema 版本作用域和公开 trait 签名。

现有独立 Rust 测试 [`util_test.rs`](util_test.rs) 的 `test_table_item_by_id_for_init_stats_avoids_v1_partition_scan` 覆盖普通表命中、分区经缓存解析、未知 ID 返回 `None`，并令 `table_item_by_partition_id` 直接 panic，从而证明 V1 专用路径没有调用昂贵回退。对应 Go 测试位于 [`util_test.go`](util_test.go) 的 `TestTableItemByIDForInitStatsAvoidsV1PartitionScan`。当前未见针对完整元数据入口、V2 分支、schema 版本失效、重复 ID 或并发重建的专门 Rust 测试，扩展这些区域时应优先补齐。

## 验证依据

- 目标源码：[`table_info.rs`](table_info.rs)。核对了全部 3 个 DTO、2 个 trait、2 个缓存结构、1 个私有缓存查找方法、4 个 trait 方法实现、建图函数和构造函数；文件无条件编译项。
- crate 边界：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)。核对 crate 名、Go 包映射、依赖条件、模块声明与公开重导出。
- Go 对照与生产接线：[`table_info.go`](table_info.go)、[`../handle.go`](../handle.go)、[`../bootstrap.go`](../bootstrap.go)、[`../types/interfaces.go`](../types/interfaces.go)。核对接口、缓存算法、构造注入与 init-stats 调用点。
- Rust 上游契约：[`../types/interfaces.rs`](../types/interfaces.rs) 的 `StatsHandle: ... + TableInfoGetter + ...`，以及 [`../types/interfaces_test.rs`](../types/interfaces_test.rs) 的 supertrait 编译契约测试。
- 独立测试：[`util_test.rs`](util_test.rs) 的 `PartitionItemLookupForbiddenInfoSchema` 与 `test_table_item_by_id_for_init_stats_avoids_v1_partition_scan`；Go 对应测试为 [`util_test.go`](util_test.go) 的 `TestTableItemByIDForInitStatsAvoidsV1PartitionScan`。
- 调用搜索：对 Rust/Go 的 `new_table_info_getter`/`NewTableInfoGetter`、`TableInfoGetter`、四个查找方法和两个建图函数执行了仓库范围 `rg`；结果支持“Rust 仅测试构造、Go 已生产接线”的结论。
- RustCodeGraph：`status` 显示索引可用（11,467 个文件），但 `files --filter table_info.rs` 返回无匹配，目标路径的 `explore`/`node` 未提供源码或调用边；因此没有用图输出替代缺失证据，相关关系由上述源码和搜索结果交叉核验。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付时另运行任务规定的 11 章节结构检查，并人工复核未把未接线能力写成已启用能力。

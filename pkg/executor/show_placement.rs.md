# `pkg/executor/show_placement.rs` 逻辑说明

## 文件定位

`pkg/executor/show_placement.rs` 是 `astersql-executor` crate 中 `SHOW PLACEMENT` 与 `SHOW PLACEMENT LABELS` 的 Rust 逻辑层。`pkg/executor/lib.rs` 通过 `pub mod show_placement` 导出它；`pkg/executor/Cargo.toml` 将 crate 命名为 `astersql-executor`，并以同目录 `lib.rs` 为库入口。本文件本身只直接使用 `std::collections::{HashMap, HashSet}` 和 `std::fmt::Display`；InfoSchema、权限、DDL range 策略、表 key 编码与 PD 复制状态都经 `ShowPlacementBackend` trait 注入，而不是直接依赖 Cargo 中的具体 crate。

当前 Rust 接线状态需要特别说明：全仓 Rust 引用搜索中，`ShowPlacementExec` 以及 `fetchShowPlacement*` 入口只由 `pkg/executor/show_placement_test.rs` 调用，没有找到 `ShowPlacementBackend` 的生产实现或 SQL 执行主链调用者。因此它是已移植并被单测验证的可注入核心，但不应将其描述成已取代 Go 生产路径。Go 的真实入口仍是 `pkg/executor/show.go` 对 `ShowExec.fetchShowPlacement*` 的分派，实现位于 `pkg/executor/show_placement.go`。本目录没有 `doc.go`。

## 核心职责

- 把命名 Placement Policy、数据库、表、分区、`TiDB_GLOBAL`/`TiDB_META` range 统一投影为 `PlacementRow`，每行包含对象、策略文本和调度状态（`ShowPlacementExec::fetchShowPlacement`）。
- 针对指定数据库、表或分区提供精确查询，并在库/表全量查询时执行权限可见性过滤（`fetchShowPlacementForDB`、`fetchShowPlacementForTable`、`fetchShowPlacementForPartition`、`fetchAllDBPlacements`、`fetchAllTablePlacements`）。
- 解析对象引用的策略，并实现“分区显式策略优先，否则继承表策略”的语义（`getPolicyPlacement`、`getPartitionPlacement`）。
- 对表及其分区的 PD 复制状态做缓存、汇总和短路，总体状态取最落后的一项（`fetchScheduleState`、`fetchTableScheduleState`、`fetchTablePartitionScheduleState`、`accumulateState`）。
- 将受限 SQL 返回的 Store labels 按 key 聚合，对 value 去重排序，产生稳定的 JSON 数组输出（`showPlacementLabelsResultBuilder`）。

## 主要符号

- `TiDBBundleRangePrefixForGlobal` / `TiDBBundleRangePrefixForMeta`：完整 `SHOW PLACEMENT` 额外查询的两个 PD range bundle ID。
- `CiString`：保留 `original` 用于展示，缓存 `lower` 用于大小写不敏感比较；`String` 返回原始写法。
- `StoreLabel` / `StoreLabelsJson`：后端已解析的 Store label 及 JSON 形态。`Null` 被忽略，`Array` 被合并，`Other` 是错误。
- `PlacementValue` / `PlacementRow`：SHOW 结果单元格与行模型，区分普通字符串和 JSON 字符串数组。
- `PlacementSettings`、`PolicyRefInfo`、`PolicyInfo`、`DatabaseInfo`、`TableInfo`、`PartitionDefinition`、`SpecialAttributeDatabase`：本文件使用的轻量元数据 DTO，把 Go 中 `model`/InfoSchema 对象所需字段缩减到本逻辑的边界。
- `PlacementScheduleState`：`Pending = 0`、`InProgress = 1`、`Scheduled = 2`；数值顺序是 `accumulateState` 取最小值的不变量，`String` 分别输出 `PENDING`/`INPROGRESS`/`SCHEDULED`。
- `ScheduleError<E>`：在底层查询失败时保留已累积状态与后端错误。
- `ShowPlacementBackend`：唯一外部能力边界，定义错误构造、权限可见性、InfoSchema 查询、range 策略/key、表 range 编码和 PD 复制状态查询。
- `showPlacementLabelsResultBuilder`：以 `HashMap<String, HashSet<String>>` 合并 labels；`AppendStoreLabels`负责验证/累积，`BuildRows` 负责确定性排序与行构造。
- `ShowPlacementExec<B>`：主执行器，持有后端、可选查询目标（`DBName`/`TableSchema`/`TableName`/`Partition`）和累积结果 `rows`。
- 状态函数组：`fetchScheduleState`、`fetchPartitionScheduleState`、`fetchTablePartitionScheduleState`、`fetchTableScheduleState`、`ShowPlacementExec::fetchTableScheduleStateByTableID`、`ShowPlacementExec::fetchDBScheduleState`。
- 内部工具：`format_identifier` 转义反引号并生成 `` `schema`.`table` ``；`decode_hex_ignoring_error`/`hex_digit` 模拟 Go `hex.DecodeString` 被忽略错误时保留已解码前缀的效果。

## 执行流程

1. 调用者构造 `ShowPlacementExec<B>`，填充查询目标并提供具体 `ShowPlacementBackend`。当前仓库中该步骤只在 Rust 测试的 `MockBackend` 中有实例。
2. 完整 `fetchShowPlacement` 首先调用 `fetchAllPlacementPolicies`，按 policy 原始名排序并输出状态 `NULL`。
3. 执行器建立一个 `HashMap<i64, PlacementScheduleState>`，先交给 `fetchAllDBPlacements`，再交给 `fetchAllTablePlacements`。同一表/分区 ID 在两阶段中共享 PD 查询结果。
4. `fetchAllDBPlacements` 对 schema 名排序，跳过不可见库；只有库显式引用 policy 时才查询全库调度状态并输出。`fetchDBScheduleState` 遍历库内简单表 ID，每得到非 `Scheduled` 状态就短路。
5. `fetchAllTablePlacements` 消费 `tables_with_special_attributes` 的库分组，过滤不可见表；表有 policy 则输出表行，分区有自身 policy 或可继承表 policy 则输出分区行。每个库组内先按表名排序，表行保持在其分区行之前。代码虽计算了排序后的 `databases`，实际遍历仍使用后端返回的 special-attribute 分组顺序，这与 Go 现状一致。
6. `fetchRangesPlacementPlocy` 依次处理 `TiDB_GLOBAL` 和 `TiDB_META`：无 policy 的 range 跳过；否则解码 key range、查 PD 状态、再解析 policy 展示文本并输出。
7. 精确入口 `fetchShowPlacementForDB`/`ForTable`/`ForPartition` 只处理指定对象；没有可展示 policy 时成功返回空结果。分区查找使用 `CiString.lower`。
8. `fetchShowPlacementLabels` 通过后端取回已解析 label JSON，聚合所有 store，再按 label key 和 value 双层排序后追加到 `rows`。

## 数据与状态

`ShowPlacementExec.rows` 是执行器的唯一持久可见结果状态；所有 fetch 方法均通过私有 `appendRow` 只追加不清空。因此实例若被重复调用会累加旧行，调用者应为一次 SHOW 创建新实例或显式管理 `rows`。

全量路径的 `scheduled: HashMap<i64, PlacementScheduleState>` 是单次调用内的查询缓存：key 是表或物理分区 ID，value 是 PD 复制状态。`fetchScheduleState` 只缓存成功结果，错误不入缓存。指定 DB/表/分区入口传入 `None`，不共享缓存。

`PlacementScheduleState` 的枚举值顺序同时表示进度偏序：`Pending < InProgress < Scheduled`。`accumulateState` 总是保留较小值，所以表或库的总状态不会比任一子 range 更乐观。`showPlacementLabelsResultBuilder.labelKey2values` 则是构建 labels 时的去重集合，`HashMap`/`HashSet` 的非确定遍历顺序在 `BuildRows` 中被显式排序消除。

## 依赖与调用关系

上游关系分为“模块可见”和“实际调用”两层：`pkg/executor/lib.rs` 公开模块，并以 `#[path = "show_placement_test.rs"]` 和 `#[path = "show_placement_labels_test.rs"]` 挂载独立测试；但 RustCodeGraph `callers fetchShowPlacement`/`callers ShowPlacementExec` 没有给出生产调用边，`rg` 也确认 Rust 侧只有上述测试构造执行器。Go 侧上游是 `pkg/executor/show.go:279-287`，根据 SHOW 类型调用 `fetchShowPlacementLabels`、`fetchShowPlacement`、`fetchShowPlacementForDB`、`fetchShowPlacementForTable` 或 `fetchShowPlacementForPartition`。

下游关系由 `ShowPlacementBackend` 清晰划分：

- 受限 SQL/JSON 边界：`restricted_store_labels`。
- 权限边界：`database_visibility_check_enabled`、`database_is_visible`、`table_privilege_check_enabled`、`table_is_visible`。
- InfoSchema/元数据边界：`schema_by_name`、`selected_table`、`all_placement_policies`、`all_schema_names`、`schema_simple_table_ids`、`tables_with_special_attributes`、`policy_by_name`、`table_by_id`。
- DDL/range 边界：`range_policy_name`、`range_key_hex`。
- 存储/PD 边界：`encoded_table_range`、`replication_state`。

RustCodeGraph 对文件内部主调用链的核对结果包括 `fetchShowPlacement -> fetchAllDBPlacements -> getDBPlacement` 和 `fetchShowPlacement -> fetchAllTablePlacements -> getTablePlacement`；`fetchAllPlacementPolicies` 与 `fetchRangesPlacementPlocy` 也由 `fetchShowPlacement` 直接调用。`pkg/executor/Cargo.toml` 列出 executor 生产环境可用的 DDL、domain/infosync、infoschema、meta/model、privilege、tablecodec 等 crate，但本文件尚未直接导入它们；`nextgen` feature 也未对本模块做条件编译。

## 错误处理与边界

- 所有外部错误由 `B::Error` 表示；trait 提供 `error`、`database_access_denied`、`database_not_exists`、`unknown_partition` 以保留生产环境的错误类型/文案。
- 指定数据库不可见时，`fetchShowPlacementForDB` 返回 access denied；全量数据库查询则静默跳过不可见库。全量表查询同样静默跳过不可见表。
- 表无分区或分区名不存在都经 `unknown_partition` 报错；比较大小写不敏感。
- policy 引用存在但无法按名找到时，`getPolicyPlacement` 和 range 路径返回 `Policy with name '...' not found`；没有 policy 引用本身不是错误，而是 `Ok(None)` 并且不输出对象行。
- `fetchScheduleState` 的 PD 错误包装为 `ScheduleError { state: Pending, error }`。表自身 range 失败时，`fetchTableScheduleState` 保留当时初值 `Scheduled`；分区 range 失败时，`fetchTablePartitionScheduleState` 对齐 Go 行为将错误附带状态设为 `Pending`。普通 fetch 入口对外只传播 `failure.error`。
- `fetchTableScheduleStateByTableID` 只在表 range 已是 `Scheduled` 时查找表元数据；此时 ID 不存在才返回 `Table with ID '...' not found`。
- Store labels 的 `Null` 允许并忽略；非 array/非 null 返回固定错误。JSON 解码和存储错误刻意由 backend 处理，因此 `StoreLabelsJson` 不能表达“解码过程中的错误”。
- `decode_hex_ignoring_error` 对奇数长度的最后半字节或首个非法 pair 之后的内容不报错，只返回成功解码的完整字节前缀；这是对 Go 路径主动忽略 `hex.DecodeString` 错误的对齐，扩展时不应随意改成严格拒绝。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、锁、事务或文件句柄；所有方法均是同步且按顺序查询。`ShowPlacementExec` 中的 backend 和 `rows` 在整个执行器值的生命期内存活，`Context: Clone` 是 trait 约束，但当前方法只借用 `&Context`，本文件未克隆它。

`Option<&mut HashMap<...>>` 使缓存仅在调用链上串行独占借用，无内部竞态；但 `ShowPlacementBackend` 没有 `Send`/`Sync` 约束，`ShowPlacementExec` 也不宣称可被多线程共享。后端应负责自身 context、InfoSchema 快照、受限 SQL 结果和 PD 客户端资源的获取/释放；本层只在函数返回前保留结果行和单次缓存。遇到非 `Scheduled` 即短路可避免对后续分区/表的额外 PD 请求。

## 与 Go 版本的对应关系

Rust 的主要函数与 `pkg/executor/show_placement.go` 的同名方法基本一一对应：完整/指定对象查询、policy 枚举、DB/表/分区 policy 解析、range policy、调度状态缓存与汇总，以及保留 Go 拼写的 `fetchRangesPlacementPlocy`。关键语义对齐包括：policy 按原始名排序；分区继承表 policy；权限不可见对象在全量列表中被过滤；调度状态取最落后值；非 `Scheduled` 短路；成功 PD 查询按物理 ID 缓存；range hex 解码错误被忽略。

两者的主要结构差异是：

- Go 方法直接挂在生产 `ShowExec` 上，并直接调用 session/privilege/InfoSchema/DDL/infosync/tablecodec；Rust 将这些收口为 `ShowPlacementBackend`，但尚没有生产 adapter。
- Go label builder 接受 `types.BinaryJSON`，自身 marshal/unmarshal 并可返回 JSON 错误；Rust 后端先把输入归类为 `StoreLabelsJson`，`BuildRows` 直接产生 `JsonStringArray`，因此不再是 fallible。
- Go 使用完整 `model` 类型、`ast.CIStr`、`ast.Ident` 和 `infosync.PlacementScheduleState`；Rust 在文件内定义等价的精简类型并手写标识符转义。
- Go 表 range 通过 `codec.EncodeBytes(nil, tablecodec.GenTablePrefix(id))` 生成；Rust 要求 backend 的 `encoded_table_range` 返回完整 key 边界。生产 adapter 必须确保编码完全一致。

Go 回归 `pkg/executor/show_placement_test.go` 还覆盖 SQL 分派、实际权限错误、不存在对象、完整排序和 PD HTTP 状态映射；Rust 单测只验证纯逻辑/后端合约的一个子集，不能作为 Rust 生产接线已完成的证据。

## 扩展指南

- 新增 SHOW 对象类型时，优先在 `ShowPlacementExec` 增加聚焦的 fetch 方法，并在完整 `fetchShowPlacement` 中按 Go 输出顺序接入；如需新的外部能力，扩展 `ShowPlacementBackend`，不要把会话/PD 具体类型泄漏进纯逻辑。
- 改动 policy 继承时应聚焦 `getPartitionPlacement`、`getTablePlacement` 和 `getPolicyPlacement`，同步扩展 `pkg/executor/show_placement_test.rs` 中的继承/缺失 policy 用例，并与 Go 的 SQL 期望对照。
- 新增调度状态或改动聚合规则时，必须同步检查 `#[repr(i32)] PlacementScheduleState`、`String`、`accumulateState` 及所有短路分支。正确性风险是枚举数值与进度顺序脱节；性能风险是取消缓存/短路后 PD 请求数按表和分区数量增长。
- 改动结果顺序时，需区分 policy 排序、schema 可见性、special-attribute 分组顺序、表名排序和分区定义顺序；同步验证 Rust `show_placement_test.rs` 及 Go `show_placement_test.go`，避免不必要的 golden 顺序变化。
- 改动 labels 解析/输出时，同步 `showPlacementLabelsResultBuilder`、`ShowPlacementBackend::restricted_store_labels`、`pkg/executor/show_placement_labels_test.rs` 和 Go 对照测试。兼容性风险是 SQL 列类型从 JSON 数组退化为普通字符串，或丢失稳定排序。
- 若要把 Rust 路径接入真实 SQL 执行，需在另一层实现生产 `ShowPlacementBackend`，并从 Rust SHOW 分派器构造 `ShowPlacementExec`。重点验证权限错误类型、InfoSchema 快照一致性、table key 编码、PD 状态映射和 JSON 列类型；不应仅依赖现有 mock 测试。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/executor/show_placement.rs --offset 1 --limit 900` 读取了全部 791 行，确认常量、DTO、trait、builder、执行器方法、状态函数和内部工具；文件无条件编译项。
- 符号与调用边：RustCodeGraph `explore 'pkg/executor/show_placement.rs ShowPlacementExec placement'`、`query ShowPlacementExec`、`callers/callees fetchShowPlacement`、`callers/callees fetchScheduleState`、`callers ShowPlacementExec`。Explore 确认文件内主链；精确 callers 查询未返回生产边，再用 `rg` 核对全仓 Rust 引用，只找到源文件与独立测试。
- crate 边界：读取 `pkg/executor/Cargo.toml` 和 `pkg/executor/lib.rs`，确认 crate 名、库入口、`nextgen` feature、模块导出及两个独立 Rust 测试的挂载位置。`pkg/executor/doc.go` 不存在，因此无可读的包级 Go 合同文件。
- Rust 回归：`pkg/executor/show_placement_test.rs` 验证最落后状态、policy 排序、分区继承/覆盖、标识符转义、缓存调用数、非 Scheduled 短路、分区 PD 错误状态、DB 可见性和缺失 policy；`pkg/executor/show_placement_labels_test.rs` 验证空输入、null、合并去重排序及非 array/null 拒绝。
- Go 对照：RustCodeGraph 读取 `pkg/executor/show_placement.go`、`pkg/executor/show_placement_test.go`、`pkg/executor/show_placement_labels_test.go`；另以 `rg` 确认 `pkg/executor/show.go:279-287` 的生产分派边。Go 测试补充了 SQL 输出、权限、缺失对象和 PD HTTP 状态映射证据。
- 本任务仅生成说明文档，依计划不运行 Cargo；结构验证使用任务文件指定的 `test -f` 与 11 个固定二级标题计数命令。

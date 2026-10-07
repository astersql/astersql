# `pkg/infoschema/tables.rs`

源码：[`tables.rs`](tables.rs)；独立 Rust 测试：[`tables_test.rs`](tables_test.rs)；Go 对照：[`tables.go`](tables.go)、[`tables_test.go`](tables_test.go)。

## 文件定位

本文件是 `astersql-infoschema` crate 中 INFORMATION_SCHEMA 静态元数据和若干元数据辅助能力的集中实现。crate 根模块 [`lib.rs`](lib.rs) 通过 `pub mod tables` 装配它，并只在根级额外再导出版本格式化和 TiFlash 标签判定函数；其余 API 由调用者经 `astersql_infoschema::tables` 使用。crate 边界和直接依赖由 [`Cargo.toml`](Cargo.toml) 声明：表元数据转换依赖 `astersql-meta-model`、`astersql-meta-autoid`、parser AST/charset/mysql，运行期开关依赖 `astersql-config`。

在应用主链中，`Builder::init_information_schema_tables`（[`builder.rs`](builder.rs)）调用 `information_schema_db_with_storage_class`，把这里构造的 `DBInfo` 和 `TableInfo` 放入 V1 的 `databases` 以及可选的 V2 `info_data`。因此本文件决定 Rust InfoSchema 快照中有哪些 INFORMATION_SCHEMA 表、它们的稳定 ID 和静态列元数据；它不负责 SQL 执行器按查询动态生成各表的业务行。

文件还承载三组与静态注册表相邻的辅助能力：通过 `ServerDiscovery` 汇总集群节点、格式化版本和识别 TiFlash 标签；用 `infoschemaTable` 表达只读内存虚拟表；用 `GetShardingInfo` 生成人类可读的行 ID 分片说明。这些能力在 Go 的同名文件中也存在，但 Rust 当前实现范围明显小于 Go，差异见后文。

## 核心职责

1. **定义表名与稳定身份。** `table_constants!` 声明 78 个本地表名常量，`TABLE_NAMES` 再加入来自 `cluster.rs` 的 `ClusterTableTiDBIndexUsage`，形成 79 个注册项。`table_id_offset` 显式复刻 Go `tableIDMap` 的非连续偏移，`table_registry` 将偏移加到 `INFORMATION_SCHEMA_DB_ID` 上。偏移可能进入已缓存计划，不能按数组下标重新编号。
2. **描述并转换列元数据。** `columnInfo` 是静态、可常量构造的简化描述；`default_columns` 为一部分关键表给出逐列定义，并把 `SLOW_QUERY`、语句摘要委托给 `catalog_columns_go45`；未单独移植的注册表项由 `fallback_columns` 暂时得到 `INSTANCE/NAME/VALUE` 三列。`buildTableMeta` 把这些描述转换成 parser/meta-model 使用的字段类型、字符集、排序规则、长度、精度、标志、默认值和公开状态。
3. **构造 INFORMATION_SCHEMA 快照。** `information_schema_db_with_storage_class` 一次性缓存完整 `DBInfo`，每次返回克隆，并按实例配置选择是否从克隆中移除 `TIKV_STORAGE_CLASS_TRANSITIONS`；`information_schema_db` 从全局配置取得该开关。
4. **提供集群辅助纯逻辑。** `ServerDiscovery` 把实际发现机制抽象为八类组件方法；`GetClusterServerInfo` 顺序汇总并规范化回环地址。版本格式化、精确标签匹配和节点过滤均为无外部 I/O 的函数。
5. **提供只读表及分片显示语义。** `infoschemaTable` 共享持有元数据和固定行集，允许遍历和读列信息，拒绝增删改；`GetShardingInfo` 根据库类别、视图标记和 `model_meta` 返回分片描述。

## 主要符号

- `ColumnType`：本文件支持的 MySQL 字段类型子集，包括整数、浮点、Blob、时间、十进制和 JSON。它不是 Go `byte` 类型码的完整替代；例如 Go `columnInfo.enumElems` 和 Enum 类型在这里没有对应字段。
- `columnInfo`：列名、类型、显示长度、可选小数位、四类布尔标志、字符串默认值和注释。`typed`、`varchar`、`integer` 提供基础构造器，`unsigned`、`not_null`、`with_default` 用 builder 风格设置属性。
- `VirtualTableMeta`：注册表中的稳定 `id`、静态 `name` 与 `columns`。
- `TABLE_NAMES`、`table_id_offset`、`TABLE_REGISTRY`、`table_registry`：79 项注册表的输入、稳定 ID 映射与 `OnceLock` 懒初始化入口。未知名称传给 `table_id_offset` 会 panic；正常路径只遍历 `TABLE_NAMES`。
- `default_columns`、`fallback_columns`：列定义分派。前者只对代码中的显式 match 分支提供真实列集；默认分支是迁移期三列占位，不应视为已与 Go `tableNameToColumns` 对齐。
- `buildColumnInfo`：构造轻量 `infoschema::ColumnInfo`，只保留 ID、名称和 `auto_increment=false`。
- `buildTableMeta`：构造完整 `meta_model::TableInfo` 并嵌入轻量 `infoschema::TableInfo.model_meta`。注册表外名称的轻量/模型 ID 都为 0。
- `GetStorageClassTransitionsTableColumns`：从注册项重新构造并返回新列向量，避免调用者修改全局定义。
- `information_schema_db`、`information_schema_db_with_storage_class`、`INFORMATION_SCHEMA_DB`：完整快照的公开入口、可测试入口与一次性缓存。
- `ServerInfo`、`ServerDiscovery`：集群节点值对象和发现接口。trait 默认实现均返回空列表，具体发现由调用方注入。
- `GetClusterServerInfo` 及八个单组件 getter：聚合或转发发现调用。聚合顺序是 TiDB、PD、Store、TiFlash、TiProxy、TiCDC、TSO、Scheduling，任一错误立即返回。
- `FormatTiDBVersion`、`FormatStoreServerVersion`：分别处理默认 TiDB 兼容前缀和单个前导 `v`。
- `StoreLabel`、`StoreInfo`、`isTiFlashStore`、`isTiFlashWriteNode`：在精简 Store 标签模型上做区分大小写的精确匹配；snake_case 两函数只是 Rust 测试友好的转发别名。
- `FilterClusterServerInfo`：节点类型集合和地址集合分别为空时表示该维度不过滤；两个非空条件之间是 AND。
- `infoschemaTable`、`createInfoSchemaTable`、`VirtualTable`：固定行集的只读包装、通用 `Table::new` 工厂及无状态标记。`VirtualTable` 当前没有 impl。
- `GetShardingInfo`：返回 `None`、`PK_AUTO_RANDOM_BITS=...`、`SHARD_BITS=...`、`NOT_SHARDED(PK_IS_HANDLE)` 或 `NOT_SHARDED`。

## 执行流程

**构建注册表和 InfoSchema 快照：**

1. 首次调用 `table_registry` 时，`TABLE_REGISTRY.get_or_init` 遍历 `TABLE_NAMES`。
2. 每个名称经 `table_id_offset` 取得与 Go 对齐的固定偏移，经 `default_columns` 取得真实或占位列定义，组成 `VirtualTableMeta` 并收集到 `HashMap`。
3. 首次调用 `information_schema_db_with_storage_class` 时，`INFORMATION_SCHEMA_DB.get_or_init` 遍历注册表值；每项由 `buildTableMeta` 构造成 `Arc<TableInfo>`，最终形成名为 `INFORMATION_SCHEMA`、ID 为 `INFORMATION_SCHEMA_DB_ID` 的 `DBInfo`。
4. `buildTableMeta` 对每列设置 MySQL 类型；字符串/Blob 使用 UTF8MB4，其余使用 binary 字符集和排序规则；Blob 长度按 16/24/32 位容量覆盖声明 size；再合成 unsigned、not-null、primary-key、binary 标志，设置 offset、公开状态、注释和可选字符串默认值。
5. 每次调用都克隆缓存快照；当 `enable_storage_class=false` 时，仅从克隆的 `tables` 中移除 `TIKV_STORAGE_CLASS_TRANSITIONS`，不会污染缓存。因此测试中的 true/false 交替调用保持独立。
6. `Builder::init_information_schema_tables` 将快照表包装成 `Table`，以表 ID 建图，并按 V1/V2 模式接入当前 schema 版本。

**发现和过滤集群节点：**

1. 调用者实现 `ServerDiscovery`；未覆盖的方法默认贡献空列表。
2. `GetClusterServerInfo` 按固定顺序逐个调用发现方法并追加结果；`?` 保证首个错误中止后续汇总。
3. 所有成功收集的节点逐个调用 `ResolveLoopBackAddr`。当业务地址与状态地址恰有一侧为回环/未指定地址时，用另一侧主机替换它的主机并保留原端口；两侧同类时不变。
4. 查询侧可再以 `FilterClusterServerInfo` 按精确组件类型和地址裁剪列表。

**读取固定内存虚拟表：**

1. `infoschemaTable::new` 将 `TableInfo` 与二维 `Datum` 行集分别放入 `Arc`。
2. `IterRecords` 依次把切片交给回调；回调返回 `false` 时提前停止。
3. `Cols`/`VisibleCols`、`Meta` 和 `GetPhysicalID` 暴露只读视图；`HiddenCols` 恒为空；三种写操作稳定返回不支持错误。

**生成分片说明：** `GetShardingInfo` 先排除视图和 information/performance/metrics/mysql/sys/workload 等内存或系统库；无 `model_meta` 时返回 `NOT_SHARDED`；否则按 auto-random、`ShardRowIDBits`、`PKIsHandle` 的优先级选择文本。auto-random 的 range bits 只在非 0 且非默认值 64 时附加。

## 数据与状态

- `TABLE_REGISTRY: OnceLock<HashMap<...>>` 是进程级不可变注册表。初始化后只借用静态引用；`VirtualTableMeta.columns` 不通过公开 API 可变借用。
- `INFORMATION_SCHEMA_DB: OnceLock<DBInfo>` 缓存完整表集合。对存储类开关的过滤发生在克隆上，这是 `information_schema_db_with_storage_class` 可重入且不同调用互不干扰的关键不变量。
- 注册表底层是 `HashMap`，所以 `DBInfo.tables` 的遍历顺序不稳定；语义依赖名称和稳定 ID，而不应依赖向量位置。
- `columnInfo` 的静态字符串使大部分列描述可由常量构造器表达；`default_columns` 每次返回新 `Vec`，注册表初始化后由注册表拥有这些向量。
- `buildTableMeta` 同时构造两层列模型：轻量 `TableInfo.columns` 只含查询此 crate 所需的摘要，`model_meta.Columns` 保留 parser/meta 所需完整字段属性。两者列次序一致，轻量列 ID 从 1 开始，模型列 ID/offset 从 0 开始。
- `ServerInfo`、`StoreInfo` 和过滤集合均由调用方拥有；聚合会移动各发现结果，过滤会消费输入 `Vec`。没有全局拓扑缓存。
- `infoschemaTable` 的 `Arc<TableInfo>` 与 `Arc<Vec<Vec<Datum>>>` 允许廉价共享不可变数据；类型本身未实现写时复制或内部可变性。

## 依赖与调用关系

上游直接证据：

- [`builder.rs`](builder.rs) 的 `init_information_schema_tables` 调用 `information_schema_db_with_storage_class`，再将表放入 V1/V2 InfoSchema。
- [`pkg/session/runtime/system_query.rs`](../session/runtime/system_query.rs) 遍历 `table_registry().values()`，消费这里的静态注册项。
- [`test/clustertablestest/cluster_tables_test.rs`](test/clustertablestest/cluster_tables_test.rs) 直接调用 `GetClusterServerInfo`；其 harness 提供假发现实现。
- [`go_merge_45_test.rs`](go_merge_45_test.rs) 直接验证注册 ID、列转换、存储类可见性和大列集；[`tables_test.rs`](tables_test.rs) 验证目录列、标签、版本、过滤和分片分支。
- [`lib.rs`](lib.rs) 再导出版本与 TiFlash 标签辅助，使 crate 外可不写 `tables::` 路径使用这些纯函数。

下游直接依赖：

- `crate::infoschema::{CiString, ColumnInfo, DBInfo, Table, TableInfo}` 提供本 crate 的轻量 schema 模型；`crate::cluster::{Datum, ClusterTableTiDBIndexUsage}` 提供行值与额外集群表名。
- `astersql-meta-model`、`astersql-meta-autoid` 和 parser AST/mysql/charset 构造兼容 TiDB 元数据对象、固定 DB ID 及 MySQL 字段语义。
- `astersql-config::get_global_config().enable_storage_class` 控制默认快照是否公开存储类迁移表。
- 私有子模块 [`catalog_columns_go45.rs`](catalog_columns_go45.rs) 提供 `slow_query_columns` 和 `statements_summary_columns`，用于承载较大的 Go 4.5 对齐列集。
- 标准库 `OnceLock`、`Arc`、`HashMap`、`HashSet`、`IpAddr` 分别负责一次性初始化、共享所有权、注册/过滤和地址类别识别。

RustCodeGraph 的文件节点把本文件标为被 20 个文件使用，并成功定位 `table_registry`、`buildTableMeta`、`information_schema_db(_with_storage_class)`、`GetClusterServerInfo`、`GetShardingInfo`。本次图后端的批量 `callers/callees` 命令超时，因此上述具体调用边以相邻 Rust 源码的精确引用补证，而不是从超时结果推断。

## 错误处理与边界

- `table_id_offset` 对未注册名称 panic；这是内部一致性断言。`buildTableMeta` 本身对注册表外名称不报错，而是使用 ID 0，因此新增调用点若绕过 `TABLE_NAMES` 可能静默得到无效身份。
- `GetStorageClassTransitionsTableColumns` 使用 `expect`，假定该固定注册项永远存在；删除或重命名表时必须同步该函数。
- `ServerDiscovery` 用 `Result<Vec<ServerInfo>, String>` 表达发现失败。聚合函数短路返回首错，不返回部分结果；默认方法返回成功的空列表，因而“未实现发现”与“确实没有节点”无法由接口区分。
- 地址识别不做 DNS。它只认 `localhost` 或能解析为 `IpAddr` 的回环/未指定字面量；拆分和替换依赖 `host:port` 形状。与 Go `net.ResolveTCPAddr` 相比，主机名、某些 IPv6/异常地址处理并非完全等价。
- `FormatTiDBVersion` 在默认版本字符串不含 `TiDB-` 时返回空串；非默认配置原样返回。`FormatStoreServerVersion` 只剥掉一个小写前导 `v`，测试明确验证 `vv8.5.0 -> v8.5.0`。
- 标签和过滤均区分大小写且精确匹配；`TiFlash`、`WRITE` 不匹配小写约定。
- `fallback_columns` 是当前最重要的迁移边界：大量表名虽然已注册并有稳定 ID，但列定义仍只是通用三列，不能据此宣称对应 Go INFORMATION_SCHEMA 表已完整可用。
- `infoschemaTable` 的写 API 一律返回 `unsupported operation on virtual table`；`IterRecords` 没有错误返回通道，访问者只能用布尔值停止。
- Rust `GetShardingInfo` 接收非空 `&TableInfo`，无需覆盖 Go 的 nil table 分支；系统库列表是本地硬编码，未来新增系统库必须同步。

## 并发与资源生命周期

`OnceLock` 保证注册表和完整 DB 快照在多线程下最多初始化一次；初始化闭包没有异步操作、锁嵌套或外部 I/O。初始化完成后，注册表以 `&'static` 共享，DB 快照按值克隆，因此存储类过滤不会竞争修改全局对象。

`Arc` 用于表元数据、模型元数据和固定行集的共享生命周期。当前类型不含 `Mutex`、`RwLock`、channel、事务或后台任务；线程安全性质取决于其字段的只读共享。`GetClusterServerInfo` 与 Go 不同，是同步串行发现和串行地址规范化，不创建任务，也没有 Go 版本的受限并发错误组。任何真实网络连接、超时、重试或资源释放均属于 `ServerDiscovery` 实现者，而非本文件。

`infoschemaTable::IterRecords` 在借用 `&self` 期间顺序访问 `Arc<Vec<_>>`，不复制行；回调不得保留超出调用期的行切片。所有权型 `FilterClusterServerInfo` 会消费原列表并复用符合条件的节点值，不维护跨调用资源。

## 与 Go 版本的对应关系

保持一致的核心语义：

- `table_id_offset` 对齐 Go `tableIDMap` 中当前移植表的固定偏移；例如存储类迁移表为 `INFORMATION_SCHEMA_DB_ID + 102`，集群索引用量表为 `+94`。
- `columnInfo` 到模型字段的类型、UTF8MB4/binary 选择、Blob 长度、decimal、flag、默认值和注释转换基本对应 Go `buildColumnInfo`；`buildTableMeta` 设置同样的表名、公开状态及默认字符集/排序规则。
- `GetShardingInfo` 的分支顺序和输出文本、版本格式化、TiFlash 标签精确匹配、过滤的空集合语义均有 Rust 测试与 Go 源码相互印证。
- `infoschemaTable` 的可见/隐藏列、元数据、物理 ID 和拒绝写入语义对应 Go 的只读虚拟表。

尚未等价或刻意简化的事实：

- Go `tableNameToColumns` 为几乎所有表提供专用列数组；Rust `default_columns` 只覆盖一部分目录/权限/约束/诊断表和两个大列集，其他项落入 `fallback_columns`。
- Go `columnInfo` 还支持 Enum 元素，Go `buildTableMeta` 会为特殊集群慢日志主键建立 handle/index 语义；Rust `ColumnType`/`buildTableMeta` 没有这两部分完整逻辑。
- Go `GetClusterServerInfo` 从 session/store/infosync 获取真实拓扑，带 failpoint，并可并发解析地址；Rust 用可注入的 `ServerDiscovery`，自身不接 PD、etcd 或 session context，且顺序执行。
- Go 的回环处理使用 `net.ResolveTCPAddr`；Rust 只解析 IP 字面量和特殊名称 `localhost`。两者对一般 DNS 名称和复杂地址的行为可能不同。
- Go `infoschemaTable` 实现完整 `table.Table` 方法集并区分 cluster table 类型；Rust 本地 `infoschemaTable` 只提供所列简化方法，而 `createInfoSchemaTable` 实际直接调用另一个轻量 `Table::new`，没有构造该私有结构。
- Go 文件还包含 session 变量/连接属性、诊断 gRPC、SEM 可见性、TiFlash 计数等大量逻辑，Rust 本文件没有对应实现，不能从同文件名推断已移植。

## 扩展指南

- **新增 INFORMATION_SCHEMA 表：** 同步增加表名常量、`TABLE_NAMES` 项、与 Go 保持一致且不复用的 `table_id_offset`，并在 `default_columns` 增加真实列定义。若只注册名称而让它落入 `fallback_columns`，只是占位，不是功能完成。同步扩展独立的 [`tables_test.rs`](tables_test.rs)，验证 ID、列名/类型/标志；不要把测试写进生产文件。
- **修改列定义：** 对照 Go [`tables.go`](tables.go) 的对应 `[]columnInfo`，检查次序、类型、长度、decimal、flags、default、comment。大列集应延续 `catalog_columns_go45` 的拆分方式，避免进一步膨胀主文件；并在 [`go_merge_45_test.rs`](go_merge_45_test.rs) 或 [`tables_test.rs`](tables_test.rs) 增加精确回归断言。
- **扩展类型系统：** 修改 `ColumnType` 后必须同步 `buildTableMeta` 的 MySQL 类型映射、字符串字符集判定和长度规则；若增加 Enum，还需设计元素如何进入 `model_dependency::ColumnInfo`，不能只加枚举分支。
- **改变存储类可见性：** 保持“缓存完整快照、过滤调用方克隆”的不变量，并覆盖 true/false 交替调用以及 `Builder` 的 V1/V2 接线测试。
- **实现真实节点发现：** 优先新增独立的 `ServerDiscovery` 实现，而不是把网络 I/O 塞入聚合函数；明确超时、重试、并发限制和部分失败策略。若追求 Go 等价，还需补 DNS 地址解析和相应跨平台测试。
- **扩展虚拟表接口：** 先确认应修改本文件私有 `infoschemaTable` 还是 `crate::infoschema::Table`；当前 `createInfoSchemaTable` 返回后者。新增写接口必须尊重 INFORMATION_SCHEMA 只读契约。
- **改变分片文本：** 同步 Go `GetShardingInfo`、系统库判定和 [`test/clustertablestest/tables_test.rs`](test/clustertablestest/tables_test.rs) 的用户可见结果，避免破坏客户端解析或兼容性。
- 性能风险主要来自注册表/快照重复构造、超大列向量克隆和串行拓扑发现；兼容风险主要来自稳定表 ID、列顺序/类型以及精确输出文本。新增逻辑应优先扩展现有独立测试文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点，目标 `pkg/infoschema/tables.rs` 已索引为 92 个符号；`files --filter` 命中目标；`node --file` 阅读了 1–1165 行；`query` 定位了 `table_registry`、`buildTableMeta`、`information_schema_db`、`information_schema_db_with_storage_class`、`GetClusterServerInfo`、`GetShardingInfo`。图的批量 `callers/callees` 查询超时，故没有把未返回的边当作证据。
- 已读生产文件：[`tables.rs`](tables.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`builder.rs`](builder.rs) 的直接调用段、[`catalog_columns_go45.rs`](catalog_columns_go45.rs) 的直接列定义辅助，以及直接 Rust 引用搜索结果。
- 已读测试：[`tables_test.rs`](tables_test.rs)；[`go_merge_45_test.rs`](go_merge_45_test.rs) 中注册 ID、字段转换、存储类开关和大列集测试；[`test/clustertablestest/cluster_tables_test.rs`](test/clustertablestest/cluster_tables_test.rs) 及相关 harness/测试的直接调用位置。
- 已读 Go 对照：[`tables.go`](tables.go) 中 `tableIDMap`、`columnInfo`、`buildColumnInfo`、`buildTableMeta`、`GetShardingInfo`、`ServerInfo`、发现/版本/标签/过滤函数、`tableNameToColumns`、`infoschemaTable`；[`tables_test.go`](tables_test.go) 中存储类和 TiFlash 标签测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时执行任务规定的 11 章节结构命令，并人工复核只新增本说明、不改 Rust/Go/Cargo/`plan.md`。仓库说明引用的 `.agents/skills/tidb-verify-profile` 在当前 checkout 不存在，因此 Ready 以本任务明确给出的文档结构验证和 diff 自审为准，该缺失不影响文档事实核验。

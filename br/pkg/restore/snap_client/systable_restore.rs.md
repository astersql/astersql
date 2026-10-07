# [`br/pkg/restore/snap_client/systable_restore.rs`](systable_restore.rs)

## 文件定位

该文件属于 `astersql-br-pkg-restore-snap-client` library crate；crate 入口 `br/pkg/restore/snap_client/lib.rs` 以 `pub mod systable_restore` 装载它，并通过 `pub use systable_restore::*` 扁平重导出公开符号。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 Go 包 `br/pkg/restore/snap_client`，且依赖说明明确当前 Rust 端在 arm64 Darwin 使用本地 trait/stub，而不是直接连接 `kv/domain/kvproto/grpcio`。

在恢复主链中，本文件是“系统表特殊规则与表替换辅助层”，不是完整的快照恢复控制器。直接生产调用位于 `pipeline_items.rs`：`SnapClient::replaceTables` 用 `TemporaryTableChecker` 筛选临时表，随后依次调用 `updateStatsTableSchema`、`GenerateMoveRenamedTableSQLPair`/`moveRenamedTable` 和 `notifyUpdateAllUsersPrivilege`；`updateTemporaryUserTable` 调用 `removeUserResourceGroup`。`client.rs` 的 `SnapClient` 则持有恢复配置及数据库、Domain 等可注入资源。

## 核心职责

1. 用表集合定义三类策略：统计表（`stats_tables`）、可物理重命名的系统表（`renameable_sys_tables`）、禁止恢复的 schema/表（`unrecoverable_schema`、`unrecoverable_table`）；另以 `plan_replayer_tables` 标识计划回放内部表。
2. 识别 `__TiDB_BR_Temporary_*` 临时 schema 中允许参与物理替换的表。`TemporaryTableChecker` 的两个布尔开关分别控制统计表和可重命名系统表，未开启的类别不会被选中。
3. 为物理替换生成两段式 `RENAME TABLE`：先把目标正式表移到带 `restore_ts` 的删除名，再把临时表移入正式 schema。
4. 替换前后执行系统表专项修正：通过 schema 更新函数表对齐统计元数据；从临时 `mysql.user` 移除尚不支持恢复的 resource group 属性；命中权限表时触发权限缓存刷新。
5. 检查备份端与目标端系统表列布局和 collation 是否允许物理装载，并限定唯一的 collation 迁移例外。

## 主要符号

- `sysUserTableName`：`mysql.user` 的统一名字常量，被兼容性检查、资源组清理及流水线逻辑复用。
- `set_of`、`nested`：把静态字符串切片转换为嵌套 `HashMap<String, HashMap<String, ()>>`；它们只是集合构造器，不保存全局状态。
- `stats_tables`、`renameable_sys_tables`、`plan_replayer_tables`、`unrecoverable_table`、`unrecoverable_schema`：策略集合的构造函数。`isStatsTable`、`isRenameableSysTable`、`isPlanReplayerTables` 和 `isUnrecoverableTable` 是相应查询入口。
- `sysPrivilegeTableMap`：返回八张权限表到用户过滤条件模板的映射；本文件内 `notifyUpdateAllUsersPrivilege` 只使用键集合判断是否刷新权限。
- `checkPrivilegeTableRowsCollateCompatibilitySQLPair`、`collateCompatibilityTables`：描述 `mysql.db`、`tables_priv`、`columns_priv` 的上游计数 SQL、按 `utf8mb4_general_ci` 分组后的计数 SQL及敏感列集合。Rust 当前文件只在列级兼容判断和测试中消费字段，未执行这两条计数 SQL。
- `SchemaVersionPairT::{UpstreamVersion, DownstreamVersion}`：把上下游 major/minor 版本格式化为 `major.minor`；当前生产调用边未在该 crate 中发现，独立测试用于监控版本表达。
- `InfoSchema::TableInfoByName`：统计表 schema 更新所需的最小可注入查询边界；避免辅助函数绑定完整 Domain。
- `updateStatsTableSchema`：仅对 `update_stats_meta_schema_function_map` 中存在处理器的表工作，分别查询正式表与 `TemporaryDBName(schema)` 下的临时表，然后执行对应更新函数。
- `TemporaryTableChecker`、`NewTemporaryTableChecker`、`CheckTemporaryTables`：组合两个物理装载开关并返回剥离临时前缀后的原 schema 名。
- `GenerateMoveRenamedTableSQLPair`：生成批量表交换 SQL。参数名沿用 Go，但实际既可接收统计表也可接收 `replaceTables` 收集到的所有可替换表。
- `removeUserResourceGroup`：执行 `JSON_REMOVE(User_attributes, '$.resource_group')`；仅把旧版本缺少 `User_attributes` 列视为可降级警告。
- `CheckSysTableCompatibility`：对输入的上下游 `TableInfo` 切片逐表检查列名、列数和 collation，返回 `Ok(true)` 表示可物理装载，`Ok(false)` 表示逻辑兼容但必须退回非物理路径。

## 执行流程

生产路径可由 `pipeline_items.rs::SnapClient::replaceTables` 复核：

1. 用 `TemporaryTableChecker::new(load_stats_physical, load_sys_table_physical)` 固化本次恢复允许物理装载的类别。
2. `filterAndValidateTemporaryTables` 对已创建表调用 `CheckTemporaryTables`。识别函数先由 `StripTempDBPrefix` 还原原 schema，再检查表是否在对应策略集合中；命中项聚合为 `renamed_tables`。
3. 集合为空时直接返回零。否则 `updateTemporaryUserTable` 在包含 `mysql.user` 时调用 `removeUserResourceGroup`，避免把暂不支持的资源组元数据带入目标集群。
4. `updateStatsTableSchema` 遍历集合；没有注册 schema 更新函数的表被跳过，有处理器的表读取正式/临时两份 `TableInfo` 并执行修正 SQL。
5. `moveRenamedTable` 调用 `GenerateMoveRenamedTableSQLPair`，对每张表生成“正式表 -> 删除备份名”和“临时表 -> 正式表”两项，并作为一条 `RENAME TABLE` 交给数据库执行。
6. `notifyUpdateAllUsersPrivilege` 检查是否替换了任一 `mysql` 权限表；命中后只调用一次 notifier，并立即结束扫描。

兼容性路径独立于上述替换函数：`CheckSysTableCompatibility` 为每张上游表按小写表名寻找下游表；普通表要求列数相等且每个下游列在上游存在，`mysql.user` 允许目标端新增列但把 `can_load` 降为 `false`，反向缺列仍报错。每对列的 collation 必须相等，或在启用 `collation_check` 时满足 `utf8mb4_bin -> utf8mb4_general_ci` 且列属于三张权限表的白名单；即使允许该迁移，返回值仍为 `false`，从而阻止物理装载。

## 数据与状态

策略数据由函数按调用即时构造 `HashMap`/`HashSet`，没有惰性全局变量、缓存或跨调用可变状态。其代价是每次判断都会重新分配对应集合；当前实现优先保持与 Go 字面集合清晰对齐，而不是优化查询成本。

`TemporaryTableChecker` 只含 `loadStatsPhysical`、`loadSysTablePhysical` 两个不可隐藏的布尔字段；检查过程是只读的。`SchemaVersionPairT` 是四个 `i64` 字段的值对象。`checkPrivilegeTableRowsCollateCompatibilitySQLPair` 持有静态 SQL/列名引用及拥有的 `HashSet`。

`renamed_tables` 的外层键是原 schema，内层键是表名，空元组表示集合成员。`GenerateMoveRenamedTableSQLPair` 遍历 `HashMap`，因此多表 SQL 的文本顺序不稳定；语义不能依赖顺序，测试也只检查必要片段。空集合会产生 `RENAME TABLE `，但生产调用者在集合为空时提前返回。

## 依赖与调用关系

- 上游装配：`lib.rs -> systable_restore`，并把公开 API 重导出到 crate 根。
- 生产调用：`pipeline_items.rs::replaceTables -> TemporaryTableChecker::CheckTemporaryTables -> GetDBNameIf* -> is*Table`；随后调用 `updateStatsTableSchema`、`GenerateMoveRenamedTableSQLPair`、`notifyUpdateAllUsersPrivilege`。`pipeline_items.rs::updateTemporaryUserTable -> removeUserResourceGroup`。
- 测试/兼容出口：`export_test.rs` 重导出 `NewTemporaryTableChecker`、`NotifyUpdateAllUsersPrivilege`；`systable_restore_test.rs` 与 `parity_test.rs` 直接验证分类、SQL 生成和兼容性行为。
- 下游本地依赖：`stubs.rs` 提供 `Error`/`Result`、临时库名前缀操作、系统 schema 常量、日志、`model::TableInfo` 及错误类别；`systable_schema_update.rs::update_stats_meta_schema_function_map` 决定哪些统计表需要 schema 修正。
- crate 边界：`Cargo.toml` 声明本 crate 依赖 BR utils/errors/restore 以及 serde/sha2；本文件没有直接使用外部异步运行时或网络客户端。

RustCodeGraph 的文件边显示目标文件直接被 `systable_restore_test.rs` 使用；精确文本引用还确认生产调用集中在 `pipeline_items.rs`。宽泛 `callers/callees` 命令在本仓库索引上超时，因此调用边由已索引文件节点和引用搜索交叉验证。

## 错误处理与边界

- `updateStatsTableSchema` 对下游表查询、临时上游表查询和更新函数错误分别追加含 schema/table 的上下文后立即返回；未注册更新函数不是错误，而是显式跳过。
- `notifyUpdateAllUsersPrivilege` 在 notifier 失败时记录人工执行 `FLUSH PRIVILEGES` 的警告，并包装为 `berrors::ErrUnknown`；没有权限表或 schema 不是 `mysql` 时无副作用成功返回。
- `removeUserResourceGroup` 依赖错误消息子串识别旧版本缺列。只有精确包含 `Unknown column 'User_attributes' in 'field list'` 才降级为警告，其余执行错误原样传播；这与 Go 源码中的 FIXME 一样脆弱，修改驱动错误文本时需同步验证。
- `CheckSysTableCompatibility` 对缺表、普通表列数不等、列缺失或不允许的 collation 组合返回错误。`mysql.user` 的特殊规则是不要求列数相等：目标新增列导致 `Ok(false)`，备份存在而目标缺失的列仍失败。
- Rust 版本的兼容性检查只比较列名和 `Collate`，没有 Go 版 `utils.IsTypeCompatible` 的字段类型检查；调用者不得据此宣称与 Go 的完整兼容性判定等价。
- Rust 文件未实现 Go 同文件中的 `RestoreSystemSchemas`、`restoreSystemSchema`、逐表 `REPLACE INTO`/`RENAME`、临时库清理及权限表行级重复检测执行路径。这些能力不能从 SQL 常量存在推断为已接线。

## 并发与资源生命周期

本文件自身不创建线程、任务、channel、锁或事务，也不持有数据库连接。所有外部副作用都通过同步闭包/trait 注入：`InfoSchema` 要求 `Send + Sync`，查询对象可安全跨并发边界共享；`execution` 是 `FnMut`，表示多条更新按当前遍历顺序串行复用；权限 notifier 是 `FnOnce`，从类型上保证本次调用最多消费一次。

真正的数据库资源属于 `SnapClient` 和调用方。`replaceTables` 的阶段顺序形成重要生命周期约束：先修正临时用户/统计表，再执行表交换，最后刷新权限。任一步失败都会用 `?` 中止后续阶段，但本文件不提供回滚事务；尤其 `RENAME TABLE` 后 notifier 失败时，表交换已经发生，错误只通知上层处理。`GenerateMoveRenamedTableSQLPair` 也不负责 quoting 或事务边界，输入必须来自受控 schema/table 元数据。

## 与 Go 版本的对应关系

集合、临时库识别、两段式重命名、资源组清理、权限刷新触发条件、版本字符串和 collation 白名单均直接对应 `systable_restore.go` 的同名符号。Rust 独立测试 `systable_restore_test.rs` 复现了 Go 测试的重要边界：`mysql.user` 新旧列差异、列顺序无关、唯一允许的 collation 迁移方向、临时前缀判定、两个装载开关、不可恢复表、重命名 SQL和权限刷新错误。

当前迁移不是一比一完整复刻：

- Go `CheckSysTableCompatibility` 从 Domain 和备份元数据中筛选权限表，并同时调用 `IsTypeCompatible` 检查类型与 collation；Rust 接收已准备好的上下游 `TableInfo` 切片且只检查 collation。
- Go `checkPrivilegeTableRowsCollateCompatibility` 校验列集合后执行两条受限 SQL，用计数差识别在目标 collation 下折叠的重复权限行；Rust 仅保留 SQL/列集合和列级白名单，没有对应执行方法。
- Go 在同文件中实现完整 `RestoreSystemSchemas`、逐表合并、特殊表跳过、后处理和临时库清理；Rust 的运行接线集中在简化的 `pipeline_items.rs::replaceTables`，目标文件本身没有这些入口。
- Rust 用本地 `InfoSchema` trait、同步闭包和 stub `model` 降低依赖；Go 直接使用 `domain.Domain`、真实 InfoSchema、session 和 context。

因此修改 Rust 时应以 Go 同名逻辑为语义基线，但必须明确“已移植”“简化接线”“尚未移植”三种状态，不能用 Go 行为填补 Rust 当前缺口。

## 扩展指南

- 新增/删除系统表分类时，同时更新对应集合函数、Go 同路径集合以及 `systable_restore_test.rs`；若影响流水线物理装载，还要检查 `pipeline_items_test.rs`。不可恢复表应注明包含集群局部 ID、TSO 或运行态数据等原因。
- 新增统计 schema 迁移时，在 `systable_schema_update.rs::update_stats_meta_schema_function_map` 注册处理器，并为正式/临时表查询失败和生成 SQL补独立测试；不要把测试逻辑内嵌进生产文件。
- 扩大 collation 例外前必须同时评估列级兼容和行级唯一性。当前 Rust 没有执行 Go 的计数 SQL，若移植该能力，应通过可注入 restricted-SQL executor 实现，并覆盖空结果、执行错误、计数不等和带重音/大小写冲突。
- 修改 `CheckSysTableCompatibility` 时优先补齐 Go 的类型兼容语义，而不是继续简化；回归测试至少覆盖字段类型、普通表列数、`mysql.user` 双向缺列及 `can_load=false` 的降级含义。
- 调整重命名逻辑时关注标识符 quoting、空集合、多表顺序、目标备份名冲突和执行失败后的恢复策略。若需要确定性 SQL，应先改用排序后的键序列并同步 Go/测试，而不是依赖 `HashMap` 迭代顺序。
- 修改错误分类或旧版本容错时避免继续依赖错误字符串；若引入结构化错误，需保持“仅缺 `User_attributes` 可忽略”的兼容边界。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/restore/snap_client` 确认目标、Go 对照及独立测试均被索引；`explore` 给出目标符号、测试调用和 Go `RestoreSystemSchemas -> restoreSystemSchema` 调用边；`node --file` 完整读取目标 595 行、Rust 测试 394 行，并读取 `pipeline_items.rs` 的直接生产调用段。
- Rust 源与装配：`br/pkg/restore/snap_client/systable_restore.rs`、`lib.rs`、`pipeline_items.rs`、`client.rs`、`systable_schema_update.rs`（由目标导入与调用确认）。
- crate 配置：`br/pkg/restore/snap_client/Cargo.toml`，确认 library 入口、Go 包映射、依赖和本地 stub 边界。
- Rust 测试：`br/pkg/restore/snap_client/systable_restore_test.rs`；另由引用搜索确认 `pipeline_items_test.rs` 和 `parity_test.rs` 覆盖相关接线/跨语言奇偶性。
- Go 对照：`br/pkg/restore/snap_client/systable_restore.go`、`systable_restore_test.go`，用于核对集合、完整系统表恢复流程、类型/列/collation 兼容规则及权限行冲突检查。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构命令，并人工检查上述事实边界、调用顺序和未移植能力均有直接代码依据。

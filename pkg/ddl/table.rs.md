# `pkg/ddl/table.rs`

## 文件定位

`pkg/ddl/table.rs` 属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定 crate 入口，`pkg/ddl/lib.rs` 以 `pub mod table;` 公开本模块。文件提供一个只依赖标准库 `BTreeMap`/`BTreeSet` 的表级 DDL 内存模型，覆盖表目录、删除与恢复、截断、重命名，以及一组表属性更新函数。

这个文件不是当前 Rust DDL 持久化 Job 框架的完整实现：定向调用搜索只在独立 Rust 测试中发现 `TableCatalog` 和这些属性函数的使用，没有发现 `pkg/ddl/executor.rs`、`pkg/ddl/job_worker.rs` 等生产模块调用它们。完整应用中对应的 Go 主链是 `pkg/ddl/executor.go` 构造 `model.Job`，`pkg/ddl/job_worker.go:runOneJobStep` 按 `ActionType` 分派，最终由 `pkg/ddl/table.go` 的处理函数修改持久化元数据。因此，本文件应理解为已公开、可直接运行并用于语义回归的简化模型，而不是已经替代 Go DDL 主链的实现。

## 核心职责

- 用 `TableState`、`TableInfo`、`TiFlashReplica` 和 `ForeignKeyReference` 表达与表级 DDL 有关的最小状态集合（`pkg/ddl/table.rs:24-82`）。
- 用 `TableCatalog` 管理按 schema 和小写表名索引的内存目录，并保留可恢复的已删除表快照（`pkg/ddl/table.rs:147-515`）。
- 实现删除状态迁移、GC safe point 约束下的恢复、截断换 ID、单表与批量重命名等目录操作。
- 实现 auto ID、shard bits、注释、字符集、TiFlash、版本、placement、attributes、cache、affinity、Region split policy 等局部属性变更（`pkg/ddl/table.rs:517-702`）。
- 以 `TableError` 提供有限、可比较的错误分类，便于独立测试断言；它没有承载 Go 版本的错误码、堆栈包装、Job 状态或事务错误上下文。

## 主要符号

- `TableState::{Public, WriteOnly, DeleteOnly, None}`：本文件可见性状态。`drop_table_step` 只实现 `Public → WriteOnly → DeleteOnly → None`；它没有表示 Go 模型中的全部 schema state。
- `TableInfo`：简化元信息。关键字段包括物理 ID/分区 ID、当前 schema/name/state、auto ID、历史最大 shard bits、外键目标名称、TiFlash 可用物理 ID、版本和若干表属性。
- `TableError`：错误枚举，区分目录冲突/缺失、名称过长、恢复冲突、非法 ID 或属性、分区不存在、版本回退以及 GC safe point 过新。
- `GcController`：保存 `enabled` 与 `safe_point`；`disable_for_recovery` 先拒绝早于 safe point 的快照，再关闭 GC，`restore` 恢复原开关。
- `TableCatalog`：`schemas` 是 `schema_id → lowercase(table name) → TableInfo`；`dropped` 是 `table_id → DroppedTable`。两字段均为私有，外部必须通过方法维护不变量。
- 目录方法：`create_schema`、`drop_schema`、`schema_exists`、`table_names`、`insert`、`get`、`get_mut`、`drop_table_step`、`recover_table`、`truncate_table`。
- 重命名方法：公开的 `rename_table`/`rename_table_checked`/`rename_tables`/`rename_tables_checked`，以及私有执行核心 `rename_table_inner`/`rename_tables_inner`。`RenameMode` 区分 `RENAME TABLE` 与 `ALTER TABLE ... RENAME` 的校验和错误优先级。
- 独立属性函数：`rebase_auto_increment`、`rebase_auto_random`、`alter_auto_id_cache`、`alter_shard_row_id_bits`、`alter_comment`、`alter_charset_and_collation`、`set_tiflash_replica`、`update_tiflash_replica_status`、`update_table_version`、`alter_placement`、`alter_attributes`、`alter_cache`、`alter_affinity`、`alter_region_split_policy`、`table_physical_ids`。

## 执行流程

1. 目录建立与查找：`create_schema` 显式建立空 schema；`insert` 则会用 `entry(...).or_default()` 隐式建立目标 schema。所有表名键在插入、读取和删除时都转成 ASCII 小写，所以查找大小写不敏感，但 `TableInfo.name` 保留原始拼写。
2. 删除：调用方反复调用 `drop_table_step`。前三次分别把 `Public` 推进到 `WriteOnly`、`DeleteOnly`、`None`；到 `None` 时从活动目录移除表，并以表 ID 把表快照和 `drop_ts` 放入 `dropped`。若初始状态已是 `None`，同样执行移除。
3. 恢复：`recover_table` 从 `dropped` 克隆快照，以 `drop_ts` 调用 `GcController::disable_for_recovery`；目标名称冲突时恢复 GC 原状态并返回 `RecoveryConflict`，成功时删除 dropped 快照、把表状态改回 `Public`、插回原 schema，最后恢复 GC 开关。
4. 截断：`truncate_table` 返回旧表 ID 加旧分区 ID，换成调用者传入的新表/分区 ID，清零 auto-increment/auto-random，清空 TiFlash 已就绪分区集合，并以饱和加法推进版本。
5. 单表重命名：checked 入口先执行模式相关校验，再进入 `rename_table_inner`。执行核心从旧目录取出表，维护跨 schema 的 `auto_id_schema_id`，更新 schema/name/version，插入新目录，并扫描全部表修正匹配旧表名的外键引用。
6. 批量重命名：`rename_tables` 先克隆整个目录，在克隆上校验并执行，全部成功后才以 `*self = staged` 提交，因此错误不会留下部分结果。内部先检查目标二元组不重复以及非源目标未被占用，再一次性取出所有源表，从而支持循环换名，最后逐表写回并修正外键。
7. 属性更新：文件末尾函数均在传入的 `&mut TableInfo` 上同步执行。返回 `bool` 的函数用它表示值是否变化；返回 `Result<bool, TableError>` 的函数同时表达校验失败；`set_tiflash_replica` 用 `count == 0` 表示清除配置。

## 数据与状态

- 名称索引不变量：目录键始终是 `to_ascii_lowercase()` 结果；原始显示名保存在 `TableInfo.name`。这只保证 ASCII 大小写折叠，不等价于完整 SQL 标识符校对规则。
- 删除快照：活动目录和 `dropped` 之间以“移出/移回”方式转移所有权。重复完成删除后活动目录中已无表，下一次调用会返回 `NotFound`。
- auto-ID 归属：表首次跨库移动时把原 schema 写入 `auto_id_schema_id`；移回该 schema 时清零。该字段只记录归属关系，本文件不包含实际 allocator 或持久化 rebase。
- 版本：截断和重命名使用 `saturating_add(1)`；`update_table_version` 允许相等（返回未变化），拒绝回退。其他属性函数通常不自动推进版本，这与 Go handler 统一调用 `updateVersionAndTableInfo` 的生产流程不同。
- TiFlash：`available_partition_ids` 是有序集合。非零副本数更新会保留既有可用集合；清零副本数会移除整个配置。状态更新只接受表 ID 或当前 `partition_ids` 中的 ID。
- 物理 ID：`table_physical_ids` 在无分区时返回表 ID，有分区时只返回分区 ID，不把逻辑表 ID 混入分区列表。
- 外键：模型只保存目标 schema/name 字符串；重命名修正逻辑按旧表名全局匹配，没有同时按旧 schema 限定，这是简化边界，扩展时不可直接假定等价于 Go 的完整外键元数据处理。

## 依赖与调用关系

- 直接依赖只有 `std::collections::{BTreeMap, BTreeSet}`；`pkg/ddl/Cargo.toml` 的大量 workspace 依赖没有被本文件直接导入。
- crate 装配边是 `pkg/ddl/lib.rs → pub mod table`，外部测试可通过 `astersql_ddl::table::*` 使用公开 API，例如 `pkg/ddl/tests/serial/serial_test.rs`。
- 已核实的 Rust 调用者主要是测试：`pkg/ddl/table_test.rs`、`pkg/ddl/db_integration_test.rs`、`pkg/ddl/db_change_test.rs`、`pkg/ddl/db_rename_test.rs`、`pkg/ddl/attributes_sql_test.rs`、`pkg/ddl/table_rename_aster_unit_test.rs`、`pkg/ddl/table_split_test.rs`，以及 `pkg/ddl/tests/tiflash/ddl_tiflash_test.rs`。
- 代表性内部调用边包括：`rename_table → rename_table_inner`、`rename_table_checked → get_mut/rename_table_inner`、`rename_tables → clone + rename_tables_inner`、`rename_tables_checked → schema_exists + rename_tables`、`truncate_table → get_mut`、`recover_table → GcController::disable_for_recovery/restore`。
- RustCodeGraph 的文件节点确认 `pkg/ddl/table.rs` 有 65 个符号，但对关键符号执行 `callers`/`callees` 没有返回调用边；因此上述跨文件调用关系以定向 Rust 源码引用搜索核实，并明确不宣称存在未找到的生产调用。
- Go 生产链的直接证据是 `pkg/ddl/job_worker.go:runOneJobStep`：它把 Drop/Truncate/Rename/Recover/Rebase/Shard/TiFlash/Placement/Cache/Affinity 等 Action 分派到 `pkg/ddl/table.go` 相应 handler。Rust 本文件没有 Job、owner、事务 meta mutator、schema diff 或 schema sync 接口。

## 错误处理与边界

- 所有失败均为同步返回 `TableError`；错误值实现 `Display` 时直接输出 Debug 枚举名。没有错误源链、重试分类或 SQL 错误码映射。
- `insert` 会隐式创建 schema，而 `rename_table_checked`/`rename_tables_checked` 要求目标 schema 已存在；未校验的 `rename_table` 则可能通过 `entry(...).or_default()` 创建目标 schema。调用方应根据是否需要 SQL 语义选择 checked 入口。
- `rename_table_inner` 先从源目录移除表，后向目标目录写入；公开未校验入口若在移除后的逻辑新增可失败步骤，可能破坏原子性。当前实现移除之后没有返回错误的操作；批量入口则用完整 clone 保证失败原子性。
- `RenameMode::RenameTable` 先检查目标占用，再检查源和目标 schema；`AlterTable` 先检查源，允许同 schema、同折叠键的大小写变更。这种错误优先级由 `pkg/ddl/table_rename_aster_unit_test.rs` 覆盖。
- 表名上限按 Unicode `chars().count()` 检查 64 个字符；目录大小写折叠仍只使用 ASCII 规则。
- `rebase_auto_increment` 拒绝负数；非 force 且新值不大于当前值时返回 `Ok(false)`。`rebase_auto_random` 对回退和负数都返回 `InvalidAutoId`。
- `alter_shard_row_id_bits` 上限固定为 15，降低当前值不会降低历史最大值。`alter_charset_and_collation` 只检查校对名是否以 `charset_` 开头（或 binary 特例），不是完整字符集注册表校验。
- `update_tiflash_replica_status` 在无配置时返回 `InvalidReplicaCount`，未知物理 ID 返回 `PartitionNotFound`，重复设置则返回 `Ok(false)`。Go handler 对“状态已经更新”会取消 Job 并报错，这是明确的语义差异。
- 恢复只在目标名冲突分支和成功分支显式恢复 GC；safe point 校验失败发生在关闭 GC 前，因此无需恢复。函数同步执行且没有 panic 恢复保护，未来若在关闭 GC 后增加可 panic/提前返回路径，必须引入 guard 式清理。

## 并发与资源生命周期

- `TableCatalog` 和 `GcController` 本身没有锁、原子变量、异步任务或通道；所有变更要求独占 `&mut`，并发策略由调用者提供。`pkg/ddl/db_rename_test.rs` 和 `pkg/ddl/db_integration_test.rs` 在并发场景中使用 `Arc<Mutex<TableCatalog>>`，说明锁位于模型外层。
- `rename_tables` 的 staging clone 是事务式提交边界，但不是并发事务或持久化事务；它会复制完整目录，时间和内存成本随所有 schema/表数量增长。
- `recover_table` 暂时改变共享 `GcController.enabled`。在本同步模型中关闭和恢复处于同一调用栈；它没有 RAII guard，也没有与真实 GC service 协调。
- `dropped` 快照只驻留在 `TableCatalog` 生命周期内；没有落盘、history job、delete-range GC 或重启恢复能力。
- 与 Go 生产实现相比，本文件不承担 owner 选举、Job 重试/回滚、schema version 同步、MDL、PD rule 更新、TiFlash store 校验或外部 TTL workload 注册。因此这些系统生命周期不能从本文件的成功返回推导出来。

## 与 Go 版本的对应关系

- `drop_table_step` 对应 `pkg/ddl/table.go:(*worker).onDropTableOrView` 的状态迁移意图，但 Rust 只保留 `Public → WriteOnly → DeleteOnly → None` 和 dropped 快照；Go 还通过 meta 事务、schema version、Job 状态、依赖约束和后续 GC 机制推进流程。
- `recover_table` 对应 `(*worker).onRecoverTable` 的“GC safe point 允许后恢复元信息”核心约束；Go 使用恢复参数、meta mutator、placement/TTL 等外围协调，Rust 只处理一个内存 GC 开关和名称冲突。
- `truncate_table` 对应 `(*worker).onTruncateTable` 的换表/分区 ID、重置 auto ID 和 TiFlash availability 意图。Go 还更新 schema diff、处理外键/placement/TTL、delete range 和 schema sync。
- 重命名方法对应 `(*worker).onRenameTable`、`(*worker).onRenameTables`、`checkAndRenameTables` 及 `adjustForeignKeyChildTableInfoAfterRenameTable`。Rust 保留循环批量换名、外键名称修正和 auto-ID schema 归属，但没有两次 schema version 更新、持久化事务和完整的 involving-schema 信息。
- `rebase_auto_increment`/`rebase_auto_random` 对应 `onRebaseAutoIncrementIDType`、`onRebaseAutoRandomType`、`onRebaseAutoID`。Go 会操作真实 allocator；Rust 只更新字段。非 force 行为也不同：Go 可调整到下一全局 ID 并记录 warning，Rust 对不增长直接返回 `false`。
- `alter_shard_row_id_bits`、`alter_comment`、`alter_charset_and_collation` 分别对应 `onShardRowID`、`onModifyTableComment`、`onModifyTableCharsetAndCollate` 的局部字段语义；Go 有更完整的合法性校验、列级 charset 更新和 Job 可回滚性处理。
- TiFlash 函数对应 `(*worker).onSetTableFlashReplica` 和 `onUpdateTiFlashReplicaStatus`。`pkg/ddl/table_test.rs` 的两个真实 Rust 单元测试专门确认历史最大 shard bits 不回退，以及非零副本数变更保留 availability；其余大量同文件测试函数是从 Go 迁移的占位说明，不能作为行为已实现的证据。
- placement/attributes/cache/affinity 的 Rust 函数只修改 `TableInfo`。Go 的 `onAlterTablePlacement`、`onAlterTableAttributes`、`onAlterCacheTable`/`onAlterNoCacheTable`、`onAlterTableAffinity` 还会验证元数据并与 PD/label/affinity 资源交互。

## 扩展指南

- 增加目录级行为时，优先在 `TableCatalog` 上提供 checked 公共入口，把实际搬迁封装在私有 helper；涉及多表的操作应继续保留“克隆试跑后提交”或等价的原子策略。
- 修改名称语义时同步检查小写键、原始 `TableInfo.name`、外键引用和 `auto_id_schema_id`。跨库、仅大小写改名、循环批量换名、目标占用与错误优先级至少要同步更新 `pkg/ddl/table_rename_aster_unit_test.rs` 和 `pkg/ddl/db_rename_test.rs`。
- 修改删除/恢复/截断时同步更新独立测试 `pkg/ddl/attributes_sql_test.rs`、`pkg/ddl/db_change_test.rs`、`pkg/ddl/db_integration_test.rs` 和 `pkg/ddl/tests/serial/serial_test.rs`；测试代码保持在独立文件，不嵌入本源文件。
- 修改 TiFlash 或物理 ID 语义时同步更新 `pkg/ddl/table_test.rs`、`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs` 和 `pkg/ddl/table_split_test.rs`，特别关注分区 ID、重复状态更新、清零配置和 truncate 后 availability。
- 若目标是接入完整 Rust DDL 主链，不能只扩展本模型；还需明确 Job 持久化、owner/worker 分派、schema diff/version sync、回滚/取消、GC/delete-range、MDL，以及 PD/TTL/TiFlash 外部资源的生命周期。这属于本文件当前能力之外，应单独设计和验证。
- 性能风险主要在全目录 clone（批量 rename）和全目录外键扫描（每次 rename）；表数量扩大后应考虑仅复制/扫描受影响 schema 或维护反向外键索引，但优化必须保持失败原子性和循环换名语义。
- 新增 `TableError` 时应同时决定它与 Go 错误优先级、SQL 错误码及 Job cancel/rollback 状态的对应关系，避免测试只匹配枚举而掩盖生产差异。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/ddl/table.rs` 确认目标文件已索引；`node --file pkg/ddl/table.rs` 分段读取 702 行及 65 个符号；`query TableCatalog`、`query set_tiflash_replica` 定位符号。对 `TableCatalog`、`set_tiflash_replica` 等执行 `callers`/`callees` 未得到调用边，文档已按“图未覆盖”处理而未臆测。
- Rust 源与装配：`pkg/ddl/table.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- Rust 独立测试：重点读取 `pkg/ddl/table_test.rs`；并通过定向引用核对 `pkg/ddl/table_rename_aster_unit_test.rs`、`pkg/ddl/db_rename_test.rs`、`pkg/ddl/db_change_test.rs`、`pkg/ddl/db_integration_test.rs`、`pkg/ddl/attributes_sql_test.rs`、`pkg/ddl/table_split_test.rs`、`pkg/ddl/tests/serial/serial_test.rs`、`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs`。
- Go 对照：`pkg/ddl/job_worker.go:runOneJobStep` 的 Action 分派，`pkg/ddl/table.go` 的 `onDropTableOrView`、`onRecoverTable`、`onTruncateTable`、`onRebaseAutoID`、`onShardRowID`、`onRenameTable`、`onRenameTables`、`onModifyTableComment`、`onModifyTableCharsetAndCollate`、`onSetTableFlashReplica`、`onUpdateTiFlashReplicaStatus`、`onAlterTableAttributes`、`onAlterTablePlacement`、`onAlterCacheTable`、`onAlterNoCacheTable`、`onAlterTableAffinity`，以及 `pkg/ddl/table_test.go`。
- 结构验收使用任务指定命令，要求文件存在且恰有 11 个固定二级标题。任务是纯文档分析，按计划不运行 Cargo，也不以 `pkg/ddl/table_test.rs` 中的占位测试函数作为运行时能力证明。

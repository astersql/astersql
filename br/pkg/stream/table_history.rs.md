# `br/pkg/stream/table_history.rs`

## 文件定位

本文件属于 `astersql-br-pkg-stream` library crate；`br/pkg/stream/Cargo.toml` 将 `lib.rs` 设为 crate 入口，`lib.rs` 再以 `#[path = "table_history.rs"] pub mod table_history` 挂载并通过 `pub use table_history::*` 扁平导出其公开项。它位于 BR 日志备份/PITR 的元数据辅助层：用内存映射记录数据库名称，以及表或分区从首次观察位置到最新观察位置的变化，供恢复过滤、重命名处理和分区交换检查读取。

该文件不是持久化层，也不解析原始 meta KV；解析职责在同 crate 的 `table_mapping.rs`。Go 主链会把历史管理器作为 `MetaInfoCollector` 交给解析器，而当前 Rust 文件尚未实现 `table_mapping::MetaInfoCollector`。此外，`br/pkg/restore/log_client/batch_meta_processor.rs` 使用的是其本地 `stubs::stream::LogBackupTableHistoryManager`，不是本文件的类型。因此，本文件目前是可独立使用并有测试覆盖的数据结构，同时被 `br/pkg/task/restore.rs` 的恢复选择逻辑消费，但尚未直接接入 Rust 的 meta KV 批处理主链。

## 核心职责

- `LogBackupTableHistoryManager` 为每个物理表 ID（普通表 ID 或分区 ID）保存固定两个 `TableLocationInfo`：首次观察值和按时间戳选出的最新值；这避免保存完整 rename/移动事件链。
- `RecordDBIdToName` 为每个数据库 ID 保存按提交时间戳选出的最新名称，并用独立的 `dbTimestamps` 阻止乱序旧事件回写。
- `AddTableHistory` 与 `AddPartitionHistory` 把普通表和分区的输入标准化为 `TableLocationInfo`，再共用私有的 `addHistory` 更新规则。
- `OnDatabaseInfo`、`OnTableInfo` 提供与 Go `MetaInfoCollector` 回调同名的便利入口；`OnTableInfo` 会为逻辑表及其 `TableSimpleInfo::PartitionIds` 中的每个分区记录同一名称和提交时间戳。
- `GetTableHistory`、`GetDBNameByID`、`GetNewlyCreatedDBHistory` 以借用形式暴露当前快照，供恢复阶段读取而不复制整个映射。

## 主要符号

- `TableLocationInfo`：一个时点的表位置快照。`DbID` 和 `TableName` 标识所在库与表名；`IsPartition` 区分普通表和分区；仅当它为真时 `ParentTableID` 才有意义；`Timestamp` 是该快照被记录时的提交时间戳。类型实现 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`。
- `LogBackupTableHistoryManager`：状态容器。三个字段均为私有：`tableNameHistory: HashMap<i64, [TableLocationInfo; 2]>`、`dbIdToName: HashMap<i64, String>`、`dbTimestamps: HashMap<i64, u64>`。
- `NewTableHistoryManager() -> LogBackupTableHistoryManager`：创建三个空映射；与 Go 版本返回指针不同，Rust 按值返回，由调用者持有并通过 `&mut self` 更新。
- `AddTableHistory(tableId, tableName, dbID, ts)`：构造 `IsPartition=false`、`ParentTableID=0` 的位置记录。
- `AddPartitionHistory(partitionID, tableName, dbID, parentTableID, ts)`：构造 `IsPartition=true` 且带逻辑父表 ID 的位置记录。
- `addHistory(id, locationInfo)`：唯一的表历史更新原语。首次出现时两个槽位相同；已有记录只在新时间戳大于或等于当前最新时间戳时替换第二槽，第一槽永久保留。
- `RecordDBIdToName(dbId, dbName, ts)`：数据库名称的时间戳门禁。不存在旧时间戳或 `ts >= existingTs` 时，同时更新名称和时间戳。
- 三个查询方法：`GetTableHistory` 返回整个双槽映射的共享引用；`GetDBNameByID` 将 `String` 借用收窄为 `Option<&str>`；`GetNewlyCreatedDBHistory` 返回整个数据库名称映射的共享引用。
- 两个回调方法：`OnDatabaseInfo` 直接委托 `RecordDBIdToName`；`OnTableInfo` 先调用 `AddTableHistory`，再逐项调用 `AddPartitionHistory`。

本文件没有模块级常量、trait、条件编译项或异步函数；除 `addHistory` 及三个状态字段外，其余类型和方法均公开。

## 执行流程

典型的预期数据流从 meta KV 解析开始：解析器得到 `(db_id, table_id, TableSimpleInfo, commit_ts)`，数据库事件进入 `OnDatabaseInfo`，表事件进入 `OnTableInfo`。数据库事件经过 `dbTimestamps` 的时间戳比较后决定是否同时覆盖 `dbIdToName`；表事件先为逻辑表 ID 生成普通表快照，再遍历分区 ID，为每个分区生成指回逻辑表 ID 的快照。

`addHistory` 对每个表/分区 ID 独立工作。第一次看到 ID 时把同一快照放入 `[0]` 和 `[1]`；以后 `[0]` 不变，只有 `Timestamp >= history[1].Timestamp` 的输入才进入 `[1]`。因此，乱序到达的更旧事件被忽略，而相同时间戳采用后到者，这一点与 Go 代码的 `>=` 完全一致。

恢复阶段的实际 Rust 消费者位于 `br/pkg/task/restore.rs::AdjustTablesToRestoreAndCreateTableTracker`：它先遍历 `GetNewlyCreatedDBHistory` 把匹配过滤条件的库加入 tracker，再遍历 `GetTableHistory`，把 `[0]` 作为起始位置、`[1]` 作为结束位置；结合快照表/分区映射决定应恢复的表，并再次遍历历史检测分区交换是否跨越恢复边界。辅助函数 `get_db_name_from_backup` 在快照库映射找不到名称时调用 `GetDBNameByID` 回退到历史名称。

当前 Rust 主链存在明确接线缺口：`br/pkg/stream/table_mapping.rs::ParseMetaKvAndUpdateIdMapping` 接受 `&mut dyn MetaInfoCollector`，但本文件没有相应 trait 实现；`br/pkg/restore/log_client/batch_meta_processor.rs` 又通过独立桩模块完成同名调用。因此不能仅凭同名 `On*` 方法断言本类型已经由 Rust 批处理器自动填充。

## 数据与状态

`tableNameHistory` 的键空间同时容纳普通表 ID 和分区 ID，调用者必须依赖值中的 `IsPartition` 解释 ID 类型。值的两个位置具有固定语义：索引 `0` 是首次到达事件，而不是全局最小时间戳；索引 `1` 是自首次到达后按 `>=` 规则选择的最新事件。这意味着如果最先到达的事件本身不是时间上最早的事件，后来到达的更旧事件不会改写“首次”槽；该行为是 Go/Rust 当前实现共同的处理顺序语义。

`dbIdToName` 与 `dbTimestamps` 构成一个逻辑整体。只有 `RecordDBIdToName` 修改它们，并在接受事件时同时写入，从而保证名称与其门禁时间戳对应。查询 API 不暴露 `dbTimestamps`，调用者只能观察最终名称。

输入名称通过 `to_string()` 复制进管理器，调用者传入的 `&str` 生命周期不进入状态。查询返回的引用依赖管理器借用期；在持有查询引用时不能再可变更新同一管理器，这是 Rust 借用规则提供的状态一致性约束。`Default` 仅作用于 `TableLocationInfo`，管理器本身通过显式构造函数初始化。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashMap`；唯一 crate 内类型依赖是 `crate::stubs::TableSimpleInfo`，该桩只包含 `Name: String` 与 `PartitionIds: Vec<i64>`。`Cargo.toml` 没有为本文件单独启用 feature；它随整个 `astersql-br-pkg-stream` crate 编译。文件自身不直接使用 crate 清单中的加密、备份元数据、正则、serde、哈希、压缩等依赖。

RustCodeGraph 对该文件给出的直接使用文件为 `br/pkg/stream/parity_test.rs`、`br/pkg/stream/table_mapping_test.rs`、`br/pkg/task/restore.rs` 和 `br/pkg/task/restore_test.rs`。内部调用边是 `OnDatabaseInfo -> RecordDBIdToName`、`OnTableInfo -> AddTableHistory`、`OnTableInfo -> AddPartitionHistory`，以及两个 `Add*History -> addHistory`。

上游方面，测试直接通过 `NewTableHistoryManager` 构造并调用更新方法；生产消费侧 `AdjustTablesToRestoreAndCreateTableTracker` 接收管理器引用，但当前索引和源码没有显示本文件类型在 Rust 批处理链中被构造、填充后传入该函数。Go 对照链则由 `br/pkg/restore/log_client/batch_meta_processor.go::MetaKVInfoProcessor` 构造管理器，并将其交给 `TableMappingManager.ParseMetaKvAndUpdateIdMapping`。

下游方面，本文件不执行 I/O，也不调用存储、网络或事务 API。恢复选择逻辑读取其历史，形成 `PiTRIdTracker`，并用起止位置判断跨库 rename、过滤命中和分区交换冲突。

## 错误处理与边界

所有 API 都是无失败返回值的内存操作，没有 `Result`、错误包装或日志。未知数据库 ID 在 `GetDBNameByID` 中返回 `None`；未知表 ID 需要调用者在 `GetTableHistory()` 返回的 map 上自行处理缺失，测试使用 `get(...).unwrap()` 仅因为样例先插入了该 ID。

空名称、零/负 ID、零时间戳不会被本文件拒绝；它们按普通值存储。是否有效由上游 meta 解析和恢复逻辑负责。空 `PartitionIds` 使 `OnTableInfo` 只记录逻辑表。重复 ID、相同时间戳允许后到值覆盖最新槽和数据库名；更旧时间戳不覆盖。普通表与分区若错误地复用同一 ID，会进入同一个历史双槽，本文件不做类型冲突校验。

内存分配可能由 `HashMap` 增长或字符串复制触发，但接口没有显式处理分配失败。读取方法直接暴露内部 map 的不可变引用，调用者不能通过这些引用破坏双槽和双 map 同步不变量；不过它们也使内部表示成为公开 API 的一部分，未来改变存储形状会影响消费者。

## 并发与资源生命周期

管理器没有 `Arc`、`Mutex`、通道、后台任务、异步状态或线程生命周期。更新全部要求 `&mut self`，读取要求 `&self`；并发共享若有需要，必须由更上层显式添加同步容器。文件本身不承诺跨线程共享，也没有锁顺序或取消语义。

资源生命周期从 `NewTableHistoryManager` 创建空状态开始，在 meta 事件或测试操作期间原地累积，恢复规划阶段以共享引用读取，最终随所有者一起释放。没有清空或压缩 API；空间上每个见过的表/分区 ID 固定保存两份位置记录，每个数据库 ID 保存一份名称和一份时间戳，所以状态规模与不同物理 ID、数据库 ID 的数量线性相关，而不是与 rename 事件总数线性相关。

## 与 Go 版本的对应关系

`br/pkg/stream/table_history.go` 是逐项对照文件。Rust 保留了 Go 的类型名、字段名和方法名，以便迁移核对；`TableLocationInfo` 五个字段、管理器三个映射、首次/最新双槽规则、数据库时间戳门禁、相同时间戳覆盖以及分区循环均与 Go 一致。`br/pkg/stream/table_mapping_test.go::TestTableHistoryManagerOutOfOrderTS` 覆盖数据库、普通表和分区的乱序事件；Rust 的 `br/pkg/stream/table_mapping_test.rs::test_table_history_manager_out_of_order_ts` 覆盖数据库和普通表乱序，`br/pkg/stream/parity_test.rs::go_rust_public_contract_matches` 还检查较新表名和库名覆盖。

语言层差异包括：Go 构造函数返回 `*LogBackupTableHistoryManager`，Rust 返回拥有值；Go 查询 map 直接返回引用类型语义，Rust 显式返回共享借用；Go `GetDBNameByID` 返回 `(string, bool)`，Rust 返回 `Option<&str>`；Rust 输入以 `&str` 接收并在写入时复制。

最关键的迁移差异是接口接线。Go 管理器通过同名方法隐式满足 `MetaInfoCollector`，可直接传给 `ParseMetaKvAndUpdateIdMapping`；Rust trait 需要显式 `impl`，当前文件没有实现，而且 trait 的数据库回调参数是拥有的 `String`，本文件固有方法接收 `&str`。同时 Rust `log_client` 仍使用独立的本地桩历史类型，其解析方法当前为空成功路径。故当前实现不能视为 Go 端到端元数据收集链的完整替代。

## 扩展指南

若新增历史字段，应先修改 `TableLocationInfo` 的构造点 `AddTableHistory`、`AddPartitionHistory` 和恢复侧依赖默认值的 `build_start_table_location_info`，再同步独立测试；字段若影响位置相等、序列化或调试输出，还需复核现有派生 trait。不要把测试内嵌进本源文件，测试应继续放在同目录独立的 `*_test.rs` 中。

若修改时间戳规则，应集中调整 `addHistory` 和 `RecordDBIdToName`，并明确处理相同时间戳的确定性；同步扩展 Rust `table_mapping_test.rs` 与 Go `table_mapping_test.go::TestTableHistoryManagerOutOfOrderTS` 对数据库、普通表、分区和首槽语义的断言。把“首次”改成“最早时间戳”属于行为变更，不能只改注释或排序输入。

若要完成 Rust 主链接线，最小入口是在 `table_mapping.rs` 所定义 trait 上为本类型实现 `MetaInfoCollector`，解决 `String` 到 `&str` 的适配，并让真实 `MetaKVInfoProcessor` 使用本 crate 类型而非 `restore/log_client/stubs.rs` 的同名桩；同时需要独立集成测试证明 meta KV 解析确实填充历史。此变更跨越本纯文档任务范围，且应评估 crate 依赖方向，避免形成循环依赖。

若新增并发写入，不应直接在本文件内部零散加锁；应先确定管理器是单任务独占还是跨任务共享，再选择外层同步或封装锁，并保持 `dbIdToName`/`dbTimestamps` 原子一致。性能上应维持每个 ID 常数条记录的设计，除非消费侧确实需要完整历史链。

## 验证依据

- 源码与模块边界：`br/pkg/stream/table_history.rs`、`br/pkg/stream/lib.rs`、`br/pkg/stream/Cargo.toml`、`br/pkg/stream/stubs.rs::TableSimpleInfo`。
- Go 对照：`br/pkg/stream/table_history.go`；完整生产收集链参考 `br/pkg/restore/log_client/batch_meta_processor.go::MetaKVInfoProcessor`。
- Rust 调用与迁移边界：`br/pkg/stream/table_mapping.rs::MetaInfoCollector`、`parseDBValueAndUpdateIdMapping`、`parseTableValueAndUpdateIdMapping`；`br/pkg/restore/log_client/batch_meta_processor.rs::MetaKVInfoProcessor` 与 `br/pkg/restore/log_client/stubs.rs` 中的同名桩；`br/pkg/task/restore.rs::AdjustTablesToRestoreAndCreateTableTracker`、`get_db_name_from_backup`、`build_start_table_location_info`、`should_restore_table`。
- 独立测试：`br/pkg/stream/table_mapping_test.rs::test_table_history_manager_out_of_order_ts`、`br/pkg/stream/parity_test.rs::go_rust_public_contract_matches`、`br/pkg/task/restore_test.rs::test_adjust_tables_to_restore_and_create_table_tracker`；Go 侧为 `br/pkg/stream/table_mapping_test.go::TestTableHistoryManagerOutOfOrderTS`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/stream/table_history.rs` 列出完整 150 行源码及四个直接使用文件；`explore` 核对了 `OnDatabaseInfo -> RecordDBIdToName`、`OnTableInfo -> AddTableHistory/AddPartitionHistory`、`Add*History -> addHistory` 调用边和测试/恢复消费者。单独的 `callers`/`callees` 查询在本地未于 30 秒内返回，因此调用关系又以 `explore` 结果和直接源码引用交叉核验。
- 本任务是纯文档分析，未运行 Cargo。交付前按任务命令验证目标文件存在且恰有十一个固定二级章节，并人工检查链接、事实边界和仅目标文档进入提交。

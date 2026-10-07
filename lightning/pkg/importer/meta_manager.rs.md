# `lightning/pkg/importer/meta_manager.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` crate；crate 入口 `lightning/pkg/importer/lib.rs` 通过 `#[path = "meta_manager.rs"]` 装入模块并 `pub use meta_manager::*`，因此这里的公开常量、trait、构建器和管理器会成为 importer crate 的导出面。`lightning/pkg/importer/Cargo.toml` 将该 crate 标记为 Go 包 `lightning/pkg/importer` 的 Rust library 移植，当前主要依赖 crate 内的 SQL、PD、校验和、上下文与 `TableImporter` 抽象，而没有为本文件单独声明 feature。

在 Lightning 导入链中，本文件位于“导入编排/单表导入”和“持久化协调状态”之间：

- `lightning/pkg/importer/import.rs::NewImportControllerWithPauser` 按后端与 `ParallelImport` 选择 `dbMetaMgrBuilder`、`singleMgrBuilder` 或 `noopMetaMgrBuilder`，并保存为 `Controller::metaMgrBuilder`。
- `lightning/pkg/importer/import.rs::Controller::preCheckRequirements` 调用 `metaMgrBuilder.Init`，让真实 DB 实现在正式导入前创建元数据 schema 和表。
- `lightning/pkg/importer/check_info.rs::Controller::checkClusterRegion` 通过 `taskMetaMgr::CheckTasksExclusively` 判断已有任务是否已经越过初始状态，以决定是否跳过启动前 region 检查。
- `lightning/pkg/importer/table_import.rs::TableImporter::postProcess` 通过表级管理器推进 checksum 状态并调用 `FinishTable` 清理表级元数据。

这是一个实际行为文件，不是纯门面：DB 实现会发出建库、插入、锁定查询、更新、删除和清理 SQL；同时它也包含 no-op 与单任务内存实现。不过，与 `lightning/pkg/importer/meta_manager.go` 相比，Rust 当前仍是收窄后的移植，不能把接口同名理解为 Go 全部生产语义已经具备。

## 核心职责

1. 定义两张协调表。`CreateTaskMetaTable` 对每个 task 保存 PD 配置、任务状态、退出标志、TiKV/TiFlash 来源大小和可用容量；`CreateTableMetadataTable` 以 `(table_id, task_id)` 为主键保存 row-ID 区间、导入前后 checksum、状态和重复键标志。
2. 通过 `metaMgrBuilder` 隔离实现选择。真实并行导入使用 `dbMetaMgrBuilder`，单任务路径使用 `singleMgrBuilder`，不需要元数据的路径使用 `noopMetaMgrBuilder`。
3. 通过 `tableMetaMgr` 协调同一目标表的多个 Lightning task：幂等初始化元数据行、分配不重叠 row-ID 区间、记录 checksum 基线、仲裁本地/远端重复检测以及完成后的记录清理。
4. 通过 `taskMetaMgr` 协调整批任务：记录来源大小、独占检查任务集合、暂停/恢复 PD scheduler、判断是否应恢复集群与清理元数据，并提供单任务和 no-op 兼容实现。
5. 维护与 Go 持久化格式兼容的状态字符串。表级状态由 `metaStatus` 表示，任务级状态由 `taskMetaStatus` 表示；解析函数拒绝未知字符串，避免损坏状态继续推进。

## 主要符号

- 表名与建表模板：`TaskMetaTableName = "task_meta_v2"`、`TableMetaTableName = "table_meta"`、`CreateTaskMetaTable`、`CreateTableMetadataTable`。SQL 标识符由 `common::EscapeIdentifier` 或 `common::SprintfWithIdentifiers` 处理，数据值通过 `SqlValue` 参数传递。
- `metaMgrBuilder`：公开工厂 trait，包含 `Init`、`TaskMetaMgr`、`TableMetaMgr`。`dbMetaMgrBuilder` 持有 `DB`、`taskID`、schema；`singleMgrBuilder` 只持有 task ID；`noopMetaMgrBuilder` 无状态。
- `tableMetaMgr`：表级行为 trait。核心方法是 `InitTableMeta`、`AllocTableRowIDs`、`UpdateTableBaseChecksum`、`UpdateTableStatus`、`CheckAndUpdateLocalChecksum` 和 `FinishTable`。真实实现 `dbTableMetaMgr` 固定绑定一个 `tableID`/`tableName`/`taskID`。
- `metaStatus`：有序数字 newtype。持久化顺序为 `initialized(0)`、`allocated(1)`、`restore(2)`、`restore_finished(3)`、`checksuming(4)`、`checksum_skipped(5)`、`finish(6)`；`metaStatusBaseChecksumUpdated` 和 `metaStatusLocalChecksumUpdated` 是已有状态的别名，不增加新状态值。
- `taskMetaMgr`：任务级行为 trait。`CheckTasksExclusively` 接受一个可能返回更新列表的回调；`CheckAndPausePdSchedulers` 返回 `pdutil::UndoFunc`；`CheckAndFinishRestore` 返回 `(switch_back, all_finished)`。
- `taskMetaStatus` 与 `taskMeta`：任务状态字符串依次为 `initialized`、`schedule_set`、`skip_switch`、`switched`；`taskMeta` 映射任务表八列。`storedCfgs` 当前仅有 `pause: String`，但 DB 实现没有读写它。
- `dbTaskMetaMgr`：真实 SQL 任务管理器，另有 `initialized: Mutex<bool>` 与 `tasks: Mutex<Vec<taskMeta>>` 作为内存回退/镜像。
- 清理辅助函数：`RemoveTableMetaByTableName` 可按 `table_name` 删除或在空表名时清空给定表；`MaybeCleanupAllMetas` 只在允许清理且表级元数据计数为零/表不存在时删除整个 schema。
- `noopTaskMetaMgr`、`noopTableMetaMgr`：维持调用面而不落盘；`singleTaskMetaMgr` 用 `Mutex<taskMeta>` 保存唯一任务，并用 `PdController::paused` 模拟暂停/恢复。
- 本文件没有条件编译项；测试通过 `lib.rs` 中独立的 `#[cfg(test)] #[path = "meta_manager_test.rs"]` 模块接入，符合生产逻辑与测试分文件的布局。

## 执行流程

真实 DB 路径的主要流程如下：

1. 构造控制器时，`NewImportControllerWithPauser` 根据配置选择 builder：local + parallel 使用 DB 版，local + 非 parallel 使用单任务版，其他后端使用 no-op 版。
2. `preCheckRequirements` 调用 `dbMetaMgrBuilder::Init`，依次执行 `CREATE DATABASE IF NOT EXISTS`、创建 `table_meta`、创建 `task_meta_v2`。任一 `DB::Exec` 错误立即向上传播。
3. 每个表通过 `InitTableMeta` 使用 `INSERT IGNORE` 创建 `(task_id, table_id)` 行，重试不会覆盖已有进度。
4. `AllocTableRowIDs` 先把 session 设置为悲观事务模式，再对该 `table_id` 的全部记录执行 `FOR UPDATE` 查询：
   - 忽略 `finish` 及之后的记录；遇到任一 `checksuming` 记录立即报错。
   - 当前 task 已分配时复用其 `row_id_base`，但要求保存的区间长度严格等于本次 `requiredRowIDCnt`。
   - 其他活动 task 的 `row_id_max` 决定新基线；若其他 task 已分配，则本 task 新状态直接推进到 `restore`。
   - 首次分配时更新当前行的 `row_id_base`、`row_id_max = base + requiredRowIDCnt` 和状态，并返回当前 task 已保存的基础 checksum。
5. `UpdateTableBaseChecksum` 写入导入前基线并把状态设为 `restore`；`UpdateTableStatus` 只更新状态列。
6. `CheckAndUpdateLocalChecksum` 同样先启用悲观事务并锁定该表所有 task 行：
   - 汇总所有行的重复键标志；已完成行不参与状态仲裁。
   - 当前 task 已经进入 checksum 阶段时复用其状态；其他 task 尚未进入 checksum 时，本 task 选择 `checksum_skipped` 且不再需要远端重复检测；其他 task 正在 `checksuming` 时返回冲突错误。
   - 对已越过 checksum 竞争阶段的其他 task，将其 base/local 的 KV 数与字节数相加、checksum 以 XOR 合并。
   - 当前实现随后把本 task 的本地 checksum 与 `hasLocalDupes` 写回；需要远端检查且其他 task 未报告重复时，返回合并后的基础 checksum。
7. `TableImporter::postProcess` 当前直接把状态写为 `metaStatusLocalChecksumUpdated`，执行可选 checksum 比较后调用 `FinishTable`；`FinishTable` 删除该 `table_id` 下处于 `checksuming` 或 `checksum_skipped` 的全部记录。
8. 任务级路径中，`InitTask` upsert 当前任务并置 `initialized = true`；`CheckTasksExclusively` 锁定任务表、解析八列、运行回调，并用 `REPLACE` 写回回调返回的记录。
9. `CheckAndFinishRestore` 检查缓存任务：存在仍在运行的其他任务时禁止 switch-back；当前任务失败退出时保存原状态并置 `state = 1`，当前任务完成但还有其他任务时置 `skip_switch`，全部完成时置 `switched`。
10. `CleanupTask` 删除当前 task 行；`Cleanup` 删除任务表；`CleanupAllMetas` 经 `MaybeCleanupAllMetas` 检查 `table_meta` 无残留后删除整个 schema。

需要特别注意：代码发出了 `SET SESSION ...` 和 `SELECT ... FOR UPDATE`，但 Rust `DB` 抽象下这些调用在本函数中不是被一个显式 transaction/connection 对象包围的原子闭包；Go 版本则使用 `SQLWithRetry::Transact`。因此当前 Rust 结构表达了锁定协议，却没有从本文件证据证明具备 Go 相同的事务原子性。

## 数据与状态

表级状态以数值顺序参与比较，代码依赖“状态值只前进”的语义：`AllocTableRowIDs` 用 `>= allocated` 判断是否已分配，用 `>= finish` 判断是否忽略；checksum 仲裁用 `< checksuming`、`== checksuming` 和 `>= finish` 区分竞争阶段。若新增状态插入错误位置，会改变这些比较分支。

`row_id_base`/`row_id_max` 表示为当前 task 预留的半开语义边界；Go 注释将可用区间描述为 `(base, max]`，Rust 通过校验 `row_max - row_base == requiredRowIDCnt` 保证请求量一致。跨 task 首次分配取活动记录中最大的 `row_id_max`，所以完成态记录被跳过，不再参与新的最大值计算。

checksum 数据分为导入前基线 `total_kvs_base`/`total_bytes_base`/`checksum_base` 与本 task 导入结果 `total_kvs`/`total_bytes`/`checksum`。计数和字节相加，checksum 使用 XOR 合并，与 `KVChecksum` 的组合语义一致。`has_duplicates` 在仲裁时对所有读到的行做 OR，即使对应状态随后因已完成而被跳过。

任务级记录中的 `state` 使用 `0` 表示 normal、`1` 表示未完成退出；Rust 未为这两个值声明具名常量。`tikvSourceBytes`、`tiflashSourceBytes`、`tikvAvail`、`tiflashAvail` 均为 `u64`，但 `InitTask` 接受 `i64`，真实 DB 路径直接写 SQL，单任务路径用 `as u64` 转换，因此负数会在单任务实现中发生环绕式转换，调用方必须保证来源大小非负。

DB 管理器的 `initialized` 和 `tasks` 是持久化状态的本地补充：查询得到真实行时以 DB 为准；查询为空时 `CheckTaskExist`/`CheckTasksExclusively` 会回退到内存值。它们不是独立事实源，进程重启后不会恢复。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 查询与相邻源码共同确认：

- `import.rs::NewImportControllerWithPauser -> metaMgrBuilder`：选择具体 builder；`Controller::preCheckRequirements -> metaMgrBuilder::Init`。
- `check_info.rs::Controller::checkClusterRegion -> taskMetaMgr::CheckTasksExclusively`：读取任务状态决定是否执行 region 前置检查。
- `table_import.rs::TableImporter::postProcess -> tableMetaMgr::UpdateTableStatus -> tableMetaMgr::FinishTable`：推进并清理表级状态。
- `meta_manager_test.rs` 直接覆盖 builder、DB/单任务/no-op 管理器、状态转换、row-ID 分配、checksum 聚合与清理 SQL。

本文件的直接下游依赖为：

- `crate::sql::{DB, SqlValue}`：所有持久化读写、内存 SQL 日志与查询夹具。
- `crate::common`：标识符转义和建表模板替换。
- `crate::verify::{KVChecksum, MakeKVChecksum}`：checksum 值对象及组合结果。
- `crate::pdutil::{PdController, UndoFunc, NopUndo}`：scheduler 暂停状态及恢复回调。
- `crate::table_import::TableImporter`：为表管理器提供表名与 table ID。
- `crate::context::Context` 与 `crate::errors::{Result, Errorf}`：接口兼容与错误传播。多个 DB 方法当前没有实际使用传入的 `Context`，不能据此宣称支持取消 SQL。

`Cargo.toml` 的 crate 边界显示 importer 还依赖 checkpoints、errormanager、precheck、progress、store-driver、metaservice 等库，但本文件没有直接引用它们；不能把这些 crate 级依赖误列为本文件的直接调用边。

## 错误处理与边界

- 所有真实 SQL 写入使用 `?` 原样传播 `DB` 错误；数值列字符串解析失败被转换为 `errors::Errorf`。
- `parseMetaStatus`/`parseTaskMetaStatus` 对空字符串兼容为初始态，对未知值返回错误。表级解析额外接受 `"finished"` 为完成态，而 Go 当前只接受 `"finish"`；这是 Rust 的兼容扩展。
- 查询列不足时分别返回 `invalid table meta row`、`invalid checksum meta row`、`invalid task meta row`，避免越界或用默认值掩盖存储损坏。
- row-ID 重试若发现当前 task 已有区间但长度与请求不符，会返回 allocator 校验错误；遇到任一 checksum 中记录会立即失败。Rust 没有 Go 的 30 次指数退避与 `Context` 取消分支。
- checksum 仲裁中，其他 task 处于 `checksuming` 会返回 `table ... is checksumming`；其他 task 尚未到 checksum 则选择跳过远端重复检测。这些分支依赖持久化状态顺序。
- `Mutex::lock().unwrap()` 在锁中毒时 panic；整数累计使用普通 `+=`，debug 构建溢出会 panic、release 构建可能回绕，文件中没有饱和或 checked 运算。
- `MaybeCleanupAllMetas` 对 count 字符串使用 `parse::<u64>().unwrap_or(0)`：非法计数会被当作零并继续删 schema；只有标记为非 `not_found` 的查询错误会阻止清理。这是高风险边界，扩展时应保持或有意识地修正并增加测试。
- `RemoveTableMetaByTableName` 的 `metaTable` 直接拼接到 SQL，不调用标识符转义；它要求调用方传入可信、已限定的表标识。空 `tableName` 意味着清空整表，不是“什么也不做”。
- `FinishTable` 按 `table_id` 删除所有 task 中两个 checksum 状态的记录，不限定当前 `taskID`；这是并行导入协调行为，修改 WHERE 条件会改变清理契约。

## 并发与资源生命周期

跨进程并发的设计依靠 TiDB 悲观事务和 `FOR UPDATE`：row-ID 分配、checksum 仲裁、任务集合检查都先发出 `SET SESSION tidb_txn_mode = 'pessimistic'` 并使用锁定查询。当前 Rust 实现没有像 Go 那样显式获取连接并把读取与更新包在同一 `Transact` 闭包中，因此只可确认 SQL 意图，不能确认锁一直保持到更新完成。

进程内共享对象通过 `Arc<dyn ... + Send + Sync>` 传递。`dbTaskMetaMgr` 用两个 `Mutex` 分别保护初始化标志和任务镜像；`singleTaskMetaMgr` 用 `Mutex<taskMeta>` 保护唯一记录，并另用一个 mutex 保存初始化状态。其 `CheckTasksExclusively` 先克隆快照、释放锁、运行用户回调，再重新加锁写回，因此回调期间并未持有进程内互斥锁；名称中的 “Exclusively” 在单任务实现里不等于跨线程原子读改写。

PD 暂停通过返回的 `UndoFunc` 显式管理生命周期。DB 版和单任务版把 `pd.paused` 置为 true，并把克隆的 `PdController` 捕获到闭包中，执行 undo 时置回 false；如果调用方丢弃而不执行 undo，状态不会自动恢复。no-op 版返回 `NopUndo`。`Close` 在三个 Rust 实现中都是空方法，不会关闭 PD 客户端或其他资源；Go 的 `dbTaskMetaMgr::Close` 会调用 `pd.Close()`。

schema 生命周期为：builder 创建 schema/两张表，表完成后删除表级记录，任务完成后可删 task 行或 task 表，确认 `table_meta` 为空后 `MaybeCleanupAllMetas` 删除 schema。删除动作不可由本文件自动回滚，调用方必须严格依据 `CheckAndFinishRestore` 结果决定时机。

## 与 Go 版本的对应关系

同路径 `lightning/pkg/importer/meta_manager.go` 是最直接的语义基准。Rust 对齐了 trait/struct 的分层、两张表的主要列、状态字符串、row-ID 跨 task 排序、checksum 的加法/XOR 聚合、任务完成判断，以及 no-op/single-task 替代实现。

当前已确认的差异与迁移缺口包括：

- Go `dbMetaMgrBuilder` 还有 `needChecksum`，并把完整 `TableImporter` 保存到表管理器；Rust 只复制表名/ID，因此无法在 `AllocTableRowIDs` 中检查表是否有 auto-ID、调用 `GetMaxAutoIDBase` 或执行远端 `DoChecksum`。
- Go 的 row-ID 分配与 checksum 仲裁使用独立连接、真实事务、`FOR UPDATE`、commit/rollback；对 checksum 竞争最多重试 30 次并支持上下文取消。Rust 只顺序调用 DB 抽象，没有重试/退避。
- Go 会根据 auto-ID、目标表既有数据及 `needChecksum` 决定初始基线与状态；Rust 只从元数据行计算最大 `row_id_max`，测试也明确标注当前未实现 auto-ID rebase。
- Go `CheckAndUpdateLocalChecksum` 将计算出的 `newStatus` 写入 SQL；Rust 无论仲裁结果如何都写 `metaStatusChecksuming`。因此 Rust 返回的 `need_remote_dupe` 可能为 false，但持久化状态仍为 `checksuming`，文档不能将其描述为完整 Go 对齐。
- Go `CheckAndPausePdSchedulers` 会在事务中复用/保存 JSON PD 配置，调用真实 remove/restore API，管理周期任务 context；Rust 只切换 `PdController::paused` 布尔值，`storedCfgs` 未接线，`CanPauseSchedulerByKeyRange` 在 DB/单任务实现中固定为 true。
- Go 的 DB task 检查每次从锁定表中读取；Rust `CheckAndFinishRestore` 只读取内存 `tasks` 镜像。该镜像只有调用过 `CheckTasksExclusively` 后才与 DB 查询同步。
- Go `CleanupAllMetas` 对表计数查询错误直接失败；Rust 特判 `not_found` 后允许继续，并可能把非法 count 当零。Go `Close` 关闭 PD；Rust `Close` 为空。
- Go no-op `AllocTableRowIDs` 返回 nil checksum，Rust 因返回类型不是 `Option<KVChecksum>` 而返回全零 checksum；其余 no-op 合同（任务存在、无需 switch-back、允许 cleanup、需要远端 duplicate 检查）保持一致。

Go 测试 `meta_manager_test.go` 使用 sqlmock 与 mockstore 覆盖 auto-ID 非零、已有数据 checksum、跳过 checksum、多 task 分配、checksum 冲突重试和真实事务期望；Rust 测试中的多个同名用例只对空内存夹具重复断言零基线，不能作为这些 Go 分支已移植的证据。Rust 目前更强的专门证据是 `test_alloc_table_row_ids_uses_locked_meta_ranges` 和 `test_local_checksum_aggregates_finished_peer`。

## 扩展指南

- 扩展表级状态时，必须同时修改 `metaStatus::String`、`parseMetaStatus`、所有顺序比较和 `lightning/pkg/importer/meta_manager_test.rs` 的字符串/边界测试；还要核对 Go 的数值顺序和持久化字符串，避免旧元数据不可恢复。
- 完善 row-ID 分配应从 `dbMetaMgrBuilder`/`dbTableMetaMgr` 的依赖开始，补齐 `needChecksum`、auto-ID 探测和远端 checksum，再把锁定读写放进同一事务；不要仅让现有空夹具测试通过。独立测试应覆盖 Go 测试中的 auto-ID、已有表数据、分配复用、区间不匹配、checksum 冲突重试及取消。
- 完善 checksum 仲裁时应修正/确认 SQL 使用 `new_status`，覆盖其他 task 的 `initialized`、`checksuming`、`checksum_skipped`、`finish`、重复键和多行聚合组合，并保持 `FinishTable` 的跨 task 清理意图。
- 接入真实 PD scheduler 时，应围绕 `storedCfgs`、`CheckAndPausePdSchedulers`、`CanPauseSchedulerByKeyRange` 和 `Close` 实现配置序列化、复用、回滚与资源关闭；测试必须验证 undo 在首次暂停、复用已保存配置和错误回滚三条路径上都安全。
- 调整任务完成判定时，同步验证 `state` 与 `taskMetaStatus` 的二维状态机，尤其是“当前任务失败退出”“当前完成但其他任务仍在运行”“全部完成”三种结果，以及 DB 镜像何时刷新。
- 修改清理函数前先确定调用者是否传入可信标识符，并增加非法 count、表不存在、仍有表记录、空/非空 `tableName` 的独立测试。schema 删除属于破坏性动作，不应通过放宽错误处理来提高表面成功率。
- 测试继续放在独立的 `lightning/pkg/importer/meta_manager_test.rs`，不要内嵌到生产文件；需要对齐的 Go 回归仍以 `lightning/pkg/importer/meta_manager_test.go` 为行为清单，而不是把 Rust 现有简化测试当作完整规格。

## 验证依据

本说明基于以下可复核证据编写：

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；用 `node --file` 完整读取 `lightning/pkg/importer/meta_manager.rs` 1–1005 行，并查询 `metaMgrBuilder`、`dbTableMetaMgr`、`dbTaskMetaMgr`、`AllocTableRowIDs`、`CheckAndUpdateLocalChecksum`、`CheckAndPausePdSchedulers`、`CheckAndFinishRestore`、`MaybeCleanupAllMetas`。
- RustCodeGraph 调用证据：`preCheckRequirements -> metaMgrBuilder::Init`，`checkClusterRegion -> taskMetaMgr::CheckTasksExclusively`，`TableImporter::postProcess -> UpdateTableStatus/FinishTable`；图查询还定位到 Rust 独立测试对各 builder/manager 方法的直接调用。由于 `callers` 子命令对重名 trait 方法未返回可区分结果，调用边又用精确 `explore` 与对应源码节点复核。
- crate/模块证据：`lightning/pkg/importer/Cargo.toml`、`lightning/pkg/importer/lib.rs`。
- Rust 上下游源码：`lightning/pkg/importer/import.rs`、`lightning/pkg/importer/check_info.rs`、`lightning/pkg/importer/table_import.rs`。
- Rust 独立测试：`lightning/pkg/importer/meta_manager_test.rs`，重点是状态字符串、no-op 合同、单任务初始化/回调、SQL 发射、锁定范围分配和 peer checksum 聚合。
- Go 对照：`lightning/pkg/importer/meta_manager.go` 全部相关实现段，以及 `lightning/pkg/importer/meta_manager_test.go` 的 row-ID、auto-ID、checksum、重试、任务排他与单任务用例。
- 人工事实复核：确认本文件无条件编译项；确认真实 DB、no-op、single-task 三套实现的返回值与副作用；确认当前 Rust 与 Go 的事务、重试、auto-ID、PD 配置和资源关闭差异均在文档中显式标注，未将未接线能力写成已支持。

本任务是纯文档分析，按计划不运行 Cargo。交付结构验证应确认文件存在且恰好包含本文固定的十一个二级标题。

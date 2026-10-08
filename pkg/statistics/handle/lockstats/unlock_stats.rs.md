# `pkg/statistics/handle/lockstats/unlock_stats.rs`

## 文件定位

本文件是 `astersql-statistics-handle-lockstats` crate 的统计信息解锁实现。crate 根 `pkg/statistics/handle/lockstats/lib.rs` 以私有模块 `mod unlock_stats` 装配它，并重新导出 `RemoveLockedTables`、`RemoveLockedPartitions` 以及三个 SQL 常量。它位于 `UNLOCK STATS` 执行链的持久化操作端：上层 `pkg/executor/lockstats/unlock_stats_executor.rs::UnlockExec::Next` 解析表和分区，统计句柄的 `pkg/statistics/handle/lockstats/lock_stats.rs::statsLockImpl` 再在事务会话中调用本文件入口。

`pkg/statistics/handle/lockstats/Cargo.toml` 指定 `lib.rs` 为库入口，并用 `package.metadata.porting.go-package` 标明 Go 对照包 `pkg/statistics/handle/lockstats`。清单中的历史会话、统计工具和类型依赖目前均置于 `target.'cfg(any())'` 下，恒为禁用；因此本文件当前直接依赖的是本 crate 定义的抽象，而不是这些路径 crate。

## 核心职责

本文件负责解除表或分区的统计锁，并在删除 `mysql.stats_table_locked` 锁行之前，把锁定期间累计的 `count`、`modify_count` 增量回灌到 `mysql.stats_meta`。核心不变量是“先回灌、后删锁”：任一步返回错误都会立即终止当前入口，避免在增量尚未应用时丢失锁行。

表级入口只处理请求中当前确实已锁定的表，并连带处理该表下仍处于锁定状态的分区；未锁定对象只形成稳定排序的跳过提示。分区级入口还维护父子锁约束：父表整体被锁时，不允许单独解锁其分区。分区增量同时应用到分区物理 ID 和父表 ID，使两级统计的行数与修改计数保持连续。

## 主要符号

- `selectDeltaSQL`：Go 对照 SQL 模板，用物理 `table_id` 查询锁表中的 `count` 和 `modify_count`。Rust 主流程实际通过 `RestrictedSQLExecutor::LockedStatsDelta` 执行该语义。
- `updateDeltaSQL`：Go 对照更新模板；更新版本，累加 `count`（结果小于等于零时归零）和 `modify_count`。Rust 通过 `RestrictedSQLExecutor::ApplyStatsDelta` 委托实现。
- `DeleteLockSQL`：按精确 `table_id` 删除锁行；`unlock_stats_test.rs::canonical_unlock_sql_targets_exact_locked_table_id` 固定了该契约。
- `RemoveLockedTables(s, tables) -> Result<String, StatsError>`：公开的批量表解锁入口。输入值 `StatsLockTable` 携带表全名和分区 ID 到名称的映射，返回值是供执行器追加为 warning 的跳过消息。
- `RemoveLockedPartitions(s, tid, name, parts) -> Result<String, StatsError>`：公开的分区解锁入口；`tid` 是父表 ID，`parts` 是目标分区 ID 到显示名的映射。
- `updateStatsForTable`：读取单个表的锁定增量并应用到该表。
- `updateStatsForPartition`：读取分区增量，依次应用到分区和父表。
- `getStatsDeltaFromTableLocked`、`updateDelta`、`deleteLock`：分别是 `LockedStatsDelta`、`ApplyStatsDelta`、`DeleteStatsLock` 的薄语义适配层。

文件没有自定义类型、trait、条件编译项或异步函数。两个 `RemoveLocked*` 是公开行为入口，三个 SQL 常量公开供对照与契约测试使用，其余帮助函数均为私有实现。

## 执行流程

`RemoveLockedTables` 首先用 `QueryLockedTablesWithExecutor` 读取全部锁定 ID，然后把请求中的每个表 ID 及其全部分区 ID 汇成候选集合，并用 `GetLockedTables` 取交集。随后逐表处理：未锁定的表只把 `FullName` 加入跳过集合；已锁定的表先经 `updateStatsForTable` 回灌表增量，再 `deleteLock` 删除表锁。最后遍历该表分区，仅对交集中仍锁定的分区调用 `updateStatsForPartition(pid, table_id)` 并删锁。完成后由 `generateStableSkippedTablesMessage` 排序并生成确定性的提示文本。

`RemoveLockedPartitions` 同样先查询完整锁集合。若集合包含父表 `tid`，立即返回 `skip unlocking partitions of locked table: {name}`，不读取增量也不删除任何锁。否则，它从 `parts` 收集分区 ID、筛出已锁定子集，再逐个跳过未锁分区或对已锁分区执行“回灌分区 → 回灌父表 → 删除分区锁”。末尾调用 `generateStableSkippedPartitionsMessage` 生成确定性提示。

单对象回灌中，`updateStatsForTable` 的调用序列是 `LockedStatsDelta(id)` 后 `ApplyStatsDelta(id, count, modify)`；`updateStatsForPartition` 的序列是 `LockedStatsDelta(pid)`、`ApplyStatsDelta(pid, ...)`、`ApplyStatsDelta(tid, ...)`。外层入口只有这些步骤全部成功后才调用 `DeleteStatsLock`。

## 数据与状态

持久化状态分布在 `mysql.stats_table_locked` 与 `mysql.stats_meta`：前者保存锁状态及锁定期增量，后者保存优化器读取的正式统计元数据。本文件自身不缓存状态，所有读取和写入均经调用方传入的 `&mut dyn RestrictedSQLExecutor` 完成。

`HashMap<i64, StatsLockTable>` 用物理表 ID 定位待处理表；每个 `StatsLockTable::PartitionInfo` 再映射分区物理 ID 与显示名。分区入口的 `HashMap<i64, String>` 同样以物理 ID 为主键。集合迭代顺序不构成输出契约：跳过消息生成函数负责排序；数据库变更顺序可随 `HashMap` 顺序变化，但每个对象内部仍保持回灌先于删锁。

无增量行的语义由 `RestrictedSQLExecutor::LockedStatsDelta` 提供；Rust 测试执行器以 `(0, 0)` 表示缺失，并验证表仍会执行零增量应用后删锁。真正的 `count` 下限归零、版本更新与 SQL 原子更新语义属于 `ApplyStatsDelta` 的实现契约，`updateDeltaSQL` 则记录了与 Go 一致的 SQL 形式。

## 依赖与调用关系

上游应用链为 `UnlockExec::Next` → `StatsHandle::RemoveLockedTables/RemoveLockedPartitions` → `statsLockImpl::RemoveLocked*` → 本文件同名入口。`statsLockImpl` 使用 `StatsSession::WithSession(true, callback)` 包裹调用，表明同次解锁中的查询、回灌和删锁应在一个事务会话内完成；非空返回消息由 `UnlockExec::Next` 追加为会话 warning。

本文件直接调用 `query_lock.rs::QueryLockedTablesWithExecutor` 获取锁集合，调用 `lock_stats.rs::GetLockedTables` 过滤候选，并复用 `generateStableSkippedTablesMessage`、`generateStableSkippedPartitionsMessage`、`unlockAction` 和 `unlockedStatus` 构造提示。所有存储副作用通过 `lib.rs::RestrictedSQLExecutor` 的 `LockedStatsDelta`、`ApplyStatsDelta`、`DeleteStatsLock` 三个方法下沉。

RustCodeGraph 的精确被调用边确认：`RemoveLockedTables` 指向查询锁集合、表回灌、分区回灌和删锁；`RemoveLockedPartitions` 指向查询锁集合、分区回灌和删锁；`updateStatsForTable` 与 `updateStatsForPartition` 均指向增量读取和应用。由于同名 Go/Rust/trait 符号较多，通用 `callers` 查询未可靠解析跨 trait 的动态调用；上游关系因此以 `lock_stats.rs` 和执行器源码中的显式调用为证据，不把缺失的静态图边解释为未接线。

## 错误处理与边界

所有存储操作返回统一的 `StatsError`，各函数使用 `?` 原样向上传播，没有吞错或重包装。查询全量锁集合失败时没有任何后续副作用；读取或应用增量失败时不会删除当前锁；删锁失败则返回错误。`unlock_stats_test.rs::remove_locked_tables_propagates_delta_and_delete_errors` 分别验证读取增量错误时没有调用记录，以及删锁错误前已发生零增量应用。

父表整体锁定是分区入口的正常跳过结果而非错误。请求对象未锁定也不是错误：表名或分区名进入提示消息，其余已锁对象仍继续处理。表级入口只在父表本身已锁时处理其分区；因此“父表未锁但某分区已锁”的请求不会通过该表级入口解锁该分区，应走 `RemoveLockedPartitions`。

本文件未自行检查 ID 是否属于传入表，也不校验名称；这些关联由上游 InfoSchema 解析和调用参数保证。`HashMap` 中同一 ID 不会重复，但不同表的分区映射若错误地复用 ID，本文件没有额外防御。SQL 常量采用 `%?` 占位符，不拼接用户名称。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或长生命周期资源。可变执行器借用 `&mut dyn RestrictedSQLExecutor` 让单次函数调用内的存储操作串行发生，并阻止同一执行器在安全 Rust 中被同时可变使用。

事务生命周期由调用方 `statsLockImpl` 管理：`WithSession(true, …)` 应在回调成功后提交、错误时不提交，因而把批量回灌与删锁作为一个事务边界。该原子性不由本文件单独实现；绕过 `statsLockImpl` 直接调用公开自由函数时，调用者必须提供等价的事务语义。内存中的 `Vec`、`HashMap` 借用和临时跳过列表在函数返回时释放，没有后台工作需要清理。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/statistics/handle/lockstats/unlock_stats.go`。两个版本都先查询锁集合，都禁止在父表被锁时单独解锁分区，都将分区增量同时写回分区与父表，并都在成功回灌后删除锁行。Go 测试 `unlock_stats_test.go` 进一步固定了无增量行视为 `(0, 0)`、StartTS 参与 `stats_meta.version` 更新、部分对象跳过时的消息以及父表锁阻止子分区解锁。

实现结构存在一处刻意抽象差异：Go 的 `updateDelta` 直接取得 StartTS 并执行 `updateDeltaSQL`，`getStatsDeltaFromTableLocked` 与删除逻辑也直接调用受限 SQL；Rust 把这些细节分别下沉到 `RestrictedSQLExecutor::ApplyStatsDelta`、`LockedStatsDelta` 和 `DeleteStatsLock`。因此 Rust 文件中的三个 SQL 常量主要承担移植契约和测试对照，不能仅凭本文件断言具体执行器已经采用该 SQL；具体版本更新、count 下限和事务实现需要在执行器实现处继续验证。

Go 将“回灌 + 删除”组合在 `updateStatsAndUnlockTable` / `updateStatsAndUnlockPartition` 中；Rust 分拆为 `updateStatsFor*` 后由外层入口调用 `deleteLock`，但顺序相同。Go 还记录解锁日志并有 `mockStatsVersion` failpoint，Rust 本文件没有对应日志和 failpoint；Rust 独立测试改用可记录、可注错的 trait mock 验证调用语义。

## 扩展指南

若新增锁定期统计字段，应同步扩展 `RestrictedSQLExecutor::LockedStatsDelta` 与 `ApplyStatsDelta` 的数据契约、`selectDeltaSQL`/`updateDeltaSQL`、`updateStatsForTable`、`updateStatsForPartition`，并确保分区字段仍同时回灌父表。对应测试应继续放在独立文件 `pkg/statistics/handle/lockstats/unlock_stats_test.rs`，覆盖无增量、正负边界、分区到父表传播、各阶段注错和删锁顺序；不要把测试内嵌进生产源文件。

若改变批处理或错误恢复策略，必须保持“增量不能因先删锁而丢失”的不变量，并检查 `StatsSession::WithSession(true, …)` 的事务原子性。并行化时还需明确同一父表的多个分区会并发累加同一 `stats_meta` 行，确认底层 SQL 的原子累加和锁冲突策略，不能只依赖当前串行循环。

若改变跳过规则或提示文本，应同时更新 `generateStableSkipped*Message` 的调用约定、执行器 warning 行为以及 Rust/Go 对照测试。若新增公开入口或常量，还要在 `lib.rs` 调整再导出；若引入实际跨 crate 依赖，应先处理 `Cargo.toml` 中当前恒禁用的 `cfg(any())` 配置，而不能假设这些依赖已经参与编译。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/statistics/handle/lockstats` 确认同包生产文件和独立测试；`node --file .../unlock_stats.rs` 核对全部 130 行及八个主要符号。
- RustCodeGraph 调用查询：`callees RemoveLockedTables`、`callees RemoveLockedPartitions`、`callees updateStatsForTable`、`callees updateStatsForPartition`、`callees updateDelta`，用于核对查询、回灌和删除调用边，并识别 Go/Rust 同名符号歧义。
- Rust 源与装配：`pkg/statistics/handle/lockstats/lib.rs`、`lock_stats.rs`、`query_lock.rs`，以及上游 `pkg/executor/lockstats/unlock_stats_executor.rs`。
- crate 边界：`pkg/statistics/handle/lockstats/Cargo.toml`；确认库入口、Go 包映射、历史移植任务和当前 `cfg(any())` 依赖配置。
- Rust 独立测试：`pkg/statistics/handle/lockstats/unlock_stats_test.rs`，覆盖精确删除 SQL、表/分区回灌、稳定跳过消息、父表锁约束及错误传播。
- Go 对照：`pkg/statistics/handle/lockstats/unlock_stats.go` 与 `unlock_stats_test.go`，核对 SQL、StartTS、增量读取、回灌顺序和边界行为。
- 人工复核限制：RustCodeGraph 对跨 trait 动态分发未生成可靠 callers 边，因此上游接线由 `statsLockImpl` 与 `UnlockExec::Next` 的显式源码关系补证；本任务按计划不运行 Cargo，不对具体 `RestrictedSQLExecutor` 后端作运行时断言。

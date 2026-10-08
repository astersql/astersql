# `pkg/statistics/handle/lockstats/lock_stats.rs`

## 文件定位

本文件是 `astersql-statistics-handle-lockstats` 子 crate 的“加统计锁”实现，源码由同目录 `lib.rs` 以 `mod lock_stats` 纳入并通过 `pub use lock_stats::*` 导出。它把表或分区的物理 ID 写入统计锁存储，并同步推进对应 `stats_meta.version`，使统计缓存能够感知锁状态变化；解锁实现位于同目录 `unlock_stats.rs`，查询实现位于 `query_lock.rs`。

应用主链的直接证据是 `pkg/session/runtime/admin.rs::execute_lock_stats`：会话层先把 SQL AST 中的表名、分区名解析成物理 ID，再通过 `Domain::stats_lock` 取得本文件构造的 `statsLockImpl`，最终调用 `LockTables` 或 `LockPartitions`。`pkg/domain/domain.rs::stats_lock` 用 Domain 持有的 `stats_store` 构造 `SessionRef`；Domain 的分区元数据协调逻辑还会直接调用 `InsertLockAndUpdateVersion`，让已锁父表的新分区继承锁状态。

`Cargo.toml` 将 crate 命名为 `astersql-statistics-handle-lockstats`，入口为 `lib.rs`，并用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/statistics/handle/lockstats`。当前列出的迁移依赖全部置于 `target.'cfg(any())'`，即不会参与实际编译；本文件实际依赖的是 crate 根中定义的本地抽象，而不是这些关闭的依赖项。

## 核心职责

- `statsLockImpl` 把公开锁/解锁操作放入 `StatsSession::WithSession(true, ...)` 的事务回调中；查询接口则复用 `query_lock.rs` 的非事务读取。
- `AddLockedTables` 对输入表及其分区做一次锁集合快照查询，只为尚未锁定的物理 ID 执行写入；表本身已锁时只跳过该表 ID，仍会补锁尚未锁定的分区 ID。
- `AddLockedPartitions` 在父表已锁时直接跳过全部目标分区，否则逐个补锁未锁分区。
- `InsertLockAndUpdateVersion` 规定每个物理 ID 的副作用顺序：先写统计锁，再更新统计元数据版本。
- 两个 `generateStableSkipped*Message` 函数对跳过名称排序，产生确定、与 Go 版本兼容的用户告警文本。

本文件不负责解析 SQL、解析表名/分区名、提交事务、维护统计增量或删除锁；这些职责分别位于会话/执行器层、`StatsSession` 实现和 `unlock_stats.rs`。

## 主要符号

- `lockAction`、`unlockAction`、`lockedStatus`、`unlockedStatus`：组成锁定与解锁告警的兼容文案片段。解锁文件也复用其中的解锁常量。
- `insertSQL`、`updateMetaVersionSQL`：与 Go 版一致的系统表 SQL 模板。当前 Rust 本文件不直接执行字符串，而由 `RestrictedSQLExecutor::InsertStatsLock` 和 `UpdateStatsMetaVersion` 实现具体存储动作；常量保留了协议和移植对照。
- `statsLockImpl { pool: SessionRef }`：基于共享会话池的句柄。`NewStatsLock(SessionRef) -> statsLockImpl` 是构造入口。
- `statsLockImpl::LockTables`、`LockPartitions`：事务包装后的加锁公开方法；返回空字符串表示无跳过项，非空字符串供上层作为 warning。
- `statsLockImpl::RemoveLockedTables`、`RemoveLockedPartitions`：相同事务边界下转发到 `crate::unlock_stats`，放在本类型上以形成完整锁句柄 API。
- `statsLockImpl::GetLockedTables`：先查询全部锁 ID，再筛出调用方给定 ID 的交集；注释要求调用方尽量批量查询。
- `statsLockImpl::GetTableLockedAndClearForTest`：命名沿用 Go 测试接口，但当前实现只返回锁集合，并不会清空持久化数据。
- `AddLockedTables(&mut dyn RestrictedSQLExecutor, &HashMap<i64, StatsLockTable>)`：表级核心算法。
- `AddLockedPartitions(&mut dyn RestrictedSQLExecutor, tid, name, &HashMap<i64, String>)`：分区级核心算法。
- `generateStableSkippedTablesMessage`、`generateStableSkippedPartitionsMessage`：纯字符串生成函数，覆盖空、单项、多项、部分成功和全部跳过分支。
- `InsertLockAndUpdateVersion`：单 ID 写路径，任何一步失败都立即返回 `StatsError`。

## 执行流程

表级 `LOCK STATS` 的流程如下：

1. `execute_lock_stats` 构造 `HashMap<table_id, StatsLockTable>`，其中 `StatsLockTable::PartitionInfo` 保存分区 ID 到显示名的映射。
2. `statsLockImpl::LockTables` 调用 `WithSession(true, ...)`，将查询、所有写入和版本更新置于同一事务契约中。
3. `AddLockedTables` 通过 `QueryLockedTablesWithExecutor` 一次读取全部已锁 ID，并收集所有输入表 ID 与分区 ID，再由 `GetLockedTables` 过滤出相关子集。
4. 每个已锁表只把 `FullName` 加入 `skipped`；未锁表 ID 加入 `to_lock`。每个未锁分区 ID 独立加入 `to_lock`，不因父表出现在 `skipped` 中而省略。
5. 对 `to_lock` 中每个 ID 调用 `InsertLockAndUpdateVersion`，依次插锁并更新版本。
6. `generateStableSkippedTablesMessage` 排序名称并生成空消息、全部跳过消息或“other tables ... successfully”的部分成功消息。

分区级流程先读取同一锁集合。如果 `tid` 已锁，立即返回 `skip locking partitions of locked table: {name}`，不产生写调用；否则过滤目标分区 ID，写入未锁分区，并用分区消息函数报告已锁分区。

`HashMap` 的遍历顺序没有保证，因此不同物理 ID 的写入先后也没有稳定保证；算法不依赖该顺序。对外提示显式排序跳过名称，因而具有稳定性。

## 数据与状态

持久状态由执行器背后的 `mysql.stats_table_locked` 和 `mysql.stats_meta.version` 表示。本文件自身只长期保存一个 `SessionRef`；每次调用中的 `locked`、`ids`、`skipped` 与 `to_lock` 都是局部快照或工作集合。

`StatsLockTable` 定义于同 crate 的 `lib.rs`，包含表全名和分区映射。表 ID 与分区 ID 都以 `i64` 进入同一个锁集合，因此调用方必须提供真实且不冲突的物理 ID。`GetLockedTables` 使用 `HashMap<i64, ()>` 表示集合，仅关心成员关系。

加锁写入使用 upsert 语义的 Go 对照 SQL，因此重复锁理论上是幂等的；不过 Rust 算法仍先查询并跳过已锁项，以便生成准确提示并避免无谓更新。版本更新紧随插锁，用于使依赖统计元数据版本的消费者观察到变化。

## 依赖与调用关系

上游调用关系：

- `pkg/session/runtime/admin.rs::execute_lock_stats` → `Domain::stats_lock` → `statsLockImpl::LockTables` / `LockPartitions`。
- `pkg/domain/domain.rs::stats_lock` → `NewStatsLock`。
- `pkg/domain/domain.rs` 的分区同步和新增分区路径 → `InsertLockAndUpdateVersion`，用于父表已锁时补锁物理分区。
- 同目录 `lock_stats_test.rs` 直接调用核心函数和公开句柄方法验证契约。

下游关系：

- `LockTables` / `LockPartitions` → `StatsSession::WithSession(true, ...)` → `AddLockedTables` / `AddLockedPartitions`。
- 两个 Add 函数 → `query_lock.rs::QueryLockedTablesWithExecutor` 和 `GetLockedTables`；未锁 ID → `InsertLockAndUpdateVersion`。
- `InsertLockAndUpdateVersion` → `RestrictedSQLExecutor::InsertStatsLock` → `RestrictedSQLExecutor::UpdateStatsMetaVersion`。
- 删除入口 → `unlock_stats.rs::RemoveLockedTables` / `RemoveLockedPartitions`。

RustCodeGraph 的文件节点报告本文件被 `pkg/domain/domain.rs`、`pkg/statistics/handle/lockstats/lock_stats_test.rs` 和 `query_lock_test.rs` 等文件使用。精确 `callers` 命令在本次索引上未在 30 秒内返回，因此上述具体调用边又以这些源码位置和 `rg` 精确符号引用复核，没有采用宽泛 `explore` 中的同名噪声。

## 错误处理与边界

所有存储和会话错误统一以 `StatsError` 向上传播。`WithSession` 回调中的 `?` 会立即终止算法，公开方法也立即返回该错误；是否回滚已经执行的动作由 `StatsSession::WithSession(true, ...)` 的契约保证，而不是本文件手动补偿。

`InsertLockAndUpdateVersion` 在插锁失败时不会调用版本更新；版本更新失败时锁写入已经在当前事务中发生，必须依赖事务回滚避免半完成状态。`lock_stats_test.rs::insert_lock_stops_on_the_first_error` 明确验证了这两个调用序列。

空表集合或空分区集合会得到空提示并不执行写入；本文件不把它们判为错误。上层负责 SQL 合法性和表/分区存在性检查。父表已锁是分区入口的特殊短路边界。表级入口则保留 Go 行为：父表已锁并不阻止补锁其未锁分区。

消息函数以总请求数和跳过名称数判断“部分成功”，默认调用方提供相互一致的数据；它们不验证重复名称、名称数大于总数或 ID 重复等异常输入。`GetTableLockedAndClearForTest` 的名字也不能被理解为实际清理保证。

## 并发与资源生命周期

`SessionRef` 是 `Arc<dyn StatsSession>`，且 `StatsSession: Send + Sync`，所以句柄可以共享底层会话池；本文件没有自己的互斥锁、后台任务或通道。每次公开修改调用只借用一个执行器，并把其生命周期限制在 `WithSession` 回调内。

一致性依赖事务边界：读取当前锁集合、插入所有目标锁、更新所有版本应当在 `wrap_transaction = true` 的同一事务中提交。`lock_stats_test.rs::public_lock_methods_wrap_all_mutations_in_a_transaction` 验证四个修改入口都传入 `true`。查询方法 `QueryLockedTables` 使用 `false`，只取得一个时点的集合快照，不提供跨后续写入的同步保证。

并发事务可能基于相近快照同时决定写入同一 ID；锁表插入的 upsert 设计负责容忍重复，最终串行化和冲突处理属于具体 `StatsSession`/执行器实现。本文件没有重试逻辑，也不承诺多个 ID 的遍历或写入顺序。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/lockstats/lock_stats.go`。Rust 保留了 Go 的类型/函数命名、核心分支、事务包装意图、稳定排序文案，以及“插入锁后更新 `stats_meta.version`”的顺序。Rust 独立测试复刻了 Go 测试中的空、单项、多项、部分成功、全部跳过、父表已锁及写入错误场景。

主要实现差异如下：

- Go 的 `NewStatsLock` 返回 `types.StatsLock` 接口，Rust 返回具体 `statsLockImpl`；应用侧通过该具体类型调用。
- Go 使用 `util.CallWithSCtx(..., util.FlagWrapTxn)` 和真实 `sessionctx.Context`；Rust 以 `StatsSession` 与 `RestrictedSQLExecutor` trait 隔离会话和存储。
- Go 的辅助函数直接执行 `insertSQL`、读取 StartTS 并执行 `updateMetaVersionSQL`；Rust 的辅助函数调用两个执行器方法，`StartTS` 与 SQL 常量没有在本文件直接使用，具体版本值由执行器实现负责。
- Go 在加锁路径记录结构化日志；当前 Rust 文件没有对应日志调用。
- Go 的 `queryLockedTables` 是私有方法；Rust 公开了 `GetLockedTables` 和测试查询方法，并在 crate 根重新导出。

因此“行为对齐”应理解为当前抽象可观察的调用顺序、过滤规则与提示文本对齐，不能据此声称 Rust 已直接接入 Go 的全部日志、sessionctx 或 SQL 工具链。

## 扩展指南

新增锁定策略时，优先在 `AddLockedTables` 或 `AddLockedPartitions` 修改过滤与目标集合构造；改变单 ID 的持久化协议时修改 `InsertLockAndUpdateVersion`，并同步具体 `RestrictedSQLExecutor` 实现。不要在消息生成函数中混入存储副作用，也不要绕过 `WithSession(true, ...)`，否则会破坏多 ID 原子性。

新增用户可见分支时必须同步独立测试 `pkg/statistics/handle/lockstats/lock_stats_test.rs`，并与 `lock_stats_test.go` 的对应场景核对；测试逻辑不应内嵌回生产文件。涉及解锁文案时还要检查 `unlock_stats.rs` 与 `unlock_stats_test.rs`，因为它们复用本文件的动作/状态常量和稳定消息函数。涉及 SQL 入口时应同步检查 `pkg/session/runtime/admin.rs` 以及执行器层对空输入、名称解析和 warning 的处理。

兼容性风险集中在精确提示字符串、父表与分区的独立锁语义、错误发生后的回滚以及版本更新顺序。性能上应保持“一次查询锁全集、内存中过滤”的批量模式，避免为每个 ID 单独查询；若输入很大，可优化集合容量或批量写入，但必须保留幂等、事务和部分跳过提示语义。若将 `cfg(any())` 下的迁移依赖真正启用，应先确认 crate 根的本地 trait 与真实类型边界，而不能仅替换 import。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/statistics/handle/lockstats` 列出 Rust/Go 实现与测试；`node --file .../lock_stats.rs` 读取了 224 行完整源码并报告直接使用文件；`query LockStats` / `query lock_stats` 定位 `NewStatsLock`、`statsLockImpl`、Go 对照和执行器符号。逐符号 `callers AddLockedTables` 在 30 秒内无结果并被终止，故未把它作为正向证据。
- 生产源码：`pkg/statistics/handle/lockstats/lock_stats.rs`、`lib.rs`、`query_lock.rs`，以及直接入口 `pkg/session/runtime/admin.rs`、`pkg/domain/domain.rs`、`pkg/executor/lockstats/lock_stats_executor.rs`。
- crate 配置：`pkg/statistics/handle/lockstats/Cargo.toml`；工作区及相邻 manifest 的精确引用由 `rg 'astersql-statistics-handle-lockstats'` 核验。
- Go 对照：`pkg/statistics/handle/lockstats/lock_stats.go` 与 `lock_stats_test.go`。
- Rust 独立测试：`pkg/statistics/handle/lockstats/lock_stats_test.rs`，覆盖稳定消息、过滤与写入副作用、父表锁短路、首错停止和事务包装。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核唯一生产物、路径引用、当前事实与未验证限制。

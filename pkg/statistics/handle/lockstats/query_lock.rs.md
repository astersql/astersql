# `pkg/statistics/handle/lockstats/query_lock.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-lockstats` crate，是统计锁子系统的只读查询与集合过滤层。crate 入口 `pkg/statistics/handle/lockstats/lib.rs` 以私有模块 `query_lock` 装配本文件，再通过 `pub use query_lock::*` 导出公开查询 API；同一入口定义了本文件依赖的 `RestrictedSQLExecutor`、`StatsSession`、`SessionRef` 和 `StatsError`。Cargo 元数据 `pkg/statistics/handle/lockstats/Cargo.toml` 把该 crate 对应到 Go 包 `pkg/statistics/handle/lockstats`，库入口为 `lib.rs`。

它位于统计锁操作的共同前置步骤：`lock_stats.rs` 在新增锁前查询已有记录以跳过重复对象，`unlock_stats.rs` 在回灌统计增量和删除锁记录前查询实际锁定对象，而 `statsLockImpl::GetLockedTables` 对外提供指定 ID 集合的锁定状态。该文件只观察 `mysql.stats_table_locked` 的 ID 集合，不创建、更新或删除锁。

## 核心职责

1. 用 `selectSQL` 保存与 Go 实现一致的全量锁 ID 查询语句：`SELECT table_id FROM mysql.stats_table_locked`。
2. `QueryLockedTablesWithExecutor` 从一个已经取得的受限 SQL 执行器读取所有锁定的表或分区 ID，并转换为 `HashMap<i64, ()>`。map 被当作集合使用，使成员判断平均为常数时间，并天然去重重复存储行。
3. `QueryLockedTables` 为独立查询打开一次 `StatsSession` 会话，并明确传入 `wrap_transaction = false`；实际读取委托给执行器版本。
4. `GetLockedTables` 计算“已锁全集”与调用方候选 ID 的交集，返回新的集合，不改变输入。

本文件把“怎样取得锁全集”与“怎样筛选候选对象”分开，因此事务中的加锁/解锁逻辑可以复用现有执行器，而外部状态查询可以通过会话池安全取得执行器。

## 主要符号

- `pub const selectSQL: &str`：与 `query_lock.go` 的 `selectSQL` 文本一致。当前 Rust 文件保留它作为 SQL 契约，但查询实现经 `RestrictedSQLExecutor::LockedTableIds` 间接执行；本文件不会把该常量传给 `ExecRestrictedSQL`。
- `pub(crate) fn QueryLockedTablesWithExecutor(executor: &mut dyn RestrictedSQLExecutor) -> Result<HashMap<i64, ()>, StatsError>`：crate 内部入口。调用 `executor.LockedTableIds()`，逐个映射为 `(table_id, ())` 并收集成集合。它需要可变执行器引用，因为底层查询可能推进会话/结果状态。
- `pub fn QueryLockedTables(session: &SessionRef) -> Result<HashMap<i64, ()>, StatsError>`：公开全量查询入口。通过 `StatsSession::WithSession(false, callback)` 取得执行器，把回调结果暂存在局部 `Option` 中；会话成功而回调未写入时以空 map 兜底。
- `pub fn GetLockedTables(locked: &HashMap<i64, ()>, ids: &[i64]) -> HashMap<i64, ()>`：公开纯集合函数。按 `ids` 遍历并以 `locked.contains_key` 判断，只返回候选集合中真实锁定的 ID；重复候选也会因 map 语义被折叠。

文件没有结构体、枚举、trait、异步函数或条件编译项。命名保留 Go 风格，crate 根以 `#![allow(non_snake_case, non_upper_case_globals)]` 接受这些符号。

## 执行流程

独立全量查询的流程是：调用方把 `SessionRef` 传给 `QueryLockedTables`；函数准备空的 `Option<HashMap<...>>`；`WithSession(false, ...)` 提供一个 `RestrictedSQLExecutor`；回调调用 `QueryLockedTablesWithExecutor`；执行器的 `LockedTableIds` 返回 `Vec<i64>`；迭代器将 ID 转成 map 键并去重；回调写入结果并返回成功；会话层结束后函数取出 map 返回。

事务内流程更短。`AddLockedTables`、`AddLockedPartitions`、`RemoveLockedTables` 和 `RemoveLockedPartitions` 已经持有由上层事务会话提供的执行器，直接调用 `QueryLockedTablesWithExecutor`，再把结果交给 `GetLockedTables` 或直接进行 `contains_key` 判断。这样不会为了只读前置检查再打开一个会话，也不会改变外层事务边界。

过滤流程是：遍历调用方提供的 `ids` 切片；只保留在 `locked` 中存在的 ID；将每个命中项复制为 `(i64, ())`；收集为新的 `HashMap`。空锁集合或空候选切片自然产生空结果，无需特殊共享状态。

## 数据与状态

统计锁的持久化事实来源是 `mysql.stats_table_locked.table_id`。表 ID 与分区 ID 都使用 `i64`，本文件不区分两者，语义由调用方和信息模式决定。`HashMap<i64, ()>` 是集合表示：值没有业务含义，唯一性、成员判断和去重均由键提供。

`QueryLockedTables` 唯一的临时可变状态是栈上的 `rows: Option<HashMap<i64, ()>>`，用于把同步回调中的所有权结果带出 `WithSession`。它不缓存跨调用结果，因此每次查询都反映底层执行器该次返回的快照。`GetLockedTables` 只借用输入并新建输出，不会消费或修改全量集合。

`selectSQL` 描述数据库查询契约；具体 Rust 执行位置封装在 `RestrictedSQLExecutor::LockedTableIds` 的实现中。`pkg/statistics/handle/restricted_sql.rs` 是生产执行器适配层的直接接线位置，而本文件不解析 `SqlRow`，也不拥有数据库连接。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 与源码核对如下：

- `statsLockImpl::GetLockedTables` 调用公开 `QueryLockedTables` 取得全量集合，再调用本文件的 `GetLockedTables` 筛选请求 ID。
- `statsLockImpl::GetTableLockedAndClearForTest` 直接返回 `QueryLockedTables` 的全量结果；与 Go 命名一致，但当前 Rust 实现和注释均表明它不清除持久化锁。
- `lock_stats.rs` 的 `AddLockedTables`、`AddLockedPartitions` 调用 `QueryLockedTablesWithExecutor`，据此避免重复插入锁。
- `unlock_stats.rs` 的 `RemoveLockedTables`、`RemoveLockedPartitions` 调用 `QueryLockedTablesWithExecutor`，据此只回灌和删除真实存在的锁。
- `query_lock_test.rs` 直接覆盖两个公开函数。

下游依赖为 crate 根定义的 `StatsSession::WithSession`、`RestrictedSQLExecutor::LockedTableIds` 与 `StatsError`，以及标准库 `HashMap`。RustCodeGraph 的 callee 结果显示 `QueryLockedTables` 调用 `QueryLockedTablesWithExecutor`；图索引没有把 trait 方法动态派发列为 callee，因此 `WithSession` 和 `LockedTableIds` 的边由函数体及 `lib.rs` trait 定义直接验证。

Cargo 文件中的旧 Go 包依赖均位于 `target.'cfg(any())'.dependencies`，即永不成立的兼容元数据区；当前查询文件实际只使用本 crate 类型和标准库。workspace 根与 `pkg/statistics/handle/Cargo.toml` 将该 crate 纳入统计 handle 门面。

## 错误处理与边界

`LockedTableIds` 的 `StatsError` 由 `?` 原样传播出 `QueryLockedTablesWithExecutor`；`WithSession` 返回的执行、会话取得或回调错误也由 `QueryLockedTables` 原样传播。错误路径不会返回部分集合。独立 Rust 测试用 `StatsError("query failed")` 验证了这一传播行为。

空存储返回空 map；重复行被 map 收集过程去重；空候选或完全未命中的候选由 `GetLockedTables` 返回空 map。输入候选的顺序不会保留，因为返回值是无序 `HashMap`。负数或不存在的 ID 没有额外校验，只按精确键匹配；合法 ID 约束属于上游元数据与存储层。

`rows.unwrap_or_default()` 是防御性兜底：按当前同步 `WithSession` 契约，成功回调会先写入结果；如果某个实现违反该约定并在未调用回调时返回 `Ok(())`，此处会静默返回空集合。扩展会话实现时应保持“成功意味着回调已执行”的隐含不变量，或显式收紧这一状态表达，避免把遗漏执行误报为空数据。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁守卫或事务对象。`SessionRef` 是 `Arc<dyn StatsSession>`，允许会话提供者被共享；具体并发互斥由 `StatsSession` 实现负责。测试实现 `QuerySession` 用 `Mutex<QueryExecutor>` 串行访问执行器，证明回调期间持有执行器独占可变访问权。

`QueryLockedTables` 传给 `WithSession` 的 `false` 明确表示该独立读取不要求包裹 KV 事务。相反，加锁和解锁入口在 `lock_stats.rs` 通过 `WithSession(true, ...)` 建立事务，再在同一执行器生命周期中调用 `QueryLockedTablesWithExecutor`，使“检查现有锁”和后续写操作共享外层事务语境。执行器借用只持续到回调返回，查询结果随后成为调用方独占的内存 map，不携带行游标或会话资源。

该文件不保证多个并发事务之间无竞态；原子性和隔离性取决于调用方是否使用事务会话及底层 `mysql.stats_table_locked` 操作。纯 `GetLockedTables` 可在各自不可变输入上并发执行。

## 与 Go 版本的对应关系

Rust `selectSQL`、`QueryLockedTables` 和 `GetLockedTables` 分别对应 `pkg/statistics/handle/lockstats/query_lock.go` 的同名常量与函数。两版都把所有 `table_id` 收集成集合，并通过候选 ID 与全集的交集返回锁定子集；空集合行为一致。

关键实现差异是：Go `QueryLockedTables(ctx, sctx)` 直接调用 `util.ExecRowsWithCtx(ctx, sctx, selectSQL)`，逐行读取第 0 列，并由调用方传入带有内部统计请求来源的 context；Rust `QueryLockedTables(session)` 不接收 context，也不直接调用 `ExecRestrictedSQL`，而是通过 `WithSession(false, ...)` 和 `LockedTableIds` 抽象查询。请求来源、SQL 执行与行解码必须由生产执行器适配层兑现，不能仅凭本文件的 `selectSQL` 常量推断已经使用该 SQL。

Rust 额外提供 crate 私有 `QueryLockedTablesWithExecutor`，供事务内加锁/解锁路径复用现有执行器。Go 调用点则直接把 `sessionctx.Context` 传给公开查询函数。Rust 使用 `HashMap<i64, ()>` 对应 Go 的 `map[int64]struct{}`；Rust `ids: &[i64]` 对应 Go 可变参数 `tableIDs ...int64`。

Go 测试验证 0/1/2 行、执行错误及统计前台内部请求 context；Rust 独立测试验证过滤、空结果、错误透传、全量 ID 和重复行去重。Rust 测试没有直接验证 SQL 文本或请求来源 context，这是当前覆盖差异。

## 扩展指南

- 若改变锁 ID 的读取协议，优先修改 `RestrictedSQLExecutor::LockedTableIds` 及其生产适配实现，并同步 `QueryExecutor` 测试替身；只有 SQL 契约改变时才同步修改 `selectSQL` 和 Go `query_lock.go`。
- 若新增过滤条件（租户、对象类型、分区范围等），先决定条件属于持久化查询还是内存交集。查询条件应进入执行器接口；纯候选过滤可扩展 `GetLockedTables`，但需保持表 ID/分区 ID 的兼容语义。
- 若调整会话或事务策略，必须同时检查 `statsLockImpl::GetLockedTables` 的非事务读取，以及 `AddLockedTables`、`AddLockedPartitions`、`RemoveLockedTables`、`RemoveLockedPartitions` 对执行器版本的事务内调用，避免嵌套会话或检查—写入窗口扩大。
- 功能回归测试应继续放在独立文件 `pkg/statistics/handle/lockstats/query_lock_test.rs`，不要嵌入生产文件；Go 对齐变化同步核对 `query_lock_test.go`。应覆盖成功、空集、重复 ID、底层错误、过滤交集，以及新增执行器/上下文契约。
- 返回类型若从 map 改为有序结构，会影响调用方的成员判断复杂度和无序语义；大锁集合路径应关注一次全量加载的时间与内存成本。错误包装变化则可能破坏当前 `StatsError` 精确透传测试与上层诊断。

## 验证依据

- 目标实现：`pkg/statistics/handle/lockstats/query_lock.rs`；确认 1 个常量和 3 个函数的签名、控制流及可见性。
- crate 边界：`pkg/statistics/handle/lockstats/lib.rs` 与 `pkg/statistics/handle/lockstats/Cargo.toml`；确认 trait、会话引用、错误类型、模块导出、Go 包映射及 `cfg(any())` 依赖区。
- Rust 上游：`pkg/statistics/handle/lockstats/lock_stats.rs`、`pkg/statistics/handle/lockstats/unlock_stats.rs`；确认公开会话查询与事务内执行器查询的各自调用点。
- Rust 测试：`pkg/statistics/handle/lockstats/query_lock_test.rs`；确认非事务标志、过滤交集、空存储、重复行去重和错误透传。
- Go 对照：`pkg/statistics/handle/lockstats/query_lock.go`、`pkg/statistics/handle/lockstats/query_lock_test.go`，并参考 `lock_stats.go`、`unlock_stats.go` 的调用点；确认 SQL、行读取、context 约束和集合语义。
- RustCodeGraph：索引覆盖目标目录；`query` 定位 `QueryLockedTablesWithExecutor`、Rust/Go `QueryLockedTables` 和 `GetLockedTables`；`callees QueryLockedTables` 确认 Rust 公开函数到执行器函数的调用边；`explore` 确认执行器函数被 Rust 加锁/解锁四个入口调用，公开函数被三个 Rust 单测调用。trait 动态派发边未由图工具解析，已用 `lib.rs` 和函数体补证。
- 结构验收使用任务指定命令，要求本文存在且恰好包含上述 11 个固定二级标题；本任务为纯文档分析，未运行 Cargo。

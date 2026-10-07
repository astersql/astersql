# `pkg/executor/lockstats/unlock_stats_executor.rs`

## 文件定位

本文件属于 `astersql-executor-lockstats` crate，是 `UNLOCK STATS` 的 Rust 执行器实现。crate 根 `pkg/executor/lockstats/lib.rs` 通过 `pub mod unlock_stats_executor` 导出本模块；`pkg/executor/lockstats/Cargo.toml` 又用 `package.metadata.porting.go-package = "pkg/executor/lockstats"` 明确其 Go 对照包。

当前 Rust 仓库中的定位需要分成两层理解：本文件已经提供可调用的 `UnlockExec` 及完整分支逻辑，但 RustCodeGraph 对该文件报告 `used by 0 files`，全仓 `rg` 也只找到模块自身的 Rust 定义，没有找到 Rust 执行器构造器或 `Next` 调用点。因此它目前是已导出的移植实现，而不是已经证实接入 Rust SQL 执行主链的算子。完整应用中的实际接线证据仍来自 Go 的 `pkg/executor/builder.go::buildUnlockStats`，它把 planner 的 `UnlockStats.Tables` 交给 Go `lockstats.UnlockExec`。

## 核心职责

`UnlockExec` 把语法/计划层给出的表与可选分区名转换成统计子系统理解的物理 ID 描述，并调用统计句柄解除锁定。它负责的是执行器层的编排，而不是直接修改统计系统表：

1. 从 `Runtime` 获取 `StatsHandle` 和 `InfoSchema`。
2. 拒绝空表列表。
3. 对“恰好一张表且显式指定分区”的请求调用 `RemoveLockedPartitions`；其他请求调用 `RemoveLockedTables`。
4. 将统计句柄返回的非空跳过信息追加为 warning，而不是把它升级为执行错误。

真正的增量回灌和锁记录删除位于 `pkg/statistics/handle/lockstats/unlock_stats.rs`：解锁前把 `mysql.stats_table_locked` 中的 `count`/`modify_count` 合并回 `mysql.stats_meta`，随后删除锁记录。本文件只通过 `StatsHandle` 抽象触发这一行为。

## 主要符号

- `pub struct UnlockExec { runtime: Arc<dyn Runtime>, Tables: Vec<TableName> }`：执行器状态。`runtime` 提供统计句柄、InfoSchema 和 warning 通道；`Tables` 保存待解锁的表以及可选分区名。字段公开，当前文件没有构造函数。
- `UnlockExec::Open(&mut self) -> Result<()>`：无状态初始化，始终返回 `Ok(())`。
- `UnlockExec::Close(&mut self) -> Result<()>`：无资源清理，始终返回 `Ok(())`。
- `UnlockExec::Next(&mut self) -> Result<()>`：唯一有业务行为的入口，完成依赖取得、输入校验、分支选择、元数据解析、句柄调用和 warning 转换。
- `UnlockExec::onlyUnlockPartitions(&self) -> bool`：当且仅当 `Tables.len() == 1` 且首表的 `PartitionNames` 非空时返回 `true`。

本文件没有模块级常量、trait、条件编译项或内部私有函数。`Error`、`Result`、`Runtime`、`TableName`、`populatePartitionIDAndNames` 和 `populateTableAndPartitionIDs` 均由相邻的 `lock_stats_executor.rs` 导入。

## 执行流程

`Next` 的顺序和短路规则如下：

1. 调用 `runtime.StatsHandle()`。若返回 `None`，立即返回 `Error("Unlock Stats: handle is nil")`；此时不会读取 InfoSchema。
2. 检查 `Tables`。空列表立即返回 `Error("Unlock Stats: table should not empty ")`，末尾空格与 Go 对照实现一致。
3. 调用 `runtime.InfoSchema()` 获取本次解析使用的目录对象。
4. 若 `onlyUnlockPartitions()` 为真，取唯一表，调用 `populatePartitionIDAndNames(table, &table.PartitionNames, info_schema)` 得到表 ID 和“分区 ID到小写输入名”的映射，然后调用 `StatsHandle::RemoveLockedPartitions`。传入的表显示名使用 `table.Schema.O` 与 `table.Name.O`，保留用户原始大小写。
5. 否则，调用 `populateTableAndPartitionIDs(&Tables, info_schema)` 展开每张表及其全部物理分区，再调用 `StatsHandle::RemoveLockedTables`。这个分支包括多表请求、没有显式分区的单表请求，以及任何多表中夹带分区名的输入。
6. 句柄返回的 `String` 为空时直接成功；非空时包装成 `Error` 并交给 `runtime.AppendWarning`，随后仍返回 `Ok(())`。

两个分支都用 `?` 传播元数据解析和统计句柄错误；错误发生后不会执行 warning 追加。

## 数据与状态

`UnlockExec` 自身不维护游标、输出行或“已经执行”标志。`Next` 接收 `&mut self`，但当前实现不修改字段，因此同一实例被重复调用时会重复向统计句柄提交解锁请求；是否幂等由下游锁状态处理决定，调用方不能从本文件推断只执行一次。

标识符同时保存原始形式 `CIStr.O` 和小写形式 `CIStr.L`。分区解析使用 `L` 做大小写不敏感匹配，并在映射中保留小写输入名；仅分区解锁时传给 warning 生成逻辑的表名特意使用 `O`。整表描述由 `populateTableAndPartitionIDs` 生成，`FullName` 和分区显示名使用小写形式。

共享辅助函数以 `HashMap<i64, ...>` 表示物理 ID 映射；本文件只借用这些映射完成同步调用，不缓存所有权。统计句柄由 `Arc<dyn StatsHandle>` 承载，InfoSchema 由 `Arc<dyn InfoSchema>` 承载。

## 依赖与调用关系

上游方面，`pkg/executor/lockstats/lib.rs` 导出本模块；未发现 Rust 调用者。Go 主链为 `pkg/executor/builder.go::buildUnlockStats` 构造 `pkg/executor/lockstats/unlock_stats_executor.go::UnlockExec`，执行框架随后调用其 `Next`。不能据此声称 Rust `UnlockExec` 已经接入相同 builder。

下游调用关系由源码和 RustCodeGraph 共同核对：

- `Next -> Runtime::StatsHandle`：取得可选统计锁句柄。
- `Next -> Runtime::InfoSchema`：取得表/分区元数据视图。
- `Next -> onlyUnlockPartitions`：选择分区级或整表级路径。
- 分区路径：`Next -> populatePartitionIDAndNames -> StatsHandle::RemoveLockedPartitions`。
- 整表路径：`Next -> populateTableAndPartitionIDs -> StatsHandle::RemoveLockedTables`。
- warning 路径：`Next -> Runtime::AppendWarning`。

`Runtime`、`StatsHandle` 和 `InfoSchema` 都定义在 `pkg/executor/lockstats/lock_stats_executor.rs`，是执行器与真正 session/domain/statistics 对象之间的本地适配边界。`pkg/statistics/handle/types/interfaces.rs::StatsLock` 给出了生产统计层对应的解锁接口，而 `pkg/statistics/handle/lockstats/unlock_stats.rs` 实现底层回灌和删除算法。当前未发现本地 `StatsHandle` trait 到该生产 `StatsLock` trait 的 Rust 适配实现，因此这条运行时接线也应视为尚未验证。

Cargo 边界方面，crate 的生产依赖只在 `cfg(windows)` 下声明 domain、executor internal exec、infoschema、parser AST、statistics handle types、table 和 chunk 等 workspace crate；本文件本身只直接使用标准库 `Arc` 和同 crate 的抽象类型。

## 错误处理与边界

显式边界包括：

- 无统计句柄：返回 `Unlock Stats: handle is nil`。
- 无目标表：返回 `Unlock Stats: table should not empty `。
- 分区模式下分区列表为空在分支判定前已被排除；若直接调用共享辅助函数，后者仍会返回 `partition list should not be empty`。
- 表不存在、非分区表指定分区、分区不存在等错误来自 `InfoSchema::TableByName` 或 `populatePartitionIDAndNames`，原样通过 `?` 传播。
- `RemoveLockedPartitions`/`RemoveLockedTables` 的执行错误原样传播，不会转换为 warning。
- “目标没有锁”“父表仍锁定而不能单独解分区”等可跳过情形由统计层返回消息，本文件把非空消息追加为 warning 后仍成功。

`pkg/statistics/handle/lockstats/unlock_stats_test.rs` 进一步证明底层约束：表级解锁只处理实际锁定的表/分区；分区级解锁会把分区增量同时回灌到分区和父表；父表仍锁定时跳过全部指定分区且不产生写调用；增量回灌和删除失败均作为错误传播。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或 I/O 资源。`Open`/`Close` 是对称的空操作；实际数据库事务和统计锁记录生命周期属于 `StatsHandle` 的实现，不在这里管理。

`Runtime`、`StatsHandle` 和 `InfoSchema` trait 均要求 `Send + Sync`，并通过 `Arc` 共享，因此类型边界允许跨线程持有。可是 `Next` 需要 `&mut self`，本文件没有提供内部同步，也没有证明同一个执行器实例可并发调用。扩展时不得把 trait 的 `Send + Sync` 误解为 `UnlockExec::Next` 自动具备并发安全或原子性。

资源顺序上，句柄先于输入表校验取得；InfoSchema 只在句柄存在且表列表非空后取得。warning 仅在下游成功返回消息后追加。底层 `unlock_stats.rs` 的多次回灌/删除是否处于同一事务，必须由具体 `StatsHandle`/`RestrictedSQLExecutor` 适配层保证，本文件没有事务控制证据。

## 与 Go 版本的对应关系

Rust `UnlockExec` 基本逐分支对应 `pkg/executor/lockstats/unlock_stats_executor.go`：两者都先取 StatsHandle、拒绝空表、用相同谓词识别“单表加显式分区”、复用同名解析辅助、调用相同语义的两个 Remove 方法，并把非空返回消息记为 statement warning。`Open` 和 `Close` 在两端也都是空操作。

保留的细节包括错误文本和仅分区解锁时的原始表名：Go 用 `fmt.Sprintf("%s.%s", table.Schema.O, table.Name.O)`，Rust 用 `format!("{}.{}", table.Schema.O, table.Name.O)`。这与 LOCK STATS 的 Rust 分区路径使用 `Schema.L`/`Name.L` 不同，是源码明确记录的刻意差异。

主要结构差异是 Rust 没有嵌入 Go 的 `exec.BaseExecutor`，而改用精简的 `Arc<dyn Runtime>`；Rust 的 `Next` 也没有 `context.Context` 和输出 `chunk.Chunk` 参数。Go 有 `var _ exec.Executor = &UnlockExec{}` 编译期接口断言，并已由 `executorBuilder` 构造；Rust 没有对应公共 Executor trait 实现或 builder 接线。故当前能确认的是核心分支语义移植，不是执行框架集成等价。

测试对照也不完全对称：同目录 Rust 测试 `lock_stats_executor_test.rs` 只覆盖两个共享元数据解析辅助，没有直接构造 `UnlockExec`；同目录 Go 测试同样主要覆盖辅助函数。底层解锁的 Rust 行为由 `pkg/statistics/handle/lockstats/unlock_stats_test.rs` 独立覆盖。

## 扩展指南

若修改请求分类规则，应从 `onlyUnlockPartitions` 入手，并同步检查 Go 同名方法；尤其要明确多表且带分区名是否仍应落入整表路径。若修改名称显示或大小写规则，应同时审查 `Next` 的 `Schema.O/Name.O`、`populatePartitionIDAndNames` 的分区名映射，以及 `populateTableAndPartitionIDs` 的小写整表显示名，避免改变 warning 文本兼容性。

若增加执行器行为，应优先为 `UnlockExec` 新建或扩展独立测试文件，而不是把测试内嵌到生产文件。测试至少应覆盖：句柄为空、表列表为空、两条分支选择、解析错误、句柄错误、空/非空消息，以及调用次数和参数中的原始大小写。与真实增量回灌相关的行为应继续放在 `pkg/statistics/handle/lockstats/unlock_stats_test.rs`，不要在执行器测试中复制底层算法。

若要接入 Rust 主链，需要新增可追溯的 Runtime/StatsHandle 生产适配和 executor builder/trait 接线，并验证 `Open`/`Next`/`Close` 生命周期；这超出本文件当前已实现事实。接线时还应决定如何传递取消上下文和 statement warning，并核对 `Cargo.toml` 当前仅在 Windows 声明相关 workspace 依赖的设计。

兼容风险集中在错误与 warning 文本、大小写展示和分支判定；正确性风险集中在把未锁目标当错误、在父表锁定时单独解分区，以及遗漏分区增量向父表的回灌；性能方面本执行器仅做元数据展开，但整表路径会枚举每张表的所有分区，下游数据库操作数量随实际锁定物理 ID 增长。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 7 个 Rust/Go 文件均被索引。
- RustCodeGraph `files --filter pkg/executor/lockstats` 与 `node --file pkg/executor/lockstats/unlock_stats_executor.rs`：确认目标文件 77 行、6 个符号、`used by 0 files`，并核对完整实现。
- RustCodeGraph `node --file pkg/executor/lockstats/lock_stats_executor.rs`：核对 `Runtime`、`StatsHandle`、`InfoSchema`、共享解析辅助及其错误边界。
- RustCodeGraph 的 `callees 'UnlockExec::Next'` 查询存在同名消歧限制，但结果明确列出 Go `Next -> onlyUnlockPartitions`；Rust 的精确调用边通过目标源码逐句复核。全仓 `rg` 进一步确认没有 Rust 侧 `UnlockExec` 构造/调用点。
- crate 与模块：`pkg/executor/lockstats/Cargo.toml`、`pkg/executor/lockstats/lib.rs`。
- Go 主链接线与语义：`pkg/executor/builder.go::buildUnlockStats`、`pkg/executor/lockstats/unlock_stats_executor.go`。
- 共享辅助测试：`pkg/executor/lockstats/lock_stats_executor_test.rs`、`pkg/executor/lockstats/lock_stats_executor_test.go`。
- 统计层接口、实现与独立测试：`pkg/statistics/handle/types/interfaces.rs::StatsLock`、`pkg/statistics/handle/lockstats/unlock_stats.rs`、`pkg/statistics/handle/lockstats/unlock_stats_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。最终使用任务指定命令验证目标文件存在且恰含 11 个固定二级标题，并人工检查没有把未接线能力写成已接入事实。

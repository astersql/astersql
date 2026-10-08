# `pkg/session/session.rs`

## 文件定位

本文件属于 `astersql-session` crate；crate 根 `pkg/session/lib.rs` 通过 `pub mod session` 暴露它。它不是 SQL 解析或执行器本身，而是一个精简的会话核心模型：集中定义语句结果摘要、事务抽象、运行时适配边界、表锁状态以及提交、回滚和关闭时的生命周期动作。

当前 Rust 实现只有 377 行，并未复刻同路径 `pkg/session/session.go` 的完整 `session`。仓库内对本文件 `session::doCommit`、`RollbackTxn` 和 `Close` 的直接调用证据集中在 `pkg/session/session_test.rs` 的 `session_parity` 测试模块；生产模块可经公开模块和公开类型使用，但代码搜索没有发现这些核心方法已接入完整 SQL 请求主链。因此应把它视为可执行、有测试的局部迁移边界，而不是 Go 会话层的等价替代。

## 核心职责

- 用 `Statement`、`StatementContext`、私有 `stmtRecord` 和 `StmtHistory` 保存语句文本及执行后摘要；`StmtHistory::Add` 追加记录，`Count` 返回数量。
- 用 `SessionTransaction` 隔离事务实现，用 `SessionRuntime` 隔离提交前选项、临时表提交、计划缓存清理、缓存表租约与资源关闭；`session` 只编排这些接口。
- 维护会话可观察状态：`Status`、`LastInsertID`、`LastMessage`、`AffectedRows`，以及客户端能力、连接 ID、命令、压缩算法与级别。
- 维护当前会话持有的表锁，并通过可选 `DDLOwnerManager` 判断本节点是否为 DDL Owner。
- 实现最小事务生命周期：写事务提交前执行只读保护和提交选项，回滚后清理重试状态，关闭时保证回滚尝试不会阻断其余资源释放。
- 用 `cachedTableRenewLease` 表达缓存表写租约的开始、停止与提交时间戳校验规则。

## 主要符号

- `Statement { sql }`：一条 SQL 文本的轻量记录，不包含 AST、计划或执行结果。
- `StatementContext`：保存 `LastInsertID`、`InsertID`、`Message`、`AffectedRows`。`session` 的结果读取方法都从这里取值。
- `StmtHistory`：内部持有 `Vec<stmtRecord>`；条目类型私有，因此外部只能追加和计数，不能读取或修改历史内容。
- `TableLockType` / `TableLockTpInfo`：分别表示 `None`、`Read`、`Write`、`ReadOnly` 和一张表的 `TableID`/锁类型组合。
- `RetryInfo`：保存回滚后应清理的预编译语句 ID；私有 `Clean` 只清空该向量。
- `SessionVars`：本文件所需的会话变量子集。它不是 Go `variable.SessionVars` 的完整移植。
- `TxnInfo`：事务的 `StartTS`、状态、写入条目数及当前/全部 SQL digest 的快照。
- `SessionTransaction: Send`：要求实现 `Valid`、`IsReadOnly`、`Info`、`Commit`、`Rollback`。它让 `session` 不依赖具体 KV 事务类型。
- `DDLOwnerManager: Send + Sync`：仅暴露 `IsOwner`。
- `SessionRuntime: Send + Sync`：本文件最重要的注入点。所有方法都是必需方法，没有默认空实现，避免生产适配器静默漏掉提交或资源清理行为。
- `session`：持有共享 `runtime`、独占可变 `txn`、类型擦除值表、上下文/进程摘要、`SessionVars`、表锁及可选 DDL Owner。类型名沿用 Go 命名，因此为小写且文件级允许非惯用命名。
- `cachedTableRenewLease`：持有运行时、表 ID、对应租约及停止标记；类型和方法为小写，限 crate 内部使用。

## 执行流程

1. 调用者构造 `session` 时必须提供 `Arc<dyn SessionRuntime>` 和 `Box<dyn SessionTransaction>`；本文件没有构造器，也不负责创建存储事务。
2. 表锁路径中，`AddTableLock` 逐项写入 `lockedTables`，但跳过 `ReadOnly`；释放可按锁列表、表 ID 列表或全部清空。查询返回 `(是否存在, 锁类型)`，缺失时固定为 `(false, None)`。
3. `doCommit` 先检查 `txn.Valid()` 和 `txn.IsReadOnly()`；无效或只读事务直接成功。普通外部 SQL 在 `RestrictedReadOnly` 为真时返回错误，而 `InRestrictedSQL` 可绕过该限制。
4. 可写事务通过只读保护后，`doCommit` 调用 `SessionRuntime::SetOptionsBeforeCommit`；成功才进入 `commitTxnWithTemporaryData`，最终由运行时决定如何提交事务和合并临时表数据。
5. `RollbackTxn` 仅在事务有效时调用事务回滚，但无论事务是否有效、回滚是否成功，都会随后执行 `cleanRetryInfo`。返回值保留原回滚结果。
6. `cleanRetryInfo` 在启用预编译计划缓存时，逐个调用 `DeletePreparedPlan`，之后总是清空 `DroppedPreparedStmtIDs`。
7. `Close` 忽略 `RollbackTxn` 的错误，继续按顺序关闭游标跟踪器和会话变量资源。这一顺序由 `close_rolls_back_before_releasing_session_resources` 回归测试固定。
8. 缓存表租约助手的 `start` 拒绝已停止实例，调用运行时批量续租，并要求返回租约数与表数完全一致；`commitTSCheck` 仅当提交时间戳严格小于每个租约时返回真；`stop` 只通知运行时一次。

## 数据与状态

`session` 自身没有内部锁。`runtime` 和 `ddlOwnerManager` 用 `Arc` 共享且 trait 要求 `Send + Sync`；事务用 `Box` 独占且只要求 `Send`。`values: HashMap<String, Box<dyn Any + Send + Sync>>` 提供类型擦除的会话附加值，但本文件没有访问方法；`currentCtx`、`processInfo` 和 `crossKS` 也仅作为状态字段保存。

`SessionVars` 是按值嵌入的快照式状态。`LastInsertID` 的不变量是优先返回非零 `StmtCtx.LastInsertID`，否则回退到 `InsertID`。`TxnInfo` 通过事务 trait 获取，并在 `StartTS == 0` 时过滤为 `None`。`GetAllTableLocks` 从 `HashMap` 克隆快照，返回顺序不稳定，调用者不能依赖顺序。

`cachedTableRenewLease.tables[i]` 与 `lease[i]` 必须一一对应；`start` 用长度检查维护这一不变量。`stop` 将 `stopped` 设为真，之后不能再次 `start`。该类型没有 `Drop` 实现，因此调用者必须显式执行 `stop`，否则本文件不会自动通知运行时终止续租。

## 依赖与调用关系

直接标准库依赖只有 `Any`、`HashMap` 和 `Arc`；错误统一使用 crate 根的 `SessionError` / `SessionResult`。虽然 `pkg/session/Cargo.toml` 声明了庞大的会话、规划、执行、存储和元数据依赖集合，本文件没有直接导入这些具体 crate，而是通过 `SessionTransaction`、`SessionRuntime` 和 `DDLOwnerManager` 做依赖倒置。

上游边界是 `pkg/session/lib.rs` 的 `pub mod session`。RustCodeGraph 将 `pkg/session/session.rs` 标为被大量文件依赖的公开模块，但针对核心方法的精确调用查询没有返回生产调用边；普通代码搜索确认 `doCommit`、本文件的 `RollbackTxn` 和 `Close` 目前由 `pkg/session/session_test.rs` 直接驱动。另一个 `pkg/session/txnmanager.rs` 也有 `RollbackTxn` 调用，但接收者是其自身的 session 抽象，不能据名称相同推断为本文件类型。

下游调用边可由方法体直接核验：`doCommit -> SessionTransaction::{Valid, IsReadOnly}`、`doCommit -> SessionRuntime::{SetOptionsBeforeCommit, CommitTxnWithTemporaryData}`；`RollbackTxn -> SessionTransaction::{Valid, Rollback}` 及 `cleanRetryInfo -> SessionRuntime::DeletePreparedPlan`；`Close -> RollbackTxn -> SessionRuntime::{CloseCursorTracker, CloseSessionVars}`；租约助手调用 `RenewCachedTableLeases` 和 `StopCachedTableLeaseRenewal`。

## 错误处理与边界

`doCommit` 使用 `?` 原样传播提交选项和实际提交错误。受限只读错误固定为 `SessionError::new("SQL is not allowed in restricted read-only mode")`；测试验证外部写事务被拒绝且提交次数保持为零。它不会在失败时自动回滚、使事务失效或清理重试信息，这些动作必须由更高层生命周期负责。

`RollbackTxn` 的清理不因回滚错误而跳过；但 `cleanRetryInfo` 返回 `()`，计划删除接口也不报告失败，因此本层无法表达或恢复计划缓存清理错误。`Close` 有意吞掉回滚错误，以保证游标和变量资源继续释放，也没有返回清理结果。

租约 `start` 有两种本地错误：停止后重启，以及运行时返回的租约数量和表数量不一致；运行时续租错误直接传播。`commitTSCheck` 对空租约返回真，这是迭代器 `all` 的自然结果；只有成功 `start` 后调用才具有完整的表级租约语义。

边界上，本文件没有 Go 版本中的权限复查、super-read-only、placement policy、断言失败诊断、failpoint、事务失效、事务指标/trace、临时表 staging 清理、表锁远端释放、prepared statements 全量撤回、内存/磁盘 tracker detach 等行为。不得以本文件的方法名推断这些能力已经存在。

## 并发与资源生命周期

trait 约束允许 runtime 与 DDL Owner 管理器跨线程共享，事务对象可以在线程间移动；但 `session` 的可变方法需要 `&mut self`，本文件不会并发保护 `sessionVars`、`lockedTables` 或 `values`。若上层共享整个会话，必须自行用互斥或线程绑定保证串行访问。

Rust 租约实现不直接创建线程或通道；并发续租被封装在 `SessionRuntime` 中。`cachedTableRenewLease::stop` 通过 `stopped` 保证幂等，但没有 RAII 兜底。Go 对照的 `start` 为每张表启动 goroutine、通过 channel 等待初次锁定，`stop` 关闭退出 channel；Rust 仅保留了等价的运行时协议和提交时间戳判定，没有在此文件复刻任务管理细节。

关闭顺序是“尝试回滚—关闭游标跟踪器—关闭会话变量资源”。测试覆盖了成功回滚时三项各执行一次；回滚失败后后两项仍执行的意图可由 `Close` 忽略结果的代码直接确认，但现有测试未构造失败回滚分支。

## 与 Go 版本的对应关系

同路径 `pkg/session/session.go` 是语义基准。Rust 的 `StmtHistory::{Add, Count}`、表锁增删查、`Status`、`LastInsertID`、`LastMessage`、`AffectedRows` 以及客户端/连接/命令/压缩字段 setter 保留了 Go 方法的基本形状；`AddTableLock` 同样跳过 `TableLockReadOnly`。

提交路径只保留 Go `doCommit` 的骨架：无效/只读短路、内部 SQL 绕过只读限制、提交前选项和含临时表数据的提交。Go 还会延迟使事务失效、重置 in-txn/disk-full 状态，复查动态权限和两种只读变量，检查 placement policy，启动真实缓存表续租，处理 assertion failure；这些都未在 Rust 本文件实现。Rust 的 `RestrictedReadOnly` 是会话字段，而 Go 从全局原子变量读取并进行权限例外判断。

Rust `cleanRetryInfo` 只按 ID 删除缓存计划并清空列表；Go 还处理 `Retrying`、prepared statement 对象、缓存 key、`IgnorePreparedCacheCloseStmt` 并移除 prepared statement。Rust `RollbackTxn` 也缺少 Go 的 trace、最后事务信息、事务失效、事务上下文清理、manager 回调和指标。

Rust `TxnInfo` 仅过滤零 `StartTS`；Go 在锁下复制快照，并可附加连接、用户、当前数据库和 MDL 关联表。Rust `Close` 只覆盖回滚、游标和变量资源；Go 完整释放表锁、advisory locks、统计收集器、绑定、计划缓存、prepared statements 和内存/磁盘 tracker。以上差异是当前迁移状态，不应由文档使用者自行补推。

## 扩展指南

- 新增提交规则时优先修改 `session::doCommit`；若需要访问具体存储或计划缓存能力，应向 `SessionRuntime` 增加明确、不可缺省的方法，并在 `pkg/session/session_test.rs` 的 `TestRuntime` 中实现和断言调用顺序。
- 扩展事务状态应同步 `SessionTransaction`/`TxnInfo`，避免把具体 KV 事务类型塞入 `session`。若新增 `TxnInfo` 字段，要测试 `StartTS == 0` 仍被过滤以及返回快照不泄露可变内部状态。
- 扩展重试清理需同时检查 Go `cleanRetryInfo` 的 `Retrying` 和 prepared-statement 语义。目前 Rust 逻辑是明显的子集，不能仅增加字段而不补相同行为和独立测试。
- 扩展关闭流程时保持“一个清理失败不阻断其余清理”的 Go 语义，并补充失败注入测试。若资源必须自动释放，考虑独立 RAII guard；不要给 `cachedTableRenewLease` 随意添加可能阻塞或失败但无法报告的 `Drop`。
- 表锁功能若接入真实 DDL unlock，必须区分本地 map 清理与远端解锁，验证 `ReadOnly` 跳过规则及无序快照；测试仍放在独立的 `pkg/session/session_test.rs`，不要内嵌到生产文件。
- 若目标是追平 Go，会话字段、提交/回滚和关闭三块应按可验证的增量迁移；每一步对照 `pkg/session/session.go` 并补独立 Rust 回归，不能用空 trait 实现或删减生命周期动作宣称等价。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件和 4,415 个 Go 文件；`files --filter pkg/session` 确认目标及相邻模块已索引；`node --file pkg/session/session.rs --offset 1 --limit 260` 与后续 261--377 行读取了完整目标文件。对 `doCommit` 的 `query --json` 区分出 Go 方法和 `session.rs::doCommit`；精确 callers/callees 查询未返回可用调用边，因此用方法体和限定 Rust 搜索补证，未把同名方法混为一谈。
- crate 与模块：`pkg/session/Cargo.toml` 证明包名为 `astersql-session`、库入口为 `lib.rs`、唯一 feature `nextgen` 不在本文件条件编译；`pkg/session/lib.rs` 的 `pub mod session` 证明公开模块边界。本目录未找到 `doc.go`，因此没有额外包级契约可读。
- Rust 测试：`pkg/session/session_test.rs` 的 `session_parity` 模块实现测试事务/runtime，并验证受限只读拒绝且不提交、回滚清理 `[7, 9]` 计划缓存 ID、关闭先回滚再关闭游标和变量资源。`pkg/session/test/common/common_test.rs::test_table_info_meta` 提供 `AffectedRows`/`LastInsertID` 的 SQL 行为对照，但其测试基础设施是否已接到本文件 `session` 未由调用图证实。
- Go 对照：`pkg/session/session.go` 180--420 行用于语句历史、会话字段、表锁和结果访问器；505--790 行用于事务信息、提交、缓存表租约和临时表数据；1025--1052 行用于回滚；3504--3555 行用于关闭资源。文档只把 Rust 中真实保留的子集标为已实现。
- 未运行 Cargo：本任务是纯文档分析，计划明确规定 Cargo 共享槽位不适用。交付前仅执行任务指定的文件存在与 11 个固定二级章节结构校验，并人工检查未把 Go 独有行为写成 Rust 已支持。

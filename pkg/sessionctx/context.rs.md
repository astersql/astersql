# `pkg/sessionctx/context.rs`

## 文件定位

[`context.rs`](context.rs) 是 `astersql-sessionctx` crate 的核心 API 边界。crate 根文件 [`lib.rs`](lib.rs) 通过 `mod context; pub use context::*;` 将本文件的全部公开项重新导出；[`Cargo.toml`](Cargo.toml) 表明该 crate 本身只有 `tokio-util = "0.7"` 这一运行时依赖，Go 包映射为 `pkg/sessionctx`。

本文件主要移植 Go [`context.go`](context.go) 的接口面：会话值存储、会话状态编解码、两级计划缓存、规划上下文、表锁、事务 future 和完整会话执行环境。它不是会话状态或事务的具体实现。文件中唯一包含业务执行逻辑的自由函数是 `ValidateSnapshotReadTS`；其余主体是 trait、关联类型、类型别名、上下文键和适配 Oracle 所需的最小接口。

应用侧的直接契约入口之一是 [`pkg/session/sessionapi/session.rs`](../session/sessionapi/session.rs)：`Session: sessionctx::Context`，并要求 `StateHandler: sessionctx::SessionStatesHandler`。这说明本文件位于“具体会话实现”之下、planner/executor/session API 使用者之上的公共能力层。

## 核心职责

1. 用 `Context: PlanContextCommon + TableLockContext` 汇总一条 SQL 会话在规划、执行、事务、锁、缓存、统计和生命周期方面必须提供的能力（`context.rs:99-264`）。
2. 用关联类型表达 Go 接口或指针类型，避免 `sessionctx` 反向依赖 storage、infoschema、executor、planner 等具体 crate（`PlanContextCommon` 与 `Context` 的关联类型列表）。
3. 用 `SessionStatesHandler` 隔离会话状态的编码/解码实现，用 `SessionPlanCache`、`InstancePlanCache` 分别描述会话级与实例级缓存（`context.rs:47-97`）。
4. 用 `BasicCtxType` 和三个稳定常量保存 Go `basicCtxType` 的键值与字符串协议（`context.rs:266-307`）。
5. 用 `ValidateSnapshotReadTS` 将快照时间戳校验严格委托给存储 Oracle，固定传入全局事务作用域，并原样返回 Oracle 错误（`context.rs:309-360`）。

## 主要符号

- `ExecutionContext = tokio_util::sync::CancellationToken`：Rust 侧可取消执行上下文。它承担 Go `context.Context` 在本 API 中实际需要的取消语义，但不是 Go context 的值/截止时间完整复刻（`context.rs:28`）。
- `GoError = Box<dyn Error + Send + Sync + 'static>`：跨 trait 的动态错误边界；`SharedAny = Arc<dyn Any + Send + Sync>`：计划缓存和上下文值使用的共享、线程安全类型擦除值（`context.rs:30-36`）。
- `ValueStoreContext`：`SetValue`、`Value`、`ClearValue` 与 `GetDomain`；它是 `PlanContextCommon` 的父 trait（`context.rs:40-45,105`）。键仅要求 `Display`，实现者必须自行决定字符串键的相等/存储规则。
- `SessionStatesHandler`：通过 `SessionContext`、`SessionStates` 关联类型抽象状态导入导出，两个方法均接收取消上下文并返回 `GoError`（`context.rs:52-69`）。
- `SessionPlanCache`：会话内 Get/Put/Delete/DeleteAll/Size/SetCapacity/Close 契约；`InstancePlanCache`：节点级 Get/Put/All/Evict、容量和内存上下限契约（`context.rs:71-97`）。实例缓存 `All` 返回共享只读项，调用者不应修改其内部对象。
- `PlanContextCommon`：规划/执行公共资源总线。它暴露 store、session vars、多版本 infoschema、KV/MPP client、session manager、普通与受限 SQL executor、表达式/ranger/PB 构建上下文、统计列使用、事务和脏内容查询等能力（`context.rs:99-141`）。
- `TableLockContext`：表锁的查询、批量加入、按锁或表 ID 释放、全部释放契约（`context.rs:143-156`）。
- `OracleFuture::Wait` 与 `TxnFuture<C>::Wait`：前者产生时间戳，后者持有待完成事务并在等待后返回有效事务；二者都以所有权/可变借用明确一次性或有状态等待边界（`context.rs:158-174`）。
- `Context`：完整会话环境 trait。除继承规划上下文和表锁外，还覆盖状态迁移、事务提交/回滚、schema/server/table/planner/DistSQL 上下文、计划缓存、语句级提交/回滚、TS future、统计、咨询锁、沙箱模式、索引使用采集器、游标、提交等待组和 trace context（`context.rs:176-264`）。
- `BasicCtxType`、`QueryString`、`Initing`、`LastExecuteDDL`：分别表示原始 SQL、bootstrap/upgrade 进行中、上一条是否为 DDL。`String`/`Display` 对未知整数稳定返回 `"unknown"`（`context.rs:266-307`）。
- `OracleOption { TxnScope }`、`GlobalTxnScope`、`SnapshotReadOracle`、`SnapshotReadStorage`：是快照读校验的最小本地端口（`context.rs:309-339`）。
- `ValidateSnapshotReadTS`：取得 `store.GetOracle()`，调用 `ValidateReadTS(ctx, read_ts, is_stale_read, OracleOption { TxnScope: "global" })`（`context.rs:341-360`）。

## 执行流程

本文件自身没有一个统一的运行循环；它规定了三条主要协作流程。

**会话 SQL/事务流程。** 上层持有实现了 `Context` 的对象，通过 `GetPlanCtx`、`GetDistSQLCtx`、`GetSessionVars` 等取得规划和执行资源；语句产生的 KV 修改先进入语句级缓冲。成功路径必须先 `StmtCommit` 再 `CommitTxn`，否则 Go 对照契约指出自上次 `StmtCommit` 起的修改会丢失；失败路径必须调用 `StmtRollback`，且 `for_pessimistic_retry` 只在悲观事务 DML 自动重试时为真（`context.rs:211-232`；Go `context.go:87-120`）。

**异步时间戳/事务流程。** 调用者先把 `OracleFuture` 交给 `PrepareTSFuture(ctx, future, scope)`，实现保存或启动时间戳获取；之后 `GetPreparedTxnFuture` 暴露挂起事务，`TxnFuture::Wait` 在给定会话和取消上下文中把它转换成有效事务（`context.rs:158-174,234-243`）。具体调度、重试和存储事务创建由实现者负责，本文件没有默认实现。

**快照读时间戳校验流程。** `ValidateSnapshotReadTS` 不缓存也不改写参数：取得 store 的 Oracle，传递同一个取消 token、`read_ts` 和 `is_stale_read`，构造 `TxnScope = GlobalTxnScope` 的临时 `OracleOption`，然后直接返回 `ValidateReadTS` 的结果（`context.rs:346-360`）。Go 注释说明这一显式检查用于不能等到存储 RPC 才验证的场景，例如不立即读数据的 `BEGIN`（Go `context.go:200-205`）；Rust 当前文件只提供校验函数，不决定何时调用。

## 数据与状态

- `BasicCtxType(i32)` 是可复制、可哈希的轻量键；数值 `1/2/3` 与 Go 常量保持稳定。新增值若未补 `String` 的 match 分支，会显示为 `unknown`（`context.rs:268-307`）。
- `SharedAny` 用 `Arc` 共享类型擦除值；读取方若需具体类型，必须在边界外安全 downcast。`Any + Send + Sync + 'static` 排除了借用短生命周期数据和非线程安全对象（`context.rs:36`）。
- `Context::GetBuiltinFunctionUsage` 返回可变 `HashMap<String, u32>`。Go 对照明确该 map 非线程安全；Rust 的 `&mut self` 限制同时可变访问，但实现若另行共享内部状态仍需自行同步（`context.rs:244`；Go `context.go:130-132`）。
- `InstancePlanCache` 的 `Size`、`MemUsage`、软/硬限制使用 `i64`；`SessionPlanCache::SetCapacity` 使用 `usize`，与 Go 的 `uint` 都表达平台宽度的非负容量，但跨平台序列化时不应假定固定 64 位（`context.rs:78-96`）。
- `Context` 的大量关联类型把具体对象留给下游绑定；`?Sized` 允许 trait object，`Option<&...>` 表达 session manager、SQL server、扩展和 process info 可缺失（`context.rs:182-197,215-263`）。
- `OracleOption` 只借用 `TxnScope` 字符串；`ValidateSnapshotReadTS` 构造的值只活到同步调用返回，不在本文件中持久化（`context.rs:313-316,352-359`）。

## 依赖与调用关系

**crate 内部。** [`lib.rs`](lib.rs) 是唯一装配入口并公开 re-export 本文件；两个独立测试模块也从 crate 根导入这些符号。运行时外部依赖仅为 `tokio-util`，用于 `CancellationToken`；`std::any::Any`、`Arc`、`HashMap` 和 `fmt` 分别承担类型擦除、共享所有权、使用统计和键格式化。

**上游使用者。** RustCodeGraph 的文件节点显示本文件被 19 个索引文件使用。可明确复核的生产契约边是 [`pkg/session/sessionapi/session.rs`](../session/sessionapi/session.rs) 中 `Session -> sessionctx::Context` 以及 `Session::StateHandler -> sessionctx::SessionStatesHandler`；[`pkg/session/sessionapi/lib.rs`](../session/sessionapi/lib.rs) 再导出 `Context`、`ExecutionContext`、`GoError`、`SessionStatesHandler`。定向搜索未发现当前生产 Rust 代码对 `ValidateSnapshotReadTS` 的调用，只有 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的两条测试调用，因此不能声称 Rust SQL 主链已经调用该校验。

**下游依赖。** trait 方法指向 storage、infoschema、client、MPP、SQL executor、expression/ranger、table context 等抽象资源，但通过关联类型而非 Cargo 依赖连接；真正实现必须在下游 crate 中绑定这些类型。`ValidateSnapshotReadTS` 只依赖 `SnapshotReadStorage -> SnapshotReadOracle -> ValidateReadTS` 这一条同步调用边。

**Go 对照调用面。** RustCodeGraph 显示 Go [`context.go`](context.go) 被 `pkg/domain/domain.go`、`pkg/sessionctx/stmtctx/stmtctx.go`、`pkg/sessionctx/variable/slow_log.go` 等文件使用。这是 Go 成熟实现的证据，不等同于 Rust 已完成相同生产接线。

## 错误处理与边界

- 所有可能失败的跨包方法统一返回 `Result<_, GoError>`；错误类型必须满足 `Send + Sync + 'static`。本文件不增加错误上下文、错误码或重试策略（`context.rs:32`）。
- `ValidateSnapshotReadTS` 使用尾表达式直接返回 Oracle 结果，因此成功、失败和错误文本均不被改写。迁移测试注入 `"read timestamp is in the future"` 并确认原样返回（`migration_aster_unit_test.rs:126-148`）。
- `RollbackTxn`、`StmtCommit`、`StmtRollback` 和 `Close` 没有错误返回值；实现需要遵守 Go 接口约定，不能通过本接口向调用者报告失败。文档不能据此推断这些操作一定成功。
- `BasicCtxType::String` 对任何未知值返回 `unknown` 而非 panic；这保证日志/格式化安全，但也会让未登记的新键失去可区分的字符串名（`context.rs:282-289`）。
- `GetSessionManager`、`GetSQLServer`、`ShowProcess`、`GetExtensions`、`GetPreparedTxnFuture` 明确允许 `None`；调用者必须处理缺失分支。
- trait 只定义语义边界，不验证缓存容量、内存限制、锁集合互斥、scope 字符串合法性或状态编码格式；这些属于具体实现和独立测试的责任。

## 并发与资源生命周期

- `ExecutionContext` 是可 clone 的 `CancellationToken`；取消状态可跨任务传播。迁移测试先 `cancel()` 再调用快照校验，证明本函数把同一取消状态传给 Oracle，但本函数本身不提前短路（`migration_aster_unit_test.rs:101-123`）。
- `SharedAny` 与 `NewStmtIndexUsageCollector`、`GetCursorTracker`、`GetCommitWaitGroup` 的 `Arc` 返回值支持跨所有者共享；共享不等于内部自动线程安全，具体关联类型与实现仍须满足其并发协议（`context.rs:36,260-262`）。
- `Context`/缓存/锁接口中的修改方法普遍要求 `&mut self`，从类型层面串行化同一可变借用范围；只读访问使用 `&self`。trait 本身没有 `Send`/`Sync` 父约束，不能据此宣称整个 session 可安全跨线程共享。
- `OracleFuture::Wait(self: Box<Self>)` 消耗 future，表达一次性完成；`TxnFuture::Wait(&mut self, ...)` 允许实现维护 pending/valid 状态。调用者应在成功后按实现约定停止重复等待（`context.rs:160-174`）。
- `SessionPlanCache::Close`、`DeleteAll` 和 `InstancePlanCache::Evict` 是显式资源回收点；本文件不提供析构兜底。会话结束路径应由具体实现负责调用 `Close`，并在测试中验证容量及共享缓存对象的生命周期。
- Go `GetCommitWaitGroup` 用于等待 async commit 与 secondary-lock cleanup 后台 goroutine（Go `context.go:160-161`）；Rust 仅保留关联类型和 `Arc` 契约，未在此文件定义任务启动或 join 行为。

## 与 Go 版本的对应关系

Rust [`context.rs`](context.rs) 与 Go [`context.go`](context.go) 的主要公开接口逐项对应：`SessionStatesHandler`、`SessionPlanCache`、`InstancePlanCache`、`Context`、`TxnFuture`、三个 context key 和 `ValidateSnapshotReadTS` 均保留。Rust 额外拆出了 `ValueStoreContext`、`PlanContextCommon`、`TableLockContext`、`OracleFuture`、`SnapshotReadOracle`、`SnapshotReadStorage`，用关联类型和最小 trait 代替 Go 的包级接口、具体指针与外部依赖。

关键语义保持如下：

- `Context` 仍组合 Go `planctx.Common` 与 `tablelock.TableLockContext` 的完整方法集；Rust 通过父 trait 组合表达（Rust `context.rs:182`；Go `context.go:79-87`）。
- 语句级提交必须发生在事务提交之前，悲观重试标志的适用范围不变（Rust `context.rs:225-232`；Go `context.go:108-120`）。
- `BasicCtxType` 的三个数字、字符串和未知值回退与 Go 相同，并由 Rust [`context_test.rs`](context_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖，对照 Go [`context_test.go`](context_test.go)。
- 快照读校验仍把全部参数交给 Oracle 并固定使用 global scope；Rust 把 `oracle.Option` 表达为本地 `OracleOption`（Rust `context.rs:309-360`；Go `context.go:200-205`）。

需要注意的表达差异：Go `context.Context` 被收窄为 `CancellationToken`；Go `any` 被约束为 `Any + Send + Sync + 'static`；Go 的 nil 接口/指针主要变成 `Option<&T>`；Go `SessionPlanCache.Put` 无返回值而实例缓存 `Put` 返回 bool，这一区分在 Rust 中保持。Rust 当前没有在本文件提供任何具体 `Context` 实现，且定向搜索没有证明 `ValidateSnapshotReadTS` 已接入生产 Rust 调用链，因此迁移完成度应按“公共契约已定义、校验逻辑已有测试、具体生产绑定由下游负责”描述。

## 扩展指南

- **新增会话能力：** 先确认它属于 `PlanContextCommon`、`TableLockContext` 还是 `Context`。修改 trait 会破坏所有实现者；应同步 Go `pkg/sessionctx/context.go` 的对应增量，并在独立测试文件中加入编译期契约或行为测试，不要把测试内嵌到 `context.rs`。
- **新增上下文键：** 在 `BasicCtxType` 常量区分配稳定且不重复的整数，同时扩展 `String` match；同步 [`context_test.rs`](context_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 Go [`context_test.go`](context_test.go)。风险是持久化/日志兼容和未知值误显示为 `unknown`。
- **扩展快照校验参数：** 优先修改 `OracleOption`、`SnapshotReadOracle` 和 `ValidateSnapshotReadTS` 的最小接口，再更新 RecordingOracle 测试以逐项断言转发。不能静默改变 `GlobalTxnScope`，否则会偏离 Go 快照读契约。
- **接入生产调用链：** 在真正需要提前校验 read TS 的会话/事务入口调用 `ValidateSnapshotReadTS`；新增独立回归测试覆盖成功、未来时间戳失败、stale-read 标志与取消状态。当前仅迁移测试直接调用，接线前不能删除这些测试。
- **实现缓存或 Context：** 保持实例缓存 `All` 的只读约定、`StmtCommit`/`CommitTxn` 顺序、咨询锁的会话归属以及 `GetCommitWaitGroup` 的后台任务回收语义。性能风险主要来自 `SharedAny` 的动态 downcast、`Arc` 引用计数、全量 `All` 复制和过宽锁粒度。
- **依赖边界：** 尽量继续使用关联类型和小 trait，避免把 planner/executor/storage 的具体 crate 引回 `astersql-sessionctx`，否则容易形成循环依赖并扩大基础 crate 的编译面。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`pkg/sessionctx/context.rs` 已索引。
- RustCodeGraph `files --filter pkg/sessionctx`：确认目标 Rust/Go 文件、独立测试和相邻模块均在索引中。
- RustCodeGraph `node --file pkg/sessionctx/context.rs --offset 1 --limit 500`：读取完整 360 行源码，并报告该文件被 19 个索引文件使用。
- RustCodeGraph `node --file pkg/sessionctx/context.rs --symbols-only`：核对 11 个 trait（含辅助边界）、`BasicCtxType`、`OracleOption`、`ValidateSnapshotReadTS` 及全部方法符号。精确 `callers ValidateSnapshotReadTS` 查询在本地长时间无输出而被终止，因此调用者结论改由定向引用搜索复核，不把图缺失推断为无调用。
- RustCodeGraph `node` 读取 [`context.go`](context.go)、[`context_test.rs`](context_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；普通文件读取核对 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、Go [`context_test.go`](context_test.go) 和 [`pkg/session/sessionapi/session.rs`](../session/sessionapi/session.rs)。
- 定向 `rg` 确认 `Session: sessionctx::Context`、`StateHandler: sessionctx::SessionStatesHandler`；确认 `ValidateSnapshotReadTS` 在目标文件外仅有迁移测试的两处 Rust 调用；确认两个 Rust 独立测试覆盖键字符串，迁移测试额外覆盖所有校验参数、取消状态和错误透传。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务规定的 11 章节结构命令；结构结果见任务交付记录。

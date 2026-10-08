# `pkg/util/mock/context.rs`

## 文件定位

`context.rs` 是 `astersql-util-mock` crate 的测试会话上下文实现。crate 入口 `pkg/util/mock/lib.rs` 以 `mod context; pub use context::*;` 导出本文件的公开项，并由 `pkg/util/mock/fortest.rs::NewContext` 在测试侧构造 `Context`。它模拟 Go `pkg/util/mock/context.go::Context` 的常用表面，使规划器、执行器、会话变量和存储相关测试可以在不启动完整 TiDB/AsterSQL 会话的情况下取得会话状态、规划上下文和事务句柄。

这个类型不是正式会话实现：Go 的 `NewContextDeprecated` 明确要求新生产代码使用真实 Context；Rust 也保留同名遗留入口 `NewContextDeprecated`，而常规测试入口位于独立文件 `fortest.rs`。RustCodeGraph 显示本文件被 56 个文件引用，直接测试使用包括 `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_test.rs`、`pkg/planner/core/casetest/physicalplantest/physical_plan_test.rs`、`pkg/sessionctx/variable/tests/session_test.rs`、`pkg/table/tblsession/table_test.rs` 和 `pkg/dxf/importinto/scheduler_test.rs`。

## 核心职责

1. `MockSessionVars` 包装正式的 `variable::session::SessionVars`，并补充当前 Rust 测试需要的 chunk、分页、Ranger、TiFlash、资源组等字段；`Default` 建立与 Go 构造器一致的关键默认值。
2. `wrapTxn` 在 `Empty`、`Real(Box<dyn kv::Transaction>)`、`Fake(fakeTxn)` 三种事务状态以及一个可选 `OracleFuture` 之间转换，支持立即建事务和“先准备时间戳、后开始存储事务”两条路径。
3. `Context` 保存会话变量、执行/取消上下文、Store、InfoSchema、Domain、schema validator、会话管理器、计划缓存、上下文键值以及 DDL Owner/沙箱标志，并从这些状态派生 `DistSQLContext`、`RangerContext` 和 `BuildPBContext`。
4. 对完整 `sessionctx.Context` 中当前测试不需要的能力提供显式边界：SQL 执行与状态编解码返回 `MockError::NotSupported`；表锁、咨询锁、使用统计等返回固定值或执行空操作。
5. 通过全局 `SetMockInfoschemaFactory`/`MockInfoschema` 提供可注入的 InfoSchema 构造钩子，允许测试用自身的元数据实现替代真实 Domain 路径。

## 主要符号

- `SharedAny = Arc<dyn Any + Send + Sync>`：跨线程共享且可运行时向下转型的占位句柄，用于 Domain、validator、InfoSchema、session manager 和 plan cache。
- `MockError` / `MockResult<T>`：区分不支持操作、无效事务、pending 事务缺少 Store、字符串错误和透明传播的 KV 错误。
- `MockSessionVars`：公开的 mock 会话状态。`SetInTxn`、`InTxn`、`SetSystemVar`、`GetSystemVar` 委托给内部正式 `SessionVars`；其余公开字段供测试直接配置。
- `TransactionState`：私有事务枚举；`Empty` 无事务，`Real` 持有真实 KV 事务，`Fake` 持有无 Store 的兼容事务。
- `wrapTxn`：事务门面。`Txn`、`PrepareTSFuture` 和 `GetPreparedTxnFuture` 暴露它，`Wait` 将 pending future 落成带 `StartTS` 的真实事务；`Valid`、`StartTS`、`CacheTableInfo`、`GetTableInfo` 根据当前状态分派。
- `fakeTxn`：遗留测试兼容对象，固定 `StartTS=1`、始终有效、读返回空 `ValueEntry`、提交/回滚成功，并仅在内存中保存表元信息和磁盘满选项。
- `DistSQLContext`、`RangerContext`、`BuildPBContext`：从 `MockSessionVars` 复制的轻量快照。`DistSQLContext` 的相等性以限流器容量比较，而非比较 `Arc` 身份。
- `SetMockInfoschemaFactory` / `MockInfoschema`：对 `OnceLock<RwLock<Option<InfoSchemaFactory>>>` 的写入与读取入口；工厂接收表信息切片，返回 `SharedAny`。
- `Context`：主类型。关键入口包括 `Txn`、`CommitTxn`、`RollbackTxn`、`PrepareTSFuture`、`GetDistSQLCtx`、`GetRangerCtx`、`GetBuildPBCtx`、`GetInfoSchema`、`SetValue`/`Value`/`ClearValue`、`Cancel` 与 Domain 绑定方法。
- `newContext` / `NewContextDeprecated`：前者构造默认实例且仅 crate 内可见；后者是供遗留调用者使用的公开别名。测试通常经 `fortest.rs::NewContext` 调用前者。

## 执行流程

构造流程从 `fortest.rs::NewContext` 或 `NewContextDeprecated` 进入 `newContext`。它建立空事务、空外部句柄、可取消的 `ExecutionContext`、空键值表和 `MockSessionVars::default()`。默认会话变量设置 `InitChunkSize=2`、`MaxChunkSize=32`、UTC、分页默认值、`EnableChunkRPC=true`、`QueryCopStoreLimit=15`，并向正式变量表写入 `max_allowed_packet=67108864` 与 `character_set_connection=utf8mb4`。

普通事务激活由 `Context::Txn(true)` 驱动：若 `wrapTxn::validOrPending` 为假，则调用 `newTxn`。有 Store 时，已有有效事务会先提交，再调用 `Storage::Begin(&[])` 保存为 `Real`；无 Store 时转成 `Fake`。`Txn(false)` 不激活事务，只返回当前包装器。提交路径先把 `Context::level` 下推到事务，再在事务有效时提交，最后清除 `SessionVars::InTxn`；回滚路径忽略回滚错误并同样清除 `InTxn`。

延迟时间戳路径由 `PrepareTSFuture` 把事务状态重置为 `Empty` 并保存 future。此时 `validOrPending` 为真、`pending` 为真，而 `Valid` 仍为假。调用 `wrapTxn::Wait` 后先等待 future；若成功取得时间戳但没有 Store，返回 `MissingStore`；有 Store 时以 `TxnOption::StartTS(start_ts)` 开始真实事务并转入 `Real`。`pkg/util/mock/migration_aster_unit_test.rs` 分别验证了缺 Store 的错误和 StartTS=42 的真实建事务路径。

规划/执行上下文按需生成：`GetDistSQLCtx` 复制 DistSQL/TiFlash 字段，并由 `kv::NewQueryCopStoreLimiter(QueryCopStoreLimit)` 为每次调用创建查询级限流器；`GetRangerCtx` 复制 NULL 点与前缀索引单扫标志；`GetBuildPBCtx` 同时反映 Store 是否能提供 Client 以及 PB 下推相关标志。`GetInfoSchema` 在本地缓存为空时才调用全局工厂，后续复用缓存。

## 数据与状态

`Context` 的可变状态分为五组：事务状态 `txn` 与磁盘满级别 `level`；会话状态 `session_vars` 与 `execution_ctx`；元数据/服务句柄 `dom`、`schema_validator`、`Store`、`session_manager`、`info_schema`、`plan_cache`；类型擦除的本地键值 `values`；以及布尔标志 `in_sandbox_mode`、`is_ddl_owner`。`SetValue` 将键统一为字符串并把值保存为 `Box<dyn Any>`，因此读取还要求 `Value::<T>` 的目标类型与写入类型相同，否则返回 `None`。

`GetDistSQLCtx`、`GetRangerCtx`、`GetBuildPBCtx` 都返回新值而不是借用内部缓存；修改 `MockSessionVars` 只会影响之后创建的快照。`go_merge_30_test.rs` 证明正数 `QueryCopStoreLimit` 产生对应容量的限流器、不同调用不共享同一 `Arc`，零值则不创建限流器。

`SetMockInfoschemaFactory` 操作进程级全局状态；`Context::info_schema` 是实例级懒加载缓存。直接调用 `SetInfoSchema` 会覆盖实例缓存，但不会修改全局工厂。`ResetSessionAndStmtTimeZone` 在当前 Rust 实现中只更新 `MockSessionVars::TimeZone`；Go 版本还同步更新 `StmtCtx` 时区，这是当前模型差异。

## 依赖与调用关系

crate 边界由 `pkg/util/mock/Cargo.toml` 定义：直接依赖 `astersql-kv`、`astersql-sessionctx`、`astersql-sessionctx-vardef`、`astersql-sessionctx-variable`、`astersql-util-sli`，并使用 `chrono` 表示固定时区、`thiserror` 定义错误、`tokio-util` 供 crate 的其他 mock 组件使用。测试依赖 `astersql-store-mockstore-mockstorage` 和公共测试初始化 crate。

主要下游调用为：`MockSessionVars` 调用正式 `SessionVars`；`Context::newTxn` 与 `wrapTxn::Wait` 调用 `kv::Storage::Begin`；提交/回滚和表信息缓存分派给 `kv::Transaction`；`GetClient`/`GetMPPClient` 委托 Store；`GetDistSQLCtx` 调用 `kv::NewQueryCopStoreLimiter`；全局变量访问委托 `variable::GetSysVar`/`SetSysVar`；`GetTxnWriteThroughputSLI` 构造默认 SLI。

RustCodeGraph 对精确符号的结果确认：`context.rs::GetDistSQLCtx` 调用本文件 `GetSessionVars` 并实例化 `DistSQLContext`；crate 入口通过 `lib.rs` 导出本模块；文件级反向边覆盖 56 个文件。仓库直接引用进一步表明主要上游是 Rust 测试与测试辅助代码，例如 planner casetest、executor importer、session variable、table session 和 DXF scheduler 测试。Go 侧 `NewContextDeprecated` 仍被 mock coprocessor 等遗留路径调用，这解释了保留兼容构造器的原因，但不能据此推断 Rust 已接线到这些 Go 调用者。

## 错误处理与边界

`wrapTxn::Wait` 对完全无效的状态返回 `InvalidTransaction`；future 的错误被转成保留文本的 `Message`；pending 状态缺 Store 返回 `MissingStore`；`Storage::Begin`、真实事务提交和回滚错误通过 `MockError::Kv` 透明转换。`newTxn` 会传播提交旧事务或开始新事务的错误。`CommitTxn` 无有效事务时成功；`RollbackTxn` 明确丢弃回滚错误，这是 mock 的既有契约而非生产事务策略。

`Execute`、`ExecuteStmt`、`ParseWithParams`、受限 SQL 入口、`ExecuteInternal`、`EncodeStates` 和 `DecodeStates` 固定返回 `NotSupported`。`GetSQLServer<T>` 在 Domain 未绑定或类型不匹配时 panic，以模拟 Go 类型断言失败；独立 Rust 测试验证了未绑定时的 panic 文本。系统变量不存在时 `GetGlobalSysVar` 返回带变量名的 `Message`。

其他边界是刻意的测试桩：脏内容与跨 Keyspace 固定为假；表锁查询固定未锁；咨询锁获取成功、查询为 0、释放成功；统计、语句提交/回滚、锁变更、usage 上报和 `Close` 无副作用；扩展、语句统计、游标、等待组等返回 `None`。调用者不能把这些默认值当作真实会话语义。

## 并发与资源生命周期

`SharedAny` 和 Store 使用 `Arc`，允许句柄跨线程共享；InfoSchema 工厂使用 `OnceLock<RwLock<...>>` 全局初始化并同步读写。锁中毒时通过 `into_inner()` 继续使用最后的内部值，而不是传播 panic。工厂回调要求 `Send + Sync + 'static`，返回值也要求 `Send + Sync`。

单个 `Context` 的事务、键值表、会话变量和缓存通过 `&mut self` 修改，没有内部锁，不应在多个任务间并发修改。`ExecutionContext` 可克隆；所有克隆共享取消状态，`Cancel` 会让先前由 `GoCtx` 取得的克隆观察到取消，相关测试已覆盖。

真实事务由 `Box<dyn kv::Transaction>` 独占。`newTxn` 在替换有效真实事务前先提交；`PrepareTSFuture` 会直接清空当前事务状态并换成 pending future；`Wait` 消费 future 并转移到真实事务。`Close` 是空实现，实例销毁依赖 Rust 的所有权与 `Drop`，没有额外后台任务或显式资源清理协议。

## 与 Go 版本的对应关系

Rust 的 `Context`、`wrapTxn`、`fakeTxn`、构造器、事务入口、键值存取、规划上下文生成以及大量默认接口逐项对应 `pkg/util/mock/context.go`。关键一致点包括默认 chunk 大小和系统变量、无 Store 时的 StartTS=1 假事务、pending future 选择 StartTS、提交/回滚后清除 `InTxn`、SQL 接口返回 “Not Supported”、DDL Owner/沙箱开关和默认锁行为。`mock_test.rs` 对齐 Go `mock_test.go::TestContext`，`migration_aster_unit_test.rs` 补充覆盖迁移后的事务与边界语义。

Rust 不是 Go `Context` 的完整等价实现。Go 类型直接实现 `sessionctx.Context`、`planctx.PlanContext` 和 SQL executor 接口，并持有完整 `SessionVars`、表达式/表上下文、内存/磁盘 tracker 和语句上下文；Rust 当前是具名方法集合与轻量快照，没有在本文件声明这些 trait 实现。Go `GetInfoSchema` 优先检查 snapshot/事务 InfoSchema，Rust只使用实例缓存与工厂；Go `DistSQLContext`/`RangerContext`/`BuildPBContext` 携带更多错误、统计、表达式和 tracker 句柄；Rust仅保留已迁移测试需要的字段。Go 的 fake transaction 在读时记录危险用法警告，Rust静默返回空值。因此扩展时应以调用者实际契约为准，不能把当前缩减面推广为生产等价性。

## 扩展指南

新增会话变量时，先判断它属于正式 `variable::session::SessionVars` 还是仅 mock 需要的补充字段；默认值应对照 Go `newContext`、`vardef` 常量和真实调用者，再同步 `MockSessionVars::default`。若字段影响规划或执行，还要更新对应的 `GetDistSQLCtx`、`GetRangerCtx` 或 `GetBuildPBCtx` 快照及其独立测试，避免内部值已更新但下游仍读取旧默认。

扩展事务行为应围绕 `TransactionState`、`wrapTxn`、`Context::newTxn`、`PrepareTSFuture`、`CommitTxn` 和 `RollbackTxn` 保持状态不变量：pending 必须是 `Empty + Some(future)`，成功等待后必须是 `Real + None`，无 Store 的兼容路径才使用 `Fake`。需要真实存储语义的测试应使用 `mockstorage_crate`，不要增强 `fakeTxn::Get` 去伪造业务数据；新增回归放在独立的 `*_test.rs`（优先扩展 `migration_aster_unit_test.rs` 或创建同目录独立测试文件），不要把测试嵌入本源文件。

扩展 InfoSchema 钩子时要处理全局工厂的测试隔离：测试应保存/清理工厂，避免并行测试互相污染。若为现有桩接口增加真实行为，应同步 Go 对照与 trait/调用方需求，明确原固定返回值是否被测试依赖，并评估额外锁、分配和 Store 调用的性能风险。新生产代码不应依赖 `NewContextDeprecated`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/mock` 确认 Rust/Go 源及独立测试；`node --file pkg/util/mock/context.rs` 完整核对 993 行和 148 个符号；`node` 核对 `lib.rs`、`fortest.rs`、Go 对照与测试；文件级结果报告目标被 56 个文件使用。
- 关键图查询：`query MockSessionVars`、`query wrapTxn` 定位 Rust/Go 对应符号；`callees Context::GetDistSQLCtx` 确认读取 `GetSessionVars` 并构造 `DistSQLContext`；对 `Txn`、`Wait`、InfoSchema 和 future 入口执行了 callers/callees 查询。常见方法名存在跨仓库重名，文档只采用能由目标文件、精确路径或直接引用复核的边。
- 源与边界：`pkg/util/mock/context.rs`、`pkg/util/mock/lib.rs`、`pkg/util/mock/fortest.rs`、`pkg/util/mock/Cargo.toml`。
- Go 对照：`pkg/util/mock/context.go`（接口断言、事务状态、构造默认值、桩返回和遗留构造器）与 `pkg/util/mock/mock_test.go`（键值生命周期及构造基准）。
- Rust 测试：`pkg/util/mock/mock_test.rs` 覆盖 Set/Value/Clear；`pkg/util/mock/migration_aster_unit_test.rs` 覆盖默认值、标志、假事务、future、取消、不支持接口及 Domain panic；`pkg/util/mock/go_merge_30_test.rs` 覆盖查询级 Store 限流器。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行固定 11 章节结构命令，并人工检查文档没有把桩行为描述为生产能力。

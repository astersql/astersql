# `pkg/ttl/session/session.rs` 逻辑说明

## 文件定位

`pkg/ttl/session/session.rs` 是 `astersql-ttl-session` crate 的唯一业务实现文件；`pkg/ttl/session/lib.rs` 声明 `pub mod session` 并把其中符号全部重新导出。根 `Cargo.toml` 以 `facade_ttl_session` 指向该 crate，因此它目前承担 TTL 专用会话 API 的 Rust 门面：在宿主提供的 `SessionContext` 与 `SqlExecutor` 之上统一 SQL 执行、显式事务、时区读取、语句打断和连接复用控制。

该 crate 当前仍处于隔离移植阶段。`pkg/ttl/session/Cargo.toml` 的正常 `[dependencies]` 为空，面向真实 TiDB 子系统的依赖只列在永不成立的 `target.'cfg(any())'.dependencies` 中；仓库搜索也未发现生产 Rust 代码直接构造 `TtlSession` 或调用本文件的 `new_session`。因此，本文件已有可由独立测试运行的行为模型，但不能据此声称它已经接入 TTL worker 的生产主链。当前生产侧的 TTL worker 抽象另见 `pkg/ttl/ttlworker/session.rs`，两者尚未在代码中桥接。

## 核心职责

- `TtlSession` 将 `SessionContext` 提供的存储、会话变量、最新/事务 InfoSchema 和内部 SQL 执行器收束为 `Session` trait。
- `execute_sql` 为每次内部 SQL 强制设置 `RequestSource::Ttl`，临时写入 TTL job ID，执行后恢复池化会话原状态，并把可选结果集一次 drain 为行数组。
- `run_in_transaction` 根据 `TxnMode` 发出乐观或悲观 `BEGIN`，运行调用方回调，再 `COMMIT`；任一步失败或 panic 展开时由 `TransactionCleanup::drop` 尝试独立超时回滚。
- `global_time_zone`、`kill_statement`、`now` 和 `avoid_reuse` 分别提供全局时区解析、当前语句中断、本地时区时间源以及连接淘汰钩子。
- `ExecutionContext`、`PhaseTracer`、`SessionVariables`、SQL 值/行/结果集 trait 组成独立、可测试的宿主边界，而非直接依赖完整 TiDB 类型。

## 主要符号

- `TxnMode::{Optimistic, Pessimistic, Unknown(i32)}`：决定事务开始语句；`Unknown` 使 `run_in_transaction` 返回 `SessionError::UnknownTransactionMode`。
- `SessionError`：区分 SQL、全局变量、非法时区和未知事务模式错误，并实现 `Display`/`Error`。
- `ExecutionContext`：持有请求来源、共享取消位、可选截止时间、可选 `PhaseTracer` 与当前执行的 TTL job ID。`with_timeout`、`with_phase_tracer`、`with_ttl_job` 是构造入口，`cancel`/`is_done` 暴露取消状态。
- `Phase` 与 `PhaseTracer`：用 `Mutex<Phase>` 记录 `BeginTransaction`、`CommitTransaction` 或 `Other`；事务结束时恢复进入前的相位。
- `SqlValue`、`Row`、`RecordSet`、`SqlExecutor`：定义内部 SQL 的参数、结果和宿主执行接口。`RecordSet::drain` 与 `close` 的资源协议由 `execute_sql` 驱动。
- `Storage`、`MetaOnlyInfoSchema`：当前仅含 `Send + Sync` 约束的占位 trait，用于保持会话接口形状。
- `TimeZone::parse`：识别 UTC、固定偏移 `±HH:MM` 和非空命名时区；命名时区只保留名称，不在本 crate 中解析真实偏移。
- `SessionVariables`：通过 `Mutex` 保存会话/全局时区、location 与 TTL job ID，通过 `AtomicI32` 保存 killed 标志。
- `SessionContext`：宿主注入边界；提供存储、变量、最新 InfoSchema、事务 InfoSchema 和 SQL 执行器。
- `Session`：TTL 调用方使用的对象安全接口；`TtlSession` 是其实现，`new_session` 返回 `Arc<dyn Session>`。
- `TransactionCleanup`：事务作用域守卫；`success == false` 时在 `Drop` 中发出 `ROLLBACK`，并总是恢复旧相位。

## 执行流程

构造路径从 `new_session` 进入 `TtlSession::new`：保存 `SessionContext`，提前缓存 `context.sql_executor()`，并把 `avoid_reuse` 回调放入互斥锁，最终以 `Arc<dyn Session>` 暴露。

执行 SQL 时，`TtlSession::execute_sql` 克隆调用方 `ExecutionContext` 并只在克隆上把 `request_source` 改为 `Ttl`。随后它从 `SessionContext` 取得 `SessionVariables`，用 `replace_ttl_job_id` 暂存旧 job ID；局部 `RestoreJobId` 守卫保证正常返回、错误返回或 panic 展开均恢复旧值。`SqlExecutor::execute_internal` 返回 `None` 时结果为空；返回 `RecordSet` 时以批大小 8 调用 `drain`，然后无论 drain 是否成功都尝试 `close`。最终返回 drain 的结果，close 错误被忽略。

事务路径由 `TtlSession::run_in_transaction` 驱动：先抓取 tracer 与旧相位并创建 `TransactionCleanup`，再进入 `BeginTransaction` 相位；乐观模式执行 `BEGIN OPTIMISTIC`，悲观模式执行 `BEGIN PESSIMISTIC`，未知模式立即报错。BEGIN 成功后进入 `Other`，调用回调；回调成功后进入 `CommitTransaction` 并执行 `COMMIT`，成功后回到 `Other`。只有整个闭包返回成功才把 cleanup 的 `success` 置真；其余错误及 panic 都会在析构时以携带原 job ID、1 秒截止时间的新上下文尝试 `ROLLBACK`。

辅助路径中，`global_time_zone` 从 `SessionVariables::global_system_variable("time_zone")` 读取并交给 `TimeZone::parse`；`kill_statement` 设置 killed 原子位；`now` 返回 `SystemTime::now()` 与当前 `location` 的快照；`avoid_reuse` 在锁内调用宿主回调。

## 数据与状态

`ExecutionContext` 的取消状态是 `Arc<AtomicBool>`，因此克隆上下文共享取消位；deadline、tracer 和 job ID 随克隆复制。值得注意的是，本文件的 `SqlExecutor` trait 本身不强制检查 `ExecutionContext::is_done`，取消/超时是否真正终止 SQL 由宿主执行器负责。

`SessionVariables` 是会话级共享状态：时区字符串、解析后的 `location` 和 TTL job ID 分别由独立 `Mutex` 保护，打断状态由 `AtomicI32` 表示。`execute_sql` 对 job ID 采用保存—替换—恢复协议，允许同一池化会话先后处理不同 job 或元数据 SQL；它不是跨线程并发执行多个语句的隔离容器，因为 job ID 是单一会话槽位。

事务成功状态只存在于栈上 `TransactionCleanup::success`。事务实际读写集由宿主 SQL 执行器维护，本文件仅发送 BEGIN/COMMIT/ROLLBACK 文本。`PhaseTracer` 同样只保存粗粒度单值，不记录历史或耗时。

## 依赖与调用关系

向下调用关系为：`new_session -> TtlSession::new -> SessionContext::sql_executor`；`execute_sql -> SessionContext::session_variables -> SqlExecutor::execute_internal -> RecordSet::{drain, close}`；`run_in_transaction -> execute_sql(BEGIN) -> callback -> execute_sql(COMMIT)`，失败路径则由 `TransactionCleanup::drop -> execute_sql(ROLLBACK)`；时区、kill 和时间路径分别落到 `SessionVariables` 的相关方法。

向上关系方面，`pkg/ttl/session/lib.rs` 公开重导出全部符号，根 workspace 用 `facade_ttl_session` 注册 crate。RustCodeGraph 将本文件识别为 99 个符号并报告若干“used by”文件，但对 `new_session`、`TtlSession::execute_sql` 与 `run_in_transaction` 的精确 callers/callees 查询没有产出可用边；结合 `rg` 的精确引用结果，目前可确认的直接调用者是 `pkg/ttl/session/session_test.rs`。`pkg/ttl/ttlworker/scan.rs` 与 `del.rs` 调用的是 `pkg/ttl/ttlworker/session.rs` 中另一套 `WorkerSession::execute_with_ttl_job`，不能当成本文件调用边。

Cargo 边界也印证未接线状态：常规依赖为空；`astersql-sessionctx`、`astersql-sessiontxn`、`astersql-ttl-metrics`、`astersql-util-sqlexec` 等真实集成候选只存在于 `cfg(any())` 条目。扩展生产接线前需要先明确是适配这些真实类型，还是把本文件的抽象桥接到现有 `WorkerSession`，不能仅依据同名符号推断已经连通。

## 错误处理与边界

`execute_internal` 与 `drain` 的错误直接向上传播；若 drain 失败仍会执行 close，但 `RecordSet::close` 的错误总被丢弃。无结果集被规范化为 `Ok(Vec::new())`，与 Go 的 `nil` 行切片表现不同但调用语义均表示无行。

事务中的未知模式、BEGIN 失败、回调失败和 COMMIT 失败都会保持 `success == false` 并触发回滚。回滚使用不继承调用方取消位的新 `ExecutionContext`，只设置 1 秒 deadline；但 deadline 的执行效果仍取决于宿主 `SqlExecutor` 是否检查上下文。回滚错误被显式忽略，原始错误得以保留。panic 时 Rust 的 `Drop` 仍会回滚并恢复相位；若 mutex 已中毒，本文件广泛使用的 `lock().unwrap()` 会再次 panic，这是当前未封装的边界。

`TimeZone::parse` 验证固定偏移小时不大于 14、分钟不大于 59，并拒绝空串；它没有拒绝 `+14:59`、没有验证命名时区是否存在，也把命名区偏移暂存为 0。调用方不能把 `offset_seconds == 0` 当作命名区真实偏移。

`kill_statement` 只置位、不负责清零；复用会话前如何重置 killed 状态属于宿主生命周期。`avoid_reuse` 没有 Go 实现中的 nil 检查，因为构造签名要求有效闭包。

## 并发与资源生命周期

对外 trait 均要求 `Send + Sync`，`TtlSession` 通过 `Arc` 共享宿主对象；可变状态使用 `Mutex` 或原子量。原子取消/killed 的写入采用 `Release`、读取采用 `Acquire`。`PhaseTracer`、时区与 job ID 的锁只保护单次读写，不把一整次 SQL 或事务串行化。

`RestoreJobId` 和 `TransactionCleanup` 是两个关键 RAII 守卫：前者把 TTL job 归因严格限制在一条 `execute_sql` 调用内；后者把失败回滚和相位恢复绑定到栈展开。结果集生命周期为 execute、drain、close；当前 close 不是独立 RAII 守卫，但代码在 drain 后无条件调用一次 close。`avoid_reuse` 回调在其 mutex 持锁期间执行，若回调递归调用同一会话的 `avoid_reuse` 会死锁，扩展时不应引入这种重入。

独立测试 `TestSessionKill` 用后台线程轮询模拟长 SQL，再调用 `kill_statement`，验证 killed 原子位能跨线程可见。测试未证明同一 `TtlSession` 可以安全并发执行多个 SQL；共享 TTL job ID 的设计反而要求池化层避免同一会话重叠执行。

## 与 Go 版本的对应关系

主要映射是：Rust `Session`/`TtlSession` 对应 `pkg/ttl/session/session.go` 的 `Session`/`session`；`new_session` 对应 `NewSession`；`ExecutionContext::with_ttl_job` 对应 `WithJobContext`；`execute_sql`、`run_in_transaction`、`global_time_zone`、`kill_statement`、`now`、`avoid_reuse` 分别对应 Go 的同职责方法。

核心顺序保持一致：SQL 执行临时设置 TTL job ID、标记 TTL 内部请求、以 8 为批大小 drain 并关闭结果集；事务进入 begin/other/commit/other 相位，失败时用独立 1 秒上下文回滚，并恢复进入前相位。`pkg/ttl/session/session_test.rs` 的 `TestSessionRunInTxn` 与 `TestSessionKill` 镜像 Go 同名测试，另外补充 job 归因恢复及 panic 展开回滚的直接验证；Go 的 `pkg/ttl/session/session_test.go::TestSessionTTLJobRU` 还验证真实 RU 指标归因和提交发布，这部分没有被 Rust 内存 mock 完整覆盖。

差异也必须保留：Go 直接使用 `sessionctx.Context`、事务管理器、`sqlexec`、metrics tracer、SQLKiller 和真实时区库；Rust 当前用本地 trait/结构替代。Go `timeutil.ParseTimeZone` 验证命名时区并返回真实 location，Rust 只保留命名区名称。Go `Now` 直接返回已套用 location 的 `time.Time`，Rust 返回 `(SystemTime, TimeZone)`。Go 的 `AvoidReuse` 容忍 nil 回调，Rust 构造 API 不允许缺省回调。这些都是迁移/接线时要处理的兼容点。

## 扩展指南

若新增 SQL 执行能力，应优先扩展 `SqlValue`、`Row`、`RecordSet` 或 `SqlExecutor`，并同步 `pkg/ttl/session/session_test.rs` 的 mock；不要绕过 `execute_sql`，否则会丢失 request source、TTL job 归因恢复和结果集关闭协议。若新增事务模式，应同时修改 `TxnMode` 与 `run_in_transaction` 的 BEGIN 映射，并增加 BEGIN/回调/COMMIT 各失败阶段及 panic 的独立测试。

若接入生产 Rust 主链，最可能修改的边界是 `SessionContext`、`SessionVariables`、`SqlExecutor` 和 `new_session` 的适配层，以及 `pkg/ttl/ttlworker/session.rs` 的 `WorkerSession` 桥接。必须先决定真实 session 类型的并发模型，并确保池化会话不会重叠执行导致 TTL job ID 串扰；同时对齐 Go 的真实 InfoSchema、SQL killer、RU 指标、时区数据库和事务 phase tracer。此类接线应移出 `cfg(any())` 依赖并增加真实集成测试，不能只依赖当前内存 mock。

若调整资源/错误策略，应为 close 错误、rollback 错误、取消与 deadline、poisoned mutex 和 killed 状态重置作出明确契约。性能风险主要来自每次 SQL 克隆字符串 job ID、频繁获取多个 mutex、一次性 drain 全部结果行，以及在锁内运行 `avoid_reuse` 回调；优化时不能破坏 Go 版本的归因和清理顺序。

## 验证依据

- 源码全貌与符号：`pkg/ttl/session/session.rs`，重点为 `ExecutionContext`、`SessionVariables`、`SessionContext`、`Session`、`TtlSession`、`TransactionCleanup`、`new_session`。
- crate/工作区边界：`pkg/ttl/session/Cargo.toml`、`pkg/ttl/session/lib.rs`、根 `Cargo.toml` 的 `facade_ttl_session` 条目。
- Go 对照：`pkg/ttl/session/session.go`；真实行为测试：`pkg/ttl/session/session_test.go` 的 `TestSessionTTLJobRU`、`TestSessionRunInTxn`、`TestSessionKill`。
- Rust 独立测试：`pkg/ttl/session/session_test.rs` 的 `TestSessionRunInTxn`、`ttl_job_attribution_covers_transaction_boundaries_and_is_restored`、`run_in_transaction_rolls_back_and_restores_phase_on_panic`、`TestSessionKill`。
- 相邻但独立的 worker 会话链：`pkg/ttl/ttlworker/session.rs`、`pkg/ttl/ttlworker/scan.rs`、`pkg/ttl/ttlworker/del.rs`，用于确认当前没有把同名/同职责接口误记为本文件调用边。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/ttl/session` 覆盖目标、Go 对照与测试；`node --file pkg/ttl/session/session.rs` 读取完整 552 行并显示 99 个符号。精确 `query` 找到本文件的 `Session`、`execute_sql`、`run_in_transaction`、`new_session`；callers/callees 查询未返回可用边，因此调用关系又以精确仓库引用搜索复核，并在本文中保留“尚未生产接线”的限制。
- 结构验证使用任务指定命令，确认本文恰含 11 个固定二级标题；本任务为纯文档分析，按要求未运行 Cargo。

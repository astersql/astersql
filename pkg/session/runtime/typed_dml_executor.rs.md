# `pkg/session/runtime/typed_dml_executor.rs`

源文件：[`typed_dml_executor.rs`](typed_dml_executor.rs)

## 文件定位

本文件属于 `astersql-session` crate 的私有 `runtime::typed_dml_executor` 模块；`pkg/session/runtime.rs` 通过 `mod typed_dml_executor;` 装配它，但不对 crate 外再导出。它位于“会话已绑定 canonical DML SQL”与通用执行适配器 `astersql_executor::adapter::ExecStmt` 之间：`pkg/session/runtime/scan_adapter_runtime.rs::SessionBoundAdapterOwner::BuildExecutor` 在 `PlanInfo::IsDML()` 为真时取出先前绑定的 SQL，并构造 `SessionTypedDMLExecutor`。

该类型不是 DML 算法本体。它把适配器要求的 `ExecExecutor` 生命周期翻译为会话运行时调用，真正的 INSERT、UPDATE、DELETE 逻辑分别位于 `ConcreteSession::{execute_insert, execute_update, execute_delete}`（`pkg/session/runtime/control.rs`），关系表写入、约束检查、事务和级联细节继续落到 `pkg/session/runtime/dml.rs` 等文件。

`pkg/session/Cargo.toml` 将本文件归入 `astersql-session`，并证明这里直接使用的本地 crate 边界包括 `astersql-errors`、`astersql-executor`、`astersql-parser-ast`、`astersql-parser-mysql`、`astersql-parser-types` 和 `astersql-util-chunk`。文件没有条件编译项，也没有专属 feature；crate 的 `nextgen` feature 不改变本文件代码。

## 核心职责

1. `SessionTypedDMLExecutor::new` 保存会话、绑定 SQL 与计划种类，建立一个无结果列、容量为 1 的写执行器。
2. `execute` 按当前 session SQL mode 重新解析绑定 SQL，强制恰好一个语句，并验证 AST 类型与 `PlanKind::{Insert, Update, Delete}` 一致后调用对应 `ConcreteSession` DML 入口。
3. `ExecExecutor::{Open,Next,Close}` 提供一次性、惰性的 DML 生命周期：`Open` 不写数据，第一次 `Next` 才执行，后续 `Next` 只清空输出并成功返回。
4. `NextWithContext` 在进入写路径前检查 `ExecutionContext.sql_killer`，使取消/终止信号先于本次 `Next` 生效。
5. 在悲观事务的适配器 staged-statement 阶段临时设置 `adapter_dml_defer_fk_locks`，让 DML 的外键父键检查先登记待锁键，再由适配器第二阶段统一加锁；RAII 守卫确保成功、错误和栈展开时都复位该标志。
6. 将会话中暂存的删除级联 `pending_fk_delete_cascades` 转换成通用 `CascadeBatch`，交回 `ExecStmt` 的外键触发器循环执行。

## 主要符号

- `SessionTypedDMLExecutor`：`pub(super)` 写执行器，只在 `runtime` 父模块内可见。`session: Rc<ConcreteSession>` 保留线程绑定的会话所有权；`sql` 是已绑定 canonical SQL；`kind` 是规划阶段判定的 DML 种类；`opened`/`done` 管理一次性执行状态；`schema`/`config` 声明该执行器不产出结果列。
- `SessionTypedDMLExecutor::new(session, sql, kind)`：唯一构造器。初始 `opened = false`、`done = false`，schema 和字段列表为空，chunk 初始/最大容量均为 1。
- `SessionTypedDMLExecutor::execute(&self)`：文件的行为核心。它读取 `session.state.sql_mode`，调用父模块 `parse_with_sql_mode`，核对单语句和 AST/计划类型，再分派到会话 DML 方法；返回通用 `AdapterResult`。
- `FKLockDeferGuard<'a>(&'a ConcreteSession)`：文件私有 RAII 守卫；`Drop` 无条件把 `adapter_dml_defer_fk_locks` 恢复为 `false`。它只管理临时模式位，不拥有事务或锁。
- `impl ExecExecutor for SessionTypedDMLExecutor`：把上述一次性执行接入适配器协议。关键方法是 `Open`、`Next`、`NextWithContext`、`TakeForeignKeyCascades` 和 `HasForeignKeyCascades`。
- `TakeForeignKeyCascades`：用 `std::mem::take` 原子式移出当前会话槽中的全部 `RuntimeForeignKeyDeleteCascade`，逐项包装成 `SessionForeignKeyDeleteCascadeBatch`；调用后原槽为空，避免重复消费。
- `HasForeignKeyCascades`：仅在计划是 `Delete` 且会话 `foreign_key_checks` 开启时返回真。它表示需要准备/运行适配器外键级联协议，并不保证当前已有待处理行；实际空批由 `TakeForeignKeyCascades`/`CascadeBatch::HasPendingRows` 处理。

文件没有模块级常量、公开 trait 或公开函数。`CheckForeignKeys`、`PrepareFKCascadeContext`、`AddFKCheckLockDuration` 是当前会话 DML 模型下的协议空实现；`Detach` 恒为 `None`，明确拒绝把持有 `Rc<ConcreteSession>` 的写执行器变成可脱离会话的执行器。

## 执行流程

1. 上游先通过 `SessionBoundAdapterOwner::BindDMLStatement` 绑定 SQL。`BuildExecutor` 收到 `PlanInfo` 后，若 `plan.IsDML()`，要求存在绑定 SQL，否则返回 `canonical DML was not bound to session runtime`；成功时调用 `SessionTypedDMLExecutor::new(session, sql, plan.kind)`。
2. `ExecStmt::exec_inner` 构建执行器、设置 process info，然后调用 `Open`。这里 `Open` 只把 `opened` 设为真并重置 `done`，所以构建和打开不会发生写入。`scan_adapter_runtime_test.rs::typed_dml_writer_defers_mutation_until_next_and_adapter_rejects_snapshot_writes` 直接验证了 `Open` 后查询仍看不到新行、第一次 `Next` 后才出现。
3. 由于 schema 为空且 `CalculateNoDelay` 返回真，`ExecStmt::handleNoDelay` 当场耗尽它；悲观事务走 `handlePessimisticDML`，其他情况走 `handleNoDelayExecutor`。两条路径最终都调用执行器的 `Next`，而不是向客户端返回 `RecordSet`。
4. `Next` 先拒绝未打开状态，再 `output.Reset()`。首次调用先把 `done` 置真，再调用 `execute`；先置真保证即使执行返回错误，同一实例也不会意外重放有副作用的 DML。再次调用只返回空结果。
5. `execute` 根据会话当时的 SQL mode 解析保存的 SQL。解析结果必须恰好一个 AST；随后 `PlanKind::Insert/Update/Delete` 分别要求 `InsertStmt/UpdateStmt/DeleteStmt`，匹配后进入 `ConcreteSession`。这使规划摘要和重新解析的执行 AST 不能静默分歧。
6. 执行期间若处于悲观事务且 `adapter_dml_statement_staged` 为真，`execute` 打开延迟 FK 锁模式。`dml.rs` 的父键检查看到该标志时把键写入 `TxnCtx.AddUnchangedKeyForLock`，而不是立即获取运行时锁；作用域结束时 `FKLockDeferGuard` 复位标志。
7. DML 删除逻辑若在 staged 模式发现 ON DELETE 级联，会把 `RuntimeForeignKeyDeleteCascade` 追加到 `pending_fk_delete_cascades`，而非立即递归执行。根 DML 完成后，`ExecStmt::handleStmtForeignKeyTrigger` 先按需要 `StmtCommit`，再调用 `TakeForeignKeyCascades`；每个批次由 `typed_fk_cascade_executor.rs` 构建子执行器、执行、关闭、再次 `StmtCommit` 并递归处理后续触发器。
8. 适配器完成无延迟路径后关闭执行器并执行事务/语句收尾。目标文件的 `Close` 只改变 `opened`；事务提交、回滚、悲观重试和外键保存点均由 `ExecStmt` 与 `AdapterRuntime` 实现，不在本文件重复管理。

## 数据与状态

`opened` 与 `done` 构成两位状态机：新建为 `(false,false)`；`Open` 变为 `(true,false)`；首次 `Next` 在执行前变为 `(true,true)`；`Close` 变为 `(false,true)`；再次 `Open` 可把同一执行器重置为 `(true,false)`。因此执行器允许适配器在悲观重试时重新打开，但调用方必须先用回滚/重建协议撤销上一尝试的会话副作用，不能仅靠 `Open` 重置事务状态。

`schema` 永远为空，`ChunkConfig.fields` 也为空，`NewChunk` 用空 `FieldType` 列表创建容量 `1/1` 的 chunk。这个 chunk 只是满足统一 pull 协议的控制载体，不承载 DML 返回行；`Next` 每次都清空它。`CalculateNoDelay = true` 和 `IsWriteExecutor = true` 是上游选择同步执行、写入保护和 DML 超时路径的重要分类信号。

`session` 使用 `Rc` 而非 `Arc`，与 `ConcreteSession` 内的 `RefCell`/`Cell` 状态模型一致。`execute` 的各次 `borrow()` 都是短借用：SQL mode 读取、staged 标志读取、延迟位写入分别完成后才进入解析或 DML 执行，避免把 `RefCell` 借用跨越下游调用。

`pending_fk_delete_cascades` 是会话状态中的所有权队列。生产者位于 `dml.rs::cascade_foreign_key_deletes`，消费者是本文件 `TakeForeignKeyCascades`；`std::mem::take` 同时给消费者完整所有权并恢复空向量。`adapter_dml_defer_fk_locks` 则是严格作用域化的临时布尔值，任何新增提前返回都必须继续置于 `FKLockDeferGuard` 生命周期内。

## 依赖与调用关系

RustCodeGraph 将本文件索引为 26 个符号，并显示它由 `pkg/session/runtime.rs` 装配、由 `pkg/session/runtime/scan_adapter_runtime.rs` 构造，同时与 `typed_fk_cascade_executor.rs`、`control.rs`、`dml.rs` 和适配器测试形成直接行为链。方法名 `new`/`execute` 在全仓高度重载，精确调用点因此同时用模块限定源码核验。

上游主链是：

`ExecStmt::Exec`（`pkg/executor/adapter.rs`）→ `ExecStmt::buildExecutor` → `SessionBoundAdapterOwner::BuildExecutor`（`scan_adapter_runtime.rs`）→ `SessionTypedDMLExecutor::new` → `Open` → `Next`/`NextWithContext`。

下游主链是：

- `execute` → `parse_with_sql_mode`（`pkg/session/runtime.rs`）→ parser AST；
- `execute` → `ConcreteSession::{execute_insert, execute_update, execute_delete}`（`control.rs`）；
- 上述方法继续调用 `PlanInsert/PlanUpdate/PlanDelete`、关系表 DML、事务、锁和约束逻辑（`dml.rs`、`query.rs` 等）；
- `TakeForeignKeyCascades` → `SessionForeignKeyDeleteCascadeBatch::new`（`typed_fk_cascade_executor.rs`）→ `SessionForeignKeyDeleteCascadeExecutor` → `ConcreteSession::execute_pending_fk_delete_cascade`。

协议依赖来自 `astersql_executor::adapter`：`ExecExecutor` 定义生命周期和外键钩子，`ExecutionContext` 提供 `sql_killer`，`CascadeBatch` 抽象级联批，`PlanKind` 决定 AST 分派。数据依赖来自 parser AST、mysql SQL mode、chunk 与共享错误类型。该文件不直接访问存储、网络或后台任务；这些副作用都经 `ConcreteSession` 间接发生。

## 错误处理与边界

- `Next` 在未 `Open` 时返回 `typed DML executor is not open`；`Close` 后再次 `Next` 同样失败。
- `GetSQLMode`、SQL 解析错误都转换为 `astersql_errors::SharedError`。`parse_with_sql_mode` 会保留 TiDB 风格的 `[parser:1064]` 语法错误前缀。
- 解析出零条或多条语句返回 `bound DML must contain exactly one statement`，阻止一个计划包装器执行多个副作用语句。
- `PlanKind` 与 AST 不一致时返回 `bound DML plan/AST kind mismatch`；非 Insert/Update/Delete 的种类返回 `typed DML executor requires a DML plan`。这两层检查把绑定错误留在执行边界，而不是错误地转入另一类 DML。
- `ConcreteSession` 的 `SessionError` 通过 `into_shared()` 保留底层错误源；`scan_adapter_runtime_test.rs::canonical_dml_error_bridge_preserves_write_conflict_for_adapter_retry` 验证写冲突经该桥接后仍能被适配器识别并进入悲观重试。
- `NextWithContext` 只在每次拉取开始时调用 `SQLKiller::HandleSignal`；进入同步的 `ConcreteSession` DML 后是否继续响应取消，取决于下游各阶段自己的检查，不能从本文件推断为可随时中断。
- `done` 在 `execute` 之前置真。错误后直接对同一实例再次 `Next` 会返回成功空结果；正确重试由上游回滚语句状态并重建/重新打开执行器完成。
- `HasForeignKeyCascades` 仅根据 Delete 和 `foreign_key_checks` 粗筛，可能返回真但队列为空；相反，当前实现只把删除级联包装为批次，不能据此宣称 Rust 已在此文件实现 Go 的全部 ON UPDATE/SET NULL 触发器对象模型。

## 并发与资源生命周期

该执行器是会话线程绑定对象：它持有 `Rc<ConcreteSession>`，没有 `Send`/`Sync` 承诺，`Detach` 恒为 `None`。调用者不得把它或关联的 chunk 移到其他线程；线程安全的 detached scan 是适配器中其他执行器的能力，不适用于写路径。

`Open`/`Next`/`Close` 需要 `&mut self`，因此正常 Rust 调用不能并发推进同一实例。会话共享状态通过 `RefCell` 在运行时检查借用，不提供多线程互斥；本文件刻意只做短借用，并在调用 DML 前释放借用。

`FKLockDeferGuard` 的生命周期覆盖解析、AST 检查和完整 DML 调用。无论正常返回、`?` 提前返回还是 panic 栈展开，`Drop` 都会清除延迟位，避免后续无关 DML错误地沿用 staged FK 锁模式。它不释放已取得的行锁；行锁、语句 staging、保存点和回滚由 `AdapterRuntime`/`ConcreteSession` 管理。

外键级联队列按所有权移交：根执行器通过 `TakeForeignKeyCascades` 清空会话槽，批对象持有 `Rc` 会话和待处理数据，级联子执行器执行后由 `ExecStmt` 关闭。根执行器自身不持有线程、任务、通道或网络连接，`Close` 因而无需等待后台资源。

## 与 Go 版本的对应关系

Rust 的直接协议对照位于 Go `pkg/executor/adapter.go`、`foreign_key.go`、`insert.go`、`update.go` 和 `delete.go`。Go `ExecStmt` 同样在构建并 `Open` 执行器后调用 `prepareFKCascadeContext`，对无结果 DML走 no-delay/悲观路径；外键阶段先在必要时 `StmtCommit`，再执行 checks 和 cascades，级联批次每次执行/关闭后再次 `StmtCommit`，并以 15 层为深度边界。

Go 的 `WithForeignKeyTrigger` 由 Insert/Update/Delete 执行器直接提供 `GetFKChecks`、`GetFKCascades`、`HasFKCascades`。本 Rust 文件采用较薄的桥接：父键检查已在 `ConcreteSession` DML 内同步完成，所以 `CheckForeignKeys` 为空；删除级联先写入会话 `pending_fk_delete_cascades`，再由 `TakeForeignKeyCascades` 转为通用批对象；`PrepareFKCascadeContext` 为空，因为保存点由 `SessionBoundAdapterOwner` 的运行时钩子维护。

另一个重要差异是 AST 来源。Go 主执行器通常持有 planner 构造的具体执行器，Go 外键级联注释明确说明级联 AST 直接构造以避免 Datum 字符串往返风险；本 Rust 根 DML 执行器保存 canonical SQL，并在第一次 `Next` 中按当前 SQL mode 重新解析，再以 `PlanKind` 下转 AST。文档不能把这种重新解析桥接描述为 Go executor builder 的逐对象等价实现。

Rust 当前 `HasForeignKeyCascades` 只对 Delete 返回可能为真，`TakeForeignKeyCascades` 也只包装 `RuntimeForeignKeyDeleteCascade`。`ConcreteSession` 其他路径确有更新级联逻辑，但不通过本文件的批对象暴露；扩展 Go 对齐时应先确认触发器是在根 DML 内同步完成还是需要纳入适配器递归协议，避免重复执行。

Go 回归证据包括 `pkg/executor/test/fktest/foreign_key_test.go` 的 ON DELETE/ON UPDATE cascade、深度限制和权限场景，以及 `pkg/executor/executor_failpoint_test.go::TestHandleForeignKeyCascadePanic`。它们说明上游语义目标，但 Rust 当前行为以独立 Rust 测试和目标调用链为准。

## 扩展指南

- 新增 DML `PlanKind` 或 AST 类型时，必须同步修改 `execute` 的分派和错误检查、`PlanInfo::IsDML`/statement 分类、`SessionBoundAdapterOwner::BuildExecutor` 以及独立测试；不能用通配分支把未知计划静默当成现有 DML。
- 改变执行时机时，应保留“`Open` 无副作用、首次 `Next` 执行一次”的契约，或同时更新 `ExecStmt` 的 no-delay、悲观重试和阶段耗时逻辑。回归测试应放在独立的 `pkg/session/runtime/scan_adapter_runtime_test.rs`，不要嵌入本源文件。
- 增加返回行（例如未来 DML RETURNING）需要成套修改 `schema`、`ChunkConfig`、`NewChunk`、`CalculateNoDelay` 与 `Next` 填充逻辑，并确认 `ExecStmt` 不再把它当成无结果同步路径；仅填 schema 而不改执行协议会改变事务和结果集生命周期。
- 扩充外键检查/级联时，先决定职责归属：同步约束检查属于 `ConcreteSession` DML，适配器递归批属于 `CascadeBatch`。若引入 UPDATE cascade 批，应同时调整 `HasForeignKeyCascades`、队列类型、`TakeForeignKeyCascades`、保存点/`StmtCommit` 顺序以及 15 层深度测试，防止根逻辑和批逻辑重复应用。
- 修改 `adapter_dml_defer_fk_locks` 必须保持 RAII 清理，且同步验证 `dml.rs` 中 `TxnCtx.AddUnchangedKeyForLock` 与适配器第二阶段锁获取。性能风险主要是重复解析 canonical SQL、复制 SQL/schema/config，以及级联批逐项构造；任何缓存 AST 的优化都必须证明 SQL mode、prepared 参数和重试期间状态不会失配。
- 若希望支持 `Detach`，不能只返回 boxed self：`Rc<ConcreteSession>`、`RefCell` 状态、事务所有权和 pending cascade 都是线程/会话绑定的。应先设计拥有独立快照或事务句柄的写执行模型，并增加资源释放与取消测试。
- 建议最小 Rust 回归集合为 `scan_adapter_runtime_test.rs` 中 canonical DML 惰性执行、Insert/Update/Delete、FK parent check/delete cascade、FK shared-lock second phase、真实触发器错误回滚和悲观重试用例；更底层关系 DML 边界继续由 `pkg/session/dml_runtime_test.rs` 覆盖。测试逻辑必须保持在独立 `*_test.rs` 文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/session/runtime` 确认目标及相邻实现已索引；`node --file pkg/session/runtime/typed_dml_executor.rs --offset 1 --limit 260` 读取目标 175 行并确认 26 个符号；精确 `query` 确认 `SessionTypedDMLExecutor` 唯一定义在第 18 行、`FKLockDeferGuard` 在本文件及级联执行器各有一个定义。宽泛同名方法的 caller/callee 结果存在全仓重载噪声，因此方法级边由下列限定源码位置交叉核对。
- 装配与上游：`pkg/session/runtime.rs:140-145`；`pkg/session/runtime/scan_adapter_runtime.rs:266-309`；`pkg/executor/adapter.rs:288-328, 1240-1461, 1825-1867`。
- 下游实现：`pkg/session/runtime.rs:1273-1299`（SQL mode 解析）；`pkg/session/runtime/control.rs:2104-2291`（三类 DML 入口）；`pkg/session/runtime/dml.rs:510-575, 1862-1899`（延迟父键锁、pending 删除级联和深度边界）；`pkg/session/runtime/typed_fk_cascade_executor.rs:65-140`（批次子执行器与 RAII 清理）；`pkg/session/runtime/session.rs:173-190`（会话状态字段）。
- crate 边界：`pkg/session/Cargo.toml` 的 `[package]`、`[lib]`、`[features]`、`[package.metadata.porting]` 和依赖声明；最近路径下不存在 `doc.go`，因此没有额外 package contract 可读。
- Rust 独立测试：`pkg/session/runtime/scan_adapter_runtime_test.rs::canonical_adapter_dml_executes_lazily_and_locks_insert_update_delete_mutations`、`typed_dml_writer_defers_mutation_until_next_and_adapter_rejects_snapshot_writes`、`adapter_dml_preserves_canonical_fk_parent_check_and_delete_cascade`、`adapter_real_fk_trigger_error_restores_parent_delete_and_keeps_transaction_active`、`adapter_dml_acquires_fk_parent_shared_lock_in_second_phase`、`canonical_dml_error_bridge_preserves_write_conflict_for_adapter_retry`；关系 DML 的级联限制另见 `pkg/session/dml_runtime_test.rs` 的 cascade 测试。
- Go 对照：`pkg/executor/adapter.go:800-1050`、`pkg/executor/foreign_key.go:47-108`、`pkg/executor/{insert,update,delete}.go` 的 `WithForeignKeyTrigger` 实现；Go 测试位置为 `pkg/executor/test/fktest/foreign_key_test.go` 和 `pkg/executor/executor_failpoint_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰好包含 11 个固定二级标题，并人工复核没有把未接线能力写成已支持，也没有建议把 Rust 测试写回生产源文件。

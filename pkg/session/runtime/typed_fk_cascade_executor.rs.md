# `pkg/session/runtime/typed_fk_cascade_executor.rs`

## 文件定位

本文件位于 `astersql-session` crate 的 `runtime` 模块中，由 [`pkg/session/runtime.rs`](../runtime.rs) 的 `mod typed_fk_cascade_executor` 私有装配。它不是外键级联算法本体，而是会话层与 [`astersql_executor::adapter`](../../executor/adapter.rs) 执行器协议之间的适配层：把一次已暂存的父表删除级联描述包装成 `CascadeBatch`，再构建一个无结果集、一次性执行的 `ExecExecutor`。

直接上游是 [`SessionTypedDMLExecutor::TakeForeignKeyCascades`](typed_dml_executor.rs)：普通 typed DML 执行结束后，它从会话状态取走 `pending_fk_delete_cascades`，逐项构造本文件的 `SessionForeignKeyDeleteCascadeBatch`。执行器框架随后由 [`ExecStmt::handleForeignKeyTrigger`](../../executor/adapter.rs) 和 `ExecStmt::handleForeignKeyCascade` 消费这些批次。真正的递归 DELETE/SET NULL/RESTRICT 语义仍在 [`ConcreteSession::execute_pending_fk_delete_cascade`](dml.rs) 及其下游 `cascade_foreign_key_deletes_at_depth` 中。

[`pkg/session/Cargo.toml`](../Cargo.toml) 将该目录编入包名 `astersql-session`、库入口 `lib.rs`；本文件直接使用其中声明的 `astersql-errors`、`astersql-executor`、`astersql-util-chunk` 和 `astersql-parser-types` 依赖。文件内没有 feature gate 或条件编译项。

## 核心职责

1. `SessionForeignKeyDeleteCascadeBatch` 保存一个会话引用和一个尚未完成的 `RuntimeForeignKeyDeleteCascade`，实现执行框架要求的“是否还有数据、构建子执行器、标记完成”三段式协议。
2. `SessionForeignKeyDeleteCascadeExecutor` 把该级联描述转换为一次 `Next` 调用：检查生命周期、清空输出、调用 `execute_pending_fk_delete_cascade`，并确保外键锁延迟标志在调用退出时恢复。
3. 向通用执行器框架声明该执行器是无输出的写执行器：空 schema、容量为 1 的空字段 chunk、`CalculateNoDelay = true`、`IsWriteExecutor = true`。

本文件刻意不重复实现外键匹配、递归深度、SET NULL、RESTRICT、事务写入或索引维护。源文件第 14–15 行的注释和 `Next` 对 `execute_pending_fk_delete_cascade` 的委托共同限定了这一边界。

## 主要符号

- `SessionForeignKeyDeleteCascadeBatch`（`pub(super)`）：模块父级可见的批次对象。`session: Rc<ConcreteSession>` 让批次和它创建的执行器共享同一个线程绑定会话；`pending: Option<RuntimeForeignKeyDeleteCascade>` 同时表达载荷和消费状态。
- `SessionForeignKeyDeleteCascadeBatch::new(session, pending) -> Self`：以 `Some(pending)` 初始化一个未消费批次。它只被 `SessionTypedDMLExecutor::TakeForeignKeyCascades` 调用。
- `impl CascadeBatch`：`HasPendingRows` 检查 `Option`；`BuildExecutor` 在载荷仍存在时克隆会话 `Rc` 和级联描述并返回 boxed executor，批次为空时返回 `Ok(None)`；`MarkBatchComplete` 通过 `take()` 清空载荷。
- `SessionForeignKeyDeleteCascadeExecutor`（文件私有）：一次性子执行器。`opened` 和 `done` 管理调用状态；`schema`、`config` 实现通用执行器的结果元数据契约。
- `SessionForeignKeyDeleteCascadeExecutor::new`：初始化为未打开、未完成、空 schema，并把 chunk 初始/最大容量都设为 1。
- `FKLockDeferGuard<'a>`（文件私有）：持有 `&ConcreteSession` 的 RAII guard。其 `Drop` 无条件把 `adapter_dml_defer_fk_locks` 恢复为 `false`，覆盖成功、业务错误和 Rust 栈展开退出路径。
- `impl ExecExecutor for SessionForeignKeyDeleteCascadeExecutor`：实现 `Open`、`Close`、`Next` 及执行器属性。关键入口是 `Next`；其余外键钩子为空实现，因为递归处理已经由会话层函数在本次 `Next` 内完成。

## 执行流程

1. typed DELETE 在 [`SessionTypedDMLExecutor::execute`](typed_dml_executor.rs) 中调用会话 DML。若 `adapter_dml_statement_staged` 为真，[`cascade_foreign_key_deletes`](dml.rs) 不立即递归，而是把父表元数据和已删除行复制到 `pending_fk_delete_cascades`。
2. 顶层执行器完成后，`SessionTypedDMLExecutor::TakeForeignKeyCascades` 用 `std::mem::take` 原子式地把挂起向量移出会话状态，并为每个元素创建一个 `SessionForeignKeyDeleteCascadeBatch`。
3. [`ExecStmt::handleForeignKeyTrigger`](../../executor/adapter.rs) 遍历批次并进入 `handleForeignKeyCascade`。框架先检查 `HasPendingRows` 和其自身的级联深度上限，然后调用 `BuildExecutor`。
4. `BuildExecutor` 不消费批次，只克隆 `pending` 来创建子执行器。这一点允许框架仅在子执行器成功、`StmtCommit` 成功且递归触发器处理完成后才调用 `MarkBatchComplete`；中途失败时批次仍保持 pending。
5. 框架调用子执行器 `Open`，再创建空 chunk 并调用 `Next`。`Next` 要求已打开，首先 `Reset` 输出；首次调用把 `done` 置真、打开 `adapter_dml_defer_fk_locks`，建立 `FKLockDeferGuard`，然后委托 `execute_pending_fk_delete_cascade`。
6. [`cascade_foreign_key_deletes_at_depth`](dml.rs) 扫描引用当前父表的子表，根据 `OnDelete` 执行 CASCADE 删除、SET NULL 更新或返回 1451 限制错误；它还处理自引用去重、DELETE 优先于 SET NULL、物化视图日志、索引 mutation、递归深度和语句原子性。
7. `Next` 返回前 guard 恢复锁延迟标志。框架随后 `Close` 子执行器、提交本级 statement mutation、递归处理子执行器暴露的级联，最后调用批次的 `MarkBatchComplete`。本子执行器自身返回空的 `TakeForeignKeyCascades`，因为会话层已在同一次调用内完成递归。
8. 若同一个子执行器被再次 `Next`，它只清空输出并返回成功；若在 `Open` 前调用，则返回 `FK cascade executor is not open`。

## 数据与状态

`RuntimeForeignKeyDeleteCascade` 定义在 [`runtime/session.rs`](session.rs)，含完整克隆的 `TableInfo parent` 和 `Vec<HashMap<String, Option<String>>> deleted`。因此批次不借用原始 DELETE 执行器的行缓冲，可跨顶层执行结束后的触发器阶段使用；代价是表元数据和删除行的克隆开销。

批次的 `pending: Option<_>` 是最重要的不变量：`Some` 表示尚待框架确认完成，`None` 表示已完成。`BuildExecutor` 只读取并克隆，只有 `MarkBatchComplete` 才消费；不能把消费提前到构建阶段，否则打开、执行、提交或递归失败后会丢失状态。

子执行器的 `opened`/`done` 是轻量状态机：构造后 `(false, false)`；`Open` 变为 `(true, false)`；首次 `Next` 变为 `(true, true)`；`Close` 只把 `opened` 置假。再次 `Open` 会重置 `done`，因而会再次执行同一份克隆载荷；当前框架每批只打开一次，这不是可安全重放的幂等承诺。

`schema` 永远为空，`ChunkConfig.fields` 为空，`NewChunk` 也用空 `FieldType` 列表创建容量 1 的 chunk。输出 chunk 仅用于满足统一协议，级联 mutation 不通过行输出返回。

## 依赖与调用关系

上游调用链为：

`scan_adapter_runtime.rs::BuildExecutor` → `SessionTypedDMLExecutor` → `TakeForeignKeyCascades` → `SessionForeignKeyDeleteCascadeBatch::new` → `executor/adapter.rs::handleForeignKeyCascade` → `CascadeBatch::BuildExecutor` → `SessionForeignKeyDeleteCascadeExecutor::Next`。

关键下游为：

- `ConcreteSession::execute_pending_fk_delete_cascade`：从深度 0 进入会话层递归实现。
- `ConcreteSession::cascade_foreign_key_deletes_at_depth`：实施外键检查开关、15 层限制、子表扫描和 mutation。
- `errors::New`：构造未打开错误；会话层 `SessionError` 经 `into_shared()` 转成 `AdapterResult` 错误。
- `chunk::Chunk::{Reset}` 与 `chunk::New`：履行通用 executor 的空输出契约。
- `ConcreteSession.state`：通过 `RefCell` 修改 `adapter_dml_defer_fk_locks`。[`dml.rs`](dml.rs) 的父键检查在显式悲观事务且该标志为真时，把待锁键记录到 `TxnCtx`，而不是在级联执行期间立即取锁。

RustCodeGraph 的文件节点报告本文件由 `typed_dml_executor.rs` 使用；精确符号查询定位了 `SessionForeignKeyDeleteCascadeBatch`、文件私有 executor、guard 和 `execute_pending_fk_delete_cascade`。图索引未解析 trait 动态分派的 method-level caller，因此框架侧调用关系以 `pkg/executor/adapter.rs` 的 `CascadeBatch` trait 与 `handleForeignKeyCascade` 源码核对。

## 错误处理与边界

- 生命周期误用：`Next` 在 `opened == false` 时立即返回共享错误，不修改 `done`、输出或会话级联状态。
- 一次性语义：首次 `Next` 在调用业务函数前先设置 `done = true`。若业务函数报错，同一 executor 上再次 `Next` 不会重试；错误恢复由外层触发器保存点/事务路径负责，批次也因未执行 `MarkBatchComplete` 而仍保持 pending。
- 错误转换：`execute_pending_fk_delete_cascade` 的 `SessionError` 使用 `into_shared()` 转换，错误文本（例如 1451 或深度超限）由下游保留，本文件不重写上下文。
- 深度限制有两层直接证据：通用 adapter 的 `handleForeignKeyCascade` 检查 `MAX_FOREIGN_KEY_CASCADE_DEPTH`，而当前会话算法自身在 `depth >= 15 && !deleted.is_empty()` 时失败。由于本子执行器不向 adapter 暴露嵌套批次，当前路径主要依赖会话算法的内部限制。
- `CheckForeignKeys` 返回成功、`TakeForeignKeyCascades` 为空、`PrepareFKCascadeContext` 和 `AddFKCheckLockDuration` 为空、`Detach` 返回 `None`。这些是当前适配边界，不表示整个外键操作没有检查、保存点、锁时长或递归；相应工作分别发生在会话 DML 或外层 adapter。
- 本文件没有处理 panic；RAII guard 在正常 Rust 栈展开时会恢复标志，但进程 abort 不提供该保证。

## 并发与资源生命周期

`Rc<ConcreteSession>` 与 `ConcreteSession.state: RefCell<_>` 表明该执行器是会话线程绑定对象，而非跨线程任务；`ExecExecutor` trait 本身也没有要求 `Send`。批次和子执行器共享同一会话，确保级联 mutation、事务内存缓冲、锁记录及保存点都属于触发它的原事务。

资源顺序由 adapter 固定为 `Open → NewChunk → Next → Close → StmtCommit → 递归触发 → MarkBatchComplete`。打开失败时 adapter 尝试 `Close`；运行或关闭失败时不提交、不标记批次完成。`FKLockDeferGuard` 的生命周期仅覆盖 `execute_pending_fk_delete_cascade`，所以锁延迟标志不会泄漏到随后的 statement commit 或其他执行器。

`BuildExecutor` 克隆 `Rc`，`Close` 不释放会话；真正的引用释放发生在 executor 和 batch 离开作用域时。`pending` 的表/行克隆分别存在于 batch 与 executor，直至框架完成或错误路径销毁对应对象。文件内没有异步任务、线程、锁 mutex、channel 或后台资源。

## 与 Go 版本的对应关系

最接近的 Go 对照不是 `pkg/session` 同路径文件，而是 [`pkg/executor/foreign_key.go`](../../executor/foreign_key.go) 的 `FKCascadeExec` 和 [`pkg/executor/adapter.go`](../../executor/adapter.go) 的 `handleForeignKeyCascade`：Go 同样先积累被引用键值，再为级联构建执行计划/执行器，按批执行、`StmtCommit`，随后递归处理新触发器，并受 15 层深度限制。

Rust 的 `CascadeBatch` 对应 Go `FKCascadeExec` 的“仍有待处理值、构建 executor、成功后清理已处理值”职责；Rust adapter 的 `handleForeignKeyCascade` 保留了 Go 的打开、执行、关闭、逐级 statement commit 和递归顺序。`SessionForeignKeyDeleteCascadeExecutor` 则是 AsterSQL 当前会话运行时的局部桥接：它不调用 planner 构造 SQL 子计划，而把克隆的父表和删除行交给 `dml.rs` 的关系行扫描/mutation 算法一次处理完。

因此两者的协议意图和事务阶段相近，但内部表示并非逐字段一一对应。Go `FKCascadeExec` 区分 delete/update、维护 `fkValues`/`fkUpdatedValuesMap` 并收集运行时统计；本文件只包装“delete cascade pending”，不记录级联统计，更新级联由会话 DML 的其他路径直接递归处理。文档不能据此声称 Rust 已完整复刻 Go `FKCascadeExec` 的规划器、分批上限或统计能力。

相关 Go 行为测试集中在 [`pkg/executor/test/fktest/foreign_key_test.go`](../../executor/test/fktest/foreign_key_test.go)，覆盖深度限制、级联间 `StmtCommit` 和错误；Rust 侧最直接的 adapter 回归位于 [`runtime/scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 的 `adapter_dml_preserves_canonical_fk_parent_check_and_delete_cascade` 与 `adapter_real_fk_trigger_error_restores_parent_delete_and_keeps_transaction_active`。

## 扩展指南

- 若改变批次重试或多批消费规则，应首先修改 `SessionForeignKeyDeleteCascadeBatch::{BuildExecutor, MarkBatchComplete}`，保持“成功提交和递归完成后才消费”的不变量，并在独立的 `*_test.rs` 中覆盖 open/next/commit 失败后的状态；不要把测试嵌入本生产文件。
- 若增加 UPDATE CASCADE、分批大小或运行时统计，需先对照 Go `FKCascadeExec`，再决定扩展 `RuntimeForeignKeyDeleteCascade` 还是引入独立载荷类型；同时同步 `SessionTypedDMLExecutor::TakeForeignKeyCascades`、adapter trait 和会话 DML 算法，避免把完整算法塞进这个桥接文件。
- 若改变锁延迟策略，应联合检查本文件与 `typed_dml_executor.rs` 中两个同名 `FKLockDeferGuard`，以及 `dml.rs` 使用 `adapter_dml_defer_fk_locks` 的分支。guard 应保存/恢复明确的前值或保持当前“进入前由调用点拥有、退出统一 false”的契约，不能让错误路径泄漏状态。
- 若让 executor 可跨线程或可 detach，`Rc`、`RefCell` 和 `ConcreteSession` 的线程亲和性必须整体重审；仅把 `Detach` 改为返回对象不足以安全化。
- 推荐同步扩展 [`runtime/scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 的 adapter 端到端测试；算法边界应放在 [`dml_runtime_test.rs`](../dml_runtime_test.rs)，悲观事务共享性应放在 [`runtime_pessimistic_test.rs`](../runtime_pessimistic_test.rs)。重点风险是事务原子性、保存点恢复、锁获取时序、自引用终止、DELETE/SET NULL 冲突优先级和 15 层兼容性；性能风险主要是载荷克隆与对子表目录/行的扫描。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含本文件；`files --filter pkg/session/runtime` 定位该文件；`node --file pkg/session/runtime/typed_fk_cascade_executor.rs` 读取 1–140 行并报告直接使用文件；`query` 定位 `SessionForeignKeyDeleteCascadeBatch`、`SessionForeignKeyDeleteCascadeExecutor`、`FKLockDeferGuard` 和 `execute_pending_fk_delete_cascade`；`callers`/`callees` 对动态 trait 调用未给出完整边，因此未把缺失边误写为“无调用者”。
- Rust 源码：[`typed_fk_cascade_executor.rs`](typed_fk_cascade_executor.rs)、[`typed_dml_executor.rs`](typed_dml_executor.rs)、[`dml.rs`](dml.rs)、[`session.rs`](session.rs)、[`scan_adapter_runtime.rs`](scan_adapter_runtime.rs)、[`pkg/executor/adapter.rs`](../../executor/adapter.rs) 和 [`pkg/session/runtime.rs`](../runtime.rs)。
- crate 配置：[`pkg/session/Cargo.toml`](../Cargo.toml) 的 `[package]`、`[lib]`、`[features]` 与直接依赖声明。目标包未找到 `pkg/session/doc.go`，因此没有可读取的包级 `doc.go` 契约。
- Rust 测试：[`runtime/scan_adapter_runtime_test.rs`](scan_adapter_runtime_test.rs) 验证 adapter 两阶段删除级联、每级 statement commit 和失败保存点恢复；[`dml_runtime_test.rs`](../dml_runtime_test.rs) 验证自引用、RESTRICT、检查关闭和 15 层边界；[`runtime_pessimistic_test.rs`](../runtime_pessimistic_test.rs) 验证级联与显式事务共享。未发现本文件同名独立测试。
- Go 对照：[`pkg/executor/foreign_key.go`](../../executor/foreign_key.go) 的 `FKCascadeExec`、`onDeleteRow`、`buildExecutor`，以及 [`pkg/executor/adapter.go`](../../executor/adapter.go) 的 `handleStmtForeignKeyTrigger`、`handleForeignKeyTrigger`、`handleForeignKeyCascade`、`prepareFKCascadeContext`；相关测试位于 `pkg/executor/test/fktest/foreign_key_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰有十一个规定的二级标题；事实复核以以上符号、调用协议和测试路径为界，未验证运行时性能数据或未执行测试。

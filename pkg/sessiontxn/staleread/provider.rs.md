# `pkg/sessiontxn/staleread/provider.rs`

## 文件定位

`provider.rs` 定义 Rust 侧的过期读（stale read）事务上下文 Provider，用固定历史时间戳组装只读事务、语句读时间戳和 KV 快照。它由 `pkg/sessiontxn/staleread/lib.rs` 声明并重导出，所属 crate 是 `astersql-sessiontxn-staleread`（`pkg/sessiontxn/staleread/Cargo.toml`）。

当前 Rust 实现是一个已可单元测试的轻量移植边界：它使用 `util.rs` 中的 `SessionRef`/`SessionBackend`/`Transaction`/`Snapshot` 模型，而 `Cargo.toml` 中真实 `sessionctx`、`kv`、`infoschema` 等依赖全部位于 `target.'cfg(any())'.dependencies`，永不会在正常构建中启用。全库 Rust 引用搜索也只找到本文件、`lib.rs`、`provider_test.rs` 和注释性引用，没有生产 Rust 调用者。因此，Go 侧的实际 SQL 事务主链是设计对照，不是当前 Rust 已接入真实 TiKV 的证据。

## 核心职责

- 保存不变的历史读时间戳 `ts`，并把所有语句读和快照请求钉在该时间点（`new`、`stmt_read_ts`、`snapshot_with_stmt_read_ts`）。
- 按进入类型初始化 stale transaction：默认进入和显式 `BEGIN` 都创建事务，替换 Provider 时只安装事务上下文（`on_initialize`）。
- 在非 autocommit 且尚未进入事务时惰性激活，使后续语句复用同一历史快照（`stmt_read_ts`、`activate_txn`）。
- 为事务和快照设置只读 stale 标记、`global` 事务作用域、本地临时表拦截标记以及 follower-read 偏好（`activate_stale_txn`、`snapshot_with_stmt_read_ts`）。
- 明确拒绝 `ForUpdateTS` 和 for-update 快照，因为过期读不参与当前版本的锁定/更新语义（`stmt_for_update_ts`、`snapshot_with_stmt_for_update_ts`）。

## 主要符号

- `GLOBAL_TXN_SCOPE: &str = "global"`：本文件创建的 stale transaction 及其 `TransactionContext` 的固定作用域。
- `EnterNewTxnType`：初始化分派枚举。`Default` 与 `WithBeginStatement` 激活事务，`WithReplaceProvider` 只替换上下文，`Unsupported` 进入错误分支。
- `StatementErrorAction::NoIdea`：表示 Provider 对语句错误不给出重试或回滚策略。
- `StalenessTxnContextProvider`：核心状态对象。`context` 保存最近的请求上下文，`session` 是 `Arc<Mutex<Session>>`，`info_schema` 是历史 schema 视图，`ts` 是固定读时间戳，`transaction` 是 `activate_txn` 的 Provider 本地缓存。
- `new(session, ts, info_schema)`：只构造 Provider；不提交旧事务、不创建新事务，也不把 Provider 标记到会话。
- `txn_info_schema`、`txn_scope`、`read_replica_scope`：暴露历史 schema、当前事务上下文作用域和配置的副本读作用域。
- `on_initialize`、`activate_stale_txn`、`enter_new_stale_txn_with_replace_provider`：完成 Provider 安装和事务上下文建立。
- `stmt_read_ts`、`activate_txn`、`snapshot_with_stmt_read_ts`：实现惰性激活、事务句柄复用与快照获取。
- `on_stmt_start`、`on_stmt_retry`：更新内部 `context`。其他语句钩子和建议钩子当前是无副作用成功返回，但保留了与 Go Provider 的 API 形状。

## 执行流程

1. 调用方以 `new` 绑定会话、固定 `ts` 和可选的 `InfoSchema`。此时 `transaction` 为 `None`，对会话没有副作用。
2. `on_initialize` 首先记录 `Context`。`Default`/`WithBeginStatement` 进入 `activate_stale_txn`；`WithReplaceProvider` 进入 `enter_new_stale_txn_with_replace_provider`；未支持类型立即返回 `Unsupported`。
3. `activate_stale_txn` 先从会话中克隆 backend，在不持有 session 锁时调用 `commit_before_enter_new_txn`；然后按 `ts` 创建事务，设置只读 stale 与 `global` 属性。
4. 它通过 `get_session_snapshot_info_schema(session, ts)` 取历史 schema；该辅助函数调用 `SessionBackend::snapshot_info_schema` 并强制标记本地临时表已挂接（`util.rs:317-329`）。
5. 再次锁定会话，将 assertion level、shard step 和临时表拦截标记写入事务/快照；同时安装 `TransactionContext`、`active_transaction`，设置 `provider_is_staleness = true`，并清空 `snapshot_system_variable`。
6. 替换 Provider 的分支不调用 `create_transaction`。若构造时没有 schema，先按 `ts` 获取；然后创建或复用 `session.txn_context`，只更新 schema、stale 标记和 `global` scope。
7. 读时间戳时，`stmt_read_ts` 检查 `!autocommit && !in_txn`。仅在该条件下调用 `activate_txn`，成功后才把 `in_txn` 设为 `true`，最终恒返回固定 `ts`。
8. 获取快照时，`snapshot_with_stmt_read_ts` 先复用上述自动激活逻辑；若 `session.active_transaction` 存在且 `valid`，取其快照，否则调用 backend 的 `snapshot_with_ts(ts)`。然后按需透传 Follower/Mixed 偏好，并无条件设置 `staleness_read_only = true`。

## 数据与状态

`ts` 在 Provider 生命期内不变，是事务 `start_ts`、历史 `InfoSchema.snapshot_ts` 和无活跃事务时新快照的共同键。激活后的主要不变式是：`TransactionContext.is_staleness == true`、事务与上下文的 scope 都是 `global`、事务和返回的快照都被标记为 stale-read-only。

Provider 与会话各持有一层事务状态：`session.active_transaction` 是会话可见的活跃事务，`self.transaction` 是 `activate_txn` 用于避免重复创建的缓存。`activate_stale_txn` 只写入前者，`activate_txn` 在激活后才把其克隆到后者。因此不能假设仅调用 `on_initialize(Default)` 就会填充 Provider 缓存；当前测试仅分别验证初始化与 `activate_txn` 的单次缓存语义。

`context` 在轻量 Rust 模型中是零字段占位类型；`on_stmt_start` 和 `on_stmt_retry` 只替换它，当前不用于取消或 deadline 传播。`InfoSchema.local_temporary_tables_attached` 被复制到事务和快照的 interceptor 布尔标记。

## 依赖与调用关系

直接下游都由 crate 根重导出：`errors.rs` 提供 `Error`/`ErrorKind`，`util.rs` 提供会话、backend、事务、快照、schema 模型以及 `get_session_snapshot_info_schema`。关键调用边为：

- `on_initialize -> activate_stale_txn -> SessionBackend::{commit_before_enter_new_txn, create_transaction, snapshot_info_schema}`；
- `on_initialize -> enter_new_stale_txn_with_replace_provider -> get_session_snapshot_info_schema`（仅 `info_schema` 缺失时）；
- `stmt_read_ts -> activate_txn -> activate_stale_txn`（仅 autocommit 关闭且未进事务时）；
- `snapshot_with_stmt_read_ts -> stmt_read_ts`，并在无有效活跃事务时调用 `SessionBackend::snapshot_with_ts`。

RustCodeGraph 对精确方法的 callers 主要是 `provider_test.rs` 中的测试，与 `rg` 的全库 Rust 直接引用结果一致：当前没有 Rust 事务管理器把这个 struct 当作 trait object 安装。Go 侧生产上游则是 `sessiontxn.GetTxnManager(...).EnterNewTxn(...)`，`provider_test.go::createStaleReadProvider` 展示了显式 stale `BEGIN` 与替换 Provider 两种接入方式。

## 错误处理与边界

所有可失败的后端操作都用 `Result<_, Error>` 向上传播：提交旧事务、创建新事务、获取历史 schema 和按 ts 获取快照的失败不被吞掉。`Mutex` 中毒被统一转为 `ErrorKind::Backend` 且消息为 `session lock poisoned`。

`stmt_for_update_ts` 和 `snapshot_with_stmt_for_update_ts` 恒返回 `ErrorKind::Unsupported`。`on_initialize(Unsupported)` 也返回同类错误。`activate_txn` 在后端报成功但会话未留下 `active_transaction` 时会构造 Backend 错误，而不是 panic。

`enter_new_stale_txn_with_replace_provider` 先确保 `info_schema` 存在，随后的 `expect("info schema initialized")` 依赖这一局部不变式。`txn_scope` 在锁中毒或没有 `txn_context` 时都返回空字符串；`read_replica_scope` 在锁中毒时回退到 `global`，这两个读取 API 不向上抛错。

## 并发与资源生命周期

`SessionRef = Arc<Mutex<Session>>` 允许 Provider、调用方和 backend 夹具共享会话。实现尽量缩短锁持有时间：`activate_stale_txn` 先在锁内克隆 `Arc<dyn SessionBackend>`，然后解锁再执行 commit/create/schema 调用，最后重新加锁以原子地安装会话状态。`snapshot_with_stmt_read_ts` 也在锁内只克隆 backend、事务和副本读配置，后端取快照在解锁后进行，避免 backend 回调时长时间占用会话锁。

本文件不创建线程、async task 或通道。事务和快照在轻量模型中是可 `Clone` 的值，资源清理由调用方替换/丢弃会话状态完成；这不等同于 Go `kv.Transaction` 真实网络资源的生命周期。`on_stmt_commit`、`on_stmt_rollback`、`on_pessimistic_stmt_start/end` 都不释放或改写事务，因为 stale transaction 被建模为只读。

## 与 Go 版本的对应关系

Rust `StalenessTxnContextProvider` 逐项对应 `pkg/sessiontxn/staleread/provider.go` 的同名类型：`new` 对应 `NewStalenessTxnContextProvider`，`on_initialize`/`activate_stale_txn`/`enter_new_stale_txn_with_replace_provider` 对应 Go 的初始化三件套，`stmt_read_ts` 保留 issue #64198 所需的 autocommit=0 自动激活语义，`snapshot_with_stmt_read_ts` 保留活跃事务快照复用、临时表 interceptor、follower-read 透传和 stale-only 标记。ForUpdate 两个 API 的拒绝语义也一致。

主要移植差异是：

- Go 实现真正实现 `sessiontxn.TxnContextProvider`，使用 `sessionctx.Context`、`kv.Transaction`、`internal.CommitBeforeEnterNewTxn`、TS future 和 TiKV snapshot；Rust 实现没有实现对应 trait，依赖本 crate 的同步内存模型。
- Go `context.Context` 能携带取消与 deadline；Rust `Context` 当前是空结构。
- Go 使用真实配置获取 replica scope，Rust 从 `Session.txn_scope_config` 字符串读取。
- Go 在事务上安装 KV vars/options、设置 `CreateTime`、配置 row-ID shard generator，并通过 `SetSystemVar` 清空 `tidb_snapshot`；Rust 仅保存 assertion/shard/interceptor 的建模字段并直接清空字符串，不是完整 sessionvars 副作的替代。
- Go 错误是 `error`/`errors.Trace`；Rust 使用局部 `ErrorKind` 分类。Go 对未支持进入类型返回格式化文本，Rust 还附带可程序检查的 `Unsupported` 种类。

Go `provider_test.go` 通过 `testkit`/mockstore 验证真实事务管理器、global scope、非 autocommit 激活和 follower-read RPC；Rust `provider_test.rs` 通过 `main_test.rs::MockBackend` 验证同一状态机的局部语义，但不是端到端 TiKV 证据。

## 扩展指南

- 新增进入模式时，同步扩展 `EnterNewTxnType` 和 `on_initialize` 分派，并在独立的 `provider_test.rs` 中分别证明是否 commit 旧事务、是否 create transaction、是否安装 txn context；不要把测试写进生产文件。
- 改动激活顺序时，保持“先 commit 旧事务，再 create transaction，成功获取 schema 后才安装新会话状态”的错误边界，并补充 backend 在每一阶段失败时的状态回归测试。
- 修改快照策略时，同时覆盖有效活跃事务复用、无/无效活跃事务调 backend、Leader/Follower/Mixed 以及本地临时表 interceptor，确保不会意外读最新版本。
- 如要接入 Rust 生产主链，需先定义并实现与 `sessiontxn.TxnContextProvider` 等价的 trait，将 `cfg(any())` 下的真实依赖转成可构建依赖，并用真实 session/kv 适配器取代 `util.rs` 的轻量类型。这是跨 crate 集成工作，不应只在本文件中补一个表面 trait impl。
- 对齐 Go 时优先保留 `provider.go` 的完整副作，包括 context 取消、KV vars/options、`CreateTime`、row-ID shard 生成器、系统变量错误和真实临时表 interceptor；不应以当前内存模型通过单测作为删减这些语义的理由。
- 需要特别评估的风险包括：锁持有期间的 backend 回调死锁（并发）、错误后会话半更新（正确性）、scope 或 follower 偏好丢失（兼容性），以及无效事务下频繁重建快照（性能）。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/sessiontxn/staleread` 列出本 crate 的 20 个 Go/Rust 文件。
- RustCodeGraph `node --file pkg/sessiontxn/staleread/provider.rs --offset 1 --limit 420`：核对了全文 326 行、所有公开/私有符号、分支和状态写入。
- RustCodeGraph `query StalenessTxnContextProvider --json`、`query on_initialize --json`、`query activate_stale_txn --json` 与定向 `explore`：核对 Rust/Go 同名类型、核心调用边及 `provider_test.rs` 的直接 callers。
- 已读生产与配置证据：`provider.rs`、`util.rs`、`errors.rs`、`lib.rs`、`Cargo.toml`、Go 对照 `provider.go`。目标目录与上级 `pkg/sessiontxn` 均没有 `doc.go`，因此无额外包约定可读。
- 已读测试证据：Rust `provider_test.rs`、`main_test.rs`，Go `provider_test.go`。Rust 用例覆盖两种激活进入、替换 Provider、Unsupported/ForUpdate 错误、scope、非 autocommit 惰性激活、事务缓存、快照复用/新建和 follower read；Go 用例补充了真实 TxnManager/testkit 与 RPC 层证据。
- `rg` 只用于补足图未清晰表达的生产接线边界：全库 Rust 直接引用未发现非测试的 `StalenessTxnContextProvider` 构造或调用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另行执行固定 11 章节的结构验证。

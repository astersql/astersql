# `pkg/session/runtime/transaction.rs`

源文件：[`transaction.rs`](transaction.rs)

## 文件定位

本文件是 `astersql-session` crate 内 `runtime` 模块的事务共享状态层，由 `pkg/session/runtime.rs` 以私有子模块 `mod transaction` 装配，并通过父模块的 `use super::*` 让同一运行时下的控制、DML、查询、DDL 和诊断代码共享其类型与注册表。它不负责解析 SQL、直接提交 KV 事务或实现完整的 Go `LazyTxn`；它集中定义这些流程共同依赖的进程内事务数据结构、全局注册表和小型状态操作函数。

`pkg/session/Cargo.toml` 将该代码归入 `astersql-session`，并声明它直接使用到的 crate 边界，包括 `astersql-domain`、`astersql-kv`、`astersql-meta-model` 和 `astersql-config`。文件中的绝大多数接口为 `pub(super)`，可见范围刻意限制在 `runtime` 父模块，而不是 crate 的公共 API。

## 核心职责

- 为非 TiKV/本地运行时维护按域隔离的共享/排他行锁表、FIFO 等待队列、单出边等待图以及锁所有者到连接号的映射，核心类型是 `RuntimeRowLockKey`、`RuntimeRowLockMode`、`RuntimeRowLockWaiter` 和 `RuntimeRowLockState`。
- 为保存点保存事务可见 KV、已持有锁、延迟的乐观约束错误和待处理 TTL 插入计数，载体是 `RuntimeSavepoint`。
- 为 `TIDB_TRX`、锁等待和死锁诊断路径保存活跃事务快照与最近死锁事件，入口是 `RuntimeTxnInfo`、`RuntimeTxnInfoGuard`、`record_runtime_deadlock` 及相关全局表。
- 用逻辑提交纪元记录每个运行时键的最近提交，供乐观冲突检测和诊断写集判断使用；相关对象是 `NEXT_RUNTIME_COMMIT_EPOCH` 与 `RUNTIME_KEY_COMMIT_EPOCHS`。
- 保存按域覆盖的单条事务写入大小上限，并提供与 Go 默认值一致的 `DEFAULT_TXN_ENTRY_SIZE_LIMIT = 6 MiB`。
- 在 Rust DDL 建索引期间临时发布“前台写路径仍需维护”的索引，通过 `RuntimePendingWriteIndexGuard` 的 RAII 生命周期自动登记和撤销。
- 从存储标签推导事务作用域：`runtime_txn_scope` 读取 `zone`，缺失时返回 `global`。

## 主要符号

- `RuntimeRowLockKey { domain_id, key }`：锁表和提交纪元表的复合键。`domain_id` 防止两个 `Domain` 中字节相同的 KV 键互相干扰；`key` 是实际编码后的存储键。
- `RuntimeRowLockMode::{Shared, Exclusive}`：支持共享锁与排他锁。兼容性判定由 `ConcreteSession::acquire_row_lock_mode` 完成。
- `RuntimeRowLockWaiter { owner, mode }`：等待队列元素。`owner` 是 `NEXT_ROW_LOCK_OWNER` 分配的运行时事务所有者；`mode` 保留请求的锁模式。
- `RuntimeRowLockState`：包含 `holders`、`wait_for`、`waiters`、`connections` 四张表。`wait_for` 每个等待者只保存一个当前阻塞者，因此 `runtime_lock_cycle` 可以沿单链检测环。
- `RuntimeSavepoint`：保存名称和四类可回滚状态。名称在创建时转为小写；`visible` 是保存点时事务可见 KV 的完整快照。
- `RuntimeTxnInfo`：事务诊断快照，包括域、起始时间戳、当前/历史 SQL digest、状态、等待起始时刻、内存缓冲区键/字节数、会话号和数据库。
- `RUNTIME_ROW_LOCKS`：`Mutex<RuntimeRowLockState>` 与 `Condvar` 的二元组；锁释放或等待者移除后由条件变量唤醒竞争者。
- `RUNTIME_TXN_INFOS`：按锁所有者保存活跃事务诊断信息；`RuntimeTxnInfoGuard::drop` 和会话结束路径负责清理。
- `RUNTIME_KEY_COMMIT_EPOCHS`：按 `RuntimeRowLockKey` 保存最后提交纪元；DML/事务提交成功后更新，事务开始时读取全局纪元作为冲突基线。
- `RUNTIME_GLOBAL_TXN_ENTRY_SIZE_LIMITS`：按运行时域保存全局单条写入上限，未配置时回退到 `DEFAULT_TXN_ENTRY_SIZE_LIMIT`。
- `RUNTIME_PENDING_WRITE_INDEXES`：键为 `(domain_id, lower(database), lower(table))`，值为待写 `IndexInfo` 列表。
- `runtime_lock_cycle`、`record_runtime_deadlock`、`remove_runtime_waiter`、`release_runtime_row_locks`：分别负责环检测、死锁事件记录、单锁等待状态清理和部分/全部持锁释放。

## 执行流程

**本地行锁竞争。** `ConcreteSession::acquire_row_lock_mode`（`control.rs`）取得 `RUNTIME_ROW_LOCKS` 的互斥量，先检查已有持有者和队首等待者。锁可授予时调用 `remove_runtime_waiter` 同时清理等待图与队列，再写入 `holders`；已有兼容锁时也清理旧等待状态并直接返回。发生冲突时把请求追加到对应键的等待队列，并令 `wait_for[owner] = holder`。

**死锁与等待退出。** 新增等待边后，`runtime_lock_cycle(state, owner, holder)` 从阻塞者开始沿 `wait_for` 前进；若回到等待者即形成环。`record_runtime_deadlock` 为该环分配一个 `deadlock_id`，逐边写入 `RUNTIME_DEADLOCK_HISTORY`，随后 `control.rs` 移除死锁牺牲者的等待状态、唤醒其他线程、终止其事务并返回 TiKV 1213 语义的错误。中断、NOWAIT、执行时间上限和锁等待超时分支同样调用 `remove_runtime_waiter`，避免残留队列元素。

**锁释放。** `release_runtime_row_locks(owner, Some(keys))` 只释放指定集合，供保存点回滚、批量加锁失败和适配器失败回滚使用，并保留 `connections` 关联；传入 `None` 时释放所有锁、等待边、排队请求和连接关联，供提交、回滚及 `ConcreteSessionInner::drop` 使用。两种路径最后都 `notify_all`，让等待者重新检查条件。

**保存点。** `create_savepoint`（`control.rs`）在有活动事务时抓取完整可见 KV，并复制持锁集合、延迟约束错误和 TTL 计数；同名保存点先被替换。`rollback_to_savepoint` 通过删除保存点之后新增的键、恢复值发生变化的键来还原 KV，再恢复三类伴随状态，截断更晚的保存点并仅释放保存点之后获得的锁。`release_savepoint` 只删除命名快照。

**提交冲突纪元。** 开始事务时，`begin_transaction` 将 `transaction_read_epoch` 设为 `NEXT_RUNTIME_COMMIT_EPOCH` 当前值；提交成功后 `control.rs` 或自动提交 DML 路径递增纪元，并为实际写键和乐观 `FOR UPDATE` 键更新 `RUNTIME_KEY_COMMIT_EPOCHS`。后续提交检查以“某键最后纪元是否晚于本事务读取纪元”判断运行时期间的写冲突。

**待写索引。** DDL 建索引阶段调用 `RuntimePendingWriteIndexGuard::register`，用规范化库表名登记临时索引；DML 在编码写入时读取该表并额外维护临时索引键。DDL 作用域退出时 `Drop` 按索引名删除登记，最后一个索引移除后再删除整个表项。

**事务诊断。** 语句开始/结束和 DML 执行路径更新 `RUNTIME_TXN_INFOS`；锁等待时状态切换为 `LockWaiting` 并记录 `Instant`。`system_query.rs` 读取该表及锁表生成事务和等待诊断结果，读取 `RUNTIME_DEADLOCK_HISTORY` 生成死锁历史。守卫或会话析构确保事务结束后不继续暴露陈旧信息。

## 数据与状态

所有进程级注册表都使用 `LazyLock` 延迟初始化。锁键和提交纪元键包含 `domain_id`，待写索引键同样包含域标识，因此共享静态表仍维持运行时实例隔离。事务诊断快照也显式携带 `domain_id`，查询路径可筛选当前域。

`RuntimeRowLockState` 必须维持三个互相关联的不变量：已授予锁只存在于 `holders`；等待锁同时存在于某个 `waiters[key]` 队列和 `wait_for[owner]`；空持有者映射与空等待队列应从外层表移除。`remove_runtime_waiter` 和 `release_runtime_row_locks` 集中维护后两个不变量，调用者不应只修改其中一张表。

死锁历史的容量以“完整死锁事件”而非记录行数计算。一次 N 边死锁会产生 N 条相同 `deadlock_id` 的记录；超过 `DEFAULT_DEADLOCK_HISTORY_CAPACITY`（10）时，函数删除最老事件的全部记录，避免留下半个等待环。

提交纪元是进程内单调逻辑值，不等同于存储层 TSO 或真实 commit TS。代码在存储事务有 commit TS 时仍用逻辑纪元做本地写冲突索引，而会话另行记录实际 `CommitTS()`。该表当前没有回收策略，新增长期运行场景时需评估键数量增长。

## 依赖与调用关系

上游装配点是 `pkg/session/runtime.rs`；RustCodeGraph 将目标文件标记为被 `pkg/session/runtime/control.rs`、`dispatch.rs`、`scan_adapter_runtime.rs`、`session.rs`、`pkg/ddl/job_scheduler.rs` 等文件使用。源码核验得到的关键直接关系如下：

- `session.rs` 在构造会话时用 `NEXT_ROW_LOCK_OWNER` 分配所有者，并在会话状态中保存 `RuntimeSavepoint` 和 `RuntimeRowLockKey` 集合。
- `control.rs` 是本文件锁函数、保存点结构、诊断信息和提交纪元的主要编排者；它还在事务开始时调用 `runtime_txn_scope`。
- `dml.rs` 写入/读取 `RUNTIME_TXN_INFOS`、提交纪元和待写索引；`ddl.rs` 注册待写索引并读取事务条目大小限制。
- `query.rs`、`scan_adapter_runtime.rs` 和 `typed_adapter_bridge.rs` 构造锁键，在语句失败或局部回滚时调用部分锁释放。
- `dispatch.rs` 读取全局条目限制和事务作用域，并在会话析构时执行全量锁与诊断清理。
- `system_query.rs` 消费事务信息、锁表、死锁历史和提交纪元，为诊断 SQL 组织结果。

本文件通过 `super::*` 使用父模块已导入的标准库同步/集合类型、`Domain`、`RuntimeDeadlockRecord` 等定义；`IndexInfo` 使用完整路径 `astersql_meta_model::IndexInfo`。`Cargo.toml` 没有为本文件设置专门 feature，`nextgen` feature 也未在本文件中形成条件编译分支。

## 错误处理与边界

本文件自身的函数不返回业务错误：它们要么返回布尔值/字符串，要么修改受锁保护的状态。业务错误由调用者产生，例如死锁返回 `[tikv:1213]`、NOWAIT 返回 3572、锁等待超时返回 1205、最大执行时间返回 3024、保存点缺失返回 1305。

所有全局 `Mutex` 在中毒时都以 `PoisonError::into_inner` 继续使用内部状态。这避免单次 panic 令注册表永久不可用，但意味着调用者接受“状态可能在 panic 中途只完成部分修改”的风险；修改多张关联表时必须保持临界区短且更新顺序一致。

`runtime_lock_cycle` 使用 `visited` 防止已损坏或不含原等待者的环导致无限循环；只有路径实际到达 `waiter` 才报告新死锁。`record_runtime_deadlock` 若等待边在遍历中断开，会记录已看到的边并停止；正常调用点在持有同一锁表互斥量时执行，因此不会并发断边。

`runtime_txn_scope` 只识别键名精确为 `zone` 的标签，不规范化标签名，也不验证空字符串；没有该键才使用 `global`。`RuntimePendingWriteIndexGuard::drop` 以索引小写名字段 `Name.L` 匹配，因此相同域/库/表下同名登记会一并被移除，调用方需保证同名 DDL 生命周期不重叠。

保存点仅在活动事务存在时创建；无事务时为无操作。悲观事务关闭原地约束检查时显式拒绝保存点。回滚依赖完整可见 KV 快照，事务较大时具有额外内存和遍历成本。

## 并发与资源生命周期

`RUNTIME_ROW_LOCKS` 的互斥量保护持有者、等待图、队列和连接映射，`Condvar` 只作为状态变化通知；等待者醒来后必须重新在循环中检查锁条件。队列只允许队首竞争空闲锁，结合共享锁兼容规则提供基本排队公平性。真正 TiKV 事务的锁等待由 TiKV lock manager 处理，`control.rs` 明确绕过该本地锁表；这里主要覆盖本地/非 TiKV 运行时。

所有者号、死锁号和提交纪元分别由原子计数器分配。所有者和死锁号使用 `Relaxed`，因为唯一性而非跨数据结构发布顺序是其目的；提交纪元读取/递增使用 `Acquire`/`AcqRel`，与提交后的键纪元登记共同形成运行时冲突判断时序。

`RuntimePendingWriteIndexGuard` 与 `RuntimeTxnInfoGuard` 通过 `Drop` 做资源回收。会话析构还有显式兜底：回滚仍活动的存储事务、全量释放行锁并移除诊断信息。部分锁释放不会删除所有者到连接的关联，保证保存点回滚后剩余事务仍可被诊断；全量释放才删除关联。

`RuntimeSavepoint`、诊断 digest 列表和键纪元表都可能随事务或进程增长。保存点随事务开始清空、更晚保存点在回滚时截断；诊断信息随事务/会话结束清理；键纪元表没有在此文件中清理。扩展时不能在持有全局互斥量期间执行存储 RPC 或长耗时编码。

## 与 Go 版本的对应关系

Rust 文件是对 Go 多处事务能力的集中式运行时实现，不存在同路径 `pkg/session/runtime/transaction.go`。最接近的 Go 证据分散如下：

- `pkg/session/txn.go` 的 `LazyTxn` 维护底层 `kv.Transaction`、语句 staging、`TxnInfo` 和 `lastCommitTS`；Rust 的 `RuntimeSavepoint`、`RuntimeTxnInfo` 及提交状态承担其中一部分运行时语义，但 Rust 保存点以可见 KV 完整快照实现，并不是 Go `MemDBCheckpoint` 的直接类型移植。
- `pkg/session/txninfo/txn_info.go` 定义 `TxnInfo` 以及 `TIDB_TRX` 列转换。Rust `RuntimeTxnInfo` 保留起始 TS、当前/全部 digest、运行状态、等待开始时间、缓冲区规模、会话与数据库等核心展示数据；Go 还包含状态变更时间、用户、相关表等更丰富信息，字段并非完全等价。
- `pkg/config/config.go` 的 `DefTxnEntrySizeLimit` 是 `6 * 1024 * 1024`，与 Rust `DEFAULT_TXN_ENTRY_SIZE_LIMIT` 的 `6_291_456` 一致；`DefaultPessimisticTxn().DeadlockHistoryCapacity` 为 10，与 Rust 默认死锁事件容量一致。Rust 当前使用编译期常量 10，而不是直接读取该 Go 风格配置项。
- Go 悲观锁与死锁历史主要依赖 TiKV/client-go 的事务和锁管理设施；Rust 在 TiKV 存储上同样走 `LockKeys`，但为非 TiKV 运行时额外实现进程内 `RUNTIME_ROW_LOCKS` 与等待图。因此本文件的锁表不是 Go 中某个单一结构的逐字段复制。
- Go 在线 DDL 通过 schema state 和表写路径维护 write-only/write-reorganization 索引；Rust `RuntimePendingWriteIndexGuard` 是当前运行时的局部接线机制，用 RAII 临时注册前台 DML 应维护的索引，而不是 Go 的公共 API 对照物。
- Go 事务作用域最终写入 `kv.TxnScope`，默认是 `kv.GlobalTxnScope`；Rust `runtime_txn_scope` 从全局存储标签的 `zone` 推导，并在 `begin_transaction` 中设置同类事务选项。

这些差异应被视为当前实现事实。扩展 Rust 行为时应保持用户可见错误、默认值、诊断列和事务/DDL 状态语义与 Go 一致，而不应仅追求内部数据结构同形。

## 扩展指南

- 新增锁模式或改变兼容规则时，同时修改 `RuntimeRowLockMode`、`ConcreteSession::acquire_row_lock_mode` 的持有者兼容判断和队列授予逻辑，并在独立测试文件中覆盖共享/排他组合、升级、队首公平、NOWAIT、超时、中断和死锁清理。
- 修改等待图时必须同步维护 `wait_for` 与 `waiters`；优先复用 `remove_runtime_waiter`，并确认所有退出分支都会通知条件变量。多出边等待图将使当前链式 `runtime_lock_cycle` 和死锁记录算法失效，需要成套替换。
- 扩展保存点状态时，在 `RuntimeSavepoint`、`create_savepoint` 和 `rollback_to_savepoint` 三处成对增加快照与恢复，并核对事务开始/结束清理。测试应放在独立 `*_test.rs`，不要嵌入生产文件。
- 增加事务诊断字段时，同步更新 `RuntimeTxnInfo` 的创建/刷新路径与 `system_query.rs` 的展示映射，并对照 `pkg/session/txninfo/txn_info.go` 的 Go 列语义，明确空值、时间单位和域隔离。
- 改动提交冲突检测时，区分逻辑纪元和存储 TSO；只有提交成功的实际写键才能发布新纪元，并评估 `RUNTIME_KEY_COMMIT_EPOCHS` 的回收策略、内存上界及删除旧键是否会产生漏报。
- 改动待写索引注册时，应保持库表名规范化、域隔离和异常路径自动撤销，并核对 DML 临时索引编码与 DDL 合并/重复检查。若允许同名并发登记，应将当前“按索引名 retain 删除”改为带唯一登记身份的精确移除。
- 更改事务条目默认限制或作用域推导时，同步核对 Go `pkg/config/config.go`、Rust 的全局/会话覆盖路径及 `kv::TxnScope` 设置，避免配置展示值和执行值分离。

建议直接测试目标是 `pkg/session/runtime/transaction_test.rs`；跨流程回归应扩展相应的独立 `control`/正常 DDL/系统查询测试文件。当前直接测试只覆盖死锁历史容量及完整事件保留，其他关键不变量需要在相关运行时测试中补充时保持聚焦。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/transaction.rs` 找到该文件及 30 个符号；`node --file ... --offset 1 --limit 400` 读取完整 271 行源码并报告 7 个使用文件；对 `runtime_lock_cycle`、`record_runtime_deadlock`、`remove_runtime_waiter`、`release_runtime_row_locks`、`runtime_txn_scope` 执行了 `query`，并用图结果确认目标符号和 `control.rs` 中的锁循环调用关系。图的 `callers` 对同名/限定名没有返回稳定的逐符号调用者，因此调用点再以精确源码搜索核验，未把空图结果解释成“无调用者”。
- 生产源码：`pkg/session/runtime/transaction.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/control.rs`、`dml.rs`、`ddl.rs`、`dispatch.rs`、`session.rs`、`query.rs`、`scan_adapter_runtime.rs`、`typed_adapter_bridge.rs`、`system_query.rs`。
- crate 边界：`pkg/session/Cargo.toml`，确认包名、库入口、`nextgen` feature 以及 Domain/KV/meta-model/config 等直接依赖；本文件没有条件编译生产项，只有 `#[cfg(test)]` 引入独立测试模块。
- Rust 测试：`pkg/session/runtime/transaction_test.rs` 验证 11 次二边死锁写入后只保留 10 个完整事件、共 20 条边记录；相关适配器测试搜索到外键保存点与客户端死锁流程，但它们不是本文件每个辅助函数的直接单元覆盖。
- Go 对照：`pkg/session/txn.go`、`pkg/session/txninfo/txn_info.go`、`pkg/config/config.go`，以及 `pkg/session/test/meta/session_test.go` 的保存点/TTL 行为证据。仓库不存在同路径 Go 文件，故文档明确采用职责级对照而非声称一一移植。
- 人工复核：已核对公开范围、锁状态不变量、死锁事件裁剪、保存点恢复、RAII 清理、逻辑纪元与真实提交 TS 的区别，并避免把未见于源码的能力写成已支持。

# `pkg/session/txn.rs` 逻辑说明

## 文件定位

`pkg/session/txn.rs` 属于 `astersql-session` crate；crate 根在 `pkg/session/lib.rs` 通过 `pub mod txn` 公开本模块，manifest 为 `pkg/session/Cargo.toml`。它提供一个会话侧事务兼容层：用 `LazyTxn` 包装抽象的 `TransactionBackend`，在底层事务尚未取得时保存 `TransactionFuture`，并集中处理语句级 staging、提交/回滚、悲观锁、公平加锁和事务摘要。

该文件不是底层 KV 事务实现。真正的存储行为均由 `TransactionBackend` trait 注入；本文件只定义生命周期协调规则。当前 Rust 生产代码中，`LazyTxn` 可通过 `impl SessionTransaction for LazyTxn` 接入 `pkg/session/session.rs` 的会话提交/回滚抽象，但仓库搜索没有找到生产路径直接构造 `LazyTxn`，也没有找到生产路径直接调用本文件的 `StmtCommit`、`StmtRollback` 或 `HasDirtyContent`。因此它目前更准确地说是已公开、可接线的移植边界，而不是 `ConcreteSession` 已使用的唯一事务实现。显式的跨文件 Rust 使用主要是 `KeyNeedToLock`/`KeyFlags` 测试。

## 核心职责

1. `LazyTxn` 表示 invalid、pending、valid 三种初始化状态，并把 future 完成后的后端安装、staging 初始化及事务摘要重置组合成一次状态迁移。
2. `initStmtBuf`、`flushStmtBuf`、`cleanupStmtBuf` 和 `cleanup` 管理单条 SQL 的写缓冲：成功时释放 stage，失败时清除 stage，避免失败语句的修改泄漏到事务后续阶段。
3. `Commit`、`Rollback`、`RollbackMemDBToCheckpoint` 和 `LockKeysFunc` 在调用后端的同时维护运行状态、最近提交时间与写入条目数。
4. `StartFairLocking`、`RetryFairLocking`、`CancelFairLocking`、`DoneFairLocking` 兼容事务仍为 pending 时的公平加锁请求。
5. `KeyNeedToLock` 将已解析的键类别和标志归约为“是否需要加悲观锁”的布尔决策，`KeysNeedToLock` 对当前 stage 批量应用该规则。
6. 文件还保留自动自增/自动随机 ID 重试的原子 mock 状态、失败 future、future 包装器，以及语句事务管理器钩子。这些是 Go 同文件职责的 Rust 移植接口，不等于所有 Go 行为均已实现。

## 主要符号

- `Key = Vec<u8>`、`StagingHandle = u64`、`InvalidStagingHandle = u64::MAX`：本模块自己的键和 stage 句柄抽象。注意 `pkg/kv/kv.rs` 另有同名类型/常量，二者当前不是同一类型边界。
- `MAX_TRANSACTION_STMT_HISTORY = 50`：`onStmtStart` 最多保存 50 个 SQL digest；超过上限仍更新当前 digest，但不追加历史。
- `TxnRunningState::{Idle, Running, LockAcquiring, Committing, RollingBack}`：本文件内部状态枚举。它与 `pkg/session/txninfo/txn_info.rs` 的整型状态不是同一类型。
- `KeyFlags`：把 Go 中通过 key/value 解码和 `kv.KeyFlags` 位集获得的语义预先展开为布尔字段。`table_key`、`record_key`、`index_key`、临时索引属性及唯一性属性都由调用者提供。
- `TransactionBackend: Send`：底层事务适配接口，覆盖有效性、只读/流水线属性、时间戳、内存/条目统计、stage 操作、提交回滚、锁、公平加锁、表信息缓存和 option 查询。
- `TransactionFuture: Send`：仅定义 `Wait`，返回一个装箱的 `TransactionBackend`。
- `LazyTxn`：核心状态容器。`Transaction` 与 `txnFuture` 决定初始化状态；`initCnt`/`stagingHandle` 记录语句缓冲；`enterFairLockingOnValid` 延迟公平加锁；`lazyUniquenessCheckEnabled` 控制 presume-not-exists 持久化；`lastCommitTS` 记录最近成功提交时间戳；`txnInfo`、`state` 和 `lastStateChangeTime` 保存可观测摘要。
- `impl SessionTransaction for LazyTxn`：向 `pkg/session/session.rs` 暴露 `Valid`、`IsReadOnly`、`Info`、`Commit`、`Rollback`。没有后端时 `IsReadOnly` 返回 `true`，`Info` 始终返回当前摘要副本。
- `txnFailFuture`：`Wait` 固定返回 `mock get timestamp fail`。
- `txnFuture`：保存 scope 和 pipelined 配置，但当前 `Wait` 仅转发给内部 future；这些配置字段没有在本文件应用到底层事务。
- `StatementTxnManager`、`StmtCommit`、`StmtRollback`：先调用语句钩子，钩子失败只记录而不向上传播，随后完成缓冲清理。
- `HasDirtyContent`：仅在后端存在、非 pipelined 且 `HasTablePrefix(table_id)` 为真时返回真。
- `EmptyOptions`：构造空的 `HashMap<i32, String>`，当前文件内无调用者。

## 执行流程

### 惰性初始化

1. `changeToPending` 清除现有 `Transaction` 并保存 future；此时 `pending()` 为真，`validOrPending()` 也为真。
2. `Wait(lazy_uniqueness_check_enabled)` 先拒绝 invalid 状态。若已经 valid，只返回自身且不会重写惰性唯一性开关。
3. pending 时调用 `changePendingToValid`：先 `take()` future，等待后端；失败则保持无后端并向上传播错误。
4. future 成功后安装后端、调用 `initStmtBuf`，如先前请求过公平加锁则清除延迟标志并调用后端 `StartFairLocking`。
5. 最后用后端 `StartTS`、`Len` 以及 pending 阶段已有的 SQL digest 重建 `txnInfo`。`Wait` 随后写入 `lazyUniquenessCheckEnabled`。
6. 若转换失败，`Wait` 调用 `cleanup`、将 `txnInfo.StartTS` 清零并返回原错误。由于 future 已被取走，此时事务不再 pending。

### 语句缓冲

1. `initStmtBuf` 记录后端当前 `Len`；非 pipelined 后端调用 `Stage` 并保存句柄，pipelined 后端不建立 stage。
2. 语句成功时 `flushStmtBuf` 可先调用 `PersistPresumeKeyNotExists`，再对非 pipelined 后端 `ReleaseStage`，更新 `initCnt` 并把句柄恢复为无效值。
3. 语句失败时 `cleanupStmtBuf` 对非 pipelined 后端 `CleanupStage`，更新 `initCnt` 和 `txnInfo.EntriesCount`，再使句柄失效。
4. `StmtCommit` 的顺序是 `OnStmtCommit`、`flushStmtBuf`、`cleanup`；`cleanup` 会再次尝试清理旧 stage（此时通常为空操作）并为下一条语句重新建立 stage。`StmtRollback` 则执行 `OnStmtRollback` 后直接 `cleanup`。

### 事务结束和加锁

1. `Commit` 要求 `Valid`，把状态置为 `Committing`，flush 当前语句缓冲，调用后端 `Commit`；仅成功时复制 `CommitTS` 到 `lastCommitTS`，之后无论成功失败都 `reset` 为 invalid。
2. `Rollback` 要求 `Valid`，把状态置为 `RollingBack`，先把内存变化钩子替换为 no-op，再调用后端 `Rollback`；之后无论结果如何都 `reset`。
3. `LockKeysFunc` 保存原状态，进入 `LockAcquiring`，同步调用后端 `LockKeys`，然后恢复原状态并刷新条目数；即使后端返回错误，状态恢复仍会执行。
4. `RollbackMemDBToCheckpoint` 先 flush stage，再委托后端回滚到检查点，随后 `cleanup` 以重新建立语句缓冲。

## 数据与状态

`LazyTxn` 的结构性不变量是：invalid 时后端和 future 均为空；pending 时仅 future 存在；valid 时后端有效且 future 为空。`Valid` 不仅检查后端存在及后端自身 `Valid()`，还明确要求 `txnFuture.is_none()`。`changeToPending`、`changePendingToValid` 和 `changeToInvalid` 是维护该不变量的主要入口。

`stagingHandle` 以 `InvalidStagingHandle` 表示没有活跃 stage。`countHint` 仅在句柄有效时计算 `Len - initCnt`，并用 `saturating_sub` 防止后端条目数下降造成下溢。pipelined 后端不会调用 `Stage`，因此通常保持无效句柄，`countHint` 和 `KeysNeedToLock` 返回空结果；这与 `HasDirtyContent` 对 pipelined 后端直接返回 false 的约束一致。

`txnInfo` 是 `pkg/session/session.rs` 中的精简 `TxnInfo`，字段只有 `StartTS`、字符串状态、条目数和 digest。`onStmtStart` 跳过空 digest；非空时进入 `Running` 并追加有限历史，`onStmtEnd` 清空当前 digest并回到 `Idle`。`updateState` 仅在状态实际改变时更新时间。`changeToInvalid` 会清空 `txnInfo` 和状态，但不清零 `lastCommitTS`，因此最近一次成功提交时间可跨事务保留。

两个进程级 `AtomicI64` 使用 Acquire/Release 或 AcqRel：自动自增标志一旦由 `enableMockAutoIncIDRetry` 置 1，本文件没有复位函数；自动随机 ID 计数由 `ResetMockAutoRandIDRetryCount` 设定、`decreaseMockAutoRandIDRetryCount` 递减，调用方必须先用 `needMockAutoRandIDRetry` 判断，否则可减为负数。

## 依赖与调用关系

直接 Rust 依赖很窄：标准库的 `HashMap`、`AtomicI64`、`SystemTime`，以及同 crate 的 `session::{SessionTransaction, TxnInfo}`、`SessionError`、`SessionResult`。`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`、库入口为 `lib.rs`，且 porting metadata 指向 Go 包 `pkg/session`；本文件没有直接导入 manifest 中的大量外部 crate。

上游边界包括：

- `pkg/session/lib.rs` 公开 `txn` 模块，并在 `cfg(test)` 下加载 `pkg/session/txn_test.rs`。
- `pkg/session/session.rs` 的 `session` 持有 `Box<dyn SessionTransaction>`；`doCommit`、`RollbackTxn`、`TxnInfo` 通过该 trait 工作。`LazyTxn` 的 trait 实现使其类型上可进入这条链，但当前仓库没有找到生产构造点。
- `pkg/session/session_test.rs` 与 `pkg/session/test/tidb_test.rs` 直接调用 `KeyNeedToLock`；`pkg/session/txn_test.rs` 直接调用自动自增 mock 标志函数。

下游调用全部落在 trait 后端：staging 方法负责写缓冲，`Commit`/`Rollback` 负责持久化终结，`LockKeys` 和公平加锁方法负责悲观事务锁流程，表缓存及 option 方法只是透明委托。RustCodeGraph 的文件节点报告 `pkg/session/txn.rs` 被 56 个索引文件使用，但精确 `callers`/`callees` 查询没有返回符号边；因此本文对具体 Rust 调用者以仓库文本搜索结果为准，不把文件级“used by”误写成函数级调用。

## 错误处理与边界

- `changePendingToValid` 在 future 缺失时返回 `transaction future is not set`；future 自身错误原样传播，且后端保持为空。
- `Commit`、`Rollback`、`LockKeysFunc`、`Wait` 在 invalid 状态下返回 `invalid transaction`。这些错误是本 crate 的消息型 `SessionError`，不是 Go 的 `kv.ErrInvalidTxn` 类型等价物。
- 公平加锁 API 对 invalid 状态返回明确错误。pending 状态下 `RetryFairLocking` 是成功空操作；`CancelFairLocking`/`DoneFairLocking` 只有延迟启动标志为真时才成功并清旗标。
- `StmtCommit` 与 `StmtRollback` 故意吞掉 manager hook 错误，只调用 `LogHookError`；函数自身无返回值。扩展 hook 时不能假设错误会阻止缓冲提交或清理。
- `flushStmtBuf` 和 `cleanupStmtBuf` 在句柄无效或后端缺失时静默返回。若后端实现违反“句柄只属于当前后端”的约束，本文件没有额外校验。
- `KeyNeedToLock` 信任 `KeyFlags` 已正确描述键和值。它不解析 table prefix、record/index key 或临时索引值，也不会报告解码错误；分类错误会直接改变锁集合。
- 多处 `expect` 依赖先前 `Valid` 检查或刚安装后端的不变量。自定义后端若在不可变 `Valid()` 与后续可变操作之间产生内部失效，可能触发 panic 或返回后端错误。

## 并发与资源生命周期

`TransactionBackend` 和 `TransactionFuture` 要求 `Send`，所以装箱对象可随拥有者在线程间移动；但 `LazyTxn` 的绝大多数状态变更要求 `&mut self`，文件内没有 `Mutex`/`RwLock`，也没有提供多个线程并发读写同一实例的机制。尤其 `txnInfo` 是普通字段，不具备 Go 版本为跨会话查询 `information_schema.tidb_trx` 使用的 copy-on-read 锁保护。上层若要共享实例，必须自行串行化访问。

future 的所有权在 `changePendingToValid` 中通过 `take()` 一次性消费；等待失败不会恢复 future。后端的所有权从 future 移入 `Transaction`，在 `reset`/`changeToInvalid` 时释放。stage 句柄从 `Stage` 产生，经 `ReleaseStage` 或 `CleanupStage` 终结；事务失效前 `changeToInvalid` 会先清理现有缓冲，防止遗留 stage。

`Rollback` 在释放后端前安装 no-op 内存钩子，避免后续回滚通知触及已失效的会话内存跟踪对象。锁状态恢复是同步的：`LockKeysFunc` 等待后端调用返回后恢复，而 Go 版本通过传给 `LockKeysFunc` 的回调在实际锁动作完成时恢复；若后端的 Rust `LockKeys` 语义未来变为异步，这个生命周期差异必须重新处理。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/session/txn.go`。Rust 保留了 `LazyTxn` 三态、语句 staging、digest 上限、提交/回滚、公平加锁、锁键筛选、future、脏内容判断和语句钩子的总体形状，但不是逐字段完整实现。

- Go 的 `LazyTxn` 直接嵌入 `kv.Transaction`；Rust 用本地 `TransactionBackend` trait 解耦，且当前没有看到生产 KV adapter。
- Go 的 `txnFuture.wait` 等待 TSO、按 scope/pipelined 参数调用 storage `Begin`，并在 TSO 失败时区分 UniStore；Rust `txnFuture::Wait` 只转发内部 future，保存的 scope/pipelined 参数尚未生效。
- Go 在 pending→valid 后设置 resource group 和事务 entry-size limit，并记录等待 TSO 耗时、trace region与日志；Rust没有这些上下文参数和副作用。
- Go `TxnInfo` 有锁保护、阻塞开始时间、状态指标和事务结束 recorder；Rust使用 `pkg/session/session.rs` 的精简摘要，只有独立的 `lastStateChangeTime`，没有指标、block time 或跨会话同步。
- Go commit 路径含多个 failpoint 和自动 ID 重试注入；Rust只保留原子 mock helper，没有在 `Commit` 中消费这些标志，也没有 `mockFutureCommitTS` 覆盖逻辑。
- Go `KeyNeedToLock` 从实际 key/value 与 `kv.KeyFlags` 解码表键、行键、索引、临时索引和唯一性；Rust要求调用者预填 `KeyFlags`。Rust 的 `next_gen` 分支表达了 next-gen 临时索引强制加锁，但解码失败日志这一 Go 分支不存在。
- Go `GetOption` 在无事务且查询 `TxnScope` 时返回空字符串；Rust无后端时对所有 option 都返回 `None`。
- Go `HasDirtyContent` 在 `session` 上迭代 MemDB table prefix；Rust委托后端 `HasTablePrefix`。两者都排除 pipelined 事务。

这些差异说明 Rust 文件已覆盖核心控制流和可测试的分支语义，但不能据此宣称 Go 的监控、上下文设置、failpoint、真实 TSO/storage 初始化和并发观察能力已经迁移完成。

## 扩展指南

- 接入真实事务后端时，应实现 `TransactionBackend` 的全部方法，并在独立测试文件中验证 invalid/pending/valid 迁移、stage 句柄配对、pipelined 空 stage、公平加锁延迟启动、提交失败后失效以及回滚钩子替换。不要把测试写回 `txn.rs`。
- 完善 `txnFuture` 时，最可能修改 `txnFuture::Wait` 和 `changePendingToValid`；需要明确 scope、start TS、pipelined 参数、resource group、entry-size limit 与不同存储错误策略，并对照 `pkg/session/txn.go`，避免只让 future 字段“存在但不生效”。
- 扩展 `KeyNeedToLock` 时，应同步 `pkg/session/test/tidb_test.rs` 的完整分支矩阵，并关注 `pkg/session/session_test.rs` 的最小回归用例。若改为使用 `pkg/kv` 的真实 `KeyFlags` 或 tablecodec 解码，需处理解码失败、next-gen 行为和非唯一索引的 `need_locked` 性能/正确性权衡。
- 增加事务可观测性时，应评估是否改用 `pkg/session/txninfo/txn_info.rs` 的完整模型；在此之前不能仅向精简 `TxnInfo` 加字段而忽略跨线程读写、状态指标和事务结束通知。
- 修改 `StmtCommit`/`StmtRollback` 的错误策略时，要保留或明确改变“hook 错误仅记录、缓冲仍收尾”的契约，并为 `StatementTxnManager` 使用 mock 后端添加独立测试。
- 性能敏感点是 `KeysNeedToLock` 对 stage 生成完整三元组向量再过滤，以及 digest 字符串克隆。后端接口若支持 visitor/iterator，可在保持语义和生命周期安全的前提下减少分配。
- 与 Go 对齐时应限制在真实差异的增量范围内；当前未接线的完整 KV adapter、监控子系统和会话运行时集成应作为独立任务处理，不能用本文件的局部改动伪装成全链完成。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/txn.rs` 确认目标文件已索引；两次 `node --file pkg/session/txn.rs` 读取了 1-751 行及其 115 个符号，并报告文件级 56 个使用方。`query` 分别定位了 Rust/Go 的 `LazyTxn`、`KeyNeedToLock`、`StmtCommit`、`StmtRollback`、`HasDirtyContent`。精确 `callers`/`callees` 未返回结果，故未用其推断函数级边。
- 已读生产源与装配：`pkg/session/txn.rs`、`pkg/session/lib.rs`、`pkg/session/session.rs`、`pkg/session/Cargo.toml`、`pkg/session/txninfo/txn_info.rs`。目标包不存在 `pkg/session/doc.go`。
- 已读 Go 对照与 Go 测试：`pkg/session/txn.go`、`pkg/session/test/txn/txn_test.go`；另通过符号搜索定位 `pkg/session/test/tidb_test.go::TestKeysNeedLock`。
- 已读 Rust 独立测试：`pkg/session/txn_test.rs`；并核对 `pkg/session/session_test.rs::canonical_key_lock_decision_covers_transactional_branches`、`pkg/session/test/tidb_test.rs::keys_need_lock_matches_go_branch_matrix`。这些测试分别覆盖 mock 标志的非消费读取和锁判定分支；当前未发现 `LazyTxn` 状态机、staging、提交回滚或公平加锁的直接 Rust 单元测试。
- 仓库文本搜索确认了具体跨文件引用，并确认生产代码没有直接构造 `LazyTxn` 或调用本文件的语句钩子函数。本文未运行 Cargo，符合本任务“纯文档分析、不运行 Cargo”的限制。

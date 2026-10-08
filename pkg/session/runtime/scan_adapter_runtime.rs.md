# `pkg/session/runtime/scan_adapter_runtime.rs`

## 文件定位

该文件位于 `astersql-session` crate 的会话运行时层，由 `pkg/session/runtime.rs` 以私有模块 `scan_adapter_runtime` 装配。它为 `pkg/session/runtime/typed_adapter_bridge.rs` 中公开的 `SessionBoundAdapterOwner` 实现 `pkg/executor/adapter.rs` 定义的 `AdapterRuntime` trait，把 executor 的通用语句执行协议接到一个具体的 `ConcreteSession` 上。上游的 `pkg/session/runtime/planning.rs`、`explain_analyze.rs` 和 `typed_adapter_bridge.rs` 会创建 `SessionBoundAdapterOwner` 并交给 `ExecStmt`；真正的计划执行、悲观锁、会话状态和可观测性回调则经本文件落到 session、domain、KV 和各统计子系统。

`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`，并直接依赖本文件使用的 `astersql-executor`、`astersql-kv`、`astersql-domain`、planner core/operator、resource group、store driver、statement summary、TopSQL、metrics、plugin 和 chunk 等 crate。该文件不是独立公共 API：除 `statement_ru_install_state_from_session_vars` 为 `pub(super)` 外，类型和辅助函数均限制在 runtime 模块内部；对外可见的是 `runtime.rs` 再导出的 `SessionBoundAdapterOwner`。

## 核心职责

1. **选择并构造执行器。** `BuildExecutor` 根据 `PlanKind` 和 owner 上绑定的 SQL/物理计划，在 ANALYZE、DML、常量终结计划、canonical table reader、带 leaf range 的物理计划、普通物理表扫和 KV snapshot scan 之间分派；`BuildPointGetExecutor` 处理 PointGet、prepared actor 缓存、诊断模式和事务锁运行时；`BuildExecutorForSelectLock` 专门打开受支持的加锁读计划。
2. **维持语句级事务语义。** `OnPessimisticStmtStart`/`OnPessimisticStmtEnd` 建立或释放 KV statement staging、保存语句前写集合和已持锁集合，并在失败时恢复 mem-buffer 计数、冲突上下文、DML 报告和待执行外键级联。重试路径由 `OnPessimisticLockError`、`OnPessimisticStmtRetry`、`RollbackStatementForRetry` 和 `MaximumPessimisticRetries` 共同支撑。
3. **实现点查与行锁桥接。** `SessionPointLockRuntime` 让 typed PointGet 优先读取当前事务本地写入，按 RR/RC 隔离级别检测读时间戳后的提交，并在失败或 `only_if_exists` 未命中时释放本次新取的锁。
4. **桥接会话副作用和生命周期。** 实现最大执行时间、SQL killer、prepared plan 收尾、found rows、CTE 清理、外键保存点、runaway checker、资源组、RU v2、慢查询、语句摘要、TopSQL、运行时统计、fair locking 和最终状态清理。
5. **保留 Go 执行链的兼容语义。** 这里不是 Go 文件逐行翻译，而是把 Go 中分散在 executor adapter、session、txn manager 和事务对象上的回调聚合到 Rust `AdapterRuntime` 契约中。

## 主要符号

- `SessionPessimisticTransaction`：持有 `Rc<ConcreteSession>` 和构造时的 `writes_before` 快照，实现 executor 私有契约 `pessimisticTxn`。`KeysNeedToLock` 返回本语句新增的事务写 key（排序后保证稳定），`IsValid` 查询 canonical transaction，`IsKeyWritten` 用包含 domain 身份的 `RuntimeRowLockKey` 判断本地写入。
- `SessionPointLockRuntime`：保存 session、`read_ts` 和 `read_committed`，实现 `PointLockRuntime`。`LocalValue` 的三态返回值区分“key 未被本事务写过”“本地删除”“本地值”；`LockKey` 负责快照检查、冲突判定、行锁获取和失败清理。
- `AdapterSummaryLazyInfo(StatementSummary)`：实现 `StmtExecLazyInfo`，把原始 SQL、编码/二进制计划和 plan digest 延迟提供给正式 statement summary；binding SQL/digest 当前明确返回空值。
- `begin_statement_mutation_stage` / `finish_statement_mutation_stage`：`SessionBoundAdapterOwner` 的内部辅助方法，调用 canonical transaction 的 `StageStatement`、`ReleaseStatement`、`CleanupStatement`，同时快照或恢复 session 侧派生状态。
- `impl AdapterRuntime for SessionBoundAdapterOwner`：文件主体，完整实现 executor 要求的执行构建、事务、重试、上下文、指标和清理接口。其 trait 定义在 `pkg/executor/adapter.rs:434`。
- `statement_ru_install_state_from_session_vars`：把 `SessionVars` 中的只读/SELECT/restricted SQL/request source/TTL/cursor/flat-plan 状态投影为 `StatementRUInstallState`；`StatementRUInstallState` 还会重新解析实际 AST，补偿 direct adapter 绕过 compiler `SetStatementReadOnly` 的路径。

## 执行流程

典型查询从 `planning.rs` 创建 `Arc<SessionBoundAdapterOwner>` 开始。执行框架先通过 `SetProcessInfo` 记录 SQL、开始时间和命令，并在配置了超时时创建 `ExecutionDeadline`。`RunawayBeforeExecutor` 在资源控制开启且 domain 有 manager 时派生 checker；若规则切换资源组，同时更新 owner override 和 session state，并把 checker 包成 KV 请求可使用的 `SessionRunawayChecker`。

构建阶段中，`BuildExecutor` 先重置 point/table-reader RU 证据。ANALYZE 和 DML 必须已有对应 SQL 绑定，否则返回明确错误；常量终结物理计划直接打开；普通查询必须已有 `TypedScanSpec`。存在物理绑定时，代码优先在存储支持 coprocessor、计划 prepared 且叶范围至多一个的条件下选择 `CanonicalTableReaderExecutor`，否则根据 leaf range 数量选择带 bindings 的物理计划或单表扫描；没有物理绑定才退回 `OpenTypedKVSnapshotScan`。

PointGet 路径先验证 `PlanKind`、实际 `PointGetPlan` 类型以及 binding version 与 `start_ts` 一致。满足“最新时间戳、非锁读、无事务、非分区、非 TableDual”时，可按 prepared key 重用缓存 actor；重用前必须 `RecreatedFromPlan`、重新绑定新的 snapshot source、重置 RPC read stats 和诊断模式。事务中则安装 `SessionPointLockRuntime`。RR 在锁前后均拒绝 `CommitTs > read_ts` 的值，并对当前不存在但历史存在的删除场景报写冲突；RC 返回锁后最新值。只存在本地写时直接采用事务 overlay。

悲观 DML 在 `OnPessimisticStmtStart` 验证活动事务、终止过期事务、可选启动 fair locking、记录既有锁并创建 statement stage。执行器通过 `PessimisticTransaction`、`CollectUnchangedKeysForXLock/SLock` 和 `LockKeys` 收集并锁定实际 key；`LockKeys` 会排除临时表 key 和不在逻辑 LockTableIDs 中的表 key，并把 exclusive/shared lock details 合并进 statement context。成功结束释放 stage；失败则 cleanup、恢复语句前状态并释放本语句新增锁。可重试冲突通过 error 分类决定重试、返回原错或替换错误，随后回滚语句并重新开始 fair-locking/staging。

语句结束阶段依次使用 `OnExecComplete`、`ExecLockMetrics`、`FairLockingFinishMetrics`、`OnFinishStatement`、`CommitDetailsForFinish`、`AttachFinishRuntimeStats` 和 RU evidence API 汇总锁、commit、scan、read-pool、write size、keys examined 及重试次数。`SlowQuery`、`Summary`、`TopSQLStart/Finish` 和 `ObserveStatementDuration` 投递正式观测数据。最后 `CleanupAfterFinish` 清零 MPP query 标识、staleness、TiFlash/table-cache 标志和 parse duration，并移除 runaway checker/资源组覆盖。

## 数据与状态

状态中心是 `SessionBoundAdapterOwner.session: Rc<ConcreteSession>` 及其 `state: RefCell<_>`、`SessionVars`。owner 自身的 `RefCell`/`Cell` 保存绑定计划、prepared cache、语句上下文、deadline、锁/stage 快照、runaway checker、TopSQL 当前执行以及 point-read 采集开关。`point_read_stats` 与 `table_reader_ru_evidence` 使用 `Arc<Mutex<_>>`，因为存储执行和统计生产者可能跨越 owner 的局部可变借用边界。

关键不变量如下：

- `physical_scan` 的物理类型和 version 必须与请求匹配；PointGet 不接受类型或 read TS 不一致的绑定。
- `RuntimeRowLockKey` 同时包含 `domain_id` 和原始 key，避免不同 domain 中相同字节 key 互相污染锁集合。
- 一个悲观语句只能对应一个 `statement_mutation_stage`；结束时 `take()`，从而保证 release/cleanup 至多一次。
- 失败恢复不仅回滚 KV stage，还恢复 `transaction_write_keys`、mem-buffer 计数、冲突上下文、DML report 和 pending FK cascades，防止 retry 看见前次尝试的派生状态。
- PointGet actor 可以缓存，但 snapshot source、read stats 和诊断模式不可跨语句复用。
- read-pool 统计通过 `point_read_pool_merged` 和 `point_read_pool_runtime_registered` 保证分别只合并、注册一次，且不关闭 RU evidence 采集。
- TopSQL 当前执行由 `(sql_digest, plan_digest, started)` 表示；`TopSQLFinish` 用 `take()` 保证一次性收尾，owner 的 `Drop` 还会对 statement stats 调用 `SetFinished`。

## 依赖与调用关系

上游关系由 RustCodeGraph 显示：目标文件作为模块由 `pkg/session/runtime.rs` 装配；`SessionBoundAdapterOwner` 的构造与绑定逻辑在 `typed_adapter_bridge.rs`，主要生产入口在 `planning.rs`、`explain_analyze.rs` 和 `typed_adapter_bridge.rs`。executor 的 `ExecStmt` 只依赖 `AdapterRuntime` trait，因此 session 特有状态不会泄漏进通用执行器。

主要下游分为：

- 执行/计划：`astersql_executor::adapter`、builder、typed point get，planner `Plan` 和 physical operator；本地辅助为 `canonical_table_reader.rs`、`typed_dml_executor.rs`、`typed_analyze_executor.rs`、`typed_adapter_bridge.rs`。
- 事务/存储：`astersql_kv::{Getter, Key, Context, Version}`、domain storage snapshot、`ConcreteSession` 的 canonical transaction、row-lock registry 和 savepoint API。
- 会话语义：`SessionVars`、`StmtCtx`、`TxnCtx`、prepared plan cache、foreign-key flags、CTE scopes 和 SQL killer。
- 资源与观测：resource-group runaway manager、RU v2 reporter、statement RU plan walk/result、store-driver read stats、statement summary v1/v2、TopSQL、plugin audit、metrics、slow-query domain sink 和 runtime stats collector。

`pkg/session/Cargo.toml` 对上述内部 crate 都使用 workspace 路径依赖；目标文件没有条件 feature 分支，只有 `BuildExecutor` 内一段 `#[cfg(test)]` 的全局内存仲裁预算采样。

## 错误处理与边界

该层统一返回 `AdapterResult`，常把下游错误以 `errors::New(error.to_string())` 转换到 executor adapter 的共享错误类型。缺失绑定、计划类型错误、非查询计划进入 scan runtime、非悲观事务进入锁流程、缺失 canonical transaction 等均立即报错，不用默认值掩盖状态缺失。

锁读取特别区分 KV `ErrNotExist`：它是合法的“无值”结果，其他存储错误原样传播。RR 对锁前/锁后新提交值以及“read_ts 时存在、当前已删除”的情况生成 `ErrWriteConflict`；若本次新取锁后失败或 `only_if_exists` 未命中，会从全局 row-lock registry 和 session `held_row_locks` 同时移除，避免泄漏部分锁。

外键保存点准备和释放失败只记录到 `effects.events`，因为 trait 对这些入口没有可传播错误；真正的 `HandleFKTriggerError` 回滚/释放失败则传播。`SlowQuery` 在没有 `process_started` 时不落正式慢日志；RU install 在 SQL mode、解析失败或非单语句时返回 `None`，明确表示缺少可信 eligibility，而不是猜测。指标初始化相关路径沿用下游保护策略，清理函数本身是幂等式地清除共享标志。

## 并发与资源生命周期

`Rc<ConcreteSession>`、`RefCell` 和 `Cell` 表明 owner 是绑定到 session 线程的非 `Send` 控制对象；跨边界资源通过 `Arc` 传递。`OwnedKVSnapshotSource`（定义于 `typed_adapter_bridge.rs`）让 detached executor 持有固定 MVCC version 和 domain，而不延长可变 session owner 的生命周期。

`ExecutionDeadline` 在 `SetProcessInfo` 中创建后台等待线程，`CancelMaximumExecutionTime`、`TopSQLFinish` 或 owner 后续替换 deadline 时通过 drop/cancel 结束；`KillSignal` 还会在轮询时根据 elapsed 主动发送 `MaxExecTimeExceeded`。PointGet cache 内 actor 使用 `Arc<Mutex<TypedPointGet>>`，每次复用都在锁内重建并重新绑定 source。read stats 和 table-reader evidence 也用 mutex 串行化读写。

行锁生命周期以“事务已持锁集合”和“语句开始快照”区分。失败结束或 retry 只释放语句新增锁，不触碰事务此前持有的锁；PointGet 自己只清理本次刚取得且未成功使用的锁。fair locking 由 canonical transaction 的 `StartFairLocking`/retry/cancel/done 流程管理，并在成功执行后根据 lock details 把 used/effective 标志原子写入 `TxnCtx`。

TopSQL、runaway checker、prepared pending cache、FK savepoint、statement stage 均用 `Option` 加 `take()` 表达唯一所有权和一次性收尾。`CleanupAfterFinish` 是每条语句的最终共享状态清理点；新增提前返回路径时必须保证执行框架仍会到达对应 finish/cleanup 回调。

## 与 Go 版本的对应关系

仓库不存在 `pkg/session/runtime/scan_adapter_runtime.go` 或同名 Go 类型，因此不能建立逐行或单文件一一映射。`pkg/session/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/session"` 说明 crate 的总体移植来源是 Go `pkg/session`；本文件的行为证据分散在以下 Go 实现中：

- `pkg/executor/adapter.go`：`ExecStmt` 的 build/open/next、悲观锁重试、finish、fair-locking 指标和 `SummaryStmt` 主流程，是 Rust `AdapterRuntime` 回调顺序的主要语义基准。
- `pkg/session/session.go`：会话 process info、SQL killer、慢日志、语句变量和执行期资源控制状态；其中 `session.SetProcessInfo` 对应本文件的 `SetProcessInfo` 边界。
- `pkg/session/txnmanager.go`：`OnPessimisticStmtStart`、`OnPessimisticStmtEnd`、`OnStmtCommit` 等事务管理回调，对应 Rust 中同名 adapter 方法和 canonical transaction stage。
- `pkg/session/txn.go`：`LazyTxn.LockKeys` 及 fair-locking start/retry/cancel/done，提供锁与事务生命周期的 Go 基准。

Rust 版本的结构差异是显式 trait 化：Go 可直接调用 session/txn manager，而 Rust executor 通过 `AdapterRuntime` 反向调用 session owner；同时 Rust 使用 `Rc<RefCell<_>>`、`Arc<Mutex<_>>` 和 `Option::take` 表达线程归属与一次性清理。`scan_adapter_runtime_test.rs` 中带 `go_merge_*` 名称的测试明确记录了 RU version、runtime evidence 和 install eligibility 等 Go 合并语义，但这不意味着整个文件来自单一 Go commit。

## 扩展指南

新增计划类型或扫描快路径时，优先修改 `BuildExecutor`/`BuildPointGetExecutor`，并同步检查 `SupportsSelectForUpdate`、`StatementReadTS`、prepared rebuild/cache、RU evidence 与 read-pool stats；不能仅让构建成功而遗漏 finish 统计或锁语义。新的 executor 分支应在独立文件实现，测试继续放在 `pkg/session/runtime/scan_adapter_runtime_test.rs` 或更贴近该执行器的独立 `*_test.rs`，不要把测试嵌入生产文件。

修改悲观事务时，要成对审查 `begin_statement_mutation_stage`、`finish_statement_mutation_stage`、`OnPessimisticStmtStart/End`、`RollbackStatementForRetry` 和 row-lock 差集释放。任何新增的语句级派生状态都必须加入 stage 的快照/失败恢复，否则第一次失败会污染 retry；对应回归至少覆盖成功、执行错误、锁冲突重试和事务仍可继续四类情况。

修改 PointGet 锁读时，必须分别覆盖 RR/RC、存在/缺失/删除、事务本地新增/更新/删除、等待超时、唯一索引双 key 和失败无残留锁。缓存优化必须重新绑定 snapshot/read-stats/diagnostic state，且不得把带事务或锁读 actor 放入无事务 prepared cache。

新增资源或观测字段时，应明确其开始、一次性 finish 和最终 cleanup：通常分别接入 `SetProcessInfo`/`TopSQLStart`、`OnFinishStatement`/`TopSQLFinish`、`CleanupAfterFinish`。同步检查 `StatementContext`、正式 session state 和 `effects` 测试镜像三者；性能风险集中在每语句解析、额外 snapshot/RPC、全局 mutex 和高频指标写入，兼容风险集中在 Go 回调顺序、错误分类与 retry 可见状态。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件已索引为 2,010 行、134 个符号。
- RustCodeGraph `files --filter pkg/session/runtime`：确认生产文件、独立测试 `pkg/session/runtime/scan_adapter_runtime_test.rs` 及相邻 runtime 文件均在索引中。
- RustCodeGraph `node --file pkg/session/runtime/scan_adapter_runtime.rs`：分段阅读完整源文件，核对三个内部类型、两个 stage 辅助方法、`AdapterRuntime` 全部实现及 RU helper。
- RustCodeGraph `node AdapterRuntime`：核对 trait 位于 `pkg/executor/adapter.rs:434`，其构建、事务、重试、统计和 cleanup 契约与实现逐项对应。
- RustCodeGraph `node SessionBoundAdapterOwner` 与 `node --file pkg/session/runtime/typed_adapter_bridge.rs`：核对 owner 的字段、`Drop`、`ExecutionDeadline`、snapshot source 和状态所有权。
- RustCodeGraph `node --file pkg/session/runtime.rs --offset 80 --limit 90`：确认生产模块与 `#[cfg(test)]` 独立测试模块的装配，以及 owner 的公开再导出。
- RustCodeGraph `query/callers/callees`：确认 RU helper 由本实现调用，下游构造 `StatementRUInstallState`；索引还显示目标文件由 runtime/system-session 链引用。对于 trait 动态调用，图不能穷举所有运行时 caller，因此同时以 `planning.rs`、`explain_analyze.rs` 和 `typed_adapter_bridge.rs` 中 owner 构造点作为入口证据。
- `pkg/session/Cargo.toml`：核对 crate 名、Go package 元数据、`nextgen` feature 以及 executor/KV/domain/planner/resource/metrics 等直接依赖；目标实现不受 `nextgen` 条件编译控制。
- `pkg/session/runtime/scan_adapter_runtime_test.rs`：独立测试覆盖悲观 DML 和重试、PointGet RR/RC 锁语义、等待超时与删除冲突、FK 保存点和 shared lock、runaway、RU/TopSQL/summary、慢日志、finish stats、fair locking、cleanup、read-pool 及 prepared cache 诊断重置。
- Go 对照：阅读/搜索 `pkg/executor/adapter.go`、`pkg/session/session.go`、`pkg/session/txnmanager.go`、`pkg/session/txn.go` 的对应执行、会话和事务边界；未发现同路径同名 Go 文件，文档已明确该限制。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前使用任务给定命令校验恰好 11 个固定二级标题，并人工核对没有把测试写入生产文件或声称不存在的同名 Go 映射。

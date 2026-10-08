# `pkg/sessionctx/variable/session.rs`

## 文件定位

本文件是 `astersql-sessionctx-variable` crate 中面向运行时的会话状态实现，由 [`lib.rs`](./lib.rs) 以 `pub mod session` 暴露。它把一次客户端会话中会反复被协议层、SQL 会话运行时、规划器和执行器读取或更新的状态聚合到 `SessionVars`，同时提供事务重试、事务上下文、用户变量、计划缓存参数、语句文本脱敏、运行时过滤器等小型状态机。它不执行 SQL、存储 IO 或事务提交；这些对象仍由 `pkg/session`、`pkg/planner`、`pkg/executor` 和存储模块持有。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，包名为 `astersql-sessionctx-variable`。本文件直接使用 `chrono`、`kv`、`parser-auth`、`parser-ast`、`stmtctx-dependency` 和 `vardef`；系统变量的规范定义、校验与 hook 实现在同 crate 的 [`sysvar.rs`](./sysvar.rs)、[`sysvar_builtins.rs`](./sysvar_builtins.rs) 和 [`variable.rs`](./variable.rs)，而不是本文件自行复制一套注册表。

## 核心职责

1. `SessionVars` 保存会话身份、连接状态、当前数据库、事务及语句上下文、系统变量快照、优化器开关、计划/列 ID、耗时与慢日志相关状态，并提供这些状态的窄接口。
2. `RetryInfo` 与 `TransactionContext` 保存事务重试和事务生命周期数据：重试时按原顺序复用自增/自随机 ID，区分回滚到 savepoint 时需要恢复和无需恢复的字段，维护锁 key、表增量与悲观锁缓存。
3. `GetSystemVar`、`SetSystemVar` 和 Hint 系列方法把会话状态接到规范 sysvar 注册表，并维护 `SET_VAR` 的“校验—应用—登记原值—语句结束恢复”生命周期。
4. `UserVars`、`PlanCacheParamList`、`LazyStmtText` 分别封装用户变量的值/类型一致性、计划缓存参数及日志语句的惰性脱敏拼接。
5. 规划辅助状态包括隔离读引擎、MPP 开关、分区裁剪模式、优化器相关变量/fix 记录、备选逻辑计划临时覆盖、标量子查询注册、扩展列 ID、改写阶段耗时和原子 ID 分配。

本文件是 Go [`session.go`](./session.go) 的局部 Rust 移植，而非完整等价替换。Go 文件包含更多 TiDB 对象和方法；Rust 文件顶部明确将外部执行器、事务和存储对象留在各自模块，并且部分字段采用字符串或映射等较轻的边界表示。因此只能把本文点名的已实现路径视为当前能力。

## 主要符号

- `RetryInfo` / 私有 `RetryInfoAutoIDs`：保存 `Retrying`、待删除 prepared statement ID、RC 读时间戳，以及两组带读取游标的 ID。`Clean` 清空内容，`ResetOffset` 只把游标归零，`Add*ID` 与 `GetCurr*ID` 实现记录和顺序复用。
- `TransactionContext`：组合 `TxnCtxNeedToRestore` 与 `TxnCtxNoNeedToRestore`，另有公平锁原子标志。前者包含 `TableDeltaMap`、缓存表、TTL 插入计数和悲观锁缓存；后者包含时间戳、隔离级别、savepoint、未改动锁 key、MDL 与当前语句锁缓存。
- `RewritePhaseInfo`、`DurationOptimizer`、`TableDelta`、`SavepointRecord`：分别描述改写耗时、优化器阶段耗时、表统计增量和 savepoint 快照。`RewritePhaseInfo::Reset` 恢复默认值。
- `WriteStmtBufs`、`TableSnapshot`、`TemporaryTableData`：执行器可复用缓冲、快照行结果，以及临时表数据操作边界；trait 的写操作以 `Result<(), String>` 报错。
- `ReadConsistencyLevel` 与 `validateReadConsistencyLevel`：表示 strict/weak，并以不区分大小写的字符串校验入口约束合法值。
- `UserVars`：一个 `RwLock<UserVarsState>` 同时保护字符串值 map 和 `FieldType` map；`CloneVars` 深拷贝容器状态，`UnsetUserVar` 会同步删除值和类型。
- `PlanCacheParamList`：保存参数和 `forNonPrepCache` 标志。`String` 仅为 prepared cache 输出 ` [arguments: ...]`，非 prepared cache 隐藏参数。
- `LazyStmtText`：缓存最终日志字符串。`Update` 复制 SQL、模式和参数并使缓存失效；`String` 支持 `OFF`、`ON`、`MARKER` 三种脱敏模式，并对 marker 字符自身转义。
- `PartitionPruneMode`：`Valid` 拒绝过渡态 `StaticButPrepareDynamic`，`Update` 将 only/过渡枚举归一为最终 static/dynamic。
- `RuntimeFilterType`、`RuntimeFilterTypeStringToType`、`ToRuntimeFilterType`：解析 `IN` / `MIN_MAX`，去重且保持首次出现顺序；任一未知项使整个解析返回空列表和 `false`。
- `PlannerSelectBlockNames`：用 `RwLock<Option<Arc<Vec<HintTable>>>>` 提供块别名列表的快照式 `Load` / `Store`。
- `SessionVars`：本文件的核心聚合类型。重要方法族包括构造与当前库、表缓存语句边界、计划 ID、隔离读/MPP、系统变量与 Hint、事务模式、prepared ID、代价因子、优化器相关项、标量子查询、改写阶段状态及耗时。
- `SessionVarsProvider`：只读访问 `SessionVars` 的最小 trait，`SessionVars` 自身实现它。
- `SetEnableAdaptiveReplicaRead` / `IsAdaptiveReplicaReadEnabled`：进程级 `AtomicBool` 开关；前者返回值是否相对旧值改变。`ConnStatusShutdown` 是连接关闭状态常量。

## 执行流程

### 会话建立与普通语句

1. `SessionVars::new` 建立所有必需容器和默认值：事务/重试上下文、`StatementContext`、系统变量桥接容器、三种 planner `RefCell` 状态、优化器默认参数、UTC 固定偏移和默认允许的 TiKV/TiFlash/TiDB 隔离读引擎。
2. `pkg/session/runtime/session.rs` 将该值放入运行时会话；`pkg/server/conn.rs`、`pkg/server/runtime.rs`、`pkg/session/runtime/dispatch.rs` 与 `planning.rs` 是 RustCodeGraph 文件级使用方中的主要入口。
3. 语句规划前，`pkg/session/runtime/planning.rs` 调用 `BeginTableCacheStatement` 重置 `StmtCtx` 的表缓存读标记；规划器通过 `AllocNewPlanID` / `AllocPlanColumnID` 分配会话内单调 ID，并读取隔离引擎、MPP 和优化器开关。
4. 表缓存适配器调用 `MarkReadFromTableCache`，后续可由 `ReadFromTableCache` 查询；慢日志和执行统计读取起始时间、解析/编译耗时及原子计数。

### 系统变量与 `SET_VAR`

1. `SetSystemVar` 先调用 `SetHintSystemVarWithOldState`。后者确保内建 sysvar 已注册，从 `crate::GetSysVar` 取得元数据，用 `Validate(..., ScopeSession)` 规范化输入，并执行 `SetSessionFromHook`。
2. `SetSystemVar` 再通过 `GetHintSystemVar` 取得规范值；对会影响 Rust 热路径的变量同步更新强类型字段或 `StmtCtx`，最后写入 `systems`。解析失败时返回错误，且不会执行最后的 map 插入。
3. 读取时 `GetSystemVar` 对 warning/error count 直接读字段，其余按 Hint 状态、`systems`、规范 Hint getter 的顺序回退。`GetSessionOrGlobalSystemVar` 再回退到 `GlobalVarsAccessor`。
4. `SET_VAR` 语句由 `SetHintSystemVarWithOldState` 返回原始旧值，`AddHintSystemVarRestore` 以 `entry(...).or_insert` 保证同名变量只记录第一次的旧值。`BeginHintStatement` 清理当前 binding 标志，`MarkHintStatementFromBinding` 标记来源。
5. `FinishHintStatement` 先 `take` 全部恢复项，再逐个调用 hook；即使某一项失败也继续恢复剩余项，只返回第一个错误。最后推进 `PrevFoundInBinding` 并清除当前标志。直接运行时调用点位于 `pkg/session/runtime/planning.rs`。

### 事务、重试与规划临时状态

1. 首次分配 auto ID 时，`RetryInfo::AddAutoIncrementID` / `AddAutoRandomID` 记录序列；重试前 `ResetOffset` 使读取从头开始，`GetCurr*ID` 每次成功读取都推进游标。事务彻底结束时 `Clean` 同时清空序列和待删除 prepared statement。
2. `TransactionContext::AddUnchangedKeyForLock` 将二进制 key 复制进 map；同一 key 多次出现时，只要任一次要求排他锁，合并结果就保持排他。`Collect*` 分别筛选 S/X key，`Reset*` 清空。
3. `SetForUpdateTS` 只单调增加，`GetForUpdateTS` 返回它与 `StartTS` 的较大值。`UpdateDeltaForTable` 累加行数/字节增量，`FlushStmtPessimisticLockCache` 把语句级缓存移动到事务级缓存，`Cleanup` 清除不得跨事务的数据。
4. 备选计划轮通过 `SetAlternative*Override` 交换旧覆盖值，调用者应在作用域结束时写回旧值；读取方法优先覆盖、再回退会话或 `StmtCtx` 默认值。标量子查询和扩展列 ID 也提供 snapshot/restore，供规划分支回滚。

## 数据与状态

- 会话持久状态：连接 ID/能力、用户与角色、当前库、事务模式、autocommit、系统变量 map、prepared statement map、用户变量、慢日志规则及全局变量访问器。
- 语句状态：`StmtCtx`、计划缓存参数/值、warning/error count、table-cache 标记、前一 trace ID、开始时间、解析/编译/优化/等待 TS 耗时。
- 事务状态：`RetryInfo`、`TransactionContext`、`InTxn`、`Status`。`TxnCtxNeedToRestore` 的设计边界用于 savepoint 恢复，`TxnCtxNoNeedToRestore` 保存不随 savepoint 回滚的运行时事实。
- 规划状态：计划/列原子 ID、分区裁剪模式、隔离引擎集合、MPP 与 TiFlash 参数、代价因子、相关优化器变量/fix，以及备选轮覆盖。
- `UserVars` 的值目前是 `String`，而 Go 对照使用 `types.Datum`；类型仍单独保留为 `FieldType`。这是一项明确的表示差异，扩展表达式语义时不能假定 Rust 已覆盖全部 Datum 行为。
- `LazyStmtText::Update` 克隆参数，后续重置 `SessionVars.PlanCacheParams` 不会改变已捕获的日志内容。`String` 的缓存意味着字段改变必须经 `Update` 或 `SetText`，不能绕过失效协议。
- `DefaultGlobalVarAccessor` 是本文件内部的内存回退实现：未知 sysvar 返回 `VariableError::unknown`，TiDB table value 缺失返回 `InvalidValue`；真实运行时可通过 trait 边界提供其他访问器。

## 依赖与调用关系

上游直接证据如下：

- `pkg/session/runtime/session.rs` 持有 `Arc<session::SessionVars>`，形成会话对象到变量状态的所有权边。
- `pkg/session/runtime/planning.rs` 读取规划状态、开始 table-cache 语句生命周期并在语句末调用 `FinishHintStatement`。
- `pkg/session/runtime/control.rs` 和 `dispatch.rs` 调用 `SetSystemVar`，承接控制/协议层变量设置。
- `pkg/planner/optimize.rs` 使用 `SessionVars` 和 `RewritePhaseInfo`；`pkg/planner/core/expression_rewriter.rs` 注册标量子查询。
- `pkg/session/runtime/explain_query.rs` 通过 `WithScalarSubQueries` 借用真实注册项并向下转型，避免克隆线程亲和的 planner 上下文。
- `pkg/executor/adapter_slow_log.rs` 使用本类型汇总慢日志所需会话状态。

下游依赖如下：

- `stmtctx_dependency::StatementContext` 承载语句警告、LastInsertID、Explain 标志、table-cache 标志和若干表达式/引擎开关。
- `vardef` 提供变量名与默认值；`crate::GetSysVar`、`register_builtin_sysvars`、`SysVar::Validate`、`SetSessionFromHook` / `SetGlobalFromHook` 提供规范校验和副作用。
- `kv::StoreType` 是隔离读引擎集合的值类型；`parser_ast::FieldType` 与 `HintTable` 分别支撑用户变量类型和规划块别名；`parser_auth` 提供用户/角色身份。
- 标准库同步原语负责热路径并发状态，`chrono::FixedOffset` 表示会话时区，`crate::slow_log` 构造慢日志规则。

RustCodeGraph 的文件节点报告本文件被 8 个文件使用，并点名 `pkg/server/conn.rs`、`pkg/server/runtime.rs`、`pkg/session/runtime/dispatch.rs`、`planning.rs` 和 `scan_adapter_runtime_test.rs` 等。索引未为 `SetSystemVar`、`FinishHintStatement`、`RegisterScalarSubQ` 等本文件 Rust 方法返回独立 method 节点，因此这些方法的具体上游以仓库直接引用搜索补证，不把缺失图边解释为“无调用者”。

## 错误处理与边界

- 锁中毒策略并不统一：核心会话 `RwLock` / `Mutex` 多数用 `PoisonError::into_inner` 继续访问，`RetryInfo`、事务锁和 `PlannerSelectBlockNames` 则使用 `expect` / `unwrap`，中毒会 panic。新增字段应跟随所属状态族而不是随意混用。
- `SetSystemVar` 的主要校验错误来自规范 sysvar 注册表，并转换成 `String`；强类型字段的整数/浮点解析另行生成包含变量名和值的错误。`SetSystemVarWithoutValidation` 当前仍调用 `SetSystemVar`，名称不代表真的绕过校验。
- `SetSystemVarWithRelaxedValidation` 目前只对 `tidb_foreign_key_check_in_shared_lock` 走明确的 relaxed 路径，其余仍走普通设置。不能据函数名推断所有变量都被放宽。
- `FinishHintStatement` 保证尽力恢复全部变量，但只暴露第一个错误；调用方若忽略返回值可能丢失恢复失败信号。
- `validateReadConsistencyLevel` 只接受 strict/weak；`ToRuntimeFilterType` 不 trim 单项空白，输入如 `"IN, MIN_MAX"` 的第二项会失败。这是当前代码事实。
- `LazyStmtText` 的未知 redact 模式在 debug 构建触发 `debug_assert!`，非 debug 路径返回空串；`MARKER` 会把已有 `‹`/`›` 加倍后再包裹。
- `PlanCacheParamList::GetParamValue` 返回 `Option`，与 Go 直接按索引访问可能 panic 的行为不同。Rust 的 `GetExecuteDuration` 使用 `saturating_sub`，避免编译耗时大于已流逝时间时下溢。
- `TemporaryTableData` 用字符串错误和 byte slice 定义抽象边界，但本文件没有实现者或 IO；实现的原子性、持久性与容量限制必须由下游类型证明。

## 并发与资源生命周期

- `SessionVars` 的多数普通字段按“单会话、单线程推进”使用；需要跨任务读取的计数和 ID 使用 `AtomicU64` / `AtomicI32` / `AtomicI64`，当前库、用户变量、Hint 状态、计时与覆盖项使用锁。
- `scalar_subqueries`、`extended_col_unique_ids`、`rewrite_phase_info` 使用 `RefCell`，标量子查询元素使用 `Rc<dyn Any>`，本质上既非 `Send` 也非 `Sync`。文件以 `unsafe impl Send/Sync for SessionVars` 允许外层会话句柄跨任务移动，安全前提是这些 planner 字段保持会话线程亲和、绝不并发访问。任何新增调用点都必须维持此前提；锁住外层 `Arc` 并不会自动使 `Rc`/`RefCell` 线程安全。
- `WithScalarSubQueries` 将 `RefCell` 借用严格限制在回调期间，适合 EXPLAIN 向下转型；回调内不得重入注册/恢复，否则会触发动态借用 panic。snapshot/restore 会克隆 `Rc`，共享底层对象身份。
- `PlannerSelectBlockNames::Load` 克隆 `Arc` 快照，写者替换整个 vector，不修改读者正在观察的版本。
- `RetryInfo::ResetOffset` 可通过共享引用加锁修改游标；克隆 `RetryInfo` 会复制 ID 和当前偏移到新的 mutex，而不是共享 mutex。
- 进程级 adaptive replica read 使用 `SeqCst`；计划 ID 使用 `SeqCst`，StartTS 与 LastFoundRows 使用 Release/Acquire。相关优化器记录开关使用 Relaxed，因为集合本身另由 mutex 串行化。
- 事务 `Cleanup`、Hint `FinishHintStatement`、计划分支 snapshot/restore 和 `LazyStmtText::Update` 是明确的生命周期边界；绕过它们会留下跨语句/跨事务状态。

## 与 Go 版本的对应关系

- [`session.go`](./session.go) 的 `RetryInfo` / `retryInfoAutoIDs` 与 Rust 的同名结构具有相同的“记录、游标读取、重置、清空”流程。Rust 为共享 `ResetOffset` 加 mutex，并在 `Clone` 时复制锁内状态。
- Go `TransactionContext` 通过嵌入 `TxnCtxNeedToRestore` / `TxnCtxNoNeedToRestore` 区分 savepoint 边界；Rust 用两个具名字段表达相同分类。Rust 当前只移植部分缓存/savepoint 操作，不能把 Go 的完整方法集视为已存在。
- Go `UserVars` 用单个 `sync.RWMutex` 保护 Datum map 和类型 map；Rust 同样以一个 `RwLock<UserVarsState>` 保证两个 map 的一致快照，但值降为 `String`，`CloneVars` 返回具体 `UserVars`。
- Go `PlanCacheParamList::String` 隐藏 non-prepared 参数；Rust保持该隐私行为。Go 使用 `types.Datum` 的智能格式化，Rust当前直接拼接字符串。
- Go `LazyStmtText` 还支持可选 `Format` 回调，Rust当前没有此字段；两者均复制参数并惰性缓存脱敏结果。
- `SessionVars::AllocNewPlanID`、MPP 警告分流、总耗时/执行耗时、prepared ID、代价因子、字符串匹配选择率等方法直接对应 Go 同名实现。Rust在若干位置增加原子/锁保护和下溢防护。
- Go `SetSystemVar` / `SetSystemVarWithOldStateAsRet` 的全量副作用面更大；Rust `SetSystemVar` 只显式同步本文件列出的强类型热字段，其余值保留在规范变量容器和 `systems` map。扩展变量时必须逐项核对 Go setter/hook，不能只向 map 写值。
- Go `RegisterScalarSubQ` 把真实 planner 上下文存入 `[]any` 供 EXPLAIN 展开；Rust使用 `Vec<Rc<dyn Any>>` 并由 `expression_rewriter.rs` 注册、`explain_query.rs` 借用向下转型，保留对象身份与线程亲和性。
- Rust独立测试 [`session_test.rs`](./session_test.rs) 验证二进制锁 key 模式、warning/error 热字段、惰性文本脱敏和 retry 游标；[`session_hint_bridge_aster_unit_test.rs`](./session_hint_bridge_aster_unit_test.rs)、[`session_planner_ids_aster_unit_test.rs`](./session_planner_ids_aster_unit_test.rs)、[`scalar_subquery_registry_aster_unit_test.rs`](./scalar_subquery_registry_aster_unit_test.rs) 覆盖 Hint 恢复、并发 ID/选择率/MPP 和标量子查询生命周期。更宽的变量行为还在 [`tests/session_test.rs`](./tests/session_test.rs) 及 Go 测试中覆盖。

## 扩展指南

- 新增系统变量：先在规范 sysvar 定义/注册位置增加元数据和 hook，再判断是否需要在 `SessionVars` 增加强类型热字段及 `new` 默认值，最后在 `SetSystemVar` 同步规范值。必须对照 Go 同名变量的 setter、副作用与默认值；若可能含密码、token、凭据 URL 或敏感 SQL/配置，按本目录 `AGENTS.md` 设置 `IsSensitive` 并覆盖空值/非空值诊断脱敏测试。
- 新增事务状态：先决定字段是否应随 savepoint 恢复，分别放入 `TxnCtxNeedToRestore` 或 `TxnCtxNoNeedToRestore`；再明确 `Cleanup`、savepoint snapshot/restore 和语句缓存 flush 的边界。锁 key 必须继续使用原始字节，不能用有损字符串转换。
- 新增规划临时状态：优先提供成对的 snapshot/restore 或 exchange-old-value API，使备选轮即使提前返回也能恢复；若状态可能跨线程，不能放进现有 `Rc`/`RefCell` 区域，除非同时重新证明 `unsafe Send/Sync` 的安全性。
- 扩展 `SET_VAR`：维持“首次旧值”“尽力恢复全部项”“binding 标记推进”的不变量，并在 [`session_hint_bridge_aster_unit_test.rs`](./session_hint_bridge_aster_unit_test.rs) 增加独立回归测试。
- 扩展计划 ID、标量子查询、耗时或选择率：分别同步 [`session_planner_ids_aster_unit_test.rs`](./session_planner_ids_aster_unit_test.rs)、[`scalar_subquery_registry_aster_unit_test.rs`](./scalar_subquery_registry_aster_unit_test.rs)、[`slow_log_test.rs`](./slow_log_test.rs)；普通本文件状态行为放在 [`session_test.rs`](./session_test.rs)。测试逻辑保持在独立 `*_test.rs`，不要嵌入生产文件。
- 修改 Go 对齐行为时，应逐项记录 Go `session.go` 的对应增量；不要顺手补齐 Go 文件中与该增量无关的完整子系统。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/sessionctx/variable/session.rs` 分段读取 1–1748 行；符号清单以该文件的结构、trait、函数和 impl 为准。
- 图索引：`rustcodegraph status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；文件节点给出 8 个使用文件。`query SessionVars --kind struct` 命中本文件 631 行，同时显示运行时/规划器相邻类型；对 `SetSystemVar`、`FinishHintStatement`、`RegisterScalarSubQ` 的 Rust method 查询未产生本文件节点，因此使用直接引用搜索补齐调用证据。
- crate 与入口：已读 [`Cargo.toml`](./Cargo.toml) 和 [`lib.rs`](./lib.rs)，确认 crate 名、直接依赖、`pub mod session` 及测试模块装配。
- Go 对照：已读 [`session.go`](./session.go) 中 RetryInfo、TransactionContext、UserVars、SessionVars、计划参数、惰性文本、系统变量、规划 ID、MPP、标量子查询和选择率相关定义；差异均按当前 Rust 实现表述。
- 测试证据：已读 [`session_test.rs`](./session_test.rs)、[`session_hint_bridge_aster_unit_test.rs`](./session_hint_bridge_aster_unit_test.rs)、[`session_planner_ids_aster_unit_test.rs`](./session_planner_ids_aster_unit_test.rs) 与 [`scalar_subquery_registry_aster_unit_test.rs`](./scalar_subquery_registry_aster_unit_test.rs)，并检查更宽测试入口 [`tests/session_test.rs`](./tests/session_test.rs) 的直接引用。
- 上游直接引用：已检查 `pkg/session/runtime/{session,planning,control,dispatch,explain_query}.rs`、`pkg/planner/{optimize,core/expression_rewriter}.rs` 和 `pkg/executor/adapter_slow_log.rs` 中对本模块符号的引用。
- 本任务为纯文档分析，按总计划不运行 Cargo。交付结构以任务规定的 11 个固定二级标题命令验证；结论另经人工复核，确保能回答文件存在原因、主要运行流程和安全扩展边界。

# `pkg/session/runtime/typed_adapter_bridge.rs`

## 文件定位

本文件位于 `astersql-session` crate 的运行时层，是规范会话 `ConcreteSession` 与 `astersql-executor` 类型化执行适配器之间的桥梁。`pkg/session/runtime.rs` 以私有模块 `mod typed_adapter_bridge` 装配它，只向 crate 的其他使用者重导出 `OwnedKVSnapshotSource`、`SessionBoundAdapterOwner` 和 `TypedScanSpec`；其余绑定结构、RU 终结作用域和辅助函数保持在 `runtime` 模块内部。

它不是通用 SQL 入口，也不独立负责优化。上游在 `pkg/session/runtime/planning.rs`、`explain_analyze.rs`、`dispatch.rs` 和 `load_data.rs` 中完成语句识别或计划生成，再由本文件把真实物理计划、固定 MVCC 版本、会话状态和执行终结生命周期交给 executor。实际 `AdapterRuntime` trait 实现在相邻的 `pkg/session/runtime/scan_adapter_runtime.rs`，因此本文件与该实现共同构成完整适配层。

`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`、库入口为 `lib.rs`，并直接依赖这里使用的 `astersql-executor`、`astersql-kv`、`astersql-domain`、planner core/base/physicalop、`astersql-store-driver`、`astersql-resourcegroup-runaway`、`astersql-tablecodec`、`astersql-util-codec` 等本地 crate。目标包下未找到 `doc.go`；crate 的 Go 对照包由 `[package.metadata.porting].go-package = "pkg/session"` 明确指定。

## 核心职责

1. `SessionBoundAdapterOwner` 聚合单条语句执行所需的规范会话、类型化扫描/物理计划、prepared 参数、DML/ANALYZE SQL、锁与 mutation stage、超时、Top SQL、runaway checker、RU 证据和 point-get 缓存。executor 的 `ExecStmt.Ctx` 持有其 `Arc`，从而通过同一会话状态实现执行回调。
2. `BindTypedPhysicalPlan`、`BindPreparedPlannedKVSelect` 和 `bind_physical_plan` 将优化后的物理树转换成 executor 可消费的扫描绑定；prepared 路径还从实际计划重建表/索引 key range，并保存重建参数。
3. `BuildExecStmt`/`BuildPreparedExecStmt` 从真实物理树克隆 `TypedPlan`，为 `EXECUTE` 包裹 `RuntimeExecute`，安装 statement RU owner，避免仅从文本 `PlanInfo` 推测计划行为。
4. `OwnedKVSnapshotSource` 只持有 `Arc<Domain>`、固定 `kv::Version` 和可选读统计，使快照 getter/iterator 脱离线程本地 `ConcreteSession` 后仍能存活。
5. `PendingStatementRU`、`SessionStatementRUScope` 和 `FileTransferStatementRUScope` 保证成功、失败、嵌套语句和延迟文件传输路径最终发布 RU 结果并关闭 result set。
6. `ExecutionDeadline` 用专用线程等待剩余执行时间；超时后向规范 `SQLKiller` 发送 `MaxExecTimeExceeded`，作用域结束则唤醒并回收线程。
7. `execute_do_terminal` 复用真实逻辑/物理计划构建与 `ExecStmt` 执行链处理无结果 `DO`，并把终结工作交给 RU scope，而不是建立一条摘要式旁路。

## 主要符号

- `OwnedKVSnapshotSource { domain, version, read_stats }`：实现 `kv::Getter` 与 `kv::Retriever`。`Get` 为每次读创建固定版本快照并可安装 `CollectRuntimeStats`；`Iter`/`IterReverse` 从同一版本建立迭代器。
- `SessionBoundAdapterOwner`：公开适配器所有者。`new` 初始化所有语句局部状态；`Drop` 若仍持有 `top_sql_stats`，调用 `SetFinished` 防止 Top SQL 生命周期悬空。
- `TypedScanSpec`：最低层扫描规格，包含表 ID、主键句柄标志、顺序、列、编码范围、MVCC 版本和 chunk 容量。
- `PhysicalScanBinding`：保存 `BoundPhysicalPlan`、根扫描范围、按叶节点分组的 `leaf_ranges`、版本和 chunk 配置。
- `PreparedBinding`：保存 statement ID、实参和 chunk 配置，供 `AdapterRuntime::RebuildPlan` 重新走规范 prepared 优化/缓存路径。
- `BoundPhysicalPlan::{Plain, Prepared}`：统一普通物理树与 `PreparedKVPhysicalPlan`；`as_plan` 暴露真实 `PhysicalPlan`，`is_prepared` 标识缓存相关路径。
- `collect_table_scans`、`find_index_lookup`、`find_index_reader`、`find_index_scan`：按 TableReader/IndexLookupReader 的特殊子树布局递归定位扫描叶节点。
- `encode_index_ranges`：调用 `RebuildRangesForPlanCache`，按开闭端点用 `EncodeKey`、`PrefixNext` 和 `EncodeIndexSeekKey` 生成索引 KV 范围。
- `encode_table_record_ranges`：仅接受非 common handle 的整数主键范围，验证单列上下界和 `KindInt64` 后用 `EncodeRowKeyWithHandle` 编码。
- `BuildPreparedExecStmt`：从 bound prepared 计划生成 schema、输出名和 plan summary，再委托 `BuildExecStmt`。
- `BuildExecStmt`：克隆真实物理树，构造 `ExecStmt` 和 `StatementContext`，并调用 `install_statement_ru_owner`。
- `BindDMLStatement`/`BindAnalyzeStatement`：按当前 SQL mode 重新解析并严格验证单条语句类型，只接受 INSERT/UPDATE/DELETE 或 ANALYZE TABLE。
- `BindPreparedPlannedKVSelect`：经 `ConcreteSession::PlanPreparedPlannedKVSelect` 获取规范计划和缓存状态，重建范围、固定当前版本、绑定计划并记录重建参数。
- `ConcreteSession::OpenTypedPhysicalPlan*`/`OpenTypedKVSnapshotScan`：建立 owned snapshot source，并调用 executor builder 或直接构造 `TypedKVScan`。
- `PendingStatementRU::finish`：先记录成功终态，再关闭 result set；无 result set 时直接调用 `FinishExecuteStmt`。
- `SessionStatementRUScope::{new, finish, fail}`：保存嵌套 scope 的先前 pending；成功时正常终结或将文件传输语句延迟，失败时记录失败并带错误关闭。
- `FileTransferStatementRUScope`：消费延迟 pending，完成文件传输后的最终终结；`Drop` 恢复嵌套前状态。
- `is_constant_terminal_plan`：只认可 `PhysicalTableDual`，或单子节点且最终落到 dual 的 `PhysicalProjection`。
- `execute_do_terminal`：构建并优化 DO 投影，设置 `CalculateNoDelay`，执行后要求没有 result set，再保存 pending RU。

## 执行流程

### 普通或 prepared SELECT

1. 上游构造 `SessionBoundAdapterOwner`。普通计划调用 `BindTypedPhysicalPlan`；prepared 路径由 `planning.rs` 或 `explain_analyze.rs` 调用 `BindPreparedPlannedKVSelect`。
2. `bind_physical_plan` 识别 `PointGetPlan`、常量终结计划、TableReader/IndexReader/IndexLookup 等形态。PointGet 建立空 range 的 `TypedScanSpec`；常量计划不要求扫描；其余计划至少需要表扫描或 IndexReader。
3. 单扫描绑定调用者给出的编码范围；多扫描只接受无 `AccessCondition` 的规范全表 record scan，并为每个物理表生成完整 record prefix 范围。这个限制避免把一个范围错误复用于多个叶节点。
4. prepared 绑定先用当前参数重新规划。PointGet 不预编码范围；索引读取用 `encode_index_ranges`；单表全扫生成 table record prefix；单表条件扫描用整数 handle 编码；多表扫描把具体叶范围留给 `bind_physical_plan`。
5. `BuildExecStmt` 克隆绑定树；`StatementKind::Execute` 额外包成 `RuntimeExecute`，然后安装 RU owner。`scan_adapter_runtime.rs::AdapterRuntime::BuildExecutor` 再选择 canonical table reader、multi-binding builder、point-get 或普通 typed builder。
6. executor 的 `Open`/`Next` 才真正读取 KV；`OwnedKVSnapshotSource` 每次从 Domain 的 storage registry 获取固定版本快照。可 detachable executor 因不持有 `Rc<ConcreteSession>`，能在原会话释放后继续读。
7. prepared executor 成功结束时，`AdapterRuntime::FinalizePreparedExecution` 调用 `FinishPreparedKVPhysicalPlan` 允许缓存准入；失败时丢弃 `PendingCache`。`RebuildPlan` 使用 `PreparedBinding` 的原 statement ID/参数再次调用本文件的 prepared 绑定逻辑。

### 语句终结与文件传输

1. `dispatch.rs` 在每条解析后语句前建立 `SessionStatementRUScope`，因此嵌套语句会暂存外层 pending，并在 Drop 时恢复。
2. executor 创建后，上游把 `ExecStmt`、可选 `RecordSet` 和 failure guard 放入 `statement_ru_pending`。
3. 正常 `finish` 取出 pending。一般路径调用 `PendingStatementRU::finish`；若没有 record set 且会话仍持有 file-transfer reader，则移入 `statement_ru_delayed`。
4. 执行阶段返回 `SessionError` 时，`fail` 记录失败终态，并调用 `CloseWithError` 或 `FinishExecuteStmt(..., Some(error), false)`。
5. `load_data.rs::execute_with_load_data_reader` 建立 `FileTransferStatementRUScope`；文件读取和语句处理完成后才消费延迟 pending。任何未显式消费的 pending 会在作用域 Drop 时被丢弃并由 guard 保持失败语义。

### DO 语句

`dispatch.rs` 遇到 `DoStmt` 时调用 `execute_do_terminal`。该函数解析 SQL、用 `NewPlanBuilder` 构建逻辑计划、执行 `DoOptimize`，把根投影标记为 `CalculateNoDelay`，固定当前 storage 版本，绑定/编译/执行真实 typed plan。DO 若意外返回 result set 会报错；正常无结果则保存为 `PendingStatementRU`，等待外层 RU scope 完成。

## 数据与状态

- 会话所有权被刻意拆分：`SessionBoundAdapterOwner.session` 是 `Rc<ConcreteSession>`，只能留在所属 worker；`OwnedKVSnapshotSource` 使用 `Arc<Domain>` 且测试证明 `Send + Sync`，允许 detached scan 跨越会话生命周期。
- `scan` 与 `physical_scan` 是互相关联的当前绑定。`BindTypedScan` 会清空旧物理绑定和 prepared rebuild 信息；`BindTypedPhysicalPlan` 也先清空 prepared binding，防止普通计划误走 prepared 重建。
- `PhysicalScanBinding.version` 固定所有叶读取的 MVCC 版本；`OpenTypedPhysicalPlanWithBindings` 为每个叶建立同版本 source。版本不会在迭代过程中重新获取。
- `prepared_binding` 保存重建所需参数，但真正缓存状态和 warning/snapshot 位于 `PreparedKVPhysicalPlan`；`PreparedResultMetadata` 只对 `BoundPhysicalPlan::Prepared` 返回元数据。
- `effects` 是可克隆的观察记录，保存 process SQL/开始时间、found/scanned rows、审计、慢查询、摘要、错误及 Top SQL 计数；具体写入由 `scan_adapter_runtime.rs` 的 trait 实现完成。
- `statement_locks_before` 与 `statement_mutation_stage` 保存语句开始前的锁/写缓冲/冲突/DML report/FK cascade 快照，使失败重试只回滚本语句新增状态。
- `point_read_stats`、`table_reader_ru_evidence` 使用 `Arc<Mutex<...>>`，供读取执行器与终结统计共享；`point_cache`、绝大多数语句状态用 `RefCell`/`Cell`，表明 owner 本身是线程局部对象。
- `statement_ru_pending`、`statement_ru_delayed` 和 `statement_ru_scope_depth` 实际存放于 `ConcreteSessionInner`（见 `runtime/session.rs`），scope 只持有同一 inner 的 `Rc`。

## 依赖与调用关系

上游主要调用边：

- `runtime/planning.rs::ExecutePreparedPlannedKVSelectThroughAdapter` 创建 owner，调用 `BindPreparedPlannedKVSelect` 和 `BuildPreparedExecStmt`，并保存 `PendingStatementRU`。
- `runtime/explain_analyze.rs` 对 prepared EXPLAIN ANALYZE 走同一绑定/ExecStmt 链。
- `runtime/dispatch.rs` 为逐语句执行创建 `SessionStatementRUScope`，并由 DO 分支调用 `execute_do_terminal`。
- `runtime/load_data.rs::execute_with_load_data_reader` 创建 `FileTransferStatementRUScope`。
- `runtime/scan_adapter_runtime.rs` 为 `SessionBoundAdapterOwner` 实现 `AdapterRuntime`，读取本文件绑定，调用 `collect_table_scans`/`find_index_lookup`/`is_constant_terminal_plan`，并在 `RebuildPlan` 中再次调用 `BindPreparedPlannedKVSelect`。
- `runtime/session.rs::ConcreteSessionInner` 保存本文件定义的 pending RU 类型。

下游关键调用边由 RustCodeGraph 和源码共同确认：`BindPreparedPlannedKVSelect -> PlanPreparedPlannedKVSelect / collect_table_scans / find_index_lookup / find_index_reader / encode_index_ranges / encode_table_record_ranges / bind_physical_plan`；`execute_do_terminal -> AdapterPlanContext / BindTypedPhysicalPlan / BuildExecStmt / CurrentVersion`；`BuildExecStmt -> PhysicalPlan::clone_physical / RuntimeExecute::New / install_statement_ru_owner`。执行器打开路径进一步调用 `BuildTypedPhysicalPlan`、`BuildTypedPhysicalPlanWithBindings`、`BuildTypedPhysicalSelectLockPlan` 或 `TypedKVScan::new`。

存储侧依赖通过 `Domain::storage().with_storage(...)` 访问，不把 storage 的临时借用泄漏给 executor。编码依赖 `astersql-tablecodec` 和 `astersql-util-codec`；错误边界统一转换为 `BuildError`、`SessionError` 或 `astersql_errors::SharedError`。

## 错误处理与边界

- DML/ANALYZE 绑定必须恰有一条且类型匹配；解析沿用会话 SQL mode，错误原样包装为 `SessionError`。
- prepared index lookup 必须恰有一个 table scan 和可定位的 `PhysicalIndexScan`；缺失 `IndexInfo`/`TableInfo`、非法 partition index、没有可执行扫描都会返回带上下文的 `BuildError`。
- record range 编码只支持单列、有符号整数主键、`PKIsHandle` 且非 common handle。该限制是显式边界，不代表 common handle 或复合主键已被支持。
- 多扫描当前只支持每个叶均无 access condition 的全 record scan；否则返回 `typed multi-scan binding requires canonical full record scans`。
- `BuildPreparedExecStmt` 要求已经绑定 prepared physical plan；`BuildExecStmt` 要求已有任意 physical binding。缺失或类型不符立即失败，不生成降级 executor。
- `OwnedKVSnapshotSource` 将 storage/snapshot 的读取错误直接传播；`Get` 安装统计选项只影响单 key getter，`Iter`/`IterReverse` 在本文件内没有安装该选项。
- `ExecutionDeadline` 对 poisoned mutex 使用 `into_inner` 继续清理；Drop 唤醒并 join worker。join 错误被忽略，因为清理路径不能覆盖原语句结果。
- `PendingStatementRU::finish` 先发布成功再关闭 record set；关闭失败会转换为带 `close session result` 上下文的 `SessionError`。失败路径用 `CloseWithError`，但忽略二次关闭错误以保留原始会话错误。
- `execute_do_terminal` 假定 parser 返回至少一条语句（调用者已识别 DO）；若 executor 返回 result set，则以内部契约错误终止。

## 并发与资源生命周期

`SessionBoundAdapterOwner` 含 `Rc`、`RefCell` 与 `Cell`，设计为规范会话 worker 上的非 `Send` 所有者。它的 Drop 兜底结束 Top SQL stats。与之相反，`OwnedKVSnapshotSource` 只持有线程安全的 `Arc` 状态；独立测试 `detached_snapshot_source_has_an_independent_thread_safe_owner` 以编译期约束确认其 `Send + Sync`，`canonical_session_typed_scan_is_lazy_and_detach_survives_session_drop` 证明 detached executor 在 session drop 后仍按固定版本继续读。

`ExecutionDeadline::new` 从 statement 原始开始时间扣除已耗时，因此 executor 构建前的时间也计入上限。worker 在 condvar 上等待取消或超时；`CancelMaximumExecutionTime` 通过丢弃 deadline 触发 Drop、通知并 join，避免迟到的 kill signal 污染复用会话。独立测试覆盖构建前已超时、主动取消和 result set 空闲时仍触发 kill。

RU scope 采用 RAII 恢复嵌套状态：构造时取走先前 pending 并增加 depth，Drop 丢弃本层未消费 pending、恢复先前值并减少 depth。普通 pending 和文件传输 delayed pending 使用两个槽，防止一个延迟终结覆盖另一个语句。failure guard 随 `PendingStatementRU` 存活，只有明确成功/失败终结后才被消费。

快照 source 每次操作临时进入 storage registry；迭代器由 storage snapshot 返回并继续拥有后端所需资源。物理执行器、record set 和 deadline 都依赖显式 `Close`/scope Drop 终结，新增早退路径必须维持这些 RAII 边界。

## 与 Go 版本的对应关系

Rust 文件没有一一对应的 `pkg/session/typed_adapter_bridge.go`；其职责是把 Go `pkg/session/session.go` 中分散在 session、executor、record-set 生命周期里的语义集中成类型化桥接层。`Cargo.toml` 的 porting metadata 明确 Go 包为 `pkg/session`。

- Go `executeStmtImpl` 在最早 defer 中记录 panic/错误的 RU 失败，并让 point-get 或文件传输的正常返回发布成功；Rust 用 `StatementRUFailureGuard`、`PendingStatementRU` 和两个 scope 保持相同“必须有终态”的意图。
- Go 的 `runStmt` 在有 result set 时把终结交给 `execStmtResult.Finish/Close`，无 result set 时调用 `FinishExecuteStmt`；Rust `PendingStatementRU::finish` 同样区分 `record_set: Some` 与 `None`。
- Go 对 file transfer 把 `ExecStmt` 存入 session，等连接侧处理完再 `FinishExecuteStmt`；Rust 把无 result set 的 pending 从 `statement_ru_pending` 移到 `statement_ru_delayed`，由 `FileTransferStatementRUScope` 消费。
- Go session 关闭时对 `stmtStats` 调用 `SetFinished`；Rust owner Drop 对 `top_sql_stats` 做对应兜底。
- prepared plan cache 的实际规划、参数绑定和缓存准入仍由规范 session/planner 实现；本文件不复制简化算法，而是通过 `PlanPreparedPlannedKVSelect`、`FinishPreparedKVPhysicalPlan` 和真实物理树接回该链。
- 最大执行时间沿用 Go 的 session `SQLKiller`/`MaxExecTimeExceeded` 语义；Rust 独立 worker 弥补 result set 空闲、不调用 `Next` 时仍需及时 kill 的生命周期要求。

Go 与 Rust 的结构并不完全相同：Rust 明确拆出 `OwnedKVSnapshotSource` 以支持 detachable typed executor，并用 `Rc`/`Arc` 的类型边界表达线程局部会话与线程安全快照的差别。文档不据此声称整个 Go executor 已逐字段移植；可确认的是上述 prepared、RU、kill、快照与终结语义有源码和测试证据。

## 扩展指南

- 新增物理 reader/scan 形态时，先扩展 `collect_table_scans`/`find_*` 与 `bind_physical_plan` 的树识别，再决定单一 `ranges` 是否足够或必须新增 leaf binding。不要通过空范围悄然降级；为缺失 metadata、分区和多叶情况补独立测试。
- 新增 handle 类型或复合范围支持应集中修改 `encode_table_record_ranges`，并与 planner `RebuildRangesForPlanCache` 的 datum/开闭区间规则逐项对齐。索引编码变化则修改 `encode_index_ranges`，重点验证 prefix-next、物理分区表 ID 和 unique/non-unique 编码。
- 新增 prepared 行为必须同时维护 `PreparedBinding` 的可重建信息、`BuildPreparedExecStmt` 的 schema/output names、`AdapterRuntime::RebuildPlan` 和缓存成功/失败终结；优先扩展 `pkg/session/plan_cache_runtime_test.rs` 与 `runtime_test/typed_adapter_bridge.rs`。
- 增加新的语句种类时，不应放宽 `BindDMLStatement`/`BindAnalyzeStatement` 的现有契约；应建立明确的新 binding，并在 `scan_adapter_runtime.rs::BuildExecutor` 选择对应 executor。
- 修改快照资源模型时保持 `OwnedKVSnapshotSource: Send + Sync` 和固定版本不变量；同步验证 lazy open、detach 后 session drop、正向/反向迭代及 read stats。测试必须放在独立测试文件，不能内嵌到本源文件。
- 修改 RU 生命周期时同时审查 `dispatch.rs`、`load_data.rs`、`planning.rs`、`explain_analyze.rs` 和 Go `session.go` 的终结分支，覆盖成功、编译后错误、result set 关闭错误、panic、嵌套语句和文件传输延迟。
- 修改 deadline 时验证“从原始 started 计时”“取消后不污染复用会话”“无 Next 也超时”三项；注意 join 可能阻塞当前 worker，不能引入持锁等待。
- 性能风险集中在每次 getter 新建 snapshot、物理树 clone、prepared range 重建和多叶 source 创建；正确性风险集中在错误 range、错用 MVCC 版本、遗漏 RU 终态与 session/快照所有权混淆。

## 验证依据

- 目标源码：`pkg/session/runtime/typed_adapter_bridge.rs`（1354 行）；逐段核对了所有模块级结构、枚举、函数、impl、`#[cfg(test)]` hook 和 Drop 实现。
- 模块/依赖：`pkg/session/runtime.rs` 的模块声明与重导出；`pkg/session/Cargo.toml` 的 crate 入口、porting metadata 和直接依赖。`pkg/session` 下未发现 `doc.go`。
- RustCodeGraph：`status` 显示索引包含 11467 文件、307296 节点、1848419 边；文件节点显示目标被 `runtime.rs`、`dispatch.rs`、`planning.rs`、`scan_adapter_runtime.rs` 等 8 个文件使用。精确查询确认 `BindPreparedPlannedKVSelect` 的下游包括 `collect_table_scans`、`find_index_lookup`、`find_index_reader`、`encode_index_ranges`、`encode_table_record_ranges` 和 `bind_physical_plan`；`execute_do_terminal` 的下游包括 `AdapterPlanContext`、`BindTypedPhysicalPlan`、`BuildExecStmt` 和 `CurrentVersion`。图对 impl 方法的部分 callers 为空，因此用下列源码引用补齐反向边。
- 上游/相邻实现：`pkg/session/runtime/planning.rs`、`explain_analyze.rs`、`dispatch.rs`、`load_data.rs`、`scan_adapter_runtime.rs`、`session.rs`。
- 独立 Rust 测试：`pkg/session/runtime_test/typed_adapter_bridge.rs` 覆盖 thread-safe snapshot owner、规范数据库/SQLKiller、行锁与失败回滚、deadline、lazy detach、物理 Limit、prepared ExecStmt、FOR UPDATE 和终结；`pkg/session/plan_cache_runtime_test.rs` 覆盖 covering/double-read index、point-get、range rebuild、缓存准入与内存仲裁；`pkg/session/runtime/scan_adapter_runtime_test.rs` 覆盖 DML/锁、RU 证据、prepared/explain/execute/file-transfer 终态和 Top RU。
- Go 对照：`pkg/session/session.go` 的 `executeStmtImpl`、`runStmt`、`execStmtResult::Finish`、session close；另参考 `pkg/session/session_test.go` 与 `pkg/session/test/variable/variable_test.go` 的 SQLKiller/最大执行时间行为。
- 未运行 Cargo 或代码测试：任务明确为纯文档分析。交付验证只执行任务指定的 11 章节结构命令，并人工复核本文件能回答存在原因、运行路径和安全扩展点。

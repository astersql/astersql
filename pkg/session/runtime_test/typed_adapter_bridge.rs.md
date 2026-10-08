# `pkg/session/runtime_test/typed_adapter_bridge.rs`

## 文件定位

本文件是 `astersql-session` crate 的测试模块，不是生产运行时实现。`pkg/session/lib.rs` 仅在 `#[cfg(test)]` 下装入 `runtime_test.rs`，后者再通过 `#[path = "runtime_test/typed_adapter_bridge.rs"] mod typed_adapter_bridge;` 引入本文件。文件中的 14 个入口均带 `#[test]`；唯一的辅助类型 `PreparedBridgeSchemaLoader` 也只用于构造测试域。因此，它虽然位于本计划的逐文件清单中，实际职责是跨模块回归验证，而不是向应用暴露 API。

被验证的生产边界主要位于 `pkg/session/runtime/typed_adapter_bridge.rs` 和 `pkg/session/runtime/scan_adapter_runtime.rs`：前者实现 `SessionBoundAdapterOwner`、`OwnedKVSnapshotSource` 等会话到执行器的适配状态，后者负责把物理计划、扫描范围和 `ExecStmt` 接到执行器。`pkg/session/runtime.rs` 只公开其中的 `OwnedKVSnapshotSource`、`SessionBoundAdapterOwner` 和 `TypedScanSpec`。

## 核心职责

本文件以真实 `ConcreteSession`、内存 KV、物理计划和 `astersql_executor::adapter::ExecStmt` 组合出端到端测试，覆盖以下契约：

- SQL PREPARE/EXECUTE 的参数必须重新绑定为 typed point/range scan，并能被 EXPLAIN ANALYZE 观察。
- `SessionBoundAdapterOwner` 必须复用真实会话的数据库、SQL killer、事务和行锁，而不是维护第二套脱离会话的状态。
- typed KV 扫描应延迟读取、支持按 chunk 流式消费；可分离的快照执行器必须拥有独立且 `Send + Sync` 的快照所有权。
- 参数化 `LIMIT`/`OFFSET`、prepared plan cache、物理树重建和 `RuntimeExecute` 解包必须协同工作。
- `SELECT ... FOR UPDATE` 必须先锁定扫描得到的记录再返回结果，失败语句只能撤销本语句新加的锁。
- 查询截止时间、审计插件、慢查询、Top SQL、statement summary 与 found rows 等执行副作用必须从生产适配路径触发。

这些断言是 Rust 移植对 Go `ExecStmt` 生命周期语义的回归护栏；文件本身不实现上述能力。

## 主要符号

- `PreparedBridgeSchemaLoader(SchemaRef)`：测试专用 schema loader。`load_info_schema` 返回固定版本 `10`，`load_snapshot_info_schema` 保留调用方给出的时间戳，`keyspace_exists` 恒为真；它用于让 prepared-plan 测试拥有可预测的 `Domain`/InfoSchema。
- `sql_prepared_primary_key_select_uses_typed_ranges_for_execute_and_explain`：从 SQL 文本入口验证主键参数重绑定、多列别名和 EXPLAIN ANALYZE。
- `detached_snapshot_source_has_an_independent_thread_safe_owner`：用编译期泛型约束证明 `OwnedKVSnapshotSource: Send + Sync`。
- `session_bound_adapter_owner_uses_real_session_database_and_killer`：验证 owner 读取真实当前库，并传播同一个 `SQLKiller` 的中断信号。
- `session_bound_adapter_locks_encoded_record_keys_in_canonical_transaction` 与 `failed_pessimistic_select_releases_only_its_new_statement_locks`：验证事务级锁复用、跨事务冲突、owner 释放，以及失败语句锁回滚边界。
- 三个 `session_bound_adapter_*deadline*` 测试：验证 build 前已消耗的时间也计入 `max_execution_time`、完成后可取消 deadline、没有 `Next` 轮询时后台 deadline 仍会写入 kill signal。
- `session_bound_adapter_audit_calls_ready_plugin_with_canonical_session_view`：注册真实审计插件，核对事件类型、SQL、命令与绑定表元数据。
- `canonical_session_typed_scan_is_lazy_and_detach_survives_session_drop`：验证 `Open` 不读取 KV、分页读取，以及 detach 后跨 session 生命周期继续读取固定版本。
- `session_bound_adapter_accepts_a_canonical_physical_limit_root`：把 `PhysicalLimit -> PhysicalTableScan` 绑定给生产 adapter，核对 LIMIT 截断和 EOF。
- `canonical_prepared_exec_stmt_binds_limit_and_streams_rows_then_restores_plan_cache`：本文件最大场景，覆盖参数校验、prepared 物理树、重建、缓存恢复、OFFSET 下推、SQL dispatch、EXPLAIN ANALYZE 和 detach。
- `canonical_pessimistic_select_for_update_locks_scanned_record_before_returning_rows`：直接构造 `ExecStmt`，证明锁在结果返回前已被持有。
- `canonical_session_exec_stmt_streams_typed_rows_and_finishes_statement`：覆盖普通 typed scan 的字段、流式分页、fetch 回调、finish 副作用、非法 prepared/locking 模式和独立 detach。

除 `PreparedBridgeSchemaLoader` 的 trait 方法外，本文件没有面向生产的公开 API、模块级常量、trait 定义或条件编译项；条件编译发生在父模块 `pkg/session/lib.rs`。

## 执行流程

典型测试链路如下：

1. 通过 `concrete_session()` 或 `CreateAnalyzeSession()` 建立真实 session/domain；需要自定义表元数据时，以 `PreparedBridgeSchemaLoader` 创建 mock `Domain`。
2. 通过 SQL 或底层事务写入编码记录，随后以 `CurrentVersion("global")` 固定 MVCC 读取版本；记录键由 `strict_t_multi_row` 和 `GenTableRecordPrefix` 生成。
3. 通过 SQL PREPARE，或构造 `PhysicalTableScan`、`PhysicalLimit`、`PlanInfo` 与 `StatementNode`，将物理计划和 `KeyRange` 绑定到 `SessionBoundAdapterOwner`。
4. 经 `BuildExecutor`、`BuildExecStmt` 或完整 SQL dispatch 进入生产执行路径，调用 `Open`/`Next`/`Close` 消费 chunk；prepared 场景还执行 `RebuildPlan` 和 plan-cache 再绑定。
5. 对锁、killer、缓存命中、字段、行、EXPLAIN runtime stats、审计与 statement finish 副作用作断言。
6. detach 场景先取得独立 result set，再主动丢弃原 result、statement、owner、session/domain，最后继续读取，以证明资源所有权真正分离。

参数化 LIMIT 场景还有两个重要不变量：负整数和非整数参数必须报 `Incorrect arguments to LIMIT`；对于 `LIMIT offset, count`，根 LIMIT 保存 `(offset, count)`，下推到扫描侧的 LIMIT 至少读取 `offset + count` 行。本测试以 count 为 1 核对 `(offset, 1)` 与 `(0, offset + 1)` 同时存在。

## 数据与状态

测试数据使用表 ID `88` 和手工编码的整型主键记录，列类型主要为 `LONG LONG` 与 `VARCHAR`。`KeyRange { start, end }` 覆盖整张表的 record prefix；`version` 固定快照时间，`initial_capacity` 和 `maximum_chunk_size` 多设为 1，以强制跨页、EOF 和 detach 后续读路径可观察。

关键状态由生产对象持有而非测试镜像：

- `ConcreteSession`/`SessionBoundAdapterOwner` 持有当前数据库、SQL killer、事务与语句生命周期状态。
- `ExecStmt` 持有 `PlanInfo`/typed physical plan、`StatementNode`、`StatementContext`、prepared 与 select-for-update 标志，以及各阶段耗时槽位。
- prepared plan cache 由 session 维护；测试用 `LastPlanFromCache`、`TypedFlatPlan` 和 `ClassifiedTypedPlan` 观察首次规划、重建和缓存恢复。
- 行锁区分事务既有锁与当前语句新锁；`OnPessimisticStmtStart/End(false)` 只能释放后者。
- `Effects`、domain slow-query 列表和插件收集向量用于观察 audit、Top SQL、summary、slow query 与 found rows。

测试辅助 loader 中的 InfoSchema 用 `Arc` 共享；返回 `LoadedInfoSchema` 时克隆 `Arc`，避免复制 schema 数据。

## 依赖与调用关系

crate 边界由 `pkg/session/Cargo.toml` 确认：包名为 `astersql-session`，`[lib] path = "lib.rs"`；本文件使用的 `astersql-domain`、`astersql-executor`、`astersql-infoschema`、`astersql-kv`、`astersql-meta-model`、planner core/base/physicalop、`astersql-plugin`、`astersql-tablecodec` 与 `astersql-types` 均是该 crate 的直接路径依赖。文件没有专属 feature gate；父模块的 `#[cfg(test)]` 决定其只进入测试构建。

上游是 Rust 测试 harness：`pkg/session/lib.rs` → `pkg/session/runtime_test.rs` → 本模块 → 各 `#[test]`。不存在生产调用者。

下游主链可概括为：测试场景 → `ConcreteSession`/`SessionBoundAdapterOwner` → `BindPhysicalTableScan`、`BindTypedPhysicalPlan` 或 `BindPreparedPlannedKVSelect` → `ExecStmt::Exec`/`BuildExecutor` → typed executor 的 `Open`、`Next`、`Close`。存储侧经 `Domain::storage`、`Storage::Begin/CurrentVersion`、事务 `Set/Commit` 和 `OwnedKVSnapshotSource`；规划侧经 `PhysicalTableScan`、`PhysicalLimit`、`RuntimeExecute`；可观测性侧经 SQL killer、plugin audit、slow query、Top SQL 和 statement summary。

RustCodeGraph 对关键测试的 callees 分析确认了 chunk/result-set 的 `NewChunk`、`Next`、`Close`、`TryDetach`、`OnFetchReturned` 调用，以及 `StatementContext`、`StatementNode`、`PlanInfo`、`SchemaColumn` 的构造。由于常见方法名存在大量跨 crate 同名节点，图对 `CurrentVersion`、`Key` 等边发生了歧义；这些边以目标源码中的限定类型和相邻生产文件复核，不把错误候选当成结论。

## 错误处理与边界

本文件大量使用 `expect`/`assert!`，因为任何 setup 或生产调用错误都应立即使测试失败；它不提供运行时恢复策略。显式错误边界包括：

- prepared LIMIT 拒绝负数和小数字符串，并检查 MySQL 兼容错误文本。
- peer transaction 对已持有记录锁的 `LockKeys` 必须失败；失败语句结束后 peer 应能锁住刚释放的语句级记录。
- killer 收到显式 QueryInterrupted 或 deadline 写入的 `MaxExecTimeExceeded` 后，`KillSignal` 必须返回错误；`CancelMaximumExecutionTime` 后复用 session 不应被旧 timer 杀死。
- `TryDetach` 同时返回可选 result 与 `detachable` 标志；测试只有在两者均表明成功时继续验证独立生命周期。
- 未绑定的 prepared 执行和把只读 typed scan 强行标记为 locking select 必须被拒绝。
- 审计插件由 `PluginCleanup::drop` 无条件 shutdown 并清空静态插件，避免全局插件注册污染其他测试。

`PreparedBridgeSchemaLoader` 对任意 keyspace 返回存在、对加载永不报错，这是该测试夹具的受控简化，不代表生产 schema loader 的错误行为。

## 并发与资源生命周期

并发契约首先由 `assert_send_sync::<OwnedKVSnapshotSource>()` 在编译期保证。detach 测试进一步验证运行期所有权：分离后的 executor/result set 在原 executor、session、owner 乃至 domain 被丢弃后仍能读取固定快照；这要求快照、schema/存储句柄和剩余扫描状态均由 detached 对象独立拥有。

行锁测试使用两个共享同一 domain 的 session 模拟竞争事务。owner 被 drop 后事务锁应释放；语句失败时只回滚语句开始之后新增的锁，事务此前持有的锁继续存在。`SELECT FOR UPDATE` 场景则要求执行阶段先完成锁定，再暴露缓冲结果。

deadline 场景包含后台计时行为：设置 process info 后即开始计时，即使调用方没有继续轮询 `Next`，killer 仍应在期限后收到信号；完成语句必须调用取消逻辑，防止 timer 影响复用的 session。审计插件使用 `Arc<Mutex<Vec<_>>>` 收集回调事件，并以 RAII cleanup 恢复全局插件状态。

结果集的规范生命周期是 `Open/Exec → Next* → OnFetchReturned（可选）→ Close`。`Close` 触发 statement finish 与缓存准入；测试主动按依赖逆序 drop，再检查 detached 分支，防止“仅因原 session 尚存而成功”的假阳性。

## 与 Go 版本的对应关系

没有与本测试同路径、同名的 Go 文件；对应关系是行为级而非逐行移植。主要 Go 锚点为：

- `pkg/executor/adapter.go` 的 `ExecStmt`、`RebuildPlan` 与 `Exec`：对应 Rust prepared plan 重建、执行器构建、流式 result set 和错误清理。
- 同文件 `recordSet.TryDetach`：Go 通过 `Detach(executor)` 创建不依赖当前 session context 的静态 record set，对应 Rust detached snapshot/result-set 生命周期测试。
- 同文件 `handlePessimisticSelectForUpdate`：Go 在 `OnPessimisticStmtStart/End` 包围的重试循环内执行、加锁并处理冲突，对应 Rust 的先锁后返回与失败语句锁回滚测试。
- 同文件 `logAudit` 与 `FinishExecuteStmt`：分别对应插件 completed event，以及 slow query、summary、Top SQL、runtime stats 等收尾副作用。
- `pkg/session/session.go` 的 `execStmtResult.TryDetach`：Go 还要求只读、自动提交并在 detach 后完成原 statement/session 清理；本文件验证 Rust 所暴露的低层独立所有权，但没有声称覆盖 Go 外层所有准入条件。

Rust 额外以 typed physical plan、`RuntimeExecute`、`OwnedKVSnapshotSource` 和 `SessionBoundAdapterOwner` 表达这些边界。测试要求最终可见行为尽量保持 Go 语义，同时明确检查 Rust 特有的类型化物理树和线程安全所有权。

## 扩展指南

扩展生产 adapter 时，应在生产文件修改真实实现，并在本独立测试文件增加或扩展场景；不要把测试嵌入 `pkg/session/runtime/typed_adapter_bridge.rs`。常见接入点如下：

- 新物理算子：更新生产侧物理计划绑定/构建逻辑，并扩展 `session_bound_adapter_accepts_a_canonical_physical_limit_root` 类场景，核对 typed tree、schema、分页和 EOF。
- prepared 参数或缓存规则：扩展 `canonical_prepared_exec_stmt_binds_limit_and_streams_rows_then_restores_plan_cache`，同时验证首次 miss、后续 hit、参数重绑定、`RebuildPlan` 和 SQL dispatch；避免只测手工构造入口。
- 锁或重试语义：同步检查事务既有锁、语句新增锁、peer 冲突、失败回滚和 owner drop，不可只断言返回错误。
- detach：必须在 drop 原 session/domain 后继续读取，并保留 `Send + Sync` 编译期断言；若新增资源不能安全分离，应返回不可 detach，而不是共享易悬空状态。
- timeout/审计/收尾指标：使用可确定的短 timeout、RAII 清理全局插件，并核对真实 canonical session view；注意 wall-clock 测试可能有调度抖动，断言应聚焦状态而非精确耗时。

兼容风险集中在 Go 生命周期顺序、prepared cache 参数污染、锁释放过多/过少和 detach 后资源悬空；性能风险集中在错误地把 lazy scan 变成 eager materialization、OFFSET 下推读取不足或过多，以及为 detach 不必要地复制完整结果。

## 验证依据

- 目标源码：`pkg/session/runtime_test/typed_adapter_bridge.rs`，共 1111 行；RustCodeGraph `node --file` 分三段读取，确认 14 个 `#[test]`、`PreparedBridgeSchemaLoader` 及全部断言路径。
- 图查询：`rustcodegraph status` 显示索引包含 7032 个 Rust 文件；对目标路径执行 `explore`，并对 prepared、普通 typed scan、pessimistic select 三个关键测试执行 `callees`。图中同名符号歧义已通过限定源码复核。
- 模块与 crate：`pkg/session/lib.rs`、`pkg/session/runtime_test.rs`、`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`。
- Rust 生产实现入口：`pkg/session/runtime/typed_adapter_bridge.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`；测试数据辅助来自同级 `runtime_test/planning.rs`。
- Go 行为对照：`pkg/executor/adapter.go` 的 `recordSet.TryDetach`、`ExecStmt`、`RebuildPlan`、`Exec`、`handlePessimisticSelectForUpdate`、`logAudit`、`FinishExecuteStmt`，以及 `pkg/session/session.go` 的 `execStmtResult.TryDetach`。
- 相关独立测试搜索：父模块中没有另一份同名测试文件；相邻 `pkg/session/runtime/scan_adapter_runtime_test.rs` 覆盖 adapter 的 RU、外键、悲观重试等其他切面，本文件聚焦 typed plan/scan 与 canonical session 的桥接。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付前应以任务给定命令确认目标文档存在且恰有 11 个固定二级标题，并用 `git diff --check` 检查 Markdown 差异。

# `pkg/session/runtime_test/ddl.rs`

## 文件定位

[`ddl.rs`](./ddl.rs) 不是生产 DDL 实现，而是 `astersql-session` crate 的测试专用会话运行时回归模块。它由 [`pkg/session/lib.rs`](../lib.rs) 在 `#[cfg(test)]` 下装入 [`pkg/session/runtime_test.rs`](../runtime_test.rs)，后者再以 `#[path = "runtime_test/ddl.rs"] mod ddl;` 纳入本文件。因而这里的 15 个入口只由 Rust 测试 harness 调用，不形成 crate 公共 API，也不会进入非测试构建。

[`pkg/session/Cargo.toml`](../Cargo.toml) 声明包名 `astersql-session`、库入口 `lib.rs` 和可选 feature `nextgen`；本文件没有自己的 feature gate。它通过父模块的 `use super::*` 取得 `Arc`、原子 `Ordering`、`ConcreteSession` 等测试上下文，并直接使用 crate 依赖中的 `astersql-ddl`、`astersql-dxf-*`、`astersql-executor-importer`、`astersql-infoschema`、`astersql-meta-model`、`astersql-objstore`、`astersql-errors` 与 `astersql-testkit-testfailpoint`。`pkg/session` 下没有 `doc.go`，所以这里以 Cargo、`lib.rs` 和模块入口界定包边界。

## 核心职责

- 通过 `crate::runtime::CreateAnalyzeSession` 建立带共享 `Domain` 的真实 `ConcreteSession`，用 SQL 端到端覆盖数据库、表、索引、外键、表达式索引、事务快照及管理语句，而不是直接调用内部 DDL 辅助函数。
- 验证 DDL 与已持久化行、`Domain`/InfoSchema 元数据及新会话之间的一致性，包括已有数据上的唯一键/外键校验、隐式索引、隐藏表达式列、表模式与 schema version。
- 覆盖散布和 schema self-version 的故障边界：进程级 split 开关、session/global `tidb_scatter_region`、region 拓扑、failpoint 回调以及三次 etcd 写入失败后的重试。
- 验证 classic DDL 会话与 DXF IMPORT INTO 的生产适配，包括事务起始时间戳、空表检查、统计增量、持久化 task/subtask 调度及提交时切换 `TableMode`。
- 固化若干跨 DDL/DML/planner 的兼容回归，如 `USE` 后 WordPress 查询、复合键谓词、匿名索引命名、`REPEAT` 表达式以及显式事务中的 `tidb_snapshot`。

## 主要符号

文件没有模块级常量、公开类型或公开函数；共有 15 个私有 `#[test]` 函数：

- `use_updates_planner_current_database_for_wordpress_queries`：确认 `USE` 会影响后续未限定表名的 SHOW、聚合、LEFT JOIN、GROUP BY、ORDER BY 与 LIMIT 规划。
- `concrete_database_session_state_drives_ddl_dml_and_admin_check_table` 与 `empty_database_is_visible_to_other_sessions_and_can_be_cleaned_up`：覆盖数据库名大小写、`IF [NOT] EXISTS`、当前库、跨会话可见性、`ADMIN CHECK TABLE` 及删除后的元数据失效。
- `concrete_alter_index_and_foreign_key_paths_use_persisted_rows`：从既有行校验自引用外键和 UNIQUE 索引，检查错误 1452/duplicate entry，并确认表达式索引生成隐藏列。
- `concrete_repeat_create_index_and_admin_check_index_use_production_paths`：验证表达式 DML、`CREATE INDEX` 持久化和存在/缺失索引的管理检查。
- `concrete_composite_predicates_and_anonymous_indexes_follow_mysql_ddl_dml`：验证复合谓词精确更新/删除以及匿名索引名取首列名。
- `scatter_scope_inherits_globally_and_show_regions_comes_from_table_metadata`：用局部 `SplitTableRegionReset` 的 `Drop` 恢复全局开关，观察 scatter failpoint、16 个 region 和 3 个 leader，并验证新会话继承全局变量。
- `ddl_self_version_update_retries_three_injected_etcd_failures`：要求注入的三次失败都发出 `retry` 事件，随后 CREATE/DROP TABLE 仍成功。
- `tidb_snapshot_can_be_set_in_a_normal_explicit_transaction`：捕获当前 TSO，在普通显式事务中设置快照，回滚后清空。
- `dxf_backend_alters_classic_table_mode_with_canonical_session` 与 `dxf_backend_exposes_current_transaction_start_ts_for_import_stats`：验证 DXF transaction session 的表模式切换、幂等版本行为和稳定非零 `TxnStartTS`。
- `import_scheduler_classic_table_empty_check_runs_on_canonical_session`、`import_scheduler_stats_delta_runs_on_canonical_transaction`、`registered_import_scheduler_dispatches_durable_subtask_with_storage_adapter`、`importinto_submission_uses_canonical_sql_and_classic_ddl_backend`：覆盖 import scheduler 的 classic SQL/事务/存储适配和提交链。

测试内部另有两个局部类型：`SplitTableRegionReset(u32)` 以 RAII 恢复 `EnableSplitTableRegion`；`Runtime` 实现 `ImportSchedulerRuntime`，仅为 scheduler factory 提供本测试所需的 import/normal mode 成功路径，未使用的 registration 明确返回错误。

## 执行流程

1. Rust 测试 harness 经 `lib.rs` 和 `runtime_test.rs` 发现本文件的 `#[test]` 函数。每个测试独立调用 `CreateAnalyzeSession`，得到共享的 `Domain` 与 canonical `ConcreteSession`。
2. 前九个测试主要从 `ConcreteSession::execute` 进入 [`pkg/session/runtime/dispatch.rs`](../runtime/dispatch.rs) 的解析/分派主链，再落到 [`pkg/session/runtime/ddl.rs`](../runtime/ddl.rs)、DML、查询或管理语句实现；结果通过 `ConcreteRecordSet::Next` 逐行断言，元数据则经 `domain.stats_table`、`info_schema` 或 `table_by_name` 交叉确认。
3. 散布测试先开启 `astersql_ddl::EnableSplitTableRegion`，注册 3 个 runtime store 和 `preSplitAndScatter` 回调，再建带两个分区、`shard_row_id_bits=10`、`pre_split_regions=3` 的表；它检查 failpoint 收到 `table`、SHOW REGIONS 返回 16 行且 leader 覆盖 3 个 store，最后创建新会话验证 global-to-session 继承。
4. self-version 测试为同一 failpoint 同时安装事件监听器和 `3*return(true)` 故障，执行建表触发 [`runtime/ddl.rs`](../runtime/ddl.rs) 的有界重试，并以计数恰为 3 和后续 DROP 成功证明恢复。
5. DXF 测试从 `session.ImportTaskManager()` 取得 production adapter，在 `WithNewTxn` 中切换表模式、读取事务时间戳或刷新统计；scheduler 测试先写入 managed node/task 元数据，再用 `StorageTaskManagerAdapter`、注册的 factory 和最小 encode services 执行一次调度，确认任务进入 `ImportStepImport` 且产生一个 pending subtask。
6. 最后的提交测试组装 `StorageTaskSubmissionService` 与 importer `Plan`，调用 `SubmitTask`，要求 job/task ID 都为正，并从 `Domain` 确认 classic 表已进入 `TableModeImport`。

## 数据与状态

- SQL 数据与 schema 状态保存在 canonical runtime 的共享 `Domain` 中；`ConcreteSession` 维护当前数据库、session variables、显式事务和 snapshot 状态。`ConcreteSession::new(domain)` 证明同一 Domain 上的新会话能看到空数据库和 global 变量，但拥有独立的 session 状态。
- 元数据断言读取 `TableInfo` 的 `ForeignKeys`、`Indices`、`Columns[*].Hidden`、`Mode`、`ID`，以及 InfoSchema 的 schema ID/`SchemaMetaVersion`。测试既检查 SQL 可见结果，也检查内部元数据，避免只证明表面语句成功。
- import 流程的数据载体为 `astersql_dxf_importinto::TaskMeta`、`astersql_executor_importer::Plan`/`Summary`/`Chunk`、DXF task base 与 subtask state；任务 manager 是 cloneable handle，底层持久状态由 storage adapter 共享。
- `EnableSplitTableRegion`、failpoint 注册表和 import scheduler factory 是进程级共享状态；散布观察向量用 `Arc<Mutex<Vec<String>>>`，重试计数用 `Arc<AtomicUsize>`。
- 每个测试使用不同的 database/table/task key，降低并行运行时的命名碰撞；但全局开关、failpoint 名和 scheduler factory 仍是跨测试共享资源。

## 依赖与调用关系

模块上游只有测试装配链：[`pkg/session/lib.rs`](../lib.rs) 的 `#[cfg(test)] mod runtime_test` -> [`pkg/session/runtime_test.rs`](../runtime_test.rs) 的 `mod ddl` -> 15 个 test harness 入口。RustCodeGraph 将目标识别为含 23 个符号的已索引文件；查询能定位 `scatter_scope_inherits_globally_and_show_regions_comes_from_table_metadata`，其静态 callees 仅识别出动态分派的部分边，因此不能把图中缺失边解释为没有调用。

主要下游链为：

- `CreateAnalyzeSession`（[`runtime/session.rs`](../runtime/session.rs)）创建 canonical Domain/session；`ConcreteSession::execute`（[`runtime/dispatch.rs`](../runtime/dispatch.rs)）解析 SQL 并分派 DDL、DML、query、admin 与 variables 路径。
- 会话 DDL 行为落到 [`runtime/ddl.rs`](../runtime/ddl.rs)，包括 database/table/index/foreign-key 元数据、`pre_split_and_scatter` 和 `update_self_version_with_retry`；`Domain`/InfoSchema 查询提供独立观察面。
- failpoint 由 `astersql-testkit-testfailpoint` 安装，生产触发点使用与 Go 相同的完整名称 `github.com/pingcap/tidb/pkg/ddl/preSplitAndScatter` 和 `.../util/PutKVToEtcdError`。
- import 测试直连 `astersql-dxf-importinto` 的 `ProductionCheckImportTableEmpty`、`FlushImportStatsProduction`、scheduler factory 注册和 `job::SubmitTask`，并经 `astersql-dxf-framework-storage`/`scheduler` 访问事务 SQL executor 与持久 task/subtask。

这些都是测试到生产接口的调用，不表示本文件实现了 DDL 或 DXF 算法。

## 错误处理与边界

- 正常路径普遍用带语义标签的 `expect`；预期失败必须显式取 `err()`/`unwrap_err()` 并检查错误码或稳定消息片段，例如外键 `[ddl:1452]`、重复索引值、缺失索引和非空导入目标。
- `IF NOT EXISTS`/`IF EXISTS` 的幂等路径与无保护条件的错误路径成对出现；数据库名称用不同大小写创建/选择，固定大小写不敏感契约。
- 唯一键、外键和表达式索引都先插入数据再 ALTER，确保覆盖“读取已持久化行进行校验”，而非只覆盖空表元数据写入。
- scheduler 的局部 `Runtime::new_task_registration` 返回 `unused`，说明测试只覆盖当前一次 `schedule_once` 所需链路；encode services 的若干闭包是 `unreachable!()`，不能据此声称编码、导入器 RPC、worker 或对象存储全流程已执行。
- `FlushImportStatsProduction` 测试只断言调用成功，没有再次查询统计表确认值；快照测试也只验证设置/清理成功，没有执行历史快照读。因此文档不扩大这些测试的证明范围。
- 文件没有自定义超时；若 production 调用发生阻塞，测试本身不会提供额外取消边界。

## 并发与资源生命周期

- `SplitTableRegionReset` 在作用域退出（含 panic unwind）时以 `SeqCst` 恢复旧的全局 split 开关；failpoint 的 enable guard 同样依靠析构注销。这个顺序是并行测试安全的重要约束，新增断言不能泄漏 guard 或用永久全局写替代。
- scatter 回调可能收到其他并发测试产生的默认空 scope，因此只记录本测试配置的 `table`；观察结果通过 `Mutex` 串行访问。此过滤是对进程级 failpoint 注册表共享性的明确处理。
- 重试计数用 `AtomicUsize` 和 `SeqCst`，保证回调与断言之间可见；其生命周期由两个 failpoint guard 和 `Arc` 共同限定在测试内。
- `WithNewTxn` 把 table-mode、start-ts 和 stats 操作限制在独立事务中；同一闭包内两次 `TxnStartTS` 必须相同。重复切换到 import mode 不增加 `SchemaMetaVersion`，随后切回 normal mode，体现幂等与清理契约。
- scheduler 测试在断言后显式调用 `scheduler.close()`；内存对象存储与 manager/service 由 `Arc` 持有，离开测试后释放。它没有启动真实网络服务或后台集群。

## 与 Go 版本的对应关系

- 散布语义直接对应 [`tests/realtikvtest/ddltest/ddl_test.go`](../../../tests/realtikvtest/ddltest/ddl_test.go) 中 session/global `tidb_scatter_region`、`preSplitAndScatter` 回调、SHOW REGIONS leader 分布的用例；生产算法对应 [`pkg/ddl/executor.go`](../../ddl/executor.go) 的 `preSplitAndScatter`/`preSplitAndScatterTable`。Rust 使用内存 runtime topology 做确定性断言，不等同于启动真实 TiKV。
- 三次 etcd 故障后 CREATE/DROP 仍成功，对应同一 Go 测试文件的 `TestUpdateSelfVersionFail`，注入点的 Go 实现在 [`pkg/ddl/util/util.go`](../../ddl/util/util.go) 的 `PutKVToEtcd`。Rust 额外通过 value-call 计数明确断言发生三次 retry。
- IMPORT INTO 提交的 Go 主链位于 [`pkg/dxf/importinto/job.go`](../../dxf/importinto/job.go) 的 `SubmitTask`/`doSubmitTask`，executor 接线见 [`pkg/executor/import_into.go`](../../executor/import_into.go)。Rust 测试使用 storage-backed service 显式注入 manager，更适合 canonical 内存会话，但仍核对 job/task ID 与 classic table mode。
- 空表检查、统计刷新和 scheduler 的 Rust 真实实现位于 [`pkg/dxf/importinto/scheduler.rs`](../../dxf/importinto/scheduler.rs)；相关独立 Rust 单元测试还有 [`pkg/dxf/importinto/scheduler_test.rs`](../../dxf/importinto/scheduler_test.rs) 和 [`pkg/dxf/importinto/job_test.rs`](../../dxf/importinto/job_test.rs)。本文件关注这些实现与 `astersql-session` canonical transaction adapter 的集成，不替代下游细粒度测试。
- 其余 database/index/foreign-key/admin/planner 用例是 Rust 会话运行时的聚焦回归集合；仓库中没有与本文件一一同路径、同函数名的 Go 文件，因此只能按具体 SQL/生产入口关联，不能宣称逐函数机械翻译。

## 扩展指南

- 新增会话 DDL 回归应继续放在本独立测试文件或同目录其他 `runtime_test/*.rs` 中，不要把测试嵌入 [`runtime/ddl.rs`](../runtime/ddl.rs) 等生产源文件。先选择用户可见 SQL 入口，再同时断言结果集与 `Domain`/InfoSchema 状态。
- 扩展 ALTER 校验时，应先写入能区分成功/失败的数据，再检查稳定的 TiDB/MySQL error code；不要只匹配易变化的完整错误字符串。索引、隐藏列、外键等还应核对 `TableInfo`。
- 新增进程级开关、failpoint 或全局 factory 覆盖时，必须使用 RAII guard 恢复，并考虑 Rust 测试默认并行；若事件可能来自其他测试，应使用唯一键或严格过滤，而不是依赖执行顺序。
- 扩展 DXF 用例时，区分 adapter 集成与下游算法：需要编码/RPC/worker 行为时，应在 `pkg/dxf/importinto/*_test.rs` 增加独立测试，不应继续向这里的 `unreachable!()` 最小 services 填充完整子系统。
- 变更 Go 对齐行为时，应同步复核 `tests/realtikvtest/ddltest/ddl_test.go`、`pkg/ddl/executor.go`、`pkg/ddl/util/util.go`、`pkg/dxf/importinto/job.go` 及相关 Rust 独立测试；兼容风险主要是错误码、session/global 继承、schema version 幂等和 task 状态，性能风险主要由真实 region/etcd/分布式调度路径承担，本内存测试不能测量。

## 验证依据

- RustCodeGraph：`status` 报告 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/session/runtime_test/ddl.rs` 确认目标已索引且有 23 个符号；`query scatter_scope_inherits_globally_and_show_regions_comes_from_table_metadata --kind function` 定位到 `ddl.rs:396`，`callees` 仅返回部分动态边。对较长测试名的后续 callers/callees 查询没有产生可用输出，因此调用关系又以直接源码核验，未将图缺边当成事实结论。
- 目标与装配：[`pkg/session/runtime_test/ddl.rs`](./ddl.rs)、[`pkg/session/runtime_test.rs`](../runtime_test.rs)、[`pkg/session/lib.rs`](../lib.rs)。目标源码确认 15 个 `#[test]`、一个局部 `Drop` 实现和一个局部 `ImportSchedulerRuntime` 实现。
- crate 与生产入口：[`pkg/session/Cargo.toml`](../Cargo.toml)、[`pkg/session/runtime/session.rs`](../runtime/session.rs) 的 `ConcreteSession`/`CreateAnalyzeSession`、[`pkg/session/runtime/dispatch.rs`](../runtime/dispatch.rs) 的 `execute`、[`pkg/session/runtime/ddl.rs`](../runtime/ddl.rs)。
- Go 与下游对照：[`tests/realtikvtest/ddltest/ddl_test.go`](../../../tests/realtikvtest/ddltest/ddl_test.go)、[`pkg/ddl/executor.go`](../../ddl/executor.go)、[`pkg/ddl/util/util.go`](../../ddl/util/util.go)、[`pkg/dxf/importinto/job.go`](../../dxf/importinto/job.go)、[`pkg/dxf/importinto/scheduler.rs`](../../dxf/importinto/scheduler.rs)。相关独立 Rust 测试为 [`pkg/dxf/importinto/scheduler_test.rs`](../../dxf/importinto/scheduler_test.rs)、[`pkg/dxf/importinto/job_test.rs`](../../dxf/importinto/job_test.rs) 及 [`pkg/session/runtime/ddl_test.rs`](../runtime/ddl_test.rs)。
- 本任务只新增分析文档，按计划不运行 Cargo 或代码测试；交付时运行任务指定的固定章节结构检查，并人工复核相对链接、测试边界及“不把内存适配测试扩大成真实集群证明”的表述。

# `pkg/ddl/persistent_mview_out_of_place_cutover.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），实现 `ActionMViewRefreshOutOfPlaceCutover`（动作值 92）的持久化 DDL worker 步骤。模块由 `pkg/ddl/lib.rs` 公开声明；普通持久化动作入口 `pkg/ddl/persistent_actions.rs::step` 在 `job.tp == ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER` 时调用本文件的 `step`。

它处理的是完整刷新已经在独立 shadow table 中构建完毕后的“切换”阶段，而不是构建数据本身：旧物化视图从元数据中删除，shadow table 被提升为同名的新物化视图，同时修正基表、可选 MLog、刷新调度行和 schema diff。旧表物理数据的后续清理由 `pkg/ddl/delete_range.rs` 对该动作读取 `OldMViewID` 后生成 delete-range 任务。

目标文件没有条件编译项、模块级常量、自定义类型或 trait；其运行时 API 是公开的 `step`，另有四个文件内/`pub(crate)` 辅助函数。测试独立放在 `pkg/ddl/persistent_mview_out_of_place_cutover_test.rs`，没有内嵌到生产文件。

## 核心职责

`step(context, job)` 的职责是把一次 out-of-place 刷新的多个持久状态作为一个 DDL cutover 推进：

1. 解码并验证 `RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs`，拒绝表 ID、构建 TSO、revision 或对象类型不匹配的陈旧/畸形任务。
2. 读取旧 MV、shadow table、唯一基表及可选 MLog，并验证它们之间的引用关系。
3. 把基表 `MaterializedViewBase.MViewIDs` 和 MLog `DependentMViewIDs` 中的旧 MV ID 替换为 shadow ID，同时去重。
4. 通过 `JobExecutionContext::migrate_mview_refresh_info` 把 `mysql.tidb_mview_refresh_info` 的主键及刷新进度迁到新 ID。
5. 将 shadow table 复制成提升对象：继承旧 MV 的名称、注释和 `MaterializedView` 定义，清除 `MaterializedViewShadow` 标记；再删除旧表、更新相关表、生成 schema version/diff、发通知并结束 job。

这是 job-based DDL 的单步执行器，不是 metadata-only 的前端捷径。它没有逐阶段的 `delete-only/write-only/reorg/public` 状态机；成功时直接以 `SchemaState::Public` 完成。数据构建已在切换前完成，本文件自身不扫描或回填用户数据。

## 主要符号

- `cancel(job: &mut Job, message: impl ToString) -> String`：把 `job.state` 设置为 `JobState::Cancelled`，并把错误归一成字符串。解码错误、刷新信息迁移错误会走此路径。
- `invalid(job: &mut Job, detail: &str) -> String`：在取消 job 的同时，为本模块的校验错误添加 `[ddl:8204]refresh materialized view complete OUT OF PLACE cutover:` 前缀。
- `replace_materialized_view_id(ids, old_id, new_id) -> (Vec<i64>, bool)`：按原顺序遍历 ID；把每个 `old_id` 替换成 `new_id`，用 `HashSet` 保留首次出现值并删除重复项。布尔值只表示是否见过旧 ID，即使输入本来已有新 ID也不会误报“已替换”。
- `rewrite_materialized_view_base(table, old_id, new_id) -> Result<(), String>`：要求 `table.MaterializedViewBase` 存在，调用上述替换函数，并要求旧 ID 确实出现在 `MViewIDs` 中；成功后写回去重后的列表。
- `step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：唯一公开运行入口；返回新 schema version，成功时将 job 完结为 `Done/Public`。

`step` 使用的关键外部数据类型来自 `astersql-meta-model`：`Job`、`JobState`、`SchemaState`、`TableInfo` 和 `RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs`；持久元数据操作由 `astersql-meta::TransactionMutator` 提供。这两个 crate 以及 `astersql-ddl-notifier` 都是 `pkg/ddl/Cargo.toml` 的直接依赖。

## 执行流程

1. `GetRefreshMaterializedViewCompleteOutOfPlaceCutoverArgs(job)` 解码参数。必须满足 `OldMViewID == job.table_id`、旧 ID 与 shadow ID 不同、`BuildReadTSO != 0`。
2. 第一个 `context.with_transaction` 读取同一 schema 下的旧 MV 和 shadow table。旧对象必须有 `MaterializedView`，且当前实现只允许恰好一个 `BaseTableIDs`；如果参数携带 `ExpectedOldMViewRevision`，它必须等于旧对象的 `Revision`。
3. shadow 必须处于 `Public`，必须以 `MaterializedViewShadow.SourceMViewID == OldMViewID` 指向旧 MV，并且不能已经是 MV、MLog、普通 View 或 Sequence。
4. 第二个 `with_transaction` 读取唯一基表；若基表的 `MaterializedViewBase.MLogID` 非零，再读取 MLog。随后必须在基表的 `MViewIDs` 中把旧 ID 改成 shadow ID。
5. 若读取到了 MLog，则其 `MaterializedViewLog.BaseTableID` 必须匹配基表。只有 `DependentMViewIDs` 实际包含旧 ID时才更新并保留该 MLog 进入最终写集合；否则丢弃本地 `log`，不写它。
6. `migrate_mview_refresh_info(&args)` 校验刷新行仍处于调用者预期的 `LAST_SUCCESS_READ_TSO`，把主键改为 shadow ID，并写入 `BuildReadTSO`、完成时间以及可选的下一刷新时间。实际实现位于 `pkg/session/runtime/system_session.rs`。
7. shadow 副本继承旧 MV 的 `Name`、`Comment` 和 `MaterializedView`，并清空 `MaterializedViewShadow`。
8. 最后一个 `with_transaction` 删除旧表及其 auto IDs，更新提升后的表、基表和必要的 MLog，生成 schema version，再调用 `TransactionMutator::set_mview_cutover_schema_diff`。diff 以 shadow ID 为新 `table_id`、旧 MV ID 为 `old_table_id`，并把基表/MLog 加入受影响对象以促使 InfoSchema 失效重载。
9. `persistent_actions::async_notify_event` 发布同时携带新、旧完整 `TableInfo` 的 `NewMViewRefreshOutOfPlaceCutoverEvent`。只有通知成功后，`finish_table_job(Done, Public, version, promoted)` 才将提升对象写入 job history 信息并返回 version。

## 数据与状态

输入参数的完整持久契约定义在 `pkg/meta/model/job_args.rs::RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs`：旧/新 ID、构建读 TSO、可选旧 revision、预期的刷新 TSO（含 NULL 标志）、可选下一刷新时间及是否更新该时间。前端 Go 入口 `pkg/ddl/materialized_view.go::RefreshMaterializedViewCompleteOutOfPlaceCutover` 构造动作 92 的 job，并把旧 MV ID同时放入 `job.TableID`。

切换维护以下关系：旧 MV 的 `MaterializedView.BaseTableIDs` 必须恰有一个元素；基表的 `MaterializedViewBase.MViewIDs` 必须包含旧 ID；可选 MLog 的 `BaseTableID` 必须指回基表，而它的 `DependentMViewIDs` 仅在包含旧 ID时改写。列表改写既替换全部旧 ID，也消除替换后与既有新 ID发生的重复，并保留首次出现顺序。

job 在任何参数/元数据校验失败时变为 `Cancelled`；成功时变为 `Done`，schema state 是 `Public`。本文件不维护可恢复的中间枚举状态。`affected` 仅收集基表 ID 和实际改写的 MLog ID，供 schema diff 使相关缓存失效；提升对象自身以及旧→新映射由 diff 的主字段和首个 affected option 表达。

## 依赖与调用关系

上游主链为：Go DDL executor 构造 `ActionMViewRefreshOutOfPlaceCutover` job → owner/worker 执行持久动作 → `pkg/ddl/persistent_actions.rs::step` → 本文件 `step`。RustCodeGraph 对目标文件的 `node --file` 显示直接使用者为 `pkg/ddl/persistent_actions.rs`、独立测试和 `pkg/session/runtime/system_session.rs`；源码搜索确认实际分派边在 `persistent_actions.rs`。

下游依赖包括：

- `TransactionMutator::{get_table, drop_table_and_auto_ids, update_table, gen_schema_version, set_mview_cutover_schema_diff}`，负责 KV 元数据读写和 schema diff。
- `JobExecutionContext::{with_transaction, migrate_mview_refresh_info}`；生产上下文的刷新行迁移实现在 `pkg/session/runtime/system_session.rs`。
- `persistent_actions::async_notify_event` 和 `astersql_ddl_notifier::NewMViewRefreshOutOfPlaceCutoverEvent`，向订阅者提供新旧对象。
- `Job::finish_table_job`，记录完成状态、version 和最终 `TableInfo`。
- `pkg/ddl/delete_range.rs`，在已完成 job 的后处理阶段按 `OldMViewID` 回收旧物理表范围。

该切换还依赖 InfoSchema 消费 schema diff 的语义；`pkg/meta/reader.rs::set_mview_cutover_schema_diff` 明确编码旧 ID→shadow ID 和相关基表/MLog 的 invalidation，而不是只发布一个普通单表更新。

## 错误处理与边界

可预期的无效输入均返回 `Err(String)` 并取消 job，包括：参数解码失败；ID/TSO 不合法；旧 MV、shadow 或基表不存在；旧对象不是单基表 MV；revision 已过期；shadow 类型、来源或状态不符；基表反向引用缺失；MLog 元数据类型或基表引用错误；刷新调度行缺失/陈旧；元事务或通知失败。

`ExpectedOldMViewRevision` 和预期 `LAST_SUCCESS_READ_TSO` 是两个乐观并发防线：前者防止用过期定义提升 shadow，后者防止覆盖较新的刷新进度。`BuildReadTSO == 0` 被视为非法，避免发布没有有效读快照的构建结果。

当前 Rust 边界与 Go 版本并非完全相同，扩展时不能忽略：

- 基表指向非零 MLog ID但 `get_table` 返回 `None` 时，Rust 会把 `log` 留空并继续；Go 的 `getTableInfoAndCancelNonExistJob` 会报错。这是当前 Rust 较宽松的缺失对象处理。
- Rust 对空 `MViewIDs` 返回“old materialized view id is missing”，Go 把 nil/空元数据都归为“base table materialized view metadata missing”。
- Rust 大多压成带 8204 前缀的字符串；Go 同一路径还会保留 `ErrWrongObject`、对象不存在等结构化错误类别。
- Rust 生产上下文当前通过 SQL `UPDATE mysql.tidb_mview_refresh_info` 迁移刷新行；Go 的对应实现明确改用 table API，理由是让刷新行与剩余 cutover 元数据保持同一 DDL transaction。是否具备完全相同的回滚原子性，不能仅由本文件证明，属于需要重点回归的兼容风险。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、锁或显式共享可变状态；`HashSet`、`Vec`、克隆的 `TableInfo` 都是单次调用的局部值。并发正确性主要来自持久 job 串行推进、revision/刷新 TSO 的陈旧检查以及底层 transaction。

三个 `with_transaction` 回调分别承担初始对象读取、基表/MLog 读取和最终元数据写入。刷新行迁移位于这些回调之外，通过同一 `JobExecutionContext` 的 session 执行；trait 注释要求实现者“不得独立提交”，但本文件无法单独证明生产实现跨这些调用的原子事务边界。任何在迁移刷新行之后、job 完成之前的错误都必须保证可重试或回滚；Go 集成测试专门覆盖迁移后失败、通知失败和提交前失败的回滚行为。

成功时旧 MV 元数据和 auto IDs 被删除，新 MV/基表/可选 MLog 与 schema diff 被写入，通知事件在 `finish_table_job` 之前发布。旧表物理 KV 不在此处同步删除，而由完成 job 派生的 delete-range 生命周期处理。因此新增失败点或外部副作用时，应特别检查“事件失败是否回滚”和“历史 job 是否仍足以触发旧 ID GC”。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/ddl/table.go::onRefreshMaterializedViewCompleteOutOfPlaceCutover`，辅助函数对应 `replaceMaterializedViewID` 与 `rewriteMaterializedViewBaseForOutOfPlaceCutover`。Rust 保留了 Go 的主次序和核心不变量：解码参数、验证旧/新对象、改写基表和 MLog、迁移刷新信息、删除旧对象、提升 shadow、更新 schema version、通知、完成 job。

Go 入口和涉及对象锁定信息位于 `pkg/ddl/materialized_view.go::RefreshMaterializedViewCompleteOutOfPlaceCutover` / `buildMViewRefreshOutOfPlaceCutoverInvolvingSchemaInfo`；Go schema diff 特化位于 `pkg/ddl/schema_version.go::SetSchemaDiffForMViewRefreshOutOfPlaceCutover`。Rust 把 diff 构造下沉到 `pkg/meta/reader.rs`，并显式附加基表/MLog affected options。

Go 实现还显式设置各 `TableInfo.UpdateTS`、用 `repairTableOrViewWithCheck` 更新表，并提供两个 cutover failpoint；目标 Rust 文件没有这些显式步骤或 failpoint。Go 的 `migrateMViewRefreshInfoForOutOfPlaceCutover` 使用 table API 原地维护四列系统表；Rust 的 session 实现使用 SELECT/UPDATE SQL。以上是可观察的迁移实现差异，不能写成“Rust 已与 Go 完全等价”。

Go 集成测试 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go` 验证 MLog 依赖替换、迁移后错误回滚、通知错误原子性和提交前错误保留 affinity group。Rust 独立测试目前只覆盖 ID 替换/去重、基表改写错误、事件携带新旧元数据和动作分派，不覆盖完整 `step` 的事务与失败恢复。

## 扩展指南

新增 cutover 元数据时，优先在 `step` 的验证区建立不变量，再在最终写事务中更新对象，并把会影响 InfoSchema 的额外表 ID加入 `affected`；同步检查 `pkg/meta/reader.rs::set_mview_cutover_schema_diff`、Go 的 `SetSchemaDiffForMViewRefreshOutOfPlaceCutover` 和 `pkg/infoschema` 的 diff 消费逻辑。不要只更新提升表而漏掉基表/MLog 的反向引用。

扩展 job 参数必须同时维护 Rust/Go 的 `RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs` 序列化兼容、job 提交入口和旧 job 解码测试。新增外部资源或系统表写入时，必须明确它与最终元数据写入是否同事务、错误后如何回滚、重试是否幂等；尤其应消除或证明当前 SQL 刷新行迁移与 Go table API 的原子性差异。

测试应继续放在独立文件：纯 helper/分派测试扩展 `pkg/ddl/persistent_mview_out_of_place_cutover_test.rs`；完整事务、failpoint、InfoSchema 和 GC 行为应对照并扩展 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go` 或新增同目录独立 Rust 测试文件。建议优先补齐：缺失 MLog、错误类型兼容、revision/刷新 TSO 竞争、通知失败回滚、schema diff affected options、旧 ID delete-range。

性能方面，列表替换是线性遍历并使用与唯一 ID 数量同阶的 `HashSet`；通常列表很小。更值得关注的是多次元数据读取和 SQL 系统表访问的事务往返。优化时不能用减少校验或拆分独立提交换取吞吐，否则会破坏 cutover 的原子发布语义。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query replace_materialized_view_id` 与 `query rewrite_materialized_view_base` 均唯一命中本文件；`node --file pkg/ddl/persistent_mview_out_of_place_cutover.rs --offset 1 --limit 500` 返回完整 215 行源码及三个直接使用文件。`explore` 和随后 `callers/callees` 未返回文本，因此调用关系又由直接源码搜索核验，没有把空结果当作无调用证据。
- 目标及装配：`pkg/ddl/persistent_mview_out_of_place_cutover.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/Cargo.toml`。
- 下游实现：`pkg/session/runtime/system_session.rs`、`pkg/meta/reader.rs`、`pkg/meta/model/job_args.rs`、`pkg/ddl/delete_range.rs`、`pkg/ddl/notifier/events.rs`。
- Go 对照：`pkg/ddl/table.go`、`pkg/ddl/materialized_view.go`、`pkg/ddl/schema_version.go`、`pkg/meta/model/job_args.go`。
- 测试证据：`pkg/ddl/persistent_mview_out_of_place_cutover_test.rs`；Go 集成测试 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go`；事件契约另有 `pkg/ddl/notifier/events_test.go`。
- 人工复核结论：文件存在是为了在持久 DDL worker 中原子推广已构建 shadow；运行路径、状态改写、引用修复、schema diff、通知和旧数据 GC 均能追溯到上述符号；安全扩展的关键是保持参数兼容、双向引用、缓存失效与失败回滚一致。按任务约束，本次是纯文档分析，未运行 Cargo 或代码测试。

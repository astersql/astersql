# `pkg/session/runtime/mlog.rs`

## 文件定位

`mlog.rs` 属于 `astersql-session` crate 的具体 SQL 会话运行时。模块由 `pkg/session/runtime.rs` 的 `mod mlog` 私有装配，并通过同文件的 `use mlog::RuntimeMLog` 仅供 runtime 内部 DML 代码使用；它不是公开的会话 API。`pkg/session/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/session`，并直接依赖 `astersql-kv`、`astersql-meta-model`、`astersql-table` 和 `astersql-types`。

该文件位于“关系型 DML 已确定基础表元数据”与“把基础表及 MLog 的 KV mutation 一并交给事务写入”之间。它只负责同步生成物化视图日志（Materialized View Log，MLog）记录；MLog 的 DDL、读取和后台清理分别在 `runtime/mview_ddl.rs`、`runtime/relational_scan.rs`、`runtime/mlog_purge.rs`，不属于本文件职责。

## 核心职责

- `RuntimeMLog::for_table` 判断基础表是否绑定 MLog，解析当前 InfoSchema 中的日志表，并在每条 DML 语句开始写 mutation 前验证基础表与日志表元数据契约。
- `RuntimeMLog::tracked_changed` 比较更新前后被 MLog 跟踪的列；只有至少一个跟踪值发生变化时，普通 UPDATE 才需要写旧、新两条日志。
- `RuntimeMLog::append` 将基础行投影为日志行，补充 `_MLOG$_DML_TYPE`、`_MLOG$_OLD_NEW` 和独立 `_tidb_rowid`，同时生成日志表索引 mutation 与行 mutation。
- 调用者把这些日志 mutation 追加到基础表 mutation 使用的同一个 `Vec<(kv::Key, Option<Vec<u8>>)>`，随后由 DML 层统一应用，因此日志与基础行共享语句/事务成败边界；这一性质由 `pkg/session/dml_runtime_test.rs::go_merge_49_sql_mlog_dml_shares_transaction` 覆盖。

## 主要符号

- `RuntimeMLog<'a>`：语句局部辅助对象，保存借用的 `ConcreteSession`、克隆后的日志表 `TableInfo`、已验证的基础表列偏移 `tracked_offsets` 和行编码 `Flags`。可见性为 `pub(super)`，仅父模块及其子模块使用。
- `RuntimeMLog::for_table(session, _database, base, flags) -> SessionResult<Option<Self>>`：无 `MaterializedViewBase` 或 `MLogID == 0` 时返回 `Ok(None)`；有绑定时拒绝分区基础表，从 `session.domain.info_schema().TableByID` 解析日志表，调用 `astersql_table::mview_log::validate_meta` 建立有序列映射。参数 `_database` 当前未参与解析，表关联以 ID 为准。
- `RuntimeMLog::tracked_changed(base, old, new) -> bool`：按 `tracked_offsets` 找到基础表小写列名，用 `HashMap::get` 比较 `Option<&Option<String>>`。因此缺少键与显式 `NULL` 不等价；完整 DML 行应由上游保证包含所需列。
- `RuntimeMLog::append(base, row, dml, old_new, mutations) -> SessionResult<()>`：按 MLog 元数据顺序复制跟踪列，写入 DML 类型和旧/新标记，分配日志表 row ID，编码日志行并添加索引与记录 mutation。`dml.as_str()` 产生 `I`、`U` 或 `D`；现有调用约定旧行使用 `-1`、新行使用 `1`。

文件没有模块级常量、trait、条件编译项或独立线程入口。

## 执行流程

1. `pkg/session/runtime/dml.rs` 的 INSERT、UPDATE、DELETE 入口，以及外键级联路径，在解析出目标 `TableInfo` 后调用 `RuntimeMLog::for_table`。未绑定 MLog 时得到 `None`，后续路径不产生额外 mutation。
2. `for_table` 读取基础表 `MaterializedViewBase.MLogID`。非零 ID 对分区表立即报错；普通表则从当前 InfoSchema 按 ID 取得日志表，并将 `ModelMeta` 错误转换为 `SessionError`。
3. `astersql_table::mview_log::validate_meta` 校验基础表记录的 MLog ID、日志表反向记录的基础表 ID、公开列数量与顺序、两个保留列名称，以及每个跟踪列在基础表中的合法偏移。只有校验成功才构造辅助对象。
4. INSERT/LOAD DATA 的新行调用 `append(..., Insert, 1, ...)`。DELETE 的旧行调用 `append(..., Delete, -1, ...)`。UPDATE、冲突更新、REPLACE 和主键变更按逻辑更新写旧行 `Update/-1` 与新行 `Update/1`；普通 UPDATE 和级联 UPDATE 先通过 `tracked_changed` 跳过未改变任何跟踪列的情况。
5. `append` 依照 `tracked_offsets` 与 `MaterializedViewLog.Columns` 的位置对应关系构造日志行。它不复制未跟踪列，并保留源行的 `NULL`。
6. 会话通过 `allocate_runtime_auto_id(self.log.ID, None, 2, 1, 1)` 为日志表分配 `_tidb_rowid`，随后 `encode_relational_row_for_write` 生成行 key/value，`relational_index_mutations` 生成日志表索引写入。
7. 索引 mutation 先扩展进调用者的 mutation 向量，记录 mutation 最后压入。上游 DML 再连同基础表 mutation 一次性交给 `apply_relational_mutations`；若构造阶段任一步失败，错误通过 `?` 返回，调用者不会提交未完成的向量。

## 数据与状态

`RuntimeMLog` 不持有独立存储状态。`session: &'a ConcreteSession` 的借用把对象生命周期限制在会话调用期间；`log` 是构造时从 InfoSchema 取得并克隆的日志表元数据快照；`tracked_offsets` 是经过校验、与 `log.MaterializedViewLog.Columns` 同序的基础表列下标；`flags` 在对象创建时从当前 DML 环境捕获，基础表与日志表使用同一编码设置。

日志行在内存中表示为 `HashMap<String, Option<String>>`。跟踪列键来自日志表的列名，值来自基础行对应列；两个系统列分别记录逻辑 DML 类型和旧/新侧；`_tidb_rowid` 是每条日志记录的物理句柄。输出不是立即写 KV，而是追加到调用者拥有的 mutation 向量：`Some(value)` 表示写入，索引维护可能同时包含删除或写入项。

## 依赖与调用关系

上游装配与调用关系如下：

- `pkg/session/runtime.rs` 声明 `mod mlog` 并导入 `RuntimeMLog`。
- `pkg/session/runtime/dml.rs::execute_relational_insert_with_load_counts`、`execute_relational_update`、`execute_relational_delete` 以及 UPDATE JOIN、外键级联更新/删除路径创建 `RuntimeMLog` 并追加日志。LOAD DATA 复用 INSERT mutation 流程，见 `pkg/session/dml_runtime_test.rs::mlog_load_data_uses_insert_mutation_pipeline`。
- RustCodeGraph 对 `RuntimeMLog` 的查询列出 `dml.rs` 中 7 个局部 `mlog` 实例，并把 `mlog.rs` 标记为由 `runtime.rs` 使用；图索引未为这三个 inherent method 建立可直接按限定名查询的 method 节点，因此具体调用点以 `dml.rs` 的源码搜索复核。

主要下游依赖如下：

- `ConcreteSession.domain.info_schema().TableByID`：按稳定表 ID 解析日志表，避免依赖当前数据库名；这也解释了 `_database` 尚未使用。
- `astersql_table::mview_log::validate_meta`：集中维护 Rust 表层与 Go 包装器一致的元数据不变量，返回跟踪列偏移。
- `ConcreteSession::allocate_runtime_auto_id`：为 MLog 隐式行句柄分配 ID。
- `ConcreteSession::encode_relational_row_for_write` 和 `relational_index_mutations`：将日志逻辑行转换为同一 KV mutation 协议。
- `astersql_meta_model` 提供 `TableInfo` 和两个保留列常量，`astersql_types::Flags` 控制编码，`astersql_kv::Key` 是 mutation key 类型。

## 错误处理与边界

- 未配置日志不是错误：缺少 `MaterializedViewBase` 或 `MLogID == 0` 返回 `Ok(None)`。
- 绑定 MLog 的分区基础表明确返回 `materialized view log on partitioned tables is not supported`；`pkg/session/dml_runtime_test.rs::mlog_rejects_import_into_and_partitioned_base_dml` 验证 DML 会拒绝该组合。
- InfoSchema 找不到目标 ID 时报告 `materialized view log ID ... is missing`；`ModelMeta` 与 `validate_meta` 的错误文本封装进 `SessionError`。因此 MLog ID 不匹配、反向基础表 ID 不匹配、列顺序/保留列错误、跟踪列缺失或偏移非法都会在生成 mutation 前失败。
- `append` 对 `self.log.MaterializedViewLog` 使用 `expect("validated materialized view log metadata")`。该断言依赖 `for_table` 已成功调用 `validate_meta` 且对象内部 `log` 未再变化；字段私有且对象只能由 `for_table` 构造，使这一不变量在本模块内成立。
- 跟踪列读取用 `row.get(...).cloned().flatten()`：缺失键会被投影成 `NULL`，而 `tracked_changed` 会区分缺失与显式 `NULL`。若未来引入稀疏行表示，必须先统一这两处语义，不能只修改其中之一。
- 自动 ID、行编码和索引 mutation 的错误均用 `?` 原样传播；本文件不重试、不吞错，也不单独提交。它同样不校验 `old_new` 只能为 `-1/1`，正确取值是调用者契约。

## 并发与资源生命周期

`RuntimeMLog` 本身没有锁、原子变量、通道、异步任务或后台资源。它只不可变借用 `ConcreteSession`，其方法接收 `&self`；每个调用者为当前语句创建局部对象和局部 mutation 向量，不在会话间共享。

日志表元数据在 `for_table` 时克隆，保证同一次 mutation 构造使用一致的列布局。真正的原子性来自上游：基础表 mutation 与 `append` 生成的 MLog mutation 放入同一向量并进入同一会话事务。显式事务回滚后 MLog 行消失的行为由 `go_merge_49_sql_mlog_dml_shares_transaction` 验证。MLog 后台 purge 有自己的事务、节流和任务生命周期，位于 `runtime/mlog_purge.rs`，不得在此对象中加入后台状态。

`allocate_runtime_auto_id` 会改变日志表的分配器状态，即使后续编码失败也可能留下未使用 ID；行 ID 间隙不是事务正确性问题，但扩展批量写入时应考虑分配次数与竞争成本。

## 与 Go 版本的对应关系

Go 的主要对照实现不是 `pkg/session` 下的同名文件，而是 `pkg/table/tables/mview_log.go`，由 `pkg/executor/builder.go::wrapTableWithMLogIfExists` 在执行器构建时包装基础表。两端共同语义包括：无 MLog 时直通、拒绝分区表、按 ID 解析日志表、验证元数据、仅投影跟踪列、用 `I/U/D` 与 `-1/1` 标记、更新写旧/新两条、未触及跟踪列时跳过，以及与基础写入共享事务。

实现层级不同：Go 通过实现 `table.Table` 的 `mlogTable` 拦截 `AddRecord`、`UpdateRecord`、`RemoveRecord`，并让日志表自身的 `AddRecord` 处理行 ID 与索引；本文件服务于 Rust 的 `HashMap<String, Option<String>>` 关系运行时，显式调用 auto-ID、行编码和索引 mutation 函数。Go 还用 `sourceStmt` 与 `removedConflict` 区分 REPLACE、LOAD DATA 和冲突更新；Rust 的等价分类由 `runtime/dml.rs` 各语句分支在调用 `append` 时显式给出。

Rust 表层另有更贴近 Go 包装器的一份移植 `pkg/table/mview_log.rs`，其中 `validate_meta` 与 `MLogDMLType` 被本文件直接复用。其独立测试是 `pkg/table/mview_log_test.rs`；会话层端到端语义则由 `pkg/session/dml_runtime_test.rs` 验证。扩展时应同时核对这三层，避免 Go 包装器、Rust 表层契约和 Rust session mutation 管线发生分类或元数据规则漂移。

## 扩展指南

- 新增或改变日志系统列：先更新 `astersql-meta-model` 常量与 DDL 生成，再修改 `pkg/table/mview_log.rs::validate_meta` 和本文件 `append` 的投影；同步 `pkg/table/mview_log_test.rs` 与 `pkg/session/dml_runtime_test.rs`。列顺序是持久化兼容约束，不能只按名称随意插入。
- 新增 DML 入口：在其基础表 mutation 路径开始处调用 `for_table`，并将日志 mutation 放入相同向量/事务。INSERT 使用 `I/1`，DELETE 使用 `D/-1`，逻辑 UPDATE 必须成对写 `U/-1` 与 `U/1`；REPLACE 或冲突更新应对照 Go 的 `addRecordDMLType`、`removeRecordDMLType` 决定分类。
- 改变更新过滤：修改 `tracked_changed` 时需覆盖未跟踪列、`NULL`、缺失键和主键变化。若上游改为 touched-column 位图，应与 `pkg/table/mview_log.rs::should_log_update` 保持保守边界一致。
- 支持分区表前必须设计物理表 ID、分区索引、日志归属和 purge/refresh 读取语义；不能只删除 `for_table` 的拒绝分支。现有拒绝回归位于 `mlog_rejects_import_into_and_partitioned_base_dml`。
- 优化批量写入时可考虑批量分配 MLog row ID，但必须保持每条日志唯一句柄、索引 mutation 完整、错误传播和事务共存性。性能风险集中在逐行 auto-ID 分配、HashMap 构造及每条日志的索引编码。
- Rust 单元测试应继续放在独立文件中：表层规则放 `pkg/table/mview_log_test.rs`，session 端到端路径放 `pkg/session/dml_runtime_test.rs`，不要内嵌到 `mlog.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；`files --filter pkg/session/runtime/mlog.rs` 与 `node --file ...` 确认文件共 6 个图节点、由 `pkg/session/runtime.rs` 使用；`query RuntimeMLog` 定位结构体、父模块导入和 `runtime/dml.rs` 的调用实例。
- 目标与装配源码：`pkg/session/runtime/mlog.rs`（`RuntimeMLog`、`for_table`、`tracked_changed`、`append`），`pkg/session/runtime.rs`（模块声明与导入），`pkg/session/runtime/dml.rs`（INSERT/UPDATE/DELETE、UPDATE JOIN、外键级联及 mutation 提交调用点）。
- crate 边界：`pkg/session/Cargo.toml` 的 `[package]`、`[package.metadata.porting]` 和 `astersql-kv`、`astersql-meta-model`、`astersql-table`、`astersql-types` 依赖。`pkg/session` 没有 `doc.go`，因此包级契约以 `runtime.rs` 顶部说明和 Cargo 元数据为最近依据。
- Rust 直接契约与测试：`pkg/table/mview_log.rs`（`MLogDMLType::as_str`、`validate_meta`、Go 风格包装器），`pkg/table/mview_log_test.rs`；`pkg/session/dml_runtime_test.rs::go_merge_49_sql_mlog_dml_shares_transaction`、`mlog_load_data_uses_insert_mutation_pipeline`、`mlog_rejects_import_into_and_partitioned_base_dml`。
- Go 对照：`pkg/table/tables/mview_log.go` 的 `WrapTableWithMaterializedViewLog`、`mlogTable`、`shouldLogUpdate`、DML 分类和 `writeMLogRow`，以及 `pkg/executor/builder.go::wrapTableWithMLogIfExists`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，要求文件存在且固定二级标题恰好 11 个；另人工检查只有本说明和完成后删除的任务文件属于本会话范围。

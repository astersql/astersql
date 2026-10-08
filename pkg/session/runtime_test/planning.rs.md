# `pkg/session/runtime_test/planning.rs`

## 文件定位

本文件是 `astersql-session` crate 的规划链路集成测试模块，不是生产请求路径。`pkg/session/lib.rs` 仅在 `#[cfg(test)]` 下装入 `runtime_test.rs`，后者再通过 `#[path = "runtime_test/planning.rs"] mod planning` 接入本文件。因此这里的八个 `#[test]` 只在测试构建中运行；生产实现主要位于 `pkg/session/runtime/planning.rs`、`pkg/session/runtime/relational_scan.rs` 和会话分发代码。

文件以 `use super::*` 继承 `runtime_test.rs` 的 `ConcreteTestRuntime`、`ConcreteSession`、`kv`、`infoschema`、`Arc` 等测试上下文。它没有类型、trait、常量、`impl` 或条件编译项；共有九个私有测试函数、两个私有 InfoSchema 夹具函数，以及一个供兄弟测试模块使用的 `pub(crate)` KV 行编码辅助函数。

## 核心职责

本文件从会话边界验证“解析 SQL—构建逻辑计划—物理优化—生成/执行 KV 请求—返回或解释计划”的关键契约：

- `statement_boundary_clears_scalar_subquery_registry` 与 `statement_boundary_copies_tikv_short_circuit_expression_switch` 检查每条语句开始时重置 `StmtCtx` 的状态隔离。
- `concrete_runtime_rejects_unbound_or_full_executor_sql` 检查预编译参数数量、受支持的常量查询和非法表目标边界。
- 两个 `strict_t_multi_*` 测试用真实 mock KV 快照覆盖索引、排序、Limit/Offset、过滤与聚合执行，并检查 `SET_VAR` hint 的语句级生命周期。
- `filtered_count_dag_is_built_from_the_canonical_go_aligned_plan` 解码 TiKV protobuf DAG，验证过滤 COUNT 的下推结构和无符号列元数据。
- 最后三个 EXPLAIN 回归分别固定根 `Limit`、强制索引按索引名匹配，以及常量投影仍可由复合索引范围覆盖。

这些断言约束的是生产实现，而不是在本文件中重新实现规划器。失败时应沿具体入口追踪 `ConcreteSession`、planner 或 executor，不应把测试夹具当作替代实现。

## 主要符号

- `statement_boundary_clears_scalar_subquery_registry()`：先向 `SessionVars` 注册旧的标量子查询，再执行 `SELECT 1`，断言下一语句结束后注册表为空。直接对应生产侧 `StatementContextRuntime::reset_statement_context` 中的 `RestoreScalarSubQueries(Vec::new())`。
- `statement_boundary_copies_tikv_short_circuit_expression_switch()`：分别把 `tidb_enable_tikv_short_circuit_expression` 设为 ON/OFF，触发下一条语句重置，断言会话变量和 `StmtCtx` 镜像一致。生产赋值在 `pkg/session/runtime/planning.rs` 的 `reset_statement_context`。
- `concrete_runtime_rejects_unbound_or_full_executor_sql()`：通过 `ConcreteTestRuntime` 引导 mock store，验证参数不足的 prepared INSERT 报错、常量 `SELECT 1` 与 `VERSION()` 成功、非目标表 INSERT 报错。
- `strict_t_multi_info_schema()` / `strict_t_multi_info_schema_with_index(bool)`：构造表 ID 88 的 `t_multi(a BIGINT, b VARCHAR, c VARCHAR)` 元数据；可选索引 ID 11 `idx_ab_prefix(a,b(15))`，并同时填充 planner 所需 `model_meta`。
- `strict_t_multi_row(handle, a, b, c) -> (kv::Key, Vec<u8>)`：用 `EncodeRowKeyWithHandle` 和 `EncodeRow` 产生表 88 的记录键和值。它是本文件唯一 `pub(crate)` 符号；`runtime_test/typed_adapter_bridge.rs` 还用它构造悲观锁、读写及 adapter 桥接测试数据。
- `strict_t_multi_runs_parser_builder_optimizer_and_kv_executor_with_set_var_lifecycle()`：写入六行 KV，取得快照，调用 `ExecutePlannedKVSelect`，断言三行窗口、扫描六行、正有限成本、规划期 hint 生效、`TopN`/Reader/TableScan 存在且会话值恢复。
- `strict_t_multi_plans_selection_and_count_through_the_canonical_pipeline()`：在无索引 InfoSchema 上执行 `count(*) where a = 1`，断言结果为 3，计划包含 `Selection` 与 `StreamAgg`。
- `filtered_count_dag_is_built_from_the_canonical_go_aligned_plan()`：创建真实注册表，调用 `planned_scalar_count_dag`，解码 `tipb::DagRequest`，断言执行器顺序为 `TableScan -> Selection -> StreamAgg`、只扫描 `user_id`、保留 unsigned flag 且聚合模式为 `Partial1Mode`。
- `forced_index_order_by_limit_keeps_root_limit()`：对分区表和 `idx_c` 执行 EXPLAIN，断言首个根算子仍是 `Limit`。
- `forced_index_hint_matches_index_name_instead_of_a_leading_column()`：构造索引名与列名碰撞，断言 `use index(b)` 产生 `PointGet`，`use index(b_c)` 产生 `IndexLookUp`。
- `constant_projection_is_covered_by_the_selected_index_range()`：断言 `select 1 ... where id = 1 and is_deleted = true` 的 EXPLAIN 含 `IndexRangeScan`。

## 执行流程

测试有三类主流程。

1. 语句边界流程：`concrete_session()` 建立会话，测试预置 `SessionVars`，再经 `execute` 进入正常语句路径。生产侧 `reset_statement_context` 先恢复上一语句 hint，清空标量子查询注册表，重置并重建 `StmtCtx`，复制短路表达式开关，清理 CTE scope、SQL killer 和 warning 状态。本文件在外部观察清理及复制结果。
2. 严格 KV 规划执行流程：测试用 `strict_t_multi_row` 编码数据，经 `Storage::Begin`、`Set`、`Commit` 持久化到 mock store，再取得当前版本快照。`ExecutePlannedKVSelect` 限制为单条 SELECT，注册 MDL，启动 statement hint guard，创建表达式/ranger/build-PB 上下文，经 planner 构建和优化后在传入快照上执行。返回的 `Rows`、`ScannedRows`、`Cost`、`Operators` 和规划期变量快照构成断言面。
3. DDL/EXPLAIN 与 DAG 流程：`CreateAnalyzeSession` 建立带 domain 的会话，通过 SQL 注册库表。COUNT 测试经 `planned_scalar_count_dag` 调用 `NewPlanBuilder().Init`、`buildResultSetNode`、`DoOptimize`，只接受目标表上的 TiKV `PhysicalTableReader`，最终序列化 DAG；其余测试经普通 `execute("explain ...")` 逐行读取根或全部算子名称。

`strict_t_multi_info_schema_with_index(false)` 有意关闭索引，以证明过滤和流式聚合来自规范规划链而非测试手工拼出的索引计划。相反，三个 EXPLAIN 测试有意构造索引边界，精确观察路径选择和算子保留。

## 数据与状态

- 固定测试表 ID 为 88、索引 ID 为 11、列 ID 为 1/2/3；记录键由整数 handle 组成，值按列 ID `[1,2,3]` 编码。修改这些 ID 时必须同时修改 InfoSchema 和编码夹具，否则规划元数据与 KV 数据会错位。
- `t_multi` 的 `b` 列是 `utf8mb4_bin`，索引只取 `b(15)` 前缀。六行数据让 `ORDER BY a,b LIMIT 3 OFFSET 2` 的预期窗口稳定为 `(1,gamma)`, `(2,alpha)`, `(2,beta)`，同时能观察前缀索引排序与 TopN。
- `SessionVars` 是跨语句会话状态，`StmtCtx`、标量子查询注册表、hint 覆盖值是语句级状态。测试的不变量是旧语句状态不能泄漏，而 `SET_VAR` 仅在当前规划期间覆盖并在返回前恢复。
- mock KV 的事务在提交后才创建 snapshot；执行读取的是显式 `kv::Retriever` 快照，因此夹具写入和读取版本边界清晰。
- COUNT DAG 的 protobuf 是一次规划结果快照；断言关注执行器类型、扫描列 flag 和聚合 mode，而不依赖不稳定的计划节点编号。

## 依赖与调用关系

上游接线为 `pkg/session/lib.rs` 的测试门控 → `pkg/session/runtime_test.rs` 的 `mod planning` → 本文件的 Rust test harness 函数。RustCodeGraph 能识别本文件 15 个符号，但文件级报告 `used by 0 files`，没有建出该 `#[path]` 模块边，因此模块入口以源码接线为准。

关键下游边如下：

- 状态边界测试 → `ConcreteSession::execute` → `StatementContextRuntime::reset_statement_context` → `FinishHintStatement`、`RestoreScalarSubQueries`、`StmtCtx::Reset` 和短路开关复制。
- 严格执行测试 → `ConcreteSession::ExecutePlannedKVSelect` → parser、MDL、hint runtime、planner context、`PlanBuilder`/`DoOptimize`、物理算子与 KV retriever。
- COUNT DAG 测试 → `ConcreteSession::planned_scalar_count_dag` → `physical_table_reader` → `planned_table_reader_dag` → `tipb::DagRequest`。
- 行夹具 → `astersql-tablecodec::{EncodeRowKeyWithHandle, EncodeRow}`；其 `pub(crate)` 可见性允许 `runtime_test/typed_adapter_bridge.rs` 复用，但不会成为 crate 的生产 API。

`pkg/session/Cargo.toml` 声明 crate 名 `astersql-session`，生产依赖覆盖 parser、planner、executor、infoschema、KV、mockstore、tablecodec、protobuf 与 `tipb`；`nextgen` feature 只透传配置 feature，本文件没有 feature 分支。测试模块还依赖同 crate 的 mock store/testkit 环境，但本文件自身不启动线程或外部服务。

## 错误处理与边界

测试通过 `expect`/`assert!` 让任何意外错误立即失败，并显式覆盖以下拒绝路径：prepared statement 实参数量不足、目标表不受支持、planned KV 输入不是恰好一条 SELECT、COUNT 无法形成目标 TiKV table reader 时返回 `None`，以及生产侧会话/语句上下文仍被共享或占用时无法重置。

夹具构造中的 `EncodeRow(...).expect(...)` 表示编码失败属于测试环境错误，而非待验证的业务返回值。EXPLAIN 测试避免绑定完整文本计划，只断言关键根算子或算子类别，以降低节点 ID 和展示细节变化造成的脆弱性；但索引选择断言是刻意严格的，因为索引名/前导列混淆正是回归风险。

当前文件没有测试多语句 `ExecutePlannedKVSelect`、非 SELECT、错误 table ID、非 TiKV reader 或 protobuf 序列化失败；这些边界由生产函数返回 `SessionError`/`None`，新增相关行为时应在独立测试文件或本模块新增独立测试，而不是把测试逻辑嵌入生产源文件。

## 并发与资源生命周期

本文件的测试均为同步函数，没有自行生成线程、异步任务或通道。共享对象主要用 `Arc` 表达所有权：domain/store/InfoSchema 可跨组件共享；`ConcreteSession` 内部通过其既有 `Rc`/`Arc`、interior mutability 和原子字段管理状态。语句重置要求会话和 `SessionVars` 可独占修改，生产实现会在共享时返回错误，这也是不能在持有旧计划引用时偷偷重置上下文的安全边界。

KV 资源生命周期是“Begin → 多次 Set → Commit → GetSnapshot → 规划并读取”。测试不保留未提交事务，也不创建后台资源。hint 生命周期由生产 guard 包围一次 `ExecutePlannedKVSelect` 调用；测试在调用前后分别观察基线值和恢复值。`strict_t_multi_row` 的兄弟测试还验证 session owner drop 后释放悲观锁，但该释放行为位于 `runtime_test/typed_adapter_bridge.rs`，不是本文件直接执行的流程。

## 与 Go 版本的对应关系

- 标量子查询隔离与 Go `pkg/session/session.go` 的 retry rebuild 路径一致：Go 在逐条重建历史语句前把 `MapScalarSubQ` 置空，`pkg/session/tidb_test.go::TestScalarSubqueryRegistryTxnReplay` 断言连续重放时注册表大小从 1 变为 0。Rust 测试采用更小的普通语句边界夹具验证同一“不继承上一语句子查询”的不变量，并未复刻完整事务重试场景。
- `strict_t_multi_runs_*` 对齐 `pkg/planner/core/hint_test.go` 的 `TestSetVarPartialOrderedIndexForTopN` 子测试：Go 同样检查 `SET_VAR(tidb_opt_partial_ordered_index_for_topn=...)` 只在语句内生效并恢复，还使用相同 `t_multi` 数据及 `ORDER BY a,b LIMIT 3 OFFSET 2` 结果。Rust 增加了 KV 编码、扫描行数、成本和物理算子断言。
- `constant_projection_is_covered_by_the_selected_index_range` 对齐 `pkg/planner/core/integration_test.go::TestIssue54870` 中 `select 1 from t where id=1 and is_deleted=true` 必须出现 `IndexRangeScan` 的回归意图；Rust 使用简化的非生成列夹具，只覆盖索引覆盖/路径选择，不声称复刻 Go 测试的事务和生成列全部语义。
- 根 `Limit`、索引名碰撞和过滤 COUNT 的 SQL 在检索到的 Go 测试中没有完全同形的一一对应用例；它们验证 Go 规划语义类别（根 Limit 保留、USE INDEX 解析、TableScan/Selection/StreamAgg 下推），但文档不把相似用例冒充精确移植来源。

因此本文件是 Go 语义对齐的 Rust 回归集合，而不是逐行翻译。若 Go 行为变化，应按每个测试的具体不变量判断是否同步，而不是仅比较函数名或完整 EXPLAIN 文本。

## 扩展指南

- 新增语句级状态时，修改生产侧 `reset_statement_context` 后，应在本模块增加“上一语句预置—执行下一语句—检查清除或复制”的独立测试；需明确该状态属于会话级还是语句级。
- 扩展 `ExecutePlannedKVSelect` 的 SQL/算子覆盖时，优先复用 `strict_t_multi_info_schema_with_index` 和 `strict_t_multi_row`，但若列类型、表 ID 或索引布局不同，应创建新的测试辅助函数，避免破坏 `typed_adapter_bridge.rs` 的现有调用者。
- 改动 hint 生命周期时，同时验证“规划期间有效”和“调用返回后恢复”；仅验证最终结果不足以发现状态泄漏。同步参考 Go `hint_test.go` 的当前矩阵。
- 改动 COUNT 下推时，应同时校验结果与 DAG 结构，特别是列裁剪、MySQL unsigned flag、executor 顺序、聚合 mode、TiKV store 类型以及显式事务的 root/union-scan 语义。
- 改动索引 hint 或排序/Limit 规则时，保留按索引名碰撞、分区表、常量投影与覆盖索引等回归维度。性能风险主要是意外引入全表扫描、额外排序或取消提前停止；兼容风险主要是 hint 恢复、MySQL 类型 flag 和 EXPLAIN 根结构变化。
- Rust 单元/集成测试继续放在本测试文件或同目录独立 `*_test.rs`/`runtime_test/*.rs` 中，不要嵌入 `pkg/session/runtime/planning.rs` 等生产文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/session/runtime_test/planning.rs` 确认目标文件及 15 个符号；`node --file ... --offset/--limit` 核对全部 528 行；精确 `query` 确认主要测试函数；`callees` 确认严格 KV 测试指向 `strict_t_multi_info_schema`、`strict_t_multi_row`、`Storage::Begin/GetSnapshot`。常见名解析出现跨文件误配且未建出 `#[path]` 文件边，故调用关系同时以模块入口和生产源码核验。
- Rust 源码：`pkg/session/runtime_test/planning.rs`；接线与共享夹具 `pkg/session/lib.rs`、`pkg/session/runtime_test.rs`；被测实现 `pkg/session/runtime/planning.rs`、`pkg/session/runtime/relational_scan.rs`；辅助函数的兄弟调用者 `pkg/session/runtime_test/typed_adapter_bridge.rs`。
- crate 边界：`pkg/session/Cargo.toml` 的 `[package]`、`[lib]`、`[features]`、`[dependencies]` 与 `[dev-dependencies]`。
- Go 对照：`pkg/session/session.go`、`pkg/session/tidb_test.go::TestScalarSubqueryRegistryTxnReplay`、`pkg/planner/core/hint_test.go` 的 `TestSetVarPartialOrderedIndexForTopN` 子测试、`pkg/planner/core/integration_test.go::TestIssue54870`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核文件角色、符号、状态生命周期、Go 对照和扩展入口均有上述源码证据。

# `pkg/session/runtime/explain_select.rs` 逻辑说明

## 文件定位

`explain_select.rs` 属于 `astersql-session` crate（`pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以私有模块 `mod explain_select;` 编入会话运行时，并通过 `use explain_select::*;` 与同层分派代码共享。它不定义新的会话类型，而是为 `runtime/session.rs` 中的 `ConcreteSession` 补充非 `ANALYZE` 的关系型 `SELECT` EXPLAIN 渲染。`runtime/dispatch.rs:3131` 在事务、MDL、grouping 和 `ONLY_FULL_GROUP_BY` 检查后调用 `explain_relational_select`；写语句和若干特化分支也会先调用本文件的 `explain_partition_integration_plan`。

本文件约 9,164 行，包含 16 个模块级辅助函数和一个大型 `impl ConcreteSession`；没有自定义 `struct`、`enum`、`trait`、常量或 feature gate。唯一条件编译项是 `#[cfg(test)] #[path = "explain_select_test.rs"] mod tests;`，使测试保持在独立文件。`pkg/session` 下未发现 `doc.go`，因此包边界以 `Cargo.toml`、`runtime.rs` 和直接分派入口为准。

## 核心职责

- 在 `explain_relational_select` 中为 `SELECT` 选择正确的 EXPLAIN 路径：需要代价、完整优化规则、CTE/视图/窗口/MPP 时转入 `explain_optimized_relational_select`，窄范围稳定兼容形状则直接构造 `ConcreteRecordSet`。
- 根据 AST、表/索引/分区元数据、统计行数和会话开关，渲染 Point Get、Batch Point Get、Table/Index Reader、Index Lookup/Merge、Join、Sort/TopN、TableDual、PartitionUnion 以及 TiFlash MPP 计划树。
- 为 range/list/hash/key 分区裁剪提供字面量、时间 `EXTRACT`、等值、`IN`/`BETWEEN`、析取与布尔表达式评估辅助，并在静态/动态裁剪下生成不同树形。
- 规范化 TiKV/TiFlash 下推谓词、点查范围、向量排序表达式和矛盾谓词，使兼容输出保留 Go TiDB golden 用例依赖的稳定文本。
- 处理计划隐私脱敏、TiFlash 下推 warning、伪统计/已分析行数、不可见索引、临时表和其他会话可见状态。

## 主要符号

分区与谓词辅助组：

- `extract_partition_value(unit, value, fsp) -> Option<i64>` 把日期/时间字面量按 `YEAR`、`DAY_SECOND`、`MICROSECOND` 等单位折算为可与分区上界比较的整数；不识别的单位返回 `None`。
- `range_extract_partition_names`、`equality_partition_name`、`hash_partition_names`、`list_partition_names` 分别为 range-extract、等值/key/hash 与 list-columns 形状推导分区名；无法安全裁剪时返回 `None` 或显式 `all`，空命中则可渲染 `dual`。
- `comparison_has_null_operand`、`equality_column_value`、`batch_point_handles`、`index_point_ranges`、`disjoint_column_ranges`、`has_disjoint_in_conjunction` 识别 NULL 比较、单列等值、主键 OR 点集、索引点范围、析取区间及不可满足的合取谓词。
- `explain_pushdown_predicate`、`explain_tiflash_predicate_plan` 只渲染已明确支持的简单布尔/标量谓词和单表 TiFlash 计划；复杂表达式保留给正常 planner，不做近似伪造。
- `explain_vector_order_expression`、`vector_order_column`、`derived_table_contradiction_plan` 处理向量 ANN/全扫排序文本与派生表常量/别名矛盾。

`impl ConcreteSession` 的主要方法：

- `explain_relational_select(&self, statement, statement_sql, format) -> SessionResult<ConcreteRecordSet>` 是主入口。它首先调用 `collect_statement_predicate_stats`，再按 format、hint、AST 形状、catalog/统计与会话状态按顺序选择特化渲染或真实优化器。返回顺序是行为的一部分，更窄的兼容分支必须位于通用分支之前。
- `explain_partition_integration_plan` 为分区集成和若干 DML/SELECT golden 形状返回 `Option<ConcreteRecordSet>`；`dispatch.rs:3016,3094` 与主入口内部都会调用它。
- `explain_redact_corpus_plan`/`redact_plan_literals` 以带 `‹...›` 标记的基准树为模板，在 `redact_log=ON` 时仅用 `?` 替换字面量，不改变计划形状。
- `explain_index_merge_order_plan`、`explain_intersection_index_merge_plan`、`ordered_index_merge_union_plan`、`index_merge_intersection_lines`、`static_partition_intersection_lines` 补足 compact optimizer 尚未统一暴露的 IndexMerge 物理候选和分区树形。
- `explain_join_casetest_plan`、`explain_enforce_mpp_casetest_plan`、`outer2inner_compat_plan`、`list_columns_compat_plan` 覆盖需精确保留 Go casetest 树形的局部形状；`outer2inner_compat_plan` 刻意用完整规范化 SQL 匹配，避免影响无关查询。
- `list_partition_selection_lines`、`partition_index_range_lines`、`wrap_partition_sort`、`composite_partition_point_lines`、`handle_partition_point_lines` 组装分区 selection/index range/point-get 子树。

## 执行流程

1. `ConcreteSession::execute` 在 `runtime/dispatch.rs` 解析到非 `ANALYZE` `ast::ExplainStmt`后，确保表查询的隐式事务/read-ts 条件，处理 DML、SHOW 和集合运算特例；对 `SelectStmt` 注册 MDL、校验 grouping，然后调用 `explain_relational_select`。
2. 主入口将原 SQL 小写化，从 AST 识别窗口/子查询，并调用 `collect_statement_predicate_stats` 触发与实际优化一致的谓词列统计收集。
3. 最先处理对输出格式有强约束的路径：`plan_tree` 的 outer-to-inner/脱敏模板，`verbose` MPP 用例，以及多 join key INL hint。`cost_trace` 和通用 `verbose` 必须调用 `explain_optimized_relational_select`，因为文本兼容渲染器没有递归 CostVer2 节点、代价和公式。
4. 其次依次匹配精确 Go golden 形状（join、IndexMerge、MPP、redaction、partition）。这些分支使用完整或紧凑 SQL 指纹，或同时校验 AST/元数据，命中后直接返回稳定计划树。
5. 通用路由从 `FROM` 收集的物理表、TiFlash replica 可用性、MPP/isolated-read 开关、聚合/派生表/视图/CTE/窗口、hint 和 format 决定。需要真实逻辑/物理规则的查询回退到 planner；只有简单单表、点/范围/索引/分区可靠推导时才继续 compact 渲染。
6. 尾部通过 `domain.stats_table`、`stats_context().physical_stats`、索引列和谓词选择 Point Get、IndexReader/Lookup 或 TableReader，将行数、task、access object、范围和分区写入 `ConcreteRecordSet`。静态裁剪会将每个分区展开到 `PartitionUnion`，动态裁剪则保留单个读取器。

## 数据与状态

本文件自身不持有持久化数据，输入为 `ast::SelectStmt`、原 SQL/format、`TableInfo` 和表达式树，输出为 `ConcreteRecordSet` 或安全降级用的 `Option`。计划行主要由 `id`、`estRows`、`task`、`access object` 等列组成；很多兼容分支通过 `explain_plan_tree_rows` 把带树形前缀的文本转成结果集。

会话可见状态从 `self.state.borrow()` 与 `self.session_vars` 读取，包括 `dynamic_partition_prune`、`redact_log`、`isolation_read_engines`、`allow_mpp`/​`IsMPPAllowed()`、统计与当前数据库。某些 TiFlash 下推失败分支通过 `set_warning` 修改会话 warning。表元数据使用 `astersql_meta_model::TableInfo` 的 columns、indices、partition definitions、view 与 `TiFlashReplica`；统计行数来自 Domain 的 stats context，未建立可用统计时保留 `stats:pseudo` 语义。

## 依赖与调用关系

上游主链是 `runtime/dispatch.rs` 的 EXPLAIN 分派：`dispatch.rs:3131` 调用 `explain_relational_select`，`dispatch.rs:3016,3094` 在 DML 和通用非 ANALYZE EXPLAIN 早期调用 `explain_partition_integration_plan`。本文件的模块声明位于 `runtime.rs:58`，同层 `use explain_select::*` 位于 `runtime.rs:151`。

下游通过 `use super::*` 使用 `runtime.rs` 聚合的 parser AST、`ConcreteSession`/`ConcreteRecordSet`/`SessionResult`、Domain/catalog/statistics、关系表达式与优化帮助函数。明确的 crate 依赖包括 `astersql-parser-ast`、`astersql-meta-model`、`astersql-planner-core*`、`astersql-statistics*`、`astersql-sessionctx-vardef`、`astersql-util-redact` 和 `crc32fast`；开放结果还会调用 `crate::dml_runtime::ParseGeneratedExpr`/`EvalExpr` 评估分区生成表达式。真实优化回退通过相邻 EXPLAIN/planning 实现的 `explain_optimized_relational_select`完成。

RustCodeGraph 能定位文件和上述主要函数，但对这个大型 `impl ConcreteSession` 的 `callers`/`callees` 查询未返回静态边；因此本节的具体入口行号又用 `rg` 在 `pkg/session` 模块范围核对，不将索引的“无边”解释为“无调用者”。

## 错误处理与边界

`explain_relational_select` 返回 `SessionResult`：谓词统计收集、catalog/规划和真实优化器错误使用 `?` 传播给分派层。纯识别辅助大多返回 `Option`；`None` 表示“当前窄化规则不能证明可安全渲染”，调用方应继续其他分支或回退到 planner，不是吞掉错误。分区边界解析中的失败会保守地停止裁剪，对不具单调性的范围返回 `all`，避免错误漏分区。

SQL 普通比较遇到 NULL 在顶层 `WHERE` 为 UNKNOWN，`comparison_has_null_operand` 可将该计划化为 `TableDual rows:0`，但 NULL-safe `<=>` 被明确排除。唯一索引只有在每个 key column 都有等值时才可渲染 `Point_Get`；仅前导列等值必须保留为索引范围。视图、CTE、窗口、复合行比较、代价格式、多表复杂连接和需要优化规则的 NULL/聚合语义必须回退真实优化器，不得用占位树代替。

文件中存在多个精确 SQL/casetest 兼容分支。它们是当前代码事实，不应推广为任意同类 SQL 都已完整实现；未命中的形状仍受后续通用规则和 planner 能力约束。

## 并发与资源生命周期

本文件没有启动线程/task，不创建 channel，也不直接持有 KV transaction、锁或长生命资源。它在单条 statement 的会话串行路径内读取 `RefCell` 状态，构造局部 `Vec`、`HashMap`/`HashSet` 和 `String`，返回的 `ConcreteRecordSet` 拥有自己的 columns/rows。临时 borrow 都限制在分支内，例如读取 redaction、dynamic pruning 和 MPP 开关后即释放。

事务/read-ts/MDL 生命周期由上游 `dispatch.rs` 保证，统计/catalog 由 `RuntimeDomain` 共享；本文件只做同步查询和读取。唯一可观察副作用是谓词统计收集和 `set_warning`。新增分支不应在计划渲染中执行数据读写、持有跨 await/跨返回的 borrow，或绕过上游事务与 MDL 边界。

## 与 Go 版本的对应关系

`pkg/session/Cargo.toml` 的 `[package.metadata.porting] go-package = "pkg/session"` 将 crate 边界对应到 Go `pkg/session`，但不存在同路径 `pkg/session/runtime/explain_select.go`。Go 主链是 `pkg/planner/core/planbuilder.go:6025` 的 `PlanBuilder.buildExplain`、`buildExplainPlan`，随后由 `pkg/executor/builder.go:1634` 构建 `ExplainExec`，`pkg/executor/explain.go` 执行/生成输出，`pkg/planner/core/common_plans.go` 的 `Explain.RenderResult` 和各 physical operator `ExplainInfo` 负责格式化。Rust 的 `explain_optimized_relational_select` 回退路径对应这条真实 planner 语义。

本文件额外承担一个迁移期兼容层：对 Go planner/executor 测试已固定、而 compact Rust optimizer 尚未统一暴露的 IndexMerge、特定 join/MPP、partition integration、redaction 和 vector 形状，直接渲染同样的稳定树。源码注释明确要求需要完整优化器的视图、CTE、窗口、复杂连接、CostVer2 和 TiFlash aggregate/derived query 回退真实流程，说明兼容分支不是 Go planner 的简化替代。

直接 Rust 独立测试 `pkg/session/runtime/explain_select_test.rs` 验证了分区写 EXPLAIN 只在悲观事务（或 NextGen 默认）增加 `SelectLock`，并验证 date-only/time-only 字面量在 `extract_partition_value` 中不混淆。Go 对照证据分散在 `pkg/planner/core/casetest/**`、`pkg/planner/core/tests/redact/redact_test.go`、`pkg/planner/core/casetest/partition/integration_partition_test.go`、`pkg/executor/explain*_test.go` 等 golden/行为测试，而非单一同名 Go 测试。

## 扩展指南

新增 EXPLAIN 能力时，先判断它属于通用优化语义还是局部稳定渲染。需要规则改写、代价、完整物理节点、hint warning、复杂表达式或 MPP exchange 的功能应接入 `explain_optimized_relational_select` 及 planner/operator 渲染，不要继续堆叠类似计划的 SQL 字符串特例。只有完整 SQL 指纹且是 Go golden 兼容约束时，才宜增加精确分支，并必须放在会被通用路径吞掉之前。

新增分区裁剪时，从 `range_extract_partition_names`、`equality_partition_name`、`hash_partition_names` 或 `list_partition_names` 扩展，保持“无法证明则不裁剪”的保守性，同时检查 NULL、MAXVALUE、非单调函数、FSP、负时间、组合列和空分区集。新增索引/点查时必须检查 index visibility、covering projection、unique composite key 完整性、排序方向和分区模式。

回归测试优先扩展独立文件 `pkg/session/runtime/explain_select_test.rs`，不将测试内联到生产文件；如果行为对应特定 Go golden，同步核对相应 `pkg/planner/core/casetest/**`/`tests/**` 用例。每个新分支至少应覆盖命中形状、近似但不应命中的 SQL、format 差异、统计/catalog 状态和回退路径。主要正确性风险是分支顺序导致错误截获、过度分区裁剪或计划与实际执行分叉；兼容风险是 Go 计划文本/warning 改变；性能风险是反复规范化 SQL、遍历大量表/索引/分区或过早进入完整优化器。

## 验证依据

本说明直接核对了：生产源 `pkg/session/runtime/explain_select.rs`；crate 边界 `pkg/session/Cargo.toml`；模块入口 `pkg/session/runtime.rs`；上游分派 `pkg/session/runtime/dispatch.rs`；独立 Rust 测试 `pkg/session/runtime/explain_select_test.rs`；Go 主链 `pkg/planner/core/planbuilder.go`、`pkg/planner/core/common_plans.go`、`pkg/executor/builder.go`、`pkg/executor/explain.go`；Go 对照测试入口 `pkg/planner/core/tests/redact/redact_test.go`、`pkg/planner/core/casetest/partition/integration_partition_test.go`、`pkg/planner/core/casetest/planstats/plan_stats_test.go` 和 `pkg/executor/explain*_test.go`。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query explain_select --json` 定位了文件及 `extract_partition_value`、`range_extract_partition_names`、`equality_partition_name`、`hash_partition_names`、`list_partition_names`、`explain_pushdown_predicate` 等符号，`node --file pkg/session/runtime/explain_select.rs` 核对了源码。对 `explain_relational_select`/`explain_partition_integration_plan` 的 callers/callees 因大型 impl 索引限制返回空集，所以用 `rg` 确认了 `runtime.rs:58,151`、`dispatch.rs:3016,3094,3131` 和测试直接调用。

本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的结构命令确认文档存在且恰有 11 个固定二级章节，并人工检查本文能回答文件为何存在、如何运行、何时回退真实优化器以及如何安全扩展。

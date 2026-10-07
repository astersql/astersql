# `pkg/executor/physical_plan_runtime.rs`

## 文件定位

本文件属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/executor/lib.rs` 中的 `pub mod physical_plan_runtime` 对外暴露。它位于规划器产出的规范 `PhysicalPlan` 与已有 Rust 执行器/存储接口之间：会话层在 `pkg/session/runtime/planning.rs` 的 `ExecutePlannedKVSelect` 和 `ExecutePreparedPlannedKVSelect` 中构造 `KVRetrieverTableSource`，再调用 `ExecutePhysicalPlan` 执行优化后的物理树。

这不是完整 Go executor 框架的逐类型重建，而是一条受控的物理计划解释路径。当前可识别 `PhysicalTableReader`、`PhysicalIndexLookUpReader`、`PhysicalTableScan`、`PhysicalUnionScan`、`PhysicalSelection`、`PhysicalStreamAgg`、`PhysicalHashAgg`、`PhysicalTopN`、`PhysicalSort`、`PhysicalLimit` 和 `PhysicalProjection`；其他算子由 `execute_node` 明确拒绝。排序本身委托给 `astersql-executor-sortexec`，KV 行读取和解码委托给 `astersql-kv` 与 `astersql-tablecodec`。

## 核心职责

- `ExecutePhysicalPlan`/`execute_node` 递归解释物理算子树，把 reader 解包到表计划，并把每个已支持节点翻译成扫描、过滤、投影、聚合、排序或限制操作。
- `PhysicalTableSource` 把执行逻辑与数据来源隔离，既允许测试/第三方实现直接返回内存行，也允许真实 KV 实现覆盖流式扫描、提前停止、快速计数和事务本地行来源。
- `KVRetrieverTableSource` 在表记录 key 前缀内迭代 `Retriever`，从记录 key 恢复 handle、从 value 解码列，并按 `PhysicalTableScan.Columns` 组装 `Row`。
- `stream_rows_while` 将父节点的停止信号向下传到扫描，避免 `LIMIT` 在取得足够结果后继续拉取；`execute_streaming_scalar_aggregate` 让标量聚合只保留常量大小的 partial state。
- `merge_union_rows` 合并 snapshot 与事务本地 dirty rows，并在 handle 相同时选择 dirty row。
- TopN/Sort 只负责把计划参数转换成 `SortKey`、构造现有 `TopNExec`/`SortExec` 并排空输出；`ExecuteLimitRows` 同样复用 `ProjectRows` 和 `ExecuteLimitValues`。

## 主要符号

公开边界如下：

- `PhysicalRuntimeError(String)` 与 `PhysicalRuntimeResult<T>`：统一包装本运行时、KV、解码、表达式和 sort executor 的字符串错误；实现 `Display`、`Error`，并可从 `SortError` 转换。
- `PhysicalRowVisitor` 与 `PhysicalRowWhileVisitor`：逐行回调别名；后者以 `bool` 表示继续扫描或正常提前停止。
- `PhysicalTableSource`：要求实现 `Scan`；默认 `ScanRows` 基于完整 `Vec<Row>` 迭代，默认 `ScanRowsWhile` 传播停止信号，默认 `CountRows` 逐行计数，默认 `UnionScanRows` 返回空集。
- `KVRetrieverTableSource<'a>`：借用调用方拥有的 `dyn astersql_kv::Retriever`，并用 `AtomicUsize` 累计实际访问行数；`New` 构造，`ScannedRows` 读取计数。
- `datum_to_sort_value`：把 NULL、整数、浮点、字符串/字节/二进制字面量和 decimal Datum 转成 `SortValue`，拒绝其余 Datum kind。
- `ExecuteLimitRows`：可选先按列下标投影，再调用现有 Limit 执行路径。
- `ExecutePhysicalPlan`：本文件唯一的整棵物理树执行入口。

内部核心包括 `one_child`、`sort_keys`、`evaluate_expression`、`row_matches_conditions`/`filter_rows`、`union_handle_columns`/`compare_union_rows`/`merge_union_rows`、`ScalarAggregateState` 及其 initialize/update/finish 函数、`projection_columns`/`project_row`、`can_stream_rows`、`direct_table_scan`、`is_scalar_count_star`、`stream_rows[_while]` 和 `execute_node`。文件没有条件编译项；`#![allow(non_snake_case)]` 用于保留与 Go 移植 API 一致的命名。

## 执行流程

1. 会话层完成 AST 构建、逻辑计划构建和 `DoOptimize` 后，把物理根节点及 caller-owned `Retriever` 交给 `ExecutePhysicalPlan`（`pkg/session/runtime/planning.rs`）。
2. `execute_node` 先按具体物理类型分派。TableReader/IndexLookUpReader 只解包其 table plan；TableScan 调用 `PhysicalTableSource::Scan`。
3. KV 数据源根据 `PhysicalTableID`（非零时优先）或逻辑表 ID 生成 record prefix，按 `scan.Desc` 选择 `IterReverse` 或 `Iter`。每条记录先 `DecodeRecordKey`，再 `DecodeRowToDatumMap`；`PKIsHandle` 表的主键列从 handle 补回，缺失列以 NULL 输出。
4. Selection 使用计划的表达式上下文逐条件求值，采用短路 AND；NULL 与 false 都丢弃该行。Projection 当前只接受已解析成列引用的表达式，并按列下标复制值。
5. UnionScan 分别过滤 snapshot 与 `UnionScanRows`，按 handle 列和底层表扫描方向排序归并；相同 handle 丢弃 snapshot 行、保留 dirty row。
6. StreamAgg/HashAgg 当前只执行无 GROUP BY 的标量聚合。裸 `COUNT(*)` 且子树是直接表扫描时调用 `CountRows`；可流式的扫描/过滤/投影子树通过 visitor 更新 partial state；其余子树先物化再聚合。
7. TopN/Sort 先递归取得子行，把计划中的列引用 `ByItems` 转为 `SortKey`，随后交给 `TopNExec`/`SortExec`。TopN 参数包含计划的 offset/count；两者通过重复 `Next(1024)` 排空并在结束时 `Close`。
8. Limit 对可流式子树逐行跳过 offset、收集 count，并在收满后返回 `false` 停止底层迭代；不可流式时走 `ExecuteLimitRows`。`count == 0` 直接返回空结果。
9. 无匹配分支时返回带 `plan.tp(&[])` 的 unsupported operator 错误。

## 数据与状态

执行行统一采用 `astersql_executor_sortexec::Row(Vec<SortValue>)`。表达式求值前，`sort_value_to_datum` 将其临时还原为 `Datum` 并建立 `MutRow`；结果再经 `datum_to_sort_value` 回到执行行表示。decimal 当前按其字符串字节保存，而非专用 decimal 排序类型，这是兼容/排序语义需要关注的边界。

`KVRetrieverTableSource` 不拥有存储，仅保存 `&dyn Retriever`。`scanned_rows` 每解码一行（快速 `CountRows` 时每推进一条记录）递增一次，供会话层写入执行统计。计数采用 `fetch_add(Ordering::AcqRel)`，读取采用 `load(Ordering::Acquire)`；它统计累计访问量，不会在一次 `ExecutePhysicalPlan` 前自动清零。

标量聚合状态为 `ScalarAggregateState`：普通 COUNT 保存 `i64`，MAX_COUNT/MIN_COUNT 保存 typed aggregate kernel 与 `PartialResult`，FIRST_ROW 保存 `Option<SortValue>`。FIRST_ROW 的外层 `Option` 区分“尚未见输入”与“首行本身为 SQL NULL”；完成阶段总是输出恰好一行。UnionScan 会同时持有并排序两组 `Vec<Row>`，TopN/Sort 和不可流式分支也会物化子结果，因此其内存并非一律常量。

## 依赖与调用关系

上游调用边由 RustCodeGraph 查询确认：`ExecutePhysicalPlan` 的生产调用者是 `pkg/session/runtime/planning.rs` 中的 `ExecutePlannedKVSelect` 与 `ExecutePreparedPlannedKVSelect`；独立测试 `pkg/executor/physical_plan_runtime_test.rs` 也直接调用它。`ExecuteLimitRows` 还被 `pkg/executor/benchmark_test.rs` 的 Limit 对照路径调用。

主内部调用链为 `ExecutePhysicalPlan -> execute_node`。`execute_node` 递归调用自身，并调用 `source.Scan/CountRows/UnionScanRows`、`filter_rows`、`execute[_streaming]_scalar_aggregate`、`drain_topn`、`drain_sort` 和 `ExecuteLimitRows`。`stream_rows_while` 则为 TableReader、IndexLookUpReader、TableScan、UnionScan、Selection、Projection 建立逐行链路，并最终落到 `PhysicalTableSource::ScanRowsWhile`。

crate 依赖可在 `pkg/executor/Cargo.toml` 核对：本文件直接使用 executor 子 crate `astersql-executor-sortexec`、`astersql-executor-aggfuncs`，以及 planner core/base/physicalop、expression/aggregation/exprctx、KV、tablecodec、types、parser AST/MySQL、planner util 和 util chunk。模块内还复用 `crate::projection::ProjectRows` 与 `crate::select::ExecuteLimitValues`。当前功能不受 `nextgen` feature 条件控制。

## 错误处理与边界

- 结构约束：需要单子节点的算子统一经 `one_child` 校验；reader 缺 table plan、TableScan 缺 `TableInfo` 都返回错误。
- 类型/下标约束：排序项和投影表达式必须是已解析列；负列下标、越界投影列、越界 UnionScan handle 列均报错。`partial_cmp` 无序时，UnionScan 比较退化为相等。
- 数据约束：不支持的 Datum kind 报错；行值缺失的普通列被补为 NULL，`PKIsHandle` 主键则从 handle 恢复。
- 聚合边界：只支持 scalar aggregation；COUNT(DISTINCT)、FIRST_ROW(DISTINCT)、不支持的 mode/函数和不适配的 count-extrema 参数类型会被拒绝。COUNT 与表行数都用 `checked_add` 检测溢出。
- 表达式语义：Selection 的 NULL 条件不保留行；表达式 Eval/ToBool 错误直接传播。FIRST_ROW 不跳过首行 NULL。
- 算子边界：分组聚合、任意表达式投影以及未列入 `execute_node` 的物理算子尚未支持；错误信息明确标出算子类型。
- 资源清理：KV 扫描把循环放在闭包中保存结果，然后无论成功、visitor 提前停止或循环内返回错误都调用 `iterator.Close()`。相比之下，`drain_topn`/`drain_sort` 在 `Next` 返回错误时会由 `?` 立即返回，源码中没有显式的错误路径 `Close`；扩展时应确认底层类型的析构保证或补齐关闭策略。

## 并发与资源生命周期

本适配器自身同步执行物理树，不创建线程、异步任务或通道。它借用 session 提供的 `Retriever`，返回前关闭本次创建的 KV iterator；返回的 `Vec<Row>` 归调用者所有。测试数据源可以只实现物化 `Scan`，真实 KV 数据源覆盖流式接口以缩短 iterator 生命周期和减少物化。

并发相关状态仅有 `KVRetrieverTableSource.scanned_rows`，其原子更新允许共享引用下安全累计。visitor 是 `FnMut` 且生命周期限制在调用栈内，不被保存或跨线程发送。标量流式聚合只持有当前行和与聚合函数数量成正比的 state；流式 Limit 的 `false` 从投影/过滤层一直传播到 `ScanRowsWhile`，使 iterator 在达到数量后关闭。

`TopNExec`/`SortExec` 的内部并发、堆和潜在资源由 `astersql-executor-sortexec` 管理；这里固定以 concurrency `1`、chunk size `1024`、memory limit `-1` 构造。Go 的 Projection/TopN/HashAgg 可包含 worker、channel、内存/磁盘 tracker 等更完整生命周期，本文件没有复制这些机制。

## 与 Go 版本的对应关系

仓库没有同名 `physical_plan_runtime.go`；Rust 文件是一层把多个 Go executor 行为集中解释的适配器，不能视为单个 Go 文件的直译。

- `pkg/executor/select.go` 的 `LimitExec.Next` 按 begin/end/cursor 从 child 拉取并在达到 end 后停止。Rust 的流式 Limit 同样传递停止信号，但以逐行 visitor 实现，未复制 chunk required-rows、自适应控制器和完整 Open/Close 状态机。
- 同文件的 `SelectionExec` 支持向量化和逐行两条路径；Rust `row_matches_conditions` 只走逐行表达式求值，并保持 false/NULL 不选中的 SQL 语义。
- `pkg/executor/projection.go` 的 `ProjectionExec` 支持串行/并行 worker、channel 与内存追踪；Rust 当前只支持列引用投影，不执行任意投影表达式，也不并行。
- `pkg/executor/union_scan.go` 的 `UnionScanExec` 从事务 MemBuffer 构造 added rows，并通过 `getOneRow` 有序合并。Rust 由 `PhysicalTableSource::UnionScanRows` 注入事务本地行，`merge_union_rows` 保留同 handle 脏行覆盖 snapshot 的核心语义，但未复刻虚拟列、分区/global index、MemBuffer 锁定等完整路径。
- `pkg/executor/sortexec/topn.go` 的 `TopNExec` 管理并发 worker、spill、内存/磁盘 tracker 和 result channel。Rust 适配层复用 Rust `TopNExec`，自身只转换排序键与 limit，并未声称覆盖 Go 的全部资源治理。
- `pkg/executor/aggregate/agg_stream_executor.go` 与 `agg_hash_executor.go` 使用 partial results、chunk 和资源 tracker。Rust 仅覆盖无 GROUP BY 的 COUNT、FIRST_ROW、MAX_COUNT/MIN_COUNT，并在可流式子树上逐行更新状态。

这些差异说明当前实现服务于已接线的计划型 KV SELECT 窄路径；扩展算子时应以相应 Go executor 的实际 Next/Open/Close、错误和资源语义为基准，而不是只让一个简单结果用例通过。

## 扩展指南

- 新增物理算子时，先在 `execute_node` 增加类型分派；若该算子能保持逐行语义，同时更新 `can_stream_rows` 和 `stream_rows_while`，保证 Limit/标量聚合仍能向下游背压。需要单子节点时复用 `one_child`。
- 扩展 Projection 时优先复用 expression 的 Eval/type context，明确 NULL、类型转换、副作用函数和列宽错误；不要把任意表达式误当作列下标。同步扩展独立的 `pkg/executor/physical_plan_runtime_test.rs`，不要把测试嵌入本文件。
- 扩展聚合时在 `ScalarAggregateState`、`initialize_scalar_aggregate`、`update_scalar_aggregate`、`finish_scalar_aggregate` 四处保持一一对应，并核对 Complete/Partial1/Final/Partial2/Dedup、空输入、NULL、溢出和 DISTINCT 语义。带 GROUP BY 的实现需要新的分组状态与内存治理，不能复用当前“恰好输出一行”的不变量。
- 扩展 KV Datum 支持时同时审查 `datum_to_sort_value` 与 `sort_value_to_datum` 的往返语义、比较顺序和精度；时间、JSON、decimal 等类型不能仅用字符串替代而假设排序等价。
- 增强 UnionScan 时应接入真实事务本地数据来源，处理删除标记、虚拟列、分区/global index 与锁生命周期，并以 `pkg/executor/union_scan.go` 的 `UnionScanExec` 为行为对照。
- 改动扫描或 Limit 时必须保留 iterator 的无条件关闭与提前停止传播；测试至少覆盖错误路径、offset、count=0、过滤后取满和正/反向扫描。TopN/Sort 错误路径还应验证 executor 能否得到关闭。
- 性能风险主要来自意外退回 `Vec<Row>` 物化、重复 Datum/SortValue 转换、UnionScan 全量排序以及固定单并发。兼容风险主要来自扩大支持范围时与 Go 的 NULL、collation、decimal/time 比较、聚合 mode 或事务可见性不一致。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph：`status` 显示索引含目标文件；`query physical_plan_runtime` 找到 `ExecutePhysicalPlan`、`PhysicalTableSource`、`ScanRowsWhile`、`CountRows`、`projection_columns` 等符号；`node --file pkg/executor/physical_plan_runtime.rs` 阅读了 1–1086 行；`explore "ExecutePhysicalPlan PhysicalTableSource KVRetrieverTableSource ExecuteLimitRows"` 给出 `ExecutePhysicalPlan -> execute_node -> ExecuteLimitRows`，并列出 session 与测试调用者。
- 源与 crate 边界：`pkg/executor/physical_plan_runtime.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`。
- 生产入口：`pkg/session/runtime/planning.rs` 中 `ExecutePlannedKVSelect` 和 `ExecutePreparedPlannedKVSelect`。
- Rust 独立测试：`pkg/executor/physical_plan_runtime_test.rs` 的 `strict_t_multi_uses_physical_tree_and_real_topn_executor`、`limit_stops_kv_scan_after_enough_filtered_rows`、`cached_table_scalar_count_executes_canonical_union_scan_tree`、`union_scan_merges_by_handle_and_dirty_rows_shadow_snapshot_rows`、三组 FIRST_ROW 测试、`scalar_count_streams_large_input_without_materializing_rows` 与 `scalar_count_extrema_uses_typed_kernel_for_nulls_and_duplicates`。这些测试分别固定了表前缀、存储变更可见性、提前停止、dirty-row 覆盖、空/NULL/首行、快速计数和 typed kernel 行为。
- Go 对照：`pkg/executor/select.go`、`pkg/executor/projection.go`、`pkg/executor/union_scan.go`、`pkg/executor/sortexec/topn.go`、`pkg/executor/aggregate/agg_stream_executor.go`、`pkg/executor/aggregate/agg_hash_executor.go`。

本任务是纯文档分析，按计划不运行 Cargo；文档只陈述上述源码与测试可验证的当前行为。未验证的范围包括真实 TiKV 上的端到端事务可见性、Go/Rust 全算子等价性，以及 sort executor 在 `Next` 错误时的隐式析构清理保证。

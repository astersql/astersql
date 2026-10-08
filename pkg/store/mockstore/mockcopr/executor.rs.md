# `pkg/store/mockstore/mockcopr/executor.rs`

源码链接：[executor.rs](./executor.rs)。

## 文件定位

本文件属于 `astersql-store-mockstore-mockcopr` crate 的内部 `executor` 模块。crate 入口 `pkg/store/mockstore/mockcopr/lib.rs` 将它声明为私有模块，但 `cop_handler_dag.rs` 会直接使用其中的 `executor` trait 和五类执行器，因而它是 mock TiKV Coprocessor DAG 的行式执行核心，而不是 SQL 层通用执行器。

真实请求链从 `coprHandler::handleCopDAGRequest` 开始：`buildDAGExecutor` 校验请求并建立 `dagContext`，`buildDAG` 按 `DagRequest.executors` 从扫描叶子向根部逐个调用 `buildExec`，再通过 `executor::SetSrcExec` 串成拉取链。根执行器随后被反复调用 `Next`，直至返回 `Ok(None)`；扫描计数和执行明细分别通过 `Counts`、`ExecDetails` 写回响应。上述入口与组装逻辑位于 `pkg/store/mockstore/mockcopr/cop_handler_dag.rs:138-325`。

`pkg/store/mockstore/mockcopr/Cargo.toml` 指定本 crate 的 `[lib] path = "lib.rs"`，并用 `package.metadata.porting.go-package = "pkg/store/mockstore/mockcopr"` 标明 Go 对照包。当前文件本身仅依赖同 crate 的 `copr_handler` 与 `topn` 模块；Cargo 中大量可选 TiDB 子 crate 依赖并未被本文件直接引用。

## 核心职责

1. 用 `executor` trait 统一扫描、过滤、TopN 和 Limit 的上游链接、逐行拉取、扫描计数及执行统计接口（`executor.rs:34-41`）。
2. 用 `tableScanExec`/`indexScanExec` 从 `KvReader` 按 `KeyRange`、`start_ts` 和方向读取可见行，并按区间累计已返回行数（`executor.rs:78-245`）。
3. 用 `selectionExec` 实现多个条件的 SQL AND 过滤：任一表达式为 NULL 或假即丢弃当前行（`executor.rs:247-317`）。
4. 用 `topNExec` 计算 ORDER BY 键、在容量受限的堆中保留最优 N 行，耗尽输入后按顺序输出（`executor.rs:319-415`；堆细节在 `topn.rs:90-161`）。
5. 用 `limitExec` 在产出指定行数后停止向上游拉取（`executor.rs:417-478`）。
6. 提供行列辅助函数 `hasColVal`、`getRowData` 和 `convertToExprs`，服务于列存在性检查、按列 ID 投影及表达式所有权转换（`executor.rs:480-515`）。

这是测试用 mock 执行模型，不应把它描述成完整 TiKV/TiDB 执行引擎。尤其是 Rust 扫描一次性把 `KvReader::scan` 的全部结果装入内存，行数据已经是 `Vec<Datum>`；它没有复刻 Go 版逐 KV 游标推进、rowcodec 解码、隔离级别和 resolved-lock 参数。

## 主要符号

- `RowValue = Row`、`NextRow = Option<RowValue>`：分别是行值和“有一行/流结束”的返回形状。错误独立放在 `Result<NextRow, CopError>` 中。
- `trait executor: Send`：对象安全的执行器协议。`SetSrcExec`/`GetSrcExec` 管理单一上游，`ResetCounts`/`Counts` 管理扫描统计，`Next` 拉取一行，`ExecDetails` 返回叶到根的明细序列。trait 要求实现可跨线程移动，但没有要求 `Sync`。
- `update_detail`、`update_result_detail`：累计耗时、调用次数和产出行数。后者在错误结果上也按一次 iteration 记账，以复刻 Go `defer` 行为。
- `append_details`：先取上游明细，再追加本算子的 `ExecDetail`，保持 `DAGRequest.Executors` 的“子在前、父在后”顺序。
- `tableScanExec`：保存共享 `Arc<dyn KvReader>`、范围、读时间戳、方向、惰性缓存、每行所属范围、游标、计数和本算子统计。`new` 只初始化；`load` 才真正扫描。
- `indexScanExec`：组合一个 `tableScanExec`，所有执行行为委托给 `inner`；额外字段 `unique` 及 `isUnique` 仅保存/查询唯一索引属性。
- `selectionExec` 与 `evalBool`：循环拉取上游并逐条件求值。条件为空时自然返回真；NULL 直接视为不匹配。
- `topNExec`：保存排序项、容量、上游、最终结果队列、一次性执行标志和临时 `topNHeap`。`innerNext` 拉一行，`evalTopN` 计算排序键并尝试入堆。
- `limitExec`：保存最大产出数和当前产出游标；只有拿到 `Some(row)` 才推进游标。
- `hasColVal`：只有列 ID 存在、偏移有效且值不是 `Datum::Null` 时才为真。
- `getRowData`：按请求的列 ID 顺序投影；列 ID `-1` 返回传入的 handle，其他列从 `column_ids` 指向的 `values` 槽位克隆。
- `convertToExprs`：把 `&[Expr]` 克隆为拥有所有权的 `Vec<Expr>`，没有解析或校验表达式。

文件没有模块级常量、条件编译项或 `unsafe` 代码。所有执行器类型和多数状态字段为 `pub`，但模块本身未由 `lib.rs` 重导出；主要使用方仍是同 crate 的 DAG 构建器与测试。

## 执行流程

请求侧流程如下：

1. `coprHandler::buildDAGExecutor` 从请求取出 ranges、`start_ts` 和执行器规范；空 ranges、非 DAG 载荷或空执行器列表立即返回 `CopError::InvalidRequest`。
2. `buildDAG` 依照规范数组顺序迭代。`buildExec` 要求 TableScan/IndexScan 必须是叶子；后续 Selection、聚合、TopN、Limit 通过 `SetSrcExec` 接住前一个执行器（`cop_handler_dag.rs:202-267`）。
3. `handleCopDAGRequest` 对最终根节点循环调用 `Next`。每个非扫描算子按需向自己的 `src` 拉取，所以控制从根向叶传播，数据从叶向根返回。
4. 执行结束或报错后，根节点透传叶扫描的 `Counts`，并在请求要求 execution summaries 时收集整个链的 `ExecDetails`。

各算子的具体流程：

- TableScan：首次 `Next` 调用 `load`；`KvReader::scan(ranges, start_ts, descending)` 一次返回所有 `KvPair`。每行进入 `rows` 时同步记录其所属 range 下标。之后每次 `Next` 从两个 `VecDeque` 头部弹出，命中有效 range 时增加该槽计数，并在产出行时增加 `cursor`。缓存排空后持续返回 `None`。
- IndexScan：构造时把相同扫描参数交给 `tableScanExec::new`；`Next`、计数和明细均直接委托。当前 `unique` 不会改变点查或范围扫描策略。
- Selection：循环调用上游 `Next`。遇到上游结束就结束；遇到一行则由 `evalBool` 按顺序求值，首个 NULL/假值短路丢弃，全部为真才返回原行。表达式错误立即停止循环并向上传播。
- TopN：第一次 `Next` 初始化容量为 `limit` 的 `topNHeap`，随后耗尽整个上游。每行对所有 `ByItem.expr` 求值得到排序键；堆未满时加入，满时仅用更优行替换当前最差行。输入结束后 `intoSortedRows` 按 ORDER BY（含 descending）排序，并转成输出队列。当前及后续调用每次弹出一行；`limit == 0` 时仍会耗尽上游，但堆拒绝所有行。
- Limit：若 `cursor >= limit`，不触碰上游便返回结束；否则拉取一次。只有实际返回一行才增加 `cursor`，上游结束不会虚增计数。

每个 `Next` 都在入口记录 `Instant`，并在成功、结束或错误后调用 `update_result_detail`。因此 `iterations` 表示调用次数，不等于产出行数；Selection/TopN 的一次外部调用可能包含多次上游调用。

## 数据与状态

- 执行器树是由 `Box<dyn executor>` 形成的单上游所有权链。根节点拥有父算子，父算子逐层拥有其 source；没有共享可变执行器节点。
- `tableScanExec.loaded` 保证每个实例最多调用一次 `KvReader::scan`。`rows` 与私有的 `row_range_indexes` 必须保持同序；只有产出行时两者各弹出一个元素并据此更新 `counts`。
- `tableScanExec.counts` 在构造时固定为 ranges 长度。`ResetCounts` 将全部槽位清零，但不会重置 `loaded`、`cursor` 或已经消费的队列，因此它表示“从此刻重新累计后续返回行”，不是重放扫描。
- `tableScanExec.cursor` 是总产出行数；它不表示当前 range 下标。范围归属由 `row_range_indexes` 单独保存。
- `indexScanExec.unique` 是元数据状态；Rust 当前没有像 Go `indexScanExec.Next` 那样用“point range 且 unique”选择点查路径。
- `topNExec.working_heap` 只在第一次执行期间存在，排序结果生成后通过 `take()` 释放并变成 `None`；`executed` 防止再次读取上游，`rows` 保存尚未返回的结果。
- `limitExec.cursor` 只记录已成功返回的行数。所有非扫描算子的 `Counts`/`ResetCounts` 都传递给上游扫描节点。
- `ExecDetail` 为每个算子独立拥有，`ExecDetails` 返回克隆后的快照；`append_details` 不会把内部统计对象暴露给调用者。
- `Datum`、`Expr`、`ByItem`、`KeyRange`、`KvReader`、`Row`、`CopError`、`ExecDetail` 定义在 `copr_handler.rs`。其中 `MemoryReader::scan` 负责半开范围校验、按 `start_ts` 过滤提交版本并处理升降序，执行器只消费其结果。

## 依赖与调用关系

上游直接调用者和接线点：

- `coprHandler::buildTableScan` → `tableScanExec::new`；`buildIndexScan` → `indexScanExec::new`；`buildSelection` → `selectionExec::new`；`buildTopN` → `topNExec::new`；Limit 在 `buildExec` 中直接调用 `limitExec::new`。
- `coprHandler::buildExec` → `executor::SetSrcExec`，并阻止扫描算子带 source。
- `coprHandler::handleCopDAGRequest` → 根节点 `Next`、`Counts`、`ExecDetails`。
- `aggregate.rs` 的 HashAgg/StreamAgg 也实现相同 `executor` trait，因此可插在本文件的扫描与 TopN/Limit 之间；它们不是本文件的实现范围。

下游依赖：

- 所有扫描读取最终落到 `KvReader::scan`。`reader` 用 `Arc` 共享，但读取接口接收 `&self`。
- Selection 和 TopN 调用 `Expr::eval`；Selection 再调用 `Datum::truthy`。
- TopN 调用 `topNHeap::new`、`tryToAddRow`、`intoSortedRows`。实际比较规则在 `topn.rs::compare_rows`：逐 ORDER BY 键比较，descending 时反转，全部相等则相等。
- 错误统一使用 `CopError`，调用者 `handleCopDAGRequest` 会停止拉取，并由 `buildResp` 区分 Locked、Region 和 other error。

RustCodeGraph 的文件级导航确认 `executor.rs` 被索引为 74 个符号；精确源码查询确认上述构造和驱动边位于 `cop_handler_dag.rs`。由于仓库中存在大量同名 `executor`/`Next`，宽泛 callers/callees 查询会混入 SQL 执行层及 Go 符号，结论以 `--file` 源码节点和同目录构建器的直接调用为准。

## 错误处理与边界

- 扫描错误：`tableScanExec::load` 用 `?` 原样传播 `KvReader::scan` 的 `CopError`，且只有成功后才置 `loaded = true`。因此失败后再次 `Next` 会重新尝试扫描；失败调用仍计入本算子的 iteration。
- 缺失 source：Selection、TopN、Limit 分别返回带有明确文本的 `CopError::InvalidRequest`。构建器正常接线时不会发生，但直接构造执行器后调用 `Next` 会触发该防线。
- 表达式错误：Selection 的任一条件或 TopN 的任一排序表达式求值失败都会立即传播，当前行及后续行不再处理。
- TopN 初始化不变量：`evalTopN` 在 `working_heap` 缺失时返回 `InvalidRequest`；正常 `Next` 会先初始化。排序阶段的 `expect("top-n heap was initialized")` 依赖同一内部不变量，外部 API 无法在正常路径中打破它。
- 范围计数：无法匹配任一 `KeyRange` 的行仍会被返回，但其 `row_range_indexes` 为 `None`，不会增加任何 range 计数。这依赖 reader 通常只返回请求范围内的行。
- `getRowData`：`-1` 是 handle 特例。普通列 ID 缺失、映射偏移越界都会返回 `CopError::ColumnOffset`；列 ID 完全缺失时错误携带 `usize::MAX`。它不会像 Go 版那样补默认值、NOT NULL 错误或处理新旧 rowcodec。
- `hasColVal` 对不存在列、越界偏移和 NULL 都返回 false，不会 panic；该边界由 `executor_test.rs::has_col_val_rejects_null_and_invalid_offsets` 覆盖。
- 空条件 Selection 接受所有行；Limit 0 不拉取上游；TopN 0 会拉空上游后返回空结果。二者虽然最终行数相同，但资源消耗语义不同。
- 执行器没有取消上下文。Go 的 `Next(ctx context.Context)` 接受 context，Rust trait 没有对应参数；长扫描/TopN 不能在本层响应取消。

## 并发与资源生命周期

`executor: Send` 允许把整棵执行器树转移到另一线程，但 `Next(&mut self)`、内部 `VecDeque`、计数器和堆都要求独占可变借用，所以单个执行器实例不能被并发拉取。trait 未要求 `Sync`，也没有锁或内部并发任务。

扫描 reader 是 `Arc<dyn KvReader>`，而 `KvReader: Send + Sync`，因此底层 reader 可以被多个扫描执行器安全共享。执行器自身的所有缓存和统计仍归各实例独占。

资源生命周期由所有权自然管理：Box 链随根执行器析构而递归释放；扫描缓存随 `tableScanExec` 释放；TopN 临时堆在首次执行完成时被 `take` 并转换为结果队列。没有显式 `Close`、后台线程、通道、事务或锁守卫。注意 TopN 在首次输出前必须消费全部上游，扫描也会在首次请求时把全部匹配行载入内存，因此两者叠加时峰值内存可能同时包含扫描缓存和 TopN 堆/结果。

`ResetCounts` 只影响统计，不恢复执行状态；`executed`、`cursor`、队列和 `loaded` 均不会复位。若需要重跑 DAG，应重新构建执行器树，而不是复用已耗尽实例。

## 与 Go 版本的对应关系

主要结构一一对应 `pkg/store/mockstore/mockcopr/executor.go` 中的 `executor`、`execDetail`、`tableScanExec`、`indexScanExec`、`selectionExec`、`topNExec`、`limitExec`、`evalBool`、`hasColVal`、`getRowData` 和 `convertToExprs`。共同语义包括：拉取式单行接口、子执行器统计在前、错误调用也更新执行明细、Selection 的 NULL/假短路、TopN 先消费输入再输出、Limit 只统计实际行，以及计数沿非扫描算子向叶子透传。

当前 Rust 是有意可见的简化移植，不能据名称推断完全等价：

- Go TableScan/IndexScan 按 range 和 `seekKey` 每次读取一个 KV，区分 point/range、隔离级别和 resolved locks，并现场解码行/索引；Rust 委托 `KvReader::scan` 一次加载所有 `Row`。Go `ResetCounts` 维护 `start` 窗口，Rust 清零整个定长计数向量。
- Go IndexScan 只有 unique point range 才走点查；Rust `indexScanExec` 始终复用同一批量扫描，`unique` 仅可查询。
- Go Selection/TopN 先按 related column offsets 解码求值行，并使用 session expression context；Rust `Expr` 直接对已解码的 `Datum` 行求值。
- Go TopN 的 heap 在构建器中预置；Rust 在第一次 `Next` 才创建。两者都在输出前消费完整输入，但内部数据形状不同。
- Go `getRowData` 处理新旧 rowcodec、PK handle 的有符号性、默认值、NULL 和 NOT NULL 缺列错误；Rust 版本只是列 ID 投影与 `-1` handle 替换。
- Go `convertToExprs` 调用 `expression.PBToExpr`，可能失败；Rust 版本仅克隆已经构造好的 `Expr`，不会产生错误。
- Go `Next` 接收 `context.Context`；Rust 无取消参数。

独立 Rust 测试 `pkg/store/mockstore/mockcopr/executor_test.rs` 明确验证了两项 Go 对齐语义：扫描计数只随已返回行增加且可清零，以及失败的 `Next` 也增加 iteration。其 `test_resolved_large_txn_locks` 还以底层 MVCC store 验证大事务次级锁解析后旧版本可见、主锁仍存活，对照 Go `executor_test.go::TestResolvedLargeTxnLocks`；不过该测试不是对本文件所有算子的全面等价证明。

## 扩展指南

- 新增 DAG 算子时，应在独立生产文件实现 `executor` trait，在 `cop_handler_dag.rs::buildExec` 增加规范到实现的构造分支，并保持 `SetSrcExec`、`Counts` 和 `ExecDetails` 的链式约定。测试必须放在独立 `*_test.rs`，不要嵌入本源文件。
- 修改扫描行为时，优先保持 `rows` 与 `row_range_indexes` 的一一对应、半开区间 `[start, end)` 归属、`start_ts`/descending 透传，以及“返回时才计数”的不变量。同步扩展 `executor_test.rs::table_scan_counts_only_rows_already_returned`，并与 Go 的 point/range、unique、倒序和空 end 行为逐项核对。
- 增强 IndexScan 时，最可能修改 `indexScanExec::Next` 或拆分其点查/范围逻辑；必须验证 `unique` 的真实分支，而不能只保留字段。兼容风险是计数窗口、范围游标和键边界与 Go 不一致。
- 增强表达式语义时，修改 `evalBool`、`selectionExec::Next`、`topNExec::evalTopN` 及 `copr_handler::Expr/Datum`；应覆盖 NULL、类型转换错误、多条件短路、升降序、多键和相等键。若引入 session/SQL mode/时区，应从 DAG context 显式传入，避免隐式全局状态。
- 修改 TopN 时，同时检查 `topn.rs::topNHeap`；重点风险是 `limit == 0`、堆顶“最差行”方向、descending 反转、稳定性、表达式错误和首次输出前的内存占用。
- 修改 `getRowData` 时，应先决定是否要逼近 Go rowcodec 行为；默认值、NOT NULL、unsigned handle 和格式探测不可用简单投影代替。相应测试应新增到 `executor_test.rs`，并以 `executor.go:616-663` 为语义基线。
- 若需要执行器可重用，应新增明确的完整 reset 生命周期，而不是扩张现有 `ResetCounts` 的含义；改变该方法会影响响应统计契约。
- 性能方面，扫描全量缓存和 TopN 全量消费是首要风险。任何流式化改动都要保持错误调用计数、range counts 和响应顺序，并用独立测试验证失败中途的状态。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/mockcopr` 列出 27 个 Go/Rust 文件，目标 `executor.rs` 有 74 个符号。
- 目标源码：`pkg/store/mockstore/mockcopr/executor.rs:21-515`，用于核对全部类型、trait、函数、impl、状态和错误路径。
- 直接入口与调用边：`pkg/store/mockstore/mockcopr/cop_handler_dag.rs:138-325`，确认 `handleCopDAGRequest → buildDAGExecutor → buildDAG → buildExec`、各构造函数、`SetSrcExec`、根 `Next/Counts/ExecDetails`。
- 下游类型与读取契约：`pkg/store/mockstore/mockcopr/copr_handler.rs:27-260`，确认 `CopError`、`Datum`、`KeyRange`、`KvReader`、`MemoryReader` 和 MVCC 可见性简化；`pkg/store/mockstore/mockcopr/topn.rs:21-195`，确认比较、容量堆和最终排序。
- crate 边界：`pkg/store/mockstore/mockcopr/Cargo.toml` 与 `lib.rs:18-54`，确认 crate 名、Go 包映射、私有模块装配和独立测试模块。
- Go 对照：`pkg/store/mockstore/mockcopr/executor.go:39-675`，用于核对接口、扫描游标、Selection、TopN、Limit、行解码及表达式转换的共同点与差异。
- Rust 独立测试：`pkg/store/mockstore/mockcopr/executor_test.rs:30-209`，覆盖列存在性、已返回行计数、失败 iteration 及大事务锁解析；DAG 请求入口的空 range 校验见 `cop_handler_dag_test.rs:23-43`。
- Go 独立测试：`pkg/store/mockstore/mockcopr/executor_test.go:42-122`，确认 `TestResolvedLargeTxnLocks` 的端到端 SQL、BatchGet、PointGet 和主锁存活意图。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定的结构命令验证文档存在且恰有 11 个固定二级标题，并人工复核未把上述 Rust 简化实现描述成完整 Go 等价实现。

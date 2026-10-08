# `pkg/store/mockstore/unistore/cophandler/mpp_exec.rs`

## 文件定位

本文件属于 crate `astersql-store-mockstore-unistore-cophandler`。同目录 `Cargo.toml` 以 `[lib] path = "lib.rs"` 指定 crate 根，`lib.rs` 通过 `pub mod mpp_exec` 导出本模块，并在 `#[cfg(test)]` 下把独立的 `mpp_exec_test.rs` 挂入测试。它位于 UniStore mock coprocessor 的本地执行层：接收 `cop_handler.rs::Executor` 表示的已解析执行计划和 `KvReader`，递归执行扫描及关系算子，返回仓库内自定义的 `ExecutionOutput`。

RustCodeGraph 的文件查询显示目标文件共 606 行、40 个符号，并给出 `cop_handler.rs`、`mpp.rs`、`closure_exec.rs` 和测试文件等使用方。生产入口分别是普通 DAG 的 `cop_handler.rs::handle_dag`、受限计划的 `closure_exec.rs::ClosureExecutor::execute`，以及带 MPP 上下文的 `mpp.rs::MppExecBuilder::build_and_execute`。因此，本文件既为普通 mock cop 请求提供执行内核，也为 MPP 非 Exchange 子树提供共享执行内核；真正的 MPP tunnel 收发不在这里，而在 `mpp.rs`。

## 核心职责

1. `execute_executor` 按 `Executor` 枚举递归执行 Table/Index Scan、Selection、Limit、TopN、Projection、Expand、Aggregation、Join 和无上下文的 ExchangeSender。
2. `ExecutionOutput` 聚合结果行、逐 range 行数、索引 NDV、逐算子执行摘要、Exchange 中间分区以及扫描明细，使上层可统一组装 `Response`。
3. `scan` 把 `KvReader::scan` 返回的可见 KV 行按列偏移投影，并为每个 range 计算计数；索引扫描额外计算投影行的精确去重数。
4. `aggregate`、`AggState` 和 `join` 提供 mock 所需的分组聚合与单键哈希连接语义，并保留若干已由独立测试锁定的 Go 行为。
5. `MppExec`/`MaterializedExec` 提供最小的 `open/next/stop` 拉取生命周期；`ExchangeBuffer` 提供按 task id 发送、一次性取走的进程内行批缓冲。

这些能力是仓库自定义 `Datum`/`Row`/`Executor` 上的同步、物化 mock，不是 Go 文件中基于 `chunk.Chunk`、tipb、真实 codec、session context 和并发 tunnel 的完整执行器体系。尤其是 `ExchangeReceiver` 在 `execute_executor` 中明确拒绝执行，只有 `MppExecBuilder` 附带 `MppContext` 时才可通过 tunnel 处理。

## 主要符号

- `BATCH_SIZE: usize = 1024`：`MaterializedExec::next` 单次最多返回的行数。它不等同于 Go `DefaultBatchSize = 32`。
- `ExecutionOutput`：公开输出结构。`rows` 是当前算子的最终行；`range_counts` 和 `ndvs` 来自叶子扫描；`summaries` 按递归执行过程追加；`intermediate` 保存 ExchangeSender 的分区行；`scan_detail` 记录扫描版本数和字节数。
- `ExecutionOutput::map_rows`：私有变换辅助函数，保留子树已有元数据，追加当前算子耗时/产出行数摘要，再替换 `rows`。
- `MppExec`：公开拉取式 trait，定义 `open`、`next`、`stop`、`execution_summary`。与 Go `mppExec` 同名合同相比，它没有子节点、字段类型、中间结果和 scan detail 访问器。
- `MaterializedExec`：公开的已物化行执行器。状态由 `rows`、`cursor`、`opened` 和 `started` 组成；`new` 不自动打开。
- `execute_executor`：公开的递归总入口，参数为 reader、ranges、MVCC `start_ts` 与计划根，返回 `Result<ExecutionOutput, CopError>`。
- `executor_output_width`：私有 schema 宽度推导器，用于右侧结果为空时仍能给 LeftOuter Join 补正确数量的 NULL；`ExchangeReceiver` 无法推导时返回 `None`。
- `scan`：私有叶子执行函数，逐 range 调 `KvReader::scan`，投影列并更新 `ScanDetail`、计数和 NDV。
- `AggState`：私有聚合状态机，覆盖 `Count`、`Sum`、`Min`、`Max`、`First`；`new/update/finish` 管理其生命周期。
- `sum_datum`、`aggregate`、`update_states`、`finish_group`：分别负责 SUM 类型规则、Hash/Stream 分组、逐行更新和“聚合列在前、分组列在后”的结果布局。
- `join`：以右表建 `BTreeMap<Datum, Vec<&Row>>`、左表探测的单键连接，支持 `Inner`、`LeftOuter`、`Semi`、`AntiSemi`。
- `ExchangeBuffer`：公开的 `HashMap<i64, Vec<Vec<Row>>>` 包装；`send` 追加行批，`receive` 删除 task 条目并展平返回。当前源码搜索未发现它被本 crate 的生产路径调用。

## 执行流程

普通 DAG 主链如下：

1. `cop_handler.rs::handle_dag` 从 `DagRequest.root` 取根节点，或用 `executor_list_to_tree` 组装树，然后调用 `execute_executor(reader, ranges, start_ts, root)`。
2. `execute_executor` 对叶子 `TableScan`/`IndexScan` 调 `scan`。`scan` 逐个 range 调 `reader.scan(..., descending)`，按 `columns` 投影 `KvPair.value`，记录每个 range 的结果数；索引扫描用 `BTreeSet<Row>` 计算 NDV。
3. 一元算子先递归得到子输出。Selection 对每行求条件并仅保留 `truthy`；Limit 截断前 N 行；TopN 把所有子行交给 `TopNHeap` 后取排序结果；Projection 逐表达式生成新行；Expand 按每个 level 复制行并把缺失/非法位置变为 `Datum::Null`；Aggregation 进入 `aggregate`。
4. HashAgg 用 `BTreeMap` 存储状态，但另记 `group_keys` 保持分组首次出现顺序。StreamAgg 假定输入已经按分组键相邻排列，键切换即结束上一组；本函数不验证全局有序性。无分组列且输入为空时，两种模式都输出一个由空聚合状态结束得到的行。
5. Join 递归执行左右子树，合并右子树摘要与 scan detail，然后由 `join` 在右侧建表、左侧探测。LeftOuter 未命中时根据计划推导的右侧 schema 宽度补 NULL。
6. 无 MPP 上下文的 ExchangeSender 递归执行子树；存在 `partition_keys` 时按求值后的键用 `BTreeMap` 分组，将分区复制到 `intermediate`，并把各分区按键排序后的顺序重新展平为 `rows`。空分区键时行保持不变。ExchangeReceiver 返回 `CopError::Unsupported`。
7. 每个非叶子分支经 `map_rows` 或显式追加方式记录当前层摘要；上层 `handle_dag` 再做输出列投影、分页，并通过 `response_from_output` 组装响应。

其他两条入口复用相同内核：`ClosureExecutor::execute` 仅允许 `closure_supported` 的计划形态，并可在执行后清空 range count/NDV；`MppExecBuilder::build_and_execute` 在取消检查后自行处理 ExchangeReceiver/ExchangeSender tunnel，其他节点委托本文件的 `execute_executor`。

## 数据与状态

`execute_executor` 是同步递归函数，没有模块级可变状态。除 scan 借用 reader/ranges、计划节点借用 `Executor` 外，各层输出都由当前调用拥有。实现会把扫描结果和中间算子结果完整物化为 `Vec<Row>`；Projection、Expand、Join、Exchange 分区还会克隆行或 Datum，因此峰值内存会随输入行数、行宽、Expand level 数、Join 匹配倍数和分区数量增长。

`ExecutionOutput` 的元数据传播并非所有字段都在每个二叉分支对称合并：Join 显式合并左右 `summaries` 与 `scan_detail`，但保留左侧的 `range_counts`、`ndvs`、`intermediate`；这一事实不应被描述成完整合并两边所有统计。其他一元算子直接沿用子输出，只替换行并增加摘要。

HashAgg 的 `groups` 采用有序映射是为了能以 `Vec<Datum>` 为键，但实际输出顺序由 `group_keys` 决定。StreamAgg 只保留当前键及状态，聚合状态数与 `calls` 数一致。`AggState::Count` 忽略 NULL；Sum/Min/Max 忽略 NULL；First 记录第一个值，即使它是 NULL。输出行由所有聚合结果后接分组键组成。

Join 哈希表不插入右侧 NULL 键；左侧 NULL 求值后通常不会命中，从而按 join 类型丢弃、补 NULL 或作为 AntiSemi 输出。右侧同键多行会产生多条 Inner/LeftOuter 结果。哈希结构实际使用 `BTreeMap`，因此依赖 `Datum` 的全序，而不是标准库 `HashMap`。

`MaterializedExec` 的 cursor 在重复 `open` 时不会重置；`stop` 只把 `opened` 设为 false，也不清空数据或计时起点。`ExchangeBuffer::receive` 具有消费语义：删除 task id 后再次接收返回空向量；它没有容量限制或生产者/消费者同步机制。

## 依赖与调用关系

已核对的生产上游调用边为：

- `cop_handler.rs::handle_dag` → `execute_executor` → `response_from_output`，服务普通 DAG cop 请求。
- `closure_exec.rs::ClosureExecutor::execute` → `execute_executor`，随后按 `collect_range_counts` 决定是否保留 range count/NDV，并记录根级 `ExecDetail`。
- `mpp.rs::MppExecBuilder::build_and_execute` → `execute_executor`（非 Exchange 节点）；Exchange 节点由 builder 的 `send`/`receive` 走 `MppTaskHandler` 和 `ExchangerTunnel`。

本文件内部主要调用边为：

- `execute_executor` → 自身递归，并分别 → `scan`、`TopNHeap::{new,add_data_row,into_sorted_rows}`、`Expr::eval`、`aggregate`、`join`、`executor_output_width`、`ExecutionOutput::map_rows`。
- `scan` → `KvReader::scan`、`ScanDetail::record`、`duration_summary`。
- `aggregate` → `AggState::new`、`update_states`、`finish_group`；`update_states` → `Expr::eval`、`AggState::update`；Sum 状态 → `sum_datum`。
- `join` → 两个 key expression 的 `Expr::eval`。

类型依赖来自同 crate 的 `cop_handler`（计划、表达式、行、reader、错误、摘要、扫描详情）和 `topn::TopNHeap`，集合及计时来自标准库。虽然 crate 的 `Cargo.toml` 声明了许多可选的 AsterSQL 子 crate，本文件没有直接导入它们；不能据此推断它使用了真实 TiDB expression、chunk、codec 或 KV 客户端实现。

测试调用边包括 `mpp_exec_test.rs` 对 `execute_executor` 的四项 Go 语义回归，以及 `cop_handler_test.rs` 对扫描、Selection、TopN、MaterializedExec 和请求入口的覆盖。测试保持在独立文件中，符合本仓库禁止把 Rust 单元测试内嵌到生产源文件的约定。

## 错误处理与边界

- 所有 reader、表达式和 TopN 错误均通过 `?` 作为 `CopError` 传播；`handle_cop_request`/`handle_mpp_dag_request` 再把非锁错误写入 `Response.other_error`。
- `MaterializedExec::next` 在未 `open` 时返回 `CopError::InvalidRequest("executor is not open")`；已耗尽时返回 `Ok(None)`。`stop` 当前不会失败。
- `scan` 的任意列偏移超界都返回 `CopError::ColumnOffset(offset)`。它逐 range 推进，出错时丢弃尚未返回的局部输出；没有外部写副作用可回滚。
- Selection 只保留 `Datum::truthy()` 为真的行；false 与 NULL 均被过滤。表达式求值错误不会被当作 false，而会终止执行。
- Expand 对 `None` 或超界 offset 都产生 NULL，不报列偏移错误；空 `levels` 会把所有输入行展开为零行。
- `sum_datum` 只接受同型 Int/Uint/Real，以及非负 Int 与 Uint 的混合；整数使用饱和加法，Real 直接相加，其他组合返回 `CopError::Type`。空输入的无分组聚合仍产生一行，Count 为 0，其余状态为 NULL。
- StreamAgg 的正确性依赖调用者提供按 group key 排序且相同键连续的输入；函数不验证此前提，非连续同键会被输出成多个组。
- LeftOuter 的空右输入依赖 `executor_output_width(right)` 推导补 NULL 数。若右子树最终是 schema 未携带的 ExchangeReceiver，则宽度为 `None`，回退到实际首行宽度或 0；在本函数中 ExchangeReceiver 本身也无法执行。
- Join 只支持单个左右 key expression，没有额外 join condition。NULL build key 被排除；Semi/AntiSemi 只返回左行。
- 无 MPP 上下文的 ExchangeSender 分区使用 `BTreeMap<Vec<Datum>, ...>` 的键序，而 `mpp.rs` 的真实 Hash Exchange 使用 FNV-1a 后按 tunnel 数取模；两条路径不应视为相同的网络分区协议。
- `ExchangeBuffer` 没有错误返回、背压、等待、关闭或取消语义；它不是 `ExchangerTunnel` 的替代品。

## 并发与资源生命周期

本文件本身没有线程、异步任务、锁或系统 I/O；执行调用在当前线程同步完成。`KvReader` 是借用的 trait object，实际 MVCC 可见性和底层并发由 reader 实现负责。所有中间 `Vec`、映射、集合及引用哈希表都在递归调用结束时释放；Join 哈希表中的 `&Row` 只借用本次调用的右侧切片，不会逃逸。

`MaterializedExec` 是显式生命周期对象：调用方应按 `new → open → 若干 next → stop` 使用。当前类型没有内部同步，也没有声明可被多个线程并发推进；cursor 和 opened 都要求 `&mut self`，Rust 借用规则阻止同一实例的普通并发调用。执行摘要从最近一次 `open` 记录的 `Instant` 计算耗时，并以 cursor 作为已吐出行数。

`ExchangeBuffer` 同样要求 `&mut self` 才能 send/receive；如果外层需要跨线程共享，必须自行添加互斥和唤醒机制。真实 MPP 并发资源位于 `mpp.rs`：`MppTaskHandler` 管理 tunnel，builder 检查取消，sender/receiver 连接、发包、收包和关闭。文档或扩展不能把本文件简单内存缓冲的生命周期套用到 tunnel。

资源风险主要是全量物化和克隆：TopN 先接收全部子行，HashAgg 保存所有分组状态，Join 同时持有左右输出与右侧哈希，Expand/多匹配 Join 可能放大结果，ExchangeSender 同时保留 `rows` 与克隆后的 `intermediate`。这里没有流控或 spill，适用目标是 mock 数据规模。

## 与 Go 版本的对应关系

Rust `MppExec` 对应 Go `mppExec` 的核心 `open/next/stop` 概念，`MaterializedExec` 提供最小拉取实现；`execute_executor` 各枚举分支则把 Go 中分散在 `tableScanExec`、`indexScanExec`、`limitExec`、`expandExec`、`topNExec`、`exchSenderExec`、`exchRecvExec`、`joinExec`、`aggExec`、`selExec`、`projExec` 的行为集中成递归、物化执行。

当前明确保留并有 Rust 测试证据的局部 Go 语义包括：HashAgg 结果把聚合列放在分组列之前；分组按首次出现顺序输出，而非按编码键排序；LeftOuter 即使右侧实际结果为空也按右计划 schema 宽度补 NULL；父 Join 合并左右扫描详情。`cop_handler_test.rs` 还覆盖点查范围、Selection、TopN 和 `MaterializedExec` 生命周期。Go `cop_handler_test.go::TestMppExecutor` 则验证 TableScan → Selection → Limit 的 MPP 执行及 range count。

两种实现仍有显著差异：

- Go 使用 `chunk.Chunk`、field type、session/statement context、tipb 和真实 table/index codec，并以默认 32 行 chunk 拉取；Rust 使用已解码的 `Vec<Datum>`，递归分支大多一次物化整棵子树，只有 `MaterializedExec` 按 1024 行切批。
- Go `mppExec` 还暴露 children、字段类型、中间结果、摘要与 scan detail；Rust trait 只保留四个基础方法，`ExecutionOutput` 另行携带统计。
- Go 扫描器处理 key/value 解码、MVCC scanner 和更多扫描详情；Rust `scan` 直接投影 `KvPair.value`，IndexScan 的 NDV 是投影行精确去重。
- Go Expand 维护 grouping set 范围并附加 grouping ID；Rust `Executor::Expand.levels` 直接描述每个输出列的可选源 offset，语义模型更简化。
- Go Join 支持 build/probe side、类型对齐以及左右外连接路径；Rust 固定右表 build、左表 probe，只覆盖 `Inner/LeftOuter/Semi/AntiSemi` 的单键版本。
- Go 聚合复用正式 aggregation evaluator 和编码 group key；Rust 仅覆盖五种 `AggKind`，以 `Vec<Datum>` 分组，并另外提供基于有序输入的 StreamAgg 分支。
- Go Exchange 通过 tunnel、连接通道、取消 context、tipb chunk 和 hash codec 传输；本文件只生成内存分区或返回 Unsupported，实际 Rust tunnel 逻辑在 `mpp.rs::MppExecBuilder`。
- Go 有 `indexLookUpExec`，本文件的 `Executor`/`execute_executor` 没有对应分支。

所以本文件应被视为用于 AsterSQL Rust mock 路径的局部行为移植，不能把测试覆盖的几个对齐点外推成与 Go MPP 引擎完全等价。

## 扩展指南

- 新增 `Executor` 变体时，应在 `execute_executor` 的穷尽匹配中接线，并同步检查 `executor_output_width` 是否需要推导新 schema；否则 LeftOuter 空右侧补 NULL 可能静默得到错误宽度。回归测试应放在独立的 `mpp_exec_test.rs` 或最接近入口的测试文件。
- 修改扫描时，应同时验证 ascending/descending、多 range、列越界、range count、IndexScan NDV 和 `ScanDetail`。若要接入真实 index/table codec，应先明确 `KvReader` 与 `KvPair` 边界，不能仅在 `scan` 中猜测 key/value 格式。
- 扩展聚合时，应在 `AggKind`、`AggState::{new,update,finish}` 和结果类型规则之间保持一一对应；同步验证 NULL、空输入、无 group by、首次出现顺序及 StreamAgg 有序输入合同。若追求 Go 类型语义，需要引入明确的类型/statement context 设计，而不是继续扩大 `sum_datum` 的隐式组合。
- 扩展 Join 时，应同时更新 schema 宽度推导、NULL key 行为、输出列次序和空 build side。多键或额外条件最好形成明确的计划字段及独立测试，不应把多值临时拼成不稳定的字符串键。
- 修改 Exchange 时，先区分普通 `execute_executor` 的内存分区与 `mpp.rs::MppExecBuilder` 的 tunnel 协议。任何网络分区、取消、关闭或错误传播行为都应在 builder/tunnel 侧实现并在 `mpp_test.rs` 验证；`ExchangeBuffer` 若继续保留，应避免被误用成并发队列。
- 修改 `MaterializedExec` 生命周期时，应明确重复 open 是否重置 cursor、stop 后 next 的错误、摘要起点和批大小，并扩展 `cop_handler_test.rs::MaterializedExec_and_handle_cop_request_smoke`；测试不要写回生产文件。
- 性能优化的重点是避免不必要克隆、使 TopN/聚合/Join 可增量处理，以及为大结果提供流控或 spill；优化必须保留表达式错误出现顺序、首次分组顺序、摘要/scan detail 传播和 LeftOuter schema 不变量。
- 若修改 Rust 生产代码，按仓库协议同步独立测试、与 Go 局部语义核对，完成后运行 `cargo fmt --all`；本次任务仅撰写说明文档，没有改动运行时代码。

## 验证依据

- RustCodeGraph 索引：`status` 显示 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/unistore/cophandler` 列出目标、模块入口、Go 对照及测试；`node --file pkg/store/mockstore/unistore/cophandler/mpp_exec.rs --offset 1 --limit 500` 与 `--offset 497 --limit 180` 覆盖完整 606 行并报告 40 个符号及 6 个使用文件；`query` 唯一定位 `execute_executor`、`ExchangeBuffer` 和 trait `MppExec`。精确 `callers/callees` 查询在本地索引上 30 秒超时且无输出，因此调用边另由已索引文件使用关系和直接源码引用交叉核对，未把超时命令当成成功证据。
- Rust 源码：`pkg/store/mockstore/unistore/cophandler/mpp_exec.rs`，核对所有公开/私有符号、递归分派、扫描、聚合、Join、生命周期与边界。
- crate 与入口：`pkg/store/mockstore/unistore/cophandler/Cargo.toml`、`lib.rs`、`cop_handler.rs::handle_dag`、`closure_exec.rs::ClosureExecutor::execute`、`mpp.rs::MppExecBuilder::build_and_execute`。
- Rust 测试：`mpp_exec_test.rs` 验证聚合列序、首次分组顺序、空右侧外连接宽度和双侧 scan detail；`cop_handler_test.rs` 验证点查、Selection、TopN、MaterializedExec 及普通请求入口；`mpp_test.rs` 覆盖 builder/tunnel 侧 Exchange。
- Go 对照：`mpp_exec.go` 的 `mppExec`、各 scan/limit/expand/topN/exchange/join/agg/selection/projection 实现，`mpp.go::mppExecBuilder` 构建关系，以及 `cop_handler_test.go::TestMppExecutor`。这些证据用于说明局部语义和明确差异，没有声称未实现的协议等价。
- 本任务为纯文档分析，按计划不运行 Cargo，也不修改 Rust、Go、Cargo 或只读的 `plan.md`。交付前运行任务指定的 11 章节结构检查，并人工确认本文回答了文件为何存在、如何运行、状态/错误/资源边界及安全扩展位置。

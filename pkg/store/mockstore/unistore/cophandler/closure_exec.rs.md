# `pkg/store/mockstore/unistore/cophandler/closure_exec.rs`

## 文件定位

本说明对应源文件 [`closure_exec.rs`](./closure_exec.rs)。该文件属于 Cargo crate `astersql-store-mockstore-unistore-cophandler`，由同目录 `lib.rs` 以 `pub mod closure_exec` 导出。它为 UniStore mock coprocessor 保存一组“扫描 KV、用闭包式处理器逐条消费、最后汇总”的执行抽象，并提供一个受限计划形态的 `ClosureExecutor` 门面。其直接类型依赖来自同 crate 的 `cop_handler.rs`（请求、行、表达式、KV 读取和锁错误）、`mpp_exec.rs`（当前实际执行树实现）与 `topn.rs`（有界 TopN 堆）。

当前接线状态必须与设计意图分开理解：`ClosureExecutor::execute` 会调用 `mpp_exec::execute_executor`，但 `cop_handler.rs` 的 DAG 请求入口也直接调用 `execute_executor`，仓库内生产 Rust 代码没有构造 `ClosureExecutor`。源码搜索只找到 `closure_exec_test.rs` 对 `ClosureExecutor::is_point_get_range` 的直接使用，以及 `cop_handler_test.rs` 对 `exceed_end_key` 的使用。因此本文件目前同时包含可复用 API、Go 移植语义的骨架和部分尚未接入请求主链的处理器；不能把所有公开符号都视为线上活跃路径。

## 核心职责

- `ClosureExecutor` 保存读取器、扫描范围、MVCC `start_ts`、已解析锁、计划根节点和范围统计开关；构造时用 `closure_supported` 拒绝非线性或不受支持的计划形态。
- `ClosureExecutor::execute` 将受限计划交给通用的 `execute_executor` 执行，按 `collect_range_counts` 决定是否保留 `range_counts`/`ndvs`，并记录一条根级 `ExecDetail`。
- `ClosureExecutor::check_range_locks` 在 `[start, end)` 范围内枚举调用方提供的锁，并把命中的锁交给 `cop_handler::check_lock` 做时间戳和 resolved-lock 判断。
- `ClosureProcessor` 定义逐 KV 的 `process` 与扫描结束后的 `finish`；`CountStarProcessor`、`CountColumnProcessor`、`TableScanProcessor`、`SelectionProcessor`、`TopNProcessor` 和 `HashCountProcessor` 实现常见特化处理。
- `run_processor` 是处理器驱动器；`get_executor_list`、`get_scan_executor`、`chunks_from_rows`、`safe_copy`、`is_resolved`、`exceed_end_key` 是计划遍历、结果封装及 Go 兼容辅助函数。

## 主要符号

- `CHUNK_MAX_ROWS: usize = 1024`：声明与 Go `chunkMaxRows` 相同的语义常量；本文件自身没有用它分块。实际 `chunks_from_rows` 委托 `cop_handler::append_row`，其块大小由该模块的 `ROWS_PER_CHUNK` 决定。
- `ScanType::{Table, Index}`：描述表扫描或索引扫描；当前文件内没有字段使用该枚举，属于保留的公开迁移接口。
- `ExecDetail { time_processed, num_iterations, num_produced_rows }`：执行统计；`update(started, produced)` 累加耗时、调用次数和本次行数，`ClosureExecutor::execute` 则直接追加一条根级记录。
- `ClosureExecutor<'a>`：借用 `&'a dyn KvReader`，拥有 ranges、root 与 resolved-lock 列表。`new(...) -> Result<Self, CopError>` 校验计划，`execute()` 执行，`is_point_get_range()` 判断前缀后继区间，`check_range_locks()` 检查范围锁。
- `ClosureProcessor`：对象安全的状态处理 trait。`skip_value` 默认返回 `false`，只有 `CountStarProcessor` 覆盖为 `true`；但 `run_processor` 当前仍通过 `KvReader::scan` 取得完整 `KvPair`，并不读取 `skip_value`，所以该优化标志尚未接入扫描层。
- `CountStarProcessor`：每个 pair 将 `count` 加一，`finish` 产生单行 `[Datum::Uint(count)]`。
- `CountColumnProcessor`：用列偏移读取 `pair.value`，仅对非 `Datum::Null` 计数；越界返回 `CopError::ColumnOffset`。
- `TableScanProcessor`：按 `columns` 投影并缓存行；任一偏移越界即失败，`finish` 通过 `mem::take` 转移并清空缓存。
- `SelectionProcessor<P>`：先求值 `condition`，只有 `truthy()` 为真才转交子处理器；`finish` 直接委托子处理器。
- `TopNProcessor`：逐行克隆 `pair.value` 送入 `TopNHeap`；结束时用配置相同的空堆替换旧堆，再消费旧堆并输出排序行。
- `HashCountProcessor`：对每条行求全部 `group_by` 表达式，以 `Vec<Datum>` 为 `BTreeMap` 键计数；结束时按键的确定性顺序输出“分组键列 + count”。
- `run_processor(...)`：一次取得所有扫描结果，顺序调用 `process`，最后调用一次 `finish`。
- `get_executor_list`：递归沿单子节点算子向下访问，输出子节点在前、父节点在后的列表；除 closure 支持的节点外，还能遍历 `Projection`、`Expand` 和 `ExchangeSender`。
- `get_scan_executor`：在上述列表中查找首个 `TableScan`/`IndexScan`，找不到时返回 `InvalidRequest`。
- `prefix_next`：从尾部处理进位；遇到非 `0xff` 字节就加一并把其后缀清零，全为 `0xff` 时追加 `0`。它只由 `is_point_get_range` 使用，是私有函数。

## 执行流程

1. 调用方若选择本门面，先调用 `ClosureExecutor::new`。`closure_supported` 从根向叶递归，只接受扫描叶，以及包裹其上的 `Selection`、`Limit`、`TopN`、`Aggregation` 单子链；`Projection`、`Expand`、`Join`、`ExchangeSender` 和 `ExchangeReceiver` 会在这里被拒绝。
2. 锁检查不是 `execute` 的隐式步骤。调用方必须另行调用 `check_range_locks(locks)`；方法按 executor 的每个 range 过滤锁，范围是下界包含、非空上界排除，然后调用 `check_lock`。
3. `execute` 记录开始时间，调用 `execute_executor(reader, ranges, start_ts, root)`。真正的扫描、投影、过滤、Limit、TopN 和聚合逻辑位于 `mpp_exec.rs`；本文件的各 `ClosureProcessor` 不参与这条路径。
4. 通用执行器返回 `ExecutionOutput` 后，若 `collect_range_counts == false`，本门面清空 `range_counts` 与 `ndvs`，但保留 rows、summaries、scan_detail 和 intermediate。
5. `execute` 追加一条根级 `ExecDetail`：迭代次数固定为 1，产出数取最终 `output.rows.len()`，随后返回输出。

独立的处理器路径是另一条尚未与 `ClosureExecutor::execute` 连接的流程：调用者组装具体 `ClosureProcessor`，`run_processor` 调用 `KvReader::scan` 取得所有可见 KV，逐条 `process`，遇错立即返回且不调用 `finish`，全部成功后才以 `finish` 生成最终行集。包装器如 `SelectionProcessor` 可以将过滤逻辑叠在其他处理器之上。

## 数据与状态

- `ClosureExecutor` 借用 reader，但拥有 ranges、resolved locks 和执行树，因此其生命周期不能超过 reader；`details` 会在每次成功执行后增长，不会自动重置。
- `start_ts` 是扫描可见性与锁可见性的共同输入。`MemoryReader` 等 `KvReader` 实现负责只返回对该时间戳可见的版本；本文件不自行解析 MVCC 版本。
- range 使用 `[start, end)` 约定，空 `end` 表示无上界。`is_point_get_range` 只比较 `end == prefix_next(start)`，不检查 Go 版本中的扫描唯一性、主键形态等额外条件。
- 处理器都是可变状态机：count 处理器累加计数，table scan 缓存行，TopN 维护有界堆，hash count 维护分组映射。`finish` 对 table scan、TopN 和 hash count 使用 move/`mem::take`，所以成功结束后内部结果状态被清空或替换；计数处理器则保留计数，再次运行同一实例会继续累计。
- `HashCountProcessor` 使用 `BTreeMap` 而非哈希表，因此输出按 `Vec<Datum>` 的排序顺序稳定；这与 Go `hashAggProcessor` 保存首次出现的 `groupKeys` 顺序不同。
- `KvPair.commit_ts` 在本文件的 processor 路径未被读取；它仍由读取器提供，供其他执行路径统计或解码使用。

## 依赖与调用关系

上游边界：同目录 `lib.rs` 公开模块；独立测试 `closure_exec_test.rs` 调用 `ClosureExecutor::is_point_get_range`；`cop_handler_test.rs::TestClosureExecutor` 实际调用的是 `mpp_exec::execute_executor`，只从本文件调用 `exceed_end_key`。全仓库 Rust 源码搜索没有发现生产调用方构造 `ClosureExecutor`、调用 `run_processor` 或实例化各 processor。

下游关系：

- `ClosureExecutor::new -> closure_supported`：计划形态验证。
- `ClosureExecutor::execute -> mpp_exec::execute_executor -> KvReader::scan/TopNHeap/aggregate`：当前实际执行链。
- `ClosureExecutor::check_range_locks -> cop_handler::check_lock`：锁可见性错误转换为 `CopError::Locked`。
- `run_processor -> KvReader::scan -> ClosureProcessor::{process, finish}`：独立的逐 KV 驱动链。
- `SelectionProcessor::process -> Expr::eval -> Datum::truthy`；表达式错误原样传播。
- `TopNProcessor -> TopNHeap::{add_data_row, into_sorted_rows}`；排序表达式或类型错误从堆实现传播。
- `chunks_from_rows -> cop_handler::append_row`：把行按 cophandler 的分块规则封装为 `Chunk`。

Cargo 清单把大批 TiDB 子 crate 声明为 `optional = true`，但没有定义 feature，也没有为本文件增加第三方依赖；目标代码目前只使用 crate 内部模块和 Rust 标准库。该 crate 的 `[package.metadata.porting]` 明确 Go 对照包是 `pkg/store/mockstore/unistore/cophandler`。

## 错误处理与边界

- 构造阶段遇到不支持的计划形态，返回 `CopError::Unsupported("closure executor shape")`；不会部分构造 executor。
- 列投影与 `COUNT(column)` 的偏移越界返回 `CopError::ColumnOffset(offset)`。Selection、TopN、group-by 表达式以及 reader 扫描错误使用 `?` 向上传播。
- `get_scan_executor` 找不到扫描节点时返回 `CopError::InvalidRequest("scan executor not found")`。`get_executor_list` 对二叉 `Join` 和无子节点的 `ExchangeReceiver` 不递归；这适合它的单链辅助定位用途，不是通用计划树遍历器。
- `check_range_locks` 的锁集合由调用方传入；它不访问实际 lock store，也不设置“已检查”状态。重复调用会重复扫描。底层 Rust `check_lock` 使用 `lock.start_ts <= start_ts` 且 key 相等的精简条件，与 Go 对锁操作类型、primary get 和严格小于关系的完整判断并不等价。
- `run_processor` 先把 `reader.scan` 的全部结果物化为 `Vec<KvPair>`，没有 Limit 提前终止、流式背压或 `skip_value` 优化。大范围扫描的内存占用与结果行数线性相关。
- `prefix_next([])` 返回 `[0]`，全 `0xff` 输入会追加 `0`；独立 Rust 测试只覆盖了带进位后缀的 `[0x01, 0xff] -> [0x02, 0x00]` 情形。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务。`KvReader: Send + Sync` 允许读取器被并发共享；`check_range_locks` 只是 `&self` 只读检查，但 `ClosureExecutor::execute` 与所有 processor 的状态推进需要 `&mut self`，单个执行实例没有并发推进接口，也没有内部同步。

资源释放依赖 Rust 所有权：扫描结果在 `run_processor` 栈上物化并在返回后释放；`TableScanProcessor::finish` 和 `HashCountProcessor::finish` 用 `mem::take` 移交容器；`TopNProcessor::finish` 用空堆替换后消费旧堆，避免从借用的 `self` 中直接移动字段。没有显式 open/close；若 `process` 失败，`finish` 不执行，但普通容器仍由析构自动回收。

计时使用单调时钟 `Instant`。`ExecDetail::update` 测量从传入起点到调用时的 elapsed；`ClosureExecutor::execute` 只有在下游成功返回后才写根级 detail，因此失败执行不会留下本文件级的统计记录。

## 与 Go 版本的对应关系

Go 对照文件是同目录 `closure_exec.go`。名称上的主要映射包括：`chunkMaxRows`/`CHUNK_MAX_ROWS`、`scanType`/`ScanType`、`execDetail`/`ExecDetail`、`closureExecutor`/`ClosureExecutor`、`closureProcessor`/`ClosureProcessor`、各 count/scan/selection/topN/hashAgg processor，以及 `safeCopy`、`isResolved`、`exceedEndKey`。

Rust 版本不是 Go 文件的等量重写，关键差异如下：

- Go `buildClosureExecutor` 会从 protobuf DAG 建立专用 processor，并由请求测试的 `buildExecutorsAndExecute` 实际执行；Rust 当前 DAG 入口和 `ClosureExecutor::execute` 都复用 `mpp_exec::execute_executor`，本文件 processor 尚未接线。
- Go `execute` 先检查 lock，对点查调用 `Get`，对范围按方向调用 `Scan`/`ReverseScan`，支持 Limit 提前停止并逐 range 统计 count/NDV；Rust `execute` 不自动检查 lock，也不单独走点查，范围统计由 `mpp_exec::scan` 产生后可被清空。
- Go point-range 判断还依赖无 common handle、唯一扫描等上下文；Rust `is_point_get_range` 只有纯字节前缀后继判断。
- Go `closureProcessor` 直接实现 `dbreader.ScanProcessor`，`SkipValue` 能影响底层读取；Rust trait 的 `skip_value` 当前没有消费者。
- Go table/index scan 负责 row/index codec 解码、物理表 ID、commit TS、输出 offset 和 chunk 编码；Rust processor 接收已经解码为 `Vec<Datum>` 的 `KvPair.value`，语义明显更精简。
- Go hash aggregation 支持通用聚合函数与上下文；Rust `HashCountProcessor` 只实现按表达式分组的 COUNT。
- Go lock 检查区分 resolved lock、写锁类型、primary get 与可见性；Rust 此处委托的 `cop_handler::check_lock` 是 mock 的简化模型。

因此扩展或修复时应以“保持可观测行为与 Go 一致”为目标，但必须先决定功能应落在当前活跃的 `mpp_exec` 路径，还是完成本文件 closure 路径的生产接线，不能仅修改未接线 processor 后声称请求行为已变化。

## 扩展指南

- 新增 closure 支持的单子算子时，需要同步修改 `closure_supported` 的准入规则、`get_executor_list` 的子节点遍历，并确认 `mpp_exec::execute_executor` 真正支持该算子；应在独立的 `closure_exec_test.rs` 增加构造成功/拒绝测试，并在 `cop_handler_test.rs` 增加用户可见执行测试。
- 若要启用专用 processor 路径，应明确选择构建器、处理器组合、锁检查、点查/范围扫描、Limit 提前停止、range count/NDV 和 execution summary 的接线点。必须补齐覆盖 `run_processor` 与每种 processor 的独立测试，而不是把测试嵌进源文件。
- 修改扫描投影或 NULL 计数时，重点检查 `CountColumnProcessor::process`、`TableScanProcessor::process` 与列越界错误；测试至少覆盖 NULL、缺列、空输入和多次 finish/复用语义。
- 修改 TopN 时同时核对 `TopNProcessor` 与 `topn.rs::TopNHeap`，尤其是 limit 为 0、升降序、NULL/类型比较和相等键稳定性；当前实现会克隆完整行，有内存与复制成本。
- 修改分组计数时同步检查 `HashCountProcessor` 的 key 编码/排序语义与 Go 输出顺序。若扩展为通用聚合，应优先复用 `mpp_exec.rs` 已有聚合状态机，避免形成两套不一致实现。
- 修改锁行为时同时核对 `ClosureExecutor::check_range_locks`、`cop_handler::check_lock` 与 Go `checkRangeLockForRange/checkLock`；兼容风险包括边界是否半开、`<`/`<=`、写锁类型、resolved lock 和 primary get 例外。
- 修改分块时不要只改 `CHUNK_MAX_ROWS`；当前结果分块实际由 `cop_handler::append_row` 的 `ROWS_PER_CHUNK` 控制，应统一常量来源并补 chunk 边界测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/unistore/cophandler/closure_exec.rs` 确认目标文件已索引，`node --file ... --offset 1 --limit 500` 读取到全部 415 行和 55 个符号。
- RustCodeGraph `query`：确认 `ClosureExecutor` 位于第 67 行、`ClosureProcessor` 位于第 155 行、`run_processor` 位于第 332 行、`check_range_locks` 位于第 130 行、`get_scan_executor` 位于第 369 行。方法级 `callers/callees` 未返回可用边，因此调用接线结论改由源码搜索核验。
- 已读 Rust 生产文件：`closure_exec.rs`、同目录 `Cargo.toml`、`lib.rs`、`cop_handler.rs`、`mpp_exec.rs`，以及直接依赖的 `topn.rs` 符号引用；仓库没有该包的 `doc.go`。
- 已读测试：`closure_exec_test.rs` 全文、`cop_handler_test.rs` 中 `TestClosureExecutor`/`TestMppExecutor` 及相关扫描断言；全仓库 Rust 搜索用于确认公开符号当前只有上述测试直接使用。
- 已读 Go 对照：同目录 `closure_exec.go` 的构建、执行、锁检查、各 processor、聚合与辅助函数，以及 `cop_handler_test.go` 的 `buildExecutorsAndExecute`、`TestIsPrefixNext`、`TestPointGet`、`TestClosureExecutor`、`TestMppExecutor`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的结构命令确认文件存在且恰好包含十一个固定二级标题，并人工复核当前接线状态、Go/Rust 差异和安全扩展入口均有源码依据。

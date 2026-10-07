# `pkg/executor/table_reader.rs`

## 文件定位

[`table_reader.rs`](table_reader.rs) 属于 `astersql-executor` crate；[`lib.rs`](lib.rs) 以 `pub mod table_reader` 公开它，[`Cargo.toml`](Cargo.toml) 则把该 crate 映射到 Go 包 `pkg/executor`。文件移植 Go [`table_reader.go`](table_reader.go) 的表扫描执行逻辑：把表或分区的逻辑范围编码为 DistSQL/Coprocessor 请求，在 TiKV 或 TiFlash 上执行，再把结果逐批写入调用方 Chunk。

当前 Rust 文件是一个通过泛型后端隔离具体 TiDB 类型的可复用逻辑层。核心实现依赖 `TableReaderBackend`、`kvRangeBuilder` 和 `SelectResultOverride` 注入实际的会话、计划、请求和结果类型。仓库搜索未发现这些 trait 的具体实现，也未发现 Rust 代码构造本文件的 `TableReaderExecutor`；因此它虽然已经由 crate 公开并会参与编译，当前证据只能证明逻辑骨架已移植，不能证明它已接入 Rust SQL 执行主链。

## 核心职责

- `TableReaderExecutor::Open` 初始化执行器级内存追踪，按相关列重建 DAG/范围，处理有序扫描的 int64 有符号边界，并建立一个或多个结果流。
- `buildKVReq*` 根据普通表、分区表、TiFlash batch cop 和分组范围的不同形态构建 DistSQL 请求，同时更新 DAG 中的物理表 ID。
- `buildRespForGroupedRanges` 在串行结果、单请求结果和按 `byItems` 归并排序的多路结果之间选择。
- `TableReaderExecutor::Next` 保留调用方 Chunk 的 required-rows 状态，将其直接传给底层结果流，并在取数后按定义顺序补算虚拟列。
- `tableResultHandler` 在跨 int64 边界的两个结果流之间维持稳定顺序，并负责原始数据读取和关闭清理。
- dummy 模式只收集 `kvRanges`，不访问存储，用于与 Go 临时表、缓存表经 UnionScan 读取键范围的语义对齐。

## 主要符号

### 请求与范围模型

- `KeyRange` 表示半开区间 `[start_key, end_key)`；`PartitionIDAndRanges` 把物理分区 ID 与其键区间绑定。
- `StoreType::{TiKV, TiFlash, Other}` 控制 DAG 重建方式和 TiFlash 分区请求分支。
- `RequestKeySource<R>` 统一四种键来源：已经编码的键、按分区分组的键、带分区 ID 的键，以及由表 ID、common-handle 标志和逻辑范围组成的 handle 范围。
- `TableReaderRequestOptions<D, I, M>` 汇总 DAG、快照时间戳、顺序、事务/副本范围、stale-read、InfoSchema、内存追踪、存储类型、paging、batch cop、网络数据估计和可选 TiDB server ID。分区拆分请求显式传入 `is_staleness: None`，普通 `buildKVReq` 才传入当前 stale-read 标志。

### 抽象边界

- `kvRangeBuilder<D, R, E>` 提供整体编码 `buildKeyRange` 与按分区返回 `(partition_ids, key_ranges)` 的 `buildKeyRangeSeparately`。
- `TableReaderBackend` 是文件的主要端口，定义上下文提取、DAG 重建、范围处理、请求构造、结果选择、Chunk 操作、虚拟列计算、错误规范化和索引使用上报。执行器本身不绑定具体 TiDB 数据结构。
- `SelectResultOverride<B>` 与 `selectResultHook::SelectResult` 提供测试/注入入口；有 override 时绕过后端默认的 `select_with_runtime_stats`，否则把根计划 ID 一并传给默认实现。
- `tableReaderExecutorContext<B>` 保存打开和执行所需的 DistSQL、Ranger、PB、表达式、语句内存追踪与 InfoSchema 快照。`GetDDLOwner` 只在 `getDDLOwner` 为真时向后端查询，否则返回明确错误。

### 执行器与辅助函数

- `TableReaderExecutor<B>` 持有表、逻辑/分组/KV 范围、DAG、计划、schema、顺序与存储选项、内存 tracker、结果 handler、虚拟列缓存及 dummy 状态。
- `sortAndGetKVRangesFromReqs` 先要求后端对每个请求内部的键区间排序，再按 `start_key` 全局排序并拼接，供 UnionScan 等上层观察。
- `buildVirtualColumnIndex` 找出 schema 中虚拟列，并按它们在表列定义中的 offset 排序；`buildVirtualColumnInfo` 同时缓存相应返回类型，确保依赖前序虚拟列的表达式按定义顺序计算。
- `tableResultHandler<B>` 保存可选的第一段结果和主结果；`nextChunk`/`nextRaw` 必须先耗尽第一段，`Close` 则尝试关闭两段并保留第一个错误。

## 执行流程

### 打开阶段

1. `Open` 通过 `start_trace_region` 建立 `TableReaderExecutor.Open` trace，并执行可注入的打开延迟。
2. 复用已有 `memTracker` 时先 reset，否则按执行器 ID 新建；随后把它 attach 到 `stmtMemTracker`。
3. `corColInFilter` 为真时重新序列化 DAG：TiFlash 使用 `rebuild_tree_based_dag(tablePlan)`，其他存储使用 `rebuild_list_based_dag(plans)`。启用运行时统计时还设置 execution summaries。
4. `corColInAccess` 为真时从 `plans[0]` 重新解析访问范围；若设置 `groupByColIdxs`，同步重建 `groupedRanges`。
5. 范围被统一为组，然后由 `split_ranges_across_int64_boundary` 拆为两段。该步骤只在后端判定有序、方向和 common handle 等条件需要时产生双段，目的是修正无符号主键的物理键序与 SQL 数值序差异。
6. dummy 分支按扫描方向必要时交换两段，只调用 `buildKVReq` 收集键范围并立即返回，不创建结果流。
7. 正常分支分别调用 `buildRespForGroupedRanges`。只有一段时直接设为主结果；存在两段时第一段成为 `optionalResult`，第二段成为主结果。

### 请求与结果构建

- TiFlash 且存在分区 `kvRangeBuilder` 时，代码要求输入只有一个范围组且原始 `groupedRanges` 为空。非 batch cop 为每个分区建立请求和结果，再由 `serial_select_results` 串行组合；batch cop 则构造一个 `PartitionIDAndRanges` 请求。
- 通用路径由 `buildKVReqSeparatelyForGroupedRanges` 遍历各组。只有同时存在 range builder 和非空 `byItems` 时按分区拆请求，否则每组建立一个请求。
- 空范围也必须生成一个空请求，避免后续无结果对象。单个结果直接返回；多个结果要求 `byItems` 非空，并通过 `sorted_select_results` 做有序归并。
- `buildKVReqSeparately` 每次把当前物理分区 ID 写入 DAG，并以 `KeyRanges` 构造请求。`buildKVReqForPartitionTableScan` 一次写入全部分区 ID，并使用 `PartitionIDAndRanges`。
- `buildKVReq` 有 range builder 时使用 `PartitionKeyRanges`，否则由表 ID、common-handle 和逻辑范围生成 `HandleRanges`。集群系统表若要求路由到 DDL Owner，还会查询 owner 并附加 server ID。

### 拉取与关闭

- `Next` 的 dummy 分支只清空 Chunk。正常分支记录扫描事件，调用 `tableResultHandler::nextChunk`，再调用 `fill_virtual_column_values`。它刻意不 reset/替换调用方 Chunk，使 required-rows 契约能够原样下推。
- `tableResultHandler::nextChunk` 若第一段尚未结束，先从它取数；只有返回零行才标记结束并转向主结果。`nextRaw` 用 `None` 表示第一段结束，并对两段错误都调用 `normalize_context_error`。
- `Close` 可选上报 handle 的 cop 索引使用情况，关闭 handler，清空 `kvRanges`。handler 会尝试关闭所有存在的结果；若两次关闭都失败，仅返回并 trace 第一个错误。

## 数据与状态

- 配置态：`startTS`、`txnScope`、`readReplicaScope`、`isStaleness`、`storeType`、`keepOrder`、`desc`、`paging`、`batchCop` 和 `netDataSize` 在请求创建时被复制到 `TableReaderRequestOptions`。
- 计划态：`dagPB` 可能在每次 `Open` 因相关列或分区 ID 原地更新；`plans[0]` 被用于解析相关访问范围、根计划信息及关闭时上报，因此构造方必须保证 `plans` 非空。
- 范围态：`ranges` 是普通逻辑范围，`groupedRanges` 是按访问列分组后的范围，`kvRanges` 是已经编码并排序的实际键范围。`Open` 会填充后者，`Close` 会清空它。
- 结果态：`resultHandler: Option<_>` 在 `Open` 初始化。正常 `Next` 假定它存在；生命周期调用顺序必须是 `Open -> Next* -> Close`。
- 虚拟列态：`virtualColumnIndex` 与 `virtualColumnRetFieldTypes` 由 `buildVirtualColumnInfo` 成对刷新，二者的顺序必须一致。
- 内存态：`memUsage` 估算执行器本体、`ranges` 指针容量、每个 range、`kvRanges` 与 DAG；注释和实现均未声称覆盖所有间接分配。

## 依赖与调用关系

上游方面，RustCodeGraph 与 `rg` 只确认 [`lib.rs`](lib.rs) 公开此模块，没有发现仓库内具体后端实现或 Rust 构造调用点。Go 主链中，同路径实现由 executor builder/DistSQL 执行路径构造；该事实只能用于解释移植目标，不能证明 Rust 已接线。

文件内部的关键调用边为：

- `Open -> rebuild_*_dag / resolve_correlated_ranges / split_ranges_across_int64_boundary / buildRespForGroupedRanges`；
- `buildRespForGroupedRanges -> buildKVReqSeparately* / buildKVReqForPartitionTableScan / select_result / serial_select_results / sorted_select_results`；
- `buildKVReq* -> kvRangeBuilder::buildKeyRange* / update_executor_table_ids / TableReaderBackend::build_request`；
- `Next -> tableResultHandler::nextChunk -> select_result_next`，随后 `fill_virtual_column_values`；
- `Close -> tableResultHandler::Close -> close_select_result`。

crate 级依赖由 [`Cargo.toml`](Cargo.toml) 声明，相关边界包括 `astersql-distsql`/`astersql-distsql-context`、`astersql-kv`、`astersql-planner-*`、`astersql-expression*`、`astersql-infoschema*`、`astersql-table*`、`astersql-types` 及内存/Chunk 等 `astersql-util-*` crate。不过本文件只直接使用标准库类型，具体 crate 类型被收敛到 `TableReaderBackend` 的关联类型中。

## 错误处理与边界

- DAG 重建、相关范围解析、范围编码、表 ID 更新、请求构建、Select、虚拟列计算和结果读取均使用 `Result` 与 `?` 原样传播后端错误。
- `GetDDLOwner` 在上下文不支持 DDL 时生成 `"GetDDLOwner in a context without DDL"`，避免静默选择错误节点。
- `nextRaw` 额外做上下文错误规范化；Chunk 路径 `nextChunk` 不做该转换，由后端读取错误直接返回。这与 Go `normalizeCtxErrWithCause` 只包裹 raw 路径一致。
- 多处 `expect`/`assert!` 表示构造与生命周期不变量，而非可恢复输入错误：分区构建路径必须有 `kvRangeBuilder`；多路归并必须有 `byItems`；请求选项必须在内存 tracker 初始化后创建；正常读取必须先初始化 handler 和结果；`plans[0]` 必须存在。
- `buildKVReqSeparately` 以同一索引访问 `partition_ids` 与 `key_ranges`，因此 `kvRangeBuilder` 必须返回等长且一一对应的数组；该约束未由类型系统表达。
- `buildVirtualColumnIndex` 假定每个虚拟 schema 列都能在 `columns` 中找到合法 offset；具体缺失行为由后端 `column_offset_by_id` 决定。
- 空扫描仍构造请求，避免后续无 `SelectResult` 引发 panic；这是显式的控制流边界。

## 并发与资源生命周期

- `TableReaderBackend`、`kvRangeBuilder` 和 `SelectResultOverride` 均要求 `Send + Sync`，共享实现通过 `Arc` 持有；但 `TableReaderExecutor` 的可变状态由 `&mut self` 串行推进，本文件没有为同一执行器提供并发 `Open`/`Next`/`Close`。
- `open_failpoint_delay` 使用当前线程的 `thread::sleep`，属于同步阻塞延迟，不会创建后台任务。
- 结果流不依赖 Rust `Drop` 自动关闭；调用方应显式执行 `Close`。`tableResultHandler::Close` 使用 `take()` 清空两个槽位，即使关闭失败也不会在 handler 中保留旧句柄。
- 内存 tracker 在每次 `Open` reset 或创建并 attach 到语句 tracker，但 `Close` 不 detach；具体父子 tracker 生命周期由后端实现负责。
- 多请求结果的“串行”或“有序归并”由后端返回的复合 `SelectResult` 管理；本文件不启动线程，也不规定底层请求是否并行。
- `TraceGuard` 保存在 `Open` 的局部 `_trace` 中，其离开作用域时结束区域；具体结束机制由后端返回类型的生命周期实现。

## 与 Go 版本的对应关系

Rust 的字段、函数名和控制流直接对应 [`table_reader.go`](table_reader.go)：`tableReaderExecutorContext`、`TableReaderExecutor`、`Open/Next/Close`、四个 `buildKVReq*` 路径、虚拟列辅助函数及 `tableResultHandler` 均能逐项对应。关键语义也保持一致：相关列触发 DAG/范围重建；无符号主键跨 int64 边界时分两段；dummy 只收集键；TiFlash 分区按 batch cop 选择单请求或多请求；多个分组结果按 `byItems` 归并；required-rows 不在 TableReader 层被重置。

Rust 的结构性差异主要来自移植边界：

- Go 直接依赖 `sessionctx.Context`、`distsql.RequestBuilder`、`kv.Request`、`chunk.Chunk` 等具体类型；Rust 通过 `TableReaderBackend` 关联类型和方法表达同一操作。
- Go 的可空接口/指针映射为 Rust `Option`，结果和错误映射为 `Result`，共享注入对象使用 `Arc<dyn Trait>`。
- Go `getDDLOwner` 保存可选闭包；Rust context 保存布尔能力标志并在需要时回调 backend。
- Go 的 `closeAll` 行为在 Rust 中展开为“两路都尝试关闭、保留首错、最后 trace”。
- Rust 文件目前没有具体 backend 和生产调用点，因此尚不能把 Go 已运行的 executor 行为等同为 Rust 已上线行为。

测试证据也存在迁移差异。Go [`table_readers_required_rows_test.go`](table_readers_required_rows_test.go) 的 `TestTableReaderRequiredRows` 直接构造 `TableReaderExecutor`，验证多组非均匀 required-rows 请求。Rust [`table_readers_required_rows_test.rs`](table_readers_required_rows_test.rs) 虽沿用同名文件，但当前只构造 `distsql::IndexReaderExecutor`；它不能作为本文件 TableReader 的直接回归测试。仓库中未找到本文件的独立 Rust 单元测试。

## 扩展指南

- 接入生产 Rust 主链时，优先实现具体 `TableReaderBackend` 和 `kvRangeBuilder`，并在 executor builder 中构造本类型；不要把具体会话/存储类型重新硬编码进本文件。接线后应添加独立 `table_reader_test.rs`（或遵循包内测试命名约定的独立文件），禁止把测试内嵌进生产源文件。
- 新增请求选项时，同时更新 `TableReaderRequestOptions`、`request_options`、后端 `build_request` 实现，并核对普通、按分区拆分和 batch partition 三条路径。特别检查 stale-read 是否应在分区拆分路径出现。
- 修改范围排序或双段顺序时，要覆盖 ascending/descending、signed/unsigned、common handle、dummy/UnionScan 和空范围；该区域的错误通常表现为结果乱序而非请求失败。
- 修改多分区 TiFlash 行为时，要保持 `update_executor_table_ids` 与请求中物理分区范围一致，并分别验证 `batchCop = true/false`。
- 修改 `Next` 时不要无条件 reset Chunk；应增加与 Go `TestTableReaderRequiredRows` 相同矩阵的 Rust TableReader 回归测试，证明 required-rows 原样抵达底层 `SelectResult`。
- 修改虚拟列时，同时验证列 offset 排序、返回类型顺序和依赖前序虚拟列的表达式。修改关闭逻辑时验证双路均被关闭及首错优先级。
- 性能风险主要在范围克隆、每分区请求数量、全局键排序、多路 merge sort 和 DAG 克隆；兼容风险主要在请求选项遗漏、分区物理 ID、扫描顺序与上下文错误规范化。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/table_reader.rs` 确认目标文件已索引。
- RustCodeGraph `node --file pkg/executor/table_reader.rs` 分段读取了全部 940 行，核对了本文列出的 trait、结构体、函数、分支和内部调用关系。
- RustCodeGraph `query` 核对了 Rust/Go 同名的 `TableReaderExecutor`、`tableResultHandler`、`buildVirtualColumnIndex` 和 `sortAndGetKVRangesFromReqs`；自然语言 `explore` 还确认了 `Open`、请求构建、结果处理及后端方法的局部调用边。
- 读取 [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)，确认 crate 名为 `astersql-executor`、Go 包映射为 `pkg/executor`，且模块公开；仓库搜索确认没有 `TableReaderBackend` 的具体实现或本文件执行器的 Rust 构造点。
- 读取 Go [`table_reader.go`](table_reader.go) 的完整核心实现和 [`table_readers_required_rows_test.go`](table_readers_required_rows_test.go) 的 `TestTableReaderRequiredRows`；读取 Rust [`table_readers_required_rows_test.rs`](table_readers_required_rows_test.rs) 全文，确认它当前只覆盖 IndexReader 而非本文件。
- 包目录不存在 `doc.go`，因此没有额外的包级 Go 契约可读。仓库指令所述 `.agents/skills/tidb-verify-profile` 在当前检出中也不存在，无法运行其 Ready 入口；本任务按编号任务要求执行文档结构验证，不运行 Cargo。

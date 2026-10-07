# `pkg/executor/sample.rs`

## 文件定位

本文件属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/executor/lib.rs` 的 `pub mod sample` 暴露。它是 Go `pkg/executor/sample.go` 的 Rust 移植边界：为 `TABLESAMPLE ... REGIONS()` 一类“按 Region 取代表行”的执行方式提供通用状态机，但把真实存储、Region 查询、KV 扫描、行解码和 chunk 写入抽象为 `TableSampleRuntime`。

当前 Rust 代码的直接使用者仅见独立单元测试 `pkg/executor/sample_test.rs`；仓库内未发现 Rust `builder.rs` 构造 `TableSampleExecutor` 或实现真实存储运行时。因而本文件目前是可测试的执行核心和移植基础，不应表述为已接入 Rust 生产执行器主链。Go 主链的真实入口是 `pkg/executor/builder.go::executorBuilder.buildTableSample`。

## 核心职责

- 用 `KeyRange` 表示半开键区间 `[start_key, end_key)`，并按运行时返回的 Region 边界切分整表或各分区键空间（`splitTableRanges`、`splitIntoMultiRanges`）。
- 根据 `descending` 对候选区间排序，以稳定的前缀消费方式决定采样顺序（`sortRanges`、`pickRanges`）。这里的“采样”不是在文件内生成随机数，而是每个被选 Region/区间扫描第一条 KV。
- 以受限工作线程并发扫描每个区间，同时在汇总阶段恢复输入区间顺序（`scanFirstKVForEachRange`）。
- 将 KV 交给运行时构建的列映射解码为行，按请求容量追加，并在每行追加后重置行级临时映射（`writeChunkFromRanges`）。
- 提供与 Go 执行器生命周期同形的 `Open`、`Next`、`Close`；真正工作由内部 `TableRegionSampler` 完成。

## 主要符号

- `KeyRange { start_key, end_key }`：可克隆、可比较的半开键区间。边界排序使用字节序。
- `SampleKv<H, V> { handle, value }`：扫描结果，只携带行标识与原始 value；空区间由 `None` 表示。
- `TableSampleRuntime`：本文件最重要的适配 trait。关联类型描述上下文、请求、handle、value、行、列、列映射和错误；方法分成请求容量管理、键范围/Region 查询、并发度与首 KV 扫描、列映射/行解码四组。`Clone + Send` 以及 `Handle/Value/Error: Send` 是并发扫描的类型约束。
- `TableSampleExecutor<R>`：公开执行器门面，持有一个 `TableRegionSampler<R>`。`Open`、`Close` 当前为空操作；`Next` 先 `reset_request`，完成时返回空请求，否则调用 `writeChunk`。
- `TableRegionSampler<R>`：核心有状态对象，保存 `runtime`、MVCC `start_timestamp`、`physical_table_id`、排序方向和懒初始化的 `ranges`。
- `newTableRegionSampler`：创建 `ranges: None` 的采样器，实际切分延迟到第一次写 chunk。
- `writeChunk`：循环选取所需数量的区间并写行；空 Region 未产出行时会继续取后续区间补足本次请求，直到请求无需更多行或区间耗尽。
- `initRanges` / `pickRanges` / `finished`：分别负责一次性切分排序、从已排序列表头部排空至多 `count` 个区间，以及用“已初始化且为空”表示完成。
- `writeChunkFromRanges`：构建解码元数据、扫描样本、逐行解码和追加，追加后调用 `reset_row_map`。
- `splitTableRanges` / `splitIntoMultiRanges` / `sortRanges`：范围生成、边界规范化和顺序控制工具。
- `scanFirstKVForEachRange`：并发扫描工具；用原子下标分工，用 MPSC 通道汇总 `(输入索引, 结果)`。
- `resetRowMap`：对运行时 `reset_row_map` 的公开转发，保留 Go 风格命名；主流程直接调用运行时方法。

## 执行流程

1. 上层构造 `TableSampleExecutor` 和尚未初始化范围的 `TableRegionSampler`。当前 Rust 仓库中这一动作只由 `sample_test.rs` 完成；Go 生产路径由 `builder.go::buildTableSample` 完成。
2. `TableSampleExecutor::Next` 清空调用方请求。若 `finished()` 为真，立即成功返回，调用方观察到空批次。
3. `TableRegionSampler::writeChunk` 首次调用 `initRanges`：`splitTableRanges` 优先采用 `partition_key_ranges()`；若为空，则由 `table_key_range(physical_table_id)` 生成整表范围。每个源范围再交给 `splitIntoMultiRanges`。
4. `splitIntoMultiRanges` 请求 `region_boundaries`。运行时返回 `None` 时保留原范围；返回空列表时产生 `no_regions_error`；否则仅保留严格处于起止键之间的边界，排序、去重后生成连续子区间。
5. `sortRanges` 按 `start_key` 升序排列；`descending` 为真时整体反转。降序只改变 Region/区间的访问次序，`scan_first_kv` 的契约仍是区间内升序取第一条 KV。
6. `writeChunk` 按 `required_rows(request)` 从列表头部排出区间。`writeChunkFromRanges` 先构建列信息和解码映射，再调用 `scanFirstKVForEachRange`。
7. 并发扫描用 `executor_concurrency().max(1).min(ranges.len())` 确定线程数。线程通过 `AtomicUsize::fetch_add(Ordering::Relaxed)` 领取互不重复的索引，各自使用克隆的运行时调用 `scan_first_kv(range, start_timestamp)`。
8. 主线程在作用域线程全部结束后收集通道结果，按原输入索引排序；空区间的 `None` 被跳过，错误按区间顺序传播，成功样本保持候选范围顺序。
9. 每个样本经 `decode_row` 转为输出行；若请求已满则停止追加，否则依次 `append_row`、`reset_row_map`。若空区间导致本批行数不足，外层循环继续消费后续范围。
10. 当 `ranges` 已初始化且完全排空，`finished()` 成为真；下一次 `Next` 重置请求后返回空批次，形成执行器耗尽信号。

## 数据与状态

`TableRegionSampler::ranges` 同时编码初始化状态和剩余工作：`None` 是尚未查询/切分 Region，`Some(non-empty)` 是仍有区间，`Some(empty)` 是完成。`pickRanges` 使用 `drain(..count)`，因此区间一旦派发就从状态中移除；发生扫描或解码错误时不会自动放回，调用方应把错误视为本次执行失败，而不是在同一采样器上重试。

`start_timestamp` 原样传给每次 `scan_first_kv`，真实 MVCC 快照语义由运行时保证。`physical_table_id` 只在非分区 fallback 的 `table_key_range` 中直接使用；分区选择已由 `partition_key_ranges` 封装。列和解码映射在每次 `writeChunkFromRanges` 调用时重新构建，没有跨批缓存。

请求容量由运行时的 `required_rows` 与 `request_is_full` 双重约束。正常契约要求二者一致；若运行时报告的剩余数量大于真实容量，已从 `ranges` 排出的多余样本会因 `request_is_full` 被跳过，无法在下一批恢复，因此实现运行时时必须维护这两个方法的一致性。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 公开 `sample` 模块，并在 `#[cfg(test)]` 下通过 `#[path = "sample_test.rs"] mod sample_test` 接入独立测试。RustCodeGraph 对 `TableSampleExecutor`、`newTableRegionSampler` 和 `scanFirstKVForEachRange` 的 callers 未给出生产调用方；补充的 Rust 全仓引用检查也只找到本文件与 `sample_test.rs`。因此生产接线仍未验证/未实现。

下游方面，本文件仅直接依赖 Rust 标准库：`Arc` 共享只读范围和原子计数器，`AtomicUsize` 分发任务，`std::thread::scope` 管理借用安全的线程生命周期，`mpsc` 汇总结果。业务依赖全部反转到 `TableSampleRuntime`：Region 边界、表/分区范围、快照扫描、列映射、解码和请求写入均不绑定具体 AsterSQL crate 类型。

Go 对照的上游链为 `executorBuilder.buildTableSample` → `newTableRegionSampler` → `TableSampleExecutor.Next` → `tableRegionSampler.writeChunk`。其下游直接连接 `sessionctx.Context`、TiKV Region cache、MVCC snapshot、`tablecodec`、row decoder 和 chunk；Rust trait 正是对这些职责的抽象，但 `pkg/executor/Cargo.toml` 中已有的 executor/kv/tablecodec 等依赖并未被本文件直接使用。

## 错误处理与边界

- 所有业务错误使用 `R::Error` 原样传播；文件不包装上下文。可能来源包括 Region 查询、无 Region、扫描、列映射构建和解码。
- `region_boundaries == None` 表示后端不支持/不需要切分，返回原区间；`Some(empty)` 则明确报 `no_regions_error`。这两个状态不可混淆。
- 边界恰好等于 `start_key` 或 `end_key` 会被过滤，乱序与重复边界会排序去重，避免空子区间和重复区间。
- `scanFirstKVForEachRange` 对空输入直接返回空向量，避免创建零线程；配置并发度为零时会提升到一。
- 单个区间没有 KV 时返回 `Ok(None)` 并被跳过，不是错误。`writeChunk` 会尝试后续区间补足请求。
- `pickRanges` 依赖 `ranges` 已初始化，若绕过 `initRanges` 直接调用会触发 `expect("ranges initialized")`；正常入口 `writeChunk` 先初始化。
- 工作线程 panic 会在 `std::thread::scope` 结束时向调用线程传播 panic，而不是转换为 `R::Error`。通道发送失败只会让该工作线程停止。
- 所有扫描线程结束后才开始解码；因此某个扫描错误不会提前取消其他扫描。结果排序后，遇到第一个按范围顺序排列的错误即返回。

## 并发与资源生命周期

每次 `scanFirstKVForEachRange` 创建一个短生命周期线程组。`Arc<Vec<KeyRange>>` 在所有 worker 间共享只读区间，`AtomicUsize` 保证每个索引最多被领取一次；`Ordering::Relaxed` 足够，因为原子值只用于唯一任务分配，不承担其他内存发布协议。每个 worker 克隆一份 `runtime`，所以其克隆实现必须让并发使用安全且语义正确；trait 要求 `Clone + Send`，但不要求 `Sync`，因为 worker 不共享同一个可变运行时实例。

发送端在父作用域中显式 `drop(sender)`，各 worker 的发送端在线程退出时销毁；作用域等待所有线程结束，随后 `receiver.into_iter()` 能可靠读到 EOF，不遗留后台线程。结果先全部收集再排序，内存开销为 `O(ranges.len())`，线程数上限为区间数与运行时并发度的较小值。

采样器本身不持有显式锁、事务或长期通道；`Open`/`Close` 也不分配或释放资源。真实快照、迭代器和解码缓存的生命周期属于 `TableSampleRuntime` 实现。与 Go 的持续 fetcher channel 模型相比，Rust 每批创建 scoped threads，安全性直观但可能带来重复建线程成本，接入真实运行时时应做性能评估。

## 与 Go 版本的对应关系

Rust `TableSampleExecutor`、`TableRegionSampler`、`newTableRegionSampler`、`writeChunk`、`initRanges`、`pickRanges`、`writeChunkFromRanges`、`splitTableRanges`、`splitIntoMultiRanges`、`sortRanges`、`scanFirstKVForEachRange` 和 `finished` 均可在 `pkg/executor/sample.go` 找到同名或大小写对应实现。Rust 保留了 Go 风格方法名（文件级 `#![allow(non_snake_case)]`），便于逐项对照。

关键语义一致点包括：每个 Region 取首条记录、按范围顺序消费、降序仅反转范围顺序、空 Region 不产出行、按 chunk 剩余容量分轮、解码并追加后清空复用 row map。`pkg/executor/sample_test.rs` 还专门验证并发完成顺序不会打乱范围顺序，以及 `decode → append → reset` 的事件顺序。

当前差异与迁移边界如下：

- Go `TableSampleExecutor` 实现真实 `exec.Executor`，由 builder 根据临时表与采样方法选择 `emptySampler` 或 Region sampler；Rust门面是泛型结构，尚无相应 builder/执行器 trait 接线。
- Go 直接从 TiKV Region cache 加载并裁剪范围；Rust 由 `region_boundaries` 提供边界，再在本地过滤、排序、去重。Rust 的 `None` fallback 是抽象运行时特有协议。
- Go `buildSampleColAndDecodeColMap` 包含虚拟生成列、`_tidb_rowid`、`_tidb_commit_ts` 等具体列处理；Rust 将这些细节全部委托给 `build_sample_columns` 与 `decode_row`，本文件本身不能证明真实运行时已经覆盖它们。
- Go fetcher 使用 goroutine 和每 worker channel，并在 syncer 提前返回时清空通道；Rust使用每批 scoped threads、单一 MPSC，并等待全部扫描完成后解码。
- Go 保存显式 `isFinished`；Rust 从 `ranges == Some(empty)` 推导完成状态。

Go 测试 `pkg/executor/sample_test.go` 覆盖 SQL 层空表/多 Region、默认列和生成列、额外 row id、执行计划、最大 chunk、keyspace 隔离。Rust 测试是内存运行时的单元测试，只证明核心状态机与抽象边界，不等价于这些 SQL 集成能力已经在 Rust 主链可用。

## 扩展指南

- 接入真实 Rust 执行器主链时，应在 builder 层实现与 Go `buildTableSample` 等价的临时表限制、采样方法选择、快照时间戳和物理分区信息传递，并提供连接真实 KV/Region cache/row decoder/chunk 的 `TableSampleRuntime`。不要在本文件中复制一套存储实现。
- 扩展 Region 切分策略时优先修改 `region_boundaries` 的实现；若改变边界规范化规则，则同步检查 `splitIntoMultiRanges`，并在 `pkg/executor/sample_test.rs` 增加端点、重复、乱序、空列表和后端不支持等用例。
- 修改并发模型时重点保持三个不变量：每个范围至多扫描一次、输出顺序与传入范围一致、任何 worker 生命周期在函数返回前结束。相关回归应放在独立 `sample_test.rs`，不要把测试嵌入生产源文件。
- 修改请求容量语义时必须同步维护 `required_rows` 和 `request_is_full`，并验证空 Region 后补充、最大 chunk、多次 `Next` 与完成后的空批次。
- 扩展解码列能力时，对照 Go `buildSampleColAndDecodeColMap` 处理普通列、虚拟生成列、额外 handle 与 commit timestamp；测试应同时覆盖事件顺序和 SQL 层列值正确性。
- 若要优化性能，可评估复用工作线程、流式按序消费和避免每批重建列映射，但必须保留错误顺序、范围顺序和资源收敛保证，并测量大 Region 数下的线程与内存成本。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点，目标 `pkg/executor/sample.rs` 已索引为 323 行、41 个符号。
- RustCodeGraph `node --file pkg/executor/sample.rs`：核对全部类型、trait、函数、实现和并发代码；`query` 核对 `TableSampleExecutor`、`TableRegionSampler`、`newTableRegionSampler`、`writeChunk*`、`split*`、`sortRanges`、`scanFirstKVForEachRange`。
- RustCodeGraph `callees scanFirstKVForEachRange` 与 `callees writeChunkFromRanges`：确认并发度运行时调用，以及扫描、容量检查、解码、追加、row map 重置的下游边。
- RustCodeGraph callers 对 Rust 关键入口没有返回生产调用方；随后用 Rust 文件引用检查确认直接引用仅位于 `pkg/executor/sample_test.rs`。这是“尚未发现生产接线”的依据，而非对未来接线的假设。
- 读取 `pkg/executor/Cargo.toml` 与 `pkg/executor/lib.rs`：确认 crate 名、根文件、`pub mod sample` 以及独立 `sample_test.rs` 的测试接线；目标包未发现 `doc.go`。
- 读取 Go 对照 `pkg/executor/sample.go`、生产入口 `pkg/executor/builder.go::buildTableSample` 和 Go 测试 `pkg/executor/sample_test.go`：核对执行器构造、TiKV/解码细节、错误边界和 SQL 行为。
- 读取 Rust 独立测试 `pkg/executor/sample_test.rs`：核对多轮 `Next`、边界排序去重、非切分表、分区降序、chunk 上限、keyspace 范围、前缀消费、空 Region 补充、并发保序、row map 重置顺序和区间内正向扫描。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证本文恰含 11 个固定二级章节。

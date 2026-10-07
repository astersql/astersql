# `pkg/executor/aggregate/agg_hash_final_worker.rs`

## 文件定位

本文件属于 `astersql-executor-aggregate` crate；crate 入口 `pkg/executor/aggregate/lib.rs` 以 `pub mod agg_hash_final_worker` 暴露该模块，`pkg/executor/aggregate/Cargo.toml` 则把该 crate 映射到 Go 包 `pkg/executor/aggregate`。它位于并行 Hash Aggregation 的 Final 阶段：`HashAggPartialWorker` 先生成按 final worker 分桶的 `AggMap`，`HashAggExec::execute` 再创建 `HashAggFinalWorker`，合并内存中的 partial 状态或恢复 spill 分区，最后把聚合状态物化为输出 chunk。

RustCodeGraph 将该文件识别为 11 个符号，并给出唯一的文件级使用者 `pkg/executor/aggregate/agg_hash_executor.rs`。实际可执行实现位于第 321 行以后；第 13—320 行是保留的 Go 移植说明注释，不会参与编译，不能视为 Rust 已具备 channel、failpoint 或 worker `run` 生命周期。

## 核心职责

- `HashAggFinalWorker::merge_input` 将一个 partial `AggMap` 合并到 worker 私有的 `result`。相同 key 按聚合列逐项调用 `AggState::merge`；新 key 会先建立与 `aggregations` 等宽的空状态，再执行同样的合并。
- `HashAggFinalWorker::restore_from_disk` 在配置了 `ParallelHashAggSpillHelper` 时，以 helper 的分区游标逐分区恢复 `AggMap`，再复用 `merge_input` 合并，保证内存路径和落盘路径使用同一聚合语义。
- `HashAggFinalWorker::generate_result` 消耗整个 `result`，将每组原始 group row 与各 `AggState::result` 拼成最终 row，并按 `max_chunk_size` 切成 `FinalResult`。
- `hash_state_rows` 在物化结果前报告当前分组数，供 `HashAggExec` 写入 `HashStateRuntimeStats`。

该文件不负责扫描输入、计算 group key、决定是否 spill、线程创建或结果队列消费；这些分别位于 `agg_hash_partial_worker.rs`、`agg_spill.rs` 和 `agg_hash_executor.rs`。

## 主要符号

- `FinalResult { chunk: Chunk, error: Option<String> }`：Final 阶段交给执行器结果队列的批次对象。正常路径由 `generate_result` 创建，当前文件只产生 `error: None`；`HashAggExec::next` 仍检查 `error`，为错误结果形状保留兼容入口。
- `HashAggFinalWorker`：字段 `aggregations: Arc<Vec<Aggregation>>` 描述每个状态槽的聚合类型，`spill: Option<Arc<ParallelHashAggSpillHelper>>` 共享 spill 存储和原子分区游标，私有字段 `result: AggMap` 保存 `key -> (group row, states)`。
- `new(Arc<Vec<Aggregation>>, Option<Arc<ParallelHashAggSpillHelper>>) -> Self`：保留共享配置并创建空 `AggMap`。
- `merge_input(AggMap) -> Result<(), String>`：验证每组状态宽度，同时按 key、按聚合槽合并。
- `restore_from_disk() -> Result<usize, String>`：返回读取到的 group 条目总数；无 spill helper 时为成功的 `0`。
- `hash_state_rows() -> usize`：只读返回 `result.len()`。
- `generate_result(usize) -> Vec<FinalResult>`：通过 `std::mem::take` 取得并清空 `result`，生成一个或多个结果批次。

文件没有 trait、模块级常量或条件编译项；公开面是两个结构体以及上述构造、合并、恢复、计数和生成方法。

## 执行流程

1. `HashAggExec::execute` 用同一个 `Arc<Vec<Aggregation>>` 和可选 spill helper 构造 `final_concurrency` 个 final worker。
2. 未触发 spill 时，执行器把每个 partial worker 对应桶直接传给 `merge_input`。对每个 key，方法取得或创建目标状态数组，先检查 `target.len() == states.len() == aggregations.len()`，再按三个迭代器的相同位置调用 `target.merge(aggregation, source)`。
3. 一旦任何 partial worker 触发 spill，执行器会把尚在内存的 partial map 也写入 helper，避免同一个 group 分裂在内存结果和恢复消费者之间。随后只由第一个 final worker 调用 `restore_from_disk`；该方法按 `next_partition` 给出的降序分区逐个 `restore_partition`，并将每个恢复出的 map 交给 `merge_input`。
4. 无 group 列且所有输入 chunk 都为空时，`HashAggExec` 向第一个 worker 注入一个空 group 和默认 `AggState`，使 `COUNT`/`SUM` 等能够产生 SQL 聚合的默认行。
5. 执行器先调用 `hash_state_rows` 记账，再调用 `generate_result(max_chunk_size)`。后者逐组计算最终值，把 group 列放在前、聚合列追加在后；达到批次上限便提交一个 `FinalResult`，循环结束后提交非空尾批次。
6. `HashAggExec::next` 从自身的 `VecDeque<FinalResult>` 弹出批次，错误为空时返回 chunk，队列耗尽时返回 `None`。

`AggMap` 是 `BTreeMap`，因此当前实现的组输出顺序由编码后的 key 排序决定；调用方测试不应把这种内部排序当成无 `ORDER BY` SQL 的稳定输出契约。

## 数据与状态

`AggMap` 在 `agg_util.rs` 定义为 `BTreeMap<Vec<u8>, (Row, Vec<AggState>)>`：字节 key 用于分组查找，`Row` 保存应回写输出的 group 值，状态数组与 `aggregations` 按下标一一对应。`merge_input` 对新 key 采用输入中的 `group`，但不直接搬用输入状态数组，而是创建空状态后逐项 merge；对已有 key，则保留最先进入 `result` 的 group row。

`aggregations` 和 `spill` 使用 `Arc`，便于执行器与多个 worker 共享不可变聚合描述和带内部同步的 spill helper。`result` 仅由一个 `&mut self` worker 操作，没有内部锁。`restore_from_disk` 的 `restored` 统计的是恢复 map 的条目累计数，不是去重后的最终 group 数；重叠 key 会被合并，所以它可能大于 `hash_state_rows()`。

`generate_result` 是破坏性终结操作：`std::mem::take(&mut self.result)` 让 worker 立即回到空 map，返回后再次调用只会得到空向量，除非先重新 `merge_input`。`max_chunk_size.max(1)` 保证调用者即使传入 0，也不会生成无限增长或零容量语义的批次；每个非空 group 恰好进入一个 chunk。

## 依赖与调用关系

上游直接调用全部来自 `HashAggExec::execute`（`pkg/executor/aggregate/agg_hash_executor.rs`）：

- `HashAggFinalWorker::new` 创建 final worker 列表；
- `merge_input` 接收非 spill partial 输出，并接收无分组空输入的默认 group；
- `restore_from_disk` 由第一个 worker调用，消费共享 helper 的单一分区游标；
- `hash_state_rows` 为 `HashStateRuntimeStats::AddRows` 提供计数；
- `generate_result` 生成并扩展执行器的 `results` 队列。

下游直接依赖为 `agg_util.rs` 的 `AggMap`、`Aggregation`、`Chunk`、`AggState::{new, merge, result}`，以及 `agg_spill.rs` 的 `ParallelHashAggSpillHelper::{next_partition, restore_partition}`。spill helper 内部用互斥锁保护 `PartialResultSpill`，以原子游标从最高分区递减到 0，并在恢复时采用 take 语义清空分区。

Cargo 清单的普通依赖包含 `astersql-executor-aggfuncs`、chunk、execdetails 和 serialization；本文件使用的 `crate::agg_util`/`crate::agg_spill` 会经这些 crate 完成聚合状态和落盘序列化。清单中大量完整 TiDB 依赖只在 `cfg(windows)` 下声明，不能据此推断本文件在所有平台直接依赖 session context 或 Go 风格 channel。

## 错误处理与边界

- partial 状态宽度与目标状态或聚合描述宽度不一致时，`merge_input` 返回固定字符串错误 `partial result width mismatch`。检查发生在该 key 的空目标已经插入后，因此失败会留下本次调用的部分修改；调用者当前通过 `?` 终止整个 `execute`，不会继续使用这个 worker。
- `AggState::merge` 和 `AggState::result` 当前不返回 `Result`，因此聚合类型不匹配等问题不会在本文件形成可恢复错误通道。若下游将来改为可失败 API，本文件必须同步传播错误，避免继续输出部分结果。
- `restore_partition` 的锁中毒、序列化/IO 或恢复错误通过 `Result<_, String>` 经 `restore_from_disk`、`HashAggExec::execute`、`next` 返回；错误发生前已合并的数据同样不会回滚。
- `spill: None` 是正常配置，`restore_from_disk` 返回 `Ok(0)`。分区游标耗尽也是正常 EOF，用 `None` 结束循环。
- 空 `result` 生成空结果列表；全局无分组空输入的默认行由执行器显式注入，不是 final worker 自行推断。
- `FinalResult.error` 在本文件的生产路径永远为 `None`，这与 Go worker 可向输出 channel 发送错误对象不同；实际 Rust 错误目前通过方法返回值同步传播。

## 并发与资源生命周期

Partial 阶段确实由 `HashAggExec::execute` 的 `std::thread::scope` 并行运行，但当前 final worker 随后在执行器线程中按顺序调用，并没有为每个 final worker 创建线程或 channel。`&mut self` 使 `result` 的合并和取出串行化；跨 worker 共享的 `aggregations` 不可变，共享 spill helper 自行用 `Mutex` 与 atomics 保护存储、状态和分区领取。

spill 资源的生命周期由 `ParallelHashAggSpillHelper` 持有的 `PartialResultSpill` 管理；恢复会取走分区内容，底层 `Drop` 负责临时文件清理。`restore_from_disk` 克隆 `Arc`，是为了在持有 helper 的同时可变借用 `self` 调用 `merge_input`，不会复制落盘数据。

`generate_result` 通过 move/take 转移所有 group 和 chunk 所有权，避免复制整个结果 map。它把每个满 chunk 移入 `FinalResult`，再用空 chunk 继续累积；函数返回后 worker 不再持有这些行。当前没有取消信号、背压、chunk 归还池、panic recovery 或 worker 等待组；这些只存在于顶部的 Go 对照注释和真实 Go 文件中。

## 与 Go 版本的对应关系

同路径 `agg_hash_final_worker.go` 的总体目的相同：合并 partial 结果、恢复 spill、生成最终 chunk。大致对应关系是 Rust `merge_input` 对 Go `mergeInputIntoResultMap`，Rust `restore_from_disk` 对 Go 的 `getInputFromDisk`/`restoreDataFromDisk` 循环组合，Rust `generate_result` 对 Go `generateResultAndSend`，Rust `FinalResult` 对 Go `AfFinalResult`。

但当前 Rust 不是 Go 实现的逐字段翻译：

- Go final worker 通过 `inputCh`、`outputCh`、`finalResultHolderCh` 和 `finishCh` 异步收发并复用 chunk，Rust 由 `HashAggExec` 同步调用并把 `Vec<FinalResult>` 放入本地队列。
- Go `run` 等待 partial worker `WaitGroup`，`cleanup` 回收 panic 并记 worker 时间；Rust partial scoped threads全部 join 后才创建/驱动 final 合并，没有相应 final worker 线程、failpoint 或 worker timing。
- Go 在 merge/restore 中维护 memory tracker、恢复分区内存、wait/exec 时间和 hash-state rows；Rust final worker 只保存逻辑状态，hash-state rows 由执行器在生成结果前记录，spill helper/partial storage 各自管理落盘资源。
- Go `AppendFinalResult2Chunk` 的错误当前只记录日志后继续；Rust `AggState::result` 没有错误返回。Go 的 restore 错误作为 `AfFinalResult{err}` 发往 channel，Rust则以 `Result<String>` 返回。
- Go 每次恢复一个分区、生成完即释放上一分区的内存；当前 Rust 将所有恢复分区依次合并到第一个 worker 的同一 `result`，直到全部恢复后才统一生成。这保证现有功能测试的结果语义，但峰值内存和并行度不等价于 Go。

因此，顶部 300 余行注释应当作为移植来源和差异线索，而不是已接线行为的证明。

## 扩展指南

- 增加聚合种类时，优先在 `agg_util.rs` 的 `AggState::{merge, result}` 保持 update/merge/final 三阶段一致；若改变状态数量或布局，要同时保持 `Aggregation` 与 `Vec<AggState>` 等宽，并为 `merge_input` 的宽度错误增加独立测试。
- 改变 spill 恢复策略时，要同时审查 `HashAggExec::execute` 的“spill 后所有尾部 map 也落盘”不变量及 helper 的单消费者降序游标。若允许多个 final worker 恢复，必须证明分区只被领取一次、同 key 不跨消费者，或增加最终跨 worker 合并。
- 改变 chunk 切分或输出列次序时，修改点是 `generate_result`；应覆盖 `max_chunk_size` 为 0/1、尾 chunk、零聚合函数、空 map 和多 group。不要在本生产文件内嵌测试；按仓库约定扩展同目录独立的 `agg_hash_executor_test.rs` 或 `agg_spill_test.rs`，必要时新建独立 `agg_hash_final_worker_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 区注册。
- 若要对齐 Go 的流式 final worker，应把 channel、取消、chunk 池、panic/错误传播、统计和内存追踪作为完整生命周期设计，而不是只在 `FinalResult` 增加字段；同时要防止背压等待和 spill 锁产生死锁。
- 若让 `AggState::merge/result` 返回错误，`generate_result` 很可能也应改为 `Result<Vec<FinalResult>, String>`；调用方 `HashAggExec::execute/next` 与相关测试必须同步更新。

主要兼容风险是无 `ORDER BY` 情况下误承诺组顺序、空输入默认 group 退化以及 Go/Rust 错误处理差异；性能风险集中在把所有 spill 分区聚合到一个 worker造成的峰值内存、串行恢复和最终一次性 `Vec<FinalResult>` 分配。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/executor/aggregate/agg_hash_final_worker.rs` 报告 11 个符号；`node --file ... --offset 321 --limit 120` 读取全部可执行实现并报告唯一文件级使用者 `agg_hash_executor.rs`；`query --limit 200` 精确确认 `FinalResult`、`HashAggFinalWorker`、`merge_input`、`restore_from_disk`、`hash_state_rows`、`generate_result`。精确 `callers/callees` 查询在 30 秒窗口内未返回，因此调用边再用以下局部源码搜索核验。
- 生产源码：`pkg/executor/aggregate/agg_hash_final_worker.rs`；直接上游 `agg_hash_executor.rs`；直接下游 `agg_util.rs`、`agg_spill.rs`、`agg_hash_partial_worker.rs`；模块入口 `lib.rs`；crate 清单 `Cargo.toml`。
- Go 对照：`pkg/executor/aggregate/agg_hash_final_worker.go`，并参照 `agg_hash_executor.go` 中 worker 的构造和输出 channel 接线。
- 独立 Rust 测试：`agg_hash_executor_test.rs::spill_merges_tail_rows_with_their_existing_group` 验证 spill 后重叠 group 合并和 hash-state 行数；`agg_spill_test.rs::aggregate_spill_partitions_and_restores_real_states` 验证分区恢复；`spill_uses_go_partition_order_and_worker_hash` 验证分区顺序；`hash_aggregate_matches_grouped_results_and_chunking` 验证多 partial/final worker 的聚合值与 chunk 切分；`hash_aggregate_empty_input_returns_default_group` 验证无分组空输入；`distinct_growth_in_one_group_spills_and_merges_overlapping_workers` 验证 DISTINCT 状态跨 worker spill 合并。
- 本任务只增加说明文档，按计划不运行 Cargo。交付前使用任务指定命令检查目标文件存在且恰有 11 个固定二级标题，并人工复核文档没有把注释中的 Go 并发模型描述成当前 Rust 行为。

# `pkg/executor/aggregate/agg_hash_partial_worker.rs`

## 文件定位

本文件属于 Cargo crate `astersql-executor-aggregate`；crate 入口 `pkg/executor/aggregate/lib.rs` 将其公开为 `agg_hash_partial_worker` 模块。它位于并行 Hash 聚合的 Partial 阶段：`HashAggExecutor::execute` 为输入 chunk 分桶并在线程作用域中创建 `HashAggPartialWorker`，worker 先按分组键累计局部聚合状态，再把内存结果按 Final Worker 数量分片。若配置了内存限制，文件还提供局部结果落盘、分区回读和同键合并所需的 `PartialResultSpill`。

这一 Rust 实现目前有两层数据模型。`HashAggPartialWorker` 使用本 crate 的简化 `AggMap`、`Aggregation`、`Chunk` 和 `Row`；`PartialResultSpill` 则通过 `astersql-executor-aggfuncs` 的类型擦除 `PartialResult`、`Serializer` 与真实 chunk 磁盘容器承载可序列化聚合状态。桥接位于 `pkg/executor/aggregate/agg_spill.rs::ParallelHashAggSpillHelper`。

## 核心职责

- `HashAggPartialWorker::update_partial_result` 对每一输入行编码 group key，保存分组列原值，为每个聚合函数创建并更新一份 `AggState`，同时估算当前 map 的内存增长。
- `HashAggPartialWorker::shuffle_intermediate_data` 使用与 Go `twmb/murmur3.Sum32` 相同的 MurmurHash3 x86-32 算法，将局部 map 确定性分配到 Final Worker。
- `HashAggPartialWorker::spill_remaining` 负责收尾阶段强制交出尚未分片的 map；普通更新路径则在 `ParallelHashAggSpillHelper::set_need_spill` 判定超限时主动 spill。
- `PartialResultSpill::spill_maps` 把真实类型化 partial states 序列化进可复用 chunk，按 key 分区并写入每个分区独立的 `DataInDiskByChunks`。
- `PartialResultSpill::restore_partition` 单次消费一个磁盘分区并反序列化；`restore_merged_partition` 进一步合并跨 chunk 重复 group key。
- `check_chunk_spill` 用“非空且已用字节达到 1 MiB，或 chunk 已满”作为临时序列化 chunk 的刷新条件。

## 主要符号

- `murmur3_sum32(input: &[u8]) -> u32`：公开到 crate 内的无依赖 MurmurHash3 x86-32/seed 0 实现。它处理 4 字节块、0～3 字节尾部和最终 avalanche，供内存 shuffle 与磁盘分区共同使用。
- `HashAggPartialWorker`：公开 worker 类型。`group_columns` 指定 group 列下标；`aggregations: Arc<Vec<Aggregation>>` 在多个 worker 间共享只读聚合描述；`spill` 是可选共享落盘协调器；私有 `map` 保存 `key -> (group row, states)`；`memory_usage` 是触发 spill 的本地估算值。
- `HashAggPartialWorker::new`：建立空 `AggMap` 和零内存计数，不启动线程；线程由 `HashAggExecutor::execute` 创建。
- `update_partial_result(&mut self, chunk: &Chunk) -> Result<(), String>`：逐行更新主入口，可传播 key 编码、列下标、聚合状态更新和落盘错误。
- `shuffle_intermediate_data(&mut self, final_concurrency: usize) -> Vec<AggMap>`：消费当前 map；并发度为 0 时仍创建一个输出桶，避免取模除零。
- `spill_remaining(&mut self) -> Result<(), String>`：map 为空时幂等成功；非空但未配置 helper 时返回明确错误。
- `SPILL_CHUNK_SIZE_THRESHOLD` 与 `check_chunk_spill`：临时磁盘 chunk 的 1 MiB/行容量门槛，按 `UsedMemoryUsage` 而非可复用容量 `MemoryUsage` 判断。
- `TypedPartialResultMap`：`BTreeMap<Vec<u8>, Vec<PartialResult>>`；有序 map 让单次输入 map 的遍历稳定，但跨 worker 的总体写入次序仍由调用者决定。
- `PartialResultSpill`：每分区一个磁盘文件；`partitioned_keys` 是复用的 key 暂存；`temporary` 是所有分区共享、切换所有者前必须 flush 的序列化 chunk；`serializer` 保存序列化辅助状态；`written_bytes` 累计实际磁盘字节增量。
- `PartialResultSpill::{new, spill_maps, restore_partition, restore_merged_partition, disk_bytes, is_empty, chunk_counts}`：分别负责资源建立、写入、一次性回读、回读合并和可观测状态。私有 `flush` 完成单分区写入；`Drop::drop` 关闭全部文件。

## 执行流程

1. `HashAggExecutor::execute` 将输入 chunks 轮询分给不超过输入数的 Partial Worker，并在 scoped thread 中为每桶创建一个 `HashAggPartialWorker`。
2. 对桶内每个 chunk 调用 `update_partial_result`。每行先经 `get_group_key` 得到编码 key，再按 `group_columns` 抽取可用于最终输出的原始 group row；列下标越界立即返回错误。
3. worker 以 `entry(key)` 复用已有状态或按聚合描述数量创建 `AggState::new()`。随后把 states 与 aggregations 按位置 zip，逐项 `AggState::update`。
4. 内存计数用更新前后 states 的 `memory_usage` 差值递增；新 group 额外计入 key 长度。处理完整个 chunk 后才查询 spill helper，故阈值不是逐行硬上限。
5. 若 `set_need_spill(memory_usage)` 返回真，worker 用 `mem::take` 交出整个 map，调用 helper 落盘，并在成功后清零计数；否则 map 留在 worker。
6. 正常内存路径结束时，`shuffle_intermediate_data` 同样 `take` map，以 `murmur3_sum32(key) % bucket_count` 分桶，清零计数并把 `Vec<AggMap>` 交给对应 Final Workers。
7. helper 的 spill 路径把简化 `AggMap` 包成一个可序列化 `SpillEntry` partial state，再调用 `PartialResultSpill::spill_maps`。后者先按同一哈希算法收集各分区 key，逐 key 序列化所有函数状态、最后追加 key 列；chunk 达门槛时 `flush`，且每个分区结束都强制 flush，避免共享临时 chunk 的内容写到错误分区。
8. Final Worker 的 `restore_from_disk` 通过 helper 的原子倒序游标逐分区调用 `restore_partition`。每个磁盘 chunk 的函数列先反序列化，再以最后一列重建 key 和 map；文件随后被关闭并清空元数据，体现一次性消费。
9. 直接使用 `restore_merged_partition` 的调用者会把同一分区多个 chunk/map 中的同键 states 用 `merge_spilled_partial_result` 合并；新键直接转移所有权，旧键按列合并。

## 数据与状态

`AggMap` 的值同时保存 group row 与聚合 states。group row 只在首次见到 key 时从输入行克隆；后续相同 key 不覆盖它。states 数量由 `aggregations.len()` 决定，更新依赖二者位置一致。当前 `update_partial_result` 使用 `zip`，因此它自身不另做宽度断言；宽度由新建逻辑保证，已有 map 只由该 worker 内部维护。

`memory_usage` 是触发 spill 的估算量，不是完整 allocator 或磁盘占用：它累计 state 报告的增长和新 key 字节，不计 group row、map 桶及其他容器开销；使用饱和加减避免整数下溢/溢出。shuffle、成功的阈值 spill、成功的 `spill_remaining` 都将其归零。

磁盘格式每行包含 `function_count` 个序列化 partial-result 列，末列为 group key。`PartialResultSpill::new` 至少建立一个分区、临时 chunk 至少容纳一行，所有列当前以类型码 16 创建。`written_bytes` 按每次 `Add` 前后的文件总字节差累计；恢复不会倒扣，所以它表示生命周期内累计写入量。

`restore_partition` 返回 `Vec<TypedPartialResultMap>`，一份磁盘 chunk 对应一份 map，并同时返回反序列化器报告的 heap memory 总量。`restore_merged_partition` 返回单一合并 map；其 memory 值在初始反序列化内存之上，再加入合并函数报告的增量。

## 依赖与调用关系

上游主链由 `pkg/executor/aggregate/agg_hash_executor.rs::HashAggExecutor::execute` 发起：构造 worker，调用 `update_partial_result`，再调用 `shuffle_intermediate_data`。内存结果交给 `pkg/executor/aggregate/agg_hash_final_worker.rs::HashAggFinalWorker::merge_input`；若任一 worker 已 spill，执行器会确保残余输出也进入 spill 路径，避免同一 group 同时存在于内存 final map 与 restore 消费端。

下游本 crate 依赖是 `agg_util::{get_group_key, AggMap, AggState, Aggregation, Chunk, Row}` 与 `agg_spill::ParallelHashAggSpillHelper`。跨 crate 依赖由 `pkg/executor/aggregate/Cargo.toml` 声明：`astersql-executor-aggfuncs` 提供类型化 partial state 与序列化/合并接口，`astersql-util-chunk` 提供 chunk 及磁盘容器，`astersql-util-serialization` 提供列类型。crate 的常规 dependencies 中包含这三项；大量 Go 兼容依赖仅在 `cfg(windows)` 下声明，与本文件当前无条件编译的这些符号不同。

spill 桥接链为 `ParallelHashAggSpillHelper::spill -> PartialResultSpill::spill_maps`，回读链为 `HashAggFinalWorker::restore_from_disk -> ParallelHashAggSpillHelper::restore_partition -> PartialResultSpill::restore_partition`。此外，`agg_spill_test.rs` 直接调用 `restore_merged_partition` 验证高并发、多函数 DISTINCT 状态与无 spill 基线一致。

RustCodeGraph 对 Go 文件给出的应用主链是 `run -> fetchChunkAndProcess -> updatePartialResult`，收尾为 `run -> finalizeWorkerProcess -> shuffleIntermData`；对 Rust 文件确认了 `restore_merged_partition -> restore_partition`。精确方法级 Rust callers/callees 查询未为所有方法产出静态边，因此上述 Rust 上下游另由模块内直接引用核对，未把索引缺失推断为“无人调用”。

## 错误处理与边界

- group key 编码失败、group 列越界或 `AggState::update` 失败会从 `update_partial_result` 立即返回 `String` 错误；已处理行的 map 修改不会回滚。
- 阈值 spill 先 `take` map 再调用 helper。helper 内 `spill_maps` 会在序列化宽度不匹配、磁盘 `Add` 失败或 serializer panic 时返回错误并清理复用缓冲，但 worker 已不再持有原 map；调用者必须把该错误视为执行失败，不能继续当作成功结果。
- `spill_remaining` 在 map 为空时不要求 helper；非空且 helper 缺失时报 `spill helper is not configured`。它也在 helper 成功后才清零内存计数。
- `check_chunk_spill` 明确排除零行 chunk，即使其预分配容量很大；Reset 后保留的大容量也不会错误触发。非空 chunk 满行或实际已用字节达到阈值均触发。
- `spill_maps` 要求每个 key 的 partial-result 宽度等于 serializer 数量，否则返回 `partial result width mismatch`。内部 `expect` 仅用于它刚从同一 map 收集的 key；panic 仍会被外层 `catch_unwind` 转成错误。
- `restore_partition` 检查分区下标、磁盘读取结果，以及每个函数反序列化后的行数是否等于 chunk 行数。无论成功、返回错误或反序列化 panic，选中分区都会关闭并清空，不能重试读取同一内容。
- `restore_merged_partition` 再检查同 key 的源/目标状态宽度；合并函数错误转成字符串返回。此时该分区已被 `restore_partition` 消费。
- `shuffle_intermediate_data(0)` 和 `PartialResultSpill::new(0, ...)` 都退化为一个桶/分区，避免除零；`restore_partition` 对越界分区返回带编号的错误。

## 并发与资源生命周期

单个 `HashAggPartialWorker` 通过 `&mut self` 串行修改 map 和计数，本文件本身不创建线程。`HashAggExecutor::execute` 为每个输入桶移动一个独立 worker 到 scoped thread；只读聚合描述及 spill helper 通过 `Arc` 共享，线程在 `execute` 返回前全部 join，panic 被转换为 `partial aggregate worker panicked`。

共享 spill helper 在 `agg_spill.rs` 内以 `Mutex<PartialResultSpill>` 串行化磁盘写入/回读，以原子状态协调 spill 阶段，以 `AtomicUsize` 让 Final Worker 仅领取一次每个分区。因此 `PartialResultSpill` 的 `partitioned_keys`、`temporary` 和 `serializer` 无需内部锁；并发安全来自外层互斥，而不是这些字段自身。

`spill_maps` 消费传入 maps，并在成功、错误或 panic 后清空分区 key 缓冲及临时 chunk；每个分区在临时 chunk 改变归属前必定 flush。`restore_partition` 是 single-consumer：读取后调用 `Close` 并清空 chunk offsets、行数、数据大小和缓冲。`Drop` 再次关闭全部文件，保证未显式恢复的文件也释放；关闭操作需具备幂等性。测试还验证 IO 错误和 serializer/deserializer panic 后临时文件清理及后续 retry 行为。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/aggregate/agg_hash_partial_worker.go`。Rust 保留了以下关键语义：1 MiB 的 `SpillChunkSizeThreshold`；`twmb/murmur3.Sum32` 的 seed-0 分区结果；按 key 复用或新建 partial state；按 Final Worker 分片；临时 chunk 达实际使用字节阈值或行容量时落盘；分区切换前强制 flush；spill 后清空内存中 partial maps；错误/异常路径清理复用缓冲。

Rust 当前不是 Go struct 的字段逐一运行时复刻。Go worker 直接持有 input/output/give-back channels、finish channel、wait groups、session/expression context、memory tracker、统计、failpoints 和测试注入字段，并由 `run/fetchChunkAndProcess/finalizeWorkerProcess` 驱动；Rust 的活动 `HashAggPartialWorker` 仅封装同步的 chunk 更新、分片与可选 helper，线程和结果汇合由 `HashAggExecutor::execute` 负责。文件顶部的大段注释保存了早期 Go 结构草图，不是可执行字段或 API，不能据此宣称 Rust 已具备 Go channel/failpoint 生命周期。

另一个结构差异是 Go 在内存阶段已按 final concurrency 维护多个 `partialResultsMap`，Rust 先维护单一 `AggMap`，收尾时再哈希分桶；两者依赖相同 hash 以维持目标 worker 一致。Go 的内存 tracker 记录更广泛的 map、key 和 partial result delta；Rust `memory_usage` 是局部估算。Rust 的 `PartialResultSpill` 将 Go worker 内的 `prepareForSpill/spillDataToDiskImpl/spillRemainingDataToDisk` 拆成独立可复用存储对象，并额外提供类型化 restore/merge API。

## 扩展指南

新增聚合状态时，优先保持 `Aggregation`/`AggState` 的更新、内存报告和 merge 语义一致；若状态需要落盘，还必须在 `astersql-executor-aggfuncs::Serializer` 实现中保证序列化列数、反序列化行数和 `PartialResult` 具体类型一致。应同步扩展独立测试 `pkg/executor/aggregate/agg_hash_partial_worker_test.rs`（内存更新与 hash 分片）和 `pkg/executor/aggregate/agg_spill_test.rs`（阈值、IO/panic 清理、round-trip、重复 key 合并），不要把测试嵌入本生产文件。

修改分区算法必须同时审查 `murmur3_sum32` 的所有调用点、Go `murmur3.Sum32` 对照、`ParallelHashAggSpillHelper` 以及 Final Worker 的 restore 顺序；已有磁盘数据只在一次执行生命周期内使用，但内存 shuffle 和 spill 分区必须始终一致。至少保留已知向量 `"hello" -> 613153351` 和七桶落到索引 6 的回归。

修改 spill 阈值或 chunk 布局时，应关注“大 DISTINCT 状态、行数很少但单行很宽”的风险，继续使用实际 used bytes，确保 key 始终位于 `functions.len()` 列，并维持每个分区结束时 flush。新增文件格式字段需要同步 `new`、`spill_maps`、`restore_partition` 和相应 aggfunc serializer。

修改错误恢复或并发模型时，必须维持三个不变量：临时 key/chunk 在所有退出路径清空；被 restore 的分区最多消费一次；磁盘文件在 restore 或 Drop 后关闭。若绕过外层 helper 的 `Mutex` 直接并发访问 `PartialResultSpill`，需先重新设计其可变缓冲所有权，不能只给现有方法增加共享引用。

## 验证依据

- 生产源码：`pkg/executor/aggregate/agg_hash_partial_worker.rs`（33 个索引符号；重点为 `murmur3_sum32`、`HashAggPartialWorker`、`check_chunk_spill`、`PartialResultSpill` 及其 `Drop`/restore 实现）。
- crate 与入口：`pkg/executor/aggregate/Cargo.toml`、`pkg/executor/aggregate/lib.rs`。
- Rust 调用链：`pkg/executor/aggregate/agg_hash_executor.rs::HashAggExecutor::execute`、`pkg/executor/aggregate/agg_spill.rs::ParallelHashAggSpillHelper::{spill, restore_partition}`、`pkg/executor/aggregate/agg_hash_final_worker.rs::HashAggFinalWorker::{merge_input, restore_from_disk}`。
- Go 对照：`pkg/executor/aggregate/agg_hash_partial_worker.go`，重点核对 `run`、`fetchChunkAndProcess`、`updatePartialResult`、`shuffleIntermData`、`spillDataToDiskImpl`、`CheckChunkSpill`。
- 独立测试：`pkg/executor/aggregate/agg_hash_partial_worker_test.rs` 验证 Go Murmur3 向量与目标 worker；`pkg/executor/aggregate/agg_spill_test.rs` 验证 threshold 的 nonempty/used-bytes/full 条件、跨分区 round-trip、IO 与 panic 清理、并发多类型 DISTINCT spill 后与无 spill 基线一致。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点；`files --filter pkg/executor/aggregate` 定位 20 个相关 Go/Rust 文件；`query` 定位目标 struct/function；`explore` 给出 Go worker 主链及 Rust `restore_merged_partition -> restore_partition`。方法级 Rust callers/callees 对部分符号无输出，已以同模块直接引用补证，未作超出源码的推断。
- 本任务是纯文档分析；按计划不运行 Cargo。最终以任务指定命令确认目标文件存在且恰含 11 个固定二级章节，并人工检查链接路径、符号名、边界与扩展建议均能回溯到上述文件。

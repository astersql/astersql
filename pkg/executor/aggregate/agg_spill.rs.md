# `pkg/executor/aggregate/agg_spill.rs`

## 文件定位

本文件属于 `astersql-executor-aggregate` crate，由 [`lib.rs`](./lib.rs) 以 `pub mod agg_spill` 暴露。它位于并行 Hash 聚合的内存态与磁盘态之间：[`HashAggPartialWorker::update_partial_result`](./agg_hash_partial_worker.rs) 生成并持有 `AggMap`，内存达到阈值后调用本文件的 `ParallelHashAggSpillHelper` 落盘；[`HashAggFinalWorker::restore_from_disk`](./agg_hash_final_worker.rs) 再按分区取回 `AggMap` 并合并；[`HashAggExec::execute`](./agg_hash_executor.rs) 负责创建和共享 helper、协调 partial/final 阶段。

文件第 13–451 行主要是从 Go 版本保留下来的注释化迁移草稿，不参与编译。当前可执行实现始于 `SpillStatus`，并非 Go 文件中 OOM action、tracker、条件变量和逐聚合函数恢复逻辑的完整等价移植。

## 核心职责

- `ParallelHashAggSpillHelper` 为多个 partial worker 提供共享的 spill 状态、磁盘存储互斥访问、错误标志和分区恢复游标。
- `set_need_spill`/`has_enough_data_to_spill` 实现“聚合数据至少达到限额五分之一才值得落盘”的抑制规则，避免过小数据频繁 spill。
- `spill` 把整个 `AggMap` 包装成可由 `astersql-executor-aggfuncs::StateSerializer` 处理的 `SpillEntry`，再委托 `PartialResultSpill::spill_maps` 按 group key 的 Murmur3 哈希写入分区文件。
- `next_partition`/`restore_partition` 提供从最高分区到 0 的单次领取与恢复接口；恢复后的数据仍是 `AggMap`，由 final worker 负责跨 worker 合并。
- `SpillEntry` 及 `write_*`/`read_*` 函数定义当前本地 `Row`、`Value`、`AggState` 的磁盘编码，完整保存 group row、计数、数值、可选值与 DISTINCT 集合。

## 主要符号

- `SpillStatus::{NoSpill, NeedSpill, Spilling, Triggered}`：以 `u8` 存入原子变量的四态标记。`status()` 对未知字节保守映射为 `NoSpill`。
- `ParallelHashAggSpillHelper`：公开 helper。`storage` 是 `Mutex<PartialResultSpill>`；`next_partition` 是独立的原子倒序游标；`status` 和 `error` 分别是 `AtomicU8`、`AtomicBool`；`memory_limit` 是构造时固定的字节限额。
- `ParallelHashAggSpillHelper::new(partition_count, memory_limit)`：构造至少一个分区、一个序列化函数列、每个临时 chunk 1024 行的存储；恢复游标采用 exclusive upper bound，避免无符号数使用 `-1` 哨兵。
- `set_need_spill(memory_usage)`：不足 `memory_limit / 5` 返回 `false`；处于 `Spilling` 时拒绝；已经是 `NeedSpill` 时返回 `true`；其他状态通过 CAS 改为 `NeedSpill`。
- `spill(data)`：消费传入的 `AggMap`，切换 `Spilling`，调用磁盘层，最终切换 `Triggered`；返回写入前的 group 数，错误时同时置 `error`。
- `disk_bytes()`、`buffered_groups()`、`is_empty()`：分别报告累计磁盘写入字节、内存缓冲 group 数（当前恒为 0）和磁盘分区是否无行。
- `next_partition()`：使用 `fetch_update` 原子领取分区，依次返回 `partition_count - 1` 到 `0`，耗尽后永久返回 `None`。
- `restore_partition(partition)`：从指定分区 take 出所有已写 chunk，反序列化 `SpillEntry` 并还原为 `Vec<AggMap>`；底层恢复后关闭并清空该分区文件。
- `set_error()`/`has_error()`：跨 worker 发布和读取粘滞错误标志；本文件没有清除接口。
- `has_enough_data_to_spill(aggregate_bytes, trigger_bytes)`：无状态阈值函数，即 `aggregate_bytes >= trigger_bytes / 5`。
- 私有 `SpillEntry`：把一个 group 的 `Row` 和 `Vec<AggState>` 适配为 `SpillState`。`write_spill` 与 `read_spill` 是其对称编码路径。

## 执行流程

1. `HashAggExec::execute` 在配置了 `spill_limit` 时，用 `final_concurrency` 作为分区数创建一个 `Arc<ParallelHashAggSpillHelper>`，再克隆给所有 partial worker 和 final worker。
2. 每个 partial worker 在 `update_partial_result` 中累计当前 map 的估算内存。处理完一个输入 chunk 后调用 `set_need_spill`；成功时用 `mem::take` 交出整个 map，调用 `spill`，然后把 worker 的内存计数清零。
3. `spill` 先记录 group 数并发布 `Spilling`。每个 `(key, (group, states))` 被转换为只有一个动态 partial result 的 typed map；`StateSerializer { ordinal: 0, template: SpillEntry::default() }` 驱动实际编码。
4. `PartialResultSpill::spill_maps`（定义在 `agg_hash_partial_worker.rs`）以 `murmur3_sum32(key) % partitions` 分区，写满或超过字节阈值时 flush 到 `DataInDiskByChunks`。本文件复用同一个 `murmur3_sum32`，因此写入与 worker 分片规则一致。
5. 若磁盘层返回错误，helper 置错误标志；无论成功或失败，`spill` 都把状态发布为 `Triggered`，再把原错误返回调用方。
6. partial 线程全部 join 后，`HashAggExec::execute` 检查是否发生过 spill。只要任一 worker 触发过，其他 worker 的残留输出也必须经 `spill`，避免同一个 group 一部分直接进入 final worker、另一部分进入恢复路径。
7. 第一个 final worker 调用 `restore_from_disk`，重复 `next_partition`，按最高编号到 0 调用 `restore_partition`；每个恢复出的 map 经 `merge_input` 合并。执行器随后从所有 final worker 生成结果，并在发生过 spill 时增加 `runtime_stats.spill_count`。

## 数据与状态

`AggMap` 的键是编码后的 group key，值为 `(Row, Vec<AggState>)`。落盘时每个 map 项被包进单个 `SpillEntry`，因此 `PartialResultSpill` 看见的函数宽度固定为 1，而不是 SQL 聚合函数个数；`SpillEntry` 内部再保存所有聚合态。这是本地简化聚合模型与通用 typed spill 基础设施之间的适配层。

编码格式按顺序为：group row 长度及每个 `Value`；state 数；每个 state 的 `count`、`number` 存在位和值、`value` 存在位和值、DISTINCT 项数，以及每项的字节键和值。`Value` 标签为 `0..=5`，依次表示 `Null`、`Integer`、`Float`、UTF-8 `Text`、`Bytes`、`Bool`。长度使用 `SerializeInt`，整数、浮点和布尔分别使用 serialization crate 的对应函数。

`next_partition` 与磁盘内容是两个不同生命周期的状态：游标只允许每个分区被领取一次；`restore_partition` 又对底层分区执行 take/close/clear，所以已恢复分区不能再次得到原数据。`disk_bytes` 是累计写入量，恢复后不会递减；`is_empty` 才反映当前文件是否仍有行。`buffered_groups` 恒为 0，表达 `spill` 成功后不在 helper 内保留输入 map，而不是动态统计磁盘内容。

状态转换的实际实现是 `NoSpill/Triggered -> NeedSpill -> Spilling -> Triggered`。`set_need_spill` 在 `NeedSpill` 时幂等返回成功，在 `Spilling` 时返回失败；它也允许从 `Triggered` 再 CAS 到 `NeedSpill`，所以同一执行期间可多轮 spill。当前 Rust 实现没有 Go 的 `maxSpillTimes = 10` 上限。

## 依赖与调用关系

上游调用关系（由 RustCodeGraph 文件/符号查询及源码核对）：

- `agg_hash_executor.rs::HashAggExec::execute` 调用 `new`、`status`、`spill`、`is_empty`，并调用 final worker 的恢复入口。
- `agg_hash_partial_worker.rs::HashAggPartialWorker::{update_partial_result, spill_remaining}` 调用 `set_need_spill` 和 `spill`；该文件同时提供本 helper 包装的 `PartialResultSpill` 和 `murmur3_sum32`。
- `agg_hash_final_worker.rs::HashAggFinalWorker::restore_from_disk` 调用 `next_partition`、`restore_partition`，随后调用自身 `merge_input`。
- `agg_spill_test.rs` 直接覆盖 helper、阈值、分区顺序、真实状态 round-trip、磁盘文件和完整 HashAgg spill 结果。

下游依赖：

- `crate::agg_util::{AggMap, Row, AggState, Value}` 定义内存数据模型。
- `crate::agg_hash_partial_worker::{PartialResultSpill, murmur3_sum32}` 提供磁盘分区存储和稳定哈希。
- `astersql-executor-aggfuncs::{StateSerializer, SpillState, PartialResult}` 提供动态 partial result 及序列化协议。
- `astersql-util-serialization` 提供标量编解码；`PartialResultSpill` 再经 `astersql-util-chunk::DataInDiskByChunks` 管理临时磁盘文件。
- `Cargo.toml` 将 aggfuncs、chunk、execdetails、serialization 声明为 crate 的普通路径依赖；本文件实际直接使用 aggfuncs 和 serialization，chunk 经 `PartialResultSpill` 间接使用。该 manifest 未为 spill 声明独立 feature。

## 错误处理与边界

- `spill` 和 `restore_partition` 把 mutex poison 转换为 `Err("spill storage poisoned")`，并对磁盘/序列化层返回的字符串错误继续传播；两条路径出错都会把共享错误标志置为 `true`。
- `PartialResultSpill` 会捕获 serializer/deserializer panic 并转换成字符串错误，同时清理临时缓冲或已消费的分区文件；相关行为由 `typed_spill_io_error_and_serializer_panic_cleanup_files_and_allow_retry` 验证。
- `disk_bytes` 和 `is_empty` 使用 `Mutex::lock().unwrap()`，因此 mutex 已 poison 时会 panic，与 `spill`/`restore_partition` 的可恢复错误策略不同。
- `restore_partition` 假定每个 restored value vector 至少包含一个、且动态类型必为 `SpillEntry`；`remove(0)` 越界或 `downcast(...).expect(...)` 失败会 panic。这些是不可信/损坏磁盘内容下的边界，而不是 `Result` 错误。
- `read_bytes` 信任序列化长度并直接切片；截断或恶意长度会越界 panic。`read_value` 对未知标签 panic，文本字节不是 UTF-8 时 `expect` panic。因此当前格式面向同进程自产、自消费的临时数据，不是持久化或跨版本兼容格式。
- `new(0, limit)` 仍创建一个分区；`memory_limit < 5` 时整数除法使阈值为 0，任何 usage 都满足触发条件。`restore_partition` 的越界分区由底层返回 `partition N out of range`。
- `SpillEntry::read_spill` 返回的内存增量目前只累计 DISTINCT key 的字节长度，不包含 row、普通 value、容器容量等全部堆内存，不能视为精确内存占用。

## 并发与资源生命周期

helper 设计为放入 `Arc` 后由多个 scoped partial 线程共享。磁盘写入和恢复由同一 `Mutex<PartialResultSpill>` 串行化；状态、错误标志、恢复游标则不需要拿存储锁。状态读使用 Acquire，状态写使用 Release，CAS/游标更新使用 AcqRel，从而让 worker 观察到状态发布；错误标志同样用 Release/Acquire。

多个 worker 可同时通过 `set_need_spill`：看到 `NeedSpill` 的调用者也会得到 `true`，随后都可调用 `spill`。每次实际存储写入由 mutex 串行化，但 `status` 的 `Spilling/Triggered` 写入不与整个多调用者批次绑定；所以该状态更像“发生/正在尝试 spill”的共享信号，不是严格计数的全局互斥状态。主执行路径在 scoped partial 线程全部 join 后才恢复，避免写入与恢复并行。

恢复游标的 `fetch_update` 确保并发领取时每个分区最多返回一次，不过当前 `HashAggExec` 只让第一个 final worker执行恢复。底层 `restore_partition` 在读取结束（包括错误或反序列化 panic 转换成错误）后关闭并清空分区文件；未显式恢复的文件最终由 `DataInDiskByChunks` 的资源生命周期清理。测试通过记录路径并断言恢复或 drop 后文件不存在来验证清理。

## 与 Go 版本的对应关系

直接对照文件是 [`agg_spill.go`](./agg_spill.go)，Go 测试是 [`agg_spill_test.go`](./agg_spill_test.go)。两版共同保留四态概念、五分之一触发阈值、key 哈希分区、倒序分区恢复、共享错误标志，以及 spill 后由 final 阶段合并 partial result 的总体语义。

当前 Rust 实现存在明确差异：

- Go 固定 `spilledPartitionNum = 256`；Rust 由 `HashAggExec` 传入 `final_concurrency`，且至少为 1。
- Go helper 保存 memory/disk tracker、每分区多个 `DataInDiskByChunks`、恢复用聚合函数列表、条件变量和消费/配额日志字段；Rust helper 使用一个 `PartialResultSpill`、固定 `memory_limit` 和原子状态，没有 tracker、日志或等待条件变量。
- Go `ParallelAggSpillDiskAction` 由 memory tracker 超限回调触发，并在 `inSpilling` 时等待；Rust 由 partial worker 处理完 chunk 后主动检查估算内存。
- Go 非并行 `AggSpillDiskAction` 有最多 10 次限制和 fallback action；Rust 文件没有实现这两个 action，注释草稿也不参与编译。
- Go 恢复时按每个真实聚合函数反序列化并调用 final agg function 的 `MergePartialResult`；Rust 先把本地全部 `AggState` 打包进一个 `SpillEntry`，恢复为 `AggMap` 后由 `HashAggFinalWorker::merge_input` 合并。
- Go 的 mutex 同时保护状态、游标和 IO 列表并以 cond 广播 spill 完成；Rust 将存储 mutex、状态原子和游标原子拆开。Rust 的 `set_need_spill` 还显式允许 `Triggered` 后再次触发。

因此文首注释化 Go 代码只能作为迁移来源说明；判断当前行为必须以第 452 行后的 Rust 实现及其调用者为准。

## 扩展指南

- 新增或改变 `Value`/`AggState` 字段时，必须同步修改 `write_value`/`read_value` 或 `SpillEntry::{write_spill, read_spill}`，保证字段顺序和存在位完全对称，并在独立的 `agg_spill_test.rs` 添加所有变体 round-trip、截断/非法标签边界测试。不要把测试内嵌到生产文件。
- 若要让落盘格式跨进程或跨版本使用，应先增加版本头、长度/上限校验和结构化错误，替换当前 `panic`/`expect` 假设；同时明确字节序和向后兼容策略。当前格式只适合临时文件。
- 若改变分区算法或分区数，必须保持 `spill_maps`、worker shuffle 和恢复顺序一致，重点回归 `spill_uses_go_partition_order_and_worker_hash` 以及多 worker 重叠 group 的合并测试。
- 若把恢复并行化，可以复用 `next_partition` 的唯一领取保证，但需要证明多个 final worker 的结果归属、最终输出汇合和错误取消；当前 executor 把全部 restored map 放入第一个 final worker。
- 若对齐 Go 的 OOM action/tracker/等待语义，接入点是 `HashAggExec::execute` 的 helper 创建、`HashAggPartialWorker::update_partial_result` 的触发处及 helper 状态机。应单独实现 action/fallback 和资源 tracker，而不能仅把注释草稿当成已实现功能。
- 修改错误策略时要统一 `spill`/`restore_partition` 的 `Result` 与 `disk_bytes`/`is_empty` 的 `unwrap`，并覆盖 mutex poison、IO 失败、serializer/deserializer panic 后能否重试和临时文件是否释放。
- 性能敏感点包括：整个 `AggMap` 的所有权搬移、每个 entry 的动态装箱、`SpillEntry` 深克隆、单 storage mutex 串行写入、每个字节 key/value 的复制，以及 `Vec::remove(0)`。优化必须保留序列化兼容和错误清理不变量。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点，目标目录中的 `agg_spill.rs` 被识别为含 34 个符号；查询日期为 2026-10-08。
- RustCodeGraph `node --file`：完整读取 `pkg/executor/aggregate/agg_spill.rs` 第 1–769 行，并读取直接调用者 `agg_hash_executor.rs::execute`、`agg_hash_partial_worker.rs::{update_partial_result, spill_remaining, PartialResultSpill}`、`agg_hash_final_worker.rs::restore_from_disk`。
- RustCodeGraph `query`：确认 `ParallelHashAggSpillHelper`、`spill`、`restore_partition`、`next_partition`、`has_enough_data_to_spill`、`SpillEntry`、`write_spill`、`read_spill` 的定义位置；图的宽泛 `callers/callees` 查询未在限定时间内返回，因此调用边又以已索引源码节点直接核对，未据此推测未验证调用。
- crate/模块证据：`pkg/executor/aggregate/Cargo.toml`、`pkg/executor/aggregate/lib.rs`。
- Go 对照证据：`pkg/executor/aggregate/agg_spill.go`，以及 `agg_hash_executor.go`、`agg_hash_partial_worker.go`、`agg_hash_final_worker.go` 中 helper 的创建、spill、恢复和关闭调用；测试语义参考 `agg_spill_test.go`。
- Rust 测试证据：`pkg/executor/aggregate/agg_spill_test.rs` 中 `aggregate_spill_partitions_and_restores_real_states`、`spill_uses_go_partition_order_and_worker_hash`、`spilling_creates_a_disk_file_and_releases_memory_partitions`、`distinct_growth_in_one_group_spills_and_merges_overlapping_workers`、`typed_spill_io_error_and_serializer_panic_cleanup_files_and_allow_retry`、`parallel_typed_distinct_matrix_matches_no_spill_results_for_fifty_thousand_rows`。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前以任务指定命令验证本文恰好包含 11 个固定二级章节，并人工复核链接、符号和现状/草稿边界。

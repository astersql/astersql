# `pkg/executor/aggregate/agg_util.rs`

## 文件定位

本文件是 `astersql-executor-aggregate` crate 的公共聚合数据模型与基础算法层，由模块入口 `pkg/executor/aggregate/lib.rs` 以 `pub mod agg_util` 暴露。它位于执行层的 Hash/Stream 聚合公共路径：partial worker 用它生成分组键并更新中间态，final worker 合并中间态并生成结果，spill 层保存和恢复同一套中间态。

文件前半部（第 18—339 行）是被块注释包围的 Go 移植记录，不参与 Rust 编译；实际 Rust 实现从 `use std::time::Duration` 开始。当前可执行实现是一套进程内的简化值模型，并不是 Go `aggfuncs.PartialResult`、表达式求值、SQL 类型系统和 `execdetails.RuntimeStats` 接口的完整替代品。

`pkg/executor/aggregate/Cargo.toml` 将该目录声明为 `astersql-executor-aggregate`，入口是 `lib.rs`。本文件自身的可执行部分只直接使用 Rust 标准库；其类型会被同 crate 的 spill 代码以及 Hash/Stream 执行器使用，后者再依赖 `astersql-executor-aggfuncs`、`astersql-util-execdetails` 和 `astersql-util-serialization` 等 crate。

## 核心职责

1. 用 `Value`、`Row`、`Chunk` 表示简化的聚合输入与输出，并用 `AggMap` 表示按编码分组键排序的中间结果。
2. 用 `AggKind` 和 `Aggregation` 描述 `COUNT`、`SUM`、`MIN`、`MAX`、`FIRST` 及可选的 `DISTINCT`。
3. 用 `AggState` 保存单个聚合函数的 partial/final 状态，提供逐行 `update`、跨 worker `merge`、最终 `result` 与内存估算 `memory_usage`。
4. 用 `get_group_key`/`encode_value` 为指定分组列生成带类型标签、无拼接歧义的字节键。
5. 用 `AggWorkerStat` 与 `HashAggRuntimeStats` 保存轻量运行时统计，并以追加独立 worker 样本的方式合并多次统计。

本文件不负责调度线程、切分 chunk、执行 spill I/O 或管理执行器生命周期；这些职责分别位于 `agg_hash_executor.rs`、partial/final worker、`agg_spill.rs` 和 `agg_stream_executor.rs`。

## 主要符号

- `Value`：公开枚举，包含 `Null`、`Integer(i64)`、`Float(f64)`、`Text(String)`、`Bytes(Vec<u8>)`、`Bool(bool)`。字符串与字节值拥有其缓冲区。
- `Row = Vec<Value>`、`Chunk = Vec<Row>`：公开的行与批次别名。
- `AggMap = BTreeMap<Vec<u8>, (Row, Vec<AggState>)>`：键是编码后的 group key；值保存分组列原值和与聚合描述一一对应的状态列表。`BTreeMap` 使遍历按字节键有序，但 Hash 聚合跨 final worker 的总体输出顺序仍不应视为 SQL 契约。
- `AggKind`：公开聚合种类。`First` 只保留第一次传入的值；当前实现没有 `AVG`、方差、字符串聚合等种类。
- `Aggregation::{new,new_distinct}`：分别构造普通和 DISTINCT 描述；`column: None` 会在更新时使用常量 `Integer(1)`，用于 `COUNT(*)` 一类路径。
- `AggState::{new,memory_usage,update,merge,result}`：聚合状态的生命周期 API。`count`、`number`、`value` 是公开字段；`distinct_values` 仅 crate 内可见，供 spill 序列化。
- `get_group_key`：按 `group_columns` 顺序读取行列并编码，列越界时返回 `Err(String)`。
- `encode_value`、`as_number`、`compare_values`：私有辅助函数，分别负责键编码、SUM 数值转换与 MIN/MAX 比较。
- `AggWorkerStat`：单个 worker 的工作、等待、执行时长和任务数。
- `HashAggRuntimeStats::merge`：追加 partial/final worker 样本并累加 `spill_count`。

本文件没有 trait、条件编译项或真实静态缓冲池。注释中的 Go 常量、pool、panic recovery、spill action、failpoint 和时间更新函数均不是 Rust 符号。

## 执行流程

Hash 聚合的实际调用链如下：

1. `HashAggExec::execute`（`agg_hash_executor.rs`）为输入 chunk 分配 partial worker。
2. `HashAggPartialWorker::update_partial_result` 对每行调用 `get_group_key`，从分组列构造 `group_row`，为新组创建与 `aggregations` 等宽的 `Vec<AggState>`，随后逐项调用 `AggState::update`。
3. `AggState::update` 从 `Aggregation::column` 取值；列不存在或 `column` 为 `None` 时当前实现都落到 `Integer(1)`。DISTINCT 路径先用 `encode_value` 构造键，若已存在则直接成功返回，否则写入 `distinct_values` 并执行 `update_value`。
4. `update_value` 按聚合种类更新：COUNT 忽略 NULL；SUM 只累加整数/浮点并保存为 `f64`；MIN/MAX 忽略 NULL 并用 `compare_values`；FIRST 仅在状态为空时写入，包含首次值为 NULL 的情况。
5. partial map 被分发或 spill。`HashAggFinalWorker::merge_input` 检查状态宽度后，对同组同位置调用 `AggState::merge`。DISTINCT merge 逐键去重后重新更新目标状态；普通 merge 直接合并计数、和或极值。
6. `HashAggFinalWorker::generate_result` 和 Stream 聚合的 `append_group` 调用 `AggState::result`，将分组列与最终值拼接成结果行。COUNT 被限制在 `i64::MAX`，无值的 SUM/MIN/MAX/FIRST 返回 `Value::Null`。

Stream 聚合走同一状态机，但 `StreamAggExec::consume_groups` 要求输入已按分组键连续排列；检测到 key 改变时输出上一组并重建状态。空 `group_columns` 时，`get_group_key` 返回空字节向量，使全部输入属于同一组。

## 数据与状态

`AggState` 的三个结果槽互相独立：COUNT 使用 `count`，SUM 使用 `number`，MIN/MAX/FIRST 使用 `value`。调用方必须以创建该状态时相同的 `Aggregation.kind` 更新、合并和取结果；类型本身不记录 kind，也不会阻止以错误 kind 读取同一状态。

DISTINCT 的所有权状态保存在 `BTreeMap<Vec<u8>, Value>` 中：编码键用于判重，拥有的 `Value` 用于 final merge 时重放 `update_value`。`memory_usage` 估算结构体、最终 `value` 的 Text/Bytes 堆内存、每个 DISTINCT 键和值及元组大小；它不是分配器级精确值，但 partial worker 用其增量决定是否 spill。`agg_spill.rs` 的 `SpillEntry` 会序列化并恢复 `count`、`number`、`value` 和完整 `distinct_values`，因此落盘后仍能正确做 DISTINCT final merge。

分组键格式由 `encode_value` 固定：先写一字节变体标签；整数写小端 i64；浮点写 `to_bits()` 的小端字节；Text/Bytes 写 u64 长度再写负载；Bool 再写 0/1。长度前缀避免相邻可变长值拼接冲突，类型标签避免不同 `Value` 变体冲突。该格式是本地执行器内部格式，代码没有声明跨版本持久化兼容性。

`HashAggRuntimeStats` 拥有两个 worker 样本向量和 spill 次数。`merge` 使用 `extend_from_slice` 保留每轮的独立样本，而不是按 worker 下标相加；`pkg/executor/test/aggregate/aggregate_test.rs::test_hash_agg_runtime_stat` 明确验证该不变量，因为按下标相加会错误放大 max/p95。

## 依赖与调用关系

RustCodeGraph 的文件节点显示本文件被 14 个文件引用；精确符号查询确认 `AggState`、`get_group_key`、`Aggregation` 和 `HashAggRuntimeStats` 的定义均位于本文件。主要生产调用边为：

- `agg_hash_partial_worker.rs::HashAggPartialWorker::update_partial_result` → `get_group_key`、`AggState::new`、`AggState::memory_usage`、`AggState::update`。
- `agg_hash_final_worker.rs::HashAggFinalWorker::merge_input` → `AggState::new`、`AggState::merge`；`generate_result` → `AggState::result`。
- `agg_stream_executor.rs::StreamAggExec::consume_groups` → `get_group_key`、`AggState::new`、`AggState::update`；`append_group` → `AggState::result`。
- `agg_hash_executor.rs::HashAggExec` 持有 `HashAggRuntimeStats`，在 `open` 重置，在发生 spill 时递增 `spill_count`；空输入且无 GROUP BY 时创建空 `AggState` 以生成默认聚合行。
- `agg_spill.rs::SpillEntry` 持有 `Row` 和 `Vec<AggState>`，通过 `astersql-util-serialization` 将全部状态字段落盘/恢复。

测试与基准也直接构造这些公开类型，包括 `pkg/executor/aggregate/agg_spill_test.rs`、`pkg/executor/test/aggregate/aggregate_test.rs`、`pkg/executor/benchmark_test.rs` 和 `pkg/executor/executor_required_rows_test.rs`。

RustCodeGraph 的 `callers/callees` 精确边查询在当前本地索引上超时且未返回结果；上述调用边由 RustCodeGraph 的文件级“used by”结果、索引源码节点和 `rg` 引用结果交叉核对，不依赖命名猜测。

## 错误处理与边界

- `get_group_key` 对任一分组列越界返回 `"group column {column} out of range"`。该错误由 Hash/Stream 路径的 `?` 原样传播；`aggregate_test.rs::test_random_panic_consume` 验证 Hash 执行器的错误文本。
- `AggState::update` 的签名是 `Result<(), String>`，但当前唯一内部操作不产生错误；更重要的是，指定聚合列越界会通过 `and_then(...).unwrap_or(Integer(1))` 静默变成常量 1。这与 group column 越界的显式错误不同，也是扩展时必须谨慎保留或修正的现行行为。
- COUNT 忽略 NULL；SUM 忽略非 Integer/Float；MIN/MAX 忽略 NULL。FIRST 不忽略 NULL，第一次输入为 NULL 时 `value` 变成 `Some(Value::Null)`，后续值不会替换它。
- DISTINCT 使用编码位模式判等：例如不同浮点位模式的行为由 `f64::to_bits` 决定。普通 SUM 将 i64 转为 f64，可能损失大整数精度，且没有溢出/NaN/SQL Decimal 处理。
- 同型值按自然序比较，Float 用 `total_cmp`；跨类型 MIN/MAX 回退到 `Debug` 字符串字典序。这是确定性的简化规则，不等于 TiDB 的 SQL 类型强制转换、collation 或 NULL 排序规则。
- `AggState::merge` 不返回错误；宽度不一致由 final worker 在调用前检查并返回 `"partial result width mismatch"`。
- COUNT 结果在超过 i64 上限时饱和到 `i64::MAX`；SUM 的 f64 运算没有额外的有限值或溢出检查。

## 并发与资源生命周期

本文件本身不创建线程、不持有锁、不打开文件，也没有异步任务或 channel。`AggState` 与统计类型由调用方独占可变借用；它们没有内部同步原语。Hash 执行器通过 `std::thread::scope` 把各 worker 的独立 map 分开处理，join 后再由 final worker 顺序合并，因此本文件的方法无需自行加锁。

拥有型 `String`、`Vec<u8>`、`Vec` 和 `BTreeMap` 随结构体释放。DISTINCT 会按不同值数量持续增长；partial worker 以 `memory_usage` 监控增长并在阈值到达时把整张 map 移交给 spill helper。spill 的文件/分区生命周期由 `ParallelHashAggSpillHelper` 管理，不在本文件中。

统计合并会克隆并追加 worker 样本，时间和空间均随样本数线性增长。`HashAggExec::open` 清空上一轮统计；执行中只在主执行对象上递增 `spill_count`。Go 注释中的 atomic wall time、buffer pool、panic recovery 和 spill action 没有对应的可执行 Rust 实现，不能据此推断 Rust 线程安全或资源回收行为。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/aggregate/agg_util.go`。Rust 文件顶部保留了 Go 文件的大部分控制流作为注释，但真实实现只覆盖其中一部分职责，并采用不同抽象：

- Go `GetGroupKey` 对整个 `chunk.Chunk` 向量化求值表达式，通过 SQL 类型、时区和错误上下文调用 `codec.HashGroupKey`，并专门处理 ENUM 与 DECIMAL；Rust `get_group_key` 只对单行的列下标编码简化 `Value`，没有表达式、时区、collation、SQL mode、ENUM/DECIMAL 或 warning/error context。
- Go 使用 `aggfuncs.PartialResult` 与完整聚合函数体系；Rust 在本文件内实现五种 `AggKind` 和统一 `AggState`。因此二者职责相似但数据模型并非一一同构。
- Go 提供 partial result/group key 的 `sync.Pool`、容量上限回收、group key 内存统计、Open panic 清理与 hash-agg panic 转错；Rust 可执行部分均未实现这些能力。
- Go `HashAggRuntimeStats` 实现 `String`、`Clone`、`Merge`、`Tp`，含 concurrency/wall-time 与 worker max/p95；Rust 仅依赖派生 `Clone`，保留 worker 样本与 spill 次数并提供 `merge`，未实现 Go runtime stats 接口或字符串输出。
- Go 的 spill action 选择、随机 failpoint 和 worker 时间更新函数只存在于 Rust 注释中；Rust spill 触发、序列化和恢复实现在相邻执行器与 `agg_spill.rs`。

已验证的共同语义包括：聚合中间态按 partial/final 阶段合并、独立 worker 统计样本在 merge 时追加，以及 group key 用于把等值分组送往同一聚合状态。未验证且不可宣称等价的部分包括完整 SQL 类型语义、表达式错误处理、Go 的并发恢复机制和 runtime stats 展示协议。

## 扩展指南

- 新增 `AggKind` 时，必须同步修改 `AggState::update_value`、`merge`、`result`，判断是否需要新状态字段，并更新 `memory_usage` 与 `agg_spill.rs::SpillEntry::{write_spill,read_spill}`；否则内存阈值或 spill 恢复会丢状态。测试应放在独立测试文件，例如扩展 `agg_spill_test.rs` 或 `pkg/executor/test/aggregate/aggregate_test.rs`，不要把测试内嵌到本源文件。
- 新增 `Value` 变体时，必须同步 `encode_value`、`compare_values`、必要时 `as_number`/`memory_usage`，以及 `agg_spill.rs::{write_value,read_value}`。变更标签或编码格式会影响分组与 DISTINCT 判等，也可能影响正在使用该格式的 spill 数据。
- 改动 group key 时要同时验证 Hash 和 Stream 路径、空 group columns、多列可变长值、类型区分、浮点特殊值和列越界。若目标是对齐 Go，应从表达式求值、SQL FieldType/codec 与错误上下文接线，而不是只在当前简化编码上添加特例。
- 改动 DISTINCT 时必须覆盖单 worker 去重、多个 partial state 的重叠 merge、同一组增长触发 spill及恢复；`agg_spill_test.rs::distinct_growth_in_one_group_spills_and_merges_overlapping_workers` 是最近的回归入口。
- 改动运行时统计时应保持“追加独立样本”的不变量，并同步 `aggregate_test.rs::test_hash_agg_runtime_stat`。若接入 Go 风格 String/Tp/atomic wall time，应在 `astersql-util-execdetails` 的真实接口上实现并添加独立测试，不能把顶部注释视作接口实现。
- 性能风险集中在每行重新分配 group key、DISTINCT 的 BTreeMap 插入与克隆、跨类型比较的字符串格式化，以及统计 merge 的样本复制；兼容风险集中在键编码、数值精度、NULL/FIRST 和 SQL 类型排序语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/executor/aggregate/agg_util.rs` 确认目标文件含 36 个符号；`node --file ... --offset ...` 阅读了全部 636 行；`query` 确认 `AggKind`、`Aggregation`、`AggState`、`get_group_key`、`AggWorkerStat`、`HashAggRuntimeStats` 的定义与候选。两次带文件限定的 `callers/callees` 查询超时，未将其当作完成证据。
- 源码与模块边界：`pkg/executor/aggregate/agg_util.rs`、`pkg/executor/aggregate/lib.rs`、`pkg/executor/aggregate/Cargo.toml`。目标目录不存在 `doc.go`，因此无额外 Go package contract 可读。
- 生产调用点：`agg_hash_partial_worker.rs::update_partial_result`、`agg_hash_final_worker.rs::{merge_input,generate_result}`、`agg_hash_executor.rs::{open,execute}`、`agg_stream_executor.rs::consume_groups`、`agg_spill.rs::SpillEntry`。
- Go 对照：`pkg/executor/aggregate/agg_util.go`，核对了 buffer pool、`GetGroupKey`、runtime stats、spill action、failpoint 与 worker 计时函数。
- Rust 独立测试：`pkg/executor/test/aggregate/aggregate_test.rs::{test_hash_agg_runtime_stat,test_sum_int_distinct,test_random_panic_consume}`；`pkg/executor/aggregate/agg_spill_test.rs::{aggregate_spill_partitions_and_restores_real_states,hash_aggregate_matches_grouped_results_and_chunking,hash_aggregate_empty_input_returns_default_group,stream_aggregate_preserves_sorted_group_boundaries,distinct_growth_in_one_group_spills_and_merges_overlapping_workers}`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核所有“已实现”结论均来自可执行 Rust 代码而非注释占位。

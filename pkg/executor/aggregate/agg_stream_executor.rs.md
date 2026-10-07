# `pkg/executor/aggregate/agg_stream_executor.rs`

## 文件定位

该文件属于 `astersql-executor-aggregate` crate；crate 的入口 `pkg/executor/aggregate/lib.rs` 以 `pub mod agg_stream_executor` 公开本模块。它提供当前 Rust 侧可执行的 `StreamAggExec`：在输入已按分组键连续排列的前提下，顺序扫描行、在键变化处完成上一组聚合，并把结果切成若干 `Chunk` 供调用方读取。

当前接线范围需要谨慎理解：仓库内 `StreamAggExec::new` 的直接 Rust 调用位于独立测试和 benchmark（`pkg/executor/executor_required_rows_test.rs`、`pkg/executor/aggregate/agg_spill_test.rs`、`pkg/executor/test/aggregate/aggregate_test.rs`、`pkg/executor/benchmark_test.rs`）。`pkg/executor/builder.rs` 虽定义了 `StreamAggPlanData`、`StreamAggBuildConfig` 和依赖注入式的 `build_stream_agg_executor`，但搜索不到它直接构造本文件的 `StreamAggExec`；因此不能把本实现描述成已经完整接入主执行器 trait/会话执行链。

文件第 21—106 行及末尾第 254 行是从 Go 控制流保留下来的注释性迁移材料，不参与 Rust 编译。真正生效的实现从 `use crate::agg_util...` 开始。

## 核心职责

- `StreamAggExec` 保存输入 chunks、分组列下标、聚合描述、最大输出 chunk 大小和生命周期状态。
- `open` 一次性扫描全部输入并物化所有结果，而不是像 Go `Next` 那样按需从 child executor 拉取。这一实现仍利用“相同分组必须相邻”的条件，仅保留一组 `AggState`，但所有输出会暂存在 `results: VecDeque<Chunk>` 中。
- `consume_groups` 负责识别组边界、更新每个聚合状态、生成结果行并按 `max_chunk_size` 切块。
- `next` 提供按内部 chunk 读取的接口；`next_required` 提供调用方指定行数的读取接口，并保留未消费的 chunk 后缀。
- `close` 清空物化结果并复位生命周期状态，使同一个实例可再次 `open`。

该文件不负责排序。调用者必须保证输入按 `group_columns` 对应的键排序；`pkg/executor/benchmark_test.rs::build_stream_aggregate_executor` 展示了未排序数据先经 `SortExec` 的用法。

## 主要符号

- `STREAM_AGG_MEM_DELTA_FLUSH_THRESHOLD: usize = 1 << 10`：与 Go 的 1 KiB 内存增量批量上报阈值同值，但当前可执行 Rust 路径没有内存 tracker，也没有读取该常量；它是公开的迁移契约，而非当前运行时行为。
- `StreamAggExec`：核心状态对象。其字段均为私有：
  - `input: Vec<Chunk>` 拥有全部输入；`Chunk`/`Row` 分别是 `Vec<Row>`/`Vec<Value>` 的别名。
  - `group_columns: Vec<usize>` 指定构造分组键和结果行前缀所用的列及顺序。
  - `aggregations: Vec<Aggregation>` 描述聚合种类、输入列和 DISTINCT 属性。
  - `max_chunk_size` 决定内部结果切块；构造时通过 `.max(1)` 保证不为零。
  - `opened` 控制读取前置条件；`results` 保存尚未读取的结果 chunks。
- `StreamAggExec::new`：接收并取得输入、分组列和聚合描述的所有权，不做排序或列下标预验证。
- `StreamAggExec::open`：清理旧结果，调用 `consume_groups`，仅在成功后设置 `opened = true`。
- `StreamAggExec::close`：丢弃未读结果并设置 `opened = false`。
- `StreamAggExec::next`：要求已打开，然后从 `results` 队首弹出整个内部 chunk；耗尽时返回 `Ok(None)`。
- `StreamAggExec::next_required`：最多返回 `required_rows` 行；零请求返回空 chunk 且不消费队列。若只消费了队首 chunk 的前缀，将剩余后缀重新放回队首。
- `StreamAggExec::consume_groups`：私有单遍聚合主循环。
- `append_group`：把分组列值与每个 `AggState::result` 拼接成一行并追加到输出 chunk。

## 执行流程

1. 调用方以已经按分组键排序的 `Vec<Chunk>` 调用 `StreamAggExec::new`。构造函数把 `max_chunk_size == 0` 规范化为 1。
2. `open` 先清空可能残留的 `results`，然后进入 `consume_groups`。若扫描报错，错误直接返回，`opened` 不会被置为 `true`。
3. `consume_groups` 为每个 `Aggregation` 创建一个空 `AggState`，并用 `self.input.iter().flatten()` 跨 chunk 顺序遍历所有行；chunk 边界本身不会形成组边界。
4. 每行先由 `get_group_key(row, &group_columns)` 生成带类型标签的字节键。若键不同于 `current_key`，先通过 `append_group` 输出上一组，再重建全部聚合状态。
5. 遇到新键时，从当前行按 `group_columns` 的顺序克隆分组列，形成结果行的前缀；随后每个 `(state, aggregation)` 调用 `AggState::update` 消费当前行。
6. 当已完成的输出行数达到 `max_chunk_size`，把当前输出 chunk 移入 `results` 队尾。扫描结束后，若确实见过分组，或者 `group_columns` 为空，则追加最后一组；最后把非空输出 chunk 入队。
7. `next` 每次弹出一个内部 chunk。`next_required(n)` 则可跨越多个内部 chunks 收集最多 `n` 行，并将未用后缀放回队首，因此调用方请求大小不会丢行或提前消费行。
8. `close` 清空队列。再次 `open` 会从仍然拥有的 `input` 重新计算结果；`pkg/executor/benchmark_test.rs::benchmark_aggregate_executor_reuses_open_next_close_lifecycle` 对这一复用路径有覆盖。

## 数据与状态

`opened` 与 `results` 构成简单生命周期状态机：新建后不可读取；成功 `open` 后可读取；结果耗尽并不会自动关闭，后续读取仍返回空；`close` 后再次读取会报错。重复调用 `open` 会重新计算，并用 `results.clear()` 丢弃此前未读结果。

分组不变量是“同一个编码键的所有行必须连续”。实现只比较当前键和上一键，不维护全局键集合；如果相同键在输入中分成不相邻的两段，会输出两个组，文件不会检测或合并它们。

结果行布局固定为“按 `group_columns` 顺序克隆的分组值 + 按 `aggregations` 顺序生成的最终值”。键编码和状态行为来自 `pkg/executor/aggregate/agg_util.rs`：`get_group_key` 为不同 `Value` 变体写入类型标签；`AggState::update` 处理 DISTINCT 去重及 COUNT/SUM/MIN/MAX/FIRST；`AggState::result` 把空 SUM/MIN/MAX/FIRST 映射为 `Value::Null`，COUNT 映射为整数。

无 `GROUP BY` 时，所有行共享空键。即使输入完全为空，`group_columns.is_empty()` 也会追加一行空聚合结果；例如 COUNT 为 0，其余当前支持的空聚合结果通常为 NULL。相反，有分组列且输入为空时不输出任何行。当前 Rust 实现没有 Go `DefaultVal` 字段，上述行为直接来自空 `AggState`。

## 依赖与调用关系

直接下游依赖只有标准库 `VecDeque` 和同 crate 的 `agg_util::{AggState, Aggregation, Chunk, Row, get_group_key}`。`consume_groups -> get_group_key` 生成分组键，`consume_groups -> AggState::update` 更新聚合态，`append_group -> AggState::result` 生成最终值。

crate 边界由 `pkg/executor/aggregate/Cargo.toml` 定义，包名为 `astersql-executor-aggregate`，库入口为 `lib.rs`。该 manifest 声明了 aggfuncs、chunk、execdetails、serialization 等 crate 依赖，并在 Windows target 下保留更完整的 Go 移植依赖；不过本文件当前生效代码没有直接使用这些外部 crate，而使用 `agg_util` 内的简化数据模型。

直接上游证据包括：

- `pkg/executor/executor_required_rows_test.rs`：构造执行器并验证 `next_required` 的行数与剩余保留语义。
- `pkg/executor/aggregate/agg_spill_test.rs`：验证已排序分组跨行聚合和 `max_chunk_size = 1` 的逐组输出。
- `pkg/executor/test/aggregate/aggregate_test.rs`：覆盖小批次 COUNT 以及不同输出 chunk 大小下 COUNT/SUM/MIN/MAX 的结果一致性。
- `pkg/executor/benchmark_test.rs`：构造前置排序路径、benchmark 消费路径和 `open/next/close` 复用生命周期。

RustCodeGraph 对目标文件的文件节点报告 12 个符号，并把 `agg_spill_test.rs`、`benchmark_test.rs` 标为使用者；普通精确引用搜索进一步补充了索引未列出的 `executor_required_rows_test.rs` 与 `test/aggregate/aggregate_test.rs`。

## 错误处理与边界

- `next` 和 `next_required` 在未成功 `open` 时返回 `Err("stream aggregate is not open")`；`close` 后也适用。
- `get_group_key` 会在任一分组列越界时返回 `group column {index} out of range`。
- 构造新组的结果前缀时再次读取分组列；越界同样返回上述格式的错误。正常情况下第一次 `get_group_key` 已会先捕获这个错误。
- 聚合输入列越界不会报错：`AggState::update` 通过 `and_then(row.get(...)).unwrap_or(Value::Integer(1))` 回退为整数 1。这是共享 `agg_util` 的当前语义，扩展或接线时不能误认为已做 schema 校验。
- `open` 出错前可能已生成部分 `results`，但由于 `opened` 仍为 false，调用者不能读取；下一次 `open` 会先清空它们。
- `next_required` 的 `required_rows == 0` 是显式边界：返回空 chunk 且不弹出队列。相关回归在 `stream_aggregate_zero_required_rows_does_not_consume_a_group`。
- 输入排序是调用契约而非运行时校验。违反契约不会报错，只会把不相邻的同键行当作不同组。
- 当前错误类型是 `String`，没有 Go 侧的 context 取消、表达式错误上下文、failpoint、child executor 错误栈或统一 executor error 类型。

## 并发与资源生命周期

该类型没有 `Send` 任务、线程、锁、原子量、channel 或异步 I/O；所有可变操作都要求 `&mut self`，同步串行执行。`input` 和聚合描述由执行器独占，分组值与聚合结果按需克隆。

尽管流式分组状态只保留当前组，`open` 会一次性生成全部输出并存入 `VecDeque`，所以当前实现的结果内存规模随输出组数增长，并非 Go 版本真正的拉取式常量输出缓冲。`next_required` 对队首 chunk 使用 `drain(..take)`；若有剩余，保留原顺序并放回队首。

资源释放仅依赖 Rust 所有权和 `Vec`/`VecDeque` 析构。`close` 主动清空结果，但不释放或替换输入；因此实例能够重新打开。文件没有内存 tracker、spill、child executor close、panic 清理或取消信号。公开的 1 KiB 阈值在当前生效路径中未使用，不应据此宣称已实现 Go 的内存增量批处理。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggregate/agg_stream_executor.go`。两者保持的核心意图是：输入按 group key 排序、跨 child chunk 延续同一组、组结束后输出聚合结果、支持 executor 的打开/读取/关闭生命周期。Rust 的 `get_group_key` + `current_key` 对应 Go `VecGroupChecker.SplitIntoGroups/GetNextGroup` 所承担的组边界识别；Rust `AggState::update/result` 对应 Go `AggFunc.UpdatePartialResult/AppendFinalResult2Chunk` 的简化模型。

重要差异如下：

- Go 嵌入 `BaseExecutor`，从 child executor 按需取 chunk；Rust 构造时直接拥有 `Vec<Chunk>`，并在 `open` 中全量物化输出。
- Go `Next` 服从 `chunk.RequiredRows` 且边拉取边产出；Rust 将整 chunk 的 `next` 与限制行数的 `next_required` 分成两个 API。
- Go 使用表达式求值、`VecGroupChecker` 和完整 `aggfuncs.AggFunc`；Rust 使用列下标和 `agg_util` 的简化 `Value`/`Aggregation`/`AggState`。
- Go 跟踪 child chunk、初始 partial result 和每组增长的内存，并以 1 KiB 阈值批量 `Consume`；Rust 没有 tracker，阈值常量未使用。Go 对应回归 `pkg/executor/test/aggregate/aggregate_test.go::TestStreamAggPendingMemDeltaBatching` 因而不能作为 Rust 已支持该行为的证据。
- Go 支持 context、failpoint、child 打开/关闭、panic 清理和 `DefaultVal`；Rust 均未接线。
- Go 空输入的默认结果由 builder 条件和 `DefaultVal` 控制；Rust 对无分组列总是生成一行空状态结果。

因此，该文件是可运行且有独立测试覆盖的简化 Rust 流聚合实现，但不是 Go `StreamAggExec` 的字段级或运行时资源语义完整复刻。顶部大段注释准确保存了部分 Go 迁移线索，不应与生效代码混为一谈。

## 扩展指南

- 增加新的聚合种类时，先在 `pkg/executor/aggregate/agg_util.rs` 同步 `AggKind`、`AggState::update_value`、`AggState::merge` 和 `AggState::result`；本文件的状态向量与结果拼接通常无需改动。测试应放在独立 `*_test.rs` 文件，不要内嵌到生产源文件。
- 改变分组语义时，应同时检查 `get_group_key` 的类型编码和 `consume_groups` 的相邻键比较；必须保留“输入先排序”的契约，或显式增加检测/排序，不能静默把流聚合变为哈希聚合。
- 接入真实 executor 主链时，最可能修改 `StreamAggExec` 的输入所有权、`open/next/close` 签名和 `consume_groups` 的拉取方式，并与 `pkg/executor/builder.rs::buildStreamAggFromChildExec` 的依赖工厂对接。需要新增/同步独立测试来覆盖 child 错误、context 取消、跨 chunk 同组、空输入默认行和关闭资源。
- 补齐 Go 内存语义时，应实现 partial state 基线、pending delta 阈值刷入、结果追加后的基线替换及关闭时归还，并对照 Go 的 `TestStreamAggPendingMemDeltaBatching`；仅保留 `STREAM_AGG_MEM_DELTA_FLUSH_THRESHOLD` 常量不等于完成。
- 修改结果分块时，必须同时验证 `next` 的内部 chunk 行为与 `next_required` 的调用方行数契约，尤其是零请求、跨 chunk 收集、队首剩余后缀和耗尽后的重复读取。
- 性能风险集中在 `open` 全量物化、分组值克隆、每行重新编码 group key、每组重建状态，以及 `Vec::drain` 移动后缀；兼容风险集中在空输入、NULL、DISTINCT、数值转换和错误类型与 Go 的差异。

建议至少同步或扩展以下独立测试：`pkg/executor/executor_required_rows_test.rs`（请求行数契约）、`pkg/executor/aggregate/agg_spill_test.rs`（跨行分组边界）、`pkg/executor/test/aggregate/aggregate_test.rs`（聚合种类和输出大小一致性）、`pkg/executor/benchmark_test.rs`（排序前置及生命周期复用）。若实现 Go 级语义，还需以同名 Go 测试为对照新增 Rust 回归，而不是声称现有简化测试已覆盖。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标 `pkg/executor/aggregate/agg_stream_executor.rs` 已索引；`files --filter` 返回该文件；`node --file ... --offset 1 --limit 500` 展示完整 254 行和生效符号；精确 `explore` 未建立可靠的完整静态调用路径，因此调用边以文件节点使用者和精确引用搜索交叉核对。
- 生产源码：`pkg/executor/aggregate/agg_stream_executor.rs` 的 `StreamAggExec::{new,open,close,next,next_required,consume_groups}`、`append_group` 与公开阈值；`pkg/executor/aggregate/agg_util.rs` 的 `Value`、`Row`、`Chunk`、`Aggregation`、`AggState::{new,update,result}` 和 `get_group_key`。
- crate 与装配：`pkg/executor/aggregate/Cargo.toml`、`pkg/executor/aggregate/lib.rs`；主链接线边界参考 `pkg/executor/builder.rs::{StreamAggPlanData,StreamAggBuildConfig,buildStreamAgg,buildStreamAggFromChildExec}`。
- Rust 测试/benchmark：`pkg/executor/executor_required_rows_test.rs::stream_aggregate_required_rows_are_preserved_between_calls`、`stream_aggregate_zero_required_rows_does_not_consume_a_group`；`pkg/executor/aggregate/agg_spill_test.rs::stream_aggregate_preserves_sorted_group_boundaries`；`pkg/executor/test/aggregate/aggregate_test.rs::{test_parallel_stream_agg_group_concat,test_issue20658}`；`pkg/executor/benchmark_test.rs::{build_stream_aggregate_executor,benchmark_aggregate_executor_reuses_open_next_close_lifecycle}`。
- Go 对照：`pkg/executor/aggregate/agg_stream_executor.go` 的 `Open/OpenSelf/Close/Next/consumeOneGroup/consumeGroupRows/consumeCurGroupRowsAndFetchChild/appendResult2Chunk`；`pkg/executor/executor_required_rows_test.go::TestStreamAggRequiredRows`；`pkg/executor/test/aggregate/aggregate_test.go::TestStreamAggPendingMemDeltaBatching`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务文件指定的命令验证目标文档存在且恰好包含 11 个固定二级标题，并人工复核文档明确回答文件定位、运行方式、真实接线限制与安全扩展点。

# `pkg/executor/aggfuncs/func_percent_rank.rs`

## 文件定位

该文件属于 `astersql-executor-aggfuncs` crate（见 `pkg/executor/aggfuncs/Cargo.toml`），实现 SQL 窗口函数 `PERCENT_RANK()` 的 Rust 状态机。模块由 `pkg/executor/aggfuncs/lib.rs` 的 `pub mod func_percent_rank` 公开；窗口构建器 `build_window_function` 在遇到 `FunctionName::PercentRank` 时生成 `AggImplementation::PercentRank`（`pkg/executor/aggfuncs/builder.rs`）。

需要区分“构建元数据”和“具体状态机接线”：当前仓库能构造 `AggImplementation::PercentRank`，但对 Rust 源码的精确检索只发现 `PercentRank<T>` 在 `func_percent_rank_test.rs` 和 `window_func_test.rs` 中被直接实例化，尚未发现生产执行器把该枚举实例化为本文件状态机的接线。因此，本文件已有可执行、受测试的算法实现，但不能仅凭 builder 枚举认定 Rust SQL 执行主链已经使用它。

## 核心职责

- 缓存一个窗口分区内已经按 `ORDER BY` 排好序的比较键行。
- 逐行识别 peer group（排序键相等的同行组），维护与 `RANK()` 相同的跳号 rank。
- 按 `(rank - 1) / (partition_rows - 1)` 输出每行的 `f64` 百分位秩。
- 为追加行报告与 Go `DefRowSize` 规则一致的估算内存增量，并允许复用状态处理下一分区。

本文件不负责排序、表达式求值、SQL 类型比较器构造、结果写入 chunk，也不实现部分结果合并或 spill。调用方必须先提供同一分区且顺序正确的行，并通过 `next_by` 注入符合 SQL `ORDER BY` 语义的比较器。

## 主要符号

- `PercentRank<T>`：公开的泛型状态机，唯一字段是私有 `state: RankState<T>`。`T` 代表用于判断 peer 的排序键行，而非最终 SQL 结果类型。
- `Default for PercentRank<T>`：创建 `RankState::default()`；初始游标、最近 rank 均为 0，行缓冲为空。
- `reset(&mut self)`：把 `cur_idx`、`last_rank` 归零并 `clear` 行缓冲，保留 `Vec` 容量供下一分区复用。
- `update(&mut self, rows)`：把一批行追加到缓冲，并返回“新增长度 × `DEF_ROW_SIZE`”。输入只要求实现 `IntoIterator<Item = T>`。
- `next_by(&mut self, compare)`：核心求值入口，以调用方比较器返回的 `Ordering::Equal` 判定 peer，返回下一行的 `Option<f64>`。
- `next(&mut self)`：仅在 `T: PartialEq` 时提供的便利入口；相等映射为 `Ordering::Equal`，不相等统一映射为 `Ordering::Less`。
- `RankState<T>`、`DEF_ROW_SIZE`：从 `func_rank.rs` 复用；前者保存 `cur_idx`、`last_rank` 和 `rows`，后者是每个缓冲行的固定估算大小。

该文件没有模块级常量、条件编译项、错误类型、异步函数或 trait 定义。

## 执行流程

1. 调用方以 `PercentRank::default()` 建立一个分区状态。
2. 调用 `update` 一次或多次，把已经按窗口排序规则排列的键行追加到 `state.rows`。函数不排序、不去重，也不检查不同批次间的顺序。
3. 每次请求一行结果时调用 `next_by`（生产式 SQL 比较应走此入口）或 `next`（简单 `PartialEq` 场景）。
4. `next_by` 先检查 `cur_idx >= rows.len()`；耗尽时返回 `None` 且不改变状态。
5. 尚有数据时将 `cur_idx` 增为 1-based 当前行号。首行把 `last_rank` 设为 1，并直接返回 `0.0`。
6. 后续行与紧邻前一行比较。若比较结果不是 `Equal`，说明进入新 peer group，`last_rank` 更新为当前 1-based 行号；若相等则沿用上一组 rank。
7. 返回 `(last_rank - 1) as f64 / (rows.len() - 1) as f64`。例如 `[1, 1, 3, 4]` 的 rank 为 `[1, 1, 3, 4]`，结果为 `[0, 0, 2/3, 1]`。
8. 分区结束后调用 `reset`，再追加下一分区数据。

`func_percent_rank_test.rs::percent_rank_uses_rank_gaps_for_peer_groups` 验证同行组与跳号；`window_func_test.rs::test_window_functions` 验证单行、全同行以及全不同行三种分区形状。

## 数据与状态

状态完全委托给 `RankState<T>`：

- `rows: Vec<T>` 保存整个分区，所以空间复杂度为 O(n)，不会边输出边释放已经消费的行。
- `cur_idx: usize` 是已输出行数，也是当前求值时的 1-based 行号。
- `last_rank: i64` 是当前 peer group 的 rank；仅首行或检测到新组时更新。

关键不变量是：`cur_idx <= rows.len()`；当 `cur_idx > 0` 时，`last_rank >= 1`；同一 peer group 内 `last_rank` 不变；新组的 rank 等于当前行号而不是上一 rank 加一。最后一点使它保持 `RANK` 的跳号语义，而不是 `DENSE_RANK`。

`update` 返回的只是固定模型估算值 `新增行数 × DEF_ROW_SIZE`，未包含 `T` 自身可能拥有的堆内存，也不报告 `Vec` 扩容容量。`reset` 清空逻辑长度但保留容量，因此也不返回负内存增量。这与当前 Go 更新路径按 `len(rowsInGroup) * DefRowSize` 报告增量的语义一致。

## 依赖与调用关系

直接 Rust 依赖只有标准库 `std::cmp::Ordering`，以及同 crate 的 `func_rank::{RankState, DEF_ROW_SIZE}`。该实现不直接使用 `Cargo.toml` 中列出的 expression、types、chunk 或 collate 等 crate；SQL 行提取与排序比较应由更上层完成。

已验证的上游关系如下：

- `lib.rs` 声明并公开 `func_percent_rank` 模块，同时将两个相关测试模块编入 `cfg(test)`。
- `builder.rs::build_window_function` 将 `FunctionName::PercentRank` 映射为 `AggImplementation::PercentRank`。
- `func_percent_rank_test.rs` 直接验证本文件的更新、求值、重置和自定义比较器行为。
- `window_func_test.rs::collect_percent_rank` 直接构造状态机，并由 `test_window_functions` 覆盖 Go 窗口函数用例。

未验证到生产调用边：全仓 Rust 精确检索没有发现除上述测试以外的 `PercentRank::default`、`PercentRank::<...>` 或 `func_percent_rank::PercentRank` 使用点，也没有发现 `AggImplementation::PercentRank` 的消费分支。因此不能把 Go 侧 `buildPercentRank -> percentRank` 的完整运行时接线投射为当前 Rust 事实。

## 错误处理与边界

本 API 不返回 `Result`，没有显式错误路径：缓冲耗尽通过 `None` 表达。空分区第一次调用 `next`/`next_by` 即返回 `None`；单行分区由首行提前返回 `0.0`，不会执行除法。后续公式使用 `rows.len().saturating_sub(1)`，但在现有控制流中分母只会在至少两行时被求值，所以不会产生由零分母导致的 `NaN`。

调用方违反前置条件不会被检测：未排序输入会按相邻元素错误划分 peer group；比较器若不稳定、不满足等价关系，结果也会失真。`next_by` 只关心是否为 `Ordering::Equal`，`Less` 与 `Greater` 都表示新组。

在已经开始输出后继续 `update` 虽然类型上允许，但会改变总行数分母：先前已返回的比例不会被重算，导致同一分区结果不一致。安全用法必须在第一次 `next*` 之前完成该分区的全部 `update`。`next` 的 `PartialEq` 也不能自动表达 SQL 的 NULL、排序规则、多列 ORDER BY 等价性；这些场景应使用 `next_by`。

## 并发与资源生命周期

`PercentRank` 的所有操作都需要 `&mut self`，文件内没有锁、原子、通道、任务或异步资源。单个实例设计为一个分区内由单一所有者顺序驱动；如果外部需要跨线程共享，必须自行提供同步，并保证更新完成后再开始求值。

生命周期为“默认构造 → 完整追加分区 → 逐行读取至 `None` → reset → 复用”。`reset` 保留 `Vec` 已分配容量，可降低相近分区间重复分配，但也意味着处理过大分区后内存可能继续由实例持有。丢弃 `PercentRank` 时由 Rust 自动释放 `Vec<T>` 及其中元素，无额外关闭或清理步骤。

`PercentRank<T>` 派生 `Clone`、`Debug`、`PartialEq`；这些能力取决于 `T` 和 `RankState<T>` 的相应 trait，实现本身没有共享可变状态。是否 `Send`/`Sync` 由 `T` 自动决定，文件没有手工声明线程安全保证。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_percent_rank.go`：

- Go `partialResult4Rank` 对应 Rust `RankState<T>`，都保存行缓冲、当前下标和最近 rank。
- Go `ResetPartialResult` 与 Rust `reset` 都归零两个计数并把行缓冲逻辑长度清零。
- Go `UpdatePartialResult` 与 Rust `update` 都追加整批行，并按行数乘固定 `DefRowSize`/`DEF_ROW_SIZE` 报告内存增量。
- Go `AppendFinalResult2Chunk` 与 Rust `next_by` 都令首行为 0、以相邻行比较结果识别同行组，并使用 `(rank-1)/(n-1)`。
- Go 的 `rowComparer.compareRows` 由 `buildPercentRank` 根据 `orderByCols` 构造；Rust 将这一策略作为 `next_by` 闭包注入。本文件本身没有 Go `baseAggFunc.ordinal` 和写 chunk 的职责，而是直接返回 `f64`。

Go 已通过 `builder.go::buildPercentRank` 返回实现 `AggFunc` 的 `percentRank`，属于完整构建/执行接线。Rust builder 当前只生成实现枚举，未在检索范围内发现把该枚举落到本文件状态机的生产分支，这是迁移状态上的主要差异。

测试对应方面，Go `func_percent_rank_test.go::TestMemPercentRank` 主要验证三种输入形状的部分结果基线和逐行内存增量；Rust `func_percent_rank_test.rs` 覆盖相同批次增量，并额外直接验证公式、reset 和自定义 peer 比较。Go `window_func_test.go::TestWindowFunctions` 的 PERCENT_RANK 三组结果由 Rust `window_func_test.rs::test_window_functions` 镜像验证。

## 扩展指南

- 若接入 Rust 生产窗口执行链，应在消费 `AggImplementation::PercentRank` 的实例化/分派位置构造 `PercentRank<排序键行>`，先收齐一个分区，再逐行调用 `next_by`，并补执行器级独立测试；不要把测试写回本源文件。
- SQL 比较语义应复用执行器已有的列比较、NULL 顺序、排序方向和 collation 设施，通过 `next_by` 注入。不要用 `next` 替代多列或字符串排序规则比较。
- 若允许流式追加与同时输出，必须重新设计分母的确定时机；当前状态机依赖预先知道完整 `partition_rows`，简单放宽调用顺序会改变已经输出的值。
- 若改内存统计，应同步核对 `func_rank.rs::DEF_ROW_SIZE`、Go `DefRowSize`、`func_percent_rank_test.rs::update_reports_go_row_memory_delta_for_each_batch_shape` 和 Go `TestMemPercentRank`。若开始计入 `T` 的堆内存或容量，需同时规定 reset 时是否返回释放量。
- 若改变 peer/rank 算法，应同步 `func_percent_rank_test.rs` 与 `window_func_test.rs`，至少覆盖空分区、单行、全同行、多个同行组、全不同行、自定义多字段比较器、耗尽后重复读取和 reset 后复用。
- 若新增公开 API，保持测试位于同目录独立 `*_test.rs` 文件；不要删除源文件现有 PingCAP Apache License 或 AsterSQL 处理标记。

兼容风险主要是 SQL 排序等价性与 Go 结果偏离；正确性风险集中在分区尚未收齐就开始求值；性能风险来自整分区 O(n) 缓冲、`T` 深拷贝以及大分区后 `reset` 保留容量。

## 验证依据

本说明基于以下直接证据：

- `pkg/executor/aggfuncs/func_percent_rank.rs`：`PercentRank<T>`、`Default`、`reset`、`update`、`next_by`、`next` 的完整实现。
- `pkg/executor/aggfuncs/func_rank.rs`：`RankState<T>`、`DEF_ROW_SIZE` 和相同 rank 跳号规则。
- `pkg/executor/aggfuncs/Cargo.toml`：crate 名、`lib.rs` 入口、端口元数据和依赖边界。
- `pkg/executor/aggfuncs/lib.rs`：生产模块公开及独立测试模块声明。
- `pkg/executor/aggfuncs/builder.rs`：`FunctionName::PercentRank`、`AggImplementation::PercentRank` 以及 `build_window_function` 映射。
- `pkg/executor/aggfuncs/func_percent_rank.go`、`builder.go::buildPercentRank`：Go 状态机、内存规则、比较器和生产构建接线。
- `pkg/executor/aggfuncs/func_percent_rank_test.rs`、`window_func_test.rs`：Rust 行为、内存、边界和重置证据。
- `pkg/executor/aggfuncs/func_percent_rank_test.go`、`window_func_test.go`：Go 对照用例与预期结果。
- RustCodeGraph：检查了索引状态，以文件节点读取目标实现、Rust/Go 测试及对照实现，并查询 `PercentRank`、`next_by`、builder 映射和调用关系。由于通用方法名调用图存在同名噪声，又用全仓精确检索核对生产实例化点；截至本次分析，仅确认 builder 枚举映射及测试直接调用，未确认生产执行器消费本文件状态机。

结构检查应保证本文存在且固定二级标题恰为以上十一项。本任务是纯文档分析，按计划不运行 Cargo 或代码测试；行为结论来自源码、调用/引用搜索和现有独立测试，而非本次动态执行。

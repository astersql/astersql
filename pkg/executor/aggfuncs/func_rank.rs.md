# `pkg/executor/aggfuncs/func_rank.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；crate 入口 `pkg/executor/aggfuncs/lib.rs` 以公开模块 `func_rank` 暴露它。它实现可复用的泛型 `RANK` / `DENSE_RANK` 排名状态机和 ORDER BY 多列同行组（peer group）比较器，不直接解析 SQL、读取表达式或向结果 `Chunk` 写列。

SQL 函数名到实现描述的入口在 `pkg/executor/aggfuncs/builder.rs::build_window_function`：`FunctionName::Rank` 和 `FunctionName::DenseRank` 分别生成 `AggImplementation::Rank { dense: false }` 与 `{ dense: true }`。当前非测试 Rust 源码搜索只找到这一构建描述，没有找到将该变体实例化为本文件 `Rank<T>` 的生产调用；因此本文件的算法已有独立测试覆盖，但完整 Rust 执行器接线仍是“未验证”，不能用 Go 的完整执行链替代这一事实。

## 核心职责

- `Rank<T>` 缓存一个窗口分区的已排序行，并逐行产出排名。
- `is_dense == false` 实现标准 `RANK`：新同行组的秩取当前 1-based 行号，因此并列后会跳号。
- `is_dense == true` 实现 `DENSE_RANK`：每遇到新同行组只将上次秩加一，因此不跳号。
- `RowComparer<T>` 按 ORDER BY 列优先级组合多个比较函数，以首个非 `Equal` 结果确定两行是否属于同一同行组。
- `DEF_PARTIAL_RESULT_RANK_SIZE` 和重新导出的 `DEF_ROW_SIZE` 提供固定状态、追加行的内存估算口径，与 Go 同名常量/规则对齐。

该实现假定调用方已按窗口 ORDER BY 顺序提供同一分区的行；它只比较相邻行来识别组边界，不负责排序或分区。

## 主要符号

- `pub const DEF_PARTIAL_RESULT_RANK_SIZE: i64`：以 `size_of::<RankState<()>>()` 表示空排名状态的固定体积。泛型取 `()` 意味着常量描述 `cur_idx`、`last_rank` 与 `Vec` 句柄，不包含行缓冲的堆内存。
- `pub use crate::aggfuncs::DEF_ROW_SIZE`：复用 crate 公共的 `Row` 固定大小估算，而不是为排名逻辑另设口径。
- `pub struct RowComparer<T>`：持有 `Vec<fn(&T, &T) -> Ordering>`。`new` 保留比较器顺序；`compare` 返回首个非相等结果，比较器为空或全部相等时返回 `Ordering::Equal`。
- `pub struct RankState<T>`：公共状态载体，字段为下一次输出位置 `cur_idx: usize`、最近的秩 `last_rank: i64` 和完整分区缓冲 `rows: Vec<T>`；其 `Default` 是零游标、零秩、空缓冲。
- `pub struct Rank<T>`：用私有 `is_dense` 固化排名模式，并私有持有 `RankState<T>`。`new(bool)` 是构造入口；`state()` 仅提供只读状态引用。
- `Rank::update`：接受任意 `IntoIterator<Item = T>`，把所有项目追加到 `rows`，返回“新增项目数 × `DEF_ROW_SIZE`”。
- `Rank::next_by`：核心推进器，由调用方提供同行组比较闭包；耗尽时返回 `None`。
- `Rank::next`：只对 `T: PartialEq` 提供的便捷路径；相等映射为 `Equal`，不等统一映射为 `Less`，因为算法只关心是否相等。
- `Rank::reset`：归零游标和秩并 `Vec::clear`，复用已分配容量开始下一个分区。

文件没有 trait、宏、条件编译项或错误类型；公开 API 是上述常量、结构及方法，内部实现状态仅 `Rank::is_dense` 与 `Rank::state` 字段为私有。

## 执行流程

1. 构建阶段由 `builder.rs::build_window_function` 根据 SQL 函数名记录 `dense` 模式；直接使用状态机时则调用 `Rank::new(false/true)`。
2. 调用方把同一窗口分区中已经按 ORDER BY 排好序、可用于判断同行组的行传给 `update`。可以分批追加，但在开始输出后继续追加的语义没有测试保证，安全用法是先完成分区收集。
3. 每次 `next_by` 先检查 `cur_idx >= rows.len()`；若耗尽则不修改状态并返回 `None`。
4. 未耗尽时先把 `cur_idx` 加一。第一行把 `last_rank` 设为 1；后续行比较 `rows[cur_idx - 2]` 与 `rows[cur_idx - 1]`。
5. 相邻两行比较为 `Equal` 时沿用 `last_rank`。出现新组时，稠密模式执行 `last_rank += 1`，标准模式执行 `last_rank = cur_idx as i64`。
6. 返回当前 `last_rank`。例如 `[1, 1, 3, 4]` 在标准模式得到 `[1, 1, 3, 4]`，稠密模式得到 `[1, 1, 2, 3]`；该分叉由 `func_rank_test.rs::rank_and_dense_rank_diverge_only_after_peer_gaps` 验证。
7. 分区结束后调用 `reset`，再为新分区 `update` 并逐行读取；`func_rank_test.rs::rank_reset_reuses_the_state_for_a_new_partition` 覆盖了这一生命周期。

## 数据与状态

`rows` 拥有所有 `T`，所以分区数据在 `Rank` 中保持到 `reset` 或对象销毁。`cur_idx` 是已输出行数，也是当前行的 1-based 序号；`last_rank` 是最近一个同行组的排名。核心不变量是 `cur_idx <= rows.len()`，且在至少产出一行后 `last_rank >= 1`。算法要求同行组在输入中连续；若相等排序键被不相等行隔开，它们会被视为不同组。

`update` 的内存返回值仅按新增行数乘公共 `DEF_ROW_SIZE`，不观察 `Vec` 容量增长，也不包含 `T` 可能拥有的字符串、JSON 等堆内存。`reset` 使用 `clear`，逻辑行数归零但通常保留容量；因此返回的估算增量是与 Go 约定对齐的记账量，不是 Rust 分配器的精确差值。`DEF_PARTIAL_RESULT_RANK_SIZE` 同理是固定状态基线。

## 依赖与调用关系

直接标准库依赖只有 `std::cmp::Ordering` 和 `std::mem::size_of`；直接 crate 内依赖只有 `crate::aggfuncs::DEF_ROW_SIZE`。`Cargo.toml` 将本文件纳入 `astersql-executor-aggfuncs`，本算法本身不直接使用该 crate 声明的 expression、types、chunk 等依赖。

已核实的上游关系如下：

- `lib.rs` 声明 `pub mod func_rank`，并在测试配置下声明独立的 `mod func_rank_test`。
- `builder.rs::build_window_function` 把 `Rank` / `DenseRank` 解析为含 `dense` 标志的 `AggImplementation::Rank`；这是 SQL 构建侧元数据入口，但当前生产 Rust 搜索未确认它继续调用本文件状态机。
- `func_percent_rank.rs` 复用 `RankState<T>` 和 `DEF_ROW_SIZE`，从而共享行缓冲、游标、秩及内存口径；它自行实现百分位公式。
- `func_cume_dist.rs` 复用 `DEF_ROW_SIZE`，但维护独立的同行组末尾游标。
- `func_rank_test.rs` 直接覆盖排名差异、公共内存口径、多列比较器、空/耗尽以及重置；`window_func_test.rs::collect_rank` / `test_window_functions` 提供与 Go 窗口场景相呼应的跨窗口函数测试。

RustCodeGraph 对文件给出 17 个符号并定位上述定义；对常见名称执行独立 `callers`/`callees` 时没有产出可用结果，因此调用关系以 RustCodeGraph `explore` 的命中、精确 `query` 以及相邻源码搜索交叉确认。

## 错误处理与边界

API 不返回 `Result`，也不产生领域错误。空状态或全部输出完毕时，`next` / `next_by` 返回 `None`；第一行恒返回 `Some(1)`。空 `RowComparer` 将任意两行视为同行，这对应没有有效 ORDER BY 比较列时所有行共享一个排名的行为。

比较函数是普通函数指针，不能直接捕获环境，也没有“比较失败”通道；排序规则、NULL、collation 和升降序的具体语义都必须由调用方注入的函数负责。若比较器不满足相等关系的一致性，或者输入未排序，算法仍会机械地产出值但 SQL 排名语义不可靠。

`cur_idx as i64` 在极端超过 `i64::MAX` 的分区上会产生截断语义，代码没有显式保护；现实限制通常先受内存与 `usize` 约束。`update` 的行数到 `i64` 及乘法同样没有溢出检查。当前测试未覆盖这些不可实际构造的大规模边界。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部句柄。所有可变推进操作都要求 `&mut self`，因此单个 `Rank` 实例在安全 Rust 中不会被两个调用者无同步地同时推进；跨线程能力取决于具体 `T` 和比较函数的 `Send` / `Sync` 自动 trait。

资源生命周期是“构造状态机 → 收集一个分区 → 按行输出 → reset 复用或 drop 释放”。`reset` 会丢弃所有行值并保留 `Vec` 容量，`drop` 才释放缓冲容量。`state()` 返回的共享借用受 Rust 生命周期约束，不能在同一借用存续期间调用需要可变借用的推进/重置方法。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/aggfuncs/func_rank.go`：Rust `RankState<T>` 对应 Go `partialResult4Rank`，`Rank<T>` 对应 Go `rank` 中的 `isDense` 加部分结果状态，`RowComparer<T>` 对应 Go `rowComparer`。两端首行秩为 1、同行沿用旧秩、新组按 dense 模式 `+1` 或取当前行号，算法一致。

生命周期映射为：Rust `Rank::new`/默认状态约等于 Go `AllocPartialResult`，`reset` 对应 `ResetPartialResult`，`update` 对应 `UpdatePartialResult`，`next_by` 对应 `AppendFinalResult2Chunk` 的排名推进部分。Go 最终把 `int64` 直接追加到 `chunk.Chunk`，Rust 则返回 `Option<i64>`，将输出容器留给调用方；Go 方法还返回 `error`，但当前实现始终返回 `nil`，Rust 因而没有等价错误通道。

比较器层面，Go `buildRowComparer` 从 `expression.Column` 的类型取得 `chunk.CompareFunc` 并保存列索引；Rust `RowComparer` 只保存对整个 `T` 的函数指针，不负责从表达式/字段类型构建比较器。这说明 Rust 核心算法与 Go 对齐，但生产层的类型感知比较器构建尚未在本文件中移植。

内存方面，Go `DefPartialResult4RankSize` 用 `unsafe.Sizeof(partialResult4Rank{})`，Rust 用 `size_of::<RankState<()>>()`；追加行都按新增行数乘公共行大小。Go `TestMemRank` 验证分配和更新记账；Rust `rank_uses_the_shared_row_memory_size` 验证共享行大小与更新增量，但没有直接断言固定状态常量与 Go 在具体平台数值完全相等。

## 扩展指南

- 新增排序语义时，优先在调用层构造正确的 `RowComparer` 比较函数，不要把 SQL 类型、NULL 或 collation 规则硬编码进泛型 `Rank<T>`。同步扩展 `func_rank_test.rs::row_comparer_uses_the_first_non_equal_ordering_column`，覆盖多列优先级和相等路径。
- 修改排名推进时，应同时验证标准/稠密模式、连续同行组、空分区、耗尽后重复读取和 `reset` 后复用；测试应继续放在独立文件 `pkg/executor/aggfuncs/func_rank_test.rs`，不要内嵌到生产源文件。
- 若补全生产接线，应从 `builder.rs::AggImplementation::Rank { dense }` 的消费者接入本状态机，并证明 ORDER BY 行提取、比较器构建、结果列写入和每分区 reset；不要仅凭构建器已有变体宣称运行时已接通。
- 若调整状态字段或记账，必须复核 `DEF_PARTIAL_RESULT_RANK_SIZE`、公共 `DEF_ROW_SIZE`、`func_percent_rank.rs` 对 `RankState` 的复用，以及 Go `DefPartialResult4RankSize` / `TestMemRank` 的兼容意图。
- 性能敏感点是缓存整个分区和逐行保留 `T`。改变为流式算法前需确认执行器是否要求预收集、spill 协议以及 `PERCENT_RANK` 等复用者；当前文件本身没有 spill 序列化接线。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter pkg/executor/aggfuncs` 确认目标、Go 对照与独立测试；`node --file pkg/executor/aggfuncs/func_rank.rs --offset 1 --limit 260` 读取完整 142 行；`query RankState/Rank/DenseRank/RowComparer` 定位核心符号；`explore "pkg/executor/aggfuncs/func_rank.rs Rank DenseRank PercentRank CumeDist"` 给出目标符号及测试/相邻复用命中。通用名称的单独 `callers`/`callees` 查询未返回可用输出，未据此虚构调用边。
- 源码：`pkg/executor/aggfuncs/func_rank.rs`（全部实现）、`lib.rs`（模块与独立测试装配）、`aggfuncs.rs::DEF_ROW_SIZE`、`builder.rs::{AggImplementation, build_window_function}`、`func_percent_rank.rs`、`func_cume_dist.rs`。
- crate 边界：`pkg/executor/aggfuncs/Cargo.toml` 的包名、`lib.rs` 入口和 package porting 元数据。
- Go 对照：`pkg/executor/aggfuncs/func_rank.go`、`func_rank_test.go`、`window_func_test.go::TestWindowFunctions`。
- Rust 测试：`pkg/executor/aggfuncs/func_rank_test.rs`、`window_func_test.rs::{collect_rank, test_window_functions}`。本任务按计划为纯文档分析，未运行 Cargo 或代码测试。
- 人工复核：文档区分了核心状态机、构建描述和未验证的生产实例化接线；没有把预期架构写成当前事实，也没有建议把测试放入生产源文件。

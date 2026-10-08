# `pkg/util/chunk/iterator.rs`

## 文件定位

本文件属于 `astersql-util-chunk` crate。crate 入口 `pkg/util/chunk/lib.rs` 通过 `#[path = "iterator.rs"] pub mod iterator` 挂载它；`pkg/util/chunk/Cargo.toml` 指定该 crate 的入口为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/util/chunk"` 记录 Go 来源。文件本身不直接引入第三方 crate，而是在 crate 内组合 `Chunk`、`Row`、`List`、`RowPtr`、`RowContainer` 与统一的 `ChunkError`。

它位于列式数据容器和上层 SQL 逻辑之间：把 `Chunk` 或由多个 `Chunk` 组成的容器统一成 Go 风格的行游标。当前可核实的生产 Rust 调用包括 `pkg/planner/cardinality/selectivity.rs` 中对 `chunk::NewIterator4Chunk` 的三处使用；`pkg/planner/cardinality/lib.rs` 对该构造器进行再导出。部分 executor 文件只保留了对应 Go 调用的迁移注释，因此不能据此认定那些路径已经接线。

## 核心职责

- 定义对象安全的 `Iterator` trait，以 `Begin`、`Next`、`Current`、`End`、`ReachEnd`、`Len`、`Error` 统一六类行源。
- 为 `Vec<Row>`、单个 `Chunk`、`List`、`List + Vec<RowPtr>`、`RowContainer` 分别维护游标，并把它们都映射为相同的 `Row` 返回协议。
- 由 `multiIterator` 顺序拼接多个非空子迭代器，在子迭代器之间切换，并向外传播首个已观察到的读取错误。
- 保留 Go 版的哨兵约定：`Row::default()` 是空行，也是 `End()`；正常遍历形态是 `for row = Begin(); row != End(); row = Next()`。
- 保持逻辑行语义。`Iterator4Chunk` 通过 `Chunk::NumRows` 和 `Chunk::GetRow` 遍历，因此 `Chunk::SetSel` 安装的 selection vector 会决定可见行数和逻辑下标映射，而不是被迭代器绕过。

## 主要符号

- `pub trait Iterator`：公共行游标协议。方法名称刻意保持 Go 命名；所有取行操作需要 `&mut self`，`Error` 返回可克隆的 `Option<ChunkError>`。
- `CacheLinePad`：`#[repr(align(64))]` 的零大小对齐类型，只用于 `Iterator4Slice` 两侧，复刻 Go `cpu.CacheLinePad` 对游标字段的隔离意图。
- `NewIterator4Slice(Vec<Row>) -> Box<Iterator4Slice>` / `Iterator4Slice`：拥有行句柄数组，`Reset` 可整体换批并把游标恢复为未开始状态。
- `NewIterator4Chunk(Box<Chunk>) -> Box<Iterator4Chunk>` / `Iterator4Chunk`：拥有一个稳定地址的 `Chunk`；公开 `GetChunk`、`GetChunkMut`、`Reset`、`ResetChunk` 供复用。`numRows` 在 `Begin` 时快照，供本轮 `Next` 判定终点。
- `NewIterator4List(Box<List>) -> Box<dyn Iterator>` / 私有 `iterator4List`：用 `(chkCursor, rowCursor)` 跨块顺序遍历。`List::Add` 明确拒绝空 Chunk，`AppendRow` 也只建立含行的活动块，因此实现可依赖活动块非空这一不变量。
- `NewIterator4RowPtr(Box<List>, Vec<RowPtr>) -> Box<dyn Iterator>` / 私有 `iterator4RowPtr`：按指针表给出的顺序取行，可表达子集、重复或乱序；`Len` 是指针数，不是 `List::Len`。
- `NewIterator4RowContainer(Box<RowContainer>) -> Box<dyn Iterator>` / 私有 `iterator4RowContainer`：通过 `RowContainer::NumChunks`、`NumRowsOfChunk`、`GetRow` 遍历内存或落盘数据，保存可能发生的 `ChunkError`。
- `NewMultiIterator(Vec<Box<dyn Iterator>>) -> Box<dyn Iterator>` / 私有 `multiIterator`：构造时过滤 `Len() == 0` 的输入并缓存总长度、有效子迭代器数和当前位置。

## 执行流程

1. 构造器接管底层容器所有权，并把游标置于“尚未开始”。调用方必须先调用 `Begin`；此时返回首行，同时内部位置被推进到“下一行”。
2. `Iterator4Slice`、`Iterator4Chunk` 和 `iterator4RowPtr` 使用单一 `cursor`。`Current` 读取 `cursor - 1`；`cursor == 0` 或大于长度时返回结束哨兵；`Next` 在取行后递增，耗尽时将游标放到 `len + 1`。
3. `iterator4List::Begin` 读取首块首行，并依据首块是否只有一行，预先把下一位置设为同块第 1 行或下一块第 0 行。`Next` 每次读取 `(chkCursor, rowCursor)`，块内耗尽后将行下标清零并增加块下标。`Current` 在 `rowCursor == 0` 时回看上一块末行，否则读取当前块的前一行。
4. `iterator4RowContainer::Begin` 把 `(chkIdx, rowIdx)` 设为 `(0, -1)`、清除旧错误，再委托 `Next`。`setNextPtr` 先增加行下标，命中当前块行数后跨块；`Current` 用该位置调用 `RowContainer::GetRow`。
5. `multiIterator::Begin` 回到第一个有效子迭代器，启动它后通过其 `Current` 返回首行。`Next` 先推进当前子迭代器；若得到其 `End`，先检查 `Error`，无错误才切到下一个子迭代器并调用其 `Begin`。全部耗尽或遇错后返回自身 `End`。
6. 任一实现的 `ReachEnd` 都只移动组合层的游标，不清空底层数据。除 `RowContainer` 的 `Begin` 和 `multiIterator::Begin` 会清理已保存错误外，重新开始不会重建数据源。

## 数据与状态

`Row` 是指向 `Chunk` 的轻量行视图；`Row::default()` 的 Chunk 指针为空，`PartialEq` 比较指针和行下标，所以它可安全承担本文件的结束哨兵。各迭代器拥有 `Box<Chunk>`、`Box<List>` 或 `Box<RowContainer>`，使其返回的非自持有 `Row` 在迭代器存活且底层布局未被破坏时保持来源对象存活。

单游标实现约定三个状态：`0` 表示尚未开始，`1..=Len` 表示最近返回了下标 `cursor - 1` 的行，`Len + 1` 表示结束。`iterator4List` 用二元游标表达同一状态；刚跨块时 `rowCursor == 0`，最近返回行位于前一块末尾。`iterator4RowContainer` 额外用 `rowIdx == -1` 表示开始前，并在错误时保存 `err`。

`Iterator4Chunk` 的 `numRows` 与 `Len()` 含义有细微区别：前者由 `Begin` 快照并控制本轮 `Next`，后者每次读取当前 `Chunk::NumRows()`。因此遍历期间经 `GetChunkMut` 修改行数或 selection vector 会让 `Next` 的终点与 `Current`/`Len` 的动态视图不一致；安全用法是在修改后调用 `Reset` 再 `Begin`。`ResetChunk` 本身不重置游标或快照，这一点也要求调用方显式开始新一轮。

## 依赖与调用关系

向下依赖均在同一 crate：`Iterator4Chunk` 调用 `Chunk::{NumRows, GetRow}`；`iterator4List` 调用 `List::{NumChunks, GetChunk, Len}`；`iterator4RowPtr` 调用 `List::GetRow`；`iterator4RowContainer` 调用 `RowContainer::{NumRow, NumChunks, NumRowsOfChunk, GetRow}`；所有实现使用 `Row::default` 作为终点。`RowContainer::GetRow` 在内存路径转交 `List::GetRow`，在落盘路径转交磁盘行存储，并可能返回 `ChunkError`。

RustCodeGraph 的文件节点显示 `pkg/util/chunk/iterator.rs` 被 19 个索引文件使用；精确节点证据确认 `NewIterator4Chunk` 被 `pkg/util/chunk/iterator_test.rs` 的 Chunk、selection-vector 和空/多迭代器测试调用，`NewMultiIterator` 与 `NewIterator4RowContainer` 也被同一独立测试调用。源码搜索进一步确认非测试生产调用集中在 `pkg/planner/cardinality/selectivity.rs` 的三个 `NewIterator4Chunk` 调用点。`pkg/expression/lib.rs` 也从 chunk 依赖再导出 `Iterator`、`Iterator4Chunk` 和 `NewIterator4Chunk`，但 `pkg/expression/builtin.rs` 同时存在另一个同名的借用式实现，分析调用时必须按模块路径消歧。

## 错误处理与边界

内存迭代器的 `Error` 恒为 `None`；索引越界不是 `Result` 错误，而由 `End` 哨兵表达。空切片、空 Chunk、无 Chunk 的 List、空 RowPtr 表和空 RowContainer 都应在 `Begin` 返回 `End`，相关 Rust 与 Go 测试均覆盖这些条件。

只有 `iterator4RowContainer::Current` 会直接接收可失败结果。`RowContainer::GetRow` 失败时，它保存错误、调用 `ReachEnd` 并返回空行；调用方必须在遍历因 `End` 停止后检查 `Error`，否则读取失败与正常结束在行值上不可区分。`Begin` 会清除该迭代器上一次的错误。`multiIterator` 在子迭代器返回 `End` 时读取子错误；若存在错误，保存它并终止整个组合，不继续后续子迭代器。

边界前提包括：`RowPtr` 必须指向有效块和行，否则 `List::GetRow` 的直接索引会 panic；`List` 的活动 Chunk 必须非空；调用空 `Row` 的取列方法会 panic；本协议因此只允许把空 `Row` 当哨兵，不允许当数据行。当前实现只在一次 `Next` 中跨越一个子迭代器，但构造时已过滤所有 `Len == 0` 的子项，所以不会卡在连续空输入上。

## 并发与资源生命周期

这些迭代器是有可变游标的状态对象，接口以 `&mut self` 串行推进，没有内部同步，也没有承诺一个实例可被多个任务并发消费。`CacheLinePad` 只是布局/伪共享优化，不把 `Iterator4Slice` 变成并发迭代器。

所有构造器都接管数据源，销毁迭代器会同时释放其 `Vec<Row>`、`Chunk`、`List` 或 `RowContainer`。`multiIterator` 再接管所有保留的子迭代器。`RowContainer` 内部用读写锁保护内存/落盘记录，单次 `Num*` 或 `GetRow` 调用各自加锁，但本文件先查询位置再读取的组合步骤不是跨调用原子的；并发改变容器结构不属于这里建立的一致快照。磁盘行读取可能产生拥有自身 backing data 的 `Row`，具体生命周期由 `RowContainer::GetRow` 下游保证。

`Iterator4Chunk::GetChunkMut` 暴露底层可变引用，`Iterator4Slice::Reset` 与 `Iterator4Chunk::{Reset, ResetChunk}` 支持对象复用；复用前必须确保先前取出的借用式行视图不再被使用，并在换源或改变可见行集合后重新 `Begin`。

## 与 Go 版本的对应关系

Rust 文件逐类对应 `pkg/util/chunk/iterator.go`：trait 对应 Go interface，五个具体数据源迭代器和 `multiIterator` 的游标转移、空行哨兵、长度计算、`Current`、`ReachEnd` 与错误传播顺序保持一致。`pkg/util/chunk/iterator_test.go` 的 `TestIteratorOnSel`、`TestMultiIterator`、`TestIterator` 分别在 Rust 的 `iterator_test.rs` 和 `iterator_3_aster_unit_test.rs` 中得到对应覆盖。

语言层差异主要来自所有权与类型：Go 使用指针/切片和可空 `error`，Rust 构造器接收 `Box`/`Vec` 并返回具体 `Box` 或 `Box<dyn Iterator>`，错误为 `Option<ChunkError>`；Go 的 `int`/`int32` 游标在 Rust 中是 `usize`，RowContainer 为表示 `-1` 起始位单独使用 `isize`。Go `NewMultiIterator` 是可变参数，Rust 接收 `Vec<Box<dyn Iterator>>`；Go 保存 `curIter` 接口值，Rust 以 `curPtr` 直接索引 `iters`。Rust 的 `Begin` 还显式清理 RowContainer 和 multiIterator 的旧错误，避免复用时泄漏前一轮状态。

Go 的 `Iterator4Chunk::ResetChunk` 同样只替换 Chunk；Rust 额外提供 `Reset` 和 `GetChunkMut`。Rust 通过 `Box` 保持 Chunk 地址稳定，因为 `Row` 内含原始 Chunk 指针；这是内存模型上的必要适配，而非业务语义变化。

## 扩展指南

新增行源时，应在独立生产文件或本文件中实现完整 `Iterator` 协议，并重点保持三项约束：`Begin` 可重复启动且返回首行、`Current` 始终对应最近一次返回的行、`End` 与正常数据行可明确区分。若读取可能失败，必须先保存错误再进入结束状态，并保证 `Begin` 是否清错有明确且与组合迭代器兼容的语义。

修改 Chunk 迭代时优先检查 `Iterator4Chunk::{Begin, Next, Current, Reset, ResetChunk}` 之间的快照约定及 selection-vector 行映射；修改跨块逻辑时检查 `iterator4List::{Begin, Next, Current}` 和 `iterator4RowContainer::{setNextPtr, Next, Current}` 的块边界；修改组合行为时检查 `NewMultiIterator` 的空项过滤、缓存长度以及 `multiIterator::{Begin, Next, Current, Error}` 的错误短路。

测试必须继续放在独立文件，不能嵌入 `iterator.rs`。至少同步 `pkg/util/chunk/iterator_test.rs`；若保持 Go 对齐，还要核对 `pkg/util/chunk/iterator_test.go`。涉及通用迭代器回归时也应同步检查 `pkg/util/chunk/iterator_3_aster_unit_test.rs`。新增测试应覆盖空输入、单行/跨块末端、重复 `Begin`、`Current` 的开始前与结束后状态、`ReachEnd`、selection vector、无效 RowPtr 的预期策略，以及可失败子迭代器在 `multiIterator` 中的错误短路。兼容性风险集中在哨兵等价、游标 off-by-one 和 Go/Rust 重启语义；性能风险集中在额外 Row 克隆、重复 `Len`/锁调用和破坏 Chunk 稳定地址。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 7,032 个 Rust 文件，`files --filter pkg/util/chunk` 找到目标及相邻实现；`node --file pkg/util/chunk/iterator.rs --offset 1 --limit 500` 完整读取 492 行和 70 个符号；`query Iterator`、`query Iterator4Chunk` 用于消歧；`node NewIterator4Chunk`、`node NewMultiIterator`、`node NewIterator4RowContainer` 提供定义、下游 `Len` 调用和测试调用边。批量 `callers/callees` 查询曾在 30 秒窗口内未返回，因此生产调用以精确源码搜索补齐，没有把超时结果当作“无调用者”。
- 源码与 crate 边界：`pkg/util/chunk/iterator.rs`、`pkg/util/chunk/lib.rs`、`pkg/util/chunk/Cargo.toml`。
- 直接数据依赖：`pkg/util/chunk/chunk.rs` 的 `NumRows`/`GetRow`/`SetSel`，`pkg/util/chunk/row.rs` 的 `Row::default`/`IsEmpty` 与生命周期说明，`pkg/util/chunk/list.rs` 的块非空约束和 `GetRow`，`pkg/util/chunk/row_container.rs` 的 `Num*`/`GetRow` 内存与磁盘分支。
- Go 对照与测试：`pkg/util/chunk/iterator.go`、`pkg/util/chunk/iterator_test.go`、`pkg/util/chunk/iterator_test.rs`、`pkg/util/chunk/iterator_3_aster_unit_test.rs`。
- 已人工核对：文档区分当前生产接线与迁移注释，说明每个迭代器为何存在、游标如何运行、错误和资源如何结束，以及扩展时应修改的符号和独立测试位置。任务为纯文档分析，按计划不运行 Cargo。

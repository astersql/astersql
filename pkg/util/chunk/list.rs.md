# `pkg/util/chunk/list.rs`

## 文件定位

本文件实现 `astersql-util-chunk` crate 的多块行列表。crate 入口 [`pkg/util/chunk/lib.rs`](lib.rs) 以 `pub mod list` 挂载该模块，并在 crate 根重导出 `List`、`RowPtr`；[`pkg/util/chunk/Cargo.toml`](Cargo.toml) 指定 crate 名为 `astersql-util-chunk`、库入口为 `lib.rs`，且用 `package.metadata.porting.go-package = "pkg/util/chunk"` 标明 Go 对照包。

它位于单个列式 [`Chunk`](chunk.rs) 与更高层容器之间：`List` 负责在多个 `Chunk` 间追加、定位、遍历和复用内存；[`row_container.rs`](row_container.rs) 将它作为未落盘时的存储层，并在 spill 后切换到 `DataInDiskByRows`；[`iterator.rs`](iterator.rs) 则基于 `List` 或 `RowPtr` 提供顺序/指定行迭代。

## 核心职责

- `List` 保存同一组 `fieldTypes` 对应的多个 `Box<Chunk>`，以 `initChunkSize`/`maxChunkSize` 控制新块的初始容量和增长上限，并用 `length` 缓存总行数。
- `AppendRow` 把输入 `Row` 的值追加到当前可写块；没有块、末块已满或末块已被记账并视为只读时，通过 `AllocChunk` 新开一块，并返回 `(ChkIdx, RowIdx)` 形式的 `RowPtr`。
- `Add` 接管一个非空 `Chunk`。随后追加行不会修改这个已加入的块，因为 `consumedIdx` 会指向它，下一次 `AppendRow` 必须另开块。
- `Reset` 把活跃块移入 `freelist` 供下一轮复用；`Clear` 才释放活跃块和空闲块并把 Tracker 用量归零。
- 内存 Tracker 采用延迟结算：正在追加的最后一块尚可能增长，通常在切换到下一块、`Add` 或 `Reset` 时才记录其 `MemoryUsage`；从 freelist 取块时先扣除旧用量。

## 主要符号

- `pub struct List`：核心容器。`fieldTypes`、容量配置、`length`、活跃 `chunks`、`freelist`、`memTracker` 和 `consumedIdx` 均为内部状态。`Vec<Box<Chunk>>` 使外层 `Vec` 扩容时 `Chunk` 地址保持稳定，符合 `Row` 持有底层 Chunk 指针的实现约束。
- `pub struct RowPtr { ChkIdx: u32, RowIdx: u32 }`：列表内行位置，不包含所有权、代际或边界信息；只应交还给生成它且尚未重置/清空的同一个列表。
- `pub const RowPtrSize`：`RowPtr` 的本机字节大小，供上层内存估算使用。
- `NewListWithMemTracker(...) -> Box<List>`：使用调用方提供的 `memory::Tracker` 构造空列表。
- `NewList(...) -> Box<List>` 与 `List::New(...)`：使用 `LabelForChunkList`、无限额 Tracker 的便捷构造入口；`List::New` 只是前者的关联函数门面。
- 查询方法 `GetMemTracker`、`GetMemTrackerMut`、`Len`、`NumChunks`、`FieldTypes`、`NumRowsOfChunk`、`GetChunk`：暴露 Tracker、计数、schema 和块视图；索引方法不返回 `Result`。
- 修改方法 `AppendRow`、`Add`、`AllocChunk`、`Reset`、`Clear`：分别承担逐行追加、整块接管、块分配/复用、逻辑重置和彻底清理。
- `GetRow(RowPtr) -> Row`：按两级下标取得行视图。
- `type ListWalkFunc = Box<dyn FnMut(Row) -> Result<(), ChunkError>>` 与 `Walk`：按块序、行序调用可变闭包，并允许闭包以 `ChunkError` 提前停止。

## 执行流程

1. 构造时保存字段类型与容量参数，`length = 0`、两组块为空、`consumedIdx = -1`；默认构造器同时创建无上限的 Chunk List Tracker。
2. `AppendRow` 先检查末块。若列表为空、末块达到自身 `Capacity()`，或末块下标等于 `consumedIdx`，则调用 `AllocChunk`。若被离开的末块尚未记账，先把它的当前 `MemoryUsage` 计入 Tracker，再把 `consumedIdx` 推进到该块。
3. `AllocChunk` 优先弹出 freelist 末项：先从 Tracker 扣除该块旧用量，再 `Chunk::Reset`；无空闲块但有活跃块时调用 `Renew(last, maxChunkSize)`，以末块布局续建；首次分配则调用 `New(fieldTypes, initChunkSize, maxChunkSize)`。
4. 新行追加到选定块，追加前的 `NumRows()` 即 `RowIdx`；成功后增加 `length` 并返回包含块下标和行下标的 `RowPtr`。
5. `Add` 先拒绝空块，再结算尚未记账的旧末块和输入块的内存，更新 `consumedIdx` 与 `length`，最后接管输入块。因新末块已经被标记为 consumed，后续逐行追加会创建另一块。
6. `Reset` 结算当前未记账的末块，把全部活跃块批量移动到 freelist，并复位行数和 consumed 标记；下一轮追加按 LIFO 顺序复用这些块。`Clear` 反向冲销 Tracker 的全部当前用量，清空两组块和计数。
7. `Walk` 对 `chunks` 逐块、对每块的 `[0, NumRows())` 逐行执行闭包；首个 `Err` 立即原样返回，否则遍历完返回 `Ok(())`。

## 数据与状态

`length` 是跨块总行数的缓存，正常修改路径维持 `length == chunks.iter().map(Chunk::NumRows).sum()`。`freelist` 中的块不计入 `NumChunks`/`Len`，但在 `Reset` 后仍占有分配并继续体现在 Tracker 中，直到被 `AllocChunk` 取出（先扣旧值）、被重新记账，或由 `Clear` 释放。

`consumedIdx` 是内存记账边界，也是“当前末块是否冻结”的标志：`-1` 表示没有活跃块被结算；值为末块下标时，`AppendRow` 不再写该块。该设计避免一个已按旧 `MemoryUsage` 记账的块继续增长而使 Tracker 低估，同时保留对正在增长的最后一块延迟记账的 Go 语义。

`RowPtr` 的两个字段是 `u32`，而 Rust 容器下标是 `usize`；构造时从 `usize` 截为 `u32`，读取时再转回。代码未检查超过 `u32::MAX` 的块数/行数。`Box<Chunk>` 保证活跃期间块地址不因外层向量重分配而改变，但 `Reset` 后复用会重置块内容，`Clear` 会释放块，因此旧 `Row`/`RowPtr` 不能跨这些生命周期操作继续使用。

## 依赖与调用关系

下游依赖均来自 crate 根：`Chunk` 提供 `NumRows`、`Capacity`、`AppendRow`、`MemoryUsage`、`Reset`、`GetRow`；`New` 和 `Renew` 创建布局兼容的新块；`types::FieldType` 描述列；`memory::Tracker` 提供 `Consume` 与 `BytesConsumed`；`ChunkError` 是遍历回调的错误类型。

已核对的直接上游包括：

- [`row_container.rs`](row_container.rs) 的 `RowContainer::New` 创建 `List` 并把其 Tracker 挂到容器 Tracker；`Add`、`AllocChunk`、`GetChunk`、`GetRow`、`Reset`、`Close` 和 spill 流程分别调用本文件对应能力。并发保护位于该上游的 `RwLock<rowContainerRecord>`，不在 `List` 内部。
- [`iterator.rs`](iterator.rs) 的 `NewIterator4List` 使用 `Len`、`NumChunks`、`GetChunk` 做跨块顺序遍历；`NewIterator4RowPtr` 持有 `List` 和指针表并反复调用 `GetRow`。
- [`pkg/executor/internal/applycache/apply_cache.rs`](../../executor/internal/applycache/apply_cache.rs) 以 `Arc<chunk::List>` 作为 apply cache 的值，并通过 `GetMemTracker().BytesConsumed()` 把列表已记账用量纳入缓存内存估算。它共享的是只读列表；本文件本身没有内部同步。

RustCodeGraph 状态显示本文件已被 32 个文件使用，但索引没有为 `impl List` 方法生成可消歧的方法节点；因此上述具体调用边由目标目录和生产 Rust 源码的直接引用搜索复核，而非把同名的全仓方法误认作本实现调用方。

## 错误处理与边界

本类型的大部分 API 是不返回错误的内存操作。`Add` 对空 Chunk 使用 `assert!`，会 panic；它不运行时校验输入块的字段类型与 `fieldTypes` 是否相同，也不限制输入块行数是否超过 `maxChunkSize`，调用方必须遵守与 Go 注释一致的契约：块非空、不再由调用方使用且 schema 相同。

`GetChunk`、`NumRowsOfChunk` 和 `GetRow` 使用直接索引；无效的块下标或行下标会沿 Rust 索引/`Chunk::GetRow` 路径 panic，而不是返回 `ChunkError`。`RowPtr` 不记录所属列表，传入另一个列表、在 `Reset`/`Clear` 后复用，或自行构造越界值均未被防护。

`Walk` 是唯一显式可失败路径：闭包的首个 `ChunkError` 会终止遍历并原样传播。Rust 版本没有额外错误上下文包装；Go 版本在对应位置调用 `errors.Trace(err)`。Tracker 的限额/动作行为由 `memory::Tracker::Consume` 实现，本文件只报告用量，不自行返回配额错误。

## 并发与资源生命周期

`List` 不包含锁、原子变量、线程或异步任务；修改操作要求独占 `&mut self`，共享读取使用 `&self`。若跨线程共享，调用方仍须选择满足 trait 约束的所有权与同步方式。`RowContainer` 的实际用法用 `RwLock` 包裹含 `List` 的记录；apply cache 则通过 `Arc<List>` 共享已经构造好的只读值。

资源生命周期分为活跃块与 freelist 两阶段。`Reset` 是复用操作而非释放操作，会保留块分配和 Tracker 记账；复用时 `AllocChunk` 扣掉旧用量并重置块。`Clear` 是释放边界，会清空两组 `Vec<Box<Chunk>>` 并把 Tracker 当前消费量冲销为零。`List` 没有自定义 `Drop`；正常析构依靠 Rust 所有权释放块和 Tracker，但只有显式 `Clear` 才在对象继续存活时同步把 Tracker 数值归零。

## 与 Go 版本的对应关系

直接对照 [`list.go`](list.go) 可见，Rust 保留了 Go 的字段、构造器、延迟记账算法、freelist LIFO 复用、`Add` 冻结末块、两级 `RowPtr`、Reset/Clear 区别和 Walk 顺序。Rust 用 `Vec<Box<Chunk>>` 对应 Go 的 `[]*Chunk`，用所有权接管替代“调用方不得再使用”的约定；`Box<List>` 对应 Go 指针返回值。

主要语言差异是：Go 的字段类型为 `[]*types.FieldType`、容量/长度为 `int`，Rust 使用拥有的 `Vec<FieldType>` 和 `usize`；Go Tracker 是裸指针，Rust 是 `Box<Tracker>`；Go 回调用 `func(Row) error`，Rust 用装箱的 `FnMut` trait object；Go Walk 对错误调用 `errors.Trace`，Rust 直接传播 `ChunkError`。Go 的 `GetMemTracker` 返回可变指针语义，Rust 分成不可变和可变两个借用入口。

[`list_test.rs`](list_test.rs) 将 Go `TestList` 和 `TestListMemoryUsage` 的核心行为拆成三个独立测试：额外明确验证容量为 2 时第三行跨块且指针仍能取值；验证五行形成三块、Reset 后复用、Add 后 Append 新开第 2 块、Walk 顺序；验证最后块在 Reset 时记账、freelist/外加块用量累加以及 Clear 归零。Go 测试还包含 Tracker、Add 和 GetRow benchmark；Rust 对照文件没有移植这些 benchmark。

## 扩展指南

- 修改分块或增长策略时，集中检查 `AppendRow` 与 `AllocChunk`，并保持 `consumedIdx` 的“已记账块不可继续增长”不变量；同步扩展独立的 [`list_test.rs`](list_test.rs)，不要把测试嵌入生产源文件。
- 修改内存统计时，同时验证四个结算点：离开旧末块、`Add`、`Reset`、freelist 复用；`Clear` 必须仍能完全冲销。还应检查 `RowContainer` 的 Tracker 挂接与 spill 后 `Clear` 路径，避免父子 Tracker 双计或漏计。
- 若为 `Add`、`GetRow` 或索引查询增加可恢复错误，属于 API 形状变化，需同步 `row_container.rs`、`iterator.rs` 以及 Go 对照语义；不要仅为避免 panic 而静默忽略错误。
- 若改变 `RowPtr` 表示或大小，应审查 `RowPtrSize`、迭代器、行容器、磁盘行存储以及使用其进行内存估算/序列化的上层代码，并关注 `usize`/`u32` 转换和兼容性。
- 若希望并发修改列表，应在上层建立清晰锁边界；直接向 `List` 植入内部锁会影响 `Row` 生命周期、Tracker 借用和现有 `&mut self` API，不能视为局部改动。
- 性能回归重点是块分配次数、freelist 命中、Tracker 更新频率、`Box` 地址稳定性和 `Walk` 的逐行动态派发。与 Go 对齐时保留 Go benchmark 所表达的 Add/GetRow/Tracker 查询成本关注点。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、目标 `list.rs` 有 20 个符号；`node --file pkg/util/chunk/list.rs --offset 1 --limit 260` 读取完整 231 行并报告 32 个使用文件；`query NewListWithMemTracker --kind function --json` 区分出 Go 与 Rust 两个同名构造器。针对 `impl List` 方法的精确 `callers`/`callees` 未返回边，已明确记录为图覆盖限制，并以直接引用搜索补证。
- 源码与 crate 边界：[`list.rs`](list.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。目标文件没有条件编译项；测试模块由 `lib.rs` 的 `#[cfg(test)] #[path = "list_test.rs"]` 独立挂载。
- 直接 Rust 上下游：[`row_container.rs`](row_container.rs)、[`iterator.rs`](iterator.rs)、[`pkg/executor/internal/applycache/apply_cache.rs`](../../executor/internal/applycache/apply_cache.rs)。
- Go 对照与测试：[`list.go`](list.go)、[`list_test.go`](list_test.go)；Rust 独立测试：[`list_test.rs`](list_test.rs)。
- 人工复核的问题均可由以上符号回答：文件为多 Chunk 可复用行列表而存在；追加、整块接管、定位、遍历、重置和记账流程由 `AppendRow`/`Add`/`GetRow`/`Walk`/`Reset` 驱动；安全扩展需维护地址稳定、指针有效期、延迟记账和外部同步边界。

# `pkg/util/chunk/chunk_util.rs`

## 文件定位

[对应 Rust 源文件](chunk_util.rs) 属于 `astersql-util-chunk` crate。crate 入口 `pkg/util/chunk/lib.rs` 经 `pkg/util/chunk/internal/group1/lib.rs` 的 `chunk_util_impl` 使用 `include!("../../chunk_util.rs")` 注入并重新导出这里的符号，因此调用方以 `chunk::...` 使用这些 API。文件承担三类辅助能力：按选择向量复制列式行、为 Chunk spill 提供临时文件读写层、为纯列投影提供零拷贝列交换。它不是独立执行入口，也不定义常量、trait 或条件编译项。

`pkg/util/chunk/Cargo.toml` 将该目录声明为 `astersql-util-chunk`，并标记 Go 对照包为 `pkg/util/chunk`。本文件直接依赖 crate 内的 `Chunk`、`Column`、Go 风格 `io`/`os`/`errors` 适配，以及配置、checksum、encrypt、disjointset、intest 和 atomic 适配；相应 crate 依赖由 `Cargo.toml` 中的 `config-crate`、`checksum-crate`、`encrypt-crate`、`disjointset-crate` 与 `intest-crate` 提供。

## 核心职责

1. `CopySelectedJoinRowsDirect` 与 `CopySelectedJoinRowsWithSameOuterRows` 把 join 结果中命中的物理行追加到目标 `Chunk`，同时维护 null bitmap、变长列 offsets、列长度和 `numVirtualRows`。
2. `CopySelectedRows`、`CopySelectedRowsWithRowIDFunc`、`CopyExpectedRowsWithRowIDFunc` 与 `CopyRows` 提供列级复制原语，分别支持布尔选择、逻辑下标到物理下标映射、反向条件及显式物理行号列表。
3. `diskFileReaderWriter` 在临时文件上组装可选 AES-CTR 加密与 checksum 层，并以当前写偏移创建区段读取器。实际使用者是 `pkg/util/chunk/chunk_in_disk.rs` 的 `DataInDiskByChunks`。
4. `ColumnSwapHelper` 合并同源输入列的投影映射，再通过交换首个输出列和建立其余输出引用来避免复制。生产调用链是 `pkg/expression/evaluator.rs` 的 `NewEvaluatorSuite` → `EvaluatorSuite::Run` → `ColumnSwapHelper::SwapColumns`。

Rust join 文件 `pkg/executor/join/joiner.rs`、`outer_join_probe.rs`、`base_semi_join.rs` 和 `left_outer_semi_join_probe.rs` 中可见这些复制函数的预期接线，但当前匹配到的调用均在注释中；因此不能据此声称 Rust join 主链已经调用这些 API。当前可确认的运行时接线是表达式列交换和 Chunk spill 两条路径。

## 主要符号

- `CopySelectedJoinRowsDirect(src, selected, dst) -> Result<bool, errors::Error>`：复制源 Chunk 所有列的选中行。空源返回 `false`；源或目标带 selection vector 时返回 `MSG_ERR_SEL_NOT_NIL`；零列 Chunk 只累计虚拟行数。返回值表示是否至少复制一行。
- `CopySelectedJoinRowsWithSameOuterRows(...) -> Result<bool, errors::Error>`：先用 `copySelectedInnerRows` 复制 inner 列，再用 `copySameOuterRows` 从源第 0 行起批量复制 outer 列。调用契约要求所有 outer 行内容相同。
- `CopySelectedRows`：把 `selected[i] == true` 的行按原下标复制，是带映射版本的恒等映射门面。
- `CopySelectedRowsWithRowIDFunc`：仅选择 `true`，但由闭包把 selection 下标映射成源物理行号。
- `CopyExpectedRowsWithRowIDFunc`：列复制的通用实现；固定宽列按 `rowID * elemBuf.len()` 截取，变长列按相邻 offsets 截取，并给目标追加新的累计 offset。
- `CopyRows`：按照 `&[usize]` 给出的次序复制物理行，允许重排或重复。
- `copySelectedInnerRows`：复制连续 inner 列区间并返回选中数量；inner 列数为零时直接统计选择向量。
- `copySameOuterRows`：把源第 0 个逻辑行所在位置开始的连续编码复制 `numRows` 次；固定宽和变长列走不同布局路径。
- `diskFileReaderWriter`：保存文件、最外层 writer、写偏移、checksum writer、可选 cipher writer 与 CTR 密钥/nonce。`Default` 仅创建未初始化状态。
- `diskFileReaderWriter::{initWithFileName,getReader,getSectionReader,getWriter,write}`：分别初始化包装链、重建随机读链、限定读取区间、暴露 writer、写入并推进偏移。
- `ColumnSwapHelper::{New,SwapColumns,mergeInputIdxToOutputIdxes}`：保存输入列到输出列列表的静态映射，并延迟缓存基于实际输入引用关系合并后的映射。
- `NewColumnSwapHelper(usedColumnIndex)`：从“每个输出列使用哪个输入列”的数组反向构建映射；当前 `pkg/expression/evaluator.rs` 直接调用 `ColumnSwapHelper::New`，此自由函数在已检查的非测试 Rust 代码中没有调用证据。

## 执行流程

直接 join 复制流程如下：先拒绝带 `sel` 的源或目标；若没有物理列，只按 `selected` 的真值数增加目标虚拟行。否则逐列遍历选择向量：固定宽列复制固定字节片段，变长列复制 offsets 界定的字节片段；两者都同步追加 null 位并增加列长度。最后以目标首列长度差计算实际复制数，并增加 `dst.numVirtualRows`。

同 outer 行优化流程如下：`copySelectedInnerRows` 对 inner 列复用列级选择复制；随后 `copySameOuterRows` 取 `src.GetRow(0)`，为每个 outer 列一次性追加 `numRows` 个 null 位和数据。固定宽列直接复制连续 `numRows * elemLen` 字节；变长列复制连续数据区，并用第一个元素长度重复生成目标 offsets。这依赖调用方保证所有待复制 outer 值编码等长且内容相同，正是公开函数注释中的“所有 outer rows 相同”前置条件。

spill 初始化时，`initWithFileName` 在全局 `TempStoragePath` 下创建随机后缀临时文件。非 plaintext 配置先构造 CTR cipher 和加密 writer，然后无论是否加密都在其外层套 checksum writer，最终 `writer` 指向 checksum 层。读取时 `getReader` 从原始文件开始，以写端保存的 cache/offset 恢复加密读取层，再以 checksum cache/offset 恢复校验读取层；`getSectionReader(off)` 将范围限制为 `[off, offWrite)`。`DataInDiskByChunks::Add` 调用 `write`，`GetChunk`/`FillChunk` 经 `getSectionReader` 回读，`Close` 负责 tracker 归零、关闭和删除文件。

列交换流程如下：`NewEvaluatorSuite` 将纯 `Column` 表达式聚合为“输入列 → 输出列们”的映射。首次 `SwapColumns` 根据实际 input 中 `Column::same_ref` 的结果，用并查集合并共享底层列的输入下标；CAS 只发布一个合并结果。随后每组先把根输入列交换到第一个输出位置，再让其余输出位置 `MakeRef` 到第一个输出列。以后调用直接复用已发布映射。

## 数据与状态

行复制直接修改 `Column` 的 `nullBitmap`、`length`、`data` 和（变长列）`offsets`，并修改 `Chunk::numVirtualRows`；它是追加语义，不会清空目标。固定宽判定来自 `Column::IsFixed()`，元素宽度来自 `elemBuf.len()`。所有 selection 下标、列区间与 row ID 都被当作可信的内部参数，没有在本文件内做长度校验。

`diskFileReaderWriter` 的有效状态从 `Default` 的全空值，经 `initWithFileName` 变为文件和 writer 层均存在；`offWrite` 是区段读取上界。`write` 按底层实际返回字节数推进它。上游 `DataInDiskByChunks::Add` 还按照 Go 版本逻辑再次以序列化长度推进 `offWrite`；这是当前 Rust/Go 都存在的可观察实现，不应在文档任务中擅自修正。读取缓存来自 writer 层，使尚在 writer cache 中的数据也能通过对应 reader 看到。

`ColumnSwapHelper::InputIdxToOutputIdxes` 在构造后保存原始投影关系；`mergedInputIdxToOutputIdxes` 是原子指针，初始为空，首次看到实际 input 结构后只发布一次。其缓存隐含不变量是同一 helper 后续输入具有兼容的列引用拓扑。合并映射内部的输出下标顺序来自原映射的向量；不同输入组的遍历顺序由 `HashMap` 决定，但组间交换彼此独立。

## 依赖与调用关系

上游已确认关系：

- `pkg/expression/evaluator.rs::NewEvaluatorSuite` 收集纯列表达式并构造 `ColumnSwapHelper`；`EvaluatorSuite::Run` 在默认表达式求值后调用 `SwapColumns`。
- `pkg/util/chunk/chunk_in_disk.rs::DataInDiskByChunks::initDiskFile` 调用 `initWithFileName`，`Add` 调用 `write`，`readFromFisk` 调用 `getSectionReader`，`Close` 接管并清理 `file`。
- `pkg/util/chunk/chunk_util_test.rs` 直接覆盖选择复制、同 outer 行复制、虚拟行和同源列合并；`pkg/util/chunk/chunk_in_disk_test.rs` 通过完整 spill 往返间接覆盖文件读写层。
- join 目录中仅有注释形式的复制函数调用，未形成可确认的 Rust 静态运行边。

下游依赖关系：复制函数调用 `Chunk::{NumRows,GetRow}`、`Column::{IsFixed,IsNull,appendNullBitmap,appendMultiSameNullBitmap}`；列交换调用 `Chunk::{Column,swapColumn,MakeRef}` 和 `Column::same_ref`。spill 路径调用 `config::GetGlobalConfig`、`os::CreateTemp`、`encrypt::{NewCtrCipher,NewWriter,NewReader}`、`checksum::{NewWriter,NewReader}`、`NewReaderWithCache` 与 `io::NewSectionReader`。映射合并调用 `disjointset::Set::{Union,FindRoot,FindVal}`、`intest::Assert` 和 `atomic::Pointer::{Load,CompareAndSwap}`。

## 错误处理与边界

两个公开 join 复制函数只显式返回 selection vector 非空错误；空源正常返回 `Ok(false)`。磁盘初始化把临时文件创建错误用 `errors::Trace` 包装，cipher 创建错误直接传播，`write` 传播 writer 错误。`SwapColumns` 传播 `swapColumn` 的索引/状态错误；`MakeRef` 没有返回错误。

边界与调用者责任包括：`selected` 必须覆盖被访问的源物理行；`start..end` 必须落在 selection 内；映射后的 row ID、列 offset/length 和目标列位置必须有效；源、目标 Chunk 的列布局必须匹配。违反这些条件会因 Rust slice 索引或 `unwrap` 而 panic，而不是返回结构化错误。`getReader`、`getWriter` 和 `getSectionReader` 也假定初始化已完成；在空状态调用会 panic。`getSectionReader` 未校验 `off <= offWrite`。

`copySameOuterRows` 的优化成立条件比类型系统表达得更强：outer 行必须相同，且源中从首行物理位置开始存在足够的连续编码。变长列 offsets 通过重复首元素长度重建，若调用者违反相同 outer 行契约，结果不会逐行验证。并查集合并使用 `intest::Assert(ok, &[])` 确认根值存在；这是内部不变量检查而非可恢复错误。

## 并发与资源生命周期

行复制和列交换都会原地修改 Chunk/Column；它们本身不加锁，调用方必须保证可变 Chunk 不被并发访问。`DataInDiskByChunks::Add` 的注释明确禁止并发调用；`diskFileReaderWriter` 也没有为 writer、offset 或缓存提供同步。

`ColumnSwapHelper` 唯一的并发协调点是合并映射的 CAS：多个 worker 可同时计算，但只有首个空指针替换成功，失败者不覆盖已发布值，保持 Go 的“一次初始化”语义。当前字段使用裸指针且本文件没有 `Drop` 实现；成功发布的 `Box<HashMap<...>>` 在本文件中没有回收路径，生命周期事实应视为与 helper 共存但未显式释放，而不能假定自动回收。安全扩展时还需确认 `atomic::Pointer` 的线程安全和内存序保证，而不是仅凭注释推断。

临时文件由 `os::CreateTemp` 创建，实际清理由 `DataInDiskByChunks::Close` 完成，而非 `diskFileReaderWriter` 自身。调用方若不执行 `Close`，本文件没有独立的删除保障。加密状态的 `CtrCipher`、cipher writer cache 和 checksum writer cache 必须一起保留到读取结束，否则 `getReader` 无法重建与写路径一致的视图。

## 与 Go 版本的对应关系

直接对照 `pkg/util/chunk/chunk_util.go`，Rust 文件保留了同名 API、分支顺序和数据布局操作：固定/变长列分别复制、零列只累计虚拟行、同 outer 行批量复制、checksum 包在可选 encrypt writer 外、读侧按 encrypt 后 checksum 的次序恢复、并查集合并同源列后 CAS 发布。`pkg/util/chunk/chunk_in_disk.go` 也确认 `Add` 在 `dataFile.write` 后再次增加 `offWrite`，Rust 当前行为不是本次移植单独引入的简化。

语言层差异主要是：Go 的指针、slice 与 `atomic.Pointer[map[int][]int]` 被映射为 Rust 引用、slice、`HashMap<usize, Vec<usize>>` 和 crate 的 `atomic::Pointer`；Go 的 `error` 被映射为 `Result<_, errors::Error>`；Go 的 `nil` writer/file 状态映射为 `Option`。Rust `rowIDFunc` 使用泛型闭包而非 Go 函数值。`Column::same_ref` 替代 Go 的列指针相等判断，以表达共享底层列引用。

测试对应关系：Rust `selected_join_rows_copy_fixed_variable_null_and_virtual_rows` 覆盖 Go `TestCopySelectedJoinRowsDirect` 的核心路径并补 selection 错误；`selected_join_rows_with_same_outer_rows_match_go` 对应 `TestCopySelectedJoinRows`；`selected_virtual_rows_match_go_edge_cases` 对应 `TestCopySelectedVirtualNum`；`column_swap_merges_referred_input_columns_like_go` 对应 `TestMergeInputIdxToOutputIdxes`。Go 测试还包含 1024 行随机/多类型对比与 benchmark，Rust 独立测试目前是更小的确定性样例，不能宣称覆盖强度完全相同。

## 扩展指南

新增行复制能力时，优先扩展 `CopyExpectedRowsWithRowIDFunc` 这一通用核心，并同时检查固定宽与变长列的 null bitmap、length、data、offsets 和 Chunk 虚拟行计数。不要把 Rust 测试内嵌到源文件；应更新同目录独立文件 `pkg/util/chunk/chunk_util_test.rs`，并与 `chunk_util_test.go` 的边界保持一致。若把这些函数接入 join 生产路径，应在对应 join 独立测试中验证真实调用，而不能把当前注释代码视为接线证据。

修改 spill 包装层时，必须维持写侧“可选 encrypt → checksum”和读侧相匹配的缓存/offset 恢复顺序，并联动检查 `pkg/util/chunk/chunk_in_disk.rs`、`chunk_in_disk_test.rs` 及 Go 对照文件。特别关注部分写、初始化中途失败、`offWrite` 口径、plaintext/AES 两种配置、checksum 损坏和 `Close` 后行为；当前 Rust 独立测试未在本文件测试中直接覆盖所有这些分支。

修改列交换时，应保持“先合并同源输入，再交换一次并建立多个引用”的不变量。若输入引用拓扑可能随调用变化，需要重新设计一次性缓存，而不是继续复用首次映射。还应为 CAS 竞争、helper 析构/指针回收、空映射、非法列下标和多批次不同拓扑增加独立测试；性能上避免把零拷贝路径退化为逐值复制。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `pkg/util/chunk/chunk_util.rs` 已索引且有 21 个符号；`files --filter` 确认目标在图中；`node --file ... --offset 1 --limit 400` 与后续 401 行读取覆盖完整 434 行源码。精确 `query CopySelectedJoinRowsDirect` 同时定位 Go/Rust 定义和 Go 测试；`query SwapColumns`/`initWithFileName` 未能区分或返回 Rust impl，`callers`/`callees` 亦未给出可用边，因此没有把缺失图边包装成结论。
- 已读生产与装配文件：`pkg/util/chunk/chunk_util.rs`、`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/lib.rs`、`pkg/util/chunk/internal/group1/lib.rs`、`pkg/util/chunk/chunk_in_disk.rs`、`pkg/expression/evaluator.rs`。
- 已读 Go 对照：`pkg/util/chunk/chunk_util.go`、`pkg/util/chunk/chunk_in_disk.go`。
- 已读测试：`pkg/util/chunk/chunk_util_test.rs`、`pkg/util/chunk/chunk_util_test.go`；并检查 `pkg/util/chunk/chunk_in_disk_test.rs`/`.go` 中 `DataInDiskByChunks` 的写入、`GetChunk`、`FillChunk` 与 `Close` 覆盖位置。
- 调用搜索：非测试 Rust 中确认 `EvaluatorSuite::Run → ColumnSwapHelper::SwapColumns` 和 `DataInDiskByChunks → diskFileReaderWriter`；复制 API 在 executor join 文件中的命中为注释，已明确标为尚无生产接线证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令检查目标存在且固定二级标题恰为 11 个，并人工复核文档能够回答文件存在原因、运行路径与安全扩展点。

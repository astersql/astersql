# `pkg/util/chunk/chunk_in_disk.rs`

## 文件定位

本文说明的真实源文件是 [`chunk_in_disk.rs`](chunk_in_disk.rs)。它属于 Cargo crate `astersql-util-chunk`（`pkg/util/chunk/Cargo.toml`），实现以 `Chunk` 为单位的临时磁盘 spill 容器。它不是独立 Rust 模块文件：`pkg/util/chunk/internal/group1/lib.rs` 的 `chunk_in_disk_impl` 通过 `include!("../../chunk_in_disk.rs")` 注入实现，再由 `pub use chunk_in_disk_impl::*` 以及 crate 根的 `pub use group_1::*` 对外导出。

该容器位于列式 `Chunk` 与临时文件 I/O 之间：上游提交完整 `Chunk`，本文件负责序列化、记录每块起始偏移和磁盘用量，之后按块索引恢复。当前检索到的活跃生产接线是 `pkg/executor/aggregate/agg_hash_partial_worker.rs` 中的 `PartialResultSpill`：它按分区创建 `DataInDiskByChunks`，通过 `Add` 落盘聚合中间结果，再用 `GetChunk` 恢复。`pkg/executor/aggregate/agg_spill.rs`、若干 join 文件中的同名内容目前是注释化的 Go 迁移草稿，不应视为已接线调用者。

## 核心职责

- `DataInDiskByChunks` 管理一串追加写入的序列化 Chunk，以及每块的逻辑起始偏移、累计字节数、累计行数和磁盘 tracker。
- `Add` 延迟创建临时文件，拒绝空 Chunk，把 Chunk 头、selection vector 与每列的元数据和原始字节写入复用缓冲区，再追加到文件。
- `GetChunk` 创建与 `fieldTypes` 匹配的新 Chunk；`FillChunk` 则复用调用者提供的 Chunk。两者都先按索引读出完整块，再执行同一反序列化流程。
- `Close` 清零磁盘计量、关闭文件并删除临时文件；该清理是显式生命周期操作，不由本文件中的 `Drop` 自动保证。
- `injectChunkInDiskRandomError` 在读、写入口模拟稀有错误或短延迟，用于与 Go failpoint 行为对齐。

本文件只保存临时、进程内可恢复的数据，不定义跨进程、跨版本或跨机器的持久化格式。

## 主要符号

- 尺寸常量 `byteLen`、`intLen`、`int64Len` 分别取本机 `u8`、`isize`、`i64` 宽度；`chkFixedSize = intLen * 4`，`colMetaSize = int64Len * 4`。
- `DefaultChunkDataInDiskByChunksPath` 是临时文件名的固定中段；`initDiskFile` 还拼接测试前缀和 tracker label，真正的随机文件名由 `diskFileReaderWriter::initWithFileName` 创建。
- `DataInDiskByChunks` 的公开字段保存：字段类型 `fieldTypes`、块偏移 `offsetOfEachChunk`、累计量 `totalDataSize`/`totalRowNum`、`diskTracker`、底层 `dataFile`、复用缓冲区 `buf` 和测试文件名前缀。
- 构造函数 `NewDataInDiskByChunks(Vec<FieldType>, String) -> Box<DataInDiskByChunks>` 建立空容器；tracker 使用 `memory::LabelForChunkDataInDiskByChunks` 和 `-1` 配额，表示当前不限制磁盘用量。
- 公开操作入口为 `Add`、`GetChunk`、`FillChunk`、`Close`、`GetDiskTracker`、`GetTotalBytesInDisk`、`NumRows`、`NumChunks`。虽然若干序列化辅助函数也声明为 `pub`，它们依赖容器内部 `buf` 和位置游标，语义上是实现细节。
- 写路径由 `serializeDataToBuf`、`serializeChunkData`、`serializeColumns`、`serializeColMeta`、`serializeOffset` 组成；读路径由 `readFromFisk`（保留 Go 原拼写）、`deserializeDataToChunk`、`deserializeChunkData`、`deserializeSel`、`deserializeColumns`、`deserializeColMeta`、`deserializeOffsets` 组成。
- 私有 `ensure_len`、`put_i64`、`get_i64`、`put_int`、`get_int` 用安全切片操作表达 Go `unsafe.Pointer` 的本机字节序读写。

## 执行流程

1. `NewDataInDiskByChunks` 保存字段类型，初始化空偏移表和计数器，并预留 4096 字节缓冲区；此时尚未创建文件。
2. `Add` 先执行 `injectChunkInDiskRandomError`，再通过 `Chunk::NumRows` 拒绝零行输入。首次写入时，`initDiskFile` 检查/创建临时目录并初始化 `diskFileReaderWriter`。
3. `serializeDataToBuf` 先计算精确逻辑长度：固定头、可选 selection、每列四个 `i64` 元字段，以及 `nullBitmap`、`data`、`offsets`。随后清空或扩容 `buf`，按相同次序写入。
4. 固定头依次记录 `numVirtualRows`、`capacity`、`requiredRows`、以字节计的 `selSize`；selection 中每个索引按本机 `isize` 写入。每列依次记录 `length`、三段 payload 的字节数，然后串接 null bitmap、列数据和 `i64` offsets。
5. `Add` 将完整 `buf` 一次交给 `diskFileReaderWriter::write`。只有返回字节数等于计算长度时，才提交块偏移、累计字节/行数和 tracker 消耗；写错误或短写不会更新这些逻辑元数据。
6. `GetChunk`/`FillChunk` 调用 `readFromFisk`：用 `offsetOfEachChunk` 和相邻偏移（末块用 `totalDataSize`）算出块长，构造 section reader，将整块读入复用 `buf`。
7. `deserializeDataToChunk` 按写入顺序恢复头和各列。`GetChunk` 的目标由 `NewEmptyChunk(fieldTypes.clone())` 创建；`FillChunk` 要求调用者提供列布局兼容的目标。恢复函数按目标 Chunk 已有列数消费磁盘列数据。
8. 使用完毕后调用 `Close`：tracker 反向消费全部已记账字节，关闭文件句柄并删除文件。再次调用时因 `file` 已被 `take()`，不会重复清理。

## 数据与状态

序列化布局为：

`[numVirtualRows | capacity | requiredRows | selSize | sel...]`，随后对每列重复 `[length | nullMapSize | dataSize | offsetSize | nullBitmap... | data... | offsets...]`。

关键不变量如下：

- `offsetOfEachChunk.len()` 就是 `NumChunks()`；成功 `Add` 时先记录旧的 `totalDataSize`，因此每项指向对应块的逻辑起点。
- `totalDataSize` 和 `diskTracker.BytesConsumed()` 在每次成功追加后增加同一序列化长度；`Close` 将 tracker 清零，但不会重置 `totalDataSize`、`totalRowNum` 或偏移表，因此关闭后的对象不是可重新打开并继续读取的全新容器。
- `totalRowNum` 只统计成功落盘 Chunk 的 `NumRows()`；空 Chunk 在创建文件前即被拒绝。
- `buf` 在写路径和读路径间复用，容量可能保留峰值；tracker 只记录序列化落盘字节，不记录该内存容量。
- 格式不写列数或字段类型；列数由恢复目标 Chunk 的 `columns` 决定，解释列数据所需的类型由构造时的 `fieldTypes` 或 `FillChunk` 调用者保证。
- 数字使用本机字节序，selection 使用本机指针宽度 `isize`。因此该格式只适合作为同一进程/架构内的临时文件，不能作为稳定交换格式。

`pkg/util/chunk/chunk_util.rs` 的当前 Rust `diskFileReaderWriter::write` 已自行增加 `offWrite`，而本文件的 `Add` 成功后又增加一次 `offWrite`；Go 对照版本只在 `Add` 中推进该字段。这是当前源码可观察到的 Rust/Go 差异，修改 section-reader 边界或写偏移逻辑时必须一并核查，不能假定两处推进都与 Go 完全等价。

## 依赖与调用关系

向上调用关系：

- `pkg/executor/aggregate/agg_hash_partial_worker.rs::PartialResultSpill::new` 为每个聚合分区构造容器；`flush` 调 `Add` 并通过 `GetTotalBytesInDisk` 统计本次写入；`restore_partition` 按偏移表遍历 `GetChunk`，消费完后调用 `Close`。
- `pkg/util/chunk/chunk_in_disk_test.rs` 与 `pkg/util/chunk/alloc_1_aster_unit_test.rs` 直接构造并验证容器。RustCodeGraph 的文件节点还报告目标文件被 13 个文件使用，但同名方法在 chunk/list/row 容器中很多，精确调用边需要以上下文消歧；本文只把定向源码检索确认的生产调用点列为已接线事实。

向下依赖关系：

- `Chunk`、`Column`、`NewEmptyChunk`、`types::FieldType` 来自同一 `group_1`/crate 导出层。
- `disk::CheckAndInitTempDir` 和 `disk::Tracker` 负责临时目录与磁盘计量；`memory::LabelForChunkDataInDiskByChunks` 提供 tracker 标签。
- `diskFileReaderWriter` 定义在 `pkg/util/chunk/chunk_util.rs`，负责临时文件、checksum 以及配置启用时的 AES-CTR 包装；本文件只使用其初始化、追加写和 section reader 接口。
- `errors::{New, Trace}` 统一逻辑错误与 I/O 错误；`terror::{Call, Log}` 用于吞掉但记录关闭/删除错误。
- `failpoint`、`rand`、`time` 只服务于 `injectChunkInDiskRandomError`。`pkg/util/chunk/Cargo.toml` 对应声明 `fail`、`rand-crate`，并通过本地 path 依赖接入 disk、memory、checksum、encrypt 和 types 等 crate。

## 错误处理与边界

- `Add` 明确返回 failpoint 错误、空 Chunk 错误、临时目录/文件初始化错误、底层写错误和短写错误。只有完整写成功才更新偏移、累计量和 tracker。
- `readFromFisk` 返回 failpoint 错误、读取错误或短读错误；`GetChunk`/`FillChunk` 原样传播。函数名中的 `Fisk` 是沿用 Go 源码的拼写，不代表另一种存储层。
- `Close` 没有返回值；关闭失败交给 `terror::Call`，删除失败交给 `terror::Log`。调用方无法通过返回值确认物理清理成功。
- `getChunkSize`、`readFromFisk` 直接索引偏移数组，没有显式检查空容器或 `chkIdx` 越界；无效索引会触发 Rust panic，而非返回 `errors::Error`。
- 反序列化辅助函数信任磁盘元数据和目标列布局。负数长度转 `usize`、越界切片、`try_into().unwrap()`、字段数不匹配或损坏/截断数据均可能 panic。checksum reader 可能在更早阶段报告损坏，但本文件自身不做格式版本、范围或列数校验。
- `Add` 的注释明确禁止并发调用；字段类型一致性也由调用者保证，没有运行时校验。`FillChunk` 还要求目标 Chunk 已具有正确列数和相容类型。
- `deserializeChunkData` 只在 `selSize != 0` 时写 `chk.sel`；若复用目标在调用前保留旧 selection 而磁盘块没有 selection，本函数不会主动设为 `None`。当前测试在 `FillChunk` 前调用 `Reset`，扩展复用场景时需要验证这一状态约束。

## 并发与资源生命周期

`DataInDiskByChunks` 持有可变缓冲区、追加偏移和可变文件游标，公开读写接口都需要 `&mut self`；设计上是单所有者、串行读写，且 `Add` 明确声明不可并发。`injectChunkInDiskRandomError` 可能让当前操作睡眠 5 至 14 毫秒，但不创建后台任务、线程或通道。

临时文件在第一次非空 `Add` 时懒创建，而不是在构造时创建。`Close` 是资源释放边界：先把 tracker 归零，再关闭和删除文件，并把 `dataFile.file` 置空。文件中没有 `Drop for DataInDiskByChunks`，所以提前返回、panic 或调用方遗忘 `Close` 时，本文件不保证 tracker 与临时文件立即释放；上游 `PartialResultSpill::restore_partition` 在正常恢复路径显式关闭，错误/panic 路径的资源策略应在上游扩展时单独审查。

测试文件名前缀用于并行测试的命名隔离；实际文件创建仍由临时文件接口附加随机部分。容器本身没有锁，不能因文件名隔离而推断同一个实例可并发使用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/chunk/chunk_in_disk.go`，Rust 基本保留了 Go 的结构字段、公开方法名、序列化字段顺序、错误文本、failpoint 概率分支与显式 `Close` 语义：随机数小于 3 返回错误，3 至 5 睡眠 5 至 14 毫秒，其余继续执行。

主要语言映射为：Go `[]byte`/`[]int64` 对应 Rust `Vec<u8>`/`Vec<i64>`，Go `int` 对应本文件的 `isize`，Go `unsafe.Pointer` 原生序读写改为 `to_ne_bytes`/`from_ne_bytes` 和边界受切片检查的 helper，Go `*DataInDiskByChunks` 对应 `Box<DataInDiskByChunks>`。Rust `fieldTypes` 保存值类型 `Vec<FieldType>`，创建新 Chunk 时显式 clone；Go 保存指针切片。

已确认差异包括：

- Rust `deserializeSel` 总是分配一个新 `Vec`，Go 会在容量足够时复用 selection；功能相同但分配特征不同。
- Rust 列反序列化通过 `resize` 复用已有 `Vec` 容量，与 Go 按 capacity 选择复用或分配的意图相同。
- Rust `diskFileReaderWriter::write` 与 `Add` 都推进 `offWrite`，而 Go 版本的 `write` 不推进、只由 `Add` 推进；这是需要后续行为修复任务独立验证的迁移差异，本纯文档任务不修改它。
- Rust 独立测试新增了空 Chunk 被拒绝且不产生逻辑磁盘状态的断言，以及 failpoint 回调基础行为；Go `chunk_in_disk_test.go` 主要覆盖 `GetChunk`/`FillChunk` 往返。

## 扩展指南

- 新增序列化字段时，应同时修改尺寸计算、写入和读取三处，并保持顺序严格一致；同步更新 `chkFixedSize`/`colMetaSize` 或引入显式格式版本。必须同时核对 Go 对照实现，不能只让 Rust 自洽。
- 若要把临时格式用于跨进程恢复，应先定义固定端序、固定整数宽度、列数/类型信息、格式版本与长度上限；当前本机布局不能直接升级为持久化协议。
- 若增加索引检查或损坏数据防护，接入点是 `GetChunk`/`FillChunk`、`getChunkSize` 及各 `deserialize*` helper。应把 panic 转为可传播错误，并新增独立测试覆盖越界索引、截断头、非法长度、列数不匹配和 checksum 错误。
- 若改变资源所有权，优先明确 `Close` 的幂等性以及是否实现 `Drop`，并核对 tracker、关闭顺序、删除错误和上游 `PartialResultSpill` 的错误/panic 路径。不要在测试逻辑中内嵌到生产文件；测试继续放在 `pkg/util/chunk/chunk_in_disk_test.rs`。
- 若优化内存，重点观察 `buf` 峰值保留、`deserializeSel` 分配和 `fieldTypes.clone()`；性能调整不能改变 selection、null bitmap、变长列 offsets 和辅助 Chunk 元数据的往返结果。
- 若调整底层 I/O，必须联合检查 `pkg/util/chunk/chunk_util.rs::diskFileReaderWriter::{write,getSectionReader}` 与本文件对 `offWrite` 的维护，先解决或明确双重推进的预期，再建立回归测试。
- 新增生产调用者时，应维持“字段类型匹配、索引有效、单实例串行访问、所有退出路径最终 Close”的契约，并把容器 tracker 挂接/汇总到所属执行器的资源跟踪体系。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/util/chunk` 确认目标 Rust/Go/测试文件均已索引；`node --file pkg/util/chunk/chunk_in_disk.rs --offset 1 --limit 420` 读取了完整 408 行和 31 个符号，并给出 13 个文件级使用者。`query DataInDiskByChunks`、`query NewDataInDiskByChunks` 同时命中 Go/Rust 定义；精确 `callers/callees` 因同名符号歧义未产生可用边，因此用定向源码检索消歧，未把模糊图结果当作确定调用边。
- 源与装配：`pkg/util/chunk/chunk_in_disk.rs`；`pkg/util/chunk/internal/group1/lib.rs` 的 `chunk_in_disk_impl`；`pkg/util/chunk/lib.rs` 的 group re-export 和独立测试挂接。
- crate 与下游：`pkg/util/chunk/Cargo.toml`；`pkg/util/chunk/chunk_util.rs` 的 `diskFileReaderWriter`；`pkg/executor/aggregate/agg_hash_partial_worker.rs` 的 `PartialResultSpill::{new,flush,restore_partition}`。
- Go 对照：`pkg/util/chunk/chunk_in_disk.go`；Go 测试 `pkg/util/chunk/chunk_in_disk_test.go`。
- Rust 测试：`pkg/util/chunk/chunk_in_disk_test.rs` 覆盖 100 个、每个 1000 行 Chunk 的两种恢复路径，含字符串、整数、null、JSON、Chunk 元数据、selection、计量与关闭；还覆盖空 Chunk 拒绝和 failpoint 回调。`pkg/util/chunk/alloc_1_aster_unit_test.rs` 另有直接构造、字节计量、恢复与关闭检查。
- 本任务是纯文档分析，按计划不运行 Cargo；验收只执行固定十一章节的结构命令，并人工复核上述符号、调用边和边界均可回到列出的源码证据。

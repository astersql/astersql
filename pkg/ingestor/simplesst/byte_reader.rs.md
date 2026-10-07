# `pkg/ingestor/simplesst/byte_reader.rs`

## 文件定位

本文件属于 `astersql-ingestor-simplesst` crate（`pkg/ingestor/simplesst/Cargo.toml`，crate 根为同目录 `lib.rs`），提供简单 SST 读取链路最底层的“按精确字节数读取”能力。`lib.rs` 通过 `pub mod byte_reader` 装配模块，并把 `ByteReader` 再导出到 crate 根。

在上层链路中，`kv_reader.rs::KVReader` 用它依次读取大端 `u64` 的 key/value 长度和记录正文；`stat_reader.rs::StatsReader` 用它读取大端 `u32` 的属性长度和属性正文。`iter.rs::MergeKVIter` 再通过 `KVReader` 消费多个 SST 数据对象，并在热点重平衡时配置和切换本文件的并发模式。因此该文件处于“内存对象存储 -> 精确字节流 -> KV/属性解码 -> 多路归并”的边界。

当前 Rust 实现是内存后端实现：`ByteReader::from_storage` 会先用 `MemoryStorage::read` 克隆整个对象，再由 `ByteReader` 在 `Arc<Vec<u8>>` 上维护逻辑偏移。它不是 Go 版本所使用的流式 `objectio.Reader`，也不直接向真实外部存储发起范围请求。

## 核心职责

1. `ByteReader::new` 和 `ByteReader::from_storage` 校验构造参数，建立不可变共享数据及可变逻辑偏移。
2. `ByteReader::read_n_bytes` 要么返回恰好 `count` 个字节的新 `Vec<u8>`，要么返回参数、关闭状态、正常 EOF 或截断错误；不会返回短成功结果。
3. `enable_concurrent_read` 保存并发度和每分片缓冲大小，`switch_concurrent_mode` 保存调用方期望，并在下一次成功读取时惰性建立 `ConcurrentFileReader`。
4. `close` 清除并发读取器并阻止后续读取；`position`、`buffer_size` 和 `concurrent_mode` 提供状态观测。
5. `readNBytes`、`SwitchConcurrentMode`、`Close`、`openStoreReaderAndSeek` 和 `newByteReader` 保留 Go 风格命名，作为移植兼容入口转发到 Rust 风格 API。

需要注意当前实现边界：`small_buffer_size` 只保存构造参数，`read_n_bytes` 直接从完整对象切片复制，不按该大小分批读取；并发模式会构造或重建 `ConcurrentFileReader` 来同步续读偏移，但本文件没有调用 `ConcurrentFileReader::read`。所以并发状态机和参数约束已存在，Go 版本真实的外部存储并发预取吞吐路径尚未在这里等价落地。

## 主要符号

- `ConcurrentReaderBufferSizePerConc: AtomicUsize`：每个并发分片的默认大小，初值 8 MiB。`iter.rs::rebalance_hotspot` 以 `Ordering::Acquire` 读取它并传给 `KVReader::enable_concurrent_read`；`get_concurrent_reader_concurrency` 还用它把内存预算换算成并发数。
- `CONCURRENT_READER_TOTAL_CONCURRENCY: usize`：单个 reader 的并发配置上限 256；`enable_concurrent_read` 强制执行该上限。
- `MAX_READ_SIZE: usize`：单次精确读取上限 1 GiB，仅文件内可见。
- `ByteReader`：核心状态对象。`data` 是完整对象的共享只读字节；`position` 是下一次读取起点；`small_buffer_size` 是配置值；`concurrent_enabled`、`concurrent_expected`、`concurrent_now` 描述并发能力和模式；`concurrency`、`buffer_size_per_concurrency` 保存并发参数；`concurrent_reader` 保存续读偏移模型；`closed` 是终止状态。
- `ByteReader::new(data, initial_offset, buffer_size)`：要求 `buffer_size > 0` 且 `initial_offset < data.len()`。空对象或恰好位于文件尾的初始偏移都返回 `Error::Eof`。
- `ByteReader::from_storage(store, name, initial_offset, buffer_size)`：先执行 `MemoryStorage::read(name)`，再调用 `new`；存储的 `NotFound`/`Poisoned` 等错误原样传播。
- `ByteReader::enable_concurrent_read(concurrency, buffer_size_per_concurrency)`：要求并发度在 `1..=256`，且每路缓冲非零；只写配置，不创建线程、不读数据。
- `ByteReader::switch_concurrent_mode(use_concurrent)`：未启用并发读时是成功的空操作。启用后写入期望模式；从实际并发态切回顺序态时取走 reader，并以其 `offset()` 同步 `position`。
- `ByteReader::read_n_bytes(count)`：本文件的主执行入口，返回拥有所有权的精确长度字节向量。
- `ByteReader::close()`：清除 reader、把实际模式设为 false，并永久设置 `closed = true`。重复调用仍返回成功。
- `openStoreReaderAndSeek(...)`：Rust 兼容入口，实际返回已经定位的 `ByteReader`；把 `prefetch_size` 提升到至少 1。
- `newByteReader(data, buffer_size)`：以偏移 0 调用 `ByteReader::new`。

文件没有 trait、泛型 API 或条件编译项。公开 API 同时包含 Rust snake_case 和 Go 风格别名；`ensure_open` 与 `MAX_READ_SIZE` 是内部实现。

## 执行流程

构造流程如下：

1. `new` 先拒绝零缓冲。
2. 再检查初始偏移；条件是 `initial_offset >= data.len()`，因此空数据和文件尾偏移直接产生正常 EOF。
3. 数据转入 `Arc<Vec<u8>>`；初始模式为顺序读，默认并发度为 1，默认每路大小等于 `buffer_size`，尚无 `ConcurrentFileReader`。

一次 `read_n_bytes(count)` 的流程如下：

1. `ensure_open` 先拒绝已关闭 reader。
2. 拒绝 `count == 0` 和 `count > 1 GiB`。
3. 用 `checked_add` 计算 `end = position + count`，防止 `usize` 溢出。
4. 若 `end` 越过对象末尾：当 `position` 已在末尾时返回 `Error::Eof`；若仍有部分数据，则把位置推进到末尾、清除并发 reader 和两种并发模式标志，再返回带请求量与剩余量上下文的 `Error::UnexpectedEof`。
5. 若调用方已期望并发模式但实际模式尚未开启，则以当前偏移、文件长度和已保存配置构造 `ConcurrentFileReader`，并设置 `concurrent_now = true`。
6. 直接复制 `data[position..end]` 到新的 `Vec<u8>`，再把 `position` 推进到 `end`。
7. 如果处于并发模式，以新位置重新构造 `ConcurrentFileReader`，使其 `offset()` 等于下次逻辑读取起点。

模式切换是非对称的：切入只设置 `concurrent_expected`，实际 reader 延迟到下一次成功读取时创建；切出则在 `switch_concurrent_mode(false)` 内立即取走 reader、同步偏移并关闭实际模式。`iter.rs::rebalance_hotspot` 正是先配置热点 reader，再切入；热点改变或迭代器关闭时切出。

## 数据与状态

核心不变量由实现和测试共同限定：

- 成功读取后 `position` 恰好增加 `count`，返回向量长度也恰好为 `count`。
- `data` 在构造后不变，多个内部 reader 状态通过 `Arc` 共享同一底层对象；返回值是独立拷贝，不借用内部缓冲。
- `concurrent_now == true` 时，正常成功读取后 `concurrent_reader` 存在，且其 `offset()` 与 `position` 同步。切回顺序模式时用这个偏移恢复逻辑位置。
- 部分越界读取具有消耗语义：`position` 被推进到文件尾，并发状态被复位；下一次正长度读取得到正常 EOF。
- `concurrent_expected` 与 `concurrent_now` 可能短暂不同：请求切入后、下一次成功读取前为 `(true, false)`；构造并发 reader 后为 `(true, true)`；请求切出后为 `(false, false)`。
- `closed` 一旦为 true 不会恢复；本类型没有 reopen 操作。

`ConcurrentReaderBufferSizePerConc` 是进程内可原子调整的策略参数，本文件自身不读取它；消费者在 `iter.rs` 中以 Acquire 顺序加载。其名字遵循 Go 兼容风格，crate 根允许了 `non_upper_case_globals`。

## 依赖与调用关系

下游依赖：

- `std::sync::Arc`：共享完整对象字节，供 `ByteReader` 和 `ConcurrentFileReader` 持有。
- `std::sync::atomic::AtomicUsize`：保存可运行时调整的默认每路缓冲大小。
- `crate::concurrent_reader::ConcurrentFileReader`：校验并发参数与范围，并保存/暴露续读偏移；其实现能够通过 `std::thread::scope` 并行复制范围，但 `byte_reader.rs` 当前不调用其 `read()`。
- `crate::{Error, Result}`：统一表达 `Closed`、`Eof`、`UnexpectedEof`、`InvalidData` 和存储错误。
- `crate::MemoryStorage`：`from_storage` 的对象来源，按名称返回整个 `Vec<u8>`。

上游调用：

- `kv_reader.rs::KVReader::{new, next_kv, enable_concurrent_read, switch_concurrent_mode, close}` 分别构造、精确读取记录、委托模式控制和关闭。
- `stat_reader.rs::StatsReader::{new, next_prop, close}` 用 `ByteReader` 读取统计属性。
- `iter.rs::MergeKVIter::rebalance_hotspot` 根据多数读取源选择热点，读取全局分片大小并启停 `KVReader` 的并发模式；`MergeKVIter::close` 在关闭前先退出并发模式。
- `iter.rs::get_concurrent_reader_concurrency` 以原子分片大小和内存预算计算并发数，并限制为 256。
- `lib.rs` 把 `ByteReader` 再导出，因此 crate 外使用者无需经过模块路径。

RustCodeGraph 的 `explore` 结果确认了 `read_n_bytes -> stat_reader.rs::next_prop`、`kv_reader.rs::switch_concurrent_mode -> byte_reader.rs::switch_concurrent_mode` 等边；普通检索进一步确认了 `iter.rs` 中原子配置与热点模式切换的直接使用位置。

## 错误处理与边界

- 构造时 `buffer_size == 0` 返回 `Error::InvalidData`；`initial_offset >= len` 返回正常 `Error::Eof`，而不是允许构造一个已耗尽 reader。
- 并发配置拒绝零并发、超过 256 的并发和零分片大小，统一返回 `InvalidData`。
- 读取大小必须在 `1..=1 GiB`；偏移加法溢出也返回 `InvalidData`。
- 从文件尾开始读取返回 `Eof`；尚有数据但不足 `count` 返回 `UnexpectedEof`。这一区分让 `KVReader` 把首个长度头的 EOF 当作正常迭代结束，而把记录内部缺失识别为损坏。
- 截断错误不会返回已经存在的部分字节，并会把逻辑位置推进到末尾。错误文本包含请求长度和剩余长度，便于定位损坏。
- `close` 后任何 `read_n_bytes` 都先返回 `Error::Closed`；观测方法没有关闭检查。
- `from_storage` 不包装底层存储错误；不存在的对象保持 `NotFound`，锁中毒保持 `Poisoned`。
- `openStoreReaderAndSeek` 会把零 `prefetch_size` 修正为 1；直接调用 `ByteReader::new` 则仍拒绝零缓冲。

目前未由独立 Rust 测试直接覆盖的边界包括：大于 1 GiB、`usize` 加法溢出、并发度 0/257、零并发缓冲、缺失存储对象、重复关闭和切换未启用的并发模式。扩展或重构这些分支时应补到独立的 `byte_reader_test.rs`，不要把测试内嵌进生产文件。

## 并发与资源生命周期

`ByteReader` 的所有可变操作都需要 `&mut self`，类型本身不提供跨线程共享的内部锁；调用方必须保证同一 reader 不被并发修改。对象字节通过 `Arc<Vec<u8>>` 只读共享，避免给底层数据增加锁。

启用并发读分为配置、期望、实际三个阶段：`enable_concurrent_read` 只保存参数；`switch_concurrent_mode(true)` 只改变期望；首次后续成功读取才创建 `ConcurrentFileReader`。当前 `read_n_bytes` 仍在调用线程直接复制连续切片，之后重建 reader 以同步偏移，因此这里没有启动并行任务，也没有持有待 join 的线程。真正的 `ConcurrentFileReader::read` 会建立有作用域线程并在返回前全部 join，但当前文件未走该路径。

退出并发模式会立即 drop `ConcurrentFileReader`；`close` 也会清理它并标记关闭。Rust 实现没有 Go 版本的 `membuf.Buffer`、显式 `Destroy`、外部 `objectio.Reader::Close`、重试或 context 取消生命周期。完整对象的内存在所有 `Arc` 所有者释放后自动回收；单次读取返回的新 `Vec<u8>` 由调用方独立持有。

## 与 Go 版本的对应关系

共同语义包括：

- 默认每并发分片 8 MiB、总并发预算 256、单次读取最大 1 GiB。
- `readNBytes` 拒绝非正长度，要求精确长度，并区分文件边界 EOF 与半条记录的 UnexpectedEOF。
- 并发模式请求切入是惰性的，切出要求保持连续偏移；Go `TestSwitchMode` 与 Rust `test_switch_mode` 都验证频繁切换不跳字节、不重复字节。
- 空内容构造返回正常 EOF；跨小缓冲读取内容保持连续。

重要实现差异必须保留在维护认知中：

- Go `byteReader` 持有 `objectio.Reader`、小缓冲与多片大缓冲，通过 `reload`、`next`、`readFromStorageReader` 真正分批读取外部存储；Rust 先把 `MemoryStorage` 对象整体克隆到内存，`small_buffer_size` 不控制 IO 或切片批次。
- Go 并发模式用 `membuf.Buffer` 分配大块内存，并通过 `concurrentFileReader.read` 发起并发范围读；Rust 当前只建立和同步 `ConcurrentFileReader`，没有调用它的并行 `read`。
- Go 顺序读取包含默认重试、context 取消、日志和 Prometheus 计数；Rust 本文件没有这些设施。
- Go 在切回顺序 reader 时根据已加载大缓冲、reload 次数和消费偏移对底层流执行相对 seek；Rust 的完整对象模型只需保存逻辑 `position`。
- Go 返回切片可能在下次调用后改变；Rust 每次返回拥有所有权的新 `Vec<u8>`，内容不会因后续读取而改变。
- Go 可从 `initFileOffset == fileSize` 打开底层 reader 后在读取时观察 EOF；Rust `ByteReader::new` 在该偏移直接返回 `Eof`。

因此，本文件是 Go API 和关键错误/切换语义的内存化移植，不应被描述为真实对象存储 IO、内存池、指标和重试机制的完整等价实现。

## 扩展指南

- 若要实现真正的并发预取，应以 `read_n_bytes` 的读取阶段和 `ConcurrentFileReader` 的交界为入口，让并发 reader 产出的有序 chunks 参与精确长度消费，而不是仅同步 offset。必须保持模式切换前后 `position` 连续，并补充跨分片、末分片、线程失败及切出时未消费数据的测试。
- 若要恢复流式外部存储语义，需要重新设计 `ByteReader` 的数据源抽象、reload 缓冲和 seek/close 生命周期；应对照 Go 的 `next`、`reload`、`readFromStorageReader` 与 `closeConcurrentReader`，不能只替换 `MemoryStorage::read`。
- 修改 EOF 语义时，必须同步检查 `KVReader::next_kv` 和 `StatsReader::next_prop`：记录起点 EOF 是迭代结束，记录内部 EOF 必须是截断。回归测试应放在 `byte_reader_test.rs`，并同步相关 `kv_reader`/`stat_reader` 独立测试。
- 修改并发默认值或上限时，应同步 `iter.rs::{rebalance_hotspot,get_concurrent_reader_concurrency}` 以及 `iter_test.rs::merge_kv_iter_tracks_unread_input_and_bounds_reader_concurrency`，并评估内存预算兼容性。
- 修改返回缓冲策略时，要注意 Rust 当前承诺独立 `Vec<u8>`；若改为借用或复用缓冲，会影响 `KVReader` 的所有权接口，并引入与 Go “下次读取可覆盖”相似的生命周期约束。
- 新增公开 Rust API 时优先使用 snake_case；只有确需保持 Go 移植调用面时才增加转发别名，避免两套入口出现行为漂移。
- 性能风险主要来自当前每次读取复制、完整对象驻留内存，以及若未来接通并发读取后 `concurrency * buffer_size_per_concurrency` 的峰值内存；正确性风险主要是切换时偏移同步、截断分类和长度运算溢出。

## 验证依据

- 生产源码：`pkg/ingestor/simplesst/byte_reader.rs`（全部常量、字段、构造、读取、模式切换和兼容入口）。
- crate 边界：`pkg/ingestor/simplesst/Cargo.toml`（package 名、`lib.rs` crate 根、Go 包映射）；`pkg/ingestor/simplesst/lib.rs`（模块装配、再导出、`Error`、`Result`、`MemoryStorage`）。Cargo 依赖只在 Windows target 下声明；本文件自身只依赖标准库和本 crate 符号。
- 直接下游：`pkg/ingestor/simplesst/concurrent_reader.rs`（`new`、`offset`、`read` 的参数、偏移和线程行为）。
- 直接上游：`pkg/ingestor/simplesst/kv_reader.rs`、`stat_reader.rs`、`iter.rs`（精确读取、EOF 转换、热点模式切换、原子配置读取及关闭链路）。
- Rust 独立测试：`pkg/ingestor/simplesst/byte_reader_test.rs` 验证偏移、顺序读取、截断、空内容、关闭和频繁模式切换；`iter_test.rs::test_read_after_close_conn_reader` 验证退出并发态后文件尾仍为正常 EOF，`merge_kv_iter_tracks_unread_input_and_bounds_reader_concurrency` 验证 256 上限和内存预算换算。
- Go 对照：`pkg/ingestor/simplesst/byte_reader.go` 及 `byte_reader_test.go`；并结合 `kv_reader.go`、`stat_reader.go`、`iter.go` 核对真实外部存储读取、reload、内存池、热点切换和调用入口。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ingestor/simplesst` 找到 37 个 Go/Rust 文件；`explore`/`query` 定位 `ByteReader`、`read_n_bytes`、`switch_concurrent_mode`、`from_storage`、`enable_concurrent_read`、`openStoreReaderAndSeek`、`newByteReader`，并确认 `stat_reader::next_prop` 和 `kv_reader` 委托等调用关系。单独的 `callers/callees` 命令未输出明细，缺失部分用上述已索引源码上下文和精确 `rg` 引用核验。
- 本任务是纯文档分析，按计划不运行 Cargo。最终以固定十一个二级标题的结构命令验证，并人工复核本文没有把 Go 的真实并发外部 IO 误写成 Rust 当前事实。

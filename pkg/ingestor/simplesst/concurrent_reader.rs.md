# `pkg/ingestor/simplesst/concurrent_reader.rs`

源码：[concurrent_reader.rs](./concurrent_reader.rs)

## 文件定位

本文件属于 `astersql-ingestor-simplesst` crate；该 crate 的入口是同目录的 `lib.rs`，其中以 `pub mod concurrent_reader` 暴露本模块。上层 `pkg/ingestor/doc.go` 将整个 ingestor 子系统定位为直接写入底层存储的 SST 导入能力，以及编码 KV 的排序、外部存储中间文件和归并排序等辅助能力。本文件处在 simplesst 读取侧，负责把一段已在内存中的完整对象按固定大小划成多个范围，并用作用域线程并行复制这些范围。

当前实现并不是 Go 版对象存储范围读取器的完整替代：`ConcurrentFileReader` 持有 `Arc<Vec<u8>>`，不持有存储接口、对象名或取消上下文。直接上游 `ByteReader` 会创建和重建它，并在关闭并发模式时读取它的偏移，但当前 `ByteReader::read_n_bytes` 直接切片 `self.data`，没有调用 `ConcurrentFileReader::read`。因此该类型的并发读取算法已实现且由独立测试覆盖，生产读取链上的并发预取消费尚未接通（依据：`byte_reader.rs` 的 `read_n_bytes`、`switch_concurrent_mode`）。

`Cargo.toml` 声明 crate 名为 `astersql-ingestor-simplesst`，库入口为 `lib.rs`，没有定义 feature；列出的内部依赖全部位于 `cfg(windows)` 目标段。本文件自身只依赖标准库的 `Arc`、线程 API，以及 crate 根定义的 `Error` 和 `Result`，没有直接使用 Cargo 中的外部依赖。

## 核心职责

- `ConcurrentFileReader::new` 建立并验证一个逻辑读取窗口：`offset..file_size` 必须位于 `data` 内，并发度与每片大小都必须为正。
- `ConcurrentFileReader::read` 从当前 `offset` 开始，最多生成 `concurrency` 个连续、互不重叠的半开区间，每个区间最大为 `read_buffer_size`，末片按 `file_size` 截短。
- 每批区间分别交给作用域线程复制为独立的 `Vec<u8>`；线程完成后按提交序号排序，使返回值保持文件顺序，而不是线程完成顺序。
- `offset` 在区间提交前进，表示下一批的起点；`offset()` 向上层暴露该续读位置。
- `newConcurrentFileReader` 保留 Go 风格命名的兼容构造入口，全部校验和初始化仍委托给 `ConcurrentFileReader::new`。

本文件不负责解析 SST 记录、管理对象存储连接、分配复用缓冲池或决定何时启用并发模式；这些分别属于更上层的 simplesst 读取逻辑和 `byte_reader.rs`。

## 主要符号

### `pub struct ConcurrentFileReader`

唯一的生产类型，带 `Clone` 和 `Debug`。其五个字段均为私有：

- `data: Arc<Vec<u8>>`：共享完整对象，线程只读；克隆 reader 或为线程克隆 `Arc` 不会复制完整对象。
- `concurrency: usize`：一次 `read` 最多产生的分片数，不代表跨多次调用的全局线程池大小。
- `read_buffer_size: usize`：单片最大字节数。
- `offset: usize`：下一未提交字节的位置。
- `file_size: usize`：逻辑读取上界，可以小于 `data.len()`，但不能大于它。

### `ConcurrentFileReader::new(...) -> Result<Self>`

公开构造函数。它拒绝零并发度、零分片大小、`offset > file_size` 和 `file_size > data.len()`；允许 `offset == file_size`，此时构造成功，首次 `read` 返回 EOF。

### `ConcurrentFileReader::offset(&self) -> usize`

返回下一批应开始的位置。该值按“已提交范围”推进，而不是按某个线程实际完成的字节数推进。当前上游 `ByteReader::switch_concurrent_mode(false)` 会取出 reader 并用此值同步自身 `position`。

### `ConcurrentFileReader::read(&mut self) -> Result<Vec<Vec<u8>>>`

核心批量读取入口。每个返回元素对应一个连续文件范围，元素数量在 `1..=concurrency`；仅最后一批的最后一个元素可能短于 `read_buffer_size`。耗尽后返回 `Error::Eof`，不会返回空的成功批次。

### `newConcurrentFileReader(...) -> Result<ConcurrentFileReader>`

公开的 Go 风格兼容函数，参数与 Rust 构造函数一致，仅转发调用。仓库精确引用搜索未发现它在其他 Rust 文件中被调用；实际 Rust 上游直接调用 `ConcurrentFileReader::new`。

本文件没有 trait、常量、条件编译项或模块级可变状态。

## 执行流程

构造流程如下：

1. 检查 `concurrency` 和 `read_buffer_size` 均非零，否则返回 `Error::InvalidData`。
2. 检查 `offset <= file_size <= data.len()`，否则返回 `Error::InvalidData`。
3. 保存共享数据、并发参数和读取窗口；不创建线程，也不预先复制数据。

一次 `read` 的流程如下：

1. 若 `offset >= file_size`，立即返回 `Error::Eof`。
2. 预留最多 `concurrency` 个范围的位置，逐个计算 `end = min(offset.saturating_add(read_buffer_size), file_size)`。
3. 记录 `[offset, end)` 后立即令 `offset = end`；循环到达并发上限或逻辑文件末尾即停止。因此同批与相邻批之间都不会重叠或留空洞。
4. 克隆共享数据的 `Arc`，在 `std::thread::scope` 中为每个范围创建一个线程。线程复制 `data[start..end]`，并把提交序号与结果一起返回。
5. 主线程逐个 `join`。任一线程 panic 被转换成 `Error::InvalidData("concurrent range reader panicked")`，整批返回错误。
6. 按提交序号排序并去掉序号，返回保持文件顺序的分片列表。

例如 `offset=3`、`file_size=19`、`concurrency=3`、`read_buffer_size=4` 时，第一批为 `[3,7)`、`[7,11)`、`[11,15)`，第二批只有 `[15,19)`，第三次调用返回 EOF；这也是 `concurrent_reader_test.rs` 的确定性基准场景。

## 数据与状态

核心状态不变量是 `offset <= file_size <= data.len()`，由构造函数建立；随后 `read` 只把 `offset` 单调推进至 `file_size`。`read_buffer_size > 0` 保证每轮区间非空并使循环取得进展，`concurrency > 0` 保证未到 EOF 时至少提交一个区间。

`file_size` 是逻辑边界而非底层向量长度：调用方可以只暴露 `data` 的前缀。每个成功分片拥有自己的 `Vec<u8>`，生命周期不依赖 reader，但每批最多额外复制约 `concurrency * read_buffer_size` 字节，末批除外。

`Clone` 会复制参数与当前 `offset`，并共享同一个 `Arc<Vec<u8>>`；两个 clone 之后各自推进偏移，彼此没有协调或去重语义。`offset` 是普通 `usize`，所以同一个实例必须经由 `&mut self` 串行调用 `read`；类型内部没有为多调用者共享进度提供锁或原子变量。

需特别注意错误时的状态：范围是在启动线程前就全部写入 `ranges` 并推进 `self.offset`。若线程 panic 导致 `read` 返回错误，偏移不会回滚；重试将从下一批开始，可能跳过失败批次。正常切片访问在构造不变量下不会越界，因此 panic 路径主要用于防御线程体或未来扩展引入的 panic。

## 依赖与调用关系

直接上游关系：

- `lib.rs` 用 `pub mod concurrent_reader` 装配本模块，但没有在 crate 根重导出该类型。
- `byte_reader.rs` 导入 `ConcurrentFileReader`，字段 `ByteReader::concurrent_reader` 保存可选实例。
- `ByteReader::read_n_bytes` 在期望切入并发模式时以当前位置构造 reader，并在每次逻辑读取后以新位置重建它。
- `ByteReader::switch_concurrent_mode(false)` 取出实例并读取 `offset()`，随后退出当前并发态。
- `concurrent_reader_test.rs` 直接调用 `new`、`read` 与 `offset` 验证分片契约。

当前没有找到生产代码对 `ConcurrentFileReader::read` 或兼容函数 `newConcurrentFileReader` 的调用。因而应用主链的真实状态是：simplesst 上层仍通过 `ByteReader` 消费内存对象，本文件的 `read` 仅在独立 Rust 测试中直接执行。不能据此宣称 Rust 已像 Go 一样向对象存储并行发起范围请求。

直接下游只有标准库与 crate 根错误类型：`Arc::clone` 分享对象，`std::thread::scope/spawn/join` 管理线程，切片的 `to_vec` 复制分片，`Error::{InvalidData,Eof}` 表达失败。这里没有存储 API、异步运行时、通道、锁或 Cargo 外部 crate 调用。

RustCodeGraph 已索引目标文件和 `ConcurrentFileReader`/`newConcurrentFileReader`，但本次对这些 Rust `impl` 方法执行 `callers`/`callees` 未返回调用边；以上直接上游由其文件级使用信息与精确引用搜索补齐。

## 错误处理与边界

- 配置错误：`concurrency == 0` 或 `read_buffer_size == 0` 返回 `Error::InvalidData`，防止无进展或空批次。
- 范围错误：`offset > file_size` 或 `file_size > data.len()` 返回 `Error::InvalidData`。`offset == file_size` 是合法的已耗尽 reader。
- 正常耗尽：`read` 在入口检测 `offset >= file_size` 并返回 `Error::Eof`。测试通过 `is_eof()` 区分正常结束。
- 末片边界：以 `min(..., file_size)` 截断，不会读到逻辑文件上界之外；`saturating_add` 避免 `usize` 加法溢出。
- 线程失败：线程 panic 经 `join` 映射为 `Error::InvalidData`，不会把 panic 直接传播到调用线程；但本批偏移不会回滚。

本实现没有可返回的底层 I/O 错误，因为所有数据在构造前已进入内存。它也没有取消检查、超时、重试和错误上下文中的文件名/范围；这些是与 Go 对象存储实现的边界差异，而非当前 Rust API 已支持的能力。

## 并发与资源生命周期

线程生命周期被 `std::thread::scope` 约束在一次 `read` 内：方法返回前所有分片线程都已完成并被 `join`，不会留下后台任务。每个线程只读共享的 `Arc<Vec<u8>>`，读取范围预先在主线程划分且互不重叠，因此无需锁。结果排序让可见顺序不受调度影响。

每次 `read` 都创建最多 `concurrency` 个操作系统作用域线程，没有线程池或跨批复用机制；并发度过大时会增加线程创建、调度和内存复制成本。本构造函数只要求并发度大于零，自身没有上限；直接生产上游 `ByteReader::enable_concurrent_read` 另行限制为不超过 `CONCURRENT_READER_TOTAL_CONCURRENCY`（256），但直接调用 `ConcurrentFileReader::new` 可以绕过该上限。

数据由 `Arc` 持有，所有 reader clone、局部 `data` clone 和工作线程释放后才会回收完整对象。分片结果是独立所有权的向量，不借用源数据。类型没有显式 `close` 或 `Drop`：丢弃 reader 只减少 `Arc` 引用计数。当前 `ByteReader::close` 会丢弃其可选 reader。

## 与 Go 版本的对应关系

结构和算法对应关系如下：

- Rust `ConcurrentFileReader` 对应 Go `concurrentFileReader`；`concurrency`、分片大小、`offset`、`file_size` 的概念一致。
- 两版 `read` 都在一批内最多调度 `concurrency` 个范围，先推进 offset，末片按文件大小截短，等待整批完成，并在耗尽后报告 EOF。
- Go 通过 `errgroup.Group` 等待 goroutine；Rust 使用 `std::thread::scope` 和 `JoinHandle::join`，再显式按提交序号排序。
- Go 构造函数基本只保存字段；Rust 构造函数额外提前验证零参数和读取窗口，因而错误会更早暴露。

关键差异是数据源与缓冲所有权：Go reader 持有 `context.Context`、`storeapi.Storage` 和对象名，由调用方提供可复用 `bufs`，每个 goroutine 调用 `objstore.ReadDataInRange` 原地填充缓冲，底层 I/O 错误附带 offset/readSize。Rust reader 持有完整的 `Arc<Vec<u8>>`，每次线程从内存切片新建 `Vec<u8>`；没有上下文取消、外部存储 I/O、缓冲池复用或等价错误注释。

`concurrent_reader_test.go::TestConcurrentRead` 随机选择起点、1 到 4 的并发度和 1 到 100 的分片大小，最终拼回 `data[offset:]`。Rust `concurrent_reader_test.rs::concurrent_reader_reassembles_every_go_parameter_shape` 用确定性参数矩阵覆盖相同契约，并额外检查最终 offset；`canonical_concurrent_reader_returns_ordered_non_overlapping_ranges` 明确覆盖批次顺序、末片截短与 EOF。

因此移植语义在“范围划分、顺序重组、末尾处理”上对齐；在“真实外部存储并发 I/O、缓冲复用、取消与 I/O 错误传播”上尚未对齐。

## 扩展指南

若只调整范围划分或批次语义，首要修改点是 `ConcurrentFileReader::read`，并同步独立文件 `concurrent_reader_test.rs`；至少覆盖非零起点、逻辑 `file_size < data.len()`、末片、跨多批、EOF 和返回顺序。不要把测试嵌入生产源文件。

若新增构造约束，应修改 `ConcurrentFileReader::new`，同时考虑 `newConcurrentFileReader` 的兼容行为和 `ByteReader::enable_concurrent_read` 已有的 256 并发上限。建议新增无效并发度、无效缓冲大小、`offset > file_size`、`file_size > data.len()` 以及 `offset == file_size` 的构造测试。

若要真正接入生产并发预取，不能只在本文件中调用更多线程：还需明确 `ByteReader::read_n_bytes` 如何消费分片、跨记录保存剩余数据、在顺序/并发模式间同步精确偏移，并决定是否继续使用内存数据源。若目标是对齐 Go，则需要在 crate 边界引入存储抽象、对象标识、取消机制和可复用缓冲，并验证 Cargo 的目标条件依赖；这属于跨文件设计，不能把 Go 代码的能力假定为当前 Rust 已存在。

性能变更应关注每批线程创建成本、`to_vec` 的总复制量、并发度上限和排序开销。错误语义变更应特别决定失败批次是否回滚 offset；若选择可重试语义，必须避免重复/跳过数据并添加 panic 或可注入读取失败的回归测试。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ingestor/simplesst` 列出目标源、Go 对照和两版测试。
- RustCodeGraph 源码/符号：`explore "pkg/ingestor/simplesst/concurrent_reader.rs ConcurrentFileReader ConcurrentFileReader read_range"`、`query ConcurrentFileReader`、`query newConcurrentFileReader`、`node --file pkg/ingestor/simplesst/concurrent_reader.rs --offset 1 --limit 220`，确认目标文件全貌、类型和兼容构造函数。
- RustCodeGraph 调用调查：对 `ConcurrentFileReader::new/read/offset`、`newConcurrentFileReader` 及精确节点 ID 执行 `callers`/`callees`；Rust `impl` 方法未返回边。随后以 `rg` 精确核对 `pkg/ingestor/simplesst/*.rs` 的引用，确认上游只在 `byte_reader.rs` 构造/重建 reader 和读取 offset，`read` 仅见于独立测试。
- 包与 crate 边界：读取 `pkg/ingestor/doc.go`、`pkg/ingestor/simplesst/Cargo.toml`、`pkg/ingestor/simplesst/lib.rs`。
- 上游实现：RustCodeGraph 读取 `pkg/ingestor/simplesst/byte_reader.rs` 的 `enable_concurrent_read`、`switch_concurrent_mode` 和 `read_n_bytes`，确认当前消费路径直接复制 `self.data`，并未调用本文件的 `read`。
- Rust 测试：读取 `pkg/ingestor/simplesst/concurrent_reader_test.rs`，确认有序非重叠范围、末片、EOF、参数矩阵重组和最终 offset 契约。
- Go 对照：读取 `pkg/ingestor/simplesst/concurrent_reader.go` 与 `concurrent_reader_test.go`，核对 `errgroup`、`objstore.ReadDataInRange`、调用方缓冲和随机参数测试。
- 本任务是纯文档分析，按计划不运行 Cargo；完成前使用任务指定命令验证本文档存在且恰有 11 个固定二级章节，并人工复查未把未接线能力写成现状。

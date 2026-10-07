# `pkg/ingestor/simplesst/stat_reader.rs`

## 文件定位

本文件实现 `astersql-ingestor-simplesst` crate 的统计文件顺序读取器。crate 由 `pkg/ingestor/simplesst/Cargo.toml` 定义、以 `lib.rs` 为入口，并在 `lib.rs::stat_reader` 中公开本模块；其移植元数据指向 Go 包 `pkg/ingestor/simplesst`。统计文件不是 SST 数据正文，而是 writer 为连续键范围写出的 `RangeProperty` 序列，供导入阶段归并属性、估算数据文件的安全 seek 偏移。

当前 Rust 实现依赖 crate 内的内存对象存储 `MemoryStorage`，打开时将整个统计对象读入 `Vec<u8>`，再由 `ByteReader` 维护游标。因此它是 simplesst 内部数据格式与上层算法之间的薄解析边界，不负责创建统计、排序属性或执行真正的对象存储流式 I/O。直接上游见 `iter.rs::MergePropIter` 和 `util.rs::read_offsets_for_path`。

## 核心职责

1. `StatsReader::new` 把已加载的字节数组包装成从偏移 0 开始的 `ByteReader`，并把零缓冲参数规范化为 1。
2. `StatsReader::from_storage` 通过 `MemoryStorage::read` 按名称取得完整对象，然后复用 `new`。
3. `StatsReader::next_prop` 按 `<4 字节大端 u32 正文长度><属性正文>` 消费一条记录，并把正文交给 `codec.rs::decode_prop` 还原为 `RangeProperty`。
4. `StatsReader::close` 将关闭动作下传给 `ByteReader`；`NextProp`、`Close`、`NewStatsReader` 是为 Go 命名习惯保留的公开兼容入口。

该文件只保证“按记录边界逐条解码”。属性是否按 `FirstKey` 排序、`Offset` 是否单调、属性之间是否覆盖完整数据，均由 writer 和调用方维持，而不是 reader 在此校验。

## 主要符号

- `pub struct StatsReader { byte_reader: ByteReader }`：唯一状态是底层字节读取器；字段私有，调用方不能绕过记录协议移动游标。
- `pub fn new(data: Vec<u8>, buffer_size: usize) -> Result<Self>`：拥有输入缓冲；调用 `ByteReader::new(data, 0, buffer_size.max(1))`。空数据在底层因初始偏移已到末尾而返回 `Error::Eof`。
- `pub fn from_storage(store: &MemoryStorage, name: &str, buffer_size: usize) -> Result<Self>`：先执行 `store.read(name)`，再进入 `new`；缺失对象等存储错误原样传播。
- `pub fn next_prop(&mut self) -> Result<RangeProperty>`：核心解析入口。先精确读取 4 字节长度头并按大端转为 `usize`，再精确读取指定长度的正文，最后调用 `decode_prop`。
- `pub fn close(&mut self) -> Result<()>`：释放底层并发 reader（若存在）并标记关闭。当前 `ByteReader::close` 可重复调用。
- `pub fn NextProp`、`pub fn Close`：分别直接转发到 snake_case 方法，不增加新语义。
- `pub fn NewStatsReader(...) -> Result<StatsReader>`：直接转发到 `StatsReader::from_storage`；Rust 返回拥有型值，而 Go 返回指针。

本文件没有模块级常量、trait、条件编译项或内嵌测试。线格式常量和 `RangeProperty` 定义位于 `codec.rs`，测试按仓库约定放在独立 `*_test.rs` 文件中。

## 执行流程

构造流程如下：调用方传入字节数组或 `(MemoryStorage, name)`；`from_storage` 先读出对象，`new` 再创建偏移为 0 的 `ByteReader`。`buffer_size.max(1)` 确保本层不会把非法的零缓冲传给底层，但并不改变统计文件格式。

一次 `next_prop` 的步骤为：

1. 调用 `ByteReader::read_n_bytes(4)` 精确读取记录长度头。
2. 用 `u32::from_be_bytes` 按大端解释长度；四字节切片由前一步保证，因此 `try_into().unwrap()` 的长度前提在本函数内成立。
3. 再调用 `read_n_bytes(length)` 获取完整正文。若正文开始处已经 EOF，本层将其明确改写为 `Error::UnexpectedEof("truncated range property")`；若只剩部分正文，底层已经返回带请求量/剩余量上下文的 `UnexpectedEof`，本层原样保留；其他错误也直接传播。
4. `codec.rs::decode_prop` 依次解析 `FirstKey`、`LastKey`、`Size`、`Keys`、`Offset`，返回拥有型 `RangeProperty`。正文内部字段越界时返回 `Error::InvalidData`。

正常文件结束由下一次读取长度头时的 `Error::Eof` 表示。调用方据此结束循环：`util.rs::read_offsets_for_path` 在 EOF 时补齐剩余 job key 的偏移；`iter.rs::MergePropIter::{fill_window,next}` 在 EOF 时关闭已耗尽 reader、降低活动数并补开同组后续文件。

## 数据与状态

`StatsReader` 不缓存当前属性，也没有独立的记录计数；读取进度完全保存在 `ByteReader::position` 中。每次成功读取会永久推进游标 4 字节加正文长度，返回的 `RangeProperty` 拥有两个 key 的 `Vec<u8>`，不借用 reader 缓冲。

统计正文格式由 `codec.rs` 固定：两个大端 `u32` key 长度及 key 字节，随后是大端 `u64` 的 `Size`、`Keys`、`Offset`。外层 `u32` 记录长度由 writer 的 `encode_multi_props` 写入。`decode_prop` 只消费已定义字段，不拒绝正文尾随字节；这一兼容行为由 `codec_test.rs::decode_prop_ignores_trailing_bytes_like_go` 固定。

`buffer_size` 被保存到 `ByteReader::small_buffer_size`，但当前内存后端的 `read_n_bytes` 直接复制逻辑区间；`StatsReader` 本身不会启用并发预取模式。`from_storage` 的整对象读取意味着其峰值内存至少包含统计对象和每次返回字段的复制，不能把它描述成流式、零拷贝 reader。

## 依赖与调用关系

直接下游调用边（由 RustCodeGraph 与源码节点核对）：

- `StatsReader::new -> ByteReader::new`；
- `StatsReader::from_storage -> MemoryStorage::read -> StatsReader::new`；
- `StatsReader::next_prop -> ByteReader::read_n_bytes`（长度头和正文各一次）；
- `StatsReader::next_prop -> codec::decode_prop`；
- `StatsReader::close -> ByteReader::close`；
- `NextProp -> next_prop`、`Close -> close`、`NewStatsReader -> from_storage`。

直接上游有两类：

- `iter.rs::MergePropIter` 在构造活动窗口、后台预开剩余统计文件时调用 `StatsReader::from_storage`，并在 `fill_window`/`next` 中调用 `next_prop`。每个文件的首属性进入按 `FirstKey` 排序的堆，耗尽后关闭 reader 并补充同组窗口，因此本文件位于多统计文件全局归并链上。
- `util.rs::read_offsets_for_path` 为每个统计路径创建 reader，按属性 `FirstKey` 扫描升序 job key，把 `RangeProperty::Offset` 写入二维偏移表；`get_read_range_from_props_with_limit` 以分批 scoped threads 并行执行这些单文件扫描。

此外，`onefile_writer_test.rs` 和 `writer_test.rs` 直接用 reader 回读 writer 产物，形成“写统计文件—读属性”的端到端单元测试。crate 的 `Cargo.toml` 没有为本文件声明独有外部依赖；相关类型都来自同一 crate，且现有依赖清单仅在 `cfg(windows)` 下列出。

## 错误处理与边界

- 空对象或恰好在记录边界耗尽：读取新长度头得到 `Error::Eof`，上层把它视为正常结束。`iter_test.rs::test_empty_base_reader_for_limit_size_merge_iter` 验证空统计文件不会留下迭代错误。
- 长度头只有 1–3 字节：`ByteReader::read_n_bytes(4)` 返回 `UnexpectedEof`；本层直接传播。
- 有完整长度头但正文不足：如果正文一个字节也没有，`next_prop` 把底层 `Eof` 改写为带 `truncated range property` 上下文的 `UnexpectedEof`；如果已经读到部分正文，底层直接返回带请求量/剩余量上下文的 `UnexpectedEof`。两种情况都不会被误当成记录边界的正常结束。
- 正文长度足够，但 key 长度或固定数值字段越界：`decode_prop` 返回 `InvalidData("truncated range property")`。正文尾随字节按 Go 语义允许。
- 长度头可声明至 `u32::MAX`，但 `ByteReader` 将超过 1 GiB 的单次读取拒绝为 `InvalidData`；这是当前 Rust 底层的资源保护边界。
- 缺失存储对象、锁中毒等错误从 `MemoryStorage::read` 原样传播。`MergePropIter` 对 EOF 与非 EOF 分开处理：EOF 补开下一 reader，其他错误记录到 iterator 并停止继续推进。
- `next_prop` 不验证 `FirstKey <= LastKey`、偏移单调或 job key 已排序；调用者若依赖这些不变量，必须由 writer 或入口校验保证。
- reader 关闭后再次读取会由 `ByteReader::ensure_open` 返回 `Error::Closed`；重复 `close` 当前成功，但调用方不应依赖未文档化的底层实现细节来复用已关闭 reader。

## 并发与资源生命周期

单个 `StatsReader` 的 API 需要 `&mut self` 才能读取或关闭，因而同一实例按顺序推进；本文件没有锁、异步任务或内部线程。它不自行实现 `Drop`，所以需要上层显式调用 `close` 才表达资源结束。当前内存后端只持有 `Arc<Vec<u8>>` 且 `close` 清理可选并发 reader，但未来替换为真实外部存储句柄时，显式关闭路径仍是重要契约。

并发发生在调用方而非本文件：`get_read_range_from_props_with_limit` 按 `concurrency.max(1)` 分批启动 scoped threads，每个线程独占一个 `StatsReader`；`MergePropIter` 用后台线程预开 reader，并通过有界 channel 把结果交给主迭代器。`PropertyGroup::stop` 会先设置 shutdown、排空并关闭尚未领取的 reader，再 join worker，防止有界队列导致关闭死锁。相关行为由 `iter_test.rs::{merge_prop_close_with_async_open_error, merge_prop_close_with_async_open_success, merge_prop_close_drains_full_preopen_queue}` 验证。

扩展时不能让一个 reader 被多个线程交错调用 `next_prop`，也不能在错误分支遗漏关闭。若为 `StatsReader` 增加预取或后台任务，必须同时定义取消、join、未消费 reader 和析构路径，并同步独立测试。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/simplesst/stat_reader.go`：两版都有 `StatsReader`、`NewStatsReader`、`NextProp`、`Close`，并使用相同的 4 字节大端长度前缀和 `decodeProp` 字段顺序。Go 的正文读取错误经过 `noEOF(err)`，Rust 在正文阶段显式把 EOF 映射成 `UnexpectedEof`，目的都是区分“记录边界的正常 EOF”和“半条记录的损坏 EOF”。

当前实现仍有明确边界差异：Go 构造器接收 `context.Context` 和抽象 `storeapi.Storage`，通过 `openStoreReaderAndSeek(..., 0, 250*1024)` 获得存储 reader，再用调用者的 `bufSize` 包装；Rust 构造器没有 context，只接受具体 `MemoryStorage`，并将对象整体加载到内存。Go 返回 `*StatsReader` 和 `*RangeProperty`，Rust 返回拥有型 `StatsReader` 和 `RangeProperty`。Rust 还提供 snake_case 原生 API，并保留 PascalCase 别名以方便移植调用点。

这些差异意味着当前 Rust 版复刻了统计格式、顺序读取和错误分类，但尚不能据此声称已具备 Go 版的取消传播、通用对象存储或流式 I/O 能力。Go 侧 `iter.go`、`util.go`、`onefile_writer_test.go`、`writer_test.go` 是对应调用和测试意图的直接证据。

## 扩展指南

- 修改线格式时，优先改 `codec.rs::{encode_prop,decode_prop,encode_multi_props}`，再评估 `next_prop` 的外层 framing；必须维持 Go `encodeProp`/`decodeProp` 兼容，并同步 `codec_test.rs`、writer 回读测试及 Go 对照。
- 若支持真实对象存储或取消，应从 `from_storage`/`NewStatsReader` 注入抽象 reader 与 context/cancellation，而不是把网络逻辑塞入 `next_prop`。同时验证打开失败、读取中取消、close 后行为及 `MergePropIter` 后台预开清理。
- 若新增长度上限或格式校验，应明确它作用于外层正文长度还是内部 key 长度，并为“空文件、截断头、截断正文、超大声明长度、尾随字节”分别添加回归用例。测试应放在独立 `stat_reader_test.rs`（并由 `lib.rs` 的 `#[cfg(test)] mod stat_reader_test;` 接入），不要内嵌到生产文件；若只扩展现有跨模块行为，也可在相应 `codec_test.rs`、`iter_test.rs` 或 writer 测试中补充。
- 若改变 EOF 分类，必须同步检查 `util.rs::read_offsets_for_path` 和 `iter.rs::MergePropIter`，因为二者都以 `error.is_eof()` 决定“正常耗尽”还是“停止并报错”。
- 若改变缓冲或并发策略，需测量整对象复制和属性 key 复制的内存成本，并保留 `get_read_range_from_props_with_limit` 的并发上限以及 `PropertyGroup::stop` 的无泄漏/无死锁性质。

兼容风险主要是 Go/Rust 线格式或 EOF 分类漂移；正确性风险是损坏记录被误判为正常耗尽、错误偏移被上层用于 seek；性能风险是整体加载大统计对象、恶意长度触发大读取请求，以及并发打开过多文件。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/ingestor/simplesst/stat_reader.rs` 命中目标文件并报告 12 个符号。
- RustCodeGraph 源码/调用查询：`node --file pkg/ingestor/simplesst/stat_reader.rs`；`query StatsReader`、`query NewStatsReader`；文件限定的 `callers/callees` 查询。可稳定确认的关键边为 `next_prop -> read_n_bytes`、`next_prop -> decode_prop`、`NextProp -> next_prop`；同名 `new` 的图查询存在歧义，因此以文件节点和直接引用搜索补证。
- 读过的生产与边界文件：`stat_reader.rs`、`byte_reader.rs`、`codec.rs`、`iter.rs`、`util.rs`、`lib.rs`、`Cargo.toml`。
- Go 对照：`stat_reader.go`；并以 `iter.go`、`util.go` 的直接引用和 `onefile_writer_test.go`、`writer_test.go` 的回读用法核对移植意图。
- Rust 独立测试证据：`onefile_writer_test.rs::test_onefile_writer_stat` 验证属性数和 Keys 汇总；`test_onefile_prop_offset` 验证偏移单调；`writer_test.rs::test_flush_kvs_retry` 验证重试后的统计仍可读且 FirstKey 有序；`util_test.rs` 验证偏移矩阵、空 job key、并发上限和提前停止；`iter_test.rs` 验证空文件、归并顺序、预开错误传播与关闭清理；`codec_test.rs` 验证字段往返、截断和尾随字节兼容。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行固定 11 章节结构检查，并人工复核文档只陈述上述源码、调用边、Cargo/Go 和独立测试能够支持的事实。

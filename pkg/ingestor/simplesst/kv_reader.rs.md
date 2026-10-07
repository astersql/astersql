# `pkg/ingestor/simplesst/kv_reader.rs`

## 文件定位

本文件属于 `astersql-ingestor-simplesst` crate；crate 入口 `pkg/ingestor/simplesst/lib.rs` 通过 `pub mod kv_reader` 暴露该模块。它位于 ingest 的简化 SST 数据读取链上，把 writer 产生的连续字节流解析为一条条键值记录，主要供 `pkg/ingestor/simplesst/iter.rs` 的 `MergeKVIter` 多路归并使用，也被 writer/file 的独立测试用于回读产物。

文件线格式与 Go 同路径实现一致：每条记录是 `<key-len><value-len><key><value>`，两个长度均为 8 字节大端 `u64`。这里的 “SST” 是 simplesst 自有的顺序 KV 文件格式；本文件不负责 RocksDB SST 构建、键排序、去重或统计属性解析。

`pkg/ingestor/doc.go` 将该子系统定位为 ingest 所需的编码 KV 排序与外部存储归并工具。当前 Rust crate 的 `Cargo.toml` 记录 Go 包来源为 `pkg/ingestor/simplesst`；其路径依赖只在 `cfg(windows)` 下声明，而本文件实际只直接依赖 crate 内的 `ByteReader`、`MemoryStorage`、`Error` 和 `Result`。

## 核心职责

1. `KVReader::new`/`from_storage`/`NewKVReader` 建立从指定偏移开始的记录读取器，并把用户缓冲大小按三分之一（最小 1 字节）传给底层 `ByteReader`。
2. `KVReader::next_kv` 精确读取两个长度头和记录正文，检查长度加法与平台 `usize` 转换，返回拥有所有权的 `(Vec<u8>, Vec<u8>)`。
3. `no_eof` 区分“在下一条记录开始前正常耗尽”和“记录已开始后被截断”：只有后者被转换为 `UnexpectedEof`。
4. `enable_concurrent_read`、`switch_concurrent_mode`、`concurrent_mode` 和 `close` 将并发预取状态及资源生命周期委托给 `ByteReader`，供归并迭代器的热点调度使用。
5. `NextKV`、`EnableConcurrentRead`、`SwitchConcurrentMode`、`Close` 与自由函数 `NewKVReader` 保留 Go 风格入口；snake_case 方法是 Rust 调用方当前主要使用的接口。

## 主要符号

- `DefaultReadBufferSize: usize = 64 * 1024`：默认 64 KiB 读缓冲。`MergeKVIter` 导入该常量；构造时实际交给 `ByteReader` 的小缓冲段为 `max(buffer_size / 3, 1)`。
- `KVReader { byte_reader: ByteReader }`：唯一状态是底层字节读取器；字段私有，格式解析与字节读取状态不能被调用方绕开。
- `KVReader::new(data, initial_offset, buffer_size) -> Result<Self>`：把 `u64` 偏移安全转换为 `usize`，过大时报 `InvalidData("initial offset is too large")`；随后调用 `ByteReader::new` 检查偏移和缓冲参数。
- `KVReader::from_storage(store, name, initial_offset, buffer_size)`：通过 `MemoryStorage::read` 克隆完整对象，再转交 `new`。对象不存在、锁中毒等错误保持原类型传播。
- `KVReader::next_kv() -> Result<(Vec<u8>, Vec<u8>)>`：核心解析入口。它读取 key 长度、value 长度、合计长度的正文，再在 `key_len` 处分割并各自复制为 `Vec<u8>`。
- `KVReader::enable_concurrent_read(concurrency, buffer_size)`：校验和保存并发参数的委托入口；实际限制由 `ByteReader` 执行（并发度为 `1..=256`，每路缓冲非零）。
- `KVReader::switch_concurrent_mode(enabled)`：设置期望模式；进入并发态延迟到后续读取，退出时由底层同步偏移并释放并发 reader。
- `KVReader::concurrent_mode() -> (bool, bool)`：返回底层的“期望并发、当前实际并发”二元状态，供 `MergeKVIter::reader_concurrent_mode` 和测试观测。
- `KVReader::close()`：关闭底层 reader。关闭后再读取由 `ByteReader::ensure_open` 返回 `Error::Closed`；重复关闭当前会成功。
- `no_eof<T>(Result<T>) -> Result<T>`：仅把 `Error::Eof` 转成带 `truncated key/value record` 上下文的 `Error::UnexpectedEof`，其他错误不变。
- Go 风格别名方法和 `NewKVReader`：都是薄转发，不引入第二套解析或状态逻辑。

## 执行流程

典型主链为 `MergeKVIter::new_with_options` → `KVReader::from_storage` → `KVReader::next_kv`。归并迭代器为每个路径创建一个 reader，从每路预读首条记录后放入最小键堆；`MergeKVIter::next` 每弹出一条，就再次调用同一路的 `next_kv` 补充堆。读到正常 EOF 时该路被关闭并从 reader 槽位移除，非 EOF 错误则保存到迭代器错误状态。

单次 `next_kv` 的步骤如下：

1. 调用 `ByteReader::read_n_bytes(8)` 读取 key 长度。这里不包 `no_eof`，因此输入恰好耗尽时保留 `Error::Eof`，表示迭代正常结束。
2. 再读取 8 字节 value 长度，并通过 `no_eof` 把此处的正常 EOF 改为记录截断错误。
3. 用 `u64::checked_add` 计算正文总长，防止 key/value 长度相加溢出；再分别把总长和 key 长度转换为 `usize`。
4. 精确读取 `key_len + value_len` 字节，正文不足同样由 `no_eof` 标为 `UnexpectedEof`。
5. 在 `key_len` 处切分临时字节向量，并复制成两个独立 `Vec<u8>` 返回。零长度 key 或 value 在总正文长度非零时可正常表示；若二者都为零，底层 `read_n_bytes(0)` 会返回 `InvalidData`，因此当前 Rust 实现不接受“双空”记录。

并发路径由 `MergeKVIter::rebalance_hotspot` 驱动：某一路在检测周期内严格占多数时，迭代器先调用 `enable_concurrent_read` 配置并发度和每路缓冲，再调用 `switch_concurrent_mode(true)`；热点迁移、读尽或整体关闭时调用 `switch_concurrent_mode(false)`。模式切换不改变本文件的记录解析顺序。

## 数据与状态

`KVReader` 不缓存当前 key/value，也不维护排序状态；逻辑偏移、关闭标志以及并发 reader 状态全部封装在 `ByteReader` 中。`ByteReader::read_n_bytes` 每次返回新 `Vec<u8>` 并推进 `position`，因此本文件返回的 key/value 是拥有所有权的副本，后续读取不会覆写它们。

长度头按大端 `u64` 解码。内存申请前有三层边界：长度和必须通过 `checked_add`，总长度和 key 长度必须能转成当前平台的 `usize`，底层单次读取还受 `MAX_READ_SIZE = 1 GiB` 限制。`value_len` 无需单独转成 `usize`，因为总长已验证且切片后半段自然形成 value。

`from_storage` 当前会通过 `MemoryStorage::read` 取得完整对象副本，之后 `ByteReader` 再用 `Arc<Vec<u8>>` 持有数据。这意味着 reader 创建后不观察同名对象的后续覆盖，也意味着大对象读取会产生整对象内存占用；它不是 Go `storeapi.Storage` 的流式远程读取实现。

## 依赖与调用关系

上游直接调用证据：

- `pkg/ingestor/simplesst/iter.rs` 导入 `DefaultReadBufferSize` 和 `KVReader`；`MergeKVIter::new_with_options` 为每个输入路径调用 `from_storage` 并预读首条，`MergeKVIter::next` 持续调用 `next_kv`，热点重平衡调用并发配置/切换，`close` 关闭所有存活 reader。
- `pkg/ingestor/simplesst/file_test.rs` 直接用 `KVReader::new` 验证编码文件的逐条解析、EOF、截断和不同小缓冲尺寸。
- `pkg/ingestor/simplesst/onefile_writer_test.rs` 与 `writer_test.rs` 通过 `from_storage` 回读 writer 数据文件和冲突文件，证明此 reader 是写入产物验证链的一部分。

下游调用证据：

- `ByteReader::new`/`read_n_bytes`/`enable_concurrent_read`/`switch_concurrent_mode`/`concurrent_mode`/`close` 承担精确读取、逻辑偏移、并发配置和关闭状态。
- `MemoryStorage::read` 提供按路径的整对象读取；`Error`/`Result` 统一表达 EOF、截断、非法数据、对象不存在和锁错误。
- 标准库 `u64::from_be_bytes`、`checked_add`、`usize::try_from` 完成线格式解码与溢出保护。

RustCodeGraph 的文件查询显示该文件被 `iter.rs`、`file_test.rs`、`iter_test.rs`、`onefile_writer_test.rs` 等 8 个文件使用；精确 callers/callees 查询未产出边，因此上述具体边以源码引用搜索和调用点读取为准。

## 错误处理与边界

- 在第一段 8 字节 key 长度头开始前到达文件尾：返回 `Error::Eof`，调用方据此判定正常结束或空文件。
- key 长度头只读到部分字节：底层直接返回 `UnexpectedEof`；`no_eof` 虽未包住第一步，也不影响部分头被识别为截断。
- value 长度头或正文处遇 `Error::Eof`：`no_eof` 转为 `UnexpectedEof("truncated key/value record")`；底层已经产生的 `UnexpectedEof` 原样保留。
- 伪造长度导致 `key_len + value_len` 溢出：返回 `InvalidData("key/value length overflow")`。
- 长度无法落入当前平台地址空间：返回 `InvalidData`；单次正文超过 1 GiB 时由 `ByteReader` 拒绝。
- 初始偏移无法转为 `usize`、偏移位于对象末尾或越界、对象不存在、reader 已关闭、并发配置无效，均由构造或底层委托方法返回明确错误。
- `next_kv` 中两个 `try_into().unwrap()` 的输入长度由成功的 `read_n_bytes(8)` 保证，因此不会因正常外部输入长度不足而 panic；该不变量依赖 `ByteReader` 的“成功即精确返回请求字节数”契约。
- Rust 返回拥有所有权的 key/value，与 Go 注释所述“后续读取会复用返回切片”不同，调用方无需额外复制，但会承担两次分配/复制成本。

## 并发与资源生命周期

`KVReader` 自身没有锁、线程或异步任务，并要求 `&mut self` 执行读取和模式切换，因而不会在同一实例上并发推进记录。并发预取能力完全属于 `ByteReader`/`ConcurrentFileReader`：`enable_concurrent_read` 只保存配置，`switch_concurrent_mode(true)` 只设置期望状态，真正进入并发态发生在下一次成功读取时。

退出并发模式时，底层取出并发 reader、同步其 offset 并把实际模式置为 false，保证后续顺序读从同一逻辑位置继续。到达截断边界时底层清理并发 reader，并重置期望/实际状态。`close` 释放并发 reader 并设置 closed；`KVReader` 没有自定义 `Drop`，所以需要确定性释放或让上层观察关闭错误时，应显式调用 `close`。`MergeKVIter` 在读尽、构造失败和整体关闭路径均有显式关闭逻辑。

`MemoryStorage` 内部用 `Arc<RwLock<BTreeMap<...>>>` 协调对象访问，但 `KVReader::from_storage` 在构造时已经克隆对象，后续记录读取不再持有存储锁。当前“并发读取”实现仍从内存 `Arc<Vec<u8>>` 取逻辑范围；其主要可观察契约是参数校验、模式状态与偏移连续性，而非远端请求并发度。

## 与 Go 版本的对应关系

共同语义：

- `DefaultReadBufferSize` 均为 64 KiB，构造时均把用户缓冲分为三份并保证每份至少 1 字节。
- 记录格式、两个大端 `u64` 长度头、首头 EOF 表示结束、记录内部 EOF 表示截断，以及并发模式切换/关闭入口均对应 `pkg/ingestor/simplesst/kv_reader.go`。
- Rust 的 Go 风格别名保留了迁移期间的 API 名称，核心逻辑集中在 snake_case 方法中。

当前差异：

- Go `NewKVReader` 接收 `context.Context` 和通用 `storeapi.Storage`，通过 storage reader 从偏移流式读取；Rust 接收 `MemoryStorage`，先克隆完整对象，没有 context、远端存储或文件大小查询接口。
- Go `EnableConcurrentRead` 接收 store、文件名和 `membuf.Buffer`，`Close` 显式销毁大缓冲池；Rust 只把并发度和每路缓冲大小传给内存 `ByteReader`，没有外部 buffer pool。
- Go `NextKV` 返回复用底层缓冲的借用切片；Rust 返回独立 `Vec<u8>`，生命周期更简单但有复制成本。
- Go 将 `uint64` 长度直接转换为 `int` 并做加法；Rust 显式检查加法和 `usize` 转换，并额外受底层 1 GiB 单次读取上限约束。
- Go `noEOF` 会记录 warning；Rust `no_eof` 只转换错误，不写日志。
- Go 有私有 `getFileSize`；Rust 没有对应方法，`MergeKVIter` 直接读取对象长度计算 `input_size`。

因此该文件是行为导向的内存后端移植，并非 Go 外部存储实现的一比一资源模型复刻；扩展时应保持线格式和 EOF 契约，不能把当前内存限制描述成通用对象存储能力。

## 扩展指南

- 若增加压缩、校验和或新记录头，优先修改 `next_kv` 及对应 writer 编码点，并在独立测试文件 `file_test.rs` 增加正常、截断、超长和旧格式兼容用例；不要把 Rust 单元测试嵌入本源文件。
- 若接入真实对象存储，应在构造层和 `ByteReader` 抽象中恢复流式/分段读取，而不是让 `KVReader` 直接操作存储锁。需要同步审视 Go 的 `NewKVReader`、`getFileSize`、buffer pool 销毁和 context 取消语义。
- 若改变返回值以减少复制，必须明确借用缓冲的失效时机，并检查 `MergeKVIter` 将 key 克隆进堆、value 存入 `KVPair` 的所有权需求；这是兼容性和内存安全高风险改动。
- 若调整 EOF 规则，必须保持“记录边界 EOF”和“记录内部 UnexpectedEOF”的区分，并同步 `file_test.rs::test_kv_reader_rejects_truncated_record_body`、正常读尽用例及 Go `file_test.go`/`iter_test.go` 对照行为。
- 若调整并发阈值或切换时机，修改责任主要在 `byte_reader.rs` 与 `iter.rs::rebalance_hotspot`；本文件应继续只做受检委托。同步测试 `iter_test.rs::test_read_after_close_conn_reader` 及热点切换测试。
- 性能关注点包括整对象克隆、每条记录正文分配以及 key/value 二次复制；任何优化都应同时验证小缓冲、跨预取块、初始偏移和关闭后读取，且不得用零复制改变现有拥有所有权的 API 而不评估调用方。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/ingestor/simplesst` 找到目标及同目录 Go/Rust 测试；`node --file pkg/ingestor/simplesst/kv_reader.rs --offset 1 --limit 260` 核对了全部 210 行、主要符号及“被 8 个文件使用”的文件级关系。精确 callers/callees 未返回边，故调用点由后续源码搜索核验。
- 目标实现：`pkg/ingestor/simplesst/kv_reader.rs` 的 `KVReader`、`new`、`from_storage`、`next_kv`、并发委托、别名、`no_eof` 和 `NewKVReader`。
- crate/模块边界：`pkg/ingestor/simplesst/Cargo.toml`、`pkg/ingestor/simplesst/lib.rs`，以及子系统定位 `pkg/ingestor/doc.go`。
- 下游实现：`pkg/ingestor/simplesst/byte_reader.rs` 的精确读取、1 GiB 上限、EOF、偏移、并发模式与关闭行为；`pkg/ingestor/simplesst/lib.rs` 的 `MemoryStorage`、`Error` 和 `Result`。
- 上游主链：`pkg/ingestor/simplesst/iter.rs` 的 `MergeKVIter::new_with_options`、`next`、`rebalance_hotspot`、`close` 和 `reader_concurrent_mode`。
- Rust 独立测试：`pkg/ingestor/simplesst/file_test.rs`（正常往返、正常 EOF、截断正文、缓冲大小 1/2/3/7/31）、`onefile_writer_test.rs` 与 `writer_test.rs`（存储产物回读）、`iter_test.rs`（退出并发态后 EOF 与热点模式）。
- Go 对照：`pkg/ingestor/simplesst/kv_reader.go`；相关 `file_test.go`、`onefile_writer_test.go`、`writer_test.go`、`byte_reader_test.go`、`iter_test.go` 用于核对随机缓冲、正常 EOF、并发切换和资源清理意图。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务文件规定的 11 章节结构校验，并人工复核本文没有把注释草案或 Go 的真实对象存储能力写成当前 Rust 已支持事实。

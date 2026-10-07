# `pkg/ingestor/simplesst/file.rs`

源文件：[`file.rs`](./file.rs)

## 文件定位

本文件属于 `astersql-ingestor-simplesst` crate；crate 入口 `pkg/ingestor/simplesst/lib.rs` 以 `pub mod file` 暴露该模块，并把它放在简化 SST 的编码、写入、读取与归并管线中。这里负责最底层的单条 KV 线格式和一份文件内容的内存累计，不负责对象存储提交、文件命名调度或读取。上层 `writer.rs` 与 `onefile_writer.rs` 在完成排序和重复键处理后调用本模块，再把生成的数据缓冲和统计信息写入存储。

`pkg/ingestor/simplesst/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/ingestor/simplesst`；其 Rust 依赖目前全部声明在 `cfg(windows)` 目标下。本文件自身只依赖 crate 内的 `Error`、`Result` 和 `writer::RangePropertiesCollector`，没有条件编译项。

## 核心职责

1. 定义数据对象与伴随对象使用的文件名后缀：`STAT_SUFFIX` 为 `_stat`，`DUP_SUFFIX` 为 `_dup`。
2. 以固定线格式 `<keyLen:u64 BE><valueLen:u64 BE><key><value>` 编码原始 KV；`LengthBytes` 固定为 8。
3. 由 `KeyValueStore` 顺序累计编码记录、维护已写入字节偏移，并可同步驱动 `RangePropertiesCollector` 生成范围属性。
4. 在交出结果前通过 `finish`/`into_parts` 刷新不足阈值的最后一段范围属性。

该类型名包含 “Store”，但当前 Rust 实现只是拥有 `Vec<u8>` 的内存构建器。真实对象写入发生在调用者中，因此它没有 I/O、重试、关闭或持久化职责。

## 主要符号

- `STAT_SUFFIX: &str`、`DUP_SUFFIX: &str`：由 `writer.rs` 和 `onefile_writer.rs` 拼接统计文件、冲突文件路径。
- `LengthBytes: usize = 8`：一段长度头的宽度。每条记录固定有 16 字节头部。
- `encode_kv(key, value) -> Result<Vec<u8>>`：检查总长度加法是否溢出，分配精确长度缓冲，再委托 `encode_to_buf`。
- `encode_to_buf(buf, key, value) -> Result<()>`：要求目标切片长度精确相等，写入两个大端 `u64` 长度和正文；它既不扩容，也不接受尾部余量。
- `KeyValueStore { data, collector, offset }`：分别拥有完整编码字节、可选范围属性收集器和成功写入后的文件尾偏移。该结构实现 `Clone`、`Debug`，未实现线程共享或异步接口。
- `KeyValueStore::new` / `NewKeyValueStore`：创建空缓冲、零偏移的 store；后者只是保留 Go 命名风格的包装。
- `add_encoded_data`：校验外部编码记录的头部和精确总长度，然后追加数据、更新偏移、通知 collector。
- `add_raw_kv` / `AddRawKV`：先用 `encode_kv` 编码，再进入同一条校验和累计路径。
- `offset` / `Offset`、`data`、`collector`：只读观察当前状态。
- `finish` / `Finish`：调用 collector 的 `on_file_end`，不清空数据，也不关闭或冻结 store。
- `into_parts`：消费 store，先 `finish`，再返回 `(Vec<u8>, Option<RangePropertiesCollector>)`。

## 执行流程

典型生产流程见 `writer.rs` 的 flush 路径和 `onefile_writer.rs::close`：

1. 上层先对行排序并按 `DuplicateMode` 处理重复键，得到要写入的数据行。
2. 上层用属性大小阈值和键数阈值创建 `RangePropertiesCollector`，再传给 `KeyValueStore::new`。
3. 每条行经 `add_raw_kv` 进入 `encode_kv`。编码器计算 `16 + key.len() + value.len()`，并由 `encode_to_buf` 写入大端长度头和正文。
4. `add_encoded_data` 重新解析两个长度头，确认输入至少 16 字节且声明长度与切片长度完全一致。
5. 校验成功后，记录追加到 `data`；`offset` 增加整条编码长度。若存在 collector，则调用 `on_next_encoded_data(data, offset)`。collector 从记录中取 key，累计不含 16 字节头部的 KV 大小与键数，达到任一阈值时封存一条 `RangeProperty`，并以当前文件尾作为下一段起点。
6. `into_parts` 调用 `finish`；collector 的 `on_file_end` 把尚未达到阈值且包含键的尾段封存。
7. 上层把返回的 `data` 写到数据路径，把 collector 的编码结果写到带 `_stat` 后缀的路径。冲突行可用相同记录格式写入 `_dup` 路径。

读侧 `KVReader` 使用同一格式逐条解析；`file_test.rs::canonical_kv_file_tracks_properties_and_reads_exact_records` 验证三条记录能够按边界读回并在耗尽时返回 EOF。

## 数据与状态

线格式中的两个长度均为大端 `u64`，随后紧密排列 key 和 value，没有校验和、版本号、压缩标记或记录对齐填充。`offset` 统计完整线格式字节数，包含长度头；`RangeProperty::Size` 则由 collector 统计 key/value 正文字节，不含每条记录的 16 字节头。这两个口径不可混用。

`KeyValueStore` 对缓冲与 collector 具有所有权。`data()` 和 `collector()` 只返回不可变借用；`into_parts()` 才转移所有权。成功添加记录时，状态变化顺序是追加 `data`、增加 `offset`、更新 collector。调用方应把 `finish` 视为范围属性的收尾点；它并非关闭操作。没有 collector 时，`file_test.rs::finish_does_not_close_the_store` 证明 `finish` 后仍可继续追加。

源码注释和 Go 契约要求同一 store 的 key 严格递增，但 `KeyValueStore` 不保存前一个 key，也不执行排序性检查。生产调用者通过写入前排序满足该前置条件；直接调用者必须自行保证，否则范围属性的 `FirstKey`/`LastKey` 不能代表有序范围。

## 依赖与调用关系

上游直接依赖关系：

- `writer.rs` 导入 `DUP_SUFFIX`、`STAT_SUFFIX`、`encode_kv` 和 `KeyValueStore`。其 flush 路径把排序后的 `kept` 行写成数据与统计对象，并把 `conflicts` 编码到 `_dup` 对象。
- `onefile_writer.rs` 导入两个后缀和 `KeyValueStore`；`close` 将 `data_rows` 写成一对数据/统计对象，并将 `duplicate_rows` 写成冲突对象。
- `iter_test.rs` 与 `file_test.rs` 直接使用 `KeyValueStore`/`encode_kv` 构造读取、归并测试输入；`onefile_writer_test.rs` 检查 `STAT_SUFFIX` 形成的路径。

下游直接调用关系由 RustCodeGraph 和源码共同确认：`add_raw_kv -> encode_kv -> encode_to_buf`，`add_raw_kv -> add_encoded_data`，`add_encoded_data -> RangePropertiesCollector::on_next_encoded_data`，`finish -> RangePropertiesCollector::on_file_end`，`into_parts -> finish`。错误统一使用 crate 根 `Error::InvalidData` 和 `Result`。

本文件不直接依赖 `MemoryStorage` 或外部对象存储 trait；这使编码和属性累计可独立于存储后端测试，但也意味着 Go 版的写入错误、context 取消和 writer 生命周期不在这里发生。

## 错误处理与边界

- `encode_kv` 对总缓冲长度使用 checked addition；无法表示时返回 `InvalidData("encoded key/value length overflow")`。
- `encode_to_buf` 拒绝长度不足或过长的目标切片，返回 `InvalidData("incorrect destination size for key/value")`。
- `add_encoded_data` 对少于 16 字节的输入返回 truncated 错误；对头部声明长度发生加法溢出返回 length overflow；对声明长度与实际切片不一致返回 length mismatch。它不接受拼接的多条记录，一次调用必须恰好一条。
- key/value 的 Rust 切片长度被转换为 `u64` 写头；在当前受 `usize` 限制的平台上不会产生比地址空间更大的实际切片，但格式层面仍以 `u64` 为上限。
- `offset` 使用 checked addition，理论溢出返回 `InvalidData("file offset overflow")`。
- collector 还会验证 key 边界并检查属性大小溢出，其错误通过 `?` 原样传播。

`add_encoded_data` 只在完成格式校验后才追加缓冲；但追加后才更新 offset 和 collector。因此，极端 offset 溢出或 collector 返回错误时，该方法不提供事务性回滚，`data`（以及可能的 `offset`）已经改变。常规生产输入不接近此极限，扩展错误注入或超大文件支持时仍应明确这一状态语义。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、任务、channel、异步函数或 `unsafe`。所有写方法需要 `&mut self`，Rust 借用规则阻止同一实例被无同步地并发修改；是否跨线程共享取决于调用方自行添加同步封装。

缓冲生命周期始于 `new`，记录逐条追加，通常由 `into_parts` 消费并交给上层存储。`finish` 只刷新 collector，不释放缓冲、不提交对象、不关闭资源，也没有 closed 标志。对带 collector 的实例，正常路径应在最后一次写入后只收尾一次并随即消费；虽然当前 Rust collector 在收尾后把 `Keys` 置零以避免立即重复追加尾属性，但继续写入并非生产流程，且保留的当前属性字段使其不适合作为新文件段重新开始。需要复用 collector 时应显式 `reset` 或新建实例。

内存占用随整份数据对象线性增长，并且 `add_raw_kv` 会先创建一份临时编码 `Vec`，再复制进 store 的 `data`。大对象、流式上传或零拷贝需求需要在本层与上层存储边界共同设计，不能只替换最终写对象调用。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ingestor/simplesst/file.go`。常量、记录布局、偏移含义、严格递增前置条件、范围属性阈值逻辑以及 `Finish` 只调用 `onFileEnd` 的核心语义保持一致。Rust 还提供 Go 风格别名 `NewKeyValueStore`、`AddRawKV`、`Offset`、`Finish`，便于逐步迁移调用点。

关键差异如下：

- Go `KeyValueStore` 持有 `context.Context`、`objectio.Writer` 和 collector 指针，`addEncodedData` 先直接写对象 writer，写失败便返回；Rust 持有内存 `Vec<u8>` 和拥有所有权的 `Option<RangePropertiesCollector>`，持久化由上层完成。
- Go 的 `NewKeyValueStore` 接收 context、writer、collector；Rust 构造函数只接收 collector。
- Go `addEncodedData` 假定输入编码正确，collector 也直接切片；Rust 增加了精确长度、溢出和截断校验并返回 `InvalidData`。
- Go 在每次 `AddRawKV` 中直接分配并编码；Rust 将编码拆为可复用、公开且可失败的 `encode_kv`/`encode_to_buf`。
- Go `Finish` 文档称 close，但实际不关闭 `dataWriter`；Rust 文档与独立测试明确保留这一真实行为。

Rust 测试 `file_test.rs` 对照 Go `file_test.go` 的范围属性与读写往返意图，但改为确定性数据，并额外验证截断记录的读侧错误和无 collector 时 finish 后仍可写。Go 测试还覆盖真实内存对象 writer 的 `Close`，该生命周期在 Rust 中属于上层而非本文件。

## 扩展指南

- 修改线格式时，应同步修改 `encode_kv`、`encode_to_buf`、`add_encoded_data`、`RangePropertiesCollector::on_next_encoded_data` 以及 `KVReader`，并考虑已有数据兼容性；至少同步独立测试 `file_test.rs`、`kv_reader` 相关测试和 Go 对照语义。
- 增加格式版本、校验和或压缩字段时，先明确旧格式探测方式，避免把现有 16 字节头误读为新头；范围属性的 key 起点和 `Size` 口径也必须随之复核。
- 若要支持真实流式 writer，不应把对象 I/O 简单塞入 `add_encoded_data`。需要同时决定部分写失败的回滚语义、context/取消、重试、close/commit 可见性，以及 collector 应在写成功前还是写成功后更新。
- 若要在本层强制有序性，可在 `KeyValueStore` 保存上一 key 并在追加前比较；这会增加每条记录比较/保存成本，并可能改变目前允许测试构造任意序列的行为。
- 若要保证错误原子性，应先完成 offset 与 collector 的可失败计算，再统一提交状态，或提供回滚；新增测试应直接覆盖畸形长度、collector 溢出/失败和错误后状态。
- 调整后缀会影响 `writer.rs`、`onefile_writer.rs`、对象路径消费者与测试，属于持久化命名兼容变更，不能只改本文件常量。
- Rust 单元测试应继续放在独立的 `file_test.rs`，不要内嵌到生产源文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/ingestor/simplesst` 确认本模块、Go 对照和独立测试均在索引中。
- RustCodeGraph `node --file pkg/ingestor/simplesst/file.rs --offset 1 --limit 260`：核对本文件全部 170 行、3 个常量、2 个编码函数、`KeyValueStore` 及其方法和 Go 风格包装。
- RustCodeGraph 调用边：确认 `encode_kv -> encode_to_buf`、`add_raw_kv -> encode_kv/add_encoded_data`、`add_encoded_data -> writer.rs::on_next_encoded_data`、`finish -> writer.rs::on_file_end`、`into_parts -> finish`。由于同仓库存在大量同名 `file`/`finish` 符号，泛化 callers 输出不能可靠消歧，上游引用另以精确源码搜索核验。
- 已阅读源码与边界文件：`file.rs`、`Cargo.toml`、`lib.rs`、`writer.rs` 的 collector 与 flush 路径、`onefile_writer.rs::close`、Go `file.go` 和 Go `writer.go` 的 collector 实现。
- 已阅读独立测试：Rust `file_test.rs`、Go `file_test.go`；并通过精确引用搜索确认 `iter_test.rs`、`onefile_writer_test.rs` 的相关使用。
- 人工复核结论：本文区分了编码构建与对象持久化，记录了严格递增前置条件而未声称本层会校验，并明确描述了错误后的非事务性状态、finish 非关闭语义及 Rust/Go 生命周期差异。

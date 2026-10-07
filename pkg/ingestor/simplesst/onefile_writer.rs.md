# `pkg/ingestor/simplesst/onefile_writer.rs`

## 文件定位

本文件属于 `astersql-ingestor-simplesst` crate（见 `pkg/ingestor/simplesst/Cargo.toml`），实现“一个数据对象加一个统计对象”的简化 SST 写入器 `OneFileWriter`。模块由 `pkg/ingestor/simplesst/lib.rs` 的 `pub mod onefile_writer` 暴露，构造入口是 `WriterBuilder::build_one_file`（`writer.rs`）。

当前 Rust 版本是基于 `MemoryStorage` 的整对象实现：行先保存在 writer 内部，`close` 时才编码并提交对象。它可供同 crate 测试和内存管线使用，但尚未像 Go `onefile_writer.go` 那样接收通用 `storeapi.Storage`、流式 `objectio.Writer` 或生产对象存储 sink。仓库中的生产 Rust 文件目前只直接引用本文件的常量：`pkg/dxf/importinto/task_executor.rs` 使用 `DefaultOneWriterBlockSize`，`pkg/ingestor/globalsort/merge.rs` 使用 `MaxUploadPartCount`；直接构造 `OneFileWriter` 的已检索 Rust 调用均在 `onefile_writer_test.rs`。

## 核心职责

- `write_row` 接收调用方已经按 key 排序的 KV。除 `DuplicateMode::Ignore` 外，它把连续相同 key 聚合为一个 `pivot` 组，并在换 key 或关闭时交给 `finish_pivot`。
- `finish_pivot` 落实四种重复键策略：`Ignore` 全保留；`Remove` 仅保留单元素组；`Record` 对重复组在主文件保留前两条、其余写冲突文件；`Error` 在第二条相同 key 到来时已报错，之后刷新该组时不再重复报错。
- `close` 将保留行编码为数据对象，用 `RangePropertiesCollector` 生成统计对象；若有 `Record` 模式的额外重复行，再生成 `_dup` 对象，并返回及回调 `WriterSummary`。
- 三个公开常量承载跨模块配置约定：128 MiB 默认内存/块大小，以及计算上传 part 大小时采用的 5000 分片除数。

## 主要符号

- `DefaultOneWriterMemSizeLimit`：128 MiB；本 writer 的 `emit` 用它限制**单条编码后 KV**，不是限制全部缓冲的总量。
- `DefaultOneWriterBlockSize`：等于上述内存上限。当前 `OneFileWriter::new` 会从 builder 取得 `block_size`，但没有保存或使用它。
- `MaxUploadPartCount`：值为 5000，供 `globalsort/merge.rs` 计算较保守的上传分片大小。
- `OneFileWriter`：持有 `MemoryStorage`、文件名前缀/随机状态、属性采样阈值、重复策略、关闭回调和分组偏移；运行时状态包括 `pivot`、`data_rows`、`duplicate_rows`、`buffered_bytes`、`part_size` 与 `closed`。
- `OneFileWriter::new`：crate 内构造器，从 `WriterBuilder::configuration` 取得参数，用 `join_path(prefix, writer_id)` 形成文件名前缀，并用 `writer::get_hash` 产生可复现的分区随机状态。
- `init_part_size`：只校验 part size 为正并保存；当前内存存储关闭路径不读取该值。`InitPartSizeAndLogger` 是其 Go 风格别名，Rust 版本没有 context/logger 参数。
- `emit`：计算 `16 + key.len() + value.len()` 的记录大小，拒绝超过 `memory_limit` 的单条记录，随后累计 `buffered_bytes` 并将 KV 推入 `data_rows`。
- `finish_pivot`：重复键策略的集中状态转换点。
- `write_row`：公共逐行入口；`WriteRow` 是别名。writer 关闭后返回 `Error::Closed`。
- `close`：公共提交入口；`Close` 是别名。它创建 `WriterSummary`、提交对象、调用 `on_close`，最后才把 `closed` 置为 true。

## 执行流程

1. 调用方通过 `WriterBuilder::build_one_file(storage, prefix, writer_id)` 进入 `OneFileWriter::new`。builder 提供属性距离、重复模式、回调和 `group_offset`。
2. `write_row` 首先拒绝已关闭 writer。`Ignore` 模式直接复制 KV 并调用 `emit`；其他模式将首行建立为 `pivot`，连续同 key 行追加到组内，遇到新 key 时先 `finish_pivot` 再建立下一组。
3. `Error` 模式在同 key 的第二行到达时立即返回 `Error::DuplicateKey`。该行已经被加入 `pivot`；之后切换 key 或关闭时，`finish_pivot` 识别组内存在第二项并丢弃整组，避免重复返回同一错误。这一行为由 `test_onefile_writer_dup_error` 固定。
4. `Remove` 对重复组不调用 `emit`；`Record` 对重复组克隆前两项到 `data_rows`，第三项及以后移动到 `duplicate_rows`。唯一组在两种模式下都会进入主数据。
5. `close` 先调用 `finish_pivot` 刷新最后一组。若 `data_rows` 非空，它依次产生不同的随机分区前缀，构造 `.../one-file` 数据路径和带 `_stat` 后缀的统计路径。
6. 主数据通过 `KeyValueStore::new(Some(RangePropertiesCollector::new(...)))` 和 `add_raw_kv` 编码；`into_parts` 得到数据字节与 collector，随后按“数据对象、统计对象”的顺序写入 `MemoryStorage`。
7. `close` 以 `data_rows` 的首/末 key 填充 `Min`/`Max`，计算主文件行数与不含长度头的 KV 总字节数，设置 `KVFileCount = 1`，并用 `MultipleFilesStat::build` 建立范围统计。
8. 若 `duplicate_rows` 非空，则创建 `_dup` 路径，以无范围 collector 的 `KeyValueStore` 编码和写入冲突对象，填充 `ConflictInfo`。
9. 所有写入成功后调用 `on_close(&summary)`，再置 `closed = true` 并返回汇总；因此失败的 `close` 可重试。

## 数据与状态

- `pivot` 的分组正确性依赖相同 key 连续出现；代码不验证全局单调排序。乱序输入会使同一 key 的多个非连续片段被当作不同组，且汇总 `Min`/`Max` 只是保留行的首/末 key，不会重新求真实极值。
- `data_rows` 和 `duplicate_rows` 拥有输入 KV 的副本。当前实现不会在写入过程中释放或分块落盘，内存随总行数增长。
- `memory_limit` 只在 `emit` 检查单条编码记录；`buffered_bytes` 使用饱和加法累计，但没有被读取来触发刷新或拒绝累计超限。`block_size` 配置完全未进入结构体。
- `property_size`、`property_keys` 在关闭时创建 `RangePropertiesCollector`，控制统计属性的切分距离；`test_onefile_writer_stat` 和 `test_onefile_prop_offset` 验证 key 数量、属性数量及 offset 单调性。
- `random_state` 从稳定哈希开始，每次 `rand_partitioned_prefix` 都推进。数据、统计和冲突对象因此可能位于不同随机分区，但路径序列对相同前缀与 writer ID 可复现。
- `part_size` 默认为 `MinUploadPartSize`，目前除参数校验/保存外不影响内存存储写入。`group_offset` 原样进入 `WriterSummary`。

## 依赖与调用关系

上游构造边为 `WriterBuilder::build_one_file -> OneFileWriter::new`。RustCodeGraph 将本文件列为被 `writer.rs`、`globalsort/merge.rs`、`pkg/dxf/importinto/task_executor.rs` 及测试使用；源码复核表明后两个生产文件分别只引用 `MaxUploadPartCount` 和 `DefaultOneWriterBlockSize`，没有构造 writer。当前直接行为覆盖来自独立文件 `pkg/ingestor/simplesst/onefile_writer_test.rs`。

主要下游依赖如下：

- `crate::file::KeyValueStore`：编码长度头、KV 正文，并可驱动范围属性 collector；`DUP_SUFFIX`、`STAT_SUFFIX` 定义派生对象路径。
- `crate::writer::WriterBuilder`：注入配置；`WriterSummary`、`MultipleFilesStat`、`ConflictInfo` 定义关闭结果；`join_path`、`rand_partitioned_prefix` 生成对象路径。
- `crate::writer::RangePropertiesCollector`：根据字节/键距离生成统计对象。
- `crate::MemoryStorage`：提供加锁的整对象 `write`。其克隆共享同一 `RwLock<BTreeMap<...>>`，对象仅在完整字节准备好后可见。
- `crate::Error`/`Result`：统一参数、关闭、重复键、编码和存储错误。

`Cargo.toml` 把 crate 根指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/ingestor/simplesst"` 记录 Go 对照包。清单中的外部/相邻 crate 依赖当前全部位于 `cfg(windows)` 下；本文件自身只依赖本 crate 的内存抽象。

## 错误处理与边界

- `init_part_size(0 或负数)` 返回 `Error::InvalidData`；不过成功设置的值当前不参与后续 I/O。
- 单条 KV 的编码长度超过 `memory_limit` 时，`emit` 返回 `Error::InvalidData`。长度包含两个 8 字节头；累计缓冲超过该限制不会报错。
- 第二次 `close` 或关闭后的 `write_row` 返回 `Error::Closed`。只有全部对象提交和回调完成后才置关闭标志。
- `KeyValueStore::add_raw_kv`、collector `encode`、`MultipleFilesStat::build` 与 `MemoryStorage::write` 的错误均通过 `?` 原样传播；失败时不调用关闭回调，也不设置 `closed`。
- 关闭不是事务性的：数据对象可能已写成功，而统计或冲突对象随后失败。重试时 `random_state` 已推进，会生成新路径；之前成功的对象不会回滚。独立测试只证明一次 `_stat/` 注入失败后可以再次关闭成功，没有声明或验证孤立对象清理。
- `Record` 模式只把第三条及以后计入 `ConflictInfo.Count`；前两条仍属于主数据。全部行被 `Remove` 时不创建数据/统计对象，汇总范围为空、计数为零。
- `on_close` 类型是普通 `Fn`，不能返回错误；若回调 panic，`closed` 尚未设置。代码没有捕获 panic。

## 并发与资源生命周期

`OneFileWriter` 的可变操作都要求 `&mut self`，自身不包含锁、任务或通道，不支持多个调用者并发写同一实例。共享性仅来自可克隆的 `MemoryStorage`：它在单次整对象提交时获取写锁，因此其他 reader 不会看到半个对象。

生命周期为“构造 -> 多次 `write_row` -> 一次成功 `close`”。对象在 `close` 前完全不存在于 storage；与 Go 流式 writer 不同，本实现没有显式底层 writer、上传 worker 或需要逐一关闭的资源。失败关闭保留内存行以便重试，但也可能在 storage 中留下此前已提交的对象。结构没有 `Drop` 实现，未调用 `close` 就丢弃实例时只会丢失内存缓冲，也不会触发回调。

## 与 Go 版本的对应关系

对应实现是 `pkg/ingestor/simplesst/onefile_writer.go`，对应测试是 `onefile_writer_test.go`。重复策略、主文件前两条/冲突文件其余项、长度头开销、延迟产生空文件、汇总字段和“成功后才 closed”的核心语义由 Rust 保留；Rust 测试名称也明确对照 Go 的 Basic、Stat、PropOffset、DupError 和 OnDupRemove 场景。

当前差异必须视为移植限制而非等价实现：

- Go 接受通用外部存储并在写入期间通过 `objectio.Writer`/`KeyValueStore` 流式输出；Rust 固定 `MemoryStorage`，直到关闭才整对象编码。
- Go 的 `membuf.Buffer` 用 block size 周期性刷新统计、重置缓冲，并把 collector offset 对齐已写字节；Rust 不使用 builder 的 block size，所有统计一次生成。
- Go 在创建统计 writer 失败时关闭数据 writer，并按数据、统计、冲突的次序关闭资源；Rust 没有这些 writer 生命周期，只做三次可能的整对象写入。
- Go 的 part size 控制上传且按约 999 个 part 记录进度，并累计 16 MiB 写入 metric；Rust `part_size` 当前不影响行为，也没有 logger/metric。
- Go 的内存限制意图是复用分配块，并拒绝大于块的单 KV；Rust同样拒绝过大的单 KV，但用 `Vec` 保存全部数据，不能提供 Go 的总内存/流式特性。
- Rust 汇总额外保留 builder 的 `GroupOffset`；Rust 公共 `close` 直接返回 `WriterSummary`，同时仍执行 builder 回调，而 Go `Close` 只通过回调交付汇总。

## 扩展指南

- 若接入生产对象存储，应从 `OneFileWriter` 的 `storage` 类型和 `WriterBuilder::build_one_file` 开始，引入与 `writer.rs::build_with_sink` 一致的 sink 抽象；同时实现 Go 的延迟创建、关闭顺序、部分失败清理和可重试语义。不要仅给 `MemoryStorage` 再包一层名称。
- 若实现真正的内存上限/流式刷新，应修改 `emit`、`write_row`/`finish_pivot` 和 `close`，使用 builder 的 `block_size`，保证 range-property offset 跨块连续。同步扩展独立测试 `onefile_writer_test.rs`，覆盖跨块统计、超大单 KV、总缓冲受限及写失败后的状态。
- 修改重复键行为时，以 `finish_pivot` 和 `write_row` 为共同入口，保持“连续有序组”不变量，并同步 Go 的 `onefile_writer_test.go` 场景。尤其要验证最后一组在 `close` 刷新、全重复 Remove 不产空文件、Error 报错后的关闭行为，以及 Record 的前两条/其余项边界。
- 修改路径算法时要同步 `rand_partitioned_prefix` 调用次数与顺序，并检查 `MultipleFilesStats` 和 `ConflictInfo.Files`。失败重试是否复用路径或清理旧对象需要先定义兼容契约。
- 若要支持乱序输入，不能只修改 `Min`/`Max`；还必须选择排序、全局去重或显式拒绝乱序，并评估内存、稳定顺序和 Go 兼容性。
- Rust 测试逻辑必须继续放在独立的 `pkg/ingestor/simplesst/onefile_writer_test.rs`，不要内嵌进生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认仓库索引可用；`files --filter pkg/ingestor/simplesst` 定位模块；`node --file pkg/ingestor/simplesst/onefile_writer.rs --offset 1 --limit 400` 与后续 `--offset 338 --limit 300` 阅读完整源文件。文件关系报告列出 `writer.rs`、`globalsort/merge.rs`、`pkg/dxf/importinto/task_executor.rs` 和独立测试等使用者；方法级通用名称查询未提供可靠消歧结果，因此调用边再由入口源码核验。
- Rust 源码：`onefile_writer.rs` 的 `OneFileWriter::{new, init_part_size, emit, finish_pivot, write_row, close}`；`writer.rs` 的 `WriterBuilder::{build_one_file, configuration}`、`WriterSummary`；`file.rs` 的 `KeyValueStore` 与后缀常量；`lib.rs` 的模块、错误和 `MemoryStorage` 定义。
- crate/上游证据：`pkg/ingestor/simplesst/Cargo.toml`、`pkg/dxf/importinto/task_executor.rs`、`pkg/ingestor/globalsort/merge.rs`。用精确 `rg` 检索确认生产 Rust 路径仅引用本文件常量。
- 对照与测试：完整阅读 `pkg/ingestor/simplesst/onefile_writer.go`、`onefile_writer_test.go` 和独立 Rust 测试 `onefile_writer_test.rs`。Rust 测试覆盖 Record、Basic、属性统计/offset、Error、Remove 与关闭失败重试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文档存在且恰好包含 11 个固定二级标题，并人工复核唯一生产物、源码链接、差异声明和扩展测试位置。

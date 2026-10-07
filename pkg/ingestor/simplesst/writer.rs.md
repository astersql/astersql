# `pkg/ingestor/simplesst/writer.rs`

源文件：[`writer.rs`](writer.rs)

## 文件定位

`writer.rs` 是 `astersql-ingestor-simplesst` crate 的多文件排序写入端。crate 由同目录 `Cargo.toml` 定义，`lib.rs` 通过 `pub mod writer` 暴露本模块，并将统一错误类型 `Error`、结果别名 `Result` 和内存对象存储 `MemoryStorage` 放在 crate 根。该文件接收未排序的键值对，在内存阈值处或关闭时排序，再生成一组数据对象、范围统计对象以及可选的重复键冲突对象，供后续 simplesst 读取、merge-sort 和 ingest 流程消费。

该模块既支持测试及内存管线使用的 `MemoryStorage`，也通过 `WriterSink` 接入生产对象存储适配器。图索引显示 `Writer::write_row` 的直接使用者包括本模块的 `EngineWriter::append_rows`、本目录 `writer_test.rs`，以及全局排序、DDL ingest 和 import-into 相关 Rust 测试；真正的存储后端桥接点是调用方实现的 `WriterSink`，而不是本文件拥有对象存储客户端生命周期。

## 核心职责

- `WriterBuilder` 集中保存写入器配置，构造多文件 `Writer`、外部 sink 版本的 `Writer` 或同 crate 的 `OneFileWriter`。
- `Writer::write_row` 复制并缓冲 KV，在配置了 `key_prefix` 时先编码 key，在下一条记录将使批次超过内存上限前调用 `flush(false)`。
- `Writer::flush` 对批次按 key 排序，按 `DuplicateMode` 处理同键组，写数据/统计/冲突对象，并累计 `WriterSummary`。
- `RangePropertiesCollector` 按原始 KV 字节数或 key 数量切分 `RangeProperty`，生成与数据文件对应的统计内容。
- `MultipleFilesStat` 将文件对按起始 key 排序，并计算组内范围最大重叠；`GetMaxOverlappingTotal` 再对多组统计做加权重叠估算。
- `EngineWriter` 把行切片适配到逐 KV 的 `Writer` API，作为 Lightning 风格写入接口的轻量门面。
- `rand_partitioned_prefix`、`IsValidPartition`、调整阈值函数及 `get_speed` 提供 Go 版本同类辅助语义。

本文件不负责读取或归并 SST，不管理上传线程、上下文取消或对象存储连接关闭；这些职责分别位于同 crate 的 reader/iterator 模块和调用方 sink。

## 主要符号

- `WriterSink: Send + Sync`：外部对象存储的最小写接口，只有 `write_file(path, data)`；错误字符串被 `WriterStorage::write` 映射为 `Error::Io`。
- `WriterStorage`：私有枚举，在 `MemoryStorage` 与 `Arc<dyn WriterSink>` 间统一整对象写入。
- `RangePropertiesCollector`：保存已完成属性 `props`、当前属性 `curr_prop` 和两个切分阈值。`on_next_encoded_data` 校验 `<key-len:8><val-len:8><key><value>` 至少含完整头和 key，累计不含 16 字节头的 `Size`，达到任一阈值即封存；`on_file_end` 封存尾段；`encode` 委托 `codec::encode_multi_props`。
- `DuplicateMode::{Ignore, Record, Remove, Error}`：分别保留整组、保留前两条并记录其余、丢弃整组、返回 `Error::DuplicateKey`。
- `ConflictInfo`：记录冲突 KV 数量和冲突文件路径。
- `MultipleFilesStat`：保存整组最小/最大 key、排序后的 `[data_path, stat_path]` 文件对及最大重叠数；`build` 要求三组输入长度一致。
- `WriterSummary`：关闭成功后的快照，包括 writer 标识、分组、序号、全局键范围、有效 KV 大小/数量、数据文件数、多文件统计和冲突信息。
- `WriterBuilder`：默认使用 256 MiB 内存上限、16 MiB block、1 MiB/8192 keys 属性阈值、`DuplicateMode::Ignore` 和空关闭回调。`block_size` 传给 `OneFileWriter`，当前多文件 `Writer::new` 不消费该值。
- `Writer`：持有存储、路径/随机状态、缓冲 KV、重复键策略、关闭状态、汇总和尚未封存的每文件最小/最大 key。
- `EngineWriter`：`append_rows` 顺序调用 `write_row`，`is_synced` 恒为 `true`，`close` 转交底层 writer。
- 常量与辅助函数：`MultiFileStatNum` 是可原子调整的每组文件数（默认 500）；`FLUSH_KVS_RETRY_TIMES` 为 3；`GetAdjustedMergeSortOverlapThreshold` 和 `GetAdjustedMergeSortFileCountStep` 按 `250 * max(concurrency, 1)` 截顶；`GetAdjustedBlockSize` 在对齐浪费超过 10% 时使用实际总量。

为对齐 Go 命名，若干 Rust API 同时保留 snake_case 实现和 `New*`、`Set*`、`WriteRow`、`Close` 等别名。扩展时应优先把逻辑放在 snake_case 主实现，避免两套路径漂移。

## 执行流程

1. 调用方通过 `WriterBuilder` 设置内存阈值、属性粒度、重复策略、分组偏移、keyspace 前缀和关闭回调，再调用 `build` 或 `build_with_sink`。`Writer::new` 用 `get_hash(filename_prefix)` 生成确定性随机种子。
2. `Writer::write_row` 首先拒绝已关闭 writer；若有 `key_prefix`，把它前置到 key。随后以 `16 + key.len + value.len` 计算编码占用，并用 checked arithmetic 拒绝长度溢出。
3. 当缓冲非空且加入新记录会超过 `memory_limit` 时，先执行 `flush(false)`。单条记录本身大于上限会返回错误；否则复制 key/value 入 `rows` 并增加 `buffered_size`。
4. `flush` 对当前批次按 key 排序，并扫描连续同键组。单键组总是保留；重复组按 `DuplicateMode` 分流为 `kept`、`conflicts` 或错误。
5. 若有 `kept`，函数用 `rand_partitioned_prefix` 生成 `p[01]{8}/<prefix>/<writer-id>/<seq>` 形式的数据路径，统计路径在前缀后加 `STAT_SUFFIX`。`KeyValueStore::add_raw_kv` 编码数据并驱动 `RangePropertiesCollector`，随后数据对象和统计对象各通过 `write_object_with_retry` 写入。
6. 写入成功后才更新全局 min/max、有效 KV 数量和原始 key/value 字节数、数据文件数；文件对加入当前 `MultipleFilesStat`。当上一组已达到 `MultiFileStatNum` 时，先用累计的 per-file min/max 构建该组，再开启新组。
7. 若有 `conflicts`，使用 `DUP_SUFFIX` 路径将冲突 KV 编码成独立对象，成功后更新 `ConflictInfo`。
8. 本批次全部成功后清零缓冲字节并递增序号。`closing == true` 时还会构建最后一个未封存的多文件统计组。
9. `Writer::close` 在最终 flush 之前将 `closed` 设为 `true`；最终 flush 成功后补齐 summary 标识字段、调用 `on_close`，返回 summary 克隆。即使最终 flush 失败，writer 也已被消费，后续写入或关闭返回 `Error::Closed`。

## 数据与状态

内存缓冲是 `Vec<(Vec<u8>, Vec<u8>)>`，因此 `write_row` 会取得调用方字节的所有权副本。`buffered_size` 使用带 16 字节长度头的编码大小来实施内存阈值；`WriterSummary::TotalSize` 只统计成功保留并写出的原始 key/value 字节，不含长度头，且不含被 Remove/Record 分流掉的冲突记录。`written_bytes` 返回的就是这个已成功 flush 的累计值，所以关闭前最后一个尚在缓冲的批次不计入。

每个数据对象内部按 key 有序，但不同 flush 批次间不承诺全局有序。`summary.Min`/`Max` 是所有成功数据文件的闭区间边界；`pending_min`/`pending_max` 只服务于当前尚未构建的统计组。`MultipleFilesStat::Filenames` 在 `build` 后按对应起始 key 排序，`MaxOverlappingNum` 使用 inclusive start/end 端点计算，因此边界相等也算重叠。

`RangeProperty::Offset` 是新属性开始时的数据文件尾偏移；`FirstKey`/`LastKey` 来自已编码记录，`Size` 排除两个长度字段。属性对象通过 `encode_multi_props` 序列化，数据对象则由 `KeyValueStore` 及 `file::encode_kv` 保持同一 KV framing。

`MultiFileStatNum` 是进程级 `AtomicUsize`，读取使用 Acquire；测试会临时 swap/store 改小该值。并发执行会共享这一全局配置，测试或调用方修改它时必须避免和其他 writer 并行产生统计分组。

## 依赖与调用关系

上游入口主要有两类：直接用 `WriterBuilder` 构造并逐行调用 `Writer::write_row`；或用 `NewEngineWriter`/`EngineWriter::append_rows` 适配批量行。RustCodeGraph 对 `write_row` 的调用边显示，除 `WriteRow` 别名和 `append_rows` 外，直接验证者包括 `pkg/ingestor/simplesst/writer_test.rs`、`pkg/ingestor/globalsort/util_test.rs`、`pkg/dxf/importinto/encode_and_sort_operator_test.rs` 与 DDL ingest 相关测试。这里的 `Writer` 与 `pkg/ddl/ingest/engine.rs` 中同名 trait/内存实现不是同一类型；后者是另一个 ingest 引擎抽象，不能据同名推断直接 trait 实现关系。

下游依赖如下：

- `crate::file::{KeyValueStore, encode_kv, STAT_SUFFIX, DUP_SUFFIX}` 负责 KV 文件编码和路径后缀。
- `crate::codec::{RangeProperty, encode_multi_props}` 负责范围属性模型及统计编码。
- `crate::util::{Endpoint, EndpointTp, get_max_overlapping}` 负责范围重叠计算。
- `crate::{MemoryStorage, Error, Result}` 提供测试/内存存储及 crate 统一错误。
- `crate::onefile_writer::OneFileWriter` 复用 builder 配置，但单文件实现位于独立源文件。

`Cargo.toml` 将此目录声明为 `astersql-ingestor-simplesst`，并记录 Go 包映射 `pkg/ingestor/simplesst`。当前列出的外部 crate 依赖位于 `cfg(windows)` 表；本文件自身只直接使用标准库和 crate 内模块，外部对象存储能力通过 trait 注入。

## 错误处理与边界

- 已关闭后写入或再次关闭返回 `Error::Closed`。关闭先置位，因此最后一次写失败也不可重试整个 writer。
- 编码长度加法溢出、单 KV 超过 writer 内存上限、截断的范围属性输入、属性累计大小溢出，以及 `MultipleFilesStat::build` 输入长度不一致，均返回 `Error::InvalidData`。
- `DuplicateMode::Error` 在排序 flush 时返回首个检测到的重复组的 key/value；错误发生前该批次未更新 summary。
- 每个数据、统计或冲突对象分别最多尝试写三次。`write_object_with_retry` 不做退避、取消检查或错误分类，第三次错误原样返回。
- 数据对象与统计对象不是事务性提交：例如数据成功而统计最终失败时可能留下孤立数据对象；重试也会覆盖同一路径。summary 只在相关写入步骤成功后更新，但外部 sink 必须接受重复的同路径整对象写。
- `RangePropertiesCollector::on_next_encoded_data` 验证头长度和 key 范围，但不单独验证声明的 value 长度与切片尾部一致；它按实际切片长度累计大小。完整 KV 格式的构造责任在 `KeyValueStore`/`encode_kv`。
- `GetAdjustedBlockSize(0, default)` 明确返回默认值，避免 Go 版本除零路径；调整并发函数把非正并发钳制为 1，而 Go 版本还会进行 intest 断言和错误日志。
- `get_speed` 仅对恰好为零的 duration 返回 `-`；调用方若传负数仍会格式化负速率。

## 并发与资源生命周期

单个 `Writer` 的可变操作要求 `&mut self`，不提供内部锁，也不声明 `Send + Sync` 契约；同一实例应由一个执行流串行写入和关闭。不同 writer 可以并行使用共享 sink，前提是 `WriterSink: Send + Sync` 的实现自行保证并发安全。`WriterStorage::External` 用 `Arc` 共享 sink，但 writer 不负责关闭 sink。

内存资源随 `rows` 在 flush 时由 `std::mem::take` 转移；成功结束后新缓冲为空，局部 `original`/`kept`/`conflicts` 离开作用域释放。失败路径可能在返回前保留或丢弃部分批次状态：例如重复键 Error 在 `rows` 已被 take 后返回，writer 又可能已被 close 标记，因此调用方应把 flush/close 错误视为该 writer 终止，而不是继续写入恢复。

对象采用整文件写入语义：`MemoryStorage` 在写锁下原子替换完整 `Vec<u8>`，外部 `WriterSink` 也只收到完整字节切片。与 Go 的 `objectio.Writer` 相比，Rust 本文件没有流式 close、multipart worker、context cancellation 或显式内存池销毁生命周期。

`on_close` 仅在最终 flush 成功后同步调用，回调持有 `&WriterSummary`，不能保留该借用；若需要跨线程使用，回调应复制所需字段。`EngineWriter::is_synced` 恒真只是 checkpoint 合约信号，不表示底层有额外 fsync 操作。

## 与 Go 版本的对应关系

直接对照文件是同目录 `writer.go`，独立测试是 `writer_test.go`；Rust 回归集中在 `writer_test.rs`。主要对应关系为：

- Go `WriterBuilder`、`Writer`、`WriterSummary`、`RangePropertiesCollector`、`MultipleFilesStat`、`EngineWriter` 在 Rust 中均有同名或同语义类型；默认内存/属性阈值、三次 flush 尝试、随机分区路径和 500 文件统计分组保持一致。
- Go `WriteRow` 使用 `membuf.Buffer` 和 slice location，Rust 直接拥有 `Vec`；Go 的“单 KV 不能超过 blockSize”在 Rust 中表现为“不能超过 memory_size_limit”。Rust builder 的 `block_size` 只影响 `OneFileWriter`，不是多文件 writer 的分配块限制。
- Go `tikv.Codec::EncodeKey` 在 Rust 中简化为 `set_key_prefix`，只表达 API V2 前缀拼接，不是任意 codec 接口。`writer_encodes_keyspace_before_sorting_and_summarizing` 验证前缀参与排序和 summary 边界。
- 重复键语义一致：Record 保留每组前两条、其余写冲突文件；Remove 删除整组；Error 返回重复键；Ignore 全保留。由于去重只看单个 flush 批次，两批之间的相同 key 不会在本 writer 内被识别，这也对应 Go 注释中“没有全局视图”的限制。
- Go 在一次 `flushSortedKVs` 调用内整体重试并感知 `ctx.Err()`；Rust 对每个对象独立做三次无条件尝试，没有上下文、日志或指标。Go 使用流式 writer、上传并发 20 和 `MinUploadPartSize`，Rust 虽保留相关公开常量，但 `WriterSink` 不接收上传选项。
- Go `Close` 返回错误并通过回调交付 summary；Rust `close` 直接返回 `WriterSummary`，仍在成功后调用回调。两者都在最终 flush 前标记 closed，Rust 测试 `close_failure_still_closes_writer` 固化了这一点。
- Go `LockForWrite` 是无操作方法；Rust `Writer` 没有这个兼容入口。Go 的日志、吞吐 metrics 和 buffer destroy 也未在本文件移植。

Go 测试 `TestWriterFlushMultiFileNames`、`TestWriterDuplicateDetect`、`TestWriterMultiFileStat`、`TestFlushKVsRetry`、`TestWriterOnDup`、阈值及分区前缀测试均有 Rust 对应测试。Rust 另有外部 sink、keyspace 前缀和关闭失败消费 writer 的聚焦覆盖。

## 扩展指南

- 新增或改变重复键策略时，修改 `DuplicateMode` 与 `Writer::flush` 的连续组分支，同时更新 summary/冲突文件口径；至少同步 `writer_test.rs::canonical_writer_flushes_sorted_files_and_enforces_duplicate_policy`、`test_writer_on_dup`、`test_writer_duplicate_detect`，并复核 Go `writer.go::flushKVs` 与 `writer_test.go` 的同类意图。
- 修改内存阈值或编码大小计算时，同时检查 `Writer::write_row` 的 16 字节头、`RangePropertiesCollector` 排除头的 Size 口径、`WriterSummary::TotalSize` 原始 KV 口径和 `written_bytes` 的“只计已 flush”语义。不要把三种大小混为同一指标。
- 更换文件命名或随机分区算法时，保持 data/stat/dup 三类路径的序号关联，并同步 `test_writer_flush_multi_file_names`、`test_rand_partitioned_prefix` 以及读取端对 `STAT_SUFFIX`/`DUP_SUFFIX` 的发现规则。
- 调整属性切分时，修改 `RangePropertiesCollector::on_next_encoded_data`/`on_file_end`，并用 `StatsReader` 验证 offset、键范围和尾段；相关独立测试仍应放在 `writer_test.rs`，不要内嵌进生产文件。
- 改变多文件统计分组或重叠算法时，维护 `Filenames` 与 start/end key 的同索引不变量，并同步 `test_multi_file_stat`、`test_multi_file_stat_overlap` 和 `test_writer_multi_file_stat`。尤其要保留 inclusive endpoint 语义。
- 接入新的生产存储时实现 `WriterSink`，保证同路径重试可安全覆盖、整对象成功返回后可见，并在适配层管理连接/关闭/取消。若需要 multipart、退避或取消，宜扩展 sink 合约或引入独立适配层，而不是假设现有 `write_file` 已提供这些能力。
- 扩展 builder 时检查 `configuration()` 的元组顺序和 `onefile_writer.rs` 的消费方；最好将配置改为具名内部结构后再增加大量字段，以降低位置错配风险。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/ingestor/simplesst` 确认 `writer.rs`、`writer_test.rs`、Go 对照和相邻模块均已索引。
- 源码事实：`pkg/ingestor/simplesst/writer.rs` 的 `RangePropertiesCollector`、`WriterBuilder`、`Writer::{write_row, flush, close}`、`write_object_with_retry`、`EngineWriter`、路径/阈值辅助函数。
- crate 与模块边界：`pkg/ingestor/simplesst/Cargo.toml` 的 package/lib/porting 元数据；`pkg/ingestor/simplesst/lib.rs` 的 `pub mod writer`、统一 `Error`/`Result` 和 `MemoryStorage` 整对象语义。
- 调用边证据：RustCodeGraph `explore` 显示 `write_row` 被 `WriteRow`、`EngineWriter::append_rows`、本目录 writer 测试以及 globalsort、DDL ingest、import-into 相关 Rust 测试调用；本文件的下游调用由精确文件节点核对为 `KeyValueStore`、`encode_multi_props`、`get_max_overlapping` 和存储写接口。
- Go 对照：`pkg/ingestor/simplesst/writer.go` 的 `WriterBuilder`、`flushKVs`、`flushSortedKVs`、`writeDupKVs`、`EngineWriter`、统计与路径辅助函数。
- 测试证据：`pkg/ingestor/simplesst/writer_test.rs` 覆盖排序、四类重复键行为中的关键分支、keyspace 前缀、外部 sink、文件命名、关闭失败、重试、统计分组/重叠、block 调整和分区格式；`pkg/ingestor/simplesst/writer_test.go` 提供原始 Go 意图对照。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；交付结构检查要求本文恰含十一个固定二级标题，并另以 `git diff --check` 检查 Markdown 变更格式。

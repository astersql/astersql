# `pkg/lightning/tikv/local_sst_writer.rs`

## 文件定位

本文件属于 `astersql-lightning-tikv` crate。crate 的入口 `pkg/lightning/tikv/lib.rs` 以私有模块 `local_sst_writer` 装载本文件，再通过 `pub use local_sst_writer::*` 公开其公共符号；`pkg/lightning/tikv/Cargo.toml` 用 `[package.metadata.porting] go-package = "pkg/lightning/tikv"` 指明对应的 Go 包。

它处于 Lightning 写入 TiKV write column family（CF）的本地文件边界：调用者按键序提供用户键和短值，`WriteCFWriter` 将其转换成 TiKV MVCC/write-CF 编码，关闭时生成可被 RocksDB/TiKV 工具识别的 BlockBasedTable SST。文件还包含一个面向验证和调试的读取器 `read_local_sst`，用于把本实现写出的记录及表属性读回；当前仓库搜索到的 Rust 调用均位于独立测试 `pkg/lightning/tikv/local_sst_writer_test.rs`，没有发现生产模块直接构造 `WriteCFWriter` 的证据，因此不能把它描述为已经接入 Lightning 生产导入主链。

## 核心职责

1. `WriteCFWriter::set` 把用户键编码成 `z + memcomparable(key) + !ts`，把值编码成 `P + uvarint(ts) + v + one-byte length + payload`，并强制编码后键严格递增。
2. `WriteCFWriter::close` 调用 `write_sst`，为全部记录生成 MVCC/Range 用户属性、数据块、Bloom filter、索引块、属性块、metaindex 和 RocksDB v2 footer，最后 `sync_all`。
3. `BlockBuilder` 实现 RocksDB block 的前缀压缩条目与 restart 数组；`write_block` 给每个 block 附加未压缩类型字节和 masked CRC32C。
4. `read_local_sst` 校验 footer、block 范围、block 类型、CRC、varint 和 internal-key trailer，然后读回数据记录及 `rocksdb.properties`。它是受限的本地调试读取器，不是通用 RocksDB reader。

## 主要符号

- `WriteCFWriter { path, ts, records }`：公共写入器。`path` 是目标文件，`ts` 是所有记录共用的 commit timestamp，`records` 保存已经完成 TiKV 编码且严格递增的键值。`newWriteCFWriter`、`Set`、`Close` 是与 Go 命名对齐的公共别名；惯用 Rust 入口分别是 `WriteCFWriter::new`、`set`、`close`。
- `isShortValue` / `is_short_value`：判断值长度是否不超过 `u8::MAX`（255），这是 write-CF 内联短值的一字节长度上限。
- `encode_mvcc_key`：添加数据键前缀 `z`，调用私有 `encode_memcomparable` 按 8 字节分组编码原始键，再追加按位取反的 big-endian 时间戳，使同一原始键的较新版本按字节序排在前面。
- `encode_write_value`：产生 Put 记录（`P`），追加 LEB128 风格无符号时间戳、短值标记 `v`、一字节长度和值内容。
- `BlockHandle`：记录 block 在文件中的字节偏移与不含 trailer 的长度；通过 `encode_block_handle` 编成两个 uvarint。
- `BlockBuilder`：维护 block 数据、上一键、restart offsets、条目数和 restart interval。数据块间隔为 16，index/metaindex 的间隔为 1，属性块使用 `usize::MAX`，因此只有第一个条目成为 restart point。
- `write_sst`：关闭阶段的核心私有函数。它驱动 `MvccPropCollector`、`RangePropertiesCollector`，按约 32 KiB 拆分数据块，并写入 filter/index/properties/metaindex/footer。
- `insert_standard_properties`：补齐条目数、原始键值大小、block 大小、comparator、filter policy、external SST 版本和 collector 名称等 RocksDB 属性。
- `masked_crc32c`、`bloom_hash`、`build_bloom_filter`：本地实现 block checksum 与 RocksDB builtin full-filter 数据。
- `LocalSst { records, properties }` 与 `read_local_sst`：公共测试/调试结果及读取入口；其余 `read_block`、`decode_block_handle`、`decode_uvarint`、`decode_block_entries` 负责边界校验和解码。

## 执行流程

1. `WriteCFWriter::new(path, ts)` 先以 `File::create` 创建或截断目标文件，随后仅保留路径、时间戳和空记录向量；此时尚未写 SST 结构。
2. 每次 `set(key, value)` 先以断言拒绝超过 255 字节的值，再调用 `encode_mvcc_key` 和 `encode_write_value`。若新编码键小于或等于最后一个编码键，返回 `TikvError::InvalidArgument`；否则把完整编码结果加入 `records`。
3. `close(self)` 消耗 writer 并进入 `write_sst`。函数先遍历记录，以 `InternalKey` 包装编码后的 MVCC 键，依次调用 `MvccPropCollector::Add` 和 `RangePropertiesCollector::Add`，再调用两个 `Finish` 写入 `tikv.min_ts`、`tikv.max_ts`、行数、`tikv.rows_index`、`tikv.range_index` 等属性。
4. `write_sst` 再次创建/截断文件。每条 MVCC 键由 `encode_internal_key` 追加 8 字节 little-endian trailer `1`，表示 sequence number 0 与 SET kind；数据块预计加入下一条后超过 `DATA_BLOCK_SIZE`（32 KiB）时冲刷。空记录也会写一个合法空数据块，并以 8 字节零 internal trailer 作为索引键。
5. 数据块完成后，函数记录当前位置为 `data_size`，基于所有 MVCC 用户键构建每键约 10 bits、6 probes 的 full Bloom filter；随后构建指向数据块的 index block。
6. `insert_standard_properties` 合并 RocksDB 标准属性。属性名排序后写入 property block；metaindex 写入 filter 和 properties 两个句柄；`write_footer` 写 checksum 类型、metaindex/index 句柄、footer version 2 和 RocksDB magic。最后 `File::sync_all` 保证文件内容同步到存储设备。
7. 验证读取路径从 53 字节 footer 反向取得 metaindex/index 句柄，读取属性块，再按 index 中的句柄遍历所有数据块。每个 internal key 必须至少有 8 字节 trailer 且 trailer 必须等于 1，返回结果会去除该 trailer，但保留 TiKV MVCC 编码键。

## 数据与状态

`WriteCFWriter` 的可变状态只在内存中：所有已编码记录保存在 `Vec<(Vec<u8>, Vec<u8>)>`，直到 `close` 才真正构造 SST。因此内存消耗与总键值量线性增长，关闭阶段还会建立属性、Bloom filter、block 和索引等临时缓冲；这与 Go 版本边写边交给 Pebble writer 的资源曲线不同。

排序不变量作用于 `encode_mvcc_key` 的结果，而不是仅比较原始键。由于 writer 对所有记录使用同一 `ts`，正常情况下它等价于用户键的 memcomparable 字节序；重复键同样被禁止。`ts` 同时进入每条 write value 和 MVCC key，并作为 MVCC 属性的 min/max timestamp。

数据块的 block trailer 固定为 5 字节（1 字节 compression type + 4 字节 masked CRC32C），文件 footer 固定为 53 字节。实际 `write_block` 写入的 compression type 为 0，即 block bytes 未压缩；与此同时 properties 中的 `rocksdb.compression` 被标为 `ZSTD`。当前测试只验证 magic、记录和部分 TiKV 属性，没有证据证明该属性与未压缩 block 的组合已和所有外部 reader 做过兼容验证。

`MvccPropCollector` 约每 10,000 键生成 rows-index 锚点；`RangePropertiesCollector` 默认以 4 MiB 或 40 Ki 键为距离生成 range-index 锚点。两个 collector 的实现位于 `pkg/lightning/tikv/prop_collector.rs`，不应在本文件中复制其编码规则。

## 依赖与调用关系

内部主调用边经 RustCodeGraph 核对为：

- `newWriteCFWriter -> WriteCFWriter::new -> File::create`；
- `WriteCFWriter::Set -> set -> isShortValue + encode_mvcc_key + encode_write_value`；
- `WriteCFWriter::Close -> close -> write_sst`；
- `write_sst -> MvccPropCollector/RangePropertiesCollector + BlockBuilder + build_bloom_filter + write_block + insert_standard_properties + write_footer`；
- `read_local_sst -> read_block + decode_block_handle + decode_block_entries`，而 `read_block -> masked_crc32c`、`decode_block_entries -> decode_uvarint`。

crate 内依赖来自 `crate::{InternalKey, MvccPropCollector, RangePropertiesCollector, TikvError}`：前三者的核心定义分别由 `prop_collector.rs` 提供，`TikvError` 在 `tikv.rs` 中定义。标准库提供集合、文件 I/O、路径和 seek/write 能力。`Cargo.toml` 没有为本文件直接声明 Pebble、RocksDB、CRC 或压缩依赖；SST、checksum 和 filter 格式均由本文件自行编码，crate 的 `regex`、`semver`、`thiserror` 依赖主要服务相邻模块和错误定义。

模块入口会公开再导出这些 API，但仓库级 `rg` 与 RustCodeGraph caller 查询均未发现目标写入器的 Rust 生产调用者；明确可见的上游是 `local_sst_writer_test.rs` 中的 `pebbleWriteSST`。Go 版本同名函数同样只在当前 Go 测试文件中被直接引用。因此当前可证实角色是已公开的移植实现和等价性验证基础，而非已证实的生产接线。

## 错误处理与边界

- 文件创建、写入、seek、读取及 `sync_all` 的 `std::io::Error` 通过 `?` 转成 `TikvError::Io`。
- 值长度超过 255 时，`set` 使用 `assert!` 触发 panic，消息指出 default CF 写入尚未实现；它不是可恢复的 `Result::Err`。扩展长值支持必须同时写 default CF，不能只取消断言或截断长度。
- 编码键非严格递增（包括重复）时返回 `TikvError::InvalidArgument`，不会把失败记录加入内存；但构造时创建的空目标文件仍存在。
- `close` 消耗 writer，因此成功或失败后都不能重试同一个实例。关闭阶段重新截断目标路径，任一中途错误都可能留下不完整文件，本文件没有临时文件、原子 rename 或失败清理逻辑。
- `read_local_sst` 将格式问题统一映射为 `TikvError::InvalidData`，覆盖文件过短、magic/footer 不支持、句柄和长度溢出、block 越界、压缩 block、不匹配 checksum、截断/溢出 varint、非法 restart/entry 范围及错误 internal trailer。
- 调试读取器仅接受 block type 0，不解压任何压缩 block；它读取本文件写出的 filter 句柄但不解释 filter，也不校验 restart offset 的具体值。因此不能用它替代 Pebble/RocksDB 对任意 SST 的完整兼容性验证。
- 空输入仍会生成 SST；Bloom filter 为 5 个零字节，属性中的 entry count 为 0。该路径由源码明确处理，但当前独立 Rust 测试没有专门断言空表行为。

## 并发与资源生命周期

`WriteCFWriter` 没有锁、原子变量、后台任务或通道；其写入方法需要 `&mut self`，自然要求单一可变所有者串行追加。文件路径也没有跨实例协调：多个 writer 指向同一路径会互相截断或覆盖，调用者必须保证路径独占。

构造函数立即创建文件，但不长期持有句柄；`close` 才重新打开、写入、同步并在函数返回时关闭句柄。如果 writer 在调用 `close` 前被丢弃，只会留下构造时创建的空文件，缓存在 `records` 中的数据全部丢失。相反，`close(self)` 的所有权设计可防止同一实例重复关闭。

写入阶段没有增量落盘或背压，10,000-key 测试证明会跨多个 32 KiB data block，但没有改变“全部记录常驻内存直到关闭”的生命周期。读取器同样通过 `std::fs::read` 把整个 SST 载入内存，不适合不受控的大文件生产读取。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/tikv/local_sst_writer.go`，对应测试是 `pkg/lightning/tikv/local_sst_writer_test.go`。

语义保持部分包括：短值上限为 255；MVCC key 使用 `z`、TiDB memcomparable bytes 和 `^ts` big-endian 后缀；write value 使用 `P/uvarint(ts)/v/len/value`；目标 data block size 为 32 KiB；Bloom policy 目标为 10 bits；collector 包含 MVCC、Range 和名为 `BlobFileSizeCollector` 的标识；对外保留 `newWriteCFWriter`、`set/Set`、`close/Close` 风格入口。

实现方式不同：Go `writeCFWriter` 持有 `pebble/sstable.Writer`，`set` 立即调用 `Writer.Set`，`close` 直接调用 Pebble `Close`，并由 Pebble 负责 ZSTD、filter、properties 和 SST 格式。Rust 没有直接依赖 Pebble/RocksDB，而是缓存记录并自行编码 SST；`BlobFileSizeCollector` 仅以属性列表名称体现，没有独立 collector 状态；Rust 额外提供受限的 `read_local_sst`。Go 测试中的真实 TiKV import-service 比较是手工开关的集成测试，Pebble 与 TiKV 样例 SST 的完整属性对比也处于 skip/注释状态；Rust 测试覆盖的是本地自产自读、magic、编码、部分属性和错误路径，不能据此宣称逐字节等价于 Go/Pebble/TiKV 输出。

特别需要注意，Go writer 明确配置 `rocks.ZstdCompression`，Rust properties 也声明 ZSTD，但 Rust `write_block` 当前写未压缩 block type 0。新增兼容功能时应以真实 RocksDB/Pebble/TiKV reader 的结果为准，不要仅依靠 `read_local_sst` 的自产自读验证。

## 扩展指南

- 支持超过 255 字节的值时，从 `WriteCFWriter::set` 的断言处切入，但必须设计并同时产出 default CF SST，以及维护 write CF 中对 default value 的引用语义；同步扩展独立测试，不能把值长度强转为 `u8`。
- 调整 MVCC/write 编码时修改 `encode_mvcc_key`、`encode_memcomparable`、`encode_write_value` 或 `append_uvarint`，并同步 `local_sst_writer_test.rs` 的固定字节断言及 Go/TiKV 对照证据。编码顺序变化还会影响 `set` 的排序不变量、Bloom filter 和 range properties。
- 调整 SST 布局或兼容性时关注 `BlockBuilder`、`write_block`、`write_footer`、`build_bloom_filter` 和 `insert_standard_properties`。优先增加外部 Pebble/RocksDB/TiKV reader 验证，尤其覆盖 compression type/属性一致性、空表、多 block、filter、footer 和 checksum；不要仅扩展自产自读测试。
- 调整属性统计时进入 `prop_collector.rs` 的两个 collector，并同步其独立测试 `pkg/lightning/tikv/prop_collector_test.rs`；本文件只负责喂入记录并合并标准属性。
- 降低内存占用需要重构 `WriteCFWriter.records` 和 `write_sst` 为增量 block 写入，同时保留 collector、排序校验、最后索引键、Bloom hash 与标准属性所需统计。应专门验证大输入、失败清理和 close 中途 I/O 错误。
- 强化落盘原子性应采用同目录临时文件、成功 `sync_all` 后 rename，并明确目录同步及失败清理策略；需添加独立故障测试。不要把 Rust 测试写回生产源文件，继续通过 `lib.rs` 的 `#[path = "local_sst_writer_test.rs"]` 挂载。
- 若要接入生产 Lightning 路径，应先找到真实的 KV 排序/导入边界，明确目标 TiKV/RocksDB 版本和 SST contract，再增加生产调用与端到端 ingest 验证；当前仓库证据不足以证明已有接线。

## 验证依据

- 目标源码：`pkg/lightning/tikv/local_sst_writer.rs`，核对了全部 646 行、公共/私有符号、常量、编码、写入与读取分支；文件无条件编译分支。
- crate 边界：`pkg/lightning/tikv/Cargo.toml`、`pkg/lightning/tikv/lib.rs`；目标包目录没有 `doc.go`。
- 直接依赖：`pkg/lightning/tikv/prop_collector.rs` 中 `MvccPropCollector::{Add, Finish}`、`RangePropertiesCollector::{Add, Finish}`，以及 `pkg/lightning/tikv/tikv.rs` 中 `TikvError`。
- Go 对照：`pkg/lightning/tikv/local_sst_writer.go`；Go 测试：`pkg/lightning/tikv/local_sst_writer_test.go`；Rust 独立测试：`pkg/lightning/tikv/local_sst_writer_test.rs`。
- RustCodeGraph：`status` 显示目标仓库索引包含 11,467 个文件，`files --filter pkg/lightning/tikv` 确认目标实现、Go 对照和测试均已索引；`node --file ... --offset ...` 读取目标全貌；`query` 定位 `WriteCFWriter`、`write_sst`、`read_local_sst`、`MvccPropCollector`、`RangePropertiesCollector`、`TikvError`；`callees` 验证 `set`、`close/write_sst` 和读取器的内部调用边。精确 caller 查询未返回生产调用，随后以仓库 `rg` 核对，Rust 引用仅见本文件和 `local_sst_writer_test.rs`。
- 测试证据：`TestIntegrationTest` 检查行数和 range index；`TestProducesRocksDbSst` 检查 RocksDB magic；`TestPebbleWriteSST` 检查顺序、固定编码和 256 字节拒绝；`TestPebbleWriteSSTManyKeys` 覆盖 10,000 条记录与多 data-block；`TestDebugReadSST` 覆盖坏文件和乱序键。按任务要求这是纯文档分析，没有运行 Cargo 或测试。

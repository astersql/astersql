# `br/pkg/metautil/stubs.rs`

源码：[`stubs.rs`](stubs.rs)

## 文件定位

该文件属于 `astersql-br-pkg-metautil` crate。crate 入口 [`lib.rs`](lib.rs) 通过 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并从 crate 根再导出 `kvproto`、`DecodeTableID`、`JSONTable`、`Key`、`PartitionStatisticLoadTask`、`StatsReadWriter` 和 `StatsTypesJSONTable`。因此它不是独立业务入口，而是 [`metafile.rs`](metafile.rs)、[`statsfile.rs`](statsfile.rs)、[`load.rs`](load.rs) 与 [`debug.rs`](debug.rs) 共享的依赖边界。

[`Cargo.toml`](Cargo.toml) 将该 crate 标记为 Go 包 `br/pkg/metautil` 的库移植，并明确说明为适配 darwin arm64，当前没有直接依赖 `kvproto`、`grpcio`、`statistics-handle` 或 `tablecodec`。本文件用本地类型替代这些依赖中 metautil 实际用到的最小接口。这里的“桩”仍参与当前 Rust 生产 crate 的编译和数据流，但不等于上游依赖的完整实现。

## 核心职责

文件包含四组职责：

1. `Message` 与 `parse_from_bytes<T>` 用 `serde_json` 提供类似 protobuf 的序列化接口，供元数据和统计文件读写路径统一调用。
2. `JSONTable`、`PartitionStatisticLoadTask` 与 `StatsReadWriter` 描述统计信息的 JSON 形状和恢复加载边界。
3. `Key` 与 `DecodeTableID` 从 TiDB 表键中恢复有符号 table ID，并容忍 TiKV API V2 的四字节前缀。
4. `kvproto::brpb` 定义 BR 当前会用到的 `File`、`StatsFileIndex`、`StatsBlock`、`StatsFile`、`Schema`、`MetaFile`、`RawRange` 与 `BackupMeta` 数据形状和 protobuf 风格访问器；`CipherInfo` 和 `encryptionpb` 则从 `astersql-br-pkg-utils` 复用。

该边界的关键限制是：字段语义和上层控制流尽量对齐 Go，但 `Message` 产生的是 JSON 字节，不是真实 protobuf wire bytes。文件中的结构只覆盖当前 Rust metautil 所需字段、getter/setter 和所有权操作，不能当作完整 kvproto API。

## 主要符号

- `Message: Sized + Serialize + Deserialize`：默认实现 `write_to_bytes`、追加式 `write_to_vec` 与 `compute_size`。`File`、所有 stats/brpb 容器和 `BackupMeta` 都实现该 trait。
- `parse_from_bytes<T: Message>(&[u8])`：通过 `serde_json::from_slice` 恢复具体消息；`protobuf` 子模块仅重新导出它和 `Message`，以便调用方保留近似 protobuf 的路径。
- `JSONTable`：统计 JSON 的共享结构。`Columns`、`Indices`、`Partitions` 使用 `HashMap<String, serde_json::Value>` 保存尚未强类型化的统计载荷；其余字段保存库表名、计数、版本、谓词列和历史统计标记。`#[serde(default, rename_all = "snake_case")]` 允许缺字段使用默认值，并生成 Go JSON 标签风格的字段名。
- `StatsTypesJSONTable = JSONTable`：加载侧类型别名，使序列化和加载使用同一数据形状。
- `PartitionStatisticLoadTask`：携带 rewrite 后的 `PhysicalID` 和可选的装箱 `JSONTable`。
- `StatsReadWriter`：要求实现者同时满足 `Send + Sync`，核心方法 `LoadStatsFromJSONConcurrently` 接收新表信息、任务接收端和并发度参数。
- `Key(Vec<u8>)`、`TABLE_PREFIX`、`DecodeTableID`：本地 tablecodec 边界。解码支持直接以 `t` 开头的表键，以及以 `x`/`r` 加三字节 keyspace ID 开头的 API V2 键。
- `File`：SST/备份文件的名称、CF、键范围、版本范围、校验和、KV/字节计数、大小和加密 IV。
- `StatsFileIndex`：统计载荷的内联内容或远端对象引用，以及明文哈希、密文/原文大小和 IV。
- `StatsBlock` 与 `StatsFile`：前者绑定 physical ID 与一段 JSON；后者聚合多个块。`take_json_table`、`take_blocks` 使用 `std::mem::take` 转移大对象并清空原字段。
- `Schema`：序列化后的 DB/table/stats、校验统计、TiFlash 副本数、统计文件索引及表级/分区级 merge 选项。`clear_stats` 和 `clear_stats_index` 只清本地字段。
- `MetaFile`：v2 元数据索引树节点，可包含数据文件、schema、子节点、raw range 和 DDL 字节。`take_*` 系列支持批量消费容器。
- `RawRange`：raw KV 的起止键。
- `BackupMeta`：备份根元数据，同时容纳 v1 扁平字段和 v2 的 `schema_index`、`file_index`、`ddl_indexes`。三个 `mut_*_index` 在 `None` 时惰性创建默认 `MetaFile`。

## 执行流程

元数据写入时，[`metafile.rs`](metafile.rs) 构造 `File`、`Schema`、`MetaFile` 或 `BackupMeta`，通过 getter/mutator 组织 v1 扁平列表或 v2 索引树，再调用 `Message::write_to_bytes`/`write_to_vec`。读取索引叶时，`walkLeafMetaFileDyn` 下载并解密内容、校验 SHA-256，然后调用 `stubs::protobuf::parse_from_bytes::<MetaFile>`，递归访问 `MetaFile::get_meta_files`。

统计写入时，[`statsfile.rs`](statsfile.rs) 把 `JSONTable` 编码成 JSON，写入带 physical ID 的 `StatsBlock`，再放进 `StatsFile::mut_blocks`。刷盘时整个 `StatsFile` 经 `Message` 编码；小的首个载荷可以进入 `StatsFileIndex.inline_data`，其余载荷写对象存储并由 index 记录名称、哈希、大小和 IV。恢复时 `downloadOneStatsFile` 选择内联或远端内容，解密并校验后解析 `StatsFile`，用 `take_blocks` 消费各块、查找 physical ID rewrite、反序列化 `JSONTable`，最后发送 `PartitionStatisticLoadTask` 给 `StatsReadWriter`。

schema 加载时，[`metafile.rs`](metafile.rs) 从 `BackupMeta`/`MetaFile` 展开 `Schema` 与 `File`；文件键交给 `DecodeTableID` 归入逻辑表或分区。随后 [`load.rs`](load.rs) 将得到的 `Table` 按数据库名聚合。调试路径 [`debug.rs`](debug.rs) 同样通过 `parse_from_bytes` 解码 `MetaFile` 和 `StatsFile`，再生成旁路 JSON 供检查。

`DecodeTableID` 的具体步骤是：先检查首字节是否为 `t`；若不是，则只接受长度大于四且首字节为 `x` 或 `r` 的 API V2 键，并跳过四字节；随后要求剩余长度至少九字节，读取 `t` 后的八字节大端整数，最后异或最高位，恢复 TiDB 对有符号整数进行字典序编码前翻转的符号位。

## 数据与状态

本文件没有进程级可变状态。数据主要由拥有所有权的 `String`、`Vec<u8>`、`Vec<T>`、`HashMap` 和 `Option<MetaFile>` 组成；派生的 `Default` 使缺失集合为空、标量为零/false、索引为 `None`。这只是桩的构造默认值，不表示真实 kvproto 的所有默认值和存在性语义均已移植。

`get_*` 返回借用，`set_*` 整体替换旧值，`mut_*` 暴露集合的可变引用，`take_*` 则把值移出并把字段恢复为空容器。调用方在 `take_*` 后不能期待旧对象仍持有数据。`BackupMeta` 的三个可选索引用 `has_*` 区分“通道未使用”和“存在一个内容为空的索引节点”；调用 `mut_*_index` 会把前者变成后者。

`Message::write_to_vec` 追加到调用方已有缓冲区而非覆盖。`compute_size` 每次重新序列化，并把长度收窄成 `u32`；序列化失败时返回 `0`。因此它适合作为当前分片阈值的近似值，不提供 protobuf `encoded_len` 的 wire 精确语义。

## 依赖与调用关系

直接依赖为 `serde`、`serde_json`、标准库集合/IO/同步通道，以及 `astersql-meta-model::TableInfo`。加密类型不是本地复制：`kvproto::{CipherInfo,encryptionpb}` 从 `astersql-br-pkg-utils` 再导出，这也是 [`Cargo.toml`](Cargo.toml) 中 `astersql-br-pkg-utils` 依赖的直接用途之一。

RustCodeGraph 对该文件记录 176 个符号，并显示它被 25 个文件使用。最直接的生产调用关系包括：

- [`lib.rs`](lib.rs) 公开模块和核心符号；
- [`metafile.rs`](metafile.rs) 使用 `BackupMeta`、`MetaFile`、`Schema`、`File`、`RawRange`、`Message`、`parse_from_bytes` 和 `DecodeTableID`；
- [`statsfile.rs`](statsfile.rs) 使用 `JSONTable`、统计任务/trait、stats brpb 类型和 `Message`；
- [`debug.rs`](debug.rs) 使用 `parse_from_bytes::<MetaFile/StatsFile>` 及 brpb getter 展开调试输出；
- [`load.rs`](load.rs) 通过 `MetaReader` 间接消费上述类型，并依赖 `DecodeTableID` 完成文件归属。

图查询也确认 `parse_from_bytes` 在本包的 `DecodeMetaFile`、`DecodeStatsFile`、元数据叶读取和统计恢复路径出现；`DecodeTableID` 的直接测试调用者是 [`parity_test.rs`](parity_test.rs) 的 `decode_table_id_accepts_api_v2_keyspace_prefixes`。由于常见 getter 名称在全仓库高度重名，调用关系判断应优先结合文件限定和上述具体消费路径，不能把所有同名图节点都算作本桩调用者。

## 错误处理与边界

`write_to_bytes` 和 `parse_from_bytes` 将 `serde_json` 错误统一包装成 `std::io::ErrorKind::InvalidData`；不会保留 protobuf 字段号、wire type 或未知字段信息。`write_to_vec` 原样传播编码错误。`compute_size` 则故意吞掉编码错误并返回 `0`，因此调用方不能用它判断序列化是否成功。

`DecodeTableID` 对非法前缀、过短的 API V2 前缀、剥离前缀后不是表键、以及不足九字节的键都返回 `0`，不返回 `Result` 且不会 panic。`0` 因而既可能是合法解码值，也可能是无效输入哨兵；需要区分错误的上层必须先验证键格式。

`Message` 的 JSON 字节只能保证当前 serde 类型间往返，不能由 Go 的 `proto.Unmarshal` 或真实 kvproto 消费。反之，Go protobuf bytes 也不能直接交给本文件的 `parse_from_bytes`。`JSONTable` 用任意 JSON 值保留统计载荷形状，但没有编译期验证直方图、CMSketch 等内部 schema。当前 `StatsReadWriter` 仅抽象被恢复路径调用的方法，不覆盖真实 statistics handle 的其他能力。

`mut_schema_index`、`mut_file_index` 和 `mut_ddl_indexes` 内部的 `unwrap` 由前置的 `is_none` 初始化保证，不接受外部并发修改。清理与 `take_*` 方法只改变内存对象，不会删除对象存储文件或撤销已经发送的任务。

## 并发与资源生命周期

桩类型自身不启动线程、不持有锁，也没有 `Drop` 副作用。大部分值由调用方独占；共享时需要由上层提供 `Arc`、锁或消息通道。`StatsReadWriter: Send + Sync` 明确要求统计处理器可跨线程共享，而 `PartitionStatisticLoadTask` 随 `std::sync::mpsc` 通道转移所有权。

实际并发生命周期位于 [`statsfile.rs`](statsfile.rs)：`RestoreStats` 生成有界通道，下载线程/worker 解码 `StatsFile` 并发送任务，加载实现消费任务；发送端释放后接收端结束。`StatsBlock::take_json_table` 和 `StatsFile::take_blocks` 让恢复路径尽早释放大块字节。元数据索引并行下载位于 [`metafile.rs`](metafile.rs) 的 `walkLeafMetaFileDyn`，本文件只提供可在线程间移动的数据结构。

`Message::write_to_vec` 借用调用方缓冲区期间同步追加，不执行异步 IO。`BackupMeta` 可选索引的惰性初始化要求对实例有 `&mut self`，因此安全 Rust 会阻止同一实例在无同步条件下同时修改；本文件没有额外的原子性或事务保证。

## 与 Go 版本的对应关系

Go 的 [`metafile.go`](metafile.go) 和 [`statsfile.go`](statsfile.go) 直接使用 `github.com/pingcap/kvproto/pkg/brpb`、`gogo/protobuf/proto`、`pkg/statistics` 与 `pkg/tablecodec`，同目录没有对应的 `stubs.go`。Rust 文件因此是依赖适配层，而不是某个 Go 文件的逐函数翻译。

对应关系分为三层：

- 字段层：`File`、`StatsFileIndex`、`StatsBlock`、`StatsFile`、`Schema`、`MetaFile`、`RawRange`、`BackupMeta` 的字段语义和 protobuf 风格访问器与 Go `backuppb` 的使用点对齐；`JSONTable` 的 snake_case JSON 字段对齐 Go `statistics/util.JSONTable`。
- 控制流层：`DecodeTableID` 保留 Go tablecodec 的符号位翻转，并补足 Go 在上游 `tikv.DecodeKey` 完成的 API V2 前缀剥离；stats 内联/远端索引、physical ID rewrite、v1/v2 元数据索引的上层流程保持对应。
- 实现能力层：Go 使用真实 protobuf wire 编码和完整 statistics handle；Rust 用 JSON `Message`、任意 JSON 统计子字段及单方法 trait。跨语言只能比较字段语义和业务结果，不能比较序列化字节布局、精确编码大小或完整 API 面。

[`parity_test.rs`](parity_test.rs) 直接验证 API V2 table key 解码、stats 文件命名、JSON 往返、索引刷盘和 physical ID rewrite；[`statsfile_test.rs`](statsfile_test.rs) 验证完整 `JSONTable` 字段往返及多种 cipher 下的统计备份/恢复。它们分别对照 Go 的 tablecodec/statsfile 行为，但不证明 JSON 桩与 protobuf wire 兼容。

## 扩展指南

新增或调整 BR 元数据字段时，应先确认 Go/kvproto 的真实字段语义，再同步修改本文件对应结构、getter/setter/`mut_*`/`take_*` 接口以及 serde 默认行为。随后检查 [`metafile.rs`](metafile.rs)、[`debug.rs`](debug.rs) 和 [`load.rs`](load.rs) 是否需要读写该字段，并在独立的 [`metafile_test.rs`](metafile_test.rs)、[`debug_test.rs`](debug_test.rs) 或 [`load_test.rs`](load_test.rs) 中添加回归；不要把测试内嵌进 `stubs.rs`。

扩展统计字段时，优先保持 `JSONTable` 的 Go JSON 名称和缺字段兼容性；若要把 `serde_json::Value` 收紧为强类型，必须验证 Go 产生的历史/分区/谓词列载荷仍能解码，并同步 [`statsfile_test.rs`](statsfile_test.rs) 的全字段往返测试和 [`parity_test.rs`](parity_test.rs) 的端到端用例。

修改 `DecodeTableID` 时应同时覆盖普通 `t` 键、`x`/`r` API V2 键、短键、错误模式前缀、负数/零/正数 table ID，并与真实 Go `tablecodec.DecodeTableID` 及 `tikv.DecodeKey` 的组合语义核对。修改 `take_*`、`clear_*` 或惰性索引初始化时，应测试操作后的源对象状态，避免重复消费或把 `None` 和空节点混为一谈。

若未来接入真实 kvproto/protobuf/statistics 依赖，安全的迁移点是替换本文件及 crate 根再导出，同时保留上层调用接口并新增跨语言 fixture。不要在各业务模块再维护第二套条件分支。该迁移还需关注 wire 兼容、`compute_size` 的阈值变化、未知字段处理、生成类型的指针/可选语义和编译平台依赖成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/metautil` 确认本包 22 个已索引源码/测试文件；对 `stubs.rs` 的文件节点读取显示 807 行、176 个符号和 25 个使用文件；`explore`/调用查询确认 `parse_from_bytes`、`DecodeTableID`、stats/metafile 消费路径及相关测试调用边。
- Rust 源与 crate 边界：[`stubs.rs`](stubs.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`metafile.rs`](metafile.rs)、[`statsfile.rs`](statsfile.rs)、[`load.rs`](load.rs)、[`debug.rs`](debug.rs)。
- Rust 独立测试：[`parity_test.rs`](parity_test.rs)、[`statsfile_test.rs`](statsfile_test.rs)、[`metafile_test.rs`](metafile_test.rs)、[`load_test.rs`](load_test.rs)、[`debug_test.rs`](debug_test.rs)。这些测试分别覆盖 table key、JSON/stats、索引树/消息大小、加载和调试解码；目标源文件自身不包含测试模块。
- Go 对照：[`metafile.go`](metafile.go)、[`statsfile.go`](statsfile.go)、[`load.go`](load.go) 及其同目录 `*_test.go`。Go 使用真实 kvproto/protobuf/statistics/tablecodec，证明当前 Rust 文件是局部依赖桩而非 wire 兼容实现。
- 本任务仅新增说明文档，没有修改 Rust、Go、Cargo 或 `plan.md`，按任务约束未运行 Cargo。交付前使用任务指定命令验证目标存在且固定二级标题恰为 11 个，并人工复核所有链接和“桩/真实实现”边界。

# `pkg/statistics/handle/storage/json.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-storage` crate。crate 入口 `pkg/statistics/handle/storage/lib.rs` 以 `pub mod json` 挂载本模块，并用 `pub use json::*` 重导出其公开项。它位于统计信息内存表示与历史统计持久化之间：把 `TableStats` 包装成 `JsonTable`，将其编码、gzip 封装并切成可写入 `mysql.stats_history.stats_data` 的块，也提供逆向恢复和按快照查询历史统计的入口。

`pkg/statistics/handle/storage/Cargo.toml` 指明该 crate 对应 Go 包 `pkg/statistics/handle/storage`。当前实际 Rust 实现只直接使用标准库和同 crate 的共享类型；清单中的迁移依赖均置于 `cfg(any())`，不会因此证明本文件已经接入完整 TiDB 类型系统。

## 核心职责

- `generate_json_table_from_stats` 从内存 `TableStats` 和谓词列使用信息生成导出视图，并在每个列/索引项之后调用取消检查。
- `table_stats_from_json` 恢复表统计、重绑定物理表 ID，并兼容没有记录 `stats_version` 的旧统计。
- `json_table_to_blocks` / `blocks_to_json_table` 实现确定性的私有二进制 payload、hex JSON 外壳、未压缩 DEFLATE gzip member 和定长分块的往返转换。
- `table_historical_stats_to_json` 通过 `SqlStore` 查询快照之前最新的 meta/history 版本，按 `seq_no` 拼块，覆盖实时计数并标记历史来源。
- 私有编码器和 `Cursor` 对直方图、CMSketch、FM Sketch、TopN、桶及辅助字段实施对称编码和严格边界校验。

这里的“JSON”不是 Go `encoding/json` 对 `statsutil.JSONTable` 的直接线格式：Rust 将自定义小端二进制转为十六进制，再放入单字段 `{"payload":"..."}` JSON。因此该格式只能按本模块及其调用方的实际契约解释。

## 主要符号

- `PredicateColumn { id, last_used_at, last_analyzed_at }`：谓词列使用元数据；两个时间字段均可缺失。
- `JsonTable { database_name, table_name, stats, predicate_columns, is_historical_stats }`：本模块导入、导出与历史持久化的顶层值。
- `generate_json_table_from_stats(...) -> Result<JsonTable, Error>`：克隆整张 `TableStats`，将 usage map 转为按列 ID 升序排列的列表，初始历史标记为 `false`。
- `table_stats_from_json(physical_id, json) -> TableStats`：克隆统计后覆盖 `physical_id`；对 `stats_version == 0` 且 NDV 或 NULL 数非零的列/索引提升到版本 1，并取各项版本最大值作为表版本。
- `json_table_to_blocks(table, block_size)` 与 `blocks_to_json_table(blocks)`：公开的持久化编解码边界；前者拒绝零块长，后者拒绝空块列表。
- `table_historical_stats_to_json(store, physical_id, snapshot)`：公开的历史快照读取入口，返回 `(JsonTable, exist)`。
- `encode_table` / `decode_table`、`put_columns` / `Cursor::columns`：成对的二进制编解码器。列和索引 map 写出前按名称排序，以消除 `HashMap` 随机种子的影响。
- `gzip_store` / `gunzip_store`：只写、只接受本模块支持的未压缩 DEFLATE block；`crc32` 生成并验证 gzip trailer。
- `hex` / `unhex`：payload 的小写十六进制外层编码；奇数长度或非法字符会报错。

## 执行流程

导出内存统计时，`generate_json_table_from_stats` 先遍历 `columns.values().chain(indices.values())`，每项调用一次 `check_cancelled`，任何错误立即返回；成功后克隆表统计、转换 usage，并按 ID 排序。`pkg/statistics/handle/storage/dump_test.rs::predicate_usage_is_sorted_and_keeps_nil_timestamps` 证明排序、空时间保留和检查次数，`dump_stops_at_the_first_cancellation_error` 证明首错即停。

持久化时，`json_table_to_blocks` 依次执行 `encode_table`、hex、JSON 外壳、`gzip_store`，最后按 `block_size` 切片。`encode_table` 固定按库名、表名、表级字段、列、索引、谓词列、历史标记的顺序写入；每个变长值都有小端 `u64` 长度前缀。列/索引内依次写直方图元数据、版本、两个 sketch、TopN 和 buckets。

恢复时，`blocks_to_json_table` 按传入顺序拼块，经 `gunzip_store`、UTF-8 校验、精确 JSON 外壳匹配、`unhex` 和 `decode_table` 逆向恢复。`decode_table` 要求消费全部 payload，历史标记只能为 0 或 1。`pkg/statistics/handle/storage/dump_test.rs::dump_and_load_preserve_complete_table_statistics` 覆盖完整多块往返。

按快照恢复时，`table_historical_stats_to_json` 先查询 `stats_meta_history` 中不晚于 snapshot 的最新版本，再读取该版本的 `modify_count,count`；随后独立选择 `stats_history` 中不晚于 snapshot 的最新版本，按 `seq_no` 读取 blocks 并解码。若 meta 或 history 版本不存在，返回默认表和 `false`；成功时以 meta 计数覆盖 payload 中的计数，设置 `is_historical_stats = true`。

## 数据与状态

本文件自身没有全局可变状态。公开转换函数拥有或克隆其结果；`JsonTable.stats` 是 `TableStats` 的完整克隆，谓词列字符串也被克隆。编码过程只构造局部 `Vec<u8>`，解码由只读切片上的 `Cursor { data, at }` 单调推进。

序列化保持表 ID、行数、修改数、版本、表级统计版本，列/索引名称与 `ColumnStats` 的直方图、sketch、TopN 和桶。`Option<Vec<u8>>` 在线格式中以空字节串表示缺失，因此解码时空 sketch 会变回 `None`；这意味着“存在但长度为零”和“不存在”不在该格式中区分。谓词时间通过独立 0/1 标记保留 `None`。

确定性依赖两个显式排序/顺序约束：usage 按列 ID 排序，列和索引 map 按键字典序编码；历史 blocks 必须由 SQL 的 `order by seq_no` 或调用方等价顺序提供。

## 依赖与调用关系

下游依赖集中在 `crate::{Bucket, ColumnStats, Error, Histogram, SqlStore, TableStats, TopNItem}`：数据结构和 `Error`/`SqlStore` 定义于 `pkg/statistics/handle/storage/stats_read_writer.rs`，集合与字节处理使用标准库。

RustCodeGraph 对目标文件识别出 29 个符号；精确源码节点确认上述公开/私有调用链。调用者查询未在限定时间内返回可用边，因此以仓库引用搜索补证：

- `pkg/statistics/handle/handle.rs` 在历史统计编码路径调用 `json_table_to_blocks(..., MAX_COLUMN_SIZE)`。
- 同文件 `DecodeHistoricalJsonBlocks` 先验证 gzip 布局，再调用 `blocks_to_json_table`，并继续校验历史标记、谓词列和来源元数据。
- `dump_test.rs` 直接调用生成、恢复和块编解码入口；`stats_read_writer_test.rs` 再验证旧版本提升。
- 当前仓库未发现 `table_historical_stats_to_json` 的 Rust 外部调用者，也未发现 `generate_json_table_from_stats` / `table_stats_from_json` 的生产调用者；它们目前是 crate 的公开迁移 API，并由独立测试覆盖，不能据此声称已接入完整生产主链。

## 错误处理与边界

所有可失败公开入口统一返回同 crate 的字符串包装 `Error`。主要拒绝条件包括：零 `block_size`、空 blocks、非法 UTF-8、JSON 外壳不精确、非法 hex、payload 截断或长度溢出、非法历史标记、payload 尾随字节，以及 SQL 执行错误。

gzip 解码只接受方法 8 且固定头部布局下的未压缩 DEFLATE block；它检查保留位、LEN/NLEN 互补、CRC32、ISIZE 和 member 完整消费，拒绝压缩 block 与尾随字节。`json_aster_unit_test.rs::gzip_and_binary_payload_must_be_fully_consumed` 覆盖尾随 member 字节、损坏 ISIZE 与被篡改 payload；`dump_test.rs::block_conversion_rejects_invalid_boundaries` 覆盖零块长、空 blocks 和缺块。

`Cursor::option_string` 只把标记 0 解释为 `None`，其他字节均解释为 `Some`，不同于历史布尔标记的严格 0/1 校验。历史查询的 `Row::int/uint/bytes` 类型不匹配时会返回零或空值（定义见 `stats_read_writer.rs`），所以 `SqlStore` 实现必须保持查询列型与顺序契约。另一个实际边界是 meta 查询返回版本但计数行为空时不会报错，而是保留 payload 原计数；这与 Go 版本直接索引首行的行为不同。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务，也不持有跨调用资源。所有缓冲区、游标和中间字符串均在函数返回时释放；`check_cancelled` 以 `FnMut` 由调用者提供，按项同步执行。

`SqlStore: Send + Sync` 允许存储实现被并发环境共享，但 `table_historical_stats_to_json` 自身串行发出四次查询，没有建立事务或快照句柄。两个“最新版本”查询及后续数据查询能否看到一致视图取决于上层 `SqlStore`/会话事务语义，本文件不提供额外原子性保证。

Rust 的 `gzip_store` 每次新建 `Vec`，不使用共享压缩器；因此没有池化对象的归还问题。相对地，Go 对照实现使用 gzip reader/writer pool 和内存 tracker，这些资源生命周期不能套用到当前 Rust 代码。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/statistics/handle/storage/json.go`。公开职责一一对应 `GenJSONTableFromStats`、`TableStatsFromJSON`、`JSONTableToBlocks`、`BlocksToJSONTable` 和 `TableHistoricalStatsToJSON`，并保留以下核心语义：逐项取消检查、旧版本有数据项提升为 stats version 1、空 blocks 报 `Block empty error`、按快照选择最新历史版本、按 `seq_no` 重组以及用 meta 计数覆盖历史 payload。

当前 Rust 并非 Go 的完整等价实现：Go 在导出时转换真实列类型、统计内存并释放 FM sketch/直方图资源；导入时按 `TableInfo` 名称匹配列/索引、恢复字段类型和存在性映射。Rust 只克隆简化 `TableStats` 并重绑定 ID。Go 使用标准 JSON 和通用 gzip 压缩及对象池；Rust 使用私有二进制+hex JSON，并只支持未压缩 DEFLATE。Go 的谓词列来自 `TableItemID/ColStatsTimeInfo`，Rust 使用简化 `HashMap<i64, ...>` 且主动排序。

Go 测试 `dump_test.go::TestLoadPredicateColumns` 验证谓词列使用信息落库，`TestJSONTableToBlocks` 验证 JSON/gzip 往返，`TestLoadStatsFromOldVersion` 验证旧格式空统计保持未初始化。Rust 对应测试集中在 `dump_test.rs`、`json_aster_unit_test.rs` 和 `stats_read_writer_test.rs`，但没有直接覆盖本文件的历史 SQL 查询入口。

## 扩展指南

新增顶层或列级字段时，必须同步修改 `encode_table` 与 `decode_table`，并保持相同字段顺序；涉及 map 时继续显式排序。格式没有版本标签，直接插入字段会破坏旧 payload，因此实际演进应先设计版本/兼容分支，并补充旧数据读取和新旧往返测试。

扩大 gzip 支持时应修改 `gzip_store`/`gunzip_store` 及 `handle.rs::validate_historical_gzip_member` 的共同契约，不能只改一端。修改分块逻辑必须维持块顺序和 `MAX_COLUMN_SIZE` 调用约束。修改历史 SQL 时要同步检查 Go `TableHistoricalStatsToJSON` 的版本选择、计数覆盖和空结果语义，并为 Rust 增加独立 mock `SqlStore` 测试。

测试应继续放在独立文件：常规转换/兼容场景放 `pkg/statistics/handle/storage/dump_test.rs`，编码损坏和确定性边界放 `json_aster_unit_test.rs`；不要把测试嵌入 `json.rs`。若要接入真实表元数据、类型转换或内存控制，应在任务范围内明确补齐上层适配，而不能用当前简化结构冒充 Go 完整能力。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、目标目录 25 个文件；`files --filter pkg/statistics/handle/storage` 确认目标、模块和独立测试；`node --file pkg/statistics/handle/storage/json.rs --offset 1 --limit 500` 读取目标 487 行与 29 个符号；`node --file pkg/statistics/handle/handle.rs --offset 760 --limit 140` 确认生产编解码调用。精确 `callers/callees` 查询在限定时间内未返回可用输出，调用边以引用搜索补齐。
- 源码与 crate：`pkg/statistics/handle/storage/json.rs`、`lib.rs`、`Cargo.toml`、`stats_read_writer.rs`。
- Rust 调用与测试：`pkg/statistics/handle/handle.rs`、`storage/dump_test.rs`、`storage/json_aster_unit_test.rs`、`storage/stats_read_writer_test.rs`。
- Go 对照与测试：`pkg/statistics/handle/storage/json.go`、`pkg/statistics/handle/storage/dump_test.go`。
- 人工复核重点：公开/私有符号、二进制字段顺序、确定性排序、错误分支、历史查询顺序、Rust 与 Go 的已知差异，以及当前未接线的公开入口均按代码事实描述，未把预期架构写成现状。

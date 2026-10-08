# `pkg/statistics/handle/storage/read.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-storage` crate。crate 入口
`pkg/statistics/handle/storage/lib.rs` 以 `pub mod read` 声明模块，并通过
`pub use read::*` 将本文件的公开函数重导出。它位于统计信息读路径的存储边界：上层
`StatsReadWriter` 从 `StatsHandler` 取得 `SqlStore`，本文件把对
`mysql.stats_*` 系统表的查询结果组装为 `TableStats`、`ColumnStats`、
`Histogram`、`Bucket` 和 `TopNItem`（这些类型定义在同 crate 的
`stats_read_writer.rs`）。

`pkg/statistics/handle/storage/Cargo.toml` 将该目录定义为独立 crate，并以
`package.metadata.porting.go-package = "pkg/statistics/handle/storage"` 标明 Go
对照包。当前 manifest 的依赖全部位于 `cfg(any())` 下；本文件实际只使用 crate
内的简化存储抽象和统计结构，没有直接引入外部 crate。

## 核心职责

本文件承担四类只读操作：

1. 从 `mysql.stats_meta` 读取表的行数、修改计数和版本；
2. 从 `stats_histograms`、`stats_buckets`、`stats_top_n` 和
   `stats_fm_sketch` 重建列或索引的分布统计；
3. 检查某个分区或分区列的直方图记录是否存在；
4. 以可选的既有缓存为基准，组装并返回完整 `TableStats`。

所有数据库访问都经 `SqlStore::execute` 完成。函数不拥有会话或事务，也不修改
系统表；`stats_meta_count_and_modify_count` 仅在调用者要求时把
`FOR UPDATE` 加入查询，由外层事务提供锁语义。

## 主要符号

- `stats_meta_count_and_modify_count(store, table_id, for_update)`：返回
  `(count, modify_count, is_null)`。无 `stats_meta` 行时返回 `(0, 0, true)`；
  `for_update` 为真时读取行锁。
- `histogram_from_storage(store, table_id, is_index, hist_id, priority)`：先读取
  直方图元数据，再按 `bucket_id` 读取桶。元数据缺失返回 `Ok(None)`；优先级
  `1/-1/其他` 分别生成 `high_priority`、`low_priority` 或无提示的桶查询。
- `cmsketch_and_top_n_from_storage(...)`：始终读取 TopN；仅
  `stats_version <= 1` 时读取 `cm_sketch`，空字节被视为无 CMSketch。
- `fm_sketch_from_storage(...)`：读取 `hex(value)`，存在首行时经 `decode_hex`
  恢复原始字节。
- `decode_hex(value)`：私有兼容函数。仅对非空、偶数长度且全部为 ASCII 十六进制
  字符的载荷解码；其他载荷原样复制，兼容直接返回 blob 的 `SqlStore`。
- `check_partition_stats(store, table_id, is_index, hist_id)`：查询直方图存在性。
  `hist_id = None` 检查分区整体，`Some(id)` 检查特定列/索引；缺失时分别产生
  `partition stats missing` 或 `partition column stats missing`。
- `table_stats_from_storage(store, table_id, snapshot, existing)`：本文件的组合入口，
  从 meta 和所有 histogram 身份构造表统计。
- `stats_meta_by_table_id(store, table_id, snapshot)`：读取
  `(version, modify_count, count)`；无行返回全零。

本文件没有模块级常量、类型、trait、`impl` 或条件编译项。

## 执行流程

`table_stats_from_storage` 的主流程如下：

1. 当 `snapshot == 0` 时读取当前 `stats_meta` 行；非零时增加
   `version <= snapshot` 条件。
2. meta 缺失时直接返回传入的 `existing`；若没有既有值，则以 `table_id` 创建默认
   `TableStats`。这条分支不会查询 histogram 附属表。
3. meta 存在时复用或新建表对象，但先清空 `columns`、`indices` 并把表级
   `stats_version` 归零，保证一次完整存储快照不会保留已被 DDL 删除的缓存项。
4. 写入 meta 的 `version`、`modify_count` 和 `count`，再读取该表的所有
   `(hist_id, is_index, stats_ver)`。
5. 对每个身份调用 `histogram_from_storage`。若对应 meta 在第二次查询时已不存在，
   跳过该项；否则继续加载 TopN/CMSketch 和 FM Sketch，构造 `ColumnStats`。
6. 表级 `stats_version` 取所有已装载项版本的最大值，并根据 `is_index` 把结果放入
   `indices` 或 `columns`，键为十进制 `hist_id` 字符串。

`histogram_from_storage` 将 `stats_buckets.count` 解释为增量值：循环中持续累加
`total`，把累计值写入每个 `Bucket.count`。因此输出桶的 count 是累计计数，而
`repeat`、上下界和桶内 NDV 则逐行保留。

## 数据与状态

本文件自身不保存全局或线程局部状态。输入状态由 `&dyn SqlStore` 和参数提供，输出
都是拥有所有权的 Rust 值。`Row::int/uint/bytes` 的转换规则来自
`stats_read_writer.rs`：整数类型可在有符号和无符号之间转换，`Text` 可作为 UTF-8
字节读取，类型不匹配则得到零或空字节。这意味着加载正确性依赖 `SqlStore` 按查询
列顺序返回预期 `Value` 变体。

`existing: Option<TableStats>` 是唯一的缓存输入。meta 缺失时它原样返回；meta
存在时保留同一表对象的基础身份，但列、索引和表级统计版本都会重新构造。直接上游
`StatsReadWriter::table_stats_from_storage` 在调用前从
`Mutex<HashMap<i64, TableStats>>` 克隆缓存，调用后再把新值写回；锁不会跨越本文件的
SQL 查询持有。

## 依赖与调用关系

RustCodeGraph 将 `read.rs` 标记为被 `read_test.rs` 和
`stats_read_writer_test.rs` 使用，并确认本文件内部调用边：
`table_stats_from_storage` 调用 `histogram_from_storage`、
`cmsketch_and_top_n_from_storage` 和 `fm_sketch_from_storage`，后者调用
`decode_hex`。

源码接线进一步确认：

- `StatsReadWriter::stats_meta_count_and_modify_count` 调用同名自由函数并丢弃
  `is_null` 标志；
- `StatsReadWriter::table_stats_from_storage` 从缓存取基准，调用本文件入口，再更新
  缓存；
- `lib.rs` 对所有公开读函数进行 crate 级重导出；
- 所有下游 I/O 最终落在 `SqlStore::execute(&str) -> Result<Vec<Row>, Error>`。

精确 RustCodeGraph `callers/callees` 查询没有返回更多静态边，因此没有把 Go 调用者或
名称相同的其他 Rust trait 方法误认为本文件的直接调用者。

## 错误处理与边界

所有 `SqlStore::execute` 错误都通过 `?` 原样返回 `Error`，组合加载在任何已执行的
子查询失败时整体终止，不返回部分 `TableStats`。明确的“无数据”约定并不等同于错误：
meta 计数接口用 `is_null` 标识，直方图用 `None`，FM Sketch 用 `None`，表 meta
三元组用全零。只有 `check_partition_stats` 把缺失转成业务错误。

需要注意的边界包括：

- 查询通过 `format!` 拼接数值参数；当前参数均为整型或内部生成的固定片段，没有文本
  输入，但新增字符串条件时不能照搬这种方式；
- `histogram_from_storage` 的元数据查询固定写有 `high_priority`，传入的
  `priority` 只影响桶查询；
- correlation 只有在第六列确为 `Value::Float` 时保留，否则静默使用 `0.0`；
- `decode_hex` 会把任何形式合法的 ASCII hex 当作编码文本，若某个存储实现直接返回的
  原始 blob 恰好全是 hex 字符且长度为偶数，它也会被解码；
- snapshot 过滤只出现在 meta 查询，随后 histogram、bucket、TopN 和 FM Sketch
  查询没有显式 snapshot 参数。本抽象是否提供同一 MVCC 快照取决于外层 `SqlStore`
  实现，本文件本身不保证。

## 并发与资源生命周期

`SqlStore` 要求 `Send + Sync`，所以这些无内部可变状态的函数可被并发调用；本文件不
创建线程、异步任务、通道或后台资源。每次调用同步执行 SQL，查询结果和组装对象在
函数返回时按所有权移动。

`FOR UPDATE` 只是一段 SQL 后缀：锁的获取、事务范围、提交和回滚均属于调用者和
`SqlStore`。本文件也不持有 `StatsReadWriter` 的缓存互斥锁；上游分别在 SQL 调用前后
短暂加锁，因此慢查询不会直接占用该 mutex，但并发加载同一表可能基于相同旧缓存分别
完成，最后写入者覆盖先前缓存值。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/storage/read.go`。Rust 保留了以下核心语义：

- `StatsMetaCountAndModifyCount` 的无行标志和可选 `FOR UPDATE`；
- histogram 桶按增量 count 累积、按 `bucket_id` 排序；
- stats version 2 及以上不读取 CMSketch；
- FM Sketch、分区统计存在性检查、表级统计组合和 snapshot 为零表示当前行；
- `StatsMetaByTableIDFromStorage` 的缺失行返回零值。

但当前 Rust API 是有意简化的移植面，不能视为 Go 实现完全等价。Go 版本使用真实
`sessionctx`、参数绑定和 `execRowsAtSnapshot`，并处理字段类型/排序规则转换、统计
解码对象、内存 tracker、SQL kill 信号、按 schema 过滤已删除列、懒加载状态及 lease
策略；Rust 使用裸字节边界、简化 `SqlStore` 和精简统计结构。Go 的
`TableStatsFromStorage` 还根据是否有 histogram 选择写时复制级别，而 Rust 在 meta
存在时总是清空并重建两个 map。扩展 Rust 行为时应逐项对照 Go 函数，而不能只依据
相似函数名假定行为已经覆盖。

## 扩展指南

新增读取字段或统计载荷时，优先在本文件对应的单一读取函数中扩展，并同步检查
`stats_read_writer.rs` 的 `Value`、`Row` 和输出结构是否能无损表达该字段。修改表级
组装规则时应从 `table_stats_from_storage` 接线，并维护“存储完整快照不得遗留已删除
histogram”这一不变量。

建议把回归测试放在独立的 `pkg/statistics/handle/storage/read_test.rs`；涉及缓存替换或
读写门面的场景放在 `stats_read_writer_test.rs`，不要把测试内嵌到生产源文件。至少应
覆盖：meta 缺失、`FOR UPDATE` SQL、三种优先级、桶累计、stats version 1/2 的
CMSketch 分支、空/合法/非法 hex、分区两种缺失错误、snapshot 非零过滤、子查询错误
传播以及并发缓存更新策略。

兼容风险主要在 SQL 形状、`Row` 列序、Go/Rust 类型解码差异和 snapshot 一致性；性能
风险主要来自每个 histogram 当前需要三次附属查询，形成随列/索引数量线性增长的往返。
若进行批量读取优化，应保留逐项错误语义、稳定的桶顺序和表级最大 stats version。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph 状态：索引包含 11,467 个文件；目标 `read.rs` 已索引为 238 行、9 个
  符号节点，并报告两个测试使用者；`explore/node/query/callers/callees` 用于核对符号、
  源码和调用边；
- 生产源码：`pkg/statistics/handle/storage/read.rs`、
  `pkg/statistics/handle/storage/stats_read_writer.rs`、
  `pkg/statistics/handle/storage/lib.rs`；
- crate 边界：`pkg/statistics/handle/storage/Cargo.toml`；
- Go 对照：`pkg/statistics/handle/storage/read.go` 和
  `pkg/statistics/handle/storage/stats_read_writer.go`；
- 独立 Rust 测试：`read_test.rs` 的 `canonical_storage_row_conversions_match_sql_value_coercions`
  与 `stats_meta_snapshot_zero_reads_current_row`，以及 `stats_read_writer_test.rs` 的
  `table_stats_load_drops_histograms_missing_from_storage`；
- 独立 Go 测试位置：`pkg/statistics/handle/storage/read_test.go` 与
  `stats_read_writer_test.go`，用于确认完整 Go 路径的测试边界。

结构验证要求文档恰有本文列出的十一个固定二级标题；本任务是纯文档分析，未运行
Cargo 或代码测试。人工复核重点为：文件存在原因、主执行链、无数据与错误边界、缓存
替换不变量、Go 移植差异和安全扩展位置均能追溯到上述符号或文件。

# `pkg/statistics/handle/cache/internal/testutil/testutil.rs`

源文件：[`testutil.rs`](./testutil.rs)

## 文件定位

该文件实现 crate `astersql-statistics-handle-cache-internal-testutil` 的 mock 统计表构造逻辑。crate 入口 `pkg/statistics/handle/cache/internal/testutil/lib.rs` 以 `mod testutil` 装载本文件，并通过 `pub use testutil::*` 再导出公开函数；因此使用方以 `cache_testutil::NewMockStatisticsTable` 或 `testutil::NewMockStatisticsTable` 调用，而不是直接依赖本文件的模块路径。

它位于统计信息缓存的 `internal/testutil` 边界，服务于 Rust 单元测试与基准：缓存 crate、LFU 子 crate 和 `handle/internal` 的测试依赖均在各自 `Cargo.toml` 中以路径依赖引入此 crate。它不在 SQL 请求、统计信息采集或缓存运行时主链上，也不会从存储读取真实统计信息；其作用是按可预测形状构造 `statistics::Table`，供缓存淘汰、刷新、相等性和内存成本等测试使用。

## 核心职责

- `NewMockStatisticsTable` 构造包含指定数量列和索引的 `Table`，并用三个布尔开关独立控制 CMSketch、TopN 和非空直方图载荷。
- `MockTableAppendColumn`、`MockTableAppendIndex` 向已有 mock 表追加一个带 CMSketch 的列或索引，用于验证表规模增长及缓存内存成本变化。
- `blob_type`、`mock_histogram`、`mock_top_n` 集中封装 Go helper 的默认载荷，使列和索引沿用完全相同的构造规则。
- 所有元数据都使用最小测试值：表及物理 ID、版本、统计版本为零，名称和索引列列表为空；本文件不尝试模拟真实 schema 或真实频率分布。

## 主要符号

- `fn blob_type() -> types::FieldType`：返回 `mysql::TypeBlob` 字段类型，仅供启用直方图时传给 `statistics::NewHistogram`。
- `fn mock_histogram(enabled: bool) -> Histogram`：启用时构造 ID 为 `0`、NDV 为 `10`、桶容量为 `1` 的 BLOB 直方图；关闭时仍返回一个 `Histogram` 值，但其字段类型和计数参数均为零值。函数是私有实现细节。
- `fn mock_top_n(enabled: bool) -> Option<statistics::TopN>`：关闭时返回 `None`；启用时创建容量为 `1` 的 TopN，并加入一条“空字节编码、频次 1”的记录。
- `pub fn NewMockStatisticsTable(columns: i32, indices: i32, withCMS: bool, withTopN: bool, withHist: bool) -> Table`：文件的主要公开入口。按 `1..=columns` 与 `1..=indices` 生成连续的 `i64` ID，将 `Column`/`Index` 写入 `table.HistColl`。
- `pub fn MockTableAppendColumn(table: &mut Table)`：以 `table.ColNum() + 1` 为 ID 追加列。新列有 CMSketch，无 TopN，使用关闭状态的零值直方图。
- `pub fn MockTableAppendIndex(table: &mut Table)`：以 `table.IdxNum() + 1` 为 ID 追加索引；载荷规则与追加列一致。

本文件没有常量、类型、trait、`impl`、异步函数或条件编译项。公开 API 的 Go 风格命名由 crate 根的 `#![allow(non_snake_case)]` 保留，以便与移植来源对齐。

## 执行流程

`NewMockStatisticsTable` 的执行顺序如下：

1. 调用 `Table::New(0, 0, 0)` 建立空表和空 `HistColl`。
2. 对列 ID `1..=columns` 循环。每轮按 `withCMS` 创建 `NewCMSketch(1, 1)`，按 `withTopN` 调用 `mock_top_n`，按 `withHist` 调用 `mock_histogram`；随后建立最小 `ColumnInfo`，用 `HistColl.SetCol` 按 ID 写入列。
3. 对索引 ID `1..=indices` 执行对称流程，建立最小 `IndexInfo`，并用 `HistColl.SetIdx` 写入索引。
4. 按值返回完整 `Table`；调用者通常再设置 `PhysicalID`、`Version`、`RealtimeCount` 等与具体测试有关的字段。

追加函数没有复用完整构造入口：它们读取当前 `ColNum`/`IdxNum`，用“当前数量 + 1”作为新 ID，再直接写入一个带 CMSketch 的最小对象。这与 Go helper 的追加行为一致，但隐含前提是现有 ID 与对象数量连续对应；若调用者手工制造稀疏 ID，新增 ID 可能覆盖已有项，因此这两个函数只适合其预期的 mock 表输入。

## 数据与状态

`Table` 是唯一被创建或修改的持久对象；本文件自身没有全局状态。`NewMockStatisticsTable` 返回所有权完整的表，追加函数通过独占的 `&mut Table` 原地修改调用者对象。

每个构造的 `Column` 和 `Index` 都拥有独立的 `CMSketch`、`TopN`、`Histogram` 与元数据实例，不在对象之间共享可变载荷。列额外设置 `IsHandle = false`；索引设置 `MVIndex = false`、`Unique = false`，两者的 `FMSketch` 均为 `None`。完整构造路径把 `StatsLoadedStatus` 设为 `NewStatsFullLoadStatus()`，追加路径则使用默认状态；这一区别来自当前 Rust/Go 移植结构，扩展时不应无意抹平。

三个开关彼此独立。例如 `withHist = false` 不会让 `Histogram` 变成 `Option::None`，而是保留零值直方图；`withTopN = true` 则一定产生一个含单条空编码记录的 `Some(TopN)`。同目录测试 `mock_table_without_payloads_uses_zero_value_histograms` 明确锁定了关闭载荷时的零值语义。

## 依赖与调用关系

直接下游依赖由 `pkg/statistics/handle/cache/internal/testutil/Cargo.toml` 声明：

- `statistics`（`astersql-statistics`）提供 `Table`、`Column`、`Index`、`Histogram`、CMSketch、TopN、元信息类型及集合写入 API。
- `types`（`astersql-types-datum`）提供 `FieldType` 与其零值。
- `mysql`（`astersql-parser-mysql`）提供 `TypeBlob` 类型常量。

RustCodeGraph 对本文件的索引列出 7 个符号，并给出 `pkg/statistics/handle/cache/bench_test.rs`、`pkg/statistics/handle/cache/statscache_test.rs`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache_test.rs`、`pkg/statistics/handle/internal/testutil_test.rs` 等消费文件。图结果还把 `pkg/statistics/runtime_stats_builder.rs` 列为使用方，但定向搜索表明那里只有同名局部变量 `blob_type`，不是本 crate 的调用边，因此不把它计入真实上游。源码内，`mock_histogram` 被三个公开构造/追加函数调用，`mock_top_n` 被 `NewMockStatisticsTable` 调用；`blob_type` 是 `mock_histogram(true)` 的下游。

可见的直接上游主要是测试与基准：缓存基准构造一列统计表后设置物理 ID/版本；`statscache_test.rs` 用完整载荷验证配额淘汰和刷新；LFU 测试用不同列/索引数驱动成本及淘汰场景；`handle/internal/testutil_test.rs` 用构造结果验证统计表相等逻辑。搜索未发现生产运行路径调用三个公开函数。

## 错误处理与边界

所有函数均返回普通值或 `()`，没有 `Result`，也不主动产生业务错误。底层构造与 `SetCol`/`SetIdx` 若存在 panic 条件，会直接向上传播；本文件不捕获或转换 panic。

- `columns` 或 `indices` 为 `0` 时对应循环为空，可构造零列/零索引表；`handle/internal/testutil_test.rs` 覆盖了 `(0, 0)`。
- 负数同样形成空的包含范围，不会报错；当前测试未专门锁定负数输入，因此它应视为构造语法的结果，而非承诺负数具有业务意义。
- 启用直方图时，直方图 ID 固定为 `0`，而集合键及 `ColumnInfo.ID`/`IndexInfo.ID` 是从 `1` 开始的对象 ID；同目录测试明确验证了该差异，调用者不能假设两者相等。
- `ColumnInfo.FieldType` 始终保留零值，即使直方图字段类型为 BLOB；这同样由 `mock_table_builds_and_appends_columns_and_indices` 验证。
- 追加函数按数量推导 ID，只保证连续、由 helper 构造且未被手工改写的表能安全追加；它们没有检查 ID 冲突或整数转换/加法溢出。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络资源。构造过程完全同步；临时载荷在循环中创建后移入 `Box<Column>`/`Box<Index>`，再由 `Table.HistColl` 持有。表离开作用域时由 Rust 所有权机制递归释放。

`NewMockStatisticsTable` 的返回值可由调用者包装进 `Arc<Table>`，LFU 与缓存测试正是如此，但共享和同步责任属于调用者与缓存实现。两个追加函数要求 `&mut Table`，编译期禁止在同一时刻经安全 Rust 对同一表并发追加；函数内部没有额外同步。构造成本随 `columns + indices` 线性增长，每个启用载荷都会按对象单独分配，适合小规模、确定性的测试夹具，不应作为大规模数据生成器或生产缓存加载器。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/cache/internal/testutil/testutil.go`。Rust 保留了三个公开函数名、参数顺序、列/索引的 1 起始 ID、CMSketch 参数 `(1, 1)`、TopN 的空编码/频次 1、直方图构造参数及追加函数的“当前数量 + 1”策略。

两种实现的表示形式有所不同：Go 用指针或 `nil` 表示可选 CMSketch/TopN，Rust 用 `Option`；Go 未启用直方图时依赖 `statistics.Histogram` 零值，Rust 通过 `NewHistogram` 显式构造对应零值形态；Go 的 `model.ColumnInfo`/`IndexInfo` 未填写字段自然为零值，Rust 必须列出结构体字段并显式填写空值。Rust 完整构造路径同时显式设置 `FMSketch`、`PhysicalID`、`StatsVer` 等字段，以复现 Go 默认值。

Go 注释称每个列和索引消耗 4 字节，这是原 helper 面向 Go 内存成本测试的说明；Rust 实际成本由 Rust 数据结构的 `MemoryUsage()` 计算，不能把该注释当作 Rust 固定布局保证。Rust 单测只断言总内存占用为正，没有承诺固定字节数。

## 扩展指南

- 若新增可选统计载荷，优先在 `NewMockStatisticsTable` 的列、索引两条对称路径同时接入，并在参数增加前核对 Go helper 是否已有对应语义；避免只修改一侧造成缓存成本测试偏差。
- 若改变直方图或 TopN 默认值，应同步检查私有 `mock_histogram`/`mock_top_n`、Go 对照文件以及 `pkg/statistics/handle/cache/internal/testutil/testutil_test.rs` 的开/关载荷断言。
- 若改变追加规则，应同步覆盖列与索引，并考虑增加稀疏 ID 或冲突场景测试；测试逻辑继续放在独立的 `testutil_test.rs`，不要内嵌进生产源文件。
- 若新增公开 helper，需要确认 `lib.rs` 的通配再导出是否仍合适，并在所有消费 crate 的 `Cargo.toml` 边界内使用，避免把测试工具接入生产运行链。
- 兼容风险主要是破坏 Go/Rust 夹具等价性以及改变 `MemoryUsage`，从而影响 LFU 配额/淘汰测试；性能风险来自按列和索引重复分配大载荷。扩展后至少应复核同目录单测、`statscache_test.rs`、`lfu_cache_test.rs` 和 `handle/internal/testutil_test.rs` 的相关场景。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含目标文件；`files --filter pkg/statistics/handle/cache/internal/testutil` 列出 `lib.rs`、Go/Rust 实现与 Rust 独立测试；`node --file .../testutil.rs --offset 1 --limit 320` 读取了 154 行完整实现；`query` 核对了 `NewMockStatisticsTable`、`MockTableAppendColumn`、`MockTableAppendIndex`、`mock_histogram`、`mock_top_n` 的定义。精确 `callers`/`callees` 子命令未输出文本，且文件级结果包含一个因 `blob_type` 同名产生的伪引用，因此真实调用边由定向符号搜索和消费源码交叉核验。
- 源码与 crate 边界：`pkg/statistics/handle/cache/internal/testutil/testutil.rs`、`pkg/statistics/handle/cache/internal/testutil/lib.rs`、`pkg/statistics/handle/cache/internal/testutil/Cargo.toml`。
- Go 对照：`pkg/statistics/handle/cache/internal/testutil/testutil.go`。
- 独立 Rust 测试：`pkg/statistics/handle/cache/internal/testutil/testutil_test.rs`；相关消费证据来自 `pkg/statistics/handle/cache/statscache_test.rs`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache_test.rs`、`pkg/statistics/handle/cache/bench_test.rs`、`pkg/statistics/handle/internal/testutil_test.rs`。
- Cargo 上游边界：`pkg/statistics/handle/cache/Cargo.toml`、`pkg/statistics/handle/cache/internal/lfu/Cargo.toml`、`pkg/statistics/handle/internal/Cargo.toml` 均声明该 testutil crate 的路径依赖。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务规定的命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核源码链接、符号、流程、边界和安全扩展入口。

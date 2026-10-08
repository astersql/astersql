# `pkg/testkit/stats_fixture.rs`

## 文件定位

本文件属于 `astersql-testkit` crate，是测试侧的 TiDB JSON 表统计 fixture 适配器。crate 根模块 `pkg/testkit/lib.rs` 以 `pub mod stats_fixture` 挂载它，并以 `pub use stats_fixture::LoadTableStats` 重导出唯一公开入口，因此调用方通常使用 `astersql_testkit::LoadTableStats`，而不是直接依赖子模块。

它位于“测试已创建表的元数据”与“规划器可读取的内存统计缓存”之间：输入是一个 JSON 文件路径和已初始化的 `astersql_domain::Domain`，输出不是新对象，而是把解析出的 `astersql_statistics_handle::TableStats` 写入该 `Domain` 的统计 handle。已确认的上游包括 `pkg/planner/core/casetest/ch/ch_test.rs::run_ch_explain_case` 和 `pkg/planner/core/casetest/tpch/tpch_test.rs::load_stats`；这些测试先创建表，再加载统计，随后执行带代价或选择率判断的规划测试。它不是生产 SQL 请求链上的自动统计导入接口。

`pkg/testkit/Cargo.toml` 声明 crate 名为 `astersql-testkit`，本文件直接使用其中的 `astersql-domain`、`astersql-statistics-handle`、`base64` 和 `serde_json` 依赖。文件没有 feature gate 或条件编译项；独立测试则由 `pkg/testkit/lib.rs` 在 `#[cfg(test)]` 下通过 `stats_fixture_test.rs` 挂载。

## 核心职责

- `LoadTableStats` 读取调用者给出的路径，将文件反序列化为通用 `serde_json::Value`，并校验顶层数据库名、表名、行数、修改行数和版本等必需字段。
- 它通过 `Domain::table_by_name` 找到已经存在的 `TableInfo`，再按 JSON 对象键与 `ColumnInfo.Name.L`、`IndexInfo.Name.L` 匹配，将名称转换为运行时统计缓存使用的列 ID、索引 ID 和物理表 ID。
- 它将列和索引的直方图 bucket、NDV、空值数、总大小、相关度、统计版本及 TopN 解码为 `TableStats`、`ColumnStats`、`IndexStats` 和 `Bucket`，最后调用 `StatsCache::put` 按物理表 ID 写入或覆盖缓存。
- 它刻意只实现当前 Rust 规划测试需要的内存统计子集：不会创建表，不会把统计持久化到系统表，也不会处理 Go 完整导入链中的分区、扩展统计、谓词列使用记录、历史统计语义或压缩 fixture 回退。

## 主要符号

- `field(value, name) -> TestResult<&Value>`：所有必需 JSON 字段的共同入口。字段不存在时返回包含字段名的 `TestError`。
- `string_field`、`i64_field`、`u64_field`、`f64_field`：在 `field` 基础上做类型提取。它们不做字符串到数字等宽松转换，因此 JSON 类型必须与目标类型一致；版本和 TopN 计数还必须是非负的无符号整数。
- `decode_bound(value, name) -> TestResult<Vec<u8>>`：把 `lower_bound` 或 `upper_bound` 的标准 Base64 字符串还原为统计编码字节，并给解码失败增加字段上下文。
- `histogram(value) -> TestResult<(i64, Vec<Bucket>)>`：读取子对象 `histogram`。缺失或显式为 `null` 时返回 `(0, [])`；存在时读取 `ndv`，并把可用的 `buckets` 数组逐项转换为 `Bucket { count, repeats, lower, upper, ndv }`。`buckets` 缺失或不是数组会按空数组处理。
- `top_n(value) -> TestResult<Vec<(Vec<u8>, u64)>>`：沿 `cm_sketch.top_n` 读取数组，将每项的 Base64 `data` 和无符号 `count` 转为运行时 TopN。`cm_sketch`、`top_n` 缺失、为 `null` 或类型不匹配时均按空集合处理。
- `pub fn LoadTableStats(path: impl AsRef<Path>, domain: &Domain) -> TestResult`：唯一公开 API。泛型路径参数允许 `Path`、`PathBuf`、字符串等调用方式；成功值为 `()`，失败统一收敛到 crate 的 `TestError`。

## 执行流程

1. `LoadTableStats` 通过 `fs::read` 一次性读取整个文件，再用 `serde_json::from_slice` 构造 JSON 值；两类错误都附带实际路径。
2. 它提取 `database_name` 和 `table_name`，调用 `Domain::table_by_name` 从当前 info schema 获取 `TableInfo`。因此调用者必须先在同一 `Domain` 中创建目标库表。
3. 它读取 `count`、`modify_count` 和 `version`，用表 ID 初始化非伪、已初始化的 `TableStats`。`realtime_count` 与 `analyze_count` 都取 `count`，分析版本、最后直方图版本都取顶层 `version`，列/索引映射开始为空。
4. 对顶层 `columns` 对象逐项处理：先按小写名称匹配表元数据中的列，再解析直方图、`stats_ver`、`null_count`、`tot_col_size`、`last_update_version` 和 `correlation`。平均大小在 `count > 0` 时为 `tot_col_size / count`，否则固定为 `0.0`，避免除零。列的 `analyzed_or_synthesized` 在统计版本、NDV 或空值数任一非零时成立。
5. 对顶层 `indices` 对象执行相同的名称到 ID 映射，构造 `IndexStats`。索引的 `analyzed` 只由 `stats_ver != 0` 决定；`fully_loaded` 固定为 `true`，`cms_loaded` 固定为 `false`，但 TopN 仍从 JSON 解码保存。
6. 每处理一列或索引，表级 `stats_version` 都取当前值与对象 `stats_ver` 的最大值。全部解析完成后才取得统计 handle 的互斥锁，并通过 `cache_mut().put(stats)` 按物理表 ID 一次性替换缓存条目。

## 数据与状态

函数先在局部变量中构造 JSON 值和一份完整 `TableStats`，直至最后一步才修改共享状态。`TableStats.physical_id` 来自当前 `TableInfo.ID`，而不是相信 fixture 中的外部 ID；列和索引同样使用当前 schema 的对象 ID。这让同一份按名称描述的 fixture 能绑定到本次测试实际分配的 ID。

缓存数据的关键映射如下：顶层 `count` 同时成为实时行数和分析时行数；顶层 `version` 成为表版本、最后分析版本和最后直方图版本；对象级 `stats_ver` 的最大值成为表级统计版本。列、索引以 `HashMap<i64, ...>` 保存，键分别为列 ID 和索引 ID。直方图边界与 TopN 数据保持为解码后的原始编码字节，不在本文件中解释为 SQL 类型值。

若同一物理表重复加载，`StatsCache::put` 的 `HashMap::insert` 语义会整体覆盖旧 `TableStats`。本文件不会合并旧列/索引统计，也不会更新统计后端或系统表。JSON 中的 `fm_sketch` 没有导入，运行时字段固定为空；列 `field_type` 固定为 `0`，`pre_scalar_ready` 固定为 `false`。

## 依赖与调用关系

上游公开边界是 `pkg/testkit/lib.rs` 的重导出。RustCodeGraph 将该根模块识别为 `LoadTableStats` 的导入者；由于跨 crate 重导出调用没有完整落入调用边，源码搜索补充确认了 CH 与 TPCH case 测试直接调用 `astersql_testkit::LoadTableStats`，而 `pkg/testkit/stats_fixture_test.rs` 通过 `super::LoadTableStats` 覆盖本 crate 内路径。

下游主链为：`LoadTableStats` → `fs::read` / `serde_json::from_slice` → `Domain::table_by_name` → 本文件的 `histogram`、`top_n` 及类型提取函数 → `Domain::stats_handle` → `Handle::cache_mut` → `StatsCache::put`。`Domain::table_by_name` 查询当前 info schema；`Domain::stats_handle` 返回共享的 `Arc<Mutex<Handle<DomainStatsBackend>>>`；`StatsCache::put` 按 `physical_id` 写入内存 `HashMap`。

`pkg/testkit/Cargo.toml` 中 `astersql-statistics-handle` 提供目标统计结构和缓存，`astersql-domain` 提供 schema/handle 边界，`serde_json` 提供无 schema JSON 解析，`base64` 负责 Go JSON 中二进制边界及 TopN 数据的编码兼容。该模块没有网络、KV 或异步运行时依赖。

## 错误处理与边界

所有显式失败均转换为 `TestError` 并由 `?` 立即向上传播。错误消息区分文件读取、JSON 解码、缺字段、字段类型不符、Base64 解码、目标表不存在、目标列/索引不存在和 mutex 中毒。列或索引名称无法匹配时，消息同时包含 fixture 名称和目标 `database.table`，便于识别 schema 与 fixture 漂移。

必需边界较严格：顶层 `columns`、`indices` 必须存在且为对象；每个列/索引必须具有数值统计字段，即使其直方图为空。相比之下，`histogram`、`histogram.buckets` 和 `cm_sketch.top_n` 采取缺失即空的兼容策略，`stats_ver` 缺失或不是整数时默认为 `0`。负数不能通过 `u64_field`，非法 Base64 不能静默跳过。

共享缓存具有“解析阶段不产生部分写入”的性质：任何文件、schema 或对象解析错误都发生在加锁和 `put` 之前，旧缓存保持不变。需要注意的是，本文件没有检查统计数值之间的业务一致性，例如 bucket 累计数是否单调、NDV 是否超过行数或上下界是否有序；它也没有拒绝未使用的 JSON 字段。大文件会被完整读入并构造完整内存对象，没有流式或大小限制。

## 并发与资源生命周期

文件读取和 JSON/统计对象构造都同步执行，不启动线程、任务或通道。路径只在调用期间借用，文件内容由 `fs::read` 直接读取后关闭，不持有文件句柄。

共享资源只在提交阶段接触：`Domain::stats_handle()` 克隆 `Arc`，随后取得 `Mutex` guard，在 `cache_mut().put(stats)` 完成后随表达式结束释放。锁不覆盖磁盘 I/O、JSON 解析、Base64 解码或 schema 遍历，因此慢 fixture 不会长期占用统计锁；锁若已中毒则返回错误，不尝试恢复。并发加载同一物理表时，每个调用独立构造对象，最终结果由最后一次成功取得锁并执行 `put` 的调用覆盖，函数本身不提供版本比较或冲突检测。

`TableInfo` 以 `Arc` 形式在解析期间持有，名称到 ID 的解析基于调用时获取的 schema 快照。函数没有显式处理同时发生的 DDL；测试调用者应在 schema 稳定、建表完成后加载 fixture。

## 与 Go 版本的对应关系

Go 对照入口是 `pkg/testkit/testkit.go::LoadTableStats`。两者共同目标是把 TiDB JSON 统计加载进给定 `Domain`，并都向调用者返回错误；Rust 的公开名称还保留了 Go 风格的 `LoadTableStats`，便于移植测试保持调用意图。

实现路径并非完全等价。Go 将参数视为 `testdata` 下的文件名，普通文件不存在时调用 `loadTableStatsFromZip` 从 `testdata/stats.zip` 查找；Rust 将参数视为完整/相对路径并直接读取，没有 zip 回退，因此 Rust case 测试显式用 `CARGO_MANIFEST_DIR/testdata` 拼路径。Go 反序列化为 `statisticsutil.JSONTable`，再调用 `StatsHandle.LoadStatsFromJSON(context.Background(), dom.InfoSchema(), statsTbl, 0)`；Rust 用 `serde_json::Value` 手工提取当前测试需要的字段，并只覆盖内存 `StatsCache`。

因此 Rust 已对应基本表、列、索引、直方图 bucket 和 `cm_sketch.top_n`，但不能宣称覆盖 Go 完整导入语义。当前代码没有处理 `partitions`、`ext_stats`、`predicate_columns`、`is_historical_stats`、FM sketch 或 CMS 默认值，也不执行 Go 导入函数可能承担的持久化、缓存更新细节与格式兼容。`pkg/testkit/stats_fixture_test.rs::load_table_stats_populates_real_histograms_and_indexes` 验证了表计数、列/索引数量、NDV 和 Base64 边界字节，但没有覆盖上述差异或错误分支。

## 扩展指南

新增 JSON 字段时，应先确认对应运行时结构位于 `pkg/statistics/handle/handle.rs`，然后在 `LoadTableStats` 的局部构造阶段完成解析，保持“全部成功后一次写缓存”的提交边界。若是列/索引共享字段，优先复用或扩展 `histogram`、`top_n` 和类型提取函数，避免两条循环产生不同格式语义。

若要追平 Go 的 zip 回退、分区统计或完整 `LoadStatsFromJSON` 行为，应把它视为兼容性扩展而非简单字段追加：需要核对 Go `pkg/testkit/testkit.go::LoadTableStats`、`loadTableStatsFromZip` 以及 statistics handle 的真实导入 API，并明确是否仍只修改内存缓存。特别是分区统计可能产生多个物理 ID，不能沿用当前“一文件只提交一份 `TableStats`”的假设。

测试必须继续放在独立的 `pkg/testkit/stats_fixture_test.rs`，不要嵌入生产文件。字段扩展至少应增加成功映射断言；解析规则变更还应覆盖缺字段、类型错误、非法 Base64、未知列/索引、零行数和重复覆盖。并发行为若改变，应验证锁持有范围和同表写入策略。兼容性风险主要是接受/拒绝 JSON 格式与 Go 不一致；正确性风险是名称到 ID 映射或版本字段映射错误影响计划选择；性能风险来自大 fixture 的全量读取、复制和在缓存中保留所有 bucket/TopN。

## 验证依据

- 目标源码：`pkg/testkit/stats_fixture.rs`，逐行核对 `field`、`string_field`、`i64_field`、`u64_field`、`f64_field`、`decode_bound`、`histogram`、`top_n` 和 `LoadTableStats`。
- crate 与模块边界：`pkg/testkit/Cargo.toml` 的依赖声明，以及 `pkg/testkit/lib.rs` 的模块挂载、公开重导出和 `#[cfg(test)]` 独立测试挂载。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`node pkg/testkit/stats_fixture.rs::LoadTableStats` 给出对本文件解析辅助函数的调用边及 `pkg/testkit/lib.rs` 导入边；`node` 还核对了 `pkg/domain/domain.rs::table_by_name`、`stats_handle` 和 `pkg/statistics/handle/handle.rs` 中 `TableStats`、`ColumnStats`、`IndexStats`、`Bucket`、`StatsCache::put`、`Handle::cache_mut` 的实际定义。图中跨 crate 重导出调用边不完整，因此调用点另以源码搜索确认。
- Rust 测试：`pkg/testkit/stats_fixture_test.rs` 创建真实库表，写入临时 JSON，加载后检查缓存中的非伪/初始化状态、行数、修改数、列/索引数量、NDV 和解码后的 bucket 边界，并删除临时文件。
- 实际上游：`pkg/planner/core/casetest/ch/ch_test.rs` 与 `pkg/planner/core/casetest/tpch/tpch_test.rs` 均从各自 `testdata` 目录拼出路径后调用公开重导出入口。
- Go 对照：`pkg/testkit/testkit.go::LoadTableStats` 与 `loadTableStatsFromZip`，用于确认 testdata 路径、zip 回退、强类型 `JSONTable` 和 `LoadStatsFromJSON` 语义差异；仓库中未找到专门命名为 `TestLoadTableStats` 的 Go 单元测试，Go 规划 case 测试是主要调用证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务文件给出的命令验证目标文件存在且固定二级标题恰好为 11 个，并人工复核未把未实现的 Go 语义写成 Rust 现状。

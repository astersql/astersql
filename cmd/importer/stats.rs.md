# `cmd/importer/stats.rs`

## 文件定位

`cmd/importer/stats.rs` 是 `astersql-cmd-importer` crate 内的统计信息适配与抽样层。crate 根在 `cmd/importer/lib.rs` 通过 `pub mod stats` 暴露本模块；`cmd/importer/Cargo.toml` 将该 crate 定义为 `kind = "binary"` 的 Go `cmd/importer` 移植，并只直接依赖 `toml`、`serde_json`，较重的 TiDB 统计类型由 `cmd/importer/stubs.rs` 中的轻量结构替代。

本文件位于两条链路的交界处：入口 `cmd/importer/main.rs::run_with_args` 调用 `loadStats` 并把加载出的索引/列直方图包装成 `histogram`，数据生成侧再由 `cmd/importer/db.rs` 和 `cmd/importer/rand.rs` 按列类型消费这些直方图。它不是完整的统计子系统，也不负责解析表结构、调度 worker 或执行 SQL。

## 核心职责

1. `loadStats` 把表元数据和统计文件路径转交给 `stubs::load_stats_file`，返回 importer 可消费的 `StatsTable`。
2. `histogram` 保存桶计数、边界和可选索引元数据，并将累计计数映射为“区间”或“重复值”两类抽样结果。
3. 针对整数、字符串和日期时间边界生成样本；字符串路径还计算并缓存边界字符串的平均长度。
4. `getValidPrefix` 按 Go `range string` 所使用的 UTF-8 字符起始字节偏移，构造位于两个字符串边界之间的前缀。

本模块只做 importer 所需的有限抽样。统计 JSON 的实际解析、TiDB JSON 与简化 JSON 的分派，以及 `Bounds`/时间值的具体表示均在 `cmd/importer/stubs.rs` 中实现。

## 主要符号

- `pub fn loadStats(tblInfo: &TableInfo, path: &str) -> Result<StatsTable>`：公开加载入口。实现仅调用 `stubs::load_stats_file`，保留底层文件读取、JSON 解析和统计转换错误。
- `pub struct histogram`：轻量直方图包装器。
  - `core: HistogramCore` 保存 `Buckets`、`Bounds` 和直方图 ID。
  - `index: Option<IndexInfo>` 保存索引元信息；当前文件的抽样方法不读取它，但 `main.rs` 在包装索引统计时传入 `hist.Info`，列统计则传入 `None`。
  - `avgLen: Mutex<i32>` 是字符串平均长度的惰性缓存；`0` 表示尚未初始化。
- `histogram::from_core(core, index) -> Self`：构造包装器，并把 `avgLen` 初始化为 `0`。
- `histogram::getRandomBoundIdx() -> i32`：按最后一个桶的累计 `Count` 随机取计数，并顺序寻找首个覆盖它的桶。返回 `2*i` 表示桶内部区间，返回 `2*i+1` 表示桶尾部的 `Repeat` 热点。
- `histogram::randInt() -> i64`：偶数索引在上下界间调用 `data::randInt64`；奇数索引直接返回对应上界值。
- `histogram::getAvgLen(maxLen) -> i32`：计算全部字符串边界的平均字节长度，限制为 `maxLen`，并把零结果提升为 `1`。
- `histogram::randString() -> String`：偶数索引先调用 `getValidPrefix`，再按 `avgLen` 补随机后缀；奇数索引直接返回热点边界字符串。
- `histogram::randDate(unit, mysqlFmt, dateFmt) -> String`：偶数索引计算上下界的 `timestamp_diff`，随后从下界增加随机天数并格式化；奇数索引直接格式化热点边界。
- `histogram::ensure_avg_len(n)`：持有互斥锁检查并初始化 `avgLen`，保证共享直方图只在首次字符串生成时遍历边界。
- `pub fn getValidPrefix(lower, upper) -> String`：检查 `lower` 的 UTF-8 字符起始字节；遇到首个不同字节时，在相应字节跨度内随机生成末字节并返回前缀，否则返回 `lower`。

## 执行流程

统计接入主流程如下：

1. `main.rs::run_with_args` 先解析建表和索引 SQL，得到 `table.tblInfo` 与列数组。
2. 当 `StatsCfg.Path` 非空时，它调用 `loadStats(&table.tblInfo, path)`。
3. `stubs::load_stats_file` 读取文本并解析 JSON；包含 `"bounds"` 时走简化格式，否则走 TiDB JSON。TiDB JSON 按 `TableInfo` 中的列名、索引名或索引首列名恢复统计。
4. `run_with_args` 先遍历索引统计，把非空直方图挂到索引首列；随后遍历列统计，只为尚未被索引直方图占用的列补充直方图。包装对象由 `histogram::from_core` 创建并置于 `Arc` 中。
5. 生成 SQL 值时，`db.rs::randInt64Value` 优先调用 `histogram::randInt`；`db.rs::randStringValue` 先调用 `ensure_avg_len`，再调用 `randString`；`rand.rs::{randDate, randTime, randTimestamp, randYear}` 在列带直方图时调用 `histogram::randDate`。
6. 所有类型首先通过 `getRandomBoundIdx` 选择区间或重复值分支。区间分支进一步在上下界之间抽样，重复值分支直接复用桶上界，以保留热点权重。

`getRandomBoundIdx` 使用严格条件 `bkt.Count - bkt.Repeat > randCnt`。因此落到桶尾 `Repeat` 区域的计数进入奇数索引分支；如果循环未命中则回退到索引 `0`，这一行为与同路径 Go 实现一致。

## 数据与状态

`HistogramCore.Buckets` 中的 `Count` 是累计计数，最后一个桶的 `Count` 被当作总体抽样上界；`Repeat` 表示桶上界值的重复数量。`Bounds` 按每桶两个位置组织，偶数位置是下界、紧随其后的奇数位置是上界，因此返回的边界索引可以直接传给 `Bounds.GetRow`。

`histogram` 本身拥有 `HistogramCore` 和可选 `IndexInfo`；`main.rs` 从加载结果克隆这些数据后创建包装器。列通过 `Option<Arc<histogram>>` 共享它，因此抽样方法使用 `&self`，唯一可变状态是 `avgLen` 的互斥缓存。

字符串长度按 Rust `String::len()` 计算 UTF-8 字节数，而非 Unicode 字符数，这与 Go `len(string)` 的字节长度语义一致。`getValidPrefix` 也只在 `lower.char_indices()` 产生的字符起始字节位置比较原始字节；构造出的字节序列通过 `String::from_utf8_lossy` 转回 Rust 字符串。后者会替换非法 UTF-8，而 Go 字符串可保存任意字节，这是当前跨语言表示层的一处潜在差异。

## 依赖与调用关系

上游调用者和装配关系：

- `cmd/importer/lib.rs` 声明并公开 `stats` 模块，同时把 `stats_test.rs` 作为独立测试模块接入。
- `cmd/importer/main.rs::run_with_args` 调用 `loadStats`，并调用 `histogram::from_core` 装配索引/列直方图。
- `cmd/importer/db.rs::randStringValue` 调用 `ensure_avg_len` 和 `randString`；`randInt64Value` 调用 `randInt`。
- `cmd/importer/rand.rs::{randDate, randTime, randTimestamp, randYear}` 调用 `histogram::randDate`，分别传入日期、秒级时间、秒级时间戳和年份格式参数。
- `histogram::{randInt, randString, randDate}` 都调用 `getRandomBoundIdx`；`randString` 还调用 `getValidPrefix`。

下游依赖：

- `crate::data::{randInt, randInt64, randString}` 提供闭区间整数抽样和固定长度随机字符串。
- `crate::stubs::{HistogramCore, IndexInfo, StatsTable, TableInfo}` 定义轻量统计/元数据结构。
- `stubs::load_stats_file` 完成文件与 JSON 解析；`stubs::timestamp_diff`、`CivilTime` 的格式化/加天逻辑支撑日期抽样；`stubs::fatal` 处理不可恢复的边界或格式错误。
- 标准库 `Mutex<i32>` 保护字符串平均长度缓存。

RustCodeGraph 对 `stats.rs::loadStats` 给出的直接测试调用者是 `parity_test.rs::stats_loader_rejects_malformed_json` 和 `stats_test.rs::bundled_tidb_stats_json_restores_named_histograms`；文件级引用还包括 `main.rs`、`db.rs`、`rand.rs` 等装配和消费端。对常见方法名的图查询存在跨 Go/Rust 同名噪声，因此方法级调用边同时以这些直接源码位置核验。

## 错误处理与边界

- `loadStats` 使用 `Result` 传播文件读取、JSON 解析和统计转换错误。`main.rs` 对该错误调用 `stubs::fatal`；测试 `stats_loader_rejects_malformed_json` 验证畸形 JSON 返回错误。
- `getRandomBoundIdx` 直接访问最后一个桶；空 `Buckets` 会发生越界 panic。生产装配只挂载非空桶，但手工构造 `histogram` 的调用方必须维持该前置条件。
- `getAvgLen` 在 `Bounds.NumRows() == 0` 时除以零并 panic。`stats_test.rs::empty_bounds_preserve_go_divide_by_zero_failure` 用 `#[should_panic]` 固定了这一 Go 对齐边界，因此不应擅自改成空值兜底。
- `randInt` 和 `randString` 假定边界布局与桶索引匹配；缺少配对行会由 `Bounds.GetRow` 的实现失败，而本文件不做重复校验。
- `getValidPrefix` 在遍历到超出 `upper` 长度的字符起始偏移时调用 `stubs::fatal`。不同字节的跨度使用 `wrapping_sub`，随机字节使用 `wrapping_add`，以复刻 Go 的 `uint8` 运算；调用方仍应提供按预期排序的上下界。
- `randDate` 在时间解析/格式化失败时调用 `stubs::fatal`。当 `timestamp_diff` 为零时直接格式化下界；非零时调用 `randInt(0, diff-1)`，因此反向边界导致负范围时没有本地恢复逻辑。
- `avgLen.lock().unwrap()` 会在锁中毒时 panic。当前代码没有把该故障转换为 importer 的 `Result`。
- `randDate` 虽把 `unit` 传给 `timestamp_diff`，但区间落点始终调用 `add_days(delta)`；这是同路径 Go 代码的现有行为，不应在本文件内单独“泛化”为按 unit 加秒或加年。

## 并发与资源生命周期

`main.rs` 把包装后的直方图放入 `Arc`，多个造数 worker 可以共享同一对象。桶、边界和索引元数据在构造后只读；`avgLen` 是唯一内部可变状态。

`ensure_avg_len` 在持锁状态下检查零值并调用 `getAvgLen`，因此并发首次访问只会有一个线程计算并写入缓存。`getAvgLen` 保证成功结果至少为 `1`，所以 `0` 可以稳定地兼作“未初始化”哨兵。`randString` 读取缓存时再次短暂加锁，复制出整数后立即释放，不在随机后缀生成期间持锁。

本文件不创建线程、任务、通道、文件句柄、数据库连接或事务。统计文件的读取在 `load_stats_file` 内一次性完成；随后 `StatsTable` 被主流程拆出并克隆到各列的 `Arc<histogram>`。生命周期最终随 `table` 及其 worker 共享引用一起结束。

## 与 Go 版本的对应关系

直接对照文件是 `cmd/importer/stats.go`。符号基本一一对应：Go `loadStats`、`histogram`、`getRandomBoundIdx`、`randInt`、`getValidPrefix`、`getAvgLen`、`randString`、`randDate` 在 Rust 中均保留原命名与控制流。

主要保持点：

- 桶累计计数、`Repeat` 分支、偶数/奇数边界索引协议一致。
- 整数随机范围、字符串平均字节长度及最小值 `1`、公共前缀后补随机字符的顺序一致。
- `getValidPrefix` 只访问 Go `range lower` 会产生的 UTF-8 字符起始字节偏移，并保留 `uint8` 包裹运算。
- 日期路径先按传入 unit 计算差值，再按天增加随机偏移，连同这一不完全通用的行为一起保留。
- 空 Bounds 的平均长度计算继续 panic，独立 Rust 测试明确把它作为 Go 行为契约。

主要实现差异：Go `loadStats` 直接使用 TiDB 的 JSON/统计存储包，Rust 则委托本 crate 的 `stubs::load_stats_file`，并额外支持由 `"bounds"` 识别的简化 JSON；Go 的 `avgLen` 是普通整数，Rust 因 `Arc` 跨 worker 共享而改为 `Mutex<i32>`；Rust 的 `getValidPrefix` 必须把字节结果转换为合法 `String`，因此使用有损 UTF-8 转换。当前 Rust `ensure_avg_len` 是为共享缓存新增的辅助方法，Go 版通常由调用点直接初始化 `avgLen`。

## 扩展指南

- 扩展统计文件格式时，应修改 `cmd/importer/stubs.rs::load_stats_file` 及其转换函数，而不是让 `loadStats` 承担第二套解析逻辑；同步验证 TiDB JSON、简化 JSON和错误传播。
- 增加新的抽样类型时，先保持 `getRandomBoundIdx` 的偶数/奇数协议，再在 `cmd/importer/db.rs::genColumnData` 或 `cmd/importer/rand.rs` 的对应类型分支接入。不要绕过索引直方图优先于列直方图的 `main.rs` 装配规则。
- 修改字符串策略时要同时考虑 UTF-8 字节长度、`getValidPrefix` 的 Go range 偏移、无效 UTF-8 转换差异和 `avgLen` 锁粒度；相关回归应放在独立的 `cmd/importer/stats_test.rs`，不要把测试写入生产源文件。
- 修改桶选择或范围端点时，应增加固定随机种子的频次/边界测试，并同步对照 `cmd/importer/stats.go`。尤其不要把严格的 `>`、闭区间随机辅助或热点上界返回改成看似等价的分支。
- 修改日期采样时，应先确认是否有意偏离 Go 当前的“按 unit 求差、按天加偏移”语义，并分别覆盖 DATE、TIME、TIMESTAMP、YEAR 的格式和边界；这可能带来兼容性与分布变化。
- 若要处理空桶、空 Bounds、反向边界或锁中毒，必须把它作为显式兼容性变更，而不是局部防御式修复，因为现有测试已固定部分 panic 行为。
- 性能上，桶选择是线性扫描，平均字符串长度是首次使用时线性扫描全部边界。若改为二分或预计算，需要证明累计计数、并发初始化和随机分布不变。

## 验证依据

本说明基于以下直接证据：

- 生产源码：`cmd/importer/stats.rs`（全部 195 行）、`cmd/importer/main.rs::run_with_args`、`cmd/importer/db.rs::{randStringValue, randInt64Value, genColumnData}`、`cmd/importer/rand.rs::{randDate, randTime, randTimestamp, randYear}`、`cmd/importer/lib.rs`。
- crate/实现边界：`cmd/importer/Cargo.toml`；`cmd/importer/stubs.rs::{HistogramCore, StatsTable, load_stats_file, table_stats_from_simplified_json, table_stats_from_tidb_json}`。
- Go 对照：`cmd/importer/stats.go` 的同名类型和函数。
- 独立测试：`cmd/importer/stats_test.rs::{empty_bounds_preserve_go_divide_by_zero_failure, valid_prefix_visits_go_range_byte_offsets_only, bundled_tidb_stats_json_restores_named_histograms}`；`cmd/importer/parity_test.rs::{stats_loader_rejects_malformed_json, contract_normal_path}` 中的统计契约片段。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter cmd/importer` 确认目标、Go 对照与测试均已索引；`node --file cmd/importer/stats.rs` 给出完整源码及文件引用；`node loadStats` 给出 Rust 测试调用边；`node load_stats_file`、`node table_stats_from_tidb_json`、`node table_stats_from_simplified_json` 给出解析下游调用边；方法级同名查询再以直接源码位置消歧。

本任务是纯文档分析，未运行 Cargo。交付前使用任务指定命令检查目标文件存在且恰有 11 个固定二级章节，并人工复核未建议把 Rust 测试嵌入生产文件。

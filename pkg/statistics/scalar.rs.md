# `pkg/statistics/scalar.rs`

## 文件定位

`scalar.rs` 属于 `astersql-statistics` crate，是统计直方图与旧版基数估算共用的“可比较标量/小离散区间”辅助层。模块由 `pkg/statistics/lib.rs` 以 `mod scalar` 装配，并通过 `pub use scalar::*` 将公开函数重导出到 crate 根；crate 自身由 `pkg/statistics/Cargo.toml` 定义，直接依赖本仓库的 `astersql-types-datum`（在代码中名为 `types`）。

它位于两条运行路径的中间：一条把 `Datum` 和直方图桶边界压成 `f64`，供桶内均匀分布假设下的插值、越界估算和全局桶合并使用；另一条把足够短的整数、时长或时间范围枚举成若干 `Datum`，供 StatsVer 1 基数估算逐点查询。源文件不负责比较器、桶定位、CMSketch 查询或统计持久化。

## 核心职责

1. `calcFraction` 对标量区间做防御性归一化，返回查询值在闭区间上的位置；退化区间或非有限结果回退到 `0.5`。
2. `convertDatumToScalar` 将数值、时长、时间、字符串/字节和两个哨兵 `Datum` 映射为 `f64`。字符串先跳过桶边界公共前缀，再由 `convertBytesToScalar` 使用至多 8 字节的大端数值近似其字典序位置。
3. `Histogram::PreCalculateScalar` 为 decimal、time、bytes、string 桶缓存上下界标量及公共前缀；`Histogram::calcFraction` 随后用这些信息估算桶内比例。
4. `calcFraction4Datums` 为不依赖某个 `Histogram` 实例的桶合并/裁切场景提供相同的三点插值。
5. `EnumRangeValues` 只在候选数少于 `maxNumStep`（10）时展开离散区间，使旧版统计可以把窄范围转成若干等值估算；不支持、类型不一致、空区间或范围过大时返回 `None`。

## 主要符号

- `calcDayNumber(year, month, day) -> i64`：内部日序号近似，供非 timestamp 时间转微秒；`0000-00` 特判为 0。
- `timeMicros(Time) -> i64`：把年月日时分秒和微秒拼成单调的微秒坐标。它不创建时区对象，也不校验日期合法性。
- `timeDifferenceNanos(upper, lower) -> i64`：timestamp 走 `Time::Sub`；其他时间类型用两个 `timeMicros` 之差乘 1000。仅由时间枚举分支使用。
- `pub fn calcFraction(lower, upper, value) -> f64`：连续区间插值核心。`upper <= lower` 返回 `0.5`，区间外值钳到 0/1，NaN、无穷或越界计算结果也返回 `0.5`。
- `pub fn convertDatumToScalar(&Datum, common_prefix_length) -> f64`：公开的 `Datum` 标量化入口。decimal 转换错误被折为 `0.0`；未知种类也返回 `0.0`；`KindMinNotNull`/`KindMaxValue` 分别映射到 `-f64::MAX`/`f64::MAX`。
- `Histogram::PreCalculateScalar(&mut self)`：公开的可变方法。空直方图或简单类型直接退出；其余目标类型重建与桶数等长的 `Histogram::Scalars`。
- `Histogram::calcFraction(&self, index, value)`：crate 内可见的方法。它从 `Scalars[index]` 取得公共前缀（缺项时按 0），再读取桶上下界并调用标量插值；不支持的 `Datum` 种类返回 `0.5`。
- `pub fn commonPrefixLength(&[Vec<u8>]) -> usize`：求多组字节的最长公共前缀；空输入为 0，比较上限是最短输入长度。
- `pub fn convertBytesToScalar(&[u8]) -> f64`：零填充到 8 字节，只读取前 8 字节并按大端 `u64` 解释。
- `pub fn calcFraction4Datums(lower, upper, value) -> f64`：先按 `value.Kind()` 判断是否计算字节公共前缀，再统一标量化三点并插值。
- `pub const maxNumStep: i64 = 10`：枚举保护阈值。实现要求产出数量落在 `1..10`；整数分支还保留 Go 定宽整数的回绕语义。
- `roundDuration(value, step) -> i64`：内部实现 Go `time.Duration.Round` 的近似语义，中点远离零，并用 `i128` 中间值后钳回 `i64`。
- `pub fn EnumRangeValues(...) -> Option<Vec<Datum>>`：支持 `KindInt64`、`KindUint64`、`KindMysqlDuration`、`KindMysqlTime`；大小写沿用 Go 移植 API。

本文件没有 trait、独立结构体、宏或条件编译项；它扩展的 `Histogram` 与缓存元素 `scalar { lower, upper, commonPfxLen }` 定义在 `pkg/statistics/histogram.rs`。

## 执行流程

直方图插值流程如下：

1. 直方图构建或恢复后，`Histogram::PreCalculateScalar` 查看首个下界的 `Kind`；decimal/time 直接标量化，bytes/string 先逐桶计算上下界公共前缀。
2. 需要估算严格小于某值的行数时，`Histogram::LessRowCountWithBktIdx` 先定位桶。值既非边界命中又位于桶内时，调用 `Histogram::calcFraction`。
3. `Histogram::calcFraction` 把上下界和值统一送入 `convertDatumToScalar`，再由自由函数 `calcFraction` 得到 0 到 1 的比例；调用者将比例乘以桶内除重复值外的质量。
4. `Histogram::ExtractTopN` 在从桶边界抽取高频前缀前显式预计算缓存；`Table::PreCalculateScalar`（`pkg/statistics/table.rs`）则为表内所有列和索引直方图统一刷新缓存。
5. 无直方图实例的合并路径由 `calcFraction4Datums` 完成。`pkg/statistics/merge_global.rs` 用它把跨越全局切点的桶质量拆到左右两侧；`pkg/statistics/histogram.rs` 的桶合并和 NDV 计算也用它按交叠比例分摊。

小范围枚举流程如下：

1. `EnumRangeValues` 首先要求上下界 `Kind` 一致，并把开区间端点折算为排除数量。
2. 有符号/无符号整数按定宽回绕运算计算距离、数量和起点；跨零的大有符号区间另有保护，避免溢出后被误认成短区间。
3. duration 取两端最大 FSP 决定步长，先用 `roundDuration` 对齐下界，再逐步构造 `Duration Datum`。
4. time 还要求具体 MySQL 时间类型一致。DATE 把下界归一到午夜并按一天递增；DATETIME/TIMESTAMP 按 FSP 舍入后，以对应纳秒步长递增。
5. 只有最终数量为 1 到 9 才返回 `Some`。`pkg/planner/cardinality/row_count_column.rs` 与 `row_count_index.rs` 在 StatsVer 1 路径收到 `Some` 后逐点做等值估算；收到 `None` 时保留原范围估算路径。

## 数据与状态

本文件自身没有全局可变状态。持久状态只有调用方 `Histogram::Scalars: Vec<scalar>`：每个元素对应一个桶，保存 `lower`、`upper` 和 `commonPfxLen`。`PreCalculateScalar` 对受支持类型整体替换该向量，而不是增量更新；所以改变 `Bounds` 后必须由拥有者重新调用预计算，不能假设旧缓存仍有效。

字节标量是有损表示：公共前缀后的第 9 字节及以后不会影响结果，且 `u64` 转 `f64` 可能丢失低位精度。这是用于选择性估计的排序近似，不是可逆编码或严格比较器。时间标量统一使用纳秒量级的 `i64` 再转 `f64`；非 timestamp 的日序号由本文件计算，timestamp 则相对 `types::MinTimestamp()` 计算。

`EnumRangeValues` 返回新分配的 `Vec<Datum>`，不缓存结果。`None` 同时表示类型不支持、种类不一致、具体时间类型不一致、范围过大、区间为空，或时间舍入/加法失败；调用方把这些情况统一视为“不适合枚举”。

## 依赖与调用关系

下游依赖集中在 `types` 与同 crate 的直方图定义：`Datum` 的种类/取值访问器、`Time::{Sub, Add, RoundFrac}`、`Duration`、默认无警告语句上下文，以及 `histogram::{Histogram, scalar}`。`Cargo.toml` 将 `types` 映射到本地包 `astersql-types-datum`；本文件没有新增外部第三方依赖或 feature 条件。

RustCodeGraph 对 `pkg/statistics/scalar.rs` 给出的直接使用文件为：

- `pkg/statistics/histogram.rs`：桶内 `LessRowCountWithBktIdx` 插值、越界形状标量化、TopN 预处理及桶交叠/NDV 合并。
- `pkg/statistics/merge_global.rs`：全局统计合并时按切点拆分桶质量。
- `pkg/planner/cardinality/row_count_column.rs`：StatsVer 1 的窄列范围逐点枚举。
- `pkg/planner/cardinality/row_count_index.rs`：StatsVer 1 将索引首个范围列展开为等值前缀。
- `pkg/statistics/scalar_test.rs`：独立 Rust 单元测试。

另外，文本调用点显示 `pkg/statistics/table.rs` 通过 `Histogram::PreCalculateScalar` 刷新列/索引缓存，`pkg/statistics/main_test.rs` 通过 crate 重导出做基础枚举断言；这两处属于方法/重导出调用，未出现在 RustCodeGraph 的直接文件列表中。

## 错误处理与边界

该模块不暴露 `Result`，而是为估算场景选择保守回退：比例计算异常回退 `0.5`，未知标量种类或 decimal 转换错误回退 `0.0`，不可枚举返回 `None`。时间的 `RoundFrac`/`Add` 错误通过 `.ok()?` 转成 `None`。这意味着调用方不能从返回值区分“不支持”和“数据/时间运算失败”；若新增必须诊断的场景，应先评估是否需要改变 API，而不能只在此处吞掉错误。

需要特别注意以下边界：

- `calcFraction` 对 `upper <= lower` 一律返回中点，而不是报错；NaN 会落入有限性检查并返回 `0.5`。
- string/bytes 的 `common_prefix_length` 超过实际长度时，`get` 失败并返回 `0.0`，不会 panic。
- `Histogram::calcFraction` 的 `Scalars.get(index)` 对缓存缺项容错为公共前缀 0，但随后 `GetLower(index)`/`GetUpper(index)` 仍要求合法桶下标。
- 枚举阈值是“少于 10 个输出”，恰好 10 个也拒绝；两个端点都排除且区间无剩余时拒绝。
- 整数运算显式使用 `wrapping_*`，测试确认 `i64::MAX -> i64::MIN` 和 `u64::MAX -> 0 -> 1` 的回绕枚举与 Go 定宽运算一致。
- DATE 分支先清除下界的时分秒；极大日期跨度因数量保护返回 `None`。

## 并发与资源生命周期

自由函数仅使用参数和栈上临时值，是无锁、无 I/O 的纯计算。`EnumRangeValues` 的资源生命周期限于返回向量；时间和字节转换不会启动任务、打开文件或持有通道/事务。

`PreCalculateScalar` 需要 `&mut Histogram`，Rust 借用规则保证重建 `Scalars` 时没有并发读写同一实例；本文件没有内部同步。完成预计算后，`calcFraction(&self, ...)` 只读边界和缓存，可随 `Histogram` 的外部共享策略并发读取。调用方若在别处通过重建/替换直方图更新统计，应把边界和标量缓存视作同一版本的数据一起发布。

计算复杂度方面，预计算为 O(桶数 × 公共前缀比较长度)，每次标量转换最多读取 8 个有效字节（但公共前缀搜索会读取完整共同部分），单次枚举最多分配 9 个 `Datum`。`maxNumStep` 同时限制 CPU 与分配规模。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/scalar.go`，主要函数、阈值和分支结构一一对应：`calcFraction`、`convertDatumToScalar`、`Histogram.PreCalculateScalar`、`Histogram.calcFraction`、`commonPrefixLength`、`convertBytesToScalar`、`calcFraction4Datums`、`maxNumStep`、`EnumRangeValues`。Rust 的 `Option<Vec<Datum>>` 对应 Go 的 `nil` 切片；`roundDuration` 是为复现 Go `time.Duration.Round` 单独写出的辅助函数；Rust 的 `wrapping_*` 保留 Go 整数溢出行为。

已验证的语义差异/迁移边界如下：

- Go 为非法/零日期定义了 `UTCWithAllowInvalidDateCtx`，timestamp 标量化和枚举使用允许非法日期的上下文；Rust 当前使用 `types::DefaultStmtNoWarningContext`，没有本文件级等价上下文。不能仅凭 Go 注释断言 Rust 对非法日期完全等价。
- Go `TestCalcFraction` 包含 MySQL BIT/BinaryLiteral 案例；Rust 的 `convertDatumToScalar` 与 `Histogram::calcFraction` 当前没有对应 `KindBinaryLiteral` 分支，独立 Rust 类型矩阵也未覆盖 BIT。因此 BIT 的对齐状态是“未支持/未验证”，不是已迁移能力。
- Go 的 `PreCalculateScalar` 对简单数值类型直接从块行读取值，decimal/time/bytes/string 使用缓存；Rust `Histogram::calcFraction` 统一从 `Datum` 取三点再标量化，对受支持种类结果意图相同，但实现路径不同。
- Rust 为非 timestamp 时间引入 `calcDayNumber`/`timeMicros`，避免完全依赖 `Time::Sub`；Go 版本统一通过其时间上下文做 `Sub`。现有 Rust 测试验证普通 DATE/DATETIME 的天差和枚举，但非法日期兼容性仍未验证。

Go 测试 `TestCalcFraction` 和 `TestEnumRangeValues` 是移植意图的权威对照；Rust 的 `pkg/statistics/scalar_test.rs` 用六个独立测试覆盖主要类型矩阵、边界、时间精度、空/大区间及回绕行为。

## 扩展指南

新增 `Datum Kind` 的标量插值时，应同步修改 `convertDatumToScalar`、`Histogram::calcFraction` 和必要时 `calcFraction4Datums`；若类型需要预计算缓存，还要加入 `PreCalculateScalar` 的类型筛选。新增字节类类型必须先明确排序规则是否能由“公共前缀 + 前 8 字节大端数”近似，不能直接复用而忽略 collation 或编码语义。

新增可枚举类型时，应在 `EnumRangeValues` 增加同种类校验、精度/步长定义、开闭端点处理、溢出保护和严格的 `< maxNumStep` 数量限制。不要把枚举上限放宽而不评估 `row_count_column.rs`/`row_count_index.rs` 中逐点等值估算的 CPU 成本。

所有 Rust 测试应继续放在独立的 `pkg/statistics/scalar_test.rs`，不要嵌入生产源文件。至少同步覆盖：区间内/外/退化/NaN 比例，新的类型矩阵，转换失败回退，开闭端点，空与超大范围，定宽整数边界，时间类型/FSP/时区或非法日期。Go 语义变化还应对照更新 `pkg/statistics/scalar.go` 与 `scalar_test.go` 的差异，尤其关注 BIT 和非法日期上下文这两个当前缺口。

涉及 `Histogram::Scalars` 的改动还应复核 `pkg/statistics/histogram.rs` 的桶内估算、越界估算、TopN 抽取和桶合并，以及 `pkg/statistics/table.rs` 的缓存刷新接线；涉及枚举返回契约的改动应复核两个 planner 调用方在 `None` 时是否仍正确回退到范围估算。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/statistics/scalar.rs --offset 1 --limit 500` 完整读取 367 行，并报告 5 个直接使用文件；`query` 分别确认 Rust/Go 的 `calcFraction`、`convertDatumToScalar`、`PreCalculateScalar`、`commonPrefixLength`、`convertBytesToScalar`、`calcFraction4Datums`、`EnumRangeValues` 符号。
- 源码与装配：`pkg/statistics/scalar.rs`、`pkg/statistics/histogram.rs`（`Histogram`、`scalar`、桶内估算和合并调用点）、`pkg/statistics/lib.rs`（模块及重导出）、`pkg/statistics/Cargo.toml`（crate 和 `types` 依赖）。该目录不存在 `doc.go`，因此没有额外的包级 Go 契约文件可读。
- 上游调用：`pkg/statistics/merge_global.rs`、`pkg/statistics/table.rs`、`pkg/planner/cardinality/row_count_column.rs`、`pkg/planner/cardinality/row_count_index.rs`；调用点由 RustCodeGraph 文件节点和精确符号搜索交叉核对。
- Go 对照与测试：`pkg/statistics/scalar.go`、`pkg/statistics/scalar_test.go`。
- Rust 独立测试：`pkg/statistics/scalar_test.rs` 的 `scalar_conversion_and_fraction_match_boundaries`、`datum_fraction_matches_go_type_matrix`、`small_integer_ranges_are_enumerated_with_exclusions`、`temporal_ranges_follow_fractional_precision`、`integer_ranges_wrap_at_machine_boundaries`、`enum_ranges_match_go_temporal_and_empty_cases`；`pkg/statistics/main_test.rs` 另有基础枚举断言。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核文档只描述上述源码和调用证据。

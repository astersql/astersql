# `pkg/dumpformat/parquetfile/spark_rebase_micros_generated.rs`

## 文件定位

这是 `astersql-dumpformat-parquetfile` crate 中保存 Apache Spark legacy Julian/Gregorian 重定基准数据的生成文件；crate 根模块在 [`lib.rs`](lib.rs) 以 `pub mod spark_rebase_micros_generated` 暴露它，crate 边界由 [`Cargo.toml`](Cargo.toml) 定义。文件头注明时间戳数据来自 Spark 3.5.7 的 `julian-gregorian-rebase-micros.json`，日期数据来自 `RebaseDateTime.scala`；时间戳 switch/diff 已从 Spark 的秒转换为微秒，DATE 表仍以天为单位（源码第 16--28 行）。

它是数据与低层查表辅助模块，不负责解析 Parquet footer、判断文件是否采用 legacy 历法，也不执行具体时间戳重定基准。当前 Rust 运行主链位于 [`spark_rebase.rs`](spark_rebase.rs)：该文件通过 `include_str!("spark_rebase_micros_generated.go")` 解析 Go 生成表并建立 `OnceLock<GeneratedRebase>`，没有调用本文件的两个公开函数；因此本文件目前是已编入 crate、可由外部路径访问，但未接入现行 Rust rebase 主链的并行生成表示。

## 核心职责

本文件承担三项职责：

1. 用 `sparkLegacyDateRebaseSwitchDays` 与 `sparkLegacyDateRebaseDiffs` 保存 14 个 DATE 历法切换区间及对应日偏移。
2. 用 `sparkJulianGregorianRebaseMicrosSwitches` 与 `sparkJulianGregorianRebaseMicrosDiffs` 保存所有时区拼接后的 TIMESTAMP 微秒平行表；本次静态核对两表各有 9,103 项。
3. 用 602 条 `sparkRebaseMicrosRecord` 将 IANA/兼容时区 ID 映射到全局数组中的 `(offset, length)`，并提供索引查询和零复制切片函数。

该文件只表达 Spark 兼容数据的布局。某个输入应否 rebase、1900 年 cutoff、极早日期 fallback、溢出检查等行为均由 [`spark_rebase.rs`](spark_rebase.rs) 决定。

## 主要符号

- `pub struct sparkRebaseIndex { pub offset: u32, pub length: u16 }`（第 33--38 行）：全局平行数组中的片段描述符。`Clone + Copy` 允许按值返回；字段公开意味着 crate 外调用者也能自行构造索引。
- `struct sparkRebaseMicrosRecord`（第 42--45 行）：内部记录，将静态 `timeZoneID` 绑定到 `sparkRebaseIndex`，不对模块外暴露。
- `sparkJulianGregorianRebaseMicrosIndex(&str) -> Option<sparkRebaseIndex>`（第 50--61 行）：空字符串立即返回 `None`，否则线性扫描记录并按完全相等的时区 ID 返回索引；未知 ID 返回 `None`。
- `sparkJulianGregorianRebaseMicrosSlices(sparkRebaseIndex) -> (&'static [i64], &'static [i64])`（第 66--75 行）：把 `u32/u16` 转为 `usize` 后，以相同区间切出 switches 与 diffs。它不验证索引来源。
- `sparkLegacyDateRebaseDiffs`、`sparkLegacyDateRebaseSwitchDays`（第 80--98 行）：14 项 DATE 偏移及区间起点；最后一个 switch `-141427` 对应 1582-10-15，自该区间起 diff 为 0。
- `sparkJulianGregorianRebaseMicrosSwitches`（第 102 行起）与 `sparkJulianGregorianRebaseMicrosDiffs`（第 9812 行起）：TIMESTAMP 平行表，位置相同的元素共同描述一个区间起点及应加偏移。
- `sparkJulianGregorianRebaseMicrosRecords`（第 19522 行起）：私有时区索引表。末条 `Zulu` 为 `offset: 9089, length: 14`，其结束位置恰为 9,103。

文件没有 trait、`impl`、宏或条件编译项。

## 执行流程

如果未来调用方直接使用本模块，流程是：先把 footer/fallback 得到的时区 ID 传给 `sparkJulianGregorianRebaseMicrosIndex`；函数拒绝空串并顺序比较 602 条记录；命中后将复制出的 `sparkRebaseIndex` 传给 `sparkJulianGregorianRebaseMicrosSlices`；后者计算半开区间 `[offset, offset + length)`，对两张表返回同生命周期、同长度的静态切片。真正的区间选择应由上层根据输入微秒与 switches 比较，再把对应 diff 加到输入值。

DATE 路径不需要时区索引：上层按 `sparkLegacyDateRebaseSwitchDays` 找到输入天数所在区间，并使用同下标的 `sparkLegacyDateRebaseDiffs`。早于首个 DATE switch 的完整 Julian→Gregorian 换算不在此文件内。

当前实际 Rust 流程有所不同：`SparkRebaseMicrosLookup::new` 经 `rebase_index`、`rebase_slices` 读取 [`spark_rebase.rs`](spark_rebase.rs) 中从 Go 文本懒解析出的表；RustCodeGraph 显示 `rebase_index` 的调用者是 `spark_rebase_time_zone_id` 和 `SparkRebaseMicrosLookup::new`，`rebase_slices` 的调用者是后者。本文件函数的精确 Rust callers/callees 查询为空，仓库范围文本检索也只在 `spark_rebase.rs` 的注释草稿中发现同名调用。

## 数据与状态

所有数据都是进程只读的 `'static` 数组或字符串，没有可变全局状态。TIMESTAMP 表的不变量是：switches 与 diffs 等长；每条 record 的片段必须同时落在两表范围内；同一 record 片段内两个数组按下标平行；record 的 offset 应按前一条 `offset + length` 连续推进。静态核对得到 9,103 个 switch、9,103 个 diff、602 条 record，最后一条结束位置为 9,103，证明表尾闭合；这只是当前生成产物的结构证据，不替代未来重新生成后的自动校验。

`sparkRebaseIndex` 选择 `u32/u16` 压缩元数据；切片时才转换为平台 `usize`。查找返回索引副本，切片返回对静态表的借用，不复制 9,103 项数据，也不分配堆内存。

## 依赖与调用关系

本文件内部只使用 Rust 核心语言的数组、切片、字符串引用和 `Option`，不直接依赖 [`Cargo.toml`](Cargo.toml) 中的 `chrono`、`chrono-tz`、`parquet` 或其他 AsterSQL crate。它通过 [`lib.rs`](lib.rs) 的公开模块声明归属于 `astersql-dumpformat-parquetfile`。

设计上的上游应是 Spark footer 时区选择和 lookup 构造，下游是静态表；Go 版本确实形成 `sparkRebaseTimeZoneID` / `newSparkRebaseMicrosLookup` → `sparkJulianGregorianRebaseMicrosIndex` → `sparkJulianGregorianRebaseMicrosSlices` 的调用链。当前 Rust 对应调用链则落在 [`spark_rebase.rs`](spark_rebase.rs) 的 `rebase_index` / `rebase_slices` 上，数据来源是 [`spark_rebase_micros_generated.go`](spark_rebase_micros_generated.go)，不是本 Rust 文件。后续若接线到本文件，应删除或替换重复的运行时 Go 文本解析和重复 DATE 常量，避免两份数据源漂移。

## 错误处理与边界

`sparkJulianGregorianRebaseMicrosIndex` 用 `Option` 表达空串或未知时区，没有错误文本；比较区分大小写，不做 trim、别名解析或 tzdb 规范化。那些策略必须由上层在调用前完成。

`sparkJulianGregorianRebaseMicrosSlices` 是无检查的低层接口：由表内记录获得的索引在当前生成数据下有效，但由于 `sparkRebaseIndex` 及字段均为 `pub`，任意调用者可传入越界或 `offset + length` 越界的值，Rust 切片会 panic。函数也不显式检查两表长度一致；其安全性依赖生成器保持平行布局。零长度合法地返回空切片，但当前 602 条记录均非零长度。

本文件不进行整数加法、日期换算或输入微秒运算，所以不负责 rebase 结果溢出。当前主链的 `SparkRebaseMicrosLookup::rebase` 在 [`spark_rebase.rs`](spark_rebase.rs) 使用 `checked_add` 报告溢出，这一保证不能自动推断给未来直接使用本模块的调用者。

## 并发与资源生命周期

Rust 表在程序映像中静态存在，初始化后只读，借用生命周期为 `'static`；索引扫描没有锁、缓存、任务、通道、文件句柄或显式清理，因此可被多线程并发读取。代价是每次时区查询最坏比较 602 条记录，且无首次查找分配。

这与 Go 版本的资源策略不同：Go 用 `sync.Once` 在首次查询时构造 map，首次有分配与同步，之后平均常数时间查找；Rust 本文件选择线性扫描以避免 `OnceLock/HashMap`。当前 Rust 主链又采用另一种策略：首次调用时用 `OnceLock` 解析嵌入的 Go 文本并构造 `BTreeMap`，缓存生命周期为整个进程。

## 与 Go 版本的对应关系

直接对照文件是 [`spark_rebase_micros_generated.go`](spark_rebase_micros_generated.go)。两者拥有相同的索引字段、两张 DATE 表、两张 TIMESTAMP 平行表、时区记录表，以及“索引后切片”的语义；数据源说明均指向 Spark 3.5.7，并明确时间戳单位为微秒。

主要实现差异是索引查找：Go 的 `sparkJulianGregorianRebaseMicrosIndex` 用 `sync.Once` 将 records 转成 map，并返回 `(index, bool)`；Rust 用静态记录线性扫描并返回 `Option<index>`。Go 的类型和辅助函数均为包内小写符号，Rust 则公开 `sparkRebaseIndex`、两个字段、两个辅助函数和四张数据表。两边的切片函数都信任索引，不做边界错误转换。

Go 行为测试 [`parser_test.go`](parser_test.go) 直接断言默认 `UTC` 存在、取得 switch 切片，并覆盖未知 footer 时区回退 UTC、缺失 footer 时区采用 parser location、早于表范围的 fallback 等行为。Rust 独立测试 [`spark_rebase_test.rs`](spark_rebase_test.rs) 只通过现行 `spark_rebase.rs` 路径验证现代时间快速返回、legacy 未知时区报错、版本与 floor 运算；它没有直接引用本生成 Rust 模块。因此不能把 Go 测试覆盖等同于本模块已被 Rust 测试直接覆盖。

## 扩展指南

- 更新 Spark 版本或 tzdb 数据时，应修改/运行真正的生成流程，同时再生 Rust 与 Go 文件；不要手改 9,103 项字面量。至少自动检查两张 TIMESTAMP 表等长、record offset 连续且 `offset + length` 不越界、时区 ID 唯一、每段 switches 的顺序满足上层搜索算法。
- 若把 Rust 主链切换到本模块，接入点是 [`spark_rebase.rs`](spark_rebase.rs) 的 `rebase_index`、`rebase_slices`、DATE 常量和 `parse_generated_rebase`/`generated`。应一次性消除重复来源，并保持 `SparkRebaseMicrosLookup::new` 的未知时区、空表和等长校验。
- 若保留公开 `sparkJulianGregorianRebaseMicrosSlices`，可考虑将索引字段私有化，或改为返回 `Option`/`Result` 并使用 `get`，避免外部构造索引导致 panic；这是 API 行为变更，需要独立评估兼容性。
- 测试逻辑应放在同目录独立测试文件而不是源文件中。直接测试本模块时，优先扩展 [`spark_rebase_test.rs`](spark_rebase_test.rs)，覆盖空/未知/已知时区、UTC 片段长度、首尾 record、非法索引策略和两表结构不变量；行为级回归继续与 Go 的 [`parser_test.go`](parser_test.go) 对照。
- 性能取舍需显式记录：602 条线性扫描无初始化成本，但大量重复查找可能适合静态二分表或一次性 map；改变查找结构不能改变完全匹配和未知时区语义。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件，`files --filter pkg/dumpformat/parquetfile` 收录目标文件并识别 16 个符号；`query` 定位本文件两个函数、四张公开表和 records；对目标 Rust 函数的精确 `callers`/`callees` 结果为空；对现行 `rebase_index` 的 callers 为 `spark_rebase_time_zone_id`、`SparkRebaseMicrosLookup::new`，`rebase_slices` 的 caller 为后者。
- 已读源码与边界：[`spark_rebase_micros_generated.rs`](spark_rebase_micros_generated.rs)、[`lib.rs`](lib.rs)、[`spark_rebase.rs`](spark_rebase.rs)；仓库没有该目录的 `doc.go`。
- 已读 crate/对照/测试：[`Cargo.toml`](Cargo.toml)、[`spark_rebase_micros_generated.go`](spark_rebase_micros_generated.go)、[`spark_rebase.go`](spark_rebase.go)、[`spark_rebase_test.rs`](spark_rebase_test.rs)、[`parser_test.go`](parser_test.go)。
- 静态结构统计：用 `awk` 按三个数组/records 区段计数，结果为 `switches=9103 diffs=9103 records=602`；末条 record 的范围终点为 9,103。
- 仓库范围 `rg`：排除定义文件后，Rust 中同名生成表 API 只出现在 `spark_rebase.rs` 的注释草稿和该文件的重复 DATE 常量/Go 文本解析标记，未发现现行 Rust 直接调用。
- 本任务为纯文档分析，依计划不运行 Cargo；交付前只执行任务指定的 11 章节结构验证并人工复核链接、事实边界和未接线说明。

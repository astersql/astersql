# `pkg/dumpformat/parquetfile/spark_rebase.rs`

## 文件定位

本文件属于 Cargo crate `astersql-dumpformat-parquetfile`，由同目录 `lib.rs` 以公开模块 `spark_rebase` 装配。它位于 Parquet 读取链的格式兼容层：`file_parser.rs` 读取文件 footer，判断文件是否由旧版 Spark 的 Julian/Gregorian 混合历法写出；`type_converter.rs` 再借助本文件把 DATE、TIMESTAMP_MILLIS、TIMESTAMP_MICROS 和 INT96 的旧历法值重映射到 proleptic Gregorian（前推公历）时间轴。它不负责 Parquet I/O、SQL Datum 的最终构造或普通时区换算。

该 crate 的边界由 `pkg/dumpformat/parquetfile/Cargo.toml` 给出，库入口是 `lib.rs`；本文件自身只直接使用标准库集合、`OnceLock` 和 crate 根的 `Error`/`Result`。Parquet crate 提供的真实 footer 元数据先在 `file_parser.rs` 被压缩成这里定义的 `SparkFileMeta`，因此本模块没有直接依赖 Arrow/Parquet 元数据类型。

## 核心职责

1. 解析 Spark 写入版本及 legacy marker，选择是否需要 rebase，并在 footer 时区、解析器 fallback 时区和 `UTC` 之间按确定顺序选出生成表中的有效时区（`spark_version_from_metadata`、`spark_rebase_time_zone_id`）。
2. 对 DATE 的 epoch-day 值按固定 switch/diff 表修正；对早于表覆盖范围的日期执行完整 Julian 日期标签到 Gregorian 日期轴的转换（`rebase_julian_to_gregorian_days`）。
3. 从 `spark_rebase_micros_generated.go` 解析每个时区的 TIMESTAMP switch/diff 切片，并以进程级只读缓存供 lookup 重用（`parse_generated_rebase`、`generated`、`SparkRebaseMicrosLookup::new`）。
4. 对 1900-01-01 之前的微秒时间戳查表修正；早于首个 switch 时走完整历法换算，现代时间走无分配快速路径（`SparkRebaseMicrosLookup::rebase`、`rebase_before_switch`、`rebase_spark_julian_to_gregorian_micros`）。
5. 提供负数时间戳所需的 floor division/modulus，以及少量 Go 风格兼容别名。

文件前半部第 21–259 行是被注释掉的早期 Go 对照草稿，不参与编译和运行；实际实现从 `use crate::{Error, Result};` 开始。阅读或扩展时必须以第 260 行之后的可编译代码为准。

## 主要符号

- `GeneratedRebase { switches, diffs, zones }`：拥有从生成的 Go 文件抽取出的全局微秒 switch 数组、diff 数组，以及 `time zone ID -> (offset, length)` 索引。它是内部类型，不对 crate 外暴露。
- `parse_generated_rebase() -> GeneratedRebase`：通过 `include_str!("spark_rebase_micros_generated.go")` 把生成文件嵌入二进制，再按固定 Go 字面量标记和记录字段解析。内部 `numbers` 负责忽略行尾注释并抽取有符号整数。
- `generated() -> &'static GeneratedRebase`：用 `OnceLock` 保证首次访问时只解析一次；`rebase_index` 和 `rebase_slices` 分别查时区记录及取得对应的平行切片。
- `AppVersion`：保存 `app/major/minor/patch`。`parse_spark` 接受裸版本或 `spark version X.Y.Z`；`less_than` 只在 `app` 相同的情况下比较三元组。
- `SparkFileMeta`：本模块需要的 footer 最小视图，仅含 `created_by` 和字符串键值表。
- `spark_version_from_metadata`：专用 key `org.apache.spark.version` 存在时优先解析它；只有 key 不存在时才从 `created_by` 中寻找 `spark version `。显式 key 无效时不会回退。
- `spark_rebase_time_zone_id`：legacy marker 存在，或 Spark 版本低于传入 cutoff 时启用 rebase；优先返回生成表认识的 footer 时区，其次返回认识的 fallback，最后返回 `SPARK_REBASE_DEFAULT_TIME_ZONE_ID`（`UTC`）。非 legacy 返回空字符串。
- `julian_day_number_to_date`、`days_from_civil`：前者把 Julian day number 解成 Julian 历年月日标签，后者把同一标签编码到 Gregorian epoch-day；两者组成超早日期 fallback。
- `rebase_julian_to_gregorian_days`：DATE 主入口。固定的 14 项 `sparkLegacyDateRebaseSwitchDays` 与 `sparkLegacyDateRebaseDiffs` 必须位置一一对应。
- `SparkRebaseMicrosLookup`：持有时区 ID 及生成表中的静态 switch/diff 切片。`new` 校验时区存在、切片非空且等长；`rebase` 执行 cutoff、fallback 或区间 diff 分支。
- `floor_div_i64`、`floor_mod_i64`：实现 Java/Scala 风格向负无穷取整，避免 Rust/Go 向零截断破坏负时间戳的“天 + 日内微秒”拆分。
- `rebase_before_switch`：处理早于某时区首个 switch 的时间戳，使用首项 switch/diff 恢复源、目标侧 Common Era offset，并保留日内微秒。
- `rebase_spark_julian_to_gregorian_micros`：单次 TIMESTAMP 便利入口；先判断 1900 cutoff，只有旧值才构造 lookup。末尾的 `rebaseJulianToGregorianDays`、`floorDivInt64`、`floorModInt64`、`rebaseSparkJulianToGregorianMicros` 是 Go 风格别名。

## 执行流程

实际解析链从 `file_parser.rs::FileParser::open_next_row_group` 开始。它读取 Parquet `created_by` 和 key/value footer，构造 `SparkFileMeta`；只对 INT96、DATE 以及非 NANOS 的 TIMESTAMP 建立 legacy lookup。INT96 使用 Spark 3.1.0 cutoff 和 `org.apache.spark.legacyINT96`，其他支持的时间类型使用 Spark 3.0.0 cutoff 和 `org.apache.spark.legacyDateTime`。`spark_rebase_time_zone_id` 返回空串时，列信息中的 lookup 为 `None`；否则调用 `SparkRebaseMicrosLookup::new`，并把结果缓存到该列的 `ParquetColumnType`/`ConvertedInfo` 中。

DATE 值进入 `type_converter.rs::convert_int32`：只有列上存在 lookup 才调用 `rebase_julian_to_gregorian_days`。函数先检查是否早于 DATE switch 表首项；若是，则把 epoch-day 加 `JULIAN_DAY_OF_UNIX_EPOCH`，解出 Julian 日期标签，再用 `days_from_civil` 映射到 Gregorian epoch-day。否则从表尾向前寻找满足输入的区间，并加同位置 diff。

TIMESTAMP_MILLIS/MICROS 进入 `type_converter.rs::convert_int64`，INT96 进入 `int96_micros_with_rebase`。lookup 的 `rebase` 对不早于 `LEGACY_TIMESTAMP_REBASE_CUTOFF_MICROS`（1900-01-01T00:00:00Z）的值原样返回；早于首个 switch 的值调用 `rebase_before_switch`；其余值同样从表尾向前定位区间并加 diff。TIMESTAMP_MILLIS 在查表前乘 1000 变为微秒，之后除回毫秒；解析器时区偏移由 converter 在 rebase 之后另行处理。

首次查询任何时区时，`generated()` 调用 `parse_generated_rebase`；后续所有列和行共享同一份 `GeneratedRebase`。lookup 只保存字符串和静态切片，不为每个值重新解析表或复制表数据。

## 数据与状态

DATE 数据由两个长度均为 14 的编译期数组表示。索引位置是不变量：每个 switch 的修正量必须取相同索引的 diff。TIMESTAMP 数据来自编译期嵌入的 `spark_rebase_micros_generated.go`，解析后形成两个全局 `Vec<i64>` 和按字符串有序的 `BTreeMap`；每个 zone 记录 `(offset, length)`，`rebase_slices` 用它在两个向量中切出同范围。

唯一可延迟初始化的全局状态是 `generated()` 内的 `OnceLock<GeneratedRebase>`。初始化成功后数据不再修改，lookup 持有生命周期为 `'static` 的不可变切片。`SparkFileMeta`、`AppVersion` 和 lookup 本身是普通值；没有事务、文件句柄、异步任务或通道。

重要单位包括：DATE 是相对 Unix epoch 的整天数；TIMESTAMP lookup 输入输出均为 Unix 微秒；`MICROS_PER_DAY` 为 86,400,000,000；INT96 的日/纳秒拆解发生在 `type_converter.rs`。空时区字符串是“无需 legacy rebase”的控制信号，不表示 UTC。

## 依赖与调用关系

上游装配与调用关系如下：

- `lib.rs` 公开 `spark_rebase` 模块，并通过独立的 `spark_rebase_test.rs` 注册回归测试。
- `file_parser.rs::open_next_row_group` 调用 `AppVersion::parse_spark`、`spark_rebase_time_zone_id` 和 `SparkRebaseMicrosLookup::new`，将每列的 rebase 决策固定下来。
- `type_converter.rs::convert_int32` 调用 DATE rebase；`convert_int64` 调用 lookup 处理毫秒/微秒 timestamp；`int96_micros_with_rebase` 和 `convert_int96` 处理 INT96。
- `parser_test.rs` 直接覆盖 DATE 条件启用、metadata/version/timezone 决策、现代时间快速路径、未知时区和负数 floor 运算；`spark_rebase_test.rs` 补充现代时间不查未知时区、旧时间必须有生成时区、无效显式版本不回退以及负除数语义。

下游依赖主要是 `spark_rebase_micros_generated.go` 的文本格式和 crate 根 `Error`/`Result`。虽然同目录还有 `spark_rebase_micros_generated.rs` 模块，本文件当前的真实生产 lookup 并未调用它，而是直接 `include_str!` 并解析 Go 生成文件；因此 Go 文件不仅是对照证据，也是当前 Rust 编译产物的数据输入。

## 错误处理与边界

`SparkRebaseMicrosLookup::new` 对未知时区返回 `Error("unknown ... timezone")`；对空表或 switches/diffs 长度不等返回表为空类错误。`SparkRebaseMicrosLookup::rebase` 对常规区间的 `micros + diff` 使用 `checked_add`，溢出返回 `rebased timestamp overflow`。`rebase_spark_julian_to_gregorian_micros` 在构造 lookup 前做 cutoff 判断，所以现代时间即使传入未知时区也原样成功返回。

生成表解析使用多个 `unwrap`，`rebase_slices` 使用直接范围索引：嵌入的 Go 文件若改变声明标记、记录字段格式、整数合法性或 offset/length 边界，首次访问会 panic，而不是返回普通 `Result`。这是构建时受控生成数据的不变量，不应当用于解析任意外部文本。`rebase_before_switch` 的若干偏移加减和乘法没有 `checked_*`；输入域预期是有效 Parquet/Spark 时间范围，若扩展到任意 `i64`，需专门审计极值行为。

`AppVersion::parse_spark` 会忽略 patch 数字后的非数字后缀，但要求 major 可解析；minor/patch 缺失时补零。它并不验证输入前缀里的应用名，构造结果的 `app` 固定为 `spark`。显式版本 metadata 一旦存在便具有优先权：空值或非法值会得到 `None`，不会再尝试 `created_by`，这一边界由 `spark_rebase_test.rs` 固定。

## 并发与资源生命周期

`OnceLock` 使生成表在并发首次访问下只初始化一次，并在进程生命周期内保持；初始化后的 `Vec` 不再可变，所有 lookup 仅借用其 `'static` 切片，因此逐行转换没有锁竞争、I/O 或表复制。`SparkRebaseMicrosLookup` 可 `Clone`，克隆会复制时区字符串和切片引用，不复制生成数组。

`include_str!` 在编译时读取生成 Go 文件，运行时不打开文件；首次 lookup 只做内存字符串解析和分配。列级 lookup 由 `file_parser.rs` 创建并随列转换配置生存，行级 `rebase` 只做比较、反向线性扫描和整数运算。switch 切片长度来自单个时区记录，当前实现没有额外缓存最近命中区间。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dumpformat/parquetfile/spark_rebase.go`，生成数据来源是 `spark_rebase_micros_generated.go`。Rust 保留了 Go 的核心策略：专用版本 key 优先于 CreatedBy；legacy marker 或低于 Spark 3.0/3.1 cutoff 才启用；时区只接受生成表中存在的 ID；无有效时区时落到 UTC；DATE 与 TIMESTAMP 都按 switch/diff 查表；表前值执行完整 Julian→Gregorian 转换；1900 之后的 timestamp 无条件快速返回；负数拆分使用 floor 语义。

实现形态存在几处明确差异：Rust 用自有 `SparkFileMeta`/`AppVersion` 代替 Arrow Go `metadata.FileMetaData`/`AppVersion`，由 `file_parser.rs` 负责适配；Rust 用 `OnceLock` 在运行时解析嵌入的 Go 生成文件，Go 直接引用生成的数组和索引函数；Rust lookup 构造额外校验 switches/diffs 等长，区间加法使用 `checked_add`，Go 对应加法直接执行；Rust 的时间换算使用整数 `days_from_civil`，Go 使用 `time.Date(...).Unix()`。此外，文件顶部大段注释是 Go 结构的旧草稿，不是另一个可运行实现。

Go 的 `parser_test.go` 对多组 DATE、时区和极早 timestamp 值有更广的表驱动覆盖；Rust 的直接独立测试较精简，但 `parser_test.rs` 已验证列转换接线和主要策略边界。扩展时应以 Go 生产代码及其测试意图为基线，不能因 Rust 测试样例较少而删减分支。

## 扩展指南

- 新增或修改 metadata 判定时，优先改 `spark_version_from_metadata`/`spark_rebase_time_zone_id`，并同步检查 `file_parser.rs` 的 cutoff、legacy key 和支持的 logical/physical type。需在独立的 `spark_rebase_test.rs` 增加 marker、版本、footer 时区、fallback、UTC 兜底和非 legacy 空串用例。
- 更新 Spark 生成表时，必须同时验证 `spark_rebase_micros_generated.go` 的声明标记、record 单行字段格式及 offset/length，防止 `parse_generated_rebase` 的 `unwrap` 或切片 panic；还应比较同目录 `.rs` 生成表是否仍与 Go 数据一致，避免双份数据漂移。
- 修改历法算法或 cutoff 时，要与 `spark_rebase.go` 及 Spark 上游语义逐项对齐，覆盖 switch 前、恰等于 switch、区间边界、1900 cutoff 两侧、Common Era 附近、负数日内余数以及 `i64` 溢出风险。
- 新增时间单位时，不要只在本文件加入口；还须在 `file_parser.rs` 决定该类型是否可 rebase，并在 `type_converter.rs` 保持“rebase 的单位”和转换前后缩放一致。当前 NANOS 明确不参与该路径。
- 测试逻辑应继续放在同目录独立测试文件，不要内嵌到 `spark_rebase.rs`。首选扩展 `spark_rebase_test.rs`；涉及完整列解码接线时同步扩展 `parser_test.rs`，并参考 `parser_test.go` 的表驱动预期。

## 验证依据

- RustCodeGraph `status`：索引可用，项目含 11,467 个文件、307,296 个节点、1,848,419 条边；本次通过 `explore` 和 `node --file` 检查了 `spark_rebase.rs`、`spark_rebase_test.rs`、`file_parser.rs`、`type_converter.rs`、`parser_test.rs` 的源码与使用关系。
- 生产源码：`pkg/dumpformat/parquetfile/spark_rebase.rs`，重点符号为 `parse_generated_rebase`、`generated`、`spark_rebase_time_zone_id`、`rebase_julian_to_gregorian_days`、`SparkRebaseMicrosLookup::{new,rebase}`、`rebase_before_switch`。
- crate 与模块证据：`pkg/dumpformat/parquetfile/Cargo.toml`、`pkg/dumpformat/parquetfile/lib.rs`。
- 直接上游证据：`pkg/dumpformat/parquetfile/file_parser.rs::open_next_row_group`；直接消费证据：`pkg/dumpformat/parquetfile/type_converter.rs::{convert_int32,convert_int64,int96_micros_with_rebase,convert_int96}`。
- Go 对照与生成数据：`pkg/dumpformat/parquetfile/spark_rebase.go`、`pkg/dumpformat/parquetfile/spark_rebase_micros_generated.go`；关联 Go 测试在 `pkg/dumpformat/parquetfile/parser_test.go`。
- Rust 独立测试：`pkg/dumpformat/parquetfile/spark_rebase_test.rs`；集成到列转换的测试：`pkg/dumpformat/parquetfile/parser_test.rs` 中 `date_conversion_only_rebases_with_spark_legacy_lookup`、`spark_metadata_version_timezone_and_rebase_policy_match_go`、`parser_calendar_math_uses_floor_division_for_negative_values` 等。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行任务指定的 11 章节结构检查，并人工核对文件定位、调用链、错误边界和扩展入口均有上述源码依据。

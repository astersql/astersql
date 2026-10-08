# `pkg/types/time.rs`

## 文件定位

本文件是 AsterSQL Rust 类型系统中的 MySQL 时间语义核心，集中实现 `DATE`、`DATETIME`、`TIMESTAMP` 和作为时长使用的 `TIME`。它不是由根 `pkg/types/lib.rs` 直接声明的普通模块：`pkg/types/internal/time/lib.rs` 通过 `#[path = "../../time.rs"] mod time_impl` 编译本文件并导出全部符号，形成 `astersql-types-time`；根 `pkg/types/Cargo.toml` 再以 `types-time` 路径依赖接入，`pkg/types/lib.rs` 用 `pub use types_time as time` 暴露 `astersql_types::time::*`。

该文件位于 SQL 值与日历/时长表示的公共边界。上游调用者包括 `pkg/types/datum.rs` 的 Datum 转换、`pkg/expression/builtin_cast_vec.rs` 和 `pkg/expression/builtin_time_vec_generated.rs` 的 SQL cast/时间函数、`pkg/expression/planner_bridge.rs` 的计划值转换、`pkg/lightning/backend/kv/canonical.rs` 的导入编码，以及 `pkg/ddl/persistent_masking_actions.rs` 和 `pkg/session/runtime/relational_value.rs`。目标包没有 `pkg/types/doc.go`，最近的模块契约由上述 `internal/time/lib.rs` 和 Cargo 清单给出。

## 核心职责

- 用 `CoreTime(u64)` 的高 60 位保存年月日时分秒微秒，用低 4 位同时编码 FSP 和 `DATE`/`DATETIME`/`TIMESTAMP` 类型标记，并提供构造、读取、修改、比较和 MySQL packed-u64 编解码（`FromDate`、`NewTime`、`Time::{Type,Fsp,CoreTime,ToPackedUint,FromPackedUint}`）。
- 按 `TimeContext` 的 SQL-mode 风格标志和时区规则解析、检查和转换日期时间，覆盖分隔格式、紧凑数字、整数、浮点、Decimal、显式 `Z`/`+HH[:MM]` 时区、两位年份、零日期与 TIMESTAMP 有效区间（`ParseTime`、`parseTime`、`parse_digit_datetime_string`、`Time::Check`）。
- 用 `Duration { Duration, Fsp }` 表示 MySQL `TIME`，完成解析、格式化、算术、比较、向日历时间/YEAR 转换，并强制 `±838:59:59` 边界（`ParseDuration`、`matchDuration`、`Duration::{Add,Sub,RoundFrac,ConvertToTimeWithTimestamp}`）。
- 实现 MySQL 时间函数需要的格式化、`STR_TO_DATE`、`TIMESTAMPDIFF`、`EXTRACT`、INTERVAL 解析、周序号和 FROM_DAYS 算法（`Time::DateFormat`、`Time::StrToDate`、`TimestampDiff`、`ExtractDatetimeNum`、`ParseDurationValue`、`calc_mysql_week`、`TimeFromDays`）。
- 对所有可失败路径统一返回 `TimeError`，而可容忍的 `STR_TO_DATE` 尾随内容通过 `TimeContext::append_warning` 上报。

## 主要符号

- `TimeError(String)`：时间解析、校验和运算的错误载体；`wrong` 生成 `incorrect ... value`，`overflow` 生成 `datetime function overflow`，并实现标准 `Display`/`Error`。
- `TimeContext`、`TimeFlags`、`StrictTimeContext`、`BasicTimeContext`：向纯时间算法注入零日期/非法日期放宽、TIME 转 YEAR 特殊路径、当前时区和 warning 回调。默认位置是 UTC；`StrictContext` 是无放宽标志的全局值。
- `Time { coreTime: CoreTime }`：值类型时间对象。`NewTime` 将类型和 FSP 写入低 4 位；`SetCoreTime` 保留标记位，`CoreTime` 清除标记位。`MinDatetime`/`MaxDatetime` 与 `MinTimestamp`/`MaxTimestamp` 给出显式边界。
- `Duration { Duration: i64, Fsp: i32 }`：纳秒时长及显示精度。`MinTime`/`MaxTime` 是 `±3_020_399` 秒的纳秒值，对应 `±838:59:59`；`splitDuration` 将其拆成符号、累计小时、分、秒、微秒。
- 构造与解析入口：`FromDate` 不做语义范围检查，`FromDateChecked` 只检查位宽；`ParseTime`/`ParseDatetime`/`ParseTimestamp`/`ParseDate` 解析字符串，`ParseTimeFromNum` 及整数/浮点/Decimal 包装器解析数值，`ParseDuration` 解析 TIME。
- 校验与转换入口：`Time::Check` 应用上下文、日历合法性和 TIMESTAMP UTC 范围；`GoTime`、`AdjustedGoTime`、`ConvertTimeZone` 负责 `chrono`/`chrono_tz` 转换；`Time::{Add,Sub,RoundFrac,Convert}` 与 `Duration` 同名方法负责算术和类型转换。
- 格式/区间入口：`DateFormat`、`DurationFormat`、`StrToDate`、`TimestampDiff`、`ExtractDatetimeNum`、`ExtractDurationNum`、`ParseDurationValue`、`ExtractDurationValue`、`GetFormatType`。
- 只读表与懒初始化状态：`MonthNames`、`WeekdayNames` 及内部缩写表支持英文格式化；`TZ_SUFFIX: LazyLock<Regex>` 只初始化一次，用于识别输入末尾时区。文件没有条件编译项。

## 执行流程

1. `ParseTime`、`ParseTimeWithString` 和 `ParseTimeFromFloatString` 汇合到 `parseTime`。入口先用 `CheckFsp` 把未指定精度映射为 0、拒绝负 FSP 并把过大值限制为 6，然后由 `GetTimezone` 分离可选时区后缀。
2. 纯数字输入交给 `parse_digit_datetime_string`，按 5 至 14 位长度映射日期时间字段并按 MySQL 规则扩展两位年份。带分隔符输入将 `T` 和 `/` 规范化，通过 `ParseDateFormat` 拆字段，`parse_fraction` 把小数秒四舍五入到目标 FSP；产生一秒进位时调用 `Time::Add`。
3. `FromDateChecked` 先保证字段能装入位域，`NewTime` 再附加类型/FSP。普通输入保留月或日为零的位模式，最终由 `Time::Check` 和上下文决定是否接受；只有小数进位或显式时区需要先构造真实日历值。
4. 显式 `Z` 或偏移必须落在 `±14:00`，解析成 `FixedOffset` 后转为 `ctx.location()` 的本地字段。随后 `Time::Check` 校验时分秒、微秒、日历日期；TIMESTAMP 还将本地值转 UTC，并限制在 `1970-01-01 00:00:01` 至 `2038-01-19 03:14:07.999999`。
5. `ParseDuration` 先由 `matchDuration` 识别符号、可选天数、冒号或紧凑 HHMMSS 和小数秒；分钟/秒必须不超过 59，总秒数不得超过 MySQL TIME 上限。若形态像完整日期时间且纯时长解析失败，会调用 `ParseTime(...TypeDatetime...)`，再提取时钟部分。
6. 日期时间/时长运算都使用 checked 加法或显式边界检查。`Time::Sub` 对两个 TIMESTAMP 按上下文时区比较真实 instant，其他类型按 MySQL day number 和字段计算微秒差；`Time::Add` 借助 `NaiveDateTime` 处理跨日进位，DATE 结果会清零时钟字段。
7. 格式化由 `Time::DateFormat`/`Duration::DurationFormat` 逐个解释 `%` 说明符；周相关 `%U/%u/%V/%v/%X/%x` 落到 MySQL week-mode 算法。反向 `StrToDate` 由 `parse_str_to_date` 按模板消费输入，校验 AM/PM 与 12/24 小时制，并对未消费尾部追加 warning。
8. INTERVAL 流程由 `ParseDurationValue` 按单单位或复合单位分派，统一得到年、月、日、纳秒和 FSP；`ExtractDurationValue` 拒绝不能表示成 TIME 的 YEAR/QUARTER/YEAR_MONTH，合并月日与纳秒后再次检查 `MaxTime`。

## 数据与状态

`Time` 的唯一字段是 `CoreTime(u64)`。日历分量依次占年 14 位、月 4 位、日 5 位、时 5 位、分 6 位、秒 6 位、微秒 20 位；最低 4 位不属于日历值。低位 `1110` 专用于 DATE，其他值以 bit 0 区分 TIMESTAMP/DATETIME、bit 1..3 保存 0..6 的 FSP。因而比较日历值时必须使用 `CoreTime()` 清除标记，更新日历值时必须由 `SetCoreTime` 保留标记。

全零 `CoreTime` 表示 `0000-00-00 00:00:00`；它与“月或日为零”的部分零日期不同。`Time::Check` 总是接受全零值，而部分零日期取决于 `ignore_zero_in_date`；非零日期的闰年/月日合法性可由 `ignore_invalid_date` 放宽。`ignore_zero_date` 字段保留在上下文标志中，但本文件当前没有读取它，扩展者不能假设该字段已经参与校验。

`Duration::Duration` 使用纳秒，但输入、输出和 CoreTime 最细仅到微秒；`Fsp` 决定显示/舍入的小数位，并在二元运算中取两侧最大值。`FromDate` 和 `NewTime` 是底层布局构造器，不替代 `Time::Check`：前者只掩码写位，后者还会 clamp FSP，因此不可信输入必须走 checked/parse 路径。

## 依赖与调用关系

编译接线为 `pkg/types/internal/time/lib.rs` → 本文件 → `pkg/types/lib.rs` 的 `time` 重导出。直接依赖为 `parser_mysql::type` 的 MySQL 类型码、同一内部 crate 定义的 `CoreTime`、`chrono`/`chrono-tz` 的日历和时区、`regex` 的时区后缀与复合 interval 数字提取、`rust_decimal` 的数值转换，以及 `serde` 的结构派生。`pkg/types/internal/time/Cargo.toml` 没有 feature 开关；根 `pkg/types/Cargo.toml` 只通过路径依赖聚合它。

核心内部调用边包括：`ParseTime` → `parseTime`；`parseTime` → `CheckFsp`、`GetTimezone`、`ParseDateFormat`、`parse_fraction`、`parse_digit_datetime_string`、`Time::Check`；`ParseDuration` → `matchDuration`，失败时可回退 `ParseTime` → `Time::ConvertToDuration`；`Time::Check` → `AdjustedGoTime` 并对 TIMESTAMP 做 UTC 范围判断；`Time::DateFormat` → day/week/name 辅助；`ExtractDurationValue` → `ParseDurationValue`/单单位解析。

RustCodeGraph 将本文件标为被 77 个文件使用。图查询确认 `ParseTime` 在本文件调用 `parseTime`，`ParseDuration` 调用 `matchDuration`、`canFallbackToDateTime`、`ParseTime` 和 `ConvertToDuration`；由于很多消费者经 crate 重导出和别名调用，callers 查询没有完整解析上游，故又用代码搜索核对了 `pkg/types/datum.rs`、`pkg/expression/builtin_cast_vec.rs`、`pkg/expression/builtin.rs`、`pkg/expression/planner_bridge.rs`、`pkg/lightning/backend/kv/canonical.rs` 和 `pkg/session/runtime/relational_value.rs` 的真实调用。

## 错误处理与边界

- `CheckFsp(-1)` 使用默认 0，其他负值报错，超过 6 的值降为 6。`NewTime`/`SetFsp` 则直接 clamp，调用方需要区分“校验入口”和“布局入口”。
- `GoTime` 对 DST 空洞返回错误，对歧义时间选择较早 instant；`AdjustedGoTime` 专供 TIMESTAMP 检查等兼容路径，在空洞中最多向前搜索 240 分钟。`ConvertTimeZone` 对全零时间直接成功，不尝试构造日历。
- TIMESTAMP 合法性以 `ctx.location()` 解释输入，再按 UTC 边界判断；显式时区后缀只允许 `Z` 或不超过 `±14:00` 的偏移。无效日期、无效偏移、chrono 无法表达的本地时间都返回 `TimeError`。
- `ParseDuration` 拒绝分钟/秒大于 59 和超过 `838:59:59` 的值；`Duration::Add` 同时检查 `i64` 溢出和 MySQL 范围。`TruncateOverflowMySQLTime` 是不同契约：返回截断后的边界值和一个可选错误。
- 小数秒最多保留 6 位并 half-up；进位可跨秒/日。`RoundFrac`、`parse_fraction`、字符串解析和数值解析必须维持同一 FSP 语义，尤其要覆盖负时长和 `9999995` 进位。
- `DateFormat`/`DurationFormat` 的末尾单独 `%` 报错；未知说明符输出说明符本身。`Extract*` 和 interval 分类对未知 unit 报错，`TimestampDiff` 对未知 unit 当前返回 0，这是需要保持或通过 Go 对照有意识修改的兼容边界。
- `StrToDate` 的公开返回值是成功布尔值：失败会把接收者重置为零 DATETIME；成功但有尾随内容通过 `append_warning` 通知，默认上下文的 warning 回调为空操作。

## 并发与资源生命周期

本文件没有异步任务、线程、锁、通道、事务或 I/O 资源。`Time`、`Duration`、上下文标志和 `CoreTime` 都是可复制值；修改型方法通过 `&mut self` 独占访问，其他运算通常返回新值。`MonthNames`、`WeekdayNames` 和格式表是只读静态数据。

唯一带初始化生命周期的全局对象是 `TZ_SUFFIX: LazyLock<Regex>`：首次调用 `GetTimezone` 时线程安全地编译一次，随后共享只读正则。`parse_composite_duration_value` 当前每次调用都会新建一个数字提取 `Regex`，而格式化与解析还会分配 `String`/`Vec`；这些临时值按 Rust 所有权自动释放。若优化热路径，应保持解析顺序、warning 和错误优先级，并以独立测试/基准证明没有改变 Go 兼容行为。

`CurrentTime`、`Duration::ConvertToTime` 和 `Duration::ConvertToYear` 会读取当前系统时间，因此结果依赖调用时刻；可复现逻辑应使用 `ConvertToTimeWithTimestamp` 和 `ConvertToYearFromNow` 注入明确的 `now`。

## 与 Go 版本的对应关系

直接对照是 `pkg/types/time.go`，行为测试对照是 `pkg/types/time_test.go`。Rust 保留了 Go 风格的公开命名、`Time`/`Duration` 字段语义、CoreTime 位布局、FSP 常量、两位年份规则、零日期策略、TIMESTAMP 范围、TIME 上限、packed 编码、DATE_FORMAT/STR_TO_DATE、EXTRACT 和 INTERVAL 入口。`pkg/types/time_test.rs` 以独立文件迁移了编码、日期/时间解析、算术、时区、格式化、溢出和数值转换用例；测试没有内嵌在生产文件中。

实现手段存在明确差异。Go 使用 `time.Time`/`*time.Location` 和包内 `Context`，Rust 使用 `chrono::DateTime<Tz>`、`chrono_tz::Tz` 与轻量 `TimeContext` trait；Go 的警告/错误处理接入更完整的语句上下文，Rust 通过 `append_warning` 抽象，默认实现会忽略 warning。Go `Time` 自定义 JSON 文本编解码，Rust 当前在 `Time`/`Duration` 上派生 `Serialize`/`Deserialize`，默认结构形状不等价于 Go 的字符串 JSON，不能把“派生可序列化”描述为已完成跨语言 JSON 兼容。

Rust 的 `ParseTime` 主流程较集中地使用 `chrono` 做日历进位和时区转换，但刻意保留部分零日期到 `Time::Check` 决策；若提前把所有输入转成 `NaiveDate`，会错误拒绝 Go 在放宽标志下允许的值。Go 的包级实现还包含更多细分辅助和错误类型，Rust 则统一为 `TimeError`。变更语义时必须同时核对两侧源文件和测试，而不能仅按 chrono 的自然行为推断 MySQL 规则。

## 扩展指南

- 新增日期时间输入形态时，从 `ParseTime` → `parseTime` → `parse_digit_datetime_string`/`ParseDateFormat` 这一链路接入；同步更新 `GetTimezone`、`GetFracIndex` 或 `parse_fraction` 时，要覆盖零日期、两位年份、显式偏移、FSP 进位、DST 空洞/歧义和目标类型。测试放在 `pkg/types/time_test.rs`，并以 `pkg/types/time_test.go` 的同类表为语义基准。
- 调整 SQL mode 行为时，集中审查 `TimeFlags`、`TimeContext`、`Time::Check` 和 `StrToDate` warning。特别注意 `ignore_zero_date` 当前未被消费；若要接线，应同时检查上游 context 适配器，不要仅修改字段默认值。
- 修改 CoreTime 布局或类型/FSP 标记时，必须把 `FromDate`、`NewTime`、`CoreTime`/`SetCoreTime`、`Type`/`SetType`、`Fsp`/`SetFsp`、packed 编解码及 `pkg/types/internal/time/lib.rs` 的 `CoreTime` 读取偏移视为一个整体，并验证编码兼容性。
- 扩展格式符或 week-mode 时，同步修改 `DateFormat`、`parse_str_to_date`、`GetFormatType` 及周算法；格式化和反向解析并非自动对称，应为每个新说明符分别添加成功、边界、未知/尾随输入测试。
- 调整 TIME/INTERVAL 时，联动 `matchDuration`、`Duration` 算术、`ParseDurationValue`、`ExtractDurationValue`、单位分类函数和 `MaxTime` 边界；复合单位的负号、微秒补零、跨日归一化和超界必须单独验证。
- 性能改动可优先缓存 `parse_composite_duration_value` 的正则或减少 `String`/`Vec` 分配，但不得跳过 Go 已覆盖的宽松解析、警告与错误优先级。日期/时区规则改动还应检查表达式 cast、Datum、Lightning 编码和会话 SQL 路径的代表性调用。

## 验证依据

- 源码全貌：`pkg/types/time.rs`（2812 行）；已盘点常量、`TimeError`、`TimeContext`/`TimeFlags`、`Time`、`Duration`、所有公开解析/格式/区间入口及内部日历辅助，文件没有 `#[cfg]` 条件项。
- 编译边界：`pkg/types/internal/time/lib.rs` 的 `#[path = "../../time.rs"] mod time_impl` 与 `pub use time_impl::*`；`pkg/types/internal/time/Cargo.toml` 的 `chrono`、`chrono-tz`、`regex`、`rust_decimal`、`serde`、`serde_json`、`parser-mysql`；`pkg/types/Cargo.toml` 和 `pkg/types/lib.rs` 的路径依赖与 `time` 重导出。目标包无 `pkg/types/doc.go`。
- Go 对照：`pkg/types/time.go`；Go 测试 `pkg/types/time_test.go`，覆盖编码、DATE/DATETIME/TIMESTAMP/TIME、FSP、时区、算术、EXTRACT、INTERVAL、STR_TO_DATE 与边界。
- Rust 测试：`pkg/types/time_test.rs`（独立测试文件，测试名与 Go 用例成组迁移）以及内部 crate 独立目标 `pkg/types/time_11_aster_unit_test.rs`；本任务按要求未运行 Cargo。
- RustCodeGraph：`status` 显示 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/types/time.rs` 显示本文件被 77 个文件使用；`callees ParseTime --file pkg/types/time.rs` 得到 `ParseTime` → `parseTime`，`callees ParseDuration --file pkg/types/time.rs` 得到其到 `matchDuration`、`canFallbackToDateTime`、`ParseTime`、`ConvertToDuration` 的边。
- 上游代码搜索：`pkg/types/datum.rs`、`pkg/expression/builtin_cast_vec.rs`、`pkg/expression/builtin_time_vec_generated.rs`、`pkg/expression/builtin.rs`、`pkg/expression/planner_bridge.rs`、`pkg/lightning/backend/kv/canonical.rs`、`pkg/session/runtime/relational_value.rs` 和 `pkg/ddl/persistent_masking_actions.rs`，证明该文件参与 SQL 值转换、表达式执行、导入编码和 DDL/会话路径。
- 本任务是纯文档分析；验收只运行任务指定的 11 章节结构检查，不运行 Cargo、代码构建或运行时测试。

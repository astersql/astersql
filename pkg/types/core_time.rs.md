# [`pkg/types/core_time.rs`](./core_time.rs)

## 文件定位

`pkg/types/core_time.rs` 是 MySQL 日历值的底层 Rust 实现，负责 `CoreTime` 的 64 位压缩字段访问、日期/周序计算、日期与时长混合、Go 风格时间转换以及内部 `TIMESTAMPDIFF` 计算。它不直接声明为 `pkg/types` 根 crate 的模块，而是由 `pkg/types/internal/core_time/lib.rs` 的 `types::core_time` 模块通过 `include!("../../core_time.rs")` 纳入 `astersql-types-core-time`，再由 `pkg/types/lib.rs` 以 `pub use types_core_time as core_time` 暴露。

`pkg/types/Cargo.toml` 通过路径依赖 `types-core-time = { package = "astersql-types-core-time", path = "internal/core_time" }` 建立这层边界。实际子 crate 依赖见 `pkg/types/internal/core_time/Cargo.toml`；其中 `chrono`、`chrono-tz` 和 `jiff` 由相邻的 `gotime` 兼容层使用，目标文件本身只显式导入 `crate::errors`，其余名称来自 include 位置的 `use super::*` 与 `use crate::gotime`。

该文件保存的是压缩时间和 MySQL 日历算法。面向 SQL 类型的完整 `Time`、解析、类型/FSP 和公开 `TimestampDiff` 另在 `pkg/types/time.rs`；二者不能互相替代。RustCodeGraph 将目标文件识别为被多个类型、表达式和会话侧文件使用的底层时间定义，同时精确查询也表明部分内部函数目前主要由本文件和独立测试覆盖。

## 核心职责

1. 用 `CoreTime(pub u64)` 保存年、月、日、时、分、秒和微秒，并按 include 宿主定义的 mask/offset 读取或覆写单个字段。
2. 提供 MySQL 兼容的日历算法：闰年/月末、日序号正反转换、星期、年内日、WEEK/YEARWEEK 的八种 mode 语义以及纯日期差。
3. 把压缩墙上时间转换为 `gotime::Time`，通过字段回读检测无效日期，并在 DST 跳时空洞中选择最近的真实时区边界。
4. 计算两个 datetime 或 datetime 与 `Duration` 的绝对秒/微秒差，支持日期与 duration 的合并和跨日进位。
5. 修正 Go `time.AddDate` 的日期归一化行为，使月末加减月份符合 MySQL（例如 1 月 31 日加一个月得到 2 月末），并限制溢出范围。
6. 实现内部 `timestampDiff`：年月季按“完整日历段”截断，其余单位按绝对秒/微秒差整除，并恢复方向符号。

## 主要符号

- `CoreTime(pub u64)`、`ZeroCoreTime`：可复制的压缩时间值及全零值。`String` 输出七个字段；`getYear`/`setYear` 到 `getMicrosecond`/`setMicrosecond` 按 `pkg/types/internal/core_time/lib.rs` 第 369 行起的位宽、偏移和 mask 操作，公开的 `Year`、`Month`、`Day`、`Hour`、`Minute`、`Second`、`Microsecond` 返回有符号整型视图。
- `CoreTime::{Weekday, YearWeek, Week, YearDay}`：日历派生值。`Week` 和 `YearDay` 对零月或零日返回 0；`YearWeek` 强制加入 `weekBehaviourYear`。
- `CoreTime::{GoTime, AdjustedGoTime}`：前者构造 `gotime::Time` 后逐字段回读，字段变化即返回值和错误；后者只对距离转换结果不超过四小时的时区跳变尝试最近边界修正。
- `isLeapYear`、`daysByMonth`、`GetLastDay`：格里高利闰年和月末基础表；非法月份的 `GetLastDay` 返回 0。
- `getFixDays`、`AddDate`：月末规则修正和受限日期偏移。`AddDate` 的每个增量限制为 `[-10000*365, 10000*365]`，结果年份还必须落在 `0..=9999`。
- `compareTime`、`datetimeToUint64`：先按 `YYYYMMDDHHMMSS` 比较，再比较微秒，返回 -1/0/1。
- `calcTimeDiffInternal`、`calcTimeTimeDiff`、`calcTimeDurationDiff`、`calcTimeFromSec`：统一的微秒精度差值与拆分链。差值返回 `(绝对秒, 余下微秒, 原值是否为负)`。
- `calcDaynr`、`DateDiff`、`calcDaysInYear`、`calcWeekday`、`getDateFromDaynr`：MySQL 日序算法及逆变换。`getDateFromDaynr` 对 `<=365` 或 `>=3652500` 返回 `(0,0,0)`。
- `weekBehaviour` 及 `weekBehaviourMondayFirst`、`weekBehaviourYear`、`weekBehaviourFirstWeekday`：WEEK mode 的三个位；`weekMode` 只取低三位，并在周日优先时翻转 FirstWeekday；`calcWeek` 处理年初归属上年、week 0 和年末归属下年。
- `mixDateAndDuration`：非负且不足 24 小时时只替换时钟字段；负值或至少 24 小时时走日序差值、跨日和反解日期路径。
- `intervalYEAR` 至 `intervalMICROSECOND`、`timestampDiff`：支持 YEAR、QUARTER、MONTH、WEEK、DAY、HOUR、MINUTE、SECOND、MICROSECOND；未知字符串当前返回 0。

文件没有 trait、枚举或条件编译项。目标文件内的上述符号大多声明为 `pub`，但 crate 装配、测试和更高层 `pkg/types/time.rs` 决定了实际稳定入口，不能仅凭可见性把所有辅助函数视为面向 SQL 层的公共契约。

## 执行流程

压缩字段流程从 `FromDate` 开始：宿主 `pkg/types/internal/core_time/lib.rs::FromDate` 按相同 mask/offset 把七个输入字段写入 `u64`，`CoreTime` 的 getter 反向提取；setter 总是先清目标位再写入，因此不会污染其他字段。该层只打包位宽，不执行完整日期合法性检查。

日历派生流程以 `calcDaynr` 为中心。`DateDiff` 对两个日期分别求日序再相减；`YearDay` 减去当年 1 月 1 日的日序；`calcWeek` 先得到目标日和元旦日序，再根据 MondayFirst、Year、FirstWeekday 三个位处理年初/年末跨年和 week 0/1-53；`getDateFromDaynr` 用估算年份、逐年校正、闰日修正和逐月扣减把日序还原。

时间差流程由 `calcTimeDiffInternal` 汇总：日期转换为日序，叠加时分秒，统一放大到微秒，应用 `sign` 后取绝对值并记录负号。`calcTimeTimeDiff` 传入另一个 `CoreTime` 的字段；`calcTimeDurationDiff` 先调用宿主的 `splitDuration`。`mixDateAndDuration` 对短正 duration 走快速字段替换，对其他情况把结果拆回时钟和日序日期。

时区转换流程为 `GoTime -> gotime::Date -> Date/Clock/Nanosecond 回读`。合法输入直接成功；零月、零日或越界字段被 `gotime` 归一化后与原值不同，因此携带规范化后的时间返回错误。`AdjustedGoTime` 收到错误后读取 `ZoneBounds`：若前后边界都离结果超过四小时，保留错误；否则返回距离较近的边界，距离相同时选 start。`Weekday` 特意忽略 `GoTime` 的错误，仍使用其归一化结果计算星期，与 Go 行为一致。

`AddDate` 先拒绝过大的年/月/日增量，再由 `getFixDays` 判断“加年月且 day 增量为 0”的月末偏移是否被 Go 归一化到下月；若是则以修正天数调用 `gotime::Time::AddDate`，最后检查结果年份。`timestampDiff` 先算 `t2-t1` 的绝对时间差；YEAR/QUARTER/MONTH 另按年月日和日内时刻扣除未满的月份，其他单位按秒数整除，最后乘方向符号。

## 数据与状态

`CoreTime` 是值类型，没有堆内所有权和隐藏状态。位布局由宿主定义：year 14 位、month 4 位、day/hour 各 5 位、minute/second 各 6 位、microsecond 20 位，最低 4 位未在本文件使用。setter 使用 mask 截断输入；因此调用者必须在更高层验证范围，不能把位宽容纳等同于 MySQL 合法日期。

`Duration` 来自 include 宿主，内部 `Duration: i64` 以微秒计、`Fsp` 保存精度；`splitDuration` 返回符号和绝对时分秒微秒。`gotime::Time` 则保存 UTC `NaiveDateTime` 与 `Location`，日期/时钟访问时转换为本地墙上时间。

文件内常量表和 interval 字符串均为不可变静态数据。所有计算使用局部变量或 `&mut CoreTime`，没有缓存、全局可变状态、事务或外部 I/O。重要不变量包括：比较先忽略微秒形成 14 位十进制日期时间，再单独比较微秒；时间差的 seconds/microseconds 始终非负，方向仅由 bool 表示；`timestampDiff` 只在最终结果处恢复符号。

## 依赖与调用关系

装配链为 `pkg/types/lib.rs -> types_core_time crate -> pkg/types/internal/core_time/lib.rs::types::core_time -> include!(pkg/types/core_time.rs)`。下游直接依赖包括：

- `CoreTime` getter 被本文件日历算法、`pkg/types/time.rs` 的高层 `Time` 以及表达式、会话、统计等代码使用；RustCodeGraph 对 `Year`、`Month`、`Day` 等查询显示了这些跨包调用者。
- `AdjustedGoTime -> GoTime -> gotime::{Date, Time::Date, Clock, Nanosecond}`；错误路径调用 `errors::New`，DST 路径调用 `ZoneBounds`、`Sub`、`Abs`、`Hours`。
- `Week/YearWeek -> weekMode/calcWeek -> calcDaynr/calcWeekday/calcDaysInYear`；这是文件内闭合的周序调用链。
- `DateDiff -> calcDaynr`；`compareTime -> datetimeToUint64`；`AddDate -> getFixDays/GetLastDay/gotime::Time::AddDate`。
- `mixDateAndDuration -> splitDuration` 或 `calcTimeDurationDiff -> calcTimeDiffInternal -> calcDaynr`，随后可能调用 `calcTimeFromSec/getDateFromDaynr`。
- `timestampDiff -> calcTimeTimeDiff -> calcTimeDiffInternal -> calcDaynr`，并调用各字段 getter。

RustCodeGraph `explore` 明确给出 Rust 边 `AdjustedGoTime -> GoTime`，以及 Go 对照中的 `DateDiff -> calcDaynr`、`calcWeek -> calcDaynr`、`timestampDiff -> calcTimeTimeDiff -> calcTimeDiffInternal -> calcDaynr`。精确符号查询找到 Rust `timestampDiff` 和测试 `core_time_2_timestamp_diff_keeps_go_int_width`；仓库文本核验未发现除目标文件及其两份直接测试外对 Rust `timestampDiff`、`AddDate`、`calcWeek` 等同名内部函数的直接调用。更高层 `pkg/types/time.rs::TimestampDiff` 当前有自己的实现，扩展时应先判断修改应落在内部兼容层还是正式 `Time` API，避免修一处而另一处漂移。

## 错误处理与边界

`GoTime` 和 `AdjustedGoTime` 返回 `(gotime::Time, Option<errors::SharedError>)`，即使失败也保留规范化时间供 `Weekday` 或 DST 判断使用。普通非法日期与 DST 空洞都先表现为回读不一致；只有靠近真实时区跳变边界的情况才由 `AdjustedGoTime` 消除错误。`pkg/types/core_time_test.rs::test_adjusted_go_time` 和 `pkg/types/internal/core_time/migration_aster_unit_test.rs::adjusted_go_time_matches_go_dst_boundaries` 覆盖半小时/一小时跳变、重叠时间和非法 2 月 31 日。

`AddDate` 对输入增量和结果年份分别防溢出，错误文本为 `datetime function overflow`。这与 Go 的结构化 `ErrDatetimeFunctionOverflow` 在错误类型/文本构造方式上并非完全相同，调用者不应依赖 Rust 错误的 Go 类型身份。测试覆盖正负边界、8,000 年导致结果越界和 `10001*365` 级输入。

`calcDaynr` 接受月 0 等 MySQL 内部值；`Week`/`YearDay` 则明确把零月/零日视为 0。`getDateFromDaynr` 的可还原范围是 366 到 3,652,499，边界之外返回零日期。`mixDateAndDuration` 延续 Go 注释所揭示的限制：负 duration 大于原日期的任意情况没有一般化保证，因此新增调用者不能假设它是任意带符号日期算术 API。

`timestampDiff` 对未知单位静默返回 0，不返回错误；合法单位必须由上层保证。YEAR/QUARTER/MONTH 使用无符号中间月份计数，但通过先按方向重排起止值避免正常有效日期下的负减法；若放宽输入域或日期合法性，必须重新审计下溢。微秒结果用 `i64`，测试覆盖 MySQL datetime 全范围的正负差值。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、句柄或长生命周期资源。`CoreTime`、`Duration`、`gotime::Location` 和 `gotime::Time` 都按值传递；唯一原地修改发生在 `CoreTime` setter、`calcTimeFromSec` 和 `mixDateAndDuration` 接受的独占 `&mut CoreTime` 上，Rust 借用规则排除了同时写入。

时区规则查询由相邻 `gotime::Time::ZoneBounds` 临时完成，目标文件不缓存规则或持有外部资源。函数的并发安全性因此取决于纯值输入和 `gotime`/时区库的只读规则数据，没有本地清理步骤。性能敏感点主要是 `GoTime` 的完整时区转换、`AdjustedGoTime` 的边界查询，以及 `getDateFromDaynr` 的小范围逐年/逐月循环，而不是同步竞争。

## 与 Go 版本的对应关系

主对照文件是 `pkg/types/core_time.go`，Rust 基本按同一顺序移植了 `CoreTime` 字段访问、Week/YearWeek、GoTime/DST 修正、AddDate、时间差、daynr、week、duration 混合和 timestampDiff。`pkg/types/core_time_test.go` 的表格也被 `pkg/types/core_time_test.rs` 对应覆盖，包括周 mode、月 0 时间差、比较反对称性、daynr 反解、跨日 duration、世纪闰年、月末修正、溢出和 DST。

明确差异如下：

- Go 的 `CoreTime` 是 `uint64` 新类型并通过指针/unsafe 风格更新底层值；Rust 是 tuple struct，以安全 mask 操作修改 `self.0`。
- Go 返回 `error`，Rust 返回 `Option<SharedError>`；Rust 当前构造通用字符串错误，没有复刻 Go dbterror 的完整类型信息。
- Go 的 `*time.Location`/`time.Time` 由标准库提供；Rust 的 `gotime` 是 `internal/core_time/lib.rs` 基于 `chrono`、`chrono-tz`、`jiff` 的兼容层，DST 语义由独立 migration 测试锁定。
- Go 的平台 `int/uint` 中间值在 Rust 中被固定为 `i32/u32/i64`；`core_time_2_timestamp_diff_keeps_go_int_width` 特别验证完整 datetime 范围的 DAY/SECOND/MICROSECOND 不被窄化。
- Rust 许多原 Go 私有辅助被声明为 `pub` 以跨 include/crate 测试和接线，但这不表示语义被扩展。
- `pkg/types/time.rs` 还存在正式高层 `Time` 的独立实现及 `TimestampDiff`，说明迁移期存在相邻算法副本；对齐 Go 行为时需要同步核验，而不能假设本文件自动支配全部 SQL 时间行为。

## 扩展指南

- 修改位布局或新增字段时，必须同步 `pkg/types/internal/core_time/lib.rs` 的 offset/width/mask、`FromDate`、目标文件 getter/setter/String，并在独立测试文件中增加打包往返与最大值测试。不要把测试写回生产源文件。
- 修改周序、daynr 或零日期规则时，优先扩展 `pkg/types/core_time_test.rs` 的表格，并逐项对照 `pkg/types/core_time_test.go`；特别覆盖年初/年末、闰世纪、mode 0..7、week 0/53 和 daynr 范围边界。
- 修改时区转换时，应同时扩展 `pkg/types/core_time_test.rs::test_adjusted_go_time` 与 `pkg/types/internal/core_time/migration_aster_unit_test.rs`，覆盖非一小时跳变、重叠区间、无跳变固定时区和普通非法日期。四小时阈值与“最近边界、平局选 start”是兼容契约。
- 修改 `AddDate` 时，保持 MySQL 月末语义与输入/结果两层溢出检查；用 1/31、闰年 2 月、负月份和 0/9999 年附近案例验证。若改变错误类型，还需检查上层是否匹配具体错误。
- 修改差值或 `timestampDiff` 时，同步 `pkg/types/core_time_2_aster_unit_test.rs` 的全范围 `i64` 用例，并核对 `pkg/types/time.rs::TimestampDiff` 是否也需一致改动。新增单位时不能只增加字符串常量，还需定义截断规则、未知单位策略和上层解析约束。
- 修改 `mixDateAndDuration` 时，应先明确是否扩大到“负时长超过日期”的范围；现有实现和 Go 注释都未保证该域，若扩大必须增加独立回归测试而不是依赖整数转换偶然行为。
- 性能优化应保留逐字段回读验证、微秒精度和符号截断语义；不要用直接比较 packed `u64` 替换现有比较，除非证明保留位、字段范围和微秒顺序完全等价。

## 验证依据

- 源码与装配：`pkg/types/core_time.rs`（701 行）、`pkg/types/internal/core_time/lib.rs`（`gotime`、位域常量、`Duration`、`splitDuration`、`FromDate`、`include!`）、`pkg/types/internal/core_time/Cargo.toml`、`pkg/types/Cargo.toml`、`pkg/types/lib.rs`。
- Go 对照：`pkg/types/core_time.go`（639 行）和 `pkg/types/core_time_test.go`（348 行）。
- Rust 独立测试：`pkg/types/core_time_test.rs`（457 行）、`pkg/types/core_time_2_aster_unit_test.rs`（其中 CoreTime 相关测试为第 23-190 行）、`pkg/types/internal/core_time/migration_aster_unit_test.rs`（155 行）。后者补充完整 IANA 边界与固定时区证据；`core_time_2_aster_unit_test.rs` 第 192 行后的 `ComputePlus` 测试与本文件职责无关，未据此扩张结论。
- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。`node --file pkg/types/core_time.rs` 核对了完整 701 行；`query` 找到 `pkg/types/core_time.rs::CoreTime`、Rust `timestampDiff`、宿主 `FromDate` 与 `splitDuration`；`explore` 核对了 `AdjustedGoTime -> GoTime` 以及 daynr/week/timestampDiff 调用链和字段 getter 的跨模块调用者。单独的精确 `callers` 命令在本地索引上超时，故对未清晰分离同名 Go/Rust 符号的调用点又用限定 `rg` 核验，并在“依赖与调用关系”中按已验证范围描述。
- 行为边界由测试事实支持：零日期/daynr、WEEK mode、月末 AddDate、溢出、微秒宽度、全 datetime 范围、DST 空洞和非法日历日均有独立用例；本任务是纯文档分析，按计划未运行 Cargo。

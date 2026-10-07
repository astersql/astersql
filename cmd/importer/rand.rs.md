# `cmd/importer/rand.rs`

## 文件定位

`cmd/importer/rand.rs` 属于 `astersql-cmd-importer` crate，由 [`cmd/importer/lib.rs`](lib.rs) 以 `pub mod rand` 装配。该 crate 的 manifest 是 [`cmd/importer/Cargo.toml`](Cargo.toml)：库入口为 `lib.rs`，二进制入口为 `bin_main.rs`，移植元数据把它标记为 Go `cmd/importer` 对应的 binary crate。本文件只依赖同 crate 的 `data`、`parser`、`stats`（通过 `column.hist`）和 `stubs`，没有直接使用 manifest 中的 `toml`、`serde_json` 外部依赖。

它处在“列定义已经解析、INSERT 字面量即将生成”的中间层。生产调用者是 [`cmd/importer/db.rs`](db.rs) 的 `genColumnData`：当列不是 `incremental` 时，`TypeDate`、`TypeDatetime | TypeTimestamp`、`TypeDuration`、`TypeYear` 分支分别调用 `randDate`、`randTimestamp`、`randTime`、`randYear`，再由 `db.rs` 加单引号形成 SQL 文本。本文件自身返回不带 SQL 引号的已格式化字符串。

## 核心职责

本文件为四类时间列选择采样来源并生成 Go 布局格式的文本：

1. `column.hist` 存在时优先委托 `histogram::randDate`，尽量保留已有统计分布。
2. 无直方图但有足够边界时，解析边界并在闭区间中随机选择偏移。
3. 边界缺失时，按各类型的 Go 兼容默认规则生成当前年份附近或全天范围内的值。

它还公开 Go 同名布局常量 `yearFormat`、`dateFormat`、`timeFormat`、`dateTimeFormat`，并从 `data.rs` 再导出随机字符串所用的 `alphabet`、`letterIdxBits`、`letterIdxMask`、`letterIdxMax`。字符串随机算法本身不在这里，实际实现位于 [`cmd/importer/data.rs`](data.rs)。

## 主要符号

- `yearFormat: &str`：值来自 `stubs::YEAR_FORMAT`，即 Go 年份布局 `2006`。
- `dateFormat: &str`、`timeFormat: &str`、`dateTimeFormat: &str`：分别为 `2006-01-02`、`15:04:05`、`2006-01-02 15:04:05`；既是输出契约，也是传给直方图格式化路径的布局。
- `go_zero_time() -> CivilTime`：内部辅助函数，显式构造 Go `time.Time` 零值所对应的 `0001-01-01 00:00:00`，供解析失败回退。
- `randDate(&column) -> String`：生成 `YYYY-MM-DD`。无最小值时使用当前年份、1..=12 月、1..=28 日；只有最小值时在其后 0..=365 天取样；双边界时按日差闭区间取样。
- `randTime(&column) -> String`：生成 `HH:MM:SS`。任一边界缺失便在全天的时、分、秒合法范围独立取样；双边界时按秒差闭区间取样。
- `randTimestamp(&column) -> String`：生成 `YYYY-MM-DD HH:MM:SS`。缺最小值时在当前年份的安全日期范围和全天范围取样；只有最小值时向后取 0..=365 天；双边界时按秒差使用 `randInt64` 取样。
- `randYear(&column) -> String`：生成四位年份。任一边界缺失时从当前年向前 0..=10 年取样；双边界时解析为年初时间并按整个秒区间取样，最后只输出落点年份。
- `_fmt_consts()`：带 `#[allow(dead_code)]` 的内部编译引用，收口 `date_format_go`、`time_format_go`、`datetime_format_go`、`randInt64`、`randString` 的引用；不在运行主链上，也不改变状态。

本文件没有 trait、结构体、`impl` 或条件编译项。四个 `rand*` 函数和四个格式常量是公开 API；`go_zero_time`、`_fmt_consts` 是私有实现。

## 执行流程

四个入口共享相同的第一步：若 `col.hist.as_ref()` 为 `Some`，立即调用 [`cmd/importer/stats.rs`](stats.rs) 的 `histogram::randDate(unit, mysqlFmt, dateFmt)` 并返回。参数分别是 DATE 的 `DAY/%Y-%m-%d`、TIME 的 `SECOND/%H:%i:%s`、TIMESTAMP 的 `SECOND/%Y-%m-%d %H:%i:%s`、YEAR 的 `YEAR/%Y`。

无直方图时，流程如下：

- `randDate`：复制 `min/max`；最小值空则生成当前年内的安全日期；否则解析最小值。最大值空则从最小值向后加随机天数；最大值存在则解析它，以 `timestamp_diff("DAY")` 得到跨度，再加闭区间随机天数。
- `randTime`：复制 `min/max`；只要一端为空就忽略另一端并生成全天随机时间。两端都有值时分别解析，以 `timestamp_diff("SECOND")` 求跨度并加随机秒数。
- `randTimestamp`：与 `randDate` 的分支形状相同，但双边界跨度和偏移均使用 `i64` 秒；缺最小值的默认分支还生成时、分、秒。
- `randYear`：任一端为空就使用“当前年减 0..=10”；双边界时把年份解析成 1 月 1 日零点，以秒为单位选择落点，再仅格式化其年份。

`data.rs::randInt` 与 `randInt64` 都采用 `max - min + 1`，所以上述显式边界以及 365 天、10 年等默认窗口均为闭区间。最后的 SQL 引号由 `db.rs::genColumnData` 添加，不属于本文件职责。

## 数据与状态

输入 `parser.rs::column` 中，本文件实际读取 `min: String`、`max: String`、`hist: Option<Arc<histogram>>`。`min/max` 来自列 comment 规则的 `range` 解析；本文件通过 `clone` 得到局部字符串，不修改列。`idx`、`name`、`data`、`tp`、`comment`、`incremental`、`set` 在这里不读取；其中 `incremental` 已由上游 `db.rs` 决定是否绕过随机入口。

中间时间值使用 `stubs.rs::CivilTime`，它是无时区的 civil 字段集合。`CivilTime::now()` 当前由 Unix 秒按 UTC 形状换算，并非完整本地时区实现；因此“当前年份”以该桩的当前实现为准。解析、日/秒差、加日/加秒也都由 `stubs.rs` 完成。

本文件不缓存数据、不持有数据库连接，也不改变 `column` 或 `histogram`。唯一隐式可变状态是随机源：`data.rs` 最终调用 `stubs.rs` 中线程局部的 `RNG: Cell<u64>`。因此每个线程拥有独立序列，`seed_rng` 只影响调用它的线程。

## 依赖与调用关系

上游生产调用链为：`db.rs::genColumnData` → 本文件四个 `rand*` 入口 → 生成裸时间字符串 → `genColumnData` 包装成带引号 SQL 字面量。RustCodeGraph 的文件节点确认 `rand.rs` 被 `rand_test.rs` 引用；精确源码搜索进一步确认生产调用点位于 `db.rs` 的 DATE、DATETIME/TIMESTAMP、DURATION、YEAR 类型分支。

下游关系为：

- `column.hist` 存在：调用 `stats.rs::histogram::randDate`。该方法先随机选择边界索引；偶数索引计算上下界差，零差直接格式化下界，否则选择偏移；奇数索引直接格式化桶边界。必须注意当前实现无论 `unit` 是 `DAY`、`SECOND` 还是 `YEAR`，偶数桶最终都调用 `CivilTime::add_days(delta)`，这是现有行为而不是通用单位换算。
- 边界路径：调用 `stubs::{parse_date, parse_time_of_day, parse_datetime, parse_year, timestamp_diff}` 和 `CivilTime::{add_days, add_seconds}`。
- 随机路径：调用 `data::{randInt, randInt64}`；二者再调用 `stubs::{rand_intn, rand_int63n}`。
- 告警路径：解析失败调用 `stubs::log_warn` 向标准错误写 `[WARN] ...`。

再导出的随机字符串常量服务于 crate 内其他 Go 对齐代码，但四个时间入口本身不消费这些常量。

## 错误处理与边界

四个入口返回 `String` 而不是 `Result`。边界解析失败不会向上传播：函数记录告警，并把失败的那一端替换为 `go_zero_time()`。独立测试 [`cmd/importer/rand_test.rs`](rand_test.rs) 验证双端均为 `invalid` 时，四个入口分别返回 `0001-01-01`、`00:00:00`、`0001-01-01 00:00:00`、`0001`。

边界顺序没有在本文件校验。若求得负跨度，`randInt`/`randInt64` 会把非正长度传给 `stubs::rand_intn`/`rand_int63n`，其中 `assert!(n > 0)` 会 panic；调用方必须保证最大值不早于最小值。`randDate` 和 `randTime` 还把跨度转为 `i32`，极宽范围存在截断风险；`randTimestamp` 和 `randYear` 使用 `i64`，测试覆盖了 1900 到 2000 的跨度，防止退回窄整数实现。

默认日期把日限制为 1..=28，因此不会生成无效月份日期。解析桩只检查字段形状和数字转换，没有验证月份、日期、时分秒的完整日历合法性；不要把成功解析等同于已完成 SQL 时间语义校验。直方图路径可能通过 `stubs::fatal` panic，且本文件不捕获。随机数是伪随机且存在取模实现，不适合安全用途；源文件的 `#nosec G404` 也表明其用途仅为造测试/导入数据。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、文件或网络资源，所有 `CivilTime` 和输出 `String` 都是单次调用内的局部值。`column.hist` 是 `Arc` 引用，但这里只借用并调用，不改变其所有权；时间采样也不触碰 `histogram` 中为字符串平均长度准备的互斥缓存。

随机状态由 `stubs.rs` 的 `thread_local! RNG: Cell<u64>` 管理，调用无需跨线程锁；代价是不同工作线程默认从同一个常量种子开始时可能产生相同序列，除非分别播种。告警同步写标准错误，但没有由本文件管理的句柄生命周期。

## 与 Go 版本的对应关系

直接对照文件是 [`cmd/importer/rand.go`](rand.go)。Rust 保留了四个函数名、四个布局常量、直方图优先级、缺边界分支、格式宽度、闭区间随机选择以及解析失败后继续使用 Go 零时间的可观察语义。`cmd/importer/db.go` 与 `db.rs` 也都在相同 SQL 类型分支调用这些入口。

主要实现差异是：Go 直接使用 `time.Time`、`time.Parse`、`time.Now` 和包级 `math/rand`；Rust 使用轻量 `CivilTime`、手写解析/日期运算以及线程局部 xorshift 随机桩。Rust 的 `CivilTime::now()` 按 UTC 近似墙钟，而 Go `time.Now()` 使用本地位置；时区边界附近的默认年份可能不同。Go 的 `randTimestamp`/`randYear` 中 `int` 宽度随平台，Rust 明确使用 `i64` 避免长秒跨度缩窄；`rand_test.rs::temporal_ranges_wider_than_i32_seconds_match_go_int_range` 为此提供回归证据。

Go 文件还定义 `randInt`、`randInt64`、`randString` 与字符表常量；Rust 将实现集中迁到 `data.rs`，本文件只导入函数并再导出常量。这是模块拆分差异，不改变调用契约。

## 扩展指南

- 新增时间类型或采样来源时，应在对应 `rand*` 入口保持“直方图优先、显式边界其次、默认范围最后”的顺序，并同步检查 `db.rs::genColumnData` 的类型分派。
- 修改格式时需同时核对本文件常量、`stubs.rs::CivilTime::{Format, DateFormat}`、`stats.rs::histogram::randDate` 参数以及 Go `rand.go`；否则直方图路径与边界路径可能产生不同文本。
- 修改边界策略时应明确闭区间、不完整边界、反向范围、解析失败和超 `i32` 秒跨度的行为。回归测试必须放在独立的 [`cmd/importer/rand_test.rs`](rand_test.rs)，不要内嵌进生产源文件；至少覆盖四个入口、直方图与无直方图路径、单边界/双边界、非法值和反向范围预期。
- 若修正 `histogram::randDate` 对 SECOND/YEAR 仍使用 `add_days` 的现状，修改点属于 `stats.rs`，必须先与 Go `stats.go` 及其独立测试确认是否是刻意对齐，不能只在本文件绕过。
- 若需要真实本地时区或跨线程随机独立性，应修改 `stubs.rs` 的时间/随机抽象并评估整个 importer，而不是在某个 `rand*` 函数内增加特例。

兼容风险主要是输出格式、范围端点和失败回退漂移；正确性风险主要是反向范围 panic、宽跨度转 `i32`、轻量解析未做完整日历校验；性能风险较低，每次调用只做常数次解析、随机数和字符串格式化，直方图路径的代价由 `stats.rs` 决定。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 7032 个 Rust 文件；查询 `randDate`、`randTime`、`randTimestamp`、`randYear` 定位到本文件及 Go/统计同名符号。
- RustCodeGraph `node --file cmd/importer/rand.rs`：核对 243 行完整源文件、公开/私有符号和 `rand_test.rs` 引用。
- RustCodeGraph `explore "cmd/importer/rand.rs randDate randTime randTimestamp randYear callers callees"`：确认独立测试对四个入口的调用，并区分 Go `rand.go`、Rust `rand.rs` 与 `stats.rs` 的同名 `randDate`。
- RustCodeGraph 文件节点：核对 `db.rs::genColumnData` 的四个生产调用分支、`parser.rs::column` 字段、`stats.rs::histogram::randDate`、`data.rs::{randInt, randInt64}`、`stubs.rs` 的解析/差值/日期运算实现。
- 直接读取 [`cmd/importer/Cargo.toml`](Cargo.toml)、[`cmd/importer/lib.rs`](lib.rs)、[`cmd/importer/rand.go`](rand.go)、[`cmd/importer/rand_test.rs`](rand_test.rs)；用精确源码搜索确认 `db.rs` 与 `db.go` 的调用点。Cargo 与 Go 文件不属于 RustCodeGraph Rust 源节点的完整 crate/对照证据，因此单独核验。
- 按任务约束未运行 Cargo；这是纯文档分析。结构验证要求本文恰好包含任务规定的 11 个二级标题。

# `pkg/util/importer/rand.rs`

## 文件定位

本文件属于独立库 `astersql-util-importer`。[`Cargo.toml`](Cargo.toml) 指定库入口为 [`lib.rs`](lib.rs)，普通 `[dependencies]` 为空，因此这里的随机数、时间和公历换算只依赖标准库，以及同 crate 的 [`config.rs`](config.rs) 中 `ImporterError`。`lib.rs` 以 `pub mod rand` 装入模块并通过 `pub use rand::*` 重导出公开项。

它位于导入数据生成链的底层：[`db.rs`](db.rs) 的 `generate_column_data` 为非唯一整数、字符串、DATE、TIME、DATETIME/TIMESTAMP 和 YEAR 列调用本文件；`generate_row_data`、`generate_row_data_batch` 再将这些值组装成 INSERT。[`data.rs`](data.rs) 还复用 `civil_from_days`、`days_from_civil` 来生成唯一时间类值。它不负责 SQL 解析、唯一性状态或数据库 I/O。

## 核心职责

- 用进程级 `AtomicU64` 状态和线性同余生成器（LCG）提供无锁伪随机位；`seed` 允许测试固定序列，`random_u64` 是所有随机 API 的共同底座。
- 提供闭区间整数、`usize`、布尔、base62 字符串和不超过上限的 `Duration` 生成函数。
- 将导入配置中的字符串上下界解析为 DATE、TIME、TIMESTAMP、YEAR 范围，随机取值后格式化为 MySQL 风格文本。
- 用 Howard Hinnant civil-date 算法在 Unix epoch 日数和公历年月日之间转换；这两个 crate 内函数也供 `data.rs::Datum` 使用。
- 把 Rust 中可检测的非法范围或解析失败统一转换为 `ImporterError::InvalidRange`，而不是沿用 Go 对照实现忽略解析错误的方式。

## 主要符号

- `ALPHABET: &[u8]`：公开的 62 字节数字、大小写字母表。`rand_string` 和 `data.rs::Datum::unique_string` 共用它。
- `RANDOM_STATE: AtomicU64`：私有全局 LCG 状态；`0` 是“未初始化”标记。
- `seed(u64)`：公开设种入口，把 `0` 规范为 `1`，以 `Release` 写入全局状态。
- `random_u64() -> u64`：私有 LCG 步进。乘数为 `6_364_136_223_846_793_005`、增量为 `1_442_695_040_888_963_407`，用 wrapping 算术和 `compare_exchange_weak` 重试。
- `rand_i64(minimum, maximum)`、`rand_usize(minimum, maximum)`：公开闭区间采样；前者用 `i128/u128` 计算跨度以覆盖完整 `i64` 差值，后者直接用 `usize` 算术。
- `rand_float64`、`rand_bool`、`rand_string`、`rand_duration`：公开的派生随机值 API。`rand_float64` 先采整数，再按指定小数位格式化并解析；它不会产生真正的非整数小数。
- `civil_from_days`、`days_from_civil`：`pub(crate)` 的互逆日历换算；不是 crate 外 API。
- `now_seconds`、`parse_date`、`parse_time`、`format_date`、`format_time`：私有时间辅助函数。
- `rand_date`、`rand_time`、`rand_timestamp`、`rand_year`：公开字符串时间类型生成 API，均返回 `Result<_, ImporterError>`。

文件没有类型、trait、`impl` 或条件编译项；测试通过 `lib.rs` 的 `#[cfg(test)] mod rand_test` 独立编译，未嵌入生产源文件。

## 执行流程

1. 调用者通常从 `db.rs::generate_column_data` 进入。唯一列转给 `data.rs::Datum`；只有非唯一列进入这里的随机路径。整型列经 `random_integer` 调 `rand_i64`，枚举集合与字符串长度经 `rand_usize`，时间类型直接调用对应 `rand_*`。
2. 每次随机采样最终调用 `random_u64`。它读取原子状态，计算一次 LCG 后继值，并以 CAS 写回；竞争失败时用实际状态重新计算，直到成功。因此每个成功调用获得全局序列中的一个状态值。
3. `rand_i64` 先拒绝反向区间，再把闭区间宽度扩展到 128 位，使用 `random_u64() % width` 映射并加回下界。其他简单类型用相同随机字：最低位生成布尔，模字母表长度逐字符生成字符串，模纳秒上限加一生成时长。
4. DATE 路径：无下界时取当前 UTC epoch 对应年份，再随机 1..=12 月和 1..=28 日；有下界时解析成 epoch 日数，无上界则默认下界后 365 日，否则解析上界，最后在闭区间采样并格式化。
5. TIME 路径：任一边界为空时忽略另一边界，在 0..=86,399 秒中采样；两端都有值时解析为日内秒数，在闭区间采样，格式化时用 `rem_euclid(86_400)` 限制到一天。
6. TIMESTAMP 路径：下界为空时分别调用缺省 DATE/TIME；下界存在时解析为 `日数 * 86,400 + 日内秒`。上界为空时随机增加 0..=365 个整日，因而保留下界时钟；两端存在时则在总秒数闭区间内采样。
7. YEAR 路径：任一边界为空时返回当前 UTC 年减 0..=10；两端存在时先映射到各年 1 月 1 日的 epoch 秒，再在整个秒区间采样，最后只输出采样时刻所属年份。

## 数据与状态

唯一可变共享状态是 `RANDOM_STATE`。所有 API 共用同一序列，所以一次调用消耗的随机字会改变后续结果；例如 `rand_string(n)` 消耗 `n` 次，缺省 `rand_date` 消耗两次。`seed` 是进程级操作，不是调用者私有 RNG；测试若并行设种会相互影响。

原子内存序为读取 `Acquire`、设种 `Release`、成功 CAS `AcqRel`、失败 CAS `Acquire`。这保证状态更新在线程间可见，但不提供一组多次采样的事务性快照。LCG 和模映射均不是密码学安全随机；模数不整除 `2^64` 时存在模偏差。

需特别注意当前惰性初始化的实际代码：初次读取到 `0` 时，函数只把本地变量替换为时间纳秒奇数，随后 CAS 的 expected 已不是原子中的 `0`，第一次必然失败；失败返回的实际值又把本地变量设回 `0`，下一轮从 `0` 计算并成功写入固定增量。因此未显式 `seed` 的首次结果并未实际采用时间种子。本文只记录现状，不把注释中的“用时间种子初始化”当作已实现保证。

时间计算以 `SystemTime::duration_since(UNIX_EPOCH)` 为基准；系统时间早于 epoch 时 `unwrap_or_default` 回退到零。无时区对象参与，当前年份按 UTC epoch 日计算。日期、时间和时间戳在内部用 `i64` 日数/秒数表示，格式化为固定宽度文本。

## 依赖与调用关系

RustCodeGraph 的符号轨迹确认：

- `rand_i64` 调用 `random_u64` 和 `ImporterError::InvalidRange`；其直接下游调用者包括 `rand_float64`、`rand_date`、`rand_time`、`rand_timestamp`、`rand_year`。
- `rand_timestamp` 调用 `parse_date`、`parse_time`、`rand_i64` 与 `InvalidRange`；独立测试 `rand_test.rs::timestamp_without_maximum_preserves_minimum_clock_like_go` 直接调用它。
- `generate_column_data` 由 `generate_row_data` 调用；源码进一步显示它经 `random_integer`/`rand_usize`/各时间函数接入本模块。`generate_row_data_batch` 再被导入 job 流程消费。
- `civil_from_days` 在本文件内由 `format_date`、`rand_date`、`rand_year` 调用；`days_from_civil` 由 `parse_date`、`rand_year` 调用。`data.rs` 通过 crate 内可见性直接复用两者。
- `lib.rs` 的公开重导出使所有 `pub` 函数成为 `astersql-util-importer` 的 crate 根 API；`civil_from_days`/`days_from_civil` 仍仅限 crate 内。

外部依赖仅为 Rust 标准库的原子类型与时间类型。`Cargo.toml` 的普通依赖为空；`cfg(any())` 下列出的 parser/dbutil 依赖永不启用，不能作为本文件运行时依赖来描述。

## 错误处理与边界

- `rand_i64`、`rand_usize` 明确拒绝 `minimum > maximum`，错误文本保留 `minimum..=maximum`；相等边界返回该唯一值。
- `parse_date` 只要求恰有三个可解析的 `i32` 分段，`parse_time` 只要求三个可解析的 `i64` 分段。它们不验证真实日历范围（如月份 1..=12、合法日数）或时分秒范围；换算和 `format_time` 会对这些数值做算术归一化，而非报告语义非法。
- 有界 DATE/TIME/TIMESTAMP 最终由 `rand_i64` 检查上下界顺序。TIMESTAMP 还要求每个非空边界恰有一个可由 `split_once(' ')` 找到的日期/时钟分隔点。
- `rand_usize` 的 `maximum - minimum + 1` 在完整 `0..=usize::MAX` 区间会溢出；当前生产调用的集合索引与字符串长度不使用该极端范围，但扩展 API 时不能忽略。
- `rand_duration` 将上限纳秒截到 `u64::MAX`；在该极值下 `saturating_add(1)` 仍是 `u64::MAX`，模运算不会产生精确的 `u64::MAX` 纳秒端点。普通较小上限则是闭区间。
- `rand_float64` 的解析理论上可能映射为 `InvalidRange(rendered)`；由有限 `i64` 转成十进制 `f64` 的正常路径通常可解析。精度只影响十进制往返，不增加随机小数部分。
- 所有整数映射使用取模而非拒绝采样，分布接近均匀但并非严格无偏；不要把这些 API 用于安全令牌或公平性敏感逻辑。

## 并发与资源生命周期

LCG 状态静态存活整个进程，无堆资源、文件句柄、任务或通道需要回收。CAS 循环避免互斥锁，单次调用在竞争下可能重试，但没有阻塞等待或锁中毒。

线程安全只覆盖“每次状态推进不丢失”。复合操作（如一次 DATE 的月、日两次采样，或一条记录的多个字段）可能与其他线程的随机调用交错，不能依赖连续子序列。运行中调用 `seed` 会立即替换全局状态，也可能与正在 CAS 的调用竞争；该调用最终会依据 CAS 观察到的最新状态继续，但确定性测试应隔离进程或避免并行设种。`rand_test.rs::bounded_year_samples_elapsed_seconds_like_go` 正是另起当前测试二进制的子进程，以避免其他测试消耗全局 RNG 序列。

系统时钟仅在惰性初始化、`now_seconds` 调用时读取，不缓存。日期输出没有时区资源或夏令时状态；它是纯 UTC epoch 算术。

## 与 Go 版本的对应关系

直接对照文件是 [`rand.go`](rand.go)。Rust 的 `rand_i64`/`rand_usize`、`rand_float64`、`rand_bool`、`rand_string`、`rand_duration`、四类时间函数分别承接 Go 的 `randInt64`/`randInt`、`randFloat64`、`randBool`、`randString`、`randDuration`、`randDate`/`randTime`/`randTimestamp`/`randYear` 的意图；`db.go` 与 `db.rs` 证明它们在相同列类型生成分支中使用。

关键保持一致的语义包括：范围是闭区间；缺省日期只生成当前年 1..12 月、1..28 日；缺省 TIME 覆盖一天；TIMESTAMP 只有下界时增加整数天并保留下界时钟；YEAR 任一边界为空时取当前年往前十年；有界 YEAR 不是直接均匀选年份，而是在两个元旦之间按经过秒数取样。后两点由 `rand_test.rs` 的独立回归测试锁定。

实现差异也必须保留在认知中：Go 使用全局 `math/rand`，Rust 自带原子 LCG 且暴露 `seed`；Go `randString` 从一个 `Int63` 缓存提取多个 6 位索引并跳过 62、63，Rust 每字符消耗一个 `u64` 并直接 `% 62`，所以同一种子不承诺同字符串序列；Go 时间解析丢弃 error，Rust 返回 `InvalidRange`；Go 借助 `time.Time`，Rust用整数 civil-date 算法且无时区；Rust 的 `rand_time` 在任一边界缺失时一次采样日内总秒，而 Go 分别采样时、分、秒，输出范围相同但序列不同。

## 扩展指南

- 新增随机列类型时，优先在本文件提供“解析/校验、内部数值区间、格式化”三层清晰接口，再从 `db.rs::generate_column_data` 的对应 `FieldKind` 分支接线；不要让 SQL 字面量拼装进入本文件。
- 修改日期或 YEAR 语义时，同时检查 `data.rs::Datum` 对 `civil_from_days`/`days_from_civil` 的使用，并对照 `rand.go`、`db.go`。这两个换算函数是共享基础设施，不只服务随机路径。
- 修复或替换 RNG 时，应先确定是否需要保留 `seed` 的确定序列、并发线性化和 Go 的分布语义。若追求严格均匀，应用拒绝采样消除模偏差；若修复惰性时间设种，须增加独立进程回归，避免已有全局状态掩盖首次调用行为。
- 强化时间输入校验时，要明确兼容策略：当前实现接受可解析但超出日历/时钟范围的字段，改变它会影响用户配置错误行为。回归测试应放在独立 [`rand_test.rs`](rand_test.rs)，不要嵌入 `rand.rs`。
- 扩展极端整数或 Duration 区间前，先处理 `rand_usize` 全宽溢出与 `rand_duration` 最大纳秒端点；测试应覆盖相等上下界、反向区间、epoch 前后、闰日、缺失单边界及并发设种/采样。
- 生产调用链行为还应同步检查 [`db_test.rs`](db_test.rs)；civil-date 的 crate 内消费者由 [`data_test.rs`](data_test.rs) 覆盖。测试必须继续与源文件分离。

## 验证依据

- 源与边界：`pkg/util/importer/rand.rs`、`config.rs::ImporterError`、`Cargo.toml`、`lib.rs`。
- 上游与共享消费者：`db.rs::random_integer`、`db.rs::generate_column_data`、`db.rs::generate_row_data`、`data.rs::Datum`；Go 对照为 `rand.go`、`db.go`。
- 独立测试：`rand_test.rs::timestamp_without_maximum_preserves_minimum_clock_like_go`、`rand_test.rs::bounded_year_samples_elapsed_seconds_like_go`；相关消费者测试还包括 `db_test.rs` 与 `data_test.rs`。
- RustCodeGraph 状态：索引包含 `pkg/util/importer` 的 22 个 Go/Rust 文件和 `rand.rs` 的 25 个符号。对 `rand_i64`、`rand_date`、`rand_time`、`rand_timestamp`、`rand_year`、`generate_column_data`、`civil_from_days`、`days_from_civil` 执行了 `node` 查询；查询轨迹确认了文中列出的内部 callees、测试调用者及 `generate_row_data -> generate_column_data` 调用边。宽泛 `explore` 对常见 random/time 名称产生大量跨仓库噪声，因此结论只采用精确符号和同目录源码证据。
- 人工复核重点：公开性、闭区间分支、缺省边界行为、原子状态推进、首次未设种路径、Go 差异及测试隔离均逐行对应上述文件；未把代码注释中的时间设种意图误写成现有行为。

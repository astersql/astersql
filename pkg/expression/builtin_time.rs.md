# `pkg/expression/builtin_time.rs`

## 文件定位

`builtin_time.rs` 属于 `astersql-expression` crate，由 [`pkg/expression/lib.rs`](lib.rs) 以 `#[path = "builtin_time.rs"] mod builtin_time_kernel;` 私有挂载。它不是 SQL 函数注册表，也不直接持有 `EvalContext`、行数据或返回 `Datum`；它把 MySQL/TiDB 时间函数中可独立表达的确定性规则整理成 Rust 辅助函数，供标量/向量签名及测试复用。当前生产接线的直接证据是 [`pkg/expression/builtin.rs`](builtin.rs) 调用 `time_to_sec` 和 `week_day`；测试配置下，`lib.rs` 另以 `builtin_time` 测试门面重导出全部符号。

crate 边界由 [`pkg/expression/Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-expression`，库根为 `lib.rs`，本文件直接使用的外部依赖是 `chrono`、`chrono-tz` 和 `regex`。文件本身没有 feature 或条件编译分支。

## 核心职责

该文件集中实现十类纯时间语义：日期时间解析/格式化，MySQL `TIME`（duration）解析与加减，`PERIOD_ADD/DIFF` 换算，`GET_FORMAT`/`TIME_FORMAT`，日期部件与月末计算，`TIMESTAMPADD/DIFF`，固定偏移和 IANA 时区转换，TSO 物理/逻辑部分解析，有界陈旧读时间夹取，以及 Unix 时间、`EXTRACT`、`DATE_FORMAT`、`CURRENT_DATE/TIME` 辅助。

其设计边界是“给定显式输入即可计算的内核”。Go 对照文件 [`pkg/expression/builtin_time.go`](builtin_time.go) 中的函数工厂、参数类型推断、NULL 传播、SQL mode、warning、语句级当前时间缓存和 Datum 转换不在这里；调用层必须在进入这些辅助函数前后完成这些上下文语义。

## 主要符号

- `TimeError(String)` 与 `TimeResult<T>`：本文件统一的轻量错误类型和结果别名。`invalid` 生成带输入的非法值错误，`overflow` 生成运算溢出错误。
- `MICROS_PER_SECOND/MINUTE/HOUR/DAY`、`MAX_TIME_MICROS`：统一微秒换算并限定 MySQL `TIME` 的绝对上限 `838:59:59.999999`；`TSO_LOGICAL_BITS = 18` 描述 TSO 位布局。
- `parse_datetime` / `format_datetime`：解析常见分隔符、6/8/12/14 位紧凑格式、两位年份和最多 6 位小数秒，并格式化为标准日期时间文本。
- `is_duration`、`get_fsp_for_time_add_sub`、`parse_duration_micros`、`format_duration_micros`：识别并规范化 duration；正则通过 `OnceLock<Regex>` 懒初始化。
- `add_time_strings` / `sub_time_strings`、`time_to_sec` / `sec_to_time`：实现字符串时间加减和秒数互转，包含符号、天数、微秒和范围处理。
- `valid_period`、`period_to_month`、`month_to_period`、`period_add`、`period_diff`：在 `YYMM`/`YYYYMM` 与绝对月序之间转换。
- `get_format`、`format_mask`、`time_format`、`date_format`：解释 MySQL 风格 `%` 掩码；日期格式化还使用 `ordinal_suffix` 和两种周编号辅助函数。
- `make_date`、`make_time`、`quarter`、`month_name`、`day_name`、`day_of_week`、`week_day`、`day_of_year`、`date_diff`、`last_day`：日期构造与部件查询。
- `timestamp_add` / `timestamp_diff`：按单位进行时间戳运算；`add_months` 负责月末钳制，`whole_months` 负责完整月数计算。
- `convert_tz`、`unix_timestamp`、`from_unix_time`、`current_date`、`current_time`：统一使用 `parse_offset`、`local_in_fixed`、`local_in_tz` 处理固定偏移或 IANA 时区。
- `parse_tso` / `parse_tso_logical`、`cal_appropriate_time`：提取 TSO 高位 Unix 毫秒/低 18 位逻辑时钟，并把 SafeTS 对应时间夹在请求区间内。
- `to_days`、`to_seconds`、`extract`：实现 MySQL 纪元日数、秒数和组合时间部件提取。

这些函数虽然多标记为 `pub`，但其所属 `builtin_time_kernel` 模块在 crate 根是私有模块，因此并非 crate 对外稳定 API；`pub` 主要便于同 crate 模块和测试门面使用。

## 执行流程

1. 日期时间文本先由 `parse_datetime` 去除首尾空白并判定小数秒位置；全数字输入进入 `parse_compact_datetime`，其他输入按 ASCII 标点拆日期、按冒号拆时钟，最后交给 `chrono` 的 `_opt` 构造器验证日历合法性。两位年份使用 `00..69 -> 2000..2069`、`70..99 -> 1970..1999`。
2. duration 路径先由缓存正则做词法筛选，再由 `parse_duration_micros` 解析符号、天、时分秒和小数，汇总为带符号微秒。`add_time_strings` 仅当左值同时含 `-` 和空白时把它视作 datetime；否则双方都按 duration 处理，结果再检查 MySQL `TIME` 上限。`sub_time_strings` 通过右值取负复用加法。
3. period 路径先验证月份为 `1..12`，把年份/月转换成单调绝对月数，完成加减后再转换回来；无效 period 或非正结果报错。
4. 格式化路径逐字符扫描掩码。`format_mask` 处理时间域代码，`date_format` 处理日期、星期、周数代码并委托前者处理时钟代码；未知代码的保留行为在两者间不同，扩展时必须保持现有兼容语义。
5. `timestamp_add` 对 `MONTH/QUARTER/YEAR` 使用 `add_months`，从而把不存在的目标日期钳到月末；其他单位转换成 `chrono::Duration`。`SECOND` 保留小数并截断到微秒，其他数值单位先四舍五入。`timestamp_diff` 对固定长度单位做微秒整数除法，对月/季度/年使用 `whole_months` 排除未走完的尾部月份。
6. 时区路径先把输入墙钟时间解释为 UTC，再投影到目标时区。命名时区遇到 DST 秋季重叠时选较晚的 UTC 实例；遇到春季空洞时逐分钟向后寻找首个有效墙钟时间，最多搜索 24 小时。
7. TSO 路径右移 18 位得到 Unix 毫秒，位掩码得到逻辑部分；有界陈旧读辅助只执行 `[min_time, max_time]` 夹取，不读取存储或 SafeTS 服务。
8. `current_date/current_time` 要求调用者显式传入 UTC 的 `now` 和时区；`current_time` 把纳秒按 `fsp` 四舍五入，进位时跨到下一秒。

## 数据与状态

核心中间表示是 `chrono::NaiveDateTime`、`NaiveDate`、`NaiveTime` 和带符号 `i64` 微秒。`Naive*` 类型不携带时区；仅在转换边界临时构造 `DateTime<Utc>`、`FixedOffset` 或 `chrono_tz::Tz`。因此调用者不能把普通 `NaiveDateTime` 误当作已绑定 UTC 的瞬间。

唯一进程级状态是 `duration_pattern` 内的 `OnceLock<Regex>`：首次调用编译固定正则，之后只读共享。月份名数组位于 `month_name` 的函数内常量中。文件不读取环境时区、系统时钟、会话变量、事务或存储；所有可变语义均由参数提供。

重要不变量包括：小数秒最多取前 6 位；duration 的分、秒必须小于 60；固定偏移范围为 `-14:00..+14:00`；`from_unix_time` 的微秒小于一百万；`current_time` 的 `fsp` 为 `0..6`；TSO 必须为正数；多数日期构造限制在 `chrono` 可表示且代码显式接受的 `1..9999` 年域。

## 依赖与调用关系

上游装配链为 `pkg/expression/lib.rs -> builtin_time_kernel`。RustCodeGraph 对文件节点报告有 20 个使用文件，并明确列出 `pkg/expression/builtin.rs`、独立时间测试等；源码检索进一步确认当前非测试直接边为：

- `builtin.rs` 的 `TIME_TO_SEC` 求值分支调用 `builtin_time_kernel::time_to_sec`；
- `builtin.rs` 的 `WEEKDAY` 求值分支调用 `builtin_time_kernel::week_day`；
- `lib.rs` 在 `#[cfg(test)]` 下通过 `builtin_time` 门面重导出内核，供 `builtin_time_30_aster_unit_test.rs` 使用；
- `builtin_time_test.rs` 直接使用 `parse_datetime`、`format_datetime`、`convert_tz`，并让大量 Go 迁移测试入口复用 parity suite。

下游依赖分为：`chrono` 的日历构造、差值、格式化和溢出检查；`chrono-tz` 的 IANA 时区及 DST 规则；`regex` 的 duration 词法匹配；标准库的 `OnceLock`、格式化和整数 checked 运算。RustCodeGraph 的 `callees timestamp_add` 确认内部调用 `add_months`，`callees current_time` 确认其时区路径调用 `parse_offset`；图对常见同名方法存在跨文件候选噪声，因此精确依赖以目标源码为准。

`builtin_time_vec.rs` 是相邻的向量执行内核，但当前并非对本文件所有函数的简单批量包装；不能仅凭文件注释推断每个标量辅助都已接入向量执行路径。

## 错误处理与边界

可失败 API 返回 `TimeResult<T>`，错误文本区分 `invalid <kind>: <value>` 和 `<kind> overflow`。解析函数拒绝非数字字段、字段数量不符和非法日历值；算术使用 `checked_add`、`checked_mul`、`checked_add_signed` 或 `chrono` 的可选构造器避免静默回绕。`sec_to_time` 与 `make_time` 按 MySQL 行为钳制，而 `timestamp_add`、period 运算和普通 duration 加法在相应边界报错；调用方需要理解“钳制”与“错误”并非统一策略。

该内核不表达 SQL NULL，也不把错误转换成 warning。Go 层中诸如无效日期在 SQL mode 下返回 NULL/追加 warning、参数无符号标志、会话时区加载失败等行为，必须由外层签名实现；不能直接把 `TimeError` 等同于 Go 的最终用户错误码。

已知语义边界还包括：`parse_datetime` 不是 Go `types.Time` 的完整解析器，不表示零日期；`extract` 只覆盖源码列出的单位组合；固定偏移只接受严格的 `±HH:MM`；DST 空洞采用最多 24 小时的分钟扫描；`date_format` 的周模式只实现本文件列出的 `%U/%u/%V/%v/%X/%x` 路径。

## 并发与资源生命周期

所有公开计算函数均无可变全局状态，输入按值或不可变引用传递，适合并发调用。`OnceLock` 保证 duration 正则只初始化一次且线程安全；固定正则编译失败被视为代码常量错误并通过 `expect` 终止，而不是运行时用户错误。

文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。`DateTime`、字符串和正则匹配结果均遵循普通所有权/栈生命周期。DST 空洞搜索是同步、有界循环，最坏检查 1440 个候选分钟；格式化会分配新的 `String`。这些是扩展高频向量路径时需要关注的主要 CPU/分配成本。

## 与 Go 版本的对应关系

[`pkg/expression/builtin_time.go`](builtin_time.go) 是语义来源，但粒度不同。Rust 名称直接对应的 Go 辅助包括 `isDuration -> is_duration`、`getFsp4TimeAddSub -> get_fsp_for_time_add_sub`、`validPeriod -> valid_period`、`period2Month/month2Period -> period_to_month/month_to_period`，以及 Go 内建签名中 ADDTIME/SUBTIME、TIMESTAMPADD/DIFF、CONVERT_TZ、MAKEDATE/MAKETIME、TIME_FORMAT、TIME_TO_SEC、SEC_TO_TIME、TSO 和 bounded-staleness 的确定性部分。

Go 文件同时拥有 `functionClass`/`builtin*Sig` 构建和求值、`types.Time/Duration/Datum`、上下文 warning、SQL mode、statement timestamp、NULL 传播及错误码包装；Rust 本文件没有复刻这些外层职责。因此“辅助算法已存在”不等于对应 SQL 函数已经完整迁移接线。反过来，Rust 的 `chrono`/`chrono-tz` 表示也与 Go 的 `types.Time`/`time.Location` 不同，尤其需关注零日期、DST 歧义、最大年份、精度舍入和错误到 NULL/warning 的映射。

[`pkg/expression/builtin_time_test.rs`](builtin_time_test.rs) 保留大量 Go 测试形状和注释，但许多入口主要调用同一 `run_time_parity_suite`，其余 Go 操作只是迁移记录，并不逐条执行。真正直接断言本文件辅助函数的集中测试位于 [`pkg/expression/builtin_time_30_aster_unit_test.rs`](builtin_time_30_aster_unit_test.rs)，覆盖 duration/FSP、加减、period、格式掩码、构造与日期部件、月末、时间戳差、时区、TSO、有界陈旧读和 MySQL 日数。

## 扩展指南

新增确定性时间算法时，应把纯解析/计算放在本文件，把 SQL 参数求值、NULL/warning/SQL mode 和会话状态留在 `builtin.rs` 或对应签名层；若需要向量执行，再显式检查并同步 `builtin_time_vec.rs`/生成文件，而不要假设自动复用。新增公开辅助前还应确认它是否确实需要 `pub`，以及 crate 私有模块边界是否足够。

修改解析、舍入或范围规则时，优先扩展独立的 [`pkg/expression/builtin_time_30_aster_unit_test.rs`](builtin_time_30_aster_unit_test.rs)，并按仓库要求保持测试与生产文件分离；涉及 Go 对齐时同步核对 `builtin_time_test.go` 的对应表格。若改变 SQL 层可见行为，还需在接线测试中覆盖 NULL、warning、SQL mode、时区和类型元数据，而不能只验证纯函数返回值。

风险集中在：小数秒截断/四舍五入差异、负 duration 与 `838:59:59.999999` 边界、月末和闰年、完整月份定义、DST 重叠/空洞、零日期、Unix/TSO 单位、未知格式码行为，以及 `chrono` 表示域与 TiDB 类型域不一致。格式化热路径新增 `format!` 或 DST 搜索扩大范围还可能带来性能回退。

## 验证依据

- 已完整读取目标源码 [`pkg/expression/builtin_time.rs`](builtin_time.rs)，核对常量、`TimeError`/`TimeResult`、全部公开函数、私有解析/格式化/时区/月历辅助及无条件编译事实。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/expression/builtin_time.rs` 返回 1,067 行源码及 20 个使用文件；`query` 定位 `parse_datetime`、`timestamp_add`、`convert_tz`、`current_time`；`callees timestamp_add` 和 `callees current_time` 验证关键内部调用。`files --filter` 未命中该路径，故按技能规则以精确 `node/query` 加源码检索补证。
- 已读取 [`pkg/expression/Cargo.toml`](Cargo.toml) 和 [`pkg/expression/lib.rs`](lib.rs)，确认 crate 名称、库根、`chrono`/`chrono-tz`/`regex` 依赖、模块挂载和测试门面。
- 已读取 Go 对照 [`pkg/expression/builtin_time.go`](builtin_time.go) 的相关符号，并以 [`pkg/expression/builtin_time_test.go`](builtin_time_test.go) 的测试名称/覆盖面核对原始语义范围。
- 已读取 Rust 独立测试 [`pkg/expression/builtin_time_test.rs`](builtin_time_test.rs) 的结构与关键用例，并完整读取直接 parity 测试 [`pkg/expression/builtin_time_30_aster_unit_test.rs`](builtin_time_30_aster_unit_test.rs)。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务规定的命令验证恰有 11 个固定二级标题，并人工复核仅新增本文档、没有把测试建议写入生产 Rust 文件。

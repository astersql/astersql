# `pkg/expression/builtin_time_vec.rs`

## 文件定位

该文件属于 `astersql-expression` crate（`pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs:275-276` 以私有模块 `builtin_time_vec_kernel` 装入。它实现一组以 `NullableVec<T>` 为输入/输出的时间函数向量内核，覆盖日历字段提取、日期构造、会计期运算、MySQL `TIME` 运算、语句时间广播、TSO 解析、时区转换和有界陈旧读时间钳位。

这里的“向量化”是保持列的行序并逐行处理 `Option<T>`，不是直接实现生产表达式框架的 `Expression::VecEval*` 或 `chunk::Column` 接口。仓库范围的 Rust 引用搜索显示，本文件的 `vec_*` 入口目前只被 `builtin_time_vec_29_aster_unit_test.rs` 和 `builtin_time_vec_test.rs` 调用；`lib.rs:835-837` 也只在 `cfg(test)` 下通过 `expression_builtin_time_vec` 重新导出。因此它是已实现、可独立验证的迁移内核，但尚未接到当前生产表达式派发主链。Go 的生产实现位于 `pkg/expression/builtin_time_vec.go`，直接挂在各 `builtin*Sig.vecEval*` 方法上。

## 核心职责

- 用 `NullableVec<T>` 统一维护行数、行序和 SQL `NULL` 传播；`vec_map` 与 `vec_zip_map` 是单列、双列内核的公共骨架。
- 用 `MysqlTime` 和 `MysqlDuration` 提供本文件自包含的日期时间表示，并借助 `chrono` 完成合法性、日历和时区计算。
- 用 `EvalContext` 将非法零日期、截断等情况区分为严格模式错误或非严格模式警告，模拟 Go `EvalContext`/statement type context 的关键行为。
- 实现与 Go 同名能力中的一个明确子集，例如 `MONTH`、`DATE`、`LAST_DAY`、`MAKEDATE`、`PERIOD_ADD/DIFF`、`SEC_TO_TIME`、`MAKETIME`、`TIDB_PARSE_TSO`、`CONVERT_TZ` 和 `TIDB_BOUNDED_STALENESS`。
- 为尚未迁入本文件的标量行为提供 `vec_map`/`vec_zip_map` 委托入口；文件头注释明确标量层和 expression/chunk 接线由其他文件组负责。

## 主要符号

- 常量：`WEEKDAY_NAMES`、`MONTH_NAMES` 提供固定英文名称；`MAX_TIME_HOUR = 838` 与三个 `MICROS_PER_*` 常量定义 MySQL `TIME` 范围及内部单位。
- `TimeVecError`：集中表示非法时间、非法参数/FSP、溢出、非法时区、列长不一致，以及夏令时导致的歧义或不存在的本地时间。
- `EvalContext`：保存 `no_zero_date`、`no_zero_in_date`、`strict` 和累计的 `warnings`；私有方法 `handle_invalid`、`handle_truncate` 决定返回错误还是记录警告。
- `NullableVec<T>`：持有 `Vec<Option<T>>`，公开构造、只读访问、取回所有权、长度和空判断。字段本身私有，避免调用者破坏列封装。
- `MysqlTime`：七个公开分量构成日期时间。`new` 通过 `to_naive` 校验，`from_parts_unchecked` 允许表示 MySQL 零日期/零分量日期，`date_only` 清除时间部分。
- `MysqlDuration`：以带符号 `i64` 微秒存储 MySQL `TIME`；分量提取使用绝对值，而 `as_micros` 保留总值符号。
- 通用骨架：`vec_map` 对单列映射；`vec_zip_map` 先校验等长，再令任一输入为 `NULL` 的行输出 `NULL`。
- 日历函数：`vec_month`、`vec_year`、`vec_day_of_month`、`vec_quarter`、`vec_date`、`vec_weekday`、`vec_day_of_week`、`vec_day_of_year`、`vec_day_name`、`vec_month_name`、`vec_last_day`、`vec_make_date`、`vec_date_diff`。
- 格式与会计期：`get_format`、`vec_period_diff`、`vec_period_add`，以及私有的 `valid_period`、`period_to_month`、`month_to_period`。
- `TIME` 函数：`vec_hour`、`vec_minute`、`vec_second`、`vec_microsecond`、`vec_time_to_sec`、`vec_sec_to_time`、`vec_make_time`；`checked_fsp` 将 FSP 限定为 0..=6，`seconds_to_micros` 负责舍入和溢出检查。
- 当前时间与时区：`vec_statement_timestamp`、`vec_utc_timestamp`、`vec_current_date`、`vec_parse_tso`、`vec_convert_tz`。`ConvertTimeZone`、`parse_convert_tz`、`local_to_utc`、`utc_in_timezone` 是转换链的内部组件。
- `vec_bounded_staleness` 将 `minimum_safe` 钳位到每行 `[minimum, maximum]`；`vec_literal` 将常量广播到指定行数。

## 执行流程

1. 调用者先把一列构造成 `NullableVec<T>`；多列函数要求各列行数一致。普通单列和双列函数通过 `vec_map`/`vec_zip_map` 逐行处理，复杂函数显式按索引遍历。
2. 每行先做 `NULL` 短路：单列的 `None`、双/三列中的任意 `None` 都生成 `None`，不会继续解析或校验该行。这也保留了 Go `PERIOD_ADD` 中“offset 为 NULL 时先返回 NULL、不要检查非法 period”的顺序语义。
3. 日期抽取函数直接读取合法分量；需要真实日历的函数通过 `valid_date_or_null` 或 `MysqlTime::to_naive` 校验。零日期受 `EvalContext` 的 SQL mode 标志控制，非严格模式记录警告并将该行置空，严格模式立即返回错误。
4. `vec_make_date` 按 MySQL 规则把 00..69 展开到 2000..2069、70..99 展开到 1970..1999，再从元旦增加 `day - 1` 天；非法范围或日期溢出返回行级 `NULL`。
5. 会计期先经 `valid_period` 验证，再在 `YYYYMM/YYMM` 与连续月份编号之间转换；`vec_period_diff` 求差，`vec_period_add` 加偏移并拒绝负的连续月份。
6. `vec_sec_to_time` 和 `vec_make_time` 先验证 FSP，按指定精度量化微秒，并把小时限制在 MySQL 的 838:59:59。`SEC_TO_TIME` 超界通过 `handle_truncate` 告警/报错；`MAKETIME` 的非法分秒返回 `NULL`，超界小时钳位。
7. 当前时间函数将调用者提供的单个 UTC `instant` 转到目标时区、截取 FSP 后广播，因此同一次调用的所有行完全一致。`vec_parse_tso` 右移 18 位取物理毫秒，0、负值和 `NULL` 均输出 `NULL`。
8. `vec_convert_tz` 对每行解析 IANA 时区或 `±HH:MM` 固定偏移，把源本地时间映射到 UTC，再映射到目标时区；非法时区或非法日期输出 `NULL`，DST 歧义/空洞则返回明确错误。
9. `vec_bounded_staleness` 验证两端日期，`minimum > maximum` 时输出 `NULL`，否则返回 `minimum_safe`、下界或上界三者之一。

## 数据与状态

本文件没有全局可变状态。名称表和换算常量是只读常量；每次求值创建新的输出 `Vec`，容量通常预分配为输入长度。`NullableVec` 的不变量是每个元素恰好对应一个输入行；`vec_zip_map` 和所有显式多列入口以 `LengthMismatch` 保护等长不变量。

`EvalContext` 是唯一会在调用过程中修改的状态：非严格非法时间和截断会按遇到顺序追加到 `warnings`。因此同一个上下文可跨多次调用累计警告，调用者若需要语句级隔离必须自行创建或清理上下文。`MysqlTime::from_parts_unchecked` 有意允许零日期等 MySQL 特殊值；需要公历运算前必须走 `to_naive`/`valid_date_or_null`。`MysqlDuration` 的 `i64` 微秒保留符号，小时/分/秒/微秒分量读取绝对值。

TSO 的低 18 位是逻辑计数，高位是物理毫秒；转换后输出 FSP 6。当前时间类入口不读取系统时钟，而接受 `DateTime<Utc>` 参数，这使语句时间的一致性由调用者显式保证，也让测试可确定复现。

## 依赖与调用关系

- crate 边界：`pkg/expression/Cargo.toml` 声明 `chrono = "0.4"`、`chrono-tz = "0.10"` 和 `thiserror = "2"`；本文件直接使用这三项，未直接依赖 chunk、session 或 KV crate。
- 装配关系：`pkg/expression/lib.rs:275-276` 声明私有 `builtin_time_vec_kernel`；测试期 `lib.rs:835-837` 才将其符号重新导出到 `expression_builtin_time_vec`。
- RustCodeGraph 精确查询确认主要符号位于本文件。例如 `vec_convert_tz` 在第 948 行、`vec_bounded_staleness` 在第 981 行、`vec_date` 在第 344 行。callee 查询给出的关键边包括：`vec_date → handle_invalid/is_zero/invalid_zero/date_only`，`vec_sec_to_time → checked_fsp/handle_truncate`，`vec_make_time → checked_fsp/seconds_to_micros`，`vec_convert_tz → parse_convert_tz/to_naive/local_to_utc/utc_in_timezone`，`vec_bounded_staleness → vec_zip_map/to_naive`。
- RustCodeGraph 对上述公开入口的 callers 查询均为空；仓库 `rg` 复核只发现独立测试调用，支持“尚未接入生产派发”的结论，而不是把空调用边解释成已接线。
- Go 上游链为 `builtin*Sig.vecEval*`：先调用参数的 `VecEvalTime/Int/Real/String` 填充 `chunk.Column`，合并 NULL bitmap，再逐行写结果；Rust 当前内核只对应最后的按行语义，尚不拥有参数求值、列缓冲分配器或函数签名注册。
- 测试入口：`builtin_time_vec_29_aster_unit_test.rs` 是本文件的真实行为回归；`builtin_time_vec_test.rs` 的 Go 同名入口会调用该 parity suite，并额外直接验证固定偏移 `CONVERT_TZ`。原始 Go 表驱动测试在 `builtin_time_vec_test.go`。

## 错误处理与边界

- 列长不同是整次调用错误 `LengthMismatch`；输入行为 `NULL` 则是正常的行级 `NULL`，两者不能混用。
- `MysqlTime::new` 拒绝 chrono 不能表示的日期时间；`from_parts_unchecked` 不校验，后续日历/时区函数必须再次校验。零日期与零分量日期可能根据 SQL mode 变成警告加 `NULL` 或立即错误。
- 非法会计期返回 `IncorrectArgs`，`PERIOD_ADD` 得到负月份编号返回 `Overflow`。`GET_FORMAT` 对未知组合返回空串，不返回错误。
- FSP 只允许 0..=6；非有限秒数、浮点转微秒超出 `i64`、日期时间构造失败分别返回 `InvalidTime` 或 `Overflow`。量化使用舍入，而当前时间的 `from_datetime` 对微秒按 FSP 截断，扩展时不得混淆两种规则。
- MySQL `TIME` 上限是 ±838:59:59。`vec_sec_to_time` 超界会记录截断；`vec_make_time` 的 `_ctx` 当前未使用，钳位小时不会追加警告，这是与 `vec_sec_to_time` 不同的现有事实。
- `parse_tz` 用于必须合法的会话时区，失败返回 `InvalidTimeZone`；`parse_convert_tz` 用于 SQL `CONVERT_TZ`，非法名称/偏移返回 `None` 并产生行级 `NULL`。固定偏移限制到 ±14:00。DST 重叠和跳跃分别返回 `AmbiguousLocalTime`、`NonexistentLocalTime`。
- `vec_bounded_staleness` 当前只接收已经算出的 `minimum_safe`；Go 版本还会从 session/KV store 获取语句最小安全时间。该外部状态获取不属于本文件现有实现。

## 并发与资源生命周期

所有函数都是同步的，没有线程、异步任务、锁、通道、事务或外部 I/O。只读输入通过共享借用传入，输出向量由函数独占创建；唯一的可变借用是 `&mut EvalContext`，Rust 借用规则阻止同一上下文被并发无同步地修改。`DateTime<Utc>`、`Tz` 和固定偏移仅在调用栈内使用，无需显式清理。

与 Go 版本相比，这里没有 `bufAllocator.get/put` 生命周期：每个输出和中间结果使用 Rust `Vec`/值类型，离开作用域自动释放。代价是当前内核可能为每次调用分配新向量，并且 `vec_convert_tz` 对每一行重复解析时区字符串；未来接入 chunk 主链时可在不改变 NULL、错误顺序和 DST 语义的前提下复用列缓冲或缓存常量时区。

## 与 Go 版本的对应关系

`pkg/expression/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/expression"` 明确 crate 的 Go 来源。Rust 函数可对应到 `builtin_time_vec.go` 的如下生产方法：`vec_month/year/date` 对应 `builtinMonthSig`、`builtinYearSig`、`builtinDateSig.vecEval*`；名称、星期、末日和构造函数对应各同名 `builtin*Sig`；会计期对应 `builtinPeriodDiffSig`/`builtinPeriodAddSig`；duration 分量和构造对应 `builtinHour/Minute/Second/MicroSecond/TimeToSec/SecToTime/MakeTimeSig`；TSO、时区和陈旧读对应 `builtinTidbParseTsoSig`、`builtinConvertTzSig`、`builtinTiDBBoundedStalenessSig`。

两端共同保持行序、NULL 合并、两位数年份、838 小时上限、period 合法性、零日期模式、TSO 高位物理毫秒以及 `CONVERT_TZ` 非法参数返回 NULL 等核心语义。Go 直接操作 `chunk.Column`、使用表达式参数求值与 buffer allocator，并通过完整 session/type context 生成 TiDB 错误和 warning；Rust 使用本地 `NullableVec`、`MysqlTime`、`EvalContext` 和 `TimeVecError`。因此 Rust 不是 Go 文件 3014 行全部签名的一一替换，也未覆盖 Go 文件中的全部时间函数。

`builtin_time_vec_29_aster_unit_test.rs` 验证闰日、NULL、星期编号、严格/非严格零日期、两位数年份、period、duration 符号与截断、TSO、时区和 bounded staleness。`builtin_time_vec_test.rs` 中大部分 Go 表驱动结构仍是迁移说明/轻量占位，三个 Go 同名测试入口实际复用 parity suite；性能 benchmark 函数也未执行真实 benchmark。文档因此不把这些占位描述为完整 Go 测试矩阵已运行。

## 扩展指南

- 新增纯逐行时间变换时，优先复用 `vec_map` 或 `vec_zip_map`，先明确 NULL 短路顺序；多列入口必须在任何索引访问前检查等长。
- 涉及零日期、截断或 SQL mode 时，把策略集中在 `EvalContext`，不要在新函数中静默吞错。若要进一步对齐 Go 的 error/warning 类别，应同时核对对应 `builtin*Sig.vecEval*` 和标量实现 `builtin_time.go`。
- 涉及公历计算时，不要直接信任 `from_parts_unchecked` 的值；先调用 `to_naive` 或增加与 `valid_date_or_null` 等价的检查。涉及 `TIME` 时保留带符号总微秒与无符号分量的区别。
- 时区扩展应同时覆盖 IANA 名称、固定偏移边界、DST ambiguous/nonexistent 两类结果以及非法日期。若缓存解析结果，缓存只能优化解析，不能改变逐行 NULL/错误行为。
- 若将本内核接入生产表达式主链，接入点应是 `pkg/expression/lib.rs` 的生产模块装配及相应 `builtin*Sig`/`VecEval*` 实现；必须在 `chunk.Column` 与 `NullableVec`/本地时间类型之间明确所有权、NULL bitmap、FSP、类型标志和 warning 的转换，不能仅把测试 re-export 当作接线。
- 测试逻辑应继续放在独立文件：优先扩展 `pkg/expression/builtin_time_vec_29_aster_unit_test.rs` 的真实断言，并同步审阅 `pkg/expression/builtin_time_vec_test.rs` 与 Go 的 `builtin_time_vec_test.go`；不要把 `#[test]` 嵌入本生产源文件。
- 性能敏感变更应关注每次调用的向量分配、字符串创建、时区逐行解析和 chrono 转换；行为验证之后再与 Go chunk/buffer 路径做等量 benchmark。

## 验证依据

- 源码与装配：完整阅读 `pkg/expression/builtin_time_vec.rs`；核对 `pkg/expression/lib.rs:272-276` 的模块声明、`lib.rs:610-625` 的独立测试模块以及 `lib.rs:831-837` 的测试期 re-export。目标包没有 `doc.go`。
- crate 配置：阅读 `pkg/expression/Cargo.toml`，确认 crate 名、`autotests = false`、`chrono`、`chrono-tz`、`thiserror` 依赖、Go 包迁移元数据及显式测试目标。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点和 1,848,419 条边；对 `vec_convert_tz`、`vec_bounded_staleness`、`vec_date`、`vec_sec_to_time`、`vec_make_time`、`vec_period_add`、`MysqlTime`、`TimeVecError` 执行 `query`，并对六个主要 `vec_*` 入口执行 `callers`/`callees`。调用者为空，内部 callee 边与源码一致；再以仓库引用搜索确认外部引用仅来自独立测试。
- Go 对照：阅读 `pkg/expression/builtin_time_vec.go` 中对应 `builtin*Sig.vecEval*` 的实现片段，阅读 `pkg/expression/builtin_time_vec_test.go` 的表驱动入口、空格式回归和 `TestVecMonth`；标量实现边界参考 `pkg/expression/builtin_time.go` 的存在及同名 `get_format` 实现位置。
- Rust 测试：完整阅读 `pkg/expression/builtin_time_vec_29_aster_unit_test.rs`，以及 `pkg/expression/builtin_time_vec_test.rs` 的 Go 同名测试入口、严格模式说明和固定偏移 `CONVERT_TZ` 回归。按任务要求本次是纯文档分析，未运行 Cargo 或 Rust 测试。
- 结构验收使用任务文件给定命令，要求目标文档存在且恰有十一个固定二级标题；人工复核同时确认本文明确回答文件为何存在、现有运行路径、未接线边界以及安全扩展位置。

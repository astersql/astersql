# `pkg/expression/helper.rs`

源文件：[helper.rs](helper.rs)  
Go 对照：[helper.go](helper.go)  
独立 Rust 测试：[helper_test.rs](helper_test.rs)

## 文件定位

本文件属于 `astersql-expression` crate。crate 根在 [lib.rs](lib.rs) 中通过 `#[path = "helper.rs"] mod helper_kernel;` 将它作为私有模块挂载；当前没有面向其他 crate 的常规 `pub use`，只有 `#[cfg(test)]` 下的 `expression_files_36::helper` 对测试重导出。因此它目前是表达式 crate 内部的默认时间值辅助实现，而不是稳定的跨 crate 公共 API。

[Cargo.toml](Cargo.toml) 声明该 crate 对应 Go 包 `pkg/expression`。本文件直接使用 `chrono`、`chrono-tz` 和 `thiserror`，分别承担日历/时刻处理、IANA 时区转换和错误类型派生。

## 核心职责

- `bool_to_int64` 把 Rust 布尔值映射成 MySQL 风格的 `0`/`1`。
- `is_valid_current_timestamp_expr` 判断 `CURRENT_TIMESTAMP` 默认值表达式的首个精度参数是否与字段 `decimal` 一致。
- `get_time_current_timestamp` 从语句级冻结时间生成 `TIMESTAMP`、`DATETIME` 或 `DATE` 值。
- `get_time_value` 将文本、整数、函数节点、`NULL` 等抽象输入转换为立即时间值、延迟函数名或空值。
- `MysqlTime` 及其 `Display` 实现保存并格式化墙钟时间、零日期、时间类型和小数秒精度。

这些职责对应 [helper.go](helper.go) 中 `boolToInt64`、`IsValidCurrentTimestampExpr`、`GetTimeCurrentTimestamp`、`getTimeCurrentTimeStamp` 和 `GetTimeValue` 的一部分；Go 文件中物化视图调度、测试随机时区等其他辅助逻辑不在本 Rust 文件内。

## 主要符号

- `pub fn bool_to_int64(bool) -> i64`：使用 `i64::from` 返回 `1` 或 `0`。当前仓库检索未发现生产调用；`builtin_compare_vec_generated.rs` 另有同名局部实现，不能据此认定两者已经接线。
- `TimeExpr::{Value, Function { name, arguments }}` 与 `TimeExpr::current_timestamp`：只表达本辅助函数需要的最小 AST 信息。函数参数被建模为 `Vec<i64>`。
- `TimeFieldType { decimal }`：字段精度的最小投影，不等同于完整的 `types::FieldType`。
- `is_valid_current_timestamp_expr`：名称按 ASCII 大小写不敏感比较；有参数时只比较 `arguments[0]`，不拒绝额外参数，这与 Go 实现只读取 `fn.Args[0]` 的行为一致。
- `TimeType::{Timestamp, Datetime, Date}`：限定本文件支持的三类 MySQL 时间。
- `MysqlTime { value, time_type, fsp }`：`value: None` 专门表示 MySQL 零日期；字段私有，只能由本模块构造并通过 `Display` 观察。
- `TimeValue::{Null, MysqlTime, DeferredFunction}`：区分 SQL `NULL`、已解析时间和插入时再求值的函数名。
- `TimeInput::{Text, Integer, Null, Function, UnaryInteger, Other}`：把 Go 中动态 `any`/AST 输入收敛为显式枚举；`Other` 代表未识别输入。
- `TimeContext`：持有冻结的 `DateTime<Utc>` 和会话 `chrono_tz::Tz`，并提供只读访问器。
- `TimeError::{InvalidFsp, InvalidTime, InvalidDefaultValue}`：本文件全部可恢复失败的统一错误面。
- `get_time_current_timestamp`、`get_time_value`：公开入口；`current_mysql_time`、`validate_fsp`、`parse_text_time`、`parse_numeric_time` 是内部流水线。

## 执行流程

`get_time_current_timestamp` 的流程如下：

1. 调用 `current_mysql_time`，先由 `validate_fsp` 保证精度在 `0..=6`。
2. 将 `TimeContext.statement_time` 从 UTC 投影到会话 `location`。
3. 用 `10^(9-fsp)` 纳秒量子向下截断，而不是四舍五入。
4. 若目标是 `Date`，再把时、分、秒和纳秒清零。
5. 包装成 `MysqlTime`，再返回 `TimeValue::MysqlTime`。

`get_time_value` 先统一校验 fsp，再按 `TimeInput` 分派：

1. `Null` 与 `Other` 直接返回 `TimeValue::Null`。
2. `Function("current_timestamp" | "current_date")` 返回大写的 `DeferredFunction`，不在此刻读取语句时间；其他函数返回 `InvalidDefaultValue`。
3. 同名 `Text` 则立即求值：`current_timestamp` 保留请求的 `TimeType`，`current_date` 强制生成 `Date`。
4. 固定零日期文本、整数 `0` 和一元整数 `0` 生成 `MysqlTime::zero`。
5. 其他文本经 `parse_text_time` 接受 `YYYY-MM-DD HH:MM:SS[.f]` 或纯日期；其他整数经 `parse_numeric_time` 按 MySQL 的两位/四位年份数值区间扩展到 14 位后解析。
6. `Display` 最后按类型输出日期或日期时间；非 `Date` 且 fsp 大于零时输出截断后的小数秒，零时间也保留对应位数。

## 数据与状态

唯一跨调用状态是调用方传入的不可变 `TimeContext`。`statement_time` 在构造时冻结，使同一上下文中的多次当前时间求值稳定；`location` 决定该 UTC 瞬间对应的本地墙钟时间。函数不读取全局时钟，也不缓存可变结果。

`MysqlTime.value == None` 是零日期哨兵，不是 SQL `NULL`；SQL `NULL` 由独立的 `TimeValue::Null` 表示。`fsp` 存为 `u32`，进入构造前已由 `validate_fsp` 限制。普通文本解析出来的纳秒不会按请求 fsp 再截断，只在显示时选择相应位数；当前时间路径会在构造阶段截断。

`explicit_timezone` 在 `get_time_value` 文本分支中被解析为 `_parse_location`，但当前 `parse_text_time` 只解析无时区的墙钟文本，未实际使用该值进行换算。这是现有实现事实，不应描述为已完整实现 Go `TypeCtx.WithLocation` 的解析语义。

## 依赖与调用关系

下游关系由 RustCodeGraph 和源码确认：`get_time_current_timestamp -> current_mysql_time -> validate_fsp`；`get_time_value` 调用 `validate_fsp`，并按输入调用 `current_mysql_time`、`parse_text_time` 或 `parse_numeric_time`；`parse_numeric_time` 最终复用 `parse_text_time`。

上游关系目前主要是测试：

- [helper_test.rs](helper_test.rs) 直接覆盖 `get_time_value`、`get_time_current_timestamp` 和 `is_valid_current_timestamp_expr`。
- [fts_to_like_36_aster_unit_test.rs](fts_to_like_36_aster_unit_test.rs) 经测试专用重导出覆盖精度、时区、零日期和非法时间。
- `lib.rs` 仅私有挂载 `helper_kernel`，没有发现非测试 Rust 文件调用上述时间入口。故它当前更接近已移植但尚未广泛接入表达式生产主链的内核；不能仅凭 `pub fn` 推断外部可达。

## 错误处理与边界

- fsp 小于 `0` 或大于 `6` 一律返回 `TimeError::InvalidFsp`；所有入口在指数运算前验证，避免非法幂次。
- 文本仅接受 chrono 能严格解析的完整日期时间或纯日期，非法月份、日期、时分秒返回 `InvalidTime`。
- 数值小于 `101`、落入两位年份空洞区间、扩展后不是 14 位或最终日期非法时返回 `InvalidTime`。零值在进入该函数前被特殊处理为零时间。
- 未知函数节点返回 `InvalidDefaultValue`；`Other` 则返回 `Null`。这是两类不同的容错策略，扩展输入枚举时不应无意互换。
- `is_valid_current_timestamp_expr` 不验证 fsp 自身是否在 `0..=6`，也不检查第二个及后续参数；其职责仅是复刻 Go 辅助函数当前的一致性判断。
- `NaiveDate::and_hms_opt(0, 0, 0)` 的 `expect` 只用于永远合法的午夜构造；其他输入错误均通过 `Result` 传播。

## 并发与资源生命周期

本文件没有锁、通道、异步任务、事务、I/O 或外部资源句柄。所有值均由调用栈拥有；`TimeContext` 可克隆，入口只借用它，不修改共享状态。并发调用是否共享同一语句时间完全由上层是否复用同一个 `TimeContext` 决定。

时区数据由 `chrono_tz::Tz` 值承载，没有需要显式关闭的生命周期。`String` 和 `Vec<i64>` 随枚举所有权正常释放。当前 API 的确定性来自显式传入时间，而非同步原语。

## 与 Go 版本的对应关系

对齐点：

- `bool_to_int64` 与 `boolToInt64` 都实现布尔到 `0`/`1`。
- `is_valid_current_timestamp_expr` 保留 Go 的大小写无关函数名判断、字段 decimal 规则以及“只检查首个参数”的细节。
- 当前时间路径使用冻结语句时间、按 fsp 截断并转换到会话时区；`DATE` 清零日内部分。
- 字符串当前时间立即求值，函数 AST 形式保留大写函数名延迟求值；零日期、整数时间和非法输入的主要测试分支与 Go 测试一致。

尚未等价或模型更窄的部分：

- Go 入口接收真实 `EvalContext`/`BuildContext`、`ast.ExprNode`、`types.Datum` 和完整类型上下文；Rust 使用本地轻量枚举和结构，尚未与 crate 的主表达式 AST/Datum 体系直接接线。
- Go `GetTimeValue` 的 `*driver.ValueExpr` 支持按 Datum kind 分派，并用类型上下文处理 SQL mode、警告和显式时区；Rust 版本不具备这些上下文语义，`explicit_timezone` 当前无实际效果。
- Go 一元表达式通过 `EvalSimpleAst` 和类型转换求值；Rust 的 `UnaryInteger` 已是折叠后的整数，不执行 AST 求值。
- Go `types.ParseTime`/`ParseTimeFromNum` 的兼容范围和错误/警告策略比 chrono 严格格式解析更广；文档与扩展不能假定两者对所有 MySQL 边界完全等价。

上述结论由 [helper.go](helper.go) 的对应函数及 [helper_test.go](helper_test.go) 的用例核对；迁移状态应以这些具体差异为准，而不是只按函数名判断完成度。

## 扩展指南

- 新增输入形态时，优先扩展 `TimeInput` 和 `get_time_value` 的穷尽匹配，并在独立的 [helper_test.rs](helper_test.rs) 增加正常、非法、零值和 `NULL` 用例；不要把测试嵌入生产源文件。
- 要完成主链接入，应把轻量 `TimeExpr`/`TimeFieldType`/`TimeInput` 与真实表达式 AST、字段类型、Datum 和上下文做明确适配，并决定是否从 `lib.rs` 导出；不能绕过现有类型上下文的 SQL mode、警告和时区契约。
- 扩展时间格式或数值区间时修改 `parse_text_time`/`parse_numeric_time`，并逐项对照 Go `types.ParseTime`/`ParseTimeFromNum`；重点关注无效日期、两位年份边界、小数秒截断和零日期兼容性。
- 若要让 `explicit_timezone` 生效，应先定义“无时区墙钟文本”的解释与存储规则，再补显式时区与会话时区不同的回归测试，避免无意改变显示值。
- 修改 `MysqlTime::Display` 时同时覆盖 `Date`、零时间、fsp `0`/`6` 和纳秒截断；格式化位于可见行为边界，兼容风险高于普通内部重构。
- 性能上当前解析每次分配 `String` 并调用 chrono 格式解析；高频接线前应评估是否复用 AsterSQL 类型系统解析器，而不是平行维护第二套解析路径。

## 验证依据

- RustCodeGraph 索引状态：项目包含 `11,467` 个文件、`307,296` 个节点和 `1,848,419` 条边；查询了 `pkg/expression/helper.rs` 全部 374 行及 `is_valid_current_timestamp_expr`、`get_time_current_timestamp`、`get_time_value`、`current_mysql_time` 的调用流。
- 图中确认的核心边：`get_time_current_timestamp -> current_mysql_time`、`get_time_value -> current_mysql_time`；调用者包括 `helper_test.rs` 与 `fts_to_like_36_aster_unit_test.rs` 中的对应测试。
- 已核对源码与装配：[helper.rs](helper.rs)、[lib.rs](lib.rs)、[Cargo.toml](Cargo.toml)。
- 已核对 Go 对照与测试：[helper.go](helper.go)、[helper_test.go](helper_test.go)。
- 已核对 Rust 独立测试：[helper_test.rs](helper_test.rs)、[fts_to_like_36_aster_unit_test.rs](fts_to_like_36_aster_unit_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo；结构检查用于确认目标文档存在且恰有规定的十一个二级章节。

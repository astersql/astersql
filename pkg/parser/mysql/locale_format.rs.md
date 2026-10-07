# `pkg/parser/mysql/locale_format.rs`

## 文件定位

本说明对应[源码 `locale_format.rs`](locale_format.rs)。该文件属于 `astersql-parser-mysql` crate。crate 入口 `pkg/parser/mysql/lib.rs` 以公开模块 `locale_format` 暴露它；`pkg/parser/mysql/Cargo.toml` 指定该 crate 的库入口为 `lib.rs`，并为这里的 Unicode 数字分类引入 `unicode-general-category = "1.1.0"`。它位于 SQL 表达式求值和 MySQL locale 规则表之间：`pkg/expression/builtin_string_vec.rs` 的 `format_value` 为 `FORMAT` 与 `FORMAT_WITH_LOCALE` 准备数值、精度和 locale，再调用本文件的 `FormatByLocale` 完成分组与小数点替换。

这是进程内的纯字符串格式化模块，不读取操作系统 locale、不访问网络或存储，也不负责数值舍入。上游 `format_value` 已经把实数或 decimal 转成指定精度的字符串；本文件按 locale 对该字符串进行展示格式化。

## 核心职责

- 用 `LocaleFormatStyle` 表达千分位分隔符、小数点符号以及是否使用印度式 `3,2,2,...` 分组。
- 用 `formatStyleMap` 定义八种格式风格，用 `localeToStyleMap` 把小写 locale 名称映射到风格 ID。
- 由 `GetLocaleFormatStyle` 做大小写不敏感查询，并在未知 locale 时返回 `CommaDot` 默认风格和 `found = false`。
- 由 `FormatByLocale` 保持 Go API 的三元返回形状：格式化文本、locale 是否命中、错误结果。
- 由 `formatWithStandardGrouping`、`formatWithIndianGrouping` 和 `formatWithStyle` 完成输入规范化、数字前缀截取、整数分组及小数位截断/补零。

该模块不负责向用户报告未知 locale。调用方 `pkg/expression/builtin_string_vec.rs::format_value` 根据 `found` 追加 `EvalWarning::UnknownLocale`；locale 为 SQL NULL 时调用方先回退到 `en_US`，并避免重复告警。

## 主要符号

- `pub struct LocaleFormatStyle`：可复制的三字段规则对象。`ThousandsSep` 和 `DecimalPoint` 是静态字符串，`IsIndianGrouping` 选择分组算法。字段及顺序对应 `pkg/parser/mysql/locale_format.go`。
- `styleCommaDot`、`styleDotComma`、`styleSpaceComma`、`styleNoneComma`、`styleAposDot`、`styleAposComma`、`styleNoneDot`、`styleIndian`：私有风格 ID；它们只作为两个映射表间的稳定连接键。
- `fn formatStyleMap() -> HashMap<&'static str, LocaleFormatStyle>`：每次调用构造八项风格表。
- `fn localeToStyleMap() -> HashMap<&'static str, &'static str>`：每次调用构造完整 locale 到风格 ID 的表；键均为小写。
- `pub fn GetLocaleFormatStyle(locale: &str) -> (LocaleFormatStyle, bool)`：公开查询入口。先执行 Unicode 小写化，再查询 locale 表；未命中时返回 `CommaDot` 和 `false`。
- `pub fn FormatByLocale(number, precision, locale) -> (String, bool, Result<(), ParseIntError>)`：公开格式化入口。当前实现总是返回 `Ok(())`，错误类型是为保持移植后的 API 形状而保留。
- `fn formatWithStandardGrouping(integer_part, thousands_sep) -> String`：从左向右按三位分组。
- `fn formatWithIndianGrouping(integer_part, thousands_sep) -> String`：保留最右三位，左侧按两位分组。
- `fn formatWithStyle(number, precision, style) -> String`：私有主算法，负责精度前缀、输入规范化、有效数字前缀、小数拆分和最终拼装。

文件没有 trait、impl、枚举或条件编译项。公开 API 只有 `LocaleFormatStyle`、`GetLocaleFormatStyle` 和 `FormatByLocale`，其余均为内部表或算法。

## 执行流程

1. 上游 `pkg/expression/builtin_string_vec.rs::eval_row` 将 `StringBuiltin::FormatWithLocale` 和 `StringBuiltin::Format` 分派到 `format_value`；后者把小数位限制在 `0..=30`，将 real/decimal 先格式化或舍入，再调用 `mysql::locale_format::FormatByLocale`。
2. `FormatByLocale` 调用 `GetLocaleFormatStyle`。后者对 locale 执行 `to_lowercase`，构造 locale 表和风格表；命中时返回对应规则和 `true`，否则返回逗号千分位、点小数点规则和 `false`。
3. `formatWithStyle` 从 `precision` 开头收集连续 ASCII 数字；空前缀按 `0` 处理，随后尝试解析为 `usize`。解析失败时 `position` 为 `None`，不会输出小数部分。
4. 输入 `-.5` 和 `.5` 分别规范化为 `-0.5` 和 `0.5`。首字节必须是 ASCII 数字，或为 `-` 且下一字节是 ASCII 数字；否则结果从 `0` 开始，并按有效正精度补零。
5. 对有效输入，算法去掉负号后扫描可接受前缀：Unicode `DecimalNumber` 字符继续通过；点号遵循 Go 移植逻辑。遇到其他字符便截断。随后按 `.` 拆分；只有恰好两段时才把第二段当作小数部分，多于一个点号时视为没有可用小数部分。
6. 整数部分若无千分位符则原样使用；印度风格调用 `formatWithIndianGrouping`，其他风格调用 `formatWithStandardGrouping`。最后恢复负号，并按 locale 小数点输出指定小数位：足够长则截断，不足则补 `0`。
7. `FormatByLocale` 返回结果与 `found`。上游根据 `found` 决定是否产生未知 locale 告警，并把字符串转为结果字节。

例如 `1234567890.1234`、精度 `3`、locale `en_IN` 走印度分组，得到 `1,23,45,67,890.123`；未知 locale 仍使用默认格式，但 `found` 为 `false`。

## 数据与状态

风格和 locale 数据以函数内数组收集成 `HashMap`，不是全局可变状态。所有键和值要么是 `&'static str`，要么是可复制的 `LocaleFormatStyle`；格式化期间只创建局部 `String`、`Vec<&str>` 和映射表。调用之间没有缓存，也没有跨请求状态。

关键不变量如下：

- `localeToStyleMap` 的值必须能在 `formatStyleMap` 中找到；`GetLocaleFormatStyle` 使用 `styles[style_id]` 索引，映射失配会 panic。
- 未知 locale 必须与已知 `en_US` 使用相同默认风格，但二者的 `found` 分别为 `false` 和 `true`。
- `ThousandsSep` 为空时完全跳过整数分组；`IsIndianGrouping` 只对 `styleIndian` 为真。
- 负号不参与整数分组，格式化后再放回输出开头。
- 精度按字节长度截取/补齐小数部分；该行为与 ASCII 数字输入最匹配。

## 依赖与调用关系

上游调用边由 RustCodeGraph 确认：

- `pkg/expression/builtin_string_vec.rs::format_value` → `pkg/parser/mysql/locale_format.rs::FormatByLocale`，用于向量化 `FORMAT`/`FORMAT_WITH_LOCALE`。
- `pkg/parser/mysql/locale_format_test.rs::format_by_locale_preserves_unicode_decimal_digits_after_ascii_prefix` → `FormatByLocale`，覆盖 ASCII 首位后接 Unicode 十进制数字的现有行为。
- `pkg/parser/mysql/error_3_aster_unit_test.rs::locale_formatting_matches_go_grouping_and_fallbacks` 也直接调用 `FormatByLocale`，覆盖标准分组、印度分组、未知 locale、非数字输入和重复小数点。

本文件内部主调用边为 `FormatByLocale` → `GetLocaleFormatStyle` / `formatWithStyle`，以及 `formatWithStyle` → `formatWithStandardGrouping` / `formatWithIndianGrouping`。标准库提供 `HashMap`、字符串和整数解析；`unicode_general_category::get_general_category` 判断扫描阶段的 Unicode 十进制数字。crate 的其他依赖 `astersql-errors` 与 `semver` 不被本文件直接使用。

模块由 `pkg/parser/mysql/lib.rs` 的 `pub mod locale_format` 接线，并通过工作区中的 parser/mysql crate 被表达式层引用。仓库在 `pkg/parser` 或 `pkg/parser/mysql` 下没有 `doc.go`，因此最近的模块边界证据是该 `lib.rs` 和 `Cargo.toml`。

## 错误处理与边界

- 未知 locale 不是本文件错误：返回默认风格、格式化结果和 `found = false`，由上游决定是否告警。
- 非数字开头不是错误：回退为 `0`，正精度时使用 locale 小数点并补零。
- 精度只读取开头连续 ASCII 数字；非数字开头视为零精度，过大或无法装入 `usize` 的前缀解析失败后也不输出小数部分。
- 空 `number`、仅 `-`、空 `precision` 等输入不会产生解析错误；它们经安全索引或空前缀逻辑回退。这里比会直接索引字符串的 Go 实现更防御性。
- 多个 `.` 的有效前缀可能被完整保留，但拆分结果不是恰好两段时不会采用小数部分；现有回归断言 `12.3.4`、精度 `2` 输出 `12.00`。
- 扫描阶段允许 ASCII 首位后的 Unicode 十进制数字，独立测试断言 `1٢` 在零精度下保持为 `1٢`。但两个分组函数及小数截断使用 UTF-8 字节长度与字符串字节切片；若分组边界或精度落在多字节字符内部，Rust 切片会 panic。扩展 Unicode 输入前必须先明确是保持 Go 的字节语义，还是改成字符语义，并增加独立回归测试。
- `FormatByLocale` 暴露 `Result<(), ParseIntError>`，但内部把精度解析降为 `Option`，当前没有返回 `Err` 的路径。调用方仍把该错误映射到 `EvalError::Format`，这是可观察 API 兼容面，不应无评估地移除。

## 并发与资源生命周期

模块没有锁、原子变量、线程、异步任务、通道、事务、文件句柄或网络连接。所有输入均为借用字符串，输出和临时缓冲区由单次调用独占；函数返回后局部映射和缓冲区正常释放，因此并发调用之间没有共享可变状态。

主要资源成本来自每次 `GetLocaleFormatStyle` 都重新构造两张 `HashMap`、locale 的小写副本，以及格式化过程中的若干 `String`/`Vec` 分配。若未来缓存映射，应使用只读的一次初始化结构并保持并发安全，同时验证启动成本、热路径分配和 locale 映射一致性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mysql/locale_format.go`。Rust 保留了 Go 的 `LocaleFormatStyle` 三字段、八种 style ID、完整 locale 表、未知 locale 默认回退、标准/印度分组以及 `FormatByLocale` 的 `(string, bool, error)` 语义形状。现有 Rust 测试和 `error_3_aster_unit_test.rs` 的断言体现了移植目标，而不是另行简化的算法。

已确认的实现差异与风险：

- Go 的两个 map 是包级变量；Rust 的 `formatStyleMap` 和 `localeToStyleMap` 每次查询重新构造 map，结果语义相同但分配成本不同。
- Go 使用 `unicode.IsDigit` 检查精度字符，Rust 精度前缀只接受 ASCII 数字。对常规 SQL 精度参数没有差异，但混合 Unicode 精度文本可能不同：Rust 可能保留先导 ASCII 前缀，而 Go 随后的 `ParseUint` 可能使整个精度无效。
- Rust 对空或过短输入使用安全检查，避免 Go 版本直接索引可能触发的越界。
- Go 字符串切片按字节工作，即使切开 UTF-8 也不会像 Rust `str` 切片那样立即 panic；Rust 对多字节十进制数字的分组/小数截断需要额外边界测试。
- Go 的 `formatWithStyle` 返回 `(string, error)`；Rust 私有函数只返回 `String`，但公开 `FormatByLocale` 仍保留错误槽且当前恒为 `Ok(())`。

仓库没有同名 `locale_format_test.go`；直接 Go 行为依据来自 `locale_format.go` 本身，Rust 独立测试位于 `pkg/parser/mysql/locale_format_test.rs`，更广的 Go 兼容回归位于 `pkg/parser/mysql/error_3_aster_unit_test.rs`。

## 扩展指南

- 新增或调整 locale：在 `localeToStyleMap` 修改映射；若需要新分隔组合，同时新增 style 常量和 `formatStyleMap` 项，并同步核对 `locale_format.go`。必须保持所有 locale map 值都能解析到 style。
- 改变分组：标准规则修改 `formatWithStandardGrouping`，印度规则修改 `formatWithIndianGrouping`，路由条件修改 `formatWithStyle`。应覆盖 0～4 位、多个完整分组、负数、空千分位符和印度式奇偶左侧长度。
- 改变精度或输入解析：集中修改 `formatWithStyle`，同时检查上游 `format_value` 的 `0..=30` 钳制和预舍入契约，避免在两层重复舍入。
- 改变未知 locale 策略：同时检查 `GetLocaleFormatStyle` 的 `found`、公开三元返回值和 `pkg/expression/builtin_string_vec.rs::format_value` 的告警逻辑。
- 性能优化可把两张表改成一次初始化的只读数据，但要用基准或分配证据证明收益，并确保没有引入可变全局状态。
- 测试必须放在独立文件，优先扩充 `pkg/parser/mysql/locale_format_test.rs`；跨模块 SQL 表达式行为则扩充相邻的 `pkg/expression/builtin_string_vec_*_test.rs`。不要把测试内嵌进本生产源文件。
- 任何 Unicode 支持扩展都应先添加能暴露 UTF-8 边界的回归用例，例如多位阿拉伯数字参与标准/印度分组和非零精度截断，并明确与 Go 字节语义的取舍。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；本次查询覆盖目标 Rust 文件及 Go 对照。
- RustCodeGraph `explore "pkg/parser/mysql/locale_format.rs locale_format FormatWithLocale"`：确认公开入口、内部格式函数、表达式层调用方及两处 Rust 测试使用方。
- RustCodeGraph `query` / `node`：读取并核对 `LocaleFormatStyle`、`GetLocaleFormatStyle`、`FormatByLocale`、`formatWithStyle`、两种 grouping 函数，以及 `pkg/expression/builtin_string_vec.rs::eval_row` / `format_value` 的调用现场。
- RustCodeGraph `callees FormatByLocale`：确认 Rust 与 Go 两个版本都调用各自的 `GetLocaleFormatStyle` 和 `formatWithStyle`；`callees formatWithStyle` 确认 Rust 版本调用两种分组函数。`callers` 命令在当前索引上超时，因此调用方以同一次 `explore` 的 blast radius 和已索引调用现场交叉确认。
- 读取的边界与配置文件：`pkg/parser/mysql/lib.rs`、`pkg/parser/mysql/Cargo.toml`。
- 读取的直接实现与对照：`pkg/parser/mysql/locale_format.rs`、`pkg/parser/mysql/locale_format.go`。
- 读取的独立测试：`pkg/parser/mysql/locale_format_test.rs`；RustCodeGraph 另确认 `pkg/parser/mysql/error_3_aster_unit_test.rs::locale_formatting_matches_go_grouping_and_fallbacks` 的覆盖范围。
- 未运行 Cargo 或代码测试：任务是纯文档分析，计划明确禁止运行 Cargo；结构验证及最终差异检查作为本任务的本地交付证据。

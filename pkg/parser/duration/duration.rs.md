# `pkg/parser/duration/duration.rs`

## 文件定位

本文件是 `astersql-parser-duration` crate 的唯一业务实现文件，负责把 AsterSQL 配置和 SQL 选项使用的简化时长文本转换为标准库 `std::time::Duration`。crate 边界由 `pkg/parser/duration/Cargo.toml` 定义；`pkg/parser/duration/lib.rs` 通过 `pub mod duration` 装载本文件，并用 `pub use duration::*` 将 `ParseDuration` 提升为 crate 根 API。

它不是通用的 Go `time.ParseDuration` 或 MySQL `TIME` 解析器。这里接受的是 TiDB 原有 `pkg/parser/duration/duration.go` 定义的窄语法：数值后只能跟 `d`、`h`、`m`，多个片段可以直接连接。当前 Rust 生产调用主要位于：

- `pkg/parser/parser_actions/admin.rs`：解析 `CALIBRATE RESOURCE` 的 `DURATION` 选项时即时校验文本。
- `pkg/parser/parser_actions/ddl.rs`：解析表选项 `TTL_JOB_INTERVAL` 时即时校验文本。
- `pkg/session/runtime/ttl_metadata.rs`：收集已启用 TTL 表的调度信息时，把间隔解析成秒。

RustCodeGraph 还将 `pkg/expression/builtin.rs` 列为本文件的使用者，但该文件实际调用的是 `types_dependency::time::ParseDuration`，不是本 crate 的函数；这是同名符号/文件依赖分析带来的近似结果，不能当作直接调用边。

## 核心职责

本文件只承担两层职责：

1. 私有函数 `read_float(&str) -> Result<(f64, &str), String>` 从当前片段头部切出十进制浮点 token，并返回尚未消费的后缀。
2. 公共函数 `ParseDuration(&str) -> Result<Duration, String>` 反复读取“数值 + 单字节单位”，换算为纳秒并累加，最终构造 `Duration`。

支持的典型输入包括 `1h`、`1.5d`、`1h100m` 和 `1d1.5h`。单位顺序没有额外约束，同一单位也可重复；语义只是按出现顺序累加。裸 `"0"` 有显式零值捷径，空串则因为主循环不执行而同样返回 `Duration::ZERO`。

本文件不负责修剪空白、负数、正号、科学计数法、秒或更小单位，也不负责将错误包装成 SQL 错误；解析器调用者负责把字符串错误附加到 lexer，TTL 调度调用者则通过 `?` 原样传播。

## 主要符号

### `read_float`

签名为 `fn read_float(s: &str) -> Result<(f64, &str), String>`，仅在本文件内可见。

- 用 `char_indices` 扫描 UTF-8 字符，以字节下标安全切片。
- 扫描阶段把 `char::is_numeric()` 判定为数字的字符和 `.` 都纳入 token；遇到第一个其他字符时停止。
- 只有停止位置大于零才调用 `str::parse::<f64>()`。因此首字符就是单位、符号或空白，以及遍历到结尾仍未遇到单位的输入，都会返回 `fail to read an integer`。
- Rust 的 `f64` 解析可能把范围溢出的合法字面量接受为无穷大；函数用 `is_finite()` 显式拒绝，以贴近 Go `strconv.ParseFloat` 的范围错误。
- 成功时返回数值和从第一个非数值字符开始的借用切片，不分配新的后缀字符串。

### `ParseDuration`

签名为 `pub fn ParseDuration(mut s: &str) -> Result<Duration, String>`。它保留 Go 导出函数名，因此用 `#[allow(non_snake_case)]` 局部关闭 Rust 命名警告。

- 输入和解析中的后缀都是借用的 `&str`，函数不持有调用者数据。
- 累加器 `duration_nanos: u64` 保存当前总纳秒数。
- 单位系数分别为一天、小时和分钟对应的浮点纳秒数。
- 每段结果通过 `(value * unit_nanos) as u64` 转为整数纳秒，再用 `saturating_add` 累加。
- 返回值是 `Duration::from_nanos(duration_nanos)`。

模块级没有自定义类型、trait、静态变量、条件编译项或异步入口；唯一导出符号是 `ParseDuration`。

## 执行流程

以 `1d1.5h` 为例，执行顺序如下：

1. `ParseDuration` 初始化纳秒累加器；输入不是裸 `0`，进入循环。
2. `read_float("1d1.5h")` 在 `d` 处停止，返回数值 `1.0` 和后缀 `"d1.5h"`。
3. `ParseDuration` 读取后缀首字节 `d`，把 `1.0` 乘以一天的纳秒数并加入累加器，然后跳过该 ASCII 单位。
4. 下一轮对 `"1.5h"` 调用 `read_float`，得到 `1.5` 和 `"h"`；换算为 1.5 小时后再次累加。
5. 跳过 `h` 后输入为空，循环结束，由累计纳秒构造 `Duration`。

解析器路径在构造 AST 选项时只使用成功/失败结果，不保存解析值：`admin.rs` 会把失败改写为 `The DURATION option is not a valid duration: ...`，`ddl.rs` 会改写为 `The TTL_JOB_INTERVAL option is not a valid duration: ...`。`ttl_metadata.rs::collect_ttl_schedules` 则真正使用返回值：调用 `.as_secs()` 得到调度秒数，同时保留原始表达式字符串。

## 数据与状态

函数的全部状态都在调用栈上：当前未消费切片、当前浮点数、单位字节和 `u64` 纳秒累加器。没有全局可变状态、缓存或隐藏配置。

重要不变量如下：

- 每次成功循环至少消费一个数值 token 和一个单字节 ASCII 单位，所以不会原地死循环。
- 只有 `d`、`h`、`m` 能通过单位分支；它们均为单字节，故 `&rest[1..]` 一定落在 UTF-8 字符边界。
- 小数片段先以 `f64` 计算，再转换为整数纳秒；转换会截去不足一纳秒的小数部分。
- 单段转换和总和都不会因整数溢出 panic：浮点转 `u64` 采用 Rust 的饱和转换语义，累计使用 `saturating_add`。超过范围时结果停在 `u64::MAX` 纳秒。
- `Duration` 没有负值表示，因此负数在扫描首字符 `-` 时直接失败。

空字符串返回零值是当前实现和测试确认的行为，但它不是函数注释中单独声明的语法特例；依赖这一行为的新增调用者应先确认是否需要把它上升为稳定契约。

## 依赖与调用关系

`pkg/parser/duration/Cargo.toml` 没有 `[dependencies]`，实现只依赖 `std::time::Duration`、字符串切片、`f64` 解析与格式化宏。workspace 根清单以 `facade_parser_duration` 注册该包；`pkg/parser/Cargo.toml` 以依赖别名 `parser-duration` 引入它，session crate 则直接以 `astersql-parser-duration` 引入。

已核实的调用关系为：

- `ParseDuration` → `read_float`：每个片段一次，错误用 `?` 立即返回。
- `parser_actions/admin.rs::apply_rule` → `parser_duration::ParseDuration`：校验动态资源校准的 duration 文本。
- `parser_actions/ddl.rs::apply_rule` → `parser_duration::ParseDuration`：校验 `TTL_JOB_INTERVAL` 表选项。
- `session/runtime/ttl_metadata.rs::collect_ttl_schedules` → `ParseDuration`：把模型中的 TTL job interval 转为调度秒数。

解析器两条边属于语法动作阶段，解析结果不进入对应 AST 字段，AST 仍保存原始字符串。TTL 元数据边属于运行期消费，会丢弃亚秒部分，因为调用者使用 `Duration::as_secs()`；当前语法最小单位是分钟，所以正常输入不会因此损失有效精度。

## 错误处理与边界

错误类型是无结构的 `String`，主要路径如下：

- 无法形成带终止单位的数值 token：`fail to read an integer`，例如 `"x"`、`"1"`、第二段为裸 `0` 的 `"1h0"`。
- token 不是 Rust `f64`：`invalid float ...`，例如多个小数点或包含 Unicode 数字但不能被 Rust 浮点解析器接受的 token。
- 浮点值为正/负无穷或 NaN：范围错误文本。扫描语法不能直接形成 `NaN`，该检查当前主要覆盖巨大十进制数字溢出为无穷的情况。
- 单位不是 `d`、`h`、`m`：`unknown unit X`，例如 `"1s"` 或 `"1H"`。

代码包含 `duration unit is missing` 分支，用于 `read_float` 成功而后缀为空的情形。但按当前 `read_float` 契约，只有遇到非数字终止字符才可能成功，因此成功后缀必定非空；裸数字会更早返回 `fail to read an integer`。该分支目前是防御性检查，不是可由公开输入触发的已验证错误。

其他边界：输入不做 `trim`，空白会成为未知单位或导致首字符扫描失败；不接受 `+`、`-`、指数记法；连续片段不允许分隔符。错误发生在后续片段时不会返回部分累计结果。公开 API 也不会 panic；切片跳过单位的安全性由只匹配 ASCII 单位这一分支保证。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、任务、通道、文件句柄、网络连接或事务。两个函数只读取输入并创建值类型结果，可由多个线程并发调用；线程安全来自无共享可变状态，而非显式同步。

`read_float` 返回的后缀生命周期绑定到输入 `&str`。`ParseDuration` 只在函数执行期间推进这些借用切片，返回值不再借用输入，因此调用结束后没有悬挂资源或清理动作。时间复杂度是输入字符数的线性级别，额外空间为常数；每个片段的错误格式化才可能分配 `String`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/duration/duration.go`：Rust `read_float` 对应 Go `readFloat`，Rust `ParseDuration` 对应 Go `ParseDuration`。两者保持相同的片段读取顺序、`d/h/m` 单位、复合片段累加、小数换算、裸 `0` 特判，以及空串因零次循环返回零值的现状。`pkg/parser/duration/duration_test.go::TestParseDuration` 的七个合法用例已在 Rust 独立测试中逐项复刻。

需要注意的实现差异：

- Go 用 `unicode.IsDigit` 扫描 rune 并按字节下标切片；Rust 用 `char_indices` 和 `is_numeric` 保存 UTF-8 字节边界。两者都可能先把非 ASCII 数字纳入 token，随后在浮点解析阶段报错；`migration_aster_unit_test.rs` 用阿拉伯-印度数字覆盖了这一迁移语义。
- Go `strconv.ParseFloat` 会为范围溢出返回错误；Rust 额外用 `is_finite` 弥合差异。
- Rust 对缺单位使用显式错误防护，避免 Go 原实现随后索引 `s[0]` 的潜在越界，但当前扫描契约使该分支不可达。
- Rust 明确使用 `u64` 饱和转换与饱和加法，而 Go 用有符号 `time.Duration` 直接相加。超大但仍有限的输入可能因此产生不同的极值行为；现有测试只验证浮点范围溢出，不把跨整数范围的结果声明为完全等价。

## 扩展指南

新增单位时，应同时修改 `ParseDuration` 的单位匹配和独立测试，而不是只放宽扫描器。例如增加秒单位需要考虑 TTL 调用者 `.as_secs()` 的亚秒截断；增加多字符单位则不能继续无条件使用 `rest.as_bytes()[0]` 和 `&rest[1..]`。

扩展数值语法（符号、指数、空白）应从 `read_float` 入手，并明确 token 边界与 Go 兼容性。若需要负时长，不能仅允许 `-`，还必须更换返回类型或定义拒绝负结果的上层契约，因为 `std::time::Duration` 只能表示非负值。

修改错误文本前应审查解析器包装后的用户可见错误及可能依赖文本的测试。若要让调用者区分“缺数字”“缺单位”“未知单位”和“溢出”，更稳妥的方向是引入错误枚举，但这会改变公开返回类型，需要同步所有调用点。

测试必须继续放在独立文件中：常规行为更新 `pkg/parser/duration/duration_test.rs`，Go 迁移差异更新 `pkg/parser/duration/migration_aster_unit_test.rs`，并同步检查 `pkg/parser/duration/duration_test.go` 的原始意图。解析器调用契约变化还应覆盖对应 parser actions 测试；TTL 消费语义变化则应覆盖 session runtime 的 TTL 测试。不要把测试内嵌到 `duration.rs`。

性能方面应保留单次线性扫描和借用后缀，避免为每个片段创建临时字符串。兼容性方面尤其要谨慎处理空串、大小写、非 ASCII 数字、浮点截断与超大值，这些边界比新增普通合法用例更容易造成 Go/Rust 行为漂移。

## 验证依据

本说明基于以下直接证据：

- 实现与装配：`pkg/parser/duration/duration.rs`、`pkg/parser/duration/lib.rs`、`pkg/parser/duration/Cargo.toml`。
- Go 对照：`pkg/parser/duration/duration.go`、`pkg/parser/duration/duration_test.go`。
- Rust 独立测试：`pkg/parser/duration/duration_test.rs`、`pkg/parser/duration/migration_aster_unit_test.rs`。
- 直接调用现场：`pkg/parser/parser_actions/admin.rs`、`pkg/parser/parser_actions/ddl.rs`、`pkg/session/runtime/ttl_metadata.rs`，以及相关 `pkg/parser/Cargo.toml`、`pkg/session/Cargo.toml` 依赖声明。
- RustCodeGraph：`status` 显示索引包含本目录的 Go/Rust 文件；`files --filter pkg/parser/duration` 确认实现、入口和测试集合；`node --file pkg/parser/duration/duration.rs`、`query ParseDuration`、`node ParseDuration`、`node read_float`、`callers/callees` 确认符号源码和 `ParseDuration -> read_float` 及生产调用边。通用 callers 查询受同名定义影响，最终调用结论以限定路径的 node 输出和调用现场交叉核验。

本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务文件指定命令，要求本文档存在且恰好包含上述十一个固定二级标题；语义验收通过逐项对照源码、Go 版本、独立测试和调用现场完成。

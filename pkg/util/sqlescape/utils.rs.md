# `pkg/util/sqlescape/utils.rs`

## 文件定位

本文件是 `astersql-util-sqlescape` crate 的实际实现文件，负责把带有 `%?`、`%n`、`%%` 占位符的 SQL 模板渲染为 SQL 文本。crate 入口 `pkg/util/sqlescape/lib.rs` 通过 `pub mod utils` 挂载本模块，并以 `pub use utils::*` 再导出公开项；根 façade `pkg/lib.rs` 的 `util::sqlescape` 又转发该 crate。因此它是通用 SQL 文本拼装工具，不负责解析、规划或执行 SQL。

`pkg/util/sqlescape/Cargo.toml` 声明 crate 名为 `astersql-util-sqlescape`，运行时依赖只有 `chrono`（构造和格式化 Go 对应的时间值）与 `ryu`（浮点数最短往返格式化），移植元数据明确对应 Go 包 `pkg/util/sqlescape`。当前检索到的直接生产使用包括：

- `pkg/session/starter_bootstrap_file.rs::render_starter_bootstrap_sql` 调用 `EscapeString` 转义 starter bootstrap 模板中的 keyspace；
- `pkg/statistics/handle/autoanalyze/exec/exec.rs::escape_analyze_sql` 把字符串参数包装成 `SqlArg::String` 后调用 `EscapeSQL`，供自动分析 SQL 和告警文本使用；
- 其他 crate 也在 Cargo 清单中声明该依赖，而公开 API 还经根 façade 暴露；是否实际调用应以各调用文件为准，不能仅由清单依赖推断。

## 核心职责

1. `escapeSQL` 按字节扫描模板，识别 `%?`（值参数）、`%n`（标识符）和 `%%`（字面量百分号），维持与 Go 实现相同的参数消费顺序。
2. `appendSQLArg` 将 `SqlArg` 的各种变体转换为 SQL 文本：空值、整数、无符号整数、浮点数、布尔、时间、JSON 原始消息、二进制、字符串和切片均有独立语义。
3. `escapeBytesBackslash` / `escapeStringBackslash` 实现 MySQL 风格反斜杠转义；`EscapeString` 暴露“不加引号、只转义内容”的窄接口。
4. `EscapeSQL` 与 `FormatSQL` 分别提供返回 `String` 和写入 `io::Write` 的接口；`MustEscapeSQL` 与 `MustFormatSQL` 将错误升级为 panic。
5. `SqlEscapeError` 把 Go 版由 `errors.Errorf` 产生的缺参、标识符类型错误、不支持类型和 writer 错误显式建模。

本工具只在调用方模板本身正确时安全。源文件在 `EscapeSQL` 注释中明确给出把 `%?` 放入已有引号中的反例；函数不会分析 SQL 上下文，也不能自动修复危险模板。

## 主要符号

- `SqlEscapeError`：公开错误枚举。`MissingArguments { need, got }`、`ExpectedStringIdentifier { got }`、`UnsupportedArgument { position, value }` 保持 Go 错误文案，`Io(String)` 保存 writer 错误文本；实现 `Display`、`Error` 和 `From<io::Error>`。
- `GoTime`：公开时间适配类型，内部保存 `is_zero` 与预格式化字符串。`zero()` 表示 Go `time.Time{}`；`from_naive` 输出秒或最多六位、去尾零的微秒；`from_components` 对非法年月日或时间分量返回 `None`。
- `ReflectedValue`：公开枚举，用于机械表达 Go type switch 未命中后按 `reflect.Kind` 处理的别名类型慢路径；它不是 Rust 运行时反射。
- `SqlArg`：公开参数和值类型枚举，显式列出 Go `args ...any` 支持的全部主路径。`Bytes(None)` 与 `Bytes(Some(Vec::new()))` 被刻意区分为 `NULL` 和 `_binary''`。
- `reserveBuffer(Vec<u8>, usize) -> Vec<u8>`：私有缓冲 helper；不足时按 Go 公式 `len * 2 + appendSize` 分配，并把长度扩到可按下标写入的范围。
- `escapeBytesBackslash` / `escapeStringBackslash`：私有转义核心，处理 NUL、换行、回车、`0x1a`、单双引号和反斜杠；其余字节原样保留。
- `EscapeString(&str) -> String`：公开的无引号字符串转义接口。
- `escapeSQL(&str, &[SqlArg]) -> Result<Vec<u8>, SqlEscapeError>`：私有模板扫描器，是 `EscapeSQL` 和 `FormatSQL` 的共同核心。
- `EscapeSQL` / `MustEscapeSQL`：公开的字符串返回接口及 panic 包装。
- `FormatSQL<W: Write>` / `MustFormatSQL`：公开 writer 接口及 `Vec<u8>` 特化的 panic 包装。
- `next_arg`：统一推进参数位置并生成精确缺参错误。
- `appendSQLArg` / `appendReflectedSQLArg`：值类型分派核心与 Go 反射慢路径映射。
- `appendGoInt`、`appendGoUint`、`appendGoFloat32`、`appendGoFloat64`、`appendGoFloatText`、`appendGoExponent`、`appendGoExponentSuffix`：保持 Go `strconv` 数值文本规则的 helper，尤其处理 `Inf`、`NaN`、正指数的 `+` 和至少两位指数。

## 执行流程

一次 `EscapeSQL(sql, args)` 调用依次经历：

1. `EscapeSQL` 调用 `escapeSQL`，以模板长度创建字节缓冲区。
2. `escapeSQL` 从当前位置寻找下一个 `%`；找到前先原样追加此前的字节，找不到则追加剩余模板并结束。
3. `%n` 通过 `next_arg` 消费一个参数，只接受 `SqlArg::String`，把标识符包在反引号中，并把内部反引号替换为双反引号。
4. `%?` 消费一个参数并交给 `appendSQLArg`。数值直接写文本；布尔写 `1`/`0`；字符串、JSON 和非空二进制先加相应引号/前缀再反斜杠转义；时间按零值或秒/微秒文本输出；切片逐项输出并用逗号连接。
5. `%%` 写一个 `%` 且不消费参数；未知格式符或末尾孤立 `%` 只写回 `%`，下一轮继续处理其后内容。
6. 输入参数多于占位符时，多余参数不报错；输入少于占位符时，在尝试消费缺失参数处立即返回 `MissingArguments`。
7. `EscapeSQL` 将结果按 `String::from_utf8_lossy` 转成字符串；`FormatSQL` 对传入 writer 执行一次 `write`。Must 版本在任一错误上 panic。

浮点路径先由 `ryu::Buffer` 产生最短往返文本，再由 `appendGoFloatText` 修正 Go 表达习惯：特殊值使用 `+Inf`、`-Inf`、`NaN`；科学计数法含显式指数符号和至少两位指数；显著指数不在 `[-4, 6)` 时切换到指数形式。这些边界由 `migration_aster_unit_test.rs` 的 `1e6`、`1e-4`、`1e-5`、`1e-7` 和特殊值用例补充验证。

## 数据与状态

模块没有全局可变状态。一次调用的状态全部局限于栈与所拥有的缓冲区：`escapeSQL` 保存输出 `Vec<u8>`、模板字节游标 `i` 和已消费参数数 `argPos`；`next_arg` 是唯一修改 `argPos` 的位置。`SqlArg`、`GoTime` 与 `ReflectedValue` 都是按值构造的普通数据类型，模块不缓存它们。

重要不变量如下：

- `argPos` 只在成功取得 `%?` 或 `%n` 参数时递增，错误位置使用一基序号；
- `reserveBuffer` 扩展后保留旧前缀，新增区域先置零，`escapeBytesBackslash` 最终以 `truncate(pos)` 删除未使用的预留尾部；
- 每个输入字节最多扩张成两个输出字节，因此 `len(v) * 2` 是转义函数的最坏情况预留量；
- `Bytes(None)` 表示 Go 的 nil byte slice，而 `Nil` 表示 nil interface，两者都输出 `NULL`；非 nil 空 byte slice 输出二进制空字面量；
- `StringSlice` 和浮点切片只生成逗号分隔项，不自动添加括号，括号必须由模板提供。

## 依赖与调用关系

内部主调用链由 RustCodeGraph 核实为：

`EscapeSQL` / `MustEscapeSQL` / `FormatSQL` / `MustFormatSQL` → `escapeSQL` → `next_arg`、`appendSQLArg` → 转义或数值格式化 helper。

`appendSQLArg` 的下游包括 `escapeBytesBackslash`、`escapeStringBackslash`、`appendSQLArgBool`、`appendSQLArgString`、`appendReflectedSQLArg` 以及整数/浮点格式化函数。标准库提供 `Vec<u8>`、`io::Write`、错误 trait 和字符串格式化；`chrono::{NaiveDate, NaiveDateTime, NaiveTime}` 只服务于 `GoTime`；`ryu` 只服务于浮点格式化。

调用侧上，`render_starter_bootstrap_sql` 只使用无引号的 `EscapeString`，调用者必须确保替换位置的 SQL 引号语境正确；`escape_analyze_sql` 使用 `EscapeSQL`，但当前在错误时回退原 SQL，因此修改错误条件可能改变自动分析的日志和执行文本。`pkg/session/test/session_test.rs` 还以 `MustEscapeSQL` 生成 `_binary'...'` 插入值，并验证用户输入字符串和缺参错误契约。

RustCodeGraph 对目标内部 callees 给出了有效边，但对公开符号的 `callers` 查询未产出条目；上述外部调用点因此在图查询后用精确 `rg` 与对应源文件片段补齐，未把名称相同的其他模块函数误认为本 crate 调用。

## 错误处理与边界

- 缺参、错误的 `%n` 类型、不支持的 `SqlArg`/`ReflectedValue` 都返回 `SqlEscapeError`；普通接口不 panic。
- `GoTime::from_components` 用 `Option` 拒绝非法日期/时间，而不是构造无效值。
- `FormatSQL` 先完整渲染再写入；渲染失败时 writer 未被调用。writer 错误经 `From<io::Error>` 转成 `Io(String)`。
- `FormatSQL` 与 Go 一样只调用一次 `write` 并忽略成功返回的字节数。因此一个返回 `Ok(n)` 且 `n < buf.len()` 的非标准 writer 会导致静默短写；扩展通用 writer 行为时应先评估是否改为 `write_all` 会破坏 Go 对齐。
- `MustEscapeSQL` 和 `MustFormatSQL` 明确以 panic 表示调用方静态保证被破坏，只应用于参数与模板受控的路径。
- `EscapeString` 使用 `from_utf8_lossy`，但输入本身是合法 UTF-8 的 `&str`，现有转义只插入 ASCII，因此正常路径不会替换字符。二进制参数保留任意字节进入最终缓冲；`EscapeSQL` 返回字符串时，无效 UTF-8 字节会被 lossy 转换，这与 Go 的任意字节字符串并非完全等价，当前测试只覆盖有效/控制字节组合。
- 未知格式符不报错；多余参数也不报错。这两项是现有 Go 测试锁定的兼容行为，不能作为“严格格式串校验”来依赖。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络连接。公开函数只读取借用的模板与参数，并拥有自己的输出缓冲，因此不同线程间并发调用没有共享模块状态；实际能否跨线程共享参数仍取决于调用方如何持有 `SqlArg` 容器。

`FormatSQL` 借用 writer 的生命周期仅覆盖函数调用；它不关闭、不 flush、也不保留 writer。渲染缓冲在一次写入后释放。`MustFormatSQL` 限定为 `Vec<u8>`，用内存缓冲表达 Go `strings.Builder` 的不失败写入前提。最大的瞬时资源风险来自超长模板/参数和 `len(v) * 2` 的预留计算；当前实现没有长度上限，也没有显式处理 `usize` 溢出或分配失败。

## 与 Go 版本的对应关系

Rust 文件按 `pkg/util/sqlescape/utils.go` 逐分支移植：占位符扫描、反斜杠字符表、标识符反引号翻倍、参数序号、nil/空 byte slice 区别、时间格式、切片输出、额外参数丢弃、未知格式符保留以及 Must panic 文案均有直接对应。`pkg/util/sqlescape/utils_test.rs` 复刻 Go `utils_test.go` 的 reserveBuffer、特殊字节和 42 组 EscapeSQL 用例，并让私有核心、字符串接口和 writer 接口接受同一组断言。

语言差异主要有：

- Go 用 `...any`、type switch 和 `reflect.Kind`；Rust 用封闭的 `SqlArg` / `ReflectedValue` 枚举，新增支持类型必须显式增加变体与 match 分支。
- Go `time.Time` 直接携带时间与位置；Rust `GoTime` 只保存零值标志和从无时区 `NaiveDateTime` 预格式化的文本，不表达 Go 的完整位置/单调时钟语义。
- Go 错误动态构造；Rust 用枚举分类，但 `ExpectedStringIdentifier` 和 `UnsupportedArgument` 的值展示来自 `Debug` 辅助文本，源注释明确不承诺与 Go `fmt %v` 对所有值完全一致。
- Go 字符串可含任意字节；Rust 模板和普通字符串参数要求 UTF-8。二进制值通过 `Vec<u8>` 保留，但最终字符串返回存在 lossy 转换差异。
- Go `FormatSQL` 接受任意 `io.Writer`，Rust 用泛型 `W: Write`；Go `MustFormatSQL` 要求 `*strings.Builder`，Rust 对应为 `&mut Vec<u8>`。

`migration_aster_unit_test.rs` 额外覆盖浮点指数与特殊值、全部公开 wrapper、时间/JSON/二进制组合和缺参 panic，补足逐句移植测试之外的 Rust 表示层风险。

## 扩展指南

新增参数类型时，应同时修改 `SqlArg`、`appendSQLArg`，必要时修改 `ReflectedValue` / `appendReflectedSQLArg`，并在独立的 `pkg/util/sqlescape/utils_test.rs` 增加成功、边界和错误用例；不要把 Rust 测试嵌入本生产文件。若 Go 侧也支持该类型，应逐项对齐 `utils.go` 的 type switch、输出字面量和错误文案，并同步 Go `utils_test.go` 或记录明确的迁移差异。

新增占位符时，接入点是 `escapeSQL` 的 `match ch`。必须明确它是否消费参数、如何处理缺参、是否作用于标识符或值、未知/截断输入是否仍兼容，并对 `EscapeSQL`、`FormatSQL` 与 Must wrapper 做一致性测试。改变 `%?`、`%n`、`%%` 现有语义会直接影响 SQL 文本兼容性与注入边界，风险高于增加封闭的新格式符。

修改浮点或时间格式时，应优先添加与 Go `strconv.AppendFloat` / `time.AppendFormat` 的对照向量，特别覆盖阈值指数、负零、非有限值、微秒截断和零时间。修改缓冲策略时应保留前缀、实际长度和最坏情况容量性质，并警惕整数溢出与大分配。

若要增强 writer 正确性（例如处理短写），应先确认是否允许偏离 Go 的单次 `Write` 行为，再在独立测试中加入自定义短写/报错 writer。任何安全相关扩展都不能宣称此工具能理解 SQL 上下文；推荐继续要求调用方把占位符置于正确、未预加引号的位置。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录包含 `lib.rs`、`utils.rs`、两份 Rust 测试/迁移测试及 Go 对照文件。
- RustCodeGraph `node --file pkg/util/sqlescape/utils.rs`：读取完整 646 行，确认所有公开/私有符号、分支、错误与注释。
- RustCodeGraph `query escapeSQL/EscapeSQL/MustEscapeSQL`：确认 Go/Rust 同名定义及位置；`callees escapeSQL`、`callees appendSQLArg`：确认模板扫描到参数分派、转义与数值 helper 的内部调用边。
- RustCodeGraph `node`：读取 `pkg/util/sqlescape/lib.rs`、`utils_test.rs`、`utils.go`、`utils_test.go`、`migration_aster_unit_test.rs`，并读取 `pkg/session/starter_bootstrap_file.rs`、`pkg/statistics/handle/autoanalyze/exec/exec.rs`、`pkg/lib.rs`、`pkg/session/test/session_test.rs` 的直接调用片段。
- 配置证据：`pkg/util/sqlescape/Cargo.toml` 的 crate、依赖和 Go 移植元数据；根 `Cargo.toml` workspace/facade 声明及调用 crate 的 Cargo 清单由 `rg` 核对。
- 测试证据：Rust `utils_test.rs` 验证缓冲扩容、特殊字节、42 组模板/类型/错误路径、Must panic 与 `EscapeString`；Go `utils_test.go` 提供原始契约；Rust `migration_aster_unit_test.rs` 补充浮点、组合类型与 wrapper 契约。本任务遵循纯文档要求，未运行 Cargo，也未声称通过运行时测试。

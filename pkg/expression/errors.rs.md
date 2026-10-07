# `pkg/expression/errors.rs`

## 文件定位

本文件属于 `astersql-expression` crate。`pkg/expression/Cargo.toml` 指定 `lib.rs` 为库入口并关闭自动测试发现；`pkg/expression/lib.rs:324-325` 通过 `#[path = "errors.rs"] mod expression_errors_kernel;` 将本文件作为私有内核模块编入 crate，`lib.rs:850-853` 只在 `cfg(test)` 下把该模块内容重导出为 `expression_errors`，供独立测试使用。

它对应 Go 的 `pkg/expression/errors.go`：集中构造 Expression 错误类的标准错误，并提供三条依赖求值上下文的错误/警告分流逻辑。它不是通用 Rust `Error` 枚举，也不负责解析 SQL、求值表达式或定义 `EvalContext`。当前 Rust 生产接线仍是局部的：直接源码引用可确认 `pkg/expression/builtin.rs:4234` 调用无效时间处理器，`pkg/expression/extension.rs:209` 使用权限错误；大量已定义错误目前没有来自其他生产 Rust 文件的直接引用，不能因为定义已存在就推断 Go 表达式错误路径已经全部迁移。

## 核心职责

- 以 `standard_error!` 把 `astersql-errno` 的 MySQL/TiDB 错误码绑定到 `dbterror::ClassExpression`，保持错误类别、编号和格式化模板的统一（`errors.rs:35-41`）。
- 声明 15 个 `pub` 标准错误、1 个 `pub(crate)` 错误及包内私有错误，覆盖参数数量、除零、正则、JSON/函数索引、zlib、字符集、用户锁、序列权限等类别（`errors.rs:43-107`）。`ErrFunctionsNoopImpl`、`errDefaultValue` 和 `errUnsupportedJSONComparison` 使用自定义消息，其余主要复用 errno 的标准消息。
- 将 `dbterror::terror::Error` 克隆进引用计数式 `SharedError`，供上下文接口安全持有和传播（`shared`, `errors.rs:109-112`）。
- 仅对六类日期时间错误调用 `errCtx(ctx).HandleError`，让错误组级别决定返回错误、追加警告或忽略；无关错误保持原对象返回，`None` 保持 `None`（`handleInvalidTimeError`, `errors.rs:114-132`）。
- 将除零错误统一交给错误上下文分流（`handleDivisionByZeroError`, `errors.rs:134-138`）。
- 根据类型上下文的截断标志，把 `max_allowed_packet` 溢出转换为 warning 或带 trace 的硬错误（`handleAllowedPacketOverflowed`, `errors.rs:140-157`）。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `DbError` | 私有类型别名：`LazyLock<Box<dbterror::terror::Error>>`。标准错误第一次解引用时初始化，之后进程内复用。 |
| `ExpressionError` | 私有类型别名：`contextutil::errors::SharedError`，是三个处理函数的共享错误载体。 |
| `standard_error!` | 私有宏；接收可见性、静态名和 errno 常量，生成惰性 `ClassExpression.NewStd` 错误。 |
| `ErrIncorrectParameterCount` 至 `ErrFunctionNotExists` | 15 个公开静态标准错误，语义与 Go 文件公开变量逐项对应；模块本身为私有，当前并未由生产根模块整体重导出。 |
| `ErrFunctionsNoopImpl` | 公开静态错误；错误码为 `ErrNotSupportedYet`，但保留 Go 的 noop-function 专用消息模板。 |
| `errZlibZData` 至 `errUnsupportedJSONComparison` | 私有错误目录；仅 `errSpecificAccessDenied` 放宽为 `pub(crate)`，供 `extension.rs` 使用。 |
| `shared` | 私有转换函数；克隆 terror 错误并构造 `SharedError`。 |
| `handleInvalidTimeError` | 公共函数；签名为 `(&dyn EvalContext, Option<SharedError>) -> Option<SharedError>`，只路由六种指定时间错误。 |
| `handleDivisionByZeroError` | 公共函数；签名为 `(&dyn EvalContext) -> Option<SharedError>`，向 `errCtx` 提交 `ErrDivisionByZero`。 |
| `handleAllowedPacketOverflowed` | 公共函数；签名为 `(&dyn EvalContext, &str, u64) -> Result<(), SharedError>`，格式化函数名和包上限后按类型标志分流。 |

文件没有 struct、enum、trait、impl、异步函数或条件编译项。`pub` 表示对父模块可见，但 `expression_errors_kernel` 自身是私有模块；真正的 crate 外暴露程度必须以 `lib.rs` 的再导出为准。

## 执行流程

1. 标准错误初始化：某个静态值第一次被访问时，`LazyLock` 执行闭包。普通条目调用 `dbterror::ClassExpression.NewStd(errno::<code>)`，由 errno/errname 表提供错误编号与消息；三个特殊条目调用 `NewStdErr` 注入固定消息模板。后续访问复用同一个只读错误对象。
2. 无效时间：`handleInvalidTimeError` 先用 `?` 处理空错误；再依次调用六个标准错误的 `Equal(Some(&err))`，识别 `ErrWrongValue`、`ErrWrongValueForType`、`ErrTruncatedWrongVal`、`ErrInvalidWeekModeFormat`、`ErrDatetimeFunctionOverflow` 和 `ErrIncorrectDatetimeValue`。未命中时原样返回；命中时委托 `errCtx(ctx).HandleError`。
3. 除零：`handleDivisionByZeroError` 用 `shared(&ErrDivisionByZero)` 生成本次共享错误，再交由 `HandleError`。`ErrGroupDividedByZero` 的当前级别决定它成为错误、warning 或被忽略。
4. 包溢出：`handleAllowedPacketOverflowed` 用 `FastGenByArgs` 把 `expression_name` 与 `max_allowed_packet_size` 填入标准警告模板，读取 `typeCtx(ctx).Flags()`。如果 `TruncateAsWarning` 或 `IgnoreTruncateErr` 为真，则调用 `AppendWarning` 并返回 `Ok(())`；否则通过 `contextutil::errors::Trace` 返回 `Err`。
5. 上游求值处理结果：当前已接线的 `builtin.rs::CoreBuiltinKind::Weekday` 把 `Some(error)` 传入时间路由，返回 `Some` 时终止求值，返回 `None` 时产生 SQL `NULL`。其余两个处理器当前主要由独立 Rust 测试验证；Go 版本则由算术和字符串 builtins 广泛调用。

## 数据与状态

全局状态只包含惰性初始化、初始化后不可变的标准错误句柄。`LazyLock` 保证并发首次访问只构造一次；文件没有可变静态量。实际 warning、错误级别和类型标志不保存在本文件，而由传入的 `EvalContext` 提供：`errCtx(ctx)` 返回错误分组策略，`typeCtx(ctx)` 返回类型标志和 warning appender。

每次分流生成或接收的 `SharedError` 由共享所有权管理。`shared` 会克隆标准 terror 错误，`FastGenByArgs` 会创建包含本次参数的错误；本文件不缓存带调用参数的错误，因而不会把某个会话的消息泄漏给其他会话。`handleInvalidTimeError` 对不相关错误原样返回；独立测试用 `ptr_eq` 验证共享错误身份未被替换。

## 依赖与调用关系

- 模块边界：`pkg/expression/lib.rs:324-325` 编入本文件；`lib.rs:850-853` 的测试门面让 `errors_35_aster_unit_test.rs` 可以导入私有内核。`Cargo.toml` 的 `autotests = false` 意味着该测试还必须由 `lib.rs:659-662` 显式装配。
- 下游错误定义：`dbterror-dependency` 提供 `ClassExpression` 与 terror 错误，`errno-dependency` 提供错误码，`parser-mysql-dependency` 提供自定义 `Message`，`contextutil-dependency` 经 crate 门面提供 `SharedError`、`Trace` 和 warning 机制。这些依赖均在 `pkg/expression/Cargo.toml` 声明。
- 下游上下文：三个处理器通过 crate 的 `EvalContext`、`errCtx`、`typeCtx` 访问调用者持有的语句策略；它们不拥有上下文本身。
- 当前 Rust 生产上游：`pkg/expression/builtin.rs:4234-4241` 的 `Weekday` 路径调用 `expression_errors_kernel::handleInvalidTimeError`；`pkg/expression/extension.rs:209` 调用 `expression_errors_kernel::errSpecificAccessDenied.FastGenByArgs`。精确搜索没有发现其他生产 Rust 文件直接调用三个处理器。
- 名称边界：`pkg/expression/core_support.rs:153` 另有一个不同类型的 `ErrIncorrectParameterCount`，并通过其他根级再导出供 `planner_bridge.rs` 等使用。因此 `crate::ErrIncorrectParameterCount` 的调用不能自动归因到本文件；文档只把带 `expression_errors_kernel` 路径或测试门面导入的引用计作本文件调用。
- Go 上游：`builtin_time.go`/`builtin_cast.go`/向量化时间实现调用时间处理器，`builtin_arithmetic.go` 调用除零处理器，`builtin_string.go` 与 `builtin_cast.go` 调用包溢出处理器。这些是语义对照，不是 Rust 当前调用边。

RustCodeGraph 的文件节点报告本文件被 `aggregation/base_func_test.rs`、`builtin.rs`、`errors_35_aster_unit_test.rs`、`exprstatic/evalctx_test.rs` 和 `exprstatic/exprctx_test.rs` 使用。由于 Go/Rust 同名符号较多，索引的通用 `explore` 产生歧义，限定 `callers`/`callees` 查询未输出边；上面的精确生产关系因此由限定路径 `rg` 与源码位置补证，没有把空图结果解释为“无调用”。

## 错误处理与边界

- `handleInvalidTimeError(None)` 立即返回 `None`；非六类时间错误不进入 `errCtx`，而是保持同一 `SharedError` 返回。新增错误类型若未显式加入判定，将不会获得时间错误的 warning/ignore 策略。
- 六类“候选时间错误”不保证最终都变成 warning：最终结果取决于该错误在 `errctx` 中所属的错误组及该组级别。现有测试特别证明 `ErrInvalidWeekModeFormat` 虽被识别，但按当前 Go 对齐的分组并未登记进 Truncate 组，因此在该测试上下文中仍原样返回。
- 除零分流完全服从 `ErrGroupDividedByZero`：Warn 追加一个 warning 并返回 `None`，Error 返回错误，Ignore 不产生 warning 且返回 `None`。本文件不自行读取 SQL mode。
- 包溢出路径中 `IgnoreTruncateErr` 仍会调用 `AppendWarning`；这是当前 Go 实现的精确行为，不应按标志名称擅自改成静默忽略。只有两个标志都为假时才返回 traced error。
- `Trace(Some(err)).expect("present error remains present")` 依赖 `Trace` 对 `Some` 不丢失错误的不变量；如果下游 API 契约改变，这个 `expect` 会 panic，应同步改为无 panic 的显式传播。
- 静态错误消息和 errno 是兼容接口。更换错误码、参数顺序或特殊消息会改变客户端可见编号/文本以及 `Equal` 分类结果。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或 I/O 资源。`LazyLock` 内部提供一次初始化同步；初始化结果只读，适合跨线程共享。每次调用只短暂借用 `&dyn EvalContext`，函数返回后不保存引用。

warning 的并发安全和生命周期由具体 `EvalContext`/`WarnAppender` 实现负责。本文件按一次表达式求值同步追加 warning，不做批处理或重试。测试上下文以 `Arc<StaticWarnHandler>` 共享收集器，这证明接口允许共享所有权，但不能据此推断所有生产上下文都允许多个线程同时写同一语句 warning 列表；调用方仍须遵守求值上下文自己的并发契约。

## 与 Go 版本的对应关系

主对照文件 `pkg/expression/errors.go` 与本文件的错误目录、三个特殊消息及三个处理函数基本逐项对应：

- Go 包级 `var` 由 Rust `LazyLock<Box<terror::Error>>` 表达；`standard_error!` 消除重复声明，但仍使用同一个 `ClassExpression` 和 errno。
- Go 的普通 `error`/`nil` 对应 Rust 的 `Option<SharedError>`；`handleInvalidTimeError` 保留 nil 短路、六类 `Equal` 判断与非目标错误原样返回。
- Go `handleDivisionByZeroError` 直接把 `ErrDivisionByZero` 交给 `HandleError`；Rust 先用 `shared` 转成上下文需要的共享错误，策略结果一致。
- Go 包溢出函数用 `FastGenByArgs(exprName, size)`，在 `TruncateAsWarning || IgnoreTruncateErr` 时追加 warning，否则 `errors.Trace(err)`；Rust 保留同一参数顺序、分支和 trace 语义，只把返回类型表示为 `Result<(), SharedError>`。
- `pkg/expression/errors_35_aster_unit_test.rs` 验证 Warn/Error/Ignore 的除零分支、两个截断标志及消息参数、五个已分组时间错误、未分组 week-mode 行为、严格模式、无关错误身份与空错误。

差异主要在接线范围而非本文件算法：Go 的同包未导出符号天然可供大量 builtin 文件使用，Rust 的私有内核必须显式通过限定路径或门面访问；当前 Rust 直接生产引用远少于 Go。Go 文件中的每个声明也不等于 Rust 同名声明已替代所有运行路径。

## 扩展指南

1. 新增标准 Expression 错误时，优先用 `standard_error!`，并确认 errno/errname 已定义正确编号和参数模板；只有消息确实不来自标准表时才使用 `NewStdErr`。同步核对 `pkg/expression/errors.go`，不要创造无 Go 依据的兼容差异。
2. 新增需要按时间策略处理的错误时，把精确 `Equal` 判断加入 `handleInvalidTimeError`，同时确认它在 `errctx` 的目标错误组中有映射；仅加入这里而不配置错误组可能仍返回硬错误。测试应加入独立文件 `pkg/expression/errors_35_aster_unit_test.rs`，不得内嵌到生产源文件。
3. 修改除零或包溢出策略时，覆盖 Error/Warn/Ignore 及 `TruncateAsWarning`/`IgnoreTruncateErr` 的组合，保留 warning 数量和格式化参数断言；还应核对 Go 的算术、字符串和 cast 调用点。
4. 若把更多 Rust builtin 接入本模块，使用 `crate::expression_errors_kernel::<symbol>` 或设计明确的生产再导出，并验证实际调用边。不要误用 `core_support.rs` 中同名但类型不同的错误常量。
5. 错误目录变更的主要兼容风险是 MySQL 错误码、文本参数和错误组路由；性能风险很低，主要是首次 `LazyLock` 初始化、错误克隆和 warning 追加。引入带会话参数的缓存或全局可变状态前必须重新评估隔离与并发。

## 验证依据

- 源码与模块：通过 RustCodeGraph 完整读取 `pkg/expression/errors.rs:1-157`；源码复核 `pkg/expression/lib.rs:324-325, 659-662, 850-853`、`pkg/expression/builtin.rs:4234-4241`、`pkg/expression/extension.rs:209` 和同名边界 `pkg/expression/core_support.rs:153`。
- crate 声明：读取 `pkg/expression/Cargo.toml`，确认 crate 名、`lib.rs` 入口、`autotests = false`、相关本地依赖及 `go-package = "pkg/expression"` 迁移元数据。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/expression/errors.rs --offset 1 --limit 260` 返回完整文件和 5 个使用文件；`query` 同时定位到 Go/Rust 同名处理器。`callers`/`callees` 因同名消歧未给出结果，故使用精确限定路径搜索补齐直接边。
- Go 对照：完整读取 `pkg/expression/errors.go`；精确搜索确认 `builtin_time.go`、`builtin_cast.go`、`builtin_arithmetic.go`、`builtin_string.go` 及向量化文件的调用范围。
- Rust 测试：完整读取 `pkg/expression/errors_35_aster_unit_test.rs`，确认三个处理器的分支、不变量、消息参数与错误身份；`lib.rs` 显式装配该独立测试。RustCodeGraph 还列出三个使用错误模块的相邻测试文件，但它们不是三个处理器的主行为测试。
- 本任务只新增本文档，未运行 Cargo。交付检查包括固定 11 个二级标题的结构命令、文档 diff 人工复核和仓库状态复核；没有把未执行的代码测试作为完成证据。

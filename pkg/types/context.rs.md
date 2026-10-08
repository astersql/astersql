# `pkg/types/context.rs`

## 文件定位

`pkg/types/context.rs` 定义标量类型转换所需的轻量上下文：转换策略位、时区、warning 接收器，以及“发生截断但仍有可用结果值”时的返回协议。它不是进程取消用的 `std::task::Context` 或 Go `context.Context`，而是 Go `pkg/types.Context` 的 Rust 对应物。

该文件的实际 crate 归属不是直接由 `pkg/types/Cargo.toml` 编译：`pkg/types/internal/scalar/lib.rs` 通过 `#[path = "../../context.rs"] mod context` 把它挂到 `astersql-types-scalar`，随后公开重导出全部符号。顶层 `astersql-types` 再通过 `types-group-1` 依赖和 `pkg/types/lib.rs` 重导出 `Context`、`Flags`、`NewContext` 与默认上下文。因此调用方既可能写 `types_crate::scalar::Context`，也可能从 `astersql_types` 或表达式层的再导出入口取得这些类型。

`pkg/types/internal/scalar/Cargo.toml` 表明本文件的直接外部边界是 `chrono-tz`、`astersql-util-context` 与 `astersql-types-time`；顶层 `pkg/types/Cargo.toml` 则说明该标量 crate 是整个 `astersql-types` 聚合 crate 的组成部分，并以 `pkg/types` 作为 Go 移植包。

## 核心职责

1. `Flags` 以 `u16` 位掩码集中表达转换策略，涵盖截断、负数转无符号、日期容错、字符集检查和 TIME 转 YEAR 的兼容路径。
2. `Context` 把 `Flags`、`chrono_tz::Tz` 和线程安全的 `WarnAppender` 绑定为一次转换所需的环境，并通过克隆式 `WithFlags`/`WithLocation` 避免就地改写原上下文。
3. `HandleTruncate<T>` 实现三分支优先级：忽略截断、记录 warning 后接受裁剪值、或返回同时携带裁剪值与错误的 `ErrorWithValue<T>`。
4. `types_time::TimeContext for Context` 将通用转换标志投影为时间子系统只关心的四个布尔策略，并把 `TimeError` 转成共享错误后交给同一 warning 接收器。
5. `DefaultStmtNoWarningContext` 与 `StrictContext` 提供延迟初始化、UTC、丢弃 warning 的全局只读基准上下文。

这些职责共同使类型解析、转换和比较代码不必依赖完整会话对象。典型接线见 `pkg/expression/exprstatic/evalctx.rs`（按求值状态构造类型上下文）、`pkg/sessionctx/stmtctx/stmtctx.rs`（在语句上下文中保存并委托截断处理）以及 `pkg/types/convert.rs`、`pkg/types/internal/datum/lib.rs`、`pkg/types/binary_literal.rs`（消费策略并处理部分结果）。

## 主要符号

- `StrictFlags: Flags`：数值为零，所有容错开关关闭。它是最严格策略的位掩码基线，不等同于时间模块中同名的 `types_time::StrictContext`。
- `Flags(pub u16)`：公开底层值的可复制位掩码。实现 `BitOr`、`BitAnd`、`Not`，允许组合、筛选与按位取反；内部 `contains` 和 `with` 分别完成读取及不可变开关更新。
- 十个 `Flag*` 常量：依次占用第 0 到第 9 位。`FlagIgnoreTruncateErr`、`FlagTruncateAsWarning` 控制截断；`FlagAllowNegativeToUnsigned` 控制负数到无符号数；三个日期标志控制零日期、零分量和非法日期；三个字符集标志跳过 ASCII/UTF-8/UTF8MB4 检查；`FlagCastTimeToYearThroughConcat` 选择兼容性的 TIME→YEAR 路径。
- `Flags` 的公开读写方法：每个策略由 `Xxx() -> bool` 与 `WithXxx(bool) -> Flags` 配对。`WithSkipSACIICheck` 故意保留 Go API 的历史拼写 `SACII`，不是新的独立标志。
- `ErrorWithValue<T> { value, error }`：错误结果仍保留已经裁剪、饱和或回退后的 `T`。其 `Display` 代理到底层 `SharedError`，当 `T: Debug` 时实现标准 `Error`。
- `ValueResult<T>`：`Result<T, ErrorWithValue<T>>` 的别名，是本文件截断处理的统一返回形状。
- `Context { flags, loc, warnHandler }`：字段保持私有，调用者只能通过构造函数、读取器和克隆式更新方法维护不变量。`warnHandler` 是 `Arc<dyn WarnAppender + Send + Sync>`。
- `NewContext`：以完整的标志、非可空 `Tz` 和非可空 trait object 构造上下文；Rust 类型系统消除了 Go 构造器对 nil 的运行时断言分支。
- `Context::{Flags, WithFlags, WithLocation, Location, AppendWarning}`：分别读取策略、派生新上下文、读取时区和转发 warning。克隆上下文时 warning 接收器只增加 `Arc` 引用计数。
- `Context::HandleTruncate<T>`：本文件的核心决策函数；错误输入是必需的 `SharedError`，成功与否仍保留调用方给出的 `value`。
- `impl types_time::TimeContext for Context`：让 `Context` 可直接传入 `ParseTime`、`ParseDatetime`、`ParseDuration` 等泛型时间 API。
- `IgnoreWarnings`：私有空接收器，同时丢弃 warning 与 note，仅供两个默认全局上下文使用。
- `DefaultStmtFlags`：`StrictFlags | FlagAllowNegativeToUnsigned | FlagIgnoreZeroDateErr`。
- `DefaultStmtNoWarningContext`、`StrictContext`：`LazyLock<Context>`；第一次解引用时构造，时区均为 UTC，warning 均被丢弃。

## 执行流程

构造和派生流程如下：调用方先用 `NewContext(flags, loc, handler)` 固化三项环境；若局部操作需要不同策略或时区，则调用 `WithFlags` 或 `WithLocation`。这两个方法克隆原值后只替换一个字段，所以原上下文、另一个配置字段以及共享 warning 接收器均保持不变。`pkg/types/context_test.rs::test_with_new_flags` 验证了原实例与派生实例的标志隔离及 UTC 保持。

标志读写统一经过两个私有原语。`contains(flag)` 判断交集是否非零；`with(flag, true)` 以 OR 置位，`with(flag, false)` 以 AND-NOT 清位。`test_simple_on_off_flags` 对四个常用标志验证从零、单个位和全位置位状态下的对称性；其余公开开关沿用同一实现路径。

截断处理按固定顺序执行：

1. `IgnoreTruncateErr` 为真时直接返回 `Ok(value)`；即使 `TruncateAsWarning` 同时为真，也不会追加 warning。
2. 否则，`TruncateAsWarning` 为真时调用 `AppendWarning(err)`，然后返回 `Ok(value)`。
3. 两者均为假时构造 `ErrorWithValue::new(value, err)` 并返回 `Err`，让上层既能传播错误也能读取部分结果。

真实下游包括 `pkg/types/convert.rs` 的数值/JSON 转换、`pkg/types/internal/datum/lib.rs` 的 JSON 到标量转换、`pkg/types/binary_literal.rs` 的二进制字面量转换，以及 `pkg/sessionctx/stmtctx/stmtctx.rs::HandleTruncate` 的语句层委托。源文件注释明确约定这些调用者传入的是截断类错误；此方法本身不做 errno 分类。

时间调用流程是另一条适配路径。`types_time` 通过 `TimeContext::flags()` 取得由四个 `Flags` 位映射出的 `TimeFlags`，通过 `location()` 取得 `Tz`；需要报告 warning 时，`append_warning(TimeError)` 将时间错误包装为 `contextutil::errors::SharedError`，再复用 `Context::AppendWarning`。因此日期容错和时区由同一上下文控制，但时间子系统看不到其他转换位。

## 数据与状态

`Flags` 是 16 位值，目前只定义 10 位；未定义的高位不会被 `contains` 读取，但因为元组字段公开、`Not` 会翻转全部 16 位，调用方和测试可以保留这些位。新增标志必须选择未使用位，并确认跨 Go/Rust 边界的数值布局不变。

`Context` 的运行时状态只有三个字段：复制成本很低的 `Flags`、可复制的 `Tz`，以及共享的 `Arc` warning 接收器。`Clone` 是浅克隆：标志和时区按值复制，接收器共享。因此多个派生上下文可能把 warning 写入同一底层存储；隔离配置不等于隔离 warning 队列。

`ErrorWithValue<T>` 同时拥有值和共享错误。它没有额外的错误分类或恢复标记，调用方必须依据 `Result` 分支判断是否接受 `value`。`SharedError` 使错误可跨所有权边界共享，`T` 本身不要求 `Clone`。

两个全局上下文由 `LazyLock` 管理。`DefaultStmtNoWarningContext` 的默认行为允许负数按兼容规则转无符号并忽略零日期错误；`StrictContext` 不打开任何位。两者都使用 `IgnoreWarnings`，因此即使下游调用 warning 路径也不会保存记录。需要观察 warning 的代码必须显式构造带真实 `WarnAppender` 的上下文，而不能依赖这两个全局值。

## 依赖与调用关系

直接依赖如下：

- 标准库 `fmt` 提供错误展示，`BitAnd`/`BitOr`/`Not` 提供位运算，`Arc`/`LazyLock` 提供共享接收器与全局延迟初始化。
- `chrono_tz::Tz` 表示命名时区；默认值使用 `chrono_tz::UTC`。
- `contextutil::WarnAppender` 与 `contextutil::errors::SharedError` 定义 warning 输出端和可共享错误。它们由 `astersql-util-context` 提供。
- `types_time::{TimeContext, TimeFlags, TimeError}` 是时间子系统的窄接口；`Context` 通过 trait 实现而不是让时间代码反向依赖整个标量 crate。

模块装配链为 `pkg/types/internal/scalar/lib.rs` → `pkg/types/context.rs` → `pub use context::*`，再由 `pkg/types/lib.rs` 将核心符号重导出。`pkg/types/internal/scalar/Cargo.toml` 声明 `chrono-tz`、`contextutil`、`types-time`；`pkg/types/Cargo.toml` 将该 crate 作为 `types-group-1` 依赖并聚合进 `astersql-types`。

RustCodeGraph 对目标文件报告 44 个符号、61 个文件使用者。可复核的代表性上游/下游边包括：

- `pkg/expression/exprstatic/evalctx.rs` 调用 `types::NewContext`，把表达式 warning 处理器、时区和求值标志接入类型转换。
- `pkg/sessionctx/stmtctx/stmtctx.rs` 重导出 `Context as TypeContext`，构造语句级类型上下文，并把自身的截断处理委托给 `self.typeCtx.HandleTruncate(value, err)`。
- `pkg/session/runtime/planning.rs` 克隆 `DefaultStmtNoWarningContext` 作为规划/类型操作的默认环境。
- `pkg/planner/util/handle_cols.rs`、`pkg/lightning/backend/kv/canonical.rs` 使用默认无 warning 上下文完成无需会话 warning 状态的类型操作。
- `pkg/ddl/persistent_masking_actions.rs` 以 `StrictContext` 做严格日期解析并读取其时区。
- `pkg/types/convert.rs`、`pkg/types/internal/datum/lib.rs`、`pkg/types/binary_literal.rs` 调用泛型 `HandleTruncate`，在转换失败时保留候选值。

## 错误处理与边界

`HandleTruncate<T>` 的优先级是不变量：忽略错误高于转 warning。若两个位同时开启，返回值被接受且不记录 warning。严格模式则返回 `ErrorWithValue<T>`，错误展示完全沿用内部 `SharedError`，不会添加新的上下文文本。

该 Rust 方法与 Go `pkg/types/truncate.go::Context.HandleTruncate` 的输入边界不同。Go 方法接受可能为 nil 的普通 `error`，先提取根因，并只对十个指定 MySQL errno 应用忽略/warning 策略；非截断错误原样返回。本文件的方法接受必需的 `SharedError`，不剥离根因也不检查 errno，并通过紧邻注释把“调用者只传截断类错误”作为前置条件。若未来有未分类错误进入这里，它也会被忽略或降为 warning；扩展调用点时必须先证明错误属于截断类，或在更外层完成分类。

Rust 返回类型还刻意区别于 Go：Go 只返回 `error`，结果值由调用方另行持有；Rust 的 `ValueResult<T>` 在错误分支中保留 `value`。这让 `pkg/types/convert.rs` 等调用方可以选择传播错误或提取裁剪值，但也意味着普通 `Result<T, SharedError>` 不能无损替代它。

`NewContext` 不接收空时区或空接收器：`Tz` 是值类型，warning handler 是有效的 `Arc<dyn ...>`。相较 Go 版本的 `intest.Assert` 和 nil 防御，这些不变量由类型签名保证。`AppendWarning` 不捕获接收器的 panic，也不提供容量限制；失败策略完全由具体 `WarnAppender` 实现决定。

当前独立 Rust 测试只直接覆盖 `WithFlags`、四组常用标志和测试 warning 存储器，没有直接覆盖 `WithLocation`、六个其他标志、`HandleTruncate` 三分支、`TimeContext` 映射、默认全局上下文或 `ErrorWithValue` 展示。相关下游测试提供部分间接证据，但新增或修改这些行为时不应把现有测试覆盖面当作完整保证。

## 并发与资源生命周期

`Context` 自身没有锁、任务、通道、事务或析构逻辑。它的共享边界集中在 `Arc<dyn WarnAppender + Send + Sync>`：类型约束允许上下文跨线程共享接收器，但具体并发安全由 `WarnAppender` 实现内部保证。测试辅助 `WarnStore` 使用 `Mutex<Vec<SharedError>>` 展示了可并发写入的典型实现；多个克隆上下文会竞争同一个底层锁并共享累计 warning。

`WithFlags` 与 `WithLocation` 不修改原对象，因此并发读取同一基准上下文时不存在配置写竞争。`AppendWarning(&self)` 允许只借用上下文就产生接收器内部副作用，这是本类型唯一的内部可变状态路径。

`LazyLock` 保证两个全局上下文只初始化一次，并安全发布给并发读取者。它们持有的 `IgnoreWarnings` 无状态、无锁且不分配每条 warning；生命周期与进程相同。自定义上下文则在最后一个 `Context`/接收器 `Arc` 克隆释放后销毁底层 handler。

## 与 Go 版本的对应关系

主要一一对应关系位于 `pkg/types/context.go`：`Flags uint16` 对应 `Flags(pub u16)`；十个常量保持相同位序；所有查询和 `With*` 方法保持名称与布尔语义；`WithSkipSACIICheck` 同样保留历史拼写；`Context` 都保存 flags、location 和 warning handler；默认 flags 及两个全局上下文也保持一致。

Rust 用值语义强化了 Go 的运行时约束。Go `NewContext` 和 `WithLocation` 对 nil 做测试期断言，`Location`/`AppendWarning` 还包含 nil 回退；Rust 的 `Tz` 和 `Arc<dyn WarnAppender + Send + Sync>` 无法表示这些 nil 状态，所以没有对应防御分支。Go 的 `*time.Location` 对应可复制的 `chrono_tz::Tz`，支持的时区语义应以 `chrono-tz` 的数据库为准。

Go 的 warning handler 是接口值，Rust 明确要求 `Send + Sync` 并由 `Arc` 共享。Go 的全局变量在包初始化时构造；Rust 使用 `LazyLock` 首次访问才构造。对调用者而言，两者都应视作共享基准值，局部改配置时使用 `With*` 派生，而不是试图修改全局实例。

截断逻辑在 Go 中位于独立的 `pkg/types/truncate.go`，而目标 Rust 文件自身包含面向“值 + 错误”的通用 `HandleTruncate<T>`。仓库另有 `pkg/types/truncate.rs`，它更贴近 Go 的 errno 过滤和可选错误接口，但该文件不由 `pkg/types/internal/scalar/lib.rs` 当前模块装配链挂载；不能把它的分类行为当成此 `Context::HandleTruncate<T>` 已具备的能力。

测试对应关系清楚：`pkg/types/context_test.rs::test_with_new_flags` 对齐 Go `TestWithNewFlags`，`test_simple_on_off_flags` 对齐 `TestSimpleOnOffFlags`，`WarnStore`/`warn_store_matches_go_helper` 对齐 Go 的 `warnStore` 辅助行为。Rust 测试额外用 `catch_unwind` 明确验证未实现的 `AppendNote` 会 panic。

## 扩展指南

新增转换策略位时，应同时修改 `pkg/types/context.rs` 的常量、读取器和 `With*` 方法，并核对 `pkg/types/context.go` 的位序及语义。若该位影响时间逻辑，还要更新 `types_time::TimeFlags` 与 `TimeContext::flags` 映射；若影响 protobuf/跨语言标志，则继续追踪会话或表达式层的编码边界。测试应扩展独立的 `pkg/types/context_test.rs`，至少覆盖从零开启、从全位关闭、与已有位组合不互相污染。

新增上下文字段时，应从 `NewContext`、`Clone` 派生方法、默认全局构造、顶层再导出和所有显式构造点一起检查。字段若含可变共享状态，必须维持 `Context` 所需的线程安全边界，并写清克隆后是共享还是独立；不要把测试逻辑内嵌回生产源文件。

修改截断策略时，应优先从 `Context::HandleTruncate<T>` 及其直接调用者入手，并补充三种优先级组合的独立回归测试：仅忽略、仅 warning、两者同时开启，以及严格返回 `ErrorWithValue`。还应覆盖 warning 的内容和次数。若要对齐 Go 的 errno 分类，应明确选择在调用者、此方法或独立分类层完成，并验证非截断错误绝不会被容错位吞掉；不能仅复制 `pkg/types/truncate.rs` 而不处理当前模块装配和返回值协议。

修改时区或时间 warning 行为时，应直接测试 `impl types_time::TimeContext for Context`：四个日期/转换位的映射、非 UTC `Tz` 传播、`TimeError` 包装后是否到达同一 handler。相关集成消费面包括 `pkg/types/time.rs`、表达式静态求值上下文和 DDL 日期解析路径。

性能方面，保持 `Flags` 和 `Tz` 的值复制以及 `Arc` 浅克隆很重要；不要在每次标志读取或默认上下文访问时分配。兼容性方面，公开方法采用 Go 风格命名并由 crate 级 `allow(non_snake_case)` 接受，重命名会破坏大量迁移代码和 Go/Rust 对照，不应仅为 Rust 命名习惯调整。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/types` 确认目标源、Go 对照及独立测试均已索引。
- RustCodeGraph 文件节点：`node --file pkg/types/context.rs --offset 1 --limit 500` 读取完整 327 行，报告 44 个符号及 61 个使用文件；据此核对全部常量、类型、方法、trait 实现和静态值。
- RustCodeGraph 符号/调用查询：查询了 `Context`、`Flags`、`NewContext`、`HandleTruncate`、两个默认上下文，并执行 `callers/callees`。宽查询确认 `IgnoreTruncateErr`、`TruncateAsWarning`、字符集开关等真实使用边；精确带路径的 `callers/callees` 未返回独立输出，因此调用关系又以仓库精确文本搜索及调用方源码位置交叉核验，没有据此臆造完整调用图。
- 模块与 Cargo：读取 `pkg/types/internal/scalar/lib.rs`、`pkg/types/internal/scalar/Cargo.toml`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`，确认实际 crate 挂载、依赖、feature 和顶层重导出链。
- Go 对照：读取 `pkg/types/context.go` 与 `pkg/types/truncate.go`，核对位序、构造/派生语义、nil 边界、默认值和截断错误分类；同时读取未挂载的 `pkg/types/truncate.rs`，避免把它的行为误归于目标文件。
- 独立测试：读取 `pkg/types/context_test.rs` 与 `pkg/types/context_test.go`，核对不可变更新、四个标志的开关行为及 warning 存储辅助；通过仓库搜索定位 `pkg/types/convert_test.rs`、`pkg/types/time_test.rs` 及其他间接消费测试。未运行 Cargo，符合本纯文档任务约束。
- 代表性直接调用证据：`pkg/expression/exprstatic/evalctx.rs`、`pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/types/convert.rs`、`pkg/types/internal/datum/lib.rs`、`pkg/types/binary_literal.rs`、`pkg/session/runtime/planning.rs`、`pkg/ddl/persistent_masking_actions.rs`。
- 完成前按任务指定命令验证文档恰含 11 个固定二级标题，并人工复核所有重要结论均指向真实符号或路径，无整段源码复制、无把未挂载模块描述为已接线的结论。

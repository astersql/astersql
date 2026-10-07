# `pkg/errors/stack.rs`

## 文件定位

[`pkg/errors/stack.rs`](stack.rs) 是 `astersql-errors` crate 的调用栈基础层。crate 入口 `pkg/errors/mod.rs` 将本文件声明为私有模块，并重新导出 `Frame`、`StackTrace`、`Stack`、`StackTracer`、`StackTraceCarrier`、`GetStackTracer` 与 `NewStack`，因此调用方通过 `astersql_errors::*` 使用这些 API，而不直接访问 `stack` 模块。`pkg/errors/Cargo.toml` 指定 crate 根为 `mod.rs`，本文件唯一直接的第三方运行时依赖是 `backtrace = "0.3"`。

它处在错误构造/包装与最终诊断展示之间：`pkg/errors/core.rs` 的 `New`、`Errorf` 和 `pkg/errors/wrap.rs` 的 `WithStack`、`AddStack`、`Wrap` 等路径调用 `NewStack`；之后 `GetStackTracer` 从错误 cause 链中找出可见栈，格式化实现再将帧输出给日志、Debug 展示或调用方。它不是 SQL 请求主链的业务错误定义文件，而是跨子系统复用的诊断基础设施。

## 核心职责

本文件承担四项职责：

1. 用 `backtrace::Backtrace` 捕获当前线程的真实调用栈，并把物理帧和符号展开为拥有所有权的 `Frame` 列表（`NewStack`、`resolve_backtrace`）。
2. 保存并查询每帧的指令指针、源文件、行号和函数名，在符号缺失时提供稳定占位值（`Frame`、`UNKNOWN`）。
3. 提供紧凑与扩展两种展示协议：普通展示为 `basename:line` 列表，交替 Debug 展示为逐帧 `function\n\tpath:line`（`Frame::{compact,extended}`、`StackTrace` 的 `Display`/`Debug`）。
4. 定义错误链暴露与查找栈的抽象，使 `core.rs` 和 `wrap.rs` 中不同错误实现能被统一查询（`StackTracer`、`StackTraceCarrier`、`GetStackTracer`）。

本文件不负责错误消息、错误码、多原因错误组或 cause 的具体存储；这些分别由 `core.rs`、`normalize.rs`、`group.rs`/`join.rs` 和 `wrap.rs` 负责。

## 主要符号

- `UNKNOWN: &str`：符号解析拿不到文件名时使用的稳定文本 `"unknown"`。
- `Frame`：公开的已解析帧，内部保存 `instruction_pointer: usize`、`file: String`、`line: u32`、`function: String`。字段私有，调用方通过同名 getter 只读访问。
- `Frame::from_instruction_pointer(usize) -> Frame`：公开的单地址解析入口。地址为 0 时直接返回 `unknown:0`；非零地址调用 `backtrace::resolve`，且只接受第一个命中的符号。
- `Frame::compact()` / `extended()`：分别产生文件名加行号，以及函数名、完整路径和行号。函数名为空时，`extended` 退化为 `path:line`。
- `StackTrace(Vec<Frame>)`：公开的拥有型帧序列，顺序不变量是由内（最新）到外（最旧）。它实现 `From<Vec<Frame>>`、`Deref<Target=[Frame]>`、下标访问和格式化，但不公开可变访问。
- `StackTracer`：公开 trait，核心方法为 `stack_trace`（返回克隆的拥有型栈）和 `empty`（避免仅为判断空栈而克隆）；`StackTrace`、`Empty` 是迁移 Go 调用时使用的大写别名。
- `StackTraceCarrier`：公开的 cause 链节点抽象。`stack_tracer` 暴露本层 tracer，`cause` 默认无下一层。
- `GetStackTracer(&dyn StackTraceCarrier)`：从当前节点开始逐层检查，返回第一个实现暴露出的 tracer；当前层没有 tracer 时才沿 `cause()` 继续。
- `Stack`：`NewStack` 返回的拥有型容器，内部仅有 `trace: StackTrace`；实现 `StackTracer` 和 `StackTraceCarrier`。
- `NewStack(skip: usize) -> Stack`：公开捕获入口；`#[inline(never)]` 用来提高定位自身帧并实施 skip 的稳定性。
- `resolve_backtrace`、`is_new_stack`、`strip_symbol_hash`：私有辅助函数，依次负责展开 backtrace、识别 `NewStack` 自身/闭包帧，以及移除 Rust 符号末尾 `::h` 加 16 位十六进制哈希。

## 执行流程

调用 `NewStack(skip)` 时，执行流程如下：

1. `Backtrace::new()` 捕获当前线程的物理栈。
2. `resolve_backtrace` 按 backtrace 原始顺序遍历物理帧。无 symbol 的物理帧生成一个 `Frame::unknown(ip)`；有 symbol 时逐个展开，因此同一个指令指针可能对应多个内联符号帧。
3. 每个已解析符号立即复制文件路径、行号和函数名；函数名经 `strip_symbol_hash` 去掉编译器哈希后缀。解析发生在捕获期，不延迟到格式化期。
4. `NewStack` 用 `is_new_stack` 查找自己的第一帧。找到时从它的下一帧开始，找不到时从索引 0 开始；随后再跳过调用者要求的 `skip` 帧。
5. 剩余帧构造成 `StackTrace` 并存入 `Stack`。例如 `NewStack(0)` 的首帧应是直接调用者；包装函数传入 1 时还会隐藏包装函数自身。

查询时，`GetStackTracer` 先调用当前 carrier 的 `stack_tracer()`；有值便立即返回，即使该 tracer 是空栈。没有值时通过 `cause()` 前进，链尾返回 `None`。是否把空 tracer 当作“有有效栈”由上层决定：`pkg/errors/wrap.rs::HasStack` 还会检查 `!tracer.empty()`。

格式化时，单个 `Frame` 的 `Display` 和非交替 `Debug` 都走 `compact`；`StackTrace::Display` 输出方括号和空格分隔帧，空栈为 `[]`；非交替 `Debug` 与 `Display` 相同；交替 `Debug` 为每帧先写一个换行，再写扩展格式，空栈输出空字符串。

## 数据与状态

`Frame` 和 `StackTrace` 都实现 `Clone + Eq + PartialEq`，比较的是捕获后保存的全部字段，不会重新解析符号。`StackTrace` 的向量保持捕获顺序；公开 API 能读取长度、判断为空、切片解引用和下标访问，但不能就地改写帧，从而保护顺序和帧内容。

`StackTracer::stack_trace` 返回拥有型 `StackTrace`。`Stack`、`core.rs::Fundamental` 和 `wrap.rs::WithStackError` 的实现都会克隆内部栈；只判断存在性时应调用 `empty`，避免复制所有路径和函数名字符串。`Stack::default()` 表示显式空栈，并被 `wrap.rs::with_empty_stack`/`suspend_stack` 用作“当前无有效栈”的标记。

本文件没有全局可变状态、缓存或配置开关。每次 `NewStack` 都独立捕获并拥有数据；其主要成本来自 backtrace 捕获、符号展开、字符串分配，以及之后请求完整 `stack_trace` 时的克隆。`benches/errors.rs::benchmark_stack_formatting` 只测已经捕获好的错误与 trace 的格式化成本，没有覆盖捕获成本。

## 依赖与调用关系

上游关系由模块引用和精确搜索确认：

- `pkg/errors/core.rs::{New,Errorf}` 调用 `NewStack(1)`，并由 `Fundamental: StackTracer` 暴露栈。
- `pkg/errors/wrap.rs::{WithStack,AddStack,with_stack,Wrap,Wrapf}` 最终构造带 `Stack` 的包装；`SharedError: StackTraceCarrier` 把具体包装、本源错误和下一层 cause 接到 `GetStackTracer`。
- `pkg/errors/wrap.rs::HasStack` 调用 `GetStackTracer` 并额外判空，以支持已有栈去重；`WithStack` 则按语义总是新增一层栈。
- `pkg/errors/mod.rs` 重新导出全部公开符号；`pkg/errors/tests/*.rs` 和 `benches/errors.rs` 通过 crate 公共 API 使用它们。

下游依赖为标准库的 `fmt`、`Path`、`Deref`、`Index`、`c_void`，以及 `backtrace::{Backtrace,resolve}`。`Path::file_name` 只用于紧凑格式；路径不是有效 UTF-8 时，捕获阶段的 `to_string_lossy` 会进行有损转换。`backtrace::resolve` 的回调可能产生多个结果，但 `Frame::from_instruction_pointer` 明确只取第一个；整栈解析则保留每个物理帧的所有 symbol。

RustCodeGraph 已索引 `pkg/errors/stack.rs` 的 37 个符号，并精确定位 `NewStack`、`GetStackTracer`、`resolve_backtrace`、`StackTrace`；本次索引的 `explore/callers/callees` 未返回边，因此跨文件调用关系由上述 Rust 精确引用搜索补足，不把缺失图边解释成“无调用者”。

## 错误处理与边界

这里的“失败”不会返回 `Result`：缺少文件、行号或符号名会被降级为 `unknown`、`0` 或空函数名，保证栈仍可展示。指令指针为 0 是明确的无效输入，并稳定格式化为 `unknown:0`。文件名提取失败时 `compact` 回退到保存的完整 `file` 字符串。

`GetStackTracer` 返回遇到的第一个 tracer，而不是第一个非空 tracer；上层若关心有效栈必须检查 `empty`。它只遍历 `StackTraceCarrier::cause` 表示的单因链，不遍历 `ErrorGroup` 子项；与 Go 版通过 `WalkDeep` 同时遍历深层结构的能力存在边界差异。trait 没有循环检测，错误实现若让 `cause()` 构成环会无限循环；现有 `SharedError` 实现由具体包装逐层指向内层，不会主动造环。

`StackTrace` 的 `Index` 遵循 Rust 切片语义，越界会 panic。极端大的 `skip` 在 `first_caller + skip` 处还可能发生整数溢出（debug 构建 panic，release 构建按 Rust 配置处理）；正常 API 用法应传入很小的包装层数。符号信息受编译选项、平台和二进制调试信息影响，因此调用方不应把完整函数名或绝对路径视为跨构建稳定协议。

## 并发与资源生命周期

捕获对象没有锁、通道、异步任务或外部句柄。`Backtrace` 只在 `NewStack` 调用期间作为局部值存在；`resolve_backtrace` 完成后，资源被转换为拥有型字符串和整数，原始 backtrace 随即释放。返回的 `Stack` 不借用解析器、线程栈或动态库符号对象，因此可以独立于捕获现场存活。

捕获的是调用 `NewStack` 的当前线程，而不是进程内其他线程。多个线程同时捕获时，各自构造独立值；本文件没有共享状态竞争。是否可在线程间传递由所有字段和依赖类型的自动 trait 推导决定，公共 API 没有额外的线程亲和生命周期。

`GetStackTracer` 返回值的生命周期绑定到传入 carrier；它不取得错误链所有权，也不克隆 tracer。相反，随后调用 `stack_trace()` 会返回克隆的独立快照。扩展时应维持这种借用查询、拥有快照的分工。

## 与 Go 版本的对应关系

仓库 `go.mod` 锁定 `github.com/pingcap/errors v0.11.5-0.20260508054701-306e305bcf41`；该版本模块缓存中的 `stack.go` 和 `stack_test.go` 是直接对照证据。主要对应如下：

- Go `Frame uintptr` 延迟用 `runtime.FuncForPC` 解码；Rust `Frame` 同时保留指令指针，并在捕获或显式构造时急切保存结构化符号数据。
- Go `StackTrace []Frame` 与 Rust `StackTrace(Vec<Frame>)` 都规定 newest-to-oldest 顺序；Rust 通过私有字段限制外部改写。
- Go `%v`/`%+v` 映射为 Rust 的普通展示/交替 Debug。Rust 没有完整复刻 Go 的 `%s`、`%d`、`%n` 和 `%#v` formatter verb 组合，而是提供 getter、`compact`、`extended` 与标准 trait。
- Go `callersSkip` 固定最多捕获 32 个 PC；Rust `Backtrace::new()` 后没有在本文件设置 32 帧上限，并会展开内联 symbols。
- Go `NewStack(skip)` 返回 `StackTracer` 接口；Rust 返回具体 `Stack`，它实现 `StackTracer` 和 `StackTraceCarrier`。两者都保证 `skip=0` 以调用点为首帧。
- Go `GetStackTracer` 依靠 `WalkDeep(error, ...)` 做运行时接口断言；Rust 以显式 `StackTraceCarrier` trait 取代断言。目前 Rust 只跟随 `cause`，没有 Go `WalkDeep` 的 error-group 分支。
- Go 函数名格式化会移除包路径并保留包内名称；Rust 仅移除编译器哈希，扩展格式通常保留 Rust 完整模块路径。这是可见输出差异，不应在兼容测试中假设逐字相同。

独立 Rust 测试 `pkg/errors/tests/stack_test.rs` 对应 Go `stack_test.go` 的主要意图：有效/无效帧、skip、空栈、两种格式和 cause 链查找。`api_parity_test.rs` 触及公开类型和 Go 风格别名；`wrap_test.rs`、`example_test.rs`、`format_test.rs`、`adaptor_test.rs` 与 `normalize_generation_test.rs` 从错误包装和生成路径验证栈的集成行为。

## 扩展指南

新增捕获或过滤规则时，优先修改 `NewStack`、`resolve_backtrace`、`is_new_stack`，并在独立的 `pkg/errors/tests/stack_test.rs` 中增加回归测试；不要把测试内嵌进生产源文件。改动 skip 语义必须同时检查 `core.rs` 和 `wrap.rs` 中所有 `NewStack(1)`/`NewStack(skip)` 调用，否则首帧会泄漏内部构造函数或错误地跳过业务调用点。

新增格式能力时，应分别评估 `Frame` 与 `StackTrace` 的普通、非交替和交替格式，并保持空栈 `[]`/空扩展输出及无符号 `unknown:0` 的兼容性。路径清洗或函数名规范化会改变日志与测试可见文本；需同步 `stack_test.rs`、`format_test.rs` 和 `benches/errors.rs`，并注意不同平台的路径与符号差异。

若要补齐 Go `WalkDeep` 的多原因链行为，不应仅在本文件中硬编码 `ErrorGroup` 具体类型；应先设计能表达 cause 与 group children 的统一借用接口，再同步 `group.rs`/`join.rs` 和独立测试。必须防止环形图导致无限遍历，并明确“第一条栈”的确定顺序。

若要降低开销，可研究延迟符号化、帧数上限或共享不可变栈，但必须先测量捕获和克隆两类成本。当前 benchmark 只覆盖格式化，新增优化需要单独基准，且不能以丢失 Go 对照要求的帧、skip 或展示语义换取通过测试。

## 验证依据

- 源码与 crate 边界：`pkg/errors/stack.rs`、`pkg/errors/mod.rs`、`pkg/errors/Cargo.toml`。
- 直接 Rust 接线：`pkg/errors/core.rs`、`pkg/errors/wrap.rs`；外部格式化基准：`benches/errors.rs::benchmark_stack_formatting`。
- 独立 Rust 测试：`pkg/errors/tests/stack_test.rs`（主要行为）、`api_parity_test.rs`（API 面）、`wrap_test.rs`、`example_test.rs`、`format_test.rs`、`adaptor_test.rs`、`normalize_generation_test.rs`（集成路径）。
- Go 版本锁定：`go.mod` 与 `go.sum` 中的 `github.com/pingcap/errors ...-306e305bcf41`；直接对照文件为该版本模块的 `stack.go`、`stack_test.go` 和 `errors.go`。
- RustCodeGraph：`status` 显示仓库索引包含 7,032 个 Rust 文件；`files --filter pkg/errors` 列出目标及独立测试；`query NewStack`、`query GetStackTracer`、`query resolve_backtrace`、`query StackTrace` 精确命中本文件。`explore/callers/callees` 本轮无边输出，调用关系另用 `rg` 对目标符号做精确核验。
- 结构验收使用任务指定命令，要求文件存在且固定二级标题恰好为 11 个。任务是纯文档分析，按计划不运行 Cargo 或代码测试；行为结论来自源码、调用点、Go 对照和既有独立测试的静态复核。

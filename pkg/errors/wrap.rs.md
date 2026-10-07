# `pkg/errors/wrap.rs`

## 文件定位

`pkg/errors/wrap.rs` 是 `astersql-errors` crate 的单因错误链包装层，对应仓库 `go.mod` 锁定的 `github.com/pingcap/errors` 中 `errors.go` 的 `withStack`、`withMessage` 及相关遍历 API。crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-errors`，库入口是 [`mod.rs`](mod.rs)，运行时依赖为 `backtrace` 和带 `derive` 的 `serde`；本文件没有 feature 或条件编译分支。

[`mod.rs`](mod.rs) 将 `AddStack`、`Cause`、`Find`、`GetErrStackMsg`、`HasStack`、`Unwrap`、`WithMessage`、`WithStack`、`Wrap`、`Wrapf` 从 crate 根公开。因此调用方通常依赖 `astersql_errors::*`，而不会直接访问私有 `wrap` 模块。文件内部还向 [`adaptor.rs`](adaptor.rs) 提供 `with_stack` 与 `suspend_stack`，支撑 `Annotate`、`Annotatef` 和 `SuspendStack`。

## 核心职责

本文件承担四组职责：

1. 用私有 `WithStackError` 和 `WithMessageError` 在 `SharedError` 外层分别附加调用栈或上下文消息，同时通过 `std::error::Error::source` 保留标准错误链。
2. 实现“总是加栈”的 `WithStack`/`Wrap`/`Wrapf` 与“仅缺栈时加栈”的 `AddStack`，并让 `WithMessageError::cause_has_stack` 缓存内层栈状态。
3. 通过 `immediate_cause`、`Unwrap`、`Cause`、`Find` 导航单因链；其中 `Find` 委托 `group::WalkDeep`，所以还能继续搜索 `ErrorGroup` 子树。
4. 提供诊断输出：`fmt_extended` 决定交替 `Debug` 的“根原因、逐层消息、调用栈”顺序，`GetErrStackMsg` 则只拼接各层自身消息而排除栈装饰。

它不负责基础错误的构造、格式参数解析、栈帧捕获实现、规范化错误的数据模型或多原因容器；这些分别位于 [`core.rs`](core.rs)、[`stack.rs`](stack.rs)、[`normalize.rs`](normalize.rs)、[`group.rs`](group.rs) 与 [`join.rs`](join.rs)。

## 主要符号

- `WithStackError { cause: SharedError, stack: Stack }`：私有栈包装器。`Display` 完全透传原因；普通 `Debug` 等同于 `Display`；交替 `Debug` 先调用 `fmt_extended(cause)`，再输出自身 `StackTrace`。它实现 `StdError::source`、`StackTracer`。
- `WithMessageError { cause, message, cause_has_stack }`：私有消息包装器。`Display` 生成 `message: cause`；交替 `Debug` 先展开原因再另起一行写本层消息；`cause_has_stack` 是构造时取得的快照。
- `immediate_cause`：只识别本 crate 的 `WithStackError`、`WithMessageError` 和 `normalize::error_cause`，是 `StackTraceCarrier::cause`、`Unwrap` 与 `Cause` 的单因链入口。
- `fmt_extended`：递归识别栈包装、消息包装和规范化/基础错误，避免用包装后的整串 `Display` 重复打印上下文。
- `impl StackTraceCarrier for SharedError`：把 `WithStackError` 或基础错误的 `StackTracer` 暴露给 `GetStackTracer`，并通过 `immediate_cause` 下钻。
- `HasStack(&SharedError) -> bool`：消息层直接返回缓存；其他节点调用 `GetStackTracer`，且只有非空栈才算已有栈。
- `WithStack` 与 `AddStack`：都接受 `Option<SharedError>`；前者总是创建新栈层，后者在链中已有非空栈时原样返回同一个共享错误。
- `with_stack`、`with_empty_stack`、`suspend_stack`、`clear_stack`：crate 内部的精确 skip、空栈标记和递归清栈工具。`clear_stack` 会重建经过的消息层，但保留消息与缓存值。
- `Wrap`、`Wrapf`：先构造消息层，再无条件构造栈层；`Wrapf` 通过 `core::format_message` 解释 Go 风格格式串及 `ErrorArg`。
- `WithMessage`：只加消息，不捕获新栈。
- `Unwrap`、`Cause`：分别剥一层与走到最深层；返回克隆的 `SharedError`，共享底层错误对象。
- `Find`：从外到内把谓词交给 `WalkDeep`，在首个匹配处提前停止。
- `GetErrStackMsg`：递归拼接自身消息；栈层不贡献文本，规范化错误通过 `normalize::error_message/error_cause` 处理，基础错误优先使用 `fundamental_message`，最后才退回 `to_string()`。

## 执行流程

典型 `Wrap(Some(root), "read row")` 流程如下：

1. 对 `root` 调用 `HasStack`，记录原因链是否已有有效栈。
2. 创建 `WithMessageError`，保存 `root`、上下文文本和步骤 1 的缓存值。
3. 创建最外层 `WithStackError`，以 `NewStack(1)` 捕获调用者位置；返回新的 `SharedError`。
4. 普通展示沿 `WithStackError::Display -> WithMessageError::Display` 得到 `read row: <root>`。
5. 交替调试展示由 `fmt_extended` 先写根原因、再写 `read row`，最后由最外层栈包装追加栈帧。
6. `Unwrap` 第一次移除栈层，第二次移除消息层；`Cause` 循环执行同一逻辑直至根错误。

`AddStack` 在取得原因后先运行 `HasStack`：已有非空栈则直接返回原值，保持 `SharedError::ptr_eq`；否则用 `NewStack(1)` 新建 `WithStackError`。`WithStack` 不做该检查，因此可显式跨异步或线程边界再记录一份调用位置。

`Find` 不自行写遍历循环，而是调用 `WalkDeep`。后者先访问当前节点，再沿 `Unwrap` 单链下钻，最后递归 `ErrorGroup::Errors` 返回的子节点；因此查找顺序是外层到内层、单因链优先、组子项随后，首个命中终止。

`SuspendStack` 通过 `clear_stack` 清理库所知的真实栈：基础错误由 `without_fundamental_stack` 转成无栈形态，栈包装被替换为空 `Stack`，消息包装在下层确实被清理时重建。若整个链没有可清栈，则额外挂一个空栈层，使后续 `HasStack` 仍把它视作“无有效栈”。

## 数据与状态

所有权核心是 `SharedError`：包装器持有其克隆，`Unwrap`、`Cause`、`Find` 也返回克隆，因此错误节点是共享对象而不是深拷贝；测试用 `ptr_eq` 验证 `AddStack` 去重和根原因身份保留。

`Stack` 在构造栈包装时由 `NewStack(skip)` 捕获，之后只读；`Stack::default()` 表示显式空栈。`WithMessageError::cause_has_stack` 在包装时计算一次，用于 `HasStack` 快查。该字段不是独立可变状态；包装链在本 API 中不可原地修改，`clear_stack` 通过重建节点产生新链。

消息是拥有所有权的 `String`。`Wrapf` 在包装时完成格式化；`GetErrStackMsg` 在遍历时为结果分配并拼接字符串。文件中没有模块级常量、全局变量、trait 定义或条件编译项。

## 依赖与调用关系

直接下游关系包括：

- [`core.rs`](core.rs)：`format_message`、`fundamental_message`、`fundamental_stack_tracer`、`without_fundamental_stack`，分别支撑格式化、基础消息/栈读取和清栈。
- [`stack.rs`](stack.rs)：`NewStack`、`Stack`、`StackTrace`、`StackTracer`、`StackTraceCarrier` 与 crate 根再导出的 `GetStackTracer`。
- [`normalize.rs`](normalize.rs)：`error_cause`、`error_message`，使规范化错误加入同一导航和消息链。
- [`group.rs`](group.rs)：`WalkDeep`，让 `Find` 从单因链扩展到错误组。

直接上游关系包括：

- [`mod.rs`](mod.rs) 公开再导出本文件 API。
- [`adaptor.rs`](adaptor.rs) 的 `Trace` 调用 `AddStack`；`Annotate`/`Annotatef` 组合 `HasStack`、`WithMessage`、`with_stack`；`SuspendStack` 调用 `suspend_stack`。
- [`normalize.rs`](normalize.rs) 的规范化错误生成/包装路径调用 `AddStack`，并由本文件识别其 cause、message 与基础栈。
- 独立测试 [`tests/wrap_test.rs`](tests/wrap_test.rs)、[`tests/adaptor_test.rs`](tests/adaptor_test.rs)、[`tests/format_test.rs`](tests/format_test.rs)、[`tests/normalize_generation_test.rs`](tests/normalize_generation_test.rs)、[`tests/std_interop_test.rs`](tests/std_interop_test.rs) 和 [`tests/api_parity_test.rs`](tests/api_parity_test.rs) 覆盖行为、格式、互操作及公开表面。

RustCodeGraph 对 `pkg/errors/wrap.rs` 报告 7 个直接使用文件，并确认 `group.rs`、`adaptor.rs` 与上述测试边。由于公开函数从 crate 根再导出，跨 crate 使用通常表现为对 `astersql-errors` 的依赖；根 [`Cargo.toml`](../../Cargo.toml) 以 `facade_errors` 指向本 crate，多个 `pkg`/`br` 子 crate 也通过路径依赖复用它。

## 错误处理与边界

所有接收 `Option` 的公开包装/遍历函数都把 `None` 当作 Go `nil error`：包装函数返回 `None`，`Cause`/`Unwrap`/`Find` 返回 `None`，`GetErrStackMsg` 返回空串。代码没有 panic 型业务分支；内部 `expect` 只存在于调用方 [`adaptor.rs`](adaptor.rs)，用于断言已知的 `Some` 经 `WithMessage` 后仍为 `Some`。

`immediate_cause` 有意只遍历本库已知包装和规范化错误。任意外部 `StdError::source()` 虽可通过标准 trait 遍历，却不会自动成为 `Unwrap`/`Cause` 的本库 cause；根外部错误因此是此链的终点。相反，两个私有包装器都实现 `StdError::source`，所以标准 Rust 遍历仍能到达具体外部根错误。

`HasStack` 只把非空栈视为有效栈，这是 `NewNoStackError`、快速规范化错误与 `SuspendStack` 后仍允许上层 `Trace`/`AddStack` 补栈的关键边界。消息层的缓存必须与构造时原因链一致；若以后引入可变错误链或新的包装类型，需要重新审视这个不变量。

格式语义也属于兼容边界：普通显示必须保持 `outer: inner`；交替调试必须按根原因、各层消息、对应栈的顺序输出；`GetErrStackMsg` 不能混入文件名、帧或其他格式装饰。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。错误链由 `SharedError` 提供共享所有权；包装器字段只在构造时写入，之后通过共享引用读取，因此本文件没有显式同步协议。

调用栈的生命周期与持有它的 `SharedError` 相同。捕栈发生在 `WithStack`、`AddStack` 缺栈分支、`Wrap`、`Wrapf` 或内部 `with_stack` 的调用时刻；释放由 Rust 所有权自动完成。`clear_stack` 会克隆共享原因并重建必要包装层，旧链可继续被其他克隆持有，不会被原地破坏。

跨执行边界若需要记录第二处位置，应显式使用 `WithStack`；若只想保证链上存在一份栈，应使用 `AddStack`/`Trace`。这一选择既影响诊断信息，也影响捕获 backtrace 的时间和内存成本。

## 与 Go 版本的对应关系

仓库没有 `pkg/errors` 下的同路径 Go 文件；直接基准是 `go.mod`/`go.sum` 锁定的 `github.com/pingcap/errors v0.11.5-0.20260508054701-306e305bcf41`，本机模块缓存中该版本的 `errors.go`、`errors_test.go`、`format_test.go` 与 `stack_test.go` 提供实现和回归证据。

Rust `WithStackError`/`WithMessageError` 分别映射 Go `withStack`/`withMessage`；`Display` 与交替 `Debug` 承担 Go `Error()` 与 `%+v` 的主要语义；`StdError::source` 对应 Go 1.13 `Unwrap()`。`WithStack`、`AddStack`、`Wrap`、`Wrapf`、`WithMessage`、`Cause`、`Unwrap`、`Find`、`GetErrStackMsg` 的 nil 处理、消息顺序、栈去重和深度查找策略均保持直接对应。

语言层差异是：Go 使用接口值和 `nil`，Rust 使用 `Option<SharedError>`；Go 格式化采用可变参数，Rust 使用 `&[ErrorArg]` 与 `format_message`；Go 私有包装通过 `Cause`/`Unwrap` 接口发现，Rust 对本 crate 私有具体类型做 downcast，并显式接入规范化错误。Go `Find` 同样委托 `WalkDeep`，Rust 因此保留了错误组子树搜索，而没有简化成仅遍历 `source()`。

Go 注释将 `WithStack`/`Wrap`/`Wrapf` 标为大多数场景下应优先使用去重 API 的旧式入口；Rust 实现保留它们的“总是捕栈”行为以兼容现有调用，并由 `adaptor.rs` 提供 `Trace`/`Annotate` 的去重策略。

## 扩展指南

新增一种单因包装器时，至少需要同步处理以下接入点：在 `immediate_cause` 中暴露下一层，在 `fmt_extended` 中定义交替调试顺序；若包装器携带栈，还要让 `StackTraceCarrier::stack_tracer` 或相关基础栈辅助函数能识别它；若携带自身消息，则让 `GetErrStackMsg` 只提取本层消息而不重复整个 `Display`。涉及挂起栈时还应扩展 `clear_stack`，并明确空栈与缓存的含义。

更改公开 API 时同步更新 [`mod.rs`](mod.rs) 的再导出与 [`tests/api_parity_test.rs`](tests/api_parity_test.rs)。包装、遍历、身份和错误组顺序放在独立的 [`tests/wrap_test.rs`](tests/wrap_test.rs)；格式顺序放在 [`tests/format_test.rs`](tests/format_test.rs)；清栈/恢复行为放在 [`tests/adaptor_test.rs`](tests/adaptor_test.rs)；规范化链与标准 `source` 互操作分别放在现有 normalize/std interop 测试中。测试逻辑不要内嵌进 `wrap.rs`。

兼容风险集中在 `Cause`/`Find` 的遍历顺序、`GetErrStackMsg` 的精确文本、交替 `Debug` 的换行及栈位置。性能风险集中在每次捕获 backtrace、递归遍历、消息克隆和字符串拼接；不要为追求统一而让 `AddStack` 重复捕栈，也不要移除 `cause_has_stack` 缓存而未评估深链调用成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/errors/wrap.rs` 读取完整 326 行并确认全部类型、impl、函数和无条件编译结构；文件关系报告 7 个直接使用文件。精确 `query` 确认 `wrap.rs::HasStack`、`wrap.rs::GetErrStackMsg`、`wrap.rs::suspend_stack` 的签名与位置；宽泛 `callers/callees` 查询未返回可用的精确边，因此调用关系又由已索引的 `adaptor.rs`、`group.rs` 源码和直接引用搜索交叉核验。
- Rust crate：[`Cargo.toml`](Cargo.toml)、[`mod.rs`](mod.rs)、[`core.rs`](core.rs)、[`stack.rs`](stack.rs)、[`normalize.rs`](normalize.rs)、[`group.rs`](group.rs)、[`adaptor.rs`](adaptor.rs)。
- Rust 测试：[`tests/wrap_test.rs`](tests/wrap_test.rs) 验证 `None`、原因身份、栈捕获/去重、格式顺序、标准 `source`、`Find` 与错误组；[`tests/adaptor_test.rs`](tests/adaptor_test.rs) 验证清栈与恢复；[`tests/format_test.rs`](tests/format_test.rs) 验证组合格式；[`tests/api_parity_test.rs`](tests/api_parity_test.rs) 覆盖公开 API；normalize 与 std interop 测试覆盖跨类型接线。
- Go 对照：仓库 `go.mod`、`go.sum` 锁定提交 `306e305bcf41`；模块缓存对应版本的 `errors.go`、`errors_test.go`、`format_test.go`、`stack_test.go` 已核对。仓库内不存在 `pkg/errors/wrap.go`，因此未虚构同路径 Go 文件。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证另以任务指定命令确认目标文档存在且恰有 11 个固定二级标题。

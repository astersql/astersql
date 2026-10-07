# `pkg/errors/adaptor.rs`

## 文件定位

`pkg/errors/adaptor.rs` 是 `astersql-errors` crate 的 Juju / `github.com/pingcap/errors` 兼容层。它不定义新的错误容器，而是把 `Trace`、`Annotate`、`SuspendStack`、`ErrorStack` 以及若干分类错误构造函数，组合到本 crate 已有的 `SharedError`、消息包装和堆栈实现之上。`pkg/errors/mod.rs` 将本文件的全部公开函数从 crate 根重新导出，因此调用方通常写 `astersql_errors::Trace` 或通过别名写 `errors::Trace`，不会直接引用私有 `adaptor` 模块。

crate 边界由 `pkg/errors/Cargo.toml` 确定：包名是 `astersql-errors`，库入口是 `mod.rs`，使用 Rust 2024 edition；运行时依赖只有 `backtrace` 与带 `derive` 的 `serde`。本文件自身没有 feature 或条件编译项。仓库中不存在 `pkg/errors/doc.go`；Go 语义来源是 `go.mod` 锁定的 `github.com/pingcap/errors` 版本及其 `juju_adaptor.go`。

## 核心职责

- 保留 Go/Juju 风格调用面：`Trace` 是 `AddStack` 的兼容别名；`Annotate`/`Annotatef` 给已有错误添加上下文，同时保证链上至多新增一份有效调用栈。
- 提供“现在不捕栈、由更高层决定是否捕栈”的构造与变换：`NewNoStackError`、`NewNoStackErrorf` 和 `SuspendStack`。
- 将错误链渲染成诊断字符串：`ErrorStack` 对存在的错误采用 `SharedError` 的 alternate `Debug` 格式，以展开本库识别的消息层和堆栈。
- 用与 Go 适配器相同的固定英文后缀构造分类错误，并用文本包含关系识别 `not found` 与 `already exists`。

该文件是兼容 API 与策略编排层；实际错误节点、格式化器、堆栈捕获和链遍历分别位于 `pkg/errors/core.rs`、`pkg/errors/wrap.rs` 与 `pkg/errors/stack.rs`。

## 主要符号

- `Trace(Option<SharedError>) -> Option<SharedError>`：公开兼容入口，直接调用 `AddStack`。输入 `None` 返回 `None`；错误链已有非空栈时返回原 `SharedError`，否则包装一次新栈。
- `Annotate(Option<SharedError>, impl Into<String>) -> Option<SharedError>`：为存在的错误添加消息。它先用 `HasStack` 记录原因链是否已有栈，再调用 `WithMessage`；仅在原链无栈时调用 `with_stack(annotated, 2)`。
- `Annotatef(Option<SharedError>, &str, &[ErrorArg]) -> Option<SharedError>`：先经 `format_message` 执行 Go 风格格式化，其余堆栈策略与 `Annotate` 相同。
- `NewNoStackError(impl Into<String>) -> SharedError` 与 `NewNoStackErrorf(&str, &[ErrorArg]) -> SharedError`：分别调用内部 `new_no_stack`，或先格式化再调用前者，构造携带显式空 `Stack` 的基础错误。
- `SuspendStack(Option<SharedError>) -> Option<SharedError>`：对存在的错误调用 `suspend_stack`，清除本库能识别的真实栈；若链上没有可清理节点，则增加空栈包装作为占位。
- `ErrorStack(Option<&SharedError>) -> String`：`None` 返回空串，存在错误时返回 `format!("{error:#?}")`。
- `IsNotFound(&SharedError) -> bool` 与 `IsAlreadyExists(&SharedError) -> bool`：分别检查完整显示文本是否包含 `"not found"` 或 `"already exists"`。
- `NotFoundf`、`BadRequestf`、`NotSupportedf`、`NotValidf`、`AlreadyExistsf`：公开分类构造器，统一委托私有 `suffixed_error`。
- `suffixed_error(&str, &[ErrorArg], &str) -> SharedError`：唯一私有函数；按输入长度预分配 `String`，拼接格式串与固定后缀，再交给 `Errorf` 格式化并创建带栈基础错误。

本文件没有常量、静态变量、类型、trait、`impl` 块或条件编译项。

## 执行流程

`Trace` 的路径最短：可选错误进入 `AddStack`；`None` 立即结束，已有非空栈时保持对象身份，无栈时创建一个 `WithStackError`。`pkg/errors/tests/adaptor_test.rs` 通过 `ptr_eq` 验证重复 `Trace` 不再包装。

`Annotate`/`Annotatef` 的顺序是关键不变量：

1. 先对原 `SharedError` 调用 `HasStack`，避免添加消息层后丢失判断依据。
2. `Annotate` 直接传入消息；`Annotatef` 先用 `format_message(format, args)` 生成消息。
3. `WithMessage` 创建新的外层节点，并在节点内缓存 `cause_has_stack`；错误显示成为“上下文: 原因”。
4. 原因已有栈时直接返回消息包装，不再捕栈；原因无栈时以 `skip = 2` 调用 `with_stack`，跳过捕栈辅助函数和 adaptor 自身帧。

无栈路径中，`NewNoStackErrorf` 先格式化参数，再由 `NewNoStackError`/`new_no_stack` 创建 `Fundamental { message, stack: Stack::default() }`。后续 `Trace` 通过 `HasStack == false` 判断可以补上一份有效栈。

分类路径中，五个 `*f` 函数只选择不同后缀；`suffixed_error` 将后缀附在格式串之后，再一次性调用 `Errorf`。因此参数仍由原格式串消费，固定后缀不会引入额外格式参数，并且结果按 `Errorf` 的基础错误规则携带调用栈。

## 数据与状态

所有公开错误值使用 `SharedError`。依据 `pkg/errors/core.rs`，它内部以 `Arc<dyn Error + Send + Sync + 'static>` 持有错误，克隆不会复制整条错误链；`Option<SharedError>` 对应 Go API 的可空 `error`。Go 的可变格式参数在 Rust 中显式映射为借用切片 `&[ErrorArg]`。

`Annotate`/`Annotatef` 新建消息节点，保留原 `SharedError` 作为 cause，并缓存创建当时的 `cause_has_stack`。`SuspendStack` 不维护全局状态：它递归重建本库识别的 `Fundamental`、`WithStackError` 与 `WithMessageError` 节点，把真实栈替换为空 `Stack`；`WithMessageError` 的缓存标记按现有实现保留。后一细节解释了嵌套测试中清栈后 `HasStack` 仍可能为真、再次 `Trace` 不重复包装，而诊断串只剩根消息与上下文的行为。

分类信息不是枚举、错误码或独立字段，只存在于最终错误消息的英文后缀中；对应的 `Is*` 函数也只读取渲染文本。

## 依赖与调用关系

本文件的直接下游依赖如下：

- `pkg/errors/core.rs`：`format_message` 处理 Go 风格格式串，`new_no_stack` 创建空栈基础错误，`Errorf` 创建带栈格式化错误，`ErrorArg` 与 `SharedError` 定义参数和返回值。
- `pkg/errors/wrap.rs`：`AddStack`、`HasStack`、`WithMessage` 是公开链操作；`with_stack` 与 `suspend_stack` 是 crate 内部辅助函数。
- `pkg/errors/mod.rs`：声明私有 `adaptor` 模块，并把本文件的 14 个公开函数全部再导出。

RustCodeGraph 确认 `NotFoundf`、`BadRequestf`、`NotSupportedf`、`NotValidf`、`AlreadyExistsf` 都调用 `suffixed_error`，`NewNoStackErrorf` 调用 `NewNoStackError`。代表性上游包括：`pkg/types/helper.rs`、`pkg/types/datum.rs` 和 `pkg/store/driver/error/error.rs` 使用 `Trace`；`pkg/dxf/framework/storage/task_table.rs` 使用 `Annotatef`；`lightning/pkg/progress/progress.rs` 与 `lightning/pkg/checkpoints/checkpoints.rs` 使用 `NotFoundf`/`ErrorStack`。这些调用表明本文件处在业务错误产生或传播与最终日志/用户诊断之间，而不是 SQL 执行或存储状态机本身。

## 错误处理与边界

- `Trace`、`Annotate`、`Annotatef`、`SuspendStack` 对 `None` 均保持 `None`，不会为了上下文创建一个新错误；`ErrorStack(None)` 返回空串。
- `Annotate`/`Annotatef` 中的 `expect("present cause remains present")` 依赖 `WithMessage(Some(cause), ...)` 必然返回 `Some`。这是内部 API 不变量，不是可恢复的业务失败分支。
- `IsNotFound` 与 `IsAlreadyExists` 是大小写敏感的子串判断。它们可能把包含相同短语但并非由本文件分类构造器产生的错误识别为对应类别，也不会识别改写大小写或本地化后的消息；扩展时不能把它们误当作类型安全分类。
- `suffixed_error` 直接拼接后缀；调用方若已在格式串末尾放入同一后缀，会得到重复文本。格式占位符缺参、未知动词等行为继承 `format_message`，本文件不另行校验。
- `ErrorStack` 只会展开 `pkg/errors` 格式器认识的链和栈。外部错误没有本库堆栈节点时，输出退化为其 `Display`/`Debug` 行为。
- `SuspendStack` 只清理由本库具体节点表示的栈，且嵌套消息节点的缓存语义会影响后续 `HasStack`；修改清栈逻辑时必须同步验证这一兼容边界。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄、网络连接或事务，也没有模块级可变状态。`SharedError` 的 `Arc` 和 `Send + Sync` 约束允许错误值在线程间共享；这里的函数只消费/克隆/重建错误节点，不实施额外同步。

主要资源成本是字符串分配、`Arc` 节点分配和调用栈捕获。`Annotate`/`Annotatef` 总会新增消息节点，但仅在原链无栈时捕获栈；`Trace` 通过 `AddStack` 避免重复捕获；分类构造器通过 `Errorf` 捕获一次基础栈。`suffixed_error` 使用 `String::with_capacity(format.len() + suffix.len())` 避免拼接时不必要的再次扩容。错误链生命周期由 `Arc` 引用计数管理，函数返回值离开作用域后无需显式清理。

## 与 Go 版本的对应关系

Go 基准是 `go.mod` 中的 `github.com/pingcap/errors v0.11.5-0.20260508054701-306e305bcf41`，对应模块缓存中的 `juju_adaptor.go`。公开函数集合与核心策略逐项对应：`Trace` 委托 `AddStack`；`Annotate`/`Annotatef` 先判断已有栈、再包消息、必要时捕栈；无栈构造器放入空栈；`SuspendStack` 清理已知栈或补空栈层；`ErrorStack` 使用扩展诊断格式；分类函数拼固定英文后缀，判定函数做字符串包含检查。

Rust 映射的主要语言差异是：Go `error`/`nil` 映射为 `SharedError`/`Option`，`...interface{}` 映射为 `&[ErrorArg]`，共享所有权由 `Arc` 表达；Go 的 `fmt.Sprintf("%+v", err)` 映射为 Rust 的 alternate `Debug`（`{:#?}`）。Go `clearStack` 可原地改写特定节点，Rust `suspend_stack` 因共享所有权而重建可识别节点。Rust 捕栈调用使用显式 `skip = 2` 达到跳过 adaptor 帧的目的。

Go 注释仍称 `SuspendStack` 为 deprecated，但当前 Rust 公共函数没有 `#[deprecated]` 属性；这是可见 API 元数据差异，不应在未评估仓库调用方前擅自添加。Go 源中 `NewNoStackErrorf` 的英文注释称“with error stack”，但代码实际写入 `emptyStack`；Rust 实现与 Go 代码及其测试行为对齐，而不是照搬该注释的矛盾措辞。

## 扩展指南

- 新增 Juju 分类构造器时，优先复用 `suffixed_error`，保持“格式串 + 固定后缀 → `Errorf`”路径；同时在 `pkg/errors/mod.rs` 再导出，并在 `pkg/errors/tests/adaptor_test.rs` 添加精确消息断言。
- 新增分类判定时，先确认 Go 上游究竟使用字符串兼容还是结构化类型。若仍是字符串判定，应覆盖误匹配、大小写和嵌套上下文；若改为结构化分类，则属于跨 `core.rs`/`wrap.rs` 的兼容设计，不能只改本文件。
- 修改 `Annotate` 堆栈策略时，应保持“先检查 cause、后包消息”的顺序，并验证已有栈复用、无栈补栈、重复 `Trace` 对象身份以及捕栈位置。不要把 `Wrap` 的“总是新增栈”语义误用到 `Annotate`。
- 修改 `SuspendStack` 时必须联动 `pkg/errors/wrap.rs::{suspend_stack, clear_stack}`，重点覆盖外部错误、`Fundamental`、多层 `WithStackError`、嵌套 `WithMessageError` 和后续 `Trace`；测试逻辑继续放在独立的 `pkg/errors/tests/adaptor_test.rs`，不要嵌入生产源文件。
- 扩充格式参数能力应落在 `core.rs::format_message`/`ErrorArg`，本文件只负责转发；需同步检查 Go 格式兼容和 `pkg/errors/tests/api_parity_test.rs` 的公开 API 清单。
- 性能评审应重点关注错误热路径上的回溯捕获与节点分配；保持 `Trace`/`Annotate` 的栈去重比微调后缀拼接更重要。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/errors/adaptor.rs` 给出 1--124 行完整符号；`callers/callees --file` 核对 `suffixed_error` 的五个调用者和 `NewNoStackErrorf → NewNoStackError`；`query`/`node` 核对 `wrap.rs::{HasStack, with_stack, suspend_stack, WithMessage}` 与 `core.rs::new_no_stack`。
- Rust 源与 crate 声明：`pkg/errors/adaptor.rs`、`pkg/errors/core.rs`、`pkg/errors/wrap.rs`、`pkg/errors/mod.rs`、`pkg/errors/Cargo.toml`。`pkg/errors` 下未发现 `doc.go`。
- Rust 独立测试：`pkg/errors/tests/adaptor_test.rs` 验证 `None` 透传、栈去重与来源、消息格式、无栈构造、挂起/恢复、嵌套缓存语义以及分类后缀；`pkg/errors/tests/api_parity_test.rs` 验证公开 API 盘点与 Go/Rust 参数映射。
- Go 对照：仓库 `go.mod` 锁定 `github.com/pingcap/errors` 提交 `306e305bcf41`；模块缓存中的 `juju_adaptor.go` 是直接实现对照，`errors_test.go`、`format_test.go`、`stack_test.go` 提供 nil、格式化、堆栈位置、无栈构造与 `SuspendStack` 行为证据。
- 上游使用证据：RustCodeGraph 与 `rg` 命中 `pkg/types/helper.rs`、`pkg/types/datum.rs`、`pkg/dxf/framework/storage/task_table.rs`、`lightning/pkg/progress/progress.rs`、`lightning/pkg/checkpoints/checkpoints.rs` 等真实调用点。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前用任务规定的命令确认目标文件存在且恰好包含 11 个固定二级标题，并人工检查没有建议把 Rust 测试写入生产文件。

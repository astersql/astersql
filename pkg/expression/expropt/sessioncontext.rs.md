# `pkg/expression/expropt/sessioncontext.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”体系中会话上下文这一项的适配层。`pkg/expression/expropt/lib.rs` 以私有模块 `sessioncontext` 装入它，再通过 `pub use sessioncontext::*` 导出其公开类型。它不拥有完整 SQL 会话，也不直接完成向量推理；其职责是把 `EMBED_TEXT` 所需的最小会话能力放入 `exprctx::OptPropSessionContext` 槽位，并让表达式在求值时按类型安全地取回这些能力。

crate 边界由 `pkg/expression/expropt/Cargo.toml` 给出：本文件直接使用同 crate 的 `exprctx`、`OptionalEvalPropContext`、`RequireOptionalEvalProps` 和 `get_prop_provider`，并通过依赖 `astersql-inference` 暴露嵌入运行时类型。运行时注入点在 `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext`，当前主要消费点在 `pkg/expression/builtin_inference.rs::builtinEmbedTextSig::evaluate`。

## 核心职责

1. `SessionContext` 定义 `EMBED_TEXT` 需要的窄能力边界，避免低层表达式属性 crate 依赖完整 session/domain 实现。
2. `SessionContextPropProvider` 持有一个 `Arc<dyn SessionContext>`，把它注册为键 `OptPropSessionContext` 对应的 Provider，并提供 `Any` 视图供安全向下转型。
3. `SessionContextPropReader` 声明调用方必须准备该可选属性，并通过公共的 `get_prop_provider` 校验“属性存在、键匹配、具体 Provider 类型匹配”后返回借用的 `dyn SessionContext`。
4. 文件只传递能力和生命周期，不决定部署模式、参数合法性、推理错误或向量维数；这些行为位于 `pkg/expression/builtin_inference.rs`。

## 主要符号

- `pub trait SessionContext`：会话侧必须实现的窄接口。
  - `embedding_runtime(&self) -> Option<Arc<inference::EmbedFn>>` 返回 Domain 所拥有的嵌入运行时；`None` 表示尚未初始化。
  - `embedding_cancellation(&self) -> Option<String>` 返回一次取消/终止信号的可选表示，由推理调用闭包按需读取。
  - `embedding_context_values(&self) -> inference::embed_fn::ContextValues` 提供推理上下文值；默认实现返回 `Default::default()`，因此只需要前两项的实现者无需额外样板代码。
- `pub struct SessionContextPropProvider { session: Arc<dyn SessionContext> }`：实际保存属性值的 Provider。字段私有，外部只能通过构造器创建并通过 Reader 访问。
- `SessionContextPropProvider::new<T: SessionContext + 'static>(session: Arc<T>) -> Self`：把具体会话实现擦除为 trait object。`'static` 约束保证 Provider 可被求值上下文长期持有；类型层面不存在空 `Arc`。
- `impl exprctx::OptionalEvalPropProvider for SessionContextPropProvider`：`Desc` 固定返回 `exprctx::OptPropSessionContext.Desc()`；`as_any` 返回自身，支持 `get_prop_provider` 的安全 downcast。
- `pub struct SessionContextPropReader`：无字段的 Reader，可按值临时构造。
- `impl RequireOptionalEvalProps for SessionContextPropReader`：`required_optional_eval_props` 只返回 `OptPropSessionContext.AsPropKeySet()`，供表达式提前声明依赖。
- `SessionContextPropReader::get_session_context`：唯一读取入口。它接受任意 `OptionalEvalPropContext`，返回值生命周期与传入上下文绑定，不复制或延长会话对象寿命。

## 执行流程

生产路径如下：

1. `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 创建求值上下文，并用 `EmbeddingSession(Arc<C>)` 把上层 session 的三个嵌入相关方法适配为本文件的 `SessionContext`。
2. `SessionContextPropProvider::new` 接收该适配器的 `Arc`，保存为 `Arc<dyn SessionContext>`。
3. `EvalContext::set_optional_prop` 根据 Provider 的 `Desc().Key()` 把它放入 `OptPropSessionContext` 槽位。
4. `builtinEmbedTextSig::RequiredOptionalEvalProps` 声明同一个键；实际求值时，`builtinEmbedTextSig::evaluate` 构造 `SessionContextPropReader` 并调用 `get_session_context(ctx)`。
5. Reader 调用 `get_prop_provider::<SessionContextPropProvider, _>`。公共读取逻辑先找槽位，再校验 Provider 自描述键，最后经 `as_any` 向下转型。
6. Reader 从 Provider 内部 `Arc` 借出 `&dyn SessionContext`。随后 `EvalEmbedTextArgsToDatum` 获取运行时、取消信号和上下文值，真正调用 `EmbedFn::embed_with_context_values`。

静态/测试求值上下文也可以直接构造 Provider；`pkg/expression/builtin_inference_test.rs::context` 就把带 mock runtime 的 Provider 注入 `exprstatic::NewEvalContext`，证明该边界不依赖完整服务器会话。

## 数据与状态

本文件保存的唯一运行时状态是 `SessionContextPropProvider::session`。它是引用计数指针，Provider 与上层会话适配器共享所有权；Reader 只返回对其中 trait object 的借用，不增加引用计数。Provider 没有缓存、全局变量或可变槽位，属性键也始终固定为 `OptPropSessionContext`。

`SessionContext` 返回的 `EmbedFn` 另用一个 `Arc` 共享，Domain 仍是运行时的真实所有者；文件顶部注释与 `sessionexpr::EmbeddingSession` 的转发实现共同说明本层不接管 Domain 生命周期。`embedding_context_values` 每次按值返回 `ContextValues`，默认是空值；取消信息同样按调用返回 `Option<String>`，本层不记忆历史信号。

## 依赖与调用关系

上游关系：

- `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 是生产注入者；同文件 `EmbeddingSession<C>` 实现本文件的 trait，并把调用转发给上层 `sessionexpr::SessionContext`。
- `pkg/expression/builtin_inference.rs::builtinEmbedTextSig::evaluate` 是生产读取者；`RequiredOptionalEvalProps` 同步声明该键。
- `pkg/expression/expropt/optional_test.rs::verify_session_context` 与 `pkg/expression/builtin_inference_test.rs` 分别覆盖属性读写和 `EMBED_TEXT` 集成使用。

下游关系：

- `crate::get_prop_provider` 提供统一查找、键校验和类型 downcast；其实现位于 `pkg/expression/expropt/optional.rs`。
- `exprctx::OptPropSessionContext`、`OptionalEvalPropDesc`、`OptionalEvalPropKeySet` 与 `OptionalEvalPropProvider` 来自 `astersql-expression-exprctx`；键定义在 `pkg/expression/exprctx/optional.rs`。
- `inference::EmbedFn` 与 `inference::embed_fn::ContextValues` 来自 `astersql-inference`，由 `Cargo.toml` 的本地路径依赖 `../../inference` 提供。
- `anyhow::Result` 只用于透传公共 Provider 查找错误，本文件不自行构造错误文本。

RustCodeGraph 对目标文件给出的文件级使用者为 `pkg/expression/builtin_inference.rs`、`pkg/expression/builtin_inference_test.rs` 和 `pkg/expression/sessionexpr/sessionctx.rs`，与上述生产注入、消费和测试关系一致。

## 错误处理与边界

`get_session_context` 的失败完全来自 `get_prop_provider`，包括三类可区分错误：上下文中没有该键；槽位 Provider 报告的键与请求键不一致；Provider 无法 downcast 为 `SessionContextPropProvider`。本函数用 `?` 原样传播 `anyhow::Error`。`builtinEmbedTextSig::evaluate` 当前把读取错误统一映射为面向 SQL 的 `EMBED_TEXT requires session context`，因此底层诊断不会直接暴露给 SQL 调用者。

Provider 构造器没有执行运行时可用性校验。即使属性存在，`embedding_runtime()` 仍可返回 `None`；该边界由 `EvalEmbedTextArgsToDatum` 转换为“需要已初始化 Domain embedding runtime”的错误。部署模式、空参数、JSON 选项、推理失败和向量维数错误也都不属于本文件。

`SessionContext` 没有 `Send`/`Sync` 超 trait，`OptionalEvalPropProvider` 也未声明这些约束；因此不能仅根据本文件断言 Provider 可以跨线程发送或并发共享。若未来需要跨线程执行，必须在真实持有容器和调用链上增加并验证相应边界，而不能只依赖内部使用 `Arc`。

## 并发与资源生命周期

`Arc` 负责 Provider 与上层会话适配器之间的共享所有权，但本文件不创建线程、任务、锁、通道或事务。`SessionContextPropProvider` 析构时只减少其内部 `Arc` 的引用计数；当最后一个所有者释放后，适配器及其持有的上层 `Arc<C>` 才会释放。

`get_session_context<'a>` 返回 `&'a dyn SessionContext`，`'a` 明确绑定到求值上下文借用。这防止调用者把裸引用保存到上下文生命周期之外。调用 `embedding_runtime` 时得到新的运行时 `Arc`，因此一次推理可在借用结束后依其自身引用计数继续持有运行时；是否允许这种使用以及取消闭包的线程安全性，仍由 `EmbedFn` 和上层实现保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/expropt/sessioncontext.go`。两侧保留相同的三段结构：`SessionContext` 能力接口、`SessionContextPropProvider`、`SessionContextPropReader`；Provider 描述符和 Reader 所需键都使用 `OptPropSessionContext`，缺少 Provider 时都返回错误。Go 的生产注入点同样是 `pkg/expression/sessionexpr/sessionctx.go::NewEvalContext`，消费点同样是 `pkg/expression/builtin_inference.go`。

当前 Rust 并非机械复刻类型形状，而是针对已移植推理运行时做了窄化：

- Go `SessionContext` 暴露 `GetTraceCtx`、`GetSessionVars`、`GetDomain`，由 `builtin_inference.go::EvalEmbedTextArgsToDatum` 再经 domain adaptor 找 runtime；Rust 直接暴露 `embedding_runtime`、`embedding_cancellation` 和 `embedding_context_values`，由 `sessionexpr::EmbeddingSession` 转发。
- Go Provider 是返回 `SessionContext` 的函数值；Rust Provider 是持有 `Arc<dyn SessionContext>` 的结构体。Go 测试可构造返回 `nil` 的 Provider，Rust 的安全构造路径不能持有空 trait object，但 trait 方法仍可用 `Option` 表达 runtime 缺失。
- Go 的上下文取消使用 `context.Context` 和 `SQLKiller`；Rust 将取消状态抽象为按需调用的 `Option<String>`，并额外传递 `ContextValues`。默认上下文值是 Rust 侧新增的兼容便利。
- Go 通过运行时类型断言取得 Provider；Rust 通过 `Any::downcast_ref` 完成同等检查，并显式报告类型不匹配。

这些差异是当前代码事实。扩展时应保持 SQL 可见行为与 Go 测试意图一致，但不能假设两侧会话接口可以逐方法直接替换。

## 扩展指南

- 若 `EMBED_TEXT` 需要新的会话级输入，优先在 `SessionContext` 增加最窄的方法，并同步 `pkg/expression/sessionexpr/sessionctx.rs::EmbeddingSession` 的转发实现；若常见实现不需要该信息，可像 `embedding_context_values` 一样提供语义明确的默认值。
- 修改属性键或 Provider 类型时，必须同步 `Desc`、`required_optional_eval_props`、`get_session_context` 的泛型类型以及 `pkg/expression/exprctx/optional.rs` 注册表；否则会在键校验或 downcast 阶段失败。
- 不要把推理执行、部署模式判断或参数解析塞入本文件；它们应继续留在 `pkg/expression/builtin_inference.rs`，以保持 Provider 只是依赖注入边界。
- 测试必须放在独立文件。Provider/Reader 的缺失与成功路径应扩展 `pkg/expression/expropt/optional_test.rs`；真实 `EMBED_TEXT` 调用、runtime 缺失、取消和上下文值传播应扩展 `pkg/expression/builtin_inference_test.rs`；生产会话适配变更还应同步 `pkg/expression/sessionexpr/migration_aster_unit_test.rs`。不得在 `sessioncontext.rs` 内嵌 `#[cfg(test)]` 测试模块。
- 同步检查 Go 的 `pkg/expression/expropt/sessioncontext.go`、`pkg/expression/builtin_inference.go` 及 `pkg/expression/expropt/optional_test.go`。重点风险是 SQL 错误文本兼容、取消语义、Domain runtime 生命周期，以及在没有显式 `Send`/`Sync` 契约时误引入跨线程使用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/expression/expropt` 确认 Rust/Go 对照和独立测试；`explore "pkg/expression/expropt/sessioncontext.rs symbols callers callees session context"` 读取了目标文件 1–63 行并列出三个直接使用文件；`query SessionContextPropProvider`、`query embedding_runtime` 和 `node OptionalEvalPropProvider` 核实了 Provider、trait 方法与类型擦除契约。
- 目标源码：`pkg/expression/expropt/sessioncontext.rs`，符号 `SessionContext`、`SessionContextPropProvider`、`SessionContextPropReader`、`get_session_context`。
- crate 与模块装配：`pkg/expression/expropt/Cargo.toml`、`pkg/expression/expropt/lib.rs`、`pkg/expression/expropt/optional.rs`、`pkg/expression/exprctx/optional.rs`。
- 生产调用链：`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext`、`EmbeddingSession<C>`；`pkg/expression/builtin_inference.rs::builtinEmbedTextSig::evaluate`、`EvalEmbedTextArgsToDatum`。
- Rust 独立测试：`pkg/expression/expropt/optional_test.rs::verify_session_context` 验证注册前失败与注册后读取；`pkg/expression/builtin_inference_test.rs::context` 及 `embedding_factory_starter_options_nulls_and_errors` 验证 mock runtime 注入和实际读取使用。
- Go 对照：`pkg/expression/expropt/sessioncontext.go`、`pkg/expression/sessionexpr/sessionctx.go::NewEvalContext`、`pkg/expression/builtin_inference.go`、`pkg/expression/expropt/optional_test.go`。
- 本任务是纯文档分析，依计划不运行 Cargo；交付前仅执行固定十一章节结构校验，并人工复核文档未把本文件描述成推理执行层或并发安全保证层。

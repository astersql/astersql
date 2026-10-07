# `pkg/expression/expropt/sessionvars.rs`

## 文件定位

源码入口：[sessionvars.rs](./sessionvars.rs)。

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”机制中会话变量这一项的适配层。它不创建或复制会话变量，而是把 `variable::SessionVars` 包装成 `exprctx::OptPropSessionVars` 对应的 provider，并给表达式侧提供带类型检查的 reader。模块由 `pkg/expression/expropt/lib.rs` 的 `mod sessionvars; pub use sessionvars::*;` 纳入并公开。

完整应用中的上游装配点是 `pkg/expression/sessionexpr/sessionctx.rs` 的 `NewEvalContext`：该函数从 `SessionContext::session_vars()` 得到 `Arc<SessionVars>`，构造 `SessionVarsPropProvider`，再写入求值上下文的可选属性表。下游包括 `pkg/expression/builtin.rs` 中用户变量读写、`pkg/expression/extension.rs` 中扩展函数上下文，以及 `pkg/planner/core/expression_rewriter.rs` 中用户变量和 `LIKE` 转义规则的表达式改写。

crate 边界由 `pkg/expression/expropt/Cargo.toml` 定义：包名为 `astersql-expression-expropt`，依赖 `astersql-expression-exprctx`、`astersql-sessionctx-variable`、`astersql-util-intest` 和 `anyhow`；本文件分别借助这些依赖表达属性键、会话状态、断言开关与可恢复读取错误。Cargo 文件没有为本逻辑声明条件 feature。

## 核心职责

1. 用 `ExproptSessionVarsProvider` 抽象“如何取得 `SessionVars`”以及“当前语句采用的时区名称”。
2. 用 `SessionVarsPropProvider` 把上述抽象挂到统一的 `OptionalEvalPropProvider` 注册表，且把自身描述固定为 `OptPropSessionVars`。
3. 用 `SessionVarsPropReader` 声明表达式对 `OptPropSessionVars` 的依赖，并从上下文中按键、按具体类型取回同一个 `SessionVars`。
4. 在测试断言开关开启时，校验 EvalContext、SessionVars 和 StatementContext 三处时区名称完全一致，尽早暴露上下文拼装错误。

它不是 SessionVars 的业务实现，也不决定变量值、SQL mode 或用户变量语义；这些状态仍由 `pkg/sessionctx/variable` 提供，本文件只负责注入、读取与一致性防线。

## 主要符号

- `pub trait ExproptSessionVarsProvider`：provider 的最小接口。`get_session_vars(&self) -> &variable::SessionVars` 是必需方法；`statement_location_name()` 默认退回 `SessionVars::location()`，便于测试替身或没有独立语句时区的实现使用。
- `impl ExproptSessionVarsProvider for variable::SessionVars`：规范实现直接返回自身，但覆盖 `statement_location_name()`，从 `StmtCtx.TimeZone()` 读取语句级时区；这使一致性检查不会把会话时区误当成语句时区。
- `pub struct SessionVarsPropProvider { vars: Arc<dyn ExproptSessionVarsProvider> }`：拥有 provider 的共享所有权。字段私有，外部只能通过构造函数和 reader 使用它。
- `SessionVarsPropProvider::new<T>(Arc<T>)`：把具体 `'static` provider 擦除为 trait object。泛型没有 `Send + Sync` 限制，符合源码注释中“会话绑定、单 EvalContext 使用”的边界。
- `impl exprctx::OptionalEvalPropProvider for SessionVarsPropProvider`：`Desc()` 返回 `exprctx::OptPropSessionVars.Desc()`；`as_any()` 返回自身，供 `get_prop_provider` 安全向下转型。
- `pub(crate) fn assert_session_vars_location_matches(...)`：读取会话时区和语句时区的字符串表示，并要求二者都与上下文时区字符串精确相等；失败时 panic，并在消息中列出三者。
- `pub struct SessionVarsPropReader`：无字段零尺寸 reader，可嵌入需要会话属性的表达式实现。
- `impl RequireOptionalEvalProps for SessionVarsPropReader`：返回仅含 `OptPropSessionVars` 的位集合，使上层在执行前知道需要装载哪类属性。
- `SessionVarsPropReader::get_session_vars`：主要读取入口。返回值生命周期绑定到传入上下文，避免 reader 延长上下文中对象的借用生命周期。

## 执行流程

注册流程如下：

1. `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 从会话取得 `Arc<SessionVars>`。
2. `SessionVarsPropProvider::new` 持有该 `Arc` 并擦除具体类型。
3. 求值上下文通过 `set_optional_prop` 按 provider 的 `Desc().Key()` 把它放入 `OptPropSessionVars` 槽位；注册表的具体行为定义在 `pkg/expression/expropt/optional.rs`。

读取流程如下：

1. 表达式通过 `SessionVarsPropReader::required_optional_eval_props` 声明键依赖；例如 `pkg/expression/builtin.rs` 的 `SetVar` builtin 将该集合并入自身需求。
2. 执行时调用 `get_session_vars(ctx)`。
3. `get_prop_provider::<SessionVarsPropProvider, _>` 先按 `OptPropSessionVars` 找槽位，再核对 provider 自描述键，最后通过 `Any` 向下转型为正确的 provider 类型。
4. reader 调用内部 trait object 的 `get_session_vars()`，取得上下文持有的原对象引用，不克隆 SessionVars。
5. 若全局 `intest::EnableAssert` 为真且 `ctx.location_name()` 返回 `Some`，则调用 `assert_session_vars_location_matches` 检查三处时区名称。
6. 校验通过或未启用校验时返回 `Ok(&SessionVars)`；属性读取错误则直接通过 `?` 返回。

真实消费例子包括：`pkg/expression/builtin.rs::CoreBuiltin::set_user_variable` 用返回对象更新 `UserVars` 和变量类型；`pkg/planner/core/expression_rewriter.rs::rewriteUserVariable` 在构造 SET_VAR 表达式前取会话状态；同文件的 `rewritePatternLikeExpr` 读取 `EnableNoBackslashEscapesInLike`；`pkg/expression/extension.rs::extensionFuncSig::evaluate_context` 则把会话信息加入扩展函数回调上下文。

## 数据与状态

- provider 保存的是 `Arc<dyn ExproptSessionVarsProvider>`，因此注册表与会话对象共享所有权；reader 返回的是其中 `SessionVars` 的借用，而不是快照。调用者看到的是同一对象，其内部状态变化也就是当前会话状态变化。
- `SessionVarsPropReader` 自身没有字段和可变状态；它只是类型化访问能力与依赖声明。
- 属性身份由 `exprctx::OptPropSessionVars` 决定。`pkg/expression/exprctx/optional.rs` 将其定义为编号 1，并在静态描述表中绑定字符串 `OptPropSessionVars`。
- 时区一致性涉及三个字符串：`OptionalEvalPropContext::location_name()`、`SessionVars::location()` 和 `ExproptSessionVarsProvider::statement_location_name()`。比较是字符串精确相等，不按 UTC 偏移量做语义等价归一化。
- `intest::EnableAssert` 是全局原子开关；本文件以 `Ordering::Relaxed` 读取，因为这里只决定是否执行诊断断言，不用它同步 SessionVars 数据。

## 依赖与调用关系

直接下游依赖：

- `crate::get_prop_provider`（`pkg/expression/expropt/optional.rs`）承担缺失键、键不一致和类型不匹配检查。
- `exprctx::OptPropSessionVars`、`OptionalEvalPropProvider`、`OptionalEvalPropKeySet` 定义属性协议。
- `variable::SessionVars` 提供实际会话状态、会话时区与 `StmtCtx`。
- `intest::EnableAssert` 控制额外一致性断言。
- `std::sync::Arc` 提供所有权共享，`std::sync::atomic::Ordering` 用于读取断言开关。

已核对的直接上游接线和消费者：

- `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 注册 `SessionVarsPropProvider`。
- `pkg/expression/builtin.rs::CoreBuiltin::set_user_variable` 读取并修改用户变量；同文件还为 `SetVar` 和若干需要会话状态的签名声明 `OptPropSessionVars`。
- `pkg/expression/extension.rs::extensionFuncSig` 持有 reader，并在执行扩展回调前读取会话变量。
- `pkg/planner/core/expression_rewriter.rs::rewriteUserVariable` 与 `rewritePatternLikeExpr` 读取会话变量。

RustCodeGraph 能解析本文件内部的 `get_session_vars -> get_prop_provider / location_name / assert_session_vars_location_matches` 调用边，但当前索引没有为 reader 方法给出完整跨文件 callers；上述跨文件调用者因此由精确 `rg` 命中和对应源码片段交叉核对，而不是把空 callers 输出误解为“没有调用者”。

## 错误处理与边界

- `get_session_vars` 返回 `anyhow::Result`。若上下文没有 `OptPropSessionVars`，底层错误为 `optional property: 'OptPropSessionVars' not exists in EvalContext`；若槽位的描述键或具体类型不匹配，`get_prop_provider` 返回各自的诊断错误。reader 不吞掉或改写这些错误。
- 时区不一致不是普通 `Result::Err`，而是断言 panic。该检查只在 `intest::EnableAssert` 开启且上下文能提供 location 时执行；实现仅提供默认 `None` 的窄测试上下文会跳过检查。
- 名称比较刻意严格：测试 `location_assertion_uses_go_compatible_exact_names` 证明 `UTC` 与 `+00:00` 即便偏移相同也视为不同。
- `SessionVarsPropProvider::new` 接收 `Arc<T>`，安全 Rust 调用方无法传入空指针；它不像 Go 构造函数那样显式运行 `AssertNotNil`。
- `statement_location_name` 的默认实现等于会话 location，仅是 trait 的兼容默认值；生产 `SessionVars` 实现会改读 `StmtCtx.TimeZone()`。新增 provider 若拥有独立语句时区，必须覆盖此方法，否则断言会失去检测语句/会话漂移的能力。
- 本文件不做锁定、事务处理、变量名校验或权限检查；调用者必须遵守 `SessionVars` 自身 API 的并发和业务约束。

## 并发与资源生命周期

`Arc` 让 EvalContext 可以拥有 provider，并让构造方保留同一 SessionVars 的共享所有权；reader 的返回引用受 `ctx` 生命周期约束，因此不能在上下文释放后继续使用。provider 被注册表中的 `Box<dyn OptionalEvalPropProvider>` 持有，随 EvalContext 一同释放；内部 `Arc` 的最后一个拥有者释放时才销毁会话变量。

这里的 `Arc` 不表示跨线程安全。`ExproptSessionVarsProvider` 没有 `Send` 或 `Sync` 上界，源码注释明确 SessionVars 与会话绑定且可能包含刻意非 `Send/Sync` 的语句运行时状态。安全扩展不能仅因为外层使用 `Arc` 就把 provider 或返回引用发送到其他线程。

本文件唯一直接原子操作是以 Relaxed 顺序读取 `intest::EnableAssert`。它不建立 happens-before 关系，也不保护 SessionVars；SessionVars 内部字段的同步策略由其实现和会话执行模型负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/expropt/sessionvars.go`：

- Go `SessionVarsPropProvider.vars variable.SessionVarsProvider` 对应 Rust `Arc<dyn ExproptSessionVarsProvider>`；Rust 用拥有型 `Arc` 满足 trait object 生命周期，而不增加 `Send/Sync` 要求。
- Go `NewSessionVarsProvider` 的非空断言在 Rust 中由 `Arc<T>` 的非空类型性质替代。
- Go `Desc`、`RequiredOptionalEvalProps` 和 `GetSessionVars` 分别对应 Rust 同职责实现，属性键和缺失属性错误路径保持一致。
- Go 通过运行时类型断言取 provider；Rust 的公共 `get_prop_provider` 用 `Any::downcast_ref` 实现安全向下转型，并额外明确报告键不一致。
- Go 在 `intest.EnableAssert` 时调用 `exprctx.AssertLocationWithSessionVars(ctx.Location(), ...)`。Rust 将等价检查局部化为 `assert_session_vars_location_matches`，并通过 `SessionVars` 的专门 trait 实现确保第三项来自 `StmtCtx.TimeZone()`。
- Rust 的窄接口允许 `location_name()` 返回 `None`，这种测试替身会跳过断言；完整 `exprctx::EvalContext` 的 blanket 实现总是返回 `Some(self.Location().to_string())`，对应生产 Go 路径。

Go 测试 `pkg/expression/expropt/optional_test.go` 验证缺失 provider 报错与返回同一 SessionVars；`pkg/expression/sessionexpr/sessionctx_test.go::TestSessionEvalContextOptProps` 验证真实 EvalContext 注册和指针同一性。Rust 的对应证据位于独立测试文件 `pkg/expression/expropt/optional_test.rs`、`pkg/expression/expropt/migration_aster_unit_test.rs`、`pkg/expression/expropt/sessionvars_test.rs` 和 `pkg/expression/sessionexpr/sessionctx_test.rs`。

## 扩展指南

- 若只新增一个需要 SessionVars 的表达式，复用 `SessionVarsPropReader`，并在该表达式的 `RequireOptionalEvalProps` 实现中合并 reader 返回的键集合；不要绕过 reader 自行从 EvalContext 转型。
- 若新增对会话状态的读取方法，优先把业务语义放在 `SessionVars` 或其所属模块，本文件只保留属性桥接。需要暴露新的独立能力时，应评估是否应新增可选属性，而不是继续扩大本 provider。
- 若新增 `ExproptSessionVarsProvider` 实现，确认其生命周期可由 `Arc` 拥有，并在语句时区独立于会话时区时覆盖 `statement_location_name()`。
- 修改属性键、描述或 downcast 机制时，应同步检查 `pkg/expression/exprctx/optional.rs`、`pkg/expression/expropt/optional.rs` 和 `pkg/expression/sessionexpr/sessionctx.rs`；键编号与描述表下标是不变量。
- 测试必须继续放在独立文件，不要内嵌到 `sessionvars.rs`。至少同步 `sessionvars_test.rs` 的严格时区名称用例、`optional_test.rs` 的缺失/注册/指针同一性用例，以及 `migration_aster_unit_test.rs` 的语句时区漂移用例；若改动真实装配，再同步 `sessionexpr/sessionctx_test.rs`。
- 兼容性风险主要是 Go/Rust 错误文字、时区名称比较和 required-property 集合漂移；性能风险主要是把热路径读取改成克隆 SessionVars、重复分配，或增加无条件检查。当前 reader 只做一次注册表查找和引用返回，额外时区字符串构造仅在断言路径发生。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/expropt` 确认目标 Rust/Go 文件与独立测试均在索引中。
- RustCodeGraph `query`：定位 `ExproptSessionVarsProvider`、`SessionVarsPropProvider`、`SessionVarsPropReader` 和 `assert_session_vars_location_matches`；`callees get_session_vars` 核对 reader 到 `location_name`、provider getter 和断言函数的内部调用边。跨文件 callers 缺失时使用源码搜索补证。
- 已读生产路径：`pkg/expression/expropt/sessionvars.rs`、`optional.rs`、`lib.rs`、`Cargo.toml`，`pkg/expression/exprctx/optional.rs`，`pkg/expression/sessionexpr/sessionctx.rs`，`pkg/expression/builtin.rs`，`pkg/expression/extension.rs`，`pkg/planner/core/expression_rewriter.rs`。
- 已读 Go 对照：`pkg/expression/expropt/sessionvars.go`、`optional_test.go`，以及真实装配测试 `pkg/expression/sessionexpr/sessionctx_test.go`。
- 已读 Rust 测试：`pkg/expression/expropt/sessionvars_test.rs`、`optional_test.rs`、`migration_aster_unit_test.rs`、`pkg/expression/sessionexpr/sessionctx_test.rs`。这些测试分别覆盖严格时区名称、缺失属性、注册后指针同一性、语句时区漂移和真实 EvalContext 装配。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以任务指定命令验证文件存在且固定二级标题恰好为 11 个，并人工复核唯一生产物、源码链接和无测试内嵌建议。

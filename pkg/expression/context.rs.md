# `pkg/expression/context.rs`

## 文件定位

`context.rs` 是 `astersql-expression` crate 的求值上下文适配层。crate 根在 [`pkg/expression/lib.rs`](lib.rs) 中用 `#[path = "context.rs"] mod expression_context` 挂载它，随后通过 `pub use expression_context::*` 把本文件 API 再导出。它不创建会话上下文，而是把 `astersql-expression-exprctx` 的 trait 重导出到表达式 crate，并为标量/向量 builtin 求值提供轻量访问器和测试断言装饰器。

crate 边界由 [`pkg/expression/Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-expression`，库入口是 `lib.rs`，上下文 trait 来自路径依赖 `exprctx-dependency = astersql-expression-exprctx`；本文件直接使用的时区类型来自 `chrono-tz`，其余 `mysql`、`types`、`errctx`、`contextutil` 由 crate 根再导出。

## 核心职责

1. 用 `pub use exprctx::{...}` 提供表达式侧的统一上下文 API：`EvalContext`、`BuildContext`、以 `AggFuncBuildContext` 名称导出的 `ExprContext`、参数值和可选求值属性类型。
2. 通过 `sqlMode`、`typeCtx`、`errCtx`、`location`、`warningCount` 和 `truncateWarnings` 把常用 `EvalContext` 能力收敛成与 Go 同名的自由函数，使 builtin 和表达式节点不需要依赖具体 session 实现。
3. 用 `assertionEvalContext` 在断言模式下审计 builtin 读取的可选属性：只允许读取 `RequiredOptionalEvalProps` 或 `AllowedOptionalEvalProps` 中已声明的 key。
4. 用 `checkEvalCtx` 检查 `EvalContext::Location()` 与 `EvalContext::TypeCtx().Location()` 一致，防止时间类型转换和日期函数观察到不同时区。
5. 定义 `StringerWithCtx`，统一表达式的带参数上下文/脱敏策略字符串化接口，并允许缺省参数上下文。

## 主要符号

- `BuildContext`、`EvalContext`、`AggFuncBuildContext`、`ParamValues` 是从 `exprctx` 重导出的公开 trait/API。实际 `EvalContext` 契约定义于 [`pkg/expression/exprctx/context.rs`](exprctx/context.rs)，其中同时要求 `WarnHandler + ParamValues`，并包含 SQL mode、类型/错误上下文、时区、当前时间、用户变量和可选属性等能力。
- `OptionalEvalPropDesc`、`OptionalEvalPropKey`、`OptionalEvalPropKeySet`、`OptionalEvalPropProvider` 是可选属性的描述、key、位集和 provider 接口；`assertionEvalContext::GetOptionalPropProvider` 依赖 key set 的 `Contains` 判定访问是否合法。
- `sqlMode(ctx) -> mysql::SQLMode`、`typeCtx(ctx) -> types::Context`、`errCtx(ctx) -> errctx::Context`、`location(ctx) -> chrono_tz::Tz` 均直接委托给 `EvalContext`。`typeCtx` 在 `constant.rs`、`column.rs`、`expression.rs`、`expr_to_pb.rs` 等类型转换路径中被使用；`errCtx` 在 `errors.rs` 和 `expr_to_pb.rs` 将运行时错误交给会话策略。
- `warningCount(ctx) -> usize` 和 `truncateWarnings(ctx, start) -> Vec<SQLWarn>` 分别读取告警数量与取出/删除指定位置之后的告警。后者把 `usize` 转为 `isize` 再调用 trait。当前 Rust 生产文件中未找到这两个自由函数的实际调用；`generator/*.rs` 中的命中是 Go 生成模板文本，不是 Rust 执行路径。
- `assertionEvalContext<'a>` 持有借用的 `&dyn EvalContext` 和可选的 `&dyn builtinFunc`，本身不复制 session 状态。它实现 `WarnAppender`、`WarnHandler`、`ParamValues` 和 `EvalContext`，除可选属性访问外全部委托给被包装上下文。
- `wrapEvalAssert(ctx, function) -> assertionEvalContext` 先调用 `checkEvalCtx`，再把当前 builtin 与上下文绑定。`new_for_test` 只在 `cfg(test)` 下存在，允许独立测试构造装饰器而不触发时区检查。
- `checkEvalCtx(ctx)` 使用 `assert_eq!` 检查时区不变式；不一致时直接 panic。
- `StringerWithCtx::StringWithCtx(&self, Option<&dyn ParamValues>, &str) -> String` 是公开 trait。`core_impl.rs` 为 `Column`、`CorrelatedColumn`、`Constant` 和 `ScalarFunction` 实现它，将 `None` 替换为 `exprctx::EmptyParamValues`。

## 执行流程

1. 上游 session/静态上下文实现 `exprctx::EvalContext`，表达式节点仅接收 `&dyn EvalContext`。
2. `ScalarFunction` 的八类向量入口 `VecEval*` 和八类标量入口 `Eval*` 先检查上下文非空。当 `intest::EnableAssert` 为 true 时，它们调用 `wrapEvalAssert(ctx, self.Function.as_ref())`，否则直接把原上下文交给 builtin（`scalar_function.rs::{VecEval*, Eval*}`）。
3. `wrapEvalAssert` 调用 `checkEvalCtx`。后者先通过 `TypeCtx()` 得到类型上下文，再比较两处 `Location()`；失败时在 builtin 执行前 panic。
4. builtin 求值期间读取普通上下文属性时，`assertionEvalContext` 直接委托。读取可选属性时，其 `GetOptionalPropProvider` 先合并当前 builtin 的 required/allowed 位集，再检查 key 是否被声明。
5. key 未声明时 panic；已声明时调用底层上下文的 `GetOptionalPropProviderUnwrapped`。该专用通道使嵌套装饰器只审计当前 builtin，不会让父 builtin 的声明再次拦截子 builtin 访问。
6. builtin 最终从底层 provider 取得能力，其求值结果或 `Result` 错误按原路径返回；装饰器不缓存、改写或吞掉结果。

## 数据与状态

`assertionEvalContext` 只有两个字段：`eval_context` 是底层上下文的共享借用，`function` 是当前 builtin 的可选共享借用。生命周期 `'a` 保证装饰器不能比两个被借用对象活得更久。它没有自有告警列表、参数列表、时区或可选 provider 映射，因此 `AppendWarning`、`AppendNote`、`WarningCount`、`TruncateWarnings`、`CopyWarnings` 和 `GetParamValue` 均保持底层状态的单一来源。

`OptionalEvalPropKeySet` 是位集形态；审计时直接对 required 和 allowed 的底层位值做按位或。“required”表示构建/求值必须具备，“allowed”允许存在时使用但不强制上游提供；两者在访问审计时都属于合法声明。

`types::Context`、`errctx::Context`、`mysql::SQLMode` 和 `chrono_tz::Tz` 是按 trait 签名返回的值；本文件不对它们做内部同步。告警列表的可变性由底层 `WarnHandler` 实现管理。

## 依赖与调用关系

- 模块装配：`pkg/expression/lib.rs -> expression_context -> context.rs`，并将公开符号提升到 crate 根。
- trait 定义：`context.rs -> exprctx::{EvalContext, BuildContext, ExprContext, ParamValues, OptionalEvalProp*}`；`exprctx::EvalContext` 又依赖 `contextutil::WarnHandler` 与 `ParamValues`。
- 断言主链：`ScalarFunction::{EvalInt, EvalReal, EvalDecimal, EvalString, EvalTime, EvalDuration, EvalJSON, EvalVectorFloat32}` 及对应 `VecEval*` -> `wrapEvalAssert` -> `checkEvalCtx` -> builtin 的 `eval*`/`vecEval*`。RustCodeGraph 对 `scalar_function.rs` 的索引源片段和 `rg` 的 16 个直接引用共同证实该路径。
- 类型与错误下游：`constant.rs`、`column.rs`、`expression.rs`、`expr_to_pb.rs` -> `typeCtx`；`errors.rs`、`expr_to_pb.rs` -> `errCtx`。这些函数仅选择底层策略，具体转换/错误处理由 `types::Context` 和 `errctx::Context` 执行。
- 字符串化下游：`expression.rs` 将 `StringerWithCtx` 纳入 `Expression` trait object 边界；`core_impl.rs` 实现具体表达式类型；`aggregation/base_func.rs`、`descriptor.rs`、`concat.rs` 从 `astersql-expression` 导入该 trait 以输出表达式文本。
- RustCodeGraph 的文件索引还报告 `context.rs` 被 `context_test.rs`、`exprstatic/exprctx.rs`、`exprstatic/exprctx_test.rs` 和 `session/runtime/planning.rs` 使用。其中后两类路径主要通过 crate 根的上下文别名连接会话/静态表达式与表达式 API，不持有 `assertionEvalContext` 内部状态。

## 错误处理与边界

- 普通委托方法保留底层结果。`CurrentTime` 和 `GetParamValue` 的 `Result` 原样传播；本文件不附加错误上下文，也不降级或吞掉错误。
- `checkEvalCtx` 是不可恢复的开发/测试断言：时区不一致时 `assert_eq!` panic。它不返回 `Result`，因此不应被用作处理外部用户输入错误的机制。
- `GetOptionalPropProvider` 对未声明 key 使用 `assert!` panic；对已声明但底层未提供的 key 则合法返回 `None`。声明合法性与 provider 存在性是两个独立条件。
- `truncateWarnings` 从 `usize` 使用 `as` 转成 `isize`，本文件未检查超过 `isize::MAX` 的值；正常调用者应传入之前由 `warningCount` 获得的实际列表边界。
- `StringerWithCtx` 的契约允许 `None`，实现者不得因缺省参数上下文 panic。现有四个核心实现通过 `EmptyParamValues` 满足该契约。
- 本文件不验证 `CurrentTime` 在相同 `CtxID` 内的稳定性，也不验证可选 provider 的类型/实例；这些属于底层 `EvalContext` 实现的责任。

## 并发与资源生命周期

本文件不启动线程/异步任务，不创建锁、通道、事务或 I/O 资源。`assertionEvalContext` 仅借用底层 context 和 builtin，随单次 `ScalarFunction::Eval*`/`VecEval*` 调用在栈上创建，在 builtin 返回后销毁；Rust 生命周期防止它逃逸为悬空引用。

线程安全与内部可变性由被借用的 `EvalContext`、`WarnHandler`、provider 和 `builtinFunc` 各自的实现保证；本文件没有额外同步。断言开关 `intest::EnableAssert` 在上游用原子 load 读取，但本文件不持有或更改该开关。告警截断会修改底层告警状态，调用者必须保证 `start` 来自同一上下文的正确求值阶段。

## 与 Go 版本的对应关系

Rust 文件直接对应 [`pkg/expression/context.go`](context.go)。类型别名/重导出、六个轻量访问器、`assertionEvalContext`、时区不变式、required/allowed 可选属性检查和 `StringerWithCtx` 的总体意图一致。Rust 的 trait object `&dyn EvalContext` 对应 Go 接口值，`Option<&dyn ParamValues>` 对应 Go 中允许 nil 的 `ParamValues`。

已核对的差异如下：

- Go `wrapEvalAssert` 会解包已有 `*assertionEvalContext`，并在 builtin 相同时复用原包装器；Rust `wrapEvalAssert` 当前总是包装传入的 context，没有身份比较或重用分支。Rust 通过 `GetOptionalPropProviderUnwrapped` 使子 builtin 的 provider 访问跳过父包装器的审计，`context_test.rs::nested_assertion_context_checks_only_the_current_builtin_like_go` 验证了这一可观察语义，但并不证明包装器对象被复用。
- Go 的时区检查使用 `intest.Assert`；Rust `checkEvalCtx` 使用无条件 `assert_eq!`。当调用 Rust `wrapEvalAssert` 时检查一定执行，但现有生产主链只在上游 `intest::EnableAssert` 为 true 时调用它。
- Go 的 optional provider 调用返回 `(provider, bool)`；Rust 返回 `Option<&dyn OptionalEvalPropProvider>`。Rust 还显式实现所有委托 trait，而 Go 通过嵌入 `EvalContext` 自动提升方法。
- Go 用独立 `allowOptionalEvalProps` 接口做运行时类型断言；Rust `builtinFunc` trait 直接提供 `AllowedOptionalEvalProps`，因此不需要额外 downcast。
- Go `truncateWarnings` 接收 `int`；Rust 公开函数接收 `usize`并在委托时转为 trait 所需的 `isize`。Go 使用 `*time.Location`，Rust 使用可复制的 `chrono_tz::Tz`。

## 扩展指南

- 增加通用求值上下文能力时，先修改 `pkg/expression/exprctx/context.rs::EvalContext`，再在 `assertionEvalContext` 的 `impl EvalContext` 中作透明委托，并同步 sessionexpr/exprstatic 等真实实现。漏掉装饰器委托会使断言模式与普通模式行为分叉。
- 新 builtin 如果读取 optional provider，必须在 `RequiredOptionalEvalProps` 中声明必需 key，或在 `AllowedOptionalEvalProps` 中声明可选 key。不应通过 `GetOptionalPropProviderUnwrapped` 规避审计；该方法是装饰器组合边界，不是 builtin 公开捷径。
- 新增上下文轻量访问器时，应先确认是否真正减少了跨 crate 耦合，并保持 Go 同路径 API 语义。如返回可变状态，需明确截断/恢复边界和并发前提。
- 修改断言包装逻辑时，要同时检查 `scalar_function.rs` 的标量与向量入口，避免只覆盖某一 EvalType。若要与 Go 严格对齐“同 builtin 重用包装器”，需先解决 Rust trait object 身份比较与安全解包问题，不能用删除审计的方式简化。
- `StringerWithCtx` 的新实现必须覆盖 `None` 参数上下文、redact 模式和带参数标记的情况；优先复用 `exprctx::EmptyParamValues`，不要在实现中对 `None` 做 `unwrap()`。
- 测试必须保持在独立文件 [`pkg/expression/context_test.rs`](context_test.rs)，不内嵌到生产源文件。可选属性的新分支至少覆盖 required、allowed、未声明 panic 和嵌套装饰器；时区逻辑应补充一致与不一致两条路径。
- 兼容性风险主要是改变 `EvalContext` 公开 trait 会强制所有实现者更新，以及 required/allowed 错分可能导致计划构建缺少必需 provider。性能风险主要是断言模式下每次标量/向量求值的包装与检查；普通路径不会创建该装饰器。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`node --file pkg/expression/context.rs --offset 1 --limit 400` 返回了全部 210 行源码及 4 个直接使用文件。
- RustCodeGraph 符号查询：`query assertionEvalContext --limit 20` 命中 Rust/Go 结构和关键方法；`query wrapEvalAssert --limit 20` 命中 Rust/Go 对应定义。精确 `callers/callees --file pkg/expression/context.rs` 在超过 60 秒后仍未产生结果，已中止；调用边改用索引的源片段与 `rg` 直接引用双重核对，没有把该超时当作“无调用者”证据。
- RustCodeGraph 源片段：`node --file pkg/expression/exprctx/context.rs --offset 1 --limit 130` 核对 `EvalContext`/`BuildContext` 契约；`node --file pkg/expression/scalar_function.rs` 分别核对向量和标量断言调用链；`node --file pkg/expression/core_impl.rs --offset 210 --limit 95` 核对 `StringerWithCtx` 的四个核心实现和 `None` 处理。
- 已读源与装配文件：`pkg/expression/context.rs`、`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`、`pkg/expression/exprctx/context.rs`、`pkg/expression/scalar_function.rs`、`pkg/expression/core_impl.rs`。`pkg/expression` 下没有 `doc.go`，因此无包级 `doc.go` 契约可读。
- 已读 Go 对照：`pkg/expression/context.go`。该路径没有 `context_test.go`；Go 行为依据来自生产实现本身，没有臆造不存在的 Go 测试。
- 已读独立 Rust 测试：`pkg/expression/context_test.rs`。其覆盖嵌套装饰器只审计当前 builtin、allowed key 可读但不计入 required、未声明 key panic，以及 `UNCOMPRESS` 的 allowed session-vars 注册。
- 直接引用核对：`rg` 确认 `scalar_function.rs` 的 16 个 `wrapEvalAssert` 调用、`typeCtx`/`errCtx` 的 Rust 生产调用，以及 `StringerWithCtx` 的 trait 边界和实现。
- 本任务是纯文档分析，按计划未运行 Cargo。最终结构验证应确认本文件存在且恰好包含本计划规定的 11 个二级标题。

# `pkg/expression/builtin_inference.rs`

## 文件定位

`builtin_inference.rs` 属于 `astersql-expression` crate，是 SQL 内建函数 `EMBED_TEXT(model, text[, options])` 的 Rust 运行时实现。crate 根在 `pkg/expression/lib.rs:875-883` 装入该模块，并向 crate 外导出部署检查、参数结构与参数/结果转换函数。`pkg/expression/builtin.rs:6394-6414,6794` 把名称 `embed_text` 注册到函数类，并限制为 2–3 个参数；因此本文件位于“SQL 表达式已经解析/重写完成”之后、“调用 embedding provider 并产生向量 Datum”之前。

`pkg/expression/Cargo.toml` 将该目录定义为 `astersql-expression`，本文件直接使用的显式 crate 依赖包括 `astersql-inference`（`inference`）、`astersql-config-deploymode`（`deploymode`）、`astersql-expression-expropt`（`expropt`）、`astersql-types`（`types-dependency`）和 `serde_json`。本文件没有条件编译项；只有对应的独立测试模块在 `pkg/expression/lib.rs:877-878` 上受 `#[cfg(test)]` 约束。

## 核心职责

- `embedTextFunctionClass::getFunction` 验证参数数量，将非字符串实参包装为到 `VAR_STRING` 的 cast，并构造返回类型为 `TypeTiDBVectorFloat32` 的 builtin 签名。返回类型被明确设为 binary charset/collation（`builtin_inference.rs:26-50`）。
- `builtinEmbedTextSig::evaluate` 是普通标量求值主链：先拒绝非 starter 部署，再从可选求值属性中取会话上下文，对 SQL 参数求值，最后调用会话持有的 Domain embedding runtime（`builtin_inference.rs:58-79`）。
- `EvalEmbedTextArgs` 将 SQL 表达式参数转成拥有所有权的 `EmbedTextArgs`，保持 SQL NULL 短路、参数求值错误与 options 过滤规则（`builtin_inference.rs:167-220`）。
- `EvalEmbedTextArgsToDatum` 调用 provider，传入取消信号和会话上下文值，检查向量维度，再封装成 `types::Datum`（`builtin_inference.rs:233-258`）。
- `CheckEmbedTextAllowed` 与公开数据契约也被 DDL/批量生成列路径复用：`pkg/ddl/generated_column.rs:405-438` 在接受 STORED `EMBED_TEXT` 前检查部署模式，`pkg/session/runtime/dml.rs:1327-1407` 构造 `EmbedTextArgs` 并把批量 provider 调用交给 executor。

## 主要符号

- `builtinEmbedTextSig`：包含 `formal_registry::RegistryBuiltinBase` 的具体 builtin 签名。它实现 `CollationInfo` 和 `builtinFunc`，大部分元数据、参数访问、相等性、clone、protobuf code、collator 与内存估算都委托给 `baseBuiltinFunc`（`builtin_inference.rs:19-22,81-165`）。
- `embedTextFunctionClass`：持有 `baseFunctionClass`，实现 `functionClass`。`getFunction` 构造具体签名，`verifyArgsByCount` 和 `getDisplayName` 委托基类（`builtin_inference.rs:23-57`）。它由 `pkg/expression/builtin.rs:6408-6413` 实例化，未从 crate 根导出。
- `EmbedTextArgs { Model, Text, Opts }`：公开、可 clone 的拥有型输入快照。字符串与 `inference::Options` 不借用 row buffer，因而可在参数求值后安全传给后续 provider/批处理阶段（`builtin_inference.rs:167-173`）。
- `CheckEmbedTextAllowed() -> Result<(), Error>`：唯一的部署级开关检查，非 `deploymode::Starter` 时返回固定错误（`builtin_inference.rs:174-181`）。
- `EvalEmbedTextArgs(...) -> Result<Option<EmbedTextArgs>, Error>`：仅求值和规范化参数，不调用 provider（`builtin_inference.rs:182-220`）。
- `EvalEmbedTextArgsFromExpr(...)`：要求传入表达式正好是由 `builtinEmbedTextSig` 支撑的 `ScalarFunction`，然后复用 `EvalEmbedTextArgs`（`builtin_inference.rs:221-232`）。
- `EvalEmbedTextArgsToDatum(...)`：把已求值参数实体化为向量 Datum，是 provider 调用与向量合法性检查的边界（`builtin_inference.rs:233-258`）。

## 执行流程

1. 函数注册表以 `("embed_text", 2, 3)` 创建 `embedTextFunctionClass`（`pkg/expression/builtin.rs:6391-6419,6794`）。
2. 构建表达式时，`getFunction` 通过 `verifyArgs` 检查 arity，逐个保留 ETString 参数或插入 `BuildCastFunction`，再创建二进制 collation 的 float32-vector 返回类型和递归 builtin base（`builtin_inference.rs:27-49`）。
3. 行求值通过 `builtinFunc::evalVectorFloat32` 进入 `evaluate`。它在任何参数求值前检查 starter 模式；这保证不支持的部署不会触发参数副作用或外部调用（`builtin_inference.rs:59-68`，对应回归 `embedding_factory_observes_deployment_before_nulls`）。
4. `SessionContextPropReader` 从 `EvalContext` 的可选属性中取得 session；`RequiredOptionalEvalProps` 声明了 `OptPropSessionContext` 依赖（`builtin_inference.rs:69-72,155-157`）。
5. `EvalEmbedTextArgs` 依次求值 model、text 和可选 options。model 或 text 为 NULL 立即返回 `Ok(None)`，不再求值后续参数；options 为 NULL/空字符串则保留空 map，否则必须解析成 JSON object，且所有以 `@search` 结尾的键被过滤（`builtin_inference.rs:182-220`）。
6. NULL 结果被转为 `(ZeroVectorFloat32, true)`；否则 `EvalEmbedTextArgsToDatum` 重新检查 session、starter 模式与参数非空，获取 `session.embedding_runtime()`，调用 `embed_with_context_values`，并传入 `embedding_cancellation` 回调与 `embedding_context_values`（`builtin_inference.rs:73-78,233-251`）。
7. provider 返回的 `Vec<f32>` 先经 `CheckVectorDimValid`，再经 `CreateVectorFloat32`，最后写入 Datum（`builtin_inference.rs:252-258`）。独立 Rust 测试确认超过 16,383 维会被拒绝（`pkg/expression/builtin_inference_test.rs:191-204`）。

## 数据与状态

`builtinEmbedTextSig` 的持久状态只是 `RegistryBuiltinBase`：参数表达式、返回类型、collation/coercibility、protobuf code 等通用 builtin 元数据。`Clone` 复制签名，`equal` 先向下转型再比较 base，`MemoryUsage` 委托 `memory_usage`（`builtin_inference.rs:112-151`）。它不缓存 embedding 结果、provider 或 session。

`EmbedTextArgs` 是单次调用的值快照：`Model` 和 `Text` 是拥有的 `String`，`Opts` 是 `inference::Options`。其拥有性是一个明确不变量：重用的 row buffer 不能在参数求值后改变 provider 输入（`builtin_inference.rs:167-173`）。options 只接受 JSON object；JSON `null`、array 和损坏的 JSON 都不会被视为空 options（`builtin_inference.rs:202-212`，`pkg/expression/builtin_inference_test.rs:128-145`）。

SQL NULL 与错误是不同通道：`Option<EmbedTextArgs>` 表示 model/text 的 NULL 短路，`Result` 表示类型求值、JSON、环境、provider 或向量构造失败。第三参数为 NULL 不会使整个函数为 NULL（`builtin_inference.rs:190-219`，`pkg/expression/builtin_inference_test.rs:147-177`）。

## 依赖与调用关系

上游与装配关系：

- `pkg/expression/lib.rs:875-883` 声明模块并导出公开 helper；`pkg/expression/builtin.rs:6408-6413,6794` 注册具体函数类和 arity。
- 普通 `Expression::EvalVectorFloat32` 通过 `builtinFunc::evalVectorFloat32` 进入本文件的 `evaluate`（`builtin_inference.rs:158-164`）。
- `pkg/ddl/generated_column.rs:405-438` 调用 `CheckEmbedTextAllowed`并与 inference AST helpers 一起约束生成列形态。
- `pkg/session/runtime/dml.rs:1327-1407` 在 STORED 生成列批量路径中使用 `CheckEmbedTextAllowed` 和 `EmbedTextArgs`，但它目前自行做 AST 参数求值，并通过 `pkg/executor/insert_common.rs:2157-2200` 并发调用 provider；这条路径不调用本文件的 `EvalEmbedTextArgsToDatum`。
- `EvalEmbedTextArgsFromExpr` 的生产语义是“只对直接 `EMBED_TEXT` 标量表达式求参数”；当前仓库内除本文件自调用与独立测试外，未找到 Rust 生产调用点（RustCodeGraph 查询与 `rg` 交叉核对）。

下游依赖：

- 表达式层：`BuildCastFunction`、`Expression::EvalString`、`ScalarFunction`、`RegistryBuiltinBase` 和 `builtinFunc`/`CollationInfo` trait。
- 会话/运行时层：`expropt::SessionContextPropReader`、`SessionContext::embedding_runtime`、`embedding_cancellation` 与 `embedding_context_values`。
- provider 与类型层：`inference::EmbedFn::embed_with_context_values`、`types_dependency::vector::CheckVectorDimValid`、`CreateVectorFloat32` 和 `types::Datum::SetVectorFloat32`。
- 配置层：`deploymode::IsStarter`是既在入口求值前又在公开 Datum helper 中执行的安全边界。

## 错误处理与边界

- 非 starter 模式立即返回 `EMBED_TEXT is only supported in starter deployment mode`。标量路径在参数求值之前检查，而可独立调用的 `EvalEmbedTextArgsToDatum` 也再检查一次（`builtin_inference.rs:64-68,174-180,237-239`）。
- 参数数量不是 2 或 3 时返回 `invalid EMBED_TEXT() usage`。工厂的注册 arity 是第一道防线，`EvalEmbedTextArgs` 依然自检以保护公开 helper（`builtin_inference.rs:187-189`）。
- model/text 的求值错误原样向上传播；任一为 NULL 则短路为 SQL NULL，且不解析之后的 options（`builtin_inference.rs:190-197`，`pkg/expression/builtin_inference_test.rs:262-295`）。
- options 非空时必须是 JSON object，否则统一返回 `EMBED_TEXT expects options in JSON format`；`@search` 后缀键是 `VEC_EMBED_*` 重写保留信息，不能影响直接 `EMBED_TEXT` 调用（`builtin_inference.rs:198-213`，Go 对照 `builtin_inference.go:177-203`）。
- `EvalEmbedTextArgsFromExpr` 拒绝非 `ScalarFunction` 或非 `builtinEmbedTextSig` 的标量函数，避免对任意表达式误用 EMBED_TEXT 参数协议（`builtin_inference.rs:226-231`）。
- session 缺失、Domain runtime 未初始化、provider 失败、取消、向量超维或向量构造失败都作为 `Error` 传播，不被转成 SQL NULL（`builtin_inference.rs:69-78,237-255`）。
- `vectorized()` 返回 `false`，本文件未提供列向量化求值；这不等同于生成列没有批处理，后者由 executor 的独立并发 helper 实现（`builtin_inference.rs:152-164`，`pkg/executor/insert_common.rs:2157-2200`）。

## 并发与资源生命周期

本文件本身不创建线程、任务、锁、通道或事务。普通标量路径在当前求值调用栈上同步完成 provider 调用。`EmbedTextArgs` 拥有它的数据，不延长 row buffer 借用；会话与 runtime 只在调用期间以借用或 `Arc` 形式访问，没有存入 `builtinEmbedTextSig`。

资源取消由 session 提供：`EvalEmbedTextArgsToDatum` 将 `session.embedding_cancellation()` 包装成回调传入 runtime，并同时传入 `embedding_context_values()`（`builtin_inference.rs:243-251`）。具体 provider 如何轮询取消及管理外部连接不在本文件内，不应在此增加隐式线程或全局 runtime。

对 STORED 生成列，并发生命周期在文件外：`pkg/executor/insert_common.rs:2160-2200` 使用 scoped threads，工作数为 `min(inputs.len(), 800)`，以原子索引领取任务，保留输入顺序，并在每个任务前检查取消。这是 `EmbedTextArgs` 需要拥有值而非借用行内存的直接原因之一，但不是 `builtinEmbedTextSig::evaluate` 的执行方式。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/expression/builtin_inference.go`，主要映射如下：

- Go `embedTextFunctionClass` / `builtinEmbedTextSig` 对应 Rust 同名类型；Go `getFunction` 通过 `newBaseBuiltinFuncWithTp` 统一要求字符串参数和 vector-float32 返回值，Rust 用显式 `BuildCastFunction` 与 `RegistryBuiltinBase` 实现相同契约（Go `builtin_inference.go:60-74`，Rust `builtin_inference.rs:26-50`）。
- Go `EvalEmbedTextArgs` 返回 `(*EmbedTextArgs, isNull, error)`；Rust 以 `Result<Option<EmbedTextArgs>, Error>` 表达同样的错误/NULL 分离。Go 的 `map[string]any` 空 options 可为 nil，Rust `inference::Options` 在无选项时是空 map，两者的 provider 可见语义一致（Go `builtin_inference.go:114-132,177-203`，Rust `builtin_inference.rs:182-220`）。
- Go `EvalEmbedTextArgsToDatum` 通过 `domainadaptor.GetEmbedFn(sctx)` 找 runtime，显式传入 `context.Context` 与 SQL killer；Rust 从 `SessionContext` 直接取 runtime、取消信号和 context values。两者均在 provider 返回后检查维度并创建 float32 vector Datum（Go `builtin_inference.go:142-175`，Rust `builtin_inference.rs:233-258`）。
- Go `builtinEmbedTextSig` 嵌入 `SessionContextPropReader`，Rust 在求值时创建无状态 reader，并在 `RequiredOptionalEvalProps` 中直接声明 session property；语义均是 session 上下文必须由可选求值属性供应（Go `builtin_inference.go:39-42,206-209`，Rust `builtin_inference.rs:69-72,155-157`）。
- Rust 独立测试 `pkg/expression/builtin_inference_test.rs` 对应 Go `pkg/expression/builtin_inference_test.go`，覆盖注册效果、starter 顺序、NULL/错误传播、options 过滤、provider 错误与 16,383 维上限。Rust 额外直接断言 optimizer trait 集合中的 `unfoldable`/mutable effect/非法生成列标记（Rust test `:17-30`）。

## 扩展指南

- 改参数数量或参数类型时，同步修改 `embedTextFunctionClass::getFunction`、`EvalEmbedTextArgs`、`pkg/expression/builtin.rs` 中的 `(min,max)` 注册、Go 对照实现，以及 `pkg/expression/builtin_inference_test.rs`。生成列 AST 提取还需同步 `pkg/expression/inference_helper.rs` 及其独立测试。
- 改 options 规则时，必须保持普通标量路径与 `pkg/session/runtime/dml.rs:1355-1373` 的 STORED 生成列批量路径一致，并核对 Go `evalEmbedTextOptions`。特别注意 `@search` 后缀是跨重写链路的兼容边界。
- 改 runtime/provider 接口时，优先修改 `EvalEmbedTextArgsToDatum`与 `expropt::SessionContext`，保留取消和 context values 传递；同时核对 `pkg/session/runtime/dml.rs:1389-1406` 的批量调用。不要把 session/runtime 缓存到 builtin 实例，否则容易破坏跨会话安全性。
- 改 NULL 或错误语义时，保留“model/text NULL 短路且不求值 options”、“options NULL 等于无选项”、“provider 错误不等于 NULL”这三个已有不变量，扩展 `embedding_arguments_preserve_null_short_circuit_errors_and_options` 和 `embedding_factory_starter_options_nulls_and_errors`。
- 改函数可折叠性、副作或生成列限制时，同步检查 `pkg/expression/function_traits.rs:65-97,220-236`、DDL 的专用例外校验和注册回归；`EMBED_TEXT` 依赖外部 runtime，误标为可折叠会导致规划阶段与运行阶段行为错位。
- 性能变更要分清两条路：本文件的普通标量调用非 vectorized；STORED 生成列的并发上限和有序结果在 executor helper。任何并发化都必须验证 session 隔离、取消、provider 线程安全及输入/结果顺序。
- Rust 测试必须继续放在独立的 `pkg/expression/builtin_inference_test.rs`，不要内嵌回生产文件；涉及批量生成列时还应扩展 `pkg/executor/insert_common_test.rs` 或 session/DDL 相应的独立测试。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/builtin_inference` 与 `node --file pkg/expression/builtin_inference.rs` 确认目标文件已索引且全文为 259 行。
- RustCodeGraph 查询：`query builtinEmbedTextSig --kind struct`、`query EvalEmbedTextArgsToDatum --kind function`、`explore 'pkg/expression/builtin_inference.rs ...'`，并用精确文件 `node` 查看 Rust 源码、Rust 独立测试、Go 对照和上下游调用点。因索引中 Go/Rust 同名符号会合并 blast radius，调用点另以限定 `*.rs` 的 `rg` 交叉核验。
- 已读生产/装配路径：`pkg/expression/builtin_inference.rs`、`pkg/expression/lib.rs`、`pkg/expression/builtin.rs`、`pkg/expression/function_traits.rs`、`pkg/expression/Cargo.toml`、`pkg/ddl/generated_column.rs`、`pkg/session/runtime/dml.rs`、`pkg/executor/insert_common.rs`。
- 已读对照/测试路径：`pkg/expression/builtin_inference.go`、`pkg/expression/builtin_inference_test.go`、`pkg/expression/builtin_inference_test.rs`；额外核对了 `pkg/expression/inference_helper_test.rs` 的 AST 参数边界名称和 `pkg/executor/insert_common_test.rs` 的 `EmbedTextArgs` 直接使用点。
- 当前代码证据确认：函数存在的目的是在 expression builtin 边界上把 SQL 参数、session/Domain runtime 与 vector Datum 连接起来；普通路径按“部署检查→session→参数→provider→维度检查→Datum”运行，批量生成列路径只复用其公开契约并在外部并发。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务文件指定的 `test` + `rg -c` 命令验证文档存在且恰有 11 个固定二级标题，并对限定 diff 进行人工自审。

# `pkg/expression/expropt/infoschema.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”体系中 InfoSchema 元数据快照的适配层。模块入口 [`lib.rs`](lib.rs) 将它以 `infoschema_provider` 私有模块装入，并公开再导出其中的 Provider 与 Reader；[`Cargo.toml`](Cargo.toml) 则通过 `astersql-expression-exprctx` 和 `astersql-infoschema-context` 两个路径依赖提供属性协议与 `MetaOnlyInfoSchema` 契约。本文件不实现 InfoSchema 本身，也不缓存或更新模式数据，而是把上层会话提供的两类快照接入表达式求值上下文。

生产侧直接装配入口在 [`../sessionexpr/sessionctx.rs`](../sessionexpr/sessionctx.rs) 的 `NewEvalContext`：它构造 `InfoSchemaPropProvider<C::InfoSchema>`，并约定闭包参数为 `true` 时调用 `SessionContext::latest_info_schema()`，为 `false` 时调用 `SessionContext::info_schema()`。当前 Rust 仓库中 Reader 的直接使用主要见独立测试；这意味着文件已接入求值上下文的 provider 注册链，但尚未看到与 Go `builtin_info.go` 相同规模的 Rust 表达式消费者。

## 核心职责

1. `InfoSchemaPropProvider<T>` 把一个“按布尔值选择快照”的闭包包装成 `OptionalEvalPropProvider`，并将其固定绑定到 `exprctx::OptPropInfoSchema`。
2. `InfoSchemaPropReader` 通过 `RequireOptionalEvalProps` 声明表达式依赖 InfoSchema 属性，使上游可以在求值前汇总所需属性集合。
3. Reader 提供语义清晰的两个入口：`get_session_info_schema` 传入 `false`，取得语句所属会话视角的快照；`get_latest_info_schema` 传入 `true`，取得 Domain/节点侧最新快照。
4. 两个读取入口复用 [`optional.rs`](optional.rs) 的 `get_prop_provider`，把“属性缺失、描述键不一致、具体 provider 类型不匹配”转换为 `anyhow::Result`，而不是在本文件复制注册表或类型擦除逻辑。

本层的重要不变量是：Provider 的描述键、Reader 声明的依赖键以及 Reader 查询的键必须全部为 `OptPropInfoSchema`；布尔值 `false/true` 分别稳定表示 session/latest，不能在单侧更改。

## 主要符号

- `InfoSchemaPropProvider<T>`：公开泛型结构体。`T` 必须实现 `infoschema::MetaOnlyInfoSchema + Send + Sync + 'static`；唯一字段 `provider` 是 `Box<dyn Fn(bool) -> Arc<T> + Send + Sync>`。泛型保持具体 InfoSchema 类型，`Arc<T>` 负责共享所有权，闭包 trait object 则隐藏上游取快照的具体方式。
- `InfoSchemaPropProvider::new<F>(provider: F) -> Self`：公开构造函数，把满足 `Fn(bool) -> Arc<T> + Send + Sync + 'static` 的闭包装箱。它不立即调用闭包，也不验证两个分支返回的版本关系。
- `InfoSchemaPropProvider::call(&self, is_domain: bool) -> Arc<T>`：公开同步调用入口，原样把选择标志交给闭包并返回其 `Arc`；自身不克隆、不缓存结果。
- `impl OptionalEvalPropProvider for InfoSchemaPropProvider<T>`：`Desc` 返回 `exprctx::OptPropInfoSchema.Desc()`；`as_any` 返回 `Some(self)`，允许 `get_prop_provider` 从 trait object 安全 downcast 回完全相同的泛型实例。
- `InfoSchemaPropReader`：无字段的公开单元结构体，不保存上下文或快照。
- `impl RequireOptionalEvalProps for InfoSchemaPropReader`：`required_optional_eval_props` 返回只含 `OptPropInfoSchema` 一位的集合。
- `get_session_info_schema<T, C>(&self, ctx: &C)`：从 `C: OptionalEvalPropContext` 取 `InfoSchemaPropProvider<T>`，成功后执行 `call(false)`。
- `get_latest_info_schema<T, C>(&self, ctx: &C)`：执行相同检索，成功后执行 `call(true)`。

文件没有模块级常量、枚举、条件编译项或内部测试模块；公开项由 [`lib.rs`](lib.rs) 再导出。

## 执行流程

生产装配与读取链如下：

1. [`../sessionexpr/sessionctx.rs`](../sessionexpr/sessionctx.rs) 的 `NewEvalContext` 克隆会话句柄，并用闭包创建 `InfoSchemaPropProvider<C::InfoSchema>`。
2. 闭包把 `is_domain` 映射为两个会话接口：`true -> latest_info_schema()`，`false -> info_schema()`。
3. `EvalContext::set_optional_prop` 将 provider 放入共享 `OptionalEvalPropProviders` 注册表中；槽位由 provider 的 `Desc().Key()`，即 `OptPropInfoSchema`，决定。
4. 某个 Reader 消费者通过 `required_optional_eval_props` 声明该键，随后调用 session 或 latest 读取方法。
5. 读取方法调用 `get_prop_provider::<InfoSchemaPropProvider<T>, _>`。共享逻辑先按键询问上下文，再检查 provider 自描述键，最后通过 `Any` downcast 校验具体泛型类型。
6. 检索成功后，Reader 分别以 `false` 或 `true` 调用 provider；闭包获取并返回对应的 `Arc<T>`。

[`optional_test.rs`](optional_test.rs) 的 `verify_info_schema` 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `infoschema_and_kv_readers_choose_and_preserve_provider_objects` 均验证：注册前两个 Reader 都报缺失错误，注册后 `true` 取得版本 22 的 Domain 快照、`false` 取得版本 11 的会话快照，并且返回值与闭包持有对象 `Arc::ptr_eq`。

## 数据与状态

本文件只有一项持久状态：`InfoSchemaPropProvider<T>::provider` 中的闭包及其捕获环境。Provider 不保存“当前快照”字段；每次 `call` 都重新执行闭包，因此快照是否固定、是否随 Domain 更新而改变，完全由装配闭包决定。生产闭包捕获 `Arc<C>` 会话上下文，两个分支分别向会话请求快照。

`InfoSchemaPropReader` 是零大小、无状态对象，可以临时构造或嵌入其他结构。`required_optional_eval_props` 返回值是 `OptPropInfoSchema.AsPropKeySet()` 生成的单比特集合；键在 [`../exprctx/optional.rs`](../exprctx/optional.rs) 中编号为 3，并对应静态描述字符串 `OptPropInfoSchema`。

返回的 `Arc<T>` 延长选中快照的生命周期并允许跨消费者共享。Provider 的闭包只接受不可变调用 `Fn`，因此此层不会通过可变借用维护游标或版本；若闭包内部需要可变状态，必须由上游自行使用线程安全的内部可变性并保证 `Send + Sync`。

## 依赖与调用关系

上游关系：

- [`lib.rs`](lib.rs) 声明 `infoschema_provider` 并 `pub use` 全部符号，形成 crate 公共 API。
- [`../sessionexpr/sessionctx.rs`](../sessionexpr/sessionctx.rs) 的 `NewEvalContext` 是已发现的生产注册调用者，负责把会话/Domain 两个来源连接到 provider。
- [`../sessionexpr/migration_aster_unit_test.rs`](../sessionexpr/migration_aster_unit_test.rs) 通过完整 `NewEvalContext` 验证注册链和两个 Reader 分支。
- [`optional_test.rs`](optional_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 在 expropt crate 内直接验证 provider/reader 契约。

下游关系：

- `exprctx::OptionalEvalPropProvider` 提供 `Desc`/`as_any` trait object 协议；`exprctx::OptPropInfoSchema` 提供键、描述及位集合。
- [`optional.rs`](optional.rs) 的 `OptionalEvalPropContext` 是 Reader 所需的最小上下文接口，`get_prop_provider` 完成按键查找与类型检查。
- `infoschema::MetaOnlyInfoSchema` 限定可返回对象至少具备表达式所需的元数据只读能力；本文件不调用该 trait 的任何查询方法。
- `std::sync::Arc` 管理 InfoSchema 与生产会话捕获对象的共享生命周期；`anyhow::Result` 承载查找阶段错误。

RustCodeGraph 为本文件识别出 `InfoSchemaPropProvider`、两个 Reader 方法及其实现符号，但 `callers`/`callees` 对这些泛型方法未返回边；上述生产与测试调用关系因此由全仓库精确符号搜索及对应源码核验补足。

## 错误处理与边界

`new` 与 `call` 没有 `Result`：闭包签名保证正常路径直接返回 `Arc<T>`，本层不表达“快照不存在”。若闭包 panic，本文件不捕获。两个 Reader 的错误只来自 `get_prop_provider`：

- 上下文没有 `OptPropInfoSchema` 时，返回 `optional property: 'OptPropInfoSchema' not exists in EvalContext` 类错误；
- provider 报告的键与请求键不同，返回键不匹配错误；
- 键存在但 trait object 不是完全相同的 `InfoSchemaPropProvider<T>` 时，返回包含目标 Rust 类型名的 downcast 错误。

类型匹配包含泛型参数：即使两个 `T` 都实现 `MetaOnlyInfoSchema`，用 `InfoSchemaPropProvider<A>` 注册后也不能以 `InfoSchemaPropProvider<B>` 读取。这与 Go 返回接口值后再断言具体类型不同，是 Rust 迁移中更严格的静态/运行时组合边界。

本文件不判断 session 与 latest 是否为同一对象、不比较 `SchemaMetaVersion`，也不保证 latest 的版本号一定更大；它只保持上游约定的选择语义。测试中的 11/22 仅用于区分分支，不是生产版本规则。

## 并发与资源生命周期

Provider 类型要求 `T: Send + Sync + 'static`，其闭包也要求 `Send + Sync + 'static`，因此 provider 可以安全装入要求并发共享的可选属性 trait object。`Arc<T>` 让返回快照在 Reader 返回后继续有效；测试用 `Arc::ptr_eq` 证明本层返回同一个分配，而不是深拷贝元数据。

Provider 自身没有锁、异步任务、通道、事务或显式清理逻辑。它的销毁会释放装箱闭包及闭包捕获的引用；每次返回的 `Arc<T>` 独立延长相应快照生命周期。闭包是否加锁、是否从 Domain 原子读取最新值、是否会阻塞，均属于 `SessionContext`/InfoSchema 实现的责任，不能从本文件推断。

## 与 Go 版本的对应关系

直接对照文件是 [`infoschema.go`](infoschema.go)。两边保持以下语义一致：

- Go `InfoSchemaPropProvider func(bool) MetaOnlyInfoSchema` 对应 Rust 装箱闭包；布尔值 `true` 代表 Domain 最新快照，`false` 代表会话快照。
- 两边 `Desc` 都返回 `OptPropInfoSchema.Desc()`，Reader 都声明 `OptPropInfoSchema.AsPropKeySet()`。
- `GetSessionInfoSchema`/`get_session_info_schema` 取 provider 后传 `false`；`GetLatestInfoSchema`/`get_latest_info_schema` 传 `true`。
- provider 缺失时，两边都先返回错误而不调用 provider；Go 行为由 [`optional_test.go`](optional_test.go) 的 `OptPropInfoSchema` 分支验证。

Rust 为所有权与类型安全做了结构性调整：Go provider 是函数类型并返回接口，Rust provider 是泛型结构体并返回 `Arc<T>`；Go 依赖运行时类型断言，Rust 通过 `as_any` 和 `downcast_ref::<InfoSchemaPropProvider<T>>()` 恢复具体类型。Rust 还显式要求 `Send + Sync + 'static`，以适应 trait object 和共享上下文。已读测试表明分支选择、错误路径与对象身份保持对齐，未发现本文件语义上的刻意简化。

## 扩展指南

- 若只新增使用 InfoSchema 的表达式，优先复用 `InfoSchemaPropReader`，把它嵌入消费者并合并 `required_optional_eval_props` 返回的键集合；不要绕过注册表直接依赖具体会话类型。
- 若新增第三种快照语义，不宜继续扩展布尔值，因为 `true/false` 已形成跨 Go/Rust 的稳定契约。应先在 Go 对照、属性协议和 provider API 中设计可枚举的选择类型，并同步生产装配与独立测试。
- 修改 provider 泛型界限或返回所有权时，需核对 [`../sessionexpr/sessionctx.rs`](../sessionexpr/sessionctx.rs) 的生产闭包、`OptionalEvalPropProvider::as_any` 的 downcast 要求以及所有 `Arc::ptr_eq` 测试；避免把共享快照改成隐式深拷贝。
- 修改键或描述时，必须同步 [`../exprctx/optional.rs`](../exprctx/optional.rs) 的编号/描述表、[`optional.rs`](optional.rs) 的注册表行为以及 Go `optional.go`。键顺序影响固定槽位与位集合，属于兼容性风险。
- 回归测试应放在独立测试文件而不是 `infoschema.rs`：局部 provider/reader 边界扩展 [`optional_test.rs`](optional_test.rs)，迁移对齐扩展 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，完整生产装配扩展 [`../sessionexpr/migration_aster_unit_test.rs`](../sessionexpr/migration_aster_unit_test.rs)，Go 语义同步扩展 [`optional_test.go`](optional_test.go)。
- 性能上，本层每次读取包含一次按键查找、描述检查、`Any` downcast 和闭包调用；新增逻辑不应在这里扫描元数据或持有额外锁。正确性风险主要是颠倒布尔分支、请求错误泛型类型或漏报必需属性。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点，目标目录中的 `infoschema.rs` 含 11 个符号；`query` 定位 `InfoSchemaPropProvider`、`get_session_info_schema`、`get_latest_info_schema` 和该文件的 `required_optional_eval_props`。对这些泛型符号执行 `callers`/`callees` 未返回边，因此没有把“无图边”等同于“无调用”。
- 源码与装配：已读 [`infoschema.rs`](infoschema.rs)、[`lib.rs`](lib.rs)、[`optional.rs`](optional.rs)、[`../exprctx/optional.rs`](../exprctx/optional.rs)、[`../sessionexpr/sessionctx.rs`](../sessionexpr/sessionctx.rs) 和 [`Cargo.toml`](Cargo.toml)。这些文件共同证明公开导出、键定义、查找错误语义、生产注册方式和 crate 依赖边界。
- Rust 独立测试：已读 [`optional_test.rs`](optional_test.rs) 的 `verify_info_schema`、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `infoschema_and_kv_readers_choose_and_preserve_provider_objects`，以及 [`../sessionexpr/migration_aster_unit_test.rs`](../sessionexpr/migration_aster_unit_test.rs) 的 `migration_optional_props_privilege_and_sequence_match_go`。它们覆盖缺失 provider、两个布尔分支、对象身份保持和完整装配链。
- Go 对照：已读 [`infoschema.go`](infoschema.go) 与 [`optional_test.go`](optional_test.go) 的 `OptPropInfoSchema` 分支；另由精确搜索确认 Go 表达式消费者位于 `pkg/expression/builtin_info.go`/`builtin_info_vec.go`，而当前 Rust Reader 的直接引用集中在上述测试。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证命令及最终退出状态在任务交付时记录；人工复核重点为 11 个固定章节、真实符号/路径、无测试内嵌建议，以及对未被调用图覆盖事实的明确标注。

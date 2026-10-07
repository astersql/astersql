# `pkg/expression/expropt/sqlexec.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式可选求值属性机制中“受限 SQL 执行器”能力的适配层。模块入口 `pkg/expression/expropt/lib.rs` 以 `#[path = "sqlexec.rs"] mod sqlexec_provider` 装入本文件，并通过 `pub use sqlexec_provider::*` 对外再导出。它处在表达式层与会话/SQL 执行层之间：只定义表达式允许依赖的最小执行接口，并把具体执行器作为 `exprctx::OptPropSQLExecutor` 注册和读取，避免本叶子 crate 直接依赖完整 SQL、KV、行与结果字段实现。

本文件不是 SQL 引擎，不解析、规划或执行 SQL，也不管理事务。当前 Rust 生产接线 `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 会注册 `SQLExecutorPropProvider<C::SqlExecutor>`；`pkg/expression/util.rs::runFetchDigestQuery` 已直接以本文件的 `SQLExecutor` trait 约束执行 statements summary 查询。全仓 Rust 引用核验没有发现生产代码调用 `SQLExecutorPropReader::get_sql_executor`，该 Reader 的显式读取目前由独立测试覆盖。因此“执行接口已有生产消费者”和“可选属性 Reader 已在生产链读取”必须分开描述。

## 核心职责

1. `SQLExecutor` 用四个关联类型抽象上下文、执行选项、返回行和结果字段，只暴露 `exec_restricted_sql`，限制表达式代码可使用的 SQL 能力。
2. `SQLExecutorPropProvider<T>` 保存一个可失败的惰性工厂；读取属性时才构造或取得 `Arc<T>`，并原样传播工厂错误。
3. `OptionalEvalPropProvider` 实现把 Provider 固定绑定到 `exprctx::OptPropSQLExecutor`，同时通过 `Any` 暴露具体类型以支持安全向下转换。
4. `SQLExecutorPropReader` 通过 `RequireOptionalEvalProps` 声明 SQLExecutor 属性依赖，并经公共 `get_prop_provider` 校验键、存在性和具体泛型类型后调用 Provider。

泛型 `T` 必须实现 `SQLExecutor + Send + Sync + 'static`。关联类型则没有在本文件中附加 `Send`、`Sync` 或具体 crate 约束；实际消费者按需要进一步约束，例如 `runFetchDigestQuery` 要求 `Context = kv::Context`、`Row = chunk::Row`。因此不能从本文件单独断言所有执行器都使用同一种上下文、选项或行表示。

## 主要符号

- `pub trait SQLExecutor`：公开窄接口。关联类型 `Context`、`OptionFuncAlias`、`Row`、`ResultField` 分别对应 Go 的 `context.Context`、`sqlexec.OptionFuncAlias`、`chunk.Row` 和 `resolve.ResultField` 所占角色。
- `SQLExecutor::exec_restricted_sql(&self, ctx, opts, sql, args)`：同步执行入口；SQL 为借用字符串，参数是 `&[Box<dyn Any>]`，成功返回 `(Vec<Row>, Vec<ResultField>)`，失败返回 `anyhow::Error`。trait 本身不规定参数具体类型、结果顺序之外的 SQL 语义或事务边界。
- `pub struct SQLExecutorPropProvider<T>`：公开 Provider，私有字段为 `Box<dyn Fn() -> anyhow::Result<Arc<T>> + Send + Sync>`。它保存工厂而非独立执行器字段。
- `SQLExecutorPropProvider::new<F>(provider: F) -> Self`：装箱一个线程安全、`'static`、可重复调用的 `Fn`；构造时不调用工厂。
- `SQLExecutorPropProvider::call(&self) -> anyhow::Result<Arc<T>>`：同步调用工厂，不缓存、不包装错误。
- `impl OptionalEvalPropProvider for SQLExecutorPropProvider<T>`：`Desc` 返回 `OptPropSQLExecutor.Desc()`；`as_any` 返回 `Some(self)`，使公共 helper 能校验完整的 `SQLExecutorPropProvider<T>` 类型。
- `pub struct SQLExecutorPropReader`：无字段的公开 Reader，可直接以单元结构体值使用。
- `required_optional_eval_props(&self)`：返回 `OptPropSQLExecutor.AsPropKeySet()`。`pkg/expression/exprctx/optional.rs` 将该键定义为索引 5，因此结果只含 SQLExecutor 对应位。
- `get_sql_executor<T, C>(&self, ctx: &C) -> anyhow::Result<Arc<T>>`：公开读取入口；`C` 只需实现 `OptionalEvalPropContext`，完整 `exprctx::EvalContext` 由 blanket impl 自动满足。

本文件没有模块级常量、枚举、条件编译项、全局可变状态或内嵌测试模块。

## 执行流程

注册链如下：

1. `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 克隆会话引用为 `sql_session`。
2. 它构造 `SQLExecutorPropProvider::<C::SqlExecutor>::new(move || Ok(sql_session.restricted_sql_executor()))`；闭包捕获共享会话，并把会话返回的具体执行器包装成成功结果。
3. `EvalContext::set_optional_prop` 将 Provider 类型擦除后交给 `OptionalEvalPropProviders`；注册表依据 `provider.Desc().Key()` 写入 `OptPropSQLExecutor` 槽位，并禁止重复键。
4. `NewEvalContext` 完成全部可选属性注册后断言属性位集合为满集，因而标准会话求值上下文必须包含该 Provider。

读取链如下：

1. 使用者可先通过 `required_optional_eval_props` 把 SQLExecutor 位并入自己的需求集合。
2. `get_sql_executor::<T, _>(ctx)` 以 `OptPropSQLExecutor` 调用 `get_prop_provider::<SQLExecutorPropProvider<T>, _>`。
3. `pkg/expression/expropt/optional.rs::get_prop_provider` 依次检查槽位存在、Provider 自描述键一致，并以 `Any::downcast_ref` 检查具体类型（包括泛型参数 `T`）一致。
4. Reader 调用 `provider.call()`；工厂成功时返回 `Arc<T>`，失败时错误经 `?` 原样向上传播。
5. 拿到执行器后，调用者才会调用 `exec_restricted_sql`。例如 `pkg/expression/util.rs::runFetchDigestQuery` 构造 statements summary 查询、把 digest 装箱为 `Any` 参数、标记内部事务来源，再将执行错误转换为表达式层错误并把返回行整理成 digest 映射；这部分 SQL 行为不由 Provider/Reader 实现。

独立测试 `optional_test.rs::verify_sql_executor` 覆盖缺失属性、注册、Provider 直接调用、Reader 读取、同一 `Arc` 身份及两种工厂错误；`migration_aster_unit_test.rs::sql_executor_reader_preserves_success_and_provider_errors` 还通过取回的执行器实际调用 trait 方法。

## 数据与状态

本文件唯一的持久状态是 `SQLExecutorPropProvider<T>::provider` 闭包及其捕获环境。Provider 不缓存 `Arc<T>`，也不保证相邻两次 `call` 返回同一对象；当前会话接线每次调用 `restricted_sql_executor()`，测试工厂则克隆预先存在的 `Arc`，这些调用点返回同一底层对象是各自闭包的行为，而不是 Provider 类型的不变量。

`SQLExecutorPropReader` 是零大小、无状态类型。属性键和静态描述由 `pkg/expression/exprctx/optional.rs` 所有：`OptPropSQLExecutor` 的索引为 5、描述字符串为 `"OptPropSQLExecutor"`。Provider 的实际槽位由 `pkg/expression/expropt/optional.rs::OptionalEvalPropProviders` 持有，本文件不拥有注册表。

所有权以 `Arc<T>` 跨边界共享。成功读取会产生或取得一个强引用；闭包捕获资源、求值上下文与返回执行器的精确释放时点由外层所有权决定。`exec_restricted_sql` 的 `args` 切片借用装箱参数，只在调用期间有效；本 trait 没有声明实现是否复制或保留参数，实现者不得在缺少额外所有权的情况下越过调用生命周期保存这些借用。

## 依赖与调用关系

- crate 边界：`pkg/expression/expropt/Cargo.toml` 定义 `astersql-expression-expropt`，入口为 `lib.rs`，关闭自动测试发现与 doctest。清单直接依赖 `anyhow` 和路径依赖 `astersql-expression-exprctx`；本文件还使用标准库 `Any`、`Arc` 与 crate 内再导出的公共契约。
- 模块装配：`pkg/expression/expropt/lib.rs` 把本文件命名为内部模块 `sqlexec_provider` 并公开再导出所有公开符号。
- 上游注册：`pkg/expression/sessionexpr/sessionctx.rs::SessionContext` 以关联类型 `SqlExecutor` 绑定具体实现，`restricted_sql_executor` 返回 `Arc<Self::SqlExecutor>`；`NewEvalContext` 用它构造并注册 Provider。
- 下游公共逻辑：Reader 调用 `pkg/expression/expropt/optional.rs::get_prop_provider`；Provider 和 Reader 分别使用 `OptPropSQLExecutor.Desc()` 与 `AsPropKeySet()`。
- trait 的 Rust 生产消费者：`pkg/expression/util.rs::runFetchDigestQuery`、`RetrieveLocal` 和相邻全局检索流程以 `expropt::SQLExecutor` 约束实际执行器；这条边直接调用 trait，不经过 Reader。
- Reader 的 Rust 验证调用者：`pkg/expression/expropt/optional_test.rs`、`pkg/expression/expropt/migration_aster_unit_test.rs`、`pkg/expression/sessionexpr/migration_aster_unit_test.rs`。全仓检索还找到若干其他模块为测试或适配实现 `SQLExecutor`，但没有找到额外生产 Reader 读取点。
- RustCodeGraph 的 `explore` 将 `get_sql_executor` 的调用者识别为本文件自身及 `optional_test.rs::verify_sql_executor`，并把 `restricted_sql_executor` 的上游识别为 `NewEvalContext` 和统计模块工具；由于泛型与动态分派边覆盖有限，精确引用由 `rg` 补齐。

## 错误处理与边界

`get_sql_executor` 用 `?` 传播两层错误：公共属性查找错误与 Provider 工厂错误。公共 helper 明确区分三类查找失败：`OptPropSQLExecutor` 未注册、Provider 自描述键与请求键不一致、Provider 不能向下转换为请求的 `SQLExecutorPropProvider<T>`。错误的泛型 `T` 不会产生错误类型的 `Arc`，而会在安全 downcast 阶段失败。

边界条件包括：

- 属性缺失时不会调用工厂；两个 Rust 独立测试文件都验证错误文本包含 `not exists in EvalContext`。
- 工厂返回的任意 `anyhow::Error` 不被改写。测试分别用 `mockErr1`、`mockErr2` 验证直接调用和 Reader 调用的错误透传。
- 本文件不校验 SQL 文本、占位符数量、参数实际类型、选项组合或结果字段与行的对应关系；这些属于具体 `SQLExecutor` 实现及调用者契约。
- `exec_restricted_sql` 与 Provider 工厂均为同步调用；若实现阻塞或 panic，本层不设置超时、不捕获 panic，也不执行重试。
- trait 返回拥有所有权的两个 `Vec`，空结果是合法成功值；测试桩正是返回两个空向量。
- `Box<dyn Any>` 不要求 `Send` 或 `Sync`，所以不能仅凭执行器本身线程安全就假定一次调用的参数可跨线程转移。

## 并发与资源生命周期

Provider 的具体执行器 `T` 以及工厂闭包都要求 `Send + Sync + 'static`，所以 Provider 可作为线程安全的类型擦除属性被共享。闭包是 `Fn` 而非 `FnMut`/`FnOnce`，必须允许重复且共享调用；若需要可变状态，构造者必须通过互斥量、原子类型或其他线程安全内部可变性自行管理。

本文件不创建线程、异步任务、锁、通道、事务或取消令牌。`SQLExecutor::Context` 允许具体实现携带调用级取消/超时语义，但 trait 只借用它，不解释其内容。Provider 闭包若捕获 `Arc<Session>`，Provider 存活期间会延长会话捕获对象生命周期；每次返回的 `Arc<T>` 又独立延长执行器生命周期。是否形成引用环取决于外部 Session/Executor 结构，本文件没有 `Weak` 或显式关闭协议。

`call` 不做单例缓存，因此昂贵的执行器构造会在每次读取时重复发生；当前标准会话接线只返回已有共享执行器，通常成本是外层方法调用与 `Arc` 引用计数。真正的 SQL 执行、网络 I/O、锁与事务生命周期属于 `T::exec_restricted_sql` 的实现，不应归因于本适配文件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/expropt/sqlexec.go`：

- Go `SQLExecutor` 同样刻意只暴露 `ExecRestrictedSQL`，并用编译期断言确认完整的 `sqlexec.RestrictedSQLExecutor` 满足该窄接口。Rust 用 trait 与关联类型表达同一边界，但本文件没有针对某个完整执行器类型的等价编译期断言。
- Go 方法参数是 `context.Context`、`[]sqlexec.OptionFuncAlias`、`string`、`...any`，返回 `[]chunk.Row`、`[]*resolve.ResultField` 和 `error`。Rust 以四个关联类型、借用切片、`Box<dyn Any>` 和 `anyhow::Result` 保留角色对应关系，但不会自动提供 Go 可变参数或接口切片的完全相同表示。
- Go `type SQLExecutorPropProvider func() (SQLExecutor, error)` 对应 Rust 的泛型结构体加装箱闭包；Rust 额外以 `Arc<T>` 明确共享所有权，以 `Send + Sync + 'static` 明确 Provider 可共享边界。
- 两端 `Desc` 都绑定 `OptPropSQLExecutor`，Reader 都声明单一属性位，读取流程都是先经公共 helper 取 Provider，再调用 Provider。
- Go `optional_test.go` 对 SQLExecutor 的用例验证缺失错误、同一 mock 对象和 Provider/Reader 错误透传；Rust `optional_test.rs::verify_sql_executor` 保留相同测试意图，并以 `Arc::ptr_eq` 验证对象身份。

迁移差异是 Rust 把具体 SQL 相关类型推迟到关联类型和上层实例化，减少本 crate 依赖；代价是完整兼容性要由每个实现/调用点的类型约束证明。Go 与 Rust 的 digest 检索工具都直接接收窄接口执行器参数，而不是自行通过 Reader 取值；当前 Rust 生产代码另有会话 Provider 注册证据，但没有生产 Reader 读取点，因此不能把“已注册”写成“已有表达式通过 Reader 消费”。

## 扩展指南

- 新增需要受限 SQL 的表达式时，应组合 `SQLExecutorPropReader`，把 `required_optional_eval_props()` 合入表达式的需求集合，并从实际 `OptionalEvalPropContext` 调用 `get_sql_executor::<具体类型, _>`；不要绕过公共 helper 直接读取或 downcast 注册表。
- 若只是新增 SQL 查询行为，应放在消费模块，维持本 trait 的窄边界。给 `SQLExecutor` 增加方法会影响所有实现者，包括 `optional_test.rs`、迁移测试、sessionexpr 测试和统计模块适配，必须先核对 Go 接口增量。
- 调整参数或结果类型时，要同步检查 `pkg/expression/util.rs::runFetchDigestQuery` 对 `Context`、`Row` 和 `Any` 参数的假设，以及具体执行器是否需要 `ResultField` 的指针/共享语义。
- 若 Provider 需要缓存、重试或异步构造，应明确并发初始化、错误缓存、取消和资源释放语义；不能在 `call` 中静默吞错或无界重试。
- 测试逻辑必须继续放在独立文件。适配层变更至少同步 `pkg/expression/expropt/optional_test.rs` 与 `migration_aster_unit_test.rs`；会话注册变化同步 `pkg/expression/sessionexpr/migration_aster_unit_test.rs`；Go 对齐变化核对 `pkg/expression/expropt/optional_test.go`。
- 兼容风险集中在属性键、泛型具体类型与关联类型不一致；正确性风险包括参数类型/占位符契约和错误传播被改变；性能风险包括工厂重复调用、`Arc` 原子计数和动态 downcast。SQL 执行本身的 I/O 性能必须在具体实现与消费测试中评估。

## 验证依据

- 目标源码：`pkg/expression/expropt/sqlexec.rs`，完整核对 `SQLExecutor`、四个关联类型、`exec_restricted_sql`、`SQLExecutorPropProvider::{new, call}`、`OptionalEvalPropProvider` 实现、`SQLExecutorPropReader`、`required_optional_eval_props` 和 `get_sql_executor`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/expression/expropt` 确认目标、Go 对照及独立测试已索引；`explore "pkg/expression/expropt/sqlexec.rs SQLExecutor ExecOption GetSQLExecutor"`、`query SQLExecutor --kind trait`、Provider/Reader 查询和文件 `node` 用于核对定义、动态边界及调用上下文。
- 模块和公共契约：`pkg/expression/expropt/lib.rs`、`pkg/expression/expropt/optional.rs::get_prop_provider`、`pkg/expression/exprctx/optional.rs` 中索引 5 的 `OptPropSQLExecutor` 与描述表。
- crate 清单：`pkg/expression/expropt/Cargo.toml`，核对 crate 名、入口、测试设置、`anyhow` 与 `exprctx-crate` 依赖及 Go 包移植元数据。
- Rust 生产接线与消费：`pkg/expression/sessionexpr/sessionctx.rs::SessionContext`、`NewEvalContext`；`pkg/expression/util.rs::runFetchDigestQuery`、`RetrieveLocal` 及其相邻全局检索逻辑。
- Rust 独立测试：`pkg/expression/expropt/optional_test.rs::verify_sql_executor`、`pkg/expression/expropt/migration_aster_unit_test.rs::registry_and_missing_reader_paths_match_go`、`sql_executor_reader_preserves_success_and_provider_errors`，以及 `pkg/expression/sessionexpr/migration_aster_unit_test.rs` 对标准会话注册后对象身份的验证。
- Go 对照与测试：`pkg/expression/expropt/sqlexec.go`、`pkg/expression/expropt/optional_test.go` 的 `OptPropSQLExecutor` 分支；两者用于核对窄接口、Reader、对象身份和错误透传意图。
- 全仓引用核验：`rg -n "SQLExecutorPropReader|SQLExecutorPropProvider|get_sql_executor|OptPropSQLExecutor|exec_restricted_sql" --glob '*.rs' pkg/expression pkg/statistics pkg/ttl`，用于补足 RustCodeGraph 对泛型与动态分派调用边的覆盖限制。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的结构命令，确认本文恰有十一个固定二级章节，并人工复核未把会话注册、trait 直接消费或 Go Reader 调用误写成 Rust Reader 已有生产调用。

# `pkg/ddl/systable/schema_loader.rs`

## 文件定位

本文件位于 `astersql-ddl-systable` crate，定义 DDL 调度侧触发 InfoSchema 重载时使用的最小公共抽象。模块入口 `pkg/ddl/systable/lib.rs` 通过 `pub mod schema_loader` 和 `pub use schema_loader::*` 导出它，DDL 主 crate 又在 `pkg/ddl/lib.rs` 中重新导出 `SchemaLoader` 与 `SchemaLoaderError`，因此调用者通常可以从 `astersql_ddl` 使用这两个类型。

尽管文件位于 `systable` 目录，它本身不查询 `mysql.*` 系统表，也不实现 InfoSchema 的读取或缓存替换；这些工作由 trait 的具体实现完成。本文件是 DDL owner/调度逻辑与 Domain、InfoSchema syncer 等真实加载设施之间的依赖倒置边界。按 DDL 执行模型分类，它服务于 owner 接管时的元数据刷新保护，不创建或持久化 DDL job，不推进 schema state，不执行 reorg/backfill，也不直接更新 schema version 或 schema diff。

## 核心职责

1. 以 `SchemaLoader::reload` 统一表达“从元数据来源重新加载当前模式”的同步操作。
2. 通过 `SchemaLoader: Send + Sync` 约束实现可安全放入跨线程共享的 trait object，例如 `Arc<dyn SchemaLoader>`。
3. 以 `SchemaLoaderError` 保留实现方转换后的诊断文本，并提供标准的 `Display` 与 `std::error::Error` 接口。
4. 保持调度器只依赖能力接口：重试策略、取消策略、owner 任期以及真正的加载算法均不进入本文件。

直接证据是 `pkg/ddl/job_scheduler.rs::must_reload_schemas_with`：它只通过该 trait 调用 `loader.reload()`，成功即返回，失败后检查取消条件并按传入间隔重试。生产接线之一 `pkg/session/runtime/normal_ddl_service.rs::DomainSchemaLoader` 则把 `Domain::reload` 的结果转换为本文件的错误类型。

## 主要符号

- `pub struct SchemaLoaderError(String)`：重载失败的统一错误。内部字符串字段私有，外部只能通过构造器创建、通过 `Display`/`Error` 观察；派生 `Clone`、`Debug`、`Eq`、`PartialEq`，便于传播、诊断和精确测试。
- `SchemaLoaderError::new(message: impl Into<String>) -> Self`：接受 `String`、`&str` 等可转换输入，原样保存实现方提供的诊断文本，不附加错误分类、来源链或重试标记。
- `impl fmt::Display for SchemaLoaderError`：把内部字符串直接写入 formatter，因此用户可见文本就是构造时的消息。
- `impl std::error::Error for SchemaLoaderError`：接入 Rust 标准错误生态；未覆盖 `source`，所以该类型本身不保留结构化底层错误链。
- `pub trait SchemaLoader: Send + Sync`：公开、对象安全的线程共享能力接口，无关联类型、泛型方法或生命周期参数。
- `fn reload(&self) -> Result<(), SchemaLoaderError>`：唯一方法。共享借用表明接口不要求调用者独占加载器；`Ok(())` 只表示实现报告本次重载成功，`Err` 交由上层决定重试或退出。

本文件没有模块级常量、条件编译项、异步函数、锁、通道或具体 `SchemaLoader` 实现。

## 执行流程

当前 normal DDL owner 主链可概括为：

1. `pkg/session/runtime/normal_ddl_service.rs` 在 owner epoch 变化时创建新的 `JobScheduler`。
2. 在加载持久化 job 队列之前，它调用 `JobScheduler::must_reload_schemas_with(schema_loader.as_ref(), 200ms, cancellation_predicate)`。
3. `must_reload_schemas_with` 调用本 trait 的 `reload`。成功时结束重载阶段；失败时检查 runtime 已取消、已失去 owner 身份或 epoch 已变化，未取消才等待后重试。
4. 注入的 `DomainSchemaLoader::reload` 将弱引用升级为 `Domain`，调用 `Domain::reload`，丢弃成功返回值中的额外数据，并将错误文本包装成 `SchemaLoaderError`。另一个实现 `NormalSchemaRuntime::reload` 委托给 `syncer.ReloadWithContext`。
5. 只有调用方确认重载阶段结束且 owner 任期仍有效后，才继续 owner 调度流程。

因此本文件只覆盖第 3、4 步之间的调用契约。循环次数、休眠、取消检查、owner 状态和后续队列调度都不属于 `SchemaLoader::reload` 的语义。

## 数据与状态

`SchemaLoaderError` 唯一持久字段是一个拥有所有权的 `String`，没有错误码、时间戳、schema version 或重试次数。其值语义由派生的 `Clone`/`Eq` 支持，适合测试和跨层返回，但调用方若需要按类型区分可重试错误，必须扩展错误模型，而不能可靠解析任意消息文本。

`SchemaLoader` 自身没有数据字段。具体状态由实现持有：例如 `DomainSchemaLoader` 保存 `Weak<Domain>`，避免加载器反向强持有 Domain；`NormalSchemaRuntime` 使用自身的 syncer 与 context；测试用 `SequenceLoader` 以 `Mutex<Vec<Result<...>>>` 提供确定的调用序列。trait 使用 `&self` 并不承诺实现无状态，若实现需要更新内部状态，必须采用锁、原子量或其他内部可变性机制。

## 依赖与调用关系

- 直接语言依赖：只有 `std::fmt`；本文件不直接使用 `pkg/ddl/systable/Cargo.toml` 中的 `meta-model` 依赖。
- crate 导出：`pkg/ddl/systable/lib.rs` 导出本模块；`pkg/ddl/lib.rs` 再导出两个公开类型。
- 配置入口：`pkg/ddl/options.rs::with_schema_loader` 把 `Arc<dyn SchemaLoader>` 保存到 `Options::schema_loader`。`pkg/ddl/options_test.rs` 与 `pkg/ddl/schema_loader_contract_aster_unit_test.rs` 验证对象身份和生产 trait object 注入。
- 核心调用者：`pkg/ddl/job_scheduler.rs::must_reload_schemas_with` 调用 `reload` 并实现重试/取消；`pkg/session/runtime/normal_ddl_service.rs` 在 owner epoch 切换路径中调用该调度器方法。
- 生产实现：`pkg/session/runtime/normal_ddl_service.rs::DomainSchemaLoader` 委托 `Domain::reload`；同文件 `NormalSchemaRuntime` 委托 `syncer.ReloadWithContext`。
- 测试替身：`pkg/ddl/mock/schema_loader_mock.rs::MockSchemaLoader` 经控制器消费预设的 `Reload` 结果，并将 mock 错误或错误返回类型转换为 `SchemaLoaderError`。

RustCodeGraph 的 `query` 将目标 trait、错误类型、`with_schema_loader`、`must_reload_schemas_with`、上述实现及相关独立测试关联起来；`node` 进一步确认 `must_reload_schemas_with` 的泛型约束为 `L: SchemaLoader + ?Sized`，允许传入 trait object 引用。

## 错误处理与边界

- `reload` 只有成功/失败二态，不区分暂态、永久错误、取消或 owner 丢失。当前调度调用方把任意 `Err` 视为可重试，直到外部取消谓词成立。
- `SchemaLoaderError::new` 不修改消息；`Display` 也不添加上下文。实现方应在转换底层错误时提供足够定位问题的文本。
- `DomainSchemaLoader` 的弱引用无法升级时返回 `"normal Domain is closed"`；底层 `Domain::reload` 错误通过 `to_string()` 转换，结构化来源会丢失。
- 本接口没有 context 参数，取消不能中断一个已经进入的 `reload` 调用；取消只可由具体实现自己的 context，或由调用方在一次失败返回之后检查。
- trait 没有规定幂等性，但现有 owner 重试路径可能多次调用 `reload`。安全实现必须能承受重复调用，并确保失败不会把可见 InfoSchema 留在无法判断的半更新状态。
- `Ok(())` 没有携带已加载版本。调用方依赖实现对“最新且可用”的定义；若将来需要验证特定 schema version，应扩展返回契约并同步所有实现和测试。

## 并发与资源生命周期

`Send + Sync` 是本文件唯一显式并发保证：加载器可跨线程转移并通过共享引用调用。它不保证多个 `reload` 会被串行化，也不规定重入行为；实现若不能并发加载，必须在内部同步或由更高层保证单调用。

常见持有方式是 `Arc<dyn SchemaLoader>`，所以 trait object 生命周期可独立于某个调度器借用。`DomainSchemaLoader` 特意使用 `Weak<Domain>`：Domain 关闭后不会因加载器形成强引用环，而是返回错误。normal owner 调用方把 owner epoch、取消 context 和 200ms 重试间隔放在接口之外，使加载器不会拥有调度线程、sleep、取消令牌或 worker 生命周期。

该文件不创建线程、不 spawn task、不打开事务、不持有系统表 session，也没有显式清理逻辑。若新增实现持有这些资源，应由实现自己的 `Drop`/关闭协议管理，不应假定本 trait 会代为关闭。

## 与 Go 版本的对应关系

Go 原型位于 `pkg/ddl/ddl.go`：`type SchemaLoader interface { Reload() error }`，并注明当前实现来自 Domain；`pkg/ddl/options.go::WithSchemaLoader` 将其注入 DDL `Options`。Rust 保留了相同的单方法、无参数、同步成功/失败契约，并通过 `pkg/ddl/options.rs::with_schema_loader` 对齐 functional option 接线。

Rust 的显式差异包括：方法名采用 `reload`；错误收敛为 `SchemaLoaderError`，而不是任意 Go `error`；trait 额外要求 `Send + Sync`；生产选项使用 `Arc<dyn SchemaLoader>` 表达共享所有权。

Go `pkg/ddl/job_scheduler.go::mustReloadSchemas` 在 owner 开始调度前持续调用 `Reload`，失败则记录日志并等待重试或 context 取消；`pkg/ddl/job_scheduler_test.go::TestMustReloadSchemas` 覆盖直接成功、失败后成功，以及取消后不能发布 storage-class-transition readiness。Rust `must_reload_schemas_with` 已对齐成功、失败重试和失败后取消的基本控制流，相关独立测试位于 `pkg/ddl/job_scheduler_test.rs`。但 readiness channel 的发布逻辑不在本 trait 中，Rust 这项独立测试也不声称覆盖 Go 测试的 readiness 断言；扩展 owner 接管行为时应在调度层核对，而不是塞入本接口。

## 扩展指南

- 新增加载器实现：实现 `SchemaLoader::reload`，把底层错误转换为含上下文的 `SchemaLoaderError`，保证重复调用安全，并审查是否能满足 `Send + Sync`。实现及测试应放在独立源文件与独立 `*_test.rs` 文件中，不要把测试内嵌到本文件。
- 修改接口签名：同步更新 `DomainSchemaLoader`、`NormalSchemaRuntime`、`MockSchemaLoader`、`SequenceLoader`、`TestSchemaLoader`，以及 `with_schema_loader` 的 trait object 接线。应优先扩展现有类型，而不是绕开 trait 让调度器依赖具体 Domain。
- 增加错误分类：最可能修改 `SchemaLoaderError`；同时调整 mock 的错误映射和调度器的重试判定。兼容风险是现有调用方目前默认所有错误均可重试。
- 增加取消或超时：由于当前调用是同步且无 context 参数，只修改外层重试循环不能中断正在执行的加载；需要一起设计 trait 契约和各具体实现。并发风险是实现持锁期间阻塞 owner 接管。
- 增加版本确认：若返回已加载 schema version，必须核对 Domain/syncer 的真实返回语义，以及 Go `Reload() error` 的兼容策略，避免仅凭 `Ok(())` 推断目标版本。
- 最小同步测试集合：`pkg/ddl/job_scheduler_test.rs` 验证重试/取消，`pkg/ddl/schema_loader_contract_aster_unit_test.rs` 验证生产注入，`pkg/ddl/options_test.rs` 验证选项保存，`pkg/ddl/mock/mock_aster_unit_test.rs` 验证 mock 的生产 trait 与错误转换；生产实现变化还应在 `pkg/session/runtime/normal_ddl_*_test.rs` 中选择对应 owner/runtime 场景。

## 验证依据

- 目标源码：`pkg/ddl/systable/schema_loader.rs`，确认全部公开符号、签名、派生 trait 和标准错误实现。
- crate/模块边界：`pkg/ddl/systable/Cargo.toml`、`pkg/ddl/systable/lib.rs`、`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、根 `Cargo.toml`。
- RustCodeGraph：`status` 显示索引包含目标 Rust 文件；`explore "pkg/ddl/systable/schema_loader.rs SchemaLoader SchemaLoaderError reload callers implementations"`；`query SchemaLoader`、`query SchemaLoaderError`；`node SchemaLoader`、`node SchemaLoaderError`、`node must_reload_schemas_with`。图查询确认目标定义、再导出、调度调用点、实现和测试候选；同名歧义再用路径限定的源码搜索收窄。
- Rust 调用与实现：`pkg/ddl/options.rs`、`pkg/ddl/job_scheduler.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/ddl/mock/schema_loader_mock.rs`。
- 独立 Rust 测试：`pkg/ddl/job_scheduler_test.rs`、`pkg/ddl/options_test.rs`、`pkg/ddl/schema_loader_contract_aster_unit_test.rs`、`pkg/ddl/mock/mock_aster_unit_test.rs`。
- Go 对照：`pkg/ddl/ddl.go::SchemaLoader`、`pkg/ddl/options.go::WithSchemaLoader`、`pkg/ddl/job_scheduler.go::mustReloadSchemas`、`pkg/ddl/job_scheduler_test.go::TestMustReloadSchemas`、`pkg/domain/domain.go` 的 DDL 构造注入。
- 人工边界复核：本文没有把 trait 描述成加载算法或系统表访问实现；没有声称它处理 job 持久化、schema state、backfill、版本同步或取消中断；Go readiness 行为被明确留在调度层。
- 结构验证使用任务规定的命令，只检查目标文档存在且恰好包含十一个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。

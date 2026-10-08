# `pkg/planner/plannersession/context.rs` 逻辑说明

## 文件定位

`context.rs` 是 Cargo crate `astersql-planner-plannersession` 的唯一业务实现文件，由同目录 `lib.rs` 以 `mod context; pub use context::*;` 全量导出。该 crate 在根 `Cargo.toml` 以 `facade_planner_plannersession` 注册，并被 `pkg/session/Cargo.toml` 直接依赖；其职责是保存会话级规划扩展状态，而不是执行具体的优化规则。

当前 Rust 接线需要分成两层理解：`pkg/session/contextimpl.rs::newPlanContextImpl` 会构造本文件的 `PlanCtxExtended`，`pkg/session/contextimpl_test.rs` 也直接验证了这条路径；但生产规划上下文 `pkg/session/runtime/planning.rs` 当前直接实现 `astersql_planner_planctx::PlanContext`，没有持有或委托给本文件的 `PlanCtxExtended`。因此，本文件已经是可编译、可测试的会话扩展组件，却尚不能等同于 Rust 生产规划主链的唯一实现。

同目录没有 `doc.go`，因此没有额外的包级行为契约需要合并。文件没有模块级常量、类型别名、枚举或条件编译项；公开项通过 `lib.rs` 直接暴露。

## 核心职责

本文件集中提供三组能力：

1. 用 `NullRejectCheckExprContext` 包装普通 `ExprContext`，通过 `IsInNullRejectCheck()` 标识表达式正处在 null-reject 分析路径。
2. 用 `PlanCtxExtended` 保存构造时的会话引用、固定的 null-reject 表达式上下文，以及一次规划期间使用的只读用户变量名集合。
3. 通过 `SessionContext` 将事务预热委托回具体会话，并用 `PlannerSessionError` 作为该窄接口的错误类型。

这些职责对应 Go 文件 `pkg/planner/plannersession/context.go` 的 `PlanCtxExtended`，但 Rust 当前为了保持 crate 自包含，在本文件内定义了自己的 `ExprContext`、`SessionContext`、null-reject 包装器和错误类型。它们不是 `pkg/expression/exprctx/context.rs` 与 `pkg/planner/planctx/context.rs` 中生产 trait 的类型别名。

## 主要符号

- `ExprContext: Send + Sync`：空标记 trait，代表本文件所接受的表达式上下文。`Send + Sync` 约束允许其放入跨线程共享的 `Arc<dyn ExprContext>`，但 trait 本身未暴露求值 API。
- `NullRejectCheckExprContext`：持有私有字段 `ExprContext: Arc<dyn ExprContext>`。它实现 `Clone`，克隆时只增加 `Arc` 引用计数；`ExprContext()` 返回底层 `Arc` 的克隆，`IsInNullRejectCheck()` 恒为 `true`。
- `WithNullRejectCheck(ctx)`：创建上述包装器，不复制底层上下文状态。
- `PlannerSessionError { pub Message: String }`：可克隆、可比较的轻量错误；`new` 接受任意 `Into<String>`，并实现 `Display` 与 `std::error::Error`。
- `SessionContext: Send + Sync`：要求实现者提供 `GetExprCtx()` 和 `AdviseTxnWarmup()`。前者每次返回一个 `Arc<dyn ExprContext>`，后者返回 `Result<(), PlannerSessionError>`。
- `PlanCtxExtended`：核心状态对象。`sctx` 与 `nullRejectCheckExprCtx` 在构造后不可由公开 API 替换，`readonlyUserVars` 可设置与清除。
- `NewPlanCtxExtended(sctx)`：公开构造函数。它先读取一次 `sctx.GetExprCtx()`，再创建固定的 null-reject 包装器；只读变量状态初始为 `None`。
- `PlanCtxExtended::GetNullRejectCheckExprCtx()`：返回已缓存包装器的借用；debug 构建中会校验当前会话表达式上下文仍与构造时的同一 `Arc` 分配相等。
- `PlanCtxExtended::AdviseTxnWarmup()`：原样返回 `sctx.AdviseTxnWarmup()` 的结果。
- `SetReadonlyUserVarMap`、`GetReadonlyUserVarMap`、`Reset`：分别替换整个集合、借用查看集合、清空为 `None`。键类型为 `String`，值为单位类型 `()`，因此该映射实际表达集合语义。

公开 API 保留了 Go 风格的大写命名；`lib.rs` 通过 `#![allow(dead_code, non_snake_case, non_upper_case_globals)]` 接受这些迁移期命名。

## 执行流程

构造流程由 `pkg/session/contextimpl.rs::newPlanContextImpl` 发起：调用者传入 `Arc<dyn SessionContext>`，该函数克隆 `Arc` 后调用 `NewPlanCtxExtended`。构造函数调用一次 `SessionContext::GetExprCtx`，交给 `WithNullRejectCheck`，最终把会话、包装器和空的只读变量状态组合成 `PlanCtxExtended`。RustCodeGraph 的直接调用边为 `newPlanContextImpl -> NewPlanCtxExtended -> {GetExprCtx, WithNullRejectCheck}`。

读取 null-reject 上下文时，`GetNullRejectCheckExprCtx` 在 debug 构建先再次调用 `GetExprCtx` 并用 `Arc::ptr_eq` 检查身份，然后返回构造时缓存的包装器；调用者再可通过 `IsInNullRejectCheck()` 识别模式，或通过 `ExprContext()` 取得同一底层上下文的共享所有权。

事务预热流程没有本地状态变更：`PlanCtxExtended::AdviseTxnWarmup` 直接调用会话 trait 方法并传回成功或错误。`pkg/session/contextimpl_test.rs::new_plan_context_impl_preserves_session_and_builds_real_extension` 用 `AtomicUsize` 验证委托恰能触发测试会话的预热计数。

只读变量流程是“整表替换—借用查询—语句后清空”：`SetReadonlyUserVarMap` 将 `Option` 设为 `Some(map)`，`GetReadonlyUserVarMap` 返回 `Option<&HashMap<...>>`，`Reset` 设回 `None`。该测试同时验证键可见以及重置后返回 `None`。

## 数据与状态

`PlanCtxExtended` 的稳定状态是 `sctx: Arc<dyn SessionContext>` 与 `nullRejectCheckExprCtx`。两者在构造时绑定；后续 API 只借用或克隆内部 `Arc`，不会替换底层对象。debug 身份断言形成一个重要不变量：同一 `SessionContext` 的 `GetExprCtx()` 应持续返回指向同一分配的 `Arc`。如果实现者每次重新分配一个语义相同的上下文，debug 构建仍会失败。

`readonlyUserVars: Option<HashMap<String, ()>>` 是易变的局部规划状态。`None` 表示尚未设置或已经重置；`Some(empty_map)` 表示调用者明确设置了空集合，这两个状态在 Rust API 上可区分。设置操作按值接收并取得整个 `HashMap` 的所有权，不与调用者共享后续修改；读取仅返回与 `&self` 生命周期绑定的不可变借用。

`PlannerSessionError` 仅包含文本，没有错误码、来源链或结构化上下文。错误比较按消息字符串进行。`NullRejectCheckExprContext` 的 `Clone` 与 `ExprContext()` 都共享底层对象，不克隆表达式求值状态。

## 依赖与调用关系

本文件的实际编译依赖只有标准库的 `HashMap`、`fmt` 和 `Arc`。`pkg/planner/plannersession/Cargo.toml` 将 Go 对应包涉及的 expression、planctx、sessionctx、sessiontxn 与 intest crate 全部放在 `[target.'cfg(any())'.dependencies]` 下；`cfg(any())` 恒为假，所以这些条目只记录迁移边界，不进入当前构建。这也解释了本文件为何定义本地 trait 和包装器。

已确认的 Rust 上游只有生产文件 `pkg/session/contextimpl.rs::newPlanContextImpl`；RustCodeGraph 对 `NewPlanCtxExtended` 返回这一条直接 caller。该 `planContextImpl` 保存原会话和扩展对象，测试位于 `pkg/session/contextimpl_test.rs`。根 workspace 与 `pkg/session/Cargo.toml` 提供 crate 级接线。

下游直接调用包括 `NewPlanCtxExtended -> WithNullRejectCheck`、`NewPlanCtxExtended -> SessionContext::GetExprCtx`、`GetNullRejectCheckExprCtx -> SessionContext::GetExprCtx`，以及 `PlanCtxExtended::AdviseTxnWarmup -> SessionContext::AdviseTxnWarmup`。集合方法只操作本地 `Option<HashMap<...>>`。

生产规划中的相似能力目前走另一组类型：`pkg/planner/planctx/context.rs::PlanContext` 使用 `astersql_expression_exprctx::ExprContext`，`pkg/session/runtime/planning.rs` 直接实现该 trait；例如 `pkg/planner/util/null_misc.rs::nullRejectFoldCtx` 调用的是生产 `PlanContext::GetNullRejectCheckExprCtx`。这些调用不能视为本文件方法的 caller。当前 runtime 实现还直接返回普通 `self.expression`，没有使用本文件或 expression crate 的 null-reject 包装器，这是现有迁移状态而非本文件已经完成的行为。

## 错误处理与边界

只有事务预热路径返回显式错误。`PlanCtxExtended::AdviseTxnWarmup` 不包装、不吞掉错误，测试调用者使用 `unwrap()` 只验证成功分支；当前独立 Rust 测试没有覆盖 `PlannerSessionError` 的失败透传、`Display` 或相等性。

`GetNullRejectCheckExprCtx` 的一致性检查使用 `debug_assert!`：debug 构建发现 `Arc` 身份改变会 panic，release 构建完全省略检查并继续返回旧的缓存包装器。因此它是开发期不变量检查，不是可恢复的生产错误处理。该检查只比较分配身份，不比较内部状态。

`SetReadonlyUserVarMap` 接受空表且不会报错；`Reset` 可重复调用；`GetReadonlyUserVarMap` 在未设置和重置后都返回 `None`。文件没有容量限制、变量名规范化或大小写处理，这些策略应由上游负责。

空 trait `ExprContext` 只保证线程安全标记，无法承载生产表达式上下文的求值操作。扩展本文件时不能据此假设它已经满足 `astersql_expression_exprctx::ExprContext` 或 `astersql_planner_planctx::PlanContext`。

## 并发与资源生命周期

会话与表达式上下文都由 `Arc` 管理。构造 `PlanCtxExtended` 会增加会话的强引用计数；包装表达式上下文不会深拷贝，`NullRejectCheckExprContext::clone` 和 `ExprContext()` 继续增加同一表达式对象的强引用计数。对象在最后一个强引用释放时销毁，本文件没有显式 `Drop`、后台任务、锁、通道、事务句柄或异步代码。

`ExprContext` 与 `SessionContext` 要求 `Send + Sync`，所以 trait 对象可以安全共享；但 `PlanCtxExtended` 的只读变量变更方法需要 `&mut self`，文件本身没有内部锁，也没有提供多个线程并发修改集合的机制。若上层把整个对象放入锁中，并发策略由上层锁负责。

事务“预热”的资源生命周期完全由 `SessionContext::AdviseTxnWarmup` 实现管理。本文件既不缓存预热结果，也不负责提交、回滚或清理。测试中的 `AtomicUsize` 只用于观察委托发生，不代表生产事务资源的具体实现。

## 与 Go 版本的对应关系

Rust `PlanCtxExtended` 的三个字段与 `pkg/planner/plannersession/context.go::PlanCtxExtended` 一一对应：会话、构造时固定的 null-reject 上下文、只读用户变量集合。构造、getter/setter 和 `Reset` 的总体意图也一致；Go 的 `nil map` 对应 Rust 的 `None`，Go `map[string]struct{}` 对应 Rust `HashMap<String, ()>`。

存在四个必须保留的差异：

1. Go 直接使用 `sessionctx.Context`、`exprctx.NullRejectCheckExprContext`、`planctx.PlanContext` 与 `sessiontxn.GetTxnManager`；Rust 当前使用本文件内的窄 trait、包装器与错误，Cargo 中对应 crate 依赖均被 `cfg(any())` 禁用。
2. Go `AdviseTxnWarmup` 明确执行 `sessiontxn.GetTxnManager(ctx.sctx).AdviseWarmup()`；Rust 把整个动作抽象为 `SessionContext::AdviseTxnWarmup`，因此事务管理器查找必须由 trait 实现者完成。现有测试只提供计数器实现。
3. Go 的 `intest.AssertFunc` 属于测试/断言设施；Rust 使用 `debug_assert!(Arc::ptr_eq(...))`。二者都维护“构造后表达式上下文身份不变”的意图，但启用条件和失败表现不完全相同。
4. Go 的 `PlanCtxExtended` 通过嵌入参与 `planctx.PlanContext` 的编译期接口检查；Rust 本文件没有实现 `astersql_planner_planctx::PlanContext`，而且本地 `ExprContext` 类型与生产 expression trait 不兼容。Rust 生产 `runtime/planning.rs` 目前另行直接实现生产 trait。

Go 主链证据包括：`pkg/planner/core/preprocess.go::Preprocess` 设置只读变量集合，`pkg/expression/sessionexpr/sessionctx.go::ExprContext.IsReadonlyUserVar` 读取集合，`pkg/planner/optimize.go` 在绑定处理后调用 `AdviseTxnWarmup`，`pkg/planner/util/null_misc.go` 使用 null-reject 上下文。不能据这些 Go caller 推断 Rust 对应路径已经接通。

## 扩展指南

若只扩展这个兼容组件，应优先修改对应的窄接口和状态入口：新的会话委托能力加入 `SessionContext` 与 `PlanCtxExtended`；新的语句级状态加入 `PlanCtxExtended`，并明确初始化和 `Reset` 语义；null-reject 行为变化则集中在 `NullRejectCheckExprContext`、`WithNullRejectCheck` 与 `GetNullRejectCheckExprCtx`。每次新增可变语句状态都必须同步清理逻辑，避免跨语句泄漏。

若目标是接入生产规划主链，不能只修改本文件。需要先解决本地 trait 与 `astersql-expression-exprctx`、`astersql-planner-planctx`、session/transaction 实现之间的类型桥接，再评估是否启用 Cargo 中当前禁用的依赖；同时更新 `pkg/session/runtime/planning.rs` 的 `PlanContext` 实现。直接让两个同名 `ExprContext` 混用会造成类型不兼容，错误类型也需要从 `PlannerSessionError` 映射到生产 `GoError`。

测试应优先扩展独立文件 `pkg/session/contextimpl_test.rs`，不要把测试嵌入 `context.rs`。最低覆盖建议包括：预热错误原样透传、`Some(empty_map)` 与 `None` 的区别、重复 `Reset`、底层 `Arc` 身份保持，以及在可控 debug 测试中验证替换表达式上下文会触发不变量。若生产接线发生变化，还应在使用真实 `PlanContext` 的 session/planner 测试中验证 `pkg/planner/util/null_misc.rs`、只读变量收集/查询和预热调用链。

兼容风险主要来自公开 Go 风格符号和状态语义变化；正确性风险来自缓存表达式上下文与会话当前上下文不一致；性能风险较低，当前操作主要是 `Arc` 克隆和 `HashMap` 所有权移动，但频繁复制整个只读变量集合或在热路径增加锁会改变成本。任何接线修改还需防止形成 planner、expression、session 之间的 Cargo 循环依赖。

## 验证依据

事实核验使用了本地 RustCodeGraph 索引（11467 个文件、307296 个节点、1848419 条边），读取并查询了 `pkg/planner/plannersession/context.rs` 的完整 141 行、主要符号，以及 `NewPlanCtxExtended`、`GetNullRejectCheckExprCtx`、`AdviseTxnWarmup` 等调用关系。关键图证据是 `pkg/session/contextimpl.rs::newPlanContextImpl` 为 `NewPlanCtxExtended` 的直接调用者，构造函数直接调用 `WithNullRejectCheck` 和 `SessionContext::GetExprCtx`；图中未发现 `GetNullRejectCheckExprCtx` 的生产 caller。

crate 与装配证据来自 `pkg/planner/plannersession/Cargo.toml`、`pkg/planner/plannersession/lib.rs`、根 `Cargo.toml`、`pkg/session/Cargo.toml`、`pkg/session/contextimpl.rs` 和 `pkg/session/runtime/planning.rs`。Go 语义对照读取了 `pkg/planner/plannersession/context.go`、`pkg/session/contextimpl.go`、`pkg/planner/core/preprocess.go`、`pkg/expression/sessionexpr/sessionctx.go`、`pkg/planner/optimize.go` 与 `pkg/planner/util/null_misc.go`。生产 Rust trait/使用点核验了 `pkg/planner/planctx/context.rs` 和 `pkg/planner/util/null_misc.rs`。

直接相关的独立 Rust 测试是 `pkg/session/contextimpl_test.rs::new_plan_context_impl_preserves_session_and_builds_real_extension`，覆盖会话 `Arc` 身份、null-reject 标记、事务预热委托、只读变量设置/读取和重置。`rg` 未找到同目录测试或直接针对 Go `PlanCtxExtended` 的独立测试；`pkg/planner/planctx/migration_aster_unit_test.rs` 测试的是另一个 `EmptyPlanContextExtended`，不能作为本文件实现的直接覆盖。

本任务是纯文档分析，按计划不运行 Cargo。结构验收以目标文件存在且恰好包含规定的十一个二级章节为准；人工复核同时确认文档区分了本文件当前行为、Go 对照行为与尚未完成的生产接线。

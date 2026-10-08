# `pkg/session/contextimpl.rs`

## 文件定位

`contextimpl.rs` 位于 `astersql-session` crate 的会话层，crate 根 `pkg/session/lib.rs` 通过 `pub mod contextimpl;` 公开它。它是一个很薄的规划上下文装配层：接收实现 `astersql_planner_plannersession::SessionContext` 的会话对象，同时保存原始会话引用和由该引用派生的 `PlanCtxExtended`。

当前 Rust 接线仍是局部移植状态。仓库对 `newPlanContextImpl` 的 Rust 引用只有 `pkg/session/contextimpl_test.rs`；尚未发现 Rust 生产会话构造路径调用它。相对地，Go 同路径实现由 `pkg/session/session.go` 的会话字段和初始化流程实际使用。因此，本文件当前不能被描述为已经进入 Rust SQL 请求主链。

## 核心职责

- `planContextImpl` 把同一个逻辑会话同时暴露为原始 `SessionContext` trait object 和规划器扩展状态，形成会话层与 `pkg/planner/plannersession/context.rs` 之间的适配边界。
- `newPlanContextImpl` 保证扩展上下文由传入的同一会话引用构造，而不是由另一个会话或独立表达式上下文构造。
- 本文件本身不执行 SQL 解析、优化或执行，也不直接实现空值拒绝检查、事务预热或只读用户变量管理；这些行为属于字段 `PlanCtxExtended` 的实现。
- 本文件没有实现 `pkg/planner/planctx/context.rs` 中的完整 `PlanContext` trait。该 trait 还要求 `Common` 与 `TableLockReadContext` 等能力，所以这里目前只是组合载体，不是已验证可传入所有规划器入口的完整 Rust 计划上下文。

## 主要符号

### `pub struct planContextImpl`

结构体有两个公开字段：

- `session: Arc<dyn SessionContext>`：会话能力的共享所有权引用。`SessionContext` 要求实现者同时满足 `Send + Sync`，并提供 `GetExprCtx` 与 `AdviseTxnWarmup`。
- `plan_ctx_extended: PlanCtxExtended`：基于上述会话构造的规划扩展状态，包含 null-reject 表达式上下文、对事务预热的委托，以及一次规划期间的只读用户变量集合。

类型名保留 Go 风格，文件级 `#![allow(dead_code, non_camel_case_types, non_snake_case)]` 允许尚未接线的符号和移植命名存在。结构体没有 `Clone`、`Default` 或自定义析构实现。

### `pub fn newPlanContextImpl(session: Arc<dyn SessionContext>) -> planContextImpl`

这是本文件唯一的函数和构造入口。它先以 `Arc::clone(&session)` 调用 `NewPlanCtxExtended`，然后把原始 `Arc` 移入返回结构体。函数直接返回值，不返回 `Result`，也不分配 `Box`。

## 执行流程

1. 调用者先将具体会话包装或转换为 `Arc<dyn SessionContext>`。
2. `newPlanContextImpl` 克隆该 `Arc`；克隆只增加强引用计数，不复制会话对象。
3. 克隆出的引用传给 `NewPlanCtxExtended`。
4. `NewPlanCtxExtended` 调用会话的 `GetExprCtx`，再通过 `WithNullRejectCheck` 固定一个 null-reject 包装；只读用户变量初始为 `None`。
5. 构造函数返回 `planContextImpl { session, plan_ctx_extended }`，两个字段最终指向或派生自同一个会话。
6. 后续调用必须显式经 `plan_ctx_extended` 字段访问扩展能力，例如 `GetNullRejectCheckExprCtx`、`AdviseTxnWarmup`、`SetReadonlyUserVarMap`、`GetReadonlyUserVarMap` 与 `Reset`；本文件没有转发方法。

RustCodeGraph 给出的直接调用边为 `newPlanContextImpl -> NewPlanCtxExtended`，并记录该函数实例化 `planContextImpl`。下游 `NewPlanCtxExtended` 的图边继续到 `SessionContext::GetExprCtx` 和 `WithNullRejectCheck`。RustCodeGraph 和仓库文本引用均未找到 Rust 生产调用者。

## 数据与状态

`planContextImpl` 自身只持有两个字段，不维护缓存、事务或锁。关键不变量是两条会话关联的一致性：`session` 保存调用者传入的 `Arc`，而 `plan_ctx_extended` 使用该 `Arc` 的克隆构造。`pkg/session/contextimpl_test.rs` 用 `Arc::ptr_eq` 验证返回对象的 `session` 与输入 trait object 指向同一分配。

扩展状态位于 `PlanCtxExtended` 内部：

- 构造时从会话取得一个 `Arc<dyn ExprContext>`，包装为 `NullRejectCheckExprContext`。
- `readonlyUserVars` 初始为 `None`；设置后保存 `HashMap<String, ()>`，`Reset` 将其恢复为 `None`。
- `GetNullRejectCheckExprCtx` 在 debug 构建中用 `Arc::ptr_eq` 断言会话返回的表达式上下文自构造后未被替换。若调用方替换它，debug 构建会触发断言，release 构建不会执行该检查。

由于 `session` 和 `plan_ctx_extended` 都持有会话的强引用，构造完成后通常至少新增两个指向同一会话对象的 `Arc` 所有权；销毁 `planContextImpl` 时两个字段依次释放各自持有的资源。

## 依赖与调用关系

- crate 归属：`pkg/session/Cargo.toml` 定义包 `astersql-session`，库入口为 `pkg/session/lib.rs`；该 crate 声明本地路径依赖 `astersql-planner-plannersession = ../planner/plannersession`。
- 标准库依赖：`std::sync::Arc` 提供跨所有者共享的会话引用。
- 规划器依赖：`SessionContext` 规定本适配器所需的最小会话能力；`NewPlanCtxExtended` 和 `PlanCtxExtended` 提供实际扩展行为。
- Rust 上游：当前仅 `pkg/session/contextimpl_test.rs::new_plan_context_impl_preserves_session_and_builds_real_extension` 调用构造函数；`pkg/session/lib.rs` 只完成模块公开，不等价于运行时调用。
- Go 上游对照：`pkg/session/session.go` 的 `session.pctx` 字段保存 `*planContextImpl`，会话初始化在建立 `sessionVars` 与 `exprctx` 后调用 `newPlanContextImpl(s)`；`pkg/session/bootstrap_test.go` 的手工 bootstrap 会话也执行相同装配顺序。
- 下游：`PlanCtxExtended::AdviseTxnWarmup` 委托回同一个 `SessionContext`；null-reject 与只读变量行为均由 `pkg/planner/plannersession/context.rs` 承担。

## 错误处理与边界

构造函数没有可恢复错误分支：`GetExprCtx` 的 trait 签名直接返回 `Arc`，所以创建扩展上下文不会传播 `Result`。事务预热错误只会在以后调用 `PlanCtxExtended::AdviseTxnWarmup` 时以 `PlannerSessionError` 返回，本文件不捕获或改写它。

主要边界如下：

- 输入不能是空引用；Rust 的 `Arc<dyn SessionContext>` 类型不表达 `nil`。具体实现必须遵守 `GetExprCtx` 始终返回有效对象的契约。
- 表达式上下文在扩展对象存活期间应保持身份不变；当前只在 debug 构建通过断言检测。
- 字段公开意味着调用者可以分别移动、替换或直接操作扩展状态；本文件没有额外封装来强制更强不变量。
- 尚无 Rust `PlanContext` trait 实现，也没有生产调用者证据。扩展时不能仅凭类型名假定该对象已经满足规划器的全部上下文接口。
- 文件无条件编译，没有 `cfg` 分支；`nextgen` feature 也没有在此文件中改变行为。

## 并发与资源生命周期

`SessionContext: Send + Sync` 允许 trait object 跨线程共享，`Arc` 的引用计数操作也是线程安全的；但这只保证引用与 trait 边界可跨线程，不代表 `planContextImpl` 的全部可变状态可无锁并发修改。

`PlanCtxExtended::SetReadonlyUserVarMap` 和 `Reset` 需要 `&mut self`，因此安全 Rust 会要求调用者独占访问扩展对象。该类型内部没有 `Mutex`、`RwLock`、原子状态、通道或后台任务。测试中的 `AtomicUsize` 只属于 `TestSession`，用于观察委托次数，不是生产实现的一部分。

资源生命周期由所有权自然管理：构造函数克隆一次会话 `Arc` 给扩展对象并保留原引用；当外层对象销毁后，两份强引用都被释放。若会话实现反向强持有此对象，调用者需要自行避免 `Arc` 引用环；本文件没有 `Weak` 或显式关闭流程。

## 与 Go 版本的对应关系

Rust 文件对应 `pkg/session/contextimpl.go`，保留了 `planContextImpl`、`newPlanContextImpl` 和“会话 + `PlanCtxExtended`”的基本组合意图，但并非一比一等价：

- Go 结构体匿名嵌入 `*session` 与 `*plannersession.PlanCtxExtended`，方法会被提升；Rust 使用两个具名公开字段，没有自动方法提升，调用者必须显式访问 `plan_ctx_extended`。
- Go 参数和返回值是具体 `*session` / `*planContextImpl`；Rust 接收 `Arc<dyn SessionContext>` 并按值返回结构体，以共享 trait object 代替具体会话指针。
- Go 文件通过 `var _ planctx.PlanContext = &planContextImpl{}` 编译期确认完整接口实现；Rust 文件没有对应的 `impl PlanContext for planContextImpl`，不能宣称已有同等编译期保证。
- Go 注释说明嵌入具体 `session` 是为了代码库中的强制类型转换安全；Rust trait object 不提供 Go 式嵌入或类型断言布局语义，因此当前实现只保留会话引用，未复刻该转换保证。
- Go 生产路径在 `pkg/session/session.go` 初始化 `pctx`；Rust 仓库目前只由独立测试构造此类型。这是当前最重要的迁移状态差异。
- 两边都要求先有表达式上下文再创建规划扩展。Go 主初始化顺序为 `sessionVars -> exprctx -> pctx -> tblctx`，bootstrap 测试也先建立 `exprctx` 再建立 `pctx`；Rust 测试通过预置 `expr_context` 的 `TestSession` 体现同一前置条件。

## 扩展指南

- 若要把该适配器接入 Rust 生产会话，先找到 Rust 会话的真实构造点，确保表达式上下文已经初始化，再调用 `newPlanContextImpl`；应新增或扩展同目录独立测试文件，不能把测试写回 `contextimpl.rs`。
- 若目标是使其真正实现 `pkg/planner/planctx/context.rs::PlanContext`，需要逐项核对父 trait `Common`、`TableLockReadContext` 及所有方法，而不是只转发 `PlanCtxExtended` 的五个扩展方法。此工作明显超出本文件当前薄适配器的既有实现，必须以 Go 接口实现和 Rust 现有会话能力为依据单独设计。
- 新增扩展状态时，优先修改 `pkg/planner/plannersession/context.rs` 的 `PlanCtxExtended`，并同步其独立测试；只有“会话与扩展如何组合”的变化才应修改本文件。
- 修改构造流程时要保持“同一会话引用构造两个字段”的不变量，并在 `pkg/session/contextimpl_test.rs` 中继续使用身份比较和可观察委托验证，避免只检查类型可编译。
- 涉及表达式上下文替换时，要处理 `GetNullRejectCheckExprCtx` 的身份断言和旧包装失效风险；涉及只读变量时，要验证 `Reset` 的语句生命周期清理；涉及事务预热时，要同时覆盖成功和 `PlannerSessionError` 传播。
- 性能方面，当前构造只增加 `Arc` 引用计数并建立小型扩展对象；不要在这个热路径无证据地加入深拷贝、锁或额外全局状态。

## 验证依据

- 目标源码：`pkg/session/contextimpl.rs`，确认文件只有一个结构体、一个构造函数、两个字段，无条件编译项和错误分支。
- crate 边界：`pkg/session/Cargo.toml` 的 `[package]`、`[lib]`、`[features]` 与 `astersql-planner-plannersession` 路径依赖；`pkg/session/lib.rs` 的 `pub mod contextimpl` 和独立 `contextimpl_test` 声明。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`query` 定位 Rust/Go 同名符号；`callees newPlanContextImpl` 确认 Rust 构造函数调用 `NewPlanCtxExtended` 并实例化 `planContextImpl`；`callees NewPlanCtxExtended` 确认其调用 `GetExprCtx` 与 `WithNullRejectCheck`。`callers` 未给出 Rust 生产调用边。
- 文本引用复核：`rg` 对非 Markdown 文件的全仓引用只找到 Rust 源文件与 `pkg/session/contextimpl_test.rs`；Go 引用位于 `pkg/session/session.go`、`pkg/session/bootstrap_test.go` 和 `pkg/session/contextimpl.go`。
- Rust 测试：`pkg/session/contextimpl_test.rs::new_plan_context_impl_preserves_session_and_builds_real_extension` 验证会话指针身份、null-reject 标记、事务预热委托一次、只读变量写入与 `Reset` 清理。
- Go 对照：`pkg/session/contextimpl.go` 验证匿名嵌入和接口断言设计；`pkg/session/session.go` 验证生产初始化位置；`pkg/session/bootstrap_test.go` 验证测试构造顺序。
- 下游契约：`pkg/planner/plannersession/context.rs` 定义 `SessionContext`、`PlanCtxExtended`、构造与扩展方法；`pkg/planner/planctx/context.rs` 定义完整 `PlanContext` trait，支持判断当前 Rust 适配器尚未实现完整接口。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证命令及人工复核结果在任务交付时记录。

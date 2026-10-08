# `pkg/planner/planctx/context.rs`

## 文件定位

本文件是 `astersql-planner-planctx` crate 的核心契约文件：它不执行优化算法，而是把规划器需要的会话、元数据、KV、表达式、范围构建和内部 SQL 服务收敛成 `Common` / `PlanContext` trait，并定义物理计划转为 tipb 执行器时使用的 `BuildPBContext`。crate 根 `pkg/planner/planctx/lib.rs` 私有声明 `context` 模块后以 `pub use context::*` 对外再导出这些定义。

`pkg/planner/planctx/Cargo.toml` 将该 crate 直接连接到 `util/context`、`expression/exprctx`、`infoschema/context`、`kv`、`meta/model`、`util/ranger/context`、`session/sessmgr`、`util/sqlexec`、`lock/context` 和 `sessionctx/variable`。因此它位于“会话提供能力，规划器消费能力”的接口边界，而不是具体会话或具体优化器实现中。

Rust 主规划链还存在一个更窄、可作为 trait object 保存的适配接口 `pkg/planner/core/base/plan_base.rs::PlanContext`。该接口的注释明确说明：本文件的 `planctx::PlanContext` 继承了带关联类型的 `Common`，不便直接放入计划节点使用的 `Arc<dyn ...>`，所以 core/base 会抹去 InfoSchema 细节，只保留算子实际消费的能力。`pkg/session/runtime/planning.rs::SessionPlanContext` 当前实现并返回的是这个 core/base 接口，并通过 `GetBuildPBCtx` 暴露本文件的 `BuildPBContext`。

## 核心职责

1. `Common` 声明规划期和会话期共享的服务入口，包括 KV storage/client、三种 InfoSchema 视图、会话变量、表达式与 ranger 上下文、SQL executor、事务、脏表判断和内建函数用量计数。
2. `PlanContext` 在 `Common` 与 `tablelock::TableLockReadContext` 之上增加规划专用能力：null-reject 表达式上下文、事务预热、只读用户变量集合以及局部状态重置。
3. `EmptyPlanContextExtended` 为不关心上述扩展行为的 mock 提供与 Go 一致的空操作方法，避免测试替身重复编写无意义状态。
4. `BuildPBContext` 聚合表达式下推和 tipb 构建所需的共享对象与标量配置，并提供 getter 以及替换表达式上下文的浅分离操作 `Detach`。
5. `GoError`、`BuildContextRef`、`ClientRef` 和 `WarnAppenderRef` 将 Go 的接口值/错误边界映射为 Rust trait object、`Arc` 和 `Option`，集中约束跨 crate API 的所有权形态。

## 主要符号

- `pub type GoError = Box<dyn Error + Send + Sync>`：用于 `Txn` 与 `AdviseTxnWarmup` 的跨模块错误边界；错误值可在线程间转移和共享。
- `BuildContextRef = Arc<dyn exprctx::BuildContext>`：共享表达式构建上下文。类型本身未额外声明 `Send + Sync`，不可仅凭 `Arc` 推断可跨线程使用。
- `ClientRef = Arc<dyn kv::Client + Send + Sync>` 与 `WarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>`：可共享的客户端和警告追加器；字段再由 `Option` 表达 Go 接口的 nil。
- `trait Common: ValueStoreContext`：包含关联类型 `InfoSchemaContext`、`InfoSchemaError`。`GetInfoSchema`、`GetLatestInfoSchema`、`GetLatestISWithoutSessExt` 返回使用这两个关联类型的 `MetaOnlyInfoSchema` trait object；`GetSQLExecutor` 和 `GetRestrictedSQLExecutor` 需要 `&mut self`，表达执行器访问可能修改上下文；`Txn(active)` 在 `active=true` 时必须等待 pending transaction 变为有效；`UpdateColStatsUsage` 接受惰性迭代器；`BuiltinFunctionUsageInc` 的实现必须保证底层计数线程安全。
- `trait PlanContext: Common + TableLockReadContext`：`GetNullRejectCheckExprCtx` 必须提供 null-reject 模式的表达式上下文；`SetReadonlyUserVarMap` / `GetReadonlyUserVarMap` / `Reset` 管理一次规划过程的扩展状态；`AdviseTxnWarmup` 把预热失败向上传播。
- `struct EmptyPlanContextExtended`：零大小类型。其 `AdviseTxnWarmup` 恒为 `Ok(())`，setter 丢弃输入，getter 恒为 `None`，`Reset` 不做任何事。它只提供同名方法，并未在本文件中自行实现整个 `PlanContext` trait。
- `struct BuildPBContext`：`ExprCtx` 是必需字段；`Client`、`WarnHandler`、`ExtraWarnghandler` 可空；其余字段分别控制 TiFlash 快扫、细粒度 shuffle 批大小、`GROUP_CONCAT` 上限和 EXPLAIN 模式。`ExtraWarnghandler` 保留了 Go 字段的既有拼写。
- `BuildPBContext::{GetExprCtx, GetClient}`：克隆 `Arc`/`Option<Arc>`，不深拷贝底层对象。
- `BuildPBContext::Detach(static_expr_ctx) -> Box<Self>`：克隆整个结构体，只把 `ExprCtx` 换成调用方给出的静态上下文，并以独立堆对象返回。

## 执行流程

接口层自身没有单一运行入口；典型数据流由实际调用者组成：

1. 会话在规划前构造上下文。`pkg/session/runtime/planning.rs::plan_context_with_params_and_explain` 创建表达式上下文和 ranger 上下文，并用会话参数填充 `BuildPBContext`；其中客户端使用 `SessionPushdownCapabilityClient`，`GroupConcatMaxLen` 来自会话变量默认定义，EXPLAIN 标志由入口参数传入。
2. `SessionPlanContext::GetBuildPBCtx` 返回该结构的共享只读引用。计划节点通过 core/base 的 `PlanContext` 适配层取得它，而无须依赖具体 session 类型。
3. 下推路径 `pkg/planner/util/misc.rs::GetPushDownCtx` 调用 `GetBuildPBCtx`，随后 `GetPushDownCtxFromBuildPBContext` 读取 `ExprCtx`、`Client`、警告处理器、EXPLAIN 标志和 `GroupConcatMaxLen`，组装 `expression::PushDownContext`，供表达式编码/下推使用。
4. 需要脱离可变会话表达式环境时，调用 `BuildPBContext::Detach`。该方法执行结构体浅克隆、替换 `ExprCtx`、返回新 `Box`；原对象不被修改。
5. 规划过程中的其他能力通过 `Common`/`PlanContext` 方法由实现者提供。例如统计估算读取 `GetSessionVars`，范围生成读取 `GetRangerCtx`，null-reject 推导使用专用 `GetNullRejectCheckExprCtx`，而事务路径可用 `Txn(true)` 或 `AdviseTxnWarmup` 触发等待/预热。

## 数据与状态

`Common` 和 `PlanContext` 只定义借用式能力，不拥有具体状态。状态属于实现者：InfoSchema 快照、会话变量、当前事务、只读用户变量映射以及 SQL executor 的生命周期均不在本文件管理。三种 InfoSchema getter 的语义不可互换：语句/事务绑定视图、包含会话扩展的最新视图、排除会话扩展的最新视图分别由不同方法表达；Go 文件还将 `GetInfoSchema` 标为语义含混的 deprecated 接口，新增调用应优先选择意图明确的最新视图或事务管理器路径。

`BuildPBContext` 是可克隆的配置快照，但其克隆是浅克隆：四类 `Arc` 字段仍共享底层对象，布尔值和整数按值复制。`Detach` 的唯一强制替换项是 `ExprCtx`，因此它不等价于深复制整个规划会话。`Client=None` 与两个 warning handler 的 `None` 都是合法状态，消费者必须保持可选语义。

`EmptyPlanContextExtended` 故意不保存只读用户变量：调用 `SetReadonlyUserVarMap` 后 `GetReadonlyUserVarMap` 仍为 `None`。它适用于不观察扩展状态的 mock，不能替代需要验证变量只读性或事务预热副作用的真实实现。

## 依赖与调用关系

上游方面，`pkg/planner/planctx/lib.rs` 导出本文件。`pkg/planner/cardinality/lib.rs::CardinalityContext` 为统计估算另建对象安全子集，并为实现本文件 `PlanContext` 的类型提供 blanket implementation；cardinality 源文件中的本地 `planctx::PlanContext` 实际是该子集的再导出，不能误认成本文件同名 trait。`pkg/util/ranger/types.rs` 则对满足本文件 `planctx::PlanContext + Send + Sync` 的类型提供 `RangeRebuildContext` blanket implementation。大量 planner、executor 和 session Cargo manifest 直接依赖 `astersql-planner-planctx`，说明该 crate 是公共边界而非内部细节。

当前 Rust 应用主链还通过 `pkg/planner/core/base/plan_base.rs::PlanContext` 这一适配层消费本文件类型。`pkg/session/runtime/planning.rs::SessionPlanContext` 提供 `GetSessionVars`、`GetExprCtx`、`GetRangerCtx`、`GetNullRejectCheckExprCtx`、`GetBuildPBCtx` 和线程安全的内建函数计数；物理算子持有 `ContextRef`，再经这些 getter 读取会话能力。这个适配关系必须与“本文件的完整 Go 对齐接口”区分，不能把两套同名 trait 当作同一符号。

下游方面，trait 方法依赖十个本地 crate 所导出的接口；`BuildPBContext` 的直接消费点包括 `pkg/planner/util/misc.rs::GetPushDownCtxFromBuildPBContext`。RustCodeGraph 对目标文件报告 35 个符号、107 个使用文件，并将 `SCtx`/计划上下文相关访问关联到大量逻辑计划、物理计划、代价估算和编码路径；通用方法名存在跨 Go/Rust 的同名歧义，因此具体边以目标文件、上述 session 构造点和 util 下推点为准。

## 错误处理与边界

本文件不创建业务错误。`Txn` 与 `AdviseTxnWarmup` 通过 `GoError` 透传动态错误；`Common::InfoSchemaError` 留给实现者选择，文件本身不规定错误枚举或转换策略。`EmptyPlanContextExtended::AdviseTxnWarmup` 总是成功，这只是 mock 的空行为，不能证明真实事务预热不会失败。

trait getter 多数返回引用，调用方必须遵循实现对象生命周期；`GetSQLExecutor`/`GetRestrictedSQLExecutor` 的可变借用同时阻止并行持有另一个可变借用。`GetSessionManager`、`GetReadonlyUserVarMap`、KV client 和 warning handler 明确可空；缺失不是错误。相对 Go，Rust 的 `Txn` 返回 `Box<dyn Transaction>`，类型层面保证成功结果非空，但事务仍可能处于 pending 或 invalid 状态，`active` 参数决定是否等待有效化。

边界风险主要有三类：误用三种 InfoSchema 视图造成元数据时序错误；把 `Detach` 当深复制而并发复用仍共享的 statement/warning/client 状态；给 `EmptyPlanContextExtended` 增加有状态预期却没有同步真实实现。新增错误分支时应保留 Go 的触发条件与传播语义。

## 并发与资源生命周期

`ClientRef`、`WarnAppenderRef` 和 `GoError` 显式要求相应 trait object 具备 `Send + Sync`；`Common::BuiltinFunctionUsageInc` 的文档也要求实现线程安全。与此相反，`Common`/`PlanContext` trait 自身没有全局 `Send + Sync` 上界，`BuildContextRef` 也没有附加该上界，所以不能从接口声明推导整个上下文可跨线程共享。

`Arc::clone` 只延长底层 client、表达式上下文和 warning handler 的生命周期；最后一个 `Arc` 释放时资源才释放。`Detach` 返回的新 `Box<BuildPBContext>` 拥有独立外壳，但其中未替换的 `Arc` 仍与原对象共享。源码注释和 Go 对照都特别说明：分离后会话上下文与新上下文可并行使用，并不意味着同一个 `StatementContext` 可并行复用；会话执行另一条语句前必须创建新的 `StatementContext`。

`UpdateColStatsUsage` 借用一个可变迭代器并逐项消费，保留 Go `iter.Seq` 的惰性语义，避免强制收集整个集合。事务、SQL executor、InfoSchema 与 session manager 的实际关闭、锁和取消机制均由实现者负责，本文件没有后台任务、通道、锁或析构逻辑。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/planctx/context.go`。Rust 保留了 `Common`、`PlanContext`、`EmptyPlanContextExtended`、`BuildPBContext` 以及方法/字段的 Go 命名和主要语义；`Detach` 都是“复制结构体并只替换 ExprCtx”。`pkg/planner/planctx/context_test.go::TestContextDetach` 与 Rust `context_test.rs::test_context_detach` 都验证新外壳、标量字段相等以及 ExprCtx/Client/warning handler 的接口身份语义；Rust `migration_aster_unit_test.rs` 进一步用两个不同表达式上下文验证替换确实发生，并验证空扩展的 no-op 行为。

主要语言映射差异如下：Go interface 映射为 Rust trait object；可能为 nil 的接口字段映射为 `Option<Arc<...>>`；map 映射为 `HashMap<String, ()>`；`iter.Seq` 映射为 `dyn Iterator<Item = TableItemID>`；Go `error` 映射为线程安全的 boxed dynamic error；结构体指针返回映射为引用或 `Box`。Rust 的 InfoSchema context/error 使用关联类型显式化，导致完整 `PlanContext` 不适合直接作为未指定关联类型的通用 trait object，这也是 core/base 另设窄接口的依据。

Go `BuildPBContext` 注释说明 pushdown 字段暂时不能直接嵌入 `expression.PushDownContext`，因为 expression 已依赖 planctx；Rust 仍保留同一字段布局和依赖方向。Go 对 `GetInfoSchema` 的 deprecated 说明没有通过 Rust `#[deprecated]` 属性表达；这是迁移层面的接口提示差异，不能据此认为 Rust 调用该方法是首选路径。

## 扩展指南

新增规划上下文能力前，先判断它属于完整会话契约还是算子实际需要的对象安全子集。若加入 `Common`/`PlanContext`，需同步 Go 同路径接口、所有 Rust 实现者/适配器以及 mock；若物理计划节点也要调用它，还需同步 `pkg/planner/core/base/plan_base.rs::PlanContext` 和 `SessionPlanContext`。带关联类型或泛型的方法会进一步影响对象安全，设计时应显式评估。

新增 tipb/pushdown 配置字段时，应修改 `BuildPBContext`、`pkg/session/runtime/planning.rs` 的构造点、`pkg/planner/util/misc.rs::GetPushDownCtxFromBuildPBContext` 的转换逻辑，并扩展独立测试 `context_test.rs` 或 `migration_aster_unit_test.rs`。如果字段是 `Arc`/句柄，必须决定 `Detach` 应共享还是替换，并分别断言对象身份；如果字段关联 `StatementContext`，必须重新审视并发限制。不要把 Rust 测试嵌入生产源文件。

修改 `EmptyPlanContextExtended` 时应保持其用途明确：无状态 mock 可以继续 no-op，需要真实状态的测试替身应实现自己的存储和 trait，而不是悄悄改变所有 mock 的默认行为。重命名 `ExtraWarnghandler` 会破坏 Go 字段对齐和现有构造点，除非安排完整兼容迁移，不应仅为拼写进行局部更改。

性能上，getter 应继续只借用或克隆 `Arc`，避免在规划热路径深拷贝表达式上下文、InfoSchema 或 client；`UpdateColStatsUsage` 应保留流式消费。兼容性上要重点验证 nil/`None`、pending transaction、临时表 InfoSchema 可见性和 `Detach` 后的共享身份。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/planner/planctx` 确认目标、Go 对照、两个独立 Rust 测试和 crate 根均已索引；`node --file pkg/planner/planctx/context.rs` 读取完整 204 行并报告 35 个符号、107 个使用文件；`query`/`explore` 核对 `Common`、`PlanContext`、`BuildPBContext`、`EmptyPlanContextExtended` 与计划上下文使用面。通用名称的 callers/callees 查询无精确输出且 explore 命中歧义，随后用精确文本搜索补足直接引用。
- 生产源码：`pkg/planner/planctx/context.rs`、`pkg/planner/planctx/lib.rs`、`pkg/planner/core/base/plan_base.rs`、`pkg/planner/cardinality/lib.rs`、`pkg/session/runtime/planning.rs`、`pkg/planner/util/misc.rs`；抽样物理计划访问见 `pkg/planner/core/operator/physicalop/physical_projection.rs`。
- crate 边界：`pkg/planner/planctx/Cargo.toml`，包括十个直接依赖、`exprstatic` dev-dependency，以及 `go-package = "pkg/planner/planctx"` 的移植元数据。
- Go 对照：`pkg/planner/planctx/context.go` 与 `pkg/planner/planctx/context_test.go`。
- Rust 独立测试：`pkg/planner/planctx/context_test.rs` 验证 Detach 的浅克隆和指针身份；`pkg/planner/planctx/migration_aster_unit_test.rs` 验证 ExprCtx 替换、其余 Arc 共享及空扩展 no-op。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核本文未把 core/base 同名 trait、mock 空行为或浅克隆描述成更强的运行时保证。

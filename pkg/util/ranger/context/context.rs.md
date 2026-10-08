# `pkg/util/ranger/context/context.rs`

对应源码：[context.rs](./context.rs)

## 文件定位

本文件定义 range 构造阶段的上下文对象 `RangerContext<'a>`，位于独立 crate `astersql-util-ranger-context` 中。crate 根 `pkg/util/ranger/context/lib.rs` 通过 `pub use context::*` 导出它；`pkg/planner/core/base/plan_base.rs` 又将其别名为规划器侧的 `RangerContext<'a>`。因此它是规划器、统计估算和 `pkg/util/ranger` 之间传递类型转换规则、表达式上下文、计划缓存状态和 range 回退通知的边界对象，而不是 range 算法本身。

`pkg/util/ranger/context/Cargo.toml` 声明该 crate 对应 Go 包 `pkg/util/ranger/context`，直接依赖 `astersql-util-context`、`astersql-errctx`、`astersql-expression-exprctx`、`astersql-expression-exprstatic` 和 `astersql-types`。本文件没有条件编译项；测试由 `lib.rs` 在 `#[cfg(test)]` 下从独立文件装配。

## 核心职责

1. `RangerContext` 聚合 range 构造所需的语句类型上下文、错误级别、表达式构建上下文、优化器 fix-control、缓存开关和 NULL/前缀索引策略。
2. `SetSkipPlanCache` 将 range 构造发现的缓存不安全原因转交给可选的 `PlanCacheTracker`；未配置 tracker 时保持无副作用。
3. `RecordRangeFallback` 将 range 大小超限事件转交给可选的 `RangeFallbackHandler`；后者负责禁用计划缓存并在自己的生命周期内最多报告一次容量告警。
4. `Detach` 生成脱离会话表达式上下文的副本：替换 `ExprCtx`、独立复制 `OptimizerFixControl`，同时保留类型/错误上下文、标志和两个 handler 的身份。

这些职责由 `pkg/util/ranger/ranger.rs` 和 `pkg/util/ranger/detacher.rs` 的真实调用验证：类型转换异常调用 `SetSkipPlanCache`，range 回退调用 `RecordRangeFallback`，谓词可能因参数变化而过度优化时也调用 `SetSkipPlanCache`。

## 主要符号

- `pub type BuildContextRef = Arc<dyn exprctx::BuildContext>`：共享的表达式构建上下文接口值。`Arc` 允许复制句柄；类型别名本身没有额外声明 `Send + Sync` 约束。
- `pub struct RangerContext<'a>`：公开数据结构，九个字段均公开。
  - `TypeCtx: types::Context`：Datum 转换、比较及语句类型标志；例如 `Range::IsPoint` 使用它比较上下界。
  - `ErrCtx: errctx::Context`：错误分组的处理级别；由上层 range/表达式逻辑消费，本文件只保存和复制。
  - `ExprCtx: BuildContextRef`：表达式求值/构建服务；`detacher.rs` 用它判断参数化表达式是否可能被过度优化。
  - `RangeFallbackHandler: Option<&'a contextutil::RangeFallbackHandler<'a>>`：借用的回退处理器。
  - `PlanCacheTracker: Option<&'a contextutil::PlanCacheTracker>`：借用的计划缓存跟踪器。
  - `OptimizerFixControl: HashMap<u64, String>`：按 fix ID 保存的配置值。
  - `UseCache`、`RegardNULLAsPoint`、`OptPrefixIndexSingleScan`：分别控制 range 缓存、NULL 点范围判断和前缀索引单次扫描优化。
- `impl Clone for RangerContext<'_>`：复制值字段和 map，克隆 `TypeCtx`/`ErrCtx`/`Arc`，并原样复制两个借用；它不复制 handler 的内部状态。
- `SetSkipPlanCache(&self, reason: &str)`：若 tracker 存在则向下转发原因。
- `RecordRangeFallback(&self, rangeMaxSize: i64)`：若 fallback handler 存在则向下转发容量上限。
- `Detach(&self, staticExprCtx: BuildContextRef) -> Box<RangerContext<'a>>`：返回堆分配的上下文副本，并把表达式上下文替换为调用者提供的静态版本。

## 执行流程

典型 range 构造链如下：上层会话或规划上下文构造 `RangerContext`，规划器通过 `GetRangerCtx` 获取引用，再将它传入 `pkg/util/ranger/ranger.rs`、`detacher.rs`、`points.rs` 和 `types.rs`。这些模块读取 `TypeCtx`、`ExprCtx` 和策略标志来构造、合并或判断范围。

当 `ranger.rs::convertPointInPlace` 的 Datum 类型转换报错时，它先以“错误 + 原值”为原因调用 `SetSkipPlanCache`，再按具体 TiDB/MySQL 错误决定容忍还是返回错误。当 `ranger.rs::buildColumnRange` 因 `rangeMaxSize` 回退到较粗范围时，它调用 `RecordRangeFallback`，并把原 access conditions 作为 remained conditions 交回上层，保证后续过滤仍保留精确语义。`detacher.rs` 在参数可能被覆盖、使原谓词折叠结果不稳定时，也通过 `SetSkipPlanCache` 阻止复用风险计划。

`Detach` 的步骤是：先调用 `clone()` 获得字段副本；再用参数 `staticExprCtx` 覆盖副本的 `ExprCtx`；最后显式再次克隆 `OptimizerFixControl`，固定与 Go `maps.Clone` 一致的独立存储语义；返回 `Box<RangerContext>`。它不执行表达式求值，也不调用 handler。

## 数据与状态

`RangerContext` 同时包含快照值与共享状态句柄。`OptimizerFixControl` 和三个布尔开关属于上下文本地状态；clone/detach 后对 map 的修改互不影响。`TypeCtx`、`ErrCtx` 按各自 `Clone` 实现复制。`ExprCtx` 是 `Arc`，普通 clone 共享同一对象，只有 `Detach` 才明确换入调用者给定的对象。

两个可选 handler 是借用而非所有权字段，生命周期 `'a` 防止上下文比 handler 活得更久。复制或 detach 仍指向相同的 `PlanCacheTracker` 和 `RangeFallbackHandler`，所以它们观察到同一份缓存状态、告警追加器和“一次性回退告警”状态。`Option::None` 是合法配置；例如 `pkg/session/runtime/planning.rs` 的一个实际构造路径将两者设为 `None`，此时转发方法是 no-op。

`UseCache` 是 `RangerContext` 自身的 range 缓存开关，不等同于 `PlanCacheTracker` 内部由互斥锁保护的 `useCache` 状态；修改其中一个不会由本文件自动同步另一个。

## 依赖与调用关系

上游边界包括：

- `pkg/planner/core/base/plan_base.rs` 将本类型暴露为规划器的 ranger context；多个 `PlanContext::GetRangerCtx` 实现把它提供给逻辑/物理规划和统计估算。
- `pkg/session/runtime/planning.rs` 构造真实规划上下文，填入类型、错误、表达式上下文和三个策略开关。
- `pkg/util/ranger/ranger.rs`、`detacher.rs`、`points.rs` 和 `types.rs` 接受 `&RangerContext` 或 `&mut RangerContext`，完成 range 生成、条件分离、端点转换和点范围判断。

直接下游依赖包括：

- `exprctx::BuildContext`：供 range 逻辑访问表达式构建/求值语义。
- `types::Context` 与 `errctx::Context`：提供类型转换/比较及错误处理策略。
- `contextutil::PlanCacheTracker::SetSkipPlanCache`：修改计划缓存资格并按缓存类型追加告警。
- `contextutil::RangeFallbackHandler::RecordRangeFallback`：以固定原因 `in-list is too long` 跳过缓存，并通过 `Once` 限制容量告警次数。
- 标准库 `Arc`、`HashMap` 和 `Box`：分别承载共享表达式上下文、fix-control 快照和 detach 返回值。

RustCodeGraph 将目标文件列为 6 个符号，并显示其被 planner、executor 和 ranger 相关文件使用；由于同名 `Detach`/`SetSkipPlanCache` 很多，精确 callers/callees 查询未返回可消歧边，本说明对方法调用边采用上述源码位置的直接核验，不把空查询解释为无调用。

## 错误处理与边界

本文件的三个方法都不返回 `Result`。`SetSkipPlanCache` 和 `RecordRangeFallback` 在 handler 缺失时静默跳过；这使无计划缓存/无告警设施的规划路径仍可构造 range，但不会记录对应诊断。

实际错误策略位于下游：`PlanCacheTracker::SetSkipPlanCache` 在非强制模式关闭计划缓存，在强制模式只追加风险告警；其互斥锁若 poisoned 会通过 `expect` panic。`RangeFallbackHandler::RecordRangeFallback` 始终请求跳过计划缓存，但容量告警每个 handler 只追加一次。本文件不捕获这些 panic，也不验证 `rangeMaxSize` 的正负或单位。

`Detach` 不保证所有会话状态都可并行使用。源码注释明确限定：detach 后会话上下文可以与新上下文并行使用，但原 `StatementContext` 仍不可共享；会话执行另一条语句前必须新建 `StatementContext`。此外，调用者负责传入确实适合脱离会话使用的 `staticExprCtx`，本方法不检查其实现。

## 并发与资源生命周期

`ExprCtx` 通过 `Arc` 管理生命周期，clone 增加强引用计数，detach 用新的 `Arc` 替换副本句柄。`RangerContext<'a>` 自身不拥有两个 handler；借用生命周期使返回的 `Box<RangerContext<'a>>` 仍受原 handler 生命周期约束，不会把它们提升为 `'static`。

共享的 `PlanCacheTracker` 内部以 `Mutex` 串行化状态读写，`RangeFallbackHandler` 以 `Once` 保证同一实例只发一次回退容量告警。但这些并发保证属于 handler 实现，不能推出整个 `RangerContext` 可跨线程共享：`BuildContextRef` 的 trait object 在此类型别名中没有 `Send + Sync` 边界，`StatementContext` 也被源码明确排除在可并行共享范围外。

`Detach` 不启动任务、不创建通道、不持有锁，也没有显式清理动作。返回的 `Box`、map 和 `Arc` 按 Rust 所有权自动释放；借用的 handler 由其拥有者管理。

## 与 Go 版本的对应关系

Rust `RangerContext` 逐字段对应 `pkg/util/ranger/context/context.go::RangerContext`。Go 的匿名嵌入 `*RangeFallbackHandler`、`*PlanCacheTracker` 在 Rust 中改成命名的 `Option<&...>`，所以 Rust 通过显式包装方法保持 Go 调用风格；nil 对应 `None`。Go 的 `exprctx.BuildContext` 接口对应 Rust 的 `Arc<dyn exprctx::BuildContext>`，Go `map[uint64]string` 对应 `HashMap<u64, String>`。

Go `Detach` 先做结构体浅拷贝，再替换 `ExprCtx`，并用 `maps.Clone` 深拷贝 fix-control map。Rust 先执行自定义 `Clone`，再替换 `ExprCtx` 并显式克隆 map，结果保持相同语义：标量/策略字段相同，map 存储独立，handler 身份保持。Rust 返回 `Box<RangerContext>`，对应 Go 返回新的指针。

`pkg/util/ranger/context/context_test.go::TestContextDetach` 验证 Go 的深拷贝及保留字段；Rust 的 `context_test.rs::test_context_detach` 和 `migration_aster_unit_test.rs::detach_replaces_expr_context_and_preserves_go_copy_semantics` 进一步验证 map 隔离、表达式上下文替换以及两个 handler 的指针身份。Rust 额外提供 `SetSkipPlanCache`、`RecordRangeFallback` 包装方法，是对 Go 匿名嵌入方法提升机制的显式表达，不是额外业务规则。

## 扩展指南

- 新增影响 range 语义的上下文字段时，应同时更新 `RangerContext`、自定义 `Clone`、所有生产构造点和 Go 对照结构；若字段需在 detach 后隔离，必须明确采用深拷贝或替换，而不能依赖默认共享。
- 修改 detach 语义时，应同步更新独立测试 `pkg/util/ranger/context/context_test.rs` 和 `migration_aster_unit_test.rs`，并与 `context_test.go` 的意图对照。不要把测试内嵌进生产源文件。
- 新增计划缓存失效条件应优先在真实 range 算法发现风险的位置调用 `SetSkipPlanCache`，并给出稳定、可诊断的原因；不要直接改写 tracker 内部状态。
- 新增回退条件应在确认返回的较粗 range 仍由 remained conditions 补足过滤语义后调用 `RecordRangeFallback`，避免只告警却丢失 SQL 过滤条件。
- 若希望跨线程移动或共享整个上下文，需要先核实并显式约束 `exprctx::BuildContext` 的线程安全性质，同时遵守 `StatementContext` 不可共享的约束；仅看到 `Arc` 不足以证明安全。
- 兼容风险主要是 Go/Rust 字段默认值、map 拷贝深度、handler 身份和告警次数发生偏差；性能风险主要来自不必要地复制大型 fix-control map，或过度调用缓存失效路径导致计划无法复用。

## 验证依据

- 目标实现：`pkg/util/ranger/context/context.rs`，核对 `BuildContextRef`、`RangerContext`、`Clone`、`SetSkipPlanCache`、`RecordRangeFallback`、`Detach` 全部符号。
- crate 边界：`pkg/util/ranger/context/Cargo.toml` 与 `lib.rs`，核对包名、依赖、Go 包映射、公开导出和独立测试装配。
- Go 对照：`pkg/util/ranger/context/context.go`、`context_test.go`，核对字段、匿名 handler、`maps.Clone` 和 detach 测试意图。
- Rust 测试：`pkg/util/ranger/context/context_test.rs`、`migration_aster_unit_test.rs`，核对表达式上下文替换、map 隔离、标志/类型上下文复制和 handler 身份保持。
- 调用与行为：`pkg/util/ranger/ranger.rs` 的 `convertPointInPlace`、`buildColumnRange`，`pkg/util/ranger/detacher.rs` 的参数覆盖分支，以及 `pkg/util/ranger/types.rs::Range::IsPoint`。
- handler 语义：`pkg/util/context/plancache.rs::PlanCacheTracker::SetSkipPlanCache` 与 `RangeFallbackHandler::RecordRangeFallback`，核对锁、强制缓存分支、告警和 `Once`。
- 应用接线：`pkg/planner/core/base/plan_base.rs` 的类型别名和 `pkg/session/runtime/planning.rs` 的实际构造点。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标目录 6 个文件均已索引；`files --filter pkg/util/ranger/context`、目标文件 `node`、`query RangerContext`、`query SetSkipPlanCache`、`query RecordRangeFallback`、`query Detach` 用于确认符号和使用面。精确 callers/callees 无输出的限制已在“依赖与调用关系”中披露，并由直接源码搜索补证。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终结构检查要求本文恰好包含上述十一个固定二级标题。

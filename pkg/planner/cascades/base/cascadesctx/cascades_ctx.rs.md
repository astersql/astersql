# `pkg/planner/cascades/base/cascadesctx/cascades_ctx.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-cascades-base-cascadesctx`，由同目录
`lib.rs` 的 `mod cascades_ctx` 装入并通过 `pub use cascades_ctx::*` 再导出。crate 的
直接依赖只有 `cascades-base`（调度器与任务抽象）和 `memo`（Cascades 搜索空间），见
`pkg/planner/cascades/base/cascadesctx/Cargo.toml`；这种依赖方向使任务代码可以面向一个
较小的上下文边界，而不必反向依赖具体优化器。

当前 Rust 接线必须与设计意图区分：本文件的 `Context` 与 `RuleMask` 已可通过根 facade
导出，也有同 crate 独立单元测试，但仓库搜索没有发现生产 Rust 代码实现或使用这个
trait。实际 Rust 优化主链目前使用
`pkg/planner/cascades/task/base.rs::Context` 和
`pkg/planner/cascades/cascades.rs::Context`，它们不是本文件的 trait。Go 主链则确实通过
`cascadesctx.Context` 解开 `cascades` 与 `task` 的导入环。因此本文件当前是已移植、可测试，
但尚未接入 Rust 主优化链的兼容边界，而不是运行时必经入口。

## 核心职责

文件只有两项职责：

1. `RuleMask` 用规则下标的集合表示选择性启用状态，提供 `Set`、`Test`、`Clear` 三个
   基本操作。
2. `Context` 规定优化阶段对调度器、Memo 和规则掩码的访问方式，并用默认方法
   `PushTask` 把任务入队动作统一转发给可变调度器。

它不负责构造 Memo、执行任务、应用规则或实现资源清理；这些行为分别由 trait 的具体
实现以及 `cascades-base`、`memo` 承担。文件也没有 feature 或条件编译项。

## 主要符号

- `pub struct RuleMask(BTreeSet<usize>)`：私有字段保证调用方只能通过公开方法改变掩码。
  派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`；默认值为空集合，即默认不启用
  任何规则。
- `RuleMask::Set(&mut self, index: usize)`：向集合插入下标；重复设置幂等。
- `RuleMask::Test(&self, index: usize) -> bool`：精确查询该下标是否存在。
- `RuleMask::Clear(&mut self, index: usize)`：移除下标；清除不存在的下标也是幂等操作。
- `pub trait Context`：公开、对象安全的上下文契约。实现者必须提供 `Destroy`、
  `GetScheduler`、`GetSchedulerMut`、`GetMemo`、`GetMemoMut`、`GetRuleMask` 和
  `GetRuleMaskMut`。
- `Context::PushTask(&mut self, Box<dyn cascades_base::Task>)`：唯一带实现的 trait 方法，
  调用 `GetSchedulerMut().PushTask(task)`；实现者无需重复写转发代码。

方法名保留 Go 风格的大写形式；同目录 `lib.rs` 以 `#![allow(non_snake_case)]` 明确允许
这一兼容命名。

## 执行流程

按本文件契约，典型流程是：具体上下文先持有一个调度器、一个 Memo 和一个
`RuleMask`；调用方经 `PushTask` 交入 `Box<dyn Task>`；默认实现取得调度器的可变 trait
对象并调用其 `PushTask`；后续何时以及按什么顺序执行由调度器实现决定。调用方可通过
`GetMemoMut` 更新搜索空间，通过 `GetRuleMaskMut` 设置规则，再从对应只读 getter 查询。
优化阶段结束时由调用方显式调用 `Destroy`，具体清理顺序完全由实现者负责。

独立测试 `context_routes_tasks_memo_and_rule_mask` 提供了可执行示例：任务经默认
`PushTask` 入栈，规则 7 被设置和清除，Memo 新建一个 Group，测试调度器执行任务，最后
`Destroy` 先清 Memo 再销毁调度器。另一个测试
`scheduler_executes_lifo_and_stops_at_first_error` 验证的是测试调度器及 `cascades-base`
契约：任务按 LIFO 执行，首个错误立即返回并保留尚未执行的任务；这不是本文件自行实现
的错误策略。

## 数据与状态

`RuleMask` 的唯一状态是 `BTreeSet<usize>`。集合去重使 `Set` 幂等；有序性让派生的
`Debug`/比较结果稳定，但本文件没有遍历 API，也不依赖排序来决定规则执行顺序。它没有
长度上限，不会自动启用规则，也不知道 `XFMaximumRuleLength`；任何 `usize` 都可被显式
设置。

`Context` 自身不存储数据，只约束借用关系：只读 getter 返回与 `self` 同生命周期的引用，
可变 getter 要求 `&mut self`，所以安全 Rust 会阻止同一实现上的重叠可变访问。
`GetScheduler`/`GetSchedulerMut` 返回 trait 对象，隐藏具体调度器；Memo 和 `RuleMask` 则以
具体类型暴露。`Box<dyn Task>` 把任务所有权转移给调度器。

## 依赖与调用关系

下游依赖如下：

- `std::collections::BTreeSet` 保存规则下标。
- `cascades_base::Scheduler` 提供调度器对象及 `PushTask`。
- `cascades_base::Task` 是入队任务的动态分发边界。
- `memo::Memo` 是搜索空间的具体类型。

模块入口是同目录 `lib.rs`；Cargo 清单把 Go 包映射记录为
`pkg/planner/cascades/base/cascadesctx`。RustCodeGraph 对目标文件识别出 14 个符号，对
`Context::PushTask` 给出的直接被调关系是 `GetSchedulerMut` 后调用调度器 `PushTask`。
仓库级引用搜索只找到同目录单元测试、根 workspace facade 再导出和本 crate 自身声明，
未找到生产 Rust 调用方。

作为对照，Go 上游是 `pkg/planner/cascades/cascades.go::Optimizer` 和具体 `Context`；Go
任务在 `pkg/planner/cascades/task/base.go` 持有 `cascadesctx.Context`，并在
`task_opt_group_expression.go::getValidRules` 使用 `GetRuleMask` 过滤规则。Rust 当前对应
主链已经改用 `pkg/planner/cascades/task/base.rs::Context` 的 `RulesFor`/`RuleEnabled`
接口以及 `Rc<RefCell<dyn Context>>`，所以不能把这些 Go 调用边登记成本文件的 Rust
调用边。

## 错误处理与边界

本文件没有 `Result`、`Option`、panic 或显式错误类型。`BTreeSet::insert/remove/contains`
不向调用方报告错误；重复设置、重复清除和查询未设置下标都有确定结果。`PushTask` 也不
返回错误，只负责移交任务；任务执行错误属于 `Scheduler::ExecuteTasks` 和 `Task::Execute`
边界。

`Destroy` 没有默认实现，也没有幂等性保证；实现者必须决定清理哪些资源以及重复调用
行为。getter 返回借用引用，因此不能把它们保存到上下文生命周期之外。`RuleMask` 不做
规则编号合法性校验：越界语义必须由上层约束。尤其不能把
`pkg/planner/cascades/cascades.rs::RuleMask` 的“`SetAll` 且限制在
`XFMaximumRuleLength` 内”行为归因于本文件；二者是不同类型。

## 并发与资源生命周期

`Context` 没有 `Send`、`Sync` 或 `'static` 约束，`RuleMask` 也没有内部锁、原子变量、通道
或后台任务。本文件只利用 `&self`/`&mut self` 的同步借用规则，并不承诺跨线程共享。
具体实现若需要并发，必须自行选择锁或其他同步机制，同时仍满足返回引用的方法签名。

任务在调用 `PushTask` 时被移动进调度器，生命周期随后由调度器管理。Memo、调度器和
规则掩码由具体上下文拥有或借用；`Destroy` 是显式生命周期终点，但 trait 不提供
`Drop` 兜底。测试实现的清理顺序是 Memo 后调度器，Go 具体实现也是这一顺序；本 trait
本身不强制该顺序。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/cascades/base/cascadesctx/cascades_ctx.go`。两边共同暴露
`Destroy`、`GetScheduler`、`PushTask`、`GetMemo`、`GetRuleMask`，共同目的都是把上下文
接口放到较低层包中以避免 `cascades` 与 `task` 导入循环。

Rust 为所有可变资源额外拆出 `GetSchedulerMut`、`GetMemoMut`、`GetRuleMaskMut`，以满足
借用规则；`PushTask` 因而可作为默认转发实现。Go 返回接口或指针，天然允许内部修改。
Go 的掩码类型是第三方 `bitset.BitSet`，具体上下文在
`pkg/planner/cascades/cascades.go::NewContext` 中按 `XFMaximumRuleLength` 创建并
`SetAll`；本文件的 `RuleMask` 是无上限的稀疏 `BTreeSet` 且默认为空。若未来让 Rust
生产主链接入本 trait，构造者必须显式实现与 Go 相同的默认全启用和上界语义，不能只用
`RuleMask::default()`。

Go 具体 `Context.Destroy` 清理 Memo 后清理 scheduler；Go `Optimizer::NewOptimizer`
初始化 Memo、压入根 Group 优化任务，`Execute` 再运行任务。当前 Rust 主链在
`pkg/planner/cascades/cascades.rs` 独立实现了这些行为，但没有实现本文件 trait，说明移植
已经存在功能重叠，接口统一仍未完成。

## 扩展指南

若只新增规则掩码操作，应优先扩展 `RuleMask`，并在独立文件
`pkg/planner/cascades/base/cascadesctx/cascades_ctx_aster_unit_test.rs` 添加边界测试；不要把
测试内嵌进生产源文件。需要 `SetAll`、长度上限或迭代时，应先确定与 Go bitset 的精确
兼容语义，特别覆盖空掩码、最大合法下标、越界下标、重复设置和清除。

若扩展上下文服务，必须同步考虑：trait 新方法是否保持对象安全；只读/可变访问是否成对；
`PushTask` 默认实现是否仍能避免具体优化器依赖；所有实现者和独立测试桩是否同步更新。
新增错误路径时应使用可传播的返回类型，不要在边界层吞错或 panic。

若目标是把本 trait 接入生产 Rust 主链，最可能涉及本文件 `Context`、
`pkg/planner/cascades/cascades.rs::Context`、`pkg/planner/cascades/task/base.rs::Context` 及其
测试。应先设计如何合并目前不同的 Memo API、规则列表 API 和 `Rc<RefCell>` 共享模型，
再逐步替换；否则会出现两个同名 `Context`/`RuleMask` 的语义漂移。兼容风险主要是默认
规则集合和销毁顺序，性能风险主要是规则掩码表示与共享借用方式改变，正确性风险主要是
任务在持有借用时再次入队导致动态借用冲突。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的
  `cascades_ctx.rs`、`cascades_ctx_aster_unit_test.rs`、`lib.rs` 和 Go 对照文件均已索引；
  目标源识别出 14 个符号。
- RustCodeGraph `explore`/`node`：核对了 `RuleMask`、`Context`、默认 `PushTask` 的源码及
  `GetSchedulerMut -> Scheduler::PushTask` 转发；读取了同目录 Rust 独立测试、模块入口和
  Go 接口。
- 配置证据：`pkg/planner/cascades/base/cascadesctx/Cargo.toml`、同目录
  `BUILD.bazel`、根 `Cargo.toml` workspace/facade 声明。
- Go 调用证据：`pkg/planner/cascades/cascades.go`、
  `pkg/planner/cascades/task/base.go`、
  `pkg/planner/cascades/task/task_opt_group_expression.go`。
- Rust 接线与差异证据：`pkg/planner/cascades/task/base.rs`、
  `pkg/planner/cascades/cascades.rs`、`pkg/planner/cascades/cascades_test.rs` 和
  `pkg/planner/cascades/task/task_test.rs`。仓库级引用搜索未发现本文件 trait 的生产实现；
  后两处测试覆盖的是当前主链的另一套上下文/掩码，不能视为本文件的直接测试。
- 直接测试证据：
  `pkg/planner/cascades/base/cascadesctx/cascades_ctx_aster_unit_test.rs` 覆盖默认任务转发、
  规则设置/查询/清除、Memo 可变访问、销毁效果，以及测试调度器的 LIFO/错误短路。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证另行执行并记录退出码。

# `pkg/planner/cascades/cascades.rs`

## 文件定位

本文件是 Rust crate `astersql-planner-cascades` 的门户实现。`pkg/planner/cascades/lib.rs` 将私有 `cascades` 模块的公开项全部再导出，`pkg/planner/cascades/Cargo.toml` 则把它连接到 `base`、`task`、`memo`、`pattern`、`rule` 与 `logicalop` 六个本地 crate。它负责把调用者提供的逻辑计划放入 Memo，并以任务栈启动 Cascades 的逻辑等价变换搜索；物理实现选择和最终代价最优计划不在本文件中。

当前 Rust 接线范围要特别区分：仓库搜索到本文件 `NewOptimizer` 的可执行调用集中在 `pkg/planner/cascades/cascades_test.rs`；没有发现它被 Rust 服务端规划主链调用。`Cargo.toml` 中测试依赖还放在永不成立的 `cfg(any())` 下。因此这里是已实现并由无存储单元测试覆盖的 Cascades 门户，不等同于已经替代 Go 生产规划入口。Go 生产对照是同目录的 `cascades.go`。

## 核心职责

- `PlanContext` 与 `LogicalPlan` 定义进入优化器所需的最小会话信息和根逻辑表达式接口。
- `Memo` 包装 `cascades_memo::Memo`，保存容量提示和根 `GroupExpressionRef`，并把底层错误统一转成 `TaskError`。
- `RuleMask` 表达规则开关；`TaskContext` 将 Memo、规则表和调度入口适配为 `task::Context`。
- `SharedTaskScheduler` 以单线程 LIFO 栈执行任务，并允许任务执行期间继续压入派生任务。
- `Context` 聚合一次优化阶段的所有共享状态，`Optimizer` 负责初始化根 Group、启动搜索、暴露 Memo 和清理资源。

该文件只负责“搜索框架门户和阶段状态”。规则模式绑定及变换位于 `task/task_apply_rule.rs`，Group/GroupExpression 的保存与去重位于 `memo/`，规则本体位于 `rule/`。

## 主要符号

- `PlanContext { operator_num: Vec<usize> }`：由 `LogicalPlan::SCtx` 返回，作为 Memo 各算子类别的容量预估。
- `LogicalPlan::{SCtx, RootExpression}`：调用者必须实现的入口 trait；后者返回 `LogicalPlanRef` 或 `TaskError`。
- `Memo::{NewMemo, Init, CopyIn, RemoveOut, Destroy}`：分别负责构造、插入根表达式、向既有 Group 插入等价表达式、淘汰表达式和清理阶段状态。`GetRootGroupExpression` 与 `Capacities` 主要用于观察和测试。
- `RuleMask::{SetAll, Set, Clear, Test}`：默认规则域由 `cascades_rule::XFMaximumRuleLength` 限定；全启用模式仍拒绝越界 id，`disabled` 支持在全启用模式中单独关闭规则。
- `SharedTaskScheduler`：实现 `base::Scheduler`。`PushTask` 压到 `Vec` 尾部，`ExecuteTasks` 从尾部弹出，因此是深度优先倾向的 LIFO 调度。
- `TaskContext`：实现 `task::Context`，给任务提供 `CopyInWithChildren`、`RulesFor`、`RuleEnabled` 等能力。它本身不公开，避免任务直接依赖外层 `Optimizer`。
- `Context`：保存 `PlanContext`、共享 Memo、调度器、规则掩码和 `Operand -> Vec<RuleRef>` 表。`RegisterRules` 对同一 Operand 是整体替换，不是追加。
- `Optimizer::{NewOptimizer, Execute, Destroy}`：生命周期入口。包级 `NewContext`、`NewOptimizer` 只是同名关联函数的转发门面。

## 执行流程

1. 调用者将实现了 `LogicalPlan` 的对象传给 `NewOptimizer`。构造函数先调用 `SCtx`，用 `operator_num` 创建 `Context` 和空 Memo；`Context::NewContext` 默认 `SetAll`。
2. `Memo::Init` 调用 `LogicalPlan::RootExpression`，再交给底层 `cascades_memo::Memo::Init` 建立根 Group/GroupExpression，并缓存根表达式引用。任何可返回错误都转换或传播为 `TaskError`。
3. 构造函数从根表达式取得所属 Group。该关系是初始化后的不变量；若缺失会由 `expect` 触发 panic。随后创建 `task::NewOptGroupTask` 并压入共享栈。
4. `Optimizer::Execute` 进入 `SharedTaskScheduler::ExecuteTasks`：每次先从 `RefCell<Vec<_>>` 中弹出任务并结束借用，再调用任务的 `Execute`，使当前任务能够安全地继续压栈。
5. `OptGroupTask`（`task/task_opt_group.rs`）为尚未探索的 Group 中每个逻辑表达式压入 `OptGroupExpressionTask`，然后将 Group 标记为已探索。
6. `OptGroupExpressionTask`（`task/task_opt_group_expression.rs`）按表达式 Operand 读取已注册且已启用的规则，压入 `ApplyRuleTask`；它还把子 Group 逆序压栈，以抵消 LIFO 顺序，使第 0 个子组先执行。
7. `ApplyRuleTask`（`task/task_apply_rule.rs`）遍历 Binder 产生的匹配，执行 `PreCheck` 和 `XForm`。新表达式通过 `TaskContext::CopyInWithChildren` 放回同一目标 Group，再生成后续表达式任务；规则要求替换时调用 `RemoveOut`。栈清空即搜索结束，任一任务错误则立即返回。
8. 阶段完成后，所有者应调用 `Destroy`；它销毁底层 Memo、清空根引用和容量提示，并清空调度栈。

## 数据与状态

Memo 的等价类状态由 `cascades_memo::Memo` 真正持有；本文件额外保留 `root: Option<GroupExpressionRef>` 与原始 `capacities`。初始化前或销毁后 `root` 为 `None`。`GroupRef`、`GroupExpressionRef`、Memo、规则掩码和规则表均通过 `Rc<RefCell<_>>` 在单线程任务之间共享。

规则状态有两层：`rules` 决定某个 `Operand` 有哪些候选规则，`RuleMask` 决定其中某个规则 id 是否启用。默认上下文的掩码全开，但规则表初始为空，所以“默认全开”不会自动注册任何规则。`Optimizer::SetRules` 只额外 `Set` 指定 id，不会把默认全开切换成白名单模式。

调度状态是 `SharedTaskScheduler.stack` 中的 `Vec<Box<dyn Task>>`。尾部同时是压入点和弹出点；任务顺序依赖这一 LIFO 不变量。`Context::TaskContext` 创建新的适配器对象，但其字段都克隆共享句柄，因此所有任务仍观察同一 Memo、栈、掩码和规则表。

## 依赖与调用关系

上游入口是 `LogicalPlan` 实现者以及 crate 再导出的 `NewOptimizer`。现有 Rust 直接证据是 `cascades_test.rs` 中的 `ScanPlan` 和三个 `NewOptimizer(Box::new(ScanPlan))` 调用；RustCodeGraph 将本文件列为被测试、任务路由和旧优化器等 30 个文件使用，但精确调用者查询在本地索引上未完成，故不据此声称生产接线。

下游依赖如下：

- `logicalop::LogicalPlanRef` 提供可放入 Memo 的逻辑算子对象。
- `cascades_memo::{Memo, GroupRef, GroupExpressionRef}` 提供等价类、表达式去重/归组和根初始化。
- `base::{Task, Scheduler}` 规定任务与调度器接口。
- `task::{NewOptGroupTask, Context, TaskError}` 及其后续任务构成搜索执行链。
- `cascades_pattern::Operand` 是规则表键；`cascades_rule::{Rule, XFMaximumRuleLength}` 提供规则对象和合法默认掩码范围。

Cargo 依赖全部是工作区路径依赖，未声明 feature。本 crate 的 `lib.rs` 只做模块再导出，并在 `cfg(test)` 下挂载独立测试文件。

## 错误处理与边界

`LogicalPlan::RootExpression`、底层 Memo 初始化/插入和规则变换错误会向上返回；Memo 字符串错误被包装为 `TaskError`，调度入口统一返回 `Box<dyn Error>`。调度器遇到首个错误立即停止，栈中剩余任务不会自动继续，也不会在错误路径自动调用 `Destroy`，资源清理由所有者负责。

三处所有权关系使用 `expect`：初始化后的根表达式必须属于 Group，应用规则的表达式也必须仍属于 Group。这些是内部结构不变量，违反时 panic，而不是可恢复错误。`RefCell` 的重叠可变借用也会 panic；调度循环特意在执行任务前释放栈借用以避开常见重入冲突。

`RuleMask::Test` 在全启用模式下只接受小于 `XFMaximumRuleLength` 的 id；显式 `Set` 的 id 则不受这个默认范围判断限制。`RegisterRules` 对键覆盖旧值。`Destroy` 后仍可借用 Optimizer，但根为空、容量被清空，若要再次优化必须新建实例，因为本文件没有重初始化协议。

## 并发与资源生命周期

这是单线程实现，不是线程安全调度器：`Rc`、`RefCell` 和 `Box<dyn Task>` 都没有提供跨线程共享保证。源码注释中“共享”指同一线程内多个任务句柄共享状态，不应解释为并行执行。任务执行期间可递归派生任务，但始终由一个循环串行弹栈。

生命周期为 `NewOptimizer -> Execute（可返回成功或错误） -> Destroy`。`Optimizer` 拥有逻辑计划和 `Context`；`Context` 的 `Rc` 句柄还会被任务持有。正常执行至栈空会释放任务，从而解除这些额外引用。`Destroy` 清空 Memo 和栈；该调度器没有线程、通道、锁、异步任务或 I/O 资源。与 `task/task.rs` 的线程本地 `Stack` 对象池不同，本文件的 `SharedTaskScheduler` 直接拥有 `Vec`，不会归还那个池。

## 与 Go 版本的对应关系

`pkg/planner/cascades/cascades.go` 的 `Optimizer`、`NewOptimizer`、`Execute`、`Destroy`、`GetMemo`、`SetRules`，以及 `Context` 的 Memo/调度器/bitset 组合，是本文件的直接移植基线。两边都从 `lp.SCtx()` 建 Memo、初始化根表达式、压入根 Group 任务，并由调度器执行至结束。

Rust 为适应类型和所有权模型增加了几层显式结构：`LogicalPlan` trait 把 Go `corebase.LogicalPlan` 缩成门户所需接口；`TaskContext` 隔离任务可见能力；`Rc<RefCell<_>>` 替代 Go 指针共享；`TaskError` 承接字符串化错误。Rust `RuleMask` 用集合模拟 Go 固定长度 bitset，并通过 `XFMaximumRuleLength` 保持默认范围边界。

行为差异和迁移状态也必须保留：Go `NewOptimizer` 对根初始化结果还有 `intest.Assert`，Rust 直接传播初始化错误、仅对“根必须属于 Group”使用 `expect`；Go `Context` 使用 `task.NewSimpleTaskScheduler()`，Rust 本文件内置 `SharedTaskScheduler`。Rust 还提供 `RegisterRules`、`CopyInWithChildren` 和显式规则表，这是驱动现有 Rust 规则任务所需的局部接线，而 Go 本文件没有对应字段。Go SQL 测试 `cascades_test.go` 覆盖 planner 开关与 Apply-to-Join 统计一致性；Rust 文件中这些 SQL 测试主体只是注释，当前可执行测试是无存储的门户级等价检查，不能视为完整 SQL 集成对齐。

## 扩展指南

新增逻辑计划入口时，实现 `LogicalPlan::{SCtx, RootExpression}`，并保证根表达式能由 `cascades_memo::Memo::Init` 归入 Group。新增变换规则时，在合适的 `Operand` 下调用 `RegisterRules`，保持规则 id 与 `XFMaximumRuleLength`/掩码语义一致，并在独立测试文件中验证规则匹配、重复探索和 remove 分支。

修改调度顺序时必须同步审查 `SharedTaskScheduler::PushTask/ExecuteTasks` 与 `task_opt_group_expression.rs` 的逆序压子组逻辑；将 LIFO 改成 FIFO 会改变子树和规则执行次序。若引入并行搜索，不能只替换容器：还需整体替换 `Rc<RefCell<_>>`、明确 Group/Memo 并发不变量、错误取消策略与 `Destroy` 等待语义。

修改 Memo 门户时应同步 `pkg/planner/cascades/cascades_test.rs`，不要把 Rust 测试嵌入生产文件。若要宣称生产可用，还需把本 crate 接到 Rust planner 主链，并把 Go `cascades_test.go` 中 planner 开关、EXPLAIN 结果和变换后统计行为迁移为真实可执行的独立 Rust 集成测试；仅让当前门户单测通过不足以证明这一点。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/cascades` 找到本文件及其 Go/Rust 邻接实现；`node --file pkg/planner/cascades/cascades.rs` 读取全 391 行并列出 59 个符号。
- RustCodeGraph 源码节点：`task/task_opt_group.rs` 的 `OptGroupTask::Execute`，`task/task_opt_group_expression.rs` 的规则筛选与逆序子组压栈，`task/task_apply_rule.rs` 的 Binder/XForm/CopyIn/RemoveOut 链，`base/task_scheduler_base.rs::Scheduler` 与 `task/base.rs::Context` trait。
- crate 边界：`pkg/planner/cascades/Cargo.toml`、`pkg/planner/cascades/lib.rs`。
- Go 对照：`pkg/planner/cascades/cascades.go`、`pkg/planner/cascades/cascades_test.go`。
- Rust 独立测试：`pkg/planner/cascades/cascades_test.rs`，覆盖 Memo 容量与根表达式、根 Group 探索、销毁、规则掩码范围、规则注册与探索标记；其中 SQL testkit 段落为注释，不计作已执行覆盖。
- 接线核验：仓库范围 `rg` 查询 `NewOptimizer`、`RegisterRules`、crate 名和相关观察方法；没有发现本门户进入 Rust 服务端生产规划链。RustCodeGraph 精确 `callers NewOptimizer --file ...` 在本地长时间无输出后停止，因此本结论采用仓库文本搜索作保守直接证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付只执行任务规定的 11 章节结构检查及文档差异自审。

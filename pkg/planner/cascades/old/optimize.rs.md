# `pkg/planner/cascades/old/optimize.rs`

## 文件定位

本文件是 `astersql-planner-cascades-old` crate 的旧版 Cascades 优化主循环。crate 根 `pkg/planner/cascades/old/lib.rs` 以 `mod optimize` 装配本模块并通过 `pub use optimize::*` 再导出其公开项；`pkg/planner/cascades/old/Cargo.toml` 将 crate 映射到 Go 包 `pkg/planner/cascades/old`，没有声明可改变本文件行为的 feature。

对外的主要入口是线程局部的 `DefaultOptimizer`、`Optimizer`、`Optimizer::FindBestPlan` 和 `preparePossibleProperties`。`FindBestPlan` 接收已经构造好的 `LogicalPlanRef`，输出物理计划及总代价，不负责 SQL 解析、逻辑计划构建或执行。全仓 Rust 调用搜索目前只在 `pkg/planner/cascades/old/optimize_test.rs` 找到对本文件内部能力的使用，没有找到生产代码调用 `FindBestPlan` 或 `DefaultOptimizer`；因此本模块已被 crate 导出并可直接调用，但不能据此声称它已接入当前 Rust SQL 请求主链。Go 版本的权威同路径对照是 `pkg/planner/cascades/old/optimize.go`。

## 核心职责

本文件把旧 Cascades 优化过程组织成三个阶段，并维护阶段间依赖的不变量：

1. `onPhasePreprocessing` 以根计划输出 schema 的全部列调用 `PruneColumns`，先消除无用列。
2. `memo::Convert2Group` 把逻辑树变成等价表达式组；`onPhaseExploration` 按 `TransformationRuleBatch` 的轮次扩展各组中的逻辑等价式。
3. `onPhaseImplementation` 先递归准备排序属性和 TiFlash 可达性，再由 `implGroup` 在所需 `PhysicalProperty` 与代价上限内选择最低代价实现；必要时用 enforcer 补足不能自然满足的属性。
4. `FindBestPlan` 在返回前调用 `PhysicalPlan::resolve_indices`，让物理表达式中的列索引与最终孩子 schema 对齐。

它还承担两项实现阶段的辅助工作：`fillGroupStats` 自底向上填充 memo group 统计信息，`preparePossibleProperties` 汇总每个 group 可提供的有序列组合与 `HasTiFlash`。规则本身不在此定义：逻辑变换、逻辑到物理实现、属性强制分别来自同 crate 的 `transformation_rules.rs`、`implementation_rules.rs`、`enforcer_rules.rs`。

## 主要符号

- `type OptimizeResult<T> = Result<T, Box<dyn std::error::Error>>`：把列剪枝、规则变换、统计推导、物理构造和索引解析的错误统一向上传播。
- `DefaultOptimizer: RefCell<Optimizer>`：由 `thread_local!` 创建的每线程默认优化器，初值来自 `Optimizer::NewOptimizer`。调用方若要使用它，需要通过线程局部访问闭包借用；它不是跨线程共享单例。
- `Optimizer`：仅保存 `transformation_rule_batches` 与按 `Operand` 索引的 `implementation_rule_map`。它不持有单次查询的 memo、统计或代价缓存；这些状态在计划/memo 对象内。
- `NewOptimizer`：载入 `default_rule_batches()` 与 `defaultImplementationMap()`。`ResetTransformationRules`、`ResetImplementationRules` 用于替换整套规则，主要便于测试或定制；修改会持续影响该 `Optimizer` 后续查询。
- `GetImplementationRules`：通过 `pattern::GetOperand(node)` 查表；没有条目时返回空切片，而非报错。
- `FindBestPlan`：公开三阶段入口，成功返回 `(Box<dyn PhysicalPlan>, f64)`。
- `onPhaseExploration`、`exploreGroup`、`findMoreEquiv`：以“批次轮次”为隔离单位管理 group/expression 的探索标记、模式绑定、等价式插入和旧式删除。
- `fillGroupStats`：只在 `Group.Prop.Stats` 为空时递归推导；使用 group 的第一条等价式代表共享逻辑属性。
- `implGroup`、`implGroupExpr`：前者递归搜索孩子实现、进行 branch-and-bound 式代价剪枝并缓存最优实现；后者只负责把单条 `GroupExpr` 交给匹配的实现规则。
- `preparePossibleProperties`：公开递归函数，以 group ID 为缓存键，去重排序属性并回写 `Group.Prop.PossibleProps`/`HasTiFlash`。
- `prepare_expression_properties`：针对若干已移植逻辑算子做运行时类型派发；未专门列出的算子退回到逻辑基类的 TiFlash 传播和“继承第一个孩子排序”的默认行为。

## 执行流程

`FindBestPlan` 的正常路径如下：

1. 读取根逻辑计划 `Schema().Columns`，将其作为保留列传给 `PruneColumns`。任意错误立即返回。
2. 用 `memo::Convert2Group` 生成根 `GroupRef`。
3. 对每个变换规则批次执行探索。`exploreGroup` 先把当前 group 标成已探索，再克隆当次等价式快照；每条仍存在且本轮未处理的表达式先递归探索孩子，再调用 `findMoreEquiv`。
4. `findMoreEquiv` 按根 `Operand` 选择规则，通过 `NewExprIterFromGroupElem` 枚举模式绑定。普通结果先暂存在 `pending`，绑定枚举完成后才插入，避免 `Vec` 插入改变当前表达式索引。成功插入新等价式会把 group 重新标为未探索，使外层循环下一遍处理新式。`erase_old` 延迟到调用方删除当前式；`erase_all` 则清空整个 group、插入替代式、结束当前表达式处理。
5. 实现阶段建立 `ExpectedCnt = f64::MAX` 的根属性，调用 `preparePossibleProperties`，然后以无限代价上限进入 `implGroup`。
6. `implGroup` 先复用同一所需属性的 group 缓存；随后确保统计已填充。它为每条等价式收集实现候选，逐个读取候选物理算子的孩子属性与动态孩子代价上限，递归取得孩子最优实现，再调用 `CalcCost`。一旦发现更低代价候选，就附着孩子并收紧 `cost_limit`。
7. 自然候选枚举后，再遍历 `GetEnforcerRules`。每个 enforcer 先把所需属性放宽，递归求孩子，实现后加上强制代价；若更优则替换当前最佳项。
8. 最优实现写入 group 的属性缓存。根实现被克隆为独立物理计划，读取总代价，最后解析列索引并返回。

`preparePossibleProperties` 是后序遍历：先取孩子属性，再调用 `prepare_expression_properties` 获取本表达式属性；同一 group 的各等价式结果以 `PhysicalProperty::HashCode` 去重，`HasTiFlash` 用逻辑或合并，结果同时写回 group 和调用方缓存。

## 数据与状态

- memo 共享结构使用 `GroupRef`/`GroupExprRef`（即 `Rc<RefCell<...>>`，定义于 `pkg/planner/memo/group.rs` 与 `group_expr.rs`）。本文件通过短生命周期的不可变/可变借用更新探索标记、等价式、统计、可选属性及实现缓存。
- `ExploreMark` 以 `round` 区分规则批次。同一表达式在同一轮只处理一次；新等价式插入会撤销 group 的本轮完成标记，但不会清除已处理表达式的标记。
- `Group.Prop.Stats`、`Schema`、`PossibleProps`、`HasTiFlash` 是实现阶段读取的共享属性。`fillGroupStats` 假定同一 group 的等价表达式共享逻辑属性，因此只用第一条表达式推导统计。
- 统计推导时，每个孩子 group 被包装成带 schema 和 stats 的 `MockDataSource`，临时挂到当前逻辑算子上；`DeriveStats(false)` 后立即 `TakeChildren`，避免把这些模拟孩子永久留在 memo 表达式中。
- `implGroup` 的缓存键是完整 `PhysicalProperty`，缓存值是 `ImplementationRef`；命中但代价超过本次上限时返回 `None`，不会误当作本次可行解。
- 属性递归缓存是调用方提供的 `HashMap<u64, PossiblePropertiesInfo>`，键为 `Group::ID()`。排序组合先转为 `SortItems` 再以 `HashCode()` 的字节向量去重。
- `Optimizer` 规则配置是长生命周期可变状态，单查询 memo 则由 `FindBestPlan` 局部创建。调用 `Reset*Rules` 后若要恢复默认配置，需要调用方显式恢复或新建优化器。

## 依赖与调用关系

上游边界：`pkg/planner/cascades/old/lib.rs` 公共再导出本模块；直接 Rust 测试 `pkg/planner/cascades/old/optimize_test.rs` 作为 `optimize.rs` 的子模块调用私有阶段。RustCodeGraph 对 `FindBestPlan` 和 `preparePossibleProperties` 定位到了本文件及 Go 对照符号，但 callers 查询未返回生产调用边；全仓 `rg` 同样未发现 Rust 生产调用点。由此能确认 API 边界和测试接线，不能确认运行时 SQL 主链调用。

主要下游关系：

- `memo::Convert2Group`、`Group::{Insert,Delete,DeleteAll}`、`NewExprIterFromGroupElem` 和 group 实现缓存提供搜索空间与记忆化载体。
- `pattern::GetOperand` 决定变换规则和实现规则的分派键。
- `Transformation::{matches,on_transform}` 产生逻辑等价式以及 `erase_old`/`erase_all` 控制信号。
- `ImplementationRule::{Match,OnImplement}` 产生物理候选；`Implementation` 的孩子属性、代价上限、代价计算和附着接口完成递归动态规划。
- `GetEnforcerRules` 为不满足所需物理属性的 group 提供排序等强制实现。
- `LogicalPlan::{PruneColumns,DeriveStats}`、具体算子的 `PreparePossibleProperties` 与 `PhysicalPlan::resolve_indices` 分别支撑预处理、统计/属性推导和最终计划修正。

`Cargo.toml` 的直接关键依赖包括 `astersql-planner-cascades-pattern`、`astersql-planner-memo`、`astersql-planner-property`、`astersql-planner-core-base`、`astersql-planner-core-operator-logicalop` 与 `astersql-expression`；相关 Rust 测试所需的解析/测试上下文依赖列在 `[dev-dependencies]`，但当前 Rust 测试选择手工构造逻辑计划以聚焦优化器阶段。

## 错误处理与边界

- 可恢复错误通过 `?` 从列剪枝、变换规则、统计推导、实现规则、物理计划克隆与索引解析逐层返回。找不到根物理实现时显式构造 `PlannerError("Can't find a proper physical plan for this query")`。
- `fillGroupStats` 对空 group、缺少孩子 stats、缺少孩子 schema 分别返回带上下文的 `PlannerError`，避免像 Go 版本那样直接解引用空的首元素或属性。
- `implGroup` 用 `None` 表示在当前属性/代价上限下无可行实现；候选孩子失败会把该候选代价设为 `f64::MAX` 并跳过。严格使用 `candidate_cost > cost_limit`，因此等于上限的候选仍可接受；最终会过滤无限代价结果。
- enforcer 递归使用 `cost_limit - enforce_cost`。这里没有单独钳制负数；负上限自然使递归候选不可行，这是现有剪枝语义。
- `fillGroupStats` 成功返回后，`implGroup` 用 `expect("statistics were filled above")` 读取统计；只要 `fillGroupStats` 的契约成立就不会 panic。扩展统计路径时不得出现“返回成功但未写 Stats”。
- `prepare_expression_properties` 只对列出的具体算子专门派发。新增算子若依赖特殊排序语义而未增加分支，会走默认传播逻辑；这不会报错，但可能让候选属性不完整或不精确。
- 本文件不处理取消、超时、搜索空间上限或 NaN 代价。规则导致的组合爆炸、递归深度和非有限代价都需要规则/调用层约束；当前实现仅靠 explored 标记、memo 缓存和代价上限剪枝。

## 并发与资源生命周期

`DefaultOptimizer` 是 `thread_local!` 中的 `RefCell<Optimizer>`：每个线程独立初始化规则集并可独立重置，避免对全局规则表加锁；同时，持有其可变借用时再次嵌套借用会触发 `RefCell` 运行时借用 panic。自定义配置不会自动传播到其他线程，也不会跨线程共享。

memo 使用单线程的 `Rc<RefCell<_>>`，本文件没有 `Arc`、互斥锁、异步任务或通道；`Optimizer` 与一次搜索生成的 group 图不应被假定为 `Send`/`Sync`。递归探索、统计推导、属性推导和物理实现都在调用线程同步完成。

临时资源的生命周期有三处需要注意：探索时克隆等价式/孩子引用快照以缩短 `RefCell` 借用；规则生成的新式在完成当前绑定枚举后批量插入；统计推导临时安装 `MockDataSource` 孩子，并在推导后无论成功与否都通过同一作用域内的 `TakeChildren` 取回。最终物理实现通过 `clone_physical` 从 memo 中的实现对象克隆后返回，避免返回仍受 `RefCell` 借用约束的计划引用。

## 与 Go 版本的对应关系

整体阶段和关键算法与 `pkg/planner/cascades/old/optimize.go` 一一对应：`FindBestPlan` 的预处理—探索—实现顺序、按轮 explored 标记、变换规则的 erase 语义、统计填充、按属性缓存实现、代价上限剪枝、enforcer 兜底以及可能属性递归均被保留。

已核对的实现差异如下：

- Go 的 `DefaultOptimizer` 是进程级可变指针；Rust 改为每线程 `RefCell<Optimizer>`，并发可见性不同。
- Go group 等价式是链表，可在遍历中插入；Rust group 使用 `Vec`，因此 `exploreGroup` 读取快照，`findMoreEquiv` 固定当前根表达式并延迟插入，防止绑定中的元素索引失效。
- Go 的 `fillGroupStats` 直接向 `DeriveStats` 传孩子统计/schema；当前 Rust `LogicalPlan::DeriveStats` 接口要求从计划孩子读取，故用临时 `MockDataSource` 桥接并随后移除。
- Go 的可能属性缓存以 group 指针为键；Rust 以稳定的 `Group::ID()` 为 `u64` 键。Rust 还把结果写入 `Group.Prop.PossibleProps` 和 `HasTiFlash`，而同路径 Go 函数主要返回并缓存 `PossiblePropertiesInfo`。
- Go 通过虚方法统一调用各逻辑算子的 `PreparePossibleProperties`；Rust 因当前 trait/移植边界在 `prepare_expression_properties` 中对 `DataSource`、扫描、CTE、Gather、Selection、Projection、Sort、TopN、Aggregation 做显式 downcast，并为其余算子使用基类回退。
- Go 返回 memo 中实现持有的计划并在末尾 `ResolveIndices`；Rust 先从实现中取计划，以原 `PlanContext` 调用 `clone_physical`，再解析索引。

测试意图保持在独立文件 `pkg/planner/cascades/old/optimize_test.rs`：覆盖 group schema 初始化但 stats 尚空、零代价上限返回无实现、统计递归填充、聚合排序属性准备、每轮每表达式只应用一次规则。对应 Go 测试位于 `pkg/planner/cascades/old/optimize_test.go`；Rust 测试用最小手工逻辑计划替代 Go 的 SQL parser/`BuildLogicalPlanForTest` 管线，因此证明的是本文件阶段逻辑，不是完整 SQL 接入链。

## 扩展指南

- 新增逻辑变换时，应在 `transformation_rules.rs` 实现规则并接入 `default_rule_batches`，确认 pattern 根 `Operand` 与 `findMoreEquiv` 的分派一致；若使用删除语义，要分别验证保留旧式、删除当前式、清空全部式，并验证插入新式后本轮会重新探索。
- 新增逻辑算子的物理实现时，应在 `implementation_rules.rs` 的 `defaultImplementationMap` 注册规则，确保产生的物理计划为每个孩子提供正确 `get_child_req_props`，并让代价计算与 `GetCostLimit` 能用于递归剪枝。
- 新算子若能产生或变换排序属性，必须评估 `prepare_expression_properties`：需要特殊语义时增加明确 downcast 分支；默认分支仅传播第一个孩子的排序并合并 TiFlash 可达性，不能替代 join、union 或其他多孩子算子的专门规则。
- 调整统计推导时必须保持 `fillGroupStats` 的成功后置条件（group stats 一定存在），并保证临时孩子总被移除；同时验证所有等价式确实共享 schema/逻辑统计语义。
- 改动实现选择时，要保持属性缓存键、`ExpectedCnt` 截断、等于代价上限可接受、无限代价不可缓存以及 enforcer 放宽属性后递归回到同一 group 等不变量。
- 测试应继续放在独立的 `pkg/planner/cascades/old/optimize_test.rs`，不要嵌入生产源文件；若验证 Go 行为差异，还应同步对照 `optimize_test.go`。建议至少添加目标规则命中/不命中、孩子实现失败、缓存命中且超限、enforcer 胜出、空 group 错误和新增算子属性传播用例。
- 若要把旧优化器接入 Rust 生产主链，应先在调用侧明确选择条件、`PlanContext` 生命周期和线程局部规则配置；这属于本文件之外的接线工作，不能仅凭 `lib.rs` 的公开再导出认定已经完成。

## 验证依据

- 源码全量阅读：`pkg/planner/cascades/old/optimize.rs`；主要符号为 `Optimizer::{FindBestPlan,onPhasePreprocessing,onPhaseExploration,exploreGroup,findMoreEquiv,fillGroupStats,onPhaseImplementation,implGroup,implGroupExpr}`、`preparePossibleProperties`、`prepare_expression_properties`。
- crate 与模块边界：`pkg/planner/cascades/old/Cargo.toml`、`pkg/planner/cascades/old/lib.rs`；本目录未发现需要额外遵循的 `doc.go`。
- 规则与 memo 直接证据：`pkg/planner/cascades/old/{transformation_rules.rs,implementation_rules.rs,enforcer_rules.rs}`，以及 `pkg/planner/memo/{group.rs,group_expr.rs,implementation.rs}` 中的 trait、引用类型、探索标记和缓存接口。
- Go 对照：`pkg/planner/cascades/old/optimize.go`；测试对照：`pkg/planner/cascades/old/optimize_test.rs` 与 `pkg/planner/cascades/old/optimize_test.go`。
- RustCodeGraph：状态显示索引包含 `optimize.rs`（17 个符号）；`query FindBestPlan`、`query implGroup`、`query preparePossibleProperties` 同时定位到 Rust 与 Go 对应实现。对 Rust `FindBestPlan`/`preparePossibleProperties` 的 callers/callees 查询返回空边，因此调用关系由 crate 再导出、源码内调用和全仓文本调用搜索补证，并在本文明确保留生产接线限制。
- 全仓调用搜索：除本文件外，Rust 只在 `optimize_test.rs` 调用 `preparePossibleProperties`，未检出生产端的 `DefaultOptimizer`/`FindBestPlan` 调用；Go 调用与内部链位于同路径 `optimize.go`，旧优化器入口未据此推断为 Rust 主链入口。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令检查文件存在且恰有 11 个固定二级章节，并人工复核上述结论均可回溯到列出的源码、图查询或测试。

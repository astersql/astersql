# `pkg/planner/core/recheck_cte.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate，由 [`pkg/planner/core/lib.rs`](lib.rs) 以公开模块 `recheck_cte` 装配。它位于逻辑计划完成构建之后、依赖 CTE 语义的优化规则运行之前：Rust 运行时构建入口 [`BuildBorrowedResultSetNode`](logical_plan_builder_runtime.rs) 在得到完整 `LogicalPlanRef` 后调用 `RecheckLogicalCTE`，重新计算整棵计划中共享 CTE 定义的 `IsOuterMostCTE` 标志。

这里的“最外层”不是指 CTE 节点在树中的深度，而是指该共享 CTE 是否只被主查询引用。若同一 CTE 定义还从另一个 CTE 的种子或递归部分被引用，它必须标为非最外层。该标志随后约束 CTE 谓词下推和部分派生谓词行为，相关消费者可见于 [`LogicalCTE::PredicatePushDown`](operator/logicalop/logical_cte.rs) 与 [`LogicalJoin`](operator/logicalop/logical_join.rs)。

## 核心职责

- 从主逻辑计划根开始遍历普通的 `Children_mut()` 边，并将这部分遍历上下文记为根查询树。
- 遇到 `LogicalCTE` 后，以其共享 `CTEClass.IDForStorage` 去重，并转而遍历该定义的 `SeedPartLogicalPlan` 与可选的 `RecursivePartLogicalPlan`；这两条边都属于 CTE 内部，因此后续发现的 CTE 引用使用非根上下文。
- 为每个可达共享 CTE 定义填写 `CTEClass.IsOuterMostCTE`。只要任何非根上下文引用被发现，该定义最终就保持 `false`，不受根引用和嵌套引用的发现顺序影响。
- 提供 Go 公共 API 名称 `RecheckCTE`，并保留 Rust 运行时较早接线所使用的兼容别名 `RecheckLogicalCTE`。

本文件只做标志重算，不构造、优化或替换逻辑算子，也不直接执行谓词下推。

## 主要符号

- `pub fn RecheckCTE(plan: &mut LogicalPlanRef)`：公共入口。创建本次遍历独占的 `HashSet<i32>`，以根查询上下文调用递归实现。参数要求可变计划引用，因为遍历既要访问可变子节点，也要更新共享 `CTEClass`。
- `pub fn RecheckLogicalCTE(plan: &mut LogicalPlanRef)`：兼容别名，仅转调 `RecheckCTE`。当前直接调用点是 [`logical_plan_builder_runtime.rs`](logical_plan_builder_runtime.rs) 中的 `BuildBorrowedResultSetNode`。
- `fn recheck_logical_ctes(plan: &mut dyn LogicalPlan, is_root_tree: bool, visited: &mut HashSet<i32>)`：私有深度优先遍历。它通过 `as_any_mut().downcast_mut::<LogicalCTE>()` 区分 CTE 读取节点和一般逻辑算子。
- `HashSet<i32>`：按 `CTEClass.IDForStorage` 记录已经展开过定义子计划的 CTE。它限制共享定义和递归引用的重复展开，但不会阻止后续引用先执行“嵌套引用置 false”的规则。
- `LogicalPlanRef` / `LogicalPlan`：由 `logicalop-dependency` 提供的盒装计划引用和对象安全 trait；`LogicalPlan::Children_mut` 暴露一般算子的可变子计划切片。
- `LogicalCTE`：由 `logicalop-dependency` 提供的逻辑 CTE 读取算子。多个读取节点可通过 `Rc<RefCell<CTEClass>>` 共享同一 `Cte` 定义。

## 执行流程

1. `RecheckCTE` 创建空的 `visited`，以 `is_root_tree = true` 进入根计划。
2. 若当前节点不是 `LogicalCTE`，递归遍历其全部 `Children_mut()`，并原样传递 `is_root_tree`。因此主查询普通子树保持 `true`，CTE 定义内部的普通子树保持 `false`。
3. 若当前节点是 `LogicalCTE`，先短暂不可变借用共享类，复制 `Rc` 并读取 `IDForStorage`，随后释放该借用。
4. 当 `is_root_tree == false` 时，立即把共享类的 `IsOuterMostCTE` 设为 `false`。该步骤发生在去重检查之前，确保已经从主查询访问过的定义在后来被嵌套 CTE 引用时仍会降级为非最外层。
5. 把存储 ID 插入 `visited`。若该 ID 已存在则返回，不再展开共享定义；此时第 4 步造成的 `false` 不会被覆盖。
6. 首次访问该 ID 时，把 `IsOuterMostCTE` 设为当前 `is_root_tree`。若首次就是嵌套引用则写入 `false`；若首次来自主查询则暂写 `true`，未来任何嵌套引用仍可按第 4 步改为 `false`。
7. 为避免持有 `RefCell` 可变借用跨越递归调用，函数从共享类中临时 `take()` 出种子计划和递归计划，释放借用后分别以 `is_root_tree = false` 递归。
8. 两个内部子计划处理完毕后，再次借用共享类，将原来的 `Option<LogicalPlanRef>` 放回对应字段并返回。CTE 节点自己的普通 `Children_mut()` 不在这一分支继续遍历；Go 实现同样把 CTE 定义边视为该节点的专用后继。

关键不变量是：一个存储 ID 的定义子计划最多展开一次，但每个到达的 CTE 读取节点都有机会把共享定义标成非最外层。

## 数据与状态

持久状态只有共享 `CTEClass.IsOuterMostCTE`；`visited`、`seed` 和 `recursive` 都是单次调用内的临时状态。

`visited` 的键是 `i32` 类型的 `IDForStorage`，所以算法依赖同一逻辑计划范围内不同 CTE 定义拥有不同存储 ID。若两个不相关定义错误地复用 ID，后访问定义的内部计划会被跳过；本文件不检测或报告此类上游不变量破坏。

`SeedPartLogicalPlan` 与 `RecursivePartLogicalPlan` 是共享类里的 `Option<LogicalPlanRef>`。递归期间它们暂时为 `None`，随后恢复为原对象；本函数不改变计划所有权、节点顺序或子树内容。`RecursivePartLogicalPlan == None` 表示非递归 CTE，直接跳过第二次递归。

`LogicalCTE.Cte` 是 `Rc<RefCell<CTEClass>>`，因此多个读取节点观察并修改同一个标志。算法按共享类而不是按读取节点存储结果。

## 依赖与调用关系

上游调用链：

- Rust：`PlanBuilder` 的多种语句构建路径进入 `BuildResultSetNode` 或 `BuildBorrowedResultSetNode`；后者构建完整结果集逻辑树，然后调用 `crate::recheck_cte::RecheckLogicalCTE(&mut plan)`。
- Go：[`pkg/planner/core/optimizer.go`](optimizer.go) 的 `BuildLogicalPlanForTest` 在构建出逻辑计划后调用 `RecheckCTE`；正式链路在 [`pkg/planner/optimize.go`](../optimize.go) 进入逻辑优化前调用它。Rust 当前直接接线位置与 Go 的“完整建树后、逻辑优化前”意图一致，但入口组织并非逐函数一一对应。

下游依赖：

- 标准库 `HashSet` 提供 O(1) 均摊去重；总遍历成本通常为 O(N + C)，其中 N 是普通逻辑计划边的访问量，C 是首次展开的 CTE 定义边，额外空间为不同存储 ID 数量及递归栈深度。
- `logicalop-dependency` 在 [`pkg/planner/core/Cargo.toml`](Cargo.toml) 中映射到本地 crate `astersql-planner-core-operator-logicalop`，提供 `LogicalPlan`、`LogicalPlanRef` 和 `LogicalCTE`；本文件没有 feature 条件编译，`nextgen` feature 也不改变这里的代码路径。
- [`LogicalCTE::PredicatePushDown`](operator/logicalop/logical_cte.rs) 仅在非递归且 `IsOuterMostCTE == true` 时收集可推入共享种子计划的谓词；[`LogicalJoin`](operator/logicalop/logical_join.rs) 也用该标志过滤对 CTE 子节点的派生谓词。因此错误分类可能改变优化结果，虽不应改变 SQL 语义。

RustCodeGraph 能识别 `RecheckLogicalCTE -> RecheckCTE -> recheck_logical_ctes`，但未解析私有递归函数内的动态 trait 调用，也没有返回 Rust 上游调用者；上述直接调用点由仓库文本搜索核对。

## 错误处理与边界

这些函数不返回 `Result`，也没有显式错误分支。空的普通子节点列表、缺失的递归部分以及重复 CTE 存储 ID 都通过自然返回处理。

边界行为如下：

- 根查询没有 CTE 时，只遍历普通逻辑树，不产生持久修改。
- 同一定义被主查询引用多次时，首次访问写入 `true`，后续根引用因已访问直接返回，结果保持 `true`。
- 同一定义只在另一个 CTE 内引用时，结果为 `false`。
- 同一定义同时被主查询和另一个 CTE 引用时，无论先遇到哪一种引用，嵌套引用都会在去重判断前写入 `false`，结果保持 `false`。
- 递归 CTE 通过存储 ID 去重终止重复展开；算法假定递归回边最终再次表现为相同存储 ID 的 `LogicalCTE` 引用。

潜在失败采用 Rust 运行时语义：违反 `RefCell` 动态借用规则会 panic，极深且非循环的计划也可能耗尽调用栈。本实现通过缩短借用作用域并临时取出内部计划，避免正常递归路径中的重入可变借用冲突，但没有将 panic 转换为规划错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。`Rc<RefCell<_>>` 表明该逻辑计划共享状态面向单线程所有权模型，并不提供跨线程同步。

`visited` 从公共入口创建并以可变借用贯穿一次同步深度优先遍历，调用返回即释放。临时取出的种子/递归计划由栈上 `Option` 独占持有，在正常返回路径中恢复到共享类；函数内部没有可提前传播的错误，因此恢复步骤不会被 `?` 或显式 `return` 绕过。若递归期间发生 panic，则没有 RAII 恢复守卫，尚未放回的字段可能保持 `None`，调用者不能把该函数视为 panic-safe 的事务性修改。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/planner/core/recheck_cte.go`](recheck_cte.go)：

- Go `RecheckCTE` 创建 `intset.FastIntSet` 并调用 `findCTEs(..., true)`；Rust 使用 `HashSet<i32>` 和 `recheck_logical_ctes(..., true)`。
- 两端都先识别 `LogicalCTE`，在非根上下文先写 `IsOuterMostCTE = false`，再检查存储 ID 是否访问过；首次访问时再把标志设为当前根上下文。这个顺序保留了“任何嵌套引用都使共享定义降级”的行为。
- 两端都把种子与递归部分作为 CTE 专用后继，并以非根上下文递归；非 CTE 节点则沿普通子计划传播当前上下文。
- Go 可直接递归共享类里的接口字段；Rust 因 `Rc<RefCell<_>>` 借用规则，必须先 `take()` 两个 `Option`、递归后再恢复。这是所有权实现差异，不是预期语义差异。
- Go 公共入口只叫 `RecheckCTE`。Rust 同时导出同名入口和仅作接线兼容的 `RecheckLogicalCTE`；新增调用应优先使用 Go 对齐名称，除非正在维护现有运行时接线。

Go 注释还说明这是完整采用 `Sequence` 优化 CTE 之前的临时方案。Rust 文件没有实现或替代该未来方案，因此应把当前逻辑视为对 Go 现状的移植，而不是更一般的 CTE 作用域分析器。

## 扩展指南

- 若新增一种能够引用共享 CTE 定义、但不是 `LogicalCTE` 的计划节点，应明确它属于普通子计划边还是 CTE 定义边，并相应扩展 `recheck_logical_ctes`；只把它挂入 `Children_mut()` 可能错误继承当前 `is_root_tree`。
- 若改变 CTE 身份规则，应同时检查 `CTEClass.IDForStorage` 的生成位置、`LogicalCTETable`/物理 CTE 的关联方式以及 Go `findCTEs`。不要轻易改为按 `Rc` 地址去重，否则会偏离 Go 的存储 ID 语义。
- 若移除 `RecheckLogicalCTE` 兼容别名，应同步修改 `logical_plan_builder_runtime.rs` 的直接调用，并确认所有外部 crate 是否使用公开模块；RustCodeGraph 当前没有发现其他调用者，但仍需文本搜索验证动态或未索引接线。
- 若给递归过程增加可失败操作，应先引入能保证种子/递归计划恢复的作用域守卫或重构所有权，避免 `?` 提前返回后把共享类字段留为 `None`。
- 测试应放在独立 Rust 测试文件中，不要内嵌到 `recheck_cte.rs`。最直接的回归测试应构造共享 `CTEClass` 的多个 `LogicalCTE` 读取节点，分别覆盖“仅主查询”“仅嵌套”“先根后嵌套”“先嵌套后根”“递归回引”和不同定义存储 ID 冲突保护，并断言标志及种子/递归计划均被恢复。
- 兼容和性能复核重点是：谓词下推结果是否仍与 Go 一致、嵌套 CTE 是否避免错误派生谓词、共享定义是否只展开一次，以及深层嵌套的递归栈风险。

## 验证依据

- 目标源码：[`pkg/planner/core/recheck_cte.rs`](recheck_cte.rs)，核对 3 个函数、去重顺序、`take()`/恢复流程及无条件编译事实。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/planner/core/recheck_cte.rs` 报告该文件 6 个符号；`node --file ...`、`query RecheckCTE`、`query RecheckLogicalCTE`、`query recheck_logical_ctes`、`callers`/`callees` 核对了入口和内部调用边。图未给出动态 trait 调用和 Rust 上游调用边，已用文本搜索补充，并在本文中注明限制。
- crate 与模块边界：[`pkg/planner/core/Cargo.toml`](Cargo.toml) 的包名、`logicalop-dependency` 和 feature 声明；[`pkg/planner/core/lib.rs`](lib.rs) 的 `pub mod recheck_cte`。
- 上游接线：[`pkg/planner/core/logical_plan_builder_runtime.rs`](logical_plan_builder_runtime.rs) 的 `BuildBorrowedResultSetNode`；Go 正式与测试入口分别为 [`pkg/planner/optimize.go`](../optimize.go) 和 [`pkg/planner/core/optimizer.go`](optimizer.go)。
- 数据和消费方：[`pkg/planner/core/operator/logicalop/logical_cte.rs`](operator/logicalop/logical_cte.rs) 的 `CTEClass`、`LogicalCTE`、`PredicatePushDown`；[`pkg/planner/core/operator/logicalop/base_logical_plan.rs`](operator/logicalop/base_logical_plan.rs) 的 `LogicalPlan`/`Children_mut`；[`pkg/planner/core/operator/logicalop/logical_join.rs`](operator/logicalop/logical_join.rs) 对 `IsOuterMostCTE` 的读取。
- Go 语义对照：[`pkg/planner/core/recheck_cte.go`](recheck_cte.go) 的 `RecheckCTE` 与 `findCTEs`。
- 测试证据：仓库搜索未发现 Rust 或 Go 测试直接调用 `RecheckCTE`/`RecheckLogicalCTE` 或直接断言 `IsOuterMostCTE`。相邻的 [`pkg/planner/core/casetest/tpcds/tpcds_test.rs`](casetest/tpcds/tpcds_test.rs) 覆盖嵌套 CTE 自连接能生成计划，但不直接证明本文件的各个状态分支；因此本文不把它视为该算法的完整回归覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的命令检查本文存在且恰有 11 个固定二级标题，并人工复核所有行为陈述均可回溯到上述符号或文件。

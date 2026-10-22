# `pkg/planner/core/operator/logicalop/logical_union_scan.rs` 逻辑说明

## 文件定位

`ExplainInfo`、生成文件中的 `Hash64`/`Equals` 以及 `LogicalPlan` trait 实现分别让该节点可解释、可在 Cascades Memo 中稳定判等、可参与通用逻辑优化流程。

## 核心职责

`LogicalUnionScan` 表示“快照扫描结果 + 当前事务尚未提交的本地修改”这一读己之写边界，也用于快照数据位于内存的本地临时表场景。它承担四项直接职责：

1. 通过 `Init` 建立类型名为 `UnionScan` 的逻辑计划基座，并保存查询块和规划上下文。
2. 通过 `PredicatePushDown` 将普通谓词交给唯一子计划，同时保留本地新增行仍需计算的条件；涉及虚拟列的谓词不下推到存储侧。
3. 通过 `PruneColumns` 确保父层输出列之外，句柄列、额外物理表 ID 列和条件引用列不会被子计划裁掉。
4. 通过 `PreparePossibleProperties` 继承首个子节点的有序列组合，但明确把 TiFlash 能力清为 `false`。

`ExplainInfo`、生成文件中的 `Hash64`/`Equals` 以及 `LogicalPlan` trait 实现分别让该节点可解释、可在 Cascades Memo 中稳定判等、可参与通用逻辑优化流程。

## 主要符号

- `EXTRA_PHYSICAL_TABLE_ID: i64 = -3`：额外物理表 ID 列的本地哨兵。`PruneColumns` 用列的 `ID` 与它比较，语义对应 Go 的 `model.ExtraPhysTblID`。
- `UnionScanProperties`：本文件为可能属性定义的轻量载体。`Orders` 是子节点可能提供的排序列组合，`HasTiFlash` 表示是否包含 TiFlash 路径；`PreparePossibleProperties` 对 UnionScan 总是返回 `false`。
- `LogicalUnionScan`：核心一元逻辑算子。`BaseLogicalPlan` 保存上下文、子节点、schema、输出名和统计等通用状态；`Conditions` 保存本地新增行需要重算的过滤条件；`HandleCols` 是定位、合并和去重行所需的主键或隐藏 row-id 列抽象。
- `Default::default`：建立空基座、空条件，并以 `planner_util::IntHandleCols::default()` 提供非空句柄对象。默认对象尚未初始化上下文，也没有子节点，不能直接执行需要唯一子节点的方法。
- `Init(self, ctx, query_block_offset)`：调用 `NewBaseLogicalPlan` 分配计划 ID，写入上下文、类型名 `UnionScan` 和查询块偏移。
- `ExplainInfo(&self)`：使用会话表达式求值上下文格式化条件，排序后输出 `conds:..., handle:...`，使显示结果不受条件输入次序影响。
- `PredicatePushDown(&mut self, predicates)`：划分虚拟列/普通谓词，递归下推普通谓词，处理子计划替换，并返回仍应留在上层的残差谓词。
- `PruneColumns(&mut self, parent_used_cols)`：汇总 UnionScan 自身必需列后调用唯一子节点的列裁剪；不以子节点裁剪后的 schema 覆盖自身输出元数据。
- `PreparePossibleProperties(&mut self, schema, children)`：清空基座缓存的 TiFlash 属性，复制首个子节点的 `Orders`，并返回 `HasTiFlash: false`。参数 `schema` 当前未使用。
- `impl LogicalPlan for LogicalUnionScan`：提供向下转型及基座访问，并显式将 trait 的 `PruneColumns` 路由到本文件的覆写版本。
- `hash64_equals_generated.rs` 中的 `LogicalUnionScan::Hash64/Equals`：不在本文件定义但属于该类型的直接语义配套；以条件和句柄列 `UniqueID` 判定 Memo 中的逻辑等价性。

## 执行流程

构造主链可由 `logical_plan_builder_runtime.rs` 的缓存表路径直接验证：规划器先保证数据源存在可用物理句柄；若没有显式句柄，会追加隐藏额外句柄列。随后它复制数据源的 schema、输出名和统计，以句柄构造并 `Init` 一个 `LogicalUnionScan`，再把原 `DataSource` 设为唯一子节点。WHERE 条件建立时，规划器还会把等价条件同时保留在 UnionScan 和父 `Selection`：前者过滤事务本地行，后者过滤快照侧结果。

逻辑优化期间，关键流程如下：

1. `PredicatePushDown` 按 `expression::ContainVirtualColumn` 分组。含虚拟列的表达式不能安全下推至 TiKV/TiFlash，暂存为上层残差。
2. 普通谓词经 `PredicatePushDownPlan` 传给下标 `0` 的子节点。该帮助函数允许子节点通过 `PredicatePushDownRoot` 原地替换整棵子树。
3. 若替换后的子节点是 `LogicalTableDual`，当前函数直接返回空残差，不再设置 `Conditions`；这避免空计划被 UnionScan 重新解释为可能产生本地行。
4. 否则，把普通谓词的克隆写入 `Conditions`，并将子节点未消化的残差与含虚拟列谓词合并后交给上层。普通条件同时存在于 UnionScan 与父层过滤是有意语义，而非重复优化遗漏。
5. `PruneColumns` 在父层所需列上追加句柄列、schema 中 `ID == -3` 的物理表 ID 列、以及 `Conditions` 引用的全部列，再要求唯一子节点保留这些列。
6. 可能属性推导复制第一个子节点的 `Orders`，同时把本节点和返回值的 TiFlash 标志清零。物理枚举随后拒绝 Flash 属性；对 root 属性则复制条件、句柄、schema 和统计，构造 `PhysicalUnionScan`。

## 数据与状态

`LogicalUnionScan` 自有的可变业务状态只有 `Conditions` 和 `HandleCols`；树结构及公共元数据存放在 `BaseLogicalPlan`。`Conditions` 使用拥有所有权的 `Vec<Expression>`：构造器和下推逻辑通过 `CloneExpr` 或 `clone` 保留独立表达式对象。`HandleCols` 是 `Box<dyn HandleCols>`，可容纳整数句柄和复合句柄实现；列裁剪与哈希判等都通过 `IterColumns`/`NumCols` 操作抽象接口。

本节点必须是一元算子。源码直接索引 `Children_mut()[0]`，所以“已 `Init` 但未 `SetChildren`”不是合法的优化状态。schema、输出名和统计在构造时从数据源复制；列裁剪只改变子计划，不重写 UnionScan 自身的输出 schema。`logical_union_scan_test.rs::prune_preserves_union_scan_output_metadata_like_go` 专门固定了这一不变量。

`UnionScanProperties` 当前不是 `BaseLogicalPlan` 内部缓存的同一类型，而是本文件用于传递排序/TiFlash 信息的载体。调用 `PreparePossibleProperties` 时，本文件还通过 `BaseLogicalPlan::PreparePossibleProperties(&[])` 清理基座的 `has_ti_flash` 缓存，防止先前派生状态泄漏。

## 依赖与调用关系

直接 crate 依赖可由 `Cargo.toml` 核对：本文件使用 `base` 的规划上下文、`expression` 的表达式/列/schema 工具，以及 `planner_util::HandleCols`/`IntHandleCols`；通过 crate 根的 re-export 使用 `BaseLogicalPlan`、`LogicalPlan`、`PredicatePushDownPlan`、`LogicalTableDual` 和统一 `Result`。

主要上游与下游关系为：

- 上游构造：`pkg/planner/core/logical_plan_builder_runtime.rs` 创建节点，复制数据源元数据并设置唯一子节点；同文件的 WHERE 处理把条件同步到 UnionScan。
- 通用优化入口：`LogicalPlan` trait 调用 `PredicatePushDown`/`PruneColumns`；`pkg/planner/cascades/memo/group_expr.rs` 显式转发这些可覆写方法，避免回落到 `BaseLogicalPlan` 默认行为。
- Memo 判等：`group_expr.rs::hash_logical_plan` 与 `equal_logical_plans` 下转为 `LogicalUnionScan`，调用生成的 `Hash64`/`Equals`。
- 下游谓词：`PredicatePushDownPlan` 调用子节点的根级谓词下推，并在需要时替换子计划；表达式工具检测虚拟列、提取条件列并格式化 EXPLAIN 文本。
- 下游物理化：`physical_union_scan.rs::ExhaustPhysicalPlans4LogicalUnionScan` 拒绝 Flash 属性，在 root 路径复制 `Conditions`、`HandleCols` 和 schema，生成 `PhysicalUnionScan`。
- 文件级 RustCodeGraph 结果把目标文件列为被 `group_expr.rs`、`logical_union_scan_test.rs`、`physical_union_scan.rs` 使用；索引没有生成可靠的方法级调用边，因此构造入口和 trait 路由由上述源码交叉验证。

## 错误处理与边界

`PredicatePushDown` 与 `PruneColumns` 都通过 crate 的 `Result` 传播子计划错误，不在本层吞掉或改写错误。`PredicatePushDownPlan` 若需要替换子树会完成原地更新；若子节点变成 `LogicalTableDual`，UnionScan 返回空残差。这一分支由 `pkg/planner/core/issuetest/planner_issue_test.rs` 的事务内 `a = null` 场景提供行为证据：谓词化简为空结果后 UnionScan 不得重新产生行。

两个优化方法都假定下标 `0` 存在，缺少子节点会因切片索引而 panic。`logical_union_scan_test.rs` 的 `prune_requires_a_child_like_go` 与 `predicate_pushdown_requires_a_child_like_go` 明确把这一点记录为与 Go 生命周期一致的前置条件，而不是可恢复的用户错误。

虚拟列谓词不会进入 `Conditions`，而是作为残差返回上层；原因是当前 UnionScan 不支持在其下方放置 Projection。普通条件仍需由父 Selection 保留以过滤快照行，`Conditions` 只服务于事务新增行。错误地消除父 Selection 会改变查询结果。

TiFlash/MPP 是明确边界：逻辑属性强制 `HasTiFlash = false`，基座 `CanSelfBeingPushedToCopImpl` 也排除 `UnionScan`，物理枚举遇到 Flash 属性会发出 MPP 警告并返回空候选。空上下文或无法接纳 index-join 属性时，物理枚举同样返回空候选。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄，也没有显式资源释放逻辑。优化方法要求 `&mut self`，在单次计划构建/优化期间原地修改节点；并发共享策略不由该类型提供。规划上下文的共享方式封装在 `base::ContextRef` 中，表达式与句柄的复制分别使用表达式克隆和 `CloneHandleCols`。

生命周期顺序是：`Default` 建立未接线对象，`Init` 安装上下文/类型/ID，构造器设置 schema、输出名、统计和唯一子节点，优化阶段更新条件及子树，物理枚举复制必要状态到新的物理节点。节点销毁依赖 Rust 所有权自动释放 `Vec`、`Box<dyn HandleCols>` 和基座子树。扩展时不应缓存指向 `Children_mut()` 或 `Schema()` 内部元素的跨调用借用，因为谓词下推可能整体替换子计划。

## 与 Go 版本的对应关系

Go 通过返回新的 `base.LogicalPlan` 和 `SetChild` 表示子树替换；Rust 的 `PredicatePushDownPlan(&mut child, ...)` 在盒装 trait 对象位置原地完成同一接线。Rust 返回值只包含残差谓词，因为节点自身通过可变借用保留。

列裁剪也保持 Go 的列集合：父使用列、全部句柄列、`ExtraPhysTblID` 列和条件列，然后递归裁剪子节点。Go 返回自身计划，Rust 原地修改并返回 `Result<()>`。

Rust 额外用 `UnionScanProperties` 表达返回载体，并显式清理 `BaseLogicalPlan` 的缓存布尔值。

针对本文件细节的 Rust 测试集中在 `logical_union_scan_test.rs`；同目录没有独立的 `logical_union_scan_test.go`，因此不能声称每个 Rust 单测都有一一对应的 Go 单测。

## 扩展指南

新增 UnionScan 逻辑时应先判断状态属于逻辑层还是物理/执行层：影响谓词、必需列或逻辑属性的修改进入本文件；影响行合并、排序保持、KV 范围或执行内存的修改应落在 `physical_union_scan.rs` 或执行器实现。

常见修改点及同步要求如下：

- 新增会影响结果等价性的字段时，必须同步 `hash64_equals_generated.rs` 的 `Hash64/Equals` 生成来源与结果，否则 Cascades Memo 可能错误合并计划；还要核对 `physical_union_scan.rs::ExhaustPhysicalPlans4LogicalUnionScan` 是否需要复制该字段。
- 新增条件种类或下推规则时，修改 `PredicatePushDown`，并验证事务本地行与快照行都只按预期过滤；尤其保留虚拟列屏障、父 Selection 语义和 TableDual 提前返回。
- 新增运行时必需列时，将其加入 `PruneColumns` 的 `used` 集合，并扩展同目录独立 Rust 测试，验证该列能传到子节点且 UnionScan 输出元数据不被覆盖。
- 调整排序或存储能力时，同时检查 `PreparePossibleProperties`、`BaseLogicalPlan::CanSelfBeingPushedToCopImpl` 和物理枚举的 Flash 分支；错误宣称 TiFlash 支持会导致无可执行候选或把 root-only 算子下推。
- 改变结构字段时还需检查 `logical_plan_builder_runtime.rs` 构造接线、`group_expr.rs` 的类型分派，以及 Go 对照文件和 `casetest/dag` 事务黄金场景。

兼容风险主要是读己之写错误、虚拟列求值位置变化和计划缓存/Memo 等价性错误；性能风险主要是漏裁剪导致扫描额外列、重复克隆表达式以及错误丢失排序属性导致额外 Sort。任何行为修改都应在 `logical_union_scan_test.rs` 增加聚焦回归，并按仓库测试流程补充真实事务规划用例。

# `pkg/planner/cascades/old/transformation_rules.rs`

## 文件定位

本文件是 `astersql-planner-cascades-old` crate 的逻辑等价变换规则集。crate 入口 `pkg/planner/cascades/old/lib.rs` 将本模块的公开项重新导出；`pkg/planner/cascades/old/Cargo.toml` 用 `package.metadata.porting.go-package = "pkg/planner/cascades/old"` 明确它来自同目录 Go 包。它不选择最终物理实现：逻辑探索由本文件提供的规则完成，物理实现规则和属性强制规则分别位于 `implementation_rules.rs` 与 `enforcer_rules.rs`。

真实入口在 `optimize.rs`：`Optimizer::NewOptimizer` 调用 `default_rule_batches()`，`Optimizer::onPhaseExploration` 依批次探索 memo，`Optimizer::findMoreEquiv` 根据根 `Operand` 取规则、绑定 `Pattern`、调用 `matches`/`on_transform`，再按擦除标志更新 `Group`。因此本文件处于“逻辑计划进入 memo”之后、“统计推导和物理实现”之前。

## 核心职责

1. 定义统一规则协议 `Transformation`、结果类型 `TransformResult` 和按根算子索引的 `TransformationRuleBatch`。
2. 用 `default_rule_batches` 固定 TiDB 层、TiKV 层、执行器约束收尾三轮次；轮次隔离由 `optimize.rs::onPhaseExploration` 的 `round`/`ExploreMark` 保证。
3. 实现 38 个 `Transformation`：谓词、Limit、TopN、Projection 的下推/合并/消除，访问路径枚举，聚合下推和改写，外连接消除，Apply 去关联以及相邻 Window 合并。
4. 提供 Rust 适配层：`expression` 模块封装 PB 可下推分类、常量传播和变量函数检测；`memo` 模块封装 `GroupRef`/`GroupExprRef`、类型化计划读取和 `ExprIter` 子节点访问；若干构造/克隆 trait 保留 Go 初始化、schema 与上下文语义。
5. 在改写时维护 schema、engine type、query block offset、输出名、访问条件与列 ID 等计划不变量，而不是只替换算子外形。

## 主要符号

- `Transformation`：规则接口。`get_pattern` 返回缓存模式；`matches` 检查模式无法表达的语义条件；`on_transform` 返回 `(新 GroupExpr 列表, erase_old, erase_all)`。`rule_id` 对具体 Rust 类型名做稳定于当前进程/构建的哈希，用于 `GroupExpr::AddAppliedRule` 去重。
- `TransformResult`：`logicalop::Result<(Vec<memo::GroupExprRef>, bool, bool)>`。`unchanged()` 表示无候选且不擦除；`rewritten()` 表示用候选替换当前绑定，但不清空整个 Group。
- `BaseRule`：持有 `pattern::Pattern`，默认 `matches=true`；具体规则通过组合而非继承复用它。
- `default_rule_batches`：依次返回以下三批。
  - `tidb_layer_optimization_batch`：Selection 根 7 条、Aggregation 根 5 条、Limit 根 6 条、Projection 根 3 条、TopN 根 4 条、Apply 根 2 条、Join/Window 根各 1 条。
  - `tikv_layer_optimization_batch`：`EnumeratePaths`；Selection 到 Gather/TableScan/IndexScan 的下推与合并；Aggregation/Limit/TopN 到 `TiKVSingleGather` 的下推。
  - `post_transformation_batch`：再次消除/合并 Projection，并用 `InjectProjectionBelowAgg`、`InjectProjectionBelowTopN` 预计算执行器不能直接消费的标量表达式。
- 扫描与存储下推：`PushSelDownTableScan`、`PushSelDownIndexScan`、`PushSelDownTiKVSingleGather`、`EnumeratePaths`、`PushAggDownGather`、`PushLimitDownTiKVSingleGather`、`PushTopNDownTiKVSingleGather`。
- TiDB 层重写：`PushSelDown{Sort,Projection,Aggregation,Join,UnionAll,Window}`、`TransformJoinCondToSel`、`TransformLimitToTopN`、`PushLimitDown{Projection,UnionAll,OuterJoin}`、`PushTopNDown{Projection,OuterJoin,UnionAll}`。
- 合并、消除与规范化：`EliminateProjection`、`MergeAdjacent{Projection,TopN,Selection,Limit,Window}`、`MergeAggregationProjection`、`EliminateSingleMaxMin`、`TransformLimitToTableDual`、`EliminateOuterJoinBelow{Aggregation,Projection}`、`TransformAggregateCaseToSelection`、`TransformAggToProj`。
- 去关联与收尾：`TransformApplyToJoin`、`PullSelectionUpApply`、`InjectProjectionBelow{TopN,Agg}`。
- 内部桥接：`memo::{GroupHandle,PlanHandle,ExprIterExt}` 提供带类型的 memo 访问；`NewLogical*`/`CloneLogical*` trait 负责按 Go 风格 `Init` 并恢复 schema；`GroupExprRuleExt` 写入应用规则和子 Group。

## 执行流程

1. `Optimizer::NewOptimizer` 取得三个规则批次；每批按 `Operand` 分桶，同一桶内顺序就是尝试顺序。
2. `Optimizer::exploreGroup` 深度优先探索子 Group，再把当前 `GroupExpr` 交给 `findMoreEquiv`。后者用规则的 `Pattern` 构造 `memo::ExprIter`，枚举同一根表达式的所有合法绑定。
3. 对每个绑定先执行 `matches`。例如聚合下推拒绝不支持的引擎/聚合，外连接相关规则检查 JoinType、唯一键或列引用，Window 合并要求 partition/order/frame 一致且列依赖安全。
4. `on_transform` 读取绑定算子和子 Group，构造新的逻辑算子与 `GroupExpr`。下推通常形成“上层残留条件 + 下层可执行条件”；合并规则压缩相邻算子；消除规则直接连接原孩子；路径枚举可以一次返回多个候选。
5. `findMoreEquiv` 延迟插入新表达式，避免在枚举绑定时改变 `Group` 的 `Vec` 索引。`erase_old` 在本轮结束后删除当前表达式；`erase_all` 立即清空整个 Group、插入确定更优的结果并结束当前处理。
6. 新候选使 Group 本轮重新标记为未探索，外层循环继续到本批次不再产生新表达式；随后进入下一批，最终交给统计推导和物理实现阶段。

典型分支如下：`PushSelDownIndexScan::on_transform` 合并 Selection 与旧 `AccessConds`，调用 `ranger::DetachCondAndBuildRangeForIndex` 重建 ranges；若访问条件未变化则不改写，若仍有 `RemainedConds` 则重包 Selection。`TransformApplyToJoin` 递归收集内侧 Group 的关联列，只有内侧不再引用外侧 schema 才降级为 Join。`MergeAdjacentLimit` 计算两个 `[offset, offset+count)` 窗口的交集；空交集生成零行 `TableDual` 并请求 `erase_all`。

## 数据与状态

- 规则对象的长期状态只有缓存的 `Pattern`；规则批次以 `HashMap<Operand, Vec<Box<dyn Transformation>>>` 保存。跨 Operand 的 HashMap 遍历顺序不构成语义，因为优化器按当前根 Operand 直接查桶；同一 `Vec` 内的顺序会影响候选产生顺序。
- 计划状态位于 memo：`Group.Equivalents` 保存等价表达式，`Group.Prop.Schema`、`EngineType` 和探索标记随改写维护。`GroupExpr.Children` 指向子 Group，而不是直接拥有完整树。
- 表达式和列通常通过 `CloneExpr`、`Schema::Clone` 复制；需要新计算列时由 session vars 的 `AllocPlanColumnID` 分配唯一 ID。Apply/Join 克隆还保留 hint、输出名、完整 schema 和相关列等字段。
- 访问路径规则维护 `AccessConds`、`Ranges`、`EqCondCount`、索引列及长度；聚合规则维护 mode、函数参数、group-by、hint 和 partial/final schema；TopN/Limit 维护 `Offset`/`Count`，有关加法采用与 Go `uint64` 相同的回绕语义（见 Rust 独立测试）。
- `expression::PushDownExprs` 通过 `PushDownContext::PbConverter().ExprToPB` 把条件分为 pushed/remained；当前参数名 `_store_type` 表明实际能力来自 converter/client 上下文，而不是在函数内分支判断 store type。

## 依赖与调用关系

上游调用链是 `optimize.rs::Optimizer::{NewOptimizer,onPhaseExploration,exploreGroup,findMoreEquiv}`；测试可用 `ResetTransformationRules` 注入规则子集。`lib.rs` 公开再导出本模块，因此 crate 外也能构造规则或自定义批次。

主要下游依赖可按职责分组：

- `astersql-planner-cascades-pattern`：`Operand`、`EngineType` 和模式树。
- `astersql-planner-memo`：Group、GroupExpr、ExprIter、模式绑定和候选插入。
- `astersql-planner-core-operator-logicalop`：所有逻辑算子及统一 planner 错误；`physicalop` 提供聚合能否下推等能力判断。
- `astersql-expression`/`aggregation`：表达式克隆、列抽取、常量传播、聚合 mode/descriptor；`parser-ast`/`parser-mysql` 提供函数名和类型常量。
- `astersql-util-ranger`：Table/IndexScan 访问条件拆分和范围构造；`planner-util`、`rule-util`、`coreusage` 提供 pushdown context、ByItems、唯一键/关联列等规则辅助。
- `astersql-kv` 与 `planctx`：TiKV 引擎标识、PB client 能力、session variables、表达式/range context 和列 ID 分配。

RustCodeGraph 的文件索引确认 `transformation_rules.rs`、`optimize.rs`、独立测试及 Go 对照均已入图；精确源码证据显示 `Optimizer::NewOptimizer` 调用 `default_rule_batches`，`findMoreEquiv` 调用 `rule.matches` 和 `rule.on_transform`。Cargo 清单则确认这些均为同一旧 Cascades crate 的直接路径依赖，不是动态插件。

## 错误处理与边界

- 可恢复的“不适用”必须返回 `unchanged()` 或让 `matches=false`，不能制造空候选后误删旧表达式。范围构造等真实错误用 `?` 传播并转换为 `logicalop::PlannerError`。
- `erase_old` 只表示新表达式优于当前绑定；`erase_all` 表示结果优于 Group 内所有候选。后者只用于可证明的情形，如零行 Limit 或相邻 Limit 的空窗口，误用会破坏搜索完备性。
- 多处 `expect`/类型化 downcast 依赖 Pattern 与 memo 不变量，例如 Group 必须有 schema、绑定的 Operand 必须与 `logical_*` 访问器一致。它们是内部契约，不是面向非法 SQL 的错误恢复边界。
- 下推必须保留不可执行条件：Table/IndexScan 和 Gather 规则均在存在残留条件时重建 Selection。变量读写函数、赋值语义、外连接 null-extension、相关列和非确定表达式等限制由相应 `matches`/辅助函数守卫。
- 整数窗口运算必须维持 Go 无符号整数语义；Rust 测试 `test_merge_adjacent_limit_wraps_offset_like_go_uint64` 专门防止 debug overflow 或“更安全但不兼容”的改写。
- `TransformResult` 不承载警告集合；表达式/ranger/plan context 内产生的错误或警告由其既有 API 管理。本文件不吞掉 `Result` 错误。

## 并发与资源生命周期

旧 Cascades 探索是同步、单线程地修改一个 memo。`memo::GroupHandle` 持有 `Rc<RefCell<Group>>` 对应的 `GroupRef`，其 `Deref`/`DerefMut` 使用 `unsafe` 原始指针；源码注释限定前提为 transformation 单线程执行、handle 持有 `Rc` 且 Group allocation 不被替换或移除。若将探索并行化，必须先移除或重新证明这一别名模型，不能直接把规则送入工作线程。

`optimize.rs` 的 `DefaultOptimizer` 是 `thread_local! RefCell<Optimizer>`，默认规则集不会跨线程共享。规则内没有后台任务、通道、锁、I/O 或事务；临时 `Vec`/`HashSet`、克隆表达式和新 Group 在一次同步调用内创建，所有权随后交给 memo。`findMoreEquiv` 的 pending 队列是重要生命周期边界：它在所有绑定枚举完毕后才插入，防止 `Group.Equivalents` 重排导致迭代失效。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/cascades/old/transformation_rules.go`。Rust 的 `Transformation` 对应 Go 同名接口，`TransformResult` 将 Go 的四返回值压入 Rust `Result`；`BaseRule` 对应 `baseRule`；三个批次的 Operand 分桶、规则成员与桶内顺序逐项一致。Rust 构造器保留 `NewRule...` 命名，以便审查时直接映射 Go 实现。

Rust 额外代码主要是所有权/类型适配，而非新优化语义：`GroupHandle`、`PlanHandle<T>`、`ExprIterExt` 代替 Go 指针和类型断言；`NewLogical*`/`CloneLogical*` 显式补回 Go `Init` 后隐含的 context/schema 字段；`unchanged`/`rewritten` 统一常见返回元组。`rule_id` 和 `AddAppliedRule` 用类型名哈希代替 Go 接口对象身份相关处理。

测试覆盖并不等价：Go 的 `transformation_rules_test.go` 通过 SQL 构建、预处理和探索管线，配合 `testdata/transformation_rules_suite_{in,out}.json` 覆盖聚合下推、谓词下推、TopN、Projection、外连接、去关联、注入 Projection、Window 等规则族。Rust 的独立 `transformation_rules_test.rs` 当前有 7 个聚焦测试，覆盖 PB 可编码分类、完整常量传播、零 Count Limit、相邻 Limit 的非空/空/回绕分支，以及 TopN 下推 Projection 时的常量排序项。未被 Rust 独立测试逐条覆盖的规则不能仅因已注册就宣称完整回归等价；其实现事实可由源码与 Go 对照确认，行为回归仍应补测试。

## 扩展指南

新增或修改规则时应按以下接点工作：

1. 为规则定义具体类型和 `NewRule...` 构造器，Pattern 尽量表达算子形状与 engine；仅把列依赖、JoinType、表达式性质等细粒度条件放进 `matches`。
2. 在 `on_transform` 中显式维护 schema、context、query block offset、output names、engine type 及算子特有字段；返回前判断“无实际变化”，避免 memo 重复候选或探索不收敛。
3. 把规则加入正确批次和根 Operand 桶。存储能力/访问路径属于 TiKV 批，TiDB 逻辑等价改写属于首批，执行器输入规范化属于 post 批；检查与相邻规则的顺序交互。
4. 同步更新独立测试 `transformation_rules_test.rs`，不要把测试内嵌回生产文件。至少覆盖 Pattern 匹配、拒绝分支、改写树形、schema/engine、残留条件、擦除标志和错误传播；再与 Go 的对应规则及 `transformation_rules_test.go`/golden 用例核对。
5. 修改 Apply/外连接/表达式下推时重点评估 NULL 语义、相关列、SET_VAR/GET_VAR、副作用和唯一键证明；修改 Limit/TopN 时检查 `u64` 回绕及 offset+count 边界；修改扫描规则时检查 range max size、前缀索引、remained conditions 与 TiFlash/TiKV engine 限制。
6. 性能风险主要来自规则循环、候选爆炸、深度递归收集相关列以及重复克隆表达式/schema。新增等价候选前应确认会被 applied-rule/等价检测收敛，并避免在 `matches` 中做无界昂贵工作。

## 验证依据

- 生产源码：`pkg/planner/cascades/old/transformation_rules.rs`，重点符号为 `Transformation`、三个 batch 构造函数、全部 38 个 `impl Transformation`、`expression`/`memo` 适配模块及 `unchanged`/`rewritten`。
- 真实调用入口：`pkg/planner/cascades/old/optimize.rs` 的 `Optimizer::NewOptimizer`、`onPhaseExploration`、`exploreGroup`、`findMoreEquiv`。
- crate 边界：`pkg/planner/cascades/old/Cargo.toml` 与 `pkg/planner/cascades/old/lib.rs`；目标目录无 `doc.go`，故没有额外包契约文件可读。
- Go 对照：`pkg/planner/cascades/old/transformation_rules.go`、`optimize.go`；Go 回归：`transformation_rules_test.go` 与 `testdata/transformation_rules_suite_{in,out}.json`。
- Rust 回归：`pkg/planner/cascades/old/transformation_rules_test.rs`；其 7 个测试名和覆盖差异已在“与 Go 版本的对应关系”中说明。
- RustCodeGraph：`status` 显示目标仓库索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/planner/cascades/old` 确认目标 Rust/Go/测试文件均入图；`query Transformation --kind trait` 定位本文件协议，`node --file` 核对规则批次、扫描下推、Apply 和 Window 代码，调用链再由 `optimize.rs` 精确源码节点核实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构校验要求本文恰有上述 11 个固定二级标题。

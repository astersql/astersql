# `pkg/planner/core/rule_predicate_push_down.rs`

## 文件定位

本文对应的真实源文件是 [`pkg/planner/core/rule_predicate_push_down.rs`](rule_predicate_push_down.rs)。它属于 `astersql-planner-core` crate（`pkg/planner/core/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/planner/core/lib.rs` 以公开模块 `rule_predicate_push_down` 暴露。它提供一套基于简化 `PlanNode`/`Expression` 数据模型的谓词下推规则，以及给分片索引访问条件追加前缀表达式的辅助函数。

当前实现是迁移中的局部基线，而不是 Go 规划器谓词下推子系统的完整等价实现：`pkg/planner/core/optimizer.rs::logicalOptimize` 的规则表会记录 `predicate_push_down`，但其 `normalize` 当前只实际处理 `projection_eliminate`，没有构造或调用 `PPDSolver`。已确认的 Rust 上游是直接调用 `PPDSolver::Optimize` 的独立测试与 case test。

## 核心职责

- `PPDSolver::Optimize` 从空谓词集合开始调用私有递归函数 `push`，消除遇到的 `Selection`，并将其条件下移。
- `push` 允许谓词穿过 `Projection`、`Sort`、`Limit`、`TopN`，在叶节点或其他语义屏障处把谓词附加到节点的 `conditions`。
- `exprPrefixAdder` 根据 `shardColumn` 与 `shardBits`，为命中该列的简化 `eq:`/`in:` 条件构造 `shard(...)` 条件。
- `addPrefix4ShardIndexes` 串行应用多个 adder；`addExprPrefixCond` 则就地更新一个 `AccessPath::access_conditions`。

这些职责都作用于本 crate 的简化类型（`pkg/planner/core/task.rs::{PlanNode, PlanKind, Expression}` 与 `pkg/planner/core/find_best_task.rs::AccessPath`），不能直接推出完整逻辑算子体系已接线。

## 主要符号

- `pub struct PPDSolver`：无字段、可 `Default` 构造的规则对象。
- `PPDSolver::Optimize(&self, p: PlanNode) -> (PlanNode, bool)`：公开入口；返回重写后的树，但第二个返回值固定为 `false`，保持 Go `PPDSolver` 的 `planChanged` 行为。
- `PPDSolver::Name(&self) -> &'static str`：返回稳定规则名 `predicate_push_down`。
- `pub struct exprPrefixAdder { shardColumn: usize, shardBits: u8 }`：分片列下标和分片位数。类型名沿用 Go 风格且公开，但字段不包含 Go 版本的上下文、原始条件、索引列数组或前缀长度。
- `exprPrefixAdder::addExprPrefix4ShardIndex`：当前无条件委托 CNF 路径。
- `exprPrefixAdder::addExprPrefix4CNFCond`：复制输入，并为同列且名称以 `eq:` 或 `in:` 开头的表达式追加派生项。
- `exprPrefixAdder::addExprPrefix4DNFCond`：若名称以 `or:` 开头，则按 `|` 拆成多个表达式；否则返回原表达式的克隆。它没有把每个 DNF 分支重新组合为带 shard 前缀的 CNF。
- `addPrefix4ShardIndexes`：用 `fold` 按 adder 顺序转换条件；后一个 adder会看到前一个 adder的输出。
- `addExprPrefixCond`：只替换 `AccessPath::access_conditions`，不修改 `index_filters`、`table_filters` 或其他路径属性。
- `fn push`：文件内私有递归实现，返回内部 `changed`，但公开入口丢弃该值。

## 执行流程

`PPDSolver::Optimize` 的执行顺序如下：

1. 以输入根节点和空 `incoming` 调用 `push`。
2. 若当前节点是 `Selection`，先把当前节点的 `conditions` 追加到 `incoming`；只有恰好一个孩子时才移除该节点并递归到孩子。
3. 非 Selection 节点先以 `incoming` 是否非空初始化内部 `changed`。
4. 叶节点接收全部 `incoming` 到自身 `conditions` 后返回。
5. `Projection`、`Sort`、`Limit`、`TopN` 会移出第一个孩子、递归下推，再把结果放回孩子列表末尾。
6. 其他非叶节点是屏障：若有传入谓词，则直接附加到当前节点；函数不会递归处理其孩子。
7. `Optimize` 返回改写后的树，并固定报告 `false`。

分片前缀流程与计划树流程彼此独立。批量入口从原条件克隆开始，逐个调用 `addExprPrefix4ShardIndex`；CNF 路径保留原条件顺序，并把匹配项对应的 `shard(<原名称>,<位数>)` 追加到尾部。

## 数据与状态

规则没有全局状态或内部可变缓存；`PPDSolver` 是零大小对象。计划重写取得 `PlanNode` 所有权，并通过 `Vec::append`、`remove(0)`、`push` 和 `extend` 移动/合并节点与条件。

重要顺序不变量是：外层 Selection 的条件先进入 `incoming`，递归遇到内层 Selection 后再追加其条件。因此嵌套选择最终在叶节点保持“外层在前、内层在后”；`rule_predicate_simplification_test.rs::nested_selections_collapse_without_losing_predicates` 对此断言为 `["le:a:10", "ge:a:1"]`。CNF 前缀构造不改写原项，而是在所有原项之后追加派生项。

`exprPrefixAdder` 的列通过 `Expression::column: Option<usize>` 比较；没有列下标或列不匹配的表达式不会生成前缀。`shardBits` 只被编码进字符串，没有在本文件校验范围或参与真实哈希计算。

## 依赖与调用关系

直接依赖只有两个本地模块：

- `crate::task::{Expression, PlanKind, PlanNode}` 提供简化表达式与计划树。
- `crate::find_best_task::AccessPath` 提供可被 `addExprPrefixCond` 就地更新的 `access_conditions`。

RustCodeGraph 将目标文件识别为 108 行并列出 `PPDSolver`、`Optimize`、`Name`、四个 shard 辅助入口和 `push`；精确查询也同时定位到 Go 同名符号。索引的调用边对这些 Rust 符号未返回可用的唯一边，因此用直接源码搜索补证：`pkg/planner/core/rule_predicate_push_down_test.rs`、`pkg/planner/core/casetest/rule/rule_predicate_pushdown_test.rs` 和 `pkg/planner/core/casetest/rule/rule_predicate_simplification_test.rs` 直接调用 `PPDSolver::Optimize`；目标 crate 内只有独立测试直接调用 shard 批量/DNF 辅助函数。

应用主链方面，`optimizer.rs::logicalOptimize` 包含相同规则名，但当前未调用本文件 API。这意味着本文件可由库消费者显式调用，却不能据现有源码声称 `DoOptimize` 会执行这里的下推动作。

## 错误处理与边界

本文件所有 API 都不返回 `Result`，也没有日志或恢复分支。内存分配等 Rust 进程级失败之外，转换被设计为无错误返回。

需要特别注意以下结构边界：

- 非 Selection 的可穿透节点假定至少有一个孩子，且只处理第一个孩子；若存在多个孩子，其余孩子保留，但递归结果会被追加到末尾，可能改变孩子顺序。
- Selection 只有一个孩子时才被消除；零个或多个孩子时会继续执行后续逻辑，且其自身条件已被移出。对多孩子 Selection，没有专门的合法性错误。
- 对 Join、Apply、Aggregation 等未列入可穿透集合的节点，谓词停在该节点；这里没有列引用分析、外连接语义推导、相关子查询处理或算子专用 `PredicatePushDown` 分派。
- `addExprPrefix4CNFCond` 依赖字符串前缀表达语义，可能把任何名称以 `eq:`/`in:` 开头且列匹配的占位表达式视为候选。
- DNF helper 只做字符串拆分；空片段、嵌套 OR、AND 分支、转义分隔符与真实表达式组合均未处理。

## 并发与资源生命周期

这里没有线程、异步任务、锁、通道、事务或外部句柄。所有输入要么按所有权传入（`PlanNode`），要么以不可变借用读取（条件和 adders），只有 `addExprPrefixCond` 接收 `&mut AccessPath` 并在调用期间独占修改其访问条件。

递归深度与连续可穿透/Selection 链长度成正比；条件复制与追加会产生新的 `Vec<Expression>`。多个线程可以独立使用各自的 `PPDSolver`、adder 和计划树，但本文件本身不提供跨线程共享机制。

## 与 Go 版本的对应关系

Go 对照为 `pkg/planner/core/rule_predicate_push_down.go`。名称和高层意图对应，但能力差异明显：

- Go `Optimize` 调用逻辑算子的 `lp.PredicatePushDown(nil)` 并传播 `error`，仍故意令 `planChanged` 为 `false`；Rust 固定返回 `false` 的兼容点有单测，但只在简化 `PlanNode` 上执行本地 `push`。
- Go `addPrefix4ShardIndexes` 从 `DataSource` 检查 `ContainExprPrefixUk` 和所有可能访问路径，只处理 `IsUkShardIndexPath`，失败时记录连接、库、表、索引并回退原条件；Rust 接收调用者预先构造的 adder 列表，没有 DataSource 筛选、日志或失败回退。
- Go `addExprPrefixCond` 通过索引元信息计算索引列/前缀长度并构造带 `PlanContext` 的 adder；Rust 只改写既有 `AccessPath::access_conditions`。
- Go CNF 路径调用 ranger 的 `AddExpr4EqAndInCondition`；Rust 用名称前缀和列下标生成占位字符串。
- Go DNF 路径会展开 OR，对 AND/EQ/IN 分支添加前缀，再组合回单个 DNF 表达式并传播错误；Rust 仅把 `or:a|b` 拆成列表，且 `addExprPrefix4ShardIndex` 当前不会选择该 DNF helper。

因此，Go 测试 `pkg/planner/core/logical_plans_test.go` 与 `pkg/planner/core/casetest/rule/rule_predicate_pushdown_test.go` 证明完整 TiDB 规划器的预期覆盖面，但不能作为 Rust 简化实现已经具备这些行为的证据。

## 扩展指南

若扩展计划树下推，最可能修改 `push`：先定义每种算子的谓词消费/穿透规则，再处理孩子数量与列引用映射，不能简单扩大 `matches!` 列表。应在独立的 `pkg/planner/core/rule_predicate_push_down_test.rs` 增加叶节点、语义屏障、异常孩子数、嵌套 Selection 和条件顺序用例；面向 SQL 的行为还应同步相应 case test，而不是把测试写进生产文件。

若补齐分片索引语义，应围绕 `exprPrefixAdder`、`addPrefix4ShardIndexes`、`addExprPrefixCond` 对齐 Go 的 DataSource/path 筛选、真实表达式求值、DNF 重组、错误回退及索引前缀长度。兼容风险包括 NULL/类型转换/排序规则语义和条件顺序；性能风险包括对每个路径或 adder 重复克隆整组表达式。

若要接入应用优化主链，应修改实际规则调度位置，使 `optimizer.rs::logicalOptimize` 调用本 solver（或统一到完整逻辑算子 API），并验证 trace、禁用规则和 flag 行为。仅保留相同规则名不能视为已接线。

## 验证依据

- 生产源码：`pkg/planner/core/rule_predicate_push_down.rs`（108 行）、`pkg/planner/core/task.rs`、`pkg/planner/core/find_best_task.rs`、`pkg/planner/core/optimizer.rs`、`pkg/planner/core/lib.rs`。
- crate 边界：`pkg/planner/core/Cargo.toml`；包名为 `astersql-planner-core`，库入口为 `lib.rs`，`package.metadata.porting.go-package` 指向 `pkg/planner/core`。该目录不存在 `doc.go`，因此无可读取的目标包契约文件。
- Rust 测试：`pkg/planner/core/rule_predicate_push_down_test.rs` 验证固定 `planChanged=false`、匹配列的 EQ/IN 前缀规则及非 OR 保留；`pkg/planner/core/casetest/rule/rule_predicate_pushdown_test.rs` 验证穿过 Projection 到 TableScan；`pkg/planner/core/casetest/rule/rule_predicate_simplification_test.rs` 验证嵌套 Selection 折叠及条件顺序。`pkg/planner/core/logical_plans_test.rs::test_predicate_push_down` 属于更广的 fixture 覆盖。
- Go 对照：`pkg/planner/core/rule_predicate_push_down.go`、`pkg/planner/core/logical_plans_test.go::{TestPredicatePushDown, TestJoinPredicatePushDown, TestOuterWherePredicatePushDown}`、`pkg/planner/core/casetest/rule/rule_predicate_pushdown_test.go`。
- RustCodeGraph：运行了 `status`、目标文件 `node --file`、`query PPDSolver`、`query predicate_push_down`、四个 shard 辅助符号查询以及 callers/callees 查询；索引确认符号与 Go 对照，但精确调用边没有给出可用于本文件的唯一结果，故上游调用以 `rg` 的直接源码命中复核。
- 按任务约束未运行 Cargo；本文只陈述静态源码与测试意图，不声称这些测试在本会话实际执行通过。

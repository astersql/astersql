# `pkg/planner/core/optimizer.rs`

## 文件定位

[源文件 `pkg/planner/core/optimizer.rs`](optimizer.rs) 属于 `astersql-planner-core` crate（声明见 `pkg/planner/core/Cargo.toml`），由 `pkg/planner/core/lib.rs` 以私有模块 `mod optimizer;` 编译进 crate。它在 `PlanNode` 这一紧凑、值拥有的计划树模型上提供一条可执行的优化流水线，主要用途是保存和验证从 Go `pkg/planner/core/optimizer.go` 迁移而来的核心控制流语义。

它不是当前 crate 对外的完整生产优化器入口：`lib.rs` 公开再导出的 `DoOptimize` 来自 `optimizer_runtime.rs`，而对本文件 `crate::optimizer::DoOptimize` 的直接 Rust 引用只在同 crate 的 `pkg/planner/core/optimizer_test.rs` 中出现。因而理解完整 SQL 规划主链时应从 `optimizer_runtime.rs` 继续阅读；理解紧凑模型中的阶段顺序、树变换与边界条件时，本文件是聚焦入口。

## 核心职责

- `DoOptimize` 编排逻辑规则、笛卡尔积门禁、物理化、后优化、总代价累计和追踪结果生成。
- `logicalOptimize` 根据 `flag` 的低八位和 `OptimizeOptions::disabled_rules` 决定哪些规则被记录并调用 `normalize`；当前真正改变计划的逻辑规则只有 `projection_eliminate`。
- `physicalOptimize`/`physicalize` 记录选择的物理优化路径，并自底向上把紧凑模型的 `PlanKind::Join` 改写为 `PlanKind::HashJoin`。
- `postOptimize` 负责连续 Selection 合并、空 UnionScan/单子 Lock 消除、可选并行 Apply 标记、可选运行时过滤器生成，以及 probe 次数传播。
- `iteratePhysicalPlan`、`transformPhysicalPlan` 和 `existsCartesianProduct` 提供可复用的树遍历、树重写和安全检查原语。

这些职责均针对 `common_plans.rs` 中的紧凑 `PlanNode`/`PlanKind`；它们不能替代 `optimizer_runtime.rs` 对 trait 对象逻辑/物理计划、统计信息、真实规则实现和代价搜索的处理。

## 主要符号

- `AllowCartesianProduct: AtomicBool`：进程级笛卡尔积开关，默认 `true`。`DoOptimize` 以 `Ordering::Relaxed` 读取；关闭时，逻辑优化后出现无等值条件的 `Join`/`HashJoin` 会返回错误。
- `initialMaxCores: u64 = 10_000`：与 Go 同名常量对应的占位值；本文件内没有读取者，不能据此推断当前紧凑优化流程会进行核心数或并发度计算。
- `OptimizeOptions`：调用级开关集合。`cascades` 只决定追踪字符串，`enable_runtime_filter` 和 `enable_parallel_apply` 控制后优化分支，`disabled_rules` 按规则名屏蔽逻辑步骤，`trace` 当前未被读取。
- `DoOptimize(&PlannerContext, u64, PlanNode, &OptimizeOptions) -> Result<(PlanNode, f64, Trace), String>`：公开于私有模块内的总入口；消费输入树，返回改写后的树、递归累计代价和追踪对象。
- `logicalOptimize`/`normalize`：内部逻辑阶段。规则名固定为列裁剪、键信息、解相关、谓词下推、聚合消除、投影消除、Join 重排和 TopN 下推；除恒等投影消除外，其余当前只留下追踪记录。
- `physicalOptimize`/`physicalize`：内部物理阶段。`cascades` 不触发独立搜索器，只选择 `Trace::AppendPhysical` 的 `"cascades"` 或 `"volcano"` 标签；所有紧凑 `Join` 都固定物理化为 `HashJoin { inner_child: 1, ... }`。
- `postOptimize` 及四个辅助函数：执行局部树压缩、功能标记和 `probe_count` 派生。
- `iteratePhysicalPlan`：先序只读遍历；访问器返回 `false` 只剪枝当前节点的后代，不会终止其他兄弟分支的遍历。
- `transformPhysicalPlan`：消费计划并后序改写，保证 transform 看到的当前节点已包含改写后的子节点。
- `existsCartesianProduct`：递归检查等值条件为空的 `Join` 或 `HashJoin`。
- `total_cost`：将每个节点的 `estimated_cost` 与全部后代相加，不做溢出、有限数或重复子树检查。

## 执行流程

1. `DoOptimize` 创建空 `Trace`，调用 `logicalOptimize`。
2. `logicalOptimize` 按固定数组顺序枚举八个规则。只有对应位开启且规则名不在 `disabled_rules` 时，才把名称追加到 `Trace::logical` 并调用 `normalize`。`normalize` 先递归子节点；对 `projection_eliminate`，仅当 Projection 恰有一个子节点且 `operator_info` 为空时提升该子节点。
3. 如果全局 `AllowCartesianProduct` 为假，`existsCartesianProduct` 在逻辑树上发现空等值条件 Join 后立即返回 `"cartesian product is unsupported"`，物理阶段不会执行。
4. `physicalOptimize` 在 `Trace::physical` 记录 `cascades` 或 `volcano`，随后 `physicalize` 自底向上将逻辑 Join 替换为 HashJoin，并固定右侧（索引 1）为 inner child。
5. `postOptimize` 先自底向上合并连续 Selection，再提升空条件 UnionScan 或单子 Lock。开启并行 Apply 时，`enableParallelApply` 给 Apply 的 `operator_info` 追加 `" parallel"`；遇到 Apply 后只递归 outer child（索引 0），避免在 inner 侧嵌套 Apply 上继续开启并行。
6. 开启运行时过滤器时，`RuntimeFilterGenerator::GenerateRuntimeFilter` 遍历当前计划生成过滤器；本文件不把过滤器挂回节点，只在根节点 `operator_info` 追加生成数量。
7. `propagateProbeParents` 从根的 `1.0` 开始写入每个节点的 `probe_count`。Apply、IndexJoin、IndexHashJoin、IndexMergeJoin 的 inner child（索引 1）使用 `父 probes × max(父 estimated_rows, 1.0)`，其他边原样传递。
8. `total_cost` 递归累计物理树的 `estimated_cost`，`crate::ToString` 生成最终计划摘要写入 `Trace::final_plan`，入口返回三元组。

## 数据与状态

计划状态由 `common_plans.rs::PlanNode` 按值拥有：`children: Vec<PlanNode>` 表示树结构，`kind` 表示算子，`estimated_rows`/`estimated_cost` 驱动 probe 传播和代价累计，`operator_info` 承载本文件追加的并行与运行时过滤器摘要，`probe_count` 是后优化派生值。因为没有共享子节点或 arena 引用，节点提升通过 `remove(0)` 和整值赋值完成，变换过程中原包装节点会被丢弃。

跨调用的唯一可变全局状态是 `AllowCartesianProduct`。`OptimizeOptions`、输入计划和返回的 `Trace` 都是每次调用独立的数据；`disabled_rules` 使用 `BTreeSet<String>`，成员匹配区分大小写并要求与硬编码规则名完全一致。`PlannerContext` 被传入 `postOptimize`，但参数名为 `_ctx` 且当前未读取；`OptimizeOptions::trace` 和 `initialMaxCores` 也处于未接线状态。

需要保持的不变量包括：可消除 Projection 必须恰有一个子节点；Lock 仅在恰有一个子节点时提升；inner probe 传播假定相关 Join/Apply 的 inner child 位于索引 1；`physicalize` 固定生成 `inner_child: 1`。构造不满足二叉形状的计划不会在这些位置统一报错，而可能仅跳过部分改写或保留无法对应真实执行器的索引。

## 依赖与调用关系

直接 Rust 依赖均来自同 crate：`PlanKind`、`PlanNode`、`PlannerContext` 定义于 `common_plans.rs`，`Trace` 定义于 `trace.rs`，`RuntimeFilterGenerator` 定义于 `runtime_filter_generator.rs`，最终字符串由 `stringer.rs::ToString` 生成。标准库依赖只有 `BTreeSet` 和原子布尔类型。

RustCodeGraph 对 `optimizer.rs::DoOptimize` 给出的下游边包括 `logicalOptimize`、`existsCartesianProduct`、`physicalOptimize`、`postOptimize`、`total_cost` 和 `AllowCartesianProduct`；源码还直接调用 `Trace` 的追加方法、`RuntimeFilterGenerator::GenerateRuntimeFilter` 与 `crate::ToString`。图查询对常见名称及路径行号存在歧义，因此上游关系又用精确文本检索核验：本文件入口只有 `optimizer_test.rs` 以 `DoOptimizeCompact` 别名调用，没有生产模块直接调用。

crate 边界方面，`Cargo.toml` 指定 `lib.rs` 为库入口、关闭自动测试与 doctest，并通过 `[package.metadata.porting].go-package = "pkg/planner/core"` 明确 Go 对照目录。本文件使用的紧凑模型都是 crate 内模块，不新增 Cargo feature 或第三方依赖；`nextgen` feature 对本文件没有条件编译分支。

## 错误处理与边界

`DoOptimize` 的错误类型是 `String`。当前可观察错误只有禁用笛卡尔积后的固定文本；`logicalOptimize` 和 `physicalOptimize` 虽返回 `Result`，现有实现没有产生错误的分支。这与完整生产优化器的结构化 planner error、规则执行失败、Cascades 构造/执行失败和物理实现失败不同。

`existsCartesianProduct` 把所有空等值条件的 `PlanKind::Join`/`HashJoin` 都视为笛卡尔积；紧凑 `Join` 不携带 Go 版本用于限制检查范围的 join type，因此无法复现 Go 仅针对 inner/left outer/right outer join 的精细判断。Projection 消除以 `operator_info.is_empty()` 作为恒等投影代理，未校验表达式或 schema。Selection 合并直接追加子条件，保持“父条件在前、子条件在后”的顺序。UnionScan 只有条件为空才消除，Lock 必须单子；缺少子节点不会 panic，但保持原节点。

运行时过滤器数量仅以文本追加到根 `operator_info`；重复对同一棵已优化树再次调用可能重复追加 `parallel` 或 `runtime-filters:*` 文本，本文件没有幂等保护。浮点 `estimated_rows`/`estimated_cost` 未验证 NaN、无穷或负数，结果会按 IEEE-754 规则传播。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。计划树由 `DoOptimize` 独占消费并在当前线程原地改写，递归函数的可变借用保证单次遍历中不会并发修改同一节点；`RuntimeFilterGenerator` 是调用栈内临时值，后优化结束即释放，仅其过滤器数量被写入根节点文本。

`AllowCartesianProduct` 可被其他线程同时修改。Relaxed 读取只保证原子性，不提供与其他内存状态的同步顺序；这里开关本身就是完整判定输入，因此当前用法不依赖额外 happens-before。一次 `DoOptimize` 只读取一次该开关，执行中途变化不会重新检查。

所有遍历都递归调用，时间复杂度通常为每阶段 O(n)，栈深度为计划树高度 O(h)。多个逻辑规则会各自完整遍历一次；深度异常的计划存在栈溢出风险。`total_cost` 和各变换假定结构为树；紧凑值模型不能表达共享节点，因此没有重复访问共享 DAG 或引用环生命周期问题。

## 与 Go 版本的对应关系

Go 对照是 `pkg/planner/core/optimizer.go`。两者保留了 `AllowCartesianProduct`、`initialMaxCores`、`DoOptimize`、逻辑优化、物理优化、后优化、并行 Apply、UnionScan/Lock 消除、通用遍历和笛卡尔积检查等命名与大体阶段顺序；`optimizer_test.rs` 中“inner 嵌套 Apply 不启用并行”和“IndexJoin inner probe 乘父行数”用例直接验证了两项迁移语义。

当前 Rust 文件是显式的“精简迁移基线”，与 Go 完整实现有重要差距：

- Go 根据会话变量选择真正的 Cascades 或 Volcano 优化器，并执行 flag 调整、FD 提取、规则对象、memo/implementation 与代价搜索；这里 `cascades` 只影响追踪标签，逻辑规则多数只记录名称，物理选择固定为 HashJoin。
- Go 的 `DoOptimize` 位于实际 planner 主链；Rust crate 的同名生产导出来自 `optimizer_runtime.rs`，本文件入口当前只受紧凑单元测试覆盖。
- Go `postOptimize` 还包含投影处理、列求值规避、MPP/fine-grained shuffle、执行器兼容、reuse chunk 等大量步骤；这里仅保留四类局部后处理。
- Go 的 UnionScan/Lock 消除会把锁语义和等待时间下推到 PointGet/BatchPointGet，并用通用 transform 删除包装；这里无锁上下文，只要满足局部形状就直接提升子节点。
- Go 笛卡尔积检查考虑 join type；这里紧凑 `Join` 无 join type，检查范围更宽。
- Go 的真实 runtime filter 会注册到物理 Join/Scan 和 reader 子计划；这里生成紧凑过滤器列表后只把数量写入根节点说明文本。

因此扩展时应先判断需求属于紧凑迁移模型还是完整运行时优化器，不能只修改本文件便宣称生产 SQL 行为已经对齐。

## 扩展指南

新增紧凑逻辑规则时，应同时修改 `logicalOptimize` 的规则顺序、flag 位约定和 `normalize` 的实际变换，并在独立测试文件中覆盖“启用、禁用、disabled_rules 屏蔽、非匹配计划不变”四类行为。不要把测试内嵌进 `optimizer.rs`；优先扩展同目录 `optimizer_test.rs`，复杂专项行为可沿用已有 `optimizer_*_test.rs` 独立文件模式。

修改树形变换时，应明确遍历序（先序或后序）、节点提升后的字段保留策略和多子节点形状。涉及 Apply/IndexJoin 时必须验证 outer/inner 索引和 probe 乘数；涉及 Projection 时必须补充表达式/schema 恒等条件，不能继续把空 `operator_info` 当作充分语义证明；涉及 UnionScan/Lock 时要评估锁信息是否需要下推，而不是机械删除节点。

若目标是生产优化器功能，应修改 `optimizer_runtime.rs` 及其真实逻辑/物理规则依赖，而非仅扩充本文件。需要同步检查 `lib.rs` 的公开再导出、`Cargo.toml` 的依赖/feature、Go `optimizer.go` 的对应流程和相邻独立测试。兼容风险集中在计划形状、条件顺序、join side、错误类型和追踪输出；性能风险集中在每条规则的全树遍历、深递归、运行时过滤器额外扫描及代价累计精度。

## 验证依据

- 源文件：`pkg/planner/core/optimizer.rs`，逐项核对全部 2 个模块级值、1 个结构体、4 个公开函数和 10 个内部函数，以及所有递归与条件分支。
- 数据与直接依赖：RustCodeGraph `node PlanNode`、`node common_plans.rs::PlanKind`、`node PlannerContext`、`node trace.rs::Trace`、`node RuntimeFilterGenerator`、`node GenerateRuntimeFilter`；定义分别落在 `common_plans.rs`、`trace.rs` 和 `runtime_filter_generator.rs`。
- 调用图：RustCodeGraph `query DoOptimize`、`query OptimizeOptions`、`query existsCartesianProduct`、`node DoOptimize`。`node DoOptimize` 明确列出本文件入口的主要下游调用；精确 `rg` 进一步确认紧凑入口的直接 Rust 调用仅位于 `optimizer_test.rs`。RustCodeGraph 的 `callers`/`callees` 在使用非唯一符号或路径行号时出现歧义/空输出，未将这些不可靠结果当作生产调用证据。
- crate 与接线：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。前者确认 crate、feature 和 Go porting 元数据，后者确认私有 `optimizer` 模块以及公开 `DoOptimize` 来自 `optimizer_runtime`。
- Rust 测试：`pkg/planner/core/optimizer_test.rs`，其中 `parallel_apply_does_not_enable_nested_inner_apply` 和 `probe_parent_counts_follow_index_join_inner_side` 直接调用本文件入口。`runtime_filter_generator_test.rs` 是运行时过滤器自身的相邻独立测试，但不直接覆盖 `postOptimize` 的开关接线。
- Go 对照：`pkg/planner/core/optimizer.go` 的 `doOptimize`、`DoOptimize`、`postOptimize`、`enableParallelApply`、`logicalOptimize`、`physicalOptimize`、`eliminateUnionScanAndLock`、`iteratePhysicalPlan`、`transformPhysicalPlan`、`existsCartesianProduct`，以及 `pkg/planner/core/optimizer_test.go`。对照只用于说明保留意图与已知差距，未把 Go 的完整能力推断为本文件已支持。
- 本任务是纯文档分析，依任务计划不运行 Cargo。交付结构以任务指定命令验证：目标文件存在，且固定二级标题恰好 11 个。

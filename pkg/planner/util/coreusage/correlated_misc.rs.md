# `pkg/planner/util/coreusage/correlated_misc.rs`

## 文件定位

本文档对应源码 [`correlated_misc.rs`](./correlated_misc.rs)。该文件属于 `astersql-planner-util-coreusage` crate，是规划器中“从计划树发现关联列，并按给定 Schema 规范化关联列”的公共工具。crate 根 `pkg/planner/util/coreusage/lib.rs` 通过 `pub use correlated_misc::*` 再导出这里的公开函数；`pkg/planner/util/coreusage/Cargo.toml` 表明它直接依赖计划基类 `base`、逻辑算子 `logicalop`、表达式 `expression` 和数据类型 `types`。

关联列表示内层子查询对外层查询列的引用。这里不负责构造、执行或解相关整个计划，而是为子查询重写、LATERAL/APPLY 构建、解相关规则和 Cascades 变换提供统一的“关联列发现与归一化”结果。目标源码没有模块级常量、结构体、枚举、trait 或条件 feature；它包含 2 个内部辅助函数、4 个公开函数，以及 1 个仅测试编译的辅助函数。

## 核心职责

1. `ExtractCorrelatedCols4LogicalPlan` 和 `ExtractCorrelatedCols4PhysicalPlan` 对计划树做前序遍历：先收集当前节点自身的关联列，再依次递归子节点，因此结果顺序稳定为“根、从左到右的各子树”。共同递归骨架是 `extract_correlated_cols_recursive`。
2. 逻辑计划收集需要按具体算子动态派发。`ExtractCorrelatedCols4LogicalPlan::own` 对 `LogicalAggregation`、`LogicalApply`、`DataSource`、`LogicalExpand`、`LogicalJoin`、`LogicalProjection`、`LogicalSelection`、`LogicalSort`、`LogicalTopN`、`LogicalWindow` 调用各自的 `ExtractCorrelatedCols`；未命中时回退到 `plan.base().ExtractCorrelatedCols()`。
3. `LogicalCTE` 是特殊边界：`extract_correlated_cols_from_cte` 读取 `SeedPartLogicalPlan` 和可选的 `RecursivePartLogicalPlan`，分别递归收集。该接线放在本 crate，是因为 `logicalop` crate 不能反向依赖 `coreusage`。
4. `ExtractCorColumnsBySchema` 只保留出现在目标 Schema 中的列，按 Schema 顺序去重，并把同一列的所有输入实例重绑到同一个 datum 槽。逻辑包装函数不解析物理下标；物理包装函数把输出列的 `Index` 设置为 Schema 中的位置。

## 主要符号

- `extract_correlated_cols_recursive<T, Own, Children>(plan, own, children) -> Vec<CorrelatedColumn>`：私有泛型 DFS。`own` 提供当前节点的列，`children` 提供借用的子节点；函数本身不依赖具体计划 trait，测试也复用它验证遍历顺序。
- `extract_correlated_cols_from_cte(&LogicalCTE) -> Vec<CorrelatedColumn>`：私有 CTE 适配器。通过 `RefCell::borrow` 读取 CTE 类，按种子计划、递归计划的顺序处理存在的分支。
- `ExtractCorrelatedCols4LogicalPlan(&dyn LogicalPlan) -> Vec<CorrelatedColumn>`：公开逻辑计划入口。除递归子树外，还承担具体逻辑算子的向下转型与自身列派发。
- `ExtractCorrelatedCols4PhysicalPlan(&dyn PhysicalPlan) -> Vec<CorrelatedColumn>`：公开物理计划入口，直接使用 `PhysicalPlan::extract_correlated_cols` 与 `PhysicalPlan::children`。
- `extract_correlated_cols_for_test<T>(...)`：`#[cfg(test)] pub(crate)` 测试钩子，只暴露通用递归骨架，不属于生产 API。
- `ExtractCorColumnsBySchema4LogicalPlan(&dyn LogicalPlan, &Schema)`：先递归收集逻辑计划，再以 `resolve_index = false` 调用通用 Schema 过滤函数。
- `ExtractCorColumnsBySchema4PhysicalPlan(&dyn PhysicalPlan, &Schema)`：先递归收集物理计划，再以 `resolve_index = true` 调用通用 Schema 过滤函数。
- `ExtractCorColumnsBySchema(&mut [CorrelatedColumn], &Schema, bool)`：公开的过滤、去重和共享槽位实现；也允许调用者已经从非树形结构收集好关联列后直接规范化。

## 执行流程

计划树入口的流程如下：

1. 包装入口接收逻辑或物理计划 trait object。
2. `extract_correlated_cols_recursive` 调用 `own(plan)` 取得当前节点的关联列。
3. 它按 `Children()` 或 `children()` 返回的顺序递归每个子节点，并把结果追加到当前向量，因此形成前序序列。逻辑 CTE 的自身收集会额外递归其种子和递归计划；不存在的 `Option` 分支由 `flatten()` 跳过。
4. `ExtractCorColumnsBySchema4*` 把收集结果作为可变切片交给 `ExtractCorColumnsBySchema`。
5. Schema 过滤先创建与 `schema.Len()` 等长的 `Vec<Option<CorrelatedColumn>>`。每个输入列经 `Schema::ColumnIndex` 查找；不匹配的列被忽略。
6. 某个 Schema 位置首次命中时，输出列复制自 `schema.Columns[index]`，并创建默认 `Datum` 的共享槽；每次命中都会把输入关联列的 `data` 改为该槽的克隆。因此重复出现的同一列和返回的代表列观察同一份执行期值。
7. 最后按 Schema 槽位顺序丢弃 `None`。物理模式把每个保留列的 `column.Index` 写成槽位下标，逻辑模式保留 Schema 列原有的 `Index`。

复杂度方面，树遍历与计划节点、已抽取列的总量线性相关；Schema 阶段对每个关联列调用一次 `Schema::ColumnIndex`，后者线性扫描 Schema，因此最坏时间为 `O(C × S)`，其中 `C` 是收集到的关联列数，`S` 是 Schema 列数。结果和中间槽位的额外空间为 `O(C + S)`，递归栈深度为计划树高度。

## 数据与状态

函数本身没有全局状态，也不持久化缓存。主要数据是 `expression::CorrelatedColumn { column, data }`：`column` 保存列身份和下标，`data` 是可选的 `CorrelatedDatum`。`pkg/expression/column.rs` 将 `CorrelatedDatum` 定义为 `Arc<RwLock<types::Datum>>`，用来表达 Go `*types.Datum` 的共享指针语义；执行阶段由外层行填充该槽。

`ExtractCorColumnsBySchema` 会原地修改输入切片中匹配列的 `data`，同时返回新的代表列向量。这是重要副作用：调用者不能把它当成纯过滤函数。列匹配依赖 `Schema::ColumnIndex`；该方法按 `UniqueID` 查找，优先完整列，若只有前缀列则回退到最后一个前缀位置。输出顺序由 Schema 决定，而不是由关联列首次出现的顺序决定。

## 依赖与调用关系

下游依赖如下：

- `logicalop::LogicalPlan` 提供 `as_any`、`base`、`Children` 和各具体逻辑算子的 `ExtractCorrelatedCols`。
- `base::PhysicalPlan` 提供 `extract_correlated_cols` 与 `children`。
- `expression::{CorrelatedColumn, Schema, NewCorrelatedDatum}` 提供关联列、匹配 Schema 和共享值槽；`types::datum::Datum::default()` 是新槽的初始值。

已核实的 Rust 上游包括：

- `pkg/planner/core/expression_rewriter.rs` 在比较子查询等重写中按外层 Schema 收集关联列，并用“是否仍存在关联列”决定是否需要构造 Apply。
- `pkg/planner/core/logical_plan_builder_runtime.rs` 为 LATERAL/CROSS APPLY 路径从右侧计划提取对左侧 Schema 的关联列。
- `pkg/planner/core/optimizer_runtime.rs` 在解相关后重新检查内侧计划；结果为空时，满足其他条件的 Apply 可改写为 Join。
- `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs` 在 Cascades 解相关规则中取得内侧对外侧 Schema 的关联列。
- `pkg/planner/cascades/old/transformation_rules.rs` 自行遍历 memo group 收集列，再直接调用 `ExtractCorColumnsBySchema` 规范化。

RustCodeGraph 与精确文本搜索均未发现 `ExtractCorrelatedCols4PhysicalPlan` 或 `ExtractCorColumnsBySchema4PhysicalPlan` 的有效外部 Rust 调用；`pkg/planner/core/operator/physicalop/physical_cte.rs` 中仅有注释掉的示意代码。因此物理入口当前是已实现、已公开但生产接线未验证的 API，不能据此声称物理计划主链已经使用它。Go 版本则由执行器 builder 和多个物理 reader/CTE 算子调用物理入口。

## 错误处理与边界

这些函数没有 `Result` 返回值，也不产生显式业务错误；空树节点自身无列、空子列表、空 Schema、没有匹配列时都会自然得到空向量。CTE 的种子或递归计划为 `None` 时被跳过。输入中的非 Schema 列保持原 `data` 不变。

主要边界与不变量是：

- 遍历没有环检测，要求逻辑/物理计划子节点关系构成有限无环结构；异常环会导致无限递归或栈溢出。
- `LogicalCTE.Cte.borrow()` 使用 `RefCell` 运行时借用检查；同一对象已有冲突的可变借用时会 panic，而不是返回错误。
- 新 datum 槽从 `Datum::default()` 开始；本文件只建立共享关系，不负责在表达式求值前填入外层行值。
- 去重按 Schema 位置进行，同一 Schema 列只返回一个代表对象；所有匹配输入实例被重绑到该代表对象的共享槽。
- `resolve_index` 只影响返回代表列的 `Index`，不会重写输入切片中各关联列的 `Index`。
- 逻辑派发是显式类型清单。新增一个覆写 `ExtractCorrelatedCols` 的逻辑算子但未加入清单时，会回退到基类实现，可能漏掉该算子的专有表达式。

## 并发与资源生命周期

遍历和 Schema 处理均为当前线程内的同步计算：不创建线程、任务、通道、事务、文件、网络连接或显式锁守卫。计划节点只被借用；递归完成后临时向量正常释放。

唯一可跨调用共享的资源是 datum 槽。`NewCorrelatedDatum` 创建 `Arc<RwLock<Datum>>`，本文件通过克隆 `Arc` 延长其生命周期；只要返回代表列或任一匹配输入列仍存活，槽就不会释放。实际读写锁只在后续执行/求值阶段获取，本文件不持锁。`CorrelatedColumn::SafeToShareAcrossSession` 在表达式模块中返回 `false`，说明这种执行期状态不能因使用 `Arc` 而被误认为可跨会话共享。

CTE 读取期间持有 `RefCell` 的不可变借用，并在 `extract_correlated_cols_from_cte` 返回时释放。递归调用期间该借用仍在作用域内，因此扩展代码应避免尝试对同一个 CTE 做可变借用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/coreusage/correlated_misc.go`。两版共同语义包括：递归前序收集、按 Schema 过滤、以 Schema 顺序返回唯一列、首次命中时创建 datum、让同列的所有出现共享 datum，以及仅在物理包装入口解析列下标。

Rust 为适配类型系统和 crate 边界做了以下显式转换：

- Go 递归直接调用接口的 `ExtractCorrelatedCols`；Rust 逻辑 trait 的具体算子行为需要在 `own` 中向下转型派发，并对未命中类型调用基类实现。
- Go 的 `LogicalCTE.ExtractCorrelatedCols` 位于算子实现内；Rust 因 crate 依赖方向在本文件用 `extract_correlated_cols_from_cte` 补接种子与递归计划。
- Go 用 `*types.Datum` 表示共享槽；Rust 用 `Arc<RwLock<Datum>>`，克隆 `Arc` 对齐指针共享，同时提供线程安全的内部可变性。
- Go 以 `-1` 表示 `ColumnIndex` 未命中；Rust 使用 `Option<usize>`。两者都跳过未命中的列。
- Go 先构造含 `nil` 的定长切片再原地压缩；Rust 使用 `Vec<Option<_>>` 后 `filter_map`，结果顺序和去重效果一致。

当前 Rust 版本相对 Go 的接线范围并不完全相同：已找到多个逻辑规划路径调用 Rust API，但物理入口未找到有效外部 Rust 调用。该差异属于迁移/接线状态，不应通过本文档推断为行为缺陷或擅自补建物理子系统。

## 扩展指南

- 新增会在自身表达式中持有关联列的逻辑算子时，先实现其 `ExtractCorrelatedCols`，再把具体类型加入 `ExtractCorrelatedCols4LogicalPlan::own` 的派发清单；同时在独立测试文件 `pkg/planner/util/coreusage/coreusage_aster_unit_test.rs` 增加覆盖，证明自身列和子树列均被收集。
- 调整 CTE 表示或增加其他非普通 `Children()` 所有权边时，应修改相应的特殊适配器，并验证可选分支、遍历顺序和重复访问风险。不要把图结构当普通树盲目递归。
- 修改 Schema 匹配或去重规则时，应保持三项契约：输出按 Schema 排序、同列只返回一次、所有输入出现共享返回列的 datum。必须同步对照 Go 文件，除非任务明确要求改变两端语义。
- 若接入物理生产路径，应先检查物理算子是否已在自身 `extract_correlated_cols` 中递归特殊的非 `children()` 计划字段，避免重复或漏收；随后增加独立物理计划测试，而不是把测试写进生产源文件。
- 若考虑优化 `O(C × S)` 的 Schema 查找，要保留 `Schema::ColumnIndex` 对完整列与前缀列的优先级语义；仅按 `UniqueID` 建哈希表可能改变重复/前缀列边界行为。
- 递归深度随计划高度增长。若未来支持极深或可能成环的计划图，应在此文件的递归骨架处统一引入显式栈或访问控制，并补充顺序与终止性测试。

兼容性风险主要是 Go/Rust 顺序、CTE 特殊边和 datum 共享语义漂移；正确性风险是新逻辑算子漏加入显式派发；性能风险集中在深递归和 `ColumnIndex` 的乘积复杂度。本任务只记录这些扩展点，不修改行为。

## 验证依据

- 目标实现：`pkg/planner/util/coreusage/correlated_misc.rs`，RustCodeGraph `node --file` 核实了全部 162 行和 10 个索引符号。
- crate 边界：`pkg/planner/util/coreusage/Cargo.toml`；模块导出与测试装配：`pkg/planner/util/coreusage/lib.rs`。
- Go 对照：`pkg/planner/util/coreusage/correlated_misc.go`。
- 数据语义：`pkg/expression/column.rs` 中的 `CorrelatedColumn`、`CorrelatedDatum`、`NewCorrelatedDatum`；`pkg/expression/schema.rs` 中的 `Schema::ColumnIndex`。
- trait 契约：`pkg/planner/core/base/plan_base.rs` 的 `PhysicalPlan`；`pkg/planner/core/operator/logicalop/base_logical_plan.rs` 的 `LogicalPlan`。
- 独立 Rust 测试：`pkg/planner/util/coreusage/coreusage_aster_unit_test.rs`。`recursive_collection_keeps_go_preorder` 验证前序顺序；`cte_collection_includes_seed_and_recursive_parts` 验证 CTE 两部分；`schema_deduplication_shares_datum_and_resolves_physical_index` 验证 Schema 顺序、去重、槽位共享、非匹配列不变和物理下标。
- 上游调用证据：`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/logical_plan_builder_runtime.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/cascades/old/transformation_rules.rs`、`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs`。
- RustCodeGraph 已执行 `status`、目标目录 `files`、目标文件与调用点 `node`、主要符号 `query`、`explore` 及 `callers`/`callees`。精确调用搜索用于消除同名图查询噪声，并确认物理入口没有有效外部 Rust 调用。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收以任务文件给定的 11 个固定二级标题检查为准。

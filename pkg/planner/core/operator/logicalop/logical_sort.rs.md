# `pkg/planner/core/operator/logicalop/logical_sort.rs`

## 文件定位

本文件定义逻辑计划中的 `LogicalSort`，表示 SQL `ORDER BY` 在逻辑优化阶段的排序节点。它位于 `astersql-planner-core-operator-logicalop` crate；`lib.rs` 以 `mod logical_sort` 纳入模块并用 `pub use logical_sort::*` 对外导出。该 crate 的 `Cargo.toml` 通过 `[package.metadata.porting].go-package` 明确对应 Go 包 `pkg/planner/core/operator/logicalop`，并直接依赖 `base`、`expression`、`planner_util` 与 `rule_util` 等本地 crate。

上游的 Rust `PlanBuilder::buildSortWithCheck`（`pkg/planner/core/logical_plan_builder.rs`）会建立 `PlanKind::Sort`、保存排序项并挂接一个输入节点；当前文件则提供面向完整逻辑算子接口的 `LogicalSort` 实现。下游物理化入口 `ExhaustPhysicalPlans4LogicalSort`（`pkg/planner/core/operator/physicalop/physical_sort.rs`）读取这里的 `ByItems`、Schema、统计信息和查询块偏移，枚举真正执行排序的 `PhysicalSort`，以及可利用输入有序性的 `NominalSort`。

## 核心职责

- 用 `LogicalSort { BaseLogicalPlan, ByItems }` 保存单输入排序节点及有序的排序键；每个 `ByItems` 同时携带表达式和 `Desc` 方向。
- 用 `Init` 建立类型名为 `"Sort"` 的基类状态并分配计划 ID。
- 用 `ExplainInfo`、`ReplaceExprColumns`、`ExtractCorrelatedCols` 和 `GetUsedCols` 提供展示、表达式重写和依赖分析能力。
- 用 `PruneColumns`/`pruneSortByItems` 删除无效排序键，并把父节点及剩余排序表达式需要的列传给输入节点。
- 用 `PushDownTopN` 把 Sort 与上层 Limit/TopN 合并或消去本 Sort，继续向唯一子节点下推。
- 用 `PreparePossibleProperties`/`getPossiblePropertyFromByItems` 把连续的纯列排序前缀暴露给后续物理计划选择，同时传递子树是否包含 TiFlash 路径的信息。

本文件不执行行排序，也不编码执行器参数；那些职责属于 `physicalop::PhysicalSort`、`NominalSort` 以及执行器层。

## 主要符号

- `SortProperties { Orders: Vec<Vec<Column>>, HasTiFlash: bool }`：本文件用于可能有序属性传递的值对象。`Orders` 中每个元素是一组有序列前缀；`HasTiFlash` 从第一个子属性传入。
- `LogicalSort { BaseLogicalPlan, ByItems }`：核心逻辑算子。`BaseLogicalPlan` 持有上下文、节点 ID、子节点、Schema、统计和缓存状态；`ByItems` 是排序语义本身。
- `LogicalSort::Init(self, ctx, offset) -> Self`：调用 `NewBaseLogicalPlan(ctx, "Sort", offset)` 初始化基类。
- `LogicalSort::ExplainInfo(&self) -> String`：在可用时取表达式求值上下文，对各排序表达式调用 `StringWithCtx`；降序项追加 `:desc`，再用逗号连接。
- `LogicalSort::ReplaceExprColumns(&mut self, replace)`：逐项调用 `rule_util::ResolveExprAndReplace`。实现以 `std::mem::take` 暂时取走列表，避免在遍历时同时借用并修改同一字段。
- `LogicalSort::PruneColumns(&mut self, parent_used_cols) -> Result<()>`：规范化排序项，合并所需列，并递归裁剪第一个子节点。
- `LogicalSort::PushDownTopN(&mut self, top_n) -> Option<LogicalPlanRef>`：处理无上层 TopN、纯 Limit 和已有排序 TopN 三种情况。
- `LogicalSort::PreparePossibleProperties(&mut self, _schema, infos) -> SortProperties`：从排序键导出至多一组有序列前缀，并透传首个子属性的 `HasTiFlash`。
- `LogicalSort::ExtractCorrelatedCols` / `GetUsedCols`：分别收集外层相关列和全部普通列引用；均保持按 `ByItems` 遍历所得的顺序，不在此处去重。
- `impl LogicalPlan for LogicalSort`：提供向下转型、基类访问和 trait 级 `PruneColumns` 分派；其他通用行为由 `BaseLogicalPlan` 提供。
- `pruneSortByItems(items) -> (kept, used)`：文件级公开辅助函数，执行表达式哈希去重、运行时常量裁剪、NULL 类型排序项裁剪和所需列收集。
- `sortExpressionHasNullType(expr)`：仅识别 `Column`、`CorrelatedColumn` 与 `ScalarFunction` 的静态 NULL 类型。
- `getPossiblePropertyFromByItems(items) -> Vec<Column>`：用 `map_while` 提取从首项开始的连续纯 `Column` 前缀，遇到第一个非列表达式立即停止。

## 执行流程

1. 构建阶段创建 Sort 并按 SQL 顺序写入排序表达式及升降序方向，然后把原计划设为其单一输入。完整 Go 入口是 `PlanBuilder.buildSortWithCheck`（`logical_plan_builder.go`）；Rust 的轻量构建入口位于同名 `.rs` 文件。
2. 初始化完整 `LogicalSort` 时，`Init` 将上下文、`"Sort"` 类型名和查询块偏移写入 `BaseLogicalPlan`。
3. 列裁剪阶段调用 `PruneColumns`：
   - `pruneSortByItems` 按表达式 `HashCode` 仅保留第一次出现者；方向不参与这里的去重判断。
   - 无列依赖且属于运行时常量的表达式被删除；无列依赖但不是运行时常量的表达式被保留，以免错误删除非确定性表达式。
   - 有列依赖且可识别为静态 NULL 类型的表达式被删除；其余项贡献的列加入 `used`。
   - 父节点已用列与 `used` 合并后传给第一个子节点。若节点暂时没有子节点，Rust 实现安全返回 `Ok(())`。
4. TopN 下推阶段：传入 `None` 时委托基类；传入纯 Limit 时先把当前 Sort 的 `ByItems` 复制进 TopN，使其成为有序 TopN；传入本来就带排序键的 TopN 时直接沿用上层键。两种 `Some` 情况都继续交给第一个子节点，因此当前 Sort 不再保留在返回树中；缺少子节点则返回 `None`。
5. 属性准备阶段从 `ByItems` 提取连续纯列前缀。前缀为空则不声称任何 `Orders`；非空时包装为唯一一组可能顺序。物理枚举随后以 `MatchItems` 检查请求属性是否与排序键前缀及方向一致，并在 Root 路径生成 `PhysicalSort`/可选 `NominalSort`，在 MPP 路径只尝试 `NominalSort`。

## 数据与状态

`LogicalSort` 自身只新增 `ByItems`；Schema、子节点、统计信息、查询块偏移、上下文和 TiFlash 缓存等均在 `BaseLogicalPlan` 中。`ByItems` 的顺序有语义：前面的键优先级更高，`Desc` 控制每一项方向。`ExplainInfo` 和物理计划构造都按原顺序消费该列表。

`PruneColumns` 会原地替换 `ByItems`，但不会更改 Schema 或统计信息；它收集的 `used` 可以包含重复列，因为本层职责是保证依赖不被裁掉，而不是规范化列集合。`PreparePossibleProperties` 返回新的 `SortProperties`，没有把 `HasTiFlash` 写回 `BaseLogicalPlan`；这一点与 Go 实现将值缓存到 `ls.hasTiFlash` 的内部状态布局不同。

生成文件还为该类型补充了相邻能力：`hash64_equals_generated.rs` 的 `Hash64`/`Equals` 只基于 `ByItems`，`shallow_ref_generated.rs` 的 `LogicalSortShallowRef` 克隆排序项并把基类重置为默认值，`ByItemsShallowRef` 则用于写时复制排序项。这些符号不定义在当前文件，但修改字段语义时必须同步考虑。

## 依赖与调用关系

上游与接线关系：

- `logicalop/lib.rs` 声明并重新导出本模块，同时在 `#[cfg(test)]` 下接入独立的 `logical_sort_test.rs`。
- `pkg/planner/core/logical_plan_builder.rs::buildSortWithCheck` 是 Rust 计划构建路径中的 Sort 创建点；Go 的完整对照入口是 `logical_plan_builder.go::PlanBuilder.buildSortWithCheck`。
- `LogicalPlan` trait 通过 `PruneColumns` 分派到本类型；`BaseLogicalPlan` 提供子节点、上下文、Schema、统计与默认递归行为。
- RustCodeGraph 的目标文件级索引显示该文件被 planner/cascades、planner/core、逻辑基类及其他优化器相关文件使用；精确探索还识别出基类/投影的 `PushDownTopN` 调用链和逻辑裁剪分派。

直接下游依赖：

- `expression`：表达式字符串化、列/相关列抽取、运行时常量判断、表达式静态类型和 `HashCode`。
- `rule_util::ResolveExprAndReplace`：列裁剪或规则重写后同步替换排序表达式中的列引用。
- `BaseLogicalPlan`/`LogicalPlan`/`LogicalTopN`：计划树生命周期和 TopN 下推协议。
- `physicalop::ExhaustPhysicalPlans4LogicalSort` 与 `NominalSort::FromLogical`：消费逻辑 Sort 的排序项、上下文、统计和 Schema，形成物理候选。

RustCodeGraph 对若干精确方法的独立 `callers`/`callees` 命令未返回方法级边，因此本文没有据此虚构直接调用者；调用关系以索引的 `explore`/文件级 `used by` 结果及上述入口源码交叉核验。

## 错误处理与边界

- 本文件唯一可传播的错误路径是 `PruneColumns` 调用子节点 `PruneColumns`；使用 `?` 原样向上传递 `PlannerError`。其余方法返回普通值或 `Option`，不创建错误文本。
- `ExplainInfo` 允许 `SCtx()` 缺失：此时向 `StringWithCtx` 传入空上下文。独立测试用默认基类也能验证降序后缀。
- `PruneColumns` 只访问第一个子节点；无子节点时成功返回。Sort 的正常不变量仍是单输入，扩展或构造路径应维持这一点。
- `PushDownTopN(Some(...))` 也只访问第一个子节点；无子节点返回 `None`。传入 `None` 时当前基类实现不会递归遍历子树，而是返回 `None`，因此调用方不能把它理解为“返回未变化的 Sort”。
- 去重只比较表达式 `HashCode`，不比较 `Desc`。相同表达式的后续排序项即使方向不同也会被删除，因为前一项已经决定该表达式的顺序。
- NULL 类型判断仅覆盖三种具体表达式类型；其他表达式即使最终求值为 NULL，也不会由 `sortExpressionHasNullType` 在此处删除。Go 版本则通过通用 `Expr.GetType(evalCtx)` 判断，这是需要关注的迁移差异。
- 属性推导要求连续纯列前缀；排序键中出现标量函数或其他表达式后，后面的列不会被越过并加入可能属性。这保证所声明的前缀确实能从当前排序顺序推出。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、网络或文件资源。所有方法都在优化器持有的可变计划树上同步执行。

资源生命周期主要体现为所有权转换：`ReplaceExprColumns` 和 `PruneColumns` 用 `std::mem::take` 临时取得 `ByItems` 所有权，再把结果写回；`PushDownTopN` 按值取得 `LogicalTopN`，必要时克隆当前排序项，然后把它装箱传给子节点；`PreparePossibleProperties` 克隆列形成独立返回值。`LogicalPlanRef` 是计划树引用封装，具体共享/所有权策略由逻辑计划基类定义，而当前文件没有额外同步保证。

## 与 Go 版本的对应关系

Rust `logical_sort.rs` 基本逐项对应 `logical_sort.go`：`LogicalSort` 字段、`Init`、`ExplainInfo`、`ReplaceExprColumns`、`PruneColumns`、`PushDownTopN`、`PreparePossibleProperties`、`ExtractCorrelatedCols`、`GetUsedCols` 和 `getPossiblePropertyFromByItems` 都有同名或直接等价实现。

已核实的语义一致点包括：降序 Explain 项带 `:desc`；列替换深入相关列表达式；纯 Limit 吸收 Sort 的排序键；已有 TopN 下推时当前 Sort 可消去；可能属性只采用连续纯列前缀；列裁剪按表达式哈希去重，并保留无列依赖但非运行时常量的表达式。Rust 独立测试 `logical_sort_test.rs` 直接覆盖前三类中的 Explain、相关列替换和 NULL 列裁剪；物理层测试 `physical_sort_test.rs::root_sort_enumerates_physical_and_nominal_candidates_like_go` 覆盖 Root 物理候选枚举。

需要明确的实现差异：

- Go 的 `pruneByItems` 是 `logical_plans_misc.go` 中供 Sort、TopN、Aggregation 共用的辅助函数，并通过通用 `GetType(evalCtx)` 判定 NULL；Rust 在本文件内实现 `pruneSortByItems`，其 NULL 判定限于列、相关列和标量函数的静态类型。
- Go `PreparePossibleProperties` 把 `HasTiFlash` 同时写入基类缓存并返回指针；Rust 返回值对象 `SortProperties`，只从 `infos.first()` 读取该标志，不更新基类字段。
- Go `PruneColumns` 假定第一个子节点存在并返回逻辑计划本身；Rust 返回 `Result<()>` 且对无子节点容错。Go `PushDownTopN` 返回逻辑计划接口，Rust 返回 `Option<LogicalPlanRef>`。
- Rust 的轻量 `logical_plan_builder.rs` 使用通用 `PlanNode` 表示 Sort，不等同于 Go 构建器完整的 AST 重写、DISTINCT 校验与 `LogicalSort.Init` 接线；不能仅凭该轻量入口宣称完整 Go 构建链已经一比一移植。
- Rust 生成的 `Hash64`/`Equals` 基于 `ByItems`，对应 Go 生成代码的排序项语义；Go 还区分 nil 与空切片，而 Rust `Vec` 没有 nil 状态。

## 扩展指南

- 新增排序节点字段时，先判断它属于计划基类状态还是排序语义；若影响等价性、缓存键或写时复制，必须同步检查 `hash64_equals_generated.rs`、`shallow_ref_generated.rs` 及各自生成器/测试。
- 修改排序键裁剪时，应优先改 `pruneSortByItems` 与独立 `logical_sort_test.rs`，覆盖重复键、相反方向重复键、无列运行时常量、非确定性无列表达式、普通列、相关列、静态 NULL 标量函数及子节点错误传播。不要把测试内嵌回生产源文件。
- 若要把 NULL 类型判定扩展到更多表达式，应与 Go `pruneByItems` 的通用类型语义对齐，并确认取得求值上下文是否会改变当前无上下文可构造性。
- 修改 TopN 下推协议时，需同时核对 `LogicalTopN::IsLimit`、`BaseLogicalPlan::PushDownTopN`、投影/Limit/Join 等相邻算子以及计划树返回值约定，尤其覆盖缺少子节点与 `None` 输入。
- 修改可能属性推导时，必须维持“只声明连续可证明前缀”的不变量，并同步检查 `physical_sort.rs::MatchItems`、`ExhaustPhysicalPlans4LogicalSort` 与 `nominal_sort.rs::FromLogical`，防止错误跳过真实排序或误选 MPP 路径。
- 修改展示格式或列替换逻辑时，扩展 `logical_sort_test.rs`；修改物理候选行为时，扩展独立的 `physical_sort_test.rs`。Go 侧对应语义应继续以 `logical_sort.go`、`logical_plans_misc.go` 和 `logicalop_test/hash64_equals_test.go` 为对照。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件的 `node --file` 完整读取到 230 行，并报告被 25 个文件使用。
- RustCodeGraph 精确符号查询：确认 `LogicalSort`、`pruneSortByItems`、`ExplainInfo`、`PruneColumns`、`PushDownTopN`、`PreparePossibleProperties`、Go `PlanBuilder::buildSort`、Rust `logical_plan_builder.rs::buildSort`、`ExhaustPhysicalPlans4LogicalSort` 与 `NominalSort::FromLogical` 的定义位置。精确 `callers/callees` 未产生方法级输出，该限制已在依赖章节披露。
- 已读生产源码：`pkg/planner/core/operator/logicalop/logical_sort.rs`、`lib.rs`、`Cargo.toml`、`base_logical_plan.rs`、`logical_plans_misc.rs`、`hash64_equals_generated.rs`、`shallow_ref_generated.rs`，以及 `pkg/planner/core/logical_plan_builder.rs`、`pkg/planner/core/operator/physicalop/physical_sort.rs`、`nominal_sort.rs`。目标包不存在 `doc.go`，因此无额外包契约可读。
- 已读 Go 对照：`pkg/planner/core/operator/logicalop/logical_sort.go`、`logical_plans_misc.go`、`hash64_equals_generated.go` 和 `pkg/planner/core/logical_plan_builder.go`。
- 已读独立测试：`pkg/planner/core/operator/logicalop/logical_sort_test.rs`、`pkg/planner/core/operator/physicalop/physical_sort_test.rs` 与 Go `pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文没有把轻量构建器、缺失图边或 Go 行为差异描述成已验证的一致实现。

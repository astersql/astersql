# `pkg/planner/core/operator/logicalop/logical_window.rs`

## 文件定位

本文件说明的真实源文件是 [`logical_window.rs`](./logical_window.rs)。它定义 Rust 规划器中的逻辑窗口算子及其窗口帧数据模型，对应 SQL 的 `OVER (PARTITION BY ... ORDER BY ... frame)`。它属于 crate `astersql-planner-core-operator-logicalop`；`pkg/planner/core/operator/logicalop/lib.rs` 以私有模块 `logical_window` 装配，并通过 `pub use logical_window::*` 向规划器其余部分导出类型。该 crate 的 `Cargo.toml` 没有 feature 开关，依赖通过 `crate::*` 间接提供表达式、逻辑计划基类、属性和错误类型。

在完整链路中，`pkg/planner/core/logical_plan_builder_runtime.rs` 的窗口构建逻辑把 AST 帧边界转为本文件的 `FrameBound`/`WindowFrame`，为每个窗口函数创建 `WindowFuncDesc` 和结果列，再构造 `LogicalWindow`。优化器通过 `LogicalPlan` trait 调用其谓词下推、列裁剪和统计推导；`pkg/planner/core/operator/physicalop/physical_window.rs::ExhaustPhysicalPlans4LogicalWindow` 再读取逻辑窗口的函数、分区、排序、帧和 TiFlash 检查结果，枚举物理窗口计划。

本文件不是执行器：它保存并改写逻辑计划信息，不逐行计算窗口函数。真正的物理计划和 tipb 帧序列化位于 `physical_window.rs`；Rust 本文件的 `WindowPB`/`ToPB` 只是轻量摘要，不等同于 Go 版本 `FrameBound.ToPB` 的 tipb 转换。

## 核心职责

- 用 `FrameType`、`BoundType`、`RangeCmpDataType`、`FrameBound` 和 `WindowFrame` 表示 ROWS/RANGE/GROUPS 帧、起止方向、偏移及 RANGE 比较所需表达式。
- 用 `WindowFuncDesc` 表示窗口函数名和参数，用 `LogicalWindow` 聚合函数列表、`PARTITION BY`、`ORDER BY`、可选帧以及 `LogicalSchemaProducer`。
- 实现窗口节点的优化器协议：`PredicatePushDown`、`PruneColumns`、`DeriveStats`、`ExtractColGroups`、`ExtractCorrelatedCols` 和 `PreparePossibleProperties`。
- 在投影消除等重写发生时，由 `ReplaceExprColumns` 同步替换函数参数、分区/排序列和帧边界表达式，并清除被重建标量函数的缓存哈希。
- 提供克隆、哈希、相等性、结果列、分区键和 TiFlash 可执行性等辅助接口，供 memo、规则和物理化阶段消费。

当前实现有意保留的事实边界是：`WindowPB` 仅含函数名、列 ID 和帧类型；`PreparePossibleProperties` 仅委托基类传播子节点 TiFlash 标记；`GetPartitionKeys` 返回普通 `Column`，并未像 Go 版本一样构造带 collation ID 的 MPP 分区列。这些都不应被解读为 Go 能力已完整移植。

## 主要符号

- `FrameType::{Rows, Range, Groups}`：窗口帧计量方式，默认值为 `Range`。
- `BoundType::{Preceding, CurrentRow, Following}`：边界相对当前行的位置，默认值为 `CurrentRow`。
- `RangeCmpDataType::{Int, Real, Decimal, Time, Duration, Unsupported}`：RANGE 边界比较的粗粒度类型，默认 `Unsupported`。
- `FrameBound`：保存方向、无界标志、数值偏移、`CalcFuncs`、`CompareCols`、比较函数名、比较类型和显式 RANGE 标志。`UpdateCmpFuncsAndCmpDataType` 把三种方向分别映射为字符串 `le`/`eq`/`ge`；`Hash64` 与 `Equals` 使用表达式规范哈希比较内容。
- `WindowFrame`：组合帧类型及可选的 `Start`/`End`。`Equals` 比较两侧完整边界；其本地 `Hash64` 只混合帧类型、边界 `Num` 和 `BoundType`，不是边界所有字段的完整哈希。
- `WindowFuncDesc`：仅保存 `Name` 和 `Args`；返回类型等更丰富描述由上游 aggregation 描述符在构建阶段处理。
- `WindowPB`：`LogicalWindow::ToPB` 的简化返回值，包含函数名、分区/排序列 `UniqueID` 及可选帧类型。
- `LogicalWindow`：本文件主类型。`Init` 设置计划类型名 `"Window"`；`Clone` 复制窗口数据、schema 和输出名；`LogicalPlan` impl 把 trait 调用转发给同名固有方法。
- `option_bound_equals`：比较两个可选边界；对表达式使用 `HashCode()` 序列，而 `FrameBound::Equals` 使用 `CanonicalHashCode()`，维护时需留意这两个相等性入口并非同一实现。
- `replace_window_expr`：按列 `UniqueID` 递归替换列和标量函数参数；常量等其他表达式直接克隆。

另有 `pkg/planner/core/operator/logicalop/hash64_equals_generated.rs` 为 `LogicalWindow` 提供 `Equals`：它先比较本文件的 `Hash64`，再逐项比较窗口函数名与参数。因为本文件的窗口哈希并未纳入帧边界的全部字段，新增相等性字段时必须同时审查生成实现，不能假定 `equalFrame` 会被自动调用。

## 执行流程

1. `logical_plan_builder_runtime.rs::build_window_bound` 把 AST 的 PRECEDING/CURRENT ROW/FOLLOWING 转为 `BoundType`。ROWS 偏移写入 `Num`；显式 RANGE 构造加减或日期函数写入 `CalcFuncs`，设置 `CompareCols`、`IsExplicitRange`，并调用 `FrameBound::UpdateCmpFuncsAndCmpDataType`。
2. `build_window_frame` 组合 `FrameType` 与两侧边界。窗口分组构建逻辑把 aggregation 层描述符压缩成 `WindowFuncDesc`，在子 schema 末尾追加一列窗口结果，然后 `LogicalWindow::Init`、`SetSchema`、`SetOutputNames`、`SetChildren` 完成单子节点计划。
3. 谓词下推时，`PredicatePushDown` 收集 `PartitionBy` 列 ID。一个谓词只有在至少引用一列且所有引用列都属于分区键时才下推；常量谓词、窗口结果谓词及混合引用谓词保留在窗口之上。下推后对子节点调用 `PredicatePushDownPlan`，再用 `AttachSelectionToPlan` 挂回残余条件。
4. 列裁剪时，`PruneColumns` 从父层需求中去掉窗口结果列，追加窗口函数参数、帧表达式、分区列和排序列，按 `UniqueID` 排序去重后裁剪子节点。随后以子 schema 为前缀、原窗口结果列为后缀重建自身 schema。
5. 重写阶段可调用 `ReplaceExprColumns`。列叶子按 `UniqueID` 替换；标量函数递归复制参数并执行 `CleanHashCode`；帧的 `CalcFuncs` 和 `CompareCols` 与函数参数、分区/排序列保持同步。
6. `DeriveStats` 在允许复用时返回缓存；否则要求恰有可访问的第一个子节点，继承其统计，并把每个窗口结果列的 NDV 设为子节点 `RowCount`。物理化阶段由 `ExhaustPhysicalPlans4LogicalWindow` 使用 `checkComparisonForTiFlash`、虚拟/相关表达式检查和属性约束决定是否产生 MPP 方案，同时总会按所需排序属性考虑 Root 方案。

## 数据与状态

`LogicalWindow` 的核心不变量是 schema 尾部的列与 `WindowFuncDescs` 一一对应；`GetWindowResultColumns` 按函数数量从 schema 尾部切片，`PruneColumns` 和 `DeriveStats` 都依赖这一约定。该函数使用 `saturating_sub`：若 schema 列数意外少于函数数，不会立即越界，而会把整个 schema 当成窗口结果列，因此构建者必须维持列数和顺序。

列身份以 `Column.UniqueID` 为主：谓词下推、列裁剪去重、替换映射、分区集合相等和 PB 摘要均依赖它。`equalPartitionBy` 按 Go 的集合语义忽略顺序和升降序，但先要求两个列表长度相同；`equalOrderBy` 则逐项比较列和 `Desc`，顺序具有语义。

`FrameBound::Clone`、`WindowFrame::Clone` 和派生的 `Clone` 复制拥有的数据。独立测试验证 `CompareCols` 中的表达式对象不是原对象地址；`LogicalWindow::Clone` 还重新初始化基类并复制 schema、浅复制输出名。统计缓存存放在基类中，`DeriveStats(reload = false)` 会直接复用，不会重新扫描窗口描述。

`CmpFuncs` 在 Rust 中是函数名字符串而非 Go 的比较函数指针；当前每个受支持类型只生成一个方向相关名称。`RangeCmpDataType::Unsupported` 会清空该向量。`WindowPB` 是拥有型快照，不维护回指或资源句柄。

## 依赖与调用关系

上游直接证据包括：

- `logical_plan_builder_runtime.rs` 创建 `FrameBound`、`WindowFrame`、`WindowFuncDesc`、`LogicalWindow`，是 SQL AST 到本文件数据结构的主要入口。
- `optimizer_runtime.rs::replace_plan_expr_columns` 对 `LogicalWindow` 下转型后调用 `ReplaceExprColumns`；同文件的优化流程还识别窗口节点并提取相关列。
- `logical_selection.rs` 识别 `Selection(Window(DataSource))`，读取 `OrderBy`、`GetPartitionBy` 和 `GetWindowResultColumns` 推导分区 TopN。
- `planner/cascades/memo/group_expr.rs` 将 `LogicalWindow` 纳入 memo 相等性和属性准备；`planner/cascades/pattern/pattern.rs` 将其映射为窗口模式。

下游直接依赖包括：

- `LogicalSchemaProducer`、`BaseLogicalPlan`、`LogicalPlan`、`PredicatePushDownPlan` 和 `AttachSelectionToPlan` 提供计划树、schema、统计与优化器协议。
- `expression::{ExtractColumns, ExtractCorColumns}` 提取普通列和相关列；表达式的 `HashCode`/`CanonicalHashCode` 支持边界相等性和哈希。
- `property::{SortItem, StatsInfo, GroupNDV}` 提供分区/排序项和统计数据。
- `physical_window.rs::ExhaustPhysicalPlans4LogicalWindow` 消费逻辑字段并调用 `checkComparisonForTiFlash`、`GetPartitionKeys`；物理窗口文件还负责将逻辑帧转换为 tipb。

RustCodeGraph 将目标文件识别为含 66 个符号的已索引文件，并显示它被 58 个文件使用；精确查询可区分 `logical_window.rs::LogicalWindow` 与 Go 同名类型。此次索引的 `callers/callees` 对目标 Rust 方法未返回具体边，因此上述调用边由仓库范围的 Rust 引用搜索及相邻源码逐项核实。

## 错误处理与边界

显式返回错误的主要路径是 `Result`：`PredicatePushDown` 传播子节点下推和残余 Selection 附着错误；`PruneColumns` 传播子节点裁剪错误；`DeriveStats` 在没有第一个子节点时返回 `PlannerError("LogicalWindow requires one child")`，并传播子节点统计错误。

`PredicatePushDown` 和 `PruneColumns` 对缺少子节点的处理比 `DeriveStats` 宽松：前者跳过下推，后者仍会把 schema 重建为仅包含窗口结果列。这是当前代码行为，不是合法窗口可无子节点的保证。`GetWindowResultColumns` 只按尾部数量切片，不验证结果列类型或与描述符的逐项对应。

`checkComparisonForTiFlash` 的当前规则是：显式 RANGE 的任一边界只要 `CmpDataType == Unsupported` 就拒绝；非显式 RANGE 或无帧则通过。Go 同路径实现检查的是 Duration 与 DateTime/Timestamp 的互不支持组合，因此两者判定维度并不相同。

`replace_window_expr` 只递归识别列和标量函数；其他表达式类型直接克隆。若表达式系统新增内部含列的新节点，必须扩展此函数或提供统一重写接口，否则列替换可能遗漏。相等性/哈希还有三套相关入口（`FrameBound`、`WindowFrame`、生成的 `LogicalWindow::Equals`），修改字段时漏改任一处都可能影响 memo 去重。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。所有操作都是对计划节点及其拥有的 `Vec`、`HashMap`、`HashSet` 和表达式对象进行同步内存变换；`&mut self` 保证单次改写具有独占访问。

计划生命周期由树所有权管理：`LogicalWindow` 通过基类持有子节点，`PruneColumns` 和谓词下推原地修改第一个子节点，`Clone` 创建不含共享可变借用的新窗口值。表达式替换采取复制后修改的方式，并清除标量函数哈希缓存，避免旧缓存跨重写存活。文件内没有需要调用者显式释放的资源。

需注意“资源生命周期”不等于“缓存永远有效”：统计缓存受 `reload` 控制，表达式哈希缓存受 `CleanHashCode` 控制。增加新的原地表达式修改路径时，应同步处理缓存失效。

## 与 Go 版本的对应关系

Go 权威对照为 `pkg/planner/core/operator/logicalop/logical_window.go`。Rust 已对应的主干语义包括：窗口节点及帧模型、只下推分区列谓词、裁剪后恢复“子列 + 窗口结果列”的 schema、结果列 NDV 取行数、分区集合相等、排序逐项相等、相关列提取和列替换。

已经由直接源码确认的差异包括：

- Go `WindowFuncDescs` 使用完整的 aggregation 描述符，Rust `WindowFuncDesc` 仅保留名字与参数。
- Go `FrameBound::UpdateCompareCols` 可按求值类型插入 cast 并选择真实比较函数；Rust 构建器直接设置比较列，本文件只记录字符串比较名和枚举类型。
- Go `FrameBound.ToPB` 生成 `tipb.WindowFrameBound` 并可失败；Rust `LogicalWindow::ToPB` 仅生成无错误的 `WindowPB` 摘要，真正 tipb 转换在 `physical_window.rs`。
- Go `PreparePossibleProperties` 返回 `PARTITION BY + ORDER BY` 的候选顺序；Rust 方法仅传播 TiFlash 标志。
- Go `GetPartitionKeys` 产生带 collation ID 的 `MPPPartitionColumn`；Rust返回 `Vec<Column>`，由物理窗口代码再从列类型推导 collation 名称。
- Rust `ExtractCorrelatedCols` 同时扫描 `CalcFuncs` 与 `CompareCols`；当前 Go 实现只扫描窗口参数和 `CalcFuncs`。
- Rust 拒绝下推不引用列的谓词；Go 使用 `ExprFromSchema` 判断，行为不能仅凭本文件假定完全相同。
- Go TiFlash 检查聚焦 Duration/DateTime 组合；Rust聚焦显式 RANGE 是否具有受支持的 `CmpDataType`。

测试对应关系也保持独立文件：Rust 的 `logical_window_test.rs` 验证分区集合语义；`logical_relational_aster_unit_test.rs` 验证谓词下推及帧/排序比较；`logicalop_test/hash64_equals_test.rs` 和 `logicalop_test/logical_operator_test.rs` 验证哈希、相等和深克隆。相应 Go 测试位于 `logicalop_test/hash64_equals_test.go` 与 `logicalop_test/logical_operator_test.go`。

## 扩展指南

- 新增帧字段时，同时修改 `Default`/`Clone`、`FrameBound::Hash64`、`FrameBound::Equals`、`option_bound_equals`、`WindowFrame` 的哈希/相等、生成的 `LogicalWindow::Equals` 接线以及物理层 tipb 转换；应增加独立 Rust 测试，不能把测试嵌入本源文件。
- 新增表达式承载位置时，将其加入 `extractUsedExprs`、`extractUsedCols`、`ExtractCorrelatedCols` 和 `ReplaceExprColumns`，否则列裁剪、相关子查询识别或投影消除会丢失依赖。
- 修改 schema 规则时维持“子节点列在前、每个 `WindowFuncDesc` 的结果列在尾部”不变量，并覆盖零函数、多个函数及父层只引用部分列的情况。
- 扩充 TiFlash 能力时同步审查 `checkComparisonForTiFlash`、`contains_virtual_window_expression`、`ExhaustPhysicalPlans4LogicalWindow` 和物理 PB 转换；比较类型、时区/时间类型、collation 和显式 RANGE 是兼容性重点。
- 若要达到 Go 的属性推导能力，应在明确任务中补齐 `PreparePossibleProperties`，并验证 cascades/memo 和传统优化器都能消费相同的 `PARTITION BY + ORDER BY` 顺序，不能只改变一个入口。
- 若增加窗口函数元数据，需同步 builder 中从 aggregation 描述符到 `WindowFuncDesc` 的映射、物理窗口构造、哈希/相等及序列化；性能上关注表达式重复克隆、列集合排序去重和大窗口函数列表的哈希成本。

建议同步扩展 `logical_window_test.rs` 作为本文件的聚焦测试，并按行为选择 `logical_relational_aster_unit_test.rs`、`logicalop_test/hash64_equals_test.rs`、`logicalop_test/logical_operator_test.rs` 或 `physical_window_test.rs`。仓库约定要求 Rust 测试保持在独立文件中。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter .../logical_window.rs` 确认目标已索引；`node --file ... --offset 1/521` 读取目标 598 行；`query LogicalWindow --kind struct --json`、`query replace_window_expr --json`、`query extractUsedCols --json` 核对 Rust/Go 符号。图命令对目标方法未给出 callers/callees，调用边改由源码搜索验证。
- 目标源码：`pkg/planner/core/operator/logicalop/logical_window.rs`，核对全部枚举、结构体、固有方法、两个内部辅助函数和 `LogicalPlan` impl。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml` 与 `lib.rs`；该目录没有 `doc.go`，因此不存在可额外读取的包级 Go 契约文件。
- 构建与优化调用：`pkg/planner/core/logical_plan_builder_runtime.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/operator/logicalop/logical_selection.rs`、`pkg/planner/cascades/memo/group_expr.rs`。
- 物理化证据：`pkg/planner/core/operator/physicalop/physical_window.rs`，尤其是 `contains_virtual_window_expression` 和 `ExhaustPhysicalPlans4LogicalWindow`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_window.go` 全部 627 行；Cargo 的 `[package.metadata.porting]` 也明确声明 Go package 为同一路径。
- 测试证据：`logical_window_test.rs`、`logical_relational_aster_unit_test.rs`、`logicalop_test/hash64_equals_test.rs`、`logicalop_test/logical_operator_test.rs`，并对照同目录两个 Go 测试文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核所有能力陈述均能回指上述符号或文件。

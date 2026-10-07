# `pkg/planner/core/operator/logicalop/logical_selection.rs`

## 文件定位

本文件实现逻辑计划中的选择算子 `LogicalSelection`，对应 SQL 的 `WHERE`、`HAVING` 以及优化规则临时物化的过滤节点。它位于 `astersql-planner-core-operator-logicalop` crate：[`lib.rs`](lib.rs) 以私有模块 `logical_selection` 装入实现，再用 `pub use logical_selection::*` 对外导出；[`Cargo.toml`](Cargo.toml) 的包名与 `[package.metadata.porting].go-package` 分别确认了 Rust crate 边界和 Go 对照目录。

在实际规划主链中，[`logical_plan_builder_runtime.rs`](../../logical_plan_builder_runtime.rs) 的 SELECT 构建流程遇到 `select.Where` 时，先用 `LogicalSelection::default().Init(...)` 建节点，再重写条件并挂接原计划子树。优化阶段通过 [`base_logical_plan.rs`](base_logical_plan.rs) 的 `PredicatePushDownPlan` 调用 `LogicalPlan::PredicatePushDownRoot`；物理化阶段由 [`physical_selection.rs`](../physicalop/physical_selection.rs) 的 `ExhaustPhysicalPlans4LogicalSelection` 读取本节点的 `Conditions` 并生成 `PhysicalSelection` 候选。因此本文件处在“AST 表达式已改写为 planner expression”之后、“物理计划与执行器”之前。

## 核心职责

- 保存 AND 连接的 CNF 谓词项：`LogicalSelection::Conditions: Vec<Expression>`；节点本身不改变输出列，只过滤孩子产生的行。
- 为计划显示和等价比较提供稳定表示：`ExplainInfo` 与 `HashCode` 都消除条件顺序带来的不稳定性。
- 实现选择节点特有的逻辑优化：谓词简化与下推、恒假子树转空 `LogicalTableDual`、无剩余条件时移除自身、条件引用列参与列裁剪，以及等值常量条件推导 `MaxOneRow`。
- 从条件推导优化元数据：相关列、统计基数、可能排序属性、函数依赖中的非空列/常量列/等价列。
- 识别 `Selection(Window(DataSource))` 上的 `row_number()` 上界，将分区 `LogicalTopN` 插入 Window 与 DataSource 之间；该改写受 TiFlash 路径、窗口帧和聚簇句柄前缀约束。
- 为生成列替换等规则提供递归表达式列替换，并把具有会话变量读写副作用的 `set_var`/`get_var` 条件隔离在不可下推集合中。

本文件不是执行器：它只改写和描述逻辑计划。行级过滤最终由物理选择算子及其下游执行路径完成。

## 主要符号

- `pub struct LogicalSelection`：由 `BaseLogicalPlan` 和 `Conditions` 组成。基类持有上下文、schema、输出名、孩子、统计和 FD 等通用状态；条件以 trait object 表达式保存。
- `Init(ctx, qb_offset)`：用 `plancodec::TypeSel` 初始化基类并记录查询块偏移，是构建合法 Selection 的标准入口。
- `ExplainInfo()`：在当前表达式求值上下文中字符串化条件，排序后以 `", "` 连接；无计划上下文时返回空串。
- `ReplaceExprColumns(replace)` / `replaceExpressionColumns(...)`：按列的 canonical hash 递归替换普通列和相关列；标量函数会克隆、递归替换参数并调用 `CleanHashCode`，防止复用旧哈希缓存。
- `HashCode()`：编码物理类型 ID、查询块偏移、条件数量和排序后的各条件哈希；条件顺序不同但集合相同的 Selection 得到相同结果。生成文件 [`hash64_equals_generated.rs`](hash64_equals_generated.rs) 的 `Hash64`/`Equals` 直接以此结果为依据。
- `PredicatePushDown(...)`：在保留当前节点对象的协议下下推条件并返回需要上抛的外层条件；必要时把孩子替换为空 `LogicalTableDual`。
- `PredicatePushDownRoot(...)`：适配 Rust 的树根替换协议，返回 `(未消费谓词, 可选替换节点)`；可用空 Dual 或唯一孩子整体替换当前 Selection。
- `PruneColumns(...)`：把父节点所需列与条件引用列合并后交给孩子裁剪。
- `BuildKeyInfo()`：先递归构建孩子键信息，再寻找 `列 = 常量/相关列` 条件，通过 `CheckMaxOneRowCond` 推导最多一行。
- `DeriveTopN()` / `windowIsTopN()` / `checkPartitionBy(...)`：识别并执行 Window 下方 TopN 派生。
- `PredicateSimplification()`、`PullUpConstantPredicates()`：分别简化本节点条件，及收集校对规则兼容的列—常量比较条件。
- `DeriveStats(...)`、`PreparePossibleProperties(...)`：按 `SelectionFactor` 缩放孩子统计、清空 `GroupNDVs`，并继承孩子可能排序属性和 TiFlash 标记。
- `ExtractCorrelatedCols()`、`ExtractFD()`：抽取外层引用；把条件产生的非空、常量和等价信息合并到孩子 FD 后投影到输出列集合。
- `splitSetGetVarFunc(...)` / `hasGetSetVarFunc(...)`：递归识别 `set_var`、`get_var` 标量函数，将有顺序/副作用语义的条件留在 Selection 层。
- `isConstTrue(...)`：只把非 NULL、非 deferred、非参数标记的非零整数或浮点常量视为可删除的恒真条件。
- `validCompareConstantPredicate(...)`：只接受 `GT/GE/LT/LE/EQ` 的二元列—常量比较，且要求常量类型与列的静态类型校对规则一致。
- `impl LogicalPlan for LogicalSelection`：把通用 trait 的类型擦除、基类访问、显示、哈希、谓词下推、列裁剪、键信息和统计钩子转发到上述实现。

## 执行流程

1. 构建器调用 `Init`，把已经重写的 WHERE/HAVING 条件放入 `Conditions`，复制孩子的 schema/输出名并挂接单个逻辑孩子。
2. 谓词下推入口先删除调用方传入的明确恒真数值常量，再对节点自有条件执行不带常量传播的谓词简化。
3. 若自有条件已恒假或为 NULL，`PredicatePushDown` 把当前孩子改成零行 `LogicalTableDual`；`PredicatePushDownRoot` 则返回 Dual 作为整节点替换。父谓词不能无条件丢弃：若空子树经过 Projection/Aggregation，`rewrite_predicates_for_empty_subtree` 先按对应算子的表达式映射/聚合边界拆分并递归重写。
4. 正常路径用 `splitSetGetVarFunc` 分离有会话变量副作用的条件，把可下推条件与父谓词合并后交给 `PredicatePushDownPlan`。孩子返回的残差再与副作用条件合并。
5. 只有不含相关列、且引用列全部属于 Selection schema 的残差可留在本节点；其余作为外层条件返回。保留条件再做一次允许常量传播的简化，恒假时转 Dual；没有本地条件且只有一个孩子时，根协议直接用该孩子替换 Selection。
6. 后续优化按需调用 `PruneColumns`、`BuildKeyInfo`、`ExtractFD`、`DeriveStats` 和属性准备，使条件引用不会被误裁剪，并把选择性和约束传递给代价与规则系统。
7. `DeriveTopN` 仅在 `windowIsTopN` 识别成功时改写：取下 Window 原孩子，按 Window 的 `OrderBy` 和 `PartitionBy` 新建 `LogicalTopN`，挂成 `Window -> TopN -> 原孩子`，Selection 仍位于 Window 上方。
8. 物理枚举读取 `Conditions` 和逻辑统计，生成物理 Selection；对于 MPP，还会在 [`physical_selection.rs`](../physicalop/physical_selection.rs) 中额外检查运行时标量子查询、TiFlash 可推性与虚拟列。

## 数据与状态

`Conditions` 是本文件唯一新增的业务字段。它使用 `Vec<Box<dyn expression::Expression>>`（经 crate 的 `Expression` 别名）拥有表达式树；条件的列表顺序不参与 `HashCode` 的语义，但下推时仍需隔离 `set_var`/`get_var`，因为这类表达式有可观察的会话副作用。

`BaseLogicalPlan` 保存单孩子逻辑树、`ContextRef`、query block offset、schema、输出名、统计、FD 与 `MaxOneRow` 等状态。若改写为 `LogicalTableDual`，代码显式复制 schema；根替换路径及首次恒假路径还复制输出名，以保持上层列绑定不变。`DeriveTopN` 暂时通过 `TakeChildren` 取得 Window 的孩子所有权；若孩子没有上下文，会把孩子放回 Window 后返回 `false`，避免破坏树。

计划哈希包含算子类型、查询块偏移、条件数量与带长度前缀的条件哈希。长度前缀避免不同条件分段产生拼接歧义；排序使 CNF 项交换顺序不影响等价判断。`ExtractFD` 对 Join 孩子优先使用 `FullSchema`，因为 USING 等合并列可能不在 Join 的普通输出 schema 中；其他情况使用本节点 schema，最后用输出列 ID 投影 FD。

统计状态由 `DeriveStats` 写回基类：缓存可用且 `reload == false` 时直接返回 `(已有统计, false)`；否则取孩子统计，按会话变量和 `cost::factors_thresholds::SelectionFactor` 缩放，清空已不再可靠的 `GroupNDVs`，返回 `(新统计, true)`。

## 依赖与调用关系

上游直接证据包括：

- [`logical_plan_builder_runtime.rs`](../../logical_plan_builder_runtime.rs) 在 SELECT 的 WHERE 构建阶段创建并初始化 `LogicalSelection`。
- [`base_logical_plan.rs`](base_logical_plan.rs) 的 `PredicatePushDownPlan` 通过 trait 调用 `PredicatePushDownRoot`，并把返回的替换节点写回树；`AttachSelectionToPlan` 也会为孩子未消费的残差谓词新建 Selection。
- [`optimizer_runtime.rs`](../../optimizer_runtime.rs) 的生成列表达式替换会直接遍历 `LogicalSelection::Conditions`；其他优化规则也通过向下转型识别该节点。

下游直接依赖包括：

- `expression`：表达式哈希、字符串化、列/相关列抽取、上界识别和类型/校对规则；`rule_util`：谓词简化与最多一行判断。
- 同 crate 的 `LogicalAggregation`、`LogicalProjection`、`LogicalWindow`、`LogicalTopN`、`LogicalTableDual`、`LogicalJoin` 和 `DataSource`：分别参与空子树谓词重写、TopN 派生、空结果替换和 FD/schema 特判。
- `fd`、`intset`、`planner_util`：函数依赖集合、列 ID 集合、非空/常量/等价约束抽取；`cost`、`property`：统计选择因子与物理属性。
- `kv`：检查 DataSource 的候选路径是否包含 TiFlash；`parser_ast`：比较函数名、会话变量函数名和等值函数名；`plancodec`：Selection 类型标识及哈希编码。
- [`physical_selection.rs`](../physicalop/physical_selection.rs) 的 `ExhaustPhysicalPlans4LogicalSelection` 是明确的逻辑到物理边界，读取上下文、条件和统计生成候选。

RustCodeGraph 的文件节点显示本文件被 planner 测试、cascades 实现、基类和其他直接消费者引用；精确 `query` 同时找到了 Rust 与 Go 的 `LogicalSelection` 结构及 Rust/Go 两个 `splitSetGetVarFunc`。部分 impl 方法未被索引器建成独立 method 节点，因此相关边以文件节点、trait 入口和上述真实源码调用点交叉核验，而未把缺失的静态边推断为“无人调用”。

## 错误处理与边界

返回 `Result` 的路径主要传播孩子的谓词下推、列裁剪和统计推导错误。需要创建替换节点而 `SCtx()` 缺失时，`PredicatePushDown`/`PredicatePushDownRoot` 返回 `PlannerError("Selection has no plan context")`；相比 panic，这让不完整测试计划或错误接线可被显式诊断。

无计划上下文时，显示返回空串、谓词简化跳过、统计退化为未缩放的孩子统计、FD 只继承孩子集合；TopN 派生失败且恢复被取出的孩子。无孩子时，下推条件留在本节点或作为根残差处理，列裁剪不递归，统计使用默认值，可能属性返回默认值。生产构建约定仍是一个孩子，防御分支不代表多孩子/无孩子 Selection 是正常形态。

TopN 派生必须同时满足：恰好一个条件；`FindUpperBound` 找到正上界；过滤列恰好是唯一 Window 结果列；Window 的孩子是 `DataSource`；任何候选访问路径都不是 TiFlash；只有一个 `row_number`；窗口帧为空或严格为 `ROWS BETWEEN CURRENT ROW AND CURRENT ROW`；`PARTITION BY` 为空，或逐列匹配数据源句柄列前缀。任一条件失败都保持原树。

恒真识别有意保守：NULL、参数、deferred expression、字符串等都不会被删除。常量谓词上拉也要求比较运算符、二元形态、列/常量角色和校对规则均匹配。表达式列替换只递归处理普通列、相关列和 `ScalarFunction`；未知表达式实现原样保留。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件句柄、网络连接或事务生命周期。所有优化方法都在调用方持有的可变逻辑计划树上同步执行。

资源管理依赖 Rust 所有权：条件和孩子由 `Vec`/`Box` 拥有；`std::mem::take` 在简化或拆分前搬走条件，避免同时借用和重复克隆；`TakeChildren` 转移子树所有权，成功后重新挂接，失败路径明确恢复。表达式替换克隆被修改的标量函数和目标列，未命中的表达式保持原所有权。`ContextRef` 在需要给新节点共享计划上下文时执行 `cloned()`；本文件不改变上下文内部的并发策略。

安全扩展时最重要的生命周期不变量是：任何从节点取出的孩子都必须在所有非成功分支重新挂回，任何替换节点都必须保留原输出 schema（以及调用链需要的输出名），任何会缓存哈希的表达式变更后都必须清理缓存。

## 与 Go 版本的对应关系

直接对照文件是 [`logical_selection.go`](logical_selection.go)。结构字段、`Init`、稳定 Explain/Hash、列裁剪、MaxOneRow、统计缩放、属性继承、相关列/FD 提取、会话变量条件隔离，以及 `row_number` TopN 识别条件均来自 Go 实现并保留相同意图。

需要注意的 Rust 适配与当前差异：

- Go `PredicatePushDown` 同时返回残差、替换计划和错误；Rust 的 trait 为适应 `Box<dyn LogicalPlan>` 所有权，拆成 `PredicatePushDown` 与可返回 `Option<LogicalPlanRef>` 的 `PredicatePushDownRoot`。Rust 还通过 `rewrite_predicates_for_empty_subtree` 保留空子树上方 Projection/Aggregation 的谓词映射语义。
- Go 默认直接索引第一个孩子和上下文；Rust 多处使用 `first()`、`Option` 和显式 `PlannerError`，为不完整树提供防御行为，但正常计划仍要求单孩子。
- Go `DeriveTopN` 返回逻辑计划指针；Rust 在原树内改写并返回 `bool`。Rust 会显式给派生 TopN 设置原孩子 schema，且在上下文缺失时恢复孩子。
- Go 的 `windowIsTopN` 要求显式当前行窗口帧；Rust 将 `Frame == None` 也视为默认的 current-row frame，然后对显式 frame 执行同等检查。该差异应与 Window 构建阶段的默认帧表达方式一起维护。
- Go `PredicateSimplification` 当前主要在 `intest` 中断言简化前后等价，再递归基类；Rust 实现会实际用 `ApplyPredicateSimplification` 替换本节点条件后再递归。修改此处时要用真实优化计划测试确认两端可观察结果仍一致。
- Go 的 `ResolveExprAndReplace`/`HasGetSetVarFunc` 是公共表达式工具；Rust 在本文件内实现对应的递归替换和检测逻辑。新增表达式种类时，必须同步检查 Rust 递归覆盖范围。

这些差异是移植形态或当前实现事实，不能据此假定 Rust 已覆盖 Go 文件之外的所有 planner 行为。

## 扩展指南

新增或修改 Selection 行为时，优先按职责选择接入点：

- 新的条件下推边界或副作用函数：修改 `splitSetGetVarFunc`/`hasGetSetVarFunc` 及 `PredicatePushDownRoot`，并验证条件不会丢失、重复或越过语义屏障。
- 新的恒真/恒假类型：修改 `isConstTrue` 或共享的 `Conds2TableDual`，覆盖 NULL、参数、deferred、类型转换和错误传播；不要把“能在某上下文求值”误当成全局常量。
- 新的表达式列替换能力：扩展 `replaceExpressionColumns`，保留相关列附加状态，并在任何可缓存节点变化后清理哈希。
- 新的 Window TopN 形态：修改 `windowIsTopN`/`checkPartitionBy`/`DeriveTopN`，同时评估 TiFlash、句柄列顺序、窗口帧、多个窗口函数、上界溢出和 schema/输出名保持。
- 新的统计或 FD 规则：修改 `DeriveStats`/`ExtractFD`，确认 Join `FullSchema` 特判、输出投影及 `GroupNDVs` 是否仍正确。
- trait 可见的新行为：除固有方法外，还要检查 `impl LogicalPlan` 是否需要新增转发，以及物理枚举、生成的 Hash64/Equals 和计划克隆逻辑是否同步。

测试必须放在独立测试文件，不要嵌入本生产文件。最近的精确单元证据是 [`logical_d_aster_unit_test.rs`](logical_d_aster_unit_test.rs)（条件顺序无关哈希、普通常量可下推）与 [`logicalop_test/hash64_equals_test.rs`](logicalop_test/hash64_equals_test.rs)（Selection 的 Hash64/Equals）。真实优化树回归位于 [`logical_plans_test.rs`](../../logical_plans_test.rs) 和 [`optimizer_logical_entry_aster_unit_test.rs`](../../optimizer_logical_entry_aster_unit_test.rs)，后者验证启用谓词下推后 Selection 被移除且条件进入 DataSource，并覆盖空结果 Dual。Window/TopN 的现有 casetest 在 [`casetest/windows/window_push_down_test.rs`](../../casetest/windows/window_push_down_test.rs) 与 [`casetest/rule/rule_derive_topn_from_window_test.rs`](../../casetest/rule/rule_derive_topn_from_window_test.rs)；其中使用简化 `PlanNode` 规则模型，扩展真实 `LogicalSelection::DeriveTopN` 时还应补同层真实逻辑树测试，而不能只依赖该模型测试。

兼容性风险集中在谓词副作用/相关列作用域、输出 schema 和名字、NULL 三值逻辑、哈希稳定性与 Go 计划形状；性能风险集中在重复表达式克隆、条件简化次数、FD 抽取遍历和错误派生 TopN 导致的代价变化。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust/Go 文件均在索引内。
- RustCodeGraph `explore "pkg/planner/core/operator/logicalop/logical_selection.rs LogicalSelection"`：得到目标源码上下文、测试中的 `find_selection` 引用、`base_logical_plan` 规则钩子与物理 Selection 关联候选。
- RustCodeGraph `node --file ... --offset ... --limit ...`：分段读取了目标文件全部 707 行，并核验了 `lib.rs`、`base_logical_plan.rs`、`logical_plan_builder_runtime.rs`、`optimizer_runtime.rs`、`physical_selection.rs`、`hash64_equals_generated.rs` 及上述 Rust 测试的直接相关片段。
- RustCodeGraph `query LogicalSelection --kind struct --json`：同时定位 Rust `logical_selection.rs::LogicalSelection` 和 Go `logical_selection.go::LogicalSelection`；`query splitSetGetVarFunc --kind function --json` 同时定位两端实现。`callers/callees` 对 impl/trait 方法未返回可用独立边，故调用结论以文件节点的 `used by`、trait 入口和源码调用点交叉验证。
- Cargo/模块证据：[`Cargo.toml`](Cargo.toml) 的 crate 名称、依赖和 Go 包元数据；[`lib.rs`](lib.rs) 的模块声明与公开再导出。
- Go 语义证据：完整读取 [`logical_selection.go`](logical_selection.go)，逐项比对结构、优化流程、TopN 约束、统计与 FD 行为。
- 测试证据：[`logical_d_aster_unit_test.rs`](logical_d_aster_unit_test.rs)、[`logicalop_test/hash64_equals_test.rs`](logicalop_test/hash64_equals_test.rs)、[`logical_plans_test.rs`](../../logical_plans_test.rs)、[`optimizer_logical_entry_aster_unit_test.rs`](../../optimizer_logical_entry_aster_unit_test.rs)、[`casetest/windows/window_push_down_test.rs`](../../casetest/windows/window_push_down_test.rs) 和 [`casetest/rule/rule_derive_topn_from_window_test.rs`](../../casetest/rule/rule_derive_topn_from_window_test.rs)。
- 本任务是纯文档分析，按总计划未运行 Cargo。交付前使用任务规定的命令校验文档存在且恰有十一个固定二级标题，并人工复核唯一新增生产物、链接路径、无整段源码复制和无未经证实的支持声明。

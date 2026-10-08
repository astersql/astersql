# `pkg/planner/core/operator/logicalop/logical_union_all.rs`

## 文件定位

本文件实现逻辑计划节点 `LogicalUnionAll`，表示 SQL `UNION ALL` 的多输入纵向拼接：各输入按列位置映射到同一输出 schema，所有行均被保留，不在这个节点做去重。它属于 Cargo 包 `astersql-planner-core-operator-logicalop`；包入口 `pkg/planner/core/operator/logicalop/lib.rs` 以 `mod logical_union_all` 纳入实现，并用 `pub use logical_union_all::*` 对规划器其他模块公开类型。

节点通常由 `pkg/planner/core/logical_plan_builder_runtime.rs` 的集合操作构建路径创建。构建器先检查输入非空且列数一致，计算每一列的联合类型，为输出列分配新的 `UniqueID`，再在每个分支上插入 `LogicalProjection`，完成类型转换并把所有分支的 schema 统一为 union schema，最后初始化 `LogicalUnionAll`、设置输出名和子节点。逻辑优化完成后，`pkg/planner/core/operator/physicalop/physical_union_all.rs::ExhaustPhysicalPlans4LogicalUnionAll` 根据所需物理属性枚举 Root/MPP `PhysicalUnionAll` 候选。

## 核心职责

- `LogicalUnionAll::Init` 建立类型名为 `"Union"` 的 `BaseLogicalPlan`，保存规划上下文和 query block offset。
- `PredicatePushDown` 把同一组谓词的独立克隆送入每个分支；分支未消化的谓词由 `AddSelection` 物化在对应分支之上，union 自身不保留残余谓词。
- `PruneColumns` 以 union 输出 schema 的 `UniqueID` 判断父层使用列，并对所有分支保持相同的列集合与顺序；必要时增加投影，屏蔽子算子裁剪后额外保留的列。
- `PushDownTopN` 为每个分支克隆一个局部 TopN，将局部 `Count` 设为外层 `Offset + Count`，同时保留原 TopN 在 union 上方以执行全局排序、偏移和截断。
- `DeriveStats` 汇总分支行数和逐输出列 NDV，维护节点统计缓存。
- `PreparePossibleProperties` 汇总 TiFlash 可用性；`ExtractFD` 只保留所有分支共同保证的 NOT NULL 列和等价类。

该文件不负责集合操作的类型合并、分支投影创建、`UNION DISTINCT` 去重、物理执行或具体数据传输；这些职责分别位于构建器、聚合/去重规则、物理计划与执行器层。

## 主要符号

- `pub struct LogicalUnionAll { pub LogicalSchemaProducer: LogicalSchemaProducer }`：唯一的生产类型。嵌入的 schema producer 继续持有 `BaseLogicalPlan`，因此 schema、子节点、统计、FD、上下文和计划标识都由公共基座管理。
- `Init(self, ctx, offset) -> Self`：用 `NewBaseLogicalPlan(ctx, "Union", offset)` 初始化节点。返回值仍是具体类型，调用方随后设置 schema、输出名和 children。
- `PredicatePushDown(&mut self, Vec<Expression>) -> Result<Vec<Expression>>`：逐分支调用 `PredicatePushDownPlan`，再调用 `AddSelection`；成功时总是返回空向量。
- `PruneColumns(&mut self, &[Column]) -> Result<()>`：计算输出列使用位图、递归裁剪分支、收缩自身 schema，并为仍输出额外列的分支包一层 `LogicalProjection`。
- `PushDownTopN(&mut self, Option<LogicalPlanRef>) -> Option<LogicalPlanRef>`：这是具体类型上的方法，而非本文件 `LogicalPlan` impl 显式覆写的方法。它要求传入计划可向下转型为 `LogicalTopN`，为各分支创建局部 TopN，最后把已取出的 union 挂回外层 TopN；没有外层 TopN 时返回 union 自身。
- `DeriveStats(&mut self, reload) -> Result<(StatsInfo, bool)>`：在可复用缓存时返回 `(cached, false)`，否则递归取得子统计、求和、缓存并返回 `(result, true)`。
- `PreparePossibleProperties(&mut self, &[bool]) -> bool`：直接委托 `BaseLogicalPlan::PreparePossibleProperties`；非空且所有子节点都为 `true` 时，节点才标记为可用 TiFlash。
- `ExtractFD(&mut self) -> fd::FDSet`：取得每个子节点的 FD 集，求 NOT NULL 交集和公共等价类，写入基座缓存并按值返回结果。
- `impl LogicalPlan for LogicalUnionAll`：提供 `Any` 向下转型和基座访问；只显式转发谓词下推、列裁剪、统计推导，其他 trait 默认行为来自 `BaseLogicalPlan`。

## 执行流程

构建阶段的主流程如下：

1. `logical_plan_builder_runtime.rs` 验证至少有一个分支，且所有分支列数相同。
2. 构建器按列位置计算公共字段类型，为 union 输出创建新的列 ID。
3. 每个输入分支被一个 `LogicalProjection` 包装；投影负责必要的 cast，并使用同一份 union schema 克隆。这一接线是不同行为依赖同一输出 `UniqueID` 的基础。
4. 构建器调用 `LogicalUnionAll::Init`，设置 schema、输出名和 children，并开启列裁剪、再次列裁剪、键构建、投影消除和空分支消除等优化标志。
5. 逻辑优化遍历通过 `LogicalPlan` trait 触发谓词下推、列裁剪和统计推导；TopN 专项优化也会识别 union 并把局部 TopN 放进每个分支。
6. 物理化阶段调用 `ExhaustPhysicalPlans4LogicalUnionAll`。有序属性、非 MPP 的 Flash 属性或具体 MPP 分区要求无法由普通 union 直接满足时不产出候选；其余情况生成 Root 或 MPP 物理节点。

几个方法的关键分支是：

- 谓词下推会为每个分支重新收集一份表达式克隆，避免将可变表达式对象在分支间共享；每次先用临时 `LogicalTableDual` 取走旧 child，再把 child 与残余条件交给 `AddSelection`。
- 列裁剪若发现父层没有引用任何 union 输出列，会把所有输出列都标为使用并传给 children，从而保留 union 的行形状/计数语义。若父层确实引用了部分列，则删除自身未用列；若某 child 仍比新输出 schema 更宽，就用等宽投影隐藏额外列。
- TopN 下推把每个局部 TopN 的 offset 留为默认值 0，count 使用 `wrapping_add(Count, Offset)` 的结果，并复制 `PreferLimitToCop`、排序项、上下文和 query block offset。分支只负责产生足够多的候选行，原 TopN 仍执行全局语义。
- 统计推导先检查 `reload` 与已有缓存；重算时逐 child 调用 `DeriveStats(reload)`，行数直接相加，输出列 NDV 也按相同 `UniqueID` 相加，缺失键按 0 处理，不把 NDV 截断到总行数。
- FD 提取先把结果 NOT NULL 候选初始化为全部输出列，然后依次与每个 child 的 `NotNullCols` 求交；等价类通过 `fd::FindCommonEquivClasses` 只保留所有 child 的公共项。

## 数据与状态

`LogicalUnionAll` 自身没有额外业务字段；全部持久状态都在 `LogicalSchemaProducer.BaseLogicalPlan` 中，包括 planner context、节点类型和 ID、query block offset、children、schema、统计缓存、FD 缓存以及 TiFlash 可用性。节点的核心不变量是：所有 children 的可见输出列必须与 union schema 按位置、类型和列 ID 对齐。构建器插入的分支投影建立此不变量，`PruneColumns` 在优化过程中继续同步维护它。

`PruneColumns` 的 `used: Vec<bool>` 与进入方法时的 schema 一一对应；`retain` 中用递增下标消费位图，因此 schema 与位图长度必须保持一致。`DeriveStats` 的 `StatsInfo` 以 `RowCount` 和 `ColNDVs<UniqueID, f64>` 表示估计；算法假定分支投影已经把同一位置的列改成 union 输出 ID。`ExtractFD` 产生的新 `FDSet` 同时返回给调用方并复制进基座；后续通用逻辑可通过 `base().FDs()` 读取缓存。

空 children 是可表达的内部状态，但通常不会由集合操作构建器产生，因为构建器会返回 `"set operation has no input"`。若直接构造空 union，`DeriveStats` 得到全零统计；`ExtractFD` 因没有交集约束步骤，会把 schema 的所有列保留为 NOT NULL。独立 Rust 测试明确固定了后一行为。

## 依赖与调用关系

上游与接线关系：

- `pkg/planner/core/logical_plan_builder_runtime.rs` 创建和初始化节点，并为每个分支建立共享 schema 的强制类型转换投影。
- `pkg/planner/core/operator/logicalop/lib.rs` 注册模块和公开再导出，其他 planner crate 因此可使用 `logicalop::LogicalUnionAll`。
- `pkg/planner/core/operator/logicalop/base_logical_plan.rs` 的 `LogicalPlan` trait 为优化器提供统一动态分派入口；`PredicatePushDownPlan`、递归裁剪和递归统计通过该接口进入本文件覆写。
- `pkg/planner/core/optimizer_runtime.rs` 中的 TopN/Limit、聚合和 union 消除等规则会识别或修改 `LogicalUnionAll`；可能属性遍历在子节点处理完后，把 children 的 TiFlash 标记交给基座汇总。

下游依赖：

- `PredicatePushDownPlan` 递归优化 child；`AddSelection` 将 child 未消化的谓词变成显式选择节点。
- `LogicalProjection` 和 `LogicalTableDual` 用于安全替换 trait object 形式的 child；后者只是 `mem::replace` 时的临时占位，不会作为成功路径的最终新语义分支。
- `LogicalTopN`、`ByItems` 和 `LogicalPlanRef` 构成 TopN 下推产生的计划节点。
- `property::StatsInfo`（由 crate 根再导出为 `StatsInfo`）承载基数和 NDV；`fd::FDSet` 与 `intset::FastIntSet` 承载函数依赖。
- `pkg/planner/core/operator/physicalop/physical_union_all.rs` 消费逻辑节点的 schema、统计、上下文和 children 数量，生成 `PhysicalUnionAll` 及对应 child properties。

`pkg/planner/core/operator/logicalop/Cargo.toml` 证明本文件位于独立 logicalop crate，直接依赖 `base`、`expression`、`fd`、`intset`、`property`、`planner_util`、`planctx`、`plancodec` 等本地 workspace 包；`[package.metadata.porting]` 将其 Go 对照包声明为 `pkg/planner/core/operator/logicalop`。

## 错误处理与边界

`PredicatePushDown`、`PruneColumns` 和 `DeriveStats` 用 crate 的 `Result<T>` 返回 `PlannerError`，并以 `?` 原样传播 child 或辅助函数失败。列裁剪在需要创建补偿投影却发现 union 没有 planner context 时，返回 `PlannerError("initialized Union must retain planner context")`；因此任何进入完整优化管线的 union 都应先调用 `Init`。

`PushDownTopN` 对传入类型使用 `downcast_ref::<LogicalTopN>().expect(...)`，并要求 TopN 有 context；违反调用契约会 panic，而不是返回可恢复错误。它用 `wrapping_add` 计算局部 count，和本文件正常小值测试一致，但极端 `u64` 溢出会回绕；另一条 `optimizer_runtime.rs` 专项下推路径使用 `saturating_add`，扩展或统一这两条路径时必须关注语义差异。

列裁剪依据 `UniqueID` 而非对象地址或列位置匹配父引用。它不在本文件重新验证 child 列数和 schema 对齐，这些约束由构建器建立；绕过构建器手工创建节点的调用方必须自行保持不变量。统计推导对 child 中缺少的输出 ID 记作 NDV 0，并简单相加，不修正不同分支值域重叠，也不限制 NDV 小于等于行数，因此结果是保守的加和模型而非精确集合估计。

FD 只保留跨所有分支都成立的 NOT NULL 和等价关系；任何单分支独有约束都不能提升为 union 输出约束。空 union 的 NOT NULL 结果属于当前实现及测试固定的边界，不应凭直觉改成空集合。

## 并发与资源生命周期

本节点在逻辑优化阶段以 `&mut self` 就地改写计划树，不启动线程、异步任务或通道，也不拥有文件、网络、事务或存储资源。`LogicalPlanRef` 是装箱的 trait object；改写 child 时用 `std::mem::replace`/`std::mem::take` 转移所有权，保证旧节点只被移动一次，临时 `LogicalTableDual` 随替换表达式结束而释放。

planner context 使用可克隆的 `ContextRef`，分支 TopN 和补偿投影各持有 context 的共享引用；实际生命周期由引用计数上下文决定。表达式和排序项为每个分支克隆，使各分支的后续可变优化互不共享同一个容器。文件没有内部锁；并发安全取决于上层是否把同一棵可变计划树限制在单一优化流程中。`StatsInfo`、FD 和 TiFlash 标记都是节点内缓存，修改 children 或 schema 后若不重新推导/失效缓存，可能留下陈旧状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_union_all.go`。Rust 基本逐方法保留 Go 行为：初始化为 Union；每个分支获得独立谓词切片/表达式集合；无父列引用时保留全部列；child 比 union schema 更宽时补投影；TopN 分支 count 为 offset 加 count 且保留外层 TopN；统计对行数和列 NDV求和；TiFlash 要求所有分支可用；FD 取 NOT NULL 与等价类公共部分。

接口形态存在以下移植差异：

- Go 的谓词下推和列裁剪同时返回可能替换后的计划，Rust 在 `&mut self` 上直接改写；Rust 谓词方法只返回残余条件，成功时为空。
- Go `DeriveStats` 接受预先计算的 `childStats`、`selfSchema` 和每个 child 的 `reloads`，先把 reloads 做 OR；Rust 接受单一 `reload`，自行递归取得 child 统计。
- Go 的 `PreparePossibleProperties` 接受 `PossiblePropertiesInfo`，Rust 这里只传递 children 的 `bool`，并委托基座缓存 `has_ti_flash`。
- Go TopN 排序项逐个新建 `util.ByItems`；Rust 克隆 `Vec<ByItems>`。两者都保留表达式和降序标记，但 Rust 的表达式克隆语义由表达式 trait object 实现决定。
- Go 使用普通无符号加法表达 `Count + Offset`；Rust 本方法明确使用 `wrapping_add`。同时仓库 Rust 专项优化路径存在 `saturating_add`，这是当前实现中需谨慎维护的差异。
- Go `ExtractFD` 把结果写入 `p.fdSet` 并返回指针；Rust 克隆写入基座后按值返回。

Go 的 `logicalop_test/hash64_equals_test.go::TestLogicalUnionAllHash64Equals` 还验证 schema 参与生成的 Hash/Equals；对应 Rust 测试位于 `logicalop_test/hash64_equals_test.rs`。Hash/Equals 实现在生成文件而非本文件中，但它说明 schema 是节点身份的一部分。

## 扩展指南

新增或修改 union 行为时，优先判断职责归属：输入列数检查、字段类型合并和 cast 应修改 `logical_plan_builder_runtime.rs`；逻辑重写应修改本文件或 `optimizer_runtime.rs` 的相应规则；Root/MPP 属性支持应修改 `physical_union_all.rs`；不要把执行器行为塞入逻辑节点。

修改本文件时应保持以下不变量：每个分支列位置与 union schema 对齐；下推谓词和排序项在分支间独立；局部 TopN 只做候选缩减且外层 TopN 保留；列裁剪后各分支可见宽度不超过 union 输出；统计与 FD 只声明所有分支都能安全支持的性质。若改变 children 或 schema，还要考虑 stats、FD 和 possible-properties 缓存是否需要失效或重算。

测试必须放在独立文件 `pkg/planner/core/operator/logicalop/logical_union_all_test.rs`，不要内嵌到生产源文件。现有用例分别覆盖：零引用列保留全部 schema、残余谓词附加到每个分支、额外 child 列由投影隐藏、TopN 克隆并保留外层节点、NDV 使用输出 ID 且不截断、空 union 的 NOT NULL/FD 缓存。新增错误分支可补充“未初始化 context 且需要补投影”；修改 TopN 计数时应增加接近整数上限的用例，并同步核对 `optimizer_runtime.rs` 的专项路径。若调整 Hash/Equals 或 schema 身份，需同步 `logicalop_test/hash64_equals_test.rs` 及 Go 对照测试意图。

兼容性风险主要是计划形状、列 ID/输出名稳定性和 Go 语义偏移；性能风险主要是每分支表达式克隆、额外 Projection，以及对大量分支逐一推导统计/FD。任何优化都不应以共享可变表达式、删除全局 TopN 或丢弃补偿投影为代价。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含目标仓库的 Rust/Go 文件；`node --file pkg/planner/core/operator/logicalop/logical_union_all.rs --offset 1 --limit 500` 读取了完整 232 行并确认 `LogicalUnionAll` 结构、七个固有方法和 `LogicalPlan` impl；`query LogicalUnionAll --limit 20` 定位到 Rust/Go 类型、物理枚举函数、构建器和测试符号。对方法名执行的 callers/callees 查询因 Go/Rust 同名方法较多而产生歧义，因此调用关系又以模块入口、构建器、优化器和物理枚举源码交叉核对，没有把歧义边当作唯一证据。
- 生产源码：`pkg/planner/core/operator/logicalop/logical_union_all.rs`；公共基座与 trait：`base_logical_plan.rs`；模块注册：`lib.rs`；构建接线：`pkg/planner/core/logical_plan_builder_runtime.rs`；优化器专项路径和 TiFlash 属性遍历：`pkg/planner/core/optimizer_runtime.rs`；物理候选：`pkg/planner/core/operator/physicalop/physical_union_all.rs`。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml`，包括包名、`lib.rs` 入口、本地依赖以及 Go 包映射。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_union_all.go`，逐项核对 Init、谓词、裁剪、TopN、统计、可能属性与 FD；Hash/Equals 的 Go 测试为 `pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.go`。
- Rust 独立测试：`pkg/planner/core/operator/logicalop/logical_union_all_test.rs`；生成 Hash/Equals 对应测试：`pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.rs`。本任务是纯文档分析，按任务约束未运行 Cargo，测试文件仅作为现有行为证据读取。
- 交付结构以任务指定命令检查：文档存在，且固定的十一个二级标题各出现一次；同时人工复核文档覆盖“为何存在、如何运行、如何安全扩展”，未把未验证设计写成当前事实。

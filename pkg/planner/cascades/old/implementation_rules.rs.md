# `pkg/planner/cascades/old/implementation_rules.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-old` crate，是旧版 Cascades 优化器从 memo 中的逻辑 `GroupExpr` 生成物理 `Implementation` 候选的规则层。crate 入口 `pkg/planner/cascades/old/lib.rs` 将本模块声明为 `implementation_rules` 并公开再导出；`pkg/planner/cascades/old/optimize.rs` 的 `Optimizer` 持有本文件构造的规则表。

源码入口：[implementation_rules.rs](./implementation_rules.rs)。

它处在“逻辑等价式探索”之后、“递归实现孩子并比较代价”之前。直接调用链是：`Optimizer::NewOptimizer` 调用 `defaultImplementationMap`；`Optimizer::GetImplementationRules` 按逻辑算子的 `Operand` 取规则；`Optimizer::implGroupExpr` 依次调用 `ImplementationRule::Match` 和 `ImplementationRule::OnImplement`；`Optimizer::implGroup` 再依据候选声明的孩子物理属性递归找实现、调用代价接口、附加孩子并缓存最优实现。因而本文件负责候选生成和物理属性传播，不负责 memo 探索、全局最优搜索或最终索引解析。

`pkg/planner/cascades/old/Cargo.toml` 的 `[package.metadata.porting]` 把 crate 对应到 Go 包 `pkg/planner/cascades/old`；直接语义对照文件是同目录 `implementation_rules.go`。

## 核心职责

1. 用 `ImplementationRule` 统一“属性是否匹配”和“如何生成候选”两个阶段。`Match` 是廉价可行性过滤；`OnImplement` 必须建立物理算子、输出统计和 schema，并正确写入每个孩子的 `PhysicalProperty`。
2. `defaultImplementationMap` 为 17 类 `Operand` 注册规则并保持候选顺序。大多数逻辑算子只有一个实现；`TopN` 同时尝试真实 `PhysicalTopN` 和“孩子提供顺序时退化成 `PhysicalLimit`”；`Join` 依次尝试左建表 HashJoin、右建表 HashJoin、MergeJoin。
3. 将 memo group 的统计、schema、执行引擎和逻辑算子上下文搬运到物理计划。`group_properties`、`child_properties`、`context` 将缺失状态转换成可传播的 `PlannerError`，避免 Rust 端直接解引用空状态。
4. 生成并下推物理属性。排序要求决定扫描方向、NominalSort、MergeJoin、Apply、Window 等候选是否可用；`ExpectedCnt` 用于缩放输出统计或限制孩子期望行数。
5. 通过 `PlanAdapter<T>` 把具体物理算子接到 `astersql-planner-implementation` 所需的计划访问与代价 trait 上，并读取会话变量、直方图和表类型计算 CPU、网络、扫描、seek、并发等成本参数。

## 主要符号

- `type RuleResult = logicalop::Result<Vec<ImplementationRef>>`：规则统一返回零个或多个候选；空向量表示“当前分支没有合法候选”，`Err` 表示状态损坏或不支持的执行引擎等真正错误。
- `pub trait ImplementationRule`：公开规则协议。`Match(&GroupExpr, &PhysicalProperty) -> bool` 判断所需属性；`OnImplement(...) -> RuleResult` 生成候选并声明孩子要求。
- `pub fn defaultImplementationMap()`：返回 `HashMap<Operand, Vec<Box<dyn ImplementationRule>>>`。覆盖 `TableDual`、`MemTableScan`、`Projection`、`TableScan`、`IndexScan`、`TiKVSingleGather`、`Show`、`Selection`、`Sort`、`Aggregation`、`Limit`、`TopN`、`Join`、`UnionAll`、`Apply`、`MaxOneRow`、`Window`。
- `implementation_ref`：将具体 `memo::Implementation` 包装为共享、内部可变的 `Rc<RefCell<_>>`，供优化器稍后设置成本和附加孩子。
- `group_ref`、`logical<T>`、`context`、`group_properties`、`child_properties`：集中完成弱引用升级、逻辑节点向下转型以及上下文/统计/schema 读取，是规则的前置不变量检查点。
- `max_count_property`、`scaled_stats`：构造无排序且 `ExpectedCnt = f64::MAX` 的孩子属性，并按父期望行数缩放统计。
- `histograms`：从类型擦除的 `StatsInfo::HistColl` 中恢复 `statistics::HistColl`；扫描和 reader 成本依赖它。
- `sys_f64`、`sys_i64`、`executor_concurrency`、`hash_join_concurrency`、`table_factor`：读取会话系统变量并提供默认值；并发度最少为 1；临时表的网络/扫描因子为 0。
- `CardinalityAdapter`：把 `base::PlanContext` 暴露为基数估计所需的 `CardinalityContext`。
- `PlanAdapter<T>`：实现通用 `PlanAccess`，并针对投影、选择、聚合、TopN、UnionAll、Apply、HashJoin、MergeJoin、Sort、reader、TableScan、IndexScan 实现代价接口。它把物理算子原有 `GetCost`、会话因子及平均行宽估计接入统一 Implementation 成本模型。
- 规则类型：`ImplTableDual`、`ImplMemTableScan`、`ImplProjection`、`ImplTiKVSingleReadGather`、`ImplTableScan`、`ImplIndexScan`、`ImplShow`、`ImplSelection`、`ImplSort`、`ImplHashAgg`、`ImplLimit`、`ImplTopN`、`ImplTopNAsLimit`、`ImplHashJoinBuildLeft`、`ImplHashJoinBuildRight`、`ImplMergeJoin`、`ImplUnionAll`、`ImplApply`、`ImplMaxOneRow`、`ImplWindow`。
- `getImplForHashJoin`：HashJoin 的共同构造函数，设置 inner child、build side、连接条件、并发度、schema 和孩子期望行数。
- `GetHashJoin`：为 `ImplApply` 构造其内嵌的 `PhysicalHashJoin` 基座；名称和职责对应 Go 侧调用的 planner-core helper，但 Rust 在本文件中做了适配。

## 执行流程

1. `Optimizer::NewOptimizer` 构造时调用 `defaultImplementationMap`。优化器完成 memo 探索和 `fillGroupStats` 后，从根 group 以“无排序、无限期望行数”的属性进入 `implGroup`。
2. `implGroupExpr` 用表达式的 `Operand` 找到规则序列，先执行 `Match`。不匹配的规则不构造物理节点；匹配规则执行 `OnImplement`，其返回值可包含一个、多个或零个候选。
3. 每个 `OnImplement` 先由 `logical<T>` 验证逻辑节点类型，再读取 group 状态和 `PlanContext`，构造对应物理算子，复制逻辑字段，初始化统计、query block offset、schema 和孩子要求，最后以 `New*Impl(PlanAdapter { plan })` 包装。
4. 叶子与一元规则的主要分支如下：
   - `TableDual`、`MemTableScan`、`Show` 只接受无排序要求；扫描规则还检查 handle/index 能否提供顺序，并相应设置 `KeepOrder`、`Desc`。
   - `Projection` 调用 `TryToGetChildProp` 改写父属性；属性无法穿过表达式时返回空候选。
   - `TiKVSingleGather` 按 `IsIndexGather` 生成 `PhysicalIndexReader` 或 `PhysicalTableReader`，孩子继承父属性的 essential fields。
   - `Selection` 将父 essential property 下推，并按 group engine 包装为 TiDB 或 TiKV 实现。
   - `Sort` 若排序项全可转成列属性，就生成只声明孩子排序要求的 `NominalSort`；否则生成真实 `PhysicalSort`，孩子不承担排序但按无限行数提供输入。
   - `HashAgg` 仅接受空排序，当前没有 StreamAgg 候选；孩子属性为无限期望行数。
   - `Limit`、`TopNAsLimit` 把孩子期望行数设为 `Count + Offset`；后者还把列排序项下推。`TopN` 自己排序，孩子无需提供顺序。
   - `MaxOneRow` 要求孩子最多给出 2 行，以便检测超过一行；`Window` 要求父排序是 `PartitionBy + OrderBy` 的前缀，并把完整顺序下推。
5. Join 规则中，`ImplHashJoinBuildLeft/Right` 根据 `JoinType` 选择 inner/build side。`getImplForHashJoin` 在父 `ExpectedCnt` 小于 group 行数时，仅按比例缩小 outer child 的期望行数。`ImplMergeJoin` 从等值连接键构造左右排序属性；没有连接键，或父排序列同时跨越左右 schema 时返回空候选。
6. `UnionAll` 为每个孩子复制父 `ExpectedCnt`。`Apply` 只接受排序列全部来自 outer child 的属性，把该排序交给外侧孩子，内侧孩子使用无序无限属性。
7. 候选回到 `implGroup` 后，优化器读取物理计划的孩子要求递归实现各 child group，利用 Implementation 的 `GetCostLimit`/`CalcCost` 剪枝并选最小成本，通过 `AttachChildren` 连接物理树，最后按 required property 缓存。

## 数据与状态

- 输入核心是 `GroupExpr`：`ExprNode` 保存逻辑算子，`Children` 保存孩子 group，`Group` 是指向所属 group 的弱引用。所属 group 的 `Prop` 提供 `Stats`、`Schema`、可能属性，`EngineType` 决定部分算子的 TiDB/TiKV 实现。
- `PhysicalProperty` 的关键字段是 `SortItems` 与 `ExpectedCnt`。本文件通常使用 `CloneEssentialFields` 避免传播非必要状态；排序规则显式构造 `SortItem`；无限输入约定用 `f64::MAX` 表示。
- `StatsInfo` 在大多数能被父行数截断的算子中经 `ScaleByExpectCnt` 缩放。`Limit`、`TopN`、`MaxOneRow` 等保留各自符合 Go 行为的统计初始化方式；Join 会额外用孩子 `RowCount` 计算期望行数。
- schema 通常从所属 group 克隆后写入物理 `PhysicalSchemaProducer`。Join、Apply、Window 等复合算子沿其嵌套的 schema producer 写入；遗漏会让后续索引解析和属性检查失去输出列信息。
- `PlanAdapter<T>` 自身拥有物理计划值；`ImplementationRef` 以 `Rc<RefCell<_>>` 在单线程优化过程中共享候选。候选成本初始及更新由 implementation/optimizer 层维护。
- 成本状态依赖 `PlanContext` 中的 session vars，例如投影、HashJoin、DistSQL scan 并发度，以及 CPU、cop CPU、网络、scan、desc scan、seek、并发因子。解析失败时采用 `vardef` 默认值。

## 依赖与调用关系

上游直接依赖是 `pkg/planner/cascades/old/optimize.rs`：它导入 `ImplementationRule` 和 `defaultImplementationMap`，并在 `implGroupExpr` 调用每条规则。crate 根 `lib.rs` 对外再导出本模块；独立测试也经该再导出访问规则。

下游依赖可分为四组：

- memo 与属性：`astersql-planner-memo` 提供 `GroupExpr`、`GroupRef`、`ImplementationRef`；`astersql-planner-property` 提供 `PhysicalProperty`、`SortItem`、`StatsInfo`；`astersql-planner-cascades-pattern` 提供 `Operand`、`EngineType`。
- 逻辑/物理计划：`astersql-planner-core-operator-logicalop` 提供所有输入逻辑类型和 `PlannerError`；`astersql-planner-core-operator-physicalop` 提供具体物理节点、排序匹配和扫描/连接构造 helper；`astersql-planner-core-base` 提供上下文与计划 trait。
- 实现与成本：`astersql-planner-implementation` 提供 `New*Impl` 包装器及 `PlanAccess`、各种 `*CostPlan` trait。它们反向调用本文件的 `PlanAdapter` 方法取得物理计划、成本参数和行宽估计。
- 统计与配置：`astersql-planner-cardinality`、`astersql-statistics::HistColl` 计算行宽；`astersql-sessionctx-vardef` 定义会话变量名和默认值；`astersql-meta-model`、`astersql-kv` 分别参与临时表判断和 TiKV 行宽估计；`astersql-util-ranger` 包装扫描 ranges。

RustCodeGraph 将 `implementation_rules.rs` 的文件级直接使用者识别为 `implementation_rules_test.rs`；符号级主生产调用关系由 `optimize.rs` 的显式导入及 `NewOptimizer`、`implGroupExpr` 代码确认。文件级“used by”不足以表达 Rust 模块内导入关系，因此生产链以这些符号证据为准。

## 错误处理与边界

- `group_ref` 在所属 group 已被释放时返回 `PlannerError("memo expression is detached from its group")`；`logical<T>` 对 operand/实际动态类型不一致返回包含实际 `TP()` 的错误；`context` 拒绝没有 planner context 的逻辑计划。
- `group_properties`、`child_properties` 对缺失孩子、统计或 schema 返回错误。`histograms` 对缺失或类型不正确的 `HistColl` 返回错误。reader/scan 候选还要求逻辑 data source 存在。
- `Selection`、`HashAgg`、`TopN` 只接受 `EngineTiDB` 或 `EngineTiKV`；其他 engine 返回显式 `PlannerError`，不会静默产生错误候选。
- “合法但本规则无候选”使用 `Ok(Vec::new())`：投影属性不可下推、HashJoin 遇到不支持的 join type、MergeJoin 无连接键或父排序跨两侧 schema，都属于搜索空间中的正常拒绝。
- `ImplHashJoinBuildRight::Match` 只检查空排序，因此 `OnImplement` 必须再次按 `JoinType` 分流。`FullOuterJoin` 返回空候选；`implementation_rules_test.rs::build_right_rejects_full_outer_join` 固定了此边界，并断言它不是实现错误。
- `TopNAsLimit` 的 `Match` 已保证排序表达式可作为列；`OnImplement` 仍做 Rust 动态类型检查，失败则报错。`Count + Offset` 使用 `wrapping_add`，与无 panic 的整数溢出行为一致，但超大 limit 会回绕，是扩展时必须保持或有意识修改的兼容边界。
- 多处属性判断默认上游保证 sort item 与 schema 一致；改变 `MatchItems`、`IsPrefix`、`AllColsFromSchema` 或扫描 handle/index 匹配语义会直接改变候选完整性，不能只以“能够构造物理节点”为正确性依据。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部 I/O。旧优化器的默认实例在 `optimize.rs` 中是 `thread_local! RefCell<Optimizer>`；本文件内部使用的 `Rc<RefCell<_>>`、`Weak::upgrade` 和 `Box<dyn ...>` 都是单线程所有权结构，不应跨线程共享。

`GroupExpr::Group` 是弱引用，防止 memo group 与表达式形成强引用环；规则执行时仅短暂升级，升级失败即报错。对 group、source 和 candidate 的 `RefCell` 借用都限定在局部作用域，尤其 `group_properties`/`child_properties` 在返回前克隆必要数据，避免递归 `implGroup` 时持有借用。

逻辑表达式、列、条件、schema、统计、ranges 和窗口描述符在生成候选时按各类型的 `Clone`/`CloneExpr` 语义复制；物理计划随后归 `PlanAdapter` 所有。最终 `AttachChildren` 才把选中的孩子实现挂到候选；未胜出的 `Rc` 候选在引用释放后自然回收。会话变量只在成本计算时读取，没有在本文件缓存，因此同一次优化应依赖调用期间稳定的 session context。

## 与 Go 版本的对应关系

Rust `ImplementationRule`、`defaultImplementationMap` 和全部规则类型逐项对应 `pkg/planner/cascades/old/implementation_rules.go`；Operand 覆盖及 TopN/Join 内部候选顺序一致。主要语义也保持一致：属性先匹配、`OnImplement` 设置孩子属性、统计按 `ExpectedCnt` 缩放、临时表成本因子特殊处理、Join build side 分流、Window 下推完整排序。

Rust 为适配所有权、动态类型和拆分后的 crate 边界增加了明确层次：

- Go 直接使用指针、类型断言和 group 字段；Rust 用 `Rc<RefCell<_>>`、`Weak`、`Any::downcast_ref` 以及返回 `PlannerError` 的 helper。
- Go 的 implementation 包可直接接收具体物理计划；Rust 使用 `PlanAdapter<T>` 实现成本 trait，并以 `New*Impl` 包装。
- Rust 从 `StatsInfo` 的类型擦除字段恢复 `HistColl`，再调用 cardinality crate；Go 对照版本直接使用 `TblColHists`。这是数据表示差异，不改变 reader/scan 需要统计信息的意图。
- Go `ImplMergeJoin` 调用可返回多个方案的 `physicalop.GetMergeJoin`；当前 Rust 代码由一个 `GetMergeJoin` 结果自行构造孩子排序属性，最终只产生一个候选。文档只能确认当前 Rust 行为，不能声称已覆盖 Go helper 可能枚举的全部方案；扩展 MergeJoin 时应重点核对这一差异。
- Go `ImplApply` 调用 planner core 的 `GetHashJoin`；Rust 在本文件提供同名适配构造函数。Rust 当前构造路径应继续与 Go 的 join condition、inner side 和 hint/属性语义对照，不能因 helper 已能返回物理节点就视为完全等价。
- Rust 独立测试 `implementation_rules_test.rs` 补充了 Go 代码隐含的 `FullOuterJoin -> nil, nil` 语义。相关端到端成本搜索由 `optimize_test.rs::test_impl_group_zero_cost` 间接覆盖；Go 同目录没有独立 `implementation_rules_test.go`，相邻 `optimize_test.go` 是优化主流程的对照测试面。

## 扩展指南

- 新增逻辑算子的物理实现时：新增独立规则类型并实现 `Match`/`OnImplement`，在 `defaultImplementationMap` 对应 `Operand` 中按期望优先级注册；确认 `pattern::GetOperand` 能识别该逻辑类型；为物理节点补齐所需 `PlanAccess`/成本 trait；把回归测试放在同目录独立 `implementation_rules_test.rs`，不要内嵌到生产文件。
- 新增同一 Operand 的候选时，要说明候选顺序、属性覆盖和成本差异。正确性依赖候选集合完整，性能依赖孩子 `ExpectedCnt`、排序属性及成本因子准确；不能用单一“可运行”候选替代 Go 已枚举的方案。
- 修改属性传播时，优先检查 `ImplProjection`、扫描、Sort/TopN、Join、Apply、Window 这些边界；同步验证无排序、升/降序、父期望行数小于输出、属性跨 schema、非列排序表达式等情况。
- 修改 scan/reader 或成本模型时，在 `PlanAdapter` 相应 trait impl 接入；核对普通表/临时表、唯一/非唯一索引、顺序/逆序、range 数、直方图缺失、系统变量未设置或无法解析、并发度为 unset/非正数等分支。成本改变会影响全局计划选择，应补成本及计划选择测试，而不只测试 `OnImplement` 返回非空。
- 扩展 Join 时必须逐一列举所有 `JoinType`、inner child 与 build side，保持 `getImplForHashJoin` 的 outer expected-count 缩放；MergeJoin 还需验证左右键顺序、父属性来源和多个候选枚举。`FullOuterJoin` 目前明确无右建表候选，改变时需同步独立回归。
- 保持 Go 增量对齐原则：先对照 `implementation_rules.go` 的同名规则和相邻 `optimize.go` 调用链，只移植必要语义；Rust 测试继续与生产文件分离。若修改 Rust 源码，应遵循仓库要求保留 PingCAP 许可与顶部 `// Copyright 2026 AsterSQL.`，并在完成后运行 `cargo fmt --all`；本说明任务本身不修改源码也不运行 Cargo。

## 验证依据

- 目标源码：`pkg/planner/cascades/old/implementation_rules.rs`。RustCodeGraph 索引显示 1337 行、127 个符号；已读取全文件，核对 helper、成本适配器、规则表、20 个规则结构体及全部 `Match`/`OnImplement` 分支。
- 生产入口：`pkg/planner/cascades/old/optimize.rs` 中 `Optimizer::NewOptimizer`（调用 `defaultImplementationMap`）、`GetImplementationRules`、`onPhaseImplementation`、`implGroup`、`implGroupExpr`；其中 `implGroupExpr` 明确执行 `Match -> OnImplement`，`implGroup` 明确递归孩子、计算成本、挂接和缓存。
- crate 边界：`pkg/planner/cascades/old/lib.rs` 声明并再导出 `implementation_rules`，以 `#[cfg(test)] mod implementation_rules_test` 挂载独立测试；`pkg/planner/cascades/old/Cargo.toml` 声明 crate 名、Go 包映射以及 planner/memo/property/implementation/statistics/session 依赖。
- Go 对照：`pkg/planner/cascades/old/implementation_rules.go` 全文件，核对接口、默认映射、每个规则的属性匹配、候选顺序、统计/schema 和孩子属性传播；相邻 `optimize.go` 由 RustCodeGraph 查询确认具有对应的 `GetImplementationRules`、`implGroup`、`implGroupExpr` 符号。
- 测试证据：`pkg/planner/cascades/old/implementation_rules_test.rs::build_right_rejects_full_outer_join` 验证 FullOuterJoin 返回空候选且不报错；`pkg/planner/cascades/old/optimize_test.rs::test_impl_group_zero_cost` 验证实现搜索在成本上限过低时返回 `None`，`test_fill_group_stats` 验证实现阶段所需统计的递归填充。Go 同路径未发现 `implementation_rules_test.go`，相关 Go 主流程测试位于 `optimize_test.go`。
- RustCodeGraph 查询：运行了 `status`、`files --filter pkg/planner/cascades/old`、对目标文件/Go 对照/入口/测试的 `node --file`、以及 `query defaultImplementationMap`、`query ImplementationRule`、`query implGroup`、`query GetImplementationRules`、`query getImplForHashJoin`、`query build_right_rejects_full_outer_join`。索引状态为 11467 个文件、307296 个节点、1848419 条边；`callers/callees` 对常见方法名未返回可消歧的边，因此调用关系以精确源码符号和显式导入复核，没有据此臆测。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核唯一生产物、源码链接、当前行为与未验证差异。

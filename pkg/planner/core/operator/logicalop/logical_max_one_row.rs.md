# `pkg/planner/core/operator/logicalop/logical_max_one_row.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-logicalop` crate（见同目录 `Cargo.toml`），定义标量子查询使用的逻辑算子 `LogicalMaxOneRow`。`lib.rs` 以 `mod logical_max_one_row` 纳入模块并通过 `pub use logical_max_one_row::*` 对外导出。它位于表达式重写与物理计划枚举之间：`pkg/planner/core/expression_rewriter.rs::handleScalarSubquery` 调用 `PlanBuilder::buildMaxOneRow` 包装标量子查询，`pkg/planner/core/operator/physicalop/physical_max_one_row.rs::ExhaustPhysicalPlans4LogicalMaxOneRow` 再为该逻辑节点生成根任务的物理算子。

这不是实际执行多行检查的文件；它保存逻辑树节点、阻断不安全的谓词下推、描述输出 schema 与估算统计。运行时“第二行触发标量子查询多行错误”的能力属于物理 `PhysicalMaxOneRow` 及执行器链路。

## 核心职责

1. `LogicalMaxOneRow` 用 `BaseLogicalPlan` 接入统一逻辑计划接口，表达“子查询结果不得超过一行”的语义约束。
2. `Init` 以类型名 `"MaxOneRow"` 和子查询自身的 query-block offset 初始化基座。真实构建点 `logical_plan_builder_runtime.rs::buildMaxOneRow` 还会设置输出 schema、列名、唯一子节点和 `MaxOneRow=true`。
3. 固有方法 `Schema` 克隆唯一子节点的 schema，并调用 `planner_util::ResetNotNullFlag` 清除所有输出列的 NOT NULL 标志；原因是空标量子查询会向上表现为一行 NULL。
4. `PredicatePushDown` 把来自父节点的谓词原样留在本节点上方，只用空谓词列表递归优化子树，并把子树残留条件通过 `AttachSelectionToPlan` 重新挂到子树内部。因此 MaxOneRow 是语义屏障。
5. 固有 `DeriveStats` 与 `getSingletonStats` 把输出估为一行、每个输出列 NDV 为 1，并支持已有统计缓存；但当前固有签名没有覆盖 `LogicalPlan::DeriveStats(bool)`，详见“错误处理与边界”和“与 Go 版本的对应关系”。

## 主要符号

- `pub struct LogicalMaxOneRow { pub BaseLogicalPlan: BaseLogicalPlan }`：本文件唯一状态类型；没有算子私有表达式或配置字段。
- `impl Default`：只创建默认基座。此值尚无子节点，不能安全调用固有 `Schema`；独立测试 `logical_max_one_row_test.rs::schema_requires_a_child_like_go` 明确验证会 panic。
- `Init(self, ctx, offset) -> Self`：用 `NewBaseLogicalPlan(ctx, "MaxOneRow", offset)` 替换默认基座。
- `Schema(&self) -> Schema`：返回子 schema 的拥有型克隆并清除 NOT NULL。它是固有方法；trait 的 `LogicalPlan::Schema(&self) -> &Schema` 仍读取构建器预先写入的基座 schema。
- `PredicatePushDown(&mut self, Vec<Expression>) -> Result<Vec<Expression>>`：保持父谓词，递归处理第一个子节点；`impl LogicalPlan` 显式将 trait 调用转发到该实现。
- `DeriveStats(&mut self, self_schema: &Schema, reloads: &[bool]) -> (StatsInfo, bool)`：固有的 Go 风格统计实现。仅当 `reloads` 恰有一个且为 `true` 时强制刷新；否则复用已有缓存。
- `impl LogicalPlan`：提供运行时类型转换、基座访问与谓词下推覆盖；未覆盖 trait 的 `Schema` 或 `DeriveStats(bool)`。
- `getSingletonStats(&Schema) -> StatsInfo`：公开辅助函数，生成 `RowCount=1.0`、每列 `ColNDVs[UniqueID]=1.0` 的统计，其余字段取默认值。
- `hash64_equals_generated.rs` 中的同类型扩展：Rust 当前按 `LogicalPlan::Schema(self)` 的列计算 `Hash64`/`Equals`，并非本文件自身实现。

## 执行流程

标量子查询主链如下：

1. `expression_rewriter.rs::handleScalarSubquery` 构建子查询逻辑计划后调用 `PlanBuilder::buildMaxOneRow`。
2. `logical_plan_builder_runtime.rs::buildMaxOneRow` 继承子计划的 query-block offset；克隆并清除 schema 的 NOT NULL；浅拷贝输出名；初始化 `LogicalMaxOneRow`；设置基座 schema、输出名、唯一子节点和 `MaxOneRow=true`。
3. 谓词优化通过 trait 分派进入本文件 `PredicatePushDown`。父谓词不会穿过屏障；子节点仍以空输入谓词递归优化，其残留谓词由 `AttachSelectionToPlan` 保存在子树中。
4. decorrelate 阶段的 `optimizer_runtime.rs` 只有在 MaxOneRow 的唯一子节点已经被证明 `MaxOneRow()` 时才移除这层屏障；否则保留它，避免把标量 Apply 改写为丢失运行时基数检查的 Join。
5. 物理枚举 `ExhaustPhysicalPlans4LogicalMaxOneRow` 拒绝排序要求及 Flash/MPP 属性；可行时生成 `PhysicalMaxOneRow`，给子属性设置 `ExpectedCnt=2.0`，使执行端最多探测两行并能在第二行确认违反约束。

## 数据与状态

该结构只拥有 `BaseLogicalPlan`，实际状态包括上下文、算子类型、计划 ID、query-block offset、子节点、基座 schema/输出名、统计缓存及 `max_one_row` 标志等。算子应恰有一个子节点；固有 `Schema` 使用 `Children()[0]`，这是由构建器保证而非类型系统保证的不变量。

schema 存在两种访问语义：固有 `LogicalMaxOneRow::Schema()` 每次从子节点克隆并清除 NOT NULL；`LogicalPlan::Schema()` 返回基座中已经存储的引用。正常构建器同时清除了基座 schema，因此两者预期形状一致，但手工构造或后续只修改子 schema 时可能不同。生成的 Rust hash/equality 明确使用后者。

singleton 统计固定 `RowCount=1.0`，并按每个 `Column.UniqueID` 写入 NDV 1.0。该估计覆盖“零行会物化 NULL 行”和“最多一行”的上层形状，但不携带子统计的其他字段。

## 依赖与调用关系

上游直接证据包括：

- `expression_rewriter.rs::handleScalarSubquery`：标量子查询入口，包装 MaxOneRow 后再判断是否需要 Apply。
- `logical_plan_builder_runtime.rs::buildMaxOneRow`：完整初始化节点的不变量建立点。
- `optimizer_runtime.rs`：识别该类型以处理 distinct 聚合包装和 decorrelation 屏障。
- `logical_apply.rs`、`logical_join.rs`：把 MaxOneRow 视为关联选择上提或 join reorder 的屏障类型。

本文件直接依赖基座与公共规划接口 `BaseLogicalPlan`、`LogicalPlan`、`PredicatePushDownPlan`、`AttachSelectionToPlan`、`Schema`、`StatsInfo` 和 `Result`，依赖 `planner_util::ResetNotNullFlag` 维护空输入的可空语义，使用标准库 `Any` 支持 downcast、`HashMap` 构造列 NDV。其 Cargo crate 依赖由同目录 `Cargo.toml` 声明，其中本文件直接使用的外部 workspace crate 是 `base` 与 `planner_util`；表达式和属性类型经 crate 根重导出。

RustCodeGraph 对 `logical_max_one_row.rs::LogicalMaxOneRow` 的节点查询仅给出 `logicalop_test/hash64_equals_test.rs::TestLogicalMaxOneRowHash64Equals` 的实例化边；因此上述生产调用关系由精确源码搜索和相邻入口读取补足，而不是宣称图索引覆盖了所有 trait/downcast 调用。

## 错误处理与边界

- 固有 `Schema` 直接索引第零个子节点；无子节点时 panic。`Default` 只是装配态，不是可执行计划，测试 `logical_max_one_row_test.rs` 固化了这一边界。
- `PredicatePushDown` 对无子节点保持容错：跳过子树处理并原样返回父谓词。若子树下推或附加 Selection 失败，则通过 crate 的 `Result<_, PlannerError>` 和 `?` 原样传播错误。
- 父谓词绝不能直接下推，因为空子查询经 MaxOneRow 产生 NULL 行；把父过滤移到子树可能改变空集/NULL 语义。
- 固有 `DeriveStats` 只把长度为 1 的 `reloads` 解释为刷新信号，其他长度视为不刷新；它本身不返回错误。
- 当前 `impl LogicalPlan for LogicalMaxOneRow` 没有覆盖 trait 的 `DeriveStats(bool)`。因此经 `dyn LogicalPlan` 调用时会落到 `BaseLogicalPlan::DeriveStats`，对于单子节点继承子统计，而不会使用本文件的 singleton 统计。这是当前接线事实和迁移风险，不能把固有方法的预期描述成已经覆盖所有生产统计路径。
- 物理候选只支持无排序、非 Flash/MPP 属性；不满足时返回空候选，并在有上下文时由物理枚举层报告 MPP 警告。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、显式锁、事务或外部资源。所有修改都通过规划阶段对 `&mut self` 和拥有型 `Vec`/`Schema` 完成；子计划由 `LogicalPlanRef` 持有，其具体共享模型在基座定义。

生命周期重点是计划树所有权：构建器把唯一子计划移入 `SetChildren`；谓词下推原地修改第一个子节点；schema 方法返回克隆，不借出子 schema；统计缓存保存在基座并克隆返回。decorrelator 移除屏障时用 `TakeChildren` 转移子树，只有先验证子节点自身最多一行才允许这样做。

## 与 Go 版本的对应关系

直接对照文件是同目录 `logical_max_one_row.go`。两端均以基座作为唯一字段，初始化类型为 MaxOneRow，schema 克隆子节点并清除 NOT NULL，谓词下推时禁止父谓词越过屏障，并提供一行/每列 NDV 为 1 的统计辅助。

目前有三项明确差异：

1. Go `DeriveStats` 正式实现 `base.LogicalPlan` 接口；Rust 的 Go 风格固有 `DeriveStats(&Schema, &[bool])` 没有桥接到当前 trait 签名 `DeriveStats(bool) -> Result<...>`，trait 分派行为不同。
2. Go 生成的 `Hash64`/`Equals` 纳入 `BaseLogicalPlan`，同上下文初始化的两个节点因计划 ID 不同而不相等，统一 ID 后相等；Rust `hash64_equals_generated.rs` 只比较基座 schema 列。对应 Rust 与 Go 测试分别位于 `logicalop_test/hash64_equals_test.rs` 和 `.go`，测试期待也反映这一差异。
3. Go 的 `Schema` 返回 schema 指针；Rust 固有方法返回拥有型克隆，同时 trait 另有返回基座引用的同名方法。正常 builder 接线预先同步基座 schema，但两条访问路径并非同一实现。

这些差异说明文件处于功能移植状态；不能仅凭同名函数认定 Go 行为已完全对齐。

## 扩展指南

- 修复统计接线时，应在 `impl LogicalPlan for LogicalMaxOneRow` 中覆盖当前 trait 的 `DeriveStats(bool)`，先按统一递归约定处理唯一子节点或明确说明无需子统计，再调用 singleton 逻辑；不要仅保留另一个无法动态分派的重载。测试应放在独立 `logical_max_one_row_test.rs`，覆盖缓存命中、reload、零列/多列 NDV 以及 `Box<dyn LogicalPlan>` 分派。
- 修改 schema 行为时必须同步检查 `logical_plan_builder_runtime.rs::buildMaxOneRow` 的预写 schema、固有与 trait 两种 `Schema` 调用、物理候选复制 schema，以及空输入 NULL 语义。至少增加“子列原为 NOT NULL，输出已清除”的独立测试。
- 放宽谓词下推前必须证明对空集物化 NULL 和多行错误均语义等价；通常只能优化子树自身已有谓词，不能传入父谓词。测试需覆盖空子查询、单行、多行和含 NULL 条件。
- 修改 hash/equality 时应同时对照 Go 的计划 ID 语义和 Rust memo/规则使用方式，并同步 `hash64_equals_generated.rs` 的独立测试；避免 schema 访问两条路径造成 hash 在子树变更后陈旧。
- 修改 decorrelation 或物理属性支持时，应联动 `optimizer_runtime.rs` 的屏障判定及 `physical_max_one_row.rs::ExhaustPhysicalPlans4LogicalMaxOneRow`，保留“第二行用于报错”的 `ExpectedCnt=2.0` 约束，并验证 MPP/排序兼容性。

## 验证依据

- 目标源码：`pkg/planner/core/operator/logicalop/logical_max_one_row.rs`，核对结构、全部固有方法、trait 实现和 singleton 统计。
- crate/模块边界：同目录 `Cargo.toml`、`lib.rs`；确认 crate 名、Go 包映射、模块声明、公开重导出和独立测试装配。
- RustCodeGraph：`status` 显示索引可用；`query LogicalMaxOneRow --json` 定位 Rust/Go 定义、hash 测试和物理枚举；`node logical_max_one_row.rs::LogicalMaxOneRow` 确认结构源码及测试实例化边。文件过滤/探索未返回完整目标调用边，生产链改用精确源码检索核验。
- 构建与入口：读取 `logical_plan_builder_runtime.rs::buildMaxOneRow`、`expression_rewriter.rs::handleScalarSubquery`、`optimizer_runtime.rs` 的 MaxOneRow 分支、`physicalop/physical_max_one_row.rs::ExhaustPhysicalPlans4LogicalMaxOneRow`。
- 基座契约：读取 `base_logical_plan.rs` 的 `LogicalPlan` trait、`BaseLogicalPlan::BuildKeyInfo`、`RecursiveDeriveStats` 和 `DeriveStats`，据此确认 schema/统计的 trait 分派差异。
- Go 对照：读取同目录 `logical_max_one_row.go` 与 `hash64_equals_generated.go`。
- 测试证据：读取 `logical_max_one_row_test.rs`（无子节点 schema panic）、`logical_d_aster_unit_test.rs::singleton_statistics_keep_one_row_and_one_ndv_per_column`、Rust/Go `logicalop_test/hash64_equals_test.*`，以及 `optimizer_logical_entry_aster_unit_test.rs` 中保留 decorrelation 屏障的断言位置。
- 本任务是纯文档分析，按总计划不运行 Cargo；最终以任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复查上述每项结论均可回指到真实符号或文件。

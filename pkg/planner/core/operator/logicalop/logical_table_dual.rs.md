# `pkg/planner/core/operator/logicalop/logical_table_dual.rs`

## 文件定位

本文件定义逻辑计划叶子节点 `LogicalTableDual`，属于 Cargo crate `astersql-planner-core-operator-logicalop`。crate 在 `lib.rs` 中以 `mod logical_table_dual` 装入该模块并通过 `pub use logical_table_dual::*` 对外导出。它表示不读取真实表的常量行源：`RowCount == 0` 表示空结果，`RowCount == 1` 表示一行结果；常见来源是无 `FROM` 的 `SELECT`、`DO`，以及被常量条件或零行 `LIMIT` 折叠的计划。

该节点位于“SQL 构建/逻辑改写 -> 逻辑优化 -> 物理计划”的中间层。`logical_plan_builder_runtime.rs::build_select_source` 为无 `FROM` 的查询建立一行、零列 Dual，`build_do_runtime` 也以它作为表达式求值的叶子；`build_limit_runtime` 和 `LogicalSelection` 的谓词下推路径可建立零行 Dual。物理规划由 `physicalop::ExhaustPhysicalPlans4LogicalTableDual` 将其转换为 `PhysicalTableDual`。

## 核心职责

- 用 `LogicalSchemaProducer` 保存逻辑计划基类、输出 schema、输出名和统计信息，用 `RowCount` 保存常量基数。
- 通过 `Init` 设置计划类型 `plancodec::TypeDual`、规划上下文和查询块偏移。
- 提供 EXPLAIN 文本以及用于逻辑计划识别的稳定字节编码。
- 作为无子节点的谓词下推边界：自身不消费谓词，原样返回残余谓词。
- 原地裁剪不被父节点使用的输出列；零列 schema 是合法状态。
- 根据行数设置 `MaxOneRow`，并推导行数及逐列 NDV 统计。
- 通过 `LogicalPlan` trait 将上述固有方法接入统一优化器接口，并提供 `Any` 向下转型和基类访问。

本文件不负责生成数据行、不负责表达式求值，也不直接访问 KV/存储；这些职责在后续物理算子及执行层完成。

## 主要符号

### `pub struct LogicalTableDual`

- `LogicalSchemaProducer: LogicalSchemaProducer`：嵌入 schema 生产者与 `BaseLogicalPlan`。字段名沿用 Go 风格，相关公共行为（上下文、类型、查询块、schema、统计缓存等）由基类提供。
- `RowCount: i32`：Dual 的常量行数。设计语义要求构造方传入 0 或 1，但 `Init` 不校验或归一化；独立 Rust 测试特意用 2、-1 验证透传、哈希和防御性边界。
- `Default`：派生默认值。只用于占位或随后显式初始化时，默认对象没有可用的规划上下文。

### 固有方法

- `Init(self, ctx, offset) -> Self`：以 `TypeDual` 调用 `NewBaseLogicalPlan`，保留已有 `RowCount` 和 schema 生产者其余状态。
- `ExplainInfo(&self) -> String`：返回 `rowcount:<值>`。
- `HashCode(&self) -> Vec<u8>`：依次写入物理计划类型 ID、查询块偏移、`RowCount`，每段均为 32 位无符号大端编码，总长 12 字节。负数按 Rust `as u32` 的二进制补码位模式编码。
- `PredicatePushDown(&mut self, predicates) -> Result<Vec<Expression>>`：成功返回原谓词列表。
- `PruneColumns(&mut self, parent_used_cols) -> Result<()>`：计算 schema 中每列是否被父节点使用，按原顺序保留使用列。
- `BuildKeyInfo(&mut self)`：先委托 `LogicalSchemaProducer::BuildKeyInfo`，再把 `MaxOneRow` 精确设置为 `RowCount == 1`。
- `DeriveStats(&mut self, reload) -> Result<(StatsInfo, bool)>`：可复用统计缓存；重算时令总行数和每个输出列的 NDV 都等于 `RowCount`。

### `impl LogicalPlan for LogicalTableDual`

`as_any`/`as_any_mut` 支持优化规则按具体类型识别 Dual，`base`/`base_mut` 暴露内嵌基类；其余方法薄转发到同名固有方法。此实现使节点能装入 `Box<dyn LogicalPlan>` 计划树。

此外，`hash64_equals_generated.rs` 为该类型另行提供 `Hash64`/`Equals`：它们比较 `LogicalSchemaProducer` 和 `RowCount`。这组结构相等性 API 与本文件的 12 字节 `HashCode` 用途不同，扩展字段时二者都需要评估。

## 执行流程

1. 构建器或优化规则创建 `LogicalTableDual { RowCount, ..Default::default() }`。
2. 调用 `Init(ctx, query_block)` 写入 `BaseLogicalPlan`，再按调用现场设置 schema 和输出名。无 `FROM` 的查询通常是一行、零列；空结果替换通常保留被替换子树的 schema/输出名。
3. 逻辑优化通过 `LogicalPlan` trait 调用公共阶段：
   - 谓词下推到达 Dual 时，`PredicatePushDown` 不求值、不丢弃谓词，而是全部上返；上层负责保留或物化 Selection。
   - 列裁剪根据父用列过滤 schema。存在 session 表达式上下文时使用 `expression::GetUsedList`；没有上下文时退化为按 `Column.UniqueID` 集合匹配，保证默认/占位 Dual 也可裁剪。
   - 键信息阶段把“一行”转换为 `MaxOneRow=true`，其他值为 false。
   - 统计阶段若 `reload=false` 且已有缓存则原样复用并返回 `changed=false`；否则按当前 `RowCount` 重建统计、写回缓存并返回 `changed=true`。
4. 物理计划路由在 `base_physical_plan.rs` 识别具体类型并调用 `ExhaustPhysicalPlans4LogicalTableDual`。该函数拒绝 IndexJoin 属性，也拒绝“要求排序且 `RowCount > 1`”的异常输入；正常情况下复制 schema、统计和查询块偏移，生成 `PhysicalTableDual`。

相邻优化还会直接利用该类型：`LogicalTopN::AttachChild` 对 Dual 直接计算 offset/count 后的新行数；`LogicalSelection` 在条件恒假时用零行 Dual 替换子树；`AttachSelectionToPlan` 发现零行 Dual 时跳过无意义的 Selection。

## 数据与状态

`LogicalTableDual` 自身只有两个状态来源：`LogicalSchemaProducer` 与 `RowCount`。它没有子节点专属状态、游标或数据缓冲。关键不变量如下：

- 业务语义中的 `RowCount` 应为 0 或 1；当前实现信任构造方，不在 `Init`、`BuildKeyInfo` 或 `DeriveStats` 中拒绝非法值。
- schema 可以为空。Go 文件注释明确指出 `buildTableDual()` 等场景允许“0/1 行、0 列”，Rust 测试 `pruning_all_unused_columns_can_leave_a_zero_column_dual` 固化了这一行为。
- `HashCode` 的身份由计划类型、查询块偏移和行数组成，不含 schema；生成的 `Hash64`/`Equals` 则包含 schema 生产者和行数。调用方不可混淆两种契约。
- 统计缓存可能暂时落后于 `RowCount`：修改行数后用 `DeriveStats(false)` 会继续返回旧缓存，只有 `reload=true` 才强制刷新。
- `BuildKeyInfo` 会覆盖旧的 `MaxOneRow` 状态：只有恰好一行时为 true。这比 Go 当前代码仅在一行时置 true 更具清除陈旧标志的效果，Rust 独立测试明确要求该行为。

## 依赖与调用关系

直接依赖来自本 crate 的再导出：`BaseLogicalPlan`、`LogicalSchemaProducer`、`LogicalPlan`、`Column`、`Expression`、`StatsInfo` 和统一 `Result`；外部 crate 直接使用 `base::ContextRef`、`expression::GetUsedList` 与 `plancodec`。`Cargo.toml` 声明该 crate 为 `astersql-planner-core-operator-logicalop`，并以路径依赖连接 `base`、`expression`、`property`、`plancodec` 等规划组件，`package.metadata.porting.go-package` 指向同路径 Go 包。

主要上游创建/改写点包括：

- `logical_plan_builder_runtime.rs::{build_select_source, build_do_runtime, build_limit_runtime}`：分别建立无 FROM、DO 和零行 LIMIT 的 Dual。
- `logical_selection.rs::{PredicatePushDown, PredicatePushDownRoot}`：恒假条件折叠为空 Dual，并保留 schema/输出名。
- `logical_top_n.rs::AttachChild`：直接调整 Dual 的行数，避免额外节点。
- `logical_datasource.rs`、`logical_join.rs`、`logical_plans_misc.rs`：在各自可证明为空或常量的改写中构造 Dual。
- `base_logical_plan.rs::{PredicatePushDownPlan, AttachSelectionToPlan}`：通过 trait 调度本文件方法，并对零行 Dual 做残余谓词处理。

主要下游是 `LogicalSchemaProducer`/`BaseLogicalPlan` 的公共状态、`expression::GetUsedList` 的列匹配、`StatsInfo` 的统计缓存，以及 `physical_table_dual.rs::ExhaustPhysicalPlans4LogicalTableDual` 的物理化。RustCodeGraph 将目标文件标记为被 27 个文件使用；由于索引未返回这些 Rust 方法的精确 callers/callees，以上调用边均由局部源码引用复核，不将数量当作完整调用图。

## 错误处理与边界

- `PredicatePushDown`、`PruneColumns`、`DeriveStats` 使用统一 `Result`，但当前本地分支均不主动构造错误；保留 `Result` 是为了满足逻辑计划接口及未来依赖可能失败的演进。
- `PruneColumns` 在 `SCtx()` 存在时走表达式上下文；缺失上下文时按 `UniqueID` 降级。该分支避免测试/占位计划因没有 session context 而 panic。
- `used[index]` 假定 `GetUsedList` 的返回长度与当前 schema 列数一致，这是表达式模块与计划节点之间的接口不变量；本文件没有额外长度检查。
- `RowCount` 不是运行时验证边界。负值或大于 1 的值会进入 EXPLAIN、哈希和统计；物理化只对“多行且要求排序”做特殊拒绝。生产构造点仍应维持 0/1 约束。
- `Init` 后才应依赖 `SCtx`、计划类型和查询块偏移。`Default` 对象主要适合作为临时占位，不能等同于完整初始化的计划。
- 空 schema 是支持场景，不应为满足“至少一列”的一般规则而人为补列。

## 并发与资源生命周期

本类型不创建线程、异步任务、锁、通道、事务或外部资源，也没有 `Drop` 清理。优化阶段通过 `&mut self` 串行修改 schema、基类标志和统计缓存；共享并发策略由计划树所有者控制。

规划上下文通过 `base::ContextRef` 交给 `NewBaseLogicalPlan` 保存，其所有权/共享生命周期由上下文引用类型管理。表达式上下文只在列裁剪调用期间借用。`StatsInfo` 在返回给调用方前会克隆一次，并另存入节点缓存，因此调用方得到的值不会借用节点内部状态。schema 裁剪用 `std::mem::take` 暂时取得列向量所有权，再按稳定顺序重建，不保留已删除列的资源。

## 与 Go 版本的对应关系

Rust 文件逐项移植 `logical_table_dual.go`：结构字段、`Init`、EXPLAIN、12 字节 `HashCode`、谓词原样返回、列裁剪、键信息及统计推导均有直接对应。`Cargo.toml` 的 `go-package` 元数据也明确绑定到 `pkg/planner/core/operator/logicalop`。

已核对的等价点：

- 两端 `Init` 都保留调用者给出的 `RowCount`，并以 `TypeDual` 和 query-block offset 初始化基类。
- 两端 `HashCode` 都按 PlanType、Select/QueryBlockOffset、RowCount 顺序编码为三个 uint32；Rust 的负数转 `u32` 与 Go 编码低 32 位的意图一致，独立 Rust 测试覆盖 `-1 -> u32::MAX`。
- 两端谓词下推均不消费谓词。
- 两端裁剪均根据父用列过滤 schema，并允许得到零列；Rust 额外提供无 `SCtx` 时的 `UniqueID` 回退。
- 两端统计都令 `RowCount` 与每列 NDV 相等，并在未要求 reload 时复用缓存。

需要注意的差异：Go `BuildKeyInfo` 只在 `RowCount == 1` 时设 true，未显式清除既有 true；Rust 使用 `SetMaxOneRow(self.RowCount == 1)` 明确刷新真假。Rust 测试将该行为作为防陈旧状态契约。Go 的方法签名还会返回计划自身或接收 child/self schema 参数；Rust trait 已把计划保留在 `&mut self` 上，并从节点自身读取 schema，因此签名更简洁但语义目标相同。

Go 测试 `logicalop_test/hash64_equals_test.go::TestLogicalTableDualHash64Equals` 与 Rust 对应测试共同验证 schema/RowCount 参与 `Hash64`/`Equals`；`logical_operator_test.go::TestLogicalTopNPruneColumnsRefreshesSchemaBeforeInlineProjection` 使用 Dual 作为列裁剪链路的叶子。测试逻辑保持在独立测试文件中，没有内嵌到生产源文件。

## 扩展指南

- 新增影响计划身份的字段时，同时审查本文件 `HashCode`、`hash64_equals_generated.rs::{Hash64, Equals}`、生成器配置及 Go 同路径实现，避免缓存键与结构相等性不一致。
- 改变 `RowCount` 合法范围时，需同步审查 `BuildKeyInfo`、`DeriveStats`、`LogicalTopN::AttachChild`、物理 Dual 的属性满足条件以及所有 0/1 构造点；不能只放宽字段注释。
- 改变 schema 裁剪时，保持列顺序、零列合法性和无上下文回退；测试放在同目录独立的 `logical_table_dual_test.rs`，不要放入生产文件。
- 改变谓词行为时，必须联动 `PredicatePushDownPlan`、`AttachSelectionToPlan` 与 `LogicalSelection` 的替换语义，证明残余谓词不会静默丢失。
- 新增可能失败的步骤应返回带上下文的规划错误，并补充失败路径测试；当前几个 `Result` 方法“不会失败”不是永久保证。
- 扩展物理属性能力时同步审查 `physical_table_dual.rs::ExhaustPhysicalPlans4LogicalTableDual` 与 `base_physical_plan.rs` 的类型路由。
- 性能上应保留叶子节点的 O(列数) 裁剪/统计推导及 O(1) 哈希、EXPLAIN；避免为无存储访问的 Dual 引入扫描或异步资源。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/planner/core/operator/logicalop/logical_table_dual.rs`（`LogicalTableDual`、全部固有方法及 `LogicalPlan` 实现）。
- crate 与模块边界：`pkg/planner/core/operator/logicalop/Cargo.toml`、`pkg/planner/core/operator/logicalop/lib.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_table_dual.go`。
- 独立 Rust 测试：`pkg/planner/core/operator/logicalop/logical_table_dual_test.rs`，覆盖初始化/EXPLAIN、负数哈希、零列裁剪、MaxOneRow 刷新和统计缓存/reload。
- 结构哈希测试：`pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.rs`；Go 对照测试位于同目录 `hash64_equals_test.go` 和 `logical_operator_test.go`。
- 上下游证据：`logical_plan_builder_runtime.rs`、`base_logical_plan.rs`、`logical_selection.rs`、`logical_top_n.rs`、`logical_plans_misc.rs`、`physicalop/physical_table_dual.rs`、`physicalop/base_physical_plan.rs`。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 边；`query LogicalTableDual` 命中本文件结构体及 Go 对照；目标文件节点显示完整 152 行源码并列出 27 个引用文件。精确 Rust 方法 callers/callees 查询未产生结果，因此方法级调用边改由上述局部源码引用核对。

这是纯文档分析，没有运行 Cargo 或代码测试。交付验证采用任务指定的结构命令，确认目标文件存在且恰有 11 个固定二级章节；另人工复核章节覆盖“为何存在、如何运行、如何安全扩展”，且未把未返回的图查询解释为已验证调用边。

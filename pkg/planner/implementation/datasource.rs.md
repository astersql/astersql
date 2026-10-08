# `pkg/planner/implementation/datasource.rs`

## 文件定位

本文件属于 `astersql-planner-implementation` crate；crate 入口 `pkg/planner/implementation/lib.rs` 将私有 `datasource` 模块的公开项重新导出。它位于旧 Cascades/Memo 优化链的“逻辑规则已选出物理节点”与“Memo 对物理候选计价、剪枝并最终挂接子计划”之间：`pkg/planner/cascades/old/implementation_rules.rs` 的 `ImplTableDual`、`ImplMemTableScan`、`ImplTiKVSingleReadGather`、`ImplTableScan` 和 `ImplIndexScan` 构造这里定义的六类 Implementation。

`pkg/planner/implementation/Cargo.toml` 将该目录定义为独立 library crate（`lib.rs` 为入口），没有 feature 条件；本文件直接使用 expression、meta model、planner core/base、planner memo 和 statistics 等工作区 crate。文件中没有条件编译项，也不是生成代码、门面或桩。

## 核心职责

- 用 `TableDualImpl`、`MemTableScanImpl`、`TableReaderImpl`、`TableScanImpl`、`IndexReaderImpl`、`IndexScanImpl` 包装数据源类物理计划，并通过 `impl_implementation!` 接入 `astersql_planner_memo::Implementation`。
- 保存计价所需的 `TableInfo`、`HistColl` 和投影列，按 Go 版本的旧代价模型计算扫描、网络、子计划和索引 seek 代价。
- 通过 `AttachChildren` 把 Memo 中选出的子 Implementation 所持计划克隆到当前物理计划，形成可执行物理树。
- 为 Reader 将父代价上限按 coprocessor worker 数反向放大；其余实现沿用 `BaseImpl::GetCostLimit` 的“父上限减去已有子代价”规则。

本文件只建模优化器代价与计划装配，不执行表或索引 I/O，也不拥有统计信息的加载、访问路径枚举或物理算子执行生命周期。

## 主要符号

- `zero_cost_implementation!`：模块内宏，生成一个持有 `BaseImpl` 和 `Box<dyn PlanAccess>` 的公开结构及公开构造器，并补齐 `calc_cost`、`plan`、`attach_children`、`cost_limit`。它实例化 `TableDualImpl`/`NewTableDualImpl` 与 `MemTableScanImpl`/`NewMemTableScanImpl`。其 `calc_cost` 返回 `0.0`，但刻意不写回 `BaseImpl` 的缓存。
- `NewTableReaderImpl(plan, table_info, histograms) -> TableReaderImpl`：保存 Reader 物理计划、表元信息和列直方图。`TableReaderImpl::calc_cost` 用 Reader 自身计划估算普通表行宽。
- `NewTableScanImpl(plan, columns, histograms) -> TableScanImpl`：保存表扫描物理计划、投影列与直方图。`TableScanImpl::calc_cost` 根据 `Descending()` 选择正序或逆序扫描因子。
- `NewIndexReaderImpl(plan, table_info, histograms) -> IndexReaderImpl`：与 TableReader 的状态布局相同，但 `IndexReaderImpl::calc_cost` 用第一个子计划估算索引行宽，并向 `AverageRowSize` 传入 `index = true`。
- `NewIndexScanImpl(plan, histograms) -> IndexScanImpl`：保存索引扫描计划和直方图；`IndexScanImpl::calc_cost` 在扫描代价之外按 `RangeCount()` 叠加 seek 代价。
- `impl_implementation!(...)`：定义于 `base.rs`，为上述类型公开 `CalcCost`、`SetCost`、`GetCost`、`GetPlan`、`AttachChildren` 和 `GetCostLimit` 这一 Memo trait 契约；具体类型的小写方法仍为文件内实现细节。

## 执行流程

1. `implementation_rules.rs` 先把逻辑节点转换为具体物理计划。Dual 和 MemTable 直接包装；`ImplTiKVSingleReadGather` 根据 `IsIndexGather` 选择 IndexReader 或 TableReader；TableScan/IndexScan 规则从逻辑数据源提取表、列、ranges 和统计信息后调用对应构造器。
2. 构造器创建空的 `BaseImpl` 代价缓存，并保存 `PlanAccess` 或专门的 `*CostPlan` trait object 及计价上下文。
3. Memo 调用 `CalcCost(out_count, children)`：
   - Dual/MemTable 固定返回 `0`；
   - TableReader 计算 `(out_count × NetworkFactor(table) × AverageRowSize(histograms, reader_plan, false) + children[0].cost) / workers`；
   - IndexReader 使用同一公式，但行宽来自 `children[0].plan` 且 `index = true`；
   - TableScan 计算 `out_count × AverageRowSize(histograms, columns) × (DescScanFactor 或 ScanFactor)`；
   - IndexScan 在对应扫描项后增加 `RangeCount × SeekFactor`。
4. 除零代价宏生成的两类外，计算结果经 `BaseImpl::SetCost` 缓存，供父实现读取和候选比较。
5. 剪枝查询 `GetCostLimit` 时，Reader 将上限乘以 worker 数，并在乘法会溢出 `f64::MAX` 时饱和到 `f64::MAX`；其他类型委托 `BaseImpl` 扣除已有子代价。
6. 候选确定后，`AttachChildren` 克隆每个子 Implementation 的物理计划并调用当前计划的 `set_children`，完成物理树连接。

## 数据与状态

所有类型都内嵌 `BaseImpl`；其 `Cell<f64>` 保存最近一次写入的代价，允许 `CalcCost(&self, ...)` 在不可变借用下更新缓存。各 `plan_node` 是拥有所有权的 boxed trait object：零代价类型只要求 `PlanAccess`，其余分别要求 `ReaderCostPlan`、`TableScanCostPlan` 或 `IndexScanCostPlan`，把具体物理节点与代价参数提取解耦。

`TableReaderImpl` 和 `IndexReaderImpl` 各自拥有 `TableInfo` 与 `HistColl`；`TableScanImpl` 还拥有 `Vec<Column>`；`IndexScanImpl` 只额外保存 `HistColl`。这些都是构造时的快照/克隆值，本文件没有刷新统计信息的入口。`children` 以 `ImplementationRef` 切片传入，Reader 只读取索引 0；挂接时则克隆所有子计划，而不是把 Memo 引用直接保存到物理节点。

重要不变量是调用者必须提供与算子形态一致的子节点：两个 Reader 的计价无条件访问 `children[0]`；Dual、MemTable 和两个 Scan 不读取子节点代价。扫描因子、worker 数、range 数和行宽由 trait 实现提供，本文件不再次校验其业务合法性。

## 依赖与调用关系

上游直接调用者集中在 `pkg/planner/cascades/old/implementation_rules.rs`：`ImplTableDual::OnImplement` 调用 `NewTableDualImpl`，`ImplMemTableScan::OnImplement` 调用 `NewMemTableScanImpl`，`ImplTiKVSingleReadGather::OnImplement` 调用两个 Reader 构造器之一，`ImplTableScan::OnImplement` 与 `ImplIndexScan::OnImplement` 分别调用两个 Scan 构造器。`pkg/planner/implementation/lib.rs` 通过 `pub use datasource::*` 把这些构造器和类型暴露给规则 crate 使用。

下游依赖由 `pkg/planner/implementation/base.rs` 定义：`PlanAccess` 提供物理计划读写入口，三个 `*CostPlan` trait 提供计价参数，`AttachChildren` 负责计划克隆/挂接，`BaseImpl` 负责代价缓存和默认上限计算，`impl_implementation!` 完成 Memo trait 适配。外部数据类型来自 `astersql_expression::Column`、`astersql_meta_model::TableInfo`、`astersql_planner_core_base::PhysicalPlan`、`astersql_planner_memo::ImplementationRef` 和 `astersql_statistics::HistColl`。

RustCodeGraph 的文件关系显示 `datasource.rs` 被 `implementation_rules.rs` 与 `base_test.rs` 使用；精确符号查询同时找到 Rust 构造器及 `datasource.go` 中的同名 Go 构造器。构造器的 callers 图没有产出直接边，故上述上游边由精确引用搜索和规则源码共同确认，而非把缺失的图边解释为“未接线”。

## 错误处理与边界

本文件的公开构造器与计价方法不返回 `Result`，没有显式错误分支。输入契约被违反时遵循 Rust/IEEE-754 的直接行为：Reader 缺少第一个 child 会因 `children[0]` 越界而 panic；`CopIteratorWorkers() == 0` 时，正数分子除以零得到正无穷，代价上限计算返回 `0`；若分子也为零则按浮点规则可能得到 NaN。现有测试只明确锁定正数分子产生无穷和上限为零的情况。

Reader 的 `cost_limit` 用 `f64::MAX / workers < cost_limit` 预判乘法溢出并饱和，但不限制负数、NaN 或无穷输入。Scan 公式同样不裁剪异常的行数、行宽和因子。`AttachChildren` 内部的物理计划克隆在 `base.rs::ClonePlan` 中以 `expect` 要求计划可克隆，失败会 panic。以上均是当前接口边界，不应在扩展时静默“修正”而偏离 Go 或 Memo 约定。

## 并发与资源生命周期

文件不创建线程、异步任务、通道、锁、事务或外部 I/O 资源。唯一的并发相关参数是 `CopIteratorWorkers()`：它只参与 Reader 的代价摊薄与代价上限换算，不负责启动或管理 worker。

`BaseImpl` 使用 `Cell<f64>` 做内部可变缓存，因此这些实现天然面向优化器中的局部、单线程借用模型，不能由本文件推断为可跨线程共享。`ImplementationRef` 的读取通过 `borrow()` 完成；借用只覆盖读取 child 计划/代价的局部表达式。对象拥有 boxed 计划及统计/元信息，随 Implementation 一起释放；`AttachChildren` 把子计划的克隆交给父物理计划持有，Memo 引用本身不进入最终树。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/implementation/datasource.go`。六类结构、构造器以及四套核心公式逐项对应：Dual/MemTable 返回零；两个 Reader 累加网络与 child 代价后除以 `DistSQLScanConcurrency`；TableScan 按顺序方向选择扫描因子；IndexScan 另加 ranges 数量乘 seek 因子；Reader 的代价上限都有溢出饱和保护。

Rust 将 Go 构造器从 `logicalop.DataSource` 提取 `TableInfo`/`TblColHists` 的动作移到了调用规则中，再把拥有所有权的值显式传入；将 Go 对具体 `physicalop` 的类型断言改为 `ReaderCostPlan`、`TableScanCostPlan`、`IndexScanCostPlan` trait object。TableReader 仍以 Reader 自身计划/schema 为行宽来源，IndexReader 仍以第一个子计划/schema 为来源，这一差异由 `base_test.rs::readers_use_the_same_row_size_source_as_go` 锁定。

Go 的零代价 `CalcCost` 不写 `baseImpl.cost`，Rust 宏保持相同语义；`base_test.rs::zero_cost_calc_does_not_reset_cached_cost` 验证已缓存的手工代价不会被清零。`reader_zero_workers_follow_go_float_semantics` 则验证 Rust 没有擅自把 Go 的零并发浮点行为改成错误或最小并发度。未发现专门覆盖 TableScan/IndexScan 公式的独立 Rust 测试，因此这部分一致性依据是逐行公式对照与规则接线，而不是测试证明。

## 扩展指南

- 调整某类代价公式时，先修改相应 `calc_cost`，再检查 `base.rs` 中对应 `*CostPlan` trait 及 `implementation_rules.rs` 的 adapter 是否能提供新参数；同时逐项对照 `datasource.go`，避免只在 Rust 侧引入未经要求的模型差异。
- 新增数据源 Implementation 时，应在独立生产文件或本文件定义状态与构造器，通过 `impl_implementation!` 接入 Memo，并在 `defaultImplementationMap` 所在规则链登记逻辑到物理候选；不要只创建结构而不接线。
- 若 Reader 公式变化，必须同步审视 `GetCostLimit` 的逆变换，否则候选计价与剪枝预算会不一致。处理 worker 为零、乘法溢出、NaN/无穷时应保留明确的 Go 兼容决策。
- 若改变行宽来源，需分别确认 TableReader 使用自身计划、IndexReader 使用 child 计划的设计；不能因为结构相似而合并为同一来源。
- 测试应继续放在独立文件。优先扩展 `pkg/planner/implementation/base_test.rs`，补齐 TableScan/IndexScan 的正序、逆序、多个 range、缓存回写及 Reader 上限溢出案例；规则构造与属性匹配应放到 `pkg/planner/cascades/old/implementation_rules_test.rs`。不要把测试嵌入 `datasource.rs`。
- 性能风险主要来自错误的行宽/因子放大、遗漏 worker 摊薄或 range seek 项；兼容风险主要来自改变零 worker、浮点特殊值、缺 child panic 或零代价缓存语义。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用；`node --file pkg/planner/implementation/datasource.rs --offset 1 --limit 500` 读取全部 276 行，并报告直接使用文件为 `pkg/planner/cascades/old/implementation_rules.rs` 和 `pkg/planner/implementation/base_test.rs`。
- RustCodeGraph 精确查询：`query NewTableReaderImpl`、`query NewTableScanImpl`、`query NewIndexReaderImpl`、`query NewIndexScanImpl` 均同时定位 Rust 与 Go 同名构造器；对 Rust 限定符执行 callers 查询未返回边，因此又以精确引用搜索核实调用点。
- 已读生产与配置文件：`pkg/planner/implementation/datasource.rs`、`pkg/planner/implementation/base.rs`、`pkg/planner/implementation/lib.rs`、`pkg/planner/implementation/Cargo.toml`、`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/implementation/datasource.go`。
- 已读独立测试：`pkg/planner/implementation/base_test.rs` 的 `zero_cost_calc_does_not_reset_cached_cost`、`reader_zero_workers_follow_go_float_semantics`、`readers_use_the_same_row_size_source_as_go`；另检查 `pkg/planner/cascades/old/implementation_rules_test.rs`，当前仅覆盖 HashJoin 分支，没有数据源规则专项测试。
- 本任务是纯文档分析，依计划不运行 Cargo。结构通过任务规定的 11 标题检查；事实通过上述符号、调用点、Go 公式和测试断言人工交叉复核。

# `pkg/planner/core/operator/logicalop/logical_tikv_single_gather.rs`

## 文件定位

本文件定义逻辑算子 `TiKVSingleGather`，位于 `astersql-planner-core-operator-logicalop` crate。模块由 `pkg/planner/core/operator/logicalop/lib.rs` 声明并公开再导出；crate 的 `Cargo.toml` 通过 `[package.metadata.porting].go-package` 将其对应到 Go 包 `pkg/planner/core/operator/logicalop`。

它处在访问路径选择与物理读算子之间：`DataSource::Convert2Gathers` 在 `logical_datasource.rs:1671` 把表路径或索引路径包装成一个扫描子节点和一个 `TiKVSingleGather`；随后物理计划代码按 gather 中保存的路径元数据选择 reader、过滤不符合存储属性的候选或为 Index Join 寻找探测路径。这里的“Gather”是逻辑计划边界描述，不负责网络收包或线程调度。

## 核心职责

- 保存唯一扫描子节点上方的汇集边界，以及原始 `DataSource`、表/索引路径类别、索引元数据、存储类型、是否双读和表侧过滤条件（`TiKVSingleGather`）。
- 初始化逻辑计划公共基座，使算子类型名为 `TiKVSingleGather` 并保留查询块偏移（`Init`）。
- 为 `EXPLAIN` 复用数据源说明，并在索引汇集时补充索引名（`ExplainInfo`）。
- 透明继承唯一子节点的键、统计和可能排序属性，同时把 TiFlash 可用性写回公共基座（`BuildKeyInfo`、`DeriveStats`、`PreparePossibleProperties`）。
- 实现 `LogicalPlan` 的类型擦除和基座访问接口，使下游可通过 `as_any().downcast_ref::<TiKVSingleGather>()` 识别并消费该节点。

## 主要符号

`pub struct TiKVSingleGather` 是唯一模块级类型，没有模块级常量、条件编译项或私有辅助函数。其字段语义如下：

- `LogicalSchemaProducer`：承载 `BaseLogicalPlan`、输出 schema、列名、子节点、统计缓存和 `has_ti_flash` 缓存。
- `Source: Option<DataSourceRef>`：共享的原始数据源；`DataSourceRef` 是 `Rc<RefCell<DataSource>>`。`Option` 允许默认构造和测试构造，但正常的 gather 构造路径会设置它。
- `IsIndexGather` 与 `Index`：区分表扫描和索引扫描，并在索引路径上保存 `IndexInfo`。物理化和 `ExplainInfo` 都读取这组字段。
- `StoreType`：路径对应的存储引擎，默认 `kv::StoreType::TiKV`；`buildTableGather`/`buildIndexGather` 会用 `AccessPath.StoreType` 覆盖默认值。
- `IsDoubleRead`：索引是否不能覆盖查询列，需要索引读后回表；只在索引 gather 构造时按 `!is_single_scan` 设置。
- `TableFilters`：访问路径中留在表侧执行的表达式，构造时从 `AccessPath.TableFilters` 克隆。

公开固有方法为 `Init`、`ExplainInfo`、`BuildKeyInfo`、`DeriveStats`、`PreparePossibleProperties`。`impl LogicalPlan` 公开动态类型访问、基座访问，并把 `ExplainInfo`、`BuildKeyInfo`、`DeriveStats` 转发到同名固有方法。`Default` 提供未接线的安全初值：空来源、表 gather、无索引、TiKV、非双读、无表过滤。

## 执行流程

1. `DataSource::Convert2Gathers` 遍历 `PossibleAccessPaths`。表路径进入 `buildTableGather`，索引路径进入 `buildIndexGather`（`logical_datasource.rs:1572-1683`）。
2. 表路径构造 `LogicalTableScan`，复制访问条件、表过滤、range 和存储类型；随后构造 gather，设置 `Source`、`StoreType`、`TableFilters`、schema/列名，并把 scan 作为唯一子节点。
3. 索引路径首先要求 `path.Index` 存在，否则返回 `None`；它构造 `LogicalIndexScan`，计算 `is_single_scan`，并在 gather 上额外设置 `IsIndexGather = true`、`Index` 和 `IsDoubleRead = !is_single_scan`。
4. 逻辑属性推导时，`BuildKeyInfo` 先让公共基座递归处理子节点，再由 `LogicalSchemaProducer` 仅保留仍能映射到 gather 输出 schema 的子键；`DeriveStats` 通过 `BaseLogicalPlan` 在单子节点情况下直接继承子统计并缓存。
5. `PreparePossibleProperties` 有子属性时把首个子节点的 `HasTiFlash` 写入基座，并原样克隆该子节点的 `Orders` 与 `HasTiFlash`；无子属性时清空 TiFlash 标记并返回空属性。
6. 物理计划阶段通过动态下转识别 gather。经典物理路径会依据 `StoreType`/所需属性筛选候选、调用 `DeriveStats(true)`，并依据 `IsIndexGather`、`Index` 和 `Source` 生成或评估相应 reader；旧 cascades 规则 `ImplTiKVSingleReadGather` 则依据 `IsIndexGather` 选择 `GetPhysicalIndexReader` 或 `GetPhysicalTableReader`。

## 数据与状态

该节点自身不持有执行结果。稳定元数据包括来源、路径类别、索引、存储类型、双读标志和表过滤；可变派生状态位于 `LogicalSchemaProducer.BaseLogicalPlan`，包括子计划、schema/列名、统计缓存、最大一行标志和 TiFlash 可用性。

关键不变量是正常构造的 gather 恰有一个扫描子节点。`DeriveStats` 的基类逻辑只对零或一个子节点有定义，多子节点会返回错误；`BuildKeyInfo` 也只在恰有一个子节点时继承键。`PreparePossibleProperties` 明确只读第一个子属性，因此调用方应维持一元结构。

`Source` 使用共享引用而非复制整个 `DataSource`，使路径枚举、统计和后续物理化看到同一来源。`Index`、`TableFilters`、schema、列名等在构造边界按需要克隆，避免把 `RefCell` 借用跨越构造步骤保存。

## 依赖与调用关系

上游直接构造者是 `logical_datasource.rs` 中的 `buildTableGather` 与 `buildIndexGather`，二者由 `Convert2Gathers` 调用。RustCodeGraph 对 `buildTableGather` 给出的调用边为 `Convert2Gathers -> buildTableGather`，并显示其构造 `LogicalTableScan` 和 `TiKVSingleGather`、调用 `SetChildren`；索引分支在相邻源码中具有对称结构。

本文件通过 `use crate::*` 使用同 crate 的 `LogicalSchemaProducer`、`BaseLogicalPlan`、`LogicalPlan`、`DataSourceRef`、`Expression`、`PossiblePropertiesInfo`、`StatsInfo` 和 `NewBaseLogicalPlan`，并直接依赖 `base::ContextRef`、`model::IndexInfo` 与 `kv::StoreType`。这些 crate 均由同目录 `Cargo.toml` 的路径依赖声明；本文件没有 feature gate。

下游直接消费者包括：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：识别 gather、查找索引路径、筛选 TiFlash/Mpp 候选、推导统计并参与 reader/成本路径选择。
- `pkg/planner/cascades/old/implementation_rules.rs`：`ImplTiKVSingleReadGather` 读取 `Source` 与 `IsIndexGather`，选择 IndexReader 或 TableReader；缺少来源时返回规划错误。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs`：`best_probe` 从 gather 的 `Source` 继续评估 Index Join 探测路径。

## 错误处理与边界

`Init`、`ExplainInfo`、`BuildKeyInfo` 和 `PreparePossibleProperties` 不返回错误。`ExplainInfo` 对未设置 `Source` 返回 `"gather"`；只有 `IsIndexGather` 且 `Index` 为 `Some` 时才追加索引名，因此默认或不完整测试对象不会 panic。正常构造路径仍应保持 `IsIndexGather => Index.is_some()`，因为物理化需要索引 ID 才能定位访问路径。

`DeriveStats` 返回 planner `Result`。无子节点时公共基座生成一行的默认统计；单子节点时继承其统计；意外的多子节点会产生 `multi-child logical operator must implement DeriveStats`。`buildIndexGather` 在访问路径没有索引元数据时返回 `None`，从候选集合排除该路径。

共享数据源的动态借用可能在违反 `RefCell` 借用规则时 panic；当前构造代码先在局部作用域读取 `source.borrow()`，结束借用后才移动/共享 `source`，避免重叠借用。物理规则还会显式处理缺少 `Source` 的情况，例如旧 cascades 规则返回 `TiKVSingleGather has no data source`。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。`DataSourceRef = Rc<RefCell<DataSource>>` 表明该计划对象面向单线程所有权模型：`Rc` 管理共享生命周期，最后一个引用释放时数据源被回收；`RefCell` 在运行时检查共享/独占借用，而不提供跨线程同步。

扫描子节点由 `BaseLogicalPlan.children: Vec<Box<dyn LogicalPlan>>` 独占。gather 析构时其子树随所有权释放；被其他候选 gather 共享的 `DataSource` 则由引用计数决定实际释放时机。表达式与索引元数据是拥有的克隆值，本文件不需要显式清理资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_tikv_single_gather.go`。两版都内嵌 schema producer、保存来源/索引标志/索引，并实现初始化、Explain、键继承和可能属性透传。

语义对齐点：Go `BuildKeyInfo` 把唯一子 schema 的 `PKOrUK` 交给自身 schema；Rust 通过 `LogicalSchemaProducer::BuildKeyInfo` 递归构建后，按输出列映射重建这些键，独立测试验证键列映射。Go `PreparePossibleProperties` 在无/空子属性时清除 `hasTiFlash`，否则透传首个孩子的 orders 和 TiFlash；Rust 对空切片和首元素执行同样的状态变化与属性复制。Go 从基类继承 `DeriveStats`；Rust 显式委托 `BaseLogicalPlan::DeriveStats`，保持单子统计继承行为。

已确认的结构差异：Go 结构体当前只有 `Source`、`IsIndexGather`、`Index` 三个专有字段；Rust 还保存 `StoreType`、`IsDoubleRead` 和 `TableFilters`，供当前 Rust 物理路径使用。Go `ExplainInfo` 假定 `Source`/`Index` 已设置并直接解引用，Rust 使用 `Option` 并提供不完整对象的回退文本。Go `Init` 返回指针，Rust 消费并返回 `Self`；二者都替换公共基座并写入相同算子类型和查询块偏移。

## 扩展指南

- 新增访问路径元数据时，优先在 `TiKVSingleGather` 增加拥有语义明确的字段，并同步 `Default`、`logical_datasource.rs` 的表/索引两条构造路径以及所有物理消费者；不要只在一个构造分支赋值。
- 改动键、统计或排序透传逻辑时，应修改本文件对应方法，并扩展独立测试 `logical_tikv_single_gather_test.rs`；Rust 单元测试不要嵌入生产源文件。尤其要覆盖空子属性、一元子计划和 schema 列映射。
- 改动 `IsIndexGather`、`Index`、`StoreType` 或 `IsDoubleRead` 的含义时，要同步审查 `base_physical_plan.rs`、`implementation_rules.rs` 和 `index_join_probe.rs`，并与 `logical_datasource.rs` 的 `AccessPath` 到 gather 映射保持一致。
- 若希望放宽“一元算子”约束，必须先为多子统计、键合并和属性合并定义明确语义；当前基类会拒绝多子统计，不能仅调整 `SetChildren` 调用。
- 与 Go 逐提交对齐时，应分别记录 Go 增量和 Rust 为现有物理路径保留的必要字段，不能为了表面结构相同删掉 Rust 下游正在读取的字段。

兼容风险集中在 EXPLAIN 文本、候选 reader 类型和唯一键/排序属性；性能风险集中在错误的 `StoreType`、双读判定或过滤条件复制导致不当候选和额外回表。相关行为修改后应在现有独立单测之外选择覆盖表路径、覆盖索引、非覆盖索引和 TiFlash 属性的规划测试。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`explore "TiKVSingleGather logical_tikv_single_gather"`、`query TiKVSingleGather --kind struct`、`node TiKVSingleGather` 核对 Rust/Go 定义及构造边；`node buildTableGather` 与文件节点查询核对 `Convert2Gathers`、表/索引构造链和字段复制。
- 目标与基类源码：`logical_tikv_single_gather.rs`、`logical_datasource.rs:1572-1683`、`logical_schema_producer.rs:117`、`base_logical_plan.rs:374-433`、`lib.rs`。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml`。
- Go 对照：`logical_tikv_single_gather.go`、`logical_datasource.go:523-566`。
- Rust 独立测试：`logical_tikv_single_gather_test.rs` 的四个测试分别验证排序/TiFlash 属性、单子统计继承、索引 Explain 文本和键映射继承。
- 下游证据：`pkg/planner/core/operator/physicalop/base_physical_plan.rs`、`pkg/planner/cascades/old/implementation_rules.rs:615-650`、`pkg/planner/core/operator/physicalop/index_join_probe.rs:145-166`。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付前以指定命令验证目标文档恰含十一个固定二级标题，并人工复核所有行为结论均可回溯到上述符号和文件。
